//! BlockTree → gpui elements.
//!
//! Numbers drive layout (font sizes, line heights, paddings — all constants
//! here); colors are paint. Code blocks render per-line so their height is
//! exactly `lines × line_height`, and syntax highlighting arrives later as
//! recolored `TextRun`s on the identical mono font — layout never changes
//! (mugen's "highlight is pure paint"). Streaming fade-in is a per-appended-
//! chunk opacity veil over the text runs (see [`super::veil`]) — opacity only,
//! zero translate, applied after layout-relevant properties are fixed.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use std::time::Instant;

use gpui::{
    AnyElement, BorderStyle, Bounds, Context, FontStyle, FontWeight, Hsla, Render, SharedString,
    StyledText, TextRun, UnderlineStyle, Window, canvas, div, font, point, prelude::*, px, quad,
    size,
};
use zeron_syntax::{HighlightKind, HighlightSpan, HighlightedDocument};

use crate::theme::Theme;

use super::parser::{Block, BlockTree, InlineRun, TableAlign};
use super::veil::{RowVeil, apply_veil, slice_spans};

/// Gap between markdown blocks inside one message (zeron mdBlockGap).
pub const MD_BLOCK_GAP: f32 = 12.0;
/// Body text size / line height (zeron: 14px / 22px).
pub const MD_TEXT_SIZE: f32 = 14.0;
pub const MD_LINE_HEIGHT: f32 = 22.0;
/// Default code block metrics; the rendered size comes from the theme.
pub const CODE_TEXT_SIZE: f32 = 12.5;
pub const CODE_LINE_HEIGHT: f32 = 18.0;
/// Line height as a multiple of the code size, so a user-chosen size keeps the
/// default's row rhythm.
const CODE_LINE_HEIGHT_RATIO: f32 = CODE_LINE_HEIGHT / CODE_TEXT_SIZE;
pub const CODE_PADDING_X: f32 = 12.0;
pub const CODE_PADDING_Y: f32 = 10.0;
const CODE_HEADER_HEIGHT: f32 = 28.0;
const CODE_ACTION_SIZE: f32 = 22.0;
const CODE_SCROLLBAR_HIT_HEIGHT: f32 = 10.0;

// Table metrics — a port of mugen-markdown 0.6.2's `TableBlock` under zeron's
// resolved md theme. The design is frameless ("flat hairline"): 1px horizontal
// rules under the header and between rows are the only chrome — no outer box,
// no header fill, no corner radius (theme: headerBackground transparent,
// radius 0). Cells use the body scale (14/22) with a uniform 12px padding;
// the header row is weight-700 per `table.headerWeight`.
/// Uniform cell padding in px (zeron `table.cellPadding`).
pub const TABLE_CELL_PADDING: f32 = 12.0;
/// Hairline between rows in px (zeron `table.gap`).
pub const TABLE_DIVIDER: f32 = 1.0;
/// Header row font weight (zeron `table.headerWeight` = 700).
pub const TABLE_HEADER_WEIGHT: FontWeight = FontWeight::BOLD;
/// Floor for a column's max-content share, so a short column ("1k") beside a
/// prose column keeps a readable width (mugen `MIN_COLUMN_CONTENT`).
pub const TABLE_MIN_COLUMN_CONTENT: f32 = 48.0;
/// Minimum rendered column width in px, padding included (zeron
/// `table.minColumnWidth`). Naturally narrower columns keep their content
/// width; wider ones wrap down to this floor, then the table scrolls.
pub const TABLE_MIN_COLUMN_WIDTH: f32 = 96.0;
/// Hairline tone (zeron md theme `table.borderColor`: rgba(255,255,255,0.1)).
pub fn table_hairline() -> Hsla {
    crate::theme::hairline(0.10)
}

/// Options for one rendered tree (a transcript row or a whole live message).
#[derive(Clone)]
pub struct RenderOptions {
    pub tasks: Option<TaskUi>,
    pub media: Option<MediaUi>,
    /// Stable row key — prefixes element ids (scroll state, animations).
    pub row_key: SharedString,
    /// Streaming veil state for a live row: newly appended text fades in via
    /// paint-only run opacity, keyed per (element, chunk offset) so each chunk
    /// fades exactly once. `None` renders without fades (completed rows).
    pub veil: Option<Rc<RefCell<RowVeil>>>,
    /// Flatten/shape input cache (see [`RenderCache`]): settled blocks reuse
    /// their flat text + runs across frames instead of rebuilding them — the
    /// per-frame cost of a fading live row stays O(tail block), flat in the
    /// total reply length. `None` rebuilds every pass.
    pub cache: Option<Rc<RefCell<RenderCache>>>,
    /// Frame timestamp driving veil opacities (one clock per render pass).
    pub now: Instant,
    /// Code-block copy-button plumbing (round 9): `None` renders no button
    /// (previews outside the transcript).
    pub copy: Option<CopyUi>,
    /// Optional owner-provided routing for links that belong inside the app.
    /// Routing is explicit: rejected links never reach the external opener.
    pub link: Option<LinkUi>,
    /// Root used to distinguish a direct workspace-file link from an ordinary
    /// web link. Transcript surfaces provide it; generic Markdown previews do
    /// not acquire workspace-specific decoration.
    pub workspace_root: Option<SharedString>,
    /// Agent-transcript-only fence layout controls and tracked horizontal
    /// scroll state, keyed by the same element discriminator passed to
    /// [`render_block`]. `None` keeps non-chat Markdown surfaces unchanged.
    pub code: Option<HashMap<usize, CodeUi>>,
}

#[derive(Clone)]
pub struct TaskUi {
    pub toggle: Option<Rc<dyn Fn(&super::parser::TaskMarker, &mut Window, &mut gpui::App)>>,
}

#[derive(Clone)]
pub struct MediaUi {
    pub diagram: Option<Rc<dyn Fn(&str, SharedString, &Theme) -> DiagramUi>>,
    pub image: Rc<dyn Fn(&super::parser::InlineImage, SharedString, &Theme) -> AnyElement>,
}

pub struct DiagramUi {
    pub body: AnyElement,
    pub show_source: bool,
    pub toggle_source: Rc<dyn Fn(&mut Window, &mut gpui::App)>,
}

/// Copy-button wiring for one row's code blocks: the handler writes the code
/// to the clipboard and flips a transient per-row "Copied" state owned by the
/// transcript entity; `copied_ix` is the block currently showing feedback.
#[derive(Clone)]
pub struct CopyUi {
    pub handler: Rc<dyn Fn(usize, SharedString, &mut Window, &mut gpui::App)>,
    pub copied_ix: Option<usize>,
}

pub use super::links::{LinkAction, LinkActivation, LinkOutcome, LinkTarget};

#[derive(Clone)]
pub struct LinkUi {
    pub source_session: Option<String>,
    pub handler: Rc<dyn Fn(&LinkActivation, &mut Window, &mut gpui::App) -> LinkOutcome>,
}

pub fn activate_link(
    target: LinkTarget,
    action: LinkAction,
    ui: Option<&LinkUi>,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    if action == LinkAction::Copy {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(target.original.clone()));
        return;
    }
    let activation = LinkActivation {
        target,
        action,
        source_session: ui.and_then(|ui| ui.source_session.clone()),
    };
    let outcome = ui.map_or_else(
        || activation.web_outcome(false),
        |ui| (ui.handler)(&activation, window, cx),
    );
    if let LinkOutcome::External(url) = outcome {
        cx.open_url(&url);
    }
}

#[path = "code.rs"]
mod code;
pub use code::{CodeFenceRuntime, CodeScrollbarUi, CodeUi, code_ui_for};
pub(super) use code::{CodeHighlight, render_code_block};

impl RenderOptions {
    /// Options for a completed (non-streaming) row — no veil, no cache.
    pub fn settled(row_key: SharedString) -> Self {
        Self {
            row_key,
            media: None,
            tasks: None,
            veil: None,
            cache: None,
            now: Instant::now(),
            copy: None,
            link: None,
            workspace_root: None,
            code: None,
        }
    }
}

/// Cross-frame cache of flatten results, keyed by
/// `(row key, top-level block ix, element discriminator)`.
///
/// During a streaming fade the live row re-renders every frame; without the
/// cache each frame re-derives every block's flat `String` + `TextRun`s —
/// O(reply length) per frame, growing through long replies. The incremental
/// parser only ever touches a suffix of the top-level blocks
/// ([`super::parser::IncrementalParser::stable_prefix_blocks`]), so everything
/// below that boundary is byte-identical and its flatten result (and, via
/// gpui's line-layout cache keyed on identical text+runs, its shaping) can be
/// reused as-is. `SharedString`/`Rc` make the reuse O(1) per block.
/// Cached runs carry a resolved [`gpui::Hsla`] per span, so an entry is only
/// valid for the palette that produced it — content-only keys silently serve
/// dark-mode text onto a light background after an appearance switch.
/// [`RenderCache::sync_style`] drops everything when color or typography moves.
#[derive(Default)]
pub struct RenderCache {
    flats: HashMap<(SharedString, usize, usize), Rc<FlatText>>,
    code: HashMap<(SharedString, usize, usize), Rc<CachedCode>>,
    /// The [`crate::theme::style_generation`] these entries were shaped under.
    generation: u32,
}

/// Cached per-line code runs (validity: code length + highlight identity).
pub struct CachedCode {
    code_len: usize,
    /// Slice-pointer identity + len of the highlight Arc that produced this.
    hl_key: (usize, usize),
    lines: Vec<(SharedString, Vec<TextRun>)>,
}

impl RenderCache {
    /// Keep rows laid out in the previous viewport, including GPUI overdraw.
    /// Scrolling back regenerates these derived strings and code-line runs.
    pub(crate) fn retain_rows(&mut self, rows: &std::collections::HashSet<SharedString>) {
        self.flats.retain(|(row, _, _), _| rows.contains(row));
        self.code.retain(|(row, _, _), _| rows.contains(row));
    }

    /// Drop every cached entry for `row`.
    pub fn invalidate_row(&mut self, row: &str) {
        self.flats.retain(|(r, _, _), _| r.as_ref() != row);
        self.code.retain(|(r, _, _), _| r.as_ref() != row);
    }

    pub fn clear(&mut self) {
        self.flats.clear();
        self.code.clear();
    }

    /// Drop every entry if the resolved text style changed since shaping. Cheap
    /// enough (one relaxed atomic load) to call on every cache access.
    fn sync_style(&mut self) {
        let generation = crate::theme::style_generation();
        self.sync_generation(generation);
    }

    fn sync_generation(&mut self, generation: u32) {
        if self.generation != generation {
            self.clear();
            self.generation = generation;
        }
    }
}

#[path = "render/blocks.rs"]
mod blocks;
pub use blocks::{code_block_indices, render_block, render_tree};

/// Tight monochrome heading scale (zeron: h2 ≈ 16px semibold; headings step
/// down quickly toward body size).
fn heading_metrics(level: u8) -> (f32, f32) {
    match level {
        1 => (19.0, 27.0),
        2 => (16.0, 24.0),
        3 => (15.0, 22.0),
        _ => (14.0, 22.0),
    }
}

#[path = "table.rs"]
mod table;
pub(crate) use table::render_table;
pub use table::{TableColumns, table_columns};
#[path = "render/text.rs"]
mod text;
use text::flatten_cached;
pub use text::{FlatText, flatten_runs};
pub(super) use text::{flat_text_element, flat_text_presented_element};

/// Selection tint shared with native inputs and the composer.
fn selection_wash(theme: &Theme) -> Hsla {
    theme.selection
}

/// Selection support for a plain (non-markdown) text element — the user
/// bubble. Paints the selection wash under the glyphs, registers the element
/// into the frame's document-ordered registry (so drags span into adjacent
/// markdown rows and Cmd+C joins in order), and re-registers the mouse
/// listeners. Call from a paint-phase canvas that sits UNDER the text.
pub(crate) fn paint_text_selection(
    window: &mut Window,
    key: &std::sync::Arc<str>,
    text: &SharedString,
    layout: &gpui::TextLayout,
    theme: &Theme,
) {
    paint_text_selection_with_wash(window, key, text, layout, selection_wash(theme));
}

fn paint_text_selection_with_wash(
    window: &mut Window,
    key: &std::sync::Arc<str>,
    text: &SharedString,
    layout: &gpui::TextLayout,
    wash: Hsla,
) {
    if let Some(range) = super::selection::wash_range(key) {
        for rect in range_rects(layout, &range, 0.0, 0.0) {
            window.paint_quad(quad(
                rect,
                px(0.0),
                wash,
                px(0.0),
                gpui::transparent_black(),
                BorderStyle::default(),
            ));
        }
    }
    REGISTRY.with(|r| {
        r.borrow_mut().push(RegEntry {
            key: key.clone(),
            text: text.clone(),
            layout: layout.clone(),
            offsets: None,
        })
    });
    register_selection_listeners(window, key, text, layout, None);
}

fn selectable_text_element(
    key: std::sync::Arc<str>,
    text: SharedString,
    runs: Vec<TextRun>,
    wash: Hsla,
) -> AnyElement {
    let styled = StyledText::new(text.clone()).with_runs(runs);
    let layout = styled.layout().clone();
    let underlay = canvas(
        |_, _, _| (),
        move |_, _, window, _| {
            paint_text_selection_with_wash(window, &key, &text, &layout, wash);
        },
    )
    .absolute()
    .size_full();
    div()
        .relative()
        .child(underlay)
        .child(styled)
        .into_any_element()
}

fn code_line_selection_key(row_key: &str, code_ix: usize, line_ix: usize) -> std::sync::Arc<str> {
    format!("{row_key}-code{code_ix}-line{line_ix}").into()
}

/// One painted text element, registered per frame in document order — the
/// continuity model that lets a drag span paragraphs/list items (Zed gets
/// this for free from its single-element markdown; our tree rebuilds it).
struct RegEntry {
    key: std::sync::Arc<str>,
    text: SharedString,
    layout: gpui::TextLayout,
    offsets: Option<super::link_presentation::OffsetMap>,
}

thread_local! {
    static REGISTRY: RefCell<Vec<RegEntry>> = const { RefCell::new(Vec::new()) };
}

#[cfg(test)]
pub(crate) fn selection_test_bounds(key: &str) -> gpui::Bounds<gpui::Pixels> {
    REGISTRY.with(|r| {
        r.borrow()
            .iter()
            .find(|entry| entry.key.as_ref() == key)
            .expect("text must be registered")
            .layout
            .bounds()
    })
}

#[cfg(test)]
pub(super) fn selection_test_snapshot(
    key: &str,
) -> (
    SharedString,
    gpui::TextLayout,
    Option<super::link_presentation::OffsetMap>,
) {
    REGISTRY.with(|registry| {
        let registry = registry.borrow();
        let entry = registry
            .iter()
            .find(|entry| entry.key.as_ref() == key)
            .expect("painted text");
        (
            entry.text.clone(),
            entry.layout.clone(),
            entry.offsets.clone(),
        )
    })
}

/// A zero-size canvas that clears the selection registry — paint it FIRST in
/// the transcript root (before any markdown), so each frame's registry holds
/// exactly that frame's visible text elements in paint order.
pub fn selection_frame_reset() -> impl IntoElement {
    canvas(
        |_, _, _| (),
        |_, _, _, _| {
            REGISTRY.with(|r| {
                r.borrow_mut()
                    .retain(|e| !selection_scope(&e.key).is_empty())
            })
        },
    )
    .absolute()
    .w(px(0.0))
    .h(px(0.0))
}

fn selection_scope(key: &str) -> &str {
    if key.starts_with("md-preview-") {
        key.split_once('|').map_or("", |(scope, _)| scope)
    } else {
        ""
    }
}

pub(crate) fn clear_selection_surface(prefix: &str) {
    REGISTRY.with(|r| {
        r.borrow_mut()
            .retain(|entry| !entry.key.starts_with(prefix))
    });
    if let Some(anchor) = super::selection::anchor_key().filter(|key| key.starts_with(prefix)) {
        super::selection::clear_if_owner(&anchor);
    }
}

pub fn selection_surface_reset(prefix: String) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |_, _, _, _| REGISTRY.with(|r| r.borrow_mut().retain(|e| !e.key.starts_with(&prefix))),
    )
    .absolute()
    .w(px(0.0))
    .h(px(0.0))
}

/// `(element index, byte offset)` for a window position: the registered
/// element whose vertical band contains it, else the nearest by vertical
/// distance (a drag past the gutter or between blocks clamps sensibly).
fn registry_point(position: gpui::Point<gpui::Pixels>) -> Option<(usize, usize)> {
    REGISTRY.with(|r| {
        let reg = r.borrow();
        let anchor = super::selection::anchor_key().unwrap_or_default();
        let mut best: Option<(usize, f32)> = None;
        for (ei, entry) in reg.iter().enumerate() {
            if selection_scope(&entry.key) != selection_scope(&anchor) {
                continue;
            }
            let b = entry.layout.bounds();
            let dy = if position.y < b.top() {
                f32::from(b.top() - position.y)
            } else if position.y > b.bottom() {
                f32::from(position.y - b.bottom())
            } else {
                0.0
            };
            if best.is_none_or(|(_, d)| dy < d) {
                best = Some((ei, dy));
            }
            if dy == 0.0 {
                break;
            }
        }
        let (ei, _) = best?;
        let ix = match reg[ei].layout.index_for_position(position) {
            Ok(ix) | Err(ix) => ix,
        };
        Some((
            ei,
            reg[ei].offsets.as_ref().map_or(ix, |map| map.original(ix)),
        ))
    })
}

/// Resolve the drag head into document-ordered spans over the frame's registry
/// and store them; true if the selection changed. The selection model retains
/// spans across overlapping virtualized frames once its anchor scrolls away.
fn resolve_drag(head: (usize, usize)) -> bool {
    REGISTRY.with(|r| {
        let reg = r.borrow();
        let Some(entry) = reg.get(head.0) else {
            return false;
        };
        let scope = selection_scope(&entry.key);
        let filtered: Vec<_> = reg
            .iter()
            .enumerate()
            .filter(|(_, e)| selection_scope(&e.key) == scope)
            .collect();
        let Some(index) = filtered.iter().position(|(ix, _)| *ix == head.0) else {
            return false;
        };
        let elements: Vec<_> = filtered
            .iter()
            .map(|(_, e)| (e.key.as_ref(), e.text.as_ref()))
            .collect();
        super::selection::update_drag(&elements, (index, head.1))
    })
}

/// Continue the active drag at a window position. The transcript's edge-scroll
/// driver calls this between scroll steps, so a stationary pointer keeps
/// extending through newly painted rows.
pub(crate) fn update_drag_at(position: gpui::Point<gpui::Pixels>) -> bool {
    let Some(head) = registry_point(position) else {
        return false;
    };
    resolve_drag(head)
}

/// Register this frame's window-level mouse listeners for one text element's
/// selection (Zed-markdown mechanics: window-level so a drag keeps tracking
/// outside the element's bounds; frame-scoped, so paint re-registers).
fn register_selection_listeners(
    window: &mut Window,
    key: &std::sync::Arc<str>,
    text: &SharedString,
    layout: &gpui::TextLayout,
    offsets: Option<super::link_presentation::OffsetMap>,
) {
    use gpui::{DispatchPhase, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent};
    {
        let (key, text, layout) = (key.clone(), text.clone(), layout.clone());
        window.on_mouse_event(move |e: &MouseDownEvent, phase, window, _cx| {
            if phase != DispatchPhase::Bubble || e.button != MouseButton::Left {
                return;
            }
            if layout.bounds().contains(&e.position) {
                let ix = match layout.index_for_position(e.position) {
                    Ok(ix) | Err(ix) => ix,
                };
                let ix = offsets.as_ref().map_or(ix, |map| map.original(ix));
                match e.click_count {
                    2 => {
                        let range = super::selection::word_range(&text, ix);
                        super::selection::begin_with_span(&key, &text, range);
                    }
                    n if n >= 3 => {
                        super::selection::begin_with_span(&key, &text, 0..text.len());
                    }
                    _ => super::selection::begin(&key, ix),
                }
                window.refresh();
            } else if super::selection::clear_if_owner(&key) {
                window.refresh();
            }
        });
    }
    {
        let key = key.clone();
        window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, _cx| {
            if phase != DispatchPhase::Bubble || !e.dragging() {
                return;
            }
            // Only the anchor element's listener drives the drag.
            if super::selection::drag_anchor(&key).is_none() {
                return;
            }
            if update_drag_at(e.position) {
                window.refresh();
            }
        });
    }
    {
        let key = key.clone();
        window.on_mouse_event(move |_: &MouseUpEvent, phase, _window, _cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            if let Some(_text) = super::selection::end_drag(&key) {
                // X11 middle-click paste parity (Zed does the same).
                #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                _cx.write_to_primary(gpui::ClipboardItem::new_string(_text));
            }
        });
    }
}

/// The wash boxes for one byte range: one box per visual line the range
/// covers (soft wraps split it), in window coordinates from the laid-out
/// text's own geometry. `pad_x` overhangs the box horizontally (inline code);
/// `inset_y` shrinks it vertically — both 0 for a selection wash, which wants
/// full-line-height boxes that tile seamlessly across wrapped rows.
pub(crate) fn range_rects(
    layout: &gpui::TextLayout,
    range: &Range<usize>,
    pad_x: f32,
    inset_y: f32,
) -> Vec<Bounds<gpui::Pixels>> {
    range_rects_with_positions(
        layout.bounds(),
        layout.line_height(),
        range,
        pad_x,
        inset_y,
        |ix| layout.position_for_index(ix),
    )
}

fn range_rects_with_positions(
    bounds: Bounds<gpui::Pixels>,
    line_height: gpui::Pixels,
    range: &Range<usize>,
    pad_x: f32,
    inset_y: f32,
    position_for_index: impl Fn(usize) -> Option<gpui::Point<gpui::Pixels>>,
) -> Vec<Bounds<gpui::Pixels>> {
    let mut rects = Vec::new();
    let mut cur = range.start;
    // Walk the range one visual row at a time: find the furthest index that
    // still sits on the current row (binary search over glyph positions).
    let mut guard = 0;
    while cur < range.end && guard < 256 {
        guard += 1;
        let Some(mut p1) = position_for_index(cur) else {
            break;
        };
        // GPUI gives a soft-wrap boundary upstream affinity: the boundary
        // index is reported at the end of the preceding visual row. When the
        // following byte advances to a lower row, `cur` is also the start of
        // that row; use its downstream position for the range start.
        if let Some(after) = position_for_index(cur.saturating_add(1))
            && after.y > p1.y
        {
            p1 = point(bounds.left(), after.y);
        }
        // `seg_end` closes the wash on this row; `next` is the first index on
        // the following row. A soft-wrap boundary belongs to both rows, so it
        // closes this rectangle with upstream affinity and starts the next
        // rectangle with the downstream correction above.
        let (seg_end, next) = match position_for_index(range.end) {
            Some(pe) if pe.y == p1.y => (range.end, range.end),
            _ => {
                // Largest ix on this row (probes stay on char boundaries only
                // at the ends; intermediate probes just need a y).
                let (mut lo, mut hi) = (cur, range.end);
                while hi - lo > 1 {
                    let mid = lo + (hi - lo) / 2;
                    match position_for_index(mid) {
                        Some(pm) if pm.y == p1.y => lo = mid,
                        _ => hi = mid,
                    }
                }
                (lo, lo)
            }
        };
        if let Some(p2) = position_for_index(seg_end)
            && p2.x > p1.x
        {
            rects.push(Bounds::new(
                point(p1.x - px(pad_x), p1.y + px(inset_y)),
                size(
                    p2.x - p1.x + px(2.0 * pad_x),
                    line_height - px(2.0 * inset_y),
                ),
            ));
        }
        if next <= cur {
            break;
        }
        cur = next;
    }
    rects
}

#[allow(clippy::too_many_arguments)]
fn text_element(
    runs: &[InlineRun],
    size: f32,
    line_height: f32,
    bold_default: bool,
    top_ix: usize,
    ix: usize,
    opts: &RenderOptions,
    theme: &Theme,
) -> AnyElement {
    if let Some(media) = &opts.media {
        if runs.iter().any(|run| run.style.image.is_some()) {
            let mut elements = Vec::new();
            let mut start = 0;
            for (index, run) in runs.iter().enumerate() {
                if let Some(image) = &run.style.image {
                    if start < index {
                        elements.push(text_element(
                            &runs[start..index],
                            size,
                            line_height,
                            bold_default,
                            top_ix,
                            ix.wrapping_mul(4099).wrapping_add(start + 1000),
                            opts,
                            theme,
                        ));
                    }
                    elements.push((media.image)(
                        image,
                        format!("{}-image-{ix}-{index}", opts.row_key).into(),
                        theme,
                    ));
                    start = index + 1;
                }
            }
            if start < runs.len() {
                elements.push(text_element(
                    &runs[start..],
                    size,
                    line_height,
                    bold_default,
                    top_ix,
                    ix.wrapping_mul(4099).wrapping_add(start + 1000),
                    opts,
                    theme,
                ));
            }
            return div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .children(elements)
                .into_any_element();
        }
    }
    if let Some(lines) = opts
        .workspace_root
        .as_deref()
        .and_then(|root| plain_file_reference_lines(runs, root))
    {
        let style = runs[0].style.clone();
        return div()
            .flex()
            .flex_col()
            .children(lines.into_iter().enumerate().map(|(line_ix, text)| {
                text_element(
                    &[InlineRun {
                        text,
                        style: style.clone(),
                    }],
                    size,
                    line_height,
                    bold_default,
                    top_ix,
                    ix.wrapping_mul(4099).wrapping_add(line_ix + 2000),
                    opts,
                    theme,
                )
            }))
            .into_any_element();
    }
    let weight = if bold_default {
        FontWeight::SEMIBOLD
    } else {
        FontWeight::NORMAL
    };
    let flat = flatten_cached(runs, weight, top_ix, ix, opts, theme);
    let inner = flat_text_element(&flat, ix, opts, theme);
    let direct_file = opts
        .workspace_root
        .as_deref()
        .and_then(|root| sole_file_reference(runs, root));
    let content = if let Some(path) = direct_file {
        div()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(4.0))
            .child(
                div()
                    .size(px(20.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .bg(crate::file_icons::well_bg(theme))
                    .child(
                        crate::file_icons::icon(
                            crate::file_icons::FileIconIdentity::file(&path),
                            theme.appearance,
                        )
                        .size(px(14.0)),
                    ),
            )
            .child(div().min_w_0().flex_1().child(inner))
            .into_any_element()
    } else {
        inner
    };
    div()
        .text_size(crate::typography::ui_rems(size))
        .line_height(crate::typography::ui_rems(line_height))
        .child(content)
        .into_any_element()
}

/// A file icon belongs beside a link only when the whole visible paragraph is
/// one safe workspace-file target. Mixed prose and external links retain the
/// ordinary inline Markdown layout.
fn sole_workspace_file_link(runs: &[InlineRun], workspace_root: &str) -> Option<String> {
    let mut target = None;
    for run in runs.iter().filter(|run| !run.text.is_empty()) {
        if run.style.image.is_some() || run.style.task.is_some() {
            return None;
        }
        let link = run.style.link.as_deref()?;
        if link == super::mend::PENDING_LINK_URL {
            return None;
        }
        match target {
            Some(previous) if previous != link => return None,
            None => target = Some(link),
            _ => {}
        }
    }
    crate::workspace_links::resolve_workspace_file_link(target?, workspace_root)
        .map(|link| link.path)
}

/// A model sometimes emits a bare filename after being asked for a file list
/// instead of preserving the workspace link it would normally author. Treat a
/// whole paragraph as a decorative file identity only when the token resolves
/// safely within the workspace and the icon theme recognizes it. The stricter
/// mapping check keeps version numbers, domains, and unknown dotted prose from
/// acquiring file affordances; unlike a real link, this stays non-interactive.
fn sole_plain_file_reference(runs: &[InlineRun], workspace_root: &str) -> Option<String> {
    let mut text = String::new();
    for run in runs.iter().filter(|run| !run.text.is_empty()) {
        if run.style.link.is_some()
            || run.style.image.is_some()
            || run.style.task.is_some()
            || run.style.code
        {
            return None;
        }
        text.push_str(&run.text);
    }
    let candidate = text.trim();
    if candidate.is_empty() || candidate.chars().any(char::is_whitespace) {
        return None;
    }
    let path = crate::workspace_links::resolve_workspace_file_link(candidate, workspace_root)?.path;
    crate::file_icons::has_specific_file_icon(&path).then_some(path)
}

/// Preserve hard-break file lists as one compact paragraph while giving each
/// line its own icon. Limiting this path to one uniformly styled run avoids
/// rewriting mixed inline formatting or ordinary wrapped prose.
fn plain_file_reference_lines(runs: &[InlineRun], workspace_root: &str) -> Option<Vec<String>> {
    let [run] = runs else { return None };
    if run.style.link.is_some()
        || run.style.image.is_some()
        || run.style.task.is_some()
        || run.style.code
        || !run.text.contains('\n')
    {
        return None;
    }
    let lines = run
        .text
        .lines()
        .map(str::trim)
        .map(|candidate| {
            if candidate.is_empty() || candidate.chars().any(char::is_whitespace) {
                return None;
            }
            let path =
                crate::workspace_links::resolve_workspace_file_link(candidate, workspace_root)?
                    .path;
            crate::file_icons::has_specific_file_icon(&path).then(|| candidate.to_owned())
        })
        .collect::<Option<Vec<_>>>()?;
    (lines.len() > 1).then_some(lines)
}

fn sole_file_reference(runs: &[InlineRun], workspace_root: &str) -> Option<String> {
    sole_workspace_file_link(runs, workspace_root)
        .or_else(|| sole_plain_file_reference(runs, workspace_root))
}

/// Paint color for a token class — the soft syntax palette (round 9: the
/// original's mdTheme code blocks are monochrome `#e7e7e7`, but the user
/// asked for color; these are the diff pane's hues, now shared by both).
pub fn token_color(kind: HighlightKind, theme: &Theme) -> Hsla {
    theme.syntax.color(kind)
}

/// Build the exact-cover `TextRun` list for one code line from its tokens.
/// Same font everywhere — recoloring can never change layout.
/// Build paint-only runs from the neutral Tree-sitter contract.
pub fn runs_for_syntax_line(
    line: &str,
    spans: &[HighlightSpan],
    mono: &gpui::Font,
    theme: &Theme,
) -> Vec<TextRun> {
    runs_for_syntax_line_with_plain(line, spans, mono, theme.text, theme)
}

pub fn runs_for_syntax_line_with_plain(
    line: &str,
    spans: &[HighlightSpan],
    mono: &gpui::Font,
    plain_color: Hsla,
    theme: &Theme,
) -> Vec<TextRun> {
    let plain = |len: usize| TextRun {
        len,
        font: mono.clone(),
        color: plain_color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let mut runs = Vec::new();
    let mut at = 0usize;
    for span in spans {
        if span.range.start > at {
            runs.push(plain(span.range.start - at));
        }
        let mut run = plain(span.range.len());
        run.color = token_color(span.kind, theme);
        runs.push(run);
        at = span.range.end;
    }
    if at < line.len() {
        runs.push(plain(line.len() - at));
    }
    runs.retain(|run| run.len > 0);
    runs
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
/// Native fixture access to actual shaped link ranges; absent in shipped builds.
#[cfg(feature = "browser-fixture")]
pub fn fixture_link(target: &str) -> Option<(gpui::Point<gpui::Pixels>, gpui::FocusHandle)> {
    super::link_interaction::fixture_link(target)
}
