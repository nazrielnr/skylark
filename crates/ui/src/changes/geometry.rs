use super::*;
use gpui::{font, px};

pub struct DiffHorizontalGeometry {
    pub max_code_columns: usize,
    pub max_gutter_width: f32,
}

impl DiffHorizontalGeometry {
    pub fn from_file(file: &FileDiff) -> Self {
        let max_code_columns = file
            .hunks
            .iter()
            .flat_map(|hunk| &hunk.lines)
            .map(|line| visual_columns(&line.text))
            .max()
            .unwrap_or(0);
        let max_gutter_width = gutter_width(file);
        Self {
            max_code_columns,
            max_gutter_width,
        }
    }
}

/// Measure the same runs the row paints. Color boundaries can break kerning
/// and ligatures on native platforms even when every run uses the same font.
pub fn max_shaped_text_width(
    file: &FileDiff,
    highlights: Option<&DiffHighlights>,
    theme: &Theme,
    text_system: &gpui::WindowTextSystem,
) -> f32 {
    let mono = font(theme.font_mono.clone());
    let size = px(diff_text_size(theme));
    let column_width = text_system
        .ch_advance(text_system.resolve_font(&mono), size)
        .unwrap_or(size * 0.6)
        .as_f32();
    file.hunks
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .fold(0.0f32, |widest, line| {
            let runs = line_runs(line, highlights, theme);
            let shaped = text_system
                .shape_line(line.text.clone().into(), size, &runs, None)
                .width()
                .as_f32();
            // Preserve the old column estimate as a floor, including tab stops.
            widest
                .max(shaped)
                .max(visual_columns(&line.text) as f32 * column_width)
        })
}

/// Count terminal-style display columns, including tab stops and wide
/// Unicode glyphs. This is only a floor; actual shaped runs determine the extent.
pub fn visual_columns(text: &str) -> usize {
    text.chars().fold(0usize, |columns, ch| {
        if ch == '\t' {
            columns + (DIFF_TAB_SIZE - columns % DIFF_TAB_SIZE)
        } else {
            columns + ch.width().unwrap_or(0)
        }
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiffHorizontalMetrics {
    pub max_text_width: f32,
    pub max_gutter_width: f32,
}

impl DiffHorizontalMetrics {
    /// Compensating for the file-local gutter keeps every unified code
    /// viewport's effective scroll range identical.
    pub fn unified_content_width(self, gutter_width: f32) -> f32 {
        self.max_text_width
            + UNIFIED_CODE_PADDING_LEFT
            + CODE_PADDING_RIGHT
            + 2.0 * (self.max_gutter_width - gutter_width)
    }

    /// Split has one gutter per half. Both halves use this same extent so old
    /// and new remain synchronized even when one side is a filler.
    pub fn split_content_width(self, gutter_width: f32) -> f32 {
        self.max_text_width
            + SPLIT_CODE_PADDING_LEFT
            + CODE_PADDING_RIGHT
            + (self.max_gutter_width - gutter_width)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DiffCodeWidth {
    /// Inline tool diffs keep their existing local clipping behavior.
    Clipped,
    /// Changes rows expose a stable intrinsic code width.
    Scrollable(DiffHorizontalMetrics),
    /// Changes rows consume their viewport width and grow vertically.
    Wrapped,
}

#[derive(Clone)]
pub struct DiffCodeScroll {
    pub handle: gpui::ScrollHandle,
    pub id: SharedString,
}

#[derive(Clone)]
pub struct DiffCodeScrollContext {
    pub handle: gpui::ScrollHandle,
    pub prefix: SharedString,
}

impl DiffCodeScrollContext {
    pub fn slot(&self, suffix: impl std::fmt::Display) -> DiffCodeScroll {
        DiffCodeScroll {
            handle: self.handle.clone(),
            id: SharedString::from(format!("{}-{suffix}", self.prefix)),
        }
    }
}

pub fn reset_horizontal_scroll(handle: &gpui::ScrollHandle) {
    handle.set_offset(gpui::Point::default());
}
