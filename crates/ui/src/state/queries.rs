use super::*;

impl AppState {
    // ---- queries ----

    /// Non-archived chats in sidebar order.
    pub fn visible_chats(&self) -> impl Iterator<Item = &Chat> {
        self.chats.iter().filter(|c| !c.archived)
    }

    pub(crate) fn restore_composer_target(
        &mut self,
        defaults: &crate::settings::composer::ComposerDefaults,
    ) {
        if self.selected_chat.is_some() || self.no_project {
            return;
        }
        if self.selected_device.is_none() {
            self.selected_device = defaults.device.clone();
        }
        if self.selected_space.is_none() {
            self.no_project = defaults.no_project;
            self.selected_space = if defaults.no_project {
                None
            } else {
                defaults.project.clone()
            };
        }
    }

    pub fn selected_space_row(&self) -> Option<&Space> {
        if self.no_project {
            return None;
        }
        let id = self.selected_space.as_deref()?;
        self.spaces.iter().find(|s| s.id == id)
    }

    /// The device the new-session canvas targets: the picked project's host
    /// when one is selected, else the explicit device pick, else this device.
    pub fn effective_device_id(&self) -> Option<String> {
        if let Some(space) = self.selected_space_row() {
            return Some(space.device_id.clone());
        }
        self.selected_device
            .clone()
            .or_else(|| self.local_device_id.clone())
    }

    /// Pick the composer's target device. Keeps the project pick consistent:
    /// a project on another device can't survive the switch — fall back to
    /// the first project on the new device, else "no project".
    pub fn select_device(&mut self, device_id: String, cx: &mut Context<Self>) {
        let project_moves = self
            .selected_space_row()
            .is_some_and(|s| s.device_id != device_id);
        if project_moves {
            let first = self
                .spaces_sorted()
                .iter()
                .find(|s| s.device_id == device_id)
                .map(|s| s.id.clone());
            self.no_project = first.is_none();
            self.selected_space = first;
        }
        self.selected_device = Some(device_id);
        cx.notify();
    }

    pub fn space_row(&self, space_id: &str) -> Option<&Space> {
        self.spaces.iter().find(|s| s.id == space_id)
    }

    /// Spaces in display order — case-insensitive alphabetical, the order
    /// both space selectors (sidebar filter, composer picker) list rows in.
    /// Ties break on id so the order is stable across renders.
    pub fn spaces_sorted(&self) -> Vec<&Space> {
        let mut spaces: Vec<&Space> = self.spaces.iter().collect();
        spaces.sort_by_key(|s| (s.display_name().to_lowercase(), s.id.clone()));
        spaces
    }

    pub fn space_for_chat(&self, chat: &Chat) -> Option<&Space> {
        self.space_row(chat.space_id.as_deref()?)
    }

    /// Non-archived chats of a space in tab (creation) order. Chats with a
    /// dangling/missing `space_id` are invisible by construction.
    pub fn chats_in_space(&self, space_id: &str) -> Vec<&Chat> {
        let mut chats: Vec<&Chat> = self
            .visible_chats()
            .filter(|c| c.space_id.as_deref() == Some(space_id))
            .collect();
        sort_tabs(&mut chats);
        chats
    }

    pub fn device_name(&self, device_id: &str) -> Option<&str> {
        self.devices
            .iter()
            .find(|d| d.id == device_id)
            .map(|d| d.name.as_str())
    }

    /// Host-presence check: is this device's 15s presence heartbeat fresh?
    /// Distinguishes "host offline" (its queued work syncs when it returns)
    /// from slow sync. The local device is trivially online; unknown devices
    /// get the benefit of the doubt (no evidence — don't cry wolf).
    pub fn device_online(&self, device_id: &str, now: DateTime<Utc>) -> bool {
        if self.local_device_id.as_deref() == Some(device_id) {
            return true;
        }
        match self.devices.iter().find(|d| d.id == device_id) {
            Some(d) => crate::settings::devices::device_online(d.last_seen_at, now),
            None => true,
        }
    }

    /// The "@ device" tag for a space — shared by the space pickers' rows,
    /// the sidebar filter trigger, and the composer's space chip. Returns
    /// `(tag, offline)`; staleness renders as a disconnected GLYPH at the
    /// call sites (user request), never words in the tag.
    pub fn space_device_tag(&self, space: &Space, now: DateTime<Utc>) -> (String, bool) {
        let offline = !self.device_online(&space.device_id, now);
        let device = self
            .device_name(&space.device_id)
            .unwrap_or("Unknown device");
        (format!("@ {device}"), offline)
    }

    /// Does the selected space's folder have git? Drives the branch picker and
    /// the diff sidebar (owner-stamped, synced — no RPC).
    pub fn selected_space_git(&self) -> bool {
        self.selected_space_row().is_some_and(|s| s.git_detected)
    }

    /// Full display status for a chat (tab dots, Active list). A send in
    /// flight ([`Self::begin_pending_send`]) reads as Working — the queued
    /// command is as good as running.
    pub fn display_status_for(&self, chat: &Chat, now: DateTime<Utc>) -> ChatIndicator {
        if self.send_pending(&chat.id, now) {
            return ChatIndicator::Working;
        }
        display_status(chat, self.session_for(&chat.id), now)
    }

    /// The sidebar's Sessions list: every non-archived chat of a LIVE space,
    /// on any device — idle included — in pure recency order (status drives
    /// the dot, never the position; see [`sort_active`]).
    pub fn overview_chats(&self, now: DateTime<Utc>) -> Vec<(ChatIndicator, &Chat)> {
        let mut rows: Vec<(ChatIndicator, &Chat)> = self
            .visible_chats()
            .filter(|c| match c.space_id.as_deref() {
                // Project-less sessions are first-class rows.
                None => true,
                Some(id) => self.space_row(id).is_some(),
            })
            .map(|c| (self.display_status_for(c, now), c))
            .collect();
        sort_active(&mut rows);
        rows
    }

    /// The sidebar's active list exactly as it is drawn: [`Self::overview_chats`]
    /// narrowed to the current project filter. The jump shortcuts and their
    /// hints both count positions here, so neither can drift from the rows on
    /// screen.
    pub fn sidebar_chats(
        &self,
        now: DateTime<Utc>,
        space_filter: Option<&str>,
    ) -> Vec<(ChatIndicator, &Chat)> {
        self.overview_chats(now)
            .into_iter()
            .filter(|(_, chat)| match space_filter {
                Some(space_id) => chat.space_id.as_deref() == Some(space_id),
                None => true,
            })
            .collect()
    }

    pub fn session_for(&self, chat_id: &str) -> Option<&Session> {
        self.sessions.iter().find(|s| s.chat_id == chat_id)
    }

    /// Staleness-checked status dot for a chat row. A send in flight reads as
    /// Working (see [`Self::display_status_for`]).
    pub fn indicator_for(&self, chat_id: &str, now: DateTime<Utc>) -> Indicator {
        if self.send_pending(chat_id, now) {
            return Indicator::Working;
        }
        effective_indicator(self.session_for(chat_id), now)
    }

    pub fn selected_chat_row(&self) -> Option<&Chat> {
        let id = self.selected_chat.as_deref()?;
        self.chats.iter().find(|c| c.id == id)
    }

    /// The chat the Archive session shortcut acts on: the selected one, unless
    /// it is already archived. The shortcut archives and never unarchives, so
    /// an archived chat is left alone. Pure.
    pub fn archivable_selected_chat(&self) -> Option<&str> {
        self.selected_chat_row()
            .filter(|chat| !chat.archived)
            .map(|chat| chat.id.as_str())
    }

    /// Latest valid PR for a chat, rechecked against device, checkout, cwd and branch.
    pub fn change_request_for_chat(&self, chat: &Chat) -> Option<&ChangeRequestSummary> {
        self.change_requests
            .change_request_for_chat(chat, &self.spaces)
    }

    pub fn gate(&self) -> GatePhase {
        gate_phase(&self.connection, self.workspace_scope, self.auth.as_ref())
    }

    pub fn engine(&self) -> Option<&EngineHandle> {
        self.engine.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn set_test_engine(&mut self, handle: EngineHandle) {
        self.engine = Some(handle);
    }

    /// Drop every account-scoped view and subscription after its runtime has
    /// stopped. The next bootstrap must never render rows from the previous
    /// account while the local profile is opening.
    pub fn prepare_runtime_replacement(&mut self, cx: &mut Context<Self>) {
        self.engine = None;
        self.watch_tasks.clear();
        self.transcript_task = None;
        self.change_request_tasks.clear();
        self.change_requests = ChangeRequestClientState::default();
        self.connection = ConnectionStatus::Connecting;
        self.workspace_scope = None;
        self.auth = None;
        self.devices.clear();
        self.device_presentation = None;
        self.session_presence_presentation.clear();
        self.spaces.clear();
        self.chats.clear();
        self.sessions.clear();
        self.sidebar_preferences = SidebarPreferencesState::default();
        self.session_presentation = None;
        self.selected_space = None;
        self.no_project = false;
        self.selected_device = None;
        self.selected_chat = None;
        self.auto_selected = false;
        self.chats_synced = false;
        self.spaces_synced = false;
        self.transcript.clear();
        self.transcript_baselines.clear();
        self.transcript_cache.clear();
        self.prepared_transcripts.clear();
        self.context_usage = None;
        self.transcript_revision = self.transcript_revision.wrapping_add(1);
        self.transcript_replayed = false;
        self.echoes.clear();
        self.pending_sends.clear();
        self.upload_progress = None;
        self.transfers.clear();
        self.local_device_id = None;
        self.update = None;
        cx.notify();
    }

    // ---- gpui glue ----

    /// Kick off (or retry) the engine bootstrap: probe → connect-or-embed on
    /// tokio, then attach subscriptions. Safe to call again after `Failed`.
    pub fn bootstrap(state: Entity<AppState>, config: EngineBootConfig, cx: &mut App) {
        let data_dir = config.data_dir.clone();
        state.update(cx, |s, cx| {
            s.connection = ConnectionStatus::Connecting;
            s.workspace_scope = None;
            s.auth = None;
            s.data_dir = Some(data_dir);
            cx.notify();
        });
        let boot = Tokio::spawn(cx, EngineHandle::bootstrap(config));
        cx.spawn(async move |cx| {
            let outcome = match boot.await {
                Ok(Ok(handle)) => Ok(handle),
                Ok(Err(err)) => Err(format!("{err:#}")),
                Err(join_err) => Err(join_err.to_string()),
            };
            // NB: at the pinned rev `Entity::update(&mut AsyncApp)` returns the
            // closure's value directly (no Result) — AsyncApp implements
            // AppContext like App does.
            state.update(cx, |s, cx| match outcome {
                Ok(handle) => s.attach_engine(handle, cx),
                Err(message) => {
                    tracing::error!(%message, "engine bootstrap failed");
                    s.connection = ConnectionStatus::Failed(message);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Wire the connected engine: mark Ready and start the standing watches.
    /// Methods the engine doesn't serve yet (chats/devices/auth land with the
    /// workspace doc in M4) fail their subscribe and are skipped gracefully.
    fn attach_engine(&mut self, handle: EngineHandle, cx: &mut Context<Self>) {
        // The attachment notification precedes the first connectivity frame.
        // Make that bootstrap gap explicit so the shell resets its alert
        // baseline instead of comparing the new runtime with the old one.
        self.connectivity_observed = false;
        let engine_info = handle.engine_info();
        self.workspace_scope = Some(engine_info.workspace_scope);
        self.local_device_id = Some(engine_info.device_id.clone());
        self.engine = Some(handle.clone());
        let mut watch_tasks = Vec::with_capacity(10);
        if let Some(task) = spawn_deferred_engine_watch(cx, handle.clone()) {
            watch_tasks.push(task);
        }
        watch_tasks.extend([
            spawn_watch(
                cx,
                handle.clone(),
                methods::WATCH_SESSIONS,
                AppState::apply_sessions,
            ),
            spawn_chats_watch(cx, handle.clone()),
            spawn_watch(
                cx,
                handle.clone(),
                methods::WATCH_SIDEBAR_PREFERENCES,
                AppState::apply_sidebar_preferences,
            ),
            spawn_watch(
                cx,
                handle.clone(),
                methods::WATCH_DEVICES,
                AppState::apply_devices,
            ),
            spawn_watch(
                cx,
                handle.clone(),
                methods::WATCH_CONNECTIVITY,
                |state, value| {
                    state.apply_connectivity(value);
                    true
                },
            ),
            spawn_watch(
                cx,
                handle.clone(),
                methods::WATCH_TRANSFERS,
                |state, value| {
                    state.apply_transfers(value);
                    true
                },
            ),
            spawn_watch(cx, handle.clone(), methods::WATCH_SPACES, |state, value| {
                state.apply_spaces(value);
                true
            }),
            // Auth frames parse tolerantly — engine and proto tags differ today.
            spawn_watch(cx, handle.clone(), methods::AUTH_STATUS, |state, value| {
                state.apply_auth_value(value);
                true
            }),
            spawn_local_device_probe(cx, handle.clone()),
        ]);
        if !skylark_proto::LOCAL_ONLY_BUILD {
            watch_tasks.push(spawn_watch(cx, handle.clone(), methods::UPDATE_STATUS, |state, value| {
                state.apply_update(value);
                true
            }));
        }
        self.watch_tasks = watch_tasks;
        self.reconcile_change_request_watches(cx);
        // EngineInfo is part of the attachment boundary: views must know which
        // data profile they reached before they are allowed to render Ready.
        self.connection = ConnectionStatus::Ready;
        // Re-subscribe the transcript if a chat was already selected (reconnect path).
        if let Some(chat_id) = self.selected_chat.clone() {
            self.transcript_task =
                Some(spawn_transcript_watch(cx, handle.clone(), chat_id.clone()));
            if handle
                .engine_info()
                .supports(skylark_proto::capabilities::MESSAGE_QUEUE_V1)
            {
                self.queue_task = Some(spawn_queue_watch(cx, handle, chat_id));
            }
        }
        cx.notify();
    }

    pub(super) fn reconcile_change_request_watches(&mut self, cx: &mut Context<Self>) {
        let Some(handle) = self.engine.clone() else {
            self.change_request_tasks.clear();
            return;
        };
        let targets = if self.change_requests_visible {
            desired_watch_targets(&self.chats, &self.spaces, |device| {
                !self.change_requests.is_supported(device)
            })
        } else {
            HashSet::new()
        };

        self.change_request_tasks
            .retain(|target, _| targets.contains(target));
        self.change_requests.retain_targets(&targets);

        let local_device_id = self.local_device_id.clone();
        for target in targets {
            if self.change_request_tasks.contains_key(&target) {
                continue;
            }
            let task = spawn_change_request_watch(
                cx,
                handle.clone(),
                target.clone(),
                local_device_id.clone(),
            );
            self.change_request_tasks.insert(target, task);
        }
    }

    pub fn set_change_requests_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.change_requests_visible != visible {
            self.change_requests_visible = visible;
            self.reconcile_change_request_watches(cx);
        }
    }

    pub fn open_deep_link(&mut self, url: &str, cx: &mut Context<Self>) {
        match crate::links::parse_skylark_conversation_link(url) {
            Ok(link) => {
                self.pending_deep_link = Some(link);
                self.apply_pending_deep_link(cx);
            }
            Err(error) => self.deep_link_notice = Some(error.to_string()),
        }
        cx.notify();
    }

    pub(super) fn apply_pending_deep_link(&mut self, cx: &mut Context<Self>) {
        let Some(link) = self.pending_deep_link.clone() else {
            return;
        };
        let Some(locator) = crate::links::workspace_locator(
            self.workspace_scope,
            self.auth.as_ref(),
            self.local_device_id.as_deref(),
        ) else {
            return;
        };
        if locator != link.workspace {
            self.pending_deep_link = None;
            self.deep_link_notice =
                Some("This conversation link belongs to another workspace".into());
            return;
        }
        if self.chats.iter().any(|chat| chat.id == link.chat_id) {
            self.pending_deep_link = None;
            self.select_chat(Some(link.chat_id), cx);
        } else if self.chats_synced {
            self.pending_deep_link = None;
            self.deep_link_notice = Some("The linked conversation was not found".into());
        }
    }

    pub fn take_deep_link_notice(&mut self) -> Option<String> {
        self.deep_link_notice.take()
    }

    /// Select a chat (or clear). Swaps the per-chat doc-transcript subscription:
    /// dropping the old task drops its stream receiver, which cancels the doc
    /// watch server-side. Selecting a chat also lands in its space and marks it
    /// seen (a global-list click must switch the tab strip too).
    pub fn select_chat(&mut self, chat_id: Option<String>, cx: &mut Context<Self>) {
        if self.selected_chat == chat_id {
            // Re-selecting still clears a fresh "completed" badge.
            if let Some(id) = chat_id {
                self.mark_chat_seen(&id, cx);
            }
            return;
        }
        // Take the destination before trimming: switching to the oldest warm
        // transcript must not evict the very entry we are about to display.
        let cached = self
            .transcript_cache
            .iter()
            .position(|cached| Some(&cached.chat_id) == chat_id.as_ref())
            .and_then(|index| self.transcript_cache.remove(index));
        if let Some(previous) = &self.selected_chat {
            let old_baseline = self.transcript_baselines.remove(previous);
            cx.background_executor()
                .spawn(async move {
                    drop(old_baseline);
                })
                .detach();
            let prepared = self.prepared_transcripts.remove(previous);
            if self.transcript_replayed {
                let entries = std::mem::take(&mut self.transcript);
                let bytes = prepared.as_ref().map_or_else(
                    || {
                        entries
                            .iter()
                            .map(|entry| {
                                std::mem::size_of::<SessionMessageEntry>()
                                    + entry.id.len()
                                    + entry
                                        .parts
                                        .iter()
                                        .map(|part| {
                                            std::mem::size_of::<skylark_doc::MessagePart>()
                                                + part.byte_len()
                                        })
                                        .sum::<usize>()
                            })
                            .sum()
                    },
                    |p| p.bytes,
                );
                self.transcript_cache.push_back(CachedTranscript {
                    prepared,
                    chat_id: previous.clone(),
                    entries,
                    context_usage: self.context_usage,
                    bytes,
                });
                while self.transcript_cache.len() > TRANSCRIPT_CACHE_CAP
                    || self
                        .transcript_cache
                        .iter()
                        .map(|cached| cached.bytes)
                        .sum::<usize>()
                        > TRANSCRIPT_CACHE_BYTES
                {
                    let evicted = self.transcript_cache.pop_front();
                    cx.background_executor()
                        .spawn(async move {
                            drop(evicted);
                        })
                        .detach();
                }
            }
        }
        self.selected_chat = chat_id.clone();
        self.auto_selected = true;
        self.transcript.clear();
        self.context_usage = None;
        self.transcript_revision = self.transcript_revision.wrapping_add(1);
        self.transcript_replayed = false;
        if let Some(cached) = cached {
            self.transcript_baselines.insert(
                cached.chat_id.clone(),
                cached
                    .prepared
                    .as_ref()
                    .map(|p| p.navigation_baseline.clone())
                    .unwrap_or_else(|| {
                        Arc::new(skylark_doc::TranscriptBaseline::capture(&cached.entries))
                    }),
            );
            if let Some(prepared) = cached.prepared {
                self.prepared_transcripts.insert(cached.chat_id, prepared);
            }
            self.transcript = cached.entries;
            self.context_usage = cached.context_usage;
            self.transcript_replayed = true;
        }
        self.transcript_task = None;
        self.queue.clear();
        self.queue_task = None;
        if let Some(id) = chat_id.as_deref() {
            // A chat implies its project (or the lack of one); `select_chat(None)`
            // (the new-session canvas) keeps the current project pick.
            if let Some(chat) = self.chats.iter().find(|c| c.id == id) {
                match chat.space_id.clone() {
                    Some(space_id) => {
                        self.selected_space = Some(space_id);
                        self.no_project = false;
                    }
                    None => {
                        self.selected_space = None;
                        self.no_project = true;
                        self.selected_device = Some(chat.device_id.clone());
                    }
                }
            }
            self.mark_chat_seen(id, cx);
        }
        if let (Some(chat_id), Some(handle)) = (chat_id, self.engine.clone()) {
            self.transcript_task =
                Some(spawn_transcript_watch(cx, handle.clone(), chat_id.clone()));
            if handle
                .engine_info()
                .supports(skylark_proto::capabilities::MESSAGE_QUEUE_V1)
            {
                self.queue_task = Some(spawn_queue_watch(cx, handle, chat_id));
            }
        }
        cx.notify();
    }

    /// Replace the selected chat's queue subscription without clearing its
    /// current projection. This is used after an optimistic mutation fails:
    /// the authoritative opening frame repairs the local list even though the
    /// document itself did not change and therefore emitted no new frame.
    pub(crate) fn refresh_selected_queue(&mut self, cx: &mut Context<Self>) {
        self.queue_task = None;
        let (Some(chat_id), Some(handle)) = (self.selected_chat.clone(), self.engine.clone())
        else {
            return;
        };
        if handle
            .engine_info()
            .supports(skylark_proto::capabilities::MESSAGE_QUEUE_V1)
        {
            self.queue_task = Some(spawn_queue_watch(cx, handle, chat_id));
        }
    }

    /// Select a project; the caller (shell) decides which chat to land on.
    /// `Some` clears a "Don't work in a project" opt-out and re-aims the
    /// device pick at the project's host; `None` IS that opt-out.
    pub fn select_space(&mut self, space_id: Option<String>, cx: &mut Context<Self>) {
        match &space_id {
            Some(id) => {
                self.no_project = false;
                if let Some(device) = self.space_row(id).map(|s| s.device_id.clone()) {
                    self.selected_device = Some(device);
                }
            }
            None => {
                self.selected_device = self.effective_device_id();
                self.no_project = true;
            }
        }
        if self.selected_space == space_id && space_id.is_some() {
            cx.notify();
            return;
        }
        self.selected_space = space_id;
        cx.notify();
    }

    /// Synced seen marker: only fires when the chat is currently unseen
    /// (idempotence — no mutate spam), stamps the local row optimistically so
    /// the LWW round-trip is invisible, and fire-and-forgets the mutate.
    /// Window-focus liveness sweep: ask the engine to probe every open room
    /// (workspace + chat docs). Fire-and-forget; each room ignores the hint
    /// unless it has been broadcast-quiet ≥30s, so spamming is harmless.
    pub fn probe_sync(&mut self, cx: &mut Context<Self>) {
        let Some(handle) = self.engine.clone() else {
            return;
        };
        cx.spawn(async move |_, _| {
            let params = serde_json::json!({});
            if let Err(err) = handle.client().call(methods::PROBE_SYNC, params).await {
                tracing::debug!(error = %err, "probe sync failed");
            }
        })
        .detach();
    }

    pub fn mark_chat_seen(&mut self, chat_id: &str, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.iter_mut().find(|c| c.id == chat_id) else {
            return;
        };
        if !chat.unseen() {
            return;
        }
        chat.last_seen_at = Some(Utc::now());
        cx.notify();
        let Some(handle) = self.engine.clone() else {
            return;
        };
        let chat_id = chat_id.to_string();
        cx.spawn(async move |_, _| {
            let params = serde_json::json!({ "op": "markChatSeen", "chatId": chat_id });
            if let Err(err) = handle.client().call(methods::MUTATE, params).await {
                tracing::warn!(chat = %chat_id, error = %err, "markChatSeen failed");
            }
        })
        .detach();
    }
}
