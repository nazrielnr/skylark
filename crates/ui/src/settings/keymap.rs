//! Keyboard shortcut definitions, normalization, and formatting.

use super::*;

// ---------------------------------------------------------------------------
// Keymap (customizable shortcuts, §1.4)
// ---------------------------------------------------------------------------

/// How many sidebar rows the jump shortcuts reach (t3code's
/// `THREAD_JUMP_KEYBINDING_COMMANDS`, nine slots).
pub const JUMP_SLOTS: usize = 9;

/// Default combo per jump slot, and the label the shortcuts table shows.
const JUMP_DEFAULTS: [&str; JUMP_SLOTS] = [
    "mod-1", "mod-2", "mod-3", "mod-4", "mod-5", "mod-6", "mod-7", "mod-8", "mod-9",
];
const JUMP_LABELS: [&str; JUMP_SLOTS] = [
    "Jump to session 1",
    "Jump to session 2",
    "Jump to session 3",
    "Jump to session 4",
    "Jump to session 5",
    "Jump to session 6",
    "Jump to session 7",
    "Jump to session 8",
    "Jump to session 9",
];

/// The rebindable app shortcuts. `JumpSession(slot)` is zero-based; a slot at
/// or past [`JUMP_SLOTS`] has no combo and no label, so it reads as unbound
/// rather than panicking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShortcutId {
    CaptureAppshot,
    SaveFile,
    BrowserReload,
    ToggleSidebar,
    ToggleChanges,
    ToggleTerminal,
    NewSession,
    NewProject,
    OpenModelPicker,
    NextSession,
    PrevSession,
    ArchiveSession,
    JumpSession(usize),
}

impl ShortcutId {
    pub const ALL: [ShortcutId; 12 + JUMP_SLOTS] = [
        ShortcutId::CaptureAppshot,
        ShortcutId::SaveFile,
        ShortcutId::BrowserReload,
        ShortcutId::ToggleSidebar,
        ShortcutId::ToggleChanges,
        ShortcutId::ToggleTerminal,
        ShortcutId::NewSession,
        ShortcutId::NewProject,
        ShortcutId::OpenModelPicker,
        ShortcutId::NextSession,
        ShortcutId::PrevSession,
        ShortcutId::ArchiveSession,
        ShortcutId::JumpSession(0),
        ShortcutId::JumpSession(1),
        ShortcutId::JumpSession(2),
        ShortcutId::JumpSession(3),
        ShortcutId::JumpSession(4),
        ShortcutId::JumpSession(5),
        ShortcutId::JumpSession(6),
        ShortcutId::JumpSession(7),
        ShortcutId::JumpSession(8),
    ];

    pub fn available(self) -> bool {
        self != Self::CaptureAppshot || crate::appshots::is_desktop()
    }

    /// Row label (zeron lib/shortcuts.ts `SHORTCUT_DEFINITIONS`, verbatim).
    pub fn label(self) -> &'static str {
        match self {
            ShortcutId::CaptureAppshot => "Capture Appshot",
            ShortcutId::SaveFile => "Save file",
            ShortcutId::BrowserReload => "Reload browser page",
            ShortcutId::ToggleSidebar => "Toggle left sidebar",
            ShortcutId::ToggleChanges => "Toggle right sidebar",
            ShortcutId::ToggleTerminal => "Toggle terminal",
            ShortcutId::NewSession => "New session",
            ShortcutId::NewProject => "New project",
            ShortcutId::OpenModelPicker => "Open model picker",
            ShortcutId::NextSession => "Next session",
            ShortcutId::PrevSession => "Previous session",
            ShortcutId::ArchiveSession => "Archive session",
            ShortcutId::JumpSession(slot) => JUMP_LABELS.get(slot).copied().unwrap_or(""),
        }
    }

    pub fn default_combo(self) -> &'static str {
        self.default_combo_on(cfg!(target_os = "macos"))
    }

    /// `default_combo` for an explicit platform, so the spelling invariant is
    /// testable for both from any machine (see the tests below — the mismatch
    /// this guards against only exists off macOS).
    pub fn default_combo_on(self, mac: bool) -> &'static str {
        match self {
            ShortcutId::CaptureAppshot if mac => "ctrl-alt-space",
            ShortcutId::CaptureAppshot => "mod-alt-space",
            ShortcutId::SaveFile => "mod-s",
            ShortcutId::BrowserReload => "mod-shift-r",
            ShortcutId::ToggleSidebar => "mod-b",
            ShortcutId::ToggleChanges => "mod-r",
            ShortcutId::ToggleTerminal => "mod-j",
            ShortcutId::NewSession => "mod-n",
            ShortcutId::NewProject => "mod-shift-n",
            ShortcutId::OpenModelPicker => "mod-/",
            // Ctrl+Tab on every platform — but spelled the way THAT platform's
            // recorder spells ctrl (see `combo_from_keystroke`). Off macOS
            // ctrl IS the primary and stores as "mod"; on macOS it is its own
            // modifier, and "mod" would mean Cmd+Tab, which the OS app
            // switcher eats.
            //
            // Off macOS "ctrl-tab" and "mod-tab" resolve to the same keystroke
            // through `platform_combo`, but conflict detection compares the
            // STORED spelling — so a default the recorder cannot reproduce
            // would let a rebind onto that same physical key pass as
            // conflict-free, bind twice, and silently kill one shortcut.
            ShortcutId::NextSession if mac => "ctrl-tab",
            ShortcutId::NextSession => "mod-tab",
            ShortcutId::PrevSession if mac => "ctrl-shift-tab",
            ShortcutId::PrevSession => "mod-shift-tab",
            // Mod+A is the composer's Select all, so archiving takes the
            // shifted combo.
            ShortcutId::ArchiveSession => "mod-shift-a",
            ShortcutId::JumpSession(slot) => JUMP_DEFAULTS.get(slot).copied().unwrap_or(""),
        }
    }

    /// The sidebar row this id jumps to, if it is a jump shortcut.
    pub fn jump_slot(self) -> Option<usize> {
        match self {
            ShortcutId::JumpSession(slot) if slot < JUMP_SLOTS => Some(slot),
            _ => None,
        }
    }
}

/// Persisted shortcut combos. Stored platform-neutral ("mod-s"); translated to
/// "cmd-s"/"ctrl-s" at bind time by [`platform_combo`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct KeymapConfig {
    #[cfg_attr(not(any(target_os = "macos", target_os = "linux")), serde(skip))]
    pub capture_appshot: String,
    pub save_file: String,
    pub browser_reload: String,
    pub toggle_sidebar: String,
    pub toggle_changes: String,
    pub toggle_terminal: String,
    pub new_session: String,
    pub new_project: String,
    pub open_model_picker: String,
    pub next_session: String,
    pub prev_session: String,
    pub archive_session: String,
    /// One combo per jump slot, in slot order. A list rather than nine fields:
    /// [`UiSettings::load`] discards the WHOLE file on a parse error, so a
    /// fixed-length array would let one malformed entry reset every unrelated
    /// setting. [`Self::healed`] restores the length instead.
    pub jump_session: Vec<String>,
}

/// Stable key for device-local preferences that belong to one workspace
/// profile. Authentication may arrive after `EngineInfo`, so callers must
/// treat `None` as "identity not ready" and avoid destructive cleanup.
pub fn sidebar_pin_profile_key(
    scope: Option<WorkspaceScope>,
    auth: Option<&AuthState>,
    development_org_id: Option<&str>,
) -> Option<String> {
    match scope? {
        WorkspaceScope::Local => Some("local".to_string()),
        WorkspaceScope::Synced => {
            let AuthState::SignedIn {
                user,
                org_id: Some(org_id),
            } = auth?
            else {
                return None;
            };
            Some(format!("synced:{org_id}:{}", user.id))
        }
        WorkspaceScope::Development => {
            let AuthState::SignedIn { user, .. } = auth? else {
                return None;
            };
            let (user_id, token_org_id) = user
                .id
                .split_once('@')
                .map_or((user.id.as_str(), None), |(user_id, org_id)| {
                    (user_id, (!org_id.is_empty()).then_some(org_id))
                });
            if user_id.is_empty() {
                return None;
            }
            let org_id = token_org_id
                .or(development_org_id.filter(|org_id| !org_id.is_empty()))
                .unwrap_or(zeron_engine::DEFAULT_ORG_ID);
            Some(format!("development:{org_id}:{user_id}"))
        }
    }
}

impl Default for KeymapConfig {
    fn default() -> Self {
        Self {
            capture_appshot: ShortcutId::CaptureAppshot.default_combo().into(),
            save_file: ShortcutId::SaveFile.default_combo().into(),
            browser_reload: ShortcutId::BrowserReload.default_combo().into(),
            toggle_sidebar: ShortcutId::ToggleSidebar.default_combo().into(),
            toggle_changes: ShortcutId::ToggleChanges.default_combo().into(),
            toggle_terminal: ShortcutId::ToggleTerminal.default_combo().into(),
            new_session: ShortcutId::NewSession.default_combo().into(),
            new_project: ShortcutId::NewProject.default_combo().into(),
            open_model_picker: ShortcutId::OpenModelPicker.default_combo().into(),
            next_session: ShortcutId::NextSession.default_combo().into(),
            prev_session: ShortcutId::PrevSession.default_combo().into(),
            archive_session: ShortcutId::ArchiveSession.default_combo().into(),
            jump_session: JUMP_DEFAULTS.iter().map(|c| (*c).to_string()).collect(),
        }
    }
}

impl KeymapConfig {
    pub fn get(&self, id: ShortcutId) -> &str {
        match id {
            ShortcutId::CaptureAppshot => &self.capture_appshot,
            ShortcutId::SaveFile => &self.save_file,
            ShortcutId::BrowserReload => &self.browser_reload,
            ShortcutId::ToggleSidebar => &self.toggle_sidebar,
            ShortcutId::ToggleChanges => &self.toggle_changes,
            ShortcutId::ToggleTerminal => &self.toggle_terminal,
            ShortcutId::NewSession => &self.new_session,
            ShortcutId::NewProject => &self.new_project,
            ShortcutId::OpenModelPicker => &self.open_model_picker,
            ShortcutId::NextSession => &self.next_session,
            ShortcutId::PrevSession => &self.prev_session,
            ShortcutId::ArchiveSession => &self.archive_session,
            ShortcutId::JumpSession(slot) => self
                .jump_session
                .get(slot)
                .map(String::as_str)
                .unwrap_or(""),
        }
    }

    pub fn set(&mut self, id: ShortcutId, combo: String) {
        match id {
            ShortcutId::CaptureAppshot => self.capture_appshot = combo,
            ShortcutId::SaveFile => self.save_file = combo,
            ShortcutId::BrowserReload => self.browser_reload = combo,
            ShortcutId::ToggleSidebar => self.toggle_sidebar = combo,
            ShortcutId::ToggleChanges => self.toggle_changes = combo,
            ShortcutId::ToggleTerminal => self.toggle_terminal = combo,
            ShortcutId::NewSession => self.new_session = combo,
            ShortcutId::NewProject => self.new_project = combo,
            ShortcutId::OpenModelPicker => self.open_model_picker = combo,
            ShortcutId::NextSession => self.next_session = combo,
            ShortcutId::PrevSession => self.prev_session = combo,
            ShortcutId::ArchiveSession => self.archive_session = combo,
            ShortcutId::JumpSession(slot) => {
                if slot < JUMP_SLOTS {
                    if self.jump_session.len() < JUMP_SLOTS {
                        self.heal_jump_slots();
                    }
                    self.jump_session[slot] = combo;
                }
            }
        }
    }

    pub fn reset(&mut self, id: ShortcutId) {
        self.set(id, id.default_combo().to_string());
    }

    /// Restore the jump list to exactly [`JUMP_SLOTS`] entries: a hand-edited
    /// or older file may carry a short, long or absent list. Surviving entries
    /// keep their slot; missing ones take the default.
    pub fn heal_jump_slots(&mut self) {
        self.jump_session.truncate(JUMP_SLOTS);
        while self.jump_session.len() < JUMP_SLOTS {
            self.jump_session
                .push(JUMP_DEFAULTS[self.jump_session.len()].to_string());
        }
    }

    /// Cmd/Ctrl+Enter belongs to the composer on every send mode. Older
    /// settings could assign it to an app shortcut while plain Enter was the
    /// configured sender; restore only those newly-conflicting rows to their
    /// defaults and preserve every unrelated customization.
    pub(crate) fn heal_reserved_composer_shortcuts(&mut self) {
        for id in ShortcutId::ALL {
            if self.get(id) == "mod-enter" {
                self.reset(id);
            }
        }
    }
}

/// Build a combo string from a recorded keystroke. The primary modifier
/// (cmd on macOS, ctrl elsewhere) becomes "mod"; bare modifier presses record
/// nothing.
pub fn combo_from_keystroke(
    ctrl: bool,
    alt: bool,
    shift: bool,
    cmd: bool,
    key: &str,
) -> Option<String> {
    combo_from_keystroke_on(cfg!(target_os = "macos"), ctrl, alt, shift, cmd, key)
}

/// [`combo_from_keystroke`] for an explicit platform — the ctrl spelling is
/// platform-dependent, so both paths need to be exercisable from one machine.
pub fn combo_from_keystroke_on(
    mac: bool,
    ctrl: bool,
    alt: bool,
    shift: bool,
    cmd: bool,
    key: &str,
) -> Option<String> {
    let key = key.trim().to_lowercase();
    if key.is_empty()
        || matches!(
            key.as_str(),
            "ctrl" | "control" | "alt" | "shift" | "cmd" | "platform" | "fn"
        )
    {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    // On macOS ctrl stays its own modifier rather than folding into "mod":
    // re-recording Ctrl+Tab as Cmd+Tab would hand the combo to the OS app
    // switcher, which never delivers it to the window.
    let ctrl_is_primary = ctrl && !mac;
    if cmd || ctrl_is_primary {
        parts.push("mod");
    }
    if ctrl && !ctrl_is_primary {
        parts.push("ctrl");
    }
    if alt {
        parts.push("alt");
    }
    if shift {
        parts.push("shift");
    }
    parts.push(&key);
    Some(parts.join("-"))
}

/// Shortcut ids whose combos collide with another shortcut (conflict detection).
pub fn conflicted_shortcuts(keymap: &KeymapConfig) -> Vec<ShortcutId> {
    ShortcutId::ALL
        .into_iter()
        .filter(|&id| {
            let combo = keymap.get(id);
            id.available()
                && !combo.is_empty()
                && ShortcutId::ALL
                    .into_iter()
                    .any(|other| other.available() && other != id && keymap.get(other) == combo)
        })
        .collect()
}

/// The modifiers a stored combo carries, as `(mod, alt, shift)`. Everything
/// before the final segment is a modifier; the final segment is the key.
pub fn combo_modifiers(combo: &str) -> (bool, bool, bool) {
    let mut parts: Vec<&str> = combo.split('-').collect();
    parts.pop();
    (
        parts.contains(&"mod"),
        parts.contains(&"alt"),
        parts.contains(&"shift"),
    )
}

/// Whether the sidebar should show its jump hints for the currently held
/// modifiers (t3code `shouldShowThreadJumpHintsForModifiers`). The held set
/// must match a jump combo EXACTLY, so adding Shift or Alt hides the hints and
/// a chord like Cmd+Shift+4 never flashes the overlay. `primary` is the held
/// "mod" key — cmd on macOS, ctrl elsewhere.
///
/// A jump combo with no modifiers at all never shows hints: it would otherwise
/// match the resting state and pin the overlay open. Pure.
pub fn jump_hints_visible(keymap: &KeymapConfig, primary: bool, alt: bool, shift: bool) -> bool {
    if !(primary || alt || shift) {
        return false;
    }
    ShortcutId::ALL
        .into_iter()
        .filter(|id| id.jump_slot().is_some())
        .any(|id| combo_modifiers(keymap.get(id)) == (primary, alt, shift))
}

/// Cmd on macOS and Ctrl elsewhere reveals the composer's modified-submit
/// hint only while that modifier is held by itself.
pub fn modifier_send_hint_visible(primary: bool, alt: bool, shift: bool) -> bool {
    primary && !alt && !shift
}

/// Translate a stored combo into a bindable keystroke for this platform.
pub fn platform_combo(combo: &str) -> String {
    platform_combo_on(cfg!(target_os = "macos"), combo)
}

/// [`platform_combo`] for an explicit platform (see [`combo_from_keystroke_on`]).
pub fn platform_combo_on(mac: bool, combo: &str) -> String {
    let primary = if mac { "cmd" } else { "ctrl" };
    combo
        .split('-')
        .map(|part| if part == "mod" { primary } else { part })
        .collect::<Vec<_>>()
        .join("-")
}

/// Human-readable combo for the shortcuts table ("mod-s" → "Cmd+S"/"Ctrl+S").
pub fn display_combo(combo: &str) -> String {
    display_combo_on(cfg!(target_os = "macos"), combo)
}

/// [`display_combo`] for an explicit platform (see [`combo_from_keystroke_on`]).
pub fn display_combo_on(mac: bool, combo: &str) -> String {
    combo
        .split('-')
        .map(|part| match part {
            "mod" => if mac { "Cmd" } else { "Ctrl" }.to_string(),
            "alt" => if mac { "Opt" } else { "Alt" }.to_string(),
            "shift" => "Shift".to_string(),
            other => {
                let mut chars = other.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    None => String::new(),
                }
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

/// Compact combo for badge surfaces (the sidebar jump hints): macOS spells
/// the modifiers as their key glyphs in canonical ⌃⌥⇧⌘ order and drops the
/// separators ("⌘1", "⇧⌘A") — the form the model picker's ⌘N chips already
/// use — while other platforms keep the textual [`display_combo`] ("Ctrl+1").
pub fn badge_combo(combo: &str) -> String {
    badge_combo_on(cfg!(target_os = "macos"), combo)
}

/// [`badge_combo`] for an explicit platform (see [`combo_from_keystroke_on`]).
pub fn badge_combo_on(mac: bool, combo: &str) -> String {
    if !mac {
        return display_combo_on(false, combo);
    }
    let mut parts: Vec<&str> = combo.split('-').collect();
    let key = parts.pop().unwrap_or("");
    let mut out = String::new();
    for glyph in ["ctrl", "alt", "shift", "mod"]
        .iter()
        .zip(['⌃', '⌥', '⇧', '⌘'])
        .filter_map(|(name, glyph)| parts.contains(name).then_some(glyph))
    {
        out.push(glyph);
    }
    let mut chars = key.chars();
    if let Some(first) = chars.next() {
        out.extend(first.to_uppercase());
        out.push_str(chars.as_str());
    }
    out
}
