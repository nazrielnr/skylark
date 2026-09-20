use std::time::Duration;

use gpui::{
    div, prelude::*, px, AnyElement, App, Context, Entity, Focusable as _, IntoElement,
    SharedString, Subscription, Task, Window,
};
use zeron_rpc::methods;

use crate::changes::{Changes, ChangesEvent};
use crate::files::{FilesCloseDisposition, FilesEvent, FilesSurface};
use crate::icons::{self, icon};
use crate::theme::Theme;
use crate::transcript::{Transcript, TranscriptEvent};
use crate::workspace_links::resolve_workspace_file_link;

use super::{layout::WidthTween, NavEntry, PendingExit, Shell};

/// One right-pane surface tab: a workspace browser, an individual workspace
/// file editor, a Git diff or history page, an embedded terminal, or a
/// subagent transcript. `Picker` is the empty surface chooser.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RightSurface {
    #[default]
    Picker,
    Files,
    File(u64),
    Browser(u64),
    Diff(u64),
    Terminal(u64),
    /// A subagent's transcript, read-only (per-subagent viz) — the handle
    /// keys [`Shell::subagent_tabs`].
    Subagent(u64),
}

pub(crate) fn push_unique_right_surface(tabs: &mut Vec<RightSurface>, surface: RightSurface) -> bool {
    if tabs.contains(&surface) {
        false
    } else {
        tabs.push(surface);
        true
    }
}

/// One right-pane subagent tab: the doc it shows, its strip title, and the
/// read-only transcript entity whose drop tears the view down.
pub(crate) struct SubagentTab {
    pub(crate) doc_id: String,
    pub(crate) title: SharedString,
    pub(crate) transcript: Entity<Transcript>,
    /// Keeps a frozen-blob fetch alive (it falls back to a live doc watch).
    pub(crate) _fetch: Option<Task<()>>,
    /// Spawn chips INSIDE the subagent transcript open their own tabs.
    pub(crate) _events: Subscription,
}

impl Shell {
    pub(super) fn session_links(
        source_session: Option<String>,
        cx: &Context<Self>,
    ) -> crate::markdown::render::LinkUi {
        let shell = cx.weak_entity();
        crate::markdown::render::LinkUi {
            source_session,
            handler: std::rc::Rc::new(move |activation, window, cx| {
                shell
                    .update(cx, |shell, cx| {
                        shell.activate_session_link(activation, window, cx)
                    })
                    .unwrap_or(crate::markdown::render::LinkOutcome::Rejected)
            }),
        }
    }

    pub(super) fn activate_session_link(
        &mut self,
        activation: &crate::markdown::render::LinkActivation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> crate::markdown::render::LinkOutcome {
        use crate::markdown::render::{LinkAction, LinkOutcome};
        if self.active_chat.is_empty()
            || activation.source_session.as_deref() != Some(self.active_chat.as_str())
            || self.state.read(cx).selected_chat.as_deref() != Some(self.active_chat.as_str())
        {
            return LinkOutcome::Rejected;
        }
        if activation.target.navigation.is_err() {
            return if matches!(
                activation.action,
                LinkAction::Primary | LinkAction::Internal
            ) && self.open_workspace_file_link(&activation.target.original, window, cx)
            {
                LinkOutcome::Internal
            } else {
                LinkOutcome::Rejected
            };
        }
        let mut resolved = activation.clone();
        if resolved.action == LinkAction::Primary {
            resolved.action = if crate::settings::current(cx).open_web_links_in_zeron {
                LinkAction::Internal
            } else {
                LinkAction::External
            };
        }
        let outcome = resolved.web_outcome(cfg!(any(target_os = "macos", target_os = "linux")));
        if outcome == LinkOutcome::Internal {
            if !self.right_pane_open(cx) {
                self.toggle_right_pane(cx);
            }
            self.add_browser_surface(activation.target.navigation.clone().ok(), window, cx);
        }
        outcome
    }

    /// Browser tabs are independent instances owned by the current session.
    pub(crate) fn add_browser_surface(
        &mut self,
        url: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_chat.is_empty() {
            return;
        }
        let key = self.panel_key(cx);
        let remote = {
            let state = self.state.read(cx);
            state.selected_chat_row().is_some_and(|chat| {
                Some(chat.device_id.as_str()) != state.local_device_id.as_deref()
            })
        };
        self.browser_seq += 1;
        let id = self.browser_seq;
        let browser = cx.new(|cx| {
            crate::browser::BrowserSurface::new(self.browser_context.clone(), remote, window, cx)
        });
        if let Some(handle) = self.state.read(cx).engine().cloned() {
            let chat_id = self.active_chat.clone();
            browser.update(cx, |browser, cx| {
                browser.watch_previews(handle, chat_id, cx)
            });
        }
        let owner = key.clone();
        let sub = cx.subscribe_in(&browser, window, move |this, _, event, window, cx| {
            match event {
                crate::browser::BrowserEvent::Changed => cx.notify(),
                crate::browser::BrowserEvent::NewTab(url) => {
                    // A background page cannot open a tab in the wrong session.
                    if this.panel_key(cx) == owner
                        && this.resolved_right_active(cx) == RightSurface::Browser(id)
                    {
                        this.add_browser_surface(url.clone(), window, cx);
                    }
                }
                crate::browser::BrowserEvent::Close => {
                    this.close_right_surface(RightSurface::Browser(id), window, cx)
                }
            }
        });
        self.browsers.insert(id, browser.clone());
        self.browser_subs.insert(id, sub);
        self.right_tabs
            .entry(key)
            .or_default()
            .push(RightSurface::Browser(id));
        self.set_right_active(RightSurface::Browser(id), cx);
        browser.update(cx, |browser, cx| {
            if let Some(url) = url {
                browser.navigate(&url, window, cx);
            } else {
                browser.focus_address(window, cx);
            }
        });
    }

    /// The picker's Diffs card / the `+` menu's Diff row: every click opens a
    /// FRESH diff tab with its own scope/base selection (multiple diff
    /// panels, user request).
    pub(super) fn add_diff_surface(&mut self, cx: &mut Context<Self>) {
        let changes = cx.new(|cx| Changes::new(self.state.clone(), cx));
        self.register_diff_surface(changes, cx);
    }

    /// Files is single-instance per chat: both the picker and the `+` menu
    /// focus the existing surface instead of creating duplicate trees and
    /// duplicate workspace subscriptions.
    pub(super) fn add_files_surface(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_chat.is_empty() {
            return;
        }
        let key = self.panel_key(cx);
        if !self.files.contains_key(&key) {
            let autosave_enabled = self.settings.files_autosave_enabled;
            let delay = self.settings.files_autosave_delay_ms;
            let editor_font_size = crate::typography::code_font_size(cx);
            let word_wrap = self.settings.files_word_wrap;
            let show_all_files = self.settings.files_show_all;
            let files = cx.new(|cx| {
                FilesSurface::new(
                    self.state.clone(),
                    self.active_chat.clone(),
                    autosave_enabled,
                    delay,
                    editor_font_size,
                    word_wrap,
                    show_all_files,
                    cx,
                )
            });
            let event_key = key.clone();
            let sub = cx.subscribe_in(
                &files,
                window,
                move |this: &mut Self, _, event, window, cx| match event {
                    FilesEvent::OpenFile(path) => this.add_file_surface(path.clone(), window, cx),
                    FilesEvent::OpenWebLink(activation) => {
                        if let crate::markdown::render::LinkOutcome::External(url) =
                            this.activate_session_link(activation, window, cx)
                        {
                            cx.open_url(&url);
                        }
                    }
                    FilesEvent::TitleChanged => cx.notify(),
                    FilesEvent::FileRenamed { .. } => cx.notify(),
                    FilesEvent::WordWrapChanged(word_wrap) => {
                        this.set_files_word_wrap(*word_wrap, window, cx)
                    }
                    FilesEvent::ShowAllFilesChanged(show_all_files) => {
                        this.set_files_show_all(*show_all_files, cx)
                    }
                    FilesEvent::CloseReady => {
                        this.on_file_close_ready(RightSurface::Files, &event_key, cx)
                    }
                    FilesEvent::CloseCancelled => this.cancel_file_close(RightSurface::Files, cx),
                },
            );
            self.files.insert(key.clone(), files);
            self.files_subs.insert(key.clone(), sub);
        }
        let tabs = self.right_tabs.entry(key).or_default();
        push_unique_right_surface(tabs, RightSurface::Files);
        self.set_right_active(RightSurface::Files, cx);
        self.focus_right_file_editor(RightSurface::Files, window, cx);
    }

    /// Open a workspace file as a first-class right-pane tab. Every editor is
    /// a separate FilesSurface so its tree, search, watcher and split layout
    /// stay stable while users move among open files.
    pub(crate) fn add_file_surface(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_chat.is_empty() {
            return;
        }
        let panel_key = self.panel_key(cx);
        let lookup = (panel_key.clone(), path.clone());
        if let Some(id) = self.file_surface_keys.get(&lookup).copied() {
            let surface = RightSurface::File(id);
            self.set_right_active(surface, cx);
            self.focus_right_file_editor(surface, window, cx);
            return;
        }

        self.file_surface_seq += 1;
        let id = self.file_surface_seq;
        let file = cx.new(|cx| {
            FilesSurface::new_editor(
                self.state.clone(),
                self.active_chat.clone(),
                path.clone(),
                self.settings.files_autosave_enabled,
                self.settings.files_autosave_delay_ms,
                crate::typography::code_font_size(cx),
                self.settings.files_word_wrap,
                self.settings.files_show_all,
                cx,
            )
        });
        let event_panel_key = panel_key.clone();
        let sub = cx.subscribe_in(
            &file,
            window,
            move |this: &mut Self, _, event, window, cx| match event {
                FilesEvent::OpenFile(path) => this.add_file_surface(path.clone(), window, cx),
                FilesEvent::OpenWebLink(activation) => {
                    if let crate::markdown::render::LinkOutcome::External(url) =
                        this.activate_session_link(activation, window, cx)
                    {
                        cx.open_url(&url);
                    }
                }
                FilesEvent::TitleChanged => cx.notify(),
                FilesEvent::FileRenamed { old_path, new_path } => {
                    this.rename_file_surface(id, &event_panel_key, old_path, new_path, cx)
                }
                FilesEvent::WordWrapChanged(word_wrap) => {
                    this.set_files_word_wrap(*word_wrap, window, cx)
                }
                FilesEvent::ShowAllFilesChanged(show_all_files) => {
                    this.set_files_show_all(*show_all_files, cx)
                }
                FilesEvent::CloseReady => {
                    this.on_file_close_ready(RightSurface::File(id), &event_panel_key, cx)
                }
                FilesEvent::CloseCancelled => this.cancel_file_close(RightSurface::File(id), cx),
            },
        );
        self.file_surfaces.insert(id, file);
        self.file_surface_paths.insert(id, path);
        self.file_surface_keys.insert(lookup, id);
        self.file_surface_subs.insert(id, sub);
        self.right_tabs
            .entry(panel_key)
            .or_default()
            .push(RightSurface::File(id));
        self.set_right_active(RightSurface::File(id), cx);
    }

    pub(super) fn open_workspace_file_link(
        &mut self,
        target: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(chat) = self
            .state
            .read(cx)
            .chats
            .iter()
            .find(|chat| chat.id == self.active_chat)
        else {
            return false;
        };
        let Some(root) = chat.cwd.as_deref() else {
            return false;
        };
        let Some(link) = resolve_workspace_file_link(target, root) else {
            return false;
        };

        let key = self.panel_key(cx);
        let was_open = self.panels.get(&key).changes_open;
        let from = self.right_target(cx);
        self.panels.update(&key, |panel| panel.changes_open = true);
        if !was_open {
            self.right_tween = Some(WidthTween::new(from, self.right_target(cx)));
        }
        self.add_file_surface(link.path, window, cx);
        true
    }

    pub(super) fn rename_file_surface(
        &mut self,
        id: u64,
        panel_key: &str,
        old_path: &str,
        new_path: &str,
        cx: &mut Context<Self>,
    ) {
        if self.file_surface_paths.get(&id).map(String::as_str) != Some(old_path) {
            return;
        }
        self.file_surface_paths.insert(id, new_path.to_string());
        self.file_surface_keys
            .remove(&(panel_key.to_string(), old_path.to_string()));
        self.file_surface_keys
            .entry((panel_key.to_string(), new_path.to_string()))
            .or_insert(id);
        cx.notify();
    }

    /// The dedicated History surface. Keeping it as its own tab preserves its
    /// graph/search state while Diff tabs retain their ordinary scope picker.
    pub(super) fn add_history_surface(&mut self, cx: &mut Context<Self>) {
        let history = cx.new(|cx| Changes::for_history(self.state.clone(), cx));
        self.register_diff_surface(history, cx);
    }

    /// A History row click: the commit opens as its own pinned diff tab
    /// (user request).
    pub(super) fn add_commit_diff_surface(
        &mut self,
        commit: zeron_proto::GitHistoryCommit,
        cx: &mut Context<Self>,
    ) {
        let changes = cx.new(|cx| Changes::for_commit(self.state.clone(), commit, cx));
        self.register_diff_surface(changes, cx);
    }

    pub(super) fn register_diff_surface(&mut self, changes: Entity<Changes>, cx: &mut Context<Self>) {
        self.diff_seq += 1;
        let id = self.diff_seq;
        let sub = cx.subscribe(&changes, |this: &mut Self, _, event, cx| match event {
            ChangesEvent::OpenCommit(commit) => {
                this.add_commit_diff_surface(commit.clone(), cx);
            }
        });
        self.diffs.insert(id, changes);
        self.diff_subs.insert(id, sub);
        let key = self.panel_key(cx);
        self.right_tabs
            .entry(key)
            .or_default()
            .push(RightSurface::Diff(id));
        self.set_right_active(RightSurface::Diff(id), cx);
    }

    /// The picker's Terminal card / the `+` menu's Terminal row: every click
    /// opens a fresh embedded terminal tab.
    pub(super) fn add_terminal_surface(&mut self, cx: &mut Context<Self>) {
        let panel = self.right_terminal_panel(cx);
        let opened = panel.update(cx, |panel, cx| {
            panel.set_open(true, cx);
            panel.open_tab_for_selected(cx)
        });
        if let Some(tab) = opened {
            let key = self.panel_key(cx);
            self.right_tabs
                .entry(key)
                .or_default()
                .push(RightSurface::Terminal(tab));
            self.set_right_active(RightSurface::Terminal(tab), cx);
        }
    }

    /// Spawn-chip events from the primary transcript AND from subagent-tab
    /// transcripts (nested spawns open their own tabs).
    pub(super) fn on_transcript_event(
        &mut self,
        _: Entity<Transcript>,
        event: &TranscriptEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            TranscriptEvent::OpenSubagent {
                chat_id,
                doc_id,
                title,
                frozen,
            } => {
                self.add_subagent_surface(
                    chat_id.clone(),
                    doc_id.clone(),
                    title.clone(),
                    *frozen,
                    cx,
                );
            }
        }
    }

    /// A spawn chip's "Open subagent": focus the existing tab for that doc,
    /// or open one. `frozen` (subagent done/failed) tries the uploaded
    /// transcript blob first and falls back to the live doc watch; running
    /// subagents watch the doc directly.
    pub(super) fn add_subagent_surface(
        &mut self,
        chat_id: String,
        doc_id: String,
        title: String,
        frozen: bool,
        cx: &mut Context<Self>,
    ) {
        // The chip lives in the conversation column — the pane it opens into
        // may still be closed.
        if !self.right_pane_open(cx) {
            self.toggle_right_pane(cx);
        }
        if let Some((&id, _)) = self
            .subagent_tabs
            .iter()
            .find(|(_, tab)| tab.doc_id == doc_id)
        {
            self.set_right_active(RightSurface::Subagent(id), cx);
            return;
        }
        self.subagent_seq += 1;
        let id = self.subagent_seq;
        // A live subagent follows its streaming end (main-transcript feel);
        // a frozen one reads top-down.
        let transcript =
            cx.new(|cx| Transcript::for_doc(self.state.clone(), doc_id.clone(), !frozen, cx));
        let links = Self::session_links(Some(self.active_chat.clone()), cx);
        transcript.update(cx, |transcript, _| {
            transcript.set_workspace_link_handler(links)
        });
        let events = cx.subscribe(&transcript, Self::on_transcript_event);
        let fetch = if frozen {
            self.spawn_subagent_snapshot_fetch(&chat_id, &doc_id, cx)
        } else {
            self.state
                .update(cx, |s, cx| s.watch_subagent_doc(doc_id.clone(), cx));
            None
        };
        self.subagent_tabs.insert(
            id,
            SubagentTab {
                doc_id,
                title: title.into(),
                transcript,
                _fetch: fetch,
                _events: events,
            },
        );
        let key = self.panel_key(cx);
        self.right_tabs
            .entry(key)
            .or_default()
            .push(RightSurface::Subagent(id));
        self.set_right_active(RightSurface::Subagent(id), cx);
    }

    /// Fetch a finished subagent's frozen transcript blob
    /// (`{chat_id}/{doc_id}`); on ANY failure fall back to watching the doc
    /// — the blob upload is best-effort engine-side.
    pub(super) fn spawn_subagent_snapshot_fetch(
        &self,
        chat_id: &str,
        doc_id: &str,
        cx: &mut Context<Self>,
    ) -> Option<Task<()>> {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.state
                .update(cx, |s, cx| s.watch_subagent_doc(doc_id.to_string(), cx));
            return None;
        };
        let blob_ref = format!("{chat_id}/{doc_id}");
        let state = self.state.clone();
        let doc_id = doc_id.to_string();
        Some(cx.spawn(async move |_, cx| {
            let reply = crate::attachments::call_with_timeout(
                &engine,
                cx.background_executor(),
                methods::FETCH_TOOL_BLOB,
                serde_json::json!({ "blobRef": blob_ref }),
                Duration::from_secs(20),
            )
            .await;
            let snapshot = cx
                .background_executor()
                .spawn(async move {
                    let value = reply.ok()?;
                    let entries: Vec<zeron_doc::SessionMessageEntry> =
                        serde_json::from_str(value.get("text")?.as_str()?).ok()?;
                    let update = zeron_doc::TranscriptUpdate {
                        replay_baseline: Some(zeron_doc::TranscriptBaseline::capture(&entries)),
                        frame: zeron_doc::TranscriptFrame::Reset { reset: entries },
                        context_usage: None,
                    };
                    let prepared = crate::transcript::TranscriptPreparation::default()
                        .prepare(&update)
                        .ok()?;
                    let zeron_doc::TranscriptFrame::Reset { reset } = update.frame else {
                        unreachable!()
                    };
                    Some((reset, prepared))
                })
                .await;
            state.update(cx, |s, cx| {
                match snapshot {
                    Some((entries, prepared)) => {
                        s.set_prepared_subagent_snapshot(doc_id, entries, prepared);
                    }
                    None => s.watch_subagent_doc(doc_id, cx),
                }
                cx.notify();
            });
        }))
    }

    /// A surface tab's ✕. The active fallback happens naturally through
    /// [`Self::resolved_right_active`] on the next frame.
    pub(crate) fn close_right_surface(
        &mut self,
        surface: RightSurface,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let was_active = self.resolved_right_active(cx) == surface;
        let key = self.panel_key(cx);
        let files = match surface {
            RightSurface::Files => self.files.get(&key).cloned(),
            RightSurface::File(id) => self.file_surfaces.get(&id).cloned(),
            _ => None,
        };
        if let Some(files) = files {
            match files.update(cx, |files, cx| files.prepare_close(cx)) {
                FilesCloseDisposition::Allow => {
                    self.complete_file_close(surface, &key, cx);
                }
                FilesCloseDisposition::Pending | FilesCloseDisposition::Blocked => {
                    self.pending_file_closes.insert(surface);
                    self.set_right_active(surface, cx);
                }
            }
            return;
        }
        if let Some(tabs) = self.right_tabs.get_mut(&key) {
            tabs.retain(|s| *s != surface);
        }
        match surface {
            RightSurface::Files | RightSurface::File(_) => {}
            RightSurface::Browser(id) => {
                if let Some(browser) = self.browsers.remove(&id) {
                    browser.update(cx, |browser, cx| browser.close(cx));
                }
                self.browser_subs.remove(&id);
                if was_active {
                    window.focus(&self.composer.focus_handle(cx), cx);
                }
            }
            RightSurface::Diff(id) => {
                // Dropping the entity tears down its diff watch.
                self.diffs.remove(&id);
                self.diff_subs.remove(&id);
            }
            RightSurface::Terminal(tab) => {
                let panel = self.right_terminal_panel(cx);
                panel.update(cx, |panel, cx| panel.close_tab_by_key(tab, window, cx));
            }
            RightSurface::Subagent(id) => {
                // Unwatch drops the watch task — that cancels the engine-side
                // watch and unpins the subagent doc from the engine LRU.
                if let Some(tab) = self.subagent_tabs.remove(&id) {
                    self.state
                        .update(cx, |s, _| s.unwatch_subagent_doc(&tab.doc_id));
                }
            }
            RightSurface::Picker => {}
        }
        self.panels.update(&key, |p| {
            if p.right_active == surface {
                p.right_active = RightSurface::Picker;
            }
        });
        cx.notify();
    }

    pub(super) fn on_file_close_ready(
        &mut self,
        surface: RightSurface,
        panel_key: &str,
        cx: &mut Context<Self>,
    ) {
        if self.pending_file_closes.contains(&surface) {
            self.complete_file_close(surface, panel_key, cx);
        } else {
            if self.pending_exit.is_some() {
                self.reveal_unsaved_file(cx);
            }
            cx.notify();
        }
    }

    pub(crate) fn cancel_file_close(&mut self, surface: RightSurface, cx: &mut Context<Self>) {
        self.pending_file_closes.remove(&surface);
        self.pending_exit = None;
        cx.notify();
    }

    pub fn prepare_window_close(&mut self, cx: &mut Context<Self>) -> bool {
        self.prepare_exit(PendingExit::CloseWindow, cx)
    }

    /// The first rung of `⌘W` / Window > Close Window: when the right pane is
    /// open on a real surface (the file / diff / terminal / browser tab the
    /// user just opened), close THAT and leave the window alone. Returns true
    /// when the close was consumed by the pane.
    ///
    /// The pane's empty picker state and a closed pane both yield false, so the
    /// caller falls through to [`Self::prepare_window_close`] and the window
    /// closes — the same cascade browsers use. The native traffic-light close
    /// deliberately skips this rung: it always closes the window.
    pub fn close_active_surface(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(surface) = self.closable_right_surface(cx) else {
            return false;
        };
        self.close_right_surface(surface, window, cx);
        true
    }

    /// The right pane's closable surface for the `⌘W` cascade: the resolved
    /// active surface while the pane is open, or `None` on the picker empty
    /// state / a closed pane / the new-session canvas.
    pub(crate) fn closable_right_surface(&self, cx: &App) -> Option<RightSurface> {
        if !self.right_pane_open(cx) {
            return None;
        }
        match self.resolved_right_active(cx) {
            RightSurface::Picker => None,
            surface => Some(surface),
        }
    }

    pub fn prepare_quit(&mut self, cx: &mut Context<Self>) -> bool {
        self.prepare_exit(PendingExit::Quit, cx)
    }

    pub(super) fn prepare_exit(&mut self, action: PendingExit, cx: &mut Context<Self>) -> bool {
        let surfaces = self
            .files
            .values()
            .chain(self.file_surfaces.values())
            .cloned()
            .collect::<Vec<_>>();
        if surfaces
            .iter()
            .all(|surface| !surface.read(cx).has_unsaved_changes())
        {
            self.pending_exit = None;
            return true;
        }
        self.pending_exit = Some(action);
        let mut all_ready = true;
        for surface in surfaces {
            let disposition = surface.update(cx, |surface, cx| surface.prepare_close(cx));
            all_ready &= disposition == FilesCloseDisposition::Allow;
        }
        if all_ready {
            self.pending_exit = None;
        } else {
            self.reveal_unsaved_file(cx);
        }
        cx.notify();
        all_ready
    }

    pub(super) fn reveal_unsaved_file(&mut self, cx: &mut Context<Self>) {
        let browser = self.files.iter().filter_map(|(key, files)| {
            files
                .read(cx)
                .has_unsaved_changes()
                .then(|| (key.clone(), RightSurface::Files))
        });
        let editors = self.file_surface_keys.iter().filter_map(|((key, _), id)| {
            self.file_surfaces
                .get(id)
                .filter(|files| files.read(cx).has_unsaved_changes())
                .map(|_| (key.clone(), RightSurface::File(*id)))
        });
        let current = self.panel_key(cx);
        let mut dirty = browser.chain(editors).collect::<Vec<_>>();
        dirty.sort_by_key(|(key, _)| (key != &current, key.clone()));
        if let Some((key, surface)) = dirty.into_iter().next() {
            self.panels.update(&key, |panel| {
                panel.changes_open = true;
                panel.right_active = surface;
            });
            self.apply_nav(NavEntry::Chat(key), cx);
        }
    }

    pub(crate) fn all_file_edits_flushed(&self, cx: &App) -> bool {
        self.files
            .values()
            .chain(self.file_surfaces.values())
            .all(|surface| !surface.read(cx).has_unsaved_changes())
    }

    pub(super) fn complete_file_close(
        &mut self,
        surface: RightSurface,
        panel_key: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(tabs) = self.right_tabs.get_mut(panel_key) {
            tabs.retain(|candidate| *candidate != surface);
        }
        match surface {
            RightSurface::Files => {
                self.files.remove(panel_key);
                self.files_subs.remove(panel_key);
            }
            RightSurface::File(id) => {
                self.file_surfaces.remove(&id);
                self.file_surface_paths.remove(&id);
                self.file_surface_subs.remove(&id);
                self.file_surface_keys.retain(|_, value| *value != id);
            }
            _ => return,
        }
        self.pending_file_closes.remove(&surface);
        self.panels.update(panel_key, |panel| {
            if panel.right_active == surface {
                panel.right_active = RightSurface::Picker;
            }
        });
        cx.notify();
    }

    /// The right pane's empty state: a compact vertical list of surface rows
    /// (icon + label). The old two-card grid clipped in narrow panes and
    /// wasted short ones.
    pub(super) fn render_surface_picker(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let text = theme.text;
        let muted = theme.text_muted;
        let border = theme.border;
        let border_strong = theme.border_strong;
        let row = |id: &'static str, icon_path: &'static str, title: &'static str| {
            div()
                .id(id)
                .w_full()
                .h(px(44.0))
                .px(px(14.0))
                .rounded(px(10.0))
                .border_1()
                .border_color(border)
                .bg(crate::theme::ink(0.02))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(10.0))
                .cursor_pointer()
                .hover(move |s| s.bg(crate::theme::ink(0.05)).border_color(border_strong))
                .child(icon(icon_path).size(px(15.0)).flex_none().text_color(muted))
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(13.0))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(text)
                        .child(SharedString::from(title)),
                )
        };
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .p(px(16.0))
            .child(
                div()
                    .w_full()
                    .max_w(px(280.0))
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(
                        row("surface-card-files", icons::FOLDER_WITH_FILES, "Files").on_click(
                            cx.listener(|this, _, window, cx| {
                                this.add_files_surface(window, cx);
                            }),
                        ),
                    )
                    .child(
                        row("surface-card-browser", icons::GLOBE, "Browser").on_click(cx.listener(
                            |this, _, window, cx| this.add_browser_surface(None, window, cx),
                        )),
                    )
                    .child(
                        row("surface-card-terminal", icons::TERMINAL, "Terminal").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.add_terminal_surface(cx);
                            }),
                        ),
                    )
                    // Git surfaces only where there IS git — the pane itself
                    // no longer gates on it (terminals work anywhere).
                    .when(self.space_git_detected(cx), |el| {
                        el.child(row("surface-card-diffs", icons::LIST, "Diffs").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.add_diff_surface(cx);
                            }),
                        ))
                        .child(
                            row("surface-card-history", icons::GIT_BRANCH, "History").on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.add_history_surface(cx);
                                }),
                            ),
                        )
                    }),
            )
            .into_any_element()
    }
}
