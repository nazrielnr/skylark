//! The conversation view: virtualized transcript with block-granularity rows,
//! stick-to-bottom, tool-group folding, and streaming markdown.
//!
//! Row model (docs/research/mugen-pretext.md §3):
//! - one row per BLOCK: user message = one bubble row; assistant messages split
//!   into one row per markdown top-level block, plus consecutive-tool groups
//!   (agent/spawn chips split out so they never collapse) and input/error chips;
//! - stable row ids `{msgId}#{partId}.{blockIx}` / `{msgId}#g{groupIx}` — LIVE
//!   (streaming) entries split per block exactly like completed ones (the list
//!   virtualizes them, so a fading live reply re-renders only its visible tail
//!   each frame — flat cost in the reply length); on completion each block row
//!   keeps its id, so row identity is continuous and nothing flickers;
//! - rows are cached per entry keyed by a content fingerprint — only changed
//!   messages rebuild (the anti-"streaming stutter" trick);
//! - row-set changes diff by (id, version) into one minimal `splice`.
//!
//! Stick-to-bottom is a velocity spring (mugen §1e, the same shape as
//! stackblitz's use-stick-to-bottom): while pinned, a per-frame stepper glides
//! the viewport toward the list end with a feed-forward term tracking the
//! smoothed target growth, so 120ms doc commits read as a continuous glide
//! instead of per-commit snaps. The pin breaks only on user input (the list's
//! scroll handler fires exclusively from its wheel/touch path) and re-engages
//! inside the 70px band; the first send in an empty chat anchors the prompt at
//! the viewport top and hands off to the same glide when the reply overflows.
//! Wheel/touch releases that anchor immediately, including when background
//! streaming has advanced beyond the last measured frame.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    canvas, div, img, list, point, prelude::*, px, quad, AnyElement, BorderStyle, Bounds,
    ClipboardItem, Context, Entity, ListAlignment, ListOffset, ListScrollEvent, ListState,
    MouseButton, ObjectFit, PathBuilder, Pixels, Point, SharedString, StyledImage as _, StyledText,
    Subscription, Task, TextRun, Window,
};

#[allow(unused_imports)]
pub(crate) use zeron_doc::{
    MessagePart, MessageRole, MessageStatus, SessionMessageEntry, SubagentStatus,
};
use zeron_proto::ToolCall;

use crate::markdown::parser::{
    Block, BlockTree, IncrementalParser, InlineRun, InlineStyle, parse_full,
};
use crate::markdown::render::{self, RenderCache, RenderOptions};
use crate::markdown::veil::RowVeil;
use crate::motion::{self, AnimationExt as _};
use crate::state::AppState;
use crate::theme::Theme;

// ---------------------------------------------------------------------------
// Constants (mugen ports)
// ---------------------------------------------------------------------------


pub(crate) mod tool_cards;
pub use tool_cards::*;

pub(crate) mod stick_spring;
pub use stick_spring::*;

pub(crate) mod code_block;
pub(crate) use code_block::*;

pub(crate) mod row;
pub use row::{
    call_block, diff_rows, diff_to_file, format_timestamp, parse_for_row, rows_for_entry,
    tool_detail, tool_group_summary, top_gap_for, user_message_needs_collapse,
    user_resize_duration_ms, user_resize_spec, ParseOutcome, Row, RowKind, ToolDetail, ToolItem,
    CALL_WRAP_COLS, DIFF_DETAIL_MAX_LINES, OUTPUT_DETAIL_MAX_LINES, OUTPUT_LINE_HEIGHT,
};
pub(crate) use row::{
    frame_stats_enabled, generated_image_devices, is_agent_call, is_agent_tool, is_spawn_link,
    record_live_frame_us, record_view_frame, render_cache_disabled, tool_group_collapses,
};

#[cfg(test)]
pub(crate) use row::{
    assistant_copy_text, thought_lines, FORBID_ROW_PREPARATION, THOUGHT_WRAP_COLS,
};

pub mod flavour;
pub use flavour::*;

pub(crate) mod preparation;
pub(crate) use preparation::*;
pub(crate) mod viewport;
pub(crate) use viewport::*;
pub(crate) mod own_turn;
pub(crate) mod selection;
pub mod folding;
pub use folding::*;
pub(crate) mod attachments;
pub use attachments::*;

// ---------------------------------------------------------------------------
// Transcript entity
// ---------------------------------------------------------------------------

pub struct Transcript {
    pub(super) state: Entity<AppState>,
    pub(super) list: ListState,
    pub(super) rows: Vec<Row>,
    last_source: Option<(Option<String>, TranscriptReplayState, u64)>,
    pub(crate) chat_id: Option<String>,
    /// The shell may retain this already-laid-out view briefly for its exit.
    /// Cleared as soon as the exit is invisible; never used for another chat.
    retain_on_deselect: bool,
    /// `Some(doc_id)` pins this instance to a SUBAGENT doc: rows come from
    /// `AppState::sub_transcript(doc_id)` instead of the selected chat, and
    /// the instance is READ-ONLY — no echoes, no own-turn hold, and no global
    /// attachment protection (that set is shared with the primary transcript
    /// and overwritten wholesale).
    pub(super) doc_override: Option<String>,
    /// Whether an override instance watches a LIVE doc (`for_doc(follow)`):
    /// only then may the working trailer render — a frozen snapshot must
    /// never spin, whatever its entries claim.
    doc_live: bool,
    /// Memory-only viewport state for primary chats visited in this window.
    /// A transcript instance is shared across tabs, so the active ListState is
    /// reset on every attach and cannot retain these positions by itself.
    pub(super) saved_viewports: SavedViewportCache,
    /// An anchored viewport waiting for the selected chat's async replay.
    pub(super) pending_viewport: Option<SavedViewport>,
    /// Generation of the selected chat, guarding post-layout restoration
    /// callbacks across rapid A→B→A navigation.
    pub(super) viewport_generation: u64,
    /// A restored item anchor needs one post-layout refresh of distance-based
    /// UI state; programmatic list scrolling never invokes `handle_scroll`.
    pub(super) viewport_finalize_pending: bool,
    pub(super) viewport_finalize_scheduled: bool,
    /// Bumped whenever sync or own-turn logic invalidates measured rows. The
    /// post-restore finalizer waits until one layout completes without another
    /// invalidation, avoiding a stale jump-button decision.
    pub(super) viewport_layout_revision: u64,
    /// One-shot "open at the latest content" for UNPINNED (frozen) override
    /// instances: rows land ASYNC after the tab opens (watch replay / blob
    /// fetch), so the end-scroll fires on the first non-empty sync, then
    /// never again — landing at the end and FOLLOWING it are different
    /// states, and the user owns the viewport from there. Pinned instances
    /// don't need it (the pin branch already opens at the end).
    land_end_pending: bool,
    pub(super) row_cache: HashMap<String, CachedRows>,
    live_parsers: HashMap<String, IncrementalParser>,
    tree_cache: HashMap<String, (usize, Arc<BlockTree>)>,
    pub(crate) folds: HashMap<SharedString, FoldState>,
    /// Entrance state follows stable groups through completion so fast calls
    /// finish revealing. Replay rows have no entrance timestamps.
    pub(crate) tool_group_reveals: HashMap<SharedString, ToolGroupReveal>,
    last_replay_baseline: Option<Arc<zeron_doc::TranscriptBaseline>>,
    /// Parsed historical prefixes, used to seed text before a coalesced live
    /// suffix is painted. The wire watermark contains lengths, not text.
    historical_markdown: HashMap<SharedString, Row>,
    /// Detail folds (output/diff) per chip, keyed `"{row_id}#d{ix}"` — full
    /// [`FoldState`]s so detail bodies tween open/closed exactly like the
    /// group fold. Render-local like `folds` — never part of the row
    /// fingerprint.
    pub(crate) tool_details: HashMap<SharedString, FoldState>,
    /// Expand/collapse state for user bubbles past [`USER_COLLAPSED_LINES`],
    /// keyed by row id. Render-local like `folds` — never part of the row
    /// fingerprint, so toggling one costs a repaint, not a rebuild.
    user_folds: HashMap<SharedString, FoldState>,
    /// Full laid-out text heights for long user bubbles. The text's paint
    /// canvas writes these cells without notifying or mutating the transcript;
    /// click handlers read them as exact endpoints for the RESIZE tween. This
    /// preserves smooth layout motion without a paint → notify feedback loop.
    user_heights: HashMap<SharedString, Rc<Cell<f32>>>,
    /// Pending long-press toggle. A single task is enough because only one
    /// pointer can own a hold gesture at a time; a token invalidates stale
    /// timers when the pointer is released or moves into a text selection.
    pub(super) user_hold_task: Option<Task<()>>,
    pub(super) user_hold_token: u64,
    pub(super) user_collapse_scroll: Option<UserCollapseScroll>,
    /// Tracks the queued frame, even when its animation is canceled/replaced.
    /// Only that callback clears it, so rapid input cannot fork frame drivers.
    pub(super) user_collapse_scroll_scheduled: bool,
    /// Streaming fade veils, one per live markdown row (dropped on completion).
    veils: HashMap<SharedString, Rc<RefCell<RowVeil>>>,
    /// Live rows present in the transcript's REPLAY after (re)attaching to a
    /// chat: their veils are created pre-seeded, so text that was already
    /// streamed before the switch never fades in — only appends after it do
    /// (mugen's `FadePainter.attach` baseline; user report: switching back to
    /// a streaming session dissolved the entire reply).
    veil_baseline: std::collections::HashSet<SharedString>,
    /// Armed at attach, disarmed on the first sync whose transcript is
    /// non-empty: the baseline must be captured from the doc REPLAY frame,
    /// not the attach-time sync — selection clears the transcript and the
    /// replay lands async, so capturing at attach seeded nothing and the
    /// still-streaming reply faded in whole on every session switch (user
    /// report, round 2).
    veil_attach_pending: bool,
    /// Cross-frame flatten/shape-input cache (see [`RenderCache`]): fade
    /// frames reuse settled blocks' text+runs; the incremental parser's stable
    /// boundary invalidates only the live tail per commit.
    render_cache: Rc<RefCell<RenderCache>>,
    workspace_link: Option<render::LinkUi>,
    rendered_rows: HashSet<SharedString>,
    /// Last UI typography generation reflected in `list` item measurements.
    /// Family and size changes can alter prose wrapping without changing row
    /// identity, so the virtual list must explicitly discard cached heights.
    typography_generation: u32,
    content_width: f32,
    /// Last global code-fence layout generation applied to this transcript.
    /// Each instance owns separate scroll handles and list measurements, so
    /// every one must reset itself after a global Fit-mode transition.
    code_fences_generation: u64,
    pub(crate) highlights: HighlightStore,
    pub(super) show_jump_button: bool,
    /// Distance from the bottom at the last observation (wheel event or spring
    /// tick) — restick and escape are direction-aware
    /// (see [`Transcript::should_restick`]).
    pub(super) last_scroll_distance: f32,
    /// The stick-to-bottom pin. Broken only by user input (wheel/touch up);
    /// re-engaged inside the 70px band, after an own-send first overflows, and
    /// on the jump button.
    pub(super) pinned: bool,
    /// A locally-sent prompt currently held near the viewport top while its
    /// reply grows into the empty space below it.
    pub(super) own_turn: Option<OwnTurnAnchor>,
    /// Queue rows authored in this window. They become own-turn anchors only
    /// after the host promotes their stable id into a transcript message.
    pub(super) pending_queued_turns: PendingQueuedTurns,
    /// A layout-affecting change needs one post-layout own-turn measurement.
    pub(super) own_turn_kick: bool,
    /// One own-turn `on_next_frame` callback in flight at most.
    pub(super) own_turn_scheduled: bool,
    /// Wall-clock of the previous entry-glide tick (`None` = not gliding).
    pub(super) own_turn_last_tick: Option<Instant>,
    pub(super) spring: StickSpring,
    /// Wall-clock of the previous spring tick (`None` = parked).
    pub(super) spring_last_tick: Option<Instant>,
    /// When the spring last landed on the bottom (settle-grace bookkeeping).
    pub(super) spring_settled_at: Option<Instant>,
    /// A doc commit / wake happened before layout measured it — run at least
    /// one spring tick even though the pre-layout distance still reads 0.
    pub(super) spring_kick: bool,
    /// One `on_next_frame` callback in flight at most.
    pub(super) spring_scheduled: bool,
    pub(super) scroll_anim: Option<Task<()>>,
    /// Last pointer sample while markdown selection owns a left-button drag.
    pub(super) selection_drag_position: Option<Point<Pixels>>,
    /// One-shot timer rescheduled only while the pointer remains in an edge
    /// zone. Dropping it on mouse-up stops all selection scroll work.
    pub(super) selection_scroll_task: Option<Task<()>>,
    /// MessageRail width gate (set by the shell from the container width).
    rail_enabled: bool,
    /// Height of the shell's composer/status/terminal stack overlaying the
    /// transcript's bottom (measured last frame): the last row pads past it
    /// so pinned content rests above the glass chrome it scrolls under.
    bottom_clearance: f32,
    /// Hovered rail tick (grows + shows the preview card).
    rail_hover: Option<usize>,
    /// `(row id, entry id)` under the pointer — reveals the entry's timestamp
    /// strip (zeron chat-view.tsx `group-hover`; the rows report hover
    /// themselves). Keyed by ROW so a row→row move within one entry can't
    /// clear the reveal when the old row's leave event arrives after the new
    /// row's enter (enter/leave order across rows is not guaranteed).
    hovered_entry: Option<(SharedString, SharedString)>,
    /// Code block showing "Copied" feedback: `(row id, block ix)`, cleared by
    /// the companion task after ~1.2s.
    pub(crate) copied_code: Option<(SharedString, usize)>,
    pub(crate) copied_clear: Option<Task<()>>,
    /// Per-visible-fence horizontal offsets and scrollbar hover/drag state.
    /// Keys use the transcript's stable row identity, so streaming → settled
    /// rerenders keep their local scroll position without leaking state for
    /// blocks no longer present in the selected chat.
    pub(crate) code_fences: HashMap<SharedString, render::CodeFenceRuntime>,
    /// Entry whose hover action is showing transient copied-check feedback.
    copied_message: Option<SharedString>,
    copied_message_clear: Option<Task<()>>,
    /// Transcript attachment being viewed full-size (click a user thumbnail).
    pub(super) attachment_preview: Option<crate::attachments::PreviewImage>,
    /// Focused while the lightbox is open so Escape reaches it.
    pub(super) attachment_preview_focus: gpui::FocusHandle,
    pub(super) attachment_preview_return_focus: Option<gpui::FocusHandle>,
    /// In-flight ReadAttachmentChunk loads, keyed by device, path and validation
    /// policy; results land in the global attachment cache.
    pub(super) attachment_loads: HashMap<crate::attachments::AttachmentKey, Task<()>>,
    /// Scheduled retry wake-ups for errored sources (the 2s→15s ladder).
    pub(super) attachment_retries: HashMap<(String, String), Task<()>>,
    /// Sidecar blob fetches keyed by doc ref (`chatId/partId[.diff]`,
    /// chat2-sync A3). `Ready` holds the UPGRADED detail, built once on
    /// arrival — render swaps it in per chip; rows never rebuild for it.
    /// Deliberately NOT cleared on chat switch: refs are chat-qualified and a
    /// fetched blob stays valid.
    pub(crate) blob_details: HashMap<SharedString, BlobFetch>,
    /// Monotonic fetch order per blob ref: when a tool has BOTH a diff and
    /// an output blob fetched, the chip shows the one requested most
    /// recently (click "Show full output" after a diff → see the output).
    pub(crate) blob_fetch_order: HashMap<SharedString, u64>,
    pub(super) blob_fetch_counter: u64,
    _observe: Subscription,
    _text_changes: Subscription,
}

/// Shell-facing events (the transcript itself hosts no surfaces).
#[derive(Debug, Clone)]
pub enum TranscriptEvent {
    /// A spawn chip's "Open subagent" affordance: open the subagent's
    /// transcript as a right-pane tab. `chat_id` is the doc the chip lives
    /// in (the frozen blob is keyed `{chat_id}/{doc_id}`); `frozen` means
    /// the subagent finished — try the blob before watching the doc.
    OpenSubagent {
        chat_id: String,
        doc_id: String,
        title: String,
        frozen: bool,
    },
}

impl gpui::EventEmitter<TranscriptEvent> for Transcript {}

impl Transcript {
    pub(crate) fn set_workspace_link_handler(&mut self, handler: render::LinkUi) {
        self.workspace_link = Some(handler);
    }

    pub(crate) fn link_ui(&self) -> Option<render::LinkUi> {
        self.workspace_link.clone().map(|mut link| {
            if link.source_session.is_none() {
                link.source_session = self.chat_id.clone();
            }
            link
        })
    }

    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        Self::build(state, None, true, cx)
    }

    /// A read-only transcript over one SUBAGENT doc (right-pane tab). The
    /// caller starts the feed (`watch_subagent_doc` or the frozen snapshot);
    /// this instance only renders whatever lands under `doc_id`. `follow` =
    /// the doc is live: engage the end-follow pin from the start. Either
    /// way the tab OPENS at the latest content — a frozen transcript lands
    /// at the end once, unpinned, and free-scrolls from there.
    pub fn for_doc(
        state: Entity<AppState>,
        doc_id: String,
        follow: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::build(state, Some(doc_id), follow, cx)
    }

    fn build(
        state: Entity<AppState>,
        doc_override: Option<String>,
        follow: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        // FollowMode stays Normal: the tail pin is ours (a per-frame spring),
        // not the list's per-layout hard snap.
        //
        // Override instances align TOP: a subagent transcript reads like a
        // fresh notes page — entries anchored at the top, streaming growing
        // into the empty space below, never rising from the pane's bottom.
        // Top alignment gets that structurally (a short list rests at the
        // top with no reservation pad), and the PIN machinery still runs on
        // top of it for end-follow: the spring is purely distance-based, and
        // the glue trap it was built around is Bottom-only — layout
        // materializes a Top list's past-end offset to a CONCRETE position
        // every frame (gpui list.rs: only `Bottom` re-glues to the `None`
        // sentinel), so a parked spring can't re-glue and hard-track growth.
        let alignment = if doc_override.is_some() {
            ListAlignment::Top
        } else {
            ListAlignment::Bottom
        };
        let list = ListState::new(0, alignment, px(OVERDRAW_PX));
        let weak = cx.weak_entity();
        list.set_scroll_handler(move |event: &ListScrollEvent, _window, cx| {
            weak.update(cx, |this: &mut Transcript, cx| {
                this.handle_scroll(event, cx)
            })
            .ok();
        });
        let observe = cx.observe(&state, |this: &mut Self, _, cx| this.sync(cx));
        let text_changes = cx.subscribe(
            &state,
            |this: &mut Self, state, event: &crate::state::TranscriptTextChanged, cx| {
                let doc_id = this
                    .doc_override
                    .as_deref()
                    .or_else(|| state.read(cx).selected_chat.as_deref());
                if doc_id == Some(event.doc_id.as_str()) {
                    this.sync(cx);
                }
            },
        );
        // The rail is sized for the conversation column; a narrow right-pane
        // tab has no width gate driving it, so override instances skip it.
        let rail_enabled = doc_override.is_none();
        // `follow` is the initial pin: the primary transcript always opens
        // pinned; an override instance pins only while its doc is LIVE (a
        // frozen transcript reads top-down, free-scrolling). Short content
        // is at-end by definition (distance 0), so the pin is invisible
        // until streaming overflows the pane — then it follows, releases on
        // wheel-up, and resticks/jumps exactly like the main transcript.
        let pinned = follow;
        let mut this = Self {
            state,
            list,
            rows: Vec::new(),
            last_source: None,
            // Pre-set so `sync` never sees an attach edge — an override
            // instance must not reset (or re-pin) on selection changes.
            chat_id: doc_override.clone(),
            retain_on_deselect: false,
            land_end_pending: doc_override.is_some() && !follow,
            doc_live: doc_override.is_some() && follow,
            doc_override,
            saved_viewports: SavedViewportCache::default(),
            pending_viewport: None,
            viewport_generation: 0,
            viewport_finalize_pending: false,
            viewport_finalize_scheduled: false,
            viewport_layout_revision: 0,
            row_cache: HashMap::new(),
            live_parsers: HashMap::new(),
            tree_cache: HashMap::new(),
            folds: HashMap::new(),
            tool_group_reveals: HashMap::new(),
            last_replay_baseline: None,
            historical_markdown: HashMap::new(),
            tool_details: HashMap::new(),
            user_folds: HashMap::new(),
            user_heights: HashMap::new(),
            user_hold_task: None,
            user_hold_token: 0,
            user_collapse_scroll: None,
            user_collapse_scroll_scheduled: false,
            veils: HashMap::new(),
            veil_baseline: std::collections::HashSet::new(),
            veil_attach_pending: true,
            render_cache: Rc::new(RefCell::new(RenderCache::default())),
            workspace_link: None,
            rendered_rows: HashSet::new(),
            typography_generation: crate::typography::generation(cx),
            content_width: crate::settings::transcript_width(cx),
            code_fences_generation: crate::settings::code_fences_generation(cx),
            highlights: HighlightStore::default(),
            show_jump_button: false,
            last_scroll_distance: 0.0,
            pinned,
            own_turn: None,
            pending_queued_turns: PendingQueuedTurns::default(),
            own_turn_kick: false,
            own_turn_scheduled: false,
            own_turn_last_tick: None,
            spring: StickSpring::new(),
            spring_last_tick: None,
            spring_settled_at: None,
            spring_kick: false,
            spring_scheduled: false,
            scroll_anim: None,
            selection_drag_position: None,
            selection_scroll_task: None,
            rail_enabled,
            bottom_clearance: 0.0,
            rail_hover: None,
            hovered_entry: None,
            copied_code: None,
            copied_clear: None,
            code_fences: HashMap::new(),
            copied_message: None,
            copied_message_clear: None,
            attachment_preview: None,
            attachment_preview_focus: cx.focus_handle(),
            attachment_preview_return_focus: None,
            attachment_loads: HashMap::new(),
            attachment_retries: HashMap::new(),
            blob_details: HashMap::new(),
            blob_fetch_order: HashMap::new(),
            blob_fetch_counter: 0,
            _observe: observe,
            _text_changes: text_changes,
        };
        this.sync(cx);
        this
    }

    // ---- rail plumbing (rendering lives in crate::rail) ----

    /// Shell-driven width gate: the rail hides below 48rem of container width.
    pub fn set_rail_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.rail_enabled != enabled {
            self.rail_enabled = enabled;
            cx.notify();
        }
    }

    pub(crate) fn rail_enabled(&self) -> bool {
        self.rail_enabled
    }

    /// Shell-driven: the measured height of the bottom chrome stack the
    /// transcript scrolls under. Sub-pixel jitter is ignored so steady-state
    /// frames don't re-notify.
    pub fn set_bottom_clearance(&mut self, height: f32, cx: &mut Context<Self>) {
        if (self.bottom_clearance - height).abs() > 0.5 {
            self.bottom_clearance = height;
            if self.own_turn.is_some() {
                self.remeasure_last_row();
                self.own_turn_kick = true;
            }
            cx.notify();
        }
    }

    pub(crate) fn rail_hover(&self) -> Option<usize> {
        self.rail_hover
    }

    pub(crate) fn set_rail_hover(&mut self, hover: Option<usize>) {
        self.rail_hover = hover;
    }

    pub(crate) fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub(crate) fn list_state(&self) -> &ListState {
        &self.list
    }

    pub(crate) fn state_entity(&self) -> &Entity<AppState> {
        &self.state
    }

    pub(crate) fn retain_for_route_exit(&mut self) {
        self.retain_on_deselect = true;
    }

    pub(super) fn route_exit_pending(&self, cx: &gpui::App) -> bool {
        self.retain_on_deselect
            && self.doc_override.is_none()
            && self.state.read(cx).selected_chat.is_none()
            && self.chat_id.is_some()
    }

    pub(crate) fn finish_route_exit(&mut self, cx: &mut Context<Self>) {
        if self.state.read(cx).selected_chat.is_none() && self.chat_id.is_some() {
            self.retain_on_deselect = false;
            self.sync(cx);
            self.retain_on_deselect = true;
        }
    }

    /// Rebuild rows from app state; splice minimal ranges into the list.
    fn sync(&mut self, cx: &mut Context<Self>) {
        if self.retain_on_deselect
            && self.doc_override.is_none()
            && self.state.read(cx).selected_chat.is_none()
            && self.chat_id.is_some()
        {
            // A quick return can reuse this entity before the exit finishes.
            // Its next snapshot is still a replay, not newly arriving tools.
            self.veil_attach_pending = true;
            return;
        }
        let (selected, replay) = {
            let s = self.state.read(cx);
            match &self.doc_override {
                // Pinned to a subagent doc: `selected` equals `chat_id` by
                // construction, so the attach/reset branch below never fires,
                // and echoes stay empty (nothing is ever sent from here).
                Some(doc_id) => (Some(doc_id.clone()), TranscriptReplayState::Populated),
                None => {
                    let replay = if !s.transcript_replayed {
                        TranscriptReplayState::Pending
                    } else if s.transcript.is_empty() {
                        TranscriptReplayState::Empty
                    } else {
                        TranscriptReplayState::Populated
                    };
                    (s.selected_chat.clone(), replay)
                }
            }
        };

        let source = (
            selected.clone(),
            replay,
            self.state.read(cx).transcript_revision,
        );
        if self.last_source.as_ref() == Some(&source) {
            return;
        }
        self.last_source = Some(source);

        let attached = selected != self.chat_id;
        // Arm the replay baseline before classifying tool arrivals. Selection
        // and replay may arrive in one sync; a retained same-chat entity can
        // also see a fresh pending subscription without changing chat_id.
        if attached || replay == TranscriptReplayState::Pending {
            self.veil_baseline.clear();
            self.veil_attach_pending = true;
        }
        if attached {
            // Read the incoming snapshot before inserting the outgoing one:
            // a full bounded cache may evict its oldest entry, which can be
            // exactly the chat the user is reopening.
            let saved_viewport = selected
                .as_ref()
                .and_then(|chat_id| self.saved_viewports.get_cloned_and_touch(chat_id));
            self.remember_current_viewport();
            let keep_own_turn = self
                .own_turn
                .as_ref()
                .is_some_and(|anchor| selected.as_deref() == Some(anchor.chat_id.as_str()));
            if !keep_own_turn {
                self.own_turn = None;
                self.own_turn_kick = false;
                self.own_turn_last_tick = None;
            }
            self.chat_id = selected;
            self.rows.clear();
            self.row_cache.clear();
            self.live_parsers.clear();
            self.tree_cache.clear();
            self.folds.clear();
            self.tool_group_reveals.clear();
            self.last_replay_baseline = None;
            self.historical_markdown.clear();
            self.user_folds.clear();
            self.user_heights.clear();
            self.user_hold_token = self.user_hold_token.wrapping_add(1);
            self.user_hold_task = None;
            self.user_collapse_scroll = None;
            self.veils.clear();
            self.render_cache.borrow_mut().clear();
            self.highlights.entries.clear();
            self.copied_message = None;
            self.copied_message_clear = None;
            self.list.reset(0);
            self.pending_viewport = None;
            self.viewport_generation = self.viewport_generation.wrapping_add(1);
            self.viewport_finalize_pending = false;
            if self.own_turn.is_some() {
                // A kept own-turn hold (send-created chat) owns the viewport.
                self.pinned = false;
                self.last_scroll_distance = 0.0;
                self.show_jump_button = false;
            } else if let Some(SavedViewport::Anchored {
                anchor,
                distance_from_bottom,
                own_turn,
            }) = saved_viewport
            {
                // Keep a possible runway pending until replay confirms that
                // its optimistic prompt still exists. Installing it on this
                // empty attach frame can leave a failed send's stale anchor
                // intercepting scroll-to-bottom forever.
                self.pinned = false;
                self.last_scroll_distance = distance_from_bottom;
                self.show_jump_button = distance_from_bottom > SCROLL_BUTTON_THRESHOLD_PX;
                self.pending_viewport = Some(SavedViewport::Anchored {
                    anchor,
                    distance_from_bottom,
                    own_turn,
                });
            } else {
                // New chats and chats that were following their tail retain
                // the existing open-at-bottom behavior.
                self.pinned = true;
                self.last_scroll_distance = 0.0;
                self.show_jump_button = false;
            }
            self.spring.reset();
            self.spring_last_tick = None;
            self.spring_settled_at = None;
            self.spring_kick = false;
            self.scroll_anim = None;
            self.stop_selection_scroll();
        }

        let mut new_rows: Vec<Row> = Vec::new();
        // Borrow the transcript only while deriving rows. Cloning the entity
        // handle lets rows_for mutate our caches without copying every text
        // and tool payload on each app-state notification.
        let (entries_empty, tail_streaming) = {
            let state = self.state.clone();
            let state = state.read(cx);
            let entries = match &self.doc_override {
                Some(doc_id) => state.sub_transcript(doc_id),
                None => state.transcript.as_slice(),
            };
            let prepared = self
                .chat_id
                .as_ref()
                .and_then(|id| state.prepared_transcripts.get(id));
            for entry in entries {
                if let Some(rows) = prepared.and_then(|p| p.rows.get(&entry.id)) {
                    new_rows.extend(rows.iter().cloned());
                } else {
                    new_rows.extend(self.rows_for(entry, false));
                }
            }
            if self.doc_override.is_none() {
                for echo in state.pending_echoes() {
                    new_rows.extend(self.rows_for(echo, true));
                }
            }
            (
                entries.is_empty(),
                entries
                    .last()
                    .is_some_and(|e| e.status == Some(MessageStatus::Streaming)),
            )
        };

        let baseline = self
            .chat_id
            .as_deref()
            .and_then(|id| self.state.read(cx).transcript_baseline(id))
            .cloned();
        let baseline_changed = baseline.as_ref().is_some_and(|baseline| {
            self.last_replay_baseline
                .as_ref()
                .is_none_or(|previous| !Arc::ptr_eq(previous, baseline))
        });
        let mut historical_tools: HashMap<SharedString, HashSet<String>> = HashMap::new();
        let mut fully_historical: HashSet<SharedString> = HashSet::new();
        if baseline_changed {
            let baseline = baseline.as_ref().unwrap();
            let state = self.state.read(cx);
            let entries = match &self.doc_override {
                Some(id) => state.sub_transcript(id),
                None => &state.transcript,
            };
            let previous_markdown = std::mem::take(&mut self.historical_markdown);
            let prepared = self
                .chat_id
                .as_ref()
                .and_then(|id| state.prepared_transcripts.get(id));
            let mut historical_rows = Vec::new();
            for entry in entries {
                let covered = prepared.map_or_else(
                    || baseline.covers(entry),
                    |p| {
                        Arc::ptr_eq(baseline, &p.navigation_baseline)
                            || p.fully_historical.contains(&entry.id)
                    },
                );
                if covered {
                    fully_historical.insert(entry.id.clone().into());
                    continue;
                }
                if let Some(rows) = self
                    .chat_id
                    .as_ref()
                    .and_then(|id| state.prepared_transcripts.get(id))
                    .and_then(|p| p.historical.get(&entry.id))
                {
                    historical_rows.extend(rows.iter().cloned());
                    continue;
                }
                let Some(historical) = baseline.historical_entry(entry) else {
                    continue;
                };
                historical_rows.extend(rows_for_entry(&historical, false, &mut |_, text| {
                    Arc::new(parse_full(text))
                }));
            }
            // A normal opening snapshot is entirely historical. Share its
            // parsed trees instead of parsing the whole transcript twice;
            // only an entry mixing historical and live parts needs a prefix.
            historical_rows.extend(
                new_rows
                    .iter()
                    .filter(|row| {
                        fully_historical.contains(&row.entry_id)
                            && matches!(row.kind, RowKind::LiveMarkdown { .. })
                    })
                    .cloned(),
            );
            for row in historical_rows {
                match &row.kind {
                    RowKind::ToolGroup { tools, .. }
                        if !fully_historical.contains(&row.entry_id) =>
                    {
                        historical_tools
                            .entry(row.entry_id.clone())
                            .or_default()
                            .extend(tools.iter().map(|tool| tool.part_id.clone()));
                    }
                    RowKind::LiveMarkdown { .. } => {
                        if previous_markdown
                            .get(&row.id)
                            .is_none_or(|old| old.version != row.version)
                        {
                            self.veils.remove(&row.id);
                        }
                        self.historical_markdown.insert(row.id.clone(), row);
                    }
                    _ => {}
                }
            }
            self.last_replay_baseline = Some(baseline.clone());
        }

        // Give only rows that ARRIVE after the replay baseline an entrance.
        // The first populated frame after a chat attach may already contain a
        // live tool group; treating it as history prevents a whole existing
        // task tree from reanimating on every chat switch.
        let replay_baseline = self.veil_attach_pending && !entries_empty;
        if replay_baseline {
            // Retain explicit user pins, but never resume an old arrival or
            // closing animation when revisiting the retained transcript.
            self.tool_group_reveals.clear();
            for fold in self.folds.values_mut() {
                fold.toggled_at = None;
                fold.disclosure_at = None;
            }
        }
        let previous_tools: HashMap<SharedString, HashMap<String, Option<Instant>>> = self
            .rows
            .iter()
            .filter(|row| !fully_historical.contains(&row.entry_id))
            .filter_map(|row| match &row.kind {
                RowKind::ToolGroup { tools, .. } => {
                    let reveal = self.tool_group_reveals.get(&row.id);
                    Some((
                        row.id.clone(),
                        tools
                            .iter()
                            .enumerate()
                            .map(|(ix, tool)| {
                                (
                                    tool.part_id.clone(),
                                    reveal.and_then(|r| r.starts.get(ix).copied().flatten()),
                                )
                            })
                            .collect(),
                    ))
                }
                _ => None,
            })
            .collect();
        let now = Instant::now();
        let mut live_tool_groups = HashSet::new();
        for row in &new_rows {
            let RowKind::ToolGroup { tools, .. } = &row.kind else {
                continue;
            };
            // Agent/spawn groups are standalone cards, not task trees.
            if !tool_group_collapses(tools) {
                continue;
            }
            live_tool_groups.insert(row.id.clone());
            let historical = historical_tools.get(&row.entry_id);
            let whole_group_historical =
                fully_historical.contains(&row.entry_id) || (replay_baseline && baseline.is_none());
            let is_historical = |tool: &ToolItem| {
                whole_group_historical || historical.is_some_and(|ids| ids.contains(&tool.part_id))
            };
            let historical_count = if whole_group_historical {
                tools.len()
            } else {
                tools.iter().filter(|tool| is_historical(tool)).count()
            };
            let previous = previous_tools.get(&row.id);
            let is_new_group = historical_count == 0 && previous.is_none();
            let reveal = self.tool_group_reveals.entry(row.id.clone()).or_default();
            if baseline_changed && historical_count > 0 {
                reveal.rendered_open = None;
                if let Some(fold) = self.folds.get_mut(&row.id) {
                    fold.toggled_at = None;
                    fold.disclosure_at = None;
                }
            }
            reveal.shimmer_started_at.get_or_insert(now);
            if is_new_group {
                reveal.header_started_at.get_or_insert(now);
            }
            if baseline_changed && historical_count == tools.len() {
                reveal.header_started_at = None;
            }
            let first_row_delay = is_new_group.then_some(TOOL_FIRST_ROW_DELAY_MS).unwrap_or(0);
            let mut arrival_ix = 0;
            if whole_group_historical {
                reveal.starts.clear();
                reveal.starts.resize(tools.len(), None);
                continue;
            }
            reveal.starts = tools
                .iter()
                .map(|tool| {
                    if is_historical(tool) {
                        return None;
                    }
                    if let Some(start) = previous.and_then(|tools| tools.get(&tool.part_id)) {
                        return *start;
                    }
                    let start = now
                        + Duration::from_millis(first_row_delay + arrival_ix * TOOL_ROW_STAGGER_MS);
                    arrival_ix += 1;
                    Some(start)
                })
                .collect();
        }
        self.tool_group_reveals
            .retain(|row_id, _| live_tool_groups.contains(row_id));

        // Runtime scroll handles follow the stable code rows exactly. A live
        // block keeps its handle through completion; deleted/reindexed tail
        // blocks and the previous chat cannot accumulate stale handles.
        let active_code_fences: HashSet<SharedString> = new_rows
            .iter()
            .flat_map(|row| match &row.kind {
                RowKind::Markdown { tree, block_ix } | RowKind::LiveMarkdown { tree, block_ix } => {
                    tree.blocks
                        .get(*block_ix)
                        .map(|top| {
                            render::code_block_indices(&top.block, *block_ix)
                                .into_iter()
                                .map(|ix| format!("{}#code{ix}", row.id).into())
                                .collect()
                        })
                        .unwrap_or_default()
                }
                _ => Vec::new(),
            })
            .collect();
        self.code_fences
            .retain(|key, _| active_code_fences.contains(key));

        // Text already streamed before this (re)attach is the veil BASELINE:
        // its rows' veils seed instead of fading (render creates them from
        // this set), so only post-switch appends animate. Captured from the
        // first NON-EMPTY transcript after attach — the replay frame — never
        // the attach-time sync, whose transcript is still empty (selection
        // clears it; the doc watch refills it async).
        if self.veil_attach_pending
            && (!entries_empty || replay.authoritative_empty() || baseline_changed)
        {
            self.veil_attach_pending = false;
            self.veil_baseline = new_rows
                .iter()
                .filter(|r| baseline.is_none() && matches!(r.kind, RowKind::LiveMarkdown { .. }))
                .map(|r| r.id.clone())
                .collect();
        }

        // Veils live exactly as long as their live row — drop them on the
        // live→complete flip (any mid-fade chunk snaps to full, matching the
        // row's version splice).
        let active_markdown: HashSet<&SharedString> = new_rows
            .iter()
            .filter(|row| matches!(row.kind, RowKind::LiveMarkdown { .. }))
            .map(|row| &row.id)
            .collect();
        self.veils.retain(|id, _| active_markdown.contains(id));
        self.veil_baseline.retain(|id| active_markdown.contains(id));
        self.historical_markdown
            .retain(|id, _| active_markdown.contains(id));

        // Capture this before the row splice changes the list's measured end.
        // When the user is truly live-following, retaining the end anchor
        // keeps the in-flow working trailer at the same viewport position as
        // transcript lines grow above it. Nothing about the trailer's layout
        // or coordinates changes.
        let live_following =
            should_anchor_live_stream(self.pinned, self.distance_from_bottom(), tail_streaming);
        let was_empty = self.rows.is_empty();
        let old_last = self.rows.len().checked_sub(1);
        match diff_rows(&self.rows, &new_rows) {
            None => {
                self.rows = new_rows;
                self.refresh_protected_attachments(cx);
                self.reconcile_own_turn_prompt();
                // Replay readiness is independent of row content: an empty
                // reset (or one identical to optimistic rows) still resolves
                // or retires the pending viewport.
                if self.restore_pending_viewport(replay) {
                    cx.notify();
                }
                self.promote_materialized_queued_turn(attached, cx);
                return;
            }
            Some((old_range, count)) => {
                // Any replaced row's cached flatten results are stale — and
                // because live replies splice only the rows whose content hash
                // changed (the tail), this is O(changed rows) per commit, never
                // O(reply).
                for row in &self.rows[old_range.clone()] {
                    self.render_cache.borrow_mut().invalidate_row(&row.id);
                }
                if old_range.len() == count {
                    // In-place content change, same row count — notably the
                    // live→complete flip, where EVERY row of the streamed
                    // message changes version (streaming bit, tool auto_open,
                    // timestamp bit) with identical ids. `splice` would reset
                    // those items to hint-less Unmeasured (heights read 0
                    // until the next paint) and, when the viewport-top item is
                    // inside the range, clobber the scroll anchor to the range
                    // start — the end-of-turn up/down jump the spring then has
                    // to walk back. `remeasure_items` keeps old sizes as hints
                    // and holds the anchor across the remeasure.
                    self.list.remeasure_items(old_range);
                } else {
                    self.list.splice(old_range, count);
                }
                self.viewport_layout_revision = self.viewport_layout_revision.wrapping_add(1);
            }
        }
        self.rows = new_rows;
        if old_last != self.rows.len().checked_sub(1) {
            if let Some(ix) = old_last.filter(|&ix| ix < self.rows.len()) {
                // Bottom chrome moves to the new tail too.
                self.list.remeasure_items(ix..ix + 1);
            }
            if was_empty && self.own_turn.is_some() && !self.rows.is_empty() {
                // There was no concrete row to materialize at send time.
                // Start the echo at the bottom edge before adding its runway.
                self.list.scroll_to(ListOffset {
                    item_ix: 0,
                    offset_in_item: -self.list.viewport_bounds().size.height,
                });
            }
        }
        self.refresh_protected_attachments(cx);
        self.reconcile_own_turn_prompt();
        self.restore_pending_viewport(replay);
        self.promote_materialized_queued_turn(attached, cx);
        if self.land_end_pending && !self.rows.is_empty() {
            // First content for an unpinned override tab: land at the end.
            // `scroll_to_end` is ITEM-anchored (past-the-end offset that the
            // next layout materializes) — a pixel scroll off `max_offset`
            // would land short here, since the freshly-spliced rows are
            // still unmeasured. Short content clamps back to the top under
            // Top alignment, so "end" and "top" coincide there.
            self.land_end_pending = false;
            self.list.scroll_to_end();
        }
        if self.own_turn.is_some() {
            self.own_turn_kick = true;
        }
        if self.pinned {
            if live_following || baseline_changed {
                self.list.scroll_to_end();
                self.spring.reset();
                self.spring_last_tick = None;
                self.spring_settled_at = None;
                self.spring_kick = false;
                self.last_scroll_distance = 0.0;
            } else {
                if motion::reduced_motion(cx) || was_empty {
                    // First fill (chat open) lands at the bottom instantly
                    // (mugen initialScroll:'bottom'); reduced motion snaps.
                    self.list.scroll_to_end();
                } else if self.is_glued() {
                    // A glued offset (`None` / anchored past the end) makes
                    // the upcoming layout hard-snap to the new end — the
                    // per-commit stutter. Materialize a pixel anchor a hair
                    // above the bottom so layout holds position and the
                    // spring glides the growth.
                    self.list.scroll_by(px(-0.75));
                }
                self.spring_kick = true;
            }
        }
        cx.notify();
    }

    /// The inside of a user bubble: the prompt text, clipped to
    /// [`USER_COLLAPSED_LINES`] until expanded, plus the expander chevron for
    /// prompts past the cap. Returns the bubble's children in order.
    ///
    /// The collapsed form clips a normally-laid-out text element at exactly
    /// five line boxes. Do not use gpui's `line_clamp` here: on an auto-width
    /// flex item it answers intrinsic-width probes with the truncated layout,
    /// collapsing the bubble to min-content width (one character per line).
    /// A plain height clip preserves the original bubble width calculation and
    /// never feeds measured layout back into the virtualized list.
    fn render_user_body(
        &mut self,
        row_id: &SharedString,
        row_ix: usize,
        text: SharedString,
        mentions: Arc<Vec<crate::composer::SentMentionSpan>>,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let fold = self.user_folds.get(row_id).copied().unwrap_or_default();
        let expanded = fold.open.unwrap_or(false);
        let line_height =
            f32::from(crate::typography::ui_rems(USER_LINE_HEIGHT).to_pixels(window.rem_size()));
        let collapsed_text_h = USER_COLLAPSED_LINES as f32 * line_height;
        // Include the continuation line in the resize endpoints so removing
        // it on expansion does not make the bubble jump by a line.
        let collapsed_h = collapsed_text_h + line_height;
        let measured_h = self
            .user_heights
            .entry(row_id.clone())
            .or_insert_with(|| Rc::new(Cell::new(0.0)))
            .clone();
        let measured = measured_h.get();
        let collapsible = text.lines().count() > USER_COLLAPSED_LINES
            || (measured > 0.0 && measured > collapsed_text_h + 0.5)
            || (measured == 0.0 && user_message_needs_collapse(&text));
        let full_h = measured_h.get().max(collapsed_h);
        if let Some(fold) = self.user_folds.get_mut(row_id) {
            // Wrapping can change with the window width while expanded.
            fold.user_expansion_height = (full_h - collapsed_h).max(0.0);
        }

        let hold_key = row_id.clone();
        let hold_height = measured_h.clone();
        let hold_selection: Arc<str> = format!("{row_id}:u").into();
        let body = div()
            .id(SharedString::from(format!("{row_id}-body")))
            // A long press toggles instead of double-click. A normal release
            // remains available for text selection, and pointer movement
            // cancels the pending toggle before a drag can select text.
            .when(collapsible, |el| {
                let down_key = hold_key.clone();
                let down_height = hold_height.clone();
                let down_selection = hold_selection.clone();
                el.on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.arm_user_hold(
                            down_key.clone(),
                            row_ix,
                            collapsed_h,
                            down_height.clone(),
                            down_selection.clone(),
                            cx,
                        );
                    }),
                )
                .on_mouse_up(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, _, _, _| {
                        this.cancel_user_hold();
                    }),
                )
                .on_mouse_up_out(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, _, _, _| {
                        this.cancel_user_hold();
                    }),
                )
                .on_mouse_move(cx.listener(|this, _, _, _| {
                    this.cancel_user_hold();
                }))
            })
            .child(user_bubble_text(
                row_id,
                text,
                mentions,
                theme,
                measured_h.clone(),
                cx.entity_id(),
            ));
        // Height motion uses the same ease-out curve as sidebars, tool folds,
        // and pane transitions, with duration scaled to travel distance. The
        // full text remains laid out behind the clip; only the viewport over it
        // changes, so glyph wrapping never shifts.
        let duration_ms = fold
            .duration_ms
            .max(user_resize_duration_ms(full_h - collapsed_h));
        let animating = collapsible
            && fold.epoch > 0
            && fold
                .toggled_at
                .is_some_and(|at| at.elapsed() < Duration::from_millis(duration_ms + 200))
            && !motion::reduced_motion(cx);
        let ellipsis = || div().h(px(line_height)).child("...");
        let body: AnyElement = if animating {
            let from = fold.from;
            let to = if expanded { full_h } else { collapsed_h };
            let resize = user_resize_spec(full_h - collapsed_h);
            let ellipsis_h = if expanded { 0.0 } else { line_height };
            div()
                .child(div().overflow_hidden().child(body).with_animation(
                    SharedString::from(format!("{row_id}-user-resize-{}", fold.epoch)),
                    resize.animation(),
                    move |el, t| el.h(px((motion::lerp(from, to, t) - ellipsis_h).max(0.0))),
                ))
                .when(!expanded, |el| el.child(ellipsis()))
                .into_any_element()
        } else if collapsible && !expanded {
            div()
                .child(div().h(px(collapsed_text_h)).overflow_hidden().child(body))
                .child(ellipsis())
                .into_any_element()
        } else {
            body.into_any_element()
        };
        div()
            .relative()
            .child(body)
            .when(collapsible, |el| {
                el.child(self.render_user_expander(
                    row_id,
                    row_ix,
                    expanded,
                    collapsed_h,
                    measured_h,
                    theme,
                    cx,
                ))
            })
            .into_any_element()
    }

    // ---- rendering ----

    /// The working loader, INSIDE the conversation flow: appended under the
    /// last row while the run is live (moved out of the shell's status strip
    /// — user request), so it reads as part of the streaming reply and
    /// scrolls away with it. The spinner drives this entity's frames, which
    /// keeps the elapsed timer ticking through delta-quiet tool runs.
    /// The failed-send retry (trailer affordance): re-kick every delivery
    /// road engine-side (fresh chat2 socket, host nudge, delivery escorts)
    /// and restart the grace clock so the trailer returns to Sending/Queued
    /// while the retry runs.
    fn retry_send(&mut self, cx: &mut Context<Self>) {
        let Some(chat_id) = self.chat_id.clone() else {
            return;
        };
        let engine = self.state.read(cx).engine().cloned();
        self.state.update(cx, |s, cx| {
            s.retry_pending_send(&chat_id, chrono::Utc::now());
            cx.notify();
        });
        if let Some(engine) = engine {
            cx.spawn(async move |_, _| {
                let params = serde_json::json!({ "chatId": chat_id });
                if let Err(err) = engine
                    .client()
                    .call(zeron_rpc::methods::RETRY_DELIVERY, params)
                    .await
                {
                    tracing::warn!(error = %err, "delivery retry RPC failed");
                }
            })
            .detach();
        }
    }

    fn render_working_trailer(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let now = chrono::Utc::now();
        let (sending, queued, elapsed_secs, seed) = if let Some(doc_id) = &self.doc_override {
            // A subagent doc has no Session row — `indicator_for` would read
            // the PARENT chat's live state into this tab. Liveness rides the
            // doc itself instead: the sink's assistant entry streams until
            // the subagent settles (run teardown finalizes abandoned sinks),
            // and a trailing USER entry is a steer still awaiting its reply
            // segment. Frozen snapshots never spin, whatever they claim.
            if !self.doc_live {
                return None;
            }
            let state = self.state.read(cx);
            let last = state.sub_transcript(doc_id).last()?;
            let live =
                last.status == Some(MessageStatus::Streaming) || last.role == MessageRole::User;
            if !live {
                return None;
            }
            let elapsed = ((now.timestamp_millis() - last.created_at).max(0) / 1000) as i64;
            (false, false, elapsed, flavour_seed(doc_id))
        } else {
            let chat_id = self.chat_id.clone()?;
            // Failed-send state first: past the grace window the trailer IS
            // the retry affordance, whatever the indicator fell back to.
            if self.state.read(cx).send_undelivered(&chat_id, now) {
                let theme = Theme::of(cx).clone();
                return Some(
                    div()
                        .id("undelivered-retry")
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(Theme::SPACE_SM))
                        .pt(px(Theme::SPACE_LG))
                        .text_size(crate::typography::ui_rems(12.0))
                        .text_color(theme.danger)
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| this.retry_send(cx)))
                        .child(SharedString::from("Not delivered — click to retry"))
                        .into_any_element(),
                );
            }
            let (sending, queued, elapsed) = {
                let state = self.state.read(cx);
                if state.indicator_for(&chat_id, now) != crate::state::Indicator::Working {
                    return None;
                }
                // During the send→turn window the session row's `started_at`
                // still belongs to the PREVIOUS turn — a timer based on the
                // send counted the round-trip and then restarted when the
                // turn actually began (user report). Bridge it as "Sending…"
                // with no timer instead; the word + timer start with the
                // turn.
                let turn_started = state.session_for(&chat_id).and_then(|s| s.started_at);
                let sending =
                    sending_bridge(state.pending_send_started(&chat_id, now), turn_started);
                // Degraded delivery path: the send is a durable local write
                // waiting on connectivity — say so instead of faking
                // progress. (The overlay holds while degraded, so this line
                // owns the surface until the ack or the failed state.)
                let queued = sending && state.chat_delivery_degraded(&chat_id);
                let elapsed = turn_started
                    .map(|t| now.signed_duration_since(t).num_seconds().max(0))
                    .unwrap_or(0);
                (sending, queued, elapsed)
            };
            (sending, queued, elapsed, flavour_seed(&chat_id))
        };
        let word = if queued {
            "Queued — will send automatically"
        } else if sending {
            "Sending"
        } else {
            flavour_word(seed, elapsed_secs)
        };
        let theme = Theme::of(cx).clone();
        Some(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(Theme::SPACE_SM))
                .pt(px(Theme::SPACE_LG))
                .text_size(crate::typography::ui_rems(11.0))
                .child(crate::loaders::gradient_spinner(
                    "working-indicator",
                    &theme,
                    2.5,
                    cx.entity_id(),
                    cx,
                ))
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(12.0))
                        .text_color(if queued {
                            theme.warning
                        } else {
                            theme.text_muted
                        })
                        .child(SharedString::from(if queued {
                            word.to_string()
                        } else {
                            format!("{word}…")
                        })),
                )
                .when(!sending, |el| {
                    el.child(
                        div()
                            .relative()
                            .top(px(1.0))
                            .text_color(theme.text_faint)
                            .child(SharedString::from(format_elapsed(elapsed_secs))),
                    )
                })
                .into_any_element(),
        )
    }

    fn render_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.rows.get(ix).cloned() else {
            return gpui::Empty.into_any_element();
        };
        self.rendered_rows.insert(row.id.clone());
        let theme = Theme::of(cx).clone();
        let workspace_root = {
            let state = self.state.read(cx);
            self.chat_id
                .as_deref()
                .and_then(|chat_id| state.chats.iter().find(|chat| chat.id == chat_id))
                .or_else(|| state.selected_chat_row())
                .and_then(|chat| chat.cwd.as_deref())
                .map(SharedString::from)
        };
        // The viewport spans the full window (under the titlebar): the first
        // row's gap adds the titlebar's height so a top-scrolled transcript
        // rests below the chrome it fades under. The right pane already pads
        // for the titlebar — an override instance's first row keeps only the
        // ordinary turn gap, or the content sits double-chrome low.
        let top_gap = if ix == 0 {
            if self.doc_override.is_some() {
                Theme::SPACE_LG
            } else {
                Theme::TITLEBAR_HEIGHT + Theme::SPACE_LG + 10.0
            }
        } else {
            top_gap_for(ix.checked_sub(1).and_then(|i| self.rows.get(i)), &row)
        };
        // The last row must clear the composer/status stack the transcript
        // scrolls under PLUS the fade band above it, or the timestamp strip
        // (the row's lowest content) renders half-faded (or hidden) when the
        // transcript is pinned to the bottom.
        let is_last = ix + 1 == self.rows.len();
        let bottom_pad = if is_last {
            self.bottom_clearance + Theme::TRANSCRIPT_FADE_BAND + 8.0
        } else {
            0.0
        };
        // Live-run loader rides under the LAST row's content (above its
        // clearance pad), so it sits right beneath the working reply.
        let trailer = (ix + 1 == self.rows.len())
            .then(|| self.render_working_trailer(cx))
            .flatten();

        let inner: AnyElement = match &row.kind {
            RowKind::User {
                text,
                mentions,
                attachments,
                badges,
                pending,
            } => {
                let attachments = attachments.clone();
                let badges = badges.clone();
                let text = text.clone();
                let mentions = mentions.clone();
                let pending = *pending;
                // Attachment thumbnails ride ABOVE the bubble, right-aligned
                // (chat-view.tsx RowView: UserAttachmentStrip then the text
                // HStack); image-only sends show no bubble at all.
                let mut column = div().w_full().flex().flex_col();
                if !attachments.is_empty() {
                    column = column.child(self.render_user_attachments(
                        &row.id,
                        &attachments,
                        window,
                        cx,
                    ));
                }
                if !badges.is_empty() {
                    column = column.child(
                        div()
                            .w_full()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .justify_end()
                            .items_center()
                            .gap(px(6.0))
                            .pb(px(6.0))
                            .children(badges.iter().enumerate().map(|(bix, badge)| {
                                crate::badges::render(
                                    SharedString::from(format!("{}#badge{bix}", row.id)),
                                    badge,
                                    &theme,
                                )
                            })),
                    );
                }
                if !text.is_empty() {
                    // `min_w_0` is load-bearing: gpui text answers min/max-content
                    // probes with its UNWRAPPED width, so without it the bubble's
                    // automatic min-size is the full single-line width — the flex
                    // item can't shrink, `justify_end` pushes the overflow off the
                    // left edge, and long prompts render as one clipped line
                    // instead of wrapping inside the 80% column cap.
                    column = column.child(
                        div().w_full().flex().justify_end().child(
                            div()
                                .min_w_0()
                                .max_w(px(self.content_width * 0.8))
                                .bg(crate::theme::user_bubble_bg())
                                .rounded(px(Theme::BUBBLE_RADIUS))
                                .px(px(16.0))
                                .py(px(10.0))
                                .text_size(crate::typography::ui_rems(14.0))
                                .line_height(crate::typography::ui_rems(USER_LINE_HEIGHT))
                                .text_color(theme.text)
                                .when(pending, |el| el.opacity(0.65))
                                .child(self.render_user_body(
                                    &row.id, ix, text, mentions, &theme, window, cx,
                                )),
                        ),
                    );
                }
                column.into_any_element()
            }
            RowKind::Markdown { tree, block_ix } => {
                let Some(top) = tree.blocks.get(*block_ix) else {
                    return gpui::Empty.into_any_element();
                };
                let code = self.code_uis_for(&row.id, &top.block, *block_ix, cx);
                let opts = RenderOptions {
                    tasks: None,
                    media: None,
                    row_key: row.id.clone(),
                    veil: None,
                    cache: (!render_cache_disabled()).then(|| self.render_cache.clone()),
                    now: Instant::now(),
                    copy: Some(self.copy_ui_for(&row.id, cx)),
                    link: self.link_ui(),
                    workspace_root: workspace_root.clone(),
                    code,
                };
                let highlight = self.code_highlight_for(&row.id, tree, Some(*block_ix), cx);
                render::render_block(
                    &top.block,
                    *block_ix,
                    *block_ix,
                    &opts,
                    &theme,
                    window,
                    highlight
                        .get(block_ix)
                        .and_then(|o| o.as_deref())
                        .map(|document| document.lines.as_slice()),
                )
            }
            RowKind::LiveMarkdown { tree, block_ix } => {
                let Some(top) = tree.blocks.get(*block_ix) else {
                    return gpui::Empty.into_any_element();
                };
                let code = self.code_uis_for(&row.id, &top.block, *block_ix, cx);
                // Per-appended-chunk fade veil (opacity only — layout commits
                // instantly). Reduced motion renders with no veil at all.
                // Baseline rows (text already streamed when the transcript
                // attached) start seeded: the existing reply must not fade in
                // on a session switch — only fresh appends animate.
                let seed_history = !self.veils.contains_key(&row.id)
                    && self.historical_markdown.contains_key(&row.id);
                let veil = (!motion::reduced_motion(cx)).then(|| {
                    self.veils
                        .entry(row.id.clone())
                        .or_insert_with(|| {
                            if seed_history || self.veil_baseline.contains(&row.id) {
                                Rc::new(RefCell::new(RowVeil::seeded()))
                            } else {
                                Rc::default()
                            }
                        })
                        .clone()
                });
                let opts = RenderOptions {
                    tasks: None,
                    media: None,
                    row_key: row.id.clone(),
                    veil: veil.clone(),
                    cache: (!render_cache_disabled()).then(|| self.render_cache.clone()),
                    now: Instant::now(),
                    copy: Some(self.copy_ui_for(&row.id, cx)),
                    link: self.link_ui(),
                    workspace_root: workspace_root.clone(),
                    code,
                };
                if seed_history && let Some(veil) = &veil {
                    let historical = &self.historical_markdown[&row.id];
                    if let RowKind::LiveMarkdown { tree, block_ix } = &historical.kind
                        && let Some(top) = tree.blocks.get(*block_ix)
                    {
                        // Use the renderer's own nested element keys and text
                        // flattening, but never cache/paint the historical tree.
                        let seed_opts = RenderOptions {
                            cache: None,
                            link: None,
                            ..opts.clone()
                        };
                        let _ = render::render_block(
                            &top.block, *block_ix, *block_ix, &seed_opts, &theme, window, None,
                        );
                        veil.borrow_mut().finish_seeding();
                    }
                }
                let highlight = self.code_highlight_for(&row.id, tree, Some(*block_ix), cx);
                let timer = frame_stats_enabled().then(Instant::now);
                let el = render::render_block(
                    &top.block,
                    *block_ix,
                    *block_ix,
                    &opts,
                    &theme,
                    window,
                    highlight
                        .get(block_ix)
                        .and_then(|o| o.as_deref())
                        .map(|document| document.lines.as_slice()),
                );
                if let Some(start) = timer {
                    record_live_frame_us(start.elapsed().as_micros() as u64);
                }
                // The attach pass for this row is done (every element rendered
                // above seeded its baseline synchronously): elements appearing
                // from the NEXT pass on are newly streamed and fade normally.
                if let Some(veil) = &veil {
                    veil.borrow_mut().finish_seeding();
                }
                // Share the loaders' bounded clock. A display-frame callback
                // here would pin the transcript to 60/120Hz for the whole
                // stream, bypassing the clock even with no loader mounted.
                if veil.is_some_and(|v| v.borrow().is_fading()) {
                    motion::pulse_lease(cx.entity_id(), cx);
                }
                el
            }
            RowKind::ToolGroup {
                tools,
                auto_open,
                summary,
            } => self.render_tool_group(&row.id, tools, summary, *auto_open, &theme, cx),
            RowKind::InputChip { header, resolved } => {
                input_chip(header.clone(), *resolved, &theme)
            }
            RowKind::GeneratedImage {
                owner,
                path,
                name,
                mime_type,
            } => self.render_generated_image(&row.id, owner, path, name, mime_type, cx),
            RowKind::ErrorChip { message } => error_chip(message.clone(), &theme),
        };

        // Hover-revealed metadata strip: a RESERVED 32px lane under the
        // entry's last row. Timestamp, copy action, and copied feedback only
        // flip visibility/content, so none of them shifts the virtualizer.
        // User entries align end (under the bubble), assistant entries start.
        // Both read timestamp first, then the copy action.
        let is_user_row = matches!(row.kind, RowKind::User { .. });
        let hovered = self
            .hovered_entry
            .as_ref()
            .is_some_and(|(_, entry)| entry == &row.entry_id);
        let copied_message = self.copied_message.as_ref() == Some(&row.entry_id);
        let copy_text = row.copy_text.clone();
        let copy_entry_id = row.entry_id.clone();
        let strip = row.timestamp.map(|ms| {
            let timestamp = div()
                .text_size(crate::typography::ui_rems(12.0))
                .text_color(theme.text_muted.opacity(0.55))
                .child(SharedString::from(format_timestamp(ms, &chrono::Local)));
            let copy = copy_text.map(|text| {
                let entry_id = copy_entry_id.clone();
                let fade_key = format!("copy-message-hover-{entry_id}");
                div()
                    .id(SharedString::from(format!("copy-message-{entry_id}")))
                    .size(px(Theme::SPACE_MD * 2.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(Theme::CONTROL_RADIUS))
                    .cursor_pointer()
                    // Same quiet icon-button treatment as the copy action
                    // over transcript code blocks.
                    .bg(motion::hover_blend(
                        &fade_key,
                        gpui::transparent_black(),
                        crate::theme::ink(0.08),
                    ))
                    .on_hover(motion::hover_listener(fade_key))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.copy_message(entry_id.clone(), text.clone(), cx)
                    }))
                    .child(
                        crate::icons::icon(if copied_message {
                            crate::icons::CHECK
                        } else {
                            crate::icons::COPY
                        })
                        .size(px(14.0))
                        .text_color(theme.text_muted),
                    )
            });
            let metadata = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(Theme::SPACE_SM));
            let metadata = metadata.child(timestamp).children(copy);
            div()
                .h(px(Theme::SPACE_SM + Theme::SPACE_MD * 2.0))
                .pt(px(Theme::SPACE_SM))
                .w_full()
                .flex()
                .items_center()
                // No horizontal inset: the original's `px-1` netted out flush
                // because its message text was inset by the same amount (group
                // padding 4 + inner VStack 4 = 8 = group 4 + px-1 4). Here the
                // markdown text / user bubble sit AT the content column edges,
                // so the label must too — assistant label's left edge on the
                // text's first-character x, user label's right edge on the
                // bubble's right edge (user-reported 4px drift).
                .when(is_user_row, |el| el.justify_end())
                .when(hovered, |el| {
                    el.child(motion::fade_quick(
                        SharedString::from(format!("meta-{}", row.id)),
                        metadata,
                    ))
                })
        });
        let entry_id = row.entry_id.clone();
        let row_id = row.id.clone();
        div()
            .id(row.id.clone())
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    let next = Some((row_id.clone(), entry_id.clone()));
                    if this.hovered_entry != next {
                        let entry_changed = this
                            .hovered_entry
                            .as_ref()
                            .is_none_or(|(_, entry)| entry != &entry_id);
                        this.hovered_entry = next;
                        if entry_changed {
                            cx.notify();
                        }
                    }
                } else if this
                    .hovered_entry
                    .as_ref()
                    .is_some_and(|(row, _)| row == &row_id)
                {
                    // Only the row that OWNS the current reveal may clear it —
                    // a stale leave from an earlier row must not blank the
                    // strip the newly entered row just lit.
                    this.hovered_entry = None;
                    cx.notify();
                }
            }))
            .w_full()
            .flex()
            .justify_center()
            .pt(px(top_gap))
            .pb(px(bottom_pad))
            // Keep side gutters as the configurable column shrinks to fit.
            .px(px(48.0))
            .child(
                div()
                    .w_full()
                    .max_w(px(self.content_width))
                    .min_w_0()
                    .child(inner)
                    .children(strip)
                    .children(trailer),
            )
            .into_any_element()
    }

    fn copy_message(&mut self, entry_id: SharedString, text: SharedString, cx: &mut Context<Self>) {
        cx.stop_propagation();
        cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
        self.copied_message = Some(entry_id);
        self.copied_message_clear = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1200))
                .await;
            this.update(cx, |this, cx| {
                this.copied_message = None;
                this.copied_message_clear = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }



}

/// A sent message's text with its file-mention chips. The same recipe as the
/// markdown renderer's inline code (`flat_text_element`): chip ranges shape in
/// the mono font at the spectrum's `code_text`, [`StyledText`] supplies wrapped glyph
/// geometry through its layout handle, and a canvas paints the rounded
/// `code_wash` *beneath* the glyphs — so chips wrap, clip, and scroll exactly
/// like the text they decorate.
///
/// Per-frame cost while an assistant message streams below: shaping hits
/// gpui's line-layout cache (identical text + runs ⇒ reuse) and the underlay
/// repaints O(chips) quads — no layout work, no re-projection (spans were
/// computed once in [`rows_for_entry`]).
/// The user bubble's text: runs split at mention-chip boundaries (one plain
/// run when there are none), with the same selection machinery as rendered
/// markdown — the element registers into the frame's document-ordered
/// registry, so drags select, span into adjacent rows, and Cmd+C copies.
fn user_bubble_text(
    row_id: &SharedString,
    text: SharedString,
    mentions: Arc<Vec<crate::composer::SentMentionSpan>>,
    theme: &Theme,
    measured_h: Rc<Cell<f32>>,
    entity_id: gpui::EntityId,
) -> AnyElement {
    // Split runs at chip boundaries (spans are in order): body text keeps the
    // sans font, chips read as inline code. Size/line-height flow from the
    // bubble's div like every text child.
    let body_run = |len: usize| TextRun {
        len,
        font: gpui::font(theme.font_sans.clone()),
        color: theme.text,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let chip_run = |len: usize| TextRun {
        len,
        font: gpui::font(theme.font_mono.clone()),
        color: theme.code_text,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let mut runs = Vec::with_capacity(mentions.len() * 2 + 1);
    let mut at = 0;
    for span in mentions.iter() {
        if at < span.range.start {
            runs.push(body_run(span.range.start - at));
        }
        runs.push(chip_run(span.range.len()));
        at = span.range.end;
    }
    if at < text.len() {
        runs.push(body_run(text.len() - at));
    }
    let styled = StyledText::new(text.clone()).with_runs(runs);
    let layout = styled.layout().clone();
    let wash = theme.code_wash;
    let sel_key: std::sync::Arc<str> = format!("{row_id}:u").into();
    let sel_theme = theme.clone();
    let underlay = canvas(
        |_, _, _| (),
        move |_, _, window, cx| {
            for span in mentions.iter() {
                for rect in render::range_rects(&layout, &span.range, 0.0, 2.0) {
                    window.paint_quad(quad(
                        rect,
                        px(5.0),
                        wash,
                        px(0.0),
                        gpui::transparent_black(),
                        BorderStyle::default(),
                    ));
                }
            }
            render::paint_text_selection(window, &sel_key, &text, &layout, &sel_theme);
            // Passive geometry cache only: no entity update and no notify.
            // `bounds().height` can be the collapsed clip height, so derive
            // the full text height from the wrapped line layouts instead. The
            // click handler reads this exact value as the RESIZE endpoint,
            // while idle layout never feeds back into the transcript.
            let line_count: usize = layout
                .line_layouts()
                .iter()
                .map(|line| line.wrap_boundaries.len() + 1)
                .sum();
            let next_h = (line_count.max(1) as f32) * f32::from(layout.line_height());
            if (measured_h.get() - next_h).abs() > 0.5 {
                measured_h.set(next_h);
                // The first layout is the source of truth for soft wrapping.
                // Invalidate the transcript once so the expander and clip are
                // present even when glyph widths make a short-looking string
                // exceed five visual lines.
                cx.notify(entity_id);
            }
        },
    )
    .absolute()
    .size_full();
    div()
        .relative()
        .child(underlay)
        .child(styled)
        .into_any_element()
}

impl Render for Transcript {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if record_view_frame("transcript") {
            tracing::warn!(
                distance = self.distance_from_bottom(),
                spring = self.spring_should_run(),
                velocity = self.spring.velocity,
                target_velocity = self.spring.target_vel,
                own_turn = self.own_turn.is_some(),
                veils = self.veils.len(),
                "transcript motion state"
            );
        }
        self.render_cache
            .borrow_mut()
            .retain_rows(&self.rendered_rows);
        self.rendered_rows.clear();
        let code_fences_generation = crate::settings::code_fences_generation(cx);
        if self.code_fences_generation != code_fences_generation {
            self.code_fences_generation = code_fences_generation;
            // Horizontal positions are ephemeral. Reset every block owned by
            // this Transcript even when the toggle originated in another one.
            for runtime in self.code_fences.values() {
                runtime.scroll.set_offset(Point::default());
            }
            // Fit changes every code row from analytic to measured height (or
            // back), including virtual rows outside the current viewport.
            self.list.remeasure();
            if self.pinned {
                self.wake_spring();
            }
            if self.own_turn.is_some() {
                self.own_turn_kick = true;
            }
        }
        let content_width = crate::settings::transcript_width(cx);
        if self.content_width != content_width {
            self.content_width = content_width;
            // The outer list viewport may not resize when only max-width
            // changes. Invalidate virtual row heights explicitly, retaining
            // their anchors and all live animation/provenance state.
            self.list.remeasure();
            if self.pinned {
                self.wake_spring();
            }
            if self.own_turn.is_some() {
                self.own_turn_kick = true;
            }
        }
        let typography_generation = crate::typography::generation(cx);
        if self.typography_generation != typography_generation {
            self.typography_generation = typography_generation;
            // `refresh_windows` re-lays out visible rows, but ListState keeps
            // measured heights for virtualized rows outside the viewport.
            // Mark every row unmeasured while retaining height hints and a
            // proportional scroll anchor; GPUI will refresh each measurement
            // as the row enters its layout range.
            self.list.remeasure();
        }
        // Release gpui-side decoded copies of any images the attachment LRU
        // evicted since the last frame (no-op when nothing was evicted).
        crate::attachments::flush_evicted(Some(window), cx);
        // Own-turn driver: measurements are only authoritative after layout,
        // so reservation sizing, the send glide, and the outgrown-handoff
        // each advance at most once per requested frame. Scheduled on every
        // frame while an anchor is live (not just on kicks) so viewport
        // resizes and streaming growth re-derive the reservation; the step
        // only notifies on change, so a settled hold schedules no next frame.
        if !self.route_exit_pending(cx)
            && (self.own_turn.is_some() || self.own_turn_kick)
            && !self.own_turn_scheduled
        {
            self.own_turn_scheduled = true;
            let entity = cx.weak_entity();
            window.on_next_frame(move |_, cx| {
                entity
                    .update(cx, |this: &mut Transcript, cx| {
                        this.own_turn_scheduled = false;
                        this.step_own_turn(cx);
                    })
                    .ok();
            });
        }
        // Spring driver: one on_next_frame callback at a time; each tick
        // notifies, which re-enters render and schedules the next frame until
        // the spring parks. Reduced motion never schedules (sync snaps).
        if !self.route_exit_pending(cx)
            && self.pinned
            && !motion::reduced_motion(cx)
            && !self.spring_scheduled
            && self.spring_should_run()
        {
            self.spring_scheduled = true;
            let entity = cx.weak_entity();
            window.on_next_frame(move |_, cx| {
                entity
                    .update(cx, |this: &mut Transcript, cx| {
                        this.spring_scheduled = false;
                        this.step_spring(cx);
                    })
                    .ok();
            });
        }
        // Programmatic `scroll_to` does not invoke the list's user-scroll
        // handler. Refresh distance-derived state once layout has measured the
        // replay, guarded so a stale A callback cannot mutate B (or a newer A).
        if self.viewport_finalize_pending && !self.viewport_finalize_scheduled {
            self.viewport_finalize_scheduled = true;
            let token = ViewportFinalizeToken {
                generation: self.viewport_generation,
                layout_revision: self.viewport_layout_revision,
            };
            let entity = cx.weak_entity();
            window.on_next_frame(move |_, cx| {
                entity
                    .update(cx, |this: &mut Transcript, cx| {
                        this.viewport_finalize_scheduled = false;
                        if !token.still_current(this.viewport_generation) {
                            if this.viewport_finalize_pending {
                                cx.notify();
                            }
                            return;
                        }
                        let distance = this.distance_from_bottom();
                        this.last_scroll_distance = distance;
                        this.show_jump_button = jump_visibility(this.show_jump_button, distance)
                            && !this.pinned
                            && !this.own_turn.as_ref().is_some_and(|turn| turn.held);
                        if token.layout_settled(this.viewport_layout_revision) {
                            this.viewport_finalize_pending = false;
                        }
                        cx.notify();
                    })
                    .ok();
            });
        }
        // A long-message collapse near the bottom owns the viewport for the
        // duration of its height tween. Advance the matching upward scroll once
        // per frame so the bubble stays visible instead of shrinking above the
        // fixed viewport while the bottom content remains on screen.
        if self.user_collapse_scroll.is_some() && !self.user_collapse_scroll_scheduled {
            self.user_collapse_scroll_scheduled = true;
            let entity = cx.weak_entity();
            window.on_next_frame(move |_, cx| {
                entity
                    .update(cx, |this: &mut Transcript, cx| {
                        this.user_collapse_scroll_scheduled = false;
                        this.step_user_collapse_scroll(cx);
                    })
                    .ok();
            });
        }
        let rail = self.render_rail(cx);
        // The scroll-to-bottom pill is rendered by the SHELL (conversation
        // region overlay): it must float just above the composer and paint
        // OVER the bottom fade gradient, which is a later sibling of this
        // outlet — an overlay here would be tinted by the fade.
        self.update_runway_minimum(cx);
        let list_el = list(self.list.clone(), cx.processor(Self::render_row))
            .size_full()
            .with_sizing_behavior(gpui::ListSizingBehavior::Auto);
        let content: AnyElement = if self.doc_override.is_some() {
            // The primary transcript's fade lives on the SHELL's outlet
            // wrapper (it spans the titlebar/composer chrome); an override
            // instance owns its own — top edge only (nothing overlays the
            // pane's bottom), gated on real overflow so a short top-anchored
            // transcript shows no fade. Gated here rather than at paint via
            // a ScrollHandle (the list isn't one); scrolls re-render this
            // entity, so the flag can't go stale.
            let scrolled_under_top = {
                let max = f32::from(self.list.max_offset_for_scrollbar().y);
                max - self.distance_from_bottom() > 1.0
            };
            crate::edge_fade::edge_faded(
                Theme::TRANSCRIPT_FADE_BAND,
                scrolled_under_top,
                false,
                list_el,
            )
            .into_any_element()
        } else {
            list_el.into_any_element()
        };
        let root = div()
            .relative()
            .size_full()
            .min_h_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(Self::on_selection_mouse_down),
            )
            .on_mouse_move(cx.listener(Self::on_selection_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_selection_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_selection_mouse_up))
            // FIRST child ⇒ paints first: clears the frame's markdown text-
            // selection registry before any row's text elements re-register
            // (document paint order = selection order; see markdown/render.rs).
            .child(crate::markdown::render::selection_frame_reset())
            .child(content)
            .child(rail);
        // Full-size viewer for a clicked user-bubble thumbnail
        // (AttachmentPreviewDialog: bare lightbox, click closes).
        if let Some(preview) = self.attachment_preview.clone() {
            let weak = cx.weak_entity();
            return root.child(crate::attachments::lightbox(
                window,
                &preview,
                &self.attachment_preview_focus,
                move |window, cx| {
                    if let Ok(focus) = weak.update(cx, |this, cx| {
                        this.attachment_preview = None;
                        cx.notify();
                        this.attachment_preview_return_focus.take()
                    }) && let Some(focus) = focus
                    {
                        window.focus(&focus, cx);
                    }
                },
                cx,
            ));
        }
        root
    }
}

#[cfg(test)]
mod tests;


#[cfg(feature = "appshots-fixture")]
impl Transcript {
    pub fn fixture_appshots_start(&mut self, cx: &mut Context<Self>) {
        self.list.scroll_to(gpui::ListOffset::default());
        cx.notify();
    }
}
