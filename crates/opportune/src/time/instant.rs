//! UTC instants at minute resolution, and exact minute durations.
//!
//! An [`Instant`] is a whole minute counted from the Unix epoch, restricted to
//! the range a request may name: `1975-01-01T00:00Z` through
//! `2100-01-01T00:00Z`. That range is what lets the interval arithmetic stay in
//! whole minutes — every offset in the time zone database ended its sub-minute
//! component in 1972.
//!
//! Request data arrives with second precision, so the constructors that accept
//! Unix seconds either reject a value that is not minute-aligned or round it as
//! required by the wire format: `now` rounds up, and block endpoints round
//! outward.
//!
//! ```
//! use opportune::time::{Duration, Instant};
//!
//! // `now` rounds up to the next minute: 15:00:30 and 15:00:59 both become
//! // 15:01, while an already aligned 15:00 stays where it is.
//! let now = Instant::ceil_to_minute(1_804_604_430).unwrap();
//! assert_eq!(now, Instant::ceil_to_minute(1_804_604_460).unwrap());
//! assert_eq!(now.to_string(), "2027-03-09T15:01Z");
//! assert_eq!(
//!     Instant::ceil_to_minute(1_804_604_400).unwrap().to_string(),
//!     "2027-03-09T15:00Z"
//! );
//!
//! // A duration is exact minutes, so it has no day component.
//! let notice = Duration::from_hours_minutes(4, 0);
//! assert_eq!(notice.minutes(), 240);
//! assert!(Duration::from_parts(1, 0, 0).is_err());
//! ```

use core::fmt;

use jiff::Timestamp;

use crate::time::civil::{Date, TimeOfDay};
use crate::time::error::TimeError;

/// The first Unix second a request may name: `1975-01-01T00:00Z`.
const MIN_UNIX_SECONDS: i64 = 157_766_400;
/// The last Unix second a request may name: `2100-01-01T00:00Z`.
const MAX_UNIX_SECONDS: i64 = 4_102_444_800;
/// `MIN_UNIX_SECONDS`, in whole minutes.
const MIN_UNIX_MINUTES: i64 = MIN_UNIX_SECONDS / 60;
/// `MAX_UNIX_SECONDS`, in whole minutes.
const MAX_UNIX_MINUTES: i64 = MAX_UNIX_SECONDS / 60;
/// The nanoseconds inside one second.
const NANOS_PER_SECOND: u32 = 1_000_000_000;

/// A UTC instant, aligned to a whole minute.
///
/// Instants are derived values as well as inputs: `now`, block spans, zone
/// period boundaries, occurrences and candidate starts are all instants. Any
/// of them outside `1975-01-01T00:00Z..=2100-01-01T00:00Z` is rejected at
/// construction, so no later arithmetic has to ask whether an instant is
/// representable.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Instant {
    /// Whole minutes since the Unix epoch. Always inside
    /// `MIN_UNIX_MINUTES..=MAX_UNIX_MINUTES`.
    minutes: i64,
}

impl Instant {
    /// The earliest supported instant, `1975-01-01T00:00Z`.
    pub const MIN: Self = Self {
        minutes: MIN_UNIX_MINUTES,
    };

    /// The latest supported instant, `2100-01-01T00:00Z`.
    pub const MAX: Self = Self {
        minutes: MAX_UNIX_MINUTES,
    };

    /// Creates an instant from whole minutes since the Unix epoch.
    ///
    /// ```
    /// # use opportune::time::Instant;
    /// let instant = Instant::from_minutes(Instant::MIN.minutes()).unwrap();
    /// assert_eq!(instant, Instant::MIN);
    /// assert!(Instant::from_minutes(0).is_err()); // 1970 is before the range
    /// ```
    ///
    /// # Errors
    /// Returns [`TimeError::InstantOutOfRange`] when the value is outside
    /// `1975-01-01T00:00Z..=2100-01-01T00:00Z`.
    pub fn from_minutes(minutes: i64) -> Result<Self, TimeError> {
        if !(MIN_UNIX_MINUTES..=MAX_UNIX_MINUTES).contains(&minutes) {
            // The rejected value is reported in seconds to match the
            // constructors that take seconds; it is out of range either way.
            return Err(TimeError::InstantOutOfRange {
                seconds: minutes.saturating_mul(60),
            });
        }
        Ok(Self { minutes })
    }

    /// Creates an instant from Unix seconds, rejecting a value that is not
    /// aligned to a whole minute.
    ///
    /// ```
    /// # use opportune::time::Instant;
    /// assert!(Instant::from_unix_seconds(1_804_604_400).is_ok());
    /// assert!(Instant::from_unix_seconds(1_804_604_430).is_err());
    /// ```
    ///
    /// # Errors
    /// Returns [`TimeError::InstantNotAligned`] for a value that has a seconds
    /// component, and [`TimeError::InstantOutOfRange`] outside the supported
    /// range.
    pub fn from_unix_seconds(seconds: i64) -> Result<Self, TimeError> {
        check_range(seconds)?;
        if seconds % 60 != 0 {
            return Err(TimeError::InstantNotAligned { seconds });
        }
        Ok(Self {
            minutes: seconds / 60,
        })
    }

    /// Rounds Unix seconds down to the whole minute they fall in — the
    /// direction a block's start rounds.
    ///
    /// ```
    /// # use opportune::time::Instant;
    /// let start = Instant::floor_to_minute(1_804_604_430).unwrap();
    /// assert_eq!(start.to_string(), "2027-03-09T15:00Z");
    /// ```
    ///
    /// # Errors
    /// Returns [`TimeError::InstantOutOfRange`] outside the supported range.
    pub fn floor_to_minute(seconds: i64) -> Result<Self, TimeError> {
        check_range(seconds)?;
        Ok(Self {
            minutes: seconds.div_euclid(60),
        })
    }

    /// Rounds Unix seconds up to the whole minute the instant is inside or
    /// begins — the direction `now` and a block's end round.
    ///
    /// ```
    /// # use opportune::time::Instant;
    /// // Already aligned instants are unchanged.
    /// let now = Instant::ceil_to_minute(1_804_604_400).unwrap();
    /// assert_eq!(now.to_string(), "2027-03-09T15:00Z");
    /// // A stray second moves to the next minute.
    /// assert_eq!(
    ///     Instant::ceil_to_minute(1_804_604_401).unwrap(),
    ///     Instant::ceil_to_minute(1_804_604_460).unwrap()
    /// );
    /// ```
    ///
    /// # Errors
    /// Returns [`TimeError::InstantOutOfRange`] outside the supported range.
    pub fn ceil_to_minute(seconds: i64) -> Result<Self, TimeError> {
        check_range(seconds)?;
        Ok(Self {
            minutes: seconds.div_euclid(60) + i64::from(seconds.rem_euclid(60) != 0),
        })
    }

    /// Creates an instant from Unix seconds and the nanoseconds inside that
    /// second, rejecting a value that is not aligned to a whole minute.
    ///
    /// A timestamp a binding decodes can carry a fractional second — `toISOString`
    /// and `isoformat` produce one, and Graph reports seven digits — so an
    /// instant built from one has to say what it does with the fraction. Use
    /// [`Instant::floor_to_minute_nanos`] or [`Instant::ceil_to_minute_nanos`]
    /// to round it, and this constructor when the value must already be aligned.
    ///
    /// ```
    /// # use opportune::time::{Instant, TimeError};
    /// assert!(Instant::from_unix_seconds_nanos(1_804_604_400, 0).is_ok());
    /// assert_eq!(
    ///     Instant::from_unix_seconds_nanos(1_804_604_400, 250),
    ///     Err(TimeError::InstantNotAligned { seconds: 1_804_604_400 })
    /// );
    /// ```
    ///
    /// # Errors
    /// Returns [`TimeError::NanosOutOfRange`] for a nanosecond component of a
    /// second or more, [`TimeError::InstantNotAligned`] for a stray second or
    /// nanosecond, and [`TimeError::InstantOutOfRange`] outside the supported
    /// range.
    pub fn from_unix_seconds_nanos(seconds: i64, nanos: u32) -> Result<Self, TimeError> {
        check_nanos(nanos)?;
        check_range(seconds)?;
        if seconds.rem_euclid(60) != 0 || nanos != 0 {
            return Err(TimeError::InstantNotAligned { seconds });
        }
        Ok(Self {
            minutes: seconds.div_euclid(60),
        })
    }

    /// Rounds Unix seconds and nanoseconds down to the whole minute they fall in
    /// — the direction a block's start rounds.
    ///
    /// ```
    /// # use opportune::time::Instant;
    /// let start = Instant::floor_to_minute_nanos(1_804_604_430, 500_000_000).unwrap();
    /// assert_eq!(start.to_string(), "2027-03-09T15:00Z");
    /// ```
    ///
    /// # Errors
    /// Returns [`TimeError::NanosOutOfRange`] for a nanosecond component of a
    /// second or more, and [`TimeError::InstantOutOfRange`] outside the supported
    /// range.
    pub fn floor_to_minute_nanos(seconds: i64, nanos: u32) -> Result<Self, TimeError> {
        check_nanos(nanos)?;
        check_range(seconds)?;
        Ok(Self {
            minutes: seconds.div_euclid(60),
        })
    }

    /// Rounds Unix seconds and nanoseconds up to the whole minute the instant is
    /// inside or begins — the direction a block's end rounds, and the one `now`
    /// takes.
    ///
    /// ```
    /// # use opportune::time::Instant;
    /// // A fraction of a second still carries the end into the next minute.
    /// let end = Instant::ceil_to_minute_nanos(1_804_604_400, 1).unwrap();
    /// assert_eq!(end.to_string(), "2027-03-09T15:01Z");
    /// assert_eq!(
    ///     Instant::ceil_to_minute_nanos(1_804_604_400, 0).unwrap().to_string(),
    ///     "2027-03-09T15:00Z"
    /// );
    /// ```
    ///
    /// # Errors
    /// Returns [`TimeError::NanosOutOfRange`] for a nanosecond component of a
    /// second or more, and [`TimeError::InstantOutOfRange`] when rounding up would
    /// leave the supported range — the last second before `2100-01-01T00:00Z`
    /// carries any fraction to a minute no instant names.
    pub fn ceil_to_minute_nanos(seconds: i64, nanos: u32) -> Result<Self, TimeError> {
        check_nanos(nanos)?;
        check_range(seconds)?;
        let stray = seconds.rem_euclid(60) != 0 || nanos != 0;
        let minutes = seconds.div_euclid(60) + i64::from(stray);
        // Rounding outward cannot land outside the range every instant is
        // trusted to be in, so a fraction inside the last supported second is
        // rejected rather than carried as an unrepresentable instant.
        if !(MIN_UNIX_MINUTES..=MAX_UNIX_MINUTES).contains(&minutes) {
            return Err(TimeError::InstantOutOfRange {
                seconds: minutes.saturating_mul(60),
            });
        }
        Ok(Self { minutes })
    }

    /// Creates the UTC instant a civil date and time of day name.
    ///
    /// UTC has no daylight-saving transitions, so a civil date-time inside the
    /// supported range always names exactly one instant: there is no hour that
    /// occurs twice and none that is skipped, which is why this needs no
    /// disambiguation rule.
    ///
    /// ```
    /// # use opportune::time::{Date, Instant, TimeOfDay, TimeError};
    /// let instant = Instant::from_utc(
    ///     Date::new(2027, 3, 9).unwrap(),
    ///     TimeOfDay::new(15, 0).unwrap(),
    /// )
    /// .unwrap();
    /// assert_eq!(instant.to_string(), "2027-03-09T15:00Z");
    ///
    /// // A civil date-time before the supported range names no instant.
    /// assert_eq!(
    ///     Instant::from_utc(Date::new(1974, 12, 31).unwrap(), TimeOfDay::MIDNIGHT),
    ///     Err(TimeError::CivilToInstant {
    ///         date: Date::new(1974, 12, 31).unwrap(),
    ///         time: TimeOfDay::MIDNIGHT,
    ///     })
    /// );
    /// ```
    ///
    /// # Errors
    /// Returns [`TimeError::CivilToInstant`] when the date and time fall outside
    /// `1975-01-01T00:00Z..=2100-01-01T00:00Z`.
    pub fn from_utc(date: Date, time: TimeOfDay) -> Result<Self, TimeError> {
        let unrepresentable = || TimeError::CivilToInstant { date, time };
        let seconds = jiff::tz::Offset::UTC
            .to_timestamp(time.on(date))
            .map(Timestamp::as_second)
            .map_err(|_| unrepresentable())?;
        Self::from_unix_seconds(seconds).map_err(|err| match err {
            // The civil time exists; the instant it names is outside the range
            // every instant is trusted to be in, so the mapping is what failed.
            TimeError::InstantOutOfRange { .. } => unrepresentable(),
            other => other,
        })
    }

    /// Whole minutes since the Unix epoch.
    #[must_use]
    pub const fn minutes(&self) -> i64 {
        self.minutes
    }

    /// The instant `d` later, or `None` when that would leave the supported
    /// range.
    ///
    /// ```
    /// # use opportune::time::{Duration, Instant};
    /// let later = Instant::MIN.checked_add(Duration::from_hours(1)).unwrap();
    /// assert_eq!(later.minutes() - Instant::MIN.minutes(), 60);
    /// assert_eq!(Instant::MAX.checked_add(Duration::from_minutes(1)), None);
    /// ```
    #[must_use]
    pub fn checked_add(self, d: Duration) -> Option<Self> {
        self.minutes
            .checked_add(d.minutes())
            .and_then(|minutes| Self::from_minutes(minutes).ok())
    }

    /// The instant `d` earlier, or `None` when that would leave the supported
    /// range.
    ///
    /// ```
    /// # use opportune::time::{Duration, Instant};
    /// let earlier = Instant::MAX.checked_sub(Duration::from_hours(1)).unwrap();
    /// assert_eq!(Instant::MAX.minutes() - earlier.minutes(), 60);
    /// assert_eq!(Instant::MIN.checked_sub(Duration::from_minutes(1)), None);
    /// ```
    #[must_use]
    pub fn checked_sub(self, d: Duration) -> Option<Self> {
        self.minutes
            .checked_sub(d.minutes())
            .and_then(|minutes| Self::from_minutes(minutes).ok())
    }

    /// Whole seconds since the Unix epoch.
    #[must_use]
    pub const fn unix_seconds(&self) -> i64 {
        self.minutes * 60
    }

    /// The instant as a `jiff` timestamp.
    ///
    /// Every supported instant is far inside `jiff`'s timestamp range, which
    /// [`Instant::from_minutes`] has already enforced.
    pub(crate) fn as_jiff(self) -> Timestamp {
        Timestamp::from_second(self.unix_seconds())
            .expect("a supported instant lies inside jiff's timestamp range")
    }
}

/// Checks that a nanosecond component names a point inside its second.
fn check_nanos(nanos: u32) -> Result<(), TimeError> {
    if nanos < NANOS_PER_SECOND {
        Ok(())
    } else {
        Err(TimeError::NanosOutOfRange { nanos })
    }
}

/// Checks the supported range for a Unix second value.
fn check_range(seconds: i64) -> Result<(), TimeError> {
    if (MIN_UNIX_SECONDS..=MAX_UNIX_SECONDS).contains(&seconds) {
        Ok(())
    } else {
        Err(TimeError::InstantOutOfRange { seconds })
    }
}

impl fmt::Debug for Instant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for Instant {
    /// Formats the instant as `YYYY-MM-DDTHH:MMZ` in UTC.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ts = self.as_jiff();
        let dt = jiff::tz::Offset::UTC.to_datetime(ts);
        write!(
            f,
            "{:04}-{:02}-{:02}T{:02}:{:02}Z",
            dt.year(),
            dt.month(),
            dt.day(),
            dt.hour(),
            dt.minute(),
        )
    }
}

/// An exact length of time in whole minutes.
///
/// Durations are not calendar quantities: a day is 23 to 25 hours in a named
/// zone, so a duration never contains one. Callers building one from an
/// already-decoded `days`/`hours`/`minutes` triple use [`Duration::from_parts`],
/// which rejects the day component rather than guessing how long it is.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Duration {
    minutes: i64,
}

impl Duration {
    /// No time at all.
    pub const ZERO: Self = Self { minutes: 0 };

    /// Creates a duration from whole minutes.
    #[must_use]
    pub const fn from_minutes(minutes: u32) -> Self {
        Self {
            minutes: minutes as i64,
        }
    }

    /// Creates a duration from hours and minutes, as in `1h30m`.
    ///
    /// ```
    /// # use opportune::time::Duration;
    /// assert_eq!(Duration::from_hours_minutes(1, 30).minutes(), 90);
    /// assert_eq!(Duration::from_hours_minutes(0, 0), Duration::ZERO);
    /// ```
    #[must_use]
    pub const fn from_hours_minutes(hours: u32, minutes: u32) -> Self {
        Self {
            minutes: hours as i64 * 60 + minutes as i64,
        }
    }

    /// Creates a duration from whole hours.
    ///
    /// ```
    /// # use opportune::time::Duration;
    /// assert_eq!(Duration::from_hours(4).minutes(), 240);
    /// ```
    #[must_use]
    pub const fn from_hours(hours: u32) -> Self {
        Self {
            minutes: hours as i64 * 60,
        }
    }

    /// Creates a duration from the three parts a wire duration decodes into,
    /// rejecting a day component.
    ///
    /// ```
    /// # use opportune::time::Duration;
    /// assert_eq!(Duration::from_parts(0, 2, 15).unwrap().minutes(), 135);
    /// assert!(Duration::from_parts(1, 0, 0).is_err());
    /// ```
    ///
    /// # Errors
    /// Returns [`TimeError::DurationHasDays`] when `days` is not zero.
    pub fn from_parts(days: u32, hours: u32, minutes: u32) -> Result<Self, TimeError> {
        if days != 0 {
            return Err(TimeError::DurationHasDays { days });
        }
        Ok(Self::from_hours_minutes(hours, minutes))
    }

    /// The whole minutes this duration lasts.
    #[must_use]
    pub const fn minutes(&self) -> i64 {
        self.minutes
    }
}

impl Default for Duration {
    /// No time at all: the natural default for travel, gaps and notice.
    fn default() -> Self {
        Self::ZERO
    }
}

impl fmt::Debug for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for Duration {
    /// Formats the duration as `{hours}h` for a whole number of hours, and as
    /// `{minutes}m` otherwise.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.minutes % 60 == 0 {
            write!(f, "{}h", self.minutes / 60)
        } else {
            write!(f, "{}m", self.minutes)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Duration, Instant, MAX_UNIX_SECONDS, MIN_UNIX_SECONDS};
    use crate::time::{Date, TimeError, TimeOfDay};

    #[test]
    fn a_utc_civil_time_names_exactly_one_instant() {
        assert_eq!(
            Instant::from_utc(
                Date::new(2027, 3, 9).unwrap(),
                TimeOfDay::new(15, 0).unwrap()
            )
            .unwrap()
            .to_string(),
            "2027-03-09T15:00Z"
        );
        assert_eq!(
            Instant::from_utc(Date::new(1975, 1, 1).unwrap(), TimeOfDay::MIDNIGHT),
            Ok(Instant::MIN)
        );
        assert_eq!(
            Instant::from_utc(Date::new(2100, 1, 1).unwrap(), TimeOfDay::MIDNIGHT),
            Ok(Instant::MAX)
        );
        let before = Date::new(1974, 12, 31).unwrap();
        assert_eq!(
            Instant::from_utc(before, TimeOfDay::MIDNIGHT),
            Err(TimeError::CivilToInstant {
                date: before,
                time: TimeOfDay::MIDNIGHT,
            })
        );
    }

    #[test]
    fn checked_arithmetic_stops_at_the_supported_range() {
        let later = Instant::MIN.checked_add(Duration::from_hours(1)).unwrap();
        assert_eq!(later.minutes() - Instant::MIN.minutes(), 60);
        assert_eq!(
            later.checked_sub(Duration::from_hours(1)),
            Some(Instant::MIN)
        );
        assert_eq!(Instant::MAX.checked_add(Duration::from_minutes(1)), None);
        assert_eq!(Instant::MIN.checked_sub(Duration::from_minutes(1)), None);
        assert_eq!(Instant::MIN.checked_add(Duration::ZERO), Some(Instant::MIN));
    }

    #[test]
    fn the_supported_range_is_the_specified_one() {
        assert_eq!(Instant::MIN.to_string(), "1975-01-01T00:00Z");
        assert_eq!(Instant::MAX.to_string(), "2100-01-01T00:00Z");
        assert_eq!(Instant::MIN.unix_seconds(), MIN_UNIX_SECONDS);
        assert_eq!(Instant::MAX.unix_seconds(), MAX_UNIX_SECONDS);
        assert!(Instant::from_unix_seconds(MIN_UNIX_SECONDS).is_ok());
        assert!(Instant::from_unix_seconds(MAX_UNIX_SECONDS).is_ok());
        assert_eq!(
            Instant::from_unix_seconds(MIN_UNIX_SECONDS - 60),
            Err(TimeError::InstantOutOfRange {
                seconds: MIN_UNIX_SECONDS - 60
            })
        );
        assert_eq!(
            Instant::from_unix_seconds(MAX_UNIX_SECONDS + 60),
            Err(TimeError::InstantOutOfRange {
                seconds: MAX_UNIX_SECONDS + 60
            })
        );
        assert_eq!(
            TimeError::InstantOutOfRange { seconds: 0 }.code(),
            "instant_out_of_range"
        );
    }

    #[test]
    fn rounding_moves_in_the_direction_the_request_needs() {
        // 2027-03-09T15:00:30Z
        let seconds = 1_804_604_430;
        assert_eq!(
            Instant::floor_to_minute(seconds).unwrap().to_string(),
            "2027-03-09T15:00Z"
        );
        assert_eq!(
            Instant::ceil_to_minute(seconds).unwrap().to_string(),
            "2027-03-09T15:01Z"
        );
        // An aligned instant is unchanged in both directions.
        for aligned in [1_804_604_400, 1_804_604_460] {
            assert_eq!(
                Instant::floor_to_minute(aligned).unwrap(),
                Instant::ceil_to_minute(aligned).unwrap()
            );
        }
    }

    #[test]
    fn unaligned_instants_are_rejected() {
        assert_eq!(
            Instant::from_unix_seconds(1_804_604_401),
            Err(TimeError::InstantNotAligned {
                seconds: 1_804_604_401
            })
        );
        assert_eq!(
            TimeError::InstantNotAligned { seconds: 1 }.code(),
            "instant_not_aligned"
        );
        assert_eq!(Instant::MIN.minutes() * 60, Instant::MIN.unix_seconds());
    }

    #[test]
    fn durations_are_exact_minutes() {
        assert_eq!(Duration::from_hours_minutes(1, 30).minutes(), 90);
        assert_eq!(Duration::from_minutes(45).minutes(), 45);
        assert_eq!(Duration::from_parts(0, 1, 30).unwrap().minutes(), 90);
        assert_eq!(
            Duration::from_parts(1, 0, 0),
            Err(TimeError::DurationHasDays { days: 1 })
        );
        assert_eq!(
            TimeError::DurationHasDays { days: 1 }.code(),
            "duration_has_days"
        );
        assert_eq!(Duration::from_hours_minutes(1, 30).to_string(), "90m");
        assert_eq!(Duration::from_hours_minutes(2, 0).to_string(), "2h");
    }
}
