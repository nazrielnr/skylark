//! Terminal keyboard, grid, selection, and scroll interactions.

use super::*;

impl TerminalPanel {
    // ---- input ----

    /// Queue keyboard bytes on the active tab (12 ms coalescing window).
    pub(super) fn queue_input(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        let Some(chat) = self.selected_chat(cx) else {
            return;
        };
        let Some(tabs) = self.chats.get_mut(&chat) else {
            return;
        };
        let active = tabs.active;
        let Some(tab) = tabs.tabs.get_mut(active) else {
            return;
        };
        if tab.exited.is_some() {
            return;
        }
        // A keypress while scrolled back snaps to the live bottom (xterm).
        if tab.emulator.display_offset() > 0 {
            tab.emulator.scroll_to_bottom();
        }
        let key = tab.key;
        if tab.coalescer.push(bytes) {
            tab.flush_task = Some(Self::schedule_flush(chat, key, cx));
        }
    }

    pub(super) fn schedule_flush(chat: String, key: u64, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(COALESCE_MS))
                .await;
            let _ = this.update(cx, |panel, cx| panel.flush_input(chat, key, cx));
        })
    }

    pub(super) fn flush_input(&mut self, chat: String, key: u64, cx: &mut Context<Self>) {
        let Some(engine) = self.engine(cx) else {
            return;
        };
        let Some(tab) = self.tab_mut(&chat, key) else {
            return;
        };
        let target = tab.target_device_id.clone();
        if tab.coalescer.is_empty() {
            return;
        }
        let Some(id) = tab.terminal_id.clone() else {
            // OpenTerminal still in flight — keep the buffer, retry shortly.
            if tab.exited.is_none() {
                tab.flush_task = Some(Self::schedule_flush(chat, key, cx));
            }
            return;
        };
        let data = encode_base64(&tab.coalescer.take());
        cx.spawn(async move |_, _| {
            let _ = engine
                .client()
                .call(
                    methods::WRITE_TERMINAL,
                    with_target(
                        serde_json::json!({ "terminalId": id, "data": data }),
                        &target,
                    ),
                )
                .await;
        })
        .detach();
    }

    pub(super) fn paste_clipboard(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let bracketed = self
            .active_tab(cx)
            .map(|tab| tab.emulator.bracketed_paste_mode())
            .unwrap_or(false);
        let bytes = paste_bytes(&text, bracketed);
        self.queue_input(&bytes, cx);
    }

    pub(super) fn on_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ks = &event.keystroke;
        let mods = &ks.modifiers;
        // Paste: Cmd+V (macOS) / Ctrl+Shift+V.
        if ks.key == "v" && (mods.platform || (mods.control && mods.shift)) {
            self.paste_clipboard(cx);
            cx.stop_propagation();
            return;
        }
        // Copy: Cmd+C (macOS) / Ctrl+Shift+C. Only swallowed when it actually
        // copied — so Ctrl+Shift+C with nothing selected still falls through
        // to the interrupt, and plain Ctrl+C (no shift) never reaches here.
        if ks.key == "c"
            && (mods.platform || (mods.control && mods.shift))
            && self.copy_selection(cx)
        {
            cx.stop_propagation();
            return;
        }
        let app_cursor = self
            .active_tab(cx)
            .map(|tab| tab.emulator.app_cursor_mode())
            .unwrap_or(false);
        if let Some(bytes) = keystroke_bytes(&ks.key, ks.key_char.as_deref(), mods, app_cursor) {
            self.queue_input(&bytes, cx);
            cx.stop_propagation();
        }
    }

    // ---- grid metrics / element hooks ----

    /// Called from element prepaint with the frame's grid placement. Resizes
    /// the emulator immediately; the `ResizeTerminal` RPC debounces 80 ms.
    pub(crate) fn on_grid_metrics(&mut self, geometry: GridGeometry, cx: &mut Context<Self>) {
        // Stash unconditionally, before the early returns below: pointer
        // mapping needs the placement even on frames where nothing resized,
        // which is almost all of them.
        self.geometry = Some(geometry);
        if self.resize_suspended {
            return;
        }
        let (cols, rows) = (geometry.cols, geometry.rows);
        let Some(chat) = self.selected_chat(cx) else {
            return;
        };
        let engine = self.engine(cx);
        let Some(tabs) = self.chats.get_mut(&chat) else {
            return;
        };
        let active = tabs.active;
        let Some(tab) = tabs.tabs.get_mut(active) else {
            return;
        };
        if tab.emulator.cols() == cols as usize && tab.emulator.rows() == rows as usize {
            return;
        }
        tab.emulator.resize(cols, rows);
        let key = tab.key;
        let target = tab.target_device_id.clone();
        if let (Some(engine), Some(tab)) = (engine, self.tab_mut(&chat, key)) {
            let id = tab.terminal_id.clone();
            tab.resize_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(RESIZE_DEBOUNCE_MS))
                    .await;
                // Re-read the *current* size — later prepaints may have
                // resized again inside the debounce window.
                let Ok(current) = this.update(cx, |panel, _| {
                    panel
                        .tab_mut(&chat, key)
                        .map(|t| (t.terminal_id.clone(), t.emulator.cols(), t.emulator.rows()))
                }) else {
                    return;
                };
                let Some((stored_id, cols, rows)) = current else {
                    return;
                };
                let Some(id) = stored_id.or(id) else { return };
                let _ = engine
                    .client()
                    .call(
                        methods::RESIZE_TERMINAL,
                        with_target(
                            serde_json::json!({ "terminalId": id, "cols": cols, "rows": rows }),
                            &target,
                        ),
                    )
                    .await;
            }));
        }
        // Deliberately no cx.notify(): this runs during prepaint of the
        // current frame, which already paints the resized grid.
    }

    /// Snapshot for the paint element.
    pub(crate) fn active_grid_snapshot(&self, cx: &App) -> Option<GridSnapshot> {
        let tab = self.active_tab(cx)?;
        Some(GridSnapshot {
            lines: tab.emulator.lines(),
            cursor: tab.emulator.cursor(),
        })
    }

    // ---- selection ----

    /// Run `f` against the active tab's emulator.
    pub(super) fn with_active_emulator<R>(
        &mut self,
        cx: &App,
        f: impl FnOnce(&mut Emulator) -> R,
    ) -> Option<R> {
        let chat = self.selected_chat(cx)?;
        let tabs = self.chats.get_mut(&chat)?;
        let active = tabs.active;
        tabs.tabs.get_mut(active).map(|tab| f(&mut tab.emulator))
    }

    /// Window position → grid point, using this frame's placement. `None`
    /// before the first prepaint, or when no tab is active.
    pub(super) fn grid_point_at(
        &mut self,
        position: gpui::Point<Pixels>,
        cx: &App,
    ) -> Option<(GridPoint, Side)> {
        let geometry = self.geometry?;
        let hit = cell_at(
            f32::from(position.x - geometry.origin.x),
            f32::from(position.y - geometry.origin.y),
            geometry.cell_w,
            geometry.line_h,
            geometry.cols as usize,
            geometry.rows as usize,
        );
        let point = self.with_active_emulator(cx, |emu| emu.grid_point(hit.row, hit.col))?;
        Some((point, hit.side))
    }

    pub(super) fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        let Some((point, side)) = self.grid_point_at(event.position, cx) else {
            return;
        };
        // Click count picks the granularity, the same mapping every terminal
        // uses: drag, word, line.
        let ty = match event.click_count {
            0 => return,
            1 => SelectionType::Simple,
            2 => SelectionType::Semantic,
            _ => SelectionType::Lines,
        };
        let shift = event.modifiers.shift;
        if ty == SelectionType::Simple {
            // Shift+click extends an existing selection instead of replacing
            // it — the one gesture that reaches text off the bottom of a long
            // drag without redoing the whole thing.
            let extended = shift
                && self
                    .with_active_emulator(cx, |emu| {
                        let extend = emu.has_selection();
                        if extend {
                            emu.update_selection(point, side);
                        }
                        extend
                    })
                    .unwrap_or(false);
            if extended {
                self.selection_drag = Some(SelectionDrag {
                    origin: event.position,
                    position: event.position,
                    armed: true,
                });
                cx.notify();
                return;
            }
            // A plain press clears and arms; the selection itself only begins
            // once the pointer travels far enough to mean it.
            self.with_active_emulator(cx, |emu| emu.clear_selection());
            self.selection_drag = Some(SelectionDrag {
                origin: event.position,
                position: event.position,
                armed: false,
            });
        } else {
            // Word and line selections are complete on the press, so they need
            // no threshold — but keep the drag live so the pointer can extend
            // them at that granularity.
            self.with_active_emulator(cx, |emu| emu.start_selection(ty, point, side));
            self.selection_drag = Some(SelectionDrag {
                origin: event.position,
                position: event.position,
                armed: true,
            });
        }
        cx.notify();
    }

    pub(super) fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !event.dragging() {
            return;
        }
        let Some(mut drag) = self.selection_drag else {
            return;
        };
        drag.position = event.position;
        self.selection_drag = Some(drag);
        if !drag.armed {
            let dx = f32::from(event.position.x - drag.origin.x);
            let dy = f32::from(event.position.y - drag.origin.y);
            if dx.hypot(dy) < SELECTION_DRAG_THRESHOLD {
                return;
            }
            // Threshold tripped: anchor at the *press*, not here, so the
            // selection covers the whole gesture.
            let Some((anchor, side)) = self.grid_point_at(drag.origin, cx) else {
                return;
            };
            self.with_active_emulator(cx, |emu| {
                emu.start_selection(SelectionType::Simple, anchor, side)
            });
            self.selection_drag = Some(SelectionDrag {
                armed: true,
                ..drag
            });
        }
        let Some((point, side)) = self.grid_point_at(event.position, cx) else {
            return;
        };
        self.with_active_emulator(cx, |emu| emu.update_selection(point, side));
        cx.notify();
        self.schedule_selection_scroll(cx);
    }

    pub(super) fn on_mouse_up(
        &mut self,
        _event: &MouseUpEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.selection_drag = None;
        self.selection_scroll_task = None;
    }

    /// Copy the selection. Returns whether anything was copied, so the caller
    /// can decide whether to swallow the keystroke.
    pub(super) fn copy_selection(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(text) = self
            .with_active_emulator(cx, |emu| emu.selection_text())
            .flatten()
        else {
            return false;
        };
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        true
    }

    pub(super) fn scroll_active(&mut self, delta_lines: i32, cx: &mut Context<Self>) {
        if delta_lines == 0 {
            return;
        }
        let Some(chat) = self.selected_chat(cx) else {
            return;
        };
        let Some(tabs) = self.chats.get_mut(&chat) else {
            return;
        };
        let active = tabs.active;
        if let Some(tab) = tabs.tabs.get_mut(active) {
            tab.emulator.scroll(delta_lines);
            cx.notify();
        }
    }

    pub(super) fn schedule_selection_scroll(&mut self, cx: &mut Context<Self>) {
        if self.selection_scroll_task.is_some() {
            return;
        }
        let (Some(drag), Some(geometry)) = (self.selection_drag, self.geometry) else {
            return;
        };
        if !drag.armed || selection_scroll_lines(geometry, drag.position) == 0 {
            return;
        }
        self.selection_scroll_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(SELECTION_SCROLL_TICK_MS))
                .await;
            let _ = this.update(cx, |panel, cx| {
                panel.selection_scroll_task = None;
                panel.step_selection_scroll(cx);
            });
        }));
    }

    pub(super) fn step_selection_scroll(&mut self, cx: &mut Context<Self>) {
        let (Some(drag), Some(geometry)) = (self.selection_drag, self.geometry) else {
            return;
        };
        if !drag.armed {
            return;
        }
        let lines = selection_scroll_lines(geometry, drag.position);
        if lines == 0 {
            return;
        }
        self.scroll_active(lines, cx);
        if let Some((point, side)) = self.grid_point_at(drag.position, cx) {
            self.with_active_emulator(cx, |emu| emu.update_selection(point, side));
        }
        self.schedule_selection_scroll(cx);
    }

    /// Pin the rail to the rendered tab. The shared rail host methods run
    /// without an `&App`, so they cannot read the selected chat — render
    /// records the tab here and they resolve it by key. A switch also drops
    /// the rail's activity baseline, so the incoming tab's offset lands as a
    /// first observation rather than as scroll motion.
    pub(super) fn sync_rail_tab(&mut self, cx: &App) {
        let key = self.active_tab(cx).map(|tab| tab.key);
        if self.rail_tab_key != key {
            self.rail_tab_key = key;
            self.bar.clear_scroll_baseline();
        }
    }

    pub(super) fn rail_tab(&self) -> Option<&TerminalTab> {
        let key = self.rail_tab_key?;
        self.chats
            .values()
            .find_map(|tabs| tabs.tabs.iter().find(|tab| tab.key == key))
    }

    pub(super) fn rail_tab_mut(&mut self) -> Option<&mut TerminalTab> {
        let key = self.rail_tab_key?;
        self.chats
            .values_mut()
            .find_map(|tabs| tabs.tabs.iter_mut().find(|tab| tab.key == key))
    }

    /// The rendered grid's rail geometry in the shared metrics domain, plus
    /// the track's window y (the grid bounds — the rail strip spans the
    /// terminal body) and the scroll position in px from the top of the
    /// scrollback.
    pub(super) fn rail_frame(&self) -> Option<(MenuScrollbarMetrics, Pixels, f32)> {
        let geometry = self.geometry?;
        let tab = self.rail_tab()?;
        let (viewport, content, offset) = rail_parts(
            geometry.line_h,
            tab.emulator.rows(),
            tab.emulator.history_lines(),
            tab.emulator.display_offset(),
        );
        Some((
            MenuScrollbarMetrics::from_parts(viewport, content, offset)?,
            geometry.bounds.top(),
            offset,
        ))
    }

    /// Apply an engaged drag's target to the emulator. The shared target is a
    /// fraction of the scrollback from the TOP; the emulator's scroll-to API
    /// takes lines from the live bottom.
    pub(super) fn apply_rail_drag(&mut self, pointer_y: Pixels) -> bool {
        let Some(line_h) = self.geometry.map(|geometry| geometry.line_h) else {
            return false;
        };
        let Some((metrics, track_top, _)) = self.rail_frame() else {
            return false;
        };
        let Some(fraction) = self.bar.drag_target_in(&metrics, track_top, pointer_y) else {
            return false;
        };
        let lines = (((1.0 - fraction) * metrics.max_scroll) / line_h).round() as usize;
        let Some(tab) = self.rail_tab_mut() else {
            return false;
        };
        tab.emulator.scroll_to_offset(lines);
        true
    }

    pub(super) fn on_terminal_hover(
        &mut self,
        hovered: &bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.bar.set_list_hovered(*hovered) {
            cx.notify();
        }
    }

    pub(super) fn render_scrollbar(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        self.sync_rail_tab(cx);
        crate::popover::rail(self, "terminal-scrollbar", theme, cx)
    }
}
