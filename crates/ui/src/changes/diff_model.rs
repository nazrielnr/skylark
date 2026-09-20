use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    font, px, Entity, FocusHandle, SharedString, Subscription, Task,
};
use unicode_width::UnicodeWidthChar as _;

pub use zeron_proto::Chat;
use zeron_proto::CheckoutDiff;
pub use zeron_syntax::LanguageId as Lang;

pub use crate::comments::{self, CommentSide, ReviewComment};
use crate::composer::ComposerInput;
use crate::markdown::render;
use crate::theme::Theme;

use super::parser::file_notices;
use super::split::{line_anchor, pair_anchors, split_pairs};

// ---------------------------------------------------------------------------
// Layout numbers (analytic — they drive the fold tween)
// ---------------------------------------------------------------------------

pub const FILE_HEADER_HEIGHT: f32 = crate::surface_chrome::HEADER_HEIGHT;
pub const STICKY_FILE_HEADER_BLUR: f32 = 16.0;
/// Coverage of the theme's content-plane tint over the sticky header blur.
/// Light needs substantially more coverage: dark text is much more vulnerable
/// to rows ghosting through the blur than light text is on a dark tint.
pub const STICKY_FILE_HEADER_TINT_ALPHA_DARK: f32 = 0.40;
pub const STICKY_FILE_HEADER_TINT_ALPHA_LIGHT: f32 = 0.85;
pub const HUNK_HEADER_HEIGHT: f32 = 28.0;
pub const DIFF_LINE_HEIGHT: f32 = 21.0;
pub const NOTICE_HEIGHT: f32 = 24.0;
pub const BODY_BOTTOM_PAD: f32 = 8.0;
/// Gutter width per line-number column.
pub const GUTTER_WIDTH: f32 = 36.0;
/// The +/−/· marker column between the gutters and the code.
pub const MARKER_WIDTH: f32 = 28.0;
/// Width of the coloured accent bar on the left edge of +/− rows.
pub const ACCENT_BAR_WIDTH: f32 = 3.0;
/// The marker column in split mode: each half pays for its own, so it is
/// narrower than [`MARKER_WIDTH`] to leave the code the room.
pub const SPLIT_MARKER_WIDTH: f32 = 18.0;
/// Hairline between the two split columns.
pub const SPLIT_DIVIDER_WIDTH: f32 = 1.0;
pub const DIFF_TEXT_SIZE: f32 = 12.0;
pub const DIFF_TAB_SIZE: usize = 4;
/// One shared `code_font_size` setting drives several surfaces that never
/// agreed on a size historically. Each scales off its own baseline so the
/// default setting reproduces the size that surface always had, and a
/// user-chosen size moves them all while keeping those proportions.
pub const DIFF_TEXT_SIZE_RATIO: f32 = DIFF_TEXT_SIZE / crate::typography::CODE_FONT_SIZE_DEFAULT;

/// Size of the painted diff body text, and the size the column measurement in
/// [`DiffHorizontalGeometry::resolve`] must use: they desync otherwise.
pub fn diff_text_size(theme: &Theme) -> f32 {
    crate::typography::clamp_font_size(theme.code_font_size * DIFF_TEXT_SIZE_RATIO)
}

/// The row box and the painted line box must agree, or code clips once the
/// user moves the code font size off [`DIFF_TEXT_SIZE`].
pub fn diff_line_height(theme: &Theme) -> f32 {
    diff_text_size(theme) * (DIFF_LINE_HEIGHT / DIFF_TEXT_SIZE)
}

pub const UNIFIED_CODE_PADDING_LEFT: f32 = 12.0;
pub const SPLIT_CODE_PADDING_LEFT: f32 = 6.0;
/// Breathing room after the widest source line when scrolled fully right.
pub const CODE_PADDING_RIGHT: f32 = 24.0;

/// How the diff is laid out. Persisted in `ui-settings.json` (`diffSplit`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiffMode {
    /// One column: deletions above additions (the classic patch reading).
    #[default]
    Unified,
    /// Two columns: old on the left, new on the right, paired per hunk.
    Split,
}

impl DiffMode {
    pub fn from_split(split: bool) -> Self {
        if split { Self::Split } else { Self::Unified }
    }

    pub fn is_split(self) -> bool {
        self == Self::Split
    }

    pub fn toggled(self) -> Self {
        match self {
            Self::Unified => Self::Split,
            Self::Split => Self::Unified,
        }
    }
}

// ---------------------------------------------------------------------------
// Patch model + parser (pure)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Add,
    Del,
    /// `\ No newline at end of file` and friends.
    Meta,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceSide {
    Old,
    New,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourceLineRef {
    pub side: SourceSide,
    /// One-based source line number.
    pub line_number: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DiffHighlights {
    pub old: Option<Arc<zeron_syntax::HighlightedDocument>>,
    pub new: Option<Arc<zeron_syntax::HighlightedDocument>>,
}

impl DiffHighlights {
    pub fn source_ref(&self, line: &DiffLine) -> Option<SourceLineRef> {
        match line.kind {
            LineKind::Del => line.old_no.map(|line_number| SourceLineRef {
                side: SourceSide::Old,
                line_number,
            }),
            LineKind::Add => line.new_no.map(|line_number| SourceLineRef {
                side: SourceSide::New,
                line_number,
            }),
            LineKind::Context => line
                .new_no
                .filter(|_| self.new.is_some())
                .map(|line_number| SourceLineRef {
                    side: SourceSide::New,
                    line_number,
                })
                .or_else(|| {
                    line.old_no.map(|line_number| SourceLineRef {
                        side: SourceSide::Old,
                        line_number,
                    })
                }),
            LineKind::Meta => None,
        }
    }

    pub fn spans(&self, line: &DiffLine) -> &[zeron_syntax::HighlightSpan] {
        let Some(source_ref) = self.source_ref(line) else {
            return &[];
        };
        let document = match source_ref.side {
            SourceSide::Old => self.old.as_deref(),
            SourceSide::New => self.new.as_deref(),
        };
        document
            .and_then(|document| document.lines.get(source_ref.line_number as usize - 1))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    Deleted,
    Modified,
    Renamed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileDiff {
    /// Display path (the post-change side).
    pub path: String,
    /// Pre-rename path, when different.
    pub old_path: Option<String>,
    pub status: FileStatus,
    pub binary: bool,
    /// Parser-collected notices (mode changes etc.).
    pub notices: Vec<String>,
    pub hunks: Vec<Hunk>,
    pub additions: u32,
    pub deletions: u32,
    /// Largest line number on either side — sizes the gutters analytically
    /// (a fixed column overflowed past 4 digits; user report).
    pub max_line: u32,
}

impl FileDiff {
    pub fn new(path: String, old_path: Option<String>) -> Self {
        Self {
            path,
            old_path,
            status: FileStatus::Modified,
            binary: false,
            notices: Vec::new(),
            hunks: Vec::new(),
            additions: 0,
            deletions: 0,
            max_line: 0,
        }
    }
}

/// Width of one line-number gutter column, fitted to the file's largest
/// line number: 11px mono ≈ 6.6px per digit, the 8px right pad, and a 6px
/// left gap so the number never abuts the accent bar (at 4 digits the old
/// formula left 1.6px — visually touching; user report). Never narrower
/// than the classic 36px column.
pub fn gutter_width(file: &FileDiff) -> f32 {
    let digits = file.max_line.max(1).ilog10() + 1;
    (digits as f32 * 6.6 + 8.0 + 6.0).max(GUTTER_WIDTH)
}

/// Width inputs that are independent of the active window's font metrics.
/// They are computed once with the parsed patch, off the render path.
#[derive(Debug, Clone, Copy, PartialEq)]
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

/// Analytic expanded-body height — drives the 180 ms fold tween without
/// measurement.
pub fn body_height(file: &FileDiff) -> f32 {
    body_height_with(file, &[], None, DiffMode::Unified, DIFF_LINE_HEIGHT)
}

pub fn body_height_with(
    file: &FileDiff,
    comments: &[ReviewComment],
    draft: Option<(CommentSide, u32)>,
    mode: DiffMode,
    line_h: f32,
) -> f32 {
    body_rows(0, file, comments, draft, mode)
        .iter()
        .map(|row| row.height(comments, line_h))
        .sum()
}

// ---------------------------------------------------------------------------
// Resolution + states (pure)
// ---------------------------------------------------------------------------

/// The diff shown for a chat: `checkout_id` match first, then device+cwd,
/// then cwd alone (§1.11).
pub fn resolve_diff<'a>(diffs: &'a [CheckoutDiff], chat: &Chat) -> Option<&'a CheckoutDiff> {
    if let Some(checkout_id) = chat.checkout_id.as_deref()
        && let Some(diff) = diffs.iter().find(|d| d.checkout_id == checkout_id)
    {
        return Some(diff);
    }
    let cwd = chat.cwd.as_deref()?;
    diffs
        .iter()
        .find(|d| d.device_id == chat.device_id && d.cwd == cwd)
        .or_else(|| diffs.iter().find(|d| d.cwd == cwd))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffPhase {
    /// No diff for this checkout yet.
    Preparing,
    /// Diff arrived and it's empty — working tree clean.
    Clean,
    List,
}

pub fn diff_phase(resolved: Option<&CheckoutDiff>) -> DiffPhase {
    match resolved {
        None => DiffPhase::Preparing,
        Some(diff) if diff.patch.trim().is_empty() && diff.files.is_empty() => DiffPhase::Clean,
        Some(_) => DiffPhase::List,
    }
}

/// Header label: "N Uncommitted change(s)".
pub fn uncommitted_label(count: usize) -> String {
    if count == 1 {
        "1 Uncommitted change".to_string()
    } else {
        format!("{count} Uncommitted changes")
    }
}

/// What the pane diffs against (t3code's scope dropdown).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiffScope {
    /// Uncommitted changes vs HEAD — the live watch stream.
    #[default]
    WorkingTree,
    /// Everything this branch adds over `merge-base(base_ref, HEAD)`,
    /// working tree included.
    Branch,
    /// Changes since the current chat's last turn started.
    LatestTurn,
    /// Repository commit graph. History owns this scope in its own right-pane
    /// surface tab; it remains a `Changes` variant so commit rows can open
    /// their corresponding diff tabs through the existing event path.
    History,
    /// One commit's own changes (parent vs commit) — the per-commit tab a
    /// History row click opens. Never listed in the scope menu
    /// ([`Self::ALL`]); a commit-pinned pane is born this way and stays.
    Commit,
}

impl DiffScope {
    /// Scopes available from a Diff tab. History is selected from the surface
    /// picker instead, so it can keep its own tab and toolbar state.
    pub const ALL: [DiffScope; 3] = [Self::WorkingTree, Self::Branch, Self::LatestTurn];

    pub fn label(self) -> &'static str {
        match self {
            Self::WorkingTree => "Working tree",
            Self::Branch => "Branch changes",
            Self::LatestTurn => "Latest turn",
            Self::History => "History",
            Self::Commit => "Commit",
        }
    }

    /// Wire value for `GetCheckoutDiff` `mode` (and parse-key discriminant).
    pub fn mode(self) -> &'static str {
        match self {
            Self::WorkingTree => "workingTree",
            Self::Branch => "branch",
            Self::LatestTurn => "turn",
            Self::History => "history",
            Self::Commit => "commit",
        }
    }
}

/// Header-strip label per scope.
pub fn scope_label(scope: DiffScope, count: usize, base: Option<&str>) -> String {
    let files = if count == 1 { "file" } else { "files" };
    match scope {
        DiffScope::WorkingTree => uncommitted_label(count),
        DiffScope::Branch => match base {
            Some(base) => format!("{count} Changed {files} vs {base}"),
            None => format!("{count} Changed {files}"),
        },
        DiffScope::LatestTurn => format!("{count} Changed {files} this turn"),
        DiffScope::History => "History".to_string(),
        DiffScope::Commit => format!("{count} Changed {files} in this commit"),
    }
}

/// The comparison ref the branch scope preselects. `branches` comes from
/// `ListBranches` with the repo's default branch first — but a repo with no
/// `origin/HEAD` falls back to the *checked-out* branch there, and comparing a
/// branch with itself is useless; prefer `main`/`master` in that case.
pub fn default_base_ref(branches: &[String], current: Option<&str>) -> Option<String> {
    let first = branches.first()?;
    if current != Some(first.as_str()) {
        return Some(first.clone());
    }
    for candidate in ["main", "master"] {
        if branches.iter().any(|b| b == candidate) {
            return Some(candidate.to_string());
        }
    }
    branches
        .iter()
        .find(|b| current != Some(b.as_str()))
        .or(Some(first))
        .cloned()
}

/// Empty-state copy per scope.
pub fn clean_message(scope: DiffScope, base: Option<&str>) -> String {
    match scope {
        DiffScope::WorkingTree => "No uncommitted changes".to_string(),
        DiffScope::Branch => match base {
            Some(base) => format!("No changes vs {base}"),
            None => "No branch changes".to_string(),
        },
        DiffScope::LatestTurn => "No changes this turn".to_string(),
        DiffScope::History => "No commits found".to_string(),
        DiffScope::Commit => "Empty commit".to_string(),
    }
}

/// Fold a `WatchCheckoutDiffs` frame into the diff set. Accepts either a full
/// list (replace) or a single `CheckoutDiff` (upsert by checkout id) — the
/// contract streams `CheckoutDiff` items, but list frames cost nothing to
/// support. Returns whether anything changed.
pub fn apply_diff_frame(diffs: &mut Vec<CheckoutDiff>, value: serde_json::Value) -> bool {
    if let Ok(all) = serde_json::from_value::<Vec<CheckoutDiff>>(value.clone()) {
        if *diffs != all {
            *diffs = all;
            return true;
        }
        return false;
    }
    match serde_json::from_value::<CheckoutDiff>(value) {
        Ok(one) => {
            if let Some(existing) = diffs.iter_mut().find(|d| d.checkout_id == one.checkout_id) {
                if *existing == one {
                    return false;
                }
                *existing = one;
            } else {
                diffs.push(one);
            }
            true
        }
        Err(err) => {
            tracing::warn!(error = %err, "changes: dropping malformed diff frame");
            false
        }
    }
}

pub fn comment_state_key(
    comments: &[ReviewComment],
    draft: Option<&(String, CommentSide, u32)>,
) -> u64 {
    let mut parts: Vec<String> = comments
        .iter()
        .flat_map(|comment| [comment.id.clone(), comment.body.clone()])
        .collect();
    if let Some((path, side, line)) = draft {
        parts.push(format!("draft:{path}:{}:{line}", side.tag()));
    }
    let refs: Vec<&str> = parts.iter().map(String::as_str).collect();
    hash64(&refs)
}

pub fn hash64(parts: &[&str]) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    for p in parts {
        p.hash(&mut hasher);
    }
    hasher.finish()
}

pub const MAX_EXCERPT_SOURCE_LINES: usize = 200_000;

pub fn excerpt_side(
    file: &FileDiff,
    side: SourceSide,
    language: Lang,
    path: &str,
) -> Option<Arc<zeron_syntax::HighlightedDocument>> {
    let max_line = file
        .hunks
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .filter_map(|line| match side {
            SourceSide::Old => line.old_no,
            SourceSide::New => line.new_no,
        })
        .max()
        .unwrap_or(0) as usize;
    if max_line > MAX_EXCERPT_SOURCE_LINES {
        return None;
    }
    let mut lines = vec![Vec::new(); max_line];
    for hunk in &file.hunks {
        let visible = hunk
            .lines
            .iter()
            .filter_map(|line| {
                let number = match side {
                    SourceSide::Old => line.old_no,
                    SourceSide::New => line.new_no,
                }?;
                (line.kind != LineKind::Meta).then_some((number, line.text.as_str()))
            })
            .collect::<Vec<_>>();
        if visible.is_empty() {
            continue;
        }
        let source = visible
            .iter()
            .map(|(_, text)| *text)
            .collect::<Vec<_>>()
            .join("\n");
        let document = zeron_syntax::highlight(zeron_syntax::HighlightRequest {
            source: &source,
            path: Some(path),
            fence_tag: None,
        })
        .ok()?;
        for ((number, _), spans) in visible.into_iter().zip(document.lines) {
            lines[number as usize - 1] = spans;
        }
    }
    Some(Arc::new(zeron_syntax::HighlightedDocument {
        language,
        lines,
    }))
}

pub fn excerpt_highlights(file: &FileDiff, language: Lang) -> Option<DiffHighlights> {
    if !zeron_syntax::supports_language(language) {
        return None;
    }
    let old = if file.status == FileStatus::Added {
        None
    } else {
        Some(excerpt_side(
            file,
            SourceSide::Old,
            language,
            file.old_path.as_deref().unwrap_or(&file.path),
        )?)
    };
    let new = if file.status == FileStatus::Deleted {
        None
    } else {
        Some(excerpt_side(file, SourceSide::New, language, &file.path)?)
    };
    Some(DiffHighlights { old, new })
}

pub fn sources_match_patch(file: &FileDiff, response: &zeron_proto::CheckoutFileDiffText) -> bool {
    let old = response
        .old_text
        .as_deref()
        .map(|source| source.lines().collect::<Vec<_>>());
    let new = response
        .new_text
        .as_deref()
        .map(|source| source.lines().collect::<Vec<_>>());
    file.hunks.iter().flat_map(|hunk| &hunk.lines).all(|line| {
        let actual = match line.kind {
            LineKind::Del => line
                .old_no
                .and_then(|number| old.as_ref()?.get(number as usize - 1).copied()),
            LineKind::Add => line
                .new_no
                .and_then(|number| new.as_ref()?.get(number as usize - 1).copied()),
            LineKind::Context => line
                .new_no
                .and_then(|number| new.as_ref()?.get(number as usize - 1).copied())
                .or_else(|| {
                    line.old_no
                        .and_then(|number| old.as_ref()?.get(number as usize - 1).copied())
                }),
            LineKind::Meta => return true,
        };
        actual == Some(line.text.as_str())
    })
}

pub fn full_highlights(
    file: &FileDiff,
    language: Lang,
    response: &zeron_proto::CheckoutFileDiffText,
) -> Option<DiffHighlights> {
    if response.stale
        || response.binary
        || response.truncated
        || !sources_match_patch(file, response)
    {
        return None;
    }
    let parse = |source: &str, path: &str| {
        zeron_syntax::highlight(zeron_syntax::HighlightRequest {
            source,
            path: Some(path),
            fence_tag: None,
        })
        .ok()
        .map(Arc::new)
    };
    let old = match response.old_text.as_deref() {
        Some(source) => Some(parse(
            source,
            file.old_path.as_deref().unwrap_or(&file.path),
        )?),
        None => None,
    };
    let new = match response.new_text.as_deref() {
        Some(source) => Some(parse(source, &file.path)?),
        None => None,
    };
    if old.is_none() && new.is_none() && zeron_syntax::supports_language(language) {
        return None;
    }
    Some(DiffHighlights { old, new })
}

// ---------------------------------------------------------------------------
// Entity & Horizontal State
// ---------------------------------------------------------------------------

pub struct MeasuredDiffWidth {
    pub key: (u32, u32, u32, SharedString),
    // Retaining the Arc makes pointer identity safe against allocator reuse.
    pub highlights: Option<Arc<DiffHighlights>>,
    pub width: f32,
}

pub struct FileHorizontalState {
    pub geometry: DiffHorizontalGeometry,
    pub scroll: gpui::ScrollHandle,
    /// Shape once per file, typography/theme, and highlight revision.
    pub measured: std::cell::RefCell<Option<MeasuredDiffWidth>>,
}

impl FileHorizontalState {
    pub fn new(file: &FileDiff) -> Self {
        Self {
            geometry: DiffHorizontalGeometry::from_file(file),
            scroll: gpui::ScrollHandle::new(),
            measured: std::cell::RefCell::new(None),
        }
    }

    pub fn metrics(
        &self,
        file: &FileDiff,
        highlights: Option<&Arc<DiffHighlights>>,
        theme: &Theme,
        text_system: &gpui::WindowTextSystem,
        generation: u32,
    ) -> DiffHorizontalMetrics {
        let key = (
            generation,
            crate::theme::style_generation(),
            diff_text_size(theme).to_bits(),
            theme.font_mono.clone(),
        );
        let mut cached = self.measured.borrow_mut();
        let current = cached.as_ref().is_some_and(|cached| {
            cached.key == key
                && match (cached.highlights.as_ref(), highlights) {
                    (None, None) => true,
                    (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                    _ => false,
                }
        });
        if !current {
            *cached = Some(MeasuredDiffWidth {
                key,
                highlights: highlights.cloned(),
                width: max_shaped_text_width(
                    file,
                    highlights.map(AsRef::as_ref),
                    theme,
                    text_system,
                ),
            });
        }
        DiffHorizontalMetrics {
            max_text_width: cached.as_ref().unwrap().width,
            max_gutter_width: self.geometry.max_gutter_width,
        }
    }
}

pub struct ParsedDiff {
    /// `checkout_id:checksum` — identity of the parsed content.
    pub key: String,
    pub truncated: bool,
    pub additions: u32,
    pub deletions: u32,
    pub file_count: usize,
    /// Indexed like `files`; survives row virtualization and folding.
    pub horizontal: Vec<FileHorizontalState>,
    pub files: Arc<Vec<FileDiff>>,
}

// ---------------------------------------------------------------------------
// Row model — the diff flattened to line granularity (pure)
// ---------------------------------------------------------------------------

/// One virtualized list row. The diff is flattened so each visible LINE is
/// its own row (Zed's editor draws exactly the visible line range the same
/// way): scrolling a 10k-line file materializes ~50 line rows per frame, not
/// one 10k-line element, and a collapsed file contributes no body rows at
/// all. Nowrap heights use the analytic constants above; wrapped line rows
/// are measured by the list at the current pane width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffRow {
    FileHeader {
        file: u32,
    },
    Notice {
        file: u32,
        notice: u32,
    },
    HunkHeader {
        file: u32,
        hunk: u32,
    },
    Line {
        file: u32,
        hunk: u32,
        line: u32,
        /// Flat index across the file's hunks — keys into the highlight slot.
        flat: u32,
    },
    /// One split row: the two line indices [`split_pairs`] paired. Carrying
    /// them inline keeps the pairing off the render path — it is computed
    /// once, when the body is flattened.
    SplitLine {
        file: u32,
        hunk: u32,
        left: Option<u32>,
        right: Option<u32>,
    },
    /// `card` indexes the file's own staged-comment slice, in staged order.
    CommentCard {
        file: u32,
        card: u32,
    },
    CommentDraft {
        file: u32,
    },
    /// Trailing pad closing an expanded body ([`BODY_BOTTOM_PAD`]).
    BodyPad {
        file: u32,
    },
    /// A body mid-fold-tween: one height-animated, clipped row standing in
    /// for the whole body. Only the slice that can be revealed is built —
    /// the tween never pays for off-screen lines.
    FoldingBody {
        file: u32,
    },
}

impl DiffRow {
    pub fn file(self) -> usize {
        match self {
            Self::FileHeader { file }
            | Self::Notice { file, .. }
            | Self::HunkHeader { file, .. }
            | Self::Line { file, .. }
            | Self::SplitLine { file, .. }
            | Self::CommentCard { file, .. }
            | Self::CommentDraft { file }
            | Self::BodyPad { file }
            | Self::FoldingBody { file } => file as usize,
        }
    }

    /// `FoldingBody` is height-animated, so it reports 0 and never lands in a
    /// height sum.
    pub fn height(self, comments: &[ReviewComment], line_h: f32) -> f32 {
        match self {
            DiffRow::FileHeader { .. } => FILE_HEADER_HEIGHT,
            DiffRow::Notice { .. } => NOTICE_HEIGHT,
            DiffRow::HunkHeader { .. } => HUNK_HEADER_HEIGHT,
            DiffRow::Line { .. } | DiffRow::SplitLine { .. } => line_h,
            DiffRow::CommentCard { card, .. } => comments
                .get(card as usize)
                .map(|comment| comments::card_height(&comment.body))
                .unwrap_or(0.0),
            DiffRow::CommentDraft { .. } => comments::DRAFT_CARD_HEIGHT,
            DiffRow::BodyPad { .. } => BODY_BOTTOM_PAD,
            DiffRow::FoldingBody { .. } => 0.0,
        }
    }
}

/// Capacity hint only — comment cards are not counted. Split pairs can only
/// shrink the line count, so the unified count is a safe hint for both.
pub fn body_row_count(file: &FileDiff) -> usize {
    let lines: usize = file.hunks.iter().map(|h| h.lines.len()).sum();
    file_notices(file).len() + file.hunks.len() + lines + 1
}

pub fn body_rows(
    file_ix: u32,
    file: &FileDiff,
    comments: &[ReviewComment],
    draft: Option<(CommentSide, u32)>,
    mode: DiffMode,
) -> Vec<DiffRow> {
    fn push_cards(
        rows: &mut Vec<DiffRow>,
        file_ix: u32,
        comments: &[ReviewComment],
        draft: Option<(CommentSide, u32)>,
        anchors: &[Option<(CommentSide, u32)>],
    ) {
        for anchor in anchors.iter().flatten() {
            for (ix, comment) in comments.iter().enumerate() {
                if comment.diff_anchor() == Some(*anchor) {
                    rows.push(DiffRow::CommentCard {
                        file: file_ix,
                        card: ix as u32,
                    });
                }
            }
            if draft == Some(*anchor) {
                rows.push(DiffRow::CommentDraft { file: file_ix });
            }
        }
    }

    let mut rows = Vec::with_capacity(body_row_count(file));
    for notice in 0..file_notices(file).len() {
        rows.push(DiffRow::Notice {
            file: file_ix,
            notice: notice as u32,
        });
    }
    let mut hunk_flat = 0u32;
    for (hunk_ix, hunk) in file.hunks.iter().enumerate() {
        rows.push(DiffRow::HunkHeader {
            file: file_ix,
            hunk: hunk_ix as u32,
        });
        match mode {
            DiffMode::Unified => {
                for (line_ix, line) in hunk.lines.iter().enumerate() {
                    rows.push(DiffRow::Line {
                        file: file_ix,
                        hunk: hunk_ix as u32,
                        line: line_ix as u32,
                        flat: hunk_flat + line_ix as u32,
                    });
                    push_cards(&mut rows, file_ix, comments, draft, &[line_anchor(line)]);
                }
            }
            DiffMode::Split => {
                for (left, right) in split_pairs(&hunk.lines) {
                    rows.push(DiffRow::SplitLine {
                        file: file_ix,
                        hunk: hunk_ix as u32,
                        left,
                        right,
                    });
                    let anchors = pair_anchors(&hunk.lines, (left, right));
                    push_cards(&mut rows, file_ix, comments, draft, &anchors);
                }
            }
        }
        hunk_flat += hunk.lines.len() as u32;
    }
    rows.push(DiffRow::BodyPad { file: file_ix });
    rows
}

/// Flatten all files into rows + each file's row span (header at
/// `range.start`, body rows after it). `collapsed(ix)` folds a file to just
/// its header. `comments` is the whole staged set; each file takes its own
/// path's slice.
pub fn flatten_rows(
    files: &[FileDiff],
    comments: &[ReviewComment],
    draft: Option<(&str, CommentSide, u32)>,
    mode: DiffMode,
    mut collapsed: impl FnMut(usize) -> bool,
) -> (Vec<DiffRow>, Vec<std::ops::Range<usize>>) {
    let mut rows = Vec::new();
    let mut ranges = Vec::with_capacity(files.len());
    for (ix, file) in files.iter().enumerate() {
        let start = rows.len();
        rows.push(DiffRow::FileHeader { file: ix as u32 });
        if !collapsed(ix) {
            let file_comments: Vec<ReviewComment> = comments
                .iter()
                .filter(|comment| !comment.is_file() && comment.path == file.path)
                .cloned()
                .collect();
            let file_draft = draft
                .filter(|(path, _, _)| *path == file.path)
                .map(|(_, side, line)| (side, line));
            rows.extend(body_rows(ix as u32, file, &file_comments, file_draft, mode));
        }
        ranges.push(start..rows.len());
    }
    (rows, ranges)
}

/// The file header that should remain visible for a logical list position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StickyFileHeader {
    pub file_ix: usize,
    pub header_row: usize,
    pub next_header_row: Option<usize>,
}

/// Resolve a sticky file header from the current flattened row ranges.
///
/// This remains independent of the rendered list so folds and diff resets
/// cannot leave a second, stale active-file state behind.
pub fn sticky_file_header(
    row_ranges: &[std::ops::Range<usize>],
    item_ix: usize,
    offset_in_item: f32,
) -> Option<StickyFileHeader> {
    let file_ix = row_ranges
        .partition_point(|range| range.start <= item_ix)
        .checked_sub(1)?;
    let range = row_ranges.get(file_ix)?;

    // A reset can briefly leave ListState pointing past the replacement
    // model. Treat that frame as having no sticky header.
    if !range.contains(&item_ix) || (item_ix == range.start && offset_in_item <= 0.0) {
        return None;
    }

    Some(StickyFileHeader {
        file_ix,
        header_row: range.start,
        next_header_row: row_ranges.get(file_ix + 1).map(|range| range.start),
    })
}

/// Offset a sticky header upward as the next file header enters its slot.
pub fn sticky_header_push_offset(next_header_y: Option<f32>) -> f32 {
    next_header_y
        .map(|y| (y - FILE_HEADER_HEIGHT).min(0.0))
        .unwrap_or(0.0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileHeaderPresentation {
    Row,
    Sticky,
}

impl FileHeaderPresentation {
    pub fn key_prefix(self) -> &'static str {
        match self {
            Self::Row => "file-hdr",
            Self::Sticky => "sticky-file-hdr",
        }
    }

    pub fn element_id(self, file_ix: usize) -> SharedString {
        let prefix = self.key_prefix();
        SharedString::from(format!("{prefix}-{file_ix}"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StickyFileHeaderPaint {
    pub rest_bg: gpui::Hsla,
    pub hover_bg: gpui::Hsla,
    pub border: gpui::Hsla,
    pub frost_tint: Option<gpui::Hsla>,
}

/// Resolve the sticky header from the diff's content plane, not the elevated
/// overlay plane used by menus and popovers.
pub fn sticky_file_header_paint(theme: &Theme) -> StickyFileHeaderPaint {
    if theme.is_frost() {
        let tint_alpha = match theme.appearance {
            crate::theme::Appearance::Dark => STICKY_FILE_HEADER_TINT_ALPHA_DARK,
            crate::theme::Appearance::Light => STICKY_FILE_HEADER_TINT_ALPHA_LIGHT,
        };
        StickyFileHeaderPaint {
            rest_bg: theme.ink(0.025),
            hover_bg: theme.glass_hover(),
            border: theme.border,
            frost_tint: Some(theme.bg.opacity(tint_alpha)),
        }
    } else {
        StickyFileHeaderPaint {
            rest_bg: crate::theme::flatten(theme.ink(0.025), theme.bg),
            hover_bg: crate::theme::flatten(theme.element_hover, theme.bg),
            border: theme.border,
            frost_tint: None,
        }
    }
}

#[derive(Default, Clone, Copy)]
pub struct FileFold {
    pub collapsed: bool,
    /// Bumped per toggle — keys the height tween + chevron transition.
    pub epoch: usize,
    pub from: f32,
    pub to: f32,
    /// When the toggle happened: the tweens are armed only briefly after the
    /// click — gpui replays an element's animation on remount, and in the
    /// virtualized list a row scrolling back into view is a remount (the
    /// transcript's tool groups had the same flash; user report).
    pub toggled_at: Option<std::time::Instant>,
}

/// Tween arming window after a fold toggle (COLLAPSE's 180ms plus margin).
pub const FOLD_TWEEN_WINDOW: Duration = Duration::from_millis(400);

/// Ceiling on how much body a fold tween's stand-in row materializes. A
/// tween always starts from a clicked (on-screen) header, so the revealable
/// slice is at most one viewport tall — everything past this is clipped or
/// below the fold either way.
pub const FOLD_TWEEN_MAX_PX: f32 = 2400.0;

impl FileFold {
    pub fn animating(&self) -> bool {
        self.epoch > 0
            && self
                .toggled_at
                .is_some_and(|at| at.elapsed() < FOLD_TWEEN_WINDOW)
    }
}

pub struct HighlightSlot {
    pub fingerprint: u64,
    pub state: DiffHighlightState,
    pub _excerpt_task: Option<Task<()>>,
    pub _fetch_task: Option<Task<()>>,
}

pub enum DiffHighlightState {
    Pending,
    Ready(Arc<DiffHighlights>),
    Excerpt(Arc<DiffHighlights>),
    Plain,
}

/// The open base-ref dropdown — the same searchable-menu recipe as the
/// composer's ref picker and the spaces filter: a filter input on top
/// (`PaletteSearch` context so ↑↓/⏎ bubble to the card's key handler),
/// ranked substring rows below.
pub struct RefMenu {
    pub search: Entity<ComposerInput>,
    /// Keyboard highlight within the filtered rows.
    pub active: usize,
    /// Tracked on the card — puts it on the keyboard dispatch path while the
    /// search input holds focus (the structure every working picker uses).
    pub focus: FocusHandle,
    pub list_scroll: gpui::ScrollHandle,
    pub _search_events: Subscription,
}

/// The line the pointer is on. Only one element per anchor ever takes the
/// hover — the unified row, or a split row's right column — so the anchor
/// alone identifies it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverRow {
    pub path: String,
    pub side: CommentSide,
    pub line: u32,
}

pub struct CommentDraft {
    pub editing_id: Option<String>,
    /// Composer the note will stage onto, captured when the card opened. A
    /// draft belongs to the checkout it was written over, so it must not
    /// follow the user onto whatever chat is selected by commit time.
    pub key: String,
    pub path: String,
    /// The file's pre-rename path, when it moved — carried onto the comment so
    /// an `Old`-side citation names the file that line lives in.
    pub old_path: Option<String>,
    pub side: CommentSide,
    pub line: u32,
    pub input: Entity<ComposerInput>,
    pub _events: Subscription,
}

/// The paint-only syntax runs for one diff line.
pub fn line_runs(
    line: &DiffLine,
    highlights: Option<&DiffHighlights>,
    theme: &Theme,
) -> Vec<gpui::TextRun> {
    let spans = highlights.map(|h| h.spans(line)).unwrap_or(&[]);
    render::runs_for_syntax_line_with_plain(
        &line.text,
        spans,
        &font(theme.font_mono.clone()),
        theme.text.opacity(0.92),
        theme,
    )
}
