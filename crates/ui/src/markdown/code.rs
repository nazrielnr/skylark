//! Markdown fenced-code interaction state and rendering.

use super::*;

type HoverHandler = Rc<dyn Fn(bool, &mut Window, &mut gpui::App)>;
type PointerHandler = Rc<dyn Fn(gpui::Pixels, &mut Window, &mut gpui::App)>;
type ReleaseHandler = Rc<dyn Fn(&mut Window, &mut gpui::App)>;

/// Transcript-owned interaction for the one fenced block represented by a
/// virtualized Markdown row. The renderer owns only layout/chrome; callbacks
/// keep durable preference and mutable scroll state in [`crate::transcript`].
#[derive(Clone)]
pub struct CodeUi {
    pub key: SharedString,
    pub fit_content: bool,
    pub scroll: gpui::ScrollHandle,
    pub scrollbar: Option<CodeScrollbarUi>,
    pub toggle_fit: Rc<dyn Fn(&mut Window, &mut gpui::App)>,
    pub viewport_hover: HoverHandler,
    pub drag_move: PointerHandler,
}

#[derive(Clone)]
pub struct CodeScrollbarUi {
    pub metrics: crate::popover::HorizontalScrollbarMetrics,
    pub active: bool,
    pub hover: HoverHandler,
    pub press: PointerHandler,
    pub release: ReleaseHandler,
}

#[derive(Default)]
pub struct CodeFenceRuntime {
    pub scroll: gpui::ScrollHandle,
    pub scrollbar: crate::popover::HorizontalScrollbarState,
}

/// Build the interaction state shared by chat and file-preview code fences.
/// The durable Fit choice is global; scroll and drag state stay with one fence.
pub fn code_ui_for<V, F>(
    key: SharedString,
    fit_content: bool,
    runtime: &mut CodeFenceRuntime,
    entity: gpui::WeakEntity<V>,
    runtimes: F,
) -> CodeUi
where
    V: 'static,
    F: Fn(&mut V) -> &mut HashMap<SharedString, CodeFenceRuntime> + Clone + 'static,
{
    let scroll = runtime.scroll.clone();
    let scrollbar = (!fit_content)
        .then(|| runtime.scrollbar.metrics(&scroll))
        .flatten()
        .filter(|_| runtime.scrollbar.visible())
        .map(|metrics| CodeScrollbarUi {
            metrics,
            active: runtime.scrollbar.active(),
            hover: {
                let entity = entity.clone();
                let key = key.clone();
                let runtimes = runtimes.clone();
                Rc::new(move |hovered, _, cx| {
                    let _ = entity.update(cx, |owner, cx| {
                        if runtimes(owner)
                            .get_mut(&key)
                            .is_some_and(|runtime| runtime.scrollbar.set_bar_hovered(hovered))
                        {
                            cx.notify();
                        }
                    });
                })
            },
            press: {
                let entity = entity.clone();
                let key = key.clone();
                let runtimes = runtimes.clone();
                Rc::new(move |pointer_x, _, cx| {
                    let _ = entity.update(cx, |owner, cx| {
                        let Some(runtime) = runtimes(owner).get_mut(&key) else {
                            return;
                        };
                        let scroll = runtime.scroll.clone();
                        if runtime.scrollbar.begin_press(&scroll, pointer_x) {
                            cx.stop_propagation();
                            cx.notify();
                        }
                    });
                })
            },
            release: {
                let entity = entity.clone();
                let key = key.clone();
                let runtimes = runtimes.clone();
                Rc::new(move |_, cx| {
                    let _ = entity.update(cx, |owner, cx| {
                        if let Some(runtime) = runtimes(owner).get_mut(&key) {
                            runtime.scrollbar.end_press();
                            cx.notify();
                        }
                    });
                })
            },
        });

    CodeUi {
        key: key.clone(),
        fit_content,
        scroll,
        scrollbar,
        toggle_fit: Rc::new(move |_, cx| {
            let fit = !crate::settings::current(cx).code_fences_fit_content;
            crate::settings::update(crate::settings::SavePolicy::Immediate, cx, |settings| {
                settings.code_fences_fit_content = fit
            });
            cx.refresh_windows();
        }),
        viewport_hover: {
            let entity = entity.clone();
            let key = key.clone();
            let runtimes = runtimes.clone();
            Rc::new(move |hovered, _, cx| {
                let _ = entity.update(cx, |owner, cx| {
                    if runtimes(owner)
                        .get_mut(&key)
                        .is_some_and(|runtime| runtime.scrollbar.set_viewport_hovered(hovered))
                    {
                        cx.notify();
                    }
                });
            })
        },
        drag_move: {
            Rc::new(move |pointer_x, _, cx| {
                let _ = entity.update(cx, |owner, cx| {
                    let Some(runtime) = runtimes(owner).get_mut(&key) else {
                        return;
                    };
                    let scroll = runtime.scroll.clone();
                    if runtime.scrollbar.drag_to(&scroll, pointer_x) {
                        cx.notify();
                    }
                });
            })
        },
    }
}

#[derive(Clone)]
struct CodeScrollbarDrag {
    key: SharedString,
}

struct CodeScrollbarDragGhost;

impl Render for CodeScrollbarDragGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

struct CodeBlockTooltip(&'static str);

impl Render for CodeBlockTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        div()
            .px(px(8.0))
            .py(px(6.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(theme.border_strong)
            .bg(theme.surface_raised)
            .shadow_md()
            .text_size(px(11.0))
            .text_color(theme.text)
            .child(self.0)
    }
}

/// Per-line highlight tokens for a code block, or `None` while pending.
pub type CodeHighlight<'a> = Option<&'a [Vec<HighlightSpan>]>;

#[allow(clippy::too_many_arguments)]
pub(crate) fn render_code_block(
    language: Option<&str>,
    code: &str,
    top_ix: usize,
    ix: usize,
    opts: &RenderOptions,
    theme: &Theme,
    highlight: CodeHighlight,
) -> AnyElement {
    if language.is_some_and(|l| l.eq_ignore_ascii_case("mermaid")) {
        if let Some(handler) = opts.media.as_ref().and_then(|media| media.diagram.as_ref()) {
            let frame_id: SharedString = format!("{}-mermaid-{ix}", opts.row_key).into();
            let diagram = handler(code, frame_id.clone(), theme);
            let toggle = diagram.toggle_source.clone();
            let toggle_action = code_icon_action(
                format!("{frame_id}-source-toggle").into(),
                if diagram.show_source {
                    "Show diagram"
                } else {
                    "Show source"
                },
                if diagram.show_source {
                    crate::icons::EYE
                } else {
                    crate::icons::FILE_CODE
                },
                Rc::new(move |window, cx| toggle(window, cx)),
                theme,
            );
            if diagram.show_source {
                return render_code_block_source_with_actions(
                    language,
                    code,
                    top_ix,
                    ix,
                    opts,
                    theme,
                    highlight,
                    vec![toggle_action],
                );
            }
            let mut actions = vec![toggle_action];
            actions.extend(code_copy_button(code, ix, opts, theme));
            return code_block_frame(frame_id, language, actions, diagram.body, theme)
                .into_any_element();
        }
    }
    render_code_block_source(language, code, top_ix, ix, opts, theme, highlight)
}

fn code_icon_action(
    id: SharedString,
    label: &'static str,
    icon_path: &'static str,
    handler: Rc<dyn Fn(&mut Window, &mut gpui::App)>,
    theme: &Theme,
) -> AnyElement {
    let fade_key = id.to_string();
    div()
        .id(id)
        .size(px(CODE_ACTION_SIZE))
        .rounded(px(6.0))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .bg(crate::motion::hover_blend(
            &fade_key,
            gpui::transparent_black(),
            crate::theme::ink(0.08),
        ))
        .on_hover(crate::motion::hover_listener(fade_key))
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            handler(window, cx);
        })
        .tooltip(move |_, cx| cx.new(move |_| CodeBlockTooltip(label)).into())
        .child(
            crate::icons::icon(icon_path)
                .size(px(13.0))
                .text_color(theme.text_muted),
        )
        .into_any_element()
}

fn code_copy_button(
    code: &str,
    ix: usize,
    opts: &RenderOptions,
    theme: &Theme,
) -> Option<AnyElement> {
    opts.copy.clone().map(|copy| {
        let copied = copy.copied_ix == Some(ix);
        let code_text: SharedString = code.to_string().into();
        let handler = copy.handler.clone();
        let fade_key = format!("{}-copy{ix}", opts.row_key);
        div()
            .id(SharedString::from(fade_key.clone()))
            .h(px(CODE_ACTION_SIZE))
            .px(px(6.0))
            .rounded(px(5.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.0))
            .cursor_pointer()
            .bg(crate::motion::hover_blend(
                &fade_key,
                gpui::transparent_black(),
                crate::theme::ink(0.08),
            ))
            .on_hover(crate::motion::hover_listener(fade_key))
            .text_size(px(10.5))
            .text_color(theme.text_muted)
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                handler(ix, code_text.clone(), window, cx);
            })
            .child(
                crate::icons::icon(if copied {
                    crate::icons::CHECK
                } else {
                    crate::icons::COPY
                })
                .size(px(12.0))
                .text_color(theme.text_muted),
            )
            .when(copied, |el| el.child(SharedString::from("Copied")))
            .into_any_element()
    })
}

fn code_block_header(
    language: Option<&str>,
    actions: Vec<AnyElement>,
    theme: &Theme,
) -> Option<gpui::Div> {
    if language.is_none() && actions.is_empty() {
        return None;
    }
    Some(
        div()
            .h(px(CODE_HEADER_HEIGHT))
            .flex_none()
            .pl(px(CODE_PADDING_X))
            .pr(px(5.0))
            .border_b_1()
            .border_color(theme.border)
            .bg(crate::theme::ink(0.02))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .min_w_0()
                    .text_size(px(11.0))
                    .text_color(theme.text_muted)
                    .children(language.map(|lang| SharedString::from(lang.to_string()))),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(2.0))
                    .children(actions),
            ),
    )
}

/// Shared visual shell for ordinary code and generated diagram fences.
fn code_block_frame(
    id: SharedString,
    language: Option<&str>,
    actions: Vec<AnyElement>,
    body: AnyElement,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .rounded(px(10.0))
        .bg(crate::theme::ink(0.035))
        .border_1()
        .border_color(theme.border)
        .overflow_hidden()
        .relative()
        .children(code_block_header(language, actions, theme))
        .child(body)
}

#[allow(clippy::too_many_arguments)]
fn render_code_block_source(
    language: Option<&str>,
    code: &str,
    top_ix: usize,
    ix: usize,
    opts: &RenderOptions,
    theme: &Theme,
    highlight: CodeHighlight,
) -> AnyElement {
    render_code_block_source_with_actions(
        language,
        code,
        top_ix,
        ix,
        opts,
        theme,
        highlight,
        Vec::new(),
    )
}

#[allow(clippy::too_many_arguments)]
fn render_code_block_source_with_actions(
    language: Option<&str>,
    code: &str,
    top_ix: usize,
    ix: usize,
    opts: &RenderOptions,
    theme: &Theme,
    highlight: CodeHighlight,
    mut extra_actions: Vec<AnyElement>,
) -> AnyElement {
    let mono = font(theme.font_mono.clone());
    // Per-line strings + runs through the cross-frame cache (validity: code
    // length + highlight slice identity — a fresh highlight Arc re-derives).
    let hl_key = highlight.map_or((0, 0), |h| (h.as_ptr() as usize, h.len()));
    let build = || {
        let lines: Vec<(SharedString, Vec<TextRun>)> = code
            .split('\n')
            .enumerate()
            .map(|(li, line)| {
                let spans = highlight
                    .and_then(|h| h.get(li))
                    .map(|t| &t[..])
                    .unwrap_or(&[]);
                (
                    SharedString::from(line.to_string()),
                    runs_for_syntax_line(line, spans, &mono, theme),
                )
            })
            .collect();
        Rc::new(CachedCode {
            code_len: code.len(),
            hl_key,
            lines,
        })
    };
    let cached: Rc<CachedCode> = match &opts.cache {
        Some(cache) => {
            let mut cache = cache.borrow_mut();
            cache.sync_style();
            let entry = cache
                .code
                .entry((opts.row_key.clone(), top_ix, ix))
                .or_insert_with(&build);
            if entry.code_len != code.len() || entry.hl_key != hl_key {
                *entry = build();
            }
            entry.clone()
        }
        None => build(),
    };
    // Streaming veil over appended code, tracked on the whole code text and
    // sliced per line below (paint-only run recolor — heights stay exact).
    let veil_spans = match &opts.veil {
        Some(veil) => veil.borrow_mut().advance(ix, code, opts.now),
        None => Vec::new(),
    };
    let scroll_id: SharedString = format!("{}-code{ix}", opts.row_key).into();
    let sel_wash = selection_wash(theme);
    let code_ui = opts.code.as_ref().and_then(|code| code.get(&ix)).cloned();
    let fit_content = code_ui.as_ref().is_some_and(|ui| ui.fit_content);

    let fit_button = code_ui.as_ref().map(|ui| {
        let toggle = ui.toggle_fit.clone();
        let fade_key = format!("{}-fit{ix}", opts.row_key);
        let base = if fit_content {
            crate::theme::ink(0.09)
        } else {
            gpui::transparent_black()
        };
        let hover = crate::theme::ink(if fit_content { 0.13 } else { 0.08 });
        div()
            .id(SharedString::from(fade_key.clone()))
            .size(px(CODE_ACTION_SIZE))
            .rounded(px(6.0))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .bg(crate::motion::hover_blend(&fade_key, base, hover))
            .on_hover(crate::motion::hover_listener(fade_key))
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                toggle(window, cx);
            })
            .tooltip(move |_, cx| {
                cx.new(move |_| {
                    CodeBlockTooltip(if fit_content {
                        "Use horizontal scrolling"
                    } else {
                        "Fit content"
                    })
                })
                .into()
            })
            .child(
                crate::icons::icon(crate::icons::WRAP_TEXT)
                    .size(px(13.0))
                    .text_color(theme.text_muted),
            )
    });
    let mut actions = Vec::new();
    actions.extend(fit_button.map(IntoElement::into_any_element));
    actions.append(&mut extra_actions);
    actions.extend(code_copy_button(code, ix, opts, theme));

    let lines = div()
        .map(|el| {
            if fit_content {
                el.w_full().min_w_0()
            } else {
                el.min_w_full().flex_none()
            }
        })
        .px(px(CODE_PADDING_X))
        .py(px(CODE_PADDING_Y))
        .font_family(theme.font_mono.clone())
        .text_size(px(theme.code_font_size))
        .line_height(px(theme.code_font_size * CODE_LINE_HEIGHT_RATIO))
        .map(|el| {
            if fit_content {
                el.whitespace_normal()
            } else {
                el.whitespace_nowrap()
            }
        })
        .flex()
        .flex_col()
        .children((0..cached.lines.len()).scan(0usize, move |off, li| {
            let (line, runs) = &cached.lines[li];
            let start = *off;
            *off = start + line.len() + 1; // +1 for the '\n'
            let local = slice_spans(&veil_spans, start, start + line.len());
            let runs = apply_veil(runs.clone(), &local);
            let key = code_line_selection_key(&opts.row_key, ix, li);
            Some(
                div()
                    .map(|el| {
                        if fit_content {
                            el.w_full()
                                .min_w_0()
                                .min_h(px(theme.code_font_size * CODE_LINE_HEIGHT_RATIO))
                        } else {
                            el.h(px(theme.code_font_size * CODE_LINE_HEIGHT_RATIO))
                                .flex_none()
                        }
                    })
                    .child(selectable_text_element(key, line.clone(), runs, sel_wash)),
            )
        }));

    let body: AnyElement = if let Some(ui) = code_ui.as_ref() {
        if fit_content {
            div()
                .w_full()
                .min_w_0()
                .overflow_hidden()
                .child(lines)
                .into_any_element()
        } else {
            let mut scroller = div()
                .id(scroll_id)
                .w_full()
                .min_w_0()
                .flex()
                .overflow_x_scroll()
                .track_scroll(&ui.scroll)
                .child(lines);
            // A vertical wheel over code must keep bubbling to the transcript;
            // only a true horizontal gesture moves this local viewport.
            scroller.style().restrict_scroll_to_axis = Some(true);
            scroller.into_any_element()
        }
    } else {
        // Non-transcript previews keep their existing native scroll behavior.
        div()
            .id(scroll_id)
            .w_full()
            .overflow_x_scroll()
            .child(lines)
            .into_any_element()
    };

    let scrollbar = (!fit_content)
        .then(|| code_ui.as_ref())
        .flatten()
        .and_then(|ui| {
            let bar = ui.scrollbar.as_ref()?;
            let metrics = bar.metrics;
            let hover = bar.hover.clone();
            let press = bar.press.clone();
            let release_up = bar.release.clone();
            let release_out = bar.release.clone();
            let thumb_height = if bar.active {
                crate::popover::MENU_SCROLLBAR_HOVER_THUMB_WIDTH
            } else {
                crate::popover::MENU_SCROLLBAR_THUMB_WIDTH
            };
            Some(
                div()
                    .id(SharedString::from(format!("{}-scrollbar", ui.key)))
                    .absolute()
                    .left(px(0.0))
                    .right(px(0.0))
                    .bottom(px(0.0))
                    .h(px(CODE_SCROLLBAR_HIT_HEIGHT))
                    .on_hover(move |hovered, window, cx| hover(*hovered, window, cx))
                    .on_mouse_down(gpui::MouseButton::Left, move |event, window, cx| {
                        press(event.position.x, window, cx);
                    })
                    .on_drag(
                        CodeScrollbarDrag {
                            key: ui.key.clone(),
                        },
                        |_, _, _, cx| {
                            cx.stop_propagation();
                            cx.new(|_| CodeScrollbarDragGhost)
                        },
                    )
                    .on_mouse_up(gpui::MouseButton::Left, move |_, window, cx| {
                        release_up(window, cx)
                    })
                    .on_mouse_up_out(gpui::MouseButton::Left, move |_, window, cx| {
                        release_out(window, cx)
                    })
                    .child(
                        div()
                            .absolute()
                            .left(px(
                                crate::popover::MENU_SCROLLBAR_TRACK_INSET + metrics.thumb_left
                            ))
                            .bottom(px(2.0))
                            .w(px(metrics.thumb_width))
                            .h(px(thumb_height))
                            .rounded(px(thumb_height / 2.0))
                            .bg(theme
                                .text_faint
                                .opacity(if bar.active { 0.68 } else { 0.5 })),
                    ),
            )
        });

    let frame_body = div().w_full().relative().child(body).children(scrollbar);
    let mut block = code_block_frame(
        format!("{}-code-frame{ix}", opts.row_key).into(),
        language,
        actions,
        frame_body.into_any_element(),
        theme,
    );
    if let Some(ui) = code_ui {
        let viewport_hover = ui.viewport_hover.clone();
        let drag_move = ui.drag_move.clone();
        let drag_key = ui.key;
        block = block
            .on_hover(move |hovered, window, cx| viewport_hover(*hovered, window, cx))
            .on_drag_move(
                move |event: &gpui::DragMoveEvent<CodeScrollbarDrag>, window, cx| {
                    let Some(drag) = event.dragged_item().downcast_ref::<CodeScrollbarDrag>()
                    else {
                        return;
                    };
                    if drag.key == drag_key {
                        drag_move(event.event.position.x, window, cx);
                    }
                },
            );
    }
    block.into_any_element()
}
