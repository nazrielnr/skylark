//! Attachment cache services.

use super::*;

// Transcript image cache (transcript-attachment-cache.ts)
// ---------------------------------------------------------------------------

/// A decoded transcript image, ready for `img(...)`.
#[derive(Clone)]
pub struct CachedAttachmentImage {
    pub name: SharedString,
    pub image: Arc<Image>,
}

/// What a render pass sees for one `(deviceId, path)` source.
#[derive(Clone)]
pub enum AttachmentSnapshot {
    Loading,
    Loaded(CachedAttachmentImage),
    /// Load failed; `retry_in` is how long until [`begin_load`] would hand out
    /// another attempt (the exponential 2s→15s ladder from user-attachments.tsx).
    Error {
        retry_in: Duration,
    },
}

enum CacheEntry {
    Loading {
        attempts: u32,
    },
    Loaded {
        image: CachedAttachmentImage,
        bytes: usize,
        last_used: u64,
    },
    Error {
        attempts: u32,
        at: Instant,
    },
}

fn retry_delay(attempts: u32) -> Duration {
    Duration::from_millis((2_000u64 << attempts.min(3)).min(15_000))
}

/// Retained encoded bytes plus estimated CPU/GPU pixels for normalized PNGs.
/// Generated images are always evictable and individually fit this budget.
/// Legacy user attachments retain their existing protection behavior.
const IMAGE_CACHE_BUDGET_BYTES: usize = 64 * 1024 * 1024;

/// Validation policy is part of identity, including in-flight loads and errors.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct AttachmentKey {
    device: String,
    path: String,
    raster_mime: Option<String>,
}

impl AttachmentKey {
    pub(crate) fn new(device: &str, path: &str, mime: Option<&str>) -> Self {
        Self {
            device: device.into(),
            path: path.into(),
            raster_mime: mime.map(str::to_owned),
        }
    }
}

#[derive(Default)]
pub(crate) struct ImageCache {
    pub(crate) map: HashMap<AttachmentKey, CacheEntry>,
    /// Monotonic access clock for LRU ordering.
    pub(crate) tick: u64,
    pub(crate) loaded_bytes: usize,
    pub(crate) generated_bytes: usize,
    /// Evicted images awaiting `flush_evicted` (freeing needs `&mut App`,
    /// which eviction sites — async load completions — don't always have).
    pub(crate) pending_free: Vec<Arc<Image>>,
}

impl ImageCache {
    pub(crate) fn insert_loaded(&mut self, key: AttachmentKey, image: CachedAttachmentImage) {
        // Generated rasters are normalized to PNG; account for their decoded
        // CPU/GPU copies as well as encoded bytes in the existing cache budget.
        let pixels = crate::appshots::png_dimensions(&image.image.bytes)
            .map_or(0, |(w, h)| (w as usize).saturating_mul(h as usize));
        let bytes = image
            .image
            .bytes
            .len()
            .saturating_add(pixels.saturating_mul(8));
        let generated = key.raster_mime.is_some();
        self.tick += 1;
        if let Some(CacheEntry::Loaded { image, bytes, .. }) = self.map.insert(
            key.clone(),
            CacheEntry::Loaded {
                image,
                bytes,
                last_used: self.tick,
            },
        ) {
            self.loaded_bytes = self.loaded_bytes.saturating_sub(bytes);
            if generated {
                self.generated_bytes = self.generated_bytes.saturating_sub(bytes);
            }
            self.pending_free.push(image.image);
        }
        self.loaded_bytes = self.loaded_bytes.saturating_add(bytes);
        if generated {
            self.generated_bytes = self.generated_bytes.saturating_add(bytes);
        }
        let shielded = protected().lock().unwrap().clone();
        // Separate budgets prevent protected legacy attachments from repeatedly
        // evicting visible generated previews, or vice versa.
        while (if generated {
            self.generated_bytes
        } else {
            self.loaded_bytes.saturating_sub(self.generated_bytes)
        }) > IMAGE_CACHE_BUDGET_BYTES
        {
            let oldest = self
                .map
                .iter()
                .filter(|(k, _)| {
                    **k != key
                        && k.raster_mime.is_some() == generated
                        && (k.raster_mime.is_some()
                            || !shielded.contains(&(k.device.clone(), k.path.clone())))
                })
                .filter_map(|(k, e)| match e {
                    CacheEntry::Loaded { last_used, .. } => Some((*last_used, k.clone())),
                    _ => None,
                })
                .min_by_key(|(tick, _)| *tick);
            let Some((_, evict_key)) = oldest else { break };
            if let Some(CacheEntry::Loaded { image, bytes, .. }) = self.map.remove(&evict_key) {
                self.loaded_bytes = self.loaded_bytes.saturating_sub(bytes);
                if generated {
                    self.generated_bytes = self.generated_bytes.saturating_sub(bytes);
                }
                self.pending_free.push(image.image);
            }
        }
    }
}

fn cache() -> &'static Mutex<ImageCache> {
    static CACHE: OnceLock<Mutex<ImageCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(ImageCache::default()))
}

/// Keys shielded from LRU eviction — the open transcript's attachments. The
/// gpui list caches rendered rows across frames, so a VISIBLE thumbnail's
/// `last_used` tick can go stale and budget pressure evicted images still on
/// screen (user report: "images unload before they are scrolled out of
/// view"). The transcript replaces this set on every row sync; other chats'
/// images stay evictable, so the budget still bounds the cache overall.
fn protected() -> &'static Mutex<std::collections::HashSet<(String, String)>> {
    static PROTECTED: OnceLock<Mutex<std::collections::HashSet<(String, String)>>> =
        OnceLock::new();
    PROTECTED.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

/// Replace the eviction shield with the given keys (see [`protected`]).
pub fn protect_attachments(keys: std::collections::HashSet<(String, String)>) {
    *protected().lock().unwrap() = keys;
}

fn key(device_id: &str, path: &str) -> AttachmentKey {
    AttachmentKey::new(device_id, path, None)
}

pub fn attachment_snapshot(device_id: &str, path: &str) -> AttachmentSnapshot {
    attachment_snapshot_for(&key(device_id, path))
}

pub(crate) fn attachment_snapshot_for(source: &AttachmentKey) -> AttachmentSnapshot {
    let (device_id, path) = (source.device.as_str(), source.path.as_str());
    let mut cache = cache().lock().unwrap();
    let tick = {
        cache.tick += 1;
        cache.tick
    };
    match cache.map.get_mut(source) {
        Some(CacheEntry::Loaded {
            image, last_used, ..
        }) => {
            *last_used = tick;
            AttachmentSnapshot::Loaded(image.clone())
        }
        Some(CacheEntry::Error { attempts, at }) => AttachmentSnapshot::Error {
            retry_in: retry_delay(attempts.saturating_sub(1)).saturating_sub(at.elapsed()),
        },
        Some(CacheEntry::Loading { .. }) => AttachmentSnapshot::Loading,
        None => {
            // Queued-send alias: the host materializes `pending://{id}/{name}`
            // at `{uploads}/{id8}-{name}` and rewrites the persisted ref to
            // that ABSOLUTE path — one the sender can't know up front (it's
            // the host's disk). The id8 basename prefix IS derivable though,
            // so the send seeds the bytes under an alias and this fallback
            // resolves the rewritten ref instantly instead of blanking the
            // thumbnail into a skeleton while the bytes round-trip
            // (2026-08-19 "photo disappears after it finishes sending").
            if let Some(image) = source
                .raster_mime
                .is_none()
                .then(|| upload_alias_id8(path))
                .flatten()
                .and_then(|id8| match cache.map.get(&alias_key(device_id, &id8)) {
                    Some(CacheEntry::Loaded { image, .. }) => Some(image.clone()),
                    _ => None,
                })
            {
                cache.insert_loaded(key(device_id, path), image.clone());
                return AttachmentSnapshot::Loaded(image);
            }
            AttachmentSnapshot::Loading
        }
    }
}

/// The uploadId fragment a committed upload's basename starts with
/// (`{id8}-{name}` per the engine's `Uploads::pending_target`). `None` when
/// the path can't be a committed upload.
fn upload_alias_id8(path: &str) -> Option<String> {
    let base = std::path::Path::new(path).file_name()?.to_str()?;
    let (id8, _) = base.split_at_checked(8)?;
    (base.as_bytes().get(8) == Some(&b'-') && id8.bytes().all(|b| b.is_ascii_alphanumeric()))
        .then(|| id8.to_string())
}

fn alias_key(device_id: &str, id8: &str) -> AttachmentKey {
    key(device_id, &format!("upload-alias://{id8}"))
}

/// Seed the just-sent image under its upload identity so the persisted
/// message's rewritten absolute ref (host-side path) resolves from the same
/// local bytes — see the alias fallback in [`attachment_snapshot`].
pub fn seed_attachment_alias(device_id: &str, upload_id: &str, name: &str, image: Arc<Image>) {
    let id8: String = upload_id.chars().take(8).collect();
    let source = alias_key(device_id, &id8);
    store_loaded_for(&source, name.to_string().into(), image);
}

/// Release gpui's decoded copies of evicted images: the asset-system entry
/// AND the sprite-atlas tiles (`ImageSource::evict` — `remove_asset` alone
/// left the tiles resident forever). Pass the window being updated when
/// calling from a render path, since that window is detached from
/// `App::windows` during its own update. Cheap when nothing was evicted.
pub fn flush_evicted(mut window: Option<&mut gpui::Window>, cx: &mut gpui::App) {
    let evicted = std::mem::take(&mut cache().lock().unwrap().pending_free);
    for image in evicted {
        gpui::ImageSource::Image(image).evict(window.as_deref_mut(), cx);
    }
}

/// Claim the load for a source: `true` ⇒ the caller should start fetching now
/// (the entry is marked Loading so concurrent renders don't double-fetch).
/// Errored sources hand out a retry only after their backoff has elapsed.
pub fn begin_load(device_id: &str, path: &str) -> bool {
    begin_load_for(&key(device_id, path))
}

pub(crate) fn begin_load_for(source: &AttachmentKey) -> bool {
    let mut cache = cache().lock().unwrap();
    let entry = cache.map.entry(source.clone());
    match entry {
        std::collections::hash_map::Entry::Vacant(v) => {
            v.insert(CacheEntry::Loading { attempts: 0 });
            true
        }
        std::collections::hash_map::Entry::Occupied(mut o) => match o.get() {
            CacheEntry::Error { attempts, at }
                if at.elapsed() >= retry_delay(attempts.saturating_sub(1)) =>
            {
                let attempts = *attempts;
                o.insert(CacheEntry::Loading { attempts });
                true
            }
            _ => false,
        },
    }
}

/// A cancelled view/task must release its claim so reopening can retry it.
/// Completed loads are left alone, including completions waiting on a UI notify.
pub(crate) struct AttachmentLoadGuard(pub AttachmentKey);

impl Drop for AttachmentLoadGuard {
    fn drop(&mut self) {
        let mut cache = cache().lock().unwrap();
        if let Some(entry @ CacheEntry::Loading { .. }) = cache.map.get_mut(&self.0) {
            let CacheEntry::Loading { attempts } = entry else {
                unreachable!()
            };
            *entry = CacheEntry::Error {
                attempts: attempts.saturating_add(1),
                at: Instant::now(),
            };
        }
    }
}

pub fn store_loaded(device_id: &str, path: &str, name: SharedString, image: Arc<Image>) {
    store_loaded_for(&key(device_id, path), name, image);
}

pub(crate) fn store_loaded_for(source: &AttachmentKey, name: SharedString, image: Arc<Image>) {
    cache()
        .lock()
        .unwrap()
        .insert_loaded(source.clone(), CachedAttachmentImage { name, image });
}

pub fn store_error(device_id: &str, path: &str) {
    store_error_for(&key(device_id, path));
}

pub(crate) fn store_error_for(source: &AttachmentKey) {
    let mut cache = cache().lock().unwrap();
    let attempts = match cache.map.get(source) {
        Some(CacheEntry::Loading { attempts }) => attempts + 1,
        Some(CacheEntry::Error { attempts, .. }) => *attempts,
        _ => 1,
    };
    cache.map.insert(
        source.clone(),
        CacheEntry::Error {
            attempts,
            at: Instant::now(),
        },
    );
}

/// Seed the cache after a successful upload (composer send path) so the just-
/// sent bubble's thumbnails render from local bytes instead of a round-trip.
pub fn seed_attachment(device_id: &str, path: &str, name: &str, image: Arc<Image>) {
    store_loaded(device_id, path, name.to_string().into(), image);
}

// ---------------------------------------------------------------------------
