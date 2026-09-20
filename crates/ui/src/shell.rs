//! The app shell (zeron `__root.tsx`): sidebar column + main panel + optional
//! right "Changes" pane, plus the boot splash and the connection gate.
//!
//! Layout is zeron's: collapsible drag-resizable sidebar (224–400px, default
//! 256) with a 200ms ease-out width transition; main panel with an h-11 header,
//! content outlet, and a reserved h-6 status strip so later content never
//! shifts; right pane scaffold (360px floor, default 520), hidden by default.
//! Widths/collapsed state persist to `ui-settings.json` (debounced).
//!
//! Resize handles use gpui's drag-and-drop pattern (an `on_drag` with an empty
//! ghost view + `on_drag_move::<Marker>` on the root), the same idiom as Zed's
//! dock. Double-clicking a handle resets that pane to its default width.

use std::path::PathBuf;
use std::time::Duration;

use chrono::Utc;
use gpui::{
    Action, AnyElement, App, Context, Empty, Entity, FocusHandle, Focusable as _,
    IntoElement, KeyBinding, Keystroke, MouseButton, MouseDownEvent,
    Pixels, Point, Render, SharedString, Subscription, Task, Window,
    actions, div, prelude::*, px,
};

use gpui_tokio::Tokio;
use zeron_proto::{AuthState, WorkspaceScope};
use zeron_rpc::methods;

use crate::changes::Changes;
use crate::composer::{Composer, ComposerEvent, ComposerInput, ComposerInputEvent};
use crate::files::{FilesSurface, WorkspacePathDrag};
use crate::icons::{self, icon};
use crate::loaders;
use crate::motion::{self, AnimationExt as _, MotionSpec, RESIZE, SPLASH_OUT, TAB_SLIDE};
use crate::popover::{self, Loadable};
use crate::rail;
use crate::settings::accounts::AccountsPage;
use crate::settings::appearance::{AppearancePage, AppearanceSettingsEvent};
use crate::settings::archived::ArchivedPage;
use crate::settings::devices::DevicesPage;
use crate::settings::files::{FilesSettingsEvent, FilesSettingsPage};
use crate::settings::harnesses::HarnessesPage;
use crate::settings::notifications::{NotificationsEvent, NotificationsPage};
use crate::settings::shortcuts::{ShortcutsEvent, ShortcutsPage};
use crate::settings::{
    self, CHAT_PANEL_MIN, ComposerSendBehavior, JUMP_SLOTS, KeymapConfig, RIGHT_PANE_DEFAULT,
    RIGHT_PANE_MIN, SIDEBAR_DEFAULT, SIDEBAR_MAX, SIDEBAR_MIN, ShortcutId,
    SidebarOrganization, SidebarSort, TERMINAL_DEFAULT_HEIGHT, TERMINAL_MAX_VH,
    TERMINAL_MIN_HEIGHT, UiSettings, badge_combo, jump_hints_visible, modifier_send_hint_visible,
    platform_combo, sidebar_pin_profile_key,
};
use crate::state::{
    AppState, ConnectionStatus, EngineBootConfig, GatePhase, Indicator, OrgRow,
    format_time_ago, org_name_valid, parse_orgs, sort_memberships,
};
use crate::terminal::panel::{TerminalPanel, ToggleTerminal};
use crate::theme::Theme;
use crate::transcript::{self, Transcript};

mod actions_ui;
mod command_palette;
mod fixtures;
mod layout;
mod navigation;
mod notifications;
mod org_gate;
mod project_icon;
mod right_tabs;
mod settings_modal;
mod sidebar_mutations;
mod sidebar_pins;
mod sidebar_sessions;
mod spaces;
mod surfaces;
mod sync_switch;
mod tabs;
mod terminal_container;
mod titlebar;
pub use surfaces::*;
pub use titlebar::*;
pub(crate) use layout::*;
pub(crate) use sync_switch::*;
pub(super) use org_gate::grid_backdrop;
pub(super) use right_tabs::workspace_file_title;
pub(super) use sidebar_mutations::{ChatMenuPage, ChatMenuState, RenameChatDialog};
pub(super) use terminal_container::TERMINAL_RESIZE_HITBOX_HEIGHT;
use org_gate::OrgGateUi;
use right_tabs::{RightTabDrag, RightTabDragState};

pub use navigation::{NavEntry, NavHistory};
pub use sidebar_sessions::resort_offsets;
pub(crate) use sidebar_sessions::{
    sidebar_faded_label, sidebar_row_height, SIDEBAR_DRAG_SCROLL_BAND,
    SIDEBAR_DRAG_SCROLL_FRAME_MS, SIDEBAR_DRAG_SCROLL_MAX, SIDEBAR_LIST_GAP, SIDEBAR_LIST_PAD_TOP,
};
#[cfg(test)]
pub(crate) use sidebar_sessions::SIDEBAR_SESSION_SLOT;

use spaces::{AddSpaceFlow, RenameSpaceDialog};

actions!(
    shell,
    [
        SaveFile,
        ToggleSidebar,
        ToggleChanges,
        AddSpacePalette,
        ToggleCommandPalette,
        OpenModelPicker,
        NewSession,
        OpenSettings,
        NextSession,
        PrevSession,
        ArchiveSession
    ]
);

/// Restore a default focus only after an in-flight handoff has had a frame to
/// claim the window. A synchronous focus-lost fallback can otherwise steal
/// focus from controls that are mounting in response to the same input event.
pub(crate) fn restore_focus_if_empty_on_next_frame<T: 'static>(
    focus: FocusHandle,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    window.on_next_frame(move |window, cx| {
        if window.focused(cx).is_none() {
            window.focus(&focus, cx);
        }
    });
    cx.notify();
}

/// Check the completed dispatch tree, not just the lifetime of the focused
/// handle: a hidden editor can stay alive after its element has unmounted.
pub(crate) fn restore_mounted_focus(
    root: &FocusHandle,
    preferred: &FocusHandle,
    unfocused: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) {
    let preferred_mounted = root.contains(preferred, window);
    if !root.contains_focused(window, cx) || (root.is_focused(window) && preferred_mounted) {
        // Explicit blur keeps shortcuts active without returning the caret to
        // an input. The root remains the temporary fallback for stale handles.
        let target = if window.focused(cx).is_none() {
            unfocused
        } else if preferred_mounted {
            preferred
        } else {
            root
        };
        window.focus(target, cx);
    }
}


/// Interruptible height tween for the sidebar's device/archive disclosures.
/// The rendered element owns the frame clock; this state preserves the current
/// interpolated height when a second click reverses an in-flight transition.
#[derive(Clone, Copy)]
pub(super) struct SidebarDisclosureMotion {
    pub(super) epoch: u64,
    pub(super) from: f32,
    pub(super) to: f32,
    started: std::time::Instant,
}

impl SidebarDisclosureMotion {
    fn new(epoch: u64, from: f32, to: f32) -> Self {
        Self {
            epoch,
            from,
            to,
            started: std::time::Instant::now(),
        }
    }

    fn current(self) -> f32 {
        let total = motion::COLLAPSE.total().as_secs_f32();
        let raw = if total > 0.0 {
            self.started.elapsed().as_secs_f32() / total
        } else {
            1.0
        };
        motion::lerp(self.from, self.to, motion::COLLAPSE.progress(raw))
    }

    fn animating(self) -> bool {
        self.started.elapsed() < motion::COLLAPSE.total() + spaces::SIDEBAR_DISCLOSURE_TWEEN_GRACE
    }
}



/// Open the session at `slot` (zero-based) of the sidebar's active list. One
/// action carrying the slot, rather than nine near-identical action types.
#[derive(Clone, PartialEq, Action)]
#[action(namespace = shell, no_json)]
pub struct JumpSession(pub usize);

/// (Re-)apply the whole app keymap: clears every binding, restores the composer
/// map, then binds the customizable shortcuts from `keymap` (feature-inventory
/// §1.4). Invalid persisted combos fall back to that shortcut's default.
pub fn apply_keymap(
    cx: &mut App,
    keymap: &KeymapConfig,
    composer_send_behavior: ComposerSendBehavior,
) {
    fn valid_or_default(combo: &str, fallback: &str) -> String {
        let candidate = platform_combo(combo);
        if Keystroke::parse(&candidate).is_ok() {
            candidate
        } else {
            tracing::warn!(%combo, "unparseable shortcut combo; using default");
            platform_combo(fallback)
        }
    }
    crate::appshots::set_shortcut(&keymap.capture_appshot);
    cx.clear_key_bindings();
    // `clear_key_bindings` also removes the contextual editing actions that
    // gpui-base installed at startup. Reinitialize the component layer before
    // rebuilding Zeron's bindings so the file editor keymap remains active.
    gpui_base::init(cx);
    crate::composer::init(cx, composer_send_behavior);
    // Fixed app-level shortcuts (Settings on every platform; ⌘Q quit, ⌘W
    // close, ⌘M minimize, ⌘H hide on macOS) — these back the native menu
    // key equivalents and must survive keymap re-application.
    crate::app_menus::bind_keys(cx);
    cx.bind_keys([
        KeyBinding::new(
            &valid_or_default(&keymap.save_file, "mod-s"),
            SaveFile,
            None,
        ),
        KeyBinding::new(
            &valid_or_default(&keymap.toggle_sidebar, "mod-b"),
            ToggleSidebar,
            None,
        ),
        KeyBinding::new(
            &valid_or_default(&keymap.toggle_changes, "mod-r"),
            ToggleChanges,
            None,
        ),
        KeyBinding::new(
            &valid_or_default(&keymap.toggle_terminal, "mod-j"),
            ToggleTerminal,
            None,
        ),
        KeyBinding::new(
            &valid_or_default(&keymap.new_session, "mod-n"),
            NewSession,
            None,
        ),
        KeyBinding::new(
            &valid_or_default(
                &keymap.next_session,
                crate::settings::ShortcutId::NextSession.default_combo(),
            ),
            NextSession,
            None,
        ),
        KeyBinding::new(
            &valid_or_default(
                &keymap.prev_session,
                crate::settings::ShortcutId::PrevSession.default_combo(),
            ),
            PrevSession,
            None,
        ),
        KeyBinding::new(
            &valid_or_default(&keymap.archive_session, "mod-shift-a"),
            ArchiveSession,
            None,
        ),
        KeyBinding::new(
            &valid_or_default(&keymap.new_project, ShortcutId::NewProject.default_combo()),
            AddSpacePalette,
            None,
        ),
        // Fixed: ⌘K summons the command palette.
        // Pressing it again dismisses.
        KeyBinding::new(&platform_combo("mod-k"), ToggleCommandPalette, None),
        KeyBinding::new(
            &valid_or_default(&keymap.open_model_picker, "mod-/"),
            OpenModelPicker,
            None,
        ),
    ]);
    crate::browser::bind_keys(cx, keymap);
    // ⌘1..⌘9 open the sidebar's first nine rows. A slot left unbound (an empty
    // combo in a hand-edited file) binds nothing rather than falling back —
    // the user cleared it on purpose.
    cx.bind_keys((0..JUMP_SLOTS).filter_map(|slot| {
        let id = ShortcutId::JumpSession(slot);
        let combo = keymap.get(id);
        if combo.is_empty() {
            return None;
        }
        Some(KeyBinding::new(
            &valid_or_default(combo, id.default_combo()),
            JumpSession(slot),
            None,
        ))
    }));
}

/// The settings sections (feature-inventory §1.5 routes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsSection {
    Devices,
    /// Which harnesses the composer offers (enable/disable toggles).
    Harnesses,
    /// Per-provider CLI accounts (login, usage) — labeled "Accounts".
    Agents,
    Appearance,
    Files,
    Notifications,
    Shortcuts,
    Appshots,
    Archived,
}

impl SettingsSection {
    pub const ALL: [SettingsSection; 9] = [
        SettingsSection::Devices,
        SettingsSection::Harnesses,
        SettingsSection::Agents,
        SettingsSection::Appearance,
        SettingsSection::Files,
        SettingsSection::Notifications,
        SettingsSection::Shortcuts,
        SettingsSection::Appshots,
        SettingsSection::Archived,
    ];

    /// Sidebar + header label (zeron settings-sidebar.tsx SECTIONS / __root.tsx
    /// `settingsTitle` — the same strings in both places).
    pub fn label(self) -> &'static str {
        match self {
            SettingsSection::Devices => "Devices",
            SettingsSection::Harnesses => "Agents",
            SettingsSection::Agents => "Accounts",
            SettingsSection::Appearance => "Appearance",
            SettingsSection::Files => "Files",
            SettingsSection::Notifications => "Notifications",
            SettingsSection::Shortcuts => "Shortcuts",
            SettingsSection::Appshots => "Appshots",
            SettingsSection::Archived => "Archived sessions",
        }
    }
}

/// What the main outlet shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Chat,
    Settings(SettingsSection),
}





/// Per-chat panel open flags (zeron parity: `sessionPanels` — the terminal and
/// changes panels open *per session*, in memory only; heights and every other
/// persisted setting stay global).
///
/// Everything defaults CLOSED — the right pane included (user request,
/// revising the earlier default-open: it popped open on every session you
/// visited). Opening is an explicit act, remembered per chat for the rest of
/// the app run; a fresh open with no surface tabs lands on the picker.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ChatPanels {
    pub terminal_open: bool,
    /// Right pane visible (the surface host — historically the Changes pane).
    pub changes_open: bool,
    /// Which surface tab renders; validated against the live tab list each
    /// frame (a closed tab falls back gracefully).
    pub right_active: RightSurface,
}

/// The session-scoped panel map. Keys are chat ids; the new-chat canvas uses
/// the empty key. Not persisted — a fresh app starts with everything closed.
#[derive(Debug, Default)]
pub struct SessionPanels {
    map: std::collections::HashMap<String, ChatPanels>,
}

impl SessionPanels {
    pub fn get(&self, key: &str) -> ChatPanels {
        self.map.get(key).copied().unwrap_or_default()
    }

    /// Flip the terminal flag for `key`; returns the new value.
    pub fn toggle_terminal(&mut self, key: &str) -> bool {
        let entry = self.map.entry(key.to_string()).or_default();
        entry.terminal_open = !entry.terminal_open;
        entry.terminal_open
    }

    /// Flip the changes flag for `key`; returns the new value.
    pub fn toggle_changes(&mut self, key: &str) -> bool {
        let entry = self.map.entry(key.to_string()).or_default();
        entry.changes_open = !entry.changes_open;
        entry.changes_open
    }

    /// Mutate `key`'s flags in place (right-pane surface bookkeeping).
    pub fn update(&mut self, key: &str, f: impl FnOnce(&mut ChatPanels)) {
        f(self.map.entry(key.to_string()).or_default());
    }
}

/// Sidebar resort glide (feature-inventory §1.6): 260ms
/// `cubic-bezier(0.22,1,0.36,1)` per-row translate, the View Transitions
/// equivalent.
pub const RESORT: MotionSpec = MotionSpec::new(260, motion::EASE_RESORT);

/// Ramp height of the sidebar's scroll-edge fade (the gpui
/// [`gpui::EdgeFade`] scope — per-primitive, so text fades per glyph).
const SIDEBAR_GLASS_FADE_BAND: f32 = 24.0;

/// New-thread controls float over the tail of a top-anchored image hero. The
/// hero reaches below the composer, giving its lower mask room to dissolve
/// gradually into the otherwise empty lower canvas.
const NEW_THREAD_BACKGROUND_FROSTED_OPACITY: f32 = 0.84;
const NEW_THREAD_BACKGROUND_VIEWPORT_RATIO: f32 = 0.72;
const NEW_THREAD_BACKGROUND_MAX_HEIGHT: f32 = 760.0;



/// Sidebar-only drag payload. Regular sessions never acquire a manual order.
#[derive(Clone)]
struct SidebarSessionDrag {
    chat_id: String,
    visible_ids: std::sync::Arc<Vec<String>>,
    filter: Option<String>,
    profile_key: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SidebarSessionDrop {
    Pinned(usize),
    Regular,
}

struct SidebarSessionTransfer {
    payload: SidebarSessionDrag,
    origin: std::rc::Rc<std::cell::Cell<Point<Pixels>>>,
    cursor_offset: Point<Pixels>,
    pointer: Point<Pixels>,
    viewport: Option<gpui::Bounds<Pixels>>,
    slide: SidebarSessionSlide,
    preview: Option<SidebarSessionGap>,
    source_group: String,
    source_index: usize,
    row_height: f32,
    source_collapse: SidebarSessionSlide,
    collapsed_height: f32,
    section_gaps: std::collections::HashMap<String, SidebarSessionSlide>,
    siblings: std::collections::HashMap<String, SidebarSessionSlide>,
}

#[derive(Clone)]
struct SidebarSessionGap {
    group: String,
    index: usize,
    pinned: bool,
    top: f32,
}

/// The same slot-to-slot easing as pinned reordering, applied to the actual row.
struct SidebarSessionSlide {
    from: f32,
    to: f32,
    epoch: u64,
    started: std::time::Instant,
}

impl SidebarSessionSlide {
    fn current(&self) -> f32 {
        let progress = TAB_SLIDE
            .progress(self.started.elapsed().as_secs_f32() / TAB_SLIDE.total().as_secs_f32());
        motion::lerp(self.from, self.to, progress)
    }

    fn retarget(&mut self, target: f32) {
        if (target - self.to).abs() < 0.5 {
            return;
        }
        self.from = self.current();
        self.to = target;
        self.epoch = self.epoch.wrapping_add(1);
        self.started = std::time::Instant::now();
    }
}

struct SidebarSessionReturn {
    transfer: SidebarSessionTransfer,
    epoch: u64,
    started: std::time::Instant,
}

/// Live destination for a pinned-session drag. The real row remains clipped
/// to the sidebar and slides between slots with its pinned siblings.
struct PinnedSessionDragState {
    chat_id: String,
    visible_ids: std::sync::Arc<Vec<String>>,
    from: usize,
    over: usize,
    prev_over: usize,
    epoch: usize,
    filter: Option<String>,
    profile_key: String,
    pointer_y: Option<f32>,
    viewport_top: f32,
    viewport_bottom: f32,
    generation: u64,
    autoscroll_active: bool,
}

type SidebarKeyedRow = (String, f32, AnyElement);

struct SidebarSessionRows {
    regular_count: usize,
    rows: Vec<SidebarKeyedRow>,
    pinned_count: usize,
    moving_row: Option<(AnyElement, f32)>,
}

/// Invisible drag ghost — resize drags and contained pinned-session reorders
/// render nothing at the cursor.
pub(crate) struct DragGhost;

impl Render for DragGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

fn bottom_stack_measurement_matches(
    measured_has_composer: bool,
    expected_has_composer: bool,
) -> bool {
    measured_has_composer == expected_has_composer
}

fn new_thread_background_opacity(is_frost: bool) -> f32 {
    if is_frost {
        NEW_THREAD_BACKGROUND_FROSTED_OPACITY
    } else {
        1.0
    }
}

fn new_thread_background_height(viewport_height: f32) -> f32 {
    (viewport_height.max(0.0) * NEW_THREAD_BACKGROUND_VIEWPORT_RATIO)
        .min(NEW_THREAD_BACKGROUND_MAX_HEIGHT)
}

fn new_thread_background(
    artwork: Option<std::sync::Arc<gpui::RenderImage>>,
    viewport_height: f32,
    hero_width: f32,
    composer_bounds: crate::new_thread_background_mask::SurfaceBounds,
    dissolve: f32,
    opacity: f32,
) -> AnyElement {
    let Some(artwork) = artwork else {
        return Empty.into_any_element();
    };
    let hero_height = new_thread_background_height(viewport_height);
    let dissolve = dissolve.clamp(0.0, 1.0);
    // Image and treatment share a fixed crop and fade together in place.
    // The hero uses the full conversation canvas even while the destination
    // right pane clips it. Navigation must never rescale the artwork.
    div()
        .absolute()
        .top_0()
        .left_0()
        .w(px(hero_width))
        .h(px(hero_height))
        .overflow_hidden()
        .opacity((1.0 - dissolve) * opacity)
        // Alpha resolves into the real canvas, including translucent themes;
        // no theme-colored overlay bleaches or darkens the source pixels.
        .children([false, true].into_iter().map(|cutout| {
            let artwork = artwork.clone();
            let composer_bounds = composer_bounds.clone();
            div()
                .absolute()
                .inset_0()
                .opacity(if cutout {
                    1.0
                } else {
                    crate::new_thread_background_mask::CUTOUT_REVEAL_OPACITY
                })
                .child(
                    gpui::canvas(
                        |_, _, _| {},
                        move |bounds, _, window, _cx| {
                            if let Some(composer) = composer_bounds.get() {
                                crate::new_thread_background_mask::paint(
                                    artwork.clone(),
                                    bounds,
                                    composer,
                                    cutout,
                                    window,
                                );
                            }
                        },
                    )
                    .absolute()
                    .inset_0(),
                )
        }))
        .into_any_element()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SplashPhase {
    Visible,
    FadingOut,
    Gone,
}


/// In-app update lifecycle (macOS bundle installs; see `render_update_strip`).
enum UpdateFlow {
    Idle,
    Downloading,
    /// Staged bundle ready to swap in — one click restarts into it.
    Ready(PathBuf),
    Failed(SharedString),
}



/// Sidebar render identity lets transcript/caret frames reuse its GPUI scene.
/// State and event handlers stay on Shell. Explicit Shell notifications still
/// invalidate the sidebar, including selection, menus, theme and navigation.
struct SidebarPane {
    shell: gpui::WeakEntity<Shell>,
    _observation: Subscription,
}

impl Render for SidebarPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::transcript::record_view_frame("sidebar");
        let Some(shell) = self.shell.upgrade() else {
            return div().into_any_element();
        };
        let inner = shell.update(cx, |shell, cx| {
            let theme = Theme::of(cx).clone();
            match shell.route {
                Route::Settings(section) => shell.render_settings_nav(section, &theme, cx),
                Route::Chat => shell.render_chat_sidebar(&theme, cx),
            }
        });
        div().size_full().child(inner).into_any_element()
    }
}

#[derive(Debug, Clone)]
pub(super) enum PendingExit {
    CloseWindow,
    Quit,
    RuntimeChange,
    InstallUpdate(PathBuf),
}

pub struct Shell {
    pub(super) state: Entity<AppState>,
    sidebar_pane: Entity<SidebarPane>,
    transcript: Entity<Transcript>,
    pub(super) composer: Entity<Composer>,
    /// Measured height of the bottom chrome stack (status strip + composer +
    /// terminal dock) the full-height transcript scrolls under. Paint-time
    /// measurement schedules another frame whenever this value changes.
    bottom_stack: std::rc::Rc<std::cell::Cell<f32>>,
    /// Whether `bottom_stack` was measured with the session composer present.
    /// A newly selected transcript stays hidden until this matches its route,
    /// preventing one frame at the blank canvas's stale bottom clearance.
    bottom_stack_has_composer: std::rc::Rc<std::cell::Cell<bool>>,
    /// Shared route clock and measured prepaint geometry for the persistent composer.
    composer_dock: crate::composer_dock::SharedDock,
    new_thread_artwork_ready: crate::new_thread_background_effects::Readiness,
    /// Session-transient disclosure state, matching the Archived shelf.
    pub(super) pinned_open: bool,
    pub(super) sessions_open: bool,
    /// The sidebar's archived accordion (t3code Sidebar): OPEN by default
    /// (user request), session-transient. `archived_shown` pages the
    /// expanded list ("Show more" reveals another page).
    pub(super) archived_open: bool,
    pub(super) archived_shown: usize,
    /// Ephemeral collapsed project/device sections, keyed by organization + id.
    pub(super) sidebar_collapsed_groups: std::collections::HashSet<String>,
    /// In-flight disclosure tweens, shared by device groups, Pinned and Archived.
    pub(super) sidebar_disclosure_motion:
        std::collections::HashMap<String, SidebarDisclosureMotion>,
    /// The jump-hint overlay: true while the held modifiers exactly match a
    /// jump shortcut, which swaps the first nine rows' time-ago for their
    /// key-cap chip (t3code's `showJumpHints`). Frame-transient — window
    /// deactivation clears it, so a chip cannot stick after an app switch
    /// swallows the key-up.
    pub(super) jump_hints: bool,
    /// Lazy panes: no entity (and no RPC) until first opened.
    terminal: Option<Entity<TerminalPanel>>,
    /// Embedded terminal host for right-pane Terminal surfaces — a SEPARATE
    /// entity from the bottom drawer's (own PTYs, own grid geometry; one
    /// panel can only size one visible grid at a time).
    right_terminal: Option<Entity<TerminalPanel>>,
    /// The surface-tab strip's `+` menu (Files / Terminal / Diffs / History rows).
    right_plus: popover::Popup<()>,
    /// Host-owned project Actions cached per (device, space).
    project_actions: crate::project_actions::ProjectActionsController,
    /// Diff surfaces by id — each tab its own [`Changes`] viewer with its own
    /// scope/base pick and diff watch (multiple diff panels, user request).
    pub(super) diffs: std::collections::HashMap<u64, Entity<Changes>>,
    /// One workspace browser per chat/panel key. Dropping the entity closes
    /// its file watcher and every in-flight workspace request.
    pub(super) files: std::collections::HashMap<String, Entity<FilesSurface>>,
    pub(super) files_subs: std::collections::HashMap<String, Subscription>,
    /// One independent editor/tree per opened workspace file. IDs are global
    /// while the lookup key keeps a file tab scoped to its chat panel.
    pub(super) file_surfaces: std::collections::HashMap<u64, Entity<FilesSurface>>,
    pub(super) file_surface_paths: std::collections::HashMap<u64, String>,
    pub(super) file_surface_keys: std::collections::HashMap<(String, String), u64>,
    pub(super) file_surface_subs: std::collections::HashMap<u64, Subscription>,
    pub(super) file_surface_seq: u64,
    pub(super) pending_file_closes: std::collections::HashSet<RightSurface>,
    pub(super) pending_exit: Option<PendingExit>,
    /// Event hookups for [`Self::diffs`] (History rows opening commit tabs).
    pub(super) diff_subs: std::collections::HashMap<u64, Subscription>,
    pub(super) diff_seq: u64,
    /// Subagent transcript surfaces by id — each tab a read-only
    /// [`Transcript`] pinned to its subagent doc.
    pub(super) subagent_tabs: std::collections::HashMap<u64, SubagentTab>,
    pub(super) subagent_seq: u64,
    pub(super) browsers: std::collections::HashMap<u64, Entity<crate::browser::BrowserSurface>>,
    pub(super) browser_subs: std::collections::HashMap<u64, Subscription>,
    pub(super) browser_seq: u64,
    pub(super) browser_context: crate::browser::BrowserContext,
    browser_profile: Option<String>,
    /// Ordered surface tabs per panel key (drag-reorderable; stale entries —
    /// closed terminals/diffs — are skipped at read time).
    pub(super) right_tabs: std::collections::HashMap<String, Vec<RightSurface>>,
    /// In-flight surface-tab drag (slide animation state).
    right_tab_drag: Option<RightTabDragState>,
    /// Surface-tab strip scroll (the strip overflows horizontally, t3
    /// ScrollArea-style; drag drop-math reads the offset back out).
    right_tab_scroll: gpui::ScrollHandle,
    /// Chat outlet vs settings pages.
    route: Route,
    /// Route history behind the titlebar back/forward buttons (§ nav history).
    nav: NavHistory,
    devices_page: Option<Entity<DevicesPage>>,
    archived_page: Option<Entity<ArchivedPage>>,
    appearance_page: Option<Entity<AppearancePage>>,
    files_settings_page: Option<Entity<FilesSettingsPage>>,
    notifications_page: Option<Entity<NotificationsPage>>,
    shortcuts_page: Option<Entity<ShortcutsPage>>,
    accounts_page: Option<Entity<AccountsPage>>,
    harnesses_page: Option<Entity<HarnessesPage>>,
    shortcuts_sub: Option<Subscription>,
    notifications_sub: Option<Subscription>,
    files_settings_sub: Option<Subscription>,
    appearance_settings_sub: Option<Subscription>,
    /// Session-row context menu, including the Copy submenu.
    chat_menu: popover::Popup<ChatMenuState>,
    rename_dialog: Option<RenameChatDialog>,
    /// Chat id awaiting delete confirmation.
    delete_confirm: Option<String>,
    /// Space-row context menu (dropdown rows): (space id, window position).
    space_menu: popover::Popup<(String, Point<Pixels>)>,
    rename_space_dialog: Option<RenameSpaceDialog>,
    /// Space id awaiting delete confirmation (hard delete + session cascade).
    delete_space_confirm: Option<String>,
    /// The add-space palette (device tabs + folder search), `Some`
    /// while open.
    add_space: Option<AddSpaceFlow>,
    command_palette: Option<command_palette::CommandPalette>,
    /// The sidebar's space-filter dropdown.
    spaces_menu: popover::Popup<spaces::SpacesMenu>,
    /// Hover/drag + scroll-linger state of the dropdown's floating rail.
    spaces_menu_bar: popover::MenuScrollbarState,
    /// Persisted organization/sort/metadata controls beside the project filter.
    sidebar_view_menu: popover::Popup<spaces::SidebarViewMenu>,
    /// Natural-tab-order focus target for the icon-only view-options button.
    sidebar_view_trigger_focus: gpui::FocusHandle,
    /// Current pinned-row metrics shared by hit testing and displacement animations.
    sidebar_pinned_heights: Vec<f32>,
    project_icons:
        std::cell::RefCell<std::collections::HashMap<String, Entity<project_icon::ProjectIcon>>>,
    /// Hovered row whose status is replaced by the archive control.
    chat_status_hover: Option<String>,
    /// Scroll position of the sidebar lists region (drives its edge fades).
    sidebar_scroll: gpui::ScrollHandle,
    /// In-flight reorder for the pinned section only.
    pinned_session_drag: Option<PinnedSessionDragState>,
    pinned_session_drag_generation: u64,
    sidebar_session_transfer: Option<SidebarSessionTransfer>,
    sidebar_session_return: Option<SidebarSessionReturn>,
    /// Pending pin intents are scoped to the active profile and engine attachment.
    sidebar_pin_write: Option<sidebar_pins::PendingSidebarPins>,
    sidebar_pin_write_generation: u64,
    sidebar_pin_write_notice: Option<SharedString>,
    /// `settings.last_space_id` applied once after the first spaces frame.
    space_boot_applied: bool,
    /// Last seen session status per chat — the chime trigger compares against
    /// it (a row's FIRST appearance never chimes, so boot stays silent).
    sound_prev: std::collections::HashMap<String, crate::sound::SessionNotificationState>,
    /// Startup-aware durable connectivity notification baseline.
    connectivity_notifications: crate::sound::ConnectivityNotificationState,
    /// Persistent across AppState observer callbacks so simultaneous session
    /// failures and connectivity degradation produce one attention sound.
    attention_sound_gate: crate::sound::AttentionSoundGate,
    user_menu: popover::Popup<()>,
    /// Inline sidebar error strip (mutation failures); click dismisses.
    sidebar_notice: Option<SharedString>,
    /// Local lifecycle of an in-app update (macOS bundle swap) — the engine's
    /// UpdateStatus stream says WHETHER one exists; this says how far the
    /// download/stage of it has come in this process.
    update_flow: UpdateFlow,
    update_task: Option<Task<()>>,
    /// Version whose update strip the user dismissed (advisory installs only —
    /// a newer release shows the strip again).
    update_dismissed: Option<String>,
    /// How this binary was installed — decides the strip's click behavior.
    /// Cached: `detect_install` stats `current_exe` and this renders per frame.
    install: zeron_update::InstallKind,
    org: Option<OrgGateUi>,
    sync_flow: SyncFlow,
    mutate_task: Option<Task<()>>,
    auth_task: Option<Task<()>>,
    runtime_change_task: Option<Task<()>>,
    runtime_change_error: Option<SharedString>,
    /// The one-time local→synced import stream (switch wizard progress step).
    import_task: Option<Task<()>>,
    /// Title of the chat the import stream is copying right now.
    import_current: Option<SharedString>,
    /// Kept for the failed-gate "Retry" action.
    boot: EngineBootConfig,
    data_dir: PathBuf,
    pub(super) settings: UiSettings,
    /// Session-scoped panel open flags (terminal / changes per chat; §1.10-1.11
    /// parity — heights stay in [`UiSettings`]).
    pub(super) panels: SessionPanels,
    /// The panel key of the chat currently shown ("" = new-chat canvas).
    active_chat: String,
    /// Last selected session survives opening the blank Appshot destination.
    last_appshot_chat: Option<String>,
    /// Last rendered sidebar order (key + estimated height) — the FLIP baseline
    /// for the §1.6 resort glide.
    sidebar_prev_order: Vec<(String, f32)>,
    /// Per-key paint offsets of the resort in flight, keyed elements restart on
    /// `resort_epoch` bumps.
    sidebar_resort: std::collections::HashMap<String, f32>,
    /// Keys that just appeared in a live list (fade in, no glide).
    sidebar_new_keys: std::collections::HashSet<String>,
    resort_epoch: usize,
    /// Last observed `window.is_window_active()` — rising edge fires a
    /// ProbeSync so a broadcast-deaf room heals as the user looks at the app.
    was_window_active: bool,
    /// Dev/testing knobs (`ZERON_OPEN_DIALOG`, `ZERON_FORCE_GATE`,
    /// `ZERON_DEMO_UPLOAD`) — see [`Shell::new`].
    debug_dialog: Option<String>,
    debug_gate: Option<GatePhase>,
    debug_upload: Option<String>,
    sidebar_tween: Option<WidthTween>,
    sidebar_edge_bounce: Option<motion::ResizeEdgeBounce>,
    /// Boundary currently held during a sidebar drag. Cleared on re-entry or
    /// release so the next genuine edge crossing can acknowledge the limit.
    sidebar_resize_edge: Option<motion::ResizeEdge>,
    /// Gesture-owned resize feedback. Unlike hover, this stays active while
    /// the seam moves away from the pointer and clears only on release or when
    /// a constrained edge takes over with its bounce cue.
    pane_resize_active: Option<PaneResizeKind>,
    pane_resize_dragging: Option<PaneResizeKind>,
    right_tween: Option<WidthTween>,
    right_edge_bounce: Option<motion::ResizeEdgeBounce>,
    right_resize_edge: Option<motion::ResizeEdge>,
    /// Mirrors `right_tween` only for takeover entry/exit, allowing the visible
    /// right-panel contents to resize with their outer frame in that mode.
    right_takeover_content_tween: Option<WidthTween>,
    /// Conversation-width tween used only while entering/leaving right-pane
    /// takeover. Normal right-pane open/close keeps the upstream flex behavior.
    main_takeover_tween: Option<WidthTween>,
    /// Changes-panel takeover (the header's expand button): the panel fills
    /// everything right of the sidebar and the conversation column collapses
    /// to zero. Session-local view state — never persisted, reset on close.
    right_pane_expanded: bool,
    /// Viewport width stamped each frame at render — the expanded panel's
    /// width target and the physical ceiling for free-form resizing
    /// ([`Self::right_target`] has no `Window`).
    viewport_width: f32,
    viewport_height: f32,
    terminal_tween: Option<WidthTween>,
    /// Last observed `window.is_fullscreen()` (`None` before first paint) —
    /// flips key the traffic-light inset tween.
    fullscreen: Option<bool>,
    /// 200ms ease-out tween of the cluster start on fullscreen toggles.
    titlebar_tween: Option<WidthTween>,
    titlebar_island: Option<WidthTween>,
    /// Armed by mouse-down on a titlebar strip; the next mouse-move hands the
    /// drag to the compositor (zed's platform-titlebar pattern).
    titlebar_should_move: bool,
    /// The caption buttons zeron itself draws on Linux under client-side
    /// decorations, per side, already filtered to what the compositor
    /// supports — `None` off Linux or under server decorations (where the WM
    /// draws real buttons). Re-resolved every frame at the top of `render`.
    linux_captions: Option<gpui::WindowButtonLayout>,
    /// Re-renders when the desktop's button layout changes (GNOME
    /// `button-layout` gsetting). Registered on first paint — [`Shell::new`]
    /// has no window.
    button_layout_sub: Option<Subscription>,
    /// Clears the height tween once it completes (so a closed panel unmounts).
    terminal_tween_task: Option<Task<()>>,
    /// Height-drag anchor: (pointer y, height) at mouse-down on the handle.
    terminal_drag_anchor: Option<(f32, f32)>,
    /// `motion::reduced_motion` snapshot, refreshed at the top of each render
    /// pass so [`Shell::eval_tween`] (called from `&self` render helpers) can
    /// snap without a `cx`.
    reduced_motion: bool,
    /// Set by [`Shell::eval_tween`] when any tween is mid-flight this frame;
    /// render schedules the next animation frame off it.
    motion_active: std::cell::Cell<bool>,
    /// All pane masks and chrome evaluate animation at the same frame time.
    /// A slow render must not give the native page and its titlebar different widths.
    render_time: Option<std::time::Instant>,
    splash: SplashPhase,
    splash_task: Option<Task<()>>,
    /// Focus fallback (registered on first paint — [`Shell::new`] has no
    /// window): keyboard shortcuts dispatch through the window focus chain, so
    /// with missing or unmounted focus they go dead. Recover after handoffs
    /// settle, preserving focus on mounted controls.
    focus_sub: Option<Subscription>,
    shortcut_focus: FocusHandle,
    /// Neutral shortcut target after clicking away from an input.
    unfocused: FocusHandle,
    /// Clears the jump hints when the window deactivates: a Cmd+Tab away
    /// swallows the key-up, so without this the chips stay on screen for good.
    activation_sub: Option<Subscription>,
    /// 1s heartbeat re-rendering the working indicator (elapsed + flavour word).
    _ticker: Task<()>,
    _state_observation: Subscription,
    _composer_events: Subscription,
    /// The primary transcript's spawn-chip events (subagent tabs).
    _transcript_events: Subscription,
    _transcript_invalidation: Subscription,
}

impl Shell {
    pub fn new(state: Entity<AppState>, boot: EngineBootConfig, cx: &mut Context<Self>) -> Self {
        let observation = cx.observe(&state, |this: &mut Shell, state, cx| {
            this.on_state_changed(&state, cx);
            cx.notify();
        });
        let transcript = cx.new(|cx| Transcript::new(state.clone(), cx));
        transcript.update(cx, |transcript, _| transcript.retain_for_route_exit());
        let composer = cx.new(|cx| Composer::new(state.clone(), cx));
        let links = Self::session_links(None, cx);
        transcript.update(cx, |transcript, _| {
            transcript.set_workspace_link_handler(links)
        });
        // Every send glides the prompt to the viewport top and reserves the
        // reply's space below it (notes-app parity).
        let composer_events = cx.subscribe(&composer, {
            let transcript = transcript.clone();
            move |this: &mut Shell, _, event: &ComposerEvent, cx| match event {
                ComposerEvent::NewThreadTransitionStarted => {
                    // Route observation drives the dock once selection commits.
                    cx.notify();
                }
                ComposerEvent::Sent {
                    chat_id,
                    message_id,
                } => {
                    transcript.update(cx, |t, cx| {
                        t.on_own_send(chat_id.clone(), message_id.clone(), cx)
                    });
                }
                ComposerEvent::WorktreeSetup {
                    chat_id,
                    setup_action,
                    setup_error,
                    target_device_id,
                } => this.attach_worktree_setup(
                    chat_id.clone(),
                    setup_action.clone(),
                    setup_error.clone(),
                    target_device_id.clone(),
                    cx,
                ),
                ComposerEvent::Queued {
                    chat_id,
                    message_id,
                } => {
                    transcript.update(cx, |t, cx| {
                        t.on_own_queued_send(chat_id.clone(), message_id.clone(), cx)
                    });
                }
            }
        });
        // Spawn chips open their subagent's transcript as a right-pane tab.
        let transcript_events = cx.subscribe(&transcript, Self::on_transcript_event);
        // Working-indicator heartbeat: notify once a second while a session is
        // live so elapsed time and the flavour word stay fresh.
        let ticker = cx.spawn(async move |this, cx| {
            let mut displayed_minute = Utc::now().timestamp().div_euclid(60);
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let minute = Utc::now().timestamp().div_euclid(60);
                let minute_changed = minute != displayed_minute;
                displayed_minute = minute;
                let alive = this.update(cx, |shell: &mut Shell, cx| {
                    let live = {
                        let s = shell.state.read(cx);
                        s.selected_chat
                            .as_deref()
                            .is_some_and(|id| s.indicator_for(id, Utc::now()) != Indicator::None)
                            // The connection pill's retry countdown needs the
                            // same per-second refresh while degraded.
                            || matches!(
                                s.connectivity.state,
                                zeron_proto::ConnectivityState::Offline
                                    | zeron_proto::ConnectivityState::Reconnecting
                            )
                    };
                    // Relative sidebar times still advance when unchanged
                    // presence heartbeats no longer invalidate the whole UI.
                    if live || minute_changed {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        let data_dir = boot.data_dir.clone();
        let settings = settings::current(cx);
        state.update(cx, |state, cx| {
            state.set_change_requests_visible(settings.sidebar_show_pull_request, cx)
        });
        crate::appshots::set_enabled(settings.appshots_enabled);
        crate::appshots::set_capture_sound_enabled(settings.appshot_sound_enabled);
        // Bind the customizable shortcuts from the persisted keymap.
        apply_keymap(cx, &settings.keymap, settings.composer_send_behavior);
        // Dev/testing knob: `ZERON_OPEN_ROUTE=settings[/<section>]` boots
        // straight into a settings section — these pages have no deep link and
        // synthetic input can't reach them on headless compositors.
        let route = match std::env::var("ZERON_OPEN_ROUTE").ok().as_deref() {
            Some("settings") | Some("settings/devices") => {
                Route::Settings(SettingsSection::Devices)
            }
            Some("settings/agents") => Route::Settings(SettingsSection::Agents),
            Some("settings/harnesses") => Route::Settings(SettingsSection::Harnesses),
            Some("settings/appearance") => Route::Settings(SettingsSection::Appearance),
            Some("settings/notifications") => Route::Settings(SettingsSection::Notifications),
            Some("settings/shortcuts") => Route::Settings(SettingsSection::Shortcuts),
            Some("settings/appshots") => Route::Settings(SettingsSection::Appshots),
            Some("settings/archived") => Route::Settings(SettingsSection::Archived),
            // `new` pins the new-chat canvas (suppresses boot auto-select).
            Some("new") => {
                state.update(cx, |s, _| s.auto_selected = true);
                Route::Chat
            }
            _ => Route::Chat,
        };
        // More capture knobs of the same kind: `ZERON_OPEN_DIALOG=rename|delete`
        // opens that dialog for the first chat once chats land; `=model` pops
        // the combined harness/model menu once the shell is Ready;
        // `ZERON_FORCE_GATE=signin|org|failed` renders that gate regardless of
        // real auth state (display-only — for styling passes).
        let debug_dialog = std::env::var("ZERON_OPEN_DIALOG").ok();
        // `ZERON_DEMO_UPLOAD=<pct>:<image path>` fabricates an in-flight image
        // send on the selected chat (echo bubble + frozen thumbnail progress
        // ring) — display-only; a real upload can't be paused for a capture.
        let debug_upload = std::env::var("ZERON_DEMO_UPLOAD").ok();
        let debug_gate = match std::env::var("ZERON_FORCE_GATE").ok().as_deref() {
            Some("signin") => Some(GatePhase::SignIn),
            Some("org") => Some(GatePhase::OrgGate),
            Some("failed") => Some(GatePhase::Failed(
                "Could not reach the zeron engine on port 27901".into(),
            )),
            _ => None,
        };
        let nav = NavHistory::new(match route {
            Route::Chat => NavEntry::Chat(String::new()),
            Route::Settings(section) => NavEntry::Settings(section),
        });
        // Parent notifications carry presentation changes (session status,
        // elapsed labels, menus); sibling animation/caret ticks do not.
        let transcript_invalidation = cx.observe_self(|shell, cx| {
            shell.transcript.update(cx, |_, cx| cx.notify());
        });
        let shell = cx.entity();
        let sidebar_pane = cx.new(|cx| SidebarPane {
            shell: shell.downgrade(),
            _observation: cx.observe(&shell, |_, _, cx| cx.notify()),
        });
        Self {
            state,
            sidebar_pane,
            transcript,
            composer,
            // Seed with the compact composer stack's rough height so the
            // first frame's clearance isn't zero (the measure corrects it).
            bottom_stack: std::rc::Rc::new(std::cell::Cell::new(120.0)),
            bottom_stack_has_composer: std::rc::Rc::new(std::cell::Cell::new(false)),
            composer_dock: Default::default(),
            new_thread_artwork_ready: Default::default(),
            archived_open: true,
            pinned_open: true,
            sessions_open: true,
            archived_shown: 0,
            sidebar_collapsed_groups: std::collections::HashSet::new(),
            sidebar_disclosure_motion: std::collections::HashMap::new(),
            jump_hints: false,
            terminal: None,
            right_terminal: None,
            right_plus: popover::Popup::default(),
            project_actions: crate::project_actions::ProjectActionsController::default(),
            diffs: std::collections::HashMap::new(),
            files: std::collections::HashMap::new(),
            files_subs: std::collections::HashMap::new(),
            file_surfaces: std::collections::HashMap::new(),
            file_surface_paths: std::collections::HashMap::new(),
            file_surface_keys: std::collections::HashMap::new(),
            file_surface_subs: std::collections::HashMap::new(),
            file_surface_seq: 0,
            pending_file_closes: std::collections::HashSet::new(),
            pending_exit: None,
            diff_subs: std::collections::HashMap::new(),
            diff_seq: 0,
            subagent_tabs: std::collections::HashMap::new(),
            subagent_seq: 0,
            browsers: std::collections::HashMap::new(),
            browser_subs: std::collections::HashMap::new(),
            browser_seq: 0,
            browser_context: crate::browser::BrowserContext::default(),
            browser_profile: None,
            right_tabs: std::collections::HashMap::new(),
            right_tab_drag: None,
            right_tab_scroll: gpui::ScrollHandle::new(),
            route,
            nav,
            devices_page: None,
            archived_page: None,
            appearance_page: None,
            files_settings_page: None,
            notifications_page: None,
            shortcuts_page: None,
            accounts_page: None,
            harnesses_page: None,
            shortcuts_sub: None,
            notifications_sub: None,
            files_settings_sub: None,
            appearance_settings_sub: None,
            chat_menu: popover::Popup::default(),
            rename_dialog: None,
            delete_confirm: None,
            space_menu: popover::Popup::default(),
            rename_space_dialog: None,
            delete_space_confirm: None,
            add_space: None,
            command_palette: None,
            spaces_menu: popover::Popup::default(),
            spaces_menu_bar: popover::MenuScrollbarState::default(),
            sidebar_view_menu: popover::Popup::default(),
            sidebar_view_trigger_focus: cx.focus_handle().tab_stop(true),
            sidebar_pinned_heights: Vec::new(),
            project_icons: Default::default(),
            chat_status_hover: None,
            sidebar_scroll: gpui::ScrollHandle::new(),
            pinned_session_drag: None,
            pinned_session_drag_generation: 0,
            sidebar_session_transfer: None,
            sidebar_session_return: None,
            sidebar_pin_write: None,
            sidebar_pin_write_generation: 0,
            sidebar_pin_write_notice: None,
            space_boot_applied: false,
            sound_prev: std::collections::HashMap::new(),
            connectivity_notifications: Default::default(),
            attention_sound_gate: Default::default(),
            user_menu: popover::Popup::default(),
            sidebar_notice: None,
            update_flow: UpdateFlow::Idle,
            update_task: None,
            update_dismissed: None,
            install: zeron_update::detect_install(),
            org: None,
            sync_flow: SyncFlow::Idle,
            mutate_task: None,
            auth_task: None,
            runtime_change_task: None,
            runtime_change_error: None,
            import_task: None,
            import_current: None,
            boot,
            data_dir,
            settings,
            panels: SessionPanels::default(),
            active_chat: String::new(),
            last_appshot_chat: None,
            sidebar_prev_order: Vec::new(),
            sidebar_resort: std::collections::HashMap::new(),
            sidebar_new_keys: std::collections::HashSet::new(),
            resort_epoch: 0,
            was_window_active: false,
            debug_dialog,
            debug_gate,
            debug_upload,
            sidebar_tween: None,
            sidebar_edge_bounce: None,
            sidebar_resize_edge: None,
            pane_resize_active: None,
            pane_resize_dragging: None,
            right_tween: None,
            right_edge_bounce: None,
            right_resize_edge: None,
            right_takeover_content_tween: None,
            main_takeover_tween: None,
            right_pane_expanded: false,
            viewport_width: 1280.0,
            viewport_height: 880.0,
            terminal_tween: None,
            fullscreen: None,
            titlebar_tween: None,
            titlebar_island: None,
            titlebar_should_move: false,
            linux_captions: None,
            button_layout_sub: None,
            terminal_tween_task: None,
            terminal_drag_anchor: None,
            reduced_motion: false,
            motion_active: std::cell::Cell::new(false),
            render_time: None,
            splash: SplashPhase::Visible,
            splash_task: None,
            focus_sub: None,
            shortcut_focus: cx.focus_handle(),
            unfocused: cx.focus_handle(),
            activation_sub: None,
            _ticker: ticker,
            _state_observation: observation,
            _composer_events: composer_events,
            _transcript_events: transcript_events,
            _transcript_invalidation: transcript_invalidation,
        }
    }

    /// Route a completed viewer-side capture only after its source window is
    /// safely captured. The explicit target key avoids relying on the
    /// state-observation/draft-swap effect ordering when opening the canvas.
    pub fn receive_appshot(
        &mut self,
        appshot: crate::appshots::CapturedAppshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::appshots::AppshotDestination;

        let selected = self.state.read(cx).selected_chat.clone();
        let target = match self.settings.appshot_destination {
            AppshotDestination::Automatic if selected.is_some() => selected,
            AppshotDestination::LastSession if selected.is_some() => selected,
            AppshotDestination::LastSession => self
                .last_appshot_chat
                .clone()
                .filter(|id| self.state.read(cx).chats.iter().any(|chat| &chat.id == id)),
            AppshotDestination::Automatic | AppshotDestination::NewSession => None,
        };
        if let Some(chat_id) = &target {
            self.open_chat(chat_id.clone(), cx);
        } else if self.settings.appshot_destination == AppshotDestination::NewSession
            || self.state.read(cx).selected_chat.is_some()
        {
            // Reuse upstream's project-filter and device defaults for a new
            // canvas. Automatic capture on an existing canvas keeps its pick.
            self.open_new_session(cx);
        } else {
            self.route = Route::Chat;
        }
        let key = target.unwrap_or_default();
        self.composer.update(cx, |composer, cx| {
            composer.stage_appshot_for(key, appshot, cx)
        });
        window.focus(&self.composer.focus_handle(cx), cx);
        cx.notify();
    }

    pub fn show_appshot_error(
        &mut self,
        message: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.route = Route::Chat;
        self.composer
            .update(cx, |composer, cx| composer.show_appshot_error(message, cx));
        window.focus(&self.composer.focus_handle(cx), cx);
        cx.notify();
    }









    fn contain_pinned_session_drag(
        &mut self,
        event: &gpui::DragMoveEvent<SidebarSessionDrag>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(transfer) = self.sidebar_session_transfer.as_mut() {
            transfer.pointer = event.event.position;
            cx.notify();
        }
        let inside_window = event.bounds.contains(&event.event.position);
        let pointer_x = f32::from(event.event.position.x);
        let sidebar_left = f32::from(event.bounds.left());
        let inside_sidebar =
            pointer_x >= sidebar_left && pointer_x <= sidebar_left + self.settings.sidebar_width;
        if !inside_window || !inside_sidebar {
            if let Some(transfer) = self.sidebar_session_transfer.as_mut() {
                transfer.preview = None;
            }
            self.cancel_pinned_session_drag(cx);
        }
    }

    fn retry_engine(&mut self, cx: &mut Context<Self>) {
        AppState::bootstrap(self.state.clone(), self.boot.clone(), cx);
    }

    // ---- routes / settings ----

    /// Close the user menu through the exit animation (no-op when closed).
    pub(super) fn close_user_menu(&mut self, cx: &mut Context<Self>) {
        if self.user_menu.begin_close() {
            popover::reap_popup(cx, |shell: &mut Self| &mut shell.user_menu);
            cx.notify();
        }
    }


    // ---- back/forward (route history) ----

    fn navigate_back(&mut self, cx: &mut Context<Self>) {
        if let Some(entry) = self.nav.back() {
            self.apply_nav(entry, cx);
        }
    }

    fn navigate_forward(&mut self, cx: &mut Context<Self>) {
        if let Some(entry) = self.nav.forward() {
            self.apply_nav(entry, cx);
        }
    }

    /// Land on a history entry WITHOUT recording a new one: the stack already
    /// points at `entry` (back/forward moved the index); the selection change
    /// this triggers dedups against `current()` in [`Self::on_state_changed`].
    pub(super) fn apply_nav(&mut self, entry: NavEntry, cx: &mut Context<Self>) {
        self.suspend_file_images(cx);
        match entry {
            NavEntry::Chat(chat_id) => {
                self.route = Route::Chat;
                self.focus_composer(cx);
                let target = (!chat_id.is_empty()).then_some(chat_id);
                if self.state.read(cx).selected_chat != target {
                    self.state.update(cx, |s, cx| s.select_chat(target, cx));
                }
            }
            NavEntry::Settings(section) => {
                self.route = Route::Settings(section);
            }
        }
        self.close_user_menu(cx);
        self.close_chat_menu(cx);
        cx.notify();
    }


    /// Update strip: shown above the user menu whenever the engine's
    /// UpdateStatus stream reports a newer release. On a macOS bundle install
    /// it drives the whole flow — click to download, then click to restart into
    /// the staged bundle. Elsewhere (managed/source installs) it is advisory
    /// (`zeron update`); click dismisses it for that version.
    pub(super) fn render_update_strip(&mut self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let status = self.state.read(cx).update.clone()?;
        if !status.update_available {
            return None;
        }
        let latest = status.latest_version.clone()?;
        if self.update_dismissed.as_deref() == Some(latest.as_str()) {
            return None;
        }
        let desktop_update = self.install.supports_desktop_update();

        let (label, clickable): (SharedString, bool) = if desktop_update {
            match &self.update_flow {
                UpdateFlow::Idle => (format!("Update available — v{latest}").into(), true),
                UpdateFlow::Downloading => (format!("Downloading v{latest}…").into(), false),
                UpdateFlow::Ready(_) => ("Update ready — restart to apply".into(), true),
                UpdateFlow::Failed(message) => (format!("Update failed: {message}").into(), true),
            }
        } else {
            (
                format!("Update available — v{latest} · run `zeron update`").into(),
                true,
            )
        };
        let failed = matches!(self.update_flow, UpdateFlow::Failed(_));
        let tone = if failed { theme.danger } else { theme.accent };
        // Follow the selected spectrum with a low-emphasis glass tint rather
        // than painting the bright text accent as a solid slab.
        let (chip_bg, chip_bg_hover) = if failed {
            (theme.danger.opacity(0.14), theme.danger.opacity(0.22))
        } else {
            (theme.accent_wash, theme.accent.opacity(0.16))
        };

        let mut strip = div()
            .id("update-strip")
            .mx(px(Theme::SPACE_SM))
            // No bottom margin: the user-menu block below carries its own
            // SPACE_SM padding — doubling it read as a hole (user report).
            .px(px(Theme::SPACE_SM))
            .py(px(6.0))
            .rounded(px(Theme::CONTROL_RADIUS))
            .bg(chip_bg)
            .flex()
            .flex_row()
            .items_center()
            .text_size(crate::typography::ui_rems(11.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(tone)
            .child(div().flex_1().min_w_0().child(label));
        if clickable {
            strip = strip
                .cursor_pointer()
                .hover(move |s| s.bg(chip_bg_hover))
                .on_click(cx.listener(move |this, _, _, cx| this.on_update_strip_click(cx)));
        }
        Some(strip.into_any_element())
    }

    /// Idle → download; Ready → swap + relaunch; Failed → retry; advisory
    /// installs → dismiss for this version.
    fn on_update_strip_click(&mut self, cx: &mut Context<Self>) {
        if !self.install.supports_desktop_update() {
            self.update_dismissed = self
                .state
                .read(cx)
                .update
                .as_ref()
                .and_then(|s| s.latest_version.clone());
            cx.notify();
            return;
        }
        match std::mem::replace(&mut self.update_flow, UpdateFlow::Idle) {
            UpdateFlow::Idle | UpdateFlow::Failed(_) => self.begin_update_download(cx),
            UpdateFlow::Downloading => self.update_flow = UpdateFlow::Downloading,
            UpdateFlow::Ready(staged) => self.apply_staged_update(staged, cx),
        }
    }

    /// Fetch the manifest and stage the new Zeron desktop bundle under the data dir
    /// (tokio — reqwest); the strip flips to "restart to apply" when done.
    fn begin_update_download(&mut self, cx: &mut Context<Self>) {
        let edge_url = self.boot.edge_url.clone();
        let data_dir = self.data_dir.clone();
        let install = self.install.clone();
        self.update_flow = UpdateFlow::Downloading;
        let download = Tokio::spawn(cx, async move {
            let manifest = zeron_update::fetch_latest(&edge_url).await?;
            install.stage_desktop(&edge_url, &manifest, &data_dir).await
        });
        self.update_task = Some(cx.spawn(async move |this, cx| {
            let outcome = match download.await {
                Ok(Ok(staged)) => Ok(staged),
                Ok(Err(err)) => Err(format!("{err:#}")),
                Err(join_err) => Err(join_err.to_string()),
            };
            this.update(cx, |shell, cx| {
                shell.update_flow = match outcome {
                    Ok(staged) => UpdateFlow::Ready(staged),
                    Err(message) => {
                        tracing::warn!(%message, "update download failed");
                        UpdateFlow::Failed(message.into())
                    }
                };
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Swap the staged bundle over the installed one, arm the detached
    /// relauncher, and quit — the relauncher `open`s the new bundle once this
    /// process (and its engine lock / IPC port) is gone.
    fn apply_staged_update(&mut self, staged: PathBuf, cx: &mut Context<Self>) {
        if !self.prepare_exit(PendingExit::InstallUpdate(staged.clone()), cx) {
            return;
        }
        match self.install.apply_desktop(&staged) {
            Ok(()) => {
                crate::app_menus::quit_after_save(cx);
            }
            Err(err) => {
                tracing::error!(error = %err, "update apply failed");
                self.update_flow = UpdateFlow::Failed(format!("{err:#}").into());
                cx.notify();
            }
        }
    }

    /// Scope-aware sidebar identity and account menu. Local runtimes advertise
    /// their storage boundary and offer sync; synced runtimes offer sign-out.
    pub(super) fn render_user_menu(
        &mut self,
        user_line: SharedString,
        trigger_subline: Option<SharedString>,
        menu_identity: SharedString,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = &theme.for_popup();
        let open = self.user_menu.is_open();
        let action = account_menu_action(self.state.read(cx).workspace_scope, self.sync_flow);
        // Bottom-of-sidebar identity: avatar circle + scope/account label and
        // its secondary status line.
        let initial: SharedString = user_line
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "?".into())
            .into();
        let mut trigger = div()
            .id("user-menu")
            .flex_none()
            .rounded(px(8.0))
            .px(px(Theme::SPACE_SM))
            .py(px(Theme::SPACE_SM))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.0))
            .cursor_pointer()
            // user-menu.tsx trigger: hover `bg-white/[0.04]`, open state
            // (`data-[state=open]`) the slightly stronger `bg-white/[0.06]`;
            // the hover wash fades over `transition-colors`.
            .bg(if open {
                theme.glass_hover()
            } else {
                motion::hover_blend(
                    "user-menu-trigger",
                    theme.glass_hover().opacity(0.0),
                    theme.glass_hover().opacity(0.8),
                )
            })
            .on_hover(motion::hover_listener("user-menu-trigger"))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.user_menu.note_trigger_press()),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                // A press that found the menu open closes it (the card's
                // mouse-down-out already began the close) — never reopen.
                if this.user_menu.take_press_was_open() {
                    this.close_user_menu(cx);
                } else {
                    this.user_menu.open(());
                }
                cx.notify();
            }))
            .child(
                // Avatar: white circle, initial in near-black (zeron user-menu.tsx).
                div()
                    .size(px(28.0))
                    .flex_none()
                    .rounded_full()
                    .bg(theme.text)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(crate::typography::ui_rems(12.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.bg)
                    .child(initial),
            )
            .child(
                // Name with an optional status line underneath — no chip on the right.
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(13.0))
                            .line_height(px(17.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.text)
                            .truncate()
                            .child(user_line.clone()),
                    )
                    .when_some(trigger_subline, |identity, subline| {
                        identity.child(
                            div()
                                .text_size(crate::typography::ui_rems(11.0))
                                .line_height(px(15.0))
                                .text_color(theme.text_muted)
                                .child(subline),
                        )
                    }),
            );
        if self.user_menu.get().is_some() {
            let closing = self.user_menu.closing_since();
            // user-menu.tsx content: `w-[--radix-dropdown-menu-trigger-width]`
            // (exactly as wide as the trigger row — sidebar minus its p-2
            // gutters), `flex-col gap-0.5`, then: one small muted email line
            // (`px-2 pb-1 pt-1.5 text-[11px] text-muted-foreground/70`),
            // the action selected by the runtime scope, then "Settings".
            let menu = popover::popover_card(theme)
                .w(px(self.settings.sidebar_width - 2.0 * Theme::SPACE_SM))
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.close_user_menu(cx);
                }))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .px(px(8.0))
                        .pt(px(6.0))
                        .pb(px(4.0))
                        .text_size(crate::typography::ui_rems(11.0))
                        .text_color(theme.text_muted)
                        .truncate()
                        .child(menu_identity),
                )
                .when_some(action, |menu, action| {
                    let row = match action {
                        AccountMenuAction::EnableSync => {
                            popover::menu_row(theme, false, "user-menu-enable-sync")
                                .id("user-menu-enable-sync")
                                .on_click(cx.listener(|this, _, _, cx| this.start_sign_in(cx)))
                                .child(
                                    icon(icons::GLOBAL)
                                        .size(px(16.0))
                                        .text_color(theme.text_muted),
                                )
                                .child(SharedString::from("Enable sync"))
                                .into_any_element()
                        }
                        AccountMenuAction::SyncInProgress => {
                            popover::menu_row(theme, false, "user-menu-sync-progress")
                                .id("user-menu-sync-progress")
                                .opacity(0.6)
                                .child(
                                    icon(icons::GLOBAL)
                                        .size(px(16.0))
                                        .text_color(theme.text_muted),
                                )
                                .child(SharedString::from("Sync setup in progress"))
                                .into_any_element()
                        }
                        AccountMenuAction::RestartPending => {
                            popover::menu_row(theme, false, "user-menu-sync-restart")
                                .id("user-menu-sync-restart")
                                .on_click(cx.listener(|this, _, _, cx| this.reopen_sync_notice(cx)))
                                .child(
                                    icon(icons::RESTART)
                                        .size(px(16.0))
                                        .text_color(theme.text_muted),
                                )
                                .child(SharedString::from("Finish sync setup"))
                                .into_any_element()
                        }
                        AccountMenuAction::SignOut => {
                            popover::menu_row(theme, false, "user-menu-signout")
                                .id("user-menu-signout")
                                .on_click(cx.listener(|this, _, _, cx| this.request_sign_out(cx)))
                                .child(
                                    icon(icons::LOGOUT_2)
                                        .size(px(16.0))
                                        .text_color(theme.text_muted),
                                )
                                .child(SharedString::from("Sign out"))
                                .into_any_element()
                        }
                    };
                    menu.child(row).child(popover::menu_separator())
                })
                .child(
                    popover::menu_row(theme, false, "user-menu-settings")
                        .id("user-menu-settings")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.open_settings(SettingsSection::Devices, cx)
                        }))
                        .child(
                            icon(icons::SETTINGS_MINIMALISTIC)
                                .size(px(16.0))
                                .text_color(theme.text_muted),
                        )
                        .child(SharedString::from("Settings")),
                )
                .into_any_element();
            trigger = trigger.child(popover::anchored_menu_above(
                "user-menu-popover",
                menu,
                closing,
            ));
        }
        trigger.into_any_element()
    }

    fn active_changes(&self, cx: &App) -> Option<Entity<Changes>> {
        if !self.right_pane_open(cx) {
            return None;
        }
        let RightSurface::Diff(id) = self.resolved_right_active(cx) else {
            return None;
        };
        self.diffs.get(&id).cloned()
    }

    /// Resolve shell-owned Escape surfaces in capture phase, before focused
    /// descendants such as an integrated terminal can consume the key.
    fn capture_escape_surface(&mut self, cx: &mut Context<Self>) -> bool {
        // Modals and context menus sit above the rest of the shell. Preserve
        // their existing behavior: only surfaces that already have a Cancel
        // path close here; the others remain explicit blockers.
        if self.sync_flow.has_visible_overlay()
            || self.delete_confirm.is_some()
            || self.delete_space_confirm.is_some()
            || self.chat_menu.get().is_some()
            || self.space_menu.get().is_some()
            || self.user_menu.get().is_some()
        {
            return true;
        }
        if self.rename_dialog.is_some() {
            self.rename_dialog = None;
            cx.notify();
            return true;
        }
        if self.rename_space_dialog.is_some() {
            self.rename_space_dialog = None;
            cx.notify();
            return true;
        }
        if self.add_space.is_some() {
            self.add_space = None;
            cx.notify();
            return true;
        }
        if self.spaces_menu.is_open() {
            self.close_spaces_menu(cx);
            return true;
        }
        if self.spaces_menu.get().is_some() {
            return true;
        }

        if self.right_plus.is_open() {
            self.close_right_plus(cx);
            return true;
        }
        if self.right_plus.get().is_some() {
            return true;
        }
        self.active_changes(cx)
            .is_some_and(|changes| changes.update(cx, |changes, cx| changes.handle_escape(cx)))
    }

    fn on_key_down_capture(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.key == "escape" && self.sidebar_session_transfer.is_some() {
            cx.stop_active_drag(window);
            self.cancel_sidebar_session_transfer(cx);
            cx.stop_propagation();
            return;
        }
        if event.keystroke.key == "escape" && self.command_palette.is_some() {
            self.close_command_palette(window, cx);
            cx.stop_propagation();
            return;
        }
        if event.keystroke.key == "escape" && self.capture_escape_surface(cx) {
            cx.stop_propagation();
        }
    }

    fn on_key_down(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Inputs and completion menus consume Tab first. Unhandled Tab walks
        // accessible controls, including individual transcript link ranges.
        let modifiers = event.keystroke.modifiers;
        if event.keystroke.key == "tab"
            && !modifiers.control
            && !modifiers.alt
            && !modifiers.platform
        {
            if modifiers.shift {
                window.focus_prev(cx);
            } else {
                window.focus_next(cx);
            }
            cx.stop_propagation();
            return;
        }
        let selected_chat = self.state.read(cx).selected_chat.clone();
        let indicator = selected_chat
            .as_deref()
            .map(|chat_id| self.state.read(cx).indicator_for(chat_id, Utc::now()))
            .unwrap_or(Indicator::None);
        let interrupting = selected_chat
            .as_deref()
            .is_some_and(|chat_id| self.composer.read(cx).is_interrupting(chat_id));
        let escape_stops_active_agent = self.settings.escape_stops_active_agent;

        match resolve_shell_escape(
            &event.keystroke.key,
            false,
            escape_stops_active_agent,
            self.route,
            selected_chat.as_deref(),
            indicator,
            interrupting,
        ) {
            ShellEscapeOutcome::Blocked => cx.stop_propagation(),
            ShellEscapeOutcome::InterruptChat(chat_id) => {
                cx.stop_propagation();
                self.composer
                    .update(cx, |composer, cx| composer.interrupt_chat(chat_id, cx));
            }
            ShellEscapeOutcome::OtherKey | ShellEscapeOutcome::Ignored => {}
        }
    }

    fn render_overlays(
        &mut self,
        viewport: gpui::Size<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut overlays: Vec<AnyElement> = Vec::new();

        if let Some(overlay) = self.render_chat_menu_overlay(cx) {
            overlays.push(overlay);
        }

        if let Some(overlay) = self.render_rename_dialog_overlay(viewport, window, cx) {
            overlays.push(overlay);
        }

        overlays.extend(self.render_space_overlays(viewport, window, cx));
        if let Some(overlay) = self.render_command_palette(viewport, window, cx) {
            overlays.push(overlay);
        }
        if let Some(overlay) = self.render_add_space_overlay(viewport, window, cx) {
            overlays.push(overlay);
        }
        if let Some(overlay) = self.render_project_action_overlay(viewport, window, cx) {
            overlays.push(overlay);
        }

        if let Some(overlay) = self.render_delete_confirm_dialog_overlay(viewport, cx) {
            overlays.push(overlay);
        }

        if let Some(sync) = self.render_sync_overlay(viewport, cx) {
            overlays.push(sync);
        }

        overlays
    }

    fn render_main(
        &mut self,
        window: &mut Window,
        main_content_width: f32,
        transcript_width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme_owned = Theme::of(cx).clone();
        let theme = &theme_owned;
        let (border, text, faint) = (theme.border, theme.text, theme.text_faint);

        // Settings route: just the section outlet — the section label lives in
        // the unified window titlebar now (render_title_bar). Settings never
        // underlaps: pad below the overlaid titlebar.
        if let Route::Settings(section) = self.route {
            let outlet = self.settings_outlet(section, window, cx);
            return div()
                .flex_1()
                .min_w_0()
                .h_full()
                .pt(px(Theme::TITLEBAR_HEIGHT))
                .flex()
                .flex_col()
                .child(div().flex_1().min_h_0().child(outlet))
                .into_any_element();
        }

        let _ = (text, border);
        let has_selection = self.state.read(cx).selected_chat.is_some();
        let has_spaces = !self.state.read(cx).spaces.is_empty();
        let has_appshots = !self.composer.read(cx).staged_appshots().is_empty();
        let no_project = self.state.read(cx).no_project;
        let transcript_geometry_ready = bottom_stack_measurement_matches(
            self.bottom_stack_has_composer.get(),
            (has_spaces || no_project || has_appshots) && has_selection,
        );
        let ui_settings = settings::current(cx);
        let new_thread_background_setting = ui_settings.new_thread_composer_background;
        let new_thread_background_effect = ui_settings.new_thread_background_effect;
        let frame_time = self.render_time.unwrap_or_else(std::time::Instant::now);
        // Prewarm even in an established thread. Decode/effect work is not
        // contingent on a hero measurement or a navigation gesture.
        let artwork = new_thread_background_setting
            .as_ref()
            .and_then(|background| {
                crate::new_thread_background_effects::prepare(
                    new_thread_background_effect,
                    theme,
                    std::path::Path::new(&background.path),
                    cx,
                )
            });
        let artwork_opacity = self.new_thread_artwork_ready.opacity(
            artwork.as_ref().map(|image| image.id),
            self.reduced_motion,
            frame_time,
        );
        let dock_frame =
            self.composer_dock
                .borrow_mut()
                .tick(has_selection, self.reduced_motion, frame_time);
        if dock_frame.active {
            self.motion_active.set(true);
        }
        self.composer
            .update(cx, |composer, cx| composer.set_dock_frame(dock_frame, cx));
        let composer_width = self.composer_dock.borrow_mut().layout_width(
            main_content_width.min(crate::composer::COMPOSER_MAX_WIDTH),
            self.reduced_motion,
            frame_time,
        );
        self.composer.update(cx, |composer, cx| {
            composer.set_available_width(composer_width, cx)
        });
        let term_h = self.eval_tween(self.terminal_tween, self.terminal_target(cx));
        let new_thread_background_layer = (!has_selection || dock_frame.active).then(|| {
            if artwork.is_some() && artwork_opacity < 1.0 {
                window.request_animation_frame();
            }
            new_thread_background(
                artwork,
                self.viewport_height,
                (self.viewport_width - self.sidebar_now()).max(0.0),
                self.composer.read(cx).surface_bounds(),
                dock_frame.dissolve(),
                artwork_opacity * new_thread_background_opacity(theme.is_frost()),
            )
        });

        // Content outlet: selected chat → transcript; nothing selected → the
        // centered new-thread composition; no spaces at all → the onboarding
        // card. New-chat mode mints the chat id on first send.
        let departing_transcript = !has_selection && dock_frame.transcript() > 0.0;
        if !has_selection && !departing_transcript {
            self.transcript
                .update(cx, |transcript, cx| transcript.finish_route_exit(cx));
        }
        let outlet: AnyElement = if has_selection || departing_transcript {
            div()
                .relative()
                .size_full()
                .overflow_hidden()
                .child(
                    div()
                        .relative()
                        .top(px(8.0 * (1.0 - dock_frame.transcript())))
                        .size_full()
                        .when(departing_transcript, |el| el.w(px(transcript_width)))
                        .opacity(if transcript_geometry_ready || departing_transcript {
                            dock_frame.transcript()
                        } else {
                            0.0
                        })
                        .child(self.transcript.clone()),
                )
                // A departing transcript is visual history, not an active
                // interaction surface bound to the newly blank route.
                .when(departing_transcript, |el| {
                    el.child(div().absolute().inset_0().occlude())
                })
                .into_any_element()
        } else if !has_spaces && !no_project {
            // Onboarding (first boot / after the destructive wipe): no folders
            // to work in yet — one clear affordance.
            let _ = faint;
            div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .child(motion::fade_in(
                    "no-spaces-canvas",
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .child(
                            icon(icons::ZERON_LOGO)
                                .w(px(41.9))
                                .h(px(48.0))
                                .text_color(theme.text.opacity(0.09)),
                        )
                        .child(
                            div()
                                .mt(px(24.0))
                                .text_size(crate::typography::ui_rems(16.0))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(theme.text)
                                .child(SharedString::from("Add a project to get started")),
                        )
                        .child(
                            div()
                                .mt(px(6.0))
                                .text_size(crate::typography::ui_rems(13.0))
                                .text_color(theme.text_muted.opacity(0.7))
                                .child(SharedString::from(
                                    "A project is a folder on one of your devices.",
                                )),
                        )
                        .child(
                            popover::btn_primary(&theme_owned, "Add a project")
                                .id("onboarding-add-space")
                                .mt(px(20.0))
                                .on_click(cx.listener(|this, _, _, cx| this.open_add_space(cx))),
                        ),
                ))
                .into_any_element()
        } else {
            Empty.into_any_element()
        };

        let status = self.render_status_strip(cx);
        // Attachment dropzone over the ENTIRE conversation column (transcript
        // + composer, not just the pill). OS images keep using the upload
        // pipeline; workspace files/directories and file tabs become the same
        // projected file-mention chips the composer already understands.
        // The veil itself uses typed `drag_over` styles below. Do not cache
        // drag presence in shell state: the platform's `FileDrop::Exited`
        // clears GPUI's external payload without sending one last mouse-move,
        // so a cached bit can survive and reappear during an unrelated drag
        // such as a pane resize.
        div()
            .id("chat-dropzone")
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .on_drop(cx.listener(|this, paths: &gpui::ExternalPaths, _, cx| {
                let paths = paths.paths().to_vec();
                this.composer
                    .update(cx, |composer, cx| composer.add_paths(paths, cx));
                cx.notify();
            }))
            .on_drop::<WorkspacePathDrag>(cx.listener(
                |this, payload: &WorkspacePathDrag, window, cx| {
                    this.composer.update(cx, |composer, cx| {
                        composer.add_workspace_path(&payload.path, payload.is_directory, window, cx)
                    });
                    cx.notify();
                },
            ))
            .on_drop::<RightTabDrag>(cx.listener(|this, payload: &RightTabDrag, window, cx| {
                if let Some(path) = &payload.workspace_path {
                    this.composer.update(cx, |composer, cx| {
                        composer.add_workspace_path(&path.path, path.is_directory, window, cx)
                    });
                }
                cx.notify();
            }))
            // The hero is deliberately outside the transcript EdgeFade below:
            // it must paint under the overlaid titlebar instead of becoming
            // fully transparent across the titlebar's inset band.
            .children(new_thread_background_layer)
            .child(
                // Full-height underlay: the transcript viewport spans the
                // whole column, scrolling UNDER the titlebar above and the
                // composer stack below. The per-glyph EdgeFade (glass-safe,
                // same as the sidebar's) spans the full column with
                // ASYMMETRIC bands sized to the chrome: content is opaque at
                // the chrome's inner edge and fades to zero at the window
                // edge — visible mid-fade through the glass chrome it slides
                // under. Always on (the resting paddings keep pinned content
                // out of the bands, and gating on measured scroll state left
                // the top unfaded for one frame on session switch — user
                // report). The jump pill floats outside the fade scope,
                // anchored above the measured stack.
                {
                    // The terminal dock is NOT glass the transcript may slide
                    // under: with the dock's translucent fill, transcript text
                    // ghosted through the grid (user report). The underlay
                    // ends at the dock's top instead, riding the same height
                    // tween the dock animates with; `stack_h` below is only
                    // the chrome that still overlaps the transcript (status
                    // strip + composer).
                    let stack_h = (self.bottom_stack.get() - term_h).max(0.0);
                    // Opaque from the composer PILL's top (the reserved
                    // status strip above it is empty air), zero at the
                    // underlay's bottom edge.
                    let bottom_band = (stack_h - Theme::STATUS_STRIP_HEIGHT).max(1.0);
                    div().absolute().inset_0().bottom(px(term_h)).child(
                        crate::edge_fade::edge_faded(
                            Theme::TRANSCRIPT_FADE_BAND,
                            true,
                            true,
                            div().size_full().child(outlet),
                        )
                        // Fully faded BY the titlebar's bottom edge (the
                        // title text is opaque — overlap read as collision),
                        // ramping in the band just below it.
                        .inset_top(Theme::TITLEBAR_HEIGHT)
                        .band_top(Theme::TRANSCRIPT_FADE_BAND)
                        .band_bottom(bottom_band),
                    )
                },
            )
            // The glass chrome stack, floating over the transcript's bottom:
            // reserved status strip (h-6, the WorkingIndicator — the composer
            // below never shifts), composer, terminal dock. A paint-time
            // canvas measures the stack for next frame's fade inset and
            // transcript clearance. The flex_1 spacer has no id/listeners, so
            // pointer + wheel events over it fall through to the list below.
            .child(div().flex_1().min_h_0())
            .child({
                let measured = self.bottom_stack.clone();
                let measured_has_composer = self.bottom_stack_has_composer.clone();
                let contains_composer = (has_spaces || no_project || has_appshots) && has_selection;
                let composer = self.composer.clone();
                div()
                    .flex_none()
                    .relative()
                    .flex()
                    .flex_col()
                    .child(
                        gpui::canvas(
                            move |bounds, window, cx| {
                                // Reserve the destination footprint, never the animated height.
                                let next_height = f32::from(bounds.size.height)
                                    + composer.read(cx).dock_clearance_correction();
                                let changed = (measured.get() - next_height).abs() > 0.5
                                    || measured_has_composer.get() != contains_composer;
                                measured.set(next_height);
                                measured_has_composer.set(contains_composer);
                                if changed {
                                    window.request_animation_frame();
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .child(status)
                    .when(has_spaces || no_project || has_appshots, |el| {
                        let composer_opacity = self.composer_dock.borrow().opacity();
                        el.child(crate::composer_dock::docked_composer(
                            div()
                                .id("persistent-composer")
                                .relative()
                                .w(px(composer_width))
                                .opacity(composer_opacity)
                                .mx_auto()
                                .child(self.composer.clone())
                                .children(if has_selection {
                                    self.render_jump_to_bottom(cx)
                                } else {
                                    None
                                }),
                            self.composer_dock.clone(),
                            self.viewport_height,
                            self.reduced_motion,
                            frame_time,
                        ))
                    })
                    .child(self.render_terminal_container(window, cx))
            })
            .child(
                div()
                    .id("attachment-drop-overlay")
                    .absolute()
                    .inset_0()
                    .opacity(0.0)
                    .bg(theme.scrim().opacity(0.4 / 0.6))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(crate::typography::ui_rems(13.0))
                    .text_color(theme.text)
                    // GPUI matches these styles against the active payload's
                    // concrete TypeId. Resize markers therefore cannot reveal
                    // this overlay, even after an external drag exits without
                    // another move event.
                    .drag_over::<gpui::ExternalPaths>(|style, _, _, _| style.opacity(1.0))
                    .drag_over::<WorkspacePathDrag>(|style, _, _, _| style.opacity(1.0))
                    .drag_over::<RightTabDrag>(|style, tab, _, _| {
                        if tab.workspace_path.is_some() {
                            style.opacity(1.0)
                        } else {
                            style
                        }
                    })
                    .child("Drop to attach"),
            )
            .into_any_element()
    }

    /// The "↓ Scroll to bottom" pill (round-9 §3): a LABELED rounded-full
    /// chip — down-arrow glyph + 13px label on a near-opaque raised surface
    /// with a hairline — horizontally centered over the transcript column and
    /// floating six pixels above the composer. It shares the composer's
    /// measured dock transform and paints after it, outside the transcript fade.
    fn render_jump_to_bottom(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.transcript.read(cx).jump_button_shown() {
            return None;
        }
        Some(
            div()
                .absolute()
                // Share the composer's measured translation, not its final
                // bottom-stack target. Paint after the composer so it cannot
                // pass over this control during docking.
                .top(px(-36.0))
                .left_0()
                .right(px(10.0))
                .flex()
                .justify_center()
                .child(self.jump_pill("jump-to-bottom", "jump-pill", self.transcript.clone(), cx))
                .into_any_element(),
        )
    }

    /// The jump pill itself — shared between the conversation overlay and
    /// the subagent pane so both read as one control. `anim_key`/`hover_key`
    /// must be distinct per instance (they key global animation state).
    ///
    /// Use the shared popover tint and blur, with a separate hover wash so
    /// the floating control retains the same glass surface in either theme.
    fn jump_pill(
        &self,
        anim_key: &'static str,
        hover_key: &'static str,
        transcript: Entity<Transcript>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::of(cx).for_popup();
        let glass = theme.is_frost();
        let base = if glass {
            popover::surface_bg(&theme)
        } else {
            motion::hover_blend(hover_key, theme.surface_raised, theme.surface_raised_hover)
        };
        let wash = if glass {
            motion::hover_blend(hover_key, gpui::transparent_black(), theme.glass_hover())
        } else {
            gpui::transparent_black()
        };
        let pill = div()
            .id(anim_key)
            .h(px(30.0))
            .rounded_full()
            .border_1()
            .border_color(theme.border)
            .when(!glass, |el| el.shadow_md())
            .cursor_pointer()
            .bg(base)
            .on_hover(motion::hover_listener(hover_key))
            .on_click(cx.listener(move |_, _, _, cx| {
                transcript.update(cx, |transcript, cx| transcript.jump_to_bottom(cx));
            }))
            .child(
                // The hover wash rides an inner full-height layer so it
                // composites over the tint (a div has one bg).
                div()
                    .h_full()
                    .rounded_full()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .pl(px(11.0))
                    .pr(px(13.0))
                    .bg(wash)
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(13.0))
                            .text_color(theme.text_muted)
                            .child(SharedString::from("↓")),
                    )
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(13.0))
                            .text_color(theme.text)
                            .child(SharedString::from("Scroll to bottom")),
                    ),
            );
        // Frost OUTSIDE the entry animation (the composer pill's exact
        // composition): one scene layer — blur, then the pill's quads, then
        // glyphs — so the pill always composes over the transcript content
        // scrolling under it, and never loses its washes to the kind-sorted
        // draw order (frost.rs module docs).
        crate::frost::frosted(
            15.0,
            crate::frost::MENU_BLUR,
            motion::dialog_in(anim_key, pill),
        )
        .into_any_element()
    }

    /// Working indicator strip: gradient spinner + rotating flavour word (7s,
    /// seeded per chat) + elapsed, staleness-gated via [`Indicator`]; falls back
    /// to a "Sending…" bridge and then the engine mode line.
    fn render_status_strip(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let now = Utc::now();
        let state = self.state.read(cx);

        // Aligned with the composer column: centered, same max width, small
        // inner gutter (zeron's `mx-auto h-6 max-w-3xl px-2`).
        let strip = div()
            .h(px(Theme::STATUS_STRIP_HEIGHT))
            .flex_none()
            .w_full()
            .max_w(px(768.0))
            .mx_auto()
            .flex()
            .items_center()
            .gap(px(Theme::SPACE_SM))
            .px(px(Theme::SPACE_LG + 8.0))
            .text_size(crate::typography::ui_rems(11.0));

        let Some(chat_id) = state.selected_chat.clone() else {
            return strip.into_any_element();
        };
        let indicator = state.indicator_for(&chat_id, now);
        // Timer base: the freshest of the session row's turn start and the
        // in-flight send. During the send→ack window the row (if any) still
        // carries the PREVIOUS turn's start, and using it opened the timer at
        // the old turn's elapsed instead of 0:00.
        let started = state
            .session_for(&chat_id)
            .and_then(|s| s.started_at)
            .into_iter()
            .chain(state.pending_send_started(&chat_id, now))
            .max();
        let elapsed_secs = started
            .map(|t| now.signed_duration_since(t).num_seconds().max(0))
            .unwrap_or(0);
        let sending = self.composer.read(cx).is_sending();

        // Unused here since the Working loader moved into the transcript
        // (its trailer computes its own elapsed).
        let _ = elapsed_secs;
        match indicator {
            // The working loader lives in the TRANSCRIPT now, under the
            // streaming reply (user request) — the strip stays empty (its
            // reserved height still steadies the composer).
            Indicator::Working => strip.into_any_element(),
            // No label: the QuestionPanel right below IS the awaiting-input
            // surface — a strip caption above it was redundant (user request).
            Indicator::AwaitingInput => strip.into_any_element(),
            Indicator::Errored => strip
                .text_color(theme.danger)
                .child(SharedString::from("Run failed"))
                .into_any_element(),
            Indicator::None if sending => strip
                .child(loaders::gradient_spinner(
                    "sending-indicator",
                    &theme,
                    2.5,
                    cx.entity_id(),
                    cx,
                ))
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(12.0))
                        .text_color(theme.text_muted)
                        .child(SharedString::from("Sending…")),
                )
                .into_any_element(),
            Indicator::None => strip.into_any_element(),
        }
    }

    /// Right pane — the surface host (t3code RightPanelTabs): hidden by
    /// default, drag-resizable. Content is the ACTIVE surface — the Diff
    /// page (its options row + the lazy [`Changes`] viewer), workspace Files,
    /// an embedded terminal, or the surface picker when no tabs exist.
    fn render_right_pane(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
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



    fn render_signed_out_restart(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let runtime_change_label = if self.runtime_change_task.is_some() {
            "Stopping engine…"
        } else {
            "Retry local mode"
        };
        let card = div()
            .w(px(380.0))
            .px(px(32.0))
            .py(px(40.0))
            .rounded(px(12.0))
            .border_1()
            .border_color(theme.border)
            .bg(theme.surface_card)
            .shadow_lg()
            .flex()
            .flex_col()
            .items_center()
            .text_center()
            .child(
                icon(icons::ZERON_LOGO)
                    .w(px(31.4))
                    .h(px(36.0))
                    .text_color(theme.text),
            )
            .child(
                div()
                    .mt(px(24.0))
                    .text_size(crate::typography::ui_rems(18.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.text)
                    .child(SharedString::from("Signed out")),
            )
            .child(
                div()
                    .mt(px(6.0))
                    .mb(px(24.0))
                    .text_size(crate::typography::ui_rems(13.0))
                    .line_height(px(19.0))
                    .text_color(theme.text_muted)
                    .child(SharedString::from(
                        "Zeron removed your credentials but could not finish closing the previous synced workspace. Retry before continuing in local mode.",
                    )),
            )
            .when_some(self.runtime_change_error.clone(), |card, error| {
                card.child(
                    div()
                        .mb(px(16.0))
                        .text_size(crate::typography::ui_rems(12.0))
                        .line_height(px(17.0))
                        .text_color(theme.danger)
                        .child(error),
                )
            })
            .child(
                popover::btn_primary(&theme, runtime_change_label)
                    .id("signed-out-quit")
                    .when(self.runtime_change_task.is_some(), |button| {
                        button.opacity(0.6)
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.start_local_runtime_transition(false, cx)
                    })),
            );

        div()
            .absolute()
            .inset_0()
            .occlude()
            .bg(theme.bg)
            .child(grid_backdrop(&theme))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(motion::fade_in("signed-out-restart", card)),
            )
            .into_any_element()
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_time = Some(std::time::Instant::now());
        if self.all_file_edits_flushed(cx)
            && let Some(action) = self.pending_exit.take()
        {
            let shell = cx.weak_entity();
            window.defer(cx, move |window, cx| {
                if matches!(action, PendingExit::Quit) {
                    crate::app_menus::request_quit(cx);
                } else {
                    shell
                        .update(cx, |shell, cx| match action {
                            PendingExit::CloseWindow => {
                                if shell.prepare_window_close(cx) {
                                    window.remove_window();
                                }
                            }
                            PendingExit::RuntimeChange => shell.quit_for_runtime_change(cx),
                            PendingExit::InstallUpdate(staged) => {
                                shell.apply_staged_update(staged, cx)
                            }
                            PendingExit::Quit => unreachable!(),
                        })
                        .ok();
                }
            });
        }
        crate::transcript::record_view_frame("shell");
        self.viewport_width = f32::from(window.viewport_size().width);
        // Appearance actions persist independently of the shell. Mirror the
        // globals before any later debounced settings save can overwrite them.
        self.settings.appearance = crate::appearance::mode(cx);
        self.settings.theme_selection = crate::appearance::themes(cx);
        self.settings.accent = crate::appearance::accent(cx);
        self.settings.surface = crate::appearance::surface(cx);
        self.sync_independent_settings(cx);
        // The shell frost sits over native desktop blur on macOS and Windows.
        // Content surfaces add their own backgrounds over this shared tint.
        let (frost, text, font, theme_bg, theme_is_glass) = {
            let theme = Theme::of(cx);
            (
                theme.glass(),
                theme.text,
                theme.font_sans.clone(),
                theme.bg,
                theme.is_glass(),
            )
        };
        tracing::info!(?frost, theme_is_glass, "shell render root");
        let (workspace_scope, auth) = {
            let state = self.state.read(cx);
            (state.workspace_scope, state.auth.clone())
        };
        self.sync_flow = sync_flow_after_auth(self.sync_flow, workspace_scope, auth.as_ref());
        let restart_required = self.sync_flow == SyncFlow::SignedOutRestartRequired;
        let gate = self
            .debug_gate
            .clone()
            .unwrap_or_else(|| self.state.read(cx).gate());

        let browser_profile = {
            let state = self.state.read(cx);
            crate::links::workspace_locator(
                state.workspace_scope,
                state.auth.as_ref(),
                state.local_device_id.as_deref(),
            )
        };
        if browser_profile.is_some() && browser_profile != self.browser_profile {
            if self.browser_profile.is_some() {
                for browser in self.browsers.values() {
                    browser.update(cx, |browser, cx| browser.close(cx));
                }
                self.browsers.clear();
                self.browser_subs.clear();
                self.browser_context = crate::browser::BrowserContext::default();
            }
            self.browser_profile = browser_profile;
        }
        let browser_active = matches!(gate, GatePhase::Ready)
            && !restart_required
            && matches!(self.route, Route::Chat)
            && (self.right_pane_open(cx) || self.tween_active(self.right_tween));
        // Native clipping follows the animated GPUI mask. Drags only transfer
        // pointer ownership; the browser continues rendering and reflowing.
        let browser_dragging = cx.has_active_drag();
        #[cfg(target_os = "macos")]
        let browser_resize_inset = if self.right_pane_open(cx)
            && !self.right_pane_expanded
            && !self.tween_active(self.right_tween)
        {
            // The browser starts inside the panel's one-point left border.
            px(PANE_RESIZE_HITBOX_HALF_WIDTH - 1.0)
        } else {
            px(0.0)
        };
        let selected_surface = self.resolved_right_active(cx);
        for (id, browser) in &self.browsers {
            let presentation = crate::browser::model::presentation(
                browser_active && selected_surface == RightSurface::Browser(*id),
                browser_dragging,
            );
            browser.update(cx, |browser, cx| {
                #[cfg(target_os = "macos")]
                browser.set_resize_inset(browser_resize_inset, cx);
                browser.set_shortcuts(&self.settings.keymap);
                browser.set_presentation(presentation, cx);
            });
        }

        // Fullscreen hides the macOS traffic lights — reflow the control
        // cluster with a 200ms ease-out tween (§1.1). A fullscreen transition
        // resizes the window, which re-renders us, so polling here is exact.
        let fullscreen = window.is_fullscreen();
        if self.fullscreen != Some(fullscreen) {
            if self.fullscreen.is_some() && cfg!(target_os = "macos") {
                self.titlebar_tween = Some(WidthTween::new(
                    titlebar_cluster_start(!fullscreen),
                    titlebar_cluster_start(fullscreen),
                ));
            }
            self.fullscreen = Some(fullscreen);
        }
        // Linux CSD: (re-)resolve which caption buttons we draw and on which
        // side — decorations can flip server↔client at runtime and the
        // desktop's button layout is user configuration.
        self.linux_captions = Self::resolve_linux_captions(window, cx);
        if cfg!(target_os = "linux") && self.button_layout_sub.is_none() {
            self.button_layout_sub =
                Some(cx.observe_button_layout_changed(window, |_, _, cx| cx.notify()));
        }
        // Manual tween drive bookkeeping for this pass (see [`WidthTween`]).
        self.reduced_motion = motion::reduced_motion(cx);
        self.motion_active.set(false);

        if self.activation_sub.is_none() {
            self.activation_sub = Some(cx.observe_window_activation(
                window,
                |this: &mut Shell, window, cx| {
                    if !window.is_window_active() {
                        this.reset_command_palette_key_state();
                        this.set_jump_hints(false, cx);
                        this.composer.update(cx, |composer, cx| {
                            composer.set_queue_shortcut_revealed(false, cx)
                        });
                    }
                },
            ));
        }

        // A live handle can refer to an unmounted element. Recover against
        // the completed frame so newly mounted dialogs can claim focus first.
        if self.focus_sub.is_none() {
            self.focus_sub = Some(cx.on_focus_lost(window, |this: &mut Shell, window, cx| {
                let root = this.shortcut_focus.clone();
                let unfocused = this.unfocused.clone();
                let preferred = this.composer.focus_handle(cx);
                window.on_next_frame(move |window, cx| {
                    restore_mounted_focus(&root, &preferred, &unfocused, window, cx);
                });
                cx.notify();
            }));
        }
        let shortcut_focus = self.shortcut_focus.clone();
        let unfocused = self.unfocused.clone();
        let preferred_focus = self.composer.focus_handle(cx);
        window.defer(cx, move |window, cx| {
            restore_mounted_focus(&shortcut_focus, &preferred_focus, &unfocused, window, cx);
        });

        // Modifier events follow focus too. Reconcile from the window's input
        // snapshot so a missed release (or pointer event after it) heals hints.
        if !window.is_window_active() {
            self.set_jump_hints(false, cx);
        } else if self.jump_hints {
            // Only a fresh modifier event may turn hints on: activation can
            // retain the snapshot from before Cmd+Tab.
            self.update_jump_hints(&window.modifiers(), cx);
        }

        let root = div()
            .id("shell-root")
            .track_focus(&self.shortcut_focus)
            .child(div().track_focus(&self.unfocused))
            .relative()
            .flex()
            .flex_row()
            .size_full()
            .bg(frost)
            // Linux CSD: a floating window rounds its corners against the
            // desktop (the window composites with alpha — see
            // `Theme::window_background_appearance`); tiled/maximized keeps
            // square edges flush with the screen. Everything, including the
            // splash and caption chrome, clips to the curve.
            .when(Self::linux_window_floating(window), |el| {
                el.rounded(px(LINUX_WINDOW_CORNER_RADIUS)).overflow_hidden()
            })
            .text_color(text)
            .font_family(font)
            .text_size(crate::typography::ui_rems(14.0))
            .on_drag_move::<SidebarSessionDrag>(cx.listener(Self::contain_pinned_session_drag))
            .on_drop::<SidebarSessionDrag>(cx.listener(|this, _, _, cx| {
                this.cancel_sidebar_session_transfer(cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.sidebar_session_transfer.is_some() {
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.sidebar_session_transfer.is_some() {
                        cx.notify();
                    }
                }),
            )
            .capture_key_down(cx.listener(Self::on_key_down_capture))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_drag_move(cx.listener(Self::on_sidebar_drag))
            .on_drag_move(cx.listener(Self::on_right_pane_drag))
            .on_drag_move(cx.listener(Self::on_terminal_drag))
            // The panel shortcuts are chat-scoped chrome: in Settings they are
            // no-ops (zeron __root.tsx gates the hotkey on `!isSettings`, and
            // the terminal panel is only mounted on session routes). The
            // sidebar toggle stays live everywhere, as in the original.
            .on_action(cx.listener(|this, _: &ToggleTerminal, window, cx| {
                if matches!(this.route, Route::Chat) {
                    this.toggle_terminal(window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &SaveFile, _, cx| {
                if matches!(this.route, Route::Chat) && this.right_pane_open(cx) {
                    let file = match this.resolved_right_active(cx) {
                        RightSurface::Files => this.files.get(&this.panel_key(cx)).cloned(),
                        RightSurface::File(id) => this.file_surfaces.get(&id).cloned(),
                        _ => None,
                    };
                    if let Some(file) = file {
                        file.update(cx, |file, cx| file.save_active_document(cx));
                    }
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| this.toggle_sidebar(cx)))
            // New session works from anywhere — `open_new_session` routes back
            // to chat itself, so Settings is not a dead spot.
            .on_action(cx.listener(|this, _: &NewSession, _, cx| this.open_new_session(cx)))
            // Native Settings menu item and the platform convention (Cmd+, on
            // macOS, Ctrl+, elsewhere) always land on the default section.
            .on_action(cx.listener(|this, _: &OpenSettings, _, cx| {
                this.open_settings(SettingsSection::Devices, cx)
            }))
            // Chat-scoped, unlike new-session — `cycle_session` holds the guard
            // and says why.
            .on_action(cx.listener(|this, _: &NextSession, _, cx| this.cycle_session(true, cx)))
            .on_action(cx.listener(|this, _: &PrevSession, _, cx| this.cycle_session(false, cx)))
            .on_action(cx.listener(|this, _: &ToggleChanges, window, cx| {
                if matches!(this.route, Route::Chat) {
                    this.toggle_right_pane(cx);
                    if !this.right_pane_open(cx) {
                        // The hidden editor can retain a focus handle after unmounting.
                        // Restore a mounted target so the next shortcut can reopen it.
                        window.focus(&this.composer.focus_handle(cx), cx);
                    }
                }
            }))
            // Chat-scoped like the panel toggles: Settings has no current
            // session to archive. Quiet under an open popover, like the other
            // session-nav shortcuts.
            .on_action(cx.listener(|this, _: &ArchiveSession, _, cx| {
                if matches!(this.route, Route::Chat) && !this.overlay_owns_keyboard(cx) {
                    this.archive_selected_chat(cx)
                }
            }))
            // A jump routes back to chat itself, so Settings is not a dead
            // spot — the same call a click on that sidebar row makes. But an
            // open picker/palette owns the keyboard: no jumping underneath
            // it. The MODEL menu advertises these same slots on its rows and
            // this matched binding beats its key handler to the dispatch —
            // forward the slot instead of eating it.
            .on_action(cx.listener(|this, jump: &JumpSession, _, cx| {
                let pickers = this.composer.read(cx).pickers().clone();
                let handled = pickers.update(cx, |pickers, cx| pickers.jump_model_slot(jump.0, cx));
                if !handled && !this.overlay_owns_keyboard(cx) {
                    this.jump_to_session(jump.0, cx)
                }
            }))
            .on_modifiers_changed(
                cx.listener(|this, event, _, cx| this.on_modifiers_changed(event, cx)),
            )
            .on_action(cx.listener(|this, _: &ToggleCommandPalette, window, cx| {
                this.toggle_command_palette(window, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenModelPicker, window, cx| {
                if matches!(this.route, Route::Chat) && !this.overlay_owns_keyboard(cx) {
                    let pickers = this.composer.read(cx).pickers().clone();
                    pickers.update(cx, |pickers, cx| pickers.open_model_menu(window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &AddSpacePalette, _, cx| {
                if this.add_space.is_some() {
                    this.add_space = None;
                    cx.notify();
                } else {
                    this.open_add_space(cx);
                }
            }));

        let render_gate = if restart_required {
            GatePhase::Loading
        } else {
            gate.clone()
        };
        let root = match &render_gate {
            GatePhase::Ready => {
                // Focus is a sync signal: on the rising edge of window
                // activation, nudge every open room to verify liveness — a
                // broadcast-deaf socket (accepted writes, runtime pongs,
                // nothing delivered; 2026-08-04 incident) then heals within
                // seconds of the user looking at the app rather than waiting
                // out the background probe cadence.
                let window_active = window.is_window_active();
                if window_active && !self.was_window_active {
                    self.state.update(cx, |s, cx| s.probe_sync(cx));
                }
                self.was_window_active = window_active;
                // A run finishing while you're LOOKING at the session must not
                // badge "completed" until you leave and return — mark it seen
                // live while the window is active (idempotent guard inside;
                // one extra frame settles it).
                if window_active {
                    let unseen_selected = {
                        let s = self.state.read(cx);
                        s.selected_chat_row()
                            .filter(|c| c.unseen())
                            .map(|c| c.id.clone())
                    };
                    if let Some(chat_id) = unseen_selected {
                        self.state
                            .update(cx, |s, cx| s.mark_chat_seen(&chat_id, cx));
                    }
                }
                // Capture knob: `ZERON_OPEN_DIALOG=model` pops the combined
                // harness/model menu (needs `window`, so it fires here rather
                // than in `on_state_changed`).
                if self.debug_dialog.as_deref() == Some("model") {
                    self.debug_dialog = None;
                    self.composer
                        .update(cx, |c, cx| c.debug_open_model_menu(window, cx));
                }
                // MessageRail width gate: hide below 48rem of main-panel width.
                let viewport = f32::from(window.viewport_size().width);
                self.viewport_height = f32::from(window.viewport_size().height);
                // Stamped for `right_target` — the expanded changes panel
                // sizes itself to the viewport.
                self.viewport_width = viewport;
                let on_chat = matches!(self.route, Route::Chat);
                let right_target_width = if on_chat { self.right_now(cx) } else { 0.0 };
                let panel_handoff = self.composer_dock.borrow_mut().observe_pane(
                    self.state.read(cx).selected_chat.is_some(),
                    right_target_width,
                    on_chat && !self.reduced_motion,
                    self.render_time.unwrap_or_else(std::time::Instant::now),
                );
                if panel_handoff {
                    self.motion_active.set(true);
                }
                let main_target_width =
                    conversation_width(viewport, self.sidebar_target(), right_target_width);
                let main_transition = self.active_tween_endpoints(self.main_takeover_tween);
                let main_content_width =
                    stable_panel_content_width(main_target_width, main_transition);
                let transcript_width = self.composer_dock.borrow_mut().transcript_width(
                    main_content_width,
                    self.state.read(cx).selected_chat.is_some(),
                    panel_handoff,
                );
                let main_width = (transcript_width - 10.0).max(0.0);
                // Clearance excludes the terminal dock: the transcript
                // viewport ends at the dock's top (see the underlay in
                // `render_main`), so only the chrome above it overlaps.
                let term_h = self.eval_tween(self.terminal_tween, self.terminal_target(cx));
                let stack_h = (self.bottom_stack.get() - term_h).max(0.0);
                let expected_has_composer = {
                    let state = self.state.read(cx);
                    (!state.spaces.is_empty() || state.no_project) && state.selected_chat.is_some()
                };
                let bottom_stack_ready = bottom_stack_measurement_matches(
                    self.bottom_stack_has_composer.get(),
                    expected_has_composer,
                );
                self.transcript.update(cx, |t, cx| {
                    t.set_rail_enabled(rail::rail_visible(main_width), cx);
                    if bottom_stack_ready && expected_has_composer {
                        t.set_bottom_clearance(stack_h, cx);
                    }
                });

                let sidebar = self.render_sidebar(cx);
                let sidebar_handle = self.resize_handle(
                    "sidebar-resize",
                    PaneResizeKind::Sidebar,
                    || SidebarResize,
                    |shell, _| {
                        shell.settings.sidebar_width = SIDEBAR_DEFAULT;
                        shell.sidebar_edge_bounce = None;
                    },
                    cx,
                );
                let main = self.render_main(window, main_content_width, transcript_width, cx);
                // The Changes pane is chat-scoped chrome: the Settings route
                // never renders it (zeron __root.tsx `!isSettings && activeChat`
                // around the diff column) — the per-session open flags stay
                // intact for the return trip.
                let right_open = on_chat && self.right_pane_open(cx);
                // Takeover mode derives its width from the viewport, so a
                // manual drag handle would fight the expanded target.
                let right_handle = (right_open
                    && !panel_handoff
                    && !self.right_pane_expanded
                    && !self.tween_active(self.right_tween))
                .then(|| {
                    self.resize_handle(
                        "right-pane-resize",
                        PaneResizeKind::Right,
                        || RightPaneResize,
                        |shell, _| {
                            shell.settings.right_pane_width = RIGHT_PANE_DEFAULT;
                            shell.right_edge_bounce = None;
                        },
                        cx,
                    )
                    // A forgiving transparent hit target centered on the
                    // seam; the panel's 1px border remains the visual divider.
                    .left(px(-PANE_RESIZE_HITBOX_HALF_WIDTH))
                });
                let right: AnyElement = if on_chat {
                    self.render_right_pane(window, cx)
                } else {
                    Empty.into_any_element()
                };
                let overlays = self.render_overlays(window.viewport_size(), window, cx);
                // Copied out (not held) — `render_title_bar` needs `cx` mutable.
                let border_color = Theme::of(cx).border;
                // No inset cards (user request): the conversation column sits
                // flush and unbordered, the transcript directly on the frost
                // glass; the changes pane is a flush left-bordered glass panel
                // (built inside `render_right_pane`).
                let main = if main_transition.is_some() {
                    div()
                        .h_full()
                        .w(px(main_content_width))
                        .flex_none()
                        .flex()
                        .child(main)
                        .into_any_element()
                } else {
                    main
                };
                let card_bg = if cfg!(target_os = "windows") && theme_is_glass {
                    Some(match Theme::of(cx).appearance {
                        crate::theme::Appearance::Dark => theme_bg.opacity(0.35),
                        crate::theme::Appearance::Light => theme_bg.opacity(0.85),
                    })
                } else if !theme_is_glass {
                    Some(theme_bg)
                } else {
                    None
                };
                let mut card_div = div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_row()
                    .overflow_hidden();
                if let Some(bg) = card_bg {
                    card_div = card_div.bg(bg);
                }
                let card: AnyElement = card_div
                    .child(main)
                    .into_any_element();
                // The whole app page is one keyed `animate-in` entrance (zeron
                // App.tsx `<div key={phase} className="animate-in h-full">`):
                // arriving from the splash or any gate fades the page in; the
                // splash-out crossfades over it on boot.
                // The sidebar resize handle FLOATS over the sidebar/card seam
                // (zero layout width, same idiom as the changes-pane grabber)
                // so the sidebar's right gutter stays exactly as wide as its
                // left one — a 5px flex child here read as lopsided spacing.
                let sidebar_seam = div()
                    .w(px(0.0))
                    .h_full()
                    .flex_none()
                    .relative()
                    .child(sidebar_handle.left(px(-PANE_RESIZE_HITBOX_HALF_WIDTH)));
                // Keep the right resize target outside the pane's
                // overflow-hidden width container. This mirrors the sidebar
                // seam and lets the target straddle both adjacent panes.
                // Paint it after the page so page input cannot occlude the
                // inner half. A deferred draw would capture all native input.
                let right_seam: AnyElement = if let Some(handle) = right_handle {
                    div()
                        .w(px(0.0))
                        .h_full()
                        .flex_none()
                        .absolute()
                        .left_0()
                        .top_0()
                        .child(handle)
                        .into_any_element()
                } else {
                    Empty.into_any_element()
                };
                let title_bar = self.render_title_bar(window.viewport_size().height, cx);
                // Sidebar tone: a slightly lighter column behind the sidebar,
                // spanning the FULL window height (under the traffic lights,
                // through the titlebar, down to the bottom edge). Its width
                // rides the same tween as the sidebar, so the tone melts away
                // with the collapse instead of vanishing in a frame.
                let sidebar_now = self.sidebar_now();
                // Hairline on its right edge — full height like the tone,
                // so the sidebar column reads as its own surface.
                // The tone carries the window's left corners when the CSD
                // window floats — with one caveat: a corner radius is
                // clamped to the element's own size, and the COLLAPSED
                // sidebar is a ~1px border sliver (the grab affordance).
                // macOS trims that hairline with the window server's native
                // corner clip; we reproduce the same trim by insetting the
                // sliver vertically to where the curve begins, so its tips
                // never float over the transparent corner cutouts.
                let window_corner = Self::window_corner_radius(window);
                let sidebar_tone = div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left_0()
                    .w(px(sidebar_now))
                    .when(window_corner > 0.0, |el| {
                        if sidebar_now >= 2.0 * window_corner {
                            el.rounded_tl(px(window_corner))
                                .rounded_bl(px(window_corner))
                        } else {
                            el.top(px(window_corner)).bottom(px(window_corner))
                        }
                    })
                    .bg(crate::theme::wash(0.05))
                    .border_r_1()
                    .border_color(border_color);
                // The content row spans the FULL window height — the titlebar
                // overlays it (glass, no fill), so the transcript can scroll
                // under the header and fade out at its edge. Columns that
                // must NOT underlap (sidebar content, the changes panel,
                // settings) pad themselves down by the titlebar height.
                let page = div()
                    .size_full()
                    .relative()
                    .child(
                        div()
                            .size_full()
                            .flex()
                            .flex_row()
                            .child(sidebar)
                            .child(sidebar_seam)
                            .child(card)
                            .child(
                                div()
                                    .h_full()
                                    .flex_none()
                                    .relative()
                                    .child(right)
                                    .child(right_seam),
                            ),
                    )
                    .child(div().absolute().top_0().left_0().right_0().child(title_bar))
                    .child(self.render_titlebar_cluster(cx))
                    .children(overlays);
                root.child(sidebar_tone)
                    .child(motion::fade_in("phase-app", page))
            }
            GatePhase::Loading => root, // splash overlay covers boot
            GatePhase::OrgGate => {
                let card = self.render_org_gate(cx);
                root.child(card)
            }
            phase @ (GatePhase::Failed(_) | GatePhase::SignIn) => {
                let card = self.render_gate_card(phase, cx);
                root.child(card)
            }
        };
        let root = if restart_required {
            let restart = self.render_signed_out_restart(cx);
            root.child(restart)
        } else {
            root
        };

        // A manually-driven tween is mid-flight: keep frames coming (the same
        // scheduling `with_animation` would have requested). Hover color fades
        // ride the same clock; their once-per-frame tick lives here (this is
        // the window's root render — it runs exactly once per frame).
        if self.motion_active.get() | motion::hover_fades_active() {
            window.request_animation_frame();
        }

        // Boot splash overlay: visible → crossfades out on Ready → removed.
        let root = match self.splash {
            SplashPhase::Visible => {
                let theme = Theme::of(cx).clone();
                root.child(loaders::splash_overlay(&theme, false, cx.entity_id(), cx))
            }
            SplashPhase::FadingOut => {
                let theme = Theme::of(cx).clone();
                root.child(loaders::splash_overlay(&theme, true, cx.entity_id(), cx))
            }
            SplashPhase::Gone => root,
        };

        // Caption controls are shell-level chrome, not Ready-page content:
        // keep them above the splash and every auth/org/error gate as well as
        // the full application. Gate pages also need a drag surface because
        // they do not render the unified tabs/settings titlebar — on Windows
        // the native `Drag` control area, on Linux the explicit
        // `start_window_move` strip (the control-area hit-test is inert
        // there); macOS drags gate windows natively.
        let root = if (!restart_required && matches!(gate, GatePhase::Ready))
            || cfg!(target_os = "macos")
        {
            root
        } else {
            root.child(
                self.titlebar_drag_region(
                    "gate-titlebar-drag",
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(Theme::TITLEBAR_HEIGHT)),
                    cx,
                ),
            )
        };
        let root = root
            .children(self.render_windows_caption_controls(window, cx))
            .children(self.render_linux_caption_controls(window, cx))
            // Last so the invisible CSD resize strips sit above every other
            // element at the window edges.
            .children(Self::render_linux_resize_borders(window));
        self.render_time = None;
        root
    }
}

#[cfg(test)]
mod tests;
