//! Locally-authored turn reservation, entry-glide, and queued-turn anchoring.

use std::time::Instant;

use gpui::{px, Context, ListOffset, ListState, SharedString};

use crate::motion;

use super::row::user_resize_spec;
use super::stick_spring::{
    jump_visibility, own_turn_glide_crossed, AT_BOTTOM_PX, GLIDE_MAX_VIEWPORTS,
    OWN_SEND_GLIDE_RETAIN, OWN_SEND_GLIDE_SNAP_PX, OWN_SEND_SCROLL_SLACK_PX,
    OWN_SEND_TOP_INSET_PX, SPRING_FRAME_MS, SPRING_MAX_CATCHUP_FRAMES,
};
use super::viewport::OwnTurnAnchor;
use super::Transcript;

impl Transcript {
    /// Reserve the reply's space below a locally-sent prompt — EVERY send,
    /// not just the first (a steer or a post-turn send used to collapse the
    /// previous reservation and drop the messages back down — user report).
    /// [`Self::step_own_turn`] sizes the reservation and eases the prompt to
    /// its top inset. Replacing a previous anchor starts a new glide.
    pub fn on_own_send(&mut self, chat_id: String, message_id: String, cx: &mut Context<Self>) {
        self.user_collapse_scroll = None;
        self.cancel_user_hold();
        self.discard_pending_viewport();
        self.pinned = false;
        self.show_jump_button = false;
        self.spring.reset();
        self.spring_last_tick = None;
        self.spring_settled_at = None;
        self.spring_kick = false;
        self.scroll_anim = None;
        // A glued offset re-snaps to the end on EVERY layout — the pad would
        // land and the viewport hard-track its bottom in the same frame,
        // skipping the glide entirely (rig-traced). Pin the offset to a
        // CONCRETE visible item first; the pad then reads as scrollable
        // distance for the glide to cover.
        self.materialize_scroll_anchor();
        let prompt_ix = self
            .rows
            .iter()
            .position(|row| row.turn_start && row.entry_id == message_id.as_str());
        self.own_turn = Some(OwnTurnAnchor {
            chat_id,
            message_id: SharedString::from(message_id),
            held: true,
            positioned: false,
            seen_prompt: prompt_ix.is_some(),
        });
        self.own_turn_last_tick = None;
        self.own_turn_kick = true;
        self.remeasure_last_row();
        cx.notify();
    }

    /// Remember a locally-authored queue row without touching the active
    /// runway. If host promotion won the race with the QueueMessage reply, the
    /// matching prompt is already present and can be anchored immediately.
    pub fn on_own_queued_send(
        &mut self,
        chat_id: String,
        message_id: String,
        cx: &mut Context<Self>,
    ) {
        let materialized = self.chat_id.as_deref() == Some(chat_id.as_str())
            && self
                .rows
                .iter()
                .any(|row| row.turn_start && row.entry_id == message_id.as_str());
        if materialized {
            self.on_own_send(chat_id, message_id, cx);
        } else {
            self.pending_queued_turns.register(chat_id, message_id);
        }
    }

    /// Promote a queued row only after its real transcript bubble exists.
    /// Materializations first observed while attaching to a chat are consumed
    /// without taking viewport ownership: navigation must not create a hidden
    /// auto-follow merely because queued work ran while the chat was away.
    pub(crate) fn promote_materialized_queued_turn(&mut self, attached: bool, cx: &mut Context<Self>) {
        let Some(chat_id) = self.chat_id.clone() else {
            return;
        };
        let Some(message_id) = self
            .pending_queued_turns
            .take_latest_materialized(&chat_id, &self.rows)
        else {
            return;
        };
        if !attached {
            self.on_own_send(chat_id, message_id, cx);
        }
    }

    /// Convert a glued scroll offset (`None`/past-the-end — layout re-snaps
    /// it to the end each frame) into a concrete `{item, offset}` anchored at
    /// the first visible row, which layout holds still.
    pub(crate) fn materialize_scroll_anchor(&mut self) {
        if !self.is_glued() {
            return;
        }
        let vp_top = f32::from(self.list.viewport_bounds().top());
        for ix in 0..self.rows.len() {
            if let Some(bounds) = self.list.bounds_for_item(ix)
                && f32::from(bounds.bottom()) > vp_top + 0.5
            {
                self.list.scroll_to(ListOffset {
                    item_ix: ix,
                    offset_in_item: px(vp_top - f32::from(bounds.top())),
                });
                return;
            }
        }
        // Bottom-aligned short lists expose no item bounds. Materialize
        // their actual end position using the measured height tree instead.
        // Preserve a negative first-row offset for the blank space above a
        // short chat; clamping it to zero would jump as the minimum is added.
        if !self.rows.is_empty() {
            let viewport_height = f32::from(self.list.viewport_bounds().size.height);
            self.list.scroll_by(px(-1.0));
            let content_height = -f32::from(self.list.scroll_px_offset_for_scrollbar().y) + 1.0;
            if content_height < viewport_height {
                self.list.scroll_to(ListOffset {
                    item_ix: 0,
                    offset_in_item: px(content_height - viewport_height),
                });
            } else {
                self.list.scroll_by(px(1.0 - viewport_height));
            }
        }
    }

    /// The held prompt's top offset from the viewport top. Row 0 already
    /// carries the titlebar chrome inside its own box (the first row's
    /// top gap), so the hold adds nothing — adding the inset on top parked
    /// a new chat's first prompt a double-chrome ~66px low (user report).
    pub(crate) fn own_send_inset(anchor_ix: usize) -> f32 {
        if anchor_ix == 0 {
            0.0
        } else {
            OWN_SEND_TOP_INSET_PX
        }
    }

    pub(crate) fn own_turn_anchor_ix(&self) -> Option<usize> {
        let anchor = self.own_turn.as_ref()?;
        self.rows
            .iter()
            .position(|row| row.turn_start && row.entry_id == anchor.message_id)
    }

    pub(crate) fn reconcile_own_turn_prompt(&mut self) {
        let Some(message_id) = self
            .own_turn
            .as_ref()
            .map(|anchor| anchor.message_id.clone())
        else {
            return;
        };
        let exists = self
            .rows
            .iter()
            .any(|row| row.turn_start && row.entry_id == message_id);
        let keep = self
            .own_turn
            .as_mut()
            .is_some_and(|anchor| anchor.observe_prompt(exists));
        if keep {
            return;
        }

        self.own_turn = None;
        self.own_turn_kick = false;
        self.own_turn_last_tick = None;
        self.remeasure_last_row();
        self.last_scroll_distance = self.distance_from_bottom();
        self.show_jump_button = jump_visibility(self.show_jump_button, self.last_scroll_distance);
        self.viewport_finalize_pending = true;
    }

    /// Install the reservation before list layout. Its height follows the
    /// current viewport in that same pass, including window resizes.
    pub(crate) fn update_runway_minimum(&mut self, cx: &gpui::App) {
        if let Some(ix) = self.own_turn_anchor_ix() {
            // Revealing the prompt adds reading space; only reply growth
            // consumes the runway. Include the same expansion tween in the
            // reservation before layout, including its reverse on Show less.
            let expansion = self.user_folds.get(&self.rows[ix].id).map_or(0.0, |fold| {
                let target = if fold.open == Some(true) {
                    fold.user_expansion_height
                } else {
                    0.0
                };
                match fold.toggled_at {
                    Some(at) if !motion::reduced_motion(cx) && fold.duration_ms > 0 => {
                        let raw = (at.elapsed().as_secs_f32() * 1000.0 / fold.duration_ms as f32)
                            .clamp(0.0, 1.0);
                        let progress = user_resize_spec(fold.user_expansion_height).progress(raw);
                        motion::lerp(fold.user_expansion_height - target, target, progress)
                    }
                    _ => target,
                }
            });
            self.list.set_tail_reservation(Some((
                ix,
                px(Self::own_send_inset(ix) - OWN_SEND_SCROLL_SLACK_PX - expansion),
            )));
        } else if self.own_turn.is_none() {
            self.list.set_tail_reservation(None);
        }
    }

    pub(crate) fn scroll_own_turn_by(&self, delta: f32) {
        let offset = self.list.logical_scroll_top();
        if offset.item_ix == 0 && offset.offset_in_item < px(0.0) {
            self.list.scroll_to(ListOffset {
                item_ix: 0,
                offset_in_item: offset.offset_in_item + px(delta),
            });
        } else {
            self.list.scroll_by(px(delta));
        }
    }

    /// Advance the prompt glide or hand a filled reservation to tail-follow.
    /// Reservation sizing happens in the list layout, never in this callback.
    pub(crate) fn step_own_turn(&mut self, cx: &mut Context<Self>) {
        if self.route_exit_pending(cx) {
            return;
        }
        self.own_turn_kick = false;
        // Layout moves the bottom too (pad refinement, streaming growth):
        // refresh the wheel handler's escape baseline every frame so only a
        // WHEEL's own delta registers as user intent. Without this, the pad
        // growing at turn-completion between two wheel events read as
        // "scrolled away" and silently released the hold — the next wheels
        // then sank unopposed deep into the runway blank (rig-traced).
        self.last_scroll_distance = self.distance_from_bottom();
        let Some(anchor_ix) = self.own_turn_anchor_ix() else {
            // The optimistic echo may arrive on the next state notification.
            return;
        };
        if let Some(anchor) = self.own_turn.as_mut() {
            anchor.seen_prompt = true;
        }
        let viewport = self.list.viewport_bounds();
        let viewport_height = f32::from(viewport.size.height);
        if viewport_height <= 0.0 {
            self.own_turn_kick = true;
            cx.notify();
            return;
        }
        let inset = Self::own_send_inset(anchor_ix);
        if self.is_glued() && self.own_turn.as_ref().is_some_and(|anchor| anchor.held) {
            self.list.scroll_by(px(-viewport_height));
        }
        let anchor_bounds = self.list.bounds_for_item(anchor_ix);
        // The list consumes the reservation in the same layout that measures
        // new rows. The height tree remains available when the prompt or tail
        // is outside the viewport, so neither can block the handoff.
        if self.list.tail_reservation_filled() {
            let held = self.own_turn.take().is_some_and(|a| a.held);
            self.own_turn_last_tick = None;
            self.list.set_tail_reservation(None);
            if held
                || self.pinned
                || (self.selection_drag_position.is_none()
                    && self.distance_from_bottom() <= AT_BOTTOM_PX)
            {
                self.engage_pin(cx);
            } else {
                cx.notify();
            }
            return;
        }

        // ---- entry glide, then absolute hold -------------------------------
        let (held, positioned) = self
            .own_turn
            .as_ref()
            .map_or((false, false), |a| (a.held, a.positioned));
        if !held {
            return;
        }
        if positioned {
            // Landed: re-assert the prompt's position after every layout.
            // scroll_to is absolute and bounds-independent, so neither glue
            // re-snaps, pad-sizing lag, nor a splice's unmeasured flicker can
            // carry the view off the prompt (each broke the spring-held
            // variants of this — rig-traced). ONE-SIDED: only upward drift
            // (view above the hold) is corrected. The scroll slack under the
            // reservation is legal resting space — wheel-down sinks into it
            // and stops hard at the list's own clamp; snapping back up from
            // there made the bottom bounce/stutter on every scroll event
            // (user report). Way-below-slack (impossible short of a bug)
            // still re-asserts.
            let moved = match anchor_bounds {
                Some(b) => {
                    let err = f32::from(b.top()) - (f32::from(viewport.top()) + inset);
                    // The legal rest zone below the hold is the epsilon plus
                    // rounding; anything deeper is a transient-collision sink
                    // and rubber-bands back.
                    err > 0.5 || err < -(OWN_SEND_SCROLL_SLACK_PX + 2.0)
                }
                // Bounds vanish in the glued representation (dissolved
                // above, so at most for this one frame) and through splice
                // flicker. Near the stop that is dead-band space — no
                // assert (asserting on None here was the bottom bounce);
                // far from it the position is unknowable flicker: re-assert.
                None => self.distance_from_bottom() > OWN_SEND_SCROLL_SLACK_PX + 8.0,
            };
            if moved {
                // Correct with the entry glide's ease, not a snap: the only
                // in-band escapes are one-frame commit transients and splice
                // flicker, and an eased ~200ms return reads as native
                // rubber-banding where an instant re-assert read as stutter
                // (user report). Bounds-less flicker still snaps — there is
                // nothing to ease against.
                match anchor_bounds {
                    Some(b) => {
                        let err = f32::from(b.top()) - (f32::from(viewport.top()) + inset);
                        let now = Instant::now();
                        let frames = match self.own_turn_last_tick {
                            Some(last) => (now.duration_since(last).as_secs_f32() * 1000.0
                                / SPRING_FRAME_MS)
                                .min(SPRING_MAX_CATCHUP_FRAMES),
                            None => 1.0,
                        };
                        self.own_turn_last_tick = Some(now);
                        let ease = 1.0 - OWN_SEND_GLIDE_RETAIN.powf(frames);
                        if err.abs() <= OWN_SEND_GLIDE_SNAP_PX {
                            self.list.scroll_by(px(err));
                            self.own_turn_last_tick = None;
                        } else {
                            self.scroll_own_turn_by(err * ease);
                        }
                        self.own_turn_kick = true;
                    }
                    None => {
                        self.list.scroll_to(ListOffset {
                            item_ix: anchor_ix,
                            offset_in_item: px(-inset),
                        });
                        self.own_turn_last_tick = None;
                    }
                }
                cx.notify();
            } else {
                self.own_turn_last_tick = None;
            }
            return;
        }
        let now = Instant::now();
        let frames = match self.own_turn_last_tick {
            Some(last) => (now.duration_since(last).as_secs_f32() * 1000.0 / SPRING_FRAME_MS)
                .min(SPRING_MAX_CATCHUP_FRAMES),
            None => 1.0,
        };
        self.own_turn_last_tick = Some(now);
        let ease = 1.0 - OWN_SEND_GLIDE_RETAIN.powf(frames);
        // Prefer the prompt geometry. When it is being remeasured but is
        // already the scroll anchor, its logical offset is equally exact.
        // Otherwise approach through the unmeasured rows, capping every step
        // at the prompt so the provisional minimum can never cause overshoot.
        let (err, anchored) = match anchor_bounds {
            Some(bounds) => (
                f32::from(bounds.top()) - (f32::from(viewport.top()) + inset),
                true,
            ),
            None if self.list.logical_scroll_top().item_ix == anchor_ix => (
                -f32::from(self.list.logical_scroll_top().offset_in_item) - inset,
                true,
            ),
            None => {
                // Remeasurement retains preceding row heights as hints. Read
                // the prompt's coordinate from that same height tree rather
                // than aiming at the provisional minimum's (larger) bottom.
                let current = f32::from(self.list.scroll_px_offset_for_scrollbar().y);
                let target = -f32::from(self.list.offset_for_item(anchor_ix)) + inset;
                (current - target, false)
            }
        };
        let glide_max = GLIDE_MAX_VIEWPORTS * viewport_height;
        let err = if err > glide_max {
            self.list.scroll_by(px(err - glide_max));
            glide_max
        } else {
            err
        };
        let land = |list: &ListState| {
            list.scroll_to(ListOffset {
                item_ix: anchor_ix,
                offset_in_item: px(-inset),
            });
        };
        if motion::reduced_motion(cx) {
            land(&self.list);
            if let Some(anchor) = self.own_turn.as_mut() {
                anchor.positioned = true;
            }
            self.own_turn_last_tick = None;
        } else if anchored
            && err <= OWN_SEND_GLIDE_SNAP_PX
            && err >= -(OWN_SEND_SCROLL_SLACK_PX + 2.0)
        {
            // At the hold — or resting inside the slack under it (a restick
            // that fired at the true bottom): land WITHOUT pulling the view
            // up. Only a still-above position gets the snap.
            if err > 0.5 {
                land(&self.list);
            }
            if let Some(anchor) = self.own_turn.as_mut() {
                anchor.positioned = true;
            }
            self.own_turn_last_tick = None;
        } else if !anchored && err <= OWN_SEND_GLIDE_SNAP_PX {
            // The height hints put us at the prompt. Land by row identity
            // so its final measurement cannot leave us in the reservation.
            land(&self.list);
            if let Some(anchor) = self.own_turn.as_mut() {
                anchor.positioned = true;
            }
            self.own_turn_last_tick = None;
        } else {
            self.scroll_own_turn_by(err * ease);
            if own_turn_glide_crossed(self.list.logical_scroll_top(), anchor_ix, inset) {
                land(&self.list);
            }
        }
        self.own_turn_kick = true;
        cx.notify();
    }
}
