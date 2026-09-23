//! File preview state cache, sizing, and document lifecycle helpers.

use super::*;

impl FilePreviewState {
    pub(crate) fn new(
        autosave_enabled: bool,
        autosave_delay_ms: u64,
        word_wrap: bool,
        editor_font_size: f32,
    ) -> Self {
        Self {
            images_visible: true,
            documents: HashMap::new(),
            document_recency: VecDeque::new(),
            active: None,
            highlights: HashMap::new(),
            syntax_cache: SyntaxHighlightCache::default(),
            list: ListState::new(0, ListAlignment::Top, px(520.0)),
            horizontal_scroll: ScrollHandle::new(),
            surface_width: Rc::new(Cell::new(520.0)),
            word_wrap,
            editor_font_size,
            autosave_enabled,
            autosave_delay_ms,
            reload_confirmation: None,
            close_requested: false,
            tree_sidebar_visible: false,
            tree_sidebar_dismissed: false,
            tree_width_tween: None,
            tree_motion: TreeSidebarMotion::default(),
            tree_edge_bounce: None,
            tree_resize_edge: None,
            tree_resize_active: false,
            tree_resize_dragging: false,
            comment_anchors: HashMap::new(),
            comment_draft: None,
            active_comment: None,
            typography_generation: 0,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.documents.clear();
        self.document_recency.clear();
        self.active = None;
        self.highlights.clear();
        self.list.reset(0);
        self.reload_confirmation = None;
        self.close_requested = false;
        self.tree_sidebar_visible = false;
        self.tree_width_tween = None;
        self.tree_motion = TreeSidebarMotion::default();
        self.tree_edge_bounce = None;
        self.tree_resize_edge = None;
        self.tree_resize_active = false;
        self.tree_resize_dragging = false;
        self.comment_anchors.clear();
        self.comment_draft = None;
        self.active_comment = None;
    }

    pub(crate) fn has_active(&self) -> bool {
        self.active.is_some()
    }

    pub(super) fn touch_document(&mut self, path: &str) {
        self.document_recency.retain(|candidate| candidate != path);
        self.document_recency.push_back(path.to_string());
    }

    pub(super) fn retained_document_bytes(&self) -> usize {
        let documents = self.documents.values().fold(0usize, |total, document| {
            total.saturating_add(document.estimated_retained_bytes())
        });
        self.highlights
            .values()
            .fold(documents, |total, highlight| {
                total.saturating_add(estimated_highlighted_file_bytes(highlight))
            })
    }

    pub(super) fn document_is_evictable(
        &self,
        path: &str,
        protected_paths: &HashSet<String>,
    ) -> bool {
        if self.active.as_deref() == Some(path)
            || self.reload_confirmation.as_deref() == Some(path)
            || self
                .comment_draft
                .as_ref()
                .is_some_and(|draft| draft.path == path)
            || protected_paths.contains(path)
        {
            return false;
        }
        self.documents.get(path).is_some_and(|document| {
            !document.is_dirty()
                && matches!(
                    document.phase,
                    DocumentPhase::Ready
                        | DocumentPhase::ReadOnly(_)
                        | DocumentPhase::Error(_)
                        | DocumentPhase::DeletedOnDisk
                )
                && document.read_task.is_none()
                && document.highlight_task.is_none()
                && document.autosave_task.is_none()
                && document.save_task.is_none()
                && document.reconcile_task.is_none()
                && document.pending_save.is_none()
                && document.pending_external_reload.is_none()
                && !document.reconcile_after_save
                && !document.review_comment_flush_pending
        })
    }

    pub(super) fn evict_document(&mut self, path: &str) -> bool {
        if self.documents.remove(path).is_none() {
            return false;
        }
        self.document_recency.retain(|candidate| candidate != path);
        self.highlights.remove(path);
        if let Some(anchors) = self.comment_anchors.remove(path)
            && self
                .active_comment
                .as_ref()
                .is_some_and(|id| anchors.contains_key(id))
        {
            self.active_comment = None;
        }
        true
    }

    pub(super) fn trim_document_cache(&mut self, protected_paths: &HashSet<String>) -> Vec<String> {
        self.trim_document_cache_to(
            protected_paths,
            MAX_RETAINED_DOCUMENTS,
            MAX_RETAINED_DOCUMENT_BYTES,
        )
    }

    pub(super) fn trim_document_cache_to(
        &mut self,
        protected_paths: &HashSet<String>,
        max_documents: usize,
        max_bytes: usize,
    ) -> Vec<String> {
        let mut evicted = Vec::new();
        while self.documents.len() > max_documents || self.retained_document_bytes() > max_bytes {
            let candidate = self
                .document_recency
                .iter()
                .find(|path| self.document_is_evictable(path, protected_paths))
                .cloned()
                .or_else(|| {
                    self.documents
                        .keys()
                        .find(|path| self.document_is_evictable(path, protected_paths))
                        .cloned()
                });
            let Some(candidate) = candidate else {
                break;
            };
            if self.evict_document(&candidate) {
                evicted.push(candidate);
            }
        }
        evicted
    }

    pub(crate) fn is_wide(&self) -> bool {
        self.surface_width.get() >= WIDE_BREAKPOINT
    }

    pub(crate) fn width_cell(&self) -> Rc<Cell<f32>> {
        self.surface_width.clone()
    }

    /// The measured (or seeded) surface width, for layout-branch math.
    pub(crate) fn surface_width_read(&self) -> f32 {
        self.surface_width.get()
    }

    pub(crate) fn tree_sidebar_visible(&self) -> bool {
        self.tree_sidebar_visible || (self.is_wide() && !self.tree_sidebar_dismissed)
    }

    pub(crate) fn show_tree_sidebar(&mut self) {
        // File activation (open/reveal) is immediate: an animating sidebar
        // starts closed, which renders the loading state over the tree for
        // the transition's duration (narrow panes especially, where the
        // openness animation used to run on every tab open). Only the
        // explicit toggle animates — see `toggle_tree_sidebar`.
        self.tree_sidebar_visible = true;
        self.tree_sidebar_dismissed = false;
        self.tree_motion.snap_open();
    }

    pub(crate) fn show_tree_sidebar_animated(&mut self) {
        self.tree_sidebar_visible = true;
        self.tree_sidebar_dismissed = false;
        self.tree_motion.animate_to(false, true, Instant::now());
    }

    /// Prime the wide/narrow measurement with the SURFACE width (the pane),
    /// NOT the sidebar width: the measuring canvas only runs after the first
    /// paint, so without this frame 1 would take the narrow branch, rest at
    /// the 44% fallback, and visibly wiggle once the measurement lands.
    pub(crate) fn seed_surface_width(&mut self, width: f32) {
        self.surface_width.set(width);
    }

    /// Seed a new editor tab so its first frame matches the layout the tree
    /// already has (full pane from the raw workspace browser, or the current
    /// sidebar width from another file tab), then ease to the resting width
    /// (held by the shared tree entity).
    pub(crate) fn seed_sidebar_transition(&mut self, from_width: f32) {
        self.tree_width_tween = Some((from_width, Instant::now()));
    }

    pub(crate) fn clear_tree_width_tween(&mut self) {
        self.tree_width_tween = None;
    }

    /// Debug view of the width tween for tracing.
    pub(crate) fn tree_width_tween_debug(&self) -> Option<(f32, u64)> {
        self.tree_width_tween
            .map(|(from, started)| (from, started.elapsed().as_millis() as u64))
    }

    pub(super) fn toggle_tree_sidebar(&mut self) {
        self.tree_edge_bounce = None;
        self.tree_resize_edge = None;
        self.tree_resize_active = false;
        self.tree_resize_dragging = false;
        let previous = self.tree_sidebar_visible();
        if previous {
            self.tree_sidebar_visible = false;
            self.tree_sidebar_dismissed = true;
        } else {
            self.show_tree_sidebar();
        }
        self.tree_motion
            .animate_to(previous, self.tree_sidebar_visible(), Instant::now());
    }

    pub(super) fn word_wrap(&self) -> bool {
        self.word_wrap
    }

    /// Row height for plain (non-editable) preview lines. The list's
    /// uniform-height hint and the painted rows must both read it from here: a
    /// fixed 20 px row clips glyphs at larger code sizes, and a hint that
    /// disagrees with the painted row desyncs the virtualized measurements.
    pub(super) fn line_height(&self) -> gpui::Pixels {
        px((self.preview_text_size() * PREVIEW_LINE_HEIGHT_RATIO).max(PREVIEW_LINE_HEIGHT))
    }

    /// Text size of the editable editor.
    pub(super) fn editor_text_size(&self) -> f32 {
        crate::typography::clamp_font_size(self.editor_font_size * EDITOR_TEXT_SIZE_RATIO)
    }

    /// Text size of the plain (non-editable) preview rows.
    pub(super) fn preview_text_size(&self) -> f32 {
        crate::typography::clamp_font_size(self.editor_font_size * PREVIEW_TEXT_SIZE_RATIO)
    }

    pub(crate) fn set_editor_font_size(&mut self, editor_font_size: f32) {
        self.editor_font_size = editor_font_size;
    }

    pub(crate) fn set_autosave_delay_ms(&mut self, delay_ms: u64) -> Vec<String> {
        self.autosave_delay_ms = delay_ms;
        let mut pending = Vec::new();
        for (path, document) in &mut self.documents {
            document.autosave_task = None;
            if self.autosave_enabled && document.can_autosave() {
                pending.push(path.clone());
            }
        }
        pending
    }

    pub(crate) fn set_autosave_enabled(&mut self, enabled: bool) -> Vec<String> {
        self.autosave_enabled = enabled;
        let mut pending = Vec::new();
        for (path, document) in &mut self.documents {
            document.autosave_task = None;
            if enabled && document.can_autosave() {
                pending.push(path.clone());
            }
        }
        pending
    }

    pub(crate) fn tree_sidebar_frame(&mut self, window: &mut Window, cx: &App) -> f32 {
        let (openness, active) = self.tree_motion.sample(
            self.tree_sidebar_visible(),
            Instant::now(),
            crate::motion::reduced_motion(cx),
        );
        if active {
            window.request_animation_frame();
        }
        openness
    }

    pub(crate) fn tree_width_frame(&self, resting: f32, window: &mut Window, cx: &App) -> f32 {
        // The opening transition eases the sidebar from the width the tree
        // had when the tab was created (full pane when it comes from the
        // raw workspace browser) toward the resting width — in BOTH layout
        // branches, so narrow panes get the same continuous slide.
        if let Some((from, started)) = self.tree_width_tween {
            let total = Duration::from_millis(crate::motion::RESIZE.duration_ms)
                .mul_f32(crate::motion::speed_scale());
            let raw = Instant::now()
                .saturating_duration_since(started)
                .as_secs_f32()
                / total.as_secs_f32();
            if raw < 1.0 {
                window.request_animation_frame();
                return crate::motion::lerp(from, resting, crate::motion::RESIZE.progress(raw));
            }
        }
        let Some(bounce) = self.tree_edge_bounce else {
            return resting;
        };
        if crate::motion::reduced_motion(cx) || !self.tree_sidebar_visible() {
            return resting;
        }
        let total = Duration::from_millis(crate::motion::RESIZE_EDGE_BOUNCE_MS)
            .mul_f32(crate::motion::speed_scale());
        let raw = Instant::now()
            .saturating_duration_since(bounce.started)
            .as_secs_f32()
            / total.as_secs_f32();
        if raw >= 1.0 {
            return resting;
        }
        window.request_animation_frame();
        resting + crate::motion::resize_bounce_offset(bounce.edge, raw)
    }

    pub(crate) fn tree_resize_active(&self) -> bool {
        self.tree_resize_active
    }

    pub(crate) fn tree_resize_constrained(&self) -> bool {
        self.tree_resize_dragging && !self.tree_resize_active
    }

    pub(super) fn finish_tree_resize(&mut self) {
        self.tree_resize_active = false;
        self.tree_resize_dragging = false;
        self.tree_resize_edge = None;
    }

    pub(crate) fn has_unsaved_changes(&self) -> bool {
        self.documents.values().any(FileDocument::is_dirty)
    }

    pub(crate) fn dirty_paths(&self) -> Vec<String> {
        self.documents
            .iter()
            .filter(|(_, document)| document.is_dirty())
            .map(|(path, _)| path.clone())
            .collect()
    }

    pub(crate) fn cancel_autosaves(&mut self) {
        for document in self.documents.values_mut() {
            document.autosave_task = None;
        }
    }

    pub(super) fn request_reload(&mut self, path: &str) -> ReloadDecision {
        let Some(document) = self.documents.get_mut(path) else {
            return ReloadDecision::ReloadNow;
        };
        if !document.is_dirty() {
            self.reload_confirmation = None;
            return ReloadDecision::ReloadNow;
        }

        // A destructive reload must remain pending until the user explicitly
        // confirms it. Pause delayed autosave so it cannot race the choice.
        document.autosave_task = None;
        self.reload_confirmation = Some(path.to_string());
        ReloadDecision::AwaitDiscardConfirmation
    }

    pub(super) fn autosave_paused_for_reload(&self, path: &str) -> bool {
        self.reload_confirmation.as_deref() == Some(path)
    }
}
