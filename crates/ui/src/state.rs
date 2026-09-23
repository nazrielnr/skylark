//! App state: the engine connection, entity lists, and the selected chat's
//! transcript — one gpui [`Entity`] the whole shell renders from.
//!
//! ## EngineHandle
//! The UI talks the same typed RPC whether the engine is in-process or a separate
//! daemon (ARCHITECTURE §1). [`EngineHandle::bootstrap`] probes the localhost IPC
//! port, mirroring skylark: if an engine is listening it connects over WebSocket
//! ([`RemoteEngine`]); otherwise it embeds one via [`EngineCore::assemble`] and an
//! in-memory RPC transport ([`InProcessEngine`]) — same envelopes, same dispatch.
//!
//! ## Async bridging
//! `bootstrap` runs on tokio via `gpui_tokio::Tokio::spawn`. Once an [`RpcClient`]
//! exists, its `call`/`subscribe` futures are runtime-agnostic (tokio channels),
//! so subscription pumps run on gpui's own executor via `cx.spawn` and fold each
//! frame into the entity with `this.update(...)` + `cx.notify()`.
//!
//! Pure logic (sort order, staleness, gate phase) lives in free functions with
//! unit tests; rendering reads them.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use gpui::{App, Context, Entity, Task};
use gpui_tokio::Tokio;
use serde::de::DeserializeOwned;

use crate::comments::ReviewComment;
use skylark_doc::{SessionMessageEntry, TranscriptDesync, TranscriptFrame};
use skylark_engine::{Engine, EngineConfig, EngineRuntime, InstanceLock, rpc::AuthRpc};
use skylark_proto::{
    AuthState, ChangeRequestSummary, Chat, ChatIndicator, CheckoutChangeRequestStatus, Device,
    EngineInfo, HarnessId, Session, SidebarPreferencesState, Space, WorkspaceScope,
};
use skylark_rpc::{RpcClient, RpcError, RpcReply, RpcService, connect_ws, memory_client, methods};

use crate::change_requests::{
    ChangeRequestClientState, ChangeRequestWatchKey, desired_watch_targets, watch_params,
};

// Recently viewed transcripts stay renderable while a fresh watch opens. Move
// ownership on navigation; never clone whale payloads or retain live watches.
const TRANSCRIPT_CACHE_CAP: usize = 12;
const TRANSCRIPT_CACHE_BYTES: usize = 64 * 1024 * 1024;

struct CachedTranscript {
    prepared: Option<Arc<crate::transcript::PreparedTranscript>>,
    chat_id: String,
    entries: Vec<SessionMessageEntry>,
    context_usage: Option<skylark_proto::ContextUsage>,
    bytes: usize,
}

// A cancelled GPUI watch may own a whale's mirror between updates. Its
// destructor must not free that entire object graph on the UI thread either.
struct WatchPreparation {
    worker: Option<crate::transcript::TranscriptPreparation>,
    executor: gpui::BackgroundExecutor,
}
impl WatchPreparation {
    fn new(executor: gpui::BackgroundExecutor) -> Self {
        Self {
            worker: Some(Default::default()),
            executor,
        }
    }
    fn prepare(
        &mut self,
        update: &skylark_doc::TranscriptUpdate,
    ) -> Result<Arc<crate::transcript::PreparedTranscript>, TranscriptDesync> {
        self.worker.as_mut().unwrap().prepare(update)
    }
}
impl Drop for WatchPreparation {
    fn drop(&mut self) {
        let worker = self.worker.take();
        self.executor
            .spawn(async move {
                drop(worker);
            })
            .detach();
    }
}

// ---------------------------------------------------------------------------
// Engine handle
// ---------------------------------------------------------------------------

#[path = "state/engine.rs"]
mod engine;
pub(crate) use engine::wait_for_deferred_engine;
pub use engine::{EngineBootConfig, EngineHandle, EngineMode};

// ---------------------------------------------------------------------------
// Pure state + reducers
// ---------------------------------------------------------------------------

// The frontend-agnostic derivations (sort orders, staleness gating, sidebar
// grouping, the boot gate, relative times) live in `skylark_proto::view`, pure
// and with their own test suite. Re-exported here because every call site in
// this crate reads them as `state::…`.
pub use skylark_proto::view::{
    ChatGroup, ConnectionStatus, GatePhase, Indicator, SESSION_STALE_MS, attention_rank,
    chat_location, display_status, effective_indicator, format_time_ago, gate_phase, group_chats,
    parse_auth_state, project_label, sort_active, sort_chats, sort_spaces, sort_tabs,
};

// ---------------------------------------------------------------------------
// Org gate (pure)
// ---------------------------------------------------------------------------

/// One org membership row (tolerant local mirror of the engine's ListOrgs
/// reply — `{orgs: [{id, organizationId, name}]}`).
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrgRow {
    pub organization_id: String,
    pub name: String,
}

/// Parse a ListOrgs reply tolerantly (accepts a bare array too).
pub fn parse_orgs(value: &serde_json::Value) -> Vec<OrgRow> {
    let list = value.get("orgs").unwrap_or(value);
    serde_json::from_value(list.clone()).unwrap_or_default()
}

/// Workspace names must be non-empty (trimmed) and reasonably short.
pub fn org_name_valid(name: &str) -> bool {
    let trimmed = name.trim();
    !trimmed.is_empty() && trimmed.chars().count() <= 64
}

/// Memberships sorted by name (case-insensitive), deduped by organization id.
pub fn sort_memberships(mut orgs: Vec<OrgRow>) -> Vec<OrgRow> {
    orgs.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    });
    orgs.dedup_by(|a, b| a.organization_id == b.organization_id);
    orgs
}

// ---------------------------------------------------------------------------
// AppState entity
// ---------------------------------------------------------------------------

/// A composer send whose doc command is queued but not yet executed by the
/// chat's host device — cleared when the host writes the user message back
/// into the transcript (same client-minted id as the [`AppState::echoes`]
/// dedup); past [`UNDELIVERED_GRACE_MS`] it surfaces as the explicit
/// failed/retry state instead.
#[derive(Debug, Clone)]
struct PendingSend {
    message_id: String,
    started: DateTime<Utc>,
}

/// How long an unadopted send reads as Working/Sending (or Queued when the
/// path is degraded) before flipping to the EXPLICIT failed state with a
/// retry affordance. The old 30s overlay silently expired back to Idle with
/// no visible trace of the send at all — the exact hole the 2026-08-19
/// incident fell into.
pub const UNDELIVERED_GRACE_MS: i64 = 120_000;

/// A send's attachment-upload leg in flight. `done` is bumped by the upload
/// task per completed chunk (binary bytes); the working label reads it every
/// paint (the spinner already animates each frame), so no notify plumbing is
/// needed. A slow upload renders as "Uploading… N%" instead of a
/// hang-indistinguishable "Sending…" (2026-08-18 user report).
pub struct UploadProgress {
    done: std::sync::Arc<std::sync::atomic::AtomicU64>,
    total: u64,
}

/// Root application state. Reducer methods (`apply_*`, [`Self::session_for`], …)
/// are plain `&mut self` functions so tests construct the struct directly; gpui
/// glue ([`Self::bootstrap`], [`Self::select_chat`]) layers subscriptions on top.
pub struct AppState {
    pub connection: ConnectionStatus,
    /// Fixed data boundary of the attached engine. Authentication may change
    /// in place, but changing this scope requires assembling a new runtime.
    pub workspace_scope: Option<WorkspaceScope>,
    /// Auth stream value; `None` until the engine reports one (M4).
    pub auth: Option<AuthState>,
    pub devices: Vec<Device>,
    // Last published device presentation. Heartbeats refresh the underlying
    // timestamps without invalidating every view when no displayed value changed.
    device_presentation: Option<Vec<(Device, bool, String)>>,
    // Presence ticks also retire stale remote session indicators. Remember
    // their last published appearance even when the device rows stay online.
    session_presence_presentation: Vec<Indicator>,
    /// Live edge posture (WatchConnectivity): drives the connection pill,
    /// composer honesty ("will queue"), and the Queued send badges.
    pub connectivity: skylark_proto::Connectivity,
    /// Whether this runtime's connectivity watch has delivered its first
    /// frame. The default `Disabled` value is only a placeholder and must not
    /// seed notification decisions before the watch is authoritative.
    pub(crate) connectivity_observed: bool,
    /// Sorted (see [`sort_spaces`]).
    pub spaces: Vec<Space>,
    /// Sorted (see [`sort_chats`]); includes archived rows — views filter.
    pub chats: Vec<Chat>,
    pub sessions: Vec<Session>,
    /// Synced user/org sidebar pin state. Local workspaces deliberately ignore
    /// this and continue reading their device-local settings entry.
    pub sidebar_preferences: SidebarPreferencesState,
    session_presentation: Option<Vec<Session>>,
    /// The project the new-session canvas mints into. Healed by
    /// [`Self::apply_spaces`] when the row vanishes; selecting a chat implies
    /// its project.
    pub selected_space: Option<String>,
    /// Deliberate "Don't work in a project" pick: while set, the canvas mints
    /// project-less sessions (cwd `~` on the picked device) and
    /// [`Self::selected_space_row`] reads as `None` — healing must NOT
    /// re-select a project underneath it.
    pub no_project: bool,
    /// The composer's device pick — where project-less sessions run, and the
    /// device whose projects the project picker lists. `None` falls back to
    /// the local device.
    pub selected_device: Option<String>,
    pub selected_chat: Option<String>,
    /// Boot auto-select happened (or a manual selection superseded it).
    pub auto_selected: bool,
    /// First chats / spaces watch frame has landed — device-local state that
    /// prunes against the doc (open tabs, the sidebar space filter) must not
    /// judge by the empty pre-sync lists.
    pub chats_synced: bool,
    pub spaces_synced: bool,
    pending_deep_link: Option<crate::links::ConversationDeepLink>,
    deep_link_notice: Option<String>,
    /// Joined transcript of the selected chat (continuations folded engine-side).
    pub transcript: Vec<SessionMessageEntry>,
    /// The selected chat's pending-message queue — what was typed while the
    /// agent was busy, in the order it will be sent. Device-agnostic: the
    /// chat's doc holds them (every device sees the same queue).
    pub queue: Vec<skylark_doc::QueuedMessage>,
    pub context_usage: Option<skylark_proto::ContextUsage>,
    /// The selected chat has a transcript from a `WatchDocMessages` reset
    /// (including a retained reset from an earlier visit). An
    /// empty transcript is otherwise indistinguishable from the pre-replay
    /// gap after selection, where optimistic echoes may already be visible.
    pub transcript_replayed: bool,
    transcript_baselines: HashMap<String, Arc<skylark_doc::TranscriptBaseline>>,
    transcript_cache: std::collections::VecDeque<CachedTranscript>,
    pub(crate) prepared_transcripts: HashMap<String, Arc<crate::transcript::PreparedTranscript>>,
    /// Changes only when transcript/optimistic content changes. Presence,
    /// catalogs and other app-state notifications need no row derivation.
    pub(crate) transcript_revision: u64,
    /// Optimistic user echoes per chat id, shown until the doc frame carrying
    /// the same message id arrives (client-minted ids make dedup exact).
    echoes: HashMap<String, Vec<SessionMessageEntry>>,
    /// Send-in-flight overlay per chat id: a queued doc command the host
    /// hasn't executed yet (see [`Self::begin_pending_send`]).
    pending_sends: HashMap<String, PendingSend>,
    /// The in-flight send's attachment upload, when it has one.
    upload_progress: Option<UploadProgress>,
    /// Engine-side queued-attachment transfers by uploadId (`WatchTransfers`
    /// snapshots): real relay-leg progress for the sending thumbnail's
    /// percent ring, present exactly while bytes are moving.
    transfers: HashMap<String, (u64, u64)>,
    /// Written by the changes pane, read by the composer.
    review_comments: HashMap<String, Vec<ReviewComment>>,
    /// File surfaces whose editor-backed comments currently cite a buffer
    /// revision that has not reached disk yet. A chat remains blocked until
    /// every surface waiting on a workspace write has finished or cancelled.
    review_comment_flushes: HashMap<String, HashSet<u64>>,
    /// This engine's device id (best-effort `LocalDevice` probe; `None` until
    /// the engine serves it — views degrade gracefully).
    pub local_device_id: Option<String>,
    /// Latest `UpdateStatus` frame — drives the sidebar update strip.
    pub update: Option<skylark_update::UpdateStatus>,
    /// Data directory (`ui-settings.json`, `composer-defaults.json`); set at
    /// bootstrap so child views can persist small preference files.
    pub data_dir: Option<PathBuf>,
    engine: Option<EngineHandle>,
    watch_tasks: Vec<Task<()>>,
    transcript_task: Option<Task<()>>,
    change_requests: ChangeRequestClientState,
    change_request_tasks: HashMap<ChangeRequestWatchKey, Task<()>>,
    queue_task: Option<Task<()>>,
    change_requests_visible: bool,
    /// SUBAGENT transcripts keyed by subagent doc id (the right pane's
    /// subagent tabs read these). Independent of `selected_chat`: a tab's
    /// feed must survive chat switches — the tab itself is what scopes it.
    sub_transcripts: HashMap<String, Vec<SessionMessageEntry>>,
    /// One watch task per live subagent doc (single-flight per key).
    /// Dropping a task cancels the engine-side watch and unpins the doc from
    /// the engine LRU — closing a tab MUST go through
    /// [`Self::unwatch_subagent_doc`].
    sub_watch_tasks: HashMap<String, Task<()>>,
}

/// Text/reasoning growth changes the transcript without changing session
/// status, tools, navigation, or composer state. Keep those frames local to
/// the affected transcript instead of rebuilding every app-state observer.
pub(crate) struct TranscriptTextChanged {
    pub doc_id: String,
}

impl gpui::EventEmitter<TranscriptTextChanged> for AppState {}

fn is_text_append(frame: &TranscriptFrame) -> bool {
    matches!(frame, TranscriptFrame::Delta { upsert, append, remove, .. }
        if upsert.is_empty() && remove.is_empty() && !append.is_empty())
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    pub fn new() -> Self {
        Self {
            connection: ConnectionStatus::Connecting,
            workspace_scope: None,
            auth: None,
            devices: Vec::new(),
            device_presentation: None,
            session_presence_presentation: Vec::new(),
            connectivity: skylark_proto::Connectivity::default(),
            connectivity_observed: false,
            spaces: Vec::new(),
            chats: Vec::new(),
            sessions: Vec::new(),
            sidebar_preferences: SidebarPreferencesState::default(),
            session_presentation: None,
            selected_space: None,
            no_project: false,
            selected_device: None,
            selected_chat: None,
            transcript: Vec::new(),
            queue: Vec::new(),
            context_usage: None,
            transcript_replayed: false,
            transcript_baselines: HashMap::new(),
            transcript_cache: Default::default(),
            prepared_transcripts: HashMap::new(),
            transcript_revision: 0,
            echoes: HashMap::new(),
            pending_sends: HashMap::new(),
            upload_progress: None,
            transfers: HashMap::new(),
            review_comments: HashMap::new(),
            review_comment_flushes: HashMap::new(),
            local_device_id: None,
            update: None,
            data_dir: None,
            engine: None,
            watch_tasks: Vec::new(),
            transcript_task: None,
            change_requests: ChangeRequestClientState::default(),
            change_request_tasks: HashMap::new(),
            queue_task: None,
            change_requests_visible: true,
            sub_transcripts: HashMap::new(),
            sub_watch_tasks: HashMap::new(),
            auto_selected: false,
            chats_synced: false,
            spaces_synced: false,
            pending_deep_link: None,
            deep_link_notice: None,
        }
    }

    /// The selected chat, or `""` on the new-chat canvas. Identical to the
    /// composer's own attachment/draft key, so a comment written before the
    /// first send survives the chat being minted.
    pub fn composer_key(&self) -> String {
        self.selected_chat.clone().unwrap_or_default()
    }

    pub fn review_comments(&self, key: &str) -> &[ReviewComment] {
        self.review_comments
            .get(key)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    pub fn add_review_comment(&mut self, key: &str, comment: ReviewComment) {
        self.review_comments
            .entry(key.to_string())
            .or_default()
            .push(comment);
    }

    pub fn remove_review_comment(&mut self, key: &str, id: &str) {
        if let Some(list) = self.review_comments.get_mut(key) {
            list.retain(|c| c.id != id);
            if list.is_empty() {
                self.review_comments.remove(key);
                self.review_comment_flushes.remove(key);
            }
        }
    }

    /// Update only a staged comment's body. A stale editor must never recreate
    /// a comment that has already been removed or sent to the agent.
    pub fn update_review_comment_body(&mut self, key: &str, id: &str, body: String) {
        if let Some(comment) = self
            .review_comments
            .get_mut(key)
            .and_then(|comments| comments.iter_mut().find(|comment| comment.id == id))
        {
            comment.body = body;
        }
    }

    pub fn update_review_comment_line(&mut self, key: &str, id: &str, line: u32) {
        if let Some(comment) = self
            .review_comments
            .get_mut(key)
            .and_then(|comments| comments.iter_mut().find(|comment| comment.id == id))
        {
            comment.line = line;
        }
    }

    pub fn rename_review_comment_path(&mut self, key: &str, old_path: &str, new_path: &str) {
        if let Some(comments) = self.review_comments.get_mut(key) {
            for comment in comments
                .iter_mut()
                .filter(|comment| comment.is_file() && comment.path == old_path)
            {
                comment.path = new_path.to_string();
            }
        }
    }

    pub fn take_review_comments(&mut self, key: &str) -> Vec<ReviewComment> {
        self.review_comment_flushes.remove(key);
        self.review_comments.remove(key).unwrap_or_default()
    }

    pub fn purge_review_comments(&mut self, key: &str) {
        self.review_comment_flushes.remove(key);
        self.review_comments.remove(key);
    }

    pub fn begin_review_comment_flush(&mut self, key: &str, source: u64) {
        self.review_comment_flushes
            .entry(key.to_string())
            .or_default()
            .insert(source);
    }

    pub fn finish_review_comment_flush(&mut self, key: &str, source: u64) {
        let remove_key = self
            .review_comment_flushes
            .get_mut(key)
            .is_some_and(|sources| {
                sources.remove(&source);
                sources.is_empty()
            });
        if remove_key {
            self.review_comment_flushes.remove(key);
        }
    }

    pub fn review_comment_flush_pending(&self, key: &str) -> bool {
        self.review_comment_flushes.contains_key(key)
    }

    // ---- reducers (pure) ----
}

#[path = "state/catalog.rs"]
mod catalog;

#[path = "state/transcript.rs"]
mod transcript;

#[path = "state/queries.rs"]
mod queries;

#[path = "state/watchers.rs"]
mod watchers;
use watchers::*;

#[cfg(test)]
mod tests;
#[cfg(feature = "appshots-fixture")]
impl AppState {
    /// Keep fixture documents deterministic while using the real attachment RPC.
    pub fn fixture_attachment_engine(&mut self, engine: EngineHandle) {
        self.engine = Some(engine);
    }
}

#[cfg(feature = "project-palette-fixture")]
impl AppState {
    /// Seed provider metadata for the isolated native sidebar review fixture.
    pub fn fixture_sidebar_change_request(
        &mut self,
        snapshot: skylark_proto::CheckoutChangeRequestStatus,
    ) {
        let key = crate::change_requests::ChangeRequestWatchKey {
            device_id: snapshot.device_id.clone(),
            cwd: snapshot.cwd.clone(),
            branch: snapshot.branch.clone(),
            checkout_id: Some(snapshot.checkout_id.clone()),
        };
        self.change_requests.store(key, snapshot);
    }
}
