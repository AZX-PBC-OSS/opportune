//! The documented two-phase fetch model: an initial hull fetch, then one round
//! of event-key closure.
//!
//! [`RequiredRanges`] says which data a cap needs, not that the hull is the whole
//! query: a cap merges every record sharing an event key across all supplied time
//! ranges, so a caller must fetch the initial hulls and then every record carrying
//! a key observed on an own-span occurrence-range record in that first fetch, on
//! every request participant. A one-pass
//! fetch is key-complete only under the caller-enforced identical-own-span
//! invariant, where every copy of an event has the same own span.
//!
//! [`key_closed_request`] models that contract over a full request standing in for
//! the provider's answer. It is the oracle the range-strip properties compare
//! against and lives only in this unpublished crate; the engine never fetches.
//!
//! [`RequiredRanges`]: opportune::model::RequiredRanges

use std::collections::BTreeSet;

use opportune::model::{Block, Participant, Request, RequiredRanges, Span};

/// The request a caller holds after the initial hulls alone: the one-pass fetch
/// the `RequiredRanges` documentation warns against.
///
/// Kept beside [`key_closed_request`] so a regression can prove the two disagree
/// rather than assert against a scenario that never exercised the difference.
#[must_use]
pub fn one_pass_request(request: &Request, ranges: &RequiredRanges) -> Request {
    let mut fetched = request.clone();
    let calendar = ranges.calendar;
    let Some(occurrences) = ranges.occurrences else {
        // No cap applies, so no occurrence data is fetched at all.
        fetched.occurrences.clear();
        for participant in &mut fetched.participants {
            if let Some(blocks) = &mut participant.calendar {
                blocks.retain(|block| occupied_overlaps(block, calendar));
            }
        }
        return fetched;
    };
    for participant in &mut fetched.participants {
        if let Some(blocks) = &mut participant.calendar {
            blocks.retain(|block| {
                occupied_overlaps(block, calendar) || overlaps(block.span, occurrences)
            });
        }
    }
    fetched
        .occurrences
        .retain(|occurrence| overlaps(occurrence.span, occurrences));
    fetched
}

/// The request after the second phase of the documented fetch contract.
///
/// The caller fetches the initial hulls, collects every event key observed among
/// the own-span occurrence-range records of every participant, and fetches every
/// record carrying one of those keys — all-day and otherwise nonmatching records
/// included. The result preserves record identity and request order, keeps an
/// unknown calendar (`participant.calendar == None`) exactly as it was, and clears
/// `occurrences` when no occurrence hull exists. One round settles because every
/// record carries at most one event key.
#[must_use]
pub fn key_closed_request(request: &Request, ranges: &RequiredRanges) -> Request {
    let fetched = one_pass_request(request, ranges);
    let Some(occurrences) = ranges.occurrences else {
        return fetched;
    };
    // Every event key the occurrence-range fetch observed, across every
    // participant — attendees and participants no cap covers included. Only
    // own-span occurrence-range records seed closure; a block the calendar hull
    // alone retained does not broaden the key set beyond the contract.
    let observed: BTreeSet<String> = fetched
        .occurrences
        .iter()
        .map(|occurrence| occurrence.event_key.clone())
        .chain(
            fetched
                .participants
                .iter()
                .flat_map(Participant::blocks)
                .filter(|block| overlaps(block.span, occurrences))
                .filter_map(|block| block.event_key.clone()),
        )
        .collect();

    let mut closed = request.clone();
    for participant in &mut closed.participants {
        if let Some(blocks) = &mut participant.calendar {
            blocks.retain(|block| {
                occupied_overlaps(block, ranges.calendar)
                    || overlaps(block.span, occurrences)
                    || block
                        .event_key
                        .as_ref()
                        .is_some_and(|key| observed.contains(key))
            });
        }
    }
    // The second disjunct is load-bearing: an occurrence outside the hull is still
    // authoritative for its event's span, so dropping it re-buckets a same-key
    // in-hull block into the candidate's own bucket. Pinned by
    // `tests/fetch_occurrence_closure_test.rs`.
    closed.occurrences.retain(|occurrence| {
        overlaps(occurrence.span, occurrences) || observed.contains(&occurrence.event_key)
    });
    closed
}

/// Whether `block`'s occupied span — its own span widened by its travel —
/// overlaps `hull`.
fn occupied_overlaps(block: &Block, hull: Span) -> bool {
    let start = block
        .span
        .start()
        .minutes()
        .saturating_sub(block.travel_before.minutes());
    let end = block
        .span
        .end()
        .minutes()
        .saturating_add(block.travel_after.minutes());
    start < hull.end().minutes() && hull.start().minutes() < end
}

/// Whether the half-open `span` intersects the half-open `hull`.
fn overlaps(span: Span, hull: Span) -> bool {
    span.start() < hull.end() && hull.start() < span.end()
}
