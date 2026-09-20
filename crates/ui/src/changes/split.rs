use crate::comments::CommentSide;
use super::{DiffLine, LineKind};

/// One split row: indices into the hunk's lines for the left (old) and right
/// (new) column. `None` on a side means that column is empty for this row.
pub type LinePair = (Option<u32>, Option<u32>);

/// Pair a hunk's lines into split rows.
///
/// A hunk reads as runs: context lines sit on both sides, and each run of
/// deletions immediately followed by additions is a *change block* whose two
/// sides line up index-for-index (the shape git already emits — an edited
/// line's `-`/`+` are adjacent). The longer side's leftovers get one-sided
/// rows, so a 3-for-1 rewrite is 1 paired row and 2 add-only rows rather than
/// a ragged interleave. A deletion arriving after additions opens a new block
/// (`-a +b -c +d` is two edits, not one four-line one).
///
/// Pure and index-only: the caller keeps owning the lines, and the result is
/// small enough to live in the row model.
pub fn split_pairs(lines: &[DiffLine]) -> Vec<LinePair> {
    split_pairs_upto(lines, usize::MAX)
}

/// [`split_pairs`], stopping at `max_rows`.
///
/// The fold tween's stand-in builds only the slice its clip can reveal and
/// re-renders every frame of the tween, so it must not pay to pair a 50k-line
/// hunk to draw twenty rows of it. Bounding the *output* is not enough — the
/// pending runs are bounded too, since a change block yields
/// `max(dels, adds)` rows and so anything past the budget can only land past
/// it as well.
pub fn split_pairs_upto(lines: &[DiffLine], max_rows: usize) -> Vec<LinePair> {
    /// The block being accumulated: the two sides' code lines, plus the
    /// `\ No newline…` marker each side may end on.
    #[derive(Default)]
    struct Block {
        dels: Vec<u32>,
        adds: Vec<u32>,
        del_meta: Vec<u32>,
        add_meta: Vec<u32>,
    }

    fn flush(pairs: &mut Vec<LinePair>, block: &mut Block, max_rows: usize) {
        let mut drain = |left: &mut Vec<u32>, right: &mut Vec<u32>| {
            for ix in 0..left.len().max(right.len()) {
                if pairs.len() >= max_rows {
                    break;
                }
                pairs.push((left.get(ix).copied(), right.get(ix).copied()));
            }
            left.clear();
            right.clear();
        };
        drain(&mut block.dels, &mut block.adds);
        // Markers trail the code they annotate, and pair with each other — a
        // modification where both files lost their final newline is one
        // aligned row plus one marker row, not two one-sided rows plus two
        // markers. They never share a row with code, so both render arms can
        // treat a marker on either side as spanning the row.
        drain(&mut block.del_meta, &mut block.add_meta);
    }

    let mut pairs = Vec::with_capacity(lines.len().min(max_rows));
    let mut block = Block::default();
    let mut pending_side: Option<LineKind> = None;
    for (ix, line) in lines.iter().enumerate() {
        match line.kind {
            LineKind::Del => {
                // A marker already closes its side, so code arriving after one
                // starts a fresh block — the marker row keeps its place in the
                // file's order.
                if !block.adds.is_empty()
                    || !block.del_meta.is_empty()
                    || !block.add_meta.is_empty()
                {
                    flush(&mut pairs, &mut block, max_rows);
                }
                let remaining = max_rows - pairs.len().min(max_rows);
                if remaining == 0 {
                    break;
                }
                if block.dels.len() < remaining {
                    block.dels.push(ix as u32);
                }
                pending_side = Some(LineKind::Del);
            }
            LineKind::Add => {
                // The old side's marker is the one case where a marker does
                // not close the block: `-old`, marker, `+new` is one edit.
                if !block.add_meta.is_empty() {
                    flush(&mut pairs, &mut block, max_rows);
                }
                let remaining = max_rows - pairs.len().min(max_rows);
                if remaining == 0 {
                    break;
                }
                if block.adds.len() < remaining {
                    block.adds.push(ix as u32);
                }
                pending_side = Some(LineKind::Add);
            }
            // `\ No newline at end of file` belongs to the side whose line it
            // follows, so it must NOT close the block: git writes `-old`,
            // marker, `+new`, marker for an edited last line, and treating
            // either marker as a boundary would tear that edit apart. A marker
            // after context describes the same line on both sides.
            LineKind::Meta => match pending_side {
                Some(LineKind::Del) => block.del_meta.push(ix as u32),
                Some(LineKind::Add) => block.add_meta.push(ix as u32),
                _ => {
                    block.del_meta.push(ix as u32);
                    block.add_meta.push(ix as u32);
                }
            },
            // Context sits on both sides.
            LineKind::Context => {
                flush(&mut pairs, &mut block, max_rows);
                if pairs.len() >= max_rows {
                    break;
                }
                pairs.push((Some(ix as u32), Some(ix as u32)));
                pending_side = Some(LineKind::Context);
            }
        }
    }
    flush(&mut pairs, &mut block, max_rows);
    pairs.truncate(max_rows);
    pairs
}

/// Every anchor a split row can *display* a card for, left column first.
///
/// Wider than what the row lets you write: only the right column takes a `+`
/// (see the `SplitLine` render arm), but an old-side note staged from the
/// unified layout must still show its card here, or toggling layouts would
/// look like it dropped one. A context row names the same anchor on both
/// sides, so the duplicate is dropped — the caller flattens, and two
/// identical anchors would stage the card twice. A fixed array, not a `Vec`:
/// this runs per row of every re-flatten.
pub(crate) fn pair_anchors(lines: &[DiffLine], pair: LinePair) -> [Option<(CommentSide, u32)>; 2] {
    let anchor = |ix: Option<u32>| {
        ix.and_then(|ix| lines.get(ix as usize))
            .and_then(line_anchor)
    };
    let (left, right) = (anchor(pair.0), anchor(pair.1));
    if left == right {
        [left, None]
    } else {
        [left, right]
    }
}

/// A deletion only exists in the pre-change file; everything else is cited
/// against the post-change file, which is what the agent edits.
pub fn line_anchor(line: &DiffLine) -> Option<(CommentSide, u32)> {
    match line.kind {
        LineKind::Meta => None,
        LineKind::Del => line.old_no.map(|no| (CommentSide::Old, no)),
        _ => line.new_no.map(|no| (CommentSide::New, no)),
    }
}
