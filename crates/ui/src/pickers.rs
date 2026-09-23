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

use gpui::{
    App, Context, Entity, FocusHandle, Focusable as _, Subscription, Task, Window, prelude::*,
};

use skylark_engine::registry::HarnessDescriptor;
use skylark_proto::RepoRef;

use crate::composer::{ComposerInput, ComposerInputEvent};
use crate::popover::{self, Loadable};
use crate::settings::composer::ComposerDefaults;
use crate::state::{AppState, EngineHandle};

pub mod browser;
pub mod config;
pub mod display;
pub mod git_picker;
pub mod keyboard;
pub mod loader;
pub mod model_catalog;
pub mod model_menu;
pub mod selections;
pub mod space_picker;
#[cfg(test)]
mod tests;

pub use browser::*;
pub use config::*;
pub use git_picker::*;
pub use model_catalog::*;
#[cfg(test)]
pub(crate) use display::workspace_footer_row;

pub struct Pickers {
    pub(crate) state: Entity<AppState>,
    pub(crate) config: DraftConfig,
    /// Sticky last-used picks (skylark `skylark.composer.defaults:v1`): seeds the
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
    /// `SKYLARK_OPEN_PICKER` boot: keep claiming focus until it sticks, so
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
        // Dev/testing knob: `SKYLARK_OPEN_PICKER=model|traits|repo|branch` boots
        // with that popover open — synthetic input can't reach the app on
        // headless compositors, so captures need a data-side path.
        let boot_open = match std::env::var("SKYLARK_OPEN_PICKER").ok().as_deref() {
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
    pub(crate) fn save_defaults(&self) {
        if let Some(dir) = self.data_dir.as_deref()
            && let Err(err) = self.defaults.save(dir)
        {
            tracing::warn!(error = %err, "composer-defaults save failed");
        }
    }

    pub(crate) fn engine(&self, cx: &App) -> Option<EngineHandle> {
        self.state.read(cx).engine().cloned()
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
    pub(crate) fn mounted_kind(&self) -> Option<PickerKind> {
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

    /// Capture knob (`SKYLARK_OPEN_DIALOG=model`): open the combined
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

    pub(crate) fn toggle(&mut self, kind: PickerKind, window: &mut Window, cx: &mut Context<Self>) {
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
}
