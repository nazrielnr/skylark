//! Document lifecycle, reconciliation, and external-change handling.

use super::*;

impl FilesSurface {
    pub fn has_unsaved_changes(&self) -> bool {
        self.preview.has_unsaved_changes()
    }

    pub fn prepare_close(&mut self, cx: &mut Context<Self>) -> FilesCloseDisposition {
        let dirty_paths = self.preview.dirty_paths();
        if dirty_paths.is_empty() {
            self.suspend_images(cx);
            return FilesCloseDisposition::Allow;
        }
        self.preview.close_requested = true;
        let blocked = dirty_paths.iter().any(|path| {
            self.preview
                .documents
                .get(path)
                .is_some_and(document_blocks_lifecycle)
        });
        for path in dirty_paths {
            if self
                .preview
                .documents
                .get(&path)
                .is_some_and(FileDocument::can_autosave)
            {
                self.save_document(path, cx);
            }
        }
        cx.notify();
        if blocked {
            FilesCloseDisposition::Blocked
        } else {
            FilesCloseDisposition::Pending
        }
    }

    pub(crate) fn retry_pending_close(&mut self, cx: &mut Context<Self>) {
        let paths = self.preview.dirty_paths();
        for path in paths {
            if self
                .preview
                .documents
                .get(&path)
                .is_some_and(FileDocument::can_save)
            {
                self.save_document(path, cx);
            }
        }
    }

    pub(crate) fn keep_open(&mut self, cx: &mut Context<Self>) {
        self.preview.close_requested = false;
        cx.emit(FilesEvent::CloseCancelled);
        cx.notify();
    }

    pub(crate) fn discard_changes_and_close(&mut self, cx: &mut Context<Self>) {
        let closing = self.preview.close_requested;
        for document in self.preview.documents.values_mut() {
            document.discard_changes();
        }
        self.cancel_review_comment_flush(cx);
        self.preview.close_requested = false;
        if closing {
            cx.emit(FilesEvent::CloseReady);
        } else if self.target_change_pending {
            self.apply_pending_target(cx);
        } else {
            cx.emit(FilesEvent::CloseReady);
        }
        cx.emit(FilesEvent::TitleChanged);
        cx.notify();
    }

    pub(crate) fn finish_pending_lifecycle(&mut self, cx: &mut Context<Self>) {
        if self.preview.close_requested && !self.preview.has_unsaved_changes() {
            self.preview.close_requested = false;
            cx.emit(FilesEvent::CloseReady);
        }
        if self.target_change_pending && !self.preview.has_unsaved_changes() {
            self.apply_pending_target(cx);
        }
    }

    pub(crate) fn reconcile_document(&mut self, path: String, cx: &mut Context<Self>) {
        if super::super::image_preview::is_image(&path)
            && !self.preview.documents.get(&path).is_some_and(|d| {
                d.is_dirty() || d.pending_save.is_some() || matches!(d.phase, DocumentPhase::Saving)
            })
        {
            if self.preview.documents.contains_key(&path) {
                if self.preview.images_visible && self.preview.active.as_deref() == Some(&path) {
                    self.read_image_file(path, cx);
                } else if let Some(view) = self
                    .preview
                    .documents
                    .get(&path)
                    .and_then(|d| d.image.clone())
                {
                    view.update(cx, |view, cx| view.suspend(cx));
                }
            }
            return;
        }
        let Some(context) = self.request_context.clone() else {
            return;
        };
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let Some(document) = self.preview.documents.get_mut(&path) else {
            return;
        };
        if matches!(document.phase, DocumentPhase::Saving) {
            document.reconcile_after_save = true;
            return;
        }
        let key = document.key.clone();
        let generation = document.generation;
        let request = ReadWorkspaceFileRequest {
            target: context.target.clone(),
            path: path.clone(),
        };
        let client = WorkspaceFilesClient::new(engine, context.clone());
        let task_path = path.clone();
        let task_key = key.clone();
        let task = cx.spawn(async move |this, cx| {
            let started_at = Instant::now();
            let result = client.read_file(request).await;
            tracing::trace!(
                path = %task_path,
                elapsed_ms = started_at.elapsed().as_millis(),
                success = result.is_ok(),
                error = ?result.as_ref().err(),
                "workspace document reconciliation read completed"
            );
            let _ = this.update(cx, |surface, cx| {
                if surface.request_context.as_ref() != Some(&context) {
                    return;
                }
                let Some(document) = surface.preview.documents.get_mut(&task_path) else {
                    return;
                };
                if !document.accepts(&task_key, generation) {
                    return;
                }
                if matches!(document.phase, DocumentPhase::Saving) {
                    document.reconcile_after_save = true;
                    return;
                }
                document.reconcile_task = None;
                let Ok(file) = result else {
                    return;
                };
                if file.content_hash == document.saved_hash {
                    let recovered = if matches!(document.phase, DocumentPhase::DeletedOnDisk) {
                        document.restore_on_disk(file);
                        true
                    } else if matches!(document.phase, DocumentPhase::ExternallyModified { .. }) {
                        document.phase = DocumentPhase::Ready;
                        true
                    } else {
                        false
                    };
                    if document.can_autosave() {
                        surface.schedule_autosave(task_path.clone(), cx);
                    }
                    if recovered {
                        cx.notify();
                    }
                    return;
                }
                if document.is_dirty() {
                    document.mark_external(file.content_hash);
                } else {
                    document.queue_external_reload(file);
                }
                cx.notify();
            });
        });
        if let Some(document) = self.preview.documents.get_mut(&path)
            && document.accepts(&key, generation)
        {
            document.reconcile_task = Some(task);
        }
    }

    pub(crate) fn reconcile_open_documents(&mut self, cx: &mut Context<Self>) {
        let paths = self.preview.documents.keys().cloned().collect::<Vec<_>>();
        for path in paths {
            self.reconcile_document(path, cx);
        }
    }

    pub(crate) fn reconcile_created_documents(&mut self, path: &str, cx: &mut Context<Self>) {
        let paths = self
            .preview
            .documents
            .keys()
            .filter(|document_path| path_is_same_or_descendant(document_path, path))
            .cloned()
            .collect::<Vec<_>>();
        for path in paths {
            self.reconcile_document(path, cx);
        }
    }

    pub(crate) fn invalidate_markdown_images(
        &mut self,
        path: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        for document in self.preview.documents.values() {
            if let Some(view) = &document.markdown {
                view.update(cx, |view, cx| view.invalidate_images(path, cx));
            }
        }
    }

    pub(crate) fn mark_document_deleted(&mut self, path: &str, cx: &mut Context<Self>) {
        let mut changed = false;
        for (document_path, document) in &mut self.preview.documents {
            if path_is_same_or_descendant(document_path, path) {
                document.mark_deleted();
                if let Some(view) = &document.image {
                    view.update(cx, |view, cx| view.deleted(cx));
                }
                changed = true;
            }
        }
        if changed {
            cx.notify();
        }
    }

    pub(crate) fn rename_documents(
        &mut self,
        old_path: &str,
        new_path: String,
        cx: &mut Context<Self>,
    ) -> Vec<(String, String)> {
        let renames = self
            .preview
            .documents
            .keys()
            .filter_map(|path| {
                renamed_document_path(path, old_path, &new_path)
                    .map(|renamed| (path.clone(), renamed))
            })
            .collect::<Vec<_>>();
        let image_renames: HashSet<_> = renames
            .iter()
            .filter(|(old, new)| {
                super::super::image_preview::is_image(old)
                    || super::super::image_preview::is_image(new)
            })
            .map(|(_, new)| new.clone())
            .collect();
        for (old_document_path, new_document_path) in &renames {
            let Some(mut document) = self.preview.documents.remove(old_document_path) else {
                continue;
            };
            if let Some(view) = document.image.take() {
                view.update(cx, |view, cx| view.suspend(cx));
            }
            document.generation = document.generation.wrapping_add(1);
            let needs_review =
                document.is_dirty() || matches!(document.phase, DocumentPhase::Saving);
            document.read_task = None;
            document.highlight_task = None;
            document.autosave_task = None;
            document.save_task = None;
            document.reconcile_task = None;
            document.pending_save = None;
            document.key.path = new_document_path.clone();
            if let Some(file) = document.file.as_mut() {
                file.path = new_document_path.clone();
            }
            if let Some(editor) = document.editor.clone() {
                let event_path = new_document_path.clone();
                document.editor_events = Some(super::super::editor::subscribe_to_changes(
                    &editor, event_path, cx,
                ));
            }
            if needs_review {
                document.mark_external(None);
            }
            if self.preview.active.as_deref() == Some(old_document_path) {
                self.preview.active = Some(new_document_path.clone());
            }
            if self.editor_path.as_deref() == Some(old_document_path) {
                self.editor_path = Some(new_document_path.clone());
            }
            if let Some(highlight) = self.preview.highlights.remove(old_document_path) {
                self.preview
                    .highlights
                    .insert(new_document_path.clone(), highlight);
            }
            if let Some(anchors) = self.preview.comment_anchors.remove(old_document_path) {
                self.preview
                    .comment_anchors
                    .insert(new_document_path.clone(), anchors);
            }
            if let Some(draft) = self.preview.comment_draft.as_mut()
                && draft.path == *old_document_path
            {
                draft.path = new_document_path.clone();
            }
            self.preview.document_recency.retain(|recent_path| {
                recent_path != old_document_path && recent_path != new_document_path
            });
            self.preview
                .document_recency
                .push_back(new_document_path.clone());
            let key = self.chat_id.clone();
            self.state.update(cx, |state, _| {
                state.rename_review_comment_path(&key, old_document_path, new_document_path);
            });
            self.preview
                .documents
                .insert(new_document_path.clone(), document);
        }
        for (_, path) in &renames {
            if image_renames.contains(path)
                && self.preview.active.as_deref() == Some(path)
                && !self
                    .preview
                    .documents
                    .get(path)
                    .is_some_and(FileDocument::is_dirty)
            {
                self.read_file(path.clone(), cx);
            }
        }
        if !renames.is_empty() {
            cx.notify();
        }
        renames
    }

    pub(crate) fn apply_pending_external_reload(
        &mut self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(file) = self
            .preview
            .documents
            .get_mut(path)
            .and_then(|document| document.pending_external_reload.take())
        else {
            return;
        };
        let text = file.text.clone();
        let hash = file.content_hash.clone();
        let editor = self
            .preview
            .documents
            .get(path)
            .and_then(|document| document.editor.clone());
        if let (Some(editor), Some(text)) = (editor, text.clone()) {
            super::super::editor::replace_file_contents(&editor, text, window, cx);
        }
        if let Some(document) = self.preview.documents.get_mut(path) {
            document.apply_external_reload(file);
        }
        self.sync_preview_list();
        if let (Some(text), Some(hash)) = (text, hash) {
            self.request_file_highlight(path.to_string(), text, hash, cx);
        }
    }

    pub(crate) fn sync_preview_list(&self) {
        let count = self
            .preview
            .active
            .as_deref()
            .and_then(|path| self.preview.documents.get(path))
            .map(|document| document.lines.len())
            .unwrap_or(0);
        self.preview
            .list
            .reset_with_uniform_height(count, self.preview.line_height());
    }

    pub(crate) fn request_reload_active_document(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.preview.active.clone() else {
            return;
        };
        match self.preview.request_reload(&path) {
            ReloadDecision::ReloadNow => self.read_file(path, cx),
            ReloadDecision::AwaitDiscardConfirmation => cx.notify(),
        }
    }

    pub(crate) fn confirm_reload_active_document(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.preview.active.clone() else {
            return;
        };
        if self.preview.reload_confirmation.as_deref() != Some(path.as_str()) {
            return;
        }
        self.preview.reload_confirmation = None;
        self.read_file(path, cx);
    }

    pub(crate) fn cancel_reload_confirmation(&mut self, cx: &mut Context<Self>) {
        let pending = self.preview.reload_confirmation.take();
        if let Some(path) = pending
            && self
                .preview
                .documents
                .get(&path)
                .is_some_and(FileDocument::can_autosave)
        {
            self.schedule_autosave(path, cx);
        }
        cx.notify();
    }

    pub(crate) fn keep_external_edits(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.preview.active.clone() else {
            return;
        };
        if let Some(document) = self.preview.documents.get_mut(&path)
            && let DocumentPhase::ExternallyModified { disk_hash } = &document.phase
        {
            document.phase = DocumentPhase::Conflict {
                disk_hash: disk_hash.clone(),
            };
        }
        self.preview.reload_confirmation = None;
        cx.notify();
    }

    pub fn save_active_document(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.preview.active.clone() {
            self.save_document(path, cx);
        }
    }
}
