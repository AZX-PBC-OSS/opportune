//! The segment-start breakpoint builder shared by window and blackout interval
//! sets.
//!
//! A wall-clock specification answers "is this instant inside?" from the
//! instant's local date and time. Within one constant-zone slice that local
//! reading advances one minute per minute, so the answer can only change where
//! the reading crosses something the specification names — a range boundary, a
//! local midnight — or where the zone itself jumps: a `zone_periods` boundary,
//! which ends a slice, or a daylight saving transition.
//!
//! With half-open intervals the change takes effect at the breakpoint itself,
//! so the membership of a segment's first instant speaks for the whole segment.
//! That is what keeps this builder in integer minutes and exact across
//! fall-back splits and skipped hours, with no daylight saving special case.

use crate::intervals::{IntervalSet, Span};
use crate::time::{Date, Instant, TimeError, TimeOfDay, Zone, ZoneSchedule};

/// A wall-clock specification the builder can turn into an interval set.
pub(crate) trait BreakpointSource {
    /// Whether `instant` is inside, with `zone` in force.
    fn member(&self, instant: Instant, zone: &Zone) -> bool;

    /// The instants in `[start, end)` at which membership could change, given
    /// that `zone` is in force for the whole span.
    ///
    /// Extra instants are harmless; a missing one is a wrong answer.
    fn breakpoints(
        &self,
        zone: &Zone,
        start: Instant,
        end: Instant,
    ) -> Result<Vec<Instant>, TimeError>;
}

/// Builds the set of minutes inside `from..until` that the specification
/// accepts.
pub(crate) fn build(
    source: &impl BreakpointSource,
    schedule: &ZoneSchedule,
    from: Instant,
    until: Instant,
) -> Result<IntervalSet, TimeError> {
    if from >= until {
        return Ok(IntervalSet::new());
    }
    let mut spans = Vec::new();
    for (slice_start, slice_end) in schedule.slices(from, until) {
        let zone = schedule.zone_at(slice_start);
        let mut points = vec![slice_start];
        for point in source.breakpoints(zone, slice_start, slice_end)? {
            if slice_start < point && point < slice_end {
                points.push(point);
            }
        }
        points.sort_unstable();
        points.dedup();
        for (index, &segment_start) in points.iter().enumerate() {
            let segment_end = points.get(index + 1).copied().unwrap_or(slice_end);
            if source.member(segment_start, zone) {
                spans.push(
                    Span::new(segment_start.minutes(), segment_end.minutes()).expect(
                        "a slice spans at least one minute and its breakpoints lie inside it",
                    ),
                );
            }
        }
    }
    Ok(IntervalSet::from_spans(spans))
}

/// Builds the set of minutes inside `range` that the specification accepts.
///
/// Interval construction works over representable instants, so a range whose end
/// lies past `Instant::MAX` would lose its final minute — the minute a booking at
/// the top of the supported range still occupies. That minute is classified by its
/// own first instant, `Instant::MAX`, exactly as every other segment is classified
/// by its own first instant, and nothing beyond it exists to classify.
pub(crate) fn build_minutes(
    source: &impl BreakpointSource,
    schedule: &ZoneSchedule,
    range: Span,
) -> Result<IntervalSet, TimeError> {
    let from = Instant::from_minutes(range.start().max(Instant::MIN.minutes()))?;
    let until = Instant::from_minutes(range.end().min(Instant::MAX.minutes()))?;
    let mut minutes = build(source, schedule, from, until)?;
    if range.end() > Instant::MAX.minutes()
        && source.member(Instant::MAX, schedule.zone_at(Instant::MAX))
    {
        let last = Span::new(Instant::MAX.minutes(), Instant::MAX.minutes() + 1)
            .expect("the last supported minute is a non-empty span");
        minutes = minutes.union(&IntervalSet::from_spans(vec![last]));
    }
    Ok(minutes)
}

/// Adds every instant the local date-time occurs at in `zone` to `instants`.
///
/// A skipped local time contributes nothing — the change happens at the offset
/// transition instead, which is a breakpoint of its own — and a repeated one
/// contributes both instants, because membership is judged separately at each.
pub(crate) fn push_local(
    instants: &mut Vec<Instant>,
    zone: &Zone,
    date: Date,
    time: TimeOfDay,
) -> Result<(), TimeError> {
    instants.extend(zone.local_instants(date, time)?);
    Ok(())
}

/// The local dates the instants of `[start, end)` fall on, exactly.
///
/// A boundary is collected for each of these dates and for the date before each,
/// so this is also the set that must be exact: too narrow and a date's
/// boundaries are never collected, too wide only costs work.
///
/// Exact rather than estimated: with the zone constant between transitions the
/// local reading rises one minute per minute, so its extremes over the slice sit
/// at the slice's edges or on either side of a transition, however far an offset
/// jumps. A forward jump can skip a date label and a backward one can repeat it,
/// so both readings at each transition are observed — `transition` and the
/// minute before it. `transitions` is the slice's own transition list, which the
/// caller already needs as breakpoints.
pub(crate) fn dates_covering(
    zone: &Zone,
    start: Instant,
    end: Instant,
    transitions: &[Instant],
) -> (Date, Date) {
    let mut earliest = zone.zoned(start).date();
    let mut latest = earliest;
    let mut observe = |instant: Instant| {
        let date = zone.zoned(instant).date();
        earliest = earliest.min(date);
        latest = latest.max(date);
    };
    for &transition in transitions {
        observe(transition);
        observe(
            Instant::from_minutes(transition.minutes() - 1)
                .expect("a transition inside the slice has a representable minute before it"),
        );
    }
    // `end` is exclusive and `start < end`, so the slice's last minute exists.
    observe(
        Instant::from_minutes(end.minutes() - 1)
            .expect("a slice ends at least one minute after it starts"),
    );
    (earliest, latest)
}

#[cfg(all(test, feature = "bundled-tzdb"))]
mod tests {
    use super::dates_covering;
    use crate::time::{Date, Instant, LocalResolution, TimeOfDay, TimeZoneDatabase, Zone};

    /// The instant of a UTC date at a whole hour, which always exists.
    fn utc(year: i16, month: i8, day: i8, hour: u8) -> Instant {
        let date = Date::new(year, month, day).unwrap();
        let LocalResolution::Unique(instant) = Zone::utc()
            .resolve_local(date, TimeOfDay::new(hour, 0).unwrap())
            .unwrap()
        else {
            panic!("UTC has no transitions")
        };
        instant
    }

    /// The covered range must be exactly the local dates the slice's instants
    /// fall on: too narrow and a date's boundaries are never collected, too wide
    /// only costs work and would hide an error.
    #[test]
    fn the_covered_dates_are_exactly_the_local_dates_inside_the_slice() {
        let db = TimeZoneDatabase::bundled().unwrap();
        let slices = [
            // Brazil's daylight saving used to end at local midnight, so the
            // repeated hour straddles the date label.
            (
                "America/Sao_Paulo",
                utc(2018, 2, 17, 12),
                utc(2018, 2, 18, 12),
            ),
            // Santiago springs forward from midnight to 01:00.
            ("America/Santiago", utc(2027, 9, 4, 12), utc(2027, 9, 5, 12)),
            // New York falls back at 02:00.
            ("America/New_York", utc(2027, 11, 7, 0), utc(2027, 11, 8, 0)),
            // Lord Howe moves by half an hour.
            (
                "Australia/Lord_Howe",
                utc(2027, 4, 3, 0),
                utc(2027, 4, 4, 0),
            ),
            // Samoa jumped forward a whole day, skipping 30 December 2011.
            ("Pacific/Apia", utc(2011, 12, 30, 0), utc(2011, 12, 31, 0)),
            // Kiritimati crossed the date line and skipped a date in 1994.
            (
                "Pacific/Kiritimati",
                utc(1994, 12, 30, 0),
                utc(1995, 1, 1, 0),
            ),
            // Moscow moved back an hour in 2014.
            (
                "Europe/Moscow",
                utc(2014, 10, 25, 12),
                utc(2014, 10, 26, 12),
            ),
            // Troll shifts by two hours.
            (
                "Antarctica/Troll",
                utc(2027, 3, 27, 12),
                utc(2027, 3, 28, 12),
            ),
        ];
        for (name, from, until) in slices {
            let zone = db.zone(name).unwrap();
            let transitions = zone.transition_instants(from, until).unwrap();
            assert!(
                !transitions.is_empty(),
                "{name}: the fixture must cross a transition"
            );
            let (first, last) = dates_covering(&zone, from, until, &transitions);

            let mut earliest = None;
            let mut latest = None;
            for minute in from.minutes()..until.minutes() {
                let date = zone.zoned(Instant::from_minutes(minute).unwrap()).date();
                earliest = Some(earliest.map_or(date, |seen: Date| seen.min(date)));
                latest = Some(latest.map_or(date, |seen: Date| seen.max(date)));
            }
            assert_eq!(
                first,
                earliest.unwrap(),
                "{name}: the earliest local date is the range's first date"
            );
            assert_eq!(
                last,
                latest.unwrap(),
                "{name}: the latest local date is the range's last date"
            );
        }
    }

    /// The bottom of the supported range is not transition-free: `Africa/Bissau`
    /// moved from -01:00 to +00:00 at 1975-01-01T01:00Z, one hour above
    /// `Instant::MIN`, so local 00:00-01:00 that day is skipped there.
    ///
    /// This is the only case in the bundled database where a slice starting
    /// exactly at the range floor contains an offset transition, and the only
    /// one where a crossing window's previous-day spill has to resolve local
    /// times on a civil date below the floor (1974-12-31 in Bissau, whose
    /// instants do not exist). Both must stay exact and must not error.
    #[test]
    fn a_slice_at_the_range_floor_crossing_a_transition_stays_exact_and_agrees() {
        use crate::time::{
            RuleZone, WallEnd, WallRange, Weekday, Weekdays, WeeklyKey, WindowSpec, ZoneSchedule,
        };

        let db = TimeZoneDatabase::bundled().unwrap();
        let zone = db.zone("Africa/Bissau").unwrap();
        let from = Instant::MIN;
        let until = Instant::from_minutes(from.minutes() + 3 * 1440).unwrap();

        let transitions = zone.transition_instants(from, until).unwrap();
        assert_eq!(
            transitions.len(),
            1,
            "Bissau changes offset once, an hour above the floor"
        );

        // The covered dates are still exactly the local dates inside the slice,
        // even though the transition moves the local reading forward mid-slice.
        let (first, last) = dates_covering(&zone, from, until, &transitions);
        let mut earliest = None;
        let mut latest = None;
        for minute in from.minutes()..until.minutes() {
            let date = zone.zoned(Instant::from_minutes(minute).unwrap()).date();
            earliest = Some(earliest.map_or(date, |seen: Date| seen.min(date)));
            latest = Some(latest.map_or(date, |seen: Date| seen.max(date)));
        }
        assert_eq!(first, earliest.unwrap(), "earliest local date");
        assert_eq!(last, latest.unwrap(), "latest local date");

        // A crossing window on the Tuesday the slice starts on, so the range
        // itself is live (the slice's first local date, 1974-12-31, is a
        // Tuesday) and its spill reaches Wednesday 1975-01-01 00:00-02:00 --
        // straight through the hour Bissau skipped. The Tuesday 22:00 start is
        // below the floor, so its instant does not exist and must be dropped
        // rather than erroring.
        let spec = WindowSpec::new(
            RuleZone::Event,
            vec![
                WeeklyKey::new(
                    Weekdays::list([Weekday::Tuesday]),
                    vec![
                        WallRange::new(
                            TimeOfDay::new(22, 0).unwrap(),
                            WallEnd::Time(TimeOfDay::new(2, 0).unwrap()),
                        )
                        .unwrap(),
                    ],
                )
                .unwrap(),
            ],
            Vec::new(),
        )
        .unwrap();
        let schedule = ZoneSchedule::fixed(zone.clone());
        let built = spec.intervals(&schedule, from, until).unwrap();
        assert!(
            !built.spans().is_empty(),
            "the window is live over this slice, so the agreement check below \
             is not vacuous"
        );
        for minute in from.minutes()..until.minutes() {
            let instant = Instant::from_minutes(minute).unwrap();
            assert_eq!(
                built.contains_minute(minute),
                spec.contains(instant, &schedule),
                "built set and membership disagree at minute {minute}"
            );
        }
    }
}
