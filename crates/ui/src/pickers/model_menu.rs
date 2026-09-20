use std::time::Duration;

use gpui::{AnyElement, App, Context, SharedString, div, prelude::*, px};

use crate::popover::{self, Loadable};
use crate::theme::Theme;
use super::config::PickerKind;
use super::model_catalog::*;
use super::Pickers;

/// A triangle from the last point in the active trigger to the near edge
/// of its submenu. Mirroring the edge handles menus placed on either side.
fn submenu_corridor(
    origin: gpui::Point<gpui::Pixels>,
    pointer: gpui::Point<gpui::Pixels>,
    submenu: gpui::Bounds<gpui::Pixels>,
    on_left: bool,
) -> bool {
    let edge = if on_left {
        submenu.right()
    } else {
        submenu.left()
    };
    let direction = if on_left { -1.0 } else { 1.0 };
    let distance = f32::from(edge - origin.x) * direction;
    let advance = f32::from(pointer.x - origin.x) * direction;
    if distance <= 0.0 || advance <= 0.0 || advance > distance + 8.0 {
        return false;
    }
    let fraction = (advance / distance).min(1.0);
    let top = origin.y + (submenu.top() - px(8.0) - origin.y) * fraction;
    let bottom = origin.y + (submenu.bottom() + px(8.0) - origin.y) * fraction;
    pointer.y >= top && pointer.y <= bottom
}

impl Pickers {
    /// floating scrollbar; `UniformList` tracks it internally).
    pub(crate) fn model_scroll_base(&self) -> gpui::ScrollHandle {
        self.model_scroll.0.borrow().base_handle.clone()
    }

    /// The combined harness + model switcher (zeron harness-model-picker.tsx):
    /// a vertical harness rail of square brand-icon tabs on the left, the
    /// viewed harness's models on the right. On an existing chat the other
    /// tabs stay visible but disabled — the lock reads as a rule.
    /// The harness/model picker (t3code ModelPickerContent): an icons-only
    /// harness rail on the left (favorites star on top), a search box over
    /// the model list on the right. Rows are two lines — model name over the
    /// harness icon + name (t3 `showProvider`, replacing the description) —
    /// with a ⌘N jump chip and a star toggle trailing. Searching hides the
    /// rail and spans every harness.
    pub(crate) fn render_harness_model_popover(&mut self, cx: &mut Context<Self>) -> AnyElement {
        // Compact tabbed layout (user request, modeled on the referenced
        // picker): the model LIST gets a fixed band of roughly seven compact
        // rows; the pinned traits tray below sizes to its sections.
        let list_height = if self.state.read(cx).selected_chat.is_none() {
            // Keep the settings tray visible while the model list scrolls
            // within the room below the new-chat composer.
            let tray_height = if self.setting_groups(cx).is_empty() {
                0.0
            } else {
                self.setting_groups(cx).len() as f32 * 32.0 + 7.0
            };
            (self.model_space_below.unwrap_or(640.0) - 82.0 - tray_height).clamp(30.0, 216.0)
        } else {
            216.0
        };

        let theme = Theme::of(cx).for_popup();

        // Catalog-level loading/error take over the whole card — the tabs ARE
        // the catalog, so there is nothing stable to draw above the skeleton.
        match &self.harnesses {
            Loadable::Loading | Loadable::Idle => {
                return div()
                    .h(px(list_height))
                    .p(px(8.0))
                    .child(popover::skeleton_menu_rows(
                        "harness-skeleton",
                        &theme,
                        5,
                        cx.entity_id(),
                        cx,
                    ))
                    .into_any_element();
            }
            Loadable::Error(message) => {
                let message = message.clone();
                return div()
                    .h(px(list_height))
                    .p(px(8.0))
                    .child(self.retry_row(
                        "harness-retry",
                        &message,
                        PickerKind::HarnessModel,
                        &theme,
                        cx,
                    ))
                    .into_any_element();
            }
            Loadable::Ready(_) => {}
        }

        let locked = self.harness_locked(cx);
        let effective = self.effective_harness(cx);
        let model_scroll = self.model_scroll.clone();
        let query = self.search.read(cx).text().trim().to_string();
        let searching = !query.is_empty();
        let favorites_view = self.model_rail == ModelRail::Favorites;
        let descriptors = self.rail_descriptors(cx);
        // No-agents empty state: the catalog loaded but offers nothing
        // runnable (every enabled harness is missing its CLI, or nothing is
        // enabled) and there's no committed chat harness to force-include —
        // guidance instead of an empty tab row.
        if descriptors.is_empty() {
            return div()
                .p(px(16.0))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(8.0))
                .child(
                    crate::icons::icon(crate::icons::TERMINAL)
                        .size(px(20.0))
                        .text_color(theme.text_muted),
                )
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(13.0))
                        .text_color(theme.text)
                        .child(SharedString::from("No agents available")),
                )
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(12.0))
                        .text_color(theme.text_muted)
                        .text_center()
                        .child(SharedString::from(
                            "Enable an installed agent in Settings → Agents, \
                             or install an agent CLI.",
                        )),
                )
                .into_any_element();
        }
        let rows = self.model_rows(cx);

        // ── tabs: the favorites star, then one brand icon per harness —
        //    ACROSS THE TOP (user request; was a left rail). The
        //    viewed tab wears a 2px accent bar sitting on the row's bottom
        //    hairline. Tabs never hide: a live search only filters the
        //    viewed tab's list, so switching tabs re-scopes the same query.
        let mut tabs = div()
            .flex_none()
            .h(px(40.0))
            .px(px(popover::CARD_INSET))
            .border_b_1()
            .border_color(crate::theme::hairline(0.08))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.0));
        tabs = tabs.child(
            div()
                .id("model-tab-favorites")
                .relative()
                .w(px(32.0))
                .h(px(32.0))
                .rounded(px(8.0))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .when(!favorites_view, |el| {
                    el.hover(|s| s.bg(crate::theme::ink(0.06)))
                })
                .on_click(cx.listener(|this, _, _, cx| {
                    this.setting_menu = None;
                    this.model_rail = ModelRail::Favorites;
                    // Anchor on the selected row when it's starred, else
                    // the top — never a stray second highlight.
                    this.active = this.selected_model_index(cx);
                    this.model_scroll_base().set_offset(gpui::Point::default());
                    this.model_scroll
                        .scroll_to_item(this.active, gpui::ScrollStrategy::Nearest);
                    cx.notify();
                }))
                .child(
                    crate::icons::icon(crate::icons::STAR_BOLD)
                        .size(px(15.0))
                        .text_color(if favorites_view {
                            theme.text
                        } else {
                            theme.text_muted
                        }),
                )
                .when(favorites_view, |el| el.child(tab_indicator(theme.accent))),
        );
        for (ix, descriptor) in descriptors.iter().enumerate() {
            let harness = descriptor.id;
            let is_viewed = !favorites_view && effective == Some(harness);
            let is_disabled = locked && effective != Some(harness);
            let (icon_path, tint) = harness_brand_icon(harness);
            tabs =
                tabs.child(
                    div()
                        .id(("harness-tab", ix))
                        .relative()
                        .w(px(32.0))
                        .h(px(32.0))
                        .rounded(px(8.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(is_disabled, |el| el.opacity(0.35))
                        .when(!is_disabled, |el| el.cursor_pointer())
                        .when(!is_disabled && !is_viewed, |el| {
                            el.hover(|s| s.bg(crate::theme::ink(0.06)))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.setting_menu = None;
                            this.model_rail = ModelRail::Harness;
                            this.pick_harness(harness, cx);
                            cx.notify();
                        }))
                        .child(crate::icons::icon(icon_path).size(px(16.0)).text_color(
                            tint.unwrap_or(if is_viewed {
                                theme.text
                            } else {
                                theme.text_muted
                            }),
                        ))
                        .when(is_viewed, |el| el.child(tab_indicator(theme.accent))),
                );
        }

        // ── search row: icon + borderless input over a full-bleed hairline.
        //    The placeholder names the scope — the query never leaves the
        //    viewed tab (user request; the old global search hid the rail).
        let search_row = div()
            .flex_none()
            .h(px(40.0))
            .px(px(10.0))
            .border_b_1()
            .border_color(crate::theme::hairline(0.08))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(
                crate::icons::icon(crate::icons::MAGNIFER)
                    .size(px(14.0))
                    .flex_none()
                    .text_color(theme.text_muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(crate::typography::ui_rems(13.0))
                    .child(self.search.clone()),
            );

        // ── model rows: a VIRTUALIZED uniform list — only the visible slice
        //    renders, so a 7k-model catalog scrolls as smoothly as seven
        //    (field report: the un-virtualized stack was the picker's lag).
        //    Keyboard nav scrolls via the UniformListScrollHandle.
        let effective_models = effective.and_then(|h| self.models.get(&h));
        let model_list: Option<AnyElement> = if !rows.is_empty() {
            let entity = cx.entity();
            let row_data = rows.clone();
            Some(
                gpui::uniform_list(
                    "model-menu-scroll",
                    rows.len(),
                    move |range, _window, app| {
                        entity.update(app, |this, cx| {
                            range
                                .filter_map(|ix| {
                                    row_data
                                        .get(ix)
                                        .map(|row| this.render_model_row(ix, row, cx))
                                })
                                .collect::<Vec<AnyElement>>()
                        })
                    },
                )
                .size_full()
                .px(px(popover::CARD_INSET))
                .track_scroll(&model_scroll)
                .into_any_element(),
            )
        } else {
            None
        };
        let list_children: Vec<AnyElement> = if !rows.is_empty() {
            Vec::new()
        } else if searching {
            vec![empty_list_note(&theme, "No models found")]
        } else if favorites_view {
            vec![empty_list_note(
                &theme,
                "No starred models yet — hit a row's star",
            )]
        } else {
            match effective_models {
                Some(Loadable::Error(message)) => {
                    let message = message.clone();
                    vec![self.retry_row(
                        "model-retry",
                        &message,
                        PickerKind::HarnessModel,
                        &theme,
                        cx,
                    )]
                }
                _ => vec![popover::skeleton_menu_rows(
                    "model-skeleton",
                    &theme,
                    5,
                    cx.entity_id(),
                    cx,
                )],
            }
        };

        let model_scrollbar = popover::rail(self, "model-scrollbar", &theme, cx);
        let list_host = div()
            .id("model-list-scroll-host")
            .relative()
            .flex_none()
            .h(px(list_height))
            .py(px(popover::CARD_INSET))
            // A whisper of wash keeps the scrolling band readable between
            // the pinned chrome above and the traits tray below.
            .bg(crate::theme::ink(0.02))
            .on_hover(cx.listener(Self::on_menu_list_hover))
            .child(match model_list {
                Some(list) => list,
                // Empty/loading/error notes: a plain static stack.
                None => div()
                    .id("model-menu-scroll")
                    .size_full()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .px(px(popover::CARD_INSET))
                    .children(list_children)
                    .into_any_element(),
            })
            // Absolute child: the hit rail and thumb float above the
            // scroll content without consuming any list width.
            .children(model_scrollbar);

        // ── traits tray: the reasoning ladder + model options PINNED under
        //    the list (the separate Traits popover folded in here — user
        //    request). Hidden entirely when the selected model has neither.
        let has_tray = !self.trait_ladder(cx).is_empty()
            || self
                .selected_model(cx)
                .is_some_and(|m| !m.options.is_empty());
        let tray: Option<AnyElement> = has_tray.then(|| {
            let sections = self.render_traits_sections(cx);
            div()
                .id("model-traits-tray")
                .flex_none()
                .border_t_1()
                .border_color(crate::theme::hairline(0.08))
                // Long option stacks scroll inside the tray rather than
                // growing the card past the viewport.
                .max_h(px(236.0))
                .overflow_y_scroll()
                .px(px(popover::CARD_INSET))
                .child(sections)
                .into_any_element()
        });

        div()
            .flex()
            .flex_col()
            .child(tabs)
            .child(search_row)
            .child(list_host)
            .children(tray)
            .into_any_element()
    }

    /// One model row for the virtualized list. `ix` is the row's GLOBAL index
    /// (⌘N chips, hover-cursor, and activation all key on it). The 2px
    /// inter-row gap is baked into each item's bottom padding so every item
    /// is the same height (uniform_list measures the first).
    pub(crate) fn render_model_row(
        &mut self,
        ix: usize,
        row: &ModelRowData,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::of(cx).for_popup();
        let effective = self.effective_harness(cx);
        let is_selected = Some(row.harness) == effective
            && self.selected_model(cx).map(|m| m.id.as_str()) == Some(row.model.id.as_str());
        let is_active = ix == self.active;
        let is_fav = self.defaults.is_favorite(row.harness, &row.model.id);
        let (icon_path, tint) = harness_brand_icon(row.harness);
        let label: SharedString = row.model.label.clone().into();
        let harness_name = row.harness_name.clone();
        let harness = row.harness;
        let star_model = row.model.id.clone();
        // Provider attribution (field report: several connected opencode
        // providers advertise identically-named models — "GLM-5.2" exists
        // under 64 providers — and rows were indistinguishable). The driver
        // ships the provider display name in `description`; other harnesses'
        // taglines read fine in the same slot. Skip when it just repeats the
        // harness name.
        let attribution: Option<SharedString> = row
            .model
            .description
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty() && !d.eq_ignore_ascii_case(harness_name.as_ref()))
            .map(|d| SharedString::from(d.to_owned()));
        let compact = self.model_rail == ModelRail::Harness;
        let mut el = div()
            .id(("model-row", ix))
            .px(px(8.0))
            .py(px(if compact { 5.0 } else { 6.0 }))
            .rounded(px(popover::MENU_ITEM_RADIUS))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.0))
            .cursor_pointer();
        // ONE moving highlight (t3/Base-UI combobox): hovering moves the
        // keyboard cursor instead of painting its own wash, so hover + arrow
        // cursor can never wear two washes at once. Selection is the
        // distinct stronger treatment (wash + ring).
        if is_selected {
            el = el
                .bg(crate::theme::card_selected_bg())
                .shadow(crate::theme::card_selected_shadows());
        } else if is_active {
            el = el.bg(crate::theme::ink(0.05));
        }
        el = el.on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
            if *hovered && this.active != ix {
                this.active = ix;
                cx.notify();
            }
        }));
        // Compact single-line rows on a harness tab (user request): every
        // row there shares the tab's harness, so the identity subline is
        // dead weight — attribution rides inline instead (opencode ships
        // identically-named models under 64 providers; it must stay
        // visible). The favorites tab mixes harnesses and keeps the
        // two-line layout with the brand subline.
        let body: AnyElement = if compact {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .flex_none()
                        .max_w_full()
                        .truncate()
                        .text_size(crate::typography::ui_rems(12.5))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(theme.text)
                        .child(label),
                )
                .when_some(attribution, |el, attribution| {
                    el.child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(crate::typography::ui_rems(11.0))
                            .text_color(theme.text_muted)
                            .child(attribution),
                    )
                })
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .w_full()
                        .truncate()
                        .text_size(crate::typography::ui_rems(12.5))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(theme.text)
                        .child(label),
                )
                .child(
                    // Harness identity subline (t3 `showProvider`), plus
                    // the model's own attribution when it carries one.
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            crate::icons::icon(icon_path)
                                .size(px(11.0))
                                .flex_none()
                                .text_color(tint.unwrap_or(theme.text_muted)),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(crate::typography::ui_rems(11.0))
                                .text_color(theme.text_muted)
                                .child(harness_name),
                        )
                        .when_some(attribution, |el, attribution| {
                            el.child(
                                div()
                                    .flex_none()
                                    .text_size(crate::typography::ui_rems(11.0))
                                    .text_color(theme.text_muted)
                                    .child(SharedString::from("·")),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(crate::typography::ui_rems(11.0))
                                    .text_color(theme.text_muted)
                                    .child(attribution),
                            )
                        }),
                )
                .into_any_element()
        };
        el = el
            .on_click(cx.listener(move |this, _, _, cx| {
                this.activate_model_index(ix, cx);
            }))
            .child(body);
        if ix < 9 {
            el = el.child(popover::kbd_hint(&theme, &format!("⌘{}", ix + 1)));
        }
        el = el.child(
            div()
                .id(("model-star", ix))
                .flex_none()
                .w(px(22.0))
                .h(px(22.0))
                .rounded(px(popover::MENU_ITEM_RADIUS))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(crate::theme::ink(0.08)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.toggle_model_favorite(harness, &star_model, cx);
                }))
                .child(
                    crate::icons::icon(if is_fav {
                        crate::icons::STAR_BOLD
                    } else {
                        crate::icons::STAR
                    })
                    .size(px(13.0))
                    .text_color(if is_fav {
                        theme.warning
                    } else {
                        theme.text_muted
                    }),
                ),
        );
        div().pb(px(2.0)).child(el).into_any_element()
    }

    pub(crate) fn setting_groups(&self, cx: &App) -> Vec<SettingGroup> {
        let mut groups = Vec::new();
        let levels = self.trait_ladder(cx);
        if !levels.is_empty() {
            let selected = self.effective_reasoning(cx);
            let default = default_reasoning(&levels);
            groups.push(SettingGroup {
                id: ModelSetting::Reasoning,
                label: "Reasoning".into(),
                choices: levels
                    .into_iter()
                    .map(|level| SettingChoice {
                        label: reasoning_label(level).into(),
                        value: String::new(),
                        reasoning: Some(level),
                        selected: selected == Some(level),
                        default: default == Some(level),
                    })
                    .collect(),
            });
        }
        if let Some(model) = self.selected_model(cx) {
            let selections = self.explicit_options(cx);
            for option in &model.options {
                if option.choices.is_empty() {
                    continue;
                }
                let selected = selections
                    .get(&option.id)
                    .and_then(|v| v.as_str())
                    .unwrap_or(&option.default_choice);
                groups.push(SettingGroup {
                    id: ModelSetting::Option(option.id.clone()),
                    label: option.label.clone(),
                    choices: option
                        .choices
                        .iter()
                        .map(|choice| SettingChoice {
                            label: choice.label.clone(),
                            value: choice.id.clone(),
                            reasoning: None,
                            selected: selected == choice.id,
                            default: option.default_choice == choice.id,
                        })
                        .collect(),
                });
            }
        }
        groups
    }

    pub(crate) fn cancel_setting_hover(&mut self) {
        self.setting_hover_task = None;
        self.setting_hover_pending = None;
        self.setting_hover_pointer = None;
    }

    pub(crate) fn hover_setting(
        &mut self,
        id: ModelSetting,
        index: usize,
        pointer: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.cancel_setting_hover();
        if self.setting_menu.as_ref() == Some(&id) {
            self.setting_intent_origin = Some(pointer);
            return;
        }
        let toward_child = self.setting_menu.is_some()
            && self
                .setting_intent_origin
                .zip(self.setting_bounds)
                .is_some_and(|(origin, bounds)| {
                    submenu_corridor(origin, pointer, bounds, self.setting_on_left)
                });
        if toward_child {
            // A brief pause distinguishes crossing a sibling en route to the
            // submenu from deliberately resting on that sibling. Leaving it
            // or clicking cancels this task, so dismissed menus cannot reopen.
            let source = self.setting_menu.clone();
            self.setting_hover_pending = Some(id.clone());
            self.setting_hover_pointer = Some(pointer);
            self.setting_hover_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(300))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    if this.is_open()
                        && this.setting_menu == source
                        && this.setting_hover_pending.as_ref() == Some(&id)
                    {
                        this.active = index;
                        this.open_setting(id, cx);
                        this.setting_intent_origin = Some(pointer);
                    }
                });
            }));
        } else {
            self.active = index;
            self.open_setting(id, cx);
            self.setting_intent_origin = Some(pointer);
        }
    }

    pub(crate) fn open_setting(&mut self, id: ModelSetting, cx: &mut Context<Self>) {
        self.cancel_setting_hover();
        self.setting_intent_origin = None;
        self.setting_active = self
            .setting_groups(cx)
            .iter()
            .find(|g| g.id == id)
            .and_then(|g| g.choices.iter().position(|c| c.selected))
            .unwrap_or(0);
        self.setting_menu = Some(id);
        self.setting_bounds = None;
        self.setting_scroll = gpui::ScrollHandle::new();
        self.setting_scroll.scroll_to_item(self.setting_active);
        cx.notify();
    }

    pub(crate) fn activate_setting_choice(&mut self, cx: &mut Context<Self>) {
        let Some(group) = self
            .setting_groups(cx)
            .into_iter()
            .find(|g| Some(&g.id) == self.setting_menu.as_ref())
        else {
            return;
        };
        let Some(choice) = group.choices.get(self.setting_active) else {
            return;
        };
        match group.id {
            ModelSetting::Reasoning => {
                if let Some(level) = choice.reasoning {
                    self.pick_reasoning(level, cx);
                }
            }
            ModelSetting::Option(id) => {
                self.pick_option(id, choice.value.clone(), choice.default, cx)
            }
        }
        self.setting_menu = None;
        self.setting_bounds = None;
        cx.notify();
    }

    /// Each model setting gets a compact trigger and its own nested choices.
    pub(crate) fn render_traits_sections(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).for_popup();
        let base_index = self.model_rows_len(cx);
        let mut rows = Vec::new();
        for (ix, group) in self.setting_groups(cx).into_iter().enumerate() {
            let open = self.setting_menu.as_ref() == Some(&group.id);
            let value = group
                .choices
                .iter()
                .find(|c| c.selected)
                .map(|c| c.label.clone())
                .unwrap_or_default();
            let id = group.id.clone();
            let outside_id = id.clone();
            let move_id = id.clone();
            let exit_id = id.clone();
            let exit_entity = cx.entity().downgrade();
            let entity = cx.entity().downgrade();
            let mut row = popover::menu_row(
                &theme,
                open || self.active == base_index + ix,
                format!("model-setting-{ix}"),
            )
            .id(("model-setting", ix))
            .relative()
            .h(px(30.0))
            .py(px(0.0))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.active = base_index + ix;
                this.cancel_setting_hover();
                this.setting_menu = None;
                this.setting_bounds = None;
                window.focus(&this.focus, cx);
                cx.stop_propagation();
                cx.notify();
            }))
            .on_mouse_down_out(
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    // The trigger dismisses its own child. Elsewhere in the parent,
                    // close this child during capture and let that control receive
                    // the same click. Clicks inside the floating child stay local.
                    if this.setting_menu.as_ref() == Some(&outside_id)
                        && !this
                            .setting_bounds
                            .is_some_and(|bounds| bounds.contains(&event.position))
                    {
                        this.cancel_setting_hover();
                        this.setting_menu = None;
                        this.setting_bounds = None;
                        cx.notify();
                    }
                }),
            )
            .child(
                gpui::canvas(
                    move |bounds, window, cx| {
                        let left = bounds.right() + px(244.0) > window.viewport_size().width;
                        let _ = entity.update(cx, |this, cx| {
                            if this.setting_on_left != left {
                                this.setting_on_left = left;
                                cx.notify();
                            }
                        });
                    },
                    move |trigger, _, window, _| {
                        if !open {
                            return;
                        }
                        window.on_mouse_event(move |event: &gpui::MouseMoveEvent, phase, _, cx| {
                            if phase != gpui::DispatchPhase::Bubble {
                                return;
                            }
                            let _ = exit_entity.update(cx, |this, cx| {
                                if this.setting_menu.as_ref() != Some(&exit_id) {
                                    return;
                                }
                                let pointer = event.position;
                                if trigger.contains(&pointer) {
                                    this.setting_intent_origin = Some(pointer);
                                    return;
                                }
                                let Some(bounds) = this.setting_bounds else {
                                    return;
                                };
                                if bounds.contains(&pointer) {
                                    return;
                                }
                                if this.setting_intent_origin.is_some_and(|origin| {
                                    submenu_corridor(origin, pointer, bounds, this.setting_on_left)
                                }) {
                                    return;
                                }
                                this.cancel_setting_hover();
                                this.setting_menu = None;
                                this.setting_bounds = None;
                                this.setting_intent_origin = None;
                                // Do not leave a keyboard-style selection on the
                                // trigger after pointer navigation dismisses it.
                                this.active = 0;
                                cx.notify();
                            });
                        });
                    },
                )
                .absolute()
                .inset_0(),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(SharedString::from(group.label.clone())),
            )
            .child(
                div()
                    .max_w(px(100.0))
                    .truncate()
                    .text_color(theme.text_muted)
                    .child(SharedString::from(value)),
            )
            .child(
                crate::icons::icon(crate::icons::ALT_ARROW_RIGHT)
                    .size(px(12.0))
                    .text_color(theme.text_muted),
            );
            if open {
                let entity = cx.entity().downgrade();
                let menu = popover::popover_card(&theme)
                    .w(px(232.0))
                    .relative()
                    .child(popover::menu_heading(&theme, &group.label))
                    .child(
                        div()
                            .id("model-setting-choices")
                            .max_h(px(240.0))
                            .overflow_y_scroll()
                            .track_scroll(&self.setting_scroll)
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .children(group.choices.into_iter().enumerate().map(
                                |(choice_ix, choice)| {
                                    popover::menu_row(
                                        &theme,
                                        choice_ix == self.setting_active,
                                        format!("setting-choice-{ix}-{choice_ix}"),
                                    )
                                    .id(("setting-choice", choice_ix))
                                    .h(px(30.0))
                                    .py(px(0.0))
                                    .flex_none()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.setting_active = choice_ix;
                                        this.activate_setting_choice(cx);
                                        cx.stop_propagation();
                                    }))
                                    .child(SharedString::from(choice.label))
                                    .child(div().flex_1())
                                    .when(choice.default, |el| el.child(default_badge(&theme)))
                                    .when(
                                        choice.selected,
                                        |el| {
                                            el.child(
                                                crate::icons::icon(crate::icons::CHECK)
                                                    .size(px(14.0))
                                                    .text_color(theme.text),
                                            )
                                        },
                                    )
                                },
                            )),
                    )
                    .child(
                        gpui::canvas(
                            move |bounds, _, cx| {
                                let _ =
                                    entity.update(cx, |this, _| this.setting_bounds = Some(bounds));
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    );
                row = row.child(popover::nested_menu(
                    format!("setting-menu-{ix}"),
                    menu.into_any_element(),
                    self.setting_on_left,
                ));
            }
            // Keep hover ownership on a stable wrapper: menu_row already
            // owns its hover animation, and changing the open row's styling
            // must not reopen a child just dismissed by clicking its trigger.
            rows.push(
                div()
                    .id(("model-setting-hover", ix))
                    .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                        if *hovered {
                            this.hover_setting(
                                id.clone(),
                                base_index + ix,
                                window.mouse_position(),
                                cx,
                            );
                        } else if this.setting_hover_pending.as_ref() == Some(&id) {
                            this.cancel_setting_hover();
                        }
                    }))
                    .on_mouse_move(
                        cx.listener(move |this, event: &gpui::MouseMoveEvent, _, cx| {
                            if this.setting_menu.as_ref() == Some(&move_id) {
                                this.setting_intent_origin = Some(event.position);
                            } else if this.setting_hover_pending.as_ref() == Some(&move_id) {
                                let in_corridor = this
                                    .setting_intent_origin
                                    .zip(this.setting_bounds)
                                    .is_some_and(|(origin, bounds)| {
                                        submenu_corridor(
                                            origin,
                                            event.position,
                                            bounds,
                                            this.setting_on_left,
                                        )
                                    });
                                let forward = this.setting_hover_pointer.map_or(0.0, |previous| {
                                    f32::from(event.position.x - previous.x)
                                        * if this.setting_on_left { -1.0 } else { 1.0 }
                                });
                                if !in_corridor || forward <= -2.0 {
                                    this.active = base_index + ix;
                                    this.open_setting(move_id.clone(), cx);
                                    this.setting_intent_origin = Some(event.position);
                                } else if forward >= 2.0 {
                                    // Slow but continuing progress renews grace;
                                    // only a pause should activate the sibling.
                                    this.hover_setting(
                                        move_id.clone(),
                                        base_index + ix,
                                        event.position,
                                        cx,
                                    );
                                }
                            }
                        }),
                    )
                    .child(row),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .py(px(popover::CARD_INSET))
            .children(rows)
            .into_any_element()
    }
}

/// The "Default" marker beside a section's default choice: a ghost badge —
/// bare muted text, no border or fill (user request; t3code draws an outline
/// pill here).
fn default_badge(theme: &Theme) -> gpui::Div {
    div()
        .flex_none()
        .text_size(crate::typography::ui_rems(10.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(theme.for_popup().text_muted)
        .child(SharedString::from("Default"))
}

/// Brand mark + optional tint for a harness (the Claude mark keeps its brand
/// orange even on the monochrome surface; the mock harness scripts
/// Claude-flavoured runs, so it wears the Claude mark).
/// The 2px underline marking the viewed top tab: sits on the tab row's
/// bottom hairline (the tab is 32px tall inside a 40px row, so -4px lands
/// exactly on the border), rounded like a capsule.
fn tab_indicator(tint: gpui::Hsla) -> gpui::Div {
    div()
        .absolute()
        .bottom(px(-4.0))
        .left(px(6.0))
        .right(px(6.0))
        .h(px(2.0))
        .rounded(px(1.0))
        .bg(tint)
}

/// Centered muted note filling an empty model list ("No models found").
fn empty_list_note(theme: &Theme, copy: &str) -> AnyElement {
    div()
        .px(px(8.0))
        .py(px(24.0))
        .text_size(crate::typography::ui_rems(12.0))
        .text_color(theme.for_popup().text_muted)
        .text_center()
        .child(SharedString::from(copy.to_string()))
        .into_any_element()
}
