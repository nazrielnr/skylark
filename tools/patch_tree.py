p = 'crates/ui/src/files/tree_view.rs'
s = open(p, encoding='utf8').read()

# 1. imports
old = """use gpui::{
    AnyElement, Context, Entity, EventEmitter, FocusHandle, KeyDownEvent, MouseButton,
    ScrollStrategy, SharedString, Task, UniformListScrollHandle, Window, div, prelude::*, px,
    uniform_list,
};"""
new = """use gpui::{
    AnyElement, Context, Entity, EventEmitter, FocusHandle, KeyDownEvent, ListAlignment,
    ListState, MouseButton, ScrollStrategy, SharedString, Subscription, Task,
    UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};"""
assert old in s, "gpui imports not found"
s = s.replace(old, new, 1)

old = """use crate::{file_icons::{self, FileIconIdentity}, popover, theme::Theme};
use crate::state::AppState;"""
new = """use super::search::{FileSearchState, search_row_height};
use crate::composer::{ComposerInput, ComposerInputEvent};
use crate::{file_icons::{self, FileIconIdentity}, popover, theme::Theme};
use crate::state::AppState;"""
assert old in s, "crate imports not found"
s = s.replace(old, new, 1)

# 2. fields
old = """    /// Whether the sidebar is collapsed, SHARED so the state survives tab
    /// switches and returns from the raw workspace (user request: the
    /// collapse must be remembered).
    sidebar_collapsed: bool,"""
new = """    /// Whether the sidebar is collapsed, SHARED so the state survives tab
    /// switches and returns from the raw workspace (user request: the
    /// collapse must be remembered).
    sidebar_collapsed: bool,
    /// The workspace file search — SHARED, like everything else on this
    /// view: the input, the query, the results, and the list state are ONE
    /// per panel, so switching between the raw browser and file tabs (or
    /// pressing Files mid-search) never resets it.
    search: Entity<ComposerInput>,
    search_state: FileSearchState,
    search_list: ListState,
    search_open: bool,
    search_typography_generation: u32,
    _search_events: Subscription,"""
assert old in s, "fields anchor not found"
s = s.replace(old, new, 1)

# 3. constructor: create search + subscribe
old = """            sidebar_width: TREE_SPLIT_DEFAULT,
            sidebar_collapsed: false,
            tree_scroll: UniformListScrollHandle::new(),
            tree_focus,
            tree_bar: popover::MenuScrollbarState::default(),
        };
        cx.notify();
        surface
    }"""
new = """            sidebar_width: TREE_SPLIT_DEFAULT,
            sidebar_collapsed: false,
            search,
            search_state: FileSearchState::default(),
            search_list: ListState::new(0, ListAlignment::Top, px(420.0)),
            search_open: false,
            search_typography_generation: 0,
            _search_events: search_events,
            tree_scroll: UniformListScrollHandle::new(),
            tree_focus,
            tree_bar: popover::MenuScrollbarState::default(),
        };
        cx.notify();
        surface
    }"""
assert old in s, "constructor tail not found"
s = s.replace(old, new, 1)

old = """    ) -> Self {
        let tree_focus = cx.focus_handle();
        let surface = Self {"""
new = """    ) -> Self {
        let tree_focus = cx.focus_handle();
        let search = cx.new(|cx| {
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
        });
        let surface = Self {"""
assert old in s, "constructor head not found"
s = s.replace(old, new, 1)

# 4. toggle + is_search_open accessors (near the collapse accessors)
old = """    /// The shared collapse state (see the field docs).
    pub fn sidebar_collapsed(&self) -> bool {
        self.sidebar_collapsed
    }"""
new = """    /// The shared collapse state (see the field docs).
    pub fn sidebar_collapsed(&self) -> bool {
        self.sidebar_collapsed
    }

    /// Whether the search UI is up (the toggle opened it or a query is
    /// active) — read by the surfaces' header buttons.
    pub fn is_search_open(&self) -> bool {
        self.search_open || !self.search_state.query.is_empty()
    }

    /// Toggle the shared search field. Closing also clears the query: the
    /// search UI stays up while the query is non-empty, so without this
    /// the close button looked broken.
    pub fn toggle_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        crate::ui_trace!("toggle-search open={}", !self.search_open);
        self.search_open = !self.search_open;
        if self.search_open {
            self.search.update(cx, |input, cx| {
                input.focus_handle.focus(window, cx);
            });
        } else {
            self.clear_search(cx);
        }
        cx.notify();
    }"""
assert old in s, "sidebar_collapsed accessor not found"
s = s.replace(old, new, 1)

# 5. on_key_down: guard while searching
old = """    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let handled = match event.keystroke.key.as_str() {"""
new = """    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // While searching, the input owns the keyboard (its mention
        // navigation moves the result selection); tree key nav is off.
        if !self.search_state.query.is_empty() {
            return;
        }
        let handled = match event.keystroke.key.as_str() {"""
assert old in s, "on_key_down not found"
s = s.replace(old, new, 1)

# 6. render: [search row?] + [results | tree] + rail
old = """impl Render for FileTreeView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let scrollbar = popover::rail(self, "files-tree-scrollbar", &theme, cx);
        let row_count = self.tree.visible_rows().len();
        if crate::ui_trace::enabled() {
            let rows = self
                .tree
                .visible_rows()
                .iter()
                .take(24)
                .map(|row| {
                    let kind = self
                        .tree
                        .node(&row.path)
                        .map(|node| {
                            if node.entry.kind == WorkspaceEntryKind::Directory {
                                'd'
                            } else {
                                'f'
                            }
                        })
                        .unwrap_or('?');
                    format!("{}:{}", kind, row.path)
                })
                .collect::<Vec<_>>()
                .join("|");
            eprintln!(
                "[trace] tree-rows count={} L={:.0} T={:.0} R={:.0} B={:.0} content_h={:.0} paths={}",
                row_count,
                f32::from(b.left()),
                f32::from(b.top()),
                f32::from(b.right()),
                f32::from(b.bottom()),
                item_h,
                rows
            );
        }
        div()
            .id("files-tree")
            .role(gpui::Role::Tree)
            .aria_label("Workspace file tree")
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .track_focus(&self.tree_focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.tree_focus.focus(window, cx)),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.on_key_down(event, window, cx);
            }))
            .child(
                uniform_list(
                    "files-tree-list",
                    row_count,
                    cx.processor(Self::render_tree_rows),
                )
                .flex_1()
                .min_h_0()
                .track_scroll(&self.tree_scroll),
            )
            .children(scrollbar)
    }
}"""
new = """impl Render for FileTreeView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        // The workspace view: while a query is active, the fuzzy RESULTS
        // replace the tree rows — one shared state, rendered identically
        // in the raw browser and every file tab's sidebar.
        let searching = !self.search_state.query.is_empty();
        let search_open = self.search_open || searching;
        let scrollbar = if searching {
            None
        } else {
            popover::rail(self, "files-tree-scrollbar", &theme, cx)
        };
        let row_count = self.tree.visible_rows().len();
        if crate::ui_trace::enabled() && !searching {
            let b = self.tree_scroll.0.borrow().base_handle.bounds();
            let item_h = self
                .tree_scroll
                .0
                .borrow()
                .last_item_size
                .map(|size| f32::from(size.contents.height))
                .unwrap_or(0.0);
            let rows = self
                .tree
                .visible_rows()
                .iter()
                .take(24)
                .map(|row| {
                    let kind = self
                        .tree
                        .node(&row.path)
                        .map(|node| {
                            if node.entry.kind == WorkspaceEntryKind::Directory {
                                'd'
                            } else {
                                'f'
                            }
                        })
                        .unwrap_or('?');
                    format!("{}:{}", kind, row.path)
                })
                .collect::<Vec<_>>()
                .join("|");
            eprintln!(
                "[trace] tree-rows count={} L={:.0} T={:.0} R={:.0} B={:.0} content_h={:.0} paths={}",
                row_count,
                f32::from(b.left()),
                f32::from(b.top()),
                f32::from(b.right()),
                f32::from(b.bottom()),
                item_h,
                rows
            );
        }
        let content: AnyElement = if searching {
            self.render_search_results(cx)
        } else {
            uniform_list(
                "files-tree-list",
                row_count,
                cx.processor(Self::render_tree_rows),
            )
            .flex_1()
            .min_h_0()
            .track_scroll(&self.tree_scroll)
            .into_any_element()
        };
        let search_row = search_open.then(|| self.render_search_row(&theme));
        div()
            .id("files-tree")
            .role(gpui::Role::Tree)
            .aria_label("Workspace file tree")
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .track_focus(&self.tree_focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.tree_focus.focus(window, cx)),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.on_key_down(event, window, cx);
            }))
            .when_some(search_row, |el, row| el.child(row))
            .child(content)
            .children(scrollbar)
    }

    /// The search input row: rides the top of the workspace view in BOTH
    /// layouts (raw browser below the toolbar, split sidebar at the top of
    /// the overlay body). No background — it blends with the pane.
    fn render_search_row(&self, theme: &Theme) -> gpui::Div {
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
assert old in s, "render block not found"
s = s.replace(old, new, 1)

open(p, 'w', encoding='utf8', newline='\n').write(s)
print("tree_view ok")
