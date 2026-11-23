//! End-to-end rest time-correctness fixtures: the hand-computed cases from the
//! plan's time section, run through the engine.
//!
//! Every fixture fixes the calendar and the booking by hand and states the exact
//! answer a reader can check: an all-day block that never counts as activity, a
//! meeting crossing midnight, two rest rules evaluated independently, travel
//! counting as activity, a conflict between two other events that excludes
//! nothing, and membership across daylight saving transitions and zone periods.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use opportune::Engine;
use opportune::model::{
    Block, Explain, Goal, Participant, Request, Requirement, Response, Rule, Span, Term, Travel,
    ZonePeriod,
};
use opportune::presets;
use opportune::reasons::ReasonCode;
use opportune::time::{
    Date, Duration, Instant, LocalResolution, TimeOfDay, TimeZoneDatabase, ZoneName,
};

fn database() -> &'static TimeZoneDatabase {
    static DATABASE: OnceLock<TimeZoneDatabase> = OnceLock::new();
    DATABASE.get_or_init(|| TimeZoneDatabase::bundled().expect("the bundled database is present"))
}

fn engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        Engine::builder()
            .build()
            .expect("the bundled database builds")
    })
}

fn name(zone: &str) -> ZoneName {
    ZoneName::new(zone).expect("the fixture zone is a zone name")
}

fn date(year: i16, month: i8, day: i8) -> Date {
    Date::new(year, month, day).expect("a fixture date is a real date")
}

fn time(hour: u8, minute: u8) -> TimeOfDay {
    TimeOfDay::new(hour, minute).expect("a fixture time is a time of day")
}

/// The instant a local date-time falls on, in `zone`. The fixture dates avoid
/// skipped and repeated local times; [`repeated`] covers the one that does not.
fn local(db: &TimeZoneDatabase, zone: &str, day: Date, hour: u8, minute: u8) -> Instant {
    match db
        .zone(zone)
        .unwrap_or_else(|err| panic!("`{zone}` is in the bundled database: {err}"))
        .resolve_local(day, time(hour, minute))
        .expect("the fixture instant is inside the supported range")
    {
        LocalResolution::Unique(instant) => instant,
        LocalResolution::Skipped | LocalResolution::Repeated(_, _) => {
            panic!(
                "{} {} in {zone} must exist exactly once",
                day,
                time(hour, minute)
            );
        }
    }
}

/// The later instant a repeated local date-time falls on, in `zone`.
fn repeated(db: &TimeZoneDatabase, zone: &str, day: Date, hour: u8, minute: u8) -> Instant {
    match db
        .zone(zone)
        .unwrap_or_else(|err| panic!("`{zone}` is in the bundled database: {err}"))
        .resolve_local(day, time(hour, minute))
        .expect("the fixture instant is inside the supported range")
    {
        LocalResolution::Repeated(_, later) => later,
        LocalResolution::Unique(_) | LocalResolution::Skipped => {
            panic!("{} {} in {zone} must occur twice", day, time(hour, minute));
        }
    }
}

/// The instant a UTC date-time falls on.
fn utc(day: Date, hour: u8, minute: u8) -> Instant {
    local(database(), "UTC", day, hour, minute)
}

fn span(start: Instant, end: Instant) -> Span {
    Span::new(start, end).expect("the fixture span is non-empty")
}

fn block(start: Instant, end: Instant) -> Block {
    Block::new(span(start, end))
}

/// A request booking one 30-minute event named `rich`, with an explanation so
/// every excluded start is reported.
fn request(
    zone: &str,
    now: Instant,
    search: Span,
    candidates: Option<Vec<Instant>>,
    participant: Participant,
    rules: Vec<Rule>,
) -> Request {
    let mut goal = Goal::single(
        name(zone),
        search,
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("rich")]),
    );
    goal.candidates = candidates;
    let mut request = Request::new(now, goal);
    request.participants.push(participant);
    request.rules = rules;
    request.explain = Some(Explain::new(10_000));
    request
}

fn host(zone: &str, blocks: Vec<Block>) -> Participant {
    let mut participant = Participant::new("rich", name(zone));
    participant.calendar = Some(blocks);
    participant
}

fn rest(late_after: (u8, u8), not_before: (u8, u8)) -> Rule {
    presets::rest(
        time(late_after.0, late_after.1),
        time(not_before.0, not_before.1),
    )
}

/// The starts the response offered.
fn offered(response: &Response) -> Vec<Instant> {
    response
        .offers
        .iter()
        .map(|offer| offer.start().expect("an offer has an event"))
        .collect()
}

/// The `(participant, code)` pairs one excluded start reported.
type ReportedReasons<'a> = Vec<(Option<&'a str>, ReasonCode)>;

/// The `(participant, code)` pairs each excluded start reported.
fn excluded(response: &Response) -> Vec<(Instant, ReportedReasons<'_>)> {
    response
        .excluded
        .iter()
        .map(|entry| {
            (
                entry.start,
                entry
                    .reasons
                    .iter()
                    .map(|reason| (reason.participant.as_deref(), reason.code))
                    .collect(),
            )
        })
        .collect()
}

#[test]
fn an_all_day_block_spanning_the_evening_and_morning_does_not_trigger_rest() {
    // Sunday 28 March 2027 is a 23-hour day in London: an all-day block from
    // local midnight to local midnight still ends before Monday begins.
    let sunday = date(2027, 3, 28);
    let monday = date(2027, 3, 29);
    let all_day = span(
        local(database(), "Europe/London", sunday, 0, 0),
        local(database(), "Europe/London", monday, 0, 0),
    );
    let monday_morning = local(database(), "Europe/London", monday, 9, 0);
    let search = span(
        local(database(), "Europe/London", monday, 8, 0),
        local(database(), "Europe/London", monday, 10, 0),
    );

    let mut marked_all_day = Block::new(all_day);
    marked_all_day.all_day = true;
    let protected = request(
        "Europe/London",
        local(database(), "Europe/London", sunday, 0, 0),
        search,
        Some(vec![monday_morning]),
        host("Europe/London", vec![marked_all_day]),
        vec![rest((18, 0), (10, 0))],
    );
    let response = engine().find_availability(&protected).unwrap();
    assert_eq!(
        offered(&response),
        [monday_morning],
        "an all-day block is never rest activity"
    );
    assert!(response.excluded.is_empty());

    // The same span marked as an ordinary block is activity that reaches the
    // evening, so the protected morning excludes the booking.
    let request = request(
        "Europe/London",
        local(database(), "Europe/London", sunday, 0, 0),
        search,
        Some(vec![monday_morning]),
        host("Europe/London", vec![Block::new(all_day)]),
        vec![rest((18, 0), (10, 0))],
    );
    let response = engine().find_availability(&request).unwrap();
    assert!(offered(&response).is_empty());
    assert_eq!(
        excluded(&response),
        [(monday_morning, vec![(Some("rich"), ReasonCode::Rest)],)]
    );
}

#[test]
fn a_meeting_crossing_midnight_triggers_rest_for_the_morning() {
    let monday = date(2027, 6, 14);
    let tuesday = date(2027, 6, 15);
    let meeting = block(utc(monday, 22, 0), utc(tuesday, 1, 0));
    let early = utc(tuesday, 9, 0);
    let later = utc(tuesday, 10, 0);
    let search = span(utc(tuesday, 8, 0), utc(tuesday, 11, 0));
    let rules = vec![rest((18, 0), (10, 0))];

    let protected = request(
        "UTC",
        utc(monday, 0, 0),
        search,
        Some(vec![early, later]),
        host("UTC", vec![meeting.clone()]),
        rules.clone(),
    );
    let response = engine().find_availability(&protected).unwrap();
    assert_eq!(
        offered(&response),
        [later],
        "the protected morning excludes the 09:00 booking, not the 10:00 one"
    );
    assert_eq!(
        excluded(&response),
        [(early, vec![(Some("rich"), ReasonCode::Rest)])]
    );

    // Without the rule the same morning is free: the exclusion is rest, not the
    // meeting itself, which ends at 01:00.
    let request = request(
        "UTC",
        utc(monday, 0, 0),
        search,
        Some(vec![early, later]),
        host("UTC", vec![meeting]),
        Vec::new(),
    );
    let response = engine().find_availability(&request).unwrap();
    assert_eq!(offered(&response), [early, later]);
}

#[test]
fn two_rest_rules_are_evaluated_independently() {
    // `evening` protects mornings before 09:00 after activity past 18:00;
    // `late-evening` protects them before 12:00 after activity past 22:00.
    let monday = date(2027, 6, 14);
    let tuesday = date(2027, 6, 15);
    let search = span(utc(tuesday, 7, 0), utc(tuesday, 13, 0));
    let eight = utc(tuesday, 8, 0);
    let eleven = utc(tuesday, 11, 0);
    let twelve = utc(tuesday, 12, 0);
    let rules = vec![
        rest((18, 0), (9, 0)).with_id("evening"),
        rest((22, 0), (12, 0)).with_id("late-evening"),
    ];

    // Only the 18:30 meeting: `evening` reaches it and `late-evening` does not.
    // A combined rule would falsely exclude 09:00–11:59 with `evening`'s late
    // hours and `late-evening`'s protected morning, so those starts prove the
    // rules never mix.
    let independent = request(
        "UTC",
        utc(monday, 0, 0),
        search,
        Some(vec![eight, eleven, twelve]),
        host("UTC", vec![block(utc(monday, 18, 30), utc(monday, 19, 0))]),
        rules.clone(),
    );
    let response = engine().find_availability(&independent).unwrap();
    assert_eq!(offered(&response), [eleven, twelve]);
    assert_eq!(
        excluded(&response),
        [(eight, vec![(Some("rich"), ReasonCode::Rest)])]
    );
    assert_eq!(
        response.excluded[0].reasons[0].rule.as_deref(),
        Some("evening")
    );

    // Adding the 22:30 meeting lets `late-evening` exclude on its own, citing
    // the earliest failing rule when both do.
    let request = request(
        "UTC",
        utc(monday, 0, 0),
        search,
        Some(vec![eight, eleven]),
        host(
            "UTC",
            vec![
                block(utc(monday, 18, 30), utc(monday, 19, 0)),
                block(utc(monday, 22, 30), utc(monday, 23, 0)),
            ],
        ),
        rules,
    );
    let response = engine().find_availability(&request).unwrap();
    assert!(offered(&response).is_empty());
    assert_eq!(
        excluded(&response),
        [
            (eight, vec![(Some("rich"), ReasonCode::Rest)]),
            (eleven, vec![(Some("rich"), ReasonCode::Rest)]),
        ]
    );
    assert_eq!(
        response.excluded[0].reasons[0].rule.as_deref(),
        Some("evening"),
        "both rules fail at 08:00, so the earliest in request order is cited"
    );
    assert_eq!(
        response.excluded[1].reasons[0].rule.as_deref(),
        Some("late-evening"),
        "only the later rule fails at 11:00"
    );
}

#[test]
fn travel_counts_as_activity() {
    let monday = date(2027, 6, 14);
    let tuesday = date(2027, 6, 15);
    let morning = utc(tuesday, 9, 0);
    let search = span(utc(tuesday, 8, 0), utc(tuesday, 10, 0));
    let rules = vec![rest((18, 0), (10, 0))];

    // The booking's own travel after it reaches 18:30, which the next morning's
    // meeting turns into a protected morning.
    let mut with_travel = request(
        "UTC",
        utc(monday, 0, 0),
        span(utc(tuesday, 17, 0), utc(tuesday, 18, 0)),
        Some(vec![utc(tuesday, 17, 0)]),
        host(
            "UTC",
            vec![block(
                utc(
                    tuesday.plus_days(1).expect("the fixture date is supported"),
                    8,
                    0,
                ),
                utc(
                    tuesday.plus_days(1).expect("the fixture date is supported"),
                    8,
                    30,
                ),
            )],
        ),
        rules.clone(),
    );
    with_travel.goal.parts[0].travel = Travel {
        before: Duration::ZERO,
        after: Duration::from_minutes(60),
        by_participant: BTreeMap::default(),
    };
    let response = engine().find_availability(&with_travel).unwrap();
    assert!(offered(&response).is_empty());
    assert_eq!(
        excluded(&response),
        [(utc(tuesday, 17, 0), vec![(Some("rich"), ReasonCode::Rest)],)]
    );

    // Without the travel the booking ends at 17:30, before the late hours, so
    // the next morning is untouched.
    let mut without = with_travel.clone();
    without.goal.parts[0].travel = Travel::default();
    let response = engine().find_availability(&without).unwrap();
    assert_eq!(offered(&response), [utc(tuesday, 17, 0)]);

    // An existing block's travel counts the same way: 17:00–17:30 plus an hour
    // of travel reaches 18:30, so the following morning is protected.
    let mut travelled_block = block(utc(monday, 17, 0), utc(monday, 17, 30));
    travelled_block.travel_after = Duration::from_minutes(60);
    let travelled = request(
        "UTC",
        utc(monday, 0, 0),
        search,
        Some(vec![morning]),
        host("UTC", vec![travelled_block.clone()]),
        rules.clone(),
    );
    let response = engine().find_availability(&travelled).unwrap();
    assert!(offered(&response).is_empty());
    assert_eq!(
        excluded(&response),
        [(morning, vec![(Some("rich"), ReasonCode::Rest)])]
    );
    travelled_block.travel_after = Duration::ZERO;
    let untravelled = request(
        "UTC",
        utc(monday, 0, 0),
        search,
        Some(vec![morning]),
        host("UTC", vec![travelled_block]),
        rules.clone(),
    );
    let response = engine().find_availability(&untravelled).unwrap();
    assert_eq!(offered(&response), [morning]);

    // Travel before the booking reaches into the protected morning, so a 10:00
    // booking with half an hour of travel starts it at 09:30.
    let mut travelling_booking = request(
        "UTC",
        utc(monday, 0, 0),
        span(utc(tuesday, 9, 0), utc(tuesday, 11, 0)),
        Some(vec![utc(tuesday, 10, 0)]),
        host("UTC", vec![block(utc(monday, 19, 0), utc(monday, 20, 0))]),
        rules,
    );
    travelling_booking.goal.parts[0].travel = Travel {
        before: Duration::from_minutes(30),
        after: Duration::ZERO,
        by_participant: BTreeMap::default(),
    };
    let response = engine().find_availability(&travelling_booking).unwrap();
    assert!(offered(&response).is_empty());
    assert_eq!(
        excluded(&response),
        [(utc(tuesday, 10, 0), vec![(Some("rich"), ReasonCode::Rest)],)]
    );
    travelling_booking.goal.parts[0].travel = Travel::default();
    let response = engine().find_availability(&travelling_booking).unwrap();
    assert_eq!(offered(&response), [utc(tuesday, 10, 0)]);
}

#[test]
fn a_conflict_between_two_other_events_never_excludes_an_unrelated_booking() {
    // The calendar already holds a late finish and an early start, which conflict
    // with each other. A booking that is part of neither overlap is untouched.
    let monday = date(2027, 6, 14);
    let tuesday = date(2027, 6, 15);
    let blocks = vec![
        block(utc(monday, 19, 0), utc(monday, 20, 0)),
        block(utc(tuesday, 8, 0), utc(tuesday, 8, 30)),
    ];
    let unrelated = utc(tuesday, 10, 30);
    let involved = utc(tuesday, 9, 0);
    let request = request(
        "UTC",
        utc(monday, 0, 0),
        span(utc(tuesday, 8, 0), utc(tuesday, 11, 0)),
        Some(vec![involved, unrelated]),
        host("UTC", blocks),
        vec![rest((18, 0), (10, 0))],
    );
    let response = engine().find_availability(&request).unwrap();
    assert_eq!(
        offered(&response),
        [unrelated],
        "the booking between the two protected hours is not part of either overlap"
    );
    assert_eq!(
        excluded(&response),
        [(involved, vec![(Some("rich"), ReasonCode::Rest)])]
    );
}

#[test]
fn rest_matches_exact_instant_membership_across_dst_transitions() {
    // Spring forward: Los Angeles skips 02:00–03:00 on 14 March 2027.
    let saturday = date(2027, 3, 13);
    let sunday = date(2027, 3, 14);
    let before_jump = local(database(), "America/Los_Angeles", sunday, 1, 30);
    let after_jump = local(database(), "America/Los_Angeles", sunday, 3, 30);
    let spring = request(
        "America/Los_Angeles",
        local(database(), "America/Los_Angeles", saturday, 0, 0),
        span(
            local(database(), "America/Los_Angeles", sunday, 1, 0),
            local(database(), "America/Los_Angeles", sunday, 5, 0),
        ),
        Some(vec![before_jump, after_jump]),
        host(
            "America/Los_Angeles",
            vec![block(
                local(database(), "America/Los_Angeles", saturday, 23, 0),
                local(database(), "America/Los_Angeles", saturday, 23, 30),
            )],
        ),
        vec![rest((18, 0), (3, 30))],
    );
    let response = engine().find_availability(&spring).unwrap();
    assert_eq!(
        offered(&response),
        [after_jump],
        "01:30 local is inside the protected hours; 03:30, after the skipped hour, is not"
    );
    assert_eq!(
        excluded(&response),
        [(before_jump, vec![(Some("rich"), ReasonCode::Rest)])]
    );

    // Fall back: Los Angeles repeats 01:00–02:00 on 7 November 2027. The later
    // 01:30 is still before the 02:00 boundary; 02:30 is not.
    let saturday = date(2027, 11, 6);
    let sunday = date(2027, 11, 7);
    let repeated_hour = repeated(database(), "America/Los_Angeles", sunday, 1, 30);
    let after_transition = local(database(), "America/Los_Angeles", sunday, 2, 30);
    let request = request(
        "America/Los_Angeles",
        local(database(), "America/Los_Angeles", saturday, 0, 0),
        span(
            local(database(), "America/Los_Angeles", sunday, 0, 0),
            local(database(), "America/Los_Angeles", sunday, 4, 0),
        ),
        Some(vec![repeated_hour, after_transition]),
        host(
            "America/Los_Angeles",
            vec![block(
                local(database(), "America/Los_Angeles", saturday, 22, 0),
                local(database(), "America/Los_Angeles", saturday, 22, 30),
            )],
        ),
        vec![rest((20, 0), (2, 0))],
    );
    let response = engine().find_availability(&request).unwrap();
    assert_eq!(offered(&response), [after_transition]);
    assert_eq!(
        excluded(&response),
        [(repeated_hour, vec![(Some("rich"), ReasonCode::Rest)])]
    );
}

#[test]
fn rest_matches_instant_membership_across_zone_periods() {
    // The host is in Tokyo for the late Monday meeting, then in London for the
    // Tuesday morning: the date the meeting falls on and the protected morning
    // are each read in the zone in force at that instant.
    let monday = date(2027, 6, 14);
    let tuesday = date(2027, 6, 15);
    let mut participant = host(
        "Asia/Tokyo",
        vec![block(utc(monday, 10, 0), utc(monday, 11, 0))],
    );
    participant.zone_periods.push(
        ZonePeriod::new(
            name("Europe/London"),
            utc(monday, 22, 0),
            utc(tuesday, 23, 0),
        )
        .expect("the fixture leg is non-empty"),
    );
    let early = utc(tuesday, 8, 0); // 09:00 in London
    let later = utc(tuesday, 9, 0); // 10:00 in London
    let request = request(
        "UTC",
        utc(monday, 0, 0),
        span(utc(tuesday, 7, 0), utc(tuesday, 11, 0)),
        Some(vec![early, later]),
        participant,
        vec![rest((18, 0), (10, 0))],
    );
    let response = engine().find_availability(&request).unwrap();
    assert_eq!(
        offered(&response),
        [later],
        "the Tokyo evening protects the London morning"
    );
    assert_eq!(
        excluded(&response),
        [(early, vec![(Some("rich"), ReasonCode::Rest)])]
    );
}

#[test]
fn a_booking_crossing_a_zone_period_boundary_is_read_in_both_zones() {
    // The host flies from Lord Howe (UTC+10:30) home to Anchorage (UTC-8) at
    // 06:07Z. A booking from 06:00Z runs 16:30–16:36 local on 15 June in Lord
    // Howe and 22:07–22:30 local on 14 June in Anchorage, so it reaches the late
    // part of both days. The parking block in the small hours of 15 June is the
    // protected morning that turns the Anchorage evening into a rest conflict.
    let monday = date(2027, 6, 14);
    let tuesday = date(2027, 6, 15);
    let mut participant = host(
        "America/Anchorage",
        vec![block(utc(tuesday, 8, 0), utc(tuesday, 8, 30))],
    );
    participant.zone_periods.push(
        ZonePeriod::new(
            name("Australia/Lord_Howe"),
            utc(monday, 20, 0),
            utc(tuesday, 6, 7),
        )
        .expect("the fixture leg is non-empty"),
    );
    let crossing = utc(tuesday, 6, 0);
    let later = utc(tuesday, 9, 0);
    let request = request(
        "UTC",
        utc(monday, 0, 0),
        span(utc(tuesday, 6, 0), utc(tuesday, 10, 0)),
        Some(vec![crossing, later]),
        participant,
        vec![rest((16, 0), (11, 0))],
    );
    let response = engine().find_availability(&request).unwrap();
    assert_eq!(
        offered(&response),
        [later],
        "the booking's Lord Howe evening and Anchorage evening both count"
    );
    assert_eq!(
        excluded(&response),
        [(crossing, vec![(Some("rich"), ReasonCode::Rest)])]
    );
}
