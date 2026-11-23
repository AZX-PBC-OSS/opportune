//! Civil dates, times of day and weekdays.
//!
//! These are the wall-clock values a caller writes down — `2027-03-09`,
//! `09:00`, `friday` — with no time zone attached. [`Date::new`] rejects a day
//! that does not exist in its month and year, so a [`Date`] is always a real
//! calendar date.
//!
//! ```
//! use opportune::time::{Date, LocalDateTime, TimeOfDay, Weekday};
//!
//! let date = Date::new(2028, 2, 29).unwrap(); // a leap year
//! assert_eq!(date.weekday(), Weekday::Tuesday);
//! assert!(Date::new(2027, 2, 29).is_err()); // not a leap year
//!
//! let time = TimeOfDay::new(9, 30).unwrap();
//! assert_eq!(time.minute_of_day(), 570);
//!
//! let start = LocalDateTime::new(date, time);
//! assert_eq!(start.to_string(), "2028-02-29T09:30");
//! ```

use core::fmt;

use jiff::civil;

use crate::time::error::TimeError;

/// A date in the civil calendar, independent of any time zone.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date(civil::Date);

impl Date {
    /// Creates a civil date.
    ///
    /// ```
    /// # use opportune::time::Date;
    /// let date = Date::new(2027, 3, 9).unwrap();
    /// assert_eq!((date.year(), date.month(), date.day()), (2027, 3, 9));
    /// ```
    ///
    /// # Errors
    /// Returns [`TimeError::DateInvalid`] when the day does not fall inside its
    /// month, as for `2027-02-29`.
    pub fn new(year: i16, month: i8, day: i8) -> Result<Self, TimeError> {
        civil::Date::new(year, month, day)
            .map(Self)
            .map_err(|_| TimeError::DateInvalid { year, month, day })
    }

    /// The year, which is the proleptic Gregorian year.
    #[must_use]
    pub fn year(&self) -> i16 {
        self.0.year()
    }

    /// The month, 1 to 12.
    #[must_use]
    pub fn month(&self) -> i8 {
        self.0.month()
    }

    /// The day of the month, 1 to 31.
    #[must_use]
    pub fn day(&self) -> i8 {
        self.0.day()
    }

    /// The day of the week this date falls on.
    #[must_use]
    pub fn weekday(&self) -> Weekday {
        match self.0.weekday() {
            civil::Weekday::Monday => Weekday::Monday,
            civil::Weekday::Tuesday => Weekday::Tuesday,
            civil::Weekday::Wednesday => Weekday::Wednesday,
            civil::Weekday::Thursday => Weekday::Thursday,
            civil::Weekday::Friday => Weekday::Friday,
            civil::Weekday::Saturday => Weekday::Saturday,
            civil::Weekday::Sunday => Weekday::Sunday,
        }
    }

    /// The date `days` days after this one, or before it for a negative
    /// number.
    ///
    /// ```
    /// # use opportune::time::Date;
    /// let leap_day = Date::new(2028, 2, 29).unwrap();
    /// assert_eq!(leap_day.plus_days(1).unwrap(), Date::new(2028, 3, 1).unwrap());
    /// assert_eq!(leap_day.plus_days(-1).unwrap(), Date::new(2028, 2, 28).unwrap());
    /// ```
    ///
    /// # Errors
    /// Returns [`TimeError::DateOverflow`] when the result leaves the
    /// representable calendar.
    pub fn plus_days(self, days: i64) -> Result<Self, TimeError> {
        // A span's day field is limited to twenty thousand years and panics
        // beyond it, so the step goes through an absolute duration, whose
        // out-of-range result is the error this function returns.
        const SECONDS_PER_DAY: i64 = 60 * 60 * 24;
        let seconds = days
            .checked_mul(SECONDS_PER_DAY)
            .ok_or(TimeError::DateOverflow { days })?;
        self.0
            .checked_add(jiff::SignedDuration::from_secs(seconds))
            .map(Self)
            .map_err(|_| TimeError::DateOverflow { days })
    }

    /// The first day of this date's month.
    ///
    /// A cap's month bucket is labelled with this date, so every day of a civil
    /// month shares one label.
    pub(crate) fn first_of_month(self) -> Self {
        Self::new(self.year(), self.month(), 1).expect("the first day of a month always exists")
    }

    /// The first day of the month after this date's.
    pub(crate) fn next_month_start(self) -> Result<Self, TimeError> {
        let (year, month) = if self.month() == 12 {
            (
                self.year().checked_add(1).ok_or(TimeError::DateOverflow {
                    days: i64::from(self.month()),
                })?,
                1,
            )
        } else {
            (self.year(), self.month() + 1)
        };
        Self::new(year, month, 1)
    }

    /// The first day of the week containing this date, where a week starts on
    /// `starts` and runs seven civil days.
    ///
    /// A cap's week bucket is labelled with this date, so the configured weekday
    /// — not ISO's Monday — decides where a bucket begins.
    pub(crate) fn week_start(self, starts: Weekday) -> Result<Self, TimeError> {
        let offset = i64::from(self.weekday().index()) - i64::from(starts.index());
        self.plus_days(-offset.rem_euclid(7))
    }

    /// The underlying [`jiff::civil::Date`]. Internal: no `jiff` type appears
    /// in the public API.
    pub(crate) const fn as_jiff(self) -> civil::Date {
        self.0
    }

    /// Wraps a `jiff` date, which is always a valid calendar date.
    pub(crate) const fn from_jiff(date: civil::Date) -> Self {
        Self(date)
    }
}

impl fmt::Debug for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for Date {
    /// Formats the date as `YYYY-MM-DD`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A time of day to the minute: `00:00` to `23:59`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TimeOfDay {
    hour: u8,
    minute: u8,
}

impl TimeOfDay {
    /// The number of minutes in one day.
    pub const MINUTES_PER_DAY: u16 = 24 * 60;

    /// Midnight, `00:00`.
    pub const MIDNIGHT: Self = Self { hour: 0, minute: 0 };

    /// Creates a time of day from an hour and a minute.
    ///
    /// ```
    /// # use opportune::time::TimeOfDay;
    /// let anchor = TimeOfDay::new(9, 30).unwrap();
    /// assert_eq!((anchor.hour(), anchor.minute()), (9, 30));
    /// assert!(TimeOfDay::new(24, 0).is_err());
    /// ```
    ///
    /// # Errors
    /// Returns [`TimeError::TimeOfDayInvalid`] unless `hour < 24` and
    /// `minute < 60`.
    pub const fn new(hour: u8, minute: u8) -> Result<Self, TimeError> {
        if hour >= 24 || minute >= 60 {
            return Err(TimeError::TimeOfDayInvalid { hour, minute });
        }
        Ok(Self { hour, minute })
    }

    /// Creates a time of day from the minute of the day, `0` being `00:00` and
    /// `1439` being `23:59`.
    ///
    /// # Errors
    /// Returns [`TimeError::MinuteOfDayOutOfRange`] for a minute that is not
    /// inside one day.
    pub fn from_minute_of_day(minute_of_day: u16) -> Result<Self, TimeError> {
        let out_of_range = TimeError::MinuteOfDayOutOfRange {
            minute: i64::from(minute_of_day),
        };
        if minute_of_day >= Self::MINUTES_PER_DAY {
            return Err(out_of_range);
        }
        Ok(Self {
            hour: u8::try_from(minute_of_day / 60).map_err(|_| out_of_range.clone())?,
            minute: u8::try_from(minute_of_day % 60).map_err(|_| out_of_range)?,
        })
    }

    /// This time of day on `date`, as a `jiff` civil date-time.
    pub(crate) fn on(self, date: Date) -> civil::DateTime {
        // `hour < 24` and `minute < 60` hold for every `TimeOfDay`, so both
        // narrowings are exact and `at` cannot panic.
        let hour = i8::try_from(self.hour).expect("a time-of-day hour is below 24");
        let minute = i8::try_from(self.minute).expect("a time-of-day minute is below 60");
        date.as_jiff().at(hour, minute, 0, 0)
    }

    /// Wraps a `jiff` civil time, whose components are always inside their
    /// ranges.
    pub(crate) fn from_jiff(time: civil::Time) -> Self {
        Self {
            hour: u8::try_from(time.hour()).expect("a jiff civil hour is below 24"),
            minute: u8::try_from(time.minute()).expect("a jiff civil minute is below 60"),
        }
    }

    /// The minute count of the day, `0` to `1439`.
    #[must_use]
    pub fn minute_of_day(&self) -> u16 {
        u16::from(self.hour) * 60 + u16::from(self.minute)
    }

    /// The hour, 0 to 23.
    #[must_use]
    pub const fn hour(&self) -> u8 {
        self.hour
    }

    /// The minute within the hour, 0 to 59.
    #[must_use]
    pub const fn minute(&self) -> u8 {
        self.minute
    }
}

impl fmt::Debug for TimeOfDay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for TimeOfDay {
    /// Formats the time as `HH:MM`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}:{:02}", self.hour(), self.minute())
    }
}

/// A day of the week.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Weekday {
    /// Monday.
    Monday,
    /// Tuesday.
    Tuesday,
    /// Wednesday.
    Wednesday,
    /// Thursday.
    Thursday,
    /// Friday.
    Friday,
    /// Saturday.
    Saturday,
    /// Sunday.
    Sunday,
}

impl Weekday {
    /// Every weekday, Monday first.
    ///
    /// ```
    /// use opportune::time::Weekday;
    ///
    /// assert_eq!(Weekday::ALL.len(), 7);
    /// assert_eq!(Weekday::ALL[0], Weekday::Monday);
    /// assert_eq!(Weekday::iter().last(), Some(Weekday::Sunday));
    /// ```
    pub const ALL: [Self; 7] = [
        Self::Monday,
        Self::Tuesday,
        Self::Wednesday,
        Self::Thursday,
        Self::Friday,
        Self::Saturday,
        Self::Sunday,
    ];

    /// Every weekday, Monday first.
    pub fn iter() -> impl Iterator<Item = Self> {
        Self::ALL.into_iter()
    }

    /// The weekday's position in the week, `0` for Monday through `6` for
    /// Sunday.
    pub(crate) const fn index(self) -> u8 {
        match self {
            Self::Monday => 0,
            Self::Tuesday => 1,
            Self::Wednesday => 2,
            Self::Thursday => 3,
            Self::Friday => 4,
            Self::Saturday => 5,
            Self::Sunday => 6,
        }
    }

    /// The bit this weekday occupies in a [`Weekdays`] mask, `0` for Monday
    /// through `6` for Sunday.
    pub(crate) const fn bit(self) -> u8 {
        1 << self.index()
    }
}

impl fmt::Display for Weekday {
    /// Formats the weekday as its three-letter, lower-case name: `mon` to
    /// `sun`, the spelling a `weekly` key uses.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Monday => "mon",
            Self::Tuesday => "tue",
            Self::Wednesday => "wed",
            Self::Thursday => "thu",
            Self::Friday => "fri",
            Self::Saturday => "sat",
            Self::Sunday => "sun",
        })
    }
}

/// A set of weekdays.
///
/// One representation serves both the days a `window` key covers and the
/// weekdays a working calendar works, so a set built for one is usable as the
/// other without conversion. [`WorkingDays`] is the calendar-facing name for
/// this type.
///
/// ```
/// use opportune::time::{Weekday, Weekdays};
///
/// let weekend = Weekdays::range(Weekday::Friday, Weekday::Monday);
/// assert!(weekend.contains(Weekday::Sunday));
/// assert!(!weekend.contains(Weekday::Tuesday));
///
/// let business = Weekdays::weekdays();
/// assert!(!business.contains(Weekday::Saturday));
/// assert_eq!(business.iter().count(), 5);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Weekdays {
    mask: u8,
}

impl Weekdays {
    /// No day at all.
    pub const NONE: Self = Self { mask: 0 };

    /// Every day of the week.
    pub const ALL: Self = Self { mask: 0b111_1111 };

    /// Monday to Friday: the working week.
    // The name is the one a working calendar has always used, and it still reads
    // as the set of weekdays rather than a constructor of the type.
    #[allow(clippy::self_named_constructors)]
    #[must_use]
    pub const fn weekdays() -> Self {
        Self {
            mask: Weekday::Monday.bit()
                | Weekday::Tuesday.bit()
                | Weekday::Wednesday.bit()
                | Weekday::Thursday.bit()
                | Weekday::Friday.bit(),
        }
    }

    /// The set holding only `day`.
    #[must_use]
    pub fn single(day: Weekday) -> Self {
        Self { mask: day.bit() }
    }

    /// The set holding exactly the days given, in any order and with repeats.
    #[must_use]
    pub fn list(days: impl IntoIterator<Item = Weekday>) -> Self {
        Self {
            mask: days.into_iter().fold(0, |mask, day| mask | day.bit()),
        }
    }

    /// The inclusive range from `first` to `last`, wrapping the week when
    /// `last` comes before `first`.
    #[must_use]
    pub fn range(first: Weekday, last: Weekday) -> Self {
        let mut mask = 0u8;
        let mut index = first.index();
        loop {
            mask |= 1 << index;
            if index == last.index() {
                break;
            }
            index = (index + 1) % 7;
        }
        Self { mask }
    }

    /// Whether the set holds `day`.
    #[must_use]
    pub const fn contains(&self, day: Weekday) -> bool {
        self.mask & day.bit() != 0
    }

    /// Whether the set holds no day at all.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.mask == 0
    }

    /// The days in the set, Monday first.
    pub fn iter(&self) -> impl Iterator<Item = Weekday> + '_ {
        Weekday::iter().filter(|day| self.contains(*day))
    }
}

impl fmt::Debug for Weekdays {
    /// Formats the set as a comma-separated list of three-letter names, or
    /// `<none>` for the empty set.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for day in self.iter() {
            if !first {
                f.write_str(",")?;
            }
            write!(f, "{day}")?;
            first = false;
        }
        if first {
            f.write_str("<none>")?;
        }
        Ok(())
    }
}

/// The weekdays a working calendar works.
///
/// The same set as [`Weekdays`], which is what a `window` key's day side is:
/// one representation for both, so the two spellings convert for free.
///
/// ```
/// use opportune::time::{WorkingDays, Weekdays};
///
/// let business: WorkingDays = Weekdays::weekdays();
/// assert!(!business.is_empty());
/// ```
pub type WorkingDays = Weekdays;

/// A civil date and time of day, independent of any time zone.
///
/// This is the wall-clock value a blackout span names, and the value a window
/// range is measured against, before either is resolved in a zone.
///
/// ```
/// use opportune::time::{Date, LocalDateTime, TimeOfDay};
///
/// let start = LocalDateTime::new(
///     Date::new(2026, 12, 24).unwrap(),
///     TimeOfDay::MIDNIGHT,
/// );
/// assert_eq!(start.date(), Date::new(2026, 12, 24).unwrap());
/// assert_eq!(start.time(), TimeOfDay::MIDNIGHT);
/// assert_eq!(start.to_string(), "2026-12-24T00:00");
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LocalDateTime {
    date: Date,
    time: TimeOfDay,
}

impl LocalDateTime {
    /// Joins a civil date and a time of day.
    #[must_use]
    pub const fn new(date: Date, time: TimeOfDay) -> Self {
        Self { date, time }
    }

    /// The date part.
    #[must_use]
    pub const fn date(&self) -> Date {
        self.date
    }

    /// The time-of-day part.
    #[must_use]
    pub const fn time(&self) -> TimeOfDay {
        self.time
    }
}

impl fmt::Debug for LocalDateTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for LocalDateTime {
    /// Formats the date-time as `YYYY-MM-DDTHH:MM`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}T{}", self.date, self.time)
    }
}

#[cfg(test)]
mod tests {
    use super::{Date, TimeOfDay, Weekday};
    use crate::time::TimeError;

    #[test]
    fn leap_days_follow_the_calendar() {
        assert!(Date::new(2028, 2, 29).is_ok());
        assert!(Date::new(2027, 2, 29).is_err());
        assert!("2027-02-29".parse::<jiff::civil::Date>().is_err());
        assert_eq!(
            Date::new(2027, 2, 29),
            Err(TimeError::DateInvalid {
                year: 2027,
                month: 2,
                day: 29
            })
        );
        assert_eq!(Date::new(2028, 2, 29).unwrap().to_string(), "2028-02-29");
    }

    #[test]
    fn weekdays_follow_the_date() {
        assert_eq!(Date::new(2027, 3, 8).unwrap().weekday(), Weekday::Monday);
        assert_eq!(Date::new(2027, 3, 12).unwrap().weekday(), Weekday::Friday);
        assert_eq!(Date::new(2027, 3, 13).unwrap().weekday(), Weekday::Saturday);
        assert_eq!(Date::new(2027, 3, 14).unwrap().weekday(), Weekday::Sunday);
    }

    #[test]
    fn plus_days_crosses_month_and_year_boundaries() {
        let date = Date::new(2027, 12, 31).unwrap();
        assert_eq!(date.plus_days(1).unwrap(), Date::new(2028, 1, 1).unwrap());
        assert_eq!(
            Date::new(2028, 3, 1).unwrap().plus_days(-1).unwrap(),
            Date::new(2028, 2, 29).unwrap()
        );
        assert_eq!(date.plus_days(0).unwrap(), date);
    }

    #[test]
    fn errors_carry_stable_codes() {
        assert_eq!(
            TimeError::DateInvalid {
                year: 2027,
                month: 2,
                day: 29
            }
            .code(),
            "date_invalid"
        );
        assert_eq!(
            TimeError::TimeOfDayInvalid {
                hour: 24,
                minute: 0
            }
            .code(),
            "time_of_day_invalid"
        );
        assert_eq!(
            TimeError::MinuteOfDayOutOfRange { minute: 1440 }.code(),
            "minute_of_day_out_of_range"
        );
    }

    #[test]
    fn times_of_day_round_trip_through_minutes() {
        assert_eq!(
            TimeOfDay::new(9, 30).unwrap().minute_of_day(),
            570,
            "half past nine is 570 minutes into the day"
        );
        assert_eq!(
            TimeOfDay::from_minute_of_day(570).unwrap(),
            TimeOfDay::new(9, 30).unwrap()
        );
        assert_eq!(TimeOfDay::MINUTES_PER_DAY, 1440);
        assert_eq!(
            TimeOfDay::new(23, 59).unwrap().minute_of_day(),
            TimeOfDay::MINUTES_PER_DAY - 1
        );
        assert_eq!(TimeOfDay::MIDNIGHT.to_string(), "00:00");
        assert!(TimeOfDay::from_minute_of_day(1440).is_err());
        assert!(TimeOfDay::new(12, 60).is_err());
    }
}
