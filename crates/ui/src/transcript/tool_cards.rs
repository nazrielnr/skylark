//! Tool execution cards, chips, activity rail connector tessellation, and folding.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    canvas, div, point, prelude::*, px, size, AnyElement, Bounds, ContentMask, Context,
    PathBuilder, Pixels, Point, SharedString, StyledText, TextAlign, TextRun, Window,
};
use zeron_doc::SubagentStatus;
use zeron_proto::ToolCall;

use crate::markdown::parser::InlineRun;
use crate::motion;
use crate::notice::{notice_chip, NoticeChipIcon::Tile};
use crate::theme::Theme;
use crate::transcript::row::{
    is_agent_call, is_agent_tool, is_spawn_link, tool_detail, tool_group_collapses,
    DETAIL_SEPARATOR, OUTPUT_BODY_PAD, OUTPUT_LINE_HEIGHT, ToolDetail, ToolItem,
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
pub const ACTIVITY_TRUNK_X: f32 = 12.5;
pub const ACTIVITY_BEND_RADIUS: f32 = 6.0;
pub const ACTIVITY_BRANCH_END_X: f32 = 28.0;
pub const ACTIVITY_ICON_LEFT: f32 = 32.0;
pub const ACTIVITY_ICON_SIZE: f32 = 16.0;

pub const TOOL_TEXT_SIZE: f32 = 12.0;
pub const TOOL_LABEL_SIZE: f32 = TOOL_TEXT_SIZE;
pub const TOOL_LABEL_LINE_HEIGHT: f32 = 18.0;
pub const TOOL_GROUP_HEADER_HEIGHT: f32 = 26.0;
pub const TOOL_TREE_ROW_HEIGHT: f32 = 32.0;

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
            rows as f32 * OUTPUT_LINE_HEIGHT + OUTPUT_BODY_PAD
        }
        ToolDetail::Thought {
            lines,
            truncated_by,
        } => {
            let rows = lines.len() + usize::from(*truncated_by > 0);
            rows as f32 * OUTPUT_LINE_HEIGHT + OUTPUT_BODY_PAD
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

pub fn tool_group_title(text: SharedString, shimmer_phase: Option<f32>, theme: &Theme) -> AnyElement {
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
                .px(px(8.0))
                .text_size(px(12.0))
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
    detail: &ToolDetail,
    diff_highlights: Option<Arc<crate::changes::DiffHighlights>>,
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
            .py(px(6.0))
            .font_family(theme.font_mono.clone())
            .text_size(px(TOOL_TEXT_SIZE))
            .children(stats.iter().map(|stat| {
                div()
                    .h(px(OUTPUT_LINE_HEIGHT))
                    .w_full()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        crate::file_icons::icon(
                            crate::file_icons::FileIconIdentity::file(&stat.path),
                            theme.appearance,
                        )
                        .size(px(14.0))
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
        } => body
            .py(px(6.0))
            .font_family(theme.font_mono.clone())
            .text_size(px(TOOL_TEXT_SIZE))
            .children(lines.iter().map(|line| {
                div()
                    .h(px(OUTPUT_LINE_HEIGHT))
                    .w_full()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .text_color(theme.text_faint)
                    .child(div().w_full().min_w_0().truncate().child(line.clone()))
            }))
            .when(*truncated_by > 0, |block| {
                block.child(more_lines_row(*truncated_by, theme))
            })
            .into_any_element(),
        ToolDetail::Thought {
            lines,
            truncated_by,
        } => body
            .py(px(6.0))
            .text_size(px(TOOL_TEXT_SIZE))
            .children(lines.iter().map(|line| {
                let row = div()
                    .h(px(OUTPUT_LINE_HEIGHT))
                    .w_full()
                    .min_w_0()
                    .flex()
                    .items_center();
                let Some((text, runs)) = thought_line_text(line, theme) else {
                    return row;
                };
                row.child(
                    div()
                        .w_full()
                        .min_w_0()
                        .truncate()
                        .child(StyledText::new(text).with_runs(runs)),
                )
            }))
            .when(*truncated_by > 0, |block| {
                block.child(more_lines_row(*truncated_by, theme))
            })
            .into_any_element(),
    }
}

pub fn more_lines_row(truncated_by: usize, theme: &Theme) -> gpui::Div {
    div()
        .h(px(OUTPUT_LINE_HEIGHT))
        .flex()
        .items_center()
        .text_size(px(TOOL_TEXT_SIZE))
        .text_color(theme.text_faint)
        .child(SharedString::from(format!("… {truncated_by} more lines")))
}

pub fn thought_line_text(line: &[InlineRun], theme: &Theme) -> Option<(SharedString, Vec<TextRun>)> {
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
    theme: &Theme,
    view: gpui::EntityId,
    cx: &mut gpui::App,
) -> gpui::Div {
    let (label, detail) = if tool.is_thought {
        ("Thought process", String::new())
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
        .text_size(px(TOOL_LABEL_SIZE))
        .line_height(px(TOOL_LABEL_LINE_HEIGHT))
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
                    .text_size(px(11.0))
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
    theme: &Theme,
    view: gpui::EntityId,
    cx: &mut gpui::App,
) -> gpui::Div {
    chip_header_row(tool, Some(ChipTrail::Chevron { open }), theme, view, cx)
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
    row_height: f32,
    theme: &Theme,
) -> gpui::Div {
    let color = theme.hairline(0.12);
    let tint = if tool.is_error {
        theme.danger
    } else {
        theme.text_muted
    };
    let (incoming_reveal, branch_reveal) = tool_connector_parts(reveal, has_predecessor);
    div()
        .relative()
        .w(px(ACTIVITY_GUTTER_WIDTH))
        .flex_none()
        .child(
            canvas(
                move |_, _, _| (),
                move |bounds, _, window, _| {
                    let x = bounds.origin.x + px(ACTIVITY_TRUNK_X);
                    let branch_y = bounds.origin.y + px(row_height / 2.0);
                    let bend_y = branch_y - px(ACTIVITY_BEND_RADIUS);
                    let mut tree = PathBuilder::fill().with_style(gpui::PathStyle::Fill(
                        gpui::FillOptions::default().with_fill_rule(gpui::FillRule::NonZero),
                    ));
                    if incoming_reveal > 0.0 {
                        let mut bottom = point(
                            x,
                            bounds.origin.y
                                + px((row_height / 2.0 - ACTIVITY_BEND_RADIUS) * incoming_reveal),
                        );
                        if incoming_reveal >= 1.0 && continues && continuation_reveal > 0.0 {
                            let continuation_height = (f32::from(bounds.size.height)
                                - (row_height / 2.0 - ACTIVITY_BEND_RADIUS))
                                .max(0.0);
                            bottom =
                                point(x, bend_y + px(continuation_height * continuation_reveal));
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
            .top(px(row_height / 2.0 - ACTIVITY_ICON_SIZE / 2.0))
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
                .child(chip_header_row(tool, None, theme, view, cx)),
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
                    theme,
                    view,
                    cx,
                )),
        )
        .into_any_element()
}

// ---------------------------------------------------------------------------
// Transcript methods for rendering ToolGroup
// ---------------------------------------------------------------------------

impl Transcript {
    pub(crate) fn render_tool_group(
        &mut self,
        row_id: &SharedString,
        tools: &Arc<Vec<ToolItem>>,
        summary: &SharedString,
        auto_open: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut fold = self.folds.get(row_id).copied().unwrap_or_default();
        let collapses = tool_group_collapses(tools);
        let arrival_pending = !cx.reduce_motion()
            && self.tool_group_reveals.get(row_id).is_some_and(|reveal| {
                reveal.starts.iter().flatten().any(|start| {
                    Instant::now()
                        .checked_duration_since(*start)
                        .unwrap_or_default()
                        < TOOL_CONNECTOR_REVEAL.total()
                })
            });
        let effective_auto_open = auto_open || arrival_pending;
        let open = !collapses || fold.open.unwrap_or(effective_auto_open);
        if collapses {
            let reveal = self.tool_group_reveals.entry(row_id.clone()).or_default();
            if reveal
                .rendered_open
                .is_some_and(|previous| previous != open)
            {
                fold.from = reveal.rendered_height;
                fold.toggled_at = Some(Instant::now());
                fold.disclosure_at = fold.toggled_at;
                self.folds.insert(row_id.clone(), fold);
            }
            reveal.rendered_open = Some(open);
        }
        let active = collapses && auto_open;

        let body_visible = open
            || (!cx.reduce_motion()
                && fold
                    .toggled_at
                    .is_some_and(|at| at.elapsed() < TOOL_FOLD.total()));
        let tools = if body_visible { tools.as_slice() } else { &[] };
        let details: Vec<Option<Arc<ToolDetail>>> = tools
            .iter()
            .map(|tool| {
                if is_spawn_link(tool) {
                    return None;
                }
                let mut best: Option<(u64, Arc<ToolDetail>)> = None;
                for blob_ref in [&tool.diff_ref, &tool.output_ref].into_iter().flatten() {
                    if let Some(BlobFetch::Ready(detail)) = self.blob_details.get(blob_ref) {
                        let order = self.blob_fetch_order.get(blob_ref).copied().unwrap_or(0);
                        if best.as_ref().is_none_or(|(o, _)| order > *o) {
                            best = Some((order, detail.clone()));
                        }
                    }
                }
                best.map(|(_, d)| d).or_else(|| tool.detail.clone())
            })
            .collect();
        let invocations: Vec<Option<Arc<ToolDetail>>> = tools
            .iter()
            .map(|tool| tool.invocation.clone().filter(|_| !is_spawn_link(tool)))
            .collect();
        let affordances: Vec<Option<ChipAffordance>> = tools
            .iter()
            .map(|tool| {
                let shown: Option<&SharedString> = {
                    let mut best: Option<(u64, &SharedString)> = None;
                    for blob_ref in [&tool.diff_ref, &tool.output_ref].into_iter().flatten() {
                        if matches!(self.blob_details.get(blob_ref), Some(BlobFetch::Ready(_))) {
                            let order = self.blob_fetch_order.get(blob_ref).copied().unwrap_or(0);
                            if best.is_none_or(|(o, _)| order > o) {
                                best = Some((order, blob_ref));
                            }
                        }
                    }
                    best.map(|(_, r)| r)
                };
                let candidates = [
                    (tool.diff_ref.as_ref(), "diff", None),
                    (tool.output_ref.as_ref(), "output", tool.output_bytes),
                ];
                for (blob_ref, what, bytes) in candidates {
                    let Some(blob_ref) = blob_ref else { continue };
                    let label = match self.blob_details.get(blob_ref) {
                        Some(BlobFetch::Ready(_)) => {
                            if shown == Some(blob_ref) {
                                continue;
                            }
                            format!("Show full {what}")
                        }
                        Some(BlobFetch::Loading(_)) => format!("Loading full {what}…"),
                        Some(BlobFetch::Failed) => {
                            format!("Couldn't load full {what} — tap to retry")
                        }
                        None => match bytes {
                            Some(b) => format!("Show full {what} ({})", format_kb(b)),
                            None => format!("Show full {what}"),
                        },
                    };
                    return Some(ChipAffordance {
                        blob_ref: blob_ref.clone(),
                        label: SharedString::from(label),
                    });
                }
                None
            })
            .collect();
        let detail_folds: Vec<FoldState> = details
            .iter()
            .zip(&invocations)
            .enumerate()
            .map(|(ix, (detail, invocation))| {
                if detail.is_none() && invocation.is_none() {
                    return FoldState::default();
                }
                self.tool_details
                    .get(&SharedString::from(format!("{row_id}#d{ix}")))
                    .copied()
                    .unwrap_or_default()
            })
            .collect();
        let detail_opens: Vec<bool> = details
            .iter()
            .zip(&invocations)
            .zip(&detail_folds)
            .zip(tools.iter())
            .map(|(((detail, invocation), fold), tool)| {
                let default_open = tool.is_thought && !tool.resolved;
                (detail.is_some() || invocation.is_some()) && fold.open.unwrap_or(default_open)
            })
            .collect();
        let detail_highlights: Vec<Option<Arc<crate::changes::DiffHighlights>>> = details
            .iter()
            .enumerate()
            .map(|(ix, detail)| {
                detail
                    .as_deref()
                    .filter(|_| detail_opens[ix])
                    .and_then(|detail| self.tool_diff_highlight_for(row_id, ix, detail, cx))
            })
            .collect();
        let base_row_height = if collapses {
            TOOL_TREE_ROW_HEIGHT
        } else {
            CHIP_HEIGHT
        };
        let mut motion_active = false;
        let row_heights: Vec<f32> = details
            .iter()
            .zip(&invocations)
            .zip(&affordances)
            .zip(&detail_opens)
            .zip(&detail_folds)
            .map(|((((detail, invocation), affordance), open), fold)| {
                let target = if *open {
                    base_row_height
                        + invocation.as_deref().map_or(0.0, detail_height)
                        + detail.as_deref().map_or(0.0, detail_height)
                        + if affordance.is_some() {
                            BLOB_AFFORDANCE_HEIGHT
                        } else {
                            0.0
                        }
                } else {
                    base_row_height
                };
                if !cx.reduce_motion() {
                    if let Some(at) = fold.toggled_at {
                        let t = TOOL_FOLD
                            .curve
                            .eval(at.elapsed().as_secs_f32() / TOOL_FOLD.total().as_secs_f32());
                        if t < 1.0 {
                            motion_active = true;
                        }
                        return motion::lerp(
                            fold.from + base_row_height - CHIP_CARD_HEIGHT,
                            target,
                            t,
                        );
                    }
                }
                target
            })
            .collect();
        let reduce_motion = cx.reduce_motion();
        let now = Instant::now();
        let reveal_progress: Vec<f32> = (0..tools.len())
            .map(|ix| {
                let start = self
                    .tool_group_reveals
                    .get(row_id)
                    .and_then(|reveal| reveal.starts.get(ix))
                    .copied()
                    .flatten();
                tool_row_reveal_progress(start, now, reduce_motion)
            })
            .collect();
        let connector_progress: Vec<f32> = (0..tools.len())
            .map(|ix| {
                let start = self
                    .tool_group_reveals
                    .get(row_id)
                    .and_then(|reveal| reveal.starts.get(ix))
                    .copied()
                    .flatten();
                tool_connector_reveal_progress(start, now, reduce_motion)
            })
            .collect();
        let header_reveal = tool_row_reveal_progress(
            self.tool_group_reveals
                .get(row_id)
                .and_then(|reveal| reveal.header_started_at),
            now,
            reduce_motion,
        );
        if header_reveal < 1.0
            || reveal_progress.iter().any(|progress| *progress < 1.0)
            || connector_progress.iter().any(|progress| *progress < 1.0)
        {
            motion_active = true;
        }
        let revealed_height = CHIPS_TOP_PAD
            + row_heights
                .iter()
                .zip(&reveal_progress)
                .map(|(height, progress)| height * progress)
                .sum::<f32>();
        let viewport_height = revealed_height;
        let target = if open { viewport_height } else { 0.0 };
        let shimmer_phase = if active && !reduce_motion {
            motion::pulse_lease(cx.entity_id(), cx);
            self.tool_group_reveals
                .get(row_id)
                .and_then(|reveal| reveal.shimmer_started_at)
                .map(|start| tool_title_shimmer_phase(start, now))
        } else {
            None
        };
        let disclosure_progress = if reduce_motion {
            if open { 1.0 } else { 0.0 }
        } else {
            tool_disclosure_progress(open, fold, now)
        };

        let toggle_id = row_id.clone();
        let header = div()
            .id(SharedString::from(format!("{row_id}-hdr")))
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .pr(px(4.0))
            .h(px(TOOL_GROUP_HEADER_HEIGHT))
            .cursor_pointer()
            .text_size(px(TOOL_LABEL_SIZE))
            .line_height(px(TOOL_LABEL_LINE_HEIGHT))
            .text_color(theme.text_muted)
            .hover(|s| s.text_color(theme.text))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.toggle_fold(toggle_id.clone(), target, effective_auto_open);
                cx.notify();
            }))
            .child(
                div()
                    // Keep the title adjacent to its disclosure affordance.
                    .w(px(22.0))
                    .h(px(18.0))
                    .flex_none()
                    .relative()
                    .child(
                        crate::icons::icon(crate::icons::ALT_ARROW_DOWN)
                            .absolute()
                            .left(px(ACTIVITY_TRUNK_X - 7.0))
                            .top(px(2.0))
                            .size(px(14.0))
                            .with_transformation(gpui::Transformation::rotate(gpui::radians(
                                -std::f32::consts::FRAC_PI_2 * (1.0 - disclosure_progress),
                            )))
                            .text_color(theme.text_muted),
                    ),
            )
            .child(
                div()
                    .min_w_0()
                    .h(px(TOOL_LABEL_LINE_HEIGHT))
                    .flex()
                    .items_center()
                    .truncate()
                    .child(tool_group_title(summary.clone(), shimmer_phase, theme)),
            );

        let chips = div()
            .flex()
            .flex_col()
            .gap(px(CHIP_GAP))
            .pt(px(CHIPS_TOP_PAD))
            .children(tools.iter().enumerate().map(|(ix, tool)| {
                let reveal = reveal_progress[ix];
                let connector_reveal = connector_progress[ix];
                let content_reveal = tool_connector_parts(connector_reveal, ix > 0).1;
                let continuation_reveal =
                    tool_connector_continuation(connector_progress.get(ix + 1).copied());
                let row_height = row_heights[ix];
                if let Some(doc_id) = tool.subagent_ref.clone().filter(|_| is_spawn_link(tool)) {
                    let chat_id = self.chat_id.clone().unwrap_or_default();
                    let title = subagent_tab_title(&tool.call);
                    let frozen = matches!(
                        tool.subagent_status,
                        Some(SubagentStatus::Done) | Some(SubagentStatus::Failed)
                    );
                    return subagent_chip(
                        tool,
                        SharedString::from(format!("{row_id}#s{ix}")),
                        cx.listener(move |_, _, _, cx| {
                            cx.emit(TranscriptEvent::OpenSubagent {
                                chat_id: chat_id.clone(),
                                doc_id: doc_id.to_string(),
                                title: title.to_string(),
                                frozen,
                            });
                        }),
                        collapses,
                        theme,
                        cx.entity_id(),
                        cx,
                    );
                }
                let detail = details[ix].clone();
                let invocation = invocations[ix].clone();
                if detail.is_none() && invocation.is_none() {
                    return reveal_tool_row(
                        tool_chip(
                            tool,
                            collapses,
                            ix > 0,
                            ix + 1 < tools.len(),
                            content_reveal,
                            connector_reveal,
                            continuation_reveal,
                            theme,
                            cx.entity_id(),
                            cx,
                        ),
                        row_height,
                        reveal,
                    );
                }
                let affordance = affordances[ix].clone();
                let open = detail_opens[ix];
                let dfold = detail_folds[ix];
                let key = SharedString::from(format!("{row_id}#d{ix}"));
                let animating = dfold.epoch > 0
                    && dfold
                        .toggled_at
                        .is_some_and(|at| at.elapsed() < FOLD_TWEEN_WINDOW);
                let toggle_key = key.clone();
                let mut card = div()
                    .my(px((base_row_height - CHIP_CARD_HEIGHT) / 2.0))
                    .when(collapses, |el| el.ml(px(ACTIVITY_TEXT_GAP)))
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .when(!collapses, |card| {
                        card.rounded(px(9.0))
                            .border_1()
                            .border_color(crate::theme::hairline(0.07))
                            .bg(crate::theme::ink(0.03))
                    })
                    .child(
                        div()
                            .id(key.clone())
                            .h(px(if collapses {
                                CHIP_CARD_HEIGHT
                            } else {
                                CHIP_HEADER_HEIGHT
                            }))
                            .flex_none()
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                let entry =
                                    this.tool_details.entry(toggle_key.clone()).or_default();
                                let currently_open = entry.open.unwrap_or(open);
                                entry.from = row_height - base_row_height + CHIP_CARD_HEIGHT;
                                entry.open = Some(!currently_open);
                                entry.epoch += 1;
                                entry.toggled_at = Some(Instant::now());
                                cx.notify();
                            }))
                            .child(chip_header(tool, open, theme, cx.entity_id(), cx)),
                    );
                if open || animating {
                    let mut panel = div()
                        .flex_none()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .overflow_hidden();
                    if let Some(invocation) = invocation.as_deref() {
                        panel = panel
                            .child(
                                div()
                                    .h(px(DETAIL_SEPARATOR))
                                    .flex_none()
                                    .when(!collapses, |line| line.bg(crate::theme::hairline(0.06))),
                            )
                            .child(detail_body(invocation, None, theme));
                    }
                    if let Some(detail) = detail.as_deref() {
                        panel = panel
                            .child(
                                div()
                                    .h(px(DETAIL_SEPARATOR))
                                    .flex_none()
                                    .when(!collapses, |line| line.bg(crate::theme::hairline(0.06))),
                            )
                            .child(detail_body(detail, detail_highlights[ix].clone(), theme));
                    }
                    if let Some(ChipAffordance { blob_ref, label }) = affordance {
                        let loading = matches!(
                            self.blob_details.get(&blob_ref),
                            Some(BlobFetch::Loading(_))
                        );
                        let mut row = div()
                            .id(SharedString::from(format!("{key}-blob")))
                            .h(px(BLOB_AFFORDANCE_HEIGHT))
                            .flex_none()
                            .flex()
                            .items_center()
                            .text_size(px(TOOL_TEXT_SIZE))
                            .text_color(theme.text_faint)
                            .child(label);
                        if !loading {
                            row = row
                                .cursor_pointer()
                                .hover(|s| s.text_color(theme.text_muted))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.spawn_blob_fetch(blob_ref.clone(), cx);
                                    cx.notify();
                                }));
                        }
                        panel = panel.child(row);
                    }
                    card = card.child(panel);
                }
                let card = card.h(px(row_height - base_row_height + CHIP_CARD_HEIGHT));
                let card = div().min_w_0().flex_1().child(card);
                let row = div()
                    .w_full()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .when(collapses, |row| {
                        row.child(activity_rail(
                            tool,
                            ix > 0,
                            ix + 1 < tools.len(),
                            connector_reveal,
                            continuation_reveal,
                            row_height,
                            theme,
                        ))
                    })
                    .child(
                        card.when(collapses && content_reveal < 1.0, |card| {
                            card.relative()
                                .top(px(4.0 * (1.0 - content_reveal)))
                                .opacity(content_reveal)
                        }),
                    );
                reveal_tool_row(row.into_any_element(), row_height, reveal)
            }));

        let body_height = if !collapses {
            revealed_height
        } else if !cx.reduce_motion() {
            if let Some(at) = fold.toggled_at {
                let t = TOOL_FOLD
                    .curve
                    .eval(at.elapsed().as_secs_f32() / TOOL_FOLD.total().as_secs_f32());
                if t < 1.0 {
                    motion_active = true;
                }
                motion::lerp(fold.from, target, t)
            } else {
                target
            }
        } else {
            target
        };
        if let Some(reveal) = self.tool_group_reveals.get_mut(row_id) {
            reveal.rendered_height = body_height;
        }
        let body: AnyElement = if !collapses {
            chips.into_any_element()
        } else {
            div()
                .overflow_hidden()
                .h(px(body_height))
                .child(chips)
                .into_any_element()
        };

        let view = cx.entity_id();
        div()
            .relative()
            .flex()
            .flex_col()
            .font_family(theme.font_sans_fixed.clone())
            .when(collapses, |el| {
                el.child(reveal_tool_row(
                    header.into_any_element(),
                    TOOL_GROUP_HEADER_HEIGHT,
                    header_reveal,
                ))
            })
            .child(body)
            .when(motion_active, |group| {
                group.child(
                    canvas(
                        |_, _, _| (),
                        move |_, _, window, _| {
                            window.on_next_frame(move |_, cx| cx.notify(view));
                        },
                    )
                    .absolute()
                    .inset_0(),
                )
            })
            .into_any_element()
    }
}
