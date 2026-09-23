//! Transcript, optimistic echo, pending-send, and transfer reducers.

use super::*;

impl AppState {
    pub fn apply_transcript(&mut self, entries: Vec<SessionMessageEntry>) {
        if let Some(id) = &self.selected_chat {
            self.prepared_transcripts.remove(id);
        }
        self.transcript_revision = self.transcript_revision.wrapping_add(1);
        // Doc frames supersede optimistic echoes carrying the same id.
        if let Some(chat_id) = self.selected_chat.as_deref()
            && let Some(echoes) = self.echoes.get_mut(chat_id)
        {
            echoes.retain(|echo| !entries.iter().any(|e| e.id == echo.id));
        }
        self.transcript = entries;
        self.transcript_replayed = true;
        self.ack_pending_send_from_transcript();
    }

    /// Apply a `WatchDocMessages` delta frame in place. `Err` = this copy has
    /// diverged; the watch task resubscribes for a fresh reset.
    pub fn apply_transcript_frame(
        &mut self,
        frame: TranscriptFrame,
    ) -> Result<(), TranscriptDesync> {
        if let Some(id) = &self.selected_chat {
            self.prepared_transcripts.remove(id);
        }
        self.transcript_revision = self.transcript_revision.wrapping_add(1);
        let is_reset = matches!(&frame, TranscriptFrame::Reset { .. });
        skylark_doc::apply_transcript_frame(&mut self.transcript, frame)?;
        if is_reset {
            self.transcript_replayed = true;
        }
        if let Some(chat_id) = self.selected_chat.as_deref()
            && let Some(echoes) = self.echoes.get_mut(chat_id)
        {
            let transcript = &self.transcript;
            echoes.retain(|echo| !transcript.iter().any(|e| e.id == echo.id));
        }
        self.ack_pending_send_from_transcript();
        Ok(())
    }

    /// Apply an incoming frame and invalidate only its affected presentation.
    pub fn receive_transcript_frame(
        &mut self,
        frame: TranscriptFrame,
        cx: &mut Context<Self>,
    ) -> Result<(), TranscriptDesync> {
        let text_doc = self
            .selected_chat
            .as_ref()
            .filter(|id| {
                is_text_append(&frame)
                    && !self.pending_sends.contains_key(*id)
                    && self.pending_echoes().is_empty()
            })
            .cloned();
        let result = self.apply_transcript_frame(frame);
        if let Some(doc_id) = text_doc.filter(|_| result.is_ok()) {
            cx.emit(TranscriptTextChanged { doc_id });
        } else {
            cx.notify();
        }
        result
    }

    pub(crate) fn transcript_baseline(
        &self,
        doc_id: &str,
    ) -> Option<&Arc<skylark_doc::TranscriptBaseline>> {
        self.transcript_baselines.get(doc_id)
    }

    pub fn receive_transcript_update(
        &mut self,
        update: skylark_doc::TranscriptUpdate,
        cx: &mut Context<Self>,
    ) -> Result<(), TranscriptDesync> {
        self.receive_transcript_frame(update.frame, cx)?;
        if let (Some(doc_id), Some(baseline)) = (&self.selected_chat, update.replay_baseline) {
            self.transcript_baselines
                .insert(doc_id.clone(), Arc::new(baseline));
        }
        if self.context_usage != update.context_usage {
            self.context_usage = update.context_usage;
            cx.notify();
        }
        Ok(())
    }

    /// The opt-in opening tail is provisional. Never replace a complete view
    /// with it, and don't treat it as a full reset for caching/scroll anchors.
    pub(crate) fn receive_opening_transcript_update(
        &mut self,
        update: skylark_doc::TranscriptUpdate,
        history_pending: bool,
        cx: &mut Context<Self>,
    ) -> Result<(), TranscriptDesync> {
        if history_pending && self.transcript_replayed {
            return Ok(());
        }
        let old_prepared = self
            .selected_chat
            .as_ref()
            .and_then(|id| self.prepared_transcripts.remove(id));
        let old_entries = if matches!(&update.frame, TranscriptFrame::Reset { .. }) {
            std::mem::take(&mut self.transcript)
        } else {
            Vec::new()
        };
        cx.background_executor()
            .spawn(async move {
                drop((old_prepared, old_entries));
            })
            .detach();
        self.receive_transcript_update(update, cx)?;
        if history_pending {
            self.transcript_replayed = false;
        }
        Ok(())
    }

    /// A subagent doc's current transcript copy (empty until its watch's
    /// replay frame lands, or its frozen snapshot is set).
    pub fn sub_transcript(&self, doc_id: &str) -> &[SessionMessageEntry] {
        self.sub_transcripts
            .get(doc_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Watch a SUBAGENT doc (`WatchDocMessages` works for any doc id).
    /// Single-flight per key; a frozen snapshot already in place wins — the
    /// watch would race the (complete) blob with a possibly-purged live doc.
    pub fn watch_subagent_doc(&mut self, doc_id: String, cx: &mut Context<Self>) {
        if self.sub_watch_tasks.contains_key(&doc_id) {
            return;
        }
        let Some(handle) = self.engine.clone() else {
            return;
        };
        self.sub_transcripts.entry(doc_id.clone()).or_default();
        let task = spawn_subagent_watch(cx, handle, doc_id.clone());
        self.sub_watch_tasks.insert(doc_id, task);
    }

    /// Tab closed: drop the watch task (cancels the engine-side watch and
    /// unpins the doc from the engine LRU) and the rows.
    pub fn unwatch_subagent_doc(&mut self, doc_id: &str) {
        self.transcript_revision = self.transcript_revision.wrapping_add(1);
        self.sub_watch_tasks.remove(doc_id);
        self.sub_transcripts.remove(doc_id);
        self.prepared_transcripts.remove(doc_id);
        self.transcript_baselines.remove(doc_id);
    }

    /// Frozen-blob path: the finished subagent's uploaded transcript, no
    /// watch needed (and any in-flight watch is superseded).
    pub fn set_subagent_snapshot(&mut self, doc_id: String, entries: Vec<SessionMessageEntry>) {
        self.prepared_transcripts.remove(&doc_id);
        self.transcript_revision = self.transcript_revision.wrapping_add(1);
        self.sub_watch_tasks.remove(&doc_id);
        self.transcript_baselines.insert(
            doc_id.clone(),
            Arc::new(skylark_doc::TranscriptBaseline::capture(&entries)),
        );
        self.sub_transcripts.insert(doc_id, entries);
    }

    pub(crate) fn set_prepared_subagent_snapshot(
        &mut self,
        doc_id: String,
        entries: Vec<SessionMessageEntry>,
        prepared: Arc<crate::transcript::PreparedTranscript>,
    ) {
        self.transcript_revision = self.transcript_revision.wrapping_add(1);
        self.sub_watch_tasks.remove(&doc_id);
        self.transcript_baselines
            .insert(doc_id.clone(), prepared.navigation_baseline.clone());
        self.prepared_transcripts.insert(doc_id.clone(), prepared);
        self.sub_transcripts.insert(doc_id, entries);
    }

    /// Add an optimistic user echo (composer send path).
    pub fn push_echo(&mut self, chat_id: &str, entry: SessionMessageEntry) {
        let echoes = self.echoes.entry(chat_id.to_string()).or_default();
        if !echoes.iter().any(|e| e.id == entry.id) {
            echoes.push(entry);
            self.transcript_revision = self.transcript_revision.wrapping_add(1);
        }
    }

    /// Drop an echo (send failed — the prompt returns to the draft).
    pub fn remove_echo(&mut self, chat_id: &str, message_id: &str) {
        if let Some(echoes) = self.echoes.get_mut(chat_id) {
            let previous_len = echoes.len();
            echoes.retain(|e| e.id != message_id);
            if echoes.len() != previous_len {
                self.transcript_revision = self.transcript_revision.wrapping_add(1);
            }
        }
    }

    /// Composer send fired: overlay the chat as Working until the host writes
    /// the user message back into the transcript (or the TTL lapses). A remote
    /// send has no live session row until the host drains the queued command —
    /// that gap read as "no live run" and flashed the Completed dot, and any
    /// phantom Working→Idle edge in it rang the done-chime on send (user
    /// report 2026-08-05).
    pub fn begin_pending_send(&mut self, chat_id: &str, message_id: &str, now: DateTime<Utc>) {
        self.pending_sends.insert(
            chat_id.to_string(),
            PendingSend {
                message_id: message_id.to_string(),
                started: now,
            },
        );
    }

    /// Send failed — drop the overlay so the dot tells the truth again. Only
    /// removes the overlay this message started: a quick resend must not lose
    /// its own overlay to the first send's failure cleanup.
    pub fn end_pending_send(&mut self, chat_id: &str, message_id: &str) {
        if self
            .pending_sends
            .get(chat_id)
            .is_some_and(|p| p.message_id == message_id)
        {
            self.pending_sends.remove(chat_id);
        }
    }

    /// Attachment upload starting: expose its progress to the working label.
    pub fn begin_upload_progress(
        &mut self,
        total: u64,
        done: std::sync::Arc<std::sync::atomic::AtomicU64>,
    ) {
        self.upload_progress = Some(UploadProgress { done, total });
    }

    /// Upload leg over (success or failure) — the label goes back to plain
    /// send/working wording.
    pub fn end_upload_progress(&mut self) {
        self.upload_progress = None;
    }

    /// Percent of the in-flight attachment upload, clamped to 99 — the last
    /// point belongs to the commit + queue, so "100% but still spinning"
    /// never shows. `None` when no upload is in flight (or it's empty).
    pub fn upload_progress_percent(&self) -> Option<u8> {
        let progress = self.upload_progress.as_ref()?;
        if progress.total == 0 {
            return None;
        }
        let done = progress
            .done
            .load(std::sync::atomic::Ordering::Relaxed)
            .min(progress.total);
        Some(((done * 100) / progress.total).min(99) as u8)
    }

    /// A `WatchTransfers` snapshot: the engine-side relay leg's in-flight
    /// queued-attachment transfers, replacing the whole set each frame.
    pub fn apply_transfers(&mut self, transfers: Vec<skylark_proto::TransferProgress>) {
        self.transfers = transfers
            .into_iter()
            .map(|t| (t.upload_id, (t.done, t.total)))
            .collect();
    }

    /// Percent of one queued attachment's relay transfer, by the uploadId
    /// its `pending://{uploadId}/…` ref names. Same 99-clamp as
    /// [`Self::upload_progress_percent`]: the last point belongs to the
    /// commit, so "100% but still spinning" never shows. `None` when no
    /// bytes are moving for that upload (staged-but-waiting, retry backoff,
    /// or done) — the thumbnail falls back to its indeterminate spinner.
    pub fn transfer_percent(&self, upload_id: &str) -> Option<u8> {
        let (done, total) = self.transfers.get(upload_id)?;
        if *total == 0 {
            return None;
        }
        Some(((done.min(total) * 100) / total).min(99) as u8)
    }

    /// Is a send still in flight for this chat (unacked)? Inside the grace
    /// window normally; while the chat's delivery path is degraded the
    /// overlay holds indefinitely — the truth IS "Queued", and silently
    /// expiring back to Idle left a queued send with no visible trace at
    /// all (the 30s→silence hole, 2026-08-19).
    pub fn send_pending(&self, chat_id: &str, now: DateTime<Utc>) -> bool {
        self.pending_sends.get(chat_id).is_some_and(|p| {
            now.signed_duration_since(p.started).num_milliseconds() <= UNDELIVERED_GRACE_MS
                || self.chat_delivery_degraded(chat_id)
        })
    }

    /// The send has sat unadopted past the grace window: surface the
    /// EXPLICIT failed state ("Not delivered — retry") instead of either
    /// faking progress or silently forgetting the send ever happened.
    pub fn send_undelivered(&self, chat_id: &str, now: DateTime<Utc>) -> bool {
        self.pending_sends.get(chat_id).is_some_and(|p| {
            now.signed_duration_since(p.started).num_milliseconds() > UNDELIVERED_GRACE_MS
        })
    }

    /// Retry pressed: restart the grace clock so the overlay returns to its
    /// Sending/Queued phase while the re-kicked delivery runs.
    pub fn retry_pending_send(&mut self, chat_id: &str, now: DateTime<Utc>) {
        if let Some(p) = self.pending_sends.get_mut(chat_id) {
            p.started = now;
        }
    }

    /// When the in-flight send (if any, inside the TTL) was fired — the
    /// elapsed-timer base while the overlay reads as Working. The session
    /// row's `started_at` still belongs to the PREVIOUS turn during this
    /// window, and showing it made a fresh send open at the old turn's
    /// half-hour mark.
    pub fn pending_send_started(&self, chat_id: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.pending_sends
            .get(chat_id)
            .filter(|p| {
                now.signed_duration_since(p.started).num_milliseconds() <= UNDELIVERED_GRACE_MS
                    || self.chat_delivery_degraded(chat_id)
            })
            .map(|p| p.started)
    }

    /// The host executed the queued command iff the sent message's id showed
    /// up in the transcript (it writes the message before — causally with —
    /// the Working status; sessions.rs dispatch paths).
    fn ack_pending_send_from_transcript(&mut self) {
        if let Some(chat_id) = self.selected_chat.as_deref()
            && let Some(pending) = self.pending_sends.get(chat_id)
            && self.transcript.iter().any(|e| e.id == pending.message_id)
        {
            self.pending_sends.remove(chat_id);
        }
    }

    /// Unconfirmed echoes for the selected chat, in send order.
    pub fn pending_echoes(&self) -> &[SessionMessageEntry] {
        self.selected_chat
            .as_deref()
            .and_then(|id| self.echoes.get(id))
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }
}
