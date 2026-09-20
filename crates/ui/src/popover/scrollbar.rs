//! Vertical and horizontal floating scrollbar primitives.

use super::*;

// ---------------------------------------------------------------------------
// Floating menu scrollbar — the model-list treatment, shared
// ---------------------------------------------------------------------------

/// Track inset top/bottom; the thumb travels inside it.
pub const MENU_SCROLLBAR_TRACK_INSET: f32 = 4.0;
/// Invisible hit strip width on the right edge.
pub const MENU_SCROLLBAR_HIT_WIDTH: f32 = 10.0;
/// Resting thumb width.
pub const MENU_SCROLLBAR_THUMB_WIDTH: f32 = 3.0;
/// Thumb width while hovered/dragged.
pub const MENU_SCROLLBAR_HOVER_THUMB_WIDTH: f32 = 5.0;
/// Smallest readable thumb on very long lists.
pub const MENU_SCROLLBAR_MIN_THUMB: f32 = 24.0;
/// How long the rail stays fully visible after the last scroll motion.
pub const MENU_SCROLLBAR_LINGER_MS: u64 = 1400;
/// How long the rail takes to fade out after the linger window.
pub const MENU_SCROLLBAR_FADE_MS: u64 = 260;
/// Repaint cadence through the fade window — each wake repaints the next
/// intermediate [`MenuScrollbarState::fade`] value.
const MENU_SCROLLBAR_FADE_FRAME_MS: u64 = 16;

/// Geometry of the floating thumb for a scroll viewport at one instant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MenuScrollbarMetrics {
    pub track_height: f32,
    pub thumb_top: f32,
    pub thumb_height: f32,
    pub max_scroll: f32,
}

impl MenuScrollbarMetrics {
    /// Distance the thumb itself can travel.
    pub fn travel(self) -> f32 {
        (self.track_height - self.thumb_height).max(0.0)
    }

    /// Pure geometry from the viewport and scroll distances. `None` when the
    /// content fits (`max_scroll <= 0`) or the viewport is too small to hold
    /// a track.
    pub fn from_viewport(
        viewport_height: f32,
        max_scroll: f32,
        current_scroll: f32,
    ) -> Option<Self> {
        let max_scroll = max_scroll.max(0.0);
        if viewport_height <= 0.0 || max_scroll <= 0.0 {
            return None;
        }
        let track_height = (viewport_height - MENU_SCROLLBAR_TRACK_INSET * 2.0).max(0.0);
        if track_height <= 0.0 {
            return None;
        }
        let content_height = viewport_height + max_scroll;
        let thumb_height = (track_height * viewport_height / content_height)
            .max(MENU_SCROLLBAR_MIN_THUMB)
            .min(track_height);
        let current_scroll = current_scroll.clamp(0.0, max_scroll);
        let travel = (track_height - thumb_height).max(0.0);
        Some(Self {
            track_height,
            thumb_top: travel * current_scroll / max_scroll,
            thumb_height,
            max_scroll,
        })
    }

    /// [`Self::from_viewport`] from raw viewport/content extents and the
    /// current offset — the shape of owners that know a total content height
    /// rather than a max-scroll distance (the terminal's emulator geometry:
    /// total vs. visible rows).
    pub fn from_parts(viewport_height: f32, content_height: f32, offset_y: f32) -> Option<Self> {
        Self::from_viewport(viewport_height, content_height - viewport_height, offset_y)
    }
}

/// Marker for GPUI's captured drag stream. The actual grab geometry stays in
/// [`MenuScrollbarState`] so a track click can center the thumb first.
pub struct MenuScrollbarDrag;

/// Invisible drag preview: scrollbar drags manipulate the existing thumb.
pub struct MenuScrollbarDragGhost;

impl gpui::Render for MenuScrollbarDragGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl gpui::IntoElement {
        gpui::Empty
    }
}

/// Hover/drag interaction state for one floating scrollbar, owned by the view
/// that renders the list. Event handlers stay on the view (they need its
/// listeners) and delegate here; only one list surface owns a state at a
/// time, so mutually exclusive popups may share one instance.
///
/// Visibility: the rail paints while a drag or a track-hover holds it open,
/// or while scroll motion is recent ([`Self::note_scroll`]) — scrolling
/// shows it with or without hover; hovering the list alone shows nothing.
/// When the motion stops the rail lingers [`MENU_SCROLLBAR_LINGER_MS`], fades
/// over [`MENU_SCROLLBAR_FADE_MS`], and disappears — but hopping onto the
/// track mid-linger freezes it open, and leaving the track restarts the wait
/// (it never hides under the pointer). [`schedule_scrollbar_hide`] keeps
/// wake-ups scheduled so the fade lands without any further input.
#[derive(Default)]
pub struct MenuScrollbarState {
    list_hovered: bool,
    bar_hovered: bool,
    grab: Option<f32>,
    /// Last seen scroll position — a change marks fresh scroll activity.
    last_scroll_y: Option<f32>,
    /// When that change was seen.
    last_scroll_at: Option<Instant>,
    /// When the in-flight hide wake-up fires; one wake is in flight at a
    /// time, and its render re-arms while the countdown runs.
    hide_wake_at: Option<Instant>,
}

impl MenuScrollbarState {
    /// Record scroll activity from the live handle. Call once per render
    /// before [`Self::metrics`]/[`Self::render_rail`]; a changed offset marks
    /// the rail as recently scrolled.
    pub fn note_scroll(&mut self, scroll: &gpui::ScrollHandle) {
        self.note_scroll_offset(-f32::from(scroll.offset().y));
    }

    /// Same as [`Self::note_scroll`] for owners whose scroll position is not
    /// a gpui handle (the terminal's display offset, for one) — any value
    /// that changes exactly when the visible scroll position does.
    pub fn note_scroll_offset(&mut self, position: f32) {
        self.note_scroll_offset_at(position, Instant::now());
    }

    fn note_scroll_offset_at(&mut self, position: f32, at: Instant) {
        match self.last_scroll_y {
            // First observation: establish the baseline only — a fresh mount
            // always reports its initial offset, and that is not scrolling.
            None => self.last_scroll_y = Some(position),
            Some(previous) if previous != position => {
                self.last_scroll_y = Some(position);
                self.last_scroll_at = Some(at);
            }
            Some(_) => {}
        }
    }

    /// Forget the baseline entirely: the next [`Self::note_scroll`] observes
    /// whatever offset the layout settled on (e.g. after `scroll_to_item`)
    /// as the fresh baseline, again without marking motion.
    pub fn clear_scroll_baseline(&mut self) {
        self.last_scroll_y = None;
        self.last_scroll_at = None;
    }

    fn scroll_countdown(&self, now: Instant) -> bool {
        self.last_scroll_at.is_some_and(|at| {
            now.duration_since(at)
                < Duration::from_millis(MENU_SCROLLBAR_LINGER_MS + MENU_SCROLLBAR_FADE_MS)
        })
    }

    /// Metrics from any scroll handle's live bounds/offset — both
    /// `ScrollHandle` and a virtualized list's base handle qualify. `None`
    /// when the content fits.
    pub fn metrics(&self, scroll: &gpui::ScrollHandle) -> Option<MenuScrollbarMetrics> {
        let bounds = scroll.bounds();
        // GPUI stores the maximum as a positive distance; only the live
        // scroll offset is negative while content moves upward.
        let max_scroll = f32::from(scroll.max_offset().y).max(0.0);
        let current_scroll = (-f32::from(scroll.offset().y)).clamp(0.0, max_scroll);
        MenuScrollbarMetrics::from_viewport(
            f32::from(bounds.size.height),
            max_scroll,
            current_scroll,
        )
    }
    /// Whether the rail paints at all: a drag or a track-hover holds it
    /// open, otherwise **recent scroll motion** does — hovering the list
    /// alone shows nothing, scrolling shows it even with the pointer
    /// elsewhere (touchpad momentum), and hover+scroll is the common case.
    pub fn visible(&self) -> bool {
        self.grab.is_some() || self.bar_hovered || self.scroll_countdown(Instant::now())
    }

    /// 1 → 0 across the fade window once the scroll motion stops; full while
    /// a drag or a track-hover holds the rail open.
    pub fn fade(&self) -> f32 {
        if self.grab.is_some() || self.bar_hovered {
            return 1.0;
        }
        let Some(at) = self.last_scroll_at else {
            return 0.0;
        };
        let elapsed = at.elapsed().as_millis() as f32;
        let linger = MENU_SCROLLBAR_LINGER_MS as f32;
        let fade = MENU_SCROLLBAR_FADE_MS as f32;
        (1.0 - (elapsed - linger) / fade).clamp(0.0, 1.0)
    }

    fn animating_at(&self, now: Instant) -> bool {
        self.grab.is_none() && !self.bar_hovered && self.scroll_countdown(now)
    }

    /// When the countdown next needs a repaint: the rest of the linger (the
    /// wake lands as the fade starts), then frame steps through the fade so
    /// [`Self::fade`]'s intermediate values actually get painted. `None`
    /// when nothing is winding down or the window has fully elapsed.
    fn next_wake_at(&self, now: Instant) -> Option<Instant> {
        if !self.animating_at(now) {
            return None;
        }
        let linger = Duration::from_millis(MENU_SCROLLBAR_LINGER_MS);
        let total = Duration::from_millis(MENU_SCROLLBAR_LINGER_MS + MENU_SCROLLBAR_FADE_MS);
        let motion = self.last_scroll_at?;
        let elapsed = now.duration_since(motion);
        if elapsed < linger {
            Some(motion + linger)
        } else if elapsed < total {
            Some(now + Duration::from_millis(MENU_SCROLLBAR_FADE_FRAME_MS))
        } else {
            None
        }
    }

    /// Arm the next hide wake-up and return how long the caller should wait.
    /// `None` when there is nothing to schedule. One wake is in flight at a
    /// time; its render re-arms for whatever the countdown needs then, so
    /// resumed scrolling or a refreshed linger converges on the next wake.
    pub fn arm_hide_timer(&mut self) -> Option<Duration> {
        self.arm_hide_timer_at(Instant::now())
    }

    fn arm_hide_timer_at(&mut self, now: Instant) -> Option<Duration> {
        let Some(wake) = self.next_wake_at(now) else {
            self.hide_wake_at = None;
            return None;
        };
        if self.hide_wake_at.is_some_and(|pending| pending > now) {
            return None;
        }
        self.hide_wake_at = Some(wake);
        Some(wake - now)
    }

    /// Whether the thumb carries the expanded/stronger treatment.
    pub fn active(&self) -> bool {
        self.bar_hovered || self.grab.is_some()
    }

    /// The pointer entered/left the LIST. Returns whether anything changed.
    pub fn set_list_hovered(&mut self, hovered: bool) -> bool {
        if self.list_hovered == hovered {
            return false;
        }
        self.list_hovered = hovered;
        if !hovered && self.grab.is_none() && self.bar_hovered {
            // Leaving the host straight off the track: restart the linger so
            // the rail doesn't vanish under a departing pointer (the strip's
            // own leave event usually does this first).
            self.bar_hovered = false;
            self.last_scroll_at = Some(Instant::now());
        }
        true
    }

    /// The pointer entered/left the RAIL. Keeps the active treatment while a
    /// captured drag travels outside (the hover callback correctly turns
    /// false there). Leaving the track after the rail lingered restarts the
    /// wait — it hides only a beat later, not mid-hover. Returns whether
    /// anything changed.
    pub fn set_bar_hovered(&mut self, hovered: bool) -> bool {
        let active = hovered || self.grab.is_some();
        if self.bar_hovered == active {
            return false;
        }
        self.bar_hovered = active;
        if !active {
            self.last_scroll_at = Some(Instant::now());
        }
        true
    }

    /// A press landed on the rail: choose the grab point (pressing the thumb
    /// keeps its relative position; pressing the track centers the thumb
    /// under the pointer first), engage the drag, and scroll to the pointer.
    /// `false` when there is nothing to scroll.
    pub fn begin_press(&mut self, scroll: &ScrollHandle, pointer_y: Pixels) -> bool {
        let Some(metrics) = self.metrics(scroll) else {
            return false;
        };
        self.begin_press_in(&metrics, scroll.bounds().top(), pointer_y);
        self.drag_to(scroll, pointer_y);
        true
    }

    /// [`Self::begin_press`] in the metrics domain, for owners with no
    /// `ScrollHandle` (virtualized lists): engage the grab from raw geometry
    /// — `track_top` is the window y of the track's bounds (the scroller's
    /// top). Apply the initial target with [`Self::drag_target_in`].
    pub fn begin_press_in(
        &mut self,
        metrics: &MenuScrollbarMetrics,
        track_top: Pixels,
        pointer_y: Pixels,
    ) {
        let pointer_in_track = Self::pointer_in_track_at(track_top, pointer_y);
        let grab_offset = if (metrics.thumb_top..=metrics.thumb_top + metrics.thumb_height)
            .contains(&pointer_in_track)
        {
            pointer_in_track - metrics.thumb_top
        } else {
            metrics.thumb_height / 2.0
        };
        self.grab = Some(grab_offset);
    }

    /// Move an engaged drag to `pointer_y`. `false` when no drag is engaged
    /// or the content stopped scrolling mid-drag.
    pub fn drag_to(&self, scroll: &ScrollHandle, pointer_y: Pixels) -> bool {
        let Some(metrics) = self.metrics(scroll) else {
            return false;
        };
        let Some(fraction) = self.drag_target_in(&metrics, scroll.bounds().top(), pointer_y) else {
            return false;
        };
        let offset = scroll.offset();
        scroll.set_offset(Point::new(offset.x, px(-fraction * metrics.max_scroll)));
        true
    }

    /// [`Self::drag_to`] in the metrics domain: the target scroll position as
    /// a fraction of `metrics.max_scroll` (apply it in the owner's own scroll
    /// units). `None` when no drag is engaged; a full-track thumb (zero
    /// travel) always targets 0.
    pub fn drag_target_in(
        &self,
        metrics: &MenuScrollbarMetrics,
        track_top: Pixels,
        pointer_y: Pixels,
    ) -> Option<f32> {
        let grab_offset = self.grab?;
        let pointer_in_track = Self::pointer_in_track_at(track_top, pointer_y);
        let thumb_top = (pointer_in_track - grab_offset).clamp(0.0, metrics.travel());
        Some(if metrics.travel() <= 0.0 {
            0.0
        } else {
            thumb_top / metrics.travel()
        })
    }

    /// The press ended anywhere: drop the drag; the rail stays armed only
    /// while the list is still hovered. Returns whether anything changed.
    pub fn end_press(&mut self) -> bool {
        self.grab = None;
        // Releasing a drag lingers like a stopped scroll.
        self.last_scroll_at = Some(Instant::now());
        if !self.list_hovered && self.bar_hovered {
            self.bar_hovered = false;
            true
        } else {
            false
        }
    }

    /// Window-y → track-y; `track_top` is the window y of the track's bounds.
    fn pointer_in_track_at(track_top: Pixels, pointer_y: Pixels) -> f32 {
        f32::from(pointer_y - track_top) - MENU_SCROLLBAR_TRACK_INSET
    }

    /// The positioned rail visuals: a full-height hit strip on the right with
    /// the thumb inside. `None` while hidden or the content fits. Prefer
    /// [`rail`], which layers identity + the six standard pointer listeners
    /// onto this strip; reach for the raw element only when a surface needs
    /// different listeners. Either way `.id(...)` comes first (hover needs
    /// element state).
    pub fn render_rail(&self, theme: &Theme, metrics: MenuScrollbarMetrics) -> Option<Div> {
        if !self.visible() {
            return None;
        }
        let active = self.active();
        let thumb_width = if active {
            MENU_SCROLLBAR_HOVER_THUMB_WIDTH
        } else {
            MENU_SCROLLBAR_THUMB_WIDTH
        };
        Some(
            div()
                .absolute()
                .top(px(0.0))
                .bottom(px(0.0))
                .right(px(0.0))
                .w(px(MENU_SCROLLBAR_HIT_WIDTH))
                // Post-scroll fade: full while hovered/held, then 1 → 0
                // across the fade window (see [`MenuScrollbarState::fade`]).
                .opacity(self.fade())
                // The thumb is an absolute child inside a fixed-width hit
                // rail, so hover expansion never reflows rows. Hovering the
                // thumb itself brightens it on top of the strip-hover
                // widening — pure paint-level hover styling, no notify.
                .child(
                    div()
                        .id("scrollbar-thumb")
                        .absolute()
                        .top(px(MENU_SCROLLBAR_TRACK_INSET + metrics.thumb_top))
                        .right(px(2.0))
                        .w(px(thumb_width))
                        .h(px(metrics.thumb_height))
                        .rounded(px(thumb_width / 2.0))
                        .bg(theme.text_faint.opacity(if active { 0.68 } else { 0.5 }))
                        .hover(|s| s.bg(theme.text_faint.opacity(0.85))),
                ),
        )
    }
}

/// Keep wake-ups scheduled while a scroll-triggered linger/fade countdown
/// runs: one wake at the end of the linger (the fade's first repaint), then
/// frame-cadence wakes through the fade, so the rail fades out and hides
/// [`MENU_SCROLLBAR_LINGER_MS`] + [`MENU_SCROLLBAR_FADE_MS`] after the last
/// scroll motion instead of sticking until the next unrelated render. Call
/// once per render after the rail's activity note
/// ([`MenuScrollbarState::note_scroll`] / [`MenuScrollbarState::note_scroll_offset`]);
/// at most one wake is in flight at a time.
pub fn schedule_scrollbar_hide<T: 'static>(bar: &mut MenuScrollbarState, cx: &mut Context<T>) {
    let Some(delay) = bar.arm_hide_timer() else {
        return;
    };
    cx.spawn(async move |this, cx| {
        cx.background_executor().timer(delay).await;
        this.update(cx, |_, cx| cx.notify()).ok();
    })
    .detach();
}

/// The view-side halves of one floating rail. Implement on the view that owns
/// the list, and [`rail`] folds the whole treatment — activity note, hide
/// scheduling, metrics, visuals, and all six pointer listeners — into one
/// call.
pub trait ScrollRailHost: 'static {
    /// The rail's hover/drag interaction state.
    fn rail_bar(&mut self) -> &mut MenuScrollbarState;

    /// The scroll handle feeding the rail. `None` for handle-less owners
    /// (virtualized lists, the terminal), which override [`Self::rail_metrics`]
    /// and [`Self::rail_press`]/[`Self::rail_drag_to`] to work from their own
    /// geometry.
    fn rail_scroll(&self) -> Option<ScrollHandle> {
        None
    }

    /// Record this frame's scroll activity and return the rail geometry.
    /// `None` while the content fits. The default notes + measures
    /// [`Self::rail_scroll`]; handle-less owners override to build
    /// [`MenuScrollbarMetrics`] from their own geometry and report their
    /// position proxy via [`MenuScrollbarState::note_scroll_offset`].
    fn rail_metrics(&mut self) -> Option<MenuScrollbarMetrics> {
        let scroll = self.rail_scroll()?;
        self.rail_bar().note_scroll(&scroll);
        self.rail_bar().metrics(&scroll)
    }

    /// A press landed on the rail at window-y `pointer_y`; `true` when it
    /// engaged a drag (the event is then consumed). The default presses
    /// [`Self::rail_scroll`]'s handle; handle-less owners engage via
    /// [`MenuScrollbarState::begin_press_in`] and apply the initial
    /// [`MenuScrollbarState::drag_target_in`] target themselves.
    fn rail_press(&mut self, pointer_y: Pixels) -> bool {
        self.rail_scroll()
            .is_some_and(|scroll| self.rail_bar().begin_press(&scroll, pointer_y))
    }

    /// An engaged drag moved to window-y `pointer_y`; `true` when it moved
    /// the scroll position. The default drags [`Self::rail_scroll`]'s handle;
    /// handle-less owners apply [`MenuScrollbarState::drag_target_in`]
    /// themselves.
    fn rail_drag_to(&mut self, pointer_y: Pixels) -> bool {
        self.rail_scroll()
            .is_some_and(|scroll| self.rail_bar().drag_to(&scroll, pointer_y))
    }
}

/// The whole floating-scrollbar treatment for any [`ScrollRailHost`] view:
/// note the frame's scroll activity, keep the hide countdown scheduled,
/// render the rail, and wire the six pointer listeners (strip hover, press,
/// drag capture, drag move, and both mouse-up ends) onto it. The drag-move
/// listener rides on the strip itself — gpui dispatches the captured drag
/// stream to it wherever the pointer travels. `None` while the rail is
/// hidden or the content fits; the host keeps only its own list-hover
/// listener. Handle-less owners get their overrides called at the same
/// points the default handle calls would be.
pub fn rail<V: ScrollRailHost>(
    host: &mut V,
    id: &'static str,
    theme: &Theme,
    cx: &mut Context<V>,
) -> Option<AnyElement> {
    let metrics = host.rail_metrics()?;
    schedule_scrollbar_hide(host.rail_bar(), cx);
    let strip = host.rail_bar().render_rail(theme, metrics)?;
    Some(
        strip
            .id(id)
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if this.rail_bar().set_bar_hovered(*hovered) {
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    if this.rail_press(event.position.y) {
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .on_drag(MenuScrollbarDrag, |_, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| MenuScrollbarDragGhost)
            })
            .on_drag_move(cx.listener(
                |this, event: &gpui::DragMoveEvent<MenuScrollbarDrag>, _, cx| {
                    if this.rail_drag_to(event.event.position.y) {
                        cx.notify();
                    }
                },
            ))
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    this.rail_bar().end_press();
                    cx.notify();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    this.rail_bar().end_press();
                    cx.notify();
                }),
            )
            .into_any_element(),
    )
}

/// Rewind a handle-owned menu list to its top and drop its rail activity
/// baseline in one step — the pair every "reopen this menu fresh" site must
/// make. Skipping the offset half strands the list mid-scroll; skipping the
/// baseline half lets the next note read the jump back to the top as
/// scrolling and show the rail.
pub fn reset_menu_scroll(scroll: &ScrollHandle, bar: &mut MenuScrollbarState) {
    scroll.set_offset(Point::default());
    bar.clear_scroll_baseline();
}

// ---------------------------------------------------------------------------
// Horizontal floating scrollbar — the same quiet rail used by menus, rotated
// for local code planes. Kept separate from `MenuScrollbarState` so a code
// fence can own one state per stable block while existing menu callers retain
// their vertical API.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HorizontalScrollbarMetrics {
    pub track_width: f32,
    pub thumb_left: f32,
    pub thumb_width: f32,
    pub max_scroll: f32,
}

impl HorizontalScrollbarMetrics {
    pub fn travel(self) -> f32 {
        (self.track_width - self.thumb_width).max(0.0)
    }

    pub fn from_viewport(
        viewport_width: f32,
        max_scroll: f32,
        current_scroll: f32,
    ) -> Option<Self> {
        let max_scroll = max_scroll.max(0.0);
        if viewport_width <= 0.0 || max_scroll <= 0.0 {
            return None;
        }
        let track_width = (viewport_width - MENU_SCROLLBAR_TRACK_INSET * 2.0).max(0.0);
        if track_width <= 0.0 {
            return None;
        }
        let content_width = viewport_width + max_scroll;
        let thumb_width = (track_width * viewport_width / content_width)
            .max(MENU_SCROLLBAR_MIN_THUMB)
            .min(track_width);
        let current_scroll = current_scroll.clamp(0.0, max_scroll);
        let travel = (track_width - thumb_width).max(0.0);
        Some(Self {
            track_width,
            thumb_left: travel * current_scroll / max_scroll,
            thumb_width,
            max_scroll,
        })
    }
}

/// Hover/drag state for one horizontal code viewport. Geometry comes from the
/// same tracked [`gpui::ScrollHandle`] that moves the code, so the thumb always
/// represents the block's real local overflow (virtual transcript height is
/// irrelevant here).
#[derive(Default)]
pub struct HorizontalScrollbarState {
    viewport_hovered: bool,
    bar_hovered: bool,
    grab: Option<f32>,
}

impl HorizontalScrollbarState {
    pub fn metrics(&self, scroll: &gpui::ScrollHandle) -> Option<HorizontalScrollbarMetrics> {
        let bounds = scroll.bounds();
        let max_scroll = f32::from(scroll.max_offset().x).max(0.0);
        let current_scroll = (-f32::from(scroll.offset().x)).clamp(0.0, max_scroll);
        HorizontalScrollbarMetrics::from_viewport(
            f32::from(bounds.size.width),
            max_scroll,
            current_scroll,
        )
    }

    pub fn visible(&self) -> bool {
        self.viewport_hovered || self.grab.is_some()
    }

    pub fn active(&self) -> bool {
        self.bar_hovered || self.grab.is_some()
    }

    pub fn set_viewport_hovered(&mut self, hovered: bool) -> bool {
        if self.viewport_hovered == hovered {
            return false;
        }
        self.viewport_hovered = hovered;
        if !hovered && self.grab.is_none() {
            self.bar_hovered = false;
        }
        true
    }

    pub fn set_bar_hovered(&mut self, hovered: bool) -> bool {
        let active = hovered || self.grab.is_some();
        if self.bar_hovered == active {
            return false;
        }
        self.bar_hovered = active;
        true
    }

    pub fn begin_press(&mut self, scroll: &gpui::ScrollHandle, pointer_x: Pixels) -> bool {
        let Some(metrics) = self.metrics(scroll) else {
            return false;
        };
        let pointer_in_track = self.pointer_in_track(scroll, pointer_x);
        let grab_offset = if (metrics.thumb_left..=metrics.thumb_left + metrics.thumb_width)
            .contains(&pointer_in_track)
        {
            pointer_in_track - metrics.thumb_left
        } else {
            metrics.thumb_width / 2.0
        };
        self.grab = Some(grab_offset);
        self.drag_to(scroll, pointer_x);
        true
    }

    pub fn drag_to(&self, scroll: &gpui::ScrollHandle, pointer_x: Pixels) -> bool {
        let Some(grab_offset) = self.grab else {
            return false;
        };
        let Some(metrics) = self.metrics(scroll) else {
            return false;
        };
        let thumb_left =
            (self.pointer_in_track(scroll, pointer_x) - grab_offset).clamp(0.0, metrics.travel());
        let scroll_to = if metrics.travel() <= 0.0 {
            0.0
        } else {
            thumb_left / metrics.travel() * metrics.max_scroll
        };
        let offset = scroll.offset();
        scroll.set_offset(gpui::Point::new(px(-scroll_to), offset.y));
        true
    }

    pub fn end_press(&mut self) -> bool {
        self.grab = None;
        if !self.viewport_hovered && self.bar_hovered {
            self.bar_hovered = false;
            return true;
        }
        false
    }

    fn pointer_in_track(&self, scroll: &gpui::ScrollHandle, pointer_x: Pixels) -> f32 {
        f32::from(pointer_x - scroll.bounds().left()) - MENU_SCROLLBAR_TRACK_INSET
    }
}
