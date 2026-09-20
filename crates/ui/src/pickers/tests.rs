use super::*;
use zeron_proto::{FolderEntry, Model, ModelOption, ModelOptionChoice};

struct ModelShortcutHost {
    focus_sub: Option<gpui::Subscription>,
    root: FocusHandle,
    editor: FocusHandle,
    neutral: FocusHandle,
    pickers: Entity<Pickers>,
}

impl Render for ModelShortcutHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_sub.is_none() {
            self.focus_sub = Some(cx.on_focus_lost(window, |this: &mut Self, window, cx| {
                let root = this.root.clone();
                let editor = this.editor.clone();
                let neutral = this.neutral.clone();
                window.on_next_frame(move |window, cx| {
                    crate::shell::restore_mounted_focus(&root, &editor, &neutral, window, cx);
                });
                cx.notify();
            }));
        }
        let root = self.root.clone();
        let editor = self.editor.clone();
        let neutral = self.neutral.clone();
        window.defer(cx, move |window, cx| {
            crate::shell::restore_mounted_focus(&root, &editor, &neutral, window, cx);
        });
        div()
            .size_full()
            .track_focus(&self.root)
            .on_action(
                cx.listener(|this, _: &crate::shell::OpenModelPicker, window, cx| {
                    this.pickers
                        .update(cx, |pickers, cx| pickers.open_model_menu(window, cx));
                    cx.notify();
                }),
            )
            .child(div().track_focus(&self.editor))
            .child(div().track_focus(&self.neutral))
            .child(self.pickers.clone())
    }
}

#[gpui::test]
fn model_shortcut_focuses_mounted_picker_and_routes_navigation(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        cx.set_global(Theme::dark());
        crate::composer::init(cx, Default::default());
        cx.bind_keys([gpui::KeyBinding::new(
            "cmd-/",
            crate::shell::OpenModelPicker,
            None,
        )]);
    });
    let handle = cx.add_window(|_, cx| ModelShortcutHost {
        focus_sub: None,
        root: cx.focus_handle(),
        editor: cx.focus_handle(),
        neutral: cx.focus_handle(),
        pickers: cx.new(|cx| {
            let state = cx.new(|_| AppState::new());
            let mut pickers = Pickers::new(state, cx);
            pickers.config.harness = Some(HarnessId::ClaudeCode);
            pickers.config.model = Some("first".into());
            pickers.harnesses =
                Loadable::Ready(vec![descriptor(HarnessId::ClaudeCode, "Claude Code")]);
            pickers.models.insert(
                HarnessId::ClaudeCode,
                Loadable::Ready(vec![
                    bare_model("first", "First"),
                    bare_model("second", "Second"),
                ]),
            );
            pickers
        }),
    });
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
        .unwrap();
    handle
        .update(cx, |host, window, cx| window.focus(&host.editor, cx))
        .unwrap();
    cx.simulate_keystrokes(handle.into(), "cmd-/");
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
        .unwrap();
    handle
        .update(cx, |host, window, cx| {
            host.pickers.read_with(cx, |pickers, cx| {
                assert!(pickers.is_open());
                assert!(
                    pickers.focus.contains_focused(window, cx),
                    "shortcut must transfer focus into the mounted picker"
                );
            });
        })
        .unwrap();
    cx.simulate_keystrokes(handle.into(), "down");
    handle
        .read_with(cx, |host, cx| assert_eq!(host.pickers.read(cx).active, 1))
        .unwrap();
    cx.simulate_keystrokes(handle.into(), "up");
    handle
        .read_with(cx, |host, cx| assert_eq!(host.pickers.read(cx).active, 0))
        .unwrap();
    cx.simulate_keystrokes(handle.into(), "escape");
    handle
        .read_with(cx, |host, cx| assert!(!host.pickers.read(cx).is_open()))
        .unwrap();

    // Catalog loading/error/empty cards omit the input but still need
    // immediate keyboard focus on their mounted frame for Escape.
    for catalog in [
        Loadable::Loading,
        Loadable::Error("offline".into()),
        Loadable::Ready(vec![]),
    ] {
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        handle
            .update(cx, |host, window, cx| {
                host.pickers.update(cx, |pickers, cx| {
                    pickers.config.harness = None;
                    pickers.defaults.harness = None;
                    pickers.harnesses = catalog;
                    cx.notify();
                });
                window.focus(&host.editor, cx);
            })
            .unwrap();
        cx.simulate_keystrokes(handle.into(), "cmd-/");
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        handle
            .update(cx, |host, window, cx| {
                assert!(host.pickers.read(cx).focus.contains_focused(window, cx));
            })
            .unwrap();
        cx.simulate_keystrokes(handle.into(), "escape");
        handle
            .read_with(cx, |host, cx| assert!(!host.pickers.read(cx).is_open()))
            .unwrap();
    }
}

#[gpui::test]
fn workspace_footer_pair_keeps_its_leading_edge_and_gap(cx: &mut gpui::TestAppContext) {
    struct Fixture {
        width: f32,
        bounds: std::rc::Rc<std::cell::RefCell<Vec<gpui::Bounds<gpui::Pixels>>>>,
    }
    impl gpui::Render for Fixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let chip = |width| {
                let measured = self.bounds.clone();
                gpui::canvas(
                    move |bounds, _, _| measured.borrow_mut().push(bounds),
                    |_, _, _, _| {},
                )
                .w(px(width))
                .h(px(20.0))
                .flex_none()
            };
            div().w(px(self.width)).child(
                workspace_footer_row()
                    .px(px(10.0))
                    .child(chip(120.0))
                    .child(chip(90.0))
                    .child(div().flex_1().min_w_0())
                    .child(chip(60.0)),
            )
        }
    }
    let bounds = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let handle = cx.add_window(|_, _| Fixture {
        width: 320.0,
        bounds: bounds.clone(),
    });
    let mut first_left = None;
    for width in [320.0, 680.0, 1000.0, 320.0] {
        handle
            .update(cx, |fixture, _, cx| {
                fixture.width = width;
                cx.notify();
            })
            .unwrap();
        bounds.borrow_mut().clear();
        cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        let measured = bounds.borrow();
        let pair = &measured[measured.len() - 3..];
        assert_eq!(pair[0].left(), *first_left.get_or_insert(pair[0].left()));
        assert!((f32::from(pair[1].left() - pair[0].right()) - 4.0).abs() < 0.1);
        assert_eq!(pair[0].top(), pair[1].top());
        assert!((f32::from(pair[2].right() - pair[0].left()) - (width - 20.0)).abs() < 0.1);
    }
}

#[gpui::test]
fn picker_completion_and_dismissal_have_distinct_focus_behavior(cx: &mut gpui::TestAppContext) {
    use std::cell::Cell;
    use std::rc::Rc;
    cx.update(|cx| cx.set_global(Theme::dark()));
    let handle = cx.add_window(|_, cx| {
        let state = cx.new(|_| AppState::new());
        Pickers::new(state, cx)
    });
    let returned = Rc::new(Cell::new(0));
    let observed = returned.clone();
    let _sub = cx.update(|cx| {
        cx.subscribe(
            &handle.entity(cx).unwrap(),
            move |_, _: &ReturnComposerFocus, _| observed.set(observed.get() + 1),
        )
    });
    handle
        .update(cx, |pickers, _, cx| {
            pickers.open.open(PickerKind::Checkout);
            pickers.pick_checkout(CheckoutKind::Local, cx);
        })
        .unwrap();
    assert_eq!(returned.get(), 1);
    handle
        .update(cx, |pickers, _, cx| {
            pickers.open.open(PickerKind::HarnessModel);
            pickers.pick_model("test-model".into(), cx);
            assert!(
                pickers.is_open(),
                "model options remain available after a selection"
            );
            pickers.dismiss(cx);
        })
        .unwrap();
    assert_eq!(
        returned.get(),
        1,
        "outside clicks must not request composer focus"
    );
    handle
        .update(cx, |pickers, window, cx| {
            pickers.open.open(PickerKind::Checkout);
            pickers.open.note_trigger_press();
            pickers.dismiss(cx); // Capture closes before the trigger's click.
            pickers.toggle(PickerKind::Checkout, window, cx);
        })
        .unwrap();
    assert_eq!(
        returned.get(),
        2,
        "closing via the trigger returns to the composer"
    );
}

#[gpui::test]
fn projectless_picker_clears_checkout_and_supports_keyboard_selection(
    cx: &mut gpui::TestAppContext,
) {
    let state = cx.new(|_| {
        let mut state = AppState::new();
        state.local_device_id = Some("local".into());
        state.apply_spaces(vec![Space {
            id: "repo".into(),
            device_id: "local".into(),
            path: "/repo".into(),
            name: None,
            git_detected: true,
            git_checked_at: None,
            checkout_id: None,
            created_at: chrono::Utc::now(),
        }]);
        state
    });
    let pickers = cx.new(|cx| Pickers::new(state.clone(), cx));
    pickers.update(cx, |pickers, cx| {
        pickers.config.branch = Some("old-branch".into());
        pickers.config.checkout = CheckoutKind::NewWorktree;
        pickers.open.open(PickerKind::Space);
        pickers.active = 1; // project row followed by the projectless row
        pickers.on_search_submit(cx);
    });
    cx.run_until_parked();
    pickers.update(cx, |pickers, cx| {
        assert!(pickers.config.branch.is_none());
        assert_eq!(pickers.config.checkout, CheckoutKind::default());
        assert!(pickers.defaults.no_project);
        assert!(pickers.state.read(cx).auto_selected);
        assert!(pickers.defaults.project.is_none());
        assert_eq!(pickers.selected_space_index(cx), 1);
    });
    state.update(cx, |state, cx| state.select_device("remote".into(), cx));
    cx.run_until_parked();
    pickers.update(cx, |pickers, cx| {
        assert_eq!(pickers.space_target(cx).as_deref(), Some("remote"));
        assert_eq!(pickers.selected_space_index(cx), 0); // empty device
        assert!(pickers.target_generation >= 2);
    });
}

fn bare_model(id: &str, label: &str) -> Model {
    Model {
        id: id.into(),
        label: label.into(),
        description: None,
        reasoning_levels: Vec::new(),
        options: Vec::new(),
    }
}

#[gpui::test]
fn nested_model_settings_navigate_and_preserve_independent_choices(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        cx.set_global(Theme::dark());
        crate::composer::init(cx, Default::default());
    });
    let handle = cx.add_window(|_, cx| {
        let state = cx.new(|_| AppState::new());
        Pickers::new(state, cx)
    });
    handle
        .update(cx, |pickers, window, cx| {
            let mut model = bare_model("opus", "Opus");
            model.reasoning_levels = vec![ReasoningLevel::Low, ReasoningLevel::High];
            model.options = ["contextWindow", "serviceTier"]
                .map(|id| ModelOption {
                    id: id.into(),
                    label: id.into(),
                    default_choice: "standard".into(),
                    choices: ["standard", "extended"]
                        .map(|id| ModelOptionChoice {
                            id: id.into(),
                            label: id.into(),
                        })
                        .into(),
                })
                .into();
            pickers.config.harness = Some(HarnessId::ClaudeCode);
            pickers.harnesses =
                Loadable::Ready(vec![descriptor(HarnessId::ClaudeCode, "Claude Code")]);
            pickers.models.insert(
                HarnessId::ClaudeCode,
                Loadable::Ready(vec![model, bare_model("haiku", "Haiku")]),
            );
            pickers.pick_model("opus".into(), cx);
            pickers.open.open(PickerKind::HarnessModel);
            window.focus(&pickers.focus, cx);
            assert_eq!(pickers.setting_groups(cx).len(), 3);
            let key = |key: &str| KeyDownEvent {
                keystroke: gpui::Keystroke::parse(key).unwrap(),
                is_held: false,
                prefer_character_input: false,
            };
            pickers.active = pickers.model_rows_len(cx);
            pickers.on_key_down(&key("right"), window, cx);
            assert_eq!(pickers.setting_menu, Some(ModelSetting::Reasoning));
            pickers.on_key_down(&key("up"), window, cx);
            pickers.on_key_down(&key("enter"), window, cx);
            assert_eq!(pickers.effective_reasoning(cx), Some(ReasoningLevel::Low));
            assert!(pickers.setting_menu.is_none());
            assert!(pickers.is_open());
            for id in ["contextWindow", "serviceTier"] {
                pickers.open_setting(ModelSetting::Option(id.into()), cx);
                assert_eq!(pickers.setting_active, 0);
                pickers.on_key_down(&key("down"), window, cx);
                pickers.on_key_down(&key("enter"), window, cx);
                assert_eq!(pickers.resolved(cx).model_options[id], "extended");
            }
            // Returning to one setting keeps its choice, and restoring its
            // default does not reset a sibling option or close the picker.
            pickers.open_setting(ModelSetting::Option("contextWindow".into()), cx);
            assert_eq!(pickers.setting_active, 1);
            pickers.on_key_down(&key("up"), window, cx);
            pickers.on_key_down(&key("enter"), window, cx);
            assert!(
                !pickers
                    .resolved(cx)
                    .model_options
                    .contains_key("contextWindow")
            );
            assert_eq!(
                pickers.resolved(cx).model_options["serviceTier"],
                "extended"
            );
            pickers.open_setting(ModelSetting::Reasoning, cx);
            pickers.on_key_down(&key("escape"), window, cx);
            assert!(pickers.is_open());
            assert!(pickers.setting_menu.is_none());
            pickers.open_setting(ModelSetting::Reasoning, cx);
            pickers.pick_model("haiku".into(), cx);
            assert!(pickers.setting_menu.is_none());
            assert!(pickers.setting_groups(cx).is_empty());
        })
        .unwrap();
    for setting in [
        ModelSetting::Reasoning,
        ModelSetting::Option("contextWindow".into()),
        ModelSetting::Option("serviceTier".into()),
    ] {
        handle
            .update(cx, |pickers, _, cx| {
                pickers.pick_model("opus".into(), cx);
                pickers.open_setting(setting, cx);
            })
            .unwrap();
        cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
    }
    handle
        .update(cx, |pickers, window, cx| {
            pickers.setting_menu = None;
            pickers.active = 0;
            window.focus(&pickers.search.read(cx).focus_handle(cx), cx);
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
        .unwrap();
    cx.simulate_keystrokes(handle.into(), "down down right");
    handle
        .read_with(cx, |pickers, _| {
            assert_eq!(pickers.setting_menu, Some(ModelSetting::Reasoning))
        })
        .unwrap();
    cx.simulate_keystrokes(handle.into(), "down enter");
    handle
        .read_with(cx, |pickers, cx| {
            assert_eq!(pickers.effective_reasoning(cx), Some(ReasoningLevel::High));
            assert!(pickers.setting_menu.is_none());
            assert!(pickers.is_open());
        })
        .unwrap();
}

#[gpui::test]
fn nested_model_menu_mouse_paths_work_on_both_sides(cx: &mut gpui::TestAppContext) {
    use std::{cell::Cell, rc::Rc};
    struct MouseFixture {
        pickers: Entity<Pickers>,
        on_left: bool,
        bounds: Rc<Cell<gpui::Bounds<gpui::Pixels>>>,
    }
    impl Render for MouseFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let measured = self.bounds.clone();
            let menu = self.pickers.update(cx, |pickers, cx| {
                let content = pickers.render_harness_model_popover(cx);
                pickers.popover_frame_flush(304.0, content, cx)
            });
            div().size_full().relative().child(
                div()
                    .absolute()
                    .top(px(100.0))
                    .w(px(304.0))
                    .when(self.on_left, |el| el.right(px(32.0)))
                    .when(!self.on_left, |el| el.left(px(32.0)))
                    .child(menu)
                    .child(
                        gpui::canvas(move |bounds, _, _| measured.set(bounds), |_, _, _, _| {})
                            .absolute()
                            .inset_0(),
                    ),
            )
        }
    }
    cx.update(|cx| cx.set_global(Theme::dark()));
    for on_left in [false, true] {
        let measured = Rc::new(Cell::new(gpui::Bounds::default()));
        let handle = cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            let pickers = cx.new(|cx| Pickers::new(state, cx));
            pickers.update(cx, |pickers, cx| {
                let mut model = bare_model("test", "Test model");
                model.reasoning_levels = vec![ReasoningLevel::Low, ReasoningLevel::High];
                model.options.push(ModelOption {
                    id: "contextWindow".into(),
                    label: "Context window".into(),
                    choices: ["200k", "1m"]
                        .map(|id| ModelOptionChoice {
                            id: id.into(),
                            label: id.into(),
                        })
                        .into(),
                    default_choice: "200k".into(),
                });
                pickers.config.harness = Some(HarnessId::ClaudeCode);
                pickers.harnesses =
                    Loadable::Ready(vec![descriptor(HarnessId::ClaudeCode, "Claude Code")]);
                pickers
                    .models
                    .insert(HarnessId::ClaudeCode, Loadable::Ready(vec![model]));
                pickers.open.open(PickerKind::HarnessModel);
                pickers.pick_model("test".into(), cx);
            });
            MouseFixture {
                pickers,
                on_left,
                bounds: measured.clone(),
            }
        });
        let pickers = handle
            .read_with(cx, |fixture, _| fixture.pickers.clone())
            .unwrap();
        cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        let parent = measured.get();
        let trigger = gpui::point(
            parent.center().x,
            parent.bottom() - px(popover::CARD_INSET + 30.0 + popover::MENU_GAP + 15.0),
        );
        let click = |window: &mut Window, cx: &mut App, position| {
            window.dispatch_event(
                gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                    position,
                    ..Default::default()
                }),
                cx,
            );
            window.dispatch_event(
                gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                    button: gpui::MouseButton::Left,
                    position,
                    click_count: 1,
                    ..Default::default()
                }),
                cx,
            );
            window.dispatch_event(
                gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                    button: gpui::MouseButton::Left,
                    position,
                    click_count: 1,
                    ..Default::default()
                }),
                cx,
            );
        };
        let hover = |window: &mut Window, cx: &mut App, position| {
            window.dispatch_event(
                gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                    position,
                    ..Default::default()
                }),
                cx,
            );
            window.draw(cx).clear();
            window.draw(cx).clear();
        };
        cx.update_window(handle.into(), |_, window, cx| {
            hover(window, cx, trigger);
            window.draw(cx).clear();
            window.draw(cx).clear();
        })
        .unwrap();
        let submenu = pickers.read_with(cx, |pickers, _| {
            assert_eq!(pickers.setting_menu, Some(ModelSetting::Reasoning));
            assert_eq!(pickers.setting_on_left, on_left);
            pickers
                .setting_bounds
                .expect("submenu measured after opening")
        });
        if on_left {
            assert!(submenu.right() < parent.left());
        } else {
            assert!(submenu.left() > parent.right());
        }
        // Clicking the hovered trigger dismisses its child and keeps it closed.
        cx.update_window(handle.into(), |_, window, cx| {
            click(window, cx, trigger);
            window.draw(cx).clear();
        })
        .unwrap();
        pickers.read_with(cx, |pickers, _| {
            assert!(
                pickers.setting_menu.is_none(),
                "trigger click dismisses the hovered submenu"
            );
            assert!(pickers.is_open());
        });
        cx.update_window(handle.into(), |_, window, cx| {
            // Moving inside the same hovered row must not reopen it.
            hover(window, cx, trigger + gpui::point(px(2.0), px(0.0)));
            assert!(pickers.read(cx).setting_menu.is_none());
            hover(window, cx, gpui::point(px(8.0), px(8.0)));
            hover(window, cx, trigger);
        })
        .unwrap();
        // Cross the neighboring trigger diagonally toward the open child.
        // It must not steal the menu while the pointer is inside the cone.
        let diagonal = gpui::point(
            if on_left {
                parent.left() + px(8.0)
            } else {
                parent.right() - px(8.0)
            },
            trigger.y + px(32.0),
        );
        cx.update_window(handle.into(), |_, window, cx| {
            hover(window, cx, diagonal);
            assert_eq!(pickers.read(cx).setting_menu, Some(ModelSetting::Reasoning));
            assert!(pickers.read(cx).setting_hover_pending.is_some());
        })
        .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(200));
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            hover(
                window,
                cx,
                diagonal + gpui::point(px(if on_left { -3.0 } else { 3.0 }), px(0.0)),
            );
        })
        .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(200));
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            assert_eq!(
                pickers.read(cx).setting_menu,
                Some(ModelSetting::Reasoning),
                "forward progress renews grace"
            );
            hover(window, cx, submenu.center());
        })
        .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(400));
        cx.run_until_parked();
        pickers.read_with(cx, |pickers, _| {
            assert_eq!(pickers.setting_menu, Some(ModelSetting::Reasoning));
            assert!(pickers.setting_hover_pending.is_none());
        });
        // Resting on the sibling expresses intent to switch after grace.
        cx.update_window(handle.into(), |_, window, cx| {
            hover(window, cx, trigger);
            hover(window, cx, diagonal);
        })
        .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(350));
        cx.run_until_parked();
        pickers.read_with(cx, |pickers, _| {
            assert_eq!(
                pickers.setting_menu,
                Some(ModelSetting::Option("contextWindow".into()))
            );
        });
        // Moving away from the cone switches immediately. A click during
        // the grace period dismisses instead, with no delayed reopening.
        cx.update_window(handle.into(), |_, window, cx| {
            hover(window, cx, trigger);
            hover(window, cx, diagonal);
            hover(window, cx, trigger + gpui::point(px(0.0), px(32.0)));
            assert_eq!(
                pickers.read(cx).setting_menu,
                Some(ModelSetting::Option("contextWindow".into()))
            );
            hover(window, cx, trigger);
            hover(window, cx, diagonal);
            click(window, cx, diagonal);
            window.draw(cx).clear();
        })
        .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(400));
        cx.run_until_parked();
        pickers.read_with(cx, |pickers, _| {
            assert!(pickers.setting_menu.is_none());
            assert!(pickers.setting_hover_pending.is_none());
            assert!(pickers.is_open());
        });
        cx.update_window(handle.into(), |_, window, cx| hover(window, cx, trigger))
            .unwrap();
        let gap_x = if on_left {
            (submenu.right() + parent.left()) / 2.0
        } else {
            (parent.right() + submenu.left()) / 2.0
        };
        // Leaving the safe corridor closes the child and clears its
        // trigger selection, while preserving the parent picker.
        for outside in [
            gpui::point(gap_x, submenu.bottom() + px(20.0)),
            gpui::point(parent.center().x, parent.top() + px(60.0)),
            gpui::point(px(8.0), px(8.0)),
        ] {
            cx.update_window(handle.into(), |_, window, cx| {
                hover(window, cx, outside);
                assert!(pickers.read(cx).setting_menu.is_none());
                assert_eq!(pickers.read(cx).active, 0);
                assert!(pickers.read(cx).is_open());
                hover(window, cx, trigger);
                assert_eq!(pickers.read(cx).setting_menu, Some(ModelSetting::Reasoning));
            })
            .unwrap();
        }
        // Moving through the gap into the child remains safe.
        let target = gpui::point(
            submenu.left() + px(30.0),
            submenu.bottom() - px(popover::CARD_INSET + 30.0 + popover::MENU_GAP + 15.0),
        );
        for position in [gpui::point(gap_x, trigger.y + px(16.0)), target] {
            cx.update_window(handle.into(), |_, window, cx| {
                window.dispatch_event(
                    gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                        position,
                        ..Default::default()
                    }),
                    cx,
                );
                window.draw(cx).clear();
            })
            .unwrap();
            cx.executor().advance_clock(Duration::from_secs(2));
            cx.run_until_parked();
            pickers.read_with(cx, |pickers, _| {
                assert!(pickers.is_open());
                assert_eq!(pickers.setting_menu, Some(ModelSetting::Reasoning));
            });
        }
        cx.update_window(handle.into(), |_, window, cx| click(window, cx, target))
            .unwrap();
        pickers.read_with(cx, |pickers, cx| {
            assert_eq!(pickers.effective_reasoning(cx), Some(ReasoningLevel::Low));
            assert!(pickers.setting_menu.is_none());
            assert!(pickers.is_open());
        });
        // Hover switches directly between sibling triggers without clicking.
        cx.update_window(handle.into(), |_, window, cx| {
            window.draw(cx).clear();
            hover(window, cx, trigger);
            hover(window, cx, trigger + gpui::point(px(0.0), px(32.0)));
            window.draw(cx).clear();
            window.draw(cx).clear();
        })
        .unwrap();
        pickers.read_with(cx, |pickers, _| {
            assert_eq!(
                pickers.setting_menu,
                Some(ModelSetting::Option("contextWindow".into()))
            );
            assert!(pickers.is_open());
        });
        // Clicking the parent's search closes only the child and focuses
        // the input on that same click, rather than swallowing the press.
        cx.update_window(handle.into(), |_, window, cx| {
            click(
                window,
                cx,
                gpui::point(parent.center().x, parent.top() + px(60.0)),
            );
            pickers.read_with(cx, |pickers, cx| {
                assert!(pickers.setting_menu.is_none());
                assert!(pickers.is_open());
                assert!(pickers.search.read(cx).focus_handle(cx).is_focused(window));
            });
            window.draw(cx).clear();
            hover(window, cx, trigger);
            window.draw(cx).clear();
            window.draw(cx).clear();
        })
        .unwrap();
        pickers.read_with(cx, |pickers, _| assert!(pickers.setting_menu.is_some()));
        cx.update_window(handle.into(), |_, window, cx| {
            click(window, cx, gpui::point(px(8.0), px(8.0)))
        })
        .unwrap();
        pickers.read_with(cx, |pickers, _| assert!(!pickers.is_open()));
    }
}

#[gpui::test]
fn remembered_options_stay_valid_for_the_sent_model_without_a_catalog(
    cx: &mut gpui::TestAppContext,
) {
    let one_m = serde_json::Value::String("1m".into());
    let mut opus = bare_model("opus", "Opus");
    opus.options.push(ModelOption {
        id: "contextWindow".into(),
        label: "Context window".into(),
        choices: ["200k", "1m"]
            .map(|id| ModelOptionChoice {
                id: id.into(),
                label: id.into(),
            })
            .into(),
        default_choice: "200k".into(),
    });
    let state = cx.new(|_| AppState::new());
    let pickers = cx.new(|cx| Pickers::new(state, cx));
    pickers.update(cx, |pickers, cx| {
        pickers.defaults.harness = Some(HarnessId::ClaudeCode);
        pickers.models.insert(
            HarnessId::ClaudeCode,
            Loadable::Ready(vec![opus.clone(), bare_model("haiku", "Haiku")]),
        );
        pickers.pick_model("opus".into(), cx);
        pickers.pick_option("contextWindow".into(), "1m".into(), false, cx);
        let resolved = pickers.resolved(cx);
        assert_eq!(resolved.model.as_deref(), Some("opus"));
        assert_eq!(resolved.model_options.get("contextWindow"), Some(&one_m));

        pickers.pick_model("haiku".into(), cx);
        assert!(pickers.resolved(cx).model_options.is_empty());

        // Restart (draft cleared) and send before the catalog is usable.
        for catalog in [
            Loadable::Idle,
            Loadable::Loading,
            Loadable::Error("down".into()),
        ] {
            pickers.config = DraftConfig::default();
            pickers.models.insert(HarnessId::ClaudeCode, catalog);
            let resolved = pickers.resolved(cx);
            assert_eq!(resolved.model.as_deref(), Some("haiku"));
            assert!(
                resolved.model_options.is_empty(),
                "Opus's 1M pick must not ride along with Haiku"
            );
        }

        // The Opus memory itself survives for when Opus is picked again.
        pickers.pick_model("opus".into(), cx);
        let resolved = pickers.resolved(cx);
        assert_eq!(resolved.model.as_deref(), Some("opus"));
        assert_eq!(resolved.model_options.get("contextWindow"), Some(&one_m));
    });
}

fn descriptor(id: HarnessId, name: &str) -> HarnessDescriptor {
    HarnessDescriptor {
        id,
        name: name.into(),
        installed: true,
        enabled: Some(true),
        reasoning_levels: Vec::new(),
        steering_mode: zeron_proto::SteeringMode::StepBoundary,
        supports_steering: false,
    }
}

#[test]
fn tab_search_never_leaves_the_viewed_harness() {
    let descriptors = vec![
        descriptor(HarnessId::ClaudeCode, "Claude Code"),
        descriptor(HarnessId::Codex, "Codex"),
    ];
    let claude = vec![bare_model("fable-5", "Fable 5")];
    let codex = vec![bare_model("gpt-fable", "Fable (Codex)")];
    let models_for = |harness: HarnessId| -> Option<&[Model]> {
        match harness {
            HarnessId::ClaudeCode => Some(claude.as_slice()),
            HarnessId::Codex => Some(codex.as_slice()),
            _ => None,
        }
    };
    // Both catalogs match "fable", but the viewed tab is Claude — the
    // Codex hit must not appear.
    let rows = scoped_model_rows(
        "fable",
        ModelRail::Harness,
        Some(HarnessId::ClaudeCode),
        &descriptors,
        models_for,
        |_, _| false,
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].harness, HarnessId::ClaudeCode);
    assert_eq!(rows[0].model.id, "fable-5");
}

#[test]
fn favorites_tab_search_ranks_only_starred_rows() {
    let descriptors = vec![
        descriptor(HarnessId::ClaudeCode, "Claude Code"),
        descriptor(HarnessId::Codex, "Codex"),
    ];
    let claude = vec![bare_model("fable-5", "Fable 5")];
    let codex = vec![bare_model("gpt-fable", "Fable (Codex)")];
    let models_for = |harness: HarnessId| -> Option<&[Model]> {
        match harness {
            HarnessId::ClaudeCode => Some(claude.as_slice()),
            HarnessId::Codex => Some(codex.as_slice()),
            _ => None,
        }
    };
    let starred =
        |harness: HarnessId, model: &str| harness == HarnessId::Codex && model == "gpt-fable";
    let rows = scoped_model_rows(
        "fable",
        ModelRail::Favorites,
        Some(HarnessId::ClaudeCode),
        &descriptors,
        models_for,
        starred,
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].harness, HarnessId::Codex);

    // Empty query on the favorites tab: the starred set, nothing else.
    let rows = scoped_model_rows(
        "",
        ModelRail::Favorites,
        Some(HarnessId::ClaudeCode),
        &descriptors,
        models_for,
        starred,
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].model.id, "gpt-fable");
}

#[test]
fn harness_tab_lists_stars_first_and_description_still_matches() {
    let descriptors = vec![descriptor(HarnessId::Opencode, "opencode")];
    let mut provider_a = bare_model("glm-5.2-a", "GLM-5.2");
    provider_a.description = Some("Anthropic".into());
    let mut provider_b = bare_model("glm-5.2-b", "GLM-5.2");
    provider_b.description = Some("Baseten".into());
    let models = vec![provider_a, provider_b];
    let models_for = |harness: HarnessId| -> Option<&[Model]> {
        (harness == HarnessId::Opencode).then_some(models.as_slice())
    };
    let starred = |harness: HarnessId, model: &str| {
        harness == HarnessId::Opencode && model == "glm-5.2-b"
    };
    // No query: catalog order with the star floated to the top.
    let rows = scoped_model_rows(
        "",
        ModelRail::Harness,
        Some(HarnessId::Opencode),
        &descriptors,
        models_for,
        starred,
    );
    assert_eq!(rows[0].model.id, "glm-5.2-b");
    assert_eq!(rows[1].model.id, "glm-5.2-a");
    // Provider attribution stays searchable inside the tab.
    let rows = scoped_model_rows(
        "baseten",
        ModelRail::Harness,
        Some(HarnessId::Opencode),
        &descriptors,
        models_for,
        starred,
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].model.id, "glm-5.2-b");
}

#[test]
fn normalize_drops_default_alias_and_folds_orphan_1m_rows() {
    // The shape an OLDER engine serves: a `default` alias row plus
    // 1M-pinned variants with no bare base. A non-claude harness keeps
    // wire labels (no curated catalog to borrow from).
    let models = normalize_model_rows(
        HarnessId::Codex,
        vec![
            bare_model("default", "Default (recommended)"),
            bare_model("titan[1m]", "Titan (1M context)"),
            bare_model("gpt-x-9[1m]", "GPT X-9"),
            bare_model("nano", "Nano"),
        ],
    );
    assert_eq!(
        models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        vec!["titan", "gpt-x-9", "nano"]
    );
    assert_eq!(models[0].label, "Titan");
    assert_eq!(models[1].label, "GPT X-9");
    // Folded rows pin the Context Window trait to 1M.
    assert!(
        models[0]
            .options
            .iter()
            .any(|o| o.id == "contextWindow" && o.default_choice == "1m")
    );
    assert!(models[2].options.is_empty());

    // A `default`-only list survives (nothing real to prefer).
    let only_default =
        normalize_model_rows(HarnessId::Codex, vec![bare_model("default", "Default")]);
    assert_eq!(only_default.len(), 1);

    // A base-plus-variant pair (already folded by a NEWER engine — the
    // variant never reaches us; belt-and-braces if it does): variant
    // drops, base is untouched.
    let paired = normalize_model_rows(
        HarnessId::Codex,
        vec![
            bare_model("titan-5", "Titan 5"),
            bare_model("titan-5[1m]", "Titan 5 (1M)"),
        ],
    );
    assert_eq!(paired.len(), 1);
    assert_eq!(paired[0].id, "titan-5");

    // Idempotent over a clean list.
    let clean = vec![bare_model("titan-5", "Titan 5")];
    assert_eq!(normalize_model_rows(HarnessId::Codex, clean.clone()), clean);
}

#[test]
fn normalize_gives_claude_rows_their_versioned_catalog_labels() {
    // The real prod shape: alias values with terse names. Claude rows
    // adopt the curated labels so the version number always shows
    // (user request), exact ids included; foreign ids pass through.
    let models = normalize_model_rows(
        HarnessId::ClaudeCode,
        vec![
            bare_model("default", "Default (recommended)"),
            bare_model("opus[1m]", "Opus (1M context)"),
            bare_model("claude-fable-5[1m]", "Fable"),
            bare_model("sonnet", "Sonnet"),
            bare_model("haiku", "Haiku"),
            bare_model("claude-nova-1", "Nova 1"),
        ],
    );
    assert_eq!(
        models.iter().map(|m| m.label.as_str()).collect::<Vec<_>>(),
        vec!["Opus 5", "Fable 5", "Sonnet 5", "Haiku 4.5", "Nova 1"]
    );
    assert_eq!(
        models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        vec!["opus", "claude-fable-5", "sonnet", "haiku", "claude-nova-1"]
    );
}

#[test]
fn traits_summary_formats_non_defaults() {
    let model = Model {
        id: "opus".into(),
        label: "Opus".into(),
        description: None,
        reasoning_levels: vec![ReasoningLevel::Medium, ReasoningLevel::High],
        options: vec![
            ModelOption {
                id: "context".into(),
                label: "Context window".into(),
                choices: vec![
                    ModelOptionChoice {
                        id: "standard".into(),
                        label: "Standard".into(),
                    },
                    ModelOptionChoice {
                        id: "1m".into(),
                        label: "1M".into(),
                    },
                ],
                default_choice: "standard".into(),
            },
            ModelOption {
                id: "speed".into(),
                label: "Speed".into(),
                choices: vec![
                    ModelOptionChoice {
                        id: "normal".into(),
                        label: "Normal".into(),
                    },
                    ModelOptionChoice {
                        id: "fast".into(),
                        label: "Fast".into(),
                    },
                ],
                default_choice: "normal".into(),
            },
        ],
    };
    let mut selections = serde_json::Map::new();
    selections.insert("context".into(), serde_json::Value::String("1m".into()));
    selections.insert("speed".into(), serde_json::Value::String("fast".into()));
    assert_eq!(
        traits_summary(Some(&model), Some(ReasoningLevel::High), &selections),
        Some("High · 1M · Fast".to_string())
    );
    // All defaults: the effective choices still read on the trigger.
    assert_eq!(
        traits_summary(Some(&model), None, &serde_json::Map::new()),
        Some("Standard · Normal".to_string())
    );
    // A saved choice the option no longer offers falls back to the default
    // label rather than vanishing or echoing a stale id.
    let mut stale = serde_json::Map::new();
    stale.insert(
        "speed".into(),
        serde_json::Value::String("ludicrous".into()),
    );
    assert_eq!(
        traits_summary(Some(&model), None, &stale),
        Some("Standard · Normal".to_string())
    );
    // Remembered picks drop what the model doesn't offer before sending.
    let mut remembered = selections.clone();
    remembered.insert(
        "speed".into(),
        serde_json::Value::String("ludicrous".into()),
    );
    remembered.insert("fastMode".into(), serde_json::Value::String("on".into()));
    let mut want = serde_json::Map::new();
    want.insert("context".into(), serde_json::Value::String("1m".into()));
    assert_eq!(offered_options(&model, remembered), want);
    // Reasoning shows without a model too.
    assert_eq!(
        traits_summary(
            None,
            Some(ReasoningLevel::Ultrathink),
            &serde_json::Map::new()
        ),
        Some("Ultrathink".to_string())
    );
    // Nothing to describe → "Traits" fallback upstream.
    assert_eq!(traits_summary(None, None, &serde_json::Map::new()), None);

    // Customized (bright trigger) only when something departs from its
    // default: default-choice selections and the default reasoning level
    // don't count; stale ids don't either.
    let ladder = model.reasoning_levels.clone();
    assert!(traits_customized(
        Some(&model),
        Some(ReasoningLevel::High),
        &ladder,
        &selections
    ));
    assert!(!traits_customized(
        Some(&model),
        default_reasoning(&ladder),
        &ladder,
        &serde_json::Map::new()
    ));
    let mut defaults = serde_json::Map::new();
    defaults.insert("speed".into(), serde_json::Value::String("normal".into()));
    assert!(!traits_customized(
        Some(&model),
        default_reasoning(&ladder),
        &ladder,
        &defaults
    ));
    assert!(!traits_customized(
        Some(&model),
        default_reasoning(&ladder),
        &ladder,
        &stale
    ));
    assert!(traits_customized(
        Some(&model),
        Some(ReasoningLevel::Medium),
        &ladder,
        &serde_json::Map::new()
    ));
}

#[test]
fn folder_paths_and_breadcrumbs() {
    assert_eq!(parent_path("/home/w/dev"), Some("/home/w".to_string()));
    assert_eq!(parent_path("/home"), Some("/".to_string()));
    assert_eq!(parent_path("/home/"), Some("/".to_string()));
    assert_eq!(parent_path("/"), None);
    assert_eq!(parent_path(""), None);
    assert_eq!(child_path("/home", "w"), "/home/w");
    assert_eq!(child_path("/", "home"), "/home");
    let crumbs = breadcrumbs("/home/w/dev");
    let labels: Vec<&str> = crumbs.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(labels, ["/", "home", "w", "dev"]);
    assert_eq!(crumbs[2].1, "/home/w");
    assert_eq!(breadcrumbs("/").len(), 1);
}

#[test]
fn completion_prefix_lengths() {
    // Case-insensitive; the length indexes into the NAME's bytes.
    assert_eq!(completion_prefix_len("Documents", "doc"), Some(3));
    assert_eq!(&"Documents"[3..], "uments");
    assert_eq!(completion_prefix_len("zeron", "zeron"), Some(5));
    assert_eq!(completion_prefix_len("zeron", ""), Some(0));
    assert_eq!(completion_prefix_len("zeron", "dev"), None);
    // Longer than the name → not a prefix.
    assert_eq!(completion_prefix_len("dev", "devel"), None);
    // Multibyte names slice on a char boundary.
    assert_eq!(completion_prefix_len("héllo", "hé"), Some(3));
    assert_eq!(&"héllo"[3..], "llo");
}

#[test]
fn segment_target_resolution() {
    let names = ["github", "GitHub", "worktree"];
    // Exact casing beats the earlier case-insensitive sibling…
    assert_eq!(segment_target(&names, "GitHub"), Some(1));
    assert_eq!(segment_target(&names, "github"), Some(0));
    // …but with no exact-cased hit, case-insensitive exact still lands.
    assert_eq!(segment_target(&names, "WORKTREE"), Some(2));
    // Unique prefix descends; an ambiguous one keeps the slash honest.
    assert_eq!(segment_target(&names, "work"), Some(2));
    assert_eq!(segment_target(&names, "g"), None);
    assert_eq!(segment_target(&names, "x"), None);
}

#[test]
fn typed_path_target_expands_absolute_and_home_paths() {
    let home = Some("/home/wing");
    assert_eq!(typed_path_target("/disk2/", home), Some("/disk2".into()));
    assert_eq!(
        typed_path_target("/disk2/projects", home),
        Some("/disk2/projects".into())
    );
    assert_eq!(typed_path_target("/", home), Some("/".into()));
    assert_eq!(typed_path_target("~", home), Some("/home/wing".into()));
    assert_eq!(typed_path_target("~/", home), Some("/home/wing".into()));
    assert_eq!(
        typed_path_target("~/github/", home),
        Some("/home/wing/github".into())
    );
    // `~x` is a folder name; relative queries are searches, not paths.
    assert_eq!(typed_path_target("~x", home), None);
    assert_eq!(typed_path_target("src", home), None);
    // `~` can't expand before the device's home is known.
    assert_eq!(typed_path_target("~/github", None), None);
    assert_eq!(typed_path_target("/disk2", None), Some("/disk2".into()));
}

#[test]
fn browser_navigation_reducer() {
    let listing = FolderListing {
        path: "/home/w".into(),
        entries: vec![
            FolderEntry {
                name: "notes.txt".into(),
                is_dir: false,
                is_repo: false,
            },
            FolderEntry {
                name: "dev".into(),
                is_dir: true,
                is_repo: false,
            },
            FolderEntry {
                name: "zeron".into(),
                is_dir: true,
                is_repo: true,
            },
        ],
        truncated: false,
    };
    // Files never show as rows.
    assert_eq!(browser_rows(&listing).len(), 2);
    assert_eq!(browser_rows(&listing)[1].name, "zeron");
}

#[test]
fn resolved_chat_config_requires_harness() {
    let mut resolved = ResolvedRunConfig::default();
    assert!(resolved.chat_config().is_none());
    resolved.harness = Some(HarnessId::ClaudeCode);
    resolved.model = Some("opus".into());
    resolved.reasoning = Some(ReasoningLevel::High);
    let config = resolved.chat_config().expect("harness set");
    assert_eq!(config.harness, HarnessId::ClaudeCode);
    assert_eq!(config.model.as_deref(), Some("opus"));
    assert_eq!(config.sandbox, SandboxLevel::WorkspaceWrite);
}

#[test]
fn default_model_is_first_catalog_row() {
    let models = vec![
        Model {
            id: "flagship".into(),
            label: "Flagship".into(),
            description: None,
            reasoning_levels: vec![],
            options: vec![],
        },
        Model {
            id: "fast".into(),
            label: "Fast".into(),
            description: None,
            reasoning_levels: vec![],
            options: vec![],
        },
    ];
    assert_eq!(default_model(&models).map(|m| &*m.id), Some("flagship"));
    assert!(default_model(&[]).is_none());
}

#[test]
fn default_reasoning_prefers_high_then_medium() {
    use ReasoningLevel::*;
    // Recommended default is High (user-corrected), even on full ladders.
    assert_eq!(
        default_reasoning(&[Low, Medium, High, XHigh, Max, Ultracode, Ultrathink]),
        Some(High)
    );
    assert_eq!(default_reasoning(&[Low, Medium, High, Max]), Some(High));
    // No High: Medium.
    assert_eq!(default_reasoning(&[Minimal, Low, Medium]), Some(Medium));
    // Neither offered: first entry.
    assert_eq!(default_reasoning(&[Minimal, Low]), Some(Minimal));
    // Ladder-less model (Haiku): no reasoning at all.
    assert_eq!(default_reasoning(&[]), None);
}

#[test]
fn clamp_reasoning_keeps_offered_levels_and_heals_foreign_ones() {
    use ReasoningLevel::*;
    let ladder = [Low, Medium, High, Max];
    // A pick the ladder offers survives.
    assert_eq!(clamp_reasoning(Some(Max), &ladder), Some(Max));
    // A remembered level the new model doesn't offer heals to its default.
    assert_eq!(clamp_reasoning(Some(XHigh), &ladder), Some(High));
    // No pick at all resolves to the concrete default too.
    assert_eq!(clamp_reasoning(None, &ladder), Some(High));
    assert_eq!(clamp_reasoning(Some(High), &[]), None);
}

#[test]
fn mock_harness_hidden_unless_alone() {
    let descriptor = |id: HarnessId, name: &str| HarnessDescriptor {
        id,
        name: name.into(),
        supports_steering: true,
        steering_mode: zeron_proto::SteeringMode::StepBoundary,
        reasoning_levels: vec![],
        installed: true,
        enabled: None,
    };
    let mixed = vec![
        descriptor(HarnessId::Mock, "Mock"),
        descriptor(HarnessId::ClaudeCode, "Claude Code"),
    ];
    // Env-independent core: mock hidden in production…
    let visible = visible_harnesses_impl(&mixed, false);
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].id, HarnessId::ClaudeCode);
    let only_mock = vec![descriptor(HarnessId::Mock, "Mock")];
    assert_eq!(visible_harnesses_impl(&only_mock, false).len(), 1);
    // …and opted back in by ZERON_HARNESS=mock (the e2e rig).
    assert_eq!(visible_harnesses_impl(&mixed, true).len(), 2);
    assert_eq!(visible_harnesses_impl(&mixed, true)[0].id, HarnessId::Mock);
}

#[test]
fn offered_harnesses_follow_the_catalog_enabled_flags() {
    let descriptor = |id: HarnessId, name: &str, enabled: Option<bool>| HarnessDescriptor {
        id,
        name: name.into(),
        supports_steering: true,
        steering_mode: zeron_proto::SteeringMode::StepBoundary,
        reasoning_levels: vec![],
        installed: true,
        enabled,
    };
    let catalog = |claude: Option<bool>, codex: Option<bool>, grok: Option<bool>| {
        vec![
            descriptor(HarnessId::Mock, "Mock", Some(false)),
            descriptor(HarnessId::ClaudeCode, "Claude Code", claude),
            descriptor(HarnessId::Codex, "Codex", codex),
            descriptor(HarnessId::Grok, "Grok", grok),
        ]
    };
    // A catalog from an engine predating the flag (all None) follows its
    // installed probes, so every detected real harness is offered.
    let offered = offered_harnesses_impl(&catalog(None, None, None), false);
    assert_eq!(
        offered.iter().map(|d| d.id).collect::<Vec<_>>(),
        vec![HarnessId::ClaudeCode, HarnessId::Codex, HarnessId::Grok]
    );
    // The device's flags win: Grok on, Codex off; catalog order holds.
    let offered = offered_harnesses_impl(&catalog(Some(true), Some(false), Some(true)), false);
    assert_eq!(
        offered.iter().map(|d| d.id).collect::<Vec<_>>(),
        vec![HarnessId::ClaudeCode, HarnessId::Grok]
    );
    // The dev-rig mock opt-in survives the enabled filter (and Grok's
    // unknown flag still resolves through its installed probe).
    let offered = offered_harnesses_impl(&catalog(Some(true), Some(false), None), true);
    assert_eq!(
        offered.iter().map(|d| d.id).collect::<Vec<_>>(),
        vec![HarnessId::Mock, HarnessId::ClaudeCode, HarnessId::Grok]
    );
    // Nothing enabled offers nothing — the composer renders the
    // no-agents empty state instead of resurrecting disabled agents.
    let offered =
        offered_harnesses_impl(&catalog(Some(false), Some(false), Some(false)), false);
    assert!(offered.is_empty());
    // So does a legacy catalog whose installed probes all failed: never
    // resurface unrunnable agents just to avoid an empty picker.
    let mut missing = catalog(None, None, None);
    missing.iter_mut().for_each(|d| d.installed = false);
    assert!(offered_harnesses_impl(&missing, false).is_empty());
}

#[test]
fn offered_harnesses_require_an_installed_cli() {
    let descriptor =
        |id: HarnessId, name: &str, enabled: Option<bool>, installed: bool| HarnessDescriptor {
            id,
            name: name.into(),
            supports_steering: true,
            steering_mode: zeron_proto::SteeringMode::StepBoundary,
            reasoning_levels: vec![],
            installed,
            enabled,
        };
    // Enabled-but-missing-CLI agents stay out of the rail; an installed
    // enabled one rides along. A live engine no longer stamps that
    // combination (enablement follows detection), but a catalog from an
    // older engine still can — the filter is the cross-version defense.
    let catalog = vec![
        descriptor(HarnessId::ClaudeCode, "Claude Code", Some(true), false),
        descriptor(HarnessId::Codex, "Codex", Some(true), false),
        descriptor(HarnessId::Grok, "Grok", Some(true), true),
    ];
    let offered = offered_harnesses_impl(&catalog, false);
    assert_eq!(
        offered.iter().map(|d| d.id).collect::<Vec<_>>(),
        vec![HarnessId::Grok]
    );
    // Nothing enabled AND installed: an empty offered set — the fresh
    // machine where the default-enabled Claude/Codex have no CLIs (#128).
    // No fallback: offering them again would only manufacture
    // NotInstalled errors at send; the composer shows the no-agents
    // state and blocks new sends instead.
    let catalog = vec![
        descriptor(HarnessId::ClaudeCode, "Claude Code", Some(true), false),
        descriptor(HarnessId::Codex, "Codex", Some(false), false),
        descriptor(HarnessId::Grok, "Grok", Some(false), true),
    ];
    let offered = offered_harnesses_impl(&catalog, false);
    assert!(offered.is_empty());
}
