//! Zone periods: a participant's zone as a function of the instant.
//!
//! A participant has a home zone and, while travelling, a list of
//! [`ZonePeriod`]s. Each period is a half-open instant span in another zone, so
//! adjacent legs of a trip join exactly — no gap, no overlap — and the zone
//! switches at the instant given, not at midnight.
//!
//! ```
//! # #[cfg(feature = "bundled-tzdb")] {
//! use opportune::time::{Instant, TimeZoneDatabase, ZonePeriod, ZoneSchedule};
//!
//! let db = TimeZoneDatabase::bundled().unwrap();
//! let from = Instant::from_unix_seconds(1_804_604_400).unwrap(); // 2027-03-09T15:00Z
//! let until = Instant::from_unix_seconds(1_804_780_800).unwrap(); // 2027-03-11T15:00Z
//! let anchorage = db.zone("America/Anchorage").unwrap();
//! let schedule = ZoneSchedule::new(
//!     db.zone("Europe/London").unwrap(),
//!     vec![ZonePeriod::new(anchorage, from, until).unwrap()],
//! ).unwrap();
//!
//! assert_eq!(schedule.zone_at(from).name(), "America/Anchorage");
//! // The period is half-open, so the instant it ends is back home.
//! assert_eq!(schedule.zone_at(until).name(), "Europe/London");
//! # }
//! ```

use std::collections::BTreeSet;

use crate::time::error::TimeError;
use crate::time::{Date, Instant, Zone};

/// A span of instants during which a participant is in another zone.
///
/// The span is half-open, `[from, until)`, so the instant a period ends belongs
/// to whatever follows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZonePeriod {
    zone: Zone,
    from: Instant,
    until: Instant,
}

impl ZonePeriod {
    /// Creates a zone period covering `[from, until)`.
    ///
    /// # Errors
    /// Returns [`TimeError::ZonePeriodEmpty`] when `from` is not before
    /// `until`.
    pub fn new(zone: Zone, from: Instant, until: Instant) -> Result<Self, TimeError> {
        if from >= until {
            return Err(TimeError::ZonePeriodEmpty { from, until });
        }
        Ok(Self { zone, from, until })
    }

    /// The zone the participant is in during this period.
    #[must_use]
    pub fn zone(&self) -> &Zone {
        &self.zone
    }

    /// The first instant of the period.
    #[must_use]
    pub const fn from(&self) -> Instant {
        self.from
    }

    /// One past the last instant of the period.
    #[must_use]
    pub const fn until(&self) -> Instant {
        self.until
    }

    /// Whether the period covers `instant`.
    #[must_use]
    pub fn contains(&self, instant: Instant) -> bool {
        self.from <= instant && instant < self.until
    }
}

/// A participant's zone as a function of the instant: the home zone, with
/// disjoint periods in other zones.
///
/// ```
/// # use opportune::time::{Instant, Zone, ZonePeriod, ZoneSchedule};
/// let home = Zone::utc();
/// let schedule = ZoneSchedule::new(home.clone(), Vec::new()).unwrap();
/// assert_eq!(schedule.zone_at(Instant::MIN), &home);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZoneSchedule {
    home: Zone,
    /// Sorted by `from` and proven disjoint by [`ZoneSchedule::new`].
    periods: Vec<ZonePeriod>,
}

impl ZoneSchedule {
    /// A schedule that stays in one zone: no periods, so every instant falls in
    /// `zone`.
    ///
    /// ```
    /// # use opportune::time::{Instant, Zone, ZoneSchedule};
    /// let schedule = ZoneSchedule::fixed(Zone::utc());
    /// assert_eq!(schedule.zone_at(Instant::MIN), &Zone::utc());
    /// assert!(schedule.periods().is_empty());
    /// ```
    #[must_use]
    pub fn fixed(zone: Zone) -> Self {
        Self {
            home: zone,
            periods: Vec::new(),
        }
    }

    /// Creates a schedule from a home zone and any number of periods.
    ///
    /// Periods may arrive in any order; they are sorted. They must be disjoint,
    /// though they may touch: an instant is in at most one zone.
    ///
    /// ```
    /// # use opportune::time::{Instant, Zone, ZonePeriod, ZoneSchedule, TimeError};
    /// let from = Instant::from_unix_seconds(1_804_604_400).unwrap();
    /// let middle = Instant::from_unix_seconds(1_804_780_800).unwrap();
    /// let until = Instant::from_unix_seconds(1_805_124_000).unwrap();
    /// let error = ZoneSchedule::new(
    ///     Zone::utc(),
    ///     vec![
    ///         ZonePeriod::new(Zone::utc(), from, until).unwrap(),
    ///         ZonePeriod::new(Zone::utc(), middle, until).unwrap(),
    ///     ],
    /// ).unwrap_err();
    /// assert_eq!(error.code(), "zone_periods_overlap");
    /// ```
    ///
    /// # Errors
    /// Returns [`TimeError::ZonePeriodsOverlap`] as soon as two periods share an
    /// instant.
    pub fn new(home: Zone, mut periods: Vec<ZonePeriod>) -> Result<Self, TimeError> {
        periods.sort_by_key(ZonePeriod::from);
        for pair in periods.windows(2) {
            let (earlier, later) = (&pair[0], &pair[1]);
            if later.from < earlier.until {
                return Err(TimeError::ZonePeriodsOverlap { from: later.from });
            }
        }
        Ok(Self { home, periods })
    }

    /// The participant's home zone.
    #[must_use]
    pub fn home(&self) -> &Zone {
        &self.home
    }

    /// The periods, sorted by start instant.
    #[must_use]
    pub fn periods(&self) -> &[ZonePeriod] {
        &self.periods
    }

    /// The zone the participant is in at `instant`: the period covering it, or
    /// the home zone outside every period.
    ///
    /// This answers the same half-open membership rule as
    /// [`ZonePeriod::contains`], by binary search rather than by scanning: the
    /// first period that ends after `instant` is the only candidate, and the
    /// span test decides whether it covers the instant.
    #[must_use]
    pub fn zone_at(&self, instant: Instant) -> &Zone {
        let index = self
            .periods
            .partition_point(|period| period.until <= instant);
        match self.periods.get(index) {
            Some(period) if period.contains(instant) => &period.zone,
            _ => &self.home,
        }
    }

    /// Splits `from..until` into the spans over which the zone is constant, in
    /// order.
    ///
    /// A zone-period boundary ends a slice: the local reading jumps there, so a
    /// rule's membership can change with no wall-clock crossing. Interval
    /// construction reads these slices; a caller that only walks each leg once
    /// uses [`ZoneSchedule::legs`] to avoid materialising them.
    pub(crate) fn slices(&self, from: Instant, until: Instant) -> Vec<(Instant, Instant)> {
        if from >= until {
            return Vec::new();
        }
        // `periods` is sorted and disjoint, so the boundaries inside the range are
        // already in ascending order: start at the first period that ends after
        // `from` and stop at the first that starts at or after `until`. Periods that
        // touch contribute one boundary rather than two.
        let first = self.periods.partition_point(|period| period.until <= from);
        let mut boundaries: Vec<Instant> = Vec::with_capacity(2);
        boundaries.push(from);
        for period in &self.periods[first..] {
            if period.from >= until {
                break;
            }
            for boundary in [period.from, period.until] {
                if from < boundary && boundary < until && boundaries.last() != Some(&boundary) {
                    boundaries.push(boundary);
                }
            }
        }
        boundaries
            .iter()
            .enumerate()
            .map(|(index, &start)| {
                let end = boundaries.get(index + 1).copied().unwrap_or(until);
                (start, end)
            })
            .collect()
    }

    /// The legs of `from..until` in order: each span over which the zone is
    /// constant, with the zone in force there.
    ///
    /// This is [`ZoneSchedule::slices`] paired with [`ZoneSchedule::zone_at`] at
    /// each slice's start, walked from the first period that ends after `from`
    /// instead of materialising the boundaries, so a caller that only reads each
    /// leg once pays neither the slice vector nor a binary search per leg.
    pub(crate) fn legs(
        &self,
        from: Instant,
        until: Instant,
    ) -> impl Iterator<Item = (&Zone, Instant, Instant)> + '_ {
        let mut cursor = from;
        let mut index = self.periods.partition_point(|period| period.until <= from);
        std::iter::from_fn(move || {
            if cursor >= until {
                return None;
            }
            let start = cursor;
            match self.periods.get(index) {
                // Inside a period, or the instant it starts: half-open, so the
                // instant a period begins belongs to it and the one it ends at does
                // not.
                Some(period) if period.from <= cursor => {
                    cursor = period.until.min(until);
                    index += 1;
                    Some((&period.zone, start, cursor))
                }
                // In the gap before the next period, which is home.
                Some(period) => {
                    cursor = period.from.min(until);
                    Some((&self.home, start, cursor))
                }
                None => {
                    cursor = until;
                    Some((&self.home, start, until))
                }
            }
        })
    }

    /// Every instant in `from..until` at which the local date can change for a
    /// reason other than local midnight: each zone period's ends, and every
    /// offset change of every zone the schedule can be in.
    ///
    /// A whole-day walk is exact only while the zone and its offset hold: a
    /// period end moves the local reading by whatever the two zones differ, and
    /// a backward transition can move it back across midnight, so the date it
    /// repeats would never be collected. Newfoundland ended daylight saving at
    /// 00:01 local until 2010 and Casey moved back three hours in 2010, so the
    /// bundled database holds such transitions. A zone that is not in force
    /// contributes stops nothing needs, and an extra stop only costs one step.
    ///
    /// # Errors
    /// Returns a [`TimeError`] only for a zone whose offsets are not whole
    /// minutes, which the database never produces inside the supported range.
    pub(crate) fn date_jumps(&self, from: Instant, until: Instant) -> Result<Vec<i64>, TimeError> {
        let mut jumps: Vec<i64> = Vec::new();
        let mut zones: Vec<&Zone> = vec![&self.home];
        for period in &self.periods {
            if from < period.until && period.from < until {
                if from < period.from {
                    jumps.push(period.from.minutes());
                }
                if period.until < until {
                    jumps.push(period.until.minutes());
                }
                if !zones.iter().any(|seen| seen.name() == period.zone.name()) {
                    zones.push(&period.zone);
                }
            }
        }
        for zone in zones {
            for transition in zone.transition_instants(from, until)? {
                jumps.push(transition.minutes());
            }
        }
        jumps.sort_unstable();
        jumps.dedup();
        Ok(jumps)
    }

    /// Walks `from..until` in constant-date segments, stopping at local midnights
    /// and at `jumps`.
    ///
    /// Between two stops the zone and its offset hold, so the local reading rises
    /// one minute per minute: the date of a segment's first minute is the date of
    /// every minute before the next stop, which is what makes a whole-day jump
    /// exact.
    pub(crate) fn for_each_date_segment(
        &self,
        jumps: &[i64],
        from: i64,
        until: i64,
        mut visit: impl FnMut(i64, i64, Date),
    ) {
        let mut minute = from;
        while minute < until {
            let Ok(instant) = Instant::from_minutes(minute) else {
                break;
            };
            let zone = self.zone_at(instant);
            let date = zone.zoned(instant).date();
            let day_end = date
                .plus_days(1)
                .ok()
                .and_then(|next| zone.day_start(next).ok())
                .map_or(until, |end| end.minutes());
            // `jumps` is ascending, so the first entry past this minute is the
            // next place the local date can change without a midnight.
            let next = jumps.partition_point(|jump| *jump <= minute);
            let jump_end = jumps.get(next).copied().unwrap_or(i64::MAX);
            let stop = day_end.min(jump_end).max(minute + 1).min(until);
            visit(minute, stop, date);
            minute = stop;
        }
    }

    /// Adds every local date the minutes in `from..until` fall on.
    pub(crate) fn dates_in(
        &self,
        jumps: &[i64],
        from: i64,
        until: i64,
        dates: &mut BTreeSet<Date>,
    ) {
        self.for_each_date_segment(jumps, from, until, |_, _, date| {
            dates.insert(date);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{ZonePeriod, ZoneSchedule};
    use crate::time::{Instant, TimeError, Zone};
    fn instant(hour: i64) -> Instant {
        Instant::from_minutes(Instant::MIN.minutes() + hour * 60).unwrap()
    }

    #[test]
    fn empty_periods_fall_back_to_the_home_zone() {
        let home = Zone::utc();
        let schedule = ZoneSchedule::new(home.clone(), Vec::new()).unwrap();
        assert_eq!(schedule.zone_at(Instant::MIN), &home);
        assert_eq!(schedule.zone_at(Instant::MAX), &home);
        assert!(schedule.periods().is_empty());
        assert_eq!(schedule.home(), &home);
    }

    #[test]
    fn overlapping_periods_are_rejected_whatever_order_they_arrive_in() {
        let zone = Zone::utc();
        let error = ZoneSchedule::new(
            zone.clone(),
            vec![
                ZonePeriod::new(zone.clone(), instant(5), instant(15)).unwrap(),
                ZonePeriod::new(zone.clone(), instant(0), instant(6)).unwrap(),
            ],
        )
        .unwrap_err();
        assert_eq!(error, TimeError::ZonePeriodsOverlap { from: instant(5) });
        assert_eq!(error.code(), "zone_periods_overlap");
    }

    #[test]
    fn adjacent_periods_join_exactly() {
        let zone = Zone::utc();
        let schedule = ZoneSchedule::new(
            zone.clone(),
            vec![
                ZonePeriod::new(zone.clone(), instant(9), instant(21)).unwrap(),
                ZonePeriod::new(zone.clone(), instant(21), instant(30)).unwrap(),
            ],
        )
        .unwrap();
        assert_eq!(schedule.periods().len(), 2);
        assert!(schedule.periods()[0].contains(instant(20)));
        assert!(!schedule.periods()[0].contains(instant(21)));
        assert!(schedule.periods()[1].contains(instant(21)));
    }

    #[test]
    fn slices_split_on_the_period_edges_inside_the_range_only() {
        let zone = Zone::utc();
        let other = Zone::utc();
        // Two periods that touch at nine, so that instant is one boundary rather
        // than two, and a range that starts inside the first period and ends inside
        // the second.
        let schedule = ZoneSchedule::new(
            zone.clone(),
            vec![
                ZonePeriod::new(other.clone(), instant(0), instant(9)).unwrap(),
                ZonePeriod::new(other.clone(), instant(9), instant(30)).unwrap(),
            ],
        )
        .unwrap();
        assert_eq!(
            schedule.slices(instant(5), instant(20)),
            vec![(instant(5), instant(9)), (instant(9), instant(20)),]
        );
        // Both edges inside, both returned; the range's own ends are boundaries it
        // does not need to add.
        assert_eq!(
            schedule.slices(instant(0), instant(30)),
            vec![(instant(0), instant(9)), (instant(9), instant(30))]
        );
        // A boundary exactly at either end is not repeated.
        assert_eq!(
            schedule.slices(instant(9), instant(30)),
            vec![(instant(9), instant(30))]
        );
        // Outside every period the home zone holds for the whole range.
        assert_eq!(
            schedule.slices(instant(40), instant(50)),
            vec![(instant(40), instant(50))]
        );
        assert!(schedule.slices(instant(20), instant(20)).is_empty());
    }

    #[test]
    fn a_period_must_cover_at_least_one_instant() {
        let zone = Zone::utc();
        assert_eq!(
            ZonePeriod::new(zone.clone(), instant(5), instant(5)),
            Err(TimeError::ZonePeriodEmpty {
                from: instant(5),
                until: instant(5)
            })
        );
        assert_eq!(
            TimeError::ZonePeriodEmpty {
                from: instant(5),
                until: instant(5)
            }
            .code(),
            "zone_period_empty"
        );
    }
}

/// The local-date walk the rest rule and cap buckets read: a whole-day jump is
/// exact only because the walk also stops where the local reading moves without a
/// midnight, and only because it observes every zone the participant can be in.
#[cfg(all(test, feature = "bundled-tzdb"))]
mod bundled_tests {
    use super::{ZonePeriod, ZoneSchedule};
    use crate::time::{Instant, TimeZoneDatabase};
    use std::collections::BTreeSet;

    /// The legs `required_ranges` walks are the slices with the zone in force at
    /// each start: the schedule spans a trip out and back with a gap in the middle
    /// and two periods that touch, which is where a boundary could be lost or
    /// doubled.
    #[test]
    fn legs_are_the_slices_with_the_zone_in_force_at_each_start() {
        let db = TimeZoneDatabase::bundled().unwrap();
        let utc = crate::time::Zone::utc();
        let tokyo = db.zone("Asia/Tokyo").unwrap();
        let new_york = db.zone("America/New_York").unwrap();
        let hour = |hours: i64| Instant::from_minutes(Instant::MIN.minutes() + hours * 60).unwrap();
        let schedule = ZoneSchedule::new(
            utc.clone(),
            vec![
                ZonePeriod::new(tokyo.clone(), hour(2), hour(6)).unwrap(),
                ZonePeriod::new(new_york.clone(), hour(6), hour(10)).unwrap(),
                ZonePeriod::new(tokyo.clone(), hour(14), hour(20)).unwrap(),
            ],
        )
        .unwrap();
        for (from, until) in [
            (hour(0), hour(24)),
            (hour(3), hour(8)),
            (hour(6), hour(6)),
            (hour(5), hour(21)),
            (hour(22), hour(30)),
            (hour(0), hour(2)),
            (hour(20), hour(21)),
        ] {
            let expected: Vec<_> = schedule
                .slices(from, until)
                .into_iter()
                .map(|(start, end)| (schedule.zone_at(start), start, end))
                .collect();
            let walked: Vec<_> = schedule.legs(from, until).collect();
            assert_eq!(walked, expected, "{from:?}..{until:?}");
        }
    }

    /// A backward offset transition that crosses midnight backwards —
    /// Newfoundland's 00:01 daylight saving end until 2010, Casey's three-hour
    /// move in 2010 — repeats a local date, and losing it would pair a rest rule
    /// against the wrong day. This compares the walk with a minute-by-minute
    /// collection over every such transition the bundled database holds, from a
    /// range of starts either side of it.
    #[test]
    fn the_date_walk_collects_every_local_date_a_transition_repeats() {
        let db = TimeZoneDatabase::bundled().unwrap();
        let mut checked = 0usize;
        for name in db.available_names() {
            let zone = db.zone(&name).unwrap();
            let schedule = ZoneSchedule::fixed(zone.clone());
            let repeats = zone
                .transition_instants(Instant::MIN, Instant::MAX)
                .unwrap()
                .into_iter()
                .filter(|transition| {
                    Instant::from_minutes(transition.minutes() - 1).is_ok_and(|before| {
                        zone.zoned(*transition).date() < zone.zoned(before).date()
                    })
                })
                .collect::<Vec<_>>();
            for transition in repeats {
                for offset in [-2_880i64, -1_440, -120, -61, -60, -30, -1, 0, 1, 60] {
                    let from = transition.minutes() + offset;
                    let until = from + 600;
                    let (Ok(start), Ok(end)) =
                        (Instant::from_minutes(from), Instant::from_minutes(until))
                    else {
                        continue;
                    };
                    let jumps = schedule.date_jumps(start, end).unwrap();
                    let mut walked = BTreeSet::new();
                    schedule.dates_in(&jumps, from, until, &mut walked);
                    let mut expected = BTreeSet::new();
                    for minute in from..until {
                        let Ok(instant) = Instant::from_minutes(minute) else {
                            continue;
                        };
                        expected.insert(zone.zoned(instant).date());
                    }
                    checked += 1;
                    assert_eq!(
                        walked, expected,
                        "{name}: the walk from one minute before {transition} plus {offset}"
                    );
                }
            }
        }
        assert!(
            checked > 0,
            "the bundled database no longer holds a transition that repeats a local \
             date, so this walk has nothing left to prove"
        );
    }

    /// The same guarantee when the repeating transition belongs to a zone the
    /// participant is only visiting: the walk has to collect the transitions of
    /// every zone a period puts them in, not just the home zone's. A traveller
    /// whose home zone has no transition at all still repeats a local date when
    /// the leg's zone falls back across midnight.
    #[test]
    fn the_date_walk_stops_at_a_period_zones_repeated_date() {
        let db = TimeZoneDatabase::bundled().unwrap();
        let home = db.zone("UTC").unwrap();
        let mut checked = 0usize;
        for name in db.available_names() {
            let zone = db.zone(&name).unwrap();
            let repeats = zone
                .transition_instants(Instant::MIN, Instant::MAX)
                .unwrap()
                .into_iter()
                .filter(|transition| {
                    Instant::from_minutes(transition.minutes() - 1).is_ok_and(|before| {
                        zone.zoned(*transition).date() < zone.zoned(before).date()
                    })
                })
                .collect::<Vec<_>>();
            for transition in repeats {
                let (Ok(leg_from), Ok(leg_until)) = (
                    Instant::from_minutes(transition.minutes() - 2 * 1_440),
                    Instant::from_minutes(transition.minutes() + 2 * 1_440),
                ) else {
                    continue;
                };
                let schedule = ZoneSchedule::new(
                    home.clone(),
                    vec![ZonePeriod::new(zone.clone(), leg_from, leg_until).unwrap()],
                )
                .unwrap();
                for offset in [-1_440i64, -61, -1, 0, 1, 60] {
                    let from = transition.minutes() + offset;
                    let until = from + 600;
                    let (Ok(start), Ok(end)) =
                        (Instant::from_minutes(from), Instant::from_minutes(until))
                    else {
                        continue;
                    };
                    let jumps = schedule.date_jumps(start, end).unwrap();
                    let mut walked = BTreeSet::new();
                    schedule.dates_in(&jumps, from, until, &mut walked);
                    let mut expected = BTreeSet::new();
                    for minute in from..until {
                        let Ok(instant) = Instant::from_minutes(minute) else {
                            continue;
                        };
                        expected.insert(schedule.zone_at(instant).zoned(instant).date());
                    }
                    checked += 1;
                    assert_eq!(
                        walked, expected,
                        "{name} visited: the walk from one minute before {transition} \
                         plus {offset} loses the date the leg's zone repeats"
                    );
                }
            }
        }
        assert!(
            checked > 0,
            "the bundled database no longer holds a transitional zone to visit, so \
             this walk has nothing left to prove"
        );
    }

    /// A forward transition can swallow a whole civil date, as Samoa's did on
    /// 2011-12-30. No instant falls on the missing date, so the walk must not
    /// invent it, and it must still collect every date on either side. The date
    /// after the skip begins at the transition itself, not at a midnight.
    #[test]
    fn the_date_walk_neither_invents_nor_loses_a_skipped_date() {
        let db = TimeZoneDatabase::bundled().unwrap();
        let mut checked = 0usize;
        for name in db.available_names() {
            let zone = db.zone(&name).unwrap();
            let schedule = ZoneSchedule::fixed(zone.clone());
            let skips = zone
                .transition_instants(Instant::MIN, Instant::MAX)
                .unwrap()
                .into_iter()
                .filter(|transition| {
                    Instant::from_minutes(transition.minutes() - 1).is_ok_and(|before| {
                        zone.zoned(*transition)
                            .date()
                            .plus_days(-1)
                            .is_ok_and(|previous| zone.zoned(before).date() < previous)
                    })
                })
                .collect::<Vec<_>>();
            for transition in skips {
                for offset in [-1_440i64, -61, -1, 0, 60] {
                    let from = transition.minutes() + offset;
                    let until = from + 240;
                    let (Ok(start), Ok(end)) =
                        (Instant::from_minutes(from), Instant::from_minutes(until))
                    else {
                        continue;
                    };
                    let jumps = schedule.date_jumps(start, end).unwrap();
                    let mut walked = BTreeSet::new();
                    schedule.dates_in(&jumps, from, until, &mut walked);
                    let mut expected = BTreeSet::new();
                    for minute in from..until {
                        let Ok(instant) = Instant::from_minutes(minute) else {
                            continue;
                        };
                        expected.insert(zone.zoned(instant).date());
                    }
                    checked += 1;
                    assert_eq!(
                        walked, expected,
                        "{name}: the walk from one minute before {transition} plus \
                         {offset} invents or loses a date across the skipped day"
                    );
                }
            }
        }
        assert!(
            checked > 0,
            "the bundled database no longer holds a transition that skips a civil \
             date, so this walk has nothing left to prove"
        );
    }
}
