//! Adversarial probes for the `rest` rule kind: the half-open boundaries the
//! specification defines, occupancy that only becomes late through travel, and
//! the degenerate parameter values a caller can still pass.
//!
//! Each test states the exact answer a reader can check against the
//! specification's `rest` definition, so a wrong verdict is a real defect rather
//! than a preference.

#![cfg(feature = "bundled-tzdb")]

use std::collections::BTreeMap;

use opportune::Engine;
use opportune::model::{
    Block, BlockStatus, Explain, Goal, Participant, ParticipantTravel, Proposal, ProposalEvent,
    RangesQuery, Request, Requirement, Response, Rule, Span, Term, Travel, ZonePeriod,
};
use opportune::presets;
use opportune::reasons::ReasonCode;
use opportune::time::{Date, Duration, Instant, LocalResolution, TimeOfDay, Zone, ZoneName};

fn engine() -> Engine {
    Engine::builder()
        .build()
        .expect("the bundled database builds")
}

fn date(day: (i16, i8, i8)) -> Date {
    Date::new(day.0, day.1, day.2).expect("a fixture date is a real date")
}

fn time(hour: u8, minute: u8) -> TimeOfDay {
    TimeOfDay::new(hour, minute).expect("a fixture time is a time of day")
}

fn utc(day: (i16, i8, i8), hour: u8, minute: u8) -> Instant {
    let LocalResolution::Unique(instant) = Zone::utc()
        .resolve_local(date(day), time(hour, minute))
        .expect("the fixture instant is inside the supported range")
    else {
        panic!("UTC has no transitions");
    };
    instant
}

fn span(start: Instant, end: Instant) -> Span {
    Span::new(start, end).expect("the fixture span is non-empty")
}

fn block(start: Instant, end: Instant) -> Block {
    Block::new(span(start, end))
}

fn host(id: &str, blocks: Vec<Block>) -> Participant {
    let mut participant = Participant::new(id, ZoneName::new("UTC").expect("a zone name"));
    participant.calendar = Some(blocks);
    participant
}

fn request(
    now: Instant,
    search: Span,
    candidates: Vec<Instant>,
    duration: u32,
    travel: Travel,
    participants: Vec<Participant>,
    rules: Vec<Rule>,
) -> Request {
    let mut goal = Goal::single(
        ZoneName::new("UTC").expect("a zone name"),
        search,
        Duration::from_minutes(duration),
        Requirement::new(vec![Term::person("rich")]),
    );
    goal.candidates = Some(candidates);
    goal.parts[0].travel = travel;
    let mut request = Request::new(now, goal);
    request.participants = participants;
    request.rules = rules;
    request.explain = Some(Explain::new(10_000));
    request
}

fn offered(response: &Response) -> Vec<Instant> {
    response
        .offers
        .iter()
        .map(|offer| offer.start().expect("an offer has an event"))
        .collect()
}

fn first_codes(response: &Response) -> Vec<(Instant, ReasonCode)> {
    response
        .excluded
        .iter()
        .map(|entry| (entry.start, entry.reasons[0].code))
        .collect()
}

fn evening_rest() -> Rule {
    presets::rest(time(18, 0), time(10, 0))
}

const MONDAY: (i16, i8, i8) = (2027, 6, 14);
const TUESDAY: (i16, i8, i8) = (2027, 6, 15);

#[test]
fn late_after_is_inclusive_and_the_booking_side_is_half_open() {
    // `late(D)` is every minute of D "at or after" `late_after`, so an event that
    // ends exactly at 18:00 has no late minute while one starting at 18:00 does.
    let morning = utc(TUESDAY, 9, 0);
    let search = span(utc(TUESDAY, 8, 0), utc(TUESDAY, 10, 0));
    let engine = engine();

    let ending_at = request(
        utc(MONDAY, 0, 0),
        search,
        vec![morning],
        30,
        Travel::default(),
        vec![host(
            "rich",
            vec![block(utc(MONDAY, 17, 30), utc(MONDAY, 18, 0))],
        )],
        vec![evening_rest()],
    );
    let response = engine.find_availability(&ending_at).unwrap();
    assert_eq!(
        offered(&response),
        [morning],
        "an event ending exactly at 18:00 has no minute at or after 18:00"
    );

    let starting_at = request(
        utc(MONDAY, 0, 0),
        search,
        vec![morning],
        30,
        Travel::default(),
        vec![host(
            "rich",
            vec![block(utc(MONDAY, 18, 0), utc(MONDAY, 18, 1))],
        )],
        vec![evening_rest()],
    );
    let response = engine.find_availability(&starting_at).unwrap();
    assert_eq!(
        first_codes(&response),
        [(morning, ReasonCode::Rest)],
        "a single minute at 18:00 is a late finish"
    );
}

#[test]
fn a_booking_ending_exactly_at_late_after_is_not_late() {
    // The booking's own occupied span decides the late side too: 17:30-18:00 ends
    // on the boundary, so the protected morning after an existing early start
    // does not exclude it.
    let activity = block(utc(TUESDAY, 8, 0), utc(TUESDAY, 8, 30));
    let search = span(utc(MONDAY, 16, 0), utc(MONDAY, 19, 0));
    let engine = engine();

    let ending_at = request(
        utc(MONDAY, 0, 0),
        search,
        vec![utc(MONDAY, 17, 30)],
        30,
        Travel::default(),
        vec![host("rich", vec![activity.clone()])],
        vec![evening_rest()],
    );
    let response = engine.find_availability(&ending_at).unwrap();
    assert_eq!(
        offered(&response),
        [utc(MONDAY, 17, 30)],
        "a booking ending exactly at 18:00 is not part of a late overlap"
    );

    let starting_at = request(
        utc(MONDAY, 0, 0),
        search,
        vec![utc(MONDAY, 17, 31)],
        30,
        Travel::default(),
        vec![host("rich", vec![activity])],
        vec![evening_rest()],
    );
    let response = engine.find_availability(&starting_at).unwrap();
    assert_eq!(
        first_codes(&response),
        [(utc(MONDAY, 17, 31), ReasonCode::Rest)],
        "a booking reaching 18:01 is part of a late overlap"
    );
}

#[test]
fn travel_before_the_booking_can_exclude_it_across_midnight() {
    // Only the booking's occupied span (which includes travel) counts. Half an
    // hour of travel before a 00:10 booking reaches 23:40 the evening before, so
    // the booking itself spans `late(D)` into `early(D + 1)` and excludes itself.
    let start = utc(TUESDAY, 0, 10);
    let search = span(utc(TUESDAY, 0, 0), utc(TUESDAY, 1, 0));
    let engine = engine();

    let travelling = request(
        utc(MONDAY, 0, 0),
        search,
        vec![start],
        30,
        Travel {
            before: Duration::from_minutes(30),
            after: Duration::ZERO,
            by_participant: BTreeMap::new(),
        },
        vec![host("rich", Vec::new())],
        vec![evening_rest()],
    );
    let response = engine.find_availability(&travelling).unwrap();
    assert_eq!(
        first_codes(&response),
        [(start, ReasonCode::Rest)],
        "travel before the booking reaches the late evening"
    );

    let without = request(
        utc(MONDAY, 0, 0),
        search,
        vec![start],
        30,
        Travel::default(),
        vec![host("rich", Vec::new())],
        vec![evening_rest()],
    );
    let response = engine.find_availability(&without).unwrap();
    assert_eq!(
        offered(&response),
        [start],
        "without travel the booking is entirely in the protected morning's own day"
    );
}

#[test]
fn a_per_participant_travel_override_counts_as_rest_activity() {
    // The per-participant override replaces the part's travel for that
    // participant, and the resulting occupied span is what rest reads.
    let evening = utc(MONDAY, 17, 0);
    let search = span(utc(MONDAY, 16, 0), utc(MONDAY, 19, 0));
    let mut overrides = BTreeMap::new();
    overrides.insert(
        "rich".to_owned(),
        ParticipantTravel {
            before: Duration::ZERO,
            after: Duration::from_minutes(60),
        },
    );
    let engine = engine();

    let overridden = request(
        utc(MONDAY, 0, 0),
        search,
        vec![evening],
        30,
        Travel {
            before: Duration::ZERO,
            after: Duration::ZERO,
            by_participant: overrides,
        },
        vec![host(
            "rich",
            vec![block(utc(TUESDAY, 8, 0), utc(TUESDAY, 8, 30))],
        )],
        vec![evening_rest()],
    );
    let response = engine.find_availability(&overridden).unwrap();
    assert_eq!(
        first_codes(&response),
        [(evening, ReasonCode::Rest)],
        "the override's hour of travel reaches 18:30"
    );

    let mut plain = overridden.clone();
    plain.goal.parts[0].travel = Travel::default();
    let response = engine.find_availability(&plain).unwrap();
    assert_eq!(
        offered(&response),
        [evening],
        "without the override the booking ends at 17:30"
    );
}

#[test]
fn a_rule_whose_late_side_covers_the_whole_day_pairs_with_the_next_morning() {
    // `late_after: 00:00` makes every minute of a date late, which the engine must
    // apply literally rather than treating the parameter as an evening hour.
    let morning = utc(TUESDAY, 9, 0);
    let search = span(utc(TUESDAY, 8, 0), utc(TUESDAY, 10, 0));
    let rule = presets::rest(time(0, 0), time(10, 0));
    let engine = engine();

    let with_activity = request(
        utc(MONDAY, 0, 0),
        search,
        vec![morning],
        30,
        Travel::default(),
        vec![host(
            "rich",
            vec![block(utc(MONDAY, 0, 10), utc(MONDAY, 0, 20))],
        )],
        vec![rule.clone()],
    );
    let response = engine.find_availability(&with_activity).unwrap();
    assert_eq!(
        first_codes(&response),
        [(morning, ReasonCode::Rest)],
        "00:10 on the previous date is at or after `late_after`"
    );

    // The same booking alone is not excluded: it is not an early start after its
    // own late minutes on the *following* date.
    let alone = request(
        utc(MONDAY, 0, 0),
        search,
        vec![morning],
        30,
        Travel::default(),
        vec![host("rich", Vec::new())],
        vec![rule],
    );
    let response = engine.find_availability(&alone).unwrap();
    assert_eq!(offered(&response), [morning]);
}

#[test]
fn a_midnight_not_before_protects_no_morning() {
    // `early(D + 1)` is empty when `not_before` is midnight, so the rule can never
    // pair a late finish with a protected morning.
    let morning = utc(TUESDAY, 9, 0);
    let search = span(utc(TUESDAY, 8, 0), utc(TUESDAY, 10, 0));
    let request = request(
        utc(MONDAY, 0, 0),
        search,
        vec![morning],
        30,
        Travel::default(),
        vec![host(
            "rich",
            vec![block(utc(MONDAY, 19, 0), utc(MONDAY, 20, 0))],
        )],
        vec![presets::rest(time(18, 0), time(0, 0))],
    );
    let response = engine().find_availability(&request).unwrap();
    assert_eq!(
        offered(&response),
        [morning],
        "a rule protecting nothing before midnight excludes nothing"
    );
}

#[test]
fn a_free_block_is_never_rest_activity() {
    // `free` blocks are ignored by default, so they are not blocking activity and
    // cannot protect a morning. Only a status policy that turns `free` into
    // `block` may make one count.
    let mut free = block(utc(MONDAY, 19, 0), utc(MONDAY, 20, 0));
    free.status = BlockStatus::Free;
    let morning = utc(TUESDAY, 9, 0);
    let request = request(
        utc(MONDAY, 0, 0),
        span(utc(TUESDAY, 8, 0), utc(TUESDAY, 10, 0)),
        vec![morning],
        30,
        Travel::default(),
        vec![host("rich", vec![free])],
        vec![evening_rest()],
    );
    let response = engine().find_availability(&request).unwrap();
    assert_eq!(
        offered(&response),
        [morning],
        "a free block is not blocking activity"
    );
}

#[test]
fn an_all_day_blocks_travel_is_not_activity() {
    // The all-day exclusion drops the block's whole occupied span, travel
    // included: its two hours of travel would otherwise reach 21:00 and protect
    // the morning.
    let mut all_day = block(utc(MONDAY, 19, 0), utc(MONDAY, 19, 30));
    all_day.all_day = true;
    all_day.travel_after = Duration::from_minutes(120);
    let morning = utc(TUESDAY, 9, 0);
    let search = span(utc(TUESDAY, 8, 0), utc(TUESDAY, 10, 0));
    let engine = engine();

    let ignored = request(
        utc(MONDAY, 0, 0),
        search,
        vec![morning],
        30,
        Travel::default(),
        vec![host("rich", vec![all_day])],
        vec![evening_rest()],
    );
    let response = engine.find_availability(&ignored).unwrap();
    assert_eq!(
        offered(&response),
        [morning],
        "an all-day block's travel is not activity either"
    );
}

#[test]
fn an_early_block_after_the_window_protects_an_evening_booking() {
    // The mirror of the evening-finish case: a Monday-evening booking is excluded
    // by Tuesday's early meeting even though that meeting lies after the search
    // window and no candidate start is near it. `required_ranges` must reach it
    // so a caller fetching only that range still gets the answer.
    let evening = utc(MONDAY, 19, 0);
    let early_meeting = block(utc(TUESDAY, 8, 0), utc(TUESDAY, 8, 30));
    let request = request(
        utc(MONDAY, 0, 0),
        span(utc(MONDAY, 18, 0), utc(MONDAY, 20, 0)),
        vec![evening],
        30,
        Travel::default(),
        vec![host("rich", vec![early_meeting.clone()])],
        vec![evening_rest()],
    );
    let engine = engine();

    let mut query = RangesQuery::new(request.now, request.goal.clone());
    query.rules.clone_from(&request.rules);
    query.participants.clone_from(&request.participants);
    let ranges = engine.required_ranges(&query).unwrap();
    assert!(
        ranges.calendar.end() >= early_meeting.span.end(),
        "the calendar range {}..{} misses the morning meeting after the window",
        ranges.calendar.start(),
        ranges.calendar.end()
    );

    let response = engine.find_availability(&request).unwrap();
    assert_eq!(
        first_codes(&response),
        [(evening, ReasonCode::Rest)],
        "the booking's own late minutes pair with the next morning's early activity"
    );
}

#[test]
fn one_minute_late_and_early_boundaries_are_half_open() {
    // `late(D)` is inclusive at `late_after` and `early(D + 1)` is exclusive at
    // `not_before`, so a one-minute rule still catches exactly one minute on each
    // side: 23:59 is late, 00:00 is early, and 00:01 is not.
    let exact = utc(TUESDAY, 0, 0);
    let after = utc(TUESDAY, 0, 1);
    let search = span(utc(TUESDAY, 0, 0), utc(TUESDAY, 1, 0));
    let rule = presets::rest(time(23, 59), time(0, 1));
    let activity = block(utc(MONDAY, 23, 59), utc(TUESDAY, 0, 0));
    let engine = engine();

    let caught = request(
        utc(MONDAY, 0, 0),
        search,
        vec![exact, after],
        30,
        Travel::default(),
        vec![host("rich", vec![activity.clone()])],
        vec![rule.clone()],
    );
    let response = engine.find_availability(&caught).unwrap();
    assert_eq!(
        offered(&response),
        [after],
        "only the booking whose first minute is before 00:01 is early"
    );
    assert_eq!(first_codes(&response), [(exact, ReasonCode::Rest)]);

    // Without the late minute there is nothing to pair with: the same 00:00
    // booking is offered once the activity moves one minute back.
    let trimmed = request(
        utc(MONDAY, 0, 0),
        search,
        vec![exact],
        30,
        Travel::default(),
        vec![host(
            "rich",
            vec![block(utc(MONDAY, 23, 58), utc(MONDAY, 23, 59))],
        )],
        vec![rule],
    );
    let response = engine.find_availability(&trimmed).unwrap();
    assert_eq!(offered(&response), [exact]);
}

#[test]
fn check_slot_rejects_an_attendee_rest_conflict() {
    // The attendee's protected morning must fail confirmation too, not only the
    // search. `find_availability` reports it through `attendee_unavailable`; the
    // confirmation names the attendee's own `rest` reason.
    let monday = (2027, 6, 14);
    let tuesday = (2027, 6, 15);
    let mut attendee = host("tom", vec![block(utc(monday, 19, 0), utc(monday, 20, 0))]);
    attendee.role = opportune::tags::Role::Attendee;
    let scoped = evening_rest().with_who(opportune::tags::Selector::Role(
        opportune::tags::Role::Attendee,
    ));
    let start = utc(tuesday, 9, 0);
    let request = request(
        utc(monday, 0, 0),
        span(utc(tuesday, 8, 0), utc(tuesday, 10, 0)),
        vec![start],
        30,
        Travel::default(),
        vec![host("rich", Vec::new()), attendee],
        vec![scoped],
    );
    let engine = engine();

    assert!(offered(&engine.find_availability(&request).unwrap()).is_empty());
    let check = engine
        .check_slot(
            &request,
            &Proposal {
                events: vec![ProposalEvent {
                    part: "main".to_owned(),
                    start,
                    assignment: vec![vec!["rich".to_owned()]],
                }],
            },
        )
        .unwrap();
    assert!(
        !check.valid,
        "the attendee's rest conflict must fail confirmation"
    );
    let reasons: Vec<(Option<&str>, ReasonCode)> = check
        .reasons
        .iter()
        .map(|reason| (reason.participant.as_deref(), reason.code))
        .collect();
    assert_eq!(reasons, [(Some("tom"), ReasonCode::Rest)]);
}

#[test]
fn check_slot_reads_rest_outside_the_search_window() {
    // `check_slot` ignores the search window, so a proposed Monday-evening
    // booking must be judged against Tuesday's early meeting even though no
    // search over the window would ever consider Monday.
    let evening = utc(MONDAY, 19, 0);
    let request = request(
        utc(MONDAY, 0, 0),
        span(utc(TUESDAY, 8, 0), utc(TUESDAY, 10, 0)),
        vec![utc(TUESDAY, 9, 0)],
        30,
        Travel::default(),
        vec![host(
            "rich",
            vec![block(utc(TUESDAY, 8, 0), utc(TUESDAY, 8, 30))],
        )],
        vec![evening_rest()],
    );
    let engine = engine();

    // The search agrees the window's own morning is free.
    assert_eq!(
        offered(&engine.find_availability(&request).unwrap()),
        [utc(TUESDAY, 9, 0)]
    );

    let check = engine
        .check_slot(
            &request,
            &Proposal {
                events: vec![ProposalEvent {
                    part: "main".to_owned(),
                    start: evening,
                    assignment: vec![vec!["rich".to_owned()]],
                }],
            },
        )
        .unwrap();
    assert!(
        !check.valid,
        "the Monday-evening start crosses into the early meeting"
    );
    assert_eq!(
        check.reasons[0].code,
        ReasonCode::Rest,
        "the booking is late on Monday and the meeting is early on Tuesday"
    );
}

#[test]
fn activity_crossing_a_zone_period_boundary_is_read_in_both_zones() {
    // The host leaves Kiritimati (UTC+14) for Pago Pago (UTC-11) at 06:00Z. A
    // block from 05:00Z to 07:00Z is 19:00–21:00 on Monday in Kiritimati and
    // 18:00–20:00 on *Sunday* in Pago Pago, so the walk must record the local date
    // that goes backwards when the zone changes. Only Sunday's late minutes pair
    // with the Monday-morning booking in Pago Pago; losing them would offer it.
    let monday = (2027, 6, 14);
    let tuesday = (2027, 6, 15);
    let mut participant = host("rich", vec![block(utc(monday, 5, 0), utc(monday, 7, 0))]);
    participant.zone = ZoneName::new("Pacific/Kiritimati").expect("a zone name");
    participant.zone_periods.push(
        ZonePeriod::new(
            ZoneName::new("Etc/GMT+11").expect("a zone name"),
            utc(monday, 6, 0),
            utc(tuesday, 18, 0),
        )
        .expect("the fixture leg is non-empty"),
    );
    let booking = utc(monday, 15, 0); // 04:00 on Monday in Pago Pago
    let request = request(
        utc(monday, 0, 0),
        span(utc(monday, 14, 0), utc(monday, 16, 0)),
        vec![booking],
        30,
        Travel::default(),
        vec![participant],
        vec![evening_rest()],
    );
    let response = engine().find_availability(&request).unwrap();
    assert_eq!(
        first_codes(&response),
        [(booking, ReasonCode::Rest)],
        "the Pago Pago evening the block reaches after the zone change protects the morning"
    );
}

#[test]
fn rest_at_the_supported_range_edge_is_judged_with_a_clamped_window() {
    // `Instant::MAX` is 2100-01-01T00:00Z, so a search window may legally end
    // there and rest's own window is clamped to the supported range. Activity on
    // the last full local evening must still protect the last full local morning.
    let engine = engine();
    let evening = block(utc((2099, 12, 30), 18, 0), utc((2099, 12, 30), 19, 0));
    let morning = utc((2099, 12, 31), 8, 0);
    let later = utc((2099, 12, 31), 11, 0);
    let req = request(
        utc((2099, 12, 29), 0, 0),
        span(utc((2099, 12, 31), 0, 0), Instant::MAX),
        vec![morning, later],
        30,
        Travel::default(),
        vec![host("rich", vec![evening.clone()])],
        vec![evening_rest()],
    );
    let response = engine.find_availability(&req).unwrap();
    assert_eq!(
        offered(&response),
        vec![later],
        "the last full morning is protected by the evening before it"
    );
    assert_eq!(
        first_codes(&response),
        vec![(morning, ReasonCode::Rest)],
        "and reports the rest code"
    );

    // The largest representable start with a one-minute booking: no local date
    // follows it, so no pair can form. The walk must neither panic nor invent a
    // pairing date past the end of the supported range.
    let last = Instant::from_minutes(Instant::MAX.minutes() - 1).expect("in range");
    let req = request(
        utc((2099, 12, 29), 0, 0),
        span(utc((2099, 12, 30), 0, 0), Instant::MAX),
        vec![last],
        1,
        Travel::default(),
        vec![host("rich", vec![evening])],
        vec![evening_rest()],
    );
    let response = engine.find_availability(&req).unwrap();
    assert_eq!(offered(&response), vec![last]);
    assert!(
        response.excluded.is_empty(),
        "23:59 has no following morning to pair with: {:?}",
        first_codes(&response)
    );
}

#[test]
fn a_booking_self_conflicts_through_a_clock_that_crosses_the_date_line() {
    // The host is in UTC, with sixty short legs across the date line either side
    // of the booking so the rule's late and early minutes are cut into many
    // segments, and two legs inside the booking below:
    //
    // * Monday 06:00Z-06:05Z in Etc/GMT+12 (UTC-12) reads as Sunday 18:00-18:05,
    //   which is `late(Sunday)`.
    // * Monday 06:05Z-06:10Z in Etc/GMT+3 (UTC-3) reads as Monday 03:05-03:10,
    //   which is `early(Monday)`.
    //
    // A ten-minute booking from Monday 06:00Z therefore reaches both halves of
    // one `(D, D + 1)` pair and excludes itself, although its two sides are
    // minutes of a Sunday evening and a Monday morning that only the legs put
    // next to each other. A booking entirely in the home clock's own Monday
    // morning reaches no late minute and is offered.
    let mut participant = host("rich", Vec::new());
    // Sunday's legs fragment the local readings the booking's Sunday evening
    // comes from; Monday's sit after it, where UTC-12 still reads a Sunday
    // evening and UTC+14 reads a Monday one, and the home minutes between them
    // read a Monday morning.
    let groups = [
        (utc(MONDAY, 0, 0).minutes() - 24 * 60, 30_i64, 10),
        (utc(MONDAY, 7, 0).minutes(), 30_i64, 6),
    ];
    for (base, legs, pitch) in groups {
        for leg in 0..legs {
            let from = base + leg * pitch;
            let zone = if leg % 2 == 0 {
                "Etc/GMT+12"
            } else {
                "Pacific/Kiritimati"
            };
            participant.zone_periods.push(
                ZonePeriod::new(
                    ZoneName::new(zone).expect("a zone name"),
                    Instant::from_minutes(from).expect("a leg start is in range"),
                    Instant::from_minutes(from + 3).expect("a leg end is in range"),
                )
                .expect("the fixture leg is non-empty"),
            );
        }
    }
    for (zone, from, until) in [
        ("Etc/GMT+12", utc(MONDAY, 6, 0), utc(MONDAY, 6, 5)),
        ("Etc/GMT+3", utc(MONDAY, 6, 5), utc(MONDAY, 6, 10)),
    ] {
        participant.zone_periods.push(
            ZonePeriod::new(ZoneName::new(zone).expect("a zone name"), from, until)
                .expect("the fixture leg is non-empty"),
        );
    }

    let crossing = utc(MONDAY, 6, 0);
    let at_home = utc(MONDAY, 5, 0);
    let request = request(
        utc(MONDAY, 0, 0),
        span(utc(MONDAY, 4, 0), utc(MONDAY, 7, 0)),
        vec![crossing, at_home],
        10,
        Travel::default(),
        vec![participant],
        vec![evening_rest()],
    );
    assert!(
        engine().validate(&request).errors.is_empty(),
        "the fixture's legs are disjoint: {:?}",
        engine().validate(&request).errors
    );
    let response = engine().find_availability(&request).unwrap();
    assert_eq!(
        offered(&response),
        vec![at_home],
        "the booking crossing the date line excludes itself; the one inside the \
         home morning does not"
    );
    assert_eq!(
        first_codes(&response),
        vec![(crossing, ReasonCode::Rest)],
        "and it reports the rest code"
    );

    // A confirmation judges the same booking through the same construction, so
    // it must reject the slot the search excluded.
    let proposal = Proposal {
        events: vec![ProposalEvent {
            part: "main".to_owned(),
            start: crossing,
            assignment: vec![vec!["rich".to_owned()]],
        }],
    };
    let check = engine().check_slot(&request, &proposal).unwrap();
    assert!(
        !check.valid,
        "the crossing booking is not confirmable: {:?}",
        check.reasons
    );
    assert_eq!(check.reasons[0].code, ReasonCode::Rest);
}
