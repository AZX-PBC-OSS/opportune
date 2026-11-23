//! Blocked time: the occupied spans of a participant's blocking blocks, each
//! widened by the applicable clearance, and the blackout minutes they sit among.

use std::cmp::Ordering;
use std::sync::Arc;

use crate::exclude::ExcludedKeys;
use crate::intervals::{IntervalSet, Span as MinuteSpan};
use crate::model::{Block, Participant};
use crate::rules::Applicable;

/// One blocking event, as the eligibility decision sees it.
pub(crate) struct BlockedSpan<'a> {
    /// The block's own occupied span, which is what `blocked` judges.
    pub(crate) occupied: MinuteSpan,
    /// That span widened by the clearance, which is what `clearance` judges.
    pub(crate) widened: MinuteSpan,
    /// The block itself, for the reason's event key and tags.
    pub(crate) block: &'a Block,
}

/// A participant's blocking events in canonical order, augmented with prefix
/// maximum endpoints so overlap and clearance queries binary-search past
/// earlier blocks in `O(log B)` instead of scanning from index 0.
pub(crate) struct BlockedIndex<'a> {
    spans: Vec<BlockedSpan<'a>>,
    /// `max_occupied_end[i]` is the largest `occupied.end()` in `spans[..=i]`.
    max_occupied_end: Vec<i64>,
    /// `max_widened_end[i]` is the largest `widened.end()` in `spans[..=i]`.
    max_widened_end: Vec<i64>,
}

impl<'a> BlockedIndex<'a> {
    /// Every blocking event in canonical order.
    pub(crate) fn spans(&self) -> &[BlockedSpan<'a>] {
        &self.spans
    }

    /// The first blocking event (in canonical order) whose own occupied span
    /// overlaps `span`.
    pub(crate) fn first_overlapping_occupied(&self, span: &MinuteSpan) -> Option<&BlockedSpan<'a>> {
        let first = self
            .max_occupied_end
            .partition_point(|&max_end| max_end <= span.start());
        for blocked in &self.spans[first..] {
            // `spans` is sorted by `occupied.start()`, so no later block can
            // overlap `span`.
            if blocked.occupied.start() >= span.end() {
                break;
            }
            if overlaps(&blocked.occupied, span) {
                return Some(blocked);
            }
        }
        None
    }

    /// Blocking events whose clearance-widened span may overlap `span`, in
    /// canonical order.
    pub(crate) fn widened_candidates(
        &self,
        span: &MinuteSpan,
    ) -> impl Iterator<Item = &BlockedSpan<'a>> {
        let first = self
            .max_widened_end
            .partition_point(|&max_end| max_end <= span.start());
        let end = span.end();
        // Every block for one participant is widened by the same `at_start`, so
        // `widened.start()` is monotonically non-decreasing too.
        self.spans[first..]
            .iter()
            .take_while(move |blocked| blocked.widened.start() < end)
    }
}

/// The occupied spans of the participant's blocking blocks, each widened by the
/// applicable clearance, less the blocks `excluded` names.
///
/// A block's occupied span is its own span widened by its travel. Clearance
/// widens it further, at the end by the larger of the booking's `before` gap and
/// `min_gap` and at the start by the larger of its `after` gap and `min_gap` —
/// the largest that applies, never a sum. An excluded block is the event being
/// moved: it does not stand in the way of the slot it is moving to, and `rest`
/// reads no activity from it either, because both read the spans built here.
pub(super) fn blocked_time<'a>(
    rules: &Applicable<'_>,
    participant: &'a Participant,
    excluded: &ExcludedKeys<'_>,
) -> BlockedIndex<'a> {
    let at_start = rules.clearance.after.max(rules.clearance.min_gap).minutes();
    let at_end = rules
        .clearance
        .before
        .max(rules.clearance.min_gap)
        .minutes();
    let mut spans = Vec::new();
    for block in participant.blocks() {
        if excluded.excludes(block.event_key.as_deref()) {
            continue;
        }
        if !rules.blocks(block.status, &block.tags) {
            continue;
        }
        let (start, end) = block.occupied_minutes();
        let Ok(occupied) = MinuteSpan::new(start, end) else {
            continue;
        };
        let Ok(widened) =
            MinuteSpan::new(start.saturating_sub(at_start), end.saturating_add(at_end))
        else {
            continue;
        };
        spans.push(BlockedSpan {
            occupied,
            widened,
            block,
        });
    }
    // The calendar order a caller wrote is not a rule of the request: two events
    // whose spans and keys say the same thing must cite the same one whichever
    // order they arrived in.
    spans.sort_by(canonical_block_order);
    let mut max_occupied_end = Vec::with_capacity(spans.len());
    let mut max_widened_end = Vec::with_capacity(spans.len());
    let mut occ_max = i64::MIN;
    let mut wid_max = i64::MIN;
    for blocked in &spans {
        occ_max = occ_max.max(blocked.occupied.end());
        wid_max = wid_max.max(blocked.widened.end());
        max_occupied_end.push(occ_max);
        max_widened_end.push(wid_max);
    }
    BlockedIndex {
        spans,
        max_occupied_end,
        max_widened_end,
    }
}

/// Cites blocking events in an order that does not depend on the calendar they
/// arrived in: by the span each occupies, then by the caller's event key, then by
/// the tags the reason reports.
fn canonical_block_order(left: &BlockedSpan<'_>, right: &BlockedSpan<'_>) -> Ordering {
    left.occupied
        .cmp(&right.occupied)
        .then_with(|| left.block.event_key.cmp(&right.block.event_key))
        .then_with(|| left.block.tags.cmp(&right.block.tags))
}

/// The earliest blackout whose minutes overlap `span`.
pub(super) fn blackout_overlapping<'a>(
    blackouts: &[(&'a str, Arc<IntervalSet>)],
    span: &MinuteSpan,
) -> Option<&'a str> {
    blackouts
        .iter()
        .find(|(_, set)| set.overlaps(span))
        .map(|(id, _)| *id)
}

/// The first blocking event whose own occupied span overlaps `span`.
pub(super) fn block_overlapping<'a, 'b>(
    blocked: &'b BlockedIndex<'a>,
    span: &MinuteSpan,
) -> Option<&'b BlockedSpan<'a>> {
    blocked.first_overlapping_occupied(span)
}

/// Whether two half-open minute spans share a minute. Touching is not
/// overlapping: a span ending where another begins shares none.
pub(super) fn overlaps(left: &MinuteSpan, right: &MinuteSpan) -> bool {
    left.start() < right.end() && right.start() < left.end()
}
