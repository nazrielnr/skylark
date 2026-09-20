//! Popover / menu primitives: an anchored floating layer with the `menu-in`
//! animation, outside-click dismissal, and pure keyboard-navigation + search
//! reducers shared by every picker and menu (feature-inventory §1.12 popovers).
//!
//! gpui pattern (examples/popover.rs at the pinned rev): the trigger element
//! conditionally children a `deferred(anchored().child(content))` — deferred
//! paints on a floating layer above everything, anchored positions it relative
//! to the trigger (or an explicit point for context menus).
//!
//! Pure logic (wrap-around list navigation, ranked substring filtering, key
//! classification) lives in free functions with unit tests; the elements only
//! feed them measurements/events.

use gpui::{
    Anchor, AnyElement, Context, Div, ElementId, IntoElement, MouseButton, MouseDownEvent,
    MouseUpEvent, Pixels, Point, ScrollHandle, SharedString, Stateful, Window, div, prelude::*, px,
};
use std::time::{Duration, Instant};

use crate::motion::{self, ZERON_PULSE};
use crate::theme::{Theme, hairline, ink};

// ---------------------------------------------------------------------------
// Loadable — async slot state shared by pickers/settings pages
// ---------------------------------------------------------------------------

/// One async-loaded slot: `Idle` (never requested) → `Loading` (skeletons) →
/// `Ready` / `Error` (inline message + Retry).
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Loadable<T> {
    #[default]
    Idle,
    Loading,
    Ready(T),
    Error(String),
}

impl<T> Loadable<T> {
    pub fn ready(&self) -> Option<&T> {
        match self {
            Loadable::Ready(value) => Some(value),
            _ => None,
        }
    }

    pub fn is_loading(&self) -> bool {
        matches!(self, Loadable::Loading)
    }

    pub fn error(&self) -> Option<&str> {
        match self {
            Loadable::Error(message) => Some(message),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Popup — open/closing/closed lifecycle (exit animations)
// ---------------------------------------------------------------------------

/// Popup state with an exit phase. gpui unmounts an element the frame its
/// state drops, so a closing animation needs the state held alive while
/// [`motion::menu_out`] plays: `open` → `begin_close` (render keeps mounting,
/// with the out animation and dead hit-testing) → [`reap_popup`]'s timer
/// `finish_close`es ~[`motion::MENU_OUT`] later. Use [`Self::is_open`] for
/// logic (a closing popup already reads as closed) and [`Self::get`] /
/// [`Self::is_closing`] for rendering.
pub struct Popup<T> {
    /// `Some((state, closing_since))` while mounted; `closing_since` is the
    /// exit-phase start.
    inner: Option<(T, Option<std::time::Instant>)>,
    /// Whether the popup was still mounted when the current trigger press
    /// began — see [`Self::note_trigger_press`].
    pressed_while_open: bool,
}

impl<T> Default for Popup<T> {
    fn default() -> Self {
        Self {
            inner: None,
            pressed_while_open: false,
        }
    }
}

impl<T> Popup<T> {
    pub fn open(&mut self, value: T) {
        self.inner = Some((value, None));
    }

    /// Open and interactive (not closing).
    pub fn is_open(&self) -> bool {
        matches!(self.inner, Some((_, None)))
    }

    pub fn is_closing(&self) -> bool {
        matches!(self.inner, Some((_, Some(_))))
    }

    /// When the exit phase began — what the render path hands to the popover
    /// wrappers, which derive the eased exit progress from it each frame.
    pub fn closing_since(&self) -> Option<std::time::Instant> {
        match &self.inner {
            Some((_, Some(since))) => Some(*since),
            _ => None,
        }
    }

    /// The state while mounted — open OR playing the exit animation. Render
    /// paths use this; logic paths use [`Self::as_open`]/[`Self::open_mut`].
    pub fn get(&self) -> Option<&T> {
        self.inner.as_ref().map(|(value, _)| value)
    }

    /// The state only while genuinely open — `None` during the exit phase, so
    /// event handlers on a dying popup fall through.
    pub fn as_open(&self) -> Option<&T> {
        match &self.inner {
            Some((value, None)) => Some(value),
            _ => None,
        }
    }

    pub fn open_mut(&mut self) -> Option<&mut T> {
        match &mut self.inner {
            Some((value, None)) => Some(value),
            _ => None,
        }
    }

    /// Enter the exit phase. Returns `true` when this call started it (the
    /// caller then schedules [`reap_popup`]); `false` if already closing or
    /// closed.
    pub fn begin_close(&mut self) -> bool {
        match &mut self.inner {
            Some((_, closing @ None)) => {
                *closing = Some(std::time::Instant::now());
                true
            }
            _ => false,
        }
    }

    /// Record, from the trigger's `on_mouse_down`, whether this popup is
    /// still mounted. The anchored card's `on_mouse_down_out` fires on that
    /// same press and begins the close, so by click (mouse-up) time the
    /// popup already reads as closed — the click handler alone cannot tell
    /// "this press dismissed it; stay closed" from "open fresh", and a
    /// plain toggle closes-and-reopens (user report). Both handler orders
    /// work: open and mid-exit each count as mounted. Every trigger click
    /// is preceded by a trigger mouse-down, so the note is never stale.
    pub fn note_trigger_press(&mut self) {
        self.note_trigger_press_matching(|_| true);
    }

    /// [`Self::note_trigger_press`] for popups whose state distinguishes
    /// which trigger owns them (e.g. one `Popup<PickerKind>` shared by
    /// several triggers): only a press on the OWNING trigger counts, so
    /// clicking a different trigger switches menus instead of swallowing.
    pub fn note_trigger_press_matching(&mut self, owns: impl FnOnce(&T) -> bool) {
        self.pressed_while_open = self.inner.as_ref().is_some_and(|(value, _)| owns(value));
    }

    /// Consume the press note: `true` when the press that produced the
    /// current click found the popup mounted — the click should leave it
    /// closed rather than reopen it.
    pub fn take_press_was_open(&mut self) -> bool {
        std::mem::take(&mut self.pressed_while_open)
    }

    /// Drop the state if the exit phase has run its course. A popup reopened
    /// (or re-closed) since the matching [`begin_close`] is left alone — the
    /// newer phase's own reap handles it.
    pub fn finish_close(&mut self) {
        if let Some((_, Some(since))) = &self.inner
            && since.elapsed() >= motion::MENU_OUT.total().mul_f32(motion::speed_scale())
        {
            self.inner = None;
        }
    }
}

/// Schedule the reap for a [`Popup::begin_close`]: after the exit animation's
/// span, drop the popup state and repaint. `popup` re-borrows the field from
/// the view (the state can't be captured — the view owns it).
pub fn reap_popup<V: 'static, T: 'static>(
    cx: &mut gpui::Context<V>,
    popup: impl Fn(&mut V) -> &mut Popup<T> + 'static,
) {
    cx.spawn(async move |view, cx| {
        cx.background_executor()
            .timer(
                motion::MENU_OUT
                    .total()
                    .mul_f32(motion::speed_scale())
                    .saturating_add(std::time::Duration::from_millis(20)),
            )
            .await;
        view.update(cx, |view, cx| {
            popup(view).finish_close();
            cx.notify();
        })
        .ok();
    })
    .detach();
}

// ---------------------------------------------------------------------------
// Pure reducers
// ---------------------------------------------------------------------------

/// Step the active row of a menu: wraps at both ends; `None` enters at the
/// edge matching the direction. Empty menus stay `None`.
pub fn menu_step(active: Option<usize>, count: usize, delta: isize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let count_i = count as isize;
    let next = match active {
        None => {
            if delta >= 0 {
                0
            } else {
                count_i - 1
            }
        }
        Some(at) => (at as isize + delta).rem_euclid(count_i),
    };
    Some(next as usize)
}

/// Match rank of a label against a query: `0` prefix match, `1` substring,
/// `None` no match. Case-insensitive; an empty query matches everything at
/// rank 1 (input order preserved).
pub fn match_rank(query: &str, label: &str) -> Option<usize> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Some(1);
    }
    let label = label.to_lowercase();
    if label.starts_with(&query) {
        Some(0)
    } else if label.contains(&query) {
        Some(1)
    } else {
        None
    }
}

/// Filter + rank labels for a search query: prefix matches first, then
/// substring matches, stable within each rank. Returns indices into `labels`.
pub fn filter_indices<S: AsRef<str>>(query: &str, labels: &[S]) -> Vec<usize> {
    let mut ranked: Vec<(usize, usize)> = labels
        .iter()
        .enumerate()
        .filter_map(|(ix, label)| match_rank(query, label.as_ref()).map(|rank| (rank, ix)))
        .collect();
    ranked.sort_by_key(|&(rank, ix)| (rank, ix));
    ranked.into_iter().map(|(_, ix)| ix).collect()
}

/// Keys the pickers care about, classified from a raw keystroke.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuKey {
    Up,
    Down,
    /// Plain Enter — activate the highlighted row.
    Enter,
    /// Cmd/Ctrl+Enter — the "pick this folder" accelerator in the browser.
    ModEnter,
    Escape,
    Backspace,
    Other,
}

pub fn classify_key(key: &str, cmd: bool, ctrl: bool) -> MenuKey {
    match key {
        "up" => MenuKey::Up,
        "down" => MenuKey::Down,
        // Readline/emacs motion: ctrl-n/ctrl-p mirror ↓/↑ in every picker.
        // Safe to claim frame-wide — neither chord is a text-editing binding
        // in the palette keymaps, so they always bubble here unconsumed.
        "n" if ctrl => MenuKey::Down,
        "p" if ctrl => MenuKey::Up,
        "enter" if cmd || ctrl => MenuKey::ModEnter,
        "enter" => MenuKey::Enter,
        "escape" => MenuKey::Escape,
        "backspace" => MenuKey::Backspace,
        _ => MenuKey::Other,
    }
}

// ---------------------------------------------------------------------------
// Elements
// ---------------------------------------------------------------------------

/// The floating-menu surface (zeron `.glass-surface` + `menuSurface`):
/// Shared floating surface used by palettes, popovers, dropdowns and menus.
/// Windows 11 uses 8.0px (OverlayCornerRadius) for popovers and context menus.
#[cfg(target_os = "windows")]
pub const CARD_RADIUS: f32 = 8.0;
#[cfg(not(target_os = "windows"))]
pub const CARD_RADIUS: f32 = 12.0;

pub const MENU_GAP: f32 = 2.0;
/// The four-pixel inset of [`popover_card`] that [`menu_scroll_host`] /
/// [`menu_scroll_list`] cancel for card-bleeding scroll hosts.
pub const CARD_INSET: f32 = 4.0;
/// Concentric corners: the row radius follows the card's inset curve.
pub const MENU_ITEM_RADIUS: f32 = CARD_RADIUS - CARD_INSET;
pub const PALETTE_ITEM_RADIUS: f32 = 14.0 - CARD_INSET;

pub fn surface_bg(theme: &Theme) -> gpui::Hsla {
    if theme.is_frost() {
        #[cfg(target_os = "windows")]
        {
            match theme.appearance {
                crate::theme::Appearance::Dark => theme.surface_overlay.opacity(0.55),
                crate::theme::Appearance::Light => theme.surface_overlay.opacity(0.70),
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            theme.glass_overlay()
        }
    } else {
        theme.input_glass_bg()
    }
}

pub fn popover_card(theme: &Theme) -> gpui::Div {
    let border_color = if cfg!(target_os = "windows") {
        if theme.appearance.is_dark() {
            hairline(0.12)
        } else {
            hairline(0.08)
        }
    } else {
        theme.border
    };
    div()
        .border_1()
        .border_color(border_color)
        .rounded(px(CARD_RADIUS))
        .shadow_lg()
        .bg(surface_bg(theme))
        .p(px(CARD_INSET))
        .gap(px(MENU_GAP))
        .overflow_hidden()
        .text_size(crate::typography::ui_rems(13.0))
        .text_color(theme.text)
}

/// [`popover_card`] without the shared inset — for popovers that manage their
/// own internal panes (the harness/model picker's rail + list split).
pub fn popover_card_flush(theme: &Theme) -> gpui::Div {
    popover_card(theme).p(px(0.0))
}

/// Host half of the card-bleed scroll treatment: bleed the rail to the card
/// edge; the inner list ([`menu_scroll_list`]) re-pads so rows stay put.
/// The rail mounts as a SIBLING of the scroller, above its clip — a rail
/// inside the scroller would scroll away with the content. Geometry only:
/// chain the view's own listeners (`.on_hover` for the list-hover note) onto
/// the returned element, plus `.my(px(-CARD_INSET))` when the card carries
/// content above the list and the bleed must run vertically too.
pub fn menu_scroll_host(id: &'static str) -> Stateful<Div> {
    div().id(id).relative().mx(px(-CARD_INSET))
}

/// List half of the card-bleed treatment (see [`menu_scroll_host`]): the
/// re-padded scroller itself. Chain the height budget (`.max_h`), layout
/// (`.flex().flex_col()`), and row children.
pub fn menu_scroll_list(id: &'static str, scroll: &ScrollHandle) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(CARD_INSET))
        .overflow_y_scroll()
        .track_scroll(scroll)
}

/// Shared floating layers and menu surfaces live in focused child modules.
#[path = "popover/anchors.rs"]
mod anchors;
pub use anchors::*;
#[path = "popover/components.rs"]
mod components;
pub use components::*;

// ---------------------------------------------------------------------------
// Floating menu scrollbar
// ---------------------------------------------------------------------------

#[path = "popover/scrollbar.rs"]
mod scrollbar;
pub use scrollbar::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_press_note_distinguishes_dismiss_from_open() {
        let mut popup: Popup<u8> = Popup::default();

        // Fresh open: press finds nothing mounted → click opens.
        popup.note_trigger_press();
        assert!(!popup.take_press_was_open());
        popup.open(1);

        // Trigger click while open: the card's mouse-down-out begins the
        // close on the press (either handler order) — the note still reads
        // mounted, so the click must NOT reopen.
        popup.note_trigger_press();
        popup.begin_close();
        assert!(popup.take_press_was_open());
        // Out-handler first, trigger note second: mid-exit still counts.
        popup.open(1);
        popup.begin_close();
        popup.note_trigger_press();
        assert!(popup.take_press_was_open());

        // The note is consumed — a later click starts clean.
        assert!(!popup.take_press_was_open());

        // Kind-keyed popups: a press on a DIFFERENT trigger doesn't count,
        // so that click switches menus instead of swallowing.
        let mut popup: Popup<u8> = Popup::default();
        popup.open(1);
        popup.note_trigger_press_matching(|kind| *kind == 2);
        assert!(!popup.take_press_was_open());
        popup.note_trigger_press_matching(|kind| *kind == 1);
        assert!(popup.take_press_was_open());
    }

    #[test]
    fn menu_step_wraps_and_enters() {
        // Entering an empty menu stays out.
        assert_eq!(menu_step(None, 0, 1), None);
        assert_eq!(menu_step(Some(3), 0, 1), None);
        // Entering from nothing lands on the matching edge.
        assert_eq!(menu_step(None, 3, 1), Some(0));
        assert_eq!(menu_step(None, 3, -1), Some(2));
        // Stepping wraps both ways.
        assert_eq!(menu_step(Some(2), 3, 1), Some(0));
        assert_eq!(menu_step(Some(0), 3, -1), Some(2));
        assert_eq!(menu_step(Some(1), 3, 1), Some(2));
    }

    #[test]
    fn filter_ranks_prefix_before_substring() {
        let labels = ["main", "feature/main-sync", "master", "dev"];
        // Prefix matches ("main", "master") come before the substring match.
        assert_eq!(filter_indices("ma", &labels), vec![0, 2, 1]);
        // Case-insensitive.
        assert_eq!(filter_indices("MA", &labels), vec![0, 2, 1]);
        // No matches → empty.
        assert!(filter_indices("zzz", &labels).is_empty());
        // Empty / whitespace query keeps input order.
        assert_eq!(filter_indices("", &labels), vec![0, 1, 2, 3]);
        assert_eq!(filter_indices("   ", &labels), vec![0, 1, 2, 3]);
    }

    #[test]
    fn match_rank_kinds() {
        assert_eq!(match_rank("re", "release"), Some(0));
        assert_eq!(match_rank("lease", "release"), Some(1));
        assert_eq!(match_rank("x", "release"), None);
        assert_eq!(match_rank("", "anything"), Some(1));
    }

    #[test]
    fn key_classification() {
        assert_eq!(classify_key("up", false, false), MenuKey::Up);
        assert_eq!(classify_key("down", false, false), MenuKey::Down);
        assert_eq!(classify_key("enter", false, false), MenuKey::Enter);
        assert_eq!(classify_key("enter", true, false), MenuKey::ModEnter);
        assert_eq!(classify_key("enter", false, true), MenuKey::ModEnter);
        assert_eq!(classify_key("escape", false, false), MenuKey::Escape);
        assert_eq!(classify_key("backspace", false, false), MenuKey::Backspace);
        assert_eq!(classify_key("a", false, false), MenuKey::Other);
        // Readline motion — only with ctrl held.
        assert_eq!(classify_key("n", false, true), MenuKey::Down);
        assert_eq!(classify_key("p", false, true), MenuKey::Up);
        assert_eq!(classify_key("n", false, false), MenuKey::Other);
        assert_eq!(classify_key("p", true, false), MenuKey::Other);
    }

    #[test]
    fn tracked_upper_spaces_letters() {
        assert_eq!(tracked_upper("ab"), "A\u{200A}B");
        assert_eq!(
            tracked_upper("Question"),
            "Q\u{200A}U\u{200A}E\u{200A}S\u{200A}T\u{200A}I\u{200A}O\u{200A}N"
        );
        assert_eq!(tracked_upper(""), "");
    }

    #[test]
    fn loadable_accessors() {
        let l: Loadable<u32> = Loadable::Ready(7);
        assert_eq!(l.ready(), Some(&7));
        assert!(!l.is_loading());
        let e: Loadable<u32> = Loadable::Error("boom".into());
        assert_eq!(e.error(), Some("boom"));
        assert!(Loadable::<u32>::Loading.is_loading());
        assert_eq!(Loadable::<u32>::default(), Loadable::Idle);
    }

    #[test]
    fn scrollbar_metrics_hidden_when_content_fits_or_viewport_tiny() {
        // No overflow → no scrollbar.
        assert_eq!(MenuScrollbarMetrics::from_viewport(300.0, 0.0, 0.0), None);
        assert_eq!(MenuScrollbarMetrics::from_viewport(300.0, -5.0, 0.0), None);
        // No viewport → no scrollbar.
        assert_eq!(MenuScrollbarMetrics::from_viewport(0.0, 300.0, 0.0), None);
        // Viewport smaller than two track insets → no track.
        assert_eq!(MenuScrollbarMetrics::from_viewport(8.0, 300.0, 0.0), None);
    }

    #[test]
    fn scrollbar_metrics_scales_thumb_to_content_ratio() {
        let m = MenuScrollbarMetrics::from_viewport(300.0, 300.0, 150.0).unwrap();
        // Track = 300 - 2*4; thumb = half the content (600) → 146.
        assert_eq!(m.track_height, 292.0);
        assert_eq!(m.thumb_height, 146.0);
        assert_eq!(m.travel(), 146.0);
        // Half-scrolled puts the thumb mid-track.
        assert_eq!(m.thumb_top, 73.0);
        assert_eq!(m.max_scroll, 300.0);
    }

    #[test]
    fn scrollbar_metrics_clamps_min_thumb_and_position() {
        let m = MenuScrollbarMetrics::from_viewport(100.0, 9900.0, 4950.0).unwrap();
        // Raw ratio (92 * 100 / 10000 ≈ 0.92px) clamps to the readable minimum.
        assert_eq!(m.thumb_height, MENU_SCROLLBAR_MIN_THUMB);
        assert_eq!(m.travel(), 92.0 - MENU_SCROLLBAR_MIN_THUMB);
        assert_eq!(m.thumb_top, (92.0 - MENU_SCROLLBAR_MIN_THUMB) / 2.0);
        // Overscroll clamps to the bottom of the track.
        let m = MenuScrollbarMetrics::from_viewport(100.0, 9900.0, 99_999.0).unwrap();
        assert_eq!(m.thumb_top, 92.0 - MENU_SCROLLBAR_MIN_THUMB);
        // Negative offsets clamp to the top.
        let m = MenuScrollbarMetrics::from_viewport(100.0, 9900.0, -3.0).unwrap();
        assert_eq!(m.thumb_top, 0.0);
    }

    #[test]
    fn horizontal_scrollbar_metrics_match_the_vertical_treatment() {
        let m = HorizontalScrollbarMetrics::from_viewport(300.0, 300.0, 150.0).unwrap();
        assert_eq!(m.track_width, 292.0);
        assert_eq!(m.thumb_width, 146.0);
        assert_eq!(m.travel(), 146.0);
        assert_eq!(m.thumb_left, 73.0);
        assert_eq!(m.max_scroll, 300.0);
    }

    #[test]
    fn horizontal_scrollbar_hides_without_overflow_and_clamps_position() {
        assert_eq!(
            HorizontalScrollbarMetrics::from_viewport(300.0, 0.0, 0.0),
            None
        );
        let m = HorizontalScrollbarMetrics::from_viewport(100.0, 9900.0, 99_999.0).unwrap();
        assert_eq!(m.thumb_left, 92.0 - MENU_SCROLLBAR_MIN_THUMB);
        let m = HorizontalScrollbarMetrics::from_viewport(100.0, 9900.0, -3.0).unwrap();
        assert_eq!(m.thumb_left, 0.0);
    }

    #[test]
    fn scrollbar_press_and_drag_work_in_the_metrics_domain() {
        let mut bar = MenuScrollbarState::default();
        // track 292, thumb 146, travel 146, thumb at the top.
        let metrics = MenuScrollbarMetrics::from_viewport(300.0, 300.0, 0.0).unwrap();
        let track_top = px(100.0);
        let pointer = |track_y: f32| px(100.0 + MENU_SCROLLBAR_TRACK_INSET + track_y);

        // No press engaged → no target.
        assert_eq!(bar.drag_target_in(&metrics, track_top, pointer(73.0)), None);

        // Pressing the track centers the thumb under the pointer: grab is
        // half the thumb, so the pointer 104px into the track targets the
        // far end.
        bar.begin_press_in(&metrics, track_top, pointer(250.0));
        assert_eq!(
            bar.drag_target_in(&metrics, track_top, pointer(250.0)),
            Some(1.0)
        );
        // Dragging past the top clamps to the very top.
        assert_eq!(
            bar.drag_target_in(&metrics, track_top, pointer(-100.0)),
            Some(0.0)
        );

        // Pressing the thumb keeps its relative position under the pointer:
        // grabbing the thumb's middle and dragging half its travel lands
        // mid-scroll.
        let mut bar = MenuScrollbarState::default();
        bar.begin_press_in(&metrics, track_top, pointer(73.0));
        assert_eq!(
            bar.drag_target_in(&metrics, track_top, pointer(146.0)),
            Some(0.5)
        );
    }

    #[test]
    fn hide_wakes_step_from_linger_end_through_the_fade() {
        let mut bar = MenuScrollbarState::default();
        let t0 = Instant::now();
        // Nothing winding down → nothing to schedule.
        assert_eq!(bar.arm_hide_timer_at(t0), None);

        bar.note_scroll_offset_at(0.0, t0);
        bar.note_scroll_offset_at(10.0, t0);
        // Mid-linger: the next wake is the rest of the linger, so the first
        // repaint lands as the fade starts.
        let mid = t0 + Duration::from_millis(MENU_SCROLLBAR_LINGER_MS / 2);
        assert_eq!(
            bar.arm_hide_timer_at(mid),
            Some(Duration::from_millis(MENU_SCROLLBAR_LINGER_MS / 2))
        );
        // The pending wake fired and rendered; from the fade's first frame
        // on, wakes run at frame cadence so intermediate fade values paint.
        bar.hide_wake_at = None;
        let fade_start = t0 + Duration::from_millis(MENU_SCROLLBAR_LINGER_MS);
        assert_eq!(
            bar.arm_hide_timer_at(fade_start),
            Some(Duration::from_millis(MENU_SCROLLBAR_FADE_FRAME_MS))
        );
        bar.hide_wake_at = None;
        let mid_fade =
            t0 + Duration::from_millis(MENU_SCROLLBAR_LINGER_MS + MENU_SCROLLBAR_FADE_MS / 2);
        assert_eq!(
            bar.arm_hide_timer_at(mid_fade),
            Some(Duration::from_millis(MENU_SCROLLBAR_FADE_FRAME_MS))
        );
        // Past the window the countdown is over — the rail is hidden.
        bar.hide_wake_at = None;
        let past =
            t0 + Duration::from_millis(MENU_SCROLLBAR_LINGER_MS + MENU_SCROLLBAR_FADE_MS + 1);
        assert_eq!(bar.arm_hide_timer_at(past), None);
    }

    #[test]
    fn hide_timer_keeps_one_wake_in_flight() {
        let mut bar = MenuScrollbarState::default();
        let t0 = Instant::now();
        bar.note_scroll_offset_at(0.0, t0);
        bar.note_scroll_offset_at(10.0, t0);
        assert_eq!(
            bar.arm_hide_timer_at(t0),
            Some(Duration::from_millis(MENU_SCROLLBAR_LINGER_MS))
        );
        // A pending wake covers re-renders while it is still in flight; its
        // own render re-arms for whatever the countdown needs then.
        assert_eq!(bar.arm_hide_timer_at(t0 + Duration::from_millis(10)), None);
        // A drag or track-hover holds the rail open — no countdown, no wake.
        bar.hide_wake_at = None;
        bar.begin_press_in(
            &MenuScrollbarMetrics {
                track_height: 300.0,
                thumb_top: 0.0,
                thumb_height: 30.0,
                max_scroll: 1000.0,
            },
            px(0.0),
            px(10.0),
        );
        assert_eq!(bar.arm_hide_timer_at(t0 + Duration::from_millis(20)), None);
    }

    #[test]
    fn scrollbar_metrics_from_parts_matches_viewport_shape() {
        // Content extent + offset instead of a max-scroll distance (the
        // terminal's emulator geometry).
        let from_parts = MenuScrollbarMetrics::from_parts(300.0, 600.0, 150.0);
        assert_eq!(
            from_parts,
            MenuScrollbarMetrics::from_viewport(300.0, 300.0, 150.0)
        );
        // Content that fits the viewport stays rail-less.
        assert_eq!(MenuScrollbarMetrics::from_parts(300.0, 200.0, 0.0), None);
    }

    #[test]
    fn reset_menu_scroll_rewinds_without_marking_motion() {
        let scroll = ScrollHandle::new();
        let mut bar = MenuScrollbarState::default();
        bar.note_scroll(&scroll);
        reset_menu_scroll(&scroll, &mut bar);
        assert_eq!(scroll.offset(), Point::default());
        // The re-observed top offset is a fresh baseline, not motion.
        bar.note_scroll(&scroll);
        assert!(!bar.visible());
    }
}

/// Compact mention-style match washes preserve the row's font and spacing.
pub(crate) fn search_highlight(
    text: SharedString,
    query: Option<&str>,
    theme: &Theme,
) -> AnyElement {
    let Some(query) = query.filter(|query| !query.trim().is_empty()) else {
        return text.into_any_element();
    };
    let ranges = search_match_ranges(&text, query);
    if ranges.is_empty() {
        return text.into_any_element();
    }
    let badges = ranges;
    let styled = gpui::StyledText::new(text).with_highlights(badges.iter().cloned().map(|range| {
        (
            range,
            gpui::HighlightStyle {
                color: Some(theme.code_text),
                ..Default::default()
            },
        )
    }));
    let layout = styled.layout().clone();
    let wash = theme.code_wash;
    // As in the composer, paint rounded backgrounds beneath shaped glyphs.
    // A TextRun background would be square and can disappear when clipped.
    let underlay = gpui::canvas(
        |_, _, _| (),
        move |_, _, window, _| {
            for range in &badges {
                for mut bounds in crate::markdown::render::range_rects(&layout, range, 1.0, 1.5) {
                    // Glyphs sit below the line box's optical center. Shift the
                    // wash down half a logical pixel to balance visible top/bottom padding.
                    bounds.origin.y += px(0.5);
                    window.paint_quad(gpui::quad(
                        bounds,
                        px(3.0),
                        wash,
                        px(0.0),
                        gpui::transparent_black(),
                        gpui::BorderStyle::default(),
                    ));
                }
            }
        },
    )
    .absolute()
    .size_full();
    div()
        .relative()
        .child(underlay)
        .child(styled)
        .into_any_element()
}

fn search_match_ranges(text: &str, query: &str) -> Vec<std::ops::Range<usize>> {
    let folded = text.to_lowercase();
    let mut original = Vec::with_capacity(folded.len());
    for (start, ch) in text.char_indices() {
        let range = start..start + ch.len_utf8();
        for lower in ch.to_lowercase() {
            original.extend(std::iter::repeat_n(range.clone(), lower.len_utf8()));
        }
    }
    let mut ranges = Vec::new();
    for word in query.to_lowercase().split_whitespace() {
        for (start, _) in folded.match_indices(word) {
            ranges.push(original[start].start..original[start + word.len() - 1].end);
        }
    }
    ranges.sort_by_key(|range| range.start);
    let mut merged: Vec<std::ops::Range<usize>> = Vec::new();
    for range in ranges {
        if let Some(last) = merged.last_mut()
            && range.start <= last.end
        {
            last.end = last.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    merged
}

#[cfg(test)]
mod search_highlight_tests {
    use super::search_match_ranges;

    #[test]
    fn inline_matches_keep_adjacent_word_boundaries() {
        let text = "fieldnotes/fix-authentication-redirects";
        let ranges = search_match_ranges(text, "authentication");
        assert_eq!(&text[..ranges[0].start], "fieldnotes/fix-");
        assert_eq!(&text[ranges[0].clone()], "authentication");
        assert_eq!(&text[ranges[0].end..], "-redirects");
    }

    #[test]
    fn highlights_repeated_case_insensitive_and_overlapping_words() {
        assert_eq!(
            search_match_ranges("New chat, new project", "NEW"),
            vec![0..3, 10..13]
        );
        assert_eq!(
            search_match_ranges("authentication", "auth authentication"),
            vec![0..14]
        );
        assert!(search_match_ranges("New chat", "  ").is_empty());
        assert!(search_match_ranges("New chat", "settings").is_empty());
    }

    #[test]
    fn preserves_original_unicode_boundaries_after_lowercase_expansion() {
        assert_eq!(
            search_match_ranges("İstanbul café", "i CAFÉ"),
            vec![0..2, 10..15]
        );
        assert_eq!(search_match_ranges("🚀 CAFÉ", "café"), vec![5..10]);
    }
}
