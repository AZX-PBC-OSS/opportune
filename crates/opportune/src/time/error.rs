//! The failures a civil-time value or operation can produce.

use thiserror::Error;

use crate::time::{Date, Instant, LocalDateTime, TimeOfDay, Weekday};

/// Why a civil-time value could not be constructed, or an operation rejected.
///
/// Every variant carries a stable [`TimeError::code`] so callers can match on
/// the failure across releases; the display text is for humans and may change.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum TimeError {
    /// A Unix second value lies outside the supported range
    /// `1975-01-01T00:00Z..=2100-01-01T00:00Z`.
    #[error(
        "instant {seconds} is outside the supported range 1975-01-01T00:00Z..=2100-01-01T00:00Z"
    )]
    InstantOutOfRange {
        /// The rejected Unix second value, counting from 1970-01-01T00:00Z.
        seconds: i64,
    },
    /// A Unix second value is not a whole multiple of 60, so it has no
    /// minute-resolution representation.
    #[error("instant {seconds} is not aligned to a whole minute")]
    InstantNotAligned {
        /// The rejected Unix second value.
        seconds: i64,
    },
    /// A nanosecond component that does not name a point inside its second.
    #[error("nanosecond {nanos} does not fall inside one second")]
    NanosOutOfRange {
        /// The rejected nanosecond component.
        nanos: u32,
    },
    /// A seconds component that does not name a point inside its minute.
    #[error("second {second} does not fall inside one minute")]
    SecondOutOfRange {
        /// The rejected seconds component.
        second: u8,
    },
    /// A civil date does not exist, such as 29 February in a common year.
    #[error("date {year}-{month:02}-{day:02} does not exist")]
    DateInvalid {
        /// The rejected year.
        year: i16,
        /// The rejected month, 1 to 12.
        month: i8,
        /// The rejected day of the month.
        day: i8,
    },
    /// Date arithmetic would leave the representable calendar.
    #[error("moving a date by {days} days leaves the representable calendar")]
    DateOverflow {
        /// The number of days the operation tried to move.
        days: i64,
    },
    /// A duration arrived with a day component. Durations are exact minutes: a
    /// day is 23 to 25 hours in a named zone, so a duration never contains one.
    #[error("a duration cannot contain days, but {days} were given")]
    DurationHasDays {
        /// The rejected number of days.
        days: u32,
    },
    /// An hour or minute pair that is not a time of day.
    #[error("time of day {hour}:{minute:02} does not exist")]
    TimeOfDayInvalid {
        /// The rejected hour.
        hour: u8,
        /// The rejected minute.
        minute: u8,
    },
    /// A minute count outside one day's `0..1440` cannot be a time of day.
    #[error("minute {minute} of the day does not exist")]
    MinuteOfDayOutOfRange {
        /// The rejected minute of the day.
        minute: i64,
    },
    /// A civil date-time could not be mapped to an instant, because the result
    /// would leave the representable instant range.
    #[error("the civil date-time {date} {time} cannot be mapped to an instant")]
    CivilToInstant {
        /// The civil date that could not be mapped.
        date: Date,
        /// The civil time of day that could not be mapped.
        time: TimeOfDay,
    },
    /// A zone name is not in the engine's database.
    #[error("unknown time zone `{name}`")]
    ZoneUnknown {
        /// The rejected IANA name.
        name: String,
    },
    /// A zone name that names nothing: empty, or only whitespace. Whether a
    /// name names a zone is an engine question answered by the database; a
    /// name with no characters in it is not one in any database.
    #[error("`{name}` is not a time zone name")]
    ZoneNameInvalid {
        /// The rejected name.
        name: String,
    },
    /// A zone period that covers no instants.
    #[error("zone period is empty: {from} is not before {until}")]
    ZonePeriodEmpty {
        /// The rejected period's start.
        from: Instant,
        /// The rejected period's end.
        until: Instant,
    },
    /// Two zone periods cover a shared instant. Periods are half-open and
    /// disjoint: adjacent legs of a trip join exactly, but overlapping ones are
    /// ambiguous about which zone applies.
    #[error("zone periods overlap from {from}")]
    ZonePeriodsOverlap {
        /// The instant at which the later period starts inside the earlier one.
        from: Instant,
    },
    /// A grid step outside `1..=1440` minutes.
    #[error("grid step {minutes} minutes is outside 1..=1440")]
    GridStepOutOfRange {
        /// The rejected step in minutes.
        minutes: i64,
    },
    /// A working calendar that works no weekday, which makes a count of
    /// working days meaningless: no day after today would ever be counted.
    #[error(
        "`working_days` must name at least one weekday, or a count of working days could never \
         advance"
    )]
    WorkingDaysEmpty,
    /// A time zone database directory has no `+VERSION` file and the caller
    /// supplied no label, so every response would report an undefined version.
    #[error("time zone database at `{path}` has no +VERSION file and no version label was given")]
    TzdbVersionMissing {
        /// The directory the database was loaded from.
        path: String,
    },
    /// A time zone database directory cannot serve zone data.
    #[error("time zone database at `{path}` is unusable: {reason}")]
    TzdbDirectoryUnusable {
        /// The directory the database was loaded from.
        path: String,
        /// Why the directory cannot serve zone data.
        reason: String,
    },
    /// The bundled time zone database is empty, which means the binary was
    /// built without the data it promises.
    #[error("the bundled time zone database is empty")]
    TzdbBundledUnavailable,
    /// An engine was built without a time zone database. There is no system
    /// fallback: a database is always an explicit choice.
    #[error("no time zone database was configured for the engine")]
    TzdbNotConfigured,
}

impl TimeError {
    /// A stable, machine-readable code for this failure.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InstantOutOfRange { .. } => "instant_out_of_range",
            Self::InstantNotAligned { .. } => "instant_not_aligned",
            Self::NanosOutOfRange { .. } => "nanos_out_of_range",
            Self::SecondOutOfRange { .. } => "second_out_of_range",
            Self::DateInvalid { .. } => "date_invalid",
            Self::DateOverflow { .. } => "date_overflow",
            Self::DurationHasDays { .. } => "duration_has_days",
            Self::TimeOfDayInvalid { .. } => "time_of_day_invalid",
            Self::MinuteOfDayOutOfRange { .. } => "minute_of_day_out_of_range",
            Self::CivilToInstant { .. } => "civil_to_instant",
            Self::ZoneUnknown { .. } => "zone_unknown",
            Self::ZoneNameInvalid { .. } => "zone_name_invalid",
            Self::ZonePeriodEmpty { .. } => "zone_period_empty",
            Self::ZonePeriodsOverlap { .. } => "zone_periods_overlap",
            Self::GridStepOutOfRange { .. } => "grid_step_out_of_range",
            Self::WorkingDaysEmpty => "working_days_empty",
            Self::TzdbVersionMissing { .. } => "tzdb_version_missing",
            Self::TzdbDirectoryUnusable { .. } => "tzdb_directory_unusable",
            Self::TzdbBundledUnavailable => "tzdb_bundled_unavailable",
            Self::TzdbNotConfigured => "tzdb_not_configured",
        }
    }
}

/// Why a wall-clock `window` specification was rejected, or a preset refused the
/// availability it would write as dated entries.
///
/// A window is validated once, when it is built, so no later evaluation has to
/// ask whether its keys are well formed. The [`WindowError::code`] values are
/// stable: callers match on them across releases.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum WindowError {
    /// A weekday appears in more than one `weekly` key, so two sets of hours
    /// would claim the same day.
    #[error("weekday {day:?} appears in more than one weekly key")]
    WeekdayRepeated {
        /// The weekday claimed twice.
        day: Weekday,
    },
    /// A date falls inside more than one `dates` key, so two overrides would
    /// claim the same date.
    #[error("date {date} appears in more than one dates key")]
    DateRepeated {
        /// The first date claimed by two keys.
        date: Date,
    },
    /// Two ranges within one key overlap, which leaves their union ambiguous
    /// and is almost always a mistake.
    #[error("ranges within one key overlap at {at}")]
    RangesOverlap {
        /// The first time of day covered by both ranges.
        at: TimeOfDay,
    },
    /// A range whose start equals its end, such as `00:00-00:00`. A full day is
    /// written `00:00-24:00`.
    #[error("a range must end after it starts, but it runs {start}-{start}")]
    RangeEmpty {
        /// The start, which is also the end.
        start: TimeOfDay,
    },
    /// An inclusive date range whose end falls before its start.
    #[error("date range ends before it starts: {start} to {end}")]
    DateRangeReversed {
        /// The range's start date.
        start: Date,
        /// The range's end date.
        end: Date,
    },
    /// Stated availability a preset would write as more dated entries than it
    /// accepts, one per UTC day of every stated span.
    #[error(
        "stated availability needs {entries} dated entries, more than the {max} a preset accepts"
    )]
    AvailabilityTooLong {
        /// The dated entries the stated spans need.
        entries: u64,
        /// The most dated entries a preset writes.
        max: u32,
    },
}

impl WindowError {
    /// A stable, machine-readable code for this failure.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::WeekdayRepeated { .. } => "window_weekday_repeated",
            Self::DateRepeated { .. } => "window_date_repeated",
            Self::RangesOverlap { .. } => "window_ranges_overlap",
            Self::RangeEmpty { .. } => "window_range_empty",
            Self::DateRangeReversed { .. } => "window_date_range_reversed",
            Self::AvailabilityTooLong { .. } => "window_availability_too_long",
        }
    }
}

/// Why a `blackout` specification was rejected.
///
/// The [`BlackoutError::code`] value is stable: callers match on it across
/// releases.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum BlackoutError {
    /// A span that covers no instant, because its end is at or before its start.
    /// Blackout spans are half-open, so a "span" from a date-time to itself is
    /// empty and an inverted one is a typo, not a range.
    #[error("blackout span {start} to {end} is empty: it must end after it starts")]
    SpanEmpty {
        /// The span's start.
        start: LocalDateTime,
        /// The span's end, which is not after the start.
        end: LocalDateTime,
    },
}

impl BlackoutError {
    /// A stable, machine-readable code for this failure.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::SpanEmpty { .. } => "blackout_span_empty",
        }
    }
}
