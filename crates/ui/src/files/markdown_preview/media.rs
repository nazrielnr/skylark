//! Markdown preview media lifecycle, selection scrolling, and code-fence state.

use super::*;

impl MarkdownPreview {
    pub(crate) fn close_media_preview(&mut self, cx: &mut gpui::App) {
        if let Some(preview) = self.preview_image.take() {
            if self
                .zoom_source
                .as_ref()
                .is_some_and(|source| !Arc::ptr_eq(&source.image, &preview.image))
            {
                cx.defer(move |cx| gpui::ImageSource::Image(preview.image).evict(None, cx));
            }
        }
        self.zoom_source = None;
        self.zoom_render = None;
    }

    pub(crate) fn resize_media(&mut self, window: &Window, cx: &mut Context<Self>) {
        let viewport = window.viewport_size();
        let panel_width = self.list.viewport_bounds().size.width;
        let target = (
            (f32::from(if panel_width > px(0.0) {
                panel_width
            } else {
                viewport.width
            }) - 48.0)
                .clamp(1.0, MAX_PREVIEW_CONTENT_WIDTH),
            480.0,
        );
        let mut retired = Vec::new();
        for media in self
            .images
            .values_mut()
            .chain(self.diagrams.values_mut())
            .filter_map(|m| m.as_mut().ok())
        {
            let next = media.preview_for_view(target, window.scale_factor());
            if !Arc::ptr_eq(&media.image, &next.image) {
                retired.push(std::mem::replace(media, next));
                self.media_dirty = true;
            }
        }
        release_media(retired, cx);
        if let (Some(source), Some(preview)) = (&self.zoom_source, &mut self.preview_image) {
            let used: usize = self
                .images
                .values()
                .chain(self.diagrams.values())
                .filter_map(|m| m.as_ref().ok())
                .map(|m| m.bytes)
                .sum();
            let next = source.enlarged(
                (
                    f32::from(viewport.width) * 0.9,
                    f32::from(viewport.height) * 0.85,
                ),
                window.scale_factor(),
                MAX_MEDIA_BYTES.saturating_sub(used),
                self.zoom_render.as_ref(),
            );
            // Keep a stable image identity while the viewport is unchanged.
            if !Arc::ptr_eq(&preview.image, &next.image) {
                let old = std::mem::replace(&mut preview.image, next.image.clone());
                if !Arc::ptr_eq(&old, &source.image) {
                    cx.defer(move |cx| gpui::ImageSource::Image(old).evict(None, cx));
                }
            }
            self.zoom_render = Some(next);
        }
    }
    pub(crate) fn set_comments(
        &mut self,
        owner: gpui::WeakEntity<super::super::FilesSurface>,
        comments: Vec<crate::comments::ReviewComment>,
        draft: Option<(u32, gpui::Entity<crate::composer::ComposerInput>, bool)>,
        cx: &mut Context<Self>,
    ) {
        self.comment_owner = Some(owner);
        if self.comments != comments || self.comment_draft != draft {
            self.comments = comments;
            self.comment_draft = draft;
            self.list.remeasure_items(0..self.tree.len());
            cx.notify();
        }
    }

    pub(crate) fn open_comment(
        &mut self,
        line: u32,
        source: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.truncated {
            return;
        }
        if let Some(owner) = &self.comment_owner {
            let _ = owner.update(cx, |owner, cx| {
                owner.open_markdown_comment(self.path.clone(), line, source, window, cx)
            });
        }
    }

    pub(crate) fn cancel_comment(&mut self, cx: &mut Context<Self>) {
        if let Some(owner) = &self.comment_owner {
            let _ = owner.update(cx, |owner, cx| owner.cancel_editor_comment(cx));
        }
    }

    pub(crate) fn commit_comment(&mut self, cx: &mut Context<Self>) {
        if let Some(owner) = &self.comment_owner {
            let _ = owner.update(cx, |owner, cx| owner.commit_editor_comment(cx));
        }
    }

    pub(crate) fn edit_comment(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(owner) = &self.comment_owner {
            let _ = owner.update(cx, |owner, cx| owner.edit_editor_comment(id, window, cx));
        }
    }

    pub(crate) fn remove_comment(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(owner) = &self.comment_owner {
            let _ = owner.update(cx, |owner, cx| owner.remove_editor_comment(id, cx));
        }
    }

    pub(crate) fn comment_elements(
        &self,
        ix: usize,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        let column = Some(crate::comment_ui::CommentContentColumn {
            max_width: MAX_PREVIEW_CONTENT_WIDTH,
            gutter: 24.0,
        });
        let mut elements: Vec<_> = self
            .comments
            .iter()
            .filter(|comment| comment_block(&self.block_lines, comment.line) == Some(ix))
            .map(|comment| {
                crate::comment_ui::render_comment_card(
                    comment,
                    theme,
                    cx,
                    Self::edit_comment,
                    Self::remove_comment,
                    column,
                )
            })
            .collect();
        if let Some((line, input, editing)) = &self.comment_draft {
            if comment_block(&self.block_lines, *line) == Some(ix) {
                elements.push(crate::comment_ui::render_comment_draft(
                    &self.path,
                    *line,
                    input.clone(),
                    *editing,
                    theme,
                    cx,
                    Self::cancel_comment,
                    Self::commit_comment,
                    column,
                ));
            }
        }
        elements
    }

    pub fn new(
        path: String,
        open_file: Rc<dyn Fn(String, &mut gpui::App)>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.on_release(|view, cx| {
            view.close_media_preview(cx);
            render::clear_selection_surface(&view.scope);
            let media = view
                .images
                .drain()
                .chain(view.diagrams.drain())
                .filter_map(|(_, result)| result.ok());
            release_media(media, cx);
        })
        .detach();
        Self {
            parsed_source: Arc::from(""),
            block_lines: Vec::new(),
            comment_owner: None,
            comments: Vec::new(),
            comment_draft: None,
            editor: None,
            image_generation: 0,
            media_dirty: true,
            image_allowed: Rc::default(),
            diagram_allowed: Rc::default(),
            image_snapshot: Rc::default(),
            diagram_snapshot: Rc::default(),
            visible_rows: HashSet::new(),
            code_fences: HashMap::new(),
            code_fences_generation: crate::settings::code_fences_generation(cx),
            copied_code: None,
            copied_clear: None,
            needs_focus: true,
            suspended: false,
            selection_pointer: None,
            selection_task: None,
            diagrams: HashMap::new(),
            diagram_task: None,
            diagram_style: 0,
            source_visible: HashSet::new(),
            preview_image: None,
            zoom_source: None,
            zoom_render: None,
            preview_focus: cx.focus_handle(),
            media_client: None,
            images: HashMap::new(),
            image_task: None,
            focus: cx.focus_handle(),
            version: None,
            path,
            media_location: None,
            scope: format!("md-preview-{}|", cx.entity_id()),
            tree: BlockTree::default(),
            list: ListState::new(0, ListAlignment::Top, px(400.0)),
            cache: Rc::new(RefCell::new(RenderCache::default())),
            highlights: HashMap::new(),
            anchors: HashMap::new(),
            parse_task: None,
            epoch: 0,
            loading: false,
            truncated: false,
            open_file,
            open_web_link: None,
        }
    }

    pub fn set_source(&mut self, mut source: String, truncated: bool, cx: &mut Context<Self>) {
        let clipped = source.len() > MAX_MARKDOWN_BYTES;
        if clipped {
            let mut end = MAX_MARKDOWN_BYTES;
            while !source.is_char_boundary(end) {
                end -= 1;
            }
            source.truncate(end);
        }
        render::clear_selection_surface(&self.scope);
        self.epoch = self.epoch.wrapping_add(1);
        self.image_task = None;
        self.diagram_task = None;
        self.close_media_preview(cx);
        self.source_visible.clear();
        self.copied_clear = None;
        self.copied_code = None;
        let epoch = self.epoch;
        self.loading = self.tree.is_empty();
        self.truncated = truncated || clipped;
        let parsed_source: Arc<str> = Arc::from(source.as_str());
        self.parse_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(120))
                .await;
            let (tree, highlights, anchors, block_lines) = cx
                .background_executor()
                .spawn(async move {
                    let tree = parser::parse_full(&source);
                    let block_lines = block_source_lines(&source, &tree);
                    let mut highlights = HashMap::new();
                    let mut anchors = HashMap::new();
                    for (ix, top) in tree.blocks.iter().enumerate() {
                        if let Block::CodeBlock { language, code } = &top.block {
                            if let Ok(doc) =
                                zeron_syntax::highlight(zeron_syntax::HighlightRequest {
                                    source: code,
                                    path: None,
                                    fence_tag: language.as_deref(),
                                })
                            {
                                highlights.insert(ix, Arc::new(doc));
                            }
                        }
                        if let Block::Heading { runs, .. } = &top.block {
                            let text: String = runs.iter().map(|r| r.text.as_str()).collect();
                            let slug: String = text
                                .to_lowercase()
                                .chars()
                                .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_'))
                                .map(|c| if c == ' ' { '-' } else { c })
                                .collect();
                            let mut unique = slug.clone();
                            let mut n = 1;
                            while anchors.contains_key(&unique) {
                                unique = format!("{slug}-{n}");
                                n += 1;
                            }
                            anchors.insert(unique, ix);
                        }
                    }
                    (tree, highlights, anchors, block_lines)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.epoch != epoch {
                    return;
                }
                let offset = view.list.logical_scroll_top();
                view.list.reset(tree.len());
                if !tree.is_empty() {
                    view.list.scroll_to(ListOffset {
                        item_ix: offset.item_ix.min(tree.len() - 1),
                        offset_in_item: offset.offset_in_item,
                    });
                }
                view.tree = tree;
                let scope = view.scope.clone();
                let active_code_fences: HashSet<SharedString> = view
                    .tree
                    .blocks
                    .iter()
                    .enumerate()
                    .flat_map(move |(top_ix, top)| {
                        let scope = scope.clone();
                        render::code_block_indices(&top.block, top_ix)
                            .into_iter()
                            .map(move |ix| SharedString::from(format!("{scope}{top_ix}#code{ix}")))
                    })
                    .collect();
                view.code_fences
                    .retain(|key, _| active_code_fences.contains(key));
                view.parsed_source = parsed_source;
                view.block_lines = block_lines;
                view.highlights = highlights;
                view.anchors = anchors;
                view.cache.borrow_mut().clear();
                view.loading = false;
                view.load_images(cx);
                view.load_diagrams(cx);
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn load_images(&mut self, cx: &mut Context<Self>) {
        self.media_dirty = true;
        self.image_generation = self.image_generation.wrapping_add(1);
        let generation = self.image_generation;
        use futures::{StreamExt as _, stream};
        let sources = crate::files::markdown_media::image_sources(&self.tree);
        self.image_allowed = Rc::new(sources.iter().take(MAX_MEDIA_ENTRIES).cloned().collect());
        let removed: Vec<_> = self
            .images
            .keys()
            .filter(|source| !sources.contains(source))
            .cloned()
            .collect();
        release_media(
            removed
                .into_iter()
                .filter_map(|source| self.images.remove(&source).and_then(Result::ok)),
            cx,
        );
        let Some((client, checkout)) = self.media_client.clone() else {
            for source in sources.into_iter().take(MAX_MEDIA_ENTRIES) {
                self.images
                    .entry(source)
                    .or_insert_with(|| Err("Workspace image connection unavailable".into()));
            }
            return;
        };
        let jobs: Vec<_> = sources
            .into_iter()
            .take(MAX_MEDIA_ENTRIES)
            .filter(|s| !self.images.contains_key(s))
            .collect::<Vec<_>>()
            .into_iter()
            .filter_map(|source| {
                if source.starts_with("https://") || source.starts_with("http://") {
                    return None;
                }
                match relative_target(&self.path, &source) {
                    Some((path, _)) => Some((source, path)),
                    None => {
                        self.images
                            .insert(source, Err("Image path is outside the workspace".into()));
                        None
                    }
                }
            })
            .collect();
        let epoch = self.epoch;
        self.image_task = Some(cx.spawn(async move |this, cx| {
            let executor = cx.background_executor().clone();
            let mut results = stream::iter(jobs)
                .map(|(source, path)| {
                    let client = client.clone();
                    let checkout = checkout.clone();
                    let executor = executor.clone();
                    async move {
                        let read = Box::pin(client.read_image(path, checkout));
                        let deadline = Box::pin(executor.timer(Duration::from_secs(30)));
                        let response = match futures::future::select(read, deadline).await {
                            futures::future::Either::Left((result, _)) => result,
                            futures::future::Either::Right(_) => {
                                Err(crate::files::client::FilesClientError::Transport(
                                    "Image preview timed out".into(),
                                ))
                            }
                        };
                        let result = match response {
                            Ok((mime, bytes)) => executor
                                .spawn(
                                    async move { crate::image_media::decode_image(&mime, bytes) },
                                )
                                .await,
                            Err(error) => Err(error.to_string()),
                        };
                        (source, result)
                    }
                })
                .buffer_unordered(3);
            while let Some((source, result)) = results.next().await {
                if this
                    .update(cx, |view, cx| {
                        if view.epoch != epoch || view.image_generation != generation {
                            return;
                        }
                        let result = view.admit_media(result);
                        view.images.insert(source, result);
                        view.media_dirty = true;
                        view.list.remeasure_items(0..view.tree.len());
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
        }));
    }

    pub(crate) fn load_diagrams(&mut self, cx: &mut Context<Self>) {
        self.media_dirty = true;
        let sources = crate::files::markdown_media::diagram_sources(&self.tree);
        self.diagram_allowed = Rc::new(sources.iter().take(MAX_MEDIA_ENTRIES).cloned().collect());
        let removed: Vec<_> = self
            .diagrams
            .keys()
            .filter(|code| !sources.contains(code))
            .cloned()
            .collect();
        release_media(
            removed
                .into_iter()
                .filter_map(|code| self.diagrams.remove(&code).and_then(Result::ok)),
            cx,
        );
        let style = crate::theme::style_generation();
        if self.diagram_style != style {
            self.close_media_preview(cx);
            release_media(
                self.diagrams.drain().filter_map(|(_, result)| result.ok()),
                cx,
            );
            self.diagram_style = style;
        }
        let palette = crate::markdown::mermaid::Palette::from_theme(Theme::of(cx));
        let jobs: Vec<_> = sources
            .into_iter()
            .take(MAX_MEDIA_ENTRIES)
            .filter(|code| !self.diagrams.contains_key(code))
            .collect();
        let epoch = self.epoch;
        self.diagram_task = Some(cx.spawn(async move |this, cx| {
            for code in jobs {
                let source = code.clone();
                let palette = palette.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        let svg = crate::markdown::mermaid::render(&source, &palette)?;
                        crate::image_media::decode_image("image/svg+xml", svg.into_bytes())
                    })
                    .await;
                if this
                    .update(cx, |view, cx| {
                        if view.epoch != epoch || view.diagram_style != style {
                            return;
                        }
                        let result = view.admit_media(result);
                        view.diagrams.insert(code, result);
                        view.media_dirty = true;
                        view.list.remeasure_items(0..view.tree.len());
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
        }));
    }

    fn admit_media(
        &self,
        result: Result<crate::image_media::MediaImage, String>,
    ) -> Result<crate::image_media::MediaImage, String> {
        let used: usize = self
            .images
            .values()
            .chain(self.diagrams.values())
            .filter_map(|r| r.as_ref().ok())
            .map(|m| m.bytes)
            .sum();
        result.and_then(|media| {
            if used.saturating_add(media.bytes) > MAX_MEDIA_BYTES {
                Err("Document media preview memory limit reached".into())
            } else {
                Ok(media)
            }
        })
    }

    pub(crate) fn activate(
        &mut self,
        path: &str,
        location: &(Option<String>, String),
        cx: &mut Context<Self>,
    ) {
        if self.path != path || self.media_location.as_ref() != Some(location) {
            self.path = path.to_string();
            self.media_location = Some(location.clone());
            self.version = None;
            self.image_generation = self.image_generation.wrapping_add(1);
            self.image_task = None;
            self.close_media_preview(cx);
            release_media(
                self.images.drain().filter_map(|(_, result)| result.ok()),
                cx,
            );
            self.media_dirty = true;
        }
        if self.suspended {
            self.suspended = false;
            self.needs_focus = true;
            self.version = None;
        }
    }

    pub(crate) fn suspend(&mut self, cx: &mut Context<Self>) {
        if self.suspended {
            return;
        }
        self.suspended = true;
        self.media_dirty = true;
        self.epoch = self.epoch.wrapping_add(1);
        self.parse_task = None;
        self.image_task = None;
        self.diagram_task = None;
        self.selection_task = None;
        self.copied_clear = None;
        self.copied_code = None;
        self.code_fences.clear();
        self.close_media_preview(cx);
        self.image_snapshot = Rc::default();
        self.diagram_snapshot = Rc::default();
        self.image_allowed = Rc::default();
        self.diagram_allowed = Rc::default();
        self.version = None;
        self.tree = BlockTree::default();
        self.parsed_source = Arc::from("");
        self.block_lines.clear();
        self.comment_owner = None;
        self.comments.clear();
        self.comment_draft = None;
        self.editor = None;
        self.highlights.clear();
        self.anchors.clear();
        self.cache.borrow_mut().clear();
        render::clear_selection_surface(&self.scope);
        release_media(
            self.images
                .drain()
                .chain(self.diagrams.drain())
                .filter_map(|(_, result)| result.ok()),
            cx,
        );
    }

    pub(crate) fn invalidate_images(&mut self, changed: Option<&str>, cx: &mut Context<Self>) {
        let removed: Vec<_> = self
            .images
            .keys()
            .filter(|source| {
                changed.is_none_or(|changed| {
                    relative_target(&self.path, source).is_some_and(|(path, _)| {
                        path == changed || path.starts_with(&format!("{changed}/"))
                    })
                })
            })
            .cloned()
            .collect();
        // Include pending loads in invalidation: a watcher can arrive before the first response.
        let affected = changed.is_none_or(|changed| {
            crate::files::markdown_media::image_sources(&self.tree)
                .iter()
                .any(|source| {
                    relative_target(&self.path, source).is_some_and(|(path, _)| {
                        path == changed || path.starts_with(&format!("{changed}/"))
                    })
                })
        });
        if !affected {
            return;
        }
        self.close_media_preview(cx);
        release_media(
            removed
                .into_iter()
                .filter_map(|source| self.images.remove(&source).and_then(Result::ok)),
            cx,
        );
        if !self.suspended {
            self.load_images(cx);
        }
        self.list.remeasure_items(0..self.tree.len());
        cx.notify();
    }

    pub(crate) fn owns_selection(&self) -> bool {
        crate::markdown::selection::anchor_key().is_some_and(|key| key.starts_with(&self.scope))
    }

    pub(crate) fn selection_move(
        &mut self,
        event: &gpui::MouseMoveEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !event.dragging() || !self.owns_selection() || !crate::markdown::selection::is_dragging()
        {
            self.selection_task = None;
            self.selection_pointer = None;
            return;
        }
        self.selection_pointer = Some(event.position);
        if render::update_drag_at(event.position) {
            cx.notify();
        }
        self.schedule_selection_scroll(cx);
    }

    pub(crate) fn schedule_selection_scroll(&mut self, cx: &mut Context<Self>) {
        if self.selection_task.is_some() {
            return;
        }
        let Some(position) = self.selection_pointer else {
            return;
        };
        if crate::transcript::selection_scroll_step(self.list.viewport_bounds(), position) == 0.0 {
            return;
        }
        self.selection_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(16))
                .await;
            let _ = this.update(cx, |view, cx| {
                view.selection_task = None;
                if !view.owns_selection() || !crate::markdown::selection::is_dragging() {
                    return;
                }
                let Some(position) = view.selection_pointer else {
                    return;
                };
                render::update_drag_at(position);
                view.list
                    .scroll_by(px(crate::transcript::selection_scroll_step(
                        view.list.viewport_bounds(),
                        position,
                    )));
                cx.notify();
                view.schedule_selection_scroll(cx);
            });
        }));
    }

    pub(crate) fn copy_ui_for(
        &self,
        row_id: &SharedString,
        cx: &mut Context<Self>,
    ) -> render::CopyUi {
        let copied_ix = self
            .copied_code
            .as_ref()
            .filter(|(id, _)| id == row_id)
            .map(|(_, ix)| *ix);
        let row_key = row_id.clone();
        let entity = cx.weak_entity();
        let handler = Rc::new(
            move |ix, code: SharedString, _window: &mut Window, cx: &mut gpui::App| {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(code.to_string()));
                let row_key = row_key.clone();
                let _ = entity.update(cx, |view, cx| {
                    view.copied_code = Some((row_key, ix));
                    view.copied_clear = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor()
                            .timer(Duration::from_millis(1200))
                            .await;
                        let _ = this.update(cx, |view, cx| {
                            view.copied_code = None;
                            view.copied_clear = None;
                            cx.notify();
                        });
                    }));
                    cx.notify();
                });
            },
        );
        render::CopyUi { handler, copied_ix }
    }

    pub(crate) fn code_ui_for(
        &mut self,
        row_id: &SharedString,
        block_ix: usize,
        cx: &mut Context<Self>,
    ) -> render::CodeUi {
        let key: SharedString = format!("{row_id}#code{block_ix}").into();
        let runtime = self.code_fences.entry(key.clone()).or_default();
        render::code_ui_for(
            key,
            crate::settings::current(cx).code_fences_fit_content,
            runtime,
            cx.weak_entity(),
            |view| &mut view.code_fences,
        )
    }

    pub(crate) fn code_uis_for(
        &mut self,
        row_id: &SharedString,
        block: &Block,
        block_ix: usize,
        cx: &mut Context<Self>,
    ) -> Option<HashMap<usize, render::CodeUi>> {
        let indices = render::code_block_indices(block, block_ix);
        (!indices.is_empty()).then(|| {
            indices
                .into_iter()
                .map(|ix| (ix, self.code_ui_for(row_id, ix, cx)))
                .collect()
        })
    }

    pub(crate) fn media_element(
        loaded: &crate::image_media::MediaImage,
        id: gpui::SharedString,
        name: String,
        weak: gpui::WeakEntity<Self>,
    ) -> AnyElement {
        use gpui::StyledImage as _;
        let preview = crate::attachments::PreviewImage::new(name, loaded.image.clone());
        let source = loaded.clone();
        div()
            .id(id)
            .w_full()
            .max_w(px(loaded.width))
            .mx_auto()
            .max_h(px(480.0))
            .aspect_ratio(loaded.width / loaded.height)
            .cursor_pointer()
            .role(gpui::Role::Button)
            .aria_label("Enlarge image")
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                let _ = weak.update(cx, |view, cx| {
                    view.close_media_preview(cx);
                    view.zoom_source = Some(source.clone());
                    preview.viewer.reset();
                    view.preview_image = Some(preview.clone());
                    window.focus(&view.preview_focus, cx);
                    cx.notify();
                });
            })
            .child(
                gpui::img(loaded.image.clone())
                    .size_full()
                    .object_fit(gpui::ObjectFit::Contain),
            )
            .into_any_element()
    }

    #[cfg(test)]
    pub(super) fn test_tree(&self) -> &BlockTree {
        &self.tree
    }

    #[cfg(test)]
    pub(super) fn test_block_bounds(&self, ix: usize) -> gpui::Bounds<gpui::Pixels> {
        self.list.bounds_for_item(ix).unwrap()
    }
}
