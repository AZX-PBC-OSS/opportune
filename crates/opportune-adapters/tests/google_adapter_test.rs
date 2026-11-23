//! Google `events.list` and `freeBusy.query` conversions.

// Every case resolves zone names against a real database, so the suite runs where
// the crate compiles one in. `make test-matrix` runs the `graph`-only build with
// `bundled-tzdb` on, because no zone data exists to convert against without it;
// the bundle-free build is gated by `cargo check` and by the dependency tree.
#![cfg(all(feature = "google", feature = "bundled-tzdb"))]

use opportune::model::{Block, BlockStatus};
use opportune::tags::Tag;
use opportune::time::{TimeZoneDatabase, ZoneName};
use opportune_adapters::google::{events_list_to_blocks, free_busy_to_blocks};
use opportune_adapters::{AdapterDiagnostic, AnnotatedBlock, CalendarOutcome};
use serde_json::{Value, json};

/// The bundled database every conversion resolves names against.
fn db() -> TimeZoneDatabase {
    TimeZoneDatabase::bundled().expect("the bundled database is compiled in")
}

fn zone(name: &str) -> ZoneName {
    ZoneName::new(name).unwrap()
}

/// The blocks and diagnostics of a calendar that converted.
fn available(outcome: CalendarOutcome) -> (Vec<AnnotatedBlock>, Vec<AdapterDiagnostic>) {
    match outcome {
        CalendarOutcome::Available {
            blocks,
            diagnostics,
        } => (blocks, diagnostics),
        CalendarOutcome::Unavailable { code, message } => {
            panic!("the calendar should convert, not fail with `{code}`: {message}");
        }
    }
}

/// The code and message of a calendar that did not convert.
fn unavailable(outcome: CalendarOutcome) -> (&'static str, String) {
    match outcome {
        CalendarOutcome::Unavailable { code, message } => (code, message),
        CalendarOutcome::Available { blocks, .. } => {
            panic!(
                "the calendar should not convert, but produced {} blocks",
                blocks.len()
            )
        }
    }
}

/// One calendar's blocks, converted with the participant in UTC.
fn converted(payload: &str) -> Vec<AnnotatedBlock> {
    let (blocks, diagnostics) = available(events_list_to_blocks(&db(), payload, &zone("UTC")));
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    blocks
}

/// The block for `event_id` among the blocks a calendar converted.
fn block_for(blocks: &[AnnotatedBlock], event_id: &str) -> Block {
    blocks
        .iter()
        .find(|annotated| annotated.event_id.as_deref() == Some(event_id))
        .unwrap_or_else(|| panic!("no block for `{event_id}`"))
        .block
        .clone()
}

/// The tags of a block, as strings.
fn tag_names(block: &Block) -> Vec<&str> {
    block.tags.iter().map(Tag::as_str).collect()
}

/// An `events.list` response carrying `items`, with the calendar zone in UTC.
fn events(items: &[Value]) -> String {
    json!({ "timeZone": "UTC", "items": items }).to_string()
}

/// One confirmed, opaque event as Google writes it.
fn timed(id: &str, start: &str, end: &str) -> Value {
    json!({
        "id": id,
        "status": "confirmed",
        "transparency": "opaque",
        "iCalUID": format!("{id}@example.com"),
        "start": { "dateTime": start },
        "end": { "dateTime": end }
    })
}

/// A timed event that ends before it starts cannot describe a block, so the calendar
/// fails closed rather than answering with the events that did convert.
#[test]
fn a_reversed_timed_span_fails_the_calendar() {
    let payload = events(&[timed(
        "evt-reversed",
        "2027-03-09T15:00:00Z",
        "2027-03-09T14:00:00Z",
    )]);
    let (code, message) = unavailable(events_list_to_blocks(&db(), &payload, &zone("UTC")));
    assert_eq!(code, "payload_invalid");
    assert!(message.contains("evt-reversed"), "{message}");
}

#[test]
fn a_timed_event_resolves_from_the_offset_its_timestamp_carries() {
    let payload = events(&[
        // An attendee in New York reads the same instant as one in Tokyo.
        timed(
            "evt-new-york",
            "2027-03-09T10:00:00-05:00",
            "2027-03-09T11:00:00-05:00",
        ),
        timed(
            "evt-tokyo",
            "2027-03-10T00:00:00+09:00",
            "2027-03-10T01:00:00+09:00",
        ),
        // Google writes RFC 3339 without the second when it is zero, and keeps a
        // fraction when the source had one.
        timed("evt-seconds", "2027-03-09T12:00:00Z", "2027-03-09T12:30Z"),
        timed(
            "evt-fraction",
            "2027-03-09T13:00:45.500Z",
            "2027-03-09T13:30:01.250Z",
        ),
    ]);
    let blocks = converted(&payload);
    assert_eq!(
        block_for(&blocks, "evt-new-york").span.start().to_string(),
        "2027-03-09T15:00Z"
    );
    assert_eq!(
        block_for(&blocks, "evt-tokyo").span.start().to_string(),
        "2027-03-09T15:00Z",
        "a +09:00 copy names the same instant as a -05:00 one"
    );
    assert_eq!(
        block_for(&blocks, "evt-seconds").span.end().to_string(),
        "2027-03-09T12:30Z"
    );
    // 13:00:45 rounds down and 13:30:01.250 rounds up, outward from the instant.
    let fraction = block_for(&blocks, "evt-fraction");
    assert_eq!(fraction.span.start().to_string(), "2027-03-09T13:00Z");
    assert_eq!(fraction.span.end().to_string(), "2027-03-09T13:31Z");
}

#[test]
fn a_zero_length_event_is_dropped_before_the_endpoints_round_outward() {
    let payload = events(&[
        timed(
            "evt-reminder",
            "2027-03-09T10:00:30-05:00",
            "2027-03-09T10:00:30-05:00",
        ),
        timed(
            "evt-half-minute",
            "2027-03-09T11:00:00Z",
            "2027-03-09T11:00:30Z",
        ),
    ]);
    let (blocks, diagnostics) = available(events_list_to_blocks(&db(), &payload, &zone("UTC")));
    // The zero-length event is dropped, not widened into `[15:00, 15:01)`.
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].event_id.as_deref(), Some("evt-half-minute"));
    assert_eq!(
        blocks[0].block.span.start().to_string(),
        "2027-03-09T11:00Z"
    );
    assert_eq!(blocks[0].block.span.end().to_string(), "2027-03-09T11:01Z");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, "zero_length_event");
    assert_eq!(diagnostics[0].event_id.as_deref(), Some("evt-reminder"));
}

#[test]
fn a_cancelled_event_is_dropped_before_its_times_are_read() {
    // A cancelled event may carry no `start`/`end` at all, so reading them first
    // would fail the whole calendar over an event that occupies nothing.
    let payload = events(&[
        json!({ "id": "evt-cancelled", "status": "cancelled", "iCalUID": "cancelled@example.com" }),
        // A cancelled series master still names the wrong endpoint, but it
        // occupies nothing, so it is dropped like any other cancelled event.
        json!({
            "id": "evt-cancelled-series",
            "status": "cancelled",
            "recurrence": ["RRULE:FREQ=WEEKLY;BYDAY=TU"],
            "iCalUID": "cancelled-series@example.com"
        }),
        timed("evt-kept", "2027-03-09T09:00:00Z", "2027-03-09T10:00:00Z"),
    ]);
    let (blocks, diagnostics) = available(events_list_to_blocks(&db(), &payload, &zone("UTC")));
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].event_id.as_deref(), Some("evt-kept"));
    assert!(diagnostics.is_empty());
}

#[test]
fn the_five_step_status_precedence_decides_each_event() {
    let cases = [
        // (id, transparency, own response, expected status, expected tags)
        (
            "evt-opaque-accepted",
            "opaque",
            Some("accepted"),
            BlockStatus::Busy,
            vec![],
        ),
        (
            "evt-opaque-tentative",
            "opaque",
            Some("tentative"),
            BlockStatus::Tentative,
            vec![],
        ),
        (
            "evt-opaque-needs-action",
            "opaque",
            Some("needsAction"),
            BlockStatus::Tentative,
            vec!["status/needs-action"],
        ),
        (
            "evt-transparent-accepted",
            "transparent",
            Some("accepted"),
            BlockStatus::Free,
            vec![],
        ),
        // Transparency is checked before the user's own answer.
        (
            "evt-transparent-tentative",
            "transparent",
            Some("tentative"),
            BlockStatus::Free,
            vec![],
        ),
        (
            "evt-transparent-needs-action",
            "transparent",
            Some("needsAction"),
            BlockStatus::Free,
            vec![],
        ),
        // No attendee list at all is the organiser's own event: it blocks.
        (
            "evt-no-attendees",
            "opaque",
            None,
            BlockStatus::Busy,
            vec![],
        ),
    ];
    let mut items = Vec::new();
    for (index, (id, transparency, response, _, _)) in cases.iter().enumerate() {
        let hour = 9 + u8::try_from(index).unwrap();
        let mut event = timed(
            id,
            &format!("2027-03-09T{hour:02}:00:00Z"),
            &format!("2027-03-09T{hour:02}:30:00Z"),
        );
        event["transparency"] = json!(transparency);
        if let Some(response) = response {
            event["attendees"] =
                json!([{ "email": "user@example.com", "self": true, "responseStatus": response }]);
        }
        items.push(event);
    }
    // A declined invitation is dropped before transparency is considered.
    items.push(json!({
        "id": "evt-declined",
        "status": "confirmed",
        "transparency": "transparent",
        "iCalUID": "evt-declined@example.com",
        "start": { "dateTime": "2027-03-09T18:00:00Z" },
        "end": { "dateTime": "2027-03-09T19:00:00Z" },
        "attendees": [{ "email": "user@example.com", "self": true, "responseStatus": "declined" }]
    }));
    let payload = events(&items);

    let (blocks, diagnostics) = available(events_list_to_blocks(&db(), &payload, &zone("UTC")));
    assert!(diagnostics.is_empty());
    assert_eq!(blocks.len(), cases.len());
    for (id, transparency, response, status, tags) in cases {
        let block = block_for(&blocks, id);
        assert_eq!(
            block.status, status,
            "`{id}` (transparency `{transparency}`, own response `{response:?}`)"
        );
        assert_eq!(tag_names(&block), tags, "`{id}`");
    }

    // Google's own `tentative` is the organiser's reading, and the five steps read
    // only the user's own answer, so an event nobody declined blocks.
    let mut organiser_tentative = timed(
        "evt-organiser-tentative",
        "2027-03-09T19:00:00Z",
        "2027-03-09T20:00:00Z",
    );
    organiser_tentative["status"] = json!("tentative");
    organiser_tentative["attendees"] =
        json!([{ "email": "user@example.com", "self": true, "responseStatus": "accepted" }]);
    let blocks = converted(&events(&[organiser_tentative]));
    assert_eq!(
        block_for(&blocks, "evt-organiser-tentative").status,
        BlockStatus::Busy
    );
}

#[test]
fn an_omitted_attendee_list_reads_as_busy_unless_the_event_is_transparent() {
    let payload = events(&[
        json!({
            "id": "evt-opaque-omitted",
            "status": "confirmed",
            "transparency": "opaque",
            "attendeesOmitted": true,
            "iCalUID": "opaque-omitted@example.com",
            "start": { "dateTime": "2027-03-09T09:00:00Z" },
            "end": { "dateTime": "2027-03-09T10:00:00Z" }
        }),
        // Transparency is decided first, so an omitted transparent event is free
        // — and still says why the user's own answer is unknown.
        json!({
            "id": "evt-transparent-omitted",
            "status": "confirmed",
            "transparency": "transparent",
            "attendeesOmitted": true,
            "iCalUID": "transparent-omitted@example.com",
            "start": { "dateTime": "2027-03-09T10:00:00Z" },
            "end": { "dateTime": "2027-03-09T11:00:00Z" }
        }),
    ]);
    let (blocks, diagnostics) = available(events_list_to_blocks(&db(), &payload, &zone("UTC")));
    assert_eq!(
        block_for(&blocks, "evt-opaque-omitted").status,
        BlockStatus::Busy
    );
    assert_eq!(
        block_for(&blocks, "evt-transparent-omitted").status,
        BlockStatus::Free
    );
    let codes: Vec<&str> = diagnostics.iter().map(|finding| finding.code).collect();
    assert_eq!(codes, ["attendees_omitted", "attendees_omitted"]);
    assert_eq!(
        diagnostics[0].event_id.as_deref(),
        Some("evt-opaque-omitted")
    );
    assert_eq!(
        diagnostics[1].event_id.as_deref(),
        Some("evt-transparent-omitted")
    );
}

#[test]
fn every_event_type_gets_its_status_tag() {
    let all_day = |id: &str, event_type: &str, transparency: &str| {
        json!({
            "id": id,
            "status": "confirmed",
            "eventType": event_type,
            "transparency": transparency,
            "iCalUID": format!("{id}@example.com"),
            "start": { "date": "2027-03-09" },
            "end": { "date": "2027-03-10" }
        })
    };
    let mut items = vec![
        all_day("evt-working-location", "workingLocation", "transparent"),
        all_day("evt-birthday", "birthday", "transparent"),
    ];
    let mut timed_event = |id: &str, event_type: &str| {
        let mut event = timed(id, "2027-03-09T09:00:00Z", "2027-03-09T10:00:00Z");
        event["eventType"] = json!(event_type);
        items.push(event);
    };
    timed_event("evt-ooo", "outOfOffice");
    timed_event("evt-focus", "focusTime");
    timed_event("evt-from-gmail", "fromGmail");
    timed_event("evt-default", "default");
    // An event with no `eventType` at all is Google's own default.
    items.push(timed(
        "evt-untyped",
        "2027-03-09T11:00:00Z",
        "2027-03-09T12:00:00Z",
    ));

    let payload = events(&items);
    let blocks = converted(&payload);
    let cases = [
        ("evt-working-location", vec!["status/working-location"]),
        ("evt-birthday", vec!["status/birthday"]),
        ("evt-ooo", vec!["status/oof"]),
        ("evt-focus", vec!["status/focus"]),
        ("evt-from-gmail", vec!["status/from-gmail"]),
        ("evt-default", vec![]),
        ("evt-untyped", vec![]),
    ];
    for (id, tags) in cases {
        assert_eq!(tag_names(&block_for(&blocks, id)), tags, "`{id}`");
    }
    assert_eq!(
        block_for(&blocks, "evt-working-location").status,
        BlockStatus::Free,
        "a transparent working-location day does not block"
    );
}

#[test]
fn a_recurrence_fails_the_calendar_rather_than_treating_a_master_as_an_instance() {
    let payload = events(&[
        timed("evt-ok", "2027-03-09T09:00:00Z", "2027-03-09T10:00:00Z"),
        json!({
            "id": "evt-series",
            "status": "confirmed",
            "transparency": "opaque",
            "iCalUID": "series@example.com",
            "recurrence": ["RRULE:FREQ=WEEKLY;BYDAY=TU"],
            "start": { "dateTime": "2027-03-09T11:00:00Z" },
            "end": { "dateTime": "2027-03-09T12:00:00Z" }
        }),
    ]);
    let (code, message) = unavailable(events_list_to_blocks(&db(), &payload, &zone("UTC")));
    assert_eq!(code, "unexpanded_series");
    assert!(message.contains("singleEvents=true"), "{message}");

    // An empty `recurrence` names no series.
    let mut single = timed("evt-single", "2027-03-09T09:00:00Z", "2027-03-09T10:00:00Z");
    single["recurrence"] = json!([]);
    let blocks = converted(&events(&[single]));
    assert_eq!(blocks.len(), 1);
}

#[test]
fn an_unspecified_end_keeps_the_end_google_still_sent() {
    let mut event = timed(
        "evt-open-ended",
        "2027-03-09T09:00:00Z",
        "2027-03-09T10:00:00Z",
    );
    event["endTimeUnspecified"] = json!(true);
    let payload = events(&[event]);
    let (blocks, diagnostics) = available(events_list_to_blocks(&db(), &payload, &zone("UTC")));
    assert_eq!(blocks[0].block.span.end().to_string(), "2027-03-09T10:00Z");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, "end_time_unspecified");
    assert_eq!(diagnostics[0].event_id.as_deref(), Some("evt-open-ended"));
}

#[test]
fn an_all_day_event_is_read_from_its_civil_dates_in_the_responses_zone() {
    // The response names the calendar's zone, and the civil dates are resolved
    // there rather than in UTC or the participant's zone.
    let payload = json!({
        "timeZone": "America/New_York",
        "items": [
            {
                "id": "evt-all-day",
                "status": "confirmed",
                "transparency": "opaque",
                "iCalUID": "all-day@example.com",
                "start": { "date": "2027-03-09" },
                "end": { "date": "2027-03-10" }
            },
            {
                "id": "evt-spring-forward",
                "status": "confirmed",
                "transparency": "opaque",
                "iCalUID": "spring-forward@example.com",
                "start": { "date": "2027-03-14" },
                "end": { "date": "2027-03-15" }
            }
        ]
    })
    .to_string();
    let (blocks, diagnostics) = available(events_list_to_blocks(
        &db(),
        &payload,
        &zone("Europe/London"),
    ));
    assert!(diagnostics.is_empty());
    let all_day = block_for(&blocks, "evt-all-day");
    assert!(all_day.all_day);
    assert_eq!(all_day.span.start().to_string(), "2027-03-09T05:00Z");
    assert_eq!(all_day.span.end().to_string(), "2027-03-10T05:00Z");
    // The spring-forward day is 23 hours long: 00:00 EST to 00:00 EDT.
    let spring = block_for(&blocks, "evt-spring-forward");
    assert!(spring.all_day);
    assert_eq!(spring.span.minutes(), 23 * 60);

    // Without a response zone, the participant's own zone decides the day.
    let payload = json!({
        "items": [{
            "id": "evt-all-day",
            "status": "confirmed",
            "iCalUID": "all-day@example.com",
            "start": { "date": "2027-03-09" },
            "end": { "date": "2027-03-10" }
        }]
    })
    .to_string();
    let blocks = {
        let (blocks, _) = available(events_list_to_blocks(
            &db(),
            &payload,
            &zone("Europe/London"),
        ));
        blocks
    };
    assert_eq!(
        block_for(&blocks, "evt-all-day").span.start().to_string(),
        "2027-03-09T00:00Z"
    );
}

#[test]
fn a_zone_nothing_resolves_fails_only_a_calendar_that_needs_it() {
    // A timed event carries its own offset, so neither the participant's zone nor
    // the response's zone is read: an unusable name for either leaves it alone.
    let payload = events(&[timed(
        "evt-timed",
        "2027-03-09T09:00:00Z",
        "2027-03-09T10:00:00Z",
    )]);
    let (blocks, diagnostics) = available(events_list_to_blocks(
        &db(),
        &payload,
        &zone("Mars/Olympus_Mons"),
    ));
    assert_eq!(blocks.len(), 1);
    assert!(diagnostics.is_empty());

    // An all-day event needs a zone. The response's is the calendar's own, so a
    // name nothing resolves fails the calendar rather than reading the day in UTC.
    let all_day = json!({
        "timeZone": "Mars/Olympus_Mons",
        "items": [{
            "id": "evt-all-day",
            "status": "confirmed",
            "iCalUID": "all-day@example.com",
            "start": { "date": "2027-03-09" },
            "end": { "date": "2027-03-10" }
        }]
    })
    .to_string();
    let (code, message) = unavailable(events_list_to_blocks(&db(), &all_day, &zone("UTC")));
    assert_eq!(code, "zone_unknown");
    assert!(message.contains("Mars/Olympus_Mons"), "{message}");

    // Without one, the participant's zone is the fallback, so it is what fails.
    let all_day_no_zone = json!({
        "items": [{
            "id": "evt-all-day",
            "status": "confirmed",
            "iCalUID": "all-day@example.com",
            "start": { "date": "2027-03-09" },
            "end": { "date": "2027-03-10" }
        }]
    })
    .to_string();
    let (code, message) = unavailable(events_list_to_blocks(
        &db(),
        &all_day_no_zone,
        &zone("Mars/Olympus_Mons"),
    ));
    assert_eq!(code, "zone_unknown");
    assert!(message.contains("Mars/Olympus_Mons"), "{message}");

    // Timed events carry offsets, so they convert whatever the response zone says.
    let timed_only = json!({
        "timeZone": "Mars/Olympus_Mons",
        "items": [{
            "id": "evt-timed",
            "status": "confirmed",
            "iCalUID": "timed@example.com",
            "start": { "dateTime": "2027-03-09T09:00:00Z" },
            "end": { "dateTime": "2027-03-09T10:00:00Z" }
        }]
    })
    .to_string();
    let blocks = converted(&timed_only);
    assert_eq!(blocks.len(), 1);
}

#[test]
fn a_recurring_instance_key_is_the_same_for_attendees_in_different_zones() {
    let instance = |id: &str, original: &str, start: &str, end: &str| {
        json!({
            "id": id,
            "status": "confirmed",
            "transparency": "opaque",
            "recurringEventId": "series-interview",
            "iCalUID": "series-interview@example.com",
            "originalStartTime": { "dateTime": original },
            "start": { "dateTime": start },
            "end": { "dateTime": end }
        })
    };
    let payload = events(&[
        // One instance as two attendees receive it.
        instance(
            "evt-ana",
            "2027-03-09T10:00:00-05:00",
            "2027-03-09T10:00:00-05:00",
            "2027-03-09T11:00:00-05:00",
        ),
        instance(
            "evt-bo",
            "2027-03-10T00:00:00+09:00",
            "2027-03-10T00:00:00+09:00",
            "2027-03-10T01:00:00+09:00",
        ),
        // A different occurrence of the same series.
        instance(
            "evt-next",
            "2027-03-10T10:00:00-05:00",
            "2027-03-10T10:00:00-05:00",
            "2027-03-10T11:00:00-05:00",
        ),
    ]);
    let blocks = converted(&payload);
    let ana = block_for(&blocks, "evt-ana");
    let bo = block_for(&blocks, "evt-bo");
    let expected = Some("series-interview@example.com@2027-03-09T15:00:00Z");
    assert_eq!(ana.event_key.as_deref(), expected);
    assert_eq!(
        bo.event_key.as_deref(),
        expected,
        "the same occurrence derives one key whatever offset the copy carries"
    );
    assert_ne!(
        block_for(&blocks, "evt-next").event_key,
        ana.event_key,
        "distinct occurrences derive distinct keys"
    );

    // A non-recurring event has no `originalStartTime`, so its `iCalUID` alone is
    // the key; an all-day instance carries a civil date instead of a timestamp.
    let payload = events(&[
        timed("evt-single", "2027-03-09T09:00:00Z", "2027-03-09T10:00:00Z"),
        json!({
            "id": "evt-all-day-instance",
            "status": "confirmed",
            "transparency": "opaque",
            "iCalUID": "series-all-day@example.com",
            "originalStartTime": { "date": "2027-03-09" },
            "start": { "date": "2027-03-09" },
            "end": { "date": "2027-03-10" }
        }),
    ]);
    let blocks = converted(&payload);
    assert_eq!(
        block_for(&blocks, "evt-single").event_key.as_deref(),
        Some("evt-single@example.com")
    );
    assert_eq!(
        block_for(&blocks, "evt-all-day-instance")
            .event_key
            .as_deref(),
        Some("series-all-day@example.com@2027-03-09")
    );
}

#[test]
fn a_sub_minute_original_start_still_derives_one_key_per_occurrence() {
    // `utc_key` keeps the second and any fraction the original carried rather than
    // emitting the floored minute, so an `events.import`ed series whose DTSTART
    // carries seconds still derives one key per occurrence, identically in every
    // zone. The key is a value callers compare and persist, so its spelling is a
    // contract, not a detail.
    let instance = |id: &str, original: &str, start: &str, end: &str| {
        json!({
            "id": id,
            "status": "confirmed",
            "transparency": "opaque",
            "recurringEventId": "series-imported",
            "iCalUID": "series-imported@example.com",
            "originalStartTime": { "dateTime": original },
            "start": { "dateTime": start },
            "end": { "dateTime": end }
        })
    };
    // Two attendees' copies of one occurrence, stamped in different offsets, and
    // the next weekly occurrence of the same series.
    let payload = events(&[
        instance(
            "evt-ana",
            "2027-03-09T10:00:30.250-05:00",
            "2027-03-09T10:00:30.250-05:00",
            "2027-03-09T11:00:30.250-05:00",
        ),
        instance(
            "evt-bo",
            "2027-03-10T00:00:30.250+09:00",
            "2027-03-10T00:00:30.250+09:00",
            "2027-03-10T01:00:30.250+09:00",
        ),
        instance(
            "evt-next",
            "2027-03-16T10:00:30.250-04:00",
            "2027-03-16T10:00:30.250-04:00",
            "2027-03-16T11:00:30.250-04:00",
        ),
    ]);
    let blocks = converted(&payload);
    let ana = block_for(&blocks, "evt-ana");
    assert_eq!(
        ana.event_key.as_deref(),
        Some("series-imported@example.com@2027-03-09T15:00:30.25Z"),
        "the key keeps the second and the fraction the original carried"
    );
    assert_eq!(
        block_for(&blocks, "evt-bo").event_key,
        ana.event_key,
        "the same sub-minute occurrence derives one key whatever offset it carries"
    );
    assert_eq!(
        block_for(&blocks, "evt-next").event_key.as_deref(),
        Some("series-imported@example.com@2027-03-16T14:00:30.25Z"),
        "the next occurrence of the series derives its own key"
    );
}

#[test]
fn the_shared_marker_wins_the_key_and_the_tag_lists_merge() {
    let payload = events(&[
        json!({
            "id": "evt-both",
            "status": "confirmed",
            "transparency": "opaque",
            "iCalUID": "both@example.com",
            "start": { "dateTime": "2027-03-09T09:00:00Z" },
            "end": { "dateTime": "2027-03-09T10:00:00Z" },
            "extendedProperties": {
                "shared": {
                    "opportune.event_key": "shared-key",
                    "opportune.tags": "team/sales interview/x"
                },
                "private": {
                    "opportune.event_key": "private-key",
                    "opportune.tags": "interview/x private/y not:a:tag"
                }
            }
        }),
        json!({
            "id": "evt-private",
            "status": "confirmed",
            "transparency": "opaque",
            "iCalUID": "private@example.com",
            "start": { "dateTime": "2027-03-09T10:00:00Z" },
            "end": { "dateTime": "2027-03-09T11:00:00Z" },
            "extendedProperties": {
                "private": { "opportune.event_key": "private-only-key" }
            }
        }),
    ]);
    let (blocks, diagnostics) = available(events_list_to_blocks(&db(), &payload, &zone("UTC")));
    let both = block_for(&blocks, "evt-both");
    assert_eq!(
        both.event_key.as_deref(),
        Some("shared-key"),
        "the caller's marker wins over `iCalUID`, and `shared` over `private`"
    );
    assert_eq!(
        tag_names(&both),
        ["team/sales", "interview/x", "private/y"],
        "`shared` tags come first, the private list adds what is new, and duplicates go"
    );
    assert_eq!(
        block_for(&blocks, "evt-private").event_key.as_deref(),
        Some("private-only-key"),
        "a private marker still beats `iCalUID`"
    );
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, "marker_tag_invalid");
    assert_eq!(diagnostics[0].event_id.as_deref(), Some("evt-both"));
}

#[test]
fn an_instance_without_an_original_start_fails_the_calendar() {
    // Without a normalisable original start the adapter cannot tell two instances
    // of one series apart, so it fails rather than collapse them onto one key.
    let payload = events(&[json!({
        "id": "evt-instance",
        "status": "confirmed",
        "transparency": "opaque",
        "iCalUID": "series@example.com",
        "originalStartTime": { "timeZone": "America/New_York" },
        "start": { "dateTime": "2027-03-09T09:00:00Z" },
        "end": { "dateTime": "2027-03-09T10:00:00Z" }
    })]);
    let (code, message) = unavailable(events_list_to_blocks(&db(), &payload, &zone("UTC")));
    assert_eq!(code, "payload_invalid");
    assert!(message.contains("originalStartTime"), "{message}");
}

#[test]
fn a_payload_that_is_not_an_events_list_fails_the_calendar() {
    for payload in [
        "not json",
        r#"{ "items": null }"#,
        r#"{ "timeZone": "UTC" }"#,
        // A confirmed event with no times cannot become a block, and answering
        // with the other events would read as free time over it.
        r#"{
            "items": [
                { "id": "evt-ok", "status": "confirmed", "iCalUID": "ok@example.com",
                  "start": { "dateTime": "2027-03-09T09:00:00Z" },
                  "end": { "dateTime": "2027-03-09T10:00:00Z" } },
                { "id": "evt-broken", "status": "confirmed", "iCalUID": "broken@example.com" }
            ]
        }"#,
        // A civil date that does not exist cannot become a day.
        r#"{
            "items": [{ "id": "evt", "status": "confirmed", "iCalUID": "evt@example.com",
                        "start": { "date": "2027-02-30" },
                        "end": { "date": "2027-03-02" } }]
        }"#,
    ] {
        let (code, message) = unavailable(events_list_to_blocks(&db(), payload, &zone("UTC")));
        assert_eq!(code, "payload_invalid", "{payload}: {message}");
    }
}

#[test]
fn a_paginated_response_fails_closed_instead_of_converting_one_page() {
    // The events after `nextPageToken` are missing from the document entirely, so
    // converting the page as the whole calendar would read them as free time.
    let page = json!({
        "timeZone": "UTC",
        "nextPageToken": "CiAKGjBpNDd2Nmp2Zml2cXRwYjBpOXA",
        "items": [{
            "id": "evt-busy",
            "status": "confirmed",
            "iCalUID": "busy@example.com",
            "start": { "dateTime": "2027-03-09T09:00:00Z" },
            "end": { "dateTime": "2027-03-09T10:00:00Z" }
        }]
    })
    .to_string();
    let (code, message) = unavailable(events_list_to_blocks(&db(), &page, &zone("UTC")));
    assert_eq!(code, "payload_invalid");
    assert!(message.contains("nextPageToken"), "{message}");

    // A gateway that writes the key with no token behind it is not a page.
    let mut blank: Value = serde_json::from_str(&page).unwrap();
    blank["nextPageToken"] = json!("  ");
    let blocks = converted(&blank.to_string());
    assert_eq!(blocks.len(), 1);
}

#[test]
fn free_busy_isolates_a_calendar_the_provider_could_not_answer() {
    let payload = include_str!("fixtures/google/free_busy.json");
    let outcomes = free_busy_to_blocks(payload).unwrap();
    assert_eq!(outcomes.len(), 3);

    let (blocks, diagnostics) = available(outcomes["room-4@example.com"].clone());
    assert!(diagnostics.is_empty());
    assert_eq!(blocks.len(), 2);
    // A busy range is busy, and free/busy carries no event identity, no key and
    // no all-day flag.
    assert_eq!(blocks[0].block.status, BlockStatus::Busy);
    assert_eq!(
        blocks[0].block.span.start().to_string(),
        "2027-03-09T16:00Z"
    );
    assert_eq!(blocks[0].block.span.end().to_string(), "2027-03-09T17:00Z");
    // The second range is stamped in an offset, not UTC.
    assert_eq!(
        blocks[1].block.span.start().to_string(),
        "2027-03-10T03:00Z"
    );
    for annotated in &blocks {
        assert!(annotated.event_id.is_none());
        assert!(annotated.block.event_key.is_none());
        assert!(!annotated.block.all_day);
        assert!(annotated.block.tags.is_empty());
    }

    let (code, message) = unavailable(outcomes["room-vienna@example.com"].clone());
    assert_eq!(code, "provider_error");
    assert!(message.contains("notFound"), "{message}");

    // An explicitly empty busy list is a free calendar, not a failure.
    let (blocks, diagnostics) = available(outcomes["bo@example.com"].clone());
    assert!(blocks.is_empty());
    assert!(diagnostics.is_empty());
}

#[test]
fn free_busy_drops_a_range_whose_endpoints_coincide() {
    let payload = json!({
        "calendars": {
            "room@example.com": {
                "busy": [
                    { "start": "2027-03-09T10:00:30Z", "end": "2027-03-09T10:00:30Z" },
                    { "start": "2027-03-09T11:00:00Z", "end": "2027-03-09T12:00:00Z" }
                ]
            }
        }
    })
    .to_string();
    let outcomes = free_busy_to_blocks(&payload).unwrap();
    let (blocks, diagnostics) = available(outcomes["room@example.com"].clone());
    assert_eq!(blocks.len(), 1);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, "zero_length_event");
    // A range's position is not an event ID, so the finding carries none.
    assert!(diagnostics[0].event_id.is_none());
}

#[test]
fn free_busy_reads_a_calendar_with_no_ranges_as_free() {
    // Google omits an empty repeated field, so a calendar with nothing busy in the
    // window comes back as an entry with no `busy` key at all.
    for payload in [
        r#"{ "calendars": { "room@example.com": {} } }"#,
        r#"{ "calendars": { "room@example.com": { "busy": null } } }"#,
        r#"{ "calendars": { "room@example.com": { "errors": [] } } }"#,
    ] {
        let outcomes = free_busy_to_blocks(payload).unwrap();
        let (blocks, diagnostics) = available(outcomes["room@example.com"].clone());
        assert!(blocks.is_empty(), "{payload}");
        assert!(diagnostics.is_empty(), "{payload}");
    }
}

#[test]
fn free_busy_rejects_a_document_that_is_not_a_query_response() {
    for payload in [
        "not json",
        r#"{ "timeMin": "2027-03-09T00:00:00Z" }"#,
        r#"{ "calendars": null }"#,
    ] {
        let error = free_busy_to_blocks(payload).unwrap_err();
        assert_eq!(error.code(), "payload_invalid", "{payload}: {error}");
    }

    // A range that cannot become a span fails its own calendar, not the call.
    let payload = json!({
        "calendars": {
            "room@example.com": {
                "busy": [{ "start": "nope", "end": "nope" }]
            },
            "ana@example.com": {
                "busy": [{ "start": "2027-03-09T10:00:00Z", "end": "2027-03-09T11:00:00Z" }]
            }
        }
    })
    .to_string();
    let outcomes = free_busy_to_blocks(&payload).unwrap();
    assert_eq!(
        unavailable(outcomes["room@example.com"].clone()).0,
        "payload_invalid"
    );
    assert_eq!(available(outcomes["ana@example.com"].clone()).0.len(), 1);
}

#[test]
fn the_recorded_tenant_payload_converts_every_status_and_event_type() {
    let payload = include_str!("fixtures/google/events_list_tenant.json");
    let (blocks, diagnostics) = available(events_list_to_blocks(
        &db(),
        payload,
        &zone("America/New_York"),
    ));
    assert_eq!(
        blocks.len(),
        12,
        "three of the fifteen events do not occupy time"
    );
    assert!(blocks.iter().all(|annotated| annotated.event_id.is_some()));

    let by_id = |id: &str| block_for(&blocks, id);
    assert_eq!(by_id("evt-focus").status, BlockStatus::Busy);
    assert_eq!(tag_names(&by_id("evt-focus")), ["status/focus"]);
    assert_eq!(by_id("evt-working-location").status, BlockStatus::Free);
    assert_eq!(
        tag_names(&by_id("evt-working-location")),
        ["status/working-location"]
    );
    assert!(by_id("evt-working-location").all_day);
    assert_eq!(by_id("evt-ooo").status, BlockStatus::Busy);
    assert_eq!(tag_names(&by_id("evt-ooo")), ["status/oof"]);
    assert_eq!(tag_names(&by_id("evt-birthday")), ["status/birthday"]);
    assert_eq!(tag_names(&by_id("evt-from-gmail")), ["status/from-gmail"]);
    assert_eq!(by_id("evt-default").status, BlockStatus::Busy);
    assert!(by_id("evt-default").tags.is_empty());
    assert_eq!(by_id("evt-needs-action").status, BlockStatus::Tentative);
    assert_eq!(
        tag_names(&by_id("evt-needs-action")),
        ["status/needs-action"]
    );
    assert_eq!(by_id("evt-tentative").status, BlockStatus::Tentative);
    assert_eq!(
        by_id("evt-unspecified-end").span.end().to_string(),
        "2027-03-11T15:00Z"
    );
    let all_day = by_id("evt-all-day");
    assert!(all_day.all_day);
    assert_eq!(all_day.span.start().to_string(), "2027-03-11T05:00Z");
    assert_eq!(all_day.span.end().to_string(), "2027-03-12T05:00Z");
    assert_eq!(tag_names(&all_day), ["time-off"]);
    assert_eq!(
        by_id("evt-recurring").event_key.as_deref(),
        Some("series-platform@example.com@2027-03-09T15:00:00Z")
    );
    assert_eq!(
        by_id("evt-markers").event_key.as_deref(),
        Some("booking-42")
    );
    assert_eq!(
        tag_names(&by_id("evt-markers")),
        ["team/sales", "interview/platform", "private/note"]
    );

    let codes: Vec<&str> = diagnostics.iter().map(|finding| finding.code).collect();
    assert_eq!(
        codes,
        [
            "zero_length_event",
            "end_time_unspecified",
            "marker_tag_invalid"
        ]
    );
}

#[test]
fn non_utc_offset_timestamps_at_supported_utc_boundaries_convert_cleanly() {
    // Both instants lie inside [1975-01-01T00:00Z, 2100-01-01T00:00Z] once
    // their offsets are applied, even though their local clock dates sit on
    // 1974-12-31 (-05:00) and 2100-01-01T09:00 (+09:00).
    let payload = events(&[
        json!({
            "id": "evt-min-edge",
            "status": "confirmed",
            "transparency": "opaque",
            "start": { "dateTime": "1974-12-31T19:00:00-05:00" },
            "end": { "dateTime": "1974-12-31T20:00:00-05:00" }
        }),
        json!({
            "id": "evt-max-edge",
            "status": "confirmed",
            "transparency": "opaque",
            "start": { "dateTime": "2100-01-01T08:00:00+09:00" },
            "end": { "dateTime": "2100-01-01T09:00:00+09:00" }
        }),
    ]);
    let (blocks, diagnostics) = available(events_list_to_blocks(&db(), &payload, &zone("UTC")));
    assert!(diagnostics.is_empty());
    assert_eq!(blocks.len(), 2);
    let min_edge = block_for(&blocks, "evt-min-edge");
    assert_eq!(min_edge.span.start().to_string(), "1975-01-01T00:00Z");
    assert_eq!(min_edge.span.end().to_string(), "1975-01-01T01:00Z");
    let max_edge = block_for(&blocks, "evt-max-edge");
    assert_eq!(max_edge.span.start().to_string(), "2099-12-31T23:00Z");
    assert_eq!(max_edge.span.end().to_string(), "2100-01-01T00:00Z");
}

#[test]
fn malformed_signed_or_extra_clock_fields_are_rejected() {
    for bad in [
        "2027-03-09T09:00:+5Z",
        "2027-03-09T+9:00:00Z",
        "2027-03-09T09:00::..00Z",
        "2027-03-09T09:00.5Z",
    ] {
        let payload = events(&[json!({
            "id": "evt-bad-clock",
            "status": "confirmed",
            "start": { "dateTime": bad },
            "end": { "dateTime": "2027-03-09T10:00:00Z" }
        })]);
        let (code, _) = unavailable(events_list_to_blocks(&db(), &payload, &zone("UTC")));
        assert_eq!(code, "payload_invalid", "expected rejection for {bad}");
    }
}

#[test]
fn an_offsetless_date_time_is_read_in_the_zone_it_names() {
    // Google's contract allows a `dateTime` with no offset when the sibling
    // `timeZone` names the zone. Reading that as a malformed instant failed the
    // whole calendar, not just the event.
    let event_zone = json!({
        "items": [{
            "id": "evt-wall",
            "status": "confirmed",
            "iCalUID": "wall@example.com",
            "start": { "dateTime": "2027-03-09T10:00:00", "timeZone": "America/New_York" },
            "end": { "dateTime": "2027-03-09T11:00:00", "timeZone": "America/New_York" }
        }]
    })
    .to_string();
    let blocks = converted(&event_zone);
    let block = block_for(&blocks, "evt-wall");
    // 2027-03-09 is before the March transition, so New York is UTC-05:00.
    assert_eq!(block.span.start().to_string(), "2027-03-09T15:00Z");
    assert_eq!(block.span.end().to_string(), "2027-03-09T16:00Z");

    // The response's `timeZone` is the next fallback.
    let response_zone = json!({
        "timeZone": "America/New_York",
        "items": [{
            "id": "evt-wall",
            "status": "confirmed",
            "iCalUID": "wall@example.com",
            "start": { "dateTime": "2027-03-09T10:00:00" },
            "end": { "dateTime": "2027-03-09T11:00:00" }
        }]
    })
    .to_string();
    let blocks = converted(&response_zone);
    assert_eq!(
        block_for(&blocks, "evt-wall").span.start().to_string(),
        "2027-03-09T15:00Z"
    );

    // The participant's zone is the last fallback, since Google may name neither.
    let bare = json!({
        "items": [{
            "id": "evt-wall",
            "status": "confirmed",
            "iCalUID": "wall@example.com",
            "start": { "dateTime": "2027-03-09T10:00:00" },
            "end": { "dateTime": "2027-03-09T11:00:00" }
        }]
    })
    .to_string();
    let (blocks, _) = available(events_list_to_blocks(&db(), &bare, &zone("Asia/Tokyo")));
    assert_eq!(
        block_for(&blocks, "evt-wall").span.start().to_string(),
        "2027-03-09T01:00Z"
    );

    // With no zone left to resolve the reading, the calendar fails closed.
    let (code, message) = unavailable(events_list_to_blocks(
        &db(),
        &bare,
        &zone("Mars/Olympus_Mons"),
    ));
    assert_eq!(code, "zone_unknown");
    assert!(message.contains("Mars/Olympus_Mons"), "{message}");
}

#[test]
fn an_offsetless_original_start_is_read_in_its_zone() {
    let payload = events(&[json!({
        "id": "evt-instance",
        "status": "confirmed",
        "transparency": "opaque",
        "recurringEventId": "series",
        "iCalUID": "series@example.com",
        "originalStartTime": {
            "dateTime": "2027-03-09T10:00:00",
            "timeZone": "America/New_York"
        },
        "start": { "dateTime": "2027-03-09T10:00:00-05:00" },
        "end": { "dateTime": "2027-03-09T11:00:00-05:00" }
    })]);
    let blocks = converted(&payload);
    assert_eq!(
        block_for(&blocks, "evt-instance").event_key.as_deref(),
        Some("series@example.com@2027-03-09T15:00:00Z")
    );
}

#[test]
fn a_series_level_marker_takes_the_original_start_too() {
    // `extendedProperties.shared` written once on a series reaches every
    // instance, so a marker needs the same discriminator `iCalUID` does.
    let instance = |id: &str, original: &str| {
        json!({
            "id": id,
            "status": "confirmed",
            "transparency": "opaque",
            "recurringEventId": "series",
            "iCalUID": "series@example.com",
            "originalStartTime": { "dateTime": original },
            "start": { "dateTime": "2027-03-09T10:00:00Z" },
            "end": { "dateTime": "2027-03-09T11:00:00Z" },
            "extendedProperties": { "shared": { "opportune.event_key": "booking-42" } }
        })
    };
    let single = json!({
        "id": "evt-single",
        "status": "confirmed",
        "transparency": "opaque",
        "iCalUID": "single@example.com",
        "start": { "dateTime": "2027-03-09T10:00:00Z" },
        "end": { "dateTime": "2027-03-09T11:00:00Z" },
        "extendedProperties": { "shared": { "opportune.event_key": "single-key" } }
    });
    let payload = events(&[
        instance("evt-ana", "2027-03-09T10:00:00Z"),
        instance("evt-next", "2027-03-16T10:00:00Z"),
        single,
    ]);
    let blocks = converted(&payload);
    assert_eq!(
        block_for(&blocks, "evt-ana").event_key.as_deref(),
        Some("booking-42@2027-03-09T10:00:00Z")
    );
    assert_eq!(
        block_for(&blocks, "evt-next").event_key.as_deref(),
        Some("booking-42@2027-03-16T10:00:00Z")
    );
    assert_eq!(
        block_for(&blocks, "evt-single").event_key.as_deref(),
        Some("single-key")
    );
}

#[test]
fn an_informational_all_day_event_without_transparency_is_free() {
    // Google writes birthday and working-location markers as `transparent`. When
    // the field is omitted the event must not fall back to the `opaque` default and
    // block the host's whole day; an explicit `opaque` still blocks.
    let marker = |id: &str, event_type: &str, transparency: Option<&str>| {
        let mut event = json!({
            "id": id,
            "status": "confirmed",
            "eventType": event_type,
            "iCalUID": format!("{id}@example.com"),
            "start": { "date": "2027-03-09" },
            "end": { "date": "2027-03-10" }
        });
        if let Some(transparency) = transparency {
            event["transparency"] = json!(transparency);
        }
        event
    };
    let payload = events(&[
        marker("evt-birthday", "birthday", None),
        marker("evt-working-location", "workingLocation", None),
        marker("evt-birthday-opaque", "birthday", Some("opaque")),
        marker("evt-birthday-transparent", "birthday", Some("transparent")),
    ]);
    let (blocks, diagnostics) = available(events_list_to_blocks(&db(), &payload, &zone("UTC")));
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(block_for(&blocks, "evt-birthday").status, BlockStatus::Free);
    assert_eq!(
        block_for(&blocks, "evt-working-location").status,
        BlockStatus::Free
    );
    assert_eq!(
        block_for(&blocks, "evt-birthday-opaque").status,
        BlockStatus::Busy,
        "an explicit opaque still blocks"
    );
    assert_eq!(
        block_for(&blocks, "evt-birthday-transparent").status,
        BlockStatus::Free
    );
    // The event-type tag is unaffected by the default.
    assert_eq!(
        tag_names(&block_for(&blocks, "evt-birthday")),
        ["status/birthday"]
    );
}
