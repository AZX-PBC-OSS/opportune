//! The wire primitives the JSON format spells out as text: instants, durations,
//! civil dates, times and day keys, and the `true` selector.
//!
//! Every parser takes the RFC 6901 `path` of the field it is reading, so a
//! rejection names the exact place in the document. Rounding is a parameter
//! rather than a parser variant, because the wire format rounds the same
//! spelling in three ways: `now` up to the next minute, a block's endpoints
//! outward, and everything else must already be minute-aligned.
//!
//! Instants are parsed here rather than through `jiff`, which the core keeps to
//! itself: the civil-to-Unix conversion is a few lines of exact integer
//! arithmetic, and the rounded value goes through [`Instant`]'s own constructors,
//! so the supported range and the rounding rules are stated once.

use core::fmt;

use opportune::time::{
    Date, DateRange, Duration, Instant, LocalDateTime, TimeOfDay, WallEnd, WallRange, Weekday,
    Weekdays, WorkingDays,
};
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::JsonError;

/// How an instant that carries seconds or a fraction is rounded to the wire's
/// minute resolution.
///
/// The wire format rounds `now` up, a block's start down and its end up, and
/// requires candidate starts and every other instant to be minute-aligned
/// already.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstantRounding {
    /// Reject a value with a seconds or nanosecond component.
    Exact,
    /// Round up to the next whole minute.
    Ceil,
    /// Round down to the whole minute the value falls in.
    Floor,
}

/// Parses an RFC 3339 timestamp with an explicit offset, rounded by `rounding`.
///
/// `Z` and `±HH:MM` offsets are accepted, with seconds and up to nine fractional
/// digits optional. RFC 3339 lets the `T` and `Z` be written lower case, and they
/// read the same way here, because a producer emitting `2027-03-09t15:00z` is
/// writing a timestamp this contract says it reads. A timestamp without an offset
/// is rejected: a wall-clock reading is not an instant.
///
/// ```
/// use opportune_json::primitives::{InstantRounding, parse_instant};
///
/// let instant = parse_instant("2027-03-09T15:00:00Z", InstantRounding::Exact, "/now").unwrap();
/// assert_eq!(instant.to_string(), "2027-03-09T15:00Z");
///
/// // A block's start rounds down; `now` rounds up.
/// assert_eq!(
///     parse_instant("2027-03-09T15:00:59Z", InstantRounding::Floor, "/span/0")
///         .unwrap()
///         .to_string(),
///     "2027-03-09T15:00Z"
/// );
/// assert_eq!(
///     parse_instant("2027-03-09T15:00:01Z", InstantRounding::Ceil, "/now")
///         .unwrap()
///         .to_string(),
///     "2027-03-09T15:01Z"
/// );
/// ```
///
/// # Errors
/// Returns [`JsonError`] with code `instant_invalid` for a spelling that is not a
/// timestamp, and the code of the [`Instant`] constructor that rejected the value
/// — `instant_not_aligned` for [`InstantRounding::Exact`], `instant_out_of_range`
/// outside 1975-01-01T00:00Z to 2100-01-01T00:00Z — otherwise.
pub fn parse_instant(
    input: &str,
    rounding: InstantRounding,
    path: &str,
) -> Result<Instant, JsonError> {
    let invalid = || {
        JsonError::new(
            "instant_invalid",
            format!("`{input}` is not an RFC 3339 timestamp with an explicit offset"),
            path,
        )
    };
    let (date, clock) = split_date_time(input).ok_or_else(invalid)?;
    let date = parse_date(date, path)?;
    let (clock, offset_seconds) = split_offset(clock).ok_or_else(invalid)?;
    let (seconds, nanos) = parse_clock(clock).ok_or_else(invalid)?;
    let unix_seconds = days_from_civil(date.year(), date.month(), date.day()) * SECONDS_PER_DAY
        + seconds
        - offset_seconds;
    let rounded = match rounding {
        InstantRounding::Exact => Instant::from_unix_seconds_nanos(unix_seconds, nanos),
        InstantRounding::Ceil => Instant::ceil_to_minute_nanos(unix_seconds, nanos),
        InstantRounding::Floor => Instant::floor_to_minute_nanos(unix_seconds, nanos),
    };
    rounded.map_err(|err| JsonError::from_domain(err.into(), path))
}

/// Formats a duration the way the wire reads it: `{hours}h`, `{minutes}m`, or a
/// combination such as `1h30m`.
///
/// ```
/// use opportune::time::Duration;
/// use opportune_json::primitives::format_duration;
///
/// assert_eq!(format_duration(Duration::from_minutes(45)), "45m");
/// assert_eq!(format_duration(Duration::from_hours(2)), "2h");
/// ```
#[must_use]
pub fn format_duration(duration: Duration) -> String {
    duration.to_string()
}

/// Parses a duration written in hours and minutes, such as `30m`, `4h` or
/// `1h30m`.
///
/// A day is not an exact length — it is 23 to 25 hours in a named zone — so a
/// duration that names one is rejected with its own code rather than guessed at.
///
/// ```
/// use opportune_json::primitives::parse_duration;
///
/// assert_eq!(parse_duration("1h30m", "/goal/parts/0/duration").unwrap().minutes(), 90);
/// assert_eq!(parse_duration("0m", "/goal/parts/0/duration").unwrap().minutes(), 0);
/// assert_eq!(
///     parse_duration("1d", "/goal/parts/0/duration").unwrap_err().code(),
///     "duration_contains_days"
/// );
/// ```
///
/// # Errors
/// Returns [`JsonError`] with code `duration_contains_days` for a duration that
/// names days, and `duration_invalid` for a spelling that is not a pair of
/// hour and minute components, or that names more hours or minutes than a duration
/// can hold.
pub fn parse_duration(input: &str, path: &str) -> Result<Duration, JsonError> {
    let invalid = || {
        JsonError::new(
            "duration_invalid",
            format!(
                "`{input}` is not a duration written in hours and minutes, such as `30m` or `1h30m`"
            ),
            path,
        )
    };
    let mut rest = input;
    let mut hours = None;
    let mut minutes = None;
    while !rest.is_empty() {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return Err(invalid());
        }
        let (number, tail) = rest.split_at(digits);
        // The digits are only ASCII digits, so the parse can fail only because the
        // count does not fit; saying so is the honest rejection, where the shape
        // sentence would claim a spelling that is exactly the shape it asks for.
        let value: u32 = number.parse().map_err(|_| {
            JsonError::new(
                "duration_invalid",
                format!("`{input}` names more hours or minutes than a duration can hold"),
                path,
            )
        })?;
        let Some(unit) = tail.chars().next() else {
            return Err(invalid());
        };
        match unit {
            'd' => {
                return Err(JsonError::new(
                    "duration_contains_days",
                    format!("`{input}` names days, and a day is not an exact duration"),
                    path,
                ));
            }
            'h' if hours.is_none() => hours = Some(value),
            'm' if minutes.is_none() => minutes = Some(value),
            _ => return Err(invalid()),
        }
        rest = &tail[unit.len_utf8()..];
    }
    match (hours, minutes) {
        (None, None) => Err(invalid()),
        (hours, minutes) => Ok(Duration::from_hours_minutes(
            hours.unwrap_or(0),
            minutes.unwrap_or(0),
        )),
    }
}

/// Parses a civil date written `YYYY-MM-DD`.
///
/// ```
/// use opportune_json::primitives::parse_date;
///
/// assert_eq!(parse_date("2028-02-29", "/date").unwrap().to_string(), "2028-02-29");
/// assert_eq!(parse_date("2027-02-29", "/date").unwrap_err().code(), "date_invalid");
/// assert_eq!(parse_date("2027-3-9", "/date").unwrap_err().code(), "date_invalid");
/// ```
///
/// # Errors
/// Returns [`JsonError`] with code `date_invalid` for a spelling that is not
/// `YYYY-MM-DD` and for a day that does not exist in its month.
pub fn parse_date(input: &str, path: &str) -> Result<Date, JsonError> {
    let invalid = || {
        JsonError::new(
            "date_invalid",
            format!("`{input}` is not a civil date written YYYY-MM-DD"),
            path,
        )
    };
    let mut parts = input.split('-');
    let (Some(year), Some(month), Some(day), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(invalid());
    };
    // The integer parser accepts a leading sign, so `+123-01-01` and `2027-+1-09`
    // would otherwise read as dates. A date is digits only.
    if year.len() != 4
        || month.len() != 2
        || day.len() != 2
        || !is_digits(year)
        || !is_digits(month)
        || !is_digits(day)
    {
        return Err(invalid());
    }
    let year: i16 = year.parse().map_err(|_| invalid())?;
    let month: i8 = month.parse().map_err(|_| invalid())?;
    let day: i8 = day.parse().map_err(|_| invalid())?;
    Date::new(year, month, day).map_err(|err| JsonError::from_domain(err.into(), path))
}

/// Parses a single civil date or an inclusive range `start..end`.
///
/// ```
/// use opportune_json::primitives::parse_date_range;
///
/// let week = parse_date_range("2027-03-08..2027-03-10", "/dates").unwrap();
/// assert_eq!(week.start().to_string(), "2027-03-08");
/// assert_eq!(week.end().to_string(), "2027-03-10");
/// let day = parse_date_range("2027-03-08", "/dates").unwrap();
/// assert_eq!(day.start(), day.end());
/// ```
///
/// # Errors
/// Returns [`JsonError`] with code `date_invalid` for a date that is malformed or
/// does not exist, `date_range_invalid` for a spelling that is neither form, and
/// `window_date_range_reversed` for a range that ends before it starts.
pub fn parse_date_range(input: &str, path: &str) -> Result<DateRange, JsonError> {
    let Some((start, end)) = input.split_once("..") else {
        return Ok(DateRange::single(parse_date(input, path)?));
    };
    if end.contains("..") {
        return Err(JsonError::new(
            "date_range_invalid",
            format!("`{input}` is not a date or an inclusive date range written `start..end`"),
            path,
        ));
    }
    let start = parse_date(start, path)?;
    let end = parse_date(end, path)?;
    DateRange::new(start, end).map_err(|err| JsonError::from_domain(err.into(), path))
}

/// Parses a time of day written `HH:MM`.
///
/// [`WallEnd`] reads `24:00` through [`parse_wall_end`]; this parser stops at
/// `23:59`, because no time of day begins at midnight's end.
///
/// ```
/// use opportune_json::primitives::parse_time_of_day;
///
/// assert_eq!(parse_time_of_day("09:30", "/anchor").unwrap().to_string(), "09:30");
/// assert!(parse_time_of_day("24:00", "/anchor").is_err());
/// ```
///
/// # Errors
/// Returns [`JsonError`] with code `time_of_day_invalid` for a spelling that is
/// not `HH:MM`, and for an hour or minute outside its range.
pub fn parse_time_of_day(input: &str, path: &str) -> Result<TimeOfDay, JsonError> {
    let invalid = || {
        JsonError::new(
            "time_of_day_invalid",
            format!("`{input}` is not a time of day written HH:MM"),
            path,
        )
    };
    let (hour, minute) = input.split_once(':').ok_or_else(invalid)?;
    if hour.len() != 2 || minute.len() != 2 || !is_digits(hour) || !is_digits(minute) {
        return Err(invalid());
    }
    let hour: u8 = hour.parse().map_err(|_| invalid())?;
    let minute: u8 = minute.parse().map_err(|_| invalid())?;
    TimeOfDay::new(hour, minute).map_err(|err| JsonError::from_domain(err.into(), path))
}

/// Parses the end of a wall-clock range, which may be `24:00`.
///
/// ```
/// use opportune_json::primitives::parse_wall_end;
///
/// assert_eq!(parse_wall_end("24:00", "/end").unwrap().to_string(), "24:00");
/// assert_eq!(parse_wall_end("17:00", "/end").unwrap().to_string(), "17:00");
/// ```
///
/// # Errors
/// Returns [`JsonError`] with code `time_of_day_invalid` for a spelling that is
/// neither a time of day nor `24:00`.
pub fn parse_wall_end(input: &str, path: &str) -> Result<WallEnd, JsonError> {
    if input == "24:00" {
        return Ok(WallEnd::Midnight);
    }
    parse_time_of_day(input, path).map(WallEnd::Time)
}

/// Parses a half-open wall-clock range written `start-end`, such as `09:00-17:00`.
///
/// An end earlier than the start crosses midnight, and `24:00` is a valid end.
/// A range whose start equals its end is rejected: `00:00-00:00` is almost
/// certainly a mistyped full day.
///
/// ```
/// use opportune_json::primitives::parse_wall_range;
///
/// let workday = parse_wall_range("09:00-17:00", "/hours").unwrap();
/// assert_eq!(workday.start().to_string(), "09:00");
/// assert_eq!(workday.end().to_string(), "17:00");
/// let night = parse_wall_range("22:00-02:00", "/hours").unwrap();
/// assert_eq!(night.end().to_string(), "02:00");
/// assert_eq!(
///     parse_wall_range("00:00-00:00", "/hours").unwrap_err().code(),
///     "window_range_empty"
/// );
/// ```
///
/// # Errors
/// Returns [`JsonError`] with code `wall_range_invalid` for a spelling that is not
/// `start-end`, `time_of_day_invalid` for a malformed time, and
/// the code the [`WallRange`] constructor reports for an empty range.
pub fn parse_wall_range(input: &str, path: &str) -> Result<WallRange, JsonError> {
    let Some((start, end)) = input.split_once('-') else {
        return Err(JsonError::new(
            "wall_range_invalid",
            format!("`{input}` is not a wall-clock range written `start-end`"),
            path,
        ));
    };
    let start = parse_time_of_day(start, path)?;
    let end = parse_wall_end(end, path)?;
    WallRange::new(start, end).map_err(|err| JsonError::from_domain(err.into(), path))
}

/// Parses a civil date and time of day written `YYYY-MM-DDTHH:MM`.
///
/// A blackout span is written this way: a wall-clock reading in the blackout's
/// zone, with no offset, because the zone is named beside it.
///
/// ```
/// use opportune_json::primitives::parse_local_date_time;
///
/// let start = parse_local_date_time("2026-12-24T00:00", "/spans/0/0").unwrap();
/// assert_eq!(start.to_string(), "2026-12-24T00:00");
/// ```
///
/// # Errors
/// Returns [`JsonError`] with code `local_date_time_invalid` when the `T` is
/// missing, and the date or time parser's code for either half.
pub fn parse_local_date_time(input: &str, path: &str) -> Result<LocalDateTime, JsonError> {
    let Some((date, time)) = input.split_once('T') else {
        return Err(JsonError::new(
            "local_date_time_invalid",
            format!("`{input}` is not a date-time written YYYY-MM-DDTHH:MM"),
            path,
        ));
    };
    Ok(LocalDateTime::new(
        parse_date(date, path)?,
        parse_time_of_day(time, path)?,
    ))
}

/// Parses a weekday written as its three-letter lower-case name, `mon` to `sun`.
///
/// ```
/// use opportune_json::primitives::parse_weekday;
///
/// assert_eq!(parse_weekday("mon", "/week_starts").unwrap().to_string(), "mon");
/// assert_eq!(parse_weekday("Monday", "/week_starts").unwrap_err().code(), "weekday_invalid");
/// ```
///
/// # Errors
/// Returns [`JsonError`] with code `weekday_invalid` for any other spelling.
pub fn parse_weekday(input: &str, path: &str) -> Result<Weekday, JsonError> {
    match input {
        "mon" => Ok(Weekday::Monday),
        "tue" => Ok(Weekday::Tuesday),
        "wed" => Ok(Weekday::Wednesday),
        "thu" => Ok(Weekday::Thursday),
        "fri" => Ok(Weekday::Friday),
        "sat" => Ok(Weekday::Saturday),
        "sun" => Ok(Weekday::Sunday),
        _ => Err(JsonError::new(
            "weekday_invalid",
            format!("`{input}` is not a weekday written `mon` to `sun`"),
            path,
        )),
    }
}

/// Parses a set of weekdays written as a comma-separated list of days and
/// inclusive ranges, such as `mon`, `mon,wed`, `mon-fri` or `fri-mon`.
///
/// A range wraps the week when its last day comes before its first. A day may be
/// named more than once; the key it belongs to is a set.
///
/// ```
/// use opportune::time::Weekday;
/// use opportune_json::primitives::parse_weekdays;
///
/// let weekdays = parse_weekdays("mon-fri", "/weekly/mon-fri").unwrap();
/// assert!(weekdays.contains(Weekday::Friday));
/// let wrapping = parse_weekdays("fri-mon", "/weekly/fri-mon").unwrap();
/// assert!(wrapping.contains(Weekday::Sunday));
/// assert!(!wrapping.contains(Weekday::Tuesday));
/// ```
///
/// # Errors
/// Returns [`JsonError`] with code `weekdays_invalid` for an empty key, and
/// `weekday_invalid` for a day that is not one of `mon` to `sun`.
pub fn parse_weekdays(input: &str, path: &str) -> Result<Weekdays, JsonError> {
    if input.is_empty() {
        return Err(JsonError::new(
            "weekdays_invalid",
            "a day key must name at least one weekday",
            path,
        ));
    }
    let mut days = Vec::new();
    for item in input.split(',') {
        match item.split_once('-') {
            Some((first, last)) => {
                let first = parse_weekday(first, path)?;
                let last = parse_weekday(last, path)?;
                days.extend(Weekdays::range(first, last).iter());
            }
            None => days.push(parse_weekday(item, path)?),
        }
    }
    Ok(Weekdays::list(days))
}

/// Parses the weekdays a working calendar works, in the same notation as
/// [`parse_weekdays`].
///
/// ```
/// use opportune_json::primitives::parse_working_days;
///
/// let days = parse_working_days("mon,tue,wed,thu,fri", "/calendars/org/working_days").unwrap();
/// assert_eq!(days.iter().count(), 5);
/// ```
///
/// # Errors
/// Returns the same [`JsonError`] codes as [`parse_weekdays`].
pub fn parse_working_days(input: &str, path: &str) -> Result<WorkingDays, JsonError> {
    parse_weekdays(input, path)
}

/// The `true` selector: the wire spelling of "everything".
///
/// It serialises as `true` and accepts no other value. A selector that must
/// match nothing is written `{"not": true}`, so a bare `false` — which reads as
/// a predicate rather than a selector — is rejected instead of guessed at.
///
/// ```
/// use opportune_json::MatchAll;
///
/// assert_eq!(serde_json::to_string(&MatchAll).unwrap(), "true");
/// assert!(serde_json::from_str::<MatchAll>("true").is_ok());
/// assert!(serde_json::from_str::<MatchAll>("false").is_err());
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MatchAll;

impl Serialize for MatchAll {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bool(true)
    }
}

impl<'de> Deserialize<'de> for MatchAll {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_bool(MatchAllVisitor)
    }
}

/// Describes the selector as the constant `true`, which is the only value the
/// deserialiser above accepts.
#[cfg(feature = "schema")]
impl schemars::JsonSchema for MatchAll {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "MatchAll".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({ "const": true })
    }
}

/// Rejects every boolean but `true`.
struct MatchAllVisitor;

impl Visitor<'_> for MatchAllVisitor {
    type Value = MatchAll;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the selector `true`")
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
        if value {
            Ok(MatchAll)
        } else {
            Err(E::invalid_value(
                de::Unexpected::Bool(false),
                &FALSE_SELECTOR,
            ))
        }
    }
}

/// What `false` should have been written as, for the rejection message both
/// selector deserialisers give.
pub(crate) const FALSE_SELECTOR: &str =
    "`false` is not a selector: a selector that matches nothing is `{ \"not\": true }`";

/// The number of seconds in a day, which is exact in Unix time.
const SECONDS_PER_DAY: i64 = 86_400;

/// Splits a timestamp at its date/time separator, which RFC 3339 permits in
/// either case.
///
/// The date is `YYYY-MM-DD` in every timestamp this grammar accepts, so a
/// well-formed separator sits at byte ten and is read there in either case. A
/// string with no separator there is not necessarily date-shaped — it may not be a
/// timestamp at all — so the upper-case separator stays the split point, and the
/// date parser goes on reporting whatever it finds before it.
fn split_date_time(input: &str) -> Option<(&str, &str)> {
    let canonical = input
        .as_bytes()
        .get(10)
        .is_some_and(|byte| *byte == b'T' || *byte == b't');
    let at = if canonical { 10 } else { input.find('T')? };
    // Both separators are ASCII, so the byte after one starts the next character.
    Some((input.get(..at)?, input.get(at + 1..)?))
}

/// Splits the part of a timestamp after `T` into its clock and its offset.
///
/// Returns `None` when no offset is present, which makes the reading a wall
/// clock rather than an instant.
fn split_offset(input: &str) -> Option<(&str, i64)> {
    if let Some(clock) = input.strip_suffix('Z').or_else(|| input.strip_suffix('z')) {
        return Some((clock, 0));
    }
    let split = input.len().checked_sub("+00:00".len())?;
    // The offset is ASCII, so its start is a character boundary only when the
    // bytes in front of it are too; a clock holding a multi-byte character is
    // rejected rather than sliced through.
    if !input.is_char_boundary(split) {
        return None;
    }
    let (clock, offset) = input.split_at(split);
    let sign = match offset.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let (hour, minute) = offset.get(1..)?.split_once(':')?;
    let hour = number(hour, 23)?;
    let minute = number(minute, 59)?;
    Some((
        clock,
        sign * (i64::from(hour) * 3_600 + i64::from(minute) * 60),
    ))
}

/// Splits a clock reading into its seconds since midnight and its nanoseconds.
///
/// Returns `None` for anything that is not `HH:MM`, `HH:MM:SS` or
/// `HH:MM:SS.fff` with at most nine fraction digits.
fn parse_clock(input: &str) -> Option<(i64, u32)> {
    let mut parts = input.splitn(3, ':');
    let hour = number(parts.next()?, 23)?;
    let minute = number(parts.next()?, 59)?;
    let (second, nanos) = match parts.next() {
        None => (0, 0),
        Some(rest) => {
            let (second, fraction) = match rest.split_once('.') {
                Some((second, fraction)) => (second, Some(fraction)),
                None => (rest, None),
            };
            let second = number(second, 59)?;
            (second, parse_nanos(fraction)?)
        }
    };
    Some((
        i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second),
        nanos,
    ))
}

/// Parses up to nine fraction digits into nanoseconds, padding on the right.
fn parse_nanos(fraction: Option<&str>) -> Option<u32> {
    let Some(fraction) = fraction else {
        return Some(0);
    };
    if fraction.is_empty() || fraction.len() > 9 || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let value: u32 = fraction.parse().ok()?;
    let scale = 10u32.pow(u32::try_from(9 - fraction.len()).ok()?);
    value.checked_mul(scale)
}

/// Whether `input` is one or more ASCII digits, which the fixed-width date and
/// clock fields must be: the integer parser accepts a leading sign.
fn is_digits(input: &str) -> bool {
    !input.is_empty() && input.bytes().all(|byte| byte.is_ascii_digit())
}

/// Parses an unsigned decimal field no larger than `max`.
fn number(input: &str, max: u32) -> Option<u32> {
    if !is_digits(input) {
        return None;
    }
    let value: u32 = input.parse().ok()?;
    (value <= max).then_some(value)
}

/// The number of whole days from 1970-01-01 to `year-month-day`.
///
/// Howard Hinnant's `days_from_civil`, in exact integer arithmetic: the civil
/// calendar repeats every 400 years, so shifting the year to start in March makes
/// the month lengths a polynomial. It is exact for every year this layer accepts,
/// which the timestamp and date parsers have already bounded.
fn days_from_civil(year: i16, month: i8, day: i8) -> i64 {
    let (year, month, day) = (i64::from(year), i64::from(month), i64::from(day));
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use opportune::time::{Duration, Instant, LocalDateTime, TimeOfDay, WallEnd, Weekday};
    use serde_json::{from_str, to_string};

    use super::{
        InstantRounding, MatchAll, SECONDS_PER_DAY, days_from_civil, format_duration, parse_date,
        parse_date_range, parse_duration, parse_instant, parse_local_date_time, parse_time_of_day,
        parse_wall_end, parse_wall_range, parse_weekday, parse_weekdays, parse_working_days,
    };

    #[test]
    fn the_civil_day_count_is_the_unix_epoch_offset() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(1970, 3, 1), 59);
        assert_eq!(days_from_civil(1969, 12, 31), -1);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        // 2027-03-09T15:00Z, the reference instant used across these examples.
        assert_eq!(
            days_from_civil(2027, 3, 9) * SECONDS_PER_DAY + 15 * 3_600,
            1_804_604_400
        );
    }

    #[test]
    fn instants_read_every_supported_wire_spelling() {
        let exact = |input: &str| parse_instant(input, InstantRounding::Exact, "/now").unwrap();
        assert_eq!(exact("2027-03-09T15:00Z"), exact("2027-03-09T15:00:00Z"));
        assert_eq!(
            exact("2027-03-09T10:00:00-05:00").to_string(),
            "2027-03-09T15:00Z"
        );
        assert_eq!(
            parse_instant("2027-03-09T15:00:30.250Z", InstantRounding::Ceil, "/now")
                .unwrap()
                .to_string(),
            "2027-03-09T15:01Z"
        );
    }

    #[test]
    fn instants_round_in_the_direction_asked_for() {
        let ceil = |input: &str| parse_instant(input, InstantRounding::Ceil, "/now").unwrap();
        assert_eq!(
            ceil("2027-03-09T15:00:00.001Z").to_string(),
            "2027-03-09T15:01Z"
        );
        assert_eq!(
            ceil("2027-03-09T15:00:00Z").to_string(),
            "2027-03-09T15:00Z"
        );
        let floor = |input: &str| parse_instant(input, InstantRounding::Floor, "/span/0").unwrap();
        assert_eq!(
            floor("2027-03-09T15:00:59.999Z").to_string(),
            "2027-03-09T15:00Z"
        );
    }

    #[test]
    fn an_unaligned_instant_is_rejected_exactly_where_it_was_read() {
        for input in ["2027-03-09T15:00:30Z", "2027-03-09T15:00:00.001Z"] {
            let error =
                parse_instant(input, InstantRounding::Exact, "/goal/candidates/0").unwrap_err();
            assert_eq!(error.code(), "instant_not_aligned", "{input}");
            assert_eq!(error.path(), "/goal/candidates/0", "{input}");
        }
    }

    #[test]
    fn an_instant_outside_the_supported_range_or_without_an_offset_is_rejected() {
        for input in ["1974-12-31T23:59Z", "2100-01-01T00:01Z"] {
            let error = parse_instant(input, InstantRounding::Exact, "/now").unwrap_err();
            assert_eq!(error.code(), "instant_out_of_range", "{input}");
        }
        let error = parse_instant("2027-03-09T15:00", InstantRounding::Exact, "/now").unwrap_err();
        assert_eq!(error.code(), "instant_invalid");
        assert_eq!(error.path(), "/now");
        // A timestamp is a document-level reading, so a malformed one names the
        // field it was read from rather than a made-up nested location.
        assert!(
            parse_instant("not a timestamp", InstantRounding::Exact, "/now")
                .unwrap_err()
                .message()
                .contains("`not a timestamp`")
        );
    }

    #[test]
    fn a_timestamp_that_is_not_ascii_is_rejected_rather_than_panicking() {
        for input in [
            "2027-03-09T\u{e9}a\u{e9}\u{e9}",
            "2027-03-09T15:00+\u{e9}:00",
            "\u{e9}",
        ] {
            let error = parse_instant(input, InstantRounding::Exact, "/now").unwrap_err();
            assert_eq!(error.code(), "instant_invalid", "{input}");
        }
    }

    #[test]
    fn durations_are_hours_and_minutes() {
        for (input, minutes) in [("30m", 30), ("4h", 240), ("1h30m", 90), ("0m", 0)] {
            assert_eq!(
                parse_duration(input, "/duration").unwrap().minutes(),
                minutes,
                "{input}"
            );
        }
        assert_eq!(format_duration(Duration::from_minutes(30)), "30m");
        assert_eq!(format_duration(Duration::from_hours_minutes(1, 30)), "90m");
        assert_eq!(
            parse_duration(&format_duration(Duration::from_hours(4)), "/duration").unwrap(),
            Duration::from_hours(4)
        );
    }

    #[test]
    fn a_duration_that_names_days_is_rejected_with_its_own_code() {
        for input in ["1d", "1d2h"] {
            let error = parse_duration(input, "/rules/2/params/min").unwrap_err();
            assert_eq!(error.code(), "duration_contains_days", "{input}");
            assert_eq!(error.path(), "/rules/2/params/min");
        }
        for input in ["", "-5m", "90", "1h1h", "m", "1x"] {
            assert_eq!(
                parse_duration(input, "/duration").unwrap_err().code(),
                "duration_invalid",
                "{input}"
            );
        }
    }

    /// A count too large for a `u32` is its own sentence: the spelling is exactly
    /// the hours-and-minutes shape the parser asks for, so claiming otherwise sends
    /// a caller looking for a syntax mistake that is not there.
    #[test]
    fn a_duration_count_too_large_to_hold_says_so() {
        for input in ["4294967296h", "999999999999m", "1h999999999999m"] {
            let error = parse_duration(input, "/goal/parts/0/duration").unwrap_err();
            assert_eq!(error.code(), "duration_invalid", "{input}");
            assert_eq!(error.path(), "/goal/parts/0/duration", "{input}");
            assert!(
                error
                    .message()
                    .contains("more hours or minutes than a duration can hold"),
                "{input}: {error}",
            );
            assert!(
                !error
                    .message()
                    .contains("is not a duration written in hours and minutes"),
                "{input} is written in hours and minutes: {error}",
            );
        }
        // The largest count a component can hold still reads, so the sentence
        // names the representable edge rather than the first big number.
        assert!(parse_duration("4294967295m", "/duration").is_ok());
    }

    #[test]
    fn wall_ranges_cross_midnight_and_reject_empty_ones() {
        for input in ["09:00-17:00", "22:00-02:00", "00:00-24:00"] {
            assert!(parse_wall_range(input, "/hours").is_ok(), "{input}");
        }
        let error = parse_wall_range("00:00-00:00", "/hours").unwrap_err();
        assert_eq!(error.code(), "window_range_empty");
        assert_eq!(
            parse_wall_range("09:00", "/hours").unwrap_err().code(),
            "wall_range_invalid"
        );
        assert_eq!(
            parse_wall_range("24:00-17:00", "/hours")
                .unwrap_err()
                .code(),
            "time_of_day_invalid"
        );
    }

    #[test]
    fn weekdays_read_lists_and_ranges_that_wrap() {
        assert_eq!(
            parse_weekdays("mon", "/weekly").unwrap(),
            opportune::time::Weekdays::single(Weekday::Monday)
        );
        assert_eq!(
            parse_weekdays("mon,wed", "/weekly").unwrap(),
            opportune::time::Weekdays::list([Weekday::Monday, Weekday::Wednesday])
        );
        assert_eq!(
            parse_weekdays("mon-fri", "/weekly").unwrap(),
            opportune::time::Weekdays::weekdays()
        );
        let wrapping = parse_weekdays("fri-mon", "/weekly").unwrap();
        assert!(wrapping.contains(Weekday::Friday));
        assert!(wrapping.contains(Weekday::Monday));
        assert!(!wrapping.contains(Weekday::Tuesday));
        assert_eq!(
            parse_working_days("sat,sun", "/working_days")
                .unwrap()
                .iter()
                .count(),
            2
        );
    }

    #[test]
    fn a_weekday_key_that_names_no_day_or_an_unknown_day_is_rejected() {
        for input in ["", "mon,", "funday"] {
            assert!(parse_weekdays(input, "/weekly").is_err(), "{input}");
        }
        let error = parse_weekday("Funday", "/week_starts").unwrap_err();
        assert_eq!(error.code(), "weekday_invalid");
        assert_eq!(error.path(), "/week_starts");
    }

    #[test]
    fn dates_and_date_ranges_follow_the_calendar() {
        assert_eq!(
            parse_date("2027-03-08", "/dates").unwrap().to_string(),
            "2027-03-08"
        );
        assert_eq!(
            parse_date_range("2027-03-08", "/dates")
                .unwrap()
                .start()
                .to_string(),
            "2027-03-08"
        );
        let range = parse_date_range("2027-03-08..2027-03-10", "/dates").unwrap();
        assert_eq!(
            (range.start().to_string(), range.end().to_string()).0,
            "2027-03-08"
        );
        assert_eq!(range.end().to_string(), "2027-03-10");
        assert!(parse_date_range("2028-02-29", "/dates").is_ok());
        assert_eq!(
            parse_date_range("2027-02-29", "/dates").unwrap_err().code(),
            "date_invalid"
        );
        assert_eq!(
            parse_date_range("2027-03-10..2027-03-08", "/dates")
                .unwrap_err()
                .code(),
            "window_date_range_reversed"
        );
    }

    #[test]
    fn times_and_local_date_times_read_their_wire_spelling() {
        assert_eq!(
            parse_time_of_day("09:00", "/anchor").unwrap(),
            TimeOfDay::new(9, 0).unwrap()
        );
        assert_eq!(parse_wall_end("24:00", "/end").unwrap(), WallEnd::Midnight);
        assert_eq!(
            parse_local_date_time("2026-12-24T00:00", "/spans/0/0").unwrap(),
            LocalDateTime::new(
                opportune::time::Date::new(2026, 12, 24).unwrap(),
                TimeOfDay::MIDNIGHT
            )
        );
        assert_eq!(
            parse_local_date_time("2026-12-24", "/spans/0/0")
                .unwrap_err()
                .code(),
            "local_date_time_invalid"
        );
    }

    #[test]
    fn match_all_accepts_only_true() {
        assert_eq!(to_string(&MatchAll).unwrap(), "true");
        assert_eq!(from_str::<MatchAll>("true").unwrap(), MatchAll);
        let error = from_str::<MatchAll>("false").unwrap_err();
        assert!(
            error.to_string().contains("`false` is not a selector"),
            "{error}"
        );
        assert!(from_str::<MatchAll>("\"true\"").is_err());
    }

    #[test]
    fn an_instant_parses_to_the_round_trip_spelling() {
        let instant = parse_instant("2027-03-09T15:00Z", InstantRounding::Exact, "/now").unwrap();
        assert_eq!(instant, Instant::from_unix_seconds(1_804_604_400).unwrap());
        assert_eq!(
            parse_instant(&instant.to_string(), InstantRounding::Exact, "/now").unwrap(),
            instant
        );
    }
}
