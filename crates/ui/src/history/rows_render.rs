use super::*;

impl GitHistory {
    fn render_ref(
        reference: GitHistoryRef,
        row_index: usize,
        ref_index: usize,
        theme: &Theme,
    ) -> AnyElement {
        let color = ref_color(&reference, theme);
        let icon = ref_icon(reference.kind);
        let description = ref_description(&reference);
        div()
            .id(SharedString::from(format!(
                "history-ref-{row_index}-{ref_index}"
            )))
            .h(px(16.0))
            .max_w(px(112.0))
            .px(px(5.0))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(2.0))
            .rounded(px(4.0))
            .bg(color.opacity(0.07))
            .text_size(px(10.0))
            .text_color(color.opacity(0.9))
            .child(
                crate::icons::icon(icon)
                    .size(px(10.0))
                    .mt(px(1.0))
                    .text_color(color.opacity(0.78)),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .child(SharedString::from(reference.label)),
            )
            .tooltip(move |_, cx| {
                cx.new(|_| HistoryRefTooltip {
                    descriptions: vec![description.clone()],
                })
                .into()
            })
            .tooltip_show_delay(Duration::from_millis(350))
            .into_any_element()
    }

    fn render_ref_area(
        refs: Vec<GitHistoryRef>,
        row_index: usize,
        available_width: f32,
        theme: &Theme,
    ) -> AnyElement {
        let visible_count = visible_ref_count(&refs, available_width);
        let hidden_refs: Vec<_> = refs.iter().skip(visible_count).cloned().collect();
        let hidden_count = hidden_refs.len();
        let hidden_descriptions: Vec<_> = hidden_refs.iter().map(ref_description).collect();

        div()
            .max_w(px(available_width))
            .min_w_0()
            .overflow_hidden()
            .flex()
            .items_center()
            .gap(px(HISTORY_REF_GAP))
            .children(refs.into_iter().take(visible_count).enumerate().map(
                |(ref_index, reference)| Self::render_ref(reference, row_index, ref_index, theme),
            ))
            .when(hidden_count > 0, |element| {
                element.child(
                    div()
                        .id(("history-ref-overflow", row_index))
                        .flex_none()
                        .text_size(px(10.0))
                        .text_color(theme.text_faint)
                        .child(SharedString::from(format!("+{hidden_count}")))
                        .tooltip(move |_, cx| {
                            cx.new(|_| HistoryRefTooltip {
                                descriptions: hidden_descriptions.clone(),
                            })
                            .into()
                        })
                        .tooltip_show_delay(Duration::from_millis(350)),
                )
            })
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_author_cell(
        index: usize,
        width: f32,
        display: GitHistoryAuthorDisplay,
        name: SharedString,
        initial: SharedString,
        avatar_image: Option<Arc<Image>>,
        opacity: f32,
        theme: &Theme,
    ) -> AnyElement {
        let has_avatar = avatar_image.is_some();
        div()
            .w(px(width))
            .min_w(px(GitHistoryColumnWidths::AUTHOR_MIN))
            .h_full()
            .flex_shrink(1.0)
            .flex()
            .items_center()
            .opacity(opacity)
            .when(display == GitHistoryAuthorDisplay::Avatar, |author| {
                let tooltip_name = name.clone();
                author.justify_center().child(
                    div()
                        .id(("history-author-avatar", index))
                        .size(px(20.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .overflow_hidden()
                        .rounded_full()
                        .border_1()
                        .border_color(crate::theme::hairline(0.12))
                        .bg(crate::theme::wash(0.08))
                        .when_some(avatar_image, |avatar, image| {
                            avatar.child(
                                img(image)
                                    .size_full()
                                    .rounded_full()
                                    .object_fit(ObjectFit::Cover),
                            )
                        })
                        .when(!has_avatar, |avatar| {
                            avatar.child(
                                div()
                                    .w_full()
                                    .text_center()
                                    .font_family(theme.font_sans.clone())
                                    .text_size(px(9.0))
                                    .line_height(px(18.0))
                                    .relative()
                                    .top(px(0.5))
                                    .text_color(theme.text_faint)
                                    .child(initial),
                            )
                        })
                        .tooltip(move |_, cx| {
                            cx.new(|_| HistoryAuthorTooltip {
                                name: tooltip_name.clone(),
                            })
                            .into()
                        })
                        .tooltip_show_delay(Duration::from_millis(300)),
                )
            })
            .when(display == GitHistoryAuthorDisplay::Name, |author| {
                author
                    .pr(px(8.0))
                    .truncate()
                    .text_color(theme.text_muted)
                    .child(name)
            })
            .into_any_element()
    }

    fn render_date_cell(width: f32, authored_at: &str, opacity: f32, theme: &Theme) -> AnyElement {
        div()
            .w(px(width))
            .min_w(px(GitHistoryColumnWidths::DATE_MIN))
            .h_full()
            .flex_shrink(1.0)
            .flex()
            .items_center()
            .truncate()
            .pr(px(8.0))
            .text_size(px(10.5))
            .opacity(opacity)
            .text_color(theme.text_muted)
            .child(SharedString::from(format_date(authored_at)))
            .into_any_element()
    }

    fn render_sha_cell(
        index: usize,
        width: f32,
        sha: String,
        copied: bool,
        opacity: f32,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = if copied {
            "Copied".to_string()
        } else {
            sha.chars().take(7).collect()
        };
        div()
            .w(px(width))
            .min_w(px(GitHistoryColumnWidths::SHA_MIN))
            .h_full()
            .pr(px(6.0))
            .flex_shrink(1.0)
            .flex()
            .items_center()
            .opacity(opacity)
            .child(
                div()
                    .id(SharedString::from(format!("history-sha-{index}")))
                    .w_full()
                    .h(px(24.0))
                    .flex()
                    .items_center()
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .hover(|style| style.bg(crate::theme::ink(0.07)))
                    .font_family(theme.font_mono.clone())
                    .text_size(px(10.5))
                    .text_color(if copied {
                        theme.accent
                    } else {
                        theme.text_muted
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.copy_sha(sha.clone(), cx)
                    }))
                    .child(SharedString::from(label)),
            )
            .into_any_element()
    }

    pub(super) fn render_row(
        &mut self,
        index: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if index >= self.visible_commits.len() {
            let theme = Theme::of(cx).clone();
            let searching = self.search_active();
            let pending = if searching {
                self.search_loading
            } else {
                self.loading
            };
            let has_error = if searching {
                self.search_error.is_some()
            } else {
                self.error.is_some()
            };
            let label = if pending {
                "Loading…"
            } else if has_error {
                "Retry"
            } else {
                "Load more"
            };
            let button = div()
                .id("history-load-older")
                .h(crate::typography::ui_rems(28.0))
                .px(crate::typography::ui_rems(12.0))
                .flex()
                .items_center()
                .justify_center()
                .gap(crate::typography::ui_rems(6.0))
                .rounded(crate::typography::ui_rems(7.0))
                .border_1()
                .border_color(theme.border.opacity(0.85))
                .bg(theme.surface_raised.opacity(0.72))
                .text_size(crate::typography::ui_rems(13.0))
                .text_color(if pending {
                    theme.text_muted
                } else {
                    theme.text
                })
                .when(!pending, |element| {
                    element
                        .cursor_pointer()
                        .hover(|style| {
                            style
                                .bg(theme.element_hover)
                                .border_color(theme.border_strong.opacity(0.75))
                                .text_color(theme.text)
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.load_older(cx)))
                })
                .when(!pending, |element| {
                    element.child(
                        crate::icons::icon(if has_error {
                            crate::icons::REFRESH
                        } else {
                            crate::icons::ALT_ARROW_DOWN
                        })
                        .size(px(11.0))
                        .flex_none()
                        .text_color(theme.text_faint),
                    )
                })
                .child(SharedString::from(label));
            return div()
                .w_full()
                .h(px(48.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(button)
                .into_any_element();
        }
        let Some(commit) = self.visible_commits.get(index).cloned() else {
            return gpui::Empty.into_any_element();
        };
        let Some(graph_row) = self.graph.rows.get(index).cloned() else {
            return gpui::Empty.into_any_element();
        };
        let theme = Theme::of(cx).clone();
        let sha = commit.sha.clone();
        let open_commit = commit.clone();
        let copied = self.copied_sha.as_deref() == Some(sha.as_str());
        let commit_subject = if commit.subject.is_empty() {
            "(no subject)".to_string()
        } else {
            commit.subject
        };
        let commit_refs = commit.refs;
        let commit_theme = theme.clone();
        let columns = configured_columns(cx);
        let column_widths = configured_column_widths(cx);
        let column_order = configured_column_order(cx);
        let author_display = configured_author_display(cx);
        let author_name = history_author_name(&commit.author_name);
        let author_initial = history_author_initial(&author_name);
        let avatar_image = self
            .avatar_images
            .get(&commit.author_email.trim().to_ascii_lowercase())
            .cloned();
        let graph_focus = self.graph_focus(cx);
        let row_is_focused =
            graph_focus.is_some_and(|focus| focus.color_id == graph_row.node_color_id);
        let row_content_opacity = graph_focus
            .filter(|_| !row_is_focused)
            .map(|focus| 1.0 - (1.0 - HISTORY_ROW_UNFOCUSED_OPACITY) * focus.amount)
            .unwrap_or(1.0);
        let focused_row_wash = graph_focus
            .filter(|_| row_is_focused)
            .map(|focus| crate::theme::ink(0.018 * focus.amount));
        let row_hover_path = graph_row.node_color_id;
        let optional_cells = visible_history_columns(&column_order, columns)
            .into_iter()
            .map(|column| match column {
                GitHistoryColumn::Author => Self::render_author_cell(
                    index,
                    column_widths.author,
                    author_display,
                    author_name.clone(),
                    author_initial.clone(),
                    avatar_image.clone(),
                    row_content_opacity,
                    &theme,
                ),
                GitHistoryColumn::Date => Self::render_date_cell(
                    column_widths.date,
                    &commit.authored_at,
                    row_content_opacity,
                    &theme,
                ),
                GitHistoryColumn::Sha => Self::render_sha_cell(
                    index,
                    column_widths.sha,
                    sha.clone(),
                    copied,
                    row_content_opacity,
                    &theme,
                    cx,
                ),
            })
            .collect::<Vec<_>>();

        let row = div()
            .id(("history-row", index))
            .h(px(HISTORY_ROW_HEIGHT))
            .w_full()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .text_size(px(11.0))
            .cursor_pointer()
            .when_some(focused_row_wash, |element, wash| element.bg(wash))
            .hover(|style| style.bg(crate::theme::ink(0.025)))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.set_row_hover((*hovered).then_some(row_hover_path), cx);
            }))
            // A commit row click opens the commit as its own diff tab (the
            // host — the right pane's surface strip — listens; user request).
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(GitHistoryEvent::OpenCommit(open_commit.clone()));
            }))
            .child(self.graph_cell(index, graph_row, graph_focus, &theme, cx))
            .child(
                container_query(move |size, _, _| {
                    let refs_width = ref_area_width(f32::from(size.width));
                    div()
                        .size_full()
                        .flex()
                        .items_center()
                        .gap(px(HISTORY_REF_GAP))
                        .pr(px(8.0))
                        .opacity(row_content_opacity)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(12.0))
                                .text_color(commit_theme.text)
                                .child(SharedString::from(commit_subject.clone())),
                        )
                        .when(!commit_refs.is_empty(), |element| {
                            element.child(Self::render_ref_area(
                                commit_refs.clone(),
                                index,
                                refs_width,
                                &commit_theme,
                            ))
                        })
                })
                .flex_1()
                .min_w(px(HISTORY_COMMIT_SUBJECT_MIN_WIDTH))
                .overflow_hidden(),
            )
            .child(
                div()
                    .h_full()
                    .flex()
                    .flex_row()
                    .flex_shrink(1.0)
                    .children(optional_cells),
            );

        let transition = self
            .view_transition
            .as_ref()
            .and_then(|transition| transition.rows.get(index).copied());
        match transition {
            Some(HistoryRowTransition::Entering | HistoryRowTransition::Exiting) => {
                let entering = transition == Some(HistoryRowTransition::Entering);
                let epoch = self.view_epoch;
                let sha = commit.sha;
                div()
                    .w_full()
                    .flex_none()
                    .overflow_hidden()
                    .child(row)
                    .with_animation(
                        SharedString::from(format!(
                            "history-row-fold-{epoch}-{sha}-{}",
                            if entering { "in" } else { "out" }
                        )),
                        crate::motion::COLLAPSE.animation(),
                        move |element, progress| {
                            let amount = if entering { progress } else { 1.0 - progress };
                            element
                                .h(px(HISTORY_ROW_HEIGHT * amount))
                                .opacity(0.35 + 0.65 * amount)
                        },
                    )
                    .into_any_element()
            }
            _ => row.into_any_element(),
        }
    }
}
