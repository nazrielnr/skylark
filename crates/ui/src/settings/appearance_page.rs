use super::*;

impl EventEmitter<AppearanceSettingsEvent> for AppearancePage {}

impl Render for AppearancePage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let availability = typography::availability(cx);
        let fixed = theme.font_sans_fixed.clone();
        let current_mode = appearance::mode(cx);
        let current_themes = appearance::themes(cx);
        let current_accent = appearance::accent(cx);
        let current_surface = appearance::surface(cx);
        let ui_settings = crate::settings::current(cx);
        let current_background = ui_settings.new_thread_composer_background;
        let current_background_effect = ui_settings.new_thread_background_effect;
        let cards = AppearanceMode::ALL
            .into_iter()
            .map(|mode| {
                widgets::option_card(
                    &theme,
                    mode.icon(),
                    mode.label(),
                    mode == current_mode,
                    preview(mode, &current_themes, current_accent, current_surface),
                )
                .id(SharedString::from(format!("appearance-{}", mode.label())))
                .on_click(cx.listener(move |_, _, _, cx| {
                    appearance::set_mode(mode, cx);
                    cx.notify();
                }))
            })
            .collect::<Vec<_>>();

        let mut theme_rows = Vec::new();
        for (index, appearance_kind) in [Appearance::Light, Appearance::Dark]
            .into_iter()
            .enumerate()
        {
            let (label, mode) = if appearance_kind.is_light() {
                ("Light theme", AppearanceMode::Light)
            } else {
                ("Dark theme", AppearanceMode::Dark)
            };
            let selector = self.render_theme_selector(appearance_kind, &current_themes, &theme, cx);
            theme_rows.push(
                widgets::card_row(&theme, index == 0)
                    .child(widgets::row_tile(&theme, mode.icon()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(widgets::row_title(&theme, label))
                            .child(widgets::meta_line(
                                &theme,
                                vec![
                                    div()
                                        .child(SharedString::from(
                                            "Used whenever this appearance is active.",
                                        ))
                                        .into_any_element(),
                                ],
                            )),
                    )
                    .child(selector)
                    .into_any_element(),
            );
        }

        let mut accent_choices = vec![AccentSelection::ThemeDefault];
        accent_choices.extend(AccentPreset::ALL.map(AccentSelection::Preset));
        let accent_controls = accent_choices
            .into_iter()
            .map(|selection| {
                let selected = selection == current_accent;
                accent_swatch(&theme, selection, selected).on_click(cx.listener(
                    move |_, _, _, cx| {
                        appearance::set_accent(selection, cx);
                        cx.notify();
                    },
                ))
            })
            .collect::<Vec<_>>();
        let surface_controls = SurfacePreference::ALL
            .into_iter()
            .map(|surface| {
                surface_choice(&theme, surface, surface == current_surface).on_click(cx.listener(
                    move |_, _, _, cx| {
                        appearance::set_surface(surface, cx);
                        cx.notify();
                    },
                ))
            })
            .collect::<Vec<_>>();
        let mut settings_rows = theme_rows;
        settings_rows.push(
            widgets::card_row(&theme, false)
                .child(widgets::row_tile(&theme, icons::TUNING))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(widgets::row_title(&theme, "Accent color"))
                        .child(widgets::meta_line(
                            &theme,
                            vec![
                                div()
                                    .child(SharedString::from(accent_helper(current_accent)))
                                    .into_any_element(),
                            ],
                        )),
                )
                .child(
                    div()
                        .flex_none()
                        .ml(px(10.0))
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .children(accent_controls),
                )
                .into_any_element(),
        );
        settings_rows.push(
            widgets::card_row(&theme, false)
                .child(widgets::row_tile(&theme, icons::WIDGET))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(widgets::row_title(&theme, "Glass"))
                        .child(widgets::meta_line(
                            &theme,
                            vec![
                                div()
                                    .child(SharedString::from(surface_helper(
                                        current_surface,
                                        theme.surface_treatment,
                                    )))
                                    .into_any_element(),
                            ],
                        )),
                )
                .child(
                    div()
                        .flex_none()
                        .ml(px(10.0))
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .children(surface_controls),
                )
                .into_any_element(),
        );
        let background_available = current_background
            .as_ref()
            .is_some_and(|background| Path::new(&background.path).is_file());
        let background_tile: AnyElement = if let Some(background) =
            current_background.as_ref().filter(|_| background_available)
        {
            div()
                .flex_none()
                .size(px(36.0))
                .rounded(px(10.0))
                .overflow_hidden()
                .border_1()
                .border_color(crate::theme::hairline(0.10))
                .child(
                    img(PathBuf::from(background.path.clone()))
                        .size(px(34.0))
                        .rounded(px(9.0))
                        .object_fit(ObjectFit::Cover),
                )
                .into_any_element()
        } else {
            widgets::row_tile(&theme, icons::FILE_IMAGE).into_any_element()
        };
        let background_meta = match current_background.as_ref() {
            Some(background) if background_available => vec![
                div()
                    .child(SharedString::from(background.name.clone()))
                    .into_any_element(),
                div()
                    .child("Softened automatically on frosted themes.")
                    .into_any_element(),
            ],
            Some(_) => vec![
                div().child("Image unavailable").into_any_element(),
                div()
                    .child("Choose a replacement or remove it.")
                    .into_any_element(),
            ],
            None => vec![
                div()
                    .child("Add an image behind the composer on empty new threads.")
                    .into_any_element(),
            ],
        };
        settings_rows.push(
            widgets::card_row(&theme, false)
                .child(background_tile)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(widgets::row_title(&theme, "New thread composer background"))
                        .child(widgets::meta_line(&theme, background_meta)),
                )
                .child(
                    div()
                        .flex_none()
                        .ml(px(10.0))
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .when(current_background.is_some(), |actions| {
                            actions
                                .child(
                                    compact_action(
                                        &theme,
                                        "Replace image",
                                        "new-thread-background-replace",
                                    )
                                    .on_click(cx.listener(
                                        |this, _, _, cx| this.choose_new_thread_background(cx),
                                    )),
                                )
                                .child(
                                    compact_action(
                                        &theme,
                                        "Remove",
                                        "new-thread-background-remove",
                                    )
                                    .text_color(theme.danger)
                                    .on_click(cx.listener(
                                        |this, _, _, cx| this.remove_new_thread_background(cx),
                                    )),
                                )
                        })
                        .when(current_background.is_none(), |actions| {
                            actions.child(
                                compact_action(
                                    &theme,
                                    "Choose image",
                                    "new-thread-background-choose",
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| this.choose_new_thread_background(cx),
                                )),
                            )
                        }),
                )
                .into_any_element(),
        );
        if background_available {
            let effect_controls = crate::settings::NewThreadBackgroundEffect::ALL
                .into_iter()
                .map(|effect| {
                    background_effect_choice(&theme, effect, effect == current_background_effect)
                        .on_click(cx.listener(move |_, _, _, cx| {
                            crate::settings::set_new_thread_background_effect(effect, cx);
                            cx.notify();
                        }))
                })
                .collect::<Vec<_>>();
            settings_rows.push(
                widgets::card_row(&theme, false)
                    .child(widgets::row_tile(&theme, icons::TUNING))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(widgets::row_title(&theme, "Background effect"))
                            .child(widgets::meta_line(
                                &theme,
                                vec![
                                    div()
                                        .child(current_background_effect.description())
                                        .into_any_element(),
                                ],
                            )),
                    )
                    .child(
                        div()
                            .flex_none()
                            .ml(px(10.0))
                            .max_w(px(430.0))
                            .flex()
                            .flex_wrap()
                            .justify_end()
                            .gap(px(6.0))
                            .children(effect_controls),
                    )
                    .into_any_element(),
            );
        }
        if let Some(error) = self.background_error.clone() {
            settings_rows.push(
                div()
                    .px(px(20.0))
                    .py(px(10.0))
                    .border_t_1()
                    .border_color(theme.border)
                    .child(widgets::error_strip(&theme, error))
                    .into_any_element(),
            );
        }
        settings_rows.extend(self.render_theme_library_rows(&theme, cx));
        let library_warning = self
            .library_error
            .clone()
            .or_else(|| theme_library::load_warning(cx).map(SharedString::from));
        let modal = self
            .render_import_dialog(window.viewport_size(), &theme, window, cx)
            .or_else(|| self.render_review_dialog(window.viewport_size(), &theme, cx));

        let ui_picker =
            self.render_font_picker(FontKind::Ui, &theme, &availability, fixed.clone(), cx);
        let terminal_picker =
            self.render_font_picker(FontKind::Terminal, &theme, &availability, fixed.clone(), cx);
        let code_picker =
            self.render_font_picker(FontKind::Code, &theme, &availability, fixed.clone(), cx);
        let ui_size = self.render_size_picker(FontKind::Ui, &theme, fixed.clone(), cx);
        let terminal_size = self.render_size_picker(FontKind::Terminal, &theme, fixed.clone(), cx);
        let code_size = self.render_size_picker(FontKind::Code, &theme, fixed.clone(), cx);

        let mut font_section = div()
            .mt(px(36.0))
            .flex()
            .flex_col()
            .gap(px(18.0))
            .font_family(fixed.clone());
        for (kind, picker, size_control) in [
            (FontKind::Ui, ui_picker, ui_size),
            (FontKind::Terminal, terminal_picker, terminal_size),
            (FontKind::Code, code_picker, code_size),
        ] {
            font_section = font_section.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(24.0))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(widgets::field_label(&theme, kind.label()))
                            .child(
                                div()
                                    .max_w(px(520.0))
                                    .text_size(typography::ui_rems(12.0))
                                    .line_height(px(18.0))
                                    .text_color(theme.text_muted)
                                    .child(kind.description()),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .child(picker)
                            .child(size_control),
                    ),
            );
        }
        font_section = font_section.child(self.render_transcript_width(&theme, window, cx));
        for kind in FontKind::ALL {
            let (requested, effective) = (kind.requested(cx), kind.effective(cx));
            if requested != effective {
                font_section = font_section.child(
                    widgets::error_strip(
                        &theme,
                        format!(
                            "{} \"{}\" isn't available on this device. Using {}.",
                            kind.label(),
                            requested.label(),
                            effective.label()
                        ),
                    )
                    .font_family(fixed.clone()),
                );
            }
        }

        let scrollbar = popover::rail(self, "appearance-page-scrollbar", &theme, cx);
        div()
            .id("appearance-page-host")
            .on_drag_move(cx.listener(
                |this, event: &gpui::DragMoveEvent<TranscriptWidthDrag>, window, cx| {
                    this.drag_width(event.event.position.x, window, cx);
                },
            ))
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, _: &gpui::MouseUpEvent, _, cx| {
                    if this.width_pressed {
                        this.width_pressed = false;
                        cx.notify();
                    }
                    this.apply_pending_width(cx);
                }),
            )
            .on_mouse_up_out(
                gpui::MouseButton::Left,
                cx.listener(|this, _: &gpui::MouseUpEvent, _, cx| {
                    if this.width_pressed {
                        this.width_pressed = false;
                        cx.notify();
                    }
                    this.apply_pending_width(cx);
                }),
            )
            .relative()
            .size_full()
            .on_hover(cx.listener(Self::on_scroll_hovered))
            .child(
                div()
                    .id("appearance-page")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll.scroll)
                    .child(
                        widgets::page_column()
                            .child(widgets::page_header(&theme, "Appearance", None))
                            .child(
                                widgets::page_subtitle(
                                    &theme,
                                    "Choose how Zeron looks. These settings stay on this device.",
                                )
                                .max_w(px(512.0))
                                .line_height(px(20.0)),
                            )
                            .child(
                                div()
                                    .mt(px(32.0))
                                    .flex()
                                    .flex_col()
                                    .gap(px(12.0))
                                    .child(widgets::field_label(&theme, "Appearance"))
                                    .child(widgets::option_card_row().children(cards)),
                            )
                            .child(widgets::section_card(&theme).children(settings_rows))
                            .child(font_section)
                            .when_some(library_warning, |page, warning| {
                                page.child(
                                    div()
                                        .mt(px(8.0))
                                        .text_size(crate::typography::ui_rems(11.5))
                                        .text_color(theme.warning)
                                        .child(warning),
                                )
                            }),
                    ),
            )
            .children(scrollbar)
            .children(modal)
    }
}
