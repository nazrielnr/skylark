use gpui::{px, Context};

use crate::comments::ReviewComment;
use crate::theme::Theme;

use super::diff_model::{
    body_height_with, body_rows, diff_line_height, flatten_rows, DiffRow, FOLD_TWEEN_WINDOW,
};
use super::Changes;

impl Changes {
    pub(crate) fn replace_file_body(&mut self, file_ix: usize, new_body: Vec<DiffRow>) {
        let Some(range) = self.row_ranges.get(file_ix).cloned() else {
            return;
        };
        let body = range.start + 1..range.end;
        let delta = new_body.len() as isize - body.len() as isize;
        // Only splice the rows that moved: `ListState::splice` clamps the
        // scroll anchor to the range start when the anchored row is inside it,
        // so replacing a whole body jumped the pane to the top of the file.
        let (prefix, suffix) = {
            let old = &self.rows[body.clone()];
            let prefix = old
                .iter()
                .zip(&new_body)
                .take_while(|(a, b)| a == b)
                .count();
            let suffix = old[prefix..]
                .iter()
                .rev()
                .zip(new_body[prefix..].iter().rev())
                .take_while(|(a, b)| a == b)
                .count();
            (prefix, suffix)
        };
        if delta == 0 && prefix + suffix >= body.len() {
            return;
        }
        let changed = body.start + prefix..body.end - suffix;
        let mid: Vec<DiffRow> = new_body[prefix..new_body.len() - suffix].to_vec();
        self.list.splice(changed.clone(), mid.len());
        self.rows.splice(changed, mid);
        self.row_ranges[file_ix] = range.start..(range.end as isize + delta) as usize;
        for r in &mut self.row_ranges[file_ix + 1..] {
            *r = (r.start as isize + delta) as usize..(r.end as isize + delta) as usize;
        }
    }

    pub(crate) fn toggle_fold(&mut self, file_ix: usize, cx: &mut Context<Self>) {
        let Some(parsed) = &self.parsed else {
            return;
        };
        let Some(file) = parsed.files.get(file_ix) else {
            return;
        };
        if self.wrap_lines {
            let collapsed = !self
                .folds
                .get(&file.path)
                .is_some_and(|fold| fold.collapsed);
            let body = if collapsed {
                Vec::new()
            } else {
                body_rows(
                    file_ix as u32,
                    file,
                    &self.comments_for(&file.path, cx),
                    self.draft_anchor_in(&file.path),
                    self.mode,
                )
            };
            let fold = self.folds.entry(file.path.clone()).or_default();
            fold.collapsed = collapsed;
            fold.toggled_at = None;
            self.replace_file_body(file_ix, body);
            cx.notify();
            return;
        }
        let expanded_height = body_height_with(
            file,
            &self.comments_for(&file.path, cx),
            self.draft_anchor_in(&file.path),
            self.mode,
            diff_line_height(Theme::of(cx)),
        );
        let fold = self.folds.entry(file.path.clone()).or_default();
        let currently_collapsed = fold.collapsed;
        fold.from = if currently_collapsed {
            0.0
        } else {
            expanded_height
        };
        fold.to = if currently_collapsed {
            expanded_height
        } else {
            0.0
        };
        fold.collapsed = !currently_collapsed;
        fold.epoch += 1;
        fold.toggled_at = Some(std::time::Instant::now());
        // The body tweens as ONE clipped stand-in row; the settle sweep
        // swaps it for steady rows (all lines, or none) once the window
        // elapses.
        self.replace_file_body(
            file_ix,
            vec![DiffRow::FoldingBody {
                file: file_ix as u32,
            }],
        );
        self.ensure_fold_settle(cx);
    }

    /// Keep a sweep alive while any [`DiffRow::FoldingBody`] stand-ins
    /// remain; each tick settles the ones whose tween window has elapsed.
    fn ensure_fold_settle(&mut self, cx: &mut Context<Self>) {
        if self.fold_settle.is_some() {
            return;
        }
        self.fold_settle = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(FOLD_TWEEN_WINDOW).await;
                let more = this
                    .update(cx, |changes, cx| changes.settle_folds(cx))
                    .unwrap_or(false);
                if !more {
                    break;
                }
            }
            this.update(cx, |changes, _| changes.fold_settle = None)
                .ok();
        }));
    }

    /// Replace every settled folding stand-in with its steady-state rows.
    /// Returns whether any stand-ins are still mid-tween.
    fn settle_folds(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(parsed) = &self.parsed else {
            return false;
        };
        let files = parsed.files.clone();
        let mut pending = false;
        for file_ix in (0..self.row_ranges.len()).rev() {
            let range = &self.row_ranges[file_ix];
            let folding = self.rows.get(range.start + 1)
                == Some(&DiffRow::FoldingBody {
                    file: file_ix as u32,
                });
            if !folding {
                continue;
            }
            let Some(file) = files.get(file_ix) else {
                continue;
            };
            let fold = self.folds.get(&file.path).copied().unwrap_or_default();
            if fold.animating() {
                pending = true;
                continue;
            }
            let body = if fold.collapsed {
                Vec::new()
            } else {
                body_rows(
                    file_ix as u32,
                    file,
                    &self.comments_for(&file.path, cx),
                    self.draft_anchor_in(&file.path),
                    self.mode,
                )
            };
            self.replace_file_body(file_ix, body);
        }
        cx.notify();
        pending
    }

    /// Every parsed file currently folded shut?
    fn all_collapsed(&self) -> bool {
        let Some(parsed) = &self.parsed else {
            return false;
        };
        !parsed.files.is_empty()
            && parsed.files.iter().all(|file| {
                self.folds
                    .get(&file.path)
                    .is_some_and(|fold| fold.collapsed)
            })
    }

    /// Collapse every file section, or expand them all when everything is
    /// already shut (the toolbar's fold button, t3code parity). Steady-state
    /// writes ??? no per-row tween arming, the whole list just snaps. List
    /// splices run bottom-up over the OLD ranges (each is O(log n)), then
    /// the row model rebuilds wholesale; the scroll anchor rides the
    /// splices, landing on the nearest file header when its body vanishes.
    pub(crate) fn toggle_collapse_all(&mut self, cx: &mut Context<Self>) {
        let Some(parsed) = &self.parsed else {
            return;
        };
        let collapse = !self.all_collapsed();
        let files = parsed.files.clone();
        for file in files.iter() {
            let fold = self.folds.entry(file.path.clone()).or_default();
            fold.collapsed = collapse;
            fold.toggled_at = None;
        }
        let staged = self.staged_comments(cx);
        let draft = self.draft_anchor();
        for file_ix in (0..self.row_ranges.len().min(files.len())).rev() {
            let range = &self.row_ranges[file_ix];
            let body = range.start + 1..range.end;
            let new_len = if collapse {
                0
            } else {
                let file = &files[file_ix];
                let comments: Vec<ReviewComment> = staged
                    .iter()
                    .filter(|comment| !comment.is_file() && comment.path == file.path)
                    .cloned()
                    .collect();
                body_rows(
                    file_ix as u32,
                    file,
                    &comments,
                    self.draft_anchor_in(&file.path),
                    self.mode,
                )
                .len()
            };
            if body.len() != new_len {
                self.list.splice(body, new_len);
            }
        }
        let (rows, ranges) = flatten_rows(
            &files,
            &staged,
            draft
                .as_ref()
                .map(|(path, side, line)| (path.as_str(), *side, *line)),
            self.mode,
            |_| collapse,
        );
        self.rows = rows;
        self.row_ranges = ranges;
        cx.notify();
    }

    /// Swap unified ??? split (toolbar toggle). The parse is untouched ??? only
    /// the flattening changes ??? so this rebuilds the row model and re-anchors
    /// the scroll onto whichever file was under the viewport's top edge (row
    /// indices do not survive the re-pairing).
    pub(crate) fn toggle_mode(&mut self, cx: &mut Context<Self>) {
        self.mode = self.mode.toggled();
        self.reset_horizontal_scroll();
        let split = self.mode.is_split();
        crate::settings::update(crate::settings::SavePolicy::Immediate, cx, |settings| {
            settings.diff_split = split;
        });
        // A draft's `+` sits in a column that may not exist after the swap.
        self.hover = None;
        self.reflatten(cx);
    }

    pub(crate) fn toggle_wrap(&mut self, cx: &mut Context<Self>) {
        self.wrap_lines = !self.wrap_lines;
        self.reset_horizontal_scroll();
        let wrap = self.wrap_lines;
        crate::settings::update(crate::settings::SavePolicy::Immediate, cx, |settings| {
            settings.diff_wrap = wrap;
        });

        // A folding stand-in has an analytic fixed-line height. Settle it
        // before switching to variable-height rows; steady rows can then be
        // measured by the virtual list at the current pane width.
        let folding = self
            .rows
            .iter()
            .any(|row| matches!(row, DiffRow::FoldingBody { .. }));
        if folding {
            self.fold_settle = None;
            for fold in self.folds.values_mut() {
                fold.toggled_at = None;
            }
            self.reflatten(cx);
        } else {
            self.list.remeasure();
            cx.notify();
        }
    }

    pub(crate) fn reflatten(&mut self, cx: &mut Context<Self>) {
        let Some(parsed) = &self.parsed else {
            cx.notify();
            return;
        };
        let files = parsed.files.clone();
        let top = self.list.logical_scroll_top().item_ix;
        let anchor_file = self
            .row_ranges
            .iter()
            .position(|range| range.contains(&top));
        let collapsed: Vec<bool> = files
            .iter()
            .map(|file| {
                self.folds
                    .get(&file.path)
                    .is_some_and(|fold| fold.collapsed)
            })
            .collect();
        let staged = self.staged_comments(cx);
        let draft = self.draft_anchor();
        let (rows, ranges) = flatten_rows(
            &files,
            &staged,
            draft
                .as_ref()
                .map(|(path, side, line)| (path.as_str(), *side, *line)),
            self.mode,
            |ix| collapsed.get(ix).copied().unwrap_or(false),
        );
        let row_height = px(diff_line_height(Theme::of(cx)));
        self.list.reset_with_uniform_height(rows.len(), row_height);
        self.rows = rows;
        self.row_ranges = ranges;
        if let Some(start) = anchor_file
            .and_then(|ix| self.row_ranges.get(ix))
            .map(|r| r.start)
        {
            self.list.scroll_to_reveal_item(start);
        }
        cx.notify();
    }

}
