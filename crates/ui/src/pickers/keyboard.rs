use gpui::{App, Context, KeyDownEvent, Window};
use zeron_engine::registry::HarnessDescriptor;
use zeron_proto::{HarnessId, ReasoningLevel};

use crate::popover::{self, MenuKey};
use super::config::{CheckoutKind, PickerKind};
use super::git_picker::MAX_REF_ROWS;
use super::model_catalog::{
    ModelRowData, ModelRowsKey, NO_ACTIVE_ROW, offered_harnesses, scoped_model_rows,
};
use super::Pickers;

impl Pickers {
    // ---- keyboard ----

    /// The traits popover's reasoning ladder (model levels, falling back to
    /// the harness's advertised ladder) — shared by render and keyboard nav.
    pub(crate) fn trait_ladder(&self, cx: &App) -> Vec<ReasoningLevel> {
        let Some(model) = self.selected_model(cx) else {
            return Vec::new();
        };
        if !model.reasoning_levels.is_empty() {
            return model.reasoning_levels.clone();
        }
        self.effective_harness(cx)
            .and_then(|h| {
                self.harnesses
                    .ready()
                    .and_then(|list| list.iter().find(|d| d.id == h))
                    .map(|d| d.reasoning_levels.clone())
            })
            .unwrap_or_default()
    }

    /// The harness descriptors the picker rail offers, with the committed
    /// harness force-included even when it's outside the offered set (a
    /// dev session's mock harness, or one disabled after the chat existed).
    pub(crate) fn rail_descriptors(&self, cx: &App) -> Vec<HarnessDescriptor> {
        let Some(list) = self.harnesses.ready() else {
            return Vec::new();
        };
        let mut descriptors = offered_harnesses(list);
        if let Some(effective) = self.effective_harness(cx)
            && !descriptors.iter().any(|d| d.id == effective)
            && let Some(descriptor) = list.iter().find(|d| d.id == effective)
        {
            descriptors.insert(0, descriptor.clone());
        }
        descriptors
    }

    /// The model rows the picker currently shows, flat and in render order —
    /// keyboard nav, ⌘N jumps, Enter and the render walk THE SAME list.
    ///
    /// A live search spans every ready harness (t3: the sidebar hides and
    /// the query ignores it); otherwise the rail selection decides —
    /// favorites across harnesses, or the effective harness's list with its
    /// starred rows floated to the top (t3 `groupFavorites`). A locked chat
    /// restricts every view to its own harness.
    /// Cached [`Self::visible_model_rows`]: selection/highlight changes and
    /// re-renders share one flattened list until an input actually changes.
    pub(crate) fn model_rows(&self, cx: &App) -> std::sync::Arc<Vec<ModelRowData>> {
        let key = ModelRowsKey {
            query: self.search.read(cx).text().trim().to_string(),
            rail: self.model_rail,
            effective: self.effective_harness(cx),
            locked: self.harness_locked(cx),
            catalog_rev: self.catalog_rev,
        };
        if let Some((cached_key, rows)) = self.model_rows_cache.borrow().as_ref()
            && *cached_key == key
        {
            return rows.clone();
        }
        let rows = std::sync::Arc::new(self.visible_model_rows(cx));
        *self.model_rows_cache.borrow_mut() = Some((key, rows.clone()));
        rows
    }

    pub(crate) fn visible_model_rows(&self, cx: &App) -> Vec<ModelRowData> {
        let effective = self.effective_harness(cx);
        let mut descriptors = self.rail_descriptors(cx);
        if self.harness_locked(cx) {
            descriptors.retain(|d| Some(d.id) == effective);
        }
        // Favorite lookups are per-row; the Vec scan made the flatten
        // O(models × favorites).
        let favorites: std::collections::HashSet<(HarnessId, &str)> = self
            .defaults
            .favorites
            .iter()
            .map(|f| (f.harness, f.model.as_str()))
            .collect();
        let query = self.search.read(cx).text().trim().to_string();
        scoped_model_rows(
            &query,
            self.model_rail,
            effective,
            &descriptors,
            |harness| {
                self.models
                    .get(&harness)
                    .and_then(|l| l.ready())
                    .map(|models| models.as_slice())
            },
            |harness, model| favorites.contains(&(harness, model)),
        )
    }

    /// The row the keyboard-nav highlight starts on: the resolved selected
    /// model's index in the VISIBLE rows (the favorites/search views may not
    /// contain it — then 0), 0 while the list is loading.
    pub(crate) fn selected_model_index(&self, cx: &App) -> usize {
        let selected = self.selected_model(cx).map(|m| m.id.clone());
        let effective = self.effective_harness(cx);
        self.model_rows(cx)
            .iter()
            .position(|row| {
                Some(row.harness) == effective && selected.as_deref() == Some(row.model.id.as_str())
            })
            .unwrap_or(0)
    }

    /// The picker's visible row count (keyboard nav bounds).
    pub(crate) fn model_rows_len(&self, cx: &App) -> usize {
        self.model_rows(cx).len()
    }

    /// Enter on the harness/model popover: pick the highlighted model.
    pub(crate) fn activate_model_row(&mut self, cx: &mut Context<Self>) {
        if self.setting_menu.is_some() {
            self.activate_setting_choice(cx);
        } else if let Some(index) = self.active.checked_sub(self.model_rows_len(cx)) {
            if let Some(group) = self.setting_groups(cx).get(index) {
                self.open_setting(group.id.clone(), cx);
            }
        } else {
            self.activate_model_index(self.active, cx);
        }
    }

    /// Pick the visible row at `ix` — a foreign-harness row (favorites /
    /// search) switches the harness first, exactly like clicking its rail
    /// icon and then the model.
    pub(crate) fn activate_model_index(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(row) = self.model_rows(cx).get(ix).cloned() else {
            return;
        };
        if self.effective_harness(cx) != Some(row.harness) {
            if self.harness_locked(cx) {
                return;
            }
            self.pick_harness(row.harness, cx);
        }
        self.pick_model(row.model.id, cx);
    }

    /// Star/unstar a model and persist it with the sticky defaults.
    pub(crate) fn toggle_model_favorite(&mut self, harness: HarnessId, model: &str, cx: &mut Context<Self>) {
        self.defaults.toggle_favorite(harness, model);
        self.save_defaults();
        self.catalog_rev += 1;
        // Starring REORDERS the list (stars float to the top / leave the
        // favorites view) — re-home the keyboard highlight onto the SELECTED
        // row so exactly one row reads highlighted afterwards. Following the
        // starred row instead left its cursor wash next to the selected
        // row's ring: "two highlighted rows" (user report, twice).
        self.active = self.selected_model_index(cx);
        cx.notify();
    }

    pub(crate) fn on_search_submit(&mut self, cx: &mut Context<Self>) {
        if self.open_kind() == Some(PickerKind::Branch)
            && let Some(row) = self.filtered_ref_rows(cx).into_iter().nth(self.active)
        {
            self.pick_ref(row, cx);
        }
        if self.open_kind() == Some(PickerKind::Space) {
            let rows = self.filtered_space_rows(cx);
            if let Some(space) = rows.get(self.active) {
                self.pick_space(space.id.clone(), cx);
            } else if self.active == rows.len() {
                self.pick_no_project(cx);
            }
        }
        if self.open_kind() == Some(PickerKind::Device)
            && let Some(device) = self.filtered_device_rows(cx).into_iter().nth(self.active)
        {
            self.pick_device(device.id, cx);
        }
        // Palette-search Enter submits the highlighted model or setting.
        if self.open_kind() == Some(PickerKind::HarnessModel) {
            self.activate_model_row(cx);
        }
    }

    pub(crate) fn on_key_down(&mut self, event: &KeyDownEvent, _window: &Window, cx: &mut Context<Self>) {
        self.cancel_setting_hover();
        // The frame stays mounted (and possibly focused) through the exit
        // animation — keys must not drive a dying popover.
        if !self.open.is_open() {
            return;
        }
        if self.setting_menu.is_some() {
            match event.keystroke.key.as_str() {
                "escape" | "left" => {
                    self.setting_menu = None;
                    self.setting_bounds = None;
                }
                "up" | "down" => {
                    let count = self
                        .setting_groups(cx)
                        .into_iter()
                        .find(|g| Some(&g.id) == self.setting_menu.as_ref())
                        .map(|g| g.choices.len())
                        .unwrap_or(0);
                    self.setting_active = popover::menu_step(
                        Some(self.setting_active),
                        count,
                        if event.keystroke.key == "up" { -1 } else { 1 },
                    )
                    .unwrap_or(0);
                    self.setting_scroll.scroll_to_item(self.setting_active);
                }
                "enter" => self.activate_setting_choice(cx),
                _ => return,
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if event.keystroke.key == "right"
            && self.open_kind() == Some(PickerKind::HarnessModel)
            && self.active >= self.model_rows_len(cx)
        {
            self.activate_model_row(cx);
            cx.stop_propagation();
            return;
        }
        // ⌘1…⌘9 jump-picks the Nth visible model row (t3 modelPickerKeys;
        // the chips on the rows advertise these).
        if self.open_kind() == Some(PickerKind::HarnessModel)
            && event.keystroke.modifiers.platform
            && let Ok(n) = event.keystroke.key.parse::<usize>()
            && (1..=9).contains(&n)
        {
            self.activate_model_index(n - 1, cx);
            cx.notify();
            return;
        }
        let key = popover::classify_key(
            event.keystroke.key.as_str(),
            event.keystroke.modifiers.platform,
            event.keystroke.modifiers.control,
        );

        match key {
            MenuKey::Escape => {
                self.animate_close(cx);
                cx.notify();
                cx.stop_propagation();
            }
            MenuKey::Up | MenuKey::Down => {
                let delta = if key == MenuKey::Up { -1 } else { 1 };
                let count = match self.open_kind() {
                    Some(PickerKind::Branch) => self.filtered_ref_rows(cx).len().min(MAX_REF_ROWS),
                    Some(PickerKind::Checkout) => 2,
                    // Continue from model rows into the pinned settings triggers.
                    Some(PickerKind::HarnessModel) => {
                        self.model_rows_len(cx) + self.setting_groups(cx).len()
                    }
                    Some(PickerKind::Space) => self.filtered_space_rows(cx).len() + 1,
                    Some(PickerKind::Device) => self.filtered_device_rows(cx).len(),
                    None => 0,
                };
                let current = (self.active != NO_ACTIVE_ROW).then_some(self.active);
                self.active = popover::menu_step(current, count, delta).unwrap_or(0);
                // Keep the highlighted MODEL row in view (the rows are the
                // scroll container's direct children, so indices map 1:1);
                // the traits chips below live in the pinned tray and never
                // need scrolling into view.
                if self.open_kind() == Some(PickerKind::HarnessModel)
                    && self.active < self.model_rows_len(cx)
                {
                    self.model_scroll
                        .scroll_to_item(self.active, gpui::ScrollStrategy::Nearest);
                }
                cx.notify();
                cx.stop_propagation();
            }
            MenuKey::Enter | MenuKey::ModEnter => {
                if self.open_kind() == Some(PickerKind::HarnessModel) {
                    self.activate_model_row(cx);
                } else if self.open_kind() == Some(PickerKind::Checkout) {
                    let kind = if self.active == 0 {
                        CheckoutKind::Local
                    } else {
                        CheckoutKind::NewWorktree
                    };
                    self.pick_checkout(kind, cx);
                } else {
                    self.on_search_submit(cx);
                }
                cx.stop_propagation();
            }
            _ => {}
        }
    }
}
