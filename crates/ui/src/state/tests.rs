//! App-state unit and integration tests.

use super::*;
use super::*;
use chrono::TimeDelta;
use gpui::AppContext;
use zeron_engine::{EngineCore, default_registry};
// `SessionStatus` is only needed to build the fixtures below — the module
// itself derives everything through `zeron_proto::view`.
use zeron_proto::{SessionStatus, UserProfile};

/// A localhost port that was just free (bind :0, read, drop).
async fn free_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    listener.local_addr().unwrap().port()
}

struct LegacyIdentityRpc;

#[async_trait]
impl RpcService for LegacyIdentityRpc {
    async fn handle(&self, method: &str, _params: serde_json::Value) -> Result<RpcReply, RpcError> {
        match method {
            methods::LOCAL_DEVICE => {
                RpcReply::value(&serde_json::json!({ "deviceId": "legacy-device" }))
            }
            other => Err(RpcError::UnknownMethod(other.into())),
        }
    }
}

struct DeferredIdentityRpc {
    engine_info: EngineInfo,
    state: tokio::sync::watch::Receiver<DeferredEngineState>,
}

#[async_trait]
impl RpcService for DeferredIdentityRpc {
    async fn handle(&self, method: &str, _params: serde_json::Value) -> Result<RpcReply, RpcError> {
        match method {
            methods::ENGINE_INFO => RpcReply::value(&self.engine_info),
            methods::ENGINE_READY => {
                let mut state = self.state.clone();
                wait_for_deferred_engine(&mut state)
                    .await
                    .map_err(RpcError::Failed)?;
                RpcReply::value(&serde_json::json!({ "ready": true }))
            }
            other => Err(RpcError::UnknownMethod(other.into())),
        }
    }
}

#[tokio::test]
async fn legacy_daemon_identity_falls_back_to_synced_scope() {
    let client = memory_client(Arc::new(LegacyIdentityRpc));

    let info = query_engine_info(&client).await.unwrap();

    assert_eq!(info.device_id, "legacy-device");
    assert_eq!(info.workspace_scope, WorkspaceScope::Synced);
    assert_eq!(
        gate_phase(
            &ConnectionStatus::Ready,
            Some(info.workspace_scope),
            Some(&AuthState::SignedOut),
        ),
        GatePhase::SignIn
    );
}

#[tokio::test]
async fn remote_viewport_treats_legacy_daemon_as_ready() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(zeron_rpc::serve_ws_listener(
        listener,
        Arc::new(LegacyIdentityRpc),
    ));
    let dir = tempfile::tempdir().unwrap();
    let handle = EngineHandle::bootstrap(EngineBootConfig {
        data_dir: dir.path().to_path_buf(),
        ipc_port: port,
        edge_url: "http://127.0.0.1:1".into(),
        edge_token: None,
        org_id: None,
        workos_client_id: None,
        default_harness: HarnessId::Mock,
    })
    .await
    .expect("legacy daemon remains attachable");

    let mut deferred = handle
        .deferred_state()
        .expect("remote viewport tracks readiness");
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        wait_for_deferred_engine(&mut deferred),
    )
    .await
    .expect("legacy readiness fallback completes")
    .expect("unknown EngineReady means the old daemon is assembled");

    handle.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn bootstrap_embeds_engine_when_port_is_free() {
    let dir = tempfile::tempdir().unwrap();
    let handle = EngineHandle::bootstrap(EngineBootConfig {
        data_dir: dir.path().to_path_buf(),
        ipc_port: free_port().await,
        edge_url: "http://127.0.0.1:1".into(),
        edge_token: None, // offline
        org_id: None,
        workos_client_id: None,
        default_harness: HarnessId::Mock,
    })
    .await
    .unwrap();
    assert_eq!(handle.mode(), EngineMode::InProcess);
    assert!(matches!(
        handle
            .deferred_state()
            .expect("embedded lifecycle")
            .borrow()
            .clone(),
        DeferredEngineState::Ready
    ));
    // Same protocol over the in-memory transport: a real engine answers.
    let harnesses = handle
        .client()
        .call(methods::LIST_HARNESSES, serde_json::json!({}))
        .await
        .unwrap();
    assert!(harnesses.as_array().is_some_and(|h| !h.is_empty()));
    handle.shutdown().await;
}

#[tokio::test]
async fn bootstrap_reports_local_assembly_failure_before_returning_a_handle() {
    let dir = tempfile::tempdir().unwrap();
    zeron_engine::EngineProfile::local(dir.path()).unwrap();
    std::fs::create_dir(dir.path().join("profiles")).unwrap();
    std::fs::write(dir.path().join("profiles/local"), b"not a directory").unwrap();
    let port = free_port().await;

    let error = match EngineHandle::bootstrap(EngineBootConfig {
        data_dir: dir.path().to_path_buf(),
        ipc_port: port,
        edge_url: "http://127.0.0.1:1".into(),
        edge_token: None,
        org_id: None,
        workos_client_id: Some("client_test".into()),
        default_harness: HarnessId::Mock,
    })
    .await
    {
        Ok(handle) => {
            handle.shutdown().await;
            panic!("a corrupt local store must fail bootstrap")
        }
        Err(error) => error,
    };

    assert!(!format!("{error:#}").is_empty());
    assert!(
        tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_err(),
        "failed bootstrap must release the IPC listener"
    );
}

#[tokio::test]
async fn deferred_engine_failure_remains_observable_after_early_attach() {
    let (state_tx, mut state_rx) = tokio::sync::watch::channel(DeferredEngineState::Waiting);
    state_tx.send_replace(DeferredEngineState::Failed("store failed".into()));

    assert_eq!(
        wait_for_deferred_engine(&mut state_rx).await,
        Err("store failed".into())
    );
}

#[tokio::test]
async fn remote_viewport_observes_deferred_engine_failure() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (state_tx, state_rx) = tokio::sync::watch::channel(DeferredEngineState::Waiting);
    let server = tokio::spawn(zeron_rpc::serve_ws_listener(
        listener,
        Arc::new(DeferredIdentityRpc {
            engine_info: EngineInfo {
                device_id: "owner-device".into(),
                workspace_scope: WorkspaceScope::Local,
                cursor_sdk_version: None,
                capabilities: zeron_proto::capabilities::current(),
            },
            state: state_rx,
        }),
    ));

    let dir = tempfile::tempdir().unwrap();
    let handle = EngineHandle::bootstrap(EngineBootConfig {
        data_dir: dir.path().to_path_buf(),
        ipc_port: port,
        edge_url: "http://127.0.0.1:1".into(),
        edge_token: None,
        org_id: None,
        workos_client_id: None,
        default_harness: HarnessId::Mock,
    })
    .await
    .expect("second viewport attaches over IPC");
    assert!(matches!(handle.mode(), EngineMode::Remote { .. }));

    let mut deferred = handle
        .deferred_state()
        .expect("remote viewport tracks engine readiness");
    state_tx.send_replace(DeferredEngineState::Failed("store failed".into()));
    assert_eq!(
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            wait_for_deferred_engine(&mut deferred),
        )
        .await
        .expect("remote readiness probe completes"),
        Err("store failed".into())
    );

    handle.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn an_embedded_engine_serves_the_ipc_port_for_other_viewports() {
    // The whole point of embedding-and-serving: a second viewport (the
    // terminal app) can attach to this window's engine with no setup, no
    // separate daemon, and no launch ordering.
    let dir = tempfile::tempdir().unwrap();
    let port = free_port().await;
    let handle = EngineHandle::bootstrap(EngineBootConfig {
        data_dir: dir.path().to_path_buf(),
        ipc_port: port,
        edge_url: "http://127.0.0.1:1".into(),
        edge_token: None, // offline
        org_id: None,
        workos_client_id: None,
        default_harness: HarnessId::Mock,
    })
    .await
    .unwrap();
    assert_eq!(handle.mode(), EngineMode::InProcess);

    // Attach the way an external viewport would, and speak the same protocol.
    let attached = connect_ws(&format!("ws://127.0.0.1:{port}"))
        .await
        .expect("a second viewport must be able to attach");
    let harnesses = attached
        .call(methods::LIST_HARNESSES, serde_json::json!({}))
        .await
        .unwrap();
    assert!(harnesses.as_array().is_some_and(|h| !h.is_empty()));

    // Shutting the window down stops accepting, so the next viewport
    // starts its own engine rather than talking to closing stores.
    handle.shutdown().await;
    assert!(
        tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_err(),
        "the port must be released on shutdown"
    );
}

#[tokio::test]
async fn concurrent_bootstraps_elect_one_embedded_engine() {
    // Two viewports of one app booting at once (the Local-switch restart
    // path): both used to probe a closed port, both embedded, and one lost
    // the data-dir lock. The bootstrap gate must elect exactly one owner
    // and turn the other into a plain remote attach.
    let dir = tempfile::tempdir().unwrap();
    let port = free_port().await;
    let config = EngineBootConfig {
        data_dir: dir.path().to_path_buf(),
        ipc_port: port,
        edge_url: "http://127.0.0.1:1".into(),
        edge_token: None, // offline
        org_id: None,
        workos_client_id: None,
        default_harness: HarnessId::Mock,
    };
    let (a, b) = tokio::join!(
        EngineHandle::bootstrap(config.clone()),
        EngineHandle::bootstrap(config.clone()),
    );
    let a = a.expect("first viewport boots");
    let b = b.expect("second viewport boots");

    let modes = [a.mode(), b.mode()];
    assert_eq!(
        modes
            .iter()
            .filter(|mode| **mode == EngineMode::InProcess)
            .count(),
        1,
        "exactly one viewport embeds: {modes:?}"
    );
    assert_eq!(
        modes
            .iter()
            .filter(|mode| matches!(mode, EngineMode::Remote { .. }))
            .count(),
        1,
        "the other attaches over IPC: {modes:?}"
    );

    for handle in [&a, &b] {
        let mut deferred = handle.deferred_state().expect("lifecycle tracked");
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            wait_for_deferred_engine(&mut deferred),
        )
        .await
        .expect("readiness resolves")
        .expect("both viewports reach Ready");
    }

    b.shutdown().await;
    a.shutdown().await;
}

#[tokio::test]
async fn a_stranger_on_the_ipc_port_does_not_wedge_the_window() {
    // The port probe only proves *something* is listening. A process that
    // accepts TCP and never speaks WebSocket used to hang the dial forever;
    // now it times out and we embed instead, losing only the ability to
    // serve other viewports.
    let squatter = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = squatter.local_addr().unwrap().port();
    let dir = tempfile::tempdir().unwrap();
    let handle = EngineHandle::bootstrap(EngineBootConfig {
        data_dir: dir.path().to_path_buf(),
        ipc_port: port,
        edge_url: "http://127.0.0.1:1".into(),
        edge_token: None,
        org_id: None,
        workos_client_id: None,
        default_harness: HarnessId::Mock,
    })
    .await
    .expect("a taken port must not fail the boot");
    assert_eq!(handle.mode(), EngineMode::InProcess);
    assert!(
        handle
            .client()
            .call(methods::LIST_HARNESSES, serde_json::json!({}))
            .await
            .is_ok(),
        "the window still works over its own transport"
    );
    handle.shutdown().await;
    drop(squatter);
}

#[tokio::test]
async fn production_bootstrap_opens_local_data_without_sign_in() {
    let dir = tempfile::tempdir().unwrap();
    let handle = EngineHandle::bootstrap(EngineBootConfig {
        data_dir: dir.path().to_path_buf(),
        ipc_port: free_port().await,
        edge_url: "http://127.0.0.1:1".into(),
        edge_token: None,
        org_id: None,
        workos_client_id: Some("client_test".into()),
        default_harness: HarnessId::Mock,
    })
    .await
    .unwrap();

    assert_eq!(handle.engine_info().workspace_scope, WorkspaceScope::Local);
    let info: EngineInfo = handle
        .client()
        .call_as(methods::ENGINE_INFO, serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(info, *handle.engine_info());

    let mut auth = handle
        .client()
        .subscribe(methods::AUTH_STATUS, serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(
        parse_auth_state(&auth.recv().await.unwrap()),
        Some(AuthState::SignedOut)
    );
    let harnesses = handle
        .client()
        .call(methods::LIST_HARNESSES, serde_json::json!({}))
        .await
        .expect("local data RPC is immediately available");
    assert!(harnesses.as_array().is_some_and(|items| !items.is_empty()));
    assert!(
        !dir.path().join("orgs/dev-org/dev-user").exists(),
        "production boot must not create dev-user data"
    );
    assert!(dir.path().join("profiles/local").is_dir());
    handle.shutdown().await;
}

#[tokio::test]
async fn engine_info_is_available_while_cloud_onboarding_is_deferred() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("session.json"),
        r#"{"refreshToken":"saved","user":{"id":"user_1","email":"u@example.com"}}"#,
    )
    .unwrap();
    let handle = EngineHandle::bootstrap(EngineBootConfig {
        data_dir: dir.path().to_path_buf(),
        ipc_port: free_port().await,
        edge_url: "http://127.0.0.1:1".into(),
        edge_token: None,
        org_id: None,
        workos_client_id: Some("client_test".into()),
        default_harness: HarnessId::Mock,
    })
    .await
    .unwrap();

    assert!(matches!(
        handle
            .deferred_state()
            .expect("embedded lifecycle")
            .borrow()
            .clone(),
        DeferredEngineState::Waiting
    ));

    let info: EngineInfo = handle
        .client()
        .call_as(methods::ENGINE_INFO, serde_json::json!({}))
        .await
        .expect("EngineInfo bypasses deferred cloud stores");
    assert_eq!(info.workspace_scope, WorkspaceScope::Synced);
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            handle
                .client()
                .call(methods::LIST_HARNESSES, serde_json::json!({})),
        )
        .await
        .is_err(),
        "cloud data waits for organization onboarding"
    );
    assert!(!dir.path().join("orgs").exists());
    handle.shutdown().await;
}

#[tokio::test]
async fn bootstrap_connects_when_daemon_is_listening() {
    // Stand in for `zeron headless`: an engine served over the WS IPC port.
    let daemon_dir = tempfile::tempdir().unwrap();
    let core = EngineCore::assemble(
        daemon_dir.path(),
        Arc::new(default_registry()),
        HarnessId::Mock,
        None,
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(zeron_rpc::serve_ws_listener(listener, core.rpc_service()));

    let ui_dir = tempfile::tempdir().unwrap();
    let handle = EngineHandle::bootstrap(EngineBootConfig {
        data_dir: ui_dir.path().to_path_buf(),
        ipc_port: port,
        edge_url: "http://127.0.0.1:1".into(),
        edge_token: None,
        org_id: None,
        workos_client_id: None,
        default_harness: HarnessId::Mock,
    })
    .await
    .unwrap();
    assert_eq!(
        handle.mode(),
        EngineMode::Remote {
            url: format!("ws://127.0.0.1:{port}")
        }
    );
    assert_eq!(
        handle.engine_info().workspace_scope,
        WorkspaceScope::Development
    );
    let harnesses = handle
        .client()
        .call(methods::LIST_HARNESSES, serde_json::json!({}))
        .await
        .unwrap();
    assert!(harnesses.as_array().is_some_and(|h| !h.is_empty()));
    assert!(matches!(
        handle
            .client()
            .call(methods::STOP_ENGINE, serde_json::json!({}))
            .await,
        Err(RpcError::UnknownMethod(method)) if method == methods::STOP_ENGINE
    ));
}

fn chat(id: &str, created_min: i64, last_msg_min: Option<i64>) -> Chat {
    let base = DateTime::parse_from_rfc3339("2026-07-19T12:00:00Z")
        .unwrap()
        .to_utc();
    Chat {
        id: id.into(),
        device_id: "dev".into(),
        title: None,
        archived: false,
        cwd: None,
        branch: None,
        checkout_id: None,
        source_context: None,
        config: None,
        last_message_preview: None,
        last_message_at: last_msg_min.map(|m| base + TimeDelta::minutes(m)),
        created_at: base + TimeDelta::minutes(created_min),
        harness_session_id: None,
        harness_session_cwd: None,
        space_id: None,
        last_seen_at: None,
        room_gen: None,
    }
}

fn space(id: &str, device_id: &str, path: &str, created_min: i64) -> Space {
    let base = DateTime::parse_from_rfc3339("2026-07-19T12:00:00Z")
        .unwrap()
        .to_utc();
    Space {
        id: id.into(),
        device_id: device_id.into(),
        path: path.into(),
        name: None,
        git_detected: false,
        git_checked_at: None,
        checkout_id: None,
        created_at: base + TimeDelta::minutes(created_min),
    }
}

fn session(
    chat_id: &str,
    status: SessionStatus,
    updated_secs_ago: i64,
    now: DateTime<Utc>,
) -> Session {
    Session {
        last_completed_turn: None,
        chat_id: chat_id.into(),
        device_id: "dev".into(),
        status,
        started_at: None,
        updated_at: now - TimeDelta::seconds(updated_secs_ago),
    }
}

fn user_entry(id: &str) -> SessionMessageEntry {
    SessionMessageEntry {
        id: id.into(),
        role: zeron_doc::MessageRole::User,
        parts: Vec::new(),
        created_at: 0,
        device_id: "dev".into(),
        status: None,
        continuation_of: None,
    }
}

#[gpui::test]
fn opening_tail_is_visible_but_never_replaces_or_caches_complete_history(
    cx: &mut gpui::TestAppContext,
) {
    let state = cx.new(|_| AppState::new());
    state.update(cx, |state, cx| {
        let update = |id: &str| zeron_doc::TranscriptUpdate {
            frame: TranscriptFrame::reset(&[user_entry(id)]),
            context_usage: None,
            replay_baseline: None,
        };
        state.select_chat(Some("whale".into()), cx);
        state
            .receive_opening_transcript_update(update("tail"), true, cx)
            .unwrap();
        assert_eq!(state.transcript[0].id, "tail");
        assert!(
            !state.transcript_replayed,
            "scroll-anchor fallback must await full history"
        );
        state.select_chat(None, cx);
        assert!(
            state.transcript_cache.is_empty(),
            "a preview is not a complete cached copy"
        );
        state.select_chat(Some("whale".into()), cx);
        state
            .receive_opening_transcript_update(update("full"), false, cx)
            .unwrap();
        state
            .receive_opening_transcript_update(update("tail"), true, cx)
            .unwrap();
        assert_eq!(
            state.transcript[0].id, "full",
            "reconnect must preserve the full view"
        );
        state.select_chat(None, cx);
        state.select_chat(Some("whale".into()), cx);
        state
            .receive_opening_transcript_update(update("tail"), true, cx)
            .unwrap();
        assert_eq!(
            state.transcript[0].id, "full",
            "revisit must preserve the cache"
        );
    });
}

#[gpui::test]
fn whale_transcript_revisit_is_synchronous_and_fresh_reset_wins(cx: &mut gpui::TestAppContext) {
    let state = cx.new(|_| AppState::new());
    state.update(cx, |state, cx| {
        state.select_chat(Some("whale".into()), cx);
        let entries: Vec<_> = (0..2000)
            .map(|i| {
                let mut entry = user_entry(&format!("message-{i}"));
                entry.parts.push(zeron_doc::MessagePart::Text {
                    id: "text".into(),
                    text: "x".repeat(2048),
                });
                entry
            })
            .collect();
        state.apply_transcript(entries);
        let allocation = state.transcript.as_ptr();
        for _ in 0..10 {
            state.select_chat(Some("other".into()), cx);
            assert!(
                state.transcript.is_empty(),
                "never show another chat's rows"
            );
            state.select_chat(Some("whale".into()), cx);
            assert_eq!(
                state.transcript.len(),
                2000,
                "revisit must render before any RPC frame"
            );
            assert_eq!(
                state.transcript.as_ptr(),
                allocation,
                "move whale payloads, don't copy them"
            );
            assert!(state.transcript_replayed);
            assert!(
                state
                    .transcript_baseline("whale")
                    .unwrap()
                    .covers(&state.transcript[0])
            );
        }
        state
            .apply_transcript_frame(TranscriptFrame::reset(&[user_entry(
                "new-authoritative-row",
            )]))
            .unwrap();
        assert_eq!(state.transcript.len(), 1);
        assert_eq!(state.transcript[0].id, "new-authoritative-row");
        state.select_chat(None, cx);
        state.select_chat(Some("whale".into()), cx);
        assert_eq!(state.transcript[0].id, "new-authoritative-row");
        // An authoritative empty reset must still be honored.
        state
            .apply_transcript_frame(TranscriptFrame::reset(&[]))
            .unwrap();
        assert!(state.transcript.is_empty());
    });
}

#[gpui::test]
fn transcript_revisit_cache_is_bounded_and_account_scoped(cx: &mut gpui::TestAppContext) {
    let state = cx.new(|_| AppState::new());
    state.update(cx, |state, cx| {
        for i in 0..20 {
            state.select_chat(Some(format!("chat-{i}")), cx);
            state.apply_transcript(vec![user_entry("row")]);
        }
        state.select_chat(None, cx);
        assert_eq!(state.transcript_cache.len(), TRANSCRIPT_CACHE_CAP);
        state.select_chat(Some("chat-0".into()), cx);
        assert!(state.transcript.is_empty(), "oldest entry was evicted");
        state.apply_chats(vec![chat("chat-19", 0, None)]);
        assert_eq!(
            state.transcript_cache.len(),
            1,
            "deleted chats leave no cached rows"
        );
        state.prepare_runtime_replacement(cx);
        state.select_chat(Some("chat-19".into()), cx);
        assert!(
            state.transcript.is_empty(),
            "account replacement must clear cached content"
        );
        assert!(state.transcript_cache.is_empty());
    });
}

#[gpui::test]
fn transcript_cache_rejects_oversize_and_unloaded_entries(cx: &mut gpui::TestAppContext) {
    let state = cx.new(|_| AppState::new());
    state.update(cx, |state, cx| {
        state.select_chat(Some("loading".into()), cx);
        state.select_chat(Some("oversize".into()), cx);
        assert!(state.transcript_cache.is_empty());
        let mut entry = user_entry("large");
        entry.parts.push(zeron_doc::MessagePart::Text {
            id: "text".into(),
            text: "x".repeat(TRANSCRIPT_CACHE_BYTES + 1),
        });
        state.apply_transcript(vec![entry]);
        state.select_chat(None, cx);
        assert!(
            state.transcript_cache.is_empty(),
            "byte budget applies even to one whale"
        );
    });
}

#[test]
fn transcript_revision_tracks_replay_echoes_and_subagent_content() {
    let mut state = AppState::new();
    state.selected_chat = Some("c".into());
    let initial = state.transcript_revision;
    state.apply_transfers(Vec::new());
    state.apply_auth(AuthState::SignedOut);
    assert_eq!(
        state.transcript_revision, initial,
        "unrelated notifications are inert"
    );

    state.push_echo("c", user_entry("m1"));
    let echo = state.transcript_revision;
    assert_ne!(echo, initial);
    state.push_echo("c", user_entry("m1"));
    state.remove_echo("c", "absent");
    assert_eq!(
        state.transcript_revision, echo,
        "unchanged echoes do not invalidate"
    );
    state.apply_transcript(vec![user_entry("m1")]);
    assert!(state.pending_echoes().is_empty());
    assert_ne!(
        state.transcript_revision, echo,
        "echo-to-replay handoff invalidates"
    );

    let before_reset = state.transcript_revision;
    state
        .apply_transcript_frame(TranscriptFrame::Reset { reset: Vec::new() })
        .unwrap();
    assert!(state.transcript_replayed);
    assert_ne!(
        state.transcript_revision, before_reset,
        "empty replay is authoritative"
    );

    let before_subagent = state.transcript_revision;
    state.set_subagent_snapshot("sub".into(), vec![user_entry("nested")]);
    assert_ne!(state.transcript_revision, before_subagent);
    let before_close = state.transcript_revision;
    state.unwatch_subagent_doc("sub");
    assert!(state.sub_transcript("sub").is_empty());
    assert_ne!(state.transcript_revision, before_close);
}

fn device(id: &str, name: &str) -> Device {
    Device {
        id: id.into(),
        name: name.into(),
        platform: "macos".into(),
        last_seen_at: None,
        created_at: None,
        version: None,
        cursor_sdk_version: None,
        capabilities: Vec::new(),
    }
}

#[test]
fn session_heartbeats_preserve_freshness_without_redrawing_unchanged_status() {
    let mut state = AppState::new();
    let now = Utc::now();
    let mut row = Session {
        last_completed_turn: None,
        chat_id: "chat".into(),
        device_id: "host".into(),
        status: SessionStatus::Working,
        started_at: Some(now),
        updated_at: now,
    };
    assert!(state.apply_sessions_at(vec![row.clone()], now));
    row.updated_at = now + TimeDelta::seconds(30);
    assert!(!state.apply_sessions_at(vec![row.clone()], now + TimeDelta::seconds(30)));
    assert_eq!(state.sessions[0].updated_at, row.updated_at);
    assert_eq!(
        state.indicator_for("chat", now + TimeDelta::seconds(60)),
        Indicator::Working
    );
    assert!(state.apply_sessions_at(vec![row.clone()], now + TimeDelta::seconds(76)));
    row.updated_at = now + TimeDelta::seconds(80);
    assert!(state.apply_sessions_at(vec![row.clone()], now + TimeDelta::seconds(80)));
    row.status = SessionStatus::AwaitingInput;
    assert!(state.apply_sessions_at(vec![row.clone()], now + TimeDelta::seconds(80)));
    row.status = SessionStatus::Idle;
    row.started_at = None;
    assert!(state.apply_sessions_at(vec![row], now + TimeDelta::seconds(80)));
    assert!(state.apply_sessions_at(vec![], now + TimeDelta::seconds(80)));
}

#[test]
fn unchanged_device_heartbeat_still_retires_stale_session_indicators() {
    let mut state = AppState::new();
    let now = Utc::now();
    state.sessions = vec![Session {
        last_completed_turn: None,
        chat_id: "chat".into(),
        device_id: "host".into(),
        status: SessionStatus::Working,
        started_at: Some(now),
        updated_at: now,
    }];
    let mut row = device("host", "Host");
    row.last_seen_at = Some(now);
    assert!(state.apply_devices_at(vec![row.clone()], now));
    row.last_seen_at = Some(now + TimeDelta::seconds(30));
    assert!(!state.apply_devices_at(vec![row.clone()], now + TimeDelta::seconds(30)));
    row.last_seen_at = Some(now + TimeDelta::seconds(46));
    assert!(state.apply_devices_at(vec![row], now + TimeDelta::seconds(46)));
    assert_eq!(
        state.indicator_for("chat", now + TimeDelta::seconds(46)),
        Indicator::None
    );
    let mut recovered = state.sessions.clone();
    recovered[0].updated_at = now + TimeDelta::seconds(47);
    assert!(
        state.apply_sessions_at(recovered, now + TimeDelta::seconds(47)),
        "a heartbeat must immediately restore an indicator retired by a device tick"
    );
}

#[test]
fn device_heartbeats_keep_freshness_without_repainting_unchanged_presentation() {
    let mut state = AppState::new();
    let now = Utc::now();
    let mut row = device("host", "Host");
    row.last_seen_at = Some(now);
    assert!(state.apply_devices_at(vec![row.clone()], now));
    row.last_seen_at = Some(now + TimeDelta::seconds(15));
    assert!(!state.apply_devices_at(vec![row.clone()], now + TimeDelta::seconds(15)));
    assert_eq!(state.devices[0].last_seen_at, row.last_seen_at);
    // The settings label still advances even before the presence dot expires.
    assert!(state.apply_devices_at(vec![row.clone()], now + TimeDelta::seconds(75)));
    // An identical frame must redraw when presence crosses the expiry boundary.
    assert!(state.apply_devices_at(vec![row.clone()], now + TimeDelta::seconds(86)));
    assert!(!state.device_online("host", now + TimeDelta::seconds(86)));
    row.last_seen_at = Some(now + TimeDelta::seconds(90));
    assert!(state.apply_devices_at(vec![row.clone()], now + TimeDelta::seconds(90)));
    assert!(state.device_online("host", now + TimeDelta::seconds(90)));
    row.name = "Renamed".into();
    assert!(state.apply_devices_at(vec![row.clone()], now + TimeDelta::seconds(90)));
    row.version = Some("9.0.0".into());
    assert!(state.apply_devices_at(vec![row], now + TimeDelta::seconds(90)));
    assert!(state.apply_devices_at(vec![], now + TimeDelta::seconds(90)));
    assert!(!state.apply_devices_at(vec![], now + TimeDelta::seconds(90)));
}

#[test]
fn local_workspace_hides_the_unknown_device_sentinel() {
    let mut state = AppState::new();
    state.workspace_scope = Some(WorkspaceScope::Local);
    state.local_device_id = Some("local".into());

    state.apply_devices(vec![
        device("local", "unknown-device"),
        device("remote", "unknown-device"),
    ]);

    assert_eq!(state.device_name("local"), Some("Local"));
    assert_eq!(state.device_name("remote"), Some("unknown-device"));

    state.apply_devices(vec![device("local", "José's MacBook Pro")]);
    assert_eq!(state.device_name("local"), Some("José's MacBook Pro"));
}

#[test]
fn device_version_change_reenables_change_request_capability() {
    let mut state = AppState::new();
    state
        .change_requests
        .mark_unsupported("remote".into(), Some("0.2.2".into()));
    let mut old = device("remote", "Remote");
    old.version = Some("0.2.2".into());
    state.apply_devices(vec![old]);
    assert!(!state.change_requests.is_supported("remote"));

    let mut upgraded = device("remote", "Remote");
    upgraded.version = Some("0.2.3".into());
    state.apply_devices(vec![upgraded]);
    assert!(state.change_requests.is_supported("remote"));
}

#[test]
fn transfer_percent_tracks_snapshots_by_upload_id() {
    let mut s = AppState::new();
    assert_eq!(s.transfer_percent("u1"), None);

    let frame = |id: &str, done, total| zeron_proto::TransferProgress {
        upload_id: id.into(),
        file_name: "a.png".into(),
        done,
        total,
    };
    s.apply_transfers(vec![frame("u1", 430, 1_000), frame("u2", 0, 400)]);
    assert_eq!(s.transfer_percent("u1"), Some(43));
    assert_eq!(s.transfer_percent("u2"), Some(0));
    assert_eq!(s.transfer_percent("other"), None);

    // The last point belongs to the commit — never a stuck 100%; b64
    // padding overshoot stays clamped too.
    s.apply_transfers(vec![frame("u1", 1_000, 1_000), frame("u3", 12, 0)]);
    assert_eq!(s.transfer_percent("u1"), Some(99));
    // Zero-total renders indeterminate, not a division blowup.
    assert_eq!(s.transfer_percent("u3"), None);
    // Snapshots REPLACE: u2's transfer retired with its entry.
    assert_eq!(s.transfer_percent("u2"), None);

    s.apply_transfers(Vec::new());
    assert_eq!(s.transfer_percent("u1"), None);
}

#[test]
fn upload_progress_percent_clamps_and_clears() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    let mut s = AppState::new();
    assert_eq!(s.upload_progress_percent(), None);

    let done = Arc::new(AtomicU64::new(0));
    s.begin_upload_progress(1_000, done.clone());
    assert_eq!(s.upload_progress_percent(), Some(0));
    done.store(430, Ordering::Relaxed);
    assert_eq!(s.upload_progress_percent(), Some(43));
    // The last point belongs to commit+queue — never show a stuck 100%.
    done.store(1_000, Ordering::Relaxed);
    assert_eq!(s.upload_progress_percent(), Some(99));
    // Overshoot (b64 padding rounding) stays clamped.
    done.store(1_002, Ordering::Relaxed);
    assert_eq!(s.upload_progress_percent(), Some(99));

    s.end_upload_progress();
    assert_eq!(s.upload_progress_percent(), None);

    // A zero-byte total renders as plain "Sending…", not a percent.
    s.begin_upload_progress(0, Arc::new(AtomicU64::new(0)));
    assert_eq!(s.upload_progress_percent(), None);
}

#[test]
fn send_pending_overlays_working_until_the_grace_window() {
    let now = Utc::now();
    let s_chat = chat("c", 0, Some(10)); // unseen, no session row
    let mut s = AppState::new();
    assert_eq!(s.display_status_for(&s_chat, now), ChatIndicator::Completed);
    assert_eq!(s.indicator_for("c", now), Indicator::None);
    s.begin_pending_send("c", "m1", now);
    assert_eq!(s.display_status_for(&s_chat, now), ChatIndicator::Working);
    assert_eq!(s.indicator_for("c", now), Indicator::Working);
    // Time-bounded: an offline host must not leave an eternal spinner —
    // past the grace the overlay yields (and `send_undelivered` takes
    // over with the explicit failed state).
    let later = now + TimeDelta::milliseconds(UNDELIVERED_GRACE_MS + 1);
    assert_eq!(
        s.display_status_for(&s_chat, later),
        ChatIndicator::Completed
    );
    assert_eq!(s.indicator_for("c", later), Indicator::None);
    assert!(s.send_undelivered("c", later));
}

#[test]
fn send_pending_acked_when_the_host_writes_the_message_back() {
    let now = Utc::now();
    let mut s = AppState::new();
    s.selected_chat = Some("c".into());
    s.begin_pending_send("c", "m1", now);
    // A frame without the message keeps the overlay.
    s.apply_transcript(vec![user_entry("other")]);
    assert!(s.send_pending("c", now));
    // The host executed the command: our id comes back in the doc.
    s.apply_transcript(vec![user_entry("other"), user_entry("m1")]);
    assert!(!s.send_pending("c", now));
}

#[test]
fn send_failure_cleanup_only_ends_its_own_overlay() {
    let now = Utc::now();
    let mut s = AppState::new();
    s.begin_pending_send("c", "m1", now);
    s.begin_pending_send("c", "m2", now); // quick resend superseded m1
    s.end_pending_send("c", "m1"); // m1's failure cleanup arrives late
    assert!(s.send_pending("c", now), "m2's overlay must survive");
    s.end_pending_send("c", "m2");
    assert!(!s.send_pending("c", now));
}

#[test]
fn chats_sort_by_last_message_desc_with_created_fallback() {
    let mut chats = vec![
        chat("a", 0, Some(10)),
        chat("b", 5, None), // no messages → keys on created_at (+5min)
        chat("c", 1, Some(30)),
        chat("d", 40, None), // created after every message
    ];
    sort_chats(&mut chats);
    let order: Vec<&str> = chats.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(order, ["d", "c", "a", "b"]);
}

#[test]
fn chat_sort_ties_are_deterministic() {
    let mut chats = vec![chat("z", 0, Some(10)), chat("a", 0, Some(10))];
    sort_chats(&mut chats);
    assert_eq!(chats[0].id, "a");
}

#[test]
fn working_indicator_staleness() {
    let now = Utc::now();
    // Fresh working session shows.
    let fresh = session("c", SessionStatus::Working, 10, now);
    assert_eq!(effective_indicator(Some(&fresh), now), Indicator::Working);
    // Stale working session is suppressed — crashed backend, not eternal spinner.
    let stale = session("c", SessionStatus::Working, 46, now);
    assert_eq!(effective_indicator(Some(&stale), now), Indicator::None);
    // Exactly at the boundary still shows (strictly-older-than semantics).
    let edge = session("c", SessionStatus::Working, 45, now);
    assert_eq!(effective_indicator(Some(&edge), now), Indicator::Working);
    // Future timestamps (clock skew) count as fresh.
    let skewed = session("c", SessionStatus::Working, -30, now);
    assert_eq!(effective_indicator(Some(&skewed), now), Indicator::Working);
}

#[test]
fn indicator_kinds() {
    let now = Utc::now();
    assert_eq!(effective_indicator(None, now), Indicator::None);
    let idle = session("c", SessionStatus::Idle, 0, now);
    assert_eq!(effective_indicator(Some(&idle), now), Indicator::None);
    // Errored is not staleness-gated: the error stays visible.
    let errored = session("c", SessionStatus::Errored, 600, now);
    assert_eq!(effective_indicator(Some(&errored), now), Indicator::Errored);
    let awaiting = session("c", SessionStatus::AwaitingInput, 5, now);
    assert_eq!(
        effective_indicator(Some(&awaiting), now),
        Indicator::AwaitingInput
    );
    let awaiting_stale = session("c", SessionStatus::AwaitingInput, 300, now);
    assert_eq!(
        effective_indicator(Some(&awaiting_stale), now),
        Indicator::None
    );
}

#[test]
fn display_status_derivation() {
    let now = Utc::now();
    let mut c = chat("c", 0, Some(10));
    // Live states win regardless of seen.
    let working = session("c", SessionStatus::Working, 5, now);
    assert_eq!(
        display_status(&c, Some(&working), now),
        ChatIndicator::Working
    );
    let awaiting = session("c", SessionStatus::AwaitingInput, 5, now);
    assert_eq!(
        display_status(&c, Some(&awaiting), now),
        ChatIndicator::AwaitingInput
    );
    // Finished + unseen = Completed (no session row at all).
    assert_eq!(display_status(&c, None, now), ChatIndicator::Completed);
    // Idle session + unseen = Completed.
    let idle = session("c", SessionStatus::Idle, 5, now);
    assert_eq!(
        display_status(&c, Some(&idle), now),
        ChatIndicator::Completed
    );
    // Stale working session falls back to the seen check.
    let stale = session("c", SessionStatus::Working, 300, now);
    assert_eq!(
        display_status(&c, Some(&stale), now),
        ChatIndicator::Completed
    );
    // Seen after the last message = Idle.
    c.last_seen_at = c.last_message_at.map(|t| t + TimeDelta::minutes(1));
    assert_eq!(display_status(&c, Some(&idle), now), ChatIndicator::Idle);
    // Errored + unseen = Errored; seen clears it to Idle.
    let errored = session("c", SessionStatus::Errored, 600, now);
    assert_eq!(display_status(&c, Some(&errored), now), ChatIndicator::Idle);
    c.last_seen_at = None;
    assert_eq!(
        display_status(&c, Some(&errored), now),
        ChatIndicator::Errored
    );
    // No messages at all: nothing to see — Idle.
    let fresh = chat("f", 0, None);
    assert_eq!(display_status(&fresh, None, now), ChatIndicator::Idle);
}

#[test]
fn active_list_sorts_by_recency_only_status_never_moves_rows() {
    let a = chat("a", 0, Some(10)); // Completed (older)
    let b = chat("b", 0, Some(20)); // Completed (newer)
    let c = chat("c", 0, Some(5)); // AwaitingInput
    let d = chat("d", 0, Some(1)); // Working
    let mut rows = vec![
        (ChatIndicator::Completed, &a),
        (ChatIndicator::Completed, &b),
        (ChatIndicator::AwaitingInput, &c),
        (ChatIndicator::Working, &d),
    ];
    sort_active(&mut rows);
    let order: Vec<&str> = rows.iter().map(|(_, c)| c.id.as_str()).collect();
    assert_eq!(order, ["b", "a", "c", "d"], "recency desc, status ignored");

    // Opening a completed session (completed → seen → idle) must NOT
    // change its position (user report: rows jumped under the pointer).
    let mut seen = vec![
        (ChatIndicator::Idle, &a),
        (ChatIndicator::Completed, &b),
        (ChatIndicator::AwaitingInput, &c),
        (ChatIndicator::Working, &d),
    ];
    sort_active(&mut seen);
    let order_after: Vec<&str> = seen.iter().map(|(_, c)| c.id.as_str()).collect();
    assert_eq!(order, order_after);
}

#[test]
fn tabs_order_by_creation_not_activity() {
    let a = chat("a", 5, Some(100)); // created later, very active
    let b = chat("b", 1, Some(2));
    let mut tabs = vec![&a, &b];
    sort_tabs(&mut tabs);
    let order: Vec<&str> = tabs.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(order, ["b", "a"]);
}

#[test]
fn apply_spaces_sorts_and_heals_selection() {
    let mut state = AppState::new();
    state.apply_spaces(vec![
        space("s2", "dev", "/b", 2),
        space("s1", "dev", "/a", 1),
    ]);
    let ids: Vec<&str> = state.spaces.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["s1", "s2"]);
    // First frame auto-selects the first space.
    assert_eq!(state.selected_space.as_deref(), Some("s1"));
    state.selected_space = Some("s2".into());
    // Vanished selection heals to the first space.
    state.apply_spaces(vec![space("s1", "dev", "/a", 1)]);
    assert_eq!(state.selected_space.as_deref(), Some("s1"));
    // No spaces at all: selection clears.
    state.apply_spaces(vec![]);
    assert_eq!(state.selected_space, None);
}

#[test]
fn projectless_preference_survives_restart_and_space_refreshes() {
    let dir = tempfile::tempdir().unwrap();
    let defaults = crate::settings::composer::ComposerDefaults {
        device: Some("remote".into()),
        // Older versions kept a stale project alongside the opt-out.
        project: Some("old-project".into()),
        no_project: true,
        ..Default::default()
    };
    defaults.save(dir.path()).unwrap();
    let mut state = AppState::new();
    state.restore_composer_target(&crate::settings::composer::ComposerDefaults::load(
        dir.path(),
    ));
    for spaces in [
        vec![],
        vec![space("s1", "remote", "/a", 1)],
        vec![],
        vec![space("s2", "other", "/b", 2)],
    ] {
        state.apply_spaces(spaces);
        assert!(state.no_project);
        assert!(state.selected_space.is_none());
        assert!(state.selected_space_row().is_none());
        assert_eq!(state.effective_device_id().as_deref(), Some("remote"));
    }
}

#[gpui::test]
fn projectless_selection_clears_project_and_survives_device_switch(cx: &mut gpui::TestAppContext) {
    let state = cx.new(|_| AppState::new());
    state.update(cx, |state, cx| {
        state.apply_spaces(vec![
            space("s1", "dev", "/a", 1),
            space("s2", "remote", "/b", 2),
        ]);
        state.select_space(Some("s1".into()), cx);
        state.select_space(None, cx);
        assert!(state.selected_space.is_none());
        state.select_device("remote".into(), cx);
        assert!(state.no_project);
        assert!(state.selected_space.is_none());
        assert_eq!(state.effective_device_id().as_deref(), Some("remote"));
        state.select_space(Some("s2".into()), cx);
        assert!(!state.no_project);
        assert_eq!(state.selected_space_row().unwrap().id, "s2");
        state.selected_device = Some("dev".into());
        state.select_space(None, cx);
        assert_eq!(state.effective_device_id().as_deref(), Some("remote"));
        state.select_space(Some("s2".into()), cx);
        state.select_device("empty-device".into(), cx);
        assert!(state.selected_space.is_none());
        assert_eq!(state.effective_device_id().as_deref(), Some("empty-device"));
    });
}

#[test]
fn chats_in_space_filters_and_orders() {
    let mut state = AppState::new();
    state.apply_spaces(vec![space("s1", "dev", "/a", 1)]);
    let mut in_space_new = chat("new", 5, None);
    in_space_new.space_id = Some("s1".into());
    let mut in_space_old = chat("old", 1, Some(50)); // active but created first
    in_space_old.space_id = Some("s1".into());
    let mut other = chat("other", 2, None);
    other.space_id = Some("s2".into());
    let mut archived = chat("gone", 0, None);
    archived.space_id = Some("s1".into());
    archived.archived = true;
    let dangling = chat("dangling", 3, None); // no space id
    state.apply_chats(vec![in_space_new, in_space_old, other, archived, dangling]);
    let ids: Vec<&str> = state
        .chats_in_space("s1")
        .iter()
        .map(|c| c.id.as_str())
        .collect();
    assert_eq!(ids, ["old", "new"]);
    // The overview shows every live-space chat (idle included) PLUS
    // project-less chats (first-class since the project selectors);
    // chats of unknown spaces stay hidden. Completed ("old") outranks
    // idle ("new"/"dangling").
    let now = Utc::now();
    let overview: Vec<&str> = state
        .overview_chats(now)
        .iter()
        .map(|(_, c)| c.id.as_str())
        .collect();
    assert_eq!(overview, ["old", "new", "dangling"]);
}

#[test]
fn apply_chats_drops_vanished_selection() {
    let mut state = AppState::new();
    state.apply_chats(vec![chat("a", 0, None), chat("b", 1, None)]);
    state.selected_chat = Some("a".into());
    state.transcript = vec![];
    state.apply_chats(vec![chat("b", 1, None)]);
    assert_eq!(state.selected_chat, None);
    // Still-present selection survives.
    state.selected_chat = Some("b".into());
    state.apply_chats(vec![chat("b", 1, None), chat("c", 2, None)]);
    assert_eq!(state.selected_chat.as_deref(), Some("b"));
}

#[test]
fn apply_chat_config_stamps_the_row() {
    let mut state = AppState::new();
    state.apply_chats(vec![chat("a", 0, None), chat("b", 1, None)]);
    let config = zeron_proto::ChatConfig {
        harness: HarnessId::ClaudeCode,
        model: Some("claude-fable-5".into()),
        reasoning: Some(zeron_proto::ReasoningLevel::XHigh),
        model_options: serde_json::Map::new(),
        sandbox: zeron_proto::SandboxLevel::WorkspaceWrite,
    };
    state.apply_chat_config("a", config.clone());
    assert_eq!(
        state.chats.iter().find(|c| c.id == "a").unwrap().config,
        Some(config)
    );
    assert!(
        state
            .chats
            .iter()
            .find(|c| c.id == "b")
            .unwrap()
            .config
            .is_none()
    );
    // Unknown chat: no-op, no panic.
    state.apply_chat_config(
        "missing",
        zeron_proto::ChatConfig {
            harness: HarnessId::ClaudeCode,
            model: None,
            reasoning: None,
            model_options: serde_json::Map::new(),
            sandbox: zeron_proto::SandboxLevel::WorkspaceWrite,
        },
    );
}

#[test]
fn visible_chats_filters_archived() {
    let mut state = AppState::new();
    let mut archived = chat("a", 0, Some(99));
    archived.archived = true;
    state.apply_chats(vec![archived, chat("b", 1, None)]);
    let visible: Vec<&str> = state.visible_chats().map(|c| c.id.as_str()).collect();
    assert_eq!(visible, ["b"]);
}

#[test]
fn jump_slots_count_the_rows_the_sidebar_draws() {
    let now = Utc::now();
    let mut state = AppState::new();
    let mut in_space = chat("a", 0, Some(3));
    in_space.space_id = Some("s1".into());
    let mut other_space = chat("b", 1, Some(2));
    other_space.space_id = Some("s2".into());
    let mut archived = chat("gone", 2, Some(1));
    archived.space_id = Some("s1".into());
    archived.archived = true;
    state.apply_spaces(vec![
        space("s1", "d1", "/tmp/s1", 0),
        space("s2", "d1", "/tmp/s2", 1),
    ]);
    state.apply_chats(vec![in_space, other_space, archived]);

    // The archived row is not in the active list, so no slot reaches it.
    let order: Vec<&str> = state
        .sidebar_chats(now, None)
        .iter()
        .map(|(_, c)| c.id.as_str())
        .collect();
    assert_eq!(order.len(), 2);
    assert!(!order.contains(&"gone"));

    // A project filter renumbers: the visible rows only.
    let filtered: Vec<&str> = state
        .sidebar_chats(now, Some("s2"))
        .iter()
        .map(|(_, c)| c.id.as_str())
        .collect();
    assert_eq!(filtered, ["b"]);
}

#[test]
fn archive_shortcut_only_targets_an_open_active_chat() {
    let mut state = AppState::new();
    let mut archived = chat("a", 0, None);
    archived.archived = true;
    state.apply_chats(vec![archived, chat("b", 1, None)]);
    // No chat open: nothing to archive.
    assert_eq!(state.archivable_selected_chat(), None);
    // The open active chat is the target.
    state.selected_chat = Some("b".into());
    assert_eq!(state.archivable_selected_chat(), Some("b"));
    // An already archived chat stays put — the shortcut never unarchives.
    state.selected_chat = Some("a".into());
    assert_eq!(state.archivable_selected_chat(), None);
}

#[test]
fn echoes_show_until_doc_frame_confirms() {
    let mut state = AppState::new();
    state.selected_chat = Some("c1".into());
    let echo = SessionMessageEntry {
        id: "m1".into(),
        role: zeron_doc::MessageRole::User,
        parts: vec![],
        created_at: 0,
        device_id: "local".into(),
        status: None,
        continuation_of: None,
    };
    state.push_echo("c1", echo.clone());
    // Duplicate pushes dedupe.
    state.push_echo("c1", echo.clone());
    assert_eq!(state.pending_echoes().len(), 1);
    // Frames without the id keep the echo.
    state.apply_transcript(vec![]);
    assert_eq!(state.pending_echoes().len(), 1);
    // The confirming frame prunes it.
    state.apply_transcript(vec![SessionMessageEntry {
        id: "m1".into(),
        ..echo.clone()
    }]);
    assert!(state.pending_echoes().is_empty());
    // Failure path: explicit removal.
    state.push_echo(
        "c1",
        SessionMessageEntry {
            id: "m2".into(),
            ..echo.clone()
        },
    );
    state.remove_echo("c1", "m2");
    assert!(state.pending_echoes().is_empty());
    // Echoes are per chat.
    state.push_echo(
        "other",
        SessionMessageEntry {
            id: "m3".into(),
            ..echo
        },
    );
    assert!(state.pending_echoes().is_empty());
}

#[test]
fn transcript_replay_barrier_requires_the_opening_reset() {
    let mut state = AppState::new();
    assert!(!state.transcript_replayed);

    state
        .apply_transcript_frame(TranscriptFrame::Delta {
            upsert: Vec::new(),
            append: Vec::new(),
            remove: Vec::new(),
            count: 0,
        })
        .expect("empty delta");
    assert!(!state.transcript_replayed);

    state
        .apply_transcript_frame(TranscriptFrame::Reset { reset: Vec::new() })
        .expect("empty reset");
    assert!(state.transcript_replayed);
}

#[test]
fn gate_phases() {
    let user = UserProfile {
        id: "u".into(),
        email: "w@example.com".into(),
        name: None,
    };
    assert_eq!(
        gate_phase(&ConnectionStatus::Connecting, None, None),
        GatePhase::Loading
    );
    assert_eq!(
        gate_phase(&ConnectionStatus::Failed("boom".into()), None, None),
        GatePhase::Failed("boom".into())
    );
    assert_eq!(
        gate_phase(
            &ConnectionStatus::Ready,
            Some(WorkspaceScope::Local),
            Some(&AuthState::SignedOut),
        ),
        GatePhase::Ready
    );
    assert_eq!(
        gate_phase(
            &ConnectionStatus::Ready,
            Some(WorkspaceScope::Synced),
            Some(&AuthState::SignedOut),
        ),
        GatePhase::SignIn
    );
    assert_eq!(
        gate_phase(
            &ConnectionStatus::Ready,
            Some(WorkspaceScope::Synced),
            Some(&AuthState::SignedIn {
                user: user.clone(),
                org_id: None
            })
        ),
        GatePhase::Ready
    );
    // No org yet → org gate.
    assert_eq!(
        gate_phase(
            &ConnectionStatus::Ready,
            Some(WorkspaceScope::Synced),
            Some(&AuthState::NeedsOrganization { user })
        ),
        GatePhase::OrgGate
    );
}

#[test]
fn auth_changes_do_not_change_a_local_runtime_scope_or_watches() {
    let mut state = AppState::new();
    state.workspace_scope = Some(WorkspaceScope::Local);
    state.watch_tasks.push(Task::ready(()));

    state.apply_auth(AuthState::NeedsOrganization {
        user: UserProfile {
            id: "u".into(),
            email: "w@example.com".into(),
            name: None,
        },
    });
    assert_eq!(state.workspace_scope, Some(WorkspaceScope::Local));
    assert_eq!(state.watch_tasks.len(), 1);

    state.apply_auth(AuthState::SignedIn {
        user: UserProfile {
            id: "u".into(),
            email: "w@example.com".into(),
            name: None,
        },
        org_id: Some("org-1".into()),
    });
    assert_eq!(state.workspace_scope, Some(WorkspaceScope::Local));
    assert_eq!(state.watch_tasks.len(), 1);
}

#[test]
fn auth_frames_parse_both_wire_shapes() {
    // Proto shape.
    let proto = serde_json::json!({ "state": "signedOut" });
    assert_eq!(parse_auth_state(&proto), Some(AuthState::SignedOut));
    // Engine shape (`_tag`, PascalCase, orgId).
    let engine = serde_json::json!({
        "_tag": "SignedIn",
        "user": { "id": "u1", "email": "w@example.com" },
        "orgId": "org-1",
    });
    let Some(AuthState::SignedIn { user, org_id }) = parse_auth_state(&engine) else {
        panic!("expected SignedIn");
    };
    assert_eq!(user.email, "w@example.com");
    assert_eq!(org_id.as_deref(), Some("org-1"));
    let needs = serde_json::json!({
        "_tag": "NeedsOrganization",
        "user": { "id": "u1", "email": "w@example.com", "name": "W" },
    });
    assert!(matches!(
        parse_auth_state(&needs),
        Some(AuthState::NeedsOrganization { .. })
    ));
    // Garbage → None (frame dropped, not a crash).
    assert_eq!(
        parse_auth_state(&serde_json::json!({ "_tag": "Wat" })),
        None
    );
    assert_eq!(parse_auth_state(&serde_json::json!(42)), None);
}

fn chat_with_cwd(id: &str, created_min: i64, cwd: Option<&str>) -> Chat {
    let mut c = chat(id, created_min, None);
    c.cwd = cwd.map(str::to_string);
    c
}

#[test]
fn project_labels_from_cwd() {
    assert_eq!(project_label(Some("/home/w/dev/zeron")), "zeron");
    assert_eq!(project_label(Some("/home/w/dev/zeron/")), "zeron");
    assert_eq!(project_label(None), "No project");
    assert_eq!(project_label(Some("~")), "No project");
    assert_eq!(project_label(Some("~/")), "No project");
    assert_eq!(project_label(Some("   ")), "No project");
    assert_eq!(project_label(Some("/")), "/");
}

#[test]
fn grouped_sidebar_preserves_recency_order() {
    // Input is sidebar-sorted (most recent first).
    let chats = [
        chat_with_cwd("a", 9, Some("/dev/zeron")),
        chat_with_cwd("b", 8, Some("/dev/zed")),
        chat_with_cwd("c", 7, Some("/dev/zeron")),
        chat_with_cwd("d", 6, None),
    ];
    let groups = group_chats(chats.iter());
    let labels: Vec<&str> = groups.iter().map(|g| g.label.as_str()).collect();
    // Groups ordered by their most recent chat; rows keep order.
    assert_eq!(labels, ["zeron", "zed", "No project"]);
    let zeron_ids: Vec<&str> = groups[0].chats.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(zeron_ids, ["a", "c"]);
    assert!(group_chats(std::iter::empty()).is_empty());
}

#[test]
fn relative_times_match_zeron_format() {
    let now = Utc::now();
    let ago = |secs: i64| now - chrono::Duration::seconds(secs);
    assert_eq!(format_time_ago(ago(0), now), "now");
    assert_eq!(format_time_ago(ago(59), now), "now");
    assert_eq!(format_time_ago(ago(60), now), "1m");
    assert_eq!(format_time_ago(ago(59 * 60), now), "59m");
    assert_eq!(format_time_ago(ago(60 * 60), now), "1h");
    assert_eq!(format_time_ago(ago(23 * 3600 + 3599), now), "23h");
    assert_eq!(format_time_ago(ago(24 * 3600), now), "1d");
    assert_eq!(format_time_ago(ago(6 * 86400), now), "6d");
    assert_eq!(format_time_ago(ago(7 * 86400), now), "1w");
    assert_eq!(format_time_ago(ago(30 * 86400), now), "4w");
    assert_eq!(format_time_ago(ago(35 * 86400), now), "1mo");
    assert_eq!(format_time_ago(ago(400 * 86400), now), "1y");
    // Clock skew (future timestamps) clamps to "now".
    assert_eq!(
        format_time_ago(now + chrono::Duration::hours(2), now),
        "now"
    );
}

#[test]
fn chat_location_joins_project_and_branch() {
    let mut c = chat_with_cwd("x", 1, Some("/home/w/dev/soccertcg"));
    c.branch = Some("zeron/rebalance".into());
    assert_eq!(
        chat_location(&c).as_deref(),
        Some("soccertcg · zeron/rebalance")
    );
    c.branch = None;
    assert_eq!(chat_location(&c).as_deref(), Some("soccertcg"));
    c.cwd = None;
    c.branch = Some("main".into());
    assert_eq!(chat_location(&c).as_deref(), Some("main"));
    c.branch = Some("   ".into());
    assert_eq!(chat_location(&c), None);
    c.branch = None;
    assert_eq!(chat_location(&c), None);
}

#[test]
fn org_gate_reducers() {
    assert!(org_name_valid("Acme"));
    assert!(org_name_valid("  padded  "));
    assert!(!org_name_valid(""));
    assert!(!org_name_valid("   "));
    assert!(!org_name_valid(&"x".repeat(65)));

    let rows = parse_orgs(&serde_json::json!({ "orgs": [
        { "id": "m2", "organizationId": "o2", "name": "beta" },
        { "id": "m1", "organizationId": "o1", "name": "Alpha" },
        { "id": "m3", "organizationId": "o1", "name": "Alpha" },
    ]}));
    assert_eq!(rows.len(), 3);
    let sorted = sort_memberships(rows);
    let names: Vec<&str> = sorted.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(
        names,
        ["Alpha", "beta"],
        "case-insensitive sort + dedupe by org id"
    );
    // Bare-array replies parse too; garbage yields empty.
    assert_eq!(
        parse_orgs(&serde_json::json!([{ "id": "m", "organizationId": "o", "name": "n" }])).len(),
        1
    );
    assert!(parse_orgs(&serde_json::json!("nope")).is_empty());
}

#[test]
fn review_comment_body_updates_are_scoped_and_keep_current_metadata() {
    let mut state = AppState::new();
    let original = ReviewComment::file("a.rs", 2, "Original");
    state.add_review_comment("chat-1", original.clone());
    state.add_review_comment("chat-2", original.clone());
    state.begin_review_comment_flush("chat-1", 1);
    state.update_review_comment_line("chat-1", &original.id, 9);
    state.rename_review_comment_path("chat-1", "a.rs", "renamed.rs");
    state.update_review_comment_body("chat-1", &original.id, "Revised".into());
    let mut expected = original.clone();
    expected.path = "renamed.rs".into();
    expected.line = 9;
    expected.body = "Revised".into();
    assert_eq!(state.review_comments("chat-1"), &[expected]);
    assert_eq!(state.review_comments("chat-2"), &[original.clone()]);
    assert!(state.review_comment_flush_pending("chat-1"));
    state.remove_review_comment("chat-1", &original.id);
    state.update_review_comment_body("chat-1", &original.id, "Stale".into());
    state.update_review_comment_body("missing-chat", &original.id, "Stale".into());
    assert!(state.review_comments("chat-1").is_empty());
    assert!(state.review_comments("missing-chat").is_empty());
}

#[test]
fn review_comment_flush_waits_for_every_file_surface() {
    let mut state = AppState::new();

    state.begin_review_comment_flush("chat-1", 1);
    state.begin_review_comment_flush("chat-1", 2);
    assert!(state.review_comment_flush_pending("chat-1"));

    state.finish_review_comment_flush("chat-1", 1);
    assert!(state.review_comment_flush_pending("chat-1"));

    state.finish_review_comment_flush("chat-1", 2);
    assert!(!state.review_comment_flush_pending("chat-1"));
}

#[test]
fn version_triple_parses_and_gates_device_features() {
    assert_eq!(version_triple("0.2.12"), Some((0, 2, 12)));
    assert_eq!(version_triple("0.2.12-beta.1"), Some((0, 2, 12)));
    assert_eq!(version_triple("1.0.0+build7"), Some((1, 0, 0)));
    assert_eq!(version_triple("0.2"), None);
    assert_eq!(version_triple("garbage"), None);

    let mut s = AppState::default();
    assert!(
        !s.device_version_at_least("d1", (0, 2, 12)),
        "unknown device conservatively fails the gate"
    );
    s.devices = vec![Device {
        id: "d1".into(),
        name: "laptop".into(),
        platform: "macos".into(),
        last_seen_at: None,
        created_at: None,
        version: Some("0.2.12".into()),
        cursor_sdk_version: None,
        capabilities: Vec::new(),
    }];
    assert!(s.device_version_at_least("d1", (0, 2, 12)));
    assert!(!s.device_version_at_least("d1", (0, 2, 13)));
    s.devices[0].version = None;
    assert!(
        !s.device_version_at_least("d1", (0, 2, 12)),
        "unstamped version conservatively fails the gate"
    );
}

#[test]
fn explicit_capabilities_distinguish_same_version_builds() {
    let mut state = AppState::default();
    state.devices = vec![
        Device {
            id: "personal".into(),
            name: "personal".into(),
            platform: "macos".into(),
            last_seen_at: None,
            created_at: None,
            version: Some("0.2.31".into()),
            cursor_sdk_version: None,
            capabilities: vec![zeron_proto::capabilities::MESSAGE_QUEUE_V1.into()],
        },
        Device {
            id: "upstream".into(),
            name: "upstream".into(),
            platform: "macos".into(),
            last_seen_at: None,
            created_at: None,
            version: Some("0.2.31".into()),
            cursor_sdk_version: None,
            capabilities: Vec::new(),
        },
    ];

    assert!(state.device_supports("personal", zeron_proto::capabilities::MESSAGE_QUEUE_V1));
    assert!(!state.device_supports("upstream", zeron_proto::capabilities::MESSAGE_QUEUE_V1));
}

#[test]
fn delivery_degradation_and_queued_sends_tell_the_truth() {
    use zeron_proto::{ChatConnectivity, ConnectivityState};
    let now = Utc::now();
    let mut s = AppState::default();
    s.local_device_id = Some("local".into());
    let mut remote = chat("c-remote", 0, None);
    remote.device_id = "remote".into();
    let mut local = chat("c-local", 0, None);
    local.device_id = "local".into();
    s.chats = vec![remote, local];
    s.devices = vec![Device {
        id: "remote".into(),
        name: "vps".into(),
        platform: "linux".into(),
        last_seen_at: Some(now),
        created_at: None,
        version: None,
        cursor_sdk_version: None,
        capabilities: Vec::new(),
    }];
    s.connectivity.state = ConnectivityState::Connected;
    s.connectivity.chats = vec![ChatConnectivity {
        chat_id: "c-remote".into(),
        connected: true,
        pending_pushes: 0,
    }];

    // Healthy: nothing degraded.
    assert!(!s.chat_delivery_degraded("c-remote"));
    assert!(!s.chat_delivery_degraded("c-local"));

    // The chat's own room down → degraded even while globally Connected.
    s.connectivity.chats[0].connected = false;
    assert!(s.chat_delivery_degraded("c-remote"));
    s.connectivity.chats[0].connected = true;

    // Host gone presence-dark → degraded (a send would queue at best).
    s.devices[0].last_seen_at = Some(now - TimeDelta::minutes(10));
    assert!(s.chat_delivery_degraded("c-remote"));
    s.devices[0].last_seen_at = Some(now);

    // OS offline: remote degrades; a locally-hosted chat NEVER does (the
    // queued command executes on this device even fully offline).
    s.connectivity.state = ConnectivityState::Offline;
    assert!(s.chat_delivery_degraded("c-remote"));
    assert!(!s.chat_delivery_degraded("c-local"));

    // Local profile (Disabled): nothing degrades.
    s.connectivity.state = ConnectivityState::Disabled;
    assert!(!s.chat_delivery_degraded("c-remote"));

    // Queued = pending send + degraded path — and degradation HOLDS the
    // overlay past the grace window instead of silently expiring (the
    // no-trace hole).
    s.connectivity.state = ConnectivityState::Offline;
    s.begin_pending_send("c-remote", "m1", now - TimeDelta::seconds(200));
    assert!(s.send_pending("c-remote", now));
    assert!(s.send_queued("c-remote", now));
    // Past the grace: the explicit failed state surfaces alongside.
    assert!(s.send_undelivered("c-remote", now));
    // Retry restarts the clock: back to Queued, no longer failed.
    s.retry_pending_send("c-remote", now);
    assert!(!s.send_undelivered("c-remote", now));
    assert!(s.send_queued("c-remote", now));
    // Path healed with a stale unacked send: the overlay expires after
    // the grace rather than holding forever.
    s.retry_pending_send("c-remote", now - TimeDelta::seconds(200));
    s.connectivity.state = ConnectivityState::Connected;
    assert!(!s.send_pending("c-remote", now));
    assert!(!s.send_queued("c-remote", now));
    // …but the explicit undelivered flag still tells the truth.
    assert!(s.send_undelivered("c-remote", now));
}
