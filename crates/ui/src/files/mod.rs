//! Workspace file browsing surface.

use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use gpui::{
    App, Context, Entity, EventEmitter, ListAlignment, ListState, Pixels, Point, Render,
    SharedString, Subscription, Task, Window, div, prelude::*, px,
};

use crate::{
    composer::{ComposerInput, ComposerInputEvent},
    state::AppState,
};

pub mod client;
pub mod document;
pub mod editor;
pub mod editor_adapter;
mod image_preview;
pub(crate) mod markdown_media;
mod markdown_preview;
pub mod model;
pub mod preview;
pub mod search;
mod tree_view;
pub mod watch;

pub use tree_view::{FileTreeView, WorkspaceTreeEvent};

use client::FilesRequestContext;
use model::DirectoryLoadState;
use preview::FilePreviewState;
use search::FileSearchState;

static NEXT_REVIEW_COMMENT_FLUSH_SOURCE: AtomicU64 = AtomicU64::new(1);
use crate::surface_chrome::{
    CONTROL_RADIUS as TOOLBAR_BUTTON_RADIUS, CONTROL_SIZE as TOOLBAR_BUTTON_SIZE, toolbar,
};

pub(super) fn toolbar_button(id: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .size(crate::typography::ui_rems(TOOLBAR_BUTTON_SIZE))
        .flex_none()
        .rounded(crate::typography::ui_rems(TOOLBAR_BUTTON_RADIUS))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .role(gpui::Role::Button)
        .aria_label(label)
        .occlude()
        .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
            window.prevent_default()
        })
        .hover(|style| style.bg(crate::theme::wash(0.14)))
        .tooltip(move |_, cx| {
            cx.new(|_| preview::FileEditorTooltip { text: label.into() })
                .into()
        })
        .tooltip_show_delay(Duration::from_millis(350))
}

/// A workspace-relative file or directory dragged out of a Files surface.
///
/// Keeping the payload relative is important: the composer may target a
/// remote device, and its existing file-mention transport resolves paths in
/// that workspace instead of leaking a path from the UI machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspacePathDrag {
    pub path: String,
    pub is_directory: bool,
}

impl WorkspacePathDrag {
    pub(crate) fn new(path: String, is_directory: bool) -> Self {
        Self { path, is_directory }
    }

    fn title(&self) -> SharedString {
        self.path
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(&self.path)
            .to_string()
            .into()
    }
}

/// Compact drag preview shared by tree and search rows. It deliberately uses
/// the same raised surface, hairline, type scale, and opacity as surface tabs.
pub(crate) struct WorkspacePathDragGhost {
    payload: WorkspacePathDrag,
}

impl Render for WorkspacePathDragGhost {
    fn render(&mut self, _window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = crate::theme::Theme::of(cx);
        div()
            .h(px(24.0))
            .max_w(px(220.0))
            .px(px(8.0))
            .flex()
            .items_center()
            .gap(px(5.0))
            .rounded(px(6.0))
            .bg(theme.surface_raised)
            .border_1()
            .border_color(theme.border_strong)
            .text_size(px(11.5))
            .text_color(theme.text)
            .opacity(0.85)
            .child({
                let identity = if self.payload.is_directory {
                    crate::file_icons::FileIconIdentity::directory(&self.payload.path, false)
                } else {
                    crate::file_icons::FileIconIdentity::file(&self.payload.path)
                };
                crate::file_icons::icon(identity, theme.appearance)
                    .size(px(14.0))
                    .flex_none()
            })
            .child(div().min_w_0().truncate().child(self.payload.title()))
    }
}

pub(crate) fn workspace_path_drag_ghost(
    payload: &WorkspacePathDrag,
    cx: &mut gpui::App,
) -> gpui::Entity<WorkspacePathDragGhost> {
    let payload = payload.clone();
    cx.new(|_| WorkspacePathDragGhost { payload })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilesEvent {
    OpenFile(String),
    OpenWebLink(crate::markdown::render::LinkActivation),
    TitleChanged,
    FileRenamed { old_path: String, new_path: String },
    WordWrapChanged(bool),
    ShowAllFilesChanged(bool),
    CloseReady,
    CloseCancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilesCloseDisposition {
    Allow,
    Pending,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FilesPresentation {
    Browser,
    Editor,
}

impl FilesPresentation {
    fn is_editor(self) -> bool {
        matches!(self, Self::Editor)
    }
}

impl EventEmitter<FilesEvent> for FilesSurface {}

struct EditorContextMenu {
    editor: Entity<editor::FileEditorState>,
    position: Point<Pixels>,
    availability: editor::EditorMenuAvailability,
}

pub struct FilesSurface {
    state: Entity<AppState>,
    chat_id: String,
    review_comment_flush_source: u64,
    presentation: FilesPresentation,
    editor_path: Option<String>,
    request_context: Option<FilesRequestContext>,
    target_change_pending: bool,
    pending_request_context: Option<FilesRequestContext>,
    /// The panel's SHARED workspace tree, owned by the shell. Every Files
    /// surface (browser + editor tabs) embeds the same entity, so tree
    /// state, loads, and the watcher exist once per panel.
    tree_view: Entity<FileTreeView>,
    /// Cached window handle for the animation driver's forced draws.
    window_handle: Option<gpui::AnyWindowHandle>,
    /// Frame driver for tree transitions: the platform's
    /// request_animation_frame path doesn't schedule frames on Windows, so
    /// an 8 ms timer notifies while an animation is in flight (the
    /// input-driven notify path is the one proven to draw at vsync rate).
    anim_driver: Option<Task<()>>,
    preview: FilePreviewState,
    editor_context_menu: crate::popover::Popup<EditorContextMenu>,
    actions_menu: crate::popover::Popup<()>,
    _observe: Subscription,
}

impl Render for FilesSurface {
    fn render(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.window_handle.is_none() {
            self.window_handle = Some(window.window_handle());
        }
        let theme = crate::theme::Theme::of(cx).clone();
        // Tree load state comes from the shared tree entity; reading it here
        // also re-renders this surface when the tree notifies.
        let (phase, tree_has_content, tree_error) = self.tree_view.read_with(cx, |tree, _| {
            (tree.phase(), tree.has_content(), tree.error().cloned())
        });
        // NOTE: the fuzzy search results render INSIDE the shared tree view
        // (its render swaps rows for results), so no surface-side branch.
        let content = if let Some(error) = tree_error.filter(|_| !tree_has_content) {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.0))
                .px(px(28.0))
                .child(
                    div()
                        .text_center()
                        .text_size(px(12.0))
                        .text_color(theme.text_muted)
                        .child(error),
                )
                .child(
                    div()
                        .id("files-retry-root")
                        .h(px(28.0))
                        .px(px(12.0))
                        .rounded(px(7.0))
                        .border_1()
                        .border_color(theme.border)
                        .bg(crate::theme::wash(0.04))
                        .hover(|style| style.bg(crate::theme::wash(0.09)))
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .text_size(crate::typography::ui_rems(13.0))
                        .text_color(theme.text)
                        .child("Retry")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.tree_view.update(cx, |tree, cx| tree.retry_root(cx));
                        })),
                )
                .into_any_element()
        } else if !tree_has_content
            && matches!(
                phase.as_ref(),
                Some(DirectoryLoadState::Unloaded | DirectoryLoadState::Loading { .. })
            )
        {
            div().flex_1().into_any_element()
        } else {
            self.render_tree(cx)
        };
        let split_editor = self.presentation.is_editor();
        let watch_error = self
            .tree_view
            .read_with(cx, |tree, _| tree.watch_error().cloned());
        let tree_pane = div()
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .when(!split_editor, |pane| {
                pane.child(self.render_header(&theme, false, cx))
            })
            .when_some(watch_error, |element, error| {
                element.child(
                    div()
                        .h(crate::typography::ui_rems(30.0))
                        .flex_none()
                        .px(crate::typography::ui_rems(10.0))
                        .border_b_1()
                        .border_color(theme.warning.opacity(0.22))
                        .bg(theme.warning.opacity(0.045))
                        .flex()
                        .items_center()
                        .gap(crate::typography::ui_rems(6.0))
                        .text_size(crate::typography::ui_rems(12.5))
                        .text_color(theme.warning_muted)
                        .child(
                            crate::icons::icon(crate::icons::REFRESH)
                                .size(crate::typography::ui_rems(13.0))
                                .flex_none(),
                        )
                        .child(div().min_w_0().flex_1().truncate().child(error))
                        .child(
                            div()
                                .id("files-watch-refresh-now")
                                .h(crate::typography::ui_rems(22.0))
                                .flex_none()
                                .px(crate::typography::ui_rems(8.0))
                                .rounded(crate::typography::ui_rems(5.0))
                                .flex()
                                .items_center()
                                .cursor_pointer()
                                .role(gpui::Role::Button)
                                .aria_label("Refresh workspace files now")
                                .text_size(crate::typography::ui_rems(12.0))
                                .text_color(theme.text_muted)
                                .hover(|style| style.bg(crate::theme::wash(0.07)))
                                .child("Refresh now")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.tree_view.update(cx, |tree, cx| tree.refresh(cx));
                                    this.reconcile_open_documents(cx);
                                })),
                        ),
                )
            })
            .child(content);
        let is_editor = self.presentation.is_editor();
        let mut header = None;
        let mut preview_split_right = None;
        let body = if split_editor {
            let wide = self.preview.is_wide();
            // OVERLAY TREE LAYOUT: the preview renders at its FINAL width
            // (pane minus the state-reserved sidebar width) and never
            // re-lays-out during tree animations — the tree is a
            // right-anchored overlay whose width animates ON TOP of the
            // frozen preview. That keeps every transition repaint cheap
            // (no per-frame text re-wrap) and the file content visible
            // until the tree covers it.
            let collapsed = self
                .tree_view
                .read_with(cx, |tree, _| tree.sidebar_collapsed());
            let resting = self.resting_tree_width(cx);
            let visible = !collapsed;
            let layout_width = self.preview.sidebar_layout_width(resting, visible);
            let overlay_width = self.preview.tree_overlay_width(resting, visible, window, cx);
            if crate::ui_trace::enabled() {
                static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
                let t0 = T0.get_or_init(std::time::Instant::now);
                eprintln!(
                    "[trace] split-frame t={} w={:.0} layout={:.0} surf={:.0} wide={} collapsed={} tween={:?}",
                    t0.elapsed().as_millis(),
                    overlay_width,
                    layout_width,
                    self.preview.surface_width_read(),
                    wide,
                    collapsed,
                    self.preview.tree_width_tween_debug()
                );
            }
            if wide && visible {
                preview_split_right =
                    Some(overlay_width - preview::TREE_SPLIT_HITBOX_HALF_WIDTH);
            }
            // Same arrangement as the outer right-sidebar toggle: the trigger
            // is outside the animated controls, in a permanently mounted slot.
            let toggle_width = f32::from(
                crate::typography::ui_rems(
                    crate::surface_chrome::CONTROL_SIZE + crate::surface_chrome::EDGE_INSET,
                )
                .to_pixels(window.rem_size()),
            );
            let tree_header = self
                .render_header(&theme, true, cx)
                .h_full()
                .pr(crate::typography::ui_rems(
                    crate::surface_chrome::CONTROL_GAP,
                ));
            // The tree header rides the SAME animated width as the body
            // overlay (workspace name and branch slide with the tree in
            // every transition), covering the frozen editor header instead
            // of squeezing it — no per-frame breadcrumb re-flow.
            header = Some(
                div()
                    .w_full()
                    .h(crate::typography::ui_rems(
                        crate::surface_chrome::HEADER_HEIGHT,
                    ))
                    .flex_none()
                    .flex()
                    .relative()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .children(self.render_editor_header(&theme, cx)),
                    )
                    .child(div().flex_none().w(px(
                        (layout_width - toggle_width).max(0.0),
                    )))
                    .child(self.render_tree_toggle(&theme, cx))
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .right(px(toggle_width))
                            .w(px((overlay_width - toggle_width).max(0.0)))
                            .overflow_hidden()
                            .bg(theme.bg)
                            .border_l_1()
                            .border_color(theme.border)
                            .child(tree_header)
                            .child(crate::ui_trace::bounds_probe("tree-header")),
                    ),
            );

            div()
                .size_full()
                .min_w_0()
                .relative()
                .flex()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(self.render_preview(window, cx)),
                )
                .child(div().flex_none().w(px(layout_width.max(0.0))))
                // The overlay tree: right-anchored, animated width, painted
                // above the frozen preview (later sibling). OPAQUE: the
                // file content beneath must never bleed through the tree's
                // transparent rows — that read as flickering during every
                // overlay transition. (theme.bg is opaque; ink(0.0) is
                // fully transparent and was a no-op here.)
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .right_0()
                        .w(px(overlay_width.max(0.0)))
                        .overflow_hidden()
                        .bg(theme.bg)
                        .child(
                            div()
                                .w(px(overlay_width.max(0.0)))
                                .h_full()
                                .relative()
                                .border_l_1()
                                .border_color(theme.border)
                                .flex()
                                .flex_col()
                                .child(tree_pane),
                        ),
                )
                .into_any_element()
        } else {
            tree_pane.into_any_element()
        };
        let measured_width = self.preview.width_cell();
        let entity = cx.entity();
        let editor_context_menu = self.render_editor_context_menu(&theme, cx);
        let preview_split_handle =
            preview_split_right.map(|right| self.preview_split_handle(right, cx));
        div()
            .id(SharedString::from(format!(
                "files-surface-{}",
                self.chat_id
            )))
            .role(gpui::Role::Group)
            .aria_label("Workspace files")
            .size_full()
            .relative()
            .flex()
            .bg(crate::theme::ink(0.0))
            .when(is_editor, |element| {
                element
                    .on_drag_move(cx.listener(Self::on_preview_split_drag))
                    .child(
                        gpui::canvas(
                            move |bounds, _, cx| {
                                let width = f32::from(bounds.size.width);
                                if (measured_width.get() - width).abs() > 1.0 {
                                    measured_width.set(width);
                                    entity.update(cx, |_, cx| cx.notify());
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    )
            })
            .flex_col()
            .children(header)
            .child(div().flex_1().min_h_0().w_full().child(body))
            .children(preview_split_handle)
            .children(editor_context_menu)
            .into_any_element()
    }
}

impl FilesSurface {
    pub fn new(
        state: Entity<AppState>,
        chat_id: String,
        tree_view: Entity<FileTreeView>,
        autosave_enabled: bool,
        autosave_delay_ms: u64,
        editor_font_size: f32,
        word_wrap: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with_presentation(
            state,
            chat_id,
            FilesPresentation::Browser,
            None,
            tree_view,
            autosave_enabled,
            autosave_delay_ms,
            editor_font_size,
            word_wrap,
            cx,
        )
    }

    pub fn new_editor(
        state: Entity<AppState>,
        chat_id: String,
        path: String,
        tree_view: Entity<FileTreeView>,
        initial_sidebar: Option<f32>,
        pane_width: f32,
        autosave_enabled: bool,
        autosave_delay_ms: u64,
        editor_font_size: f32,
        word_wrap: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut surface = Self::new_with_presentation(
            state,
            chat_id,
            FilesPresentation::Editor,
            Some(path),
            tree_view,
            autosave_enabled,
            autosave_delay_ms,
            editor_font_size,
            word_wrap,
            cx,
        );
        // Frame-1 correctness: the pane width primes the wide/narrow branch
        // (the canvas only measures after the first paint), the sidebar seed
        // starts the visual transition.
        surface.preview.seed_surface_width(pane_width);
        if let Some(from_width) = initial_sidebar {
            // A collapsed tree swipes away to the corner; an expanded one
            // eases to its shared resting width.
            let collapsed = surface
                .tree_view
                .read_with(cx, |tree, _| tree.sidebar_collapsed());
            let target = if collapsed {
                0.0
            } else {
                surface.resting_tree_width(cx)
            };
            surface.preview.seed_sidebar_transition(from_width, target);
            surface.drive_tree_animation(cx);
        }
        surface
    }

    fn new_with_presentation(
        state: Entity<AppState>,
        chat_id: String,
        presentation: FilesPresentation,
        editor_path: Option<String>,
        tree_view: Entity<FileTreeView>,
        autosave_enabled: bool,
        autosave_delay_ms: u64,
        editor_font_size: f32,
        word_wrap: bool,
        cx: &mut Context<Self>,
    ) -> Self {

        let observe = cx.observe(&state, |this: &mut Self, _, cx| {
            if this.sync_target(cx) {
                this.ensure_loaded(cx);
            }
            this.sync_active_markdown_comments(cx);
        });
        let mut surface = Self {
            state,
            chat_id,
            review_comment_flush_source: NEXT_REVIEW_COMMENT_FLUSH_SOURCE
                .fetch_add(1, Ordering::Relaxed),
            presentation,
            editor_path: editor_path.clone(),
            request_context: None,
            target_change_pending: false,
            pending_request_context: None,
            tree_view,
            window_handle: None,
            anim_driver: None,
            preview: FilePreviewState::new(
                autosave_enabled,
                autosave_delay_ms,
                word_wrap,
                editor_font_size,
            ),
            editor_context_menu: crate::popover::Popup::default(),
            actions_menu: crate::popover::Popup::default(),
            _observe: observe,
        };
        surface.sync_target(cx);
        surface
    }

    pub(in crate::files) fn open_editor_context_menu(
        &mut self,
        editor: Entity<editor::FileEditorState>,
        availability: editor::EditorMenuAvailability,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.editor_context_menu.open(EditorContextMenu {
            editor,
            position,
            availability,
        });
        cx.notify();
    }

    fn close_editor_context_menu(&mut self, cx: &mut Context<Self>) {
        if self.editor_context_menu.begin_close() {
            crate::popover::reap_popup(cx, |surface: &mut Self| &mut surface.editor_context_menu);
            cx.notify();
        }
    }

    fn dispatch_editor_context_action(
        &mut self,
        editor: Entity<editor::FileEditorState>,
        action: editor::EditorContextAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_editor_context_menu(cx);
        editor::dispatch_context_action(&editor, action, window, cx);
    }

    fn editor_context_menu_row(
        theme: &crate::theme::Theme,
        id: &'static str,
        label: &'static str,
        enabled: bool,
        editor: Entity<editor::FileEditorState>,
        action: editor::EditorContextAction,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        crate::popover::menu_row(theme, false, id)
            .id(id)
            .when(!enabled, |row| row.opacity(0.38).cursor_default())
            .when(enabled, |row| {
                row.on_click(cx.listener(move |this, _, window, cx| {
                    this.dispatch_editor_context_action(editor.clone(), action, window, cx)
                }))
            })
            .child(label)
            .into_any_element()
    }

    fn render_editor_context_menu(
        &mut self,
        theme: &crate::theme::Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let theme = &theme.for_popup();
        let menu = self.editor_context_menu.get()?;
        let editor = menu.editor.clone();
        let position = menu.position;
        let availability = menu.availability;
        let closing = self.editor_context_menu.closing_since();

        let card = crate::popover::popover_card(theme)
            .w(px(170.0))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_editor_context_menu(cx)))
            .flex()
            .flex_col()
            .child(Self::editor_context_menu_row(
                theme,
                "files-editor-context-cut",
                "Cut",
                availability.cut,
                editor.clone(),
                editor::EditorContextAction::Cut,
                cx,
            ))
            .child(Self::editor_context_menu_row(
                theme,
                "files-editor-context-copy",
                "Copy",
                availability.copy,
                editor.clone(),
                editor::EditorContextAction::Copy,
                cx,
            ))
            .child(Self::editor_context_menu_row(
                theme,
                "files-editor-context-paste",
                "Paste",
                availability.paste,
                editor.clone(),
                editor::EditorContextAction::Paste,
                cx,
            ))
            .child(crate::popover::menu_separator())
            .child(Self::editor_context_menu_row(
                theme,
                "files-editor-context-select-all",
                "Select All",
                true,
                editor,
                editor::EditorContextAction::SelectAll,
                cx,
            ))
            .into_any_element();

        Some(crate::popover::menu_at(
            "files-editor-context-menu",
            position,
            card,
            closing,
        ))
    }

    pub fn set_autosave_delay_ms(&mut self, delay_ms: u64, cx: &mut Context<Self>) {
        let pending = self.preview.set_autosave_delay_ms(delay_ms);
        for path in pending {
            self.schedule_autosave(path, cx);
        }
        cx.notify();
    }

    pub fn set_autosave_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        let pending = self.preview.set_autosave_enabled(enabled);
        for path in pending {
            self.schedule_autosave(path, cx);
        }
        cx.notify();
    }

    pub fn set_word_wrap(
        &mut self,
        word_wrap: bool,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_word_wrap(word_wrap, window, cx);
    }

    pub fn set_editor_font_size(&mut self, editor_font_size: f32, cx: &mut Context<Self>) {
        self.preview.set_editor_font_size(editor_font_size);
        cx.notify();
    }

    pub fn set_show_all_files(&mut self, _show_all_files: bool, cx: &mut Context<Self>) {
        // The tree side AND the search live on the shared FileTreeView; the
        // shell applies the setting there, and the search clears there.
        self.tree_view.update(cx, |tree, cx| tree.clear_search(cx));
    }

    /// Documents only: the shared tree entity loads the workspace once for
    /// the whole panel (the shell ensures it). This surface just keeps its
    /// request target in sync and opens its own editor file.
    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        self.sync_target(cx);
        if self.request_context.is_none() {
            return;
        }
        if self.presentation.is_editor()
            && !self.preview.has_active()
            && let Some(path) = self.editor_path.clone()
        {
            self.open_file(path, cx);
        }
    }

    pub fn tab_title(&self) -> SharedString {
        self.editor_path
            .as_deref()
            .and_then(|path| path.rsplit('/').next())
            .unwrap_or("Files")
            .into()
    }

    /// The file represented by this surface tab, when the browser has already
    /// promoted itself to an editor.
    pub fn attachment_path(&self) -> Option<&str> {
        self.editor_path.as_deref()
    }

    pub(crate) fn close_actions_menu(&mut self, cx: &mut Context<Self>) {
        if self.actions_menu.begin_close() {
            crate::popover::reap_popup(cx, |this: &mut Self| &mut this.actions_menu);
        }
    }

    pub(super) fn open_tree_file(&mut self, path: String, cx: &mut Context<Self>) {
        // The shell owns tab creation and selection for every file open. The
        // old in-place promotion of the browser surface left one file open
        // under two owners (the promoted tab plus the File tab a later click
        // created), so a click could activate a stale tab, re-read an already
        // open file, and reset the tree state on each new tab.
        cx.emit(FilesEvent::OpenFile(path));
    }

    /// The sidebar width this surface's tree currently sits at (editor
    /// layout). `None` for the raw workspace browser — its tree fills the
    /// pane, so the shell seeds a new tab from the pane width instead.
    pub fn current_sidebar_width(&self, cx: &App) -> Option<f32> {
        if self.presentation.is_editor() {
            Some(self.resting_tree_width(cx))
        } else {
            None
        }
    }

    pub fn chat_id(&self) -> &str {
        &self.chat_id
    }

    fn sync_target(&mut self, cx: &mut Context<Self>) -> bool {
        let next = FilesRequestContext::for_chat(self.state.read(cx), &self.chat_id);
        if self.request_context == next {
            self.target_change_pending = false;
            self.pending_request_context = None;
            return false;
        }
        if self.preview.has_unsaved_changes() {
            self.target_change_pending = true;
            self.pending_request_context = next;
            self.preview.cancel_autosaves();
            self.suspend_images(cx);
            cx.notify();
            return false;
        }
        self.apply_target(next, cx);
        true
    }

    pub(super) fn apply_pending_target(&mut self, cx: &mut Context<Self>) {
        if !self.target_change_pending {
            return;
        }
        let next = self.pending_request_context.take();
        self.target_change_pending = false;
        self.apply_target(next, cx);
        self.ensure_loaded(cx);
        cx.notify();
    }

    fn apply_target(&mut self, next: Option<FilesRequestContext>, cx: &mut Context<Self>) {
        // Document-side reset only: the shared tree entity syncs its own
        // target (reset + single reload) independently of the surfaces.
        self.suspend_images(cx);
        self.cancel_review_comment_flush(cx);
        self.editor_context_menu = crate::popover::Popup::default();
        self.preview.reset();
        self.request_context = next;
        cx.notify();
    }

    pub fn workspace_name(&self, cx: &Context<Self>) -> String {
        if let Some(ctx) = &self.request_context {
            let name = ctx
                .cwd
                .trim_end_matches(['/', '\\'])
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or("");
            if !name.is_empty() {
                return name.to_string();
            }
        }
        let state = self.state.read(cx);
        if let Some(chat) = state.chats.iter().find(|c| c.id == self.chat_id) {
            if let Some(cwd) = &chat.cwd {
                let name = cwd
                    .trim_end_matches(['/', '\\'])
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or("");
                if !name.is_empty() {
                    return name.to_string();
                }
            }
        }
        "workspace".to_string()
    }

    pub fn workspace_branch(&self, cx: &Context<Self>) -> Option<String> {
        let state = self.state.read(cx);
        let chat = state.chats.iter().find(|c| c.id == self.chat_id)?;
        if let Some(branch) = &chat.branch {
            let trimmed = branch.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
        if let Some(source) = &chat.source_context {
            let trimmed = source.branch.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
        None
    }

    pub(super) fn toggle_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The search lives on the shared tree view — one state per panel.
        self.tree_view
            .update(cx, |tree, cx| tree.toggle_search(window, cx));
    }

    pub(super) fn collapse_all(&mut self, cx: &mut Context<Self>) {
        self.tree_view.update(cx, |tree, cx| tree.collapse_all(cx));
    }

    /// The sidebar width this surface rests at, derived from the SHARED
    /// tree entity so every tab agrees (a drag on one tab persists).
    fn resting_tree_width(&self, cx: &App) -> f32 {
        let shared = self.tree_view.read_with(cx, |tree, _| tree.sidebar_width());
        if self.preview.is_wide() {
            shared
        } else {
            (self.preview.surface_width_read() * 0.44).clamp(152.0, shared)
        }
    }

    /// Reveal the tree (search-reveal path): un-collapse the SHARED state
    /// and snap this surface's overlay open.
    pub(super) fn show_tree_sidebar(&mut self, cx: &mut Context<Self>) {
        self.tree_view
            .update(cx, |tree, _| tree.set_sidebar_collapsed(false));
        self.preview.snap_sidebar_open();
    }

    /// The sidebar toggle: flip the SHARED collapsed state (remembered
    /// across tabs and the raw workspace) and animate this surface's
    /// overlay.
    pub(super) fn toggle_tree_sidebar(&mut self, cx: &mut Context<Self>) {
        let collapsed = self
            .tree_view
            .update(cx, |tree, _| tree.toggle_sidebar_collapsed());
        let resting = self.resting_tree_width(cx);
        self.preview.animate_sidebar_toggle(!collapsed, resting);
        self.drive_tree_animation(cx);
        cx.notify();
    }

    /// Re-activation clears any held cover width (the shell's raw swap
    /// already happened; this tab must show its normal sidebar layout).
    pub fn end_cover_transition(&mut self, cx: &mut Context<Self>) {
        self.preview.end_cover_hold();
        cx.notify();
    }

    /// The cover transition (raw return): the overlay tree grows from the
    /// sidebar width to the FULL pane width, covering the frozen preview.
    /// The shell swaps to the raw surface when the ease completes.
    pub fn begin_cover_expand(&mut self, from: f32, pane_width: f32, cx: &mut Context<Self>) {
        crate::ui_trace!("cover-expand from={:.0} to={:.0}", from, pane_width);
        self.preview.begin_cover_expand(from, pane_width);
        self.drive_tree_animation(cx);
        cx.notify();
    }

    /// Coming from the raw workspace: the overlay starts at full pane
    /// width and eases to the sidebar (the frozen preview is revealed).
    pub fn begin_sidebar_reveal(&mut self, pane_width: f32, cx: &mut Context<Self>) {
        // Respect the user's collapse: a collapsed tree swipes from full
        // width away to the corner (target 0), it does NOT expand to the
        // resting sidebar width.
        let collapsed = self
            .tree_view
            .read_with(cx, |tree, _| tree.sidebar_collapsed());
        let target = if collapsed {
            0.0
        } else {
            self.resting_tree_width(cx)
        };
        crate::ui_trace!("sidebar-reveal from={:.0} to={:.0}", pane_width, target);
        self.preview.seed_sidebar_transition(pane_width, target);
        self.drive_tree_animation(cx);
        cx.notify();
    }

    /// Keep frames coming while a tree animation is in flight: an 8 ms
    /// timer notifies this surface, which the (proven vsync-rate) dirty
    /// draw path picks up. Stops itself when every animation settles.
    fn drive_tree_animation(&mut self, cx: &mut Context<Self>) {
        if self.anim_driver.is_some() {
            return;
        }
        let window = self.window_handle;
        self.anim_driver = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(8))
                    .await;
                let still_animating = this
                    .update(cx, |surface, cx| {
                        if surface.preview.tree_animation_active() {
                            cx.notify();
                            true
                        } else {
                            false
                        }
                    })
                    .unwrap_or(false);
                if !still_animating {
                    break;
                }
                // The platform's request_animation_frame path schedules
                // nothing on Windows; a forced refresh keeps the next draw
                // (and its present) coming, which keeps the composition —
                // and the vsync redraw loop — running at frame rate.
                if let Some(handle) = window {
                    let _ = cx.update_window(handle, |_, window, _| window.refresh());
                }
            }
            // Release the slot so the next transition can spawn a driver.
            let _ = this.update(cx, |surface, _| surface.anim_driver = None);
        }));
    }

    /// The shared tree entity — the shell owns it; every surface embeds the
    /// same view.
    fn render_tree(&mut self, _cx: &mut Context<Self>) -> gpui::AnyElement {
        self.tree_view.clone().into_any_element()
    }

    fn render_header(
        &mut self,
        theme: &crate::theme::Theme,
        is_split: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let include_ignored = self
            .tree_view
            .read_with(cx, |tree, _| tree.include_ignored());
        let ws_name = self.workspace_name(cx);
        let branch = self.workspace_branch(cx);
        let is_search_open = self
            .tree_view
            .read_with(cx, |tree, _| tree.is_search_open());

        let trailing_actions = if is_split {
            let is_menu_open = self.actions_menu.get().is_some();
            let mut menu_trigger = toolbar_button("files-action-menu-trigger", "Workspace options")
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, _, _| {
                        this.actions_menu.note_trigger_press();
                    }),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    if this.actions_menu.take_press_was_open() {
                        this.close_actions_menu(cx);
                    } else {
                        this.actions_menu.open(());
                        cx.notify();
                    }
                }))
                .child(
                    crate::icons::icon(crate::icons::LIST)
                        .size(crate::typography::ui_rems(crate::surface_chrome::ICON_SIZE))
                        .text_color(if is_menu_open {
                            theme.text
                        } else {
                            theme.text_muted
                        }),
                );

            if is_menu_open {
                let closing = self.actions_menu.closing_since();
                let menu_card = crate::popover::popover_card(&theme.for_popup())
                    .w(crate::typography::ui_rems(210.0))
                    .p(crate::typography::ui_rems(4.0))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.close_actions_menu(cx);
                    }))
                    .flex()
                    .flex_col()
                    .gap(crate::typography::ui_rems(2.0))
                    .child(
                        crate::popover::menu_row(&theme.for_popup(), false, "files-menu-search")
                            .id("files-menu-search")
                            .child(
                                crate::icons::icon(crate::icons::MAGNIFER)
                                    .size(crate::typography::ui_rems(13.5))
                                    .flex_none()
                                    .text_color(theme.text_muted),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .font_family(theme.font_sans.clone())
                                    .text_size(crate::typography::ui_rems(12.5))
                                    .child(if is_search_open {
                                        "Close file search"
                                    } else {
                                        "Search files"
                                    }),
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_actions_menu(cx);
                                this.toggle_search(window, cx);
                            })),
                    )
                    .child(
                        crate::popover::menu_row(&theme.for_popup(), false, "files-menu-ignored")
                            .id("files-menu-ignored")
                            .child(
                                crate::icons::icon(if include_ignored {
                                    crate::icons::EYE
                                } else {
                                    crate::icons::EYE_CLOSED
                                })
                                .size(crate::typography::ui_rems(13.5))
                                .flex_none()
                                .text_color(theme.text_muted),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .font_family(theme.font_sans.clone())
                                    .text_size(crate::typography::ui_rems(12.5))
                                    .child(if include_ignored {
                                        "Hide hidden and ignored files"
                                    } else {
                                        "Show all files (even hidden)"
                                    }),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.close_actions_menu(cx);
                                this.tree_view
                                    .update(cx, |tree, cx| tree.toggle_ignored(cx));
                            })),
                    )
                    .child(
                        crate::popover::menu_row(&theme.for_popup(), false, "files-menu-collapse")
                            .id("files-menu-collapse")
                            .child(
                                crate::icons::icon(crate::icons::ALT_ARROW_UP)
                                    .size(crate::typography::ui_rems(13.5))
                                    .flex_none()
                                    .text_color(theme.text_muted),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .font_family(theme.font_sans.clone())
                                    .text_size(crate::typography::ui_rems(12.5))
                                    .child("Collapse all folders"),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.close_actions_menu(cx);
                                this.collapse_all(cx);
                            })),
                    )
                    .child(crate::popover::menu_separator())
                    .child(
                        crate::popover::menu_row(&theme.for_popup(), false, "files-menu-refresh")
                            .id("files-menu-refresh")
                            .child(
                                crate::icons::icon(crate::icons::REFRESH)
                                    .size(crate::typography::ui_rems(13.5))
                                    .flex_none()
                                    .text_color(theme.text_muted),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .font_family(theme.font_sans.clone())
                                    .text_size(crate::typography::ui_rems(12.5))
                                    .child("Refresh workspace"),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.close_actions_menu(cx);
                                this.tree_view.update(cx, |tree, cx| tree.refresh(cx));
                                this.reconcile_open_documents(cx);
                            })),
                    )
                    .into_any_element();

                menu_trigger =
                    menu_trigger
                        .relative()
                        .child(crate::popover::anchored_menu_below_end(
                            "files-actions-menu-popover",
                            menu_card,
                            closing,
                        ));
            }

            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(crate::typography::ui_rems(2.0))
                .child(
                    toolbar_button(
                        "files-split-toggle-search",
                        if is_search_open {
                            "Close file search"
                        } else {
                            "Search files"
                        },
                    )
                    .when(is_search_open, |el| el.bg(crate::theme::wash(0.12)))
                    .relative()
                    .child(crate::ui_trace::bounds_probe("search-toggle"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_search(window, cx)
                    }))
                    .child(
                        crate::icons::icon(crate::icons::MAGNIFER)
                            .size(crate::typography::ui_rems(
                                crate::surface_chrome::ICON_SIZE,
                            ))
                            .text_color(if is_search_open {
                                theme.text
                            } else {
                                theme.text_muted
                            }),
                    ),
                )
                .child(menu_trigger)
                .into_any_element()
        } else {
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(crate::typography::ui_rems(2.0))
                .child(
                    toolbar_button(
                        "files-toggle-search",
                        if is_search_open {
                            "Close file search"
                        } else {
                            "Search files"
                        },
                    )
                    .when(is_search_open, |el| el.bg(crate::theme::wash(0.12)))
                    .relative()
                    .child(crate::ui_trace::bounds_probe("search-toggle"))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_search(window, cx)))
                    .child(
                        crate::icons::icon(crate::icons::MAGNIFER)
                            .size(crate::typography::ui_rems(crate::surface_chrome::ICON_SIZE))
                            .text_color(if is_search_open {
                                theme.text
                            } else {
                                theme.text_muted
                            }),
                    ),
                )
                .child(
                    toolbar_button(
                        "files-toggle-ignored",
                        if include_ignored {
                            "Hide hidden and ignored files"
                        } else {
                            "Show all files (even hidden)"
                        },
                    )
                    .when(include_ignored, |element| {
                        element.bg(crate::theme::wash(0.1))
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.tree_view
                            .update(cx, |tree, cx| tree.toggle_ignored(cx));
                    }))
                    .child(
                        crate::icons::icon(if include_ignored {
                            crate::icons::EYE
                        } else {
                            crate::icons::EYE_CLOSED
                        })
                        .size(crate::typography::ui_rems(crate::surface_chrome::ICON_SIZE))
                        .text_color(if include_ignored {
                            theme.text
                        } else {
                            theme.text_muted
                        }),
                    ),
                )
                .child(
                    toolbar_button("files-collapse-all", "Collapse all folders")
                        .on_click(cx.listener(|this, _, _, cx| this.collapse_all(cx)))
                        .child(
                            crate::icons::icon(crate::icons::ALT_ARROW_UP)
                                .size(crate::typography::ui_rems(crate::surface_chrome::ICON_SIZE))
                                .text_color(theme.text_muted),
                        ),
                )
                .child(
                    toolbar_button("files-refresh-button", "Refresh workspace")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.tree_view.update(cx, |tree, cx| tree.refresh(cx));
                            this.reconcile_open_documents(cx);
                        }))
                        .child(
                            crate::icons::icon(crate::icons::REFRESH)
                                .size(crate::typography::ui_rems(crate::surface_chrome::ICON_SIZE))
                                .text_color(theme.text_muted),
                        ),
                )
                .into_any_element()
        };

        div()
            .w_full()
            .flex()
            .flex_col()
            .child(
                toolbar(theme)
                    .pl(crate::typography::ui_rems(16.0))
                    .pr(crate::typography::ui_rems(8.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(crate::typography::ui_rems(6.0))
                            .overflow_hidden()
                            .child(
                                crate::file_icons::icon(
                                    crate::file_icons::FileIconIdentity::directory(&ws_name, true),
                                    theme.appearance,
                                )
                                .size(crate::typography::ui_rems(15.0))
                                .flex_none(),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .font_family(theme.font_sans.clone())
                                    .text_size(crate::typography::ui_rems(12.0))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(theme.text)
                                    .child(ws_name),
                            )
                            .when_some(branch, |parent, b| {
                                parent
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_size(crate::typography::ui_rems(11.0))
                                            .text_color(theme.text_faint)
                                            .child("/"),
                                    )
                                    .child(
                                        crate::icons::icon(crate::icons::GIT_BRANCH)
                                            .size(crate::typography::ui_rems(11.5))
                                            .flex_none()
                                            .text_color(theme.text_muted),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .truncate()
                                            .font_family(theme.font_sans.clone())
                                            .text_size(crate::typography::ui_rems(11.5))
                                            .text_color(theme.text_muted)
                                            .child(b),
                                    )
                            }),
                    )
                    .child(trailing_actions),
            )
    }
}
