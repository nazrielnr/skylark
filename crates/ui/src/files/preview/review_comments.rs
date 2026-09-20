//! Inline review-comment editor behavior.

use super::*;

impl FilesSurface {
    pub(super) fn sync_editor_comment_anchors(
        &mut self,
        path: &str,
        editor: &Entity<super::super::editor::FileEditorState>,
        cx: &mut Context<Self>,
    ) {
        let comments = self.staged_file_comments(path, cx);
        let anchors = self
            .preview
            .comment_anchors
            .entry(path.to_string())
            .or_default();
        anchors.retain(|id, _| comments.iter().any(|comment| comment.id == *id));
        let missing = comments
            .into_iter()
            .filter(|comment| !anchors.contains_key(&comment.id))
            .collect::<Vec<_>>();
        for comment in missing {
            let anchor = editor.update(cx, |state, cx| {
                let line = comment.line.min(state.text().lines_len().max(1) as u32);
                let (range, edge) = comment_anchor_range(state.text(), line)?;
                let range = state.create_decorations_collection(
                    vec![TextDecoration::new(range, HighlightStyle::default())],
                    cx,
                );
                Some(EditorCommentAnchor { range, edge })
            });
            if let Some(anchor) = anchor {
                self.preview
                    .comment_anchors
                    .entry(path.to_string())
                    .or_default()
                    .insert(comment.id, anchor);
            }
        }
    }

    pub(super) fn sync_editor_comment_lines(
        &mut self,
        path: &str,
        editor: &Entity<super::super::editor::FileEditorState>,
        cx: &mut Context<Self>,
    ) {
        let Some(anchors) = self.preview.comment_anchors.get(path) else {
            return;
        };
        let (updates, detached) = editor.read_with(cx, |state, cx| {
            let mut updates = Vec::new();
            let mut detached = Vec::new();
            for (id, anchor) in anchors {
                if let Some(range) = anchor.range.get_ranges(cx).into_iter().next() {
                    updates.push((
                        id.clone(),
                        tracked_comment_line(state.text(), &range, anchor.edge),
                    ));
                } else {
                    detached.push(id.clone());
                }
            }
            (updates, detached)
        });
        if !detached.is_empty() {
            if let Some(anchors) = self.preview.comment_anchors.get_mut(path) {
                anchors.retain(|id, _| !detached.contains(id));
            }
            self.sync_editor_comment_anchors(path, editor, cx);
        }
        if let Some(draft) = self.preview.comment_draft.as_mut()
            && draft.path == path
            && let Some((_, line)) = updates
                .iter()
                .find(|(id, _)| draft.editing_id.as_ref() == Some(id))
        {
            draft.line = *line;
        }
        if !updates.is_empty() {
            let key = self.chat_id.clone();
            self.state.update(cx, |state, cx| {
                for (id, line) in updates {
                    state.update_review_comment_line(&key, &id, line);
                }
                cx.notify();
            });
        }
    }

    pub(super) fn open_editor_comment_draft(
        &mut self,
        path: String,
        line: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let placeholder = if self
            .preview
            .documents
            .get(&path)
            .is_some_and(|d| d.show_markdown)
        {
            "Request a change…"
        } else {
            "Add a comment…"
        };
        let input = cx.new(|cx| {
            ComposerInput::new(placeholder, cx)
                .with_text_metrics(12.0, crate::composer::INPUT_LINE_HEIGHT)
        });
        let events = cx.subscribe(&input, |this: &mut Self, _, event, cx| match event {
            ComposerInputEvent::Submitted => this.commit_editor_comment(cx),
            ComposerInputEvent::Edited => cx.notify(),
            _ => {}
        });
        let focus = input.read(cx).focus_handle(cx);
        self.preview.comment_draft = Some(EditorCommentDraft {
            editing_id: None,
            key: self.chat_id.clone(),
            path,
            line,
            input,
            _events: events,
        });
        self.preview.active_comment = None;
        window.focus(&focus, cx);
        cx.notify();
    }

    pub(crate) fn edit_editor_comment(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(comment) = self
            .state
            .read(cx)
            .review_comments(&self.chat_id)
            .iter()
            .find(|comment| comment.id == id && comment.is_file())
            .cloned()
        else {
            return;
        };
        self.open_editor_comment_draft(comment.path, comment.line, window, cx);
        let draft = self.preview.comment_draft.as_mut().unwrap();
        draft.editing_id = Some(comment.id);
        draft
            .input
            .update(cx, |input, cx| input.set_text(comment.body, cx));
        cx.notify();
    }

    pub(crate) fn cancel_editor_comment(&mut self, cx: &mut Context<Self>) {
        if let Some(draft) = self.preview.comment_draft.take() {
            self.preview.active_comment = draft.editing_id;
        }
        self.trim_document_cache(cx);
        cx.notify();
    }

    pub(crate) fn commit_editor_comment(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.preview.comment_draft.take() else {
            return;
        };
        let body = draft.input.read(cx).text().trim().to_string();
        if body.is_empty() {
            self.preview.active_comment = draft.editing_id;
            cx.notify();
            return;
        }
        if let Some(id) = draft.editing_id {
            self.state.update(cx, |state, cx| {
                state.update_review_comment_body(&draft.key, &id, body);
                cx.notify();
            });
            self.preview.active_comment = Some(id);
            self.trim_document_cache(cx);
            cx.notify();
            return;
        }
        let comment = ReviewComment::file(draft.path.clone(), draft.line, body);
        self.state.update(cx, |state, cx| {
            state.add_review_comment(&draft.key, comment);
            cx.notify();
        });
        if let Some(editor) = self
            .preview
            .documents
            .get(&draft.path)
            .and_then(|document| document.editor.clone())
        {
            self.sync_editor_comment_anchors(&draft.path, &editor, cx);
        }
        self.preview.active_comment = None;
        self.require_review_comment_flush(&draft.path, cx);
        if self.preview.autosave_enabled
            && self
                .preview
                .documents
                .get(&draft.path)
                .is_some_and(FileDocument::can_autosave)
        {
            self.save_document(draft.path, cx);
        }
        cx.notify();
    }

    pub(crate) fn remove_editor_comment(&mut self, id: &str, cx: &mut Context<Self>) {
        let key = self.chat_id.clone();
        let removed_path = self
            .state
            .read(cx)
            .review_comments(&key)
            .iter()
            .find(|comment| comment.id == id && comment.is_file())
            .map(|comment| comment.path.clone());
        self.state.update(cx, |state, cx| {
            state.remove_review_comment(&key, id);
            cx.notify();
        });
        for anchors in self.preview.comment_anchors.values_mut() {
            anchors.remove(id);
        }
        if self.preview.active_comment.as_deref() == Some(id) {
            self.preview.active_comment = None;
        }
        if let Some(path) = removed_path
            && self
                .state
                .read(cx)
                .review_comments(&key)
                .iter()
                .all(|comment| !comment.is_file() || comment.path != path)
            && let Some(document) = self.preview.documents.get_mut(&path)
        {
            document.review_comment_flush_pending = false;
        }
        self.finish_review_comment_flush_if_idle(cx);
        self.trim_document_cache(cx);
        cx.notify();
    }

    pub(crate) fn open_markdown_comment(
        &mut self,
        path: String,
        line: u32,
        source: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self
            .preview
            .documents
            .get(&path)
            .and_then(|d| d.editor.as_ref())
        else {
            return;
        };
        if !editor.read(cx).context_menu_capabilities().is_editable()
            || editor.read(cx).value().as_ref() != source
        {
            return;
        }
        self.open_editor_comment_draft(path, line, window, cx);
    }

    pub(super) fn toggle_editor_comment(&mut self, id: String, cx: &mut Context<Self>) {
        if self.preview.active_comment.as_deref() == Some(id.as_str()) {
            self.preview.active_comment = None;
        } else {
            self.preview.active_comment = Some(id);
            self.preview.comment_draft = None;
        }
        cx.notify();
    }
}
