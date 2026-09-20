//! Markdown table geometry and rendering.

use super::*;

/// Shared per-column table geometry (port of mugen `tableColumns`).
pub struct TableColumns {
    /// Per-column max-content width, padding included.
    pub naturals: Vec<f32>,
    /// Per-column minimum width, padding included = `min(natural, minColumnWidth)`.
    pub minimums: Vec<f32>,
    /// Σ minimums — the width below which the table stops shrinking and scrolls.
    pub min_table_width: f32,
}

/// Resolve column geometry from measured per-column max-content widths
/// (content only — padding is added here, as the source adds `2 * cellPadding`).
pub fn table_columns(content_widths: &[f32]) -> TableColumns {
    let naturals: Vec<f32> = content_widths
        .iter()
        .map(|w| w.max(TABLE_MIN_COLUMN_CONTENT) + 2.0 * TABLE_CELL_PADDING)
        .collect();
    let minimums: Vec<f32> = naturals
        .iter()
        .map(|n| n.min(TABLE_MIN_COLUMN_WIDTH))
        .collect();
    let min_table_width = minimums.iter().sum();
    TableColumns {
        naturals,
        minimums,
        min_table_width,
    }
}

/// Element/cache discriminator for a table cell (row-major under the block ix).
fn table_cell_ix(ix: usize, r: usize, c: usize) -> usize {
    ix * 100_000 + r * 100 + c
}

/// A GFM table — a port of mugen-markdown's `TableBlock` under zeron's md
/// theme (see the `TABLE_*` constants).
///
/// Column widths resolve exactly the way the source's CSS does: each cell is
/// `flex: <max-content> <max-content> 0; min-width: min(max-content, 96px)`,
/// so widths are content-proportional with a readable per-column floor.
/// Naturals come from shaping each cell's runs unwrapped (gpui's line-layout
/// cache makes repeat frames cheap); the flex resolution itself is Taffy's —
/// the same algorithm as the web's. When even the floors no longer fit, the
/// rows overflow the viewport and the table scrolls horizontally instead of
/// crushing every column into per-character wrapping.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_table(
    header: &[Vec<InlineRun>],
    rows: &[Vec<Vec<InlineRun>>],
    align: &[TableAlign],
    top_ix: usize,
    ix: usize,
    opts: &RenderOptions,
    theme: &Theme,
    window: &Window,
) -> AnyElement {
    // Header row first, mirroring the source's `rows` shape (rows may be ragged).
    let all: Vec<&[Vec<InlineRun>]> = std::iter::once(header)
        .filter(|h| !h.is_empty())
        .map(|h| h as &[Vec<InlineRun>])
        .chain(rows.iter().map(|r| r.as_slice()))
        .collect();
    let cols = all.iter().map(|r| r.len()).max().unwrap_or(0);
    if cols == 0 {
        return gpui::Empty.into_any_element();
    }
    let has_header = !header.is_empty();

    // Flatten every cell (cache-aware) and take per-column max-content widths.
    let text_system = window.text_system();
    let mut flats: Vec<Vec<Option<Rc<FlatText>>>> = Vec::with_capacity(all.len());
    let mut content = vec![0.0f32; cols];
    for (r, row) in all.iter().enumerate() {
        let weight = if has_header && r == 0 {
            TABLE_HEADER_WEIGHT
        } else {
            FontWeight::NORMAL
        };
        let mut out: Vec<Option<Rc<FlatText>>> = Vec::with_capacity(cols);
        for (c, natural) in content.iter_mut().enumerate() {
            let Some(runs) = row.get(c) else {
                out.push(None);
                continue;
            };
            let flat = flatten_cached(runs, weight, top_ix, table_cell_ix(ix, r, c), opts, theme);
            if !flat.text.is_empty() {
                // Intrinsic table proportions use the same bounded link presentation;
                // each cell then resolves its exact width during measured layout.
                let measured = opts
                    .link
                    .as_ref()
                    .filter(|ui| ui.source_session.is_some())
                    .map(|_| {
                        crate::markdown::link_presentation::present(
                            &flat,
                            px(560.),
                            px(MD_TEXT_SIZE),
                            window,
                        )
                    });
                let flat = measured.as_ref().unwrap_or(&flat);
                // Cell sources are single-line; guard anyway (same byte count,
                // so the runs still cover the text exactly).
                let line: SharedString = if flat.text.contains('\n') {
                    flat.text.replace('\n', " ").into()
                } else {
                    flat.text.clone()
                };
                let width = f32::from(
                    text_system
                        .shape_line(line, px(MD_TEXT_SIZE), &flat.runs, None)
                        .width(),
                );
                if width > *natural {
                    *natural = width;
                }
            }
            out.push(Some(flat));
        }
        flats.push(out);
    }
    let geo = table_columns(&content);

    // Frameless flat-hairline chrome: 1px rules under the header and between
    // rows are the only paint (`table.gap` = 1, borderColor white@10%); the
    // theme's headerBackground is transparent and its radius 0, so there is no
    // header fill, outer box, or rounding.
    let hairline = table_hairline();
    let mut inner = div()
        .flex()
        .flex_col()
        .w_full()
        .min_w(px(geo.min_table_width));
    for (r, row) in flats.iter().enumerate() {
        if r > 0 {
            inner = inner.child(div().flex_none().h(px(TABLE_DIVIDER)).w_full().bg(hairline));
        }
        let mut row_el = div().flex().flex_row();
        for (c, cell_flat) in row.iter().enumerate() {
            let mut cell = div()
                .flex_grow(geo.naturals[c])
                .flex_shrink(geo.naturals[c])
                .flex_basis(px(0.0))
                .min_w(px(geo.minimums[c]))
                .p(px(TABLE_CELL_PADDING))
                .text_size(crate::typography::ui_rems(MD_TEXT_SIZE))
                .line_height(crate::typography::ui_rems(MD_LINE_HEIGHT));
            cell = match align.get(c).copied().unwrap_or_default() {
                TableAlign::Left => cell,
                TableAlign::Center => cell.text_center(),
                TableAlign::Right => cell.text_right(),
            };
            if opts.media.is_some()
                && all[r]
                    .get(c)
                    .is_some_and(|runs| runs.iter().any(|run| run.style.image.is_some()))
            {
                cell = cell.child(text_element(
                    &all[r][c],
                    MD_TEXT_SIZE,
                    MD_LINE_HEIGHT,
                    has_header && r == 0,
                    top_ix,
                    table_cell_ix(ix, r, c),
                    opts,
                    theme,
                ));
            } else if let Some(flat) = cell_flat {
                cell = cell.child(flat_text_element(
                    flat,
                    table_cell_ix(ix, r, c),
                    opts,
                    theme,
                ));
            }
            row_el = row_el.child(cell);
        }
        inner = inner.child(row_el);
    }

    // The horizontal scroller — when the floors exceed the viewport the inner
    // block keeps `min_table_width` and this viewport scrolls it.
    let scroll_id: SharedString = format!("{}-table{ix}", opts.row_key).into();
    div()
        .id(scroll_id)
        .w_full()
        .overflow_x_scroll()
        .child(inner)
        .into_any_element()
}
