//! The shared workspace file tree.
//!
//! ONE `FileTreeView` per panel, owned by the shell, embedded by every
//! Files surface (the browser tab and each editor tab). The tree model,
//! directory loads, and the workspace watcher live here exactly once, so:
//!
//! * opening more tabs never reloads the workspace (no per-tab `set_tree`
//!   clone, no root reload, no loading-state flash),
//! * rapid tab opens cannot race directory loads (one in-flight map),
//! * expansion, selection, scroll, and focus are shared — the tree the
//!   next tab shows is the tree the last click mutated.
//!
//! File-row activation is routed to the shell via
//! [`WorkspaceTreeEvent::OpenFile`]; the shell owns tab creation.

use std::{collections::HashMap, ops::Range, time::Duration};

use gpui::{
    AnyElement, Context, Entity, EventEmitter, FocusHandle, KeyDownEvent, MouseButton,
    ScrollStrategy, SharedString, Task, UniformListScrollHandle, Window, div, prelude::*, px,
    uniform_list,
};
use zeron_proto::{
    ListWorkspaceDirectoryRequest, WorkspaceEntryKind, WorkspaceFileChangeKind,
    WorkspaceFileChanges,
};

use super::{
    WorkspacePathDrag,
    client::{FilesRequestContext, WorkspaceFilesClient},
    model::{DirectoryLoadState, FileTreeModel, VisibleRowKind, parent_path},
    preview::{TREE_SPLIT_DEFAULT, TREE_SPLIT_MAX, TREE_SPLIT_MIN},
    workspace_path_drag_ghost,
};
use crate::state::AppState;
use crate::{
    file_icons::{self, FileIconIdentity},
    popover,
    theme::Theme,
};

pub(crate) const TREE_ROW_HEIGHT: f32 = 31.0;
const TREE_INDENT: f32 = 16.0;
const TREE_BASE_PAD: f32 = 8.0;
const ICON_SIZE: f32 = 15.0;

pub enum WorkspaceTreeEvent {
    /// A file row was activated: the shell owns tab creation/activation.
    OpenFile(String),
    /// The show-all toggle flipped; the shell persists the setting and
    /// applies it to every tree.
    ShowAllFilesChanged(bool),
    /// The watcher applied a frame to the tree; open documents should
    /// reconcile against it. `resync` marks a full refresh (gap or server
    /// request): surfaces reconcile everything.
    DocumentsChanged {
        frame: WorkspaceFileChanges,
        resync: bool,
    },
}

impl EventEmitter<WorkspaceTreeEvent> for FileTreeView {}

pub struct FileTreeView {
    state: Entity<AppState>,
    chat_id: String,
    tree: FileTreeModel,
    request_context: Option<FilesRequestContext>,
    loads: HashMap<(String, Option<String>), Task<()>>,
    watch_task: Option<Task<()>>,
    watch_sequence: Option<u64>,
    watch_error: Option<SharedString>,
    error: Option<SharedString>,
    started: bool,
    /// The sidebar's resting width, SHARED by every surface of the panel so
    /// a drag on one tab persists across tab switches (per-surface copies
    /// used to make each tab rest at a different width).
    sidebar_width: f32,
    /// Whether the sidebar is collapsed, SHARED so the state survives tab
    /// switches and returns from the raw workspace (user request: the
    /// collapse must be remembered).
    sidebar_collapsed: bool,
    tree_scroll: UniformListScrollHandle,
    tree_focus: FocusHandle,
    tree_bar: popover::MenuScrollbarState,
}

impl popover::ScrollRailHost for FileTreeView {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        &mut self.tree_bar
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        Some(self.tree_scroll.0.borrow().base_handle.clone())
    }
}

impl FileTreeView {
    pub fn new(
        state: Entity<AppState>,
        chat_id: String,
        show_all_files: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let tree_focus = cx.focus_handle();
        let surface = Self {
            state,
            chat_id,
            tree: FileTreeModel::with_include_ignored(show_all_files),
            request_context: None,
            loads: HashMap::new(),
            watch_task: None,
            watch_sequence: None,
            watch_error: None,
            error: None,
            started: false,
            sidebar_width: TREE_SPLIT_DEFAULT,
            sidebar_collapsed: false,
            tree_scroll: UniformListScrollHandle::new(),
            tree_focus,
            tree_bar: popover::MenuScrollbarState::default(),
        };
        cx.notify();
        surface
    }

    // -- Shell-facing API -------------------------------------------------

    /// Load the workspace once; later calls only sync the target and keep
    /// the single watcher running. Idempotent.
    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        self.sync_target(cx);
        if self.request_context.is_none() {
            return;
        }
        self.ensure_watch(cx);
        if self.started {
            return;
        }
        self.started = true;
        self.load_directory(String::new(), None, cx);
    }

    /// Persisted setting applied to every tree by the shell.
    pub fn set_show_all_files(&mut self, show_all_files: bool, cx: &mut Context<Self>) {
        if self.tree.set_include_ignored(show_all_files) {
            self.loads.clear();
            self.error = None;
            self.started = true;
            self.load_directory(String::new(), None, cx);
        }
    }

    /// The header's show-all toggle: the shell persists the setting and
    /// applies it back through [`Self::set_show_all_files`].
    pub fn toggle_ignored(&mut self, cx: &mut Context<Self>) {
        cx.emit(WorkspaceTreeEvent::ShowAllFilesChanged(
            !self.tree.include_ignored(),
        ));
    }

    pub fn collapse_all(&mut self, cx: &mut Context<Self>) {
        let expanded = self.tree.expanded_directories();
        for dir in expanded {
            self.tree.toggle_expanded(&dir);
        }
        cx.notify();
    }

    pub fn retry_root(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        self.started = true;
        self.load_directory(String::new(), None, cx);
    }

    /// Invalidate every loaded directory and reload the expanded ones.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.tree.invalidate_all_directories();
        let directories = std::iter::once(String::new())
            .chain(self.tree.expanded_directories())
            .collect::<Vec<_>>();
        for directory in directories {
            self.load_directory(directory, None, cx);
        }
    }

    /// Search-result reveal: select a path after loading its ancestor
    /// pages (`(page, ancestor-to-expand)` pairs, root first) and scroll it
    /// into view.
    pub fn reveal_search_result(
        &mut self,
        path: &str,
        pages: Vec<(zeron_proto::WorkspaceDirectoryPage, String)>,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        if self.tree.generation() != generation {
            return;
        }
        for (page, next) in pages {
            self.tree.apply_page(page, generation);
            self.tree.expand(&next);
        }
        self.tree.select(path.to_string());
        self.reveal_tree_selection();
        cx.notify();
    }

    pub fn generation(&self) -> u64 {
        self.tree.generation()
    }

    pub fn has_content(&self) -> bool {
        self.tree.node("").is_some_and(|node| node.has_loaded)
    }

    pub fn phase(&self) -> Option<DirectoryLoadState> {
        self.tree.node("").map(|root| root.load.clone())
    }

    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    pub fn watch_error(&self) -> Option<&SharedString> {
        self.watch_error.as_ref()
    }

    pub fn include_ignored(&self) -> bool {
        self.tree.include_ignored()
    }

    /// Whether a directory's children are already in the tree (no network
    /// needed to reveal through it).
    pub fn is_directory_loaded(&self, path: &str) -> bool {
        self.tree.is_directory_loaded(path)
    }

    /// Expand an already-loaded directory (search reveal fast path).
    pub fn expand_directory(&mut self, dir: &str, cx: &mut Context<Self>) {
        if self.tree.expand(dir) {
            cx.notify();
        }
    }

    /// Select a file path when it exists in the tree — the workspace's
    /// active info follows the ACTIVE file tab regardless of how it was
    /// opened (tree click, search, markdown link, mention).
    pub fn select_file(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.tree.node(path).is_some() {
            crate::ui_trace!("tree-select-file path={:?}", path);
            self.tree.select(path.to_string());
            self.reveal_tree_selection();
            cx.notify();
        }
    }

    /// The shared resting sidebar width (see the field docs).
    pub fn sidebar_width(&self) -> f32 {
        self.sidebar_width
    }

    pub fn set_sidebar_width(&mut self, width: f32) {
        self.sidebar_width = width.clamp(TREE_SPLIT_MIN, TREE_SPLIT_MAX);
    }

    pub fn reset_sidebar_width(&mut self) {
        self.sidebar_width = TREE_SPLIT_DEFAULT;
    }

    /// The shared collapse state (see the field docs).
    pub fn sidebar_collapsed(&self) -> bool {
        self.sidebar_collapsed
    }

    pub fn set_sidebar_collapsed(&mut self, collapsed: bool) {
        self.sidebar_collapsed = collapsed;
    }

    pub fn toggle_sidebar_collapsed(&mut self) -> bool {
        self.sidebar_collapsed = !self.sidebar_collapsed;
        self.sidebar_collapsed
    }

    // -- Target sync ------------------------------------------------------

    fn sync_target(&mut self, cx: &mut Context<Self>) -> bool {
        let next = FilesRequestContext::for_chat(self.state.read(cx), &self.chat_id);
        if self.request_context == next {
            return false;
        }
        self.apply_target(next);
        true
    }

    fn apply_target(&mut self, next: Option<FilesRequestContext>) {
        self.loads.clear();
        self.watch_task = None;
        self.watch_sequence = None;
        self.watch_error = None;
        self.tree.reset();
        self.error = if next.is_none() {
            Some("No workspace available for this chat.".into())
        } else {
            None
        };
        self.request_context = next;
        self.started = false;
    }

    // -- Directory loading (single-flight for the whole panel) ------------

    fn load_directory(
        &mut self,
        directory: String,
        cursor: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(request_context) = self.request_context.clone() else {
            self.error = Some("No workspace available for this chat.".into());
            cx.notify();
            return;
        };
        crate::ui_trace!("tree-load dir={:?} cursor={:?}", directory, cursor);
        let generation = self.tree.generation();
        let cached_paths = if cursor.is_none() {
            self.tree
                .node(&directory)
                .map(|node| node.children.clone())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        if !self.tree.begin_load(&directory, cursor.clone(), generation) {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.tree.fail_load(
                &directory,
                cursor,
                "Workspace service is still starting.",
                generation,
            );
            cx.notify();
            return;
        };
        let key = (directory.clone(), cursor.clone());
        let request = ListWorkspaceDirectoryRequest {
            target: request_context.target.clone(),
            directory: directory.clone(),
            include_ignored: self.tree.include_ignored(),
            cursor: cursor.clone(),
        };
        let client = WorkspaceFilesClient::new(engine, request_context);
        let task = cx.spawn(async move |this, cx| {
            let mut result = client
                .list_directory_snapshot(request.clone(), &cached_paths)
                .await;
            if result.as_ref().is_err_and(|error| error.retryable()) {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                result = client.list_directory_snapshot(request, &cached_paths).await;
            }
            let _ = this.update(cx, |tree, cx| {
                if tree.tree.generation() != generation {
                    return;
                }
                let reload = tree.tree.node(&directory).is_some_and(|node| node.stale);
                match result {
                    Ok(page) => {
                        tree.error = None;
                        tree.tree.apply_page(page, generation);
                    }
                    Err(error) => {
                        let message = error.to_string();
                        if directory.is_empty() {
                            tree.error = Some(message.clone().into());
                        }
                        tree.tree.fail_load(&directory, cursor, message, generation);
                    }
                }
                if reload {
                    tree.load_directory(directory, None, cx);
                }
                cx.notify();
            });
        });
        self.loads.insert(key, task);
        cx.notify();
    }

    // -- Workspace watcher (one per panel) --------------------------------

    fn ensure_watch(&mut self, cx: &mut Context<Self>) {
        if self.watch_task.is_some() {
            return;
        }
        let Some(context) = self.request_context.clone() else {
            return;
        };
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let client = WorkspaceFilesClient::new(engine, context);
        self.watch_task = Some(cx.spawn(async move |this, cx| {
            loop {
                match client.watch().await {
                    Ok(mut receiver) => {
                        let _ = this.update(cx, |tree, cx| {
                            tree.watch_error = None;
                            cx.notify();
                        });
                        while let Some(value) = receiver.recv().await {
                            let frame = serde_json::from_value::<WorkspaceFileChanges>(value);
                            if this
                                .update(cx, |tree, cx| match frame {
                                    Ok(frame) => tree.apply_tree_changes(frame, cx),
                                    Err(error) => {
                                        tree.watch_error = Some(
                                            format!("File updates could not be decoded: {error}")
                                                .into(),
                                        );
                                        cx.notify();
                                    }
                                })
                                .is_err()
                            {
                                return;
                            }
                        }
                        if this
                            .update(cx, |tree, cx| {
                                tree.watch_error =
                                    Some("File updates interrupted — retrying".into());
                                cx.notify();
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(error) => {
                        if this
                            .update(cx, |tree, cx| {
                                tree.watch_error = Some(error.to_string().into());
                                cx.notify();
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                }
                cx.background_executor().timer(Duration::from_secs(2)).await;
            }
        }));
    }

    /// Tree-side of a watch frame: mutations, invalidation, and reloads of
    /// the affected expanded directories. The shell fans the frame out to
    /// the panel's surfaces for the document side.
    fn apply_tree_changes(&mut self, frame: WorkspaceFileChanges, cx: &mut Context<Self>) {
        let resync =
            frame.resync_required || sequence_needs_resync(self.watch_sequence, frame.sequence);
        tracing::trace!(
            sequence = frame.sequence,
            previous_sequence = ?self.watch_sequence,
            resync_required = frame.resync_required,
            gap = resync,
            change_count = frame.changes.len(),
            "workspace file changes received by the tree"
        );
        crate::ui_trace!(
            "watch-frame seq={} changes={} resync={}",
            frame.sequence,
            frame.changes.len(),
            resync
        );
        self.watch_sequence = Some(frame.sequence);
        if resync {
            self.refresh(cx);
            cx.emit(WorkspaceTreeEvent::DocumentsChanged { frame, resync });
            return;
        }

        let mut parents = std::collections::HashSet::new();
        for change in &frame.changes {
            if let Some(old_path) = &change.old_path {
                self.tree.remove(old_path);
                if let Some(parent) = parent_path(old_path) {
                    parents.insert(parent);
                }
            }
            match change.kind {
                WorkspaceFileChangeKind::Created | WorkspaceFileChangeKind::Renamed => {
                    if let Some(parent) = parent_path(&change.path) {
                        parents.insert(parent);
                    }
                }
                WorkspaceFileChangeKind::Modified => {}
                WorkspaceFileChangeKind::Removed => {
                    self.tree.remove(&change.path);
                    if let Some(parent) = parent_path(&change.path) {
                        parents.insert(parent);
                    }
                }
            }
        }

        for parent in &parents {
            self.tree.invalidate_directory(parent);
        }
        let reload = parents
            .into_iter()
            .filter(|parent| self.tree.is_expanded(parent))
            .collect::<Vec<_>>();
        for parent in reload {
            self.load_directory(parent, None, cx);
        }
        cx.emit(WorkspaceTreeEvent::DocumentsChanged { frame, resync });
        cx.notify();
    }

    // -- Row activation ---------------------------------------------------

    fn activate_path(&mut self, path: String, cx: &mut Context<Self>) {
        crate::ui_trace!("tree-select path={:?}", path);
        self.tree.select(path.clone());
        let is_directory = self
            .tree
            .node(&path)
            .is_some_and(|node| node.entry.kind == WorkspaceEntryKind::Directory);
        if is_directory {
            self.toggle_directory(path, cx);
        } else {
            crate::ui_trace!("tree-open path={:?}", path);
            cx.emit(WorkspaceTreeEvent::OpenFile(path));
        }
        self.reveal_tree_selection();
        cx.notify();
    }

    fn toggle_directory(&mut self, path: String, cx: &mut Context<Self>) {
        if !self.tree.toggle_expanded(&path) {
            return;
        }
        let needs_load = self.tree.node(&path).is_some_and(|node| {
            node.stale
                || matches!(
                    node.load,
                    DirectoryLoadState::Unloaded | DirectoryLoadState::Error { .. }
                )
        });
        if self.tree.is_expanded(&path) && needs_load {
            self.load_directory(path, None, cx);
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let handled = match event.keystroke.key.as_str() {
            "up" => {
                self.tree.select_previous();
                true
            }
            "down" => {
                self.tree.select_next();
                true
            }
            "left" => {
                let selected = self.tree.selected().map(str::to_string);
                if let Some(path) = selected {
                    if self.tree.is_expanded(&path) {
                        self.tree.toggle_expanded(&path);
                    } else {
                        self.tree.select_parent();
                    }
                }
                true
            }
            "right" => {
                let selected = self.tree.selected().map(str::to_string);
                if let Some(path) = selected
                    && self
                        .tree
                        .node(&path)
                        .is_some_and(|node| node.entry.kind == WorkspaceEntryKind::Directory)
                {
                    if self.tree.is_expanded(&path) {
                        self.tree.select_first_child();
                    } else {
                        self.toggle_directory(path, cx);
                    }
                }
                true
            }
            "enter" | "space" => {
                if let Some(path) = self.tree.selected().map(str::to_string) {
                    self.activate_path(path, cx);
                }
                true
            }
            _ => false,
        };
        if handled {
            window.prevent_default();
            cx.stop_propagation();
            self.reveal_tree_selection();
            cx.notify();
        }
    }

    pub(super) fn reveal_tree_selection(&self) {
        let Some(selected) = self.tree.selected() else {
            return;
        };
        if let Some(index) = self
            .tree
            .visible_rows()
            .iter()
            .position(|row| row.path == selected)
        {
            self.tree_scroll
                .scroll_to_item(index, ScrollStrategy::Nearest);
        }
    }

    // -- Rendering --------------------------------------------------------

    fn render_tree_rows(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        // One theme/scale read per range pass: rows are rebuilt on every
        // scroll and hover repaint, so per-row lookups dominated row cost.
        let theme = Theme::of(cx).clone();
        let scale = crate::typography::font_size(cx).pixels() / 16.0;
        range
            .map(|index| self.render_tree_row(index, &theme, scale, window, cx))
            .collect()
    }

    fn render_tree_row(
        &mut self,
        index: usize,
        theme: &Theme,
        scale: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(row) = self.tree.visible_rows().get(index) else {
            return gpui::Empty.into_any_element();
        };
        let indent = TREE_INDENT * scale;
        let base_pad = TREE_BASE_PAD * scale;

        match &row.kind {
            VisibleRowKind::Entry => {
                let Some((name, kind, ignored)) = self
                    .tree
                    .node(&row.path)
                    .map(|n| (n.entry.name.clone(), n.entry.kind, n.entry.ignored))
                else {
                    return gpui::Empty.into_any_element();
                };
                let path = row.path.clone();
                let selected = self.tree.selected() == Some(path.as_str());
                let focused = self.tree_focus.is_focused(window);
                let is_directory = kind == WorkspaceEntryKind::Directory;
                let drag_payload = if crate::click_activation_drag_enabled() {
                    Some(WorkspacePathDrag::new(path.clone(), is_directory))
                } else {
                    None
                };
                let expanded = is_directory && self.tree.is_expanded(&path);
                let text_color = if selected {
                    theme.text
                } else if is_directory {
                    theme.text.opacity(0.92)
                } else {
                    theme.text_muted
                };
                let file_identity = match kind {
                    WorkspaceEntryKind::Directory => FileIconIdentity::directory(&name, expanded),
                    WorkspaceEntryKind::File => FileIconIdentity::file(&name),
                    WorkspaceEntryKind::Symlink => FileIconIdentity::symlink(&name),
                };

                let content_left = base_pad + (row.depth as f32 * indent);

                let row_el = div()
                    .id(SharedString::from(format!("files-tree-entry:{path}")))
                    .role(gpui::Role::TreeItem)
                    .aria_label(name.clone())
                    .aria_selected(selected)
                    .when(is_directory, |element| element.aria_expanded(expanded))
                    .h(crate::typography::ui_rems(TREE_ROW_HEIGHT))
                    .w_full()
                    .flex_none()
                    .px(crate::typography::ui_rems(8.0))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.tree_focus.focus(window, cx);
                        this.activate_path(path.clone(), cx);
                    }))
                    .when_some(drag_payload, |element, payload| {
                        element.on_drag(payload, |payload, _, _, cx| {
                            cx.stop_propagation();
                            workspace_path_drag_ghost(payload, cx)
                        })
                    })
                    // The fork's hover-style transition doesn't schedule a
                    // window draw, so the hover highlight would only appear
                    // at the next unrelated draw. Notify this (tree-only)
                    // view from a listener, which does schedule one, and
                    // keep the hover style for the visuals.
                    .on_hover(cx.listener(|_, _, _, cx| cx.notify()));

                let mut inner = div()
                    .relative()
                    .size_full()
                    .rounded(crate::typography::ui_rems(6.0))
                    .when(selected, |element| {
                        element
                            .border_1()
                            .border_color(if focused {
                                theme.accent
                            } else {
                                theme.accent.opacity(0.45)
                            })
                            .bg(if focused {
                                theme.accent.opacity(0.12)
                            } else {
                                theme.accent.opacity(0.06)
                            })
                    })
                    .when(!selected, |element| {
                        element.hover(|style| style.bg(crate::theme::wash(0.055)))
                    })
                    .pl(px(content_left))
                    .pr(crate::typography::ui_rems(10.0))
                    .flex()
                    .items_center()
                    .gap(crate::typography::ui_rems(6.0))
                    .when(ignored, |element| element.opacity(0.52));

                // Tree indentation guidelines: 1px vertical line for each ancestor level
                let guide_color = if selected {
                    theme.accent.opacity(0.45)
                } else if theme.appearance.is_dark() {
                    theme.text_faint.opacity(0.28)
                } else {
                    theme.border_strong
                };
                for level in 0..row.depth {
                    let line_x = base_pad + (level as f32 * indent) + (ICON_SIZE * 0.5 * scale);
                    inner = inner.child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(px(line_x))
                            .w(px(1.0))
                            .bg(guide_color),
                    );
                }

                inner = inner
                    .child(
                        file_icons::icon(file_identity, theme.appearance)
                            .size(crate::typography::ui_rems(ICON_SIZE))
                            .flex_none(),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_family(theme.font_sans.clone())
                            .text_size(crate::typography::ui_rems(13.0))
                            .when(selected, |el| el.font_weight(gpui::FontWeight::MEDIUM))
                            .text_color(text_color)
                            .child(name),
                    );

                row_el.child(inner).into_any_element()
            }
            VisibleRowKind::Loading { .. } => status_row(
                &row.path,
                row.depth,
                "Loading…",
                theme.text_faint,
                scale,
                theme,
            ),
            VisibleRowKind::Empty { .. } => status_row(
                &row.path,
                row.depth,
                "Empty folder",
                theme.text_faint.opacity(0.7),
                scale,
                theme,
            ),
            VisibleRowKind::Error { directory, message } => {
                let directory = directory.clone();
                let content_left = base_pad + (row.depth as f32 * indent);
                let row_el = div()
                    .id(SharedString::from(format!("files-tree-error:{}", row.path)))
                    .h(crate::typography::ui_rems(TREE_ROW_HEIGHT))
                    .w_full()
                    .flex_none()
                    .px(crate::typography::ui_rems(8.0))
                    .cursor_pointer()
                    .hover(|style| style.bg(crate::theme::wash(0.055)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let cursor = this
                            .tree
                            .node(&directory)
                            .and_then(|node| match &node.load {
                                DirectoryLoadState::Error { cursor, .. } => cursor.clone(),
                                _ => None,
                            });
                        this.load_directory(directory.clone(), cursor, cx);
                    }));

                let mut inner = div()
                    .relative()
                    .size_full()
                    .rounded(crate::typography::ui_rems(6.0))
                    .pl(px(content_left))
                    .pr(crate::typography::ui_rems(10.0))
                    .flex()
                    .items_center()
                    .gap(crate::typography::ui_rems(6.0));

                let guide_color = if theme.appearance.is_dark() {
                    theme.text_faint.opacity(0.28)
                } else {
                    theme.border_strong
                };
                for level in 0..row.depth {
                    let line_x = base_pad + (level as f32 * indent) + (ICON_SIZE * 0.5 * scale);
                    inner = inner.child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(px(line_x))
                            .w(px(1.0))
                            .bg(guide_color),
                    );
                }

                inner = inner.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(crate::typography::ui_rems(11.0))
                        .text_color(theme.danger.opacity(0.82))
                        .child(format!("{message} — Retry")),
                );

                row_el.child(inner).into_any_element()
            }
            VisibleRowKind::LoadMore { directory, cursor } => {
                let directory = directory.clone();
                let cursor = cursor.clone();
                let content_left = base_pad + (row.depth as f32 * indent);
                let row_el = div()
                    .id(SharedString::from(format!("files-tree-more:{}", row.path)))
                    .h(crate::typography::ui_rems(TREE_ROW_HEIGHT))
                    .w_full()
                    .flex_none()
                    .px(crate::typography::ui_rems(8.0))
                    .cursor_pointer()
                    .hover(|style| style.bg(crate::theme::wash(0.055)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.load_directory(directory.clone(), Some(cursor.clone()), cx);
                    }));

                let mut inner = div()
                    .relative()
                    .size_full()
                    .rounded(crate::typography::ui_rems(6.0))
                    .pl(px(content_left))
                    .pr(crate::typography::ui_rems(10.0))
                    .flex()
                    .items_center();

                let guide_color = if theme.appearance.is_dark() {
                    theme.text_faint.opacity(0.28)
                } else {
                    theme.border_strong
                };
                for level in 0..row.depth {
                    let line_x = base_pad + (level as f32 * indent) + (ICON_SIZE * 0.5 * scale);
                    inner = inner.child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(px(line_x))
                            .w(px(1.0))
                            .bg(guide_color),
                    );
                }

                inner = inner.child(
                    div()
                        .text_size(crate::typography::ui_rems(11.0))
                        .text_color(theme.text_muted)
                        .child("Load more…"),
                );

                row_el.child(inner).into_any_element()
            }
        }
    }
}

impl Render for FileTreeView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let scrollbar = popover::rail(self, "files-tree-scrollbar", &theme, cx);
        let row_count = self.tree.visible_rows().len();
        if crate::ui_trace::enabled() {
            let b = self.tree_scroll.0.borrow().base_handle.bounds();
            let item_h = self
                .tree_scroll
                .0
                .borrow()
                .last_item_size
                .map(|size| f32::from(size.contents.height))
                .unwrap_or(0.0);
            let rows = self
                .tree
                .visible_rows()
                .iter()
                .take(24)
                .map(|row| {
                    let kind = self
                        .tree
                        .node(&row.path)
                        .map(|node| {
                            if node.entry.kind == WorkspaceEntryKind::Directory {
                                'd'
                            } else {
                                'f'
                            }
                        })
                        .unwrap_or('?');
                    format!("{}:{}", kind, row.path)
                })
                .collect::<Vec<_>>()
                .join("|");
            eprintln!(
                "[trace] tree-rows count={} L={:.0} T={:.0} R={:.0} B={:.0} content_h={:.0} paths={}",
                row_count,
                f32::from(b.left()),
                f32::from(b.top()),
                f32::from(b.right()),
                f32::from(b.bottom()),
                item_h,
                rows
            );
        }
        div()
            .id("files-tree")
            .role(gpui::Role::Tree)
            .aria_label("Workspace file tree")
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .track_focus(&self.tree_focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.tree_focus.focus(window, cx)),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.on_key_down(event, window, cx);
            }))
            .child(
                uniform_list(
                    "files-tree-list",
                    row_count,
                    cx.processor(Self::render_tree_rows),
                )
                .flex_1()
                .min_h_0()
                .track_scroll(&self.tree_scroll),
            )
            .children(scrollbar)
    }
}

pub(super) fn sequence_needs_resync(previous: Option<u64>, next: u64) -> bool {
    previous.is_some_and(|previous| next != previous.saturating_add(1))
}

fn status_row(
    path: &str,
    depth: usize,
    label: &'static str,
    color: gpui::Hsla,
    scale: f32,
    theme: &Theme,
) -> AnyElement {
    let base_pad = TREE_BASE_PAD * scale;
    let indent = TREE_INDENT * scale;
    let content_left = base_pad + (depth as f32 * indent);
    let mut inner = div()
        .relative()
        .size_full()
        .rounded(crate::typography::ui_rems(6.0))
        .pl(px(content_left))
        .pr(crate::typography::ui_rems(10.0))
        .flex()
        .items_center();

    let guide_color = if theme.appearance.is_dark() {
        theme.text_faint.opacity(0.28)
    } else {
        theme.border_strong
    };
    for level in 0..depth {
        let line_x = base_pad + (level as f32 * indent) + (ICON_SIZE * 0.5 * scale);
        inner = inner.child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(line_x))
                .w(px(1.0))
                .bg(guide_color),
        );
    }

    inner = inner.child(
        div()
            .font_family(theme.font_sans.clone())
            .text_size(crate::typography::ui_rems(13.0))
            .text_color(color)
            .child(label),
    );

    div()
        .id(SharedString::from(format!("files-tree-status:{path}")))
        .h(crate::typography::ui_rems(TREE_ROW_HEIGHT))
        .w_full()
        .flex_none()
        .px(crate::typography::ui_rems(8.0))
        .child(inner)
        .into_any_element()
}
