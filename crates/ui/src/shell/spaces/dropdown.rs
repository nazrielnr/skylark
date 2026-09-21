//! Spaces menu and sidebar view options dropdowns.
//!
//! Hosts the space-filter dropdown ("All projects", search input, space rows,
//! and pinned "New project…" action) and the sidebar view options menu
//! (organization, sorting, display toggles, compact mode).

use super::*;
use crate::composer::{ComposerInput, ComposerInputEvent};
use crate::icons::{self, icon};
use crate::motion;
use crate::popover;
use crate::shell::{SidebarOrganization, SidebarSort};
use crate::theme::Theme;
use chrono::Utc;
use gpui::{
    div, px, AnyElement, App, Context, Entity, FocusHandle, MouseButton, MouseDownEvent,
    Render, SharedString, Subscription, Window,
};

/// The space-filter dropdown, `Some` while open. The same searchable-menu
/// recipe as the composer's ref picker: filter input on top
/// (`PaletteSearch` context so ↑↓/↵ bubble to the card), ranked substring
/// rows, keyboard highlight.
pub(in crate::shell) struct SpacesMenu {
    pub(in crate::shell) search: Entity<ComposerInput>,
    /// Keyboard highlight — an index into [`Shell::spaces_menu_rows`], or
    /// that list's length when the pinned "New project…" footer holds it.
    pub(in crate::shell) active: usize,
    /// Tracked on the card — puts it on the keyboard dispatch path while the
    /// search input holds focus (the structure every working picker uses).
    pub(in crate::shell) focus: FocusHandle,
    pub(in crate::shell) list_scroll: gpui::ScrollHandle,
    pub(in crate::shell) _search_events: Subscription,
}

pub(in crate::shell) struct SidebarViewMenu {
    /// Keyboard cursor. Mouse-opened menus start without one so the persisted
    /// radio/check state is the only selection signal until an arrow key is
    /// pressed.
    pub(in crate::shell) active: Option<usize>,
    pub(in crate::shell) focus: FocusHandle,
}

struct SidebarViewOptionsTooltip;

impl Render for SidebarViewOptionsTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        div()
            .px(crate::typography::ui_rems(8.0))
            .py(crate::typography::ui_rems(6.0))
            .rounded(crate::typography::ui_rems(6.0))
            .border_1()
            .border_color(theme.border_strong)
            .bg(theme.surface_raised)
            .shadow_md()
            .text_size(crate::typography::ui_rems(11.0))
            .text_color(theme.text)
            .child("Sidebar view options")
    }
}

#[derive(Clone, Copy)]
enum SidebarViewRow {
    ByProject,
    Compact,
    ShowProjectIcon,
    ShowProjectLabel,
    ByDevice,
    InOneList,
    LastUpdated,
    Created,
    ShowBranch,
    ShowPullRequest,
    ShowHarness,
}

impl SidebarViewRow {
    /// Radio-style presentation choices behave like the project selector and
    /// dismiss after selection. Show toggles stay open for batch changes.
    fn closes_menu(self) -> bool {
        matches!(
            self,
            Self::ByProject | Self::ByDevice | Self::InOneList | Self::LastUpdated | Self::Created
        )
    }
}

const SIDEBAR_VIEW_ROWS: [SidebarViewRow; 11] = [
    SidebarViewRow::ByDevice,
    SidebarViewRow::ByProject,
    SidebarViewRow::InOneList,
    SidebarViewRow::LastUpdated,
    SidebarViewRow::Created,
    SidebarViewRow::ShowBranch,
    SidebarViewRow::ShowPullRequest,
    SidebarViewRow::ShowHarness,
    SidebarViewRow::ShowProjectIcon,
    SidebarViewRow::ShowProjectLabel,
    SidebarViewRow::Compact,
];

/// One activatable row of the open dropdown, in nav order. `AddSpace` names
/// the card's pinned "New project…" footer, not a list row — keyboard nav
/// maps the list-length index to it.
#[derive(Clone, PartialEq)]
pub(in crate::shell) enum SpacesMenuRow {
    All,
    Space(String),
    AddSpace,
}

// Handle-based rail host for the spaces dropdown: its list is a plain
// tracked scroller, so the trait's default metrics/press/drag (off the live
// ScrollHandle) apply unchanged.
impl popover::ScrollRailHost for Shell {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        &mut self.spaces_menu_bar
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.spaces_menu.get().map(|menu| menu.list_scroll.clone())
    }
}

impl Shell {
    pub(in crate::shell) fn set_space_filter(
        &mut self,
        filter: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.settings.space_filter != filter {
            self.cancel_pinned_session_drag(cx);
        }
        self.settings.space_filter = filter.clone();
        if let Some(space_id) = filter
            && self.state.read(cx).selected_chat.is_none()
        {
            self.state
                .update(cx, |s, cx| s.select_space(Some(space_id), cx));
        }
        self.close_spaces_menu(cx);
        self.schedule_save(cx);
        cx.notify();
    }

    /// Close the space-filter dropdown through the exit animation (no-op when
    /// it isn't open). Every close path funnels here so the menu always
    /// animates out instead of vanishing.
    pub(in crate::shell) fn close_spaces_menu(&mut self, cx: &mut Context<Self>) {
        if self.spaces_menu.begin_close() {
            popover::reap_popup(cx, |shell: &mut Self| &mut shell.spaces_menu);
            cx.notify();
        }
    }

    fn on_spaces_menu_list_hover(
        &mut self,
        hovered: &bool,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.spaces_menu_bar.set_list_hovered(*hovered) {
            cx.notify();
        }
    }

    /// Open the new-session canvas in a just-added space, preserving the
    /// sidebar's current project filter.
    pub(in crate::shell) fn land_in_space(&mut self, space_id: String, cx: &mut Context<Self>) {
        self.cancel_pinned_session_drag(cx);
        self.route = Route::Chat;
        self.focus_composer(cx);
        // "All" stays as-is; an explicit project filter follows the new
        // project so the first send lands in a visible session.
        if self.settings.space_filter.is_some() {
            self.settings.space_filter = Some(space_id.clone());
        }
        self.settings.last_space_id = Some(space_id.clone());
        self.state.update(cx, |s, cx| {
            s.select_space(Some(space_id), cx);
            s.select_chat(None, cx);
        });
        self.schedule_save(cx);
        cx.notify();
    }

    /// The filter's scrollable rows: "All projects", then spaces matching
    /// the search (ranked — `popover::filter_indices`). "All" only shows on
    /// an empty query (searching means hunting a space). The "New project…"
    /// action is not a row here — the card renders it as a pinned footer.
    fn spaces_menu_rows(&self, cx: &App) -> Vec<SpacesMenuRow> {
        let query = self
            .spaces_menu
            .get()
            .map(|menu| menu.search.read(cx).text().to_string())
            .unwrap_or_default();
        let state = self.state.read(cx);
        let spaces = state.spaces_sorted();
        let names: Vec<String> = spaces
            .iter()
            .map(|s| s.display_name().to_string())
            .collect();
        let mut rows: Vec<SpacesMenuRow> = Vec::new();
        if query.trim().is_empty() {
            rows.push(SpacesMenuRow::All);
        }
        rows.extend(
            popover::filter_indices(&query, &names)
                .into_iter()
                .map(|ix| SpacesMenuRow::Space(spaces[ix].id.clone())),
        );
        rows
    }

    pub(in crate::shell) fn open_spaces_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_sidebar_view_menu(cx);
        // "PaletteSearch" context: ↑↓/↵ stay unbound in the input and bubble
        // to the card's key handler.
        let search =
            cx.new(|cx| ComposerInput::with_context("Search projects…", "PaletteSearch", cx));
        let search_events = cx.subscribe(&search, |this: &mut Shell, _, event, cx| {
            if matches!(event, ComposerInputEvent::Edited) {
                if let Some(menu) = this.spaces_menu.open_mut() {
                    menu.active = 0;
                }
                cx.notify();
            }
        });
        // The highlight starts ON the current filter row.
        let current = self.settings.space_filter.clone();
        let handle = search.read(cx).focus_handle(cx);
        self.spaces_menu.open(SpacesMenu {
            search,
            active: 0,
            focus: cx.focus_handle(),
            list_scroll: gpui::ScrollHandle::new(),
            _search_events: search_events,
        });
        // Fresh handle at the top — don't let the stale rail baseline read
        // the reopen as scrolling.
        self.spaces_menu_bar.clear_scroll_baseline();
        let rows = self.spaces_menu_rows(cx);
        let start = match &current {
            None => 0,
            Some(id) => rows
                .iter()
                .position(|row| matches!(row, SpacesMenuRow::Space(s) if s == id))
                .unwrap_or(0),
        };
        if let Some(menu) = self.spaces_menu.open_mut() {
            menu.active = start;
        }
        // Focusable before first paint (the add-space palette's proven order).
        window.focus(&handle, cx);
        cx.notify();
    }

    fn activate_spaces_menu_row(&mut self, row: SpacesMenuRow, cx: &mut Context<Self>) {
        match row {
            SpacesMenuRow::All => self.set_space_filter(None, cx),
            SpacesMenuRow::Space(id) => self.set_space_filter(Some(id), cx),
            SpacesMenuRow::AddSpace => {
                self.close_spaces_menu(cx);
                self.open_add_space(cx);
            }
        }
    }

    /// Dropdown keys (bubbling from the focused search input): ↑↓ navigate,
    /// ↵ activates the highlighted row, esc closes.
    fn spaces_menu_key(&mut self, event: &gpui::KeyDownEvent, cx: &mut Context<Self>) {
        // The card stays mounted (and focused) through the exit animation —
        // keys must not drive a dying menu.
        if !self.spaces_menu.is_open() {
            return;
        }
        let key = popover::classify_key(
            event.keystroke.key.as_str(),
            event.keystroke.modifiers.platform,
            event.keystroke.modifiers.control,
        );
        match key {
            popover::MenuKey::Escape => {
                self.close_spaces_menu(cx);
                cx.stop_propagation();
            }
            popover::MenuKey::Up | popover::MenuKey::Down => {
                let rows = self.spaces_menu_rows(cx);
                // +1: the pinned footer stays in the nav order, exactly as
                // when it was the list's last row.
                let count = rows.len() + 1;
                let delta = if key == popover::MenuKey::Up { -1 } else { 1 };
                if let Some(menu) = self.spaces_menu.open_mut() {
                    menu.active = popover::menu_step(Some(menu.active), count, delta).unwrap_or(0);
                    // The footer renders below the scroller — only in-list
                    // rows can be scrolled to (the footer index would leave
                    // a request pending against a row that never exists).
                    if menu.active < rows.len() {
                        menu.list_scroll.scroll_to_item(menu.active);
                    }
                    cx.notify();
                }
            }
            popover::MenuKey::Enter | popover::MenuKey::ModEnter => {
                let active = self.spaces_menu.get().map(|m| m.active).unwrap_or(0);
                let rows = self.spaces_menu_rows(cx);
                // One past the scrollable rows is the pinned footer.
                let row = if active < rows.len() {
                    rows[active].clone()
                } else {
                    SpacesMenuRow::AddSpace
                };
                self.activate_spaces_menu_row(row, cx);
            }
            popover::MenuKey::Backspace | popover::MenuKey::Other => {}
        }
    }

    pub(in crate::shell) fn close_sidebar_view_menu(&mut self, cx: &mut Context<Self>) {
        if self.sidebar_view_menu.begin_close() {
            popover::reap_popup(cx, |shell: &mut Self| &mut shell.sidebar_view_menu);
            cx.notify();
        }
    }

    pub(in crate::shell) fn open_sidebar_view_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_spaces_menu(cx);
        let focus = cx.focus_handle();
        self.sidebar_view_menu.open(SidebarViewMenu {
            active: None,
            focus: focus.clone(),
        });
        window.focus(&focus, cx);
        cx.notify();
    }

    fn activate_sidebar_view_row(&mut self, row: SidebarViewRow, cx: &mut Context<Self>) {
        self.cancel_sidebar_session_transfer(cx);
        self.cancel_pinned_session_drag(cx);
        match row {
            SidebarViewRow::ByProject => {
                self.settings.sidebar_organization = SidebarOrganization::ByProject
            }
            SidebarViewRow::ShowProjectLabel => {
                self.settings.sidebar_show_project_label = !self.settings.sidebar_show_project_label
            }
            SidebarViewRow::Compact => {
                self.settings.sidebar_compact = !self.settings.sidebar_compact
            }
            SidebarViewRow::ShowProjectIcon => {
                self.settings.sidebar_show_project_icon = !self.settings.sidebar_show_project_icon
            }
            SidebarViewRow::ByDevice => {
                self.settings.sidebar_organization = SidebarOrganization::ByDevice
            }
            SidebarViewRow::InOneList => {
                self.settings.sidebar_organization = SidebarOrganization::InOneList
            }
            SidebarViewRow::LastUpdated => self.settings.sidebar_sort = SidebarSort::LastUpdated,
            SidebarViewRow::Created => self.settings.sidebar_sort = SidebarSort::Created,
            SidebarViewRow::ShowBranch => {
                self.settings.sidebar_show_branch = !self.settings.sidebar_show_branch
            }
            SidebarViewRow::ShowPullRequest => {
                self.settings.sidebar_show_pull_request = !self.settings.sidebar_show_pull_request;
                let visible = self.settings.sidebar_show_pull_request;
                self.state.update(cx, |state, cx| {
                    state.set_change_requests_visible(visible, cx)
                });
            }
            SidebarViewRow::ShowHarness => {
                self.settings.sidebar_show_harness = !self.settings.sidebar_show_harness
            }
        }
        self.schedule_save(cx);
        if row.closes_menu() {
            self.close_sidebar_view_menu(cx);
        }
        cx.notify();
    }

    fn sidebar_view_menu_key(&mut self, event: &gpui::KeyDownEvent, cx: &mut Context<Self>) {
        if !self.sidebar_view_menu.is_open() {
            return;
        }
        match popover::classify_key(
            event.keystroke.key.as_str(),
            event.keystroke.modifiers.platform,
            event.keystroke.modifiers.control,
        ) {
            popover::MenuKey::Escape => self.close_sidebar_view_menu(cx),
            popover::MenuKey::Up | popover::MenuKey::Down => {
                let up = event.keystroke.key.eq_ignore_ascii_case("arrowup");
                if let Some(menu) = self.sidebar_view_menu.open_mut() {
                    menu.active = popover::menu_step(
                        menu.active,
                        SIDEBAR_VIEW_ROWS.len(),
                        if up { -1 } else { 1 },
                    );
                    cx.notify();
                }
            }
            popover::MenuKey::Enter | popover::MenuKey::ModEnter => {
                let active = self.sidebar_view_menu.get().and_then(|m| m.active);
                if let Some(row) = active.and_then(|ix| SIDEBAR_VIEW_ROWS.get(ix)).copied() {
                    self.activate_sidebar_view_row(row, cx);
                }
            }
            popover::MenuKey::Backspace | popover::MenuKey::Other => {}
        }
    }

    fn render_sidebar_view_menu(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let theme = &theme.for_popup();
        let Some(menu_state) = self.sidebar_view_menu.get() else {
            return div().into_any_element();
        };
        let active = menu_state.active;
        let focus = menu_state.focus.clone();
        let organization = self.settings.sidebar_organization;
        let sort = self.settings.sidebar_sort;
        let show_harness = self.settings.sidebar_show_harness;
        let show_branch = self.settings.sidebar_show_branch;
        let show_pr = self.settings.sidebar_show_pull_request;

        let labels = [
            "By device",
            "By project",
            "In one list",
            "Last updated",
            "Created",
            "Branch",
            "Pull request",
            "Harness",
            "Project icon",
            "Location",
            "Compact mode",
        ];
        let icons = [
            icons::LAPTOP,
            icons::FOLDER,
            icons::LIST,
            icons::CLOCK_CIRCLE,
            icons::CALENDAR,
            icons::GIT_BRANCH,
            icons::PULL_REQUEST,
            icons::BOT,
            icons::PROJECT_DEFAULT,
            icons::FOLDER,
            icons::LIST,
        ];
        let selected = [
            organization == SidebarOrganization::ByDevice,
            organization == SidebarOrganization::ByProject,
            organization == SidebarOrganization::InOneList,
            sort == SidebarSort::LastUpdated,
            sort == SidebarSort::Created,
            show_branch,
            show_pr,
            show_harness,
            self.settings.sidebar_show_project_icon,
            self.settings.sidebar_show_project_label,
            self.settings.sidebar_compact,
        ];
        let mut rows: Vec<AnyElement> = SIDEBAR_VIEW_ROWS
            .iter()
            .copied()
            .enumerate()
            .map(|(ix, row)| {
                popover::menu_row_nav(
                    theme,
                    selected[ix],
                    active == Some(ix),
                    format!("sidebar-view-row-{ix}"),
                )
                .id(("sidebar-view-row", ix))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(menu) = this.sidebar_view_menu.open_mut() {
                        menu.active = None;
                    }
                    this.activate_sidebar_view_row(row, cx)
                }))
                .child(
                    icon(icons[ix])
                        .size(crate::typography::ui_rems(15.0))
                        .flex_none()
                        .text_color(theme.text_muted),
                )
                .child(div().flex_1().child(SharedString::from(labels[ix])))
                .child(div().w(crate::typography::ui_rems(14.0)).flex_none().when(selected[ix], |el| {
                    el.child(
                        icon(icons::CHECK)
                            .size(crate::typography::ui_rems(14.0))
                            .text_color(theme.text_muted),
                    )
                }))
                .into_any_element()
            })
            .collect();
        // Compact mode is a layout choice, not a row-content toggle, so it
        // gets its own section under Show.
        let layout_rows = rows.split_off(10);
        let show_rows = rows.split_off(5);
        let sort_rows = rows.split_off(3);
        let organization_rows = rows;

        popover::popover_card(theme)
            .w(px((self.settings.sidebar_width - 2.0 * Theme::SPACE_SM) * self.ui_scale()))
            .track_focus(&focus)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                this.sidebar_view_menu_key(event, cx)
            }))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_sidebar_view_menu(cx)))
            .flex()
            .flex_col()
            .child(popover::menu_heading(theme, "Organize"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(crate::typography::ui_rems(2.0))
                    .children(organization_rows),
            )
            .child(popover::menu_separator())
            .child(popover::menu_heading(theme, "Sort"))
            .child(div().flex().flex_col().gap(crate::typography::ui_rems(2.0)).children(sort_rows))
            .child(popover::menu_separator())
            .child(popover::menu_heading(theme, "Show"))
            .child(div().flex().flex_col().gap(crate::typography::ui_rems(2.0)).children(show_rows))
            .child(popover::menu_separator())
            .child(popover::menu_heading(theme, "Layout"))
            .child(div().flex().flex_col().gap(crate::typography::ui_rems(2.0)).children(layout_rows))
            .into_any_element()
    }

    /// The sidebar's space-filter row: current filter ("All projects" or the
    /// space's name) + chevron, the dropdown floating beneath while open.
    /// Sits OUTSIDE the sidebar's scroll region so the float never clips.
    pub(in crate::shell) fn render_spaces_filter(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let filter = self.settings.space_filter.clone();
        // Name + the dropdown rows' "@ device" tag on the trigger itself, so
        // the filtered space's host reads without opening the picker.
        let (label, device_tag): (SharedString, Option<(SharedString, bool)>) = {
            let state = self.state.read(cx);
            match filter.as_deref().and_then(|id| state.space_row(id)) {
                Some(space) => {
                    let (tag, offline) = state.space_device_tag(space, Utc::now());
                    (
                        space.display_name().to_string().into(),
                        Some((tag.into(), offline)),
                    )
                }
                None => (SharedString::from("All projects"), None),
            }
        };
        let open = self.spaces_menu.is_open();

        let trigger = div()
            .id("spaces-filter")
            .flex_1()
            .min_w_0()
            .h(crate::typography::ui_rems(29.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(crate::typography::ui_rems(Theme::SPACE_SM))
            .rounded(crate::typography::ui_rems(8.0))
            .px(crate::typography::ui_rems(Theme::SPACE_SM))
            .text_size(crate::typography::ui_rems(13.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(motion::hover_blend(
                "spaces-filter",
                theme.text.opacity(0.8),
                theme.text,
            ))
            .bg(if open {
                theme.glass_hover()
            } else {
                motion::hover_blend(
                    "spaces-filter",
                    theme.glass_hover().opacity(0.0),
                    theme.glass_hover(),
                )
            })
            .on_hover(motion::hover_listener("spaces-filter"))
            .cursor_pointer()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, _| this.spaces_menu.note_trigger_press()),
            )
            .on_click(cx.listener(|this, _, window, cx| {
                // A press that found the menu open closes it (the card's
                // mouse-down-out already began the close) — never reopen.
                if this.spaces_menu.take_press_was_open() {
                    this.close_spaces_menu(cx);
                } else {
                    this.open_spaces_menu(window, cx);
                }
            }))
            .child(
                icon(icons::FOLDER)
                    .size(crate::typography::ui_rems(16.0))
                    .flex_none()
                    .text_color(theme.text_muted),
            )
            // flex_1 pushes the caret to the trigger's right edge and gives
            // long space names a bound to fade against; the "@ device"
            // tag hugs the name inside it rather than sitting by the caret.
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(crate::typography::ui_rems(6.0))
                    .child(super::sidebar_faded_label(
                        "spaces-filter-label".into(),
                        false,
                        label,
                    ))
                    .when_some(device_tag, |el, (tag, offline)| {
                        el.child(super::sidebar_faded_label(
                            "spaces-filter-device".into(),
                            false,
                            div()
                                .text_size(crate::typography::ui_rems(10.0))
                                .font_weight(gpui::FontWeight::NORMAL)
                                .text_color(theme.text_muted.opacity(0.45))
                                .child(tag),
                        ))
                        // Disconnected glyph, not the word (user request).
                        .when(offline, |el| {
                            el.child(
                                icon(icons::WIFI_OFF)
                                    .size(crate::typography::ui_rems(12.0))
                                    .flex_none()
                                    .text_color(theme.warning.opacity(0.8)),
                            )
                        })
                    }),
            )
            .child(
                icon(icons::ALT_ARROW_DOWN)
                    .size(crate::typography::ui_rems(14.0))
                    .flex_none()
                    .text_color(theme.text_muted.opacity(0.6)),
            );
        let trigger = if self.spaces_menu.get().is_some() {
            let closing = self.spaces_menu.closing_since();
            let menu = self.render_spaces_menu(theme, cx);
            trigger.relative().child(popover::anchored_menu_below(
                "spaces-filter-menu",
                menu,
                closing,
            ))
        } else {
            trigger
        };

        let view_open = self.sidebar_view_menu.is_open();
        let view_focus = self.sidebar_view_trigger_focus.clone();
        let view_trigger = div()
            .id("sidebar-view-options")
            .role(gpui::Role::Button)
            .aria_label("Sidebar view options")
            .aria_expanded(view_open)
            .track_focus(&view_focus)
            .size(crate::typography::ui_rems(29.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(crate::typography::ui_rems(8.0))
            .border_1()
            .border_color(theme.border.opacity(0.0))
            .focus_visible(|el| el.border_color(theme.border_strong))
            .cursor_pointer()
            .text_color(theme.text_muted)
            .bg(if view_open {
                theme.glass_hover()
            } else {
                theme.glass_hover().opacity(0.0)
            })
            .hover(|el| el.bg(theme.glass_hover()))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, _| this.sidebar_view_menu.note_trigger_press()),
            )
            .on_click(cx.listener(|this, _, window, cx| {
                if this.sidebar_view_menu.take_press_was_open() {
                    this.close_sidebar_view_menu(cx);
                } else {
                    this.open_sidebar_view_menu(window, cx);
                }
            }))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if matches!(
                    event.keystroke.key.to_ascii_lowercase().as_str(),
                    "enter" | "space" | "arrowdown"
                ) {
                    cx.stop_propagation();
                    if this.sidebar_view_menu.is_open()
                        && !event.keystroke.key.eq_ignore_ascii_case("arrowdown")
                    {
                        this.close_sidebar_view_menu(cx);
                    } else if !this.sidebar_view_menu.is_open() {
                        this.open_sidebar_view_menu(window, cx);
                    }
                }
            }))
            .tooltip(|_, cx| cx.new(|_| SidebarViewOptionsTooltip).into())
            .tooltip_show_delay(std::time::Duration::from_millis(350))
            .child(
                icon(icons::SORT)
                    .size(crate::typography::ui_rems(16.0))
                    .text_color(theme.text_muted.opacity(0.6)),
            );
        let view_trigger = if self.sidebar_view_menu.get().is_some() {
            let closing = self.sidebar_view_menu.closing_since();
            let menu = self.render_sidebar_view_menu(theme, cx);
            view_trigger.relative().child(popover::anchored_menu_right(
                "sidebar-view-options-menu",
                menu,
                closing,
            ))
        } else {
            view_trigger
        };

        div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(crate::typography::ui_rems(4.0))
            .px(crate::typography::ui_rems(Theme::SPACE_SM))
            .pt(crate::typography::ui_rems(8.0))
            .pb(crate::typography::ui_rems(4.0))
            .child(trigger)
            .child(view_trigger)
            .into_any_element()
    }

    /// The dropdown card: search on top, "All projects" + space rows (check on
    /// the active filter; right-click for rename/remove) + "New project…".
    fn render_spaces_menu(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let theme = &theme.for_popup();
        let (search, active, focus, list_scroll) = {
            let Some(menu) = self.spaces_menu.get() else {
                return div().into_any_element();
            };
            (
                menu.search.clone(),
                menu.active,
                menu.focus.clone(),
                menu.list_scroll.clone(),
            )
        };
        let rows = self.spaces_menu_rows(cx);
        let scrollbar = popover::rail(self, "spaces-menu-scrollbar", theme, cx);
        let filter = self.settings.space_filter.clone();
        // Keep the host tag so projects with the same name on different
        // devices remain distinguishable. Consume `rows` to avoid cloning
        // the list children per frame.
        let details: Vec<(
            SpacesMenuRow,
            SharedString,
            Option<SharedString>,
            bool,
            bool,
        )> = {
            let state = self.state.read(cx);
            rows.into_iter()
                .map(|row| match row {
                    SpacesMenuRow::All => (
                        SpacesMenuRow::All,
                        SharedString::from("All projects"),
                        None,
                        false,
                        filter.is_none(),
                    ),
                    SpacesMenuRow::Space(id) => {
                        let selected = filter.as_deref() == Some(id.as_str());
                        match state.space_row(&id) {
                            Some(space) => {
                                let (tag, offline) = state.space_device_tag(space, Utc::now());
                                (
                                    SpacesMenuRow::Space(id),
                                    space.display_name().to_string().into(),
                                    Some(tag.into()),
                                    offline,
                                    selected,
                                )
                            }
                            None => (
                                SpacesMenuRow::Space(id),
                                SharedString::from("?"),
                                None,
                                false,
                                selected,
                            ),
                        }
                    }
                    // spaces_menu_rows never yields this variant — the
                    // footer is rendered by the card, not the list.
                    SpacesMenuRow::AddSpace => unreachable!(),
                })
                .collect()
        };
        // The pinned footer's keyboard-nav index: one past the last
        // scrollable row, its permanent place at the end of the nav order.
        let add_index = details.len();

        let list = popover::menu_scroll_host("spaces-menu-list-host")
            .on_hover(cx.listener(Self::on_spaces_menu_list_hover))
            .child(
                popover::menu_scroll_list("spaces-menu-list", &list_scroll)
                    .flex()
                    .flex_col()
                    .gap(crate::typography::ui_rems(2.0))
                    // Same scroll budget as the composer project menu.
                    .max_h(crate::typography::ui_rems(224.0))
                    .children(details.into_iter().enumerate().map(
                        |(ix, (row, label, tag, offline, selected))| {
                            let menu_space = match &row {
                                SpacesMenuRow::Space(id) => Some(id.clone()),
                                _ => None,
                            };
                            let activate = row;
                            popover::menu_row_nav(
                                theme,
                                selected,
                                ix == active,
                                format!("spaces-menu-row-{ix}"),
                            )
                            .id(("spaces-menu-row", ix))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.activate_spaces_menu_row(activate.clone(), cx);
                            }))
                            .when_some(menu_space, |el, space_id| {
                                el.on_mouse_down(
                                    MouseButton::Right,
                                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                        this.space_menu.open((space_id.clone(), event.position));
                                        cx.notify();
                                    }),
                                )
                            })
                            .child(div().flex_1().min_w_0().truncate().child(label))
                            .when_some(tag, |el, tag| {
                                el.child(
                                    div()
                                        .max_w(gpui::relative(0.5))
                                        .min_w_0()
                                        .truncate()
                                        .text_size(crate::typography::ui_rems(10.0))
                                        .font_weight(gpui::FontWeight::NORMAL)
                                        .text_color(theme.text_muted)
                                        .child(tag),
                                )
                            })
                            // Disconnected glyph, not the word (user request).
                            .when(offline, |el| {
                                el.child(
                                    icon(icons::WIFI_OFF)
                                        .size(crate::typography::ui_rems(12.0))
                                        .flex_none()
                                        .text_color(theme.warning.opacity(0.8)),
                                )
                            })
                            // No check glyph — the selected row's wash (menu_row's
                            // active styling) is the selection signal.
                        },
                    )),
            )
            .children(scrollbar);

        popover::popover_card(theme)
            // Match the trigger row as the sidebar is resized. Both live
            // inside the same SPACE_SM horizontal gutters.
            .w(px((self.settings.sidebar_width - 2.0 * Theme::SPACE_SM) * self.ui_scale()))
            .track_focus(&focus)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                this.spaces_menu_key(event, cx)
            }))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.close_spaces_menu(cx);
            }))
            .flex()
            .flex_col()
            // Same 2px rhythm as the composer project menu's root.
            .gap(crate::typography::ui_rems(2.0))
            .child(popover::search_input_frame(
                theme,
                search.into_any_element(),
            ))
            .child(list)
            // "New project…" is a pinned action row under the list (the
            // chat composer's project menu treatment) — scrolling must
            // never carry it away, and its nav index (`add_index`) keeps it
            // LAST.
            .child(
                // Full-bleed through the card's 4px inset — a divider
                // stopping short of the edges reads as a mistake (the
                // composer project menu's treatment).
                div()
                    .my(crate::typography::ui_rems(2.0))
                    .mx(crate::typography::ui_rems(-popover::CARD_INSET))
                    .h(px(1.0))
                    .flex_none()
                    .bg(theme.border.opacity(0.6)),
            )
            .child(
                popover::menu_row_nav(
                    theme,
                    false,
                    active == add_index,
                    "spaces-menu-add".to_string(),
                )
                .id("spaces-menu-add")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.activate_spaces_menu_row(SpacesMenuRow::AddSpace, cx);
                }))
                .child(
                    icon(icons::PLUS)
                        .size(crate::typography::ui_rems(12.0))
                        .flex_none()
                        .text_color(theme.text_muted),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(SharedString::from("New project…")),
                ),
            )
            .into_any_element()
    }
}
