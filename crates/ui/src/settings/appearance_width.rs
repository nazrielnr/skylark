//! Transcript-width slider behavior.

use super::*;

impl AppearancePage {
    fn queue_width(&mut self, width: f32, window: &mut Window, cx: &mut Context<Self>) {
        let width = crate::settings::normalize_transcript_width(width);
        if self
            .pending_width
            .unwrap_or_else(|| crate::settings::transcript_width(cx))
            == width
        {
            return;
        }
        self.pending_width = Some(width);
        // Coalesce pointer events into one layout/settings update per frame.
        if !self.width_frame_pending {
            self.width_frame_pending = true;
            let page = cx.weak_entity();
            window.on_next_frame(move |_, cx| {
                let _ = page.update(cx, |this, cx| {
                    this.width_frame_pending = false;
                    this.apply_pending_width(cx);
                });
            });
        }
        cx.notify();
    }

    pub(super) fn apply_pending_width(&mut self, cx: &mut Context<Self>) {
        if let Some(width) = self.pending_width.take() {
            crate::settings::set_transcript_width(width, cx);
            cx.notify();
        }
    }

    pub(super) fn drag_width(
        &mut self,
        x: gpui::Pixels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(bounds) = self.width_bounds.get() else {
            return;
        };
        let fraction =
            (f32::from(x - bounds.left()) - 7.0) / (f32::from(bounds.size.width) - 14.0).max(1.0);
        self.queue_width(
            crate::settings::TRANSCRIPT_WIDTH_MIN
                + fraction.clamp(0.0, 1.0)
                    * (crate::settings::TRANSCRIPT_WIDTH_MAX
                        - crate::settings::TRANSCRIPT_WIDTH_MIN),
            window,
            cx,
        );
    }

    pub(super) fn render_transcript_width(
        &self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use crate::settings::{
            TRANSCRIPT_WIDTH_DEFAULT, TRANSCRIPT_WIDTH_MAX, TRANSCRIPT_WIDTH_MIN,
            TRANSCRIPT_WIDTH_STEP,
        };
        let width = self
            .pending_width
            .unwrap_or_else(|| crate::settings::transcript_width(cx));
        let fraction =
            (width - TRANSCRIPT_WIDTH_MIN) / (TRANSCRIPT_WIDTH_MAX - TRANSCRIPT_WIDTH_MIN);
        let bounds = self.width_bounds.clone();
        let show_details = self.width_hovered
            || self.width_pressed
            || (self.width_keyboard_active && self.width_focus.is_focused(window));
        let slider = div()
            .id("transcript-width-slider")
            .track_focus(&self.width_focus)
            .relative()
            .w(px(240.0))
            .h(px(28.0))
            .cursor_pointer()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                    window.focus(&this.width_focus, cx);
                    this.width_pressed = true;
                    this.width_keyboard_active = false;
                    cx.notify();
                    cx.stop_propagation();
                    this.drag_width(event.position.x, window, cx);
                }),
            )
            .on_drag(TranscriptWidthDrag, |drag, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| drag.clone())
            })
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let width = this
                    .pending_width
                    .unwrap_or_else(|| crate::settings::transcript_width(cx));
                let next = match event.keystroke.key.as_str() {
                    "left" | "down" => width - TRANSCRIPT_WIDTH_STEP,
                    "right" | "up" => width + TRANSCRIPT_WIDTH_STEP,
                    "home" => TRANSCRIPT_WIDTH_MIN,
                    "end" => TRANSCRIPT_WIDTH_MAX,
                    _ => return,
                };
                this.width_keyboard_active = true;
                cx.notify();
                cx.stop_propagation();
                window.prevent_default();
                this.queue_width(next, window, cx);
            }))
            .child(
                gpui::canvas(move |rect, _, _| bounds.set(Some(rect)), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .left(px(7.0))
                    .right(px(7.0))
                    .top(px(12.0))
                    .h(px(4.0))
                    .rounded_full()
                    .bg(theme.border)
                    .child(
                        div()
                            .h_full()
                            .w(gpui::relative(fraction))
                            .rounded_full()
                            .bg(theme.accent),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(gpui::relative(fraction))
                            .ml(px(-7.0))
                            .top(px(-5.0))
                            .size(px(14.0))
                            .rounded_full()
                            .bg(theme.accent),
                    ),
            );
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(20.0))
            .child(
                div()
                    .flex_1()
                    .min_w(px(200.0))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(widgets::field_label(theme, "Conversation width"))
                    .child(
                        div()
                            .text_size(typography::ui_rems(12.0))
                            .line_height(px(18.0))
                            .text_color(theme.text_muted)
                            .child("Maximum width of messages. Adapts to smaller windows."),
                    ),
            )
            .child(
                div()
                    .id("transcript-width-control")
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        this.width_hovered = *hovered;
                        if *hovered {
                            this.width_keyboard_active = false;
                        }
                        cx.notify();
                    }))
                    // Details occupy the surrounding whitespace, so the
                    // slider keeps the same row rhythm as the font controls.
                    .my(px(-12.0))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .when(!show_details, |el| el.invisible())
                            .flex()
                            .justify_between()
                            .text_size(typography::ui_rems(12.0))
                            .line_height(px(16.0))
                            .child(format!("{width:.0} px"))
                            .child(
                                div()
                                    .id("reset-transcript-width")
                                    .cursor_pointer()
                                    .text_color(theme.text_muted)
                                    .hover(|style| style.text_color(theme.text))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.pending_width = None;
                                        crate::settings::set_transcript_width(
                                            TRANSCRIPT_WIDTH_DEFAULT,
                                            cx,
                                        );
                                        cx.notify();
                                    }))
                                    .child("Reset"),
                            ),
                    )
                    .child(slider)
                    .child(
                        div()
                            .when(!show_details, |el| el.invisible())
                            .flex()
                            .justify_between()
                            .text_size(typography::ui_rems(11.0))
                            .line_height(px(14.0))
                            .text_color(theme.text_muted)
                            .child("560 px")
                            .child("1,200 px"),
                    ),
            )
            .into_any_element()
    }
}
