//! Selection drag tracking, edge-zone detection, and auto-scroll stepping.

use std::time::Duration;

use gpui::{px, Context, MouseMoveEvent, MouseUpEvent, Window};
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use gpui::ClipboardItem;

use crate::markdown::render;

use super::stick_spring::{jump_visibility, selection_scroll_step, SELECTION_SCROLL_TICK_MS};
use super::Transcript;

impl Transcript {
    pub(crate) fn on_selection_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !event.dragging() || !crate::markdown::selection::is_dragging() {
            self.stop_selection_scroll();
            return;
        }
        self.selection_drag_position = Some(event.position);
        if render::update_drag_at(event.position) {
            cx.notify();
        }
        self.schedule_selection_scroll(cx);
    }

    pub(crate) fn on_selection_mouse_down(
        &mut self,
        event: &gpui::MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Text listeners claim the drag before it bubbles here. Stop following
        // immediately: a stream commit can otherwise virtualize the anchor
        // before the first mouse move. Keep the user bubble's long-press timer.
        if !crate::markdown::selection::is_dragging() {
            return;
        }
        self.selection_drag_position = Some(event.position);
        self.discard_pending_viewport();
        self.user_collapse_scroll = None;
        self.stop_automatic_scrolling();
        self.materialize_scroll_anchor();
        cx.notify();
    }

    pub(crate) fn on_selection_mouse_up(
        &mut self,
        _event: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let was_selecting = self.selection_drag_position.is_some();
        self.stop_selection_scroll();
        if let Some(_text) = crate::markdown::selection::end_active_drag() {
            // X11 middle-click paste parity, including the case where the
            // anchor row has virtualized away and cannot receive mouse-up.
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            cx.write_to_primary(ClipboardItem::new_string(_text));
        }
        if was_selecting {
            self.last_scroll_distance = self.distance_from_bottom();
            self.show_jump_button =
                jump_visibility(self.show_jump_button, self.last_scroll_distance);
            cx.notify();
        }
    }

    pub(crate) fn stop_selection_scroll(&mut self) {
        self.selection_drag_position = None;
        self.selection_scroll_task = None;
    }

    pub(crate) fn schedule_selection_scroll(&mut self, cx: &mut Context<Self>) {
        if self.selection_scroll_task.is_some() || !crate::markdown::selection::is_dragging() {
            return;
        }
        let Some(position) = self.selection_drag_position else {
            return;
        };
        if selection_scroll_step(self.list.viewport_bounds(), position) == 0.0 {
            return;
        }
        self.selection_scroll_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(SELECTION_SCROLL_TICK_MS))
                .await;
            let _ = this.update(cx, |transcript, cx| {
                transcript.selection_scroll_task = None;
                transcript.step_selection_scroll(cx);
            });
        }));
    }

    pub(crate) fn step_selection_scroll(&mut self, cx: &mut Context<Self>) {
        if !crate::markdown::selection::is_dragging() {
            self.stop_selection_scroll();
            return;
        }
        let Some(position) = self.selection_drag_position else {
            return;
        };
        let step = selection_scroll_step(self.list.viewport_bounds(), position);
        if step == 0.0 {
            return;
        }

        // Resolve against the registry painted after the previous step before
        // moving it again. This is what lets a stationary edge pointer consume
        // successive virtualized rows.
        render::update_drag_at(position);
        self.begin_scroll_navigation();
        self.list.scroll_by(px(step));
        self.last_scroll_distance = self.distance_from_bottom();
        self.show_jump_button = jump_visibility(self.show_jump_button, self.last_scroll_distance);
        cx.notify();
        self.schedule_selection_scroll(cx);
    }
}
