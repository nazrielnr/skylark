//! Shell regression and integration tests.

use super::*;


    #[test]
    fn sidebar_drag_nudges_each_edge_once_until_rearmed() {
        let min = sidebar_drag_sample(SIDEBAR_MIN, None, false);
        assert_eq!(min.width, SIDEBAR_MIN);
        assert_eq!(min.edge, Some(motion::ResizeEdge::Min));
        assert!(min.starts_bounce);

        let held_min = sidebar_drag_sample(SIDEBAR_MIN - 80.0, min.edge, false);
        assert_eq!(held_min.width, SIDEBAR_MIN);
        assert_eq!(held_min.edge, min.edge);
        assert!(!held_min.starts_bounce);

        let inside = sidebar_drag_sample(SIDEBAR_MIN + 1.0, held_min.edge, false);
        assert_eq!(inside.edge, None);
        assert!(!inside.starts_bounce);

        let rearmed_min = sidebar_drag_sample(SIDEBAR_MIN - 1.0, inside.edge, false);
        assert!(rearmed_min.starts_bounce);

        let max = sidebar_drag_sample(SIDEBAR_MAX, rearmed_min.edge, false);
        assert_eq!(max.width, SIDEBAR_MAX);
        assert_eq!(max.edge, Some(motion::ResizeEdge::Max));
        assert!(max.starts_bounce);

        let held_max = sidebar_drag_sample(SIDEBAR_MAX + 80.0, max.edge, false);
        assert_eq!(held_max.width, SIDEBAR_MAX);
        assert!(!held_max.starts_bounce);
    }

    #[test]
    fn sidebar_drag_stays_exact_in_range_and_reduced_motion_never_nudges() {
        let middle = sidebar_drag_sample(312.0, None, false);
        assert_eq!(middle.width, 312.0);
        assert_eq!(middle.edge, None);
        assert!(!middle.starts_bounce);

        for pointer_x in [
            SIDEBAR_MIN - 100.0,
            SIDEBAR_MIN,
            SIDEBAR_MAX,
            SIDEBAR_MAX + 100.0,
        ] {
            let sample = sidebar_drag_sample(pointer_x, None, true);
            assert!((SIDEBAR_MIN..=SIDEBAR_MAX).contains(&sample.width));
            assert!(!sample.starts_bounce);
        }
    }

    #[test]
    fn right_pane_uses_the_shared_clamp_and_edge_latch() {
        let min =
            motion::resize_drag_sample(RIGHT_PANE_MIN - 40.0, RIGHT_PANE_MIN, 820.0, None, false);
        assert_eq!(min.width, RIGHT_PANE_MIN);
        assert_eq!(min.edge, Some(motion::ResizeEdge::Min));
        assert!(min.starts_bounce);

        let held = motion::resize_drag_sample(
            RIGHT_PANE_MIN - 80.0,
            RIGHT_PANE_MIN,
            820.0,
            min.edge,
            false,
        );
        assert!(!held.starts_bounce);

        let max = motion::resize_drag_sample(900.0, RIGHT_PANE_MIN, 820.0, None, false);
        assert_eq!(max.width, 820.0);
        assert_eq!(max.edge, Some(motion::ResizeEdge::Max));
        assert!(max.starts_bounce);
    }

    #[test]
    fn sidebar_bounce_has_rounded_out_and_return_phases() {
        assert_eq!(
            motion::resize_bounce_offset(motion::ResizeEdge::Max, 0.0),
            0.0
        );
        assert_eq!(
            motion::resize_bounce_offset(
                motion::ResizeEdge::Max,
                motion::RESIZE_EDGE_BOUNCE_OUT_FRACTION
            ),
            motion::RESIZE_EDGE_NUDGE
        );
        assert_eq!(
            motion::resize_bounce_offset(motion::ResizeEdge::Max, 1.0),
            0.0
        );

        let gentle_start = motion::resize_bounce_offset(motion::ResizeEdge::Max, 0.01);
        let outbound = motion::resize_bounce_offset(motion::ResizeEdge::Max, 0.2);
        let returning = motion::resize_bounce_offset(motion::ResizeEdge::Max, 0.7);
        assert!(gentle_start > 0.0 && gentle_start < 0.1);
        assert!(outbound > gentle_start && outbound < motion::RESIZE_EDGE_NUDGE);
        assert!(returning > 0.0 && returning < motion::RESIZE_EDGE_NUDGE);
        assert_eq!(
            motion::resize_bounce_offset(motion::ResizeEdge::Min, 0.2),
            -outbound
        );
    }

    #[test]
    fn every_default_shortcut_binds_on_this_platform() {
        // `apply_keymap` silently falls back on an unparseable combo, so a
        // default gpui cannot parse would ship as a dead shortcut.
        for id in crate::settings::ShortcutId::ALL {
            let combo = platform_combo(id.default_combo());
            assert!(
                Keystroke::parse(&combo).is_ok(),
                "{} default {combo:?} does not parse",
                id.label()
            );
        }
    }

    #[test]
    fn island_stays_centered_on_controls_while_expanding() {
        let center = (Theme::TITLEBAR_HEIGHT + Theme::TITLEBAR_TOP_PAD) * 0.5;
        for step in 0..=20 {
            let (top, height) = titlebar_island_vertical_geometry(step as f32 / 20.0);
            assert_eq!(top + height * 0.5, center);
            assert!((28.0..=32.0).contains(&height));
        }
        let (top, height) = titlebar_island_vertical_geometry(1.0);
        assert_eq!(center, 21.0);
        assert_eq!(center - 12.0 - top, 4.0);
        assert_eq!(top + height - (center + 12.0), 4.0);
    }

    #[test]
    fn new_thread_handoff_is_continuous_and_staged() {
        assert!(bottom_stack_measurement_matches(false, false));
        assert!(bottom_stack_measurement_matches(true, true));
        assert!(!bottom_stack_measurement_matches(false, true));
        assert!(!bottom_stack_measurement_matches(true, false));
        assert_eq!(new_thread_background_opacity(false), 1.0);
        assert_eq!(
            new_thread_background_opacity(true),
            NEW_THREAD_BACKGROUND_FROSTED_OPACITY
        );
        assert_eq!(new_thread_background_height(400.0), 288.0);
        assert!((new_thread_background_height(600.0) - 432.0).abs() < 0.001);
        assert_eq!(new_thread_background_height(1_000.0), 720.0);
        assert_eq!(new_thread_background_height(1_200.0), 760.0);
        assert!(new_thread_background_height(848.0) > 848.0 / 2.0);
    }

    #[test]
    fn right_pane_ceiling_preserves_the_chat_floor() {
        assert_eq!(right_pane_max_width(1200.0, 256.0), 644.0);
        assert_eq!(1200.0 - 256.0 - 644.0, CHAT_PANEL_MIN);
        // The chat floor wins over the right pane's preferred 360px minimum
        // when the whole window is unusually narrow.
        assert_eq!(right_pane_max_width(800.0, 256.0), 244.0);
        assert_eq!(800.0 - 256.0 - 244.0, CHAT_PANEL_MIN);
    }

    #[test]
    fn right_pane_takeover_consumes_the_chat_column() {
        assert_eq!(right_pane_takeover_width(1200.0, 256.0), 944.0);
        assert_eq!(1200.0 - 256.0 - 944.0, 0.0);
    }

    #[test]
    fn escape_interrupts_only_the_active_live_chat() {
        assert_eq!(
            resolve_shell_escape(
                "escape",
                false,
                true,
                Route::Chat,
                Some("chat-a"),
                Indicator::Working,
                false,
            ),
            ShellEscapeOutcome::InterruptChat("chat-a".to_owned())
        );
        assert_eq!(
            resolve_shell_escape(
                "escape",
                false,
                true,
                Route::Chat,
                Some("chat-b"),
                Indicator::AwaitingInput,
                false,
            ),
            ShellEscapeOutcome::InterruptChat("chat-b".to_owned())
        );
    }

    #[test]
    fn escape_ignores_non_live_or_ineligible_views() {
        assert_eq!(
            resolve_shell_escape(
                "escape",
                true,
                true,
                Route::Chat,
                Some("chat-a"),
                Indicator::Working,
                false,
            ),
            ShellEscapeOutcome::Blocked
        );
        for (route, selected, indicator, interrupting) in [
            (Route::Chat, Some("chat-a"), Indicator::None, false),
            (Route::Chat, None, Indicator::Working, false),
            (Route::Chat, Some("chat-a"), Indicator::Working, true),
            (
                Route::Settings(SettingsSection::Devices),
                Some("chat-a"),
                Indicator::Working,
                false,
            ),
        ] {
            assert_eq!(
                resolve_shell_escape(
                    "escape",
                    false,
                    true,
                    route,
                    selected,
                    indicator,
                    interrupting,
                ),
                ShellEscapeOutcome::Ignored
            );
        }
        assert_eq!(
            resolve_shell_escape(
                "enter",
                true,
                true,
                Route::Chat,
                Some("chat-a"),
                Indicator::Working,
                false,
            ),
            ShellEscapeOutcome::OtherKey
        );
    }

    #[test]
    fn escape_interrupt_is_opt_in() {
        assert_eq!(
            resolve_shell_escape(
                "escape",
                false,
                false,
                Route::Chat,
                Some("chat-a"),
                Indicator::Working,
                false,
            ),
            ShellEscapeOutcome::Ignored
        );
    }

    #[test]
    fn only_visible_sync_steps_block_escape() {
        assert!(!SyncFlow::Idle.has_visible_overlay());
        assert!(!SyncFlow::SwitchOffer { notice_open: false }.has_visible_overlay());
        assert!(SyncFlow::SwitchOffer { notice_open: true }.has_visible_overlay());
        assert!(SyncFlow::Importing { done: 1, total: 3 }.has_visible_overlay());
        assert!(SyncFlow::SignOutConfirm.has_visible_overlay());
    }

    #[test]
    fn right_pane_takeover_control_reverses_direction() {
        assert_eq!(tabs::right_pane_expand_icon(false), icons::EXPAND_ARROWS);
        assert_eq!(tabs::right_pane_expand_icon(true), icons::COLLAPSE_ARROWS);
    }

    #[test]
    fn pane_resize_hitboxes_yield_the_titlebar_chrome() {
        assert_eq!(PANE_RESIZE_HITBOX_TOP, Theme::TITLEBAR_HEIGHT);
        assert_eq!(PANE_RESIZE_HITBOX_HALF_WIDTH * 2.0, 20.0);
        assert_eq!(TERMINAL_RESIZE_HITBOX_HEIGHT, 10.0);
    }

    #[test]
    fn new_session_action_lives_in_the_titlebar_only_when_useful() {
        assert_eq!(titlebar_new_session_alpha(true, true), 1.0);
        assert_eq!(titlebar_new_session_alpha(true, false), 0.0);
        assert_eq!(titlebar_new_session_alpha(false, true), 0.0);
        assert_eq!(titlebar_new_session_alpha(false, false), 0.0);
    }

    #[test]
    fn right_panel_content_keeps_the_larger_width_only_during_transition() {
        assert_eq!(right_panel_content_width(520.0, None, None), 520.0);
        assert_eq!(
            right_panel_content_width(0.0, Some((520.0, 0.0)), None),
            520.0
        );
        assert_eq!(
            right_panel_content_width(760.0, Some((520.0, 760.0)), None),
            760.0
        );
        assert_eq!(
            right_panel_content_width(1064.0, Some((520.0, 1064.0)), Some(760.0)),
            760.0
        );

        let conversation = conversation_width(1320.0, 256.0, 520.0);
        let takeover = conversation_width(1320.0, 256.0, 1064.0);
        assert_eq!(conversation, 544.0);
        assert_eq!(takeover, 0.0);
        assert_eq!(
            stable_panel_content_width(takeover, Some((conversation, takeover))),
            conversation
        );
        assert_eq!(
            stable_panel_content_width(conversation, Some((takeover, conversation))),
            conversation
        );
    }

    #[tokio::test]
    async fn remote_shutdown_waits_for_ipc_release() {
        let dir = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let release = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            drop(listener);
        });

        wait_for_remote_engine_shutdown(port, dir.path(), Duration::from_secs(2))
            .await
            .unwrap();
        release.await.unwrap();
    }

    #[tokio::test]
    async fn signed_out_synced_runtime_stops_and_reboots_local() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("session.json"),
            r#"{"refreshToken":"still-valid","user":{"id":"user_1","email":"u@example.com"},"orgId":"org_1"}"#,
        )
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let boot = EngineBootConfig {
            data_dir: dir.path().to_path_buf(),
            ipc_port: port,
            edge_url: "http://127.0.0.1:1".into(),
            edge_token: None,
            org_id: None,
            workos_client_id: Some("client_test".into()),
            default_harness: skylark_proto::HarnessId::Mock,
        };
        let synced = crate::state::EngineHandle::bootstrap(boot.clone())
            .await
            .expect("saved session opens its synced profile");
        if skylark_proto::LOCAL_ONLY_BUILD {
            assert_eq!(synced.engine_info().workspace_scope, WorkspaceScope::Local);
            assert!(synced.client().call(methods::SIGN_OUT, serde_json::json!({})).await.is_err());
            assert!(dir.path().join("session.json").exists());
            synced.shutdown().await;
            return;
        }
        assert_eq!(synced.engine_info().workspace_scope, WorkspaceScope::Synced);

        synced
            .client()
            .call(methods::SIGN_OUT, serde_json::json!({}))
            .await
            .expect("sign out clears credentials");
        stop_synced_runtime(synced, port, dir.path())
            .await
            .expect("synced runtime drains and releases ownership");

        assert!(!dir.path().join("session.json").exists());
        let local = crate::state::EngineHandle::bootstrap(boot)
            .await
            .expect("same process can continue locally");
        assert_eq!(local.engine_info().workspace_scope, WorkspaceScope::Local);
        local.shutdown().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn remote_shutdown_waits_for_engine_lock_release() {
        let dir = tempfile::tempdir().unwrap();
        let lock = InstanceLock::acquire(dir.path()).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let lock_released = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let released_by_task = lock_released.clone();
        let release = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            drop(lock);
            released_by_task.store(true, std::sync::atomic::Ordering::SeqCst);
        });

        wait_for_remote_engine_shutdown(port, dir.path(), Duration::from_secs(2))
            .await
            .unwrap();
        assert!(lock_released.load(std::sync::atomic::Ordering::SeqCst));
        release.await.unwrap();
    }

    #[tokio::test]
    async fn remote_shutdown_times_out_while_ipc_remains_open() {
        let dir = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let error = wait_for_remote_engine_shutdown(port, dir.path(), Duration::from_millis(100))
            .await
            .unwrap_err();

        assert!(error.contains("did not finish stopping"));
        drop(listener);
    }

    #[test]
    fn account_actions_follow_the_attached_workspace_scope() {
        assert_eq!(
            account_menu_action(Some(WorkspaceScope::Local), SyncFlow::Idle),
            Some(AccountMenuAction::EnableSync)
        );
        assert_eq!(
            account_menu_action(Some(WorkspaceScope::Synced), SyncFlow::Idle),
            Some(AccountMenuAction::SignOut)
        );
        assert_eq!(
            account_menu_action(Some(WorkspaceScope::Development), SyncFlow::Idle),
            None
        );
    }

    #[test]
    fn local_sign_in_offers_the_in_place_switch() {
        let signed_in = AuthState::SignedIn {
            user: skylark_proto::UserProfile {
                id: "user-1".into(),
                email: "user@example.com".into(),
                name: None,
            },
            org_id: Some("org-1".into()),
        };

        assert_eq!(
            sync_flow_after_auth(
                SyncFlow::Enabling,
                Some(WorkspaceScope::Local),
                Some(&signed_in),
            ),
            SyncFlow::SwitchOffer { notice_open: true }
        );
        assert_eq!(
            sync_flow_after_auth(
                SyncFlow::Idle,
                Some(WorkspaceScope::Local),
                Some(&signed_in),
            ),
            SyncFlow::SwitchOffer { notice_open: true },
            "another viewport derives the pending switch from AuthStatus"
        );
        assert_eq!(
            sync_flow_after_auth(
                SyncFlow::SwitchOffer { notice_open: false },
                Some(WorkspaceScope::Local),
                Some(&signed_in),
            ),
            SyncFlow::SwitchOffer { notice_open: false },
            "shared auth updates do not reopen a postponed wizard"
        );
        assert_eq!(
            sync_flow_after_auth(
                SyncFlow::RestartPending { notice_open: false },
                Some(WorkspaceScope::Local),
                Some(&signed_in),
            ),
            SyncFlow::RestartPending { notice_open: false },
            "the quit fallback survives shared auth updates too"
        );
        assert_eq!(
            account_menu_action(
                Some(WorkspaceScope::Local),
                SyncFlow::SwitchOffer { notice_open: false },
            ),
            Some(AccountMenuAction::RestartPending)
        );
        for notice_open in [true, false] {
            assert_eq!(
                sync_flow_after_auth(
                    SyncFlow::SwitchOffer { notice_open },
                    Some(WorkspaceScope::Local),
                    Some(&AuthState::SignedOut),
                ),
                SyncFlow::Idle,
                "revoked credentials cancel the pending switch"
            );
        }
    }

    #[test]
    fn import_summary_errors_are_a_failure_not_a_success() {
        // Clean summary → done with counts.
        let clean = serde_json::json!({
            "kind": "summary", "importedChats": 2, "skippedChats": 1, "errors": []
        });
        assert_eq!(import_summary_outcome(&clean), Ok((2, 1)));

        // Any error means the wizard must NOT say "all set" — partial
        // migrations surface as an explicit failure with the first cause.
        let partial = serde_json::json!({
            "kind": "summary", "importedChats": 1, "skippedChats": 0,
            "errors": ["chat c2: journal copy failed"]
        });
        let message = import_summary_outcome(&partial).expect_err("errors must fail");
        assert!(message.contains("journal copy failed"), "{message}");
        assert!(message.contains("1 imported"), "{message}");

        let many = serde_json::json!({
            "kind": "summary", "importedChats": 0, "skippedChats": 0,
            "errors": ["a", "b", "c"]
        });
        let message = import_summary_outcome(&many).expect_err("errors must fail");
        assert!(message.contains("3 failures"), "{message}");

        // A summary missing the errors field entirely (older engine) is
        // treated as clean rather than failing every import.
        let legacy = serde_json::json!({ "kind": "summary", "importedChats": 4 });
        assert_eq!(import_summary_outcome(&legacy), Ok((4, 0)));
    }

    #[test]
    fn spaces_only_local_work_still_gets_the_import_offer() {
        assert_eq!(local_work_phrase(0, 0), None, "nothing to bring");
        assert_eq!(local_work_phrase(2, 0).as_deref(), Some("the 2 sessions"));
        assert_eq!(
            local_work_phrase(0, 1).as_deref(),
            Some("the 1 project"),
            "a projects-only profile must be offered the import, not a bare switch"
        );
        assert_eq!(
            local_work_phrase(1, 2).as_deref(),
            Some("the 1 session and 2 projects")
        );
    }

    #[test]
    fn dismissed_import_failure_stays_reachable_on_a_synced_runtime() {
        let signed_in = AuthState::SignedIn {
            user: skylark_proto::UserProfile {
                id: "user-1".into(),
                email: "user@example.com".into(),
                name: None,
            },
            org_id: Some("org-1".into()),
        };

        // "Later" postpones the failure notice; it must not evaporate.
        let dismissed = SyncFlow::ImportFailed { notice_open: false };
        assert_eq!(
            sync_flow_after_auth(dismissed, Some(WorkspaceScope::Synced), Some(&signed_in)),
            dismissed,
            "a postponed import failure survives auth/scope updates"
        );

        // …and the account menu on the SYNCED runtime still exposes the
        // re-entry point. This is the whole point: after the switch there is
        // no local runtime left to re-derive an offer from, so this menu row
        // is the only path back to the retry dialog.
        assert_eq!(
            account_menu_action(Some(WorkspaceScope::Synced), dismissed),
            Some(AccountMenuAction::RestartPending),
            "retry must remain reachable after dismissal"
        );
        assert_eq!(
            account_menu_action(
                Some(WorkspaceScope::Synced),
                SyncFlow::ImportFailed { notice_open: true },
            ),
            Some(AccountMenuAction::RestartPending)
        );

        // Resolving the failure restores the normal synced menu.
        assert_eq!(
            account_menu_action(Some(WorkspaceScope::Synced), SyncFlow::Idle),
            Some(AccountMenuAction::SignOut)
        );
    }

    #[test]
    fn switch_lifecycle_survives_the_runtime_replacement_window() {
        let signed_in = AuthState::SignedIn {
            user: skylark_proto::UserProfile {
                id: "user-1".into(),
                email: "user@example.com".into(),
                name: None,
            },
            org_id: Some("org-1".into()),
        };
        for flow in [
            SyncFlow::Switching { import: true },
            SyncFlow::Importing { done: 1, total: 3 },
            SyncFlow::ImportDone {
                imported: 3,
                skipped: 0,
            },
            SyncFlow::ImportFailed { notice_open: true },
            SyncFlow::ImportFailed { notice_open: false },
        ] {
            // Local (before the stop), detached (mid-replacement), and synced
            // (replacement runtime up): the driver owns these states — auth
            // and scope edges must never reset them.
            assert_eq!(
                sync_flow_after_auth(flow, Some(WorkspaceScope::Local), Some(&signed_in)),
                flow
            );
            assert_eq!(sync_flow_after_auth(flow, None, None), flow);
            assert_eq!(
                sync_flow_after_auth(flow, Some(WorkspaceScope::Synced), Some(&signed_in)),
                flow
            );
        }
    }

    #[test]
    fn synced_sign_out_blocks_every_viewport_and_cannot_switch_accounts() {
        let signed_in_as_another_user = AuthState::SignedIn {
            user: skylark_proto::UserProfile {
                id: "user-2".into(),
                email: "other@example.com".into(),
                name: None,
            },
            org_id: Some("org-2".into()),
        };

        assert_eq!(
            sync_flow_after_auth(
                SyncFlow::SigningOut,
                Some(WorkspaceScope::Synced),
                Some(&AuthState::SignedOut),
            ),
            SyncFlow::SignedOutRestartRequired,
            "the viewport that requested sign-out is blocked by AuthStatus"
        );
        assert_eq!(
            sync_flow_after_auth(
                SyncFlow::Idle,
                Some(WorkspaceScope::Synced),
                Some(&AuthState::SignedOut),
            ),
            SyncFlow::SignedOutRestartRequired,
            "another viewport observing the same runtime is also blocked"
        );
        assert_eq!(
            sync_flow_after_auth(
                SyncFlow::SignedOutRestartRequired,
                Some(WorkspaceScope::Synced),
                Some(&signed_in_as_another_user),
            ),
            SyncFlow::SignedOutRestartRequired,
            "new credentials cannot reopen the previous account's store"
        );
    }

    #[test]
    fn titlebar_cluster_matches_skylark_window_controls() {
        // skylark window-controls.tsx: `left: fullscreen ? 12 : 88` — the
        // cluster clears the {14,15} traffic lights, and reclaims the inset
        // when fullscreen hides them.
        assert_eq!(titlebar_cluster_start(false), 88.0);
        assert_eq!(titlebar_cluster_start(true), 12.0);
        assert_eq!(TITLEBAR_CONTROL_GAP, 2.0);
        assert_eq!(TITLEBAR_GROUP_GAP, Theme::SPACE_SM);
        assert_eq!(TITLEBAR_IDENTITY_GAP, Theme::SPACE_MD);
        assert_eq!(CLUSTER_BUTTONS_WIDTH, 82.0);
        assert_eq!(TITLEBAR_ACTION_SLOT_WIDTH, 32.0);
        assert_eq!(TITLEBAR_ACTION_EDGE_INSET, 6.0);
    }

    #[test]
    fn titlebar_spacer_selects_per_platform_and_fullscreen() {
        // macOS, lights visible: spacer fills up to the 88px cluster start.
        assert_eq!(titlebar_spacer_width(true, false, 10.0), 78.0);
        assert_eq!(titlebar_spacer_width(true, false, 12.0), 76.0);
        assert_eq!(titlebar_spacer_width(true, false, 26.0), 62.0);
        // macOS fullscreen: the inset animates away (clamped at zero when the
        // strip's own padding already exceeds the 12px cluster start).
        assert_eq!(titlebar_spacer_width(true, true, 10.0), 2.0);
        assert_eq!(titlebar_spacer_width(true, true, 26.0), 0.0);
        // Linux / Windows: never any inset.
        assert_eq!(titlebar_spacer_width(false, false, 10.0), 0.0);
        assert_eq!(titlebar_spacer_width(false, true, 10.0), 0.0);
        assert_eq!(
            TITLEBAR_CLUSTER_PAD + titlebar_spacer_width(true, false, TITLEBAR_CLUSTER_PAD),
            titlebar_cluster_start(false),
            "the rendered row padding and spacer must land on the declared cluster start"
        );
    }

    #[test]
    fn windows_caption_controls_reserve_titlebar_space() {
        assert_eq!(titlebar_right_padding(true, 0, 16.0), 124.0);
        assert_eq!(titlebar_right_padding(false, 0, 16.0), 16.0);
    }

    #[test]
    fn linux_caption_controls_reserve_titlebar_space() {
        // 24px buttons on the cluster's 2px rhythm.
        assert_eq!(caption_buttons_width(0), 0.0);
        assert_eq!(caption_buttons_width(1), 24.0);
        assert_eq!(caption_buttons_width(3), 76.0);
        // Right-side captions (the Linux default: minimize,maximize,close):
        // content pads past the 10px edge inset + the button row.
        assert_eq!(titlebar_right_padding(false, 3, 16.0), 16.0 + 10.0 + 76.0);
        // GNOME-vanilla ":close" — a single right button.
        assert_eq!(titlebar_right_padding(false, 1, 16.0), 16.0 + 10.0 + 24.0);
        // Left-side captions ("close:…" layouts) shift the app cluster right
        // by the button row + one 2px gap.
        assert_eq!(cluster_buttons_start(false, false, 0), 10.0);
        assert_eq!(cluster_buttons_start(false, false, 1), 10.0 + 24.0 + 2.0);
        assert_eq!(cluster_buttons_start(false, false, 3), 10.0 + 76.0 + 2.0);
        // macOS ignores the Linux caption count entirely.
        assert_eq!(cluster_buttons_start(true, false, 3), 88.0);
    }

    #[test]
    fn cluster_clearance_clears_the_overlay_buttons() {
        // Linux: buttons at 10..92; a 16px-padded header needs 84 more px to
        // put content at 92 + 8 breathing room.
        assert_eq!(cluster_clearance(false, false, 0, 16.0), 84.0);
        assert_eq!(cluster_clearance(false, false, 0, 10.0), 90.0);
        // Linux with a left-side close caption: everything shifts one slot.
        assert_eq!(cluster_clearance(false, false, 1, 16.0), 84.0 + 26.0);
        // macOS: buttons start at the 88px traffic-light cluster start.
        assert_eq!(
            cluster_clearance(true, false, 0, 16.0),
            88.0 + CLUSTER_BUTTONS_WIDTH + 8.0 - 16.0
        );
        // macOS fullscreen: cluster reclaims the inset (starts at 12).
        assert_eq!(
            cluster_clearance(true, true, 0, 16.0),
            12.0 + CLUSTER_BUTTONS_WIDTH + 8.0 - 16.0
        );
    }

    // ---- per-session panel flags (§1.10/1.11 parity: skylark sessionPanels) ----

    #[test]
    fn session_panels_default_closed_per_chat() {
        let panels = SessionPanels::default();
        assert_eq!(panels.get("a"), ChatPanels::default());
        // Everything closed until explicitly opened (user request — the
        // brief default-open popped the pane on every visited session).
        assert!(!panels.get("a").terminal_open);
        assert!(!panels.get("a").changes_open);
        assert_eq!(panels.get("a").right_active, RightSurface::Picker);
        // The new-chat canvas ("" key) is its own session, also closed.
        assert!(!panels.get("").terminal_open);
    }

    #[test]
    fn session_panels_flags_are_chat_scoped() {
        let mut panels = SessionPanels::default();
        // Opening the terminal in chat A opens it ONLY in chat A.
        assert!(panels.toggle_terminal("a"));
        assert!(panels.get("a").terminal_open);
        assert!(!panels.get("b").terminal_open);
        assert!(!panels.get("").terminal_open);
        // Changes pane in B is independent of A's terminal.
        assert!(panels.toggle_changes("b"));
        assert!(panels.get("b").changes_open);
        assert!(!panels.get("b").terminal_open);
        assert!(!panels.get("a").changes_open);
        // Switching back to A restores A's state untouched.
        assert!(panels.get("a").terminal_open);
        // Toggling off round-trips.
        assert!(!panels.toggle_terminal("a"));
        assert!(!panels.get("a").terminal_open);
    }

    #[test]
    fn session_panels_both_flags_coexist_per_chat() {
        let mut panels = SessionPanels::default();
        panels.toggle_terminal("a");
        panels.toggle_changes("a");
        assert_eq!(
            panels.get("a"),
            ChatPanels {
                terminal_open: true,
                changes_open: true,
                ..Default::default()
            }
        );
        assert_eq!(panels.get("b"), ChatPanels::default());
        // The right pane round-trips back closed.
        assert!(!panels.toggle_changes("a"));
        assert!(!panels.get("a").changes_open);
    }

    #[test]
    fn session_panels_update_tracks_right_surfaces() {
        let mut panels = SessionPanels::default();
        panels.update("a", |p| p.right_active = RightSurface::Diff(3));
        assert_eq!(panels.get("a").right_active, RightSurface::Diff(3));
        // Other chats keep the picker default.
        assert_eq!(panels.get("b").right_active, RightSurface::Picker);
        panels.update("a", |p| p.right_active = RightSurface::Terminal(7));
        assert_eq!(panels.get("a").right_active, RightSurface::Terminal(7));
        panels.update("a", |p| p.right_active = RightSurface::Files);
        assert_eq!(panels.get("a").right_active, RightSurface::Files);
    }

    #[test]
    fn files_surface_is_single_instance_per_tab_list() {
        let mut tabs = vec![RightSurface::Terminal(1)];
        assert!(push_unique_right_surface(&mut tabs, RightSurface::Files));
        assert!(!push_unique_right_surface(&mut tabs, RightSurface::Files));
        assert_eq!(tabs, vec![RightSurface::Terminal(1), RightSurface::Files]);
    }

    #[test]
    fn file_editors_are_distinct_surface_tabs_with_stable_titles() {
        let mut tabs = vec![RightSurface::Files];
        assert!(push_unique_right_surface(&mut tabs, RightSurface::File(1)));
        assert!(push_unique_right_surface(&mut tabs, RightSurface::File(2)));
        assert!(!push_unique_right_surface(&mut tabs, RightSurface::File(1)));
        assert_eq!(workspace_file_title("src/árbol.rs"), "árbol.rs");
    }


    // ---- navigation history (titlebar back/forward) ----

    fn chat(id: &str) -> NavEntry {
        NavEntry::Chat(id.to_string())
    }

    #[test]
    fn nav_history_starts_with_nothing_to_walk() {
        let nav = NavHistory::new(chat(""));
        assert!(!nav.can_back());
        assert!(!nav.can_forward());
        assert_eq!(*nav.current(), chat(""));
    }

    #[test]
    fn nav_push_then_back_and_forward() {
        let mut nav = NavHistory::new(chat("a"));
        nav.push(chat("b"));
        nav.push(NavEntry::Settings(SettingsSection::Devices));
        assert!(nav.can_back());
        assert!(!nav.can_forward());

        // Back walks toward the oldest entry without dropping anything.
        assert_eq!(
            nav.back(),
            Some(chat("b")),
            "back lands on the previous route"
        );
        assert_eq!(nav.back(), Some(chat("a")));
        assert!(!nav.can_back());
        assert!(nav.can_forward());
        assert_eq!(nav.back(), None, "past the oldest entry is a no-op");

        // Forward retraces the same path.
        assert_eq!(nav.forward(), Some(chat("b")));
        assert_eq!(
            nav.forward(),
            Some(NavEntry::Settings(SettingsSection::Devices))
        );
        assert!(!nav.can_forward());
        assert_eq!(nav.forward(), None);
    }

    #[test]
    fn nav_push_dedups_the_current_route() {
        let mut nav = NavHistory::new(chat("a"));
        nav.push(chat("a"));
        nav.push(chat("a"));
        assert_eq!(nav.len(), 1, "re-selecting the current route never stacks");
        nav.push(NavEntry::Settings(SettingsSection::Agents));
        nav.push(NavEntry::Settings(SettingsSection::Agents));
        assert_eq!(nav.len(), 2);
    }

    #[test]
    fn nav_push_truncates_the_forward_branch() {
        // a → b → c, back to a, then push d: the b/c branch is gone (browser
        // semantics — skylark's memory history PUSH truncates entries ahead).
        let mut nav = NavHistory::new(chat("a"));
        nav.push(chat("b"));
        nav.push(chat("c"));
        nav.back();
        nav.back();
        assert_eq!(*nav.current(), chat("a"));
        assert!(nav.can_forward());
        nav.push(chat("d"));
        assert!(!nav.can_forward(), "the old branch is unreachable");
        assert_eq!(nav.len(), 2);
        assert_eq!(nav.back(), Some(chat("a")));
        assert_eq!(nav.forward(), Some(chat("d")));
    }

    #[test]
    fn nav_replace_swaps_in_place() {
        // The boot auto-select replaces the untouched canvas entry, so Back
        // stays disabled after landing in the last-used chat.
        let mut nav = NavHistory::new(chat(""));
        nav.replace(chat("boot"));
        assert_eq!(nav.len(), 1);
        assert_eq!(*nav.current(), chat("boot"));
        assert!(!nav.can_back());
    }

    #[test]
    fn nav_settings_sections_are_distinct_entries() {
        let mut nav = NavHistory::new(chat("a"));
        nav.push(NavEntry::Settings(SettingsSection::Devices));
        nav.push(NavEntry::Settings(SettingsSection::Shortcuts));
        assert_eq!(nav.len(), 3, "section changes are navigations");
        assert_eq!(
            nav.back(),
            Some(NavEntry::Settings(SettingsSection::Devices))
        );
        assert_eq!(nav.back(), Some(chat("a")));
    }

    #[test]
    fn sidebar_disclosure_motion_lands_exactly_on_its_target() {
        let mut tween = SidebarDisclosureMotion::new(1, 240.0, 0.0);
        tween.started = std::time::Instant::now() - motion::COLLAPSE.total().mul_f32(2.0);
        assert_eq!(tween.current(), 0.0);
        assert!(!tween.animating());
    }

mod exit_regressions {
    use super::*;
    use gpui::{AppContext, TestAppContext};

    #[gpui::test]
    fn appshot_destinations_retain_last_session_and_use_new_canvas_defaults(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            settings::init(settings::UiSettings::default(), dir.path(), cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
        });
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            Shell::new(
                state,
                EngineBootConfig {
                    data_dir: dir.path().into(),
                    ipc_port: 0,
                    edge_url: "http://127.0.0.1:1".into(),
                    edge_token: None,
                    org_id: None,
                    workos_client_id: None,
                    default_harness: skylark_proto::HarnessId::Mock,
                },
                cx,
            )
        });
        window
            .update(cx, |shell, window, cx| {
                shell.state.update(cx, |state, cx| {
                    state.spaces = ["a", "b"]
                        .into_iter()
                        .map(|id| {
                            serde_json::from_value(serde_json::json!({
                                "id": id, "deviceId": "local", "path": "/tmp", "gitDetected": false,
                                "createdAt": Utc::now(),
                            }))
                            .unwrap()
                        })
                        .collect();
                    state.chats =
                        vec![serde_json::from_value(serde_json::json!({
                    "id": "last", "deviceId": "local", "spaceId": "a", "archived": false,
                    "createdAt": Utc::now(),
                })).unwrap()];
                    state.select_chat(Some("last".into()), cx);
                });
                shell.on_state_changed(&shell.state.clone(), cx);
                shell.settings.space_filter = Some("b".into());
                shell.open_new_session(cx);
                shell.on_state_changed(&shell.state.clone(), cx);
                assert!(shell.active_chat.is_empty());
                shell.settings.appshot_destination =
                    crate::appshots::AppshotDestination::LastSession;
                shell.receive_appshot(crate::appshots::tests::shot(), window, cx);
                assert_eq!(shell.state.read(cx).selected_chat.as_deref(), Some("last"));
                assert_eq!(shell.composer.read(cx).appshots["last"].len(), 1);
                shell.settings.appshot_destination =
                    crate::appshots::AppshotDestination::NewSession;
                shell.receive_appshot(crate::appshots::tests::shot(), window, cx);
                assert!(shell.state.read(cx).selected_chat.is_none());
                assert_eq!(shell.state.read(cx).selected_space.as_deref(), Some("b"));
                assert_eq!(shell.composer.read(cx).appshots[""].len(), 1);
            })
            .unwrap();
    }

    #[gpui::test]
    fn pane_geometry_uses_one_animation_time_per_frame(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            settings::init(settings::UiSettings::default(), dir.path(), cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
        });
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            Shell::new(
                state,
                EngineBootConfig {
                    data_dir: dir.path().into(),
                    ipc_port: 0,
                    edge_url: "http://127.0.0.1:1".into(),
                    edge_token: None,
                    org_id: None,
                    workos_client_id: None,
                    default_harness: skylark_proto::HarnessId::Mock,
                },
                cx,
            )
        });
        window
            .update(cx, |shell, window, cx| {
                let duration = RESIZE.total().mul_f32(motion::speed_scale());
                let started = std::time::Instant::now() - duration.mul_f32(2.);
                let tween = Some(WidthTween {
                    from: 520.,
                    to: 0.,
                    started,
                });
                shell.reduced_motion = false;
                // The frame started halfway through the transition but rendering
                // crossed its deadline. Every region must still use that frame.
                shell.render_time = Some(started + duration.mul_f32(0.5));
                assert!(shell.tween_active(tween));
                assert_eq!(shell.active_tween_endpoints(tween), Some((520., 0.)));
                let width = shell.eval_tween(tween, 0.);
                assert!(width > 0. && width < 520.);
                assert_eq!(shell.eval_tween(tween, 0.), width);
                shell.active_chat = "preview".into();
                shell.viewport_width = 1000.;
                shell.right_tween = tween;
                shell.toggle_right_pane(cx);
                assert_eq!(
                    shell.right_tween.unwrap().from,
                    width,
                    "reversing must not restart from zero"
                );
                let files = cx.new(|cx| {
                    FilesSurface::new(
                        shell.state.clone(),
                        "preview".into(),
                        false,
                        1000,
                        13.0,
                        false,
                        false,
                        cx,
                    )
                });
                let key = shell.panel_key(cx);
                shell.files.insert(key.clone(), files.clone());
                shell
                    .panels
                    .update(&key, |panel| panel.right_active = RightSurface::Files);
                assert!(files.read(cx).test_images_visible());
                shell.toggle_right_pane(cx);
                assert!(!shell.right_pane_open(cx));
                assert!(shell.tween_active(shell.right_tween));
                assert!(
                    !files.read(cx).test_images_visible(),
                    "closing suspends image resources immediately"
                );
                let _ = shell.render_right_pane(window, cx);
                assert!(
                    !files.read(cx).test_images_visible(),
                    "closing animation must not reactivate images"
                );
                shell.settings.sidebar_collapsed = true;
                shell.sidebar_tween = tween;
                shell.toggle_sidebar(cx);
                assert_eq!(shell.sidebar_tween.unwrap().from, width);
                shell.render_time = None;
                assert!(!shell.tween_active(tween));
                assert_eq!(shell.active_tween_endpoints(tween), None);
                assert_eq!(shell.eval_tween(tween, 0.), 0.);
            })
            .unwrap();
    }

    #[gpui::test]
    fn panel_saves_preserve_settings_selected_outside_the_shell(cx: &mut TestAppContext) {
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
            settings::init(settings::UiSettings::default(), dir.path(), cx);
        });
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            Shell::new(
                state,
                EngineBootConfig {
                    data_dir: dir.path().into(),
                    ipc_port: 0,
                    edge_url: "http://127.0.0.1:1".into(),
                    edge_token: None,
                    org_id: None,
                    workos_client_id: None,
                    default_harness: skylark_proto::HarnessId::Mock,
                },
                cx,
            )
        });
        for (index, effect) in settings::NewThreadBackgroundEffect::ALL
            .into_iter()
            .enumerate()
        {
            let open_links_in_skylark = index % 2 == 0;
            let terminal_family = if open_links_in_skylark {
                crate::typography::UiFontFamily::System
            } else {
                crate::typography::UiFontFamily::Geist
            };
            let code_family = if open_links_in_skylark {
                crate::typography::UiFontFamily::Geist
            } else {
                crate::typography::UiFontFamily::System
            };
            let terminal_size = 15.0 + index as f32;
            let code_size = 11.0 + index as f32;
            let transcript_width = 736.0 + 16.0 * index as f32;
            let geometry = Some(settings::WindowGeometry {
                display_uuid: Some(uuid::Uuid::from_u128(7)),
                x: 80.0 + index as f32,
                y: 60.0,
                width: 1100.0,
                height: 750.0,
            });
            window
                .update(cx, |shell, _, cx| {
                    // Selection changes in Appearance, independently of the shell's
                    // cached snapshot. Include a previously queued geometry save.
                    shell.settings.sidebar_width = 280.0;
                    shell.schedule_save(cx);
                    settings::set_new_thread_background_effect(effect, cx);
                    settings::update(settings::SavePolicy::Immediate, cx, |settings| {
                        settings.window_geometry = geometry;
                        settings.open_web_links_in_skylark = open_links_in_skylark;
                        settings.terminal_font_family = terminal_family.clone();
                        settings.terminal_font_size = terminal_size;
                        settings.code_font_family = code_family.clone();
                        settings.code_font_size = code_size;
                        settings.transcript_width = transcript_width;
                    });
                    for step in 0..3 {
                        shell.settings.sidebar_width = 290.0 + step as f32;
                        shell.settings.right_pane_width = 540.0 + step as f32;
                        shell.settings.terminal_height = 300.0 + step as f32;
                        shell.schedule_save(cx);
                        let current = settings::current(cx);
                        assert_eq!(current.window_geometry, geometry);
                        assert_eq!(current.new_thread_background_effect, effect);
                        assert_eq!(current.open_web_links_in_skylark, open_links_in_skylark);
                        assert_eq!(current.terminal_font_family, terminal_family);
                        assert_eq!(current.terminal_font_size, terminal_size);
                        assert_eq!(current.code_font_family, code_family);
                        assert_eq!(current.code_font_size, code_size);
                        assert_eq!(current.transcript_width, transcript_width);
                    }
                    settings::flush(cx);
                    let loaded = settings::UiSettings::load(dir.path());
                    assert_eq!(loaded.window_geometry, geometry);
                    assert_eq!(loaded.new_thread_background_effect, effect);
                    assert_eq!(loaded.open_web_links_in_skylark, open_links_in_skylark);
                    assert_eq!(loaded.terminal_font_family, terminal_family);
                    assert_eq!(loaded.terminal_font_size, terminal_size);
                    assert_eq!(loaded.code_font_family, code_family);
                    assert_eq!(loaded.code_font_size, code_size);
                    assert_eq!(loaded.transcript_width, transcript_width);
                    assert_eq!(loaded.sidebar_width, 292.0);
                    assert_eq!(loaded.right_pane_width, 542.0);
                    assert_eq!(loaded.terminal_height, 302.0);
                })
                .unwrap();
        }
    }

    #[gpui::test]
    fn opening_terminals_focuses_the_terminal_once(cx: &mut TestAppContext) {
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
            settings::init(settings::UiSettings::default(), dir.path(), cx);
        });
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            Shell::new(
                state,
                EngineBootConfig {
                    data_dir: dir.path().into(),
                    ipc_port: 0,
                    edge_url: "http://127.0.0.1:1".into(),
                    edge_token: None,
                    org_id: None,
                    workos_client_id: None,
                    default_harness: skylark_proto::HarnessId::Mock,
                },
                cx,
            )
        });
        let mut drawer_window = None;
        for embedded in [false, false, true] {
            let panel = window
                .update(cx, |shell, window, cx| {
                    shell.open_chat("terminal-session".into(), cx);
                    shell.active_chat = "terminal-session".into();
                    let panel = if embedded {
                        shell.add_terminal_surface(cx);
                        shell.right_terminal.clone().unwrap()
                    } else {
                        shell.toggle_terminal(window, cx);
                        shell.terminal.clone().unwrap()
                    };
                    assert!(!shell.composer.read(cx).focus_pending);
                    panel
                })
                .unwrap();
            let terminal_window = if !embedded && drawer_window.is_some() {
                drawer_window.unwrap()
            } else {
                let handle = cx.update(|cx| {
                    cx.open_window(gpui::WindowOptions::default(), |_, _| panel.clone())
                        .unwrap()
                });
                if !embedded {
                    drawer_window = Some(handle);
                }
                handle
            };
            cx.update_window(terminal_window.into(), |_, window, cx| {
                window.draw(cx).clear();
                assert!(
                    panel.read(cx).focus_handle().is_focused(window),
                    "embedded: {embedded}"
                );
                // Ordinary redraws must not steal focus from another control.
                let other_input = cx.focus_handle();
                window.focus(&other_input, cx);
                window.draw(cx).clear();
                assert!(other_input.is_focused(window));
            })
            .unwrap();
            if !embedded {
                window
                    .update(cx, |shell, window, cx| {
                        shell.toggle_terminal(window, cx);
                        assert!(shell.composer.focus_handle(cx).is_focused(window));
                    })
                    .unwrap();
            }
        }
    }

    #[gpui::test]
    fn session_navigation_focuses_composer_once(cx: &mut TestAppContext) {
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
            settings::init(settings::UiSettings::default(), dir.path(), cx);
        });
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            Shell::new(
                state,
                EngineBootConfig {
                    data_dir: dir.path().into(),
                    ipc_port: 0,
                    edge_url: "http://127.0.0.1:1".into(),
                    edge_token: None,
                    org_id: None,
                    workos_client_id: None,
                    default_harness: skylark_proto::HarnessId::Mock,
                },
                cx,
            )
        });
        // Render the real composer in its own window to exercise mounting
        // without the shell's boot/connection gate hiding the destination.
        let composer_window = cx.update(|cx| {
            let composer = window.read(cx).unwrap().composer.clone();
            cx.open_window(gpui::WindowOptions::default(), |_, _| composer)
                .unwrap()
        });
        for destination in ["initial", "chat", "chat", "new", "new", "back", "settings"] {
            window
                .update(cx, |shell, _, cx| match destination {
                    "chat" => shell.open_chat("existing-session".into(), cx),
                    "new" => shell.open_new_session(cx),
                    "back" => shell.apply_nav(NavEntry::Chat("existing-session".into()), cx),
                    "settings" => {
                        shell.open_settings(SettingsSection::Devices, cx);
                        shell.close_settings(cx);
                    }
                    _ => {}
                })
                .unwrap();
            cx.update_window(composer_window.into(), |composer, window, cx| {
                window.draw(cx).clear();
                assert!(
                    composer
                        .downcast::<Composer>()
                        .unwrap()
                        .focus_handle(cx)
                        .is_focused(window),
                    "{destination}"
                );
                // Subsequent renders after a click-away must not reclaim it.
                window.blur();
                window.draw(cx).clear();
                assert!(window.focused(cx).is_none(), "{destination}");
            })
            .unwrap();
        }
    }

    #[gpui::test]
    fn projectless_new_session_restores_opt_out_and_clears_sidebar_filter(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        crate::settings::composer::ComposerDefaults {
            device: Some("remote".into()),
            no_project: true,
            ..Default::default()
        }
        .save(dir.path())
        .unwrap();
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
            settings::init(settings::UiSettings::default(), dir.path(), cx);
        });
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            Shell::new(
                state,
                EngineBootConfig {
                    data_dir: dir.path().into(),
                    ipc_port: 0,
                    edge_url: "http://127.0.0.1:1".into(),
                    edge_token: None,
                    org_id: None,
                    workos_client_id: None,
                    default_harness: skylark_proto::HarnessId::Mock,
                },
                cx,
            )
        });
        window
            .update(cx, |shell, _, cx| {
                shell.state.update(cx, |state, _| {
                    state.apply_spaces(vec![skylark_proto::Space {
                        id: "repo".into(),
                        device_id: "local".into(),
                        path: "/repo".into(),
                        name: None,
                        git_detected: false,
                        git_checked_at: None,
                        checkout_id: None,
                        created_at: Utc::now(),
                    }]);
                    // Boot opened an existing project session after loading defaults.
                    state.selected_chat = Some("existing-project-chat".into());
                    state.no_project = false;
                });
                shell.settings.space_filter = None;
                shell.open_new_session(cx);
                assert!(shell.state.read(cx).no_project);
                assert!(shell.state.read(cx).selected_space.is_none());
                assert_eq!(
                    shell.state.read(cx).effective_device_id().as_deref(),
                    Some("remote")
                );

                // An explicit filter may select its project, but opting out again
                // must remove that filter before the first send can hide the chat.
                shell.set_space_filter(Some("repo".into()), cx);
                shell
                    .state
                    .update(cx, |state, cx| state.select_space(None, cx));
                shell.on_state_changed(&shell.state.clone(), cx);
                assert!(shell.settings.space_filter.is_none());
                assert!(shell.state.read(cx).no_project);
            })
            .unwrap();
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[gpui::test]
    fn transcript_links_open_new_tabs_and_reject_stale_sessions(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
            settings::init(settings::UiSettings::default(), dir.path(), cx);
        });
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            Shell::new(
                state,
                EngineBootConfig {
                    data_dir: dir.path().into(),
                    ipc_port: 0,
                    edge_url: "http://127.0.0.1:1".into(),
                    edge_token: None,
                    org_id: None,
                    workos_client_id: None,
                    default_harness: skylark_proto::HarnessId::Mock,
                },
                cx,
            )
        });
        let weak = window
            .update(cx, |shell, window, cx| {
                use crate::markdown::render::{
                    LinkAction, LinkActivation, LinkOutcome, LinkTarget,
                };
                shell.active_chat = "first-session".into();
                shell.state.update(cx, |state, _| {
                    state.selected_chat = Some("first-session".into())
                });
                let mut activation = LinkActivation {
                    target: LinkTarget::new("Docs", "https://example.com/docs"),
                    action: LinkAction::Primary,
                    source_session: Some("first-session".into()),
                };
                assert!(!shell.right_pane_open(cx));
                assert_eq!(
                    shell.activate_session_link(&activation, window, cx),
                    LinkOutcome::Internal
                );
                assert!(shell.right_pane_open(cx));
                let first = shell.browser_seq;
                assert_eq!(
                    shell.browsers[&first].read(cx).page.url.as_deref(),
                    Some("https://example.com/docs")
                );
                assert_eq!(
                    shell.resolved_right_active(cx),
                    RightSurface::Browser(first)
                );
                shell.activate_session_link(&activation, window, cx);
                assert_eq!(shell.browsers.len(), 2);
                settings::update(settings::SavePolicy::Immediate, cx, |settings| {
                    settings.open_web_links_in_skylark = false;
                });
                assert_eq!(
                    shell.activate_session_link(&activation, window, cx),
                    LinkOutcome::External("https://example.com/docs".into())
                );
                assert_eq!(shell.browsers.len(), 2);
                activation.action = LinkAction::Internal;
                assert_eq!(
                    shell.activate_session_link(&activation, window, cx),
                    LinkOutcome::Internal,
                    "the explicit internal action ignores the default preference"
                );
                assert_eq!(shell.browsers.len(), 3);
                activation.action = LinkAction::External;
                assert_eq!(
                    shell.activate_session_link(&activation, window, cx),
                    LinkOutcome::External("https://example.com/docs".into())
                );
                assert_eq!(shell.browsers.len(), 3);
                activation.source_session = Some("other-session".into());
                assert_eq!(
                    shell.activate_session_link(&activation, window, cx),
                    LinkOutcome::Rejected
                );
                activation.source_session = Some("first-session".into());
                shell.state.update(cx, |state, _| {
                    state.selected_chat = Some("switch-in-progress".into())
                });
                assert_eq!(
                    shell.activate_session_link(&activation, window, cx),
                    LinkOutcome::Rejected
                );
                assert_eq!(shell.browsers.len(), 3);
                shell.state.update(cx, |state, _| {
                    state.selected_chat = Some("first-session".into())
                });
                shell.add_subagent_surface(
                    "first-session".into(),
                    "child-doc".into(),
                    "Child".into(),
                    true,
                    cx,
                );
                let child = shell
                    .subagent_tabs
                    .values()
                    .next()
                    .unwrap()
                    .transcript
                    .clone();
                let ui = child.read(cx).link_ui().unwrap();
                assert_eq!(ui.source_session.as_deref(), Some("first-session"));
                activation.action = LinkAction::Internal;
                activation.source_session = ui.source_session;
                assert_eq!(
                    shell.activate_session_link(&activation, window, cx),
                    LinkOutcome::Internal
                );
                assert_eq!(shell.browsers.len(), 4);
                let weak = shell.browsers[&first].downgrade();
                shell.close_right_surface(RightSurface::Browser(first), window, cx);
                weak
            })
            .unwrap();
        cx.run_until_parked();
        assert!(weak.upgrade().is_none());
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[gpui::test]
    fn markdown_preview_events_open_browser_from_tree_and_file_tabs(cx: &mut TestAppContext) {
        use crate::markdown::render::{LinkAction, LinkActivation, LinkTarget};
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
            settings::init(settings::UiSettings::default(), dir.path(), cx);
        });
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            Shell::new(
                state,
                EngineBootConfig {
                    data_dir: dir.path().into(),
                    ipc_port: 0,
                    edge_url: "http://127.0.0.1:1".into(),
                    edge_token: None,
                    org_id: None,
                    workos_client_id: None,
                    default_harness: skylark_proto::HarnessId::Mock,
                },
                cx,
            )
        });
        let surfaces = window
            .update(cx, |shell, window, cx| {
                shell.active_chat = "owner".into();
                shell
                    .state
                    .update(cx, |state, _| state.selected_chat = Some("owner".into()));
                shell.add_files_surface(window, cx);
                shell.add_file_surface("README.md".into(), window, cx);
                [
                    shell.files[&shell.panel_key(cx)].clone(),
                    shell.file_surfaces[&shell.file_surface_seq].clone(),
                ]
            })
            .unwrap();
        let mut last_external_url = None;
        for (index, surface) in surfaces.iter().enumerate() {
            let mut activation = LinkActivation {
                target: LinkTarget::new("Docs", &format!("https://example.com/preview/{index}")),
                action: LinkAction::Primary,
                source_session: Some("owner".into()),
            };
            surface.update(cx, |_, cx| {
                cx.emit(FilesEvent::OpenWebLink(activation.clone()))
            });
            cx.run_until_parked();
            assert_eq!(cx.opened_url(), last_external_url);
            window
                .update(cx, |shell, _, cx| {
                    assert!(shell.right_pane_open(cx));
                    assert_eq!(shell.browsers.len(), index + 1);
                    assert_eq!(
                        shell.resolved_right_active(cx),
                        RightSurface::Browser(shell.browser_seq)
                    );
                    assert_eq!(
                        shell.browsers[&shell.browser_seq]
                            .read(cx)
                            .page
                            .url
                            .as_deref(),
                        Some(activation.target.original.as_str())
                    );
                })
                .unwrap();
            activation.action = LinkAction::External;
            surface.update(cx, |_, cx| {
                cx.emit(FilesEvent::OpenWebLink(activation.clone()))
            });
            cx.run_until_parked();
            assert_eq!(
                cx.opened_url().as_deref(),
                Some(activation.target.original.as_str())
            );
            last_external_url = Some(activation.target.original.clone());
            activation.target = LinkTarget::new("Stale", "https://example.com/stale");
            activation.source_session = Some("stale-owner".into());
            for action in [LinkAction::Internal, LinkAction::External] {
                activation.action = action;
                surface.update(cx, |_, cx| {
                    cx.emit(FilesEvent::OpenWebLink(activation.clone()))
                });
                cx.run_until_parked();
                assert_eq!(cx.opened_url(), last_external_url);
            }
            window
                .update(cx, |shell, _, _| {
                    assert_eq!(shell.browsers.len(), index + 1)
                })
                .unwrap();
        }
    }

    #[gpui::test]
    fn browser_tabs_keep_session_ownership_and_release_on_close(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
        });
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            Shell::new(
                state,
                EngineBootConfig {
                    data_dir: dir.path().into(),
                    ipc_port: 0,
                    edge_url: "http://127.0.0.1:1".into(),
                    edge_token: None,
                    org_id: None,
                    workos_client_id: None,
                    default_harness: skylark_proto::HarnessId::Mock,
                },
                cx,
            )
        });
        let weak = window
            .update(cx, |shell, window, cx| {
                // A fresh-session canvas never accumulates ownerless tabs.
                shell.add_browser_surface(None, window, cx);
                assert!(shell.browsers.is_empty());
                shell.active_chat = "first-session".into();
                shell.add_browser_surface(None, window, cx);
                let first = shell.browser_seq;
                let weak = shell.browsers[&first].downgrade();
                shell.add_browser_surface(None, window, cx);
                let second = shell.browser_seq;
                assert_ne!(first, second);
                assert_eq!(
                    shell.resolved_right_active(cx),
                    RightSurface::Browser(second)
                );
                shell.active_chat = "second-session".into();
                assert!(shell.right_surface_rows(cx).is_empty());
                shell.add_browser_surface(None, window, cx);
                let other = shell.browser_seq;
                shell.active_chat = "first-session".into();
                assert_eq!(shell.right_surface_rows(cx).len(), 2);
                assert_eq!(
                    shell.resolved_right_active(cx),
                    RightSurface::Browser(second)
                );
                // Closing a background tab preserves the selected address input.
                let focus = window.focused(cx);
                shell.close_right_surface(RightSurface::Browser(first), window, cx);
                assert_eq!(window.focused(cx), focus);
                assert_eq!(
                    shell.resolved_right_active(cx),
                    RightSurface::Browser(second)
                );
                shell.close_right_surface(RightSurface::Browser(second), window, cx);
                assert_eq!(shell.resolved_right_active(cx), RightSurface::Picker);
                shell.active_chat = "second-session".into();
                assert_eq!(
                    shell.resolved_right_active(cx),
                    RightSurface::Browser(other)
                );
                shell.close_right_surface(RightSurface::Browser(other), window, cx);
                assert!(shell.browsers.is_empty());
                assert!(shell.browser_subs.is_empty());
                weak
            })
            .unwrap();
        cx.run_until_parked();
        assert!(
            weak.upgrade().is_none(),
            "closed browser retained by callbacks"
        );
    }

    #[gpui::test]
    fn cmd_w_closes_the_active_right_pane_surface_before_the_window(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
        });
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            Shell::new(
                state,
                EngineBootConfig {
                    data_dir: dir.path().into(),
                    ipc_port: 0,
                    edge_url: "http://127.0.0.1:1".into(),
                    edge_token: None,
                    org_id: None,
                    workos_client_id: None,
                    default_harness: skylark_proto::HarnessId::Mock,
                },
                cx,
            )
        });
        window
            .update(cx, |shell, window, cx| {
                shell.active_chat = "session".into();
                shell.toggle_right_pane(cx);
                assert!(shell.right_pane_open(cx));

                shell.add_browser_surface(None, window, cx);
                let first = shell.browser_seq;
                shell.add_browser_surface(None, window, cx);
                let second = shell.browser_seq;
                assert_eq!(
                    shell.resolved_right_active(cx),
                    RightSurface::Browser(second)
                );

                // ⌘W closes the active tab, not the window.
                assert!(shell.close_active_surface(window, cx));
                assert_eq!(
                    shell.resolved_right_active(cx),
                    RightSurface::Browser(first)
                );

                // …and again for the last tab.
                assert!(shell.close_active_surface(window, cx));
                assert_eq!(shell.resolved_right_active(cx), RightSurface::Picker);

                // An open pane with nothing left to close falls through to the
                // window-close rung instead of being consumed.
                assert!(!shell.close_active_surface(window, cx));

                // So does an already-closed pane.
                shell.toggle_right_pane(cx);
                assert!(!shell.right_pane_open(cx));
                assert!(!shell.close_active_surface(window, cx));
            })
            .unwrap();
    }

    #[gpui::test]
    fn lifecycle_actions_keep_pending_and_failed_file_saves_alive(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
        });
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            Shell::new(
                state,
                EngineBootConfig {
                    data_dir: dir.path().into(),
                    ipc_port: 0,
                    edge_url: "http://127.0.0.1:1".into(),
                    edge_token: None,
                    org_id: None,
                    workos_client_id: None,
                    default_harness: skylark_proto::HarnessId::Mock,
                },
                cx,
            )
        });
        for failed in [false, true] {
            window
                .update(cx, |shell, window, cx| {
                    window.activate_window();
                    let state = shell.state.clone();
                    let files = cx.new(|cx| {
                        let mut files = FilesSurface::new(
                            state,
                            "test".into(),
                            false,
                            1000,
                            13.0,
                            false,
                            false,
                            cx,
                        );
                        files.seed_pending_exit_test_document(failed);
                        files
                    });
                    shell.files.insert("test".into(), files);
                })
                .unwrap();
            cx.update(|cx| cx.dispatch_action(&crate::app_menus::Quit));
            cx.run_until_parked();
            window
                .update(cx, |shell, _, cx| {
                    assert!(matches!(shell.pending_exit, Some(PendingExit::Quit)));
                    assert!(!shell.all_file_edits_flushed(cx));
                    shell.cancel_file_close(RightSurface::Files, cx);
                    assert!(shell.pending_exit.is_none());
                })
                .unwrap();
            cx.update(|cx| cx.dispatch_action(&crate::app_menus::CloseWindow));
            cx.run_until_parked();
            window
                .update(cx, |shell, _, cx| {
                    assert!(matches!(shell.pending_exit, Some(PendingExit::CloseWindow)));
                    shell.quit_for_runtime_change(cx);
                    assert!(matches!(
                        shell.pending_exit,
                        Some(PendingExit::RuntimeChange)
                    ));
                    assert!(shell.runtime_change_task.is_none());
                    shell.apply_staged_update(PathBuf::from("must-not-install"), cx);
                    assert!(matches!(
                        shell.pending_exit,
                        Some(PendingExit::InstallUpdate(_))
                    ));
                    assert!(matches!(shell.update_flow, UpdateFlow::Idle));
                    shell.cancel_file_close(RightSurface::Files, cx);
                    assert!(shell.pending_exit.is_none());
                })
                .unwrap();
        }
    }
}
