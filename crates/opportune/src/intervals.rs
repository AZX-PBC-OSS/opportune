//! Sorted, coalesced sets of half-open minute spans.
//!
//! All arithmetic is in whole minutes (Unix minutes; negative values are
//! before the epoch). A [`Span`] is the half-open range `[start, end)`; an
//! [`IntervalSet`] is a canonical collection of them — strictly sorted,
//! non-overlapping and non-adjacent, so touching spans always coalesce.
//! Every operation preserves the canonical form and every query answers from
//! it, so callers never re-normalise a set and never see two spellings of one
//! membership.
//!
//! ```
//! use opportune::intervals::{IntervalSet, Span};
//!
//! let meetings = IntervalSet::from_spans(vec![
//!     Span::new(510, 600).unwrap(),
//!     Span::new(595, 610).unwrap(), // overlaps the first: the two coalesce
//! ]);
//! assert_eq!(meetings.spans(), [Span::new(510, 610).unwrap()]);
//! assert!(meetings.overlaps(&Span::new(605, 700).unwrap()));
//! assert!(!meetings.overlaps(&Span::new(610, 700).unwrap()));
//! ```

use thiserror::Error;

/// A half-open span of whole minutes, `[start, end)`.
///
/// Spans are non-empty by construction: [`Span::new`] rejects `start >= end`,
/// so every span contains at least one minute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Span {
    start: i64,
    end: i64,
}

impl Span {
    /// Creates the half-open span `[start, end)`.
    ///
    /// ```
    /// # use opportune::intervals::Span;
    /// let span = Span::new(100, 190).unwrap();
    /// assert_eq!(span.start(), 100);
    /// assert_eq!(span.end(), 190);
    /// assert!(span.contains(100));
    /// assert!(!span.contains(190));
    /// ```
    ///
    /// # Errors
    /// Returns [`IntervalError::EmptySpan`] when `start >= end`: non-empty
    /// spans are required, so an empty range must be spelled [`IntervalSet`]
    /// (with no spans), not an empty span.
    pub fn new(start: i64, end: i64) -> Result<Self, IntervalError> {
        if start >= end {
            return Err(IntervalError::EmptySpan { start, end });
        }
        Ok(Self { start, end })
    }

    /// The first minute in the span.
    #[must_use]
    pub const fn start(&self) -> i64 {
        self.start
    }

    /// One past the last minute in the span.
    #[must_use]
    pub const fn end(&self) -> i64 {
        self.end
    }

    /// Whether the span contains `minute`.
    #[must_use]
    pub const fn contains(&self, minute: i64) -> bool {
        self.start <= minute && minute < self.end
    }

    /// Creates a span from endpoints already proven to satisfy `start < end`.
    #[must_use]
    pub(crate) const fn from_valid(start: i64, end: i64) -> Self {
        debug_assert!(start < end);
        Self { start, end }
    }

    /// The smallest span covering both `self` and `other`.
    #[must_use]
    pub(crate) const fn hull(self, other: Self) -> Self {
        let start = if self.start < other.start {
            self.start
        } else {
            other.start
        };
        let end = if self.end > other.end {
            self.end
        } else {
            other.end
        };
        Self { start, end }
    }
}

/// Why an interval construction or operation was rejected.
///
/// The [`IntervalError::code`] values are stable: callers match on them
/// across releases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum IntervalError {
    /// A span was constructed with `start >= end`, which is an empty range.
    #[error("a span must be non-empty: start {start} is not before end {end}")]
    EmptySpan {
        /// The rejected span's start.
        start: i64,
        /// The rejected span's end.
        end: i64,
    },
    /// Widening would push a span outside the representable minute range.
    #[error(
        "widening by {before} minutes before and {after} minutes after overflows the minute range"
    )]
    WidenOverflow {
        /// The requested minutes to widen before each span.
        before: i64,
        /// The requested minutes to widen after each span.
        after: i64,
    },
    /// A widening amount was negative. Widening grows spans; shrinking is
    /// spelled [`IntervalSet::difference`].
    #[error("widening amounts must be non-negative: before {before}, after {after}")]
    NegativeWiden {
        /// The rejected `before` amount.
        before: i64,
        /// The rejected `after` amount.
        after: i64,
    },
}

impl IntervalError {
    /// A stable, machine-readable code for this failure.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::EmptySpan { .. } => "span_empty",
            Self::WidenOverflow { .. } => "span_widen_overflow",
            Self::NegativeWiden { .. } => "span_widen_negative",
        }
    }
}

/// A canonical set of half-open minute spans: strictly sorted, non-overlapping
/// and non-adjacent.
///
/// Build one from spans in any order with [`IntervalSet::from_spans`]; the
/// constructor sorts and coalesces, and every operation below returns a set
/// that is canonical again. Two sets are `==` exactly when they cover the same
/// minutes.
///
/// ```
/// use opportune::intervals::{IntervalSet, Span};
///
/// let busy = IntervalSet::from_spans(vec![
///     Span::new(600, 660).unwrap(),
///     Span::new(480, 540).unwrap(),
/// ]);
/// // Sorted and coalesced, whatever order the input arrived in:
/// assert_eq!(busy.spans(), [Span::new(480, 540).unwrap(), Span::new(600, 660).unwrap()]);
///
/// let free = IntervalSet::from_spans(vec![Span::new(0, 1_440).unwrap()]).difference(&busy);
/// assert_eq!(
///     free.spans(),
///     [Span::new(0, 480).unwrap(), Span::new(540, 600).unwrap(), Span::new(660, 1_440).unwrap()]
/// );
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default, Hash)]
pub struct IntervalSet {
    spans: Vec<Span>,
}

impl IntervalSet {
    /// The empty set.
    #[must_use]
    pub fn new() -> Self {
        Self { spans: Vec::new() }
    }

    /// Builds a set from spans in any order, merging spans that overlap or
    /// touch. An empty input gives the empty set.
    ///
    /// ```
    /// # use opportune::intervals::{IntervalSet, Span};
    /// let set = IntervalSet::from_spans(vec![
    ///     Span::new(10, 20).unwrap(),
    ///     Span::new(20, 30).unwrap(), // touches the first
    ///     Span::new(0, 5).unwrap(),
    /// ]);
    /// assert_eq!(
    ///     set.spans(),
    ///     [Span::new(0, 5).unwrap(), Span::new(10, 30).unwrap()]
    /// );
    /// ```
    #[must_use]
    pub fn from_spans(mut spans: Vec<Span>) -> Self {
        if spans.len() <= 1 {
            return Self { spans };
        }
        spans.sort_unstable();
        let mut write = 0;
        for read in 1..spans.len() {
            let span = spans[read];
            if span.start <= spans[write].end {
                spans[write].end = spans[write].end.max(span.end);
            } else {
                write += 1;
                spans[write] = span;
            }
        }
        spans.truncate(write + 1);
        Self { spans }
    }

    /// Whether the set covers no minutes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    /// The set's spans, in sorted, coalesced order.
    #[must_use]
    pub fn spans(&self) -> &[Span] {
        &self.spans
    }

    /// Whether the set covers `minute`.
    #[must_use]
    pub fn contains_minute(&self, minute: i64) -> bool {
        let idx = self.spans.partition_point(|span| span.end <= minute);
        self.spans.get(idx).is_some_and(|span| span.start <= minute)
    }

    /// Whether the set covers every minute of `span`.
    ///
    /// Because the set is coalesced, a covered span is always covered by one
    /// of its spans.
    #[must_use]
    pub fn contains_span(&self, span: &Span) -> bool {
        let idx = self.spans.partition_point(|s| s.end <= span.start);
        self.spans
            .get(idx)
            .is_some_and(|s| s.start <= span.start && span.end <= s.end)
    }

    /// Whether the set and `span` share at least one minute. Touching is not
    /// overlapping: a span ending where another begins shares no minute.
    #[must_use]
    pub fn overlaps(&self, span: &Span) -> bool {
        let idx = self.spans.partition_point(|s| s.end <= span.start);
        self.spans.get(idx).is_some_and(|s| s.start < span.end)
    }

    /// The minutes covered by either set.
    ///
    /// ```
    /// # use opportune::intervals::{IntervalSet, Span};
    /// let a = IntervalSet::from_spans(vec![Span::new(0, 10).unwrap()]);
    /// let b = IntervalSet::from_spans(vec![Span::new(8, 12).unwrap()]);
    /// assert_eq!(a.union(&b).spans(), [Span::new(0, 12).unwrap()]);
    /// ```
    #[must_use]
    pub fn union(&self, other: &Self) -> Self {
        Self::from_spans(
            self.spans
                .iter()
                .copied()
                .chain(other.spans.iter().copied())
                .collect(),
        )
    }

    /// The minutes covered by both sets.
    #[must_use]
    pub fn intersection(&self, other: &Self) -> Self {
        let mut spans = Vec::new();
        let (mut i, mut j) = (0, 0);
        while i < self.spans.len() && j < other.spans.len() {
            let a = &self.spans[i];
            let b = &other.spans[j];
            let start = a.start.max(b.start);
            let end = a.end.min(b.end);
            if start < end {
                spans.push(Span { start, end });
            }
            if a.end <= b.end {
                i += 1;
            } else {
                j += 1;
            }
        }
        debug_assert!(
            is_canonical(&spans),
            "intersection output must be canonical"
        );
        Self { spans }
    }

    /// The minutes covered by this set and not by `other`.
    #[must_use]
    pub fn difference(&self, other: &Self) -> Self {
        let mut spans = Vec::new();
        let mut j = 0;
        for span in &self.spans {
            let mut cursor = span.start;
            while j < other.spans.len() && other.spans[j].end <= cursor {
                j += 1;
            }
            let mut k = j;
            while k < other.spans.len() && other.spans[k].start < span.end {
                let blocker = &other.spans[k];
                if blocker.start > cursor {
                    spans.push(Span {
                        start: cursor,
                        end: blocker.start,
                    });
                }
                cursor = cursor.max(blocker.end);
                if cursor >= span.end {
                    break;
                }
                k += 1;
            }
            if cursor < span.end {
                spans.push(Span {
                    start: cursor,
                    end: span.end,
                });
            }
        }
        debug_assert!(is_canonical(&spans), "difference output must be canonical");
        Self { spans }
    }

    /// Widens every span by `before` minutes at its start and `after` minutes
    /// at its end, then coalesces. The empty set widens to the empty set.
    ///
    /// ```
    /// # use opportune::intervals::{IntervalSet, Span};
    /// let busy = IntervalSet::from_spans(vec![
    ///     Span::new(600, 660).unwrap(),
    ///     Span::new(663, 700).unwrap(),
    /// ]);
    /// // Two minutes of travel each way make the first two spans overlap:
    /// assert_eq!(
    ///     busy.widened(2, 2).unwrap().spans(),
    ///     [Span::new(598, 702).unwrap()]
    /// );
    /// ```
    ///
    /// # Errors
    /// Returns [`IntervalError::NegativeWiden`] for a negative amount and
    /// [`IntervalError::WidenOverflow`] when widening a span would leave the
    /// representable minute range.
    pub fn widened(&self, before: i64, after: i64) -> Result<Self, IntervalError> {
        if before < 0 || after < 0 {
            return Err(IntervalError::NegativeWiden { before, after });
        }
        let mut spans = Vec::with_capacity(self.spans.len());
        for span in &self.spans {
            let start = span
                .start
                .checked_sub(before)
                .ok_or(IntervalError::WidenOverflow { before, after })?;
            let end = span
                .end
                .checked_add(after)
                .ok_or(IntervalError::WidenOverflow { before, after })?;
            spans.push(Span { start, end });
        }
        Ok(Self::from_spans(spans))
    }
}

/// Whether `spans` is strictly sorted, non-empty, non-overlapping and
/// non-adjacent — the invariant [`IntervalSet`] maintains.
fn is_canonical(spans: &[Span]) -> bool {
    spans.iter().all(|span| span.start < span.end)
        && spans.windows(2).all(|pair| pair[0].end < pair[1].start)
}

#[cfg(test)]
mod tests {
    use super::{IntervalError, IntervalSet, Span};

    fn set(spans: &[(i64, i64)]) -> IntervalSet {
        IntervalSet::from_spans(
            spans
                .iter()
                .map(|&(start, end)| Span::new(start, end).unwrap())
                .collect(),
        )
    }

    #[test]
    fn rejects_empty_and_reversed_spans() {
        assert_eq!(
            Span::new(5, 5),
            Err(IntervalError::EmptySpan { start: 5, end: 5 })
        );
        assert_eq!(
            Span::new(9, 3),
            Err(IntervalError::EmptySpan { start: 9, end: 3 })
        );
        assert_eq!(
            IntervalError::EmptySpan { start: 5, end: 5 }.code(),
            "span_empty"
        );
    }

    #[test]
    fn single_minute_spans_are_valid() {
        let span = Span::new(7, 8).unwrap();
        assert!(span.contains(7));
        assert!(!span.contains(6));
        assert!(!span.contains(8));
    }

    #[test]
    fn touching_spans_coalesce_but_adjacent_minutes_do_not() {
        // [0,10) and [10,20) touch and coalesce; [0,10) and [11,20) leave a gap.
        assert_eq!(
            set(&[(0, 10), (10, 20)]).spans(),
            [Span::new(0, 20).unwrap()]
        );
        assert_eq!(
            set(&[(0, 10), (11, 20)]).spans(),
            [Span::new(0, 10).unwrap(), Span::new(11, 20).unwrap()]
        );
    }

    #[test]
    fn from_spans_sorts_overlaps_and_coalesces() {
        assert_eq!(
            set(&[(30, 40), (0, 12), (10, 15), (50, 60)]).spans(),
            [
                Span::new(0, 15).unwrap(),
                Span::new(30, 40).unwrap(),
                Span::new(50, 60).unwrap()
            ]
        );
        assert!(IntervalSet::from_spans(Vec::new()).is_empty());
        assert_eq!(IntervalSet::from_spans(Vec::new()).spans(), []);
    }

    #[test]
    fn containment_queries_cover_the_boundaries() {
        let busy = set(&[(10, 20), (30, 40)]);
        assert!(!busy.contains_minute(9));
        assert!(busy.contains_minute(10));
        assert!(busy.contains_minute(19));
        assert!(!busy.contains_minute(20));

        assert!(busy.contains_span(&Span::new(10, 20).unwrap()));
        assert!(busy.contains_span(&Span::new(15, 19).unwrap()));
        // A covered span must be covered by one span, not by two together:
        assert!(!busy.contains_span(&Span::new(19, 31).unwrap()));
        assert!(!busy.contains_span(&Span::new(10, 21).unwrap()));
        assert!(!busy.contains_span(&Span::new(29, 31).unwrap()));
    }

    #[test]
    fn overlap_distinguishes_sharing_from_touching() {
        let busy = set(&[(10, 20)]);
        assert!(busy.overlaps(&Span::new(9, 11).unwrap()));
        assert!(busy.overlaps(&Span::new(19, 21).unwrap()));
        assert!(busy.overlaps(&Span::new(10, 20).unwrap()));
        assert!(!busy.overlaps(&Span::new(20, 30).unwrap()));
        assert!(!busy.overlaps(&Span::new(0, 10).unwrap()));
        assert!(!busy.overlaps(&Span::new(21, 30).unwrap()));
    }

    #[test]
    fn union_intersection_and_difference_agree_on_hand_cases() {
        let a = set(&[(0, 10), (20, 30)]);
        let b = set(&[(5, 25)]);

        assert_eq!(a.union(&b).spans(), [Span::new(0, 30).unwrap()]);
        assert_eq!(
            a.intersection(&b).spans(),
            [Span::new(5, 10).unwrap(), Span::new(20, 25).unwrap()]
        );
        assert_eq!(
            a.difference(&b).spans(),
            [Span::new(0, 5).unwrap(), Span::new(25, 30).unwrap()]
        );
        assert_eq!(b.difference(&a).spans(), [Span::new(10, 20).unwrap()]);
    }

    #[test]
    fn set_algebra_with_the_empty_set() {
        let empty = IntervalSet::new();
        let full = set(&[(0, 10)]);
        assert_eq!(empty.union(&full), full);
        assert_eq!(full.union(&empty), full);
        assert!(empty.intersection(&full).is_empty());
        assert_eq!(full.difference(&empty), full);
        assert!(full.difference(&full).is_empty());
    }

    #[test]
    fn intersection_of_a_touching_input_list_keeps_one_span() {
        // The input list [0,2) and [2,5) is coalesced to [0,5) before the
        // sweep, so intersecting it with [0,10) is that whole span.
        let a = set(&[(0, 10)]);
        let b = set(&[(0, 2), (2, 5)]);
        assert_eq!(a.intersection(&b).spans(), [Span::new(0, 5).unwrap()]);
    }

    #[test]
    fn difference_removes_wholly_covered_spans() {
        let a = set(&[(0, 10), (20, 30)]);
        let b = set(&[(0, 10), (20, 21), (22, 30)]);
        assert_eq!(a.difference(&b).spans(), [Span::new(21, 22).unwrap()]);
    }

    #[test]
    fn widening_is_separate_before_and_after() {
        let busy = set(&[(100, 110)]);
        assert_eq!(
            busy.widened(0, 0).unwrap().spans(),
            [Span::new(100, 110).unwrap()]
        );
        assert_eq!(
            busy.widened(5, 1).unwrap().spans(),
            [Span::new(95, 111).unwrap()]
        );
        assert!(IntervalSet::new().widened(5, 5).unwrap().is_empty());
    }

    #[test]
    fn widening_can_coalesce_separated_spans() {
        let busy = set(&[(100, 110), (113, 120)]);
        assert_eq!(
            busy.widened(2, 2).unwrap().spans(),
            [Span::new(98, 122).unwrap()]
        );
        // Widened by 1 before and 2 after, the first span ends at 112 and the
        // second starts at 112: they touch, so they coalesce.
        assert_eq!(
            busy.widened(1, 2).unwrap().spans(),
            [Span::new(99, 122).unwrap()]
        );
    }

    #[test]
    fn widening_rejects_negative_amounts_and_overflow() {
        let busy = set(&[(100, 110)]);
        assert_eq!(
            busy.widened(-1, 0),
            Err(IntervalError::NegativeWiden {
                before: -1,
                after: 0
            })
        );
        assert_eq!(
            IntervalError::NegativeWiden {
                before: -1,
                after: 0
            }
            .code(),
            "span_widen_negative"
        );

        let extreme = set(&[(i64::MIN, i64::MIN + 1)]);
        assert_eq!(
            extreme.widened(1, 0),
            Err(IntervalError::WidenOverflow {
                before: 1,
                after: 0
            })
        );
        assert_eq!(
            IntervalError::WidenOverflow {
                before: 1,
                after: 0
            }
            .code(),
            "span_widen_overflow"
        );
        let high = set(&[(i64::MAX - 1, i64::MAX)]);
        assert_eq!(
            high.widened(0, 1),
            Err(IntervalError::WidenOverflow {
                before: 0,
                after: 1
            })
        );
    }

    #[test]
    fn spans_span_the_whole_minute_range_without_overflow() {
        let extreme = set(&[(i64::MIN, i64::MAX)]);
        assert!(extreme.contains_minute(i64::MIN));
        assert!(extreme.contains_minute(-1));
        assert!(extreme.contains_minute(0));
        assert!(extreme.contains_minute(i64::MAX - 1));
        assert!(!extreme.contains_minute(i64::MAX));
        assert!(extreme.contains_span(&Span::new(i64::MIN, i64::MAX).unwrap()));
        assert!(extreme.overlaps(&Span::new(i64::MIN, i64::MIN + 1).unwrap()));
        // The set algebra stays exact at the bounds: subtracting the low half
        // leaves exactly the high half.
        let low = set(&[(i64::MIN, 0)]);
        assert_eq!(
            extreme.difference(&low).spans(),
            [Span::new(0, i64::MAX).unwrap()]
        );
    }
}
