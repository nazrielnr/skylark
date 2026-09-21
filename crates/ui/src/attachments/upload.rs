//! Attachment upload services.

use super::*;

fn name_from_path(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("image")
        .to_string()
}

// Upload (state.ts uploadAttachment) + read-back (state.ts readAttachmentImage)
// ---------------------------------------------------------------------------

fn with_target(mut params: serde_json::Value, target_device_id: Option<&str>) -> serde_json::Value {
    if let (Some(target), Some(map)) = (target_device_id, params.as_object_mut()) {
        map.insert("targetDeviceId".into(), target.into());
    }
    params
}

/// Per-call deadlines (desktop state.ts): a stalled-but-open relay link never
/// fails an RPC on its own, so every attachment call races a timer. The first
/// chunk gets 90s (a cold dial to a remote device), later chunks 30s; commit
/// 150s (it must outlast the engine's cross-device assemble); reads 20s.
const FIRST_CHUNK_TIMEOUT: Duration = Duration::from_secs(90);
const CHUNK_TIMEOUT: Duration = Duration::from_secs(30);
const COMMIT_TIMEOUT: Duration = Duration::from_secs(150);
const READ_CHUNK_TIMEOUT: Duration = Duration::from_secs(20);

/// Race an RPC against `timeout` on the gpui background executor (these
/// futures run under `cx.spawn`, so tokio's timer reactor isn't available).
pub async fn call_with_timeout(
    engine: &EngineHandle,
    executor: &BackgroundExecutor,
    method: &str,
    params: serde_json::Value,
    timeout: Duration,
) -> Result<serde_json::Value, String> {
    let call = engine.client().call(method, params);
    let timer = executor.timer(timeout);
    futures::pin_mut!(call);
    match futures::future::select(call, timer).await {
        futures::future::Either::Left((result, _)) => result.map_err(|e| e.to_string()),
        futures::future::Either::Right(_) => Err(format!("{method} timed out")),
    }
}

/// Chunks in flight at once. `seq` slots are idempotent engine-side, so
/// completion order doesn't matter; a small window hides per-chunk latency
/// without flooding the relay socket.
const UPLOAD_CONCURRENCY: usize = 3;

/// Whole-attachment deadline. Per-chunk timeouts + retries bound each CALL,
/// but on a flapping link chunks that succeed on attempt 2-of-3 never trip
/// the 3-consecutive-failure abort — an upload could lawfully crawl for hours
/// reading "Sending…" (2026-08-18 user report). Scaled with size, capped:
/// past this, fail the send with the banner instead of spinning.
pub(crate) fn attachment_deadline(n_chunks: usize) -> Duration {
    Duration::from_secs((120 + 15 * n_chunks as u64).min(900))
}

/// The `(seq, b64 byte-range)` plan for a file's chunks. An empty file still
/// sends one empty chunk (the commit needs the uploadId staged).
pub(crate) fn chunk_ranges(b64_len: usize) -> Vec<(u64, std::ops::Range<usize>)> {
    let mut ranges = Vec::new();
    let mut start = 0usize;
    let mut seq = 0u64;
    loop {
        let end = (start + UPLOAD_CHUNK_B64_CHARS).min(b64_len);
        ranges.push((seq, start..end));
        start = end;
        seq += 1;
        if start >= b64_len {
            break;
        }
    }
    ranges
}

/// Chunked upload: base64 the bytes, `UploadChunk{uploadId,seq,data}` per
/// [`UPLOAD_CHUNK_B64_CHARS`] slice (positional `seq` makes the cheap retry
/// idempotent), a few chunks in flight at once, then
/// `UploadCommit{uploadId,fileName}` → the durable absolute path on the target
/// device. The caller mints `upload_id` — the queued-attachment flow derives
/// its `pending://` refs from the same identity before the bytes move.
/// `progress` (when given) accumulates uploaded BINARY bytes — the
/// composer's "Uploading… N%" reads it every paint. Errors return the raw
/// cause (the composer shows friendly copy).
pub async fn upload_attachment(
    engine: &EngineHandle,
    executor: &BackgroundExecutor,
    target_device_id: Option<&str>,
    upload_id: &str,
    attachment: &StagedAttachment,
    progress: Option<Arc<std::sync::atomic::AtomicU64>>,
) -> Result<String, String> {
    let b64 = BASE64.encode(attachment.bytes());
    let ranges = chunk_ranges(b64.len());
    let deadline = executor.timer(attachment_deadline(ranges.len()));
    let upload = async {
        futures::stream::iter(ranges.iter().cloned().map(Ok::<_, String>))
            .try_for_each_concurrent(UPLOAD_CONCURRENCY, |(seq, range)| {
                let progress = progress.clone();
                let upload_id = &upload_id;
                let b64 = &b64;
                async move {
                    let params = with_target(
                        serde_json::json!({
                            "uploadId": upload_id,
                            "seq": seq,
                            "data": &b64[range.clone()],
                        }),
                        target_device_id,
                    );
                    // The first WINDOW (not just seq 0) gets the cold-dial
                    // allowance — its chunks all start before the link is warm.
                    let timeout = if seq < UPLOAD_CONCURRENCY as u64 {
                        FIRST_CHUNK_TIMEOUT
                    } else {
                        CHUNK_TIMEOUT
                    };
                    // One transient blip must not abort the upload; `seq`
                    // slots are idempotent engine-side, so a blind re-send is
                    // safe (timeouts retry too).
                    let mut attempt = 0u32;
                    loop {
                        match call_with_timeout(
                            engine,
                            executor,
                            methods::UPLOAD_CHUNK,
                            params.clone(),
                            timeout,
                        )
                        .await
                        {
                            Ok(_) => break,
                            Err(err) if attempt < 2 => {
                                attempt += 1;
                                // warn, not debug: the 2026-08-19 incident
                                // ground through silent timeout/retry cycles
                                // for minutes with a literally empty log —
                                // degraded uploads must narrate.
                                tracing::warn!(error = %err, seq, attempt, "upload chunk retry");
                                // Stagger by seq so parallel chunks that failed
                                // together don't re-collide in lockstep.
                                executor
                                    .timer(Duration::from_millis(50 * (attempt as u64) * (seq + 1)))
                                    .await;
                            }
                            Err(err) => return Err(err),
                        }
                    }
                    if let Some(progress) = &progress {
                        // b64 → binary bytes (final chunk's padding rounds up
                        // by ≤2 bytes — irrelevant for a percentage).
                        progress.fetch_add(
                            (range.len() * 3 / 4) as u64,
                            std::sync::atomic::Ordering::Relaxed,
                        );
                    }
                    Ok(())
                }
            })
            .await?;
        let params = with_target(
            serde_json::json!({ "uploadId": upload_id, "fileName": attachment.name }),
            target_device_id,
        );
        let reply = call_with_timeout(
            engine,
            executor,
            methods::UPLOAD_COMMIT,
            params,
            COMMIT_TIMEOUT,
        )
        .await?;
        reply
            .get("path")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| "upload commit returned no path".to_string())
    };
    futures::pin_mut!(upload);
    match futures::future::select(upload, deadline).await {
        futures::future::Either::Left((result, _)) => result,
        futures::future::Either::Right(_) => Err(format!(
            "attachment upload exceeded {}s",
            attachment_deadline(ranges.len()).as_secs()
        )),
    }
}

/// A transcript image read back from the owning device.
pub struct LoadedAttachmentImage {
    pub name: String,
    pub image: Arc<Image>,
}

/// `ReadAttachmentChunk` loop: 45KB base64 chunks until `done` (bounded, with
/// the same stuck-offset guard as zeron's `readAttachmentImage`).
pub async fn read_attachment_image(
    engine: &EngineHandle,
    executor: &BackgroundExecutor,
    target_device_id: Option<&str>,
    path: &str,
    expected_raster_mime: Option<&str>,
) -> Option<LoadedAttachmentImage> {
    let mut name = String::new();
    let mut mime = String::new();
    let mut b64 = String::new();
    let mut offset = 0u64;
    let mut done = false;
    for _ in 0..MAX_READ_CHUNKS {
        let params = with_target(
            serde_json::json!({ "path": path, "offset": offset }),
            target_device_id,
        );
        let chunk = call_with_timeout(
            engine,
            executor,
            methods::READ_ATTACHMENT_CHUNK,
            params,
            READ_CHUNK_TIMEOUT,
        )
        .await
        .ok()?;
        name = chunk.get("name")?.as_str()?.to_string();
        mime = chunk.get("mimeType")?.as_str()?.to_string();
        if expected_raster_mime.is_some_and(|expected| expected != mime) {
            return None;
        }
        let data = chunk.get("data")?.as_str()?;
        if expected_raster_mime.is_some()
            && b64.len().saturating_add(data.len())
                > (MAX_ATTACHMENT_BYTES as usize).div_ceil(3) * 4
        {
            return None;
        }
        b64.push_str(data);
        done = chunk.get("done")?.as_bool()?;
        if done {
            break;
        }
        let next = chunk.get("nextOffset")?.as_u64()?;
        if next <= offset {
            return None;
        }
        offset = next;
    }
    if !done || b64.is_empty() {
        return None;
    }
    let bytes = BASE64.decode(b64.as_bytes()).ok()?;
    let image = if let Some(expected) = expected_raster_mime {
        if expected != mime
            || !matches!(
                mime.as_str(),
                "image/png" | "image/jpeg" | "image/webp" | "image/gif"
            )
        {
            return None;
        }
        let expected = expected.to_owned();
        executor
            .spawn(async move {
                crate::image_media::decode_generated_image(
                    bytes,
                    &expected,
                    MAX_ATTACHMENT_BYTES as usize,
                )
                .ok()
                .map(|media| media.image)
            })
            .await?
    } else {
        let format = ImageFormat::from_mime_type(&mime).unwrap_or(ImageFormat::Png);
        Arc::new(Image::from_bytes(format, bytes))
    };
    Some(LoadedAttachmentImage {
        name: if name.is_empty() {
            name_from_path(path)
        } else {
            name
        },
        image,
    })
}

/// Decode with a fixed allocation budget and retain only a small queue image.
/// Full resolution is fetched on explicit preview, never retained by queue rows.
pub fn queue_thumbnail_image(source: &Image) -> Option<Arc<Image>> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(source.bytes.as_slice()))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let thumb = reader.decode().ok()?.thumbnail(160, 112);
    let mut bytes = std::io::Cursor::new(Vec::new());
    thumb.write_to(&mut bytes, image::ImageFormat::Png).ok()?;
    Some(Arc::new(Image::from_bytes(
        ImageFormat::Png,
        bytes.into_inner(),
    )))
}

// ---------------------------------------------------------------------------
