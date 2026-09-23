p = 'crates/ui/src/files/tree_view.rs'
s = open(p, encoding='utf8').read()
s = s.replace("    search_typography_generation: u32,\n    _search_events: Subscription,",
              "    pub(super) search_typography_generation: u32,\n    _search_events: Subscription,")
open(p, 'w', encoding='utf8', newline='\n').write(s)
print("tv ok")

p2 = 'crates/ui/src/files/mod.rs'
t = open(p2, encoding='utf8').read()

# 1. constructor: remove the search creation + subscription
old = """        let search = cx.new(|cx| {
            ComposerInput::new("Search files", cx)
                .with_accessibility_role(gpui::Role::SearchInput)
                .with_text_metrics(12.0, 18.0)
        });
        let search_events = cx.subscribe(&search, |this: &mut Self, _, event, cx| match event {
            ComposerInputEvent::Edited => this.on_search_edited(cx),
            ComposerInputEvent::Submitted
            | ComposerInputEvent::ModifiedSubmitted
            | ComposerInputEvent::MentionAccept => this.activate_search_result(cx),
            ComposerInputEvent::MentionNavigate(delta) => {
                let len = this.search_state.visible_len();
                if len > 0 {
                    this.search_state.active = if *delta < 0 {
                        this.search_state.active.saturating_sub(1)
                    } else {
                        (this.search_state.active + 1).min(len - 1)
                    };
                    this.search_list
                        .scroll_to_reveal_item(this.search_state.active);
                    cx.notify();
                }
            }
            ComposerInputEvent::MentionDismiss => this.clear_search(cx),
            ComposerInputEvent::PastedImages(_)
            | ComposerInputEvent::PastedPaths(_)
            | ComposerInputEvent::CursorMoved
            | ComposerInputEvent::ViewportChanged => {}
        });"""
assert old in t, "constructor search block not found"
t = t.replace(old, "", 1)

# 2. constructor field inits
old = """            search,
            search_state: FileSearchState::default(),
            search_list: ListState::new(0, ListAlignment::Top, px(420.0)),
            search_typography_generation: 0,
"""
assert old in t
t = t.replace(old, "", 1)
old = "            search_open: false,\n"
if old in t:
    t = t.replace(old, "", 1)
old = "            _search_events: search_events,\n"
assert old in t
t = t.replace(old, "", 1)

# 3. struct fields
old = """    search: Entity<ComposerInput>,
    search_state: FileSearchState,
    search_list: ListState,
    search_typography_generation: u32,
"""
assert old in t, "struct fields not found"
t = t.replace(old, "", 1)
old = "    pub(super) search_open: bool,\n"
assert old in t
t = t.replace(old, "", 1)
old = "    _search_events: Subscription,\n"
assert old in t
t = t.replace(old, "", 1)

# 4. render content branch
old = """        let content = if !self.search_state.query.is_empty() {
            self.render_search_results(cx)
        } else if let Some(error) = tree_error.filter(|_| !tree_has_content) {"""
new = """        // NOTE: the fuzzy search results render INSIDE the shared tree view
        // (its render swaps rows for results), so no surface-side branch.
        let content = if let Some(error) = tree_error.filter(|_| !tree_has_content) {"""
assert old in t, "content branch not found"
t = t.replace(old, new, 1)

# 5. split branch: remove the search_row plumbing
old = """            let search_open = self.search_open || !self.search_state.query.is_empty();
            let search_row = search_open.then(|| self.render_search_input_row(&theme));
            let collapsed = self"""
new = """            let collapsed = self"""
assert old in t, "split search plumbing not found"
t = t.replace(old, new, 1)
old = """                            .flex()
                            .flex_col()
                            .when_some(search_row, |el, row| el.child(row))
                            .child(tree_pane),"""
new = """                            .flex()
                            .flex_col()
                            .child(tree_pane),"""
assert old in t, "split when_some not found"
t = t.replace(old, new, 1)

# 6. header: is_search_open via tree
old = """        let is_search_open = self.search_open || !self.search_state.query.is_empty();"""
new = """        let is_search_open = self
            .tree_view
            .read_with(cx, |tree, _| tree.is_search_open());"""
assert old in t, "header is_search_open not found"
t = t.replace(old, new, 1)

# 7. header: drop the raw search-row block (tree renders it)
old = """            .when(is_search_open && !is_split, |parent| {
                parent.child(self.render_search_input_row(&theme))
            })
    }

    /// The file-search input row. In the raw workspace it sits under the
    /// toolbar; in the split sidebar it rides the tree overlay's body (the
    /// fixed-height header slot would clip it) so the search field is
    /// available in BOTH layouts. No background: it blends with the pane.
    pub(super) fn render_search_input_row(&self, theme: &crate::theme::Theme) -> gpui::Div {
        div()
            .w_full()
            .px(crate::typography::ui_rems(8.0))
            .py(crate::typography::ui_rems(4.0))
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .h(crate::typography::ui_rems(
                        crate::surface_chrome::CONTROL_SIZE,
                    ))
                    .min_w_0()
                    .flex_1()
                    .px(crate::typography::ui_rems(8.0))
                    .rounded(crate::typography::ui_rems(
                        crate::surface_chrome::CONTROL_RADIUS,
                    ))
                    .flex()
                    .items_center()
                    .gap(crate::typography::ui_rems(6.0))
                    .text_size(crate::typography::ui_rems(12.0))
                    .child(
                        crate::icons::icon(crate::icons::MAGNIFER)
                            .size(crate::typography::ui_rems(13.0))
                            .flex_none()
                            .text_color(theme.text_faint),
                    )
                    .child(div().min_w_0().flex_1().child(self.search.clone())),
            )
            .relative()
            .child(crate::ui_trace::bounds_probe("search-input"))
    }
}"""
new = """    }
}"""
assert old in t, "raw search row block not found"
t = t.replace(old, new, 1)

# 8. set_show_all_files
old = """    pub fn set_show_all_files(&mut self, _show_all_files: bool, cx: &mut Context<Self>) {
        // The tree side lives on the shared FileTreeView; the shell applies
        // the setting there. Only the surface-local search filter resets.
        if !self.search_state.query.is_empty() {
            self.search_state.query.clear();
            self.on_search_edited(cx);
        }
    }"""
new = """    pub fn set_show_all_files(&mut self, _show_all_files: bool, cx: &mut Context<Self>) {
        // The tree side AND the search live on the shared FileTreeView; the
        // shell applies the setting there, and the search clears there.
        self.tree_view.update(cx, |tree, cx| tree.clear_search(cx));
    }"""
assert old in t, "set_show_all_files not found"
t = t.replace(old, new, 1)

# 9. toggle_search delegate
old = """    pub(super) fn toggle_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        crate::ui_trace!("toggle-search open={}", !self.search_open);
        self.search_open = !self.search_open;
        if self.search_open {
            self.search.update(cx, |input, cx| {
                input.focus_handle.focus(window, cx);
            });
        } else {
            // Closing must also clear the query: the search UI stays up
            // while the query is non-empty, so without this the close
            // button looked broken.
            self.search.update(cx, |search, cx| search.set_text("", cx));
        }
        cx.notify();
    }"""
new = """    pub(super) fn toggle_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The search lives on the shared tree view — one state per panel.
        self.tree_view
            .update(cx, |tree, cx| tree.toggle_search(window, cx));
    }"""
assert old in t, "toggle_search not found"
t = t.replace(old, new, 1)

open(p2, 'w', encoding='utf8', newline='\n').write(t)
print("mod ok")
