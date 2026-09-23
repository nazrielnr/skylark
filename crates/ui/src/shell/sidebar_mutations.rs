//! Sidebar chat mutations, session actions, context menus, and jump hint shortcuts.

use super::*;
use gpui::{ClipboardItem, ModifiersChangedEvent, Pixels, Point, Window, div, px};

#[derive(Clone, Copy)]
pub(crate) enum ChatMenuPage {
    Root,
    Copy,
}

#[derive(Clone)]
pub(crate) struct ChatMenuState {
    pub(crate) chat_id: String,
    pub(crate) position: Point<Pixels>,
    pub(crate) page: ChatMenuPage,
}

/// The chat-row Rename dialog.
pub(crate) struct RenameChatDialog {
    pub(crate) chat_id: String,
    pub(crate) input: Entity<ComposerInput>,
    /// Focus the input on the dialog's first paint (opened without window access).
    pub(crate) focus_pending: bool,
    pub(crate) _events: Subscription,
}

impl Shell {
    // ---- chat context menu ----

    /// Close the session-row context menu through the exit animation.
    pub(super) fn close_chat_menu(&mut self, cx: &mut Context<Self>) {
        if self.chat_menu.begin_close() {
            popover::reap_popup(cx, |shell: &mut Self| &mut shell.chat_menu);
            cx.notify();
        }
    }

    pub(super) fn open_chat_copy_menu(&mut self, cx: &mut Context<Self>) {
        if let Some(menu) = self.chat_menu.open_mut() {
            menu.page = ChatMenuPage::Copy;
            cx.notify();
        }
    }

    pub(super) fn copy_skylark_conversation_link(&mut self, chat_id: &str, cx: &mut Context<Self>) {
        let link = {
            let state = self.state.read(cx);
            crate::links::workspace_locator(
                state.workspace_scope,
                state.auth.as_ref(),
                state.local_device_id.as_deref(),
            )
            .map(|workspace| crate::links::skylark_conversation_link(chat_id, &workspace))
        };
        if let Some(link) = link {
            cx.write_to_clipboard(ClipboardItem::new_string(link));
            self.sidebar_notice = Some("Skylark conversation link copied".into());
        } else {
            self.sidebar_notice = Some("Conversation link is not ready yet".into());
        }
        self.close_chat_menu(cx);
        cx.notify();
    }

    pub(super) fn copy_harness_conversation_link(&mut self, chat_id: &str, cx: &mut Context<Self>) {
        let link = self
            .state
            .read(cx)
            .chats
            .iter()
            .find(|chat| chat.id == chat_id)
            .and_then(crate::links::harness_conversation_link);
        if let Some(link) = link {
            cx.write_to_clipboard(ClipboardItem::new_string(link.url));
            self.sidebar_notice = Some(format!("{} copied", link.label).into());
        }
        self.close_chat_menu(cx);
        cx.notify();
    }

    pub(super) fn copy_harness_session_id(&mut self, chat_id: &str, cx: &mut Context<Self>) {
        let id = self
            .state
            .read(cx)
            .chats
            .iter()
            .find(|chat| chat.id == chat_id)
            .and_then(|chat| chat.harness_session_id.clone());
        if let Some(id) = id.filter(|id| !id.trim().is_empty()) {
            cx.write_to_clipboard(ClipboardItem::new_string(id));
            self.sidebar_notice = Some("Harness session ID copied".into());
        }
        self.close_chat_menu(cx);
        cx.notify();
    }

    // ---- sidebar mutations ----

    /// Fire a Mutate op; failures surface in the sidebar notice strip.
    pub(super) fn mutate(&mut self, params: serde_json::Value, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.sidebar_notice = Some("Engine not connected".into());
            cx.notify();
            return;
        };
        self.mutate_task = Some(cx.spawn(async move |this, cx| {
            if let Err(err) = engine.client().call(methods::MUTATE, params).await {
                this.update(cx, |shell, cx| {
                    shell.sidebar_notice = Some(format!("{err}").into());
                    cx.notify();
                })
                .ok();
            }
        }));
    }

    pub(super) fn open_rename_chat(&mut self, chat_id: String, cx: &mut Context<Self>) {
        self.close_chat_menu(cx);
        let current = self
            .state
            .read(cx)
            .chats
            .iter()
            .find(|c| c.id == chat_id)
            .and_then(|c| c.title.clone())
            .unwrap_or_default();
        let input = cx.new(|cx| {
            ComposerInput::new("Session title", cx).with_accessibility_role(gpui::Role::TextInput)
        });
        input.update(cx, |input, cx| input.set_text(current, cx));
        let events = cx.subscribe(&input, |this: &mut Shell, _, event, cx| {
            if matches!(event, ComposerInputEvent::Submitted) {
                this.submit_rename_chat(cx);
            }
        });
        self.rename_dialog = Some(RenameChatDialog {
            chat_id,
            input,
            focus_pending: true,
            _events: events,
        });
        cx.notify();
    }

    pub(super) fn submit_rename_chat(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.rename_dialog.take() else {
            return;
        };
        let title = dialog.input.read(cx).text().trim().to_string();
        if !title.is_empty() {
            self.mutate(
                serde_json::json!({ "op": "renameChat", "chatId": dialog.chat_id, "title": title }),
                cx,
            );
        }
        cx.notify();
    }

    pub(super) fn archive_chat(&mut self, chat_id: String, cx: &mut Context<Self>) {
        self.set_chat_archived(chat_id, true, cx);
    }

    pub(super) fn reconcile_sidebar_pins(&mut self, cx: &mut Context<Self>) {
        self.discard_stale_sidebar_pin_writes(cx);
        let Some(profile_key) = self.active_sidebar_pin_profile_key(cx) else {
            return;
        };
        let state = self.state.read(cx);
        if state.workspace_scope == Some(WorkspaceScope::Local) {
            if !state.chats_synced {
                return;
            }
            let known = state.chats.iter().map(|chat| chat.id.clone()).collect();
            let changed = self
                .settings
                .sidebar_pinned_session_ids_by_profile
                .get_mut(&profile_key)
                .is_some_and(|pins| spaces::retain_known_pins(pins, &known));
            if changed {
                self.schedule_save(cx);
            }
            return;
        }
        // Remote pins come exclusively from per-pin registry records.
    }

    pub(super) fn active_sidebar_pin_profile_key(&self, cx: &App) -> Option<String> {
        let state = self.state.read(cx);
        sidebar_pin_profile_key(
            state.workspace_scope,
            state.auth.as_ref(),
            self.boot.org_id.as_deref(),
        )
    }

    pub(super) fn active_sidebar_pins(&self, cx: &App) -> Vec<String> {
        if let Some(pins) = self.optimistic_sidebar_pins(cx) {
            return pins;
        }
        let state = self.state.read(cx);
        match state.workspace_scope {
            Some(WorkspaceScope::Local) => self
                .active_sidebar_pin_profile_key(cx)
                .map(|key| self.settings.sidebar_pins(&key).to_vec())
                .unwrap_or_default(),
            Some(WorkspaceScope::Synced | WorkspaceScope::Development) => {
                state.sidebar_preferences.pinned_session_ids.clone()
            }
            None => Vec::new(),
        }
    }

    pub(super) fn sidebar_pins_for_profile(&self, profile_key: &str, cx: &App) -> Vec<String> {
        if self.active_sidebar_pin_profile_key(cx).as_deref() != Some(profile_key) {
            return Vec::new();
        }
        self.active_sidebar_pins(cx)
    }

    pub(super) fn validate_sidebar_pin_change(
        &mut self,
        profile_key: &str,
        pins: &[String],
        cx: &mut Context<Self>,
    ) -> bool {
        if self.active_sidebar_pin_profile_key(cx).as_deref() != Some(profile_key) {
            return false;
        }
        let state = self.state.read(cx);
        let remote = matches!(
            state.workspace_scope,
            Some(WorkspaceScope::Synced | WorkspaceScope::Development)
        );
        let result = if remote && !state.sidebar_preferences.can_edit() {
            Err("Pins are still syncing")
        } else {
            let current = self.active_sidebar_pins(cx);
            skylark_proto::validate_sidebar_pin_update(&current, pins)
        };
        if let Err(message) = result {
            self.sidebar_notice = Some(message.into());
            cx.notify();
            return false;
        }
        true
    }

    pub(super) fn apply_sidebar_pin_change(
        &mut self,
        profile_key: String,
        change: skylark_proto::SidebarPinChange,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut pinned_session_ids = self.active_sidebar_pins(cx);
        change.project(&mut pinned_session_ids);
        if !self.validate_sidebar_pin_change(&profile_key, &pinned_session_ids, cx)
            || self.active_sidebar_pins(cx) == pinned_session_ids
        {
            return false;
        }
        match self.state.read(cx).workspace_scope {
            Some(WorkspaceScope::Local) => {
                if pinned_session_ids.is_empty() {
                    self.settings
                        .sidebar_pinned_session_ids_by_profile
                        .remove(&profile_key);
                } else {
                    self.settings
                        .sidebar_pinned_session_ids_by_profile
                        .insert(profile_key, pinned_session_ids);
                }
                self.schedule_save(cx);
            }
            Some(WorkspaceScope::Synced | WorkspaceScope::Development) => {
                return self.queue_sidebar_pin_write(profile_key, change, cx);
            }
            None => return false,
        }
        true
    }

    pub(super) fn set_chat_pinned(&mut self, chat_id: String, pinned: bool, cx: &mut Context<Self>) {
        self.close_chat_menu(cx);
        self.cancel_pinned_session_drag(cx);
        let Some(profile_key) = self.active_sidebar_pin_profile_key(cx) else {
            return;
        };
        if !self
            .state
            .read(cx)
            .chats
            .iter()
            .any(|chat| chat.id == chat_id)
        {
            return;
        }
        let pins = self.active_sidebar_pins(cx);
        if pins.contains(&chat_id) == pinned {
            return;
        }
        let change = if pinned {
            skylark_proto::SidebarPinChange::Pin {
                session_id: chat_id,
                after: pins.last().cloned(),
                before: None,
            }
        } else {
            skylark_proto::SidebarPinChange::Unpin {
                session_id: chat_id,
            }
        };
        self.apply_sidebar_pin_change(profile_key, change, cx);
        cx.notify();
    }

    /// The Archive session shortcut. With no chat open, or with an already
    /// archived one, it does nothing — the shortcut archives, it never
    /// unarchives.
    pub(super) fn archive_selected_chat(&mut self, cx: &mut Context<Self>) {
        let Some(chat_id) = self
            .state
            .read(cx)
            .archivable_selected_chat()
            .map(str::to_string)
        else {
            return;
        };
        self.archive_chat(chat_id, cx);
    }

    pub(super) fn set_chat_archived(
        &mut self,
        chat_id: String,
        archived: bool,
        cx: &mut Context<Self>,
    ) {
        self.close_chat_menu(cx);
        self.mutate(
            serde_json::json!({ "op": "setChatArchived", "chatId": chat_id, "archived": archived }),
            cx,
        );
        cx.notify();
    }

    /// A jump shortcut: open the sidebar row at `slot`. A slot past the end of
    /// a short list does nothing. Reads the DISPLAYED order — sort and
    /// grouping view options permute the list, and the chip on a row must
    /// name the key that opens it.
    pub(super) fn jump_to_session(&mut self, slot: usize, cx: &mut Context<Self>) {
        let Some(chat_id) = self.sidebar_visible_order(cx).into_iter().nth(slot) else {
            return;
        };
        // Same path a click on that row takes.
        self.open_chat(chat_id, cx);
    }

    /// Whether an overlay that owns the keyboard is up — the add-space
    /// palette or a composer picker popover (model selector, traits, repo,
    /// branch…). Session-nav shortcuts (cycle/jump/archive) go quiet
    /// underneath one: gpui runs a matched binding before any `on_key_down`,
    /// so an unguarded jump would switch sessions UNDER the open popover,
    /// stranding it over a session the user never picked.
    pub(super) fn overlay_owns_keyboard(&self, cx: &App) -> bool {
        self.command_palette.is_some()
            || self.add_space.is_some()
            || self.composer.read(cx).pickers().read(cx).is_open()
    }

    /// Track held modifiers for sidebar jump hints and the queue's submit hint.
    /// Only a visibility change repaints; modifier traffic is otherwise constant.
    pub(super) fn on_modifiers_changed(&mut self, event: &ModifiersChangedEvent, cx: &mut Context<Self>) {
        self.update_jump_hints(&event.modifiers, cx);
    }

    pub(super) fn update_jump_hints(&mut self, mods: &gpui::Modifiers, cx: &mut Context<Self>) {
        let primary = if cfg!(target_os = "macos") {
            mods.platform
        } else {
            mods.control
        };
        // No hints while an overlay owns the keyboard — the jumps they
        // advertise are suppressed there.
        let visible = matches!(self.route, Route::Chat)
            && !self.overlay_owns_keyboard(cx)
            && jump_hints_visible(&self.settings.keymap, primary, mods.alt, mods.shift);
        self.set_jump_hints(visible, cx);
        let queue_shortcut_revealed = matches!(self.route, Route::Chat)
            && !self.overlay_owns_keyboard(cx)
            && modifier_send_hint_visible(primary, mods.alt, mods.shift);
        self.composer.update(cx, |composer, cx| {
            composer.set_queue_shortcut_revealed(queue_shortcut_revealed, cx)
        });
    }

    pub(super) fn set_jump_hints(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.jump_hints != visible {
            self.jump_hints = visible;
            cx.notify();
        }
    }

    pub(super) fn delete_chat(&mut self, chat_id: String, cx: &mut Context<Self>) {
        self.delete_confirm = None;
        if let Some(tabs) = self.right_tabs.get(&chat_id) {
            for surface in tabs {
                if let RightSurface::Browser(id) = surface {
                    if let Some(browser) = self.browsers.remove(id) {
                        browser.update(cx, |browser, cx| browser.close(cx));
                    }
                    self.browser_subs.remove(id);
                }
            }
        }
        if self.state.read(cx).selected_chat.as_deref() == Some(chat_id.as_str()) {
            self.state.update(cx, |s, cx| s.select_chat(None, cx));
        }
        self.composer
            .update(cx, |composer, cx| composer.purge_chat(&chat_id, cx));
        self.mutate(
            serde_json::json!({ "op": "deleteChat", "chatId": chat_id }),
            cx,
        );
        cx.notify();
    }

    // ---- overlays ----

    pub(super) fn render_chat_menu_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu_state = self.chat_menu.get().cloned()?;
        let theme = Theme::of(cx).for_popup();
        let chat_id = menu_state.chat_id;
        let position = menu_state.position;
        let chat_menu_closing = self.chat_menu.closing_since();
        let is_pinned = self.active_sidebar_pins(cx).contains(&chat_id);
        let rename_id = chat_id.clone();
        let pin_id = chat_id.clone();
        let archive_id = chat_id.clone();
        let delete_id = chat_id.clone();
        let menu = popover::popover_card(&theme)
            .w(px(216.0))
            .on_mouse_down_out(cx.listener(|this: &mut Shell, _, _, cx| {
                this.close_chat_menu(cx);
            }))
            .flex()
            .flex_col();
        let menu = match menu_state.page {
            ChatMenuPage::Root => menu
                .child(
                    popover::menu_row(&theme, false, format!("chat-menu-rename-{chat_id}"))
                        .id("chat-menu-rename")
                        .on_click(cx.listener(move |this: &mut Shell, _, _, cx| {
                            this.open_rename_chat(rename_id.clone(), cx)
                        }))
                        .child(icon(icons::PEN).size(px(16.0)).text_color(theme.text_muted))
                        .child(SharedString::from("Rename…")),
                )
                .child(
                    popover::menu_row(&theme, false, format!("chat-menu-pin-{chat_id}"))
                        .id("chat-menu-pin")
                        .on_click(cx.listener(move |this: &mut Shell, _, _, cx| {
                            this.set_chat_pinned(pin_id.clone(), !is_pinned, cx)
                        }))
                        .child(icon(icons::PIN).size(px(16.0)).text_color(theme.text_muted))
                        .child(SharedString::from(if is_pinned { "Unpin" } else { "Pin" })),
                )
                .child(
                    popover::menu_row(&theme, false, format!("chat-menu-archive-{chat_id}"))
                        .id("chat-menu-archive")
                        .on_click(cx.listener(move |this: &mut Shell, _, _, cx| {
                            this.archive_chat(archive_id.clone(), cx)
                        }))
                        .child(
                            icon(icons::ARCHIVE_MINIMALISTIC)
                                .size(px(16.0))
                                .text_color(theme.text_muted),
                        )
                        .child(SharedString::from("Archive")),
                )
                .child(
                    popover::menu_row(&theme, false, format!("chat-menu-copy-{chat_id}"))
                        .id("chat-menu-copy")
                        .on_click(cx.listener(|this: &mut Shell, _, _, cx| this.open_chat_copy_menu(cx)))
                        .child(
                            icon(icons::COPY)
                                .size(px(16.0))
                                .text_color(theme.text_muted),
                        )
                        .child(div().flex_1().child(SharedString::from("Copy")))
                        .child(
                            icon(icons::ALT_ARROW_RIGHT)
                                .size(px(14.0))
                                .text_color(theme.text_muted),
                        ),
                )
                .child(popover::menu_separator())
                .child(
                    popover::menu_row(&theme, false, format!("chat-menu-delete-{chat_id}"))
                        .id("chat-menu-delete")
                        .text_color(theme.danger)
                        .on_click(cx.listener(move |this: &mut Shell, _, _, cx| {
                            this.close_chat_menu(cx);
                            this.delete_confirm = Some(delete_id.clone());
                            cx.notify();
                        }))
                        .child(
                            icon(icons::TRASH_BIN_MINIMALISTIC)
                                .size(px(16.0))
                                .text_color(theme.danger),
                        )
                        .child(SharedString::from("Delete…")),
                ),
            ChatMenuPage::Copy => {
                let chat = self
                    .state
                    .read(cx)
                    .chats
                    .iter()
                    .find(|chat| chat.id == chat_id)
                    .cloned();
                let harness_link = chat
                    .as_ref()
                    .and_then(crate::links::harness_conversation_link);
                let session_id = chat
                    .as_ref()
                    .and_then(|chat| chat.harness_session_id.as_deref())
                    .is_some_and(|id| !id.trim().is_empty());
                let skylark_id = chat_id.clone();
                let harness_id = chat_id.clone();
                let session_chat_id = chat_id.clone();
                menu.child(
                    popover::menu_row(&theme, false, format!("chat-copy-back-{chat_id}"))
                        .id("chat-copy-back")
                        .on_click(cx.listener(|this: &mut Shell, _, _, cx| {
                            if let Some(menu) = this.chat_menu.open_mut() {
                                menu.page = ChatMenuPage::Root;
                                cx.notify();
                            }
                        }))
                        .child(
                            icon(icons::ALT_ARROW_LEFT)
                                .size(px(16.0))
                                .text_color(theme.text_muted),
                        )
                        .child(SharedString::from("Back")),
                )
                .child(popover::menu_separator())
                .child(
                    popover::menu_row(&theme, false, format!("chat-copy-skylark-{chat_id}"))
                        .id("chat-copy-skylark")
                        .on_click(cx.listener(move |this: &mut Shell, _, _, cx| {
                            this.copy_skylark_conversation_link(&skylark_id, cx)
                        }))
                        .child(
                            icon(icons::COPY)
                                .size(px(16.0))
                                .text_color(theme.text_muted),
                        )
                        .child(SharedString::from("Skylark conversation link")),
                )
                .when_some(harness_link, |menu, link| {
                    menu.child(
                        popover::menu_row(
                            &theme,
                            false,
                            format!("chat-copy-harness-{chat_id}"),
                        )
                        .id("chat-copy-harness")
                        .on_click(cx.listener(move |this: &mut Shell, _, _, cx| {
                            this.copy_harness_conversation_link(&harness_id, cx)
                        }))
                        .child(
                            icon(icons::COPY)
                                .size(px(16.0))
                                .text_color(theme.text_muted),
                        )
                        .child(SharedString::from(link.label)),
                    )
                })
                .when(session_id, |menu| {
                    menu.child(
                        popover::menu_row(
                            &theme,
                            false,
                            format!("chat-copy-session-{chat_id}"),
                        )
                        .id("chat-copy-session")
                        .on_click(cx.listener(move |this: &mut Shell, _, _, cx| {
                            this.copy_harness_session_id(&session_chat_id, cx)
                        }))
                        .child(
                            icon(icons::COPY)
                                .size(px(16.0))
                                .text_color(theme.text_muted),
                        )
                        .child(SharedString::from("Harness session ID")),
                    )
                })
            }
        }
        .into_any_element();
        Some(popover::menu_at(
            "chat-context-menu",
            position,
            menu,
            chat_menu_closing,
        ))
    }

    pub(super) fn render_rename_dialog_overlay(
        &mut self,
        viewport: gpui::Size<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let theme = Theme::of(cx).for_popup();
        let dialog = self.rename_dialog.as_mut()?;
        if std::mem::take(&mut dialog.focus_pending) {
            window.focus(&dialog.input.focus_handle(cx), cx);
        }
        let input = dialog.input.clone();
        let card = popover::dialog_card(&theme)
            .on_key_down(cx.listener(|this: &mut Shell, ev: &gpui::KeyDownEvent, _, cx| {
                if ev.keystroke.key == "escape" {
                    this.rename_dialog = None;
                    cx.notify();
                    cx.stop_propagation();
                }
            }))
            .child(popover::dialog_title(&theme, "Rename session"))
            .child(
                div()
                    .mt(px(12.0))
                    .child(popover::dialog_field(input.into_any_element())),
            )
            .child(
                div()
                    .mt(px(16.0))
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap(px(8.0))
                    .child(
                        popover::btn_ghost(&theme, "Cancel", "rename-chat-cancel")
                            .id("rename-chat-cancel")
                            .on_click(cx.listener(|this: &mut Shell, _, _, cx| {
                                this.rename_dialog = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        popover::btn_primary(&theme, "Rename")
                            .id("rename-chat-save")
                            .on_click(
                                cx.listener(|this: &mut Shell, _, _, cx| this.submit_rename_chat(cx)),
                            ),
                    ),
            )
            .into_any_element();
        Some(popover::modal("rename-chat-dialog", viewport, card))
    }

    pub(super) fn render_delete_confirm_dialog_overlay(
        &mut self,
        viewport: gpui::Size<Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let theme = Theme::of(cx).for_popup();
        let chat_id = self.delete_confirm.clone()?;
        let title = transcript::single_line(
            &self
                .state
                .read(cx)
                .chats
                .iter()
                .find(|c| c.id == chat_id)
                .and_then(|c| c.title.clone())
                .unwrap_or_else(|| "New session".into()),
        );
        let card = popover::dialog_card(&theme)
            .child(popover::dialog_title(&theme, "Delete session?"))
            .child(div().mt(px(6.0)).child(popover::dialog_body(
                &theme,
                format!("\u{201C}{title}\u{201D} will be permanently deleted. This can\u{2019}t be undone."),
            )))
            .child(
                div()
                    .mt(px(16.0))
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap(px(8.0))
                    .child(
                        popover::btn_ghost(&theme, "Cancel", "delete-chat-cancel")
                            .id("delete-chat-cancel")
                            .on_click(cx.listener(|this: &mut Shell, _, _, cx| {
                                this.delete_confirm = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        popover::btn_danger(&theme, "Delete")
                            .id("delete-chat-confirm")
                            .on_click(cx.listener(move |this: &mut Shell, _, _, cx| {
                                this.delete_chat(chat_id.clone(), cx)
                            })),
                    ),
            )
            .into_any_element();
        Some(popover::modal("delete-chat-dialog", viewport, card))
    }
}
