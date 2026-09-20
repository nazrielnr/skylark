//! History toolbar controls and search interaction.

use super::*;

impl GitHistorySearchControl {
    pub fn new(history: Entity<GitHistory>, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            ComposerInput::with_context("Search", "PaletteSearch", cx).with_text_metrics(11.0, 14.0)
        });
        let observe = cx.observe(&history, |this, history, cx| {
            let query = history.read(cx).search_query.clone();
            if this.input.read(cx).text() != query {
                this.input.update(cx, |input, cx| input.set_text(query, cx));
            }
            cx.notify();
        });
        let input_events = cx.subscribe(&input, |this: &mut Self, input, event, cx| {
            if matches!(event, ComposerInputEvent::Edited) {
                let query = input.read(cx).text().to_string();
                let query_is_empty = query.is_empty();
                this.history
                    .update(cx, |history, cx| history.set_search_query(query, cx));
                if query_is_empty {
                    this.schedule_idle_dismiss(cx);
                } else {
                    this.cancel_idle_dismiss();
                }
            }
        });
        Self {
            history,
            input,
            mode: GitHistorySearchMode::Collapsed,
            idle_dismiss_epoch: 0,
            transition_epoch: 0,
            idle_dismiss_task: None,
            transition_task: None,
            blur_subscription: None,
            _observe: observe,
            _input_events: input_events,
        }
    }

    fn cancel_idle_dismiss(&mut self) {
        self.idle_dismiss_epoch = self.idle_dismiss_epoch.wrapping_add(1);
        self.idle_dismiss_task = None;
    }

    fn schedule_idle_dismiss(&mut self, cx: &mut Context<Self>) {
        self.cancel_idle_dismiss();
        if self.mode != GitHistorySearchMode::Expanded || !self.input.read(cx).text().is_empty() {
            return;
        }
        let epoch = self.idle_dismiss_epoch;
        self.idle_dismiss_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(HISTORY_SEARCH_IDLE_DISMISS)
                .await;
            this.update(cx, |control, cx| {
                if control.idle_dismiss_epoch == epoch
                    && control.mode == GitHistorySearchMode::Expanded
                    && control.input.read(cx).text().is_empty()
                {
                    control.begin_collapse(cx);
                }
            })
            .ok();
        }));
    }

    fn begin_collapse(&mut self, cx: &mut Context<Self>) {
        if self.mode != GitHistorySearchMode::Expanded || !self.input.read(cx).text().is_empty() {
            return;
        }
        self.cancel_idle_dismiss();
        self.mode = GitHistorySearchMode::Collapsing;
        self.transition_epoch = self.transition_epoch.wrapping_add(1);
        let epoch = self.transition_epoch;
        let duration = crate::motion::RESIZE
            .total()
            .mul_f32(crate::motion::speed_scale());
        self.transition_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(duration).await;
            this.update(cx, |control, cx| {
                if control.transition_epoch == epoch
                    && control.mode == GitHistorySearchMode::Collapsing
                {
                    // Unmounting the input is intentional: GPUI otherwise keeps
                    // its focused caret alive after this compact control is idle.
                    control.mode = GitHistorySearchMode::Collapsed;
                    control.transition_task = None;
                    cx.notify();
                }
            })
            .ok();
        }));
        cx.notify();
    }

    fn expand(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_idle_dismiss();
        self.transition_epoch = self.transition_epoch.wrapping_add(1);
        self.transition_task = None;
        self.mode = GitHistorySearchMode::Expanded;
        // The collapsed render does not mount `input`. Focusing its handle in
        // this click cycle leaves the next focus path empty, so Shell's
        // focus-lost fallback legitimately restores the composer. Wait until
        // the expanded state has driven a frame, then complete the handoff.
        let control = cx.entity().downgrade();
        window.on_next_frame(move |window, cx| {
            control
                .update(cx, |control, cx| {
                    if control.mode != GitHistorySearchMode::Expanded {
                        return;
                    }
                    let focus = control.input.read(cx).focus_handle(cx);
                    window.focus(&focus, cx);
                    control.schedule_idle_dismiss(cx);
                    cx.notify();
                })
                .ok();
        });
        cx.notify();
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        if !self.input.read(cx).text().is_empty() {
            self.input.update(cx, |input, cx| input.set_text("", cx));
        }
        self.schedule_idle_dismiss(cx);
        cx.notify();
    }
}

impl GitHistoryFetchButton {
    pub fn new(history: Entity<GitHistory>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&history, |_, _, cx| cx.notify());
        Self {
            history,
            _observe: observe,
        }
    }
}

impl GitHistoryViewButton {
    pub fn new(history: Entity<GitHistory>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&history, |_, _, cx| cx.notify());
        Self {
            history,
            _observe: observe,
        }
    }
}

impl Render for GitHistoryFetchButton {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let fetching = self.history.read(cx).fetching_all;
        let history = self.history.clone();
        div()
            .id("history-fetch-all")
            .h(px(crate::surface_chrome::CONTROL_SIZE))
            .px(px(8.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(6.0))
            .rounded(px(crate::surface_chrome::CONTROL_RADIUS))
            .bg(if fetching {
                crate::theme::wash(0.05)
            } else {
                crate::motion::hover_blend(
                    "history-fetch-all",
                    crate::theme::wash(0.0),
                    crate::theme::wash(0.14),
                )
            })
            .occlude()
            .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
                window.prevent_default()
            })
            .when(!fetching, |element| {
                element
                    .cursor_pointer()
                    .on_hover(crate::motion::hover_listener("history-fetch-all"))
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        history.update(cx, |history, cx| history.fetch_all(cx));
                    })
            })
            .child(if fetching {
                crate::loaders::mini_glyph_spinner(
                    "history-fetch-all-spinner",
                    1.75,
                    theme.glyph,
                    cx.entity_id(),
                    cx,
                )
                .into_any_element()
            } else {
                crate::icons::icon(crate::icons::CLOUD)
                    .size(px(crate::surface_chrome::ICON_SIZE))
                    .text_color(theme.text_muted.opacity(0.75))
                    .into_any_element()
            })
            .child(
                div()
                    .whitespace_nowrap()
                    .text_size(px(11.0))
                    .text_color(if fetching {
                        theme.text_faint
                    } else {
                        theme.text_muted
                    })
                    .child(if fetching { "Fetching…" } else { "Fetch all" }),
            )
    }
}

impl Render for GitHistoryViewButton {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let showing_tips = self.history.read(cx).view_mode == GitHistoryViewMode::BranchTips;
        let history = self.history.clone();
        let tooltip = if showing_tips {
            "Show all commits"
        } else {
            "Show branch tips"
        };

        div()
            .id("history-view-trigger")
            .size(px(crate::surface_chrome::CONTROL_SIZE))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(crate::surface_chrome::CONTROL_RADIUS))
            .cursor_pointer()
            .bg(if showing_tips {
                theme.accent.opacity(0.12)
            } else {
                crate::motion::hover_blend(
                    "history-view-trigger",
                    crate::theme::wash(0.0),
                    crate::theme::wash(0.14),
                )
            })
            .on_hover(crate::motion::hover_listener("history-view-trigger"))
            .occlude()
            .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
                window.prevent_default()
            })
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                history.update(cx, |history, cx| {
                    let mode = if history.view_mode == GitHistoryViewMode::BranchTips {
                        GitHistoryViewMode::AllCommits
                    } else {
                        GitHistoryViewMode::BranchTips
                    };
                    history.set_view_mode(mode, cx);
                });
            })
            .child(
                crate::icons::icon(crate::icons::FOLD_VERTICAL)
                    .size(px(crate::surface_chrome::ICON_SIZE))
                    .text_color(if showing_tips {
                        theme.accent
                    } else {
                        theme.text_muted
                    }),
            )
            .tooltip(move |_, cx| {
                cx.new(|_| HistoryRefTooltip {
                    descriptions: vec![tooltip.into()],
                })
                .into()
            })
            .tooltip_show_delay(Duration::from_millis(350))
    }
}

impl Render for GitHistorySearchControl {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        if self.mode == GitHistorySearchMode::Collapsed {
            let control = cx.entity().downgrade();
            return div()
                .id("history-search-trigger")
                .size(px(crate::surface_chrome::CONTROL_SIZE))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(crate::surface_chrome::CONTROL_RADIUS))
                .cursor_pointer()
                .bg(crate::motion::hover_blend(
                    "history-search-trigger",
                    crate::theme::wash(0.0),
                    crate::theme::wash(0.14),
                ))
                .on_hover(crate::motion::hover_listener("history-search-trigger"))
                .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
                    window.prevent_default()
                })
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    control
                        .update(cx, |control, cx| control.expand(window, cx))
                        .ok();
                })
                .child(
                    crate::icons::icon(crate::icons::MAGNIFER)
                        .size(px(crate::surface_chrome::ICON_SIZE))
                        .text_color(theme.text_muted),
                )
                .tooltip(|_, cx| {
                    cx.new(|_| HistoryRefTooltip {
                        descriptions: vec!["Search commits".into()],
                    })
                    .into()
                })
                .tooltip_show_delay(Duration::from_millis(350))
                .into_any_element();
        }
        if self.blur_subscription.is_none() {
            let focus = self.input.read(cx).focus_handle(cx);
            self.blur_subscription = Some(cx.on_blur(&focus, window, |control, _, cx| {
                control.clear(cx);
            }));
        }
        let control = cx.entity().downgrade();
        let search_loading = self.history.read(cx).search_loading;
        let closing = self.mode == GitHistorySearchMode::Collapsing;
        let transition_epoch = self.transition_epoch;
        let status_icon = if search_loading {
            crate::loaders::mini_glyph_spinner(
                "history-search-spinner",
                1.5,
                theme.glyph,
                cx.entity_id(),
                cx,
            )
            .into_any_element()
        } else {
            crate::icons::icon(crate::icons::MAGNIFER)
                .size(px(11.0))
                .flex_none()
                .text_color(theme.text_faint)
                .into_any_element()
        };
        div()
            .id("history-search-expanded")
            .h(px(crate::surface_chrome::CONTROL_SIZE))
            .w(px(HISTORY_SEARCH_WIDTH))
            .min_w(px(80.0))
            .flex_shrink(1.0)
            .overflow_hidden()
            .flex()
            .items_center()
            .gap(px(6.0))
            .pl(px(crate::surface_chrome::EDGE_INSET))
            .pr(px(2.0))
            .rounded(px(crate::surface_chrome::CONTROL_RADIUS))
            .bg(crate::theme::ink(0.035))
            .child(
                div()
                    .size(px(14.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(status_icon),
            )
            .child(
                div()
                    .h(px(14.0))
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .child(self.input.clone()),
            )
            .child(
                div()
                    .id("history-search-close")
                    .size(px(16.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(3.5))
                    .cursor_pointer()
                    .hover(|style| style.bg(crate::theme::ink(0.08)))
                    .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
                        window.prevent_default()
                    })
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        control.update(cx, |control, cx| control.clear(cx)).ok();
                    })
                    .child(
                        crate::icons::icon(crate::icons::CLOSE)
                            .size(px(9.0))
                            .text_color(theme.text_faint),
                    ),
            )
            .with_animation(
                SharedString::from(format!(
                    "history-search-morph-{transition_epoch}-{}",
                    if closing { "out" } else { "in" }
                )),
                crate::motion::RESIZE.animation(),
                move |element, progress| {
                    let amount = if closing { 1.0 - progress } else { progress };
                    element
                        .w(px(24.0 + (HISTORY_SEARCH_WIDTH - 24.0) * amount))
                        .opacity(0.45 + 0.55 * amount)
                },
            )
            .into_any_element()
    }
}

impl GitHistoryCount {
    pub fn new(history: Entity<GitHistory>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&history, |_, _, cx| cx.notify());
        Self {
            history,
            _observe: observe,
        }
    }
}

impl Render for GitHistoryCount {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let history = self.history.read(cx);
        let count = history.commit_count();
        let comparison = history
            .comparison
            .clone()
            .filter(|comparison| comparison.ahead > 0 || comparison.behind > 0);
        div()
            .h_full()
            .min_w_0()
            .flex_1()
            .flex()
            .items_center()
            .overflow_hidden()
            .child(
                container_query(move |size, _, _| {
                    let comparison_fits = f32::from(size.width) >= HISTORY_COMPARISON_MIN_WIDTH;
                    div()
                        .size_full()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .when_some(count, |element, count| {
                            element.child(
                                div()
                                    .flex_none()
                                    .whitespace_nowrap()
                                    .text_size(px(11.0))
                                    .line_height(px(14.0))
                                    .text_color(theme.text_muted)
                                    .child(SharedString::from(format!(
                                        "{count} commit{}",
                                        if count == 1 { "" } else { "s" }
                                    ))),
                            )
                        })
                        .when_some(
                            comparison_fits.then(|| comparison.clone()).flatten(),
                            |element, comparison| {
                                let base = comparison.base.clone();
                                let ahead = comparison.ahead;
                                let behind = comparison.behind;
                                element.child(
                                    div()
                                        .id("history-comparison")
                                        .relative()
                                        .top(px(1.0))
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .gap(px(4.0))
                                        .when(ahead > 0, |comparison| {
                                            comparison.child(
                                                div()
                                                    .whitespace_nowrap()
                                                    .text_size(px(10.5))
                                                    .line_height(px(13.0))
                                                    .text_color(theme.accent.opacity(0.88))
                                                    .child(SharedString::from(format!(
                                                        "{ahead} ahead"
                                                    ))),
                                            )
                                        })
                                        .when(ahead > 0 && behind > 0, |comparison| {
                                            comparison.child(
                                                div()
                                                    .text_size(px(10.0))
                                                    .text_color(theme.text_faint)
                                                    .child("·"),
                                            )
                                        })
                                        .when(behind > 0, |comparison| {
                                            comparison.child(
                                                div()
                                                    .whitespace_nowrap()
                                                    .text_size(px(10.5))
                                                    .line_height(px(13.0))
                                                    .text_color(theme.warning.opacity(0.82))
                                                    .child(SharedString::from(format!(
                                                        "{behind} behind"
                                                    ))),
                                            )
                                        })
                                        .tooltip(move |_, cx| {
                                            cx.new(|_| HistoryRefTooltip {
                                                descriptions: vec![
                                                    format!(
                                                        "Compared with {base}: {ahead} ahead, {behind} behind"
                                                    )
                                                    .into(),
                                                ],
                                            })
                                            .into()
                                        }),
                                )
                            },
                        )
                })
                .size_full(),
            )
    }
}
