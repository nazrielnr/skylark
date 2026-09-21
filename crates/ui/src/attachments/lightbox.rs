//! Attachment lightbox services.

use super::*;

// Preview lightbox (attachment-ui.tsx AttachmentPreviewDialog)
// ---------------------------------------------------------------------------

/// A full-size preview target (staged strip or transcript thumbnail).
#[derive(Clone)]
pub struct PreviewImage {
    pub name: SharedString,
    pub image: Arc<Image>,
    pub(crate) viewer: crate::image_viewer::ImageView,
}

impl PreviewImage {
    pub fn new(name: impl Into<SharedString>, image: Arc<Image>) -> Self {
        Self {
            name: name.into(),
            image,
            viewer: Default::default(),
        }
    }
}

/// Shared image viewer over a dim scrim. Escape and a click close it;
/// dragging and zoom controls are consumed by the viewer.
pub fn lightbox(
    window: &mut gpui::Window,
    preview: &PreviewImage,
    focus: &gpui::FocusHandle,
    on_close: impl Fn(&mut gpui::Window, &mut gpui::App) + 'static,
    cx: &mut gpui::App,
) -> AnyElement {
    lightbox_with_size(window, preview, focus, None, on_close, cx)
}

/// Sanitized SVG variants retain their source's natural logical dimensions.
pub(crate) fn lightbox_with_size(
    window: &mut gpui::Window,
    preview: &PreviewImage,
    focus: &gpui::FocusHandle,
    natural_size: Option<Size<gpui::Pixels>>,
    on_close: impl Fn(&mut gpui::Window, &mut gpui::App) + 'static,
    cx: &mut gpui::App,
) -> AnyElement {
    let viewport = window.viewport_size();
    let max_h = px(f32::from(viewport.height) * 0.85);
    let max_w = px(f32::from(viewport.width) * 0.9);
    let natural_size = natural_size.or_else(|| {
        preview
            .image
            .clone()
            .use_render_image(window, cx)
            .map(|image| {
                let dimensions = image.size(0);
                gpui::size(
                    px(dimensions.width.0 as f32),
                    px(dimensions.height.0 as f32),
                )
            })
    });
    let content = match natural_size {
        Some(natural) => preview
            .viewer
            .render(preview.image.clone(), natural, None, window, cx),
        None => div()
            .text_color(ink(0.6))
            .child("Loading image…")
            .into_any_element(),
    };
    let on_close = std::rc::Rc::new(on_close);
    let close_on_key = on_close.clone();
    let press_state = preview.viewer.clone();
    let click_state = preview.viewer.clone();
    gpui::deferred(
        gpui::anchored()
            .position(gpui::point(px(0.0), px(0.0)))
            .child(
                div()
                    .id("attachment-lightbox")
                    .occlude()
                    .track_focus(focus)
                    .w(viewport.width)
                    .h(viewport.height)
                    .bg(crate::popover::scrim_alpha(0.7))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(12.0))
                    .on_key_down(move |event: &gpui::KeyDownEvent, window, cx| {
                        if event.keystroke.key == "escape" {
                            cx.stop_propagation();
                            close_on_key(window, cx);
                        }
                    })
                    .capture_any_mouse_down(move |event, _, _| {
                        if event.button == gpui::MouseButton::Left {
                            press_state.begin_click();
                        }
                    })
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        if !click_state.dragged() {
                            on_close(window, cx);
                        }
                    })
                    .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                    .child(div().w(max_w).h(max_h).child(content))
                    .child(
                        div()
                            .max_w(max_w)
                            .overflow_hidden()
                            .text_size(crate::typography::ui_rems(11.0))
                            .text_color(ink(0.45))
                            .child(preview.name.clone()),
                    ),
            ),
    )
    .priority(3)
    .into_any_element()
}
