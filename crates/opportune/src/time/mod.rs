//! Civil time: UTC instants, zones, the time zone database, zone periods,
//! candidate grids, working calendars, `lead_time` horizons and the wall-clock
//! window and blackout specifications.
//!
//! This is the only module that touches civil time and time zone data; every
//! other module works in UTC minutes.
//!
//! * [`Instant`] is a UTC instant at minute resolution, restricted to the
//!   supported range `1975-01-01T00:00Z..=2100-01-01T00:00Z` and rounded the way
//!   the request demands: `now` up, block endpoints outward. The constructors that
//!   take a fractional second (`from_unix_seconds_nanos`, `floor_to_minute_nanos`,
//!   `ceil_to_minute_nanos`) are what a binding uses on a timestamp that carries
//!   one.
//! * [`Duration`] is an exact number of minutes — never days, because a day is
//!   23 to 25 hours in a named zone.
//! * [`TimeZoneDatabase`] is the bundled database or a directory the caller
//!   names, never the host's; a [`ZoneName`] is what a request carries and
//!   [`Zone`] is what the database resolves it to, converting between instants
//!   and local times. [`Zone::day_start`] resolves a civil date to the instant it
//!   begins at, which an all-day event becomes, and
//!   [`Zone::resolve_local_compatible`] reads a provider's local timestamp with the
//!   compatible disambiguation a repeated or skipped hour needs.
//! * [`ZoneSchedule`] makes a participant's zone a function of the instant, so a
//!   trip switches zones at the instants given.
//! * [`Grid`] generates candidate start instants from a local clock grid.
//! * [`WorkingCalendar`] and [`Horizon`] evaluate `lead_time` maximums, resolved
//!   into a [`StartBound`] once per request.
//! * [`WindowSpec`] and [`BlackoutSpec`] are validated wall-clock specifications:
//!   each answers instant membership directly and builds the exact half-open UTC
//!   interval set over any requested range with the segment-start breakpoint
//!   algorithm.
//!
//! ```
//! use opportune::time::{Date, Instant, LocalResolution, TimeOfDay, Zone};
//!
//! let utc = Zone::utc();
//! let LocalResolution::Unique(instant) = utc
//!     .resolve_local(Date::new(2027, 3, 9).unwrap(), TimeOfDay::new(15, 0).unwrap())
//!     .unwrap()
//! else {
//!     panic!("UTC has no daylight saving transitions")
//! };
//! assert_eq!(instant.to_string(), "2027-03-09T15:00Z");
//! ```

mod blackout;
mod breakpoints;
mod calendar;
mod civil;
mod error;
mod grid;
mod instant;
mod schedule;
mod tzdb;
mod window;

pub(crate) use breakpoints::build_minutes;

#[cfg(test)]
mod blackout_tests;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod window_tests;

pub use blackout::{BlackoutSpan, BlackoutSpec};
pub use calendar::{Horizon, StartBound, WorkingCalendar};
pub use civil::{Date, LocalDateTime, TimeOfDay, Weekday, Weekdays, WorkingDays};
pub use error::{BlackoutError, TimeError, WindowError};
pub use grid::{Grid, GridZone};
pub use instant::{Duration, Instant};
pub use schedule::{ZonePeriod, ZoneSchedule};
pub use tzdb::{LocalResolution, TimeZoneDatabase, UtcOffset, Zone, ZoneName, ZonedInstant};
pub use window::{DateKey, DateRange, RuleZone, WallEnd, WallRange, WeeklyKey, WindowSpec};
