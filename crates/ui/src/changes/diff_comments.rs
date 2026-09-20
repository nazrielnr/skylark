use gpui::{div, prelude::*, px, AnyElement, App, Context, Focusable as _, Window};

use crate::comments::{self, CommentSide, ReviewComment};
use crate::composer::{ComposerInput, ComposerInputEvent};
use crate::theme::Theme;

use super::diff_model::{
    body_rows, comment_state_key, CommentDraft, DiffRow, HoverRow, ACCENT_BAR_WIDTH,
};
use super::Changes;

pub const COMMENT_ADDER_SIZE: f32 = 16.0;

/// A split row's `+` only ever appears in the right column, which carries one
/// gutter — so the offset is the same for every line. It is measured from the
/// column, not the row: the halves are fluid, so the right one has no
/// absolute left edge to measure from.
pub fn split_adder_left(gutter_px: f32) -> f32 {
    ACCENT_BAR_WIDTH + (gutter_px - COMMENT_ADDER_SIZE) / 2.0
}

/// A unified row carries both gutters side by side, and a deletion numbers in
/// the first.
pub fn comment_adder_left(side: CommentSide, gutter_px: f32) -> f32 {
    let column = match side {
        CommentSide::Old => 0.0,
        CommentSide::New => gutter_px,
    };
    ACCENT_BAR_WIDTH + column + (gutter_px - COMMENT_ADDER_SIZE) / 2.0
}

pub(crate) fn positioned_adder(left: f32, adder: AnyElement) -> gpui::Div {
    div()
        .absolute()
        .left(px(left))
        .top(px(0.0))
        .h_full()
        .flex()
        .items_center()
        .child(adder)
}

pub(crate) fn render_comment_adder(
    path: &str,
    side: CommentSide,
    line: u32,
    theme: &Theme,
    cx: &Context<Changes>,
) -> AnyElement {
    let target = path.to_string();
    crate::comment_ui::render_comment_adder(
        format!("cmt-add-{path}-{}-{line}", side.tag()).into(),
        theme,
        cx,
        move |this, window, cx| this.open_draft(target.clone(), side, line, window, cx),
    )
}

/// Mirrors [`ReviewComment::cite_path`] for the not-yet-staged note.
pub fn draft_cite_path(draft: &CommentDraft) -> &str {
    match draft.side {
        CommentSide::Old => draft.old_path.as_deref().unwrap_or(&draft.path),
        CommentSide::New => &draft.path,
    }
}

impl Changes {
    /// Cloned because rendering borrows `self` mutably a moment later.
    pub(crate) fn staged_comments(&self, cx: &App) -> Vec<ReviewComment> {
        let state = self.state.read(cx);
        state
            .review_comments(&state.composer_key())
            .iter()
            .filter(|comment| {
                self.draft
                    .as_ref()
                    .and_then(|draft| draft.editing_id.as_ref())
                    != Some(&comment.id)
            })
            .cloned()
            .collect()
    }

    pub(crate) fn comments_for(&self, path: &str, cx: &App) -> Vec<ReviewComment> {
        self.staged_comments(cx)
            .into_iter()
            .filter(|comment| !comment.is_file() && comment.path == path)
            .collect()
    }

    /// The parsed diff's pre-rename path for `path`, when the file moved.
    pub(crate) fn old_path_of(&self, path: &str) -> Option<String> {
        self.parsed
            .as_ref()?
            .files
            .iter()
            .find(|file| file.path == path)?
            .old_path
            .clone()
    }

    /// A draft belongs to the checkout it was opened over. Chat navigation
    /// swaps both the diff under it and the composer it would stage onto, so
    /// the half-written note is dropped rather than following the user across.
    pub(crate) fn discard_stale_draft(&mut self, cx: &mut Context<Self>) {
        let key = self.state.read(cx).composer_key();
        if self.draft.as_ref().is_some_and(|draft| draft.key != key) {
            self.draft = None;
            self.sync_comment_rows(cx);
            cx.notify();
        }
    }

    pub(crate) fn draft_anchor(&self) -> Option<(String, CommentSide, u32)> {
        self.draft
            .as_ref()
            .map(|draft| (draft.path.clone(), draft.side, draft.line))
    }

    pub(crate) fn draft_anchor_in(&self, path: &str) -> Option<(CommentSide, u32)> {
        self.draft
            .as_ref()
            .filter(|draft| draft.path == path)
            .map(|draft| (draft.side, draft.line))
    }

    pub(crate) fn sync_comment_rows(&mut self, cx: &mut Context<Self>) {
        if self.parsed.is_none() {
            return;
        }
        let staged = self.staged_comments(cx);
        let draft = self.draft_anchor();
        let key = comment_state_key(&staged, draft.as_ref());
        if key == self.comment_key {
            return;
        }
        self.comment_key = key;
        let Some(parsed) = &self.parsed else {
            return;
        };
        let files = parsed.files.clone();
        for file_ix in (0..self.row_ranges.len().min(files.len())).rev() {
            let file = &files[file_ix];
            // A mid-tween stand-in is the settle sweep's to replace.
            if self
                .folds
                .get(&file.path)
                .is_some_and(|fold| fold.collapsed)
            {
                continue;
            }
            let range = &self.row_ranges[file_ix];
            if self.rows.get(range.start + 1)
                == Some(&DiffRow::FoldingBody {
                    file: file_ix as u32,
                })
            {
                continue;
            }
            let comments: Vec<ReviewComment> = staged
                .iter()
                .filter(|comment| !comment.is_file() && comment.path == file.path)
                .cloned()
                .collect();
            let body = body_rows(
                file_ix as u32,
                file,
                &comments,
                self.draft_anchor_in(&file.path),
                self.mode,
            );
            self.replace_file_body(file_ix, body);
        }
        cx.notify();
    }

    pub(crate) fn set_hover(
        &mut self,
        path: &str,
        anchor: Option<(CommentSide, u32)>,
        cx: &mut Context<Self>,
    ) {
        let next = anchor.map(|(side, line)| HoverRow {
            path: path.to_string(),
            side,
            line,
        });
        if next != self.hover {
            self.hover = next;
            cx.notify();
        }
    }

    pub(crate) fn hovering(&self, path: &str, anchor: (CommentSide, u32)) -> bool {
        self.hover
            .as_ref()
            .is_some_and(|hover| hover.path == path && (hover.side, hover.line) == anchor)
    }

    pub(crate) fn clear_hover_at(
        &mut self,
        path: &str,
        anchor: (CommentSide, u32),
        cx: &mut Context<Self>,
    ) {
        if self.hovering(path, anchor) {
            self.hover = None;
            cx.notify();
        }
    }

    pub fn open_draft(
        &mut self,
        path: String,
        side: CommentSide,
        line: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| ComposerInput::new("Request a change…", cx));
        let events = cx.subscribe(&input, |this: &mut Self, _, event, cx| match event {
            ComposerInputEvent::Submitted => this.commit_draft(cx),
            ComposerInputEvent::Edited => cx.notify(),
            _ => {}
        });
        let handle = input.read(cx).focus_handle(cx);
        let key = self.state.read(cx).composer_key();
        let old_path = self.old_path_of(&path);
        self.draft = Some(CommentDraft {
            editing_id: None,
            key,
            path,
            old_path,
            side,
            line,
            input,
            _events: events,
        });
        window.focus(&handle, cx);
        self.sync_comment_rows(cx);
        cx.notify();
    }

    pub fn edit_comment(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let Some(comment) = state
            .review_comments(&state.composer_key())
            .iter()
            .find(|comment| comment.id == id && !comment.is_file())
            .cloned()
        else {
            return;
        };
        let Some((side, line)) = comment.diff_anchor() else {
            return;
        };
        self.open_draft(comment.path.clone(), side, line, window, cx);
        let draft = self.draft.as_mut().unwrap();
        draft.editing_id = Some(comment.id);
        if let comments::CommentSource::Diff { old_path, .. } = comment.source {
            draft.old_path = old_path;
        }
        draft
            .input
            .update(cx, |input, cx| input.set_text(comment.body, cx));
        self.sync_comment_rows(cx);
        cx.notify();
    }

    pub fn cancel_draft(&mut self, cx: &mut Context<Self>) {
        self.draft = None;
        self.sync_comment_rows(cx);
        cx.notify();
    }

    pub fn commit_draft(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.draft.take() else {
            return;
        };
        let body = draft.input.read(cx).text().trim().to_string();
        if body.is_empty() {
            self.sync_comment_rows(cx);
            cx.notify();
            return;
        }

        // `draft.key`, not the live one: the note stages onto the composer it
        // was written against even if the selection moved under it.
        let key = draft.key;
        self.state.update(cx, |state, cx| {
            if let Some(id) = draft.editing_id {
                state.update_review_comment_body(&key, &id, body);
            } else {
                let comment = ReviewComment::new(draft.path, draft.side, draft.line, body)
                    .renamed_from(draft.old_path);
                state.add_review_comment(&key, comment);
            }
            cx.notify();
        });
        self.sync_comment_rows(cx);
        cx.notify();
    }

    pub fn remove_comment(&mut self, id: &str, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            let key = state.composer_key();
            state.remove_review_comment(&key, id);
            cx.notify();
        });
        self.sync_comment_rows(cx);
        cx.notify();
    }
}
