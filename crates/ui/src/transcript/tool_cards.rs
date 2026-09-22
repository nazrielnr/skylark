//! Tool execution cards, chips, activity rail connector tessellation, and folding.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Bounds, ContentMask, Context, DispatchPhase, PathBuilder, Pixels, Point,
    ScrollHandle, SharedString, StyledText, TextAlign, TextRun, Window, canvas, div, point,
    prelude::*, px, size,
};
use zeron_doc::SubagentStatus;
use zeron_proto::ToolCall;

use crate::markdown::parser::InlineRun;
use crate::motion;
use crate::notice::{NoticeChipIcon::Tile, notice_chip};
use crate::theme::Theme;
use crate::transcript::row::{
    DETAIL_SEPARATOR, OUTPUT_BODY_PAD, OUTPUT_DETAIL_MAX_LINES, OUTPUT_LINE_HEIGHT, ToolDetail,
    ToolItem, is_agent_call, is_agent_tool, is_spawn_link, tool_detail, tool_group_collapses,
};
use crate::transcript::{BlobFetch, FoldState, Transcript, TranscriptEvent};

pub use zeron_proto::view::{single_line, tool_chip_content};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const CHIP_HEIGHT: f32 = 38.0;
pub const CHIP_GAP: f32 = 0.0;
pub const CHIP_CARD_HEIGHT: f32 = 30.0;
pub const CHIP_HEADER_HEIGHT: f32 = CHIP_CARD_HEIGHT - 2.0;

pub const ACTIVITY_GUTTER_WIDTH: f32 = 48.0;
pub const ACTIVITY_TEXT_GAP: f32 = 8.0;
pub const ACTIVITY_TRUNK_X: f32 = 9.0;
pub const ACTIVITY_BEND_RADIUS: f32 = 6.0;
pub const ACTIVITY_BRANCH_END_X: f32 = 28.0;
pub const ACTIVITY_ICON_LEFT: f32 = 32.0;
pub const ACTIVITY_ICON_SIZE: f32 = 16.0;

pub const TOOL_TEXT_SIZE: f32 = 13.5;
pub const TOOL_LABEL_SIZE: f32 = TOOL_TEXT_SIZE;
pub const TOOL_LABEL_LINE_HEIGHT: f32 = 20.0;
pub const TOOL_GROUP_HEADER_HEIGHT: f32 = 28.0;
pub const TOOL_TREE_ROW_HEIGHT: f32 = 34.0;

pub const TOOL_FOLD: motion::MotionSpec = motion::MotionSpec::new(140, motion::EASE_OUT);

pub const TOOL_GROUP_SHIMMER_DURATION: Duration = Duration::from_millis(3_400);
pub const TOOL_GROUP_SHIMMER_HALF_WIDTH: f32 = 0.36;
pub const TOOL_GROUP_SHIMMER_STRIP_WIDTH: f32 = 2.0;

pub const TOOL_ROW_REVEAL: motion::MotionSpec = motion::MotionSpec::new(360, motion::EASE_OUT_EXPO);
pub const TOOL_CONNECTOR_REVEAL: motion::MotionSpec =
    motion::MotionSpec::new(480, motion::EASE_OUT_QUINT);

pub const TOOL_FIRST_ROW_DELAY_MS: u64 = 90;
pub const TOOL_ROW_STAGGER_MS: u64 = 65;

pub const CHIPS_TOP_PAD: f32 = 2.0;
pub const FOLD_TWEEN_WINDOW: Duration = Duration::from_millis(400);

pub const BLOB_AFFORDANCE_HEIGHT: f32 = 24.0;
pub const FULL_OUTPUT_MAX_LINES: usize = 400;
pub const SUBAGENT_TITLE_MAX: usize = 40;
const DETAIL_FADE_BAND: f32 = 28.0;
const DETAIL_SCROLLBAR_WIDTH: f32 = 10.0;

#[derive(Clone)]
pub(crate) struct DetailScrollbarDrag {
    id: SharedString,
}

pub(crate) struct DetailScrollbarDragGhost;

impl gpui::Render for DetailScrollbarDragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        gpui::Empty
    }
}

// ---------------------------------------------------------------------------
// Structs & Enums
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub(crate) struct ChipAffordance {
    pub(crate) blob_ref: SharedString,
    pub(crate) label: SharedString,
}

/// Reveal epochs for one live ordinary tool group. `None` means the row was
/// already present when this transcript attached (or has finished revealing),
/// so replaying history and scrolling a virtualized row back into view stay
/// completely still.
#[derive(Default)]
pub(crate) struct ToolGroupReveal {
    /// A newly streamed task header participates in the same height/fade/lift
    /// reveal as its steps. Replayed headers leave this unset.
    pub(crate) header_started_at: Option<Instant>,
    pub(crate) starts: Vec<Option<Instant>>,
    /// A title sweep begins with this group instead of inheriting the shared
    /// loader clock at an arbitrary point midway across the label.
    pub(crate) shimmer_started_at: Option<Instant>,
    pub(crate) rendered_open: Option<bool>,
    pub(crate) rendered_height: f32,
    pub(crate) settled_duration_secs: Option<u64>,
}

/// The trailing tile on a chip header, when it has one.
pub(crate) enum ChipTrail {
    /// Expand/collapse chevron — flipped while the detail body is open.
    Chevron { open: bool },
    /// Top-right "opens elsewhere" arrow — the spawn chip's link to its
    /// subagent tab.
    OpenArrow,
}

// ---------------------------------------------------------------------------
// Helper Functions
// ---------------------------------------------------------------------------

/// Analytic expanded-chips height — no measurement needed for the fold tween.
pub fn chips_height(count: usize) -> f32 {
    if count == 0 {
        return 0.0;
    }
    CHIPS_TOP_PAD + count as f32 * CHIP_HEIGHT + (count as f32 - 1.0) * CHIP_GAP
}

/// Analytic height an open detail adds to its chip's card (separator + body)
/// — output blocks by line count, diff blocks via the changes pane's own
/// [`crate::changes::body_height`]. The chip's own [`CHIP_HEIGHT`] is already
/// counted by [`chips_height`].
pub fn detail_height(detail: &ToolDetail) -> f32 {
    let body = match detail {
        ToolDetail::Output {
            lines,
            truncated_by,
        } => {
            let rows = lines.len() + usize::from(*truncated_by > 0);
            rows.min(OUTPUT_DETAIL_MAX_LINES) as f32 * OUTPUT_LINE_HEIGHT + OUTPUT_BODY_PAD
        }
        ToolDetail::Thought { lines, .. } => {
            lines.len().min(OUTPUT_DETAIL_MAX_LINES) as f32 * OUTPUT_LINE_HEIGHT + OUTPUT_BODY_PAD
        }
        ToolDetail::Diff { file, .. } => crate::changes::body_height(file),
        ToolDetail::Stats { stats } => stats.len() as f32 * OUTPUT_LINE_HEIGHT + OUTPUT_BODY_PAD,
    };
    DETAIL_SEPARATOR + body
}

/// Build the upgraded detail from a fetched sidecar blob. Diff blobs parse
/// the `ToolDiff` JSON through the same pipeline as inline diffs; output
/// blobs render (near-)uncapped — fetching past the summary was the point.
pub fn blob_detail(text: &str, is_diff: bool) -> Option<ToolDetail> {
    if is_diff {
        let diff: zeron_proto::ToolDiff = serde_json::from_str(text).ok()?;
        return tool_detail(None, Some(&diff), None);
    }
    let mut lines: Vec<SharedString> = text
        .lines()
        .map(|l| SharedString::from(l.to_owned()))
        .collect();
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    if lines.is_empty() {
        return None;
    }
    let truncated_by = lines.len().saturating_sub(FULL_OUTPUT_MAX_LINES);
    lines.truncate(FULL_OUTPUT_MAX_LINES);
    Some(ToolDetail::Output {
        lines,
        truncated_by,
    })
}

/// Compact byte size for the fetch affordance label ("812 B", "12 KB").
pub fn format_kb(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else {
        format!("{} KB", bytes.div_ceil(1024))
    }
}

fn detail_should_follow(max_scroll: f32, offset_y: f32) -> bool {
    max_scroll + offset_y <= 2.0
}

fn detail_scroll_to_pointer(
    scroll: &ScrollHandle,
    follow: &Cell<bool>,
    metrics: crate::popover::MenuScrollbarMetrics,
    pointer_y: Pixels,
) {
    let local =
        f32::from(pointer_y - scroll.bounds().top()) - crate::popover::MENU_SCROLLBAR_TRACK_INSET;
    let thumb_top = (local - metrics.thumb_height / 2.0).clamp(0.0, metrics.travel());
    let fraction = if metrics.travel() > 0.0 {
        thumb_top / metrics.travel()
    } else {
        0.0
    };
    let offset = scroll.offset();
    scroll.set_offset(Point::new(offset.x, px(-fraction * metrics.max_scroll)));
    follow.set(detail_should_follow(
        metrics.max_scroll,
        -fraction * metrics.max_scroll,
    ));
}

fn scrollable_detail(
    id: SharedString,
    scroll: Option<&ScrollHandle>,
    follow: Option<&Rc<Cell<bool>>>,
    row_count: usize,
    rows: impl IntoElement,
    theme: &Theme,
) -> AnyElement {
    let scroll = scroll.cloned().unwrap_or_default();
    let follow = follow.cloned().unwrap_or_else(|| Rc::new(Cell::new(false)));
    let viewport_height = OUTPUT_DETAIL_MAX_LINES as f32 * OUTPUT_LINE_HEIGHT + OUTPUT_BODY_PAD;
    let content_height = row_count as f32 * OUTPUT_LINE_HEIGHT + OUTPUT_BODY_PAD;
    let max_scroll = (content_height - viewport_height).max(0.0);
    if follow.get() {
        let offset = scroll.offset();
        scroll.set_offset(Point::new(offset.x, px(-max_scroll)));
    }
    let metrics = crate::popover::MenuScrollbarMetrics::from_parts(
        viewport_height,
        content_height,
        -f32::from(scroll.offset().y),
    );
    let viewport = div()
        .id(id.clone())
        .h(crate::typography::ui_rems(
            viewport_height.min(content_height),
        ))
        .max_h(crate::typography::ui_rems(viewport_height))
        .overflow_y_scroll()
        .track_scroll(&scroll)
        .child(rows);
    let faded = crate::edge_fade::edge_faded(DETAIL_FADE_BAND, true, true, viewport)
        .fade_overflow_y(&scroll);
    let rail = metrics.map(|metrics| {
        let press_scroll = scroll.clone();
        let press_follow = follow.clone();
        let drag_scroll = scroll.clone();
        let drag_follow = follow.clone();
        let drag_id = id.clone();
        div()
            .id(SharedString::from(format!("{id}-scrollbar")))
            .absolute()
            .top_0()
            .bottom_0()
            .right_0()
            .w(px(DETAIL_SCROLLBAR_WIDTH))
            .cursor_pointer()
            .role(gpui::Role::ScrollBar)
            .aria_label("Output scrollbar")
            .on_mouse_down(gpui::MouseButton::Left, move |event, window, cx| {
                detail_scroll_to_pointer(&press_scroll, &press_follow, metrics, event.position.y);
                cx.stop_propagation();
                window.refresh();
            })
            .on_drag(DetailScrollbarDrag { id: id.clone() }, |_, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| DetailScrollbarDragGhost)
            })
            .on_drag_move(
                move |event: &gpui::DragMoveEvent<DetailScrollbarDrag>, window, cx| {
                    let Some(drag) = event.dragged_item().downcast_ref::<DetailScrollbarDrag>()
                    else {
                        return;
                    };
                    if drag.id == drag_id {
                        detail_scroll_to_pointer(
                            &drag_scroll,
                            &drag_follow,
                            metrics,
                            event.event.position.y,
                        );
                        cx.stop_propagation();
                        window.refresh();
                    }
                },
            )
            .child(
                div()
                    .absolute()
                    .top(px(
                        crate::popover::MENU_SCROLLBAR_TRACK_INSET + metrics.thumb_top
                    ))
                    .right(px(2.0))
                    .w(px(crate::popover::MENU_SCROLLBAR_THUMB_WIDTH))
                    .h(px(metrics.thumb_height))
                    .rounded(px(crate::popover::MENU_SCROLLBAR_THUMB_WIDTH / 2.0))
                    .bg(theme.text_faint.opacity(0.62)),
            )
    });
    let wheel_scroll = scroll.clone();
    let wheel_follow = follow.clone();
    let wheel_capture = canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let scroll = wheel_scroll.clone();
            window.on_mouse_event(move |event: &gpui::ScrollWheelEvent, phase, window, cx| {
                if phase != DispatchPhase::Capture || !bounds.contains(&event.position) {
                    return;
                }
                let delta = event.delta.pixel_delta(px(20.0));
                let offset = scroll.offset();
                let max = scroll.max_offset();
                let next_y = (offset.y + delta.y).clamp(-max.y, px(0.0));
                scroll.set_offset(Point::new(offset.x, next_y));
                wheel_follow.set(detail_should_follow(f32::from(max.y), f32::from(next_y)));
                cx.stop_propagation();
                window.refresh();
            });
        },
    )
    .absolute()
    .inset_0();
    div()
        .relative()
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .child(faded)
        .children(rail)
        .child(wheel_capture)
        .into_any_element()
}

pub fn tool_row_reveal_progress(start: Option<Instant>, now: Instant, reduce_motion: bool) -> f32 {
    let Some(start) = start.filter(|_| !reduce_motion) else {
        return 1.0;
    };
    let elapsed = now.checked_duration_since(start).unwrap_or_default();
    let raw = elapsed.as_secs_f32() / TOOL_ROW_REVEAL.total().as_secs_f32();
    TOOL_ROW_REVEAL.curve.eval(raw)
}

pub fn tool_connector_reveal_progress(
    start: Option<Instant>,
    now: Instant,
    reduce_motion: bool,
) -> f32 {
    let Some(start) = start.filter(|_| !reduce_motion) else {
        return 1.0;
    };
    let elapsed = now.checked_duration_since(start).unwrap_or_default();
    let raw = elapsed.as_secs_f32() / TOOL_CONNECTOR_REVEAL.total().as_secs_f32();
    TOOL_CONNECTOR_REVEAL.curve.eval(raw)
}

/// Split one arrival into a continuous tree draw. For child rows, the previous
/// row grows its continuation to the boundary first, then this row draws its
/// incoming trunk and elbow. The branch slightly overlaps the end of the
/// incoming phase so there is no dead frame at the bend.
pub fn tool_connector_parts(progress: f32, has_predecessor: bool) -> (f32, f32) {
    let progress = progress.clamp(0.0, 1.0);
    let (incoming_start, incoming_end, branch_start) = if has_predecessor {
        (0.45, 0.72, 0.68)
    } else {
        (0.0, 0.62, 0.58)
    };
    let incoming = ((progress - incoming_start) / (incoming_end - incoming_start)).clamp(0.0, 1.0);
    let branch = ((progress - branch_start) / (1.0 - branch_start)).clamp(0.0, 1.0);
    (incoming, branch)
}

/// The outgoing trunk belongs visually to the row that is already present,
/// but its timing belongs to the next row's arrival.
pub fn tool_connector_continuation(next_progress: Option<f32>) -> f32 {
    next_progress
        .map(|progress| (progress / 0.45).clamp(0.0, 1.0))
        .unwrap_or(0.0)
}

/// Reveal by distance along the elbow and straight leg, so changing the branch
/// length does not introduce a speed jump at their junction.
pub fn activity_branch_points(progress: f32) -> Vec<Point<f32>> {
    let mut path: Vec<_> = (0..=24)
        .map(|step| {
            let t = step as f32 / 24.0;
            point(
                ACTIVITY_BEND_RADIUS * t * t,
                ACTIVITY_BEND_RADIUS * (2.0 * t - t * t),
            )
        })
        .collect();
    path.push(point(
        ACTIVITY_BRANCH_END_X - ACTIVITY_TRUNK_X,
        ACTIVITY_BEND_RADIUS,
    ));
    if progress >= 1.0 {
        return path;
    }
    let lengths: Vec<_> = path
        .windows(2)
        .map(|pair| (pair[1].x - pair[0].x).hypot(pair[1].y - pair[0].y))
        .collect();
    let mut remaining = lengths.iter().sum::<f32>() * progress.clamp(0.0, 1.0);
    let mut visible = vec![path[0]];
    for (pair, length) in path.windows(2).zip(lengths) {
        if remaining <= 0.0 {
            break;
        }
        let t = (remaining / length).min(1.0);
        visible.push(point(
            motion::lerp(pair[0].x, pair[1].x, t),
            motion::lerp(pair[0].y, pair[1].y, t),
        ));
        remaining -= length;
    }
    visible
}

pub(crate) fn tool_disclosure_progress(open: bool, fold: FoldState, now: Instant) -> f32 {
    let Some(start) = fold.disclosure_at else {
        return if open { 1.0 } else { 0.0 };
    };
    let raw = now
        .checked_duration_since(start)
        .unwrap_or_default()
        .as_secs_f32()
        / TOOL_FOLD.total().as_secs_f32();
    let progress = TOOL_FOLD.curve.eval(raw);
    if open { progress } else { 1.0 - progress }
}

/// BoardUI's measured recipe: a 300%-wide repeating gradient moves from 200%
/// to -100%. Its 38→50→62% highlight maps to a 36%-of-title shoulder around
/// each peak; adjacent copies sit three title-widths apart. Sampling this by
/// x-coordinate lets the paint clips reproduce the continuous pattern.
pub fn tool_title_shimmer_amount(x: f32, phase: f32) -> f32 {
    let primary_center = -2.5 + phase.clamp(0.0, 1.0) * 6.0;
    (-2..=2)
        .map(|copy| primary_center + copy as f32 * 3.0)
        .map(|center| (1.0 - (x - center).abs() / TOOL_GROUP_SHIMMER_HALF_WIDTH).clamp(0.0, 1.0))
        .fold(0.0, f32::max)
}

pub fn tool_title_shimmer_phase(start: Instant, now: Instant) -> f32 {
    let elapsed = now.checked_duration_since(start).unwrap_or_default();
    (elapsed.as_secs_f32() / TOOL_GROUP_SHIMMER_DURATION.as_secs_f32()).fract()
}

pub fn tool_group_title(
    text: SharedString,
    shimmer_phase: Option<f32>,
    theme: &Theme,
) -> AnyElement {
    let Some(shimmer_phase) = shimmer_phase else {
        return text.into_any_element();
    };
    let overlay_text = text.clone();
    let overlay_font = gpui::font(theme.font_sans_fixed.clone());
    let base = theme.text_muted;
    let peak = theme.text;
    let overlay = canvas(
        move |bounds, window, _| {
            let probe = window.text_system().shape_line(
                overlay_text.clone(),
                px(TOOL_LABEL_SIZE),
                &[TextRun {
                    len: overlay_text.len(),
                    font: overlay_font.clone(),
                    color: peak,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }],
                None,
            );
            let text_width = f32::from(probe.width()).min(f32::from(bounds.size.width));
            let strip_count = (text_width / TOOL_GROUP_SHIMMER_STRIP_WIDTH).ceil() as usize;
            let mut strips = Vec::with_capacity(strip_count);
            for ix in 0..strip_count {
                let left = ix as f32 * TOOL_GROUP_SHIMMER_STRIP_WIDTH;
                let right = ((ix + 1) as f32 * TOOL_GROUP_SHIMMER_STRIP_WIDTH).min(text_width);
                let x = (left + right) * 0.5 / text_width.max(1.0);
                let amount = tool_title_shimmer_amount(x, shimmer_phase);
                if amount <= 0.001 {
                    continue;
                }
                let line = window.text_system().shape_line(
                    overlay_text.clone(),
                    px(TOOL_LABEL_SIZE),
                    &[TextRun {
                        len: overlay_text.len(),
                        font: overlay_font.clone(),
                        color: motion::mix(base, peak, amount),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }],
                    None,
                );
                strips.push((left, right, line));
            }
            strips
        },
        move |bounds, strips, window, cx| {
            for (left, right, line) in strips {
                let mask = ContentMask {
                    bounds: Bounds {
                        origin: point(bounds.origin.x + px(left), bounds.origin.y),
                        size: size(px(right - left), bounds.size.height),
                    },
                };
                window.with_content_mask(Some(mask), |window| {
                    let line_height = px(TOOL_LABEL_LINE_HEIGHT);
                    let _ = line.paint(
                        bounds.origin,
                        line_height,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                });
            }
        },
    )
    .absolute()
    .inset_0();
    div()
        .relative()
        .h_full()
        .min_w_0()
        .flex_1()
        .overflow_hidden()
        .child(text)
        .child(overlay)
        .into_any_element()
}

pub fn error_chip(message: SharedString, theme: &Theme) -> AnyElement {
    div()
        .py(px(4.0))
        .w_full()
        .child(
            notice_chip(theme, false, "Error", message, Tile)
                .overflow_hidden()
                .w_full(),
        )
        .into_any_element()
}

pub fn input_chip(header: SharedString, resolved: bool, theme: &Theme) -> AnyElement {
    let value: SharedString = if resolved {
        header
    } else {
        "Awaiting your answer…".into()
    };
    div()
        .py(px(4.0))
        .w_full()
        .child(
            div()
                .h(px(34.0))
                .w_full()
                .flex()
                .items_center()
                .gap(px(8.0))
                .overflow_hidden()
                .rounded(px(10.0))
                .border_1()
                .border_color(crate::theme::hairline(0.08))
                .bg(crate::theme::ink(0.045))
                .px(crate::typography::ui_rems(8.0))
                .text_size(crate::typography::ui_rems(13.0))
                .child(
                    div()
                        .flex_none()
                        .size(px(20.0))
                        .rounded(px(6.0))
                        .bg(crate::theme::ink(0.09))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            crate::icons::icon(crate::icons::CHAT_ROUND_LINE)
                                .size(px(12.0))
                                .text_color(theme.text_muted),
                        ),
                )
                .child(
                    div()
                        .flex_none()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(theme.text_muted)
                        .child(SharedString::from("Question")),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .text_color(theme.text.opacity(0.9))
                        .child(value),
                ),
        )
        .into_any_element()
}

pub fn tool_icon_path(call: &ToolCall) -> &'static str {
    match call {
        ToolCall::Exec { .. } => crate::icons::TERMINAL,
        ToolCall::ReadFile { .. } | ToolCall::ApplyPatch { .. } => crate::icons::DOCUMENT,
        ToolCall::WriteFile { .. } => crate::icons::DOCUMENT_ADD,
        ToolCall::EditFile { .. } => crate::icons::PEN,
        ToolCall::Search { .. } => crate::icons::MAGNIFER,
        ToolCall::Glob { .. } => crate::icons::FOLDER_WITH_FILES,
        ToolCall::WebFetch { .. } | ToolCall::WebSearch { .. } => crate::icons::GLOBAL,
        ToolCall::Todo { .. } => crate::icons::CHECKLIST,
        call if is_agent_call(call) => crate::icons::BOT,
        ToolCall::Unknown { name, .. } if name == "Wait for agents" => crate::icons::BOT,
        ToolCall::Mcp { .. } | ToolCall::Unknown { .. } => crate::icons::WIDGET,
    }
}

pub fn file_badge_name(path: &str) -> &str {
    path.rsplit(['/', '\\'])
        .find(|component| !component.is_empty())
        .unwrap_or(path)
}

pub fn detail_body(
    id: SharedString,
    detail: &ToolDetail,
    diff_highlights: Option<Arc<crate::changes::DiffHighlights>>,
    scroll: Option<&ScrollHandle>,
    follow: Option<&Rc<Cell<bool>>>,
    detail_veil: Option<&Rc<RefCell<crate::markdown::veil::RowVeil>>>,
    theme: &Theme,
) -> AnyElement {
    let body = div().w_full().min_w_0().flex().flex_col().overflow_hidden();
    match detail {
        ToolDetail::Diff { file, .. } => body
            .child(crate::changes::render_file_body_with_syntax(
                file,
                diff_highlights,
                theme,
            ))
            .into_any_element(),
        ToolDetail::Stats { stats } => body
            .py(crate::typography::ui_rems(6.0))
            .font_family(theme.font_mono.clone())
            .text_size(crate::typography::ui_rems(TOOL_TEXT_SIZE))
            .children(stats.iter().map(|stat| {
                div()
                    .h(crate::typography::ui_rems(OUTPUT_LINE_HEIGHT))
                    .w_full()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(crate::typography::ui_rems(8.0))
                    .child(
                        crate::file_icons::icon(
                            crate::file_icons::FileIconIdentity::file(&stat.path),
                            theme.appearance,
                        )
                        .size(crate::typography::ui_rems(14.0))
                        .flex_none(),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .text_color(theme.text_faint)
                            .child(SharedString::from(stat.path.clone())),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(theme.success)
                            .child(SharedString::from(format!("+{}", stat.additions))),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(theme.danger)
                            .child(SharedString::from(format!("−{}", stat.deletions))),
                    )
            }))
            .into_any_element(),
        ToolDetail::Output {
            lines,
            truncated_by,
        } => {
            let row_count = lines.len() + usize::from(*truncated_by > 0);
            let rows = div()
                .py(crate::typography::ui_rems(6.0))
                .pr(px(DETAIL_SCROLLBAR_WIDTH))
                .font_family(theme.font_mono.clone())
                .text_size(crate::typography::ui_rems(TOOL_TEXT_SIZE))
                .children(lines.iter().enumerate().map(|(line_ix, line)| {
                    let text = line.clone();
                    let runs = detail_veil.map(|veil| {
                        let run = TextRun {
                            len: text.len(),
                            font: gpui::font(theme.font_mono.clone()),
                            color: theme.text_faint,
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        let spans = veil.borrow_mut().advance(line_ix, &text, Instant::now());
                        crate::markdown::veil::apply_veil(vec![run], &spans)
                    });
                    let text = match runs {
                        Some(runs) => StyledText::new(text).with_runs(runs).into_any_element(),
                        None => div().child(text).into_any_element(),
                    };
                    div()
                        .h(crate::typography::ui_rems(OUTPUT_LINE_HEIGHT))
                        .flex_none()
                        .w_full()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .text_color(theme.text_faint)
                        .child(div().w_full().min_w_0().truncate().child(text))
                }))
                .when(*truncated_by > 0, |block| {
                    block.child(more_lines_row(*truncated_by, theme))
                });
            scrollable_detail(id, scroll, follow, row_count, rows, theme)
        }
        ToolDetail::Thought { lines, .. } => {
            let rows = div()
                .py(crate::typography::ui_rems(6.0))
                .pr(px(DETAIL_SCROLLBAR_WIDTH))
                .text_size(crate::typography::ui_rems(TOOL_TEXT_SIZE))
                .children(lines.iter().enumerate().map(|(line_ix, line)| {
                    let row = div()
                        .h(crate::typography::ui_rems(OUTPUT_LINE_HEIGHT))
                        .flex_none()
                        .w_full()
                        .min_w_0()
                        .flex()
                        .items_center();
                    let Some((text, mut runs)) = thought_line_text(line, theme) else {
                        return row;
                    };
                    if let Some(veil) = detail_veil {
                        let spans = veil.borrow_mut().advance(line_ix, &text, Instant::now());
                        runs = crate::markdown::veil::apply_veil(runs, &spans);
                    }
                    row.child(
                        div()
                            .w_full()
                            .min_w_0()
                            .truncate()
                            .child(StyledText::new(text).with_runs(runs)),
                    )
                }));
            scrollable_detail(id, scroll, follow, lines.len(), rows, theme)
        }
    }
}

pub fn more_lines_row(truncated_by: usize, theme: &Theme) -> gpui::Div {
    div()
        .h(crate::typography::ui_rems(OUTPUT_LINE_HEIGHT))
        .flex()
        .items_center()
        .text_size(crate::typography::ui_rems(TOOL_TEXT_SIZE))
        .text_color(theme.text_faint)
        .child(SharedString::from(format!("… {truncated_by} more lines")))
}

pub fn thought_line_text(
    line: &[InlineRun],
    theme: &Theme,
) -> Option<(SharedString, Vec<TextRun>)> {
    let mut text = String::new();
    let mut runs: Vec<TextRun> = Vec::new();
    for run in line {
        if run.text.is_empty() {
            continue;
        }
        let mut f = if run.style.code {
            gpui::font(theme.font_mono.clone())
        } else {
            gpui::font(theme.font_sans_fixed.clone())
        };
        if run.style.bold {
            f.weight = gpui::FontWeight::SEMIBOLD;
        }
        if run.style.italic {
            f.style = gpui::FontStyle::Italic;
        }
        runs.push(TextRun {
            len: run.text.len(),
            font: f,
            color: theme.text_faint,
            background_color: None,
            underline: run.style.link.is_some().then_some(gpui::UnderlineStyle {
                color: Some(theme.text_faint),
                thickness: px(1.0),
                wavy: false,
            }),
            strikethrough: run.style.strikethrough.then_some(gpui::StrikethroughStyle {
                thickness: px(1.0),
                color: Some(theme.text_faint),
            }),
        });
        text.push_str(&run.text);
    }
    if text.trim().is_empty() {
        return None;
    }
    Some((text.into(), runs))
}

pub(crate) fn chip_header_row(
    tool: &ToolItem,
    trail: Option<ChipTrail>,
    live_elapsed_secs: Option<u64>,
    theme: &Theme,
    view: gpui::EntityId,
    cx: &mut gpui::App,
) -> gpui::Div {
    let thought_label_storage: String;
    let (label, detail) = if tool.is_thought {
        if !tool.resolved {
            if let Some(elapsed) = live_elapsed_secs {
                if elapsed > 0 {
                    let dur = zeron_proto::view::format_duration_secs(elapsed);
                    thought_label_storage = format!("Thinking {dur}...");
                    (thought_label_storage.as_str(), String::new())
                } else {
                    ("Thinking...", String::new())
                }
            } else {
                ("Thinking...", String::new())
            }
        } else if let Some(ms) = tool.thought_duration_ms {
            let dur = zeron_proto::view::format_duration_ms(ms);
            thought_label_storage = format!("Thought for {dur}");
            (thought_label_storage.as_str(), String::new())
        } else if let Some(elapsed) = live_elapsed_secs {
            if elapsed > 0 {
                let dur = zeron_proto::view::format_duration_secs(elapsed);
                thought_label_storage = format!("Thought for {dur}");
                (thought_label_storage.as_str(), String::new())
            } else {
                ("Thought", String::new())
            }
        } else {
            ("Thought", String::new())
        }
    } else {
        tool_chip_content(&tool.call)
    };
    let activity = !is_agent_tool(tool);
    let file_path = match &tool.call {
        ToolCall::ReadFile { path }
        | ToolCall::WriteFile { path, .. }
        | ToolCall::EditFile { path, .. }
        | ToolCall::ApplyPatch { path: Some(path) } => Some(path.as_str()),
        _ => None,
    };
    let running = tool.subagent_ref.is_some()
        && matches!(tool.subagent_status, Some(SubagentStatus::Running));
    let failed = tool.is_error
        || (tool.subagent_ref.is_some()
            && matches!(tool.subagent_status, Some(SubagentStatus::Failed)));
    let hover_text = activity && trail.is_some() && !failed;
    let tint = if failed {
        theme.danger
    } else {
        theme.text_muted
    };
    div()
        .group("tool-header")
        .h(px(if activity {
            CHIP_CARD_HEIGHT
        } else {
            CHIP_HEADER_HEIGHT
        }))
        .w_full()
        .min_w_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .px(px(if activity { 0.0 } else { 8.0 }))
        .text_size(crate::typography::ui_rems(TOOL_LABEL_SIZE))
        .line_height(crate::typography::ui_rems(TOOL_LABEL_LINE_HEIGHT))
        .when(!activity, |row| {
            row.child(
                div()
                    .size(px(18.0))
                    .flex_none()
                    .rounded(px(5.0))
                    .bg(crate::theme::ink(0.08))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        crate::icons::icon(if tool.is_thought {
                            crate::icons::CHAT_ROUND_LINE
                        } else {
                            tool_icon_path(&tool.call)
                        })
                        .size(px(12.0))
                        .text_color(theme.text_muted),
                    ),
            )
        })
        .child(
            div()
                .flex_none()
                .h(px(TOOL_LABEL_LINE_HEIGHT))
                .flex()
                .items_center()
                .when(!activity, |label| {
                    label.font_weight(gpui::FontWeight::MEDIUM)
                })
                .text_color(tint)
                .child(SharedString::from(label))
                .map(|label| {
                    if hover_text {
                        label
                            .id("tool-label")
                            .group_hover("tool-header", |style| style.text_color(theme.text))
                            .into_any_element()
                    } else {
                        label.into_any_element()
                    }
                }),
        )
        .child(
            div()
                .when(!activity, |detail| detail.flex_1())
                .min_w_0()
                .h(px(if file_path.is_some() {
                    22.0
                } else {
                    TOOL_LABEL_LINE_HEIGHT
                }))
                .flex()
                .when(activity && detail.is_empty(), |detail| detail.hidden())
                .items_center()
                .truncate()
                .text_color(if failed {
                    theme.danger
                } else if activity {
                    theme.text_muted
                } else {
                    theme.text.opacity(0.85)
                })
                .child(if let Some(path) = file_path {
                    let badge = div()
                        .min_w_0()
                        .h(px(22.0))
                        .flex()
                        .items_center()
                        .overflow_hidden()
                        .gap(px(6.0))
                        .rounded(px(5.0))
                        .bg(theme.ink(0.06))
                        .pl(px(1.0))
                        .pr(px(6.0))
                        .text_color(if failed {
                            theme.danger
                        } else {
                            theme.text.opacity(0.85)
                        })
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
                                        crate::file_icons::FileIconIdentity::file(path),
                                        theme.appearance,
                                    )
                                    .size(px(14.0)),
                                ),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .child(SharedString::from(file_badge_name(path).to_owned())),
                        )
                        .map(|badge| {
                            if hover_text {
                                badge
                                    .id("tool-file-badge")
                                    .group_hover("tool-header", |style| {
                                        style.text_color(theme.text)
                                    })
                                    .into_any_element()
                            } else {
                                badge.into_any_element()
                            }
                        });
                    crate::frost::frosted(5.0, 16.0, badge).into_any_element()
                } else {
                    div()
                        .min_w_0()
                        .truncate()
                        .child(SharedString::from(detail))
                        .into_any_element()
                })
                .map(|detail| {
                    if hover_text {
                        detail
                            .id("tool-detail")
                            .group_hover("tool-header", |style| style.text_color(theme.text))
                            .into_any_element()
                    } else {
                        detail.into_any_element()
                    }
                }),
        )
        .when_some(tool.call.subagent_model(), |row, model| {
            row.child(
                div()
                    .flex_none()
                    .h(px(18.0))
                    .flex()
                    .items_center()
                    .text_size(crate::typography::ui_rems(12.5))
                    .text_color(theme.text_faint)
                    .child(SharedString::from(model.to_owned())),
            )
        })
        .when(running, |row| {
            row.child(div().flex_none().child(crate::loaders::mini_glyph_spinner(
                format!(
                    "subagent-chip-{}",
                    tool.subagent_ref.as_deref().unwrap_or_default()
                ),
                2.0,
                theme.glyph,
                view,
                cx,
            )))
        })
        .when_some(trail, |row, trail| {
            let tile = div()
                .size(px(18.0))
                .flex_none()
                .when(activity, |tile| {
                    tile.opacity(0.0)
                        .group_hover("tool-header", |style| style.opacity(1.0))
                })
                .when(!activity, |tile| {
                    tile.rounded(px(5.0)).bg(crate::theme::ink(0.06))
                })
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.text_muted.opacity(0.8));
            row.child(match trail {
                ChipTrail::Chevron { open } => tile.child(
                    crate::icons::icon(if open {
                        crate::icons::ALT_ARROW_DOWN
                    } else {
                        crate::icons::ALT_ARROW_RIGHT
                    })
                    .size(px(12.0))
                    .text_color(theme.text_faint)
                    .when(activity, |caret| {
                        caret.group_hover("tool-header", |style| {
                            style.text_color(if failed { theme.danger } else { theme.text })
                        })
                    }),
                ),
                ChipTrail::OpenArrow => tile.child(
                    crate::icons::icon(crate::icons::ARROW_UP_RIGHT)
                        .size(px(11.0))
                        .text_color(theme.text_muted.opacity(0.8)),
                ),
            })
        })
}

pub fn chip_header(
    tool: &ToolItem,
    open: bool,
    live_elapsed_secs: Option<u64>,
    theme: &Theme,
    view: gpui::EntityId,
    cx: &mut gpui::App,
) -> gpui::Div {
    chip_header_row(
        tool,
        Some(ChipTrail::Chevron { open }),
        live_elapsed_secs,
        theme,
        view,
        cx,
    )
}

pub fn title_line(text: &str, max: usize) -> Option<String> {
    let line = text.lines().find(|l| !l.trim().is_empty())?.trim();
    let mut out: String = line.chars().take(max).collect();
    if line.chars().count() > max {
        out.push('…');
    }
    Some(out)
}

pub fn strip_spawn_prefix(text: &str) -> &str {
    let t = text.trim();
    for prefix in ["agent", "task"] {
        if t.len() >= prefix.len()
            && t.is_char_boundary(prefix.len())
            && t[..prefix.len()].eq_ignore_ascii_case(prefix)
        {
            let rest = &t[prefix.len()..];
            if rest.is_empty() {
                return "";
            }
            if rest.starts_with(':') || rest.starts_with(char::is_whitespace) {
                return rest.trim_start_matches(':').trim();
            }
        }
    }
    t
}

pub fn subagent_tab_title(call: &ToolCall) -> SharedString {
    let (name, input) = match call {
        ToolCall::Unknown { name, input } => (name.as_str(), input.as_ref()),
        ToolCall::Mcp { tool, input, .. } => (tool.as_str(), input.as_ref()),
        _ => return "Subagent".into(),
    };
    let candidates = [
        Some(name),
        input.and_then(|i| i.get("description")?.as_str()),
        input.and_then(|i| i.get("prompt")?.as_str()),
    ];
    for text in candidates.into_iter().flatten() {
        if let Some(title) = title_line(strip_spawn_prefix(text), SUBAGENT_TITLE_MAX) {
            return title.into();
        }
    }
    "Subagent".into()
}

pub fn reveal_tool_row(row: AnyElement, height: f32, progress: f32) -> AnyElement {
    if progress >= 1.0 {
        return row;
    }
    div()
        .w_full()
        .h(px(height * progress))
        .flex_none()
        .overflow_hidden()
        .child(row)
        .into_any_element()
}

pub fn activity_rail(
    tool: &ToolItem,
    has_predecessor: bool,
    continues: bool,
    reveal: f32,
    continuation_reveal: f32,
    _row_height: f32,
    theme: &Theme,
) -> gpui::Div {
    let color = theme.hairline(0.12);
    let tint = if tool.is_error {
        theme.danger
    } else {
        theme.text_muted
    };
    let (incoming_reveal, branch_reveal) = tool_connector_parts(reveal, has_predecessor);
    let header_center_y = TOOL_TREE_ROW_HEIGHT / 2.0;
    div()
        .relative()
        .w(px(ACTIVITY_GUTTER_WIDTH))
        .h_full()
        .flex_none()
        .child(
            canvas(
                move |_, _, _| (),
                move |bounds, _, window, _| {
                    let x = bounds.origin.x + px(ACTIVITY_TRUNK_X);
                    let branch_y = bounds.origin.y + px(header_center_y);
                    let bend_y = branch_y - px(ACTIVITY_BEND_RADIUS);
                    let mut tree = PathBuilder::fill().with_style(gpui::PathStyle::Fill(
                        gpui::FillOptions::default().with_fill_rule(gpui::FillRule::NonZero),
                    ));
                    if incoming_reveal > 0.0 {
                        let mut bottom = point(
                            x,
                            bounds.origin.y
                                + px((header_center_y - ACTIVITY_BEND_RADIUS) * incoming_reveal),
                        );
                        if incoming_reveal >= 1.0 && continues && continuation_reveal > 0.0 {
                            let continuation_height = (f32::from(bounds.size.height)
                                - (header_center_y - ACTIVITY_BEND_RADIUS))
                                .max(0.0);
                            bottom = point(
                                x,
                                bend_y + px(continuation_height * continuation_reveal),
                            );
                        }
                        activity_ribbon(&mut tree, &[point(x, bounds.origin.y), bottom]);
                    }
                    if branch_reveal > 0.0 {
                        let points: Vec<_> = activity_branch_points(branch_reveal)
                            .into_iter()
                            .map(|p| point(x + px(p.x), bend_y + px(p.y)))
                            .collect();
                        activity_ribbon(&mut tree, &points);
                    }
                    if branch_reveal >= 1.0 {
                        let icon_center_x =
                            bounds.origin.x + px(ACTIVITY_ICON_LEFT + ACTIVITY_ICON_SIZE / 2.0);
                        let line_top =
                            bounds.origin.y + px(header_center_y + ACTIVITY_ICON_SIZE / 2.0 + 4.0);
                        let line_bottom = bounds.origin.y + bounds.size.height - px(6.0);
                        if line_bottom > line_top {
                            activity_ribbon(
                                &mut tree,
                                &[point(icon_center_x, line_top), point(icon_center_x, line_bottom)],
                            );
                        }
                    }
                    if let Ok(path) = tree.build() {
                        window.paint_path(path, color);
                    }
                },
            )
            .absolute()
            .inset_0(),
        )
        .child(
            crate::icons::icon(if tool.is_thought {
                crate::icons::CHAT_ROUND_LINE
            } else {
                tool_icon_path(&tool.call)
            })
            .absolute()
            .left(px(ACTIVITY_ICON_LEFT))
            .top(px(header_center_y - ACTIVITY_ICON_SIZE / 2.0))
            .size(px(ACTIVITY_ICON_SIZE))
            .opacity(branch_reveal)
            .text_color(tint),
        )
}

pub fn activity_ribbon(path: &mut PathBuilder, points: &[Point<Pixels>]) {
    let mut left = Vec::with_capacity(points.len());
    let mut right = Vec::with_capacity(points.len());
    for (ix, p) in points.iter().enumerate() {
        let a = points[ix.saturating_sub(1)];
        let b = points[(ix + 1).min(points.len() - 1)];
        let dx = f32::from(b.x - a.x);
        let dy = f32::from(b.y - a.y);
        let length = dx.hypot(dy).max(0.0001);
        let normal = point(px(-dy / length * 0.5), px(dx / length * 0.5));
        left.push(*p + normal);
        right.push(*p - normal);
    }
    path.move_to(left[0]);
    for p in left.iter().skip(1).chain(right.iter().rev()) {
        path.line_to(*p);
    }
    path.close();
}

pub fn tool_chip(
    tool: &ToolItem,
    rail: bool,
    has_predecessor: bool,
    continues: bool,
    content_reveal: f32,
    connector_reveal: f32,
    continuation_reveal: f32,
    theme: &Theme,
    view: gpui::EntityId,
    cx: &mut gpui::App,
) -> AnyElement {
    let row_height = if rail {
        TOOL_TREE_ROW_HEIGHT
    } else {
        CHIP_HEIGHT
    };
    div()
        .h(px(row_height))
        .w_full()
        .flex_none()
        .flex()
        .flex_row()
        .when(rail, |row| {
            row.child(activity_rail(
                tool,
                has_predecessor,
                continues,
                connector_reveal,
                continuation_reveal,
                row_height,
                theme,
            ))
        })
        .child(
            div()
                .when(rail, |el| el.ml(px(ACTIVITY_TEXT_GAP)))
                .my(px((row_height - CHIP_CARD_HEIGHT) / 2.0))
                .h(px(CHIP_CARD_HEIGHT))
                .min_w_0()
                .flex_1()
                .flex()
                .items_center()
                .overflow_hidden()
                .when(!rail, |card| {
                    card.rounded(px(9.0))
                        .border_1()
                        .border_color(crate::theme::hairline(0.07))
                        .bg(crate::theme::ink(0.03))
                })
                .when(rail && content_reveal < 1.0, |card| {
                    card.relative()
                        .top(px(4.0 * (1.0 - content_reveal)))
                        .opacity(content_reveal)
                })
                .child(chip_header_row(tool, None, None, theme, view, cx)),
        )
        .into_any_element()
}

pub fn subagent_chip(
    tool: &ToolItem,
    id: SharedString,
    on_open: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
    rail: bool,
    theme: &Theme,
    view: gpui::EntityId,
    cx: &mut gpui::App,
) -> AnyElement {
    div()
        .h(px(CHIP_HEIGHT))
        .w_full()
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .when(rail, |row| {
            row.child(
                div()
                    .ml(px(12.0))
                    .h_full()
                    .w(px(1.0))
                    .flex_none()
                    .bg(crate::theme::ink(0.08)),
            )
        })
        .child(
            div()
                .id(id)
                .when(rail, |el| el.ml(px(12.0)))
                .h(px(CHIP_CARD_HEIGHT))
                .min_w_0()
                .flex_1()
                .flex()
                .items_center()
                .overflow_hidden()
                .rounded(px(9.0))
                .border_1()
                .border_color(crate::theme::hairline(0.07))
                .bg(crate::theme::ink(0.03))
                .cursor_pointer()
                .hover(|s| s.bg(crate::theme::ink(0.05)))
                .on_click(on_open)
                .child(chip_header_row(
                    tool,
                    Some(ChipTrail::OpenArrow),
                    None,
                    theme,
                    view,
                    cx,
                )),
        )
        .into_any_element()
}

#[cfg(test)]
mod detail_follow_tests {
    use super::detail_should_follow;

    #[test]
    fn follows_only_at_detail_bottom() {
        assert!(detail_should_follow(120.0, -120.0));
        assert!(detail_should_follow(120.0, -118.0));
        assert!(!detail_should_follow(120.0, -117.9));
        assert!(!detail_should_follow(120.0, -40.0));
    }
}

// ---------------------------------------------------------------------------
// Transcript methods for rendering ToolGroup
// ---------------------------------------------------------------------------

#[path = "tool_group.rs"]
mod tool_group;
