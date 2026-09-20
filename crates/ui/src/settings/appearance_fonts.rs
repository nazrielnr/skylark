//! Font family and size controls, including keyboard navigation.

use super::*;

impl AppearancePage {
    pub(super) fn font_menu(&self, kind: FontKind) -> &Popup<()> {
        match kind {
            FontKind::Ui => &self.font_menu,
            FontKind::Terminal => &self.terminal_font_menu,
            FontKind::Code => &self.code_font_menu,
        }
    }

    pub(super) fn font_menu_mut(&mut self, kind: FontKind) -> &mut Popup<()> {
        match kind {
            FontKind::Ui => &mut self.font_menu,
            FontKind::Terminal => &mut self.terminal_font_menu,
            FontKind::Code => &mut self.code_font_menu,
        }
    }

    pub(super) fn font_list_mut(&mut self, kind: FontKind) -> &mut widgets::PageScroll {
        match kind {
            FontKind::Ui => &mut self.font_list,
            FontKind::Terminal => &mut self.terminal_font_list,
            FontKind::Code => &mut self.code_font_list,
        }
    }

    pub(super) fn selected_font(&self, kind: FontKind) -> &UiFontFamily {
        match kind {
            FontKind::Ui => &self.selected_font,
            FontKind::Terminal => &self.selected_terminal_font,
            FontKind::Code => &self.selected_code_font,
        }
    }

    pub(super) fn set_selected_font(&mut self, kind: FontKind, family: UiFontFamily) {
        match kind {
            FontKind::Ui => self.selected_font = family,
            FontKind::Terminal => self.selected_terminal_font = family,
            FontKind::Code => self.selected_code_font = family,
        }
    }

    pub(super) fn font_focus(&self, kind: FontKind) -> &FocusHandle {
        match kind {
            FontKind::Ui => &self.font_focus,
            FontKind::Terminal => &self.terminal_font_focus,
            FontKind::Code => &self.code_font_focus,
        }
    }

    pub(super) fn font_dismissed_at(&mut self, kind: FontKind) -> &mut Option<std::time::Instant> {
        match kind {
            FontKind::Ui => &mut self.font_menu_dismissed_at,
            FontKind::Terminal => &mut self.terminal_font_menu_dismissed_at,
            FontKind::Code => &mut self.code_font_menu_dismissed_at,
        }
    }

    pub(super) fn commit_font(&mut self, kind: FontKind, cx: &mut Context<Self>) {
        let family = self.selected_font(kind).clone();
        if kind.is_available_for(&typography::availability(cx), &family) {
            kind.apply_family(family, cx);
            let effective = kind.effective(cx);
            self.set_selected_font(kind, effective);
            self.close_font_menu(kind, cx);
            cx.notify();
        }
    }

    pub(super) fn size_menu(&self, kind: FontKind) -> &Popup<()> {
        match kind {
            FontKind::Ui => &self.size_menu,
            FontKind::Terminal => &self.terminal_size_menu,
            FontKind::Code => &self.code_size_menu,
        }
    }

    pub(super) fn size_menu_mut(&mut self, kind: FontKind) -> &mut Popup<()> {
        match kind {
            FontKind::Ui => &mut self.size_menu,
            FontKind::Terminal => &mut self.terminal_size_menu,
            FontKind::Code => &mut self.code_size_menu,
        }
    }

    pub(super) fn size_focus(&self, kind: FontKind) -> &FocusHandle {
        match kind {
            FontKind::Ui => &self.size_focus,
            FontKind::Terminal => &self.terminal_size_focus,
            FontKind::Code => &self.code_size_focus,
        }
    }

    pub(super) fn size_dismissed_at(&mut self, kind: FontKind) -> &mut Option<std::time::Instant> {
        match kind {
            FontKind::Ui => &mut self.size_menu_dismissed_at,
            FontKind::Terminal => &mut self.terminal_size_menu_dismissed_at,
            FontKind::Code => &mut self.code_size_menu_dismissed_at,
        }
    }

    /// Highlighted rung of this kind's size ladder.
    pub(super) fn selected_size_ix(&self, kind: FontKind) -> usize {
        match kind {
            FontKind::Ui => UiFontSize::ALL
                .iter()
                .position(|size| *size == self.selected_size)
                .unwrap_or_default(),
            FontKind::Terminal => nearest_mono_ix(self.selected_terminal_size),
            FontKind::Code => nearest_mono_ix(self.selected_code_size),
        }
    }

    pub(super) fn set_selected_size_ix(&mut self, kind: FontKind, ix: usize) {
        let ix = ix.min(kind.size_count() - 1);
        match kind {
            FontKind::Ui => self.selected_size = UiFontSize::ALL[ix],
            FontKind::Terminal => self.selected_terminal_size = MONO_FONT_SIZES[ix],
            FontKind::Code => self.selected_code_size = MONO_FONT_SIZES[ix],
        }
    }

    pub(super) fn commit_size(
        &mut self,
        kind: FontKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ix = self.selected_size_ix(kind);
        match kind {
            FontKind::Ui => {
                typography::set_font_size(UiFontSize::ALL[ix], window, cx);
                self.selected_size = typography::font_size(cx);
            }
            FontKind::Terminal => {
                let next = MONO_FONT_SIZES[ix];
                typography::set_terminal_font_size(next, cx);
                self.selected_terminal_size = next;
            }
            FontKind::Code => {
                let next = MONO_FONT_SIZES[ix];
                typography::set_code_font_size(next, cx);
                self.selected_code_size = next;
                // Open file editors only learn the new size through this event.
                cx.emit(AppearanceSettingsEvent::CodeFontSizeChanged(next));
            }
        }
        self.close_size_menu(kind, cx);
        cx.notify();
    }

    /// This kind's catalog, narrowed and ranked by the typed query.
    pub(super) fn visible_choices(
        &self,
        kind: FontKind,
        availability: &FontAvailability,
        cx: &gpui::App,
    ) -> Vec<UiFontFamily> {
        filter_families(
            self.font_search.read(cx).text(),
            kind.choices_for(availability),
        )
    }

    /// The kind whose menu is open, if any. Opening one closes the others.
    pub(super) fn open_font_kind(&self) -> Option<FontKind> {
        FontKind::ALL
            .into_iter()
            .find(|kind| self.font_menu(*kind).is_open())
    }

    pub(super) fn on_font_search_edited(&mut self, cx: &mut Context<Self>) {
        if let Some(kind) = self.open_font_kind() {
            self.clamp_highlight(kind, cx);
        }
    }

    /// Keep the highlighted row inside the filtered list, so the next Enter
    /// cannot commit a family the query has already filtered out of existence.
    pub(super) fn clamp_highlight(&mut self, kind: FontKind, cx: &mut Context<Self>) {
        let availability = typography::availability(cx);
        let visible = self.visible_choices(kind, &availability, cx);
        if !visible.contains(self.selected_font(kind)) {
            let next = if visible.is_empty() {
                kind.effective(cx)
            } else {
                first_available(&visible, kind, &availability)
            };
            self.set_selected_font(kind, next);
        }
        cx.notify();
    }

    pub(super) fn close_font_menu(&mut self, kind: FontKind, cx: &mut Context<Self>) {
        if !self.font_menu_mut(kind).begin_close() {
            return;
        }
        match kind {
            FontKind::Ui => popover::reap_popup(cx, |page| &mut page.font_menu),
            FontKind::Terminal => popover::reap_popup(cx, |page| &mut page.terminal_font_menu),
            FontKind::Code => popover::reap_popup(cx, |page| &mut page.code_font_menu),
        }
    }

    /// Only one of this page's six font dropdowns may be open at a time;
    /// opening any one closes the other five.
    pub(super) fn close_other_menus(
        &mut self,
        keep_family: Option<FontKind>,
        keep_size: Option<FontKind>,
        cx: &mut Context<Self>,
    ) {
        for kind in FontKind::ALL {
            if Some(kind) != keep_family {
                self.close_font_menu(kind, cx);
            }
            if Some(kind) != keep_size {
                self.close_size_menu(kind, cx);
            }
        }
    }

    pub(super) fn close_size_menu(&mut self, kind: FontKind, cx: &mut Context<Self>) {
        if !self.size_menu_mut(kind).begin_close() {
            return;
        }
        match kind {
            FontKind::Ui => popover::reap_popup(cx, |page| &mut page.size_menu),
            FontKind::Terminal => popover::reap_popup(cx, |page| &mut page.terminal_size_menu),
            FontKind::Code => popover::reap_popup(cx, |page| &mut page.code_size_menu),
        }
    }

    pub(super) fn dismiss_font_menu(&mut self, kind: FontKind, cx: &mut Context<Self>) {
        *self.font_dismissed_at(kind) = Some(std::time::Instant::now());
        self.close_font_menu(kind, cx);
    }

    pub(super) fn dismiss_size_menu(&mut self, kind: FontKind, cx: &mut Context<Self>) {
        *self.size_dismissed_at(kind) = Some(std::time::Instant::now());
        self.close_size_menu(kind, cx);
    }

    pub(super) fn toggle_font_menu(
        &mut self,
        kind: FontKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_other_menus(Some(kind), None, cx);
        let just_dismissed = self
            .font_dismissed_at(kind)
            .take()
            .is_some_and(|at| at.elapsed() < std::time::Duration::from_millis(400));
        if self.font_menu(kind).is_open() {
            self.close_font_menu(kind, cx);
        } else if !just_dismissed {
            // Clear before opening: the resulting `Edited` finds no open menu,
            // so it cannot clobber the highlight we anchor on the next line.
            self.font_search
                .update(cx, |input, cx| input.set_text("", cx));
            let effective = kind.effective(cx);
            self.set_selected_font(kind, effective);
            // Every open starts at the top, and the rail's baseline with it —
            // no reopen flash from the previous session's offset.
            self.font_list_mut(kind).reset();
            self.font_menu_mut(kind).open(());
            window.focus(&self.font_search.read(cx).focus_handle(cx), cx);
        }
        cx.notify();
    }

    pub(super) fn toggle_size_menu(&mut self, kind: FontKind, cx: &mut Context<Self>) {
        self.close_other_menus(None, Some(kind), cx);
        let just_dismissed = self
            .size_dismissed_at(kind)
            .take()
            .is_some_and(|at| at.elapsed() < std::time::Duration::from_millis(400));
        if self.size_menu(kind).is_open() {
            self.close_size_menu(kind, cx);
        } else if !just_dismissed {
            self.set_selected_size_ix(kind, kind.size_ix(cx));
            self.size_menu_mut(kind).open(());
        }
        cx.notify();
    }

    /// Open on the first navigation key, so ↑↓/Home/End work from the closed
    /// trigger exactly as they do inside the list.
    pub(super) fn open_size_menu(&mut self, kind: FontKind, cx: &mut Context<Self>) {
        if !self.size_menu(kind).is_open() {
            *self.size_dismissed_at(kind) = None;
            self.toggle_size_menu(kind, cx);
        }
    }

    /// Returns whether the key was consumed. The card and its trigger both
    /// listen, and the trigger's guard re-reads `is_open` — which Enter has
    /// already flipped — so a consumed key must not bubble on.
    pub(super) fn on_font_key_down(
        &mut self,
        kind: FontKind,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let key = event.keystroke.key.as_str();

        if !self.font_menu(kind).is_open() {
            // Closed: any list key opens the menu, and the filter takes over
            // from there — no stepping through hundreds of rows by hand.
            if matches!(
                key,
                "up" | "down" | "left" | "right" | "home" | "end" | "enter" | "space"
            ) {
                *self.font_dismissed_at(kind) = None;
                self.toggle_font_menu(kind, window, cx);
                return true;
            }
            return false;
        }

        // Open: the filter input owns text and caret keys. Only navigation,
        // commit, and dismiss bubble out to us.
        let availability = typography::availability(cx);
        let choices = self.visible_choices(kind, &availability, cx);
        let modifiers = event.keystroke.modifiers;
        let next = match (
            key,
            popover::classify_key(key, modifiers.platform, modifiers.control),
        ) {
            (_, popover::MenuKey::Up) => {
                step_font(self.selected_font(kind), -1, &choices, kind, &availability)
            }
            (_, popover::MenuKey::Down) => {
                step_font(self.selected_font(kind), 1, &choices, kind, &availability)
            }
            ("home", _) => first_available(&choices, kind, &availability),
            ("end", _) => last_available(&choices, kind, &availability),
            (_, popover::MenuKey::Enter) => {
                self.commit_font(kind, cx);
                return true;
            }
            (_, popover::MenuKey::Escape) => {
                let effective = kind.effective(cx);
                self.set_selected_font(kind, effective);
                self.close_font_menu(kind, cx);
                window.focus(&self.font_focus(kind).clone(), cx);
                cx.notify();
                return true;
            }
            _ => return false,
        };
        self.set_selected_font(kind, next);
        cx.notify();
        true
    }

    pub(super) fn on_size_key_down(
        &mut self,
        kind: FontKind,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let last = kind.size_count() - 1;
        let current = self.selected_size_ix(kind);
        match event.keystroke.key.as_str() {
            "up" | "left" => {
                self.open_size_menu(kind, cx);
                self.set_selected_size_ix(kind, current.saturating_sub(1));
                cx.notify();
            }
            "down" | "right" => {
                self.open_size_menu(kind, cx);
                self.set_selected_size_ix(kind, current + 1);
                cx.notify();
            }
            "home" => {
                self.open_size_menu(kind, cx);
                self.set_selected_size_ix(kind, 0);
                cx.notify();
            }
            "end" => {
                self.open_size_menu(kind, cx);
                self.set_selected_size_ix(kind, last);
                cx.notify();
            }
            "enter" | "space" => {
                if self.size_menu(kind).is_open() {
                    self.commit_size(kind, window, cx);
                } else {
                    *self.size_dismissed_at(kind) = None;
                    self.toggle_size_menu(kind, cx);
                }
            }
            "escape" => {
                self.set_selected_size_ix(kind, kind.size_ix(cx));
                self.close_size_menu(kind, cx);
                cx.notify();
            }
            _ => {}
        }
    }
}
