//! Transcript tool-group folding and rendering.

use super::*;

impl Transcript {
    pub(crate) fn render_tool_group(
        &mut self,
        row_id: &SharedString,
        tools: &Arc<Vec<ToolItem>>,
        summary: &SharedString,
        auto_open: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut fold = self.folds.get(row_id).copied().unwrap_or_default();
        let collapses = tool_group_collapses(tools);
        let arrival_pending = !cx.reduce_motion()
            && self.tool_group_reveals.get(row_id).is_some_and(|reveal| {
                reveal.starts.iter().flatten().any(|start| {
                    Instant::now()
                        .checked_duration_since(*start)
                        .unwrap_or_default()
                        < TOOL_CONNECTOR_REVEAL.total()
                })
            });
        let effective_auto_open = auto_open || arrival_pending;
        let open = !collapses || fold.open.unwrap_or(effective_auto_open);
        if collapses {
            let reveal = self.tool_group_reveals.entry(row_id.clone()).or_default();
            if reveal
                .rendered_open
                .is_some_and(|previous| previous != open)
            {
                fold.from = reveal.rendered_height;
                fold.toggled_at = Some(Instant::now());
                fold.disclosure_at = fold.toggled_at;
                self.folds.insert(row_id.clone(), fold);
            }
            reveal.rendered_open = Some(open);
        }
        let active = collapses && auto_open;

        let body_visible = open
            || (!cx.reduce_motion()
                && fold
                    .toggled_at
                    .is_some_and(|at| at.elapsed() < TOOL_FOLD.total()));
        let tools = if body_visible { tools.as_slice() } else { &[] };
        let details: Vec<Option<Arc<ToolDetail>>> = tools
            .iter()
            .map(|tool| {
                if is_spawn_link(tool) {
                    return None;
                }
                let mut best: Option<(u64, Arc<ToolDetail>)> = None;
                for blob_ref in [&tool.diff_ref, &tool.output_ref].into_iter().flatten() {
                    if let Some(BlobFetch::Ready(detail)) = self.blob_details.get(blob_ref) {
                        let order = self.blob_fetch_order.get(blob_ref).copied().unwrap_or(0);
                        if best.as_ref().is_none_or(|(o, _)| order > *o) {
                            best = Some((order, detail.clone()));
                        }
                    }
                }
                best.map(|(_, d)| d).or_else(|| tool.detail.clone())
            })
            .collect();
        let invocations: Vec<Option<Arc<ToolDetail>>> = tools
            .iter()
            .map(|tool| tool.invocation.clone().filter(|_| !is_spawn_link(tool)))
            .collect();
        let affordances: Vec<Option<ChipAffordance>> = tools
            .iter()
            .map(|tool| {
                let shown: Option<&SharedString> = {
                    let mut best: Option<(u64, &SharedString)> = None;
                    for blob_ref in [&tool.diff_ref, &tool.output_ref].into_iter().flatten() {
                        if matches!(self.blob_details.get(blob_ref), Some(BlobFetch::Ready(_))) {
                            let order = self.blob_fetch_order.get(blob_ref).copied().unwrap_or(0);
                            if best.is_none_or(|(o, _)| order > o) {
                                best = Some((order, blob_ref));
                            }
                        }
                    }
                    best.map(|(_, r)| r)
                };
                let candidates = [
                    (tool.diff_ref.as_ref(), "diff", None),
                    (tool.output_ref.as_ref(), "output", tool.output_bytes),
                ];
                for (blob_ref, what, bytes) in candidates {
                    let Some(blob_ref) = blob_ref else { continue };
                    let label = match self.blob_details.get(blob_ref) {
                        Some(BlobFetch::Ready(_)) => {
                            if shown == Some(blob_ref) {
                                continue;
                            }
                            format!("Show full {what}")
                        }
                        Some(BlobFetch::Loading(_)) => format!("Loading full {what}…"),
                        Some(BlobFetch::Failed) => {
                            format!("Couldn't load full {what} — tap to retry")
                        }
                        None => match bytes {
                            Some(b) => format!("Show full {what} ({})", format_kb(b)),
                            None => format!("Show full {what}"),
                        },
                    };
                    return Some(ChipAffordance {
                        blob_ref: blob_ref.clone(),
                        label: SharedString::from(label),
                    });
                }
                None
            })
            .collect();
        let detail_folds: Vec<FoldState> = details
            .iter()
            .zip(&invocations)
            .enumerate()
            .map(|(ix, (detail, invocation))| {
                if detail.is_none() && invocation.is_none() {
                    return FoldState::default();
                }
                self.tool_details
                    .get(&SharedString::from(format!("{row_id}#d{ix}")))
                    .copied()
                    .unwrap_or_default()
            })
            .collect();
        let detail_opens: Vec<bool> = details
            .iter()
            .zip(&invocations)
            .zip(&detail_folds)
            .zip(tools.iter())
            .map(|(((detail, invocation), fold), tool)| {
                let default_open = tool.is_thought && !tool.resolved;
                (detail.is_some() || invocation.is_some()) && fold.open.unwrap_or(default_open)
            })
            .collect();
        let detail_highlights: Vec<Option<Arc<crate::changes::DiffHighlights>>> = details
            .iter()
            .enumerate()
            .map(|(ix, detail)| {
                detail
                    .as_deref()
                    .filter(|_| detail_opens[ix])
                    .and_then(|detail| self.tool_diff_highlight_for(row_id, ix, detail, cx))
            })
            .collect();
        let base_row_height = if collapses {
            TOOL_TREE_ROW_HEIGHT
        } else {
            CHIP_HEIGHT
        };
        let mut motion_active = false;
        let row_heights: Vec<f32> = details
            .iter()
            .zip(&invocations)
            .zip(&affordances)
            .zip(&detail_opens)
            .zip(&detail_folds)
            .map(|((((detail, invocation), affordance), open), fold)| {
                let target = if *open {
                    base_row_height
                        + invocation.as_deref().map_or(0.0, detail_height)
                        + detail.as_deref().map_or(0.0, detail_height)
                        + if affordance.is_some() {
                            BLOB_AFFORDANCE_HEIGHT
                        } else {
                            0.0
                        }
                } else {
                    base_row_height
                };
                if !cx.reduce_motion() {
                    if let Some(at) = fold.toggled_at {
                        let t = TOOL_FOLD
                            .curve
                            .eval(at.elapsed().as_secs_f32() / TOOL_FOLD.total().as_secs_f32());
                        if t < 1.0 {
                            motion_active = true;
                        }
                        return motion::lerp(
                            fold.from + base_row_height - CHIP_CARD_HEIGHT,
                            target,
                            t,
                        );
                    }
                }
                target
            })
            .collect();
        let reduce_motion = cx.reduce_motion();
        let now = Instant::now();
        let reveal_progress: Vec<f32> = (0..tools.len())
            .map(|ix| {
                let start = self
                    .tool_group_reveals
                    .get(row_id)
                    .and_then(|reveal| reveal.starts.get(ix))
                    .copied()
                    .flatten();
                tool_row_reveal_progress(start, now, reduce_motion)
            })
            .collect();
        let connector_progress: Vec<f32> = (0..tools.len())
            .map(|ix| {
                let start = self
                    .tool_group_reveals
                    .get(row_id)
                    .and_then(|reveal| reveal.starts.get(ix))
                    .copied()
                    .flatten();
                tool_connector_reveal_progress(start, now, reduce_motion)
            })
            .collect();
        let header_reveal = tool_row_reveal_progress(
            self.tool_group_reveals
                .get(row_id)
                .and_then(|reveal| reveal.header_started_at),
            now,
            reduce_motion,
        );
        if header_reveal < 1.0
            || reveal_progress.iter().any(|progress| *progress < 1.0)
            || connector_progress.iter().any(|progress| *progress < 1.0)
        {
            motion_active = true;
        }
        let revealed_height = CHIPS_TOP_PAD
            + row_heights
                .iter()
                .zip(&reveal_progress)
                .map(|(height, progress)| height * progress)
                .sum::<f32>();
        let viewport_height = revealed_height;
        let target = if open { viewport_height } else { 0.0 };
        let shimmer_phase = if active && !reduce_motion {
            motion::pulse_lease(cx.entity_id(), cx);
            self.tool_group_reveals
                .get(row_id)
                .and_then(|reveal| reveal.shimmer_started_at)
                .map(|start| tool_title_shimmer_phase(start, now))
        } else {
            None
        };
        let disclosure_progress = if reduce_motion {
            if open { 1.0 } else { 0.0 }
        } else {
            tool_disclosure_progress(open, fold, now)
        };

        let toggle_id = row_id.clone();
        let header = div()
            .id(SharedString::from(format!("{row_id}-hdr")))
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .gap(crate::typography::ui_rems(6.0))
            .pr(crate::typography::ui_rems(4.0))
            .h(crate::typography::ui_rems(TOOL_GROUP_HEADER_HEIGHT))
            .cursor_pointer()
            .text_size(crate::typography::ui_rems(TOOL_LABEL_SIZE))
            .line_height(crate::typography::ui_rems(TOOL_LABEL_LINE_HEIGHT))
            .text_color(theme.text_muted)
            .hover(|s| s.text_color(theme.text))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.toggle_fold(toggle_id.clone(), target, effective_auto_open);
                cx.notify();
            }))
            .child(
                div()
                    // Keep the title adjacent to its disclosure affordance.
                    .w(px(22.0))
                    .h(px(18.0))
                    .flex_none()
                    .relative()
                    .child(
                        crate::icons::icon(crate::icons::ALT_ARROW_DOWN)
                            .absolute()
                            .left(px(ACTIVITY_TRUNK_X - 7.0))
                            .top(px(2.0))
                            .size(px(14.0))
                            .with_transformation(gpui::Transformation::rotate(gpui::radians(
                                -std::f32::consts::FRAC_PI_2 * (1.0 - disclosure_progress),
                            )))
                            .text_color(theme.text_muted),
                    ),
            )
            .child(
                div()
                    .min_w_0()
                    .h(px(TOOL_LABEL_LINE_HEIGHT))
                    .flex()
                    .items_center()
                    .truncate()
                    .child(tool_group_title(summary.clone(), shimmer_phase, theme)),
            );

        let chips = div()
            .flex()
            .flex_col()
            .gap(px(CHIP_GAP))
            .pt(px(CHIPS_TOP_PAD))
            .children(tools.iter().enumerate().map(|(ix, tool)| {
                let reveal = reveal_progress[ix];
                let connector_reveal = connector_progress[ix];
                let content_reveal = tool_connector_parts(connector_reveal, ix > 0).1;
                let continuation_reveal =
                    tool_connector_continuation(connector_progress.get(ix + 1).copied());
                let row_height = row_heights[ix];
                if let Some(doc_id) = tool.subagent_ref.clone().filter(|_| is_spawn_link(tool)) {
                    let chat_id = self.chat_id.clone().unwrap_or_default();
                    let title = subagent_tab_title(&tool.call);
                    let frozen = matches!(
                        tool.subagent_status,
                        Some(SubagentStatus::Done) | Some(SubagentStatus::Failed)
                    );
                    return subagent_chip(
                        tool,
                        SharedString::from(format!("{row_id}#s{ix}")),
                        cx.listener(move |_, _, _, cx| {
                            cx.emit(TranscriptEvent::OpenSubagent {
                                chat_id: chat_id.clone(),
                                doc_id: doc_id.to_string(),
                                title: title.to_string(),
                                frozen,
                            });
                        }),
                        collapses,
                        theme,
                        cx.entity_id(),
                        cx,
                    );
                }
                let detail = details[ix].clone();
                let invocation = invocations[ix].clone();
                if detail.is_none() && invocation.is_none() {
                    return reveal_tool_row(
                        tool_chip(
                            tool,
                            collapses,
                            ix > 0,
                            ix + 1 < tools.len(),
                            content_reveal,
                            connector_reveal,
                            continuation_reveal,
                            theme,
                            cx.entity_id(),
                            cx,
                        ),
                        row_height,
                        reveal,
                    );
                }
                let affordance = affordances[ix].clone();
                let open = detail_opens[ix];
                let dfold = detail_folds[ix];
                let key = SharedString::from(format!("{row_id}#d{ix}"));
                let animating = dfold.epoch > 0
                    && dfold
                        .toggled_at
                        .is_some_and(|at| at.elapsed() < FOLD_TWEEN_WINDOW);
                let toggle_key = key.clone();
                let mut card = div()
                    .my(px((base_row_height - CHIP_CARD_HEIGHT) / 2.0))
                    .when(collapses, |el| el.ml(px(ACTIVITY_TEXT_GAP)))
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .when(!collapses, |card| {
                        card.rounded(px(9.0))
                            .border_1()
                            .border_color(crate::theme::hairline(0.07))
                            .bg(crate::theme::ink(0.03))
                    })
                    .child(
                        div()
                            .id(key.clone())
                            .h(px(if collapses {
                                CHIP_CARD_HEIGHT
                            } else {
                                CHIP_HEADER_HEIGHT
                            }))
                            .flex_none()
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                let entry =
                                    this.tool_details.entry(toggle_key.clone()).or_default();
                                let currently_open = entry.open.unwrap_or(open);
                                entry.from = row_height - base_row_height + CHIP_CARD_HEIGHT;
                                entry.open = Some(!currently_open);
                                entry.epoch += 1;
                                entry.toggled_at = Some(Instant::now());
                                cx.notify();
                            }))
                            .child(chip_header(tool, open, theme, cx.entity_id(), cx)),
                    );
                if open || animating {
                    let mut panel = div()
                        .flex_none()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .overflow_hidden();
                    if let Some(invocation) = invocation.as_deref() {
                        panel = panel
                            .child(
                                div()
                                    .h(px(DETAIL_SEPARATOR))
                                    .flex_none()
                                    .when(!collapses, |line| line.bg(crate::theme::hairline(0.06))),
                            )
                            .child(detail_body(invocation, None, theme));
                    }
                    if let Some(detail) = detail.as_deref() {
                        panel = panel
                            .child(
                                div()
                                    .h(px(DETAIL_SEPARATOR))
                                    .flex_none()
                                    .when(!collapses, |line| line.bg(crate::theme::hairline(0.06))),
                            )
                            .child(detail_body(detail, detail_highlights[ix].clone(), theme));
                    }
                    if let Some(ChipAffordance { blob_ref, label }) = affordance {
                        let loading = matches!(
                            self.blob_details.get(&blob_ref),
                            Some(BlobFetch::Loading(_))
                        );
                        let mut row = div()
                            .id(SharedString::from(format!("{key}-blob")))
                            .h(crate::typography::ui_rems(BLOB_AFFORDANCE_HEIGHT))
                            .flex_none()
                            .flex()
                            .items_center()
                            .text_size(crate::typography::ui_rems(TOOL_TEXT_SIZE))
                            .text_color(theme.text_faint)
                            .child(label);
                        if !loading {
                            row = row
                                .cursor_pointer()
                                .hover(|s| s.text_color(theme.text_muted))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.spawn_blob_fetch(blob_ref.clone(), cx);
                                    cx.notify();
                                }));
                        }
                        panel = panel.child(row);
                    }
                    card = card.child(panel);
                }
                let card = card.h(px(row_height - base_row_height + CHIP_CARD_HEIGHT));
                let card = div().min_w_0().flex_1().child(card);
                let row = div()
                    .w_full()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .when(collapses, |row| {
                        row.child(activity_rail(
                            tool,
                            ix > 0,
                            ix + 1 < tools.len(),
                            connector_reveal,
                            continuation_reveal,
                            row_height,
                            theme,
                        ))
                    })
                    .child(card.when(collapses && content_reveal < 1.0, |card| {
                        card.relative()
                            .top(px(4.0 * (1.0 - content_reveal)))
                            .opacity(content_reveal)
                    }));
                reveal_tool_row(row.into_any_element(), row_height, reveal)
            }));

        let body_height = if !collapses {
            revealed_height
        } else if !cx.reduce_motion() {
            if let Some(at) = fold.toggled_at {
                let t = TOOL_FOLD
                    .curve
                    .eval(at.elapsed().as_secs_f32() / TOOL_FOLD.total().as_secs_f32());
                if t < 1.0 {
                    motion_active = true;
                }
                motion::lerp(fold.from, target, t)
            } else {
                target
            }
        } else {
            target
        };
        if let Some(reveal) = self.tool_group_reveals.get_mut(row_id) {
            reveal.rendered_height = body_height;
        }
        let body: AnyElement = if !collapses {
            chips.into_any_element()
        } else {
            div()
                .overflow_hidden()
                .h(px(body_height))
                .child(chips)
                .into_any_element()
        };

        let view = cx.entity_id();
        div()
            .relative()
            .flex()
            .flex_col()
            .font_family(theme.font_sans_fixed.clone())
            .when(collapses, |el| {
                el.child(reveal_tool_row(
                    header.into_any_element(),
                    TOOL_GROUP_HEADER_HEIGHT,
                    header_reveal,
                ))
            })
            .child(body)
            .when(motion_active, |group| {
                group.child(
                    canvas(
                        |_, _, _| (),
                        move |_, _, window, _| {
                            window.on_next_frame(move |_, cx| cx.notify(view));
                        },
                    )
                    .absolute()
                    .inset_0(),
                )
            })
            .into_any_element()
    }
}
