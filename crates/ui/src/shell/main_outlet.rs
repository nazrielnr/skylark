//! Main conversation outlet rendering, new-thread background, and status strip.

use chrono::Utc;
use gpui::{
    div, prelude::*, px, AnyElement, Context, Empty, Entity, IntoElement, SharedString, Window,
};

use crate::files::WorkspacePathDrag;
use crate::icons::{self, icon};
use crate::loaders;
use crate::motion;
use crate::popover;
use crate::settings;
use crate::state::Indicator;
use crate::theme::Theme;
use crate::transcript::Transcript;

use super::{right_tabs::RightTabDrag, Route, Shell};

/// New-thread controls float over the tail of a top-anchored image hero. The
/// hero reaches below the composer, giving its lower mask room to dissolve
/// gradually into the otherwise empty lower canvas.
pub(crate) const NEW_THREAD_BACKGROUND_FROSTED_OPACITY: f32 = 0.84;
pub(crate) const NEW_THREAD_BACKGROUND_VIEWPORT_RATIO: f32 = 0.72;
pub(crate) const NEW_THREAD_BACKGROUND_MAX_HEIGHT: f32 = 760.0;

pub(crate) fn bottom_stack_measurement_matches(
    measured_has_composer: bool,
    expected_has_composer: bool,
) -> bool {
    measured_has_composer == expected_has_composer
}

pub(crate) fn new_thread_background_opacity(is_frost: bool) -> f32 {
    if is_frost {
        NEW_THREAD_BACKGROUND_FROSTED_OPACITY
    } else {
        1.0
    }
}

pub(crate) fn new_thread_background_height(viewport_height: f32) -> f32 {
    (viewport_height.max(0.0) * NEW_THREAD_BACKGROUND_VIEWPORT_RATIO)
        .min(NEW_THREAD_BACKGROUND_MAX_HEIGHT)
}

pub(crate) fn new_thread_background(
    artwork: Option<std::sync::Arc<gpui::RenderImage>>,
    viewport_height: f32,
    hero_width: f32,
    composer_bounds: crate::new_thread_background_mask::SurfaceBounds,
    dissolve: f32,
    opacity: f32,
) -> AnyElement {
    let Some(artwork) = artwork else {
        return Empty.into_any_element();
    };
    let hero_height = new_thread_background_height(viewport_height);
    let dissolve = dissolve.clamp(0.0, 1.0);
    // Image and treatment share a fixed crop and fade together in place.
    // The hero uses the full conversation canvas even while the destination
    // right pane clips it. Navigation must never rescale the artwork.
    div()
        .absolute()
        .top_0()
        .left_0()
        .w(px(hero_width))
        .h(px(hero_height))
        .overflow_hidden()
        .opacity((1.0 - dissolve) * opacity)
        // Alpha resolves into the real canvas, including translucent themes;
        // no theme-colored overlay bleaches or darkens the source pixels.
        .children([false, true].into_iter().map(|cutout| {
            let artwork = artwork.clone();
            let composer_bounds = composer_bounds.clone();
            div()
                .absolute()
                .inset_0()
                .opacity(if cutout {
                    1.0
                } else {
                    crate::new_thread_background_mask::CUTOUT_REVEAL_OPACITY
                })
                .child(
                    gpui::canvas(
                        |_, _, _| {},
                        move |bounds, _, window, _cx| {
                            if let Some(composer) = composer_bounds.get() {
                                crate::new_thread_background_mask::paint(
                                    artwork.clone(),
                                    bounds,
                                    composer,
                                    cutout,
                                    window,
                                );
                            }
                        },
                    )
                    .absolute()
                    .inset_0(),
                )
        }))
        .into_any_element()
}

impl Shell {
    pub(crate) fn render_main(
        &mut self,
        window: &mut Window,
        main_content_width: f32,
        transcript_width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme_owned = Theme::of(cx).clone();
        let theme = &theme_owned;
        let (border, text, faint) = (theme.border, theme.text, theme.text_faint);

        // Settings route: just the section outlet — the section label lives in
        // the unified window titlebar now (render_title_bar). Settings never
        // underlaps: pad below the overlaid titlebar.
        if let Route::Settings(section) = self.route {
            let outlet = self.settings_outlet(section, window, cx);
            return div()
                .flex_1()
                .min_w_0()
                .h_full()
                .pt(px(Theme::TITLEBAR_HEIGHT))
                .flex()
                .flex_col()
                .child(div().flex_1().min_h_0().child(outlet))
                .into_any_element();
        }

        let _ = (text, border);
        let has_selection = self.state.read(cx).selected_chat.is_some();
        let has_spaces = !self.state.read(cx).spaces.is_empty();
        let has_appshots = !self.composer.read(cx).staged_appshots().is_empty();
        let no_project = self.state.read(cx).no_project;
        let transcript_geometry_ready = bottom_stack_measurement_matches(
            self.bottom_stack_has_composer.get(),
            (has_spaces || no_project || has_appshots) && has_selection,
        );
        let ui_settings = settings::current(cx);
        let new_thread_background_setting = ui_settings.new_thread_composer_background;
        let new_thread_background_effect = ui_settings.new_thread_background_effect;
        let frame_time = self.render_time.unwrap_or_else(std::time::Instant::now);
        // Prewarm even in an established thread. Decode/effect work is not
        // contingent on a hero measurement or a navigation gesture.
        let artwork = new_thread_background_setting
            .as_ref()
            .and_then(|background| {
                crate::new_thread_background_effects::prepare(
                    new_thread_background_effect,
                    theme,
                    std::path::Path::new(&background.path),
                    cx,
                )
            });
        let artwork_opacity = self.new_thread_artwork_ready.opacity(
            artwork.as_ref().map(|image| image.id),
            self.reduced_motion,
            frame_time,
        );
        let dock_frame =
            self.composer_dock
                .borrow_mut()
                .tick(has_selection, self.reduced_motion, frame_time);
        if dock_frame.active {
            self.motion_active.set(true);
        }
        self.composer
            .update(cx, |composer, cx| composer.set_dock_frame(dock_frame, cx));
        let composer_width = self.composer_dock.borrow_mut().layout_width(
            main_content_width.min(crate::composer::COMPOSER_MAX_WIDTH),
            self.reduced_motion,
            frame_time,
        );
        self.composer.update(cx, |composer, cx| {
            composer.set_available_width(composer_width, cx)
        });
        let term_h = self.eval_tween(self.terminal_tween, self.terminal_target(cx));
        let new_thread_background_layer = (!has_selection || dock_frame.active).then(|| {
            if artwork.is_some() && artwork_opacity < 1.0 {
                window.request_animation_frame();
            }
            new_thread_background(
                artwork,
                self.viewport_height,
                (self.viewport_width - self.sidebar_now()).max(0.0),
                self.composer.read(cx).surface_bounds(),
                dock_frame.dissolve(),
                artwork_opacity * new_thread_background_opacity(theme.is_frost()),
            )
        });

        // Content outlet: selected chat → transcript; nothing selected → the
        // centered new-thread composition; no spaces at all → the onboarding
        // card. New-chat mode mints the chat id on first send.
        let departing_transcript = !has_selection && dock_frame.transcript() > 0.0;
        if !has_selection && !departing_transcript {
            self.transcript
                .update(cx, |transcript, cx| transcript.finish_route_exit(cx));
        }
        let outlet: AnyElement = if has_selection || departing_transcript {
            div()
                .relative()
                .size_full()
                .overflow_hidden()
                .child(
                    div()
                        .relative()
                        .top(px(8.0 * (1.0 - dock_frame.transcript())))
                        .size_full()
                        .when(departing_transcript, |el| el.w(px(transcript_width)))
                        .opacity(if transcript_geometry_ready || departing_transcript {
                            dock_frame.transcript()
                        } else {
                            0.0
                        })
                        .child(self.transcript.clone()),
                )
                // A departing transcript is visual history, not an active
                // interaction surface bound to the newly blank route.
                .when(departing_transcript, |el| {
                    el.child(div().absolute().inset_0().occlude())
                })
                .into_any_element()
        } else if !has_spaces && !no_project {
            // Onboarding (first boot / after the destructive wipe): no folders
            // to work in yet — one clear affordance.
            let _ = faint;
            div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .child(motion::fade_in(
                    "no-spaces-canvas",
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .child(
                            icon(icons::ZERON_LOGO)
                                .w(px(41.9))
                                .h(px(48.0))
                                .text_color(theme.text.opacity(0.09)),
                        )
                        .child(
                            div()
                                .mt(px(24.0))
                                .text_size(crate::typography::ui_rems(16.0))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(theme.text)
                                .child(SharedString::from("Add a project to get started")),
                        )
                        .child(
                            div()
                                .mt(px(6.0))
                                .text_size(crate::typography::ui_rems(13.0))
                                .text_color(theme.text_muted.opacity(0.7))
                                .child(SharedString::from(
                                    "A project is a folder on one of your devices.",
                                )),
                        )
                        .child(
                            popover::btn_primary(&theme_owned, "Add a project")
                                .id("onboarding-add-space")
                                .mt(px(20.0))
                                .on_click(cx.listener(|this, _, _, cx| this.open_add_space(cx))),
                        ),
                ))
                .into_any_element()
        } else {
            Empty.into_any_element()
        };

        let status = self.render_status_strip(cx);
        // Attachment dropzone over the ENTIRE conversation column (transcript
        // + composer, not just the pill). OS images keep using the upload
        // pipeline; workspace files/directories and file tabs become the same
        // projected file-mention chips the composer already understands.
        // The veil itself uses typed `drag_over` styles below. Do not cache
        // drag presence in shell state: the platform's `FileDrop::Exited`
        // clears GPUI's external payload without sending one last mouse-move,
        // so a cached bit can survive and reappear during an unrelated drag
        // such as a pane resize.
        div()
            .id("chat-dropzone")
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .on_drop(cx.listener(|this, paths: &gpui::ExternalPaths, _, cx| {
                let paths = paths.paths().to_vec();
                this.composer
                    .update(cx, |composer, cx| composer.add_paths(paths, cx));
                cx.notify();
            }))
            .on_drop::<WorkspacePathDrag>(cx.listener(
                |this, payload: &WorkspacePathDrag, window, cx| {
                    this.composer.update(cx, |composer, cx| {
                        composer.add_workspace_path(&payload.path, payload.is_directory, window, cx)
                    });
                    cx.notify();
                },
            ))
            .on_drop::<RightTabDrag>(cx.listener(|this, payload: &RightTabDrag, window, cx| {
                if let Some(path) = &payload.workspace_path {
                    this.composer.update(cx, |composer, cx| {
                        composer.add_workspace_path(&path.path, path.is_directory, window, cx)
                    });
                }
                cx.notify();
            }))
            // The hero is deliberately outside the transcript EdgeFade below:
            // it must paint under the overlaid titlebar instead of becoming
            // fully transparent across the titlebar's inset band.
            .children(new_thread_background_layer)
            .child(
                // Full-height underlay: the transcript viewport spans the
                // whole column, scrolling UNDER the titlebar above and the
                // composer stack below. The per-glyph EdgeFade (glass-safe,
                // same as the sidebar's) spans the full column with
                // ASYMMETRIC bands sized to the chrome: content is opaque at
                // the chrome's inner edge and fades to zero at the window
                // edge — visible mid-fade through the glass chrome it slides
                // under. Always on (the resting paddings keep pinned content
                // out of the bands, and gating on measured scroll state left
                // the top unfaded for one frame on session switch — user
                // report). The jump pill floats outside the fade scope,
                // anchored above the measured stack.
                {
                    // The terminal dock is NOT glass the transcript may slide
                    // under: with the dock's translucent fill, transcript text
                    // ghosted through the grid (user report). The underlay
                    // ends at the dock's top instead, riding the same height
                    // tween the dock animates with; `stack_h` below is only
                    // the chrome that still overlaps the transcript (status
                    // strip + composer).
                    let stack_h = (self.bottom_stack.get() - term_h).max(0.0);
                    // Opaque from the composer PILL's top (the reserved
                    // status strip above it is empty air), zero at the
                    // underlay's bottom edge.
                    let bottom_band = (stack_h - Theme::STATUS_STRIP_HEIGHT).max(1.0);
                    div().absolute().inset_0().bottom(px(term_h)).child(
                        crate::edge_fade::edge_faded(
                            Theme::TRANSCRIPT_FADE_BAND,
                            true,
                            true,
                            div().size_full().child(outlet),
                        )
                        // Fully faded BY the titlebar's bottom edge (the
                        // title text is opaque — overlap read as collision),
                        // ramping in the band just below it.
                        .inset_top(Theme::TITLEBAR_HEIGHT)
                        .band_top(Theme::TRANSCRIPT_FADE_BAND)
                        .band_bottom(bottom_band),
                    )
                },
            )
            // The glass chrome stack, floating over the transcript's bottom:
            // reserved status strip (h-6, the WorkingIndicator — the composer
            // below never shifts), composer, terminal dock. A paint-time
            // canvas measures the stack for next frame's fade inset and
            // transcript clearance. The flex_1 spacer has no id/listeners, so
            // pointer + wheel events over it fall through to the list below.
            .child(div().flex_1().min_h_0())
            .child({
                let measured = self.bottom_stack.clone();
                let measured_has_composer = self.bottom_stack_has_composer.clone();
                let contains_composer = (has_spaces || no_project || has_appshots) && has_selection;
                let composer = self.composer.clone();
                div()
                    .flex_none()
                    .relative()
                    .flex()
                    .flex_col()
                    .child(
                        gpui::canvas(
                            move |bounds, window, cx| {
                                // Reserve the destination footprint, never the animated height.
                                let next_height = f32::from(bounds.size.height)
                                    + composer.read(cx).dock_clearance_correction();
                                let changed = (measured.get() - next_height).abs() > 0.5
                                    || measured_has_composer.get() != contains_composer;
                                measured.set(next_height);
                                measured_has_composer.set(contains_composer);
                                if changed {
                                    window.request_animation_frame();
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .child(status)
                    .when(has_spaces || no_project || has_appshots, |el| {
                        let composer_opacity = self.composer_dock.borrow().opacity();
                        el.child(crate::composer_dock::docked_composer(
                            div()
                                .id("persistent-composer")
                                .relative()
                                .w(px(composer_width))
                                .opacity(composer_opacity)
                                .mx_auto()
                                .child(self.composer.clone())
                                .children(if has_selection {
                                    self.render_jump_to_bottom(cx)
                                } else {
                                    None
                                }),
                            self.composer_dock.clone(),
                            self.viewport_height,
                            self.reduced_motion,
                            frame_time,
                        ))
                    })
                    .child(self.render_terminal_container(window, cx))
            })
            .child(
                div()
                    .id("attachment-drop-overlay")
                    .absolute()
                    .inset_0()
                    .opacity(0.0)
                    .bg(theme.scrim().opacity(0.4 / 0.6))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(crate::typography::ui_rems(13.0))
                    .text_color(theme.text)
                    // GPUI matches these styles against the active payload's
                    // concrete TypeId. Resize markers therefore cannot reveal
                    // this overlay, even after an external drag exits without
                    // another move event.
                    .drag_over::<gpui::ExternalPaths>(|style, _, _, _| style.opacity(1.0))
                    .drag_over::<WorkspacePathDrag>(|style, _, _, _| style.opacity(1.0))
                    .drag_over::<RightTabDrag>(|style, tab, _, _| {
                        if tab.workspace_path.is_some() {
                            style.opacity(1.0)
                        } else {
                            style
                        }
                    })
                    .child("Drop to attach"),
            )
            .into_any_element()
    }

    /// The "↓ Scroll to bottom" pill (round-9 §3): a LABELED rounded-full
    /// chip — down-arrow glyph + 13px label on a near-opaque raised surface
    /// with a hairline — horizontally centered over the transcript column and
    /// floating six pixels above the composer. It shares the composer's
    /// measured dock transform and paints after it, outside the transcript fade.
    pub(crate) fn render_jump_to_bottom(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.transcript.read(cx).jump_button_shown() {
            return None;
        }
        Some(
            div()
                .absolute()
                // Share the composer's measured translation, not its final
                // bottom-stack target. Paint after the composer so it cannot
                // pass over this control during docking.
                .top(px(-36.0))
                .left_0()
                .right(px(10.0))
                .flex()
                .justify_center()
                .child(self.jump_pill("jump-to-bottom", "jump-pill", self.transcript.clone(), cx))
                .into_any_element(),
        )
    }

    /// The jump pill itself — shared between the conversation overlay and
    /// the subagent pane so both read as one control. `anim_key`/`hover_key`
    /// must be distinct per instance (they key global animation state).
    ///
    /// Use the shared popover tint and blur, with a separate hover wash so
    /// the floating control retains the same glass surface in either theme.
    pub(crate) fn jump_pill(
        &self,
        anim_key: &'static str,
        hover_key: &'static str,
        transcript: Entity<Transcript>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::of(cx).for_popup();
        let glass = theme.is_frost();
        let base = if glass {
            popover::surface_bg(&theme)
        } else {
            motion::hover_blend(hover_key, theme.surface_raised, theme.surface_raised_hover)
        };
        let wash = if glass {
            motion::hover_blend(hover_key, gpui::transparent_black(), theme.glass_hover())
        } else {
            gpui::transparent_black()
        };
        let pill = div()
            .id(anim_key)
            .h(px(30.0))
            .rounded_full()
            .border_1()
            .border_color(theme.border)
            .when(!glass, |el| el.shadow_md())
            .cursor_pointer()
            .bg(base)
            .on_hover(motion::hover_listener(hover_key))
            .on_click(cx.listener(move |_, _, _, cx| {
                transcript.update(cx, |transcript, cx| transcript.jump_to_bottom(cx));
            }))
            .child(
                // The hover wash rides an inner full-height layer so it
                // composites over the tint (a div has one bg).
                div()
                    .h_full()
                    .rounded_full()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .pl(px(11.0))
                    .pr(px(13.0))
                    .bg(wash)
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(13.0))
                            .text_color(theme.text_muted)
                            .child(SharedString::from("↓")),
                    )
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(13.0))
                            .text_color(theme.text)
                            .child(SharedString::from("Scroll to bottom")),
                    ),
            );
        // Frost OUTSIDE the entry animation (the composer pill's exact
        // composition): one scene layer — blur, then the pill's quads, then
        // glyphs — so the pill always composes over the transcript content
        // scrolling under it, and never loses its washes to the kind-sorted
        // draw order (frost.rs module docs).
        crate::frost::frosted(
            15.0,
            crate::frost::MENU_BLUR,
            motion::dialog_in(anim_key, pill),
        )
        .into_any_element()
    }

    /// Working indicator strip: gradient spinner + rotating flavour word (7s,
    /// seeded per chat) + elapsed, staleness-gated via [`Indicator`]; falls back
    /// to a "Sending…" bridge and then the engine mode line.
    pub(crate) fn render_status_strip(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let now = Utc::now();
        let state = self.state.read(cx);

        // Aligned with the composer column: centered, same max width, small
        // inner gutter (zeron's `mx-auto h-6 max-w-3xl px-2`).
        let strip = div()
            .h(px(Theme::STATUS_STRIP_HEIGHT))
            .flex_none()
            .w_full()
            .max_w(px(768.0))
            .mx_auto()
            .flex()
            .items_center()
            .gap(px(Theme::SPACE_SM))
            .px(px(Theme::SPACE_LG + 8.0))
            .text_size(crate::typography::ui_rems(11.0));

        let Some(chat_id) = state.selected_chat.clone() else {
            return strip.into_any_element();
        };
        let indicator = state.indicator_for(&chat_id, now);
        // Timer base: the freshest of the session row's turn start and the
        // in-flight send. During the send→ack window the row (if any) still
        // carries the PREVIOUS turn's start, and using it opened the timer at
        // the old turn's elapsed instead of 0:00.
        let started = state
            .session_for(&chat_id)
            .and_then(|s| s.started_at)
            .into_iter()
            .chain(state.pending_send_started(&chat_id, now))
            .max();
        let elapsed_secs = started
            .map(|t| now.signed_duration_since(t).num_seconds().max(0))
            .unwrap_or(0);
        let sending = self.composer.read(cx).is_sending();

        // Unused here since the Working loader moved into the transcript
        // (its trailer computes its own elapsed).
        let _ = elapsed_secs;
        match indicator {
            // The working loader lives in the TRANSCRIPT now, under the
            // streaming reply (user request) — the strip stays empty (its
            // reserved height still steadies the composer).
            Indicator::Working => strip.into_any_element(),
            // No label: the QuestionPanel right below IS the awaiting-input
            // surface — a strip caption above it was redundant (user request).
            Indicator::AwaitingInput => strip.into_any_element(),
            Indicator::Errored => strip
                .text_color(theme.danger)
                .child(SharedString::from("Run failed"))
                .into_any_element(),
            Indicator::None if sending => strip
                .child(loaders::gradient_spinner(
                    "sending-indicator",
                    &theme,
                    2.5,
                    cx.entity_id(),
                    cx,
                ))
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(12.0))
                        .text_color(theme.text_muted)
                        .child(SharedString::from("Sending…")),
                )
                .into_any_element(),
            Indicator::None => strip.into_any_element(),
        }
    }
}
