//! Fold states, user bubble expansion, and smooth collapse scroll compensation.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    div, prelude::*, px, AnyElement, Context, SharedString, Task,
};

use crate::motion;
use crate::theme::Theme;

use super::row::{user_resize_duration_ms, user_resize_spec};
use super::stick_spring::jump_visibility;
use super::Transcript;

/// A user prompt renders at most this many wrapped lines until expanded. A
/// pasted log or file drops into the transcript as one endless slab otherwise
/// (user report) — past the cap the bubble clips and grows a chevron.
pub const USER_COLLAPSED_LINES: usize = 5;
/// The user bubble's line box.
pub const USER_LINE_HEIGHT: f32 = 22.0;
/// Conservative first-frame soft-wrap proxy for the fixed-width long-prompt
/// bubble. The final decision uses the wrapped `StyledText` layout, but this
/// fallback lets clearly long prompts render their affordance immediately
/// before that first layout has completed.
pub const USER_COLLAPSE_CHARS: usize = 400;
/// Vertical separation before the plain expand/collapse link.
pub const USER_TOGGLE_GAP: f32 = 8.0;

#[derive(Default, Clone, Copy)]
pub struct FoldState {
    /// User pin (click); `None` follows the auto-open rule.
    pub open: Option<bool>,
    /// Bumped per toggle — keys the 200ms height tween.
    pub epoch: usize,
    /// Height at the moment of the toggle (the tween's start). The destination
    /// is always the *current* target height, so content growth after a toggle
    /// snaps instead of replaying a stale tween.
    pub from: f32,
    /// When the toggle happened. The tween is armed only for a short window
    /// after the click: gpui replays an element's animation on REMOUNT, and a
    /// virtualized row scrolling back into view is a remount — an armed-forever
    /// tween made every once-collapsed group flash open→closed on each
    /// reappearance (user report).
    pub toggled_at: Option<Instant>,
    pub disclosure_at: Option<Instant>,
    /// Per-toggle duration. User bubbles scale this with travel distance;
    /// existing tool folds leave it at zero and keep their catalog constants.
    pub duration_ms: u64,
    /// Extra user-body height revealed by Show more. It is not reply growth
    /// and must not permanently consume the sent turn's reservation.
    pub user_expansion_height: f32,
}

/// Viewport compensation paired with a long user-message collapse. While the
/// row loses height, this scrolls upward by the same eased distance so a
/// bottom-pinned viewport keeps the collapsing bubble in view.
#[derive(Clone, Copy, Debug)]
pub struct UserCollapseScroll {
    pub started_at: Instant,
    pub duration_ms: u64,
    pub height_delta: f32,
    pub row_ix: usize,
    pub initial_top: f32,
    pub target_top: f32,
}

impl Transcript {
    /// Expand/collapse one long user bubble. Heights come from the text's
    /// passive paint cache, never from transcript state, so this changes
    /// render-local fold state only and does not rebuild or splice rows.
    pub(crate) fn toggle_user_fold(
        &mut self,
        row_id: SharedString,
        row_ix: usize,
        collapsed_h: f32,
        full_h: f32,
        reduced_motion: bool,
    ) {
        let duration_ms = user_resize_duration_ms(full_h - collapsed_h);
        // A fold owns the viewport just like explicit navigation. Release
        // the sent-turn hold as well as the spring: otherwise growing beyond
        // the reserved space hands the still-held turn back to the bottom
        // pin and hides the beginning of the newly expanded prompt.
        self.begin_scroll_navigation();
        let entry = self.user_folds.entry(row_id).or_default();
        let currently_open = entry.open.unwrap_or(false);
        entry.from = if currently_open { full_h } else { collapsed_h };
        entry.open = Some(!currently_open);
        entry.epoch += 1;
        entry.toggled_at = Some(Instant::now());
        entry.duration_ms = duration_ms;
        entry.user_expansion_height = (full_h - collapsed_h).max(0.0);

        // Capture a screen-space anchor for the clicked row. We do not subtract
        // the removed height: that is only correct when the row's top is
        // exactly at the viewport bottom. At the top or middle of a long
        // message it overscrolls past the collapsed bubble. The same anchor is
        // used for expansion, with the full target height, so a bottom-pinned
        // prompt reveals its beginning below the top fade instead of above it.
        let Some(item_bounds) = self.list.bounds_for_item(row_ix) else {
            return;
        };
        let viewport = self.list.viewport_bounds();
        let initial_top = f32::from(item_bounds.top());
        // Keep a newly revealed bubble below the transcript's top fade band. A
        // 12px inset alone still leaves the first lines washed into the edge
        // fade when the expanded row started above view.
        let viewport_top = f32::from(viewport.top()) + Theme::TRANSCRIPT_FADE_BAND + 28.0;
        let target_height = if currently_open { collapsed_h } else { full_h };
        let viewport_bottom = f32::from(viewport.bottom()) - target_height - 12.0;
        let target_top = if viewport_bottom >= viewport_top {
            initial_top.clamp(viewport_top, viewport_bottom)
        } else {
            viewport_top
        };
        let needs_scroll = (target_top - initial_top).abs() > 0.5;

        if needs_scroll {
            if reduced_motion {
                if let Some(current) = self.list.bounds_for_item(row_ix) {
                    self.list
                        .scroll_by(px(f32::from(current.top()) - target_top));
                }
            } else {
                self.user_collapse_scroll = Some(UserCollapseScroll {
                    started_at: Instant::now(),
                    duration_ms,
                    height_delta: (full_h - collapsed_h).max(0.0),
                    row_ix,
                    initial_top,
                    target_top,
                });
            }
        }
    }

    pub(crate) fn cancel_user_hold(&mut self) {
        self.user_hold_token = self.user_hold_token.wrapping_add(1);
        self.user_hold_task = None;
    }

    /// Arm a long-press toggle instead of using double-click. Releasing before
    /// the threshold preserves an ordinary click/selection gesture; moving
    /// cancels the timer so drag selection never unexpectedly toggles the
    /// message.
    pub(crate) fn arm_user_hold(
        &mut self,
        row_id: SharedString,
        row_ix: usize,
        collapsed_h: f32,
        measured_h: Rc<Cell<f32>>,
        selection_key: Arc<str>,
        cx: &mut Context<Self>,
    ) {
        const USER_HOLD_DELAY: Duration = Duration::from_millis(360);
        self.cancel_user_hold();
        self.user_hold_token = self.user_hold_token.wrapping_add(1);
        let token = self.user_hold_token;
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(USER_HOLD_DELAY).await;
            this.update(cx, |this, cx| {
                if this.user_hold_token != token {
                    return;
                }
                this.user_hold_task = None;
                crate::markdown::selection::clear_if_owner(&selection_key);
                this.toggle_user_fold(
                    row_id,
                    row_ix,
                    collapsed_h,
                    measured_h.get().max(collapsed_h),
                    motion::reduced_motion(cx),
                );
                cx.notify();
            })
            .ok();
        });
        self.user_hold_task = Some(task);
    }

    pub(crate) fn step_user_collapse_scroll(&mut self, cx: &mut Context<Self>) {
        let Some(scroll) = self.user_collapse_scroll.as_ref() else {
            return;
        };
        let started_at = scroll.started_at;
        let duration_ms = scroll.duration_ms;
        let height_delta = scroll.height_delta;
        let row_ix = scroll.row_ix;
        let initial_top = scroll.initial_top;
        let target_top = scroll.target_top;
        let raw =
            (started_at.elapsed().as_secs_f32() / (duration_ms as f32 / 1000.0)).clamp(0.0, 1.0);
        let spec = user_resize_spec(height_delta);
        let progress = spec.progress(raw);
        let desired_top = motion::lerp(initial_top, target_top, progress);
        if let Some(current) = self.list.bounds_for_item(row_ix) {
            // `scroll_by(+x)` moves content up, so correcting current minus
            // desired keeps the row on the interpolated screen-space path.
            let correction = f32::from(current.top()) - desired_top;
            if correction.abs() > 0.1 {
                self.list.scroll_by(px(correction));
            }
        }
        if raw >= 1.0 {
            self.user_collapse_scroll = None;
            self.last_scroll_distance = self.distance_from_bottom();
            self.show_jump_button =
                jump_visibility(self.show_jump_button, self.last_scroll_distance);
        }
        cx.notify();
    }

    pub(crate) fn toggle_fold(&mut self, row_id: SharedString, open_height: f32, auto_open: bool) {
        let entry = self.folds.entry(row_id).or_default();
        let currently_open = entry.open.unwrap_or(auto_open);
        entry.from = if currently_open { open_height } else { 0.0 };
        entry.open = Some(!currently_open);
        entry.epoch += 1;
        entry.toggled_at = Some(Instant::now());
        entry.disclosure_at = entry.toggled_at;
    }

    /// A plain text link aligned with the message's left edge, following the
    /// continuation ellipsis when collapsed. No pill, border, or button wash.
    pub(crate) fn render_user_expander(
        &mut self,
        row_id: &SharedString,
        row_ix: usize,
        expanded: bool,
        collapsed_h: f32,
        measured_h: Rc<Cell<f32>>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let toggle_key = row_id.clone();
        let glyph = if expanded {
            crate::icons::ALT_ARROW_UP
        } else {
            crate::icons::ALT_ARROW_DOWN
        };
        let label = if expanded { "Show less" } else { "Show more" };
        let button = div()
            .id(SharedString::from(format!("{row_id}-expander")))
            .group("user-message-toggle")
            .role(gpui::Role::Button)
            .aria_label(if expanded {
                "Collapse message"
            } else {
                "Expand message"
            })
            .aria_expanded(expanded)
            .flex()
            .items_center()
            .gap(px(5.0))
            .text_size(crate::typography::ui_rems(14.0))
            .line_height(crate::typography::ui_rems(USER_LINE_HEIGHT))
            .text_color(theme.text_muted)
            .cursor_pointer()
            .hover(|s| s.text_color(theme.text))
            .child(label)
            .child(
                crate::icons::icon(glyph)
                    .size(px(12.0))
                    .text_color(theme.text_muted)
                    .group_hover("user-message-toggle", |s| s.text_color(theme.text)),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_user_fold(
                    toggle_key.clone(),
                    row_ix,
                    collapsed_h,
                    measured_h.get().max(collapsed_h),
                    motion::reduced_motion(cx),
                );
                cx.notify();
            }));
        div()
            .mt(px(USER_TOGGLE_GAP))
            .flex()
            .items_start()
            .child(button)
            .into_any_element()
    }
}
