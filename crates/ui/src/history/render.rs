//! History graph and commit-list rendering.

use super::*;
use gpui::prelude::*;

impl GitHistory {
    fn graph_paths(&self, theme: &Theme, focus: Option<GraphFocus>) -> AnyElement {
        let palette = [
            graph_color(theme.accent),
            graph_color(theme.busy),
            graph_color(theme.success),
            graph_color(theme.warning),
            graph_color(theme.danger),
            graph_color(theme.text_muted),
        ];
        let graph_bg = theme.bg;
        let rows = self.graph.rows.clone();
        let list = self.list.clone();
        // Branch tips are independent overview entries: their parents are
        // intentionally removed in `recompute_view`. Do not restore a false
        // relationship between adjacent tips just because the graph has
        // collapsed into its narrow rail.
        let show_compact_rail = self.view_mode == GitHistoryViewMode::AllCommits;
        let graph_geometry = self.graph_geometry.clone();
        let graph_hover_suppressed = self.graph_hover_suppressed.clone();
        canvas(
            |_, _, _| (),
            move |viewport_bounds, _, window, _| {
                let geometry = graph_geometry.get();
                let focus = if graph_hover_suppressed.get() {
                    None
                } else {
                    focus
                };
                let pass_count = if focus.is_some() { 2 } else { 1 };
                if geometry.compact && show_compact_rail {
                    let rail_x = geometry.lane_x(0);
                    for pass in 0..pass_count {
                        let selected_pass = focus.is_some() && pass == 1;
                        for (color_index, color) in palette.iter().enumerate() {
                            let stroke_width = focus
                                .filter(|_| selected_pass)
                                .map(|focus| {
                                    HISTORY_STROKE_WIDTH
                                        + (HISTORY_GRAPH_FOCUSED_STROKE_WIDTH
                                            - HISTORY_STROKE_WIDTH)
                                            * focus.amount
                                })
                                .unwrap_or(HISTORY_STROKE_WIDTH);
                            let mut builder = PathBuilder::stroke(px(stroke_width));
                            let mut has_segments = false;
                            for (index, row) in rows.iter().enumerate() {
                                if row.node_color_id % palette.len() != color_index
                                    || focus.is_some_and(|focus| {
                                        (row.node_color_id == focus.color_id) != selected_pass
                                    })
                                {
                                    continue;
                                }
                                let Some(row_bounds) = list.bounds_for_item(index) else {
                                    continue;
                                };
                                if row_bounds.bottom() < viewport_bounds.top()
                                    || row_bounds.top() > viewport_bounds.bottom()
                                {
                                    continue;
                                }
                                let row_height = f32::from(row_bounds.size.height);
                                if row_height <= 0.5 {
                                    continue;
                                }
                                let start = point(
                                    row_bounds.origin.x + px(rail_x),
                                    row_bounds.origin.y + px(row_height / 2.0),
                                );
                                let end_y = list
                                    .bounds_for_item(index + 1)
                                    .map(|next| {
                                        next.origin.y + px(f32::from(next.size.height) / 2.0)
                                    })
                                    .unwrap_or_else(|| row_bounds.bottom());
                                builder.move_to(start);
                                builder.line_to(point(row_bounds.origin.x + px(rail_x), end_y));
                                has_segments = true;
                            }
                            if has_segments && let Ok(path) = builder.build() {
                                let mut paint_color = *color;
                                if let Some(focus) = focus
                                    && !selected_pass
                                {
                                    // Keep the dimmed stroke opaque: overlapping row
                                    // segments otherwise stack alpha into tiny dark dots.
                                    paint_color = crate::motion::mix(
                                        paint_color,
                                        graph_bg,
                                        (1.0 - HISTORY_GRAPH_UNFOCUSED_OPACITY) * focus.amount,
                                    );
                                }
                                window.paint_path(path, paint_color);
                            }
                        }
                    }
                    return;
                }
                for pass in 0..pass_count {
                    let selected_pass = focus.is_some() && pass == 1;
                    for (color_index, color) in palette.iter().enumerate() {
                        let stroke_width = focus
                            .filter(|_| selected_pass)
                            .map(|focus| {
                                HISTORY_STROKE_WIDTH
                                    + (HISTORY_GRAPH_FOCUSED_STROKE_WIDTH - HISTORY_STROKE_WIDTH)
                                        * focus.amount
                            })
                            .unwrap_or(HISTORY_STROKE_WIDTH);
                        let mut builder = PathBuilder::stroke(px(stroke_width));
                        let mut has_segments = false;

                        for (index, row) in rows.iter().enumerate() {
                            let Some(row_bounds) = list.bounds_for_item(index) else {
                                continue;
                            };
                            if row_bounds.bottom() < viewport_bounds.top()
                                || row_bounds.top() > viewport_bounds.bottom()
                            {
                                continue;
                            }
                            let row_height = f32::from(row_bounds.size.height);
                            if row_height <= 0.5 {
                                continue;
                            }
                            let middle = row_height / 2.0;
                            let overlap = HISTORY_GRAPH_ROW_OVERLAP
                                * (row_height / HISTORY_ROW_HEIGHT).clamp(0.0, 1.0);
                            for segment in row.segments.iter().filter(|segment| {
                                segment.color_id % palette.len() == color_index
                                    && focus.is_none_or(|focus| {
                                        (segment.color_id == focus.color_id) == selected_pass
                                    })
                            }) {
                                has_segments = true;
                                let from_x = geometry.lane_x(segment.from_lane);
                                let to_x = geometry.lane_x(segment.to_lane);
                                let origin = row_bounds.origin;
                                match segment.shape {
                                    SegmentShape::Incoming => {
                                        builder.move_to(point(
                                            origin.x + px(from_x),
                                            origin.y - px(overlap),
                                        ));
                                        builder.cubic_bezier_to(
                                            point(origin.x + px(to_x), origin.y + px(middle)),
                                            point(
                                                origin.x + px(from_x),
                                                origin.y + px(middle * 0.55),
                                            ),
                                            point(
                                                origin.x + px(to_x),
                                                origin.y + px(middle * 0.55),
                                            ),
                                        );
                                    }
                                    SegmentShape::Outgoing => {
                                        builder.move_to(point(
                                            origin.x + px(from_x),
                                            origin.y + px(middle),
                                        ));
                                        builder.cubic_bezier_to(
                                            point(
                                                origin.x + px(to_x),
                                                origin.y + px(row_height + overlap),
                                            ),
                                            point(
                                                origin.x + px(from_x),
                                                origin.y + px(middle * 1.45),
                                            ),
                                            point(
                                                origin.x + px(to_x),
                                                origin.y + px(middle * 1.45),
                                            ),
                                        );
                                    }
                                    SegmentShape::Through => {
                                        builder.move_to(point(
                                            origin.x + px(from_x),
                                            origin.y - px(overlap),
                                        ));
                                        let end = point(
                                            origin.x + px(to_x),
                                            origin.y + px(row_height + overlap),
                                        );
                                        if segment.from_lane == segment.to_lane {
                                            builder.line_to(end);
                                        } else {
                                            builder.cubic_bezier_to(
                                                end,
                                                point(origin.x + px(from_x), origin.y + px(middle)),
                                                point(origin.x + px(to_x), origin.y + px(middle)),
                                            );
                                        }
                                    }
                                }
                            }
                        }

                        if has_segments && let Ok(path) = builder.build() {
                            let mut paint_color = *color;
                            if let Some(focus) = focus
                                && !selected_pass
                            {
                                // Keep the dimmed stroke opaque: overlapping row
                                // segments otherwise stack alpha into tiny dark dots.
                                paint_color = crate::motion::mix(
                                    paint_color,
                                    graph_bg,
                                    (1.0 - HISTORY_GRAPH_UNFOCUSED_OPACITY) * focus.amount,
                                );
                            }
                            window.paint_path(path, paint_color);
                        }
                    }
                }
            },
        )
        .absolute()
        .inset_0()
        .into_any_element()
    }

    fn graph_cell(
        &mut self,
        row_index: usize,
        row: GraphRow,
        focus: Option<GraphFocus>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let geometry = self.graph_geometry.get();
        let width = geometry.width;
        let palette = [
            graph_color(theme.accent),
            graph_color(theme.busy),
            graph_color(theme.success),
            graph_color(theme.warning),
            graph_color(theme.danger),
            graph_color(theme.text_muted),
        ];
        let mut color = palette[row.node_color_id % palette.len()];
        let selected = focus.is_some_and(|focus| focus.color_id == row.node_color_id);
        if let Some(focus) = focus
            && !selected
        {
            color.a *= 1.0 - (1.0 - HISTORY_GRAPH_UNFOCUSED_OPACITY) * focus.amount;
        }
        let node_radius = HISTORY_NODE_RADIUS
            + focus
                .filter(|_| selected)
                .map(|focus| focus.amount * 0.75)
                .unwrap_or_default();
        let node_x = geometry.lane_x(row.node_lane);
        let fold_reference = (self.view_mode == GitHistoryViewMode::AllCommits)
            .then(|| {
                self.visible_commits.get(row_index).and_then(|commit| {
                    commit
                        .refs
                        .iter()
                        .find(|reference| branch_ref_key(reference).is_some())
                        .cloned()
                })
            })
            .flatten();
        let fold_control = fold_reference.map(|reference| {
            let key = branch_ref_key(&reference).unwrap_or_default();
            let collapsed = self.collapsed_branches.contains(&key);
            let hidden_count = self.collapsed_counts.get(&key).copied().unwrap_or_default();
            let tooltip = if collapsed {
                format!(
                    "Expand {}{}",
                    reference.label,
                    if hidden_count == 0 {
                        String::new()
                    } else {
                        format!(" ({hidden_count} hidden)")
                    }
                )
            } else {
                format!("Collapse {}", reference.label)
            };
            let history = cx.entity();
            div()
                .id(("history-graph-fold", row_index))
                .absolute()
                .left(px(node_x + HISTORY_NODE_RADIUS + 3.0))
                .top(px((HISTORY_ROW_HEIGHT - 16.0) / 2.0))
                .size(px(16.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .border_1()
                .border_color(color.opacity(0.32))
                .bg(theme.bg.opacity(0.96))
                .cursor_pointer()
                .opacity(if collapsed { 1.0 } else { 0.0 })
                .group_hover("history-graph-tip", |style| style.opacity(1.0))
                .on_mouse_down(gpui::MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    history.update(cx, |history, cx| {
                        history.toggle_branch_ref(reference.clone(), cx)
                    });
                })
                .child(
                    crate::icons::icon(if collapsed {
                        crate::icons::EXPAND_ARROWS
                    } else {
                        crate::icons::FOLD_VERTICAL
                    })
                    .size(px(9.0))
                    .text_color(color.opacity(0.9)),
                )
                .tooltip(move |_, cx| {
                    cx.new(|_| HistoryRefTooltip {
                        descriptions: vec![tooltip.clone().into()],
                    })
                    .into()
                })
                .tooltip_show_delay(Duration::from_millis(250))
        });
        let node = div()
            .absolute()
            .left(px(node_x - node_radius))
            .top(px(HISTORY_ROW_HEIGHT / 2.0 - node_radius))
            .size(px(node_radius * 2.0))
            .rounded_full()
            .bg(color);
        div()
            .id(("history-graph-cell", row_index))
            .relative()
            .group("history-graph-tip")
            .w(px(width))
            .h(px(HISTORY_ROW_HEIGHT))
            .flex_none()
            .on_mouse_move(
                cx.listener(move |this, event: &gpui::MouseMoveEvent, _, cx| {
                    this.update_graph_hover(row_index, event.position, cx);
                }),
            )
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !*hovered {
                    this.set_graph_hover(None, cx);
                }
            }))
            .when(row.is_head, |element| {
                element.child(
                    div()
                        .absolute()
                        .left(px(node_x - node_radius - HISTORY_HEAD_RING_PADDING))
                        .top(px(HISTORY_ROW_HEIGHT / 2.0
                            - node_radius
                            - HISTORY_HEAD_RING_PADDING))
                        .size(px((node_radius + HISTORY_HEAD_RING_PADDING) * 2.0))
                        .rounded_full()
                        .border_1()
                        .border_color(color)
                        .bg(theme.bg),
                )
            })
            .child(node)
            .children(fold_control)
            .into_any_element()
    }

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

    fn render_row(
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
                .h(px(28.0))
                .px(px(11.0))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(6.0))
                .rounded(px(7.0))
                .border_1()
                .border_color(theme.border.opacity(0.85))
                .bg(theme.surface_raised.opacity(0.72))
                .text_size(px(11.0))
                .text_color(if pending {
                    theme.text_faint
                } else {
                    theme.text_muted
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

impl Render for GitHistory {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_loaded(cx);
        if !cx.has_active_drag() {
            self.column_drag = None;
        }
        let theme = Theme::of(cx).clone();
        let graph_focus = self.graph_focus(cx);
        let columns = configured_columns(cx);
        let column_widths = configured_column_widths(cx);
        let column_order = configured_column_order(cx);
        let visible_columns = visible_history_columns(&column_order, columns);
        let optional_columns_width = visible_columns
            .iter()
            .copied()
            .map(|column| history_optional_width(column, column_widths))
            .sum::<f32>();
        let mut previous = HistoryDataColumn::Commit;
        let mut header_cells = Vec::with_capacity(visible_columns.len());
        for (index, column) in visible_columns.iter().copied().enumerate() {
            header_cells.push(self.render_column_header_cell(
                column,
                previous,
                index,
                self.column_drag,
                column_widths,
                &theme,
                cx,
            ));
            previous = history_data_column(column);
        }
        let optional_headers = div()
            .id("history-optional-column-headers")
            .h_full()
            .flex()
            .flex_row()
            .flex_shrink(1.0)
            .on_drag_move::<HistoryColumnDrag>(cx.listener(Self::on_column_drag_move))
            .on_drop::<HistoryColumnDrag>(cx.listener(
                |this, payload: &HistoryColumnDrag, _, cx| {
                    this.commit_column_reorder(payload.column, cx);
                },
            ))
            .children(header_cells);
        let column_button = div()
            .id("history-columns-button")
            .absolute()
            .right(px(3.0))
            .top(px(2.0))
            .size(px(20.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(5.0))
            .cursor_pointer()
            .opacity(0.0)
            .group_hover("history-column-header", |style| style.opacity(1.0))
            .when(self.column_menu.is_open(), |button| button.opacity(1.0))
            .hover(|style| style.bg(crate::theme::ink(0.08)))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                    this.open_column_menu(event.position, cx);
                }),
            )
            .child(
                crate::icons::icon(crate::icons::CHECKLIST)
                    .size(px(12.0))
                    .text_color(theme.text_muted),
            );
        let column_menu_position = self.column_menu.get().copied();
        let column_menu = column_menu_position.map(|position| {
            let closing = self.column_menu.closing_since();
            let menu = self.render_column_menu(&theme, cx);
            (position, menu, closing)
        });
        let author_menu_position = self.author_menu.get().copied();
        let author_menu = author_menu_position.map(|position| {
            let closing = self.author_menu.closing_since();
            let menu = self.render_author_menu(&theme, cx);
            (position, menu, closing)
        });
        let graph_geometry = self.graph_geometry.clone();
        let graph_target_geometry = self.graph_target_geometry.clone();
        let graph_geometry_morph = self.graph_geometry_morph.clone();
        let graph_hover_suppressed = self.graph_hover_suppressed.clone();
        let graph_lane_capacity = self.graph_lane_capacity;
        let show_header = !self.visible_commits.is_empty();
        let header_theme = theme.clone();

        let body: AnyElement = if self.target_key.is_none() {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(12.0))
                .text_color(theme.text_faint)
                .child("No repository selected")
                .into_any_element()
        } else if self.loading && self.commits.is_empty() {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(8.0))
                .child(crate::loaders::gradient_spinner(
                    "history-loading",
                    &theme,
                    3.0,
                    cx.entity_id(),
                    cx,
                ))
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme.text_faint)
                        .child("Loading history…"),
                )
                .into_any_element()
        } else if self.visible_commits.is_empty() {
            let active_error = if self.search_active() {
                self.search_error.clone()
            } else {
                self.error.clone()
            };
            let message = active_error.clone().unwrap_or_else(|| {
                SharedString::from(if self.search_active() {
                    "No matching commits"
                } else if self.view_mode == GitHistoryViewMode::BranchTips {
                    "No branch tips found"
                } else {
                    "No commits found"
                })
            });
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .px(px(20.0))
                .text_size(px(12.0))
                .text_color(if active_error.is_some() {
                    theme.warning
                } else {
                    theme.text_faint
                })
                .child(message)
                .into_any_element()
        } else {
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .child(self.graph_paths(&theme, graph_focus))
                .child(
                    list(self.list.clone(), cx.processor(Self::render_row))
                        .size_full()
                        .with_sizing_behavior(gpui::ListSizingBehavior::Auto),
                )
                .into_any_element()
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .on_drag_move(cx.listener(Self::on_column_resize))
            .when_some(self.fetch_error.clone(), |element, error| {
                element.child(
                    div()
                        .h(px(28.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .px(px(8.0))
                        .border_b_1()
                        .border_color(theme.danger.opacity(0.16))
                        .bg(theme.danger.opacity(0.05))
                        .truncate()
                        .text_size(px(11.0))
                        .text_color(theme.danger_muted)
                        .child(SharedString::from(format!("Fetch failed: {error}"))),
                )
            })
            .when_some(
                if self.search_active() {
                    self.search_error.clone()
                } else {
                    self.error.clone()
                }
                .filter(|_| !self.visible_commits.is_empty()),
                |element, error| {
                    element.child(
                        div()
                            .h(px(28.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .px(px(8.0))
                            .border_b_1()
                            .border_color(theme.danger.opacity(0.16))
                            .bg(theme.danger.opacity(0.05))
                            .truncate()
                            .text_size(px(11.0))
                            .text_color(theme.danger_muted)
                            .child(error),
                    )
                },
            )
            .child(
                container_query(move |size, window, cx| {
                    let responsive_target = responsive_graph_geometry(
                        graph_lane_capacity,
                        f32::from(size.width),
                        optional_columns_width,
                    );
                    let previous_target = graph_target_geometry.get();
                    let compact = should_use_compact_graph(
                        responsive_target,
                        previous_target,
                        f32::from(size.width),
                        optional_columns_width,
                    );
                    let target = stabilized_graph_geometry(
                        responsive_target,
                        previous_target,
                        window.scale_factor(),
                        compact,
                    );
                    let active_morph = graph_geometry_morph.get().filter(|morph| {
                        if graph_geometry.get() == morph.to {
                            graph_geometry_morph.set(None);
                            false
                        } else {
                            true
                        }
                    });
                    let morph = match active_morph {
                        Some(active) if active.to.compact == target.compact => Some(active),
                        _ if target.compact != previous_target.compact && !cx.reduce_motion() => {
                            let epoch = graph_geometry_morph
                                .get()
                                .map_or(0, |morph| morph.epoch.wrapping_add(1));
                            let morph = GraphGeometryMorph {
                                from: graph_geometry.get(),
                                to: target,
                                epoch,
                            };
                            graph_target_geometry.set(target);
                            graph_geometry_morph.set(Some(morph));
                            graph_hover_suppressed.set(true);
                            Some(morph)
                        }
                        _ => {
                            if graph_geometry.get() != target {
                                graph_hover_suppressed.set(true);
                            }
                            graph_target_geometry.set(target);
                            graph_geometry_morph.set(None);
                            graph_geometry.set(target);
                            None
                        }
                    };
                    let graph_spacer: AnyElement = match morph {
                        Some(morph) => {
                            let graph_geometry = graph_geometry.clone();
                            div()
                                .id("history-graph-morph-spacer")
                                .w(px(morph.from.width))
                                .flex_none()
                                .with_animation(
                                    SharedString::from(format!(
                                        "history-graph-morph-{}",
                                        morph.epoch
                                    )),
                                    crate::motion::COLLAPSE.animation(),
                                    move |element, progress| {
                                        let geometry = interpolate_graph_geometry(
                                            morph.from, morph.to, progress,
                                        );
                                        graph_geometry.set(geometry);
                                        element.w(px(geometry.width))
                                    },
                                )
                                .into_any_element()
                        }
                        None => div()
                            .w(px(graph_geometry.get().width))
                            .flex_none()
                            .into_any_element(),
                    };
                    div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .when(show_header, |element| {
                            element.child(
                                div()
                                    .id("history-column-header")
                                    .group("history-column-header")
                                    .relative()
                                    .h(px(24.0))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .border_b_1()
                                    .border_color(crate::theme::hairline(0.06))
                                    .text_size(px(9.5))
                                    .text_color(header_theme.text_faint)
                                    .child(graph_spacer)
                                    .child(div().flex_1().min_w(px(80.0)).child("Commit"))
                                    .child(optional_headers)
                                    .child(column_button),
                            )
                        })
                        .child(body)
                })
                .w_full()
                .flex_1()
                .min_h_0(),
            )
            .when_some(column_menu, |element, (position, menu, closing)| {
                element.child(popover::menu_at(
                    "history-columns-menu",
                    position,
                    menu,
                    closing,
                ))
            })
            .when_some(author_menu, |element, (position, menu, closing)| {
                element.child(popover::menu_at(
                    "history-author-menu",
                    position,
                    menu,
                    closing,
                ))
            })
    }
}
