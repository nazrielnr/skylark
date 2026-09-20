use gpui::{
    div, list, prelude::*, AnyElement, Context, IntoElement, MouseButton, Point, Render,
    Window,
};

use crate::motion;
use crate::theme::Theme;

use super::row::record_view_frame;
use super::stick_spring::jump_visibility;
use super::viewport::ViewportFinalizeToken;
use super::Transcript;

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
