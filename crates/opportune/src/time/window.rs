//! Validated wall-clock `window` specifications and the instants they allow.
//!
//! A [`WindowSpec`] is the parsed form of a window rule's `weekly` and `dates`
//! keys: a zone selection, hours per weekday and date overrides, all validated
//! once at construction, so evaluation never has to ask whether the
//! specification is well formed.
//!
//! Membership is decided from the instant's local date and time alone — an
//! instant satisfies the rule when its local reading does — so daylight saving
//! transitions need no special case: a local time inside a skipped hour simply
//! never occurs, and a repeated local time is judged twice, once per instant.
//! [`WindowSpec::intervals`] builds the same answer as half-open UTC minute
//! spans with the segment-start breakpoint algorithm.
//!
//! A specification names its zone rather than holding one, so a rule is a value
//! a caller can write down without a time zone database;
//! [`RuleZone::resolve`] turns the choice into the zone function the evaluation
//! reads, once per participant.
//!
//! ```
//! use opportune::intervals::Span;
//! use opportune::time::{
//!     Date, Instant, LocalResolution, RuleZone, TimeOfDay, WallEnd, WallRange, Weekday, Weekdays,
//!     WeeklyKey, WindowSpec, Zone, ZoneName, ZoneSchedule,
//! };
//!
//! let hours = WallRange::new(
//!     TimeOfDay::new(9, 0).unwrap(),
//!     WallEnd::Time(TimeOfDay::new(17, 0).unwrap()),
//! )
//! .unwrap();
//! let spec = WindowSpec::new(
//!     RuleZone::Named(ZoneName::new("UTC").unwrap()),
//!     vec![WeeklyKey::new(Weekdays::single(Weekday::Monday), vec![hours]).unwrap()],
//!     Vec::new(),
//! )
//! .unwrap();
//!
//! let at = |hour: u8| match Zone::utc()
//!     .resolve_local(Date::new(2027, 3, 8).unwrap(), TimeOfDay::new(hour, 0).unwrap())
//!     .unwrap()
//! {
//!     LocalResolution::Unique(instant) => instant,
//!     _ => panic!("UTC has no transitions"),
//! };
//! let schedule = ZoneSchedule::fixed(Zone::utc());
//! assert!(spec.contains(at(10), &schedule));
//! assert!(!spec.contains(at(8), &schedule));
//!
//! let day = spec
//!     .intervals(
//!         &schedule,
//!         at(0),
//!         Instant::from_minutes(at(0).minutes() + 1_440).unwrap(),
//!     )
//!     .unwrap();
//! assert_eq!(day.spans(), [Span::new(at(9).minutes(), at(17).minutes()).unwrap()]);
//! ```

use crate::intervals::IntervalSet;
use crate::time::breakpoints::{self, BreakpointSource};
use crate::time::error::{TimeError, WindowError};
use crate::time::{
    Date, Instant, TimeOfDay, TimeZoneDatabase, Weekday, Weekdays, Zone, ZoneName, ZoneSchedule,
};

/// The zone a rule's `zone` field names.
///
/// Each window and blackout is read in one of three zones: the participant's own
/// zone at the instant — which a trip moves — the event's zone, or a named IANA
/// zone. [`RuleZone::resolve`] turns the choice into the zone function the
/// evaluation reads.
///
/// ```
/// # #[cfg(feature = "bundled-tzdb")] {
/// use opportune::time::{RuleZone, TimeZoneDatabase, Zone, ZoneSchedule};
///
/// let db = TimeZoneDatabase::bundled().unwrap();
/// let participant = ZoneSchedule::fixed(Zone::utc());
/// let resolved = RuleZone::Participant
///     .resolve(&db, &participant, &Zone::utc())
///     .unwrap();
/// assert_eq!(resolved.home().name(), "UTC");
/// assert!(resolved.periods().is_empty());
/// # }
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuleZone {
    /// The participant's zone at each instant, following their zone periods.
    Participant,
    /// The event's zone.
    Event,
    /// A named zone, whatever the event's or participant's zone is.
    Named(ZoneName),
}

impl RuleZone {
    /// The zone function this selection names, with every named zone resolved
    /// through `db`.
    ///
    /// `participant` supplies the zone periods for [`RuleZone::Participant`];
    /// `event` supplies the zone for [`RuleZone::Event`]. Each variant reads
    /// only one of the two, so a caller holding a single zone can pass it as
    /// `event` and [`ZoneSchedule::fixed`] over it as `participant`.
    ///
    /// # Errors
    /// Returns [`TimeError::ZoneUnknown`] when a named zone is not in `db`.
    pub fn resolve(
        &self,
        db: &TimeZoneDatabase,
        participant: &ZoneSchedule,
        event: &Zone,
    ) -> Result<ZoneSchedule, TimeError> {
        match self {
            Self::Participant => Ok(participant.clone()),
            Self::Event => Ok(ZoneSchedule::fixed(event.clone())),
            Self::Named(name) => Ok(ZoneSchedule::fixed(db.zone(name.as_str())?)),
        }
    }
}

/// The end of a wall-clock range, which may be `24:00`.
///
/// A [`TimeOfDay`] stops at `23:59`, so the end of the day needs its own value:
/// `24:00` is the instant the day ends, not the start of the next one.
///
/// ```
/// use opportune::time::{TimeOfDay, WallEnd};
///
/// let end = WallEnd::Time(TimeOfDay::new(17, 0).unwrap());
/// assert_eq!(end.minutes(), 1_020);
/// assert_eq!(WallEnd::Midnight.minutes(), 1_440);
/// assert_eq!(WallEnd::Midnight.to_string(), "24:00");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WallEnd {
    /// A time of day from `00:00` to `23:59`.
    Time(TimeOfDay),
    /// `24:00`, the end of the day.
    Midnight,
}

impl WallEnd {
    /// The end in minutes from the start of the day: `0` to `1440`.
    #[must_use]
    pub fn minutes(&self) -> u16 {
        match self {
            Self::Time(time) => time.minute_of_day(),
            Self::Midnight => TimeOfDay::MINUTES_PER_DAY,
        }
    }
}

impl core::fmt::Display for WallEnd {
    /// Formats the end as `HH:MM`, or `24:00` for the end of the day.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Time(time) => time.fmt(f),
            Self::Midnight => f.write_str("24:00"),
        }
    }
}

impl From<TimeOfDay> for WallEnd {
    /// The end of a range at a time of day, so a range can be written
    /// `WallRange::new(from, until)` directly with a [`TimeOfDay`].
    fn from(time: TimeOfDay) -> Self {
        Self::Time(time)
    }
}

/// A half-open wall-clock range `[start, end)` within a day, on one day's key.
///
/// A range whose end is earlier than its start crosses midnight: `22:00-02:00`
/// runs from 22:00 on the key's day to 02:00 on the next one. `24:00` is a valid
/// end; a start equal to its end is rejected, because `00:00-00:00` is almost
/// certainly a mistyped `00:00-24:00`.
///
/// ```
/// use opportune::time::{TimeOfDay, WallEnd, WallRange};
///
/// let workday = WallRange::new(
///     TimeOfDay::new(9, 0).unwrap(),
///     WallEnd::Time(TimeOfDay::new(17, 0).unwrap()),
/// )
/// .unwrap();
/// assert_eq!(workday.start().to_string(), "09:00");
/// assert_eq!(workday.end().to_string(), "17:00");
///
/// let equal = WallRange::new(
///     TimeOfDay::MIDNIGHT,
///     WallEnd::Time(TimeOfDay::MIDNIGHT),
/// );
/// assert_eq!(
///     equal.unwrap_err().code(),
///     "window_range_empty"
/// );
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WallRange {
    start: TimeOfDay,
    end: WallEnd,
}

impl WallRange {
    /// Creates the range `[start, end)`.
    ///
    /// # Errors
    /// Returns [`WindowError::RangeEmpty`] when `start` and `end` name the same
    /// minute, as `00:00-00:00` does.
    pub fn new(start: TimeOfDay, end: impl Into<WallEnd>) -> Result<Self, WindowError> {
        let end = end.into();
        if start.minute_of_day() == end.minutes() {
            return Err(WindowError::RangeEmpty { start });
        }
        Ok(Self { start, end })
    }

    /// The range's start.
    #[must_use]
    pub const fn start(&self) -> TimeOfDay {
        self.start
    }

    /// The range's end.
    #[must_use]
    pub const fn end(&self) -> WallEnd {
        self.end
    }

    /// The start in minutes from the start of the day.
    fn start_minutes(self) -> u16 {
        self.start.minute_of_day()
    }

    /// The end in minutes from the start of the day on which the range starts,
    /// so a crossing range's end is past `1440`.
    fn linear_end(self) -> u16 {
        if self.crosses_midnight() {
            self.end.minutes() + TimeOfDay::MINUTES_PER_DAY
        } else {
            self.end.minutes()
        }
    }

    /// Whether the range runs past the end of the day it starts on.
    fn crosses_midnight(self) -> bool {
        self.end.minutes() < self.start_minutes()
    }

    /// Whether the range covers `minute` of the day it starts on.
    fn covers(self, minute: u16) -> bool {
        if self.crosses_midnight() {
            minute >= self.start_minutes()
        } else {
            self.start_minutes() <= minute && minute < self.end.minutes()
        }
    }

    /// Whether a range that started on the previous date still covers `minute`
    /// of this one.
    fn spills_over(self, minute: u16) -> bool {
        self.crosses_midnight() && minute < self.end.minutes()
    }

    /// The local date and time at which this range stops, when it starts on
    /// `date`.
    fn end_boundary(self, date: Date) -> Result<(Date, TimeOfDay), TimeError> {
        match self.end {
            WallEnd::Time(time) if self.crosses_midnight() => Ok((date.plus_days(1)?, time)),
            WallEnd::Time(time) => Ok((date, time)),
            WallEnd::Midnight => Ok((date.plus_days(1)?, TimeOfDay::MIDNIGHT)),
        }
    }
}

/// One `weekly` key: the days it covers and the hours they share.
///
/// ```
/// use opportune::time::{TimeOfDay, WallEnd, WallRange, Weekday, Weekdays, WeeklyKey};
///
/// let hours = WallRange::new(
///     TimeOfDay::new(9, 0).unwrap(),
///     WallEnd::Time(TimeOfDay::new(17, 0).unwrap()),
/// )
/// .unwrap();
/// let key = WeeklyKey::new(
///     Weekdays::list([Weekday::Monday, Weekday::Wednesday]),
///     vec![hours],
/// )
/// .unwrap();
/// assert!(key.days().contains(Weekday::Monday));
/// assert_eq!(key.hours().len(), 1);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WeeklyKey {
    days: Weekdays,
    hours: Vec<WallRange>,
}

impl WeeklyKey {
    /// Creates a key from the days it covers and the ranges they share.
    ///
    /// # Errors
    /// Returns [`WindowError::RangesOverlap`] when two ranges within the key
    /// overlap. A midnight-crossing range is measured past the end of the day it
    /// starts on, so `22:00-02:00` overlaps `23:00-01:00` but not `01:00-03:00`.
    pub fn new(days: Weekdays, hours: Vec<WallRange>) -> Result<Self, WindowError> {
        Ok(Self {
            days,
            hours: non_overlapping(hours)?,
        })
    }

    /// The days this key covers.
    #[must_use]
    pub fn days(&self) -> &Weekdays {
        &self.days
    }

    /// The hours every covered day shares, sorted by start.
    #[must_use]
    pub fn hours(&self) -> &[WallRange] {
        &self.hours
    }
}

/// An inclusive range of civil dates, the key side of a `dates` entry.
///
/// ```
/// use opportune::time::{Date, DateRange};
///
/// let holiday = DateRange::new(
///     Date::new(2026, 12, 24).unwrap(),
///     Date::new(2027, 1, 2).unwrap(),
/// )
/// .unwrap();
/// assert!(holiday.contains(Date::new(2026, 12, 25).unwrap()));
/// assert!(!holiday.contains(Date::new(2027, 1, 3).unwrap()));
/// assert!(DateRange::new(Date::new(2027, 1, 2).unwrap(), Date::new(2026, 12, 24).unwrap()).is_err());
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DateRange {
    start: Date,
    end: Date,
}

impl DateRange {
    /// Creates the inclusive range from `start` to `end`.
    ///
    /// # Errors
    /// Returns [`WindowError::DateRangeReversed`] when `end` falls before
    /// `start`.
    pub fn new(start: Date, end: Date) -> Result<Self, WindowError> {
        if end < start {
            return Err(WindowError::DateRangeReversed { start, end });
        }
        Ok(Self { start, end })
    }

    /// The range holding only `date`.
    #[must_use]
    pub fn single(date: Date) -> Self {
        Self {
            start: date,
            end: date,
        }
    }

    /// The first date in the range.
    #[must_use]
    pub const fn start(&self) -> Date {
        self.start
    }

    /// The last date in the range.
    #[must_use]
    pub const fn end(&self) -> Date {
        self.end
    }

    /// Whether the range covers `date`.
    #[must_use]
    pub fn contains(&self, date: Date) -> bool {
        self.start <= date && date <= self.end
    }
}

/// One `dates` entry: the dates it overrides and the hours they have.
///
/// An entry with no hours closes its dates, whatever the weekly key would have
/// allowed. An override also governs its own midnight-crossing spill: the spill
/// from an overridden date follows the override, and the spill *into* an
/// overridden date still comes from the previous date's key.
///
/// ```
/// use opportune::time::{Date, DateRange, DateKey};
///
/// // Christmas Day is closed.
/// let christmas = DateKey::new(
///     DateRange::single(Date::new(2026, 12, 25).unwrap()),
///     Vec::new(),
/// )
/// .unwrap();
/// assert!(christmas.hours().is_empty());
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DateKey {
    dates: DateRange,
    hours: Vec<WallRange>,
}

impl DateKey {
    /// Creates a key from the dates it overrides and the hours they have.
    ///
    /// # Errors
    /// Returns [`WindowError::RangesOverlap`] when two ranges within the key
    /// overlap.
    pub fn new(dates: DateRange, hours: Vec<WallRange>) -> Result<Self, WindowError> {
        Ok(Self {
            dates,
            hours: non_overlapping(hours)?,
        })
    }

    /// The dates this key overrides.
    #[must_use]
    pub fn dates(&self) -> &DateRange {
        &self.dates
    }

    /// The hours those dates have, sorted by start. Empty means closed.
    #[must_use]
    pub fn hours(&self) -> &[WallRange] {
        &self.hours
    }
}

/// A validated wall-clock `window` specification.
///
/// The specification is the parsed form of a window rule: the zone it is read
/// in, its `weekly` keys and its `dates` overrides. Construction rejects every
/// structurally ambiguous combination — a weekday or date claimed by two keys, a
/// reversed date range, overlapping ranges within a key, a range whose start
/// equals its end — so membership and interval building below can be total.
///
/// An instant is inside when its local time `m` on its local date `D` satisfies:
/// a range of `D`'s key with `a ≤ m < b`; a midnight-crossing range of `D`'s key
/// with `m ≥ a`; or a midnight-crossing range of `D - 1`'s key with `m < b`. A
/// date's key is its `dates` entry if it has one, otherwise its `weekly` day.
///
/// ```
/// use opportune::time::{
///     Date, Instant, LocalResolution, RuleZone, TimeOfDay, WallRange, WallEnd, Weekday, Weekdays,
///     WeeklyKey, WindowSpec, Zone, ZoneName, ZoneSchedule,
/// };
///
/// // Open 22:00 to 02:00 on Fridays, which runs into Saturday morning.
/// let night = WallRange::new(
///     TimeOfDay::new(22, 0).unwrap(),
///     WallEnd::Time(TimeOfDay::new(2, 0).unwrap()),
/// )
/// .unwrap();
/// let spec = WindowSpec::new(
///     RuleZone::Named(ZoneName::new("UTC").unwrap()),
///     vec![WeeklyKey::new(Weekdays::single(Weekday::Friday), vec![night]).unwrap()],
///     Vec::new(),
/// )
/// .unwrap();
/// let zones = ZoneSchedule::fixed(Zone::utc());
///
/// let at = |day: i8, hour: u8| match Zone::utc()
///     .resolve_local(Date::new(2027, 3, day).unwrap(), TimeOfDay::new(hour, 0).unwrap())
///     .unwrap()
/// {
///     LocalResolution::Unique(instant) => instant,
///     _ => panic!("UTC has no transitions"),
/// };
/// // Friday 2027-03-12 is open in the evening and the next morning.
/// assert!(spec.contains(at(12, 23), &zones));
/// assert!(spec.contains(at(13, 1), &zones));
/// assert!(!spec.contains(at(13, 2), &zones));
/// assert!(!spec.contains(at(12, 12), &zones));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowSpec {
    zone: RuleZone,
    /// Indexed Monday to Sunday; an empty list means the day is closed.
    weekly: [Vec<WallRange>; 7],
    /// Sorted by start date and disjoint, so at most one key covers a date.
    dates: Vec<DateKey>,
}

impl WindowSpec {
    /// Creates a specification from its zone selection, `weekly` keys and
    /// `dates` overrides.
    ///
    /// # Errors
    /// Returns [`WindowError::WeekdayRepeated`] when two `weekly` keys claim the
    /// same day and [`WindowError::DateRepeated`] when two `dates` keys claim the
    /// same date. Range-level mistakes are rejected earlier, by [`WallRange`],
    /// [`WeeklyKey`], [`DateKey`] and [`DateRange`].
    pub fn new(
        zone: RuleZone,
        weekly: Vec<WeeklyKey>,
        dates: Vec<DateKey>,
    ) -> Result<Self, WindowError> {
        let mut days: [Vec<WallRange>; 7] = core::array::from_fn(|_| Vec::new());
        let mut claimed = 0u8;
        for key in weekly {
            for day in Weekday::ALL {
                if key.days().contains(day) {
                    if claimed & day.bit() != 0 {
                        return Err(WindowError::WeekdayRepeated { day });
                    }
                    claimed |= day.bit();
                    days[usize::from(day.index())] = key.hours().to_vec();
                }
            }
        }

        let mut dates = dates;
        dates.sort_by_key(|key| key.dates().start());
        for pair in dates.windows(2) {
            if pair[1].dates().start() <= pair[0].dates().end() {
                return Err(WindowError::DateRepeated {
                    date: pair[1].dates().start(),
                });
            }
        }

        Ok(Self {
            zone,
            weekly: days,
            dates,
        })
    }

    /// Whether `instant` is inside, read in `zones` — the zone function this
    /// specification's [`RuleZone`] resolved to.
    ///
    /// ```
    /// # use opportune::time::{Instant, RuleZone, WindowSpec, Zone, ZoneSchedule};
    /// // No keys at all: a window that is closed everywhere.
    /// let closed = WindowSpec::new(RuleZone::Event, Vec::new(), Vec::new()).unwrap();
    /// let zones = ZoneSchedule::fixed(Zone::utc());
    /// assert!(!closed.contains(Instant::MIN, &zones));
    /// ```
    #[must_use]
    pub fn contains(&self, instant: Instant, zones: &ZoneSchedule) -> bool {
        self.member(instant, zones.zone_at(instant))
    }

    /// The minutes of `from..until` that are inside, as half-open UTC spans.
    ///
    /// The result is built over exactly the range asked for — never clipped to
    /// midnight, a day, or any padded bound — and the range need not be
    /// midnight-aligned.
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

    /// The hours of each weekday, Monday first. An empty list means the day is
    /// closed.
    #[must_use]
    pub fn weekly(&self) -> &[Vec<WallRange>; 7] {
        &self.weekly
    }

    /// The date overrides, sorted by start date.
    #[must_use]
    pub fn dates(&self) -> &[DateKey] {
        &self.dates
    }

    /// The hours of the key that governs `date`: its override if it has one,
    /// otherwise its weekday's.
    fn hours_on(&self, date: Date) -> &[WallRange] {
        // Overrides are sorted and disjoint, so at most one can cover the date.
        let index = self.dates.partition_point(|key| key.dates().end() < date);
        if let Some(key) = self.dates.get(index)
            && key.dates().contains(date)
        {
            return key.hours();
        }
        &self.weekly[usize::from(date.weekday().index())]
    }

    /// Membership from the local reading alone, so one rule decides both instant
    /// membership and every segment the interval builder classifies.
    fn contains_local(&self, date: Date, minute: u16) -> bool {
        if self.hours_on(date).iter().any(|range| range.covers(minute)) {
            return true;
        }
        // A crossing range on yesterday's key runs into today. Non-overlapping
        // ranges in `[0, 2880)` allow at most one crossing range per day, and it
        // is always the last one.
        match date.plus_days(-1) {
            Ok(previous) => self
                .hours_on(previous)
                .last()
                .is_some_and(|range| range.spills_over(minute)),
            Err(_) => false,
        }
    }

    /// Breakpoints inside a transition-free slice `[start, end)`.
    ///
    /// With no offset transition inside `[start, end)`, the zone's UTC offset is
    /// constant across the slice, so each local minute on `date` maps to a UTC
    /// minute by integer addition from that date's midnight. Narrowing `hours_on`
    /// to the slice's local time bounds avoids resolving ranges outside short
    /// slices (such as a schedule with many zone periods).
    fn constant_offset_breakpoints(
        &self,
        zone: &Zone,
        start: Instant,
        end: Instant,
    ) -> Vec<Instant> {
        let start_min = start.minutes();
        let end_min = end.minutes();
        let start_local = zone.zoned(start);
        let first = start_local.date();
        let lo_min = start_local.time().minute_of_day();
        let last_instant = Instant::from_minutes(end_min - 1)
            .expect("a slice ends at least one minute after it starts");
        let last_local = zone.zoned(last_instant);
        let last = last_local.date();
        let hi_min = last_local.time().minute_of_day();

        let mut instants = Vec::new();
        let mut date = first;
        let mut midnight = start_min - i64::from(lo_min);
        let push_if_inside = |out: &mut Vec<Instant>, minute: i64| {
            if minute > start_min && minute < end_min {
                out.push(
                    Instant::from_minutes(minute)
                        .expect("a breakpoint strictly inside the slice is representable"),
                );
            }
        };
        loop {
            let day_lo = if date == first { lo_min } else { 0 };
            let day_hi = if date == last {
                hi_min
            } else {
                TimeOfDay::MINUTES_PER_DAY
            };
            push_if_inside(&mut instants, midnight);

            let hours = self.hours_on(date);
            let from_idx = hours.partition_point(|range| range.linear_end() <= day_lo);
            for range in &hours[from_idx..] {
                let range_start = range.start_minutes();
                if range_start > day_hi {
                    break;
                }
                push_if_inside(&mut instants, midnight + i64::from(range_start));
                if !range.crosses_midnight() {
                    let range_end = range.end().minutes();
                    if range_end <= day_hi {
                        push_if_inside(&mut instants, midnight + i64::from(range_end));
                    }
                }
            }
            if let Ok(previous) = date.plus_days(-1)
                && let Some(range) = self.hours_on(previous).last()
                && range.crosses_midnight()
            {
                let spill_end = range.end().minutes();
                if spill_end > day_lo && spill_end <= day_hi {
                    push_if_inside(&mut instants, midnight + i64::from(spill_end));
                }
            }
            if date >= last {
                break;
            }
            date = date
                .plus_days(1)
                .expect("a date strictly before `last` has a representable next date");
            midnight += i64::from(TimeOfDay::MINUTES_PER_DAY);
        }
        instants
    }
}

impl BreakpointSource for WindowSpec {
    fn member(&self, instant: Instant, zone: &Zone) -> bool {
        let local = instant.zoned(zone);
        self.contains_local(local.date(), local.time().minute_of_day())
    }

    fn breakpoints(
        &self,
        zone: &Zone,
        start: Instant,
        end: Instant,
    ) -> Result<Vec<Instant>, TimeError> {
        let transitions = zone.transition_instants(start, end)?;
        if transitions.is_empty() {
            return Ok(self.constant_offset_breakpoints(zone, start, end));
        }
        let (first, last) = breakpoints::dates_covering(zone, start, end, &transitions);
        let mut instants = Vec::new();
        let mut date = first;
        loop {
            // The local date changes at this date's midnight, where the key that
            // governs it changes too.
            breakpoints::push_local(&mut instants, zone, date, TimeOfDay::MIDNIGHT)?;
            for range in self.hours_on(date) {
                breakpoints::push_local(&mut instants, zone, date, range.start())?;
                // A crossing range's end is a boundary on the next date, and is
                // collected there as the previous date's spill.
                if !range.crosses_midnight() {
                    let (end_date, end_time) = range.end_boundary(date)?;
                    breakpoints::push_local(&mut instants, zone, end_date, end_time)?;
                }
            }
            if let Ok(previous) = date.plus_days(-1)
                && let Some(range) = self.hours_on(previous).last()
                && range.crosses_midnight()
            {
                let (end_date, end_time) = range.end_boundary(previous)?;
                breakpoints::push_local(&mut instants, zone, end_date, end_time)?;
            }
            if date >= last {
                break;
            }
            date = date.plus_days(1)?;
        }
        instants.extend(transitions);
        Ok(instants)
    }
}

/// Sorts ranges by start and rejects any that overlap.
///
/// A crossing range is measured on past the day it starts, so two ranges that
/// only touch — one ends where the next starts — are allowed, while a genuine
/// overlap is the error.
fn non_overlapping(mut hours: Vec<WallRange>) -> Result<Vec<WallRange>, WindowError> {
    hours.sort_by_key(|range| range.start_minutes());
    let mut reached_end: Option<u16> = None;
    for range in &hours {
        if let Some(end) = reached_end
            && range.start_minutes() < end
        {
            return Err(WindowError::RangesOverlap { at: range.start() });
        }
        reached_end =
            Some(reached_end.map_or(range.linear_end(), |end| end.max(range.linear_end())));
    }
    Ok(hours)
}

#[cfg(test)]
mod tests {
    use super::{
        DateKey, DateRange, RuleZone, WallEnd, WallRange, Weekdays, WeeklyKey, WindowSpec,
    };
    use crate::time::{
        Date, Instant, LocalResolution, TimeOfDay, Weekday, WindowError, Zone, ZoneName,
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

    fn time(hour: u8, minute: u8) -> TimeOfDay {
        TimeOfDay::new(hour, minute).unwrap()
    }

    fn range(start: (u8, u8), end: (u8, u8)) -> WallRange {
        WallRange::new(time(start.0, start.1), WallEnd::Time(time(end.0, end.1))).unwrap()
    }

    fn until_midnight(start: (u8, u8)) -> WallRange {
        WallRange::new(time(start.0, start.1), WallEnd::Midnight).unwrap()
    }

    fn weekly(day: Weekday, hours: Vec<WallRange>) -> WeeklyKey {
        WeeklyKey::new(Weekdays::single(day), hours).unwrap()
    }

    fn spec(weekly: Vec<WeeklyKey>, dates: Vec<DateKey>) -> WindowSpec {
        WindowSpec::new(
            RuleZone::Named(ZoneName::new("UTC").unwrap()),
            weekly,
            dates,
        )
        .unwrap()
    }

    fn minutes(spans: &[crate::intervals::Span]) -> Vec<(i64, i64)> {
        spans
            .iter()
            .map(|span| (span.start(), span.end()))
            .collect()
    }

    /// The built span form, as minute pairs, over `from..until`.
    fn built(spec: &WindowSpec, from: Instant, until: Instant) -> Vec<(i64, i64)> {
        let zones = ZoneSchedule::fixed(Zone::utc());
        minutes(spec.intervals(&zones, from, until).unwrap().spans())
    }

    #[test]
    fn a_weekday_claimed_by_two_keys_is_rejected() {
        let monday = weekly(Weekday::Monday, vec![range((9, 0), (17, 0))]);
        let again = WeeklyKey::new(
            Weekdays::list([Weekday::Monday, Weekday::Tuesday]),
            vec![range((9, 0), (17, 0))],
        )
        .unwrap();
        let error = WindowSpec::new(
            RuleZone::Named(ZoneName::new("UTC").unwrap()),
            vec![monday, again],
            Vec::new(),
        )
        .unwrap_err();
        assert_eq!(
            error,
            WindowError::WeekdayRepeated {
                day: Weekday::Monday
            }
        );
        assert_eq!(error.code(), "window_weekday_repeated");
    }

    #[test]
    fn a_date_claimed_by_two_keys_is_rejected() {
        let date = Date::new(2027, 3, 10).unwrap();
        let first = DateKey::new(
            DateRange::new(date, Date::new(2027, 3, 12).unwrap()).unwrap(),
            Vec::new(),
        )
        .unwrap();
        let second = DateKey::new(
            DateRange::new(
                Date::new(2027, 3, 11).unwrap(),
                Date::new(2027, 3, 14).unwrap(),
            )
            .unwrap(),
            Vec::new(),
        )
        .unwrap();
        let error = WindowSpec::new(
            RuleZone::Named(ZoneName::new("UTC").unwrap()),
            Vec::new(),
            vec![first, second],
        )
        .unwrap_err();
        assert_eq!(
            error,
            WindowError::DateRepeated {
                date: Date::new(2027, 3, 11).unwrap()
            }
        );
        assert_eq!(error.code(), "window_date_repeated");
    }

    #[test]
    fn date_keys_are_matched_whatever_order_they_arrive_in() {
        // `dates` is a JSON object, so the keys arrive in whatever order the
        // caller wrote them. Both the duplicate check and the per-date lookup
        // below it assume the keys are sorted by start date, so an unsorted
        // input must not silently lose an override or miss a collision.
        let later = DateKey::new(
            DateRange::new(
                Date::new(2027, 3, 20).unwrap(),
                Date::new(2027, 3, 25).unwrap(),
            )
            .unwrap(),
            Vec::new(),
        )
        .unwrap();
        let earlier = DateKey::new(
            DateRange::new(
                Date::new(2027, 3, 10).unwrap(),
                Date::new(2027, 3, 21).unwrap(),
            )
            .unwrap(),
            Vec::new(),
        )
        .unwrap();
        // Supplied latest-first, the collision is still found, and the date it
        // reports is the first date both keys claim.
        let error = WindowSpec::new(
            RuleZone::Named(ZoneName::new("UTC").unwrap()),
            Vec::new(),
            vec![later, earlier],
        )
        .unwrap_err();
        assert_eq!(
            error,
            WindowError::DateRepeated {
                date: Date::new(2027, 3, 20).unwrap()
            }
        );

        // Disjoint keys supplied latest-first: each date still finds its own.
        let monday = DateKey::new(
            DateRange::single(Date::new(2027, 3, 15).unwrap()),
            vec![range((9, 0), (17, 0))],
        )
        .unwrap();
        let earlier_monday = DateKey::new(
            DateRange::single(Date::new(2027, 3, 8).unwrap()),
            vec![range((12, 0), (14, 0))],
        )
        .unwrap();
        let spec = spec(Vec::new(), vec![monday, earlier_monday]);
        let zones = ZoneSchedule::fixed(Zone::utc());
        assert!(spec.contains(utc(2027, 3, 8, 13, 0), &zones));
        assert!(!spec.contains(utc(2027, 3, 8, 10, 0), &zones));
        assert!(spec.contains(utc(2027, 3, 15, 13, 0), &zones));
        assert!(!spec.contains(utc(2027, 3, 15, 8, 0), &zones));
    }

    #[test]
    fn overlapping_ranges_within_one_key_are_rejected() {
        let error = WeeklyKey::new(
            Weekdays::single(Weekday::Monday),
            vec![range((9, 0), (17, 0)), range((16, 0), (18, 0))],
        )
        .unwrap_err();
        assert_eq!(error, WindowError::RangesOverlap { at: time(16, 0) });
        assert_eq!(error.code(), "window_ranges_overlap");

        // A crossing range is measured past the end of its day, so it overlaps a
        // later evening range but not an early-morning one.
        let crossing = range((22, 0), (2, 0));
        assert_eq!(
            WeeklyKey::new(
                Weekdays::single(Weekday::Monday),
                vec![crossing, range((23, 0), (1, 0))],
            )
            .unwrap_err()
            .code(),
            "window_ranges_overlap"
        );
        assert!(
            WeeklyKey::new(
                Weekdays::single(Weekday::Monday),
                vec![crossing, range((1, 0), (3, 0))],
            )
            .is_ok()
        );
        // A range that runs to midnight and one that starts at midnight are
        // adjacent, not overlapping.
        assert!(
            WeeklyKey::new(
                Weekdays::single(Weekday::Monday),
                vec![until_midnight((9, 0)), range((0, 0), (3, 0))],
            )
            .is_ok()
        );
    }

    #[test]
    fn a_range_that_starts_where_it_ends_is_rejected() {
        let error = WallRange::new(time(0, 0), WallEnd::Time(time(0, 0))).unwrap_err();
        assert_eq!(error, WindowError::RangeEmpty { start: time(0, 0) });
        assert_eq!(error.code(), "window_range_empty");
        // `24:00` is the end of the day, so a full day is expressible.
        assert!(WallRange::new(time(0, 0), WallEnd::Midnight).is_ok());
    }

    #[test]
    fn a_reversed_date_range_is_rejected() {
        let error = DateRange::new(
            Date::new(2027, 3, 12).unwrap(),
            Date::new(2027, 3, 10).unwrap(),
        )
        .unwrap_err();
        assert_eq!(
            error,
            WindowError::DateRangeReversed {
                start: Date::new(2027, 3, 12).unwrap(),
                end: Date::new(2027, 3, 10).unwrap(),
            }
        );
        assert_eq!(error.code(), "window_date_range_reversed");
        // A single-day range is not reversed.
        assert!(
            DateRange::single(Date::new(2027, 3, 10).unwrap())
                .contains(Date::new(2027, 3, 10).unwrap())
        );
    }

    /// The specification's read accessors expose the normalised form both the
    /// engine and the wire layer read: weekdays indexed Monday first, and date
    /// overrides sorted by start with their hours as supplied.
    #[test]
    fn the_read_accessors_expose_the_normalised_form() {
        let monday = weekly(Weekday::Monday, vec![range((9, 0), (17, 0))]);
        let later = DateKey::new(
            DateRange::single(Date::new(2027, 3, 20).unwrap()),
            Vec::new(),
        )
        .unwrap();
        let earlier = DateKey::new(
            DateRange::single(Date::new(2027, 3, 10).unwrap()),
            vec![range((12, 0), (14, 0))],
        )
        .unwrap();
        let spec = spec(vec![monday], vec![later, earlier]);

        assert_eq!(spec.weekly()[0].len(), 1, "Monday is index 0");
        assert!(spec.weekly()[1..].iter().all(Vec::is_empty));
        assert_eq!(
            spec.dates()
                .iter()
                .map(|key| key.dates().start())
                .collect::<Vec<_>>(),
            [
                Date::new(2027, 3, 10).unwrap(),
                Date::new(2027, 3, 20).unwrap()
            ],
            "date overrides are sorted by start date"
        );
        assert_eq!(spec.zone(), &RuleZone::Named(ZoneName::new("UTC").unwrap()));
    }

    #[test]
    fn a_day_in_no_key_is_closed() {
        // Monday 2027-03-08 and Tuesday 2027-03-09 in UTC.
        let spec = spec(
            vec![weekly(Weekday::Monday, vec![range((9, 0), (17, 0))])],
            Vec::new(),
        );
        assert!(spec.contains(utc(2027, 3, 8, 9, 0), &ZoneSchedule::fixed(Zone::utc()),));
        assert!(!spec.contains(utc(2027, 3, 9, 9, 0), &ZoneSchedule::fixed(Zone::utc()),));
        assert_eq!(
            built(&spec, utc(2027, 3, 9, 0, 0), utc(2027, 3, 10, 0, 0)),
            []
        );
    }

    #[test]
    fn a_midnight_crossing_range_runs_into_the_next_date() {
        // Friday 2027-03-12 opens at 22:00 and runs to 02:15 on Saturday.
        let spec = spec(
            vec![weekly(Weekday::Friday, vec![range((22, 0), (2, 15))])],
            Vec::new(),
        );
        assert_eq!(
            built(&spec, utc(2027, 3, 12, 0, 0), utc(2027, 3, 14, 0, 0)),
            [(
                utc(2027, 3, 12, 22, 0).minutes(),
                utc(2027, 3, 13, 2, 15).minutes()
            )]
        );
        let zones = ZoneSchedule::fixed(Zone::utc());
        assert!(spec.contains(utc(2027, 3, 13, 2, 14), &zones));
        assert!(!spec.contains(utc(2027, 3, 13, 2, 15), &zones));
    }

    #[test]
    fn a_spill_still_reaches_a_date_closed_by_an_override() {
        // The spill belongs to the previous date's key, and a date's key
        // governs its own spill. Closing Saturday therefore closes Saturday's
        // own hours without retracting Friday night's run into it.
        let saturday = DateKey::new(
            DateRange::single(Date::new(2027, 3, 13).unwrap()),
            Vec::new(),
        )
        .unwrap();
        let spec = spec(
            vec![weekly(Weekday::Friday, vec![range((22, 0), (2, 0))])],
            vec![saturday],
        );
        assert_eq!(
            built(&spec, utc(2027, 3, 13, 0, 0), utc(2027, 3, 14, 0, 0)),
            [(
                utc(2027, 3, 13, 0, 0).minutes(),
                utc(2027, 3, 13, 2, 0).minutes()
            )]
        );
    }

    #[test]
    fn a_spill_unions_with_the_next_dates_own_ranges() {
        let spec = spec(
            vec![
                weekly(Weekday::Friday, vec![range((22, 0), (2, 0))]),
                weekly(Weekday::Saturday, vec![range((1, 0), (3, 0))]),
            ],
            Vec::new(),
        );
        assert_eq!(
            built(&spec, utc(2027, 3, 13, 0, 0), utc(2027, 3, 14, 0, 0)),
            [(
                utc(2027, 3, 13, 0, 0).minutes(),
                utc(2027, 3, 13, 3, 0).minutes()
            )]
        );
    }

    #[test]
    fn an_override_on_the_spilling_date_governs_its_own_spill() {
        let friday = DateKey::new(
            DateRange::single(Date::new(2027, 3, 12).unwrap()),
            vec![range((23, 0), (4, 0))],
        )
        .unwrap();
        let spec = spec(
            vec![weekly(Weekday::Friday, vec![range((22, 0), (2, 0))])],
            vec![friday],
        );
        // The weekly Friday hours are gone, and the override's own crossing range
        // runs into Saturday.
        assert_eq!(
            built(&spec, utc(2027, 3, 12, 12, 0), utc(2027, 3, 13, 12, 0)),
            [(
                utc(2027, 3, 12, 23, 0).minutes(),
                utc(2027, 3, 13, 4, 0).minutes()
            )]
        );
    }

    #[test]
    fn an_empty_weekly_with_dates_is_open_on_those_dates_only() {
        let date = DateKey::new(
            DateRange::single(Date::new(2027, 3, 10).unwrap()),
            vec![range((10, 0), (16, 0))],
        )
        .unwrap();
        let spec = spec(Vec::new(), vec![date]);
        assert_eq!(
            built(&spec, utc(2027, 3, 9, 0, 0), utc(2027, 3, 12, 0, 0)),
            [(
                utc(2027, 3, 10, 10, 0).minutes(),
                utc(2027, 3, 10, 16, 0).minutes()
            )]
        );
    }

    #[test]
    fn a_date_override_replaces_the_weekday_hours() {
        let date = DateKey::new(
            DateRange::single(Date::new(2027, 3, 8).unwrap()), // a Monday
            vec![range((12, 0), (14, 0))],
        )
        .unwrap();
        let spec = spec(
            vec![weekly(Weekday::Monday, vec![range((9, 0), (17, 0))])],
            vec![date],
        );
        let zones = ZoneSchedule::fixed(Zone::utc());
        assert!(!spec.contains(utc(2027, 3, 8, 10, 0), &zones));
        assert!(spec.contains(utc(2027, 3, 8, 13, 0), &zones));
        // The next Monday is back on the weekly hours.
        assert!(spec.contains(utc(2027, 3, 15, 10, 0), &zones));
    }

    #[test]
    fn a_window_builds_over_the_requested_range_without_clipping_to_midnight() {
        let spec = spec(
            vec![weekly(Weekday::Monday, vec![range((9, 0), (17, 0))])],
            Vec::new(),
        );
        assert_eq!(
            built(&spec, utc(2027, 3, 8, 10, 30), utc(2027, 3, 8, 12, 30)),
            [(
                utc(2027, 3, 8, 10, 30).minutes(),
                utc(2027, 3, 8, 12, 30).minutes()
            )]
        );
    }

    #[test]
    fn the_interval_set_agrees_with_instant_membership_minute_by_minute() {
        // A week with weekday hours, a crossing Friday night, an override and a
        // closed date, read in UTC where the local reading is the UTC one.
        let spec = spec(
            vec![
                WeeklyKey::new(
                    Weekdays::list([Weekday::Monday, Weekday::Tuesday, Weekday::Wednesday]),
                    vec![range((9, 0), (17, 0)), range((18, 0), (20, 0))],
                )
                .unwrap(),
                weekly(Weekday::Friday, vec![range((22, 0), (2, 15))]),
                weekly(Weekday::Saturday, vec![until_midnight((8, 0))]),
            ],
            vec![
                DateKey::new(
                    DateRange::single(Date::new(2027, 3, 10).unwrap()),
                    vec![range((11, 0), (13, 0))],
                )
                .unwrap(),
                DateKey::new(
                    DateRange::single(Date::new(2027, 3, 12).unwrap()),
                    Vec::new(),
                )
                .unwrap(),
            ],
        );
        let zones = ZoneSchedule::fixed(Zone::utc());
        let from = utc(2027, 3, 8, 0, 0);
        let until = utc(2027, 3, 15, 0, 0);
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
