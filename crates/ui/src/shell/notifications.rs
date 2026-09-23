//! State event listening, sound chimes, OS notifications, and debug hooks.

use super::*;
use chrono::Utc;
use std::time::Duration;

impl Shell {
    /// Fire a debug demo upload when requested by `SKYLARK_DEMO_UPLOAD=<pct>:<image path>`.
    pub(super) fn handle_debug_upload(&mut self, state: &Entity<AppState>, cx: &mut Context<Self>) {
        if let Some(spec) = self.debug_upload.clone()
            && let Some(chat_id) = state.read(cx).selected_chat.clone()
        {
            self.debug_upload = None;
            if let Some((pct, img_path)) = spec.split_once(':')
                && let Ok(pct) = pct.parse::<u64>()
                && let Ok(att) = crate::attachments::stage_file(std::path::Path::new(img_path))
            {
                let pending_path = format!("pending/{}/{}", att.id, att.name);
                let device_ids: Vec<String> = {
                    let s = state.read(cx);
                    s.selected_chat_row()
                        .map(|c| c.device_id.clone())
                        .into_iter()
                        .chain(s.local_device_id.clone())
                        .chain(Some("local".to_string()))
                        .collect()
                };
                for device_id in &device_ids {
                    crate::attachments::seed_attachment(
                        device_id,
                        &pending_path,
                        &att.name,
                        att.image.clone(),
                    );
                }
                let text = crate::attachments::with_attachments(
                    "Here is the screenshot of the bug.",
                    std::slice::from_ref(&pending_path),
                );
                let echo = skylark_doc::SessionMessageEntry {
                    id: "demo-upload-echo".into(),
                    role: skylark_doc::MessageRole::User,
                    parts: vec![skylark_doc::MessagePart::Text {
                        id: "t0".into(),
                        text,
                    }],
                    created_at: chrono::Utc::now().timestamp_millis(),
                    device_id: "local".into(),
                    status: None,
                    continuation_of: None,
                };
                state.update(cx, |s, cx| {
                    s.push_echo(&chat_id, echo);
                    s.begin_upload_progress(
                        100,
                        std::sync::Arc::new(std::sync::atomic::AtomicU64::new(pct)),
                    );
                    cx.notify();
                });
            }
        }
    }

    /// Pop the requested debug dialog once prerequisites land.
    pub(super) fn handle_debug_dialog(&mut self, state: &Entity<AppState>, cx: &mut Context<Self>) {
        // Capture knob: the add-space palette needs only the device registry.
        if self.debug_dialog.as_deref() == Some("add-space") && !state.read(cx).devices.is_empty() {
            self.debug_dialog = None;
            self.open_add_space(cx);
        }
        // Capture knob: pop the requested dialog once chats have landed.
        if let Some(which) = self.debug_dialog.clone()
            && let Some(first) = state.read(cx).chats.first().map(|c| c.id.clone())
        {
            self.debug_dialog = None;
            match which.as_str() {
                "rename" => self.open_rename_chat(first, cx),
                "delete" => {
                    self.delete_confirm = Some(first);
                }
                _ => {}
            }
        }
    }

    /// Banners and chimes share one session detector. Completion markers survive
    /// queue handoffs and never advance for interrupts or stale activity.
    /// A row's first appearance seeds the baseline silently (boot/replay).
    /// Pending sends consume completion changes silently, while questions
    /// still ring immediately. Output settings do not affect the baseline.
    pub(super) fn dispatch_session_notifications(
        &mut self,
        state: &Entity<AppState>,
        cx: &mut Context<Self>,
    ) {
        let now = Utc::now();
        type Ping = (
            String,
            crate::sound::SessionNotificationState,
            bool,
            Option<String>,
        );
        let (sessions, connectivity, connectivity_observed) = {
            let state = state.read(cx);
            let sessions: Vec<Ping> = state
                .sessions
                .iter()
                .map(|s| {
                    let status = crate::sound::SessionNotificationState::new(s, now);
                    let send_pending = state.send_pending(&s.chat_id, now);
                    let title = state
                        .chats
                        .iter()
                        .find(|c| c.id == s.chat_id)
                        .and_then(|c| c.title.clone());
                    (s.chat_id.clone(), status, send_pending, title)
                })
                .collect();
            (
                sessions,
                state.connectivity.state,
                state.connectivity_observed,
            )
        };
        // Background-only banners: `active_window()` is app-level (any
        // Skylark window being key), so a ping for a *background chat* in a
        // focused app still stays a chime — you're already looking at
        // Skylark; the sidebar dot carries the rest.
        let app_focused = cx.active_window().is_some();
        for (chat_id, status, send_pending, title) in sessions {
            let prev = self.sound_prev.insert(chat_id.clone(), status.clone());
            if let Some(prev) = prev
                && let Some(sound) = status.sound_since(&prev, send_pending)
            {
                if self.settings.session_sound_enabled(sound) {
                    let should_play = sound != crate::sound::Sound::Attention
                        || self
                            .attention_sound_gate
                            .should_play(std::time::Instant::now());
                    if should_play {
                        crate::sound::play(sound);
                    }
                }
                if self.settings.notifications_enabled
                    && !(self.settings.notifications_background_only && app_focused)
                {
                    let title = title.unwrap_or_else(|| "New session".into());
                    let body = match sound {
                        crate::sound::Sound::Done => "Run finished",
                        crate::sound::Sound::Request => "Waiting on your input",
                        crate::sound::Sound::Attention => "Run failed",
                    };
                    crate::notify::post(&title, body, Some(&chat_id));
                }
            }
        }
        if let Some(sound) = self.connectivity_notifications.update(
            connectivity,
            connectivity_observed,
            std::time::Instant::now(),
        ) {
            if self.settings.session_sound_enabled(sound)
                && self
                    .attention_sound_gate
                    .should_play(std::time::Instant::now())
            {
                crate::sound::play(sound);
            }
            if self.settings.notifications_enabled
                && !(self.settings.notifications_background_only && app_focused)
            {
                let body = match connectivity {
                    skylark_proto::ConnectivityState::Offline => "Your device is offline",
                    _ => "Skylark is trying to reconnect",
                };
                crate::notify::post("Connection unavailable", body, None);
            }
        }
    }

    pub(super) fn on_state_changed(&mut self, state: &Entity<AppState>, cx: &mut Context<Self>) {
        if let Some(notice) = state.update(cx, |state, _| state.take_deep_link_notice()) {
            self.sidebar_notice = Some(notice.into());
        }
        let next_sync_flow = if skylark_proto::LOCAL_ONLY_BUILD {
            SyncFlow::Idle
        } else {
            let state = state.read(cx);
            sync_flow_after_auth(self.sync_flow, state.workspace_scope, state.auth.as_ref())
        };
        if next_sync_flow != self.sync_flow {
            self.sync_flow = next_sync_flow;
            if matches!(
                self.sync_flow,
                SyncFlow::RestartPending { .. } | SyncFlow::SwitchOffer { .. }
            ) {
                self.org = None;
            }
        }
        // The in-place local→synced switch: once the replacement runtime is
        // attached and Ready, kick the import (or finish) from here.
        self.drive_sync_switch(cx);
        let signed_out_synced = {
            let state = state.read(cx);
            state.workspace_scope == Some(WorkspaceScope::Synced)
                && matches!(state.auth, Some(AuthState::SignedOut))
        };
        // AuthStatus is shared by every viewport. Whichever viewport owns the
        // embedded runtime drains it; remote viewports request daemon shutdown
        // and all of them independently reattach to the new local runtime.
        if signed_out_synced && self.runtime_change_task.is_none() {
            self.start_local_runtime_transition(false, cx);
        }

        self.handle_debug_dialog(state, cx);
        self.handle_debug_upload(state, cx);
        self.dispatch_session_notifications(state, cx);

        // An explicit projectless canvas must be visible in the sidebar:
        // retaining a project filter would hide the session on its first send.
        if state.read(cx).no_project
            && state.read(cx).selected_chat.is_none()
            && self.settings.space_filter.take().is_some()
        {
            self.schedule_save(cx);
        }
        // Boot: restore the last selected space once the first spaces frame
        // lands (a still-existing row wins over the auto-selected first one;
        // the boot-auto-selected chat's own space wins over both — selecting a
        // chat implies its space, which `select_chat` already applied).
        if !self.space_boot_applied && !state.read(cx).spaces.is_empty() {
            self.space_boot_applied = true;
            if state.read(cx).selected_chat.is_none() {
                // A set sidebar filter is an explicit standing choice — the
                // canvas defaults (project AND its device) follow it, even
                // over a remembered "no project" opt-out. Otherwise the last
                // selected project stands, unless opted out.
                let exists = |id: &String| state.read(cx).space_row(id).is_some();
                let filter = self.settings.space_filter.clone().filter(&exists);
                let target = match filter {
                    Some(filter) => Some(filter),
                    None if !state.read(cx).no_project => {
                        self.settings.last_space_id.clone().filter(&exists)
                    }
                    None => None,
                };
                if target.is_some() {
                    state.update(cx, |s, cx| s.select_space(target, cx));
                }
            }
        }
        // Persist the selected space (the new-tab fallback under "All").
        {
            let selected_space = state.read(cx).selected_space.clone();
            if selected_space != self.settings.last_space_id && selected_space.is_some() {
                self.settings.last_space_id = selected_space;
                self.schedule_save(cx);
            }
        }
        // Boot landing: the most recent session once the first chats frame
        // syncs (manual selection wins).
        self.boot_select_chat(cx);
        // Heal a dangling sidebar filter (space deleted, possibly elsewhere):
        // fall back to "All" rather than filtering everything out.
        if state.read(cx).spaces_synced
            && let Some(filter) = self.settings.space_filter.clone()
            && state.read(cx).space_row(&filter).is_none()
        {
            self.settings.space_filter = None;
            self.schedule_save(cx);
        }
        self.reconcile_sidebar_pins(cx);
        if !self.pinned_session_drag_is_valid(cx) {
            self.cancel_pinned_session_drag(cx);
        }
        // Chat switch: restore THAT chat's panel state (per-session open flags;
        // snap, no tween — the panels belong to the destination chat).
        let selected = state.read(cx).selected_chat.clone().unwrap_or_default();
        if !selected.is_empty() {
            self.last_appshot_chat = Some(selected.clone());
        }
        if selected != self.active_chat {
            self.suspend_file_images(cx);
            self.active_chat = selected;
            // Route history: a chat switch is a navigation. The very first
            // selection off the untouched boot canvas REPLACES that entry —
            // skylark's `/` route redirected into the last-used chat, leaving no
            // dead Back target. Walking history lands here too, but the
            // destination already equals `current()`, so the push dedups.
            if matches!(self.route, Route::Chat) {
                let entry = NavEntry::Chat(self.active_chat.clone());
                if self.nav.len() == 1 && *self.nav.current() == NavEntry::Chat(String::new()) {
                    self.nav.replace(entry);
                } else {
                    self.nav.push(entry);
                }
            }
            self.right_tween = None;
            self.right_takeover_content_tween = None;
            self.main_takeover_tween = None;
            self.terminal_tween = None;
            let panels = self.panels.get(&self.panel_key(cx));
            if let Some(panel) = self.terminal.clone() {
                panel.update(cx, |panel, cx| panel.set_open(panels.terminal_open, cx));
            }
            if panels.changes_open
                && let RightSurface::Diff(id) = self.resolved_right_active(cx)
                && let Some(changes) = self.diffs.get(&id).cloned()
            {
                changes.update(cx, |changes, cx| changes.ensure_content(cx));
            }
        }
        match state.read(cx).connection {
            ConnectionStatus::Ready => {
                if self.splash == SplashPhase::Visible {
                    self.splash = SplashPhase::FadingOut;
                    self.splash_task = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor()
                            .timer(SPLASH_OUT.total() + Duration::from_millis(30))
                            .await;
                        this.update(cx, |shell, cx| {
                            shell.splash = SplashPhase::Gone;
                            cx.notify();
                        })
                        .ok();
                    }));
                }
            }
            // Reveal the gate card immediately; the splash never returns mid-session.
            ConnectionStatus::Failed(_) => self.splash = SplashPhase::Gone,
            ConnectionStatus::Connecting => {}
        }
    }
}
