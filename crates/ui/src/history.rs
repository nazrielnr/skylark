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
use skylark_engine::repos::git_history_matches;
use skylark_proto::{
    GitHistoryCommit, GitHistoryComparison, GitHistoryPage, GitHistoryRef, GitHistoryRefKind,
};
use skylark_rpc::methods;

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

#[path = "history/column_layout.rs"]
mod column_layout;
#[path = "history/column_types.rs"]
mod column_types;
pub(crate) use column_layout::*;
use column_types::*;
pub use column_types::{
    configured_author_display, configured_column_order, configured_column_widths,
    configured_columns, init,
};

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

#[path = "history/controls.rs"]
mod controls;

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
}

#[path = "history/data.rs"]
mod data;

impl GitHistory {
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
}

#[path = "history/columns.rs"]
mod columns;
mod render;
#[path = "history/rows_render.rs"]
mod rows_render;
pub(crate) use rows_render::*;

impl Render for GitHistory {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        GitHistory::render(self, window, cx)
    }
}
use rows_render::*;

#[cfg(test)]
mod tests;
