//! Google Calendar payloads: `events.list` events and `freeBusy.query` ranges.
//!
//! Two conversions:
//!
//! - [`events_list_to_blocks`] converts one calendar's `events.list` response into
//!   blocks, with each event's status, `eventType` tag, marker tags and event key.
//! - [`free_busy_to_blocks`] converts a `freeBusy.query` response into one outcome
//!   per calendar ID. A calendar the provider reported an `errors` entry for comes
//!   back [`CalendarOutcome::Unavailable`] with `provider_error`, while the healthy
//!   calendars in the same response keep their blocks. Free/busy carries less than
//!   events do — no event ID, no tags and no all-day flag — so its blocks have no
//!   event key and are never `all_day`; rest rules and category-based caps need
//!   `events.list`.
//!
//! # Empty collections
//!
//! Google omits an empty repeated field rather than writing `[]`, so a window with
//! no events is an `events.list` collection with no `items` key, and a calendar
//! with nothing busy in the window is a `freeBusy.query` entry with no `busy` key.
//! Both read as a free calendar: a participant with nothing in the window is the
//! ordinary case for an availability search. A document carrying neither the field
//! nor Google's own `kind` is not the response the adapter asked for — a provider
//! error body, say — and fails rather than reading as free time.
//!
//! # Times
//!
//! Google timed events usually carry an offset (`2027-03-09T10:00:00-05:00`), so
//! a timed span is an instant and needs no zone name: the offsets may differ
//! between two attendees' copies of the same event. The API also permits a
//! `dateTime` with no offset when the sibling `timeZone` names the zone, so such
//! a reading is resolved in that zone, then in the response's `timeZone`, then in
//! the participant's. All-day events carry civil dates instead —
//! `start.date`, with an exclusive `end.date` — and are read from those dates in
//! the response's `timeZone`, falling back to the participant's zone, whatever
//! the participant's own zone is. A name nothing resolves fails the calendar with
//! `zone_unknown` rather than reading the day in the wrong zone.
//!
//! An event whose start and end resolve to the same instant is dropped with a
//! `zero_length_event` diagnostic *before* the endpoints are rounded outward,
//! which would otherwise turn a zero-length reminder at `10:00:30` into the
//! one-minute block `[10:00, 10:01)`.
//!
//! # Status
//!
//! Applied in this order, which is the order the adapter contract fixes:
//!
//! 1. `status: "cancelled"` is dropped, before anything reads its times, because a
//!    cancelled event may carry none.
//! 2. The user's own attendee entry (`self: true`) with a `declined` response is
//!    dropped.
//! 3. A non-empty `recurrence` fails the calendar with `unexpanded_series`: the
//!    caller queried the wrong endpoint, so the series is not expanded.
//! 4. `transparency: "transparent"` is free — checked before the omitted attendee
//!    list and before the user's own tentative answer, so a transparent event never
//!    blocks.
//! 5. A `birthday` or `workingLocation` event with no `transparency` at all is free
//!    too: Google writes those informational all-day markers as `transparent`, and
//!    reading an omitted field as the `opaque` default would block the whole day.
//!    An explicit `"opaque"` on one still blocks, because the field decides.
//! 6. The user's own `tentative` response is tentative; `needsAction` (an
//!    unanswered invitation) is tentative tagged `status/needs-action`; otherwise
//!    busy. An event with `attendeesOmitted` always reports an
//!    `attendees_omitted` diagnostic, because Google omitted the entry that would
//!    say how the user answered, and reads as busy unless it is transparent.
//!
//! Every event also gets a tag from its `eventType`: `outOfOffice` → `status/oof`,
//! `focusTime` → `status/focus`, `workingLocation` → `status/working-location`,
//! `birthday` → `status/birthday`, `fromGmail` → `status/from-gmail`, and `default`
//! (or a type Google adds later) none. Tags come back in the order status, event
//! type, markers, with duplicates removed.
//!
//! Google's own `status: "tentative"` is the organiser's reading of the event and
//! does not soften the block: only the user's own answer says whether they hold
//! the time, so an event nobody declined blocks.
//!
//! # Event keys
//!
//! The caller's marker wins, then `iCalUID`. All instances of a recurring series
//! share one `iCalUID` and one set of shared extended properties, so a recurring
//! instance's key also carries its `originalStartTime`, normalised to UTC first:
//! two attendees whose copies carry `2027-03-09T10:00:00-05:00` and
//! `2027-03-10T00:00:00+09:00` derive the same key, and distinct occurrences
//! derive distinct ones. A marker written once on a series would otherwise merge
//! every occurrence into one event, so it takes the same discriminator.
//!
//! # Markers
//!
//! `opportune.tags` and `opportune.event_key` are read from
//! `extendedProperties.shared`, which every attendee's copy of an event carries,
//! and from `extendedProperties.private` as well. `shared` wins for the event key,
//! and `opportune.tags` merges both lists with `shared` first.
//!
//! # Caller requirements
//!
//! - Pass `singleEvents=true` to `events.list`, so Google expands each recurring
//!   series into the instances inside the window. The conversion rejects a payload
//!   that still carries a `recurrence` rather than treating one master event as the
//!   series.
//! - Never set `maxAttendees`: Google then omits attendees, the user's own entry
//!   among them, so the event can no longer be read as anything but busy.
//! - Follow `nextPageToken` until it is absent and pass the concatenated `items`
//!   array as one payload, keeping the response's `timeZone` with it: without that
//!   zone an all-day event is read in the participant's zone instead, which moves
//!   it by hours whenever the two differ. A single page is only part of the window,
//!   and converting it alone would read the events on the later pages as free time.
//! - Batch `freeBusy.query` within Google's per-call calendar limit (50 calendars in
//!   one request), and split a longer window into several queries.
//!
//! # Example
//!
//! ```
//! # #[cfg(feature = "bundled-tzdb")] {
//! use opportune::time::{TimeZoneDatabase, ZoneName};
//! use opportune_adapters::CalendarOutcome;
//! use opportune_adapters::google::events_list_to_blocks;
//!
//! // The two copies of one recurring instance are stamped in different offsets.
//! let payload = r#"{
//!   "timeZone": "America/New_York",
//!   "items": [{
//!     "id": "evt-instance",
//!     "status": "confirmed",
//!     "transparency": "opaque",
//!     "iCalUID": "series@example.com",
//!     "originalStartTime": { "dateTime": "2027-03-10T00:00:00+09:00" },
//!     "start": { "dateTime": "2027-03-09T10:00:00-05:00" },
//!     "end": { "dateTime": "2027-03-09T11:00:00-05:00" }
//!   }]
//! }"#;
//!
//! let db = TimeZoneDatabase::bundled().unwrap();
//! let zone = ZoneName::new("America/New_York").unwrap();
//! let CalendarOutcome::Available { blocks, diagnostics } = events_list_to_blocks(&db, payload, &zone)
//! else {
//!     panic!("the payload converts");
//! };
//! assert!(diagnostics.is_empty());
//! assert_eq!(blocks[0].block.span.start().to_string(), "2027-03-09T15:00Z");
//! assert_eq!(
//!     blocks[0].block.event_key.as_deref(),
//!     Some("series@example.com@2027-03-09T15:00:00Z")
//! );
//! # }
//! ```

use std::collections::BTreeMap;

use opportune::model::{Block, BlockStatus, Span};
use opportune::tags::Tag;
use opportune::time::{TimeZoneDatabase, ZoneName};
use serde::Deserialize;

use crate::blocks::{
    Failure, LocalStamp, Resolved, Stamp, Subject, ZoneCache, finish_span, own_tag, parse_date,
    unavailable, unique_tags,
};
use crate::types::{
    AdapterDiagnostic, AdapterPayloadError, AnnotatedBlock, CalendarOutcome, MARKER_EVENT_KEY,
    MARKER_TAGS,
};
use crate::zones::parse_marker_tags;

/// The status tag an unanswered invitation carries.
const TAG_NEEDS_ACTION: &str = "status/needs-action";

/// The `kind` Google stamps on an `events.list` collection.
const EVENT_COLLECTION: &str = "calendar#events";

/// The tag each Google `eventType` carries.
const EVENT_TYPE_TAGS: [(&str, &str); 5] = [
    ("outOfOffice", "status/oof"),
    ("focusTime", "status/focus"),
    ("workingLocation", "status/working-location"),
    ("birthday", "status/birthday"),
    ("fromGmail", "status/from-gmail"),
];

/// Converts one calendar's `events.list` response into blocks.
///
/// The participant's zone is what an all-day event is read in when the response
/// names no `timeZone` of its own, and it is the last fallback for a `dateTime`
/// whose offset was left to the sibling `timeZone`. A payload that is not an
/// `events.list` response, one page of a paginated response, an unresolvable zone
/// (when an event needs it), an unexpanded series, or an event that cannot become
/// a block fails the whole calendar: answering with the events that did convert
/// would read as free time over the ones that did not. A `calendar#events`
/// collection with no `items` key is a calendar with nothing in the window, not a
/// failure.
#[must_use]
pub fn events_list_to_blocks(
    db: &TimeZoneDatabase,
    payload_json: &str,
    participant_zone: &ZoneName,
) -> CalendarOutcome {
    let response = match serde_json::from_str::<EventsResponse>(payload_json) {
        Ok(response) => response,
        Err(err) => {
            return unavailable(
                "payload_invalid",
                format!("the events.list payload is not readable: {err}"),
            );
        }
    };
    // A page is not the response. The events after `nextPageToken` are missing
    // from the document entirely, so converting the page alone would read them as
    // free time.
    if response
        .next_page_token
        .as_deref()
        .is_some_and(|token| !token.trim().is_empty())
    {
        return unavailable(
            "payload_invalid",
            "the events.list payload is one page of a paginated response, so it is missing the \
             events on the pages after it: follow `nextPageToken` and pass the concatenated \
             `items` array as one payload, keeping the response's `timeZone` with it"
                .to_owned(),
        );
    }
    // Google omits an empty repeated field, so a window with no events is a
    // collection of its own kind with no `items` key. That `kind` is what keeps a
    // document which is not the collection at all from reading as free time.
    let items = match (response.items, response.kind.as_deref()) {
        (Some(items), _) => items,
        (None, Some(EVENT_COLLECTION)) => Vec::new(),
        (None, _) => {
            return unavailable(
                "payload_invalid",
                "the events.list payload is not readable: it carries no `items` array and is not \
                 a `calendar#events` collection"
                    .to_owned(),
            );
        }
    };
    // A participant zone is only read for an all-day event with no response
    // `timeZone` of its own, or for a reading whose offset was left to its zone,
    // so it is resolved when one needs it: a timed calendar converts even when
    // the participant name is unusable.
    let participant = participant_zone.as_str();
    let mut zones = ZoneCache::new(db);
    let mut blocks = Vec::new();
    let mut diagnostics = Vec::new();
    for event in items {
        let outcome = convert_event(
            &mut zones,
            participant,
            response.time_zone.as_deref(),
            event,
            &mut diagnostics,
        );
        match outcome {
            Ok(Some(block)) => blocks.push(block),
            Ok(None) => {}
            Err((code, message)) => return unavailable(code, message),
        }
    }
    CalendarOutcome::Available {
        blocks,
        diagnostics,
    }
}

/// Converts a `freeBusy.query` response into one outcome per calendar ID.
///
/// A calendar Google reported an `errors` entry for comes back
/// [`CalendarOutcome::Unavailable`] with `provider_error`, while the healthy
/// calendars in the same response keep their blocks. Every busy range becomes a
/// busy block with no event key and no all-day flag, and a range whose endpoints
/// coincide is dropped with a `zero_length_event` diagnostic. A calendar with
/// nothing busy in the window carries no `busy` key — Google omits an empty
/// repeated field — and converts to no blocks.
///
/// ```
/// use opportune_adapters::CalendarOutcome;
/// use opportune_adapters::google::free_busy_to_blocks;
///
/// let payload = r#"{
///   "calendars": {
///     "ana@example.com": {
///       "busy": [{ "start": "2027-03-09T10:00:00-05:00", "end": "2027-03-09T11:00:00-05:00" }]
///     },
///     "private@example.com": {
///       "errors": [{ "domain": "global", "reason": "forbidden" }]
///     }
///   }
/// }"#;
///
/// let outcomes = free_busy_to_blocks(payload).unwrap();
/// let CalendarOutcome::Available { blocks, .. } = &outcomes["ana@example.com"] else {
///     panic!("the healthy calendar converts");
/// };
/// assert_eq!(blocks[0].block.span.start().to_string(), "2027-03-09T15:00Z");
/// assert!(blocks[0].block.event_key.is_none());
/// assert!(matches!(
///     outcomes["private@example.com"],
///     CalendarOutcome::Unavailable { code: "provider_error", .. }
/// ));
/// ```
///
/// # Errors
/// Returns [`AdapterPayloadError::InvalidPayload`] when the document is not a
/// `freeBusy.query` response this adapter can read. A calendar Google answered for
/// is never an error: its entry carries either the errors it reported or the busy
/// ranges it found, and no `busy` key means it found none.
pub fn free_busy_to_blocks(
    payload_json: &str,
) -> Result<BTreeMap<String, CalendarOutcome>, AdapterPayloadError> {
    let response: FreeBusyResponse = serde_json::from_str(payload_json)
        .map_err(|err| AdapterPayloadError::invalid("freeBusy.query", err.to_string()))?;
    let mut outcomes = BTreeMap::new();
    for (calendar, entry) in response.calendars {
        outcomes.insert(calendar, free_busy_calendar(entry));
    }
    Ok(outcomes)
}

/// One calendar's entry in a `freeBusy.query` response.
#[derive(Debug, Deserialize)]
struct FreeBusyResponse {
    calendars: BTreeMap<String, FreeBusyCalendar>,
}

/// One calendar's busy ranges, or the errors Google reported for it.
#[derive(Debug, Deserialize)]
struct FreeBusyCalendar {
    busy: Option<Vec<FreeBusyRange>>,
    errors: Option<Vec<FreeBusyError>>,
}

/// One busy range: two RFC 3339 instants with no event identity.
#[derive(Debug, Deserialize)]
struct FreeBusyRange {
    start: String,
    end: String,
}

/// The `freeBusyError` Google reports in place of a calendar's ranges.
#[derive(Debug, Deserialize)]
struct FreeBusyError {
    domain: Option<String>,
    reason: Option<String>,
}

/// An `events.list` response's `items` collection.
///
/// `items` is optional because Google omits an empty repeated field: the calendar
/// with no events in the window carries no `items` key. [`EVENT_COLLECTION`] is
/// the discriminator that tells that collection from a document which is not one.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventsResponse {
    time_zone: Option<String>,
    kind: Option<String>,
    items: Option<Vec<GoogleEvent>>,
    /// Google's token for the next page. Its presence is what says the
    /// collection is only part of the window.
    next_page_token: Option<String>,
}

/// One event from `events.list`.
///
/// Only the fields the conversion reads are named; a payload's subjects,
/// locations and the rest of Google's fields are ignored, because the engine
/// never sees them.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleEvent {
    id: Option<String>,
    status: Option<String>,
    #[serde(rename = "iCalUID")]
    ical_uid: Option<String>,
    event_type: Option<String>,
    transparency: Option<String>,
    #[serde(default)]
    attendees_omitted: bool,
    #[serde(default)]
    end_time_unspecified: bool,
    attendees: Option<Vec<GoogleAttendee>>,
    start: Option<GoogleEventTime>,
    end: Option<GoogleEventTime>,
    original_start_time: Option<GoogleEventTime>,
    recurrence: Option<Vec<String>>,
    extended_properties: Option<GoogleExtendedProperties>,
    /// The `id` of the series an instance belongs to. Google shares one
    /// `iCalUID` across a series, so this is what says a caller marker needs the
    /// instance's original slot appended to stay distinct.
    recurring_event_id: Option<String>,
}

/// One entry of an event's attendee list: only the user's own answer is read.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleAttendee {
    // `self` is a Rust keyword, so the field is named for what it means and
    // renamed to the key Google uses.
    #[serde(rename = "self", default)]
    is_self: bool,
    response_status: Option<String>,
}

/// An event's start, end or original start: a civil date, or a timestamp.
///
/// Google's contract allows the timestamp to omit its offset when `timeZone`
/// carries the zone ("a time zone offset is required unless a time zone is
/// explicitly specified in `timeZone`"), so the zone is read alongside it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleEventTime {
    date: Option<String>,
    date_time: Option<String>,
    time_zone: Option<String>,
}

/// An event's extended properties, where callers write their markers.
#[derive(Debug, Deserialize)]
struct GoogleExtendedProperties {
    private: Option<BTreeMap<String, String>>,
    shared: Option<BTreeMap<String, String>>,
}

/// Converts one event, or nothing when it does not occupy time.
fn convert_event(
    zones: &mut ZoneCache<'_>,
    participant_zone: &str,
    response_zone: Option<&str>,
    event: GoogleEvent,
    diagnostics: &mut Vec<AdapterDiagnostic>,
) -> Result<Option<AnnotatedBlock>, Failure> {
    let GoogleEvent {
        id,
        status,
        ical_uid,
        event_type,
        transparency,
        attendees_omitted,
        end_time_unspecified,
        attendees,
        start,
        end,
        original_start_time,
        recurrence,
        extended_properties,
        recurring_event_id,
    } = event;
    let shown = id.as_deref().unwrap_or("without an id");
    // A cancelled event may carry no times at all, so it is dropped before
    // anything reads them.
    if status
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case("cancelled"))
    {
        return Ok(None);
    }
    let own_response = self_response(attendees.as_deref());
    if own_response.is_some_and(|response| response.eq_ignore_ascii_case("declined")) {
        return Ok(None);
    }
    if recurrence.is_some_and(|rules| rules.iter().any(|rule| !rule.trim().is_empty())) {
        return Err((
            "unexpanded_series",
            format!(
                "the event `{shown}` carries a `recurrence`, so Google has not expanded the \
                 series into instances: query `events.list` with `singleEvents=true`"
            ),
        ));
    }
    let subject = Subject {
        label: shown,
        event_id: id.as_deref(),
    };
    if end_time_unspecified {
        diagnostics.push(subject.diagnostic(
            "end_time_unspecified",
            format!(
                "the event `{shown}` reports `endTimeUnspecified`, so its end is kept as given"
            ),
        ));
    }
    let all_day = start.as_ref().is_some_and(|time| time.date.is_some());
    let Some(span) = event_span(
        zones,
        participant_zone,
        response_zone,
        start.as_ref(),
        end.as_ref(),
        subject,
        diagnostics,
    )?
    else {
        return Ok(None);
    };

    let (status, status_tag) = event_status(
        own_response,
        transparency.as_deref(),
        event_type.as_deref(),
        attendees_omitted,
        subject,
        diagnostics,
    );
    let tags = block_tags(
        status_tag,
        event_type.as_deref(),
        extended_properties.as_ref(),
        id.as_deref(),
        diagnostics,
    );
    let event_key = event_key(
        ical_uid.as_deref(),
        recurring_event_id.as_deref(),
        original_start_time.as_ref(),
        extended_properties.as_ref(),
        shown,
        zones,
        participant_zone,
        response_zone,
    )?;

    let mut block = Block::new(span).with_status(status).with_tags(tags);
    if all_day {
        block = block.with_all_day(true);
    }
    if let Some(key) = event_key {
        block = block
            .with_event_key(key)
            .map_err(|err| invalid_event(shown, err.to_string()))?;
    }
    Ok(Some(AnnotatedBlock {
        event_id: id,
        block,
    }))
}

/// One event's span, read from civil dates for an all-day event and from the
/// offsets a timed event's timestamps carry.
#[allow(clippy::too_many_arguments)]
fn event_span(
    zones: &mut ZoneCache<'_>,
    participant_zone: &str,
    response_zone: Option<&str>,
    start: Option<&GoogleEventTime>,
    end: Option<&GoogleEventTime>,
    subject: Subject<'_>,
    diagnostics: &mut Vec<AdapterDiagnostic>,
) -> Result<Option<Span>, Failure> {
    if start.is_some_and(|time| time.date.is_some()) {
        all_day_span(
            zones,
            participant_zone,
            response_zone,
            start,
            end,
            subject,
            diagnostics,
        )
    } else {
        timed_span(
            zones,
            participant_zone,
            response_zone,
            start,
            end,
            subject,
            diagnostics,
        )
    }
}

/// The block's tags, in the order status, event type, markers.
fn block_tags(
    status_tag: Option<&'static str>,
    event_type: Option<&str>,
    extended: Option<&GoogleExtendedProperties>,
    event_id: Option<&str>,
    diagnostics: &mut Vec<AdapterDiagnostic>,
) -> Vec<Tag> {
    let status = status_tag.map(own_tag);
    let event_type = event_type
        .and_then(|kind| {
            EVENT_TYPE_TAGS
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(kind))
        })
        .map(|(_, tag)| own_tag(tag));
    let markers = marker_tags(extended, event_id, diagnostics);
    unique_tags(status.into_iter().chain(event_type).chain(markers))
}

/// The user's own invitation response, when Google reported one.
fn self_response(attendees: Option<&[GoogleAttendee]>) -> Option<&str> {
    attendees?
        .iter()
        .find(|attendee| attendee.is_self)?
        .response_status
        .as_deref()
}

/// The block status the five-step Google precedence names, and the status tag it
/// carries.
fn event_status(
    own_response: Option<&str>,
    transparency: Option<&str>,
    event_type: Option<&str>,
    attendees_omitted: bool,
    subject: Subject<'_>,
    diagnostics: &mut Vec<AdapterDiagnostic>,
) -> (BlockStatus, Option<&'static str>) {
    // The omitted list is reported whatever the event's status: the entry that
    // would say how the user answered is gone, so a status policy cannot rely on
    // their response either way.
    if attendees_omitted {
        diagnostics.push(subject.diagnostic(
            "attendees_omitted",
            format!(
                "the event `{}` sets `attendeesOmitted`, so the user's own invitation response is \
                 unknown",
                subject.label
            ),
        ));
    }
    // Transparency is checked before the user's own answer, so a transparent
    // event never blocks, however the invitation was answered.
    if transparency.is_some_and(|value| value.eq_ignore_ascii_case("transparent")) {
        return (BlockStatus::Free, None);
    }
    // Google writes these all-day marker types as `transparent`, but the event is
    // otherwise read as `opaque` and an omitted field would block the whole day.
    // An explicit `opaque` still wins, because it is the field that decides.
    if transparency.is_none()
        && event_type.is_some_and(|kind| {
            kind.eq_ignore_ascii_case("birthday") || kind.eq_ignore_ascii_case("workingLocation")
        })
    {
        return (BlockStatus::Free, None);
    }
    if own_response.is_some_and(|response| response.eq_ignore_ascii_case("tentative")) {
        return (BlockStatus::Tentative, None);
    }
    if own_response.is_some_and(|response| response.eq_ignore_ascii_case("needsAction")) {
        return (BlockStatus::Tentative, Some(TAG_NEEDS_ACTION));
    }
    (BlockStatus::Busy, None)
}

/// The marker tags an event carries: `shared` first, because it is visible on
/// every attendee's copy, then `private`.
fn marker_tags(
    extended: Option<&GoogleExtendedProperties>,
    event_id: Option<&str>,
    diagnostics: &mut Vec<AdapterDiagnostic>,
) -> Vec<Tag> {
    let mut tags = Vec::new();
    for properties in [
        extended.and_then(|extended| extended.shared.as_ref()),
        extended.and_then(|extended| extended.private.as_ref()),
    ] {
        let Some(value) = properties.and_then(|properties| properties.get(MARKER_TAGS)) else {
            continue;
        };
        tags.extend(parse_marker_tags(value, event_id, diagnostics));
    }
    unique_tags(tags)
}

/// The event key for one event: the caller's marker, then `iCalUID`, combined
/// with the UTC-normalised `originalStartTime` of a recurring instance.
///
/// Google shares one `iCalUID` across a series, and a shared extended property is
/// copied to every instance, so an instance's key carries its original slot
/// whatever supplied the identifier: without it, one series would collapse into a
/// single event for cap counting and rescheduling.
#[allow(clippy::too_many_arguments)]
fn event_key(
    ical_uid: Option<&str>,
    recurring_event_id: Option<&str>,
    original_start_time: Option<&GoogleEventTime>,
    extended: Option<&GoogleExtendedProperties>,
    label: &str,
    zones: &mut ZoneCache<'_>,
    participant_zone: &str,
    response_zone: Option<&str>,
) -> Result<Option<String>, Failure> {
    if let Some(key) = marker_key(extended, MARKER_EVENT_KEY) {
        // Only a recurring instance takes the discriminator: a single event's
        // marker is the caller's own key and is returned exactly as written.
        if recurring_event_id.is_some()
            && let Some(original) = original_start_time
        {
            let instance = instance_of(original, zones, participant_zone, response_zone, label)?;
            return Ok(Some(format!("{key}@{instance}")));
        }
        return Ok(Some(key));
    }
    let Some(uid) = ical_uid.map(str::trim).filter(|uid| !uid.is_empty()) else {
        return Ok(None);
    };
    let Some(original) = original_start_time else {
        return Ok(Some(uid.to_owned()));
    };
    let instance = instance_of(original, zones, participant_zone, response_zone, label)?;
    Ok(Some(format!("{uid}@{instance}")))
}

/// The UTC-normalised identity of one recurring instance: its civil date, or its
/// instant with an offset (or the wall-clock reading resolved in its zone).
fn instance_of(
    original: &GoogleEventTime,
    zones: &mut ZoneCache<'_>,
    participant_zone: &str,
    response_zone: Option<&str>,
    label: &str,
) -> Result<String, Failure> {
    match (original.date.as_deref(), original.date_time.as_deref()) {
        (Some(date), _) => parse_date(date)
            .map(|date| date.to_string())
            .map_err(|message| invalid_event(label, message)),
        (None, Some(date_time)) => {
            let resolved = resolve_reading(
                date_time,
                original.time_zone.as_deref(),
                response_zone,
                participant_zone,
                zones,
                label,
            )?;
            Stamp {
                seconds: resolved.seconds,
                nanos: resolved.nanos,
            }
            .utc_key()
            .map_err(|message| invalid_event(label, message))
        }
        (None, None) => Err(invalid_event(
            label,
            "the `originalStartTime` names neither a `date` nor a `dateTime`".to_owned(),
        )),
    }
}

/// An event's marker value for `name`, from `shared` before `private`.
fn marker_key(extended: Option<&GoogleExtendedProperties>, name: &str) -> Option<String> {
    let extended = extended?;
    [extended.shared.as_ref(), extended.private.as_ref()]
        .into_iter()
        .flatten()
        .filter_map(|properties| properties.get(name))
        .map(|key| key.trim())
        .find(|key| !key.is_empty())
        .map(str::to_owned)
}

/// One all-day event's span, resolved from its civil dates in the response's
/// zone or, without one, the participant's zone.
#[allow(clippy::too_many_arguments)]
fn all_day_span(
    zones: &mut ZoneCache<'_>,
    participant_zone: &str,
    response_zone: Option<&str>,
    start: Option<&GoogleEventTime>,
    end: Option<&GoogleEventTime>,
    subject: Subject<'_>,
    diagnostics: &mut Vec<AdapterDiagnostic>,
) -> Result<Option<Span>, Failure> {
    let label = subject.label;
    let start = start.ok_or_else(|| {
        (
            "payload_invalid",
            format!("the all-day event `{label}` has no start"),
        )
    })?;
    let end = end.ok_or_else(|| {
        (
            "payload_invalid",
            format!("the all-day event `{label}` has no end"),
        )
    })?;
    // The response's zone is the calendar's own, and the participant's is the
    // fallback. Both resolve lazily: a timed calendar never reads either.
    let zone_name = response_zone
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(participant_zone);
    let zone = zones
        .resolve(zone_name)
        .map_err(|err| (err.code(), err.to_string()))?;
    let resolve = |time: &GoogleEventTime| -> Result<Resolved, Failure> {
        let date = time.date.as_deref().ok_or_else(|| {
            (
                "payload_invalid",
                format!("the all-day event `{label}` names no `date`"),
            )
        })?;
        let date = parse_date(date).map_err(|message| invalid_event(label, message))?;
        zone.day_start(date).map(Resolved::of).map_err(|err| {
            (
                "payload_invalid",
                format!("the all-day event `{label}` begins outside the supported range: {err}"),
            )
        })
    };
    finish_span(resolve(start)?, resolve(end)?, subject, diagnostics)
}

/// One timed span's instants, each read from the offset its own timestamp carries
/// or from the zone the timestamp leaves its offset to.
fn timed_span(
    zones: &mut ZoneCache<'_>,
    participant_zone: &str,
    response_zone: Option<&str>,
    start: Option<&GoogleEventTime>,
    end: Option<&GoogleEventTime>,
    subject: Subject<'_>,
    diagnostics: &mut Vec<AdapterDiagnostic>,
) -> Result<Option<Span>, Failure> {
    let label = subject.label;
    let start = timed_endpoint(
        zones,
        participant_zone,
        response_zone,
        start,
        label,
        "start",
    )?;
    let end = timed_endpoint(zones, participant_zone, response_zone, end, label, "end")?;
    finish_span(start, end, subject, diagnostics)
}

/// One timed endpoint, or the failure that says which field is missing.
fn timed_endpoint(
    zones: &mut ZoneCache<'_>,
    participant_zone: &str,
    response_zone: Option<&str>,
    time: Option<&GoogleEventTime>,
    label: &str,
    field: &str,
) -> Result<Resolved, Failure> {
    let Some(time) = time else {
        return Err((
            "payload_invalid",
            format!("the event `{label}` has no {field}"),
        ));
    };
    let raw = time.date_time.as_deref().ok_or_else(|| {
        (
            "payload_invalid",
            format!("the timed event `{label}` names no `dateTime`"),
        )
    })?;
    resolve_reading(
        raw,
        time.time_zone.as_deref(),
        response_zone,
        participant_zone,
        zones,
        label,
    )
}

/// One `dateTime` reading, as an instant.
///
/// An offset is the ordinary case. Google's contract also allows the offset to be
/// left to the sibling `timeZone`, so a reading without one is resolved in the
/// event's own zone, then the response's, then the participant's; the calendar
/// fails only when none of them names a zone the database holds.
fn resolve_reading(
    raw: &str,
    event_zone: Option<&str>,
    response_zone: Option<&str>,
    participant_zone: &str,
    zones: &mut ZoneCache<'_>,
    label: &str,
) -> Result<Resolved, Failure> {
    if Stamp::has_offset(raw) {
        let stamp = Stamp::parse(raw).map_err(|message| invalid_event(label, message))?;
        return Ok(Resolved {
            seconds: stamp.seconds,
            nanos: stamp.nanos,
        });
    }
    let zone_name = event_zone
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .or_else(|| response_zone.map(str::trim).filter(|name| !name.is_empty()))
        .unwrap_or(participant_zone);
    let zone = zones
        .resolve(zone_name)
        .map_err(|err| (err.code(), err.to_string()))?;
    LocalStamp::parse(raw)
        .and_then(|local| local.resolve(zone))
        .map_err(|message| invalid_event(label, message))
}

/// One calendar's outcome: the provider's errors, or its busy ranges.
///
/// An entry with no `busy` key is a calendar Google found nothing busy in, not a
/// missing answer: Google omits an empty repeated field, so no key is how it writes
/// a calendar with no ranges in the window.
fn free_busy_calendar(entry: FreeBusyCalendar) -> CalendarOutcome {
    if let Some(errors) = entry.errors.filter(|errors| !errors.is_empty()) {
        let message = errors
            .iter()
            .map(describe_error)
            .collect::<Vec<_>>()
            .join("; ");
        return unavailable("provider_error", message);
    }
    let mut blocks = Vec::new();
    let mut diagnostics = Vec::new();
    for (index, range) in entry.busy.unwrap_or_default().into_iter().enumerate() {
        let shown = format!("free/busy range {index}");
        let subject = Subject {
            label: &shown,
            event_id: None,
        };
        let span = match busy_span(&range, subject, &mut diagnostics) {
            Ok(Some(span)) => span,
            Ok(None) => continue,
            Err((code, message)) => return unavailable(code, message),
        };
        blocks.push(AnnotatedBlock {
            event_id: None,
            block: Block::new(span),
        });
    }
    CalendarOutcome::Available {
        blocks,
        diagnostics,
    }
}

/// One busy range's span, dropped when its endpoints coincide.
fn busy_span(
    range: &FreeBusyRange,
    subject: Subject<'_>,
    diagnostics: &mut Vec<AdapterDiagnostic>,
) -> Result<Option<Span>, Failure> {
    let label = subject.label;
    let instant = |raw: &str| {
        Stamp::parse(raw)
            .map(|stamp| Resolved {
                seconds: stamp.seconds,
                nanos: stamp.nanos,
            })
            .map_err(|message| ("payload_invalid", format!("the {label}: {message}")))
    };
    finish_span(
        instant(&range.start)?,
        instant(&range.end)?,
        subject,
        diagnostics,
    )
}

/// The sentence one `freeBusyError` becomes.
fn describe_error(error: &FreeBusyError) -> String {
    match (error.domain.as_deref(), error.reason.as_deref()) {
        (Some(domain), Some(reason)) => format!("{domain}: {reason}"),
        (None, Some(reason)) => reason.to_owned(),
        (Some(domain), None) => domain.to_owned(),
        (None, None) => "the provider reported an error for this calendar".to_owned(),
    }
}

/// A payload failure about the event `label`, carrying `message` as its cause.
fn invalid_event(label: &str, message: impl Into<String>) -> Failure {
    (
        "payload_invalid",
        format!("the event `{label}`: {}", message.into()),
    )
}
