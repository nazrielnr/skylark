//! File-preview unit tests.

use super::*;
use super::*;

// Rendering a preview needs a window, so this asserts on `line_height`,
// the single source both the uniform-height hint passed to
// `reset_with_uniform_height` and the painted row in
// `render_preview_line` read.
#[test]
fn preview_rows_track_the_code_font_size_from_the_default_to_the_maximum() {
    let default = FilePreviewState::new(false, 900, false, 12.5);
    assert_eq!(default.line_height(), px(PREVIEW_LINE_HEIGHT));

    let maximum = FilePreviewState::new(false, 900, false, 32.0);
    assert!(maximum.line_height() > px(PREVIEW_LINE_HEIGHT));

    let minimum = FilePreviewState::new(false, 900, false, 8.0);
    assert_eq!(minimum.line_height(), px(PREVIEW_LINE_HEIGHT));
}

/// The regression this guards: collapsing both surfaces onto the raw
/// setting silently resized them on a fresh install.
#[test]
fn the_default_code_font_size_reproduces_the_historical_per_surface_sizes() {
    let default =
        FilePreviewState::new(false, 900, false, crate::typography::CODE_FONT_SIZE_DEFAULT);
    assert_eq!(default.editor_text_size(), 13.0);
    assert_eq!(default.preview_text_size(), 11.5);
}

#[test]
fn scaled_preview_sizes_keep_their_proportions_and_stay_clamped() {
    let doubled = FilePreviewState::new(
        false,
        900,
        false,
        2.0 * crate::typography::CODE_FONT_SIZE_DEFAULT,
    );
    assert_eq!(doubled.editor_text_size(), 26.0);
    assert_eq!(doubled.preview_text_size(), 23.0);

    // The editor ratio is >1, so the maximum setting would overshoot.
    let maximum = FilePreviewState::new(false, 900, false, crate::typography::FONT_SIZE_MAX);
    assert_eq!(maximum.editor_text_size(), crate::typography::FONT_SIZE_MAX);
    assert!(maximum.preview_text_size() < crate::typography::FONT_SIZE_MAX);
}

#[test]
fn tree_split_uses_the_standard_resize_geometry_and_limits() {
    assert_eq!(TREE_SPLIT_HITBOX_HALF_WIDTH * 2.0, 20.0);
    let min = crate::motion::resize_drag_sample(
        TREE_SPLIT_MIN - 1.0,
        TREE_SPLIT_MIN,
        TREE_SPLIT_MAX,
        None,
        false,
    );
    let max = crate::motion::resize_drag_sample(
        TREE_SPLIT_MAX + 1.0,
        TREE_SPLIT_MIN,
        TREE_SPLIT_MAX,
        None,
        false,
    );
    assert_eq!(min.width, TREE_SPLIT_MIN);
    assert_eq!(min.edge, Some(crate::motion::ResizeEdge::Min));
    assert!(min.starts_bounce);
    assert_eq!(max.width, TREE_SPLIT_MAX);
    assert_eq!(max.edge, Some(crate::motion::ResizeEdge::Max));
    assert!(max.starts_bounce);
}

fn cached_document(path: &str, text: &str) -> FileDocument {
    let mut document = FileDocument::loading(DocumentKey {
        chat_id: "chat-1".into(),
        checkout_id: Some("checkout-1".into()),
        path: path.into(),
    });
    document.phase = DocumentPhase::Ready;
    document.lines = Arc::new(vec![SharedString::from(text.to_string())]);
    document
}

#[test]
fn read_only_reasons_have_specific_messages() {
    assert!(read_only_message(Some(WorkspaceReadOnlyReason::Binary)).contains("Binary"));
    assert!(
        read_only_message(Some(WorkspaceReadOnlyReason::UnsupportedEncoding)).contains("encoding")
    );
}

#[test]
fn reset_drops_documents_and_active_preview_from_the_previous_target() {
    let mut preview = FilePreviewState::new(false, 900, false, 11.5);
    preview.active = Some("private.env".into());
    preview.documents.insert(
        "private.env".into(),
        FileDocument::loading(DocumentKey {
            chat_id: "chat-1".into(),
            checkout_id: Some("checkout-1".into()),
            path: "private.env".into(),
        }),
    );
    preview
        .documents
        .get_mut("private.env")
        .unwrap()
        .mark_external(None);
    preview.tree_sidebar_visible = true;

    preview.reset();

    assert!(preview.documents.is_empty());
    assert!(preview.active.is_none());
    assert!(!preview.tree_sidebar_visible);
}

#[test]
fn document_cache_evicts_the_oldest_safe_entries() {
    let mut preview = FilePreviewState::new(false, 900, false, 11.5);
    for index in 0..18 {
        let path = format!("src/{index}.rs");
        preview
            .documents
            .insert(path.clone(), cached_document(&path, "fn main() {}"));
        preview.touch_document(&path);
    }
    preview.active = Some("src/0.rs".into());
    preview.documents.get_mut("src/1.rs").unwrap().revision = 1;
    preview.documents.get_mut("src/2.rs").unwrap().phase =
        DocumentPhase::Conflict { disk_hash: None };
    preview
        .documents
        .get_mut("src/3.rs")
        .unwrap()
        .review_comment_flush_pending = true;
    preview.documents.get_mut("src/7.rs").unwrap().phase = DocumentPhase::Loading;
    let protected = HashSet::from(["src/4.rs".to_string()]);

    let evicted = preview.trim_document_cache_to(&protected, 15, usize::MAX);

    assert_eq!(evicted, vec!["src/5.rs", "src/6.rs", "src/8.rs"]);
    for retained in ["src/0.rs", "src/1.rs", "src/2.rs", "src/3.rs", "src/4.rs"] {
        assert!(preview.documents.contains_key(retained));
    }
    assert!(preview.documents.contains_key("src/7.rs"));
}

#[test]
fn document_cache_uses_retained_bytes_not_only_entry_count() {
    let mut preview = FilePreviewState::new(false, 900, false, 11.5);
    for path in ["old.rs", "active.rs"] {
        preview
            .documents
            .insert(path.into(), cached_document(path, &"x".repeat(4 * 1024)));
        preview.touch_document(path);
    }
    preview.active = Some("active.rs".into());
    let active_bytes = preview.documents["active.rs"].estimated_retained_bytes();
    let total_bytes = preview.retained_document_bytes();

    let evicted =
        preview.trim_document_cache_to(&HashSet::new(), usize::MAX, total_bytes.saturating_sub(1));

    assert_eq!(evicted, vec!["old.rs"]);
    assert!(preview.retained_document_bytes() <= active_bytes);
}

#[test]
fn document_eviction_cleans_path_scoped_companion_state() {
    let mut preview = FilePreviewState::new(false, 900, false, 11.5);
    for path in ["old.rs", "active.rs"] {
        preview
            .documents
            .insert(path.into(), cached_document(path, "fn main() {}"));
        preview.touch_document(path);
    }
    preview.active = Some("active.rs".into());
    preview
        .comment_anchors
        .insert("old.rs".into(), HashMap::new());
    let highlighted = Arc::new(
        zeron_syntax::highlight(zeron_syntax::HighlightRequest {
            source: "fn main() {}",
            path: Some("old.rs"),
            fence_tag: None,
        })
        .unwrap(),
    );
    preview.highlights.insert(
        "old.rs".into(),
        HighlightedFile {
            content_hash: "hash".into(),
            document: highlighted,
        },
    );

    let evicted = preview.trim_document_cache_to(&HashSet::new(), 1, usize::MAX);

    assert_eq!(evicted, vec!["old.rs"]);
    assert!(!preview.highlights.contains_key("old.rs"));
    assert!(!preview.comment_anchors.contains_key("old.rs"));
    assert!(!preview.document_recency.iter().any(|path| path == "old.rs"));
}

#[test]
fn reset_preserves_global_editor_preferences() {
    let mut preview = FilePreviewState::new(false, 900, true, 15.0);

    preview.reset();

    assert!(!preview.autosave_enabled);
    assert!(preview.word_wrap());
    assert_eq!(preview.editor_font_size, 15.0);
}

#[test]
fn file_highlight_completion_rejects_a_result_after_a_user_edit() {
    let path = "src/lib.rs";
    let disk_hash = "disk-hash-1";
    let stale_source = "ab";
    let updated_source = "é";
    let mut document = FileDocument::loading(DocumentKey {
        chat_id: "chat-1".into(),
        checkout_id: Some("checkout-1".into()),
        path: path.into(),
    });
    document.set_loaded(zeron_proto::WorkspaceFileText {
        checkout_id: "checkout-1".into(),
        path: path.into(),
        text: Some(stale_source.into()),
        content_hash: Some(disk_hash.into()),
        size: stale_source.len() as u64,
        modified_at: None,
        encoding: zeron_proto::WorkspaceTextEncoding::Utf8,
        line_ending: Some(zeron_proto::WorkspaceLineEnding::Lf),
        read_only_reason: None,
        truncated: false,
    });
    let task_document_key = document.key.clone();
    let task_generation = document.generation;
    let task_revision = document.revision;
    let stale_highlight_key =
        DocumentHighlightKey::new(zeron_syntax::LanguageId::Rust, stale_source);

    assert!(file_highlight_result_is_current(
        &document,
        &task_document_key,
        task_generation,
        task_revision,
        disk_hash,
    ));

    document.mark_user_edit();
    let current_highlight_key =
        DocumentHighlightKey::new(zeron_syntax::LanguageId::Rust, updated_source);

    assert_ne!(document.revision, task_revision);
    assert_ne!(current_highlight_key, stale_highlight_key);
    assert!(
        !file_highlight_result_is_current(
            &document,
            &task_document_key,
            task_generation,
            task_revision,
            disk_hash,
        ),
        "highlight for revision {task_revision} was accepted at revision {} after the editor source changed from {stale_source:?} to {updated_source:?}",
        document.revision
    );
}

#[test]
fn autosave_is_opt_in_and_enabling_schedules_dirty_documents() {
    let path = "src/lib.rs";
    let mut preview = FilePreviewState::new(false, 900, false, 11.5);
    let mut document = FileDocument::loading(DocumentKey {
        chat_id: "chat-1".into(),
        checkout_id: Some("checkout-1".into()),
        path: path.into(),
    });
    document.set_loaded(zeron_proto::WorkspaceFileText {
        checkout_id: "checkout-1".into(),
        path: path.into(),
        text: Some("fn main() {}".into()),
        content_hash: Some("hash-1".into()),
        size: 12,
        modified_at: None,
        encoding: zeron_proto::WorkspaceTextEncoding::Utf8,
        line_ending: Some(zeron_proto::WorkspaceLineEnding::Lf),
        read_only_reason: None,
        truncated: false,
    });
    document.revision = 1;
    preview.documents.insert(path.into(), document);

    assert!(preview.set_autosave_delay_ms(600).is_empty());
    assert_eq!(preview.set_autosave_enabled(true), vec![path.to_string()]);
    assert!(preview.set_autosave_enabled(false).is_empty());
}

#[test]
fn sidebar_layout_changes_are_immediate_without_a_user_toggle() {
    let mut preview = FilePreviewState::new(false, 900, false, 11.5);
    let now = Instant::now();
    assert_eq!(
        preview
            .tree_motion
            .sample(preview.tree_sidebar_visible(), now, false),
        (0.0, false)
    );
    // A newly opened surface measures its width after the first render.
    preview.surface_width.set(WIDE_BREAKPOINT);
    assert_eq!(
        preview
            .tree_motion
            .sample(preview.tree_sidebar_visible(), now, false),
        (1.0, false)
    );
    preview.surface_width.set(WIDE_BREAKPOINT - 1.0);
    assert_eq!(
        preview
            .tree_motion
            .sample(preview.tree_sidebar_visible(), now, false),
        (0.0, false)
    );
    preview.show_tree_sidebar();
    assert_eq!(
        preview
            .tree_motion
            .sample(preview.tree_sidebar_visible(), now, false),
        (1.0, false)
    );
    preview.toggle_tree_sidebar();
    let started = preview.tree_motion.started.unwrap();
    assert_eq!(
        preview
            .tree_motion
            .sample(preview.tree_sidebar_visible(), started, false),
        (1.0, true)
    );
}

#[test]
fn sidebar_motion_reverses_from_its_current_width() {
    let mut motion = TreeSidebarMotion::default();
    let now = Instant::now();
    assert_eq!(motion.sample(true, now, false), (1.0, false));
    motion.animate_to(true, false, now);
    assert_eq!(motion.sample(false, now, false), (1.0, true));
    let midway = now
        + crate::motion::RESIZE
            .total()
            .mul_f32(crate::motion::speed_scale() * 0.4);
    let closing = motion.sample(false, midway, false).0;
    assert!(closing > 0.0 && closing < 1.0);
    motion.animate_to(false, true, midway);
    assert_eq!(motion.sample(true, midway, false), (closing, true));
    assert_eq!(
        motion.sample(true, midway + Duration::from_secs(10), false),
        (1.0, false)
    );
    motion.animate_to(true, false, midway + Duration::from_secs(10));
    assert_eq!(
        motion.sample(false, midway + Duration::from_secs(20), false),
        (0.0, false)
    );
}

#[test]
fn sidebar_motion_snaps_when_reduced_motion_is_enabled() {
    let mut motion = TreeSidebarMotion::default();
    let now = Instant::now();
    motion.sample(true, now, false);
    motion.animate_to(true, false, now);
    assert_eq!(motion.sample(false, now, true), (0.0, false));
    assert_eq!(motion.sample(true, now, true), (1.0, false));
    assert_eq!(motion.sample(true, now, false), (1.0, false));
}

#[test]
fn wide_layout_respects_an_explicitly_hidden_tree_sidebar() {
    let mut preview = FilePreviewState::new(false, 900, false, 11.5);
    preview.surface_width.set(WIDE_BREAKPOINT);

    assert!(preview.tree_sidebar_visible());

    preview.toggle_tree_sidebar();
    assert!(!preview.tree_sidebar_visible());

    preview.surface_width.set(WIDE_BREAKPOINT - 1.0);
    preview.surface_width.set(WIDE_BREAKPOINT);
    assert!(!preview.tree_sidebar_visible());
}

#[test]
fn explicitly_showing_tree_sidebar_clears_responsive_dismissal() {
    let mut preview = FilePreviewState::new(false, 900, false, 11.5);
    preview.surface_width.set(WIDE_BREAKPOINT);
    preview.toggle_tree_sidebar();

    preview.show_tree_sidebar();

    assert!(preview.tree_sidebar_visible());
    preview.tree_sidebar_visible = false;
    assert!(preview.tree_sidebar_visible());
}

#[test]
fn dirty_reload_waits_for_explicit_discard_confirmation() {
    let path = "src/lib.rs";
    let mut preview = FilePreviewState::new(false, 900, false, 11.5);
    let mut document = FileDocument::loading(DocumentKey {
        chat_id: "chat-1".into(),
        checkout_id: Some("checkout-1".into()),
        path: path.into(),
    });
    document.revision = 1;
    preview.documents.insert(path.into(), document);

    assert_eq!(
        preview.request_reload(path),
        ReloadDecision::AwaitDiscardConfirmation
    );
    assert_eq!(preview.reload_confirmation.as_deref(), Some(path));
    assert!(preview.documents[path].is_dirty());

    // Repeated toolbar clicks must not bypass the explicit confirmation.
    assert_eq!(
        preview.request_reload(path),
        ReloadDecision::AwaitDiscardConfirmation
    );
}

#[test]
fn clean_reload_proceeds_without_confirmation() {
    let path = "src/lib.rs";
    let mut preview = FilePreviewState::new(false, 900, false, 11.5);
    preview.documents.insert(
        path.into(),
        FileDocument::loading(DocumentKey {
            chat_id: "chat-1".into(),
            checkout_id: Some("checkout-1".into()),
            path: path.into(),
        }),
    );
    preview.reload_confirmation = Some("stale.rs".into());

    assert_eq!(preview.request_reload(path), ReloadDecision::ReloadNow);
    assert!(preview.reload_confirmation.is_none());
}

#[test]
fn failed_save_blocks_lifecycle_until_explicit_recovery() {
    let mut document = FileDocument::loading(DocumentKey {
        chat_id: "chat-1".into(),
        checkout_id: Some("checkout-1".into()),
        path: "src/lib.rs".into(),
    });
    document.revision = 1;
    document.phase = DocumentPhase::SaveFailed("offline".into());

    assert!(document_blocks_lifecycle(&document));

    document.phase = DocumentPhase::Ready;
    assert!(!document_blocks_lifecycle(&document));
}

#[test]
fn terminal_save_outcomes_release_review_comment_flushes() {
    let mut document = FileDocument::loading(DocumentKey {
        chat_id: "chat-1".into(),
        checkout_id: Some("checkout-1".into()),
        path: "src/lib.rs".into(),
    });
    document.revision = 1;
    document.review_comment_flush_pending = true;

    document.phase = DocumentPhase::Saving;
    assert!(!document_finishes_review_comment_flush(&document));

    document.phase = DocumentPhase::SaveFailed("offline".into());
    assert!(document_finishes_review_comment_flush(&document));

    document.phase = DocumentPhase::Conflict { disk_hash: None };
    assert!(document_finishes_review_comment_flush(&document));

    document.phase = DocumentPhase::Ready;
    assert!(!document_finishes_review_comment_flush(&document));

    document.saved_revision = document.revision;
    assert!(document_finishes_review_comment_flush(&document));
}

#[test]
fn directory_rename_updates_open_descendant_paths() {
    assert_eq!(
        renamed_document_path("src/files/mod.rs", "src", "crates/ui/src"),
        Some("crates/ui/src/files/mod.rs".into())
    );
    assert_eq!(
        renamed_document_path("src-old/lib.rs", "src", "crates/ui/src"),
        None
    );
}

#[test]
fn document_paths_match_recreated_files_and_directory_descendants() {
    assert!(path_is_same_or_descendant("src/lib.rs", "src/lib.rs"));
    assert!(path_is_same_or_descendant("src/files/mod.rs", "src"));
    assert!(!path_is_same_or_descendant("src-old/lib.rs", "src"));
}

#[test]
fn comment_ranges_anchor_normal_and_trailing_empty_lines() {
    let text = gpui_base::input::Rope::from("one\ntwo\n");
    let (second, second_edge) = comment_anchor_range(&text, 2).unwrap();
    assert_eq!(second, 4..8);
    assert_eq!(second_edge, CommentAnchorEdge::Start);
    assert_eq!(tracked_comment_line(&text, &second, second_edge), 2);

    let (trailing, trailing_edge) = comment_anchor_range(&text, 3).unwrap();
    assert_eq!(trailing, 7..8);
    assert_eq!(trailing_edge, CommentAnchorEdge::End);
    assert_eq!(tracked_comment_line(&text, &trailing, trailing_edge), 3);
}

#[test]
fn comment_overlay_stays_inside_the_editor_viewport() {
    let layout = EditorOverlayLayout {
        gutter_width: 32.0,
        line_height: 20.0,
        viewport_width: 420.0,
        viewport_height: 100.0,
        rows: vec![EditorOverlayRow { line: 5, top: 80.0 }],
    };
    assert_eq!(editor_comment_overlay_top(&layout, 5, 60.0), Some(40.0));
    assert_eq!(editor_comment_overlay_top(&layout, 6, 60.0), None);
}

#[test]
fn comment_overlay_is_compact_when_space_allows_and_inset_when_narrow() {
    let mut layout = EditorOverlayLayout {
        gutter_width: 40.0,
        line_height: 20.0,
        viewport_width: 480.0,
        viewport_height: 300.0,
        rows: Vec::new(),
    };
    assert_eq!(editor_comment_overlay_horizontal(&layout), (32.0, 320.0));

    layout.viewport_width = 210.0;
    assert_eq!(editor_comment_overlay_horizontal(&layout), (8.0, 194.0));
}
