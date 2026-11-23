//! The JSON envelopes over the Google entry points.

// Every case resolves zone names against a real database, so this target runs
// where the crate compiles one in: `make test-matrix`'s `graph` build keeps
// `bundled-tzdb` on, and checks the bundle-free build separately.
#![cfg(all(feature = "json", feature = "google", feature = "bundled-tzdb"))]

use opportune::time::TimeZoneDatabase;
use opportune_adapters::json::{
    WireCalendarOutcome, events_list_to_blocks, events_list_to_blocks_with_db, free_busy_to_blocks,
};
use serde_json::{Value, json};

/// One event whose conversion exercises the block wire shape, plus a zero-length
/// one that only leaves a diagnostic.
const CALENDAR: &str = r#"{
  "timeZone": "America/New_York",
  "items": [{
    "id": "gcal-1",
    "status": "confirmed",
    "transparency": "opaque",
    "iCalUID": "series@example.com",
    "originalStartTime": { "dateTime": "2027-03-09T10:00:00-05:00" },
    "start": { "dateTime": "2027-03-09T10:00:00-05:00" },
    "end": { "dateTime": "2027-03-09T11:00:00-05:00" },
    "eventType": "outOfOffice",
    "extendedProperties": { "shared": { "opportune.tags": "interview/front-end" } }
  }, {
    "id": "gcal-zero",
    "status": "confirmed",
    "iCalUID": "zero@example.com",
    "start": { "dateTime": "2027-03-09T10:00:30-05:00" },
    "end": { "dateTime": "2027-03-09T10:00:30-05:00" }
  }, {
    "id": "gcal-all-day",
    "status": "confirmed",
    "iCalUID": "day@example.com",
    "start": { "date": "2027-03-11" },
    "end": { "date": "2027-03-12" }
  }]
}"#;

#[test]
fn a_calendar_envelope_carries_blocks_and_diagnostics() {
    let outcome = events_list_to_blocks(CALENDAR, "Europe/London").unwrap();
    let value: Value = serde_json::from_str(&outcome).unwrap();
    assert_eq!(value["status"], "available");
    assert_eq!(value["blocks"].as_array().unwrap().len(), 2);
    assert_eq!(value["blocks"][0]["event_id"], "gcal-1");
    let block = &value["blocks"][0]["block"];
    assert_eq!(
        block["span"],
        json!(["2027-03-09T15:00Z", "2027-03-09T16:00Z"])
    );
    assert_eq!(block["status"], "busy");
    assert_eq!(block["all_day"], false);
    assert_eq!(block["tags"], json!(["status/oof", "interview/front-end"]));
    assert_eq!(
        block["event_key"],
        "series@example.com@2027-03-09T15:00:00Z"
    );
    // The all-day event resolves in the response's zone, not the participant's.
    assert_eq!(value["blocks"][1]["block"]["all_day"], true);
    assert_eq!(
        value["blocks"][1]["block"]["span"],
        json!(["2027-03-11T05:00Z", "2027-03-12T05:00Z"])
    );
    assert_eq!(value["diagnostics"][0]["code"], "zero_length_event");
    assert_eq!(value["diagnostics"][0]["event_id"], "gcal-zero");
    assert!(value["diagnostics"][0]["message"].is_string());

    // The envelope is closed and symmetric: what the adapter wrote parses back
    // to the same bytes, so a binding can read it with the same models.
    let parsed: WireCalendarOutcome = serde_json::from_str(&outcome).unwrap();
    assert_eq!(serde_json::to_string(&parsed).unwrap(), outcome);
}

#[test]
fn a_calendar_that_cannot_convert_is_an_unavailable_envelope() {
    let payload = json!({
        "items": [{
            "id": "gcal-series",
            "status": "confirmed",
            "iCalUID": "series@example.com",
            "recurrence": ["RRULE:FREQ=WEEKLY;BYDAY=TU"],
            "start": { "dateTime": "2027-03-09T10:00:00-05:00" },
            "end": { "dateTime": "2027-03-09T11:00:00-05:00" }
        }]
    })
    .to_string();
    let outcome = events_list_to_blocks(&payload, "UTC").unwrap();
    let value: Value = serde_json::from_str(&outcome).unwrap();
    assert_eq!(value["status"], "unavailable");
    assert_eq!(value["code"], "unexpanded_series");
    assert!(value.get("blocks").is_none());
    let _: WireCalendarOutcome = serde_json::from_str(&outcome).unwrap();
}

#[test]
fn a_free_busy_envelope_is_keyed_by_calendar_id() {
    let outcome = free_busy_to_blocks(include_str!("fixtures/google/free_busy.json")).unwrap();
    let value: Value = serde_json::from_str(&outcome).unwrap();
    assert_eq!(value["room-4@example.com"]["status"], "available");
    assert_eq!(
        value["room-4@example.com"]["blocks"][0]["block"]["span"],
        json!(["2027-03-09T16:00Z", "2027-03-09T17:00Z"])
    );
    // Free/busy carries no event identity.
    assert_eq!(
        value["room-4@example.com"]["blocks"][0]["event_id"],
        Value::Null
    );
    assert_eq!(value["room-vienna@example.com"]["status"], "unavailable");
    assert_eq!(value["room-vienna@example.com"]["code"], "provider_error");
    assert_eq!(value["bo@example.com"]["blocks"], json!([]));
    for calendar in [
        "room-4@example.com",
        "room-vienna@example.com",
        "bo@example.com",
    ] {
        let _: WireCalendarOutcome = serde_json::from_value(value[calendar].clone()).unwrap();
    }
}

#[test]
fn a_payload_failure_is_a_json_error_and_a_blank_zone_is_rejected_first() {
    let error = free_busy_to_blocks("not json").unwrap_err();
    assert_eq!(error.code(), "payload_invalid");

    let error = events_list_to_blocks(CALENDAR, "  ").unwrap_err();
    assert_eq!(error.code(), "zone_name_invalid");
    assert_eq!(error.path(), "/participant_zone");
}

/// The `_with_db` variant reads zone names from the database it is given, so an
/// engine built from a `tzdb_dir` converts payloads against the same data it
/// evaluates with.
#[test]
fn a_with_db_conversion_resolves_zones_in_its_own_database() {
    // A real `TZif` tree holding `America/Sao_Paulo` and no other zone.
    let directory = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/tzdb-2018");
    let db = TimeZoneDatabase::from_dir(directory, None).expect("the fixture directory loads");

    // A zone the directory holds converts; the participant's zone is what an
    // all-day event is read in, so this payload names it in its own `timeZone`.
    let payload = json!({
        "timeZone": "America/Sao_Paulo",
        "items": [{
            "id": "evt-1",
            "status": "confirmed",
            "start": { "dateTime": "2018-07-02T10:00:00Z" },
            "end": { "dateTime": "2018-07-02T11:00:00Z" }
        }]
    })
    .to_string();
    let outcome = events_list_to_blocks_with_db(&db, &payload, "UTC").unwrap();
    let value: Value = serde_json::from_str(&outcome).unwrap();
    assert_eq!(value["status"], "available", "{outcome}");

    // A zone the directory lacks is unknown, although the bundled database has
    // it. An all-day event with no response `timeZone` is read in the
    // participant's zone, so that is where the database difference shows.
    let all_day = json!({
        "items": [{
            "id": "evt-1",
            "status": "confirmed",
            "start": { "date": "2018-07-02" },
            "end": { "date": "2018-07-03" }
        }]
    })
    .to_string();
    let outcome = events_list_to_blocks_with_db(&db, &all_day, "America/New_York").unwrap();
    let value: Value = serde_json::from_str(&outcome).unwrap();
    assert_eq!(value["status"], "unavailable", "{outcome}");
    assert_eq!(value["code"], "zone_unknown");
    let bundled = events_list_to_blocks(&all_day, "America/New_York").unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&bundled).unwrap()["status"],
        "available",
        "the bundled database does resolve the zone, so the difference is the database",
    );
}
