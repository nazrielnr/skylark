//! Unit and regression tests for composer input, attachments, and keybindings.

use super::*;

    fn composer_focus_window(
        cx: &mut gpui::TestAppContext,
    ) -> (tempfile::TempDir, gpui::WindowHandle<Composer>) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::dark());
            crate::app_menus::init(cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
            crate::settings::init(crate::settings::UiSettings::default(), dir.path(), cx);
        });
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            Composer::new(state, cx)
        });
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        (dir, window)
    }

    #[gpui::test]
    fn dock_morph_restores_skinny_height_with_a_continuous_editor_origin(
        cx: &mut gpui::TestAppContext,
    ) {
        let (_dir, handle) = composer_focus_window(cx);
        let input = handle
            .read_with(cx, |composer, _| composer.input.clone())
            .unwrap();
        for docked in [true, false] {
            let amounts = if docked {
                [0.0, 0.2, 0.6, 0.98, 1.0]
            } else {
                [1.0, 0.98, 0.6, 0.2, 0.0]
            };
            for amount in amounts {
                handle
                    .update(cx, |composer, _, cx| {
                        composer.state.update(cx, |state, _| {
                            state.selected_chat = docked.then(|| "chat".into());
                        });
                        composer.on_state_changed(cx);
                        composer
                            .input
                            .update(cx, |input, cx| input.set_text("Hi", cx));
                        composer.expanded_mode = false;
                        let mut frame = crate::composer_dock::DockFrame::settled(docked);
                        frame.amount = amount;
                        frame.active = amount != if docked { 1.0 } else { 0.0 };
                        composer.set_dock_frame(frame, cx);
                    })
                    .unwrap();
                cx.update_window(handle.into(), |_, window, cx| {
                    window.draw(cx).clear();
                })
                .unwrap();
                handle.read_with(cx, |composer, cx| {
                    assert_eq!(composer.input, input);
                    let surface = composer.surface_bounds.get().unwrap();
                    let origin = input.read(cx).last_bounds.unwrap().origin;
                    assert!((f32::from(origin.y - surface.top()) - (17.0 - 4.0 * amount)).abs() <= 1.0,
                        "editor jumped: docked={docked}, amount={amount}, origin={origin:?}, surface={surface:?}");
                    let model = composer.model_bounds.get().unwrap();
                    let left = surface.left() + px(1.0 + 12.0 + 28.0 + ACTION_UTILITY_GAP);
                    let travel = surface.size.width - px(2.0 + 12.0 + 28.0 + ACTION_UTILITY_GAP
                        + ACTION_PRIMARY_GAP + 28.0 + motion::lerp(12.0, 8.0, amount)) - model.size.width;
                    let (side, _, drift) = model_handoff(amount);
                    let expected_x = left + travel * side + px(drift);
                    assert!((f32::from(model.left() - expected_x)).abs() <= 1.0,
                        "model jumped: docked={docked}, amount={amount}, actual={model:?}, expected={expected_x:?}");
                    let expected = if docked { COMPACT_TOTAL_HEIGHT } else { COMPOSER_MIN_HEIGHT };
                    assert!((composer.last_rendered_height + composer.dock_clearance_correction - expected).abs() < 0.1);
                    assert!((composer.last_rendered_height - motion::lerp(COMPOSER_MIN_HEIGHT, COMPACT_TOTAL_HEIGHT, amount)).abs() < 0.1);
                }).unwrap();
            }
        }
    }

    #[test]
    fn model_handoff_hides_relocation_and_keeps_visible_motion_local() {
        assert_eq!(model_handoff(0.0), (0.0, 1.0, 0.0));
        assert_eq!(model_handoff(1.0), (1.0, 1.0, -0.0));
        for amount in [0.44, 0.49, 0.50, 0.51, 0.56] {
            assert!(model_handoff(amount).1 < 0.0001);
        }
        for step in 0..=100 {
            let (side, opacity, drift) = model_handoff(step as f32 / 100.0);
            assert!((0.0..=1.0).contains(&opacity));
            assert!(drift.abs() <= 6.0);
            assert!(side == 0.0 || side == 1.0);
        }
    }

    #[gpui::test]
    fn composer_padding_and_file_prompt_restore_focus(cx: &mut gpui::TestAppContext) {
        let (dir, handle) = composer_focus_window(cx);
        let image_path = dir.path().join("attachment.png");
        let png = base64::Engine::decode(&base64::engine::general_purpose::STANDARD,
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aF1cAAAAASUVORK5CYII=").unwrap();
        std::fs::write(&image_path, png).unwrap();
        let input = handle
            .read_with(cx, |composer, _| composer.input.clone())
            .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            window.blur();
            window.draw(cx).clear();
            let bounds = input.read(cx).last_bounds.unwrap();
            // Click padding immediately left of the actual editor.
            let position = point(bounds.left() - px(4.0), bounds.center().y);
            window.dispatch_event(
                gpui::PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position,
                    click_count: 1,
                    ..Default::default()
                }),
                cx,
            );
            assert!(input.read(cx).focus_handle.is_focused(window));
            window.dispatch_event(
                gpui::PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Left,
                    position,
                    ..Default::default()
                }),
                cx,
            );
        })
        .unwrap();
        for accepted in [false, true] {
            handle
                .update(cx, |composer, window, cx| {
                    composer
                        .input
                        .update(cx, |input, cx| input.set_text("Keep this draft", cx));
                    window.blur();
                    composer.open_file_picker(cx);
                })
                .unwrap();
            assert!(cx.did_prompt_for_paths());
            let path = image_path.clone();
            cx.simulate_path_prompt_response(move |_| accepted.then(|| vec![path]));
            cx.run_until_parked();
            cx.update_window(handle.into(), |_, window, cx| {
                window.draw(cx).clear();
                assert!(input.read(cx).focus_handle.is_focused(window));
                assert_eq!(input.read(cx).text(), "Keep this draft");
            })
            .unwrap();
            assert_eq!(
                handle
                    .read_with(cx, |composer, _| composer.staged().len())
                    .unwrap(),
                usize::from(accepted)
            );
        }
        // The same staging path handles external file drops.
        handle
            .update(cx, |composer, window, cx| {
                window.blur();
                composer.add_paths(vec![image_path], cx);
            })
            .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            window.draw(cx).clear();
            assert!(input.read(cx).focus_handle.is_focused(window));
        })
        .unwrap();
    }

    #[gpui::test]
    fn composer_picker_escape_restores_focus_but_click_away_does_not(
        cx: &mut gpui::TestAppContext,
    ) {
        let (_dir, handle) = composer_focus_window(cx);
        let input = handle
            .read_with(cx, |composer, _| composer.input.clone())
            .unwrap();
        for escape in [true, false] {
            handle
                .update(cx, |composer, window, cx| {
                    composer
                        .pickers
                        .update(cx, |pickers, cx| pickers.open_model_menu(window, cx));
                })
                .unwrap();
            cx.update_window(handle.into(), |_, window, cx| {
                window.draw(cx).clear();
                assert!(!input.read(cx).focus_handle.is_focused(window));
            })
            .unwrap();
            if escape {
                cx.simulate_keystrokes(handle.into(), "escape");
            } else {
                cx.update_window(handle.into(), |_, window, cx| {
                    window.dispatch_event(
                        gpui::PlatformInput::MouseDown(MouseDownEvent {
                            button: MouseButton::Left,
                            position: point(px(5.0), window.viewport_size().height - px(1.0)),
                            click_count: 1,
                            ..Default::default()
                        }),
                        cx,
                    );
                })
                .unwrap();
            }
            cx.update_window(handle.into(), |_, window, cx| {
                window.draw(cx).clear();
                assert_eq!(input.read(cx).focus_handle.is_focused(window), escape);
            })
            .unwrap();
        }
    }

    #[gpui::test]
    fn inputs_release_focus_on_click_away(cx: &mut gpui::TestAppContext) {
        struct Inputs(Vec<Entity<ComposerInput>>);
        impl Render for Inputs {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().size_full().flex().flex_col().children(
                    self.0
                        .iter()
                        .map(|input| div().w(px(200.0)).child(input.clone())),
                )
            }
        }
        cx.update(|cx| cx.set_global(Theme::dark()));
        let host = cx.add_window(|_, cx| {
            Inputs(vec![
                cx.new(|cx| ComposerInput::new("Composer", cx)),
                cx.new(|cx| ComposerInput::new("URL", cx).with_single_line()),
            ])
        });
        cx.run_until_parked();
        cx.update_window(host.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        let inputs = host.read_with(cx, |host, _| host.0.clone()).unwrap();
        // Visit both fields and return to the first: click-away capture must
        // never clear focus acquired by the clicked input during bubbling.
        for index in [0, 1, 0] {
            let position =
                inputs[index].read_with(cx, |input, _| input.last_bounds.unwrap().center());
            cx.update_window(host.into(), |_, window, cx| {
                window.dispatch_event(
                    gpui::PlatformInput::MouseDown(MouseDownEvent {
                        button: MouseButton::Left,
                        position,
                        click_count: 1,
                        ..Default::default()
                    }),
                    cx,
                );
                assert!(inputs[index].read(cx).focus_handle.is_focused(window));
                window.dispatch_event(
                    gpui::PlatformInput::MouseUp(MouseUpEvent {
                        button: MouseButton::Left,
                        position,
                        ..Default::default()
                    }),
                    cx,
                );
            })
            .unwrap();
        }
        cx.update_window(host.into(), |_, window, cx| {
            window.dispatch_event(
                gpui::PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position: point(px(400.0), px(300.0)),
                    click_count: 1,
                    ..Default::default()
                }),
                cx,
            );
            assert!(window.focused(cx).is_none());
        })
        .unwrap();
    }

    #[gpui::test]
    fn projectless_composer_allows_send_and_enter_submission(cx: &mut gpui::TestAppContext) {
        let state = cx.new(|_| AppState::new());
        let composer = cx.new(|cx| Composer::new(state.clone(), cx));
        // A device with no projects is a valid home-directory target too.
        composer.update(cx, |composer, cx| assert!(!composer.send_blocked(cx)));
        state.update(cx, |state, cx| state.select_space(None, cx));
        composer.update(cx, |composer, cx| {
            composer.input.update(cx, |input, cx| {
                input.set_text("Hello without a project", cx);
            });
            assert_eq!(composer.button_mode(cx), SendButtonMode::Send);
            assert!(
                !composer.send_blocked(cx),
                "The Send button must be enabled without a project"
            );
            composer.on_submit(cx);
            // With no engine attached, reaching the normal send error proves
            // Enter dispatched instead of silently stopping at the UI gate.
            assert_eq!(composer.failure.as_deref(), Some("Engine not connected"));
            composer.queue_edit_finishing = true;
            assert!(
                composer.send_blocked(cx),
                "Pending edits must still block submission"
            );
        });
    }

    /// Issue #406: Enter submits — it must never stop a run. Stop mode only
    /// exists on a live run with an EMPTY composer, so a habitual
    /// double-Enter after sending interrupted the just-dispatched prompt
    /// and the agent ate it silently. Keyboard stop is the Esc setting's
    /// job; Enter on an empty composer is a no-op.
    #[gpui::test]
    fn enter_on_empty_composer_during_a_live_run_never_interrupts(cx: &mut gpui::TestAppContext) {
        // RpcClient::new spawns its reader on tokio — give the test a reactor.
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _guard = runtime.enter();
        // The client's write end: any RPC Enter dispatches lands here.
        let (out, mut server_in) = tokio::sync::mpsc::channel::<String>(16);
        let (_server_out, inbound) = tokio::sync::mpsc::channel::<String>(16);
        let state = cx.new(|_| AppState::new());
        state.update(cx, |state, _| {
            state.set_test_engine(crate::state::EngineHandle::from_test_client(
                zeron_rpc::RpcClient::new(out, inbound),
            ));
            state.selected_chat = Some("c".into());
            // A send in flight reads as Working — the double-Enter window.
            state.begin_pending_send("c", "m1", chrono::Utc::now());
        });
        let composer = cx.new(|cx| Composer::new(state, cx));
        composer.update(cx, |composer, cx| {
            assert_eq!(composer.button_mode(cx), SendButtonMode::Stop);
            composer.on_submit(cx);
            // interrupt_chat marks the chat before its RPC even flies.
            assert!(!composer.interrupting.contains("c"));
        });
        // If Enter had dispatched an interrupt, the spawned call would have
        // written its frame into the channel by the time the executor parks.
        cx.run_until_parked();
        assert!(server_in.try_recv().is_err());
    }

    /// The press intent is judged by eye everywhere except here: that a
    /// multi-click leaves the drag disarmed is invisible until a selection
    /// collapses under the pointer.
    #[test]
    fn a_press_of_two_or_more_clicks_takes_the_whole_field_and_leaves_the_drag_disarmed() {
        assert_eq!(press_intent(1, false), PressIntent::PlaceCaret);
        assert_eq!(press_intent(1, true), PressIntent::ExtendSelection);
        assert_eq!(press_intent(2, false), PressIntent::SelectAll);
        // A triple click keeps the whole field, so holding the button down
        // through a third click does not change what is selected.
        assert_eq!(press_intent(3, false), PressIntent::SelectAll);
        // The whole field wins over the shift modifier: shift has nothing
        // left to extend once everything is selected.
        assert_eq!(press_intent(2, true), PressIntent::SelectAll);
        // Only a caret press arms the drag. A select-all that armed it would
        // collapse to a drag selection on the next mouse move.
        assert!(press_intent(1, false).arms_drag());
        assert!(press_intent(1, true).arms_drag());
        assert!(!press_intent(2, false).arms_drag());
    }

    #[test]
    fn message_enter_bindings_cover_both_platform_modifiers() {
        assert_eq!(
            message_enter_bindings(ComposerSendBehavior::Enter, "cmd-enter"),
            vec![
                MessageEnterBinding {
                    keystroke: "enter".into(),
                    action: MessageEnterBindingAction::Submit,
                },
                MessageEnterBinding {
                    keystroke: "cmd-enter".into(),
                    action: MessageEnterBindingAction::ModifiedSubmit,
                },
            ]
        );
        assert_eq!(
            message_enter_bindings(ComposerSendBehavior::ModEnter, "cmd-enter"),
            vec![
                MessageEnterBinding {
                    keystroke: "enter".into(),
                    action: MessageEnterBindingAction::NewlineOrAccept,
                },
                MessageEnterBinding {
                    keystroke: "cmd-enter".into(),
                    action: MessageEnterBindingAction::ModifiedSubmit,
                },
            ]
        );
        assert_eq!(
            message_enter_bindings(ComposerSendBehavior::ModEnter, "ctrl-enter"),
            vec![
                MessageEnterBinding {
                    keystroke: "enter".into(),
                    action: MessageEnterBindingAction::NewlineOrAccept,
                },
                MessageEnterBinding {
                    keystroke: "ctrl-enter".into(),
                    action: MessageEnterBindingAction::ModifiedSubmit,
                },
            ]
        );
    }

    #[test]
    fn message_enter_never_adds_extra_modifier_bindings() {
        let bindings = message_enter_bindings(ComposerSendBehavior::ModEnter, "cmd-enter");
        assert!(!bindings.iter().any(|binding| {
            matches!(
                binding.keystroke.as_str(),
                "ctrl-enter" | "shift-cmd-enter" | "alt-cmd-enter"
            )
        }));
    }

    #[test]
    fn enter_accepts_a_completion_before_submit_or_newline() {
        assert_eq!(
            enter_outcome(true, EnterOutcome::Submit),
            EnterOutcome::AcceptCompletion
        );
        assert_eq!(
            enter_outcome(true, EnterOutcome::Newline),
            EnterOutcome::AcceptCompletion
        );
        assert_eq!(
            enter_outcome(false, EnterOutcome::Submit),
            EnterOutcome::Submit
        );
        assert_eq!(
            enter_outcome(false, EnterOutcome::Newline),
            EnterOutcome::Newline
        );
    }

    #[test]
    fn wizard_borrows_the_generic_enter_context_only_while_active() {
        assert_eq!(message_input_context(false), MESSAGE_COMPOSER_CONTEXT);
        assert_eq!(message_input_context(true), GENERIC_COMPOSER_CONTEXT);
    }

    #[test]
    fn stable_outer_width_only_schedules_reflow_on_real_changes() {
        assert!(composer_width_changed(None, 400.0));
        assert!(!composer_width_changed(Some(400.0), 400.0));
        assert!(!composer_width_changed(Some(400.0), 400.5));
        assert!(composer_width_changed(Some(400.0), 400.51));
    }

    fn tooltip_target(range: Range<usize>, path: &str) -> MentionTooltipTarget {
        MentionTooltipTarget {
            range,
            path: path.into(),
        }
    }

    #[test]
    fn mention_tooltip_wait_survives_pointer_jitter_and_promotes_once() {
        let target = tooltip_target(3..20, "src/composer.rs");
        let waiting = MentionTooltipPhase::Waiting {
            target: target.clone(),
            generation: 1,
        };
        let restarted = mention_tooltip_reduce(waiting.clone(), Some(target.clone()), false, 2);
        assert_eq!(restarted, waiting);
        assert!(matches!(
            restarted,
            MentionTooltipPhase::Waiting { generation: 1, .. }
        ));
        assert_eq!(
            mention_tooltip_promote(restarted.clone(), 2, true),
            restarted,
            "a stale timer must not reveal the tooltip"
        );
        let visible = mention_tooltip_promote(restarted, 1, true);
        assert!(matches!(
            visible,
            MentionTooltipPhase::Visible { generation: 1, .. }
        ));
        assert_eq!(
            mention_tooltip_reduce(visible.clone(), Some(target), false, 3),
            visible,
            "one visible activation keeps its presentation generation stable"
        );
    }

    #[test]
    fn mention_tooltip_changes_target_and_cancels_disappeared_target() {
        let first = tooltip_target(0..10, "src/a.rs");
        let second = tooltip_target(20..30, "src/a.rs");
        let visible = MentionTooltipPhase::Visible {
            target: first,
            generation: 4,
        };
        assert!(matches!(
            mention_tooltip_reduce(visible, Some(second), false, 5),
            MentionTooltipPhase::Waiting { generation: 5, .. }
        ));
        assert_eq!(
            mention_tooltip_promote(
                MentionTooltipPhase::Waiting {
                    target: tooltip_target(20..30, "src/a.rs"),
                    generation: 5,
                },
                5,
                false,
            ),
            MentionTooltipPhase::Hidden
        );
    }

    #[test]
    fn mention_tooltip_stays_visible_over_chip_or_popup_only() {
        assert!(mention_tooltip_contains(true, false));
        assert!(mention_tooltip_contains(false, true));
        assert!(!mention_tooltip_contains(false, false));
    }

    #[test]
    fn mention_wash_moves_wholly_to_the_next_visual_row_at_a_wrap() {
        assert_eq!(
            display_row_segments(12..24, [12, 40]),
            vec![(1, 12, 12..24)]
        );
        assert_eq!(
            display_row_segments(8..24, [12, 40]),
            vec![(0, 0, 8..12), (1, 12, 12..24)]
        );
    }

    #[test]
    fn mention_token_requires_a_token_boundary_and_tracks_full_token() {
        assert_eq!(
            mention_token("Fix @src/com", 12),
            Some(MentionToken {
                range: 4..12,
                query: "src/com".into(),
            })
        );
        assert!(mention_token("mail@example.com", 16).is_none());
        assert!(mention_token("word@file", 9).is_none());
        assert!(mention_token("path/@file", 10).is_none());
        assert_eq!(
            mention_token("See (@lib", 9).map(|token| token.range),
            Some(5..9)
        );
    }

    #[test]
    fn slash_token_only_opens_the_prompt() {
        assert_eq!(
            slash_token("/comp", 5),
            Some(MentionToken {
                range: 0..5,
                query: "comp".into(),
            })
        );
        // Token range spans the whole command word even mid-cursor.
        assert_eq!(
            slash_token("/compact now", 3),
            Some(MentionToken {
                range: 0..8,
                query: "co".into(),
            })
        );
        // Not at offset 0 → prose, not a command.
        assert!(slash_token("run /compact", 12).is_none());
        // Cursor past the command word (typing the argument) → closed.
        assert!(slash_token("/goal ship it", 10).is_none());
        // A typed absolute path is not a command.
        assert!(slash_token("/usr/bin", 8).is_none());
        // Bare "/" with cursor at 0 → closed; cursor after it → open-all.
        assert!(slash_token("/", 0).is_none());
        assert_eq!(slash_token("/", 1).map(|t| t.query), Some(String::new()));
    }

    #[test]
    fn dismissed_mentions_reject_stale_responses() {
        let mut state = FileMentionState {
            token: mention_token("@src", 4),
            request: 7,
            ..FileMentionState::default()
        };
        assert!(mention_response_is_current(&state, 7));
        state.request += 1;
        state.token = None;
        assert!(!mention_response_is_current(&state, 7));
        assert!(!mention_response_is_current(&state, 8));
    }

    #[test]
    fn file_mentions_serialize_to_strict_local_markdown() {
        let raw = local_file_link("src/a file#[x].rs", false);
        assert_eq!(
            raw,
            "[a file#\\[x\\].rs](zeron-file:src/a%20file%23%5Bx%5D.rs)"
        );
        let links = file_mention_links(&raw);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].path, "src/a file#[x].rs");
        assert_eq!(links[0].basename, "a file#[x].rs");
        assert!(!links[0].is_dir);

        let folder = local_file_link("src/components", true);
        assert_eq!(folder, "[components](zeron-file:src/components/)");
        let links = file_mention_links(&folder);
        assert_eq!(links[0].path, "src/components");
        assert!(links[0].is_dir);
    }

    #[test]
    fn dropped_mentions_are_separated_from_surrounding_text() {
        let (inserted, cursor_advance) =
            dropped_file_mention("fixnow", 3..3, "src/lib.rs", false).expect("valid drop");
        assert_eq!(inserted, " [lib.rs](zeron-file:src/lib.rs) ");
        assert_eq!(cursor_advance, inserted.len());

        let (inserted, cursor_advance) =
            dropped_file_mention("fix now", 3..3, "src/components", true).expect("valid drop");
        assert_eq!(inserted, " [components](zeron-file:src/components/)");
        assert_eq!(cursor_advance, inserted.len() + 1);
    }

    #[test]
    fn dropped_mentions_reject_paths_outside_the_workspace() {
        assert!(dropped_file_mention("", 0..0, "/tmp/file.rs", false).is_none());
        assert!(dropped_file_mention("", 0..0, "../file.rs", false).is_none());
    }

    #[test]
    fn file_mentions_reject_external_or_noncanonical_markdown() {
        assert!(file_mention_links("[site](https://example.com/a)").is_empty());
        assert!(file_mention_links("[a.rs](../a.rs)").is_empty());
        assert!(file_mention_links("[a.rs](src/a file.rs)").is_empty());
        assert!(file_mention_links("[other](src/a.rs)").is_empty());
        assert!(file_mention_links("[a.rs](src/a.rs)").is_empty());
        assert!(file_mention_links("[a.rs](src%5Cfake%5Ca.rs)").is_empty());
        assert!(file_mention_links("[a.rs](src/a%0A.rs)").is_empty());
    }

    #[test]
    fn duplicate_mention_basenames_use_unique_suffixes() {
        let raw = format!(
            "{} {}",
            local_file_link("src/one/mod.rs", false),
            local_file_link("src/two/mod.rs", false)
        );
        let projection = TextProjection::new(&raw);
        assert!(projection.display.contains("one/mod.rs"));
        assert!(projection.display.contains("two/mod.rs"));
    }

    #[test]
    fn mention_suffixes_compare_path_components() {
        let links = vec![
            FileMentionLink {
                range: 0..0,
                basename: "mod.rs".into(),
                path: "foo/mod.rs".into(),
                is_dir: false,
            },
            FileMentionLink {
                range: 0..0,
                basename: "oomod.rs".into(),
                path: "bar/oomod.rs".into(),
                is_dir: false,
            },
        ];
        assert_eq!(
            mention_display_labels(&links),
            vec!["mod.rs".to_string(), "oomod.rs".to_string()]
        );
    }

    #[test]
    fn projection_maps_and_expands_atomic_chip_ranges() {
        let raw = format!("open {} now", local_file_link("src/composer.rs", false));
        let projection = TextProjection::new(&raw);
        let (link, chip) = &projection.mentions[0];
        assert_eq!(
            &projection.display[chip.clone()],
            "\u{00A0}@composer.rs\u{00A0}"
        );
        assert_eq!(projection.display_to_raw(chip.start + 1), link.range.start);
        assert_eq!(projection.display_to_raw(chip.end - 1), link.range.end);
        assert_eq!(
            projection.previous_boundary(link.range.end),
            Some(link.range.start)
        );
        assert_eq!(
            projection.next_boundary(link.range.start),
            Some(link.range.end)
        );
        assert_eq!(
            projection.normalize_range(link.range.start + 2..link.range.end - 2),
            link.range
        );
    }

    #[test]
    fn sent_mention_display_projects_chips_for_the_transcript() {
        let raw = format!(
            "check {} and {}",
            local_file_link("src/composer.rs", false),
            local_file_link("src/components", true)
        );
        let (display, spans) = sent_mention_display(&raw).expect("mentions project");
        assert!(!display.contains(FILE_MENTION_SCHEME));
        assert!(display.contains("composer.rs"));
        assert!(display.contains("components"));
        assert_eq!(spans.len(), 2);
        assert_eq!(
            &display[spans[0].range.clone()],
            "\u{00A0}@composer.rs\u{00A0}"
        );
        assert!(!spans[0].is_dir);
        assert_eq!(spans[0].path.as_ref(), "src/composer.rs");
        assert!(spans[1].is_dir);
        assert_eq!(spans[1].path.as_ref(), "src/components/");
    }

    /// Ordinary prompts must stay on the zero-cost path, including ones that
    /// merely *talk about* the scheme without containing a valid mention.
    #[test]
    fn sent_mention_display_leaves_plain_prompts_untouched() {
        assert_eq!(sent_mention_display("fix the composer"), None);
        assert_eq!(
            sent_mention_display("what is a zeron-file: link?"),
            None,
            "scheme substring without a valid mention link"
        );
        assert_eq!(
            sent_mention_display("[a.rs](zeron-file:../a.rs)"),
            None,
            "a hostile path never becomes a chip in the transcript either"
        );
    }

    fn question(id: &str, options: &[&str], multi: bool) -> UserInputQuestion {
        UserInputQuestion {
            id: id.into(),
            header: "Header".into(),
            question: format!("Question {id}"),
            options: options.iter().map(|s| s.to_string()).collect(),
            multi_select: multi,
        }
    }

    #[test]
    fn flip_decision() {
        // Fits in the pill → compact stays compact.
        assert!(!composer_flip(false, 150.0, 300.0, false, false));
        // Overflow → expand.
        assert!(composer_flip(false, 320.0, 300.0, false, false));
        // Newline always expands (either mode, even mid-resize).
        assert!(composer_flip(false, 10.0, 300.0, true, false));
        assert!(composer_flip(true, 10.0, 300.0, true, true));
        // Narrow column (< MIN_COMPACT_INPUT_WIDTH) always expands.
        assert!(composer_flip(false, 10.0, 199.0, false, false));
        assert!(!composer_flip(false, 10.0, 200.0, false, false));
    }

    #[test]
    fn flip_hysteresis_band_prevents_oscillation() {
        let cap = 300.0;
        // Text just over capacity expands…
        assert!(composer_flip(false, cap + 1.0, cap, false, false));
        // …and the SAME width, now expanded, does NOT collapse back — the
        // collapse threshold sits COLLAPSE_HYSTERESIS below the expand one.
        assert!(composer_flip(true, cap + 1.0, cap, false, false));
        // Anywhere inside the band the two modes are both stable (no width in
        // (cap - 32, cap] flips in either direction).
        let in_band = cap - COLLAPSE_HYSTERESIS + 1.0;
        assert!(!composer_flip(false, in_band, cap, false, false));
        assert!(composer_flip(true, in_band, cap, false, false));
        // Comfortably under the band → collapses.
        assert!(!composer_flip(
            true,
            cap - COLLAPSE_HYSTERESIS - 1.0,
            cap,
            false,
            false
        ));
    }

    #[test]
    fn resize_expands_live_but_defers_collapse() {
        // A compact composer expands immediately as its text or controls stop
        // fitting, even while the divider is moving.
        assert!(composer_flip(false, 500.0, 300.0, false, true));
        assert!(composer_flip(false, 10.0, 150.0, false, true));
        // An expanded composer waits for the drag to settle before collapsing,
        // avoiding mode chatter while the user reverses direction.
        assert!(composer_flip(true, 0.0, 300.0, false, true));
        // Once settled, the same wide layout may collapse.
        assert!(composer_flip(false, 500.0, 300.0, false, false));
        assert!(!composer_flip(true, 0.0, 300.0, false, false));
        assert!(composer_flip(false, 10.0, 150.0, false, false));
    }

    #[test]
    fn caret_blink_phase() {
        // Solid through the first half-period (typing burst never blinks).
        assert!(caret_visible(0));
        assert!(caret_visible(CARET_BLINK_MS - 1));
        // Off for the second half-period, back on for the third.
        assert!(!caret_visible(CARET_BLINK_MS));
        assert!(!caret_visible(2 * CARET_BLINK_MS - 1));
        assert!(caret_visible(2 * CARET_BLINK_MS));
    }

    #[test]
    fn auto_grow_math() {
        // The source heights (zeron composer.tsx line 235 clamp, composer-
        // actions.tsx row, 1px hairlines): 76+46+2 empty … 260+46+2 capped.
        assert_eq!(COMPOSER_MIN_HEIGHT, 124.0);
        assert_eq!(COMPOSER_MAX_HEIGHT, 308.0);
        // One line sits at the floor: the textarea BOX (content + `pt-4 pb-1`)
        // clamps UP to 76 exactly like `Math.max(scrollHeight, 76)` — this is
        // what makes the always-expanded new-chat composer 124px tall.
        assert_eq!(
            composer_total_height(input_content_height(1)),
            COMPOSER_MIN_HEIGHT
        );
        // Growth is linear once the textarea box exceeds its 76px floor.
        let h4 = composer_total_height(input_content_height(4));
        assert_eq!(
            h4,
            4.0 * INPUT_LINE_HEIGHT + TEXTAREA_PAD_V + ACTIONS_ROW_HEIGHT + PILL_BORDER_V
        );
        // Caps at a 260px textarea box (zeron max-h-[260px] / the JS clamp).
        assert_eq!(
            composer_total_height(input_content_height(100)),
            COMPOSER_MAX_HEIGHT
        );
        // Zero lines still measures one.
        assert_eq!(input_content_height(0), INPUT_LINE_HEIGHT);
    }

    #[test]
    fn appshot_strip_height_tracks_cards() {
        assert_eq!(appshot_strip_height(0), 0.0);
        assert_eq!(appshot_strip_height(1), STRIP_PAD_TOP + APPSHOT_TILE_HEIGHT);
        assert_eq!(appshot_strip_height(2), appshot_strip_height(1));
    }

    #[test]
    fn appshot_images_share_height_and_adapt_width_without_losing_aspect_ratio() {
        let landscape = appshot_contained_size(Some((1600, 900)), 320.0);
        assert!((landscape.0 - 234.66667).abs() < 0.01);
        assert_eq!(landscape.1, APPSHOT_IMAGE_MAX_HEIGHT);
        let portrait = appshot_contained_size(Some((900, 1600)), 320.0);
        assert!((portrait.0 - 74.25).abs() < 0.01);
        assert_eq!(portrait.1, landscape.1);
        assert_eq!(
            appshot_contained_size(Some((1000, 1000)), 320.0),
            (132.0, 132.0)
        );
        // Narrow side-by-side layouts and panoramas fit without distortion.
        let narrow = appshot_contained_size(Some((1600, 900)), 160.0);
        assert_eq!(narrow, (160.0, 90.0));
        assert_eq!(
            appshot_contained_size(Some((4000, 1000)), 900.0),
            (320.0, 80.0)
        );
        let fallback = appshot_contained_size(None, 320.0);
        assert!((fallback.0 - 211.2).abs() < 0.01);
        assert_eq!(fallback.1, 132.0);
    }

    #[test]
    fn input_wheel_scroll_uses_gpui_direction_and_clamps() {
        // Positive wheel delta moves toward the start; negative moves down.
        assert_eq!(input_scroll_offset(40.0, 20.0, 200.0, 100.0), 20.0);
        assert_eq!(input_scroll_offset(40.0, -30.0, 200.0, 100.0), 70.0);
        // Neither edge can be overscrolled.
        assert_eq!(input_scroll_offset(10.0, 50.0, 200.0, 100.0), 0.0);
        assert_eq!(input_scroll_offset(90.0, -50.0, 200.0, 100.0), 100.0);
        // Short content has no internal scroll range.
        assert_eq!(input_scroll_offset(20.0, -50.0, 80.0, 100.0), 0.0);
    }

    #[test]
    fn input_scroll_reveals_only_when_caret_leaves_viewport() {
        // A visible caret preserves the user's viewport.
        assert_eq!(
            input_scroll_offset_for_cursor(40.0, 60.0, 20.0, 300.0, 100.0, None),
            40.0
        );
        // Moving above or below reveals the row with the smallest adjustment.
        assert_eq!(
            input_scroll_offset_for_cursor(80.0, 30.0, 20.0, 300.0, 100.0, None),
            30.0
        );
        assert_eq!(
            input_scroll_offset_for_cursor(20.0, 130.0, 20.0, 300.0, 100.0, None),
            50.0
        );
        // Revealing the final row clamps exactly to the content end.
        assert_eq!(
            input_scroll_offset_for_cursor(0.0, 290.0, 20.0, 300.0, 100.0, None),
            200.0
        );
    }

    #[test]
    fn input_drag_autoscroll_is_edge_proportional_and_capped() {
        let top = 100.0;
        let bottom = 300.0;
        let line = INPUT_LINE_HEIGHT;
        assert_eq!(input_drag_scroll_delta(200.0, top, bottom, line), 0.0);
        assert_eq!(input_drag_scroll_delta(90.0, top, bottom, line), -2.0);
        assert_eq!(input_drag_scroll_delta(315.0, top, bottom, line), 3.0);
        assert_eq!(input_drag_scroll_delta(-100.0, top, bottom, line), -line);
        assert_eq!(input_drag_scroll_delta(500.0, top, bottom, line), line);
    }

    /// One frame short of the full morph timeline (never rounds up to done).
    const ALMOST: f32 = 179.0;

    #[test]
    fn flip_morph_starts_once_per_committed_flip() {
        // No committed flip → no morph.
        assert_eq!(flip_morph_step(None, false, 49.0, 0.0, false, false), None);
        // A committed flip starts one, from the last rendered height…
        let m = flip_morph_step(None, true, 49.0, 100.0, false, false).unwrap();
        assert_eq!(m.from, 49.0);
        assert_eq!(m.start_ms, 100.0);
        // …and same-mode renders keep it UNCHANGED (no restart at the
        // boundary, whatever the heights are doing).
        assert_eq!(
            flip_morph_step(Some(m), false, 80.0, 150.0, false, false),
            Some(m)
        );
        // A finished morph clears on the next same-mode render.
        assert_eq!(
            flip_morph_step(Some(m), false, 124.0, 100.0 + ALMOST, false, false),
            Some(m)
        );
        assert_eq!(
            flip_morph_step(Some(m), false, 124.0, 300.0, false, false),
            None
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn resolved_layout_does_not_keep_notifying_on_repaint() {
        use std::cell::Cell;
        gpui_platform::headless().run(|cx| {
            cx.set_global(Theme::dark());
            let handle = cx.open_window(gpui::WindowOptions::default(), |_, cx| {
                cx.new(|cx| {
                    let mut input = ComposerInput::new("Draft", cx);
                    input.set_text("A long line whose wrapping differs between provisional and resolved widths.\n".repeat(100), cx);
                    input
                })
            }).unwrap();
            let changes = Rc::new(Cell::new(0));
            let observed = changes.clone();
            let subscription = cx.subscribe(&handle.entity(cx).unwrap(), move |_, event, _| {
                if matches!(event, ComposerInputEvent::ViewportChanged) {
                    observed.set(observed.get() + 1);
                }
            });
            cx.spawn(async move |cx| {
                let _subscription = subscription;
                cx.update(|cx| {
                    handle.update(cx, |input, _, _| input.last_notified_layout = None).unwrap();
                    cx.update_window(handle.into(), |_, window, cx| { window.refresh(); let _ = window.draw(cx); }).unwrap();
                });
                let settled = changes.get();
                assert!(settled > 0, "the first resolved layout must be published");
                for _ in 0..30 {
                    cx.update(|cx| {
                        cx.update_window(handle.into(), |_, window, cx| { window.refresh(); let _ = window.draw(cx); }).unwrap();
                    });
                }
                assert_eq!(changes.get(), settled, "unchanged draws must not schedule more layout");
                cx.update(|cx| cx.quit());
            }).detach();
        });
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn single_line_address_reveals_caret_and_maps_scrolled_pointer() {
        gpui_platform::headless().run(|cx| {
            cx.set_global(Theme::dark());
            let handle = cx
                .open_window(gpui::WindowOptions::default(), |_, cx| {
                    cx.new(|cx| {
                        ComposerInput::new("Address", cx)
                            .with_single_line()
                            .with_text_metrics(11.0, 16.0)
                    })
                })
                .unwrap();
            handle
                .update(cx, |input, window, cx| {
                    let style = window.text_style();
                    input.set_text(
                        "http://device.a-very-long-project-name.localhost:7331/path",
                        cx,
                    );
                    input.layout_text(px(100.0), &style, window, cx);
                    assert_eq!(input.content_height, 16.0, "long hostnames must not wrap");
                    input.clamp_scroll(16.0);
                    assert!(input.scroll_left > 0.0);
                    let bounds = Bounds::new(point(px(10.0), px(20.0)), size(px(100.0), px(16.0)));
                    input.last_bounds = Some(bounds);
                    let caret = input
                        .bounds_for_range(
                            input.content.len()..input.content.len(),
                            bounds,
                            window,
                            cx,
                        )
                        .unwrap();
                    assert!(caret.left() >= bounds.left() && caret.right() <= bounds.right());
                    assert_eq!(
                        input.index_for_mouse_position(caret.origin),
                        input.content.len()
                    );
                    input.selected_range = 0..0;
                    input.clamp_scroll(16.0);
                    assert_eq!(input.scroll_left, 0.0, "Home must reveal the URL start");
                    input.replace_text_in_range(None, "one\r\ntwo", window, cx);
                    assert!(!input.content.contains(['\r', '\n']));
                    input.set_text("short", cx);
                    input.layout_text(px(100.0), &style, window, cx);
                    input.clamp_scroll(16.0);
                    assert_eq!(
                        input.scroll_left, 0.0,
                        "short replacement must reset scrolling"
                    );
                })
                .unwrap();
            cx.spawn(async move |cx| {
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn layout_cache_reuses_resize_frames_and_invalidates_text_inputs() {
        gpui_platform::headless().run(|cx| {
            cx.set_global(Theme::dark());
            let handle = cx
                .open_window(gpui::WindowOptions::default(), |_, cx| {
                    cx.new(|cx| ComposerInput::new("Draft", cx))
                })
                .unwrap();
            handle
                .update(cx, |input, window, cx| {
                    input.layout_rebuilds = 0; // Exclude the window's initial placeholder paint.
                    let mut style = window.text_style();
                    style.font_size = px(INPUT_TEXT_SIZE).into();
                    input.set_text(
                        "A wrapped draft with enough text to measure.\n".repeat(100),
                        cx,
                    );
                    input.layout_text(px(400.0), &style, window, cx);
                    assert_eq!(input.layout_rebuilds, 1);
                    let height = input.content_height;
                    for frame in 0..120 {
                        input.viewport_height = Some(40.0 + frame as f32);
                        input.scroll_top = frame as f32;
                        input.selected_range = 2..8;
                        assert_eq!(input.layout_text(px(400.0), &style, window, cx), height);
                    }
                    assert_eq!(
                        input.layout_rebuilds, 1,
                        "resize/scroll/selection must reuse shaping"
                    );
                    input.set_text("Edited draft", cx);
                    input.layout_text(px(400.0), &style, window, cx);
                    assert_eq!(input.layout_rebuilds, 2);
                    input.layout_text(px(200.0), &style, window, cx);
                    assert_eq!(input.layout_rebuilds, 3, "width changes must rewrap");
                    style.font_size = px(18.0).into();
                    input.layout_text(px(200.0), &style, window, cx);
                    assert_eq!(input.layout_rebuilds, 4);
                    input.marked_range = Some(0..2);
                    input.layout_text(px(200.0), &style, window, cx);
                    assert_eq!(
                        input.layout_rebuilds, 5,
                        "IME marking must repaint decoration"
                    );
                    input.unmark_text(window, cx);
                    input.layout_text(px(200.0), &style, window, cx);
                    assert_eq!(input.layout_rebuilds, 6, "IME unmark must also invalidate");
                    input.set_text("", cx);
                    input.layout_text(px(200.0), &style, window, cx);
                    input.set_placeholder("New placeholder", cx);
                    input.layout_text(px(200.0), &style, window, cx);
                    assert_eq!(input.layout_rebuilds, 8);
                    style.color = gpui::rgb(0xff0000).into();
                    input.layout_text(px(200.0), &style, window, cx);
                    assert_eq!(input.layout_rebuilds, 9);
                    input.enable_mentions();
                    input.layout_text(px(200.0), &style, window, cx);
                    assert_eq!(input.layout_rebuilds, 10);
                    cx.set_global(Theme::light());
                    input.layout_text(px(200.0), &style, window, cx);
                    assert_eq!(input.layout_rebuilds, 11, "mention colors follow the theme");
                })
                .unwrap();
            cx.spawn(async move |cx| {
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    }

    #[test]
    fn resize_reveals_only_complete_rows() {
        for visible in [0.0, 5.0, 22.0, 22.75, 30.0, 45.5, 70.0, 150.0] {
            let height = input_reveal_height(visible, 0.0, INPUT_LINE_HEIGHT, true);
            assert!(height <= visible);
            assert_eq!(height % INPUT_LINE_HEIGHT, 0.0);
        }
        // The row grid moves with scrolling; the clip still ends between rows.
        assert_eq!(input_reveal_height(39.0, 7.0, 20.0, true), 33.0);
        // Normal overflow scrolling keeps its full viewport and existing fades.
        assert_eq!(input_reveal_height(39.0, 7.0, 20.0, false), 39.0);
        assert_eq!(input_reveal_height(100.0, 0.0, 20.0, true), 100.0);
    }

    #[test]
    fn resize_keeps_text_anchored_to_the_input_origin() {
        // A fitting draft grows from one row to seven. Caret-follow must
        // never temporarily scroll earlier lines through the top clip.
        for visible in [0.0, 22.75, 60.0, 110.0, 159.25] {
            assert_eq!(
                input_scroll_offset_for_cursor(0.0, 136.5, 22.75, 159.25, visible, Some(159.25),),
                0.0
            );
        }
        // A genuinely overflowing draft keeps the same caret-follow offset
        // through every frame of the reveal, rather than chasing its height.
        for visible in [30.0, 100.0, 180.0, 240.0] {
            assert_eq!(
                input_scroll_offset_for_cursor(160.0, 377.25, 22.75, 400.0, visible, Some(240.0),),
                160.0
            );
        }
        // Deleting back to a fitting draft resets scroll immediately, even
        // while the old, larger viewport is still shrinking.
        assert_eq!(
            input_scroll_offset_for_cursor(160.0, 77.25, 22.75, 100.0, 240.0, Some(100.0),),
            0.0
        );
    }

    #[test]
    fn scroll_fade_ignores_temporary_resize_overflow() {
        for visible_height in [0.0, 20.0, 60.0, 100.0, 160.0] {
            let scroll = input_max_scroll(160.0, visible_height);
            assert_eq!(
                input_overflow_edges(160.0, 160.0, visible_height, scroll),
                (false, false)
            );
        }
        // Deleting a capped draft disables fading immediately, even while
        // its scroll position and outer height are still settling.
        assert_eq!(
            input_overflow_edges(100.0, 100.0, 240.0, 80.0),
            (false, false)
        );
    }

    #[test]
    fn scroll_fade_tracks_real_overflow_edges() {
        for (scroll, top, bottom) in [(0.0, false, true), (80.0, true, true), (160.0, true, false)]
        {
            assert_eq!(
                input_overflow_edges(400.0, 240.0, 240.0, scroll),
                (top, bottom)
            );
        }
    }

    #[test]
    fn content_resize_retargets_from_visible_height_and_settles() {
        let start = composer_total_height(input_content_height(3));
        let target = composer_total_height(input_content_height(6));
        let grow = flip_morph_step(None, true, start, 0.0, false, false).unwrap();
        let visible = grow.height(target, 60.0);
        assert!(visible > start && visible < target);
        // A delete during growth reverses from what is on screen, with no snap.
        let shrink = flip_morph_step(Some(grow), true, visible, 60.0, false, false).unwrap();
        assert_eq!(shrink.height(start, 60.0), visible);
        assert!(shrink.height(start, 120.0) < visible);
        assert_eq!(shrink.height(start, 240.0), start);
        assert_eq!(
            flip_morph_step(Some(shrink), false, start, 240.0, false, false),
            None
        );
        // Toggling reduced motion also cancels an already running resize.
        assert_eq!(
            flip_morph_step(Some(grow), false, visible, 60.0, true, false),
            None
        );
    }

    #[test]
    fn flip_morph_height_ramps_monotonically_to_target() {
        let m = FlipMorph {
            from: 49.0,
            start_ms: 0.0,
            spec: motion::COLLAPSE,
        };
        // Starts exactly at the committed height…
        let mut prev = m.height(124.0, 0.0);
        assert_eq!(prev, 49.0);
        // …ramps without ever moving backwards…
        for step in 1..=18 {
            let h = m.height(124.0, step as f32 * 10.0);
            assert!(h >= prev, "height regressed at {step}: {h} < {prev}");
            prev = h;
        }
        // …and lands exactly on the target when done (and stays there).
        assert_eq!(m.height(124.0, 180.0), 124.0);
        assert!(m.done(180.0));
        assert_eq!(m.height(124.0, 500.0), 124.0);
        // Collapse runs the same ramp downward.
        assert!(m.height(124.0, 90.0) > 49.0);
        let down = FlipMorph {
            from: 124.0,
            start_ms: 0.0,
            spec: motion::COLLAPSE,
        };
        assert!(down.height(49.0, 90.0) < 124.0);
        assert!(down.height(49.0, 90.0) > 49.0);
    }

    #[test]
    fn flip_morph_reverse_hands_off_from_current_height() {
        let m = FlipMorph {
            from: 49.0,
            start_ms: 0.0,
            spec: motion::COLLAPSE,
        };
        let mid = m.height(124.0, 90.0);
        assert!(mid > 49.0 && mid < 124.0);
        // A reverse flip mid-flight commits a new morph FROM the animated
        // height — continuous at the handoff, no pop to an endpoint.
        let rev = flip_morph_step(Some(m), true, mid, 90.0, false, false).unwrap();
        assert_eq!(rev.from, mid);
        assert_eq!(rev.height(49.0, 90.0), mid);
    }

    #[test]
    fn flip_morph_snaps_for_reduced_motion_and_first_paint() {
        // Reduced motion never creates a morph (the flip just snaps)…
        assert_eq!(flip_morph_step(None, true, 49.0, 0.0, true, false), None);
        // …and neither does a flip before anything was ever rendered.
        assert_eq!(flip_morph_step(None, true, 0.0, 0.0, false, false), None);
    }

    #[test]
    fn route_change_never_arms_the_morph() {
        // A flip committed inside the route-snap window must NOT animate —
        // switching sessions (chat↔chat or chat↔new-session) snaps the
        // composer straight to the target mode, like the header (round 6).
        assert_eq!(flip_morph_step(None, true, 49.0, 0.0, false, true), None);
        // The route change also kills anything already in flight…
        let m = FlipMorph {
            from: 49.0,
            start_ms: 0.0,
            spec: motion::COLLAPSE,
        };
        assert_eq!(
            flip_morph_step(Some(m), false, 80.0, 50.0, false, true),
            None
        );
        assert_eq!(
            flip_morph_step(Some(m), true, 80.0, 50.0, false, true),
            None
        );
        // …while outside the window the same flip animates as usual.
        let armed = flip_morph_step(None, true, 49.0, 300.0, false, false).unwrap();
        assert_eq!(armed.from, 49.0);
    }

    #[test]
    fn morph_anchoring_holds_controls_and_glides_text() {
        // Steady state (progress 1): no offsets, everything at rest.
        assert_eq!(morph_cluster_dy(1.0), 0.0);
        assert_eq!(morph_text_pad(1.0), 16.0);
        assert_eq!(collapse_text_glide(124.0, 1.0), 0.0);
        // At the commit instant the pieces start from the OLD mode's resting
        // geometry: text pad at the compact 12px inset, cluster displaced by
        // exactly the 4.5px centering delta.
        assert_eq!(morph_text_pad(0.0), 12.0);
        assert_eq!(morph_cluster_dy(0.0), CLUSTER_Y_DELTA);
        // Collapse glide: starts where the expanded text sat (17px below the
        // committed pill top → `from − 53` above the compact resting spot)…
        assert_eq!(collapse_text_glide(124.0, 0.0), 71.0);
        // …decays monotonically to zero…
        let mut prev = collapse_text_glide(124.0, 0.0);
        for step in 1..=10 {
            let g = collapse_text_glide(124.0, step as f32 / 10.0);
            assert!(g <= prev, "glide regressed at {step}");
            prev = g;
        }
        // …and can't go negative on shallow mid-flight reversals.
        assert_eq!(collapse_text_glide(50.0, 0.0), 0.0);
    }

    #[test]
    fn cluster_inset_glides_between_the_source_endpoints() {
        assert_eq!(ACTION_UTILITY_GAP, 2.0);
        assert_eq!(ACTION_PRIMARY_GAP, Theme::SPACE_SM);
        assert!(ACTION_UTILITY_GAP < ACTION_PRIMARY_GAP);
        // The morph starts from the OLD mode's resting inset (no sideways
        // step at the commit) and eases to the committed mode's…
        assert_eq!(morph_cluster_inset(true, 0.0), 8.0); // expand: from compact pr-2
        assert_eq!(morph_cluster_inset(true, 1.0), 12.0); // …to expanded px-3
        assert_eq!(morph_cluster_inset(false, 0.0), 12.0); // collapse: from px-3
        assert_eq!(morph_cluster_inset(false, 1.0), 8.0); // …to pr-2
        // …monotonically, bounded by the 4px source delta.
        let mut prev = morph_cluster_inset(true, 0.0);
        for step in 1..=10 {
            let v = morph_cluster_inset(true, step as f32 / 10.0);
            assert!(v >= prev && v <= 8.0 + CLUSTER_X_DELTA);
            prev = v;
        }
        // Internal group spacing is shared between modes — only this wrapper
        // inset may differ across the flip.
    }

    #[test]
    fn flip_morph_tracks_live_target_and_drives_fade() {
        let m = FlipMorph {
            from: 49.0,
            start_ms: 0.0,
            spec: motion::COLLAPSE,
        };
        // Auto-grow can move the target mid-morph: evaluation tracks the
        // live value instead of finishing on a stale height.
        assert!(m.height(159.0, 90.0) > m.height(124.0, 90.0));
        // The eased progress is the actions-row fade: 0 at commit, 1 at rest.
        assert_eq!(m.progress(0.0), 0.0);
        assert_eq!(m.progress(180.0), 1.0);
        let mid = m.progress(90.0);
        assert!(mid > 0.0 && mid < 1.0);
    }

    #[test]
    fn new_thread_route_changes_use_the_coordinated_timeline() {
        let m = FlipMorph::new_thread_transition(124.0, 0.0);
        assert_eq!(m.spec, motion::NEW_THREAD_TRANSITION);
        assert_eq!(m.height(49.0, 0.0), 124.0);
        assert!(m.height(49.0, 250.0) < 124.0);
        assert!(m.height(49.0, 250.0) > 49.0);
        assert_eq!(m.height(49.0, 420.0), 49.0);
        let reverse = FlipMorph::new_thread_transition(49.0, 0.0);
        assert_eq!(reverse.height(124.0, 0.0), 49.0);
        assert_eq!(reverse.height(124.0, 420.0), 124.0);
    }

    #[test]
    fn new_thread_selectors_restore_the_compact_floating_row() {
        assert_eq!(NEW_THREAD_SELECTOR_ROW_HEIGHT, 20.0);
        assert_eq!(SESSION_FOOTER_HEIGHT, 24.0);
    }

    #[test]
    fn route_chrome_crossfade_never_duplicates_picker_controls() {
        assert_eq!(route_chrome_opacities(1.0), (1.0, 0.0));
        assert_eq!(route_chrome_opacities(0.5), (0.0, 0.0));
        assert_eq!(route_chrome_opacities(0.0), (0.0, 1.0));
        for step in 0..=20 {
            let (new_thread, session) = route_chrome_opacities(step as f32 / 20.0);
            assert!(new_thread == 0.0 || session == 0.0);
        }
    }

    #[test]
    fn staged_comments_alone_are_content() {
        assert!(!composer_has_content("   ", 0, 0));
        assert!(composer_has_content("hi", 0, 0));
        assert!(composer_has_content("", 1, 0));
        assert!(composer_has_content("", 0, 1));
    }

    #[test]
    fn modified_submit_sends_content_and_activates_latest_queue_row_when_empty() {
        assert_eq!(
            modified_submit_target(composer_has_content("message", 0, 0)),
            ModifiedSubmitTarget::SubmitContent
        );
        assert_eq!(
            modified_submit_target(composer_has_content("", 1, 0)),
            ModifiedSubmitTarget::SubmitContent
        );
        assert_eq!(
            modified_submit_target(composer_has_content("", 0, 1)),
            ModifiedSubmitTarget::SubmitContent
        );
        assert_eq!(
            modified_submit_target(composer_has_content("  ", 0, 0)),
            ModifiedSubmitTarget::ActivateLatestQueued
        );
    }

    #[test]
    fn a_comment_only_stage_queues_during_a_live_run() {
        let live = true;
        let comment_only = composer_has_content("", 0, 2);
        assert_eq!(
            send_button_mode(live, comment_only),
            SendButtonMode::Queue,
            "comment-only submit must queue without interrupting the run"
        );
        // Nothing staged at all is still the stop square.
        assert_eq!(
            send_button_mode(live, composer_has_content("", 0, 0)),
            SendButtonMode::Stop
        );
    }

    #[test]
    fn send_button_morph() {
        assert_eq!(send_button_mode(false, false), SendButtonMode::Send);
        assert_eq!(send_button_mode(false, true), SendButtonMode::Send);
        assert_eq!(send_button_mode(true, true), SendButtonMode::Queue);
        assert_eq!(send_button_mode(true, false), SendButtonMode::Stop);
    }

    #[test]
    fn queued_submit_does_not_publish_an_optimistic_transcript_echo() {
        assert!(should_publish_optimistic_echo(false));
        assert!(!should_publish_optimistic_echo(true));
    }

    #[test]
    fn interrupt_tracking_is_idempotent_per_chat() {
        let mut pending = HashSet::new();
        assert!(begin_interrupt(&mut pending, "chat-a"));
        assert!(!begin_interrupt(&mut pending, "chat-a"));
        assert!(begin_interrupt(&mut pending, "chat-b"));
        assert_eq!(pending.len(), 2);
    }

    #[test]
    fn interrupt_tracking_releases_only_settled_chats() {
        let mut pending = HashSet::from(["chat-a".to_string(), "chat-b".to_string()]);
        retain_live_interrupts(&mut pending, |chat_id| chat_id == "chat-b");
        assert_eq!(pending, HashSet::from(["chat-b".to_string()]));
        assert!(begin_interrupt(&mut pending, "chat-a"));
    }

    #[test]
    fn interrupt_payload_keeps_the_captured_chat() {
        let params = interrupt_params("chat-a");
        assert_eq!(params["chatId"], "chat-a");
        assert_eq!(params["command"]["kind"], "interrupt");
    }

    #[test]
    fn escape_consumers_keep_completion_and_wizard_priority() {
        assert!(escape_dismisses_completion("escape", true));
        assert!(!escape_dismisses_completion("escape", false));
        assert!(!escape_dismisses_completion("enter", true));

        assert!(wizard_escape_goes_back("escape", false, false));
        assert!(wizard_escape_goes_back("escape", true, true));
        assert!(!wizard_escape_goes_back("escape", true, false));
        assert!(!wizard_escape_goes_back("enter", false, true));
    }

    #[test]
    fn wizard_single_select_auto_advances_and_completes() {
        let mut w = Wizard::new(
            "req".into(),
            vec![
                question("q1", &["a", "b"], false),
                question("q2", &["x"], false),
            ],
        );
        assert_eq!(w.counter(), "1/2");
        assert_eq!(w.select(1), WizardStep::AutoAdvance);
        assert!(w.is_picked(1));
        assert_eq!(w.advance(), WizardStep::Stay);
        assert_eq!(w.counter(), "2/2");
        assert_eq!(w.select(0), WizardStep::AutoAdvance);
        let WizardStep::Done(answers) = w.advance() else {
            panic!("expected Done")
        };
        assert_eq!(answers.len(), 2);
        assert_eq!(answers[0].labels, vec!["b"]);
        assert_eq!(answers[1].labels, vec!["x"]);
    }

    #[test]
    fn wizard_multi_select_toggles_and_stays() {
        let mut w = Wizard::new("req".into(), vec![question("q", &["a", "b", "c"], true)]);
        assert_eq!(w.select(0), WizardStep::Stay);
        assert_eq!(w.select(2), WizardStep::Stay);
        assert!(w.is_picked(0) && w.is_picked(2));
        // Toggle off.
        assert_eq!(w.select(0), WizardStep::Stay);
        assert!(!w.is_picked(0));
        let WizardStep::Done(answers) = w.advance() else {
            panic!()
        };
        assert_eq!(answers[0].labels, vec!["c"]);
    }

    #[test]
    fn wizard_number_keys_and_bounds() {
        let mut w = Wizard::new("req".into(), vec![question("q", &["a", "b"], false)]);
        assert_eq!(w.press_number(9), WizardStep::Stay, "out of range ignored");
        assert_eq!(w.press_number(0), WizardStep::Stay);
        assert_eq!(w.press_number(2), WizardStep::AutoAdvance);
        assert!(w.is_picked(1));
        assert_eq!(w.select(5), WizardStep::Stay, "bad option ix ignored");
    }

    #[test]
    fn wizard_typed_answer_overrides_and_back_pages() {
        let mut w = Wizard::new(
            "req".into(),
            vec![
                question("q1", &["a"], false),
                question("q2", &["x", "y"], false),
            ],
        );
        w.select(0);
        w.advance();
        assert_eq!(w.page, 1);
        assert!(w.back());
        assert_eq!(w.page, 0);
        assert!(!w.back(), "already at first page");
        w.advance();
        w.set_typed("  custom answer  ".into());
        let WizardStep::Done(answers) = w.advance() else {
            panic!()
        };
        assert_eq!(answers[0].labels, vec!["a"]);
        assert_eq!(
            answers[1].labels,
            vec!["custom answer"],
            "typed overrides picked, trimmed"
        );
    }

    #[test]
    fn pending_input_detection() {
        use zeron_doc::MessageStatus;
        let input_part = MessagePart::Input {
            id: "in-r1".into(),
            request_id: "r1".into(),
            questions: vec![question("q", &["a"], false)],
            resolved: false,
        };
        let entry = |status: Option<MessageStatus>, parts: Vec<MessagePart>| SessionMessageEntry {
            id: "m".into(),
            role: MessageRole::Assistant,
            parts,
            created_at: 0,
            device_id: "d".into(),
            status,
            continuation_of: None,
        };
        // Streaming entry with unresolved input → panel.
        let t = vec![entry(
            Some(MessageStatus::Streaming),
            vec![input_part.clone()],
        )];
        assert_eq!(
            pending_input_request(&t).map(|(id, _)| id),
            Some("r1".into())
        );
        // DEAD entry with an unresolved input STILL gets the panel: the
        // question stays answerable until answered (the engine delivers the
        // answer as a resumed turn), so a run reaped under its question —
        // engine restart — must not orphan it (user report).
        let t = vec![entry(
            Some(MessageStatus::Aborted),
            vec![input_part.clone()],
        )];
        assert_eq!(
            pending_input_request(&t).map(|(id, _)| id),
            Some("r1".into())
        );
        // A NEWER assistant entry supersedes an unanswered question.
        let t = vec![
            entry(Some(MessageStatus::Aborted), vec![input_part.clone()]),
            SessionMessageEntry {
                id: "m2".into(),
                role: MessageRole::Assistant,
                parts: vec![MessagePart::Text {
                    id: "t2".into(),
                    text: "moved on".into(),
                }],
                created_at: 2,
                device_id: "d".into(),
                status: Some(MessageStatus::Complete),
                continuation_of: None,
            },
        ];
        assert!(pending_input_request(&t).is_none());
        // Resolved part → no panel.
        let resolved = MessagePart::Input {
            id: "in-r1".into(),
            request_id: "r1".into(),
            questions: vec![],
            resolved: true,
        };
        let t = vec![entry(
            Some(MessageStatus::Streaming),
            vec![resolved.clone()],
        )];
        assert!(pending_input_request(&t).is_none());
        assert!(pending_input_request(&[]).is_none());

        // Regression (user forensics): a steer prompt appends a USER entry
        // AFTER the streaming assistant entry — the question must still be
        // found (a last-entry-only read vanished the panel exactly when the
        // user typed, bricking the answer flow).
        let user_echo = SessionMessageEntry {
            id: "u2".into(),
            role: MessageRole::User,
            parts: vec![MessagePart::Text {
                id: "t".into(),
                text: "I answered".into(),
            }],
            created_at: 1,
            device_id: "d".into(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
        };
        let t = vec![
            entry(Some(MessageStatus::Streaming), vec![input_part.clone()]),
            user_echo,
        ];
        assert_eq!(
            pending_input_request(&t).map(|(id, _)| id),
            Some("r1".into()),
            "question survives entries appended behind the streaming entry"
        );

        // Latch release: only an explicitly resolved matching part releases.
        assert!(!input_request_resolved(&t, "r1"));
        let t = vec![entry(Some(MessageStatus::Streaming), vec![resolved])];
        assert!(input_request_resolved(&t, "r1"));
        assert!(!input_request_resolved(&t, "other"));
    }

#[cfg(test)]
mod appshot_rebase_tests {
    use super::*;
    use gpui::{AppContext, TestAppContext};

    #[gpui::test]
    fn appshot_only_draft_counts_as_content_and_removal_clears_it(cx: &mut TestAppContext) {
        let state = cx.new(|_| AppState::new());
        let composer = cx.new(|cx| Composer::new(state, cx));
        composer.update(cx, |composer, cx| {
            let shot = appshots::tests::shot();
            composer.stage_appshot(shot.clone(), cx);
            assert!(composer_has_content(
                "",
                composer.staged().len() + composer.staged_appshots().len(),
                0
            ));
            composer.remove_appshot(&shot.id, cx);
            assert!(composer.staged_appshots().is_empty());
            assert!(!composer_has_content(
                "",
                composer.staged().len() + composer.staged_appshots().len(),
                0
            ));
        });
    }

    #[gpui::test]
    fn failed_send_restores_complete_appshots_without_duplicates(cx: &mut TestAppContext) {
        let state = cx.new(|_| AppState::new());
        let composer = cx.new(|cx| Composer::new(state, cx));
        composer.update(cx, |composer, cx| {
            let original = appshots::tests::shot();
            let mut fresh = original.clone();
            fresh.id = "fresh".into();
            composer.stage_appshot_for("minted".into(), original.clone(), cx);
            composer.stage_appshot(fresh, cx);
            composer.restore_failed_appshots(&[original.clone()], "minted", "");
            assert_eq!(composer.staged_appshots().len(), 2);
            assert_eq!(
                composer.staged_appshots()[0].accessibility,
                original.accessibility
            );
            assert_eq!(
                composer.staged_appshots()[0].screenshot.id,
                original.screenshot.id
            );
            assert_eq!(composer.staged_appshots()[1].id, "fresh");
            assert!(!composer.appshots.contains_key("minted"));
        });
    }

    #[gpui::test]
    fn remote_queue_removal_recovers_both_appshot_drafts(cx: &mut TestAppContext) {
        let state = cx.new(|_| AppState::new());
        let composer = cx.new(|cx| Composer::new(state, cx));
        composer.update(cx, |composer, cx| {
            let original = appshots::tests::shot();
            let mut edited = original.clone();
            edited.id = "edited".into();
            composer.stage_appshot(edited, cx);
            composer.queue_edit_draft = Some(("original".into(), vec![], vec![original]));
            composer.editing_queued = Some("removed-row".into());
            composer
                .input
                .update(cx, |input, cx| input.set_text("edited", cx));
            composer.on_state_changed(cx);
            assert!(composer.editing_queued.is_none());
            assert_eq!(composer.staged_appshots().len(), 2);
            assert_eq!(composer.input.read(cx).text(), "original\n\nedited");
        });
    }
}
