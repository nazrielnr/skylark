//! Incremental Markdown parsing and streaming repair.

use super::*;

// ---------------------------------------------------------------------------
// Incremental parse
// ---------------------------------------------------------------------------

/// Streaming parser: appends reparse only from the last stable top-level block
/// boundary (snapped back to a line start so indentation context survives).
#[derive(Debug, Default)]
pub struct IncrementalParser {
    source: String,
    tree: BlockTree,
    /// Display-only replacement for the last top-level block when its source
    /// has hanging inline markers ([`super::mend`]): `None` means the display
    /// tree is exactly [`Self::tree`]. Never fed back into the incremental
    /// state — the canonical tree stays parity-exact with `parse_full`.
    display_tail: Option<Vec<Arc<TopBlock>>>,
    /// Link-reference definitions act at a distance — full reparses only.
    full_only: bool,
    /// Bytes fed through `parse_full` by the most recent `set_text`/`append`/
    /// `reset` — instrumentation proving per-append work is O(tail), not
    /// O(total). 0 for a no-op set_text.
    last_parse_bytes: usize,
    /// Number of leading top-level blocks guaranteed untouched by the most
    /// recent update (render caches for these blocks stay valid).
    stable_prefix_blocks: usize,
}

impl IncrementalParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn tree(&self) -> &BlockTree {
        &self.tree
    }

    /// The tree to render while streaming: the canonical tree with the last
    /// block swapped for its mended parse when inline markers hang (an
    /// unclosed `**bold`, a half-streamed `[link](url…`). Same shape and cost
    /// as `tree().clone()` — the stable prefix shares its blocks; only a
    /// hanging tail adds one O(tail) reparse, done at append time.
    pub fn display_tree(&self) -> BlockTree {
        let Some(tail) = &self.display_tail else {
            return self.tree.clone();
        };
        let stable = &self.tree.blocks[..self.tree.blocks.len() - 1];
        let mut blocks = Vec::with_capacity(stable.len() + tail.len());
        blocks.extend_from_slice(stable);
        blocks.extend_from_slice(tail);
        BlockTree { blocks }
    }

    /// Bytes actually reparsed by the last update (see field docs).
    pub fn last_parse_bytes(&self) -> usize {
        self.last_parse_bytes
    }

    /// Leading top-level blocks left untouched by the last update.
    pub fn stable_prefix_blocks(&self) -> usize {
        self.stable_prefix_blocks
    }

    /// Set the source: appends take the incremental path, anything else resets.
    pub fn set_text(&mut self, text: &str) {
        if text.len() >= self.source.len() && text.starts_with(self.source.as_str()) {
            let delta = &text[self.source.len()..];
            if delta.is_empty() {
                self.last_parse_bytes = 0;
                self.stable_prefix_blocks = self.tree.blocks.len();
                return;
            }
            self.append(delta);
        } else {
            self.reset(text);
        }
    }

    pub fn reset(&mut self, text: &str) {
        self.source = text.to_string();
        self.full_only = has_link_defs(text);
        self.tree = parse_full(text);
        self.last_parse_bytes = text.len();
        self.stable_prefix_blocks = 0;
        self.remend();
    }

    /// Append streamed text, reparsing from the last stable boundary.
    pub fn append(&mut self, delta: &str) {
        if delta.is_empty() {
            self.last_parse_bytes = 0;
            self.stable_prefix_blocks = self.tree.blocks.len();
            return;
        }
        // The delta may complete a line begun earlier — rescan from that line's
        // start when checking for definitions.
        let scan_from = self.source.rfind('\n').map(|i| i + 1).unwrap_or(0);
        self.source.push_str(delta);
        if !self.full_only && has_link_defs(&self.source[scan_from..]) {
            self.full_only = true;
        }
        if self.full_only {
            self.tree = parse_full(&self.source);
            self.last_parse_bytes = self.source.len();
            self.stable_prefix_blocks = 0;
            self.remend();
            return;
        }

        // Stable boundary: start of the SECOND-to-last top-level block, snapped
        // back to its line start (keeps indented-code / fenced-indent context
        // intact). Reparsing the last two blocks — not just the last — covers
        // continuation merges: a trailing paragraph like `3` can become `3.`
        // and fuse into the preceding loose list. Merges cannot cascade
        // further back (a block's separation from its predecessor is decided
        // by its own already-streamed leading bytes), so two blocks suffice;
        // the parity tests stream corpora to hold this invariant.
        let boundary = match self.tree.blocks.len() {
            0 | 1 => 0,
            n => self.tree.blocks[n - 2].range.start,
        };
        let boundary = self.source[..boundary]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0);

        let tail = parse_at(&self.source[boundary..], boundary);
        self.last_parse_bytes = self.source.len() - boundary;
        self.tree.blocks.retain(|b| b.range.start < boundary);
        self.stable_prefix_blocks = self.tree.blocks.len();
        for top in tail.blocks {
            self.tree.blocks.push(top);
        }
        self.remend();
    }

    /// Recompute the display tail: mend hanging inline markers in the last
    /// top-level block (only place they can hang — a blank line settles a
    /// block, and CommonMark keeps unclosed markers literal across it) and
    /// reparse just that block's source. `close_hanging` is one O(last block)
    /// scan and returns `None` when nothing hangs, so the extra parse happens
    /// only while a marker is actually open.
    fn remend(&mut self) {
        self.display_tail = None;
        let Some(last) = self.tree.blocks.last() else {
            return;
        };
        // Code blocks render an unclosed fence verbatim (already stable);
        // rules and tables have no inline tail to mend.
        if matches!(
            last.block,
            Block::CodeBlock { .. } | Block::Rule | Block::Table { .. }
        ) {
            return;
        }
        let start = last.range.start;
        let Some(mended) = crate::markdown::mend::close_hanging(&self.source[start..]) else {
            return;
        };
        // Count toward the O(tail) instrumentation — this is real parse work,
        // in the same bound as the reparse that produced the block.
        self.last_parse_bytes += mended.len();
        let mut tail = parse_at(&mended, start).blocks;
        for top in &mut tail {
            let top = Arc::make_mut(top);
            // Display ranges point back into the unmended source; synthetic
            // closers at the end clamp away.
            top.range.end = top.range.end.min(self.source.len());
        }
        self.display_tail = Some(tail);
    }
}

/// Conservative detector for link-reference-definition lines
/// (`[label]: destination`, up to 3 leading spaces).
fn has_link_defs(text: &str) -> bool {
    text.lines().any(|line| {
        let trimmed = line.trim_start();
        line.len() - trimmed.len() <= 3 && trimmed.starts_with('[') && trimmed.contains("]:")
    })
}
