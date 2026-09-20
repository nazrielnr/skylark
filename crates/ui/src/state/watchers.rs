//! AppState engine subscriptions and resubscription pumps.

use super::*;

/// Observe assembly after an early attach (cloud onboarding or another viewport
/// reaching the embedded engine over IPC). Data subscriptions wait on the same
/// result, but their individual errors are not authoritative: older engines may
/// legitimately omit a watch method. Only the assembly result may fail the
/// whole connection.
pub(super) fn spawn_deferred_engine_watch(
    cx: &mut Context<AppState>,
    handle: EngineHandle,
) -> Option<Task<()>> {
    let mut deferred = handle.deferred_state()?;
    Some(cx.spawn(async move |this, cx| {
        let Err(failure) = wait_for_deferred_engine(&mut deferred).await else {
            return;
        };
        tracing::error!(error = %failure, "engine assembly failed after attachment");
        // Embedded handles release their IPC listener before exposing Retry;
        // remote handles stop their completed readiness probe.
        handle.shutdown().await;
        this.update(cx, |state, cx| {
            state.connection = ConnectionStatus::Failed(failure);
            cx.notify();
        })
        .ok();
    }))
}

/// Chats watch. Boot selection is the shell's job (it lands on the first
/// restored open tab, device-local state this entity can't see); this task
/// only pumps frames.
pub(super) fn spawn_chats_watch(cx: &mut Context<AppState>, handle: EngineHandle) -> Task<()> {
    cx.spawn(async move |this, cx| {
        // Resubscribe loop (same contract as the transcript watch): a daemon
        // restart or RPC drop ends the stream, and a bare return here froze
        // the sidebar until app restart — new chats, renames and archives
        // from every device silently stopped arriving.
        const RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(2);
        loop {
            let mut rx = match handle
                .client()
                .subscribe(methods::WATCH_CHATS, serde_json::json!({}))
                .await
            {
                Ok(rx) => rx,
                Err(err) => {
                    tracing::debug!(error = %err, "chats watch unavailable; retrying");
                    if this.update(cx, |_, _| {}).is_err() {
                        return;
                    }
                    cx.background_executor().timer(RETRY_DELAY).await;
                    continue;
                }
            };
            while let Some(value) = rx.recv().await {
                let parsed: Vec<Chat> = match serde_json::from_value(value) {
                    Ok(parsed) => parsed,
                    Err(err) => {
                        tracing::warn!(error = %err, "dropping malformed chats frame");
                        continue;
                    }
                };
                let alive = this.update(cx, |state, cx| {
                    state.apply_chats(parsed);
                    state.apply_pending_deep_link(cx);
                    state.reconcile_change_request_watches(cx);
                    cx.notify();
                });
                if alive.is_err() {
                    return;
                }
            }
            tracing::debug!("chats stream ended; resubscribing");
            if this.update(cx, |_, _| {}).is_err() {
                return;
            }
            cx.background_executor().timer(RETRY_DELAY).await;
        }
    })
}

pub use zeron_proto::version_triple;

pub(super) fn spawn_change_request_watch(
    cx: &mut Context<AppState>,
    handle: EngineHandle,
    target: ChangeRequestWatchKey,
    local_device_id: Option<String>,
) -> Task<()> {
    cx.spawn(async move |this, cx| {
        const RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(2);
        loop {
            let params = watch_params(&target, local_device_id.as_deref());

            let mut subscription = match handle
                .client()
                .subscribe_checked(methods::WATCH_CHECKOUT_CHANGE_REQUEST, params)
                .await
            {
                Ok(subscription) => subscription,
                Err(RpcError::UnknownMethod(_)) => {
                    tracing::debug!(
                        device = %target.device_id,
                        "checkout change requests unsupported on device"
                    );
                    this.update(cx, |state, cx| {
                        let engine_version = state
                            .devices
                            .iter()
                            .find(|device| device.id == target.device_id)
                            .and_then(|device| device.version.clone());
                        state
                            .change_requests
                            .mark_unsupported(target.device_id.clone(), engine_version);
                        cx.notify();
                    })
                    .ok();
                    return;
                }
                Err(err) => {
                    tracing::debug!(
                        device = %target.device_id,
                        cwd = %target.cwd,
                        error = %err,
                        "checkout change request watch unavailable; retrying"
                    );
                    if this.update(cx, |_, _| {}).is_err() {
                        return;
                    }
                    cx.background_executor().timer(RETRY_DELAY).await;
                    continue;
                }
            };

            while let Some(value) = subscription.recv().await {
                let snapshot: CheckoutChangeRequestStatus = match serde_json::from_value(value) {
                    Ok(snapshot) => snapshot,
                    Err(err) => {
                        tracing::warn!(
                            device = %target.device_id,
                            cwd = %target.cwd,
                            error = %err,
                            "dropping malformed checkout change request frame"
                        );
                        continue;
                    }
                };
                if this
                    .update(cx, |state, cx| {
                        state.change_requests.store(target.clone(), snapshot);
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }

            // Preserve the latest successful snapshot during a transport gap.
            tracing::debug!(
                device = %target.device_id,
                cwd = %target.cwd,
                "checkout change request stream ended; resubscribing"
            );
            if this.update(cx, |_, _| {}).is_err() {
                return;
            }
            cx.background_executor().timer(RETRY_DELAY).await;
        }
    })
}

pub(super) fn spawn_watch<T: DeserializeOwned + 'static>(
    cx: &mut Context<AppState>,
    handle: EngineHandle,
    method: &'static str,
    apply: fn(&mut AppState, T) -> bool,
) -> Task<()> {
    cx.spawn(async move |this, cx| {
        // Resubscribe loop: these are the standing Sessions/Devices/Spaces
        // watches — a daemon restart ended the stream and a bare return froze
        // them for the rest of the app's life (remote Working dots staled out
        // to nothing after 45s, and Idle/Completed transitions from other
        // devices never arrived again — "the session never completes").
        const RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(2);
        loop {
            let mut rx = match handle
                .client()
                .subscribe(method, serde_json::json!({}))
                .await
            {
                Ok(rx) => rx,
                Err(err) => {
                    tracing::debug!(method, error = %err, "watch unavailable; retrying");
                    if this.update(cx, |_, _| {}).is_err() {
                        return;
                    }
                    cx.background_executor().timer(RETRY_DELAY).await;
                    continue;
                }
            };
            while let Some(value) = rx.recv().await {
                let parsed: T = match serde_json::from_value(value) {
                    Ok(parsed) => parsed,
                    Err(err) => {
                        tracing::warn!(method, error = %err, "dropping malformed watch frame");
                        continue;
                    }
                };
                let alive = this.update(cx, |state, cx| {
                    let changed = apply(state, parsed);
                    state.apply_pending_deep_link(cx);
                    if changed {
                        if matches!(method, methods::WATCH_SPACES | methods::WATCH_DEVICES) {
                            state.reconcile_change_request_watches(cx);
                        }
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    return;
                }
            }
            tracing::debug!(method, "watch stream ended; resubscribing");
            if this.update(cx, |_, _| {}).is_err() {
                return;
            }
            cx.background_executor().timer(RETRY_DELAY).await;
        }
    })
}

/// Best-effort `LocalDevice` probe: fills `local_device_id` for the "This
/// device" badge. Engines that don't serve the method leave it `None`.
pub(super) fn spawn_local_device_probe(
    cx: &mut Context<AppState>,
    handle: EngineHandle,
) -> Task<()> {
    cx.spawn(async move |this, cx| {
        let Ok(value) = handle
            .client()
            .call("LocalDevice", serde_json::json!({}))
            .await
        else {
            tracing::debug!("LocalDevice unavailable; skipping this-device badge");
            return;
        };
        let id = value
            .get("id")
            .or_else(|| value.get("deviceId"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        if let Some(id) = id {
            this.update(cx, |state, cx| {
                state.local_device_id = Some(id);
                state.apply_pending_deep_link(cx);
                // Watches opened before this probe conservatively route through
                // targetDeviceId. Recreate them now that local routing is known.
                state.change_request_tasks.clear();
                state.reconcile_change_request_watches(cx);
                cx.notify();
            })
            .ok();
        }
    })
}

pub(super) fn spawn_transcript_watch(
    cx: &mut Context<AppState>,
    handle: EngineHandle,
    chat_id: String,
) -> Task<()> {
    cx.spawn(async move |this, cx| {
        // Outer loop: a delta desync (missed frame) resubscribes immediately
        // and the fresh stream's opening reset heals the copy; a subscribe
        // failure, malformed frame, or stream end retries on a delay. Every
        // path re-enters the loop — a return here freezes the transcript
        // with no banner and no heal short of an app restart (this watch and
        // its engine-side room are the ONLY transcript delivery path). The
        // task itself is dropped by select_chat/apply_chats when the chat is
        // deselected or deleted, so retrying can't outlive relevance.
        const RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(2);
        'resubscribe: loop {
            let params = serde_json::json!({ "chatId": chat_id, "openingTail": true });
            let mut rx = match handle
                .client()
                .subscribe(methods::WATCH_DOC_MESSAGES, params)
                .await
            {
                Ok(rx) => rx,
                Err(err) => {
                    tracing::warn!(%chat_id, error = %err, "transcript watch failed; retrying");
                    if this.update(cx, |_, _| {}).is_err() {
                        return;
                    }
                    cx.background_executor().timer(RETRY_DELAY).await;
                    continue 'resubscribe;
                }
            };
            let mut preparation = WatchPreparation::new(cx.background_executor().clone());
            while let Some(value) = rx.recv().await {
                let history_pending = value
                    .get("historyPending")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let decoded = cx
                    .background_executor()
                    .spawn(async move {
                        let update: zeron_doc::TranscriptUpdate =
                            serde_json::from_value(value).map_err(|e| e.to_string())?;
                        let prepared = preparation.prepare(&update).map_err(|e| e.to_string())?;
                        Ok::<_, String>((update, prepared, preparation))
                    })
                    .await;
                let (update, prepared, next_preparation) = match decoded {
                    Ok(frame) => frame,
                    Err(err) => {
                        // Schema skew (a newer peer's entry shape arriving
                        // through sync): a skipped frame is a silently stale
                        // copy, so resubscribe for a fresh reset — delayed,
                        // in case the reset itself is what can't parse.
                        tracing::warn!(error = %err, "malformed transcript frame; resubscribing");
                        cx.background_executor().timer(RETRY_DELAY).await;
                        continue 'resubscribe;
                    }
                };
                preparation = next_preparation;
                let mut desync = false;
                let alive = this.update(cx, |state, cx| {
                    // Guard against a stale pump racing a newer selection.
                    if state.selected_chat.as_deref() == Some(chat_id.as_str()) {
                        if history_pending && state.transcript_replayed {
                            return;
                        }
                        if let Err(err) =
                            state.receive_opening_transcript_update(update, history_pending, cx)
                        {
                            tracing::warn!(%chat_id, error = %err, "resubscribing transcript");
                            desync = true;
                        } else {
                            state.prepared_transcripts.insert(chat_id.clone(), prepared);
                        }
                    }
                });
                if alive.is_err() {
                    return;
                }
                if desync {
                    continue 'resubscribe;
                }
            }
            // Stream ended: engine restart, RPC drop, or chat purge. Retry;
            // the purge case is cleaned up by apply_chats dropping this task.
            tracing::debug!(%chat_id, "transcript stream ended; resubscribing");
            if this.update(cx, |_, _| {}).is_err() {
                return;
            }
            cx.background_executor().timer(RETRY_DELAY).await;
        }
    })
}

/// The selected chat's pending-message queue, straight off its doc. Whole-list
/// frames (the queue is a handful of rows at most), retried like the transcript
/// watch — a queue that silently stopped updating would show messages the host
/// has already sent.
pub(super) fn spawn_queue_watch(
    cx: &mut Context<AppState>,
    handle: EngineHandle,
    chat_id: String,
) -> Task<()> {
    #[derive(serde::Deserialize)]
    struct QueueFrame {
        #[serde(default)]
        items: Vec<zeron_doc::QueuedMessage>,
    }
    cx.spawn(async move |this, cx| {
        const RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(2);
        'resubscribe: loop {
            let params = serde_json::json!({ "chatId": chat_id });
            let mut rx = match handle
                .client()
                .subscribe(methods::WATCH_QUEUE, params)
                .await
            {
                Ok(rx) => rx,
                Err(err) => {
                    tracing::debug!(%chat_id, error = %err, "queue watch failed; retrying");
                    if this.update(cx, |_, _| {}).is_err() {
                        return;
                    }
                    cx.background_executor().timer(RETRY_DELAY).await;
                    continue 'resubscribe;
                }
            };
            while let Some(value) = rx.recv().await {
                let frame: QueueFrame = match serde_json::from_value(value) {
                    Ok(frame) => frame,
                    Err(err) => {
                        tracing::warn!(error = %err, "dropping malformed queue frame");
                        continue;
                    }
                };
                let alive = this.update(cx, |state, cx| {
                    // Guard against a stale pump racing a newer selection.
                    if state.selected_chat.as_deref() == Some(chat_id.as_str()) {
                        state.queue = frame.items;
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    return;
                }
            }
            if this.update(cx, |_, _| {}).is_err() {
                return;
            }
            cx.background_executor().timer(RETRY_DELAY).await;
        }
    })
}

/// [`spawn_transcript_watch`]'s shape, writing into `sub_transcripts[doc_id]`
/// instead of the selected chat's transcript. The apply guard is per key so a
/// subagent tab can outlive chat switches.
pub(super) fn spawn_subagent_watch(
    cx: &mut Context<AppState>,
    handle: EngineHandle,
    doc_id: String,
) -> Task<()> {
    cx.spawn(async move |this, cx| {
        const RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(2);
        'resubscribe: loop {
            let params = serde_json::json!({ "chatId": doc_id });
            let mut rx = match handle
                .client()
                .subscribe(methods::WATCH_DOC_MESSAGES, params)
                .await
            {
                Ok(rx) => rx,
                Err(err) => {
                    tracing::warn!(%doc_id, error = %err, "subagent watch failed; retrying");
                    if this.update(cx, |_, _| {}).is_err() {
                        return;
                    }
                    cx.background_executor().timer(RETRY_DELAY).await;
                    continue 'resubscribe;
                }
            };
            let mut preparation = WatchPreparation::new(cx.background_executor().clone());
            while let Some(value) = rx.recv().await {
                let decoded = cx
                    .background_executor()
                    .spawn(async move {
                        let update: zeron_doc::TranscriptUpdate =
                            serde_json::from_value(value).map_err(|e| e.to_string())?;
                        let prepared = preparation.prepare(&update).map_err(|e| e.to_string())?;
                        Ok::<_, String>((update, prepared, preparation))
                    })
                    .await;
                let (update, prepared, next_preparation) = match decoded {
                    Ok(frame) => frame,
                    Err(err) => {
                        tracing::warn!(error = %err, "malformed subagent frame; resubscribing");
                        cx.background_executor().timer(RETRY_DELAY).await;
                        continue 'resubscribe;
                    }
                };
                preparation = next_preparation;
                let mut desync = false;
                let alive = this.update(cx, |state, cx| {
                    // A stale pump racing a snapshot/unwatch finds no key.
                    if let Some(rows) = state.sub_transcripts.get_mut(&doc_id) {
                        let frame = update.frame;
                        let text_only = is_text_append(&frame);
                        state.transcript_revision = state.transcript_revision.wrapping_add(1);
                        if let Err(err) = zeron_doc::apply_transcript_frame(rows, frame) {
                            tracing::warn!(%doc_id, error = %err, "resubscribing subagent watch");
                            desync = true;
                        }
                        if !desync {
                            state.prepared_transcripts.insert(doc_id.clone(), prepared);
                        }
                        if !desync && let Some(baseline) = update.replay_baseline {
                            state
                                .transcript_baselines
                                .insert(doc_id.clone(), Arc::new(baseline));
                        }
                        if text_only && !desync {
                            cx.emit(TranscriptTextChanged {
                                doc_id: doc_id.clone(),
                            });
                        } else {
                            cx.notify();
                        }
                    }
                });
                if alive.is_err() {
                    return;
                }
                if desync {
                    continue 'resubscribe;
                }
            }
            tracing::debug!(%doc_id, "subagent stream ended; resubscribing");
            if this.update(cx, |_, _| {}).is_err() {
                return;
            }
            cx.background_executor().timer(RETRY_DELAY).await;
        }
    })
}
