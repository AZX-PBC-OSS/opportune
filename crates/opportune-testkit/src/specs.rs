//! Point-by-point reference models for the engine's wall-clock `window` and
//! `blackout` specifications.
//!
//! A plan is the plain description a generator produces: weekday masks, minute
//! ranges, date overrides, zone legs and spans. This module decides membership
//! straight from the specification text — an instant's local date and time,
//! then the documented rules — without using the engine's `WindowSpec`,
//! `BlackoutSpec` or breakpoint builder, so a shared misreading of a rule shows
//! up as a disagreement with the engine rather than agreeing quietly.

use opportune::time::{Date, Instant, TimeOfDay, TimeZoneDatabase, Weekday, Zone};

/// Which zone a rule is read in, matching the engine's `RuleZone` choice without
/// sharing its type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanZone {
    /// The participant's zone at each instant.
    Participant,
    /// The event's zone.
    Event,
    /// A named zone.
    Named,
}

/// A participant's zone as a plan describes it: a home zone name and disjoint
/// named legs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZonePlan {
    /// The home zone's IANA name.
    pub home: String,
    /// Legs in other zones: name, first instant, one past the last instant.
    pub periods: Vec<(String, Instant, Instant)>,
}

/// A [`ZonePlan`] with its zone names resolved, for the oracle's own lookups.
#[derive(Clone, Debug)]
pub struct ResolvedZones {
    home: Zone,
    periods: Vec<(Zone, Instant, Instant)>,
}

impl ZonePlan {
    /// Resolves every name through `db`.
    ///
    /// # Panics
    /// Panics when a name is not in the database, which a generator drawing
    /// from the zone matrix never does.
    #[must_use]
    pub fn resolve(&self, db: &TimeZoneDatabase) -> ResolvedZones {
        let lookup = |name: &str| {
            db.zone(name)
                .unwrap_or_else(|err| panic!("`{name}` is in the bundled database: {err}"))
        };
        ResolvedZones {
            home: lookup(&self.home),
            periods: self
                .periods
                .iter()
                .map(|(name, from, until)| (lookup(name), *from, *until))
                .collect(),
        }
    }
}

impl ResolvedZones {
    /// The zone in force at `instant`: the covering leg, or home.
    ///
    /// A linear scan, deliberately: the engine binary-searches, so the two
    /// implementations of the half-open rule stay independent.
    #[must_use]
    pub fn zone_at(&self, instant: Instant) -> &Zone {
        match self
            .periods
            .iter()
            .find(|(_, from, until)| *from <= instant && instant < *until)
        {
            Some((zone, _, _)) => zone,
            None => &self.home,
        }
    }
}

/// A date override in plan form: inclusive start and end dates, and the ranges
/// those dates have, each `(start, end)` in minutes from the start of the day.
pub type DateRangePlan = (Date, Date, Vec<(u16, u16)>);

/// A generated window: the plain form of a `weekly`/`dates` specification.
#[derive(Clone, Debug)]
pub struct WindowCase {
    /// The zone the window is read in.
    pub zone: PlanZone,
    /// The participant's zone plan, used by [`PlanZone::Participant`].
    pub zones: ZonePlan,
    /// The event zone's IANA name.
    pub event: String,
    /// The named zone's IANA name, used by [`PlanZone::Named`].
    pub named: String,
    /// Weekly keys: a Monday-first day mask and the ranges those days share,
    /// as `(start, end)` minutes from the start of the day, `end` at most 1440.
    pub weekly: Vec<(u8, Vec<(u16, u16)>)>,
    /// Date overrides, inclusive, disjoint: the dates and their ranges.
    pub dates: Vec<DateRangePlan>,
    /// The first instant the interval set is built over.
    pub from: Instant,
    /// One past the last instant.
    pub until: Instant,
}

impl WindowCase {
    /// Whether `instant` is inside, applying the window membership rule to the
    /// instant's local reading.
    #[must_use]
    pub fn contains(
        &self,
        instant: Instant,
        zones: &ResolvedZones,
        event: &Zone,
        named: &Zone,
    ) -> bool {
        let zone = match self.zone {
            PlanZone::Participant => zones.zone_at(instant),
            PlanZone::Event => event,
            PlanZone::Named => named,
        };
        let local = instant.zoned(zone);
        self.contains_local(local.date(), local.time().minute_of_day())
    }

    fn contains_local(&self, date: Date, minute: u16) -> bool {
        if self
            .hours(date)
            .iter()
            .any(|&(start, end)| covers(start, end, minute))
        {
            return true;
        }
        match date.plus_days(-1) {
            Ok(previous) => self
                .hours(previous)
                .iter()
                .any(|&(start, end)| end < start && minute < end),
            Err(_) => false,
        }
    }

    /// The ranges of the key that governs `date`: its override if it has one,
    /// otherwise its weekday's.
    fn hours(&self, date: Date) -> &[(u16, u16)] {
        for (start, end, hours) in &self.dates {
            if *start <= date && date <= *end {
                return hours;
            }
        }
        let bit = 1u8 << weekday_index(date.weekday());
        self.weekly
            .iter()
            .find(|(mask, _)| mask & bit != 0)
            .map_or(&[], |(_, hours)| hours.as_slice())
    }
}

/// Whether a range covers `minute` of the day it starts on.
fn covers(start: u16, end: u16, minute: u16) -> bool {
    if end < start {
        minute >= start
    } else {
        start <= minute && minute < end
    }
}

/// Monday-first index of a weekday, written out rather than shared with the
/// engine so the two derivations stay independent.
fn weekday_index(day: Weekday) -> u8 {
    match day {
        Weekday::Monday => 0,
        Weekday::Tuesday => 1,
        Weekday::Wednesday => 2,
        Weekday::Thursday => 3,
        Weekday::Friday => 4,
        Weekday::Saturday => 5,
        Weekday::Sunday => 6,
    }
}

/// A generated blackout: the plain form of a list of local date-time spans.
#[derive(Clone, Debug)]
pub struct BlackoutCase {
    /// The zone the spans are read in.
    pub zone: PlanZone,
    /// The participant's zone plan, used by [`PlanZone::Participant`].
    pub zones: ZonePlan,
    /// The event zone's IANA name.
    pub event: String,
    /// The named zone's IANA name, used by [`PlanZone::Named`].
    pub named: String,
    /// Half-open local date-time spans.
    pub spans: Vec<((Date, TimeOfDay), (Date, TimeOfDay))>,
    /// The first instant the interval set is built over.
    pub from: Instant,
    /// One past the last instant.
    pub until: Instant,
}

impl BlackoutCase {
    /// Whether `instant` is blacked out, applying the blackout rule to the
    /// instant's local reading.
    #[must_use]
    pub fn contains(
        &self,
        instant: Instant,
        zones: &ResolvedZones,
        event: &Zone,
        named: &Zone,
    ) -> bool {
        let zone = match self.zone {
            PlanZone::Participant => zones.zone_at(instant),
            PlanZone::Event => event,
            PlanZone::Named => named,
        };
        let local = instant.zoned(zone);
        let at = (local.date(), local.time());
        self.spans
            .iter()
            .any(|(start, end)| *start <= at && at < *end)
    }
}
