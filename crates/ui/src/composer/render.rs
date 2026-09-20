//! Composer render and layout pipeline.

use std::time::{Duration, Instant};

use gpui::{
    div, px, Context, Focusable, KeyDownEvent, MouseButton, Render, SharedString, Window,
    prelude::*,
};

use crate::attachments;
use crate::motion;
use crate::notice::{NoticeChipIcon, notice_chip};
use crate::theme::Theme;

use super::attachment_strip::*;
use super::decision::*;
use super::Composer;

impl Render for Composer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_pending {
            self.focus_pending = false;
            let focus = self.input.focus_handle(cx);
            window.focus(&focus, cx);
        }
        let theme = Theme::of(cx).clone();
        let wizard_active = self.wizard.is_some();
        if self.mention.token.is_some()
            && (wizard_active || !self.input.focus_handle(cx).is_focused(window))
        {
            self.reset_mention(None, cx);
        }
        if self.slash.token.is_some()
            && (wizard_active || !self.input.focus_handle(cx).is_focused(window))
        {
            self.reset_slash(None, cx);
        }
        let mode = self.button_mode(cx);
        // Shape the current draft before sizing the pill. Waiting for the child
        // layout leaves the parent using the previous edit's height.
        self.input.update(cx, |input, cx| {
            if input.needs_measure && input.last_width > 0.0 {
                let mut style = window.text_style();
                style.font_family = theme.font_sans.clone();
                style.font_size = crate::typography::ui_rems(INPUT_TEXT_SIZE).into();
                style.color = if input.content.is_empty() {
                    theme.text_faint
                } else {
                    theme.text
                };
                input.layout_text(px(input.last_width), &style, window, cx);
            }
        });
        let (text_width, has_newline, content_height, last_width, epoch) = {
            let input = self.input.read(cx);
            (
                input.measured_text_width(),
                input.has_newline(),
                input.measured_content_height(),
                input.last_width,
                input.layout_epoch,
            )
        };
        let now = Instant::now();
        // Only measurements taken *after* the last flip may drive the next one
        // (at most one flip per layout pass — a flip invalidates the widths).
        let measured_since_flip = epoch > self.flip_epoch && last_width > 0.0;
        if measured_since_flip {
            // A same-mode width change is an interactive window/pane resize:
            // defer collapse until sizes settle for RESIZE_SETTLE_MS. Expansion
            // remains live so compact controls never squeeze the input away.
            if self.last_seen_width > 0.0 && (last_width - self.last_seen_width).abs() > 0.5 {
                self.width_changed_at = Some(now);
            }
            self.last_seen_width = last_width;
            if self.expanded_mode {
                if self.expanded_anchor <= 0.0 {
                    self.expanded_anchor = last_width;
                }
            } else {
                // The compact pill's content box is the layout-stable capacity
                // both thresholds measure against.
                self.compact_capacity = last_width - 8.0;
            }
        }
        let resizing = self
            .width_changed_at
            .is_some_and(|t| now.duration_since(t) < Duration::from_millis(RESIZE_SETTLE_MS));
        if resizing && self.settle_task.is_none() {
            // Re-evaluate once the settle window has passed.
            self.settle_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(RESIZE_SETTLE_MS + 20))
                    .await;
                this.update(cx, |composer, cx| {
                    composer.settle_task = None;
                    cx.notify();
                })
                .ok();
            }));
        }
        // Layout-stable compact capacity: measured directly while compact;
        // while expanded, the learned value shifted by any container resize
        // (the expanded input width tracks the container 1:1).
        let capacity = if !self.expanded_mode {
            if last_width > 0.0 {
                last_width - 8.0
            } else {
                f32::MAX // before first measure default to compact
            }
        } else if self.compact_capacity > 0.0 {
            if self.expanded_anchor > 0.0 && last_width > 0.0 {
                self.compact_capacity + (last_width - self.expanded_anchor)
            } else {
                self.compact_capacity
            }
        } else {
            f32::MAX
        };
        let next = composer_flip(
            self.expanded_mode,
            text_width,
            capacity,
            has_newline,
            resizing,
        );
        let committed_flip = next != self.expanded_mode && measured_since_flip;
        if committed_flip {
            self.expanded_mode = next;
            self.flip_epoch = epoch;
            self.expanded_anchor = 0.0;
            // The mode change moves the input width; don't read that jump as
            // an interactive resize.
            self.last_seen_width = 0.0;
        }
        // New chats render expanded regardless of `expanded_mode` (see below),
        // so a mode flip there changes nothing visible — never morph it.
        let new_chat = self.state.read(cx).selected_chat.is_none();
        // Morph clock in ms; dividing by the measurement knob stretches the
        // timeline exactly like shell.rs eval_tween's scaled duration.
        let now_ms = self.morph_clock.elapsed().as_secs_f32() * 1000.0 / motion::speed_scale();
        let route_snap = self
            .route_snap_until
            .is_some_and(|until| Instant::now() < until);
        let dock_height_changed = std::mem::take(&mut self.dock_height_changed);
        self.flip_morph =
            if dock_height_changed || self.dock_frame.is_some_and(|frame| frame.active) {
                None
            } else {
                flip_morph_step(
                    self.flip_morph,
                    committed_flip && !new_chat,
                    self.last_rendered_height,
                    now_ms,
                    motion::reduced_motion(cx),
                    route_snap,
                )
            };
        let expanded = self.expanded_mode;

        // Chat-scoped failures render only under their own chat; a global
        // failure (no key) renders everywhere.
        let failure = self.failure.clone().filter(|_| {
            self.failure_key
                .as_ref()
                .is_none_or(|key| *key == self.current_key)
        });
        // Composer honesty: when the target's delivery path is degraded, say
        // UP FRONT that a send will queue (a durable local write delivered on
        // reconnect) instead of letting the button imply instant delivery.
        let queue_notice: Option<(SharedString, bool)> = {
            use zeron_proto::ConnectivityState as S;
            let state = self.state.read(cx);
            let degraded = match state.selected_chat.as_deref() {
                Some(id) => state.chat_delivery_degraded(id),
                None => {
                    // New-chat canvas: judge by the picked target device.
                    let remote_target = state
                        .effective_device_id()
                        .is_some_and(|id| state.local_device_id.as_deref() != Some(id.as_str()));
                    remote_target
                        && (matches!(state.connectivity.state, S::Offline | S::Reconnecting)
                            || state
                                .effective_device_id()
                                .is_some_and(|id| !state.device_online(&id, chrono::Utc::now())))
                }
            };
            let offline = state.connectivity.state == S::Offline;
            degraded.then(|| {
                let text: SharedString = if offline {
                    "Offline — messages will send when you're back online.".into()
                } else {
                    "Messages will send once the connection recovers.".into()
                };
                (text, offline)
            })
        };
        // Centered composer column (zeron `mx-auto w-full max-w-3xl`).
        let container = div()
            .w_full()
            .max_w(px(COMPOSER_MAX_WIDTH))
            .mx_auto()
            .flex()
            .flex_col()
            .gap(px(Theme::SPACE_SM))
            .px(px(Theme::SPACE_LG))
            .pb(px(Theme::SPACE_LG))
            .when_some(failure, |el, message| {
                // Amber with "Warning" for the offline-ish case (engine not
                // connected), red with "Error" for send/run failures. Click
                // dismisses.
                let offline = message.as_ref() == "Engine not connected";
                el.child(
                    notice_chip(
                        &theme,
                        offline,
                        if offline { "Warning" } else { "Error" },
                        message,
                        NoticeChipIcon::Plain,
                    )
                    .id("composer-failure")
                    .mx(px(4.0))
                    .mt(px(6.0))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.failure = None;
                        this.failure_key = None;
                        cx.notify();
                    })),
                )
            })
            .when_some(queue_notice, |el, (notice, offline)| {
                // Not a warning box (v0.2.12 feedback: the amber Notice read
                // as an error and flashed on every blip — pre-grace). One
                // quiet caption line, amber dot only for hard offline; it
                // clears itself the moment the path heals.
                let dot = if offline {
                    theme.warning
                } else {
                    theme.text_faint
                };
                el.child(crate::motion::fade_in(
                    "composer-queue-notice",
                    div()
                        .id("composer-queue-notice")
                        .mx(px(8.0))
                        .mt(px(6.0))
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_size(px(11.0))
                        .line_height(px(14.0))
                        .text_color(theme.text_faint)
                        .child(div().size(px(5.0)).rounded_full().bg(dot))
                        .child(div().min_w_0().truncate().child(notice)),
                ))
            });

        if wizard_active {
            let wizard = self.render_wizard(cx);
            return container.child(motion::fade_quick("composer-wizard", div().child(wizard)));
        }

        // What is waiting to be sent, stacked directly above the box it was
        // typed in — the queue is a property of this composer, not a panel
        // somewhere else.
        let show_queue_latest_shortcut = self.queue_shortcut_revealed
            && self.editing_queued.is_none()
            && !self.pickers.read(cx).is_open()
            && !composer_has_content(
                self.input.read(cx).text(),
                self.staged().len() + self.staged_appshots().len(),
                self.staged_comments(cx).len(),
            );
        let container = container.when_some(
            self.render_queue_panel(show_queue_latest_shortcut, window, cx),
            |el, panel| {
                el.child(motion::fade_quick(
                    "composer-queue",
                    div()
                        .mx(px(QUEUE_SIDE_INSET))
                        // Cancel the column gap, then tuck the tray one pixel
                        // behind the composer painted after it.
                        .mb(px(-(Theme::SPACE_SM + QUEUE_COMPOSER_OVERLAP)))
                        .child(panel),
                ))
            },
        );
        // Escape backs out of a queue-row edit (the row keeps its old text).
        // Bound here rather than in the input: the input's own Escape belongs
        // to the mention/slash popups, which outrank this while they're open.
        let container = container.when(self.editing_queued.is_some(), |el| {
            el.on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape"
                    && this.mention.token.is_none()
                    && this.slash.token.is_none()
                    && this.cancel_queue_edit(cx)
                {
                    cx.stop_propagation();
                }
            }))
        });

        // Route coordination must not force an established thread into the
        // two-row layout. Short drafts keep the original skinny composer.
        let session_expanded = expanded;
        let expanded = expanded || new_chat;
        let dock_amount = self.dock_frame.map_or(0.0, |frame| frame.amount);
        let dock_height = |amount: f32| {
            let hero = composer_total_height(content_height);
            let session = if session_expanded {
                (content_height + TEXTAREA_PAD_V).clamp(TEXTAREA_MIN - 16.0, TEXTAREA_MAX)
                    + ACTIONS_ROW_HEIGHT
                    + PILL_BORDER_V
            } else {
                COMPACT_TOTAL_HEIGHT
            };
            motion::lerp(hero, session, amount)
        };

        // Committed-height morph: the layout below is already the NEW mode's;
        // only the pill's height (and the entrance fade/text glide driven by
        // `morph_t`) animates. Steady state renders exactly the target.
        // Staged attachments add the wrap strip's height to the pill in BOTH
        // modes (attachment-ui.tsx AttachmentStrip sits above the input row).
        let staged_count = self.staged().len();
        // The input width excludes the inline controls in compact mode.
        // Wrap against the pill's content width in both modes, accounting
        // for the outer container padding and the pill's 1px borders.
        let strip_width_hint =
            self.last_available_width.unwrap_or(COMPOSER_MAX_WIDTH) - 2.0 * Theme::SPACE_LG - 2.0;
        let appshot_count = self.staged_appshots().len();
        let strip_h = attachment_strip_height(staged_count, strip_width_hint);
        let comment_strip_h = comment_strip_height(self.staged_comments(cx).len());
        let base_height = if self.dock_frame.is_some() {
            dock_height(dock_amount)
        } else if expanded {
            composer_total_height(content_height)
        } else {
            COMPACT_TOTAL_HEIGHT
        };
        let target_height =
            base_height + strip_h + appshot_strip_height(appshot_count) + comment_strip_h;
        let coordinated_route_morph = self
            .flip_morph
            .filter(|m| m.spec == motion::NEW_THREAD_TRANSITION && !m.done(now_ms));
        // The route state commits before its shared-element animation begins.
        // Reconstruct the departing chrome at t=0, then progressively trade
        // it for the destination chrome so neither route changes the outer
        // composer geometry in a single frame.
        let new_thread_chrome = self
            .dock_frame
            .map(|frame| frame.selectors())
            .unwrap_or_else(|| {
                coordinated_route_morph.map_or_else(
                    || if new_chat { 1.0 } else { 0.0 },
                    |morph| {
                        let progress = morph.progress(now_ms);
                        if new_chat { progress } else { 1.0 - progress }
                    },
                )
            });
        let (new_thread_chrome_opacity, session_chrome_opacity) = self.dock_frame.map_or_else(
            || route_chrome_opacities(new_thread_chrome),
            |frame| (frame.selectors(), frame.footer()),
        );
        self.height_morph =
            if dock_height_changed || self.dock_frame.is_some_and(|frame| frame.active) {
                None
            } else if coordinated_route_morph.is_some() {
                coordinated_route_morph
            } else {
                flip_morph_step(
                    self.height_morph,
                    (target_height - self.last_target_height).abs() > 0.5,
                    self.last_rendered_height,
                    now_ms,
                    motion::reduced_motion(cx),
                    route_snap,
                )
            };
        self.last_target_height = target_height;
        let pill_height = self
            .height_morph
            .map_or(target_height, |m| m.height(target_height, now_ms));
        if self.height_morph.is_some() {
            window.request_animation_frame();
        }
        let (_, morph_t, morphing) = match self.flip_morph {
            Some(m) if !m.done(now_ms) => {
                (m.height(target_height, now_ms), m.progress(now_ms), true)
            }
            _ => (target_height, 1.0, false),
        };
        if !morphing {
            self.flip_morph = None;
        } else {
            // Manual tween drive: keep frames coming (shell.rs motion_active).
            window.request_animation_frame();
        }
        self.last_rendered_height = pill_height;
        self.dock_clearance_correction = self.dock_frame.map_or(0.0, |frame| {
            dock_height(if frame.docked { 1.0 } else { 0.0 })
                + strip_h
                + appshot_strip_height(appshot_count)
                + comment_strip_h
                - pill_height
        });
        // Route morphs use the dock's reversible clock; typing flips keep
        // their existing local clock once the composer reaches its dock.
        let layout_morph_t =
            if self.dock_frame.is_some_and(|frame| frame.active) && !session_expanded {
                if expanded {
                    1.0 - dock_amount
                } else {
                    dock_amount
                }
            } else {
                morph_t
            };
        let text_pt = morph_text_pad(layout_morph_t);
        let surface_radius = COMPOSER_RADIUS - 4.0 * dock_amount;
        let route_to_single_line =
            self.dock_frame.is_some_and(|frame| frame.active) && !session_expanded;
        let textarea_height = (pill_height
            - strip_h
            - appshot_strip_height(appshot_count)
            - comment_strip_h
            - PILL_BORDER_V
            - ACTIONS_ROW_HEIGHT)
            .max(if route_to_single_line {
                INPUT_LINE_HEIGHT + text_pt + 4.0
            } else {
                0.0
            });
        self.input.update(cx, |input, cx| {
            let height = if expanded {
                (textarea_height - text_pt - 4.0).max(0.0)
            } else {
                INPUT_LINE_HEIGHT
            };
            let settled_height = if expanded {
                (base_height - PILL_BORDER_V - ACTIONS_ROW_HEIGHT - TEXTAREA_PAD_V).max(
                    if route_to_single_line {
                        INPUT_LINE_HEIGHT
                    } else {
                        0.0
                    },
                )
            } else {
                INPUT_LINE_HEIGHT
            };
            let resizing = self.height_morph.is_some();
            let top_padding = if expanded { text_pt } else { 0.0 };
            if input.viewport_height != Some(height)
                || input.settled_viewport_height != Some(settled_height)
                || input.resizing != resizing
                || input.overflow_top_padding != top_padding
            {
                input.resizing = resizing;
                input.overflow_top_padding = top_padding;
                input.viewport_height = Some(height);
                input.settled_viewport_height = Some(settled_height);
                cx.notify();
            }
        });

        let send_button = self.render_send_button(mode, cx);
        // Attach button — opens the native image picker (the original's hidden
        // `<input type=file accept="image/*" multiple>`); paste/drop also feed
        // the same strip. The leading utility group owns the spacing between
        // this button and the model picker.
        let attach = div()
            .id("composer-attach")
            .size(px(28.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .cursor_pointer()
            // zeron composer-actions.tsx attach: `transition-colors`.
            .bg(motion::hover_blend(
                "composer-attach",
                gpui::transparent_black(),
                crate::theme::ink(0.10),
            ))
            .on_hover(motion::hover_listener("composer-attach"))
            .on_click(cx.listener(|this, _, _, cx| this.open_file_picker(cx)))
            .child(
                crate::icons::icon(crate::icons::PAPERCLIP)
                    .size(px(16.0))
                    // The source path's painted bounds are centered at x=11
                    // inside a 24px viewbox. Correct that optical offset while
                    // keeping the 28px hit target geometrically centered.
                    .relative()
                    .left(px(1.0))
                    .text_color(theme.text_muted),
            );
        // Staged-thumbnail strip (attachment-ui.tsx AttachmentStrip), above
        // the input inside the pill in both modes.
        let strip = self.render_attachment_strip(&theme, cx);
        let appshot_strip = self.render_appshot_strip(&theme, window, cx);
        let comments_chip = self.render_comments_chip(&theme, cx);

        // A translucent cool silver/slate edge sits more naturally on frost
        // than the general-purpose white/black separator color.
        let pill_border = if theme.is_frost() {
            match theme.appearance {
                crate::theme::Appearance::Dark => gpui::hsla(210.0 / 360.0, 0.18, 0.78, 0.09),
                crate::theme::Appearance::Light => gpui::hsla(210.0 / 360.0, 0.18, 0.32, 0.10),
            }
        } else {
            theme.border
        };
        let pill_bg = if theme.is_frost() {
            #[cfg(target_os = "windows")]
            {
                match theme.appearance {
                    crate::theme::Appearance::Dark => theme.surface_overlay.opacity(0.50),
                    crate::theme::Appearance::Light => theme.surface_overlay.opacity(0.75),
                }
            }
            #[cfg(not(target_os = "windows"))]
            {
                theme.composer_sidebar_tint()
            }
        } else {
            theme.input_glass_bg()
        };
        let pill = div()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    // Padding and action controls are part of the text composer.
                    // Open menus keep their own keyboard/search focus.
                    if !this.pickers.read(cx).is_open() {
                        window.focus(&this.input.focus_handle(cx), cx);
                    }
                }),
            )
            .rounded(px(surface_radius))
            .border_1()
            .border_color(pill_border)
            .shadow_lg()
            .bg(pill_bg);
        // The pill's bottom edge is stationary on screen (the composer sits at
        // the bottom of the shell column; growth moves the TOP edge), so the
        // controls pin to the bottom and only the text glides with the reveal
        // (round-9 follow-up: the send/attach/chips must not ride the height,
        // while the model picker fades between its two horizontal anchors).
        let cluster_dy = morph_cluster_dy(layout_morph_t);
        // Share the height/route timeline instead of starting an independent
        // animation. Reversals continue from the current handoff phase.
        if self.model_handoff_morph != self.flip_morph {
            self.model_handoff_from = self.model_handoff_position;
            self.model_handoff_morph = self.flip_morph;
        }
        let compact_target = if expanded { 0.0 } else { 1.0 };
        self.model_handoff_position =
            if self.dock_frame.is_some_and(|frame| frame.active) && !session_expanded {
                dock_amount
            } else {
                self.flip_morph.map_or(compact_target, |morph| {
                    motion::lerp(
                        self.model_handoff_from,
                        compact_target,
                        motion::EASE_IN_OUT.eval(morph.raw(now_ms)),
                    )
                })
            };
        let surface_width = self
            .surface_bounds
            .get()
            .map_or(strip_width_hint + PILL_BORDER_V, |bounds| {
                f32::from(bounds.size.width)
            });
        let model_travel = (surface_width
            - PILL_BORDER_V
            - 12.0
            - 28.0
            - ACTION_UTILITY_GAP
            - self
                .model_bounds
                .get()
                .map_or(0.0, |bounds| f32::from(bounds.size.width))
            - ACTION_PRIMARY_GAP
            - 28.0
            - morph_cluster_inset(expanded, layout_morph_t))
        .max(0.0);
        let (model_side, model_opacity, model_drift) = model_handoff(self.model_handoff_position);
        let model_offset = (model_side - compact_target) * model_travel + model_drift;
        let measured_model_bounds = self.model_bounds.clone();
        let model_picker = div()
            .min_w_0()
            .max_w(px(surface_width * 0.45))
            .relative()
            .left(px(model_offset))
            .opacity(model_opacity)
            .child(self.pickers.clone())
            .child(
                gpui::canvas(
                    move |bounds, _, _| measured_model_bounds.set(Some(bounds)),
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            );
        let body = if expanded {
            // Expanded: textarea on top (`px-4 pb-1 pt-4`), actions row
            // (12px bottom + 2px top, 32px chips → 46px) ABSOLUTE at the pill's
            // stationary bottom — constant screen-y through the morph, with
            // the 4.5px compact↔expanded centering delta gliding out. The
            // text viewport follows the animated height so it cannot paint
            // over the controls. Its width stays fixed (no tween rewraps);
            // top padding eases 12→16. Attachment and Send stay on the bottom
            // anchor while the model chip fades between its horizontal slots.
            pill.h(px(pill_height))
                .overflow_hidden()
                .relative()
                .flex()
                .flex_col()
                .children(comments_chip)
                .children(appshot_strip)
                .children(strip)
                .child(
                    div()
                        .h(px(textarea_height))
                        .flex_none()
                        .overflow_hidden()
                        .px(px(16.0))
                        .pt(px(text_pt))
                        .pb(px(4.0))
                        .child(self.render_input_with_completion()),
                )
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom(px(-cluster_dy))
                        .h(px(ACTIONS_ROW_HEIGHT))
                        .flex()
                        .flex_row()
                        .items_center()
                        // Shared group geometry (see CLUSTER_X_DELTA): the
                        // attachment belongs to the utility pickers, while
                        // Send has a larger structural separation.
                        .gap(px(ACTION_PRIMARY_GAP))
                        .pl(px(12.0))
                        .pr(px(morph_cluster_inset(true, layout_morph_t)))
                        .pt(px(2.0))
                        .pb(px(12.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(ACTION_UTILITY_GAP))
                                .child(attach)
                                .child(model_picker),
                        )
                        .child(send_button),
                )
        } else {
            // Compact pill: attachment on the left, input in the middle,
            // then model and Send on the right, all on one 47px line.
            // The row is BOTTOM-justified: during the collapse morph the pill
            // top sweeps down over a stationary row, the text walks down from
            // its expanded resting place via a decaying relative offset, and
            // attachment/Send hold their spots (4.5px centering delta gliding
            // in), with the model handoff sharing that same timeline.
            let text_glide = if self.dock_frame.is_some_and(|frame| frame.active) {
                collapse_text_glide(dock_height(0.0), dock_amount)
            } else {
                match self.flip_morph {
                    Some(m) if morphing => collapse_text_glide(m.from, morph_t),
                    _ => 0.0,
                }
            };
            pill.h(px(pill_height))
                .overflow_hidden()
                .flex()
                .flex_col()
                .justify_end()
                .children(comments_chip)
                .children(appshot_strip)
                .children(strip)
                .child(
                    div()
                        .h(px(COMPACT_TOTAL_HEIGHT - PILL_BORDER_V))
                        .flex()
                        .flex_row()
                        .items_center()
                        .child(
                            div()
                                .flex_none()
                                .pl(px(12.0))
                                .relative()
                                .top(px(-cluster_dy))
                                .child(attach),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .px(px(8.0))
                                .relative()
                                .top(px(-text_glide))
                                .child(self.render_input_with_completion()),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .max_w(px(surface_width * 0.45))
                                .relative()
                                .top(px(-cluster_dy))
                                .child(model_picker),
                        )
                        .child(
                            div()
                                .flex_none()
                                .pl(px(ACTION_PRIMARY_GAP))
                                .pr(px(morph_cluster_inset(false, layout_morph_t)))
                                .relative()
                                .top(px(-cluster_dy))
                                .child(send_button),
                        ),
                )
        };
        let new_thread_target_selectors = (new_thread_chrome_opacity > 0.0).then(|| {
            self.pickers.update(cx, |pickers, cx| {
                pickers.render_new_thread_target_selectors(cx)
            })
        });
        let new_thread_git_selectors = (new_thread_chrome_opacity > 0.0)
            .then(|| {
                self.pickers.update(cx, |pickers, cx| {
                    pickers.render_new_thread_git_selectors(cx)
                })
            })
            .flatten();
        let has_new_thread_git_selectors = self
            .state
            .read(cx)
            .selected_space_row()
            .is_some_and(|space| space.git_detected);
        // The file dropzone lives in the shell (the whole conversation column,
        // not just the pill — shell.rs `chat-dropzone`); drops land back here
        // via `add_paths`.
        // Frosted: the pill backdrop-blurs the transcript scrolling under it
        // (the popover glass treatment; radius matches the pill's rounding).
        // The shell keeps this entity under one parent on both routes. The
        // surface itself never fades, and frost follows the same morph radius.
        let pill_surface = div()
            .relative()
            .id("composer-surface")
            .child(crate::frost::frosted(surface_radius, crate::frost::MENU_BLUR, body))
            .child({
                let measured = self.surface_bounds.clone();
                // All prepaint completes before any paint. The background
                // reads this cell during paint, never last frame's geometry.
                gpui::canvas(
                    move |bounds, _, _| measured.set(Some(bounds)),
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0()
            })
            // Both completion popups span the full pill width above it —
            // the file-mention and slash tokens are mutually exclusive.
            .children(self.render_file_mention_popup(&theme, cx))
            .children(self.render_slash_popup(&theme, cx));
        // Restore the original chip-only selector treatment: destination at
        // the top-right, no surrounding surface. Cancel the column gap as the
        // row collapses so the pill never jumps at the route boundary.
        let container = if self.dock_frame.is_some() {
            // Floating selectors share the surface's origin and never change its height.
            container.relative().child(
                div()
                    .id("dock-target-selectors")
                    .absolute()
                    .top(px(-28.0))
                    .left(px(Theme::SPACE_LG + 10.0))
                    .right(px(Theme::SPACE_LG + 10.0))
                    .h(px(NEW_THREAD_SELECTOR_ROW_HEIGHT))
                    .flex()
                    .items_start()
                    .justify_end()
                    .opacity(new_thread_chrome_opacity)
                    .children(new_thread_target_selectors),
            )
        } else if new_thread_chrome > 0.0 {
            container.child(
                div()
                    .w_full()
                    .h(px(NEW_THREAD_SELECTOR_ROW_HEIGHT * new_thread_chrome))
                    .mb(px(-Theme::SPACE_SM * (1.0 - new_thread_chrome)))
                    .px(px(10.0))
                    .flex()
                    .items_start()
                    .justify_end()
                    .opacity(new_thread_chrome_opacity)
                    .children(new_thread_target_selectors),
            )
        } else {
            container
        };
        let container = container.child(pill_surface);

        // The lower slot keeps a stable footprint for Git projects while its
        // old floating checkout/ref controls dissolve into the session footer.
        // Non-Git sessions grow the slot continuously from zero.
        let session_chrome = 1.0 - new_thread_chrome;
        let bottom_slot = if has_new_thread_git_selectors || self.dock_frame.is_some() {
            1.0
        } else {
            session_chrome
        };
        let container = if bottom_slot > 0.0 {
            let footer = (session_chrome_opacity > 0.0).then(|| {
                self.pickers
                    .update(cx, |pickers, cx| pickers.render_footer(cx))
            });
            let usage = self.state.read(cx).context_usage;
            container.child(
                div()
                    .w_full()
                    .h(px(SESSION_FOOTER_HEIGHT * bottom_slot))
                    .mt(px(-Theme::SPACE_SM * (1.0 - bottom_slot)))
                    .mb(px(-Theme::SPACE_SM * bottom_slot))
                    .relative()
                    .when(new_thread_chrome_opacity > 0.0, |slot| {
                        slot.child(
                            div()
                                .absolute()
                                .inset_0()
                                .px(px(10.0))
                                .flex()
                                .items_center()
                                .opacity(new_thread_chrome_opacity)
                                .children(new_thread_git_selectors),
                        )
                    })
                    .when(session_chrome_opacity > 0.0, |slot| {
                        slot.child(
                            div()
                                .absolute()
                                .inset_0()
                                .w_full()
                                .h(px(SESSION_FOOTER_HEIGHT))
                                .flex()
                                .items_center()
                                .opacity(session_chrome_opacity)
                                .child(div().flex_1().min_w_0().children(footer.flatten()))
                                .children(crate::context_usage::has_window(usage).then(|| {
                                    div().flex_none().pr(px(10.0)).child(
                                        crate::context_usage::render(
                                            usage,
                                            self.state.clone(),
                                            &theme,
                                        ),
                                    )
                                })),
                        )
                    }),
            )
        } else {
            container
        };
        // Full-size preview of a staged thumbnail (AttachmentPreviewDialog).
        if let Some(preview) = self.preview.clone() {
            if std::mem::take(&mut self.preview_focus_pending) {
                window.focus(&self.preview_focus, cx);
            }
            let weak = cx.weak_entity();
            return container.child(attachments::lightbox(
                window,
                &preview,
                &self.preview_focus,
                move |window, cx| {
                    // Hand focus back to the input so typing (and the next
                    // Escape) lands where it did before the lightbox opened.
                    if let Ok(input_focus) = weak.update(cx, |this, cx| {
                        this.preview = None;
                        cx.notify();
                        this.input.read(cx).focus_handle.clone()
                    }) {
                        window.focus(&input_focus, cx);
                    }
                },
                cx,
            ));
        }
        container
    }
}
