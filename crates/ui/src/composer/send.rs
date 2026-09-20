//! Prompt submission, run request handling, queue delivery, interrupts, and send button rendering.

use std::collections::HashMap;
use std::time::Duration;

use gpui::{div, px, App, Context, IntoElement, prelude::*};
use zeron_doc::{MessagePart, SessionCommandPayload, SessionMessageEntry};
use zeron_proto::{capabilities, RunRequest, SandboxLevel};
use zeron_rpc::methods;

use crate::appshots;
use crate::attachments;
use crate::state::Indicator;
use crate::theme::Theme;

use super::decision::*;
use super::{Composer, ComposerEvent};

impl Composer {
    pub(crate) fn run_live(&self, cx: &App) -> bool {
        let s = self.state.read(cx);
        let Some(chat_id) = s.selected_chat.as_deref() else {
            return false;
        };
        matches!(
            s.indicator_for(chat_id, chrono::Utc::now()),
            Indicator::Working | Indicator::AwaitingInput
        )
    }

    /// New chats need a runnable agent, but may target the device's home
    /// directory without a project. Existing chats carry their own run config.
    pub(super) fn send_blocked(&self, cx: &App) -> bool {
        if self.queue_edit_finishing {
            return true;
        }
        let state = self.state.read(cx);
        if state.review_comment_flush_pending(&self.current_key) {
            return true;
        }
        if state.selected_chat.is_some() {
            return false;
        }
        // New-chat canvas: needs a runnable agent. The
        // no-agents check only fires once the catalog is loaded — offline
        // and still-loading states must not block (the harness resolves from
        // the remembered default and the engine reports real failures).
        self.pickers.read(cx).no_agents_available()
    }

    pub(super) fn button_mode(&self, cx: &App) -> SendButtonMode {
        if self.editing_queued.is_some() {
            return SendButtonMode::Send;
        }
        let has_text = composer_has_content(
            self.input.read(cx).text(),
            self.staged().len() + self.staged_appshots().len(),
            self.staged_comments(cx).len(),
        );
        send_button_mode(self.run_live(cx), has_text)
    }

    pub(super) fn on_submit(&mut self, cx: &mut Context<Self>) {
        if self.commit_queue_edit(cx) {
            return;
        }
        if self.wizard.is_some() {
            // Enter inside the panel's free-text input submits the page.
            let typed = self.input.read(cx).text().trim().to_string();
            if let Some(w) = self.wizard.as_mut() {
                w.set_typed(typed);
            }
            self.wizard_advance(cx);
            return;
        }
        let text = self.input.read(cx).text().trim().to_string();
        let no_content = !composer_has_content(
            &text,
            self.staged().len() + self.staged_appshots().len(),
            self.staged_comments(cx).len(),
        );
        match self.button_mode(cx) {
            // Enter never stops a run: Stop mode implies an empty composer,
            // so a stray extra Enter right after sending landed an interrupt
            // on the just-dispatched prompt and the agent ate it silently
            // (issue #406). Stop stays on the button — and on Esc when
            // escape_stops_active_agent is enabled.
            SendButtonMode::Stop => {}
            _ if no_content => {}
            _ if self.send_blocked(cx) => {}
            SendButtonMode::Send => self.send(text, false, cx),
            // Busy: keep the message queued until the current turn ends.
            SendButtonMode::Queue => self.send(text, true, cx),
        }
    }

    /// Cmd/Ctrl+Enter remains an ordinary submit while the composer carries
    /// content. With a truly empty composer it instead activates the most
    /// recently queued row, and never turns an empty chord into Stop.
    pub(super) fn on_modified_submit(&mut self, cx: &mut Context<Self>) {
        if self.commit_queue_edit(cx) {
            return;
        }
        let has_content = composer_has_content(
            self.input.read(cx).text(),
            self.staged().len() + self.staged_appshots().len(),
            self.staged_comments(cx).len(),
        );
        match modified_submit_target(has_content) {
            ModifiedSubmitTarget::SubmitContent => self.on_submit(cx),
            ModifiedSubmitTarget::ActivateLatestQueued => self.activate_latest_queued(cx),
        }
    }

    /// Queue a Run doc command with an optimistic echo — or, with the agent
    /// busy, park the message on the chat's pending queue instead. New chats
    /// thread the picked config in: worktree creation (when the isolated toggle
    /// is on), `Mutate createChat` with the `ChatConfig` + cwd, and the model /
    /// reasoning / options on the Run request itself (§1.7).
    pub(super) fn send(&mut self, text: String, queue: bool, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.failure = Some("Engine not connected".into());
            self.failure_key = None; // global — meaningful on every chat
            cx.notify();
            return;
        };
        // Chat id: existing selection, or client-minted for the new-chat canvas
        // (the chat then appears from the doc host once the doc materializes).
        let (chat_id, is_new) = match self.state.read(cx).selected_chat.clone() {
            Some(id) => (id, false),
            None => (uuid::Uuid::new_v4().to_string(), true),
        };
        // Where the new session runs (Current checkout / reuse an existing
        // worktree / fresh worktree off the picked base) — resolved NOW so
        // the async block needs no picker access.
        let plan = self.pickers.read(cx).checkout_plan();
        // Fully-resolved model/reasoning/options — concrete values (chat config
        // or defaults), so the engine never has to guess a "default".
        let resolved = self.pickers.read(cx).resolved(cx);
        let existing_cwd = self
            .state
            .read(cx)
            .selected_chat_row()
            .and_then(|c| c.cwd.clone());
        // The PROJECT fixes the new chat's device + base folder — sessions are
        // minted onto the project's device, not necessarily this one. With no
        // project ("Don't work in a project") the composer's device pick is
        // the host and the session runs from `~` there.
        let space = self.state.read(cx).selected_space_row().cloned();
        let local_device_id = self.state.read(cx).local_device_id.clone();
        let target_device_id = self.state.read(cx).effective_device_id();
        let device_id = if is_new {
            target_device_id
                .clone()
                .unwrap_or_else(|| "local".to_string())
        } else {
            self.state
                .read(cx)
                .selected_chat_row()
                .map(|c| c.device_id.clone())
                .or_else(|| local_device_id.clone())
                .unwrap_or_else(|| "local".to_string())
        };
        // Uploads/read-backs target the chat's HOST device (forwardable RPCs);
        // for a new chat that's the target device (None when it's local).
        let host_device_id = if is_new {
            target_device_id
                .clone()
                .filter(|id| local_device_id.as_deref() != Some(id.as_str()))
        } else {
            self.state
                .read(cx)
                .selected_chat_row()
                .map(|c| c.device_id.clone())
        };
        let space_id = space.as_ref().map(|s| s.id.clone());
        let space_path = space.as_ref().map(|s| s.path.clone());
        if queue && !is_new {
            let capability = if self.staged().is_empty() && self.staged_appshots().is_empty() {
                capabilities::MESSAGE_QUEUE_V1
            } else {
                capabilities::MESSAGE_QUEUE_ATTACHMENTS_V1
            };
            if !engine.engine_info().supports(capability)
                || !self.state.read(cx).chat_host_supports(&chat_id, capability)
            {
                self.failure =
                    Some("Update the chat's engine to queue messages during a response.".into());
                cx.notify();
                return;
            }
        }
        // Snapshot-and-clear NOW (use-attachments.ts takeAttachments): the
        // strip empties the instant you hit send; a failure hands the files
        // back into the chat's stash.
        let ordinary_staged = self
            .attachments
            .remove(&self.current_key)
            .unwrap_or_default();
        let staged_appshots = self.appshots.remove(&self.current_key).unwrap_or_default();
        let mut staged = ordinary_staged.clone();
        staged.extend(
            staged_appshots
                .iter()
                .map(|appshot| appshot.screenshot.clone()),
        );
        // `typed` keeps the user's own words for the failure hand-back below:
        // restoring the folded prompt would paste the comment block into the
        // input as literal text.
        let key = self.current_key.clone();
        let comments = self.state.update(cx, |state, cx| {
            let taken = state.take_review_comments(&key);
            if !taken.is_empty() {
                cx.notify();
            }
            taken
        });
        let typed = text.clone();
        let text = crate::comments::with_comments(&text, &comments);
        self.preview = None;
        let message_id = uuid::Uuid::new_v4().to_string();
        let created_at = chrono::Utc::now().timestamp_millis();
        // Existing busy chats always queue; compatibility was checked before
        // taking the draft, attachments, or review comments.
        let queue = queue && !is_new;
        let clean_queue_attachment_text = staged.is_empty()
            || (engine
                .engine_info()
                .supports(capabilities::MESSAGE_QUEUE_CLEAN_ATTACHMENT_TEXT_V1)
                && self.state.read(cx).chat_host_supports(
                    &chat_id,
                    capabilities::MESSAGE_QUEUE_CLEAN_ATTACHMENT_TEXT_V1,
                ));

        // Queued-attachment flow (durable-by-design): stage the bytes on the
        // LOCAL engine, queue the command immediately with `pending://` refs,
        // and let the engine push the bytes to a remote host afterwards —
        // staging must never gate the queue (2026-08-19 incident: a send
        // died with a zombie peer link because the upload sat in front of
        // QueueCommand). Requires every engine involved to understand the
        // ref scheme — the local engine (an IPC daemon may be older than
        // this UI) and, for remotely-hosted chats, the host; anything older
        // keeps the legacy blocking upload.
        let host_is_remote = host_device_id
            .as_deref()
            .is_some_and(|id| local_device_id.as_deref() != Some(id));
        // Queue rows do not carry upstream's attachment-transfer escort, so
        // they retain the proven host-upload path and store absolute refs.
        // Appshot XML attributes require escaped final paths. The engine's plain
        // string replacement of pending refs cannot safely rewrite those, so
        // rich captures use the existing upload-before-send path.
        let queued_flow = !queue && staged_appshots.is_empty() && !staged.is_empty() && {
            let state = self.state.read(cx);
            let local_ok = local_device_id
                .as_deref()
                .is_some_and(|id| state.device_version_at_least(id, QUEUED_ATTACHMENTS_MIN));
            let host_ok = !host_is_remote
                || host_device_id
                    .as_deref()
                    .is_some_and(|id| state.device_version_at_least(id, QUEUED_ATTACHMENTS_MIN));
            local_ok && host_ok
        };
        // Upload identities minted NOW: in the queued flow the `pending://`
        // ref IS the persisted transport until the host rewrites it, so the
        // id must exist before any bytes move.
        let upload_ids: Vec<String> = staged
            .iter()
            .map(|_| uuid::Uuid::new_v4().to_string())
            .collect();
        // The echo carries attachment refs from the first frame, so photos
        // render while the send is still pending. Queued flow: the refs are
        // the real `pending://` identities (stable — no post-upload refresh).
        // Legacy flow: synthetic `pending/…` paths that the post-upload
        // refresh replaces with the host's absolute paths. Either way the
        // staged bytes are seeded into the transcript cache under every
        // device key the transcript consults.
        let echo_paths: Vec<String> = if queued_flow {
            staged
                .iter()
                .zip(&upload_ids)
                .map(|(att, id)| format!("pending://{id}/{}", att.name))
                .collect()
        } else {
            staged
                .iter()
                .map(|att| format!("pending/{}/{}", att.id, att.name))
                .collect()
        };
        let echo_appshot_paths: HashMap<String, String> = staged
            .iter()
            .zip(&echo_paths)
            .map(|(attachment, path)| (attachment.id.clone(), path.clone()))
            .collect();
        let echo_text = attachments::with_attachments(
            &appshots::with_appshots(&text, &staged_appshots, &echo_appshot_paths),
            &echo_paths,
        );
        // Queued flow also seeds the UPLOAD ALIAS: the host rewrites the
        // persisted ref to `{its uploads dir}/{id8}-{name}` — an absolute
        // path the sender can't predict, but whose id8 it minted. The alias
        // keeps the thumbnail on the already-local bytes through that
        // rewrite instead of blanking into a reload skeleton.
        if queued_flow {
            for (upload_id, att) in upload_ids.iter().zip(&staged) {
                attachments::seed_attachment_alias(
                    &device_id,
                    upload_id,
                    &att.name,
                    att.image.clone(),
                );
                if let Some(local) = local_device_id.as_deref()
                    && local != device_id
                {
                    attachments::seed_attachment_alias(
                        local,
                        upload_id,
                        &att.name,
                        att.image.clone(),
                    );
                }
            }
        }
        for (path, att) in echo_paths.iter().zip(&staged) {
            attachments::seed_attachment(&device_id, path, &att.name, att.image.clone());
            if let Some(local) = local_device_id.as_deref()
                && local != device_id
            {
                attachments::seed_attachment(local, path, &att.name, att.image.clone());
            }
        }

        // Optimistic echo (client-minted id doubles as the persisted message id,
        // so the doc frame dedups it away).
        let echo = SessionMessageEntry {
            id: message_id.clone(),
            role: zeron_doc::MessageRole::User,
            parts: vec![MessagePart::Text {
                id: "t0".into(),
                text: echo_text.clone(),
            }],
            created_at,
            device_id: "local".into(),
            status: None,
            continuation_of: None,
        };
        self.launching_new_chat = is_new;
        if is_new {
            cx.emit(ComposerEvent::NewThreadTransitionStarted);
        }
        // A queued message is not in the transcript yet — the queue panel is
        // its echo, and it gets a real bubble when the host sends it.
        self.state.update(cx, |s, cx| {
            if is_new {
                s.select_chat(Some(chat_id.clone()), cx);
            }
            if should_publish_optimistic_echo(queue) {
                s.push_echo(&chat_id, echo);
                // Working overlay until the host executes the queued command —
                // without it a remote send flashed Completed (and could ring
                // the done-chime) in the queue→drain→sync gap.
                s.begin_pending_send(&chat_id, &message_id, chrono::Utc::now());
            }
            cx.notify();
        });

        self.input.update(cx, |input, cx| input.set_text("", cx));
        self.drafts.remove(&self.current_key);
        self.failure = None;
        self.sending = true;
        // A queued row is represented by the queue panel, not the transcript.
        // Claiming an own-turn anchor for it here would replace the live
        // turn's runway with an id that has no transcript row yet.
        if should_publish_optimistic_echo(queue) {
            cx.emit(ComposerEvent::Sent {
                chat_id: chat_id.clone(),
                message_id: message_id.clone(),
            });
        }
        cx.notify();

        let restore_text = typed;
        let err_chat_id = chat_id.clone();
        let err_message_id = message_id.clone();
        self.send_task = Some(cx.spawn(async move |this, cx| {
            let result: Result<Option<String>, String> = async {
                // Attachments stage FIRST — before the chat row or anything
                // else exists. Staging is chat-independent (keyed by
                // uploadId), and ordering it first makes a new-chat send
                // atomic: a staging failure aborts with NOTHING created,
                // instead of stranding a just-minted empty chat (v0.2.12
                // "failed to stage → empty transcript" report).
                //
                // Queued flow: commit the bytes to the LOCAL engine's uploads
                // dir (fast, offline-safe) — the queued command carries the
                // `pending://` refs and the engine delivers the bytes to a
                // remote host afterwards, retrying until they land. Legacy
                // flow (old engines): stage on the host device up front,
                // bounded by a total budget so a degraded link fails the send
                // loudly instead of grinding through silent per-chunk retries
                // for minutes.
                let mut content = text.clone();
                let mut attachment_paths: Vec<String> = Vec::new();
                let mut transfers: Vec<serde_json::Value> = Vec::new();
                if !staged.is_empty() && queued_flow {
                    // Local staging is disk-speed; publish progress anyway so
                    // huge files still narrate.
                    let progress = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
                    let total: u64 = staged.iter().map(|a| a.bytes().len() as u64).sum();
                    {
                        let progress = progress.clone();
                        this.update(cx, |composer, cx| {
                            composer.state.update(cx, |s, cx| {
                                s.begin_upload_progress(total, progress);
                                cx.notify();
                            });
                        })
                        .ok();
                    }
                    for (att, upload_id) in staged.iter().zip(&upload_ids) {
                        if let Err(err) = attachments::upload_attachment(
                            &engine,
                            cx.background_executor(),
                            None,
                            upload_id,
                            att,
                            Some(progress.clone()),
                        )
                        .await
                        {
                            tracing::warn!(name = %att.name, error = %err, "local attachment stage failed");
                            return Err("Couldn't stage the attachment locally.".to_string());
                        }
                        transfers.push(serde_json::json!({
                            "uploadId": upload_id,
                            "fileName": att.name,
                        }));
                    }
                    // The echo refs ARE the persisted refs — no refresh pass.
                    attachment_paths = echo_paths.clone();
                    content = echo_text.clone();
                } else if !staged.is_empty() {
                    let progress = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
                    let total: u64 = staged.iter().map(|a| a.bytes().len() as u64).sum();
                    {
                        let progress = progress.clone();
                        this.update(cx, |composer, cx| {
                            composer.state.update(cx, |s, cx| {
                                s.begin_upload_progress(total, progress);
                                cx.notify();
                            });
                        })
                        .ok();
                    }
                    for (att, upload_id) in staged.iter().zip(&upload_ids) {
                        match attachments::upload_attachment(
                            &engine,
                            cx.background_executor(),
                            host_device_id.as_deref(),
                            upload_id,
                            att,
                            Some(progress.clone()),
                        )
                        .await
                        {
                            Ok(path) => attachment_paths.push(path),
                            Err(err) => {
                                tracing::warn!(name = %att.name, error = %err, "attachment upload failed");
                                return Err(
                                    "Couldn't upload the attachment — the device may be offline."
                                        .to_string(),
                                );
                            }
                        }
                    }
                    // Seed the transcript cache from local bytes so the sent
                    // bubble's thumbnails never round-trip (seedTranscript-
                    // Attachment in the original send path).
                    let seed_device = host_device_id.clone().unwrap_or_else(|| device_id.clone());
                    for (path, att) in attachment_paths.iter().zip(&staged) {
                        attachments::seed_attachment(&seed_device, path, &att.name, att.image.clone());
                        if seed_device != device_id {
                            attachments::seed_attachment(&device_id, path, &att.name, att.image.clone());
                        }
                    }
                    let appshot_paths: HashMap<String, String> = staged
                        .iter()
                        .zip(&attachment_paths)
                        .map(|(attachment, path)| (attachment.id.clone(), path.clone()))
                        .collect();
                    content = attachments::with_attachments(
                        &appshots::with_appshots(&text, &staged_appshots, &appshot_paths),
                        &attachment_paths,
                    );
                    // A normal send already has an optimistic echo: refresh it
                    // in place with the uploaded refs so its thumbnails never
                    // flicker. A queued message has no transcript echo at all;
                    // its queue row is the only representation until dispatch.
                    if should_publish_optimistic_echo(queue) {
                        let refreshed = SessionMessageEntry {
                            id: message_id.clone(),
                            role: zeron_doc::MessageRole::User,
                            parts: vec![MessagePart::Text {
                                id: "t0".into(),
                                text: content.clone(),
                            }],
                            created_at,
                            device_id: "local".into(),
                            status: None,
                            continuation_of: None,
                        };
                        let echo_chat_id = chat_id.clone();
                        this.update(cx, |composer, cx| {
                            composer.state.update(cx, |s, cx| {
                                composer.state.update(cx, |s, _| s.remove_echo(&echo_chat_id, &message_id));
                                s.push_echo(&echo_chat_id, refreshed);
                                cx.notify();
                            });
                        })
                        .ok();
                    }
                }

                // Resolve the working directory: existing chats keep theirs;
                // new chats run per the checkout plan (t3code env-mode): the
                // space's folder as-is, an EXISTING worktree of the picked ref
                // (a plain cwd override — multiple sessions share one
                // worktree), or a fresh isolated worktree created off the
                // picked base ref (CreateWorktree on send, targeted at the
                // space's device; the RPC relay-forwards).
                let mut cwd = if is_new {
                    // Project-less sessions run from the host's home dir —
                    // "~" is expanded on the host when the run spawns.
                    space_path.clone().or_else(|| Some("~".to_string()))
                } else {
                    existing_cwd
                }
                .unwrap_or_else(|| ".".to_string());
                let mut worktree_cwd: Option<String> = None;
                // Fresh-worktree plans ride the QUEUED Run command (a
                // WorktreeSpec the HOST materializes at drain time) instead of
                // a blocking CreateWorktree relay RPC here: the RPC had no
                // timeout, so a lost relay frame wedged the send on "Sending…"
                // forever while the session ran remotely anyway (2026-08-18).
                let mut run_worktree: Option<zeron_proto::WorktreeSpec> = None;
                // The picked ref rides createChat so the session footer names
                // it from the first frame (it read "Select ref" until the
                // host's diff reconciler got around to stamping the branch).
                let mut chat_branch: Option<String> = None;
                if is_new && space_path.is_some() {
                    match &plan {
                        crate::pickers::CheckoutPlan::CurrentCheckout { branch } => {
                            chat_branch = branch.clone();
                        }
                        crate::pickers::CheckoutPlan::ReuseWorktree { path, branch } => {
                            cwd = path.clone();
                            worktree_cwd = Some(path.clone());
                            chat_branch = Some(branch.clone());
                        }
                        crate::pickers::CheckoutPlan::NewWorktree { base } => {
                            // Footer shows the base until the host stamps the
                            // actual zeron/<name> branch post-creation. cwd
                            // stays the repo folder — an old host that doesn't
                            // know the spec degrades to the main checkout
                            // instead of failing the run.
                            chat_branch = base.clone();
                            if let Some(repo_path) = &space_path {
                                // A remote repo's branch list loads over the
                                // relay — on a bad link it may never arrive
                                // and the picker has no base. That must NOT
                                // silently drop the isolation the user picked
                                // (2026-08-19: "New worktree" ran in the main
                                // checkout): default to HEAD, which git — any
                                // host version — resolves as the repo's
                                // current checkout state.
                                let base =
                                    base.clone().unwrap_or_else(|| "HEAD".to_string());
                                run_worktree = Some(zeron_proto::WorktreeSpec {
                                    repo_path: repo_path.clone(),
                                    base,
                                    space_id: space_id.clone(),
                                });
                            }
                        }
                    }
                }

                // Best-effort Mutate createChat with the picked config: the
                // engine resolves device + cwd from the PROJECT row when one
                // is picked; project-less chats name the host device outright
                // (idempotent; the doc host would materialize the chat on
                // first command anyway, so failures are non-fatal).
                if is_new {
                    let mut mutate = serde_json::json!({
                        "op": "createChat",
                        "chatId": chat_id,
                    });
                    if let Some(object) = mutate.as_object_mut() {
                        match &space_id {
                            Some(space_id) => {
                                object.insert(
                                    "spaceId".into(),
                                    serde_json::Value::String(space_id.clone()),
                                );
                            }
                            None => {
                                object.insert(
                                    "deviceId".into(),
                                    serde_json::Value::String(device_id.clone()),
                                );
                            }
                        }
                    }
                    if let Some(object) = mutate.as_object_mut() {
                        if let Some(worktree_cwd) = &worktree_cwd {
                            object.insert(
                                "cwd".into(),
                                serde_json::Value::String(worktree_cwd.clone()),
                            );
                        }
                        if let Some(branch) = &chat_branch {
                            object.insert(
                                "branch".into(),
                                serde_json::Value::String(branch.clone()),
                            );
                        }
                        if let Some(config) = resolved.chat_config()
                            && let Ok(config) = serde_json::to_value(&config)
                        {
                            object.insert("config".into(), config);
                        }
                    }
                    if let Err(err) = attachments::call_with_timeout(
                        &engine,
                        cx.background_executor(),
                        methods::MUTATE,
                        mutate,
                        std::time::Duration::from_secs(30),
                    )
                    .await
                    {
                        tracing::warn!(error = %err, "CreateChat mutate unavailable; doc host will materialize the chat");
                    }
                }

                if queue {
                    // A queue row is editable UI state, so its text must stay
                    // free of the internal attachment-path trailer. The host
                    // rebuilds that transport when it promotes the row.
                    let appshot_paths = staged.iter().zip(&attachment_paths)
                        .map(|(attachment, path)| (attachment.id.clone(), path.clone())).collect();
                    let queue_body = appshots::with_appshots(&text, &staged_appshots, &appshot_paths);
                    let queue_text = if !clean_queue_attachment_text {
                        content.as_str()
                    } else if queue_body.trim().is_empty() && !attachment_paths.is_empty() {
                        attachments::ATTACHMENT_ONLY_TEXT
                    } else {
                        queue_body.as_str()
                    };
                    let params = serde_json::json!({
                        "chatId": chat_id,
                        "text": queue_text,
                        "attachments": attachment_paths,
                        "holdForTurnEnd": true,
                    });
                    let reply = engine
                        .client()
                        .call(methods::QUEUE_MESSAGE, params)
                        .await
                        .map_err(|e| format!("Send failed: {e}"))?;
                    let queue_id = reply
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .filter(|id| !id.is_empty())
                        .ok_or_else(|| "Send failed: queue did not return an id".to_string())?;
                    return Ok(Some(queue_id.to_string()));
                }

                let expects_setup_handoff = run_worktree
                    .as_ref()
                    .and_then(|spec| spec.space_id.as_ref())
                    .is_some();
                let command = SessionCommandPayload::Run {
                    request: RunRequest {
                        prompt: content.clone(),
                        harness: resolved.harness,
                        model: resolved.model.clone(),
                        reasoning: resolved.reasoning,
                        model_options: resolved.model_options.clone(),
                        cwd,
                        sandbox: SandboxLevel::WorkspaceWrite,
                        auto_approve: false,
                        resume: None,
                        attachments: attachment_paths,
                        worktree: run_worktree,
                    },
                    message_id: message_id.clone(),
                };
                let command = serde_json::to_value(&command)
                    .map_err(|e| format!("Send failed: {e}"))?;
                let mut params = serde_json::json!({ "chatId": chat_id, "command": command });
                if !transfers.is_empty() {
                    params["transfers"] = serde_json::Value::Array(transfers);
                }
                // Deadline-bounded: QueueCommand is a local write (in-process
                // or IPC), but a deferred engine handle can park forever.
                let queued = attachments::call_with_timeout(
                    &engine,
                    cx.background_executor(),
                    methods::QUEUE_COMMAND,
                    params,
                    std::time::Duration::from_secs(30),
                )
                .await
                .map_err(|e| format!("Send failed: {e}"))?;
                if expects_setup_handoff
                    && let Some(command_id) = queued
                        .get("commandId")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                {
                    let poll_engine = engine.clone();
                    let poll_chat_id = chat_id.clone();
                    let poll_target_device_id = host_device_id.clone();
                    this.update(cx, |_, cx| {
                        cx.spawn(async move |this, cx| {
                            for _ in 0..480 {
                                let mut params = serde_json::json!({
                                    "chatId": poll_chat_id,
                                    "commandId": command_id,
                                });
                                if let (Some(target), Some(object)) =
                                    (&poll_target_device_id, params.as_object_mut())
                                {
                                    object.insert(
                                        "targetDeviceId".into(),
                                        serde_json::Value::String(target.clone()),
                                    );
                                }
                                match attachments::call_with_timeout(
                                    &poll_engine,
                                    cx.background_executor(),
                                    methods::TAKE_PROJECT_ACTION_SETUP,
                                    params,
                                    std::time::Duration::from_secs(10),
                                )
                                .await
                                {
                                    Ok(value) if value.get("ready").and_then(|v| v.as_bool()) == Some(true) => {
                                        let setup_action = value
                                            .get("setupAction")
                                            .cloned()
                                            .filter(|value| !value.is_null())
                                            .and_then(|value| serde_json::from_value(value).ok());
                                        let setup_error = value
                                            .get("setupError")
                                            .and_then(|value| value.as_str())
                                            .map(str::to_string);
                                        this.update(cx, |_, cx| {
                                            cx.emit(ComposerEvent::WorktreeSetup {
                                                chat_id: poll_chat_id.clone(),
                                                setup_action,
                                                setup_error,
                                                target_device_id: poll_target_device_id.clone(),
                                            });
                                        })
                                        .ok();
                                        return;
                                    }
                                    Err(error) if error.starts_with("unknown method: ") => return,
                                    Ok(_) | Err(_) => {}
                                }
                                cx.background_executor()
                                    .timer(Duration::from_millis(250))
                                    .await;
                            }
                            tracing::warn!(
                                chat = %poll_chat_id,
                                command = %command_id,
                                "worktree setup handoff timed out"
                            );
                        })
                        .detach();
                    })
                    .ok();
                }
                Ok(None)
            }
            .await;
            if result.is_err() && is_new {
                // A failed new-chat send must not strand a just-minted empty
                // chat in the sidebar (v0.2.12 "empty transcript" report).
                // Staging now runs before CreateChat, so usually nothing was
                // created — but a post-mutate failure (QueueCommand) still
                // leaves a row. Best-effort delete; a no-op if the chat was
                // never materialized.
                let _ = attachments::call_with_timeout(
                    &engine,
                    cx.background_executor(),
                    methods::MUTATE,
                    serde_json::json!({ "op": "deleteChat", "chatId": err_chat_id }),
                    std::time::Duration::from_secs(5),
                )
                .await;
            }
            this.update(cx, |composer, cx| {
                composer.sending = false;
                composer
                    .state
                    .update(cx, |s, _| s.end_upload_progress());
                if let Ok(Some(message_id)) = &result {
                    cx.emit(ComposerEvent::Queued {
                        chat_id: err_chat_id.clone(),
                        message_id: message_id.clone(),
                    });
                }
                if let Err(message) = result {
                    // Failure: red banner, echo removed, prompt back in the
                    // draft, staged files back in the stash. A failed NEW
                    // chat restores to the CANVAS (key "") and navigates back
                    // there — the minted chat is gone (deleted above), so
                    // nothing may restore under its key.
                    let restore_key = if is_new {
                        String::new()
                    } else {
                        err_chat_id.clone()
                    };
                    composer.failure = Some(message.into());
                    composer.failure_key = Some(restore_key.clone());
                    composer.state.update(cx, |s, cx| {
                        s.remove_echo(&err_chat_id, &err_message_id);
                        s.end_pending_send(&err_chat_id, &err_message_id);
                        if is_new && s.selected_chat.as_deref() == Some(err_chat_id.as_str()) {
                            // Back to the canvas; the navigation draft-swap
                            // loads the restored draft below.
                            s.select_chat(None, cx);
                        }
                        for comment in &comments {
                            s.add_review_comment(&restore_key, comment.clone());
                        }
                        cx.notify();
                    });
                    if is_new && composer.current_key != restore_key {
                        // A re-key swap to the canvas is pending (the
                        // select_chat(None) above); it loads this draft into
                        // the input on flush — setting the input directly
                        // here would be clobbered by that same swap.
                        composer.drafts.insert(restore_key.clone(), restore_text.clone());
                    } else {
                        // Already keyed to the restore target (either an
                        // existing chat, or the deleted row's watch event
                        // re-keyed to the canvas before this handler ran —
                        // no further swap will fire). Set the input directly.
                        composer.input.update(cx, |input, cx| input.set_text(restore_text, cx));
                    }
                    if !ordinary_staged.is_empty() {
                        // Merge by id (stashAttachments): files the user staged
                        // while the send was in flight survive the hand-back —
                        // draining the minted chat's slot too when the restore
                        // target is the canvas.
                        let mut merged = ordinary_staged.clone();
                        for key in [err_chat_id.clone(), restore_key.clone()] {
                            if let Some(slot) = composer.attachments.get_mut(&key) {
                                let fresh: Vec<_> = slot
                                    .drain(..)
                                    .filter(|e| !merged.iter().any(|f| f.id == e.id))
                                    .collect();
                                merged.extend(fresh);
                            }
                        }
                        composer.attachments.insert(restore_key.clone(), merged);
                    }
                    composer.restore_failed_appshots(&staged_appshots, &err_chat_id, &restore_key);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    pub(crate) fn interrupt_selected(&mut self, cx: &mut Context<Self>) {
        let Some(chat_id) = self.state.read(cx).selected_chat.clone() else {
            return;
        };
        self.interrupt_chat(chat_id, cx);
    }

    pub(crate) fn interrupt_chat(&mut self, chat_id: String, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        if !begin_interrupt(&mut self.interrupting, &chat_id) {
            return;
        }
        let params = interrupt_params(&chat_id);
        let task_chat_id = chat_id.clone();
        let failure_chat = chat_id.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = engine.client().call(methods::QUEUE_COMMAND, params).await;
            if let Err(err) = result {
                this.update(cx, |composer, cx| {
                    composer.interrupting.remove(&task_chat_id);
                    composer.failure = Some(format!("Stop failed: {err}").into());
                    composer.failure_key = Some(failure_chat);
                    cx.notify();
                })
                .ok();
            }
        });
        self.interrupt_tasks.insert(chat_id, task);
    }

    pub(crate) fn is_interrupting(&self, chat_id: &str) -> bool {
        self.interrupting.contains(chat_id)
    }

    pub(super) fn render_send_button(
        &mut self,
        mode: SendButtonMode,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = Theme::of(cx);
        // Zeron composer-actions.tsx: a size-7 filled circle — up-arrow to
        // send/queue, a dark rounded square on the same light circle to stop.
        match mode {
            SendButtonMode::Stop => div()
                .id("composer-stop")
                .size(px(28.0))
                .flex_none()
                .rounded_full()
                .bg(theme.text)
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.opacity(0.85))
                .on_click(cx.listener(|this, _, _, cx| this.interrupt_selected(cx)))
                .child(div().size(px(11.0)).rounded(px(3.0)).bg(theme.bg))
                .into_any_element(),
            SendButtonMode::Send | SendButtonMode::Queue => {
                // Share the submission guard with Enter, including pending
                // edits and the new-session runnable-agent check.
                let blocked = self.send_blocked(cx);
                div()
                    .id("composer-send")
                    .size(px(28.0))
                    .flex_none()
                    .rounded_full()
                    .bg(theme.text)
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(blocked, |el| el.opacity(0.35))
                    .when(!blocked, |el| {
                        el.cursor_pointer()
                            .hover(|s| s.opacity(0.85))
                            .on_click(cx.listener(|this, _, _, cx| this.on_submit(cx)))
                    })
                    .child(
                        crate::icons::icon(crate::icons::ARROW_UP)
                            .size(px(14.0))
                            .text_color(theme.bg),
                    )
                    .into_any_element()
            }
        }
    }
}
