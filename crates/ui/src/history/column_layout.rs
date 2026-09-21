use super::*;

pub(super) fn history_column_label(column: GitHistoryColumn) -> &'static str {
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

pub(super) fn visible_history_columns(
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

pub(super) fn history_data_column(column: GitHistoryColumn) -> HistoryDataColumn {
    match column {
        GitHistoryColumn::Author => HistoryDataColumn::Author,
        GitHistoryColumn::Date => HistoryDataColumn::Date,
        GitHistoryColumn::Sha => HistoryDataColumn::Sha,
    }
}

pub(super) fn history_optional_width(
    column: GitHistoryColumn,
    widths: GitHistoryColumnWidths,
) -> f32 {
    history_column_width(history_data_column(column), widths)
}

pub(super) fn history_column_drop_index(
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

pub(super) fn reordered_history_columns(
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

pub(super) fn history_column_width(
    column: HistoryDataColumn,
    widths: GitHistoryColumnWidths,
) -> f32 {
    match column {
        HistoryDataColumn::Commit => HISTORY_COMMIT_SUBJECT_MIN_WIDTH,
        HistoryDataColumn::Author => widths.author,
        HistoryDataColumn::Date => widths.date,
        HistoryDataColumn::Sha => widths.sha,
    }
}

pub(super) fn history_column_limits(column: HistoryDataColumn) -> (f32, f32) {
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

pub(super) fn set_history_column_width(
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

pub(super) fn resized_history_column_widths(
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
