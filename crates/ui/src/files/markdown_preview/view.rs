//! Markdown preview surface rendering.

use super::*;

impl Render for MarkdownPreview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let code_fences_generation = crate::settings::code_fences_generation(cx);
        if self.code_fences_generation != code_fences_generation {
            self.code_fences_generation = code_fences_generation;
            for runtime in self.code_fences.values() {
                runtime.scroll.set_offset(gpui::Point::default());
            }
            self.list.remeasure();
        }
        if self.needs_focus {
            self.needs_focus = false;
            window.focus(&self.focus, cx);
        }
        self.resize_media(window, cx);
        self.cache
            .borrow_mut()
            .retain_rows(&std::mem::take(&mut self.visible_rows));
        if self.diagram_style != crate::theme::style_generation() {
            self.load_diagrams(cx);
        }
        if self.media_dirty {
            self.media_dirty = false;
            self.image_snapshot = Rc::new(self.images.clone());
            self.diagram_snapshot = Rc::new(self.diagrams.clone());
        }
        let mut root = div()
            .id("markdown-file-preview")
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .py(px(16.0))
            .font_family(theme.font_sans.clone())
            .text_color(theme.text)
            .track_focus(&self.focus)
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|view, _, window, cx| window.focus(&view.focus, cx)),
            )
            .on_mouse_move(cx.listener(Self::selection_move))
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|view, _, _, cx| {
                    if view.owns_selection() {
                        crate::markdown::selection::end_active_drag();
                        view.selection_task = None;
                        view.selection_pointer = None;
                        cx.notify();
                    }
                }),
            )
            .on_key_down(cx.listener(|_, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "c"
                    && (event.keystroke.modifiers.platform || event.keystroke.modifiers.control)
                {
                    if let Some(text) = crate::markdown::selection::selected_text() {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                        cx.stop_propagation();
                    }
                }
            }))
            .child(render::selection_surface_reset(self.scope.clone()))
            .when(self.loading, |el| {
                el.child(
                    div()
                        .px(px(24.0))
                        .text_color(theme.text_muted)
                        .child("Loading preview…"),
                )
            })
            .when(self.truncated, |el| {
                el.child(
                    div()
                        .px(px(24.0))
                        .text_color(theme.warning_muted)
                        .child("Large file preview is truncated and read-only."),
                )
            })
            .child(
                list(self.list.clone(), cx.processor(Self::render_row))
                    .flex_1()
                    .min_h_0()
                    .with_sizing_behavior(ListSizingBehavior::Auto),
            );
        if let Some(preview) = &self.preview_image {
            let weak = cx.weak_entity();
            let display_size = self
                .zoom_source
                .as_ref()
                .map(|source| gpui::size(px(source.width), px(source.height)));
            root = root.child(crate::attachments::lightbox_with_size(
                window,
                preview,
                &self.preview_focus,
                display_size,
                move |window, cx| {
                    let _ = weak.update(cx, |view, cx| {
                        view.close_media_preview(cx);
                        window.focus(&view.focus, cx);
                        cx.notify();
                    });
                },
                cx,
            ));
        }
        root
    }
}
