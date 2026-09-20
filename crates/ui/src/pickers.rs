//! Composer pickers (feature-inventory §1.7): RepoPicker (recents + search +
//! in-app folder browser + clone/create), BranchPicker (search + isolated-
//! worktree toggle), HarnessModelPicker (harness rail + model list, harness
//! locked once the chat exists), TraitsPicker (reasoning ladder + advertised
//! model options; trigger shows the non-default summary "High · 1M · Fast").
//!
//! All selections accumulate into a [`DraftConfig`] the composer threads into
//! the Run command and the `Mutate createChat` call on first send.
//!
//! Pure logic (repo ordering, folder-browser navigation, traits summary) lives
//! in free functions with unit tests; RPC results land in [`Loadable`] slots
//! rendered as skeletons / inline errors with Retry.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable as _, KeyDownEvent, SharedString,
    Subscription, Task, Window, div, prelude::*, px,
};

use zeron_engine::registry::HarnessDescriptor;
use zeron_proto::{
    ChatConfig, FolderListing, HarnessId, Model, ReasoningLevel, RepoRef, SandboxLevel, Space,
};
use zeron_rpc::methods;


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

const FOOTER_CHIP_RADIUS: f32 = 6.0;

/// Both sides of the composer handoff share one leading-aligned workspace
/// cluster. Available width belongs after the pair, never between its labels.
fn workspace_footer_row() -> gpui::Div {
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(4.0))
}

use crate::composer::{ComposerInput, ComposerInputEvent};
use crate::motion;
use crate::popover::{self, Loadable, MenuKey};
use crate::settings::composer::ComposerDefaults;
use crate::state::{AppState, EngineHandle};
use crate::theme::Theme;

/// Dev/testing knob: `ZERON_SLOW_CATALOG_MS=<ms>` delays every harness and
/// model catalog result app-side — the chip/tab/list loading states are
/// sub-second against a warm local daemon and unstageable otherwise
/// (headless-rig captures; same family as `ZERON_OPEN_PICKER`).
fn slow_catalog_delay() -> Option<std::time::Duration> {
    std::env::var("ZERON_SLOW_CATALOG_MS")
        .ok()
        .and_then(|ms| ms.parse::<u64>().ok())
        .map(std::time::Duration::from_millis)
}


pub mod browser;
pub mod config;
pub mod git_picker;
pub mod model_catalog;
pub mod space_picker;
#[cfg(test)]
mod tests;

pub use browser::*;
pub use config::*;
pub use git_picker::*;
pub use model_catalog::*;

pub struct Pickers {
    pub(crate) state: Entity<AppState>,
    pub(crate) config: DraftConfig,
    /// Sticky last-used picks (zeron `zeron.composer.defaults:v1`): seeds the
    /// new-chat chips and is rewritten on every new-chat pick.
    pub(crate) defaults: ComposerDefaults,
    /// Where [`Self::defaults`] persists (`{data_dir}/composer-defaults.json`);
    /// `None` before bootstrap stamps the state (writes are skipped).
    pub(crate) data_dir: Option<PathBuf>,
    /// Selection the draft picks belong to — switching chats drops them so a
    /// pick made in one chat never leaks into another.
    pub(crate) draft_owner: Option<String>,
    /// Space the branch draft/cache belong to (see the state observer).
    pub(crate) space_owner: Option<String>,
    pub(crate) device_owner: Option<String>,
    pub(crate) target_generation: u64,
    pub(crate) open: popover::Popup<PickerKind>,
    /// The harness/model picker's rail selection (favorites vs the effective
    /// harness's list). Re-primed on every open.
    pub(crate) model_rail: ModelRail,
    pub(crate) setting_menu: Option<ModelSetting>,
    pub(crate) setting_active: usize,
    pub(crate) setting_on_left: bool,
    pub(crate) setting_intent_origin: Option<gpui::Point<gpui::Pixels>>,
    pub(crate) setting_hover_pending: Option<ModelSetting>,
    pub(crate) setting_hover_pointer: Option<gpui::Point<gpui::Pixels>>,
    pub(crate) setting_hover_task: Option<Task<()>>,
    pub(crate) model_space_below: Option<f32>,
    pub(crate) setting_bounds: Option<gpui::Bounds<gpui::Pixels>>,
    pub(crate) setting_scroll: gpui::ScrollHandle,
    pub(crate) harnesses: Loadable<Vec<HarnessDescriptor>>,
    pub(crate) models: HashMap<HarnessId, Loadable<Vec<Model>>>,
    pub(crate) refs: Loadable<Vec<RepoRef>>,
    /// Space id the `refs` slot belongs to (invalidated on space change).
    pub(crate) refs_space: Option<String>,
    /// Highlighted row in the open list (keyboard nav).
    pub(crate) active: usize,
    /// Models-list scroll — keyboard nav keeps the highlighted row in view.
    /// A `UniformListScrollHandle`: the model list virtualizes (7k-model
    /// catalogs must scroll smoothly), and this is its handle; the plain
    /// base handle inside serves the floating scrollbar's metrics.
    pub(crate) model_scroll: gpui::UniformListScrollHandle,
    /// Flattened rows the list/keyboard/⌘N all walk, cached per
    /// [`ModelRowsKey`]: a 7k-model catalog rebuilt+ranked on every
    /// keystroke, arrow press AND render was the picker's open/scroll lag.
    pub(crate) model_rows_cache: std::cell::RefCell<Option<(ModelRowsKey, std::sync::Arc<Vec<ModelRowData>>)>>,
    /// Bumped on every catalog/favorites mutation; invalidates the cache.
    pub(crate) catalog_rev: u64,
    /// Hover/drag state of the floating menu scrollbar. One instance serves
    /// every picker list like `menu_scroll` does — the popups are mutually
    /// exclusive, so only one list mounts at a time.
    pub(crate) menu_bar: popover::MenuScrollbarState,
    /// Scroll handle shared by the plain-div picker lists (branch, project,
    /// device) — the popups are mutually exclusive, so only one mounts at a
    /// time and a fresh open resets the offset.
    pub(crate) menu_scroll: gpui::ScrollHandle,
    /// Shared search / URL / name input, reused across popovers.
    pub(crate) search: Entity<ComposerInput>,
    /// One-shot mute for the next Edited event's highlight reset — armed by
    /// [`Self::toggle`]'s programmatic clear (see the subscription).
    pub(crate) search_reset_muted: bool,
    pub(crate) focus: FocusHandle,
    /// `ZERON_OPEN_PICKER` boot: keep claiming focus until it sticks, so
    /// keyboard nav drives the data-side-opened popover (headless rigs have
    /// no synthetic pointer, but synthetic keys do arrive).
    pub(crate) boot_focus_pending: bool,
    /// Reclaim focus while mounting: shell recovery can replace an immediate
    /// focus request while the menu is still absent from the dispatch tree.
    pub(crate) focus_on_mount: bool,
    pub(crate) load_task: Option<Task<()>>,
    /// Own slot: the refs load runs concurrently with the eager
    /// harness/model loads — sharing `load_task` would abort one mid-flight.
    pub(crate) refs_task: Option<Task<()>>,
    /// In-flight mid-session `SwitchRef` (the ref being switched to).
    pub(crate) switching: Option<String>,
    pub(crate) switch_task: Option<Task<()>>,
    /// Last mid-session switch failure (shown in the ref popover).
    pub(crate) switch_error: Option<String>,
    pub(crate) mutate_task: Option<Task<()>>,
    pub(crate) _search_events: Subscription,
    pub(crate) _state_observe: Subscription,
    pub(crate) _catalog_observe: Subscription,
}

impl gpui::EventEmitter<ReturnComposerFocus> for Pickers {}

impl Pickers {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| {
            ComposerInput::with_context("Search…", "PaletteSearch", cx)
                .with_accessibility_role(gpui::Role::SearchInput)
        });
        let search_events = cx.subscribe(&search, |this: &mut Self, _, event, cx| match event {
            ComposerInputEvent::Edited => {
                // Typing in a filter resets the highlight to the top of the
                // fresh results. `set_text` emits Edited on programmatic
                // clears too, and this subscription runs AFTER `toggle`
                // returns — an unmuted reset clobbers the just-anchored
                // selected row back to 0, leaving the top row wearing a
                // second highlight next to the selection (user report;
                // `toggle` arms the mute right before its clear).
                if !std::mem::take(&mut this.search_reset_muted) {
                    if matches!(
                        this.open_kind(),
                        Some(PickerKind::Branch | PickerKind::Space | PickerKind::Device)
                    ) {
                        this.active = 0;
                    }
                    if this.open_kind() == Some(PickerKind::HarnessModel) {
                        this.setting_menu = None;
                        this.setting_bounds = None;
                        this.active = 0;
                        this.model_scroll_base().set_offset(gpui::Point::default());
                    }
                }
                cx.notify();
            }
            ComposerInputEvent::Submitted | ComposerInputEvent::ModifiedSubmitted => {
                this.on_search_submit(cx)
            }
            // Pasted images/files don't apply to a search box.
            ComposerInputEvent::PastedImages(_)
            | ComposerInputEvent::PastedPaths(_)
            | ComposerInputEvent::CursorMoved
            | ComposerInputEvent::ViewportChanged
            | ComposerInputEvent::MentionNavigate(_)
            | ComposerInputEvent::MentionAccept
            | ComposerInputEvent::MentionDismiss => {}
        });
        // Chat selection / config changes must re-render the chips (child views
        // only re-render on their own notify). A selection change also drops
        // the draft picks — they belonged to the previous chat/new-chat canvas.
        let state_observe = cx.observe(&state, |this: &mut Self, state, cx| {
            let selected = state.read(cx).selected_chat.clone();
            if selected != this.draft_owner {
                this.draft_owner = selected;
                this.config.harness = None;
                this.config.model = None;
                this.config.reasoning = None;
                this.switch_error = None;
            }
            // A space switch invalidates the branch draft + cache — the folder
            // (and possibly the device) changed under them.
            let space = state.read(cx).selected_space.clone();
            let device = state.read(cx).effective_device_id();
            if space != this.space_owner || device != this.device_owner {
                this.space_owner = space;
                this.device_owner = device;
                this.target_generation = this.target_generation.wrapping_add(1);
                this.setting_menu = None;
                this.setting_bounds = None;
                this.refs_task = None;
                this.load_task = None;
                this.config.branch = None;
                this.config.checkout = CheckoutKind::default();
                this.refs = Loadable::Idle;
                this.refs_space = None;
                // Catalogs are per-DEVICE (fetched from the space's host):
                // a space switch may land on another device, so refetch.
                this.harnesses = Loadable::Idle;
                this.models.clear();
                this.catalog_rev += 1;
            }
            cx.notify();
        });
        // A Settings → Agents toggle changed some device's enabled set:
        // force-refresh the cached catalog so the rail/chips follow without a
        // restart (stale rows stay visible while the reload runs).
        let catalog_observe = cx.observe_global::<HarnessCatalogChanged>(|this: &mut Self, cx| {
            this.ensure_harnesses(true, cx);
            cx.notify();
        });
        // Dev/testing knob: `ZERON_OPEN_PICKER=model|traits|repo|branch` boots
        // with that popover open — synthetic input can't reach the app on
        // headless compositors, so captures need a data-side path.
        let boot_open = match std::env::var("ZERON_OPEN_PICKER").ok().as_deref() {
            Some("model") => Some(PickerKind::HarnessModel),
            Some("traits") => Some(PickerKind::HarnessModel),
            Some("branch") => Some(PickerKind::Branch),
            Some("checkout") => Some(PickerKind::Checkout),
            Some("project") => Some(PickerKind::Space),
            Some("device") => Some(PickerKind::Device),
            _ => None,
        };
        let mut open = popover::Popup::default();
        if let Some(kind) = boot_open {
            open.open(kind);
        }
        // Sticky last-used picks: loaded synchronously so the very first frame
        // shows the remembered harness/model/reasoning, never a placeholder.
        let data_dir = state.read(cx).data_dir.clone();
        let defaults = data_dir
            .as_deref()
            .map(ComposerDefaults::load)
            .unwrap_or_default();
        // Restore explicit opt-outs as well as project picks before the first frame.
        state.update(cx, |s, _| s.restore_composer_target(&defaults));
        let draft_owner = state.read(cx).selected_chat.clone();
        let space_owner = state.read(cx).selected_space.clone();
        let device_owner = state.read(cx).effective_device_id();
        Self {
            state,
            space_owner,
            device_owner,
            target_generation: 0,
            config: DraftConfig::default(),
            defaults,
            data_dir,
            draft_owner,
            open,
            model_rail: ModelRail::default(),
            setting_menu: None,
            setting_active: 0,
            setting_on_left: false,
            setting_intent_origin: None,
            setting_hover_pending: None,
            setting_hover_pointer: None,
            setting_hover_task: None,
            model_space_below: None,
            setting_bounds: None,
            setting_scroll: gpui::ScrollHandle::new(),
            harnesses: Loadable::Idle,
            models: HashMap::new(),
            refs: Loadable::Idle,
            refs_space: None,
            active: 0,
            model_scroll: gpui::UniformListScrollHandle::new(),
            model_rows_cache: std::cell::RefCell::new(None),
            catalog_rev: 0,
            menu_bar: popover::MenuScrollbarState::default(),
            menu_scroll: gpui::ScrollHandle::new(),
            search,
            search_reset_muted: false,
            focus: cx.focus_handle(),
            boot_focus_pending: boot_open.is_some(),
            focus_on_mount: false,
            load_task: None,
            refs_task: None,
            switching: None,
            switch_task: None,
            switch_error: None,
            mutate_task: None,
            _search_events: search_events,
            _state_observe: state_observe,
            _catalog_observe: catalog_observe,
        }
    }

    /// Persist the sticky defaults (best-effort; picks are rare and tiny).
    fn save_defaults(&self) {
        if let Some(dir) = self.data_dir.as_deref()
            && let Err(err) = self.defaults.save(dir)
        {
            tracing::warn!(error = %err, "composer-defaults save failed");
        }
    }

    pub fn draft(&self) -> &DraftConfig {
        &self.config
    }

    /// Harness is locked once the chat exists (feature-inventory §1.7).
    fn harness_locked(&self, cx: &App) -> bool {
        self.state.read(cx).selected_chat.is_some()
    }

    pub(crate) fn engine(&self, cx: &App) -> Option<EngineHandle> {
        self.state.read(cx).engine().cloned()
    }

    /// The selected target device when it differs from the connected
    /// engine's own — harness/model catalogs come from the device that RUNS
    /// the agents (the CLIs live there; the viewer may have neither claude
    /// nor codex installed — user report: "can't load codex models/traits
    /// anywhere" from a Mac without codex).
    fn space_target(&self, cx: &App) -> Option<String> {
        let state = self.state.read(cx);
        let device = state.effective_device_id()?;
        (state.local_device_id.as_deref() != Some(device.as_str())).then_some(device)
    }

    /// Effective harness: picked, or the chat's config, or the first listed.
    fn effective_harness(&self, cx: &App) -> Option<HarnessId> {
        if let Some(harness) = self.config.harness {
            return Some(harness);
        }
        if let Some(config) = self
            .state
            .read(cx)
            .selected_chat_row()
            .and_then(|c| c.config.as_ref())
        {
            return Some(config.harness);
        }
        // New-chat canvas: the remembered last-used harness (sticky defaults),
        // when the loaded catalog still offers it (the device may have
        // disabled it in Settings → Agents since).
        if let Some(harness) = self.defaults.harness {
            let offered = match self.harnesses.ready() {
                Some(list) => offered_harnesses(list).iter().any(|d| d.id == harness),
                None => true, // catalog not loaded yet — trust the memory
            };
            if offered {
                return Some(harness);
            }
        }
        // Fall back to the first OFFERED harness: the registry lists the mock
        // harness first, and resolving chips against it would boot the
        // new-chat canvas onto "Mock" instead of Claude Code + its default
        // model (it stays available under `ZERON_HARNESS=mock`).
        self.harnesses
            .ready()
            .and_then(|list| offered_harnesses(list).first().map(|d| d.id))
    }

    /// Effective model id: the draft pick, the selected chat's config, or (on
    /// the new-chat canvas) the remembered last-used model for the harness.
    fn effective_model_id<'a>(&'a self, cx: &'a App) -> Option<&'a str> {
        if let Some(id) = self.config.model.as_deref() {
            return Some(id);
        }
        if let Some(chat) = self.state.read(cx).selected_chat_row() {
            return chat.config.as_ref().and_then(|c| c.model.as_deref());
        }
        let harness = self.effective_harness(cx)?;
        self.defaults.model_for(harness).map(|m| m.id.as_str())
    }

    /// Effective reasoning — always concrete once the model is known: the
    /// draft pick / chat config / remembered default, clamped to the selected
    /// model's ladder, falling back to the model's default level.
    fn effective_reasoning(&self, cx: &App) -> Option<ReasoningLevel> {
        let explicit = self.config.reasoning.or_else(|| {
            match self.state.read(cx).selected_chat_row() {
                Some(chat) => chat.config.as_ref().and_then(|c| c.reasoning),
                // New chat: the remembered last-used level.
                None => self.defaults.reasoning,
            }
        });
        if self.selected_model(cx).is_none() {
            // Catalog not loaded yet: show the explicit value as-is (nothing
            // to clamp against); it resolves to a concrete level on load.
            return explicit;
        }
        clamp_reasoning(explicit, &self.trait_ladder(cx))
    }

    /// The selected model — concrete from the moment the list loads: the
    /// effective id when the list still offers it, else the harness default
    /// (first row). Never `None` with a non-empty catalog.
    fn selected_model<'a>(&'a self, cx: &'a App) -> Option<&'a Model> {
        let harness = self.effective_harness(cx)?;
        let models = self.models.get(&harness)?.ready()?;
        match self.effective_model_id(cx) {
            Some(id) => models
                .iter()
                .find(|m| m.id == id)
                .or_else(|| default_model(models)),
            None => default_model(models),
        }
    }

    /// The explicit (non-default) option picks: the chat's persisted
    /// selections for existing chats, the remembered picks for the model the
    /// new-chat canvas resolves to (same id [`Self::resolved`] sends).
    fn explicit_options(&self, cx: &App) -> serde_json::Map<String, serde_json::Value> {
        if let Some(chat) = self.state.read(cx).selected_chat_row() {
            return chat
                .config
                .as_ref()
                .map(|c| c.model_options.clone())
                .unwrap_or_default();
        }
        let Some(harness) = self.effective_harness(cx) else {
            return Default::default();
        };
        match self.selected_model(cx) {
            Some(model) => offered_options(
                model,
                self.defaults
                    .model_options_for(harness, &model.id)
                    .cloned()
                    .unwrap_or_default(),
            ),
            // Catalog not loaded (or failed): the picks were validated for
            // this exact model when made, so they are safe to send as-is.
            None => self
                .effective_model_id(cx)
                .and_then(|id| self.defaults.model_options_for(harness, id))
                .cloned()
                .unwrap_or_default(),
        }
    }

    /// The catalog is loaded and offers nothing runnable — the no-agents
    /// state (every enabled harness is missing its CLI, or nothing is
    /// enabled). False while the catalog is still loading or failed
    /// (nothing to conclude yet; offline sends must not be blocked on it).
    pub fn no_agents_available(&self) -> bool {
        self.harnesses
            .ready()
            .is_some_and(|list| offered_harnesses(list).is_empty())
    }

    /// The fully-resolved config the composer threads into the Run request and
    /// `Mutate createChat`: concrete model + reasoning whenever the catalog is
    /// loaded (no "engine picks a default" passthrough).
    pub fn resolved(&self, cx: &App) -> ResolvedRunConfig {
        ResolvedRunConfig {
            harness: self.effective_harness(cx),
            model: self
                .selected_model(cx)
                .map(|m| m.id.clone())
                // Catalog not loaded (offline): still send the id we know.
                .or_else(|| self.effective_model_id(cx).map(str::to_string)),
            reasoning: self.effective_reasoning(cx),
            model_options: self.explicit_options(cx),
        }
    }

    // ---- open/close ----

    /// The picker that's open AND interactive — `None` while one animates out.
    pub(crate) fn open_kind(&self) -> Option<PickerKind> {
        self.open.as_open().copied()
    }

    /// Whether any picker popover is open (shell-side: session-nav shortcuts
    /// go quiet underneath an open popover instead of yanking the session out
    /// from under it).
    pub fn is_open(&self) -> bool {
        self.open.as_open().is_some()
    }

    /// The picker to render: open or mid-exit.
    fn mounted_kind(&self) -> Option<PickerKind> {
        self.open.get().copied()
    }

    /// Begin the exit animation (shared by every close path).
    pub(crate) fn animate_close(&mut self, cx: &mut Context<Self>) {
        if self.is_open() {
            cx.emit(ReturnComposerFocus);
        }
        self.dismiss(cx);
    }

    /// Outside clicks and navigation keep focus at the clicked destination.
    pub(crate) fn dismiss(&mut self, cx: &mut Context<Self>) {
        self.focus_on_mount = false;
        self.cancel_setting_hover();
        self.setting_menu = None;
        self.setting_bounds = None;
        self.menu_bar = popover::MenuScrollbarState::default();
        if self.open.begin_close() {
            popover::reap_popup(cx, |pickers: &mut Self| &mut pickers.open);
        }
        cx.notify();
    }

    pub(crate) fn close(&mut self, cx: &mut Context<Self>) {
        self.animate_close(cx);
        cx.notify();
    }

    /// Capture knob (`ZERON_OPEN_DIALOG=model`): open the combined
    /// harness/model menu programmatically.
    /// A jump-slot press while the model menu is open. The shell's session
    /// bindings (Mod+1…9) win the dispatch race — gpui runs a matched
    /// binding before any key handler — so the shell forwards the slot here
    /// instead of going quiet and eating the very chips the rows advertise
    /// (macOS field report: "cmd shortcuts do nothing in the model
    /// selector"). Returns whether the menu was open and the slot consumed.
    pub fn jump_model_slot(&mut self, slot: usize, cx: &mut Context<Self>) -> bool {
        if self.open_kind() != Some(PickerKind::HarnessModel) {
            return false;
        }
        self.activate_model_index(slot, cx);
        cx.notify();
        true
    }

    pub fn open_model_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open_kind() != Some(PickerKind::HarnessModel) {
            self.toggle(PickerKind::HarnessModel, window, cx);
        }
    }

    fn toggle(&mut self, kind: PickerKind, window: &mut Window, cx: &mut Context<Self>) {
        // A press that found this picker open closes it — the card's
        // `on_mouse_down_out` already began the close on that same press,
        // so by click time the popup reads as closed and a plain toggle
        // would reopen it. A press while a DIFFERENT picker is open doesn't
        // count (see note_trigger_press_matching): that click switches.
        let pressed_open = self.open.take_press_was_open();
        if self.open_kind() == Some(kind) || pressed_open {
            self.animate_close(cx);
            if pressed_open {
                cx.emit(ReturnComposerFocus);
            }
            cx.notify();
            return;
        }
        self.open.open(kind);
        self.focus_on_mount = true;
        // The plain-div menus (branch / project / device) share one scroll
        // handle; a fresh open starts at the top. The model list resets its
        // own virtualized handle below. Sync the rail baselines so the jump
        // back to the top isn't read as scrolling.
        if kind != PickerKind::HarnessModel {
            popover::reset_menu_scroll(&self.menu_scroll, &mut self.menu_bar);
        }
        // Clearing stale text emits Edited AFTER this function returns —
        // mute that one event so its reset can't clobber the highlight
        // anchored below (the no-op clear is also skipped for the same
        // reason).
        self.search_reset_muted = !self.search.read(cx).text().is_empty();
        self.search.update(cx, |input, cx| {
            input.set_placeholder("Search…", cx);
            if !input.text().is_empty() {
                input.set_text("", cx);
            }
        });
        // Prime the model picker's rail BEFORE anchoring the highlight (the
        // visible rows depend on it): the favorites view when stars exist —
        // t3 ModelPickerContent's initial selection — else the effective
        // harness. Locked chats stay on their own harness.
        if kind == PickerKind::HarnessModel {
            self.model_rail = if !self.harness_locked(cx) && !self.defaults.favorites.is_empty() {
                ModelRail::Favorites
            } else {
                ModelRail::Harness
            };
        }
        // The keyboard-nav highlight starts ON the selected row — row 0
        // otherwise reads as a second active row (user report).
        self.active = match kind {
            PickerKind::Checkout => match self.config.checkout {
                CheckoutKind::Local => 0,
                CheckoutKind::NewWorktree => 1,
            },
            PickerKind::Branch => self.selected_ref_index(cx),
            PickerKind::HarnessModel => self.selected_model_index(cx),
            PickerKind::Space => self.selected_space_index(cx),
            PickerKind::Device => self.selected_device_index(cx),
        };
        if kind == PickerKind::HarnessModel {
            // scroll_to_item below may land anywhere; the first note of the
            // settled position is the fresh baseline, not scroll motion.
            popover::reset_menu_scroll(&self.model_scroll_base(), &mut self.menu_bar);
            self.model_scroll
                .scroll_to_item(self.active, gpui::ScrollStrategy::Nearest);
        }
        // Searchable pickers focus the filter input (it sits inside the frame,
        // so the frame's key handler still sees arrows/Enter); the rest focus
        // the frame itself for pure keyboard nav.
        match kind {
            PickerKind::Branch => {
                self.switch_error = None; // stale mid-session failures don't linger
                let handle = self.search.read(cx).focus_handle(cx);
                self.search.update(cx, |input, cx| {
                    input.set_placeholder("Search refs…", cx);
                });
                window.focus(&handle, cx);
            }
            PickerKind::Space => {
                let handle = self.search.read(cx).focus_handle(cx);
                self.search.update(cx, |input, cx| {
                    input.set_placeholder("Search projects…", cx);
                });
                window.focus(&handle, cx);
            }
            PickerKind::Device => {
                let handle = self.search.read(cx).focus_handle(cx);
                self.search.update(cx, |input, cx| {
                    input.set_placeholder("Search devices…", cx);
                });
                window.focus(&handle, cx);
            }
            PickerKind::HarnessModel => {
                let handle = self.search.read(cx).focus_handle(cx);
                self.search.update(cx, |input, cx| {
                    input.set_placeholder("Search models…", cx);
                });
                window.focus(&handle, cx);
            }
            _ => window.focus(&self.focus, cx),
        }
        match kind {
            // Force: the checkout state moves under us (a send mints a
            // worktree+branch, terminals switch refs) — every open
            // revalidates, keeping stale rows visible until fresh ones land.
            PickerKind::Branch | PickerKind::Checkout => self.ensure_refs(true, cx),
            PickerKind::HarnessModel => {
                // Force: the enabled set moves under us (Settings → Agents,
                // possibly from another viewer) — every open revalidates,
                // keeping current rows visible until the fresh catalog lands.
                self.ensure_harnesses(true, cx);
                // Model discovery can recover after a slow/plugin-heavy ACP
                // cold start. Revalidate on every open instead of pinning a
                // timeout/fallback result until the application restarts.
                self.prefetch_models(true, cx);
            }
            // Projects and devices are already synced state — nothing to load.
            PickerKind::Space | PickerKind::Device => {}
        }
        cx.notify();
    }

    // ---- loads ----

    fn ensure_harnesses(&mut self, force: bool, cx: &mut Context<Self>) {
        // Non-forced (the render loop's eager kick) only loads from Idle: an
        // Error that could re-trigger a load would flip back to Loading
        // before the retry row ever painted (and spam the engine); Retry
        // resets to Idle. FORCED refreshes (a Settings → Agents toggle, a
        // picker open) reload through Ready/Error too — the enabled set just
        // changed under the cache, which otherwise served the boot-time
        // catalog until restart (user report). Stale-while-revalidate: loaded
        // rows stay on screen while the fresh catalog lands.
        let reload = match self.harnesses {
            Loadable::Idle => true,
            Loadable::Loading => false,
            Loadable::Ready(_) | Loadable::Error(_) => force,
        };
        if !reload {
            return;
        }
        let Some(engine) = self.engine(cx) else {
            return;
        };
        let target = self.space_target(cx);
        let generation = self.target_generation;
        if !matches!(self.harnesses, Loadable::Ready(_)) {
            self.harnesses = Loadable::Loading;
            self.catalog_rev += 1;
        }
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let mut params = serde_json::Map::new();
            if let Some(target) = &target {
                params.insert(
                    "targetDeviceId".into(),
                    serde_json::Value::String(target.clone()),
                );
            }
            let result = engine
                .client()
                .call(methods::LIST_HARNESSES, serde_json::Value::Object(params))
                .await;
            if let Some(delay) = slow_catalog_delay() {
                cx.background_executor().timer(delay).await;
            }
            this.update(cx, |pickers, cx| {
                if pickers.target_generation != generation {
                    return;
                }
                pickers.catalog_rev += 1;
                pickers.harnesses = match result {
                    Ok(value) => match serde_json::from_value::<Vec<HarnessDescriptor>>(value) {
                        Ok(list) => Loadable::Ready(list),
                        Err(err) => Loadable::Error(err.to_string()),
                    },
                    Err(err) => Loadable::Error(err.to_string()),
                };
                pickers.prefetch_models(false, cx);
                cx.notify();
            })
            .ok();
        }));
    }

    /// Kick a model load for the effective harness AND every offered one, in
    /// parallel — by the time the user opens the picker (or switches rail
    /// tabs) the lists are already there, instead of a per-selection
    /// "Loading models…" round-trip. Each `ensure_models` call is guarded by
    /// its slot state, so re-running this every catalog load/render is free.
    fn prefetch_models(&mut self, force: bool, cx: &mut Context<Self>) {
        let mut targets: Vec<HarnessId> = match self.harnesses.ready() {
            Some(list) => offered_harnesses(list).iter().map(|d| d.id).collect(),
            None => Vec::new(),
        };
        // The committed chat's harness may be outside the offered set (e.g.
        // disabled after the chat was created) — its models still matter.
        if let Some(effective) = self.effective_harness(cx)
            && !targets.contains(&effective)
        {
            targets.push(effective);
        }
        for harness in targets {
            self.ensure_models(harness, force, cx);
        }
    }

    fn ensure_models(&mut self, harness: HarnessId, force: bool, cx: &mut Context<Self>) {
        // Normal prefetches load absent/Idle slots once. Picker-open refreshes
        // also retry Ready/Error slots, while an in-flight load is always
        // reused. Ready rows stay visible until the replacement lands.
        let reload = match self.models.get(&harness) {
            None | Some(Loadable::Idle) => true,
            Some(Loadable::Loading) => false,
            Some(Loadable::Ready(_)) | Some(Loadable::Error(_)) => force,
        };
        if !reload {
            return;
        }
        let Some(engine) = self.engine(cx) else {
            return;
        };
        let target = self.space_target(cx);
        let generation = self.target_generation;
        if !matches!(self.models.get(&harness), Some(Loadable::Ready(_))) {
            self.models.insert(harness, Loadable::Loading);
            self.catalog_rev += 1;
        }
        cx.spawn(async move |this, cx| {
            let mut params = serde_json::json!({ "harness": harness });
            if let (Some(target), Some(object)) = (&target, params.as_object_mut()) {
                object.insert(
                    "targetDeviceId".into(),
                    serde_json::Value::String(target.clone()),
                );
            }
            // A plugin-heavy OpenCode cold start can fail once while caches,
            // MCP servers, or plugin runtimes are still warming. Keep this
            // single Loading slot alive for two retries so recovery requires
            // no picker close/reopen and cannot launch duplicate probes.
            let mut attempt = 1_u64;
            let result = loop {
                let result = engine
                    .client()
                    .call(methods::LIST_MODELS, params.clone())
                    .await;
                if result.is_ok() || harness != HarnessId::Opencode || attempt >= 3 {
                    break result;
                }
                if let Err(error) = &result {
                    tracing::warn!(
                        %error,
                        attempt,
                        "OpenCode model discovery failed; retrying automatically"
                    );
                }
                if this.update(cx, |_, _| {}).is_err() {
                    return;
                }
                cx.background_executor()
                    .timer(Duration::from_secs(attempt * 2))
                    .await;
                attempt += 1;
            };
            if let Some(delay) = slow_catalog_delay() {
                cx.background_executor().timer(delay).await;
            }
            this.update(cx, |pickers, cx| {
                if pickers.target_generation != generation {
                    return;
                }
                let loaded = match result {
                    Ok(value) => match serde_json::from_value::<Vec<Model>>(value) {
                        // Display hygiene for catalogs from older engines
                        // (`default` alias rows, orphan `[1m]` variants,
                        // version-less alias labels).
                        Ok(models) => Loadable::Ready(normalize_model_rows(harness, models)),
                        Err(err) => Loadable::Error(err.to_string()),
                    },
                    Err(err) => Loadable::Error(err.to_string()),
                };
                if let Loadable::Ready(models) = &loaded {
                    let fresh = pickers
                        .defaults
                        .remember_labels(models.iter().map(|m| (m.id.as_str(), m.label.as_str())));
                    if fresh {
                        pickers.save_defaults();
                    }
                }
                pickers.models.insert(harness, loaded);
                pickers.catalog_rev += 1;
                // A list that landed while its popover is open re-anchors the
                // keyboard highlight onto the selected row (it sat at 0 while
                // loading).
                if pickers.open_kind() == Some(PickerKind::HarnessModel)
                    && pickers.effective_harness(cx) == Some(harness)
                {
                    pickers.active = pickers.selected_model_index(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
    // ---- selections ----


    fn pick_harness(&mut self, harness: HarnessId, cx: &mut Context<Self>) {
        if self.harness_locked(cx) {
            return;
        }
        if self.config.harness != Some(harness) {
            // The remembered model for this harness takes over via the
            // defaults fallback; a foreign pick must not linger.
            self.config.model = None;
            self.config.reasoning = None;
        }
        self.config.harness = Some(harness);
        self.defaults.harness = Some(harness);
        self.save_defaults();
        self.model_scroll_base().set_offset(gpui::Point::default());
        self.ensure_models(harness, false, cx);
        // Re-anchor the keyboard highlight onto the new harness's selected row.
        self.active = self.selected_model_index(cx);
        cx.notify();
    }

    fn pick_model(&mut self, model_id: String, cx: &mut Context<Self>) {
        self.setting_menu = None;
        // The card stays open on a pick (user request): model and traits
        // share one popover now, and adjusting the tray right after choosing
        // a model is the expected flow. Esc, click-out, or the chip close it.
        if self.state.read(cx).selected_chat.is_some() {
            // Existing chat: persist to the chat row (Mutate setChatConfig) —
            // survives restarts and syncs; next runs in this chat use it.
            self.update_chat_config(cx, move |config| config.model = Some(model_id));
        } else {
            // New chat: draft pick + sticky last-used memory for this harness.
            self.config.model = Some(model_id.clone());
            if let Some(harness) = self.effective_harness(cx) {
                let label = self
                    .models
                    .get(&harness)
                    .and_then(|l| l.ready())
                    .and_then(|models| models.iter().find(|m| m.id == model_id))
                    .map(|m| m.label.clone())
                    .unwrap_or_else(|| model_id.clone());
                self.defaults.remember_model(harness, model_id, label);
                self.save_defaults();
            }
        }
        cx.notify();
    }

    fn pick_reasoning(&mut self, level: ReasoningLevel, cx: &mut Context<Self>) {
        // Always a concrete selection (no toggle-back-to-default).
        if self.state.read(cx).selected_chat.is_some() {
            self.update_chat_config(cx, move |config| config.reasoning = Some(level));
        } else {
            self.config.reasoning = Some(level);
            self.defaults.reasoning = Some(level);
            self.save_defaults();
        }
        cx.notify();
    }

    fn pick_option(
        &mut self,
        option_id: String,
        choice_id: String,
        default: bool,
        cx: &mut Context<Self>,
    ) {
        if self.state.read(cx).selected_chat.is_some() {
            self.update_chat_config(cx, move |config| {
                if default {
                    config.model_options.remove(&option_id);
                } else {
                    config
                        .model_options
                        .insert(option_id, serde_json::Value::String(choice_id));
                }
            });
        } else if let Some(harness) = self.effective_harness(cx)
            && let Some(model) = self
                .selected_model(cx)
                .filter(|m| m.options.iter().any(|o| o.id == option_id))
                .map(|m| m.id.clone())
        {
            // New chat: the pick is the sticky memory for the catalog model
            // that offered it — never stored unvalidated.
            let options = self.defaults.model_options_mut(harness, &model);
            if default {
                options.remove(&option_id);
            } else {
                options.insert(option_id, serde_json::Value::String(choice_id));
            }
            self.save_defaults();
        }
        cx.notify();
    }

    /// Apply `change` to the selected chat's effective config and persist it:
    /// optimistic row stamp (chips update on click) + `Mutate setChatConfig`
    /// (LWW workspace write — restarts and other devices see it). The written
    /// row always carries the CONCRETE resolved model/reasoning, with the
    /// reasoning re-clamped to the (possibly just-changed) model's ladder.
    fn update_chat_config(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut ChatConfig)) {
        let Some(chat_id) = self.state.read(cx).selected_chat.clone() else {
            return;
        };
        let resolved = self.resolved(cx);
        let Some(mut config) = resolved.chat_config() else {
            return; // harness unknown (catalog + chat row both missing) — nothing safe to write
        };
        // Preserve fields the pickers don't own.
        if let Some(existing) = self
            .state
            .read(cx)
            .selected_chat_row()
            .and_then(|c| c.config.as_ref())
        {
            config.sandbox = existing.sandbox;
        }
        change(&mut config);
        // Reasoning must stay concrete for whatever model the row now names —
        // same ladder resolution as [`Self::trait_ladder`] (model levels, else
        // the harness's advertised ladder).
        if let Some(models) = self.models.get(&config.harness).and_then(|l| l.ready()) {
            let mut ladder = config
                .model
                .as_deref()
                .and_then(|id| models.iter().find(|m| m.id == id))
                .map(|m| m.reasoning_levels.clone())
                .unwrap_or_default();
            if ladder.is_empty()
                && let Some(descriptor) = self
                    .harnesses
                    .ready()
                    .and_then(|list| list.iter().find(|d| d.id == config.harness))
            {
                ladder = descriptor.reasoning_levels.clone();
            }
            if !ladder.is_empty() {
                config.reasoning = clamp_reasoning(config.reasoning, &ladder);
            }
            // Options likewise: a model switch must not carry picks the new
            // model doesn't offer (e.g. a 1M context onto Haiku).
            if let Some(model) = config
                .model
                .as_deref()
                .and_then(|id| models.iter().find(|m| m.id == id))
            {
                config.model_options =
                    offered_options(model, std::mem::take(&mut config.model_options));
            }
        }
        self.state.update(cx, |state, cx| {
            state.apply_chat_config(&chat_id, config.clone());
            cx.notify();
        });
        let Some(engine) = self.engine(cx) else {
            return;
        };
        self.mutate_task = Some(cx.spawn(async move |_, _| {
            let params = serde_json::json!({
                "op": "setChatConfig",
                "chatId": chat_id,
                "config": config,
            });
            if let Err(err) = engine.client().call(methods::MUTATE, params).await {
                tracing::warn!(error = %err, "setChatConfig mutate failed");
            }
        }));
    }

    // ---- keyboard ----

    /// The traits popover's reasoning ladder (model levels, falling back to
    /// the harness's advertised ladder) — shared by render and keyboard nav.
    fn trait_ladder(&self, cx: &App) -> Vec<ReasoningLevel> {
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
    fn rail_descriptors(&self, cx: &App) -> Vec<HarnessDescriptor> {
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
    fn model_rows(&self, cx: &App) -> std::sync::Arc<Vec<ModelRowData>> {
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

    fn visible_model_rows(&self, cx: &App) -> Vec<ModelRowData> {
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
    fn selected_model_index(&self, cx: &App) -> usize {
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
    fn model_rows_len(&self, cx: &App) -> usize {
        self.model_rows(cx).len()
    }

    /// Enter on the harness/model popover: pick the highlighted model.
    fn activate_model_row(&mut self, cx: &mut Context<Self>) {
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
    fn activate_model_index(&mut self, ix: usize, cx: &mut Context<Self>) {
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
    fn toggle_model_favorite(&mut self, harness: HarnessId, model: &str, cx: &mut Context<Self>) {
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

    fn on_search_submit(&mut self, cx: &mut Context<Self>) {
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

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &Window, cx: &mut Context<Self>) {
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

    // ---- render ----

    #[allow(clippy::too_many_arguments)]
    fn trigger_chip(
        &self,
        kind: PickerKind,
        label: SharedString,
        set: bool,
        chip_icon: Option<(&'static str, Option<gpui::Hsla>)>,
        // The chip never collapses while identity resolves (user report):
        // `icon_loading` swaps the brand slot for the pixel-glyph loader
        // (harness unknown), `label_loading` swaps the text for a ghost bar
        // (model unknown).
        icon_loading: bool,
        label_loading: bool,
        suffix: Option<(SharedString, Option<gpui::Hsla>)>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let id: &'static str = match kind {
            PickerKind::Branch => "picker-branch",
            PickerKind::Checkout => "picker-checkout",
            PickerKind::HarnessModel => "picker-model",
            PickerKind::Space => "picker-space",
            PickerKind::Device => "picker-device",
        };
        let open = self.open_kind() == Some(kind);
        // Ghost pill (zeron composer/styles.tsx `pill`): `h-8 rounded-lg px-2.5
        // gap-1.5 text-[12px] font-medium text-muted-foreground`, icons size-4,
        // hover/open wash — no border, no caret; the actions row stays quiet.
        div()
            .id(id)
            .h(px(32.0))
            .max_w(px(248.0))
            // Shrinkable under row pressure — four footer chips share one
            // line; without min_w_0 they overflowed and painted overlapped.
            .min_w_0()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .px(px(10.0))
            .rounded(px(8.0))
            .text_size(crate::typography::ui_rems(12.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            // zeron composer/styles.tsx `pill`: `transition-colors` — the wash
            // and text brighten fade over 150ms.
            .text_color(motion::hover_blend(
                id,
                if set {
                    theme.text.opacity(0.9)
                } else {
                    theme.text_muted
                },
                theme.text,
            ))
            .when(kind == PickerKind::HarnessModel && theme.is_frost(), |el| {
                el.border_1().border_color(crate::theme::hairline(if theme.appearance.is_dark() { 0.12 } else { 0.08 }))
            })
            .bg({
                let is_engine = kind == PickerKind::HarnessModel;
                let rest_bg = if is_engine && theme.is_frost() {
                    crate::theme::wash(if theme.appearance.is_dark() { 0.08 } else { 0.05 })
                } else {
                    gpui::transparent_black()
                };
                let hover_bg = if is_engine && theme.is_frost() {
                    crate::theme::wash(if theme.appearance.is_dark() { 0.16 } else { 0.12 })
                } else {
                    theme.element_hover
                };
                if open {
                    hover_bg
                } else {
                    motion::hover_blend(id, rest_bg, hover_bg)
                }
            })
            .on_hover(motion::hover_listener(id))
            .cursor_pointer()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, _, _| {
                    this.open.note_trigger_press_matching(|open| *open == kind)
                }),
            )
            .on_click(cx.listener(move |this, _, window, cx| this.toggle(kind, window, cx)))
            .when(icon_loading, |el| {
                el.child(div().flex_none().child(crate::loaders::mini_glyph_spinner(
                    "picker-chip-loader",
                    2.0,
                    theme.glyph,
                    cx.entity_id(),
                    cx,
                )))
            })
            .when_some(
                (!icon_loading).then_some(chip_icon).flatten(),
                |el, (path, tint)| {
                    el.child(
                        crate::icons::icon(path)
                            .size(px(16.0))
                            .text_color(tint.unwrap_or(theme.text_muted)),
                    )
                },
            )
            .when(label_loading, |el| {
                el.child(popover::skeleton_bar(56.0, cx.entity_id(), cx))
            })
            .when(!label_loading, |el| {
                el.child(div().min_w_0().truncate().child(label))
            })
            // The effort half of the combined model+effort chip (and the space
            // chip's "@ device" tag): muted, no icon — one button, two tones.
            // `tint` overrides the muted tone (the offline warning). Under row
            // pressure the suffix yields FIRST (large shrink factor) so the
            // model name — the run's identity — truncates last.
            .when_some(suffix, |el, (suffix, tint)| {
                el.child(
                    div()
                        .flex_shrink(1000.0)
                        .min_w_0()
                        .truncate()
                        .text_color(tint.unwrap_or(theme.text_muted.opacity(0.7)))
                        .child(suffix),
                )
            })
    }

    /// A footer-row trigger (t3code ghost `Button size="xs"`): leading icon,
    /// truncating label, trailing chevron — smaller and quieter than the
    /// in-pill chips.
    fn footer_chip(
        &self,
        kind: PickerKind,
        id: &'static str,
        icon_path: &'static str,
        label: SharedString,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let open = self.open_kind() == Some(kind);
        div()
            .id(id)
            .h(px(20.0))
            .max_w(px(280.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .px(px(8.0))
            .rounded(px(FOOTER_CHIP_RADIUS))
            .text_size(crate::typography::ui_rems(12.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(motion::hover_blend(
                id,
                theme.text_muted.opacity(0.7),
                theme.text.opacity(0.8),
            ))
            .bg(if open {
                theme.element_hover
            } else {
                motion::hover_blend(id, gpui::transparent_black(), theme.element_hover)
            })
            .on_hover(motion::hover_listener(id))
            .cursor_pointer()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, _, _| {
                    this.open.note_trigger_press_matching(|open| *open == kind)
                }),
            )
            .on_click(cx.listener(move |this, _, window, cx| this.toggle(kind, window, cx)))
            .child(
                crate::icons::icon(icon_path)
                    .size(px(12.0))
                    .flex_none()
                    .text_color(theme.text_muted.opacity(0.7)),
            )
            .child(div().min_w_0().truncate().child(label))
            .child(
                crate::icons::icon(crate::icons::ALT_ARROW_DOWN)
                    .size(px(12.0))
                    .flex_none()
                    .text_color(theme.text_muted.opacity(0.5)),
            )
    }

    /// A read-only footer label (locked sessions — t3code's
    /// `resolveLockedWorkspaceLabel` span).
    fn footer_label(icon_path: &'static str, label: SharedString, theme: &Theme) -> gpui::Div {
        div()
            .h(px(20.0))
            // Four of these share one row now (device, project, checkout,
            // ref): cap each early and let them SHRINK (`min_w_0`) — without
            // it the clusters overflowed into each other and the labels
            // painted overlapped (user report).
            .max_w(px(160.0))
            .min_w_0()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .px(px(8.0))
            .text_size(crate::typography::ui_rems(12.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(theme.text_muted.opacity(0.6))
            .child(
                crate::icons::icon(icon_path)
                    .size(px(12.0))
                    .text_color(theme.text_muted.opacity(0.6)),
            )
            .child(div().min_w_0().truncate().child(label))
    }

    /// New-session destination controls. Machine and project form the
    /// original chip-only cluster floating above the composer's trailing edge.
    pub fn render_new_thread_target_selectors(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let closing = self.open.closing_since();
        let mut overlay: Option<(PickerKind, AnyElement)> = match self.mounted_kind() {
            Some(PickerKind::Space) => {
                let content = self.render_space_popover(cx);
                Some((PickerKind::Space, self.popover_frame(280.0, content, cx)))
            }
            Some(PickerKind::Device) => {
                let content = self.render_device_popover(cx);
                Some((PickerKind::Device, self.popover_frame(224.0, content, cx)))
            }
            _ => None,
        };
        let (device_label, project_label, offline) = {
            let state = self.state.read(cx);
            let device_id = state.effective_device_id();
            let device_label: SharedString = device_id
                .as_deref()
                .and_then(|id| state.device_name(id))
                .map(str::to_string)
                .unwrap_or_else(|| "This device".to_string())
                .into();
            let offline = device_id
                .as_deref()
                .is_some_and(|id| !state.device_online(id, chrono::Utc::now()));
            let project_label: SharedString = state
                .selected_space_row()
                .map(|s| s.display_name().to_string())
                .unwrap_or_else(|| "No project".to_string())
                .into();
            (device_label, project_label, offline)
        };
        let device_chip = self
            .footer_chip(
                PickerKind::Device,
                "picker-device",
                crate::icons::MONITOR,
                device_label,
                &theme,
                cx,
            )
            .when(offline, |el| el.text_color(theme.warning.opacity(0.8)));
        let project_chip = self.footer_chip(
            PickerKind::Space,
            "picker-project",
            crate::icons::FOLDER,
            project_label,
            &theme,
            cx,
        );
        div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.0))
            .child(attach_overlay(
                device_chip,
                &mut overlay,
                PickerKind::Device,
                "device-popover",
                closing,
            ))
            .child(attach_overlay_end(
                project_chip,
                &mut overlay,
                PickerKind::Space,
                "project-popover",
                closing,
            ))
            .into_any_element()
    }

    /// New-session Git controls. Checkout mode and branch form the original
    /// chip-only cluster floating below the composer's leading edge.
    pub fn render_new_thread_git_selectors(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let git = self
            .state
            .read(cx)
            .selected_space_row()
            .is_some_and(|space| space.git_detected);
        if !git {
            return None;
        }
        self.ensure_refs(false, cx);
        let theme = Theme::of(cx).clone();
        let closing = self.open.closing_since();
        let mut overlay: Option<(PickerKind, AnyElement)> = match self.mounted_kind() {
            Some(PickerKind::Branch) => {
                let content = self.render_branch_popover(cx);
                Some((PickerKind::Branch, self.popover_frame(320.0, content, cx)))
            }
            Some(PickerKind::Checkout) => {
                let content = self.render_checkout_popover(cx);
                Some((PickerKind::Checkout, self.popover_frame(224.0, content, cx)))
            }
            _ => None,
        };
        let kind_icon = match (self.config.checkout, self.selected_ref_worktree().is_some()) {
            (CheckoutKind::Local, false) => crate::icons::FOLDER,
            _ => crate::icons::FOLDER_WITH_FILES,
        };
        let checkout_chip = self.footer_chip(
            PickerKind::Checkout,
            "picker-checkout",
            kind_icon,
            SharedString::from(self.checkout_label()),
            &theme,
            cx,
        );
        let branch_chip = self.footer_chip(
            PickerKind::Branch,
            "picker-branch",
            crate::icons::GIT_BRANCH,
            self.ref_label(),
            &theme,
            cx,
        );
        Some(
            workspace_footer_row()
                .child(attach_overlay_below(
                    checkout_chip,
                    &mut overlay,
                    PickerKind::Checkout,
                    "checkout-popover",
                    closing,
                ))
                .child(attach_overlay_below(
                    branch_chip,
                    &mut overlay,
                    PickerKind::Branch,
                    "branch-popover",
                    closing,
                ))
                .into_any_element(),
        )
    }

    /// The composer footer row: checkout-kind + ref, LEFT-aligned, only when
    /// the picked (or session's) project has git. New sessions use the floating
    /// chip clusters; sessions name their target in the titlebar.
    pub fn render_footer(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = Theme::of(cx).clone();
        // A selected chat whose workspace row hasn't synced yet (the moment
        // right after send mints it) still renders the DRAFT footer — the
        // values are identical, so the toolbar never blinks through a
        // half-empty locked state.
        let (space, session, change_request) = {
            let state = self.state.read(cx);
            let space = state.selected_space_row().cloned();
            let session = state
                .selected_chat
                .as_ref()
                .and_then(|_| state.selected_chat_row().cloned());
            let change_request = session
                .as_ref()
                .and_then(|chat| state.change_request_for_chat(chat).cloned());
            (space, session, change_request)
        };
        let row = || {
            // The composer owns the row's animated reveal and negative bottom
            // margin. Keeping that geometry outside this reusable content
            // lets the new-thread route handoff collapse the footer without
            // clipping its controls or changing its steady-state spacing.
            // `w_full` is load-bearing: without it the canvas layout sizes
            // the row to CONTENT, and the left cluster's flex_1 (basis 0)
            // collapsed to zero width — both clusters painted from the same
            // origin, chips overlapping (user report).
            workspace_footer_row().px(px(10.0))
        };

        if let Some(chat) = &session {
            // Sessions never move: read-only checkout-kind + ref labels,
            // LEFT-aligned, only when the session's project has git. The
            // target (project @ device) lives in the titlebar now.
            let Some(space) = space.as_ref().filter(|s| s.git_detected) else {
                return None;
            };
            let is_worktree = chat.cwd.as_deref().is_some_and(|cwd| cwd != space.path);
            let (icon_path, label) = if is_worktree {
                (crate::icons::FOLDER_WITH_FILES, "Worktree")
            } else {
                (crate::icons::FOLDER, "Local checkout")
            };
            // Keep the same reading order and leading edge as the draft.
            let left = div()
                .flex()
                .flex_row()
                .items_center()
                .min_w_0()
                .child(Self::footer_label(
                    icon_path,
                    SharedString::from(label),
                    &theme,
                ));
            let right = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.0))
                .min_w_0()
                .child(Self::footer_label(
                    crate::icons::GIT_BRANCH,
                    chat.branch
                        .clone()
                        .map(SharedString::from)
                        .unwrap_or_else(|| SharedString::from("No ref")),
                    &theme,
                ));
            // Checkout + branch stay together. PR and usage form the trailing
            // status group, independently of the branch label's length.
            return Some(
                row()
                    .pr_0()
                    .child(left)
                    .child(right)
                    .child(div().flex_1().min_w_0())
                    .when_some(change_request, |el, summary| {
                        el.child(div().flex_none().child(
                            crate::change_requests::pull_request_badge(
                                "composer-pull-request".into(),
                                summary,
                                crate::change_requests::ChangeRequestBadgeSurface::Composer,
                                &theme,
                            ),
                        ))
                    })
                    .into_any_element(),
            );
        }

        // New-session draft: checkout + ref only, LEFT-aligned (device +
        // project live in the row above the pill now).
        let git = space.as_ref().is_some_and(|s| s.git_detected);
        if !git {
            return None;
        }
        // Refs feed the draft labels — eager + idempotent.
        self.ensure_refs(false, cx);
        let closing = self.open.closing_since();
        let mut overlay: Option<(PickerKind, AnyElement)> = match self.mounted_kind() {
            Some(PickerKind::Branch) => {
                let content = self.render_branch_popover(cx);
                Some((PickerKind::Branch, self.popover_frame(320.0, content, cx)))
            }
            Some(PickerKind::Checkout) => {
                let content = self.render_checkout_popover(cx);
                Some((PickerKind::Checkout, self.popover_frame(224.0, content, cx)))
            }
            // Space/Device popovers mount in the floating row above the pill.
            _ => None,
        };

        let ref_label = self.ref_label();
        let ref_chip = self.footer_chip(
            PickerKind::Branch,
            "picker-branch",
            crate::icons::GIT_BRANCH,
            ref_label,
            &theme,
            cx,
        );
        let kind_icon = match (self.config.checkout, self.selected_ref_worktree().is_some()) {
            (CheckoutKind::Local, false) => crate::icons::FOLDER,
            _ => crate::icons::FOLDER_WITH_FILES,
        };
        let kind_chip = self.footer_chip(
            PickerKind::Checkout,
            "picker-checkout",
            kind_icon,
            SharedString::from(self.checkout_label()),
            &theme,
            cx,
        );
        // Match the floating draft's adjacent checkout/ref pair, including
        // while the newly created session is waiting for its workspace row.
        let left = div()
            .flex()
            .flex_row()
            .items_center()
            .min_w_0()
            .child(attach_overlay(
                kind_chip,
                &mut overlay,
                PickerKind::Checkout,
                "checkout-popover",
                closing,
            ));
        let right = div()
            .flex()
            .flex_row()
            .items_center()
            .min_w_0()
            .child(attach_overlay(
                ref_chip,
                &mut overlay,
                PickerKind::Branch,
                "branch-popover",
                closing,
            ));
        Some(row().child(left).child(right).into_any_element())
    }

    fn popover_frame(&self, width: f32, content: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).for_popup();
        popover::popover_card(&theme)
            .w(px(width))
            // zeron caps its tallest picker at min(640px, 75vh).
            .max_h(px(640.0))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.on_key_down(event, window, cx)
            }))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if this.is_open() && !this.focus.contains_focused(window, cx) {
                        window.focus(&this.focus, cx);
                    }
                }),
            )
            .on_mouse_down_out(
                cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                    if this.setting_menu.is_some()
                        && this
                            .setting_bounds
                            .is_some_and(|bounds| bounds.contains(&event.position))
                    {
                        return;
                    }
                    this.dismiss(cx);
                    if this.focus.contains_focused(window, cx) {
                        window.blur();
                    }
                }),
            )
            .flex()
            .flex_col()
            .child(content)
            .into_any_element()
    }

    /// [`Self::popover_frame`] without the p-1 inset — the harness/model
    /// picker's rail + list panes bleed to the card edge (zeron
    /// harness-model-picker.tsx `className="w-80 p-0"`).
    fn popover_frame_flush(
        &self,
        width: f32,
        content: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::of(cx).for_popup();
        popover::popover_card_flush(&theme)
            .w(px(width))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.on_key_down(event, window, cx)
            }))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if this.is_open() && !this.focus.contains_focused(window, cx) {
                        window.focus(&this.focus, cx);
                    }
                }),
            )
            .on_mouse_down_out(
                cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                    if this.setting_menu.is_some()
                        && this
                            .setting_bounds
                            .is_some_and(|bounds| bounds.contains(&event.position))
                    {
                        return;
                    }
                    this.dismiss(cx);
                    if this.focus.contains_focused(window, cx) {
                        window.blur();
                    }
                }),
            )
            .flex()
            .flex_col()
            .child(content)
            .into_any_element()
    }

    pub(crate) fn search_box(&self, theme: &Theme) -> AnyElement {
        popover::search_input_frame(theme, self.search.clone().into_any_element())
            .into_any_element()
    }

    pub(crate) fn retry_row(
        &self,
        id: &'static str,
        message: &str,
        kind: PickerKind,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        popover::error_row(theme, message)
            .child(
                div()
                    .id(id)
                    .px(px(Theme::SPACE_SM))
                    .py(px(3.0))
                    .rounded(px(Theme::CONTROL_RADIUS))
                    .border_1()
                    .border_color(theme.border)
                    .text_color(theme.text)
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.element_hover))
                    .on_click(cx.listener(move |this, _, _, cx| match kind {
                        PickerKind::Branch | PickerKind::Checkout => this.ensure_refs(true, cx),
                        PickerKind::HarnessModel => {
                            this.harnesses = Loadable::Idle;
                            this.models.clear();
                            this.catalog_rev += 1;
                            this.ensure_harnesses(false, cx);
                        }
                        // Projects/devices load nothing; no retry surface exists.
                        PickerKind::Space | PickerKind::Device => {}
                    }))
                    .child(SharedString::from("Retry")),
            )
            .into_any_element()
    }

    /// The virtualized list's plain scroll handle (bounds/offset for the
    /// floating scrollbar; `UniformList` tracks it internally).
    fn model_scroll_base(&self) -> gpui::ScrollHandle {
        self.model_scroll.0.borrow().base_handle.clone()
    }

    /// The scroll handle of whichever picker menu is mounted. The popups are
    /// mutually exclusive: the model list owns its virtualized handle, the
    /// plain-div menus (branch / project / device) share `menu_scroll`.
    /// Keys on the MOUNTED menu, not `open_kind` — popovers keep rendering
    /// through the exit animation, and the rail must keep measuring the
    /// closing menu's own handle, not `menu_scroll`'s idle geometry.
    fn active_menu_scroll(&self) -> gpui::ScrollHandle {
        if self.mounted_kind() == Some(PickerKind::HarnessModel) {
            self.model_scroll_base()
        } else {
            self.menu_scroll.clone()
        }
    }

    /// The list-hover half of the rail treatment; the strip's own hover,
    /// press, drag, and mouse-up listeners come from [`popover::rail`].
    pub(crate) fn on_menu_list_hover(
        &mut self,
        hovered: &bool,
        _window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        if self.menu_bar.set_list_hovered(*hovered) {
            cx.notify();
        }
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
    fn render_harness_model_popover(&mut self, cx: &mut Context<Self>) -> AnyElement {
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
    fn render_model_row(
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

    fn setting_groups(&self, cx: &App) -> Vec<SettingGroup> {
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

    fn cancel_setting_hover(&mut self) {
        self.setting_hover_task = None;
        self.setting_hover_pending = None;
        self.setting_hover_pointer = None;
    }

    fn hover_setting(
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

    fn open_setting(&mut self, id: ModelSetting, cx: &mut Context<Self>) {
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

    fn activate_setting_choice(&mut self, cx: &mut Context<Self>) {
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
    fn render_traits_sections(&mut self, cx: &mut Context<Self>) -> AnyElement {
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

/// The floating-scrollbar treatment for every picker list
/// ([`popover::rail`] folds the note/hide/metrics/render + pointer listeners
/// into one call): one shared rail state, fed by whichever handle
/// [`Pickers::active_menu_scroll`] resolves for the mounted menu.
impl popover::ScrollRailHost for Pickers {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        &mut self.menu_bar
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        Some(self.active_menu_scroll())
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

/// Flatten the picker's visible rows for one tab. The QUERY NEVER LEAVES THE
/// VIEWED TAB (user request; the old global search spanned every harness and
/// hid the rail): on a harness tab it ranks that harness's models only, on
/// the favorites tab it ranks the starred set. Without a query, a harness
/// tab lists its catalog stars-first and the favorites tab lists every star.
fn scoped_model_rows<'a>(
    query: &str,
    rail: ModelRail,
    effective: Option<HarnessId>,
    descriptors: &[HarnessDescriptor],
    models_for: impl Fn(HarnessId) -> Option<&'a [Model]>,
    is_favorite: impl Fn(HarnessId, &str) -> bool,
) -> Vec<ModelRowData> {
    let row = |descriptor: &HarnessDescriptor, model: &Model| ModelRowData {
        harness: descriptor.id,
        harness_name: SharedString::from(descriptor.name.clone()),
        model: model.clone(),
    };
    let in_scope = |descriptor: &HarnessDescriptor, model: &Model| match rail {
        ModelRail::Favorites => is_favorite(descriptor.id, &model.id),
        ModelRail::Harness => Some(descriptor.id) == effective,
    };
    if !query.is_empty() {
        // Rank: label prefix < label substring < description hit; stars,
        // then input order, break ties (t3 modelPickerSearch's field ladder
        // + favorite boost, collapsed to our ranks). The description stays
        // in the haystack — opencode's provider attribution ("anthropic")
        // must find its models even inside one tab.
        let mut ranked: Vec<(usize, usize, usize, ModelRowData)> = Vec::new();
        let mut input_ix = 0usize;
        for descriptor in descriptors {
            let Some(models) = models_for(descriptor.id) else {
                continue;
            };
            for model in models {
                if !in_scope(descriptor, model) {
                    continue;
                }
                let by_label = popover::match_rank(query, &model.label);
                let by_description = popover::match_rank(
                    query,
                    &format!(
                        "{} {}",
                        model.description.as_deref().unwrap_or(""),
                        model.label
                    ),
                )
                .map(|rank| rank + 2);
                if let Some(rank) = by_label.into_iter().chain(by_description).min() {
                    let starred = !is_favorite(descriptor.id, &model.id);
                    ranked.push((rank, starred as usize, input_ix, row(descriptor, model)));
                }
                input_ix += 1;
            }
        }
        ranked.sort_by_key(|(rank, unstarred, ix, _)| (*rank, *unstarred, *ix));
        return ranked.into_iter().map(|(_, _, _, row)| row).collect();
    }
    match rail {
        ModelRail::Favorites => {
            let mut rows = Vec::new();
            for descriptor in descriptors {
                let Some(models) = models_for(descriptor.id) else {
                    continue;
                };
                for model in models {
                    if is_favorite(descriptor.id, &model.id) {
                        rows.push(row(descriptor, model));
                    }
                }
            }
            rows
        }
        ModelRail::Harness => {
            let Some(descriptor) = descriptors.iter().find(|d| Some(d.id) == effective) else {
                return Vec::new();
            };
            let Some(models) = models_for(descriptor.id) else {
                return Vec::new();
            };
            let (starred, rest): (Vec<&Model>, Vec<&Model>) = models
                .iter()
                .partition(|m| is_favorite(descriptor.id, &m.id));
            starred
                .into_iter()
                .chain(rest)
                .map(|model| row(descriptor, model))
                .collect()
        }
    }
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

fn attach_overlay(
    chip: gpui::Stateful<gpui::Div>,
    overlay: &mut Option<(PickerKind, AnyElement)>,
    kind: PickerKind,
    id: &'static str,
    closing: Option<std::time::Instant>,
) -> gpui::Stateful<gpui::Div> {
    if overlay.as_ref().is_some_and(|(k, _)| *k == kind)
        && let Some((_, element)) = overlay.take()
    {
        return chip.child(popover::anchored_menu_above(id, element, closing));
    }
    chip
}

/// Attach the (single) open popover below a selector trigger.
fn attach_overlay_below(
    chip: gpui::Stateful<gpui::Div>,
    overlay: &mut Option<(PickerKind, AnyElement)>,
    kind: PickerKind,
    id: &'static str,
    closing: Option<std::time::Instant>,
) -> gpui::Stateful<gpui::Div> {
    if overlay.as_ref().is_some_and(|(k, _)| *k == kind)
        && let Some((_, element)) = overlay.take()
    {
        return chip.child(popover::anchored_menu_below(id, element, closing));
    }
    chip
}

/// Attach the menu ABOVE and RIGHT-ALIGNED to the trigger (t3code
/// `align="end"` — right-edge controls like the model picker open leftward).
fn attach_overlay_end(
    chip: gpui::Stateful<gpui::Div>,
    overlay: &mut Option<(PickerKind, AnyElement)>,
    kind: PickerKind,
    id: &'static str,
    closing: Option<std::time::Instant>,
) -> gpui::Stateful<gpui::Div> {
    if overlay.as_ref().is_some_and(|(k, _)| *k == kind)
        && let Some((_, element)) = overlay.take()
    {
        return chip
            .relative()
            .child(popover::anchored_menu_above_end(id, element, closing));
    }
    chip
}

impl Render for Pickers {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        // A ZERON_OPEN_PICKER popover never went through `toggle`, so claim
        // its keyboard focus here (re-claim until it sticks — the shell's
        // first-paint fallback focuses the composer after our first render).
        if self.boot_focus_pending {
            match self.open_kind() {
                Some(PickerKind::Branch) => {
                    self.search.update(cx, |input, cx| {
                        input.set_placeholder("Search refs…", cx);
                    });
                    let handle = self.search.read(cx).focus_handle(cx);
                    if handle.is_focused(window) {
                        self.boot_focus_pending = false;
                    } else {
                        window.focus(&handle, cx);
                    }
                }
                Some(_) => {
                    if self.focus.is_focused(window) {
                        self.boot_focus_pending = false;
                    } else {
                        window.focus(&self.focus, cx);
                    }
                }
                None => self.boot_focus_pending = false,
            }
        }

        let focus_on_mount = std::mem::take(&mut self.focus_on_mount) && self.is_open();
        if focus_on_mount {
            // The frame exists even when loading/error/empty content omits
            // the input. Claim it during mount, after any pre-frame shell
            // recovery has handled the old dispatch tree.
            window.focus(&self.focus, cx);
        }
        if self.is_open() {
            let search = self.search.focus_handle(cx);
            let frame = self.focus.clone();
            window.defer(cx, move |window, cx| {
                let has_search = frame.contains(&search, window);
                if focus_on_mount && frame.is_focused(window) && has_search {
                    // Transfer to the filter only once it is actually mounted.
                    window.focus(&search, cx);
                } else if search.is_focused(window) && !has_search {
                    // Loading, empty, and error states can omit the search box.
                    // Keep Escape/arrow keys on the mounted menu in those states.
                    window.focus(&frame, cx);
                }
            });
        }

        // Eager-load the harness catalog + every offered harness's models so
        // the chip reads "Fable 5" (a concrete pick) before any popover
        // opens, and rail switches inside the picker are instant.
        self.ensure_harnesses(false, cx);
        self.prefetch_models(false, cx);
        // A popover opened data-side (ZERON_OPEN_PICKER) never went through
        // `toggle`, so kick its loads here (all ensure_* are idempotent).
        if matches!(
            self.open_kind(),
            Some(PickerKind::Branch) | Some(PickerKind::Checkout)
        ) && matches!(self.refs, Loadable::Idle)
        {
            self.ensure_refs(false, cx);
        }
        // Chip shows the model's display name alone (zeron `modelText`); the
        // harness reads from the brand mark beside it. Never "Default model":
        // before the catalog lands the remembered label (or the configured id)
        // names the pick; the loaded list then resolves it to a concrete row.
        // No-agents state: nothing runnable resolved (and the catalog is
        // loaded, so that's a conclusion, not a loading gap) — the chip says
        // so instead of wearing a brand mark for an agent that can't run.
        let no_agents = self.no_agents_available() && self.effective_harness(cx).is_none();
        let model_label: SharedString = if no_agents {
            SharedString::from("No agents available")
        } else {
            let loaded = self.selected_model(cx).map(|m| m.label.clone());
            let label = loaded.or_else(|| {
                let remembered = self
                    .effective_harness(cx)
                    .and_then(|h| self.defaults.model_for(h));
                match self.effective_model_id(cx) {
                    Some(id) => Some(
                        remembered
                            .filter(|m| m.id == id)
                            .map(|m| m.label.clone())
                            .or_else(|| self.defaults.label_for(id).map(str::to_string))
                            .unwrap_or_else(|| id.to_string()),
                    ),
                    None => remembered.map(|m| m.label.clone()),
                }
            });
            label.map(SharedString::from).unwrap_or_default()
        };
        let catalog_loading = matches!(self.harnesses, Loadable::Idle | Loadable::Loading);
        let models_loading = self.effective_harness(cx).is_some_and(|harness| {
            !matches!(
                self.models.get(&harness),
                Some(Loadable::Ready(_)) | Some(Loadable::Error(_))
            )
        });
        // Harness unknown while the catalog resolves: the pixel-glyph loader
        // instead of guessing a brand mark.
        let chip_icon_loading =
            self.effective_harness(cx).is_none() && !no_agents && catalog_loading;
        // Harness known but nothing names the model yet (fresh install, no
        // remembered pick): a ghost label instead of a bare icon.
        let chip_label_loading =
            !no_agents && model_label.is_empty() && (catalog_loading || models_loading);
        let harness_icon: (&'static str, Option<gpui::Hsla>) = match self.effective_harness(cx) {
            Some(harness) => harness_brand_icon(harness),
            None if no_agents => (crate::icons::TERMINAL, Some(theme.text_muted)),
            None => (
                crate::icons::CLAUDE_MARK,
                Some(crate::icons::claude_brand()),
            ),
        };
        let explicit_options = self.explicit_options(cx);
        let traits_set = traits_summary(
            self.selected_model(cx),
            self.effective_reasoning(cx),
            &explicit_options,
        );
        let traits_active = traits_customized(
            self.selected_model(cx),
            self.effective_reasoning(cx),
            &self.trait_ladder(cx),
            &explicit_options,
        );
        // Render the open popover's body first (mutable borrow), then the
        // chips. Branch/Checkout render in the composer FOOTER row (see
        // `render_footer`), not here.
        let closing = self.open.closing_since();
        let mut overlay: Option<(PickerKind, AnyElement)> = match self.mounted_kind() {
            // Footer-row pickers — their popovers mount down there.
            Some(PickerKind::Branch)
            | Some(PickerKind::Checkout)
            | Some(PickerKind::Space)
            | Some(PickerKind::Device) => None,
            Some(PickerKind::HarnessModel) => {
                let content = self.render_harness_model_popover(cx);
                Some((
                    PickerKind::HarnessModel,
                    // Compact single-harness pane (t3 ModelPickerContent
                    // shrunk to its tabbed layout).
                    self.popover_frame_flush(304.0, content, cx),
                ))
            }
            None => None,
        };

        // The composer places this model chip beside the attachment button.
        // ONE chip for the whole run identity (user request): brand icon +
        // model name, then the joined traits summary ("Medium", "High · 1M ·
        // Fast", "Agent · Balance") as the chip's muted second tone — the
        // run's configuration reads without opening anything, and the suffix
        // brightens only when something departs from its default. No suffix
        // when the model has neither a ladder nor options (e.g. Hermes).
        let chip_suffix = traits_set.map(|summary| {
            (
                SharedString::from(summary),
                traits_active.then(|| theme.text.opacity(0.85)),
            )
        });
        let model_chip = self.trigger_chip(
            PickerKind::HarnessModel,
            model_label,
            true,
            Some(harness_icon),
            chip_icon_loading,
            chip_label_loading,
            chip_suffix,
            &theme,
            cx,
        );
        let new_chat = self.state.read(cx).selected_chat.is_none();
        let entity = cx.entity().downgrade();
        let model_chip = model_chip.relative().child(
            gpui::canvas(
                move |bounds, window, cx| {
                    let available = (f32::from(window.viewport_size().height - bounds.bottom())
                        - 14.0)
                        .max(0.0);
                    let _ = entity.update(cx, |this, cx| {
                        if this.model_space_below != Some(available) {
                            this.model_space_below = Some(available);
                            if new_chat && this.open_kind() == Some(PickerKind::HarnessModel) {
                                cx.notify();
                                window.request_animation_frame();
                            }
                        }
                    });
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0(),
        );
        let model_chip = if new_chat {
            if overlay
                .as_ref()
                .is_some_and(|(kind, _)| *kind == PickerKind::HarnessModel)
                && let Some((_, content)) = overlay.take()
            {
                model_chip.child(popover::anchored_menu_below_end(
                    "model-popover",
                    content,
                    closing,
                ))
            } else {
                model_chip
            }
        } else {
            attach_overlay_end(
                model_chip,
                &mut overlay,
                PickerKind::HarnessModel,
                "model-popover",
                closing,
            )
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            // Shrinkable under row pressure, like the footer chips: the chip's
            // own `min_w_0().truncate()` label/suffix only engage when this
            // cluster is allowed to give up width — `flex_none` here let the
            // labels paint over the attach/send buttons at narrow widths
            // instead of truncating (user report).
            .min_w_0()
            .gap(px(4.0))
            .child(model_chip)
    }
}


/// Catalog for the isolated native screenshot fixture; never used by the app.
#[cfg(feature = "project-palette-fixture")]
impl Pickers {
    pub(crate) fn fixture_model_catalog(&mut self, cx: &mut Context<Self>) {
        if matches!(self.models.get(&HarnessId::Codex), Some(Loadable::Ready(_))) {
            return;
        }
        self.config.harness = Some(HarnessId::Codex);
        self.config.model = Some("gpt-5.4".into());
        self.harnesses = Loadable::Ready(serde_json::from_value(serde_json::json!([
            {"id":"codex","name":"Codex","supportsSteering":true,"steeringMode":"step-boundary","reasoningLevels":[]}
        ])).unwrap());
        self.models.insert(HarnessId::Codex, Loadable::Ready(serde_json::from_value(serde_json::json!([
            {"id":"gpt-5.4","label":"GPT-5.4","description":"For complex coding and reasoning", "reasoningLevels":["low","medium","high","xhigh"], "options":[
                {"id":"context-window","label":"Context window","defaultChoice":"standard","choices":[{"id":"standard","label":"Standard"},{"id":"1m","label":"1M tokens"}]},
                {"id":"service-tier","label":"Service tier","defaultChoice":"auto","choices":[{"id":"auto","label":"Standard"},{"id":"fast","label":"Fast"}]}
            ]},
            {"id":"gpt-5.3-codex","label":"GPT-5.3 Codex","description":"Optimized for agentic coding"},
            {"id":"gpt-5.2","label":"GPT-5.2","description":"General purpose reasoning"},
            {"id":"gpt-5.1-codex-mini","label":"GPT-5.1 Codex Mini","description":"Fast, efficient coding"}
        ])).unwrap()));
        self.catalog_rev += 1;
        cx.notify();
    }
}
