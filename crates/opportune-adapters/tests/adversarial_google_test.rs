//! Adversarial probes over the Google conversions: the shapes Google itself emits
//! for a calendar with no events and a calendar with nothing busy in the window.
//!
//! Google's JSON serialisation omits an empty repeated field rather than writing
//! `[]`. Its own Node and Python quickstarts therefore guard the absence
//! (`const events = result.data.items; if (!events || events.length === 0)`)
//! rather than indexing it, and a tool that indexed it crashed with
//! `KeyError: 'items'` against a real calendar. A participant with nothing in the
//! window is the ordinary case for an availability search, so a payload with no
//! `items` — or a `freeBusy` calendar entry with no `busy` — must convert as a
//! free calendar rather than as a calendar the adapter could not read.

// Every case resolves zone names against a real database, so this target runs
// where the crate compiles one in: `make test-matrix`'s `graph` build keeps
// `bundled-tzdb` on, and checks the bundle-free build separately.
#![cfg(all(feature = "google", feature = "bundled-tzdb"))]

use opportune::model::BlockStatus;
use opportune::time::{TimeZoneDatabase, ZoneName};
use opportune_adapters::google::{events_list_to_blocks, free_busy_to_blocks};
use opportune_adapters::{AdapterDiagnostic, AnnotatedBlock, CalendarOutcome};

fn db() -> TimeZoneDatabase {
    TimeZoneDatabase::bundled().expect("the bundled database is compiled in")
}

fn zone(name: &str) -> ZoneName {
    ZoneName::new(name).unwrap()
}

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

#[test]
fn an_empty_events_list_collection_without_items_is_a_free_calendar() {
    // A window with no events, as `events.list` writes it: the collection's own
    // metadata and no `items` key.
    let payload = r#"{
      "kind": "calendar#events",
      "etag": "\"p32o8nsu6u1m7f\"",
      "summary": "ana@example.com",
      "updated": "2027-03-01T14:22:09.512Z",
      "timeZone": "America/New_York",
      "accessRole": "owner",
      "defaultReminders": [],
      "nextSyncToken": "CJDUzdKmzZMDFOiPnNAGGAU="
    }"#;
    let (blocks, diagnostics) = available(events_list_to_blocks(
        &db(),
        payload,
        &zone("America/New_York"),
    ));
    assert!(
        blocks.is_empty(),
        "a calendar with no events has no blocks, got {blocks:?}"
    );
    assert!(diagnostics.is_empty(), "{diagnostics:?}");

    // The same collection with the empty array written out is still free: both
    // spellings of "no events" take one reading.
    let explicit = r#"{ "kind": "calendar#events", "timeZone": "UTC", "items": [] }"#;
    let (blocks, _) = available(events_list_to_blocks(&db(), explicit, &zone("UTC")));
    assert!(blocks.is_empty(), "{blocks:?}");
}

#[test]
fn a_free_busy_calendar_without_a_busy_array_is_free_rather_than_failing_the_batch() {
    // Google omits the empty `busy` array, so a calendar with nothing busy comes
    // back as an entry carrying neither `busy` nor `errors`.
    let payload = r#"{
      "kind": "calendar#freeBusy",
      "timeMin": "2027-03-09T00:00:00.000Z",
      "timeMax": "2027-03-10T00:00:00.000Z",
      "calendars": {
        "room-4@example.com": {},
        "room-vienna@example.com": {
          "busy": [{ "start": "2027-03-09T10:00:00Z", "end": "2027-03-09T11:00:00Z" }]
        }
      }
    }"#;
    let outcomes = free_busy_to_blocks(payload).expect("a readable freeBusy response");
    let (blocks, diagnostics) = available(outcomes["room-4@example.com"].clone());
    assert!(blocks.is_empty(), "the free room has no busy ranges");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    // The healthy calendar in the same response still converts.
    assert_eq!(
        available(outcomes["room-vienna@example.com"].clone())
            .0
            .len(),
        1
    );

    // An empty `errors` array alongside ranges is not a provider error, and a
    // non-empty one wins over any ranges that came back: the calendar's time is
    // unknown, so it cannot be read as partly busy.
    let outcomes = free_busy_to_blocks(
        r#"{
          "calendars": {
            "healthy@example.com": {
              "errors": [],
              "busy": [{ "start": "2027-03-09T10:00:00Z", "end": "2027-03-09T11:00:00Z" }]
            },
            "unknown@example.com": {
              "errors": [{ "domain": "global", "reason": "internalError" }],
              "busy": [{ "start": "2027-03-09T10:00:00Z", "end": "2027-03-09T11:00:00Z" }]
            }
          }
        }"#,
    )
    .expect("a readable freeBusy response");
    assert_eq!(
        available(outcomes["healthy@example.com"].clone()).0.len(),
        1
    );
    assert!(matches!(
        outcomes["unknown@example.com"],
        CalendarOutcome::Unavailable {
            code: "provider_error",
            ..
        }
    ));
}

#[test]
fn a_document_that_is_not_a_provider_collection_is_still_refused() {
    // The repair above must not turn a provider error body into free time.
    for payload in [
        r#"{ "error": { "code": 403, "message": "forbidden", "errors": [] } }"#,
        r#"{ "timeZone": "UTC" }"#,
        r#"{ "items": null }"#,
        "{}",
        // A page with no events still hides the events on the pages after it.
        r#"{ "kind": "calendar#events", "nextPageToken": "CiAKGjBpNDd2Nmp2" }"#,
    ] {
        let outcome = events_list_to_blocks(&db(), payload, &zone("UTC"));
        match outcome {
            CalendarOutcome::Unavailable { code, .. } => {
                assert_eq!(code, "payload_invalid", "{payload}");
            }
            CalendarOutcome::Available { blocks, .. } => {
                panic!(
                    "`{payload}` is not an empty calendar, but read as {} blocks",
                    blocks.len()
                );
            }
        }
    }
    assert_eq!(
        free_busy_to_blocks(r#"{ "error": { "code": 403, "message": "forbidden" } }"#)
            .unwrap_err()
            .code(),
        "payload_invalid"
    );
}

#[test]
fn an_event_without_transparency_blocks() {
    // `transparency` is optional and Google omits it for its default, `opaque`, so
    // an event with no `transparency`, no `status` and no `eventType` is the
    // ordinary busy event.
    let payload = r#"{
      "kind": "calendar#events",
      "timeZone": "UTC",
      "items": [{
        "id": "evt-plain",
        "iCalUID": "plain@example.com",
        "start": { "dateTime": "2027-03-09T09:00:00Z" },
        "end": { "dateTime": "2027-03-09T10:00:00Z" }
      }]
    }"#;
    let (blocks, diagnostics) = available(events_list_to_blocks(&db(), payload, &zone("UTC")));
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].block.status, BlockStatus::Busy);
    assert!(
        blocks[0].block.tags.is_empty(),
        "{:?}",
        blocks[0].block.tags
    );
}
