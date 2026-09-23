//! Queue mutations, edit leases, and delivery actions.

use super::*;

impl Composer {
    /// Track the drop slot while a row is dragged over the list.
    pub(crate) fn update_queue_drag_over(
        &mut self,
        from: usize,
        over: usize,
        cx: &mut Context<Self>,
    ) {
        match &mut self.queue_drag {
            Some(drag) if drag.from == from => {
                if drag.over != over {
                    drag.prev_over = drag.over;
                    drag.over = over;
                    drag.epoch = drag.epoch.wrapping_add(1);
                    cx.notify();
                }
            }
            _ => {
                self.queue_drag = Some(QueueDragState {
                    from,
                    over,
                    prev_over: from,
                    epoch: 0,
                });
                cx.notify();
            }
        }
    }

    /// Restore a row whose pointer was released outside the queue's drop zone.
    pub(crate) fn cancel_queue_drag(&mut self, cx: &mut Context<Self>) {
        if self.queue_drag.take().is_some() {
            cx.notify();
        }
    }

    /// Move the row at `from` to `to`, optimistically here and for real on the
    /// doc (the watch frame is what everyone else sees).
    pub(crate) fn move_queued(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        if from == to {
            cx.notify();
            return;
        }
        let Some(id) = self
            .state
            .read(cx)
            .queue
            .get(from)
            .map(|item| item.id.clone())
        else {
            return;
        };
        self.state.update(cx, |state, cx| {
            if from < state.queue.len() {
                let item = state.queue.remove(from);
                state.queue.insert(to.min(state.queue.len()), item);
                cx.notify();
            }
        });
        self.queue_rpc(
            methods::MOVE_QUEUED_MESSAGE,
            serde_json::json!({ "id": id, "toIndex": to }),
            "Couldn't reorder the queue",
            cx,
        );
    }

    /// Cancel a queued message at its host. The row remains visible and inert
    /// until the host acknowledges winning the race against automatic drain.
    pub(crate) fn remove_queued(&mut self, id: String, cx: &mut Context<Self>) {
        if self.queue_removing.contains(&id) {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let (chat_id, host_device_id, supported) = {
            let state = self.state.read(cx);
            let Some(chat_id) = state.selected_chat.clone() else {
                return;
            };
            let Some(host_device_id) = state.selected_chat_row().map(|chat| chat.device_id.clone())
            else {
                return;
            };
            let supported = state.chat_host_supports(
                &chat_id,
                skylark_proto::capabilities::MESSAGE_QUEUE_ACTIONS_V1,
            );
            (chat_id, host_device_id, supported)
        };
        if !supported {
            self.failure = Some("The chat host does not support safe queue removal".into());
            cx.notify();
            return;
        }
        if self.editing_queued.as_deref() == Some(id.as_str()) {
            self.clear_queue_edit(cx);
        }
        self.queue_removing.insert(id.clone());
        self.queue_drag = None;
        cx.notify();

        let params = serde_json::json!({
            "chatId": chat_id,
            "id": id,
            "targetDeviceId": host_device_id,
        });
        cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::REMOVE_QUEUED_MESSAGE, params)
                .await;
            this.update(cx, |composer, cx| {
                composer.queue_removing.remove(&id);
                let selected_matches =
                    composer.state.read(cx).selected_chat.as_deref() == Some(chat_id.as_str());
                match result {
                    Ok(reply)
                        if queue_mutation_acknowledged(methods::REMOVE_QUEUED_MESSAGE, &reply) =>
                    {
                        if selected_matches {
                            composer.state.update(cx, |state, cx| {
                                state.queue.retain(|item| item.id != id);
                                cx.notify();
                            });
                        }
                    }
                    Ok(reply) => {
                        tracing::debug!(
                            ?reply,
                            "queued message had already left the queue before removal"
                        );
                        if selected_matches {
                            composer.failure =
                                Some("That message had already left the queue".into());
                            composer
                                .state
                                .update(cx, |state, cx| state.refresh_selected_queue(cx));
                        }
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "host-authoritative queue removal failed");
                        if selected_matches {
                            composer.failure = Some("Couldn't remove the message".into());
                            composer
                                .state
                                .update(cx, |state, cx| state.refresh_selected_queue(cx));
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Send one now: the host stops the turn and hands this message over. Not
    /// optimistic — the row leaves the queue when the host has actually taken
    /// it, so a failed interrupt doesn't lose the text.
    pub(crate) fn send_queued_now(&mut self, id: String, cx: &mut Context<Self>) {
        if self.editing_queued.as_deref() == Some(id.as_str()) {
            self.clear_queue_edit(cx);
        }
        self.queue_rpc(
            methods::SEND_QUEUED_MESSAGE_NOW,
            serde_json::json!({ "id": id }),
            "Couldn't send that message",
            cx,
        );
    }

    /// Execute the same resolved action advertised on the row. Both pointer
    /// clicks and the empty-composer Enter gesture come through here.
    pub(crate) fn activate_queued_primary(
        &mut self,
        id: String,
        action: QueuePrimaryAction,
        cx: &mut Context<Self>,
    ) {
        match action {
            QueuePrimaryAction::SendNow => self.send_queued_now(id, cx),
        }
    }

    /// Cmd/Ctrl+Enter on an empty composer activates the same action shown on
    /// the most recently queued row: Send now, interrupting the current response.
    /// An edit/review gate or an old chat host makes it a no-op.
    pub(crate) fn activate_latest_queued(&mut self, cx: &mut Context<Self>) {
        if self.editing_queued.is_some() {
            return;
        }
        let (id, delivery_blocked, host_supports_actions) = {
            let state = self.state.read(cx);
            let Some(chat_id) = state.selected_chat.as_deref() else {
                return;
            };
            let Some(item) = latest_queued_message(&state.queue) else {
                return;
            };
            (
                item.id.clone(),
                item.delivery_gate.is_some(),
                state.chat_host_supports(
                    chat_id,
                    skylark_proto::capabilities::MESSAGE_QUEUE_ACTIONS_V1,
                ),
            )
        };
        let Some(action) = available_queue_primary_action(delivery_blocked, host_supports_actions)
        else {
            return;
        };
        self.activate_queued_primary(id, action, cx);
    }

    /// Borrow the composer while the leased row reserves its queue position.
    pub(crate) fn begin_queue_edit(&mut self, id: String, cx: &mut Context<Self>) {
        if self.queue_edit_pending_id.is_some()
            || self.queue_edit_finishing
            || self.editing_queued.is_some()
            || !self.can_edit_queue_in_composer()
        {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let (chat_id, host_device_id, supported) = {
            let state = self.state.read(cx);
            let Some(chat_id) = state.selected_chat.clone() else {
                return;
            };
            let Some(host_device_id) = state.selected_chat_row().map(|chat| chat.device_id.clone())
            else {
                return;
            };
            let capability = skylark_proto::capabilities::MESSAGE_QUEUE_EDIT_LEASE_V1;
            let supported = engine.engine_info().supports(capability)
                && state.chat_host_supports(&chat_id, capability);
            (chat_id, host_device_id, supported)
        };
        if !supported {
            self.failure = Some("Update the chat host to edit queued messages safely".into());
            cx.notify();
            return;
        }
        if !self.state.read(cx).queue.iter().any(|item| item.id == id) {
            return;
        }
        let owner_device_id = engine.engine_info().device_id.clone();
        let instance_id = self.queue_edit_instance_id.clone();
        self.queue_edit_pending_id = Some(id.clone());
        self.queue_drag = None;
        cx.notify();
        let params = serde_json::json!({
            "chatId": chat_id,
            "id": id,
            "editorDeviceId": owner_device_id,
            "editorInstanceId": instance_id,
            "targetDeviceId": host_device_id,
        });
        let task = cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::BEGIN_QUEUED_MESSAGE_EDIT, params)
                .await;
            let mut loaded_attachments = Vec::new();
            let mut loaded_appshots = Vec::new();
            if let Ok(reply) = &result
                && reply.get("outcome").and_then(|v| v.as_str()) == Some("acquired")
            {
                let paths = reply.get("attachments")
                    .and_then(|v| serde_json::from_value::<Vec<String>>(v.clone()).ok());
                let mut load_failed = paths.is_none();
                for path in paths.unwrap_or_default() {
                    let loaded = crate::attachments::read_attachment_image(
                        &engine, cx.background_executor(), Some(&host_device_id), &path,
                        None,
                    ).await;
                    match loaded {
                        Some(loaded) => loaded_attachments.push(crate::attachments::StagedAttachment {
                            id: uuid::Uuid::new_v4().to_string(), name: loaded.name, image: loaded.image,
                        }),
                        None => { load_failed = true; break; }
                    }
                }
                if !load_failed {
                    let paths: Vec<String> = serde_json::from_value(reply["attachments"].clone()).unwrap_or_default();
                    match crate::appshots::restore_queued_appshots(
                        reply.get("text").and_then(|v| v.as_str()).unwrap_or_default(),
                        &paths, &loaded_attachments,
                    ) {
                        Ok((ordinary, shots)) => { loaded_attachments = ordinary; loaded_appshots = shots; }
                        Err(_) => { load_failed = true; }
                    }
                }
                if load_failed {
                    let _ = engine.client().call(methods::FINISH_QUEUED_MESSAGE_EDIT, serde_json::json!({
                        "chatId": chat_id, "id": id, "leaseId": reply.get("leaseId"),
                        "action": "cancel", "targetDeviceId": host_device_id,
                    })).await;
                    this.update(cx, |composer, cx| {
                        composer.queue_edit_pending_id = None;
                        composer.failure = Some("Couldn't load the queued attachments or Appshot context. Check the connection and update the chat host.".into());
                        cx.notify();
                    }).ok();
                    return;
                }
            }
            this.update(cx, |composer, cx| {
                composer.queue_edit_pending_id = None;
                match result {
                    Ok(reply)
                        if reply.get("outcome").and_then(|v| v.as_str()) == Some("acquired") =>
                    {
                        let Some(lease_id) = reply.get("leaseId").and_then(|v| v.as_str()) else {
                            composer.failure =
                                Some("The chat host returned an invalid edit lease".into());
                            cx.notify();
                            return;
                        };
                        let Some(base_text_hash) =
                            reply.get("baseTextHash").and_then(|v| v.as_str())
                        else {
                            composer.failure =
                                Some("The chat host returned an invalid edit lease".into());
                            cx.notify();
                            return;
                        };
                        let raw_text = reply
                            .get("text")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        let attachments: Vec<String> = serde_json::from_value(reply["attachments"].clone()).unwrap_or_default();
                        let text = queue_visible_text(&raw_text, &attachments);
                        let text = if !attachments.is_empty() && text == crate::attachments::ATTACHMENT_ONLY_TEXT {
                            String::new()
                        } else { text };
                        let selected_matches = composer.state.read(cx).selected_chat.as_deref()
                            == Some(chat_id.as_str());
                        if !selected_matches || !composer.can_edit_queue_in_composer() {
                            // Navigation or another composer action won acquisition. Release
                            // immediately; the expiry/review path is the backup.
                            let params = serde_json::json!({
                                "chatId": chat_id,
                                "id": id,
                                "leaseId": lease_id,
                                "action": "cancel",
                                "targetDeviceId": host_device_id,
                            });
                            let engine = engine.clone();
                            cx.spawn(async move |_, _| {
                                let _ = engine
                                    .client()
                                    .call(methods::FINISH_QUEUED_MESSAGE_EDIT, params)
                                    .await;
                            })
                            .detach();
                            return;
                        }
                        composer.editing_queued = Some(id.clone());
                        composer.queue_edit_lease_id = Some(lease_id.to_string());
                        composer.queue_edit_base_text_hash = Some(base_text_hash.to_string());
                        composer.queue_edit_chat_id = Some(chat_id.clone());
                        composer.queue_edit_host_device_id = Some(host_device_id.clone());
                        composer.queue_edit_draft = Some((
                            composer.input.read(cx).text().to_string(),
                            composer.attachments.remove(&composer.current_key).unwrap_or_default(),
                            composer.appshots.remove(&composer.current_key).unwrap_or_default(),
                        ));
                        composer.attachments.insert(composer.current_key.clone(), loaded_attachments);
                        composer.appshots.insert(composer.current_key.clone(), loaded_appshots);
                        composer.focus_pending = true;
                        composer.input.update(cx, |input, cx| input.set_text(text, cx));
                        composer.start_queue_edit_renewal(engine.clone(), cx);
                    }
                    Ok(reply)
                        if reply.get("outcome").and_then(|v| v.as_str()) == Some("locked") =>
                    {
                        composer.failure =
                            Some("That queued message is being edited on another device".into());
                    }
                    Ok(_) => {
                        composer.failure =
                            Some("That queued message is no longer available".into());
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "begin queue edit failed");
                        composer.failure =
                            Some("Connect to the chat host to edit this message".into());
                    }
                }
                cx.notify();
            })
            .ok();
        });
        self.queue_edit_task = Some(task);
    }

    /// Save the composer into the existing row, including its attachments.
    /// An entirely empty composer removes the row.
    pub(crate) fn commit_queue_edit(&mut self, cx: &mut Context<Self>) -> bool {
        if self.editing_queued.is_none() {
            return false;
        }
        let text = self.input.read(cx).text().trim().to_string();
        if text.is_empty() && self.staged().is_empty() && self.staged_appshots().is_empty() {
            self.finish_queue_edit("discard", None, cx);
        } else {
            self.finish_queue_edit("commit", Some(text), cx);
        }
        true
    }

    /// Escape out of an edit, leaving the row as it was.
    pub(crate) fn cancel_queue_edit(&mut self, cx: &mut Context<Self>) -> bool {
        if self.editing_queued.is_none() {
            return false;
        }
        self.finish_queue_edit("cancel", None, cx);
        true
    }

    pub(crate) fn clear_queue_edit(&mut self, cx: &mut Context<Self>) {
        self.release_queue_edit_best_effort(cx);
        self.clear_queue_edit_local(cx);
    }

    pub(crate) fn clear_queue_edit_local(&mut self, cx: &mut Context<Self>) {
        self.editing_queued = None;
        self.queue_edit_lease_id = None;
        self.queue_edit_base_text_hash = None;
        self.queue_edit_chat_id = None;
        self.queue_edit_host_device_id = None;
        self.queue_edit_pending_id = None;
        self.queue_edit_finishing = false;
        self.input.update(cx, |input, cx| {
            input.read_only = false;
            cx.notify();
        });
        self.queue_edit_task = None;
        self.queue_edit_renew_task = None;
        if let Some((text, attachments, appshots)) = self.queue_edit_draft.take() {
            self.appshots.insert(self.current_key.clone(), appshots);
            self.input.update(cx, |input, cx| input.set_text(text, cx));
            self.attachments
                .insert(self.current_key.clone(), attachments);
        }
        self.focus_pending = true;
        cx.notify();
    }

    pub(crate) fn finish_queue_edit(
        &mut self,
        action: &'static str,
        text: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.queue_edit_finishing {
            return;
        }
        let (Some(id), Some(lease_id), Some(chat_id), Some(host_device_id), Some(engine)) = (
            self.editing_queued.clone(),
            self.queue_edit_lease_id.clone(),
            self.queue_edit_chat_id.clone(),
            self.queue_edit_host_device_id.clone(),
            self.state.read(cx).engine().cloned(),
        ) else {
            self.failure = Some("The edit lease was lost; your text is still in the editor".into());
            cx.notify();
            return;
        };
        let expected = self.queue_edit_base_text_hash.clone();
        let mut staged = self.staged().to_vec();
        let staged_appshots = self.staged_appshots().to_vec();
        staged.extend(staged_appshots.iter().map(|shot| shot.screenshot.clone()));
        let mut params = serde_json::json!({
            "chatId": chat_id,
            "id": id,
            "leaseId": lease_id,
            "action": action,
            "text": text,
            "expectedTextHash": expected,
            "targetDeviceId": host_device_id,
        });
        self.queue_edit_finishing = true;
        self.input.update(cx, |input, cx| {
            input.read_only = true;
            cx.notify();
        });
        cx.notify();
        let task = cx.spawn(async move |this, cx| {
            let result = async {
                if action == "commit" {
                    let mut paths = Vec::new();
                    for attachment in &staged {
                        let path = crate::attachments::upload_attachment(
                            &engine, cx.background_executor(), Some(&host_device_id),
                            &uuid::Uuid::new_v4().to_string(), attachment, None,
                        ).await.map_err(|err| err.to_string())?;
                        paths.push(path);
                    }
                    let appshot_paths = staged.iter().zip(&paths)
                        .map(|(attachment, path)| (attachment.id.clone(), path.clone())).collect();
                    params["text"] = crate::appshots::with_appshots(
                        params["text"].as_str().unwrap_or_default(), &staged_appshots, &appshot_paths,
                    ).into();
                    if params["text"].as_str().is_some_and(|text| text.trim().is_empty()) && !paths.is_empty() {
                        params["text"] = crate::attachments::ATTACHMENT_ONLY_TEXT.into();
                    }
                    params["attachments"] = serde_json::json!(paths);
                }
                crate::attachments::call_with_timeout(
                    &engine, cx.background_executor(), methods::FINISH_QUEUED_MESSAGE_EDIT,
                    params, std::time::Duration::from_secs(30),
                ).await.map_err(|err| err.to_string())
            }.await;
            this.update(cx, |composer, cx| {
                composer.queue_edit_finishing = false;
                composer.input.update(cx, |input, cx| { input.read_only = false; cx.notify(); });
                match result {
                    Ok(reply) => match reply.get("outcome").and_then(|v| v.as_str()) {
                        Some("committed" | "cancelled" | "discarded" | "released") => {
                            composer.clear_queue_edit_local(cx);
                            return;
                        }
                        Some("conflict") => {
                            composer.failure = Some(
                                "This message changed on another device; your edit was kept locally".into(),
                            );
                        }
                        Some("missing") => {
                            composer.failure = Some(
                                "The queued message was removed; your edit was kept locally".into(),
                            );
                        }
                        _ => {
                            composer.failure = Some(
                                "The edit lease changed; your text is still in the editor".into(),
                            );
                        }
                    },
                    Err(err) => {
                        tracing::warn!(error = %err, "finish queue edit failed");
                        composer.failure = Some(
                            "Couldn't reach the chat host; your edit is still in the editor".into(),
                        );
                    }
                }
                cx.notify();
            }).ok();
        });
        self.queue_edit_task = Some(task);
    }

    pub(crate) fn start_queue_edit_renewal(
        &mut self,
        engine: crate::state::EngineHandle,
        cx: &mut Context<Self>,
    ) {
        let (Some(id), Some(lease_id), Some(chat_id), Some(host_device_id)) = (
            self.editing_queued.clone(),
            self.queue_edit_lease_id.clone(),
            self.queue_edit_chat_id.clone(),
            self.queue_edit_host_device_id.clone(),
        ) else {
            return;
        };
        self.queue_edit_renew_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(20))
                    .await;
                let params = serde_json::json!({
                    "chatId": chat_id,
                    "id": id,
                    "leaseId": lease_id,
                    "targetDeviceId": host_device_id,
                });
                match engine
                    .client()
                    .call(methods::RENEW_QUEUED_MESSAGE_EDIT, params)
                    .await
                {
                    Ok(reply)
                        if reply.get("outcome").and_then(|v| v.as_str()) == Some("renewed") => {}
                    Ok(_) => {
                        this.update(cx, |composer, cx| {
                            composer.failure = Some(
                                "Edit protection expired; review this message before sending"
                                    .into(),
                            );
                            cx.notify();
                        })
                        .ok();
                        break;
                    }
                    Err(err) => {
                        tracing::debug!(error = %err, "queue edit heartbeat failed");
                        // A transient miss is tolerated by the 60s lease. Keep
                        // trying; the host fails closed if all attempts miss.
                    }
                }
            }
        }));
    }

    pub(crate) fn release_queue_edit_best_effort(&self, cx: &mut Context<Self>) {
        let (Some(id), Some(lease_id), Some(chat_id), Some(host_device_id), Some(engine)) = (
            self.editing_queued.clone(),
            self.queue_edit_lease_id.clone(),
            self.queue_edit_chat_id.clone(),
            self.queue_edit_host_device_id.clone(),
            self.state.read(cx).engine().cloned(),
        ) else {
            return;
        };
        let params = serde_json::json!({
            "chatId": chat_id,
            "id": id,
            "leaseId": lease_id,
            "action": "cancel",
            "targetDeviceId": host_device_id,
        });
        cx.spawn(async move |_, _| {
            let _ = engine
                .client()
                .call(methods::FINISH_QUEUED_MESSAGE_EDIT, params)
                .await;
        })
        .detach();
    }

    /// Fire one queue mutation at the chat's doc host.
    pub(crate) fn queue_rpc(
        &mut self,
        method: &'static str,
        params: serde_json::Value,
        failure: &'static str,
        cx: &mut Context<Self>,
    ) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let (chat_id, host_device_id, host_supports_action) = {
            let state = self.state.read(cx);
            let Some(chat_id) = state.selected_chat.clone() else {
                return;
            };
            let host = queue_action_needs_host(method)
                .then(|| state.selected_chat_row().map(|chat| chat.device_id.clone()))
                .flatten();
            let supported = !queue_action_needs_host(method)
                || state.chat_host_supports(
                    &chat_id,
                    skylark_proto::capabilities::MESSAGE_QUEUE_ACTIONS_V1,
                );
            (chat_id, host, supported)
        };
        if !host_supports_action {
            self.failure = Some("The chat host does not support queue actions".into());
            cx.notify();
            return;
        }
        let pending_message = matches!(
            method,
            methods::SEND_QUEUED_MESSAGE_NOW | methods::STEER_QUEUED_MESSAGE_NOW
        )
        .then(|| {
            params
                .get("id")
                .and_then(|id| id.as_str())
                .map(str::to_owned)
        })
        .flatten();
        if let Some(id) = &pending_message {
            self.state.update(cx, |state, cx| {
                state.begin_pending_send(&chat_id, id, chrono::Utc::now());
                cx.notify();
            });
        }
        let mut params = params;
        if let Some(object) = params.as_object_mut() {
            object.insert("chatId".into(), serde_json::Value::String(chat_id.clone()));
            if let Some(host) = host_device_id {
                object.insert("targetDeviceId".into(), serde_json::Value::String(host));
            }
        }
        // Detached, not held: these are independent one-shot mutations, and
        // parking them in a single slot meant the next arrow tap dropped — and
        // so cancelled — the move still in flight, leaving the optimistic list
        // showing an order the doc never got.
        cx.spawn(
            async move |this, cx| match engine.client().call(method, params).await {
                Ok(reply) if queue_mutation_acknowledged(method, &reply) => {
                    // The host has adopted this message. Clear even if the
                    // user switched chats and its transcript is no longer watched.
                    if let Some(id) = &pending_message {
                        this.update(cx, |composer, cx| {
                            composer.state.update(cx, |state, cx| {
                                state.end_pending_send(&chat_id, id);
                                cx.notify();
                            });
                        })
                        .ok();
                    }
                }
                Ok(reply) => {
                    tracing::debug!(
                        method,
                        ?reply,
                        "queue mutation was not applied; reconciling"
                    );
                    this.update(cx, |composer, cx| {
                        if let Some(id) = &pending_message {
                            composer.state.update(cx, |state, cx| {
                                state.end_pending_send(&chat_id, id);
                                cx.notify();
                            });
                        }
                        composer
                            .state
                            .update(cx, |state, cx| state.refresh_selected_queue(cx));
                    })
                    .ok();
                }
                Err(err) => {
                    tracing::warn!(method, error = %err, "queue mutation failed");
                    this.update(cx, |composer, cx| {
                        if let Some(id) = &pending_message {
                            composer.state.update(cx, |state, cx| {
                                state.end_pending_send(&chat_id, id);
                                cx.notify();
                            });
                        }
                        composer.failure = Some(failure.into());
                        composer
                            .state
                            .update(cx, |state, cx| state.refresh_selected_queue(cx));
                        cx.notify();
                    })
                    .ok();
                }
            },
        )
        .detach();
    }
}
