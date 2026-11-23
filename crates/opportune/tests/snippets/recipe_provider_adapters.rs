// A Graph `calendarView` and a Google `events.list` converted into participants,
// with a calendar the provider could not answer for left unknown rather than free.

use std::collections::BTreeMap;

use opportune::Engine;
use opportune::model::{Goal, Participant, Request, Requirement, Span, Term};
use opportune::presets;
use opportune::tags::{Selector, Tag};
use opportune::time::{
    Date, Duration, Instant, RuleZone, TimeOfDay, WallEnd, Weekdays, ZoneName,
};
use opportune_adapters::graph;
use opportune_adapters::google;
use opportune_adapters::CalendarOutcome;

const CALENDAR_VIEW: &str = r#"{
  "value": [
    {
      "id": "AAMk-interview",
      "isAllDay": false,
      "showAs": "busy",
      "categories": ["Interview"],
      "start": { "dateTime": "2027-06-14T09:00:00.0000000", "timeZone": "UTC" },
      "end": { "dateTime": "2027-06-14T10:00:00.0000000", "timeZone": "UTC" }
    }
  ]
}"#;

const WORKING_HOURS: &str = r#"{
  "daysOfWeek": ["monday", "tuesday", "wednesday", "thursday", "friday"],
  "startTime": "09:00:00.0000000",
  "endTime": "17:00:00.0000000",
  "timeZone": { "name": "America/New_York" }
}"#;

const EVENTS_LIST: &str = r#"{
  "kind": "calendar#events",
  "timeZone": "America/New_York",
  "items": [
    {
      "id": "evt-2",
      "status": "confirmed",
      "start": { "dateTime": "2027-06-14T10:00:00-04:00", "timeZone": "America/New_York" },
      "end": { "dateTime": "2027-06-14T11:00:00-04:00", "timeZone": "America/New_York" }
    }
  ]
}"#;

// A page is not the response: the events after `nextPageToken` are missing from
// the document, so converting the page alone would read them as free time.
const FIRST_PAGE: &str = r#"{
  "kind": "calendar#events",
  "timeZone": "America/New_York",
  "nextPageToken": "CJDUzdKmzZMDFOiPnNAGGAU=",
  "items": []
}"#;

fn utc(date: (i16, i8, i8), clock: (u8, u8)) -> Instant {
    Instant::from_utc(
        Date::new(date.0, date.1, date.2).expect("a valid civil date"),
        TimeOfDay::new(clock.0, clock.1).expect("a valid time of day"),
    )
    .expect("UTC has no transitions")
}

/// The participant a conversion describes: their blocks when the provider
/// answered, and no calendar at all when it did not. An empty calendar would read
/// as completely free, which is the one thing an errored mailbox does not mean.
fn participant(id: &str, zone: &ZoneName, outcome: CalendarOutcome) -> Participant {
    match outcome {
        CalendarOutcome::Available { blocks, .. } => Participant::new(id, zone.clone())
            .with_calendar(blocks.into_iter().map(|entry| entry.block).collect()),
        CalendarOutcome::Unavailable { .. } => Participant::new(id, zone.clone()),
    }
}

fn working_hours_window(
    engine: &Engine,
    payload: &str,
    zone: &ZoneName,
) -> Result<opportune::model::Rule, Box<dyn std::error::Error>> {
    let rule = match graph::working_hours_to_rule(
        engine.time_zone_database(),
        payload,
        "working_hours",
    ) {
        Ok(rule) => rule,
        Err(error) => {
            assert_eq!(error.code(), "custom_time_zone");
            presets::business_hours(
                RuleZone::Named(zone.clone()),
                Weekdays::weekdays(),
                TimeOfDay::new(9, 0)?,
                WallEnd::Time(TimeOfDay::new(17, 0)?),
            )?
        }
    };
    Ok(rule.with_who(Selector::ids(["ana"])))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine = Engine::builder().build()?;
    let database = engine.time_zone_database();
    let new_york = ZoneName::new("America/New_York")?;

    // The query that asks Graph for the markers carries this clause in `$expand`;
    // the response is what the adapter reads.
    assert!(graph::EXTENDED_PROPERTY_EXPAND.contains("opportune.event_key"));

    let categories = BTreeMap::from([("Interview".to_owned(), Tag::new("interview")?)]);
    let view = graph::calendar_view_to_blocks(database, CALENDAR_VIEW, &new_york, Some(&categories));
    let ana = participant("ana", &new_york, view);
    let blocks = ana.calendar.as_deref().ok_or("the calendar view converts")?;
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].tags, vec![Tag::new("interview")?]);

    // A mailbox Graph could not answer for keeps its participant, without a
    // calendar: its offers carry `calendar_unknown` instead of reading as free.
    let broken = graph::calendar_view_to_blocks(database, r#"{"value":"not a list"}"#, &new_york, None);
    assert!(matches!(
        broken,
        CalendarOutcome::Unavailable {
            code: "payload_invalid",
            ..
        }
    ));
    assert!(participant("bo", &new_york, broken).calendar.is_none());

    // The mailbox's own working hours, scoped to the mailbox's owner, with the
    // caller's window as the fallback for a custom Graph time zone the adapter
    // does not resolve.
    let window = working_hours_window(&engine, WORKING_HOURS, &new_york)?;
    assert_eq!(window.id, "working_hours");
    assert_eq!(window.who, Selector::ids(["ana"]));

    let events = google::events_list_to_blocks(database, EVENTS_LIST, &new_york);
    let cy = participant("cy", &new_york, events);
    assert_eq!(cy.blocks().len(), 1);
    let page = google::events_list_to_blocks(database, FIRST_PAGE, &new_york);
    assert!(matches!(
        page,
        CalendarOutcome::Unavailable {
            code: "payload_invalid",
            ..
        }
    ));

    let mut request = Request::new(
        utc((2027, 6, 14), (0, 0)),
        Goal::new(
            new_york,
            Span::new(utc((2027, 6, 14), (0, 0)), utc((2027, 6, 15), (0, 0)))?,
            vec![opportune::Part::new(
                Duration::from_minutes(30),
                Requirement::new(vec![Term::person("ana")]),
            )],
        ),
    );
    request.participants = vec![ana, cy];
    request.rules = vec![window];
    assert!(!engine.find_availability(&request)?.offers.is_empty());
    Ok(())
}
