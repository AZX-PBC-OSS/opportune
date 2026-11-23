//! Recorded Google payloads through the adapter into `Engine::find_availability`.
//!
//! The scenario is two interviewers in different zones, a shared recurring
//! interview series, one interview of Ana's own, a pooled daily cap on interviews,
//! Ana's focus time and an all-day day off, and an interview room whose availability
//! comes from free/busy. The assertions read the offers and exclusions the engine
//! produced from the converted blocks, so a conversion that drops, widens or
//! mistags a block shows up as wrong availability rather than as a green adapter
//! unit test.

// Every case resolves zone names against a real database, so this target runs
// where the crate compiles one in: `make test-matrix`'s `graph` build keeps
// `bundled-tzdb` on, and checks the bundle-free build separately.
#![cfg(all(feature = "google", feature = "bundled-tzdb"))]

use opportune::Engine;
use opportune::model::{
    Block, BlockStatus, Cap, CapMeasure, CapPer, CapPeriod, Goal, Part, Participant, Request,
    Requirement, Rule, RuleKind, Span, StatusEffect, StatusPolicy, StatusPolicyEntry, Term,
};
use opportune::presets;
use opportune::reasons::ReasonCode;
use opportune::tags::{Selector, Tag};
use opportune::time::{
    Date, Duration, Instant, LocalResolution, RuleZone, TimeOfDay, TimeZoneDatabase, WallEnd,
    Weekday, Weekdays, ZoneName,
};
use opportune_adapters::google::{events_list_to_blocks, free_busy_to_blocks};
use opportune_adapters::{AnnotatedBlock, CalendarOutcome};

fn db() -> TimeZoneDatabase {
    TimeZoneDatabase::bundled().expect("the bundled database is compiled in")
}

fn zone(name: &str) -> ZoneName {
    ZoneName::new(name).unwrap()
}

/// The UTC instant of a local reading in UTC.
fn utc_at(year: i16, month: i8, day: i8, hour: u8) -> Instant {
    let LocalResolution::Unique(instant) = opportune::time::Zone::utc()
        .resolve_local(
            Date::new(year, month, day).unwrap(),
            TimeOfDay::new(hour, 0).unwrap(),
        )
        .unwrap()
    else {
        panic!("UTC has no transitions");
    };
    instant
}

/// One calendar's blocks, converted from an `events.list` fixture.
fn mailboxes(payload: &str, participant_zone: &str) -> Vec<AnnotatedBlock> {
    let outcome = events_list_to_blocks(&db(), payload, &zone(participant_zone));
    match outcome {
        CalendarOutcome::Available {
            blocks,
            diagnostics,
        } => {
            assert!(diagnostics.is_empty(), "{diagnostics:?}");
            blocks
        }
        CalendarOutcome::Unavailable { code, message } => {
            panic!("the fixture should convert, not fail with `{code}`: {message}");
        }
    }
}

/// One free/busy calendar's blocks.
fn free_busy_calendar(payload: &str, calendar: &str) -> Vec<Block> {
    let outcomes = free_busy_to_blocks(payload).expect("the free/busy response is readable");
    match &outcomes[calendar] {
        CalendarOutcome::Available {
            blocks,
            diagnostics,
        } => {
            assert!(diagnostics.is_empty(), "{diagnostics:?}");
            blocks
                .iter()
                .map(|annotated| annotated.block.clone())
                .collect()
        }
        CalendarOutcome::Unavailable { code, message } => {
            panic!("the calendar should convert, not fail with `{code}`: {message}");
        }
    }
}

/// The first block for `event_id`.
fn block_of<'a>(blocks: &'a [AnnotatedBlock], event_id: &str) -> &'a Block {
    &blocks
        .iter()
        .find(|annotated| annotated.event_id.as_deref() == Some(event_id))
        .unwrap_or_else(|| panic!("no block for `{event_id}`"))
        .block
}

/// The request the recorded calendars are booked against.
fn booking_request(ana: &[AnnotatedBlock], bo: &[AnnotatedBlock], room: Vec<Block>) -> Request {
    // Google has no working-hours API, so Ana's working day is a caller window.
    let working_hours = presets::business_hours(
        RuleZone::Participant,
        Weekdays::range(Weekday::Monday, Weekday::Friday),
        TimeOfDay::new(9, 0).unwrap(),
        WallEnd::Time(TimeOfDay::new(17, 0).unwrap()),
    )
    .expect("09:00-17:00 is a window")
    .with_who(Selector::ids(["ana"]));
    // Focus time blocks by default; this caller's policy does not treat it as
    // occupied time.
    let focus_policy = Rule::new(
        "focus_policy",
        RuleKind::StatusPolicy(StatusPolicy {
            entries: vec![StatusPolicyEntry::tags(
                Selector::Tag(Tag::new("status/focus").unwrap()),
                StatusEffect::Ignore,
            )],
        }),
    );
    let cap = Rule::new(
        "interviews_per_day",
        RuleKind::Cap(Cap::new(
            Selector::Tag(Tag::new("interview").unwrap()),
            CapMeasure::Count,
            2,
            CapPeriod::Day,
            CapPer::Pooled,
            RuleZone::Event,
            Weekday::Monday,
        )),
    );

    let goal = Goal::new(
        zone("UTC"),
        Span::new(utc_at(2027, 3, 8, 0), utc_at(2027, 3, 13, 0)).unwrap(),
        vec![
            Part::new(
                Duration::from_minutes(60),
                Requirement::new(vec![
                    Term::person("ana"),
                    Term::person("bo"),
                    Term::person("room-4"),
                ]),
            )
            // The booking being made is an interview, which is what the cap counts.
            .with_tags(vec![Tag::new("interview").unwrap()]),
        ],
    );
    Request::new(utc_at(2027, 3, 8, 0), goal)
        .with_participants(vec![
            Participant::new("ana", zone("America/New_York")).with_calendar(
                ana.iter()
                    .map(|annotated| annotated.block.clone())
                    .collect(),
            ),
            Participant::new("bo", zone("Asia/Tokyo"))
                .with_calendar(bo.iter().map(|annotated| annotated.block.clone()).collect()),
            Participant::new("room-4", zone("UTC")).with_calendar(room),
        ])
        .with_rules(vec![working_hours, focus_policy, cap])
        .with_explain(1_000)
}

/// The recorded calendars, converted.
fn calendars() -> (Vec<AnnotatedBlock>, Vec<AnnotatedBlock>, Vec<Block>) {
    let ana = mailboxes(
        include_str!("fixtures/google/events_list_ana.json"),
        "America/New_York",
    );
    let bo = mailboxes(
        include_str!("fixtures/google/events_list_bo.json"),
        "Asia/Tokyo",
    );
    let room = free_busy_calendar(
        include_str!("fixtures/google/free_busy.json"),
        "room-4@example.com",
    );
    (ana, bo, room)
}

/// Whether `start` is one of the offer starts.
fn offers(response: &opportune::model::Response, start: Instant) -> bool {
    response
        .offers
        .iter()
        .any(|offer| offer.start() == Some(start))
}

#[test]
fn the_recorded_mailboxes_convert_with_one_key_per_shared_instance() {
    let (ana, bo, room) = calendars();
    // The two mailboxes received the same recurring instance stamped in different
    // offsets, and derived one key from it.
    let shared = block_of(&ana, "ana-shared-interview-0900");
    assert_eq!(
        shared.event_key.as_deref(),
        Some("series-interview@example.com@2027-03-09T15:00:00Z")
    );
    assert_eq!(
        block_of(&bo, "bo-shared-interview-0900").event_key,
        shared.event_key,
        "the same instance in two zones shares one key"
    );
    assert_eq!(
        block_of(&ana, "ana-shared-interview-1000")
            .event_key
            .as_deref(),
        Some("series-interview@example.com@2027-03-10T15:00:00Z")
    );
    assert_eq!(
        block_of(&bo, "bo-shared-interview-1000")
            .event_key
            .as_deref(),
        Some("series-interview@example.com@2027-03-10T15:00:00Z")
    );

    // Focus time is busy with its tag; the all-day day off is an all-day block
    // resolved in the response's zone.
    let focus = block_of(&ana, "ana-focus-0312");
    assert_eq!(focus.status, BlockStatus::Busy);
    assert_eq!(focus.tags, vec![Tag::new("status/focus").unwrap()]);
    let day_off = block_of(&ana, "ana-pto-0311");
    assert!(day_off.all_day);
    assert_eq!(day_off.span.start().to_string(), "2027-03-11T05:00Z");
    assert_eq!(day_off.span.end().to_string(), "2027-03-12T05:00Z");

    // Free/busy blocks carry no event identity.
    assert_eq!(room.len(), 2);
    assert!(room.iter().all(|block| block.event_key.is_none()));
    assert!(room.iter().all(|block| block.status == BlockStatus::Busy));
}

#[test]
fn recorded_calendars_become_the_offers_the_rules_describe() {
    let (ana, bo, room) = calendars();
    let engine = Engine::builder().build().unwrap();
    let request = booking_request(&ana, &bo, room);
    let validation = engine.validate(&request);
    assert!(validation.errors.is_empty(), "{:?}", validation.errors);
    let response = engine.find_availability(&request).unwrap();

    // Ana's working day bounds every offer.
    assert!(!response.offers.is_empty());
    for offer in &response.offers {
        let start = offer.start().unwrap();
        let end = offer.end().unwrap();
        assert!(
            start >= utc_at(2027, 3, 8, 14) && end <= utc_at(2027, 3, 12, 22),
            "offer {start}..{end} is outside Ana's working hours"
        );
    }

    // The shared interview counts once for the pooled cap even though both hosts
    // hold a copy, so 9 March has room for one more interview. Without the shared
    // key the day would already be full.
    assert!(
        offers(&response, utc_at(2027, 3, 9, 14)),
        "the pooled cap counts the shared interview once, leaving room on 9 March"
    );
    // The room's recorded free/busy range holds 16:00-17:00Z on 9 March.
    assert!(
        !offers(&response, utc_at(2027, 3, 9, 16)),
        "the room's free/busy range blocks the start inside it"
    );
    assert!(
        offers(&response, utc_at(2027, 3, 9, 17)),
        "the room is free again after its busy range"
    );

    // The tenth holds the shared interview plus Ana's own, so the cap is reached.
    assert!(
        !response.offers.iter().any(|offer| {
            offer.start().is_some_and(|start| {
                start >= utc_at(2027, 3, 10, 0) && start < utc_at(2027, 3, 11, 0)
            })
        }),
        "two interviews plus a booking pass the cap of two"
    );
    assert!(
        response.excluded.iter().any(|excluded| {
            excluded.start >= utc_at(2027, 3, 10, 0)
                && excluded.start < utc_at(2027, 3, 11, 0)
                && excluded
                    .reasons
                    .iter()
                    .any(|reason| reason.code == ReasonCode::CapReached)
        }),
        "the exclusions on 10 March name the cap"
    );

    // The all-day day off occupies 11 March, so no offer starts inside it.
    assert!(
        !response.offers.iter().any(|offer| {
            offer.start().is_some_and(|start| {
                start >= utc_at(2027, 3, 11, 0) && start < utc_at(2027, 3, 12, 5)
            })
        }),
        "the all-day block occupies 11 March, so no offer starts inside it"
    );

    // `status_policy` ignores Ana's focus time, so the noon gap it covers still
    // offers starts.
    assert!(
        offers(&response, utc_at(2027, 3, 12, 14)),
        "a `status/focus` block the policy ignores does not block"
    );

    // The offer the search produced is one `check_slot` confirms.
    let booked = response
        .offers
        .iter()
        .find(|offer| offer.start() == Some(utc_at(2027, 3, 9, 14)))
        .expect("the ninth is offered");
    let check = engine.check_slot(&request, &booked.to_proposal()).unwrap();
    assert!(check.valid, "{:?}", check.reasons);
}

#[test]
fn focus_time_blocks_when_the_policy_does_not_ignore_it() {
    let (ana, bo, room) = calendars();
    let engine = Engine::builder().build().unwrap();
    let mut request = booking_request(&ana, &bo, room);
    // Without the status policy the focus block blocks, which is what proves the
    // tag reached the block rather than the conversion having dropped it.
    request.rules.retain(|rule| rule.id != "focus_policy");
    let response = engine.find_availability(&request).unwrap();
    assert!(
        !offers(&response, utc_at(2027, 3, 12, 14)),
        "the focus block covers 14:00-16:00Z on 12 March"
    );
    assert!(offers(&response, utc_at(2027, 3, 12, 16)));
}
