//! Shared metrics for sidebar surface toolbars and their controls.

use gpui::{Div, div, prelude::*};

use crate::{theme::Theme, typography::ui_rems};

pub(crate) const HEADER_HEIGHT: f32 = Theme::TITLEBAR_HEIGHT;
pub(crate) const CONTROL_SIZE: f32 = 28.0;
pub(crate) const CONTROL_RADIUS: f32 = 6.0;
pub(crate) const ICON_SIZE: f32 = 14.0;
pub(crate) const CONTROL_GAP: f32 = 4.0;
pub(crate) const EDGE_INSET: f32 = 8.0;

/// Shared field treatment for the file search and browser address controls.
pub(crate) fn input() -> Div {
    div()
        .h(ui_rems(CONTROL_SIZE))
        .min_w_0()
        .flex_1()
        .px(ui_rems(8.0))
        .rounded(ui_rems(CONTROL_RADIUS))
        .bg(crate::theme::ink(0.035))
        .flex()
        .items_center()
        .gap(ui_rems(6.0))
        .text_size(ui_rems(12.0))
}

pub(crate) fn toolbar(theme: &Theme) -> Div {
    div()
        .h(ui_rems(HEADER_HEIGHT))
        .w_full()
        .flex_none()
        .px(ui_rems(EDGE_INSET))
        .flex()
        .items_center()
        .gap(ui_rems(CONTROL_GAP))
        .border_t_1()
        .border_b_1()
        .border_color(theme.border)
        .bg(if theme.is_glass() {
            theme.surface.opacity(0.26)
        } else {
            theme.surface
        })
}
