//! Test fixtures and regression test suites for Shell.
//!
//! Native visual QA and browser regression fixture hooks are excluded from shipped builds.

#[allow(unused_imports)]
use super::*;

/// Native browser regression fixture hooks are excluded from shipped builds.
#[cfg(feature = "browser-fixture")]
impl Shell {
    pub fn fixture_focus_mounted(&self, window: &Window, cx: &App) -> bool {
        self.shortcut_focus.contains_focused(window, cx)
    }
    pub fn fixture_active_browser(
        &self,
        cx: &App,
    ) -> Option<(u64, Entity<crate::browser::BrowserSurface>)> {
        let RightSurface::Browser(id) = self.resolved_right_active(cx) else {
            return None;
        };
        self.browsers.get(&id).cloned().map(|browser| (id, browser))
    }
    pub fn fixture_open_browser(
        &mut self,
        url: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (u64, Entity<crate::browser::BrowserSurface>) {
        if !self.right_pane_open(cx) {
            self.toggle_right_pane(cx);
        }
        // Hosted Macs can expose only a 1024px desktop. Use the app's
        // normal collapsed-sidebar layout to keep both conversation and
        // preview readable in that real window.
        if f32::from(window.viewport_size().width) < 1200.0 {
            self.settings.sidebar_collapsed = true;
        }
        self.add_browser_surface(url, window, cx);
        (self.browser_seq, self.browsers[&self.browser_seq].clone())
    }
    pub fn fixture_select_browser(&mut self, id: u64, cx: &mut Context<Self>) {
        self.set_right_active(RightSurface::Browser(id), cx);
    }
    pub fn fixture_close_browser(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.close_right_surface(RightSurface::Browser(id), window, cx);
    }
    pub fn fixture_browser_menu(&mut self, open: bool, cx: &mut Context<Self>) {
        if open {
            self.right_plus.open(());
            cx.notify();
        } else {
            self.close_right_plus(cx);
        }
    }
    pub fn fixture_browser_menu_mounted(&self) -> bool {
        self.right_plus.get().is_some()
    }
    pub fn fixture_expand_browser(&mut self, cx: &mut Context<Self>) {
        self.toggle_right_pane_expand(cx);
    }
    pub fn fixture_blur_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.route = Route::Settings(SettingsSection::Devices);
        window.blur();
        cx.notify();
    }
    pub fn fixture_toggle_sidebar(&mut self, right: bool, cx: &mut Context<Self>) {
        if right {
            self.toggle_right_pane(cx);
        } else {
            self.toggle_sidebar(cx);
        }
    }
    pub fn fixture_resize_browser(&mut self, width: f32, cx: &mut Context<Self>) {
        self.settings.right_pane_width = width;
        cx.notify();
    }
}

#[cfg(test)]
mod right_tab_mouse_regressions {
    use super::*;
    use gpui::{AppContext, TestAppContext, VisualTestContext};

    // Render the production strip and use its real Shell callbacks, without
    // starting an engine or rendering the rest of the desktop application.
    struct TabHost {
        shell: Entity<Shell>,
        _data_dir: tempfile::TempDir,
    }

    impl Render for TabHost {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.shell.update(cx, |shell, cx| {
                let tabs = shell.render_right_tab_strip(cx);
                shell.titlebar_drag_region(
                    "right-tab-test-titlebar",
                    div().w(px(400.)).h(px(40.)).child(tabs),
                    cx,
                )
            })
        }
    }

    fn setup(cx: &mut TestAppContext) -> (Entity<Shell>, &mut VisualTestContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
        });
        let (host, cx) = cx.add_window_view(|_, cx| {
            let shell = cx.new(|cx| {
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
                shell.active_chat = "parent".into();
                for id in ["first", "second"] {
                    shell.add_subagent_surface("parent".into(), id.into(), id.into(), false, cx);
                }
                shell
            });
            TabHost {
                shell,
                _data_dir: dir,
            }
        });
        let shell = host.read_with(cx, |host, _| host.shell.clone());
        cx.update(|window, cx| window.draw(cx).clear());
        (shell, cx)
    }

    #[gpui::test]
    fn subagent_close_press_does_not_start_parent_drag(cx: &mut TestAppContext) {
        let (shell, cx) = setup(cx);
        let start = cx.debug_bounds("right-surface-close-0").unwrap().center();
        let end = start + gpui::point(px(6.), px(0.));
        cx.simulate_mouse_down(start, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(end, Some(MouseButton::Left), gpui::Modifiers::default());
        cx.update(|_, cx| assert!(!cx.has_active_drag(), "close press started a tab drag"));
        cx.simulate_mouse_up(end, MouseButton::Left, gpui::Modifiers::default());
        shell.read_with(cx, |shell, cx| {
            assert!(!shell.subagent_tabs.contains_key(&1));
            assert!(shell.subagent_tabs.contains_key(&2));
            assert_eq!(shell.resolved_right_active(cx), RightSurface::Subagent(2));
        });
    }

    #[gpui::test]
    fn tab_strip_scrolls_over_chips_inside_titlebar(cx: &mut TestAppContext) {
        let (shell, cx) = setup(cx);
        shell.update(cx, |shell, cx| {
            for id in ["third", "fourth", "fifth", "sixth"] {
                shell.add_subagent_surface("parent".into(), id.into(), id.into(), false, cx);
            }
        });
        cx.update(|window, cx| window.draw(cx).clear());
        let start = cx.debug_bounds("right-surface-tab-0").unwrap().center();
        cx.update(|window, cx| {
            window.dispatch_event(
                gpui::PlatformInput::ScrollWheel(gpui::ScrollWheelEvent {
                    position: start,
                    delta: gpui::ScrollDelta::Pixels(gpui::point(px(-100.), px(0.))),
                    modifiers: gpui::Modifiers::default(),
                    touch_phase: gpui::TouchPhase::Moved,
                }),
                cx,
            );
        });
        shell.read_with(cx, |shell, _| {
            assert!(
                shell.right_tab_scroll.offset().x < px(0.),
                "tab strip did not scroll"
            );
        });
    }

    #[gpui::test]
    fn subagent_tab_body_still_selects_drags_and_middle_closes(cx: &mut TestAppContext) {
        let (shell, cx) = setup(cx);
        let start = cx.debug_bounds("right-surface-tab-0").unwrap().center();
        cx.simulate_click(start, gpui::Modifiers::default());
        shell.read_with(cx, |shell, cx| {
            assert_eq!(shell.resolved_right_active(cx), RightSurface::Subagent(1));
        });
        cx.simulate_mouse_down(start, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            start + gpui::point(px(8.), px(0.)),
            Some(MouseButton::Left),
            gpui::Modifiers::default(),
        );
        cx.update(|_, cx| {
            assert_eq!(
                cx.has_active_drag(),
                crate::click_activation_drag_enabled(),
                "tab drag policy does not match the current platform"
            )
        });
        cx.simulate_mouse_up(start, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_down(start, MouseButton::Middle, gpui::Modifiers::default());
        cx.simulate_mouse_up(start, MouseButton::Middle, gpui::Modifiers::default());
        shell.read_with(cx, |shell, _| {
            assert!(!shell.subagent_tabs.contains_key(&1));
            assert!(shell.subagent_tabs.contains_key(&2));
        });
    }

    #[cfg(target_os = "windows")]
    #[gpui::test]
    fn subagent_tab_click_jitter_selects_without_starting_a_drag(cx: &mut TestAppContext) {
        let (shell, cx) = setup(cx);
        let start = cx.debug_bounds("right-surface-tab-0").unwrap().center();
        let end = start + gpui::point(px(8.), px(0.));

        cx.simulate_mouse_down(start, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(end, Some(MouseButton::Left), gpui::Modifiers::default());
        cx.update(|_, cx| {
            assert!(
                !cx.has_active_drag(),
                "ordinary Windows click jitter started a tab drag"
            )
        });
        cx.simulate_mouse_up(end, MouseButton::Left, gpui::Modifiers::default());

        shell.read_with(cx, |shell, cx| {
            assert_eq!(shell.resolved_right_active(cx), RightSurface::Subagent(1));
        });
    }
}

#[cfg(test)]
mod shortcut_focus_regressions {
    use super::*;
    use gpui::{AppContext, TestAppContext};

    struct ShortcutHost {
        root: FocusHandle,
        unfocused: FocusHandle,
        editor: FocusHandle,
        show_editor: bool,
        jumps: usize,
    }

    impl Render for ShortcutHost {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let root = self.root.clone();
            let unfocused = self.unfocused.clone();
            let preferred = self.editor.clone();
            window.defer(cx, move |window, cx| {
                restore_mounted_focus(&root, &preferred, &unfocused, window, cx);
            });
            div()
                .size_full()
                .track_focus(&self.root)
                .child(div().track_focus(&self.unfocused))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, window, cx| {
                        // Exercise mouse focus handoffs, hiding a focused pane,
                        // and clicking a control that explicitly clears focus.
                        this.show_editor = event.position.x < px(100.0);
                        if event.position.x < px(200.0) {
                            window.focus(&this.editor, cx);
                        } else {
                            window.blur();
                        }
                        cx.notify();
                    }),
                )
                .on_action(cx.listener(|this, _: &JumpSession, _, _| this.jumps += 1))
                .when(self.show_editor, |el| {
                    el.child(div().track_focus(&self.editor))
                })
        }
    }

    #[gpui::test]
    fn explicit_blur_does_not_refocus_a_mounted_input(cx: &mut TestAppContext) {
        let host = cx.add_window(|_, cx| ShortcutHost {
            root: cx.focus_handle(),
            unfocused: cx.focus_handle(),
            editor: cx.focus_handle(),
            show_editor: true,
            jumps: 0,
        });
        cx.run_until_parked();
        cx.update_window(host.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        host.update(cx, |host, window, cx| {
            window.focus(&host.editor, cx);
            window.blur();
            restore_mounted_focus(&host.root, &host.editor, &host.unfocused, window, cx);
            assert!(host.unfocused.is_focused(window));
            // Subsequent renders must keep the neutral shortcut focus too.
            restore_mounted_focus(&host.root, &host.editor, &host.unfocused, window, cx);
            assert!(host.unfocused.is_focused(window));
        })
        .unwrap();
    }

    #[gpui::test]
    fn shortcuts_recover_from_retained_editor_focus(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.bind_keys([KeyBinding::new(
                &platform_combo("mod-2"),
                JumpSession(1),
                None,
            )]);
        });
        let host = cx.add_window(|_, cx| ShortcutHost {
            root: cx.focus_handle(),
            unfocused: cx.focus_handle(),
            editor: cx.focus_handle(),
            show_editor: true,
            jumps: 0,
        });
        for show_editor in [true, false, true, false] {
            host.update(cx, |host, window, cx| {
                host.show_editor = show_editor;
                // Keep the editor handle alive and focused even when hidden.
                window.focus(&host.editor, cx);
                cx.notify();
            })
            .unwrap();
            cx.run_until_parked();
            cx.update_window(host.into(), |_, window, cx| window.draw(cx).clear())
                .unwrap();
            host.update(cx, |host, window, cx| {
                restore_mounted_focus(&host.root, &host.editor, &host.unfocused, window, cx);
                assert!(host.root.contains_focused(window, cx));
                assert_eq!(host.editor.is_focused(window), show_editor);
            })
            .unwrap();
            cx.simulate_keystrokes(host.into(), &platform_combo("mod-2"));
        }
        host.update(cx, |host, window, cx| {
            assert_eq!(host.jumps, 4);
            window.blur();
            restore_mounted_focus(&host.root, &host.editor, &host.unfocused, window, cx);
            assert!(host.unfocused.is_focused(window));
        })
        .unwrap();
    }

    #[gpui::test]
    fn shortcuts_work_after_mouse_focus_changes(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.bind_keys([KeyBinding::new(
                &platform_combo("mod-2"),
                JumpSession(1),
                None,
            )]);
        });
        let host = cx.add_window(|_, cx| ShortcutHost {
            root: cx.focus_handle(),
            unfocused: cx.focus_handle(),
            editor: cx.focus_handle(),
            show_editor: true,
            jumps: 0,
        });
        for (index, x) in [50.0, 150.0, 250.0, 50.0, 150.0, 250.0]
            .into_iter()
            .enumerate()
        {
            cx.update_window(host.into(), |_, window, cx| {
                window.draw(cx).clear();
                window.dispatch_event(
                    gpui::PlatformInput::MouseDown(MouseDownEvent {
                        position: gpui::point(px(x), px(20.0)),
                        button: MouseButton::Left,
                        modifiers: gpui::Modifiers::default(),
                        click_count: 1,
                        first_mouse: false,
                    }),
                    cx,
                );
            })
            .unwrap();
            // Dispatch immediately after the mouse event; no manual recovery.
            cx.simulate_keystrokes(host.into(), &platform_combo("mod-2"));
            host.update(cx, |host, window, cx| {
                assert_eq!(
                    host.jumps,
                    index + 1,
                    "shortcut failed after mouse click at {x}"
                );
                assert!(host.root.contains_focused(window, cx));
                assert_eq!(host.editor.is_focused(window), x < 100.0);
            })
            .unwrap();
        }
    }
}

/// Native visual QA uses the production shell with isolated fixture data.
#[cfg(feature = "appshots-fixture")]
impl Shell {
    pub fn fixture_appshots_settings(&mut self, open: bool, cx: &mut Context<Self>) {
        if open {
            self.open_settings(SettingsSection::Appshots, cx);
        } else {
            self.close_settings(cx);
        }
    }
    pub fn fixture_appshots_composer(&self) -> Entity<Composer> {
        self.composer.clone()
    }
    pub fn fixture_appshots_sidebar(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        self.settings.sidebar_collapsed = collapsed;
        cx.notify();
    }
    pub fn fixture_appshots_transcript_start(&self, cx: &mut Context<Self>) {
        self.transcript
            .update(cx, |t, cx| t.fixture_appshots_start(cx));
    }
}
