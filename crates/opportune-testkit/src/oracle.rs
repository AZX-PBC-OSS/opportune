//! Naive, point-by-point reference models for the engine's in-house
//! primitives.
//!
//! The oracle re-implements the engine's semantics without the engine's
//! in-house interval and matching code — here as sets of individual minutes —
//! so a bug in either surfaces as a disagreement with the other rather than a
//! shared blind spot.

use std::collections::BTreeSet;

/// A set of whole minutes, stored as the minutes themselves: the point-by-point
/// reference model for the engine's coalesced half-open span sets.
///
/// The model is deliberately naive — membership tests and set algebra are
/// per-minute, and the canonical span form is recomputed by scanning the
/// sorted minutes — so it shares no code and no shortcuts with the engine's
/// [`IntervalSet`](opportune::intervals::IntervalSet).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MinuteSet {
    minutes: BTreeSet<i64>,
}

impl MinuteSet {
    /// The empty set.
    #[must_use]
    pub fn new() -> Self {
        Self {
            minutes: BTreeSet::new(),
        }
    }

    /// Adds every minute of the half-open span `[start, end)`. A caller must
    /// pass `start < end`, as the engine's span constructor guarantees.
    pub fn insert_span(&mut self, start: i64, end: i64) {
        for minute in start..end {
            self.minutes.insert(minute);
        }
    }

    /// Whether `minute` is in the set.
    #[must_use]
    pub fn contains(&self, minute: i64) -> bool {
        self.minutes.contains(&minute)
    }

    /// The minutes covered by either set.
    #[must_use]
    pub fn union(&self, other: &Self) -> Self {
        Self {
            minutes: self.minutes.union(&other.minutes).copied().collect(),
        }
    }

    /// The minutes covered by both sets.
    #[must_use]
    pub fn intersection(&self, other: &Self) -> Self {
        Self {
            minutes: self.minutes.intersection(&other.minutes).copied().collect(),
        }
    }

    /// The minutes covered by this set and not by `other`.
    #[must_use]
    pub fn difference(&self, other: &Self) -> Self {
        Self {
            minutes: self
                .minutes
                .iter()
                .copied()
                .filter(|minute| !other.minutes.contains(minute))
                .collect(),
        }
    }

    /// Widens every contributing span by `before` minutes at its start and
    /// `after` minutes at its end: `minute` is in the result exactly when some
    /// span of this set overlaps `[minute - before, minute + after)`'s
    /// contribution, i.e. when widening one of this set's spans puts it there.
    #[must_use]
    pub fn widened(&self, before: i64, after: i64) -> Self {
        let mut widened = Self::new();
        for (start, end) in self.spans() {
            widened.insert_span(start - before, end + after);
        }
        widened
    }

    /// Whether the set covers every minute of the half-open span
    /// `[start, end)`.
    #[must_use]
    pub fn contains_span(&self, start: i64, end: i64) -> bool {
        (start..end).all(|minute| self.minutes.contains(&minute))
    }

    /// Whether the set and the half-open span `[start, end)` share a minute.
    #[must_use]
    pub fn overlaps_span(&self, start: i64, end: i64) -> bool {
        (start..end).any(|minute| self.minutes.contains(&minute))
    }

    /// The set as sorted, coalesced half-open spans, recomputed by grouping
    /// runs of consecutive minutes.
    #[must_use]
    pub fn spans(&self) -> Vec<(i64, i64)> {
        let mut spans: Vec<(i64, i64)> = Vec::new();
        for &minute in &self.minutes {
            match spans.last_mut() {
                Some((_, end)) if *end == minute => *end = minute + 1,
                _ => spans.push((minute, minute + 1)),
            }
        }
        spans
    }
}
