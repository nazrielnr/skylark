//! Git history tests.

use super::*;
use super::*;

struct HistorySearchFocusHarness {
    composer: Entity<ComposerInput>,
    search: Entity<GitHistorySearchControl>,
    _focus_lost: Subscription,
}

impl HistorySearchFocusHarness {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = cx.new(|_| AppState::new());
        let history = cx.new(|cx| GitHistory::new(state, cx));
        let search = cx.new(|cx| GitHistorySearchControl::new(history, cx));
        let composer = cx.new(|cx| ComposerInput::new("Message", cx));
        let focus_lost = cx.on_focus_lost(window, |this, window, cx| {
            crate::shell::restore_focus_if_empty_on_next_frame(
                this.composer.read(cx).focus_handle(cx),
                window,
                cx,
            );
        });
        Self {
            composer,
            search,
            _focus_lost: focus_lost,
        }
    }
}

impl Render for HistorySearchFocusHarness {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(div().h(px(40.0)).child(self.composer.clone()))
            .child(self.search.clone())
    }
}

fn commit(sha: &str, parents: &[&str]) -> GitHistoryCommit {
    GitHistoryCommit {
        sha: sha.into(),
        parent_shas: parents.iter().map(|value| (*value).into()).collect(),
        subject: sha.into(),
        author_name: "Test".into(),
        author_email: "test@example.com".into(),
        authored_at: "2026-08-12T12:00:00Z".into(),
        refs: Vec::new(),
    }
}

fn with_branch(
    mut commit: GitHistoryCommit,
    kind: GitHistoryRefKind,
    label: &str,
) -> GitHistoryCommit {
    commit.refs.push(GitHistoryRef {
        kind,
        label: label.into(),
    });
    commit
}

#[test]
fn history_search_matches_fuzzy_subject_terms_and_sha_prefix() {
    let mut candidate = commit("a1b2c3d4", &[]);
    candidate.subject = "Polish the history graph".into();

    assert!(git_history_matches("plsh grph", &candidate));
    assert!(git_history_matches("A1B2", &candidate));
    assert!(!git_history_matches("terminal", &candidate));
}

#[test]
fn history_search_keeps_unicode_engine_result_visible() {
    let mut app = gpui::TestApp::new();

    app.update(|cx| {
        let state = cx.new(|_| AppState::new());
        let history = cx.new(|cx| GitHistory::new(state, cx));
        let mut candidate = commit("a1b2c3d4", &[]);
        candidate.subject = "RÉPARER la recherche".into();

        // Simulate the page returned by the engine for this query. The UI
        // must not discard a result that the shared matcher accepted.
        assert!(git_history_matches("réparer", &candidate));
        history.update(cx, |history, _cx| {
            history.search_query = "réparer".into();
            history.search_results = Some(vec![candidate]);
            history.recompute_view();

            assert_eq!(history.visible_commits.len(), 1);
            assert_eq!(history.visible_commits[0].subject, "RÉPARER la recherche");
        });
    });
}

#[test]
fn history_search_focus_survives_the_composer_fallback() {
    let mut app = gpui::TestApp::new();
    app.update(|cx| {
        Theme::install(crate::theme::Appearance::Dark, cx);
        crate::composer::init(cx, crate::settings::ComposerSendBehavior::default());
    });
    let mut window = app.open_window(HistorySearchFocusHarness::new);
    window.draw();

    window.update(|harness, window, cx| {
        window.focus(&harness.composer.read(cx).focus_handle(cx), cx);
    });
    window.draw();
    window.update(|harness, window, cx| {
        assert!(
            harness
                .composer
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        );
        harness
            .search
            .update(cx, |search, cx| search.expand(window, cx));
    });

    // TestApp has no platform frame loop, so deliver the callback that a
    // headed window runs immediately before painting the expanded input.
    window.update(|_, window, cx| {
        window.simulate_next_frame(cx);
    });
    window.draw();
    window.update(|harness, window, cx| {
        let search = harness.search.read(cx);
        assert!(search.input.read(cx).focus_handle(cx).is_focused(window));
        assert!(
            !harness
                .composer
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        );
    });

    window.simulate_input("fix");
    window.read(|harness, cx| {
        let search = harness.search.read(cx);
        assert_eq!(search.input.read(cx).text(), "fix");
        assert_eq!(search.history.read(cx).search_query, "fix");
        assert!(search.mode == GitHistorySearchMode::Expanded);
    });

    app.advance_clock(HISTORY_SEARCH_IDLE_DISMISS + Duration::from_millis(1));
    app.run_until_parked();
    window.read(|harness, cx| {
        assert!(harness.search.read(cx).mode == GitHistorySearchMode::Expanded);
    });
}

#[test]
fn search_compaction_connects_matches_across_hidden_commits() {
    let commits = vec![
        commit("tip", &["middle"]),
        commit("middle", &["base"]),
        commit("base", &["root"]),
        commit("root", &[]),
    ];
    let visible = HashSet::from(["tip".to_string(), "base".to_string()]);
    let compact = compact_commits_to_visible(&commits, &visible);

    assert_eq!(
        compact
            .iter()
            .map(|commit| commit.sha.as_str())
            .collect::<Vec<_>>(),
        vec!["tip", "base"]
    );
    assert_eq!(compact[0].parent_shas, vec!["base"]);
    assert!(compact[1].parent_shas.is_empty());
}

#[test]
fn search_compaction_handles_a_twenty_thousand_commit_gap() {
    const DEPTH: usize = 20_000;
    let commits = (0..DEPTH)
        .rev()
        .map(|index| {
            let sha = format!("c{index:05}");
            if index == 0 {
                commit(&sha, &[])
            } else {
                let parent = format!("c{:05}", index - 1);
                commit(&sha, &[parent.as_str()])
            }
        })
        .collect::<Vec<_>>();
    let newest = format!("c{:05}", DEPTH - 1);
    let oldest = "c00000".to_string();
    let visible = HashSet::from([newest.clone(), oldest.clone()]);

    let compact = compact_commits_to_visible(&commits, &visible);

    assert_eq!(compact.len(), 2);
    assert_eq!(compact[0].sha, newest);
    assert_eq!(compact[0].parent_shas, vec![oldest.clone()]);
    assert_eq!(compact[1].sha, oldest);
    assert!(compact[1].parent_shas.is_empty());
}

#[test]
fn collapsing_a_branch_contracts_linear_parents_and_keeps_junctions() {
    let commits = vec![
        with_branch(
            commit("feature", &["middle"]),
            GitHistoryRefKind::Branch,
            "feature",
        ),
        commit("middle", &["base"]),
        with_branch(commit("main", &["base"]), GitHistoryRefKind::Branch, "main"),
        commit("base", &["root"]),
        commit("root", &[]),
    ];
    let collapsed = HashSet::from(["local:feature".to_string()]);
    let (visible, counts) = collapse_branch_runs(&commits, &collapsed, Some("main"));

    assert_eq!(
        visible
            .iter()
            .map(|commit| commit.sha.as_str())
            .collect::<Vec<_>>(),
        vec!["feature", "main", "base", "root"]
    );
    assert_eq!(visible[0].parent_shas, vec!["base"]);
    assert_eq!(counts.get("local:feature"), Some(&1));
}

#[test]
fn transition_rows_fold_old_commits_beside_their_stable_anchor() {
    let old = vec![
        commit("tip", &["one"]),
        commit("one", &["two"]),
        commit("two", &["base"]),
        commit("base", &[]),
    ];
    let target = vec![commit("tip", &["base"]), commit("base", &[])];
    let (rows, transitions) = history_transition_rows(&old, &target);

    assert_eq!(
        rows.iter()
            .map(|commit| commit.sha.as_str())
            .collect::<Vec<_>>(),
        vec!["tip", "one", "two", "base"]
    );
    assert_eq!(
        transitions,
        vec![
            HistoryRowTransition::Stable,
            HistoryRowTransition::Exiting,
            HistoryRowTransition::Exiting,
            HistoryRowTransition::Stable,
        ]
    );
}

#[test]
fn transition_rows_expand_new_commits_in_their_final_order() {
    let old = vec![commit("tip", &["base"]), commit("base", &[])];
    let target = vec![
        commit("tip", &["one"]),
        commit("one", &["two"]),
        commit("two", &["base"]),
        commit("base", &[]),
    ];
    let (rows, transitions) = history_transition_rows(&old, &target);

    assert_eq!(
        rows.iter()
            .map(|commit| commit.sha.as_str())
            .collect::<Vec<_>>(),
        vec!["tip", "one", "two", "base"]
    );
    assert_eq!(
        transitions,
        vec![
            HistoryRowTransition::Stable,
            HistoryRowTransition::Entering,
            HistoryRowTransition::Entering,
            HistoryRowTransition::Stable,
        ]
    );
}

#[test]
fn list_splice_preserves_the_unchanged_prefix_around_a_fold() {
    let old = vec![
        commit("tip", &["one"]),
        commit("one", &["two"]),
        commit("two", &["base"]),
        commit("base", &[]),
    ];
    let target = vec![commit("tip", &["base"]), commit("base", &[])];

    assert_eq!(
        history_list_splice(&old, true, &target, true),
        Some((1..3, 0))
    );
}

#[test]
fn removed_scroll_anchor_moves_to_the_next_surviving_commit() {
    let old = vec![
        commit("tip", &["one"]),
        commit("one", &["two"]),
        commit("two", &["base"]),
        commit("base", &[]),
    ];
    let target = vec![commit("tip", &["base"]), commit("base", &[])];
    let anchor = HistoryScrollAnchor {
        sha: "one".into(),
        offset_in_item: px(12.0),
    };

    let resolved = resolve_history_scroll_anchor(Some(anchor), &old, &target).unwrap();
    assert_eq!(resolved.sha, "base");
    assert_eq!(resolved.offset_in_item, px(0.0));
}

#[test]
fn branch_fold_keys_keep_local_and_remote_identity() {
    let local = GitHistoryRef {
        kind: GitHistoryRefKind::Branch,
        label: "main".into(),
    };
    let remote = GitHistoryRef {
        kind: GitHistoryRefKind::Remote,
        label: "origin/main".into(),
    };
    assert_eq!(branch_ref_key(&local).as_deref(), Some("local:main"));
    assert_eq!(
        branch_ref_key(&remote).as_deref(),
        Some("remote:origin/main")
    );
}

#[test]
fn author_avatar_fallback_uses_the_first_visible_initial() {
    assert_eq!(history_author_name(""), SharedString::from("Unknown"));
    assert_eq!(history_author_initial("  josé"), SharedString::from("J"));
    assert_eq!(history_author_initial("   "), SharedString::from("?"));
}

#[test]
fn github_avatar_payload_decodes_into_a_gpui_image() {
    let encoded = base64::engine::general_purpose::STANDARD.encode(b"\xff\xd8\xffpayload");
    let image = decode_history_avatar(&encoded).expect("jpeg payload");
    assert_eq!(image.format, ImageFormat::Jpeg);
    assert!(decode_history_avatar("not base64").is_none());
}

#[test]
fn graph_splits_and_rejoins_merge_lanes() {
    let commits = vec![
        commit("merge", &["main", "feature"]),
        commit("main", &["base"]),
        commit("feature", &["base"]),
        commit("base", &[]),
    ];
    let graph = layout_graph(&commits, Some("merge"));
    assert_eq!(graph.rows.len(), commits.len());
    assert!(graph.max_lane_count >= 2);
    assert!(graph.rows[0].is_head);
    assert_eq!(graph.rows[0].segments.len(), 2);
    assert_eq!(graph.rows[3].segments.len(), 2);
    assert_eq!(graph.rows[3].node_lane, 0);
}

#[test]
fn appending_older_commits_preserves_the_loaded_prefix_layout() {
    let prefix = vec![commit("tip", &["parent"]), commit("parent", &["root"])];
    let before = layout_graph(&prefix, Some("tip"));
    let mut all = prefix.clone();
    all.push(commit("root", &[]));
    let after = layout_graph(&all, Some("tip"));
    assert_eq!(before.rows, after.rows[..before.rows.len()]);
}

#[test]
fn graph_palette_only_reduces_saturation() {
    let source = gpui::hsla(0.62, 0.8, 0.55, 0.9);
    let muted = graph_color(source);
    assert_eq!(muted.h, source.h);
    assert_eq!(muted.l, source.l);
    assert_eq!(muted.a, source.a);
    assert!((muted.s - source.s * HISTORY_GRAPH_SATURATION).abs() < f32::EPSILON);
}

#[test]
fn graph_hover_detects_vertical_and_curved_paths() {
    let geometry = GraphGeometry::natural(2);
    let vertical = GraphRow {
        sha: "vertical".into(),
        node_lane: 0,
        node_color_id: 1,
        segments: vec![GraphSegment {
            from_lane: 1,
            to_lane: 1,
            color_id: 7,
            shape: SegmentShape::Through,
        }],
        is_head: false,
    };
    assert_eq!(
        hovered_graph_path(&vertical, geometry.lane_x(1), 5.0, geometry),
        Some(7)
    );

    let curved_segment = GraphSegment {
        from_lane: 0,
        to_lane: 1,
        color_id: 9,
        shape: SegmentShape::Outgoing,
    };
    let middle = HISTORY_ROW_HEIGHT / 2.0;
    let curve_x = cubic_coordinate(
        geometry.lane_x(0),
        geometry.lane_x(0),
        geometry.lane_x(1),
        geometry.lane_x(1),
        0.5,
    );
    let curve_y = cubic_coordinate(
        middle,
        middle * 1.45,
        middle * 1.45,
        HISTORY_ROW_HEIGHT + HISTORY_GRAPH_ROW_OVERLAP,
        0.5,
    );
    let curved = GraphRow {
        sha: "curved".into(),
        node_lane: 0,
        node_color_id: 1,
        segments: vec![curved_segment],
        is_head: false,
    };
    assert_eq!(
        hovered_graph_path(&curved, curve_x, curve_y, geometry),
        Some(9)
    );
}

#[test]
fn graph_hover_prefers_the_node_and_ignores_empty_space() {
    let geometry = GraphGeometry::natural(4);
    let row = GraphRow {
        sha: "node".into(),
        node_lane: 1,
        node_color_id: 11,
        segments: vec![GraphSegment {
            from_lane: 0,
            to_lane: 1,
            color_id: 4,
            shape: SegmentShape::Incoming,
        }],
        is_head: false,
    };
    assert_eq!(
        hovered_graph_path(&row, geometry.lane_x(1), HISTORY_ROW_HEIGHT / 2.0, geometry,),
        Some(11)
    );
    assert_eq!(
        hovered_graph_path(&row, geometry.width - 1.0, 2.0, geometry),
        None
    );
}

#[test]
fn responsive_graph_keeps_natural_spacing_when_it_fits() {
    let geometry = responsive_graph_geometry(8, 900.0, 240.0);
    assert_eq!(geometry, GraphGeometry::natural(8));
}

#[test]
fn responsive_graph_compresses_lanes_to_preserve_commit_space() {
    let geometry = responsive_graph_geometry(20, 400.0, 240.0);
    assert_eq!(geometry.width, 80.0);
    assert!(geometry.lane_spacing < HISTORY_LANE_SPACING);
    assert_eq!(geometry.lane_x(0), GraphGeometry::natural(20).lane_x(0));
    assert!(geometry.lane_x(19) < GraphGeometry::natural(20).lane_x(19));
}

#[test]
fn narrow_commit_space_switches_to_a_compact_rail_with_hysteresis() {
    let target = responsive_graph_geometry(10, 500.0, 240.0);
    assert!(should_use_compact_graph(
        target,
        GraphGeometry::natural(10),
        500.0,
        240.0,
    ));

    let compact = GraphGeometry::compact(10);
    let still_narrow = responsive_graph_geometry(10, 550.0, 240.0);
    assert!(should_use_compact_graph(
        still_narrow,
        compact,
        550.0,
        240.0,
    ));

    let wide_again = responsive_graph_geometry(10, 570.0, 240.0);
    assert!(!should_use_compact_graph(wide_again, compact, 570.0, 240.0,));
}

#[test]
fn compact_graph_keeps_each_rows_color_as_its_hover_identity() {
    let geometry = GraphGeometry::compact(20);
    let row = GraphRow {
        sha: "compact".into(),
        node_lane: 14,
        node_color_id: 9,
        segments: Vec::new(),
        is_head: false,
    };
    assert_eq!(geometry.lane_x(0), geometry.lane_x(14));
    assert_eq!(
        hovered_graph_path(&row, geometry.lane_x(0), 2.0, geometry),
        Some(9)
    );
}

#[test]
fn graph_geometry_morph_converges_lanes_before_entering_the_compact_rail() {
    let full = GraphGeometry::natural(8);
    let compact = GraphGeometry::compact(8);
    let halfway = interpolate_graph_geometry(full, compact, 0.5);
    assert!(!halfway.compact);
    assert!(halfway.width < full.width);
    assert!(halfway.width > compact.width);
    assert!(halfway.lane_spacing < full.lane_spacing);
    assert!(halfway.lane_spacing > compact.lane_spacing);

    let settled = interpolate_graph_geometry(full, compact, 1.0);
    assert!(settled.compact);
    assert_eq!(settled.width, compact.width);
    assert_eq!(settled.lane_spacing, 0.0);
}

#[test]
fn graph_geometry_morph_expands_the_rail_from_its_compact_start() {
    let compact = GraphGeometry::compact(8);
    let full = GraphGeometry::natural(8);
    assert!(interpolate_graph_geometry(compact, full, 0.0).compact);
    let halfway = interpolate_graph_geometry(compact, full, 0.5);
    assert!(!halfway.compact);
    assert!(halfway.lane_spacing > 0.0);
    assert!(halfway.lane_spacing < full.lane_spacing);
}

#[test]
fn compressed_graph_hit_testing_uses_the_fitted_lane_positions() {
    let geometry = GraphGeometry::fitted(20, 80.0);
    let row = GraphRow {
        sha: "compact".into(),
        node_lane: 19,
        node_color_id: 7,
        segments: Vec::new(),
        is_head: false,
    };
    assert_eq!(
        hovered_graph_path(
            &row,
            geometry.lane_x(19),
            HISTORY_ROW_HEIGHT / 2.0,
            geometry,
        ),
        Some(7)
    );
}

#[test]
fn responsive_graph_geometry_ignores_sub_step_resize_jitter() {
    let previous = GraphGeometry::fitted(20, 80.0);
    let target = GraphGeometry::fitted(20, 81.9);
    let stable = stabilized_graph_geometry(target, previous, 2.0, false);
    assert_eq!(stable.width, previous.width);
    assert_eq!(stable.lane_spacing, previous.lane_spacing);
    assert_eq!(stable.device_scale, 2.0);

    let next = stabilized_graph_geometry(GraphGeometry::fitted(20, 82.1), stable, 2.0, false);
    assert_eq!(next.width, 82.0);
    assert_ne!(next.lane_spacing, stable.lane_spacing);
}

#[test]
fn responsive_graph_lanes_snap_to_device_pixels() {
    let geometry = stabilized_graph_geometry(
        GraphGeometry::fitted(20, 82.0),
        GraphGeometry::natural(1),
        2.0,
        false,
    );
    for lane in 0..20 {
        assert!((geometry.lane_x(lane) * 2.0).fract().abs() < f32::EPSILON);
    }
}

#[test]
fn ref_badges_expand_with_the_available_width() {
    let reference = |label: &str| GitHistoryRef {
        kind: GitHistoryRefKind::Branch,
        label: label.into(),
    };
    let refs = vec![reference("main"), reference("tag"), reference("origin")];
    assert_eq!(visible_ref_count(&[], 100.0), 0);
    assert_eq!(visible_ref_count(&refs, 70.0), 1);
    assert_eq!(visible_ref_count(&refs, 115.0), 2);
    assert_eq!(visible_ref_count(&refs, 200.0), 3);
}

#[test]
fn ref_badges_preserve_overflow_when_the_first_badge_is_too_wide() {
    let reference = |label: &str| GitHistoryRef {
        kind: GitHistoryRefKind::Branch,
        label: label.into(),
    };
    let refs = vec![
        reference("feature/accessibility-polish"),
        reference("main"),
        reference("origin/main"),
        reference("upstream/main"),
        reference("v0.1.53"),
        reference("HEAD"),
    ];

    assert_eq!(visible_ref_count(&refs, 120.0), 0);
}

#[test]
fn ref_area_preserves_subject_space_and_caps_at_forty_five_percent() {
    assert_eq!(ref_area_width(80.0), 0.0);
    assert!((ref_area_width(200.0) - 86.4).abs() < 0.001);
    assert!((ref_area_width(400.0) - 176.4).abs() < 0.001);
}

#[test]
fn commit_divider_resizes_the_first_visible_fixed_column() {
    let widths = GitHistoryColumnWidths::default();
    let resized = resized_history_column_widths(
        widths,
        HistoryColumnDragAnchor {
            start_x: 0.0,
            left: HistoryDataColumn::Commit,
            right: HistoryDataColumn::Author,
            left_width: HISTORY_COMMIT_SUBJECT_MIN_WIDTH,
            right_width: widths.author,
        },
        20.0,
    );
    assert_eq!(resized.author, 68.0);
    assert_eq!(resized.date, widths.date);
    assert_eq!(resized.sha, widths.sha);
}

#[test]
fn interior_column_divider_preserves_width_and_clamps_both_sides() {
    let widths = GitHistoryColumnWidths::default();
    let anchor = HistoryColumnDragAnchor {
        start_x: 0.0,
        left: HistoryDataColumn::Author,
        right: HistoryDataColumn::Date,
        left_width: widths.author,
        right_width: widths.date,
    };
    let resized = resized_history_column_widths(widths, anchor, 10.0);
    assert_eq!(resized.author, 98.0);
    assert_eq!(resized.date, 78.0);
    assert_eq!(resized.author + resized.date, widths.author + widths.date);

    let clamped = resized_history_column_widths(widths, anchor, 1_000.0);
    assert_eq!(clamped.date, GitHistoryColumnWidths::DATE_MIN);
    assert_eq!(clamped.author, 108.0);
}

#[test]
fn visible_columns_follow_persisted_order_and_skip_hidden_entries() {
    let order = GitHistoryColumnOrder(vec![
        GitHistoryColumn::Sha,
        GitHistoryColumn::Author,
        GitHistoryColumn::Date,
    ]);
    let columns = GitHistoryColumns {
        author: false,
        date: true,
        sha: true,
    };
    assert_eq!(
        visible_history_columns(&order, columns),
        vec![GitHistoryColumn::Sha, GitHistoryColumn::Date]
    );
}

#[test]
fn reordering_visible_columns_preserves_hidden_columns() {
    let order = GitHistoryColumnOrder::default();
    let reordered =
        reordered_history_columns(&order, GitHistoryColumn::Sha, GitHistoryColumn::Date);
    assert_eq!(
        reordered,
        GitHistoryColumnOrder(vec![
            GitHistoryColumn::Author,
            GitHistoryColumn::Sha,
            GitHistoryColumn::Date,
        ])
    );
}

#[test]
fn reorder_drop_index_respects_uneven_column_widths() {
    let columns = [
        GitHistoryColumn::Author,
        GitHistoryColumn::Date,
        GitHistoryColumn::Sha,
    ];
    let widths = GitHistoryColumnWidths::default();
    let rendered = widths.author + widths.date + widths.sha;
    assert_eq!(
        history_column_drop_index(10.0, rendered, &columns, widths),
        0
    );
    assert_eq!(
        history_column_drop_index(100.0, rendered, &columns, widths),
        1
    );
    assert_eq!(
        history_column_drop_index(240.0, rendered, &columns, widths),
        2
    );
}

#[test]
fn ref_tooltip_describes_each_reference_kind() {
    let reference = |kind: GitHistoryRefKind, label: &str| GitHistoryRef {
        kind,
        label: label.into(),
    };
    assert_eq!(
        ref_description(&reference(GitHistoryRefKind::Branch, "main")),
        "Branch: main"
    );
    assert_eq!(
        ref_description(&reference(GitHistoryRefKind::Remote, "origin/main")),
        "Remote branch: origin/main"
    );
    assert_eq!(
        ref_description(&reference(GitHistoryRefKind::Tag, "v0.1.52")),
        "Tag: v0.1.52"
    );
}
