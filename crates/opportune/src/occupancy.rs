//! Occupied-range representability: whether a booking and the travel it needs lie
//! inside the minutes the public model can name.

use crate::time::{Duration, Instant};

/// The edge of the supported range an occupied range runs past.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Edge {
    /// The occupied range would reach a minute before `Instant::MIN`.
    Before,
    /// The occupied range would reach a minute at or after `Instant::MAX`.
    After,
}

/// The edge of the supported range the occupied range
/// `[start - before, start + duration + after)` runs past, or `None` when every
/// minute of it has an [`Instant`] naming it.
///
/// A [`Span`](crate::model::Span) is half-open and ends at an `Instant`, so the
/// last minute the model can name is `[Instant::MAX - 1, Instant::MAX)`. A
/// booking, or travel, that reaches the minute beginning at `Instant::MAX` — or
/// one ending before `Instant::MIN` — has no `Span` to report, so a search
/// withholds such a start and a confirmation rejects it rather than offering a
/// booking whose required hold cannot be persisted.
pub(crate) fn overflow(
    start: Instant,
    duration: Duration,
    before: Duration,
    after: Duration,
) -> Option<Edge> {
    if start.minutes().saturating_sub(before.minutes()) < Instant::MIN.minutes() {
        return Some(Edge::Before);
    }
    let end = start.minutes().saturating_add(duration.minutes());
    if end.saturating_add(after.minutes()) > Instant::MAX.minutes() {
        return Some(Edge::After);
    }
    None
}
