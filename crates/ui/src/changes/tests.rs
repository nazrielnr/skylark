    use super::*;
    use chrono::Utc;

    #[gpui::test]
    fn editing_staged_diff_comments_preserves_identity_and_cancellation(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::dark());
        });
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            Changes::new(state, cx)
        });
        window
            .update(cx, |changes, window, cx| {
                for side in [CommentSide::Old, CommentSide::New] {
                    let original =
                        ReviewComment::new("new.rs", side, 7, "Original 🦀\nSecond line")
                            .renamed_from(Some("old.rs"));
                    changes.state.update(cx, |state, _| {
                        state.add_review_comment("", original.clone())
                    });
                    changes.edit_comment(&original.id, window, cx);
                    let draft = changes.draft.as_ref().unwrap();
                    assert_eq!(draft.input.read(cx).text(), original.body);
                    assert_eq!(draft_cite_path(draft), original.cite_path());
                    assert!(draft.input.read(cx).focus_handle(cx).is_focused(window));
                    let input = draft.input.clone();
                    input.update(cx, |input, cx| input.set_text("Cancelled", cx));
                    assert_eq!(changes.state.read(cx).review_comments("")[0], original);
                    changes.cancel_draft(cx);
                    assert_eq!(
                        changes.state.read(cx).review_comments(""),
                        &[original.clone()]
                    );

                    changes.edit_comment(&original.id, window, cx);
                    changes
                        .draft
                        .as_ref()
                        .unwrap()
                        .input
                        .clone()
                        .update(cx, |input, cx| {
                            input.set_text("  Revised\nMore detail  ", cx)
                        });
                    changes.commit_draft(cx);
                    let mut expected = original.clone();
                    expected.body = "Revised\nMore detail".into();
                    assert_eq!(
                        changes.state.read(cx).review_comments(""),
                        &[expected.clone()]
                    );
                    assert_ne!(
                        comment_state_key(&[original], None),
                        comment_state_key(&[expected.clone()], None)
                    );
                    changes.edit_comment(&expected.id, window, cx);
                    let sent = changes
                        .state
                        .update(cx, |state, _| state.take_review_comments(""));
                    assert!(comments::with_comments("", &sent).contains("Revised\n  More detail"));
                    changes.commit_draft(cx);
                    assert!(changes.state.read(cx).review_comments("").is_empty());
                }
            })
            .unwrap();
    }

    const PATCH: &str = "\
diff --git a/src/main.rs b/src/main.rs
index 111..222 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,4 +1,5 @@ fn main
 fn main() {
-    println!(\"old\");
+    println!(\"new\");
+    let x = 1;
 }
@@ -10,2 +11,2 @@
 // tail
-old_line
+new_line
diff --git a/added.txt b/added.txt
new file mode 100644
--- /dev/null
+++ b/added.txt
@@ -0,0 +1,2 @@
+first
+second
\\ No newline at end of file
diff --git a/gone.txt b/gone.txt
deleted file mode 100644
--- a/gone.txt
+++ /dev/null
@@ -1,1 +0,0 @@
-bye
diff --git a/img.png b/img.png
new file mode 100644
Binary files /dev/null and b/img.png differ
diff --git a/old_name.rs b/new_name.rs
similarity index 90%
rename from old_name.rs
rename to new_name.rs
";

    #[test]
    fn parses_files_hunks_and_lines() {
        let files = parse_patch(PATCH);
        assert_eq!(files.len(), 5);

        let main = &files[0];
        assert_eq!(main.path, "src/main.rs");
        assert_eq!(main.status, FileStatus::Modified);
        assert_eq!(main.hunks.len(), 2);
        assert_eq!(main.additions, 3);
        assert_eq!(main.deletions, 2);
        let h0 = &main.hunks[0];
        assert_eq!(h0.header, "@@ -1,4 +1,5 @@ fn main");
        assert_eq!(h0.lines.len(), 5);
        assert_eq!(h0.lines[0].kind, LineKind::Context);
        assert_eq!(h0.lines[0].old_no, Some(1));
        assert_eq!(h0.lines[0].new_no, Some(1));
        assert_eq!(h0.lines[1].kind, LineKind::Del);
        assert_eq!(h0.lines[1].old_no, Some(2));
        assert_eq!(h0.lines[1].new_no, None);
        assert_eq!(h0.lines[2].kind, LineKind::Add);
        assert_eq!(h0.lines[2].new_no, Some(2));
        assert_eq!(h0.lines[3].kind, LineKind::Add);
        assert_eq!(h0.lines[3].new_no, Some(3));
        // Closing context line: numbering advanced past the add/del block.
        assert_eq!(h0.lines[4].old_no, Some(3));
        assert_eq!(h0.lines[4].new_no, Some(4));
        // Second hunk restarts numbering from its header.
        assert_eq!(main.hunks[1].lines[0].old_no, Some(10));
        assert_eq!(main.hunks[1].lines[0].new_no, Some(11));
    }

    #[test]
    fn detects_new_deleted_binary_and_renamed() {
        let files = parse_patch(PATCH);
        let added = &files[1];
        assert_eq!(added.status, FileStatus::Added);
        assert_eq!(added.additions, 2);
        // The no-newline marker rides as a Meta line.
        let last = added.hunks[0].lines.last().unwrap();
        assert_eq!(last.kind, LineKind::Meta);
        assert!(last.text.contains("No newline"));
        assert!(file_notices(added).iter().any(|n| n == "New file"));

        let deleted = &files[2];
        assert_eq!(deleted.status, FileStatus::Deleted);
        assert_eq!(deleted.deletions, 1);
        assert!(file_notices(deleted).iter().any(|n| n == "Deleted file"));

        let binary = &files[3];
        assert!(binary.binary);
        assert_eq!(binary.status, FileStatus::Added);
        assert!(binary.hunks.is_empty());
        assert!(file_notices(binary).iter().any(|n| n.contains("Binary")));

        let renamed = &files[4];
        assert_eq!(renamed.status, FileStatus::Renamed);
        assert_eq!(renamed.path, "new_name.rs");
        assert_eq!(renamed.old_path.as_deref(), Some("old_name.rs"));
        assert!(
            file_notices(renamed)
                .iter()
                .any(|n| n.contains("old_name.rs"))
        );
    }

    #[test]
    fn empty_and_garbage_patches_parse_to_nothing() {
        assert!(parse_patch("").is_empty());
        assert!(parse_patch("not a diff\nat all\n").is_empty());
        // Truncated mid-hunk: keeps what parsed.
        let files = parse_patch("diff --git a/x b/x\n@@ -1,9 +1,9 @@\n ctx\n+add");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].hunks[0].lines.len(), 2);
        assert_eq!(files[0].additions, 1);
    }

    #[test]
    fn quoted_and_spaced_paths() {
        let (old, new) = parse_git_paths("a/simple.rs b/simple.rs");
        assert_eq!((old.as_str(), new.as_str()), ("simple.rs", "simple.rs"));
        let (old, new) = parse_git_paths("\"a/with space.rs\" \"b/with space.rs\"");
        assert_eq!(old, "with space.rs");
        assert_eq!(new, "with space.rs");
    }

    #[test]
    fn hunk_headers_parse_with_and_without_counts() {
        assert_eq!(parse_hunk_header("@@ -1,4 +2,5 @@"), Some((1, 2)));
        assert_eq!(parse_hunk_header("@@ -7 +9 @@ fn ctx"), Some((7, 9)));
        assert_eq!(parse_hunk_header("@@ garbage"), None);
    }

    #[test]
    fn rows_flatten_to_line_granularity() {
        let files = parse_patch(PATCH);
        let (rows, ranges) = flatten_rows(&files, &[], None, DiffMode::Unified, |_| false);
        assert_eq!(ranges.len(), files.len());
        // Every file's span starts with its header…
        for (ix, range) in ranges.iter().enumerate() {
            assert_eq!(rows[range.start], DiffRow::FileHeader { file: ix as u32 });
            // …and spans exactly header + analytic body rows.
            assert_eq!(range.len(), 1 + body_row_count(&files[ix]));
        }
        // Spans tile the whole row vec.
        assert_eq!(ranges.last().unwrap().end, rows.len());

        // src/main.rs: header, 2 hunk headers, 8 lines, pad.
        let main_rows = &rows[ranges[0].clone()];
        assert_eq!(main_rows.len(), 1 + 2 + 8 + 1);
        assert_eq!(main_rows[1], DiffRow::HunkHeader { file: 0, hunk: 0 });
        // Flat line indices run across hunks (they key the highlight slot).
        let flats: Vec<u32> = main_rows
            .iter()
            .filter_map(|r| match r {
                DiffRow::Line { flat, .. } => Some(*flat),
                _ => None,
            })
            .collect();
        assert_eq!(flats, (0..8).collect::<Vec<u32>>());
        assert_eq!(*main_rows.last().unwrap(), DiffRow::BodyPad { file: 0 });

        // A collapsed file contributes its header row only.
        let (rows, ranges) = flatten_rows(&files, &[], None, DiffMode::Unified, |ix| ix == 0);
        assert_eq!(ranges[0].len(), 1);
        assert_eq!(rows[ranges[1].start], DiffRow::FileHeader { file: 1 });

        // Notices lead the body: the added file carries "New file".
        let added_rows = &rows[ranges[1].clone()];
        assert_eq!(added_rows[1], DiffRow::Notice { file: 1, notice: 0 });
    }

    #[test]
    fn sticky_header_tracks_the_logical_top_row() {
        let ranges = vec![0..4, 4..5, 5..10];

        assert_eq!(sticky_file_header(&[], 0, 0.0), None);
        assert_eq!(sticky_file_header(&ranges, 0, 0.0), None);
        assert_eq!(
            sticky_file_header(&ranges, 0, 0.5),
            Some(StickyFileHeader {
                file_ix: 0,
                header_row: 0,
                next_header_row: Some(4),
            })
        );
        assert_eq!(
            sticky_file_header(&ranges, 2, 0.0),
            Some(StickyFileHeader {
                file_ix: 0,
                header_row: 0,
                next_header_row: Some(4),
            })
        );

        // Landing exactly on a new header hands ownership to that file; its
        // real row remains visible until it starts crossing the viewport.
        assert_eq!(sticky_file_header(&ranges, 4, 0.0), None);
        assert_eq!(
            sticky_file_header(&ranges, 4, 1.0),
            Some(StickyFileHeader {
                file_ix: 1,
                header_row: 4,
                next_header_row: Some(5),
            })
        );
        assert_eq!(sticky_file_header(&ranges, 5, 0.0), None);
        assert_eq!(
            sticky_file_header(&ranges, 8, 0.0),
            Some(StickyFileHeader {
                file_ix: 2,
                header_row: 5,
                next_header_row: None,
            })
        );
        assert_eq!(sticky_file_header(&ranges, 10, 0.0), None);
    }

    #[test]
    fn sticky_header_is_pushed_by_the_next_file() {
        assert_eq!(sticky_header_push_offset(None), 0.0);
        assert_eq!(sticky_header_push_offset(Some(80.0)), 0.0);
        assert_eq!(sticky_header_push_offset(Some(FILE_HEADER_HEIGHT)), 0.0);
        assert_eq!(
            sticky_header_push_offset(Some(FILE_HEADER_HEIGHT - 12.0)),
            -12.0
        );
        assert_eq!(sticky_header_push_offset(Some(0.0)), -FILE_HEADER_HEIGHT);
    }

    #[test]
    fn sticky_header_uses_the_content_theme_in_dark_and_light() {
        use zeron_theme::{AccentSelection, SurfacePreference};

        for (appearance, variant_id) in [
            (crate::theme::Appearance::Dark, "gruvbox-dark"),
            (crate::theme::Appearance::Light, "gruvbox-light"),
        ] {
            let opaque = Theme::for_selection(
                appearance,
                variant_id,
                AccentSelection::ThemeDefault,
                SurfacePreference::Opaque,
            );
            let opaque_paint = sticky_file_header_paint(&opaque);
            assert_eq!(opaque_paint.frost_tint, None, "{variant_id}");
            assert_eq!(
                opaque_paint.rest_bg,
                crate::theme::flatten(opaque.ink(0.025), opaque.bg),
                "{variant_id} opaque background"
            );
            assert_eq!(
                opaque_paint.hover_bg,
                crate::theme::flatten(opaque.element_hover, opaque.bg),
                "{variant_id} opaque hover"
            );
            assert_eq!(opaque_paint.border, opaque.border, "{variant_id} border");

            let frosted = Theme::for_selection(
                appearance,
                variant_id,
                AccentSelection::ThemeDefault,
                SurfacePreference::Frosted,
            );
            let frosted_paint = sticky_file_header_paint(&frosted);
            if frosted.is_frost() {
                let expected_alpha = match appearance {
                    crate::theme::Appearance::Dark => STICKY_FILE_HEADER_TINT_ALPHA_DARK,
                    crate::theme::Appearance::Light => STICKY_FILE_HEADER_TINT_ALPHA_LIGHT,
                };
                let tint = frosted.bg.opacity(expected_alpha);
                assert_eq!(
                    frosted_paint.frost_tint,
                    Some(tint),
                    "{variant_id} content-plane tint"
                );
                assert_ne!(
                    tint,
                    frosted.glass_overlay(),
                    "{variant_id} must not borrow the elevated overlay plane"
                );
                assert_eq!(tint.a, expected_alpha, "{variant_id} tint coverage");
                assert_eq!(
                    frosted_paint.hover_bg,
                    frosted.glass_hover(),
                    "{variant_id} themed hover"
                );
            } else {
                assert_eq!(frosted_paint.frost_tint, None, "{variant_id}");
            }
            assert_eq!(
                frosted_paint.border, frosted.border,
                "{variant_id} frosted border"
            );
        }
    }

    #[test]
    fn split_pairs_align_edits_and_strand_the_rest() {
        let files = parse_patch(PATCH);
        // src/main.rs hunk 0: context, −1, +1, +1, context. The edited line
        // pairs across; the extra addition is stranded on the right.
        assert_eq!(
            split_pairs(&files[0].hunks[0].lines),
            vec![
                (Some(0), Some(0)),
                (Some(1), Some(2)),
                (None, Some(3)),
                (Some(4), Some(4)),
            ]
        );
        // A pure add: every row is right-only, including the trailing
        // no-newline Meta line — it belongs to the side it follows, and its
        // row spans both columns at render.
        assert_eq!(
            split_pairs(&files[1].hunks[0].lines),
            vec![(None, Some(0)), (None, Some(1)), (None, Some(2))]
        );
        // A pure delete strands the left.
        assert_eq!(split_pairs(&files[2].hunks[0].lines), vec![(Some(0), None)]);
        assert!(split_pairs(&[]).is_empty());

        // `-a +b -c +d` is two one-line edits, not one four-line one: a
        // deletion arriving after additions opens a new block.
        let line = |kind| DiffLine {
            kind,
            old_no: Some(1),
            new_no: Some(1),
            text: String::new(),
        };
        let lines = [
            line(LineKind::Del),
            line(LineKind::Add),
            line(LineKind::Del),
            line(LineKind::Add),
        ];
        assert_eq!(
            split_pairs(&lines),
            vec![(Some(0), Some(1)), (Some(2), Some(3))]
        );
    }

    #[test]
    fn no_newline_markers_keep_their_edit_paired() {
        // Both files lost their final newline: git writes the marker twice,
        // once per side. Neither may split the edit into one-sided rows.
        let both = "diff --git a/a.txt b/a.txt\n\
             --- a/a.txt\n\
             +++ b/a.txt\n\
             @@ -1 +1 @@\n\
             -old\n\
             \\ No newline at end of file\n\
             +new\n\
             \\ No newline at end of file\n";
        let files = parse_patch(both);
        let lines = &files[0].hunks[0].lines;
        assert_eq!(
            lines.iter().map(|line| line.kind).collect::<Vec<_>>(),
            vec![LineKind::Del, LineKind::Meta, LineKind::Add, LineKind::Meta]
        );
        // One aligned old/new row, then the two markers on one row of their
        // own — four lines read as two rows, not four.
        assert_eq!(
            split_pairs(lines),
            vec![(Some(0), Some(2)), (Some(1), Some(3))]
        );
        let full = split_pairs(lines);
        for cap in 0..=full.len() + 2 {
            assert_eq!(split_pairs_upto(lines, cap), full[..cap.min(full.len())]);
        }

        // Only the old file lacked one: the edit still pairs, and the lone
        // marker takes a row on its own side.
        let old_only = "diff --git a/a.txt b/a.txt\n\
             --- a/a.txt\n\
             +++ b/a.txt\n\
             @@ -1 +1 @@\n\
             -old\n\
             \\ No newline at end of file\n\
             +new\n";
        let files = parse_patch(old_only);
        assert_eq!(
            split_pairs(&files[0].hunks[0].lines),
            vec![(Some(0), Some(2)), (Some(1), None)]
        );
    }

    #[test]
    fn split_flattening_pairs_rows_and_keeps_heights_analytic() {
        let files = parse_patch(PATCH);
        let (rows, ranges) = flatten_rows(&files, &[], None, DiffMode::Split, |_| false);
        assert_eq!(ranges.len(), files.len());
        assert_eq!(ranges.last().unwrap().end, rows.len());

        // src/main.rs: header, 2 hunk headers, 4 + 2 paired rows, pad — the
        // same 8 lines, two columns.
        let main_rows = &rows[ranges[0].clone()];
        assert_eq!(main_rows.len(), 1 + 2 + (4 + 2) + 1);
        assert_eq!(
            main_rows[2],
            DiffRow::SplitLine {
                file: 0,
                hunk: 0,
                left: Some(0),
                right: Some(0),
            }
        );
        assert_eq!(
            main_rows[4],
            DiffRow::SplitLine {
                file: 0,
                hunk: 0,
                left: None,
                right: Some(3),
            }
        );
        assert_eq!(*main_rows.last().unwrap(), DiffRow::BodyPad { file: 0 });
        // Pairing only ever merges rows, so split is never the taller layout.
        assert!(main_rows.len() < 1 + body_row_count(&files[0]));

        // Heights stay analytic — the fold tween needs no measurement.
        assert_eq!(
            body_height_with(&files[0], &[], None, DiffMode::Split, DIFF_LINE_HEIGHT),
            2.0 * HUNK_HEADER_HEIGHT + 6.0 * DIFF_LINE_HEIGHT + BODY_BOTTOM_PAD
        );
    }

    #[test]
    fn capped_pairing_agrees_with_the_full_pairing_and_stays_bounded() {
        // The fold tween re-renders its stand-in every frame, so the capped
        // walk must be a true prefix of the full one — not an approximation.
        let lines = &parse_patch(PATCH)[0].hunks[0].lines;
        let full = split_pairs(lines);
        for cap in 0..=full.len() + 2 {
            assert_eq!(split_pairs_upto(lines, cap), full[..cap.min(full.len())]);
        }

        // A huge single-sided run must not be materialized to yield a few
        // rows: 20k deletions, 5 rows asked for, 5 rows built.
        let many: Vec<DiffLine> = (0..20_000u32)
            .map(|n| DiffLine {
                kind: LineKind::Del,
                old_no: Some(n + 1),
                new_no: None,
                text: String::new(),
            })
            .collect();
        let capped = split_pairs_upto(&many, 5);
        assert_eq!(capped.len(), 5);
        assert!(
            capped.capacity() < 100,
            "capacity tracks the cap, not the hunk"
        );
        assert_eq!(capped[4], (Some(4), None));
    }

    #[test]
    fn a_split_row_offers_each_column_its_own_anchor() {
        let files = parse_patch(PATCH);
        let lines = &files[0].hunks[0].lines;
        // The paired edit cites the old line on the left, the new on the right.
        assert_eq!(
            pair_anchors(lines, (Some(1), Some(2))),
            [Some((CommentSide::Old, 2)), Some((CommentSide::New, 2))]
        );
        // A context row names one anchor, not the same one twice — otherwise
        // its card would be pushed into the body in duplicate. The caller
        // flattens, so the dropped duplicate reads as an empty slot.
        assert_eq!(
            pair_anchors(lines, (Some(0), Some(0))),
            [Some((CommentSide::New, 1)), None]
        );
        // A stranded side contributes nothing.
        assert_eq!(
            pair_anchors(lines, (None, Some(3))),
            [None, Some((CommentSide::New, 3))]
        );
    }

    #[test]
    fn split_rows_carry_the_comments_of_both_columns() {
        let files = parse_patch(PATCH);
        // A context row must not stack the same card twice.
        let comment = ReviewComment::new("src/main.rs", CommentSide::New, 1, "why");
        let rows = body_rows(0, &files[0], &[comment], None, DiffMode::Split);
        assert_eq!(
            rows.iter()
                .filter(|row| matches!(row, DiffRow::CommentCard { .. }))
                .count(),
            1
        );

        // Both sides of one paired row hang off that row, in column order.
        let staged = vec![
            ReviewComment::new("src/main.rs", CommentSide::Old, 2, "left"),
            ReviewComment::new("src/main.rs", CommentSide::New, 2, "right"),
        ];
        let rows = body_rows(0, &files[0], &staged, None, DiffMode::Split);
        let edit = rows
            .iter()
            .position(|row| matches!(row, DiffRow::SplitLine { left: Some(1), .. }))
            .unwrap();
        assert_eq!(rows[edit + 1], DiffRow::CommentCard { file: 0, card: 0 });
        assert_eq!(rows[edit + 2], DiffRow::CommentCard { file: 0, card: 1 });
    }

    #[test]
    fn a_split_rows_right_column_is_never_a_deletion() {
        // The invariant the `+` placement rests on: only the right column is
        // hoverable, so every note a split row can start must cite the
        // post-change file. Were a deletion ever to land on the right, that
        // rule would quietly start filing notes against lines the agent
        // cannot edit.
        for file in parse_patch(PATCH) {
            for hunk in &file.hunks {
                for (_, right) in split_pairs(&hunk.lines) {
                    let Some(line) = right.and_then(|ix| hunk.lines.get(ix as usize)) else {
                        continue;
                    };
                    assert_ne!(line.kind, LineKind::Del, "{:?}", line);
                    assert!(matches!(
                        line_anchor(line),
                        None | Some((CommentSide::New, _))
                    ));
                }
            }
        }
    }

    #[test]
    fn truncate_caps_lines_and_appends_notice() {
        let mut file = parse_patch(PATCH).remove(0); // 2 hunks, 8 lines
        let untouched = file.clone();
        truncate_file_lines(&mut file, 10);
        assert_eq!(file, untouched, "under the cap: untouched");

        truncate_file_lines(&mut file, 6);
        let lines: usize = file.hunks.iter().map(|h| h.lines.len()).sum();
        assert_eq!(lines, 6);
        assert_eq!(file.hunks.len(), 2);
        assert!(
            file_notices(&file)
                .iter()
                .any(|n| n.contains("first 6 of 8 lines"))
        );
        // body_height stays consistent with what actually renders.
        assert_eq!(
            body_height(&file),
            NOTICE_HEIGHT + 2.0 * HUNK_HEADER_HEIGHT + 6.0 * DIFF_LINE_HEIGHT + BODY_BOTTOM_PAD
        );

        // A cap below the first hunk's length drops later hunks entirely.
        let mut file = parse_patch(PATCH).remove(0);
        truncate_file_lines(&mut file, 3);
        assert_eq!(file.hunks.len(), 1);
        assert_eq!(file.hunks[0].lines.len(), 3);
    }

    #[test]
    fn gutters_fit_the_largest_line_number() {
        let files = parse_patch(PATCH);
        // src/main.rs second hunk ends at old 11 / new 12.
        assert_eq!(files[0].max_line, 12);
        assert_eq!(gutter_width(&files[0]), GUTTER_WIDTH);

        // Every digit count keeps ≥6px clear of the accent bar on the left
        // of the number (digits×6.6 + 8px right pad + 6px gap), and the
        // column never shrinks below the classic 36px.
        let mut file = files[0].clone();
        for digits in 1..=7u32 {
            file.max_line = 10u32.pow(digits) - 1;
            let w = gutter_width(&file);
            assert!(w >= GUTTER_WIDTH);
            let left_gap = w - (digits as f32 * 6.6 + 8.0);
            assert!(
                left_gap >= 6.0,
                "{digits} digits: left gap {left_gap} < 6px"
            );
        }
        // 4 digits outgrow the classic column now (the old formula left
        // them 1.6px off the bar — visually touching).
        file.max_line = 9999;
        assert!(gutter_width(&file) > GUTTER_WIDTH);
        file.max_line = 27404;
        assert!(
            gutter_width(&file)
                > gutter_width(&{
                    let mut f = file.clone();
                    f.max_line = 9999;
                    f
                })
        );

        // Truncation refits the gutter to what actually renders: the first
        // 3 lines are ctx(1,1) / del(2,·) / add(·,2) — max line 2.
        let mut file = files[0].clone();
        truncate_file_lines(&mut file, 3);
        assert_eq!(file.max_line, 2);
    }

    #[test]
    fn horizontal_geometry_counts_tabs_and_unicode_columns() {
        assert_eq!(visual_columns("ab\tc"), 5);
        assert_eq!(visual_columns("界"), 2);
        assert_eq!(visual_columns("e\u{301}"), 1);

        let files = parse_patch("diff --git a/x b/x\n@@ -1 +1 @@\n-old\n+ab\t界\n");
        let geometry = DiffHorizontalGeometry::from_file(&files[0]);
        assert_eq!(geometry.max_code_columns, 6);
        assert_eq!(geometry.max_gutter_width, GUTTER_WIDTH);
    }

    #[test]
    fn horizontal_content_width_compensates_for_local_gutters() {
        let metrics = DiffHorizontalMetrics {
            max_text_width: 240.0,
            max_gutter_width: 52.0,
        };
        let narrow = 36.0;
        let wide = 52.0;

        let unified_total = |gutter| {
            ACCENT_BAR_WIDTH + 2.0 * gutter + MARKER_WIDTH + metrics.unified_content_width(gutter)
        };
        assert_eq!(unified_total(narrow), unified_total(wide));

        let split_total = |gutter| {
            ACCENT_BAR_WIDTH + gutter + SPLIT_MARKER_WIDTH + metrics.split_content_width(gutter)
        };
        assert_eq!(split_total(narrow), split_total(wide));
    }

    /// Uses the native font backend, not TestAppContext's simulated metrics.
    #[test]
    fn native_diff_font_geometry() {
        // Windows headless mode uses NoopTextSystem. This regression needs
        // actual DirectWrite metrics, as it does CoreText/fontconfig elsewhere.
        let platform = gpui_platform::current_platform(!cfg!(windows));
        let text_system =
            gpui::WindowTextSystem::new(Arc::new(gpui::TextSystem::new(platform.text_system())));
        text_system
            .add_fonts(
                crate::typography::bundled_font_faces()
                    .map(std::borrow::Cow::Borrowed)
                    .collect(),
            )
            .unwrap();
        let mut theme = Theme::dark();
        theme.font_mono = "Geist".into();
        let wide = "W".repeat(100);
        let source = format!("{wide}\n{}\n\t漢字🙂e\u{301}\n", "WWW(WWW);".repeat(200));
        let patch = format!(
            "diff --git a/x.ts b/x.ts\n@@ -0,0 +1,3 @@\n+{}",
            source.trim_end_matches('\n').replace('\n', "\n+")
        );
        let files = parse_patch(&patch);
        let file = &files[0];
        let state = FileHorizontalState::new(file);
        let highlight = Arc::new(DiffHighlights {
            old: None,
            new: Some(Arc::new(
                zeron_syntax::highlight(zeron_syntax::HighlightRequest {
                    source: &source,
                    path: Some("x.ts"),
                    fence_tag: None,
                })
                .unwrap(),
            )),
        });
        let mono = font(theme.font_mono.clone());
        let column = text_system
            .ch_advance(text_system.resolve_font(&mono), px(12.))
            .unwrap()
            .as_f32();
        let line = &file.hunks[0].lines[0];
        let width = text_system
            .shape_line(wide.into(), px(12.), &line_runs(line, None, &theme), None)
            .width()
            .as_f32();
        assert!(
            width > 100. * column * 1.1,
            "requires a real proportional font: {width} vs {}",
            100. * column
        );

        // Simulate plain -> excerpt -> full highlighting, including replacement
        // while an old cached Arc is still alive. Every paint must be reachable.
        for size in [12.5, 32., 8.] {
            theme.code_font_size = size;
            for highlights in [
                None,
                Some(highlight.clone()),
                Some(Arc::new(DiffHighlights {
                    old: None,
                    new: highlight.new.clone(),
                })),
            ] {
                let metrics = state.metrics(file, highlights.as_ref(), &theme, &text_system, 0);
                for line in &file.hunks[0].lines {
                    let runs = line_runs(line, highlights.as_deref(), &theme);
                    let painted = text_system
                        .shape_line(
                            line.text.clone().into(),
                            px(diff_text_size(&theme)),
                            &runs,
                            None,
                        )
                        .width()
                        .as_f32();
                    assert!(metrics.max_text_width >= painted);
                    assert!(
                        metrics.unified_content_width(gutter_width(file))
                            >= UNIFIED_CODE_PADDING_LEFT + painted
                    );
                    assert!(
                        metrics.split_content_width(gutter_width(file))
                            >= SPLIT_CODE_PADDING_LEFT + painted
                    );
                }
                assert_eq!(
                    state.metrics(file, highlights.as_ref(), &theme, &text_system, 0),
                    metrics
                );
            }
        }
    }

    #[test]
    fn horizontal_scroll_and_width_are_independent_per_file() {
        let files = parse_patch(
            "diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+short\n\
             diff --git a/b b/b\n@@ -1 +1 @@\n-old\n+a much longer source line\n",
        );
        let states: Vec<_> = files.iter().map(FileHorizontalState::new).collect();
        assert_eq!(states.len(), 2);
        assert_eq!(states[0].geometry.max_code_columns, 5);
        assert_eq!(states[1].geometry.max_code_columns, 25);

        let row = DiffRow::Line {
            file: 0,
            hunk: 0,
            line: 0,
            flat: 0,
        };
        let folding = DiffRow::FoldingBody { file: 0 };
        let split = DiffRow::SplitLine {
            file: 1,
            hunk: 0,
            left: Some(0),
            right: Some(1),
        };
        let first = DiffCodeScrollContext {
            handle: states[row.file()].scroll.clone(),
            prefix: "first".into(),
        };
        let second = DiffCodeScrollContext {
            handle: states[split.file()].scroll.clone(),
            prefix: "second".into(),
        };
        first
            .slot("unified")
            .handle
            .set_offset(gpui::Point::new(px(-96.0), px(0.0)));
        assert_eq!(states[folding.file()].scroll.offset().x, px(-96.0));
        assert_eq!(second.slot("old").handle.offset(), gpui::Point::default());

        second
            .slot("old")
            .handle
            .set_offset(gpui::Point::new(px(-48.0), px(0.0)));
        assert_eq!(second.slot("new").handle.offset().x, px(-48.0));
        assert_eq!(first.slot("unified").handle.offset().x, px(-96.0));
    }

    #[test]
    fn horizontal_scroll_reset_returns_to_origin() {
        let handle = gpui::ScrollHandle::new();
        handle.set_offset(gpui::Point::new(px(-120.0), px(-18.0)));

        reset_horizontal_scroll(&handle);

        assert_eq!(handle.offset(), gpui::Point::default());
    }

    #[test]
    fn horizontal_scroll_slots_share_offset_but_keep_unique_ids() {
        let context = DiffCodeScrollContext {
            handle: gpui::ScrollHandle::new(),
            prefix: "row-7".into(),
        };
        let old = context.slot("old");
        let new = context.slot("new");

        old.handle.set_offset(gpui::Point::new(px(-96.0), px(0.0)));

        assert_eq!(new.handle.offset(), old.handle.offset());
        assert_ne!(old.id, new.id);
    }

    #[test]
    fn body_height_is_analytic() {
        let files = parse_patch(PATCH);
        let main = &files[0];
        let lines: usize = main.hunks.iter().map(|h| h.lines.len()).sum();
        assert_eq!(
            body_height(main),
            2.0 * HUNK_HEADER_HEIGHT + lines as f32 * DIFF_LINE_HEIGHT + BODY_BOTTOM_PAD
        );
        // Notices add height (added file: 1 notice + meta line inside hunk).
        let added = &files[1];
        assert_eq!(
            body_height(added),
            NOTICE_HEIGHT + HUNK_HEADER_HEIGHT + 3.0 * DIFF_LINE_HEIGHT + BODY_BOTTOM_PAD
        );
    }

    fn diff(checkout: &str, device: &str, cwd: &str, patch: &str) -> CheckoutDiff {
        CheckoutDiff {
            checkout_id: checkout.into(),
            device_id: device.into(),
            cwd: cwd.into(),
            patch: patch.into(),
            files: Vec::new(),
            additions: 0,
            deletions: 0,
            truncated: false,
            checksum: format!("sum-{}", patch.len()),
            updated_at: Utc::now(),
        }
    }

    fn chat(checkout: Option<&str>, device: &str, cwd: Option<&str>) -> Chat {
        Chat {
            id: "c1".into(),
            device_id: device.into(),
            title: None,
            archived: false,
            cwd: cwd.map(Into::into),
            branch: None,
            checkout_id: checkout.map(Into::into),
            source_context: None,
            config: None,
            last_message_preview: None,
            last_message_at: None,
            created_at: Utc::now(),
            harness_session_id: None,
            harness_session_cwd: None,
            space_id: None,
            last_seen_at: None,
            room_gen: None,
        }
    }

    #[test]
    fn diff_resolution_prefers_checkout_id_then_cwd() {
        let diffs = vec![
            diff("co-1", "dev-a", "/repo/one", "x"),
            diff("co-2", "dev-b", "/repo/two", "y"),
        ];
        // checkout_id match wins even when cwd points elsewhere.
        let c = chat(Some("co-2"), "dev-a", Some("/repo/one"));
        assert_eq!(resolve_diff(&diffs, &c).unwrap().checkout_id, "co-2");
        // Unknown checkout falls back to device+cwd.
        let c = chat(Some("co-9"), "dev-a", Some("/repo/one"));
        assert_eq!(resolve_diff(&diffs, &c).unwrap().checkout_id, "co-1");
        // Wrong device still matches by cwd alone.
        let c = chat(None, "dev-z", Some("/repo/two"));
        assert_eq!(resolve_diff(&diffs, &c).unwrap().checkout_id, "co-2");
        // Nothing to go on.
        let c = chat(None, "dev-a", None);
        assert!(resolve_diff(&diffs, &c).is_none());
        let c = chat(None, "dev-a", Some("/elsewhere"));
        assert!(resolve_diff(&diffs, &c).is_none());
    }

    #[test]
    fn phases() {
        assert_eq!(diff_phase(None), DiffPhase::Preparing);
        let clean = diff("co", "d", "/w", "  \n");
        assert_eq!(diff_phase(Some(&clean)), DiffPhase::Clean);
        let full = diff("co", "d", "/w", "diff --git a/x b/x\n");
        assert_eq!(diff_phase(Some(&full)), DiffPhase::List);
        // Engine may report files without patch text (truncation edge).
        let mut summarized = diff("co", "d", "/w", "");
        summarized.files.push(zeron_proto::DiffFileSummary {
            path: "x".into(),
            old_path: None,
            status: "modified".into(),
            additions: 1,
            deletions: 0,
            binary: false,
        });
        assert_eq!(diff_phase(Some(&summarized)), DiffPhase::List);
    }

    #[test]
    fn header_label_pluralizes() {
        assert_eq!(uncommitted_label(0), "0 Uncommitted changes");
        assert_eq!(uncommitted_label(1), "1 Uncommitted change");
        assert_eq!(uncommitted_label(4), "4 Uncommitted changes");
    }

    #[test]
    fn scope_labels_and_clean_messages() {
        assert_eq!(
            scope_label(DiffScope::WorkingTree, 2, None),
            "2 Uncommitted changes"
        );
        assert_eq!(
            scope_label(DiffScope::Branch, 1, Some("main")),
            "1 Changed file vs main"
        );
        assert_eq!(scope_label(DiffScope::Branch, 3, None), "3 Changed files");
        assert_eq!(
            scope_label(DiffScope::LatestTurn, 2, None),
            "2 Changed files this turn"
        );
        assert_eq!(
            clean_message(DiffScope::WorkingTree, None),
            "No uncommitted changes"
        );
        assert_eq!(
            clean_message(DiffScope::Branch, Some("develop")),
            "No changes vs develop"
        );
        assert_eq!(
            clean_message(DiffScope::LatestTurn, None),
            "No changes this turn"
        );
    }

    #[test]
    fn base_ref_defaults_to_repo_default_then_main() {
        let branches =
            |names: &[&str]| -> Vec<String> { names.iter().map(|n| n.to_string()).collect() };
        // Engine order puts the repo default first — take it when it isn't
        // the checked-out branch itself.
        let b = branches(&["main", "feature"]);
        assert_eq!(
            default_base_ref(&b, Some("feature")).as_deref(),
            Some("main")
        );
        // No origin/HEAD: engine "default" is the current branch — fall
        // through to main/master.
        let b = branches(&["feature", "main"]);
        assert_eq!(
            default_base_ref(&b, Some("feature")).as_deref(),
            Some("main")
        );
        let b = branches(&["feature", "master"]);
        assert_eq!(
            default_base_ref(&b, Some("feature")).as_deref(),
            Some("master")
        );
        // No main/master: any branch that isn't the current one.
        let b = branches(&["feature", "develop"]);
        assert_eq!(
            default_base_ref(&b, Some("feature")).as_deref(),
            Some("develop")
        );
        // Checked out ON main: comparing main with itself is the honest
        // default (empty branch diff).
        let b = branches(&["main", "feature"]);
        assert_eq!(default_base_ref(&b, Some("main")).as_deref(), Some("main"));
        // Single-branch repo, and empty list.
        let b = branches(&["main"]);
        assert_eq!(default_base_ref(&b, Some("main")).as_deref(), Some("main"));
        assert_eq!(default_base_ref(&[], Some("main")), None);
    }

    #[test]
    fn scope_modes_are_wire_stable() {
        // `mode` is the GetCheckoutDiff wire contract — engine matches on it.
        assert_eq!(DiffScope::WorkingTree.mode(), "workingTree");
        assert_eq!(DiffScope::Branch.mode(), "branch");
        assert_eq!(DiffScope::LatestTurn.mode(), "turn");
        assert_eq!(DiffScope::default(), DiffScope::WorkingTree);
    }

    #[test]
    fn diff_scope_menu_keeps_history_as_a_separate_surface() {
        assert_eq!(
            DiffScope::ALL,
            [
                DiffScope::WorkingTree,
                DiffScope::Branch,
                DiffScope::LatestTurn,
            ]
        );
        assert!(!DiffScope::ALL.contains(&DiffScope::History));
    }

    #[test]
    fn diff_frames_replace_lists_and_upsert_singles() {
        let mut diffs = Vec::new();
        let one = diff("co-1", "d", "/w", "p1");
        // Single frame inserts.
        assert!(apply_diff_frame(
            &mut diffs,
            serde_json::to_value(&one).unwrap()
        ));
        assert_eq!(diffs.len(), 1);
        // Identical frame is a no-op.
        assert!(!apply_diff_frame(
            &mut diffs,
            serde_json::to_value(&one).unwrap()
        ));
        // Same checkout upserts in place.
        let mut updated = one.clone();
        updated.patch = "p2".into();
        assert!(apply_diff_frame(
            &mut diffs,
            serde_json::to_value(&updated).unwrap()
        ));
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].patch, "p2");
        // List frame replaces wholesale.
        let two = diff("co-2", "d", "/x", "q");
        assert!(apply_diff_frame(
            &mut diffs,
            serde_json::to_value(vec![two.clone()]).unwrap()
        ));
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].checkout_id, "co-2");
        // Malformed frames change nothing.
        assert!(!apply_diff_frame(
            &mut diffs,
            serde_json::json!({"nope": true})
        ));
        assert_eq!(diffs[0].checkout_id, "co-2");
    }

    #[test]
    fn full_diff_highlights_map_old_new_and_context_by_source_line() {
        let old_source = "export function old(value: string) {\n    return value.trim();\n}\n";
        let new_source = "export function new(value: string) {\n    return value.trim();\n}\n";
        let parse = |source| {
            Arc::new(
                zeron_syntax::highlight(zeron_syntax::HighlightRequest {
                    source,
                    path: Some("src/derive.ts"),
                    fence_tag: None,
                })
                .unwrap(),
            )
        };
        let highlights = DiffHighlights {
            old: Some(parse(old_source)),
            new: Some(parse(new_source)),
        };
        let deleted = DiffLine {
            kind: LineKind::Del,
            old_no: Some(1),
            new_no: None,
            text: "export function old(value: string) {".into(),
        };
        let added = DiffLine {
            kind: LineKind::Add,
            old_no: None,
            new_no: Some(1),
            text: "export function new(value: string) {".into(),
        };
        let context = DiffLine {
            kind: LineKind::Context,
            old_no: Some(2),
            new_no: Some(2),
            text: "    return value.trim();".into(),
        };
        assert_eq!(
            highlights.source_ref(&deleted),
            Some(SourceLineRef {
                side: SourceSide::Old,
                line_number: 1
            })
        );
        assert_eq!(
            highlights.source_ref(&added),
            Some(SourceLineRef {
                side: SourceSide::New,
                line_number: 1
            })
        );
        assert_eq!(
            highlights.source_ref(&context),
            Some(SourceLineRef {
                side: SourceSide::New,
                line_number: 2
            })
        );
        assert!(
            highlights
                .spans(&deleted)
                .iter()
                .any(|span| span.kind == zeron_syntax::HighlightKind::Function)
        );
        assert!(
            highlights
                .spans(&added)
                .iter()
                .any(|span| span.kind == zeron_syntax::HighlightKind::Function)
        );
    }

    /// The regression this guards: rendering the diff at the raw shared
    /// setting silently enlarged it from 12.0 to 12.5 on a fresh install.
    #[test]
    fn the_default_code_font_size_reproduces_the_historical_diff_size() {
        let theme = Theme::dark();
        assert_eq!(
            theme.code_font_size,
            crate::typography::CODE_FONT_SIZE_DEFAULT
        );
        assert_eq!(diff_text_size(&theme), DIFF_TEXT_SIZE);
        assert_eq!(diff_line_height(&theme), DIFF_LINE_HEIGHT);
    }

    #[test]
    fn scaled_diff_sizes_keep_their_proportions_and_stay_clamped() {
        let mut theme = Theme::dark();
        theme.code_font_size = 2.0 * crate::typography::CODE_FONT_SIZE_DEFAULT;
        assert_eq!(diff_text_size(&theme), 2.0 * DIFF_TEXT_SIZE);
        assert_eq!(diff_line_height(&theme), 2.0 * DIFF_LINE_HEIGHT);

        theme.code_font_size = crate::typography::FONT_SIZE_MAX;
        assert!(diff_text_size(&theme) <= crate::typography::FONT_SIZE_MAX);
        assert!(diff_text_size(&theme) >= crate::typography::FONT_SIZE_MIN);
    }

    #[test]
    fn split_line_runs_use_affected_old_and_new_documents() {
        let theme = Theme::dark();
        for (path, source, required) in [
            (
                "src/card.tsx",
                "const view: JSX.Element = <main id=\"app\" />;",
                zeron_syntax::HighlightKind::Tag,
            ),
            (
                "src/Greeter.kt",
                "fun greet(name: String) = println(name)",
                zeron_syntax::HighlightKind::Function,
            ),
            (
                "Dockerfile",
                "RUN echo \"hello\"",
                zeron_syntax::HighlightKind::Function,
            ),
        ] {
            let document = Arc::new(
                zeron_syntax::highlight(zeron_syntax::HighlightRequest {
                    source,
                    path: Some(path),
                    fence_tag: None,
                })
                .unwrap(),
            );
            let highlights = DiffHighlights {
                old: Some(document.clone()),
                new: Some(document),
            };
            for line in [
                DiffLine {
                    kind: LineKind::Del,
                    old_no: Some(1),
                    new_no: None,
                    text: source.into(),
                },
                DiffLine {
                    kind: LineKind::Add,
                    old_no: None,
                    new_no: Some(1),
                    text: source.into(),
                },
            ] {
                assert!(
                    highlights
                        .spans(&line)
                        .iter()
                        .any(|span| span.kind == required),
                    "missing {required:?} for {path} on {:?}",
                    line.kind
                );
                let runs = line_runs(&line, Some(&highlights), &theme);
                assert_eq!(runs.iter().map(|run| run.len).sum::<usize>(), source.len());
                assert!(
                    runs.iter()
                        .any(|run| run.color == render::token_color(required, &theme)),
                    "split runs dropped {required:?} for {path} on {:?}",
                    line.kind
                );
            }
        }
    }

    #[test]
    fn excerpt_parses_old_and_new_hunks_as_separate_documents() {
        let file = FileDiff {
            path: "src/lib.rs".into(),
            old_path: None,
            status: FileStatus::Modified,
            binary: false,
            notices: vec![],
            hunks: vec![Hunk {
                header: "@@ -1,3 +1,3 @@".into(),
                lines: vec![
                    DiffLine {
                        kind: LineKind::Context,
                        old_no: Some(1),
                        new_no: Some(1),
                        text: "/* start".into(),
                    },
                    DiffLine {
                        kind: LineKind::Del,
                        old_no: Some(2),
                        new_no: None,
                        text: "old body".into(),
                    },
                    DiffLine {
                        kind: LineKind::Add,
                        old_no: None,
                        new_no: Some(2),
                        text: "new body".into(),
                    },
                    DiffLine {
                        kind: LineKind::Context,
                        old_no: Some(3),
                        new_no: Some(3),
                        text: "end */".into(),
                    },
                ],
            }],
            additions: 1,
            deletions: 1,
            max_line: 3,
        };
        let highlights = excerpt_highlights(&file, Lang::Rust).expect("excerpt");
        let deleted = &file.hunks[0].lines[1];
        let added = &file.hunks[0].lines[2];
        assert!(
            highlights
                .spans(deleted)
                .iter()
                .any(|span| span.kind == zeron_syntax::HighlightKind::Comment)
        );
        assert!(
            highlights
                .spans(added)
                .iter()
                .any(|span| span.kind == zeron_syntax::HighlightKind::Comment)
        );
    }

    #[test]
    fn mismatched_full_sources_are_rejected_atomically() {
        let file = FileDiff {
            path: "src/lib.rs".into(),
            old_path: None,
            status: FileStatus::Modified,
            binary: false,
            notices: vec![],
            hunks: vec![Hunk {
                header: "@@ -1 +1 @@".into(),
                lines: vec![
                    DiffLine {
                        kind: LineKind::Del,
                        old_no: Some(1),
                        new_no: None,
                        text: "let old = 1;".into(),
                    },
                    DiffLine {
                        kind: LineKind::Add,
                        old_no: None,
                        new_no: Some(1),
                        text: "let new = 2;".into(),
                    },
                ],
            }],
            additions: 1,
            deletions: 1,
            max_line: 1,
        };
        let response = zeron_proto::CheckoutFileDiffText {
            diff_checksum: "sum".into(),
            old_text: Some("let old = 1;\n".into()),
            new_text: Some("different snapshot\n".into()),
            old_content_hash: None,
            new_content_hash: None,
            binary: false,
            truncated: false,
            stale: false,
        };
        assert!(!sources_match_patch(&file, &response));
        assert!(full_highlights(&file, Lang::Rust, &response).is_none());
    }
