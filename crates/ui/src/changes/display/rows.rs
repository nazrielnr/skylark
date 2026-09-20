//! Virtualized unified/split diff row rendering.

use super::*;

impl Changes {
    pub(super) fn render_row(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(parsed) = &self.parsed else {
            return gpui::Empty.into_any_element();
        };
        let files = parsed.files.clone();
        let parsed_key = parsed.key.clone();
        let Some(row) = self.rows.get(ix).copied() else {
            return gpui::Empty.into_any_element();
        };
        let theme = Theme::of(cx).clone();
        let highlight = files
            .get(row.file())
            .and_then(|file| self.request_highlight(file, &parsed_key, cx));
        let horizontal = &self.parsed.as_ref().unwrap().horizontal[row.file()];
        let code_width = match files.get(row.file()) {
            Some(file) if !self.wrap_lines => DiffCodeWidth::Scrollable(horizontal.metrics(
                file,
                highlight.as_ref(),
                &theme,
                window.text_system(),
                crate::typography::generation(cx),
            )),
            _ => DiffCodeWidth::Wrapped,
        };
        let code_scroll = DiffCodeScrollContext {
            handle: horizontal.scroll.clone(),
            prefix: SharedString::from(format!("changes-code-row-{ix}")),
        };
        match row {
            DiffRow::FileHeader { file } => {
                let Some(file_diff) = files.get(file as usize) else {
                    return gpui::Empty.into_any_element();
                };
                let fold = self.folds.get(&file_diff.path).copied().unwrap_or_default();
                self.render_file_header(
                    file as usize,
                    file_diff,
                    &fold,
                    FileHeaderPresentation::Row,
                    &theme,
                    cx,
                )
            }
            DiffRow::Notice { file, notice } => files
                .get(file as usize)
                .and_then(|f| file_notices(f).into_iter().nth(notice as usize))
                .map(|text| notice_row(text, &theme))
                .unwrap_or_else(|| gpui::Empty.into_any_element()),
            DiffRow::HunkHeader { file, hunk } => files
                .get(file as usize)
                .and_then(|f| f.hunks.get(hunk as usize))
                .map(|h| hunk_header_row(&h.header, &theme))
                .unwrap_or_else(|| gpui::Empty.into_any_element()),
            DiffRow::Line {
                file,
                hunk,
                line,
                flat: _,
            } => {
                let Some(file_diff) = files.get(file as usize) else {
                    return gpui::Empty.into_any_element();
                };
                let Some(line) = file_diff
                    .hunks
                    .get(hunk as usize)
                    .and_then(|h| h.lines.get(line as usize))
                else {
                    return gpui::Empty.into_any_element();
                };
                let spans = highlight
                    .as_deref()
                    .map(|highlights| highlights.spans(line))
                    .unwrap_or(&[]);
                let gutter_px = gutter_width(file_diff);
                let row = diff_line_row(
                    line,
                    spans,
                    &theme,
                    gutter_px,
                    code_width,
                    Some(code_scroll.slot("unified")),
                );
                let Some((side, line_no)) = line_anchor(line) else {
                    return row;
                };
                let path = file_diff.path.clone();
                let hovered = self.hovering(&path, (side, line_no));
                let move_path = path.clone();
                let leave_path = path.clone();
                div()
                    .id(("diff-line", ix))
                    .w_full()
                    .relative()
                    .child(row)
                    .when(hovered, |el| {
                        el.child(positioned_adder(
                            comment_adder_left(side, gutter_px),
                            render_comment_adder(&path, side, line_no, &theme, cx),
                        ))
                    })
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        this.set_hover(&move_path, Some((side, line_no)), cx);
                    }))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if !*hovered {
                            this.clear_hover_at(&leave_path, (side, line_no), cx);
                        }
                    }))
                    .into_any_element()
            }
            DiffRow::SplitLine {
                file,
                hunk,
                left,
                right,
            } => {
                let Some(file_diff) = files.get(file as usize) else {
                    return gpui::Empty.into_any_element();
                };
                let Some(lines) = file_diff.hunks.get(hunk as usize).map(|h| &h.lines) else {
                    return gpui::Empty.into_any_element();
                };
                let gutter_px = gutter_width(file_diff);
                // Same slot on both sides = a context row: one line, drawn in
                // both columns.
                let mirrored = left.is_some() && left == right;
                let left = left.and_then(|slot| lines.get(slot as usize));
                let right = right.and_then(|slot| lines.get(slot as usize));
                // `\ No newline at end of file` is not code on one side — it
                // is a note about the row, so it spans both columns. Pairing
                // never puts a marker opposite code, so either side having one
                // means the whole row is the marker.
                if let Some(line) = [left, right]
                    .into_iter()
                    .flatten()
                    .find(|line| line.kind == LineKind::Meta)
                {
                    return meta_line_row(&line.text, &theme, 2.0 * (ACCENT_BAR_WIDTH + gutter_px));
                }
                // Refcounted, not cloned per listener: a split row wires up to
                // four of them, and this runs for every row in the viewport
                // plus the list's overdraw, every frame.
                let path: SharedString = file_diff.path.clone().into();
                // A mirrored row's columns carry the same text and the same
                // spans, so the runs are built once and shared — context is
                // most of a diff, so this is most of the rows.
                let shared_runs = mirrored
                    .then(|| left.map(|line| line_runs(line, highlight.as_deref(), &theme)))
                    .flatten();
                let cell = |line: Option<&DiffLine>, old: bool| {
                    line.map(|line| {
                        let runs = shared_runs
                            .clone()
                            .unwrap_or_else(|| line_runs(line, highlight.as_deref(), &theme));
                        let number = if old { line.old_no } else { line.new_no };
                        split_line_cell(
                            line,
                            number,
                            runs,
                            &theme,
                            gutter_px,
                            code_width,
                            Some(code_scroll.slot(if old { "old" } else { "new" })),
                        )
                    })
                };
                // The left column is inert. It shows the pre-change file, and
                // a deleted line is not there to be changed — a note on it
                // would cite a line the agent cannot edit. Everything is cited
                // against the new file, so only the right column takes a `+`.
                // Cards for old-side notes still render (they are pushed by
                // the row, not the column), so switching layouts never hides
                // one that is already staged.
                let left = cell(left, true)
                    .map(IntoElement::into_any_element)
                    .unwrap_or_else(|| split_filler().into_any_element());
                let right = match (cell(right, false), right.and_then(line_anchor)) {
                    (Some(cell), Some(anchor)) => {
                        let (side, line_no) = anchor;
                        let (move_path, leave_path) = (path.clone(), path.clone());
                        cell.id(("split-new", ix))
                            .when(self.hovering(&path, anchor), |el| {
                                el.relative().child(positioned_adder(
                                    split_adder_left(gutter_px),
                                    render_comment_adder(&path, side, line_no, &theme, cx),
                                ))
                            })
                            .on_mouse_move(cx.listener(move |this, _, _, cx| {
                                this.set_hover(&move_path, Some(anchor), cx);
                            }))
                            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                                if !*hovered {
                                    this.clear_hover_at(&leave_path, anchor, cx);
                                }
                            }))
                            .into_any_element()
                    }
                    (Some(cell), None) => cell.into_any_element(),
                    (None, _) => split_filler().into_any_element(),
                };
                split_row(left, right, self.wrap_lines, &theme).into_any_element()
            }
            DiffRow::CommentCard { file, card } => {
                let Some(file_diff) = files.get(file as usize) else {
                    return gpui::Empty.into_any_element();
                };
                let comments = self.comments_for(&file_diff.path, cx);
                match comments.get(card as usize) {
                    Some(comment) => crate::comment_ui::render_comment_card(
                        comment,
                        &theme,
                        cx,
                        Self::edit_comment,
                        Self::remove_comment,
                        None,
                    ),
                    None => gpui::Empty.into_any_element(),
                }
            }
            DiffRow::CommentDraft { file } => {
                let Some(file_diff) = files.get(file as usize) else {
                    return gpui::Empty.into_any_element();
                };
                match self
                    .draft
                    .as_ref()
                    .filter(|draft| draft.path == file_diff.path)
                {
                    // Header cites the same path the staged card and the
                    // prompt bullet will.
                    Some(draft) => crate::comment_ui::render_comment_draft(
                        draft_cite_path(draft),
                        draft.line,
                        draft.input.clone(),
                        draft.editing_id.is_some(),
                        &theme,
                        cx,
                        Self::cancel_draft,
                        Self::commit_draft,
                        None,
                    ),
                    None => gpui::Empty.into_any_element(),
                }
            }
            DiffRow::BodyPad { .. } => div().w_full().h(px(BODY_BOTTOM_PAD)).into_any_element(),
            DiffRow::FoldingBody { file } => {
                let Some(file_diff) = files.get(file as usize) else {
                    return gpui::Empty.into_any_element();
                };
                let fold = self.folds.get(&file_diff.path).copied().unwrap_or_default();
                let (from, to) = (fold.from, fold.to);
                // Only the revealable slice is built — the tween never pays
                // for lines it cannot show.
                let cap = from.max(to).min(FOLD_TWEEN_MAX_PX);
                let body = render_file_body_upto(
                    file_diff,
                    highlight,
                    &theme,
                    cap,
                    self.mode,
                    code_width,
                    Some(DiffCodeScrollContext {
                        handle: code_scroll.handle.clone(),
                        prefix: SharedString::from(format!(
                            "changes-fold-code-{file}-{}",
                            fold.epoch
                        )),
                    }),
                );
                let clipped = div().w_full().overflow_hidden().child(body);
                if fold.animating() {
                    clipped
                        .with_animation(
                            SharedString::from(format!("fold-{}-{}", file_diff.path, fold.epoch)),
                            COLLAPSE.animation(),
                            move |el, t| el.h(px(motion::lerp(from, to, t))),
                        )
                        .into_any_element()
                } else {
                    // Post-tween, pre-settle: hold the full target height so
                    // the settle splice swaps rows without any reflow (the
                    // capped slice always covers what the viewport can see —
                    // tweens start from a clicked, on-screen header).
                    clipped.h(px(to)).into_any_element()
                }
            }
        }
    }
}
