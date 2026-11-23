//! Recorded Graph payloads through the adapter into `Engine::find_availability`.
//!
//! The scenario is two hosts, one shared interview room, a pooled daily cap on
//! interviews and a rest rule. Ana's mailbox carries the shared interviews, one
//! interview of her own, and an all-day out-of-office day; Bo's carries the same
//! shared interviews. The assertions read the offers and exclusions the engine
//! produced from the converted blocks, so a conversion that drops, widens or
//! mistags a block shows up as wrong availability rather than as a green adapter
//! unit test.

// Every case resolves zone names against a real database, so this target runs
// where the crate compiles one in: `make test-matrix`'s `graph` build keeps
// `bundled-tzdb` on, and checks the bundle-free build separately.
#![cfg(all(feature = "graph", feature = "bundled-tzdb"))]

use std::collections::BTreeMap;

use opportune::model::{
    Cap, CapMeasure, CapPer, CapPeriod, Goal, Part, Participant, Request, Requirement, Rest, Span,
    Term,
};
use opportune::reasons::ReasonCode;
use opportune::tags::{Selector, Tag};
use opportune::time::{
    Date, Instant, LocalResolution, RuleZone, TimeOfDay, TimeZoneDatabase, Weekday, ZoneName,
};
use opportune::{Engine, model::Block};
use opportune_adapters::graph::{calendar_view_to_blocks, working_hours_to_rule};
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

/// The blocks one mailbox converted into.
fn blocks_of(payload: &str, participant_zone: &str) -> Vec<AnnotatedBlock> {
    let outcome = calendar_view_to_blocks(
        &db(),
        payload,
        &zone(participant_zone),
        Some(&BTreeMap::from([(
            "Interview".to_owned(),
            Tag::new("interview").unwrap(),
        )])),
    );
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

/// The request the two recorded mailboxes are booked against.
fn booking_request(ana: Vec<AnnotatedBlock>, bo: Vec<AnnotatedBlock>) -> Request {
    let working_hours = working_hours_to_rule(
        &db(),
        include_str!("fixtures/graph/working_hours.json"),
        "working_hours",
    )
    .expect("the mailbox's working hours convert")
    .with_who(Selector::ids(["ana"]));
    let rest = opportune::model::Rule::new(
        "rest",
        opportune::model::RuleKind::Rest(Rest::new(
            TimeOfDay::new(18, 0).unwrap(),
            TimeOfDay::new(10, 0).unwrap(),
        )),
    );
    let cap = opportune::model::Rule::new(
        "interviews_per_day",
        opportune::model::RuleKind::Cap(Cap::new(
            Selector::Tag(Tag::new("interview").unwrap()),
            CapMeasure::Count,
            2,
            CapPeriod::Day,
            CapPer::Pooled,
            RuleZone::Event,
            Weekday::Monday,
        )),
    );

    let event_zone = zone("UTC");
    let goal = Goal::new(
        event_zone,
        Span::new(utc_at(2027, 3, 8, 0), utc_at(2027, 3, 13, 0)).unwrap(),
        vec![
            Part::new(
                opportune::time::Duration::from_minutes(60),
                Requirement::new(vec![Term::person("ana"), Term::person("bo")]),
            )
            // The booking being made is an interview, which is what the cap counts.
            .with_tags(vec![Tag::new("interview").unwrap()]),
        ],
    );
    Request::new(utc_at(2027, 3, 8, 0), goal)
        .with_participants(vec![
            Participant::new("ana", zone("America/New_York"))
                .with_calendar(ana.into_iter().map(|annotated| annotated.block).collect()),
            Participant::new("bo", zone("Europe/London"))
                .with_calendar(bo.into_iter().map(|annotated| annotated.block).collect()),
        ])
        .with_rules(vec![working_hours, rest, cap])
        .with_explain(1_000)
}

/// The first block for `event_id`.
fn block_of<'a>(blocks: &'a [AnnotatedBlock], event_id: &str) -> &'a Block {
    &blocks
        .iter()
        .find(|annotated| annotated.event_id.as_deref() == Some(event_id))
        .unwrap_or_else(|| panic!("no block for `{event_id}`"))
        .block
}

/// Both recorded mailboxes, converted.
fn mailboxes() -> (Vec<AnnotatedBlock>, Vec<AnnotatedBlock>) {
    let ana = blocks_of(
        include_str!("fixtures/graph/calendar_view_ana.json"),
        "America/New_York",
    );
    let bo = blocks_of(
        include_str!("fixtures/graph/calendar_view_bo.json"),
        "Europe/London",
    );
    (ana, bo)
}

#[test]
fn the_recorded_mailboxes_convert_with_one_key_per_shared_event() {
    let (ana, bo) = mailboxes();
    // A shared event key across the two mailboxes, and an all-day block resolved
    // from its civil date in Ana's zone.
    let shared = block_of(&ana, "AAMkAGI2THAAA-ana-interview-0900");
    assert_eq!(
        shared.event_key.as_deref(),
        Some("040000008200E00074C5B7101A82E00800000000INTERVIEW0900")
    );
    assert_eq!(
        block_of(&bo, "AAMkAGI2THBBB-interview-0900").event_key,
        shared.event_key,
        "the same event in two mailboxes shares one key"
    );
    let all_day = block_of(&ana, "AAMkAGI2THAAA-ana-pto-0311");
    assert!(all_day.all_day);
    assert_eq!(all_day.span.start().to_string(), "2027-03-11T05:00Z");
    assert_eq!(all_day.span.end().to_string(), "2027-03-12T05:00Z");
}

#[test]
fn recorded_mailboxes_become_the_offers_the_rules_describe() {
    let (ana, bo) = mailboxes();
    let engine = Engine::builder().build().unwrap();
    let request = booking_request(ana, bo);
    let validation = engine.validate(&request);
    assert!(validation.errors.is_empty(), "{:?}", validation.errors);
    let response = engine.find_availability(&request).unwrap();

    // Ana's working hours (09:00-17:00 New York) bound every offer, and the grid
    // is the goal's default.
    assert!(!response.offers.is_empty());
    for offer in &response.offers {
        let start = offer.start().unwrap();
        let end = offer.end().unwrap();
        assert!(
            start >= utc_at(2027, 3, 8, 14) && end <= utc_at(2027, 3, 12, 22),
            "offer {start}..{end} is outside Ana's working hours"
        );
    }

    // The shared interview counts once for the pooled cap, so the ninth supports
    // one more interview: without the shared event key the day would be full.
    assert!(
        response
            .offers
            .iter()
            .any(|offer| offer.start() == Some(utc_at(2027, 3, 9, 14))),
        "the pooled cap counts the shared interview once, leaving room on 9 March"
    );

    // The tenth holds the shared interview plus Ana's own, so the cap is reached.
    assert!(
        !response
            .offers
            .iter()
            .any(|offer| offer.start().is_some_and(
                |start| start < utc_at(2027, 3, 10, 22) && start >= utc_at(2027, 3, 10, 0)
            )),
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

    // The all-day out-of-office day blocks availability.
    assert!(
        !response.offers.iter().any(|offer| {
            offer.start().is_some_and(|start| {
                start >= utc_at(2027, 3, 11, 0) && start < utc_at(2027, 3, 12, 5)
            })
        }),
        "the all-day block occupies 11 March, so no offer starts inside it"
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
fn rest_ignores_an_all_day_block_but_not_a_timed_one() {
    let (ana, bo) = mailboxes();
    let engine = Engine::builder().build().unwrap();
    // Ana's all-day 11 March would otherwise make every start before 10:00 on 12
    // March too early after a late finish.
    let offered = |ana: Vec<AnnotatedBlock>| {
        let response = engine
            .find_availability(&booking_request(ana, bo.clone()))
            .unwrap();
        response
            .offers
            .iter()
            .any(|offer| offer.start() == Some(utc_at(2027, 3, 12, 14)))
    };
    assert!(
        offered(ana.clone()),
        "an all-day block is not rest activity, so 09:00 on 12 March is offered"
    );

    // The same blocks reported as timed make the rule exclude that start, which
    // is what the all-day flag is protecting.
    let mut flipped = ana;
    for annotated in &mut flipped {
        if annotated.event_id.as_deref() == Some("AAMkAGI2THAAA-ana-pto-0311") {
            annotated.block = annotated.block.clone().with_all_day(false);
        }
    }
    assert!(
        !offered(flipped),
        "a timed late event is rest activity, so the start it protects is not offered"
    );
}
