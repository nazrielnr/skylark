use gpui::{
    AnyElement, Context, Focusable as _, KeyDownEvent, SharedString,
    Window, div, prelude::*, px,
};

use crate::motion;
use crate::popover::{self, Loadable};
use crate::theme::Theme;
use super::config::{CheckoutKind, PickerKind};
use super::model_catalog::*;
use super::Pickers;

pub(crate) const FOOTER_CHIP_RADIUS: f32 = 6.0;

/// Both sides of the composer handoff share one leading-aligned workspace
/// cluster. Available width belongs after the pair, never between its labels.
pub(crate) fn workspace_footer_row() -> gpui::Div {
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(4.0))
}


impl Pickers {
    // ---- render ----

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn trigger_chip(
        &self,
        kind: PickerKind,
        label: SharedString,
        set: bool,
        chip_icon: Option<(&'static str, Option<gpui::Hsla>)>,
        // The chip never collapses while identity resolves (user report):
        // `icon_loading` swaps the brand slot for the pixel-glyph loader
        // (harness unknown), `label_loading` swaps the text for a ghost bar
        // (model unknown).
        icon_loading: bool,
        label_loading: bool,
        suffix: Option<(SharedString, Option<gpui::Hsla>)>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let id: &'static str = match kind {
            PickerKind::Branch => "picker-branch",
            PickerKind::Checkout => "picker-checkout",
            PickerKind::HarnessModel => "picker-model",
            PickerKind::Space => "picker-space",
            PickerKind::Device => "picker-device",
        };
        let open = self.open_kind() == Some(kind);
        // Ghost pill (zeron composer/styles.tsx `pill`): `h-8 rounded-lg px-2.5
        // gap-1.5 text-[12px] font-medium text-muted-foreground`, icons size-4,
        // hover/open wash — no border, no caret; the actions row stays quiet.
        div()
            .id(id)
            .h(px(32.0))
            .max_w(px(248.0))
            // Shrinkable under row pressure — four footer chips share one
            // line; without min_w_0 they overflowed and painted overlapped.
            .min_w_0()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .px(px(10.0))
            .rounded_full()
            .text_size(crate::typography::ui_rems(12.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(motion::hover_blend(
                id,
                if set {
                    theme.text.opacity(0.9)
                } else {
                    theme.text_muted
                },
                theme.text,
            ))
            .bg(if open {
                theme.element_hover
            } else {
                motion::hover_blend(id, gpui::transparent_black(), theme.element_hover)
            })
            .on_hover(motion::hover_listener(id))
            .cursor_pointer()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, _, _| {
                    this.open.note_trigger_press_matching(|open| *open == kind)
                }),
            )
            .on_click(cx.listener(move |this, _, window, cx| this.toggle(kind, window, cx)))
            .when(icon_loading, |el| {
                el.child(div().flex_none().child(crate::loaders::mini_glyph_spinner(
                    "picker-chip-loader",
                    2.0,
                    theme.glyph,
                    cx.entity_id(),
                    cx,
                )))
            })
            .when_some(
                (!icon_loading).then_some(chip_icon).flatten(),
                |el, (path, tint)| {
                    el.child(
                        crate::icons::icon(path)
                            .size(px(16.0))
                            .text_color(tint.unwrap_or(theme.text_muted)),
                    )
                },
            )
            .when(label_loading, |el| {
                el.child(popover::skeleton_bar(56.0, cx.entity_id(), cx))
            })
            .when(!label_loading, |el| {
                el.child(div().min_w_0().truncate().child(label))
            })
            // The effort half of the combined model+effort chip (and the space
            // chip's "@ device" tag): muted, no icon — one button, two tones.
            // `tint` overrides the muted tone (the offline warning). Under row
            // pressure the suffix yields FIRST (large shrink factor) so the
            // model name — the run's identity — truncates last.
            .when_some(suffix, |el, (suffix, tint)| {
                el.child(
                    div()
                        .flex_shrink(1000.0)
                        .min_w_0()
                        .truncate()
                        .text_color(tint.unwrap_or(theme.text_muted.opacity(0.7)))
                        .child(suffix),
                )
            })
    }

    /// A footer-row trigger (t3code ghost `Button size="xs"`): leading icon,
    /// truncating label, trailing chevron — smaller and quieter than the
    /// in-pill chips.
    pub(crate) fn footer_chip(
        &self,
        kind: PickerKind,
        id: &'static str,
        icon_path: &'static str,
        label: SharedString,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let open = self.open_kind() == Some(kind);
        div()
            .id(id)
            .h(px(20.0))
            .max_w(px(280.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .px(px(8.0))
            .rounded_full()
            .text_size(crate::typography::ui_rems(12.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(motion::hover_blend(
                id,
                theme.text_muted.opacity(0.7),
                theme.text.opacity(0.8),
            ))
            .bg(if open {
                theme.element_hover
            } else {
                motion::hover_blend(id, gpui::transparent_black(), theme.element_hover)
            })
            .on_hover(motion::hover_listener(id))
            .cursor_pointer()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, _, _| {
                    this.open.note_trigger_press_matching(|open| *open == kind)
                }),
            )
            .on_click(cx.listener(move |this, _, window, cx| this.toggle(kind, window, cx)))
            .child(
                crate::icons::icon(icon_path)
                    .size(px(12.0))
                    .flex_none()
                    .text_color(theme.text_muted.opacity(0.7)),
            )
            .child(div().min_w_0().truncate().child(label))
            .child(
                crate::icons::icon(crate::icons::ALT_ARROW_DOWN)
                    .size(px(12.0))
                    .flex_none()
                    .text_color(theme.text_muted.opacity(0.5)),
            )
    }

    /// A read-only footer label (locked sessions — t3code's
    /// `resolveLockedWorkspaceLabel` span).
    pub(crate) fn footer_label(icon_path: &'static str, label: SharedString, theme: &Theme) -> gpui::Div {
        div()
            .h(px(20.0))
            // Four of these share one row now (device, project, checkout,
            // ref): cap each early and let them SHRINK (`min_w_0`) — without
            // it the clusters overflowed into each other and the labels
            // painted overlapped (user report).
            .max_w(px(160.0))
            .min_w_0()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .px(px(8.0))
            .text_size(crate::typography::ui_rems(12.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(theme.text_muted.opacity(0.6))
            .child(
                crate::icons::icon(icon_path)
                    .size(px(12.0))
                    .text_color(theme.text_muted.opacity(0.6)),
            )
            .child(div().min_w_0().truncate().child(label))
    }

    /// New-session destination controls. Machine and project form the
    /// original chip-only cluster floating above the composer's trailing edge.
    pub fn render_new_thread_target_selectors(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let closing = self.open.closing_since();
        let mut overlay: Option<(PickerKind, AnyElement)> = match self.mounted_kind() {
            Some(PickerKind::Space) => {
                let content = self.render_space_popover(cx);
                Some((PickerKind::Space, self.popover_frame(280.0, content, cx)))
            }
            Some(PickerKind::Device) => {
                let content = self.render_device_popover(cx);
                Some((PickerKind::Device, self.popover_frame(224.0, content, cx)))
            }
            _ => None,
        };
        let (device_label, project_label, offline) = {
            let state = self.state.read(cx);
            let device_id = state.effective_device_id();
            let device_label: SharedString = device_id
                .as_deref()
                .and_then(|id| state.device_name(id))
                .map(str::to_string)
                .unwrap_or_else(|| "This device".to_string())
                .into();
            let offline = device_id
                .as_deref()
                .is_some_and(|id| !state.device_online(id, chrono::Utc::now()));
            let project_label: SharedString = state
                .selected_space_row()
                .map(|s| s.display_name().to_string())
                .unwrap_or_else(|| "No project".to_string())
                .into();
            (device_label, project_label, offline)
        };
        let device_chip = self
            .footer_chip(
                PickerKind::Device,
                "picker-device",
                crate::icons::MONITOR,
                device_label,
                &theme,
                cx,
            )
            .when(offline, |el| el.text_color(theme.warning.opacity(0.8)));
        let project_chip = self.footer_chip(
            PickerKind::Space,
            "picker-project",
            crate::icons::FOLDER,
            project_label,
            &theme,
            cx,
        );
        div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.0))
            .child(attach_overlay(
                device_chip,
                &mut overlay,
                PickerKind::Device,
                "device-popover",
                closing,
            ))
            .child(attach_overlay_end(
                project_chip,
                &mut overlay,
                PickerKind::Space,
                "project-popover",
                closing,
            ))
            .into_any_element()
    }

    /// New-session Git controls. Checkout mode and branch form the original
    /// chip-only cluster floating below the composer's leading edge.
    pub fn render_new_thread_git_selectors(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let git = self
            .state
            .read(cx)
            .selected_space_row()
            .is_some_and(|space| space.git_detected);
        if !git {
            return None;
        }
        self.ensure_refs(false, cx);
        let theme = Theme::of(cx).clone();
        let closing = self.open.closing_since();
        let mut overlay: Option<(PickerKind, AnyElement)> = match self.mounted_kind() {
            Some(PickerKind::Branch) => {
                let content = self.render_branch_popover(cx);
                Some((PickerKind::Branch, self.popover_frame(320.0, content, cx)))
            }
            Some(PickerKind::Checkout) => {
                let content = self.render_checkout_popover(cx);
                Some((PickerKind::Checkout, self.popover_frame(224.0, content, cx)))
            }
            _ => None,
        };
        let kind_icon = match (self.config.checkout, self.selected_ref_worktree().is_some()) {
            (CheckoutKind::Local, false) => crate::icons::FOLDER,
            _ => crate::icons::FOLDER_WITH_FILES,
        };
        let checkout_chip = self.footer_chip(
            PickerKind::Checkout,
            "picker-checkout",
            kind_icon,
            SharedString::from(self.checkout_label()),
            &theme,
            cx,
        );
        let branch_chip = self.footer_chip(
            PickerKind::Branch,
            "picker-branch",
            crate::icons::GIT_BRANCH,
            self.ref_label(),
            &theme,
            cx,
        );
        Some(
            workspace_footer_row()
                .child(attach_overlay_below(
                    checkout_chip,
                    &mut overlay,
                    PickerKind::Checkout,
                    "checkout-popover",
                    closing,
                ))
                .child(attach_overlay_below(
                    branch_chip,
                    &mut overlay,
                    PickerKind::Branch,
                    "branch-popover",
                    closing,
                ))
                .into_any_element(),
        )
    }

    /// The composer footer row: checkout-kind + ref, LEFT-aligned, only when
    /// the picked (or session's) project has git. New sessions use the floating
    /// chip clusters; sessions name their target in the titlebar.
    pub fn render_footer(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = Theme::of(cx).clone();
        // A selected chat whose workspace row hasn't synced yet (the moment
        // right after send mints it) still renders the DRAFT footer — the
        // values are identical, so the toolbar never blinks through a
        // half-empty locked state.
        let (space, session, change_request) = {
            let state = self.state.read(cx);
            let space = state.selected_space_row().cloned();
            let session = state
                .selected_chat
                .as_ref()
                .and_then(|_| state.selected_chat_row().cloned());
            let change_request = session
                .as_ref()
                .and_then(|chat| state.change_request_for_chat(chat).cloned());
            (space, session, change_request)
        };
        let row = || {
            // The composer owns the row's animated reveal and negative bottom
            // margin. Keeping that geometry outside this reusable content
            // lets the new-thread route handoff collapse the footer without
            // clipping its controls or changing its steady-state spacing.
            // `w_full` is load-bearing: without it the canvas layout sizes
            // the row to CONTENT, and the left cluster's flex_1 (basis 0)
            // collapsed to zero width — both clusters painted from the same
            // origin, chips overlapping (user report).
            workspace_footer_row().px(px(10.0))
        };

        if let Some(chat) = &session {
            // Sessions never move: read-only checkout-kind + ref labels,
            // LEFT-aligned, only when the session's project has git. The
            // target (project @ device) lives in the titlebar now.
            let Some(space) = space.as_ref().filter(|s| s.git_detected) else {
                return None;
            };
            let is_worktree = chat.cwd.as_deref().is_some_and(|cwd| cwd != space.path);
            let (icon_path, label) = if is_worktree {
                (crate::icons::FOLDER_WITH_FILES, "Worktree")
            } else {
                (crate::icons::FOLDER, "Local checkout")
            };
            // Keep the same reading order and leading edge as the draft.
            let left = div()
                .flex()
                .flex_row()
                .items_center()
                .min_w_0()
                .child(Self::footer_label(
                    icon_path,
                    SharedString::from(label),
                    &theme,
                ));
            let right = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.0))
                .min_w_0()
                .child(Self::footer_label(
                    crate::icons::GIT_BRANCH,
                    chat.branch
                        .clone()
                        .map(SharedString::from)
                        .unwrap_or_else(|| SharedString::from("No ref")),
                    &theme,
                ));
            // Checkout + branch stay together. PR and usage form the trailing
            // status group, independently of the branch label's length.
            return Some(
                row()
                    .pr_0()
                    .child(left)
                    .child(right)
                    .child(div().flex_1().min_w_0())
                    .when_some(change_request, |el, summary| {
                        el.child(div().flex_none().child(
                            crate::change_requests::pull_request_badge(
                                "composer-pull-request".into(),
                                summary,
                                crate::change_requests::ChangeRequestBadgeSurface::Composer,
                                &theme,
                            ),
                        ))
                    })
                    .into_any_element(),
            );
        }

        // New-session draft: checkout + ref only, LEFT-aligned (device +
        // project live in the row above the pill now).
        let git = space.as_ref().is_some_and(|s| s.git_detected);
        if !git {
            return None;
        }
        // Refs feed the draft labels — eager + idempotent.
        self.ensure_refs(false, cx);
        let closing = self.open.closing_since();
        let mut overlay: Option<(PickerKind, AnyElement)> = match self.mounted_kind() {
            Some(PickerKind::Branch) => {
                let content = self.render_branch_popover(cx);
                Some((PickerKind::Branch, self.popover_frame(320.0, content, cx)))
            }
            Some(PickerKind::Checkout) => {
                let content = self.render_checkout_popover(cx);
                Some((PickerKind::Checkout, self.popover_frame(224.0, content, cx)))
            }
            // Space/Device popovers mount in the floating row above the pill.
            _ => None,
        };

        let ref_label = self.ref_label();
        let ref_chip = self.footer_chip(
            PickerKind::Branch,
            "picker-branch",
            crate::icons::GIT_BRANCH,
            ref_label,
            &theme,
            cx,
        );
        let kind_icon = match (self.config.checkout, self.selected_ref_worktree().is_some()) {
            (CheckoutKind::Local, false) => crate::icons::FOLDER,
            _ => crate::icons::FOLDER_WITH_FILES,
        };
        let kind_chip = self.footer_chip(
            PickerKind::Checkout,
            "picker-checkout",
            kind_icon,
            SharedString::from(self.checkout_label()),
            &theme,
            cx,
        );
        // Match the floating draft's adjacent checkout/ref pair, including
        // while the newly created session is waiting for its workspace row.
        let left = div()
            .flex()
            .flex_row()
            .items_center()
            .min_w_0()
            .child(attach_overlay(
                kind_chip,
                &mut overlay,
                PickerKind::Checkout,
                "checkout-popover",
                closing,
            ));
        let right = div()
            .flex()
            .flex_row()
            .items_center()
            .min_w_0()
            .child(attach_overlay(
                ref_chip,
                &mut overlay,
                PickerKind::Branch,
                "branch-popover",
                closing,
            ));
        Some(row().child(left).child(right).into_any_element())
    }

    pub(crate) fn popover_frame(&self, width: f32, content: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).for_popup();
        popover::popover_card(&theme)
            .w(px(width))
            // zeron caps its tallest picker at min(640px, 75vh).
            .max_h(px(640.0))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.on_key_down(event, window, cx)
            }))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if this.is_open() && !this.focus.contains_focused(window, cx) {
                        window.focus(&this.focus, cx);
                    }
                }),
            )
            .on_mouse_down_out(
                cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                    if this.setting_menu.is_some()
                        && this
                            .setting_bounds
                            .is_some_and(|bounds| bounds.contains(&event.position))
                    {
                        return;
                    }
                    this.dismiss(cx);
                    if this.focus.contains_focused(window, cx) {
                        window.blur();
                    }
                }),
            )
            .flex()
            .flex_col()
            .child(content)
            .into_any_element()
    }

    /// [`Self::popover_frame`] without the p-1 inset — the harness/model
    /// picker's rail + list panes bleed to the card edge (zeron
    /// harness-model-picker.tsx `className="w-80 p-0"`).
    pub(crate) fn popover_frame_flush(
        &self,
        width: f32,
        content: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::of(cx).for_popup();
        popover::popover_card_flush(&theme)
            .w(px(width))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.on_key_down(event, window, cx)
            }))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if this.is_open() && !this.focus.contains_focused(window, cx) {
                        window.focus(&this.focus, cx);
                    }
                }),
            )
            .on_mouse_down_out(
                cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                    if this.setting_menu.is_some()
                        && this
                            .setting_bounds
                            .is_some_and(|bounds| bounds.contains(&event.position))
                    {
                        return;
                    }
                    this.dismiss(cx);
                    if this.focus.contains_focused(window, cx) {
                        window.blur();
                    }
                }),
            )
            .flex()
            .flex_col()
            .child(content)
            .into_any_element()
    }

    pub(crate) fn search_box(&self, theme: &Theme) -> AnyElement {
        popover::search_input_frame(theme, self.search.clone().into_any_element())
            .into_any_element()
    }

    pub(crate) fn retry_row(
        &self,
        id: &'static str,
        message: &str,
        kind: PickerKind,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        popover::error_row(theme, message)
            .child(
                div()
                    .id(id)
                    .px(px(Theme::SPACE_SM))
                    .py(px(3.0))
                    .rounded(px(Theme::CONTROL_RADIUS))
                    .border_1()
                    .border_color(theme.border)
                    .text_color(theme.text)
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.element_hover))
                    .on_click(cx.listener(move |this, _, _, cx| match kind {
                        PickerKind::Branch | PickerKind::Checkout => this.ensure_refs(true, cx),
                        PickerKind::HarnessModel => {
                            this.harnesses = Loadable::Idle;
                            this.models.clear();
                            this.catalog_rev += 1;
                            this.ensure_harnesses(false, cx);
                        }
                        // Projects/devices load nothing; no retry surface exists.
                        PickerKind::Space | PickerKind::Device => {}
                    }))
                    .child(SharedString::from("Retry")),
            )
            .into_any_element()
    }

    /// The virtualized list's plain scroll handle (bounds/offset for the

    /// The scroll handle of whichever picker menu is mounted. The popups are
    /// mutually exclusive: the model list owns its virtualized handle, the
    /// plain-div menus (branch / project / device) share `menu_scroll`.
    /// Keys on the MOUNTED menu, not `open_kind` — popovers keep rendering
    /// through the exit animation, and the rail must keep measuring the
    /// closing menu's own handle, not `menu_scroll`'s idle geometry.
    pub(crate) fn active_menu_scroll(&self) -> gpui::ScrollHandle {
        if self.mounted_kind() == Some(PickerKind::HarnessModel) {
            self.model_scroll_base()
        } else {
            self.menu_scroll.clone()
        }
    }

    /// The list-hover half of the rail treatment; the strip's own hover,
    /// press, drag, and mouse-up listeners come from [`popover::rail`].
    pub(crate) fn on_menu_list_hover(
        &mut self,
        hovered: &bool,
        _window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        if self.menu_bar.set_list_hovered(*hovered) {
            cx.notify();
        }
    }
}

/// The floating-scrollbar treatment for every picker list
/// ([`popover::rail`] folds the note/hide/metrics/render + pointer listeners
/// into one call): one shared rail state, fed by whichever handle
/// [`Pickers::active_menu_scroll`] resolves for the mounted menu.
impl popover::ScrollRailHost for Pickers {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        &mut self.menu_bar
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        Some(self.active_menu_scroll())
    }
}



fn attach_overlay(
    chip: gpui::Stateful<gpui::Div>,
    overlay: &mut Option<(PickerKind, AnyElement)>,
    kind: PickerKind,
    id: &'static str,
    closing: Option<std::time::Instant>,
) -> gpui::Stateful<gpui::Div> {
    if overlay.as_ref().is_some_and(|(k, _)| *k == kind)
        && let Some((_, element)) = overlay.take()
    {
        return chip.child(popover::anchored_menu_above(id, element, closing));
    }
    chip
}

/// Attach the (single) open popover below a selector trigger.
fn attach_overlay_below(
    chip: gpui::Stateful<gpui::Div>,
    overlay: &mut Option<(PickerKind, AnyElement)>,
    kind: PickerKind,
    id: &'static str,
    closing: Option<std::time::Instant>,
) -> gpui::Stateful<gpui::Div> {
    if overlay.as_ref().is_some_and(|(k, _)| *k == kind)
        && let Some((_, element)) = overlay.take()
    {
        return chip.child(popover::anchored_menu_below(id, element, closing));
    }
    chip
}

/// Attach the menu ABOVE and RIGHT-ALIGNED to the trigger (t3code
/// `align="end"` — right-edge controls like the model picker open leftward).
fn attach_overlay_end(
    chip: gpui::Stateful<gpui::Div>,
    overlay: &mut Option<(PickerKind, AnyElement)>,
    kind: PickerKind,
    id: &'static str,
    closing: Option<std::time::Instant>,
) -> gpui::Stateful<gpui::Div> {
    if overlay.as_ref().is_some_and(|(k, _)| *k == kind)
        && let Some((_, element)) = overlay.take()
    {
        return chip
            .relative()
            .child(popover::anchored_menu_above_end(id, element, closing));
    }
    chip
}

impl Render for Pickers {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        // A ZERON_OPEN_PICKER popover never went through `toggle`, so claim
        // its keyboard focus here (re-claim until it sticks — the shell's
        // first-paint fallback focuses the composer after our first render).
        if self.boot_focus_pending {
            match self.open_kind() {
                Some(PickerKind::Branch) => {
                    self.search.update(cx, |input, cx| {
                        input.set_placeholder("Search refs…", cx);
                    });
                    let handle = self.search.read(cx).focus_handle(cx);
                    if handle.is_focused(window) {
                        self.boot_focus_pending = false;
                    } else {
                        window.focus(&handle, cx);
                    }
                }
                Some(_) => {
                    if self.focus.is_focused(window) {
                        self.boot_focus_pending = false;
                    } else {
                        window.focus(&self.focus, cx);
                    }
                }
                None => self.boot_focus_pending = false,
            }
        }

        let focus_on_mount = std::mem::take(&mut self.focus_on_mount) && self.is_open();
        if focus_on_mount {
            // The frame exists even when loading/error/empty content omits
            // the input. Claim it during mount, after any pre-frame shell
            // recovery has handled the old dispatch tree.
            window.focus(&self.focus, cx);
        }
        if self.is_open() {
            let search = self.search.focus_handle(cx);
            let frame = self.focus.clone();
            window.defer(cx, move |window, cx| {
                let has_search = frame.contains(&search, window);
                if focus_on_mount && frame.is_focused(window) && has_search {
                    // Transfer to the filter only once it is actually mounted.
                    window.focus(&search, cx);
                } else if search.is_focused(window) && !has_search {
                    // Loading, empty, and error states can omit the search box.
                    // Keep Escape/arrow keys on the mounted menu in those states.
                    window.focus(&frame, cx);
                }
            });
        }

        // Eager-load the harness catalog + every offered harness's models so
        // the chip reads "Fable 5" (a concrete pick) before any popover
        // opens, and rail switches inside the picker are instant.
        self.ensure_harnesses(false, cx);
        self.prefetch_models(false, cx);
        // A popover opened data-side (ZERON_OPEN_PICKER) never went through
        // `toggle`, so kick its loads here (all ensure_* are idempotent).
        if matches!(
            self.open_kind(),
            Some(PickerKind::Branch) | Some(PickerKind::Checkout)
        ) && matches!(self.refs, Loadable::Idle)
        {
            self.ensure_refs(false, cx);
        }
        // Chip shows the model's display name alone (zeron `modelText`); the
        // harness reads from the brand mark beside it. Never "Default model":
        // before the catalog lands the remembered label (or the configured id)
        // names the pick; the loaded list then resolves it to a concrete row.
        // No-agents state: nothing runnable resolved (and the catalog is
        // loaded, so that's a conclusion, not a loading gap) — the chip says
        // so instead of wearing a brand mark for an agent that can't run.
        let no_agents = self.no_agents_available() && self.effective_harness(cx).is_none();
        let model_label: SharedString = if no_agents {
            SharedString::from("No agents available")
        } else {
            let loaded = self.selected_model(cx).map(|m| m.label.clone());
            let label = loaded.or_else(|| {
                let remembered = self
                    .effective_harness(cx)
                    .and_then(|h| self.defaults.model_for(h));
                match self.effective_model_id(cx) {
                    Some(id) => Some(
                        remembered
                            .filter(|m| m.id == id)
                            .map(|m| m.label.clone())
                            .or_else(|| self.defaults.label_for(id).map(str::to_string))
                            .unwrap_or_else(|| id.to_string()),
                    ),
                    None => remembered.map(|m| m.label.clone()),
                }
            });
            label.map(SharedString::from).unwrap_or_default()
        };
        let catalog_loading = matches!(self.harnesses, Loadable::Idle | Loadable::Loading);
        let models_loading = self.effective_harness(cx).is_some_and(|harness| {
            !matches!(
                self.models.get(&harness),
                Some(Loadable::Ready(_)) | Some(Loadable::Error(_))
            )
        });
        // Harness unknown while the catalog resolves: the pixel-glyph loader
        // instead of guessing a brand mark.
        let chip_icon_loading =
            self.effective_harness(cx).is_none() && !no_agents && catalog_loading;
        // Harness known but nothing names the model yet (fresh install, no
        // remembered pick): a ghost label instead of a bare icon.
        let chip_label_loading =
            !no_agents && model_label.is_empty() && (catalog_loading || models_loading);
        let harness_icon: (&'static str, Option<gpui::Hsla>) = match self.effective_harness(cx) {
            Some(harness) => harness_brand_icon(harness),
            None if no_agents => (crate::icons::TERMINAL, Some(theme.text_muted)),
            None => (
                crate::icons::CLAUDE_MARK,
                Some(crate::icons::claude_brand()),
            ),
        };
        let explicit_options = self.explicit_options(cx);
        let traits_set = traits_summary(
            self.selected_model(cx),
            self.effective_reasoning(cx),
            &explicit_options,
        );
        let traits_active = traits_customized(
            self.selected_model(cx),
            self.effective_reasoning(cx),
            &self.trait_ladder(cx),
            &explicit_options,
        );
        // Render the open popover's body first (mutable borrow), then the
        // chips. Branch/Checkout render in the composer FOOTER row (see
        // `render_footer`), not here.
        let closing = self.open.closing_since();
        let mut overlay: Option<(PickerKind, AnyElement)> = match self.mounted_kind() {
            // Footer-row pickers — their popovers mount down there.
            Some(PickerKind::Branch)
            | Some(PickerKind::Checkout)
            | Some(PickerKind::Space)
            | Some(PickerKind::Device) => None,
            Some(PickerKind::HarnessModel) => {
                let content = self.render_harness_model_popover(cx);
                Some((
                    PickerKind::HarnessModel,
                    // Compact single-harness pane (t3 ModelPickerContent
                    // shrunk to its tabbed layout).
                    self.popover_frame_flush(304.0, content, cx),
                ))
            }
            None => None,
        };

        // The composer places this model chip beside the attachment button.
        // ONE chip for the whole run identity (user request): brand icon +
        // model name, then the joined traits summary ("Medium", "High · 1M ·
        // Fast", "Agent · Balance") as the chip's muted second tone — the
        // run's configuration reads without opening anything, and the suffix
        // brightens only when something departs from its default. No suffix
        // when the model has neither a ladder nor options (e.g. Hermes).
        let chip_suffix = traits_set.map(|summary| {
            (
                SharedString::from(summary),
                traits_active.then(|| theme.text.opacity(0.85)),
            )
        });
        let model_chip = self.trigger_chip(
            PickerKind::HarnessModel,
            model_label,
            true,
            Some(harness_icon),
            chip_icon_loading,
            chip_label_loading,
            chip_suffix,
            &theme,
            cx,
        );
        let new_chat = self.state.read(cx).selected_chat.is_none();
        let entity = cx.entity().downgrade();
        let model_chip = model_chip.relative().child(
            gpui::canvas(
                move |bounds, window, cx| {
                    let available = (f32::from(window.viewport_size().height - bounds.bottom())
                        - 14.0)
                        .max(0.0);
                    let _ = entity.update(cx, |this, cx| {
                        if this.model_space_below != Some(available) {
                            this.model_space_below = Some(available);
                            if new_chat && this.open_kind() == Some(PickerKind::HarnessModel) {
                                cx.notify();
                                window.request_animation_frame();
                            }
                        }
                    });
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0(),
        );
        let model_chip = if new_chat {
            if overlay
                .as_ref()
                .is_some_and(|(kind, _)| *kind == PickerKind::HarnessModel)
                && let Some((_, content)) = overlay.take()
            {
                model_chip.child(popover::anchored_menu_below_end(
                    "model-popover",
                    content,
                    closing,
                ))
            } else {
                model_chip
            }
        } else {
            attach_overlay_end(
                model_chip,
                &mut overlay,
                PickerKind::HarnessModel,
                "model-popover",
                closing,
            )
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            // Shrinkable under row pressure, like the footer chips: the chip's
            // own `min_w_0().truncate()` label/suffix only engage when this
            // cluster is allowed to give up width — `flex_none` here let the
            // labels paint over the attach/send buttons at narrow widths
            // instead of truncating (user report).
            .min_w_0()
            .gap(px(4.0))
            .child(model_chip)
    }
}


/// Catalog for the isolated native screenshot fixture; never used by the app.
#[cfg(feature = "project-palette-fixture")]
impl Pickers {
    pub(crate) fn fixture_model_catalog(&mut self, cx: &mut Context<Self>) {
        if matches!(self.models.get(&HarnessId::Codex), Some(Loadable::Ready(_))) {
            return;
        }
        self.config.harness = Some(HarnessId::Codex);
        self.config.model = Some("gpt-5.4".into());
        self.harnesses = Loadable::Ready(serde_json::from_value(serde_json::json!([
            {"id":"codex","name":"Codex","supportsSteering":true,"steeringMode":"step-boundary","reasoningLevels":[]}
        ])).unwrap());
        self.models.insert(HarnessId::Codex, Loadable::Ready(serde_json::from_value(serde_json::json!([
            {"id":"gpt-5.4","label":"GPT-5.4","description":"For complex coding and reasoning", "reasoningLevels":["low","medium","high","xhigh"], "options":[
                {"id":"context-window","label":"Context window","defaultChoice":"standard","choices":[{"id":"standard","label":"Standard"},{"id":"1m","label":"1M tokens"}]},
                {"id":"service-tier","label":"Service tier","defaultChoice":"auto","choices":[{"id":"auto","label":"Standard"},{"id":"fast","label":"Fast"}]}
            ]},
            {"id":"gpt-5.3-codex","label":"GPT-5.3 Codex","description":"Optimized for agentic coding"},
            {"id":"gpt-5.2","label":"GPT-5.2","description":"General purpose reasoning"},
            {"id":"gpt-5.1-codex-mini","label":"GPT-5.1 Codex Mini","description":"Fast, efficient coding"}
        ])).unwrap()));
        self.catalog_rev += 1;
        cx.notify();
    }
}
