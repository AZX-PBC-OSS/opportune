//! Microsoft Graph (Entra / Outlook) payloads: `calendarView` events,
//! `getSchedule` free/busy, and a mailbox's `workingHours`.
//!
//! Three conversions, deliberately separate, so a working-hours problem cannot
//! break availability:
//!
//! - [`calendar_view_to_blocks`] converts the expanded events of one mailbox into
//!   blocks, with the event's categories, status tags, marker tags and event key.
//! - [`get_schedule_to_blocks`] converts a batch of free/busy schedules, one
//!   outcome per `scheduleId`. Free/busy carries less than events do: no event ID,
//!   no categories and no all-day flag, so these blocks have no event key and are
//!   never `all_day`. Rest rules and category-based caps therefore need
//!   `calendarView`.
//! - [`working_hours_to_rule`] converts one mailbox's working hours into a
//!   `window` rule in the IANA zone behind the Windows name the payload carries.
//!
//! # Time reading
//!
//! Graph `dateTimeTimeZone` values carry no offset, so a timed event is resolved
//! from its wall-clock reading in the zone the value names, with jiff's compatible
//! disambiguation: the earlier instant in a repeated hour, the instant past the gap
//! in a skipped one. Callers request UTC (`Prefer: outlook.timezone="UTC"`) and
//! Windows zone names still resolve, so a payload that ignores the header converts
//! correctly too.
//!
//! All-day events are read from their civil dates instead, midnight to midnight in
//! the participant's zone whatever zone the payload stamps on them — the 23-hour
//! spring-forward day and the 25-hour fall-back day included.
//!
//! An event whose start and end resolve to the same instant is dropped with a
//! `zero_length_event` diagnostic *before* the endpoints are rounded outward,
//! which would otherwise turn a zero-length reminder at `10:00:30` into the
//! one-minute block `[10:00, 10:01)`.
//!
//! # Working hours
//!
//! A `workingHours` window is read on whole minutes, because that is the engine's
//! resolution. A boundary with a sub-minute component — Graph's `Edm.TimeOfDay`
//! can spell `23:59:59` — is rejected with `invalid_working_hours` rather than
//! truncated, so the caller keeps its own fallback window instead of silently
//! moving the edge of the working day.
//!
//! # Status
//!
//! `showAs` and the free/busy status share one mapping, matched case-insensitively:
//! `free` is free, `tentative` is tentative, and `busy`, `oof` and `unknown` are
//! busy — `oof` tagged `status/oof` and `unknown` tagged `status/unknown`.
//! `workingElsewhere` is free and tagged `status/working-elsewhere`. Cancelled
//! events and events the user declined are dropped; draft events are kept as
//! `showAs` says and tagged `status/draft`.
//!
//! # Event keys
//!
//! A block's `event_key` is the caller's `opportune.event_key` marker when the
//! event carries one, otherwise the provider's `iCalUId`. Graph may share one
//! `iCalUId` across the occurrences of a recurring series, and a shared marker
//! written once on a series reaches every occurrence, so an occurrence or
//! exception also carries its `originalStart` slot (`<key>@<utc>`). Without that
//! discriminator the engine would count a whole series as one event and take the
//! hull of its occurrences.
//!
//! # Expansion
//!
//! A `calendarView` response never expands a series master into instances, but a
//! payload that carries one means the caller queried another endpoint. That
//! calendar is [`CalendarOutcome::Unavailable`] with `unexpanded_series` rather
//! than an empty or partial block list.
//!
//! # Caller requirements
//!
//! - Query `calendarView`, not `/events`, so Graph expands each recurring series
//!   into the instances that fall in the window.
//! - Follow `@odata.nextLink` until it is absent, and pass the concatenated
//!   `value` array as one payload. Graph pages `calendarView`, so a single page is
//!   only part of the window; the conversion rejects a page rather than answer
//!   with the events it happens to hold, because the rest would read as free time.
//! - Send `Prefer: outlook.timezone="UTC"` so every `dateTime` is UTC-stamped and
//!   the payload needs no zone name to be read correctly.
//! - Expand the marker properties with [`EXTENDED_PROPERTY_EXPAND`], or the
//!   `opportune.tags` and `opportune.event_key` markers never arrive. The filter
//!   holds spaces, so it must be URL-encoded in the request URI.
//! - Chunk `getSchedule` to at most 20 schedules per call and 62 days per window,
//!   Graph's documented limits. A larger request is rejected by Graph, not
//!   converted here.
//!
//! # Example
//!
//! ```
//! # #[cfg(feature = "bundled-tzdb")] {
//! use opportune::time::{TimeZoneDatabase, ZoneName};
//! use opportune_adapters::graph::calendar_view_to_blocks;
//!
//! let payload = r#"{
//!   "value": [{
//!     "id": "AAMkAD",
//!     "type": "singleInstance",
//!     "isAllDay": false,
//!     "showAs": "busy",
//!     "iCalUId": "040000008200E00074C5B7101A82E00800000000",
//!     "start": { "dateTime": "2027-03-09T14:00:00.0000000", "timeZone": "UTC" },
//!     "end": { "dateTime": "2027-03-09T15:00:00.0000000", "timeZone": "UTC" },
//!     "categories": ["Interview"]
//!   }]
//! }"#;
//!
//! let db = TimeZoneDatabase::bundled().unwrap();
//! let zone = ZoneName::new("America/New_York").unwrap();
//! let outcome = calendar_view_to_blocks(&db, payload, &zone, None);
//! let opportune_adapters::CalendarOutcome::Available { blocks, diagnostics } = outcome else {
//!     panic!("the payload converts");
//! };
//! assert!(diagnostics.is_empty());
//! assert_eq!(blocks[0].block.span.start().to_string(), "2027-03-09T14:00Z");
//! assert_eq!(
//!     blocks[0].block.event_key.as_deref(),
//!     Some("040000008200E00074C5B7101A82E00800000000")
//! );
//! assert_eq!(blocks[0].block.tags[0].as_str(), "category/interview");
//! # }
//! ```

use std::collections::BTreeMap;

use opportune::model::{Block, BlockStatus, Rule, RuleKind, Span};
use opportune::tags::Tag;
use opportune::time::{
    RuleZone, TimeOfDay, TimeZoneDatabase, WallEnd, WallRange, Weekday, Weekdays, WeeklyKey,
    WindowSpec, ZoneName,
};
use serde::Deserialize;

use crate::blocks::{
    Failure, LocalStamp, Resolved, Stamp, Subject, ZoneCache, finish_span, own_tag, parse_clock,
    unavailable, unique_tags,
};
use crate::types::{
    AdapterDiagnostic, AdapterPayloadError, AdapterZoneError, AnnotatedBlock, CalendarOutcome,
    MARKER_EVENT_KEY, MARKER_TAGS, WorkingHoursError,
};
use crate::zones::{categories_to_tags, parse_marker_tags, resolve_zone};

/// The GUID of opportune's Graph extended-property set.
///
/// It is permanent: a caller that writes `opportune.tags` or
/// `opportune.event_key` addresses the property by this GUID and the property
/// name, so changing it would orphan every marker already written to a tenant.
pub const EXTENDED_PROPERTY_NAMESPACE_GUID: &str = "a5d0c4e2-7b1f-4c9a-9e3d-2f8b6c1a4e70";

/// The `id` of the extended property that carries [`MARKER_TAGS`].
pub const EXTENDED_PROPERTY_TAGS_ID: &str =
    "String {a5d0c4e2-7b1f-4c9a-9e3d-2f8b6c1a4e70} Name opportune.tags";

/// The `id` of the extended property that carries [`MARKER_EVENT_KEY`].
pub const EXTENDED_PROPERTY_EVENT_KEY_ID: &str =
    "String {a5d0c4e2-7b1f-4c9a-9e3d-2f8b6c1a4e70} Name opportune.event_key";

/// The `$expand` value that fetches both markers with an event.
///
/// The filter holds spaces, so a caller URL-encodes it before appending it to the
/// request URI. Graph filters an extended property's `id` the way the property was
/// created, so the GUID inside it may come back in either case; the adapter
/// matches the namespace case-insensitively and the property name exactly.
pub const EXTENDED_PROPERTY_EXPAND: &str = "singleValueExtendedProperties($filter=id eq 'String \
     {a5d0c4e2-7b1f-4c9a-9e3d-2f8b6c1a4e70} Name opportune.tags' or id eq 'String \
     {a5d0c4e2-7b1f-4c9a-9e3d-2f8b6c1a4e70} Name opportune.event_key')";

/// The Graph time-zone discriminator that means "a bias and a daylight offset
/// follow", which this adapter does not resolve.
const CUSTOM_TIME_ZONE_TYPE: &str = "microsoft.graph.customTimeZone";

/// The placeholder name Graph writes on a custom time zone. Some payloads carry
/// it without the discriminator or the bias, so the name identifies the form on
/// its own.
const CUSTOM_TIME_ZONE_NAME: &str = "Customized Time Zone";

/// The status tag Graph's `oof` status carries.
const TAG_OOF: &str = "status/oof";

/// The status tag Graph's `workingElsewhere` status carries.
const TAG_WORKING_ELSEWHERE: &str = "status/working-elsewhere";

/// The status tag Graph's `unknown` status carries.
const TAG_UNKNOWN: &str = "status/unknown";

/// The tag a draft event carries.
const TAG_DRAFT: &str = "status/draft";

/// Converts a `calendarView` response for one mailbox into blocks.
///
/// The participant's zone is what every all-day event is read in, and the category
/// map turns the caller's category names into tags. A participant zone nothing
/// resolves (when an all-day event needs it), a zone the payload names that no
/// database entry covers, a payload that is not readable, an event that cannot
/// become a block, and a payload that is one page of a paginated response all fail
/// the whole calendar: answering with the events that did convert would read as
/// free time over the ones that did not.
///
/// A category the caller's map does not name and whose name has no ASCII slug makes
/// no tag; the event still converts and a `category_tag_unmappable` diagnostic names
/// the category, so a caller whose category is written in a non-Latin script can see
/// why its `category/...` policy never matched. A blank category string is not a
/// category and is dropped silently.
///
/// ```
/// # #[cfg(feature = "bundled-tzdb")] {
/// use opportune::time::{TimeZoneDatabase, ZoneName};
/// use opportune_adapters::graph::calendar_view_to_blocks;
/// use opportune_adapters::CalendarOutcome;
///
/// let db = TimeZoneDatabase::bundled().unwrap();
/// let zone = ZoneName::new("UTC").unwrap();
/// let outcome = calendar_view_to_blocks(&db, r#"{ "value": [] }"#, &zone, None);
/// let CalendarOutcome::Available { blocks, .. } = outcome else {
///     panic!("an empty calendar converts");
/// };
/// assert!(blocks.is_empty());
/// # }
/// ```
#[must_use]
pub fn calendar_view_to_blocks(
    db: &TimeZoneDatabase,
    payload_json: &str,
    participant_zone: &ZoneName,
    category_map: Option<&BTreeMap<String, Tag>>,
) -> CalendarOutcome {
    let collection = match serde_json::from_str::<GraphCollection<GraphEvent>>(payload_json) {
        Ok(collection) => collection,
        Err(err) => {
            return unavailable(
                "payload_invalid",
                format!("the calendarView payload is not readable: {err}"),
            );
        }
    };
    // A page is not the response. The events after `@odata.nextLink` are missing
    // from the document entirely, so converting the page alone would read them as
    // free time.
    if collection
        .next_link
        .as_deref()
        .is_some_and(|link| !link.trim().is_empty())
    {
        return unavailable(
            "payload_invalid",
            "the calendarView payload is one page of a paginated response, so it is missing the \
             events on the pages after it: follow `@odata.nextLink` and pass the concatenated \
             `value` array"
                .to_owned(),
        );
    }
    // A participant zone is only read for all-day events, so it is resolved when
    // the first one needs it: a timed-only calendar converted from a payload that
    // stamps zones per event never fails over an unused name.
    let participant = participant_zone.as_str();
    // A payload that stamps every event in the same zone names that zone once per
    // endpoint, and resolving a Windows name means a scan of the CLDR table. The
    // cache is request-scoped: it lives as long as this conversion and nothing
    // outlives it.
    let mut zones = ZoneCache::new(db);
    let mut blocks = Vec::new();
    let mut diagnostics = Vec::new();
    for event in collection.value {
        match convert_event(
            participant,
            &mut zones,
            event,
            category_map,
            &mut diagnostics,
        ) {
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

/// Converts a `getSchedule` response into one outcome per `scheduleId`.
///
/// A schedule the provider reported an `error` for — a mailbox the caller may not
/// read, say — comes back [`CalendarOutcome::Unavailable`] with `provider_error`,
/// while the healthy schedules in the same batch keep their blocks. An entry whose
/// `scheduleItems` Graph omitted answers from `availabilityView`: an all-free view
/// is a free mailbox, while a view naming busy slots is that mailbox's
/// `payload_invalid` outcome, so one unreadable mailbox never fails the batch. A
/// response that is not readable, repeats a `scheduleId`, names no `scheduleId`, or
/// carries an entry that reports neither `scheduleItems` nor an `error` nor an
/// `availabilityView` fails the whole call: there is no per-calendar answer to
/// attach that to, and an empty block list would read as "completely free".
///
/// ```
/// # #[cfg(feature = "bundled-tzdb")] {
/// use opportune::time::TimeZoneDatabase;
/// use opportune_adapters::graph::get_schedule_to_blocks;
/// use opportune_adapters::CalendarOutcome;
///
/// let payload = r#"{
///   "value": [{
///     "scheduleId": "ana@example.com",
///     "scheduleItems": [{
///       "status": "busy",
///       "start": { "dateTime": "2027-03-09T09:00:00.0000000", "timeZone": "UTC" },
///       "end": { "dateTime": "2027-03-09T10:00:00.0000000", "timeZone": "UTC" }
///     }]
///   }]
/// }"#;
///
/// let db = TimeZoneDatabase::bundled().unwrap();
/// let outcomes = get_schedule_to_blocks(&db, payload).unwrap();
/// let CalendarOutcome::Available { blocks, .. } = &outcomes["ana@example.com"] else {
///     panic!("the schedule converts");
/// };
/// assert_eq!(blocks[0].block.span.start().to_string(), "2027-03-09T09:00Z");
/// assert!(blocks[0].block.event_key.is_none());
/// # }
/// ```
///
/// # Errors
/// Returns [`AdapterPayloadError::InvalidPayload`] when the document is not a
/// `getSchedule` response this adapter can read.
pub fn get_schedule_to_blocks(
    db: &TimeZoneDatabase,
    payload_json: &str,
) -> Result<BTreeMap<String, CalendarOutcome>, AdapterPayloadError> {
    let response: GraphScheduleResponse = serde_json::from_str(payload_json)
        .map_err(|err| AdapterPayloadError::invalid("getSchedule", err.to_string()))?;
    let mut zones = ZoneCache::new(db);
    let mut outcomes = BTreeMap::new();
    for (index, schedule) in response.value.into_iter().enumerate() {
        let GraphSchedule {
            schedule_id,
            schedule_items,
            error,
            availability_view,
        } = schedule;
        let id = schedule_id.filter(|id| !id.is_empty()).ok_or_else(|| {
            AdapterPayloadError::invalid(
                "getSchedule",
                format!("the schedule at index {index} names no `scheduleId`"),
            )
        })?;
        if outcomes.contains_key(&id) {
            return Err(AdapterPayloadError::invalid(
                "getSchedule",
                format!("two schedules share the id `{id}`"),
            ));
        }
        let outcome = schedule_entry(
            &id,
            schedule_items,
            error,
            availability_view.as_deref(),
            &mut zones,
        )?;
        outcomes.insert(id, outcome);
    }
    Ok(outcomes)
}

/// One schedule's answer: its items, the provider's error, or the merged
/// `availabilityView` when the items were omitted.
///
/// Graph omits an empty collection just as Google does, and the merged view is
/// the answer it sends instead. An all-free view is a free mailbox; a view with
/// busy slots and no items is a mailbox whose busy time is missing, which fails
/// that mailbox alone rather than the whole batch. An entry with neither is not
/// an answer at all, and fails the call.
fn schedule_entry(
    id: &str,
    items: Option<Vec<GraphScheduleItem>>,
    error: Option<GraphScheduleError>,
    availability_view: Option<&str>,
    zones: &mut ZoneCache<'_>,
) -> Result<CalendarOutcome, AdapterPayloadError> {
    // A provider error is the per-calendar answer: it names why this mailbox
    // could not be read and leaves the healthy schedules alone.
    if let Some(error) = error {
        let message = error
            .message
            .or(error.response_code)
            .unwrap_or_else(|| "the provider reported an error for this schedule".to_owned());
        return Ok(unavailable("provider_error", message));
    }
    let Some(items) = items else {
        let Some(view) = availability_view.map(str::trim) else {
            return Err(AdapterPayloadError::invalid(
                "getSchedule",
                format!(
                    "the schedule `{id}` reports neither `scheduleItems` nor an `error`, so \
                     whether it is free is unknown"
                ),
            ));
        };
        if view.bytes().all(|slot| slot == b'0') {
            return Ok(CalendarOutcome::Available {
                blocks: Vec::new(),
                diagnostics: Vec::new(),
            });
        }
        return Ok(unavailable(
            "payload_invalid",
            format!(
                "the schedule `{id}` reports busy slots in `availabilityView` (`{view}`) but no \
                 `scheduleItems`, so its busy time cannot be read"
            ),
        ));
    };
    Ok(schedule_to_outcome(zones, items))
}

/// Converts one mailbox's `workingHours` into a `window` rule.
///
/// `payload_json` is the `workingHours` object from a `getSchedule` entry, not the
/// whole response: the blocks from that response convert whether or not the
/// working hours do. The rule's window is read in the IANA zone behind the name
/// the payload carries, and works on every day the payload lists.
///
/// ```
/// # #[cfg(feature = "bundled-tzdb")] {
/// use opportune::time::TimeZoneDatabase;
/// use opportune_adapters::graph::working_hours_to_rule;
///
/// let payload = r#"{
///   "daysOfWeek": ["monday", "tuesday", "wednesday", "thursday", "friday"],
///   "startTime": "09:00:00.0000000",
///   "endTime": "17:00:00.0000000",
///   "timeZone": { "name": "America/New_York" }
/// }"#;
///
/// let db = TimeZoneDatabase::bundled().unwrap();
/// let rule = working_hours_to_rule(&db, payload, "working_hours").unwrap();
/// assert_eq!(rule.id, "working_hours");
/// assert!(rule.window().is_some());
/// # }
/// ```
///
/// A `#microsoft.graph.customTimeZone` is Graph's `@odata.type` for a zone defined
/// by a bias and daylight offset instead of a name; `workingHours` for a mailbox
/// that sets one carries it, and this adapter does not resolve it.
///
/// # Errors
/// Returns [`WorkingHoursError::CustomTimeZone`] for a custom Graph time zone,
/// [`WorkingHoursError::UnresolvedZone`] for a name nothing resolves, and
/// [`WorkingHoursError::InvalidWorkingHours`] for days, times or a range that
/// cannot describe a window. The blocks from the same response are unaffected:
/// this is a separate conversion.
pub fn working_hours_to_rule(
    db: &TimeZoneDatabase,
    payload_json: &str,
    rule_id: &str,
) -> Result<Rule, WorkingHoursError> {
    let working: GraphWorkingHours = serde_json::from_str(payload_json).map_err(|err| {
        invalid_working_hours(format!("the workingHours payload is not readable: {err}"))
    })?;
    let time_zone = working
        .time_zone
        .as_ref()
        .ok_or_else(|| invalid_working_hours("the workingHours payload names no time zone"))?;
    // A custom zone is identified by its type, its bias, or the placeholder name
    // Graph writes, so a payload carrying only one of the three is still
    // recognised rather than reported as an unresolvable name.
    if time_zone.is_custom() {
        return Err(WorkingHoursError::CustomTimeZone);
    }
    let zone_name = time_zone
        .name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| invalid_working_hours("the working hours name no time zone"))?;
    let zone = match resolve_zone(db, zone_name) {
        Ok(zone) => zone,
        Err(AdapterZoneError::Unresolved { name }) => {
            return Err(WorkingHoursError::UnresolvedZone { name });
        }
    };
    let days = working_days(working.days_of_week.as_deref())?;
    let range = working_range(working.start_time.as_deref(), working.end_time.as_deref())?;
    let zone_name = ZoneName::new(zone.name())
        .map_err(|err| invalid_working_hours(format!("`{}`: {err}", zone.name())))?;
    let key =
        WeeklyKey::new(days, vec![range]).map_err(|err| invalid_working_hours(err.to_string()))?;
    let spec = WindowSpec::new(RuleZone::Named(zone_name), vec![key], Vec::new())
        .map_err(|err| invalid_working_hours(err.to_string()))?;
    Ok(Rule::new(rule_id, RuleKind::Window(spec)))
}

/// One calendarView response's `value` collection.
#[derive(Debug, Deserialize)]
struct GraphCollection<T> {
    value: Vec<T>,
    /// Graph's link to the next page. Its presence is what says the collection is
    /// only part of the window.
    #[serde(rename = "@odata.nextLink")]
    next_link: Option<String>,
}

/// One event from `calendarView`.
///
/// Only the fields the conversion reads are named; a tenant's extra fields and
/// Graph's `@odata.*` annotations are ignored, because Graph adds fields and older
/// payloads lack them.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphEvent {
    id: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    is_all_day: bool,
    #[serde(default)]
    is_cancelled: bool,
    #[serde(default)]
    is_draft: bool,
    show_as: Option<String>,
    response_status: Option<GraphResponseStatus>,
    start: Option<GraphDateTimeTimeZone>,
    end: Option<GraphDateTimeTimeZone>,
    #[serde(rename = "iCalUId")]
    ical_uid: Option<String>,
    categories: Option<Vec<String>>,
    single_value_extended_properties: Option<Vec<GraphExtendedProperty>>,
    /// The `seriesMasterId` an occurrence or exception carries, which marks it as
    /// part of a series even when its `type` is missing.
    series_master_id: Option<String>,
    /// The slot the instance was created for, in UTC. Two occurrences of one
    /// series share an `iCalUId`, so this is what keeps their keys distinct.
    original_start: Option<String>,
}

/// An event's response status: only the user's own answer is read.
#[derive(Debug, Deserialize)]
struct GraphResponseStatus {
    response: Option<String>,
}

/// One `singleValueExtendedProperties` entry.
#[derive(Debug, Deserialize)]
struct GraphExtendedProperty {
    id: Option<String>,
    value: Option<String>,
}

/// A Graph timestamp: wall-clock fields and the zone they are read in.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphDateTimeTimeZone {
    date_time: String,
    time_zone: Option<String>,
}

/// A `getSchedule` response.
#[derive(Debug, Deserialize)]
struct GraphScheduleResponse {
    value: Vec<GraphSchedule>,
}

/// One schedule in a `getSchedule` response.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphSchedule {
    schedule_id: Option<String>,
    schedule_items: Option<Vec<GraphScheduleItem>>,
    error: Option<GraphScheduleError>,
    /// The merged `0`/`1`/`2`/`3` view Graph sends alongside the items. Graph
    /// omits an empty `scheduleItems` collection, so an all-free view is how a
    /// free mailbox answers.
    #[serde(rename = "availabilityView")]
    availability_view: Option<String>,
}

/// One free/busy item: a span and a status, with no event identity.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphScheduleItem {
    status: Option<String>,
    start: Option<GraphDateTimeTimeZone>,
    end: Option<GraphDateTimeTimeZone>,
}

/// The `freeBusyError` a schedule reports in place of its items.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphScheduleError {
    message: Option<String>,
    response_code: Option<String>,
}

/// One mailbox's `workingHours`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphWorkingHours {
    days_of_week: Option<Vec<String>>,
    start_time: Option<String>,
    end_time: Option<String>,
    time_zone: Option<GraphTimeZone>,
}

/// The zone a `workingHours` object is read in, or Graph's custom-zone form.
///
/// A named zone is a `timeZoneBase` with a `name`. A custom zone is a
/// `customTimeZone`: the same `name` (which Graph writes as `Customized Time
/// Zone`), plus `bias` and the standard and daylight offsets. Either the
/// `@odata.type` or the presence of `bias` identifies it, since a payload may carry
/// one without the other.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphTimeZone {
    #[serde(rename = "@odata.type")]
    odata_type: Option<String>,
    name: Option<String>,
    bias: Option<i64>,
}

impl GraphTimeZone {
    /// Whether this is Graph's custom-zone form rather than a named zone.
    ///
    /// Graph identifies the form with `@odata.type` or with the `bias` a custom
    /// zone follows, but some payloads carry only the placeholder name, so the
    /// name is a third signal rather than decoration.
    fn is_custom(&self) -> bool {
        self.bias.is_some()
            || self
                .odata_type
                .as_deref()
                .is_some_and(|kind| kind.strip_prefix('#').unwrap_or(kind) == CUSTOM_TIME_ZONE_TYPE)
            || self
                .name
                .as_deref()
                .is_some_and(|name| name.trim().eq_ignore_ascii_case(CUSTOM_TIME_ZONE_NAME))
    }
}

/// One event's block, or nothing when the event does not occupy time.
fn convert_event(
    participant_zone: &str,
    zones: &mut ZoneCache<'_>,
    event: GraphEvent,
    category_map: Option<&BTreeMap<String, Tag>>,
    diagnostics: &mut Vec<AdapterDiagnostic>,
) -> Result<Option<AnnotatedBlock>, Failure> {
    let GraphEvent {
        id,
        kind,
        is_all_day,
        is_cancelled,
        is_draft,
        show_as,
        response_status,
        start,
        end,
        ical_uid,
        categories,
        single_value_extended_properties,
        series_master_id,
        original_start,
    } = event;
    let shown = id.as_deref().unwrap_or("without an id");
    if kind.as_deref() == Some("seriesMaster") {
        return Err((
            "unexpanded_series",
            format!(
                "the event `{shown}` is a series master: query `calendarView` so Graph \
                 expands a recurring series into the instances inside the window"
            ),
        ));
    }
    if is_cancelled {
        return Ok(None);
    }
    if response_status
        .as_ref()
        .and_then(|status| status.response.as_deref())
        .is_some_and(|response| response.eq_ignore_ascii_case("declined"))
    {
        return Ok(None);
    }
    let subject = Subject {
        label: shown,
        event_id: id.as_deref(),
    };
    let span = if is_all_day {
        all_day_span(
            participant_zone,
            zones,
            start.as_ref(),
            end.as_ref(),
            subject,
            diagnostics,
        )
    } else {
        timed_span(zones, start.as_ref(), end.as_ref(), subject, diagnostics)
    };
    let Some(span) = span? else {
        return Ok(None);
    };

    let (status, status_tag) = map_status(show_as.as_deref().unwrap_or("busy"));
    let (tags, marker_key) = event_tags(
        status_tag,
        is_draft,
        categories,
        category_map,
        single_value_extended_properties,
        id.as_deref(),
        diagnostics,
    );
    let event_key = block_key(
        marker_key,
        ical_uid.as_deref(),
        kind.as_deref(),
        series_master_id.as_deref(),
        original_start.as_deref(),
    );

    let mut block = Block::new(span).with_status(status).with_tags(tags);
    if is_all_day {
        block = block.with_all_day(true);
    }
    if let Some(key) = event_key {
        block = block
            .with_event_key(key)
            .map_err(|err| (err.code(), format!("the event `{shown}`: {err}")))?;
    }
    Ok(Some(AnnotatedBlock {
        event_id: id,
        block,
    }))
}

/// The block's tags and the caller's marker key, from one event's fields.
fn event_tags(
    status_tag: Option<&'static str>,
    is_draft: bool,
    categories: Option<Vec<String>>,
    category_map: Option<&BTreeMap<String, Tag>>,
    properties: Option<Vec<GraphExtendedProperty>>,
    event_id: Option<&str>,
    diagnostics: &mut Vec<AdapterDiagnostic>,
) -> (Vec<Tag>, Option<String>) {
    let mut marker_tags = Vec::new();
    let mut marker_key = None;
    for property in properties.unwrap_or_default() {
        let Some(name) = property.id.as_deref().and_then(extended_property_name) else {
            continue;
        };
        let Some(value) = property.value.as_deref() else {
            continue;
        };
        if name == MARKER_TAGS {
            marker_tags.extend(parse_marker_tags(value, event_id, diagnostics));
        } else if name == MARKER_EVENT_KEY {
            let key = value.trim();
            if !key.is_empty() {
                marker_key = Some(key.to_owned());
            }
        }
    }
    let mut tags = Vec::new();
    if let Some(tag) = status_tag {
        tags.push(own_tag(tag));
    }
    if is_draft {
        tags.push(own_tag(TAG_DRAFT));
    }
    let categories = categories.unwrap_or_default();
    tags.extend(categories_to_tags(
        &categories,
        category_map,
        event_id,
        diagnostics,
    ));
    tags.extend(marker_tags);
    (unique_tags(tags), marker_key)
}

/// The block's event key: the caller's marker, then the provider's identifier,
/// with the original slot a recurring instance carries.
fn block_key(
    marker_key: Option<String>,
    ical_uid: Option<&str>,
    kind: Option<&str>,
    series_master_id: Option<&str>,
    original_start: Option<&str>,
) -> Option<String> {
    // The caller's marker wins, then the provider's identifier. Either way a
    // recurring instance carries its original slot, because Graph shares one
    // `iCalUId` across a series.
    let mut key = marker_key.or_else(|| {
        ical_uid
            .map(str::trim)
            .filter(|uid| !uid.is_empty())
            .map(str::to_owned)
    })?;
    if let Some(discriminator) = instance_discriminator(kind, series_master_id, original_start) {
        key.push('@');
        key.push_str(&discriminator);
    }
    Some(key)
}

/// The discriminator a recurring instance's event key carries, when the event is
/// one.
///
/// Graph writes one `iCalUId` for a recurring series (its own reference and its
/// own announcement have described both behaviours), so the original slot is
/// what keeps each occurrence a distinct event for cap counting and rescheduling.
/// An event with no original start cannot be told apart from its siblings and
/// keeps its key as written.
fn instance_discriminator(
    kind: Option<&str>,
    series_master_id: Option<&str>,
    original_start: Option<&str>,
) -> Option<String> {
    let instance = original_start.is_some_and(|start| !start.trim().is_empty())
        || series_master_id.is_some_and(|id| !id.trim().is_empty())
        || kind.is_some_and(|kind| {
            kind.eq_ignore_ascii_case("occurrence") || kind.eq_ignore_ascii_case("exception")
        });
    if !instance {
        return None;
    }
    original_start.and_then(original_start_key)
}

/// A Graph `originalStart`, normalised to the UTC rendering a key carries.
///
/// The property is a UTC `DateTimeOffset`, so the common path only rewrites its
/// fractional digits. A reading that cannot be parsed is used as written: it is
/// still unique per instance, which is the property the key needs.
fn original_start_key(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if !Stamp::has_offset(raw) {
        return Some(raw.to_owned());
    }
    Some(
        Stamp::parse(raw)
            .and_then(Stamp::utc_key)
            .unwrap_or_else(|_| raw.to_owned()),
    )
}

/// The marker name an extended property's `id` carries, when it belongs to
/// opportune's namespace.
///
/// Graph writes the id as `String {guid} Name {name}`. The namespace GUID comes
/// back in the case the property was created with and the name is matched
/// case-sensitively, so the GUID is compared case-insensitively and the name is
/// returned as Graph wrote it for the caller to compare exactly.
fn extended_property_name(id: &str) -> Option<&str> {
    let rest = id.strip_prefix("String {")?;
    let (guid, name) = rest.split_once("} Name ")?;
    guid.eq_ignore_ascii_case(EXTENDED_PROPERTY_NAMESPACE_GUID)
        .then_some(name)
}

/// One all-day event's span, resolved from its civil dates in the participant's
/// zone.
fn all_day_span(
    participant_zone: &str,
    zones: &mut ZoneCache<'_>,
    start: Option<&GraphDateTimeTimeZone>,
    end: Option<&GraphDateTimeTimeZone>,
    subject: Subject<'_>,
    diagnostics: &mut Vec<AdapterDiagnostic>,
) -> Result<Option<Span>, Failure> {
    let id = subject.label;
    let start = start.ok_or_else(|| {
        (
            "payload_invalid",
            format!("the all-day event `{id}` has no start"),
        )
    })?;
    let end = end.ok_or_else(|| {
        (
            "payload_invalid",
            format!("the all-day event `{id}` has no end"),
        )
    })?;
    // Resolved here rather than up front: a timed-only calendar never reads the
    // participant zone, so a name nothing resolves only fails a calendar that
    // needs it.
    let participant = zones
        .resolve(participant_zone)
        .map_err(|err| (err.code(), err.to_string()))?;
    let resolve = |stamp: &GraphDateTimeTimeZone| {
        let date = LocalStamp::parse(&stamp.date_time)
            .map_err(|message| {
                (
                    "payload_invalid",
                    format!("the all-day event `{id}`: {message}"),
                )
            })?
            .date;
        participant
            .day_start(date)
            .map(Resolved::of)
            .map_err(|err| {
                (
                    "payload_invalid",
                    format!("the all-day event `{id}` begins outside the supported range: {err}"),
                )
            })
    };
    finish_span(resolve(start)?, resolve(end)?, subject, diagnostics)
}

/// One timed span's instants, each read in the zone its own timestamp names.
fn timed_span(
    zones: &mut ZoneCache<'_>,
    start: Option<&GraphDateTimeTimeZone>,
    end: Option<&GraphDateTimeTimeZone>,
    subject: Subject<'_>,
    diagnostics: &mut Vec<AdapterDiagnostic>,
) -> Result<Option<Span>, Failure> {
    let id = subject.label;
    let start =
        start.ok_or_else(|| ("payload_invalid", format!("the event `{id}` has no start")))?;
    let end = end.ok_or_else(|| ("payload_invalid", format!("the event `{id}` has no end")))?;
    let start = resolve_timestamp(zones, start)?;
    let end = resolve_timestamp(zones, end)?;
    finish_span(start, end, subject, diagnostics)
}

/// One Graph timestamp resolved through the zone it names.
fn resolve_timestamp(
    zones: &mut ZoneCache<'_>,
    stamp: &GraphDateTimeTimeZone,
) -> Result<Resolved, Failure> {
    let zone_name = stamp.time_zone.as_deref().ok_or_else(|| {
        (
            "payload_invalid",
            format!("the timestamp `{}` names no time zone", stamp.date_time),
        )
    })?;
    let zone = zones
        .resolve(zone_name)
        .map_err(|err| (err.code(), err.to_string()))?;
    LocalStamp::parse(&stamp.date_time)
        .and_then(|local| local.resolve(zone))
        .map_err(|message| ("payload_invalid", message))
}

/// One schedule's blocks, or the provider's error.
fn schedule_to_outcome(
    zones: &mut ZoneCache<'_>,
    items: Vec<GraphScheduleItem>,
) -> CalendarOutcome {
    let mut blocks = Vec::new();
    let mut diagnostics = Vec::new();
    for (index, item) in items.into_iter().enumerate() {
        let shown = format!("schedule item {index}");
        let subject = Subject {
            label: &shown,
            event_id: None,
        };
        let span = match timed_span(
            zones,
            item.start.as_ref(),
            item.end.as_ref(),
            subject,
            &mut diagnostics,
        ) {
            Ok(Some(span)) => span,
            Ok(None) => continue,
            Err((code, message)) => return unavailable(code, message),
        };
        let (status, status_tag) = map_status(item.status.as_deref().unwrap_or("busy"));
        let tags = status_tag.map_or_else(Vec::new, |tag| vec![own_tag(tag)]);
        blocks.push(AnnotatedBlock {
            event_id: None,
            block: Block::new(span).with_status(status).with_tags(tags),
        });
    }
    CalendarOutcome::Available {
        blocks,
        diagnostics,
    }
}

/// The status a provider status name means, and the tag it carries.
///
/// The names are matched case-insensitively because Graph's own payloads spell
/// them differently by endpoint: `workingElsewhere` from `calendarView`,
/// `WorkingElsewhere` from a case-preserving proxy. A name outside the six Graph
/// defines reads as `busy`: blocking by default is the safe reading of a value the
/// engine cannot interpret.
fn map_status(raw: &str) -> (BlockStatus, Option<&'static str>) {
    if raw.eq_ignore_ascii_case("free") {
        (BlockStatus::Free, None)
    } else if raw.eq_ignore_ascii_case("tentative") {
        (BlockStatus::Tentative, None)
    } else if raw.eq_ignore_ascii_case("oof") {
        (BlockStatus::Busy, Some(TAG_OOF))
    } else if raw.eq_ignore_ascii_case("workingelsewhere") {
        (BlockStatus::Free, Some(TAG_WORKING_ELSEWHERE))
    } else if raw.eq_ignore_ascii_case("unknown") {
        (BlockStatus::Busy, Some(TAG_UNKNOWN))
    } else {
        (BlockStatus::Busy, None)
    }
}

/// The weekdays Graph's `daysOfWeek` names.
const WEEKDAYS: [(&str, Weekday); 7] = [
    ("sunday", Weekday::Sunday),
    ("monday", Weekday::Monday),
    ("tuesday", Weekday::Tuesday),
    ("wednesday", Weekday::Wednesday),
    ("thursday", Weekday::Thursday),
    ("friday", Weekday::Friday),
    ("saturday", Weekday::Saturday),
];

/// The weekdays a `workingHours` payload names.
fn working_days(names: Option<&[String]>) -> Result<Weekdays, WorkingHoursError> {
    let names = names.filter(|names| !names.is_empty()).ok_or_else(|| {
        invalid_working_hours("the working hours name no days, so the window would close every day")
    })?;
    let mut days = Vec::with_capacity(names.len());
    for name in names {
        let matched = WEEKDAYS
            .iter()
            .find(|(day, _)| day.eq_ignore_ascii_case(name.trim()))
            .map(|(_, day)| *day)
            .ok_or_else(|| invalid_working_hours(format!("`{name}` is not a day of the week")))?;
        days.push(matched);
    }
    Ok(Weekdays::list(days))
}

/// The single wall-clock range a `workingHours` payload describes.
fn working_range(start: Option<&str>, end: Option<&str>) -> Result<WallRange, WorkingHoursError> {
    let start = working_time(start, "startTime")?;
    let end = working_time(end, "endTime")?;
    // A `00:00` end is the end of the working day, not the start of an empty
    // range. `00:00` to `00:00` then names no working time at all, which is a
    // payload defect rather than a day that is open around the clock.
    if start == TimeOfDay::MIDNIGHT && end == TimeOfDay::MIDNIGHT {
        return Err(invalid_working_hours(
            "the working day starts and ends at 00:00, which names no range",
        ));
    }
    let end = if end == TimeOfDay::MIDNIGHT {
        WallEnd::Midnight
    } else {
        WallEnd::Time(end)
    };
    WallRange::new(start, end).map_err(|err| invalid_working_hours(err.to_string()))
}

/// One `workingHours` time of day.
///
/// Graph's `Edm.TimeOfDay` is written `09:00:00.0000000`, and the engine reads
/// whole minutes. A sub-minute component is rejected rather than truncated:
/// silently moving a working day's edge is a wrong answer, while failing loudly
/// lets the caller keep its own window.
fn working_time(raw: Option<&str>, field: &str) -> Result<TimeOfDay, WorkingHoursError> {
    let raw =
        raw.ok_or_else(|| invalid_working_hours(format!("the working hours has no {field}")))?;
    let invalid = || format!("`{raw}` is not a time of day");
    let (time, second, nanos) = parse_clock(raw, raw, &invalid).map_err(invalid_working_hours)?;
    if second != 0 || nanos != 0 {
        return Err(invalid_working_hours(format!(
            "`{raw}` is not on a whole minute"
        )));
    }
    Ok(time)
}

/// A working-hours failure with `message` as its cause.
fn invalid_working_hours(message: impl Into<String>) -> WorkingHoursError {
    WorkingHoursError::InvalidWorkingHours {
        message: message.into(),
    }
}
