//! Right-pane surface tab strip, drag-and-drop reordering, and docking header.

use std::time::Duration;

use super::*;
use gpui::{div, px, AnyElement, Context, IntoElement, Render, SharedString, Window};

/// The dragged surface-tab payload (strip reorder).
pub(crate) struct RightTabDrag {
    pub(crate) panel_key: String,
    pub(crate) from: usize,
    pub(crate) title: SharedString,
    pub(crate) workspace_path: Option<WorkspacePathDrag>,
}

/// Live drag-over state for the surface-tab strip — the terminal drawer's
/// [`crate::terminal::panel`] DragState, ported: `epoch` keys the 150ms
/// slide-animation restarts as the hovered slot changes.
pub(crate) struct RightTabDragState {
    pub(crate) from: usize,
    pub(crate) over: usize,
    pub(crate) epoch: usize,
    pub(crate) prev_over: usize,
}

/// Ghost chip following the pointer while a surface tab drags.
pub(super) struct SurfaceTabGhost {
    pub(super) title: SharedString,
}

impl Render for SurfaceTabGhost {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        div()
            .h(crate::typography::ui_rems(28.0))
            .min_w(crate::typography::ui_rems(52.0))
            .max_w(crate::typography::ui_rems(180.0))
            .pl(crate::typography::ui_rems(6.0))
            .pr(crate::typography::ui_rems(8.0))
            .flex()
            .items_center()
            .rounded(crate::typography::ui_rems(6.0))
            .bg(theme.surface_raised)
            .border_1()
            .border_color(theme.border_strong)
            .text_size(crate::typography::ui_rems(12.0))
            .text_color(theme.text)
            .opacity(0.85)
            .child(div().truncate().child(self.title.clone()))
    }
}

pub(super) struct SurfaceTabTooltip {
    pub(super) text: SharedString,
}

impl Render for SurfaceTabTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let card = div()
            .max_w(px(380.0))
            .px(px(9.0))
            .py(px(6.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(theme.border)
            .bg(crate::popover::surface_bg(theme))
            .text_size(px(10.5))
            .text_color(theme.text_muted)
            .child(self.text.clone());
        crate::frost::frosted(6.0, crate::frost::MENU_BLUR, card)
    }
}

pub(crate) fn workspace_file_title(path: &str) -> SharedString {
    path.rsplit('/').next().unwrap_or(path).to_string().into()
}

pub(crate) fn estimated_tab_width(title: &str, dirty: bool, scale: f32) -> f32 {
    let scale = if scale <= 0.0 { 1.0 } else { scale };
    let base = (if dirty { 42.0 + 10.0 } else { 42.0 }) * scale;
    let text_w = title.chars().count() as f32 * (7.2 * scale);
    (base + text_w).clamp(56.0 * scale, 200.0 * scale)
}

pub(crate) fn right_tab_drop_index(rel_x: f32, widths: &[f32], gap: f32) -> usize {
    if widths.is_empty() {
        return 0;
    }
    let mut cursor = 0.0;
    for (i, &w) in widths.iter().enumerate() {
        let center = cursor + w * 0.5;
        if rel_x < center {
            return i;
        }
        cursor += w + gap;
    }
    widths.len() - 1
}

pub(crate) fn right_tab_slide_offset(
    ix: usize,
    from: usize,
    over: usize,
    widths: &[f32],
    gap: f32,
) -> f32 {
    if ix == from {
        0.0
    } else if from < over && ix > from && ix <= over {
        -(widths[from] + gap)
    } else if from > over && ix >= over && ix < from {
        widths[from] + gap
    } else {
        0.0
    }
}

impl Shell {
    pub(super) fn right_surface_rows(
        &self,
        cx: &App,
    ) -> Vec<(RightSurface, SharedString, bool, Option<SharedString>)> {
        let key = self.panel_key(cx);
        let stored: &[RightSurface] = self
            .right_tabs
            .get(&key)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        let terminals: Vec<(u64, SharedString, bool)> = self
            .right_terminal
            .as_ref()
            .map(|t| t.read(cx).tab_summaries(cx))
            .unwrap_or_default();
        stored
            .iter()
            .filter_map(|surface| match surface {
                RightSurface::Files => self.files.get(&key).map(|files| {
                    let files = files.read(cx);
                    (
                        *surface,
                        files.tab_title(),
                        files.has_unsaved_changes(),
                        files.attachment_path().map(SharedString::from),
                    )
                }),
                RightSurface::File(id) => self.file_surfaces.get(id).map(|file| {
                    let path = self.file_surface_paths.get(id);
                    let title = path
                        .map(|path| workspace_file_title(path))
                        .unwrap_or_else(|| SharedString::from("File"));
                    (
                        *surface,
                        title,
                        file.read(cx).has_unsaved_changes(),
                        path.cloned().map(Into::into),
                    )
                }),
                RightSurface::Diff(id) => self
                    .diffs
                    .get(id)
                    // Contextual title (user request): the pane's scope
                    // label, or the pinned commit's subject.
                    .map(|changes| (*surface, changes.read(cx).tab_title(), false, None)),
                RightSurface::Terminal(tab) => terminals
                    .iter()
                    .find(|(k, _, _)| k == tab)
                    .map(|(_, title, _)| (*surface, title.clone(), false, None)),
                RightSurface::Subagent(id) => self
                    .subagent_tabs
                    .get(id)
                    .map(|tab| (*surface, tab.title.clone(), false, None)),
                RightSurface::Browser(id) => self.browsers.get(id).map(|browser| {
                    let browser = browser.read(cx);
                    (
                        *surface,
                        browser.title(),
                        false,
                        browser.page.url.clone().map(Into::into),
                    )
                }),
                RightSurface::Picker => None,
            })
            .collect()
    }

    pub(super) fn workspace_path_for_surface(
        &self,
        surface: RightSurface,
        cx: &App,
    ) -> Option<WorkspacePathDrag> {
        let path = match surface {
            RightSurface::Files => self
                .files
                .get(&self.panel_key(cx))
                .and_then(|files| files.read(cx).attachment_path().map(str::to_string))?,
            RightSurface::File(id) => self.file_surface_paths.get(&id)?.clone(),
            RightSurface::Picker
            | RightSurface::Diff(_)
            | RightSurface::Terminal(_)
            | RightSurface::Subagent(_)
            | RightSurface::Browser(_) => {
                return None;
            }
        };
        Some(WorkspacePathDrag::new(path, false))
    }

    /// Drag-reorder a surface tab within this chat's strip.
    pub(super) fn reorder_right_tabs(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        let key = self.panel_key(cx);
        if let Some(tabs) = self.right_tabs.get_mut(&key)
            && from < tabs.len()
            && to < tabs.len()
            && from != to
        {
            let surface = tabs.remove(from);
            tabs.insert(to, surface);
            cx.notify();
        }
    }

    /// Track the hovered drop slot mid-drag (the terminal drawer's
    /// `update_drag_over`, ported: epoch bumps restart the slide tween).
    pub(super) fn update_right_tab_drag_over(&mut self, from: usize, over: usize, cx: &mut Context<Self>) {
        match &mut self.right_tab_drag {
            Some(drag) if drag.over != over => {
                drag.prev_over = drag.over;
                drag.over = over;
                drag.epoch += 1;
                cx.notify();
            }
            Some(_) => {}
            None => {
                self.right_tab_drag = Some(RightTabDragState {
                    from,
                    over,
                    epoch: 0,
                    prev_over: from,
                });
                cx.notify();
            }
        }
    }

    pub(super) fn close_right_plus(&mut self, cx: &mut Context<Self>) {
        if self.right_plus.begin_close() {
            popover::reap_popup(cx, |shell: &mut Self| &mut shell.right_plus);
        }
        cx.notify();
    }

    /// The titlebar strip over the right pane: one chip per surface tab
    /// (icon · title · ✕) plus the `+` menu — the t3code RightPanelTabs bar,
    /// living in the top row; the diff options moved into the pane below.
    pub(crate) fn render_right_tab_strip(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        // Heal drag state if the pointer was released outside the strip.
        if self.right_tab_drag.is_some() && !cx.has_active_drag() {
            self.right_tab_drag = None;
        }
        let rows = self.right_surface_rows(cx);
        let count = rows.len();
        let scale = crate::typography::font_size(cx).pixels() / 16.0;
        let tab_widths: Vec<f32> = rows
            .iter()
            .map(|(_, title, dirty, _)| estimated_tab_width(title, *dirty, scale))
            .collect();
        let widths_for_drag = tab_widths.clone();
        let active = self.resolved_right_active(cx);
        let drag = self
            .right_tab_drag
            .as_ref()
            .map(|d| (d.from, d.over, d.epoch, d.prev_over));

        // Fade flags from the LAST frame's scroll state (invisible lag).
        // The EdgeFade scope below fades per-pixel on x for glyphs AND
        // quads/images (fork 5d1f83d) — washes dissolve across the band.
        const FADE_WIDTH: f32 = 36.0;
        let scrolled = -f32::from(self.right_tab_scroll.offset().x);
        let max_scroll = f32::from(self.right_tab_scroll.max_offset().x);
        let fade_left = scrolled > 1.0;
        let fade_right = scrolled < max_scroll - 1.0;
        // The old session-tab strip's proven scroll shape: the flex row IS
        // the scroller (id + overflow_x_scroll + track_scroll), wrapped in a
        // relative min_w_0 region below; drop math runs in CONTENT
        // coordinates (viewport-relative x plus the scrolled-off width).
        let scroll_for_drag = self.right_tab_scroll.clone();
        let mut strip = div()
            .id("right-surface-strip")
            .flex()
            .flex_row()
            .items_center()
            .gap(crate::typography::ui_rems(6.0))
            .min_w_0()
            .overflow_x_scroll()
            .track_scroll(&self.right_tab_scroll)
            // Windows caption hit-testing includes the scroll-only hitboxes
            // behind each chip. Stop at the scroller so the titlebar cannot
            // claim tab clicks, while wheel events still reach this scroller.
            .when(cfg!(target_os = "windows"), |strip| strip.occlude())
            .on_drag_move::<RightTabDrag>(cx.listener(
                move |this, event: &gpui::DragMoveEvent<RightTabDrag>, _, cx| {
                    let payload = event.drag(cx);
                    if payload.panel_key != this.panel_key(cx) {
                        return;
                    }
                    let from = payload.from;
                    let rel_x = f32::from(event.event.position.x)
                        - f32::from(event.bounds.left())
                        - f32::from(scroll_for_drag.offset().x);
                    let over = right_tab_drop_index(rel_x, &widths_for_drag, 4.0);
                    this.update_right_tab_drag_over(from, over, cx);
                },
            ))
            .on_drop::<RightTabDrag>(cx.listener(move |this, payload: &RightTabDrag, _, cx| {
                if payload.panel_key != this.panel_key(cx) {
                    this.right_tab_drag = None;
                    cx.notify();
                    return;
                }
                let to = this
                    .right_tab_drag
                    .as_ref()
                    .map(|d| d.over)
                    .unwrap_or(payload.from);
                this.right_tab_drag = None;
                this.reorder_right_tabs(payload.from, to, cx);
            }));
        for (ix, (surface, title, dirty, detail)) in rows.into_iter().enumerate() {
            let is_active = surface == active;
            let file_identity_path = detail.as_ref().cloned().unwrap_or_else(|| title.clone());
            let is_file = matches!(surface, RightSurface::File(_))
                || (matches!(surface, RightSurface::Files) && detail.is_some());
            let icon_path = match surface {
                RightSurface::Files => {
                    if is_file {
                        icons::DOCUMENT
                    } else {
                        icons::FOLDER_WITH_FILES
                    }
                }
                RightSurface::File(_) => icons::DOCUMENT,
                RightSurface::Diff(id) => self
                    .diffs
                    .get(&id)
                    .map(|changes| {
                        if changes.read(cx).is_history() {
                            icons::GIT_BRANCH
                        } else {
                            icons::LIST
                        }
                    })
                    .unwrap_or(icons::LIST),
                RightSurface::Subagent(_) => icons::BOT,
                RightSurface::Terminal(_) => icons::TERMINAL,
                RightSurface::Browser(_) => icons::GLOBE,
                RightSurface::Picker => icons::PLUS,
            };
            // A live subagent tab swaps its icon for the mini working
            // spinner (the history fetch button's in-flight recipe) — the
            // doc's streaming tail entry IS the run's liveness, so the swap
            // settles by itself when the subagent finishes.
            let browser_favicon = match surface {
                RightSurface::Browser(id) => self
                    .browsers
                    .get(&id)
                    .and_then(|b| b.read(cx).favicon.clone()),
                _ => None,
            };
            let subagent_running = match surface {
                RightSurface::Browser(id) => self
                    .browsers
                    .get(&id)
                    .is_some_and(|b| b.read(cx).page.loading),
                RightSurface::Subagent(id) => self.subagent_tabs.get(&id).is_some_and(|tab| {
                    self.state
                        .read(cx)
                        .sub_transcript(&tab.doc_id)
                        .last()
                        .is_some_and(|e| e.status == Some(zeron_doc::MessageStatus::Streaming))
                }),
                _ => false,
            };
            // t3 tab hover: the surface icon swaps IN PLACE for the close ✕
            // (same slot, no width jump) — the ✕ only shows while the tab is
            // hovered (user request).
            let group: SharedString = format!("right-surface-tab-{ix}").into();
            let ghost_title = title.clone();
            let workspace_path = self.workspace_path_for_surface(surface, cx);
            let accessible_name = detail.as_ref().unwrap_or(&title);
            let accessible_label = if dirty {
                format!("{accessible_name}, unsaved changes")
            } else {
                accessible_name.to_string()
            };
            let chip = div()
                .id(("right-surface-tab", ix))
                .debug_selector(|| format!("right-surface-tab-{ix}"))
                .group(group.clone())
                .h(crate::typography::ui_rems(28.0))
                .min_w(crate::typography::ui_rems(52.0))
                .max_w(crate::typography::ui_rems(200.0))
                .flex_none()
                .pl(crate::typography::ui_rems(8.0))
                .pr(crate::typography::ui_rems(10.0))
                .rounded(crate::typography::ui_rems(6.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(crate::typography::ui_rems(6.0))
                .cursor_pointer()
                .role(gpui::Role::Button)
                .aria_label(accessible_label)
                .when_some(detail, |chip, detail| {
                    chip.tooltip(move |_, cx| {
                        cx.new(|_| SurfaceTabTooltip {
                            text: detail.clone(),
                        })
                        .into()
                    })
                    .tooltip_show_delay(Duration::from_millis(350))
                })
                // The old session-tab strip's solved carve-out: NOT
                // `.occlude()` — a BlockMouse hitbox ends the hit test,
                // so the scroll container behind the tabs never saw
                // wheel events and an overflowing strip could not be
                // scrolled (tabs tile the whole region). ExceptScroll
                // keeps the titlebar drag-region carve-out and lets the
                // strip scroll.
                .block_mouse_except_scroll()
                .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
                    window.prevent_default()
                })
                .when(is_active, |el| el.bg(crate::theme::wash(0.10)))
                .when(!is_active, |el| {
                    el.hover(|s| s.bg(crate::theme::wash(0.06)))
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.set_right_active(surface, cx);
                    this.focus_right_file_editor(surface, window, cx);
                }))
                // Middle-click closes, like every tab strip.
                .on_mouse_down(
                    gpui::MouseButton::Middle,
                    cx.listener(move |this, _, window, cx| {
                        this.close_right_surface(surface, window, cx);
                    }),
                )
                .when(crate::click_activation_drag_enabled(), |el| {
                    el.on_drag(
                        RightTabDrag {
                            panel_key: self.panel_key(cx),
                            from: ix,
                            title: ghost_title,
                            workspace_path,
                        },
                        |payload, _point, _, cx| {
                            let title = payload.title.clone();
                            cx.stop_propagation();
                            cx.new(|_| SurfaceTabGhost { title })
                        },
                    )
                })
                .child(
                    // Leading slot: icon normally, ✕ on tab hover — two
                    // stacked layers opacity-swapped by the group hover.
                    div()
                        .id(("right-surface-close", ix))
                        .debug_selector(|| format!("right-surface-close-{ix}"))
                        .flex_none()
                        .size(crate::typography::ui_rems(18.0))
                        .rounded(crate::typography::ui_rems(4.0))
                        .relative()
                        .hover(|s| s.bg(crate::theme::wash(0.12)))
                        // The tab owns a drag payload. Claim the close press
                        // before it reaches that parent or GPUI starts a tab
                        // drag instead of delivering the close click.
                        .on_mouse_down(gpui::MouseButton::Left, |_, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.close_right_surface(surface, window, cx);
                        }))
                        .child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .group_hover(group.clone(), |s| s.opacity(0.0))
                                .child(if subagent_running {
                                    loaders::mini_glyph_spinner(
                                        format!("subagent-tab-{ix}"),
                                        2.0,
                                        theme.glyph,
                                        cx.entity_id(),
                                        cx,
                                    )
                                    .into_any_element()
                                } else if let Some(favicon) = browser_favicon {
                                    gpui::img(favicon).size(crate::typography::ui_rems(13.0)).into_any_element()
                                } else if is_file {
                                    crate::file_icons::icon(
                                        crate::file_icons::FileIconIdentity::file(
                                            file_identity_path.as_ref(),
                                        ),
                                        theme.appearance,
                                    )
                                    .size(crate::typography::ui_rems(15.0))
                                    .when(!is_active, |icon| icon.opacity(0.78))
                                    .into_any_element()
                                } else {
                                    icon(icon_path)
                                        .size(crate::typography::ui_rems(13.0))
                                        .text_color(if is_active {
                                            theme.text_muted
                                        } else {
                                            theme.text_muted.opacity(0.7)
                                        })
                                        .into_any_element()
                                }),
                        )
                        .child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .opacity(0.0)
                                .group_hover(group.clone(), |s| s.opacity(1.0))
                                .child(
                                    icon(icons::CLOSE)
                                        .size(crate::typography::ui_rems(12.0))
                                        .text_color(theme.text_muted),
                                 ),
                        ),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(crate::typography::ui_rems(12.0))
                        .text_color(if is_active {
                            theme.text
                        } else {
                            theme.text_muted
                        })
                        .child(title),
                )
                .when(dirty, |chip| {
                    chip.child(
                        div()
                            .flex_none()
                            .size(crate::typography::ui_rems(6.0))
                            .rounded_full()
                            .bg(theme.text_muted),
                    )
                });
            // Sliding transform while a sibling drags over (the terminal
            // drawer's exact recipe): animate 150ms between committed
            // offsets; the dragged tab leaves an invisible spacer — the
            // ghost carries it.
            let wrapped: AnyElement = match drag {
                Some((from, over, epoch, prev_over)) if ix != from => {
                    let target = right_tab_slide_offset(ix, from, over, &tab_widths, 4.0);
                    let start = right_tab_slide_offset(ix, from, prev_over, &tab_widths, 4.0);
                    div()
                        .relative()
                        .child(chip.with_animation(
                            ("right-tab-slide", (ix as u64) | ((epoch as u64) << 32)),
                            TAB_SLIDE.animation(),
                            move |el, t| el.left(px(motion::lerp(start, target, t))),
                        ))
                        .into_any_element()
                }
                Some((from, ..)) if ix == from => div()
                    .w(px(tab_widths[ix]))
                    .h(crate::typography::ui_rems(28.0))
                    .flex_none()
                    .into_any_element(),
                _ => chip.into_any_element(),
            };
            strip = strip.child(wrapped);
        }

        // The `+` — a small menu offering the available surfaces (t3 "Add panel
        // surface"); mirrors the picker cards.
        let plus_open = self.right_plus.get().is_some();
        let plus_fade = "right-surface-add-fade";
        let mut plus = div()
            .id("right-surface-add")
            .size(crate::typography::ui_rems(28.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(crate::typography::ui_rems(6.0))
            .cursor_pointer()
            .bg(motion::hover_blend(
                plus_fade,
                crate::theme::wash(0.0),
                crate::theme::wash(0.11),
            ))
            .on_hover(motion::hover_listener(plus_fade))
            .block_mouse_except_scroll()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, _| {
                    window.prevent_default();
                    this.right_plus.note_trigger_press();
                }),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                if this.right_plus.take_press_was_open() {
                    this.close_right_plus(cx);
                } else {
                    this.right_plus.open(());
                    cx.notify();
                }
            }))
            .child(
                icon(icons::PLUS)
                    .size(crate::typography::ui_rems(14.0))
                    .text_color(theme.text_muted),
            );
        if plus_open {
            let closing = self.right_plus.closing_since();
            let menu = popover::popover_card(&theme)
                .w(px(168.0))
                .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_right_plus(cx)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            popover::menu_row(&theme, false, "right-plus-files")
                                .id("right-plus-files-row")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.add_files_surface(window, cx);
                                    this.close_right_plus(cx);
                                }))
                                .child(
                                    icon(icons::FOLDER_WITH_FILES)
                                        .size(px(13.0))
                                        .text_color(theme.text_muted),
                                )
                                .child(SharedString::from("Files")),
                        )
                        .child(
                            popover::menu_row(&theme, false, "right-plus-browser")
                                .id("right-plus-browser-row")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.add_browser_surface(None, window, cx);
                                    this.close_right_plus(cx);
                                }))
                                .child(
                                    icon(icons::GLOBE)
                                        .size(px(13.0))
                                        .text_color(theme.text_muted),
                                )
                                .child(SharedString::from("Browser")),
                        )
                        .child(
                            popover::menu_row(&theme, false, "right-plus-terminal")
                                .id("right-plus-terminal-row")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.add_terminal_surface(cx);
                                    this.close_right_plus(cx);
                                }))
                                .child(
                                    icon(icons::TERMINAL)
                                        .size(px(13.0))
                                        .text_color(theme.text_muted),
                                )
                                .child(SharedString::from("Terminal")),
                        )
                        .when(self.space_git_detected(cx), |menu| {
                            menu.child(
                                popover::menu_row(&theme, false, "right-plus-diff")
                                    .id("right-plus-diff-row")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.add_diff_surface(cx);
                                        this.close_right_plus(cx);
                                    }))
                                    .child(
                                        icon(icons::LIST)
                                            .size(px(13.0))
                                            .text_color(theme.text_muted),
                                    )
                                    .child(SharedString::from("Diffs")),
                            )
                            .child(
                                popover::menu_row(&theme, false, "right-plus-history")
                                    .id("right-plus-history-row")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.add_history_surface(cx);
                                        this.close_right_plus(cx);
                                    }))
                                    .child(
                                        icon(icons::GIT_BRANCH)
                                            .size(px(13.0))
                                            .text_color(theme.text_muted),
                                    )
                                    .child(SharedString::from("History")),
                            )
                        }),
                )
                .into_any_element();
            plus = plus.relative().child(popover::anchored_menu_below_gap(
                "right-plus-menu",
                menu,
                closing,
                10.0,
            ));
        }
        // The empty-state picker already offers every surface. Show a single
        // Chrome-style add-tab affordance only after at least one tab exists.
        strip = strip.when(count > 0, |strip| strip.child(plus));
        // Edge fades on whichever side hides tabs (flags computed above).
        // Glass: per-glyph EdgeFade scope over the chips' own opacity ramps;
        // opaque: painted gradients in the shell surface tone.
        let glass = theme.is_glass();
        let bar_bg = theme.surface;
        let region = div()
            .relative()
            .min_w_0()
            .size_full()
            .flex()
            .items_center()
            .child(strip)
            .when(fade_left && !glass, |el| {
                el.child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(FADE_WIDTH))
                        .bg(gpui::linear_gradient(
                            90.0,
                            gpui::linear_color_stop(bar_bg, 0.0),
                            gpui::linear_color_stop(bar_bg.opacity(0.0), 1.0),
                        )),
                )
            })
            .when(fade_right && !glass, |el| {
                el.child(
                    div()
                        .absolute()
                        .right_0()
                        .top_0()
                        .bottom_0()
                        .w(px(FADE_WIDTH))
                        .bg(gpui::linear_gradient(
                            270.0,
                            gpui::linear_color_stop(bar_bg, 0.0),
                            gpui::linear_color_stop(bar_bg.opacity(0.0), 1.0),
                        )),
                )
            });
        if glass {
            crate::edge_fade::edge_faded(FADE_WIDTH, false, false, region)
                .fade_left(fade_left)
                .fade_right(fade_right)
                .into_any_element()
        } else {
            region.into_any_element()
        }
    }

    /// Toggle the changes-panel takeover (the header's expand button, t3code
    /// parity): the panel grows to fill everything right of the sidebar,
    /// hiding the conversation column; toggling back restores the saved
    /// width. Rides the same width tween as open/close so the jump glides.
    pub(super) fn toggle_right_pane_expand(&mut self, cx: &mut Context<Self>) {
        let from = self.right_target(cx);
        self.right_edge_bounce = None;
        self.right_resize_edge = None;
        self.finish_pane_resize(PaneResizeKind::Right);
        let sidebar_now = self.sidebar_now();
        let from_main = conversation_width(self.viewport_width, sidebar_now, from);
        self.right_pane_expanded = !self.right_pane_expanded;
        let to = self.right_target(cx);
        let right_transition = WidthTween::new(from, to);
        self.right_tween = Some(right_transition);
        self.right_takeover_content_tween = Some(right_transition);
        self.main_takeover_tween = Some(WidthTween::new(
            from_main,
            conversation_width(self.viewport_width, sidebar_now, to),
        ));
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_width_adapts_to_content_and_clamps() {
        let files_w = estimated_tab_width("Files", false, 1.0);
        assert!(files_w < 100.0, "Files tab should be compact: {files_w}");
        assert!(files_w >= 52.0, "Files tab should respect minimum width: {files_w}");

        let dirty_files_w = estimated_tab_width("Files", true, 1.0);
        assert!(dirty_files_w > files_w, "Dirty tab should account for dirty indicator");

        let long_w = estimated_tab_width("a_very_long_file_name_that_exceeds_max_width_here.rs", false, 1.0);
        assert_eq!(long_w, 200.0, "Long tab should be capped at max width");
    }

    #[test]
    fn drop_index_calculates_correct_slot_with_variable_widths() {
        let widths = [60.0, 120.0, 80.0];
        let gap = 4.0;
        // Item 0: center = 30.0
        assert_eq!(right_tab_drop_index(10.0, &widths, gap), 0);
        assert_eq!(right_tab_drop_index(29.0, &widths, gap), 0);
        // Item 1: starts at 64.0, center = 64 + 60 = 124.0
        assert_eq!(right_tab_drop_index(50.0, &widths, gap), 1);
        assert_eq!(right_tab_drop_index(120.0, &widths, gap), 1);
        // Item 2: starts at 64 + 124 = 188.0, center = 188 + 40 = 228.0
        assert_eq!(right_tab_drop_index(150.0, &widths, gap), 2);
        assert_eq!(right_tab_drop_index(300.0, &widths, gap), 2);
    }

    #[test]
    fn slide_offset_shifts_affected_tabs_correctly() {
        let widths = [60.0, 120.0, 80.0];
        let gap = 4.0;

        // Drag from 0 to 1 (left to right): item 1 shifts left by (widths[0] + gap) = -64
        assert_eq!(right_tab_slide_offset(1, 0, 1, &widths, gap), -64.0);
        assert_eq!(right_tab_slide_offset(2, 0, 1, &widths, gap), 0.0);
        assert_eq!(right_tab_slide_offset(0, 0, 1, &widths, gap), 0.0);

        // Drag from 2 to 0 (right to left): items 0 and 1 shift right by (widths[2] + gap) = +84
        assert_eq!(right_tab_slide_offset(0, 2, 0, &widths, gap), 84.0);
        assert_eq!(right_tab_slide_offset(1, 2, 0, &widths, gap), 84.0);
        assert_eq!(right_tab_slide_offset(2, 2, 0, &widths, gap), 0.0);
    }
}
