//! Terminal tab/session lifecycle and engine stream handling.

use super::*;

impl TerminalPanel {
    pub(super) fn on_state_changed(&mut self, cx: &mut Context<Self>) {
        let selected = self.state.read(cx).selected_chat.clone();
        let switched = selected != self.last_selected;
        if switched {
            self.last_selected = selected;
            self.drag = None;
        }
        if self.open && !self.embedded {
            // Returning to a chat with tabs restores them; a fresh chat (or an
            // engine that only just finished booting) gets its first tab —
            // ensure_tab is idempotent, so calling on every state change is safe.
            // Embedded: surface tabs are explicit — a chat switch just shows
            // that chat's own tabs (or the shell's surface picker).
            self.ensure_tab(cx);
        }
        if switched {
            cx.notify();
        }
    }

    pub(super) fn engine(&self, cx: &App) -> Option<EngineHandle> {
        self.state.read(cx).engine().cloned()
    }

    /// The chat's host device when it differs from the connected engine's own —
    /// the PTY lives on the chat's device (feature-inventory §2.1 "terminals
    /// live on the chat's host device"), so every terminal RPC for a remote
    /// chat needs the `targetDeviceId` passthrough. Without it the local
    /// engine checks the chat's cwd against its OWN filesystem and fails with
    /// "Session working directory is unavailable" (user report).
    pub(super) fn chat_target(&self, chat: &str, cx: &App) -> Option<String> {
        let state = self.state.read(cx);
        let device = state.chats.iter().find(|c| c.id == chat)?.device_id.clone();
        (state.local_device_id.as_deref() != Some(device.as_str())).then_some(device)
    }

    pub(super) fn selected_chat(&self, cx: &App) -> Option<String> {
        self.state.read(cx).selected_chat.clone()
    }

    pub(super) fn ensure_tab(&mut self, cx: &mut Context<Self>) {
        let Some(chat) = self.selected_chat(cx) else {
            return;
        };
        if self.chats.get(&chat).is_none_or(|c| c.tabs.is_empty()) {
            self.open_tab(chat, cx);
        }
    }

    pub(super) fn tab_mut(&mut self, chat: &str, key: u64) -> Option<&mut TerminalTab> {
        self.chats
            .get_mut(chat)?
            .tabs
            .iter_mut()
            .find(|t| t.key == key)
    }

    pub(super) fn active_tab(&self, cx: &App) -> Option<&TerminalTab> {
        let chat = self.state.read(cx).selected_chat.clone()?;
        let tabs = self.chats.get(&chat)?;
        tabs.tabs.get(tabs.active)
    }

    // ---- open / stream lifecycle ----

    pub(super) fn open_tab(&mut self, chat: String, cx: &mut Context<Self>) {
        let Some(engine) = self.engine(cx) else {
            return;
        };
        let tab_no = self
            .chats
            .get(&chat)
            .map_or(1, |entry| entry.tabs.len() + 1);
        let key = self.reserve_tab_for_chat(chat.clone(), format!("Terminal {tab_no}"), cx);
        let target = self.chat_target(&chat, cx);
        let run = Self::spawn_session(chat.clone(), key, engine, target, None, cx);
        if let Some(tab) = self.tab_mut(&chat, key) {
            tab._run = Some(run);
        }
        cx.notify();
    }

    /// OpenTerminal, then pump SubscribeTerminal with reconnect backoff.
    pub(super) fn spawn_session(
        chat: String,
        key: u64,
        engine: EngineHandle,
        target: Option<String>,
        existing_session: Option<TerminalSession>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn(async move |this, cx| {
            let (cols, rows) = this
                .update(cx, |panel, _| {
                    panel
                        .tab_mut(&chat, key)
                        .map(|t| (t.emulator.cols() as u16, t.emulator.rows() as u16))
                        .unwrap_or((80, 24))
                })
                .unwrap_or((80, 24));

            let session = match existing_session {
                Some(session) => session,
                None => match engine
                    .client()
                    .call_as::<TerminalSession>(
                        methods::OPEN_TERMINAL,
                        with_target(
                            serde_json::json!({ "chatId": chat, "cols": cols, "rows": rows }),
                            &target,
                        ),
                    )
                    .await
                {
                    Ok(session) => session,
                    Err(err) => {
                    tracing::warn!(error = %err, "OpenTerminal failed");
                    let _ = this.update(cx, |panel, cx| {
                        if let Some(tab) = panel.tab_mut(&chat, key) {
                            tab.emulator.feed(
                                format!("\x1b[31mfailed to open terminal: {err}\x1b[0m\r\n")
                                    .as_bytes(),
                            );
                            tab.exited = Some(-1);
                            cx.notify();
                        }
                    });
                    return;
                    }
                },
            };
            let terminal_id = session.id.clone();
            let attached = this
                .update(cx, |panel, cx| {
                    if let Some(tab) = panel.tab_mut(&chat, key) {
                        tab.terminal_id = Some(terminal_id.clone());
                        tab.target_device_id = target.clone();
                        cx.notify();
                        true
                    } else {
                        false
                    }
                })
                .unwrap_or(false);
            if !attached {
                // Tab was closed before the open completed — release the PTY.
                let _ = engine
                    .client()
                    .call(
                        methods::CLOSE_TERMINAL,
                        with_target(
                            serde_json::json!({ "terminalId": terminal_id }),
                            &target,
                        ),
                    )
                    .await;
                return;
            }

            let mut attempt: u32 = 0;
            loop {
                let Ok(after_seq) = this.update(cx, |panel, _| {
                    panel.tab_mut(&chat, key).map(|t| t.last_seq)
                }) else {
                    return; // entity released
                };
                let Some(after_seq) = after_seq else { return }; // tab closed

                let subscribed = engine
                    .client()
                    .subscribe(
                        methods::SUBSCRIBE_TERMINAL,
                        with_target(
                            serde_json::json!({ "terminalId": terminal_id, "afterSeq": after_seq }),
                            &target,
                        ),
                    )
                    .await;
                let mut rx = match subscribed {
                    Ok(rx) => rx,
                    Err(err) => {
                        tracing::debug!(error = %err, attempt, "SubscribeTerminal failed; backing off");
                        cx.background_executor()
                            .timer(Duration::from_millis(backoff_ms(attempt)))
                            .await;
                        attempt = attempt.saturating_add(1);
                        continue;
                    }
                };

                while let Some(value) = rx.recv().await {
                    let event: TerminalEvent = match serde_json::from_value(value) {
                        Ok(event) => event,
                        Err(err) => {
                            tracing::warn!(error = %err, "terminal: malformed stream frame");
                            continue;
                        }
                    };
                    attempt = 0;
                    let outcome = this.update(cx, |panel, cx| {
                        panel.apply_stream_event(&chat, key, &engine, event, cx)
                    });
                    match outcome {
                        Ok(StreamDisposition::Continue) => {}
                        Ok(StreamDisposition::Stop) => return,
                        Err(_) => return,
                    }
                }

                // Stream dropped without an exit — reconnect from afterSeq.
                let done = this
                    .update(cx, |panel, _| {
                        panel.tab_mut(&chat, key).map(|t| t.exited.is_some()).unwrap_or(true)
                    })
                    .unwrap_or(true);
                if done {
                    return;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(backoff_ms(attempt)))
                    .await;
                attempt = attempt.saturating_add(1);
            }
        })
    }

    pub(super) fn apply_stream_event(
        &mut self,
        chat: &str,
        key: u64,
        engine: &EngineHandle,
        event: TerminalEvent,
        cx: &mut Context<Self>,
    ) -> StreamDisposition {
        let Some(tab) = self.tab_mut(chat, key) else {
            return StreamDisposition::Stop;
        };
        let target = tab.target_device_id.clone();
        match event {
            TerminalEvent::Data { seq, data } => {
                tab.last_seq = seq;
                let responses = tab.emulator.feed(&decode_base64(&data));
                if !responses.is_empty()
                    && let Some(id) = tab.terminal_id.clone()
                {
                    // Query responses (DSR etc.) go straight back, no coalescing.
                    let engine = engine.clone();
                    let data = encode_base64(&responses);
                    cx.spawn(async move |_, _| {
                        let _ = engine
                            .client()
                            .call(
                                methods::WRITE_TERMINAL,
                                with_target(
                                    serde_json::json!({ "terminalId": id, "data": data }),
                                    &target,
                                ),
                            )
                            .await;
                    })
                    .detach();
                }
                cx.notify();
                StreamDisposition::Continue
            }
            TerminalEvent::Exit { seq, exit_code, .. } => {
                tab.last_seq = seq;
                tab.exited = Some(exit_code);
                tab.emulator.feed(&exit_message(exit_code));
                cx.notify();
                StreamDisposition::Stop
            }
        }
    }
}
