//! Layout sizing, pane resize drag physics, and tween animations.

use std::time::Duration;

/// The cover transition window before the deferred surface swap: the tree
/// overlay's ease duration plus a frame of margin.
const SURFACE_COVER_MS: u64 = RESIZE.duration_ms + 40;

use super::*;
use crate::motion::{self, RESIZE};
use crate::settings::{self, SavePolicy};
use crate::theme::Theme;
use gpui::{
    AnyElement, App, Context, Entity, IntoElement, MouseButton, MouseUpEvent, Point, Window, div,
    px,
};

/// Vertical pane resize hitboxes yield the global titlebar. Keeping this in
/// the shared constructor makes left/right seams mirror each other and avoids
/// relying on paint order when chrome crosses an animated pane boundary.
pub(crate) const PANE_RESIZE_HITBOX_HALF_WIDTH: f32 = 10.0;
pub(crate) const PANE_RESIZE_HITBOX_TOP: f32 = Theme::TITLEBAR_HEIGHT;

pub(crate) fn stable_panel_content_width(target: f32, transition: Option<(f32, f32)>) -> f32 {
    transition.map(|(from, to)| from.max(to)).unwrap_or(target)
}

pub(crate) fn right_panel_content_width(
    target: f32,
    transition: Option<(f32, f32)>,
    takeover_width: Option<f32>,
) -> f32 {
    takeover_width.unwrap_or_else(|| stable_panel_content_width(target, transition))
}

pub(crate) fn conversation_width(viewport: f32, sidebar: f32, right: f32) -> f32 {
    (viewport - sidebar - right).max(0.0)
}

/// Maximum width the right pane may occupy while retaining the conversation
/// floor. On unusually small windows this deliberately falls below the right
/// pane's preferred minimum: the chat remains usable and the side surface
/// yields the scarce space.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn right_pane_max_width(viewport: f32, sidebar: f32) -> f32 {
    right_pane_max_width_scaled(viewport, sidebar, 1.0)
}

pub(crate) fn right_pane_max_width_scaled(viewport: f32, sidebar: f32, scale: f32) -> f32 {
    let scale = if scale <= 0.0 { 1.0 } else { scale };
    (viewport - sidebar - CHAT_PANEL_MIN * scale).max(0.0)
}

/// Width used by right-pane takeover. Unlike manual resizing, takeover is
/// intentionally allowed to consume the conversation column completely.
pub(crate) fn right_pane_takeover_width(viewport: f32, sidebar: f32) -> f32 {
    (viewport - sidebar).max(0.0)
}

/// Drag marker for the sidebar resize handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SidebarResize;

/// Drag marker for the right-pane resize handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RightPaneResize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PaneResizeKind {
    Sidebar,
    Right,
    Terminal,
}

/// Resolve one pointer sample while keeping the persisted width legal. The
/// edge is latched by the caller, so a held pointer produces one nudge rather
/// than restarting the animation for every drag event.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn sidebar_drag_sample(
    pointer_x: f32,
    latched_edge: Option<motion::ResizeEdge>,
    reduced_motion: bool,
) -> motion::ResizeDragSample {
    motion::resize_drag_sample(
        pointer_x,
        SIDEBAR_MIN,
        SIDEBAR_MAX,
        latched_edge,
        reduced_motion,
    )
}

/// A oneshot width tween (200ms ease-out), driven MANUALLY from render via
/// [`Shell::eval_tween`] — never through a `with_animation` wrapper. gpui keys
/// an animation element's start time by its full global element-id path, so a
/// wrapper that mounts/remounts (route swap, or an ancestor animation keyed by
/// a fresh epoch) silently REPLAYS the tween from t=0. Manual evaluation keeps
/// the element tree's shape constant: a finished or stale tween is exactly the
/// steady state, no matter how the tree around it remounts (round-6 §1–3).
#[derive(Debug, Clone, Copy)]
pub(crate) struct WidthTween {
    pub(crate) from: f32,
    pub(crate) to: f32,
    pub(crate) started: std::time::Instant,
}

impl WidthTween {
    pub(crate) fn new(from: f32, to: f32) -> Self {
        Self {
            from,
            to,
            started: std::time::Instant::now(),
        }
    }
}

impl Shell {
    // ---- layout state ----

    pub(crate) fn ui_scale(&self) -> f32 {
        let px = self.settings.ui_font_size.pixels();
        if px <= 0.0 { 1.0 } else { px / 16.0 }
    }

    pub(crate) fn sidebar_target(&self) -> f32 {
        if self.settings.sidebar_collapsed {
            0.0
        } else {
            self.settings.sidebar_width * self.ui_scale()
        }
    }

    /// Does the selected space's folder have git? Owner-stamped and synced —
    /// gates the Changes pane, its toggle, and Cmd-B with zero RPCs.
    pub(crate) fn space_git_detected(&self, cx: &App) -> bool {
        self.state.read(cx).selected_space_git()
    }

    /// The current chat's changes-pane flag (per-session, in-memory), gated on
    /// the space having git at all: a stale per-chat open flag must not reopen
    /// the pane after switching into a non-git space.
    /// The per-session panel key. The new-chat canvas (no selection) keys per
    /// SPACE — one shared "" key made a canvas toggle read as global state
    /// (user report).
    pub(crate) fn panel_key(&self, cx: &App) -> String {
        if self.active_chat.is_empty() {
            let space = self
                .state
                .read(cx)
                .selected_space
                .clone()
                .unwrap_or_default();
            format!("space-canvas:{space}")
        } else {
            self.active_chat.clone()
        }
    }

    /// Whether the right pane shows. NOT gated on git any more: the pane is
    /// a surface HOST now (terminals work in any space), so only the Git
    /// surface rows check `space_git_detected`. Still hidden on the
    /// new-session canvas, where the titlebar carries no toggle to close it
    /// again (an earlier user request).
    pub(crate) fn right_pane_open(&self, cx: &App) -> bool {
        !self.active_chat.is_empty() && self.panels.get(&self.panel_key(cx)).changes_open
    }

    /// The current chat's terminal flag (per-session, in-memory).
    pub(crate) fn terminal_open(&self, cx: &App) -> bool {
        self.panels.get(&self.panel_key(cx)).terminal_open
    }

    pub(crate) fn right_target(&self, cx: &App) -> f32 {
        if !self.right_pane_open(cx) {
            0.0
        } else {
            // Manual sizing preserves a usable conversation column. Takeover
            // intentionally consumes it completely. Both ride the sidebar
            // tween so toggling it remains seamless.
            let sidebar_now = self.sidebar_now();
            if self.right_pane_expanded {
                right_pane_takeover_width(self.viewport_width, sidebar_now)
            } else {
                let scale = self.ui_scale();
                (self.settings.right_pane_width * scale).min(right_pane_max_width_scaled(
                    self.viewport_width,
                    sidebar_now,
                    scale,
                ))
            }
        }
    }

    pub(crate) fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        let from = self.sidebar_now();
        self.sidebar_edge_bounce = None;
        self.sidebar_resize_edge = None;
        self.pane_resize_active = None;
        self.pane_resize_dragging = None;
        self.settings.sidebar_collapsed = !self.settings.sidebar_collapsed;
        self.sidebar_tween = Some(WidthTween::new(from, self.sidebar_target()));
        self.schedule_save(cx);
        cx.notify();
    }

    pub(crate) fn toggle_right_pane(&mut self, cx: &mut Context<Self>) {
        // Reverse from the visible width when toggled during an animation.
        let from = self.eval_tween(self.right_tween, self.right_target(cx));
        self.right_edge_bounce = None;
        self.right_resize_edge = None;
        self.finish_pane_resize(PaneResizeKind::Right);
        let sidebar_now = self.sidebar_now();
        let from_main = conversation_width(self.viewport_width, sidebar_now, from);
        let was_expanded = self.right_pane_expanded;
        let key = self.panel_key(cx);
        let open = self.panels.toggle_changes(&key);
        if !open {
            self.suspend_file_images(cx);
            // Closing always leaves takeover mode — reopening at full bleed
            // with the conversation gone read as a broken chat.
            self.right_pane_expanded = false;
        }
        let to = self.right_target(cx);
        self.right_tween = Some(WidthTween::new(from, to));
        self.right_takeover_content_tween = None;
        self.main_takeover_tween = was_expanded.then(|| {
            WidthTween::new(
                from_main,
                conversation_width(self.viewport_width, sidebar_now, to),
            )
        });
        if open
            && let RightSurface::Diff(id) = self.resolved_right_active(cx)
            && let Some(changes) = self.diffs.get(&id).cloned()
        {
            // Reopening onto a diff tab revalidates its watch.
            changes.update(cx, |changes, cx| changes.ensure_content(cx));
        }
        cx.notify();
    }

    pub(crate) fn right_terminal_panel(&mut self, cx: &mut Context<Self>) -> Entity<TerminalPanel> {
        if let Some(terminal) = &self.right_terminal {
            return terminal.clone();
        }
        let terminal = cx.new(|cx| TerminalPanel::new_embedded(self.state.clone(), cx));
        self.right_terminal = Some(terminal.clone());
        terminal
    }

    /// The surface that actually renders: the stored pick when it still
    /// exists, else the first remaining tab, else the picker. Terminal keys
    /// go stale when their tab closes/exits — never render a dead surface.
    pub(crate) fn resolved_right_active(&self, cx: &App) -> RightSurface {
        let picked = self.panels.get(&self.panel_key(cx)).right_active;
        let rows = self.right_surface_rows(cx);
        let exists = match picked {
            RightSurface::Picker => false,
            surface => rows.iter().any(|(s, _, _, _)| *s == surface),
        };
        if exists {
            picked
        } else {
            rows.first()
                .map(|(s, _, _, _)| *s)
                .unwrap_or(RightSurface::Picker)
        }
    }

    pub(crate) fn suspend_file_images(&mut self, cx: &mut Context<Self>) {
        for files in self.files.values().chain(self.file_surfaces.values()) {
            files.update(cx, |files, cx| files.suspend_images(cx));
        }
    }

    pub(crate) fn set_right_active(&mut self, surface: RightSurface, cx: &mut Context<Self>) {
        // Cover transition: switching from a file tab to the raw workspace
        // keeps the file surface mounted while its overlay tree grows to
        // full pane width — the file content stays visible until the tree
        // covers it, and only THEN does the surface actually swap. The
        // pending switch aborts if the user navigates elsewhere meanwhile.
        if let RightSurface::Files = surface
            && let RightSurface::File(prev_id) = self.resolved_right_active(cx)
            && let Some(prev) = self.file_surfaces.get(&prev_id).cloned()
        {
            let key = self.panel_key(cx);
            let pane_width = self.right_target(cx);
            // A collapsed sidebar starts the cover from the corner (0):
            // the tree grows out of its hidden state, not from the resting
            // width it no longer shows.
            let from = self
                .workspace_trees
                .get(&key)
                .map(|tree| {
                    tree.read_with(cx, |tree, _| {
                        if tree.sidebar_collapsed() {
                            0.0
                        } else {
                            tree.sidebar_width()
                        }
                    })
                })
                .filter(|width| *width < pane_width)
                .unwrap_or(pane_width);
            prev.update(cx, |prev, cx| {
                prev.begin_cover_expand(from, pane_width, cx);
            });
            let switch_key = key;
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(SURFACE_COVER_MS))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    if this.resolved_right_active(cx) == RightSurface::File(prev_id) {
                        this.panels.update(&switch_key, |p| p.right_active = surface);
                        cx.notify();
                    }
                });
            })
            .detach();
            return;
        }

        if self.resolved_right_active(cx) != surface {
            self.suspend_file_images(cx);
        }
        let key = self.panel_key(cx);
        // Capture what was active BEFORE the switch (for transition seeds).
        let previous = self.resolved_right_active(cx);
        self.panels.update(&key, |p| p.right_active = surface);
        match surface {
            RightSurface::Files => {
                if let Some(files) = self.files.get(&key).cloned() {
                    files.update(cx, |files, cx| files.ensure_loaded(cx));
                }
            }
            RightSurface::File(id) => {
                if let Some(file) = self.file_surfaces.get(&id).cloned() {
                    // Coming from the raw workspace to an existing file tab:
                    // the tree overlay starts at full pane width and eases
                    // to the sidebar — no instant fallback snap. Any OTHER
                    // activation (another file tab, a tree click) clears a
                    // stale held cover width first.
                    if let RightSurface::Files = previous {
                        let pane_width = self.right_target(cx);
                        file.update(cx, |file, cx| {
                            file.begin_sidebar_reveal(pane_width, cx);
                        });
                    } else {
                        file.update(cx, |file, cx| {
                            file.end_cover_transition(cx);
                        });
                    }
                    file.update(cx, |file, cx| file.ensure_loaded(cx));
                }
            }
            RightSurface::Terminal(tab) => {
                let panel = self.right_terminal_panel(cx);
                self.composer
                    .update(cx, |composer, _| composer.focus_pending = false);
                panel.update(cx, |panel, cx| {
                    panel.select_tab_by_key(tab, cx);
                    panel.request_focus(cx);
                });
            }
            RightSurface::Diff(id) => {
                if let Some(changes) = self.diffs.get(&id).cloned() {
                    changes.update(cx, |changes, cx| changes.ensure_content(cx));
                }
            }
            // The tab's feed (watch or snapshot) runs from open to close —
            // activation needs no revalidation.
            RightSurface::Subagent(_) | RightSurface::Browser(_) => {}
            RightSurface::Picker => {}
        }
        cx.notify();
    }

    pub(crate) fn focus_right_file_editor(
        &mut self,
        surface: RightSurface,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let RightSurface::Browser(id) = surface {
            if let Some(browser) = self.browsers.get(&id).cloned() {
                browser.update(cx, |browser, cx| browser.focus_address(window, cx));
            }
            return;
        }
        let key = self.panel_key(cx);
        let files = match surface {
            RightSurface::Files => self.files.get(&key).cloned(),
            RightSurface::File(id) => self.file_surfaces.get(&id).cloned(),
            _ => None,
        };
        if let Some(files) = files {
            files.update(cx, |files, cx| {
                files.focus_editor(window, cx);
            });
        }
    }

    pub(crate) fn set_files_word_wrap(
        &mut self,
        word_wrap: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.settings.files_word_wrap = word_wrap;
        if let Some(page) = self.files_settings_page.clone() {
            page.update(cx, |page, cx| page.set_word_wrap(word_wrap, cx));
        }
        let surfaces = self
            .files
            .values()
            .chain(self.file_surfaces.values())
            .cloned()
            .collect::<Vec<_>>();
        for surface in surfaces {
            surface.update(cx, |surface, cx| {
                surface.set_word_wrap(word_wrap, window, cx)
            });
        }
        self.schedule_save(cx);
        cx.notify();
    }

    /// Push a new code size into every open file surface. Called by the
    /// Appearance settings page, which owns the control. The typography
    /// global is the canonical store and persists on its own; this only
    /// propagates the change to already-open surfaces.
    pub(crate) fn set_code_font_size(&mut self, code_font_size: f32, cx: &mut Context<Self>) {
        let surfaces = self
            .files
            .values()
            .chain(self.file_surfaces.values())
            .cloned()
            .collect::<Vec<_>>();
        for surface in surfaces {
            surface.update(cx, |surface, cx| {
                surface.set_editor_font_size(code_font_size, cx)
            });
        }
        cx.notify();
    }

    pub(crate) fn set_files_show_all(&mut self, show_all_files: bool, cx: &mut Context<Self>) {
        self.settings.files_show_all = show_all_files;
        if let Some(page) = self.files_settings_page.clone() {
            page.update(cx, |page, cx| page.set_show_all_files(show_all_files, cx));
        }
        for tree in self.workspace_trees.values() {
            tree.update(cx, |tree, cx| tree.set_show_all_files(show_all_files, cx));
        }
        let surfaces = self
            .files
            .values()
            .chain(self.file_surfaces.values())
            .cloned()
            .collect::<Vec<_>>();
        for surface in surfaces {
            surface.update(cx, |surface, cx| {
                surface.set_show_all_files(show_all_files, cx)
            });
        }
        self.schedule_save(cx);
        cx.notify();
    }

    pub(crate) fn on_sidebar_drag(
        &mut self,
        event: &gpui::DragMoveEvent<SidebarResize>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let x = f32::from(event.event.position.x);
        let scale = self.ui_scale();
        let sample = motion::resize_drag_sample(
            x,
            settings::SIDEBAR_MIN * scale,
            settings::SIDEBAR_MAX * scale,
            self.sidebar_resize_edge,
            self.reduced_motion,
        );
        self.settings.sidebar_width = sample.width / scale;
        self.settings.sidebar_collapsed = false;
        self.pane_resize_dragging = Some(PaneResizeKind::Sidebar);
        self.sidebar_tween = None; // live drag tracks the pointer directly
        if sample.starts_bounce {
            self.sidebar_edge_bounce = sample.edge.map(motion::ResizeEdgeBounce::new);
        } else if sample.edge.is_none() {
            self.sidebar_edge_bounce = None;
        }
        self.pane_resize_active = sample.edge.is_none().then_some(PaneResizeKind::Sidebar);
        self.sidebar_resize_edge = sample.edge;
        self.schedule_save(cx);
        cx.notify();
    }

    pub(crate) fn finish_pane_resize(&mut self, kind: PaneResizeKind) {
        if self.pane_resize_active == Some(kind) {
            self.pane_resize_active = None;
        }
        if self.pane_resize_dragging == Some(kind) {
            self.pane_resize_dragging = None;
        }
        match kind {
            PaneResizeKind::Sidebar => self.sidebar_resize_edge = None,
            PaneResizeKind::Terminal => self.terminal_drag_anchor = None,
            PaneResizeKind::Right => self.right_resize_edge = None,
        }
    }

    pub(crate) fn on_right_pane_drag(
        &mut self,
        event: &gpui::DragMoveEvent<RightPaneResize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let viewport = f32::from(window.viewport_size().width);
        let width = viewport - f32::from(event.event.position.x);
        let scale = self.ui_scale();
        let min_w = RIGHT_PANE_MIN * scale;
        // No arbitrary percentage ceiling, but retain the chat's usable floor
        // instead of allowing the conversation to collapse to zero.
        let max = right_pane_max_width_scaled(viewport, self.sidebar_target(), scale);
        let sample = if max >= min_w {
            motion::resize_drag_sample(
                width,
                min_w,
                max,
                self.right_resize_edge,
                self.reduced_motion,
            )
        } else {
            motion::ResizeDragSample {
                width: max,
                edge: None,
                starts_bounce: false,
            }
        };
        self.settings.right_pane_width = sample.width / scale;
        self.pane_resize_dragging = Some(PaneResizeKind::Right);
        if sample.starts_bounce {
            self.right_edge_bounce = sample.edge.map(motion::ResizeEdgeBounce::new);
        } else if sample.edge.is_none() {
            self.right_edge_bounce = None;
        }
        self.pane_resize_active = sample.edge.is_none().then_some(PaneResizeKind::Right);
        self.right_resize_edge = sample.edge;
        self.right_tween = None;
        self.right_takeover_content_tween = None;
        self.main_takeover_tween = None;
        self.schedule_save(cx);
        cx.notify();
    }

    /// Publish this view's working copy to the central settings store. The
    /// store owns the single debounce task and the only production writer.
    pub(crate) fn schedule_save(&mut self, cx: &mut Context<Self>) {
        self.settings.appearance = crate::appearance::mode(cx);
        self.settings.git_history_columns = crate::history::configured_columns(cx);
        self.settings.git_history_column_widths = crate::history::configured_column_widths(cx);
        self.settings.git_history_column_order = crate::history::configured_column_order(cx);
        self.settings.git_history_author_display = crate::history::configured_author_display(cx);
        self.settings.theme_selection = crate::appearance::themes(cx);
        self.settings.accent = crate::appearance::accent(cx);
        self.settings.surface = crate::appearance::surface(cx);
        self.sync_independent_settings(cx);
        settings::replace(self.settings.clone(), SavePolicy::Debounced, cx);
    }

    /// Controls outside the Shell mutate these choices directly. A geometry
    /// save must never publish the Shell's older values over those selections.
    /// The typography globals own the font choices but persist every change
    /// immediately, so the central store is an equally canonical read and
    /// keeps this block on a single source.
    pub(crate) fn sync_independent_settings(&mut self, cx: &App) {
        let current = settings::current(cx);
        self.settings.window_geometry = current.window_geometry;
        self.settings.new_thread_composer_background = current.new_thread_composer_background;
        self.settings.new_thread_background_effect = current.new_thread_background_effect;
        self.settings.open_web_links_in_zeron = current.open_web_links_in_zeron;
        self.settings.ui_font_family = current.ui_font_family;
        self.settings.ui_font_size = current.ui_font_size;
        self.settings.terminal_font_family = current.terminal_font_family;
        self.settings.terminal_font_size = current.terminal_font_size;
        self.settings.code_font_family = current.code_font_family;
        self.settings.code_font_size = current.code_font_size;
        self.settings.transcript_width = current.transcript_width;
    }

    pub(crate) fn tween_elapsed(&self, started: std::time::Instant) -> Duration {
        self.render_time
            .unwrap_or_else(std::time::Instant::now)
            .saturating_duration_since(started)
    }

    /// Evaluate a width tween at the frame time (see [`WidthTween`]).
    /// Mid-flight: eased 200ms lerp, and `motion_active` is flagged so render
    /// schedules the next animation frame. Finished, stale, absent, or under
    /// reduced motion: exactly `target`. Honors `ZERON_MOTION_SCALE`.
    pub(crate) fn eval_tween(&self, tween: Option<WidthTween>, target: f32) -> f32 {
        let Some(WidthTween { from, to, started }) = tween else {
            return target;
        };
        if self.reduced_motion {
            return target;
        }
        let total = RESIZE.total().mul_f32(motion::speed_scale());
        let raw = self.tween_elapsed(started).as_secs_f32() / total.as_secs_f32();
        if raw >= 1.0 {
            return target;
        }
        self.motion_active.set(true);
        motion::lerp(from, to, RESIZE.progress(raw))
    }

    pub(crate) fn eval_resize_edge_bounce(
        &self,
        bounce: Option<motion::ResizeEdgeBounce>,
        enabled: bool,
    ) -> f32 {
        let Some(bounce) = bounce else {
            return 0.0;
        };
        if self.reduced_motion || !enabled {
            return 0.0;
        }
        let total =
            Duration::from_millis(motion::RESIZE_EDGE_BOUNCE_MS).mul_f32(motion::speed_scale());
        let raw = self.tween_elapsed(bounce.started).as_secs_f32() / total.as_secs_f32();
        if raw >= 1.0 {
            return 0.0;
        }
        self.motion_active.set(true);
        motion::resize_bounce_offset(bounce.edge, raw)
    }

    pub(crate) fn sidebar_now(&self) -> f32 {
        self.eval_tween(self.sidebar_tween, self.sidebar_target())
            + self
                .eval_resize_edge_bounce(self.sidebar_edge_bounce, !self.settings.sidebar_collapsed)
    }

    pub(crate) fn right_now(&self, cx: &App) -> f32 {
        self.eval_tween(self.right_tween, self.right_target(cx))
            + self.eval_resize_edge_bounce(
                self.right_edge_bounce,
                self.right_pane_open(cx) && !self.right_pane_expanded,
            )
    }

    pub(crate) fn tween_active(&self, tween: Option<WidthTween>) -> bool {
        tween.is_some_and(|tween| {
            !self.reduced_motion
                && self.tween_elapsed(tween.started) < RESIZE.total().mul_f32(motion::speed_scale())
        })
    }

    pub(crate) fn active_tween_endpoints(&self, tween: Option<WidthTween>) -> Option<(f32, f32)> {
        tween
            .filter(|transition| {
                !self.reduced_motion
                    && self.tween_elapsed(transition.started)
                        < RESIZE.total().mul_f32(motion::speed_scale())
            })
            .map(|transition| (transition.from, transition.to))
    }

    /// Right-anchored variant for the changes pane. The outer width follows the
    /// existing shell tween, while descendants retain the larger endpoint's
    /// geometry for that 200ms transition. This mirrors the sidebar's stable
    /// inner/clipped outer behavior without changing the center column's
    /// upstream flex layout.
    pub(crate) fn right_pane_container(
        &self,
        tween: Option<WidthTween>,
        target: f32,
        edge_offset: f32,
        inner: AnyElement,
    ) -> AnyElement {
        let takeover_width = self
            .active_tween_endpoints(self.right_takeover_content_tween)
            .map(|_| self.eval_tween(self.right_takeover_content_tween, target));
        let content_width =
            right_panel_content_width(target, self.active_tween_endpoints(tween), takeover_width)
                + edge_offset;
        div()
            .h_full()
            .flex_none()
            .relative()
            .overflow_hidden()
            .w(px(self.eval_tween(tween, target) + edge_offset))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .h_full()
                    .w(px(content_width))
                    .child(inner),
            )
            .into_any_element()
    }

    pub(crate) fn render_sidebar(&mut self, _cx: &mut Context<Self>) -> AnyElement {
        // The sidebar is part of the resolved theme. A second fixed-Zeron
        // palette here made imported families look split in half and froze
        // activity/glyph personality independently of the selected variant.
        let inner = self.sidebar_pane.clone().cached(
            gpui::StyleRefinement::default()
                .w(px(self.settings.sidebar_width * self.ui_scale()))
                .h_full()
                .flex_none(),
        );
        // Transparent — the sidebar sits directly on the frost shell; the main
        // card's own border provides the separation. The content row spans the
        // full window height (the titlebar overlays it), so the column pads
        // itself below the chrome.
        div()
            .h_full()
            .flex_none()
            .overflow_hidden()
            .w(px(self.sidebar_now()))
            .child(
                div()
                    .h_full()
                    .pt(crate::typography::ui_rems(Theme::TITLEBAR_HEIGHT))
                    .child(inner),
            )
            .into_any_element()
    }

    pub(crate) fn resize_handle<T>(
        &self,
        id: &'static str,
        kind: PaneResizeKind,
        marker: fn() -> T,
        reset: fn(&mut Shell, &mut Context<Shell>),
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div>
    where
        T: 'static,
    {
        let theme = Theme::of(cx);
        let fade_key = format!("pane-resize-{id}");
        let hover_highlight = motion::hover_blend(
            &fade_key,
            theme.border_strong.opacity(0.0),
            theme.border_strong,
        );
        let active = self.pane_resize_active == Some(kind);
        let constrained = self.pane_resize_dragging == Some(kind) && !active;
        let highlight = if constrained {
            theme.border_strong.opacity(0.0)
        } else if active {
            theme.border_strong
        } else {
            hover_highlight
        };
        let clear = highlight.opacity(0.0);
        let release_key = fade_key.clone();
        let release_out_key = fade_key.clone();
        div()
            .id(id)
            .absolute()
            .top(crate::typography::ui_rems(PANE_RESIZE_HITBOX_TOP))
            .bottom_0()
            .w(px(PANE_RESIZE_HITBOX_HALF_WIDTH * 2.0))
            .flex_none()
            .occlude()
            .cursor_col_resize()
            .on_hover(motion::hover_listener(fade_key))
            // Codex-style seam feedback: the existing 1px panel border stays
            // visible at rest; hover adds a stronger center highlight that
            // fades back into that border toward both ends.
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(PANE_RESIZE_HITBOX_HALF_WIDTH))
                    .w(px(1.0))
                    .flex()
                    .flex_col()
                    .child(div().flex_1().bg(gpui::linear_gradient(
                        180.0,
                        gpui::linear_color_stop(clear, 0.0),
                        gpui::linear_color_stop(highlight, 1.0),
                    )))
                    .child(div().flex_1().bg(gpui::linear_gradient(
                        180.0,
                        gpui::linear_color_stop(highlight, 0.0),
                        gpui::linear_color_stop(clear, 1.0),
                    ))),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.pane_resize_dragging = Some(kind);
                    this.pane_resize_active = Some(kind);
                    cx.notify();
                }),
            )
            .on_drag(marker(), |_, _point: Point<gpui::Pixels>, _, cx| {
                cx.stop_propagation();
                cx.new(|_| DragGhost)
            })
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseUpEvent, window, cx| {
                    if event.click_count == 2 {
                        reset(this, cx);
                        this.schedule_save(cx);
                        cx.notify();
                    }
                    this.finish_pane_resize(kind);
                    motion::set_hover(&release_key, false, this.reduced_motion);
                    window.refresh();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(move |this, _, window, _| {
                    this.finish_pane_resize(kind);
                    motion::set_hover(&release_out_key, false, this.reduced_motion);
                    window.refresh();
                }),
            )
    }

    pub(super) fn render_ready_page(
        &mut self,
        window: &mut Window,
        theme_is_glass: bool,
        theme_bg: gpui::Hsla,
        cx: &mut Context<Self>,
    ) -> (AnyElement, AnyElement) {
        // MessageRail width gate: hide below 48rem of main-panel width.
        let viewport = f32::from(window.viewport_size().width);
        self.viewport_height = f32::from(window.viewport_size().height);
        // Stamped for `right_target` — the expanded changes panel
        // sizes itself to the viewport.
        self.viewport_width = viewport;
        let on_chat = matches!(self.route, Route::Chat);
        let right_target_width = if on_chat { self.right_now(cx) } else { 0.0 };
        let panel_handoff = self.composer_dock.borrow_mut().observe_pane(
            self.state.read(cx).selected_chat.is_some(),
            right_target_width,
            on_chat && !self.reduced_motion,
            self.render_time.unwrap_or_else(std::time::Instant::now),
        );
        if panel_handoff {
            self.motion_active.set(true);
        }
        let main_target_width =
            conversation_width(viewport, self.sidebar_target(), right_target_width);
        let main_transition = self.active_tween_endpoints(self.main_takeover_tween);
        let main_content_width = stable_panel_content_width(main_target_width, main_transition);
        let transcript_width = self.composer_dock.borrow_mut().transcript_width(
            main_content_width,
            self.state.read(cx).selected_chat.is_some(),
            panel_handoff,
        );
        let main_width = (transcript_width - 10.0).max(0.0);
        // Clearance excludes the terminal dock: the transcript
        // viewport ends at the dock's top (see the underlay in
        // `render_main`), so only the chrome above it overlaps.
        let term_h = self.eval_tween(self.terminal_tween, self.terminal_target(cx));
        let stack_h = (self.bottom_stack.get() - term_h).max(0.0);
        let expected_has_composer = {
            let state = self.state.read(cx);
            (!state.spaces.is_empty() || state.no_project) && state.selected_chat.is_some()
        };
        let bottom_stack_ready = bottom_stack_measurement_matches(
            self.bottom_stack_has_composer.get(),
            expected_has_composer,
        );
        self.transcript.update(cx, |t, cx| {
            t.set_rail_enabled(rail::rail_visible(main_width), cx);
            if bottom_stack_ready && expected_has_composer {
                t.set_bottom_clearance(stack_h, cx);
            }
        });

        let sidebar = self.render_sidebar(cx);
        let sidebar_handle = self.resize_handle(
            "sidebar-resize",
            PaneResizeKind::Sidebar,
            || SidebarResize,
            |shell, _| {
                shell.settings.sidebar_width = SIDEBAR_DEFAULT;
                shell.sidebar_edge_bounce = None;
            },
            cx,
        );
        let main = self.render_main(window, main_content_width, transcript_width, cx);
        // The Changes pane is chat-scoped chrome: the Settings route
        // never renders it (zeron __root.tsx `!isSettings && activeChat`
        // around the diff column) — the per-session open flags stay
        // intact for the return trip.
        let right_open = on_chat && self.right_pane_open(cx);
        // Takeover mode derives its width from the viewport, so a
        // manual drag handle would fight the expanded target.
        let right_handle = (right_open
            && !panel_handoff
            && !self.right_pane_expanded
            && !self.tween_active(self.right_tween))
        .then(|| {
            self.resize_handle(
                "right-pane-resize",
                PaneResizeKind::Right,
                || RightPaneResize,
                |shell, _| {
                    shell.settings.right_pane_width = RIGHT_PANE_DEFAULT;
                    shell.right_edge_bounce = None;
                },
                cx,
            )
            // A forgiving transparent hit target centered on the
            // seam; the panel's 1px border remains the visual divider.
            .left(px(-PANE_RESIZE_HITBOX_HALF_WIDTH))
        });
        let right: AnyElement = if on_chat {
            self.render_right_pane(window, cx)
        } else {
            Empty.into_any_element()
        };
        let overlays = self.render_overlays(window.viewport_size(), window, cx);
        // Copied out (not held) — `render_title_bar` needs `cx` mutable.
        let border_color = Theme::of(cx).border;
        // No inset cards (user request): the conversation column sits
        // flush and unbordered, the transcript directly on the frost
        // glass; the changes pane is a flush left-bordered glass panel
        // (built inside `render_right_pane`).
        let main = if main_transition.is_some() {
            div()
                .h_full()
                .w(px(main_content_width))
                .flex_none()
                .flex()
                .child(main)
                .into_any_element()
        } else {
            main
        };
        let card_bg = if cfg!(target_os = "windows") && theme_is_glass {
            Some(match Theme::of(cx).appearance {
                crate::theme::Appearance::Dark => theme_bg.opacity(0.35),
                crate::theme::Appearance::Light => theme_bg.opacity(0.85),
            })
        } else if !theme_is_glass {
            Some(theme_bg)
        } else {
            None
        };
        let mut card_div = div().flex_1().min_w_0().flex().flex_row().overflow_hidden();
        if let Some(bg) = card_bg {
            card_div = card_div.bg(bg);
        }
        let card: AnyElement = card_div.child(main).into_any_element();
        // The whole app page is one keyed `animate-in` entrance (zeron
        // App.tsx `<div key={phase} className="animate-in h-full">`):
        // arriving from the splash or any gate fades the page in; the
        // splash-out crossfades over it on boot.
        // The sidebar resize handle FLOATS over the sidebar/card seam
        // (zero layout width, same idiom as the changes-pane grabber)
        // so the sidebar's right gutter stays exactly as wide as its
        // left one — a 5px flex child here read as lopsided spacing.
        let sidebar_seam = div()
            .w(px(0.0))
            .h_full()
            .flex_none()
            .relative()
            .child(sidebar_handle.left(px(-PANE_RESIZE_HITBOX_HALF_WIDTH)));
        // Keep the right resize target outside the pane's
        // overflow-hidden width container. This mirrors the sidebar
        // seam and lets the target straddle both adjacent panes.
        // Paint it after the page so page input cannot occlude the
        // inner half. A deferred draw would capture all native input.
        let right_seam: AnyElement = if let Some(handle) = right_handle {
            div()
                .w(px(0.0))
                .h_full()
                .flex_none()
                .absolute()
                .left_0()
                .top_0()
                .child(handle)
                .into_any_element()
        } else {
            Empty.into_any_element()
        };
        let title_bar = self.render_title_bar(window.viewport_size().height, cx);
        // Sidebar tone: a slightly lighter column behind the sidebar,
        // spanning the FULL window height (under the traffic lights,
        // through the titlebar, down to the bottom edge). Its width
        // rides the same tween as the sidebar, so the tone melts away
        // with the collapse instead of vanishing in a frame.
        let sidebar_now = self.sidebar_now();
        // Hairline on its right edge — full height like the tone,
        // so the sidebar column reads as its own surface.
        // The tone carries the window's left corners when the CSD
        // window floats — with one caveat: a corner radius is
        // clamped to the element's own size, and the COLLAPSED
        // sidebar is a ~1px border sliver (the grab affordance).
        // macOS trims that hairline with the window server's native
        // corner clip; we reproduce the same trim by insetting the
        // sliver vertically to where the curve begins, so its tips
        // never float over the transparent corner cutouts.
        let window_corner = Self::window_corner_radius(window);
        let sidebar_tone = div()
            .absolute()
            .top_0()
            .bottom_0()
            .left_0()
            .w(px(sidebar_now))
            .when(window_corner > 0.0, |el| {
                if sidebar_now >= 2.0 * window_corner {
                    el.rounded_tl(px(window_corner))
                        .rounded_bl(px(window_corner))
                } else {
                    el.top(px(window_corner)).bottom(px(window_corner))
                }
            })
            .bg(crate::theme::wash(0.05))
            .border_r_1()
            .border_color(border_color);
        // The content row spans the FULL window height — the titlebar
        // overlays it (glass, no fill), so the transcript can scroll
        // under the header and fade out at its edge. Columns that
        // must NOT underlap (sidebar content, the changes panel,
        // settings) pad themselves down by the titlebar height.
        let page = div()
            .size_full()
            .relative()
            .child(
                div()
                    .size_full()
                    .flex()
                    .flex_row()
                    .child(sidebar)
                    .child(sidebar_seam)
                    .child(card)
                    .child(
                        div()
                            .h_full()
                            .flex_none()
                            .relative()
                            .child(right)
                            .child(right_seam),
                    ),
            )
            .child(div().absolute().top_0().left_0().right_0().child(title_bar))
            .child(self.render_titlebar_cluster(cx))
            .children(overlays);
        (
            sidebar_tone.into_any_element(),
            motion::fade_in("phase-app", page).into_any_element(),
        )
    }
}
