//! Native, virtualized preview of a file's current Markdown buffer.
use crate::image_media::release_media;
use crate::{
    markdown::{
        parser::{self, Block, BlockTree},
        render::{self, LinkUi, RenderCache, RenderOptions},
    },
    theme::Theme,
};
use gpui::{
    AnyElement, Context, FocusHandle, ListAlignment, ListOffset, ListSizingBehavior, ListState,
    Render, SharedString, Task, Window, div, list, prelude::*, px,
};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::Arc,
    time::Duration,
};

const MAX_MEDIA_BYTES: usize = 64 * 1024 * 1024;
const MAX_MEDIA_ENTRIES: usize = 32;
const MAX_MARKDOWN_BYTES: usize = 2 * 1024 * 1024;
const MAX_PREVIEW_CONTENT_WIDTH: f32 = 900.0;

/// A visual block cites its first source line. Notes on inner lines (for
/// example list items or fenced code) remain attached to that containing block.
fn block_source_lines(source: &str, tree: &BlockTree) -> Vec<u32> {
    let starts: Vec<usize> = std::iter::once(0)
        .chain(source.match_indices('\n').map(|(offset, _)| offset + 1))
        .collect();
    tree.blocks
        .iter()
        .map(|top| starts.partition_point(|start| *start <= top.range.start) as u32)
        .collect()
}

fn comment_block(lines: &[u32], line: u32) -> Option<usize> {
    (!lines.is_empty()).then(|| {
        lines
            .partition_point(|start| *start <= line)
            .saturating_sub(1)
    })
}

pub(super) fn is_markdown(path: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, ext)| matches!(ext.to_ascii_lowercase().as_str(), "md" | "markdown"))
}

fn preview_link_outcome(activation: &render::LinkActivation) -> render::LinkOutcome {
    let target = &activation.target.original;
    // File previews already support mail links; chat's web-only policy does
    // not replace this surface's specialized routing.
    if !target.chars().any(char::is_control)
        && url::Url::parse(target).is_ok_and(|url| url.scheme() == "mailto")
    {
        render::LinkOutcome::External(target.clone())
    } else {
        activation.web_outcome(false)
    }
}

/// URL path resolution is independent of the UI host's filesystem.
pub(super) fn relative_target(document: &str, target: &str) -> Option<(String, Option<String>)> {
    if let Some(target) = target.strip_prefix("zeron-file:") {
        return relative_target("", target);
    }
    if target.starts_with('/') || target.contains(':') || target.contains('\\') {
        return None;
    }
    let (path, anchor) = target
        .split_once('#')
        .map_or((target, None), |(p, a)| (p, Some(a.to_string())));
    let decode = |s: &str| -> Option<String> {
        let mut bytes = Vec::new();
        let mut chars = s.as_bytes().iter().copied();
        while let Some(c) = chars.next() {
            if c == b'%' {
                let a = (chars.next()? as char).to_digit(16)?;
                let b = (chars.next()? as char).to_digit(16)?;
                bytes.push((a * 16 + b) as u8);
            } else {
                bytes.push(c);
            }
        }
        String::from_utf8(bytes).ok()
    };
    let path = decode(path)?;
    if path.contains(['\\', ':', '\0']) || path.starts_with('/') {
        return None;
    }
    if path.is_empty() {
        return Some((document.into(), anchor.and_then(|a| decode(&a))));
    }
    let mut parts: Vec<&str> = document.split('/').collect();
    parts.pop();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            _ => parts.push(part),
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some((parts.join("/"), anchor.and_then(|a| decode(&a))))
}

pub(super) struct MarkdownPreview {
    pub focus: FocusHandle,
    pub version: Option<(u64, u64, Option<String>)>,
    path: String,
    media_location: Option<(Option<String>, String)>,
    scope: String,
    tree: BlockTree,
    parsed_source: Arc<str>,
    block_lines: Vec<u32>,
    comment_owner: Option<gpui::WeakEntity<super::FilesSurface>>,
    comments: Vec<crate::comments::ReviewComment>,
    comment_draft: Option<(u32, gpui::Entity<crate::composer::ComposerInput>, bool)>,
    pub editor: Option<gpui::WeakEntity<super::editor::FileEditorState>>,
    list: ListState,
    cache: Rc<RefCell<RenderCache>>,
    highlights: HashMap<usize, Arc<zeron_syntax::HighlightedDocument>>,
    anchors: HashMap<String, usize>,
    parse_task: Option<Task<()>>,
    epoch: u64,
    loading: bool,
    truncated: bool,
    pub media_client: Option<(super::client::WorkspaceFilesClient, String)>,
    images: HashMap<String, Result<crate::image_media::MediaImage, String>>,
    image_task: Option<Task<()>>,
    image_generation: u64,
    media_dirty: bool,
    image_allowed: Rc<HashSet<String>>,
    diagram_allowed: Rc<HashSet<String>>,
    image_snapshot: Rc<HashMap<String, Result<crate::image_media::MediaImage, String>>>,
    diagram_snapshot: Rc<HashMap<String, Result<crate::image_media::MediaImage, String>>>,
    visible_rows: HashSet<gpui::SharedString>,
    code_fences: HashMap<SharedString, render::CodeFenceRuntime>,
    code_fences_generation: u64,
    copied_code: Option<(SharedString, usize)>,
    copied_clear: Option<Task<()>>,
    needs_focus: bool,
    suspended: bool,
    selection_pointer: Option<gpui::Point<gpui::Pixels>>,
    selection_task: Option<Task<()>>,
    diagrams: HashMap<String, Result<crate::image_media::MediaImage, String>>,
    diagram_task: Option<Task<()>>,
    diagram_style: u32,
    source_visible: HashSet<String>,
    preview_image: Option<crate::attachments::PreviewImage>,
    zoom_source: Option<crate::image_media::MediaImage>,
    zoom_render: Option<crate::image_media::MediaImage>,
    preview_focus: FocusHandle,
    open_file: Rc<dyn Fn(String, &mut gpui::App)>,
    open_web_link: Option<WebLinkHandler>,
}

pub(super) type WebLinkHandler = Rc<dyn Fn(&render::LinkActivation, &mut gpui::App)>;

impl MarkdownPreview {}

#[path = "markdown_preview/media.rs"]
mod media;

impl MarkdownPreview {
    fn render_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.render_rows(ix..ix + 1, window, cx)
            .pop()
            .unwrap_or_else(|| gpui::Empty.into_any_element())
    }

    pub(super) fn set_web_link_handler(&mut self, handler: WebLinkHandler) {
        self.open_web_link = Some(handler);
    }

    pub(super) fn link_ui(&self, cx: &Context<Self>) -> LinkUi {
        let weak = cx.weak_entity();
        let open_web_link = self.open_web_link.clone();
        LinkUi {
            source_session: None,
            handler: Rc::new(move |activation, _, cx| {
                if weak.upgrade().is_none() {
                    return render::LinkOutcome::Rejected;
                }
                if activation.target.navigation.is_ok() {
                    if let Some(open_web_link) = &open_web_link {
                        // Emit through the owning FilesSurface before borrowing
                        // this preview: selecting Browser can suspend this view.
                        open_web_link(activation, cx);
                        return render::LinkOutcome::Internal;
                    }
                }
                let target = &activation.target.original;
                weak.update(cx, |view, cx| {
                    let Some((path, anchor)) = relative_target(&view.path, target) else {
                        return preview_link_outcome(activation);
                    };
                    if path == view.path {
                        if let Some(ix) = anchor.as_ref().and_then(|a| view.anchors.get(a)) {
                            view.list.scroll_to(ListOffset {
                                item_ix: *ix,
                                offset_in_item: px(0.0),
                            });
                            cx.notify();
                        }
                    } else {
                        (view.open_file)(path, cx);
                    }
                    render::LinkOutcome::Internal
                })
                .unwrap_or(render::LinkOutcome::Rejected)
            }),
        }
    }

    fn render_rows(
        &mut self,
        range: std::ops::Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = Theme::of(cx).clone();
        let link = self.link_ui(cx);
        range
            .filter_map(|ix| {
                let top = self.tree.blocks.get(ix)?.clone();
                let mut opts = RenderOptions::settled(format!("{}{ix}", self.scope).into());
                opts.code = self.code_uis_for(&opts.row_key, &top.block, ix, cx);
                let editor = self.editor.as_ref().and_then(|editor| editor.upgrade());
                let source = self.parsed_source.clone();
                opts.tasks = Some(render::TaskUi {
                    toggle: editor
                        .filter(|editor| {
                            !self.truncated
                                && editor.read(cx).context_menu_capabilities().is_editable()
                        })
                        .map(|editor| {
                            let editor = editor.downgrade();
                            Rc::new(
                                move |task: &parser::TaskMarker,
                                      window: &mut Window,
                                      cx: &mut gpui::App| {
                                    if let Some(editor) = editor.upgrade() {
                                        super::editor::toggle_markdown_task(
                                            &editor, &source, task, window, cx,
                                        );
                                    }
                                },
                            )
                                as Rc<dyn Fn(&parser::TaskMarker, &mut Window, &mut gpui::App)>
                        }),
                });
                self.visible_rows.insert(opts.row_key.clone());
                opts.cache = Some(self.cache.clone());
                let images = self.image_snapshot.clone();
                let image_owner = cx.weak_entity();
                let image_link = link.clone();
                let diagram_owner = cx.weak_entity();
                let diagrams = self.diagram_snapshot.clone();
                let source_visible = self.source_visible.clone();
                let image_allowed = self.image_allowed.clone();
                let diagram_allowed = self.diagram_allowed.clone();
                opts.media = Some(render::MediaUi {
                    diagram: Some(Rc::new(move |code, id, theme| {
                        let state = diagrams.get(code);
                        let allowed = diagram_allowed.contains(code);
                        let source_shown = source_visible.contains(id.as_ref()) || !allowed;
                        let owner = diagram_owner.clone();
                        let toggle_id = id.to_string();
                        let body = match state {
                            Some(Ok(loaded)) => Self::media_element(
                                loaded,
                                format!("{id}-image").into(),
                                "Mermaid diagram".into(),
                                diagram_owner.clone(),
                            ),
                            Some(Err(error)) => div()
                                .p(px(12.0))
                                .text_size(px(12.0))
                                .text_color(theme.warning_muted)
                                .child(error.clone())
                                .into_any_element(),
                            None => div()
                                .p(px(12.0))
                                .text_color(theme.text_muted)
                                .child(if diagram_allowed.contains(code) {
                                    "Rendering diagram…"
                                } else {
                                    "Document diagram preview limit reached"
                                })
                                .into_any_element(),
                        };
                        render::DiagramUi {
                            body,
                            show_source: source_shown,
                            toggle_source: Rc::new(move |_, cx| {
                                let _ = owner.update(cx, |view, cx| {
                                    if !view.source_visible.remove(&toggle_id) {
                                        view.source_visible.insert(toggle_id.clone());
                                    }
                                    view.list.remeasure_items(0..view.tree.len());
                                    cx.notify();
                                });
                            }),
                        }
                    })),
                    image: Rc::new(move |image, id, theme| match images.get(&image.source) {
                        Some(Ok(loaded)) => {
                            let mut el =
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.0))
                                    .child(Self::media_element(
                                        loaded,
                                        id,
                                        if image.alt.is_empty() {
                                            image.source.clone()
                                        } else {
                                            image.alt.clone()
                                        },
                                        image_owner.clone(),
                                    ));
                            if let Some(target) = image.link.clone() {
                                let link = image_link.clone();
                                el = el.child(
                                    super::toolbar_button("markdown-image-link", "Open image link")
                                        .on_click(move |_, window, cx| {
                                            render::activate_link(
                                                render::LinkTarget::new(&target, &target),
                                                render::LinkAction::Primary,
                                                Some(&link),
                                                window,
                                                cx,
                                            );
                                        })
                                        .child(
                                            crate::icons::icon(crate::icons::ARROW_UP_RIGHT)
                                                .size(px(crate::surface_chrome::ICON_SIZE))
                                                .text_color(theme.text_muted),
                                        ),
                                );
                            }
                            el.into_any_element()
                        }
                        state => {
                            let text = if image.source.starts_with("https://")
                                || image.source.starts_with("http://")
                            {
                                format!("{} — {}", image.alt, image.source)
                            } else {
                                format!(
                                    "{} — {}",
                                    image.alt,
                                    state
                                        .and_then(|s| s.as_ref().err())
                                        .map(String::as_str)
                                        .unwrap_or(if image_allowed.contains(&image.source) {
                                            "Loading image…"
                                        } else {
                                            "Document image preview limit reached"
                                        })
                                )
                            };
                            let target = image.source.clone();
                            let external =
                                target.starts_with("https://") || target.starts_with("http://");
                            div()
                                .id(id)
                                .text_color(theme.text_muted)
                                .child(text)
                                .when(external, |el| {
                                    let link = image_link.clone();
                                    el.cursor_pointer().on_click(move |_, window, cx| {
                                        render::activate_link(
                                            render::LinkTarget::new(&target, &target),
                                            render::LinkAction::Primary,
                                            Some(&link),
                                            window,
                                            cx,
                                        );
                                    })
                                })
                                .into_any_element()
                        }
                    }),
                });
                opts.link = Some(link.clone());
                opts.copy = Some(self.copy_ui_for(&opts.row_key, cx));
                let group: gpui::SharedString = format!("{}-comment-{ix}", self.scope).into();
                let comment_line = self.block_lines.get(ix).copied().filter(|_| {
                    self.comment_owner.is_some()
                        && !self.truncated
                        && self
                            .editor
                            .as_ref()
                            .and_then(|e| e.upgrade())
                            .is_some_and(|e| e.read(cx).context_menu_capabilities().is_editable())
                });
                let comments = self.comment_elements(ix, &theme, cx);
                let comment_source = self.parsed_source.clone();
                Some(
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .pb(px(render::MD_BLOCK_GAP))
                        // Include the comment gutter so moving from the block to
                        // its button never leaves the hover group.
                        .group(group.clone())
                        .child(
                            div().w_full().flex().justify_center().px(px(24.0)).child(
                                div()
                                    .w_full()
                                    .max_w(px(MAX_PREVIEW_CONTENT_WIDTH))
                                    .min_w_0()
                                    .relative()
                                    .when_some(comment_line, |el, line| {
                                        el.child(
                                            div()
                                                .absolute()
                                                .left(px(-20.0))
                                                .top(px(3.0))
                                                .opacity(0.0)
                                                .group_hover(group, |style| style.opacity(1.0))
                                                .child(crate::comment_ui::render_comment_adder(
                                                    format!("{}-comment-add-{ix}", self.scope)
                                                        .into(),
                                                    &theme,
                                                    cx,
                                                    move |this, window, cx| {
                                                        this.open_comment(
                                                            line,
                                                            &comment_source,
                                                            window,
                                                            cx,
                                                        )
                                                    },
                                                )),
                                        )
                                    })
                                    .child(render::render_block(
                                        &top.block,
                                        ix,
                                        ix,
                                        &opts,
                                        &theme,
                                        window,
                                        self.highlights.get(&ix).map(|h| h.lines.as_slice()),
                                    )),
                            ),
                        )
                        .children(comments)
                        .into_any_element(),
                )
            })
            .collect()
    }
}

#[path = "markdown_preview/view.rs"]
mod view;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_retains_mail_links_without_allowing_active_schemes() {
        for (url, allowed) in [
            ("mailto:reader@example.com", true),
            ("https://example.com", true),
            ("javascript:alert(1)", false),
        ] {
            let a = render::LinkActivation {
                target: render::LinkTarget::new("label", url),
                action: render::LinkAction::Internal,
                source_session: None,
            };
            assert_eq!(
                matches!(preview_link_outcome(&a), render::LinkOutcome::External(_)),
                allowed
            );
        }
    }
    #[test]
    fn comments_map_to_original_lines_and_containing_blocks() {
        let source = "# Título 🦀\r\n\r\nPárrafo\r\nsegunda línea\r\n\r\n- uno\r\n- dos\r\n\r\n```mermaid\r\ngraph TD; A-->B\r\n```\r\n";
        let lines = block_source_lines(source, &parser::parse_full(source));
        assert_eq!(lines, [1, 3, 6, 9]);
        assert_eq!(comment_block(&lines, 4), Some(1));
        assert_eq!(comment_block(&lines, 7), Some(2));
        assert_eq!(comment_block(&lines, 10), Some(3));
        assert_eq!(comment_block(&lines, 11), Some(3));
        assert_eq!(comment_block(&[], 1), None);
    }
    #[test]
    fn markdown_paths_and_links() {
        assert!(is_markdown("docs/README.MD"));
        assert!(is_markdown("x.markdown"));
        assert!(!is_markdown("x.mdx"));
        assert_eq!(
            relative_target("docs/readme.md", "../a%20b.md#hello"),
            Some(("a b.md".into(), Some("hello".into())))
        );
        assert_eq!(relative_target("readme.md", "../secret"), None);
        assert_eq!(relative_target("docs/readme.md", "%2Fetc/passwd"), None);
        assert_eq!(
            relative_target("docs/readme.md", "https://example.com"),
            None
        );
        assert_eq!(
            relative_target("docs/readme.md", "#hello"),
            Some(("docs/readme.md".into(), Some("hello".into())))
        );
    }
}

#[cfg(all(test, target_os = "linux"))]
mod layout_tests {
    use super::*;
    use gpui::{AppContext, Bounds, Point};

    struct TaskDocument {
        preview: gpui::Entity<MarkdownPreview>,
        editor: gpui::Entity<super::super::editor::FileEditorState>,
    }

    impl Render for TaskDocument {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(div().h(px(250.0)).child(self.preview.clone()))
                .child(super::super::editor::editor_element(&self.editor))
        }
    }

    #[test]
    fn code_fences_scroll_copy_and_share_persistent_fit_preference() {
        gpui_platform::headless().run(|cx| {
            let settings_dir = tempfile::tempdir().unwrap();
            crate::settings::init(
                crate::settings::UiSettings::default(),
                settings_dir.path(),
                cx,
            );
            cx.set_global(Theme::dark());
            let code = format!("```text\n{}\n```", "long-line-".repeat(100));
            let window = cx
                .open_window(
                    gpui::WindowOptions {
                        window_bounds: Some(gpui::WindowBounds::Windowed(Bounds::new(
                            Point::default(),
                            gpui::size(px(600.0), px(400.0)),
                        ))),
                        ..Default::default()
                    },
                    |_, cx| {
                        cx.new(|cx| {
                            let mut view =
                                MarkdownPreview::new("README.md".into(), Rc::new(|_, _| {}), cx);
                            view.tree = parser::parse_full(&code);
                            view.list.reset(1);
                            view.diagram_style = crate::theme::style_generation();
                            view
                        })
                    },
                )
                .unwrap();
            let view = window.entity(cx).unwrap();
            cx.update_window(window.into(), |_, window, cx| {
                window.refresh();
                let _ = window.draw(cx);
            })
            .unwrap();
            assert!(
                view.read(cx)
                    .code_fences
                    .values()
                    .next()
                    .unwrap()
                    .scroll
                    .max_offset()
                    .x
                    > px(0.0),
                "nowrap fences expose a real horizontal scroll plane"
            );

            let row_key: SharedString = format!("{}0", view.read(cx).scope).into();
            let ui = view.update(cx, |view, cx| view.code_ui_for(&row_key, 0, cx));
            cx.update_window(window.into(), |_, window, cx| {
                (ui.viewport_hover)(true, window, cx)
            })
            .unwrap();
            cx.update_window(window.into(), |_, window, cx| {
                window.refresh();
                let _ = window.draw(cx);
            })
            .unwrap();
            let ui = view.update(cx, |view, cx| view.code_ui_for(&row_key, 0, cx));
            assert!(
                ui.scrollbar.is_some(),
                "overflow thumb appears while hovered"
            );

            let copy = view.update(cx, |view, cx| view.copy_ui_for(&row_key, cx));
            cx.update_window(window.into(), |_, window, cx| {
                (copy.handler)(0, "copied source".into(), window, cx);
            })
            .unwrap();
            assert_eq!(view.read(cx).copied_code, Some((row_key.clone(), 0)));
            assert_eq!(
                view.update(cx, |view, cx| view.copy_ui_for(&row_key, cx))
                    .copied_ix,
                Some(0),
                "the next frame renders the Copied confirmation"
            );
            cx.update_window(window.into(), |_, window, cx| (ui.toggle_fit)(window, cx))
                .unwrap();
            assert!(crate::settings::current(cx).code_fences_fit_content);
            cx.update_window(window.into(), |_, window, cx| {
                window.refresh();
                let _ = window.draw(cx);
            })
            .unwrap();
            assert_eq!(
                view.read(cx)
                    .code_fences
                    .values()
                    .next()
                    .unwrap()
                    .scroll
                    .offset(),
                gpui::Point::default(),
                "changing the persistent mode resets ephemeral offsets"
            );
            let second =
                cx.new(|cx| MarkdownPreview::new("SECOND.md".into(), Rc::new(|_, _| {}), cx));
            let second_row: SharedString = format!("{}0", second.read(cx).scope).into();
            assert!(
                second
                    .update(cx, |view, cx| view.code_ui_for(&second_row, 0, cx))
                    .fit_content
            );
            cx.spawn(async move |cx| cx.update(|cx| cx.quit())).detach();
        });
    }

    #[test]
    fn task_click_edits_buffer_and_supports_undo_redo_and_stale_guards() {
        gpui_platform::headless().run(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::dark());
            let source = "- [ ] tarea 🦀\n- [X] tarea 🦀\n";
            let window = cx
                .open_window(gpui::WindowOptions::default(), |window, cx| {
                    let editor = cx.new(|cx| {
                        gpui_base::input::EditorState::new(window, cx).default_value(source)
                    });
                    let preview = cx.new(|cx| {
                        let mut view =
                            MarkdownPreview::new("README.md".into(), Rc::new(|_, _| {}), cx);
                        view.tree = parser::parse_full(source);
                        view.parsed_source = Arc::from(source);
                        view.editor = Some(editor.downgrade());
                        view.list.reset(view.tree.len());
                        view.diagram_style = crate::theme::style_generation();
                        view
                    });
                    cx.new(|_| TaskDocument { preview, editor })
                })
                .unwrap();
            let root = window.entity(cx).unwrap();
            let preview = root.read(cx).preview.clone();
            let editor = root.read(cx).editor.clone();
            let changes = Rc::new(std::cell::Cell::new(0));
            let observed = changes.clone();
            let _subscription = cx.subscribe(&editor, move |_, event, _| {
                if matches!(event, gpui_base::input::InputEvent::Change) {
                    observed.set(observed.get() + 1);
                }
            });
            cx.update_window(window.into(), |_, window, cx| {
                window.refresh();
                let _ = window.draw(cx);
                let bounds = preview.read(cx).list.bounds_for_item(0).unwrap();
                let gutter =
                    ((bounds.size.width - px(MAX_PREVIEW_CONTENT_WIDTH)) / 2.0).max(px(24.0));
                let position =
                    gpui::point(bounds.left() + gutter + px(8.0), bounds.top() + px(11.0));
                window.dispatch_event(
                    gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                        button: gpui::MouseButton::Left,
                        position,
                        click_count: 1,
                        ..Default::default()
                    }),
                    cx,
                );
                window.dispatch_event(
                    gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                        button: gpui::MouseButton::Left,
                        position,
                        click_count: 1,
                        ..Default::default()
                    }),
                    cx,
                );
            })
            .unwrap();
            assert_eq!(
                editor.read(cx).value().as_ref(),
                "- [x] tarea 🦀\n- [X] tarea 🦀\n"
            );
            assert_eq!(
                changes.get(),
                1,
                "click must emit the normal document change event"
            );
            cx.update_window(window.into(), |_, window, cx| {
                use gpui::Focusable;
                window.focus(&editor.focus_handle(cx), cx);
                window.refresh();
                let _ = window.draw(cx);
                window.dispatch_action(Box::new(gpui_base::input::Undo), cx);
            })
            .unwrap();
            assert_eq!(editor.read(cx).value().as_ref(), source);
            cx.update_window(window.into(), |_, window, cx| {
                window.dispatch_action(Box::new(gpui_base::input::Redo), cx);
            })
            .unwrap();
            assert_eq!(
                editor.read(cx).value().as_ref(),
                "- [x] tarea 🦀\n- [X] tarea 🦀\n"
            );
            cx.update_window(window.into(), |_, window, cx| {
                let current = editor.read(cx).value().to_string();
                let start = current.find("[X]").unwrap();
                let task = parser::TaskMarker {
                    checked: true,
                    range: start..start + 3,
                };
                super::super::editor::toggle_markdown_task(&editor, source, &task, window, cx);
                assert_eq!(
                    editor.read(cx).value().as_ref(),
                    current,
                    "stale source must be ignored"
                );
                editor.update(cx, |editor, cx| editor.set_readonly(true, cx));
                super::super::editor::toggle_markdown_task(&editor, &current, &task, window, cx);
                assert_eq!(editor.read(cx).value().as_ref(), current);
                editor.update(cx, |editor, cx| editor.set_readonly(false, cx));
                super::super::editor::toggle_markdown_task(&editor, &current, &task, window, cx);
                assert_eq!(
                    editor.read(cx).value().as_ref(),
                    "- [x] tarea 🦀\n- [ ] tarea 🦀\n"
                );
            })
            .unwrap();
            cx.spawn(async move |cx| {
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    }

    struct Pair(gpui::Entity<MarkdownPreview>, gpui::Entity<MarkdownPreview>);
    impl Render for Pair {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .flex()
                .child(div().w(px(400.0)).h_full().child(self.0.clone()))
                .child(render::selection_frame_reset())
                .child(div().w(px(400.0)).h_full().child(self.1.clone()))
        }
    }

    #[test]
    fn selection_stays_in_its_document_with_two_markdown_surfaces() {
        gpui_platform::headless().run(|cx| {
            cx.set_global(Theme::dark());
            let make = |text: &str, cx: &mut gpui::App| {
                cx.new(|cx| {
                    let mut view = MarkdownPreview::new("README.md".into(), Rc::new(|_, _| {}), cx);
                    view.tree = parser::parse_full(text);
                    view.list.reset(view.tree.len());
                    view.diagram_style = crate::theme::style_generation();
                    view
                })
            };
            let left = make("Left document text", cx);
            let right = make("Right document text", cx);
            let left_key = format!("{}0:0", left.read(cx).scope);
            let right_key = format!("{}0:0", right.read(cx).scope);
            let window = cx
                .open_window(
                    gpui::WindowOptions {
                        window_bounds: Some(gpui::WindowBounds::Windowed(Bounds::new(
                            Point::default(),
                            gpui::size(px(800.0), px(600.0)),
                        ))),
                        ..Default::default()
                    },
                    |_, cx| cx.new(|_| Pair(left, right)),
                )
                .unwrap();
            cx.update_window(window.into(), |_, window, cx| {
                window.refresh();
                let _ = window.draw(cx);
            })
            .unwrap();
            let left_bounds = render::selection_test_bounds(&left_key);
            let right_bounds = render::selection_test_bounds(&right_key);
            assert!(right_bounds.left() > left_bounds.left());
            crate::markdown::selection::begin(&left_key, 0);
            assert!(render::update_drag_at(gpui::point(
                right_bounds.right(),
                right_bounds.top() + px(3.0)
            )));
            assert_eq!(
                crate::markdown::selection::selected_text().as_deref(),
                Some("Left document text")
            );
            crate::markdown::selection::clear_if_owner(&left_key);
            cx.spawn(async move |cx| {
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    }

    #[test]
    fn rendered_image_opens_centered_lightbox_and_escape_restores_focus() {
        gpui_platform::headless().run(|cx| {
            cx.set_global(Theme::dark());
            let window = cx.open_window(gpui::WindowOptions { window_bounds: Some(gpui::WindowBounds::Windowed(Bounds::new(Point::default(), gpui::size(px(1200.0), px(600.0))))), ..Default::default() }, |_, cx| {
                cx.new(|cx| {
                    let mut view = MarkdownPreview::new("README.md".into(), Rc::new(|_, _| {}), cx);
                    view.tree = parser::parse_full("![Example](example.svg)"); view.list.reset(1);
                    view.diagram_style = crate::theme::style_generation();
                    let media = crate::image_media::decode_image("image/svg+xml", br##"<svg xmlns="http://www.w3.org/2000/svg" width="240" height="160"><rect width="240" height="160" fill="#468"/></svg>"##.to_vec()).unwrap();
                    view.images.insert("example.svg".into(), Ok(media));
                    view
                })
            }).unwrap();
            let view = window.entity(cx).unwrap();
            cx.update_window(window.into(), |_, window, cx| { window.refresh(); let _ = window.draw(cx); }).unwrap();
            let bounds = view.read(cx).list.bounds_for_item(0).unwrap();
            assert!(bounds.size.height > px(100.0));
            let position = bounds.center();
            cx.update_window(window.into(), |_, window, cx| {
                window.dispatch_event(gpui::PlatformInput::MouseDown(gpui::MouseDownEvent { button: gpui::MouseButton::Left, position, click_count: 1, ..Default::default() }), cx);
                window.dispatch_event(gpui::PlatformInput::MouseUp(gpui::MouseUpEvent { button: gpui::MouseButton::Left, position, click_count: 1, ..Default::default() }), cx);
            }).unwrap();
            assert!(view.read(cx).preview_image.is_some());
            cx.update_window(window.into(), |_, window, cx| {
                assert!(view.read(cx).preview_focus.is_focused(window));
                window.refresh(); let _ = window.draw(cx);
                window.dispatch_event(gpui::PlatformInput::KeyDown(gpui::KeyDownEvent { keystroke: gpui::Keystroke::parse("escape").unwrap(), is_held: false, prefer_character_input: false }), cx);
            }).unwrap();
            assert!(view.read(cx).preview_image.is_none());
            assert!(view.read(cx).zoom_source.is_none());
            assert!(view.read(cx).zoom_render.is_none());
            cx.update_window(window.into(), |_, window, cx| { assert!(view.read(cx).focus.is_focused(window)); }).unwrap();
            cx.spawn(async move |cx| { cx.update(|cx| cx.quit()); }).detach();
        });
    }

    #[test]
    fn rendered_mermaid_opens_lightbox_and_toggles_source() {
        gpui_platform::headless().run(|cx| {
            cx.set_global(Theme::dark());
            let window = cx
                .open_window(
                    gpui::WindowOptions {
                        window_bounds: Some(gpui::WindowBounds::Windowed(Bounds::new(
                            Point::default(),
                            gpui::size(px(1200.0), px(600.0)),
                        ))),
                        ..Default::default()
                    },
                    |_, cx| {
                        cx.new(|cx| {
                            let mut view = MarkdownPreview::new(
                                "README.md".into(),
                                Rc::new(|_, _| {}),
                                cx,
                            );
                            view.tree = parser::parse_full(
                                "```mermaid\nflowchart LR\n    A --> B\n```",
                            );
                            let Block::CodeBlock { code, .. } = &view.tree.blocks[0].block else {
                                panic!("missing Mermaid block");
                            };
                            let media = crate::image_media::decode_image(
                                "image/svg+xml",
                                br##"<svg xmlns="http://www.w3.org/2000/svg" width="240" height="160"><rect width="240" height="160" fill="#468"/></svg>"##.to_vec(),
                            )
                            .unwrap();
                            view.diagrams.insert(code.clone(), Ok(media));
                            view.diagram_allowed = Rc::new(HashSet::from([code.clone()]));
                            view.list.reset(1);
                            view.diagram_style = crate::theme::style_generation();
                            view
                        })
                    },
                )
                .unwrap();
            let view = window.entity(cx).unwrap();
            cx.update_window(window.into(), |_, window, cx| {
                window.refresh();
                let _ = window.draw(cx);
            })
            .unwrap();

            let bounds = view.read(cx).list.bounds_for_item(0).unwrap();
            let image_position =
                gpui::point(bounds.center().x, bounds.top() + px(28.0 + 80.0));
            cx.update_window(window.into(), |_, window, cx| {
                window.dispatch_event(
                    gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                        button: gpui::MouseButton::Left,
                        position: image_position,
                        click_count: 1,
                        ..Default::default()
                    }),
                    cx,
                );
                window.dispatch_event(
                    gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                        button: gpui::MouseButton::Left,
                        position: image_position,
                        click_count: 1,
                        ..Default::default()
                    }),
                    cx,
                );
            })
            .unwrap();
            assert!(view.read(cx).preview_image.is_some());

            cx.update_window(window.into(), |_, window, cx| {
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
                window.refresh();
                let _ = window.draw(cx);
            })
            .unwrap();

            let frame_right = bounds.center().x + px(MAX_PREVIEW_CONTENT_WIDTH / 2.0);
            let toggle_position = gpui::point(frame_right - px(42.0), bounds.top() + px(14.0));
            cx.update_window(window.into(), |_, window, cx| {
                window.dispatch_event(
                    gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                        button: gpui::MouseButton::Left,
                        position: toggle_position,
                        click_count: 1,
                        ..Default::default()
                    }),
                    cx,
                );
                window.dispatch_event(
                    gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                        button: gpui::MouseButton::Left,
                        position: toggle_position,
                        click_count: 1,
                        ..Default::default()
                    }),
                    cx,
                );
            })
            .unwrap();
            assert_eq!(view.read(cx).source_visible.len(), 1);
            assert!(view.read(cx).preview_image.is_none());

            cx.spawn(async move |cx| {
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    }
}

#[cfg(test)]
mod async_tests {
    use super::*;
    use gpui::{AppContext, TestAppContext};

    #[gpui::test]
    fn edits_and_suspension_cancel_obsolete_preview_work(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_global(Theme::dark()));
        let view = cx.new(|cx| MarkdownPreview::new("README.md".into(), Rc::new(|_, _| {}), cx));
        view.update(cx, |view, cx| {
            view.set_source("# Old".into(), false, cx);
            view.set_source("# Current".into(), false, cx);
        });
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            let Block::Heading { runs, .. } = &view.tree.blocks[0].block else {
                panic!("missing heading");
            };
            assert_eq!(runs[0].text, "Current");
        });
        view.update(cx, |view, cx| {
            view.set_source("# Hidden".into(), false, cx);
            view.suspend(cx);
        });
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert!(view.tree.is_empty());
            assert!(view.images.is_empty());
            assert!(view.diagrams.is_empty());
        });
        let weak = view.downgrade();
        drop(view);
        cx.run_until_parked();
        assert!(weak.upgrade().is_none());
    }

    #[gpui::test]
    fn raster_admission_accounts_for_cpu_pixels_and_gpu_texture(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_global(Theme::dark()));
        let view = cx.new(|cx| MarkdownPreview::new("README.md".into(), Rc::new(|_, _| {}), cx));
        // The compressed source and one pixel buffer fit, but retaining both
        // decoded CPU pixels and the GPU texture exceeds the document budget.
        let raster = image::RgbaImage::new(3000, 3000);
        let mut png = std::io::Cursor::new(Vec::new());
        raster.write_to(&mut png, image::ImageFormat::Png).unwrap();
        assert!(png.get_ref().len() < zeron_proto::MAX_WORKSPACE_IMAGE_BYTES);
        let media = crate::image_media::decode_image("image/png", png.into_inner()).unwrap();
        view.read_with(cx, |view, _| {
            assert!(view.admit_media(Ok(media)).is_err());
        });
    }

    #[gpui::test]
    fn referenced_image_change_invalidates_cache_and_memory_is_bounded(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_global(Theme::dark()));
        let view =
            cx.new(|cx| MarkdownPreview::new("docs/README.md".into(), Rc::new(|_, _| {}), cx));
        view.update(cx, |view, cx| {
            view.tree = parser::parse_full("![a](../image.png)");
            let mut media = crate::image_media::decode_image(
                "image/svg+xml",
                br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"/>"#.to_vec(),
            )
            .unwrap();
            media.bytes = MAX_MEDIA_BYTES;
            assert!(view.admit_media(Ok(media.clone())).is_ok());
            view.images.insert("../image.png".into(), Ok(media.clone()));
            assert!(view.admit_media(Ok(media)).is_err());
            view.invalidate_images(Some("unrelated.png"), cx);
            assert!(view.images["../image.png"].is_ok());
            view.invalidate_images(Some("image.png"), cx);
            assert!(view.images["../image.png"].is_err());
            let generation = view.image_generation;
            view.activate(
                "docs/README.md",
                &(Some("other-device".into()), "other-checkout".into()),
                cx,
            );
            assert!(view.images.is_empty());
            assert!(view.image_generation > generation);
            assert!(view.version.is_none());
        });
    }
}
