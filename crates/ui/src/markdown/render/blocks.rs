//! Markdown block-tree rendering and nested block indexing.

use super::*;

/// Render a whole tree stacked with the md block gap. `highlight` resolves
/// tokens for a top-level block index (code blocks only).
pub fn render_tree(
    tree: &BlockTree,
    opts: &RenderOptions,
    theme: &Theme,
    window: &Window,
    highlight: &dyn Fn(usize) -> Option<std::sync::Arc<HighlightedDocument>>,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(MD_BLOCK_GAP))
        .children(tree.blocks.iter().enumerate().map(|(ix, top)| {
            let document = highlight(ix);
            render_block(
                &top.block,
                ix,
                ix,
                opts,
                theme,
                window,
                document
                    .as_deref()
                    .map(|document| document.lines.as_slice()),
            )
        }))
        .into_any_element()
}

fn quote_child_ix(ix: usize, child_ix: usize) -> usize {
    ix * 100 + child_ix
}

fn list_child_ix(ix: usize, item_ix: usize, child_ix: usize) -> usize {
    ix * 100 + item_ix * 10 + child_ix
}

/// Element discriminators for every code block below `block`. Transcript rows
/// use this to provision one independent scroll handle per nested fence before
/// the renderer recursively reaches it.
pub fn code_block_indices(block: &Block, ix: usize) -> Vec<usize> {
    fn collect(block: &Block, ix: usize, out: &mut Vec<usize>) {
        match block {
            Block::CodeBlock { .. } => out.push(ix),
            Block::BlockQuote { children } => {
                for (child_ix, child) in children.iter().enumerate() {
                    collect(child, quote_child_ix(ix, child_ix), out);
                }
            }
            Block::List { items, .. } => {
                for (item_ix, item) in items.iter().enumerate() {
                    for (child_ix, child) in item.iter().enumerate() {
                        collect(child, list_child_ix(ix, item_ix, child_ix), out);
                    }
                }
            }
            Block::Paragraph { .. } | Block::Heading { .. } | Block::Table { .. } | Block::Rule => {
            }
        }
    }

    let mut indices = Vec::new();
    collect(block, ix, &mut indices);
    indices
}

/// Render one block (top-level or nested). `top_ix` is the enclosing top-level
/// block index (cache invalidation scope); `ix` the per-element discriminator.
#[allow(clippy::too_many_arguments)]
pub fn render_block(
    block: &Block,
    top_ix: usize,
    ix: usize,
    opts: &RenderOptions,
    theme: &Theme,
    window: &Window,
    highlight: CodeHighlight,
) -> AnyElement {
    match block {
        Block::Paragraph { runs } => text_element(
            runs,
            MD_TEXT_SIZE,
            MD_LINE_HEIGHT,
            false,
            top_ix,
            ix,
            opts,
            theme,
        ),
        Block::Heading { level, runs } => {
            let (size, line) = heading_metrics(*level);
            text_element(runs, size, line, true, top_ix, ix, opts, theme)
        }
        Block::CodeBlock { language, code } => render_code_block(
            language.as_deref(),
            code,
            top_ix,
            ix,
            opts,
            theme,
            highlight,
        ),
        Block::BlockQuote { children } => div()
            // Accent-tinted quote: indigo rail + a whisper of the same hue
            // behind it (the inline-code treatment, dialed down).
            .border_l_2()
            .border_color(theme.accent.opacity(0.6))
            .bg(theme.accent.opacity(0.05))
            .rounded_tr(px(6.0))
            .rounded_br(px(6.0))
            .pl(px(12.0))
            .pr(px(10.0))
            .py(px(6.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .text_color(theme.text_muted)
            .children(children.iter().enumerate().map(|(ci, child)| {
                render_block(
                    child,
                    top_ix,
                    quote_child_ix(ix, ci),
                    opts,
                    theme,
                    window,
                    None,
                )
            }))
            .into_any_element(),
        Block::List {
            ordered_start,
            items,
        } => div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .children(items.iter().enumerate().map(|(item_ix, item)| {
                // Accent markers (the inline-code hue): ordered numbers as
                // tinted text, unordered as a REAL 5px disc — the glyph "•"
                // reads too small at 14px.
                let task = item
                    .first()
                    .and_then(|block| match block {
                        Block::Paragraph { runs } => runs.first()?.style.task.as_ref(),
                        _ => None,
                    })
                    .filter(|_| opts.tasks.is_some());
                let marker: gpui::AnyElement = if let Some(task) = task {
                    let toggle = opts.tasks.as_ref().and_then(|ui| ui.toggle.clone());
                    let marker = task.clone();
                    let label: String = match &item[0] {
                        Block::Paragraph { runs } => {
                            runs.iter().skip(1).map(|r| r.text.as_str()).collect()
                        }
                        _ => String::new(),
                    };
                    div()
                        .flex_none()
                        .min_w(px(18.0))
                        .h(px(MD_LINE_HEIGHT))
                        .flex()
                        .items_center()
                        .child(
                            gpui_base::Checkbox::new(SharedString::from(format!(
                                "{}-task-{}",
                                opts.row_key, task.range.start
                            )))
                            .checked(task.checked)
                            .disabled(toggle.is_none())
                            .when(toggle.is_some(), |checkbox| checkbox.cursor_pointer())
                            .focus_visible(|style| style.border_color(theme.text))
                            .styles(|styles| styles.disabled(|style| style.opacity(0.5)))
                            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation()
                            })
                            .accessibility_label(label)
                            .size(px(16.0))
                            .border_1()
                            .rounded(px(3.0))
                            .border_color(if task.checked {
                                theme.accent
                            } else {
                                theme.border
                            })
                            .bg(if task.checked {
                                theme.accent
                            } else {
                                gpui::transparent_black()
                            })
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(task.checked, |checkbox| {
                                checkbox.child(
                                    crate::icons::icon(crate::icons::CHECK)
                                        .size(px(12.0))
                                        .text_color(theme.bg),
                                )
                            })
                            .on_change(move |_, _, window, cx| {
                                cx.stop_propagation();
                                if let Some(toggle) = &toggle {
                                    toggle(&marker, window, cx);
                                }
                            }),
                        )
                        .into_any_element()
                } else {
                    match ordered_start {
                        Some(start) => div()
                            .flex_none()
                            .min_w(px(18.0))
                            .text_size(crate::typography::ui_rems(MD_TEXT_SIZE))
                            .line_height(crate::typography::ui_rems(MD_LINE_HEIGHT))
                            .text_color(theme.accent)
                            .child(SharedString::from(format!("{}.", start + item_ix as u64)))
                            .into_any_element(),
                        None => div()
                            .flex_none()
                            .min_w(px(18.0))
                            // Center the disc on the first text line's cap band.
                            .h(px(MD_LINE_HEIGHT))
                            .flex()
                            .items_center()
                            .child(
                                div()
                                    .ml(px(1.0))
                                    .w(px(5.0))
                                    .h(px(5.0))
                                    .rounded_full()
                                    .bg(theme.accent),
                            )
                            .into_any_element(),
                    }
                };
                div().flex().flex_row().gap(px(8.0)).child(marker).child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .children(item.iter().enumerate().map(|(ci, child)| {
                            let task_body;
                            let child = if ci == 0 && task.is_some() {
                                let Block::Paragraph { runs } = child else {
                                    unreachable!()
                                };
                                task_body = Block::Paragraph {
                                    runs: runs.iter().skip(1).cloned().collect(),
                                };
                                &task_body
                            } else {
                                child
                            };
                            render_block(
                                child,
                                top_ix,
                                list_child_ix(ix, item_ix, ci),
                                opts,
                                theme,
                                window,
                                None,
                            )
                        })),
                )
            }))
            .into_any_element(),
        Block::Table {
            header,
            rows,
            align,
        } => render_table(header, rows, align, top_ix, ix, opts, theme, window),
        Block::Rule => div()
            .h(px(1.0))
            .w_full()
            .bg(theme.border)
            .into_any_element(),
    }
}
