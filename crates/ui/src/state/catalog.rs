//! AppState sidebar, catalog, device, auth, and delivery reducers.

use super::*;

impl AppState {
    pub(crate) fn apply_sidebar_preferences(&mut self, value: SidebarPreferencesState) -> bool {
        if value.revision < self.sidebar_preferences.revision || value == self.sidebar_preferences {
            return false;
        }
        self.sidebar_preferences = value;
        true
    }

    pub fn apply_chats(&mut self, mut chats: Vec<Chat>) {
        sort_chats(&mut chats);
        self.chats = chats;
        self.chats_synced = true;
        self.transcript_cache
            .retain(|cached| self.chats.iter().any(|c| c.id == cached.chat_id));
        if let Some(selected) = &self.selected_chat
            && !self.chats.iter().any(|c| &c.id == selected)
        {
            // Selected chat vanished (deleted elsewhere): drop selection + transcript.
            self.transcript_baselines.remove(selected);
            self.prepared_transcripts.remove(selected);
            self.selected_chat = None;
            self.transcript.clear();
            self.context_usage = None;
            self.transcript_revision = self.transcript_revision.wrapping_add(1);
            self.transcript_replayed = false;
            self.transcript_task = None;
            self.queue.clear();
            self.queue_task = None;
        }
    }

    pub fn apply_sessions(&mut self, sessions: Vec<Session>) -> bool {
        self.apply_sessions_at(sessions, Utc::now())
    }

    fn apply_sessions_at(&mut self, sessions: Vec<Session>, now: DateTime<Utc>) -> bool {
        let presence: Vec<_> = sessions
            .iter()
            .map(|session| effective_indicator(Some(session), now))
            .collect();
        let presentation: Vec<_> = sessions
            .iter()
            .map(|session| {
                let mut metadata = session.clone();
                // The timestamp is a liveness lease, not visible text. Keep its
                // effective indicator in the key and retain the actual value below.
                metadata.updated_at = DateTime::<Utc>::UNIX_EPOCH;
                metadata
            })
            .collect();
        let changed = self.session_presentation.as_ref() != Some(&presentation)
            || self.session_presence_presentation != presence;
        self.session_presentation = Some(presentation);
        self.session_presence_presentation = presence;
        self.sessions = sessions;
        changed
    }

    pub fn apply_spaces(&mut self, mut spaces: Vec<Space>) {
        sort_spaces(&mut spaces);
        self.spaces = spaces;
        self.spaces_synced = true;
        if self.no_project {
            self.selected_space = None;
            return;
        }
        // Heal a vanished selection (project deleted elsewhere): fall back to
        // the first project; its chats died with it, so a matching chat
        // selection is healed by the accompanying chats frame (`apply_chats`).
        // The picker lists projects per-device, so healing prefers one on the
        // picked device — a global fallback would silently re-aim the canvas
        // at another machine.
        if let Some(selected) = &self.selected_space
            && !self.spaces.iter().any(|s| &s.id == selected)
        {
            self.selected_space = self.first_space_on_picked_device();
        }
        // First frame with no selection yet: pick the first project so the
        // canvas never boots project-less by accident — unless the user
        // deliberately opted out.
        if self.selected_space.is_none() && !self.no_project {
            self.selected_space = self.first_space_on_picked_device();
        }
    }

    /// Optimistic local echo of a `setChatConfig` mutate: stamp the row now so
    /// the chips update on click; the next chats watch frame carries the same
    /// value once the engine applies the LWW write.
    pub fn apply_chat_config(&mut self, chat_id: &str, config: zeron_proto::ChatConfig) {
        if let Some(chat) = self.chats.iter_mut().find(|c| c.id == chat_id) {
            chat.config = Some(config);
        }
    }

    pub fn apply_connectivity(&mut self, connectivity: zeron_proto::Connectivity) {
        self.connectivity = connectivity;
        self.connectivity_observed = true;
    }

    /// Is this chat's delivery path degraded — will a send QUEUE rather than
    /// reach its executor promptly? Locally-hosted chats are never degraded
    /// (a queued command executes on this device even fully offline). Remote
    /// chats degrade when the OS says offline, when the chat's own edge room
    /// is down, or when the host device has gone presence-dark.
    pub fn chat_delivery_degraded(&self, chat_id: &str) -> bool {
        use zeron_proto::ConnectivityState as S;
        if self.connectivity.state == S::Disabled {
            return false;
        }
        let Some(chat) = self.chats.iter().find(|c| c.id == chat_id) else {
            // Unknown chat (a just-minted canvas send): only the global
            // state can speak.
            return self.connectivity.state == S::Offline;
        };
        if Some(chat.device_id.as_str()) == self.local_device_id.as_deref() {
            return false;
        }
        if self.connectivity.state == S::Offline {
            return true;
        }
        let room_down = match self
            .connectivity
            .chats
            .iter()
            .find(|c| c.chat_id == chat_id)
        {
            Some(net) => !net.connected,
            None => self.connectivity.state != S::Connected,
        };
        room_down || !self.device_online(&chat.device_id, Utc::now())
    }

    /// A send is queued: in flight AND its delivery path is degraded — the
    /// honest badge is "Queued", not a Working spinner.
    pub fn send_queued(&self, chat_id: &str, now: DateTime<Utc>) -> bool {
        self.send_pending(chat_id, now) && self.chat_delivery_degraded(chat_id)
    }

    pub fn apply_devices(&mut self, devices: Vec<Device>) -> bool {
        self.apply_devices_at(devices, Utc::now())
    }

    fn apply_devices_at(&mut self, mut devices: Vec<Device>, now: DateTime<Utc>) -> bool {
        // A local-only workspace has no remote device identity to distinguish.
        // Keep the engine's legacy sentinel out of the UI while preserving real
        // hostnames and user-assigned device names.
        if self.workspace_scope == Some(WorkspaceScope::Local)
            && let Some(local_id) = self.local_device_id.as_deref()
            && let Some(device) = devices.iter_mut().find(|device| device.id == local_id)
            && device.name == "unknown-device"
        {
            device.name = "Local".to_string();
        }
        for device in &devices {
            self.change_requests
                .clear_unsupported_on_version_change(&device.id, device.version.as_deref());
        }
        let presentation: Vec<_> = devices
            .iter()
            .map(|device| {
                let mut metadata = device.clone();
                metadata.last_seen_at = None;
                (
                    metadata,
                    crate::settings::devices::device_online(device.last_seen_at, now),
                    crate::settings::devices::format_last_seen(device.last_seen_at, now),
                )
            })
            .collect();
        let session_presence: Vec<_> = self
            .sessions
            .iter()
            .map(|session| effective_indicator(Some(session), now))
            .collect();
        let changed = self.device_presentation.as_ref() != Some(&presentation)
            || self.session_presence_presentation != session_presence;
        self.device_presentation = Some(presentation);
        self.session_presence_presentation = session_presence;
        // Freshness must advance even when the heartbeat does not redraw:
        // delivery gating and later renders still need the newest timestamp.
        self.devices = devices;
        changed
    }

    /// True when `device_id`'s engine (per its registry device row) is at
    /// least `min`. Unknown devices and unstamped versions are conservatively
    /// false — feature gates fall back to the legacy path rather than speak a
    /// protocol the peer may not understand.
    pub fn device_version_at_least(&self, device_id: &str, min: (u64, u64, u64)) -> bool {
        self.devices
            .iter()
            .find(|d| d.id == device_id)
            .and_then(|d| d.version.as_deref())
            .and_then(version_triple)
            .is_some_and(|v| v >= min)
    }

    /// Capability checks use the live EngineInfo for this device (including a
    /// localhost daemon that may not match the UI binary) and synced device
    /// rows for peers. Missing declarations are conservatively unsupported.
    pub fn device_supports(&self, device_id: &str, capability: &str) -> bool {
        if let Some(engine) = self.engine.as_ref()
            && engine.engine_info().device_id == device_id
        {
            return engine.engine_info().supports(capability);
        }
        self.devices
            .iter()
            .find(|device| device.id == device_id)
            .is_some_and(|device| device.supports(capability))
    }

    pub fn chat_host_supports(&self, chat_id: &str, capability: &str) -> bool {
        self.chats
            .iter()
            .find(|chat| chat.id == chat_id)
            .is_some_and(|chat| self.device_supports(&chat.device_id, capability))
    }

    /// First project on the composer's picked device (falling back through
    /// the local device, then any project at all — better a cross-device
    /// project than a surprise project-less canvas). Display order.
    fn first_space_on_picked_device(&self) -> Option<String> {
        let device = self
            .selected_device
            .as_deref()
            .or(self.local_device_id.as_deref());
        let sorted = self.spaces_sorted();
        device
            .and_then(|d| sorted.iter().find(|s| s.device_id == d).copied())
            .or_else(|| sorted.first().copied())
            .map(|s| s.id.clone())
    }

    pub fn apply_update(&mut self, status: zeron_update::UpdateStatus) {
        self.update = Some(status);
    }

    pub fn apply_auth(&mut self, auth: AuthState) {
        self.auth = Some(auth);
    }

    /// Tolerant AuthStatus frame reducer (see [`parse_auth_state`]).
    pub fn apply_auth_value(&mut self, value: serde_json::Value) {
        match parse_auth_state(&value) {
            Some(auth) => self.apply_auth(auth),
            None => tracing::warn!("dropping unrecognized AuthStatus frame"),
        }
    }

    /// The signed-in user, if the engine reports one.
    pub fn auth_user(&self) -> Option<&zeron_proto::UserProfile> {
        match self.auth.as_ref()? {
            AuthState::SignedIn { user, .. } | AuthState::NeedsOrganization { user } => Some(user),
            AuthState::SignedOut => None,
        }
    }
}
