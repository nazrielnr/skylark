//! Attachments (feature-inventory §1.7/§1.8): the composer's staged images,
//! the chunked upload to the chat's host device, the plain-text attachment-ref
//! transport that rides the prompt, the transcript read-back cache, and the
//! full-size preview lightbox.
//!
//! Ports of zeron's `composer/use-attachments.ts` (staging/upload),
//! `control/message-attachments.ts` (the `withAttachments` /
//! `parseUserMessageImages` text transport — attachment refs are embedded in
//! the user message's plain text, which is exactly what persists in the doc),
//! and `lib/transcript-attachment-cache.ts` (decoded-image cache keyed by
//! `(deviceId, path)`, seeded locally after a send so own bubbles never
//! round-trip).

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures::TryStreamExt as _;
use gpui::{
    AnyElement, BackgroundExecutor, Image, ImageFormat, SharedString, Size, div, prelude::*, px,
};

use crate::state::EngineHandle;
use crate::theme::ink;
use zeron_rpc::methods;

/// use-attachments.ts `MAX_ATTACHMENT_BYTES`.
pub const MAX_ATTACHMENT_BYTES: u64 = 24 * 1024 * 1024;
/// Base64 chars per `UploadChunk`, sized against the relay's hard ceiling:
/// Cloudflare caps a WebSocket message at 1 MiB, and a chunk rides one relay
/// frame (JSON envelope + uleb header add ~150 bytes) — 680 000 chars ≈
/// 510 KB binary leaves ~35% headroom. Multiple of 4 so a slice of the
/// whole-file base64 stays independently decodable. The old 60 000 (45 KB)
/// made a 3 MB screenshot ~70 sequential round trips — each one a stall
/// opportunity on a flaky link.
pub const UPLOAD_CHUNK_B64_CHARS: usize = 680_000;
/// state.ts `MAX_ATTACHMENT_READ_CHUNKS` — bounds the read-back loop.
const MAX_READ_CHUNKS: usize = 1_000;

// ---------------------------------------------------------------------------
mod transport;
pub use transport::{
    ATTACHMENT_ONLY_TEXT, ParsedUserMessage, UserImageAttachment, parse_user_message_images,
    user_message_rail_text, with_attachments,
};
mod staging;
pub use staging::{
    StagedAttachment, ensure_extension, format_by_extension, stage_clipboard_image, stage_file,
    stage_png_bytes,
};
mod upload;
pub use upload::{
    LoadedAttachmentImage, call_with_timeout, queue_thumbnail_image, read_attachment_image,
    upload_attachment,
};
mod cache;
pub use cache::*;
mod lightbox;
pub use lightbox::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appshot_cards_follow_their_exact_image_reference() {
        let shot = crate::appshots::tests::shot();
        let paths = HashMap::from([(shot.screenshot.id.clone(), "/remote/a & b.png".to_string())]);
        let body = crate::appshots::with_appshots("Look here", &[shot], &paths);
        let message = with_attachments(
            &body,
            &["/remote/ordinary.png".into(), "/remote/a & b.png".into()],
        );
        let parsed = parse_user_message_images(&message);
        assert_eq!(parsed.text, "Look here");
        assert!(parsed.attachments[0].appshot.is_none());
        let appshot = parsed.attachments[1].appshot.as_ref().unwrap();
        assert_eq!(appshot.app_name, "Safari & Notes");
        assert_eq!(appshot.title(), "A \"window\"");
    }

    #[test]
    fn duplicate_or_invalid_appshot_metadata_stays_an_ordinary_attachment() {
        let body = format!(
            "Question\n\n{}\n<appshot app=\"One\" image=\"/a.png\">private text</appshot><appshot app=\"Two\" image=\"/a.png\">other text</appshot>",
            crate::appshots::CONTEXT_MARKER
        );
        let parsed = parse_user_message_images(&with_attachments(&body, &["/a.png".into()]));
        assert!(parsed.attachments[0].appshot.is_none());
        assert_eq!(parsed.text, "Question");
    }

    #[test]
    fn queue_images_have_a_small_retained_pixel_budget() {
        let source = image::DynamicImage::new_rgba8(2400, 1600);
        let mut bytes = std::io::Cursor::new(Vec::new());
        source
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        let source = Image::from_bytes(ImageFormat::Png, bytes.into_inner());
        let thumbnail = queue_thumbnail_image(&source).unwrap();
        let (width, height) = crate::appshots::png_dimensions(&thumbnail.bytes).unwrap();
        assert!(width <= 160 && height <= 112);
        assert!(thumbnail.bytes.len() < 160 * 112 * 4);
    }

    #[test]
    fn with_attachments_round_trips_through_parse() {
        let paths = vec!["/data/uploads/ab-cat.png".to_string(), "/x/dog.jpg".into()];
        let content = with_attachments("look at these", &paths);
        let parsed = parse_user_message_images(&content);
        assert_eq!(parsed.text, "look at these");
        assert_eq!(parsed.attachments.len(), 2);
        assert_eq!(parsed.attachments[0].path, "/data/uploads/ab-cat.png");
        assert_eq!(parsed.attachments[0].name, "ab-cat.png");
        assert_eq!(parsed.attachments[1].name, "dog.jpg");
        assert_eq!(parsed.attachments[0].id, "0:/data/uploads/ab-cat.png");
    }

    #[test]
    fn image_only_send_hides_placeholder_body() {
        let content = with_attachments("", &["/a/b.png".to_string()]);
        assert!(content.starts_with(ATTACHMENT_ONLY_TEXT));
        let parsed = parse_user_message_images(&content);
        assert_eq!(parsed.text, "");
        assert_eq!(parsed.attachments.len(), 1);
    }

    #[test]
    fn appshot_context_is_hidden_but_image_remains() {
        let body = format!(
            "Fix the layout\n\n{}\n<appshot app=\"Safari\">secret AX text</appshot>",
            crate::appshots::CONTEXT_MARKER
        );
        let content = with_attachments(&body, &["/a/appshot.png".to_string()]);
        let parsed = parse_user_message_images(&content);
        assert_eq!(parsed.text, "Fix the layout");
        assert_eq!(parsed.attachments.len(), 1);
        assert_eq!(parsed.attachments[0].path, "/a/appshot.png");
    }

    #[test]
    fn plain_text_passes_through_unchanged() {
        assert_eq!(with_attachments("hello", &[]), "hello");
        let parsed = parse_user_message_images("hello\n\nno images here");
        assert!(parsed.attachments.is_empty());
        assert_eq!(parsed.text, "hello\n\nno images here");
    }

    #[test]
    fn marker_is_case_insensitive_and_requires_ref_lines() {
        let parsed = parse_user_message_images(
            "hi\n\nATTACHED IMAGES (local files — open them to view):\n- /p/q.png",
        );
        assert_eq!(parsed.attachments.len(), 1);
        // A trailer with no valid `- path` lines is left as plain text.
        let empty = parse_user_message_images(
            "hi\n\nAttached images (local files — open them to view):\nnothing",
        );
        assert!(empty.attachments.is_empty());
        assert!(empty.text.contains("Attached images"));
    }

    #[test]
    fn rail_text_summarizes_image_only_sends() {
        let one = with_attachments("", &["/a/b.png".to_string()]);
        assert_eq!(user_message_rail_text(&one), "Attached image");
        let two = with_attachments("", &["/a/b.png".to_string(), "/c/d.png".into()]);
        assert_eq!(user_message_rail_text(&two), "2 attached images");
        let with_text = with_attachments("fix this", &["/a/b.png".to_string()]);
        assert_eq!(user_message_rail_text(&with_text), "fix this");
        assert_eq!(user_message_rail_text("plain"), "plain");
    }

    #[test]
    fn ensure_extension_matches_browser_heuristic() {
        assert_eq!(ensure_extension("shot.png", ImageFormat::Png), "shot.png");
        assert_eq!(ensure_extension("image", ImageFormat::Png), "image.png");
        assert_eq!(
            ensure_extension("photo.j", ImageFormat::Jpeg),
            "photo.j.jpg"
        );
        assert_eq!(
            ensure_extension("archive.tar.gz", ImageFormat::Png),
            "archive.tar.gz"
        );
    }

    #[test]
    fn supported_formats_match_engine_jail() {
        for (ext, expect) in [
            ("png", Some(ImageFormat::Png)),
            ("JPG", Some(ImageFormat::Jpeg)),
            ("webp", Some(ImageFormat::Webp)),
            ("svg", Some(ImageFormat::Svg)),
            ("ico", None),
            ("txt", None),
        ] {
            assert_eq!(
                format_by_extension(Path::new(&format!("f.{ext}"))),
                expect,
                "ext {ext}"
            );
        }
    }

    #[test]
    fn retry_ladder_is_2s_doubling_capped_at_15s() {
        assert_eq!(retry_delay(0), Duration::from_millis(2_000));
        assert_eq!(retry_delay(1), Duration::from_millis(4_000));
        assert_eq!(retry_delay(2), Duration::from_millis(8_000));
        assert_eq!(retry_delay(3), Duration::from_millis(15_000));
        assert_eq!(retry_delay(9), Duration::from_millis(15_000));
    }

    #[test]
    fn upload_chunk_fits_the_relay_frame_ceiling() {
        // Cloudflare caps a WebSocket message at 1 MiB; the chunk rides one
        // relay frame with a small JSON envelope + uleb header.
        assert!(UPLOAD_CHUNK_B64_CHARS + 1_024 < 1_048_576);
        // A slice of the whole-file base64 must stay independently decodable.
        assert_eq!(UPLOAD_CHUNK_B64_CHARS % 4, 0);
    }

    #[test]
    fn chunk_ranges_cover_the_buffer_exactly() {
        // Empty file: one empty chunk (the commit needs the id staged).
        assert_eq!(chunk_ranges(0), vec![(0, 0..0)]);
        // Exact multiple: no trailing empty chunk.
        let exact = chunk_ranges(UPLOAD_CHUNK_B64_CHARS * 2);
        assert_eq!(exact.len(), 2);
        assert_eq!(
            exact[1],
            (1, UPLOAD_CHUNK_B64_CHARS..UPLOAD_CHUNK_B64_CHARS * 2)
        );
        // Partial tail.
        let partial = chunk_ranges(UPLOAD_CHUNK_B64_CHARS + 7);
        assert_eq!(partial.len(), 2);
        assert_eq!(
            partial[1],
            (1, UPLOAD_CHUNK_B64_CHARS..UPLOAD_CHUNK_B64_CHARS + 7)
        );
        // Ranges tile the buffer: contiguous, in order, fully covering.
        let mut expected_start = 0;
        for (seq, range) in &partial {
            assert_eq!(range.start, expected_start, "seq {seq} contiguous");
            expected_start = range.end;
        }
        assert_eq!(expected_start, UPLOAD_CHUNK_B64_CHARS + 7);
    }

    #[test]
    fn attachment_deadline_scales_and_caps() {
        // A one-chunk screenshot fails within ~2 minutes, not hours.
        assert_eq!(attachment_deadline(1), Duration::from_secs(135));
        // A max-size upload is still bounded.
        assert_eq!(attachment_deadline(1_000), Duration::from_secs(900));
    }
}

#[cfg(test)]
mod generated_image_tests {
    use super::*;

    #[test]
    fn generated_image_cache_isolates_policy_aliases_and_load_claims() {
        let owner = "policy-audit-owner";
        let path = "/uploads/policy01-image.png";
        let png = AttachmentKey::new(owner, path, Some("image/png"));
        let gif = AttachmentKey::new(owner, path, Some("image/gif"));
        let raw = Arc::new(Image::from_bytes(ImageFormat::Gif, b"GIF89a".to_vec()));
        seed_attachment(owner, path, "image.gif", raw.clone());
        seed_attachment_alias(owner, "policy01", "image.gif", raw.clone());
        assert!(matches!(
            attachment_snapshot_for(&png),
            AttachmentSnapshot::Loading
        ));
        let alias = AttachmentKey::new(owner, "/another/policy01-image.png", Some("image/png"));
        assert!(matches!(
            attachment_snapshot_for(&alias),
            AttachmentSnapshot::Loading
        ));
        assert!(begin_load_for(&png));
        assert!(!begin_load_for(&png));
        assert!(begin_load_for(&gif));
        drop(AttachmentLoadGuard(png.clone()));
        assert!(matches!(
            attachment_snapshot_for(&png),
            AttachmentSnapshot::Error { .. }
        ));
        assert!(matches!(
            attachment_snapshot_for(&gif),
            AttachmentSnapshot::Loading
        ));
        store_loaded_for(&gif, "image.gif".into(), raw);
        assert!(matches!(
            attachment_snapshot_for(&png),
            AttachmentSnapshot::Error { .. }
        ));
        assert!(matches!(
            attachment_snapshot_for(&gif),
            AttachmentSnapshot::Loaded(_)
        ));
        let changed = AttachmentKey::new(owner, path, Some("image/jpeg"));
        assert!(matches!(
            attachment_snapshot_for(&changed),
            AttachmentSnapshot::Loading
        ));
    }

    #[test]
    fn generated_image_history_is_evicted_with_decoded_memory_accounting() {
        let mut cache = ImageCache::default();
        // Cache accounting reads only IHDR. No large allocations are needed
        // to exercise the production eviction policy across a long history.
        for i in 0..100 {
            let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
            bytes.extend_from_slice(&2048_u32.to_be_bytes());
            bytes.extend_from_slice(&2048_u32.to_be_bytes());
            cache.insert_loaded(
                AttachmentKey::new("bounded-history", &format!("/{i}.png"), Some("image/png")),
                CachedAttachmentImage {
                    name: "generated.png".into(),
                    image: Arc::new(Image::from_bytes(ImageFormat::Png, bytes)),
                },
            );
            assert!(cache.loaded_bytes <= IMAGE_CACHE_BUDGET_BYTES);
            // The real render loop calls flush_evicted to release these CPU/GPU assets.
            cache.pending_free.clear();
        }
        assert!(!cache.map.contains_key(&AttachmentKey::new(
            "bounded-history",
            "/0.png",
            Some("image/png")
        )));
        assert!(cache.map.contains_key(&AttachmentKey::new(
            "bounded-history",
            "/99.png",
            Some("image/png")
        )));
    }

    #[test]
    fn generated_image_decoder_checks_actual_type_and_downsamples() {
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgba8(3072, 16)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let bytes = png.into_inner();
        assert!(
            crate::image_media::decode_generated_image(
                bytes.clone(),
                "image/jpeg",
                MAX_ATTACHMENT_BYTES as usize
            )
            .is_err()
        );
        let decoded = crate::image_media::decode_generated_image(
            bytes,
            "image/png",
            MAX_ATTACHMENT_BYTES as usize,
        )
        .unwrap();
        assert_eq!(decoded.width, 2048.0);
        assert!(decoded.bytes < IMAGE_CACHE_BUDGET_BYTES);
        assert!(
            crate::image_media::decode_generated_image(
                b"GIF89a".to_vec(),
                "image/gif",
                MAX_ATTACHMENT_BYTES as usize
            )
            .is_err()
        );
    }

    #[test]
    fn generated_image_animation_retains_only_first_static_frame() {
        let mut bytes = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
            for color in [[255, 0, 0, 255], [0, 0, 255, 255]] {
                encoder
                    .encode_frame(image::Frame::new(image::RgbaImage::from_pixel(
                        16,
                        16,
                        image::Rgba(color),
                    )))
                    .unwrap();
            }
        }
        let decoded = crate::image_media::decode_generated_image(
            bytes,
            "image/gif",
            MAX_ATTACHMENT_BYTES as usize,
        )
        .unwrap();
        assert_eq!(
            image::guess_format(&decoded.image.bytes).unwrap(),
            image::ImageFormat::Png
        );
        let pixels = image::load_from_memory(&decoded.image.bytes)
            .unwrap()
            .to_rgba8();
        assert_eq!(pixels.get_pixel(0, 0).0, [255, 0, 0, 255]);
    }

    struct ImageRpc {
        calls: Arc<Mutex<Vec<serde_json::Value>>>,
        bytes: Vec<u8>,
    }

    #[async_trait::async_trait]
    impl zeron_rpc::RpcService for ImageRpc {
        async fn handle(
            &self,
            method: &str,
            params: serde_json::Value,
        ) -> Result<zeron_rpc::RpcReply, zeron_rpc::RpcError> {
            assert_eq!(method, methods::READ_ATTACHMENT_CHUNK);
            self.calls.lock().unwrap().push(params);
            zeron_rpc::RpcReply::value(
                &serde_json::json!({"name":"generated.png", "mimeType":"image/png", "data":BASE64.encode(&self.bytes), "nextOffset": self.bytes.len(), "done":true}),
            )
        }
    }

    #[tokio::test]
    async fn generated_image_chunk_reader_targets_owner_and_bounds_decode() {
        let executor = gpui_platform::background_executor();
        for (width, target, expected, succeeds) in [
            (64, Some("remote-owner"), "image/png", true),
            (64, None, "image/png", true),
            (4097, None, "image/png", false),
            (64, None, "image/gif", false),
        ] {
            let mut png = std::io::Cursor::new(Vec::new());
            image::DynamicImage::new_rgba8(width, 1)
                .write_to(&mut png, image::ImageFormat::Png)
                .unwrap();
            let calls = Arc::new(Mutex::new(vec![]));
            let engine =
                EngineHandle::from_test_client(zeron_rpc::memory_client(Arc::new(ImageRpc {
                    calls: calls.clone(),
                    bytes: png.into_inner(),
                })));
            let loaded = read_attachment_image(
                &engine,
                &executor,
                target,
                "/profile/uploads/image.png",
                Some(expected),
            )
            .await;
            assert_eq!(loaded.is_some(), succeeds);
            let calls = calls.lock().unwrap();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0]["path"], "/profile/uploads/image.png");
            assert_eq!(
                calls[0].get("targetDeviceId").and_then(|v| v.as_str()),
                target
            );
        }
    }

    #[test]
    fn generated_image_cancelled_load_releases_claim_without_losing_completed_images() {
        let owner = "cancelled-generated-owner";
        let path = "/fixture/cancelled-generated.png";
        assert!(begin_load(owner, path));
        drop(AttachmentLoadGuard(key(owner, path)));
        assert!(matches!(
            attachment_snapshot(owner, path),
            AttachmentSnapshot::Error { .. }
        ));
        let image = Arc::new(Image::from_bytes(ImageFormat::Png, Vec::new()));
        store_loaded(owner, path, "generated.png".into(), image);
        drop(AttachmentLoadGuard(key(owner, path)));
        assert!(matches!(
            attachment_snapshot(owner, path),
            AttachmentSnapshot::Loaded(_)
        ));
    }

    #[test]
    fn generated_image_decode_accepts_attachment_byte_cap_but_rejects_excess() {
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgba8(1, 1)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let mut bytes = png.into_inner();
        // Padding represents a large source with small dimensions; the intake
        // cap is 24 MiB, independent of the workspace preview's 8 MiB cap.
        bytes.resize(9 * 1024 * 1024, 0);
        assert!(
            crate::image_media::decode_raster_image(bytes.clone(), MAX_ATTACHMENT_BYTES as usize)
                .is_ok()
        );
        assert!(crate::image_media::decode_raster_image(bytes, 8 * 1024 * 1024).is_err());
    }
}
