//! Background transcript preparation, entry fingerprinting, and row caching.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use skylark_doc::{MessagePart, MessageStatus, SessionMessageEntry, SubagentStatus};

use crate::markdown::parser::{parse_full, BlockTree, IncrementalParser};

use super::row::{fnv1a, parse_for_row, rows_for_entry, Row};
use super::Transcript;

/// Expensive presentation work belongs to the subscription's background job,
/// never to a GPUI observer/render callback. Shared rows survive navigation.
#[derive(Default)]
pub(crate) struct TranscriptPreparation {
    entries: Vec<SessionMessageEntry>,
    cache: HashMap<String, Arc<Vec<Row>>>,
    live_parsers: HashMap<String, IncrementalParser>,
    tree_cache: HashMap<String, (usize, Arc<BlockTree>)>,
    baseline: Option<skylark_doc::TranscriptBaseline>,
}

pub(crate) struct PreparedTranscript {
    pub(crate) rows: HashMap<String, Arc<Vec<Row>>>,
    pub(crate) historical: HashMap<String, Vec<Row>>,
    pub(crate) fully_historical: HashSet<String>,
    pub(crate) navigation_baseline: Arc<skylark_doc::TranscriptBaseline>,
    pub(crate) bytes: usize,
}

impl TranscriptPreparation {
    pub(crate) fn prepare(
        &mut self,
        update: &skylark_doc::TranscriptUpdate,
    ) -> Result<Arc<PreparedTranscript>, skylark_doc::TranscriptDesync> {
        match &update.frame {
            skylark_doc::TranscriptFrame::Reset { .. } => {
                self.cache.clear();
                self.tree_cache.clear();
                self.live_parsers.clear();
            }
            skylark_doc::TranscriptFrame::Delta {
                upsert,
                append,
                remove,
                ..
            } => {
                if !upsert.is_empty() {
                    self.tree_cache.clear();
                }
                for id in upsert
                    .iter()
                    .map(|u| &u.entry.id)
                    .chain(append.iter().map(|a| &a.entry))
                    .chain(remove.iter())
                {
                    self.cache.remove(id);
                }
            }
        }
        skylark_doc::apply_transcript_frame(&mut self.entries, update.frame.clone())?;
        if let Some(baseline) = &update.replay_baseline {
            self.baseline = Some(baseline.clone());
        }
        let mut rows = HashMap::new();
        let mut historical = HashMap::new();
        let mut fully_historical = HashSet::new();
        let mut bytes = 0;
        for entry in &self.entries {
            let built = if let Some(rows) = self.cache.get(&entry.id) {
                rows.clone()
            } else {
                let streaming = entry.status == Some(MessageStatus::Streaming);
                let built = Arc::new(rows_for_entry(entry, false, &mut |key, text| {
                    parse_for_row(
                        streaming,
                        key,
                        text,
                        &mut self.live_parsers,
                        &mut self.tree_cache,
                    )
                    .0
                }));
                self.cache.insert(entry.id.clone(), built.clone());
                built
            };
            rows.insert(entry.id.clone(), built);
            if self.baseline.as_ref().is_some_and(|b| b.covers(entry)) {
                fully_historical.insert(entry.id.clone());
            }
            if let Some(baseline) = &self.baseline
                && !fully_historical.contains(&entry.id)
                && let Some(prefix) = baseline.historical_entry(entry)
            {
                historical.insert(
                    entry.id.clone(),
                    rows_for_entry(&prefix, false, &mut |_, text| Arc::new(parse_full(text))),
                );
            }
            bytes += std::mem::size_of::<SessionMessageEntry>()
                + entry.id.len()
                + entry
                    .parts
                    .iter()
                    .map(|part| std::mem::size_of::<MessagePart>() + part.byte_len())
                    .sum::<usize>();
        }
        self.cache.retain(|id, _| rows.contains_key(id));
        Ok(Arc::new(PreparedTranscript {
            rows,
            historical,
            fully_historical,
            bytes,
            navigation_baseline: Arc::new(skylark_doc::TranscriptBaseline::capture(&self.entries)),
        }))
    }
}

pub(crate) struct CachedRows {
    pub(crate) fingerprint: u64,
    pub(crate) rows: Vec<Row>,
}

pub(crate) fn entry_fingerprint(entry: &SessionMessageEntry, pending: bool) -> u64 {
    let mut acc: Vec<u8> = Vec::with_capacity(entry.parts.len() * 8 + 16);
    acc.extend_from_slice(entry.id.as_bytes());
    acc.extend_from_slice(entry.device_id.as_bytes());
    acc.push(match entry.status {
        None => 0,
        Some(MessageStatus::Streaming) => 1,
        Some(MessageStatus::Complete) => 2,
        Some(MessageStatus::Aborted) => 3,
    });
    acc.push(pending as u8);
    for part in &entry.parts {
        acc.extend_from_slice(part.id().as_bytes());
        acc.extend_from_slice(&(part.byte_len() as u64).to_le_bytes());
        if let MessagePart::Tool {
            is_error,
            resolved,
            subagent_ref,
            subagent_status,
            subagent_tail,
            ..
        } = part
        {
            acc.push(*is_error as u8 | (*resolved as u8) << 1);
            // Subagent lifecycle mutates a COMPLETED entry in place (eager-
            // done: the spawn resolves while the subagent runs on) and
            // `byte_len` above doesn't cover these fields — hash them or the
            // cached rows never refresh on status/tail changes.
            acc.push(
                subagent_ref.is_some() as u8
                    | match subagent_status {
                        None => 0,
                        Some(SubagentStatus::Running) => 1 << 1,
                        Some(SubagentStatus::Done) => 2 << 1,
                        Some(SubagentStatus::Failed) => 3 << 1,
                    },
            );
            if let Some(tail) = subagent_tail {
                acc.extend_from_slice(tail.as_bytes());
            }
        }
        if let MessagePart::Image {
            path,
            name,
            mime_type,
            ..
        } = part
        {
            for field in [path, name, mime_type] {
                acc.extend_from_slice(field.as_bytes());
                acc.push(0);
            }
        }
        if let MessagePart::Input { resolved, .. } = part {
            acc.push(0x10 | *resolved as u8);
        }
    }
    fnv1a(&acc)
}

impl Transcript {
    /// Cached row build for one entry (streaming entries bypass the cache).
    pub(super) fn rows_for(&mut self, entry: &SessionMessageEntry, pending: bool) -> Vec<Row> {
        let streaming = entry.status == Some(MessageStatus::Streaming);
        // Live entries always rebuild; don't allocate a fingerprint that the
        // streaming path cannot use.
        let fingerprint = if streaming {
            0
        } else {
            entry_fingerprint(entry, pending)
        };
        if !streaming
            && let Some(cached) = self.row_cache.get(&entry.id)
            && cached.fingerprint == fingerprint
        {
            return cached.rows.clone();
        }

        let live_parsers = &mut self.live_parsers;
        let tree_cache = &mut self.tree_cache;
        let mut parse = |key: &str, text: &str| -> Arc<BlockTree> {
            // Render-cache invalidation rides on the row diff in `sync` (only
            // rows whose content hash changed are spliced — the reparsed tail).
            parse_for_row(streaming, key, text, live_parsers, tree_cache).0
        };
        let rows = rows_for_entry(entry, pending, &mut parse);

        if !streaming {
            self.row_cache.insert(
                entry.id.clone(),
                CachedRows {
                    fingerprint,
                    rows: rows.clone(),
                },
            );
        }

        rows
    }
}
