//! Workspace document loading, highlighting, and saving.

use super::*;

impl FilesSurface {
    pub(crate) fn open_file(&mut self, path: String, cx: &mut Context<Self>) {
        if self
            .preview
            .active
            .as_ref()
            .is_some_and(|active| active != &path)
        {
            self.suspend_images(cx);
            if let Some(view) = self
                .preview
                .active
                .as_ref()
                .and_then(|active| self.preview.documents.get(active))
                .and_then(|d| d.markdown.clone())
            {
                view.update(cx, |view, cx| view.suspend(cx));
            }
        }

        let leaving_reload_confirmation = self
            .preview
            .reload_confirmation
            .as_deref()
            .is_some_and(|pending| pending != path);
        if leaving_reload_confirmation {
            self.cancel_reload_confirmation(cx);
        }
        self.preview.active = Some(path.clone());
        self.preview.touch_document(&path);
        self.preview.show_tree_sidebar();
        if !self.preview.documents.contains_key(&path) {
            let Some(context) = self.request_context.as_ref() else {
                return;
            };
            self.preview.documents.insert(
                path.clone(),
                FileDocument::loading(document_key(context, path.clone())),
            );
            self.read_file(path, cx);
        } else if self.preview.documents.get(&path).is_some_and(|d| {
            d.image.is_none()
                && (super::super::image_preview::is_image(&path)
                    || (d.file.is_none() && d.read_task.is_none()))
        }) {
            self.read_file(path, cx);
        } else {
            self.sync_preview_list();
        }
        self.trim_document_cache(cx);
        cx.notify();
    }

    pub(super) fn read_file(&mut self, path: String, cx: &mut Context<Self>) {
        if super::super::image_preview::is_image(&path) {
            self.read_image_file(path, cx);
            return;
        }
        let Some(context) = self.request_context.clone() else {
            return;
        };
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            if let Some(document) = self.preview.documents.get_mut(&path) {
                document.phase =
                    DocumentPhase::Error("Workspace service is still starting.".into());
            }
            return;
        };
        let Some(document) = self.preview.documents.get_mut(&path) else {
            return;
        };
        let key = document_key(&context, path.clone());
        document.key = key.clone();
        let generation = document.begin_load();
        let request = ReadWorkspaceFileRequest {
            target: context.target.clone(),
            path: path.clone(),
        };
        let client = WorkspaceFilesClient::new(engine, context.clone());
        let task_path = path.clone();
        let task_key = key.clone();
        let task = cx.spawn(async move |this, cx| {
            let mut result = client.read_file(request.clone()).await;
            if result.as_ref().is_err_and(|error| error.retryable()) {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(250))
                    .await;
                result = client.read_file(request).await;
            }
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
                document.read_task = None;
                match result {
                    Ok(file) => {
                        let highlight = file
                            .text
                            .as_ref()
                            .zip(file.content_hash.as_ref())
                            .map(|(source, hash)| (source.clone(), hash.clone()));
                        document.set_loaded(file);
                        surface.sync_preview_list();
                        if let Some((source, hash)) = highlight {
                            surface.request_file_highlight(task_path.clone(), source, hash, cx);
                        }
                    }
                    Err(error) => {
                        tracing::warn!(path = %task_path, error = %error, "workspace file load failed");
                        document.set_error(error.to_string());
                        surface.sync_preview_list();
                    }
                }
                surface.trim_document_cache(cx);
                cx.notify();
            });
        });
        if let Some(document) = self.preview.documents.get_mut(&path)
            && document.accepts(&key, generation)
        {
            document.read_task = Some(task);
        }
        self.sync_preview_list();
        cx.notify();
    }

    pub(super) fn read_image_file(&mut self, path: String, cx: &mut Context<Self>) {
        // A rename can give an edited text buffer an image extension. Never
        // discard that buffer (or an in-flight save) to create a preview.
        if self.preview.documents.get(&path).is_some_and(|d| {
            d.is_dirty() || d.pending_save.is_some() || matches!(d.phase, DocumentPhase::Saving)
        }) {
            return;
        }
        let Some(context) = self.request_context.clone() else {
            return;
        };
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            if let Some(document) = self.preview.documents.get_mut(&path) {
                document.set_error("Workspace service is still starting.");
            }
            return;
        };
        let client = WorkspaceFilesClient::new(engine, context.clone());
        let view = cx.new(|cx| {
            super::super::image_preview::ImagePreview::new(path.clone(), context, client, cx)
        });
        if !self.preview.images_visible {
            view.update(cx, |view, cx| view.suspend(cx));
        }
        if let Some(document) = self.preview.documents.get_mut(&path) {
            document.begin_load();
            document.image = Some(view);
            document.file = None;
            document.editor = None;
            document.editor_events = None;
            document.editor_observer = None;
            document.lines = Arc::new(Vec::new());
            document.phase = DocumentPhase::ReadOnly(WorkspaceReadOnlyReason::Binary);
        }
        cx.notify();
    }

    pub(super) fn request_file_highlight(
        &mut self,
        path: String,
        source: String,
        content_hash: String,
        cx: &mut Context<Self>,
    ) {
        let Some(language) = zeron_syntax::language_for_path(&path) else {
            return;
        };
        let Some((document_key, generation, revision)) = self
            .preview
            .documents
            .get(&path)
            .map(|document| (document.key.clone(), document.generation, document.revision))
        else {
            return;
        };
        let key = DocumentHighlightKey::new(language, &source);
        if let Some(document) = self.preview.syntax_cache.get(&key) {
            if let Some(editor) = self
                .preview
                .documents
                .get(&path)
                .and_then(|file| file.editor.clone())
            {
                super::super::editor_adapter::install_highlighter(
                    &editor,
                    source,
                    document.clone(),
                    cx,
                );
            }
            self.preview.highlights.insert(
                path,
                HighlightedFile {
                    content_hash,
                    document,
                },
            );
            cx.notify();
            return;
        }
        let highlight_path = path.clone();
        let task_document_key = document_key.clone();
        let source_for_install = source.clone();
        let task = cx.spawn(async move |this, cx| {
            let request_path = highlight_path.clone();
            let highlighted = cx
                .background_executor()
                .spawn(async move {
                    zeron_syntax::highlight(zeron_syntax::HighlightRequest {
                        source: &source,
                        path: Some(&request_path),
                        fence_tag: None,
                    })
                    .ok()
                    .map(Arc::new)
                })
                .await;
            let _ = this.update(cx, |surface, cx| {
                let still_current = surface
                    .preview
                    .documents
                    .get_mut(&highlight_path)
                    .is_some_and(|document| {
                        let current = file_highlight_result_is_current(
                            document,
                            &task_document_key,
                            generation,
                            revision,
                            &content_hash,
                        );
                        if current {
                            document.highlight_task = None;
                        }
                        current
                    });
                if !still_current {
                    return;
                }
                if let Some(document) = highlighted {
                    surface.preview.syntax_cache.insert(key, document.clone());
                    if let Some(editor) = surface
                        .preview
                        .documents
                        .get(&highlight_path)
                        .and_then(|file| file.editor.clone())
                    {
                        super::super::editor_adapter::install_highlighter(
                            &editor,
                            source_for_install,
                            document.clone(),
                            cx,
                        );
                    }
                    surface.preview.highlights.insert(
                        highlight_path.clone(),
                        HighlightedFile {
                            content_hash,
                            document,
                        },
                    );
                    cx.notify();
                }
                surface.trim_document_cache(cx);
            });
        });
        if let Some(document) = self.preview.documents.get_mut(&path)
            && document.accepts(&document_key, generation)
        {
            document.highlight_task = Some(task);
        }
    }

    pub(crate) fn on_editor_change(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(editor) = self
            .preview
            .documents
            .get(path)
            .and_then(|document| document.editor.clone())
        else {
            return;
        };
        let source = editor.read(cx).value().to_string();
        let revision = {
            let Some(document) = self.preview.documents.get_mut(path) else {
                return;
            };
            document.mark_user_edit();
            document.revision
        };
        self.sync_editor_comment_lines(path, &editor, cx);
        if !self.staged_file_comments(path, cx).is_empty() {
            self.require_review_comment_flush(path, cx);
        }
        self.request_editor_highlight(path.to_string(), source, revision, cx);
        self.schedule_autosave(path.to_string(), cx);
        cx.emit(FilesEvent::TitleChanged);
        cx.notify();
    }

    fn request_editor_highlight(
        &mut self,
        path: String,
        source: String,
        revision: u64,
        cx: &mut Context<Self>,
    ) {
        let Some(language) = zeron_syntax::language_for_path(&path) else {
            return;
        };
        let Some((document_key, generation)) = self
            .preview
            .documents
            .get(&path)
            .map(|document| (document.key.clone(), document.generation))
        else {
            return;
        };
        let cache_key = DocumentHighlightKey::new(language, &source);
        let task_path = path.clone();
        let task_document_key = document_key.clone();
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(120))
                .await;
            let request_path = task_path.clone();
            let source_for_parse = source.clone();
            let highlighted = cx
                .background_executor()
                .spawn(async move {
                    zeron_syntax::highlight(zeron_syntax::HighlightRequest {
                        source: &source_for_parse,
                        path: Some(&request_path),
                        fence_tag: None,
                    })
                    .ok()
                    .map(Arc::new)
                })
                .await;
            let _ = this.update(cx, |surface, cx| {
                let Some(document) = surface.preview.documents.get_mut(&task_path) else {
                    return;
                };
                if !document.accepts(&task_document_key, generation)
                    || document.revision != revision
                {
                    return;
                }
                document.highlight_task = None;
                let Some(highlighted) = highlighted else {
                    surface.trim_document_cache(cx);
                    return;
                };
                let editor = document.editor.clone();
                surface
                    .preview
                    .syntax_cache
                    .insert(cache_key, highlighted.clone());
                if let Some(editor) = editor {
                    super::super::editor_adapter::install_highlighter(
                        &editor,
                        source,
                        highlighted,
                        cx,
                    );
                }
                surface.trim_document_cache(cx);
                cx.notify();
            });
        });
        if let Some(document) = self.preview.documents.get_mut(&path)
            && document.accepts(&document_key, generation)
        {
            document.highlight_task = Some(task);
        }
    }

    pub(crate) fn schedule_autosave(&mut self, path: String, cx: &mut Context<Self>) {
        if !self.preview.autosave_enabled || self.preview.autosave_paused_for_reload(&path) {
            return;
        }
        let delay = Duration::from_millis(self.preview.autosave_delay_ms);
        let Some((key, generation, revision)) = self
            .preview
            .documents
            .get(&path)
            .filter(|document| document.can_autosave())
            .map(|document| (document.key.clone(), document.generation, document.revision))
        else {
            return;
        };
        let task_path = path.clone();
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, |surface, cx| {
                let still_current = surface.preview.autosave_enabled
                    && !surface.preview.autosave_paused_for_reload(&task_path)
                    && surface
                        .preview
                        .documents
                        .get(&task_path)
                        .is_some_and(|document| {
                            document.accepts(&key, generation)
                                && document.revision == revision
                                && document.can_autosave()
                        });
                if still_current {
                    surface.save_document(task_path, cx);
                }
            });
        });
        if let Some(document) = self.preview.documents.get_mut(&path) {
            document.autosave_task = Some(task);
        }
    }

    pub(super) fn save_document(&mut self, path: String, cx: &mut Context<Self>) {
        if self.target_change_pending {
            return;
        }
        let Some(context) = self.request_context.clone() else {
            return;
        };
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            if let Some(document) = self.preview.documents.get_mut(&path) {
                document.autosave_task = None;
                document.phase =
                    DocumentPhase::SaveFailed("Workspace service is unavailable.".into());
            }
            cx.notify();
            return;
        };
        let Some(editor) = self
            .preview
            .documents
            .get(&path)
            .and_then(|document| document.editor.clone())
        else {
            return;
        };
        let text = editor.read(cx).value().to_string();
        let Some((key, generation, pending)) =
            self.preview.documents.get_mut(&path).and_then(|document| {
                let key = document.key.clone();
                let generation = document.generation;
                document
                    .begin_save(text)
                    .map(|pending| (key, generation, pending))
            })
        else {
            return;
        };
        let request = WriteWorkspaceFileRequest {
            expected_checkout_id: pending.expected_checkout_id.clone(),
            target: context.target.clone(),
            path: path.clone(),
            text: pending.text.clone(),
            expected_content_hash: pending.expected_content_hash.clone(),
            encoding: pending.encoding,
            line_ending: pending.line_ending,
        };
        let revision = pending.revision;
        let task_path = path.clone();
        let task_key = key.clone();
        let client = WorkspaceFilesClient::new(engine, context.clone());
        let task = cx.spawn(async move |this, cx| {
            let result = client.write_file(request).await;
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
                match result {
                    Ok(WriteWorkspaceFileOutcome::Written { file }) => {
                        let hash = file.content_hash.clone();
                        if document.finish_save(revision, hash.clone())
                            && let Some(loaded) = document.file.as_mut()
                        {
                            loaded.content_hash = Some(hash);
                            loaded.size = file.size;
                            loaded.modified_at = file.modified_at;
                        }
                    }
                    Ok(WriteWorkspaceFileOutcome::Conflict {
                        current_content_hash,
                        ..
                    }) => {
                        tracing::warn!(path = %task_path, "workspace file save conflicted");
                        document.conflict_save(revision, current_content_hash);
                    }
                    Err(error) => {
                        tracing::warn!(path = %task_path, error = %error, "workspace file save failed");
                        document.fail_save(revision, error.to_string());
                    }
                }
                let save_again = document.can_autosave();
                let reconcile = document.reconcile_after_save;
                document.reconcile_after_save = false;
                if document.review_comment_flush_pending
                    && document_finishes_review_comment_flush(document)
                {
                    document.review_comment_flush_pending = false;
                }
                if save_again {
                    if surface.preview.close_requested || surface.target_change_pending {
                        surface.save_document(task_path.clone(), cx);
                    } else {
                        surface.schedule_autosave(task_path.clone(), cx);
                    }
                }
                if reconcile {
                    surface.reconcile_document(task_path.clone(), cx);
                }
                surface.finish_review_comment_flush_if_idle(cx);
                surface.finish_pending_lifecycle(cx);
                surface.trim_document_cache(cx);
                cx.emit(FilesEvent::TitleChanged);
                cx.notify();
            });
        });
        if let Some(document) = self.preview.documents.get_mut(&path)
            && document.accepts(&key, generation)
        {
            document.save_task = Some(task);
        }
        cx.notify();
    }
}
