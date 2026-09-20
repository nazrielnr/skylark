//! The right-pane "Changes" content (feature-inventory §1.11): a unified-diff
//! viewer over `WatchCheckoutDiffs`.
//!
//! - pure patch parser: `diff --git` sections → file/hunk/line/notice rows,
//!   with add/delete/rename/binary detection and per-file counts;
//! - resolution: the shown diff matches the selected chat by `checkout_id`
//!   first, then by device+cwd, then cwd alone;
//! - states: *preparing* (no diff yet), *clean* (empty patch), *list*; a watch
//!   error shows a banner while the last content stays;
//! - virtualized with gpui `list()` at LINE granularity — every file header,
//!   hunk header, and diff line is its own row (the flat model Zed's editor
//!   uses for its project diff: only the visible slice materializes, and a
//!   collapsed file's body rows are removed from the list outright, not
//!   hidden); nowrap sections collapse with a 180 ms height tween on a
//!   clipped stand-in row (analytic heights, capped to what the clip can
//!   reveal), while variable-height wrapped sections settle immediately;
//! - syntax highlight reuses the markdown tokenizer per diff line, computed
//!   time-sliced on the background executor and applied as paint-only run
//!   colors (layout never changes);
//! - scopes (t3code parity): *Working tree* rides the watch stream; *Branch
//!   changes* (vs a selectable base ref, default branch preselected) and
//!   *Latest turn* fetch one-shot `GetCheckoutDiff` captures, refreshed when
//!   the watch checksum says the tree moved;
//! - two layouts ([`DiffMode`], toolbar toggle, persisted): *unified* stacks
//!   old and new; *split* pairs each hunk's deletions against its additions
//!   into one row with two columns. Split is a pure re-flatten of the same
//!   parse — the row model, virtualization, folds, and highlights are shared.
//!   Its left column is inert: notes are cited against the post-change file,
//!   so only the right column takes a `+` (already-staged old-side notes
//!   still show their cards).
//! - long-line wrapping is a persisted toolbar choice. It only changes the
//!   code plane: gutters, comment affordances, and sticky headers stay fixed,
//!   while the virtual list measures each logical row's resulting height.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{
    Context, Entity, ListAlignment, ListState, SharedString, Subscription, Task,
    prelude::*, px,
};

use zeron_proto::{CheckoutDiff, GitHistoryCommit};
use crate::history::{
    GitHistory, GitHistoryCount, GitHistoryEvent, GitHistoryFetchButton, GitHistorySearchControl,
    GitHistoryViewButton,
};
use crate::popover::Popup;
use crate::state::AppState;
use crate::theme::Theme;

pub mod watch;
pub mod menu;
pub mod folding;
pub mod highlight;
pub mod display;
pub(crate) use display::*;
pub mod diff_comments;
pub use diff_comments::*;
pub mod diff_model;
pub use diff_model::*;
pub mod parser;
pub use parser::*;
pub mod split;
pub use split::*;

/// The Changes pane entity. Lazy: no RPC until [`Changes::ensure_watch`] runs
/// (the shell calls it when the pane first opens).
pub struct Changes {
    pub(crate) state: Entity<AppState>,
    pub(crate) diffs: Vec<CheckoutDiff>,
    pub(crate) started: bool,
    pub(crate) error: Option<SharedString>,
    /// Device the running watch targets: `None` = the connected engine itself,
    /// `Some(id)` = a remote chat's host (relay-forwarded). The stream only
    /// carries the TARGET device's checkouts, so a selection change onto a
    /// chat hosted elsewhere tears the watch down and re-subscribes.
    pub(crate) watch_target: Option<String>,
    pub(crate) watch_task: Option<Task<()>>,
    pub(crate) parsed: Option<ParsedDiff>,
    pub(crate) parse_task: Option<Task<()>>,
    pub(crate) folds: HashMap<String, FileFold>,
    pub(crate) highlights: HashMap<String, HighlightSlot>,
    /// The flattened row model the list virtualizes over (line granularity;
    /// collapsed bodies excluded) + each file's row span within it.
    pub(crate) rows: Vec<DiffRow>,
    pub(crate) row_ranges: Vec<std::ops::Range<usize>>,
    /// Sweeps [`DiffRow::FoldingBody`] stand-ins back to steady-state rows
    /// once their tween window elapses.
    pub(crate) fold_settle: Option<Task<()>>,
    pub(crate) list: ListState,
    /// What the pane diffs against (toolbar dropdown).
    pub(crate) scope: DiffScope,
    /// Unified or side-by-side (toolbar toggle, persisted per user).
    pub(crate) mode: DiffMode,
    /// Wrap long source lines instead of exposing the horizontal code plane.
    pub(crate) wrap_lines: bool,
    /// Comparison ref for [`DiffScope::Branch`] — preset to the repo's
    /// default branch once the branch list lands.
    pub(crate) base_ref: Option<String>,
    pub(crate) branches: Vec<String>,
    /// `device:cwd` the branch list was fetched for.
    pub(crate) branches_for: Option<String>,
    pub(crate) branches_task: Option<Task<()>>,
    /// One-shot scoped capture (Branch / Latest turn) + its fetch key.
    pub(crate) scoped: Option<CheckoutDiff>,
    pub(crate) scoped_for: Option<String>,
    pub(crate) scoped_error: Option<SharedString>,
    pub(crate) scoped_inflight: Option<String>,
    pub(crate) scoped_task: Option<Task<()>>,
    pub(crate) scope_menu: Popup<()>,
    pub(crate) ref_menu: Popup<RefMenu>,
    /// Only ever one: a second `+` moves the card rather than stacking two
    /// half-written notes.
    pub(crate) draft: Option<CommentDraft>,
    pub(crate) hover: Option<HoverRow>,
    pub(crate) comment_key: u64,
    pub(crate) history: Option<Entity<GitHistory>>,
    pub(crate) history_count: Option<Entity<GitHistoryCount>>,
    pub(crate) history_search_control: Option<Entity<GitHistorySearchControl>>,
    pub(crate) history_fetch_button: Option<Entity<GitHistoryFetchButton>>,
    pub(crate) history_view_button: Option<Entity<GitHistoryViewButton>>,
    pub(crate) history_events: Option<Subscription>,
    /// Pinned commit for a [`DiffScope::Commit`] pane (sha + subject drive
    /// the fetch and the surface-tab title).
    pub(crate) commit: Option<GitHistoryCommit>,
    pub(crate) _observe: Subscription,
}

/// Events the host (the right pane's surface strip) listens for.
pub enum ChangesEvent {
    /// A History row was clicked — open this commit as its own diff tab.
    OpenCommit(GitHistoryCommit),
}

impl gpui::EventEmitter<ChangesEvent> for Changes {}

impl Changes {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&state, |this: &mut Self, _, cx| this.sync(cx));
        let settings = crate::settings::current(cx);
        let mode = DiffMode::from_split(settings.diff_split);
        Self {
            state,
            mode,
            wrap_lines: settings.diff_wrap,
            diffs: Vec::new(),
            started: false,
            error: None,
            watch_target: None,
            watch_task: None,
            parsed: None,
            parse_task: None,
            folds: HashMap::new(),
            highlights: HashMap::new(),
            rows: Vec::new(),
            row_ranges: Vec::new(),
            fold_settle: None,
            // Rows are single lines now — a deep overdraw is cheap and keeps
            // fast wheel flicks from outrunning measurement.
            list: ListState::new(0, ListAlignment::Top, px(1024.0)),
            scope: DiffScope::default(),
            base_ref: None,
            branches: Vec::new(),
            branches_for: None,
            branches_task: None,
            scoped: None,
            scoped_for: None,
            scoped_error: None,
            scoped_inflight: None,
            scoped_task: None,
            scope_menu: Popup::default(),
            ref_menu: Popup::default(),
            draft: None,
            hover: None,
            comment_key: 0,
            history: None,
            history_count: None,
            history_search_control: None,
            history_fetch_button: None,
            history_view_button: None,
            history_events: None,
            commit: None,
            _observe: observe,
        }
    }

    /// A pane pinned to one commit's diff (a History row click) — fetches
    /// `parent vs commit` once and never offers the scope menu.
    pub fn for_commit(
        state: Entity<AppState>,
        commit: GitHistoryCommit,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut changes = Self::new(state, cx);
        changes.scope = DiffScope::Commit;
        changes.commit = Some(commit);
        changes
    }

    /// A dedicated History surface. It shares the commit-opening event path
    /// with diffs, but is never offered as an item in a Diff tab's scope menu.
    pub fn for_history(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let mut changes = Self::new(state, cx);
        changes.scope = DiffScope::History;
        changes
    }

    pub fn is_history(&self) -> bool {
        self.scope == DiffScope::History
    }

    /// The surface-tab title (contextual, user request): the pinned commit's
    /// subject (short sha for subject-less commits), else the scope's label.
    pub fn tab_title(&self) -> gpui::SharedString {
        if let Some(commit) = &self.commit {
            let subject = commit.subject.trim();
            if !subject.is_empty() {
                return subject.to_string().into();
            }
            return commit.sha.chars().take(7).collect::<String>().into();
        }
        gpui::SharedString::from(self.scope.label())
    }


    pub(crate) fn reset_horizontal_scroll(&self) {
        if let Some(parsed) = &self.parsed {
            for file in &parsed.horizontal {
                reset_horizontal_scroll(&file.scroll);
            }
        }
    }

    pub(crate) fn set_scope(&mut self, scope: DiffScope, cx: &mut Context<Self>) {
        if self.scope != scope {
            self.scope = scope;
            self.reset_horizontal_scroll();
            if scope == DiffScope::History {
                self.history_pane(cx)
                    .update(cx, |history, cx| history.ensure_loaded(cx));
            }
            self.sync(cx);
        }
        cx.notify();
    }

    pub(crate) fn history_pane(&mut self, cx: &mut Context<Self>) -> Entity<GitHistory> {
        if let Some(history) = &self.history {
            return history.clone();
        }
        let history = cx.new(|cx| GitHistory::new(self.state.clone(), cx));
        self.history_events =
            Some(
                cx.subscribe(&history, |this: &mut Self, _, event, cx| match event {
                    GitHistoryEvent::OpenCommit(commit) => {
                        // Bubble to the host — the surface strip opens the tab.
                        cx.emit(ChangesEvent::OpenCommit(commit.clone()));
                    }
                    GitHistoryEvent::FetchSucceeded => {
                        // Remote refs affect branch choices and every scoped diff
                        // based on a ref. Force fresh reads after the engine has
                        // also kicked its checkout-status watcher.
                        this.branches_for = None;
                        this.scoped_for = None;
                        this.scoped_inflight = None;
                        this.scoped_task = None;
                        this.ensure_branches(cx);
                        if this.scope != DiffScope::History {
                            this.ensure_scoped(cx);
                        }
                        cx.notify();
                    }
                }),
            );
        self.history = Some(history.clone());
        history
    }

    pub(crate) fn history_count(&mut self, cx: &mut Context<Self>) -> Entity<GitHistoryCount> {
        if let Some(count) = &self.history_count {
            return count.clone();
        }
        let history = self.history_pane(cx);
        let count = cx.new(|cx| GitHistoryCount::new(history, cx));
        self.history_count = Some(count.clone());
        count
    }

    pub(crate) fn history_fetch_button(&mut self, cx: &mut Context<Self>) -> Entity<GitHistoryFetchButton> {
        if let Some(button) = &self.history_fetch_button {
            return button.clone();
        }
        let history = self.history_pane(cx);
        let button = cx.new(|cx| GitHistoryFetchButton::new(history, cx));
        self.history_fetch_button = Some(button.clone());
        button
    }

    pub(crate) fn history_search_control(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Entity<GitHistorySearchControl> {
        if let Some(control) = &self.history_search_control {
            return control.clone();
        }
        let history = self.history_pane(cx);
        let control = cx.new(|cx| GitHistorySearchControl::new(history, cx));
        self.history_search_control = Some(control.clone());
        control
    }

    pub(crate) fn history_view_button(&mut self, cx: &mut Context<Self>) -> Entity<GitHistoryViewButton> {
        if let Some(button) = &self.history_view_button {
            return button.clone();
        }
        let history = self.history_pane(cx);
        let button = cx.new(|cx| GitHistoryViewButton::new(history, cx));
        self.history_view_button = Some(button.clone());
        button
    }

    fn set_base_ref(&mut self, base: String, cx: &mut Context<Self>) {
        if self.base_ref.as_deref() != Some(base.as_str()) {
            self.base_ref = Some(base);
            self.sync(cx);
        }
        cx.notify();
    }

    /// Everything the pane needs kicked when (re)shown: the watch plus the
    /// scope-specific loads (branches, scoped/commit capture, history) — the
    /// shell's hook for freshly-mounted surface tabs.
    pub fn ensure_content(&mut self, cx: &mut Context<Self>) {
        self.sync(cx);
    }

    /// Reconcile parsed content with the currently-active diff.
    fn sync(&mut self, cx: &mut Context<Self>) {
        self.discard_stale_draft(cx);
        // The watch follows the selected chat's host device (idempotent when
        // the target is unchanged); a boot-deferred attempt retries here too.
        self.ensure_watch(cx);
        if self.scope == DiffScope::History {
            self.history_pane(cx)
                .update(cx, |history, cx| history.ensure_loaded(cx));
            return;
        }
        if self.scope != DiffScope::Commit {
            self.ensure_branches(cx);
        }
        self.ensure_scoped(cx);
        let Some(diff) = self.active_diff(cx) else {
            if self.parsed.take().is_some() {
                self.rows.clear();
                self.row_ranges.clear();
                self.list.reset(0);
                self.folds.clear();
                self.highlights.clear();
                cx.notify();
            }
            return;
        };
        let key = self.parse_key(&diff);
        if self.parsed.as_ref().is_some_and(|p| p.key == key) {
            self.sync_comment_rows(cx);
            return;
        }
        // Parse off the render path — patches run to megabytes.
        let patch = diff.patch.clone();
        let truncated = diff.truncated;
        let additions = diff.additions;
        let deletions = diff.deletions;
        let file_count = diff.files.len();
        self.parse_task = Some(cx.spawn(async move |this, cx| {
            let files = cx
                .background_executor()
                .spawn(async move { parse_patch(&patch) })
                .await;
            this.update(cx, |changes, cx| {
                // Late results for a superseded diff are re-checked by key.
                let current = changes.active_diff(cx).map(|d| changes.parse_key(&d));
                if current.as_deref() != Some(key.as_str()) {
                    return;
                }
                let file_count = if file_count > 0 {
                    file_count
                } else {
                    files.len()
                };
                let horizontal = files.iter().map(FileHorizontalState::new).collect();
                changes.folds.clear();
                changes.highlights.clear();
                let staged = changes.staged_comments(cx);
                let draft = changes.draft_anchor();
                let (rows, ranges) = flatten_rows(
                    &files,
                    &staged,
                    draft
                        .as_ref()
                        .map(|(path, side, line)| (path.as_str(), *side, *line)),
                    changes.mode,
                    |_| false,
                );
                changes.comment_key = comment_state_key(&staged, draft.as_ref());
                // The uniform hint keeps offsets for never-rendered rows
                // sane (most rows ARE lines); real heights land as rows
                // render.
                let row_height = px(diff_line_height(Theme::of(cx)));
                changes
                    .list
                    .reset_with_uniform_height(rows.len(), row_height);
                changes.rows = rows;
                changes.row_ranges = ranges;
                changes.parsed = Some(ParsedDiff {
                    key,
                    truncated,
                    additions,
                    deletions,
                    file_count,
                    horizontal,
                    files: Arc::new(files),
                });
                cx.notify();
            })
            .ok();
        }));
    }

}

#[cfg(test)]
mod tests;
