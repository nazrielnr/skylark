use super::*;

    fn pin_change(id: &str) -> zeron_proto::SidebarPinChange {
        zeron_proto::SidebarPinChange::Pin {
            session_id: id.into(),
            after: None,
            before: None,
        }
    }
    use super::{
        pinned_drag_scroll_delta, pinned_drag_scroll_step, pinned_drag_snapshot_is_valid,
        pinned_session_clamped_index, pinned_session_drop_index, project_pinned_first,
        reorder_visible_pins, retain_known_pins,
    };
    use std::collections::HashSet;

    fn ids(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    fn pin_test_shell(
        cx: &mut gpui::TestAppContext,
        path: &std::path::Path,
    ) -> gpui::WindowHandle<super::Shell> {
        use super::*;
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
        });
        cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            Shell::new(
                state,
                EngineBootConfig {
                    data_dir: path.into(),
                    ipc_port: 0,
                    edge_url: "http://127.0.0.1:1".into(),
                    edge_token: None,
                    org_id: None,
                    workos_client_id: None,
                    default_harness: zeron_proto::HarnessId::Mock,
                },
                cx,
            )
        })
    }

    fn remote_pin_state(state: &mut super::AppState, synced: bool, initialized: bool) {
        state.workspace_scope = Some(zeron_proto::WorkspaceScope::Synced);
        state.auth = Some(zeron_proto::AuthState::SignedIn {
            user: zeron_proto::UserProfile {
                id: "user".into(),
                email: "test@example.com".into(),
                name: None,
            },
            org_id: Some("org".into()),
        });
        state.sidebar_preferences.synced = synced;
        state.sidebar_preferences.initialized = initialized;
    }

    fn pin_test_chat(id: &str) -> zeron_proto::Chat {
        serde_json::from_value(serde_json::json!({
            "id": id, "title": id, "deviceId": "local", "archived": false,
            "createdAt": chrono::Utc::now(),
        }))
        .unwrap()
    }

    fn pin_test_engine() -> (
        crate::state::EngineHandle,
        tokio::sync::mpsc::Receiver<String>,
        tokio::sync::mpsc::Sender<String>,
    ) {
        let (out, requests) = tokio::sync::mpsc::channel(16);
        let (replies, inbound) = tokio::sync::mpsc::channel(16);
        (
            crate::state::EngineHandle::from_test_client(zeron_rpc::RpcClient::new(out, inbound)),
            requests,
            replies,
        )
    }

    fn pin_snapshot(revision: u64, pins: &[&str]) -> zeron_proto::SidebarPreferencesState {
        zeron_proto::SidebarPreferencesState {
            revision,
            synced: true,
            initialized: true,
            pinned_session_ids: ids(pins),
        }
    }

    fn deliver_pin_rpc_reply(
        runtime: &tokio::runtime::Runtime,
        replies: &tokio::sync::mpsc::Sender<String>,
        reply: serde_json::Value,
    ) {
        // The current-thread reactor drains the actual RpcClient reader before
        // GPUI resumes. No wall-clock sleeps or hand-invoked completion handlers.
        runtime.block_on(async {
            replies.send(reply.to_string()).await.unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(1), async {
                while replies.capacity() < replies.max_capacity() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
        });
    }

    #[gpui::test]
    fn sidebar_rpc_replies_dispatch_each_queued_drop_once_and_recover_rejection(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let (engine, mut requests, replies) = pin_test_engine();
        let dir = tempfile::tempdir().unwrap();
        let window = pin_test_shell(cx, dir.path());
        window
            .update(cx, |shell, _, cx| {
                shell.state.update(cx, |state, _| {
                    remote_pin_state(state, true, true);
                    state.set_test_engine(engine);
                });
                let key = shell.active_sidebar_pin_profile_key(cx).unwrap();
                shell.apply_sidebar_pin_change(key.clone(), pin_change("first"), cx);
                shell.apply_sidebar_pin_change(key, pin_change("second"), cx);
            })
            .unwrap();
        cx.run_until_parked();
        let first: serde_json::Value = serde_json::from_str(&requests.try_recv().unwrap()).unwrap();
        assert_eq!(
            first["params"]["change"],
            serde_json::to_value(pin_change("first")).unwrap()
        );
        assert!(requests.try_recv().is_err());
        deliver_pin_rpc_reply(
            &runtime,
            &replies,
            serde_json::json!({"id": first["id"], "ok": {"ok": true, "sidebarPreferences": pin_snapshot(1, &["first"])}}),
        );
        cx.run_until_parked();
        let second: serde_json::Value =
            serde_json::from_str(&requests.try_recv().unwrap()).unwrap();
        assert_eq!(
            second["params"]["change"],
            serde_json::to_value(pin_change("second")).unwrap()
        );
        assert!(requests.try_recv().is_err());
        window
            .update(cx, |shell, _, cx| {
                assert_eq!(shell.active_sidebar_pins(cx), ids(&["first", "second"]));
                assert_eq!(
                    shell.state.read(cx).sidebar_preferences.pinned_session_ids,
                    ids(&["first"])
                );
                shell.state.update(cx, |state, _| {
                    state.apply_sidebar_preferences(pin_snapshot(3, &["remote"]));
                });
            })
            .unwrap();
        deliver_pin_rpc_reply(
            &runtime,
            &replies,
            serde_json::json!({"id": second["id"], "err": "rejected by test registry"}),
        );
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                assert_eq!(shell.active_sidebar_pins(cx), ids(&["remote"]));
                assert!(shell.sidebar_pin_write.is_none());
                assert!(
                    shell
                        .sidebar_notice
                        .as_deref()
                        .unwrap()
                        .contains("rejected by test registry")
                );
            })
            .unwrap();
        assert!(requests.try_recv().is_err());
    }

    #[gpui::test]
    fn sidebar_rpc_timeout_blocks_overtaking_until_the_late_reply_arrives(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let (engine, mut requests, replies) = pin_test_engine();
        let dir = tempfile::tempdir().unwrap();
        let window = pin_test_shell(cx, dir.path());
        window
            .update(cx, |shell, _, cx| {
                shell.state.update(cx, |state, _| {
                    remote_pin_state(state, true, true);
                    state.set_test_engine(engine);
                    state.sidebar_preferences = pin_snapshot(1, &["confirmed"]);
                });
                let key = shell.active_sidebar_pin_profile_key(cx).unwrap();
                shell.apply_sidebar_pin_change(key.clone(), pin_change("slow"), cx);
                shell.apply_sidebar_pin_change(key, pin_change("queued"), cx);
            })
            .unwrap();
        cx.run_until_parked();
        let request: serde_json::Value =
            serde_json::from_str(&requests.try_recv().unwrap()).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(21));
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                assert!(shell.sidebar_pin_write.as_ref().unwrap().unconfirmed);
                assert_eq!(shell.active_sidebar_pins(cx), ids(&["confirmed"]));
                let key = shell.active_sidebar_pin_profile_key(cx).unwrap();
                assert!(!shell.apply_sidebar_pin_change(key, pin_change("overtaking"), cx));
            })
            .unwrap();
        assert!(
            requests.try_recv().is_err(),
            "queued and fresh drops must not overtake the slow write"
        );
        deliver_pin_rpc_reply(
            &runtime,
            &replies,
            serde_json::json!({"id": request["id"], "ok": {"ok": true, "sidebarPreferences": pin_snapshot(2, &["slow"])}}),
        );
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                assert!(shell.sidebar_pin_write.is_none());
                assert_eq!(shell.active_sidebar_pins(cx), ids(&["slow"]));
                assert!(
                    shell.sidebar_notice.is_none(),
                    "late success must clear the waiting notice"
                );
                let key = shell.active_sidebar_pin_profile_key(cx).unwrap();
                assert!(shell.apply_sidebar_pin_change(key, pin_change("after-confirmation"), cx));
            })
            .unwrap();
        cx.run_until_parked();
        let next: serde_json::Value = serde_json::from_str(&requests.try_recv().unwrap()).unwrap();
        assert_eq!(
            next["params"]["change"],
            serde_json::to_value(pin_change("after-confirmation")).unwrap()
        );
        assert!(requests.try_recv().is_err());
    }

    #[gpui::test]
    fn sidebar_unconfirmed_write_stops_the_queue_without_overwriting_observed_pins(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _guard = runtime.enter();
        let (engine, _requests, _replies) = pin_test_engine();
        let dir = tempfile::tempdir().unwrap();
        let window = pin_test_shell(cx, dir.path());
        window
            .update(cx, |shell, _, cx| {
                shell.state.update(cx, |state, _| {
                    remote_pin_state(state, true, true);
                    state.set_test_engine(engine);
                    state.sidebar_preferences = pin_snapshot(2, &["confirmed"]);
                });
                let key = shell.active_sidebar_pin_profile_key(cx).unwrap();
                shell.apply_sidebar_pin_change(key.clone(), pin_change("first"), cx);
                shell.apply_sidebar_pin_change(key, pin_change("second"), cx);
                let id = shell.sidebar_pin_write.as_ref().unwrap().id;
                shell.mark_pin_write_unconfirmed(id, cx);
                assert!(shell.sidebar_pin_write.as_ref().unwrap().unconfirmed);
                assert_eq!(shell.active_sidebar_pins(cx), ids(&["confirmed"]));
                let key = shell.active_sidebar_pin_profile_key(cx).unwrap();
                assert!(!shell.apply_sidebar_pin_change(key, pin_change("third"), cx));
                assert_eq!(
                    shell.finish_sidebar_pin_write(id, Err("late error".into()), cx),
                    None
                );
                assert!(shell.sidebar_pin_write.is_none());
                shell.state.update(cx, |state, _| {
                    state.apply_sidebar_preferences(pin_snapshot(3, &["first"]));
                });
                assert_eq!(
                    shell.active_sidebar_pins(cx),
                    ids(&["first"]),
                    "a timed-out request may still commit and must be reconciled via its watch"
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn sidebar_optimistic_writes_preserve_newer_edits_and_watch_state_on_failure(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _guard = runtime.enter();
        let (engine, mut requests, _replies) = pin_test_engine();
        let dir = tempfile::tempdir().unwrap();
        let window = pin_test_shell(cx, dir.path());
        let write_id = window
            .update(cx, |shell, _, cx| {
                shell.state.update(cx, |state, _| {
                    remote_pin_state(state, false, true);
                    state.sidebar_preferences = pin_snapshot(1, &["original"]);
                    state.set_test_engine(engine);
                });
                let key = shell.active_sidebar_pin_profile_key(cx).unwrap();
                assert!(shell.apply_sidebar_pin_change(key.clone(), pin_change("first"), cx));
                assert!(shell.apply_sidebar_pin_change(key, pin_change("second"), cx));
                assert_eq!(
                    shell.active_sidebar_pins(cx),
                    ids(&["original", "first", "second"])
                );
                assert_eq!(
                    shell.state.read(cx).sidebar_preferences.pinned_session_ids,
                    ids(&["original"])
                );
                assert!(
                    shell.mutate_task.is_none(),
                    "pin writes cannot be cancelled by generic sidebar mutations"
                );
                shell.sidebar_pin_write.as_ref().unwrap().id
            })
            .unwrap();
        cx.run_until_parked();
        let request: serde_json::Value =
            serde_json::from_str(&requests.try_recv().unwrap()).unwrap();
        assert_eq!(
            request["params"]["change"],
            serde_json::to_value(pin_change("first")).unwrap()
        );
        assert!(
            requests.try_recv().is_err(),
            "only one write may be in flight"
        );
        window
            .update(cx, |shell, _, cx| {
                shell.state.update(cx, |state, _| {
                    state.apply_sidebar_preferences(pin_snapshot(7, &["remote"]));
                });
                assert_eq!(
                    shell.finish_sidebar_pin_write(write_id, Err("rejected".into()), cx),
                    Some(pin_change("second"))
                );
                assert_eq!(
                    shell.active_sidebar_pins(cx),
                    ids(&["remote", "second"]),
                    "an older failure must not roll back a newer drop"
                );
                assert_eq!(
                    shell.finish_sidebar_pin_write(write_id, Err("rejected".into()), cx),
                    None
                );
                assert_eq!(
                    shell.active_sidebar_pins(cx),
                    ids(&["remote"]),
                    "failure restores latest observed state, not the old backup"
                );
                assert!(
                    shell
                        .sidebar_notice
                        .as_deref()
                        .unwrap()
                        .contains("Couldn't save pins")
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn sidebar_write_acknowledgements_ignore_older_watches_and_previous_operations(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _guard = runtime.enter();
        let (engine, _requests, _replies) = pin_test_engine();
        let dir = tempfile::tempdir().unwrap();
        let window = pin_test_shell(cx, dir.path());
        window
            .update(cx, |shell, _, cx| {
                shell.state.update(cx, |state, _| {
                    remote_pin_state(state, true, true);
                    state.set_test_engine(engine);
                });
                let key = shell.active_sidebar_pin_profile_key(cx).unwrap();
                shell.apply_sidebar_pin_change(key.clone(), pin_change("saved"), cx);
                let first = shell.sidebar_pin_write.as_ref().unwrap().id;
                shell.finish_sidebar_pin_write(first, Ok(pin_snapshot(5, &["saved"])), cx);
                shell.state.update(cx, |state, _| {
                    assert!(!state.apply_sidebar_preferences(pin_snapshot(4, &["stale"])));
                });
                assert_eq!(shell.active_sidebar_pins(cx), ids(&["saved"]));
                shell.apply_sidebar_pin_change(key, pin_change("new-drop"), cx);
                let second = shell.sidebar_pin_write.as_ref().unwrap().id;
                shell.finish_sidebar_pin_write(first, Err("late failure".into()), cx);
                assert_eq!(shell.active_sidebar_pins(cx), ids(&["saved", "new-drop"]));
                shell.state.update(cx, |state, _| {
                    state.apply_sidebar_preferences(pin_snapshot(8, &["newer-remote"]));
                });
                shell.sidebar_notice = Some("Unrelated archive error".into());
                shell.finish_sidebar_pin_write(second, Ok(pin_snapshot(6, &["new-drop"])), cx);
                assert_eq!(shell.active_sidebar_pins(cx), ids(&["newer-remote"]));
                assert_eq!(
                    shell.sidebar_notice.as_deref(),
                    Some("Unrelated archive error")
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn sidebar_write_replies_cannot_cross_profile_or_engine_boundaries(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _guard = runtime.enter();
        let dir = tempfile::tempdir().unwrap();
        let window = pin_test_shell(cx, dir.path());
        for change_profile in [true, false] {
            let (engine, _requests, _replies) = pin_test_engine();
            let (replacement, _other_requests, _other_replies) = pin_test_engine();
            window
                .update(cx, |shell, _, cx| {
                    shell.state.update(cx, |state, _| {
                        remote_pin_state(state, true, true);
                        state.set_test_engine(engine);
                    });
                    let key = shell.active_sidebar_pin_profile_key(cx).unwrap();
                    shell.apply_sidebar_pin_change(key, pin_change("pending"), cx);
                    let id = shell.sidebar_pin_write.as_ref().unwrap().id;
                    shell.state.update(cx, |state, _| {
                        if change_profile {
                            state.workspace_scope = Some(zeron_proto::WorkspaceScope::Local);
                        } else {
                            state.set_test_engine(replacement);
                        }
                        state.sidebar_preferences = pin_snapshot(0, &["new-runtime"]);
                    });
                    shell.finish_sidebar_pin_write(id, Ok(pin_snapshot(99, &["old-runtime"])), cx);
                    assert_eq!(
                        shell.state.read(cx).sidebar_preferences.pinned_session_ids,
                        ids(&["new-runtime"])
                    );
                    assert!(shell.sidebar_pin_write.is_none());
                })
                .unwrap();
        }
    }

    #[gpui::test]
    fn sidebar_drop_without_engine_returns_without_an_optimistic_pin(
        cx: &mut gpui::TestAppContext,
    ) {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let window = pin_test_shell(cx, dir.path());
        window
            .update(cx, |shell, window, cx| {
                shell.state.update(cx, |state, _| {
                    remote_pin_state(state, false, true);
                    state.chats = vec![pin_test_chat("normal")];
                });
                shell.settings.space_filter = None;
                let payload = SidebarSessionDrag {
                    chat_id: "normal".into(),
                    visible_ids: std::sync::Arc::new(vec![]),
                    filter: None,
                    profile_key: shell.active_sidebar_pin_profile_key(cx).unwrap(),
                };
                shell.begin_sidebar_session_transfer(
                    &payload,
                    gpui::point(px(0.0), px(0.0)),
                    window,
                    cx,
                );
                shell.finish_sidebar_session_transfer(&payload, SidebarSessionDrop::Pinned(0), cx);
                assert!(shell.active_sidebar_pins(cx).is_empty());
                assert!(shell.sidebar_session_return.is_some());
                assert!(shell.sidebar_pin_write.is_none());
            })
            .unwrap();
    }

    #[gpui::test]
    fn sidebar_menu_and_drop_both_reject_unknown_remote_preferences(cx: &mut gpui::TestAppContext) {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let window = pin_test_shell(cx, dir.path());
        window
            .update(cx, |shell, window, cx| {
                shell.state.update(cx, |state, _| {
                    remote_pin_state(state, false, false);
                    state.chats = vec![pin_test_chat("normal")];
                });
                shell.settings.space_filter = None;
                shell.set_chat_pinned("normal".into(), true, cx);
                assert_eq!(
                    shell.sidebar_notice.as_deref(),
                    Some("Pins are still syncing")
                );
                let payload = SidebarSessionDrag {
                    chat_id: "normal".into(),
                    visible_ids: std::sync::Arc::new(vec![]),
                    filter: None,
                    profile_key: shell.active_sidebar_pin_profile_key(cx).unwrap(),
                };
                shell.begin_sidebar_session_transfer(
                    &payload,
                    gpui::point(px(0.0), px(0.0)),
                    window,
                    cx,
                );
                shell.finish_sidebar_session_transfer(&payload, SidebarSessionDrop::Pinned(0), cx);
                assert!(shell.active_sidebar_pins(cx).is_empty());
                assert!(!shell.state.read(cx).sidebar_preferences.initialized);
                assert_eq!(
                    shell.sidebar_notice.as_deref(),
                    Some("Pins are still syncing")
                );
                assert!(
                    shell.sidebar_session_return.is_some(),
                    "rejected drop returns to its origin"
                );
                assert!(shell.mutate_task.is_none());
            })
            .unwrap();
    }

    #[gpui::test]
    fn sidebar_full_capacity_drop_returns_without_mutating_local_or_remote_pins(
        cx: &mut gpui::TestAppContext,
    ) {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let window = pin_test_shell(cx, dir.path());
        window
            .update(cx, |shell, window, cx| {
                let saved: Vec<String> = (0..zeron_proto::MAX_SIDEBAR_PINS)
                    .map(|n| format!("hidden-{n}"))
                    .collect();
                for remote in [false, true] {
                    shell.state.update(cx, |state, _| {
                        if remote {
                            remote_pin_state(state, true, true);
                            state.sidebar_preferences.pinned_session_ids = saved.clone();
                        } else {
                            state.workspace_scope = Some(WorkspaceScope::Local);
                        }
                        state.chats = vec![pin_test_chat("normal")];
                    });
                    let key = shell.active_sidebar_pin_profile_key(cx).unwrap();
                    if !remote {
                        shell
                            .settings
                            .sidebar_pinned_session_ids_by_profile
                            .insert(key.clone(), saved.clone());
                    }
                    shell.settings.space_filter = None;
                    let payload = SidebarSessionDrag {
                        chat_id: "normal".into(),
                        visible_ids: std::sync::Arc::new(vec![]),
                        filter: None,
                        profile_key: key,
                    };
                    shell.begin_sidebar_session_transfer(
                        &payload,
                        gpui::point(px(0.0), px(0.0)),
                        window,
                        cx,
                    );
                    shell.finish_sidebar_session_transfer(
                        &payload,
                        SidebarSessionDrop::Pinned(0),
                        cx,
                    );
                    assert_eq!(shell.active_sidebar_pins(cx), saved);
                    assert_eq!(
                        shell.sidebar_notice.as_deref(),
                        Some("You can pin up to 200 sessions")
                    );
                    assert!(shell.sidebar_session_return.is_some());
                }
            })
            .unwrap();
    }

    #[gpui::test]
    fn sidebar_validation_preserves_offline_edits_and_rejects_stale_profiles(
        cx: &mut gpui::TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let window = pin_test_shell(cx, dir.path());
        window
            .update(cx, |shell, _, cx| {
                shell
                    .state
                    .update(cx, |state, _| remote_pin_state(state, false, true));
                let key = shell.active_sidebar_pin_profile_key(cx).unwrap();
                assert!(shell.validate_sidebar_pin_change(&key, &ids(&["cached"]), cx));
                assert!(!shell.validate_sidebar_pin_change(
                    "another-profile",
                    &ids(&["cached"]),
                    cx
                ));
                assert!(!shell.validate_sidebar_pin_change(
                    &key,
                    &ids(&["duplicate", "duplicate"]),
                    cx
                ));
                assert!(!shell.validate_sidebar_pin_change(&key, &ids(&[""]), cx));
                let saved: Vec<_> = (0..zeron_proto::MAX_SIDEBAR_PINS)
                    .map(|n| format!("pin-{n}"))
                    .collect();
                let reordered = super::sidebar_session_drop_pins(
                    &saved,
                    &saved,
                    &saved[0],
                    super::SidebarSessionDrop::Pinned(199),
                );
                assert!(shell.validate_sidebar_pin_change(&key, &reordered, cx));
                let unpinned = super::sidebar_session_drop_pins(
                    &saved,
                    &saved,
                    &saved[0],
                    super::SidebarSessionDrop::Regular,
                );
                assert!(shell.validate_sidebar_pin_change(&key, &unpinned, cx));
            })
            .unwrap();
    }

    #[gpui::test]
    fn sidebar_preferences_arriving_before_chats_never_prune_live_pins(
        cx: &mut gpui::TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let window = pin_test_shell(cx, dir.path());
        window
            .update(cx, |shell, _, cx| {
                shell.state.update(cx, |state, _| {
                    state.apply_chats(vec![]);
                    remote_pin_state(state, true, true);
                    state.sidebar_preferences.pinned_session_ids = ids(&["live-remote"]);
                });
                shell.on_state_changed(&shell.state.clone(), cx);
                assert_eq!(shell.active_sidebar_pins(cx), ids(&["live-remote"]));
                assert!(shell.mutate_task.is_none());
            })
            .unwrap();
    }

    #[gpui::test]
    fn sidebar_project_groups_preserve_pin_order_and_keyboard_order(cx: &mut gpui::TestAppContext) {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let window = pin_test_shell(cx, dir.path());
        window
            .update(cx, |shell, _, cx| {
                shell.settings.sidebar_organization = SidebarOrganization::ByProject;
                shell
                    .settings
                    .sidebar_pins_mut("local".into())
                    .push("pin".into());
                shell.state.update(cx, |state, _| {
                    state.workspace_scope = Some(WorkspaceScope::Local);
                    state.spaces = ["a", "b"].into_iter().map(|id| serde_json::from_value(serde_json::json!({
                        "id": id, "deviceId": "local", "path": format!("/project/{id}"), "createdAt": Utc::now()
                    })).unwrap()).collect();
                    state.chats = [("pin", "a"), ("b-new", "b"), ("a-new", "a"), ("b-old", "b")]
                        .into_iter()
                        .enumerate()
                        .map(|(ix, (id, project))| {
                            let mut chat = pin_test_chat(id);
                            chat.space_id = Some(project.into());
                            chat.created_at = Utc::now() - chrono::Duration::minutes(ix as i64);
                            chat
                        })
                        .collect();
                });
                assert_eq!(
                    shell.sidebar_visible_order(cx),
                    ids(&["pin", "b-new", "b-old", "a-new"])
                );
                shell.settings.space_filter = Some("a".into());
                assert_eq!(shell.sidebar_visible_order(cx), ids(&["pin", "a-new"]));
            })
            .unwrap();
        cx.run_until_parked();
    }

    #[gpui::test]
    fn sidebar_remote_pins_ignore_old_local_preferences(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let window = pin_test_shell(cx, dir.path());
        window
            .update(cx, |shell, _, cx| {
                shell.state.update(cx, |state, _| {
                    state.apply_chats(vec![]);
                    remote_pin_state(state, true, false);
                });
                let key = shell.active_sidebar_pin_profile_key(cx).unwrap();
                shell
                    .settings
                    .sidebar_pinned_session_ids_by_profile
                    .insert(key.clone(), ids(&["live-remote"]));
                shell.on_state_changed(&shell.state.clone(), cx);
                assert_eq!(shell.settings.sidebar_pins(&key), ids(&["live-remote"]));
                assert!(!shell.state.read(cx).sidebar_preferences.initialized);
            })
            .unwrap();
    }

    #[test]
    fn pins_lead_without_changing_unpinned_recency() {
        let recency = ids(&["newest", "p2", "middle", "p1", "oldest"]);
        assert_eq!(
            project_pinned_first(&recency, &ids(&["p1", "p2"])),
            ids(&["p1", "p2", "newest", "middle", "oldest"])
        );
    }

    #[test]
    fn inline_session_slide_retargets_and_returns_without_a_ghost() {
        use super::*;
        let mut slide = SidebarSessionSlide {
            from: 0.0,
            to: SIDEBAR_SESSION_SLOT,
            epoch: 1,
            started: std::time::Instant::now() - TAB_SLIDE.total(),
        };
        slide.retarget(2.0 * SIDEBAR_SESSION_SLOT);
        assert_eq!(slide.from, SIDEBAR_SESSION_SLOT);
        assert_eq!(slide.to, 2.0 * SIDEBAR_SESSION_SLOT);
        assert_eq!(slide.epoch, 2);
        slide.retarget(2.0 * SIDEBAR_SESSION_SLOT);
        assert_eq!(slide.epoch, 2);
        slide.started -= TAB_SLIDE.total();
        slide.retarget(0.0);
        assert_eq!(slide.from, 2.0 * SIDEBAR_SESSION_SLOT);
        assert_eq!(slide.to, 0.0);
    }

    #[test]
    fn session_layout_reverses_from_its_current_height() {
        use super::*;
        let mut slide = SidebarSessionSlide {
            from: 0.0,
            to: SIDEBAR_SESSION_SLOT,
            epoch: 0,
            started: std::time::Instant::now() - TAB_SLIDE.total() / 2,
        };
        let halfway = slide.current();
        assert!(halfway > 0.0 && halfway < SIDEBAR_SESSION_SLOT);
        slide.retarget(0.0);
        assert!((slide.from - halfway).abs() < 0.5);
        assert_eq!(slide.to, 0.0);
        slide.started -= TAB_SLIDE.total();
        assert_eq!(slide.current(), 0.0);
    }

    #[test]
    fn transfer_gaps_shift_neighbors_without_reordering_data() {
        use super::sidebar_gap_offset;
        // Entering from another section opens a full slot at the destination.
        assert_eq!(sidebar_gap_offset(0, None, 1, 63.0), 0.0);
        assert_eq!(sidebar_gap_offset(1, None, 1, 63.0), 63.0);
        assert_eq!(sidebar_gap_offset(2, None, 1, 63.0), 63.0);
        // Within a normal group, its original vacant slot is reused.
        assert_eq!(sidebar_gap_offset(0, Some(2), 0, 63.0), 63.0);
        assert_eq!(sidebar_gap_offset(1, Some(2), 0, 63.0), 63.0);
        assert_eq!(sidebar_gap_offset(2, Some(2), 0, 63.0), 0.0);
        assert_eq!(sidebar_gap_offset(1, Some(0), 3, 63.0), -63.0);
        assert_eq!(sidebar_gap_offset(3, Some(0), 3, 63.0), 0.0);
    }

    #[gpui::test]
    fn pinned_disclosure_click_collapses_and_restores_rows(cx: &mut gpui::TestAppContext) {
        use super::*;

        struct PinnedHost(Entity<Shell>);
        impl Render for PinnedHost {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                self.0.update(cx, |shell, cx| {
                    let items = (0..2)
                        .map(|_| div().h(px(61.0)).into_any_element())
                        .collect();
                    div()
                        .w(px(280.0))
                        .flex()
                        .flex_col()
                        .child(shell.render_pinned_section(items, 128.0, &Theme::default(), cx))
                })
            }
        }

        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
        });
        let (host, cx) = cx.add_window_view(|_, cx| {
            PinnedHost(cx.new(|cx| {
                let state = cx.new(|_| AppState::new());
                let shell = Shell::new(
                    state,
                    EngineBootConfig {
                        data_dir: dir.path().into(),
                        ipc_port: 0,
                        edge_url: "http://127.0.0.1:1".into(),
                        edge_token: None,
                        org_id: None,
                        workos_client_id: None,
                        default_harness: zeron_proto::HarnessId::Mock,
                    },
                    cx,
                );
                shell.state.update(cx, |state, _| {
                    state.workspace_scope = Some(WorkspaceScope::Development);
                    state.sidebar_preferences.pinned_session_ids = vec!["pin".into()];
                    state.chats = ["pin", "regular"]
                        .into_iter()
                        .map(|id| {
                            serde_json::from_value(serde_json::json!({
                                "id": id, "deviceId": "local", "archived": false,
                                "createdAt": Utc::now(),
                            }))
                            .unwrap()
                        })
                        .collect();
                });
                shell
            }))
        });
        let shell = host.read_with(cx, |host, _| host.0.clone());
        for open in [true, false, true] {
            if !open || !shell.read_with(cx, |shell, _| shell.pinned_open) {
                let toggle = cx.debug_bounds("pinned-toggle").unwrap().center();
                cx.simulate_mouse_down(toggle, MouseButton::Left, gpui::Modifiers::default());
                cx.simulate_mouse_up(toggle, MouseButton::Left, gpui::Modifiers::default());
            }
            shell.update(cx, |shell, cx| {
                assert_eq!(shell.pinned_open, open);
                let expected = if open {
                    vec!["pin", "regular"]
                } else {
                    vec!["regular"]
                };
                assert_eq!(shell.sidebar_visible_order(cx), expected);
                assert_eq!(shell.active_sidebar_pins(cx), ["pin"]);
                // Finish the tween to assert settled layout without wall-clock sleeps.
                shell.sidebar_disclosure_motion.clear();
                cx.notify();
            });
            cx.update(|window, cx| {
                window.refresh();
                window.draw(cx).clear();
            });
            let bounds = cx.debug_bounds("sidebar-pinned-section").unwrap();
            assert_eq!(
                f32::from(bounds.size.height),
                if open { 156.0 } else { 28.0 }
            );
            assert!(cx.debug_bounds("sidebar-pinned-divider").is_none());
        }
    }

    #[gpui::test]
    fn session_section_drops_preserve_live_activity_order(cx: &mut gpui::TestAppContext) {
        exercise_session_section_drops(cx, false, true);
    }

    #[gpui::test]
    fn compact_sidebar_section_drops_follow_pointer(cx: &mut gpui::TestAppContext) {
        exercise_session_section_drops(cx, true, false);
    }

    #[gpui::test]
    fn hidden_label_sidebar_section_drops_follow_pointer(cx: &mut gpui::TestAppContext) {
        exercise_session_section_drops(cx, false, false);
    }

    fn exercise_session_section_drops(
        cx: &mut gpui::TestAppContext,
        compact: bool,
        show_label: bool,
    ) {
        use super::*;

        struct SidebarHost(Entity<Shell>);
        impl Render for SidebarHost {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                self.0.update(cx, |shell, cx| {
                    div()
                        .w(px(280.0))
                        .h(px(800.0))
                        .on_drag_move::<SidebarSessionDrag>(
                            cx.listener(Shell::contain_pinned_session_drag),
                        )
                        .on_drop::<SidebarSessionDrag>(
                            cx.listener(|shell, _, _, cx| {
                                shell.cancel_sidebar_session_transfer(cx)
                            }),
                        )
                        .child(shell.render_chat_sidebar(&Theme::default(), cx))
                })
            }
        }
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
        });
        let (host, cx) = cx.add_window_view(|_, cx| {
            SidebarHost(cx.new(|cx| {
                let state = cx.new(|_| AppState::new());
                let mut shell = Shell::new(
                    state,
                    EngineBootConfig {
                        data_dir: dir.path().into(),
                        ipc_port: 0,
                        edge_url: "http://127.0.0.1:1".into(),
                        edge_token: None,
                        org_id: None,
                        workos_client_id: None,
                        default_harness: zeron_proto::HarnessId::Mock,
                    },
                    cx,
                );
                shell.settings.sidebar_organization = SidebarOrganization::InOneList;
                shell.settings.sidebar_show_branch = true;
                shell.settings.sidebar_compact = compact;
                shell.settings.sidebar_show_project_label = show_label;
                shell.state.update(cx, |state, _| {
                    state.workspace_scope = Some(WorkspaceScope::Local);
                    state.local_device_id = Some("local".into());
                    state.chats = ["older", "newer"]
                        .into_iter()
                        .enumerate()
                        .map(|(ix, id)| {
                            serde_json::from_value(serde_json::json!({
                                "id": id, "title": id, "deviceId": "local", "archived": false,
                                "sourceContext": {
                                    "checkoutId": "checkout", "repoRoot": "/project", "cwd": "/project",
                                    "branch": "feature/sidebar-drag", "observedAt": Utc::now(),
                                },
                                "createdAt": Utc::now() - chrono::Duration::minutes(10 - ix as i64),
                            }))
                            .unwrap()
                        })
                        .collect();
                    let mut archived = state.chats[0].clone();
                    archived.id = "archived".into();
                    archived.archived = true;
                    state.chats.push(archived);
                });
                shell
            }))
        });
        let shell = host.read_with(cx, |host, _| host.0.clone());

        // Archived rows share geometry and metadata in every sidebar layout.
        let active = cx.debug_bounds("chat-older").unwrap();
        let archived = cx.debug_bounds("chat-archived").unwrap();
        assert_eq!(active.size, archived.size);
        assert_eq!(cx.debug_bounds("chat-branch-archived").is_some(), !compact);
        assert_eq!(
            cx.debug_bounds("chat-device-archived").is_some(),
            !compact && show_label
        );
        if compact {
            assert!(cx.debug_bounds("chat-status-archived").is_some());
            let time = cx.debug_bounds("chat-time-archived").unwrap();
            cx.simulate_mouse_move(archived.center(), None, gpui::Modifiers::default());
            assert_eq!(cx.debug_bounds("chat-time-archived").unwrap(), time);
        }

        // Sessions owns all unpinned rows, including their keyboard traversal.
        for open in [false, true] {
            let toggle = cx.debug_bounds("sessions-toggle").unwrap().center();
            cx.simulate_mouse_down(toggle, MouseButton::Left, gpui::Modifiers::default());
            cx.simulate_mouse_up(toggle, MouseButton::Left, gpui::Modifiers::default());
            shell.update(cx, |shell, cx| {
                assert_eq!(shell.sessions_open, open);
                assert_eq!(
                    shell.sidebar_visible_order(cx),
                    if open {
                        ids(&["newer", "older"])
                    } else {
                        Vec::new()
                    }
                );
                shell.sidebar_disclosure_motion.clear();
                cx.notify();
            });
        }
        if compact {
            let status = cx.debug_bounds("chat-status-older").unwrap();
            let time = cx.debug_bounds("chat-time-older").unwrap();
            assert!(status.right() < time.left());
            let row = cx.debug_bounds("chat-older").unwrap();
            let title = cx.debug_bounds("chat-title-older").unwrap();
            cx.simulate_mouse_move(row.center(), None, gpui::Modifiers::default());
            assert!(cx.debug_bounds("chat-title-older").unwrap().size.width < title.size.width);
            assert_eq!(cx.debug_bounds("chat-status-older").unwrap(), status);
            assert_eq!(cx.debug_bounds("chat-time-older").unwrap(), time);
        }

        // Dragging a regular session over another regular session is a no-op.
        let source_bounds = cx.debug_bounds("chat-older").unwrap();
        let from = source_bounds.center();
        cx.simulate_mouse_down(from, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            from + gpui::point(px(8.0), px(0.0)),
            Some(MouseButton::Left),
            gpui::Modifiers::default(),
        );
        let dragged_card = cx.debug_bounds("chat-older").unwrap();
        assert!(cx.debug_bounds("drag-chat-older").is_none());
        assert_eq!(dragged_card.size, source_bounds.size);
        assert_eq!(dragged_card.origin.x, source_bounds.origin.x);
        assert_eq!(
            f32::from(dragged_card.size.height),
            super::super::sidebar_row_height(compact, show_label, true, false)
        );
        // Even a sub-row move follows the pointer immediately, before a drop slot changes.
        let pointer = from + gpui::point(px(8.0), px(7.0));
        cx.simulate_mouse_move(pointer, Some(MouseButton::Left), gpui::Modifiers::default());
        let moving = cx.debug_bounds("chat-older").unwrap();
        assert!((f32::from(moving.origin.y - source_bounds.origin.y) - 7.0).abs() < 1.0);
        let slot = cx.debug_bounds("session-slot-newer").unwrap();
        let target = gpui::point(slot.center().x, slot.top() + px(3.0));
        cx.simulate_mouse_move(target, Some(MouseButton::Left), gpui::Modifiers::default());
        shell.read_with(cx, |shell, cx| {
            let drag = shell.sidebar_session_transfer.as_ref().unwrap();
            assert_eq!(drag.preview.as_ref().unwrap().group, "regular");
            assert!(drag.siblings["newer"].to > 0.0);
            assert!(shell.active_sidebar_pins(cx).is_empty());
        });
        cx.simulate_mouse_up(target, MouseButton::Left, gpui::Modifiers::default());
        let returning_card = cx.debug_bounds("chat-older").unwrap();
        assert_eq!(returning_card.size, source_bounds.size);
        shell.update(cx, |shell, cx| {
            assert_eq!(shell.active_sidebar_pins(cx), Vec::<String>::new());
            assert!(shell.sidebar_session_return.is_some());
            assert_eq!(shell.sidebar_visible_order(cx), ids(&["newer", "older"]));
            shell.sidebar_session_return = None;
            cx.notify();
        });

        // The transient empty Pinned header accepts the first pin.
        let from = cx.debug_bounds("chat-older").unwrap().center();
        cx.simulate_mouse_down(from, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            from + gpui::point(px(8.0), px(0.0)),
            Some(MouseButton::Left),
            gpui::Modifiers::default(),
        );
        let target = cx.debug_bounds("pinned-toggle").unwrap().center();
        cx.simulate_mouse_move(target, Some(MouseButton::Left), gpui::Modifiers::default());
        shell.update(cx, |shell, cx| {
            let drag = shell.sidebar_session_transfer.as_mut().unwrap();
            assert_eq!(drag.source_collapse.to, drag.row_height + SIDEBAR_LIST_GAP);
            drag.source_collapse.started -= TAB_SLIDE.total();
            for gap in drag.section_gaps.values_mut() {
                gap.started -= TAB_SLIDE.total();
            }
            cx.notify();
        });
        assert_eq!(
            cx.debug_bounds("session-slot-older").unwrap().size.height,
            px(0.0)
        );
        // Only the surviving row (and minimum list spacing) remains, not a
        // second card-sized vacancy at the source.
        assert!(
            cx.debug_bounds("sidebar-regular-sessions")
                .unwrap()
                .size
                .height
                <= cx.debug_bounds("session-slot-newer").unwrap().size.height
                    + px(SIDEBAR_LIST_GAP)
        );
        cx.simulate_mouse_up(target, MouseButton::Left, gpui::Modifiers::default());
        shell.update(cx, |shell, cx| {
            assert_eq!(shell.active_sidebar_pins(cx), ids(&["older"]));
            assert_eq!(shell.sidebar_visible_order(cx), ids(&["older", "newer"]));
            assert!(
                shell.sidebar_resort.is_empty(),
                "successful pin must not replay the movement"
            );
            assert!(shell.sidebar_new_keys.is_empty());
            cx.notify();
        });

        // A single pin can leave Pinned and returns to its activity position.
        let from = cx.debug_bounds("chat-older").unwrap().center();
        cx.simulate_mouse_down(from, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            from + gpui::point(px(8.0), px(0.0)),
            Some(MouseButton::Left),
            gpui::Modifiers::default(),
        );
        let slot = cx.debug_bounds("session-slot-newer").unwrap();
        let target = gpui::point(slot.center().x, slot.top() + px(3.0));
        cx.simulate_mouse_move(target, Some(MouseButton::Left), gpui::Modifiers::default());
        shell.read_with(cx, |shell, cx| {
            assert!(shell.sidebar_transfer_extra_gap("regular") > 0.0);
            assert!(shell.sidebar_session_transfer.as_ref().unwrap().siblings["newer"].to > 0.0);
            assert_eq!(shell.active_sidebar_pins(cx), ids(&["older"]));
        });
        shell.update(cx, |shell, cx| {
            let drag = shell.sidebar_session_transfer.as_mut().unwrap();
            assert_eq!(drag.source_collapse.to, drag.row_height);
            drag.source_collapse.started -= TAB_SLIDE.total();
            for gap in drag.section_gaps.values_mut() {
                gap.started -= TAB_SLIDE.total();
            }
            cx.notify();
        });
        assert_eq!(
            cx.debug_bounds("session-slot-older").unwrap().size.height,
            px(0.0)
        );
        cx.simulate_mouse_up(target, MouseButton::Left, gpui::Modifiers::default());
        shell.update(cx, |shell, cx| {
            assert!(shell.active_sidebar_pins(cx).is_empty());
            assert_eq!(shell.sidebar_visible_order(cx), ids(&["newer", "older"]));
            assert!(
                shell.sidebar_resort.is_empty(),
                "successful unpin must not replay the movement"
            );
            assert!(shell.sidebar_new_keys.is_empty());
            cx.notify();
        });

        // A live activity update still wins while the normal session is dragged.
        let from = cx.debug_bounds("chat-older").unwrap().center();
        cx.simulate_mouse_down(from, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            from + gpui::point(px(8.0), px(0.0)),
            Some(MouseButton::Left),
            gpui::Modifiers::default(),
        );
        shell.update(cx, |shell, cx| {
            shell.state.update(cx, |state, _| {
                state
                    .chats
                    .iter_mut()
                    .find(|chat| chat.id == "older")
                    .unwrap()
                    .last_message_at = Some(Utc::now());
            });
            cx.notify();
        });
        let target = cx.debug_bounds("chat-newer").unwrap().center();
        cx.simulate_mouse_move(target, Some(MouseButton::Left), gpui::Modifiers::default());
        cx.simulate_mouse_up(target, MouseButton::Left, gpui::Modifiers::default());
        shell.update(cx, |shell, cx| {
            assert!(shell.active_sidebar_pins(cx).is_empty());
            assert_eq!(shell.sidebar_visible_order(cx), ids(&["older", "newer"]));
            assert!(shell.sidebar_session_return.is_some());
        });

        // A closed Pinned header remains a valid target and opens on success.
        shell.update(cx, |shell, cx| {
            shell.pinned_open = false;
            cx.notify();
        });
        let from = cx.debug_bounds("chat-newer").unwrap().center();
        cx.simulate_mouse_down(from, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            from + gpui::point(px(8.0), px(0.0)),
            Some(MouseButton::Left),
            gpui::Modifiers::default(),
        );
        let target = cx.debug_bounds("pinned-toggle").unwrap().center();
        cx.simulate_mouse_move(target, Some(MouseButton::Left), gpui::Modifiers::default());
        cx.simulate_mouse_up(target, MouseButton::Left, gpui::Modifiers::default());
        shell.update(cx, |shell, cx| {
            assert!(shell.pinned_open);
            assert_eq!(shell.active_sidebar_pins(cx), ids(&["newer"]));
            assert!(shell.sidebar_resort.is_empty());
            assert!(shell.sidebar_new_keys.is_empty());
            cx.notify();
        });

        // Dropping outside the sections cancels; it must not unpin the source.
        let from = cx.debug_bounds("chat-newer").unwrap().center();
        cx.simulate_mouse_down(from, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            from + gpui::point(px(8.0), px(0.0)),
            Some(MouseButton::Left),
            gpui::Modifiers::default(),
        );
        let regular = cx.debug_bounds("session-slot-older").unwrap().center();
        cx.simulate_mouse_move(regular, Some(MouseButton::Left), gpui::Modifiers::default());
        shell.update(cx, |shell, cx| {
            let drag = shell.sidebar_session_transfer.as_mut().unwrap();
            assert!(drag.source_collapse.to > 0.0);
            drag.source_collapse.started -= TAB_SLIDE.total() / 2;
            for gap in drag.section_gaps.values_mut() {
                gap.started -= TAB_SLIDE.total() / 2;
            }
            cx.notify();
        });
        let outside = gpui::point(px(275.0), px(790.0));
        cx.simulate_mouse_move(outside, Some(MouseButton::Left), gpui::Modifiers::default());
        cx.simulate_mouse_up(outside, MouseButton::Left, gpui::Modifiers::default());
        shell.update(cx, |shell, cx| {
            assert_eq!(shell.active_sidebar_pins(cx), ids(&["newer"]));
            assert!(shell.sidebar_session_transfer.is_none());
            let returning = shell.sidebar_session_return.as_ref().unwrap();
            assert!(returning.transfer.source_collapse.from > 0.0);
            assert_eq!(returning.transfer.source_collapse.to, 0.0);
            assert!(
                returning
                    .transfer
                    .section_gaps
                    .values()
                    .all(|gap| gap.to == 0.0)
            );
        });

        // Remote pin changes invalidate an in-flight snapshot instead of being overwritten.
        let from = cx.debug_bounds("chat-older").unwrap().center();
        cx.simulate_mouse_down(from, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            from + gpui::point(px(8.0), px(0.0)),
            Some(MouseButton::Left),
            gpui::Modifiers::default(),
        );
        shell.update(cx, |shell, cx| {
            let key = shell.active_sidebar_pin_profile_key(cx).unwrap();
            shell
                .settings
                .sidebar_pinned_session_ids_by_profile
                .insert(key, ids(&["older", "newer"]));
            cx.notify();
        });
        let target = cx.debug_bounds("pinned-toggle").unwrap().center();
        cx.simulate_mouse_move(target, Some(MouseButton::Left), gpui::Modifiers::default());
        cx.simulate_mouse_up(target, MouseButton::Left, gpui::Modifiers::default());
        shell.update(cx, |shell, cx| {
            assert_eq!(shell.active_sidebar_pins(cx), ids(&["older", "newer"]));
            shell.sidebar_resort.clear();
            shell.sidebar_new_keys.clear();
            shell.reduced_motion = true;
            cx.notify();
        });

        // Even with no regular rows, a temporary destination can unpin one.
        let from = cx.debug_bounds("chat-newer").unwrap().center();
        cx.simulate_mouse_down(from, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            from + gpui::point(px(8.0), px(0.0)),
            Some(MouseButton::Left),
            gpui::Modifiers::default(),
        );
        let target = cx
            .debug_bounds("sidebar-regular-sessions")
            .unwrap()
            .center();
        cx.simulate_mouse_move(target, Some(MouseButton::Left), gpui::Modifiers::default());
        cx.simulate_mouse_up(target, MouseButton::Left, gpui::Modifiers::default());
        shell.update(cx, |shell, cx| {
            assert_eq!(shell.active_sidebar_pins(cx), ids(&["older"]));
            assert_eq!(shell.sidebar_visible_order(cx), ids(&["older", "newer"]));
            assert!(shell.sidebar_resort.is_empty());
            assert!(shell.sidebar_new_keys.is_empty());
            cx.notify();
        });

        // Dropping in the lower half of a pinned row inserts after that row.
        let from = cx.debug_bounds("chat-newer").unwrap().center();
        cx.simulate_mouse_down(from, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            from + gpui::point(px(8.0), px(0.0)),
            Some(MouseButton::Left),
            gpui::Modifiers::default(),
        );
        let row = cx.debug_bounds("chat-older").unwrap();
        let target = gpui::point(row.center().x, row.bottom() - px(3.0));
        cx.simulate_mouse_move(target, Some(MouseButton::Left), gpui::Modifiers::default());
        shell.read_with(cx, |shell, cx| {
            assert!(shell.sidebar_transfer_extra_gap("pinned") > 0.0);
            assert_eq!(shell.active_sidebar_pins(cx), ids(&["older"]));
        });
        cx.simulate_mouse_up(target, MouseButton::Left, gpui::Modifiers::default());
        shell.update(cx, |shell, cx| {
            assert_eq!(shell.active_sidebar_pins(cx), ids(&["older", "newer"]));
            assert!(shell.sidebar_resort.is_empty());
            assert!(shell.sidebar_new_keys.is_empty());
            cx.notify();
        });

        // Existing pin-to-pin reordering still works with the shared payload.
        shell.update(cx, |shell, cx| {
            shell.reduced_motion = false;
            cx.notify();
        });
        let from = cx.debug_bounds("chat-newer").unwrap().center();
        cx.simulate_mouse_down(from, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            from + gpui::point(px(8.0), px(0.0)),
            Some(MouseButton::Left),
            gpui::Modifiers::default(),
        );
        let row = cx.debug_bounds("chat-older").unwrap();
        let target = gpui::point(row.center().x, row.top() + px(3.0));
        cx.simulate_mouse_move(target, Some(MouseButton::Left), gpui::Modifiers::default());
        cx.simulate_mouse_up(target, MouseButton::Left, gpui::Modifiers::default());
        shell.update(cx, |shell, cx| {
            assert_eq!(shell.active_sidebar_pins(cx), ids(&["newer", "older"]));
            assert!(
                shell.sidebar_resort.is_empty(),
                "successful reorder must not replay the movement"
            );
            assert!(shell.sidebar_new_keys.is_empty());
            assert!(shell.sidebar_session_transfer.is_none());
            assert!(shell.sidebar_session_return.is_none());
        });

        // Suppressing a completed drag must not disable later activity-driven glides.
        shell.update(cx, |shell, cx| {
            let key = shell.active_sidebar_pin_profile_key(cx).unwrap();
            for id in shell.active_sidebar_pins(cx) {
                shell.apply_sidebar_pin_change(
                    key.clone(),
                    zeron_proto::SidebarPinChange::Unpin { session_id: id },
                    cx,
                );
            }
            cx.notify();
        });
        let epoch = shell.update(cx, |shell, cx| {
            assert_eq!(shell.sidebar_visible_order(cx), ids(&["older", "newer"]));
            shell.resort_epoch
        });
        shell.update(cx, |shell, cx| {
            shell.state.update(cx, |state, _| {
                state
                    .chats
                    .iter_mut()
                    .find(|chat| chat.id == "newer")
                    .unwrap()
                    .last_message_at = Some(Utc::now() + chrono::Duration::seconds(1));
            });
            cx.notify();
        });
        shell.update(cx, |shell, cx| {
            assert_eq!(shell.sidebar_visible_order(cx), ids(&["newer", "older"]));
            assert!(shell.resort_epoch > epoch);
            assert!(!shell.sidebar_resort.is_empty());
        });
        // A collapsed Sessions header remains an unpin target and opens on drop.
        shell.update(cx, |shell, cx| {
            let key = shell.active_sidebar_pin_profile_key(cx).unwrap();
            *shell.settings.sidebar_pins_mut(key) = ids(&["older"]);
            shell.sidebar_session_return = None;
            shell.sidebar_resort.clear();
            shell.sidebar_disclosure_motion.clear();
            cx.notify();
        });
        let toggle = cx.debug_bounds("sessions-toggle").unwrap().center();
        cx.simulate_mouse_down(toggle, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(toggle, MouseButton::Left, gpui::Modifiers::default());
        shell.update(cx, |shell, cx| {
            assert!(!shell.sessions_open);
            shell.sidebar_disclosure_motion.clear();
            cx.notify();
        });
        let from = cx.debug_bounds("chat-older").unwrap().center();
        cx.simulate_mouse_down(from, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            from + gpui::point(px(8.0), px(0.0)),
            Some(MouseButton::Left),
            gpui::Modifiers::default(),
        );
        let target = cx.debug_bounds("sessions-toggle").unwrap().center();
        cx.simulate_mouse_move(target, Some(MouseButton::Left), gpui::Modifiers::default());
        cx.simulate_mouse_up(target, MouseButton::Left, gpui::Modifiers::default());
        shell.read_with(cx, |shell, cx| {
            assert!(shell.sessions_open);
            assert!(shell.active_sidebar_pins(cx).is_empty());
        });
    }

    #[test]
    fn missing_duplicate_and_archived_pins_do_not_disturb_regular_rows() {
        let recency = ids(&["b", "a", "c"]);
        assert_eq!(
            project_pinned_first(&recency, &ids(&["archived", "a", "a"])),
            ids(&["a", "b", "c"])
        );
    }

    #[test]
    fn filtered_pin_reorder_preserves_every_other_pins_relative_order() {
        let saved = ids(&["a1", "b1", "a2", "archived", "b2"]);
        let visible = ids(&["a1", "a2"]);
        assert_eq!(
            reorder_visible_pins(&saved, &visible, 0, 1),
            ids(&["b1", "a2", "a1", "archived", "b2"])
        );
    }

    #[test]
    fn pin_reorder_rejects_invalid_or_noop_moves() {
        let saved = ids(&["a", "b"]);
        assert_eq!(reorder_visible_pins(&saved, &saved, 0, 0), saved);
        assert_eq!(reorder_visible_pins(&saved, &saved, 8, 0), saved);
    }

    #[test]
    fn pin_cleanup_retains_archived_and_prunes_deleted() {
        let mut saved = ids(&["active", "archived", "deleted", "active"]);
        let known = HashSet::from(["active".to_string(), "archived".to_string()]);
        assert!(retain_known_pins(&mut saved, &known));
        assert_eq!(saved, ids(&["active", "archived"]));
        assert!(!retain_known_pins(&mut saved, &known));
    }

    #[test]
    fn pin_cleanup_for_one_profile_leaves_other_profiles_untouched() {
        let mut settings = crate::settings::UiSettings::default();
        settings
            .sidebar_pins_mut("local".to_string())
            .extend(ids(&["local-active", "local-deleted"]));
        settings
            .sidebar_pins_mut("synced:org-1:user-1".to_string())
            .push("synced-pin".to_string());

        let known = HashSet::from(["local-active".to_string()]);
        assert!(retain_known_pins(
            settings.sidebar_pins_mut("local".to_string()),
            &known,
        ));
        assert_eq!(settings.sidebar_pins("local"), ["local-active"]);
        assert_eq!(settings.sidebar_pins("synced:org-1:user-1"), ["synced-pin"]);
    }

    #[test]
    fn sidebar_hit_testing_uses_compact_and_mixed_row_heights() {
        assert_eq!(super::row_drop_index(31.0, &[29.0, 29.0], false), Some(1));
        assert_eq!(super::row_drop_index(61.0, &[29.0, 29.0], false), None);
        assert_eq!(super::row_drop_index(48.0, &[45.0, 63.0], false), Some(1));
        assert_eq!(super::row_drop_index(-1.0, &[29.0], false), None);
        assert_eq!(super::row_drop_index(-1.0, &[29.0], true), Some(0));
        assert_eq!(super::row_drop_index(500.0, &[29.0, 45.0], true), Some(1));
    }

    #[test]
    fn pinned_drop_index_quantizes_clamps_and_rejects_outside() {
        assert_eq!(pinned_session_drop_index(-1.0, 3), None);
        assert_eq!(pinned_session_drop_index(0.0, 3), Some(0));
        assert_eq!(
            pinned_session_drop_index(super::super::SIDEBAR_SESSION_SLOT, 3),
            Some(1)
        );
        assert_eq!(pinned_session_drop_index(500.0, 3), None);
        assert_eq!(pinned_session_drop_index(0.0, 0), None);
    }

    #[test]
    fn pinned_drag_accounts_for_disclosure_header_and_scroll() {
        // The first row starts 36px below the scroll viewport: 4px list
        // padding, 28px header, and 4px body inset.
        let viewport_top = 100.0;
        let row_top = 136.0;
        assert_eq!(
            super::pinned_session_pointer_y(row_top, viewport_top, 0.0),
            0.0
        );
        assert_eq!(
            pinned_session_drop_index(super::pinned_session_pointer_y(125.0, viewport_top, 0.0), 3),
            None,
        );
        assert_eq!(
            pinned_session_drop_index(
                super::pinned_session_pointer_y(row_top, viewport_top, 63.0),
                3
            ),
            Some(1),
        );
    }

    #[test]
    fn sidebar_wide_pin_drag_clamps_to_the_nearest_pinned_slot() {
        assert_eq!(pinned_session_clamped_index(-50.0, 3), Some(0));
        assert_eq!(
            pinned_session_clamped_index(super::super::SIDEBAR_SESSION_SLOT, 3),
            Some(1)
        );
        assert_eq!(pinned_session_clamped_index(500.0, 3), Some(2));
        assert_eq!(pinned_session_clamped_index(0.0, 0), None);
    }

    #[test]
    fn session_transfers_only_change_pin_membership_and_order() {
        use super::{SidebarSessionDrop, sidebar_session_drop_pins};
        let saved = ids(&["hidden", "a", "b", "hidden-tail"]);
        let visible = ids(&["a", "b"]);
        assert_eq!(
            sidebar_session_drop_pins(&saved, &visible, "normal", SidebarSessionDrop::Regular),
            saved
        );
        assert_eq!(
            sidebar_session_drop_pins(&saved, &visible, "normal", SidebarSessionDrop::Pinned(1)),
            ids(&["hidden", "a", "normal", "b", "hidden-tail"])
        );
        assert_eq!(
            sidebar_session_drop_pins(&saved, &visible, "normal", SidebarSessionDrop::Pinned(2)),
            ids(&["hidden", "a", "b", "normal", "hidden-tail"])
        );
        assert_eq!(
            sidebar_session_drop_pins(&saved, &visible, "a", SidebarSessionDrop::Regular),
            ids(&["hidden", "b", "hidden-tail"])
        );
        assert_eq!(
            sidebar_session_drop_pins(&saved, &visible, "a", SidebarSessionDrop::Pinned(1)),
            ids(&["hidden", "b", "a", "hidden-tail"])
        );
        assert_eq!(
            sidebar_session_drop_pins(&[], &[], "first", SidebarSessionDrop::Pinned(0)),
            ids(&["first"])
        );
        assert!(
            sidebar_session_drop_pins(
                &ids(&["only"]),
                &ids(&["only"]),
                "only",
                SidebarSessionDrop::Regular
            )
            .is_empty()
        );
    }

    #[test]
    fn pinned_edge_scroll_is_proportional_and_lifecycle_bound() {
        let top = 100.0;
        let bottom = 300.0;
        assert_eq!(pinned_drag_scroll_delta(200.0, top, bottom), 0.0);
        assert_eq!(pinned_drag_scroll_delta(124.0, top, bottom), -6.0);
        assert_eq!(pinned_drag_scroll_delta(276.0, top, bottom), 6.0);
        assert_eq!(
            pinned_drag_scroll_step(true, 4, 4, 20.0, 100.0, 6.0),
            Some(26.0)
        );
        assert_eq!(pinned_drag_scroll_step(false, 4, 4, 20.0, 100.0, 6.0), None);
        assert_eq!(pinned_drag_scroll_step(true, 3, 4, 20.0, 100.0, 6.0), None);
    }

    #[test]
    fn pinned_drag_snapshot_requires_the_same_remote_order() {
        let snapshot = ids(&["a", "b"]);
        assert!(pinned_drag_snapshot_is_valid("a", &snapshot, &snapshot));
        assert!(!pinned_drag_snapshot_is_valid(
            "a",
            &snapshot,
            &ids(&["b", "a"])
        ));
        assert!(!pinned_drag_snapshot_is_valid("a", &snapshot, &ids(&["a"])));
    }

    use chrono::{TimeZone as _, Utc};

    use super::{compare_sidebar_chats, promote_local_device_group};
    use crate::settings::SidebarSort;

    fn group(device: &str, value: u8) -> (Option<(String, String)>, Vec<u8>) {
        (Some((device.into(), device.into())), vec![value])
    }

    fn chat(id: &str) -> zeron_proto::Chat {
        zeron_proto::Chat {
            id: id.into(),
            device_id: "device".into(),
            title: None,
            archived: false,
            cwd: None,
            branch: None,
            checkout_id: None,
            source_context: None,
            config: None,
            last_message_preview: None,
            last_message_at: Some(Utc.timestamp_opt(10, 0).unwrap()),
            created_at: Utc.timestamp_opt(5, 0).unwrap(),
            harness_session_id: None,
            harness_session_cwd: None,
            space_id: None,
            last_seen_at: None,
            room_gen: None,
        }
    }

    #[test]
    fn equal_sidebar_timestamps_sort_by_stable_chat_id() {
        let alpha = chat("alpha");
        let beta = chat("beta");
        assert!(compare_sidebar_chats(SidebarSort::Created, &alpha, &beta).is_lt());
        assert!(compare_sidebar_chats(SidebarSort::LastUpdated, &alpha, &beta).is_lt());
    }

    #[test]
    fn current_device_is_promoted_without_resorting_remote_groups() {
        let mut groups = vec![
            group("recent-remote", 1),
            group("local", 2),
            group("older-remote", 3),
        ];

        promote_local_device_group(&mut groups, Some("local"));

        let order: Vec<_> = groups
            .iter()
            .map(|(group, _)| group.as_ref().unwrap().0.as_str())
            .collect();
        assert_eq!(order, ["local", "recent-remote", "older-remote"]);
    }

    #[test]
    fn missing_current_device_leaves_group_order_untouched() {
        let mut groups = vec![group("first", 1), group("second", 2)];
        let before = groups.clone();

        promote_local_device_group(&mut groups, Some("not-present"));

        assert_eq!(groups, before);
    }

    #[gpui::test]
    fn devices_locations_folders_and_back_clear_stale_state(cx: &mut gpui::TestAppContext) {
        let data = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
        });
        let shell = cx.new(|cx| {
            let state = cx.new(|_| {
                let mut state = AppState::new();
                state.devices = serde_json::from_value(serde_json::json!([
                    {"id":"local","name":"Studio","platform":"macos","lastSeenAt":null},
                    {"id":"remote","name":"Server","platform":"linux","lastSeenAt":null}
                ]))
                .unwrap();
                state
            });
            Shell::new(
                state,
                EngineBootConfig {
                    data_dir: data.path().into(),
                    ipc_port: 0,
                    edge_url: String::new(),
                    edge_token: None,
                    org_id: None,
                    workos_client_id: None,
                    default_harness: zeron_proto::HarnessId::Mock,
                },
                cx,
            )
        });
        shell.update(cx, |shell, cx| {
            shell.open_add_space(cx);
            assert_eq!(shell.add_space.as_ref().unwrap().step, ProjectStep::Devices);
            assert!(shell.add_space.as_ref().unwrap().device.is_none());
            let search = shell.add_space.as_ref().unwrap().search.clone();
            search.update(cx, |input, cx| input.set_text("server", cx));
            assert_eq!(shell.add_space_devices(cx).len(), 1);
            shell.add_space_open_active(cx);
            let flow = shell.add_space.as_mut().unwrap();
            assert_eq!(flow.step, ProjectStep::Locations);
            assert_eq!(flow.device.as_ref().unwrap().id, "remote");
            assert!(flow.search.read(cx).is_empty());
            flow.drives = Loadable::Ready(vec![DriveEntry {
                name: "Projects".into(),
                path: "/projects".into(),
            }]);
            search.update(cx, |input, cx| input.set_text("projects", cx));
            shell.add_space_open_active(cx);
            let flow = shell.add_space.as_mut().unwrap();
            assert_eq!(flow.step, ProjectStep::Folders);
            assert_eq!(flow.browser_path.as_deref(), Some("/projects"));
            flow.browser = Loadable::Ready(FolderListing {
                path: "/projects".into(),
                entries: Vec::new(),
                truncated: false,
            });
            shell.add_space_go_up(cx);
            assert_eq!(
                shell.add_space.as_ref().unwrap().step,
                ProjectStep::Locations
            );
            assert!(shell.add_space.as_ref().unwrap().browser.ready().is_none());
            shell.add_space_go_up(cx);
            let flow = shell.add_space.as_ref().unwrap();
            assert_eq!(flow.step, ProjectStep::Devices);
            assert!(flow.device.is_none());
            assert!(flow.drives.ready().is_none());
            assert!(flow.search.read(cx).is_empty());
            // Slash navigation only applies to folders, never device search.
            search.update(cx, |input, cx| input.set_text("/projects/", cx));
            assert!(!shell.add_space_slash_descend(cx));
        });
    }
