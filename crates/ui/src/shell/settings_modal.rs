//! Settings modal navigation drawer and page routing for the application shell.

use super::*;

impl Shell {
    pub(super) fn open_settings(&mut self, section: SettingsSection, cx: &mut Context<Self>) {
        self.command_palette = None;
        // Recreate per visit: the page's ListHarnesses load re-probes which
        // CLIs are installed, so installing one shows up on the next open.
        if section == SettingsSection::Harnesses {
            self.harnesses_page = None;
        }
        self.route = Route::Settings(section);
        self.nav.push(NavEntry::Settings(section));
        self.close_user_menu(cx);
        self.close_chat_menu(cx);
        cx.notify();
    }

    pub(super) fn close_settings(&mut self, cx: &mut Context<Self>) {
        self.route = Route::Chat;
        self.focus_composer(cx);
        self.nav.push(NavEntry::Chat(self.active_chat.clone()));
        cx.notify();
    }

    /// Lazily create the entity for a settings section and return it renderable.
    pub(super) fn settings_outlet(
        &mut self,
        section: SettingsSection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match section {
            SettingsSection::Devices => {
                if self.devices_page.is_none() {
                    let state = self.state.clone();
                    self.devices_page = Some(cx.new(|cx| DevicesPage::new(state, cx)));
                }
                match &self.devices_page {
                    Some(page) => page.clone().into_any_element(),
                    None => Empty.into_any_element(),
                }
            }
            SettingsSection::Harnesses => {
                if self.harnesses_page.is_none() {
                    let state = self.state.clone();
                    self.harnesses_page = Some(cx.new(|cx| HarnessesPage::new(state, cx)));
                }
                match &self.harnesses_page {
                    Some(page) => page.clone().into_any_element(),
                    None => Empty.into_any_element(),
                }
            }
            SettingsSection::Agents => {
                if self.accounts_page.is_none() {
                    let state = self.state.clone();
                    self.accounts_page = Some(cx.new(|cx| AccountsPage::new(state, cx)));
                }
                match &self.accounts_page {
                    Some(page) => page.clone().into_any_element(),
                    None => Empty.into_any_element(),
                }
            }
            SettingsSection::Appearance => {
                if self.appearance_page.is_none() {
                    let page = cx.new(AppearancePage::new);
                    self.appearance_settings_sub = Some(cx.subscribe(
                        &page,
                        |this: &mut Shell, _, event: &AppearanceSettingsEvent, cx| match *event {
                            AppearanceSettingsEvent::CodeFontSizeChanged(size) => {
                                this.set_code_font_size(size, cx);
                            }
                        },
                    ));
                    self.appearance_page = Some(page);
                }
                match &self.appearance_page {
                    Some(page) => page.clone().into_any_element(),
                    None => Empty.into_any_element(),
                }
            }
            SettingsSection::Files => {
                if self.files_settings_page.is_none() {
                    let page = cx.new(|cx| {
                        FilesSettingsPage::new(
                            self.settings.files_autosave_enabled,
                            self.settings.files_autosave_delay_ms,
                            self.settings.files_word_wrap,
                            self.settings.files_show_all,
                            cx,
                        )
                    });
                    self.files_settings_sub = Some(cx.subscribe_in(
                        &page,
                        window,
                        |this: &mut Shell, _, event: &FilesSettingsEvent, window, cx| match *event {
                            FilesSettingsEvent::AutosaveChanged(autosave_enabled) => {
                                this.settings.files_autosave_enabled = autosave_enabled;
                                for surface in
                                    this.files.values().chain(this.file_surfaces.values())
                                {
                                    surface.update(cx, |surface, cx| {
                                        surface.set_autosave_enabled(autosave_enabled, cx)
                                    });
                                }
                                this.schedule_save(cx);
                                cx.notify();
                            }
                            FilesSettingsEvent::AutosaveDelayChanged(autosave_delay_ms) => {
                                this.settings.files_autosave_delay_ms = autosave_delay_ms;
                                for surface in
                                    this.files.values().chain(this.file_surfaces.values())
                                {
                                    surface.update(cx, |surface, cx| {
                                        surface.set_autosave_delay_ms(autosave_delay_ms, cx)
                                    });
                                }
                                this.schedule_save(cx);
                                cx.notify();
                            }
                            FilesSettingsEvent::WordWrapChanged(word_wrap) => {
                                this.set_files_word_wrap(word_wrap, window, cx);
                            }
                            FilesSettingsEvent::ShowAllFilesChanged(show_all_files) => {
                                this.set_files_show_all(show_all_files, cx);
                            }
                        },
                    ));
                    self.files_settings_page = Some(page);
                }
                match &self.files_settings_page {
                    Some(page) => page.clone().into_any_element(),
                    None => Empty.into_any_element(),
                }
            }
            SettingsSection::Notifications => {
                if self.notifications_page.is_none() {
                    let page = cx.new(|cx| {
                        NotificationsPage::new(
                            self.settings.sound_enabled,
                            self.settings.sound_completion_enabled,
                            self.settings.sound_input_enabled,
                            self.settings.sound_attention_enabled,
                            self.settings.notifications_enabled,
                            self.settings.notifications_background_only,
                            cx,
                        )
                    });
                    // Persist the flags whenever the page flips one.
                    self.notifications_sub = Some(cx.subscribe(
                        &page,
                        |this: &mut Shell, _, event: &NotificationsEvent, cx| {
                            let NotificationsEvent::Changed {
                                sound,
                                completion_sound,
                                input_sound,
                                attention_sound,
                                desktop,
                                background_only,
                            } = *event;
                            this.settings.sound_enabled = sound;
                            this.settings.sound_completion_enabled = completion_sound;
                            this.settings.sound_input_enabled = input_sound;
                            this.settings.sound_attention_enabled = attention_sound;
                            this.settings.notifications_enabled = desktop;
                            this.settings.notifications_background_only = background_only;
                            this.schedule_save(cx);
                            cx.notify();
                        },
                    ));
                    self.notifications_page = Some(page);
                }
                match &self.notifications_page {
                    Some(page) => page.clone().into_any_element(),
                    None => Empty.into_any_element(),
                }
            }
            SettingsSection::Shortcuts | SettingsSection::Appshots => {
                if self.shortcuts_page.is_none() {
                    let state = self.state.clone();
                    let keymap = self.settings.keymap.clone();
                    let escape_stops_active_agent = self.settings.escape_stops_active_agent;
                    let composer_send_behavior = self.settings.composer_send_behavior;
                    let appshots_enabled = self.settings.appshots_enabled;
                    let appshot_sound_enabled = self.settings.appshot_sound_enabled;
                    let appshot_destination = self.settings.appshot_destination;
                    let page = cx.new(|cx| {
                        ShortcutsPage::new(
                            state,
                            keymap,
                            escape_stops_active_agent,
                            composer_send_behavior,
                            appshots_enabled,
                            appshot_sound_enabled,
                            appshot_destination,
                            cx,
                        )
                    });
                    // Persist + re-apply shortcut preferences whenever the page changes them.
                    self.shortcuts_sub = Some(cx.subscribe(
                        &page,
                        |this: &mut Shell, _, event: &ShortcutsEvent, cx| {
                            match event {
                                ShortcutsEvent::KeymapChanged(keymap) => {
                                    this.settings.keymap = keymap.clone();
                                }
                                ShortcutsEvent::EscapeStopsActiveAgentChanged(enabled) => {
                                    this.settings.escape_stops_active_agent = *enabled;
                                }
                                ShortcutsEvent::ComposerSendBehaviorChanged(behavior) => {
                                    this.settings.composer_send_behavior = *behavior;
                                }
                                ShortcutsEvent::AppshotsChanged {
                                    enabled,
                                    sound_enabled,
                                    destination,
                                } => {
                                    this.settings.appshots_enabled = *enabled;
                                    this.settings.appshot_sound_enabled = *sound_enabled;
                                    crate::appshots::set_capture_sound_enabled(*sound_enabled);
                                    this.settings.appshot_destination = *destination;
                                    crate::appshots::set_enabled(*enabled);
                                }
                            }
                            apply_keymap(
                                cx,
                                &this.settings.keymap,
                                this.settings.composer_send_behavior,
                            );
                            this.schedule_save(cx);
                            cx.notify();
                        },
                    ));
                    self.shortcuts_page = Some(page);
                }
                match &self.shortcuts_page {
                    Some(page) => {
                        page.update(cx, |page, _| {
                            page.show_appshots(section == SettingsSection::Appshots)
                        });
                        page.clone().into_any_element()
                    }
                    None => Empty.into_any_element(),
                }
            }
            SettingsSection::Archived => {
                if self.archived_page.is_none() {
                    let state = self.state.clone();
                    self.archived_page = Some(cx.new(|cx| ArchivedPage::new(state, cx)));
                }
                match &self.archived_page {
                    Some(page) => page.clone().into_any_element(),
                    None => Empty.into_any_element(),
                }
            }
        }
    }

    /// Settings-mode sidebar (zeron settings-sidebar.tsx): window-control
    /// strip, "Settings" heading, icon section rows styled like session rows,
    /// and a Back row pinned to the bottom.
    pub(super) fn render_settings_nav(
        &mut self,
        section: SettingsSection,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let section_icon = |item: SettingsSection| match item {
            SettingsSection::Devices => icons::MONITOR,
            SettingsSection::Harnesses => icons::WIDGET,
            SettingsSection::Agents => icons::KEY_MINIMALISTIC,
            SettingsSection::Appearance => icons::TUNING,
            SettingsSection::Files => icons::FOLDER,
            SettingsSection::Notifications => icons::BELL,
            SettingsSection::Shortcuts => icons::KEYBOARD,
            SettingsSection::Appshots => icons::MONITOR,
            SettingsSection::Archived => icons::ARCHIVE_MINIMALISTIC,
        };
        // Match the user's dragged sidebar width — the pane container clips to
        // it, so a hardcoded default here left hover washes stopping short of
        // the sidebar's right edge (user-reported). Device identity lives on
        // the Accounts page now — the one surface where the device matters.
        div()
            .w(px(self.settings.sidebar_width))
            .h_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_1()
                    .px(px(Theme::SPACE_SM))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .px(px(Theme::SPACE_SM))
                            .pt(px(12.0))
                            .pb(px(4.0))
                            .text_size(crate::typography::ui_rems(11.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.text_muted.opacity(0.6))
                            .child(SharedString::from("Settings")),
                    )
                    .child(
                        div().flex().flex_col().gap(px(2.0)).children(
                            SettingsSection::ALL
                                .into_iter()
                                .filter(|item| {
                                    *item != SettingsSection::Appshots
                                        || crate::appshots::is_desktop()
                                })
                                .map(|item| {
                                    let selected = item == section;
                                    div()
                                        .id(SharedString::from(format!(
                                            "settings-nav-{}",
                                            item.label()
                                        )))
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(8.0))
                                        .rounded(px(8.0))
                                        .border_1()
                                        .border_color(if selected && theme.appearance.is_light() {
                                            crate::theme::hairline(0.12)
                                        } else {
                                            gpui::transparent_black()
                                        })
                                        .px(px(Theme::SPACE_SM))
                                        .py(px(6.0))
                                        .text_size(crate::typography::ui_rems(13.0))
                                        .when(selected, |el| {
                                            // Same tokens as the main sidebar's session
                                            // rows — the two sidebars must feel alike.
                                            el.bg(crate::theme::glass_selected_bg())
                                                .font_weight(gpui::FontWeight::MEDIUM)
                                        })
                                        .text_color(if selected {
                                            theme.text
                                        } else {
                                            theme.text_muted
                                        })
                                        .cursor_pointer()
                                        .hover(|s| s.bg(theme.glass_hover()).text_color(theme.text))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.open_settings(item, cx)
                                        }))
                                        .child(
                                            icon(section_icon(item))
                                                .size(px(16.0))
                                                .text_color(theme.text_muted),
                                        )
                                        .child(SharedString::from(item.label()))
                                }),
                        ),
                    ),
            )
            // Back pinned to the bottom (zeron settings-sidebar.tsx).
            .child(
                div().px(px(Theme::SPACE_SM)).pb(px(12.0)).child(
                    div()
                        .id("settings-back")
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .rounded(px(8.0))
                        .px(px(Theme::SPACE_SM))
                        .py(px(6.0))
                        .text_size(crate::typography::ui_rems(13.0))
                        .text_color(theme.text_muted)
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.glass_hover()).text_color(theme.text))
                        .on_click(cx.listener(|this, _, _, cx| this.close_settings(cx)))
                        .child(
                            // AltArrowLeft chevron (zeron settings-sidebar.tsx),
                            // not the straight history arrow.
                            icon(icons::ALT_ARROW_LEFT)
                                .size(px(16.0))
                                .text_color(theme.text_muted),
                        )
                        .child(SharedString::from("Back")),
                ),
            )
            .into_any_element()
    }
}
