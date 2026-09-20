//! Staged attachments, appshot strips, and thumbnail preview rendering.

use std::path::PathBuf;
use std::time::Instant;

use gpui::{
    div, img, px, Context, IntoElement, KeyDownEvent, ObjectFit, Render, SharedString, Window,
};

use crate::appshots::{self, CapturedAppshot};
use crate::attachments::{self, StagedAttachment};
use crate::motion;
use crate::theme::Theme;

use super::*;

// ---------------------------------------------------------------------------
// Constants + calculations for attachment & appshot strips
// ---------------------------------------------------------------------------

pub const STRIP_THUMB: f32 = 56.0;
pub const STRIP_GAP: f32 = 8.0;
pub const STRIP_PAD_TOP: f32 = 12.0;
pub const STRIP_PAD_X: f32 = 16.0;

pub fn attachment_strip_height(count: usize, inner_width: f32) -> f32 {
    if count == 0 {
        return 0.0;
    }
    let usable = (inner_width - 2.0 * STRIP_PAD_X).max(STRIP_THUMB);
    let per_row = (((usable + STRIP_GAP) / (STRIP_THUMB + STRIP_GAP)).floor() as usize).max(1);
    let rows = count.div_ceil(per_row);
    STRIP_PAD_TOP + rows as f32 * STRIP_THUMB + (rows - 1) as f32 * STRIP_GAP
}

pub fn comment_strip_height(count: usize) -> f32 {
    if count == 0 {
        return 0.0;
    }
    STRIP_PAD_TOP + crate::badges::BADGE_HEIGHT
}

pub const APPSHOT_TILE_MIN_WIDTH: f32 = 96.0;
pub const APPSHOT_IMAGE_INSET: f32 = 12.0;
pub const APPSHOT_PREVIEW_HEIGHT: f32 = 148.0;
pub const APPSHOT_IMAGE_MAX_WIDTH: f32 = 320.0;
pub const APPSHOT_IMAGE_MAX_HEIGHT: f32 = 132.0;
pub const APPSHOT_TILE_HEIGHT: f32 = 192.0;

pub(crate) struct AppshotActionTooltip(pub SharedString);

impl Render for AppshotActionTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        div()
            .px(px(8.0))
            .py(px(6.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(theme.border_strong)
            .bg(theme.surface_raised)
            .shadow_md()
            .text_size(px(11.0))
            .text_color(theme.text)
            .child(self.0.clone())
    }
}

pub fn appshot_contained_size(dimensions: Option<(u32, u32)>, max_width: f32) -> (f32, f32) {
    let max_width = if max_width.is_finite() {
        max_width.clamp(1.0, APPSHOT_IMAGE_MAX_WIDTH)
    } else {
        APPSHOT_IMAGE_MAX_WIDTH
    };
    let (width, height) = dimensions
        .filter(|(width, height)| *width > 0 && *height > 0)
        .unwrap_or((16, 10));
    let scale = (max_width / width as f32).min(APPSHOT_IMAGE_MAX_HEIGHT / height as f32);
    (width as f32 * scale, height as f32 * scale)
}

pub fn appshot_strip_height(count: usize) -> f32 {
    if count == 0 {
        0.0
    } else {
        STRIP_PAD_TOP + APPSHOT_TILE_HEIGHT
    }
}

// ---------------------------------------------------------------------------
// Composer attachments & appshots implementation
// ---------------------------------------------------------------------------

impl Composer {
    pub(crate) fn staged_appshots(&self) -> &[CapturedAppshot] {
        self.appshots
            .get(&self.current_key)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn stage_appshot(&mut self, appshot: CapturedAppshot, cx: &mut Context<Self>) {
        self.stage_appshot_for(self.current_key.clone(), appshot, cx);
    }

    pub fn stage_appshot_for(
        &mut self,
        key: String,
        appshot: CapturedAppshot,
        cx: &mut Context<Self>,
    ) -> bool {
        let staged_bytes = self
            .appshots
            .get(&key)
            .into_iter()
            .flatten()
            .map(|shot| shot.screenshot.bytes().len() as u64)
            .sum::<u64>();
        let saved_bytes = self
            .queue_edit_draft
            .as_ref()
            .filter(|_| key == self.current_key)
            .map(|(_, _, shots)| {
                shots
                    .iter()
                    .map(|shot| shot.screenshot.bytes().len() as u64)
                    .sum::<u64>()
            })
            .unwrap_or_default();
        let staged_bytes = staged_bytes.saturating_add(saved_bytes);
        let incoming = appshot.screenshot.bytes().len() as u64;
        if incoming > attachments::MAX_ATTACHMENT_BYTES
            || staged_bytes.saturating_add(incoming) > appshots::MAX_STAGED_APPSHOT_BYTES
        {
            self.failure = Some(
                "Remove an Appshot before adding another (96 MB staged Appshot limit).".into(),
            );
            self.failure_key = Some(key);
            cx.notify();
            return false;
        }
        self.appshot_entrances
            .retain(|_, start| start.elapsed().as_secs_f32() < motion::speed_scale());
        if !motion::reduced_motion(cx) {
            self.appshot_entrances
                .insert(appshot.id.clone(), Instant::now());
        }
        // A capture completing while a queue edit is being saved belongs to
        // the displaced draft; it must not be lost or change the in-flight edit.
        if self.queue_edit_finishing && key == self.current_key {
            if let Some((_, _, saved_appshots)) = &mut self.queue_edit_draft {
                saved_appshots.push(appshot);
            } else {
                self.appshots.entry(key).or_default().push(appshot);
            }
        } else {
            self.appshots.entry(key).or_default().push(appshot);
        }
        self.failure = None;
        self.failure_key = None;
        cx.notify();
        true
    }

    pub fn show_appshot_error(&mut self, message: String, cx: &mut Context<Self>) {
        self.failure = Some(message.into());
        self.failure_key = Some(self.current_key.clone());
        cx.notify();
    }

    pub(crate) fn add_staged(&mut self, staged: Vec<StagedAttachment>, cx: &mut Context<Self>) {
        if self.queue_edit_finishing {
            return;
        }
        if staged.is_empty() {
            return;
        }
        self.attachments
            .entry(self.current_key.clone())
            .or_default()
            .extend(staged);
        self.focus_pending = true;
        cx.notify();
    }

    /// Stage image files (picker / drop / pasted paths). Non-images are
    /// skipped silently (matching the original's `image/*` filter); read
    /// failures and oversize files surface in the failure notice.
    pub(crate) fn add_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let mut staged = Vec::new();
        for path in &paths {
            if attachments::format_by_extension(path).is_none() {
                continue;
            }
            match attachments::stage_file(path) {
                Ok(att) => staged.push(att),
                Err(message) => {
                    self.failure = Some(message.into());
                    self.failure_key = Some(self.current_key.clone());
                    cx.notify();
                }
            }
        }
        self.add_staged(staged, cx);
    }

    /// Add a file-tree or file-tab drop through the existing file-mention
    /// pipeline. This keeps the reference workspace-relative and therefore
    /// valid for local and remote sessions alike.
    pub(crate) fn add_workspace_path(
        &mut self,
        path: &str,
        is_directory: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let inserted = self.input.update(cx, |input, cx| {
            input.insert_dropped_mention(path, is_directory, cx)
        });
        if inserted {
            self.reset_mention(None, cx);
            self.reset_slash(None, cx);
            let focus = self.input.read(cx).focus_handle.clone();
            window.focus(&focus, cx);
            cx.notify();
        }
    }

    pub(crate) fn remove_attachment(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.queue_edit_finishing {
            return;
        }
        if let Some(list) = self.attachments.get_mut(&self.current_key) {
            list.retain(|a| a.id != id);
            if list.is_empty() {
                self.attachments.remove(&self.current_key);
            }
        }
        cx.notify();
    }

    pub(crate) fn restore_failed_appshots(
        &mut self,
        sent: &[CapturedAppshot],
        failed_key: &str,
        restore_key: &str,
    ) {
        if sent.is_empty() {
            return;
        }
        let mut merged = sent.to_vec();
        for key in [failed_key, restore_key] {
            for shot in self.appshots.remove(key).unwrap_or_default() {
                if !merged.iter().any(|existing| existing.id == shot.id) {
                    merged.push(shot);
                }
            }
        }
        self.appshots.insert(restore_key.to_string(), merged);
    }

    pub(crate) fn remove_appshot(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.queue_edit_finishing {
            return;
        }
        if let Some(list) = self.appshots.get_mut(&self.current_key) {
            list.retain(|appshot| appshot.id != id);
            if list.is_empty() {
                self.appshots.remove(&self.current_key);
            }
        }
        cx.notify();
    }

    pub(crate) fn render_comments_chip(&self, theme: &Theme, cx: &App) -> Option<gpui::Div> {
        let count = self.staged_comments(cx).len();
        if count == 0 {
            return None;
        }
        Some(
            div()
                .flex()
                .flex_row()
                .px(px(STRIP_PAD_X))
                .pt(px(STRIP_PAD_TOP))
                .child(crate::badges::render(
                    "composer-comments",
                    &crate::badges::MessageBadge {
                        icon: crate::icons::CHAT_ROUND_LINE,
                        label: crate::comments::chip_label(count).into(),
                        // The staged set is already on screen in the changes
                        // pane, so a hover card would only repeat it.
                        details: Vec::new(),
                    },
                    theme,
                )),
        )
    }

    /// The staged-thumbnail strip (attachment-ui.tsx AttachmentStrip):
    /// `flex flex-wrap gap-2 px-4 pt-3`, 56px rounded thumbs, a remove button
    /// revealed on hover, click opens the full-size preview.
    pub(crate) fn render_attachment_strip(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let staged = self.staged();
        if staged.is_empty() {
            return None;
        }
        let mut strip = div()
            .w_full()
            .flex_none()
            .flex()
            .flex_row()
            .flex_wrap()
            .gap(px(STRIP_GAP))
            .px(px(STRIP_PAD_X))
            .pt(px(STRIP_PAD_TOP));
        for (ix, att) in staged.iter().enumerate() {
            let group: SharedString = format!("composer-att-{}", att.id).into();
            let preview = attachments::PreviewImage::new(att.name.clone(), att.image.clone());
            let remove_id = att.id.clone();
            strip = strip.child(
                div()
                    .group(group.clone())
                    .flex_none()
                    .relative()
                    .child(
                        div()
                            .id(("composer-att-thumb", ix))
                            .size(px(STRIP_THUMB))
                            .rounded(px(8.0))
                            .overflow_hidden()
                            .border_1()
                            .border_color(crate::theme::hairline(0.10))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                preview.viewer.reset();
                                this.preview = Some(preview.clone());
                                this.preview_focus_pending = true;
                                cx.notify();
                            }))
                            .child(
                                img(att.image.clone())
                                    // EXPLICIT dims, not size_full: img layout
                                    // honors the image's intrinsic aspect
                                    // ratio over a percent height (gpui
                                    // f8d8a90 repoint), so size_full let a
                                    // tall photo grow past the frame — the
                                    // rectangular overflow clip then squared
                                    // the bottom corners (2026-08-19 report).
                                    // 56−2 = frame minus its 1px borders.
                                    .w(px(STRIP_THUMB - 2.0))
                                    .h(px(STRIP_THUMB - 2.0))
                                    // Own radii — the frame's rounding only
                                    // clips rectangularly (7 = 8 - border).
                                    .rounded(px(7.0))
                                    .object_fit(ObjectFit::Cover),
                            ),
                    )
                    // Own layer: inside the frosted pill everything shares one
                    // draw order and images render last, so without it the
                    // thumbnail paints OVER this button (user report).
                    .child(crate::frost::layered(
                        div()
                            .id(("composer-att-remove", ix))
                            .absolute()
                            .top(px(-6.0))
                            .right(px(-6.0))
                            .size(px(18.0))
                            .rounded_full()
                            .bg(theme.bg)
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .shadow_sm()
                            .opacity(0.0)
                            .group_hover(group, |s| s.opacity(1.0))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                // The button overhangs the thumbnail, whose
                                // hitbox is right underneath — don't let the
                                // same click also open the preview.
                                cx.stop_propagation();
                                this.remove_attachment(&remove_id, cx);
                            }))
                            .child(
                                crate::icons::icon(crate::icons::CLOSE_CIRCLE)
                                    .size(px(14.0))
                                    .text_color(theme.text_muted),
                            ),
                    )),
            );
        }
        Some(strip)
    }

    pub(crate) fn render_appshot_strip(
        &self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let appshots = self.staged_appshots();
        if appshots.is_empty() {
            return None;
        }
        let mut strip = div()
            .id("composer-appshots-strip")
            .flex()
            .flex_row()
            .gap(px(STRIP_GAP))
            .px(px(STRIP_PAD_X))
            .pt(px(STRIP_PAD_TOP))
            .overflow_x_scroll();
        let max_image_width = self.last_available_width.unwrap_or(COMPOSER_MAX_WIDTH)
            - 2.0 * Theme::SPACE_LG
            - 2.0
            - 2.0 * STRIP_PAD_X
            - 2.0 * APPSHOT_IMAGE_INSET;
        for (ix, appshot) in appshots.iter().enumerate() {
            let group: SharedString = format!("composer-appshot-{}", appshot.id).into();
            let preview = crate::attachments::PreviewImage::new(
                appshot.screenshot.name.clone(),
                appshot.screenshot.image.clone(),
            );
            let preview_on_key = preview.clone();
            let preview_on_a11y = preview.clone();
            let composer_for_preview = cx.entity().downgrade();
            let remove_id = appshot.id.clone();
            let remove_on_key_id = remove_id.clone();
            let remove_on_a11y_id = remove_id.clone();
            let composer_for_remove = cx.entity().downgrade();
            let source: SharedString = appshot
                .window_title
                .as_deref()
                .filter(|title| !title.trim().is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| appshot.app_name.clone())
                .into();
            let preview_label: SharedString = format!("Preview {source}").into();
            let remove_label: SharedString = format!("Remove {source}").into();
            let preview_aria = preview_label.clone();
            let remove_aria = remove_label.clone();
            let (image_width, image_height) =
                appshot_contained_size(appshot.screenshot_dimensions, max_image_width);
            let tile_width = (image_width + 2.0 * APPSHOT_IMAGE_INSET).max(APPSHOT_TILE_MIN_WIDTH);
            let mut card = div()
                .id(("composer-appshot", ix))
                .group(group.clone())
                .relative()
                .w(px(tile_width))
                .h(px(APPSHOT_TILE_HEIGHT))
                .flex_none()
                .flex()
                .flex_col()
                .items_center()
                .rounded(px(14.0))
                .overflow_hidden()
                .cursor_pointer()
                .hover(|style| style.bg(crate::theme::ink(0.045)))
                .tooltip(move |_, cx| {
                    cx.new(|_| AppshotActionTooltip(preview_label.clone()))
                        .into()
                })
                .role(gpui::Role::Button)
                .aria_label(preview_aria)
                .tab_index(0)
                .focus_visible(|style| {
                    style
                        .bg(crate::theme::ink(0.06))
                        .border_1()
                        .border_color(theme.accent)
                })
                .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        cx.stop_propagation();
                        preview_on_key.viewer.reset();
                        this.preview = Some(preview_on_key.clone());
                        this.preview_focus_pending = true;
                        cx.notify();
                    }
                }))
                .on_a11y_action(gpui::AccessibleAction::Click, move |_, _, cx| {
                    composer_for_preview
                        .update(cx, |this, cx| {
                            preview_on_a11y.viewer.reset();
                            this.preview = Some(preview_on_a11y.clone());
                            this.preview_focus_pending = true;
                            cx.notify();
                        })
                        .ok();
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    preview.viewer.reset();
                    this.preview = Some(preview.clone());
                    this.preview_focus_pending = true;
                    cx.notify();
                }))
                .child(
                    div()
                        .id(("composer-appshot-preview", ix))
                        .w(px(tile_width))
                        .h(px(APPSHOT_PREVIEW_HEIGHT))
                        .relative()
                        .flex_none()
                        .flex()
                        .items_end()
                        .justify_center()
                        .overflow_hidden()
                        .rounded(px(12.0))
                        .child(
                            div()
                                .w(px(image_width))
                                .h(px(image_height))
                                .flex_none()
                                .overflow_hidden()
                                .rounded(px(4.0))
                                .shadow_sm()
                                .child(crate::edge_fade::edge_faded(
                                    44.0,
                                    false,
                                    true,
                                    img(appshot.screenshot.image.clone())
                                        .w(px(image_width))
                                        .h(px(image_height))
                                        .object_fit(ObjectFit::Contain),
                                )),
                        ),
                )
                .child(
                    div()
                        .mt(px(20.0))
                        .max_w(px(tile_width - 20.0))
                        .truncate()
                        .text_center()
                        .text_size(px(12.5))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(theme.text)
                        .child(source),
                );
            if let Some(icon) = &appshot.app_icon {
                card = card.child(crate::frost::layered(
                    div()
                        .absolute()
                        .top(px(APPSHOT_PREVIEW_HEIGHT - 22.0))
                        .left(px((tile_width - 28.0) / 2.0))
                        .size(px(28.0))
                        .rounded(px(7.0))
                        .bg(theme.bg)
                        .border_1()
                        .border_color(theme.border)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            img(icon.clone())
                                .size(px(24.0))
                                .rounded(px(5.0))
                                .object_fit(ObjectFit::Contain),
                        ),
                ));
            }
            card = card.child(crate::frost::layered(
                div()
                    .id(("composer-appshot-remove", ix))
                    .absolute()
                    .top(px(6.0))
                    .right(px(6.0))
                    .size(px(22.0))
                    .rounded_full()
                    .bg(theme.bg.opacity(0.92))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .shadow_sm()
                    .opacity(0.0)
                    .group_hover(group, |style| style.opacity(1.0))
                    .tooltip(move |_, cx| {
                        cx.new(|_| AppshotActionTooltip(remove_label.clone()))
                            .into()
                    })
                    .role(gpui::Role::Button)
                    .aria_label(remove_aria)
                    .tab_index(0)
                    .focus_visible(|style| style.opacity(1.0).border_1().border_color(theme.accent))
                    .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                            cx.stop_propagation();
                            this.remove_appshot(&remove_on_key_id, cx);
                        }
                    }))
                    .on_a11y_action(gpui::AccessibleAction::Click, move |_, _, cx| {
                        composer_for_remove
                            .update(cx, |this, cx| {
                                this.remove_appshot(&remove_on_a11y_id, cx);
                            })
                            .ok();
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.remove_appshot(&remove_id, cx);
                    }))
                    .child(
                        crate::icons::icon(crate::icons::CLOSE_CIRCLE)
                            .size(px(15.0))
                            .text_color(theme.text_muted),
                    ),
            ));
            // Entity-owned timestamps prevent the entrance replaying on route remount.
            if let Some(start) = self.appshot_entrances.get(&appshot.id) {
                let raw = (start.elapsed().as_secs_f32() / (0.24 * motion::speed_scale()))
                    .clamp(0.0, 1.0);
                if raw < 1.0 && !motion::reduced_motion(cx) {
                    let progress =
                        motion::MotionSpec::new(240, motion::EASE_OUT_EXPO).progress(raw);
                    card = card.opacity(progress).top(px(8.0 * (1.0 - progress)));
                    window.request_animation_frame();
                }
            }
            strip = strip.child(card);
        }
        Some(strip.into_any_element())
    }

    #[cfg(feature = "appshots-fixture")]
    pub fn fixture_clear_appshots(&mut self, cx: &mut Context<Self>) {
        self.appshots.clear();
        self.appshot_entrances.clear();
        cx.notify();
    }
}
