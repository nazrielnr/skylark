//! Appearance theme, font picker, and custom-theme library rendering.

use super::*;

impl AppearancePage {
    fn theme_menu(&self, appearance: Appearance) -> &Popup<()> {
        match appearance {
            Appearance::Light => &self.light_theme_menu,
            Appearance::Dark => &self.dark_theme_menu,
        }
    }

    fn theme_menu_mut(&mut self, appearance: Appearance) -> &mut Popup<()> {
        match appearance {
            Appearance::Light => &mut self.light_theme_menu,
            Appearance::Dark => &mut self.dark_theme_menu,
        }
    }

    fn close_theme_menu(&mut self, appearance: Appearance, cx: &mut Context<Self>) {
        if !self.theme_menu_mut(appearance).begin_close() {
            return;
        }
        match appearance {
            Appearance::Light => popover::reap_popup(cx, |page| &mut page.light_theme_menu),
            Appearance::Dark => popover::reap_popup(cx, |page| &mut page.dark_theme_menu),
        }
    }

    pub(super) fn render_font_picker(
        &mut self,
        kind: FontKind,
        theme: &Theme,
        availability: &FontAvailability,
        fixed: SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let slug = kind.slug();
        // The scroll helpers key off `&'static str`, so the ids are spelled
        // out rather than formatted from the slug.
        let (host_id, list_id, rail_id) = match kind {
            FontKind::Ui => (
                "interface-font-host",
                "interface-font-scroll",
                "interface-font-scrollbar",
            ),
            FontKind::Terminal => (
                "terminal-font-host",
                "terminal-font-scroll",
                "terminal-font-scrollbar",
            ),
            FontKind::Code => ("code-font-host", "code-font-scroll", "code-font-scrollbar"),
        };
        let effective = kind.effective(cx);
        let selected = self.selected_font(kind).clone();
        let visible = self.visible_choices(kind, availability, cx);
        let filtered = !self.font_search.read(cx).text().trim().is_empty();
        let rows: Vec<AnyElement> = visible
            .into_iter()
            .enumerate()
            .map(|(ix, family)| {
                let available = kind.is_available_for(availability, &family);
                let active = family == effective;
                let focused = family == selected;
                let label = SharedString::from(family.label().to_owned());
                popover::menu_row_nav(theme, active, focused, format!("{slug}-font-option-{ix}"))
                    .id(SharedString::from(format!("{slug}-font-option-{ix}")))
                    .when(available, |row| {
                        row.on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.set_selected_font(kind, family.clone());
                            this.commit_font(kind, cx);
                        }))
                    })
                    .when(!available, |row| row.opacity(0.45))
                    .child(div().flex_1().min_w_0().truncate().child(label))
                    .child(div().w(px(18.0)).flex_none().when(active, |slot| {
                        slot.child(
                            icons::icon(icons::CHECK)
                                .size(px(14.0))
                                .text_color(theme.accent),
                        )
                    }))
                    .into_any_element()
            })
            .collect();

        let list: AnyElement = if rows.is_empty() {
            div()
                .px(px(8.0))
                .py(px(6.0))
                .text_size(px(12.0))
                .text_color(theme.for_popup().text_faint)
                .child(SharedString::from(if filtered {
                    "No matching fonts"
                } else {
                    "No fonts"
                }))
                .into_any_element()
        } else {
            // Card-bleed scroll host (see [`popover::menu_scroll_host`]): the
            // rail mounts as a sibling of the scroller, above its clip. The
            // bleed stays horizontal — the search input sits above the list.
            let rail = widgets::rail(self.font_list_mut(kind), rail_id, theme, cx, move |page| {
                page.font_list_mut(kind)
            });
            let scroll = self.font_list_mut(kind).scroll.clone();
            popover::menu_scroll_host(host_id)
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    if this.font_list_mut(kind).set_list_hovered(*hovered) {
                        cx.notify();
                    }
                }))
                .child(
                    popover::menu_scroll_list(list_id, &scroll)
                        .max_h(px(280.0))
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .children(rows),
                )
                .children(rail)
                .into_any_element()
        };

        // The key handler sits on the card, not just the trigger: focus moves
        // into the filter input, and `PaletteSearch` lets arrows/Enter/Escape
        // bubble to exactly this ancestor.
        let menu = popover::popover_card(theme)
            .w(px(220.0))
            .font_family(fixed)
            .on_mouse_down_out(cx.listener(move |this, _, _, cx| this.dismiss_font_menu(kind, cx)))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if this.on_font_key_down(kind, event, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .flex()
            .flex_col()
            .child(popover::search_input_frame(
                theme,
                self.font_search.clone().into_any_element(),
            ))
            .child(list)
            .into_any_element();

        let open = self.font_menu(kind).is_open();
        let closing = self.font_menu(kind).closing_since();
        div()
            .id(SharedString::from(format!("{slug}-font-dropdown")))
            .relative()
            .w(px(220.0))
            .h(px(36.0))
            .px(px(11.0))
            .rounded(px(9.0))
            .border_1()
            .border_color(if open {
                theme.border_strong
            } else {
                theme.border
            })
            .bg(crate::theme::ink(0.025))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .cursor_pointer()
            .track_focus(self.font_focus(kind))
            // Only the closed state: once open, the card above owns these keys
            // and stops their propagation before they reach us.
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if !this.font_menu(kind).is_open() {
                    this.on_font_key_down(kind, event, window, cx);
                }
            }))
            .on_click(cx.listener(move |this, _, window, cx| {
                window.focus(&this.font_focus(kind).clone(), cx);
                this.toggle_font_menu(kind, window, cx);
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(SharedString::from(effective.label().to_owned())),
            )
            .child(
                icons::icon(icons::ALT_ARROW_DOWN)
                    .size(px(14.0))
                    .flex_none()
                    .text_color(theme.text_muted),
            )
            .when_some(self.font_menu(kind).get(), |trigger, _| {
                trigger.child(popover::anchored_menu_below(
                    SharedString::from(format!("{slug}-font-menu")),
                    menu,
                    closing,
                ))
            })
            .into_any_element()
    }

    /// The size dropdown, identical in shape to the family picker beside it.
    /// Every kind picks from a discrete ladder, so all three rows read alike.
    pub(super) fn render_size_picker(
        &mut self,
        kind: FontKind,
        theme: &Theme,
        fixed: SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let slug = kind.slug();
        let labels = kind.size_labels();
        let current = kind.size_ix(cx);
        let selected = self.selected_size_ix(kind);
        let rows: Vec<AnyElement> = labels
            .iter()
            .enumerate()
            .map(|(ix, label)| {
                popover::menu_row_nav(
                    theme,
                    ix == current,
                    ix == selected,
                    format!("{slug}-font-size-option-{ix}"),
                )
                .id(SharedString::from(format!("{slug}-font-size-option-{ix}")))
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.set_selected_size_ix(kind, ix);
                    this.commit_size(kind, window, cx);
                }))
                .child(div().flex_1().child(label.clone()))
                .child(div().w(px(18.0)).flex_none().when(ix == current, |slot| {
                    slot.child(
                        icons::icon(icons::CHECK)
                            .size(px(14.0))
                            .text_color(theme.accent),
                    )
                }))
                .into_any_element()
            })
            .collect();

        let menu = popover::popover_card(theme)
            .w(px(128.0))
            .font_family(fixed)
            .on_mouse_down_out(cx.listener(move |this, _, _, cx| this.dismiss_size_menu(kind, cx)))
            .flex()
            .flex_col()
            .gap(px(2.0))
            .children(rows)
            .into_any_element();

        let open = self.size_menu(kind).is_open();
        let closing = self.size_menu(kind).closing_since();
        div()
            .id(SharedString::from(format!("{slug}-font-size-dropdown")))
            .relative()
            .w(px(128.0))
            .h(px(36.0))
            .px(px(11.0))
            .rounded(px(9.0))
            .border_1()
            .border_color(if open {
                theme.border_strong
            } else {
                theme.border
            })
            .bg(crate::theme::ink(0.025))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .cursor_pointer()
            .track_focus(self.size_focus(kind))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                this.on_size_key_down(kind, event, window, cx)
            }))
            .on_click(cx.listener(move |this, _, window, cx| {
                window.focus(&this.size_focus(kind).clone(), cx);
                this.toggle_size_menu(kind, cx);
            }))
            .child(div().flex_1().child(labels[current].clone()))
            .child(
                icons::icon(icons::ALT_ARROW_DOWN)
                    .size(px(14.0))
                    .flex_none()
                    .text_color(theme.text_muted),
            )
            .when_some(self.size_menu(kind).get(), |trigger, _| {
                trigger.child(popover::anchored_menu_below(
                    SharedString::from(format!("{slug}-font-size-menu")),
                    menu,
                    closing,
                ))
            })
            .into_any_element()
    }

    pub(super) fn render_theme_selector(
        &mut self,
        appearance_kind: Appearance,
        selections: &ThemeSelection,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let registry = ThemeRegistry::active();
        let selected_id = selections
            .variant_id(model_appearance(appearance_kind))
            .to_owned();
        let selected_variant = registry
            .variant(&selected_id)
            .or_else(|| {
                registry
                    .variants_for(model_appearance(appearance_kind))
                    .next()
            })
            .expect("the built-in registry has both appearances");
        let selected_theme = Theme::for_selection(
            appearance_kind,
            &selected_variant.id,
            AccentSelection::ThemeDefault,
            theme.surface_preference,
        );
        let open = self.theme_menu(appearance_kind).is_open();

        let mut trigger = div()
            .id(SharedString::from(format!(
                "{}-theme-selector",
                if appearance_kind.is_light() {
                    "light"
                } else {
                    "dark"
                }
            )))
            .relative()
            .flex_none()
            .w(px(218.0))
            .h(px(34.0))
            .px(px(10.0))
            .rounded(px(8.0))
            .border_1()
            .border_color(if open {
                theme.border_strong
            } else {
                theme.border
            })
            .bg(theme.surface_raised.opacity(if open { 0.75 } else { 0.42 }))
            .flex()
            .items_center()
            .gap(px(8.0))
            .cursor_pointer()
            .when(!open, |el| {
                el.hover(|style| style.bg(theme.surface_raised_hover))
            })
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, _, _| {
                    this.theme_menu_mut(appearance_kind).note_trigger_press();
                }),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.theme_menu_mut(appearance_kind).take_press_was_open() {
                    this.close_theme_menu(appearance_kind, cx);
                } else {
                    let other = if appearance_kind.is_light() {
                        Appearance::Dark
                    } else {
                        Appearance::Light
                    };
                    this.close_theme_menu(other, cx);
                    this.theme_menu_mut(appearance_kind).open(());
                }
                cx.notify();
            }))
            .child(palette_preview(&selected_theme))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(crate::typography::ui_rems(12.5))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text)
                    .child(SharedString::from(selected_variant.name.clone())),
            )
            .child(
                icons::icon(icons::SORT_VERTICAL)
                    .size(px(14.0))
                    .text_color(theme.text_muted.opacity(if open { 0.9 } else { 0.45 })),
            );

        if self.theme_menu(appearance_kind).get().is_some() {
            let closing = self.theme_menu(appearance_kind).closing_since();
            let heading = if appearance_kind.is_light() {
                "Light themes"
            } else {
                "Dark themes"
            };
            let menu = popover::popover_card(theme)
                .w(px(260.0))
                .on_mouse_down_out(cx.listener(move |this, _, _, cx| {
                    this.close_theme_menu(appearance_kind, cx);
                    cx.notify();
                }))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(popover::menu_heading(theme, heading))
                .children(
                    registry
                        .variants_for(model_appearance(appearance_kind))
                        .enumerate()
                        .map(|(index, variant)| {
                            let id = variant.id.clone();
                            let name = variant.name.clone();
                            let active = id == selected_id;
                            let sample = Theme::for_selection(
                                appearance_kind,
                                &id,
                                AccentSelection::ThemeDefault,
                                theme.surface_preference,
                            );
                            popover::menu_row(
                                theme,
                                active,
                                SharedString::from(format!(
                                    "appearance-theme-menu-{appearance_kind:?}-{index}"
                                )),
                            )
                            .id(SharedString::from(format!(
                                "appearance-theme-row-{appearance_kind:?}-{index}"
                            )))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                appearance::set_theme(appearance_kind, id.clone(), cx);
                                this.close_theme_menu(appearance_kind, cx);
                                cx.notify();
                            }))
                            .child(palette_preview(&sample))
                            .child(div().flex_1().min_w_0().truncate().child(name))
                            .when(active, |row| {
                                row.child(
                                    icons::icon(icons::CHECK)
                                        .size(px(14.0))
                                        .text_color(theme.accent),
                                )
                            })
                        }),
                )
                .into_any_element();
            trigger = trigger.child(popover::anchored_menu_below(
                SharedString::from(format!(
                    "appearance-{}-theme-menu",
                    if appearance_kind.is_light() {
                        "light"
                    } else {
                        "dark"
                    }
                )),
                menu,
                closing,
            ));
        }

        trigger.into_any_element()
    }

    pub(super) fn render_import_dialog(
        &mut self,
        viewport: gpui::Size<gpui::Pixels>,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        {
            let dialog = self.import_dialog.as_mut()?;
            if std::mem::take(&mut dialog.focus_pending) {
                let input_focus = dialog.input.focus_handle(cx);
                window.focus(&input_focus, cx);
            }
        }
        let dialog = self.import_dialog.as_ref()?;
        let input = dialog.input.clone();
        let focus = dialog.focus.clone();
        let mode = dialog.mode;
        let compilation = dialog.compilation.clone();
        let selected = dialog.selected.clone();
        let review_variant = dialog.review_variant.clone();
        let error = dialog.error.clone();
        let ready = compilation.is_some() && !selected.is_empty();
        let hairline = crate::theme::hairline(0.08);

        let mode_control = |label: &'static str, description: &'static str, value: InstallMode| {
            let active = mode == value;
            div()
                .id(SharedString::from(format!(
                    "theme-import-mode-{}",
                    slug(label)
                )))
                .flex_1()
                .min_w_0()
                .p(px(10.0))
                .rounded(px(9.0))
                .border_1()
                .border_color(if active { theme.accent } else { theme.border })
                .bg(if active {
                    theme.accent_wash
                } else {
                    theme.surface_raised.opacity(0.28)
                })
                .cursor_pointer()
                .when(!active, |control| {
                    control.hover(|style| style.bg(theme.surface_raised_hover))
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(dialog) = this.import_dialog.as_mut() {
                        dialog.mode = value;
                    }
                    cx.notify();
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(7.0))
                        .child(
                            div()
                                .size(px(16.0))
                                .rounded_full()
                                .border_1()
                                .border_color(if active {
                                    theme.accent
                                } else {
                                    theme.border_strong
                                })
                                .flex()
                                .items_center()
                                .justify_center()
                                .when(active, |dot| {
                                    dot.child(div().size(px(8.0)).rounded_full().bg(theme.accent))
                                }),
                        )
                        .child(
                            div()
                                .text_size(crate::typography::ui_rems(12.0))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(if active { theme.text } else { theme.text_muted })
                                .child(label),
                        ),
                )
                .child(
                    div()
                        .mt(px(4.0))
                        .ml(px(23.0))
                        .text_size(crate::typography::ui_rems(10.5))
                        .text_color(theme.text_muted)
                        .child(description),
                )
        };

        let section_label = |label: &'static str| {
            div()
                .mb(px(7.0))
                .text_size(crate::typography::ui_rems(11.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.text_muted)
                .child(label)
        };

        let mut main = div()
            .id("theme-import-main")
            .max_h(px(520.0))
            .overflow_y_scroll()
            .px(px(20.0))
            .pb(px(18.0))
            .flex()
            .flex_col()
            .child(section_label("Source"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        popover::dialog_field(input.into_any_element())
                            .flex_1()
                            .min_w_0()
                            .h(px(36.0))
                            .py(px(0.0))
                            .flex()
                            .items_center(),
                    )
                    .child(
                        compact_action(theme, "Browse…", "theme-import-browse")
                            .h(px(36.0))
                            .px(px(12.0))
                            .flex_none()
                            .on_click(cx.listener(|this, _, _, cx| this.choose_import_source(cx))),
                    ),
            )
            .child(
                div()
                    .mt(px(16.0))
                    .child(section_label("Keep it up to date"))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .child(mode_control(
                                "Import a copy",
                                "Works independently from the original file.",
                                InstallMode::Snapshot,
                            ))
                            .child(mode_control(
                                "Link to source",
                                "Reload changes from the file on disk.",
                                InstallMode::Link,
                            )),
                    ),
            );

        if let Some(ref compilation) = compilation {
            main = main.child(
                div()
                    .mt(px(18.0))
                    .pt(px(16.0))
                    .border_t_1()
                    .border_color(hairline)
                    .flex()
                    .items_baseline()
                    .justify_between()
                    .child(section_label("Detected themes").mb(px(0.0)))
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(10.5))
                            .text_color(theme.text_muted)
                            .child(SharedString::from(format!(
                                "{} variant{}",
                                compilation.family.variants.len(),
                                if compilation.family.variants.len() == 1 {
                                    ""
                                } else {
                                    "s"
                                }
                            ))),
                    ),
            );
            for variant in &compilation.family.variants {
                let variant_id = variant.id.clone();
                let selected_now = selected.contains(&variant_id);
                let review_open = review_variant.as_deref() == Some(variant_id.as_str());
                let appearance = if variant.appearance.is_dark() {
                    "Dark"
                } else {
                    "Light"
                };
                let report = compilation.reports.get(&variant.id);
                let sample = Theme::from_variant(
                    variant,
                    AccentSelection::ThemeDefault,
                    SurfacePreference::ThemeDefault,
                );
                main = main.child(
                    div()
                        .id(SharedString::from(format!("theme-import-row-{variant_id}")))
                        .mt(px(8.0))
                        .p(px(11.0))
                        .rounded(px(10.0))
                        .border_1()
                        .border_color(if selected_now {
                            theme.accent.opacity(0.7)
                        } else {
                            theme.border
                        })
                        .bg(if selected_now {
                            theme.accent_wash.opacity(0.42)
                        } else {
                            theme.surface_raised.opacity(0.22)
                        })
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(9.0))
                                .child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "theme-import-select-{variant_id}"
                                        )))
                                        .size(px(18.0))
                                        .rounded(px(5.0))
                                        .border_1()
                                        .border_color(if selected_now {
                                            theme.accent
                                        } else {
                                            theme.border_strong
                                        })
                                        .bg(if selected_now { theme.accent } else { theme.bg })
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .when(selected_now, |item| {
                                            item.child(
                                                icons::icon(icons::CHECK)
                                                    .size(px(12.0))
                                                    .text_color(theme.on_accent),
                                            )
                                        })
                                        .on_click(cx.listener({
                                            let variant_id = variant_id.clone();
                                            move |this, _, _, cx| {
                                                if let Some(dialog) = this.import_dialog.as_mut() {
                                                    if !dialog.selected.remove(&variant_id) {
                                                        dialog.selected.insert(variant_id.clone());
                                                    }
                                                }
                                                cx.notify();
                                            }
                                        })),
                                )
                                .child(palette_preview(&sample))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .child(
                                            div()
                                                .text_size(crate::typography::ui_rems(12.5))
                                                .font_weight(gpui::FontWeight::MEDIUM)
                                                .text_color(theme.text)
                                                .child(SharedString::from(variant.name.clone())),
                                        )
                                        .child(
                                            div()
                                                .text_size(crate::typography::ui_rems(11.0))
                                                .text_color(theme.text_muted)
                                                .child(appearance),
                                        ),
                                )
                                .child(
                                    compact_action(
                                        theme,
                                        if review_open {
                                            "Hide details"
                                        } else {
                                            "Details"
                                        },
                                        format!("theme-import-review-{variant_id}"),
                                    )
                                    .on_click(cx.listener({
                                        let variant_id = variant_id.clone();
                                        move |this, _, _, cx| {
                                            if let Some(dialog) = this.import_dialog.as_mut() {
                                                dialog.review_variant =
                                                    if dialog.review_variant.as_deref()
                                                        == Some(variant_id.as_str())
                                                    {
                                                        None
                                                    } else {
                                                        Some(variant_id.clone())
                                                    };
                                            }
                                            cx.notify();
                                        }
                                    })),
                                ),
                        )
                        .when(review_open, |row| {
                            row.child(
                                div()
                                    .mt(px(10.0))
                                    .pt(px(10.0))
                                    .border_t_1()
                                    .border_color(hairline)
                                    .child(import_scene_preview(variant)),
                            )
                            .when_some(report, |row, report| row.child(report_panel(theme, report)))
                        }),
                );
            }
            for failure in &compilation.failures {
                main = main.child(
                    div()
                        .mt(px(8.0))
                        .p(px(10.0))
                        .rounded(px(8.0))
                        .bg(theme.warning.opacity(0.08))
                        .text_size(crate::typography::ui_rems(11.0))
                        .text_color(theme.warning)
                        .child(SharedString::from(format!(
                            "{} could not be compiled · {}",
                            failure.name, failure.message
                        ))),
                );
            }
        } else {
            main = main.child(
                div()
                    .mt(px(14.0))
                    .flex()
                    .items_start()
                    .gap(px(7.0))
                    .text_size(crate::typography::ui_rems(11.0))
                    .line_height(px(16.0))
                    .text_color(theme.text_muted)
                    .child(
                        icons::icon(icons::INFO_CIRCLE)
                            .size(px(13.0))
                            .mt(px(1.0))
                            .flex_none(),
                    )
                    .child("Zeron finds light and dark variants automatically."),
            );
        }

        if let Some(error) = error {
            main = main.child(
                div()
                    .mt(px(12.0))
                    .p(px(10.0))
                    .rounded(px(8.0))
                    .bg(theme.danger.opacity(0.08))
                    .flex()
                    .items_start()
                    .gap(px(7.0))
                    .text_size(crate::typography::ui_rems(11.0))
                    .line_height(px(16.0))
                    .text_color(theme.danger)
                    .child(
                        icons::icon(icons::DANGER_TRIANGLE)
                            .size(px(13.0))
                            .flex_none()
                            .mt(px(1.0)),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(error)),
            );
        }

        let header = div()
            .px(px(20.0))
            .pt(px(18.0))
            .pb(px(16.0))
            .flex()
            .items_start()
            .gap(px(16.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(popover::dialog_title(theme, "Add a theme"))
                    .child(
                        popover::dialog_body(
                            theme,
                            "Import a local theme into your library or keep it linked to its source.",
                        )
                        .mt(px(4.0)),
                    ),
            )
            .child(
                div()
                    .id("theme-import-close")
                    .size(px(28.0))
                    .rounded(px(7.0))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.surface_raised.opacity(0.28))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.surface_raised_hover))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.import_dialog = None;
                        cx.notify();
                    }))
                    .child(
                        icons::icon(icons::CLOSE)
                            .size(px(12.0))
                            .text_color(theme.text_muted),
                    ),
            );

        let footer = div()
            .border_t_1()
            .border_color(hairline)
            .bg(theme.surface_raised.opacity(0.18))
            .px(px(20.0))
            .py(px(12.0))
            .flex()
            .items_center()
            .justify_end()
            .gap(px(8.0))
            .child(
                compact_action(theme, "Cancel", "theme-import-cancel")
                    .h(px(34.0))
                    .px(px(13.0))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.import_dialog = None;
                        cx.notify();
                    })),
            )
            .child(
                popover::btn_primary(
                    theme,
                    if compilation.is_some() {
                        "Import selected"
                    } else {
                        "Analyze theme"
                    },
                )
                .id("theme-import-action")
                .h(px(34.0))
                .px(px(14.0))
                .py(px(0.0))
                .flex()
                .items_center()
                .when(compilation.is_some() && !ready, |button| {
                    button.opacity(0.45)
                })
                .when(compilation.is_none() || ready, |button| {
                    button.on_click(cx.listener(move |this, _, _, cx| {
                        if this
                            .import_dialog
                            .as_ref()
                            .is_some_and(|dialog| dialog.compilation.is_some())
                        {
                            this.finish_import(cx);
                        } else {
                            this.compile_import(cx);
                        }
                    }))
                }),
            );

        let card = popover::dialog_card(theme)
            .id("theme-import-card")
            .w(px(600.0))
            .max_h(px(760.0))
            .p(px(0.0))
            .overflow_hidden()
            .track_focus(&focus)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                match popover::classify_key(
                    event.keystroke.key.as_str(),
                    event.keystroke.modifiers.platform,
                    event.keystroke.modifiers.control,
                ) {
                    popover::MenuKey::Escape => {
                        this.import_dialog = None;
                        cx.notify();
                    }
                    popover::MenuKey::Enter | popover::MenuKey::ModEnter => {
                        if this
                            .import_dialog
                            .as_ref()
                            .is_some_and(|dialog| dialog.compilation.is_some())
                        {
                            this.finish_import(cx);
                        } else {
                            this.compile_import(cx);
                        }
                    }
                    _ => {}
                }
            }))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.import_dialog = None;
                cx.notify();
            }))
            .child(header)
            .child(main)
            .child(footer)
            .into_any_element();

        Some(popover::modal("theme-import-dialog", viewport, card))
    }

    pub(super) fn render_review_dialog(
        &mut self,
        viewport: gpui::Size<gpui::Pixels>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let entry_id = self.review_entry.as_ref()?;
        let entry = theme_library::entries(cx)
            .into_iter()
            .find(|entry| &entry.id == entry_id)?;
        let mut card = popover::dialog_card(theme)
            .id("theme-review-card")
            .w(px(660.0))
            .max_h(px(720.0))
            .overflow_y_scroll()
            .child(popover::dialog_title(theme, "Theme mapping"))
            .child(
                popover::dialog_body(theme, format!("{} · {}", entry.name, entry.source.label()))
                    .mt(px(6.0)),
            );
        for variant in &entry.family.variants {
            card = card
                .child(
                    div()
                        .mt(px(14.0))
                        .text_size(crate::typography::ui_rems(12.5))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child(SharedString::from(variant.name.clone())),
                )
                .child(import_scene_preview(variant));
            if let Some(report) = entry.reports.get(&variant.id) {
                card = card.child(report_panel(theme, report));
            }
        }
        card = card.child(
            div().mt(px(16.0)).flex().justify_end().child(
                popover::btn_primary(theme, "Done")
                    .id("theme-review-close")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.review_entry = None;
                        cx.notify();
                    })),
            ),
        );
        Some(popover::modal(
            "theme-review-dialog",
            viewport,
            card.into_any_element(),
        ))
    }

    pub(super) fn render_library_entry(
        &mut self,
        entry: CustomThemeEntry,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = entry.id.clone();
        let linked = entry.source.is_linked();
        let source = entry
            .source
            .path()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "Self-contained snapshot".into());
        let status = match &entry.status {
            CustomThemeStatus::Ready => format!(
                "{} · {} variant{} · {}",
                entry.source.label(),
                entry.family.variants.len(),
                if entry.family.variants.len() == 1 {
                    ""
                } else {
                    "s"
                },
                source
            ),
            CustomThemeStatus::Warning { message } => {
                format!("Using last known good · {message}")
            }
        };
        widgets::card_row(theme, false)
            .child(widgets::row_tile(
                theme,
                if linked {
                    icons::GLOBAL
                } else {
                    icons::DOCUMENT
                },
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(widgets::row_title(theme, &entry.name))
                    .child(
                        div()
                            .truncate()
                            .text_size(crate::typography::ui_rems(11.0))
                            .text_color(
                                if matches!(entry.status, CustomThemeStatus::Warning { .. }) {
                                    theme.warning
                                } else {
                                    theme.text_muted
                                },
                            )
                            .child(SharedString::from(status)),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .when(linked, |actions| {
                        actions.child(
                            compact_action(theme, "Reload", format!("theme-reload-{id}")).on_click(
                                cx.listener({
                                    let id = id.clone();
                                    move |_, _, _, cx| {
                                        let _ = theme_library::reload(&id, cx);
                                        cx.notify();
                                    }
                                }),
                            ),
                        )
                    })
                    .child(
                        compact_action(theme, "Reveal", format!("theme-reveal-{id}")).on_click(
                            cx.listener({
                                let id = id.clone();
                                move |this, _, _, cx| {
                                    if let Err(error) = theme_library::reveal(&id, cx) {
                                        this.library_error = Some(error.to_string().into());
                                    }
                                    cx.notify();
                                }
                            }),
                        ),
                    )
                    .child(
                        compact_action(theme, "Review", format!("theme-review-{id}")).on_click(
                            cx.listener({
                                let id = id.clone();
                                move |this, _, _, cx| {
                                    this.review_entry = Some(id.clone());
                                    cx.notify();
                                }
                            }),
                        ),
                    )
                    .child(
                        compact_action(
                            theme,
                            "Duplicate as editable",
                            format!("theme-duplicate-{id}"),
                        )
                        .on_click(cx.listener({
                            let id = id.clone();
                            move |this, _, _, cx| {
                                if let Err(error) = theme_library::duplicate_as_editable(&id, cx) {
                                    this.library_error = Some(error.to_string().into());
                                }
                                cx.notify();
                            }
                        })),
                    )
                    .when(linked, |actions| {
                        actions.child(
                            compact_action(theme, "Unlink", format!("theme-unlink-{id}")).on_click(
                                cx.listener({
                                    let id = id.clone();
                                    move |this, _, _, cx| {
                                        if let Err(error) = theme_library::unlink(&id, cx) {
                                            this.library_error = Some(error.to_string().into());
                                        }
                                        cx.notify();
                                    }
                                }),
                            ),
                        )
                    })
                    .child(
                        compact_action(theme, "Remove", format!("theme-remove-{id}"))
                            .text_color(theme.danger)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Err(error) = theme_library::remove(&id, cx) {
                                    this.library_error = Some(error.to_string().into());
                                }
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_theme_library_rows(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let entries = theme_library::entries(cx);
        let (linked, imported): (Vec<_>, Vec<_>) = entries
            .into_iter()
            .partition(|entry| entry.source.is_linked());
        let mut rows = vec![
            widgets::card_row(theme, false)
                .child(widgets::row_tile(theme, icons::FOLDER_WITH_FILES))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(widgets::row_title(theme, "Theme library"))
                        .child(widgets::meta_line(
                            theme,
                            vec![
                                div()
                                    .child("Import or link custom themes.")
                                    .into_any_element(),
                            ],
                        )),
                )
                .child(
                    popover::btn_primary(theme, "Add theme")
                        .id("theme-library-add")
                        .on_click(cx.listener(|this, _, _, cx| this.open_import(cx))),
                )
                .into_any_element(),
        ];
        if !imported.is_empty() {
            rows.push(
                div()
                    .px(px(16.0))
                    .pt(px(12.0))
                    .pb(px(4.0))
                    .border_t_1()
                    .border_color(theme.border)
                    .text_size(crate::typography::ui_rems(10.5))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.text_faint)
                    .child("IMPORTED")
                    .into_any_element(),
            );
            rows.extend(
                imported
                    .into_iter()
                    .map(|entry| self.render_library_entry(entry, theme, cx)),
            );
        }
        if !linked.is_empty() {
            rows.push(
                div()
                    .px(px(16.0))
                    .pt(px(12.0))
                    .pb(px(4.0))
                    .border_t_1()
                    .border_color(theme.border)
                    .text_size(crate::typography::ui_rems(10.5))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.text_faint)
                    .child("LINKED")
                    .into_any_element(),
            );
            rows.extend(
                linked
                    .into_iter()
                    .map(|entry| self.render_library_entry(entry, theme, cx)),
            );
        }
        rows
    }
}
