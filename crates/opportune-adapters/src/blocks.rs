//! What a provider conversion builds a block out of: a resolved span, the finding
//! attached to one event, and the rounding rules a block's endpoints obey.
//!
//! Different providers spell the same things differently — Graph writes a
//! wall-clock reading and a zone name, Google writes an RFC 3339 instant — but a
//! provider timestamp becomes a block the same way: resolve the raw reading, drop
//! an event whose endpoints coincide *before* rounding, then round the start down
//! and the end up to whole minutes. Sharing that here is what keeps a zero-length
//! reminder from widening into a one-minute block in one adapter and not the
//! other.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;

use opportune::model::Span;
use opportune::tags::Tag;
use opportune::time::{Date, Instant, TimeOfDay, TimeZoneDatabase, Zone};

use crate::types::{AdapterDiagnostic, AdapterZoneError, CalendarOutcome};
use crate::zones::resolve_zone;

/// Why one event, or the calendar holding it, cannot convert: the stable code
/// and the sentence beside it.
pub(crate) type Failure = (&'static str, String);

/// A timestamp resolved to Unix seconds and nanoseconds, before minute rounding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Resolved {
    pub(crate) seconds: i64,
    pub(crate) nanos: u32,
}

impl Resolved {
    /// A minute-aligned instant, as the civil-date resolution returns one.
    pub(crate) fn of(instant: Instant) -> Self {
        Self {
            seconds: instant.unix_seconds(),
            nanos: 0,
        }
    }
}

/// Drops a zero-length event before rounding the rest outward to whole minutes.
pub(crate) fn finish_span(
    start: Resolved,
    end: Resolved,
    subject: Subject<'_>,
    diagnostics: &mut Vec<AdapterDiagnostic>,
) -> Result<Option<Span>, Failure> {
    let label = subject.label;
    if start == end {
        diagnostics.push(subject.diagnostic(
            "zero_length_event",
            format!(
                "the event `{label}` starts and ends at the same instant, so it occupies no time"
            ),
        ));
        return Ok(None);
    }
    let start = Instant::floor_to_minute_nanos(start.seconds, start.nanos)
        .map_err(|err| out_of_range(label, &err))?;
    let end = Instant::ceil_to_minute_nanos(end.seconds, end.nanos)
        .map_err(|err| out_of_range(label, &err))?;
    Span::new(start, end)
        .map(Some)
        .map_err(|err| ("payload_invalid", format!("the event `{label}`: {err}")))
}

/// What a converted span belongs to: a label for the sentences, and the
/// provider's event ID when the source reports events rather than spans.
#[derive(Clone, Copy)]
pub(crate) struct Subject<'a> {
    pub(crate) label: &'a str,
    pub(crate) event_id: Option<&'a str>,
}

impl Subject<'_> {
    /// The diagnostic for this subject, carrying the provider's ID only when the
    /// source has one: a free/busy item's position is not an event ID.
    pub(crate) fn diagnostic(
        &self,
        code: &'static str,
        message: impl Into<String>,
    ) -> AdapterDiagnostic {
        match self.event_id {
            Some(event_id) => AdapterDiagnostic::for_event(code, event_id, message),
            None => AdapterDiagnostic::for_span(code, message),
        }
    }
}

/// The outcome of a calendar that cannot convert.
pub(crate) fn unavailable(code: &'static str, message: impl Into<String>) -> CalendarOutcome {
    CalendarOutcome::Unavailable {
        code,
        message: message.into(),
    }
}

/// A tag path an adapter names itself.
///
/// The paths are compile-time literals, so an invalid one is a defect in this
/// crate rather than a payload failure, and failing here is louder than dropping
/// a status tag a caller's policy would have matched.
pub(crate) fn own_tag(path: &'static str) -> Tag {
    Tag::new(path).expect("the adapter's own tag paths are valid")
}

/// The tags in `tags`, in first-seen order, with duplicates removed.
///
/// A provider repeats a tag when a status, an event type, a category and a
/// marker name the same path, so the dedupe has to happen somewhere; a
/// `BTreeSet` keeps the check logarithmic without pulling in a hasher.
pub(crate) fn unique_tags(tags: impl IntoIterator<Item = Tag>) -> Vec<Tag> {
    let mut seen = std::collections::BTreeSet::new();
    let mut unique = Vec::new();
    for tag in tags {
        if seen.insert(tag.clone()) {
            unique.push(tag);
        }
    }
    unique
}

/// An event whose endpoints resolve outside the supported instant range.
fn out_of_range(label: &str, err: &opportune::time::TimeError) -> Failure {
    (
        "payload_invalid",
        format!("the event `{label}` lies outside the supported instant range: {err}"),
    )
}

/// The digits `field` promises, or the error `invalid` builds.
fn number<'a>(field: Option<&'a str>, invalid: &impl Fn() -> String) -> Result<&'a str, String> {
    match field {
        Some(field) if !field.is_empty() && field.bytes().all(|byte| byte.is_ascii_digit()) => {
            Ok(field)
        }
        _ => Err(invalid()),
    }
}

/// The nanoseconds a fractional-second field names: one to nine digits, read to
/// the left.
fn parse_nanos(fraction: &str) -> Option<u32> {
    if fraction.is_empty() || fraction.len() > 9 || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut value = fraction.parse::<u32>().ok()?;
    for _ in fraction.len()..9 {
        value *= 10;
    }
    Some(value)
}

/// Reads `HH:MM[:SS[.fraction]]`, rejecting signed numbers, extra separators,
/// and out-of-range fields.
pub(crate) fn parse_clock(
    raw: &str,
    whole: &str,
    invalid: &impl Fn() -> String,
) -> Result<(TimeOfDay, u8, u32), String> {
    let mut fields = raw.split(':');
    let hour = number(fields.next(), invalid)?.parse::<u8>();
    let minute = number(fields.next(), invalid)?.parse::<u8>();
    let (second, nanos) = match fields.next() {
        None => (Ok(0), 0),
        Some(rest) => {
            let (second_field, fraction) = match rest.split_once('.') {
                Some((second, fraction)) => (second, Some(fraction)),
                None => (rest, None),
            };
            let second = number(Some(second_field), invalid)?.parse::<u8>();
            let nanos = match fraction {
                Some(fraction) => parse_nanos(fraction).ok_or_else(invalid)?,
                None => 0,
            };
            (second, nanos)
        }
    };
    if fields.next().is_some() {
        return Err(invalid());
    }
    let (Ok(hour), Ok(minute), Ok(second)) = (hour, minute, second) else {
        return Err(invalid());
    };
    let time = TimeOfDay::new(hour, minute).map_err(|err| format!("`{whole}`: {err}"))?;
    if second >= 60 {
        return Err(format!("`{whole}` has a second outside a minute"));
    }
    Ok((time, second, nanos))
}

/// Reads a `YYYY-MM-DD` civil date.
pub(crate) fn parse_date(raw: &str) -> Result<Date, String> {
    let invalid = || format!("`{raw}` is not a civil date");
    let mut fields = raw.split('-');
    let year = number(fields.next(), &invalid)?.parse::<i16>();
    let month = number(fields.next(), &invalid)?.parse::<i8>();
    let day = number(fields.next(), &invalid)?.parse::<i8>();
    if fields.next().is_some() {
        return Err(invalid());
    }
    let (Ok(year), Ok(month), Ok(day)) = (year, month, day) else {
        return Err(invalid());
    };
    Date::new(year, month, day).map_err(|err| format!("`{raw}`: {err}"))
}

/// A Graph wall-clock reading: a civil date and time with the sub-minute fields
/// Graph appends, none of which carries an offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LocalStamp {
    pub(crate) date: Date,
    time: TimeOfDay,
    second: u8,
    nanos: u32,
}

impl LocalStamp {
    /// Reads `YYYY-MM-DDTHH:MM[:SS[.fraction]]`, the shape Graph stamps on
    /// `dateTime`, and the shape a Google `dateTime` takes when its offset was
    /// left to the sibling `timeZone`.
    pub(crate) fn parse(raw: &str) -> Result<Self, String> {
        let invalid = || format!("`{raw}` is not a date-time with a wall clock");
        let (date, clock) = raw.split_once(['T', 't']).ok_or_else(invalid)?;
        let date = parse_date(date)?;
        let (time, second, nanos) = parse_clock(clock, raw, &invalid)?;
        Ok(Self {
            date,
            time,
            second,
            nanos,
        })
    }

    /// The instant this reading names in `zone`, with the compatible
    /// disambiguation a repeated or skipped local time needs.
    pub(crate) fn resolve(&self, zone: &Zone) -> Result<Resolved, String> {
        let seconds = zone
            .resolve_local_compatible(self.date, self.time, self.second)
            .map_err(|err| format!("`{self}` in `{}`: {err}", zone.name()))?;
        Ok(Resolved {
            seconds,
            nanos: self.nanos,
        })
    }
}

impl core::fmt::Display for LocalStamp {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(formatter, "{}T{}:{:02}", self.date, self.time, self.second)
    }
}

/// The number of seconds in a day, exact in Unix time.
const SECONDS_PER_DAY: i64 = 86_400;

/// Whole days from `1970-01-01` to `year-month-day` (Howard Hinnant's
/// `days_from_civil`), exact for every valid Gregorian [`Date`].
fn days_from_civil(year: i16, month: i8, day: i8) -> i64 {
    let (year, month, day) = (i64::from(year), i64::from(month), i64::from(day));
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// A timestamp written as an ISO 8601 instant that carries its offset: Google
/// events stamp `dateTime` with one, and Graph's `originalStart` is a UTC
/// `DateTimeOffset`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Stamp {
    pub(crate) seconds: i64,
    pub(crate) nanos: u32,
}

impl Stamp {
    /// Reads `YYYY-MM-DDTHH:MM[:SS[.fraction]]` followed by `Z` or `±HH:MM`.
    ///
    /// Google writes RFC 3339 without the second when it is zero. Unix seconds
    /// are computed from the civil date, clock and offset before range-checking
    /// the resulting UTC instant, so a valid boundary instant written in a
    /// non-UTC offset (such as `1974-12-31T19:00:00-05:00` or
    /// `2100-01-01T09:00:00+09:00`) resolves accurately.
    ///
    /// An instant with no offset is not read here: Google's API contract allows
    /// one when a sibling `timeZone` carries the zone, so the caller resolves the
    /// wall clock instead (see [`Stamp::has_offset`]).
    pub(crate) fn parse(raw: &str) -> Result<Self, String> {
        let invalid = || format!("`{raw}` is not an ISO 8601 timestamp with an offset");
        let (date, rest) = raw.split_once(['T', 't']).ok_or_else(invalid)?;
        let date = parse_date(date)?;
        let (clock, offset_seconds) = split_offset(rest).ok_or_else(invalid)?;
        let (time, second, nanos) = parse_clock(clock, raw, &invalid)?;
        let unix_seconds = days_from_civil(date.year(), date.month(), date.day()) * SECONDS_PER_DAY
            + i64::from(time.minute_of_day()) * 60
            + i64::from(second)
            - offset_seconds;
        Ok(Self {
            seconds: unix_seconds,
            nanos,
        })
    }

    /// Whether `raw` names an instant whose offset is written out, so
    /// [`Stamp::parse`] is the reader for it.
    pub(crate) fn has_offset(raw: &str) -> bool {
        raw.split_once(['T', 't'])
            .and_then(|(_, rest)| split_offset(rest))
            .is_some()
    }

    /// The instant written in UTC, which is what makes two attendees' copies of
    /// one recurring instance the same event key.
    pub(crate) fn utc_key(self) -> Result<String, String> {
        let minute = Instant::floor_to_minute_nanos(self.seconds, 0)
            .map_err(|err| format!("outside the supported instant range: {err}"))?;
        let zoned = Zone::utc().zoned(minute);
        let second = self.seconds.rem_euclid(60);
        let fraction = match self.nanos {
            0 => String::new(),
            nanos => {
                let digits = format!("{nanos:09}");
                format!(".{}", digits.trim_end_matches('0'))
            }
        };
        Ok(format!(
            "{}T{}:{second:02}{fraction}Z",
            zoned.date(),
            zoned.time()
        ))
    }
}

/// Splits the part after `T` into its clock and the seconds its offset is east of
/// UTC.
fn split_offset(rest: &str) -> Option<(&str, i64)> {
    if let Some(clock) = rest.strip_suffix('Z').or_else(|| rest.strip_suffix('z')) {
        return Some((clock, 0));
    }
    let split = rest.len().checked_sub("+00:00".len())?;
    // The offset is ASCII, so its start is a character boundary only when the
    // bytes in front of it are too; a clock holding a multi-byte character is
    // rejected rather than sliced through.
    if !rest.is_char_boundary(split) {
        return None;
    }
    let (clock, offset) = rest.split_at(split);
    let sign = match offset.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let (hour, minute) = offset.get(1..)?.split_once(':')?;
    let hour = bounded(hour, 23)?;
    let minute = bounded(minute, 59)?;
    Some((
        clock,
        sign * (i64::from(hour) * 3_600 + i64::from(minute) * 60),
    ))
}

/// The value `field` names, when it is one or two digits no larger than `max`.
fn bounded(field: &str, max: u8) -> Option<u8> {
    if field.is_empty() || field.len() > 2 || !field.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    field.parse::<u8>().ok().filter(|value| *value <= max)
}

/// The zones one payload names, each resolved at most once for the conversion.
///
/// A response names a zone per timestamp endpoint, and a Windows name costs a
/// CLDR scan to resolve. Resolving the same name once per payload keeps a large
/// response proportional to the events it holds rather than to the events times
/// the size of the CLDR table, and the cache is dropped when the conversion
/// returns — nothing outside this call sees it.
pub(crate) struct ZoneCache<'a> {
    db: &'a TimeZoneDatabase,
    zones: BTreeMap<String, Zone>,
}

impl<'a> ZoneCache<'a> {
    pub(crate) fn new(db: &'a TimeZoneDatabase) -> Self {
        Self {
            db,
            zones: BTreeMap::new(),
        }
    }

    /// The zone `name` resolves to, resolving it only the first time.
    pub(crate) fn resolve(&mut self, name: &str) -> Result<&Zone, AdapterZoneError> {
        let trimmed = name.trim();
        let zone = match self.zones.entry(trimmed.to_owned()) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let resolved = resolve_zone(self.db, trimmed)?;
                entry.insert(resolved)
            }
        };
        Ok(zone)
    }
}
