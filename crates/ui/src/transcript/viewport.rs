//! Viewport anchoring, stick-to-bottom pinning, and scroll spring orchestration.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use gpui::{px, Context, ListOffset, ListScrollEvent, Pixels, SharedString, Task};

use crate::motion;

use super::row::Row;
use super::stick_spring::{
    self, jump_visibility, StickSpring, AT_BOTTOM_PX, GLIDE_MAX_VIEWPORTS,
    MAX_PENDING_QUEUED_TURNS, MAX_SAVED_VIEWPORTS, OWN_SEND_SCROLL_SLACK_PX,
    SCROLL_BUTTON_THRESHOLD_PX, SPRING_FRAME_MS, SPRING_MAX_CATCHUP_FRAMES, SPRING_SETTLE_GRACE_MS,
};
use super::Transcript;

/// A locally-sent turn reserves the viewport below its prompt. The last row
/// has a minimum height, so streaming content and the working trailer consume
/// or release space in the same layout pass. Only changes to the preceding
/// rows require a post-layout refinement. Wheel input releases the automatic
/// glide/hold while preserving the reservation; overflow hands off to the
/// ordinary bottom spring. Chat switches restore the reservation released.
#[derive(Clone, Debug)]
pub(crate) struct OwnTurnAnchor {
    pub(crate) chat_id: String,
    pub(crate) message_id: SharedString,
    /// The step still owns the viewport (glide → hold). Any wheel/touch
    /// input releases it — the reservation stays behind as plain scrollable
    /// space, and the ordinary escape/restick rules apply from then on.
    pub(crate) held: bool,
    /// The entry glide has landed; the hold now re-asserts the prompt's
    /// position absolutely after every layout (glue- and lag-proof — the
    /// exact mechanism the shipped first-send anchor used).
    pub(crate) positioned: bool,
    /// A fresh send may install the anchor one notification before its echo.
    /// Once the prompt has appeared, its later disappearance is terminal
    /// (failed echo or removed entry) and the runway must retire.
    pub(crate) seen_prompt: bool,
}

impl OwnTurnAnchor {
    pub(crate) fn released_for_restore(mut self) -> Self {
        self.held = false;
        self.positioned = false;
        self.seen_prompt = true;
        self
    }

    pub(crate) fn observe_prompt(&mut self, exists: bool) -> bool {
        if exists {
            self.seen_prompt = true;
        }
        exists || !self.seen_prompt
    }
}

/// Locally-authored queue rows whose stable ids have not appeared in the
/// transcript yet. Registration is deliberately inert: adding a queue row
/// must leave the currently-visible turn and its runway untouched. Once a
/// matching prompt materializes, the newest match becomes the own-turn anchor.
#[derive(Default)]
pub(crate) struct PendingQueuedTurns {
    pub(crate) items: VecDeque<(String, SharedString)>,
}

impl PendingQueuedTurns {
    pub(crate) fn register(&mut self, chat_id: String, message_id: String) {
        self.items
            .retain(|(chat, id)| chat != &chat_id || id.as_ref() != message_id);
        self.items
            .push_back((chat_id, SharedString::from(message_id)));
        while self.items.len() > MAX_PENDING_QUEUED_TURNS {
            self.items.pop_front();
        }
    }

    /// Consume every candidate from this chat that is now present and return
    /// the newest one. Multiple rows can land in one doc frame; the last send
    /// owns the runway, matching consecutive immediate sends.
    pub(crate) fn take_latest_materialized(&mut self, chat_id: &str, rows: &[Row]) -> Option<String> {
        let mut latest = None;
        self.items.retain(|(chat, message_id)| {
            let materialized = chat == chat_id
                && rows
                    .iter()
                    .any(|row| row.turn_start && row.entry_id == message_id.as_ref());
            if materialized {
                latest = Some(message_id.to_string());
            }
            !materialized
        });
        latest
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }
}

/// A stable per-chat viewport anchor. Row identity is preferred over its old
/// index because async replay can insert or remove rows while a chat is away.
#[derive(Clone, Debug)]
pub(crate) struct ViewportAnchor {
    pub(crate) row_id: SharedString,
    pub(crate) entry_id: SharedString,
    pub(crate) fallback_ix: usize,
    pub(crate) offset_in_row: Pixels,
}

impl ViewportAnchor {
    pub(crate) fn capture(rows: &[Row], scroll_top: ListOffset) -> Option<Self> {
        let fallback_ix = scroll_top.item_ix.min(rows.len().checked_sub(1)?);
        let row = &rows[fallback_ix];
        Some(Self {
            row_id: row.id.clone(),
            entry_id: row.entry_id.clone(),
            fallback_ix,
            offset_in_row: scroll_top.offset_in_item,
        })
    }

    pub(crate) fn resolve_exact(&self, rows: &[Row]) -> Option<ListOffset> {
        let item_ix = rows.iter().position(|row| row.id == self.row_id)?;
        Some(ListOffset {
            item_ix,
            offset_in_item: self.offset_in_row,
        })
    }

    pub(crate) fn resolve(&self, rows: &[Row]) -> Option<ListOffset> {
        if let Some(offset) = self.resolve_exact(rows) {
            return Some(offset);
        }

        // A row can disappear when a streaming block is reshaped. Stay in the
        // same message entry, choosing the surviving row nearest the old
        // location; the intra-row offset is no longer meaningful in that case.
        let item_ix = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.entry_id == self.entry_id)
            .min_by_key(|(ix, _)| ix.abs_diff(self.fallback_ix))
            .map(|(ix, _)| ix)
            .unwrap_or_else(|| self.fallback_ix.min(rows.len().saturating_sub(1)));
        (!rows.is_empty()).then_some(ListOffset {
            item_ix,
            offset_in_item: px(0.0),
        })
    }
}

/// Session-local viewport state. Chats that were following their tail keep
/// following it; only user-owned viewports restore a concrete row anchor.
#[derive(Clone, Debug)]
pub(crate) enum SavedViewport {
    FollowTail,
    Anchored {
        anchor: ViewportAnchor,
        distance_from_bottom: f32,
        /// Preserve the runway that made a short active turn scrollable.
        /// Navigation releases its automatic hold, so revisiting restores the
        /// viewport without immediately following new output to the bottom.
        own_turn: Option<OwnTurnAnchor>,
    },
}

pub(crate) struct RestoredViewport {
    pub(crate) offset: ListOffset,
    pub(crate) distance_from_bottom: f32,
    pub(crate) own_turn: Option<OwnTurnAnchor>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ViewportFinalizeToken {
    pub(crate) generation: u64,
    pub(crate) layout_revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TranscriptReplayState {
    Pending,
    Empty,
    Populated,
}

impl TranscriptReplayState {
    pub(crate) fn authoritative_empty(self) -> bool {
        self == Self::Empty
    }

    pub(crate) fn allows_fallback(self) -> bool {
        self == Self::Populated
    }
}

impl ViewportFinalizeToken {
    pub(crate) fn still_current(self, generation: u64) -> bool {
        self.generation == generation
    }

    pub(crate) fn layout_settled(self, layout_revision: u64) -> bool {
        self.layout_revision == layout_revision
    }
}

impl SavedViewport {
    pub(crate) fn capture(
        rows: &[Row],
        scroll_top: ListOffset,
        pinned: bool,
        distance_from_bottom: f32,
        own_turn: Option<&OwnTurnAnchor>,
    ) -> Option<Self> {
        if rows.is_empty() {
            return None;
        }
        if pinned {
            return Some(Self::FollowTail);
        }
        Some(Self::Anchored {
            anchor: ViewportAnchor::capture(rows, scroll_top)?,
            distance_from_bottom,
            own_turn: own_turn.cloned(),
        })
    }

    /// Before the opening reset arrives, rows may contain only optimistic
    /// echoes. In that gap an exact row is safe, but entry/index fallbacks
    /// would mistake an unrelated echo for the authoritative transcript.
    pub(crate) fn resolve(&self, rows: &[Row], allow_fallback: bool) -> Option<RestoredViewport> {
        let Self::Anchored {
            anchor,
            distance_from_bottom,
            own_turn,
        } = self
        else {
            return None;
        };
        let offset = if allow_fallback {
            anchor.resolve(rows)?
        } else {
            anchor.resolve_exact(rows)?
        };
        let own_turn = own_turn
            .clone()
            .filter(|turn| {
                rows.iter()
                    .any(|row| row.turn_start && row.entry_id == turn.message_id)
            })
            .map(OwnTurnAnchor::released_for_restore);
        Some(RestoredViewport {
            offset,
            distance_from_bottom: *distance_from_bottom,
            own_turn,
        })
    }
}

#[derive(Default)]
pub(crate) struct SavedViewportCache {
    pub(crate) by_chat: HashMap<String, SavedViewport>,
    pub(crate) recency: VecDeque<String>,
}

impl SavedViewportCache {
    pub(crate) fn insert(&mut self, chat_id: String, viewport: SavedViewport) {
        if self.by_chat.contains_key(&chat_id) {
            self.recency.retain(|candidate| candidate != &chat_id);
        }
        self.recency.push_back(chat_id.clone());
        self.by_chat.insert(chat_id, viewport);
        while self.by_chat.len() > MAX_SAVED_VIEWPORTS {
            let Some(evicted) = self.recency.pop_front() else {
                break;
            };
            self.by_chat.remove(&evicted);
        }
    }

    pub(crate) fn get_cloned_and_touch(&mut self, chat_id: &str) -> Option<SavedViewport> {
        let viewport = self.by_chat.get(chat_id).cloned()?;
        self.recency.retain(|candidate| candidate != chat_id);
        self.recency.push_back(chat_id.to_string());
        Some(viewport)
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.by_chat.len()
    }
}

impl Transcript {
    /// Snapshot the outgoing primary chat before its rows and ListState are
    /// reset. Empty rows never overwrite an older snapshot: during a rapid
    /// A→B→A switch, B's replay may not have arrived before leaving it again.
    pub(crate) fn remember_current_viewport(&mut self) {
        // Rows can already contain optimistic echoes while an older snapshot
        // is still waiting for the authoritative replay. Leaving again in
        // that window must preserve the older snapshot, not replace it with
        // the partial echo-only viewport.
        if self.pending_viewport.is_some() {
            return;
        }
        let Some(chat_id) = self.chat_id.clone() else {
            return;
        };
        let distance_from_bottom = if self.pinned {
            0.0
        } else {
            self.distance_from_bottom()
        };
        let Some(viewport) = SavedViewport::capture(
            &self.rows,
            self.list.logical_scroll_top(),
            self.pinned,
            distance_from_bottom,
            self.own_turn.as_ref(),
        ) else {
            return;
        };
        self.saved_viewports.insert(chat_id, viewport);
    }

    /// Restore an exact optimistic row while replay is pending, enable stable
    /// fallbacks only after a populated reset, and retire snapshots proven
    /// absent by an empty reset. `scroll_to` remains valid while the virtual
    /// list measures restored rows on the following layout pass.
    pub(crate) fn restore_pending_viewport(&mut self, replay: TranscriptReplayState) -> bool {
        if self.pending_viewport.is_none() {
            return false;
        }
        if !self.rows.is_empty()
            && let Some(restored) = self
                .pending_viewport
                .as_ref()
                .and_then(|saved| saved.resolve(&self.rows, replay.allows_fallback()))
        {
            self.pending_viewport = None;
            self.list.scroll_to(restored.offset);
            self.own_turn = restored.own_turn;
            self.own_turn_kick = self.own_turn.is_some();
            self.own_turn_last_tick = None;
            if self.own_turn.is_some() {
                // Replay readiness can change while echo rows stay identical,
                // so the no-diff path may install a runway without splicing.
                self.remeasure_last_row();
            }
            self.last_scroll_distance = restored.distance_from_bottom;
            self.show_jump_button = restored.distance_from_bottom > SCROLL_BUTTON_THRESHOLD_PX;
            self.viewport_finalize_pending = true;
            return true;
        }

        if !replay.authoritative_empty() {
            return false;
        }
        // The reset's document rows, not the combined rows, define
        // authoritative emptiness. A matching optimistic row above remains
        // valid, but an unrelated echo must never become an index fallback
        // for old history.
        self.discard_pending_viewport();
        if self.own_turn.is_none() {
            self.pinned = true;
            self.last_scroll_distance = 0.0;
            self.show_jump_button = false;
            self.list.scroll_to_end();
        }
        true
    }

    /// Explicit user/navigation intent supersedes a replay-delayed restore.
    /// Replace its cache entry with tail-follow until current rows can be
    /// snapshotted normally on the next chat switch.
    pub(crate) fn discard_pending_viewport(&mut self) {
        if self.pending_viewport.take().is_some()
            && let Some(chat_id) = self.chat_id.clone()
        {
            self.saved_viewports
                .insert(chat_id, SavedViewport::FollowTail);
        }
    }

    /// Hand viewport ownership to explicit rail/navigation input before its
    /// reduced-motion or animated branch moves the list.
    pub(crate) fn begin_scroll_navigation(&mut self) {
        self.discard_pending_viewport();
        self.cancel_user_hold();
        self.user_collapse_scroll = None;
        self.stop_automatic_scrolling();
    }

    pub(crate) fn stop_automatic_scrolling(&mut self) {
        // Navigation and selection within the session release the hold but keep the
        // runway (user spec: only leaving and revisiting the session clears
        // it) — scrolling back down re-arms the hold like any restick.
        self.release_own_turn_hold();
        self.pinned = false;
        self.spring.reset();
        self.spring_last_tick = None;
        self.spring_settled_at = None;
        self.spring_kick = false;
        self.scroll_anim = None;
    }

    /// Store the animation after [`Self::begin_scroll_navigation`].
    pub(crate) fn set_scroll_task(&mut self, task: Task<()>) {
        self.scroll_anim = Some(task);
    }

    /// Give the viewport to the user/navigation without dropping the
    /// reservation: the pad stays, the hold stands down until a restick.
    pub(crate) fn release_own_turn_hold(&mut self) {
        if let Some(anchor) = self.own_turn.as_mut() {
            anchor.held = false;
        }
        self.own_turn_last_tick = None;
    }

    pub(crate) fn remeasure_last_row(&mut self) {
        if let Some(last) = self.rows.len().checked_sub(1) {
            self.list.remeasure_items(last..last + 1);
            self.viewport_layout_revision = self.viewport_layout_revision.wrapping_add(1);
        }
    }

    pub(crate) fn distance_from_bottom(&self) -> f32 {
        let max = f32::from(self.list.max_offset_for_scrollbar().y);
        let cur = f32::from(self.list.scroll_px_offset_for_scrollbar().y);
        (max + cur).max(0.0)
    }

    /// Whether a user scroll should re-engage the bottom pin: inside the 70px
    /// stick band *and* moving toward the bottom. Direction matters — a small
    /// wheel-up notch near the bottom stays inside the band, and re-sticking
    /// on it would snap the view straight back, making the pin unbreakable.
    pub fn should_restick(distance: f32, previous_distance: f32) -> bool {
        stick_spring::should_restick(distance, previous_distance)
    }

    pub(crate) fn handle_scroll(&mut self, _event: &ListScrollEvent, cx: &mut Context<Self>) {
        // Cancel synchronously, before a queued animation frame can undo the
        // wheel/touch input. Neither operation reads the borrowed ListState.
        self.user_collapse_scroll = None;
        self.cancel_user_hold();
        let released_own_turn = self.own_turn.as_ref().is_some_and(|anchor| anchor.held);
        self.release_own_turn_hold();
        if self.own_turn.is_some() {
            // Cancel any tail spring synchronously too; the deferred input
            // decision below may re-engage it after reading the new offset.
            self.pinned = false;
            self.spring.reset();
            self.spring_last_tick = None;
        }
        // The list invokes this handler ONLY from its wheel/touch input path
        // (programmatic scroll_by/scroll_to never re-enter it), while holding
        // its internal RefCell borrow — reading the ListState back
        // synchronously panics with "already mutably borrowed". Defer to the
        // end of the effect cycle, after the list has released its borrow.
        let this = cx.weak_entity();
        cx.defer(move |cx| {
            this.update(cx, |this: &mut Transcript, cx| {
                this.discard_pending_viewport();
                // Input owns the viewport immediately, including wheel-down
                // after background streaming. A held turn can be stale while
                // frame callbacks are paused; reasserting its old prompt here
                // made scrolling down impossible until an upward gesture.
                if this.own_turn.is_some() {
                    let distance = this.distance_from_bottom();
                    let previous = this.last_scroll_distance;
                    this.last_scroll_distance = distance;
                    // Reaching the end preserves normal tail-follow intent
                    // without reasserting a possibly stale prompt hold.
                    this.pinned =
                        distance <= AT_BOTTOM_PX || Self::should_restick(distance, previous);
                    this.spring.reset();
                    this.spring_last_tick = None;
                    // Re-stick only when returning to a short turn's actual
                    // hold. An off-screen prompt belongs to an overflowing
                    // reply, even if reservation refinement hasn't run yet.
                    let at_hold = this.own_turn_anchor_ix().is_some_and(|ix| {
                        this.list.bounds_for_item(ix).is_some_and(|bounds| {
                            f32::from(bounds.top() - this.list.viewport_bounds().top())
                                >= Self::own_send_inset(ix) - OWN_SEND_SCROLL_SLACK_PX - 2.0
                        })
                    });
                    if !released_own_turn && at_hold && Self::should_restick(distance, previous) {
                        if let Some(anchor) = this.own_turn.as_mut() {
                            anchor.held = true;
                            anchor.positioned = false;
                        }
                        this.pinned = false;
                        this.own_turn_kick = true;
                    }
                    if this.pinned {
                        this.wake_spring();
                    }
                    this.show_jump_button = jump_visibility(this.show_jump_button, distance)
                        && !this.own_turn.as_ref().is_some_and(|a| a.held);
                    cx.notify();
                    return;
                }
                let distance = this.distance_from_bottom();
                let previous = this.last_scroll_distance;
                this.last_scroll_distance = distance;
                if distance > previous + 1.0 && distance > AT_BOTTOM_PX {
                    // User input moving away from the bottom breaks the pin.
                    // Content growth never lands here — it doesn't fire the
                    // scroll handler (mugen §1e: interrupt from input, not
                    // scrollbar position).
                    this.pinned = false;
                    this.spring.reset();
                    this.spring_last_tick = None;
                } else if distance <= AT_BOTTOM_PX || Self::should_restick(distance, previous) {
                    // Returning toward the bottom inside the 70px band (or
                    // arriving at it) re-engages the pin with a glide.
                    if !this.pinned {
                        this.pinned = true;
                        this.wake_spring();
                    }
                }
                let show = jump_visibility(this.show_jump_button, distance) && !this.pinned;
                if show != this.show_jump_button {
                    this.show_jump_button = show;
                }
                cx.notify();
            })
            .ok();
        });
    }

    /// Whether the transcript is currently pinned to the bottom.
    pub fn is_pinned(&self) -> bool {
        self.pinned
    }

    /// Whether the shell should float the "Scroll to bottom" pill (scrolled
    /// more than [`SCROLL_BUTTON_THRESHOLD_PX`] off the end, unpinned).
    pub fn jump_button_shown(&self) -> bool {
        self.show_jump_button
    }

    /// The scroll-to-bottom pill's click: glide back to the end and re-pin.
    pub fn jump_to_bottom(&mut self, cx: &mut Context<Self>) {
        self.user_collapse_scroll = None;
        self.cancel_user_hold();
        self.discard_pending_viewport();
        // An expanded prompt can be taller than the viewport. Its reservation
        // is retained, but jumping should reveal the reply below that prompt.
        if self.own_turn_anchor_ix().is_some_and(|ix| {
            self.user_folds
                .get(&self.rows[ix].id)
                .is_some_and(|fold| fold.open == Some(true))
        }) {
            self.release_own_turn_hold();
            self.engage_pin(cx);
            return;
        }
        // With a live runway, "bottom" IS the held position (the reservation
        // makes prompt-at-top and pad-bottom the same place): re-arm the hold
        // and glide back instead of destroying the runway (user spec — only
        // navigating away and back clears it).
        if let Some(anchor) = self.own_turn.as_mut() {
            anchor.held = true;
            anchor.positioned = false;
            self.own_turn_last_tick = None;
            self.own_turn_kick = true;
            self.show_jump_button = false;
            cx.notify();
            return;
        }
        self.engage_pin(cx);
    }

    /// Re-engage the bottom pin with a glide. Long jumps teleport to within
    /// [`GLIDE_MAX_VIEWPORTS`] of the end first (mugen `springToBottom`);
    /// reduced motion snaps.
    pub(crate) fn engage_pin(&mut self, cx: &mut Context<Self>) {
        self.pinned = true;
        self.show_jump_button = false;
        if motion::reduced_motion(cx) {
            self.list.scroll_to_end();
            cx.notify();
            return;
        }
        let viewport = f32::from(self.list.viewport_bounds().size.height);
        let distance = self.distance_from_bottom();
        let glide_max = GLIDE_MAX_VIEWPORTS * viewport;
        if viewport > 0.0 && distance > glide_max {
            self.list.scroll_by(px(distance - glide_max));
        }
        self.wake_spring();
        cx.notify();
    }

    /// Arm the per-frame spring driver — `render` schedules the next frame
    /// while [`Self::spring_should_run`].
    pub(crate) fn wake_spring(&mut self) {
        if self.spring_settled_at.is_some_and(|settled| {
            settled.elapsed() >= Duration::from_millis(SPRING_SETTLE_GRACE_MS)
        }) {
            self.spring.reset();
            self.spring_last_tick = None;
        }
        self.spring_settled_at = None;
        self.spring_kick = true;
    }

    /// A layout kick needs one observation; otherwise only unfinished motion
    /// needs another frame. The settle grace retains state without repainting.
    pub(crate) fn spring_should_run(&self) -> bool {
        self.spring_kick || StickSpring::needs_frame(self.distance_from_bottom())
    }

    /// Whether the scroll offset is in a bottom-glued representation (`None`
    /// or anchored past the end) — states where the next layout hard-snaps to
    /// the new end instead of holding a pixel position.
    pub(crate) fn is_glued(&self) -> bool {
        self.list.logical_scroll_top().item_ix >= self.rows.len()
    }

    /// One spring frame: observe target growth, step the stepper, apply the
    /// delta, and park on landing. Runs from `window.on_next_frame`,
    /// i.e. after layout — measurements are fresh.
    pub(crate) fn step_spring(&mut self, cx: &mut Context<Self>) {
        if self.route_exit_pending(cx) {
            return;
        }
        self.spring_kick = false;
        if !self.pinned {
            self.spring_last_tick = None;
            return;
        }
        let now = Instant::now();
        if self.spring_settled_at.is_some_and(|settled| {
            now.duration_since(settled) >= Duration::from_millis(SPRING_SETTLE_GRACE_MS)
        }) {
            self.spring.reset();
            self.spring_last_tick = None;
            self.spring_settled_at = None;
        }
        let frames = match self.spring_last_tick {
            Some(last) => (now.duration_since(last).as_secs_f32() * 1000.0 / SPRING_FRAME_MS)
                .min(SPRING_MAX_CATCHUP_FRAMES),
            None => 1.0,
        };
        self.spring_last_tick = Some(now);

        let target = f32::from(self.list.max_offset_for_scrollbar().y);
        let mut distance = self.distance_from_bottom();
        // Long jumps (chat switch mid-history, huge pastes) teleport first.
        let viewport = f32::from(self.list.viewport_bounds().size.height);
        let glide_max = GLIDE_MAX_VIEWPORTS * viewport;
        if viewport > 0.0 && distance > glide_max {
            self.list.scroll_by(px(distance - glide_max));
            distance = glide_max;
        }
        let pos = target - distance;
        let next = self.spring.step(pos, target, frames);
        if next > pos {
            self.list.scroll_by(px(next - pos));
        }
        self.last_scroll_distance = (target - next).max(0.0);

        if target - next <= 0.5 {
            // Land on the final item, not the scrollbar's estimated pixel
            // total. Remeasuring virtual rows can otherwise move that total
            // after every landing and restart the glide indefinitely.
            self.list.scroll_to_end();
            self.spring_settled_at.get_or_insert(now);
        } else {
            self.spring_settled_at = None;
        }
        // A stationary spring used to repaint throughout the 500ms grace.
        // Repeated layout kicks kept that loop alive for entire streams even
        // at distance=0, velocity=0. Preserve the final movement's paint and
        // every moving frame; a settled spring wakes on the next layout kick.
        if next > pos || StickSpring::needs_frame(self.last_scroll_distance) {
            cx.notify();
        }
    }
}
