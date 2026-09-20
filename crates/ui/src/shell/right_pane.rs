//! Right pane surface host and layout rendering.

use gpui::{div, prelude::*, px, AnyElement, Context, IntoElement, Window};

use crate::theme::Theme;

use super::{RightSurface, Shell};

impl Shell {
    /// Right pane — the surface host (t3code RightPanelTabs): hidden by
    /// default, drag-resizable. Content is the ACTIVE surface — the Diff
    /// page (its options row + the lazy [`Changes`] viewer), workspace Files,
    /// an embedded terminal, or the surface picker when no tabs exist.
    pub(crate) fn render_right_pane(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let bg = theme.bg;
        let content: AnyElement = if self.right_pane_open(cx) || self.tween_active(self.right_tween)
        {
            match self.resolved_right_active(cx) {
                // Rendering a Files surface activates its image. Keep it unmounted
                // throughout the closing animation after suspending its resources.
                RightSurface::Files | RightSurface::File(_) if !self.right_pane_open(cx) => {
                    gpui::Empty.into_any_element()
                }
                RightSurface::Files => {
                    let key = self.panel_key(cx);
                    if let Some(files) = self.files.get(&key).cloned() {
                        files.update(cx, |files, cx| files.ensure_loaded(cx));
                        files.into_any_element()
                    } else {
                        self.render_surface_picker(cx)
                    }
                }
                RightSurface::File(id) => {
                    if let Some(file) = self.file_surfaces.get(&id).cloned() {
                        file.update(cx, |file, cx| file.ensure_loaded(cx));
                        file.into_any_element()
                    } else {
                        self.render_surface_picker(cx)
                    }
                }
                RightSurface::Diff(id) if self.diffs.contains_key(&id) => {
                    let changes = self.diffs.get(&id).cloned().expect("checked");
                    // Idempotent — also covers a persisted-open pane on boot.
                    changes.update(cx, |changes, cx| changes.ensure_content(cx));
                    // The diff options (scope dropdown, ref selector,
                    // fold-all) moved DOWN from the titlebar band — the
                    // surface tabs own that row now; the expand/close
                    // buttons stayed up there (user request).
                    let controls =
                        changes.update(cx, |changes, cx| changes.render_header_controls(cx));
                    div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .child(crate::surface_chrome::toolbar(&theme).child(controls))
                        .child(div().flex_1().min_h_0().child(changes))
                        .into_any_element()
                }
                RightSurface::Browser(id) => self
                    .browsers
                    .get(&id)
                    .cloned()
                    .map(|browser| browser.into_any_element())
                    .unwrap_or_else(|| self.render_surface_picker(cx)),
                RightSurface::Terminal(tab) => {
                    let panel = self.right_terminal_panel(cx);
                    // Keep the embedded panel's own active tab aligned with
                    // the resolved surface (fallbacks can move it).
                    let resize_suspended = self.tween_active(self.right_tween);
                    panel.update(cx, |panel, cx| {
                        panel.set_resize_suspended(resize_suspended);
                        panel.select_tab_by_key(tab, cx);
                    });
                    panel.into_any_element()
                }
                RightSurface::Subagent(id) if self.subagent_tabs.contains_key(&id) => {
                    let transcript = self
                        .subagent_tabs
                        .get(&id)
                        .expect("checked")
                        .transcript
                        .clone();
                    // The pane hosts its own jump pill: the conversation
                    // overlay's is bound to the PRIMARY transcript, and this
                    // one anchors to the pane (no composer stack to clear).
                    let pill = transcript.read(cx).jump_button_shown().then(|| {
                        div()
                            .absolute()
                            .bottom(px(16.0))
                            .left_0()
                            .right_0()
                            .flex()
                            .justify_center()
                            .child(self.jump_pill(
                                "subagent-jump-to-bottom",
                                "subagent-jump-pill",
                                transcript.clone(),
                                cx,
                            ))
                    });
                    // Read-only surface: the transcript fills the pane — no
                    // composer, no status strip.
                    div()
                        .size_full()
                        .relative()
                        .flex()
                        .flex_col()
                        .child(div().flex_1().min_h_0().child(transcript))
                        .children(pill)
                        .into_any_element()
                }
                _ => self.render_surface_picker(cx),
            }
        } else {
            gpui::Empty.into_any_element()
        };
        // Flush panel (user request — the inset card is gone): full window
        // height with a left hairline, glass-friendly like the terminal dock
        // (translucent over the frost; solid otherwise). The resize grabber
        // lives outside this clipped container, on the root layout's seam.
        let panel_bg = if cfg!(target_os = "windows") && theme.is_glass() {
            match theme.appearance {
                crate::theme::Appearance::Dark => bg.opacity(0.35),
                crate::theme::Appearance::Light => bg.opacity(0.85),
            }
        } else if theme.is_glass() {
            bg.opacity(0.4)
        } else {
            bg
        };
        let panel = div()
            .size_full()
            .flex()
            .flex_col()
            // In takeover the panel's left edge IS the sidebar seam, which
            // already carries the sidebar tone's right hairline — a second
            // border there doubled up (user report).
            .when(!self.right_pane_expanded, |el| {
                el.border_l_1().border_color(theme.border)
            })
            // The panel's right edge IS the window's right edge: it carries
            // the CSD window's rounded corners directly (gpui cannot clip
            // children rounded — each full-bleed layer rounds itself; see
            // [`Self::window_corner_radius`]).
            .when(Self::window_corner_radius(window) > 0.0, |el| {
                let corner = Self::window_corner_radius(window);
                el.rounded_tr(px(corner)).rounded_br(px(corner))
            })
            .bg(panel_bg)
            .overflow_hidden()
            // The titlebar is a glass overlay over the full-height content
            // row; the panel's own chrome starts below it.
            .pt(px(Theme::TITLEBAR_HEIGHT))
            .child(content);
        let target = self.right_target(cx);
        let edge_offset = self.eval_resize_edge_bounce(
            self.right_edge_bounce,
            self.right_pane_open(cx) && !self.right_pane_expanded,
        );
        self.right_pane_container(
            self.right_tween,
            target,
            edge_offset,
            div().h_full().relative().child(panel).into_any_element(),
        )
    }
}
