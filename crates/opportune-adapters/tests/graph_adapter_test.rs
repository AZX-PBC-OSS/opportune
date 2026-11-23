//! Graph `calendarView`, `getSchedule` and `workingHours` conversions.

// Every case resolves zone names against a real database, so this target runs
// where the crate compiles one in: `make test-matrix`'s `graph` build keeps
// `bundled-tzdb` on, and checks the bundle-free build separately.
#![cfg(all(feature = "graph", feature = "bundled-tzdb"))]

use std::collections::BTreeMap;

use opportune::model::{Block, BlockStatus, RuleKind};
use opportune::tags::Tag;
use opportune::time::{Date, LocalResolution, RuleZone, TimeOfDay, TimeZoneDatabase, ZoneName};
use opportune_adapters::graph::{
    EXTENDED_PROPERTY_EVENT_KEY_ID, EXTENDED_PROPERTY_EXPAND, EXTENDED_PROPERTY_NAMESPACE_GUID,
    EXTENDED_PROPERTY_TAGS_ID, calendar_view_to_blocks, get_schedule_to_blocks,
    working_hours_to_rule,
};
use opportune_adapters::{AdapterDiagnostic, AnnotatedBlock, CalendarOutcome, WorkingHoursError};
use serde_json::json;

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
            panic!("the calendar should convert, not fail with `{code}`: {message}")
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

/// One calendarView payload carrying `events`.
fn calendar_view(events: Vec<serde_json::Value>) -> String {
    let mut payload = json!({ "value": [] });
    payload["value"] = serde_json::Value::Array(events);
    payload.to_string()
}

/// One timed event with the fields the conversion reads.
fn timed_event(
    id: &str,
    show_as: &str,
    start: &str,
    end: &str,
    time_zone: &str,
) -> serde_json::Value {
    json!({
        "id": id,
        "type": "singleInstance",
        "isAllDay": false,
        "isCancelled": false,
        "isDraft": false,
        "showAs": show_as,
        "iCalUId": format!("ical-{id}"),
        "start": { "dateTime": start, "timeZone": time_zone },
        "end": { "dateTime": end, "timeZone": time_zone }
    })
}

#[test]
fn a_timed_event_resolves_in_the_zone_its_timestamp_names() {
    let payload = calendar_view(vec![
        timed_event(
            "evt-utc",
            "busy",
            "2027-03-09T14:00:00.0000000",
            "2027-03-09T15:00:00.0000000",
            "UTC",
        ),
        timed_event(
            "evt-iana",
            "busy",
            "2027-03-09T09:00:00.0000000",
            "2027-03-09T10:00:00.0000000",
            "America/New_York",
        ),
    ]);
    let outcome = calendar_view_to_blocks(&db(), &payload, &zone("UTC"), None);
    let (blocks, diagnostics) = available(outcome);
    assert!(diagnostics.is_empty());
    // Seven fractional digits are accepted, and the minute is exact.
    assert_eq!(
        block_for(&blocks, "evt-utc").span.start().to_string(),
        "2027-03-09T14:00Z"
    );
    // A payload that ignores `Prefer: outlook.timezone="UTC"` reads correctly:
    // 09:00 in New York is 14:00Z, before the March transition.
    assert_eq!(
        block_for(&blocks, "evt-iana").span.start().to_string(),
        "2027-03-09T14:00Z"
    );
}

#[cfg(feature = "windows-zones")]
#[test]
fn two_windows_zones_in_one_payload_keep_their_own_offsets() {
    // A conversion resolves each zone name once and reuses it, so the reused entry
    // has to be the one the payload wrote: two zones in one response must not
    // collapse onto each other.
    let payload = calendar_view(vec![
        timed_event(
            "evt-pacific",
            "busy",
            "2027-03-09T09:00:00.0000000",
            "2027-03-09T10:00:00.0000000",
            "Pacific Standard Time",
        ),
        timed_event(
            "evt-gmt",
            "busy",
            "2027-03-09T09:00:00.0000000",
            "2027-03-09T10:00:00.0000000",
            "GMT Standard Time",
        ),
    ]);
    let (blocks, diagnostics) =
        available(calendar_view_to_blocks(&db(), &payload, &zone("UTC"), None));
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(
        block_for(&blocks, "evt-pacific").span.start().to_string(),
        "2027-03-09T17:00Z",
    );
    assert_eq!(
        block_for(&blocks, "evt-gmt").span.start().to_string(),
        "2027-03-09T09:00Z",
    );
}

/// A timed event that ends before it starts cannot describe a block, so the calendar
/// fails closed rather than answering with the events that did convert — an answer
/// missing this event's span reads as free time over it.
#[test]
fn a_reversed_timed_span_fails_the_calendar() {
    let payload = calendar_view(vec![timed_event(
        "evt-reversed",
        "busy",
        "2027-03-09T15:00:00.0000000",
        "2027-03-09T14:00:00.0000000",
        "UTC",
    )]);
    let (code, message) = unavailable(calendar_view_to_blocks(&db(), &payload, &zone("UTC"), None));
    assert_eq!(code, "payload_invalid");
    assert!(message.contains("evt-reversed"), "{message}");
}

#[cfg(feature = "windows-zones")]
#[test]
fn a_timed_event_in_a_windows_zone_resolves_through_the_cldr_mapping() {
    let payload = calendar_view(vec![timed_event(
        "evt-windows",
        "busy",
        "2027-03-09T09:00:00.0000000",
        "2027-03-09T10:00:00.0000000",
        "Pacific Standard Time",
    )]);
    let block = block_for(
        &available(calendar_view_to_blocks(&db(), &payload, &zone("UTC"), None)).0,
        "evt-windows",
    );
    // 09:00 Pacific Standard Time is 17:00Z before the March transition.
    assert_eq!(block.span.start().to_string(), "2027-03-09T17:00Z");
}

#[test]
fn a_zero_length_event_is_dropped_before_the_endpoints_round_outward() {
    let payload = calendar_view(vec![
        timed_event(
            "evt-reminder",
            "busy",
            "2027-03-09T10:00:30.0000000",
            "2027-03-09T10:00:30.0000000",
            "UTC",
        ),
        timed_event(
            "evt-half-minute",
            "busy",
            "2027-03-09T11:00:00.0000000",
            "2027-03-09T11:00:30.0000000",
            "UTC",
        ),
    ]);
    let (blocks, diagnostics) =
        available(calendar_view_to_blocks(&db(), &payload, &zone("UTC"), None));
    // The zero-length event is dropped, not widened into `[10:00, 10:01)`.
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
fn a_sub_minute_event_rounds_outward() {
    let payload = calendar_view(vec![timed_event(
        "evt-fraction",
        "busy",
        "2027-03-09T10:30:45.0000000",
        "2027-03-09T11:00:01.0000000",
        "UTC",
    )]);
    let block = block_for(
        &available(calendar_view_to_blocks(&db(), &payload, &zone("UTC"), None)).0,
        "evt-fraction",
    );
    assert_eq!(block.span.start().to_string(), "2027-03-09T10:30Z");
    assert_eq!(block.span.end().to_string(), "2027-03-09T11:01Z");
}

#[test]
fn an_all_day_event_is_read_from_its_civil_dates_in_the_participants_zone() {
    // Both events are stamped at UTC midnight, which is 19:00 or 20:00 the day
    // before in New York: reading the stamp as an instant would move the day.
    let payload = calendar_view(vec![
        json!({
            "id": "evt-spring",
            "type": "singleInstance",
            "isAllDay": true,
            "showAs": "busy",
            "start": { "dateTime": "2027-03-14T00:00:00.0000000", "timeZone": "UTC" },
            "end": { "dateTime": "2027-03-15T00:00:00.0000000", "timeZone": "UTC" }
        }),
        json!({
            "id": "evt-fall",
            "type": "singleInstance",
            "isAllDay": true,
            "showAs": "busy",
            "start": { "dateTime": "2027-11-07T00:00:00.0000000", "timeZone": "UTC" },
            "end": { "dateTime": "2027-11-08T00:00:00.0000000", "timeZone": "UTC" }
        }),
    ]);
    let outcome = calendar_view_to_blocks(&db(), &payload, &zone("America/New_York"), None);
    let (blocks, _) = available(outcome);
    let spring = &blocks[0].block;
    assert!(spring.all_day);
    // The spring-forward day is 23 hours long: 00:00 EST to 00:00 EDT.
    assert_eq!(spring.span.start().to_string(), "2027-03-14T05:00Z");
    assert_eq!(spring.span.end().to_string(), "2027-03-15T04:00Z");
    assert_eq!(spring.span.minutes(), 23 * 60);

    let fall = &blocks[1].block;
    assert!(fall.all_day);
    // The fall-back day is 25 hours long: 00:00 EDT to 00:00 EST.
    assert_eq!(fall.span.start().to_string(), "2027-11-07T04:00Z");
    assert_eq!(fall.span.end().to_string(), "2027-11-08T05:00Z");
    assert_eq!(fall.span.minutes(), 25 * 60);
}

#[test]
fn an_all_day_event_ignores_the_zone_the_payload_stamps_on_it() {
    // The stamped zone is neither IANA nor, without the CLDR mapping, resolvable:
    // an all-day event still converts, because its civil dates are read in the
    // participant's zone whatever the payload says.
    let payload = calendar_view(vec![json!({
        "id": "evt-all-day",
        "isAllDay": true,
        "showAs": "busy",
        "start": { "dateTime": "2027-03-09T00:00:00.0000000", "timeZone": "Pacific Standard Time" },
        "end": { "dateTime": "2027-03-10T00:00:00.0000000", "timeZone": "Pacific Standard Time" }
    })]);
    let block = block_for(
        &available(calendar_view_to_blocks(
            &db(),
            &payload,
            &zone("Europe/London"),
            None,
        ))
        .0,
        "evt-all-day",
    );
    assert!(block.all_day);
    assert_eq!(block.span.start().to_string(), "2027-03-09T00:00Z");
    assert_eq!(block.span.end().to_string(), "2027-03-10T00:00Z");
}

#[cfg(feature = "windows-zones")]
#[test]
fn the_recorded_dst_fixture_converts_all_four_traps() {
    // The recorded payload carries the four DST traps together: both all-day
    // transition days and a timed event inside each transition hour, stamped in
    // the Windows zone a tenant reports.
    let payload = include_str!("fixtures/graph/all_day_dst.json");
    let (blocks, diagnostics) = available(calendar_view_to_blocks(
        &db(),
        payload,
        &zone("America/New_York"),
        None,
    ));
    assert!(diagnostics.is_empty());
    assert_eq!(blocks.len(), 4);

    let spring = block_for(&blocks, "evt-spring-forward");
    assert!(spring.all_day);
    assert_eq!(spring.span.start().to_string(), "2027-03-14T05:00Z");
    assert_eq!(spring.span.end().to_string(), "2027-03-15T04:00Z");
    assert_eq!(spring.span.minutes(), 23 * 60);

    let fall = block_for(&blocks, "evt-fall-back");
    assert!(fall.all_day);
    assert_eq!(fall.span.start().to_string(), "2027-11-07T04:00Z");
    assert_eq!(fall.span.end().to_string(), "2027-11-08T05:00Z");
    assert_eq!(fall.span.minutes(), 25 * 60);

    let repeated = block_for(&blocks, "evt-repeated-hour");
    assert_eq!(repeated.span.start().to_string(), "2027-11-07T05:30Z");
    assert_eq!(repeated.span.end().to_string(), "2027-11-07T05:45Z");

    let skipped = block_for(&blocks, "evt-skipped-hour");
    assert_eq!(skipped.span.start().to_string(), "2027-03-14T07:30Z");
    assert_eq!(skipped.span.end().to_string(), "2027-03-14T07:45Z");
}

#[test]
fn a_local_time_in_a_repeated_hour_takes_the_earlier_instant() {
    let payload = calendar_view(vec![timed_event(
        "evt-repeated",
        "busy",
        "2027-11-07T01:30:00.0000000",
        "2027-11-07T01:45:00.0000000",
        "America/New_York",
    )]);
    let block = block_for(
        &available(calendar_view_to_blocks(&db(), &payload, &zone("UTC"), None)).0,
        "evt-repeated",
    );
    // 01:30 happens at 05:30Z (EDT) and 06:30Z (EST); the earlier instant is the
    // compatible reading.
    assert_eq!(block.span.start().to_string(), "2027-11-07T05:30Z");
    assert_eq!(block.span.end().to_string(), "2027-11-07T05:45Z");
}

#[test]
fn a_local_time_in_a_skipped_hour_moves_past_the_gap() {
    let payload = calendar_view(vec![timed_event(
        "evt-skipped",
        "busy",
        "2027-03-14T02:30:00.0000000",
        "2027-03-14T02:45:00.0000000",
        "America/New_York",
    )]);
    let block = block_for(
        &available(calendar_view_to_blocks(&db(), &payload, &zone("UTC"), None)).0,
        "evt-skipped",
    );
    // 02:30 does not exist in New York that day; compatible reading moves it to
    // 03:30 EDT, which is 07:30Z.
    assert_eq!(block.span.start().to_string(), "2027-03-14T07:30Z");
    assert_eq!(block.span.end().to_string(), "2027-03-14T07:45Z");
}

#[test]
fn every_show_as_value_maps_to_its_status_and_tag() {
    let cases = [
        ("free", BlockStatus::Free, None),
        ("Free", BlockStatus::Free, None),
        ("tentative", BlockStatus::Tentative, None),
        ("TENTATIVE", BlockStatus::Tentative, None),
        ("busy", BlockStatus::Busy, None),
        ("OOF", BlockStatus::Busy, Some("status/oof")),
        (
            "WorkingElsewhere",
            BlockStatus::Free,
            Some("status/working-elsewhere"),
        ),
        ("unknown", BlockStatus::Busy, Some("status/unknown")),
        // A value Graph has not defined yet blocks rather than reads as free.
        ("SomethingNew", BlockStatus::Busy, None),
    ];
    for (index, (show_as, status, tag)) in cases.iter().enumerate() {
        let id = format!("evt-{index}");
        let payload = calendar_view(vec![timed_event(
            &id,
            show_as,
            "2027-03-09T09:00:00.0000000",
            "2027-03-09T10:00:00.0000000",
            "UTC",
        )]);
        let block = block_for(
            &available(calendar_view_to_blocks(&db(), &payload, &zone("UTC"), None)).0,
            &id,
        );
        assert_eq!(block.status, *status, "showAs `{show_as}`");
        let expected: Vec<&str> = tag.iter().copied().collect();
        assert_eq!(tag_names(&block), expected, "showAs `{show_as}`");
    }
}

#[test]
fn cancelled_events_and_declined_invitations_are_dropped_and_drafts_are_tagged() {
    let payload = calendar_view(vec![
        json!({
            "id": "evt-cancelled",
            "isCancelled": true,
            "showAs": "busy",
            "start": { "dateTime": "2027-03-09T09:00:00.0000000", "timeZone": "UTC" },
            "end": { "dateTime": "2027-03-09T10:00:00.0000000", "timeZone": "UTC" }
        }),
        json!({
            "id": "evt-declined",
            "isCancelled": false,
            "showAs": "busy",
            "responseStatus": { "response": "Declined", "time": "2027-03-01T09:00:00Z" },
            "start": { "dateTime": "2027-03-09T10:00:00.0000000", "timeZone": "UTC" },
            "end": { "dateTime": "2027-03-09T11:00:00.0000000", "timeZone": "UTC" }
        }),
        json!({
            "id": "evt-draft",
            "isDraft": true,
            "showAs": "tentative",
            "responseStatus": { "response": "accepted", "time": "2027-03-01T09:00:00Z" },
            "start": { "dateTime": "2027-03-09T11:00:00.0000000", "timeZone": "UTC" },
            "end": { "dateTime": "2027-03-09T12:00:00.0000000", "timeZone": "UTC" }
        }),
    ]);
    let (blocks, diagnostics) =
        available(calendar_view_to_blocks(&db(), &payload, &zone("UTC"), None));
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].event_id.as_deref(), Some("evt-draft"));
    assert_eq!(blocks[0].block.status, BlockStatus::Tentative);
    assert_eq!(tag_names(&blocks[0].block), ["status/draft"]);
    assert!(diagnostics.is_empty());
}

#[test]
fn a_series_master_fails_the_calendar_rather_than_returning_a_partial_list() {
    let payload = calendar_view(vec![
        timed_event(
            "evt-ok",
            "busy",
            "2027-03-09T09:00:00.0000000",
            "2027-03-09T10:00:00.0000000",
            "UTC",
        ),
        json!({
            "id": "evt-series",
            "type": "seriesMaster",
            "isAllDay": false,
            "showAs": "busy",
            "start": { "dateTime": "2027-03-09T11:00:00.0000000", "timeZone": "UTC" },
            "end": { "dateTime": "2027-03-09T12:00:00.0000000", "timeZone": "UTC" }
        }),
    ]);
    let (code, message) = unavailable(calendar_view_to_blocks(&db(), &payload, &zone("UTC"), None));
    assert_eq!(code, "unexpanded_series");
    assert!(message.contains("calendarView"), "{message}");
}

#[test]
fn the_event_key_prefers_the_marker_then_the_ical_uid() {
    let payload = calendar_view(vec![
        timed_event(
            "evt-ical",
            "busy",
            "2027-03-09T09:00:00.0000000",
            "2027-03-09T10:00:00.0000000",
            "UTC",
        ),
        json!({
            "id": "evt-marker",
            "isAllDay": false,
            "showAs": "busy",
            "iCalUId": "ical-from-graph",
            "start": { "dateTime": "2027-03-09T10:00:00.0000000", "timeZone": "UTC" },
            "end": { "dateTime": "2027-03-09T11:00:00.0000000", "timeZone": "UTC" },
            "singleValueExtendedProperties": [{
                "id": EXTENDED_PROPERTY_EVENT_KEY_ID,
                "value": "booking-42"
            }]
        }),
    ]);
    let (blocks, _) = available(calendar_view_to_blocks(&db(), &payload, &zone("UTC"), None));
    assert_eq!(blocks[0].block.event_key.as_deref(), Some("ical-evt-ical"));
    assert_eq!(
        blocks[1].block.event_key.as_deref(),
        Some("booking-42"),
        "the caller's marker wins over the provider's identifier"
    );
}

#[test]
fn marker_tags_and_categories_become_block_tags() {
    let payload = calendar_view(vec![json!({
        "id": "evt-tagged",
        "isAllDay": false,
        "showAs": "busy",
        "start": { "dateTime": "2027-03-09T10:00:00.0000000", "timeZone": "UTC" },
        "end": { "dateTime": "2027-03-09T11:00:00.0000000", "timeZone": "UTC" },
        "categories": ["Interview", "1:1 Sync!", "!!!", "Client Demo (Q3)!"],
        "singleValueExtendedProperties": [
            // Graph returns the namespace in the case it was created with.
            {
                "id": "String {A5D0C4E2-7B1F-4C9A-9E3D-2F8B6C1A4E70} Name opportune.tags",
                "value": "team/sales not:a:tag"
            }
        ]
    })]);
    let map = BTreeMap::from([("Interview".to_owned(), Tag::new("recruiting").unwrap())]);
    let (blocks, diagnostics) = available(calendar_view_to_blocks(
        &db(),
        &payload,
        &zone("UTC"),
        Some(&map),
    ));
    assert_eq!(
        tag_names(&blocks[0].block),
        [
            "recruiting",
            "category/1-1-sync",
            "category/client-demo-q3",
            "team/sales"
        ]
    );
    assert_eq!(diagnostics.len(), 2);
    assert_eq!(diagnostics[0].code, "marker_tag_invalid");
    // `!!!` slugs to nothing, so it makes no tag; the finding names it and the
    // event, without changing the block.
    assert_eq!(diagnostics[1].code, "category_tag_unmappable");
    assert_eq!(diagnostics[1].event_id.as_deref(), Some("evt-tagged"));
    assert!(diagnostics[1].message.contains("!!!"), "{diagnostics:?}");
}

#[test]
fn a_non_latin_category_is_reported_when_it_makes_no_tag() {
    let payload = calendar_view(vec![json!({
        "id": "evt-non-latin",
        "isAllDay": false,
        "showAs": "busy",
        "start": { "dateTime": "2027-03-09T10:00:00.0000000", "timeZone": "UTC" },
        "end": { "dateTime": "2027-03-09T11:00:00.0000000", "timeZone": "UTC" },
        "categories": ["会議", "Встреча", "!!!", "Interview", "   ", ""]
    })]);
    let (blocks, diagnostics) =
        available(calendar_view_to_blocks(&db(), &payload, &zone("UTC"), None));
    // The diagnostic changes nothing: the one category that slugs still becomes a
    // tag, and the blank strings are not categories at all.
    assert_eq!(tag_names(&blocks[0].block), ["category/interview"]);
    assert_eq!(diagnostics.len(), 3, "{diagnostics:?}");
    for diagnostic in &diagnostics {
        assert_eq!(diagnostic.code, "category_tag_unmappable");
        assert_eq!(diagnostic.event_id.as_deref(), Some("evt-non-latin"));
    }
    for category in ["会議", "Встреча", "!!!"] {
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains(category)),
            "no finding names `{category}`: {diagnostics:?}"
        );
    }
}

#[test]
fn an_event_reports_at_most_sixteen_unmappable_categories() {
    let categories = vec!["Встреча".to_owned(); 19];
    let payload = calendar_view(vec![json!({
        "id": "evt-many-categories",
        "isAllDay": false,
        "showAs": "busy",
        "start": { "dateTime": "2027-03-09T10:00:00.0000000", "timeZone": "UTC" },
        "end": { "dateTime": "2027-03-09T11:00:00.0000000", "timeZone": "UTC" },
        "categories": categories
    })]);
    let (blocks, diagnostics) =
        available(calendar_view_to_blocks(&db(), &payload, &zone("UTC"), None));
    assert!(blocks[0].block.tags.is_empty());
    // Sixteen names are reported individually and the remaining three are folded
    // into one summary, so a hostile event cannot flood the findings.
    assert_eq!(diagnostics.len(), 17, "{diagnostics:?}");
    for diagnostic in &diagnostics[..16] {
        assert_eq!(diagnostic.code, "category_tag_unmappable");
        assert_eq!(diagnostic.event_id.as_deref(), Some("evt-many-categories"));
        assert!(diagnostic.message.contains("Встреча"), "{diagnostic:?}");
    }
    assert_eq!(diagnostics[16].code, "category_tag_unmappable");
    assert_eq!(
        diagnostics[16].event_id.as_deref(),
        Some("evt-many-categories")
    );
    assert!(
        diagnostics[16].message.contains("3 more"),
        "{diagnostics:?}"
    );
}

#[test]
fn a_payload_that_is_not_a_calendar_view_fails_the_calendar() {
    for payload in [
        "not json",
        r#"{ "value": null }"#,
        r#"{ "@odata.context": "x" }"#,
        r#"{ "value": [{ "id": "x", "isAllDay": false, "showAs": "busy" }] }"#,
    ] {
        let (code, message) =
            unavailable(calendar_view_to_blocks(&db(), payload, &zone("UTC"), None));
        assert_eq!(code, "payload_invalid", "{payload}: {message}");
    }
}

#[test]
fn a_paginated_calendar_view_fails_closed_instead_of_converting_one_page() {
    // Graph pages `calendarView`. A payload carrying `@odata.nextLink` is the
    // first page of several, so the events after it are not in the document at
    // all; converting the page as the whole calendar would read the missing
    // events as free time.
    let page = json!({
        "@odata.context": "https://graph.microsoft.com/v1.0/$metadata#users('ana%40example.com')/calendarView",
        "@odata.nextLink": "https://graph.microsoft.com/v1.0/users/ana@example.com/calendarView?$skiptoken=abc",
        "value": [{
            "id": "evt-busy",
            "isAllDay": false,
            "showAs": "busy",
            "start": { "dateTime": "2027-03-09T09:00:00.0000000", "timeZone": "UTC" },
            "end": { "dateTime": "2027-03-09T10:00:00.0000000", "timeZone": "UTC" }
        }]
    })
    .to_string();
    let (code, message) = unavailable(calendar_view_to_blocks(&db(), &page, &zone("UTC"), None));
    assert_eq!(code, "payload_invalid");
    assert!(message.contains("@odata.nextLink"), "{message}");

    // The same page with the link dropped is a complete response and converts.
    let mut complete: serde_json::Value = serde_json::from_str(&page).unwrap();
    complete.as_object_mut().unwrap().remove("@odata.nextLink");
    let (blocks, diagnostics) = available(calendar_view_to_blocks(
        &db(),
        &complete.to_string(),
        &zone("UTC"),
        None,
    ));
    assert_eq!(blocks.len(), 1);
    assert!(diagnostics.is_empty());

    // A gateway that writes the key with no link behind it is not a page.
    let mut blank: serde_json::Value = serde_json::from_str(&page).unwrap();
    blank["@odata.nextLink"] = json!("  ");
    let (blocks, _) = available(calendar_view_to_blocks(
        &db(),
        &blank.to_string(),
        &zone("UTC"),
        None,
    ));
    assert_eq!(blocks.len(), 1);
}

#[test]
fn a_zone_nothing_resolves_fails_the_calendar() {
    let payload = calendar_view(vec![json!({
        "id": "evt-unknown-zone",
        "isAllDay": false,
        "showAs": "busy",
        "start": { "dateTime": "2027-03-09T09:00:00.0000000", "timeZone": "Mars/Olympus_Mons" },
        "end": { "dateTime": "2027-03-09T10:00:00.0000000", "timeZone": "Mars/Olympus_Mons" }
    })]);
    let (code, message) = unavailable(calendar_view_to_blocks(&db(), &payload, &zone("UTC"), None));
    assert_eq!(code, "zone_unknown");
    assert!(message.contains("Mars/Olympus_Mons"), "{message}");

    // The participant's own zone is what all-day events are read in.
    let all_day = calendar_view(vec![json!({
        "id": "evt-all-day",
        "isAllDay": true,
        "showAs": "busy",
        "start": { "dateTime": "2027-03-09T00:00:00.0000000", "timeZone": "UTC" },
        "end": { "dateTime": "2027-03-10T00:00:00.0000000", "timeZone": "UTC" }
    })]);
    let (code, _) = unavailable(calendar_view_to_blocks(
        &db(),
        &all_day,
        &zone("Mars/Olympus_Mons"),
        None,
    ));
    assert_eq!(code, "zone_unknown");
}

#[cfg(feature = "windows-zones")]
#[test]
fn a_recorded_tenant_payload_converts_every_status_and_category() {
    let payload = include_str!("fixtures/graph/calendar_view_tenant.json");
    let (blocks, diagnostics) = available(calendar_view_to_blocks(
        &db(),
        payload,
        &zone("America/New_York"),
        None,
    ));
    assert_eq!(
        blocks.len(),
        9,
        "three of the twelve events do not occupy time"
    );
    assert!(blocks.iter().all(|annotated| annotated.event_id.is_some()));
    assert!(blocks.iter().all(|annotated| !annotated.block.all_day));

    let by_id = |id: &str| {
        blocks
            .iter()
            .find(|annotated| annotated.event_id.as_deref() == Some(id))
            .unwrap_or_else(|| panic!("no block for `{id}`"))
            .block
            .clone()
    };
    assert_eq!(by_id("evt-free").status, BlockStatus::Free);
    assert_eq!(by_id("evt-tentative").status, BlockStatus::Tentative);
    assert_eq!(by_id("evt-oof").status, BlockStatus::Busy);
    assert_eq!(tag_names(&by_id("evt-oof")), ["status/oof"]);
    assert_eq!(by_id("evt-elsewhere").status, BlockStatus::Free);
    assert_eq!(
        tag_names(&by_id("evt-elsewhere")),
        ["status/working-elsewhere"]
    );
    assert_eq!(tag_names(&by_id("evt-unknown")), ["status/unknown"]);
    assert_eq!(tag_names(&by_id("evt-draft")), ["status/draft"]);
    // 09:00 Pacific Standard Time is 17:00Z.
    assert_eq!(
        by_id("evt-windows-zone").span.start().to_string(),
        "2027-03-09T17:00Z"
    );
    assert_eq!(
        by_id("evt-fractional").span.start().to_string(),
        "2027-03-10T10:30Z"
    );
    assert_eq!(
        by_id("evt-fractional").span.end().to_string(),
        "2027-03-10T11:01Z"
    );
    assert_eq!(
        tag_names(&by_id("evt-categories")),
        [
            "category/interview",
            "category/1-1-sync",
            "category/client-demo-q3",
            "team/sales",
            "interview/platform"
        ]
    );
    assert_eq!(
        by_id("evt-categories").event_key.as_deref(),
        Some("booking-42")
    );

    let codes: Vec<&str> = diagnostics.iter().map(|finding| finding.code).collect();
    assert_eq!(
        codes,
        [
            "zero_length_event",
            "marker_tag_invalid",
            "category_tag_unmappable"
        ]
    );
    let unmappable = diagnostics
        .iter()
        .find(|finding| finding.code == "category_tag_unmappable")
        .expect("`!!!` makes no tag, so it is reported");
    assert_eq!(unmappable.event_id.as_deref(), Some("evt-categories"));
    assert!(unmappable.message.contains("!!!"), "{unmappable:?}");
}

#[test]
fn get_schedule_isolates_a_schedule_the_provider_could_not_answer() {
    let payload = include_str!("fixtures/graph/get_schedule.json");
    let outcomes = get_schedule_to_blocks(&db(), payload).unwrap();
    assert_eq!(outcomes.len(), 3);

    let (blocks, diagnostics) = available(outcomes["ana@example.com"].clone());
    // The zero-length reminder is dropped; the busy and working-elsewhere items
    // convert.
    assert_eq!(blocks.len(), 2);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, "zero_length_event");
    // A free/busy item's position is not a provider event ID, so the finding
    // carries none.
    assert!(diagnostics[0].event_id.is_none());
    assert!(diagnostics[0].message.contains("schedule item 2"));
    for annotated in &blocks {
        // Free/busy carries no event identity and no all-day flag.
        assert!(annotated.event_id.is_none());
        assert!(annotated.block.event_key.is_none());
        assert!(!annotated.block.all_day);
    }
    assert_eq!(blocks[0].block.status, BlockStatus::Busy);
    assert_eq!(
        tag_names(&blocks[1].block),
        ["status/working-elsewhere"],
        "workingElsewhere reads as free and is tagged"
    );

    let (code, message) = unavailable(outcomes["room-vienna@example.com"].clone());
    assert_eq!(code, "provider_error");
    assert!(message.contains("not resolvable"), "{message}");

    let (blocks, _) = available(outcomes["bo@example.com"].clone());
    assert!(blocks.is_empty());
}

#[test]
fn get_schedule_rejects_a_response_without_a_per_schedule_answer() {
    for payload in [
        "not json",
        r#"{ "value": null }"#,
        r#"{ "@odata.context": "x" }"#,
        r#"{ "value": [{ "scheduleItems": [] }] }"#,
        r#"{ "value": [{ "scheduleId": "a" }, { "scheduleId": "a" }] }"#,
    ] {
        let error = get_schedule_to_blocks(&db(), payload).unwrap_err();
        assert_eq!(error.code(), "payload_invalid", "{payload}: {error}");
    }
}

#[test]
fn a_schedule_that_names_no_items_is_not_silently_free() {
    // A schedule entry with neither `scheduleItems` nor an `error` says nothing
    // about the mailbox's time. An empty block list would read as "completely
    // free", which is the reading the contract forbids for an unanswerable
    // calendar.
    for payload in [
        r#"{ "value": [{ "scheduleId": "a" }] }"#,
        r#"{ "value": [{ "scheduleId": "a", "scheduleItems": null }] }"#,
    ] {
        let error = get_schedule_to_blocks(&db(), payload).unwrap_err();
        assert_eq!(error.code(), "payload_invalid", "{payload}: {error}");
    }

    // A schedule that reports an explicit empty list is free in the window.
    let outcomes = get_schedule_to_blocks(
        &db(),
        r#"{ "value": [{ "scheduleId": "a", "scheduleItems": [] }] }"#,
    )
    .unwrap();
    let (blocks, _) = available(outcomes["a"].clone());
    assert!(blocks.is_empty());

    // A provider error is still the per-calendar answer, not a payload failure.
    let outcomes = get_schedule_to_blocks(
        &db(),
        r#"{ "value": [{ "scheduleId": "a", "error": { "message": "denied" } }] }"#,
    )
    .unwrap();
    assert_eq!(unavailable(outcomes["a"].clone()).0, "provider_error");
}

#[test]
fn working_hours_become_a_window_rule_in_the_zone_the_payload_names() {
    let payload = include_str!("fixtures/graph/working_hours.json");
    let rule = working_hours_to_rule(&db(), payload, "working_hours").unwrap();
    assert_eq!(rule.id, "working_hours");
    assert_eq!(
        rule.who,
        opportune::tags::Selector::Role(opportune::tags::Role::Host)
    );
    let Some(spec) = rule.window() else {
        panic!("the working hours become a window rule");
    };
    // The Windows name resolves to the IANA zone, so the engine can resolve the
    // rule without the CLDR data.
    assert_eq!(spec.zone(), &RuleZone::Named(zone("America/New_York")),);
    let monday = &spec.weekly()[0];
    assert_eq!(monday.len(), 1);
    assert_eq!(monday[0].start(), TimeOfDay::new(9, 0).unwrap());
    assert_eq!(monday[0].end().to_string(), "17:00");
    assert!(spec.weekly()[5].is_empty(), "Saturday is not a working day");

    // The rule holds on a Monday and not before the working day starts.
    let new_york = db().zone("America/New_York").unwrap();
    let at = |date: Date, hour: u8| {
        let LocalResolution::Unique(instant) = new_york
            .resolve_local(date, TimeOfDay::new(hour, 0).unwrap())
            .unwrap()
        else {
            panic!("the local time exists once");
        };
        instant
    };
    let starts = [
        (Date::new(2027, 3, 8).unwrap(), 10),
        (Date::new(2027, 3, 8).unwrap(), 8),
        (Date::new(2027, 3, 13).unwrap(), 10),
    ];
    let instants = starts.map(|(date, hour)| at(date, hour));
    let zones = opportune::time::ZoneSchedule::fixed(new_york);
    assert!(spec.contains(instants[0], &zones));
    assert!(!spec.contains(instants[1], &zones));
    assert!(!spec.contains(instants[2], &zones));
}

#[test]
fn a_custom_time_zone_fails_only_the_working_hours() {
    let payload = json!({
        "daysOfWeek": ["monday"],
        "startTime": "09:00:00.0000000",
        "endTime": "17:00:00.0000000",
        "timeZone": {
            "@odata.type": "#microsoft.graph.customTimeZone",
            "bias": 480,
            "name": "Customized Time Zone"
        }
    })
    .to_string();
    let error = working_hours_to_rule(&db(), &payload, "working_hours").unwrap_err();
    assert_eq!(error, WorkingHoursError::CustomTimeZone);
    assert_eq!(error.code(), "custom_time_zone");

    // The type alone identifies the custom form: Graph's own responses carry the
    // placeholder name `Customized Time Zone` with it.
    let typed = json!({
        "daysOfWeek": ["monday"],
        "startTime": "09:00:00.0000000",
        "endTime": "17:00:00.0000000",
        "timeZone": { "@odata.type": "microsoft.graph.customTimeZone" }
    })
    .to_string();
    assert_eq!(
        working_hours_to_rule(&db(), &typed, "working_hours").unwrap_err(),
        WorkingHoursError::CustomTimeZone
    );

    // The blocks from the response that carried it are unaffected: the two
    // conversions are independent.
    let schedule = include_str!("fixtures/graph/get_schedule.json");
    let outcomes = get_schedule_to_blocks(&db(), schedule).unwrap();
    let (blocks, _) = available(outcomes["ana@example.com"].clone());
    assert_eq!(blocks.len(), 2);
}

#[test]
fn working_hours_that_cannot_describe_a_window_fail_the_conversion() {
    let cases = [
        // No days: the window would close every day.
        json!({
            "daysOfWeek": [],
            "startTime": "09:00:00.0000000",
            "endTime": "17:00:00.0000000",
            "timeZone": { "name": "UTC" }
        }),
        json!({
            "startTime": "09:00:00.0000000",
            "endTime": "17:00:00.0000000",
            "timeZone": { "name": "UTC" }
        }),
        json!({
            "daysOfWeek": ["funday"],
            "startTime": "09:00:00.0000000",
            "endTime": "17:00:00.0000000",
            "timeZone": { "name": "UTC" }
        }),
        json!({
            "daysOfWeek": ["monday"],
            "startTime": "09:00:30.0000000",
            "endTime": "17:00:00.0000000",
            "timeZone": { "name": "UTC" }
        }),
        json!({
            "daysOfWeek": ["monday"],
            "startTime": "09:00:00.0000000",
            "endTime": "09:00:00.0000000",
            "timeZone": { "name": "UTC" }
        }),
        json!({
            "daysOfWeek": ["monday"],
            "startTime": "09:00:00.0000000",
            "endTime": "17:00:00.0000000"
        }),
        // A time zone object without a name is a payload defect, not an unresolvable
        // name.
        json!({
            "daysOfWeek": ["monday"],
            "startTime": "09:00:00.0000000",
            "endTime": "17:00:00.0000000",
            "timeZone": {}
        }),
        json!({
            "daysOfWeek": ["monday"],
            "startTime": "",
            "endTime": "17:00:00.0000000",
            "timeZone": { "name": "UTC" }
        }),
        json!("not a working hours object"),
    ];
    for case in cases {
        let payload = case.to_string();
        let error = working_hours_to_rule(&db(), &payload, "working_hours").unwrap_err();
        assert_eq!(error.code(), "invalid_working_hours", "{payload}: {error}");
    }
}

#[test]
fn working_hours_ending_at_midnight_close_the_end_of_the_day() {
    let payload = json!({
        "daysOfWeek": ["monday", "Tuesday"],
        "startTime": "00:00",
        "endTime": "00:00",
        "timeZone": { "name": "UTC" }
    })
    .to_string();
    // `00:00` to `00:00` names no range at all, which is a payload defect.
    assert_eq!(
        working_hours_to_rule(&db(), &payload, "working_hours")
            .unwrap_err()
            .code(),
        "invalid_working_hours"
    );

    let overnight = json!({
        "daysOfWeek": ["monday"],
        "startTime": "22:00",
        "endTime": "00:00",
        "timeZone": { "name": "UTC" }
    })
    .to_string();
    let rule = working_hours_to_rule(&db(), &overnight, "working_hours").unwrap();
    assert!(matches!(rule.kind, RuleKind::Window(_)));
    assert_eq!(
        rule.window().unwrap().weekly()[0][0].end().to_string(),
        "24:00"
    );
}

#[test]
fn working_hours_with_an_iana_zone_need_no_cldr_mapping() {
    let payload = json!({
        "daysOfWeek": ["monday", "tuesday"],
        "startTime": "08:00:00.0000000",
        "endTime": "16:00:00.0000000",
        "timeZone": { "name": "Europe/London" }
    })
    .to_string();
    let rule = working_hours_to_rule(&db(), &payload, "working_hours").unwrap();
    assert_eq!(
        rule.window().unwrap().zone(),
        &RuleZone::Named(zone("Europe/London"))
    );
}

/// A mailbox whose working hours name a Windows zone, as Graph reports them.
fn windows_working_hours() -> String {
    json!({
        "daysOfWeek": ["monday", "tuesday", "wednesday", "thursday", "friday"],
        "startTime": "09:00:00.0000000",
        "endTime": "17:00:00.0000000",
        "timeZone": { "name": "Eastern Standard Time" }
    })
    .to_string()
}

#[cfg(feature = "windows-zones")]
#[test]
fn a_windows_working_hours_zone_resolves_through_the_cldr_mapping() {
    let rule = working_hours_to_rule(&db(), &windows_working_hours(), "working_hours").unwrap();
    assert_eq!(
        rule.window().unwrap().zone(),
        &RuleZone::Named(zone("America/New_York"))
    );
}

#[cfg(not(feature = "windows-zones"))]
#[test]
fn a_windows_working_hours_zone_needs_the_cldr_mapping() {
    let error =
        working_hours_to_rule(&db(), &windows_working_hours(), "working_hours").unwrap_err();
    assert_eq!(
        error,
        WorkingHoursError::UnresolvedZone {
            name: "Eastern Standard Time".to_owned()
        }
    );
    assert_eq!(error.code(), "zone_unknown");
}

#[test]
fn the_exported_property_ids_and_expand_clause_agree() {
    assert_eq!(
        EXTENDED_PROPERTY_TAGS_ID,
        format!("String {{{EXTENDED_PROPERTY_NAMESPACE_GUID}}} Name opportune.tags")
    );
    assert_eq!(
        EXTENDED_PROPERTY_EVENT_KEY_ID,
        format!("String {{{EXTENDED_PROPERTY_NAMESPACE_GUID}}} Name opportune.event_key")
    );
    assert_eq!(
        EXTENDED_PROPERTY_EXPAND,
        format!(
            "singleValueExtendedProperties($filter=id eq '{EXTENDED_PROPERTY_TAGS_ID}' or id eq \
             '{EXTENDED_PROPERTY_EVENT_KEY_ID}')"
        )
    );
    assert!(EXTENDED_PROPERTY_EXPAND.contains(&format!("{{{EXTENDED_PROPERTY_NAMESPACE_GUID}}}")));
}

#[test]
fn malformed_signed_or_extra_clock_fields_are_rejected_in_events_and_working_hours() {
    for bad_time in ["+9:00", "09:00::..00", "09:00:+5", "09:00.000"] {
        let wh = json!({
            "daysOfWeek": ["monday"],
            "startTime": bad_time,
            "endTime": "17:00:00.0000000",
            "timeZone": { "name": "UTC" }
        })
        .to_string();
        let error = working_hours_to_rule(&db(), &wh, "working_hours").unwrap_err();
        assert_eq!(
            error.code(),
            "invalid_working_hours",
            "expected workingHours rejection for {bad_time}"
        );

        let cal = json!({
            "value": [{
                "id": "evt-bad-clock",
                "isAllDay": false,
                "showAs": "busy",
                "start": { "dateTime": format!("2027-03-09T{bad_time}"), "timeZone": "UTC" },
                "end": { "dateTime": "2027-03-09T10:00:00", "timeZone": "UTC" }
            }]
        })
        .to_string();
        let (code, _) = unavailable(calendar_view_to_blocks(&db(), &cal, &zone("UTC"), None));
        assert_eq!(
            code, "payload_invalid",
            "expected calendarView rejection for {bad_time}"
        );
    }
}

#[test]
fn occurrences_of_one_series_get_distinct_keys_from_their_original_start() {
    // `calendarView` expands a series into occurrences that share one `iCalUId`.
    // The original slot is what keeps them distinct events: one shared key would
    // merge a whole series into a single hull event in a cap count.
    let occurrence = |id: &str, original: &str| {
        json!({
            "id": id,
            "type": "occurrence",
            "seriesMasterId": "AAMk-series",
            "iCalUId": "series-ical",
            "isAllDay": false,
            "showAs": "busy",
            "originalStart": original,
            "start": { "dateTime": "2027-03-09T09:00:00.0000000", "timeZone": "UTC" },
            "end": { "dateTime": "2027-03-09T10:00:00.0000000", "timeZone": "UTC" }
        })
    };
    let payload = calendar_view(vec![
        occurrence("occ-1", "2027-03-09T09:00:00.0000000Z"),
        occurrence("occ-2", "2027-03-16T09:00:00.0000000Z"),
    ]);
    let (blocks, _) = available(calendar_view_to_blocks(&db(), &payload, &zone("UTC"), None));
    assert_eq!(blocks.len(), 2);
    assert_eq!(
        block_for(&blocks, "occ-1").event_key.as_deref(),
        Some("series-ical@2027-03-09T09:00:00Z")
    );
    assert_eq!(
        block_for(&blocks, "occ-2").event_key.as_deref(),
        Some("series-ical@2027-03-16T09:00:00Z")
    );

    // An exception is an instance too, so it carries the same discriminator.
    let exception = json!({
        "id": "exc-1",
        "type": "exception",
        "seriesMasterId": "AAMk-series",
        "iCalUId": "series-ical",
        "isAllDay": false,
        "showAs": "busy",
        "originalStart": "2027-03-23T09:00:00.0000000Z",
        "start": { "dateTime": "2027-03-23T11:00:00.0000000", "timeZone": "UTC" },
        "end": { "dateTime": "2027-03-23T12:00:00.0000000", "timeZone": "UTC" }
    });
    let payload = calendar_view(vec![exception]);
    let (blocks, _) = available(calendar_view_to_blocks(&db(), &payload, &zone("UTC"), None));
    assert_eq!(
        block_for(&blocks, "exc-1").event_key.as_deref(),
        Some("series-ical@2027-03-23T09:00:00Z")
    );

    // A single instance has no original start and keeps the provider ID alone.
    let payload = calendar_view(vec![timed_event(
        "evt-single",
        "busy",
        "2027-03-09T09:00:00.0000000",
        "2027-03-09T10:00:00.0000000",
        "UTC",
    )]);
    let (blocks, _) = available(calendar_view_to_blocks(&db(), &payload, &zone("UTC"), None));
    assert_eq!(
        block_for(&blocks, "evt-single").event_key.as_deref(),
        Some("ical-evt-single")
    );
}

#[test]
fn a_series_level_marker_takes_the_original_start_too() {
    // A shared marker written once on a series reaches every occurrence, so it
    // needs the same discriminator `iCalUId` does. A single event's marker is the
    // caller's own key and is returned exactly as written.
    let marker = json!({
        "id": EXTENDED_PROPERTY_EVENT_KEY_ID,
        "value": "booking-42"
    });
    let occurrence = |id: &str, original: &str| {
        json!({
            "id": id,
            "type": "occurrence",
            "seriesMasterId": "AAMk-series",
            "iCalUId": "series-ical",
            "isAllDay": false,
            "showAs": "busy",
            "originalStart": original,
            "start": { "dateTime": "2027-03-09T09:00:00.0000000", "timeZone": "UTC" },
            "end": { "dateTime": "2027-03-09T10:00:00.0000000", "timeZone": "UTC" },
            "singleValueExtendedProperties": [marker.clone()]
        })
    };
    let single = json!({
        "id": "evt-single",
        "type": "singleInstance",
        "iCalUId": "single-ical",
        "isAllDay": false,
        "showAs": "busy",
        "start": { "dateTime": "2027-03-09T09:00:00.0000000", "timeZone": "UTC" },
        "end": { "dateTime": "2027-03-09T10:00:00.0000000", "timeZone": "UTC" },
        "singleValueExtendedProperties": [marker]
    });
    let payload = calendar_view(vec![
        occurrence("occ-1", "2027-03-09T09:00:00.0000000Z"),
        occurrence("occ-2", "2027-03-16T09:00:00.0000000Z"),
        single,
    ]);
    let (blocks, _) = available(calendar_view_to_blocks(&db(), &payload, &zone("UTC"), None));
    assert_eq!(
        block_for(&blocks, "occ-1").event_key.as_deref(),
        Some("booking-42@2027-03-09T09:00:00Z")
    );
    assert_eq!(
        block_for(&blocks, "occ-2").event_key.as_deref(),
        Some("booking-42@2027-03-16T09:00:00Z")
    );
    assert_eq!(
        block_for(&blocks, "evt-single").event_key.as_deref(),
        Some("booking-42")
    );
}

#[test]
fn a_timed_calendar_never_reads_the_participant_zone() {
    // The participant's zone is only read for all-day events, so a timed-only
    // calendar converts even when the name is one nothing resolves.
    let payload = calendar_view(vec![timed_event(
        "evt-timed",
        "busy",
        "2027-03-09T09:00:00.0000000",
        "2027-03-09T10:00:00.0000000",
        "UTC",
    )]);
    let (blocks, _) = available(calendar_view_to_blocks(
        &db(),
        &payload,
        &zone("Mars/Olympus_Mons"),
        None,
    ));
    assert_eq!(blocks.len(), 1);
}

#[test]
fn get_schedule_reads_an_all_free_availability_view_without_schedule_items() {
    let outcomes = get_schedule_to_blocks(
        &db(),
        r#"{
          "value": [
            { "scheduleId": "free@example.com", "availabilityView": "000000000" },
            { "scheduleId": "empty@example.com", "availabilityView": "" },
            {
              "scheduleId": "busy@example.com",
              "availabilityView": "002000000"
            },
            {
              "scheduleId": "ok@example.com",
              "availabilityView": "022000000",
              "scheduleItems": [{
                "status": "busy",
                "start": { "dateTime": "2027-03-09T09:00:00.0000000", "timeZone": "UTC" },
                "end": { "dateTime": "2027-03-09T10:00:00.0000000", "timeZone": "UTC" }
              }]
            }
          ]
        }"#,
    )
    .unwrap();
    // An all-free merged view is the answer Graph sends for a free mailbox.
    let (blocks, diagnostics) = available(outcomes["free@example.com"].clone());
    assert!(blocks.is_empty());
    assert!(diagnostics.is_empty());
    let (blocks, _) = available(outcomes["empty@example.com"].clone());
    assert!(blocks.is_empty());

    // Busy slots with no items are that one mailbox's failure, not the batch's.
    assert_eq!(
        unavailable(outcomes["busy@example.com"].clone()).0,
        "payload_invalid"
    );

    // The healthy schedule in the same response still converts its items.
    let (blocks, _) = available(outcomes["ok@example.com"].clone());
    assert_eq!(blocks.len(), 1);
}

#[test]
fn a_name_only_custom_time_zone_is_recognised_as_custom() {
    // Graph writes `Customized Time Zone` as the name of a bias-and-offset zone.
    // A payload that carries only the name is still that form, not a zone name
    // nothing resolves.
    let payload = json!({
        "daysOfWeek": ["monday"],
        "startTime": "09:00:00.0000000",
        "endTime": "17:00:00.0000000",
        "timeZone": { "name": "Customized Time Zone" }
    })
    .to_string();
    let error = working_hours_to_rule(&db(), &payload, "working_hours").unwrap_err();
    assert_eq!(error, WorkingHoursError::CustomTimeZone);
    assert_eq!(error.code(), "custom_time_zone");
}
