//! Git history pane: paged commit rows plus a topological lane graph.
//!
//! The pane is hosted by the current Changes surface for now, but owns its
//! data and rendering so it can move intact into the future right-panel tabs.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use chrono::DateTime;
use gpui::{
    AnyElement, App, ClipboardItem, Context, Entity, EventEmitter, Focusable as _, Image,
    ImageFormat, ListAlignment, ListOffset, ListState, ObjectFit, PathBuilder, Pixels, Render,
    SharedString, Subscription, Task, Window, canvas, container_query, div, img, list, point,
    prelude::*, px,
};
use zeron_engine::repos::git_history_matches;
use zeron_proto::{
    GitHistoryCommit, GitHistoryComparison, GitHistoryPage, GitHistoryRef, GitHistoryRefKind,
};
use zeron_rpc::methods;

use crate::composer::{ComposerInput, ComposerInputEvent};
use crate::motion::AnimationExt;
use crate::popover::{self, Popup};
use crate::settings::{
    self, GitHistoryAuthorDisplay, GitHistoryColumn, GitHistoryColumnOrder, GitHistoryColumnWidths,
    GitHistoryColumns, SavePolicy,
};
use crate::state::AppState;
use crate::theme::Theme;

const HISTORY_PAGE_SIZE: usize = 100;
const HISTORY_ROW_HEIGHT: f32 = 36.0;
const HISTORY_LANE_SPACING: f32 = 12.0;
const HISTORY_NODE_RADIUS: f32 = 3.0;
const HISTORY_HEAD_RING_PADDING: f32 = 2.0;
const HISTORY_STROKE_WIDTH: f32 = 1.5;
const HISTORY_GRAPH_SATURATION: f32 = 0.72;
// Align the first lane center with the header text gutter. The HEAD ring
// must also have breathing room against the pane border.
const HISTORY_GRAPH_SIDE_PADDING: f32 =
    crate::surface_chrome::EDGE_INSET * 2.0 - HISTORY_NODE_RADIUS;
const HISTORY_GRAPH_TRAILING_PADDING: f32 = 12.0;
const HISTORY_GRAPH_MIN_COMPACT_WIDTH: f32 = 48.0;
const HISTORY_GRAPH_MAX_WIDTH_RATIO: f32 = 0.34;
const HISTORY_GRAPH_RESIZE_STEP: f32 = 2.0;
const HISTORY_GRAPH_COMPACT_ENTER_SUBJECT_WIDTH: f32 = 160.0;
const HISTORY_GRAPH_COMPACT_EXIT_SUBJECT_WIDTH: f32 = 184.0;
const HISTORY_GRAPH_COMPACT_ENTER_LANE_SPACING: f32 = 4.0;
const HISTORY_GRAPH_COMPACT_EXIT_LANE_SPACING: f32 = 5.0;
const HISTORY_GRAPH_ROW_OVERLAP: f32 = 0.75;
const HISTORY_GRAPH_HIT_RADIUS: f32 = 5.5;
const HISTORY_GRAPH_FOCUSED_STROKE_WIDTH: f32 = 2.25;
const HISTORY_GRAPH_UNFOCUSED_OPACITY: f32 = 0.24;
const HISTORY_ROW_UNFOCUSED_OPACITY: f32 = 0.6;
const HISTORY_COMMIT_SUBJECT_MIN_WIDTH: f32 = 80.0;
const HISTORY_REF_AREA_RATIO: f32 = 0.45;
const HISTORY_REF_BADGE_MAX_WIDTH: f32 = 112.0;
const HISTORY_REF_GAP: f32 = 5.0;
const HISTORY_SEARCH_WIDTH: f32 = 196.0;
const HISTORY_SEARCH_DEBOUNCE: Duration = Duration::from_millis(70);
const HISTORY_SEARCH_IDLE_DISMISS: Duration = Duration::from_millis(1_500);
/// The header's comparison pill is supplemental metadata. Hide it before it
/// can crowd the fixed History controls on a narrow pane.
const HISTORY_COMPARISON_MIN_WIDTH: f32 = 260.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum GitHistorySearchMode {
    Collapsed,
    Expanded,
    Collapsing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SegmentShape {
    Through,
    Incoming,
    Outgoing,
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct GraphSegment {
    from_lane: usize,
    to_lane: usize,
    color_id: usize,
    shape: SegmentShape,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GraphRow {
    sha: String,
    node_lane: usize,
    node_color_id: usize,
    segments: Vec<GraphSegment>,
    is_head: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct GraphLayout {
    rows: Vec<GraphRow>,
    max_lane_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct GraphGeometry {
    lane_count: usize,
    width: f32,
    lane_spacing: f32,
    device_scale: f32,
    compact: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct GraphGeometryMorph {
    from: GraphGeometry,
    to: GraphGeometry,
    epoch: usize,
}

impl GraphGeometry {
    fn natural(lane_count: usize) -> Self {
        let count = lane_count.max(1);
        Self {
            lane_count: count,
            width: HISTORY_GRAPH_SIDE_PADDING
                + HISTORY_GRAPH_TRAILING_PADDING
                + HISTORY_NODE_RADIUS * 2.0
                + (count - 1) as f32 * HISTORY_LANE_SPACING,
            lane_spacing: HISTORY_LANE_SPACING,
            device_scale: 1.0,
            compact: false,
        }
    }

    fn fitted(lane_count: usize, width: f32) -> Self {
        let count = lane_count.max(1);
        let natural = Self::natural(count);
        if count == 1 || width >= natural.width {
            return natural;
        }
        let fixed_width =
            HISTORY_GRAPH_SIDE_PADDING + HISTORY_GRAPH_TRAILING_PADDING + HISTORY_NODE_RADIUS * 2.0;
        let width = width.clamp(fixed_width, natural.width);
        Self {
            lane_count: count,
            width,
            lane_spacing: (width - fixed_width) / (count - 1) as f32,
            device_scale: 1.0,
            compact: false,
        }
    }

    fn compact(lane_count: usize) -> Self {
        let mut geometry = Self::natural(1);
        geometry.lane_count = lane_count.max(1);
        geometry.lane_spacing = 0.0;
        geometry.compact = true;
        geometry
    }

    fn lane_x(self, lane: usize) -> f32 {
        let x = HISTORY_GRAPH_SIDE_PADDING + HISTORY_NODE_RADIUS + lane as f32 * self.lane_spacing;
        (x * self.device_scale).round() / self.device_scale
    }
}

fn interpolate_graph_geometry(
    from: GraphGeometry,
    to: GraphGeometry,
    progress: f32,
) -> GraphGeometry {
    let progress = progress.clamp(0.0, 1.0);
    let lerp = |start: f32, end: f32| start + (end - start) * progress;
    GraphGeometry {
        lane_count: to.lane_count,
        width: lerp(from.width, to.width),
        lane_spacing: lerp(from.lane_spacing, to.lane_spacing),
        device_scale: to.device_scale,
        // Keep the full topology while lanes converge; the compact rail takes
        // over only once all paths already share its x coordinate. Expanding
        // does the inverse from the rail's first frame.
        compact: if from.compact {
            progress <= 0.001
        } else {
            to.compact && progress >= 0.999
        },
    }
}

#[derive(Debug, Clone, Copy)]
struct GraphFocus {
    color_id: usize,
    amount: f32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GitHistoryViewMode {
    #[default]
    AllCommits,
    BranchTips,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HistoryRowTransition {
    Stable,
    Entering,
    Exiting,
}

struct HistoryViewTransition {
    rows: Vec<HistoryRowTransition>,
    final_commits: Vec<GitHistoryCommit>,
    final_collapsed_counts: HashMap<String, usize>,
    epoch: usize,
}

#[derive(Clone)]
struct HistoryScrollAnchor {
    sha: String,
    offset_in_item: Pixels,
}

fn resolve_history_scroll_anchor(
    anchor: Option<HistoryScrollAnchor>,
    old: &[GitHistoryCommit],
    target: &[GitHistoryCommit],
) -> Option<HistoryScrollAnchor> {
    let anchor = anchor?;
    if target.iter().any(|commit| commit.sha == anchor.sha) {
        return Some(anchor);
    }
    let old_index = old.iter().position(|commit| commit.sha == anchor.sha)?;
    let target_shas: HashSet<&str> = target.iter().map(|commit| commit.sha.as_str()).collect();
    let replacement = old
        .iter()
        .skip(old_index + 1)
        .find(|commit| target_shas.contains(commit.sha.as_str()))
        .or_else(|| {
            old[..old_index]
                .iter()
                .rev()
                .find(|commit| target_shas.contains(commit.sha.as_str()))
        })?;
    Some(HistoryScrollAnchor {
        sha: replacement.sha.clone(),
        offset_in_item: px(0.0),
    })
}

fn history_list_splice(
    old: &[GitHistoryCommit],
    old_has_load_more: bool,
    target: &[GitHistoryCommit],
    target_has_load_more: bool,
) -> Option<(std::ops::Range<usize>, usize)> {
    const LOAD_MORE_KEY: &str = "\0history-load-more";
    let mut old_keys: Vec<&str> = old.iter().map(|commit| commit.sha.as_str()).collect();
    let mut target_keys: Vec<&str> = target.iter().map(|commit| commit.sha.as_str()).collect();
    if old_has_load_more {
        old_keys.push(LOAD_MORE_KEY);
    }
    if target_has_load_more {
        target_keys.push(LOAD_MORE_KEY);
    }
    let prefix = old_keys
        .iter()
        .zip(&target_keys)
        .take_while(|(old, target)| old == target)
        .count();
    let suffix_limit = old_keys.len().min(target_keys.len()).saturating_sub(prefix);
    let suffix = old_keys
        .iter()
        .rev()
        .zip(target_keys.iter().rev())
        .take(suffix_limit)
        .take_while(|(old, target)| old == target)
        .count();
    if prefix == old_keys.len() && prefix == target_keys.len() {
        return None;
    }
    Some((
        prefix..old_keys.len() - suffix,
        target_keys.len() - prefix - suffix,
    ))
}

/// Build a temporary list that preserves every old row while introducing the
/// target rows in their final order. Old-only rows sit beside their previous
/// stable anchor, so contracting them pulls the surrounding commits together
/// instead of making the list flash to a different ordering first.
fn history_transition_rows(
    old: &[GitHistoryCommit],
    target: &[GitHistoryCommit],
) -> (Vec<GitHistoryCommit>, Vec<HistoryRowTransition>) {
    let target_shas: HashSet<&str> = target.iter().map(|commit| commit.sha.as_str()).collect();
    let old_shas: HashSet<&str> = old.iter().map(|commit| commit.sha.as_str()).collect();
    let mut before_first = Vec::new();
    let mut after_anchor: HashMap<String, Vec<GitHistoryCommit>> = HashMap::new();
    let mut anchor: Option<String> = None;

    for commit in old {
        if target_shas.contains(commit.sha.as_str()) {
            anchor = Some(commit.sha.clone());
        } else if let Some(anchor) = anchor.as_ref() {
            after_anchor
                .entry(anchor.clone())
                .or_default()
                .push(commit.clone());
        } else {
            before_first.push(commit.clone());
        }
    }

    let mut commits = Vec::with_capacity(target.len() + old.len());
    let mut rows = Vec::with_capacity(target.len() + old.len());
    for commit in before_first {
        commits.push(commit);
        rows.push(HistoryRowTransition::Exiting);
    }
    for commit in target {
        commits.push(commit.clone());
        rows.push(if old_shas.contains(commit.sha.as_str()) {
            HistoryRowTransition::Stable
        } else {
            HistoryRowTransition::Entering
        });
        if let Some(exiting) = after_anchor.remove(commit.sha.as_str()) {
            for commit in exiting {
                commits.push(commit);
                rows.push(HistoryRowTransition::Exiting);
            }
        }
    }

    // This is only reachable when old contains duplicate anchors or an old
    // sequence has no shared row. Keeping the rows is safer than dropping the
    // exit animation, and the settled target still removes them afterwards.
    for exiting in after_anchor.into_values() {
        for commit in exiting {
            commits.push(commit);
            rows.push(HistoryRowTransition::Exiting);
        }
    }
    (commits, rows)
}

fn branch_ref_key(reference: &GitHistoryRef) -> Option<String> {
    let prefix = match reference.kind {
        GitHistoryRefKind::Branch => "local",
        GitHistoryRefKind::Remote => "remote",
        GitHistoryRefKind::Tag => return None,
    };
    Some(format!("{prefix}:{}", reference.label))
}

struct HistoryColumnPreferences {
    columns: GitHistoryColumns,
    widths: GitHistoryColumnWidths,
    order: GitHistoryColumnOrder,
    author_display: GitHistoryAuthorDisplay,
}

impl gpui::Global for HistoryColumnPreferences {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HistoryDataColumn {
    Commit,
    Author,
    Date,
    Sha,
}

#[derive(Debug, Clone, Copy)]
struct HistoryColumnDragAnchor {
    start_x: f32,
    left: HistoryDataColumn,
    right: HistoryDataColumn,
    left_width: f32,
    right_width: f32,
}

struct HistoryColumnResize;

#[derive(Clone)]
struct HistoryColumnDrag {
    column: GitHistoryColumn,
    label: SharedString,
}

#[derive(Debug, Clone, Copy)]
struct HistoryColumnDragState {
    from: usize,
    over: usize,
}

struct HistoryResizeGhost;

struct HistoryColumnGhost {
    label: SharedString,
}

impl Render for HistoryResizeGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

impl Render for HistoryColumnGhost {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        div()
            .h(px(24.0))
            .min_w(px(64.0))
            .px(px(9.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(5.0))
            .border_1()
            .border_color(theme.border_strong)
            .bg(theme.surface_raised)
            .shadow_md()
            .text_size(px(10.5))
            .text_color(theme.text_muted)
            .opacity(0.9)
            .child(self.label.clone())
    }
}

pub fn init(
    columns: GitHistoryColumns,
    widths: GitHistoryColumnWidths,
    order: GitHistoryColumnOrder,
    author_display: GitHistoryAuthorDisplay,
    cx: &mut App,
) {
    cx.set_global(HistoryColumnPreferences {
        columns,
        widths,
        order,
        author_display,
    });
}

pub fn configured_columns(cx: &App) -> GitHistoryColumns {
    cx.global::<HistoryColumnPreferences>().columns
}

pub fn configured_column_widths(cx: &App) -> GitHistoryColumnWidths {
    cx.global::<HistoryColumnPreferences>().widths
}

pub fn configured_column_order(cx: &App) -> GitHistoryColumnOrder {
    cx.global::<HistoryColumnPreferences>().order.clone()
}

pub fn configured_author_display(cx: &App) -> GitHistoryAuthorDisplay {
    cx.global::<HistoryColumnPreferences>().author_display
}

fn history_column_label(column: GitHistoryColumn) -> &'static str {
    match column {
        GitHistoryColumn::Author => "Author",
        GitHistoryColumn::Date => "Date",
        GitHistoryColumn::Sha => "SHA",
    }
}

fn history_column_is_visible(column: GitHistoryColumn, columns: GitHistoryColumns) -> bool {
    match column {
        GitHistoryColumn::Author => columns.author,
        GitHistoryColumn::Date => columns.date,
        GitHistoryColumn::Sha => columns.sha,
    }
}

fn visible_history_columns(
    order: &GitHistoryColumnOrder,
    columns: GitHistoryColumns,
) -> Vec<GitHistoryColumn> {
    order
        .0
        .iter()
        .copied()
        .filter(|column| history_column_is_visible(*column, columns))
        .collect()
}

fn history_data_column(column: GitHistoryColumn) -> HistoryDataColumn {
    match column {
        GitHistoryColumn::Author => HistoryDataColumn::Author,
        GitHistoryColumn::Date => HistoryDataColumn::Date,
        GitHistoryColumn::Sha => HistoryDataColumn::Sha,
    }
}

fn history_optional_width(column: GitHistoryColumn, widths: GitHistoryColumnWidths) -> f32 {
    history_column_width(history_data_column(column), widths)
}

fn history_column_drop_index(
    relative_x: f32,
    rendered_width: f32,
    columns: &[GitHistoryColumn],
    widths: GitHistoryColumnWidths,
) -> usize {
    if columns.is_empty() || rendered_width <= 0.0 {
        return 0;
    }
    let desired_width = columns
        .iter()
        .map(|column| history_optional_width(*column, widths))
        .sum::<f32>();
    let x = relative_x.clamp(0.0, rendered_width) * desired_width / rendered_width;
    let mut cursor = 0.0;
    for (index, column) in columns.iter().enumerate() {
        let width = history_optional_width(*column, widths);
        if x < cursor + width / 2.0 {
            return index;
        }
        cursor += width;
    }
    columns.len() - 1
}

fn reordered_history_columns(
    order: &GitHistoryColumnOrder,
    dragged: GitHistoryColumn,
    target: GitHistoryColumn,
) -> GitHistoryColumnOrder {
    if dragged == target {
        return order.clone();
    }
    let Some(from) = order.0.iter().position(|column| *column == dragged) else {
        return order.clone();
    };
    let Some(over) = order.0.iter().position(|column| *column == target) else {
        return order.clone();
    };
    let mut columns = order.0.clone();
    columns.remove(from);
    let target_after_removal = columns
        .iter()
        .position(|column| *column == target)
        .unwrap_or(columns.len());
    let insertion = if from < over {
        target_after_removal + 1
    } else {
        target_after_removal
    };
    columns.insert(insertion.min(columns.len()), dragged);
    GitHistoryColumnOrder(columns)
}

fn history_column_width(column: HistoryDataColumn, widths: GitHistoryColumnWidths) -> f32 {
    match column {
        HistoryDataColumn::Commit => HISTORY_COMMIT_SUBJECT_MIN_WIDTH,
        HistoryDataColumn::Author => widths.author,
        HistoryDataColumn::Date => widths.date,
        HistoryDataColumn::Sha => widths.sha,
    }
}

fn history_column_limits(column: HistoryDataColumn) -> (f32, f32) {
    match column {
        HistoryDataColumn::Commit => (HISTORY_COMMIT_SUBJECT_MIN_WIDTH, f32::MAX),
        HistoryDataColumn::Author => (
            GitHistoryColumnWidths::AUTHOR_MIN,
            GitHistoryColumnWidths::AUTHOR_MAX,
        ),
        HistoryDataColumn::Date => (
            GitHistoryColumnWidths::DATE_MIN,
            GitHistoryColumnWidths::DATE_MAX,
        ),
        HistoryDataColumn::Sha => (
            GitHistoryColumnWidths::SHA_MIN,
            GitHistoryColumnWidths::SHA_MAX,
        ),
    }
}

fn set_history_column_width(
    widths: &mut GitHistoryColumnWidths,
    column: HistoryDataColumn,
    width: f32,
) {
    match column {
        HistoryDataColumn::Commit => {}
        HistoryDataColumn::Author => widths.author = width,
        HistoryDataColumn::Date => widths.date = width,
        HistoryDataColumn::Sha => widths.sha = width,
    }
}

fn resized_history_column_widths(
    mut widths: GitHistoryColumnWidths,
    anchor: HistoryColumnDragAnchor,
    requested_delta: f32,
) -> GitHistoryColumnWidths {
    if anchor.left == HistoryDataColumn::Commit {
        let (right_min, right_max) = history_column_limits(anchor.right);
        set_history_column_width(
            &mut widths,
            anchor.right,
            (anchor.right_width - requested_delta).clamp(right_min, right_max),
        );
    } else {
        let (left_min, left_max) = history_column_limits(anchor.left);
        let (right_min, right_max) = history_column_limits(anchor.right);
        let min_delta = (left_min - anchor.left_width).max(anchor.right_width - right_max);
        let max_delta = (left_max - anchor.left_width).min(anchor.right_width - right_min);
        let delta = requested_delta.clamp(min_delta, max_delta);
        set_history_column_width(&mut widths, anchor.left, anchor.left_width + delta);
        set_history_column_width(&mut widths, anchor.right, anchor.right_width - delta);
    }
    widths
}

#[path = "history/graph.rs"]
mod graph;
use graph::*;

fn estimated_ref_badge_width(reference: &GitHistoryRef) -> f32 {
    // 10 px icon + 2 px gap + 10 px horizontal padding + the 10 px label.
    (22.0 + reference.label.chars().count() as f32 * 5.7).min(HISTORY_REF_BADGE_MAX_WIDTH)
}

fn estimated_ref_overflow_width(hidden: usize) -> f32 {
    format!("+{hidden}").chars().count() as f32 * 5.7
}

fn ref_area_width(commit_column_width: f32) -> f32 {
    let inner = (commit_column_width - 8.0).max(0.0);
    (inner * HISTORY_REF_AREA_RATIO)
        .min((inner - HISTORY_COMMIT_SUBJECT_MIN_WIDTH - HISTORY_REF_GAP).max(0.0))
}

fn visible_ref_count(refs: &[GitHistoryRef], available_width: f32) -> usize {
    if refs.is_empty() {
        return 0;
    }

    // Start with no visible badges so the overflow target remains available
    // when even the first badge plus `+N` would exceed the ref area.
    let mut visible = 0;
    for count in 1..=refs.len() {
        let hidden = refs.len() - count;
        let item_count = count + usize::from(hidden > 0);
        let badges = refs[..count]
            .iter()
            .map(estimated_ref_badge_width)
            .sum::<f32>();
        let overflow = (hidden > 0)
            .then(|| estimated_ref_overflow_width(hidden))
            .unwrap_or_default();
        let gaps = item_count.saturating_sub(1) as f32 * HISTORY_REF_GAP;
        if badges + overflow + gaps <= available_width {
            visible = count;
        }
    }
    visible
}

fn format_date(value: &str) -> String {
    DateTime::parse_from_rfc3339(value)
        .map(|date| date.format("%b %-d, %Y").to_string())
        .unwrap_or_else(|_| "—".to_string())
}

fn ref_color(reference: &GitHistoryRef, theme: &Theme) -> gpui::Hsla {
    match reference.kind {
        GitHistoryRefKind::Branch => theme.accent,
        GitHistoryRefKind::Remote => theme.busy,
        GitHistoryRefKind::Tag => theme.warning,
    }
}

fn ref_icon(kind: GitHistoryRefKind) -> &'static str {
    match kind {
        GitHistoryRefKind::Branch => crate::icons::GIT_BRANCH,
        GitHistoryRefKind::Remote => crate::icons::CLOUD,
        GitHistoryRefKind::Tag => crate::icons::TAG,
    }
}

fn ref_description(reference: &GitHistoryRef) -> SharedString {
    let kind = match reference.kind {
        GitHistoryRefKind::Branch => "Branch",
        GitHistoryRefKind::Remote => "Remote branch",
        GitHistoryRefKind::Tag => "Tag",
    };
    format!("{kind}: {}", reference.label).into()
}

fn graph_color(mut color: gpui::Hsla) -> gpui::Hsla {
    color.s *= HISTORY_GRAPH_SATURATION;
    color
}

struct HistoryRefTooltip {
    descriptions: Vec<SharedString>,
}

impl Render for HistoryRefTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let longest = self
            .descriptions
            .iter()
            .map(|description| description.chars().count())
            .max()
            .unwrap_or_default();
        let width = (longest as f32 * 6.4 + 16.0).clamp(72.0, 360.0);
        div()
            .w(px(width))
            .px(px(8.0))
            .py(px(6.0))
            .flex()
            .flex_col()
            .gap(px(3.0))
            .rounded(px(5.0))
            .border_1()
            .border_color(theme.border_strong)
            .bg(theme.surface_raised)
            .shadow_md()
            .text_size(px(11.0))
            .text_color(theme.text_muted)
            .children(self.descriptions.iter().cloned().map(|description| {
                div()
                    .min_w_0()
                    .truncate()
                    .whitespace_nowrap()
                    .font_family(theme.font_mono.clone())
                    .child(description)
            }))
    }
}

struct HistoryAuthorTooltip {
    name: SharedString,
}

impl Render for HistoryAuthorTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let width = (self.name.chars().count() as f32 * 6.2 + 16.0).clamp(72.0, 260.0);
        div()
            .w(px(width))
            .px(px(8.0))
            .py(px(5.0))
            .rounded(px(5.0))
            .border_1()
            .border_color(theme.border_strong)
            .bg(theme.surface_raised)
            .shadow_md()
            .truncate()
            .whitespace_nowrap()
            .text_size(px(11.0))
            .text_color(theme.text_muted)
            .child(self.name.clone())
    }
}

fn history_author_name(name: &str) -> SharedString {
    if name.trim().is_empty() {
        "Unknown".into()
    } else {
        name.to_string().into()
    }
}

fn history_author_initial(name: &str) -> SharedString {
    name.chars()
        .find(|character| !character.is_whitespace())
        .map(|character| character.to_uppercase().collect::<String>())
        .unwrap_or_else(|| "?".to_string())
        .into()
}

fn decode_history_avatar(encoded: &str) -> Option<Arc<Image>> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    let format = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        ImageFormat::Png
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        ImageFormat::Jpeg
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        ImageFormat::Gif
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        ImageFormat::Webp
    } else {
        return None;
    };
    Some(Arc::new(Image::from_bytes(format, bytes)))
}

pub struct GitHistory {
    state: Entity<AppState>,
    started: bool,
    target_key: Option<String>,
    commits: Vec<GitHistoryCommit>,
    visible_commits: Vec<GitHistoryCommit>,
    branch_tips: Vec<GitHistoryCommit>,
    view_mode: GitHistoryViewMode,
    view_epoch: usize,
    view_transition: Option<HistoryViewTransition>,
    view_transition_task: Option<Task<()>>,
    collapsed_branches: HashSet<String>,
    collapsed_counts: HashMap<String, usize>,
    head_sha: Option<String>,
    next_cursor: Option<usize>,
    total_count: Option<usize>,
    head_commit_count: Option<usize>,
    comparison: Option<GitHistoryComparison>,
    search_query: String,
    search_results: Option<Vec<GitHistoryCommit>>,
    search_next_cursor: Option<usize>,
    search_total_count: Option<usize>,
    search_loading: bool,
    search_error: Option<SharedString>,
    search_generation: usize,
    search_scroll_anchor: Option<HistoryScrollAnchor>,
    search_task: Option<Task<()>>,
    loading: bool,
    error: Option<SharedString>,
    graph: GraphLayout,
    graph_lane_capacity: usize,
    /// Updated by the surface-level container query before rows paint. The
    /// same geometry drives canvas paths, nodes, headers, and hit testing.
    graph_geometry: Rc<Cell<GraphGeometry>>,
    /// The settled responsive target. This stays separate from the animated
    /// geometry so resize hysteresis never reads an in-between lane layout.
    graph_target_geometry: Rc<Cell<GraphGeometry>>,
    graph_geometry_morph: Rc<Cell<Option<GraphGeometryMorph>>>,
    graph_hover_suppressed: Rc<Cell<bool>>,
    list: ListState,
    hovered_path: Option<usize>,
    hovered_graph_path: Option<usize>,
    hovered_row_path: Option<usize>,
    graph_hover_active: bool,
    graph_hover_clear_task: Option<Task<()>>,
    column_drag_anchor: Option<HistoryColumnDragAnchor>,
    column_drag: Option<HistoryColumnDragState>,
    avatar_images: HashMap<String, Arc<Image>>,
    column_menu: Popup<gpui::Point<gpui::Pixels>>,
    author_menu: Popup<gpui::Point<gpui::Pixels>>,
    copied_sha: Option<String>,
    request_task: Option<Task<()>>,
    copy_task: Option<Task<()>>,
    fetching_all: bool,
    fetch_for: Option<String>,
    fetch_error: Option<SharedString>,
    fetch_task: Option<Task<()>>,
    _observe: Subscription,
}

pub enum GitHistoryEvent {
    FetchSucceeded,
    /// A commit row was clicked — the host opens it as its own diff tab.
    OpenCommit(GitHistoryCommit),
}

impl EventEmitter<GitHistoryEvent> for GitHistory {}

pub struct GitHistoryCount {
    history: Entity<GitHistory>,
    _observe: Subscription,
}

pub struct GitHistoryFetchButton {
    history: Entity<GitHistory>,
    _observe: Subscription,
}

pub struct GitHistoryViewButton {
    history: Entity<GitHistory>,
    _observe: Subscription,
}

pub struct GitHistorySearchControl {
    history: Entity<GitHistory>,
    input: Entity<ComposerInput>,
    mode: GitHistorySearchMode,
    idle_dismiss_epoch: usize,
    transition_epoch: usize,
    idle_dismiss_task: Option<Task<()>>,
    transition_task: Option<Task<()>>,
    blur_subscription: Option<Subscription>,
    _observe: Subscription,
    _input_events: Subscription,
}

impl GitHistorySearchControl {
    pub fn new(history: Entity<GitHistory>, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            ComposerInput::with_context("Search", "PaletteSearch", cx).with_text_metrics(11.0, 14.0)
        });
        let observe = cx.observe(&history, |this, history, cx| {
            let query = history.read(cx).search_query.clone();
            if this.input.read(cx).text() != query {
                this.input.update(cx, |input, cx| input.set_text(query, cx));
            }
            cx.notify();
        });
        let input_events = cx.subscribe(&input, |this: &mut Self, input, event, cx| {
            if matches!(event, ComposerInputEvent::Edited) {
                let query = input.read(cx).text().to_string();
                let query_is_empty = query.is_empty();
                this.history
                    .update(cx, |history, cx| history.set_search_query(query, cx));
                if query_is_empty {
                    this.schedule_idle_dismiss(cx);
                } else {
                    this.cancel_idle_dismiss();
                }
            }
        });
        Self {
            history,
            input,
            mode: GitHistorySearchMode::Collapsed,
            idle_dismiss_epoch: 0,
            transition_epoch: 0,
            idle_dismiss_task: None,
            transition_task: None,
            blur_subscription: None,
            _observe: observe,
            _input_events: input_events,
        }
    }

    fn cancel_idle_dismiss(&mut self) {
        self.idle_dismiss_epoch = self.idle_dismiss_epoch.wrapping_add(1);
        self.idle_dismiss_task = None;
    }

    fn schedule_idle_dismiss(&mut self, cx: &mut Context<Self>) {
        self.cancel_idle_dismiss();
        if self.mode != GitHistorySearchMode::Expanded || !self.input.read(cx).text().is_empty() {
            return;
        }
        let epoch = self.idle_dismiss_epoch;
        self.idle_dismiss_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(HISTORY_SEARCH_IDLE_DISMISS)
                .await;
            this.update(cx, |control, cx| {
                if control.idle_dismiss_epoch == epoch
                    && control.mode == GitHistorySearchMode::Expanded
                    && control.input.read(cx).text().is_empty()
                {
                    control.begin_collapse(cx);
                }
            })
            .ok();
        }));
    }

    fn begin_collapse(&mut self, cx: &mut Context<Self>) {
        if self.mode != GitHistorySearchMode::Expanded || !self.input.read(cx).text().is_empty() {
            return;
        }
        self.cancel_idle_dismiss();
        self.mode = GitHistorySearchMode::Collapsing;
        self.transition_epoch = self.transition_epoch.wrapping_add(1);
        let epoch = self.transition_epoch;
        let duration = crate::motion::RESIZE
            .total()
            .mul_f32(crate::motion::speed_scale());
        self.transition_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(duration).await;
            this.update(cx, |control, cx| {
                if control.transition_epoch == epoch
                    && control.mode == GitHistorySearchMode::Collapsing
                {
                    // Unmounting the input is intentional: GPUI otherwise keeps
                    // its focused caret alive after this compact control is idle.
                    control.mode = GitHistorySearchMode::Collapsed;
                    control.transition_task = None;
                    cx.notify();
                }
            })
            .ok();
        }));
        cx.notify();
    }

    fn expand(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_idle_dismiss();
        self.transition_epoch = self.transition_epoch.wrapping_add(1);
        self.transition_task = None;
        self.mode = GitHistorySearchMode::Expanded;
        // The collapsed render does not mount `input`. Focusing its handle in
        // this click cycle leaves the next focus path empty, so Shell's
        // focus-lost fallback legitimately restores the composer. Wait until
        // the expanded state has driven a frame, then complete the handoff.
        let control = cx.entity().downgrade();
        window.on_next_frame(move |window, cx| {
            control
                .update(cx, |control, cx| {
                    if control.mode != GitHistorySearchMode::Expanded {
                        return;
                    }
                    let focus = control.input.read(cx).focus_handle(cx);
                    window.focus(&focus, cx);
                    control.schedule_idle_dismiss(cx);
                    cx.notify();
                })
                .ok();
        });
        cx.notify();
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        if !self.input.read(cx).text().is_empty() {
            self.input.update(cx, |input, cx| input.set_text("", cx));
        }
        self.schedule_idle_dismiss(cx);
        cx.notify();
    }
}

impl GitHistoryFetchButton {
    pub fn new(history: Entity<GitHistory>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&history, |_, _, cx| cx.notify());
        Self {
            history,
            _observe: observe,
        }
    }
}

impl GitHistoryViewButton {
    pub fn new(history: Entity<GitHistory>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&history, |_, _, cx| cx.notify());
        Self {
            history,
            _observe: observe,
        }
    }
}

impl Render for GitHistoryFetchButton {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let fetching = self.history.read(cx).fetching_all;
        let history = self.history.clone();
        div()
            .id("history-fetch-all")
            .h(px(crate::surface_chrome::CONTROL_SIZE))
            .px(px(8.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(6.0))
            .rounded(px(crate::surface_chrome::CONTROL_RADIUS))
            .bg(if fetching {
                crate::theme::wash(0.05)
            } else {
                crate::motion::hover_blend(
                    "history-fetch-all",
                    crate::theme::wash(0.0),
                    crate::theme::wash(0.14),
                )
            })
            .occlude()
            .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
                window.prevent_default()
            })
            .when(!fetching, |element| {
                element
                    .cursor_pointer()
                    .on_hover(crate::motion::hover_listener("history-fetch-all"))
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        history.update(cx, |history, cx| history.fetch_all(cx));
                    })
            })
            .child(if fetching {
                crate::loaders::mini_glyph_spinner(
                    "history-fetch-all-spinner",
                    1.75,
                    theme.glyph,
                    cx.entity_id(),
                    cx,
                )
                .into_any_element()
            } else {
                crate::icons::icon(crate::icons::CLOUD)
                    .size(px(crate::surface_chrome::ICON_SIZE))
                    .text_color(theme.text_muted.opacity(0.75))
                    .into_any_element()
            })
            .child(
                div()
                    .whitespace_nowrap()
                    .text_size(px(11.0))
                    .text_color(if fetching {
                        theme.text_faint
                    } else {
                        theme.text_muted
                    })
                    .child(if fetching { "Fetching…" } else { "Fetch all" }),
            )
    }
}

impl Render for GitHistoryViewButton {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let showing_tips = self.history.read(cx).view_mode == GitHistoryViewMode::BranchTips;
        let history = self.history.clone();
        let tooltip = if showing_tips {
            "Show all commits"
        } else {
            "Show branch tips"
        };

        div()
            .id("history-view-trigger")
            .size(px(crate::surface_chrome::CONTROL_SIZE))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(crate::surface_chrome::CONTROL_RADIUS))
            .cursor_pointer()
            .bg(if showing_tips {
                theme.accent.opacity(0.12)
            } else {
                crate::motion::hover_blend(
                    "history-view-trigger",
                    crate::theme::wash(0.0),
                    crate::theme::wash(0.14),
                )
            })
            .on_hover(crate::motion::hover_listener("history-view-trigger"))
            .occlude()
            .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
                window.prevent_default()
            })
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                history.update(cx, |history, cx| {
                    let mode = if history.view_mode == GitHistoryViewMode::BranchTips {
                        GitHistoryViewMode::AllCommits
                    } else {
                        GitHistoryViewMode::BranchTips
                    };
                    history.set_view_mode(mode, cx);
                });
            })
            .child(
                crate::icons::icon(crate::icons::FOLD_VERTICAL)
                    .size(px(crate::surface_chrome::ICON_SIZE))
                    .text_color(if showing_tips {
                        theme.accent
                    } else {
                        theme.text_muted
                    }),
            )
            .tooltip(move |_, cx| {
                cx.new(|_| HistoryRefTooltip {
                    descriptions: vec![tooltip.into()],
                })
                .into()
            })
            .tooltip_show_delay(Duration::from_millis(350))
    }
}

impl Render for GitHistorySearchControl {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        if self.mode == GitHistorySearchMode::Collapsed {
            let control = cx.entity().downgrade();
            return div()
                .id("history-search-trigger")
                .size(px(crate::surface_chrome::CONTROL_SIZE))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(crate::surface_chrome::CONTROL_RADIUS))
                .cursor_pointer()
                .bg(crate::motion::hover_blend(
                    "history-search-trigger",
                    crate::theme::wash(0.0),
                    crate::theme::wash(0.14),
                ))
                .on_hover(crate::motion::hover_listener("history-search-trigger"))
                .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
                    window.prevent_default()
                })
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    control
                        .update(cx, |control, cx| control.expand(window, cx))
                        .ok();
                })
                .child(
                    crate::icons::icon(crate::icons::MAGNIFER)
                        .size(px(crate::surface_chrome::ICON_SIZE))
                        .text_color(theme.text_muted),
                )
                .tooltip(|_, cx| {
                    cx.new(|_| HistoryRefTooltip {
                        descriptions: vec!["Search commits".into()],
                    })
                    .into()
                })
                .tooltip_show_delay(Duration::from_millis(350))
                .into_any_element();
        }
        if self.blur_subscription.is_none() {
            let focus = self.input.read(cx).focus_handle(cx);
            self.blur_subscription = Some(cx.on_blur(&focus, window, |control, _, cx| {
                control.clear(cx);
            }));
        }
        let control = cx.entity().downgrade();
        let search_loading = self.history.read(cx).search_loading;
        let closing = self.mode == GitHistorySearchMode::Collapsing;
        let transition_epoch = self.transition_epoch;
        let status_icon = if search_loading {
            crate::loaders::mini_glyph_spinner(
                "history-search-spinner",
                1.5,
                theme.glyph,
                cx.entity_id(),
                cx,
            )
            .into_any_element()
        } else {
            crate::icons::icon(crate::icons::MAGNIFER)
                .size(px(11.0))
                .flex_none()
                .text_color(theme.text_faint)
                .into_any_element()
        };
        div()
            .id("history-search-expanded")
            .h(px(crate::surface_chrome::CONTROL_SIZE))
            .w(px(HISTORY_SEARCH_WIDTH))
            .min_w(px(80.0))
            .flex_shrink(1.0)
            .overflow_hidden()
            .flex()
            .items_center()
            .gap(px(6.0))
            .pl(px(crate::surface_chrome::EDGE_INSET))
            .pr(px(2.0))
            .rounded(px(crate::surface_chrome::CONTROL_RADIUS))
            .bg(crate::theme::ink(0.035))
            .child(
                div()
                    .size(px(14.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(status_icon),
            )
            .child(
                div()
                    .h(px(14.0))
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .child(self.input.clone()),
            )
            .child(
                div()
                    .id("history-search-close")
                    .size(px(16.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(3.5))
                    .cursor_pointer()
                    .hover(|style| style.bg(crate::theme::ink(0.08)))
                    .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
                        window.prevent_default()
                    })
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        control.update(cx, |control, cx| control.clear(cx)).ok();
                    })
                    .child(
                        crate::icons::icon(crate::icons::CLOSE)
                            .size(px(9.0))
                            .text_color(theme.text_faint),
                    ),
            )
            .with_animation(
                SharedString::from(format!(
                    "history-search-morph-{transition_epoch}-{}",
                    if closing { "out" } else { "in" }
                )),
                crate::motion::RESIZE.animation(),
                move |element, progress| {
                    let amount = if closing { 1.0 - progress } else { progress };
                    element
                        .w(px(24.0 + (HISTORY_SEARCH_WIDTH - 24.0) * amount))
                        .opacity(0.45 + 0.55 * amount)
                },
            )
            .into_any_element()
    }
}

impl GitHistoryCount {
    pub fn new(history: Entity<GitHistory>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&history, |_, _, cx| cx.notify());
        Self {
            history,
            _observe: observe,
        }
    }
}

impl Render for GitHistoryCount {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let history = self.history.read(cx);
        let count = history.commit_count();
        let comparison = history
            .comparison
            .clone()
            .filter(|comparison| comparison.ahead > 0 || comparison.behind > 0);
        div()
            .h_full()
            .min_w_0()
            .flex_1()
            .flex()
            .items_center()
            .overflow_hidden()
            .child(
                container_query(move |size, _, _| {
                    let comparison_fits = f32::from(size.width) >= HISTORY_COMPARISON_MIN_WIDTH;
                    div()
                        .size_full()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .when_some(count, |element, count| {
                            element.child(
                                div()
                                    .flex_none()
                                    .whitespace_nowrap()
                                    .text_size(px(11.0))
                                    .line_height(px(14.0))
                                    .text_color(theme.text_muted)
                                    .child(SharedString::from(format!(
                                        "{count} commit{}",
                                        if count == 1 { "" } else { "s" }
                                    ))),
                            )
                        })
                        .when_some(
                            comparison_fits.then(|| comparison.clone()).flatten(),
                            |element, comparison| {
                                let base = comparison.base.clone();
                                let ahead = comparison.ahead;
                                let behind = comparison.behind;
                                element.child(
                                    div()
                                        .id("history-comparison")
                                        .relative()
                                        .top(px(1.0))
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .gap(px(4.0))
                                        .when(ahead > 0, |comparison| {
                                            comparison.child(
                                                div()
                                                    .whitespace_nowrap()
                                                    .text_size(px(10.5))
                                                    .line_height(px(13.0))
                                                    .text_color(theme.accent.opacity(0.88))
                                                    .child(SharedString::from(format!(
                                                        "{ahead} ahead"
                                                    ))),
                                            )
                                        })
                                        .when(ahead > 0 && behind > 0, |comparison| {
                                            comparison.child(
                                                div()
                                                    .text_size(px(10.0))
                                                    .text_color(theme.text_faint)
                                                    .child("·"),
                                            )
                                        })
                                        .when(behind > 0, |comparison| {
                                            comparison.child(
                                                div()
                                                    .whitespace_nowrap()
                                                    .text_size(px(10.5))
                                                    .line_height(px(13.0))
                                                    .text_color(theme.warning.opacity(0.82))
                                                    .child(SharedString::from(format!(
                                                        "{behind} behind"
                                                    ))),
                                            )
                                        })
                                        .tooltip(move |_, cx| {
                                            cx.new(|_| HistoryRefTooltip {
                                                descriptions: vec![
                                                    format!(
                                                        "Compared with {base}: {ahead} ahead, {behind} behind"
                                                    )
                                                    .into(),
                                                ],
                                            })
                                            .into()
                                        }),
                                )
                            },
                        )
                })
                .size_full(),
            )
    }
}

impl GitHistory {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&state, |this, _, cx| {
            if this.started {
                this.ensure_loaded(cx);
            }
        });
        Self {
            state,
            started: false,
            target_key: None,
            commits: Vec::new(),
            visible_commits: Vec::new(),
            branch_tips: Vec::new(),
            view_mode: GitHistoryViewMode::default(),
            view_epoch: 0,
            view_transition: None,
            view_transition_task: None,
            collapsed_branches: HashSet::new(),
            collapsed_counts: HashMap::new(),
            head_sha: None,
            next_cursor: None,
            total_count: None,
            head_commit_count: None,
            comparison: None,
            search_query: String::new(),
            search_results: None,
            search_next_cursor: None,
            search_total_count: None,
            search_loading: false,
            search_error: None,
            search_generation: 0,
            search_scroll_anchor: None,
            search_task: None,
            loading: false,
            error: None,
            graph: GraphLayout::default(),
            graph_lane_capacity: 0,
            graph_geometry: Rc::new(Cell::new(GraphGeometry::natural(1))),
            graph_target_geometry: Rc::new(Cell::new(GraphGeometry::natural(1))),
            graph_geometry_morph: Rc::new(Cell::new(None)),
            graph_hover_suppressed: Rc::new(Cell::new(false)),
            list: ListState::new(0, ListAlignment::Top, px(HISTORY_ROW_HEIGHT * 5.0)),
            hovered_path: None,
            hovered_graph_path: None,
            hovered_row_path: None,
            graph_hover_active: false,
            graph_hover_clear_task: None,
            column_drag_anchor: None,
            column_drag: None,
            avatar_images: HashMap::new(),
            column_menu: Popup::default(),
            author_menu: Popup::default(),
            copied_sha: None,
            request_task: None,
            copy_task: None,
            fetching_all: false,
            fetch_for: None,
            fetch_error: None,
            fetch_task: None,
            _observe: observe,
        }
    }

    fn context(&self, cx: &App) -> Option<(String, String, Option<String>)> {
        let state = self.state.read(cx);
        let chat = state.selected_chat_row()?;
        let cwd = chat.cwd.clone()?;
        let target = (state.local_device_id.as_deref() != Some(chat.device_id.as_str()))
            .then(|| chat.device_id.clone());
        let key = format!("{}|{cwd}", target.as_deref().unwrap_or("local"));
        Some((key, cwd, target))
    }

    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        self.started = true;
        let Some((key, cwd, target)) = self.context(cx) else {
            self.fetch_task = None;
            self.fetching_all = false;
            self.fetch_for = None;
            self.fetch_error = None;
            self.target_key = None;
            self.commits.clear();
            self.visible_commits.clear();
            self.branch_tips.clear();
            self.collapsed_branches.clear();
            self.collapsed_counts.clear();
            self.total_count = None;
            self.head_commit_count = None;
            self.comparison = None;
            self.search_query.clear();
            self.search_results = None;
            self.search_next_cursor = None;
            self.search_total_count = None;
            self.search_loading = false;
            self.search_error = None;
            self.search_generation = self.search_generation.wrapping_add(1);
            self.search_scroll_anchor = None;
            self.search_task = None;
            self.graph = GraphLayout::default();
            self.graph_lane_capacity = 0;
            self.list.reset(0);
            self.hovered_path = None;
            self.hovered_graph_path = None;
            self.hovered_row_path = None;
            self.graph_hover_active = false;
            self.graph_hover_clear_task = None;
            self.avatar_images.clear();
            self.loading = false;
            return;
        };
        if self.target_key.as_deref() == Some(key.as_str()) {
            if !self.loading && self.commits.is_empty() && self.error.is_none() {
                self.fetch_page(key, cwd, target, 0, true, cx);
            }
            return;
        }
        self.request_task = None;
        self.fetch_task = None;
        self.fetching_all = false;
        self.fetch_for = None;
        self.fetch_error = None;
        self.loading = false;
        self.target_key = Some(key.clone());
        self.commits.clear();
        self.visible_commits.clear();
        self.branch_tips.clear();
        self.collapsed_branches.clear();
        self.collapsed_counts.clear();
        self.head_sha = None;
        self.next_cursor = None;
        self.total_count = None;
        self.head_commit_count = None;
        self.comparison = None;
        self.search_query.clear();
        self.search_results = None;
        self.search_next_cursor = None;
        self.search_total_count = None;
        self.search_loading = false;
        self.search_error = None;
        self.search_generation = self.search_generation.wrapping_add(1);
        self.search_scroll_anchor = None;
        self.search_task = None;
        self.error = None;
        self.graph = GraphLayout::default();
        self.graph_lane_capacity = 0;
        self.list.reset(0);
        self.hovered_path = None;
        self.hovered_graph_path = None;
        self.hovered_row_path = None;
        self.graph_hover_active = false;
        self.graph_hover_clear_task = None;
        self.avatar_images.clear();
        self.fetch_page(key, cwd, target, 0, true, cx);
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some((key, cwd, target)) = self.context(cx) else {
            return;
        };
        self.target_key = Some(key.clone());
        self.fetch_page(key, cwd, target, 0, true, cx);
    }

    pub fn fetch_all(&mut self, cx: &mut Context<Self>) {
        if self.fetching_all {
            return;
        }
        let Some((key, cwd, target)) = self.context(cx) else {
            return;
        };
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.fetching_all = true;
        self.fetch_for = Some(key.clone());
        self.fetch_error = None;
        cx.notify();
        self.fetch_task = Some(cx.spawn(async move |this, cx| {
            let mut params = serde_json::Map::new();
            params.insert("repoPath".into(), serde_json::Value::String(cwd.clone()));
            if let Some(target) = target.clone() {
                params.insert("targetDeviceId".into(), serde_json::Value::String(target));
            }
            let result = engine
                .client()
                .call(methods::FETCH_ALL, serde_json::Value::Object(params))
                .await;
            this.update(cx, |history, cx| {
                if history.fetch_for.as_deref() != Some(key.as_str()) {
                    return;
                }
                history.fetching_all = false;
                history.fetch_for = None;
                match result {
                    Ok(_) => {
                        history.fetch_error = None;
                        // Cancel a pre-fetch history request so the next page
                        // is guaranteed to observe the updated remote refs.
                        history.request_task = None;
                        history.loading = false;
                        history.fetch_page(key, cwd, target, 0, true, cx);
                        cx.emit(GitHistoryEvent::FetchSucceeded);
                    }
                    Err(error) => history.fetch_error = Some(error.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    pub fn commit_count(&self) -> Option<usize> {
        if self.search_active() {
            self.search_total_count.or(Some(self.visible_commits.len()))
        } else {
            self.head_commit_count
        }
    }

    fn recompute_view(&mut self) {
        if self.search_active() {
            let source = self.search_results.as_ref().unwrap_or(&self.commits);
            let visible: HashSet<_> = source
                .iter()
                .filter(|commit| git_history_matches(&self.search_query, commit))
                .map(|commit| commit.sha.clone())
                .collect();
            self.visible_commits = compact_commits_to_visible(source, &visible);
            self.collapsed_counts.clear();
            self.update_graph_layout();
            return;
        }
        match self.view_mode {
            GitHistoryViewMode::AllCommits => {
                let (visible, counts) = collapse_branch_runs(
                    &self.commits,
                    &self.collapsed_branches,
                    self.head_sha.as_deref(),
                );
                self.visible_commits = visible;
                self.collapsed_counts = counts;
            }
            GitHistoryViewMode::BranchTips => {
                // Immediate parents generally are not tips themselves. Clear
                // them rather than painting false dangling connections; the
                // branch badges carry the stable identity in this overview.
                self.visible_commits = self
                    .branch_tips
                    .iter()
                    .cloned()
                    .map(|mut commit| {
                        commit.parent_shas.clear();
                        commit
                    })
                    .collect();
                self.collapsed_counts.clear();
            }
        }
        self.update_graph_layout();
    }

    fn update_graph_layout(&mut self) {
        self.graph = layout_graph(&self.visible_commits, self.head_sha.as_deref());
        // The commit subject starts immediately after the graph column. Keep
        // that column at the widest lane count seen for this repository so a
        // fold cannot move every title sideways when its compact graph settles.
        let loaded_lane_count =
            layout_graph(&self.commits, self.head_sha.as_deref()).max_lane_count;
        self.graph_lane_capacity = self
            .graph_lane_capacity
            .max(self.graph.max_lane_count)
            .max(loaded_lane_count);
    }

    fn rebuild_view(&mut self, cx: &mut Context<Self>) {
        self.view_transition = None;
        self.view_transition_task = None;
        self.recompute_view();
        let item_count = self.visible_commits.len() + usize::from(self.has_load_more());
        self.list
            .reset_with_uniform_height(item_count, px(HISTORY_ROW_HEIGHT));
        self.hovered_path = None;
        self.hovered_graph_path = None;
        self.hovered_row_path = None;
        self.graph_hover_active = false;
        self.graph_hover_clear_task = None;
        cx.notify();
    }

    fn search_active(&self) -> bool {
        !self.search_query.trim().is_empty()
    }

    fn has_load_more(&self) -> bool {
        if self.search_active() {
            self.search_next_cursor.is_some()
        } else {
            self.view_mode == GitHistoryViewMode::AllCommits && self.next_cursor.is_some()
        }
    }

    fn current_scroll_anchor(&self) -> Option<HistoryScrollAnchor> {
        let scroll_top = self.list.logical_scroll_top();
        let commit = self
            .visible_commits
            .get(scroll_top.item_ix)
            .or_else(|| self.visible_commits.last())?;
        Some(HistoryScrollAnchor {
            sha: commit.sha.clone(),
            offset_in_item: if scroll_top.item_ix < self.visible_commits.len() {
                scroll_top.offset_in_item
            } else {
                px(0.0)
            },
        })
    }

    fn restore_scroll_anchor(&self, anchor: Option<&HistoryScrollAnchor>) {
        let Some(anchor) = anchor else {
            return;
        };
        let Some(item_ix) = self
            .visible_commits
            .iter()
            .position(|commit| commit.sha == anchor.sha)
        else {
            return;
        };
        self.list.scroll_to(ListOffset {
            item_ix,
            offset_in_item: anchor.offset_in_item,
        });
    }

    fn reconcile_list_items(
        &self,
        old: &[GitHistoryCommit],
        old_has_load_more: bool,
        target: &[GitHistoryCommit],
        target_has_load_more: bool,
    ) {
        if let Some((range, count)) =
            history_list_splice(old, old_has_load_more, target, target_has_load_more)
        {
            self.list.splice(range, count);
        }
    }

    fn settle_view_transition(&mut self, cx: &mut Context<Self>) {
        let Some(transition) = self.view_transition.take() else {
            return;
        };
        // Resolve at settle time rather than reusing the click-time anchor: if
        // the user scrolls during the tween, that newer position must win.
        let final_scroll_anchor = resolve_history_scroll_anchor(
            self.current_scroll_anchor(),
            &self.visible_commits,
            &transition.final_commits,
        );
        let old_has_load_more = self.list.item_count() > self.visible_commits.len();
        let final_has_load_more = self.has_load_more();
        self.reconcile_list_items(
            &self.visible_commits,
            old_has_load_more,
            &transition.final_commits,
            final_has_load_more,
        );
        self.visible_commits = transition.final_commits;
        self.collapsed_counts = transition.final_collapsed_counts;
        self.update_graph_layout();
        self.restore_scroll_anchor(final_scroll_anchor.as_ref());
        self.view_transition_task = None;
        cx.notify();
    }

    fn apply_view_change(&mut self, animate_rows: bool, cx: &mut Context<Self>) {
        // A second click during the short tween starts from the previous
        // destination, preventing zero-height transitional rows from leaking
        // into the next merge.
        if self.view_transition.is_some() {
            self.settle_view_transition(cx);
        }
        let scroll_anchor = self.current_scroll_anchor();
        let old_commits = self.visible_commits.clone();
        let old_has_load_more = self.list.item_count() > old_commits.len();
        self.recompute_view();
        let final_commits = std::mem::take(&mut self.visible_commits);
        let final_collapsed_counts = std::mem::take(&mut self.collapsed_counts);
        let final_scroll_anchor =
            resolve_history_scroll_anchor(scroll_anchor.clone(), &old_commits, &final_commits);
        let final_has_load_more = self.has_load_more();

        let unchanged = old_commits.len() == final_commits.len()
            && old_commits
                .iter()
                .zip(&final_commits)
                .all(|(old, new)| old.sha == new.sha);
        if unchanged || !animate_rows || crate::motion::reduced_motion(cx) {
            self.visible_commits = final_commits;
            self.collapsed_counts = final_collapsed_counts;
            self.update_graph_layout();
            self.reconcile_list_items(
                &old_commits,
                old_has_load_more,
                &self.visible_commits,
                final_has_load_more,
            );
            self.restore_scroll_anchor(final_scroll_anchor.as_ref());
            cx.notify();
            return;
        }

        let (transition_commits, rows) = history_transition_rows(&old_commits, &final_commits);
        self.reconcile_list_items(
            &old_commits,
            old_has_load_more,
            &transition_commits,
            final_has_load_more,
        );
        self.visible_commits = transition_commits;
        self.collapsed_counts = final_collapsed_counts.clone();
        self.update_graph_layout();
        self.restore_scroll_anchor(scroll_anchor.as_ref());
        let epoch = self.view_epoch;
        self.view_transition = Some(HistoryViewTransition {
            rows,
            final_commits,
            final_collapsed_counts,
            epoch,
        });
        let duration = crate::motion::COLLAPSE
            .total()
            .mul_f32(crate::motion::speed_scale());
        self.view_transition_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(duration).await;
            this.update(cx, |history, cx| {
                if history
                    .view_transition
                    .as_ref()
                    .is_some_and(|transition| transition.epoch == epoch)
                {
                    history.settle_view_transition(cx);
                }
            })
            .ok();
        }));
        self.hovered_path = None;
        self.hovered_graph_path = None;
        self.hovered_row_path = None;
        self.graph_hover_active = false;
        self.graph_hover_clear_task = None;
        cx.notify();
    }

    fn set_view_mode(&mut self, mode: GitHistoryViewMode, cx: &mut Context<Self>) {
        let cleared_individual =
            mode == GitHistoryViewMode::AllCommits && !self.collapsed_branches.is_empty();
        if cleared_individual {
            self.collapsed_branches.clear();
        }
        if self.view_mode == mode && !cleared_individual {
            return;
        }
        self.view_mode = mode;
        self.view_epoch = self.view_epoch.wrapping_add(1);
        self.apply_view_change(false, cx);
    }

    fn toggle_branch_ref(&mut self, reference: GitHistoryRef, cx: &mut Context<Self>) {
        let Some(key) = branch_ref_key(&reference) else {
            return;
        };
        self.view_mode = GitHistoryViewMode::AllCommits;
        self.view_epoch = self.view_epoch.wrapping_add(1);
        if !self.collapsed_branches.remove(&key) {
            self.collapsed_branches.insert(key);
        }
        self.apply_view_change(true, cx);
    }

    fn set_search_query(&mut self, query: String, cx: &mut Context<Self>) {
        let query = query.trim_start().to_string();
        if self.search_query == query {
            return;
        }
        let was_active = self.search_active();
        if !was_active && !query.trim().is_empty() {
            self.search_scroll_anchor = self.current_scroll_anchor();
        }
        self.search_generation = self.search_generation.wrapping_add(1);
        self.search_task = None;
        self.search_query = query;
        self.search_results = None;
        self.search_next_cursor = None;
        self.search_total_count = None;
        self.search_loading = false;
        self.search_error = None;

        if !self.search_active() {
            self.rebuild_view(cx);
            let anchor = self.search_scroll_anchor.take();
            self.restore_scroll_anchor(anchor.as_ref());
            cx.notify();
            return;
        }

        // The loaded page filters synchronously on the keystroke. The complete
        // repository search replaces it after a tiny debounce without changing
        // topological ordering.
        self.rebuild_view(cx);
        self.list.scroll_to(ListOffset {
            item_ix: 0,
            offset_in_item: px(0.0),
        });
        let query = self.search_query.clone();
        self.request_search_page(query, 0, true, HISTORY_SEARCH_DEBOUNCE, cx);
    }

    fn request_search_page(
        &mut self,
        query: String,
        cursor: usize,
        reset: bool,
        delay: Duration,
        cx: &mut Context<Self>,
    ) {
        let Some((key, cwd, target)) = self.context(cx) else {
            return;
        };
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let generation = self.search_generation;
        self.search_loading = true;
        self.search_error = None;
        cx.notify();
        self.search_task = Some(cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let mut params = serde_json::Map::new();
            params.insert("cwd".into(), serde_json::Value::String(cwd.clone()));
            params.insert("query".into(), serde_json::Value::String(query.clone()));
            params.insert("cursor".into(), serde_json::json!(cursor));
            params.insert("limit".into(), serde_json::json!(HISTORY_PAGE_SIZE));
            if let Some(target) = target.clone() {
                params.insert("targetDeviceId".into(), serde_json::Value::String(target));
            }
            let result = engine
                .client()
                .call(
                    methods::SEARCH_GIT_HISTORY,
                    serde_json::Value::Object(params),
                )
                .await;
            this.update(cx, |history, cx| {
                if history.target_key.as_deref() != Some(key.as_str())
                    || history.search_generation != generation
                    || history.search_query != query
                {
                    return;
                }
                history.search_loading = false;
                match result.and_then(|value| {
                    serde_json::from_value::<GitHistoryPage>(value)
                        .map_err(|error| zeron_rpc::RpcError::Failed(error.to_string()))
                }) {
                    Ok(page) => {
                        let anchor = history.current_scroll_anchor();
                        if reset {
                            history.search_results = Some(page.commits);
                        } else {
                            let results = history.search_results.get_or_insert_default();
                            let mut seen: HashSet<_> =
                                results.iter().map(|commit| commit.sha.clone()).collect();
                            results.extend(
                                page.commits
                                    .into_iter()
                                    .filter(|commit| seen.insert(commit.sha.clone())),
                            );
                        }
                        history.search_next_cursor = page.next_cursor;
                        history.search_total_count = page.total_count;
                        history.rebuild_view(cx);
                        history.restore_scroll_anchor(anchor.as_ref());
                        history.resolve_avatars(key, cwd, target, cursor, cx);
                    }
                    Err(error) => {
                        // Keep the instantaneous local matches useful when a
                        // remote/device search is temporarily unavailable.
                        history.search_error = Some(error.to_string().into());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn load_older(&mut self, cx: &mut Context<Self>) {
        if self.search_active() {
            if self.search_loading {
                return;
            }
            let Some(cursor) = self.search_next_cursor else {
                return;
            };
            self.request_search_page(self.search_query.clone(), cursor, false, Duration::ZERO, cx);
            return;
        }
        let Some(cursor) = self.next_cursor else {
            return;
        };
        let Some((key, cwd, target)) = self.context(cx) else {
            return;
        };
        self.fetch_page(key, cwd, target, cursor, false, cx);
    }

    fn resolve_avatars(
        &mut self,
        key: String,
        cwd: String,
        target: Option<String>,
        cursor: usize,
        cx: &mut Context<Self>,
    ) {
        if configured_author_display(cx) != GitHistoryAuthorDisplay::Avatar {
            return;
        }
        let mut unique_authors = HashMap::new();
        for commit in self
            .commits
            .iter()
            .chain(self.search_results.iter().flatten())
        {
            let email = commit.author_email.trim().to_ascii_lowercase();
            if !email.is_empty() {
                unique_authors
                    .entry(email)
                    .or_insert_with(|| (commit.sha.clone(), commit.author_email.clone()));
            }
        }
        let authors: Vec<_> = unique_authors
            .into_values()
            .map(|(sha, email)| serde_json::json!({ "sha": sha, "email": email }))
            .collect();
        if authors.is_empty() {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let mut params = serde_json::Map::new();
        params.insert("cwd".into(), serde_json::Value::String(cwd));
        params.insert("authors".into(), serde_json::Value::Array(authors));
        params.insert("cursor".into(), serde_json::json!(cursor));
        params.insert("limit".into(), serde_json::json!(HISTORY_PAGE_SIZE));
        if let Some(target) = target {
            params.insert("targetDeviceId".into(), serde_json::Value::String(target));
        }
        cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::RESOLVE_GIT_AVATARS,
                    serde_json::Value::Object(params),
                )
                .await;
            this.update(cx, |history, cx| {
                if history.target_key.as_deref() != Some(key.as_str()) {
                    return;
                }
                if let Ok(avatars) = result.and_then(|value| {
                    serde_json::from_value::<HashMap<String, String>>(value)
                        .map_err(|error| zeron_rpc::RpcError::Failed(error.to_string()))
                }) {
                    history.avatar_images.extend(avatars.into_iter().filter_map(
                        |(email, encoded)| {
                            decode_history_avatar(&encoded).map(|image| (email, image))
                        },
                    ));
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn resolve_loaded_avatars(&mut self, cx: &mut Context<Self>) {
        if self.commits.is_empty() {
            return;
        }
        let Some((key, cwd, target)) = self.context(cx) else {
            return;
        };
        for cursor in (0..self.commits.len()).step_by(HISTORY_PAGE_SIZE) {
            self.resolve_avatars(key.clone(), cwd.clone(), target.clone(), cursor, cx);
        }
    }

    fn fetch_page(
        &mut self,
        key: String,
        cwd: String,
        target: Option<String>,
        cursor: usize,
        reset: bool,
        cx: &mut Context<Self>,
    ) {
        if self.loading {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.loading = true;
        self.error = None;
        cx.notify();
        self.request_task = Some(cx.spawn(async move |this, cx| {
            let mut params = serde_json::Map::new();
            params.insert("cwd".into(), serde_json::Value::String(cwd.clone()));
            params.insert("cursor".into(), serde_json::json!(cursor));
            params.insert("limit".into(), serde_json::json!(HISTORY_PAGE_SIZE));
            if let Some(target) = target.clone() {
                params.insert("targetDeviceId".into(), serde_json::Value::String(target));
            }
            let result = engine
                .client()
                .call(methods::LIST_GIT_HISTORY, serde_json::Value::Object(params))
                .await;
            this.update(cx, |history, cx| {
                if history.target_key.as_deref() != Some(key.as_str()) {
                    return;
                }
                history.loading = false;
                match result.and_then(|value| {
                    serde_json::from_value::<GitHistoryPage>(value)
                        .map_err(|error| zeron_rpc::RpcError::Failed(error.to_string()))
                }) {
                    Ok(page) => {
                        let restart_search = (reset && history.search_active())
                            .then(|| history.search_query.clone());
                        if restart_search.is_some() {
                            history.search_generation = history.search_generation.wrapping_add(1);
                            history.search_task = None;
                            history.search_results = None;
                            history.search_next_cursor = None;
                            history.search_total_count = None;
                            history.search_loading = false;
                            history.search_error = None;
                        }
                        let old_visible_count = history.visible_commits.len();
                        let old_item_count =
                            old_visible_count + usize::from(history.has_load_more());
                        if reset {
                            history.commits = page.commits;
                            history.branch_tips = page.branch_tips;
                            history.total_count = page.total_count;
                            history.head_commit_count = page.head_commit_count;
                            history.comparison = page.comparison;
                        } else {
                            let mut seen: HashSet<String> = history
                                .commits
                                .iter()
                                .map(|commit| commit.sha.clone())
                                .collect();
                            history.commits.extend(
                                page.commits
                                    .into_iter()
                                    .filter(|commit| seen.insert(commit.sha.clone())),
                            );
                            if page.total_count.is_some() {
                                history.total_count = page.total_count;
                            }
                            if page.head_commit_count.is_some() {
                                history.head_commit_count = page.head_commit_count;
                            }
                        }
                        history.head_sha = page.head_sha;
                        history.next_cursor = page.next_cursor;
                        let incremental_all = !history.search_active()
                            && !reset
                            && history.view_mode == GitHistoryViewMode::AllCommits
                            && history.collapsed_branches.is_empty();
                        if incremental_all {
                            history.recompute_view();
                            let new_item_count = history.visible_commits.len()
                                + usize::from(history.next_cursor.is_some());
                            history.list.splice(
                                old_visible_count..old_item_count,
                                new_item_count - old_visible_count,
                            );
                        } else {
                            history.rebuild_view(cx);
                        }
                        history.resolve_avatars(
                            key.clone(),
                            cwd.clone(),
                            target.clone(),
                            cursor,
                            cx,
                        );
                        if let Some(query) = restart_search {
                            history.request_search_page(query, 0, true, Duration::ZERO, cx);
                        }
                    }
                    Err(error) => history.error = Some(error.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn copy_sha(&mut self, sha: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(sha.clone()));
        self.copied_sha = Some(sha);
        self.copy_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1_200))
                .await;
            this.update(cx, |history, cx| {
                history.copied_sha = None;
                cx.notify();
            })
            .ok();
        }));
    }

    fn graph_hover_key(cx: &Context<Self>) -> String {
        format!("history-graph-focus-{}", cx.entity_id())
    }

    fn graph_focus(&self, cx: &Context<Self>) -> Option<GraphFocus> {
        if self.graph_hover_suppressed.get() {
            return None;
        }
        self.hovered_path.map(|color_id| GraphFocus {
            color_id,
            amount: crate::motion::hover_t(&Self::graph_hover_key(cx)),
        })
    }

    fn set_history_hover(&mut self, path: Option<usize>, cx: &mut Context<Self>) {
        if let Some(path) = path {
            if self.graph_hover_active && self.hovered_path == Some(path) {
                return;
            }
            self.graph_hover_clear_task = None;
            self.graph_hover_active = true;
            self.hovered_path = Some(path);
            crate::motion::set_hover(
                &Self::graph_hover_key(cx),
                true,
                crate::motion::reduced_motion(cx),
            );
            cx.notify();
            return;
        }

        if !self.graph_hover_active {
            return;
        }
        self.graph_hover_active = false;
        let reduced_motion = crate::motion::reduced_motion(cx);
        crate::motion::set_hover(&Self::graph_hover_key(cx), false, reduced_motion);
        if reduced_motion {
            self.hovered_path = None;
            self.graph_hover_clear_task = None;
        } else {
            let fading_path = self.hovered_path;
            let duration = crate::motion::HOVER_FADE
                .total()
                .mul_f32(crate::motion::speed_scale());
            self.graph_hover_clear_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(duration).await;
                this.update(cx, |history, cx| {
                    if !history.graph_hover_active && history.hovered_path == fading_path {
                        history.hovered_path = None;
                        history.graph_hover_clear_task = None;
                        cx.notify();
                    }
                })
                .ok();
            }));
        }
        cx.notify();
    }

    fn set_graph_hover(&mut self, path: Option<usize>, cx: &mut Context<Self>) {
        self.hovered_graph_path = path;
        self.set_history_hover(path.or(self.hovered_row_path), cx);
    }

    fn set_row_hover(&mut self, path: Option<usize>, cx: &mut Context<Self>) {
        if path.is_some() && self.graph_hover_suppressed.replace(false) {
            cx.notify();
        }
        self.hovered_row_path = path;
        self.set_history_hover(self.hovered_graph_path.or(path), cx);
    }

    fn update_graph_hover(
        &mut self,
        row_index: usize,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self.graph_hover_suppressed.replace(false) {
            cx.notify();
        }
        let Some(bounds) = self.list.bounds_for_item(row_index) else {
            self.set_graph_hover(None, cx);
            return;
        };
        let Some(row) = self.graph.rows.get(row_index) else {
            self.set_graph_hover(None, cx);
            return;
        };
        let local_x = f32::from(position.x - bounds.left());
        let local_y = f32::from(position.y - bounds.top());
        self.set_graph_hover(
            hovered_graph_path(row, local_x, local_y, self.graph_geometry.get()),
            cx,
        );
    }

    fn toggle_column(&mut self, column: GitHistoryColumn, cx: &mut Context<Self>) {
        let (columns, widths, order) = {
            let preferences = cx.global_mut::<HistoryColumnPreferences>();
            match column {
                GitHistoryColumn::Author => {
                    preferences.columns.author = !preferences.columns.author
                }
                GitHistoryColumn::Date => preferences.columns.date = !preferences.columns.date,
                GitHistoryColumn::Sha => preferences.columns.sha = !preferences.columns.sha,
            }
            (
                preferences.columns,
                preferences.widths,
                preferences.order.clone(),
            )
        };
        Self::persist_column_layout(columns, widths, &order, SavePolicy::Immediate, cx);
        cx.refresh_windows();
        cx.notify();
    }

    fn reset_columns(&mut self, cx: &mut Context<Self>) {
        let columns = GitHistoryColumns::default();
        let widths = GitHistoryColumnWidths::default();
        let order = GitHistoryColumnOrder::default();
        {
            let preferences = cx.global_mut::<HistoryColumnPreferences>();
            preferences.columns = columns;
            preferences.widths = widths;
            preferences.order = order.clone();
        }
        Self::persist_column_layout(columns, widths, &order, SavePolicy::Immediate, cx);
        cx.refresh_windows();
        cx.notify();
    }

    fn persist_column_layout(
        columns: GitHistoryColumns,
        widths: GitHistoryColumnWidths,
        order: &GitHistoryColumnOrder,
        policy: SavePolicy,
        cx: &mut Context<Self>,
    ) {
        let order = order.clone();
        settings::update(policy, cx, move |settings| {
            settings.git_history_columns = columns;
            settings.git_history_column_widths = widths;
            settings.git_history_column_order = order;
        });
    }

    fn schedule_column_layout_save(&mut self, cx: &mut Context<Self>) {
        let (columns, widths, order) = {
            let preferences = cx.global::<HistoryColumnPreferences>();
            (
                preferences.columns,
                preferences.widths,
                preferences.order.clone(),
            )
        };
        Self::persist_column_layout(columns, widths, &order, SavePolicy::Debounced, cx);
    }

    fn begin_column_resize(
        &mut self,
        left: HistoryDataColumn,
        right: HistoryDataColumn,
        start_x: f32,
        cx: &mut Context<Self>,
    ) {
        let widths = configured_column_widths(cx);
        self.column_drag_anchor = Some(HistoryColumnDragAnchor {
            start_x,
            left,
            right,
            left_width: history_column_width(left, widths),
            right_width: history_column_width(right, widths),
        });
    }

    fn on_column_resize(
        &mut self,
        event: &gpui::DragMoveEvent<HistoryColumnResize>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(anchor) = self.column_drag_anchor else {
            return;
        };
        let requested_delta = f32::from(event.event.position.x) - anchor.start_x;
        // Commit owns the flexible remainder. Interior dividers instead
        // preserve their pair's total width, so no drag creates overflow.
        let widths =
            resized_history_column_widths(configured_column_widths(cx), anchor, requested_delta);

        cx.global_mut::<HistoryColumnPreferences>().widths = widths;
        self.schedule_column_layout_save(cx);
        cx.refresh_windows();
        cx.notify();
    }

    fn reset_column_widths(&mut self, cx: &mut Context<Self>) {
        cx.global_mut::<HistoryColumnPreferences>().widths = GitHistoryColumnWidths::default();
        self.column_drag_anchor = None;
        self.schedule_column_layout_save(cx);
        cx.refresh_windows();
        cx.notify();
    }

    fn column_resize_handle(
        &self,
        left: HistoryDataColumn,
        right: HistoryDataColumn,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let hover = theme.border_strong;
        div()
            .id(SharedString::from(format!(
                "history-resize-{left:?}-{right:?}"
            )))
            .absolute()
            .left(px(-3.0))
            .top_0()
            .bottom_0()
            .w(px(6.0))
            .cursor_col_resize()
            .hover(move |style| style.bg(hover.opacity(0.7)))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                    this.begin_column_resize(left, right, f32::from(event.position.x), cx);
                }),
            )
            .on_drag(
                HistoryColumnResize,
                |_, _point: gpui::Point<gpui::Pixels>, _, cx| {
                    cx.stop_propagation();
                    cx.new(|_| HistoryResizeGhost)
                },
            )
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseUpEvent, _, cx| {
                    if event.click_count == 2 {
                        this.reset_column_widths(cx);
                    } else {
                        this.column_drag_anchor = None;
                    }
                }),
            )
            .into_any_element()
    }

    fn render_column_header_cell(
        &self,
        column: GitHistoryColumn,
        left: HistoryDataColumn,
        index: usize,
        drag: Option<HistoryColumnDragState>,
        widths: GitHistoryColumnWidths,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let data_column = history_data_column(column);
        let width = history_optional_width(column, widths);
        let (min_width, _) = history_column_limits(data_column);
        let label = history_column_label(column);
        let id = match column {
            GitHistoryColumn::Author => "history-author-header",
            GitHistoryColumn::Date => "history-date-header",
            GitHistoryColumn::Sha => "history-sha-header",
        };
        let resize = self.column_resize_handle(left, data_column, theme, cx);
        let indicator = drag
            .filter(|state| state.over == index && state.from != state.over)
            .map(|state| {
                let place_after = state.from < state.over;
                div()
                    .absolute()
                    .top(px(3.0))
                    .bottom(px(3.0))
                    .w(px(2.0))
                    .rounded_full()
                    .bg(theme.accent)
                    .when(place_after, |line| line.right_0())
                    .when(!place_after, |line| line.left_0())
            });
        let ghost_label: SharedString = label.into();
        div()
            .id(id)
            .relative()
            .w(px(width))
            .min_w(px(min_width))
            .h_full()
            .flex_shrink(1.0)
            .flex()
            .items_center()
            .cursor_pointer()
            .when(column == GitHistoryColumn::Author, |header| {
                header.justify_center().on_mouse_down(
                    gpui::MouseButton::Right,
                    cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                        window.prevent_default();
                        cx.stop_propagation();
                        this.open_author_menu(event.position, cx);
                    }),
                )
            })
            .on_drag(
                HistoryColumnDrag {
                    column,
                    label: ghost_label,
                },
                |payload, _point, _, cx| {
                    cx.stop_propagation();
                    cx.new(|_| HistoryColumnGhost {
                        label: payload.label.clone(),
                    })
                },
            )
            .children(indicator)
            .child(label)
            // Paint last so this narrow hitbox wins over the header drag.
            .child(resize)
            .into_any_element()
    }

    fn on_column_drag_move(
        &mut self,
        event: &gpui::DragMoveEvent<HistoryColumnDrag>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let payload = event.drag(cx);
        let columns = configured_columns(cx);
        let order = configured_column_order(cx);
        let visible = visible_history_columns(&order, columns);
        let Some(from) = visible.iter().position(|column| *column == payload.column) else {
            return;
        };
        let relative_x = f32::from(event.event.position.x) - f32::from(event.bounds.left());
        let over = history_column_drop_index(
            relative_x,
            f32::from(event.bounds.size.width),
            &visible,
            configured_column_widths(cx),
        );
        if self
            .column_drag
            .is_some_and(|state| state.from == from && state.over == over)
        {
            return;
        }
        self.column_drag = Some(HistoryColumnDragState { from, over });
        cx.notify();
    }

    fn commit_column_reorder(&mut self, dragged: GitHistoryColumn, cx: &mut Context<Self>) {
        let columns = configured_columns(cx);
        let current = configured_column_order(cx);
        let visible = visible_history_columns(&current, columns);
        let target = self
            .column_drag
            .and_then(|state| visible.get(state.over).copied())
            .unwrap_or(dragged);
        let reordered = reordered_history_columns(&current, dragged, target);
        self.column_drag = None;
        if reordered == current {
            cx.notify();
            return;
        }
        cx.global_mut::<HistoryColumnPreferences>().order = reordered;
        self.schedule_column_layout_save(cx);
        cx.refresh_windows();
        cx.notify();
    }

    fn open_column_menu(&mut self, position: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        self.close_author_menu(cx);
        self.column_menu.open(position);
        cx.notify();
    }

    fn close_column_menu(&mut self, cx: &mut Context<Self>) {
        if self.column_menu.begin_close() {
            popover::reap_popup(cx, |history: &mut Self| &mut history.column_menu);
        }
    }

    fn open_author_menu(&mut self, position: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        self.close_column_menu(cx);
        self.author_menu.open(position);
        cx.notify();
    }

    fn close_author_menu(&mut self, cx: &mut Context<Self>) {
        if self.author_menu.begin_close() {
            popover::reap_popup(cx, |history: &mut Self| &mut history.author_menu);
        }
    }

    fn toggle_author_display(&mut self, cx: &mut Context<Self>) {
        let display = {
            let preferences = cx.global_mut::<HistoryColumnPreferences>();
            preferences.author_display = match preferences.author_display {
                GitHistoryAuthorDisplay::Avatar => GitHistoryAuthorDisplay::Name,
                GitHistoryAuthorDisplay::Name => GitHistoryAuthorDisplay::Avatar,
            };
            preferences.author_display
        };
        settings::update(SavePolicy::Immediate, cx, |settings| {
            settings.git_history_author_display = display;
        });
        if display == GitHistoryAuthorDisplay::Avatar {
            self.resolve_loaded_avatars(cx);
        }
        cx.refresh_windows();
        cx.notify();
    }

    fn render_author_menu(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let theme = &theme.for_popup();
        let show_name = configured_author_display(cx) == GitHistoryAuthorDisplay::Name;
        popover::popover_card(theme)
            .w(px(116.0))
            .p(px(popover::CARD_INSET))
            .rounded(px(9.0))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_author_menu(cx)))
            .child(
                popover::menu_row(theme, false, "history-author-display-name")
                    .id("history-author-display-name")
                    .gap(px(0.0))
                    .px(px(7.0))
                    .py(px(4.0))
                    .rounded(px(9.0 - popover::CARD_INSET))
                    .text_size(px(11.5))
                    .on_click(cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_author_display(cx);
                        this.close_author_menu(cx);
                    }))
                    .child(div().flex_1().child("Name"))
                    .child(div().w(px(12.0)).flex_none().flex().justify_end().when(
                        show_name,
                        |element| {
                            element.child(
                                crate::icons::icon(crate::icons::CHECK)
                                    .size(px(10.0))
                                    .text_color(theme.text_muted),
                            )
                        },
                    )),
            )
            .into_any_element()
    }

    fn render_column_menu(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let theme = &theme.for_popup();
        let columns = configured_columns(cx);
        let widths = configured_column_widths(cx);
        let order = configured_column_order(cx);
        let option =
            |label: &'static str,
             checked: bool,
             column: GitHistoryColumn,
             index: usize,
             cx: &mut Context<Self>| {
                popover::menu_row(
                    theme,
                    false,
                    SharedString::from(format!("history-column-option-{index}")),
                )
                .id(("history-column-option", index))
                .gap(px(0.0))
                .px(px(7.0))
                .py(px(4.0))
                .rounded(px(9.0 - popover::CARD_INSET))
                .text_size(px(11.5))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.toggle_column(column, cx);
                }))
                .child(div().flex_1().child(label))
                .child(div().w(px(12.0)).flex_none().flex().justify_end().when(
                    checked,
                    |element| {
                        element.child(
                            crate::icons::icon(crate::icons::CHECK)
                                .size(px(10.0))
                                .text_color(theme.text_muted),
                        )
                    },
                ))
            };
        let defaults = GitHistoryColumns::default();
        let can_reset = columns != defaults
            || widths != GitHistoryColumnWidths::default()
            || order != GitHistoryColumnOrder::default();

        popover::popover_card(theme)
            .w(px(132.0))
            .p(px(popover::CARD_INSET))
            .rounded(px(9.0))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_column_menu(cx)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(popover::MENU_GAP))
                    .child(option(
                        "Author",
                        columns.author,
                        GitHistoryColumn::Author,
                        0,
                        cx,
                    ))
                    .child(option("Date", columns.date, GitHistoryColumn::Date, 1, cx))
                    .child(option("SHA", columns.sha, GitHistoryColumn::Sha, 2, cx))
                    .when(can_reset, |menu| {
                        menu.child(
                            div()
                                .h(px(1.0))
                                .mx(px(5.0))
                                .my(px(2.0))
                                .bg(crate::theme::hairline(0.08)),
                        )
                        .child(
                            popover::menu_row(theme, false, "history-columns-reset")
                                .id("history-columns-reset")
                                .px(px(7.0))
                                .py(px(4.0))
                                .rounded(px(9.0 - popover::CARD_INSET))
                                .text_size(px(11.5))
                                .text_color(theme.text_muted)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.reset_columns(cx);
                                    this.close_column_menu(cx);
                                }))
                                .child("Reset"),
                        )
                    }),
            )
            .into_any_element()
    }
}
mod render;

#[cfg(test)]
mod tests;
