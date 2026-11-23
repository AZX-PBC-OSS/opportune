//! Adversarial probes for structured reasons, `explain`, `check_slot` and the
//! single-part sugar: the assumptions a caller relies on that the behavioural
//! suite does not pin.
//!
//! Each test states the assumption it attacks in its name and asserts the answer
//! a caller acts on, so deleting the behaviour it covers turns it red.

#![cfg(feature = "bundled-tzdb")]

use opportune::Engine;
use opportune::model::{
    Block, Clearance, Explain, Goal, LeadTimeMax, Part, Participant, Proposal, ProposalEvent,
    Request, Requirement, Rule, RuleKind, Span, Term,
};
use opportune::presets;
use opportune::tags::{Role, Selector, Tag};
use opportune::time::{
    BlackoutSpan, BlackoutSpec, Date, Duration, Instant, LocalDateTime, RuleZone, TimeOfDay,
    WallEnd, WallRange, Weekday, Weekdays, WeeklyKey, WindowSpec, ZoneName,
};

fn name(zone: &str) -> ZoneName {
    ZoneName::new(zone).unwrap()
}

/// `count` minutes after the earliest supported instant, `1975-01-01T00:00Z`.
fn minutes(count: i64) -> Instant {
    Instant::from_minutes(Instant::MIN.minutes() + count).unwrap()
}

fn span(start: Instant, end: Instant) -> Span {
    Span::new(start, end).unwrap()
}

fn block(start: Instant, end: Instant) -> Block {
    Block::new(span(start, end))
}

fn engine() -> Engine {
    Engine::builder().build().unwrap()
}

fn free_participant(id: &str) -> Participant {
    let mut participant = Participant::new(id, name("UTC"));
    participant.calendar = Some(Vec::new());
    participant
}

/// One 30-minute part filled by any host, over the given search window.
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

fn proposal(start: Instant, assignment: &[&str]) -> Proposal {
    Proposal {
        events: vec![ProposalEvent {
            part: "main".to_owned(),
            start,
            assignment: vec![assignment.iter().map(|id| (*id).to_owned()).collect()],
        }],
    }
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

fn offer_starts(response: &opportune::model::Response) -> Vec<Instant> {
    response
        .offers
        .iter()
        .map(|offer| offer.start().unwrap())
        .collect()
}

// -------------------------------------------------- explain only explains ---

#[test]
fn an_explanation_limit_does_not_hide_offers() {
    // 09:00 is free, 10:00 is blocked for both hosts (two reasons, more than the
    // explanation limit allows), and 11:00 is free again. Asking for fewer
    // reasons than the blocked start needs must truncate the *explanation*; the
    // offers a caller books from are not the explanation's to cut.
    let mut ana = free_participant("ana");
    ana.calendar = Some(vec![block(minutes(10 * 60), minutes(11 * 60))]);
    let mut tom = free_participant("tom");
    tom.calendar = Some(vec![block(minutes(10 * 60), minutes(11 * 60))]);

    let mut request = request_for(
        span(minutes(9 * 60), minutes(12 * 60)),
        vec![ana, tom],
        Vec::new(),
    );
    request.goal.candidates = Some(vec![minutes(9 * 60), minutes(10 * 60), minutes(11 * 60)]);

    let without_explanation = engine().find_availability(&request).unwrap();
    assert_eq!(
        offer_starts(&without_explanation),
        [minutes(9 * 60), minutes(11 * 60)],
        "both free starts are offered when nothing is explained"
    );

    request.explain = Some(Explain::new(1));
    let explained = engine().find_availability(&request).unwrap();
    assert!(
        explained.excluded_truncated,
        "two reasons do not fit a limit of one"
    );
    assert!(
        explained.excluded.is_empty(),
        "the blocked start's entry is atomic and does not fit, so it is left out"
    );
    assert_eq!(
        offer_starts(&explained),
        [minutes(9 * 60), minutes(11 * 60)],
        "an explanation limit must not change which starts are offered"
    );
    assert_eq!(
        explained.dates[0].offers, 2,
        "date counts are computed from the offers, not from the explanation"
    );
}

// ------------------------------------------------------------ rule citation ---

#[test]
fn a_zero_notice_rule_does_not_answer_for_the_minimum_notice() {
    // `presets::horizon` builds `lead_time { min: 0, max: ... }`. The zero it
    // supplies is the implicit minimum every request already has, so it cannot
    // be the rule behind a `lead_time_min` exclusion: the exclusion is `now`.
    let mut request = request_for(
        span(minutes(0), minutes(24 * 60)),
        vec![free_participant("rich")],
        vec![presets::horizon(LeadTimeMax::Days(3))],
    );
    request.now = minutes(10 * 60);
    request.goal.candidates = Some(vec![minutes(9 * 60)]);
    request.explain = Some(Explain::new(10));

    let response = engine().find_availability(&request).unwrap();
    let [excluded] = response.excluded.as_slice() else {
        panic!("one excluded start expected, got {:?}", response.excluded);
    };
    let [reason] = excluded.reasons.as_slice() else {
        panic!("one reason expected, got {:?}", excluded.reasons);
    };
    assert_eq!(reason.code.as_str(), "lead_time_min");
    assert_eq!(
        reason.rule, None,
        "a rule that asks for no notice did not exclude the start"
    );
}

#[test]
fn a_clearance_tie_cites_the_earliest_rule_in_request_order() {
    // The combined `after` gap is `max(after, min_gap)` = 10, supplied by both
    // rules. The least-gap rule comes first in request order, so it is the one
    // the reason cites.
    let mut participant = free_participant("rich");
    participant.calendar = Some(vec![block(minutes(10 * 60), minutes(11 * 60))]);

    let mut request = request_for(
        span(minutes(0), minutes(24 * 60)),
        vec![participant],
        vec![
            Rule::new(
                "least",
                RuleKind::Clearance(Clearance {
                    before: Duration::ZERO,
                    after: Duration::ZERO,
                    min_gap: Duration::from_minutes(10),
                }),
            ),
            Rule::new(
                "gap",
                RuleKind::Clearance(Clearance {
                    before: Duration::ZERO,
                    after: Duration::from_minutes(10),
                    min_gap: Duration::ZERO,
                }),
            ),
        ],
    );
    // The booking ends five minutes before the event: inside the ten-minute gap,
    // clear of the event itself.
    request.goal.candidates = Some(vec![minutes(9 * 60 + 25)]);
    request.explain = Some(Explain::new(10));

    let response = engine().find_availability(&request).unwrap();
    let [excluded] = response.excluded.as_slice() else {
        panic!("one excluded start expected, got {:?}", response.excluded);
    };
    let [reason] = excluded.reasons.as_slice() else {
        panic!("one reason expected, got {:?}", excluded.reasons);
    };
    assert_eq!(reason.code.as_str(), "clearance");
    assert_eq!(
        reason.rule.as_deref(),
        Some("least"),
        "both rules supplied the ten-minute gap; the earliest in request order is cited"
    );
}

/// Every calendar the same two events arrive in must produce the same answer, so
/// a citation cannot depend on the order a caller listed their calendar in.
///
/// Two events are in the way of one booking: the one after it needs the gap the
/// booking owes a following event (the `after` gap), and the one before it the gap
/// it owes a preceding one (the `before` gap). Those gaps come from two different
/// clearance rules, and the reason must cite the earlier rule in request order —
/// not whichever event happened to come first on the calendar.
#[test]
fn clearance_cites_the_earliest_rule_across_competing_events() {
    let event = |start: i64, end: i64, key: &str| {
        let mut block = block(minutes(start), minutes(end));
        block.event_key = Some(key.to_owned());
        block
    };
    // The booking 11:45-12:15 is too close to both: the 10:00-11:00 event needs an
    // hour after it, and the 12:30-13:30 one an hour before it.
    let preceding = event(10 * 60, 11 * 60, "evt_before");
    let following = event(12 * 60 + 30, 13 * 60 + 30, "evt_after");
    let rules = || {
        vec![
            Rule::new(
                "buffer-after",
                RuleKind::Clearance(Clearance {
                    before: Duration::ZERO,
                    after: Duration::from_minutes(60),
                    min_gap: Duration::ZERO,
                }),
            ),
            Rule::new(
                "buffer-before",
                RuleKind::Clearance(Clearance {
                    before: Duration::from_minutes(60),
                    after: Duration::ZERO,
                    min_gap: Duration::ZERO,
                }),
            ),
        ]
    };
    let cited = |calendar: Vec<Block>| {
        let mut participant = free_participant("rich");
        participant.calendar = Some(calendar);
        let mut request = request_for(
            span(minutes(0), minutes(24 * 60)),
            vec![participant],
            rules(),
        );
        request.goal.candidates = Some(vec![minutes(11 * 60 + 45)]);
        request.explain = Some(Explain::new(10));
        let response = engine().find_availability(&request).unwrap();
        let reason = response.excluded[0].reasons[0].clone();
        assert_eq!(reason.code.as_str(), "clearance");
        (reason.rule, reason.event_key)
    };

    let calendar_order = cited(vec![preceding.clone(), following.clone()]);
    assert_eq!(
        calendar_order.0.as_deref(),
        Some("buffer-after"),
        "the earliest rule in request order supplied the gap the booking crossed"
    );
    let reversed = cited(vec![following, preceding]);
    assert_eq!(
        reversed, calendar_order,
        "the order of the calendar does not decide which rule or event is cited"
    );
}

/// Two events can be in the way of one booking through the *same* clearance rule,
/// so the request order cannot separate them: the tie is broken canonically, on the
/// events themselves, and never on the order the caller listed their calendar in.
#[test]
fn a_clearance_tie_between_two_events_of_one_rule_is_broken_canonically() {
    let event = |start: i64, end: i64, key: &str| {
        let mut block = block(minutes(start), minutes(end));
        block.event_key = Some(key.to_owned());
        block
    };
    // The booking 11:45-12:15 is too close to both, and one rule supplies both
    // gaps, so both conflicts cite it at the same position in request order.
    let preceding = event(10 * 60, 11 * 60, "evt_before");
    let following = event(12 * 60 + 30, 13 * 60 + 30, "evt_after");
    let cited = |calendar: Vec<Block>| {
        let mut participant = free_participant("rich");
        participant.calendar = Some(calendar);
        let mut request = request_for(
            span(minutes(0), minutes(24 * 60)),
            vec![participant],
            vec![Rule::new(
                "buffer",
                RuleKind::Clearance(Clearance {
                    before: Duration::from_minutes(60),
                    after: Duration::from_minutes(60),
                    min_gap: Duration::ZERO,
                }),
            )],
        );
        request.goal.candidates = Some(vec![minutes(11 * 60 + 45)]);
        request.explain = Some(Explain::new(10));
        let response = engine().find_availability(&request).unwrap();
        let reason = response.excluded[0].reasons[0].clone();
        assert_eq!(reason.code.as_str(), "clearance");
        (reason.rule, reason.event_key)
    };

    // The earlier event occupies the earlier span, so it is the canonical citation.
    let expected = (Some("buffer".to_owned()), Some("evt_before".to_owned()));
    assert_eq!(
        cited(vec![preceding.clone(), following.clone()]),
        expected,
        "one rule supplying both gaps still cites one event deterministically"
    );
    assert_eq!(
        cited(vec![following, preceding]),
        expected,
        "reversing the calendar does not move the citation"
    );
}

/// Two events overlapping one booking both block it; which one the reason names
/// is decided by the events themselves, not by the order the caller listed them.
#[test]
fn a_blocked_reason_cites_the_same_event_whatever_the_calendar_order() {
    let event = |start: i64, end: i64, key: &str| {
        let mut block = block(minutes(start), minutes(end));
        block.event_key = Some(key.to_owned());
        block
    };
    let cited = |calendar: Vec<Block>| {
        let mut participant = free_participant("rich");
        participant.calendar = Some(calendar);
        let mut request = request_for(
            span(minutes(0), minutes(24 * 60)),
            vec![participant],
            Vec::new(),
        );
        request.goal.candidates = Some(vec![minutes(10 * 60 + 45)]);
        request.explain = Some(Explain::new(10));
        let reason = engine().find_availability(&request).unwrap().excluded[0].reasons[0].clone();
        assert_eq!(reason.code.as_str(), "blocked");
        reason.event_key
    };

    let named = cited(vec![
        event(10 * 60, 11 * 60, "evt_earlier"),
        event(10 * 60 + 30, 11 * 60 + 30, "evt_later"),
    ]);
    assert_eq!(named.as_deref(), Some("evt_earlier"));
    assert_eq!(
        cited(vec![
            event(10 * 60 + 30, 11 * 60 + 30, "evt_later"),
            event(10 * 60, 11 * 60, "evt_earlier"),
        ]),
        named,
        "reversing the calendar cites the same event"
    );
}

/// Two events occupying the same minutes are ordered by what a reason reports of
/// them — the event key, then the tags — so neither level of the tie-break falls
/// back to the order the caller listed their calendar in. A free/busy source
/// carries no event key at all, which is exactly the case two blocks share.
#[test]
fn a_blocked_reason_breaks_ties_on_the_event_key_and_then_the_tags() {
    let same_span = |key: Option<&str>, tags: &[&str]| {
        let mut block = block(minutes(10 * 60), minutes(11 * 60));
        block.event_key = key.map(str::to_owned);
        block.tags = tags
            .iter()
            .map(|path| Tag::new(path).expect("a tag path is valid"))
            .collect();
        block
    };
    // No key on either of the first two, so the tags decide between them; the
    // third carries a key, which outranks both.
    let untagged_keyed = same_span(Some("evt"), &[]);
    let freeze = same_span(None, &["veto/crm-freeze"]);
    let busy = same_span(None, &["busy"]);

    let cited = |calendar: Vec<Block>| {
        let mut participant = free_participant("rich");
        participant.calendar = Some(calendar);
        let mut request = request_for(
            span(minutes(0), minutes(24 * 60)),
            vec![participant],
            Vec::new(),
        );
        request.goal.candidates = Some(vec![minutes(10 * 60 + 45)]);
        request.explain = Some(Explain::new(10));
        let reason = engine().find_availability(&request).unwrap().excluded[0].reasons[0].clone();
        assert_eq!(reason.code.as_str(), "blocked");
        (reason.event_key, reason.tags)
    };

    let expected = (None, vec![Tag::new("busy").unwrap()]);
    for calendar in [
        vec![freeze.clone(), busy.clone(), untagged_keyed.clone()],
        vec![busy.clone(), freeze.clone(), untagged_keyed.clone()],
        vec![untagged_keyed.clone(), freeze.clone(), busy.clone()],
        vec![untagged_keyed.clone(), busy.clone(), freeze.clone()],
        vec![freeze.clone(), untagged_keyed.clone(), busy.clone()],
        vec![busy.clone(), untagged_keyed.clone(), freeze.clone()],
    ] {
        let keys: Vec<Option<String>> = calendar
            .iter()
            .map(|block| block.event_key.clone())
            .collect();
        assert_eq!(cited(calendar), expected, "calendar {keys:?}");
    }
}

// ------------------------------------------------- explanation / check_slot ---

#[test]
fn check_slot_agrees_with_the_explanation_for_every_excluded_start() {
    // The explanation's first reason and a confirmation's first reason come from
    // one decision, so they cannot disagree about the same start.
    let mut participant = free_participant("rich");
    participant.calendar = Some(vec![block(minutes(10 * 60), minutes(11 * 60))]);
    let mut request = request_for(
        span(minutes(9 * 60), minutes(14 * 60)),
        vec![participant],
        vec![
            Rule::new("hours", RuleKind::Window(daily_window(10, 12))),
            Rule::new("freeze", RuleKind::Blackout(utc_blackout((11, 0), (12, 0)))),
        ],
    );
    request.explain = Some(Explain::new(1_000));

    let response = engine().find_availability(&request).unwrap();
    assert!(!response.excluded.is_empty(), "starts are excluded");
    for excluded in &response.excluded {
        let check = engine()
            .check_slot(&request, &proposal(excluded.start, &["rich"]))
            .unwrap();
        assert!(
            !check.valid,
            "{} was excluded and cannot be confirmed",
            excluded.start
        );
        assert_eq!(
            check.reasons.first().map(|reason| reason.code),
            excluded.reasons.first().map(|reason| reason.code),
            "the explanation and the confirmation judge {} the same way",
            excluded.start
        );
        assert_eq!(
            check
                .reasons
                .first()
                .and_then(|reason| reason.rule.as_deref()),
            excluded
                .reasons
                .first()
                .and_then(|reason| reason.rule.as_deref()),
        );
    }
}

#[test]
fn check_slot_agrees_with_the_explanation_where_travel_and_clearance_judge_ranges_differ() {
    // A search builds allowed and blocked time over its padded window; a
    // confirmation builds them over the booking's own span plus travel. Start
    // eligibility and the cited rule must not depend on which range judged it.
    let mut participant = free_participant("rich");
    participant.calendar = Some(vec![block(minutes(10 * 60), minutes(11 * 60))]);
    let mut request = request_for(
        span(minutes(9 * 60), minutes(13 * 60)),
        vec![participant],
        vec![
            Rule::new("hours", RuleKind::Window(daily_window(9, 13))),
            presets::buffers(Duration::from_minutes(10), Duration::from_minutes(10)),
        ],
    );
    request.goal.parts[0].travel = opportune::model::Travel {
        before: Duration::from_minutes(20),
        after: Duration::from_minutes(20),
        by_participant: std::collections::BTreeMap::new(),
    };
    // Starts chosen so the three reasons this range difference can move all
    // appear: travel outside the window, travel over the event, the event
    // itself, and a booking that is only too close to it.
    request.goal.candidates = Some(vec![
        minutes(9 * 60),
        minutes(9 * 60 + 30),
        minutes(10 * 60),
        minutes(11 * 60 + 20),
        minutes(12 * 60),
    ]);
    request.explain = Some(Explain::new(1_000));

    let response = engine().find_availability(&request).unwrap();
    let mut seen: Vec<&str> = Vec::new();
    for excluded in &response.excluded {
        let check = engine()
            .check_slot(&request, &proposal(excluded.start, &["rich"]))
            .unwrap();
        assert!(
            !check.valid,
            "{} was excluded and cannot be confirmed",
            excluded.start
        );
        let explained = excluded
            .reasons
            .first()
            .expect("an excluded start has a reason");
        let confirmed = check.reasons.first().expect("an invalid slot has a reason");
        assert_eq!(
            (confirmed.code, confirmed.rule.as_deref()),
            (explained.code, explained.rule.as_deref()),
            "both judge {} the same way",
            excluded.start
        );
        seen.push(explained.code.as_str());
    }
    for code in ["blocked", "travel", "clearance"] {
        assert!(
            seen.contains(&code),
            "the probe exercises `{code}`: {seen:?}"
        );
    }
}

#[test]
fn check_slot_reports_every_reason_even_when_explain_is_set() {
    // `explain` governs a search's explanation only; a confirmation always
    // learns everything standing in the way.
    let mut participant = free_participant("rich");
    participant.calendar = Some(vec![block(minutes(9 * 60), minutes(10 * 60))]);
    let mut request = request_for(
        span(minutes(0), minutes(24 * 60)),
        vec![participant],
        vec![Rule::new("hours", RuleKind::Window(daily_window(10, 12)))],
    );
    request.now = minutes(10 * 60);
    request.goal.candidates = Some(vec![minutes(9 * 60)]);
    request.explain = Some(Explain::new(1));

    let check = engine()
        .check_slot(&request, &proposal(minutes(9 * 60), &["rich"]))
        .unwrap();
    let codes: Vec<&str> = check
        .reasons
        .iter()
        .map(|reason| reason.code.as_str())
        .collect();
    assert_eq!(codes, ["lead_time_min", "outside_window", "blocked"]);
}

#[test]
fn a_free_host_keeps_a_start_out_of_the_explanation() {
    // A start offered through one host is bookable, so it belongs in neither
    // `excluded` nor a confirmation's failures for that host.
    let mut ana = free_participant("ana");
    ana.calendar = Some(vec![block(minutes(10 * 60), minutes(11 * 60))]);
    let tom = free_participant("tom");
    let mut request = request_for(
        span(minutes(9 * 60), minutes(12 * 60)),
        vec![ana, tom],
        Vec::new(),
    );
    request.goal.candidates = Some(vec![minutes(10 * 60)]);
    request.explain = Some(Explain::new(10));

    let response = engine().find_availability(&request).unwrap();
    let [offer] = response.offers.as_slice() else {
        panic!("one offer expected, got {:?}", response.offers);
    };
    assert_eq!(offer.assigned_hosts(), ["tom"]);
    assert!(
        response.excluded.is_empty(),
        "the start was offered, so it is not excluded: {:?}",
        response.excluded
    );
    assert!(!response.excluded_truncated);

    let check = engine()
        .check_slot(&request, &proposal(minutes(10 * 60), &["tom"]))
        .unwrap();
    assert!(check.valid);
}

#[test]
fn a_confirmation_answers_for_the_host_it_was_given_not_the_searchs_choice() {
    // The search assigns the first eligible host by ID; a caller may confirm a
    // different eligible one. The flags and the travel to reserve belong to the
    // proposed assignment, so a host with no calendar is flagged even when the
    // offer's chosen host has one.
    let ana = free_participant("ana");
    let tom = Participant::new("tom", name("UTC"));
    let mut request = request_for(
        span(minutes(9 * 60), minutes(12 * 60)),
        vec![ana, tom],
        Vec::new(),
    );
    request.goal.candidates = Some(vec![minutes(9 * 60)]);
    request.goal.parts[0].travel = opportune::model::Travel {
        before: Duration::from_minutes(15),
        after: Duration::ZERO,
        by_participant: std::collections::BTreeMap::from([(
            "tom".to_owned(),
            opportune::model::ParticipantTravel {
                before: Duration::from_minutes(45),
                after: Duration::ZERO,
            },
        )]),
    };

    let response = engine().find_availability(&request).unwrap();
    let [offer] = response.offers.as_slice() else {
        panic!("one offer expected, got {:?}", response.offers);
    };
    assert_eq!(
        offer.assigned_hosts(),
        ["ana"],
        "the first eligible host by ID"
    );
    assert!(offer.flags.is_empty(), "ana has a calendar");

    let check = engine()
        .check_slot(&request, &proposal(minutes(9 * 60), &["tom"]))
        .unwrap();
    assert!(check.valid, "tom is free: {:?}", check.reasons);
    assert_eq!(
        check.flags,
        [opportune::model::OfferFlag::CalendarUnknown],
        "the proposed host has no calendar, so their availability is unknown"
    );
    let [reserve] = check.reserve.as_slice() else {
        panic!("tom's travel is reserved, got {:?}", check.reserve);
    };
    assert_eq!(reserve.participant, "tom");
    assert_eq!(reserve.span, span(minutes(9 * 60 - 45), minutes(9 * 60)));
}
