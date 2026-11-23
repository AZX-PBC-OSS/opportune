//! End-to-end tests for the `rest` rule kind: what counts as activity, how the
//! booking excludes itself, where the reason sits in the precedence order, and
//! that `find_availability`, `check_slot`, `validate` and `required_ranges`
//! agree.
//!
//! Every test builds a real request and runs it on a real engine, so deleting
//! the behaviour it covers turns it red.

#![cfg(feature = "bundled-tzdb")]

use opportune::Engine;
use opportune::model::{
    Block, BlockStatus, Explain, Goal, Participant, Proposal, ProposalEvent, RangesQuery, Request,
    Requirement, Rule, RuleKind, Span, StatusEffect, StatusPolicy, StatusPolicyEntry, Term,
};
use opportune::presets;
use opportune::reasons::ReasonCode;
use opportune::time::{
    BlackoutSpan, BlackoutSpec, Date, Duration, Instant, LocalDateTime, LocalResolution, RuleZone,
    TimeOfDay, Zone, ZoneName,
};

/// Monday 14 June 2027, the fixture's first day.
const MONDAY: (i16, i8, i8) = (2027, 6, 14);
/// Tuesday 15 June 2027, the fixture's second day.
const TUESDAY: (i16, i8, i8) = (2027, 6, 15);

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

/// The UTC instant of a local date-time.
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

/// The evening rest rule every fixture uses: no booking before 10:00 after
/// activity at or after 18:00 the day before.
fn evening_rest() -> Rule {
    presets::rest(time(18, 0), time(10, 0))
}

fn host(id: &str, blocks: Vec<Block>) -> Participant {
    let mut participant = Participant::new(id, ZoneName::new("UTC").expect("a zone name"));
    participant.calendar = Some(blocks);
    participant
}

/// A one-host, 30-minute request with the given search window, candidates and
/// rules, and an explanation so every excluded start is reported.
fn request(
    now: Instant,
    search: Span,
    candidates: Vec<Instant>,
    participants: Vec<Participant>,
    rules: Vec<Rule>,
) -> Request {
    let mut goal = Goal::single(
        ZoneName::new("UTC").expect("a zone name"),
        search,
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("rich")]),
    );
    goal.candidates = Some(candidates);
    let mut request = Request::new(now, goal);
    request.participants = participants;
    request.rules = rules;
    request.explain = Some(Explain::new(10_000));
    request
}

fn proposal(start: Instant) -> Proposal {
    Proposal {
        events: vec![ProposalEvent {
            part: "main".to_owned(),
            start,
            assignment: vec![vec!["rich".to_owned()]],
        }],
    }
}

fn offered(response: &opportune::model::Response) -> Vec<Instant> {
    response
        .offers
        .iter()
        .map(|offer| offer.start().expect("an offer has an event"))
        .collect()
}

#[test]
fn the_rest_preset_builds_the_rule_it_names() {
    let rule = evening_rest();
    assert_eq!(rule.id, "rest");
    let rest = rule.rest().expect("the preset writes a rest rule");
    assert_eq!(rest.late_after.to_string(), "18:00");
    assert_eq!(rest.not_before.to_string(), "10:00");

    // The engine accepts the preset's rule unchanged and applies it.
    let morning = utc(TUESDAY, 9, 0);
    let request = request(
        utc(MONDAY, 0, 0),
        span(utc(TUESDAY, 8, 0), utc(TUESDAY, 10, 0)),
        vec![morning],
        vec![host(
            "rich",
            vec![block(utc(MONDAY, 19, 0), utc(MONDAY, 20, 0))],
        )],
        vec![rule],
    );
    assert!(engine().validate(&request).is_valid());
    let response = engine().find_availability(&request).unwrap();
    assert!(offered(&response).is_empty());
    let reason = &response.excluded[0].reasons[0];
    assert_eq!(reason.code, ReasonCode::Rest);
    assert_eq!(reason.rule.as_deref(), Some("rest"));
    assert_eq!(
        reason.to_string(),
        "rich: too early after a late finish (rule `rest`)"
    );
}

#[test]
fn a_blackout_never_counts_as_rest_activity() {
    // Blackouts spanning the evening and the morning exclude nothing by rest:
    // only blocking, non-all-day blocks are activity. The booking at 09:00 is
    // outside both blackouts, so it is offered.
    let blackout = Rule::new(
        "freeze",
        RuleKind::Blackout(BlackoutSpec::new(
            RuleZone::Named(ZoneName::new("UTC").expect("a zone name")),
            vec![
                BlackoutSpan::new(
                    LocalDateTime::new(date(MONDAY), time(18, 0)),
                    LocalDateTime::new(date(MONDAY), time(23, 0)),
                )
                .expect("the fixture blackout is ascending"),
                BlackoutSpan::new(
                    LocalDateTime::new(date(TUESDAY), time(0, 0)),
                    LocalDateTime::new(date(TUESDAY), time(8, 0)),
                )
                .expect("the fixture blackout is ascending"),
            ],
        )),
    );
    let morning = utc(TUESDAY, 9, 0);
    let request = request(
        utc(MONDAY, 0, 0),
        span(utc(TUESDAY, 8, 0), utc(TUESDAY, 10, 0)),
        vec![morning],
        vec![host("rich", Vec::new())],
        vec![blackout, evening_rest()],
    );
    let response = engine().find_availability(&request).unwrap();
    assert_eq!(offered(&response), [morning]);
}

#[test]
fn an_ignored_block_and_its_travel_never_count_as_activity() {
    // The first matching status-policy entry ignores tentative blocks, and an
    // ignored block's travel is ignored with it. The tentative block reaches the
    // late hours only through its travel, so without the policy it would protect
    // the morning.
    let mut tentative = block(utc(MONDAY, 19, 0), utc(MONDAY, 19, 30));
    tentative.status = BlockStatus::Tentative;
    tentative.travel_after = Duration::from_minutes(120);
    let morning = utc(TUESDAY, 9, 0);
    let search = span(utc(TUESDAY, 8, 0), utc(TUESDAY, 10, 0));

    let ignored = Rule::new(
        "ignore-tentative",
        RuleKind::StatusPolicy(StatusPolicy {
            entries: vec![StatusPolicyEntry::status(
                BlockStatus::Tentative,
                StatusEffect::Ignore,
            )],
        }),
    );
    let ignored_request = request(
        utc(MONDAY, 0, 0),
        search,
        vec![morning],
        vec![host("rich", vec![tentative.clone()])],
        vec![ignored, evening_rest()],
    );
    let response = engine().find_availability(&ignored_request).unwrap();
    assert_eq!(
        offered(&response),
        [morning],
        "an ignored block and its travel are not activity"
    );

    let request = request(
        utc(MONDAY, 0, 0),
        search,
        vec![morning],
        vec![host("rich", vec![tentative])],
        vec![evening_rest()],
    );
    let response = engine().find_availability(&request).unwrap();
    assert!(offered(&response).is_empty());
    assert_eq!(
        response.excluded[0].reasons[0].code,
        ReasonCode::Rest,
        "a blocking block's travel reaches the late hours"
    );
}

#[test]
fn a_booking_that_crosses_midnight_excludes_itself() {
    // A booking running from 23:30 into the protected morning is exactly what
    // the rule prevents, so it excludes itself even with an empty calendar.
    let crossing = utc(MONDAY, 23, 30);
    let evening = utc(MONDAY, 22, 0);
    let request = request(
        utc(MONDAY, 0, 0),
        span(utc(MONDAY, 21, 0), utc(TUESDAY, 2, 0)),
        vec![evening, crossing],
        vec![host("rich", Vec::new())],
        vec![evening_rest()],
    );
    // The part lasts an hour and a half, so the second booking ends at 01:00.
    let mut request = request;
    request.goal.parts[0].duration = Duration::from_minutes(90);
    let response = engine().find_availability(&request).unwrap();
    assert_eq!(
        offered(&response),
        [evening],
        "a booking ending at 23:30 never reaches the protected morning"
    );
    assert_eq!(
        response
            .excluded
            .iter()
            .map(|entry| (entry.start, entry.reasons[0].code))
            .collect::<Vec<_>>(),
        [(crossing, ReasonCode::Rest)]
    );
}

#[test]
fn clearance_precedes_rest_and_check_slot_reports_both() {
    // One start fails clearance against the morning meeting and rest against the
    // evening one. The search reports clearance first; confirmation reports both.
    let morning = utc(TUESDAY, 9, 0);
    let buffers = presets::buffers(Duration::from_minutes(30), Duration::from_minutes(0));
    let request = request(
        utc(MONDAY, 0, 0),
        span(utc(TUESDAY, 8, 0), utc(TUESDAY, 10, 0)),
        vec![morning],
        vec![host(
            "rich",
            vec![
                block(utc(MONDAY, 19, 0), utc(MONDAY, 20, 0)),
                block(utc(TUESDAY, 8, 0), utc(TUESDAY, 8, 40)),
            ],
        )],
        vec![buffers, evening_rest()],
    );
    let response = engine().find_availability(&request).unwrap();
    let reason = &response.excluded[0].reasons[0];
    assert_eq!(reason.code, ReasonCode::Clearance);
    assert_eq!(reason.rule.as_deref(), Some("buffers"));

    let check = engine().check_slot(&request, &proposal(morning)).unwrap();
    assert!(!check.valid);
    let codes: Vec<ReasonCode> = check.reasons.iter().map(|reason| reason.code).collect();
    assert_eq!(
        codes,
        [ReasonCode::Clearance, ReasonCode::Rest],
        "confirmation reports every failing reason in precedence order"
    );
    assert_eq!(
        check.reasons[1].rule.as_deref(),
        Some("rest"),
        "the rest reason cites its rule"
    );

    // An offered start confirms, so the two entry points agree in both
    // directions.
    let later = utc(TUESDAY, 10, 0);
    let mut offered_request = request.clone();
    offered_request.goal.candidates = Some(vec![later]);
    offered_request.goal.search_window = span(utc(TUESDAY, 9, 0), utc(TUESDAY, 11, 0));
    let response = engine().find_availability(&offered_request).unwrap();
    assert_eq!(offered(&response), [later]);
    let check = engine()
        .check_slot(&offered_request, &proposal(later))
        .unwrap();
    assert!(check.valid);
    assert!(check.reasons.is_empty());
}

#[test]
fn required_ranges_covers_the_neighbouring_days_rest_reads() {
    // A late Monday meeting excludes a Tuesday morning booking, so the calendar
    // range must cover Monday as well as the search window's own day.
    let search = span(utc(TUESDAY, 9, 0), utc(TUESDAY, 17, 0));
    let evening = block(utc(MONDAY, 19, 0), utc(MONDAY, 20, 0));
    let request = request(
        utc(MONDAY, 0, 0),
        search,
        vec![utc(TUESDAY, 9, 0)],
        vec![host("rich", vec![evening.clone()])],
        vec![evening_rest()],
    );
    let mut query = RangesQuery::new(request.now, request.goal.clone());
    query.rules.clone_from(&request.rules);
    query.participants.clone_from(&request.participants);
    let ranges = engine().required_ranges(&query).unwrap();
    assert!(
        ranges.calendar.start() <= evening.span.start(),
        "the calendar range {}..{} does not reach the evening before the search",
        ranges.calendar.start(),
        ranges.calendar.end()
    );
    assert!(ranges.calendar.contains(utc(MONDAY, 23, 59)));

    let response = engine().find_availability(&request).unwrap();
    assert!(offered(&response).is_empty());
    assert_eq!(response.excluded[0].reasons[0].code, ReasonCode::Rest);
}

#[test]
fn a_rest_rule_naming_no_participant_selects_every_host() {
    // `who` defaults to hosts like any other kind, so one rule protects both
    // hosts until a selector narrows it.
    let ana = host("ana", vec![block(utc(MONDAY, 19, 0), utc(MONDAY, 20, 0))]);
    let bo = host("bo", vec![block(utc(MONDAY, 19, 0), utc(MONDAY, 20, 0))]);
    let mut goal = Goal::single(
        ZoneName::new("UTC").expect("a zone name"),
        span(utc(TUESDAY, 8, 0), utc(TUESDAY, 10, 0)),
        Duration::from_minutes(30),
        Requirement::new(vec![Term::at_least(
            2,
            opportune::tags::Selector::Role(opportune::tags::Role::Host),
        )]),
    );
    goal.candidates = Some(vec![utc(TUESDAY, 9, 0)]);
    let mut request = Request::new(utc(MONDAY, 0, 0), goal);
    request.participants = vec![ana, bo];
    request.rules = vec![evening_rest()];
    request.explain = Some(Explain::new(10_000));
    let response = engine().find_availability(&request).unwrap();
    assert!(offered(&response).is_empty());
    let reasons: Vec<(Option<&str>, ReasonCode)> = response.excluded[0]
        .reasons
        .iter()
        .map(|reason| (reason.participant.as_deref(), reason.code))
        .collect();
    assert_eq!(
        reasons,
        [
            (Some("ana"), ReasonCode::Rest),
            (Some("bo"), ReasonCode::Rest),
        ]
    );

    // Scoped to one host, the rule leaves the other free and the start is
    // offered with `bo`:
    request.rules =
        vec![evening_rest().with_who(opportune::tags::Selector::Ids(vec!["ana".to_owned()]))];
    let response = engine().find_availability(&request).unwrap();
    // The requirement needs two distinct hosts, so one eligible host still
    // leaves the start unfillable — and the remaining host's reason is absent
    // while the term is short.
    assert!(offered(&response).is_empty());
    let reasons: Vec<(Option<&str>, ReasonCode)> = response.excluded[0]
        .reasons
        .iter()
        .map(|reason| (reason.participant.as_deref(), reason.code))
        .collect();
    assert_eq!(reasons, [(Some("ana"), ReasonCode::Rest)]);
}

#[test]
fn a_rest_rule_leaves_an_event_with_no_activity_alone() {
    // The rule is inert on an empty calendar: it only removes what a late finish
    // or an early start would protect.
    let starts = vec![utc(TUESDAY, 9, 0), utc(TUESDAY, 11, 0)];
    let request = request(
        utc(MONDAY, 0, 0),
        span(utc(TUESDAY, 8, 0), utc(TUESDAY, 12, 0)),
        starts.clone(),
        vec![host("rich", Vec::new())],
        vec![evening_rest()],
    );
    let response = engine().find_availability(&request).unwrap();
    assert_eq!(offered(&response), starts);
    assert!(response.excluded.is_empty());
}

#[test]
fn required_ranges_reaches_a_zone_the_participant_left_before_the_window() {
    // The host leaves Kiritimati (UTC+14) for Pago Pago (UTC-11) a day before the
    // window. The Kiritimati evening and small hours are what protect the booking
    // in Pago Pago, and they sit before the hull the window alone would give, so
    // the calendar range has to reach them.
    let monday = (2027, 6, 14);
    let tuesday = (2027, 6, 15);
    let mut participant = host(
        "rich",
        vec![
            // 22:00–22:30 on 14 June in Kiritimati.
            block(utc(monday, 8, 0), utc(monday, 8, 30)),
            // 00:00–00:30 on 15 June in Kiritimati.
            block(utc(monday, 10, 0), utc(monday, 10, 30)),
        ],
    );
    participant.zone = ZoneName::new("Etc/GMT+11").expect("a zone name");
    participant.zone_periods.push(
        opportune::model::ZonePeriod::new(
            ZoneName::new("Pacific/Kiritimati").expect("a zone name"),
            utc((2027, 6, 13), 0, 0),
            utc(monday, 12, 0),
        )
        .expect("the fixture leg is non-empty"),
    );
    let booking = utc(tuesday, 15, 0); // 04:00 on 15 June in Pago Pago
    let request = request(
        utc(monday, 0, 0),
        span(utc(tuesday, 12, 0), utc(tuesday, 23, 0)),
        vec![booking],
        vec![participant],
        vec![evening_rest()],
    );

    let mut query = RangesQuery::new(request.now, request.goal.clone());
    query.rules.clone_from(&request.rules);
    query.participants.clone_from(&request.participants);
    let ranges = engine().required_ranges(&query).unwrap();
    assert!(
        ranges.calendar.start() <= utc(monday, 8, 0),
        "the calendar range {}..{} misses the Kiritimati evening",
        ranges.calendar.start(),
        ranges.calendar.end()
    );
    assert!(ranges.calendar.start() <= utc(monday, 10, 0));

    let response = engine().find_availability(&request).unwrap();
    assert!(offered(&response).is_empty());
    assert_eq!(response.excluded[0].reasons[0].code, ReasonCode::Rest);
}

#[test]
fn rest_scoped_to_an_attendee_gates_the_offer() {
    // An attendee's late finish protects their morning like a host's, and the
    // start-level `attendee_unavailable` explains why the offer is missing even
    // though the host is free.
    let monday = (2027, 6, 14);
    let tuesday = (2027, 6, 15);
    let mut attendee = host("tom", vec![block(utc(monday, 19, 0), utc(monday, 20, 0))]);
    attendee.role = opportune::tags::Role::Attendee;
    let scoped = evening_rest().with_who(opportune::tags::Selector::Role(
        opportune::tags::Role::Attendee,
    ));
    let booking = utc(tuesday, 9, 0);
    let request = request(
        utc(monday, 0, 0),
        span(utc(tuesday, 8, 0), utc(tuesday, 10, 0)),
        vec![booking],
        vec![host("rich", Vec::new()), attendee],
        vec![scoped],
    );
    let response = engine().find_availability(&request).unwrap();
    assert!(offered(&response).is_empty());
    let reasons: Vec<(Option<&str>, ReasonCode)> = response.excluded[0]
        .reasons
        .iter()
        .map(|reason| (reason.participant.as_deref(), reason.code))
        .collect();
    assert_eq!(
        reasons,
        [
            (Some("tom"), ReasonCode::Rest),
            (None, ReasonCode::AttendeeUnavailable),
        ]
    );
    assert_eq!(
        response.excluded[0].reasons[0].rule.as_deref(),
        Some("rest")
    );
}

#[test]
fn rest_reads_the_local_date_a_backward_transition_repeats() {
    // Newfoundland ended daylight saving at 00:01 local until 2010, so
    // 2010-11-07T02:31Z moves the local reading *backwards* across midnight:
    // 02:30Z is 00:00 on 7 November and 02:31Z is 23:01 on 6 November. A span
    // beginning at 02:30Z therefore reaches both local dates, and a date walk
    // that jumps from 00:00 on 7 November straight to the next midnight loses
    // 6 November entirely.
    //
    // The rule protects everything before 23:30 after a finish at or after
    // 22:00. Both directions — existing activity crossing the transition, and
    // the booking itself crossing it — must pair 5 November's late hours with
    // the repeated 6 November minutes.
    let saturday = (2010, 11, 6);
    let sunday = (2010, 11, 7);
    let rule = presets::rest(time(22, 0), time(23, 30));

    // Activity crosses the transition (00:00 on 7 Nov, then 23:01–23:29 on 6 Nov);
    // the booking is late on 5 November (22:00–22:30 local).
    let mut participant = Participant::new("rich", ZoneName::new("America/St_Johns").unwrap());
    participant.calendar = Some(vec![block(utc(sunday, 2, 30), utc(sunday, 3, 0))]);
    let evening_booking = utc(saturday, 0, 30);
    let req = request(
        utc((2010, 11, 1), 0, 0),
        span(utc(saturday, 0, 0), utc(saturday, 12, 0)),
        vec![evening_booking],
        vec![participant],
        vec![rule.clone()],
    );
    let response = engine().find_availability(&req).unwrap();
    assert!(
        offered(&response).is_empty(),
        "the 6 November early minutes the transition repeats are rest activity, \
         so the 5 November evening booking is excluded"
    );
    assert_eq!(response.excluded[0].reasons[0].code, ReasonCode::Rest);

    // Mirror: existing activity is late on 5 November (22:00–22:30 local), and
    // the booking itself starts at 02:30Z and crosses the backward transition.
    let mut participant = Participant::new("rich", ZoneName::new("America/St_Johns").unwrap());
    participant.calendar = Some(vec![block(utc(saturday, 0, 30), utc(saturday, 1, 0))]);
    let crossing_booking = utc(sunday, 2, 30);
    let req = request(
        utc((2010, 11, 1), 0, 0),
        span(utc(sunday, 2, 0), utc(sunday, 4, 0)),
        vec![crossing_booking],
        vec![participant],
        vec![rule],
    );
    let response = engine().find_availability(&req).unwrap();
    assert!(
        offered(&response).is_empty(),
        "a booking crossing the backward transition reaches the repeated \
         6 November early minutes and is excluded"
    );
    assert_eq!(response.excluded[0].reasons[0].code, ReasonCode::Rest);
}

#[test]
fn required_ranges_cover_the_date_line_case_rest_adds() {
    // Home Kiritimati (UTC+14) with a leg in Etc/GMT+12 (UTC-12) across the
    // search window. The meeting that protects the booking is the evening before
    // in Kiritimati, which is five hours earlier than the hull the window alone
    // gives. A rest rule widens the calendar range far enough to reach it.
    let saturday = (2027, 3, 6);
    let sunday = (2027, 3, 7);
    let mut participant = host(
        "rich",
        vec![block(utc(saturday, 5, 0), utc(saturday, 5, 30))],
    );
    participant.zone = ZoneName::new("Pacific/Kiritimati").expect("a zone name");
    participant.zone_periods.push(
        opportune::model::ZonePeriod::new(
            ZoneName::new("Etc/GMT+12").expect("a zone name"),
            utc(sunday, 18, 0),
            utc((2027, 3, 8), 18, 0),
        )
        .expect("the fixture leg is non-empty"),
    );
    let booking = utc(sunday, 20, 0); // 08:00 on 7 March in UTC-12
    let request = request(
        utc(sunday, 0, 0),
        span(utc(sunday, 20, 0), utc((2027, 3, 8), 20, 0)),
        vec![booking],
        vec![participant],
        vec![evening_rest()],
    );

    let mut query = RangesQuery::new(request.now, request.goal.clone());
    query.rules.clone_from(&request.rules);
    query.participants.clone_from(&request.participants);
    let ranges = engine().required_ranges(&query).unwrap();
    assert!(
        ranges.calendar.start() <= utc(saturday, 5, 0),
        "the calendar range {}..{} misses the Kiritimati evening",
        ranges.calendar.start(),
        ranges.calendar.end()
    );

    let response = engine().find_availability(&request).unwrap();
    assert!(offered(&response).is_empty());
    assert_eq!(response.excluded[0].reasons[0].code, ReasonCode::Rest);
}

/// The last minute the model can name lies inside a rest rule's protected morning
/// in a zone ahead of UTC, so a confirmation there is judged against a late finish
/// the evening before like any other, and the booking ends at the last instant the
/// model can name.
#[test]
fn rest_protects_the_last_supported_minute() {
    let evening = block(utc((2099, 12, 31), 17, 0), utc((2099, 12, 31), 18, 0));
    let mut participant = host("rich", vec![evening.clone()]);
    // UTC+1 in winter, so 23:59Z is 00:59 on 1 January locally: inside the
    // protected morning after the previous evening's late finish.
    participant.zone = ZoneName::new("Europe/Paris").expect("a zone name");
    let last = Instant::from_minutes(Instant::MAX.minutes() - 1).expect("a supported instant");
    let mut request = request(
        utc((2099, 12, 30), 0, 0),
        span(utc((2099, 12, 30), 0, 0), Instant::MAX),
        vec![last],
        vec![participant],
        vec![evening_rest()],
    );
    request.goal.parts[0].duration = Duration::from_minutes(1);

    let check = engine()
        .check_slot(&request, &proposal(last))
        .expect("the proposal fits the request");
    assert!(!check.valid);
    assert_eq!(
        check
            .reasons
            .iter()
            .map(|reason| reason.code)
            .collect::<Vec<_>>(),
        vec![ReasonCode::Rest],
        "the previous evening's late finish protects the last supported minute"
    );
}
