//! End-to-end tests for structured reasons, `explain`, `check_slot`, and the
//! single-part sugar and presets, over the bundled database.
//!
//! Every test builds a real request, runs it on a real engine and asserts the
//! answer a caller acts on, so deleting the behaviour it covers turns it red.

#![cfg(feature = "bundled-tzdb")]

use std::collections::BTreeMap;

use opportune::model::{
    Block, Cap, CapMeasure, CapPer, CapPeriod, Clearance, Explain, Goal, LeadTime, LeadTimeMax,
    Offer, Part, Participant, Proposal, ProposalEvent, Request, Requirement, Rule, RuleKind, Span,
    Term, Travel, ZonePeriod,
};
use opportune::presets;
use opportune::reasons::{Reason, ReasonCode};
use opportune::tags::{Role, Selector, Tag};
use opportune::time::{
    BlackoutSpan, BlackoutSpec, Date, Duration, Grid, GridZone, Instant, LocalDateTime,
    LocalResolution, RuleZone, TimeOfDay, WallEnd, WallRange, Weekday, Weekdays, WeeklyKey,
    WindowSpec, Zone, ZoneName,
};
use opportune::{Engine, Limits};

// ---------------------------------------------------------------- helpers ---

fn name(zone: &str) -> ZoneName {
    ZoneName::new(zone).unwrap()
}

fn tag(path: &str) -> Tag {
    Tag::new(path).unwrap()
}

/// `count` minutes after the earliest supported instant, `1975-01-01T00:00Z`.
fn minutes(count: i64) -> Instant {
    Instant::from_minutes(Instant::MIN.minutes() + count).unwrap()
}

fn span(start: Instant, end: Instant) -> Span {
    Span::new(start, end).unwrap()
}

/// A window open every day between `start` and `end` o'clock, in UTC.
fn daily_window(start_hour: u8, end_hour: u8) -> WindowSpec {
    let hours = WallRange::new(
        TimeOfDay::new(start_hour, 0).unwrap(),
        WallEnd::Time(TimeOfDay::new(end_hour, 0).unwrap()),
    )
    .unwrap();
    WindowSpec::new(
        RuleZone::Named(name("UTC")),
        vec![
            WeeklyKey::new(
                Weekdays::range(Weekday::Monday, Weekday::Sunday),
                vec![hours],
            )
            .unwrap(),
        ],
        Vec::new(),
    )
    .unwrap()
}

/// A blackout over UTC civil date-times on 1 January 1975.
fn utc_blackout(from: (u8, u8), until: (u8, u8)) -> BlackoutSpec {
    let date = Date::new(1975, 1, 1).unwrap();
    let at =
        |(hour, minute): (u8, u8)| LocalDateTime::new(date, TimeOfDay::new(hour, minute).unwrap());
    BlackoutSpec::new(
        RuleZone::Named(name("UTC")),
        vec![BlackoutSpan::new(at(from), at(until)).unwrap()],
    )
}

fn window_rule(id: &str, spec: WindowSpec) -> Rule {
    Rule::new(id, RuleKind::Window(spec))
}

fn blackout_rule(id: &str, spec: BlackoutSpec) -> Rule {
    Rule::new(id, RuleKind::Blackout(spec))
}

fn block(start: Instant, end: Instant) -> Block {
    Block::new(span(start, end))
}

fn engine() -> Engine {
    Engine::builder().build().unwrap()
}

/// A participant with an empty calendar: known to be free.
fn free_participant(id: &str) -> Participant {
    let mut participant = Participant::new(id, name("UTC"));
    participant.calendar = Some(Vec::new());
    participant
}

/// A minimal request: one 30-minute part for `rich`, over `search`, with the
/// participants and rules given.
fn request_for(search: Span, participants: Vec<Participant>, rules: Vec<Rule>) -> Request {
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::at_least(1, Selector::Role(Role::Host))]),
    );
    let mut request = Request::new(minutes(0), Goal::new(name("UTC"), search, vec![part]));
    request.participants = participants;
    request.rules = rules;
    request
}

/// A request whose only candidate start is `start`, so a test speaks about one
/// instant.
fn request_at(start: Instant, participant: Participant, rules: Vec<Rule>) -> Request {
    let mut request = request_for(span(minutes(0), minutes(24 * 60)), vec![participant], rules);
    request.goal.candidates = Some(vec![start]);
    request
}

fn offer_starts(response: &opportune::model::Response) -> Vec<Instant> {
    response
        .offers
        .iter()
        .map(|offer| offer.start().unwrap())
        .collect()
}

fn reason_codes(reasons: &[Reason]) -> Vec<&'static str> {
    reasons.iter().map(|reason| reason.code.as_str()).collect()
}

fn proposal(start: Instant, assignment: &[&str]) -> Proposal {
    Proposal {
        events: vec![ProposalEvent {
            part: "main".to_owned(),
            start,
            assignment: vec![assignment.iter().map(|id| (*id).to_owned()).collect()],
        }],
    }
}

// ---------------------------------------------------------------- explain ---

#[test]
fn explain_absent_reports_no_exclusions() {
    let mut participant = free_participant("rich");
    participant.calendar = Some(vec![block(minutes(10 * 60), minutes(11 * 60))]);
    let mut request = request_for(
        span(minutes(9 * 60), minutes(12 * 60)),
        vec![participant],
        Vec::new(),
    );
    let response = engine().find_availability(&request).unwrap();
    assert!(!offer_starts(&response).contains(&minutes(10 * 60)));
    assert!(response.excluded.is_empty());
    assert!(!response.excluded_truncated);

    // The same request with an explanation reports the start it left out.
    request.explain = Some(Explain::new(100));
    let response = engine().find_availability(&request).unwrap();
    assert_eq!(response.excluded.len(), 2, "10:00 and 10:30 are blocked");
    assert_eq!(response.excluded[0].start, minutes(10 * 60));
    assert_eq!(response.excluded[0].part, "main");
    assert!(!response.excluded_truncated);
}

#[test]
fn a_blocked_start_cites_the_event_and_reads_as_english() {
    let mut participant = free_participant("rich");
    let mut veto = block(minutes(10 * 60), minutes(11 * 60));
    veto.event_key = Some("evt_91".to_owned());
    veto.tags = vec![tag("veto/crm-freeze")];
    participant.calendar = Some(vec![veto]);

    let mut request = request_at(minutes(10 * 60 + 30), participant, Vec::new());
    request.explain = Some(Explain::new(10));

    let response = engine().find_availability(&request).unwrap();
    let [excluded] = response.excluded.as_slice() else {
        panic!("one excluded start expected, got {:?}", response.excluded);
    };
    let [reason] = excluded.reasons.as_slice() else {
        panic!("one reason expected, got {:?}", excluded.reasons);
    };
    assert_eq!(reason.code, ReasonCode::Blocked);
    assert_eq!(reason.participant.as_deref(), Some("rich"));
    assert_eq!(reason.rule, None, "a calendar event blocks on its own");
    assert_eq!(reason.event_key.as_deref(), Some("evt_91"));
    assert_eq!(reason.tags, [tag("veto/crm-freeze")]);
    assert_eq!(
        reason.to_string(),
        "rich: blocked by an existing event (event `evt_91`) [veto/crm-freeze]"
    );
}

#[test]
fn the_minimum_notice_precedes_the_other_reasons() {
    // The candidate is before `now` and outside the window, and sits on a block.
    let mut participant = free_participant("rich");
    participant.calendar = Some(vec![block(minutes(8 * 60), minutes(9 * 60 + 30))]);
    let mut request = request_at(
        minutes(9 * 60),
        participant,
        vec![window_rule("hours", daily_window(10, 12))],
    );
    request.now = minutes(10 * 60);
    request.explain = Some(Explain::new(10));

    let response = engine().find_availability(&request).unwrap();
    let reason = &response.excluded[0].reasons[0];
    assert_eq!(
        reason.code,
        ReasonCode::LeadTimeMin,
        "the implicit minimum notice comes first"
    );
    assert_eq!(
        reason.rule, None,
        "the implicit minimum has no rule to cite"
    );

    // A `lead_time` rule supplying the minimum is cited.
    request
        .rules
        .push(presets::min_notice(Duration::from_minutes(30)));
    let response = engine().find_availability(&request).unwrap();
    let reason = &response.excluded[0].reasons[0];
    assert_eq!(reason.code, ReasonCode::LeadTimeMin);
    assert_eq!(reason.rule.as_deref(), Some("min_notice"));
}

#[test]
fn a_core_span_failure_is_distinguished_from_a_travel_only_failure() {
    let mut participant = free_participant("rich");
    // Travel reaches back before the window opens, but the booking itself fits.
    participant.calendar = Some(Vec::new());
    let mut request = request_at(
        minutes(10 * 60),
        participant,
        vec![window_rule("hours", daily_window(10, 12))],
    );
    request.goal.parts[0].travel = Travel {
        before: Duration::from_minutes(30),
        after: Duration::ZERO,
        by_participant: BTreeMap::new(),
    };
    request.explain = Some(Explain::new(10));

    let response = engine().find_availability(&request).unwrap();
    let reason = &response.excluded[0].reasons[0];
    assert_eq!(reason.code, ReasonCode::Travel);
    assert_eq!(reason.rule.as_deref(), Some("hours"));

    // The same booking an hour later has its own span outside the window, which
    // is `outside_window` rather than `travel`.
    request.goal.candidates = Some(vec![minutes(12 * 60 + 30)]);
    let response = engine().find_availability(&request).unwrap();
    let reason = &response.excluded[0].reasons[0];
    assert_eq!(reason.code, ReasonCode::OutsideWindow);
    assert_eq!(reason.rule.as_deref(), Some("hours"));
}

#[test]
fn travel_over_a_blackout_is_a_travel_reason_citing_the_blackout() {
    let mut request = request_at(
        minutes(10 * 60),
        free_participant("rich"),
        vec![blackout_rule("freeze", utc_blackout((9, 0), (10, 0)))],
    );
    request.goal.parts[0].travel = Travel {
        before: Duration::from_minutes(30),
        after: Duration::ZERO,
        by_participant: BTreeMap::new(),
    };
    request.explain = Some(Explain::new(10));

    let response = engine().find_availability(&request).unwrap();
    let reason = &response.excluded[0].reasons[0];
    assert_eq!(reason.code, ReasonCode::Travel);
    assert_eq!(
        reason.rule.as_deref(),
        Some("freeze"),
        "the blackout the travel overlapped is cited"
    );
}

#[test]
fn travel_over_a_meeting_carries_its_event_key_and_tags() {
    let mut participant = free_participant("rich");
    let mut meeting = block(minutes(9 * 60 + 30), minutes(10 * 60));
    meeting.event_key = Some("evt_42".to_owned());
    meeting.tags = vec![tag("veto/crm-freeze")];
    participant.calendar = Some(vec![meeting]);

    // The booking ends exactly where the meeting begins, so its own span is
    // clear and only the journey after it reaches the meeting: the reason is
    // `travel`, and it still has to name the event behind it.
    let mut request = request_at(minutes(9 * 60), participant, Vec::new());
    request.goal.parts[0].travel = Travel {
        before: Duration::ZERO,
        after: Duration::from_minutes(30),
        by_participant: BTreeMap::new(),
    };
    request.explain = Some(Explain::new(10));

    let response = engine().find_availability(&request).unwrap();
    let [excluded] = response.excluded.as_slice() else {
        panic!("one excluded start expected, got {:?}", response.excluded);
    };
    let [reason] = excluded.reasons.as_slice() else {
        panic!("one reason expected, got {:?}", excluded.reasons);
    };
    assert_eq!(reason.code, ReasonCode::Travel);
    assert_eq!(reason.rule, None, "a calendar event blocks on its own");
    assert_eq!(reason.event_key.as_deref(), Some("evt_42"));
    assert_eq!(reason.tags, [tag("veto/crm-freeze")]);
    assert_eq!(
        reason.to_string(),
        "rich: travel time is not clear (event `evt_42`) [veto/crm-freeze]"
    );

    // A confirmation reports the same reason with the same evidence.
    let check = engine()
        .check_slot(&request, &proposal(minutes(9 * 60), &["rich"]))
        .unwrap();
    assert_eq!(check.reasons, vec![reason.clone()]);
}

#[test]
fn a_blacked_out_start_cites_its_rule() {
    let mut request = request_at(
        minutes(10 * 60),
        free_participant("rich"),
        vec![blackout_rule("freeze", utc_blackout((9, 0), (10, 30)))],
    );
    request.explain = Some(Explain::new(10));

    let response = engine().find_availability(&request).unwrap();
    let reason = &response.excluded[0].reasons[0];
    assert_eq!(reason.code, ReasonCode::Blackout);
    assert_eq!(reason.rule.as_deref(), Some("freeze"));
    assert_eq!(reason.event_key, None);
}

#[test]
fn a_blackout_is_reported_before_the_block_it_covers() {
    // Both a blackout and a calendar event cover the booking. The explanation
    // reports the earlier reason only, and a confirmation reports both in order.
    let mut participant = free_participant("rich");
    participant.calendar = Some(vec![block(minutes(10 * 60), minutes(11 * 60))]);
    let mut request = request_at(
        minutes(10 * 60 + 30),
        participant,
        vec![blackout_rule("freeze", utc_blackout((9, 0), (12, 0)))],
    );
    request.explain = Some(Explain::new(10));

    let response = engine().find_availability(&request).unwrap();
    assert_eq!(
        reason_codes(&response.excluded[0].reasons),
        ["blackout"],
        "the explanation reports the first failing reason"
    );
    let check = engine()
        .check_slot(&request, &proposal(minutes(10 * 60 + 30), &["rich"]))
        .unwrap();
    assert_eq!(
        reason_codes(&check.reasons),
        ["blackout", "blocked"],
        "a confirmation reports both, blackout first"
    );
}

#[test]
fn clearance_is_judged_on_the_occupied_span_and_cites_the_gap_rule() {
    let mut participant = free_participant("rich");
    let mut adjacent = block(minutes(9 * 60), minutes(10 * 60));
    adjacent.event_key = Some("evt_7".to_owned());
    participant.calendar = Some(vec![adjacent]);

    // A five-minute gap required before the booking widens the preceding
    // block's end past the booking's start; the `before` gap supplies it.
    let mut request = request_at(
        minutes(10 * 60),
        participant.clone(),
        vec![Rule::new(
            "buffers",
            RuleKind::Clearance(Clearance {
                before: Duration::from_minutes(5),
                after: Duration::from_minutes(10),
                min_gap: Duration::ZERO,
            }),
        )],
    );
    request.explain = Some(Explain::new(10));
    let response = engine().find_availability(&request).unwrap();
    let reason = &response.excluded[0].reasons[0];
    assert_eq!(reason.code, ReasonCode::Clearance);
    assert_eq!(reason.rule.as_deref(), Some("buffers"));
    assert_eq!(reason.event_key.as_deref(), Some("evt_7"));

    // A larger least gap is the value that applies, so that rule is cited.
    request.rules = vec![
        Rule::new(
            "buffers",
            RuleKind::Clearance(Clearance {
                before: Duration::from_minutes(5),
                after: Duration::from_minutes(10),
                min_gap: Duration::ZERO,
            }),
        ),
        Rule::new(
            "least-gap",
            RuleKind::Clearance(Clearance {
                before: Duration::ZERO,
                after: Duration::ZERO,
                min_gap: Duration::from_minutes(25),
            }),
        ),
    ];
    let response = engine().find_availability(&request).unwrap();
    let reason = &response.excluded[0].reasons[0];
    assert_eq!(reason.code, ReasonCode::Clearance);
    assert_eq!(reason.rule.as_deref(), Some("least-gap"));
}

#[test]
fn same_kind_ties_cite_the_earliest_rule_in_request_order() {
    let mut request = request_at(
        minutes(10 * 60),
        free_participant("rich"),
        vec![
            window_rule("first", daily_window(8, 9)),
            window_rule("second", daily_window(8, 9)),
        ],
    );
    request.explain = Some(Explain::new(10));
    let response = engine().find_availability(&request).unwrap();
    let reason = &response.excluded[0].reasons[0];
    assert_eq!(reason.code, ReasonCode::OutsideWindow);
    assert_eq!(reason.rule.as_deref(), Some("first"));
}

#[test]
fn the_earliest_maximum_is_cited() {
    let mut request = request_at(
        minutes(90),
        free_participant("rich"),
        vec![
            presets::horizon(LeadTimeMax::Duration(Duration::from_minutes(60))),
            Rule::new(
                "long-horizon",
                RuleKind::LeadTime(LeadTime {
                    min: Duration::ZERO,
                    max: Some(LeadTimeMax::Duration(Duration::from_minutes(120))),
                }),
            ),
        ],
    );
    request.explain = Some(Explain::new(10));
    let response = engine().find_availability(&request).unwrap();
    let reason = &response.excluded[0].reasons[0];
    assert_eq!(reason.code, ReasonCode::LeadTimeMax);
    assert_eq!(reason.rule.as_deref(), Some("horizon"));

    // Equal maxima keep the earliest rule in request order.
    request.rules = vec![
        presets::horizon(LeadTimeMax::Duration(Duration::from_minutes(60))),
        Rule::new(
            "same-as-horizon",
            RuleKind::LeadTime(LeadTime {
                min: Duration::ZERO,
                max: Some(LeadTimeMax::Duration(Duration::from_minutes(60))),
            }),
        ),
    ];
    let response = engine().find_availability(&request).unwrap();
    assert_eq!(
        response.excluded[0].reasons[0].rule.as_deref(),
        Some("horizon")
    );
}

#[test]
fn explain_truncates_atomically_at_the_limit() {
    // Two hosts a single term can fill, both blocked: each excluded start carries
    // two reasons, so a limit of one cannot report either of them.
    let mut ana = free_participant("ana");
    ana.tags = vec![tag("team/interviewers")];
    ana.calendar = Some(vec![block(minutes(10 * 60), minutes(11 * 60))]);
    let mut tom = free_participant("tom");
    tom.tags = vec![tag("team/interviewers")];
    tom.calendar = Some(vec![block(minutes(10 * 60), minutes(11 * 60))]);

    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::at_least(
            1,
            Selector::Tag(tag("team/interviewers")),
        )]),
    );
    let mut request = Request::new(
        minutes(0),
        Goal::new(
            name("UTC"),
            span(minutes(9 * 60), minutes(12 * 60)),
            vec![part],
        ),
    );
    request.participants = vec![ana, tom];

    request.explain = Some(Explain::new(1));
    let response = engine().find_availability(&request).unwrap();
    assert!(
        response.excluded.is_empty(),
        "an entry needing two reasons is not reported with one"
    );
    assert!(response.excluded_truncated);

    request.explain = Some(Explain::new(2));
    let response = engine().find_availability(&request).unwrap();
    assert_eq!(response.excluded.len(), 1);
    assert_eq!(response.excluded[0].start, minutes(10 * 60));
    let ids: Vec<&str> = response.excluded[0]
        .reasons
        .iter()
        .map(|reason| reason.participant.as_deref().unwrap_or(""))
        .collect();
    assert_eq!(ids, ["ana", "tom"], "reasons are sorted by participant ID");
    assert!(
        response.excluded_truncated,
        "10:30 would have needed two more"
    );

    request.explain = Some(Explain::new(4));
    let response = engine().find_availability(&request).unwrap();
    assert_eq!(response.excluded.len(), 2);
    assert_eq!(response.excluded[1].start, minutes(10 * 60 + 30));
    assert!(!response.excluded_truncated);

    // An excluded start is never one the same response offered.
    let offered = offer_starts(&response);
    assert!(
        response
            .excluded
            .iter()
            .all(|excluded| !offered.contains(&excluded.start))
    );
}

#[test]
fn an_explain_limit_above_the_configured_maximum_is_a_validation_error() {
    let engine = Engine::builder()
        .limits(Limits::default().with_max_explain_limit(5))
        .build()
        .unwrap();
    let mut request = request_at(
        minutes(10 * 60),
        free_participant("rich"),
        vec![window_rule("hours", daily_window(8, 9))],
    );
    request.explain = Some(Explain::new(5));
    assert!(engine.validate(&request).is_valid());

    request.explain = Some(Explain::new(6));
    let diagnostics = engine.validate(&request);
    assert_eq!(diagnostics.errors[0].code(), "explain_limit");
    let err = engine.find_availability(&request).unwrap_err();
    assert_eq!(err.code(), "invalid_request");
}

// -------------------------------------------------------------- check_slot ---

#[test]
fn check_slot_agrees_with_every_offer() {
    let mut participant = free_participant("rich");
    participant.calendar = Some(vec![block(minutes(10 * 60), minutes(11 * 60))]);
    let mut request = request_for(
        span(minutes(9 * 60), minutes(13 * 60)),
        vec![participant],
        vec![window_rule("hours", daily_window(9, 13))],
    );
    request.goal.parts[0].travel = Travel {
        before: Duration::from_minutes(20),
        after: Duration::from_minutes(20),
        by_participant: BTreeMap::new(),
    };

    let response = engine().find_availability(&request).unwrap();
    assert!(!response.offers.is_empty());
    for offer in &response.offers {
        let event = &offer.events[0];
        let check = engine()
            .check_slot(
                &request,
                &Proposal {
                    events: vec![ProposalEvent {
                        part: event.part.clone(),
                        start: event.start,
                        assignment: event.assignment.clone(),
                    }],
                },
            )
            .unwrap();
        assert!(check.valid, "{:?} should be bookable", event.start);
        assert!(check.reasons.is_empty());
        assert_eq!(check.flags, offer.flags);
        assert_eq!(check.reserve, event.reserve);
        assert_eq!(check.meta, response.meta);
        assert_eq!(check.warnings, response.warnings);
    }

    // An excluded start is not valid, and its reasons name the event.
    let excluded_start = minutes(10 * 60);
    let check = engine()
        .check_slot(&request, &proposal(excluded_start, &["rich"]))
        .unwrap();
    assert!(!check.valid);
    assert_eq!(reason_codes(&check.reasons), ["blocked"]);
}

#[test]
fn check_slot_flags_a_host_with_no_calendar() {
    let request = request_at(
        minutes(9 * 60),
        Participant::new("rich", name("UTC")),
        vec![window_rule("hours", daily_window(8, 12))],
    );
    let check = engine()
        .check_slot(&request, &proposal(minutes(9 * 60), &["rich"]))
        .unwrap();
    assert!(check.valid);
    assert_eq!(check.flags, [opportune::model::OfferFlag::CalendarUnknown]);
    let offer = &engine().find_availability(&request).unwrap().offers[0];
    assert_eq!(offer.flags, check.flags);
}

#[test]
fn check_slot_ignores_the_grid_and_the_search_window() {
    let mut request = request_at(
        minutes(12 * 60),
        free_participant("rich"),
        vec![window_rule("hours", daily_window(8, 13))],
    );
    // The search window and the candidate list say nothing about noon.
    request.goal.search_window = span(minutes(0), minutes(60));
    request.goal.candidates = Some(Vec::new());
    assert!(
        engine()
            .find_availability(&request)
            .unwrap()
            .offers
            .is_empty()
    );

    let check = engine()
        .check_slot(&request, &proposal(minutes(12 * 60), &["rich"]))
        .unwrap();
    assert!(
        check.valid,
        "the search window does not bound a confirmation"
    );
}

#[test]
fn check_slot_ignores_the_shape_and_limits_of_a_search() {
    // A confirmation names the start it proposes, so nothing about how a search
    // would generate starts decides anything for it: not the search window's
    // length, the number of starts a grid produces, a goal that carries both a
    // grid and candidate starts, or a grid zone that cannot be resolved.
    let mut request = request_at(
        minutes(12 * 60),
        free_participant("rich"),
        vec![window_rule("nights", daily_window(0, 1))],
    );
    request.goal.search_window = span(minutes(0), minutes(365 * 24 * 60));
    request.goal.grid = Some(
        Grid::new(
            Duration::from_minutes(1),
            TimeOfDay::MIDNIGHT,
            GridZone::Named(name("Mars/Olympus")),
        )
        .unwrap(),
    );

    // The search itself cannot run, and says why.
    let diagnostics = engine().validate(&request);
    let errors: Vec<&str> = diagnostics
        .errors
        .iter()
        .map(opportune::Diagnostic::code)
        .collect();
    for expected in ["search_window_limit", "grid_and_candidates", "zone_unknown"] {
        assert!(errors.contains(&expected), "{expected}: {errors:?}");
    }
    assert!(engine().find_availability(&request).is_err());

    // A grid alone, fine enough to produce more starts than the configured
    // maximum, is a search error too and still no obstacle to a confirmation.
    let mut gridded = request.clone();
    gridded.goal.candidates = None;
    gridded.goal.grid = Some(
        Grid::new(
            Duration::from_minutes(1),
            TimeOfDay::MIDNIGHT,
            GridZone::Named(name("UTC")),
        )
        .unwrap(),
    );
    let errors: Vec<&str> = engine()
        .validate(&gridded)
        .errors
        .iter()
        .map(opportune::Diagnostic::code)
        .collect();
    assert!(errors.contains(&"candidates_limit"), "{errors:?}");
    let check = engine()
        .check_slot(&gridded, &proposal(minutes(12 * 60), &["rich"]))
        .expect("a candidate-start limit does not reject a confirmation");
    assert!(!check.valid, "the window still excludes the proposed start");

    // The confirmation is judged on the rules alone, and reports the rule that
    // excludes the proposed start rather than the search it could not run.
    let check = engine()
        .check_slot(&request, &proposal(minutes(12 * 60), &["rich"]))
        .unwrap();
    assert!(!check.valid);
    assert_eq!(reason_codes(&check.reasons), ["outside_window"]);
    assert_eq!(check.reasons[0].rule.as_deref(), Some("nights"));
    let warnings: Vec<&str> = check
        .warnings
        .iter()
        .map(opportune::Diagnostic::code)
        .collect();
    assert!(
        !warnings.contains(&"no_candidate_in_window"),
        "a confirmation generates no candidates to compare against a window: {warnings:?}"
    );
    let validate_warnings: Vec<&str> = diagnostics
        .warnings
        .iter()
        .map(opportune::Diagnostic::code)
        .collect();
    assert!(
        validate_warnings.contains(&"no_candidate_in_window"),
        "the search still warns that no start falls in the window: {validate_warnings:?}"
    );
}

#[test]
fn check_slot_enforces_the_horizon() {
    let mut request = request_at(
        minutes(120),
        free_participant("rich"),
        vec![presets::horizon(LeadTimeMax::Duration(
            Duration::from_minutes(60),
        ))],
    );
    request.goal.search_window = span(minutes(0), minutes(24 * 60));

    let check = engine()
        .check_slot(&request, &proposal(minutes(120), &["rich"]))
        .unwrap();
    assert!(!check.valid);
    assert_eq!(reason_codes(&check.reasons), ["lead_time_max"]);
    assert_eq!(check.reasons[0].rule.as_deref(), Some("horizon"));
}

#[test]
fn check_slot_returns_every_failing_reason_in_precedence_order() {
    let mut participant = free_participant("rich");
    participant.calendar = Some(vec![block(minutes(8 * 60), minutes(9 * 60 + 30))]);
    let mut request = request_at(
        minutes(9 * 60),
        participant,
        vec![
            window_rule("hours", daily_window(10, 12)),
            // A horizon that ends before 1975: every start is past it.
            presets::horizon(LeadTimeMax::Date(Date::new(1974, 12, 31).unwrap())),
        ],
    );
    request.now = minutes(10 * 60);

    let check = engine()
        .check_slot(&request, &proposal(minutes(9 * 60), &["rich"]))
        .unwrap();
    assert!(!check.valid);
    assert_eq!(
        reason_codes(&check.reasons),
        [
            "lead_time_min",
            "lead_time_max",
            "outside_window",
            "blocked"
        ],
        "every failing reason, in the canonical order"
    );
}

#[test]
fn a_malformed_proposal_is_an_error() {
    let mut participant = free_participant("rich");
    participant.tags = vec![tag("team/interviewers")];
    let mut request = request_at(
        minutes(9 * 60),
        participant,
        vec![window_rule("hours", daily_window(8, 12))],
    );

    let check = |proposal: &Proposal| {
        engine()
            .check_slot(&request, proposal)
            .map(|check| check.valid)
            .map_err(|err| err.code())
    };

    let mut unknown_part = proposal(minutes(9 * 60), &["rich"]);
    unknown_part.events[0].part = "other".to_owned();
    assert_eq!(check(&unknown_part), Err("proposal_part_unknown"));

    let mut duplicated_part = proposal(minutes(9 * 60), &["rich"]);
    duplicated_part
        .events
        .push(duplicated_part.events[0].clone());
    assert_eq!(check(&duplicated_part), Err("proposal_part_duplicated"));

    assert_eq!(
        check(&Proposal { events: Vec::new() }),
        Err("proposal_part_missing")
    );

    let mut wrong_terms = proposal(minutes(9 * 60), &["rich"]);
    wrong_terms.events[0].assignment.push(Vec::new());
    assert_eq!(check(&wrong_terms), Err("proposal_term_count"));

    let mut wrong_hosts = proposal(minutes(9 * 60), &["rich"]);
    wrong_hosts.events[0].assignment[0].clear();
    assert_eq!(check(&wrong_hosts), Err("proposal_host_count"));

    assert_eq!(
        check(&proposal(minutes(9 * 60), &["nobody"])),
        Err("proposal_host_unknown")
    );

    // A named-person term cannot be filled by somebody else.
    let named = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("rich")]),
    );
    let mut named_request = request.clone();
    named_request.goal.parts = vec![named];
    named_request
        .participants
        .push(Participant::new("tom", name("UTC")));
    let err = engine()
        .check_slot(&named_request, &proposal(minutes(9 * 60), &["tom"]))
        .unwrap_err();
    assert_eq!(err.code(), "proposal_host_mismatch");

    // A two-slot term listing the same host twice is malformed, and is reported
    // before the shape this engine version cannot evaluate.
    let pair = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::at_least(
            2,
            Selector::Tag(tag("team/interviewers")),
        )]),
    );
    let mut pair_request = request.clone();
    pair_request.goal.parts = vec![pair];
    let mut tom = free_participant("tom");
    tom.tags = vec![tag("team/interviewers")];
    pair_request.participants.push(tom);
    let mut repeated = proposal(minutes(9 * 60), &["rich"]);
    repeated.events[0].assignment[0] = vec!["rich".to_owned(), "rich".to_owned()];
    let err = engine().check_slot(&pair_request, &repeated).unwrap_err();
    assert_eq!(err.code(), "proposal_host_duplicated");

    // A well-formed two-slot assignment is checked like any other.
    let check = engine()
        .check_slot(&pair_request, &proposal(minutes(9 * 60), &["rich", "tom"]))
        .unwrap();
    assert!(check.valid);

    // An attendee is never assigned, whatever term it is listed under.
    let mut candidate = free_participant("cand");
    candidate.role = Role::Attendee;
    pair_request.participants.push(candidate);
    let err = engine()
        .check_slot(&pair_request, &proposal(minutes(9 * 60), &["rich", "cand"]))
        .unwrap_err();
    assert_eq!(err.code(), "proposal_host_attendee");
    // Keep `request` used after the closure borrows it.
    request.explain = None;
}

#[test]
fn the_free_check_slot_agrees_with_the_engine_method() {
    let request = request_at(
        minutes(9 * 60),
        free_participant("rich"),
        vec![window_rule("hours", daily_window(8, 12))],
    );
    let proposal = proposal(minutes(9 * 60), &["rich"]);
    assert_eq!(
        engine().check_slot(&request, &proposal).unwrap(),
        opportune::check_slot(&request, &proposal).unwrap()
    );
}

// ------------------------------------------------------------------- sugar ---

#[test]
fn single_part_sugar_and_presets_build_the_general_shapes() {
    let zone = name("UTC");
    let search = span(minutes(9 * 60), minutes(12 * 60));
    let duration = Duration::from_minutes(30);
    let requirement = Requirement::new(vec![Term::person("rich")]);

    let sugared = Goal::single(zone.clone(), search, duration, requirement.clone());
    let general = Goal::new(zone.clone(), search, vec![Part::new(duration, requirement)]);
    assert_eq!(sugared, general);

    // The sugar's defaults are the general shapes' defaults.
    assert_eq!(sugared.parts[0].id, "main");
    assert_eq!(sugared.display_zone, zone);
    assert!(sugared.parts[0].tags.is_empty());
    assert_eq!(sugared.parts[0].travel, Travel::default());
    assert!(sugared.grid.is_none());
    assert!(sugared.candidates.is_none());

    let preset_rules = vec![
        presets::business_hours(
            RuleZone::Named(zone.clone()),
            Weekdays::range(Weekday::Monday, Weekday::Sunday),
            TimeOfDay::new(9, 0).unwrap(),
            WallEnd::Time(TimeOfDay::new(12, 0).unwrap()),
        )
        .unwrap(),
        presets::buffers(Duration::from_minutes(10), Duration::from_minutes(10)),
        presets::min_notice(Duration::from_hours_minutes(4, 0)),
        presets::horizon(LeadTimeMax::Days(3)),
    ];
    let manual_rules = vec![
        Rule::new("business_hours", RuleKind::Window(daily_window(9, 12))),
        Rule::new(
            "buffers",
            RuleKind::Clearance(Clearance {
                before: Duration::from_minutes(10),
                after: Duration::from_minutes(10),
                min_gap: Duration::ZERO,
            }),
        ),
        Rule::new(
            "min_notice",
            RuleKind::LeadTime(LeadTime {
                min: Duration::from_hours_minutes(4, 0),
                max: None,
            }),
        ),
        Rule::new(
            "horizon",
            RuleKind::LeadTime(LeadTime {
                min: Duration::ZERO,
                max: Some(LeadTimeMax::Days(3)),
            }),
        ),
    ];
    assert_eq!(
        preset_rules, manual_rules,
        "a preset builds the domain type"
    );

    let mut from_presets = request_for(search, vec![free_participant("rich")], preset_rules);
    from_presets.goal = sugared;
    let mut from_general = from_presets.clone();
    from_general.goal = general;
    from_general.rules = manual_rules;

    let engine = engine();
    assert_eq!(
        engine.find_availability(&from_presets).unwrap(),
        engine.find_availability(&from_general).unwrap(),
        "the sugar and presets add no semantics"
    );
}

#[test]
fn offer_accessors_read_the_general_shapes() {
    let mut request = request_at(
        minutes(9 * 60),
        free_participant("rich"),
        vec![window_rule("hours", daily_window(8, 12))],
    );
    request.goal.parts[0].travel = Travel {
        before: Duration::from_minutes(15),
        after: Duration::from_minutes(0),
        by_participant: BTreeMap::new(),
    };
    let response = engine().find_availability(&request).unwrap();
    let offer: &Offer = &response.offers[0];
    assert_eq!(offer.start(), Some(minutes(9 * 60)));
    assert_eq!(offer.end(), Some(minutes(9 * 60 + 30)));
    assert_eq!(offer.assigned_hosts(), ["rich"]);
    assert_eq!(
        offer.events[0].reserve[0].span,
        span(minutes(9 * 60 - 15), minutes(9 * 60))
    );
    assert!(!offer.calendar_unknown());
}

#[test]
fn an_unknown_zone_proposal_target_does_not_hide_a_request_validation_error() {
    // `check_slot` answers with the request's validation errors first, exactly as
    // `find_availability` does, so a caller never chases a proposal shape the
    // request cannot describe.
    let mut request = request_at(minutes(9 * 60), free_participant("rich"), Vec::new());
    request.goal.zone = name("Mars/Olympus");
    let err = engine()
        .check_slot(&request, &proposal(minutes(9 * 60), &["rich"]))
        .unwrap_err();
    assert_eq!(err.code(), "invalid_request");
}

#[test]
fn a_booking_that_ends_at_the_last_supported_instant_confirms_and_one_minute_later_does_not() {
    // `Instant::MAX` is an instant a `Span` can end at, so a 30-minute booking may
    // end exactly there. The supported range ending is not a rule the caller broke.
    let last = after(Instant::MAX, -30);
    let request = request_at(last, free_participant("rich"), Vec::new());
    let check = engine()
        .check_slot(&request, &proposal(last, &["rich"]))
        .unwrap();
    assert!(check.valid, "{:?}", reason_codes(&check.reasons));
    assert!(check.reasons.is_empty());

    // Travel before the booking is reserved, and the span it holds is the minutes
    // that exist rather than a phantom one past the range.
    let mut travelled = request.clone();
    travelled.goal.parts[0].travel = Travel {
        before: Duration::from_minutes(10),
        after: Duration::ZERO,
        by_participant: BTreeMap::new(),
    };
    let check = engine()
        .check_slot(&travelled, &proposal(last, &["rich"]))
        .unwrap();
    assert!(check.valid, "{:?}", reason_codes(&check.reasons));
    assert_eq!(check.reserve[0].span, span(after(last, -10), last));

    // One minute later the booking's end is past the last supported instant, so no
    // `Span` can name the reservation a caller would persist.
    let past = after(Instant::MAX, -29);
    let request = request_at(past, free_participant("rich"), Vec::new());
    let err = engine()
        .check_slot(&request, &proposal(past, &["rich"]))
        .expect_err("the booking does not end at a representable instant");
    assert_eq!(err.code(), "proposal_out_of_range");
    assert_eq!(err.path(), Some("/events/0/start"));

    // A blackout covering the last supported minute still excludes a booking that
    // can be represented.
    let closed = BlackoutSpec::new(
        RuleZone::Named(name("UTC")),
        vec![
            BlackoutSpan::new(
                LocalDateTime::new(
                    Date::new(2099, 12, 31).unwrap(),
                    TimeOfDay::new(23, 59).unwrap(),
                ),
                LocalDateTime::new(Date::new(2100, 1, 1).unwrap(), TimeOfDay::MIDNIGHT),
            )
            .unwrap(),
        ],
    );
    let request = request_at(
        last,
        free_participant("rich"),
        vec![blackout_rule("closed", closed)],
    );
    let check = engine()
        .check_slot(&request, &proposal(last, &["rich"]))
        .unwrap();
    assert!(!check.valid);
    assert_eq!(reason_codes(&check.reasons), ["blackout"]);
    assert_eq!(check.reasons[0].rule.as_deref(), Some("closed"));
}

/// The last supported minute is inside the range every window is read over, so a
/// window that covers the travel reaching it covers both. Before that minute was
/// judged, an all-night window still reported the travel as outside itself.
#[test]
fn a_window_covering_the_last_minute_covers_the_travel_that_reaches_it() {
    // Open 23:00 to 01:00. The booking ends thirty minutes before the range does,
    // and the travel after it stops exactly at the last supported instant.
    let start = after(Instant::MAX, -60);
    let mut request = request_at(
        start,
        free_participant("rich"),
        // Open 23:00 to 01:00, so it covers both the booking and the travel after
        // it across the last midnight of the supported range.
        vec![window_rule("evening", daily_window(23, 1))],
    );
    request.goal.parts[0].travel = Travel {
        before: Duration::ZERO,
        after: Duration::from_minutes(30),
        by_participant: BTreeMap::new(),
    };

    let check = engine()
        .check_slot(&request, &proposal(start, &["rich"]))
        .unwrap();
    assert!(check.valid, "{:?}", reason_codes(&check.reasons));
    assert!(check.reasons.is_empty());
    assert_eq!(check.reserve.len(), 1);
    assert_eq!(check.reserve[0].span, span(after(start, 30), Instant::MAX));
}

/// A window that closes at midnight covers the last supported minute, which begins
/// at 23:59, and one that closes earlier excludes the booking ending at the range's
/// end.
#[test]
fn a_window_that_closes_at_midnight_covers_the_last_supported_minute() {
    // Open 08:00 to the given end every day.
    let days = |end| {
        WindowSpec::new(
            RuleZone::Named(name("UTC")),
            vec![
                WeeklyKey::new(
                    Weekdays::range(Weekday::Monday, Weekday::Sunday),
                    vec![WallRange::new(TimeOfDay::new(8, 0).unwrap(), end).unwrap()],
                )
                .unwrap(),
            ],
            Vec::new(),
        )
        .unwrap()
    };

    // The booking's own minute is the last one, and the window covers it.
    let last = after(Instant::MAX, -1);
    let mut request = request_at(
        last,
        free_participant("rich"),
        vec![window_rule("days", days(WallEnd::Midnight))],
    );
    request.goal.parts[0].duration = Duration::from_minutes(1);
    let check = engine()
        .check_slot(&request, &proposal(last, &["rich"]))
        .unwrap();
    assert!(check.valid, "{:?}", reason_codes(&check.reasons));

    // A window that closes at 23:00 does not cover the 23:30 booking, which still
    // ends at the last supported instant.
    let start = after(Instant::MAX, -30);
    let request = request_at(
        start,
        free_participant("rich"),
        vec![window_rule(
            "days",
            days(WallEnd::Time(TimeOfDay::new(23, 0).unwrap())),
        )],
    );
    let check = engine()
        .check_slot(&request, &proposal(start, &["rich"]))
        .unwrap();
    assert!(!check.valid, "{:?}", reason_codes(&check.reasons));
    assert_eq!(reason_codes(&check.reasons), ["outside_window"]);
    assert_eq!(check.reasons[0].rule.as_deref(), Some("days"));
}

/// An instant from a UTC civil date-time.
fn utc_at(year: i16, month: i8, day: i8, hour: u8, minute: u8) -> Instant {
    let date = Date::new(year, month, day).unwrap();
    let time = TimeOfDay::new(hour, minute).unwrap();
    let LocalResolution::Unique(instant) = Zone::utc().resolve_local(date, time).unwrap() else {
        panic!("UTC has no transitions");
    };
    instant
}

/// `count` minutes after `base`.
fn after(base: Instant, count: i64) -> Instant {
    Instant::from_minutes(base.minutes() + count).unwrap()
}

/// A window open every day from `from` until `until`, read in `zone`. A range
/// whose end is not later than its start crosses local midnight.
fn daily_window_in(zone: RuleZone, from: (u8, u8), until: (u8, u8)) -> WindowSpec {
    let at = |(hour, minute): (u8, u8)| TimeOfDay::new(hour, minute).unwrap();
    let hours = WallRange::new(at(from), WallEnd::Time(at(until))).unwrap();
    WindowSpec::new(
        zone,
        vec![
            WeeklyKey::new(
                Weekdays::range(Weekday::Monday, Weekday::Sunday),
                vec![hours],
            )
            .unwrap(),
        ],
        Vec::new(),
    )
    .unwrap()
}

/// A blackout over one local date in the participant's own zone.
fn participant_blackout(day: Date, from: (u8, u8), until: (u8, u8)) -> BlackoutSpec {
    let at =
        |(hour, minute): (u8, u8)| LocalDateTime::new(day, TimeOfDay::new(hour, minute).unwrap());
    BlackoutSpec::new(
        RuleZone::Participant,
        vec![BlackoutSpan::new(at(from), at(until)).unwrap()],
    )
}

/// One sweep scenario: a participant at home in `zone` who travels to another
/// zone for the middle day, a window that crosses local midnight in each of the
/// two zone readings, a blackout and a meeting with its own travel, over three
/// days from `base`.
fn zone_trip_request(zone: &str, base: Instant, index: usize) -> Request {
    let home = name(zone);
    let mut participant = Participant::new("rich", home.clone());
    let mut meeting = block(after(base, 21 * 60 + 45), after(base, 22 * 60 + 15));
    meeting.travel_before = Duration::from_minutes(10);
    meeting.travel_after = Duration::from_minutes(10);
    meeting.event_key = Some(format!("evt_{zone}_{index}"));
    participant.calendar = Some(vec![meeting]);
    // A trip that moves the participant to another zone for the middle day, so a
    // window read in the participant's zone moves while one read in the goal's
    // zone does not.
    participant.zone_periods.push(
        ZonePeriod::new(
            name("America/Santiago"),
            after(base, 24 * 60),
            after(base, 2 * 24 * 60),
        )
        .unwrap(),
    );

    let part = Part::new(
        Duration::from_minutes(45),
        Requirement::new(vec![Term::person("rich")]),
    );
    let mut request = Request::new(
        after(base, 6 * 60),
        Goal::new(home, span(base, after(base, 3 * 24 * 60)), vec![part]),
    );
    request.participants = vec![participant];
    request.goal.parts[0].travel = Travel {
        before: Duration::from_minutes(25),
        after: Duration::from_minutes(35),
        by_participant: BTreeMap::new(),
    };
    let blackout_day = Zone::utc().zoned(base).date();
    request.rules = vec![
        // Crosses local midnight in the participant's own zone, so the trip and
        // the transition both move it.
        Rule::new(
            "night",
            RuleKind::Window(daily_window_in(RuleZone::Participant, (20, 0), (2, 0))),
        ),
        // Intersects the first, so the scenario still offers something, and
        // narrows it, so a start can fail either one.
        Rule::new(
            "hours",
            RuleKind::Window(daily_window_in(
                RuleZone::Named(name(zone)),
                (21, 0),
                (1, 30),
            )),
        ),
        Rule::new(
            "freeze",
            RuleKind::Blackout(participant_blackout(blackout_day, (22, 30), (23, 0))),
        ),
        presets::buffers(Duration::from_minutes(20), Duration::from_minutes(15)),
        presets::min_notice(Duration::from_minutes(90)),
        presets::horizon(LeadTimeMax::Days(1)),
        // Protects the morning after any activity running past 18:00, which the
        // evening meeting reaches in every zone.
        presets::rest(
            TimeOfDay::new(18, 0).unwrap(),
            TimeOfDay::new(10, 0).unwrap(),
        ),
        // One meeting a day: starts on the day the evening meeting falls on are
        // past the cap, so the sweep reaches `cap_reached` alongside the rules
        // that exclude other starts first.
        Rule::new(
            "load",
            RuleKind::Cap(Cap::new(
                Selector::True,
                CapMeasure::Count,
                1,
                CapPeriod::Day,
                CapPer::Each,
                RuleZone::Participant,
                Weekday::Monday,
            )),
        ),
    ];
    request.explain = Some(Explain::new(10_000));
    request
}

/// Confirms every start the search judged, recording in `seen` the first reason
/// code each exclusion gave, and answers with how many offers and exclusions
/// were confirmed.
fn confirm_every_start(
    engine: &Engine,
    request: &Request,
    response: &opportune::model::Response,
    label: &str,
    seen: &mut Vec<&'static str>,
) -> (usize, usize) {
    let mut offers = 0;
    for offer in &response.offers {
        let event = &offer.events[0];
        let check = engine
            .check_slot(
                request,
                &Proposal {
                    events: vec![ProposalEvent {
                        part: event.part.clone(),
                        start: event.start,
                        assignment: event.assignment.clone(),
                    }],
                },
            )
            .unwrap();
        assert!(
            check.valid,
            "{label}: {} was offered but cannot be confirmed: {:?}",
            event.start,
            reason_codes(&check.reasons)
        );
        assert_eq!(check.flags, offer.flags, "{label}: {}", event.start);
        assert_eq!(check.reserve, event.reserve, "{label}: {}", event.start);
        offers += 1;
    }

    let mut exclusions = 0;
    for excluded in &response.excluded {
        let check = engine
            .check_slot(request, &proposal(excluded.start, &["rich"]))
            .unwrap();
        assert!(
            !check.valid,
            "{label}: {} was excluded but confirms",
            excluded.start
        );
        let explained = excluded.reasons.first().expect("an exclusion has a reason");
        let confirmed = check.reasons.first().expect("an invalid slot has a reason");
        assert_eq!(
            (confirmed.code, confirmed.rule.as_deref()),
            (explained.code, explained.rule.as_deref()),
            "{label}: the search and the confirmation judge {} differently",
            excluded.start
        );
        seen.push(explained.code.as_str());
        exclusions += 1;
    }
    (offers, exclusions)
}

#[test]
fn check_slot_agrees_with_the_search_across_zones_trips_and_midnight_windows() {
    /// The reason codes an exclusion carries about one participant; the sweep's
    /// request has one host in one term, so it cannot reach a start-level code.
    const START_LEVEL: [ReasonCode; 2] =
        [ReasonCode::AttendeeUnavailable, ReasonCode::TermUnfillable];
    // A search builds allowed and blocked time over its padded window; a
    // confirmation builds them over the booking and its travel alone. Where the
    // participant's zone changes mid-search, a window crosses local midnight and
    // a daylight saving transition falls inside the range, the narrower range
    // must restrict the interval set rather than reshape it: the two answers have
    // to agree on every start either of them judges.
    let zones = [
        "Europe/London",
        "America/New_York",
        "Australia/Lord_Howe",
        "Pacific/Kiritimati",
        "Asia/Kolkata",
    ];
    let bases = [
        utc_at(2027, 3, 14, 0, 0),
        utc_at(2027, 11, 7, 0, 0),
        utc_at(2027, 4, 4, 0, 0),
        utc_at(2011, 12, 29, 0, 0),
    ];
    let engine = engine();
    let mut seen: Vec<&'static str> = Vec::new();
    let mut confirmed_offers = 0_usize;
    let mut confirmed_exclusions = 0_usize;

    for zone in zones {
        for (index, base) in bases.iter().enumerate() {
            let request = zone_trip_request(zone, *base, index);
            let response = engine.find_availability(&request).unwrap();
            let label = format!("{zone} at {base}");
            let (offers, exclusions) =
                confirm_every_start(&engine, &request, &response, &label, &mut seen);
            confirmed_offers += offers;
            confirmed_exclusions += exclusions;
        }
    }

    // The sweep is only worth running if both sides of the agreement were
    // actually exercised, and if it reached every code this version produces: a
    // scenario that offered and excluded nothing would pass vacuously.
    assert!(
        confirmed_offers > 0 && confirmed_exclusions > 0,
        "the sweep confirmed {confirmed_offers} offer(s) and {confirmed_exclusions} exclusion(s)"
    );
    // Every participant-level code this version produces.
    for code in ReasonCode::ALL
        .into_iter()
        .filter(|code| !START_LEVEL.contains(code))
    {
        assert!(
            seen.contains(&code.as_str()),
            "the sweep never exercised `{}`: {seen:?}",
            code.as_str()
        );
    }
}

#[test]
fn clearance_cites_the_rule_that_supplied_each_gap_side() {
    // Two clearance rules, each supplying one side of the combined gap. A booking
    // after an event is too close because of the `before` gap, and one before an
    // event because of the `after` gap, so each citation names its own rule
    // rather than whichever rule happens to sit first in the request.
    let rules = vec![
        Rule::new(
            "spaces",
            RuleKind::Clearance(Clearance {
                before: Duration::from_minutes(10),
                after: Duration::ZERO,
                min_gap: Duration::ZERO,
            }),
        ),
        Rule::new(
            "wakes",
            RuleKind::Clearance(Clearance {
                before: Duration::ZERO,
                after: Duration::from_minutes(30),
                min_gap: Duration::ZERO,
            }),
        ),
    ];

    // A booking after the 09:00–10:00 event: the block's end widened by the
    // `before` gap (10 minutes) reaches 10:10 and catches a 10:05 start.
    let mut participant = free_participant("rich");
    participant.calendar = Some(vec![block(minutes(9 * 60), minutes(10 * 60))]);
    let mut request = request_at(minutes(10 * 60 + 5), participant, rules.clone());
    request.explain = Some(Explain::new(10));
    let response = engine().find_availability(&request).unwrap();
    let reason = &response.excluded[0].reasons[0];
    assert_eq!(reason.code, ReasonCode::Clearance);
    assert_eq!(
        reason.rule.as_deref(),
        Some("spaces"),
        "the booking after the event is caught by the `before` gap it requires"
    );

    // A booking before the 11:00–12:00 event: the block's start widened back by
    // the `after` gap (30 minutes) reaches 10:30 and catches a 10:25 start.
    let mut participant = free_participant("rich");
    participant.calendar = Some(vec![block(minutes(11 * 60), minutes(12 * 60))]);
    let mut request = request_at(minutes(10 * 60 + 25), participant, rules);
    request.explain = Some(Explain::new(10));
    let response = engine().find_availability(&request).unwrap();
    let reason = &response.excluded[0].reasons[0];
    assert_eq!(reason.code, ReasonCode::Clearance);
    assert_eq!(
        reason.rule.as_deref(),
        Some("wakes"),
        "the booking before the event is caught by the `after` gap it requires"
    );
}
