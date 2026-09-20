//! Git history topology, responsive graph geometry, and hit testing.

use super::*;

#[derive(Debug, Clone)]
struct ActiveLane {
    id: usize,
    color_id: usize,
    target_sha: String,
}

fn lane_index(lanes: &[ActiveLane], id: usize) -> usize {
    lanes
        .iter()
        .position(|lane| lane.id == id)
        .expect("active Git history lane exists")
}

/// Commits arrive child-before-parent from `git log --topo-order`. Active
/// lanes point at the parent commit that will eventually resolve each path.
pub(super) fn layout_graph(commits: &[GitHistoryCommit], head_sha: Option<&str>) -> GraphLayout {
    let mut active_lanes: Vec<ActiveLane> = Vec::new();
    let mut next_lane_id = 0usize;
    let mut next_color_id = 0usize;
    let mut max_lane_count = 0usize;
    let mut rows = Vec::with_capacity(commits.len());

    for commit in commits {
        let before = active_lanes.clone();
        let incoming: Vec<usize> = before
            .iter()
            .enumerate()
            .filter_map(|(index, lane)| (lane.target_sha == commit.sha).then_some(index))
            .collect();
        let primary_incoming = incoming.first().copied();
        let node_lane = primary_incoming.unwrap_or(before.len());
        let primary_lane = primary_incoming.and_then(|index| before.get(index));
        let node_color_id = primary_lane.map(|lane| lane.color_id).unwrap_or_else(|| {
            let color = next_color_id;
            next_color_id += 1;
            color
        });
        let resolved_ids: HashSet<usize> = incoming.iter().map(|&index| before[index].id).collect();
        let mut next_lanes: Vec<ActiveLane> = before
            .iter()
            .filter(|lane| !resolved_ids.contains(&lane.id))
            .cloned()
            .collect();
        let mut outgoing: Vec<(usize, usize)> = Vec::new();

        let mut primary_outgoing_id = None;
        if let Some(first_parent) = commit.parent_shas.first() {
            let id = primary_lane.map(|lane| lane.id).unwrap_or_else(|| {
                let id = next_lane_id;
                next_lane_id += 1;
                id
            });
            let lane = ActiveLane {
                id,
                color_id: node_color_id,
                target_sha: first_parent.clone(),
            };
            next_lanes.insert(node_lane.min(next_lanes.len()), lane.clone());
            primary_outgoing_id = Some(id);
            outgoing.push((id, lane.color_id));
        }

        let mut parent_offset = 1usize;
        for parent_sha in commit.parent_shas.iter().skip(1) {
            if let Some(existing) = next_lanes
                .iter()
                .find(|lane| lane.target_sha == *parent_sha)
            {
                outgoing.push((existing.id, existing.color_id));
                continue;
            }
            let lane = ActiveLane {
                id: next_lane_id,
                color_id: next_color_id,
                target_sha: parent_sha.clone(),
            };
            next_lane_id += 1;
            next_color_id += 1;
            let primary_index = primary_outgoing_id
                .map(|id| lane_index(&next_lanes, id))
                .unwrap_or_else(|| node_lane.min(next_lanes.len()));
            next_lanes.insert(
                (primary_index + parent_offset).min(next_lanes.len()),
                lane.clone(),
            );
            parent_offset += 1;
            outgoing.push((lane.id, lane.color_id));
        }

        let mut segments: Vec<GraphSegment> = before
            .iter()
            .enumerate()
            .filter(|(_, lane)| !resolved_ids.contains(&lane.id))
            .map(|(from_lane, lane)| GraphSegment {
                from_lane,
                to_lane: lane_index(&next_lanes, lane.id),
                color_id: lane.color_id,
                shape: SegmentShape::Through,
            })
            .collect();
        segments.extend(incoming.iter().map(|&from_lane| GraphSegment {
            from_lane,
            to_lane: node_lane,
            color_id: before[from_lane].color_id,
            shape: SegmentShape::Incoming,
        }));
        segments.extend(outgoing.into_iter().map(|(id, color_id)| GraphSegment {
            from_lane: node_lane,
            to_lane: lane_index(&next_lanes, id),
            color_id,
            shape: SegmentShape::Outgoing,
        }));

        max_lane_count = max_lane_count
            .max(before.len())
            .max(next_lanes.len())
            .max(node_lane + 1);
        rows.push(GraphRow {
            sha: commit.sha.clone(),
            node_lane,
            node_color_id,
            segments,
            is_head: head_sha == Some(commit.sha.as_str()),
        });
        active_lanes = next_lanes;
    }

    GraphLayout {
        rows,
        max_lane_count,
    }
}

/// Remove the linear portions of selected branch lanes while retaining refs,
/// roots, merges and branch points. Parents that cross a hidden run are
/// contracted to the nearest visible ancestors so the compact graph never
/// paints a line toward a row that no longer exists.
pub(super) fn collapse_branch_runs(
    commits: &[GitHistoryCommit],
    collapsed_refs: &HashSet<String>,
    head_sha: Option<&str>,
) -> (Vec<GitHistoryCommit>, HashMap<String, usize>) {
    if collapsed_refs.is_empty() || commits.is_empty() {
        return (commits.to_vec(), HashMap::new());
    }

    let source_graph = layout_graph(commits, head_sha);
    let mut colors_by_ref = HashMap::new();
    for (commit, row) in commits.iter().zip(&source_graph.rows) {
        for reference in &commit.refs {
            if let Some(key) = branch_ref_key(reference)
                && collapsed_refs.contains(&key)
            {
                colors_by_ref.insert(key, row.node_color_id);
            }
        }
    }
    if colors_by_ref.is_empty() {
        return (commits.to_vec(), HashMap::new());
    }

    let collapsed_colors: HashSet<_> = colors_by_ref.values().copied().collect();
    let mut child_counts: HashMap<&str, usize> = HashMap::new();
    for commit in commits {
        for parent in &commit.parent_shas {
            *child_counts.entry(parent.as_str()).or_default() += 1;
        }
    }

    let mut visible = HashSet::new();
    let mut hidden_counts = HashMap::new();
    for (commit, row) in commits.iter().zip(&source_graph.rows) {
        let is_selected_tip = commit.refs.iter().any(|reference| {
            branch_ref_key(reference).is_some_and(|key| collapsed_refs.contains(&key))
        });
        let is_junction = commit.parent_shas.len() != 1
            || child_counts
                .get(commit.sha.as_str())
                .copied()
                .unwrap_or_default()
                > 1;
        let hide = collapsed_colors.contains(&row.node_color_id)
            && !is_selected_tip
            && commit.refs.is_empty()
            && !is_junction;
        if hide {
            for (key, color) in &colors_by_ref {
                if *color == row.node_color_id {
                    *hidden_counts.entry(key.clone()).or_default() += 1;
                }
            }
        } else {
            visible.insert(commit.sha.clone());
        }
    }

    (compact_commits_to_visible(commits, &visible), hidden_counts)
}

/// Retain a subset in its original topological order and contract parent
/// edges across omitted commits. Search uses this to keep a coherent graph
/// without pretending that a fuzzy match is a new ancestry boundary.
pub(super) fn compact_commits_to_visible(
    commits: &[GitHistoryCommit],
    visible: &HashSet<String>,
) -> Vec<GitHistoryCommit> {
    let by_sha: HashMap<_, _> = commits
        .iter()
        .map(|commit| (commit.sha.as_str(), commit))
        .collect();

    fn nearest_visible_parents(
        sha: &str,
        visible: &HashSet<String>,
        by_sha: &HashMap<&str, &GitHistoryCommit>,
        memo: &mut HashMap<String, Vec<String>>,
    ) -> Vec<String> {
        if visible.contains(sha) || !by_sha.contains_key(sha) {
            return vec![sha.to_string()];
        }
        if let Some(cached) = memo.get(sha) {
            return cached.clone();
        }

        struct Frame {
            sha: String,
            next_parent: usize,
            resolved: Vec<String>,
            seen: HashSet<String>,
        }

        impl Frame {
            fn new(sha: String) -> Self {
                Self {
                    sha,
                    next_parent: 0,
                    resolved: Vec::new(),
                    seen: HashSet::new(),
                }
            }

            fn extend(&mut self, parents: &[String]) {
                for parent in parents {
                    if self.seen.insert(parent.clone()) {
                        self.resolved.push(parent.clone());
                    }
                }
            }
        }

        let mut visiting = HashSet::from([sha.to_string()]);
        let mut stack = vec![Frame::new(sha.to_string())];
        loop {
            let next_parent = {
                let frame = stack.last_mut().expect("history traversal frame");
                let parents = &by_sha[frame.sha.as_str()].parent_shas;
                (frame.next_parent < parents.len()).then(|| {
                    let parent = parents[frame.next_parent].clone();
                    frame.next_parent += 1;
                    parent
                })
            };

            let Some(parent) = next_parent else {
                let frame = stack.pop().expect("history traversal frame");
                visiting.remove(&frame.sha);
                let resolved = frame.resolved;
                memo.insert(frame.sha, resolved.clone());
                if let Some(caller) = stack.last_mut() {
                    caller.extend(&resolved);
                    continue;
                }
                return resolved;
            };

            let resolved = if visible.contains(&parent) || !by_sha.contains_key(parent.as_str()) {
                Some(vec![parent.clone()])
            } else if let Some(cached) = memo.get(&parent) {
                Some(cached.clone())
            } else if visiting.contains(&parent) {
                Some(Vec::new())
            } else {
                None
            };

            if let Some(resolved) = resolved {
                stack
                    .last_mut()
                    .expect("history traversal frame")
                    .extend(&resolved);
            } else {
                visiting.insert(parent.clone());
                stack.push(Frame::new(parent));
            }
        }
    }

    let mut memo = HashMap::new();
    commits
        .iter()
        .filter(|commit| visible.contains(&commit.sha))
        .cloned()
        .map(|mut commit| {
            let mut seen = HashSet::new();
            commit.parent_shas = commit
                .parent_shas
                .iter()
                .flat_map(|parent| nearest_visible_parents(parent, visible, &by_sha, &mut memo))
                .filter(|parent| seen.insert(parent.clone()))
                .collect();
            commit
        })
        .collect()
}

pub(super) fn responsive_graph_geometry(
    lane_count: usize,
    container_width: f32,
    optional_columns_width: f32,
) -> GraphGeometry {
    let natural = GraphGeometry::natural(lane_count);
    if natural.width <= HISTORY_GRAPH_MIN_COMPACT_WIDTH {
        return natural;
    }

    // Commit subjects remain the primary content. Give the graph at most a
    // third of the surface, and never consume the subject's minimum width or
    // the configured metadata columns when the pane becomes narrow.
    let content_budget =
        (container_width - optional_columns_width - HISTORY_COMMIT_SUBJECT_MIN_WIDTH)
            .max(HISTORY_GRAPH_MIN_COMPACT_WIDTH);
    let share_budget =
        (container_width * HISTORY_GRAPH_MAX_WIDTH_RATIO).max(HISTORY_GRAPH_MIN_COMPACT_WIDTH);
    GraphGeometry::fitted(
        lane_count,
        natural.width.min(content_budget.min(share_budget)),
    )
}

pub(super) fn should_use_compact_graph(
    target: GraphGeometry,
    previous: GraphGeometry,
    container_width: f32,
    optional_columns_width: f32,
) -> bool {
    if target.lane_count <= 1 {
        return false;
    }
    let subject_width = container_width - optional_columns_width - target.width;
    if previous.compact {
        subject_width < HISTORY_GRAPH_COMPACT_EXIT_SUBJECT_WIDTH
            || target.lane_spacing < HISTORY_GRAPH_COMPACT_EXIT_LANE_SPACING
    } else {
        subject_width < HISTORY_GRAPH_COMPACT_ENTER_SUBJECT_WIDTH
            || target.lane_spacing < HISTORY_GRAPH_COMPACT_ENTER_LANE_SPACING
    }
}

pub(super) fn stabilized_graph_geometry(
    target: GraphGeometry,
    previous: GraphGeometry,
    device_scale: f32,
    compact: bool,
) -> GraphGeometry {
    if compact {
        let mut compact_geometry = GraphGeometry::compact(target.lane_count);
        compact_geometry.device_scale = device_scale.max(1.0);
        return compact_geometry;
    }
    let natural = GraphGeometry::natural(target.lane_count);
    let target_is_natural = (target.width - natural.width).abs() < f32::EPSILON;
    let snapped_width = if target_is_natural {
        natural.width
    } else {
        (target.width / HISTORY_GRAPH_RESIZE_STEP).floor() * HISTORY_GRAPH_RESIZE_STEP
    };
    let mut next = GraphGeometry::fitted(target.lane_count, snapped_width);
    next.device_scale = device_scale.max(1.0);

    if !target_is_natural
        && previous.lane_count == next.lane_count
        && (previous.width - next.width).abs() < HISTORY_GRAPH_RESIZE_STEP
    {
        // Small width oscillations are common while dragging a GPUI split.
        // Keep the last geometry until a complete step has accumulated.
        let mut stable = previous;
        stable.device_scale = next.device_scale;
        stable
    } else {
        next
    }
}

fn cubic_coordinate(start: f32, control_1: f32, control_2: f32, end: f32, t: f32) -> f32 {
    let inverse = 1.0 - t;
    inverse.powi(3) * start
        + 3.0 * inverse.powi(2) * t * control_1
        + 3.0 * inverse * t.powi(2) * control_2
        + t.powi(3) * end
}

fn point_to_segment_distance(
    point_x: f32,
    point_y: f32,
    start_x: f32,
    start_y: f32,
    end_x: f32,
    end_y: f32,
) -> f32 {
    let delta_x = end_x - start_x;
    let delta_y = end_y - start_y;
    let length_squared = delta_x * delta_x + delta_y * delta_y;
    if length_squared <= f32::EPSILON {
        return ((point_x - start_x).powi(2) + (point_y - start_y).powi(2)).sqrt();
    }
    let projection = (((point_x - start_x) * delta_x + (point_y - start_y) * delta_y)
        / length_squared)
        .clamp(0.0, 1.0);
    let closest_x = start_x + projection * delta_x;
    let closest_y = start_y + projection * delta_y;
    ((point_x - closest_x).powi(2) + (point_y - closest_y).powi(2)).sqrt()
}

fn segment_distance(
    segment: &GraphSegment,
    point_x: f32,
    point_y: f32,
    geometry: GraphGeometry,
) -> f32 {
    let middle = HISTORY_ROW_HEIGHT / 2.0;
    let from_x = geometry.lane_x(segment.from_lane);
    let to_x = geometry.lane_x(segment.to_lane);
    let (start_y, end_y, control_1_y, control_2_y) = match segment.shape {
        SegmentShape::Incoming => (
            -HISTORY_GRAPH_ROW_OVERLAP,
            middle,
            middle * 0.55,
            middle * 0.55,
        ),
        SegmentShape::Outgoing => (
            middle,
            HISTORY_ROW_HEIGHT + HISTORY_GRAPH_ROW_OVERLAP,
            middle * 1.45,
            middle * 1.45,
        ),
        SegmentShape::Through => (
            -HISTORY_GRAPH_ROW_OVERLAP,
            HISTORY_ROW_HEIGHT + HISTORY_GRAPH_ROW_OVERLAP,
            middle,
            middle,
        ),
    };
    if segment.shape == SegmentShape::Through && segment.from_lane == segment.to_lane {
        return point_to_segment_distance(point_x, point_y, from_x, start_y, to_x, end_y);
    }

    // A short polyline approximation is enough for pointer hit testing and
    // keeps the interactive target in lockstep with the painted Bezier.
    const SAMPLES: usize = 10;
    let mut closest = f32::MAX;
    let mut previous_x = from_x;
    let mut previous_y = start_y;
    for sample in 1..=SAMPLES {
        let t = sample as f32 / SAMPLES as f32;
        let current_x = cubic_coordinate(from_x, from_x, to_x, to_x, t);
        let current_y = cubic_coordinate(start_y, control_1_y, control_2_y, end_y, t);
        closest = closest.min(point_to_segment_distance(
            point_x, point_y, previous_x, previous_y, current_x, current_y,
        ));
        previous_x = current_x;
        previous_y = current_y;
    }
    closest
}

pub(super) fn hovered_graph_path(
    row: &GraphRow,
    point_x: f32,
    point_y: f32,
    geometry: GraphGeometry,
) -> Option<usize> {
    let node_x = geometry.lane_x(row.node_lane);
    let node_y = HISTORY_ROW_HEIGHT / 2.0;
    let node_distance = ((point_x - node_x).powi(2) + (point_y - node_y).powi(2)).sqrt();
    if node_distance <= HISTORY_GRAPH_HIT_RADIUS + HISTORY_NODE_RADIUS {
        return Some(row.node_color_id);
    }

    if geometry.compact && (point_x - geometry.lane_x(0)).abs() <= HISTORY_GRAPH_HIT_RADIUS {
        return Some(row.node_color_id);
    }

    row.segments
        .iter()
        .map(|segment| {
            (
                segment.color_id,
                segment_distance(segment, point_x, point_y, geometry),
            )
        })
        .filter(|(_, distance)| *distance <= HISTORY_GRAPH_HIT_RADIUS)
        .min_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(color_id, _)| color_id)
}
