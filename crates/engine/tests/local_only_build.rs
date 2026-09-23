use futures::StreamExt;
use skylark_engine::{AuthState, Engine, EngineConfig, EngineProfile, HarnessId, WorkspaceScope};
use skylark_proto::{LOCAL_ONLY_BUILD, LOCAL_ONLY_MESSAGE, capabilities};
use skylark_rpc::{RpcReply, RpcService, methods};

#[tokio::test]
async fn local_only_boot_preserves_cloud_data_and_rejects_product_services() {
    assert!(LOCAL_ONLY_BUILD);
    let dir = tempfile::tempdir().unwrap();
    let session = br#"{"refreshToken":"saved-token","user":{"id":"user_1","email":"u@example.com"},"orgId":"org_1"}"#;
    let session_path = dir.path().join("session.json");
    std::fs::write(&session_path, session).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = EngineConfig {
        data_dir: dir.path().into(),
        edge_url: format!("http://{}", listener.local_addr().unwrap()),
        edge_token: Some("user_1@org_1".into()),
        org_id: Some("org_1".into()),
        workos_client_id: Some("client_test".into()),
        ipc_port: 0,
        default_harness: HarnessId::Mock,
    };
    let auth = Engine::build_auth(&config).await;
    assert!(!auth.workos_enabled());
    assert!(!auth.loaded_workos_session());
    assert!(matches!(auth.state(), AuthState::SignedOut));
    assert!(auth.user_id().is_none());
    assert!(matches!(auth.access_token().await, Err(skylark_rpc::TokenError::SignedOut)));
    assert_eq!(
        Engine::initial_workspace_scope(&auth),
        WorkspaceScope::Local
    );
    for scope in [
        WorkspaceScope::Local,
        WorkspaceScope::Synced,
        WorkspaceScope::Development,
    ] {
        let profile = Engine::resolve_profile(&config, &auth, scope)
            .unwrap()
            .unwrap();
        assert_eq!(profile.scope(), WorkspaceScope::Local);
        assert_eq!(profile.store_root(), dir.path().join("profiles/local"));
    }
    let synced = EngineProfile::synced(dir.path(), "org_1", "user_1");
    assert!(
        Engine::assemble_runtime(&config, auth.clone(), synced)
            .await
            .is_err()
    );
    let profile = Engine::resolve_profile(&config, &auth, WorkspaceScope::Local)
        .unwrap()
        .unwrap();
    let runtime = Engine::assemble_runtime(&config, auth, profile)
        .await
        .unwrap();
    assert!(runtime.core().updater().is_none());
    let rpc = runtime.core().rpc_service();
    for method in [
        methods::SIGN_IN,
        methods::SIGN_IN_HEADLESS,
        methods::COMPLETE_SIGN_IN,
        methods::SIGN_OUT,
        methods::LIST_ORGS,
        methods::CREATE_ORG,
        methods::SELECT_ORG,
        methods::UPDATE_STATUS,
        methods::APPLY_UPDATE,
        methods::LOCAL_IMPORT_STATUS,
        methods::IMPORT_LOCAL_WORKSPACE,
    ] {
        let Err(error) = rpc.handle(method, serde_json::json!({})).await else {
            panic!("{method} must be blocked");
        };
        assert!(
            error.to_string().contains(LOCAL_ONLY_MESSAGE),
            "{method}: {error}"
        );
    }
    assert!(
        rpc.handle(methods::LIST_HARNESSES, serde_json::json!({}))
            .await
            .is_ok()
    );
    assert!(
        rpc.handle(
            methods::LIST_HARNESSES,
            serde_json::json!({"targetDeviceId":"other"})
        )
        .await
        .is_err()
    );
    let RpcReply::Stream(mut state) = rpc
        .handle(methods::AUTH_STATUS, serde_json::json!({}))
        .await
        .unwrap()
    else {
        panic!("auth status must remain observable");
    };
    assert_eq!(
        state.next().await.unwrap(),
        serde_json::to_value(AuthState::SignedOut).unwrap()
    );
    assert!(
        Engine::engine_info(&config, WorkspaceScope::Local)
            .unwrap()
            .supports(capabilities::LOCAL_ONLY_V1)
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), listener.accept())
            .await
            .is_err()
    );
    runtime.shutdown().await;
    assert_eq!(std::fs::read(&session_path).unwrap(), session);
    assert!(!dir.path().join("orgs").exists());
}
