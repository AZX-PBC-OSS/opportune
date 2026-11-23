//! Validated wall-clock `blackout` specifications and the instants they block.
//!
//! A blackout is a list of local date-time spans — "when bookings may not
//! happen" — read in one zone. An instant is blacked out when its local
//! date-time in that zone falls inside a span, so a blackout across a daylight
//! saving transition needs no special case: the local reading is compared
//! directly, and a repeated local date-time is judged twice, once per instant.
//!
//! ```
//! use opportune::intervals::Span;
//! use opportune::time::{
//!     BlackoutSpec, BlackoutSpan, Date, Instant, LocalDateTime, LocalResolution, RuleZone,
//!     TimeOfDay, Zone, ZoneName, ZoneSchedule,
//! };
//!
//! let span = BlackoutSpan::new(
//!     LocalDateTime::new(Date::new(2027, 3, 10).unwrap(), TimeOfDay::MIDNIGHT),
//!     LocalDateTime::new(Date::new(2027, 3, 10).unwrap(), TimeOfDay::new(12, 0).unwrap()),
//! )
//! .unwrap();
//! let spec = BlackoutSpec::new(RuleZone::Named(ZoneName::new("UTC").unwrap()), vec![span]);
//!
//! let at = |hour: u8| match Zone::utc()
//!     .resolve_local(Date::new(2027, 3, 10).unwrap(), TimeOfDay::new(hour, 0).unwrap())
//!     .unwrap()
//! {
//!     LocalResolution::Unique(instant) => instant,
//!     _ => panic!("UTC has no transitions"),
//! };
//! let zones = ZoneSchedule::fixed(Zone::utc());
//! assert!(spec.contains(at(9), &zones));
//! assert!(!spec.contains(at(12), &zones)); // the span is half-open
//!
//! let day = spec
//!     .intervals(
//!         &zones,
//!         at(0),
//!         Instant::from_minutes(at(0).minutes() + 1_440).unwrap(),
//!     )
//!     .unwrap();
//! assert_eq!(day.spans(), [Span::new(at(0).minutes(), at(12).minutes()).unwrap()]);
//! ```

use crate::intervals::IntervalSet;
use crate::time::breakpoints::{self, BreakpointSource};
use crate::time::error::{BlackoutError, TimeError};
use crate::time::window::RuleZone;
use crate::time::{Instant, LocalDateTime, Zone, ZoneSchedule};

/// A half-open span of local date-times, `[start, end)`, in the blackout's zone.
///
/// `end` must be after `start`: a span that ends where it starts covers no
/// instant, and an inverted one is a typo rather than a range.
///
/// ```
/// use opportune::time::{BlackoutSpan, Date, LocalDateTime, TimeOfDay};
///
/// let christmas = BlackoutSpan::new(
///     LocalDateTime::new(Date::new(2026, 12, 24).unwrap(), TimeOfDay::MIDNIGHT),
///     LocalDateTime::new(Date::new(2027, 1, 2).unwrap(), TimeOfDay::MIDNIGHT),
/// )
/// .unwrap();
/// assert_eq!(christmas.start().to_string(), "2026-12-24T00:00");
///
/// let inverted = BlackoutSpan::new(
///     LocalDateTime::new(Date::new(2027, 1, 2).unwrap(), TimeOfDay::MIDNIGHT),
///     LocalDateTime::new(Date::new(2026, 12, 24).unwrap(), TimeOfDay::MIDNIGHT),
/// );
/// assert_eq!(inverted.unwrap_err().code(), "blackout_span_empty");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlackoutSpan {
    start: LocalDateTime,
    end: LocalDateTime,
}

impl BlackoutSpan {
    /// Creates the span `[start, end)`.
    ///
    /// # Errors
    /// Returns [`BlackoutError::SpanEmpty`] when `end` is not after `start`.
    pub fn new(start: LocalDateTime, end: LocalDateTime) -> Result<Self, BlackoutError> {
        if end <= start {
            return Err(BlackoutError::SpanEmpty { start, end });
        }
        Ok(Self { start, end })
    }

    /// The span's first local date-time.
    #[must_use]
    pub const fn start(&self) -> LocalDateTime {
        self.start
    }

    /// One past the span's last local date-time.
    #[must_use]
    pub const fn end(&self) -> LocalDateTime {
        self.end
    }

    /// Whether the span covers `at`, half-open.
    fn contains(&self, at: LocalDateTime) -> bool {
        self.start <= at && at < self.end
    }
}

/// A validated wall-clock `blackout` specification.
///
/// Spans may arrive in any order and may overlap or touch; they are sorted and
/// coalesced, because the union of the spans is all that decides membership.
///
/// ```
/// use opportune::time::{BlackoutSpec, Instant, RuleZone, Zone, ZoneSchedule};
///
/// let closed = BlackoutSpec::new(RuleZone::Event, Vec::new());
/// let zones = ZoneSchedule::fixed(Zone::utc());
/// assert!(!closed.contains(Instant::MIN, &zones));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlackoutSpec {
    zone: RuleZone,
    /// Sorted by start and coalesced: non-overlapping, non-adjacent.
    spans: Vec<BlackoutSpan>,
}

impl BlackoutSpec {
    /// Creates a specification from its zone selection and spans.
    #[must_use]
    pub fn new(zone: RuleZone, spans: Vec<BlackoutSpan>) -> Self {
        let mut spans = spans;
        spans.sort_unstable();
        let mut merged: Vec<BlackoutSpan> = Vec::with_capacity(spans.len());
        for span in spans {
            match merged.last_mut() {
                Some(last) if span.start <= last.end => {
                    last.end = last.end.max(span.end);
                }
                _ => merged.push(span),
            }
        }
        Self {
            zone,
            spans: merged,
        }
    }

    /// Whether `instant` is blacked out, read in `zones` — the zone function
    /// this specification's [`RuleZone`] resolved to.
    #[must_use]
    pub fn contains(&self, instant: Instant, zones: &ZoneSchedule) -> bool {
        self.member(instant, zones.zone_at(instant))
    }

    /// The minutes of `from..until` that are blacked out, as half-open UTC
    /// spans.
    ///
    /// The result is built over exactly the range asked for, never clipped to
    /// midnight or to any padded bound.
    ///
    /// # Errors
    /// Returns a [`TimeError`] only for a zone whose offsets are not whole
    /// minutes, which the database never produces inside the supported range.
    pub fn intervals(
        &self,
        zones: &ZoneSchedule,
        from: Instant,
        until: Instant,
    ) -> Result<IntervalSet, TimeError> {
        breakpoints::build(self, zones, from, until)
    }

    /// The zone selection the specification was built with.
    #[must_use]
    pub fn zone(&self) -> &RuleZone {
        &self.zone
    }

    /// The spans, sorted by start and coalesced.
    #[must_use]
    pub fn spans(&self) -> &[BlackoutSpan] {
        &self.spans
    }

    /// The local date-time an instant falls on in `zone`.
    fn local(instant: Instant, zone: &Zone) -> LocalDateTime {
        let zoned = instant.zoned(zone);
        LocalDateTime::new(zoned.date(), zoned.time())
    }
}

impl BreakpointSource for BlackoutSpec {
    fn member(&self, instant: Instant, zone: &Zone) -> bool {
        let local = Self::local(instant, zone);
        // Spans are coalesced, so the last span starting at or before `local` is
        // the only one that can cover it.
        let index = self.spans.partition_point(|span| span.start <= local);
        index > 0 && self.spans[index - 1].contains(local)
    }

    fn breakpoints(
        &self,
        zone: &Zone,
        start: Instant,
        end: Instant,
    ) -> Result<Vec<Instant>, TimeError> {
        let transitions = zone.transition_instants(start, end)?;
        let (first, last) = breakpoints::dates_covering(zone, start, end, &transitions);
        let mut instants = Vec::new();
        // The binary search below needs more than the sort: `new` coalesces spans
        // until no two are adjacent, so `start > previous.end` for every pair and
        // both `start` and `end` — and therefore `end.date()` — strictly increase.
        // A merge that let touching spans through, or one that kept them in arrival
        // order, would make the predicate non-monotone and the search silently skip
        // a boundary.
        debug_assert!(self
            .spans
            .windows(2)
            .all(|pair| pair[0].end < pair[1].start
                && pair[0].end.date() <= pair[1].end.date()));
        // Spans are coalesced, so `span.end.date()` is non-decreasing: skip every
        // span that ends strictly before the slice's earliest date.
        let first_index = self.spans.partition_point(|span| span.end.date() < first);
        for span in &self.spans[first_index..] {
            // Spans are sorted by start, so once one starts past the last local
            // date the slice can reach, no later span can be crossed in it.
            if span.start.date() > last {
                break;
            }
            // A boundary on a date the slice never reads is not a breakpoint of
            // it, and resolving it can fail outright: `BlackoutSpan` accepts any
            // civil date, so a span running to 9999-12-31 has an end that no
            // instant near the supported range can represent.
            if (first..=last).contains(&span.start.date()) {
                breakpoints::push_local(&mut instants, zone, span.start.date(), span.start.time())?;
            }
            if (first..=last).contains(&span.end.date()) {
                breakpoints::push_local(&mut instants, zone, span.end.date(), span.end.time())?;
            }
        }
        instants.extend(transitions);
        Ok(instants)
    }
}

#[cfg(test)]
mod tests {
    use super::{BlackoutSpan, BlackoutSpec};
    use crate::time::window::RuleZone;
    use crate::time::{
        BlackoutError, Date, Instant, LocalDateTime, LocalResolution, TimeOfDay, Zone, ZoneName,
        ZoneSchedule,
    };

    /// The instant of a UTC civil date-time, which always exists.
    fn utc(year: i16, month: i8, day: i8, hour: u8, minute: u8) -> Instant {
        let date = Date::new(year, month, day).unwrap();
        let time = TimeOfDay::new(hour, minute).unwrap();
        let LocalResolution::Unique(instant) = Zone::utc().resolve_local(date, time).unwrap()
        else {
            panic!("UTC has no transitions")
        };
        instant
    }

    fn at(day: i8, hour: u8, minute: u8) -> LocalDateTime {
        LocalDateTime::new(
            Date::new(2027, 3, day).unwrap(),
            TimeOfDay::new(hour, minute).unwrap(),
        )
    }

    fn span(start: (i8, u8, u8), end: (i8, u8, u8)) -> BlackoutSpan {
        BlackoutSpan::new(at(start.0, start.1, start.2), at(end.0, end.1, end.2)).unwrap()
    }

    fn spec(spans: Vec<BlackoutSpan>) -> BlackoutSpec {
        BlackoutSpec::new(RuleZone::Named(ZoneName::new("UTC").unwrap()), spans)
    }

    fn built(spec: &BlackoutSpec, from: Instant, until: Instant) -> Vec<(i64, i64)> {
        spec.intervals(&ZoneSchedule::fixed(Zone::utc()), from, until)
            .unwrap()
            .spans()
            .iter()
            .map(|span| (span.start(), span.end()))
            .collect()
    }

    #[test]
    fn an_inverted_or_zero_length_span_is_rejected() {
        for (start, end) in [
            ((10, 12, 0), (10, 12, 0)),
            ((10, 12, 0), (10, 9, 0)),
            ((11, 0, 0), (10, 0, 0)),
        ] {
            let error = BlackoutSpan::new(at(start.0, start.1, start.2), at(end.0, end.1, end.2))
                .unwrap_err();
            assert_eq!(
                error,
                BlackoutError::SpanEmpty {
                    start: at(start.0, start.1, start.2),
                    end: at(end.0, end.1, end.2),
                }
            );
            assert_eq!(error.code(), "blackout_span_empty");
        }
        assert!(BlackoutSpan::new(at(10, 0, 0), at(10, 0, 1)).is_ok());
    }

    #[test]
    fn a_span_is_half_open() {
        let spec = spec(vec![span((10, 9, 0), (10, 17, 0))]);
        let zones = ZoneSchedule::fixed(Zone::utc());
        assert!(!spec.contains(utc(2027, 3, 10, 8, 59), &zones));
        assert!(spec.contains(utc(2027, 3, 10, 9, 0), &zones));
        assert!(spec.contains(utc(2027, 3, 10, 16, 59), &zones));
        assert!(!spec.contains(utc(2027, 3, 10, 17, 0), &zones));
    }

    #[test]
    fn overlapping_and_touching_spans_coalesce() {
        // Two overlapping spans and one that starts where the others end.
        let spec = spec(vec![
            span((10, 12, 0), (10, 14, 0)),
            span((10, 9, 0), (10, 11, 0)),
            span((10, 11, 0), (10, 13, 0)),
        ]);
        assert_eq!(
            built(&spec, utc(2027, 3, 10, 0, 0), utc(2027, 3, 11, 0, 0)),
            [(
                utc(2027, 3, 10, 9, 0).minutes(),
                utc(2027, 3, 10, 14, 0).minutes()
            )]
        );
        // Construction coalesces, so the accessor reports one span and not the
        // three that were supplied.
        assert_eq!(spec.spans().len(), 1);
        assert_eq!(
            (spec.spans()[0].start(), spec.spans()[0].end()),
            (at(10, 9, 0), at(10, 14, 0))
        );
    }

    #[test]
    fn a_blackout_spanning_midnight_is_one_span() {
        let spec = spec(vec![span((10, 22, 0), (11, 3, 0))]);
        assert_eq!(
            built(&spec, utc(2027, 3, 10, 0, 0), utc(2027, 3, 12, 0, 0)),
            [(
                utc(2027, 3, 10, 22, 0).minutes(),
                utc(2027, 3, 11, 3, 0).minutes()
            )]
        );
    }

    #[test]
    fn a_blackout_builds_over_the_requested_range_without_clipping_to_midnight() {
        let spec = spec(vec![span((10, 9, 0), (10, 17, 0))]);
        assert_eq!(
            built(&spec, utc(2027, 3, 10, 10, 30), utc(2027, 3, 10, 12, 30)),
            [(
                utc(2027, 3, 10, 10, 30).minutes(),
                utc(2027, 3, 10, 12, 30).minutes()
            )]
        );
    }

    #[test]
    fn the_interval_set_agrees_with_instant_membership_minute_by_minute() {
        let spec = spec(vec![
            span((9, 18, 0), (10, 6, 30)),
            span((10, 12, 0), (10, 14, 0)),
            span((11, 0, 0), (12, 0, 0)),
        ]);
        let zones = ZoneSchedule::fixed(Zone::utc());
        let from = utc(2027, 3, 9, 0, 0);
        let until = utc(2027, 3, 12, 0, 0);
        let intervals = spec.intervals(&zones, from, until).unwrap();
        for minute in from.minutes()..until.minutes() {
            let instant = Instant::from_minutes(minute).unwrap();
            assert_eq!(
                intervals.contains_minute(minute),
                spec.contains(instant, &zones),
                "{instant}: the breakpoint builder and instant membership disagree"
            );
        }
    }
}
