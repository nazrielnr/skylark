use std::{
    cell::Cell,
    collections::{HashMap, HashSet, VecDeque},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use gpui::{
    AnyElement, App, Context, Entity, Focusable as _, HighlightStyle, ListAlignment,
    ListSizingBehavior, ListState, Point, Render, ScrollHandle, SharedString, Subscription, Window,
    div, font, list, prelude::*, px,
};
use gpui_base::input::{RopeExt as _, TextDecoration, TextDecorationCollection};
use zeron_proto::{
    ReadWorkspaceFileRequest, WorkspaceFileSearchMatch, WorkspaceReadOnlyReason,
    WriteWorkspaceFileOutcome, WriteWorkspaceFileRequest,
};

use super::{
    FilesCloseDisposition, FilesEvent, FilesSurface,
    client::{FilesRequestContext, WorkspaceFilesClient},
    document::{DocumentKey, DocumentPhase, FileDocument},
    toolbar, toolbar_button,
};
use crate::{
    comments::{self, ReviewComment},
    composer::{ComposerInput, ComposerInputEvent},
    icons::{self, icon},
    syntax_cache::{DocumentHighlightKey, SyntaxHighlightCache},
    theme::Theme,
};

const PREVIEW_LINE_HEIGHT: f32 = 20.0;
/// One shared `code_font_size` setting drives two surfaces in this file that
/// never agreed on a size: the editable editor and the plain preview. Each
/// scales off its own baseline so the default setting reproduces the size that
/// surface always had, and a user-chosen size moves both while keeping their
/// proportions.
const EDITOR_TEXT_SIZE: f32 = 13.0;
const PREVIEW_TEXT_SIZE: f32 = 11.5;
const EDITOR_TEXT_SIZE_RATIO: f32 = EDITOR_TEXT_SIZE / crate::typography::CODE_FONT_SIZE_DEFAULT;
const PREVIEW_TEXT_SIZE_RATIO: f32 = PREVIEW_TEXT_SIZE / crate::typography::CODE_FONT_SIZE_DEFAULT;
const PREVIEW_LINE_HEIGHT_RATIO: f32 = PREVIEW_LINE_HEIGHT / PREVIEW_TEXT_SIZE;
const WIDE_BREAKPOINT: f32 = 680.0;
const TREE_SPLIT_DEFAULT: f32 = 286.0;
const TREE_SPLIT_MIN: f32 = 220.0;
const TREE_SPLIT_MAX: f32 = 360.0;
pub(super) const TREE_SPLIT_HITBOX_HALF_WIDTH: f32 = 10.0;
const EDITOR_COMMENT_CARD_WIDTH: f32 = 320.0;
const EDITOR_COMMENT_CARD_MARGIN: f32 = 8.0;
const EDITOR_COMMENT_CARD_MIN_ANCHORED_WIDTH: f32 = 220.0;
const EDITOR_COMMENT_DRAFT_HEIGHT: f32 = 92.0;
// A read response is capped at 8 MiB and editable files at 1 MiB. The byte
// budget bounds large previews while the entry cap bounds many tiny editors.
// Protected documents may exceed either limit rather than risk losing work.
const MAX_RETAINED_DOCUMENTS: usize = 16;
const MAX_RETAINED_DOCUMENT_BYTES: usize = 32 * 1024 * 1024;

struct HighlightedFile {
    content_hash: String,
    document: Arc<zeron_syntax::HighlightedDocument>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommentAnchorEdge {
    Start,
    End,
}

struct EditorCommentAnchor {
    range: TextDecorationCollection,
    edge: CommentAnchorEdge,
}

struct EditorCommentDraft {
    editing_id: Option<String>,
    key: String,
    path: String,
    line: u32,
    input: Entity<ComposerInput>,
    _events: Subscription,
}

struct EditorOverlayRow {
    line: u32,
    top: f32,
}

struct EditorOverlayLayout {
    gutter_width: f32,
    line_height: f32,
    viewport_width: f32,
    viewport_height: f32,
    rows: Vec<EditorOverlayRow>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReloadDecision {
    ReloadNow,
    AwaitDiscardConfirmation,
}

/// Openness is independent of the dragged width, so resizing remains direct.
#[derive(Default)]
struct TreeSidebarMotion {
    target: Option<bool>,
    from: f32,
    started: Option<Instant>,
}

impl TreeSidebarMotion {
    /// Snap to fully open with no transition (file activation path).
    fn snap_open(&mut self) {
        self.target = Some(true);
        self.started = None;
    }

    fn sample(&mut self, visible: bool, now: Instant, reduced: bool) -> (f32, bool) {
        let end = f32::from(visible);
        let duration = crate::motion::RESIZE
            .total()
            .mul_f32(crate::motion::speed_scale());
        // Layout and file activation changes are immediate. Only the toggle
        // action starts a transition through animate_to.
        if reduced || self.target != Some(visible) {
            self.target = Some(visible);
            self.started = None;
            return (end, false);
        }
        if let Some(started) = self.started {
            let raw = now.saturating_duration_since(started).as_secs_f32() / duration.as_secs_f32();
            if raw < 1.0 {
                return (
                    crate::motion::lerp(self.from, end, crate::motion::RESIZE.progress(raw)),
                    true,
                );
            }
            self.started = None;
        }
        (end, false)
    }

    fn animate_to(&mut self, previous: bool, visible: bool, now: Instant) {
        self.from = self.sample(previous, now, false).0;
        self.target = Some(visible);
        self.started = Some(now);
    }
}

pub(super) struct FilePreviewState {
    images_visible: bool,
    documents: HashMap<String, FileDocument>,
    document_recency: VecDeque<String>,
    pub(super) active: Option<String>,
    highlights: HashMap<String, HighlightedFile>,
    syntax_cache: SyntaxHighlightCache,
    list: ListState,
    horizontal_scroll: ScrollHandle,
    surface_width: Rc<Cell<f32>>,
    word_wrap: bool,
    editor_font_size: f32,
    autosave_enabled: bool,
    autosave_delay_ms: u64,
    reload_confirmation: Option<String>,
    close_requested: bool,
    tree_sidebar_visible: bool,
    tree_sidebar_dismissed: bool,
    tree_width: f32,
    /// One-shot width transition: the tab was opened while the tree filled
    /// (or sat at) `from` width — the sidebar eases from there to
    /// `tree_width` so the browser→editor switch reads as the tree sliding
    /// aside instead of the layout snapping.
    tree_width_tween: Option<(f32, Instant)>,
    tree_motion: TreeSidebarMotion,
    tree_edge_bounce: Option<crate::motion::ResizeEdgeBounce>,
    tree_resize_edge: Option<crate::motion::ResizeEdge>,
    tree_resize_active: bool,
    tree_resize_dragging: bool,
    comment_anchors: HashMap<String, HashMap<String, EditorCommentAnchor>>,
    comment_draft: Option<EditorCommentDraft>,
    active_comment: Option<String>,
    typography_generation: u32,
}

#[path = "preview/state.rs"]
mod preview_state;
use preview_state::*;
#[path = "preview/helpers.rs"]
mod helpers;
use helpers::*;

pub(super) struct PreviewSplitResize;

struct PreviewDragGhost;

impl Render for PreviewDragGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size(px(1.0))
    }
}

pub(super) struct FileEditorTooltip {
    pub(super) text: SharedString,
}

impl Render for FileEditorTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let card = div()
            .max_w(px(360.0))
            .px(px(9.0))
            .py(px(6.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(theme.border)
            .bg(crate::popover::surface_bg(theme))
            .font_family(theme.font_sans.clone())
            .text_size(px(10.5))
            .text_color(theme.text_muted)
            .child(self.text.clone());
        crate::frost::frosted(6.0, crate::frost::MENU_BLUR, card)
    }
}

impl FilesSurface {
    #[cfg(test)]
    pub(crate) fn test_images_visible(&self) -> bool {
        self.preview.images_visible
    }

    pub(crate) fn suspend_images(&mut self, cx: &mut Context<Self>) {
        if !self.preview.images_visible {
            return;
        }
        self.preview.images_visible = false;
        for document in self.preview.documents.values() {
            if let Some(view) = &document.image {
                view.update(cx, |view, cx| view.suspend(cx));
            }
        }
    }

    pub(crate) fn focus_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = self
            .preview
            .active
            .as_ref()
            .and_then(|path| self.preview.documents.get(path))
            .and_then(|d| d.image.clone())
        {
            let focus = view.read(cx).focus.clone();
            window.defer(cx, move |window, cx| focus.focus(window, cx));
            return;
        }
        if let Some(view) = self
            .preview
            .active
            .as_ref()
            .and_then(|path| self.preview.documents.get(path))
            .filter(|d| d.show_markdown)
            .and_then(|d| d.markdown.clone())
        {
            let focus = view.read(cx).focus.clone();
            window.defer(cx, move |window, cx| focus.focus(window, cx));
            return;
        }
        let Some(editor) = self
            .preview
            .active
            .as_deref()
            .and_then(|path| self.preview.documents.get(path))
            .and_then(|document| document.editor.clone())
        else {
            return;
        };
        let focus = editor.focus_handle(cx);
        // Tab activation remounts this surface after the click handler returns.
        window.defer(cx, move |window, cx| focus.focus(window, cx));
    }

    pub(super) fn show_tree_sidebar(&mut self, cx: &mut Context<Self>) {
        self.preview.show_tree_sidebar();
        cx.notify();
    }

    fn toggle_tree_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.preview.toggle_tree_sidebar();
        if !self.preview.tree_sidebar_visible() {
            // A hidden search input must not keep receiving editor keystrokes.
            self.focus_editor(window, cx);
        }
        cx.notify();
    }

    fn toggle_word_wrap(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let word_wrap = !self.preview.word_wrap;
        cx.emit(FilesEvent::WordWrapChanged(word_wrap));
    }

    pub(super) fn apply_word_wrap(
        &mut self,
        word_wrap: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.preview.word_wrap == word_wrap {
            return;
        }
        self.preview.word_wrap = word_wrap;
        self.preview.list.remeasure();
        let editors = self
            .preview
            .documents
            .values()
            .filter_map(|document| document.editor.clone())
            .collect::<Vec<_>>();
        for editor in editors {
            editor.update(cx, |state, cx| state.set_soft_wrap(word_wrap, window, cx));
        }
        cx.notify();
    }

    fn staged_file_comments(&self, path: &str, cx: &App) -> Vec<ReviewComment> {
        self.state
            .read(cx)
            .review_comments(&self.chat_id)
            .iter()
            .filter(|comment| comment.is_file() && comment.path == path)
            .cloned()
            .collect()
    }

    fn trim_document_cache(&mut self, cx: &App) {
        let mut protected_paths = self
            .state
            .read(cx)
            .review_comments(&self.chat_id)
            .iter()
            .filter(|comment| comment.is_file())
            .map(|comment| comment.path.clone())
            .collect::<HashSet<_>>();
        if let Some(path) = self.editor_path.as_ref() {
            protected_paths.insert(path.clone());
        }
        for path in self.preview.trim_document_cache(&protected_paths) {
            tracing::debug!(path = %path, "evicted inactive workspace document");
        }
    }

    fn require_review_comment_flush(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(document) = self.preview.documents.get_mut(path) else {
            return;
        };
        if !document.is_dirty() {
            return;
        }
        document.review_comment_flush_pending = true;
        let key = self.chat_id.clone();
        let source = self.review_comment_flush_source;
        self.state.update(cx, |state, cx| {
            state.begin_review_comment_flush(&key, source);
            cx.notify();
        });
    }

    fn finish_review_comment_flush_if_idle(&mut self, cx: &mut Context<Self>) {
        if self
            .preview
            .documents
            .values()
            .any(|document| document.review_comment_flush_pending)
        {
            return;
        }
        let key = self.chat_id.clone();
        let source = self.review_comment_flush_source;
        self.state.update(cx, |state, cx| {
            state.finish_review_comment_flush(&key, source);
            cx.notify();
        });
    }

    pub(super) fn cancel_review_comment_flush(&mut self, cx: &mut Context<Self>) {
        for document in self.preview.documents.values_mut() {
            document.review_comment_flush_pending = false;
        }
        self.finish_review_comment_flush_if_idle(cx);
    }
}

#[path = "preview/comment_render.rs"]
mod comment_render;
mod review_comments;

impl FilesSurface {}

mod document_io;

impl FilesSurface {}

mod lifecycle;

impl FilesSurface {}

mod render;

#[cfg(test)]
mod tests;
#[cfg(test)]
impl FilesSurface {
    pub(crate) fn seed_pending_exit_test_document(&mut self, failed: bool) {
        let mut document = FileDocument::loading(DocumentKey {
            chat_id: "test".into(),
            checkout_id: None,
            path: "test.rs".into(),
        });
        document.revision = 1;
        document.phase = if failed {
            DocumentPhase::SaveFailed("offline".into())
        } else {
            DocumentPhase::Saving
        };
        self.preview.documents.insert("test.rs".into(), document);
    }
}

#[cfg(test)]
mod markdown_buffer_tests {
    use super::*;
    use gpui::{AppContext, TestAppContext};

    #[cfg(target_os = "linux")]
    #[test]
    fn markdown_inline_comments_use_file_review_workflow() {
        struct Harness {
            owner: Entity<FilesSurface>,
            view: Entity<super::super::markdown_preview::MarkdownPreview>,
            _observer: Subscription,
        }
        impl gpui::Render for Harness {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                self.owner.update(cx, |owner, cx| {
                    let editor = owner.preview.documents["README.md"].editor.clone();
                    owner.prepare_markdown_preview("README.md", editor.as_ref(), cx);
                });
                div().size_full().child(self.view.clone())
            }
        }
        fn click(window: &mut Window, position: gpui::Point<gpui::Pixels>, cx: &mut App) {
            window.dispatch_event(
                gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                    position,
                    ..Default::default()
                }),
                cx,
            );
            window.refresh();
            let _ = window.draw(cx);
            window.dispatch_event(
                gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                    position,
                    button: gpui::MouseButton::Left,
                    click_count: 1,
                    ..Default::default()
                }),
                cx,
            );
            window.dispatch_event(
                gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                    position,
                    button: gpui::MouseButton::Left,
                    click_count: 1,
                    ..Default::default()
                }),
                cx,
            );
        }
        gpui_platform::headless().run(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::dark());
            let source =
                "# Título 🦀\n\nUn párrafo para revisar.\n\n- Primera tarea\n- Segunda tarea\n";
            let window = cx
                .open_window(
                    gpui::WindowOptions {
                        window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
                            gpui::Point::default(),
                            gpui::size(px(1100.0), px(600.0)),
                        ))),
                        ..Default::default()
                    },
                    |window, cx| {
                        let state = cx.new(|_| crate::state::AppState::new());
                        let owner = cx.new(|cx| {
                            let mut owner = FilesSurface::new(
                                state,
                                "chat".into(),
                                false,
                                1000,
                                13.0,
                                false,
                                false,
                                cx,
                            );
                            let mut document = FileDocument::loading(DocumentKey {
                                chat_id: "chat".into(),
                                checkout_id: Some("checkout".into()),
                                path: "README.md".into(),
                            });
                            document.set_loaded(zeron_proto::WorkspaceFileText {
                                checkout_id: "checkout".into(),
                                path: "README.md".into(),
                                text: Some(source.into()),
                                content_hash: Some("hash".into()),
                                size: source.len() as u64,
                                modified_at: None,
                                encoding: zeron_proto::WorkspaceTextEncoding::Utf8,
                                line_ending: Some(zeron_proto::WorkspaceLineEnding::Lf),
                                read_only_reason: None,
                                truncated: false,
                            });
                            let theme = Theme::of(cx).clone();
                            document.editor = Some(super::super::editor::new_file_editor(
                                source,
                                "README.md",
                                false,
                                &theme,
                                window,
                                cx,
                            ));
                            document.show_markdown = true;
                            owner.preview.active = Some("README.md".into());
                            owner.preview.documents.insert("README.md".into(), document);
                            owner
                        });
                        let view = owner.update(cx, |owner, cx| {
                            let editor = owner.preview.documents["README.md"].editor.clone();
                            owner
                                .prepare_markdown_preview("README.md", editor.as_ref(), cx)
                                .unwrap()
                        });
                        cx.new(|cx| Harness {
                            _observer: cx.observe(&owner, |_, _, cx| cx.notify()),
                            owner,
                            view,
                        })
                    },
                )
                .unwrap();
            let root = window.entity(cx).unwrap();
            let owner = root.read(cx).owner.clone();
            let view = root.read(cx).view.clone();
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(300))
                    .await;
                cx.update(|cx| {
                    cx.update_window(window.into(), |_, window, cx| {
                        window.refresh();
                        let _ = window.draw(cx);
                        let bounds = view.read(cx).test_block_bounds(1);
                        let gutter = ((bounds.size.width - px(900.0)) / 2.0).max(px(24.0));
                        click(
                            window,
                            gpui::point(bounds.left() + gutter - px(12.0), bounds.top() + px(11.0)),
                            cx,
                        );
                    })
                    .unwrap();
                    let input = owner
                        .read(cx)
                        .preview
                        .comment_draft
                        .as_ref()
                        .expect("plus opens draft")
                        .input
                        .clone();
                    assert_eq!(
                        owner.read(cx).preview.comment_draft.as_ref().unwrap().line,
                        3
                    );
                    cx.update_window(window.into(), |_, window, cx| {
                        window.refresh();
                        let _ = window.draw(cx);
                        assert!(input.read(cx).focus_handle(cx).is_focused(window));
                        input.update(cx, |input, cx| input.set_text("Clarify this paragraph", cx));
                        window.refresh();
                        let _ = window.draw(cx);
                        let bounds = view.read(cx).test_block_bounds(1);
                        assert!(bounds.size.height > px(comments::DRAFT_CARD_HEIGHT));
                        let gutter = ((bounds.size.width - px(900.0)) / 2.0).max(px(24.0));
                        click(
                            window,
                            gpui::point(
                                bounds.right() - gutter - px(45.0),
                                bounds.bottom() - px(36.0),
                            ),
                            cx,
                        );
                    })
                    .unwrap();
                    assert!(
                        owner.read(cx).preview.comment_draft.is_none(),
                        "Comment commits the draft"
                    );
                    let staged = owner.read(cx).staged_file_comments("README.md", cx);
                    assert_eq!(staged.len(), 1);
                    assert_eq!(staged[0].line, 3);
                    assert_eq!(staged[0].body, "Clarify this paragraph");
                    assert!(staged[0].is_file());
                    for save in [false, true] {
                        cx.update_window(window.into(), |_, window, cx| {
                            window.refresh();
                            let _ = window.draw(cx);
                            let bounds = view.read(cx).test_block_bounds(1);
                            let gutter = ((bounds.size.width - px(900.0)) / 2.0).max(px(24.0));
                            click(
                                window,
                                gpui::point(
                                    bounds.right() - gutter - px(30.0),
                                    bounds.bottom()
                                        - px(12.0)
                                        - px(comments::card_height(&staged[0].body))
                                        + px(21.0),
                                ),
                                cx,
                            );
                            let input = owner
                                .read(cx)
                                .preview
                                .comment_draft
                                .as_ref()
                                .expect("Edit reopens the comment input")
                                .input
                                .clone();
                            assert_eq!(input.read(cx).text(), staged[0].body);
                            assert!(input.read(cx).focus_handle(cx).is_focused(window));
                            input.update(cx, |input, cx| input.set_text("Revised paragraph", cx));
                            window.refresh();
                            let _ = window.draw(cx);
                            if save {
                                let bounds = view.read(cx).test_block_bounds(1);
                                click(
                                    window,
                                    gpui::point(
                                        bounds.right() - gutter - px(22.0),
                                        bounds.bottom() - px(36.0),
                                    ),
                                    cx,
                                );
                            } else {
                                window.dispatch_event(
                                    gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
                                        keystroke: gpui::Keystroke::parse("escape").unwrap(),
                                        is_held: false,
                                        prefer_character_input: false,
                                    }),
                                    cx,
                                );
                            }
                        })
                        .unwrap();
                        assert!(owner.read(cx).preview.comment_draft.is_none());
                        let mut expected = staged[0].clone();
                        if save {
                            expected.body = "Revised paragraph".into();
                        }
                        assert_eq!(
                            owner.read(cx).staged_file_comments("README.md", cx),
                            vec![expected]
                        );
                    }
                    let editor = owner.read(cx).preview.documents["README.md"]
                        .editor
                        .clone()
                        .unwrap();
                    assert_eq!(editor.read(cx).value().as_ref(), source);
                    assert!(!owner.read(cx).preview.documents["README.md"].is_dirty());
                    cx.update_window(window.into(), |_, window, cx| {
                        window.refresh();
                        let _ = window.draw(cx);
                        let bounds = view.read(cx).test_block_bounds(1);
                        let gutter = ((bounds.size.width - px(900.0)) / 2.0).max(px(24.0));
                        // The 16px remove button ends at the reading column's right edge.
                        // Click its center; Markdown cards have no inner horizontal padding.
                        click(
                            window,
                            gpui::point(
                                bounds.right() - gutter - px(8.0),
                                bounds.bottom()
                                    - px(12.0)
                                    - px(comments::card_height("Clarify this paragraph"))
                                    + px(21.0),
                            ),
                            cx,
                        );
                    })
                    .unwrap();
                    assert!(
                        owner
                            .read(cx)
                            .staged_file_comments("README.md", cx)
                            .is_empty()
                    );
                    cx.update_window(window.into(), |_, window, cx| {
                        owner.update(cx, |owner, cx| {
                            owner.open_markdown_comment(
                                "README.md".into(),
                                3,
                                "outdated buffer",
                                window,
                                cx,
                            );
                            assert!(owner.preview.comment_draft.is_none());
                            owner.open_markdown_comment("README.md".into(), 3, source, window, cx);
                        });
                        window.refresh();
                        let _ = window.draw(cx);
                        window.dispatch_event(
                            gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
                                keystroke: gpui::Keystroke::parse("escape").unwrap(),
                                is_held: false,
                                prefer_character_input: false,
                            }),
                            cx,
                        );
                    })
                    .unwrap();
                    assert!(
                        owner.read(cx).preview.comment_draft.is_none(),
                        "Escape cancels without staging a comment"
                    );
                    cx.quit();
                });
            })
            .detach();
        });
    }

    #[gpui::test]
    fn editing_file_comments_keeps_the_live_anchor(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::dark());
        });
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| crate::state::AppState::new());
            FilesSurface::new(state, "chat".into(), false, 1000, 13.0, false, false, cx)
        });
        window
            .update(cx, |surface, window, cx| {
                let theme = Theme::of(cx).clone();
                let editor = super::super::editor::new_file_editor(
                    "first\nsecond\nthird\n",
                    "a.rs",
                    false,
                    &theme,
                    window,
                    cx,
                );
                let mut document = FileDocument::loading(DocumentKey {
                    chat_id: "chat".into(),
                    checkout_id: None,
                    path: "a.rs".into(),
                });
                document.editor = Some(editor.clone());
                surface.preview.documents.insert("a.rs".into(), document);
                let original = ReviewComment::file("a.rs", 2, "Original 🦀\nSecond line");
                surface.state.update(cx, |state, _| {
                    state.add_review_comment("chat", original.clone())
                });
                surface.sync_editor_comment_anchors("a.rs", &editor, cx);
                let anchor = surface.preview.comment_anchors["a.rs"][&original.id]
                    .range
                    .clone();

                surface.edit_editor_comment(&original.id, window, cx);
                let input = surface
                    .preview
                    .comment_draft
                    .as_ref()
                    .unwrap()
                    .input
                    .clone();
                assert_eq!(input.read(cx).text(), original.body);
                assert!(input.read(cx).focus_handle(cx).is_focused(window));
                input.update(cx, |input, cx| input.set_text("Cancelled", cx));
                assert_eq!(
                    surface.staged_file_comments("a.rs", cx),
                    vec![original.clone()]
                );
                surface.cancel_editor_comment(cx);
                assert_eq!(
                    surface.staged_file_comments("a.rs", cx),
                    vec![original.clone()]
                );
                assert_eq!(
                    surface.preview.active_comment.as_deref(),
                    Some(original.id.as_str())
                );

                surface.edit_editor_comment(&original.id, window, cx);
                // Move the existing editor decoration while the body is being edited.
                let (range, _) = comment_anchor_range(editor.read(cx).text(), 3).unwrap();
                anchor.set(
                    vec![TextDecoration::new(range, HighlightStyle::default())],
                    cx,
                );
                surface.sync_editor_comment_lines("a.rs", &editor, cx);
                assert_eq!(surface.preview.comment_draft.as_ref().unwrap().line, 3);
                surface
                    .preview
                    .comment_draft
                    .as_ref()
                    .unwrap()
                    .input
                    .clone()
                    .update(cx, |input, cx| {
                        input.set_text("  Revised\nMore detail  ", cx)
                    });
                surface.commit_editor_comment(cx);
                let mut expected = original.clone();
                expected.line = 3;
                expected.body = "Revised\nMore detail".into();
                assert_eq!(surface.staged_file_comments("a.rs", cx), vec![expected]);
                // The original decoration handle still controls the stored anchor.
                let (range, _) = comment_anchor_range(editor.read(cx).text(), 1).unwrap();
                anchor.set(
                    vec![TextDecoration::new(range, HighlightStyle::default())],
                    cx,
                );
                surface.sync_editor_comment_lines("a.rs", &editor, cx);
                assert_eq!(surface.staged_file_comments("a.rs", cx)[0].line, 1);

                surface.edit_editor_comment(&original.id, window, cx);
                let sent = surface
                    .state
                    .update(cx, |state, _| state.take_review_comments("chat"));
                assert!(comments::with_comments("", &sent).contains("Revised\n  More detail"));
                surface.commit_editor_comment(cx);
                assert!(surface.staged_file_comments("a.rs", cx).is_empty());
            })
            .unwrap();
    }

    #[gpui::test]
    fn image_rename_preserves_unsaved_text_on_reopen_and_watcher(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::dark());
        });
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| crate::state::AppState::new());
            FilesSurface::new(state, "chat".into(), false, 1000, 13.0, false, false, cx)
        });
        window
            .update(cx, |surface, window, cx| {
                surface.request_context = Some(FilesRequestContext {
                    target: zeron_proto::WorkspaceTarget {
                        chat_id: Some("chat".into()),
                        space_id: None,
                        checkout_path: None,
                    },
                    target_device_id: None,
                    cwd: "/workspace".into(),
                    checkout_id: Some("checkout".into()),
                });
                let mut document = FileDocument::loading(DocumentKey {
                    chat_id: "chat".into(),
                    checkout_id: Some("checkout".into()),
                    path: "drawing.txt".into(),
                });
                document.set_loaded(zeron_proto::WorkspaceFileText {
                    checkout_id: "checkout".into(),
                    path: "drawing.txt".into(),
                    text: Some("disk text".into()),
                    content_hash: Some("disk-hash".into()),
                    size: 9,
                    modified_at: None,
                    encoding: zeron_proto::WorkspaceTextEncoding::Utf8,
                    line_ending: Some(zeron_proto::WorkspaceLineEnding::Lf),
                    read_only_reason: None,
                    truncated: false,
                });
                let theme = Theme::of(cx).clone();
                let editor = super::super::editor::new_file_editor(
                    "unsaved text",
                    "drawing.txt",
                    false,
                    &theme,
                    window,
                    cx,
                );
                document.editor = Some(editor.clone());
                document.mark_user_edit();
                surface.preview.active = Some("drawing.txt".into());
                surface
                    .preview
                    .documents
                    .insert("drawing.txt".into(), document);
                surface.rename_documents("drawing.txt", "drawing.svg".into(), cx);
                surface.open_file("drawing.svg".into(), cx);
                surface.reconcile_document("drawing.svg".into(), cx);
                let document = surface.preview.documents.get_mut("drawing.svg").unwrap();
                assert!(document.is_dirty());
                assert!(document.is_editable());
                assert_eq!(document.editor.as_ref(), Some(&editor));
                assert!(document.image.is_none());
                assert!(matches!(
                    document.phase,
                    DocumentPhase::ExternallyModified { .. }
                ));
                // After resolving the external-change warning, saving still uses
                // the original buffer and cannot be cancelled by preview creation.
                document.phase = DocumentPhase::Ready;
                assert!(document.can_save());
                let pending = document.begin_save("unsaved text".into()).unwrap();
                surface.read_image_file("drawing.svg".into(), cx);
                let document = &surface.preview.documents["drawing.svg"];
                assert_eq!(document.pending_save.as_ref(), Some(&pending));
                assert_eq!(document.editor.as_ref(), Some(&editor));
            })
            .unwrap();
    }

    #[gpui::test]
    fn markdown_preview_web_links_keep_the_file_session_and_full_target(cx: &mut TestAppContext) {
        use crate::markdown::render::{self, LinkAction, LinkTarget};
        use std::cell::RefCell;

        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::dark());
        });
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| crate::state::AppState::new());
            FilesSurface::new(state, "owner".into(), false, 1000, 13.0, false, false, cx)
        });
        let (owner, preview) = window
            .update(cx, |surface, _, cx| {
                let mut document = FileDocument::loading(DocumentKey {
                    chat_id: "owner".into(),
                    checkout_id: Some("checkout".into()),
                    path: "README.md".into(),
                });
                document.set_loaded(zeron_proto::WorkspaceFileText {
                    checkout_id: "checkout".into(),
                    path: "README.md".into(),
                    text: Some("[Docs](https://example.com/docs)".into()),
                    content_hash: Some("hash".into()),
                    size: 32,
                    modified_at: None,
                    encoding: zeron_proto::WorkspaceTextEncoding::Utf8,
                    line_ending: Some(zeron_proto::WorkspaceLineEnding::Lf),
                    read_only_reason: None,
                    truncated: false,
                });
                document.show_markdown = true;
                surface.preview.active = Some("README.md".into());
                surface
                    .preview
                    .documents
                    .insert("README.md".into(), document);
                let preview = surface
                    .prepare_markdown_preview("README.md", None, cx)
                    .unwrap();
                // Changing the selected chat must not rewrite this file's owner.
                surface
                    .state
                    .update(cx, |state, _| state.selected_chat = Some("other".into()));
                (cx.entity(), preview)
            })
            .unwrap();
        let received = Rc::new(RefCell::new(Vec::new()));
        let _subscription = cx.update(|cx| {
            cx.subscribe(&owner, {
                let received = received.clone();
                let preview = preview.clone();
                move |_, event, cx| {
                    if let FilesEvent::OpenWebLink(activation) = event {
                        received.borrow_mut().push(activation.clone());
                        // Browser selection can suspend this same preview. This
                        // must run after its owner/preview borrows have unwound.
                        preview.update(cx, |preview, cx| preview.suspend(cx));
                    }
                }
            })
        });
        let ui = preview.update(cx, |preview, cx| preview.link_ui(cx));
        assert!(
            ui.source_session.is_none(),
            "file preview labels are not truncated"
        );
        let target = LinkTarget::new("Different label", "https://example.com/docs?q=%C3%B1#full");
        for action in [
            LinkAction::Primary,
            LinkAction::Internal,
            LinkAction::External,
            LinkAction::Copy,
        ] {
            cx.update_window(window.into(), |_, window, cx| {
                render::activate_link(target.clone(), action, Some(&ui), window, cx);
            })
            .unwrap();
        }
        cx.run_until_parked();
        let events = received.borrow();
        assert_eq!(events.len(), 3);
        for (event, action) in events.iter().zip([
            LinkAction::Primary,
            LinkAction::Internal,
            LinkAction::External,
        ]) {
            assert_eq!(event.target, target);
            assert_eq!(event.action, action);
            assert_eq!(event.source_session.as_deref(), Some("owner"));
        }
        drop(events);
        cx.update(|cx| {
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().as_deref(),
                Some(target.original.as_str())
            )
        });
        assert!(
            cx.opened_url().is_none(),
            "only the shell may open web destinations"
        );
        for url in [
            "javascript:alert(1)",
            "https://user:secret@example.com",
            "https://example.com/%ZZ",
            "https://example.com/\n",
        ] {
            for action in [LinkAction::Internal, LinkAction::External] {
                cx.update_window(window.into(), |_, window, cx| {
                    render::activate_link(
                        LinkTarget::new("Bad", url),
                        action,
                        Some(&ui),
                        window,
                        cx,
                    );
                })
                .unwrap();
            }
        }
        cx.run_until_parked();
        assert_eq!(received.borrow().len(), 3);
        assert!(cx.opened_url().is_none());
        // Mail links retain their existing OS handler.
        cx.update_window(window.into(), |_, window, cx| {
            render::activate_link(
                LinkTarget::new("Mail", "mailto:hello@example.com"),
                LinkAction::Internal,
                Some(&ui),
                window,
                cx,
            );
        })
        .unwrap();
        assert_eq!(cx.opened_url().as_deref(), Some("mailto:hello@example.com"));
    }

    #[gpui::test]
    fn preview_reads_unsaved_buffer_without_starting_a_save(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::dark());
        });
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| crate::state::AppState::new());
            FilesSurface::new(state, "chat".into(), false, 1000, 13.0, false, false, cx)
        });
        let preview = window
            .update(cx, |surface, window, cx| {
                let mut document = FileDocument::loading(DocumentKey {
                    chat_id: "chat".into(),
                    checkout_id: Some("checkout".into()),
                    path: "README.md".into(),
                });
                document.set_loaded(zeron_proto::WorkspaceFileText {
                    checkout_id: "checkout".into(),
                    path: "README.md".into(),
                    text: Some("# Disk".into()),
                    content_hash: Some("disk-hash".into()),
                    size: 6,
                    modified_at: None,
                    encoding: zeron_proto::WorkspaceTextEncoding::Utf8,
                    line_ending: Some(zeron_proto::WorkspaceLineEnding::Lf),
                    read_only_reason: None,
                    truncated: false,
                });
                let theme = Theme::of(cx).clone();
                document.editor = Some(super::super::editor::new_file_editor(
                    "# Unsaved",
                    "README.md",
                    false,
                    &theme,
                    window,
                    cx,
                ));
                document.mark_user_edit();
                document.show_markdown = true;
                surface.preview.active = Some("README.md".into());
                surface
                    .preview
                    .documents
                    .insert("README.md".into(), document);
                let editor = surface.preview.documents["README.md"].editor.clone();
                surface
                    .prepare_markdown_preview("README.md", editor.as_ref(), cx)
                    .unwrap();
                let document = &surface.preview.documents["README.md"];
                assert!(document.is_dirty());
                assert!(document.save_task.is_none());
                assert!(document.autosave_task.is_none());
                document.markdown.clone().unwrap()
            })
            .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        preview.read_with(cx, |preview, _| {
            let crate::markdown::parser::Block::Heading { runs, .. } =
                &preview.test_tree().blocks[0].block
            else {
                panic!("missing heading");
            };
            assert_eq!(runs[0].text, "Unsaved");
        });
        let weak = preview.downgrade();
        drop(preview);
        window
            .update(cx, |surface, _, cx| {
                surface.preview.documents.clear();
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        assert!(weak.upgrade().is_none());
    }
}
