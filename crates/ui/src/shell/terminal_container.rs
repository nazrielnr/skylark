//! Embedded bottom terminal container, resize handle, and toggle logic.

use std::time::Duration;

use super::*;
use gpui::{
    div, px, AnyElement, Context, Entity, IntoElement, MouseButton, MouseUpEvent, Point, Window,
};
use crate::terminal::panel::{TerminalPanel, clamp_terminal_height};

pub(crate) const TERMINAL_RESIZE_HITBOX_HEIGHT: f32 = 10.0;

/// Drag marker for the terminal-panel height handle.
pub(crate) struct TerminalResize;

impl Shell {
    pub(super) fn terminal_panel(&mut self, cx: &mut Context<Self>) -> Entity<TerminalPanel> {
        if let Some(terminal) = &self.terminal {
            return terminal.clone();
        }
        let terminal = cx.new(|cx| TerminalPanel::new(self.state.clone(), cx));
        self.terminal = Some(terminal.clone());
        terminal
    }

    pub(super) fn terminal_target(&self, cx: &App) -> f32 {
        if self.terminal_open(cx) {
            self.settings.terminal_height
        } else {
            0.0
        }
    }

    /// Cmd/Ctrl+J and the header button (feature-inventory §1.10). Height
    /// animates 200 ms; closing detaches (PTYs stay alive), opening restores.
    /// The flag is per chat (skylark `sessionPanels`).
    pub(super) fn toggle_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let from = self.terminal_target(cx);
        let key = self.panel_key(cx);
        let open = self.panels.toggle_terminal(&key);
        self.terminal_tween = Some(WidthTween::new(from, self.terminal_target(cx)));
        let panel = self.terminal_panel(cx);
        panel.update(cx, |panel, cx| panel.set_open(open, cx));
        if open {
            self.composer
                .update(cx, |composer, _| composer.focus_pending = false);
            panel.update(cx, |panel, cx| panel.request_focus(cx));
            // Opening lands keyboard focus IN the shell — typing goes straight
            // to the prompt, no click needed (skylark terminal-panel.tsx: the
            // visible+active effect calls `terminal.focus()` on every open).
            // The handle is focusable before the panel's first paint; once the
            // terminal body mounts with `track_focus` it receives the keys.
            window.focus(&panel.read(cx).focus_handle(), cx);
        } else {
            // Hiding the panel removes the (likely focused) terminal view;
            // with nothing focused, window key bindings stop dispatching, so
            // hand focus to the composer. (Cmd+J is a pure toggle — a second
            // press closes even while the terminal is focused, as in skylark's
            // `useHotkey(toggleShortcut, ... setOpenScoped(!open))`.)
            window.focus(&self.composer.focus_handle(cx), cx);
        }
        self.terminal_tween_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(RESIZE.total().mul_f32(motion::speed_scale()) + Duration::from_millis(30))
                .await;
            this.update(cx, |shell, cx| {
                shell.terminal_tween = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    pub(super) fn on_terminal_drag(
        &mut self,
        event: &gpui::DragMoveEvent<TerminalResize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((anchor_y, anchor_h)) = self.terminal_drag_anchor else {
            return;
        };
        let dy = anchor_y - f32::from(event.event.position.y);
        let viewport_h = f32::from(window.viewport_size().height);
        let requested = anchor_h + dy;
        let max = (viewport_h * TERMINAL_MAX_VH).max(TERMINAL_MIN_HEIGHT);
        self.settings.terminal_height = clamp_terminal_height(requested, viewport_h);
        self.pane_resize_dragging = Some(PaneResizeKind::Terminal);
        self.pane_resize_active = (requested > TERMINAL_MIN_HEIGHT && requested < max)
            .then_some(PaneResizeKind::Terminal);
        self.terminal_tween = None; // live drag tracks the pointer
        self.schedule_save(cx);
        cx.notify();
    }

    /// Terminal panel dock at the main-column bottom: a 5px height-drag handle
    /// over the panel, the whole container height-animated 200 ms on toggle.
    pub(super) fn render_terminal_container(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let target = self.terminal_target(cx);
        let tween = self.terminal_tween;
        if target <= 0.0 && tween.is_none() {
            return gpui::Empty.into_any_element();
        }
        // Defensive: an open flag needs its entity (and set_open) even if
        // toggle_terminal never created one.
        if self.terminal_open(cx) && self.terminal.is_none() {
            let panel = self.terminal_panel(cx);
            panel.update(cx, |panel, cx| panel.set_open(true, cx));
        }
        let Some(panel) = self.terminal.clone() else {
            return gpui::Empty.into_any_element();
        };
        // The dock spans the main column's bottom: when the sidebar is fully
        // closed the column IS the window's left edge (and likewise the right
        // edge when the right pane is closed) — the panel's fill then carries
        // the CSD window's bottom corners.
        let window_corner = Self::window_corner_radius(window) > 0.0;
        {
            let bl = window_corner && self.sidebar_now() < 0.5;
            let br = window_corner && !self.right_pane_open(cx);
            panel.update(cx, |panel, cx| panel.set_window_corners(bl, br, cx));
        }
        let border = Theme::of(cx).border;
        let handle_key = "pane-resize-terminal-resize";
        let handle_hover = motion::hover_blend(
            handle_key,
            Theme::of(cx).border_strong.opacity(0.0),
            Theme::of(cx).border_strong,
        );
        let terminal_active = self.pane_resize_active == Some(PaneResizeKind::Terminal);
        let terminal_constrained =
            self.pane_resize_dragging == Some(PaneResizeKind::Terminal) && !terminal_active;
        let handle_highlight = if terminal_constrained {
            Theme::of(cx).border_strong.opacity(0.0)
        } else if terminal_active {
            Theme::of(cx).border_strong
        } else {
            handle_hover
        };
        let height = self.settings.terminal_height;

        let handle = div()
            .id("terminal-resize")
            .h(px(TERMINAL_RESIZE_HITBOX_HEIGHT))
            .w_full()
            .flex_none()
            .cursor_row_resize()
            .on_hover(motion::hover_listener(handle_key))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(px(1.0))
                    .bg(handle_highlight),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                    this.terminal_drag_anchor =
                        Some((f32::from(event.position.y), this.settings.terminal_height));
                    this.pane_resize_dragging = Some(PaneResizeKind::Terminal);
                    this.pane_resize_active = Some(PaneResizeKind::Terminal);
                    cx.notify();
                }),
            )
            .on_drag(TerminalResize, |_, _point: Point<gpui::Pixels>, _, cx| {
                cx.stop_propagation();
                cx.new(|_| DragGhost)
            })
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, window, cx| {
                    if event.click_count == 2 {
                        this.settings.terminal_height = TERMINAL_DEFAULT_HEIGHT;
                        this.schedule_save(cx);
                        cx.notify();
                    }
                    this.finish_pane_resize(PaneResizeKind::Terminal);
                    motion::set_hover(handle_key, false, this.reduced_motion);
                    window.refresh();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, window, _| {
                    this.finish_pane_resize(PaneResizeKind::Terminal);
                    motion::set_hover(handle_key, false, this.reduced_motion);
                    window.refresh();
                }),
            );

        // Fixed-height inner clipped by the animated container: content never
        // reflows mid-transition (same trick as the side panes). The handle
        // FLOATS over the panel's top edge (painted after, so it wins hit
        // testing) instead of stacking above it — stacked, its hitbox would read as
        // dead air between the seam and the tab bar (user report).
        let inner = div()
            .h(px(height))
            .w_full()
            .relative()
            .flex()
            .flex_col()
            .child(div().flex_1().min_h_0().child(panel))
            .child(handle.absolute().top_0().left_0().right_0());

        div()
            .w_full()
            .flex_none()
            .overflow_hidden()
            .border_t_1()
            .border_color(border)
            .h(px(self.eval_tween(tween, target)))
            .child(inner)
            .into_any_element()
    }
}
