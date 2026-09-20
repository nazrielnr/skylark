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
    div, list, point, prelude::*, px, AnyElement, Bounds, Context, Entity, ListAlignment,
    ListOffset, ListScrollEvent, ListState, MouseButton, PathBuilder, Pixels, Point, SharedString,
    Subscription, Task, Window,
};

#[allow(unused_imports)]
pub(crate) use zeron_doc::{
    MessagePart, MessageRole, MessageStatus, SessionMessageEntry, SubagentStatus,
};
use zeron_proto::ToolCall;

use crate::markdown::parser::{
    Block, BlockTree, IncrementalParser, InlineRun, InlineStyle, parse_full,
};
use crate::markdown::render::{self, RenderCache};
use crate::markdown::veil::RowVeil;
use crate::motion;
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
pub(crate) mod render_rows;
pub(crate) mod sync;
pub(crate) mod display;

// ---------------------------------------------------------------------------
// Transcript entity
// ---------------------------------------------------------------------------

pub struct Transcript {
    pub(super) state: Entity<AppState>,
    pub(super) list: ListState,
    pub(super) rows: Vec<Row>,
    pub(super) last_source: Option<(Option<String>, TranscriptReplayState, u64)>,
    pub(crate) chat_id: Option<String>,
    /// The shell may retain this already-laid-out view briefly for its exit.
    /// Cleared as soon as the exit is invisible; never used for another chat.
    pub(super) retain_on_deselect: bool,
    /// `Some(doc_id)` pins this instance to a SUBAGENT doc: rows come from
    /// `AppState::sub_transcript(doc_id)` instead of the selected chat, and
    /// the instance is READ-ONLY — no echoes, no own-turn hold, and no global
    /// attachment protection (that set is shared with the primary transcript
    /// and overwritten wholesale).
    pub(super) doc_override: Option<String>,
    /// Whether an override instance watches a LIVE doc (`for_doc(follow)`):
    /// only then may the working trailer render — a frozen snapshot must
    /// never spin, whatever its entries claim.
    pub(super) doc_live: bool,
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
    pub(super) land_end_pending: bool,
    pub(super) row_cache: HashMap<String, CachedRows>,
    pub(super) live_parsers: HashMap<String, IncrementalParser>,
    pub(super) tree_cache: HashMap<String, (usize, Arc<BlockTree>)>,
    pub(crate) folds: HashMap<SharedString, FoldState>,
    /// Entrance state follows stable groups through completion so fast calls
    /// finish revealing. Replay rows have no entrance timestamps.
    pub(crate) tool_group_reveals: HashMap<SharedString, ToolGroupReveal>,
    pub(super) last_replay_baseline: Option<Arc<zeron_doc::TranscriptBaseline>>,
    /// Parsed historical prefixes, used to seed text before a coalesced live
    /// suffix is painted. The wire watermark contains lengths, not text.
    pub(super) historical_markdown: HashMap<SharedString, Row>,
    /// Detail folds (output/diff) per chip, keyed `"{row_id}#d{ix}"` — full
    /// [`FoldState`]s so detail bodies tween open/closed exactly like the
    /// group fold. Render-local like `folds` — never part of the row
    /// fingerprint.
    pub(crate) tool_details: HashMap<SharedString, FoldState>,
    /// Expand/collapse state for user bubbles past [`USER_COLLAPSED_LINES`],
    /// keyed by row id. Render-local like `folds` — never part of the row
    /// fingerprint, so toggling one costs a repaint, not a rebuild.
    pub(super) user_folds: HashMap<SharedString, FoldState>,
    /// Full laid-out text heights for long user bubbles. The text's paint
    /// canvas writes these cells without notifying or mutating the transcript;
    /// click handlers read them as exact endpoints for the RESIZE tween. This
    /// preserves smooth layout motion without a paint → notify feedback loop.
    pub(super) user_heights: HashMap<SharedString, Rc<Cell<f32>>>,
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
    pub(super) veils: HashMap<SharedString, Rc<RefCell<RowVeil>>>,
    /// Live rows present in the transcript's REPLAY after (re)attaching to a
    /// chat: their veils are created pre-seeded, so text that was already
    /// streamed before the switch never fades in — only appends after it do
    /// (mugen's `FadePainter.attach` baseline; user report: switching back to
    /// a streaming session dissolved the entire reply).
    pub(super) veil_baseline: std::collections::HashSet<SharedString>,
    /// Armed at attach, disarmed on the first sync whose transcript is
    /// non-empty: the baseline must be captured from the doc REPLAY frame,
    /// not the attach-time sync — selection clears the transcript and the
    /// replay lands async, so capturing at attach seeded nothing and the
    /// still-streaming reply faded in whole on every session switch (user
    /// report, round 2).
    pub(super) veil_attach_pending: bool,
    /// Cross-frame flatten/shape-input cache (see [`RenderCache`]): fade
    /// frames reuse settled blocks' text+runs; the incremental parser's stable
    /// boundary invalidates only the live tail per commit.
    pub(super) render_cache: Rc<RefCell<RenderCache>>,
    workspace_link: Option<render::LinkUi>,
    pub(super) rendered_rows: HashSet<SharedString>,
    /// Last UI typography generation reflected in `list` item measurements.
    /// Family and size changes can alter prose wrapping without changing row
    /// identity, so the virtual list must explicitly discard cached heights.
    pub(super) typography_generation: u32,
    pub(super) content_width: f32,
    /// Last global code-fence layout generation applied to this transcript.
    /// Each instance owns separate scroll handles and list measurements, so
    /// every one must reset itself after a global Fit-mode transition.
    pub(super) code_fences_generation: u64,
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
    pub(super) bottom_clearance: f32,
    /// Hovered rail tick (grows + shows the preview card).
    rail_hover: Option<usize>,
    /// `(row id, entry id)` under the pointer — reveals the entry's timestamp
    /// strip (zeron chat-view.tsx `group-hover`; the rows report hover
    /// themselves). Keyed by ROW so a row→row move within one entry can't
    /// clear the reveal when the old row's leave event arrives after the new
    /// row's enter (enter/leave order across rows is not guaranteed).
    pub(super) hovered_entry: Option<(SharedString, SharedString)>,
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
    pub(super) copied_message: Option<SharedString>,
    pub(super) copied_message_clear: Option<Task<()>>,
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
