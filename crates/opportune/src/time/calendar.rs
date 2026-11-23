//! Working calendars and `lead_time` horizons.
//!
//! A [`WorkingCalendar`] names a zone, the weekdays that are worked and the
//! holidays that are not. A [`Horizon`] bounds how far ahead a booking may
//! start, in one of the four forms the `lead_time` rule allows: a duration, a
//! number of calendar days, a number of working days on a calendar, or a civil
//! date.
//!
//! Date-based horizons compare civil *dates* rather than midnights, using the
//! start's local date in the event zone (or in the calendar's zone for a
//! working-day count). Comparing dates keeps the answer right on days where
//! midnight itself is skipped by a daylight-saving transition.
//!
//! ```
//! use opportune::time::{Duration, Horizon, Instant, Zone};
//!
//! // Resolve a horizon once per request, then test candidate starts against it.
//! let now = Instant::MIN;
//! let horizon = Horizon::Duration(Duration::from_hours_minutes(4, 0));
//! let bound = horizon.bound(now, &Zone::utc()).unwrap();
//! assert!(bound.allows(now));
//! // The whole supported range is further ahead than four hours.
//! assert!(!bound.allows(Instant::MAX));
//! ```

use crate::time::error::TimeError;
use crate::time::{Date, Duration, Instant, WorkingDays, Zone};

/// A named set of working days and holidays, in one zone.
///
/// ```
/// use opportune::time::{Date, WorkingCalendar, WorkingDays, Zone};
///
/// let org = WorkingCalendar::new(
///     Zone::utc(),
///     WorkingDays::weekdays(),
///     vec![Date::new(2027, 1, 1).unwrap()],
/// )
/// .unwrap();
/// assert!(!org.is_working_day(Date::new(2027, 1, 1).unwrap()));
/// assert!(org.is_working_day(Date::new(2027, 1, 4).unwrap()));
/// assert!(!org.is_working_day(Date::new(2027, 1, 3).unwrap())); // Sunday
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkingCalendar {
    zone: Zone,
    working_days: WorkingDays,
    /// Sorted and deduplicated, so membership is a binary search.
    holidays: Vec<Date>,
}

impl WorkingCalendar {
    /// Creates a calendar. Holidays may arrive in any order and with repeats;
    /// they are sorted and deduplicated.
    ///
    /// # Errors
    /// Returns [`TimeError::WorkingDaysEmpty`] for a calendar that works no
    /// weekday. A working-day count on such a calendar could never advance, so
    /// the two readings of it — "ends today" and "never ends" — are equally
    /// arbitrary; the empty set is rejected instead of meaning something.
    pub fn new(
        zone: Zone,
        working_days: WorkingDays,
        mut holidays: Vec<Date>,
    ) -> Result<Self, TimeError> {
        if working_days.is_empty() {
            return Err(TimeError::WorkingDaysEmpty);
        }
        holidays.sort_unstable();
        holidays.dedup();
        Ok(Self {
            zone,
            working_days,
            holidays,
        })
    }

    /// The zone whose civil dates the calendar is read in.
    #[must_use]
    pub fn zone(&self) -> &Zone {
        &self.zone
    }

    /// The weekdays the calendar works.
    #[must_use]
    pub const fn working_days(&self) -> WorkingDays {
        self.working_days
    }

    /// The calendar's holidays, sorted.
    #[must_use]
    pub fn holidays(&self) -> &[Date] {
        &self.holidays
    }

    /// Whether `date` is a working day: a worked weekday that is not a holiday.
    #[must_use]
    pub fn is_working_day(&self, date: Date) -> bool {
        self.working_days.contains(date.weekday()) && self.holidays.binary_search(&date).is_err()
    }

    /// The last local date a horizon of `count` working days after `date`
    /// allows, or `None` when the count runs past `ceiling`.
    ///
    /// A count of zero ends the horizon on `date` itself: the rest of today is
    /// inside the horizon but is never one of the counted days. A calendar that
    /// works no weekday cannot be constructed, so every count above zero
    /// advances.
    fn working_day_limit(
        &self,
        date: Date,
        count: u32,
        ceiling: Date,
    ) -> Result<Option<Date>, TimeError> {
        if count == 0 {
            return Ok(Some(date));
        }
        let mut current = date;
        let mut remaining = count;
        // Bounded by `ceiling`, which a caller inside the supported instant
        // range puts at most a working lifetime away.
        loop {
            current = current.plus_days(1)?;
            if current > ceiling {
                return Ok(None);
            }
            if self.is_working_day(current) {
                remaining -= 1;
                if remaining == 0 {
                    return Ok(Some(current));
                }
            }
        }
    }

    /// Resolves a working-day horizon against `now` without cloning this
    /// calendar's holiday list.
    pub(crate) fn working_days_bound(
        &self,
        now: Instant,
        count: u32,
    ) -> Result<StartBound, TimeError> {
        self.working_days_bound_until(now, count, Instant::MAX)
    }

    /// Resolves a working-day horizon against `now`, stopping the calendar walk
    /// once it passes the local date `horizon_end` touches.
    ///
    /// A count larger than the horizon is only ever asked about starts inside that
    /// horizon, and for those a bound that ends past it allows every one: the walk
    /// therefore reports "unbounded" rather than stepping through every remaining
    /// day to the end of the supported calendar. That turns a `working_days` count
    /// of `u32::MAX` across a request's rules from one pass over the whole calendar
    /// per rule into a walk bounded by the search window, and it stays exact for
    /// every start the caller judges: a limit this walk cannot reach lies strictly
    /// after `horizon_end`'s date, so no such start is outside it.
    ///
    /// `horizon_end` is the latest instant the caller will judge — a search's last
    /// candidate, or a confirmation's proposed start. Pass [`Instant::MAX`] for the
    /// exact bound with no cutoff, which is what
    /// [`working_days_bound`](Self::working_days_bound) does.
    pub(crate) fn working_days_bound_until(
        &self,
        now: Instant,
        count: u32,
        horizon_end: Instant,
    ) -> Result<StartBound, TimeError> {
        let today = now.zoned(self.zone()).date();
        // No start carries a later local date than the latest supported instant
        // does, so the walk never needs to pass it.
        let latest = Instant::MAX.zoned(self.zone()).date();
        // One day past the horizon is enough to decide a bound the judged starts
        // cannot reach: a limit after that date is after every judged start's date.
        let ceiling = horizon_end
            .zoned(self.zone())
            .date()
            .plus_days(1)
            .unwrap_or(latest)
            .min(latest);
        let limit = match self.working_day_limit(today, count, ceiling)? {
            Some(date) => StartLimit::Date(date),
            None => StartLimit::Unbounded,
        };
        let zone = self.zone().clone();
        let fast = FastBound::resolve(&limit, &zone);
        Ok(StartBound { limit, zone, fast })
    }
}

/// A `lead_time` maximum: how far ahead a booking's start may be.
///
/// Each form measures from `now` and compares either an instant (for a
/// duration) or a local date (for every calendar form).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Horizon {
    /// At most `now + duration`, compared as instants.
    Duration(Duration),
    /// At most `today + days` calendar days, where `today` is `now`'s local
    /// date in the event zone.
    Days(u32),
    /// At most the `count`-th working day after today, where today is `now`'s
    /// local date in the calendar's zone. The rest of today is inside the
    /// horizon but is not one of the counted days.
    WorkingDays {
        /// How many working days after today remain inside the horizon.
        count: u32,
        /// The calendar that decides which days are working days.
        calendar: WorkingCalendar,
    },
    /// At most this local date in the event zone.
    Date(Date),
}

impl Horizon {
    /// Resolves the horizon into a bound on start instants.
    ///
    /// Resolve once per request: the result answers per start in constant
    /// time, while resolving a working-day count walks a calendar. `now` is the
    /// request's `now`, already rounded up to the next minute; `event_zone` is
    /// the booking's event zone, which supplies the local date for every form
    /// but [`Horizon::WorkingDays`], which uses the calendar's zone.
    ///
    /// ```
    /// use opportune::time::{Date, Horizon, Instant, Zone};
    ///
    /// let now = Instant::MIN; // 1975-01-01T00:00Z
    /// let horizon = Horizon::Date(Date::new(1975, 1, 2).unwrap());
    /// let bound = horizon.bound(now, &Zone::utc()).unwrap();
    /// let inside = Instant::from_unix_seconds(157_852_800).unwrap(); // 1975-01-02T00:00Z
    /// let past = Instant::from_unix_seconds(157_939_200).unwrap(); // 1975-01-03T00:00Z
    /// assert!(bound.allows(inside));
    /// assert!(!bound.allows(past));
    /// ```
    ///
    /// # Errors
    /// Returns [`TimeError::DateOverflow`] if a calendar walk leaves the
    /// representable calendar. A walk is bounded by the latest supported start
    /// date, so a request whose `now` is inside the supported range does not
    /// reach it; the fallback exists because civil date arithmetic is fallible.
    pub fn bound(&self, now: Instant, event_zone: &Zone) -> Result<StartBound, TimeError> {
        let today_in = |zone: &Zone| now.zoned(zone).date();
        let bound = match self {
            Self::Duration(duration) => {
                // A horizon past the last supported instant bounds nothing.
                StartLimit::instant(now.minutes().saturating_add(duration.minutes()))?
            }
            Self::Days(days) => match today_in(event_zone).plus_days(i64::from(*days)) {
                Ok(date) => StartLimit::Date(date),
                // Further ahead than any representable local date, so it
                // bounds no start that can exist.
                Err(TimeError::DateOverflow { .. }) => StartLimit::Unbounded,
                Err(err) => return Err(err),
            },
            Self::WorkingDays { count, calendar } => {
                let today = today_in(calendar.zone());
                // No start carries a later local date than the latest
                // supported instant does, so a count that passes it bounds
                // nothing and the walk can stop there.
                let latest = Instant::MAX.zoned(calendar.zone()).date();
                match calendar.working_day_limit(today, *count, latest)? {
                    Some(date) => StartLimit::Date(date),
                    None => StartLimit::Unbounded,
                }
            }
            Self::Date(date) => StartLimit::Date(*date),
        };
        let zone = match self {
            Self::WorkingDays { calendar, .. } => calendar.zone().clone(),
            _ => event_zone.clone(),
        };
        let fast = FastBound::resolve(&bound, &zone);
        Ok(StartBound {
            limit: bound,
            zone,
            fast,
        })
    }
}

/// A horizon resolved against a request's `now`: the bound it places on
/// booking start instants.
///
/// Resolution is separate from the per-start test because a working-day count
/// walks a calendar, and a search tests a bound against every candidate start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartBound {
    limit: StartLimit,
    /// The zone a start's local date is read in.
    zone: Zone,
    /// Pre-resolved instant comparison when the bound's cutoff has no
    /// backward transition repeating its last allowed date.
    fast: FastBound,
}

impl StartBound {
    /// Whether a booking starting at `start` is inside the horizon.
    ///
    /// ```
    /// # use opportune::time::{Duration, Horizon, Instant, Zone};
    /// let now = Instant::MIN;
    /// let bound = Horizon::Duration(Duration::from_hours_minutes(1, 0))
    ///     .bound(now, &Zone::utc())
    ///     .unwrap();
    /// assert!(bound.allows(now));
    /// assert!(bound.allows(Instant::from_minutes(Instant::MIN.minutes() + 60).unwrap()));
    /// assert!(!bound.allows(Instant::from_minutes(Instant::MIN.minutes() + 61).unwrap()));
    /// ```
    #[must_use]
    pub fn allows(&self, start: Instant) -> bool {
        match self.fast {
            FastBound::Unbounded => true,
            FastBound::Never => false,
            FastBound::UpTo(latest) => start <= latest,
            FastBound::ByDate(latest) => start.zoned(&self.zone).date() <= latest,
        }
    }

    /// Whether this bound rejects every start `other` rejects and, for the two
    /// that combine, more: the comparison `lead_time` uses to pick the earliest
    /// maximum.
    ///
    /// Bounds of different kinds compare by the last instant each allows, so a
    /// calendar-date maximum and a duration maximum are ordered by what they
    /// actually admit. A date's last instant is read from the boundary — the last
    /// instant that still carries the date — which keeps the answer right where a
    /// midnight is skipped by a daylight saving change and where one is repeated,
    /// on either side of that boundary.
    ///
    /// Equal bounds are not stricter than each other: `lead_time` keeps the
    /// earliest rule in request order among equals.
    #[must_use]
    pub fn is_stricter_than(&self, other: &Self) -> bool {
        self.strictness() < other.strictness()
    }

    /// How much this bound restricts starts, ordered from strictest to loosest.
    fn strictness(&self) -> Strictness {
        match &self.limit {
            StartLimit::Unbounded => Strictness::Unlimited,
            StartLimit::Instant(latest) => Strictness::UpTo(*latest),
            StartLimit::Date(latest) => {
                let first_supported = Instant::MIN.zoned(&self.zone).date();
                if *latest < first_supported {
                    // The limit's own date is below the first date a start can
                    // fall on, so no start is inside it.
                    return Strictness::Never;
                }
                // A limit at or past the last local date bounds nothing on its own,
                // and its boundary lies outside the supported instants: every start
                // that can exist reads a date at or before it.
                if *latest >= Instant::MAX.zoned(&self.zone).date() {
                    return Strictness::Unlimited;
                }
                match self.zone.last_instant_on(*latest) {
                    // The last instant that still reads the limit's date. Reading
                    // the boundary back keeps this exact where a backward
                    // transition repeats a midnight: the repeat can lie before the
                    // next date's last midnight (Newfoundland) or after its first
                    // one (Cuba), and only the local reading decides which.
                    Ok(Some(last)) => Strictness::UpTo(last),
                    // No supported instant reads a date at or before the limit, so
                    // the limit admits no start at all.
                    Ok(None) => Strictness::Never,
                    // A zone whose offsets are not whole minutes cannot be
                    // classified exactly either, and treating it as bounding
                    // nothing is the direction that neither narrows a search nor
                    // drops a rule.
                    Err(_) => Strictness::Unlimited,
                }
            }
        }
    }
}

/// The order of [`StartBound::is_stricter_than`]: no start at all, then a last
/// instant, then no bound. Variant order is the compare order.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Strictness {
    /// No start is inside the bound.
    Never,
    /// Starts up to this instant are inside.
    UpTo(Instant),
    /// Every start that can exist is inside.
    Unlimited,
}

/// Pre-resolved fast path for [`StartBound::allows`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FastBound {
    /// Every supported start is inside the bound.
    Unbounded,
    /// No supported start is inside the bound.
    Never,
    /// Allowed iff `start <= latest`.
    UpTo(Instant),
    /// Fall back to comparing local dates in the bound's zone.
    ByDate(Date),
}

impl FastBound {
    /// Resolves `limit` into an integer instant cutoff whenever no backward
    /// offset transition can repeat `latest` after `latest + 1` has begun.
    fn resolve(limit: &StartLimit, zone: &Zone) -> Self {
        match limit {
            StartLimit::Unbounded => Self::Unbounded,
            StartLimit::Instant(latest) => Self::UpTo(*latest),
            StartLimit::Date(latest) => Self::resolve_date(*latest, zone),
        }
    }

    fn resolve_date(latest: Date, zone: &Zone) -> Self {
        let Ok(next) = latest.plus_days(1) else {
            return Self::Unbounded;
        };
        let Ok(start_of_next) = zone.day_start(next) else {
            return Self::ByDate(latest);
        };
        // `day_start(next)` is the first instant whose local date is `>= next`,
        // so every instant before it is `<= latest`. An instant at or after it
        // can only read `<= latest` if a backward offset transition repeats
        // `latest` within a couple of days after `start_of_next`.
        let probe_until = Instant::from_minutes(
            start_of_next
                .minutes()
                .saturating_add(48 * 60)
                .min(Instant::MAX.minutes()),
        )
        .unwrap_or(Instant::MAX);
        let Ok(transitions) = zone.transition_instants(start_of_next, probe_until) else {
            return Self::ByDate(latest);
        };
        if transitions
            .iter()
            .any(|transition| zone.zoned(*transition).date() <= latest)
        {
            return Self::ByDate(latest);
        }
        match Instant::from_minutes(start_of_next.minutes() - 1) {
            Ok(last) => Self::UpTo(last),
            Err(_) => Self::Never,
        }
    }
}

/// What a horizon bounds starts by.
#[derive(Clone, Debug, PartialEq, Eq)]
enum StartLimit {
    /// No bound: every start that can exist is inside.
    Unbounded,
    /// The latest start instant.
    Instant(Instant),
    /// The latest local date a start may fall on, read in the bound's zone.
    Date(Date),
}

impl StartLimit {
    /// The bound for a latest minute count, unbounded past the supported range.
    fn instant(latest_minutes: i64) -> Result<Self, TimeError> {
        match Instant::from_minutes(latest_minutes) {
            Ok(instant) => Ok(Self::Instant(instant)),
            // Past the last supported instant, so no start can exceed it.
            Err(TimeError::InstantOutOfRange { .. }) => Ok(Self::Unbounded),
            Err(err) => Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Horizon, StartBound, WorkingCalendar, WorkingDays};
    use crate::time::{
        Date, Duration, Instant, LocalResolution, TimeError, TimeOfDay, Weekday, Zone,
    };

    /// The instant of a UTC civil date-time.
    fn utc_instant(y: i16, m: i8, d: i8, hour: u8, minute: u8) -> Instant {
        let LocalResolution::Unique(instant) = Zone::utc()
            .resolve_local(
                Date::new(y, m, d).unwrap(),
                TimeOfDay::new(hour, minute).unwrap(),
            )
            .unwrap()
        else {
            panic!("UTC has no transitions")
        };
        instant
    }

    fn org() -> WorkingCalendar {
        WorkingCalendar::new(
            Zone::utc(),
            WorkingDays::weekdays(),
            vec![
                Date::new(2027, 1, 1).unwrap(),
                Date::new(2027, 12, 24).unwrap(),
                // Duplicated on purpose: the calendar deduplicates.
                Date::new(2027, 12, 24).unwrap(),
            ],
        )
        .unwrap()
    }

    /// Resolves a horizon against `now` in UTC and answers for one start.
    fn allows(horizon: &Horizon, start: Instant, now: Instant) -> bool {
        let bound: StartBound = horizon.bound(now, &Zone::utc()).unwrap();
        bound.allows(start)
    }

    #[test]
    fn working_days_are_weekdays_less_holidays() {
        let calendar = org();
        assert_eq!(calendar.holidays().len(), 2);
        assert!(calendar.is_working_day(Date::new(2027, 3, 9).unwrap()));
        assert!(!calendar.is_working_day(Date::new(2027, 3, 13).unwrap()));
        assert!(!calendar.is_working_day(Date::new(2027, 12, 24).unwrap()));
        assert!(
            WorkingDays::list([Weekday::Saturday, Weekday::Sunday]).contains(Weekday::Saturday)
        );
        assert_eq!(
            format!("{:?}", WorkingDays::weekdays()),
            "mon,tue,wed,thu,fri"
        );
    }

    #[test]
    fn working_day_counts_skip_weekends_and_holidays() {
        let calendar = org();
        let horizon = Horizon::WorkingDays {
            count: 3,
            calendar: calendar.clone(),
        };
        // Friday 2027-03-12 09:00 to Wednesday 2027-03-17: three working days
        // later, after the weekend.
        let now = utc_instant(2027, 3, 12, 9, 0);
        assert!(allows(&horizon, utc_instant(2027, 3, 13, 12, 0), now));
        assert!(allows(&horizon, utc_instant(2027, 3, 17, 23, 59), now));
        assert!(!allows(&horizon, utc_instant(2027, 3, 18, 0, 0), now));

        // Across the New Year holiday and a weekend.
        let horizon = Horizon::WorkingDays { count: 2, calendar };
        let now = utc_instant(2027, 12, 30, 9, 0);
        assert!(allows(&horizon, utc_instant(2028, 1, 3, 23, 0), now));
        assert!(!allows(&horizon, utc_instant(2028, 1, 4, 0, 0), now));
    }

    #[test]
    fn a_zero_day_horizon_ends_the_horizon_today() {
        let now = utc_instant(2027, 3, 10, 9, 0);
        let horizon = Horizon::WorkingDays {
            count: 0,
            calendar: org(),
        };
        assert!(allows(&horizon, utc_instant(2027, 3, 10, 23, 59), now));
        assert!(!allows(&horizon, utc_instant(2027, 3, 11, 0, 0), now));
    }

    /// A calendar that works no weekday makes a count of working days
    /// meaningless: nothing after today is ever counted, so the count can only
    /// end today or never end. Rejecting the calendar is the answer that cannot
    /// be misread.
    #[test]
    fn a_calendar_working_no_weekday_is_rejected() {
        let error = WorkingCalendar::new(Zone::utc(), WorkingDays::NONE, Vec::new()).unwrap_err();
        assert_eq!(error, TimeError::WorkingDaysEmpty);
        assert_eq!(error.code(), "working_days_empty");
    }

    /// The comparison `lead_time` uses to pick the earliest maximum has to order
    /// bounds of different kinds by what they admit, not by their kind. A
    /// calendar-date maximum ends one minute before the next date begins, which
    /// is not a midnight where a daylight saving change skips one.
    #[test]
    fn bounds_compare_by_the_last_start_they_allow() {
        let zone = Zone::utc();
        let now = utc_instant(2027, 3, 9, 0, 0);
        let dates = Horizon::Date(Date::new(2027, 3, 11).unwrap())
            .bound(now, &zone)
            .unwrap();
        let hours = Horizon::Duration(Duration::from_hours_minutes(24, 0))
            .bound(now, &zone)
            .unwrap();
        // A date maximum admits the whole of 11 March; a 24-hour duration from
        // 9 March 00:00 ends at 10 March 00:00, so it is the stricter bound.
        assert!(hours.is_stricter_than(&dates));
        assert!(!dates.is_stricter_than(&hours));

        let unrestricted = Horizon::Days(u32::MAX).bound(now, &zone).unwrap();
        assert!(dates.is_stricter_than(&unrestricted));
        assert!(hours.is_stricter_than(&unrestricted));
        assert!(!unrestricted.is_stricter_than(&dates));
        assert!(!dates.is_stricter_than(&dates), "equals are not stricter");
    }

    /// A date maximum whose last local day is followed by a day with no local
    /// midnight ends one minute before the transition that begins it.
    #[cfg(feature = "bundled-tzdb")]
    #[test]
    fn a_date_maximum_ends_where_the_next_local_day_begins() {
        // Santiago springs forward from midnight to 01:00 on 5 September 2027.
        let santiago = crate::time::TimeZoneDatabase::bundled()
            .unwrap()
            .zone("America/Santiago")
            .unwrap();
        let now = utc_instant(2027, 9, 1, 0, 0);
        let dates = Horizon::Date(Date::new(2027, 9, 4).unwrap())
            .bound(now, &santiago)
            .unwrap();
        let first_of_the_5th = utc_instant(2027, 9, 5, 4, 0);
        let ends_at = |end: Instant| {
            Horizon::Duration(Duration::from_minutes(
                u32::try_from(end.minutes() - now.minutes()).unwrap(),
            ))
            .bound(now, &santiago)
            .unwrap()
        };
        let one_minute_before = Instant::from_minutes(first_of_the_5th.minutes() - 1).unwrap();
        assert!(
            !ends_at(one_minute_before).is_stricter_than(&dates),
            "a duration ending where the date maximum ends is not stricter"
        );
        assert!(
            !dates.is_stricter_than(&ends_at(one_minute_before)),
            "and neither is the date maximum"
        );
        assert!(
            dates.is_stricter_than(&ends_at(first_of_the_5th)),
            "the date maximum stops one minute before the 5th begins"
        );
        assert!(
            ends_at(Instant::from_minutes(one_minute_before.minutes() - 1).unwrap())
                .is_stricter_than(&dates)
        );
    }

    #[test]
    fn a_horizon_past_the_calendar_bounds_nothing() {
        let now = utc_instant(2027, 3, 9, 10, 0);
        for horizon in [
            Horizon::Days(u32::MAX),
            Horizon::WorkingDays {
                count: u32::MAX,
                calendar: org(),
            },
        ] {
            assert!(allows(&horizon, Instant::MAX, now), "{horizon:?}");
        }
    }

    #[test]
    fn durations_compare_instants() {
        let now = utc_instant(2027, 3, 9, 10, 0);
        let horizon = Horizon::Duration(Duration::from_hours_minutes(4, 0));
        // A start at `now` is inside a four-hour horizon; a duration horizon
        // only bounds how far ahead a start may be.
        assert!(allows(&horizon, now, now));
        assert!(allows(&horizon, utc_instant(2027, 3, 9, 14, 0), now));
        assert!(!allows(&horizon, utc_instant(2027, 3, 9, 14, 1), now));
    }

    #[test]
    fn the_rest_of_today_is_inside_a_working_day_horizon_without_being_counted() {
        let horizon = Horizon::WorkingDays {
            count: 3,
            calendar: org(),
        };
        // Wednesday 2027-03-10 at 09:00 local.
        let now = utc_instant(2027, 3, 10, 9, 0);
        // Later the same day is inside the horizon.
        assert!(allows(&horizon, utc_instant(2027, 3, 10, 18, 0), now));
        // Thursday and Friday are the first two counted days, so Monday is the
        // third and the last day inside the horizon.
        assert!(allows(&horizon, utc_instant(2027, 3, 15, 12, 0), now));
        assert!(!allows(&horizon, utc_instant(2027, 3, 16, 0, 0), now));
    }

    #[test]
    fn calendar_days_cross_leap_days() {
        let horizon = Horizon::Days(2);
        let now = utc_instant(2028, 2, 28, 12, 0);
        // The 29th exists in 2028, so the horizon ends on 1 March.
        assert!(allows(&horizon, utc_instant(2028, 3, 1, 23, 0), now));
        assert!(!allows(&horizon, utc_instant(2028, 3, 2, 0, 0), now));

        let horizon = Horizon::Date(Date::new(2028, 2, 29).unwrap());
        assert!(allows(&horizon, utc_instant(2028, 2, 29, 23, 59), now));
        assert!(!allows(&horizon, utc_instant(2028, 3, 1, 0, 0), now));
    }

    #[test]
    fn a_zero_day_horizon_ends_at_the_end_of_today() {
        let horizon = Horizon::Days(0);
        let now = utc_instant(2027, 3, 9, 23, 30);
        assert!(allows(&horizon, utc_instant(2027, 3, 9, 23, 45), now));
        assert!(!allows(&horizon, utc_instant(2027, 3, 10, 0, 0), now));
    }
}
