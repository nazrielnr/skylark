//! History column preferences, resizing, ordering, and menus.

use super::*;

impl GitHistory {
    pub(super) fn toggle_column(&mut self, column: GitHistoryColumn, cx: &mut Context<Self>) {
        let (columns, widths, order) = {
            let preferences = cx.global_mut::<HistoryColumnPreferences>();
            match column {
                GitHistoryColumn::Author => {
                    preferences.columns.author = !preferences.columns.author
                }
                GitHistoryColumn::Date => preferences.columns.date = !preferences.columns.date,
                GitHistoryColumn::Sha => preferences.columns.sha = !preferences.columns.sha,
            }
            (
                preferences.columns,
                preferences.widths,
                preferences.order.clone(),
            )
        };
        Self::persist_column_layout(columns, widths, &order, SavePolicy::Immediate, cx);
        cx.refresh_windows();
        cx.notify();
    }

    pub(super) fn reset_columns(&mut self, cx: &mut Context<Self>) {
        let columns = GitHistoryColumns::default();
        let widths = GitHistoryColumnWidths::default();
        let order = GitHistoryColumnOrder::default();
        {
            let preferences = cx.global_mut::<HistoryColumnPreferences>();
            preferences.columns = columns;
            preferences.widths = widths;
            preferences.order = order.clone();
        }
        Self::persist_column_layout(columns, widths, &order, SavePolicy::Immediate, cx);
        cx.refresh_windows();
        cx.notify();
    }

    pub(super) fn persist_column_layout(
        columns: GitHistoryColumns,
        widths: GitHistoryColumnWidths,
        order: &GitHistoryColumnOrder,
        policy: SavePolicy,
        cx: &mut Context<Self>,
    ) {
        let order = order.clone();
        settings::update(policy, cx, move |settings| {
            settings.git_history_columns = columns;
            settings.git_history_column_widths = widths;
            settings.git_history_column_order = order;
        });
    }

    pub(super) fn schedule_column_layout_save(&mut self, cx: &mut Context<Self>) {
        let (columns, widths, order) = {
            let preferences = cx.global::<HistoryColumnPreferences>();
            (
                preferences.columns,
                preferences.widths,
                preferences.order.clone(),
            )
        };
        Self::persist_column_layout(columns, widths, &order, SavePolicy::Debounced, cx);
    }

    pub(super) fn begin_column_resize(
        &mut self,
        left: HistoryDataColumn,
        right: HistoryDataColumn,
        start_x: f32,
        cx: &mut Context<Self>,
    ) {
        let widths = configured_column_widths(cx);
        self.column_drag_anchor = Some(HistoryColumnDragAnchor {
            start_x,
            left,
            right,
            left_width: history_column_width(left, widths),
            right_width: history_column_width(right, widths),
        });
    }

    pub(super) fn on_column_resize(
        &mut self,
        event: &gpui::DragMoveEvent<HistoryColumnResize>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(anchor) = self.column_drag_anchor else {
            return;
        };
        let requested_delta = f32::from(event.event.position.x) - anchor.start_x;
        // Commit owns the flexible remainder. Interior dividers instead
        // preserve their pair's total width, so no drag creates overflow.
        let widths =
            resized_history_column_widths(configured_column_widths(cx), anchor, requested_delta);

        cx.global_mut::<HistoryColumnPreferences>().widths = widths;
        self.schedule_column_layout_save(cx);
        cx.refresh_windows();
        cx.notify();
    }

    pub(super) fn reset_column_widths(&mut self, cx: &mut Context<Self>) {
        cx.global_mut::<HistoryColumnPreferences>().widths = GitHistoryColumnWidths::default();
        self.column_drag_anchor = None;
        self.schedule_column_layout_save(cx);
        cx.refresh_windows();
        cx.notify();
    }

    pub(super) fn column_resize_handle(
        &self,
        left: HistoryDataColumn,
        right: HistoryDataColumn,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let hover = theme.border_strong;
        div()
            .id(SharedString::from(format!(
                "history-resize-{left:?}-{right:?}"
            )))
            .absolute()
            .left(px(-3.0))
            .top_0()
            .bottom_0()
            .w(px(6.0))
            .cursor_col_resize()
            .hover(move |style| style.bg(hover.opacity(0.7)))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                    this.begin_column_resize(left, right, f32::from(event.position.x), cx);
                }),
            )
            .on_drag(
                HistoryColumnResize,
                |_, _point: gpui::Point<gpui::Pixels>, _, cx| {
                    cx.stop_propagation();
                    cx.new(|_| HistoryResizeGhost)
                },
            )
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseUpEvent, _, cx| {
                    if event.click_count == 2 {
                        this.reset_column_widths(cx);
                    } else {
                        this.column_drag_anchor = None;
                    }
                }),
            )
            .into_any_element()
    }

    pub(super) fn render_column_header_cell(
        &self,
        column: GitHistoryColumn,
        left: HistoryDataColumn,
        index: usize,
        drag: Option<HistoryColumnDragState>,
        widths: GitHistoryColumnWidths,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let data_column = history_data_column(column);
        let width = history_optional_width(column, widths);
        let (min_width, _) = history_column_limits(data_column);
        let label = history_column_label(column);
        let id = match column {
            GitHistoryColumn::Author => "history-author-header",
            GitHistoryColumn::Date => "history-date-header",
            GitHistoryColumn::Sha => "history-sha-header",
        };
        let resize = self.column_resize_handle(left, data_column, theme, cx);
        let indicator = drag
            .filter(|state| state.over == index && state.from != state.over)
            .map(|state| {
                let place_after = state.from < state.over;
                div()
                    .absolute()
                    .top(px(3.0))
                    .bottom(px(3.0))
                    .w(px(2.0))
                    .rounded_full()
                    .bg(theme.accent)
                    .when(place_after, |line| line.right_0())
                    .when(!place_after, |line| line.left_0())
            });
        let ghost_label: SharedString = label.into();
        div()
            .id(id)
            .relative()
            .w(px(width))
            .min_w(px(min_width))
            .h_full()
            .flex_shrink(1.0)
            .flex()
            .items_center()
            .cursor_pointer()
            .when(column == GitHistoryColumn::Author, |header| {
                header.justify_center().on_mouse_down(
                    gpui::MouseButton::Right,
                    cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                        window.prevent_default();
                        cx.stop_propagation();
                        this.open_author_menu(event.position, cx);
                    }),
                )
            })
            .on_drag(
                HistoryColumnDrag {
                    column,
                    label: ghost_label,
                },
                |payload, _point, _, cx| {
                    cx.stop_propagation();
                    cx.new(|_| HistoryColumnGhost {
                        label: payload.label.clone(),
                    })
                },
            )
            .children(indicator)
            .child(label)
            // Paint last so this narrow hitbox wins over the header drag.
            .child(resize)
            .into_any_element()
    }

    pub(super) fn on_column_drag_move(
        &mut self,
        event: &gpui::DragMoveEvent<HistoryColumnDrag>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let payload = event.drag(cx);
        let columns = configured_columns(cx);
        let order = configured_column_order(cx);
        let visible = visible_history_columns(&order, columns);
        let Some(from) = visible.iter().position(|column| *column == payload.column) else {
            return;
        };
        let relative_x = f32::from(event.event.position.x) - f32::from(event.bounds.left());
        let over = history_column_drop_index(
            relative_x,
            f32::from(event.bounds.size.width),
            &visible,
            configured_column_widths(cx),
        );
        if self
            .column_drag
            .is_some_and(|state| state.from == from && state.over == over)
        {
            return;
        }
        self.column_drag = Some(HistoryColumnDragState { from, over });
        cx.notify();
    }

    pub(super) fn commit_column_reorder(
        &mut self,
        dragged: GitHistoryColumn,
        cx: &mut Context<Self>,
    ) {
        let columns = configured_columns(cx);
        let current = configured_column_order(cx);
        let visible = visible_history_columns(&current, columns);
        let target = self
            .column_drag
            .and_then(|state| visible.get(state.over).copied())
            .unwrap_or(dragged);
        let reordered = reordered_history_columns(&current, dragged, target);
        self.column_drag = None;
        if reordered == current {
            cx.notify();
            return;
        }
        cx.global_mut::<HistoryColumnPreferences>().order = reordered;
        self.schedule_column_layout_save(cx);
        cx.refresh_windows();
        cx.notify();
    }

    pub(super) fn open_column_menu(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.close_author_menu(cx);
        self.column_menu.open(position);
        cx.notify();
    }

    pub(super) fn close_column_menu(&mut self, cx: &mut Context<Self>) {
        if self.column_menu.begin_close() {
            popover::reap_popup(cx, |history: &mut Self| &mut history.column_menu);
        }
    }

    pub(super) fn open_author_menu(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.close_column_menu(cx);
        self.author_menu.open(position);
        cx.notify();
    }

    pub(super) fn close_author_menu(&mut self, cx: &mut Context<Self>) {
        if self.author_menu.begin_close() {
            popover::reap_popup(cx, |history: &mut Self| &mut history.author_menu);
        }
    }

    pub(super) fn toggle_author_display(&mut self, cx: &mut Context<Self>) {
        let display = {
            let preferences = cx.global_mut::<HistoryColumnPreferences>();
            preferences.author_display = match preferences.author_display {
                GitHistoryAuthorDisplay::Avatar => GitHistoryAuthorDisplay::Name,
                GitHistoryAuthorDisplay::Name => GitHistoryAuthorDisplay::Avatar,
            };
            preferences.author_display
        };
        settings::update(SavePolicy::Immediate, cx, |settings| {
            settings.git_history_author_display = display;
        });
        if display == GitHistoryAuthorDisplay::Avatar {
            self.resolve_loaded_avatars(cx);
        }
        cx.refresh_windows();
        cx.notify();
    }

    pub(super) fn render_author_menu(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = &theme.for_popup();
        let show_name = configured_author_display(cx) == GitHistoryAuthorDisplay::Name;
        popover::popover_card(theme)
            .w(px(116.0))
            .p(px(popover::CARD_INSET))
            .rounded(px(9.0))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_author_menu(cx)))
            .child(
                popover::menu_row(theme, false, "history-author-display-name")
                    .id("history-author-display-name")
                    .gap(px(0.0))
                    .px(px(7.0))
                    .py(px(4.0))
                    .rounded(px(9.0 - popover::CARD_INSET))
                    .text_size(px(11.5))
                    .on_click(cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_author_display(cx);
                        this.close_author_menu(cx);
                    }))
                    .child(div().flex_1().child("Name"))
                    .child(div().w(px(12.0)).flex_none().flex().justify_end().when(
                        show_name,
                        |element| {
                            element.child(
                                crate::icons::icon(crate::icons::CHECK)
                                    .size(px(10.0))
                                    .text_color(theme.text_muted),
                            )
                        },
                    )),
            )
            .into_any_element()
    }

    pub(super) fn render_column_menu(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = &theme.for_popup();
        let columns = configured_columns(cx);
        let widths = configured_column_widths(cx);
        let order = configured_column_order(cx);
        let option =
            |label: &'static str,
             checked: bool,
             column: GitHistoryColumn,
             index: usize,
             cx: &mut Context<Self>| {
                popover::menu_row(
                    theme,
                    false,
                    SharedString::from(format!("history-column-option-{index}")),
                )
                .id(("history-column-option", index))
                .gap(px(0.0))
                .px(px(7.0))
                .py(px(4.0))
                .rounded(px(9.0 - popover::CARD_INSET))
                .text_size(px(11.5))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.toggle_column(column, cx);
                }))
                .child(div().flex_1().child(label))
                .child(div().w(px(12.0)).flex_none().flex().justify_end().when(
                    checked,
                    |element| {
                        element.child(
                            crate::icons::icon(crate::icons::CHECK)
                                .size(px(10.0))
                                .text_color(theme.text_muted),
                        )
                    },
                ))
            };
        let defaults = GitHistoryColumns::default();
        let can_reset = columns != defaults
            || widths != GitHistoryColumnWidths::default()
            || order != GitHistoryColumnOrder::default();

        popover::popover_card(theme)
            .w(px(132.0))
            .p(px(popover::CARD_INSET))
            .rounded(px(9.0))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_column_menu(cx)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(popover::MENU_GAP))
                    .child(option(
                        "Author",
                        columns.author,
                        GitHistoryColumn::Author,
                        0,
                        cx,
                    ))
                    .child(option("Date", columns.date, GitHistoryColumn::Date, 1, cx))
                    .child(option("SHA", columns.sha, GitHistoryColumn::Sha, 2, cx))
                    .when(can_reset, |menu| {
                        menu.child(
                            div()
                                .h(px(1.0))
                                .mx(px(5.0))
                                .my(px(2.0))
                                .bg(crate::theme::hairline(0.08)),
                        )
                        .child(
                            popover::menu_row(theme, false, "history-columns-reset")
                                .id("history-columns-reset")
                                .px(px(7.0))
                                .py(px(4.0))
                                .rounded(px(9.0 - popover::CARD_INSET))
                                .text_size(px(11.5))
                                .text_color(theme.text_muted)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.reset_columns(cx);
                                    this.close_column_menu(cx);
                                }))
                                .child("Reset"),
                        )
                    }),
            )
            .into_any_element()
    }
}
