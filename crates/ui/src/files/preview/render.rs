//! File preview and editor rendering.

use super::*;

impl FilesSurface {
    pub(crate) fn render_preview(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let typography_generation = crate::typography::generation(cx);
        if self.preview.typography_generation != typography_generation {
            self.preview.typography_generation = typography_generation;
            // Row heights are cached per item, including virtualized rows off
            // screen, so a code-size change leaves stale geometry behind.
            // `remeasure` re-derives them while holding the scroll position.
            self.preview.list.remeasure();
        }
        let Some(active) = self.preview.active.clone() else {
            return gpui::Empty.into_any_element();
        };
        let external = self.preview.documents.get(&active).is_some_and(|document| {
            matches!(
                document.phase,
                DocumentPhase::ExternallyModified { .. } | DocumentPhase::Conflict { .. }
            )
        });
        let confirming_reload = self.preview.reload_confirmation.as_deref() == Some(&active);
        let lifecycle_pending = self.preview.close_requested || self.target_change_pending;
        let lifecycle_blocked = lifecycle_pending
            && self
                .preview
                .documents
                .values()
                .any(document_blocks_lifecycle);
        let body = self.render_document_body(&active, &theme, window, cx);
        div()
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .when(lifecycle_pending, |element| {
                element.child(
                    div()
                        .min_h(px(32.0))
                        .py(px(6.0))
                        .flex_none()
                        .px(px(10.0))
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(8.0))
                        .border_b_1()
                        .border_color(theme.warning.opacity(0.25))
                        .bg(theme.warning.opacity(0.055))
                        .text_size(px(11.0))
                        .text_color(theme.warning_muted)
                        .child(div().min_w_0().max_w_full().whitespace_normal().child(
                            if self.target_change_pending {
                                "Workspace changed. Switch back to save, or discard these edits."
                            } else if lifecycle_blocked {
                                "Changes could not be saved safely."
                            } else {
                                "Saving changes before closing…"
                            },
                        ))
                        .when(lifecycle_blocked || self.target_change_pending, |banner| {
                            banner
                                .child(
                                    div()
                                        .id("files-retry-close-save")
                                        .when(self.target_change_pending, |button| button.hidden())
                                        .ml_auto()
                                        .cursor_pointer()
                                        .text_color(theme.text)
                                        .child("Retry")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.retry_pending_close(cx)
                                        })),
                                )
                                .when(!self.target_change_pending, |banner| {
                                    banner.child(
                                        div()
                                            .id("files-keep-open")
                                            .cursor_pointer()
                                            .text_color(theme.text_muted)
                                            .child("Keep Open")
                                            .on_click(
                                                cx.listener(|this, _, _, cx| this.keep_open(cx)),
                                            ),
                                    )
                                })
                                .child(
                                    div()
                                        .id("files-discard-close-changes")
                                        .cursor_pointer()
                                        .text_color(theme.danger_muted)
                                        .child("Discard Changes")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.discard_changes_and_close(cx)
                                        })),
                                )
                        }),
                )
            })
            .when(
                (external || confirming_reload) && !lifecycle_pending,
                |element| {
                    element.child(
                        div()
                            .min_h(px(32.0))
                            .py(px(6.0))
                            .flex_none()
                            .px(px(10.0))
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(8.0))
                            .border_b_1()
                            .border_color(theme.warning.opacity(0.25))
                            .bg(theme.warning.opacity(0.055))
                            .text_size(px(11.0))
                            .text_color(theme.warning_muted)
                            .child(div().min_w_0().max_w_full().whitespace_normal().child(
                                if confirming_reload {
                                    "Discard unsaved changes?"
                                } else {
                                    "This file changed outside Zeron."
                                },
                            ))
                            .child(
                                div()
                                    .ml_auto()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(px(8.0))
                                    .when(!confirming_reload, |actions| {
                                        actions
                                            .child(
                                                div()
                                                    .id("files-keep-external-edits")
                                                    .cursor_pointer()
                                                    .text_color(theme.text_muted)
                                                    .child("Keep Editing")
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.keep_external_edits(cx)
                                                    })),
                                            )
                                            .child(
                                                div()
                                                    .id("files-reload-external")
                                                    .cursor_pointer()
                                                    .text_color(theme.text)
                                                    .child("Reload from Disk")
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.request_reload_active_document(cx)
                                                    })),
                                            )
                                    })
                                    .when(confirming_reload, |actions| {
                                        actions
                                            .child(
                                                div()
                                                    .id("files-cancel-reload")
                                                    .cursor_pointer()
                                                    .text_color(theme.text_muted)
                                                    .child("Cancel")
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.cancel_reload_confirmation(cx)
                                                    })),
                                            )
                                            .child(
                                                div()
                                                    .id("files-confirm-reload")
                                                    .cursor_pointer()
                                                    .text_color(theme.text)
                                                    .child("Discard & Reload")
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.confirm_reload_active_document(cx)
                                                    })),
                                            )
                                    }),
                            ),
                    )
                },
            )
            .child(body)
            .into_any_element()
    }

    pub(crate) fn render_tree_toggle(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        toolbar(theme)
            .w(px(
                crate::surface_chrome::CONTROL_SIZE + crate::surface_chrome::EDGE_INSET
            ))
            .pl_0()
            .child(
                toolbar_button(
                    "files-toggle-tree-sidebar",
                    if self.preview.tree_sidebar_visible() {
                        "Hide files sidebar"
                    } else {
                        "Show files sidebar"
                    },
                )
                .on_click(cx.listener(|this, _, window, cx| this.toggle_tree_sidebar(window, cx)))
                .child(
                    icon(icons::SIDEBAR_MINIMALISTIC)
                        .size(px(crate::surface_chrome::ICON_SIZE))
                        .text_color(theme.text_muted),
                ),
            )
            .into_any_element()
    }

    pub(crate) fn render_editor_header(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let path = self.preview.active.clone()?;
        Some(self.render_breadcrumb(&path, theme, cx))
    }

    fn render_breadcrumb(
        &mut self,
        path: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let markdown = super::super::markdown_preview::is_markdown(path);
        let showing_markdown = self
            .preview
            .documents
            .get(path)
            .is_some_and(|d| d.show_markdown);
        let parts = path.split('/').collect::<Vec<_>>();
        let reveal_path = path.to_string();
        let tooltip_path: SharedString = path.to_string().into();
        let can_save = !self.target_change_pending
            && self
                .preview
                .documents
                .get(path)
                .is_some_and(FileDocument::can_save);
        let save_status =
            self.preview
                .documents
                .get(path)
                .and_then(|document| match &document.phase {
                    DocumentPhase::SaveFailed(error) => Some((
                        "Save failed",
                        theme.danger_muted,
                        can_save,
                        Some(error.clone()),
                    )),
                    DocumentPhase::Conflict { .. } => Some((
                        "Save conflict",
                        theme.warning_muted,
                        false,
                        Some(SharedString::from(
                            "The file changed on disk. Your editor buffer was preserved.",
                        )),
                    )),
                    DocumentPhase::DeletedOnDisk => Some((
                        "Deleted on disk",
                        theme.warning_muted,
                        false,
                        Some(SharedString::from(
                            "The file was removed on disk. Your editor buffer was preserved.",
                        )),
                    )),
                    DocumentPhase::ExternallyModified { .. } => Some((
                        "Changed on disk",
                        theme.warning_muted,
                        false,
                        Some(SharedString::from(
                            "The file changed on disk. Review it before saving.",
                        )),
                    )),
                    _ => None,
                });
        let mut crumbs = div()
            .id("files-breadcrumb-path")
            .min_w_0()
            .flex_1()
            .flex()
            .items_center()
            .overflow_hidden();
        for (index, part) in parts.iter().enumerate() {
            if index > 0 {
                crumbs = crumbs.child(
                    div()
                        .mx(px(4.0))
                        .text_size(px(11.0))
                        .text_color(theme.text_faint.opacity(0.65))
                        .child("›"),
                );
            }
            crumbs = crumbs.child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_family(theme.font_sans.clone())
                    .text_size(px(11.0))
                    .text_color(if index + 1 == parts.len() {
                        theme.text_muted
                    } else {
                        theme.text_faint
                    })
                    .child((*part).to_string()),
            );
        }
        crumbs = crumbs
            .tooltip(move |_, cx| {
                cx.new(|_| FileEditorTooltip {
                    text: tooltip_path.clone(),
                })
                .into()
            })
            .tooltip_show_delay(Duration::from_millis(350));
        toolbar(theme)
            .pr(px(crate::surface_chrome::CONTROL_GAP))
            .child(
                crate::file_icons::icon(
                    crate::file_icons::FileIconIdentity::file(path),
                    theme.appearance,
                )
                .size(px(14.0))
                .flex_none(),
            )
            .child(crumbs)
            .when(markdown, |element| {
                element.child(
                    toolbar_button(
                        "files-toggle-markdown",
                        if showing_markdown {
                            "Show Markdown code"
                        } else {
                            "Preview Markdown"
                        },
                    )
                    .when(showing_markdown, |el| el.bg(crate::theme::wash(0.1)))
                    .on_click(cx.listener(|this, _, window, cx| {
                        let Some(path) = this.preview.active.clone() else {
                            return;
                        };
                        if let Some(document) = this.preview.documents.get_mut(&path) {
                            document.show_markdown = !document.show_markdown;
                            if !document.show_markdown {
                                if let Some(view) = &document.markdown {
                                    view.update(cx, |view, cx| view.suspend(cx));
                                }
                            }
                        }
                        if this
                            .preview
                            .documents
                            .get(&path)
                            .is_some_and(|d| !d.show_markdown)
                        {
                            this.focus_editor(window, cx);
                        }
                        cx.notify();
                    }))
                    .child(
                        icon(if showing_markdown {
                            icons::FILE_CODE
                        } else {
                            icons::EYE
                        })
                        .size(px(crate::surface_chrome::ICON_SIZE))
                        .text_color(theme.text_muted),
                    ),
                )
            })
            .when_some(save_status, |element, (label, color, retry, detail)| {
                element.child(
                    div()
                        .id("files-save-status")
                        .h(px(crate::surface_chrome::CONTROL_SIZE))
                        .px(px(6.0))
                        .rounded(px(crate::surface_chrome::CONTROL_RADIUS))
                        .flex()
                        .items_center()
                        .flex_none()
                        .font_family(theme.font_sans.clone())
                        .text_size(px(11.0))
                        .text_color(color)
                        .when(retry, |element| {
                            element
                                .cursor_pointer()
                                .role(gpui::Role::Button)
                                .aria_label("Save file")
                                .hover(|style| style.bg(crate::theme::wash(0.14)))
                                .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
                                    window.prevent_default()
                                })
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.save_active_document(cx)),
                                )
                        })
                        .when_some(detail, |element, detail| {
                            element
                                .tooltip(move |_, cx| {
                                    cx.new(|_| FileEditorTooltip {
                                        text: detail.clone(),
                                    })
                                    .into()
                                })
                                .tooltip_show_delay(Duration::from_millis(350))
                        })
                        .child(label),
                )
            })
            .child(
                toolbar_button("files-reveal-active", "Reveal file in tree")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let name = reveal_path
                            .rsplit('/')
                            .next()
                            .unwrap_or(&reveal_path)
                            .to_string();
                        this.reveal_search_result(
                            WorkspaceFileSearchMatch {
                                path: reveal_path.clone(),
                                name,
                                kind: zeron_proto::WorkspaceEntryKind::File,
                                score: 0,
                            },
                            cx,
                        );
                    }))
                    .child(
                        icon(icons::FOLDER)
                            .size(px(crate::surface_chrome::ICON_SIZE))
                            .text_color(theme.text_muted),
                    ),
            )
            .child(
                toolbar_button(
                    "files-toggle-word-wrap",
                    if self.preview.word_wrap() {
                        "Disable word wrap"
                    } else {
                        "Enable word wrap"
                    },
                )
                .when(self.preview.word_wrap(), |element| {
                    element.bg(crate::theme::wash(0.1))
                })
                .on_click(cx.listener(|this, _, window, cx| this.toggle_word_wrap(window, cx)))
                .child(
                    icon(icons::LIST)
                        .size(px(crate::surface_chrome::ICON_SIZE))
                        .text_color(if self.preview.word_wrap() {
                            theme.text
                        } else {
                            theme.text_muted
                        }),
                ),
            )
            .into_any_element()
    }

    fn markdown_web_link_handler(
        cx: &Context<Self>,
    ) -> super::super::markdown_preview::WebLinkHandler {
        let owner = cx.weak_entity();
        Rc::new(move |activation, cx| {
            let _ = owner.update(cx, |surface, cx| {
                let mut activation = activation.clone();
                activation.source_session = Some(surface.chat_id.clone());
                cx.emit(FilesEvent::OpenWebLink(activation));
            });
        })
    }

    fn prepare_markdown_preview(
        &mut self,
        path: &str,
        editor: Option<&Entity<super::super::editor::FileEditorState>>,
        cx: &mut Context<Self>,
    ) -> Option<Entity<super::super::markdown_preview::MarkdownPreview>> {
        if !self.preview.documents.get(path).is_some_and(|d| {
            d.show_markdown
                && !matches!(d.phase, DocumentPhase::Loading | DocumentPhase::Error(_))
                && d.file.as_ref().is_some_and(|f| f.text.is_some())
        }) {
            return None;
        }
        let document = self.preview.documents.get_mut(path).unwrap();
        let version = (
            document.generation,
            document.revision,
            document.loaded_hash.clone(),
        );
        let view = document
            .markdown
            .get_or_insert_with(|| {
                let owner = cx.weak_entity();
                let web_links = Self::markdown_web_link_handler(cx);
                cx.new(|cx| {
                    let mut view = super::super::markdown_preview::MarkdownPreview::new(
                        path.to_string(),
                        Rc::new(move |path, cx| {
                            let _ =
                                owner.update(cx, |surface, cx| surface.open_tree_file(path, cx));
                        }),
                        cx,
                    );
                    view.set_web_link_handler(web_links);
                    view
                })
            })
            .clone();
        let media_client = self
            .request_context
            .clone()
            .zip(self.state.read(cx).engine().cloned())
            .map(|(context, engine)| {
                (
                    WorkspaceFilesClient::new(engine, context),
                    document.file.as_ref().unwrap().checkout_id.clone(),
                )
            });
        let location = (
            self.request_context
                .as_ref()
                .and_then(|context| context.target_device_id.clone()),
            document.file.as_ref().unwrap().checkout_id.clone(),
        );
        view.update(cx, |view, cx| {
            view.media_client = media_client;
            view.editor = editor.map(|editor| editor.downgrade());
            view.activate(path, &location, cx);
        });
        if view.read(cx).version.as_ref() != Some(&version) {
            let source = editor
                .map(|editor| editor.read(cx).value().to_string())
                .unwrap_or_else(|| {
                    document
                        .file
                        .as_ref()
                        .and_then(|f| f.text.clone())
                        .unwrap_or_default()
                });
            let truncated = document.file.as_ref().is_some_and(|f| f.truncated);
            view.update(cx, |view, cx| {
                view.version = Some(version);
                view.set_source(source, truncated, cx);
            });
        }
        self.sync_markdown_comments(path, cx);
        Some(view)
    }

    pub(crate) fn sync_active_markdown_comments(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.preview.active.clone() {
            self.sync_markdown_comments(&path, cx);
        }
    }

    fn sync_markdown_comments(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(view) = self
            .preview
            .documents
            .get(path)
            .filter(|document| document.show_markdown)
            .and_then(|document| document.markdown.clone())
        else {
            return;
        };
        let comments = self.staged_file_comments(path, cx);
        let draft = self
            .preview
            .comment_draft
            .as_ref()
            .filter(|draft| draft.path == path && draft.key == self.chat_id)
            .map(|draft| (draft.line, draft.input.clone(), draft.editing_id.is_some()));
        let editing_id = self
            .preview
            .comment_draft
            .as_ref()
            .and_then(|draft| draft.editing_id.as_ref());
        let comments = comments
            .into_iter()
            .filter(|comment| Some(&comment.id) != editing_id)
            .collect();
        let owner = cx.weak_entity();
        view.update(cx, |view, cx| view.set_comments(owner, comments, draft, cx));
    }

    fn render_document_body(
        &mut self,
        path: &str,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.target_change_pending && super::super::image_preview::is_image(path) {
            return centered_state(
                "Workspace changed. Image preview suspended.",
                theme.text_muted,
            );
        }
        self.preview.images_visible = true;
        if let Some(view) = self
            .preview
            .documents
            .get(path)
            .and_then(|d| d.image.clone())
        {
            view.update(cx, |view, cx| view.activate(cx));
            return view.into_any_element();
        }
        self.apply_pending_external_reload(path, window, cx);
        let editor = self.ensure_editor(path, theme, window, cx);
        if let Some(editor) = &editor {
            self.sync_editor_comment_anchors(path, editor, cx);
        }
        if let Some(view) = self.prepare_markdown_preview(path, editor.as_ref(), cx) {
            return view.into_any_element();
        }

        let Some(document) = self.preview.documents.get(path) else {
            return gpui::Empty.into_any_element();
        };
        if matches!(document.phase, DocumentPhase::Loading) {
            return centered_state("Loading file…", theme.text_faint);
        }
        if let DocumentPhase::Error(error) = &document.phase {
            return centered_state(error.clone(), theme.danger_muted);
        }
        if let Some(editor) = editor {
            editor.update(cx, |state, _| {
                state.set_editor_style(super::super::editor_adapter::editor_style(theme));
            });
            let overlays = self.render_editor_comment_overlays(path, &editor, theme, cx);
            return div()
                .id("files-editor-body")
                .flex_1()
                .min_w_0()
                .min_h_0()
                .relative()
                .overflow_hidden()
                .font_family(theme.font_mono.clone())
                .text_size(px(self.preview.editor_text_size()))
                .line_height(px(
                    (self.preview.editor_text_size() + 8.5).max(PREVIEW_LINE_HEIGHT)
                ))
                .child(super::super::editor::editor_element(&editor))
                .children(overlays)
                .into_any_element();
        }
        let Some(file) = document.file.as_ref() else {
            return centered_state("This file cannot be previewed.", theme.text_muted);
        };
        if file.text.is_none() {
            return centered_state(read_only_message(file.read_only_reason), theme.text_muted);
        }
        let truncated = file.truncated;
        let word_wrap = self.preview.word_wrap();
        let code_scroll = if word_wrap {
            div()
                .id("files-preview-code-scroll")
                .flex_1()
                .min_w_0()
                .min_h_0()
                .flex()
                .child(
                    list(
                        self.preview.list.clone(),
                        cx.processor(Self::render_preview_line),
                    )
                    .flex_1()
                    .min_h_0()
                    .with_sizing_behavior(ListSizingBehavior::Auto),
                )
        } else {
            let mut scroll = div()
                .id("files-preview-code-scroll")
                .flex_1()
                .min_w_0()
                .min_h_0()
                .flex()
                .overflow_x_scroll()
                .track_scroll(&self.preview.horizontal_scroll)
                .child(
                    div().flex_none().min_w_full().h_full().child(
                        list(
                            self.preview.list.clone(),
                            cx.processor(Self::render_preview_line),
                        )
                        .h_full()
                        .with_sizing_behavior(ListSizingBehavior::Infer),
                    ),
                );
            // GPUI otherwise maps a vertical wheel gesture to X for an x-only
            // scroller, preventing the list from receiving it.
            scroll.style().restrict_scroll_to_axis = Some(true);
            scroll
        };

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .when(truncated, |element| {
                element.child(
                    div()
                        .h(px(28.0))
                        .flex_none()
                        .px(px(10.0))
                        .border_b_1()
                        .border_color(theme.border)
                        .bg(theme.warning.opacity(0.045))
                        .flex()
                        .items_center()
                        .text_size(px(10.0))
                        .text_color(theme.warning_muted)
                        .child("Large file preview is truncated and read-only."),
                )
            })
            .child(code_scroll)
            .into_any_element()
    }

    fn render_editor_comment_overlays(
        &mut self,
        path: &str,
        editor: &Entity<super::super::editor::FileEditorState>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(layout) = editor.read_with(cx, |state, _| editor_overlay_layout(state)) else {
            return Vec::new();
        };
        let comments = self.staged_file_comments(path, cx);
        let comments_by_line = comments
            .iter()
            .map(|comment| (comment.line, comment.clone()))
            .collect::<HashMap<_, _>>();
        let (card_left, card_width) = editor_comment_overlay_horizontal(&layout);
        let mut overlays = Vec::with_capacity(layout.rows.len() + 1);
        for row in &layout.rows {
            let group: SharedString = format!("file-comment-gutter-{}-{}", path, row.line).into();
            let cell = div()
                .id(("file-comment-gutter", row.line as usize))
                .absolute()
                .left(px(0.0))
                .top(px(row.top))
                .w(px(layout.gutter_width))
                .h(px(layout.line_height))
                .flex()
                .items_center()
                .justify_center();
            if let Some(comment) = comments_by_line.get(&row.line) {
                let id = comment.id.clone();
                overlays.push(
                    cell.bg(theme.surface_card)
                        .cursor_pointer()
                        .role(gpui::Role::Button)
                        .aria_label(format!("Open comment on line {}", row.line))
                        .hover(|style| style.bg(crate::theme::wash(0.08)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_editor_comment(id.clone(), cx)
                        }))
                        .child(
                            icon(icons::CHAT_ROUND_LINE)
                                .size(px(10.5))
                                .text_color(theme.text_muted),
                        )
                        .into_any_element(),
                );
            } else {
                let target = path.to_string();
                let line = row.line;
                overlays.push(
                    cell.group(group.clone())
                        .cursor_pointer()
                        .role(gpui::Role::Button)
                        .aria_label(format!("Comment on line {line}"))
                        .hover(|style| style.bg(theme.surface_card))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_editor_comment_draft(target.clone(), line, window, cx)
                        }))
                        .child(
                            div()
                                .size(px(comments::COMMENT_ADDER_SIZE))
                                .opacity(0.0)
                                .group_hover(group, |style| style.opacity(1.0))
                                .rounded(px(4.0))
                                .bg(theme.solid)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(icon(icons::PLUS).size(px(11.0)).text_color(theme.on_solid)),
                        )
                        .into_any_element(),
                );
            }
        }

        let active_comment = self
            .preview
            .active_comment
            .as_deref()
            .and_then(|id| comments.iter().find(|comment| comment.id == id))
            .cloned();
        if let Some(comment) = active_comment
            && let Some(top) = editor_comment_overlay_top(
                &layout,
                comment.line,
                comments::card_height(&comment.body),
            )
        {
            overlays.push(
                self.render_editor_comment_card(comment, card_left, card_width, top, theme, cx),
            );
        } else if let Some(draft) = self
            .preview
            .comment_draft
            .as_ref()
            .filter(|draft| draft.path == path)
            && let Some(top) =
                editor_comment_overlay_top(&layout, draft.line, EDITOR_COMMENT_DRAFT_HEIGHT)
        {
            overlays.push(self.render_editor_comment_draft(
                draft.input.clone(),
                card_left,
                card_width,
                top,
                theme,
                cx,
            ));
        }
        overlays
    }

    fn render_editor_comment_card(
        &self,
        comment: ReviewComment,
        left: f32,
        width: f32,
        top: f32,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = &theme.for_popup();
        let group: SharedString = format!("file-comment-card-{}", comment.id).into();
        let id = comment.id.clone();
        let card = crate::popover::popover_card_flush(theme)
            .group(group.clone())
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(width))
            .h(px(comments::card_height(&comment.body)))
            .flex()
            .flex_col()
            .font_family(theme.font_sans.clone())
            .px(px(Theme::SPACE_LG))
            .py(px(comments::CARD_PAD_V / 2.0))
            .child(
                div()
                    .h(px(comments::CARD_HEADER_HEIGHT))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        icon(icons::CHAT_ROUND_LINE)
                            .size(px(12.0))
                            .text_color(theme.text_faint),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(theme.font_mono.clone())
                            .text_size(px(11.0))
                            .text_color(theme.text_faint)
                            .child(SharedString::from(comment.location())),
                    )
                    .child(crate::comment_ui::render_comment_edit(
                        &comment,
                        group.clone(),
                        theme,
                        cx,
                        Self::edit_editor_comment,
                    ))
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "file-comment-remove-{}",
                                comment.id
                            )))
                            .size(px(16.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .opacity(0.0)
                            .group_hover(group, |style| style.opacity(1.0))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.remove_editor_comment(&id, cx)
                            }))
                            .child(
                                icon(icons::CLOSE_CIRCLE)
                                    .size(px(12.0))
                                    .text_color(theme.text_muted),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .text_size(px(12.0))
                    .line_height(px(comments::CARD_LINE_HEIGHT))
                    .text_color(theme.text_dim)
                    .child(SharedString::from(comment.body)),
            );
        crate::frost::frosted(crate::popover::CARD_RADIUS, crate::frost::MENU_BLUR, card)
            .into_any_element()
    }

    fn render_editor_comment_draft(
        &self,
        input: Entity<ComposerInput>,
        left: f32,
        width: f32,
        top: f32,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = &theme.for_popup();
        let card = crate::popover::popover_card_flush(theme)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.cancel_editor_comment(cx);
                }
            }))
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(width))
            .h(px(EDITOR_COMMENT_DRAFT_HEIGHT))
            .flex()
            .flex_col()
            .font_family(theme.font_sans.clone())
            .px(px(Theme::SPACE_LG))
            .py(px(8.0))
            .child(
                div()
                    .h(px(48.0))
                    .flex_none()
                    .overflow_hidden()
                    .rounded(px(7.0))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.input_glass_bg())
                    .px(px(8.0))
                    .py(px(5.0))
                    .text_size(px(12.0))
                    .child(input.into_any_element()),
            )
            .child(
                div()
                    .h(px(28.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(6.0))
                    .child(
                        editor_comment_action("file-comment-cancel", "Cancel", false, theme)
                            .on_click(cx.listener(|this, _, _, cx| this.cancel_editor_comment(cx))),
                    )
                    .child(
                        editor_comment_action(
                            "file-comment-commit",
                            if self
                                .preview
                                .comment_draft
                                .as_ref()
                                .is_some_and(|draft| draft.editing_id.is_some())
                            {
                                "Save"
                            } else {
                                "Comment"
                            },
                            true,
                            theme,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.commit_editor_comment(cx))),
                    ),
            );
        crate::frost::frosted(crate::popover::CARD_RADIUS, crate::frost::MENU_BLUR, card)
            .into_any_element()
    }

    fn ensure_editor(
        &mut self,
        path: &str,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Entity<super::super::editor::FileEditorState>> {
        let document = self.preview.documents.get(path)?;
        if let Some(editor) = document.editor.clone() {
            return Some(editor);
        }
        if !document.is_editable() {
            return None;
        }
        let text = document.file.as_ref()?.text.clone()?;
        let focus_editor = !document.show_markdown;
        let editor = super::super::editor::new_file_editor(
            text,
            path,
            self.preview.word_wrap,
            theme,
            window,
            cx,
        );
        let event_path = path.to_string();
        let editor_events = super::super::editor::subscribe_to_changes(&editor, event_path, cx);
        let editor_observer = cx.observe(&editor, |_, _, cx| cx.notify());
        if focus_editor {
            let focus = editor.focus_handle(cx);
            window.defer(cx, move |window, cx| focus.focus(window, cx));
        }
        let document = self.preview.documents.get_mut(path)?;
        document.editor = Some(editor.clone());
        document.editor_events = Some(editor_events);
        document.editor_observer = Some(editor_observer);
        let syntax = self.preview.highlights.get(path).and_then(|highlight| {
            let document = self.preview.documents.get(path)?;
            if document.content_hash() != Some(highlight.content_hash.as_str()) {
                return None;
            }
            Some((
                document.file.as_ref()?.text.clone()?,
                highlight.document.clone(),
            ))
        });
        if let Some((source, highlighted)) = syntax {
            super::super::editor_adapter::install_highlighter(&editor, source, highlighted, cx);
        }
        self.sync_editor_comment_anchors(path, &editor, cx);
        Some(editor)
    }

    fn render_preview_line(
        &mut self,
        index: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(path) = self.preview.active.as_deref() else {
            return gpui::Empty.into_any_element();
        };
        let Some(document) = self.preview.documents.get(path) else {
            return gpui::Empty.into_any_element();
        };
        let Some(file) = document.file.as_ref() else {
            return gpui::Empty.into_any_element();
        };
        let Some(line) = document.lines.get(index) else {
            return gpui::Empty.into_any_element();
        };
        let theme = Theme::of(cx).clone();
        let word_wrap = self.preview.word_wrap();
        let spans = self
            .preview
            .highlights
            .get(path)
            .filter(|highlight| {
                file.content_hash.as_deref() == Some(highlight.content_hash.as_str())
            })
            .and_then(|highlight| highlight.document.lines.get(index))
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let mono = font(theme.font_mono.clone());
        let runs = crate::markdown::render::runs_for_syntax_line_with_plain(
            line.as_ref(),
            spans,
            &mono,
            theme.text.opacity(0.93),
            &theme,
        );
        let row_height = self.preview.line_height();
        div()
            .min_h(row_height)
            .flex_none()
            .flex()
            .when(word_wrap, |element| element.w_full().items_stretch())
            .when(!word_wrap, |element| {
                element.h(row_height).min_w_full().items_center()
            })
            .child(
                div()
                    .w(px(48.0))
                    .when(!word_wrap, |element| element.h_full())
                    .flex_none()
                    .pr(px(10.0))
                    .border_r_1()
                    .border_color(theme.border.opacity(0.55))
                    .flex()
                    .items_center()
                    .justify_end()
                    .font_family(theme.font_mono.clone())
                    .text_size(px(10.0))
                    .text_color(theme.text_faint.opacity(0.7))
                    .child((index + 1).to_string()),
            )
            .child(
                div()
                    .when(word_wrap, |element| {
                        element.flex_1().min_w_0().py(px(2.0)).whitespace_normal()
                    })
                    .pl(px(12.0))
                    .pr(px(18.0))
                    .when(!word_wrap, |element| element.whitespace_nowrap())
                    .font_family(theme.font_mono.clone())
                    // Same source `row_height` derives from; a second reading of
                    // the code size could drift from the geometry the glyphs
                    // are measured against.
                    .text_size(px(self.preview.preview_text_size()))
                    .child(gpui::StyledText::new(line.clone()).with_runs(runs)),
            )
            .into_any_element()
    }

    pub(crate) fn on_preview_split_drag(
        &mut self,
        event: &gpui::DragMoveEvent<PreviewSplitResize>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let requested = f32::from(event.bounds.right() - event.event.position.x);
        let sample = crate::motion::resize_drag_sample(
            requested,
            TREE_SPLIT_MIN,
            TREE_SPLIT_MAX,
            self.preview.tree_resize_edge,
            crate::motion::reduced_motion(cx),
        );
        self.preview.tree_width = sample.width;
        self.preview.tree_resize_dragging = true;
        self.preview.tree_resize_active = sample.edge.is_none();
        if sample.starts_bounce {
            self.preview.tree_edge_bounce = sample.edge.map(crate::motion::ResizeEdgeBounce::new);
        } else if sample.edge.is_none() {
            self.preview.tree_edge_bounce = None;
        }
        self.preview.tree_resize_edge = sample.edge;
        cx.notify();
    }

    pub(crate) fn preview_split_handle(&self, right: f32, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx);
        let fade_key = "pane-resize-files-preview-split";
        let hover_highlight = crate::motion::hover_blend(
            fade_key,
            theme.border_strong.opacity(0.0),
            theme.border_strong,
        );
        let highlight = if self.preview.tree_resize_constrained() {
            theme.border_strong.opacity(0.0)
        } else if self.preview.tree_resize_active() {
            theme.border_strong
        } else {
            hover_highlight
        };
        let clear = highlight.opacity(0.0);
        div()
            .id("files-preview-split")
            .absolute()
            .right(px(right))
            .top_0()
            .bottom_0()
            .w(px(TREE_SPLIT_HITBOX_HALF_WIDTH * 2.0))
            .occlude()
            .cursor_col_resize()
            .on_hover(crate::motion::hover_listener(fade_key))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(TREE_SPLIT_HITBOX_HALF_WIDTH))
                    .w(px(1.0))
                    .flex()
                    .flex_col()
                    .child(div().flex_1().bg(gpui::linear_gradient(
                        180.0,
                        gpui::linear_color_stop(clear, 0.0),
                        gpui::linear_color_stop(highlight, 1.0),
                    )))
                    .child(div().flex_1().bg(gpui::linear_gradient(
                        180.0,
                        gpui::linear_color_stop(highlight, 0.0),
                        gpui::linear_color_stop(clear, 1.0),
                    ))),
            )
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.preview.tree_resize_dragging = true;
                    this.preview.tree_resize_active = true;
                    cx.notify();
                }),
            )
            .on_drag(
                PreviewSplitResize,
                |_, _point: Point<gpui::Pixels>, _, cx| {
                    cx.stop_propagation();
                    cx.new(|_| PreviewDragGhost)
                },
            )
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseUpEvent, window, cx| {
                    if event.click_count == 2 {
                        this.preview.tree_width = TREE_SPLIT_DEFAULT;
                        this.preview.tree_edge_bounce = None;
                    }
                    this.preview.finish_tree_resize();
                    crate::motion::set_hover(fade_key, false, crate::motion::reduced_motion(cx));
                    window.refresh();
                    cx.notify();
                }),
            )
            .on_mouse_up_out(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.preview.finish_tree_resize();
                    crate::motion::set_hover(fade_key, false, crate::motion::reduced_motion(cx));
                    window.refresh();
                    cx.notify();
                }),
            )
            .into_any_element()
    }
}

fn editor_comment_overlay_top(
    layout: &EditorOverlayLayout,
    line: u32,
    card_height: f32,
) -> Option<f32> {
    let row = layout.rows.iter().find(|row| row.line == line)?;
    let preferred = row.top + layout.line_height;
    Some(preferred.clamp(0.0, (layout.viewport_height - card_height).max(0.0)))
}

fn editor_comment_overlay_horizontal(layout: &EditorOverlayLayout) -> (f32, f32) {
    let anchored_left =
        (layout.gutter_width - EDITOR_COMMENT_CARD_MARGIN).max(EDITOR_COMMENT_CARD_MARGIN);
    let anchored_width = (layout.viewport_width - anchored_left - EDITOR_COMMENT_CARD_MARGIN)
        .min(EDITOR_COMMENT_CARD_WIDTH)
        .max(0.0);
    if anchored_width >= EDITOR_COMMENT_CARD_MIN_ANCHORED_WIDTH {
        (anchored_left, anchored_width)
    } else {
        (
            EDITOR_COMMENT_CARD_MARGIN,
            (layout.viewport_width - EDITOR_COMMENT_CARD_MARGIN * 2.0).max(0.0),
        )
    }
}

fn editor_comment_action(
    id: &'static str,
    label: &'static str,
    primary: bool,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(22.0))
        .px(px(10.0))
        .flex()
        .items_center()
        .rounded(px(6.0))
        .text_size(px(11.0))
        .font_weight(gpui::FontWeight::MEDIUM)
        .cursor_pointer()
        .when(primary, |element| {
            element.bg(theme.solid).text_color(theme.on_solid)
        })
        .when(!primary, |element| {
            element
                .text_color(crate::motion::hover_blend(id, theme.text_muted, theme.text))
                .bg(crate::motion::hover_blend(
                    id,
                    gpui::transparent_black(),
                    theme.element_hover,
                ))
                .on_hover(crate::motion::hover_listener(id))
        })
        .child(SharedString::from(label))
}

fn centered_state(message: impl Into<SharedString>, color: gpui::Hsla) -> AnyElement {
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .px(px(24.0))
        .text_center()
        .text_size(px(11.5))
        .text_color(color)
        .child(message.into())
        .into_any_element()
}

fn read_only_message(reason: Option<WorkspaceReadOnlyReason>) -> SharedString {
    match reason {
        Some(WorkspaceReadOnlyReason::Binary) => "Binary files cannot be previewed.",
        Some(WorkspaceReadOnlyReason::UnsupportedEncoding) => {
            "This file encoding is not supported."
        }
        Some(WorkspaceReadOnlyReason::Symlink) => "Symlink targets are read-only.",
        Some(WorkspaceReadOnlyReason::PermissionDenied) => "Permission denied.",
        Some(WorkspaceReadOnlyReason::TooLarge) => "This file is too large to preview.",
        Some(WorkspaceReadOnlyReason::MixedLineEndings) => {
            "Files with mixed line endings are read-only."
        }
        Some(WorkspaceReadOnlyReason::NotRegularFile) | None => "This file cannot be previewed.",
    }
    .into()
}
