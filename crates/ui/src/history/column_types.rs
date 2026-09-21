use super::*;

pub(super) struct HistoryColumnPreferences {
    pub(super) columns: GitHistoryColumns,
    pub(super) widths: GitHistoryColumnWidths,
    pub(super) order: GitHistoryColumnOrder,
    pub(super) author_display: GitHistoryAuthorDisplay,
}

impl gpui::Global for HistoryColumnPreferences {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HistoryDataColumn {
    Commit,
    Author,
    Date,
    Sha,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct HistoryColumnDragAnchor {
    pub(super) start_x: f32,
    pub(super) left: HistoryDataColumn,
    pub(super) right: HistoryDataColumn,
    pub(super) left_width: f32,
    pub(super) right_width: f32,
}

pub(super) struct HistoryColumnResize;

#[derive(Clone)]
pub(super) struct HistoryColumnDrag {
    pub(super) column: GitHistoryColumn,
    pub(super) label: SharedString,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct HistoryColumnDragState {
    pub(super) from: usize,
    pub(super) over: usize,
}

pub(super) struct HistoryResizeGhost;

pub(super) struct HistoryColumnGhost {
    pub(super) label: SharedString,
}

impl Render for HistoryResizeGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

impl Render for HistoryColumnGhost {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        div()
            .h(px(24.0))
            .min_w(px(64.0))
            .px(px(9.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(5.0))
            .border_1()
            .border_color(theme.border_strong)
            .bg(theme.surface_raised)
            .shadow_md()
            .text_size(px(10.5))
            .text_color(theme.text_muted)
            .opacity(0.9)
            .child(self.label.clone())
    }
}

pub fn init(
    columns: GitHistoryColumns,
    widths: GitHistoryColumnWidths,
    order: GitHistoryColumnOrder,
    author_display: GitHistoryAuthorDisplay,
    cx: &mut App,
) {
    cx.set_global(HistoryColumnPreferences {
        columns,
        widths,
        order,
        author_display,
    });
}

pub fn configured_columns(cx: &App) -> GitHistoryColumns {
    cx.global::<HistoryColumnPreferences>().columns
}

pub fn configured_column_widths(cx: &App) -> GitHistoryColumnWidths {
    cx.global::<HistoryColumnPreferences>().widths
}

pub fn configured_column_order(cx: &App) -> GitHistoryColumnOrder {
    cx.global::<HistoryColumnPreferences>().order.clone()
}

pub fn configured_author_display(cx: &App) -> GitHistoryAuthorDisplay {
    cx.global::<HistoryColumnPreferences>().author_display
}
