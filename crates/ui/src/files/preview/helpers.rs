use super::*;

pub(super) fn estimated_highlighted_file_bytes(highlight: &HighlightedFile) -> usize {
    let document = &highlight.document;
    std::mem::size_of::<HighlightedFile>()
        .saturating_add(highlight.content_hash.capacity())
        .saturating_add(
            document
                .lines
                .capacity()
                .saturating_mul(std::mem::size_of::<Vec<skylark_syntax::HighlightSpan>>()),
        )
        .saturating_add(document.lines.iter().fold(0usize, |total, line| {
            total.saturating_add(
                line.capacity()
                    .saturating_mul(std::mem::size_of::<skylark_syntax::HighlightSpan>()),
            )
        }))
}

pub(super) fn document_key(context: &FilesRequestContext, path: String) -> DocumentKey {
    DocumentKey {
        chat_id: context.target.chat_id.clone().unwrap_or_default(),
        checkout_id: context.checkout_id.clone(),
        path,
    }
}

pub(super) fn document_blocks_lifecycle(document: &FileDocument) -> bool {
    document.is_dirty()
        && matches!(
            document.phase,
            DocumentPhase::SaveFailed(_)
                | DocumentPhase::Conflict { .. }
                | DocumentPhase::ExternallyModified { .. }
                | DocumentPhase::DeletedOnDisk
        )
}

pub(super) fn document_finishes_review_comment_flush(document: &FileDocument) -> bool {
    (!document.is_dirty() && matches!(document.phase, DocumentPhase::Ready))
        || matches!(
            document.phase,
            DocumentPhase::SaveFailed(_) | DocumentPhase::Conflict { .. }
        )
}

pub(super) fn comment_anchor_range(
    text: &gpui_base::input::Rope,
    line: u32,
) -> Option<(std::ops::Range<usize>, CommentAnchorEdge)> {
    let line = line.saturating_sub(1) as usize;
    if line >= text.lines_len() {
        return None;
    }
    let start = text.line_start_offset(line);
    let end = if line + 1 < text.lines_len() {
        text.line_start_offset(line + 1)
    } else {
        text.len()
    };
    if start < end {
        Some((start..end, CommentAnchorEdge::Start))
    } else if start > 0 {
        // The trailing empty line has no byte of its own. Track the newline
        // immediately before it and resolve the anchor from the range's end.
        Some((start - 1..start, CommentAnchorEdge::End))
    } else {
        None
    }
}

pub(super) fn tracked_comment_line(
    text: &gpui_base::input::Rope,
    range: &std::ops::Range<usize>,
    edge: CommentAnchorEdge,
) -> u32 {
    let offset = match edge {
        CommentAnchorEdge::Start => range.start,
        CommentAnchorEdge::End => range.end,
    }
    .min(text.len());
    (text.offset_to_point(offset).row + 1) as u32
}

pub(super) fn editor_overlay_layout(
    editor: &crate::files::editor::FileEditorState,
) -> Option<EditorOverlayLayout> {
    let visible = editor.visible_row_range()?;
    let input_bounds = editor.input_bounds();
    let text_bounds = editor.text_bounds()?;
    let line_height = f32::from(editor.line_height()?);
    let text = editor.text();
    let mut rows = Vec::with_capacity(visible.len());
    let mut gutter_width = None;
    for line in visible {
        if line >= text.lines_len() {
            continue;
        }
        let offset = text.line_start_offset(line);
        let Some(bounds) = editor.range_to_bounds(&(offset..offset)) else {
            continue;
        };
        gutter_width.get_or_insert_with(|| {
            f32::from(bounds.origin.x - text_bounds.origin.x).clamp(24.0, 64.0)
        });
        rows.push(EditorOverlayRow {
            line: (line + 1) as u32,
            top: f32::from(bounds.origin.y - input_bounds.origin.y),
        });
    }
    Some(EditorOverlayLayout {
        gutter_width: gutter_width.unwrap_or(36.0),
        line_height,
        viewport_width: f32::from(input_bounds.size.width),
        viewport_height: f32::from(input_bounds.size.height),
        rows,
    })
}

pub(super) fn file_highlight_result_is_current(
    document: &FileDocument,
    document_key: &DocumentKey,
    generation: u64,
    revision: u64,
    content_hash: &str,
) -> bool {
    document.accepts(document_key, generation)
        && document.revision == revision
        && document.content_hash() == Some(content_hash)
}

pub(super) fn renamed_document_path(path: &str, old_path: &str, new_path: &str) -> Option<String> {
    if path == old_path {
        Some(new_path.to_string())
    } else {
        path.strip_prefix(&format!("{old_path}/"))
            .map(|suffix| format!("{new_path}/{suffix}"))
    }
}

pub(super) fn path_is_same_or_descendant(path: &str, ancestor: &str) -> bool {
    path == ancestor || path.starts_with(&format!("{ancestor}/"))
}
