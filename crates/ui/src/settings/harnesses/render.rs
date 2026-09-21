use super::*;

impl HarnessesPage {
    /// The page-header device switcher (the Accounts pattern): platform glyph
    /// · name · presence dot · sort glyph, opening a dropdown of every
    /// registered device.
    fn render_device_switcher(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        use crate::icons::{self, icon};
        let (mut devices, local_id) = {
            let s = self.state.read(cx);
            (s.devices.clone(), s.local_device_id.clone())
        };
        devices.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        let effective = self.target_device.clone().or_else(|| local_id.clone());
        let selected = devices
            .iter()
            .find(|d| Some(d.id.as_str()) == effective.as_deref())
            .cloned();
        let platform_glyph = |platform: &str| match platform {
            "macos" | "darwin" => icons::LAPTOP,
            "ios" | "android" => icons::SMARTPHONE,
            _ => icons::MONITOR,
        };
        let trigger_glyph = platform_glyph(
            selected
                .as_ref()
                .map(|d| d.platform.as_str())
                .unwrap_or("macos"),
        );
        let trigger_label: SharedString = selected
            .as_ref()
            .map(|d| d.name.clone().into())
            .unwrap_or_else(|| SharedString::from("This device"));
        let emerald = theme.success;
        let open = self.device_menu_open;

        let mut trigger =
            div()
                .id("harnesses-device-switcher")
                .flex_none()
                .h(px(28.0))
                .px(px(8.0))
                .rounded(px(6.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .cursor_pointer()
                .bg(if open {
                    crate::theme::ink(0.06)
                } else {
                    gpui::transparent_black()
                })
                .when(!open, |el| el.hover(|s| s.bg(crate::theme::ink(0.04))))
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, _, _| {
                        this.device_menu_pressed_open = this.device_menu_open;
                    }),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    // A press that found the menu open closes it — never
                    // reopen on the same gesture.
                    let pressed_open = std::mem::take(&mut this.device_menu_pressed_open);
                    this.device_menu_open = !pressed_open && !this.device_menu_open;
                    cx.notify();
                }))
                .child(
                    icon(trigger_glyph)
                        .size(px(16.0))
                        .flex_none()
                        .text_color(theme.text_muted),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(crate::typography::ui_rems(12.5))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(theme.text)
                        .child(trigger_label),
                )
                .child(div().size(px(6.0)).rounded_full().flex_none().bg(
                    if effective == local_id {
                        emerald
                    } else {
                        crate::theme::ink(0.2)
                    },
                ))
                .child(
                    icon(icons::SORT_VERTICAL)
                        .size(px(14.0))
                        .flex_none()
                        .text_color(theme.text_muted.opacity(if open { 0.9 } else { 0.4 })),
                );

        if open {
            let theme = &theme.for_popup();
            let menu = popover::popover_card(theme)
                .w(px(220.0))
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.device_menu_open = false;
                    cx.notify();
                }))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(popover::menu_heading(theme, "Devices"))
                .children(devices.into_iter().enumerate().map(|(ix, d)| {
                    let is_active = Some(d.id.as_str()) == effective.as_deref();
                    let is_local = local_id.as_deref() == Some(d.id.as_str());
                    let glyph = platform_glyph(&d.platform);
                    let name: SharedString = d.name.clone().into();
                    let pick_local = is_local;
                    let pick_id = d.id.clone();
                    popover::menu_row(theme, is_active, format!("harnesses-device-row-{ix}"))
                        .id(("harnesses-device-row", ix))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            // Local device = no passthrough (calls stay direct).
                            let target = (!pick_local).then(|| pick_id.clone());
                            this.set_target_device(target, cx);
                        }))
                        .child(
                            icon(glyph)
                                .size(px(16.0))
                                .flex_none()
                                .text_color(theme.text_muted),
                        )
                        .child(div().flex_1().min_w_0().truncate().child(name))
                        .when(is_local, |el| {
                            el.child(
                                div()
                                    .flex_none()
                                    .text_size(crate::typography::ui_rems(10.5))
                                    .text_color(theme.text_muted)
                                    .child(SharedString::from("You")),
                            )
                        })
                        .child(
                            div()
                                .size(px(6.0))
                                .rounded_full()
                                .flex_none()
                                .bg(if is_local {
                                    emerald
                                } else {
                                    crate::theme::ink(0.2)
                                }),
                        )
                }))
                .into_any_element();
            trigger = trigger.child(popover::anchored_menu("harnesses-device-menu", menu, None));
        }
        trigger.into_any_element()
    }

    fn rows(&self, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let theme = Theme::of(cx).clone();
        let Loadable::Ready(list) = &self.harnesses else {
            return Vec::new();
        };
        let descriptors = visible_harnesses(list);
        let enabled_count = descriptors.iter().filter(|d| descriptor_enabled(d)).count();
        descriptors
            .into_iter()
            .enumerate()
            .map(|(ix, descriptor)| {
                let harness = descriptor.id;
                let installed = descriptor.installed;
                let enabled = descriptor_enabled(&descriptor);
                // The one enabled harness left can't be switched off — the
                // composer needs something to run — but only when it could
                // actually run: an uninstalled last harness stays togglable
                // (its hint says to turn it off) and the composer handles the
                // resulting empty set (mirrors the engine guard).
                let last_enabled = enabled && enabled_count == 1 && installed;
                let signing_in = self
                    .sign_in
                    .as_ref()
                    .filter(|sign_in| sign_in.harness == harness);
                let sign_in_failure = self
                    .sign_in_failure
                    .as_ref()
                    .filter(|failure| failure.harness == harness);
                let sign_in_cancellable = signing_in
                    .is_some_and(|sign_in| !matches!(sign_in.phase, SignInPhase::Enabling));
                // Turning OFF never needs the CLI (a default-on agent the
                // user doesn't want must not be stuck on because it isn't
                // installed); turning ON still does.
                let interactive = signing_in.is_none()
                    && sign_in_failure.is_none()
                    && !last_enabled
                    && (enabled || installed);
                let (icon_path, tint) = crate::pickers::harness_brand_icon(harness);
                let mut meta: Vec<gpui::AnyElement> = vec![
                    div()
                        .child(SharedString::from(blurb(harness)))
                        .into_any_element(),
                ];
                if let Some(sign_in) = signing_in {
                    meta.push(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .child(crate::loaders::mini_mono_spinner(
                                format!("harness-setup-spinner-{harness:?}"),
                                1.5,
                                theme.text_muted,
                                cx.entity_id(),
                                cx,
                            ))
                            .child(SharedString::from(
                                sign_in
                                    .message
                                    .clone()
                                    .unwrap_or_else(|| sign_in.phase.pending_label().into()),
                            ))
                            .into_any_element(),
                    );
                }
                if let Some(failure) = sign_in_failure {
                    meta.push(
                        div()
                            .text_color(theme.danger_muted.opacity(0.9))
                            .child(SharedString::from(format!(
                                "{} — {}",
                                failure.phase.failure_label(),
                                failure.message
                            )))
                            .into_any_element(),
                    );
                }
                if !installed {
                    meta.push(
                        div()
                            .text_color(theme.warning_muted.opacity(0.9))
                            .child(SharedString::from(if enabled {
                                format!(
                                    "{} CLI not installed — turn it off or install it",
                                    cli_name(harness)
                                )
                            } else {
                                format!("Install the {} CLI to enable", cli_name(harness))
                            }))
                            .into_any_element(),
                    );
                }
                // widgets::row_tile with the brand tint honored (the Claude
                // mark keeps its orange, like the picker rail).
                let tile = div()
                    .flex_none()
                    .size(px(36.0))
                    .rounded(px(10.0))
                    .border_1()
                    .border_color(theme.border)
                    .bg(crate::theme::ink(0.03))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        crate::icons::icon(icon_path)
                            .size(px(16.0))
                            .text_color(tint.unwrap_or(theme.text_muted)),
                    );
                widgets::card_row(&theme, ix == 0)
                    .id(("harness-row", ix))
                    .when(!installed, |el| el.opacity(0.55))
                    .when(signing_in.is_some(), |el| el.opacity(0.65))
                    .child(tile)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(widgets::row_title(&theme, descriptor.name.clone()))
                            .child(widgets::meta_line(&theme, meta)),
                    )
                    .when(sign_in_cancellable, |el| {
                        el.child(
                            widgets::ghost_action(&theme)
                                .id(("harness-cancel-sign-in", ix))
                                .hover(|s| widgets::ghost_hover(&theme, s))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.cancel_sign_in(cx);
                                }))
                                .child(SharedString::from("Cancel")),
                        )
                    })
                    .when(sign_in_failure.is_some(), |el| {
                        el.child(
                            widgets::ghost_action(&theme)
                                .id(("harness-retry-sign-in", ix))
                                .hover(|s| widgets::ghost_hover(&theme, s))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.start_sign_in(harness, cx);
                                }))
                                .child(SharedString::from("Retry")),
                        )
                    })
                    .child(
                        widgets::toggle_switch(&theme, enabled)
                            .id(("harness-toggle", ix))
                            .when(!interactive, |el| el.opacity(0.35))
                            .when(interactive, |el| {
                                el.cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.toggle(harness, !enabled, cx);
                                    }))
                            }),
                    )
                    .into_any_element()
            })
            .collect()
    }
    fn on_scroll_hovered(&mut self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if self.scroll.set_list_hovered(*hovered) {
            cx.notify();
        }
    }
}

impl popover::ScrollRailHost for HarnessesPage {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

impl Render for HarnessesPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let body: gpui::AnyElement = match &self.harnesses {
            Loadable::Idle | Loadable::Loading => widgets::section_card(&theme)
                .p(px(16.0))
                .child(popover::skeleton_rows(
                    "harnesses-skeleton",
                    &theme,
                    4,
                    cx.entity_id(),
                    cx,
                ))
                .into_any_element(),
            Loadable::Error(message) => {
                let message = message.clone();
                div()
                    .child(widgets::error_strip(&theme, message))
                    .child(
                        widgets::ghost_action(&theme)
                            .id("harnesses-retry")
                            .mt(px(8.0))
                            .hover(|s| widgets::ghost_hover(&theme, s))
                            .on_click(cx.listener(|page, _, _, cx| {
                                page.load(cx);
                                cx.notify();
                            }))
                            .child(SharedString::from("Retry")),
                    )
                    .into_any_element()
            }
            Loadable::Ready(_) => {
                let rows = self.rows(cx);
                widgets::section_card(&theme)
                    .children(rows)
                    .into_any_element()
            }
        };
        let error = self
            .error
            .clone()
            .map(|message| widgets::error_strip(&theme, message).into_any_element());
        let switcher = self.render_device_switcher(&theme, cx);
        let titles = self.render_titles(&theme, cx);
        let scrollbar = popover::rail(self, "harnesses-page-scrollbar", &theme, cx);

        div()
            .id("harnesses-page-host")
            .relative()
            .size_full()
            .on_hover(cx.listener(Self::on_scroll_hovered))
            .child(
                div()
                    .id("harnesses-page")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll.scroll)
                    .child(
                        widgets::page_column()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .child(widgets::page_header(&theme, "Agents", None))
                                    .child(switcher),
                            )
                            .child(
                                widgets::page_subtitle(
                                    &theme,
                                    "Choose which coding agents the composer offers. The setting is per \
                                     device — switch devices in the header. Agents whose CLI isn't \
                                     installed on a device can't be enabled there.",
                                )
                                .max_w(px(512.0))
                                .line_height(px(20.0)),
                            )
                            .children(error)
                            .child(body)
                            .child(titles),
                    ),
            )
            .children(scrollbar)
    }
}
