//! Attachment loading, device candidate discovery, blob fetching, and image rendering.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use gpui::{div, img, prelude::*, px, AnyElement, Context, ObjectFit, SharedString, Task, Window};

use crate::motion;
use crate::theme::Theme;

use super::row::{generated_image_devices, RowKind, ToolDetail};
use super::tool_cards::blob_detail;
use super::Transcript;

/// User-bubble attachment thumbnails (user-attachments.tsx): 112×80 thumbs in
/// a wrapping strip. Fixed thumbnail sizes keep load-state flips from
/// shifting the virtualizer.
pub const ATT_THUMB_W: f32 = 112.0;
pub const ATT_THUMB_H: f32 = 80.0;

/// One sidecar blob fetch's lifecycle.
pub(crate) enum BlobFetch {
    Loading(#[allow(dead_code)] Task<()>),
    /// Failed with the affordance re-armed as a retry.
    Failed,
    Ready(Arc<ToolDetail>),
}

impl Transcript {
    pub(crate) fn spawn_blob_fetch(&mut self, blob_ref: SharedString, cx: &mut Context<Self>) {
        // Rank BEFORE the already-fetched guard: clicking a Ready ref is the
        // "show me this one again" toggle (recency bump + repaint, no
        // re-fetch) — with both a diff and an output fetched, the two
        // affordances must be able to trade places forever.
        self.blob_fetch_counter += 1;
        self.blob_fetch_order
            .insert(blob_ref.clone(), self.blob_fetch_counter);
        match self.blob_details.get(&blob_ref) {
            Some(BlobFetch::Ready(_)) => {
                cx.notify();
                return;
            }
            Some(BlobFetch::Loading(_)) => return,
            Some(BlobFetch::Failed) | None => {}
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let is_diff = blob_ref.ends_with(".diff");
        let ref_key = blob_ref.clone();
        let task = cx.spawn(async move |this, cx| {
            let reply = crate::attachments::call_with_timeout(
                &engine,
                cx.background_executor(),
                zeron_rpc::methods::FETCH_TOOL_BLOB,
                serde_json::json!({ "blobRef": ref_key.as_ref() }),
                Duration::from_secs(20),
            )
            .await;
            let fetched = match reply {
                Ok(value) => {
                    let text = value
                        .get("text")
                        .and_then(|t| t.as_str())
                        .unwrap_or_default();
                    blob_detail(text, is_diff)
                        .map(|d| BlobFetch::Ready(Arc::new(d)))
                        .unwrap_or(BlobFetch::Failed)
                }
                Err(_) => BlobFetch::Failed,
            };
            this.update(cx, |this, cx| {
                this.blob_details.insert(ref_key, fetched);
                cx.notify();
            })
            .ok();
        });
        self.blob_details.insert(blob_ref, BlobFetch::Loading(task));
    }

    // ---- attachment read-back (user-attachments.tsx + transcript cache) ----

    /// Shield the open transcript's attachments from image-cache eviction —
    /// rebuilt on every row sync so a chat switch swaps the set. Without it,
    /// budget pressure evicted thumbnails still on screen (the list caches
    /// rendered rows, so a visible image's LRU tick goes stale).
    pub(crate) fn refresh_protected_attachments(&self, cx: &Context<Self>) {
        // The protected set is GLOBAL and replaced wholesale — an override
        // instance writing it would clobber the primary transcript's keys.
        if self.doc_override.is_some() {
            return;
        }
        crate::attachments::protect_attachments(self.protected_attachment_keys(cx));
    }

    pub(crate) fn protected_attachment_keys(&self, cx: &Context<Self>) -> HashSet<(String, String)> {
        let devices = self.attachment_device_ids(cx);
        let mut keys = std::collections::HashSet::new();
        for row in &self.rows {
            // Generated images use bounded LRU retention, not history-wide protection.
            if let RowKind::User { attachments, .. } = &row.kind {
                for att in attachments.iter() {
                    for dev in &devices {
                        keys.insert((dev.clone(), att.path.clone()));
                    }
                }
            }
        }
        keys
    }

    /// Devices that may own a user message's attachment files: the chat's host
    /// device (uploads targeted it) plus this device (zeron's
    /// `uniqueIds([attachmentDeviceId, m.device_id])`).
    pub(crate) fn attachment_device_ids(&self, cx: &Context<Self>) -> Vec<String> {
        // `selected_chat_row` belongs to the PRIMARY transcript's chat — an
        // override instance has no chat row, so it claims no devices (its
        // thumbnails degrade to placeholders instead of guessing).
        if self.doc_override.is_some() {
            return Vec::new();
        }
        let state = self.state.read(cx);
        let mut ids = Vec::new();
        if let Some(chat) = state.selected_chat_row() {
            ids.push(chat.device_id.clone());
        }
        if let Some(local) = state.local_device_id.clone()
            && !ids.contains(&local)
        {
            ids.push(local);
        }
        ids
    }

    pub(crate) fn generated_attachment_device_ids(&self, owner: &str, cx: &Context<Self>) -> Vec<String> {
        let mut fallback = self.attachment_device_ids(cx);
        if let Some(local) = &self.state.read(cx).local_device_id {
            fallback.push(local.clone());
        }
        generated_image_devices(owner, &fallback)
    }

    /// Effective load state for one attachment across its candidate devices:
    /// first Loaded source wins; otherwise loads are (re)claimed and the
    /// snapshot degrades Loading → Error with a scheduled retry wake-up.
    pub(crate) fn attachment_state(
        &mut self,
        device_ids: &[String],
        path: &str,
        expected_raster_mime: Option<&str>,
        cx: &mut Context<Self>,
    ) -> crate::attachments::AttachmentSnapshot {
        use crate::attachments::{
            AttachmentKey, AttachmentSnapshot, attachment_snapshot_for, begin_load_for,
        };
        for dev in device_ids {
            if let AttachmentSnapshot::Loaded(image) =
                attachment_snapshot_for(&AttachmentKey::new(dev, path, expected_raster_mime))
            {
                return AttachmentSnapshot::Loaded(image);
            }
        }
        let mut any_loading = false;
        let mut min_retry: Option<Duration> = None;
        for dev in device_ids {
            if begin_load_for(&AttachmentKey::new(dev, path, expected_raster_mime)) {
                self.spawn_attachment_load(
                    dev.clone(),
                    path.to_string(),
                    expected_raster_mime.map(str::to_owned),
                    cx,
                );
            }
            match attachment_snapshot_for(&AttachmentKey::new(dev, path, expected_raster_mime)) {
                AttachmentSnapshot::Loaded(image) => return AttachmentSnapshot::Loaded(image),
                AttachmentSnapshot::Loading => {
                    // Generated assets try the owner first, falling back only
                    // after failure. Repainting never launches duplicate reads.
                    if expected_raster_mime.is_some() {
                        return AttachmentSnapshot::Loading;
                    }
                    any_loading = true;
                }
                AttachmentSnapshot::Error { retry_in } => {
                    min_retry = Some(min_retry.map_or(retry_in, |m| m.min(retry_in)));
                }
            }
        }
        if any_loading {
            return AttachmentSnapshot::Loading;
        }
        match min_retry {
            Some(retry_in) => {
                if let Some(dev) = device_ids.first() {
                    self.schedule_attachment_retry((dev.clone(), path.to_string()), retry_in, cx);
                }
                AttachmentSnapshot::Error { retry_in }
            }
            // No candidate devices at all — the "unavailable" thumb, no retry.
            None => AttachmentSnapshot::Error {
                retry_in: Duration::MAX,
            },
        }
    }

    pub(crate) fn spawn_attachment_load(
        &mut self,
        device_id: String,
        path: String,
        expected_raster_mime: Option<String>,
        cx: &mut Context<Self>,
    ) {
        use crate::attachments::{
            AttachmentKey, read_attachment_image, store_error_for, store_loaded_for,
        };
        let key = AttachmentKey::new(&device_id, &path, expected_raster_mime.as_deref());
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            store_error_for(&key);
            return;
        };
        let local = self.state.read(cx).local_device_id.clone();
        // Relay-forward only for a genuinely remote owner; the local device's
        // files are served directly.
        let target = (local.as_deref() != Some(device_id.as_str())).then(|| device_id.clone());
        let claim = crate::attachments::AttachmentLoadGuard(key.clone());
        let task_key = key.clone();
        let task = cx.spawn(async move |this, cx| {
            let _claim = claim;
            match read_attachment_image(
                &engine,
                cx.background_executor(),
                target.as_deref(),
                &path,
                expected_raster_mime.as_deref(),
            )
            .await
            {
                Some(loaded) => store_loaded_for(&task_key, loaded.name.into(), loaded.image),
                None => store_error_for(&task_key),
            }
            this.update(cx, |transcript, cx| {
                transcript.attachment_loads.remove(&task_key);
                cx.notify();
            })
            .ok();
        });
        self.attachment_loads.insert(key, task);
    }

    /// One wake-up per errored source: after the backoff elapses, a notify
    /// re-renders the thumb, whose `begin_load` then claims the retry.
    pub(crate) fn schedule_attachment_retry(
        &mut self,
        key: (String, String),
        delay: Duration,
        cx: &mut Context<Self>,
    ) {
        if delay == Duration::MAX || self.attachment_retries.contains_key(&key) {
            return;
        }
        let wake = key.clone();
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(delay + Duration::from_millis(60))
                .await;
            this.update(cx, |transcript, cx| {
                transcript.attachment_retries.remove(&wake);
                cx.notify();
            })
            .ok();
        });
        self.attachment_retries.insert(key, task);
    }

    pub(crate) fn render_generated_image(
        &mut self,
        row_id: &SharedString,
        owner: &str,
        path: &str,
        name: &str,
        mime_type: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use crate::attachments::AttachmentSnapshot;
        let devices = self.generated_attachment_device_ids(owner, cx);
        let state = self.attachment_state(&devices, path, Some(mime_type), cx);
        let theme = Theme::of(cx).clone();
        let frame = div()
            .id(SharedString::from(format!("{row_id}-generated")))
            .w(px(512.0))
            .max_w_full()
            .h(px(320.0))
            .max_h(px(420.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(12.0))
            .overflow_hidden()
            .bg(crate::theme::ink(0.045));
        match state {
            AttachmentSnapshot::Loaded(loaded) => {
                let dimensions =
                    crate::appshots::png_dimensions(&loaded.image.bytes).unwrap_or((512, 320));
                let scale = (512.0 / dimensions.0 as f32)
                    .min(420.0 / dimensions.1 as f32)
                    .min(1.0);
                let preview =
                    crate::attachments::PreviewImage::new(name.to_owned(), loaded.image.clone());
                frame
                    .w(px(dimensions.0 as f32 * scale))
                    .h(px(dimensions.1 as f32 * scale))
                    .role(gpui::Role::Button)
                    .aria_label("Preview generated image")
                    .tab_index(0)
                    .cursor_pointer()
                    .focus_visible(move |style| style.border_2().border_color(theme.accent))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.attachment_preview_return_focus = window.focused(cx);
                        preview.viewer.reset();
                        this.attachment_preview = Some(preview.clone());
                        window.focus(&this.attachment_preview_focus, cx);
                        cx.notify();
                    }))
                    .child(
                        gpui::img(loaded.image)
                            .size_full()
                            // The frame's overflow clip is rectangular; round the image itself.
                            .rounded(px(12.0))
                            .object_fit(gpui::ObjectFit::Contain),
                    )
                    .into_any_element()
            }
            AttachmentSnapshot::Loading => frame
                .text_color(theme.text_muted)
                .child("Loading generated image…")
                .into_any_element(),
            AttachmentSnapshot::Error { .. } => frame
                .text_color(theme.text_muted)
                .child("Generated image unavailable")
                .into_any_element(),
        }
    }

    /// The right-aligned thumbnail strip above a user bubble.
    pub(crate) fn render_user_attachments(
        &mut self,
        row_id: &SharedString,
        atts: &[crate::attachments::UserImageAttachment],
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use crate::attachments::AttachmentSnapshot;
        let glyph = Theme::of(cx).glyph;
        let device_ids = self.attachment_device_ids(cx);
        let mut strip = div()
            .w_full()
            .min_w_0()
            .flex_none()
            .flex()
            .flex_row()
            .flex_wrap()
            .justify_end()
            .items_start()
            .gap(px(8.0))
            .px(px(4.0))
            .pt(px(4.0))
            .pb(px(6.0));
        for (aix, att) in atts.iter().enumerate() {
            let state = self.attachment_state(&device_ids, &att.path, None, cx);
            // The in-flight send's progress belongs ON the thumbnail
            // (2026-08-18 user request). Two ref shapes mean "still
            // crossing": the queued flow's `pending://` (bytes ship
            // engine-side after the send; the host rewrites the ref to an
            // absolute path once they land and the run starts) and the
            // legacy echo's synthetic `pending/`. Percent sources, in order:
            // this attachment's own relay transfer (`WatchTransfers`, by the
            // uploadId its ref names — the leg that actually takes time),
            // else the send-wide staging/legacy upload percent. Neither → the
            // indeterminate spinner (staged-but-waiting, retry backoff, or
            // committed-awaiting-rewrite), so the ring never shows a number
            // that isn't a real transfer position (2026-08-20 report: the
            // staging-only percent blinked out in ~100ms and lied about the
            // slow part).
            let sending = att.path.starts_with("pending://") || att.path.starts_with("pending/");
            let upload_id = att
                .path
                .strip_prefix("pending://")
                .and_then(|rest| rest.split_once('/'))
                .map(|(id, _)| id);
            let uploading = upload_id
                .and_then(|id| self.state.read(cx).transfer_percent(id))
                .or_else(|| {
                    sending
                        .then(|| self.state.read(cx).upload_progress_percent())
                        .flatten()
                });
            if let Some(appshot) = &att.appshot {
                let has_image = matches!(&state, AttachmentSnapshot::Loaded(_));
                let theme = Theme::of(cx).clone();
                let accent = theme.accent;
                let width = 240.0;
                let mut card = div()
                    .id(SharedString::from(format!("{row_id}-appshot-{aix}")))
                    .w(px(width))
                    .max_w_full()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .items_center()
                    .rounded(px(14.0))
                    .p(px(8.0))
                    .gap(px(6.0))
                    .hover(|style| style.bg(crate::theme::ink(0.045)));
                let image_frame = div()
                    .w_full()
                    .h(px(128.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.0))
                    .overflow_hidden();
                card = match state {
                    AttachmentSnapshot::Loaded(image) => {
                        let preview =
                            crate::attachments::PreviewImage::new(image.name, image.image.clone());
                        card.role(gpui::Role::Button)
                            .aria_label(format!(
                                "Preview {} Appshot: {}",
                                appshot.app_name,
                                appshot.title()
                            ))
                            .tab_index(0)
                            .cursor_pointer()
                            .focus_visible(move |style| style.border_2().border_color(accent))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.attachment_preview_return_focus = window.focused(cx);
                                preview.viewer.reset();
                                this.attachment_preview = Some(preview.clone());
                                window.focus(&this.attachment_preview_focus, cx);
                                cx.notify();
                            }))
                            .child(
                                // Inherit the transcript's scroll fade. GPUI replaces
                                // rather than composes nested edge-fade scopes, so a
                                // decorative thumbnail fade would bypass the chrome fade.
                                image_frame.child(
                                    img(image.image)
                                        .w_full()
                                        .h(px(126.0))
                                        .rounded(px(5.0))
                                        .object_fit(ObjectFit::Contain),
                                ),
                            )
                    }
                    AttachmentSnapshot::Loading => card.child(
                        image_frame.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(theme.text_muted)
                                .child(if sending {
                                    "Uploading Appshot…"
                                } else {
                                    "Loading Appshot…"
                                }),
                        ),
                    ),
                    AttachmentSnapshot::Error { .. } => card.child(
                        image_frame.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(theme.text_muted)
                                .child(if sending {
                                    "Uploading Appshot…"
                                } else {
                                    "Appshot unavailable"
                                }),
                        ),
                    ),
                };
                card = card
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(5.0))
                            .max_w_full()
                            .child(
                                div()
                                    .size(px(24.0))
                                    .flex_none()
                                    .rounded(px(6.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(match crate::appshots::presentation_icon(appshot) {
                                        Some(icon) => img(icon)
                                            .size(px(24.0))
                                            .object_fit(ObjectFit::Contain)
                                            .into_any_element(),
                                        None => crate::icons::icon(crate::icons::MONITOR)
                                            .size(px(15.0))
                                            .text_color(theme.text_muted)
                                            .into_any_element(),
                                    }),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(11.0))
                                    .text_color(theme.text_muted)
                                    .child(SharedString::from(format!(
                                        "{} · Appshot",
                                        appshot.app_name
                                    ))),
                            ),
                    )
                    .child(
                        div()
                            .w_full()
                            .truncate()
                            .text_center()
                            .text_size(px(12.0))
                            .text_color(theme.text)
                            .child(SharedString::from(appshot.title().to_string())),
                    );
                if sending && (has_image || uploading.is_some()) {
                    card = card.child(
                        div()
                            .text_size(px(11.0))
                            .text_color(theme.text_muted)
                            .child(SharedString::from(
                                uploading
                                    .map(|pct| format!("Uploading {pct}%"))
                                    .unwrap_or_else(|| "Uploading…".into()),
                            )),
                    );
                }
                strip = strip.child(card);
                continue;
            }
            let frame = div()
                .flex_none()
                .w(px(ATT_THUMB_W))
                .h(px(ATT_THUMB_H))
                .rounded(px(8.0))
                .overflow_hidden();
            let thumb: AnyElement = match state {
                AttachmentSnapshot::Loaded(image) => {
                    let preview = crate::attachments::PreviewImage::new(
                        image.name.clone(),
                        image.image.clone(),
                    );
                    frame
                        .id(SharedString::from(format!("{row_id}#att{aix}")))
                        .relative()
                        .border_1()
                        .border_color(crate::theme::hairline(0.11))
                        .bg(crate::theme::ink(0.035))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.attachment_preview_return_focus = window.focused(cx);
                            preview.viewer.reset();
                            this.attachment_preview = Some(preview.clone());
                            window.focus(&this.attachment_preview_focus, cx);
                            cx.notify();
                        }))
                        .child(
                            img(image.image.clone())
                                // EXPLICIT dims, not size_full: img layout
                                // honors the intrinsic aspect ratio over a
                                // percent height (gpui f8d8a90 repoint), so
                                // size_full let a tall photo grow past the
                                // frame and the rectangular overflow clip
                                // squared the bottom corners (2026-08-19).
                                .w(px(ATT_THUMB_W - 2.0))
                                .h(px(ATT_THUMB_H - 2.0))
                                // The IMG needs its own radii: the frame's
                                // rounding only clips rectangularly, so the
                                // sprite must round its own corners (7 = the
                                // frame's 8 minus its 1px border).
                                .rounded(px(7.0))
                                .object_fit(ObjectFit::Cover),
                        )
                        .when(sending, |el| {
                            // The pulse read registers this entity for frames,
                            // so the overlay stays live even once the trailer's
                            // 30s pending-send bridge has lapsed.
                            let pulse = motion::pulse_wave(motion::pulse_delta(
                                &motion::ZERON_PULSE,
                                cx.entity_id(),
                                cx,
                            ));
                            let indicator: AnyElement = match uploading {
                                Some(pct) => crate::loaders::upload_progress_ring(pct, 34.0),
                                None => crate::loaders::mini_glyph_spinner(
                                    format!("att-sending-{row_id}-{aix}"),
                                    3.0,
                                    glyph,
                                    cx.entity_id(),
                                    cx,
                                )
                                .into_any_element(),
                            };
                            el.child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .rounded(px(7.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(gpui::hsla(0.0, 0.0, 0.0, 0.38 + 0.05 * pulse))
                                    .child(indicator),
                            )
                        })
                        .into_any_element()
                }
                // Errored/unavailable: the dashed "missing" thumb.
                AttachmentSnapshot::Error { .. } => frame
                    .border_1()
                    .border_dashed()
                    .border_color(crate::theme::hairline(0.14))
                    .bg(crate::theme::ink(0.025))
                    .into_any_element(),
                // Loading: the pulsing skeleton (same wash as popover skeletons).
                AttachmentSnapshot::Loading => frame
                    .border_1()
                    .border_color(crate::theme::hairline(0.08))
                    .bg(crate::theme::ink(0.055))
                    .opacity(
                        0.35 + 0.4
                            * motion::pulse_wave(motion::pulse_delta(
                                &motion::ZERON_PULSE,
                                cx.entity_id(),
                                cx,
                            )),
                    )
                    .into_any_element(),
            };
            strip = strip.child(thumb);
        }
        strip.into_any_element()
    }

}
