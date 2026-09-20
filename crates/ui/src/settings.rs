//! UI settings persisted to a small JSON file in the data dir — pane widths and
//! collapse flags (zeron persisted the same set in localStorage).
//!
//! Loaded once at boot and then owned by [`SettingsStore`], the only production
//! writer. Frequent geometry changes are debounced; durable choices flush
//! immediately through that same writer. Corrupt or missing files fall back to
//! defaults, and loaded values are clamped so a hand-edited file can't wedge the
//! layout.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{App, Global, Task};
use serde::{Deserialize, Serialize};
use zeron_proto::{AuthState, WorkspaceScope};

pub mod accounts;
pub mod appearance;
pub mod archived;
pub mod composer;
pub mod devices;
pub mod files;
pub mod harnesses;
pub mod notifications;
pub mod shortcuts;
pub mod widgets;

/// Sidebar drag-resize bounds (px).
pub const SIDEBAR_MIN: f32 = 224.0;
pub const SIDEBAR_MAX: f32 = 400.0;
pub const SIDEBAR_DEFAULT: f32 = 256.0;

/// Right ("Changes") pane drag-resize floor and default (px). Its runtime
/// maximum is the window space remaining after the left sidebar and the
/// conversation's [`CHAT_PANEL_MIN`] reservation.
pub const RIGHT_PANE_MIN: f32 = 360.0;
pub const RIGHT_PANE_DEFAULT: f32 = 520.0;
/// Minimum width retained for the conversation when the right pane is open.
pub const CHAT_PANEL_MIN: f32 = 300.0;

/// Terminal panel height bounds: 160px … 55% of the viewport (§1.10). The
/// viewport-relative cap applies at runtime; the absolute cap here only heals
/// hand-edited files.
pub const TERMINAL_MIN_HEIGHT: f32 = 160.0;
pub const TERMINAL_MAX_VH: f32 = 0.55;
pub const TERMINAL_ABS_MAX_HEIGHT: f32 = 2000.0;
pub const TERMINAL_DEFAULT_HEIGHT: f32 = 280.0;

/// Debounce for settings writes after a drag/toggle.
pub const SAVE_DEBOUNCE_MS: u64 = 400;

pub const FILES_AUTOSAVE_DELAY_DEFAULT_MS: u64 = 900;
pub const FILES_AUTOSAVE_DELAY_MIN_MS: u64 = 100;
pub const FILES_AUTOSAVE_DELAY_MAX_MS: u64 = 10_000;

const FILE_NAME: &str = "ui-settings.json";
const NEW_THREAD_BACKGROUND_DIR: &str = "new-thread-backgrounds";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewThreadComposerBackground {
    /// Managed copy inside Zeron's device-local data directory.
    pub path: String,
    /// Original file name shown in Appearance settings.
    pub name: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NewThreadBackgroundEffect {
    #[default]
    None,
    Dither,
    Ascii,
    Halftone,
    Scanlines,
}

impl NewThreadBackgroundEffect {
    pub const ALL: [Self; 5] = [
        Self::None,
        Self::Dither,
        Self::Ascii,
        Self::Halftone,
        Self::Scanlines,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Dither => "Dither",
            Self::Ascii => "ASCII",
            Self::Halftone => "Halftone",
            Self::Scanlines => "Scanlines",
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::None => "Shows the original artwork.",
            Self::Dither => "Rebuilds the artwork with a dithered color palette.",
            Self::Ascii => "Recreates the artwork with colored characters on black.",
            Self::Halftone => "Recreates the artwork with colored print dots on black.",
            Self::Scanlines => "Adds a pronounced horizontal display-line texture.",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GitHistoryColumns {
    pub author: bool,
    pub date: bool,
    pub sha: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GitHistoryColumn {
    Author,
    Date,
    Sha,
}

/// Stable order for the optional Git History columns. Hidden columns remain in
/// the sequence so showing one again restores the position chosen by the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GitHistoryColumnOrder(pub Vec<GitHistoryColumn>);

impl GitHistoryColumnOrder {
    pub fn normalized(mut self) -> Self {
        let mut columns = Vec::with_capacity(3);
        for column in self.0.drain(..) {
            if !columns.contains(&column) {
                columns.push(column);
            }
        }
        for column in [
            GitHistoryColumn::Author,
            GitHistoryColumn::Date,
            GitHistoryColumn::Sha,
        ] {
            if !columns.contains(&column) {
                columns.push(column);
            }
        }
        Self(columns)
    }
}

impl Default for GitHistoryColumnOrder {
    fn default() -> Self {
        Self(vec![
            GitHistoryColumn::Author,
            GitHistoryColumn::Date,
            GitHistoryColumn::Sha,
        ])
    }
}

/// Persisted widths for the fixed Git History data columns. The commit column
/// remains elastic and occupies the space left after these columns and the
/// topology graph, so resizing its first divider adjusts the adjacent column.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GitHistoryColumnWidths {
    pub author: f32,
    pub date: f32,
    pub sha: f32,
}

impl GitHistoryColumnWidths {
    pub const AUTHOR_MIN: f32 = 44.0;
    pub const AUTHOR_MAX: f32 = 220.0;
    pub const DATE_MIN: f32 = 68.0;
    pub const DATE_MAX: f32 = 180.0;
    pub const SHA_MIN: f32 = 58.0;
    pub const SHA_MAX: f32 = 140.0;

    pub fn clamped(mut self) -> Self {
        let defaults = Self::default();
        self.author = clamp_or(
            self.author,
            Self::AUTHOR_MIN,
            Self::AUTHOR_MAX,
            defaults.author,
        );
        self.date = clamp_or(self.date, Self::DATE_MIN, Self::DATE_MAX, defaults.date);
        self.sha = clamp_or(self.sha, Self::SHA_MIN, Self::SHA_MAX, defaults.sha);
        self
    }
}

impl Default for GitHistoryColumnWidths {
    fn default() -> Self {
        Self {
            author: 88.0,
            date: 88.0,
            sha: 74.0,
        }
    }
}

pub const TRANSCRIPT_WIDTH_MIN: f32 = 560.0;
pub const TRANSCRIPT_WIDTH_MAX: f32 = 1200.0;
pub const TRANSCRIPT_WIDTH_DEFAULT: f32 = 736.0;
pub const TRANSCRIPT_WIDTH_STEP: f32 = 16.0;

pub fn normalize_transcript_width(width: f32) -> f32 {
    let width = clamp_or(
        width,
        TRANSCRIPT_WIDTH_MIN,
        TRANSCRIPT_WIDTH_MAX,
        TRANSCRIPT_WIDTH_DEFAULT,
    );
    TRANSCRIPT_WIDTH_MIN
        + ((width - TRANSCRIPT_WIDTH_MIN) / TRANSCRIPT_WIDTH_STEP).round() * TRANSCRIPT_WIDTH_STEP
}

pub fn transcript_width(cx: &App) -> f32 {
    cx.try_global::<SettingsStore>()
        .map(|store| store.current.transcript_width)
        .unwrap_or(TRANSCRIPT_WIDTH_DEFAULT)
}

pub fn set_transcript_width(width: f32, cx: &mut App) {
    if update(SavePolicy::Debounced, cx, |settings| {
        settings.transcript_width = normalize_transcript_width(width);
    }) {
        cx.refresh_windows();
    }
}

/// Whether a settings mutation should wait for the normal coalescing window or
/// reach disk before returning to the event loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SavePolicy {
    Debounced,
    Immediate,
}

/// The sole in-process owner and writer of `ui-settings.json`.
///
/// Mutations land in `current` before any timer starts. Replacing a pending
/// task cancels its stale snapshot, and immediate mutations cancel the timer
/// before flushing synchronously.
pub struct SettingsStore {
    current: UiSettings,
    data_dir: PathBuf,
    revision: u64,
    saved_revision: u64,
    /// In-process invalidation token for transcript code-fence layout. Unlike
    /// the persisted revision, this advances only when the global Fit choice
    /// changes, including a change back to its previous value.
    code_fences_generation: u64,
    save_task: Option<Task<()>>,
}

impl Global for SettingsStore {}

impl SettingsStore {
    fn snapshot(&self) -> (UiSettings, u64) {
        (self.current.clone(), self.revision)
    }

    fn mark_saved(&mut self, revision: u64) -> bool {
        self.saved_revision = self.saved_revision.max(revision);
        self.saved_revision == self.revision
    }

    fn update_current(&mut self, mutate: impl FnOnce(&mut UiSettings)) -> bool {
        let before = self.current.clone();
        mutate(&mut self.current);
        self.current = self.current.clone().clamped();
        if self.current == before {
            return false;
        }
        if self.current.code_fences_fit_content != before.code_fences_fit_content {
            self.code_fences_generation = self.code_fences_generation.wrapping_add(1);
        }
        self.revision = self.revision.wrapping_add(1);
        true
    }
}

pub fn init(settings: UiSettings, data_dir: impl Into<PathBuf>, cx: &mut App) {
    cx.set_global(SettingsStore {
        current: settings,
        data_dir: data_dir.into(),
        revision: 0,
        saved_revision: 0,
        code_fences_generation: 0,
        save_task: None,
    });
}

/// Latest settings, including mutations still inside the debounce window.
pub fn current(cx: &App) -> UiSettings {
    cx.try_global::<SettingsStore>()
        .map(|store| store.current.clone())
        .unwrap_or_default()
}

/// Copy a selected image into Zeron's device-local data directory and make it
/// the new-thread canvas background. A unique file name avoids stale image
/// caches when the background is replaced.
pub fn install_new_thread_composer_background(source: &Path, cx: &mut App) -> Result<(), String> {
    let staged = crate::attachments::stage_file(source)?;
    // Do not persist the candidate or retire the old managed file until the
    // renderer's decoder has accepted the exact bytes we are about to save.
    crate::new_thread_background_image::decode(staged.bytes()).map_err(|_| {
        "This background image is unsupported or damaged. Choose a valid image such as PNG or JPEG.".to_string()
    })?;
    let data_dir = cx
        .try_global::<SettingsStore>()
        .map(|store| store.data_dir.clone())
        .ok_or_else(|| "Unable to save the image. Restart Zeron and try again.".to_string())?;
    let backgrounds_dir = data_dir.join(NEW_THREAD_BACKGROUND_DIR);
    std::fs::create_dir_all(&backgrounds_dir).map_err(|_| {
        "Unable to save the image. Check folder permissions and try again.".to_string()
    })?;

    let extension = Path::new(&staged.name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("png");
    let destination = backgrounds_dir.join(format!(
        "new-thread-background-{}.{}",
        uuid::Uuid::new_v4(),
        extension
    ));
    let temporary = destination.with_extension(format!("{extension}.tmp"));
    if std::fs::write(&temporary, staged.bytes())
        .and_then(|_| std::fs::rename(&temporary, &destination))
        .is_err()
    {
        let _ = std::fs::remove_file(&temporary);
        return Err(
            "Unable to save the image. Check folder permissions and try again.".to_string(),
        );
    }

    let replacement = NewThreadComposerBackground {
        path: destination.to_string_lossy().into_owned(),
        name: staged.name,
    };
    let mut next = current(cx);
    let previous = next
        .new_thread_composer_background
        .replace(replacement.clone());
    // Persist the pointer before retiring the old file. `update(Immediate)`
    // updates memory first and only logs an I/O failure; for a file-backed
    // setting that order can leave disk pointing at an image we just deleted.
    if next.save(&data_dir).is_err() {
        let _ = std::fs::remove_file(&destination);
        return Err(
            "Unable to save the image. Check folder permissions and try again.".to_string(),
        );
    }
    replace(next, SavePolicy::Immediate, cx);
    remove_managed_new_thread_background(previous.as_ref(), &backgrounds_dir);
    cx.refresh_windows();
    Ok(())
}

pub fn remove_new_thread_composer_background(cx: &mut App) -> Result<(), String> {
    let data_dir = cx
        .try_global::<SettingsStore>()
        .map(|store| store.data_dir.clone())
        .ok_or_else(|| "Unable to remove the image. Restart Zeron and try again.".to_string())?;
    let mut next = current(cx);
    let previous = next.new_thread_composer_background.take();
    if previous.is_none() {
        return Ok(());
    }
    if next.save(&data_dir).is_err() {
        return Err(
            "Unable to remove the image. Check folder permissions and try again.".to_string(),
        );
    }
    replace(next, SavePolicy::Immediate, cx);
    remove_managed_new_thread_background(
        previous.as_ref(),
        &data_dir.join(NEW_THREAD_BACKGROUND_DIR),
    );
    cx.refresh_windows();
    Ok(())
}

pub fn set_new_thread_background_effect(effect: NewThreadBackgroundEffect, cx: &mut App) {
    if update(SavePolicy::Immediate, cx, |settings| {
        settings.new_thread_background_effect = effect;
    }) {
        cx.refresh_windows();
    }
}

fn remove_managed_new_thread_background(
    background: Option<&NewThreadComposerBackground>,
    backgrounds_dir: &Path,
) {
    let Some(background) = background else {
        return;
    };
    let path = Path::new(&background.path);
    // Never delete an arbitrary legacy or hand-edited path. Only files copied
    // directly into the directory owned by this setting are disposable.
    if path.parent() == Some(backgrounds_dir) {
        let _ = std::fs::remove_file(path);
    }
}

/// Monotonic id of the global code-fence layout choice. Every transcript
/// compares this during render so inactive subagent tabs can observe all mode
/// transitions when they next become visible.
pub fn code_fences_generation(cx: &App) -> u64 {
    cx.try_global::<SettingsStore>()
        .map(|store| store.code_fences_generation)
        .unwrap_or_default()
}

pub fn update(policy: SavePolicy, cx: &mut App, mutate: impl FnOnce(&mut UiSettings)) -> bool {
    if !cx.has_global::<SettingsStore>() {
        return false;
    }
    if !cx.global_mut::<SettingsStore>().update_current(mutate) {
        return false;
    }
    schedule(policy, cx);
    true
}

pub fn replace(settings: UiSettings, policy: SavePolicy, cx: &mut App) -> bool {
    update(policy, cx, |current| *current = settings)
}

fn schedule(policy: SavePolicy, cx: &mut App) {
    let old_task = cx.global_mut::<SettingsStore>().save_task.take();
    drop(old_task);

    match policy {
        SavePolicy::Immediate => flush(cx),
        SavePolicy::Debounced => {
            let task = cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(SAVE_DEBOUNCE_MS))
                    .await;
                cx.update(flush_latest);
            });
            cx.global_mut::<SettingsStore>().save_task = Some(task);
        }
    }
}

impl Default for GitHistoryColumns {
    fn default() -> Self {
        Self {
            author: true,
            date: true,
            sha: true,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GitHistoryAuthorDisplay {
    #[default]
    Avatar,
    Name,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ComposerSendBehavior {
    #[default]
    Enter,
    ModEnter,
}

/// Persist the latest revision. Safe to call at shutdown.
pub fn flush(cx: &mut App) {
    if !cx.has_global::<SettingsStore>() {
        return;
    }
    let pending = cx.global_mut::<SettingsStore>().save_task.take();
    drop(pending);
    flush_latest(cx);
}

fn flush_latest(cx: &mut App) {
    let Some(store) = cx.try_global::<SettingsStore>() else {
        return;
    };
    if store.saved_revision == store.revision {
        return;
    }
    let (settings, revision) = store.snapshot();
    let data_dir = store.data_dir.clone();
    match settings.save(&data_dir) {
        Ok(()) => {
            let current = cx.global_mut::<SettingsStore>().mark_saved(revision);
            debug_assert!(current, "foreground settings write cannot be overtaken");
        }
        Err(err) => tracing::warn!(error = %err, revision, "failed to persist ui settings"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum SidebarOrganization {
    ByProject,
    ByDevice,
    #[default]
    InOneList,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum SidebarSort {
    #[default]
    LastUpdated,
    Created,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowGeometry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_uuid: Option<uuid::Uuid>,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl WindowGeometry {
    pub fn is_valid(self) -> bool {
        [self.x, self.y, self.width, self.height]
            .into_iter()
            .all(f32::is_finite)
            && self.width > 0.0
            && self.height > 0.0
    }

    pub fn from_bounds(bounds: gpui::Bounds<gpui::Pixels>) -> Self {
        Self {
            display_uuid: None,
            x: bounds.origin.x.into(),
            y: bounds.origin.y.into(),
            width: bounds.size.width.into(),
            height: bounds.size.height.into(),
        }
    }

    pub fn restore(self, displays: &[Self], primary: usize) -> Option<(usize, Self)> {
        if !self.is_valid() {
            return None;
        }
        let matched = self.display_uuid.and_then(|uuid| {
            displays
                .iter()
                .position(|display| display.is_valid() && display.display_uuid == Some(uuid))
        });
        let index = matched
            .or_else(|| {
                displays
                    .get(primary)
                    .filter(|display| display.is_valid())
                    .map(|_| primary)
            })
            .or_else(|| displays.iter().position(|display| display.is_valid()))?;
        let display = displays[index];
        let mut geometry = self.fit(display);
        if self.display_uuid.is_some() && matched.is_none() {
            geometry.x = display.x + (display.width - geometry.width) / 2.0;
            geometry.y = display.y + (display.height - geometry.height) / 2.0;
        }
        geometry.display_uuid = display.display_uuid;
        Some((index, geometry))
    }

    pub fn fit(self, display: Self) -> Self {
        let width = self.width.max(900.0).min(display.width);
        let height = self.height.max(600.0).min(display.height);
        Self {
            display_uuid: self.display_uuid,
            x: self.x.clamp(display.x, display.x + display.width - width),
            y: self.y.clamp(display.y, display.y + display.height - height),
            width,
            height,
        }
    }

    pub fn bounds(self) -> gpui::Bounds<gpui::Pixels> {
        gpui::Bounds::new(
            gpui::point(gpui::px(self.x), gpui::px(self.y)),
            gpui::size(gpui::px(self.width), gpui::px(self.height)),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UiSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_geometry: Option<WindowGeometry>,
    /// Submit using Enter or the platform modifier plus Enter.
    pub composer_send_behavior: ComposerSendBehavior,
    pub sidebar_width: f32,
    pub sidebar_collapsed: bool,
    /// Legacy: the grouped-by-project toggle predates spaces (which group by
    /// folder inherently). Kept for file compatibility; no longer read.
    pub sidebar_grouped: bool,
    /// How active sessions are partitioned in the sidebar.
    pub sidebar_organization: SidebarOrganization,
    /// Timestamp used to order active sessions (newest first).
    pub sidebar_sort: SidebarSort,
    /// Optional harness branding and repository metadata shown below each
    /// session title.
    pub sidebar_show_project_label: bool,
    pub sidebar_compact: bool,
    pub sidebar_show_project_icon: bool,
    pub sidebar_show_harness: bool,
    pub sidebar_show_branch: bool,
    pub sidebar_show_pull_request: bool,
    /// The last selected space — restored on boot when the row still exists;
    /// also the new-tab default when the sidebar filter is "All".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_space_id: Option<String>,
    /// Last successfully launched Action per project in this viewport.
    #[serde(skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub last_project_action_by_space_id: std::collections::HashMap<String, String>,
    /// Open session tabs in visual order (drag-reorder edits in place).
    /// Device-local: a tab is a local viewport onto the synced session list —
    /// closing one never archives the session. Ids of archived/deleted chats
    /// are pruned against the doc ([`Shell::sync_open_tabs`]). `None` = file
    /// written by a pre-tabs build; seeded once from the last space's sessions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_tabs: Option<Vec<String>>,
    /// Sidebar session filter: a space id, or `None` for "All spaces".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub space_filter: Option<String>,
    /// Device-local pins for local profiles; synced profiles use registry pins.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub sidebar_pinned_session_ids_by_profile: HashMap<String, Vec<String>>,
    /// Legacy: per-space tab order, from when tabs were the selected space's
    /// non-archived sessions. Kept for file compatibility; no longer read.
    #[serde(skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub tab_order: std::collections::HashMap<String, Vec<String>>,
    /// Legacy: manual sidebar space order, from when spaces were a sidebar
    /// list. Kept for file compatibility; no longer read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub space_order: Vec<String>,
    /// Master switch for session notification chimes. `ZERON_DISABLE_SOUND`
    /// overrides every per-event preference below.
    pub sound_enabled: bool,
    /// Chime when an agent run completes successfully.
    pub sound_completion_enabled: bool,
    /// Chime when an agent is waiting for user input.
    pub sound_input_enabled: bool,
    /// Chime when a run fails or the durable connection state degrades.
    pub sound_attention_enabled: bool,
    /// Desktop banner notifications on the same transitions.
    /// `ZERON_DISABLE_NOTIFICATIONS` overrides.
    pub notifications_enabled: bool,
    /// Suppress the banner while a Zeron window is focused (the chime covers
    /// the foreground case).
    pub notifications_background_only: bool,
    pub right_pane_width: f32,
    /// Legacy: panel *open* flags are session-scoped in-memory state now
    /// (`shell::SessionPanels`, zeron `sessionPanels` parity). Kept for file
    /// compatibility; no longer read or written by the shell.
    pub right_pane_open: bool,
    pub terminal_height: f32,
    /// Legacy — see [`Self::right_pane_open`].
    pub terminal_open: bool,
    /// Customizable shortcut combos (feature-inventory §1.4).
    pub keymap: KeymapConfig,
    /// macOS viewer-side Appshot capture. Device-local because the shortcut
    /// and TCC permissions belong to this desktop.
    #[cfg_attr(not(any(target_os = "macos", target_os = "linux")), serde(skip))]
    pub appshots_enabled: bool,
    #[cfg_attr(not(any(target_os = "macos", target_os = "linux")), serde(skip))]
    pub appshot_sound_enabled: bool,
    #[cfg_attr(not(any(target_os = "macos", target_os = "linux")), serde(skip))]
    pub appshot_destination: crate::appshots::AppshotDestination,
    /// Whether bare Escape stops the active agent after contextual consumers
    /// decline it. Device-local and opt-in.
    pub escape_stops_active_agent: bool,
    /// Light/dark preference. Defaults to following the OS.
    pub appearance: crate::appearance::AppearanceMode,
    /// Optional columns shown in every Git History pane.
    pub git_history_columns: GitHistoryColumns,
    /// User-adjusted widths for the resizable Git History columns.
    pub git_history_column_widths: GitHistoryColumnWidths,
    /// User-selected order for the optional Git History columns.
    pub git_history_column_order: GitHistoryColumnOrder,
    /// How authors are represented in Git History rows.
    pub git_history_author_display: GitHistoryAuthorDisplay,
    /// Interface and conversational-prose family. Device-local by design.
    pub ui_font_family: crate::typography::UiFontFamily,
    /// Base size for interface and conversational prose.
    pub ui_font_size: crate::typography::UiFontSize,
    /// Terminal family and absolute pixel size. Only fixed-width families
    /// qualify, including compatible Nerd Fonts.
    pub terminal_font_family: crate::typography::UiFontFamily,
    pub terminal_font_size: f32,
    /// Family and absolute pixel size for code, diffs, and file editors.
    pub code_font_family: crate::typography::UiFontFamily,
    pub code_font_size: f32,
    /// Independently selected light and dark theme variants.
    pub theme_selection: zeron_theme::ThemeSelection,
    /// Changes pane: side-by-side diffs instead of the unified stack.
    pub diff_split: bool,
    /// Changes pane: wrap long source lines instead of scrolling horizontally.
    pub diff_wrap: bool,
    /// Agent-sent Markdown fences: wrap long lines to the chat width instead
    /// of exposing their horizontal scroll plane.
    pub code_fences_fit_content: bool,
    /// Maximum conversation width in logical pixels; composer width is independent.
    pub transcript_width: f32,
    /// Open a normal web-link activation in the session Browser. Explicit
    /// context-menu actions remain available regardless of this preference.
    pub open_web_links_in_zeron: bool,
    /// Save edited workspace files automatically after the configured delay.
    pub files_autosave_enabled: bool,
    /// Idle time before an edited workspace file is saved automatically.
    pub files_autosave_delay_ms: u64,
    /// Wrap long lines in workspace file editors and previews.
    pub files_word_wrap: bool,
    /// Include hidden and ignored entries in workspace file trees.
    pub files_show_all: bool,
    /// Interactive identity overlay; imported themes default to their own accent.
    pub accent: zeron_theme::AccentSelection,
    /// Glass policy, independent from the selected appearance, theme, and accent.
    pub surface: zeron_theme::SurfacePreference,
    /// Optional device-local artwork behind the blank new-thread composer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_thread_composer_background: Option<NewThreadComposerBackground>,
    /// Non-destructive treatment composited inside the artwork's fade mask.
    pub new_thread_background_effect: NewThreadBackgroundEffect,
    /// Pre-theme settings used `accentColor`. Read it once, migrate to
    /// [`Self::accent`], and never write it again.
    #[serde(default, rename = "accentColor", skip_serializing)]
    legacy_accent_color: Option<crate::theme::AccentColor>,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            window_geometry: None,
            sidebar_width: SIDEBAR_DEFAULT,
            sidebar_collapsed: false,
            sidebar_grouped: false,
            sidebar_organization: SidebarOrganization::InOneList,
            sidebar_sort: SidebarSort::LastUpdated,
            sidebar_show_project_label: true,
            sidebar_compact: true,
            sidebar_show_project_icon: true,
            sidebar_show_harness: true,
            sidebar_show_branch: true,
            sidebar_show_pull_request: true,
            last_space_id: None,
            last_project_action_by_space_id: std::collections::HashMap::new(),
            open_tabs: None,
            space_filter: None,
            sidebar_pinned_session_ids_by_profile: HashMap::new(),
            tab_order: std::collections::HashMap::new(),
            space_order: Vec::new(),
            sound_enabled: true,
            sound_completion_enabled: true,
            sound_input_enabled: true,
            sound_attention_enabled: true,
            notifications_enabled: true,
            notifications_background_only: true,
            right_pane_width: RIGHT_PANE_DEFAULT,
            right_pane_open: false,
            terminal_height: TERMINAL_DEFAULT_HEIGHT,
            terminal_open: false,
            keymap: KeymapConfig::default(),
            escape_stops_active_agent: false,
            composer_send_behavior: ComposerSendBehavior::default(),
            appshots_enabled: false,
            appshot_sound_enabled: true,
            appshot_destination: crate::appshots::AppshotDestination::Automatic,
            appearance: crate::appearance::AppearanceMode::default(),
            git_history_columns: GitHistoryColumns::default(),
            git_history_column_widths: GitHistoryColumnWidths::default(),
            git_history_column_order: GitHistoryColumnOrder::default(),
            git_history_author_display: GitHistoryAuthorDisplay::default(),
            ui_font_family: crate::typography::UiFontFamily::default(),
            ui_font_size: crate::typography::UiFontSize::default(),
            terminal_font_family: crate::typography::UiFontFamily::GeistMono,
            terminal_font_size: crate::typography::TERMINAL_FONT_SIZE_DEFAULT,
            code_font_family: crate::typography::UiFontFamily::GeistMono,
            code_font_size: crate::typography::CODE_FONT_SIZE_DEFAULT,
            theme_selection: zeron_theme::ThemeSelection::default(),
            diff_split: false,
            diff_wrap: false,
            code_fences_fit_content: false,
            transcript_width: TRANSCRIPT_WIDTH_DEFAULT,
            open_web_links_in_zeron: true,
            files_autosave_enabled: false,
            files_autosave_delay_ms: FILES_AUTOSAVE_DELAY_DEFAULT_MS,
            files_word_wrap: false,
            files_show_all: false,
            accent: zeron_theme::AccentSelection::default(),
            surface: zeron_theme::SurfacePreference::default(),
            new_thread_composer_background: None,
            new_thread_background_effect: NewThreadBackgroundEffect::None,
            legacy_accent_color: None,
        }
    }
}

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
    fn heal_reserved_composer_shortcuts(&mut self) {
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

impl UiSettings {
    pub fn sidebar_pins(&self, profile_key: &str) -> &[String] {
        self.sidebar_pinned_session_ids_by_profile
            .get(profile_key)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    pub fn sidebar_pins_mut(&mut self, profile_key: String) -> &mut Vec<String> {
        self.sidebar_pinned_session_ids_by_profile
            .entry(profile_key)
            .or_default()
    }

    /// Whether this session event may produce audio. Appshot capture has its
    /// own feature-local preference once the Appshots contribution lands.
    pub fn session_sound_enabled(&self, sound: crate::sound::Sound) -> bool {
        self.sound_enabled
            && match sound {
                crate::sound::Sound::Done => self.sound_completion_enabled,
                crate::sound::Sound::Request => self.sound_input_enabled,
                crate::sound::Sound::Attention => self.sound_attention_enabled,
            }
    }

    /// Clamp widths into their legal ranges (also heals NaN to defaults).
    pub fn clamped(mut self) -> Self {
        self.transcript_width = normalize_transcript_width(self.transcript_width);
        self.window_geometry = self.window_geometry.filter(|geometry| geometry.is_valid());
        self.sidebar_width = clamp_or(
            self.sidebar_width,
            SIDEBAR_MIN,
            SIDEBAR_MAX,
            SIDEBAR_DEFAULT,
        );
        // The right pane has no persisted upper bound: its live drag clamps
        // against the current window, which is unavailable while loading.
        self.right_pane_width = min_or(self.right_pane_width, RIGHT_PANE_MIN, RIGHT_PANE_DEFAULT);
        self.terminal_height = clamp_or(
            self.terminal_height,
            TERMINAL_MIN_HEIGHT,
            TERMINAL_ABS_MAX_HEIGHT,
            TERMINAL_DEFAULT_HEIGHT,
        );
        self.files_autosave_delay_ms = self
            .files_autosave_delay_ms
            .clamp(FILES_AUTOSAVE_DELAY_MIN_MS, FILES_AUTOSAVE_DELAY_MAX_MS);
        self.terminal_font_size = clamp_or(
            self.terminal_font_size,
            crate::typography::FONT_SIZE_MIN,
            crate::typography::FONT_SIZE_MAX,
            crate::typography::TERMINAL_FONT_SIZE_DEFAULT,
        );
        self.code_font_size = clamp_or(
            self.code_font_size,
            crate::typography::FONT_SIZE_MIN,
            crate::typography::FONT_SIZE_MAX,
            crate::typography::CODE_FONT_SIZE_DEFAULT,
        );
        self.git_history_column_widths = self.git_history_column_widths.clamped();
        self.git_history_column_order = self.git_history_column_order.normalized();
        self.ui_font_size = self.ui_font_size.normalized();
        self.keymap.heal_jump_slots();
        self.keymap.heal_reserved_composer_shortcuts();
        self
    }

    /// Load from `{data_dir}/ui-settings.json`; defaults on any failure.
    pub fn load(data_dir: &Path) -> Self {
        match std::fs::read_to_string(Self::path(data_dir)) {
            Ok(text) => {
                match serde_json::from_str::<serde_json::Value>(&text).and_then(|mut value| {
                    if let Some(settings) = value.as_object_mut() {
                        let previous_sound = settings
                            .get("soundEnabled")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(true);
                        settings
                            .entry("appshotSoundEnabled")
                            .or_insert(serde_json::Value::Bool(previous_sound));
                        // The files-editor size was the first user-facing code
                        // size; it now drives every code surface.
                        if let Some(legacy) = settings.remove("filesEditorFontSize") {
                            settings.entry("codeFontSize").or_insert(legacy);
                        }
                    }
                    if let Some(keymap) = value
                        .get_mut("keymap")
                        .and_then(serde_json::Value::as_object_mut)
                        && !keymap.contains_key("saveFile")
                    {
                        // Reserve customized chords before assigning any new defaults.
                        // An older map may already use Cmd+S/B/R for another action.
                        let mut migrating =
                            vec![(ShortcutId::SaveFile, "saveFile", "", "mod-shift-s")];
                        for (id, field, old, fallback) in [
                            (
                                ShortcutId::ToggleSidebar,
                                "toggleSidebar",
                                "mod-s",
                                "mod-shift-b",
                            ),
                            (
                                ShortcutId::ToggleChanges,
                                "toggleChanges",
                                "mod-b",
                                "mod-shift-r",
                            ),
                        ] {
                            if keymap
                                .get(field)
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or(old)
                                == old
                            {
                                migrating.push((id, field, old, fallback));
                            }
                        }
                        for (_, field, _, _) in &migrating {
                            keymap.insert((*field).into(), serde_json::json!(""));
                        }
                        let mut resolved: KeymapConfig =
                            serde_json::from_value(serde_json::Value::Object(keymap.clone()))?;
                        for (id, field, old, fallback) in migrating {
                            let combo = [id.default_combo(), old, fallback]
                                .into_iter()
                                .find(|candidate| {
                                    !candidate.is_empty()
                                        && !ShortcutId::ALL.iter().any(|other| {
                                            let existing = resolved.get(*other);
                                            !existing.is_empty()
                                                && platform_combo(existing)
                                                    == platform_combo(candidate)
                                        })
                                })
                                .unwrap_or("");
                            resolved.set(id, combo.into());
                            keymap.insert(field.into(), serde_json::json!(combo));
                        }
                    }
                    serde_json::from_value::<UiSettings>(value)
                }) {
                    Ok(settings) => settings.migrated().clamped(),
                    Err(err) => {
                        tracing::warn!(error = %err, "ui-settings corrupt; using defaults");
                        Self::default()
                    }
                }
            }
            Err(_) => Self::default(),
        }
    }

    /// Write atomically (temp file + rename) so a crash mid-write never corrupts.
    pub fn save(&self, data_dir: &Path) -> io::Result<()> {
        std::fs::create_dir_all(data_dir)?;
        let path = Self::path(data_dir);
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &path)
    }

    fn migrated(mut self) -> Self {
        if self.accent == zeron_theme::AccentSelection::ThemeDefault
            && let Some(accent) = self.legacy_accent_color.take()
        {
            self.accent = zeron_theme::AccentSelection::Preset(accent.into());
        }
        self.legacy_accent_color = None;
        self
    }

    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join(FILE_NAME)
    }
}

fn clamp_or(value: f32, min: f32, max: f32, default: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        default
    }
}

fn min_or(value: f32, min: f32, default: f32) -> f32 {
    if value.is_finite() {
        value.max(min)
    } else {
        default
    }
}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
