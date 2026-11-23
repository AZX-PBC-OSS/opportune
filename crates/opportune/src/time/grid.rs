//! Candidate starts on a local clock grid.
//!
//! A [`Grid`] turns a search window into candidate start instants. The rule is
//! local, per day: an instant is a candidate when its local minute `m` in the
//! grid's zone satisfies `(m − anchor).rem_euclid(step) == 0`. Local times inside
//! a spring-forward gap give no start, and local times inside a fall-back
//! overlap give two.
//!
//! ```
//! # #[cfg(feature = "bundled-tzdb")] {
//! use opportune::time::{
//!     Date, Duration, Grid, GridZone, LocalResolution, TimeOfDay, TimeZoneDatabase,
//! };
//!
//! let db = TimeZoneDatabase::bundled().unwrap();
//! let new_york = db.zone("America/New_York").unwrap();
//! let grid = Grid::new(
//!     Duration::from_minutes(30),
//!     TimeOfDay::MIDNIGHT,
//!     GridZone::Event,
//! )
//! .unwrap();
//!
//! // The whole local day on which New York springs forward at 02:00.
//! let LocalResolution::Unique(from) = new_york
//!     .resolve_local(Date::new(2027, 3, 14).unwrap(), TimeOfDay::MIDNIGHT)
//!     .unwrap()
//! else {
//!     panic!("local midnight exists on this date")
//! };
//! let LocalResolution::Unique(until) = new_york
//!     .resolve_local(Date::new(2027, 3, 15).unwrap(), TimeOfDay::MIDNIGHT)
//!     .unwrap()
//! else {
//!     panic!("local midnight exists on this date")
//! };
//!
//! let starts = grid.starts(&new_york, from, until).unwrap();
//! // 48 half-hour local starts, less the two inside the skipped hour.
//! assert_eq!(starts.len(), 46);
//! let skipped = TimeOfDay::new(2, 0).unwrap();
//! assert!(starts.iter().all(|start| start.zoned(&new_york).time() != skipped));
//! # }
//! ```

use crate::time::error::TimeError;
use crate::time::{
    Duration, Instant, LocalResolution, TimeOfDay, TimeZoneDatabase, Zone, ZoneName,
};

/// The zone a grid's local clock is read in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GridZone {
    /// The event's zone, so the grid follows the booking into another zone.
    Event,
    /// A named zone, whatever the event's zone is. A grid in the
    /// participant's zone is not expressible: participant zones are piecewise
    /// per instant, so a local clock grid in them has no single meaning.
    Named(ZoneName),
}

/// A local clock grid of candidate start instants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grid {
    step: Duration,
    anchor: TimeOfDay,
    zone: GridZone,
}

impl Grid {
    /// Creates a grid whose candidate local minutes recur every `step` minutes
    /// from `anchor`.
    ///
    /// ```
    /// # use opportune::time::{Duration, Grid, GridZone, TimeOfDay};
    /// let grid = Grid::new(
    ///     Duration::from_minutes(15),
    ///     TimeOfDay::new(9, 0).unwrap(),
    ///     GridZone::Event,
    /// ).unwrap();
    /// assert_eq!(grid.step().minutes(), 15);
    /// assert_eq!(grid.anchor(), TimeOfDay::new(9, 0).unwrap());
    /// ```
    ///
    /// # Errors
    /// Returns [`TimeError::GridStepOutOfRange`] for a step outside `1..=1440`
    /// minutes.
    pub fn new(step: Duration, anchor: TimeOfDay, zone: GridZone) -> Result<Self, TimeError> {
        if !(1..=i64::from(TimeOfDay::MINUTES_PER_DAY)).contains(&step.minutes()) {
            return Err(TimeError::GridStepOutOfRange {
                minutes: step.minutes(),
            });
        }
        Ok(Self { step, anchor, zone })
    }

    /// The step between candidate local minutes.
    #[must_use]
    pub const fn step(&self) -> Duration {
        self.step
    }

    /// The local minute each candidate day's grid is anchored on.
    #[must_use]
    pub const fn anchor(&self) -> TimeOfDay {
        self.anchor
    }

    /// The zone choice the grid was built with.
    #[must_use]
    pub fn zone_choice(&self) -> &GridZone {
        &self.zone
    }

    /// The zone the grid's local clock is read in, given the booking's event
    /// zone and the database that resolves a named one.
    ///
    /// # Errors
    /// Returns [`TimeError::ZoneUnknown`] when a named zone is not in `db`.
    pub fn zone(&self, db: &TimeZoneDatabase, event_zone: &Zone) -> Result<Zone, TimeError> {
        match &self.zone {
            GridZone::Event => Ok(event_zone.clone()),
            GridZone::Named(name) => db.zone(name.as_str()),
        }
    }

    /// The candidate starts on this grid inside `[from, until)`, in ascending
    /// instant order.
    ///
    /// A grid point whose local time maps outside the supported instant range
    /// is not a start: the search window is inside that range, so such a point
    /// cannot lie in it. Only the day of padding around each edge of the range
    /// produces them.
    ///
    /// ```
    /// # #[cfg(feature = "bundled-tzdb")] {
    /// # use opportune::time::{
    /// #     Date, Duration, Grid, GridZone, LocalResolution, TimeOfDay, TimeZoneDatabase,
    /// # };
    /// let db = TimeZoneDatabase::bundled().unwrap();
    /// let new_york = db.zone("America/New_York").unwrap();
    /// // A daily grid at 09:00 local.
    /// let grid = Grid::new(
    ///     Duration::from_minutes(1440),
    ///     TimeOfDay::new(9, 0).unwrap(),
    ///     GridZone::Event,
    /// ).unwrap();
    /// let LocalResolution::Unique(from) = new_york
    ///     .resolve_local(Date::new(2027, 3, 14).unwrap(), TimeOfDay::MIDNIGHT)
    ///     .unwrap()
    /// else {
    ///     panic!("local midnight exists on this date")
    /// };
    /// let LocalResolution::Unique(until) = new_york
    ///     .resolve_local(Date::new(2027, 3, 15).unwrap(), TimeOfDay::MIDNIGHT)
    ///     .unwrap()
    /// else {
    ///     panic!("local midnight exists on this date")
    /// };
    /// let starts = grid.starts(&new_york, from, until).unwrap();
    /// assert_eq!(starts.len(), 1);
    /// // 09:00 on the day after the change is 13:00Z, not 14:00Z.
    /// assert_eq!(starts[0].to_string(), "2027-03-14T13:00Z");
    /// # }
    /// ```
    ///
    /// # Errors
    /// Returns [`TimeError::DateOverflow`] when the window's local dates cannot
    /// be stepped, which only a window at the edge of the representable
    /// calendar can reach.
    pub fn starts(
        &self,
        zone: &Zone,
        from: Instant,
        until: Instant,
    ) -> Result<Vec<Instant>, TimeError> {
        self.starts_up_to(zone, from, until, None)
    }

    /// The candidate starts on this grid inside `[from, until)`, stopping once
    /// `limit` have been found.
    ///
    /// Used to answer "are there more than this many starts?" without
    /// materialising a request's whole search space. A truncated result is not
    /// sorted: its length is the only thing a caller may rely on.
    pub(crate) fn starts_up_to(
        &self,
        zone: &Zone,
        from: Instant,
        until: Instant,
        limit: Option<usize>,
    ) -> Result<Vec<Instant>, TimeError> {
        if from >= until {
            return Ok(Vec::new());
        }
        // One local day either side of the window is enough: a candidate's local
        // date can stray past the local dates at the window's edges only as far
        // as the zone's offset moves *backward* between them, and no zone in the
        // supported range moves backward by more than a day — pinned for the
        // bundled database by
        // `no_zone_moves_its_offset_backward_by_more_than_a_day`. A forward jump
        // only pushes local dates later, which the trailing day already covers.
        // Anything outside the window is dropped below.
        let first_date = zone.zoned(from).date().plus_days(-1)?;
        let last_date = zone.zoned(until).date().plus_days(1)?;
        let step = self.step.minutes();
        let first_minute = i64::from(self.anchor.minute_of_day()) % step;
        // Transitions within two days either side of the window decide whether
        // a local day has a constant offset; on a constant-offset day every
        // grid minute is a single addition from midnight.
        let probe_from = Instant::from_minutes(
            from.minutes()
                .saturating_sub(2 * 1_440)
                .max(Instant::MIN.minutes()),
        )?;
        let probe_until = Instant::from_minutes(
            until
                .minutes()
                .saturating_add(2 * 1_440)
                .min(Instant::MAX.minutes()),
        )?;
        let transitions = zone.transition_instants(probe_from, probe_until)?;

        let mut starts = Vec::new();
        let mut date = first_date;
        loop {
            let constant_midnight = match zone.resolve_local(date, TimeOfDay::MIDNIGHT) {
                Ok(LocalResolution::Unique(midnight))
                    if !transitions.iter().any(|transition| {
                        let minute = transition.minutes();
                        minute >= midnight.minutes() - 1_440
                            && minute <= midnight.minutes() + 2 * 1_440
                    }) =>
                {
                    Some(midnight.minutes())
                }
                _ => None,
            };
            let mut minute = first_minute;
            if let Some(midnight) = constant_midnight {
                while minute < i64::from(TimeOfDay::MINUTES_PER_DAY) {
                    if let Ok(instant) = Instant::from_minutes(midnight + minute) {
                        push_in_window(&mut starts, instant, from, until);
                    }
                    if limit.is_some_and(|limit| starts.len() >= limit) {
                        return Ok(starts);
                    }
                    minute += step;
                }
            } else {
                while minute < i64::from(TimeOfDay::MINUTES_PER_DAY) {
                    let minute_of_day = u16::try_from(minute)
                        .map_err(|_| TimeError::MinuteOfDayOutOfRange { minute })?;
                    let time = TimeOfDay::from_minute_of_day(minute_of_day)?;
                    // `local_instants` keeps each half of a repeated local time that
                    // the supported range can represent, so a fold at either end of
                    // the range keeps the half that is in the window instead of
                    // discarding both.
                    for instant in zone.local_instants(date, time)? {
                        push_in_window(&mut starts, instant, from, until);
                    }
                    if limit.is_some_and(|limit| starts.len() >= limit) {
                        return Ok(starts);
                    }
                    minute += step;
                }
            }
            if date >= last_date {
                break;
            }
            date = date.plus_days(1)?;
        }
        // A repeated local time gives two instants out of order against the
        // following local minute, so the order is restored here.
        starts.sort_unstable();
        Ok(starts)
    }
}

/// Keeps a candidate only when the search window covers it.
fn push_in_window(starts: &mut Vec<Instant>, instant: Instant, from: Instant, until: Instant) {
    if from <= instant && instant < until {
        starts.push(instant);
    }
}

#[cfg(test)]
mod tests {
    use super::{Grid, GridZone};
    use crate::time::{Date, Duration, Instant, LocalResolution, TimeError, TimeOfDay, Zone};

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

    fn midnight_grid() -> Grid {
        Grid::new(
            Duration::from_minutes(30),
            TimeOfDay::MIDNIGHT,
            GridZone::Event,
        )
        .unwrap()
    }

    #[test]
    fn steps_outside_one_day_are_rejected() {
        let anchor = TimeOfDay::MIDNIGHT;
        for minutes in [0, 1441] {
            assert_eq!(
                Grid::new(Duration::from_minutes(minutes), anchor, GridZone::Event),
                Err(TimeError::GridStepOutOfRange {
                    minutes: i64::from(minutes)
                })
            );
        }
        assert_eq!(
            TimeError::GridStepOutOfRange { minutes: 0 }.code(),
            "grid_step_out_of_range"
        );
        assert!(Grid::new(Duration::from_minutes(1440), anchor, GridZone::Event).is_ok());
        assert!(Grid::new(Duration::from_minutes(1), anchor, GridZone::Event).is_ok());
    }

    #[test]
    fn a_utc_day_has_1440_over_step_starts() {
        let utc = Zone::utc();
        let starts = midnight_grid()
            .starts(
                &utc,
                utc_instant(2027, 3, 9, 0, 0),
                utc_instant(2027, 3, 10, 0, 0),
            )
            .unwrap();
        assert_eq!(starts.len(), 48);
        assert_eq!(starts[0].to_string(), "2027-03-09T00:00Z");
        assert_eq!(starts[1].to_string(), "2027-03-09T00:30Z");
        assert_eq!(starts[47].to_string(), "2027-03-09T23:30Z");
        // The window is half-open: the start at its end is not a candidate.
        assert!(
            starts
                .iter()
                .all(|start| *start < utc_instant(2027, 3, 10, 0, 0))
        );
    }

    #[test]
    fn steps_that_do_not_divide_the_day_use_the_euclidean_rule() {
        let utc = Zone::utc();
        let from = utc_instant(2027, 3, 9, 0, 0);
        let until = utc_instant(2027, 3, 10, 0, 0);
        for (step, expected) in [(7, 205), (25, 58), (1440, 1)] {
            let grid = Grid::new(
                Duration::from_minutes(step),
                TimeOfDay::new(3, 0).unwrap(),
                GridZone::Event,
            )
            .unwrap();
            let starts = grid.starts(&utc, from, until).unwrap();
            assert_eq!(
                starts.len(),
                expected,
                "step {step} minutes from a 03:00 anchor"
            );
            for start in &starts {
                let minute = i64::from(start.zoned(&utc).time().minute_of_day());
                assert_eq!(
                    (minute - 180).rem_euclid(i64::from(step)),
                    0,
                    "{start} is on the grid"
                );
            }
            // The anchor itself is a candidate; for a step that does not divide
            // the day it is not the day's first candidate.
            assert!(
                starts
                    .iter()
                    .any(|start| start.zoned(&utc).time() == TimeOfDay::new(3, 0).unwrap()),
                "step {step} keeps the 03:00 anchor"
            );
        }
    }

    #[test]
    fn an_empty_or_reversed_window_gives_no_starts() {
        let utc = Zone::utc();
        let grid = midnight_grid();
        assert!(
            grid.starts(&utc, Instant::MIN, Instant::MIN)
                .unwrap()
                .is_empty()
        );
        assert!(
            grid.starts(&utc, Instant::MAX, Instant::MIN)
                .unwrap()
                .is_empty()
        );
    }

    #[cfg(feature = "bundled-tzdb")]
    #[test]
    fn a_grid_zone_names_its_clock() {
        use crate::time::{TimeZoneDatabase, ZoneName};

        let db = TimeZoneDatabase::bundled().unwrap();
        let event_zone = Zone::utc();
        let named = Grid::new(
            Duration::from_minutes(60),
            TimeOfDay::MIDNIGHT,
            GridZone::Named(ZoneName::new("Asia/Tokyo").unwrap()),
        )
        .unwrap();
        assert_eq!(named.zone(&db, &event_zone).unwrap().name(), "Asia/Tokyo");
        let unknown = Grid::new(
            Duration::from_minutes(60),
            TimeOfDay::MIDNIGHT,
            GridZone::Named(ZoneName::new("Mars/Olympus_Mons").unwrap()),
        )
        .unwrap();
        assert_eq!(
            unknown.zone(&db, &event_zone).unwrap_err().code(),
            "zone_unknown"
        );

        let event = midnight_grid();
        assert_eq!(event.zone(&db, &event_zone).unwrap(), event_zone);
    }
}
