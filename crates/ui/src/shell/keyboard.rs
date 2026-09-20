//! Keyboard events, shortcuts, and keymap configuration for Shell.

use chrono::Utc;
use gpui::{actions, Action, App, Context, Entity, Keystroke, Window};
pub use gpui::KeyBinding;

use crate::changes::Changes;
use crate::settings::{
    platform_combo, ComposerSendBehavior, KeymapConfig, ShortcutId, JUMP_SLOTS,
};
use crate::state::Indicator;
use crate::terminal::panel::ToggleTerminal;

use super::{resolve_shell_escape, RightSurface, Shell, ShellEscapeOutcome};

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

impl Shell {
    pub(super) fn active_changes(&self, cx: &App) -> Option<Entity<Changes>> {
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
    pub(super) fn capture_escape_surface(&mut self, cx: &mut Context<Self>) -> bool {
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

    pub(super) fn on_key_down_capture(
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

    pub(super) fn on_key_down(
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
}
