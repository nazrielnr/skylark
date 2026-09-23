//! Markdown inline run flattening, caching, and text presentation.

use super::*;

/// Flattened inline runs: one string + gpui `TextRun`s + clickable link ranges
/// + inline-code ranges (their rounded washes are painted by a canvas UNDER
/// the text — `TextRun::background_color` can only paint square boxes).
/// `text` is a `SharedString` so cached reuse across frames is an Arc clone.
#[derive(Clone)]
pub struct FlatText {
    pub original: Option<crate::markdown::link_presentation::OriginalText>,
    pub text: SharedString,
    pub runs: Vec<TextRun>,
    pub links: Vec<(Range<usize>, String)>,
    pub code_ranges: Vec<Range<usize>>,
}

/// Inline-code tint: a text-safe use of the selected accent identity.
pub fn inline_code_text(theme: &Theme) -> Hsla {
    theme.code_text
}
pub fn inline_code_wash(theme: &Theme) -> Hsla {
    theme.code_wash
}
/// Rounded-wash geometry: small radius on a slightly inset box (paint-only —
/// x extends 2px past the glyphs, y insets 2px from the 22px line box).
pub const INLINE_CODE_RADIUS: f32 = 4.5;
pub const INLINE_CODE_PAD_X: f32 = 2.0;
pub const INLINE_CODE_INSET_Y: f32 = 2.0;

/// Flatten inline runs into shaped-text inputs. Pure given a theme.
pub fn flatten_runs(runs: &[InlineRun], theme: &Theme, bold_default: bool) -> FlatText {
    flatten_runs_weighted(
        runs,
        theme,
        if bold_default {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::NORMAL
        },
    )
}

/// [`flatten_runs`] with an explicit base weight (table headers are 700 per
/// skylark's `table.headerWeight`; strong runs never drop below semibold).
fn flatten_runs_weighted(runs: &[InlineRun], theme: &Theme, base_weight: FontWeight) -> FlatText {
    let mut text = String::new();
    let mut out: Vec<TextRun> = Vec::with_capacity(runs.len());
    let mut links: Vec<(Range<usize>, String)> = Vec::new();
    let mut code_ranges: Vec<Range<usize>> = Vec::new();
    for run in runs {
        if run.text.is_empty() {
            continue;
        }
        let start = text.len();
        text.push_str(&run.text);
        let mut f = if run.style.code {
            font(theme.font_mono.clone())
        } else {
            font(theme.font_sans.clone())
        };
        f.weight = if run.style.bold && base_weight.0 < FontWeight::SEMIBOLD.0 {
            FontWeight::SEMIBOLD
        } else {
            base_weight
        };
        f.style = if run.style.italic {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        };
        // Links stay monochrome — foreground with an underline (skylark's md
        // theme underlines in the text color; indigo is reserved for primary
        // actions).
        let is_link = run.style.link.is_some();
        // Inline code uses the spectrum's code tone; everything else
        // stays the monochrome foreground.
        let color = if run.style.code {
            inline_code_text(theme)
        } else {
            theme.text
        };
        if run.style.code {
            // Merge adjacent code runs into one wash box (like links below).
            match code_ranges.last_mut() {
                Some(range) if range.end == start => range.end = text.len(),
                _ => code_ranges.push(start..text.len()),
            }
        }
        if let Some(url) = &run.style.link {
            // A still-streaming link (mend.rs sentinel) keeps link styling —
            // so the URL's completion changes nothing visually — but is not
            // clickable until the real destination exists.
            if url != crate::markdown::mend::PENDING_LINK_URL {
                // Merge adjacent runs of the same link into one clickable range.
                match links.last_mut() {
                    Some((range, last_url)) if range.end == start && last_url == url => {
                        range.end = text.len();
                    }
                    _ => links.push((start..text.len(), url.clone())),
                }
            }
        }
        out.push(TextRun {
            len: run.text.len(),
            font: f,
            color,
            // Inline code's wash is painted as ROUNDED quads by the canvas
            // underlay (`code_wash_underlay`) — a run background here could
            // only be a square box.
            background_color: None,
            underline: is_link.then_some(UnderlineStyle {
                color: Some(theme.text_muted),
                thickness: px(1.0),
                wavy: false,
            }),
            strikethrough: run.style.strikethrough.then_some(gpui::StrikethroughStyle {
                thickness: px(1.0),
                color: Some(theme.text_muted),
            }),
        });
    }
    FlatText {
        original: None,
        text: text.into(),
        runs: out,
        links,
        code_ranges,
    }
}

/// Flatten through the cross-frame cache when one is wired: settled blocks
/// reuse text + runs untouched (O(1) per block per frame); only blocks the
/// incremental parser invalidated rebuild.
pub(crate) fn flatten_cached(
    runs: &[InlineRun],
    base_weight: FontWeight,
    top_ix: usize,
    ix: usize,
    opts: &RenderOptions,
    theme: &Theme,
) -> Rc<FlatText> {
    match &opts.cache {
        Some(cache) => {
            let mut cache = cache.borrow_mut();
            cache.sync_style();
            cache
                .flats
                .entry((opts.row_key.clone(), top_ix, ix))
                .or_insert_with(|| Rc::new(flatten_runs_weighted(runs, theme, base_weight)))
                .clone()
        }
        None => Rc::new(flatten_runs_weighted(runs, theme, base_weight)),
    }
}

/// Veiled, clickable text for a flattened block (no sizing wrapper).
pub(crate) fn flat_text_element(
    flat: &Rc<FlatText>,
    ix: usize,
    opts: &RenderOptions,
    theme: &Theme,
) -> AnyElement {
    if opts
        .link
        .as_ref()
        .is_some_and(|ui| ui.source_session.is_some())
        && !flat.links.is_empty()
    {
        return crate::markdown::link_presentation::ResponsiveText {
            flat: flat.clone(),
            ix,
            opts: opts.clone(),
            theme: theme.clone(),
        }
        .into_any_element();
    }
    flat_text_presented_element(flat, ix, opts, theme)
}

pub(crate) fn flat_text_presented_element(
    flat: &FlatText,
    ix: usize,
    opts: &RenderOptions,
    theme: &Theme,
) -> AnyElement {
    // Streaming veil: opacity-only recolor of the runs covering newly appended
    // chunks. Same text, same fonts, same lengths — layout is untouched.
    // Settled elements return no spans and reuse the cached runs unsplit.
    let text_runs = match &opts.veil {
        Some(veil) => {
            let original = flat
                .original
                .as_ref()
                .map_or(&flat.text, |original| &original.text);
            let spans = veil.borrow_mut().advance(ix, original, opts.now);
            let spans = if let Some(original) = &flat.original {
                spans
                    .into_iter()
                    .filter_map(|(range, opacity)| {
                        let range = original.offsets.displayed(range.start)
                            ..original.offsets.displayed(range.end);
                        (!range.is_empty()).then_some((range, opacity))
                    })
                    .collect()
            } else {
                spans
            };
            apply_veil(flat.runs.clone(), &spans)
        }
        None => flat.runs.clone(),
    };
    let styled = StyledText::new(flat.text.clone()).with_runs(text_runs);
    let layout = styled.layout().clone();
    let text_el = styled.into_any_element();
    let link_layout = layout.clone();
    // Underlay canvas: inline-code washes + the selection wash, painted
    // BEFORE the text (earlier sibling ⇒ underneath), reading glyph geometry
    // from the text's own layout handle. Pure paint — never in layout. The
    // same paint pass re-registers the frame-scoped window mouse listeners
    // that drive text selection (round 18; see markdown/selection.rs).
    let sel_key: std::sync::Arc<str> = format!("{}:{ix}", opts.row_key).into();
    let code_ranges = flat.code_ranges.clone();
    let flat_text = flat
        .original
        .as_ref()
        .map_or_else(|| flat.text.clone(), |original| original.text.clone());
    let offsets = flat
        .original
        .as_ref()
        .map(|original| original.offsets.clone());
    let wash = inline_code_wash(theme);
    let sel_wash = selection_wash(theme);
    let underlay = canvas(
        |_, _, _| (),
        move |_, _, window, _| {
            for range in &code_ranges {
                for rect in range_rects(&layout, range, INLINE_CODE_PAD_X, INLINE_CODE_INSET_Y) {
                    window.paint_quad(quad(
                        rect,
                        px(INLINE_CODE_RADIUS),
                        wash,
                        px(0.0),
                        gpui::transparent_black(),
                        BorderStyle::default(),
                    ));
                }
            }
            if let Some(range) = crate::markdown::selection::wash_range(&sel_key) {
                let range = offsets
                    .as_ref()
                    .map_or_else(|| range.clone(), |map| map.displayed_range(range.clone()));
                for rect in range_rects(&layout, &range, 0.0, 0.0) {
                    window.paint_quad(quad(
                        rect,
                        px(0.0),
                        sel_wash,
                        px(0.0),
                        gpui::transparent_black(),
                        BorderStyle::default(),
                    ));
                }
            }
            // Register this element into the frame's document-ordered
            // registry (paint order IS document order), then the frame's
            // mouse listeners.
            REGISTRY.with(|r| {
                r.borrow_mut().push(RegEntry {
                    key: sel_key.clone(),
                    text: flat_text.clone(),
                    layout: layout.clone(),
                    offsets: offsets.clone(),
                })
            });
            register_selection_listeners(window, &sel_key, &flat_text, &layout, offsets.clone());
        },
    )
    .absolute()
    .size_full();
    let child = div()
        .relative()
        .child(underlay)
        .child(text_el)
        .into_any_element();
    if flat.links.is_empty() {
        return child;
    }
    crate::markdown::link_interaction::LinkRanges {
        id: format!(
            "{}-{}-t{ix}",
            opts.row_key,
            opts.link
                .as_ref()
                .and_then(|ui| ui.source_session.as_deref())
                .unwrap_or_default()
        )
        .into(),
        child,
        layout: link_layout,
        links: flat
            .links
            .iter()
            .map(|(range, url)| {
                (
                    range.clone(),
                    LinkTarget::new(
                        flat.original
                            .as_ref()
                            .map_or(&flat.text[range.clone()], |original| {
                                &original.text[original.offsets.original(range.start)
                                    ..original.offsets.original(range.end)]
                            }),
                        url,
                    ),
                )
            })
            .collect(),
        ui: opts.link.clone(),
    }
    .into_any_element()
}
