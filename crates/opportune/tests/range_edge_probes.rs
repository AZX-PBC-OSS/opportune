//! Adversarial probes for the ends of the supported range: a search and a
//! confirmation must agree on every start at both ends, where the judged ranges
//! and the representable minutes differ.
//!
//! Every probe here builds a real request, runs it on a real engine and asserts
//! the answer a caller acts on.

#![cfg(feature = "bundled-tzdb")]

use std::collections::BTreeMap;

use opportune::Engine;
use opportune::model::{
    Block, Explain, Goal, Participant, ParticipantTravel, Proposal, ProposalEvent, RangesQuery,
    Request, Requirement, Rule, RuleKind, Span, Term, Travel,
};
use opportune::presets;
use opportune::reasons::ReasonCode;
use opportune::tags::{Role, Selector};
use opportune::time::{
    BlackoutSpan, BlackoutSpec, Date, Duration, Grid, GridZone, Instant, LocalDateTime, RuleZone,
    TimeOfDay, WallEnd, Weekdays, ZoneName,
};

fn name(zone: &str) -> ZoneName {
    ZoneName::new(zone).unwrap()
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

fn host(id: &str, zone: &str, blocks: Vec<Block>) -> Participant {
    let mut participant = Participant::new(id, name(zone));
    participant.calendar = Some(blocks);
    participant
}

fn blackout(zone: &str, date: (i16, i8, i8), from: (u8, u8), until: (u8, u8)) -> Rule {
    let date = Date::new(date.0, date.1, date.2).unwrap();
    let spec = BlackoutSpec::new(
        RuleZone::Named(name(zone)),
        vec![
            BlackoutSpan::new(
                LocalDateTime::new(date, TimeOfDay::new(from.0, from.1).unwrap()),
                LocalDateTime::new(date, TimeOfDay::new(until.0, until.1).unwrap()),
            )
            .unwrap(),
        ],
    );
    Rule::new("closed", RuleKind::Blackout(spec))
}

/// A request the day before the end of the supported range, or the day after its
/// start: the judged range reaches past the edge either way, so the minutes that
/// exist are judged and the ones that do not are not.
fn edge_request(zone: &str, top: bool) -> Request {
    let (now, search, block, closed, travel) = if top {
        (
            Instant::from_minutes(Instant::MAX.minutes() - 36 * 60).unwrap(),
            span(
                Instant::from_minutes(Instant::MAX.minutes() - 36 * 60).unwrap(),
                Instant::MAX,
            ),
            block(
                Instant::from_minutes(Instant::MAX.minutes() - 6 * 60).unwrap(),
                Instant::from_minutes(Instant::MAX.minutes() - 5 * 60).unwrap(),
            ),
            blackout("UTC", (2099, 12, 31), (17, 0), (18, 0)),
            Travel {
                before: Duration::from_minutes(10),
                after: Duration::from_minutes(10),
                by_participant: BTreeMap::new(),
            },
        )
    } else {
        (
            Instant::MIN,
            span(
                Instant::MIN,
                Instant::from_minutes(Instant::MIN.minutes() + 36 * 60).unwrap(),
            ),
            block(
                Instant::from_minutes(Instant::MIN.minutes() + 5 * 60).unwrap(),
                Instant::from_minutes(Instant::MIN.minutes() + 6 * 60).unwrap(),
            ),
            blackout("UTC", (1975, 1, 1), (7, 0), (8, 0)),
            Travel {
                before: Duration::from_minutes(10),
                after: Duration::from_minutes(10),
                by_participant: BTreeMap::new(),
            },
        )
    };

    let mut goal = Goal::single(
        name("UTC"),
        search,
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("rich")]),
    );
    goal.parts[0].travel = travel;
    let mut request = Request::new(now, goal);
    request.participants = vec![host("rich", zone, vec![block])];
    request.rules = vec![
        presets::business_hours(
            RuleZone::Participant,
            Weekdays::range(
                opportune::time::Weekday::Monday,
                opportune::time::Weekday::Sunday,
            ),
            TimeOfDay::new(8, 0).unwrap(),
            WallEnd::Time(TimeOfDay::new(20, 0).unwrap()),
        )
        .unwrap(),
        presets::buffers(Duration::from_minutes(10), Duration::from_minutes(10)),
        presets::rest(
            TimeOfDay::new(18, 0).unwrap(),
            TimeOfDay::new(10, 0).unwrap(),
        ),
        closed,
    ];
    request.explain = Some(Explain::new(10_000));
    request
}

/// Confirms every offer and every exclusion the search reported, and returns the
/// reason codes it saw so the sweep cannot pass vacuously.
fn confirm_every_start(
    engine: &Engine,
    request: &Request,
    label: &str,
    seen: &mut Vec<&'static str>,
) -> (usize, usize) {
    let response = engine.find_availability(request).unwrap();
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
            check
                .reasons
                .iter()
                .map(|reason| reason.code)
                .collect::<Vec<_>>()
        );
        assert_eq!(check.flags, offer.flags, "{label}: {}", event.start);
        assert_eq!(check.reserve, event.reserve, "{label}: {}", event.start);
        offers += 1;
    }

    let mut exclusions = 0;
    for excluded in &response.excluded {
        let check = engine
            .check_slot(
                request,
                &Proposal {
                    events: vec![ProposalEvent {
                        part: "main".to_owned(),
                        start: excluded.start,
                        assignment: vec![vec!["rich".to_owned()]],
                    }],
                },
            )
            .unwrap();
        assert!(
            !check.valid,
            "{label}: {} was excluded by the search but confirms",
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

/// Runs the edge sweep over several zones at both ends of the range.
fn sweep(top: bool) -> (usize, usize, Vec<&'static str>) {
    let engine = engine();
    let mut seen = Vec::new();
    let (mut offers, mut exclusions) = (0, 0);
    for zone in [
        "UTC",
        "America/New_York",
        "Pacific/Kiritimati",
        "Australia/Lord_Howe",
    ] {
        let label = format!(
            "{zone} at the {} of the range",
            if top { "end" } else { "start" }
        );
        let request = edge_request(zone, top);
        let (found, rejected) = confirm_every_start(&engine, &request, &label, &mut seen);
        offers += found;
        exclusions += rejected;
    }
    (offers, exclusions, seen)
}

/// At both ends of the supported range, in several zones, a start is offered only
/// if a confirmation accepts it, and excluded with the same reason a confirmation
/// gives. The two judge different ranges there — the search pads by the duration
/// and travel, the confirmation follows the booking — so this is where the
/// representable-minute decision has to hold on both sides.
#[test]
fn the_search_and_the_confirmation_agree_at_both_ends_of_the_range() {
    for top in [true, false] {
        let (offers, exclusions, seen) = sweep(top);
        let edge = if top { "end" } else { "start" };
        assert!(
            offers > 0 && exclusions > 0,
            "{edge}: the sweep offered {offers} and excluded {exclusions}"
        );
        assert!(
            seen.contains(&"outside_window")
                && seen.contains(&"travel")
                && seen.contains(&"blackout")
                && seen.contains(&"blocked")
                && seen.contains(&"rest"),
            "{edge}: the sweep reached the decisions the edge moves: {seen:?}"
        );
    }
}

/// A request whose only candidate is `start`, for a 30-minute booking by any host,
/// with the given travel, participants and rules, and an explanation so every
/// excluded start is reported.
fn candidate_request(
    start: Instant,
    travel: Travel,
    participants: Vec<Participant>,
    rules: Vec<Rule>,
) -> Request {
    let window = span(
        Instant::from_minutes(start.minutes().saturating_sub(3 * 60)).unwrap_or(Instant::MIN),
        Instant::from_minutes(start.minutes().saturating_add(3 * 60)).unwrap_or(Instant::MAX),
    );
    let mut goal = Goal::single(
        name("UTC"),
        window,
        Duration::from_minutes(30),
        Requirement::new(vec![Term::at_least(1, Selector::Role(Role::Host))]),
    );
    goal.candidates = Some(vec![start]);
    goal.parts[0].travel = travel;
    let mut request = Request::new(Instant::MIN, goal);
    request.participants = participants;
    request.rules = rules;
    request.explain = Some(Explain::new(1_000));
    request
}

/// A proposal for `start` assigning `hosts` to the goal's one term.
fn assignment(start: Instant, hosts: &[&str]) -> Proposal {
    Proposal {
        events: vec![ProposalEvent {
            part: "main".to_owned(),
            start,
            assignment: vec![hosts.iter().map(|id| (*id).to_owned()).collect()],
        }],
    }
}

/// The out-of-range rejection of `proposal`, pinned to the stable code and the
/// pointer the caller acts on.
fn out_of_range(engine: &Engine, request: &Request, proposal: &Proposal) -> opportune::Error {
    let err = engine
        .check_slot(request, proposal)
        .expect_err("the occupied range is not representable");
    assert_eq!(err.code(), "proposal_out_of_range");
    assert_eq!(err.path(), Some("/events/0/start"));
    err
}

/// The last supported instant is `2100-01-01T00:00Z`, which the model names as a
/// `Span` end but not as a start: a booking ending exactly there is offered and
/// confirms.
#[test]
fn a_booking_that_ends_at_the_last_supported_instant_is_offered_and_confirms() {
    let engine = engine();
    let start = Instant::from_minutes(Instant::MAX.minutes() - 30).unwrap();
    let request = candidate_request(
        start,
        Travel::default(),
        vec![host("rich", "UTC", Vec::new())],
        Vec::new(),
    );

    let response = engine.find_availability(&request).unwrap();
    let offer = response
        .offers
        .first()
        .expect("the last representable start is offered");
    assert_eq!(offer.end(), Some(Instant::MAX));
    let check = engine
        .check_slot(&request, &assignment(start, &["rich"]))
        .unwrap();
    assert!(check.valid, "{:?}", check.reasons);
    assert!(check.reserve.is_empty(), "no travel to hold");
}

/// One minute later the booking's end has no instant to name it, so the search
/// withholds the start from both its offers and its exclusions and a confirmation
/// rejects it, rather than describing a booking the model cannot report.
#[test]
fn a_booking_that_ends_past_the_last_supported_instant_is_withheld_and_rejected() {
    let engine = engine();
    let start = Instant::from_minutes(Instant::MAX.minutes() - 29).unwrap();
    let request = candidate_request(
        start,
        Travel::default(),
        vec![host("rich", "UTC", Vec::new())],
        Vec::new(),
    );

    let response = engine.find_availability(&request).unwrap();
    assert!(
        response.offers.is_empty(),
        "the booking's end is not representable: {:?}",
        response.offers
    );
    assert!(
        response.excluded.is_empty(),
        "a withheld start invents no scheduling reason: {:?}",
        response
            .excluded
            .iter()
            .flat_map(|excluded| excluded.reasons.iter().map(|reason| reason.code))
            .collect::<Vec<_>>()
    );

    let err = out_of_range(&engine, &request, &assignment(start, &["rich"]));
    assert!(
        err.to_string().contains("2100-01-01T00:00Z"),
        "the message names the edge: {err}"
    );
}

/// Travel after a booking that ends before the range's end is representable when
/// it stops exactly at the last supported instant, and reserved as an offer span;
/// one minute more has no span to hold.
#[test]
fn travel_that_ends_at_the_last_supported_instant_is_reserved_and_one_minute_more_is_rejected() {
    let engine = engine();
    let start = Instant::from_minutes(Instant::MAX.minutes() - 60).unwrap();
    let end = Instant::from_minutes(start.minutes() + 30).unwrap();
    let travel = |after: u32| Travel {
        before: Duration::ZERO,
        after: Duration::from_minutes(after),
        by_participant: BTreeMap::new(),
    };

    let request = candidate_request(
        start,
        travel(30),
        vec![host("rich", "UTC", Vec::new())],
        Vec::new(),
    );
    let response = engine.find_availability(&request).unwrap();
    let offer = response
        .offers
        .first()
        .expect("the travel ends at the last supported instant");
    assert_eq!(
        offer.events[0]
            .reserve
            .iter()
            .map(|reserve| reserve.span)
            .collect::<Vec<_>>(),
        vec![span(end, Instant::MAX)]
    );
    let check = engine
        .check_slot(&request, &assignment(start, &["rich"]))
        .unwrap();
    assert!(check.valid, "{:?}", check.reasons);
    assert_eq!(check.reserve, offer.events[0].reserve);

    let request = candidate_request(
        start,
        travel(31),
        vec![host("rich", "UTC", Vec::new())],
        Vec::new(),
    );
    let response = engine.find_availability(&request).unwrap();
    assert!(response.offers.is_empty());
    assert!(response.excluded.is_empty());
    let err = out_of_range(&engine, &request, &assignment(start, &["rich"]));
    assert!(
        err.to_string().contains("rich"),
        "the message names the participant whose travel crossed: {err}"
    );
}

/// A booking may occupy the first supported minute; travel before it one minute
/// earlier has no span to hold, so the start is withheld and rejected.
#[test]
fn travel_before_the_first_supported_instant_is_rejected() {
    let engine = engine();
    let request = candidate_request(
        Instant::MIN,
        Travel::default(),
        vec![host("rich", "UTC", Vec::new())],
        Vec::new(),
    );
    let response = engine.find_availability(&request).unwrap();
    assert_eq!(
        response
            .offers
            .iter()
            .map(|offer| offer.start().unwrap())
            .collect::<Vec<_>>(),
        vec![Instant::MIN],
        "the first supported minute is occupied without travel"
    );
    assert!(
        engine
            .check_slot(&request, &assignment(Instant::MIN, &["rich"]))
            .unwrap()
            .valid
    );

    let request = candidate_request(
        Instant::MIN,
        Travel {
            before: Duration::from_minutes(1),
            after: Duration::ZERO,
            by_participant: BTreeMap::new(),
        },
        vec![host("rich", "UTC", Vec::new())],
        Vec::new(),
    );
    let response = engine.find_availability(&request).unwrap();
    assert!(response.offers.is_empty());
    assert!(response.excluded.is_empty());
    let err = out_of_range(&engine, &request, &assignment(Instant::MIN, &["rich"]));
    assert!(
        err.to_string().contains("1975-01-01T00:00Z"),
        "the message names the edge: {err}"
    );
}

/// A search withholds a start its request-wide travel envelope cannot represent,
/// even when the host it would assign needs no travel; a confirmation judges only
/// the assigned hosts and the attendees, so the primary host confirms while the
/// backup host does not.
#[test]
fn an_unassigned_hosts_travel_bounds_the_search_and_the_confirmation() {
    let engine = engine();
    let start = Instant::from_minutes(Instant::MAX.minutes() - 30).unwrap();
    let mut travel = Travel::default();
    travel.by_participant.insert(
        "backup".to_owned(),
        ParticipantTravel {
            before: Duration::ZERO,
            after: Duration::from_minutes(60),
        },
    );
    let participants = vec![
        host("primary", "UTC", Vec::new()),
        host("backup", "UTC", Vec::new()),
    ];
    let request = candidate_request(start, travel, participants, Vec::new());

    let response = engine.find_availability(&request).unwrap();
    assert!(
        response.offers.is_empty(),
        "the request-wide envelope withholds the start the backup could take: {:?}",
        response
            .offers
            .iter()
            .filter_map(opportune::Offer::start)
            .collect::<Vec<_>>()
    );
    assert!(
        response.excluded.is_empty(),
        "a withheld start invents no scheduling reason"
    );

    // The host the request actually assigns needs no travel, so its booking fits.
    let check = engine
        .check_slot(&request, &assignment(start, &["primary"]))
        .unwrap();
    assert!(check.valid, "{:?}", check.reasons);
    assert!(check.reserve.is_empty());

    // The backup host's travel would end past the last supported instant.
    let err = out_of_range(&engine, &request, &assignment(start, &["backup"]));
    assert!(
        err.to_string().contains("backup"),
        "the message names the host whose travel crossed: {err}"
    );
}

/// An attendee is never assigned, but their travel is occupied time too: both the
/// search's request-wide envelope and the confirmation include it, whatever host
/// is chosen.
#[test]
fn an_attendees_travel_is_included_whatever_host_is_assigned() {
    let engine = engine();
    let start = Instant::from_minutes(Instant::MAX.minutes() - 30).unwrap();
    let mut travel = Travel::default();
    travel.by_participant.insert(
        "zoe".to_owned(),
        ParticipantTravel {
            before: Duration::ZERO,
            after: Duration::from_minutes(60),
        },
    );
    let participants = vec![
        host("primary", "UTC", Vec::new()),
        Participant::attendee("zoe", name("UTC")),
    ];
    let request = candidate_request(start, travel, participants, Vec::new());

    let response = engine.find_availability(&request).unwrap();
    assert!(
        response.offers.is_empty(),
        "the attendee's travel withholds the start"
    );

    let err = out_of_range(&engine, &request, &assignment(start, &["primary"]));
    assert!(
        err.to_string().contains("zoe"),
        "the message names the attendee whose travel crossed: {err}"
    );

    // A start the attendee's travel can reach offers with its hold, and the
    // confirmation agrees: the attendee is never assigned, but its travel is
    // reserved all the same.
    let start = Instant::from_minutes(Instant::MAX.minutes() - 90).unwrap();
    let request = candidate_request(
        start,
        {
            let mut travel = Travel::default();
            travel.by_participant.insert(
                "zoe".to_owned(),
                ParticipantTravel {
                    before: Duration::ZERO,
                    after: Duration::from_minutes(60),
                },
            );
            travel
        },
        vec![
            host("primary", "UTC", Vec::new()),
            Participant::attendee("zoe", name("UTC")),
        ],
        Vec::new(),
    );
    let response = engine.find_availability(&request).unwrap();
    let offer = response.offers.first().expect("the attendee's travel fits");
    assert_eq!(
        offer.events[0]
            .reserve
            .iter()
            .map(|reserve| (reserve.participant.as_str(), reserve.span))
            .collect::<Vec<_>>(),
        vec![(
            "zoe",
            span(
                Instant::from_minutes(start.minutes() + 30).unwrap(),
                Instant::MAX,
            ),
        )]
    );
    let check = engine
        .check_slot(&request, &assignment(start, &["primary"]))
        .unwrap();
    assert!(check.valid, "{:?}", check.reasons);
    assert_eq!(check.reserve, offer.events[0].reserve);
}

/// A blackout over the last supported minute excludes the travel that needs it,
/// and both the search and the confirmation say so, citing the blackout rather
/// than leaving the reason without a rule.
#[test]
fn a_blackout_on_the_last_minute_excludes_the_travel_that_needs_it() {
    let engine = engine();
    // The booking ends thirty minutes before the range does, so the travel after
    // it is representable and reaches the final minute the model can name.
    let start = Instant::from_minutes(Instant::MAX.minutes() - 60).unwrap();
    let travel = Travel {
        before: Duration::ZERO,
        after: Duration::from_minutes(30),
        by_participant: BTreeMap::new(),
    };
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
    let request = candidate_request(
        start,
        travel,
        vec![host("rich", "UTC", Vec::new())],
        vec![Rule::new("closed", RuleKind::Blackout(closed))],
    );

    let response = engine.find_availability(&request).unwrap();
    assert!(
        response.offers.is_empty(),
        "the travel minute is blacked out"
    );
    let excluded = response.excluded.first().expect("the start is excluded");
    assert_eq!(excluded.start, start);
    assert_eq!(excluded.reasons[0].code, ReasonCode::Travel);
    assert_eq!(excluded.reasons[0].rule.as_deref(), Some("closed"));

    let check = engine
        .check_slot(&request, &assignment(start, &["rich"]))
        .unwrap();
    assert!(!check.valid);
    assert_eq!(check.reasons[0].code, ReasonCode::Travel);
    assert_eq!(check.reasons[0].rule.as_deref(), Some("closed"));
}

/// When more than one participant's travel crosses an edge, the reported failure
/// is the first in ascending ID order, whatever order the request lists the
/// participants in.
#[test]
fn the_out_of_range_failure_names_the_first_participant_in_id_order() {
    let engine = engine();
    let build = |request_order: [Participant; 3]| {
        let mut travel = Travel::default();
        for id in ["amber", "zoe"] {
            travel.by_participant.insert(
                id.to_owned(),
                ParticipantTravel {
                    before: Duration::from_minutes(1),
                    after: Duration::ZERO,
                },
            );
        }
        candidate_request(Instant::MIN, travel, request_order.to_vec(), Vec::new())
    };
    let host = || host("primary", "UTC", Vec::new());

    let err = out_of_range(
        &engine,
        &build([
            host(),
            Participant::attendee("amber", name("UTC")),
            Participant::attendee("zoe", name("UTC")),
        ]),
        &assignment(Instant::MIN, &["primary"]),
    );
    assert!(
        err.to_string().contains("amber"),
        "the lowest participant ID crossing the edge is the one reported: {err}"
    );

    // Reversing the request order changes nothing: the failure is chosen by ID.
    let err = out_of_range(
        &engine,
        &build([
            Participant::attendee("zoe", name("UTC")),
            Participant::attendee("amber", name("UTC")),
            host(),
        ]),
        &assignment(Instant::MIN, &["primary"]),
    );
    assert!(
        err.to_string().contains("amber"),
        "and it does not depend on the order the participants are written in: {err}"
    );
}

/// Travel before a booking is representable when it starts exactly at the first
/// supported instant, and reserved as an offer span; one minute further back has
/// no instant to name, so the start is withheld and rejected. The mirror of the
/// travel after a booking that stops exactly at the range's end.
#[test]
fn travel_that_starts_at_the_first_supported_instant_is_reserved_and_one_minute_more_is_rejected() {
    let engine = engine();
    let start = Instant::from_minutes(Instant::MIN.minutes() + 60).unwrap();
    let travel = |before: u32| Travel {
        before: Duration::from_minutes(before),
        after: Duration::ZERO,
        by_participant: BTreeMap::new(),
    };

    let request = candidate_request(
        start,
        travel(60),
        vec![host("rich", "UTC", Vec::new())],
        Vec::new(),
    );
    let response = engine.find_availability(&request).unwrap();
    let offer = response
        .offers
        .first()
        .expect("the travel starts at the first supported instant");
    assert_eq!(
        offer.events[0]
            .reserve
            .iter()
            .map(|reserve| reserve.span)
            .collect::<Vec<_>>(),
        vec![span(Instant::MIN, start)]
    );
    let check = engine
        .check_slot(&request, &assignment(start, &["rich"]))
        .unwrap();
    assert!(check.valid, "{:?}", check.reasons);
    assert_eq!(check.reserve, offer.events[0].reserve);

    // One minute further back the hold would begin before the first supported
    // instant, so the start it belongs to is not bookable.
    let request = candidate_request(
        start,
        travel(61),
        vec![host("rich", "UTC", Vec::new())],
        Vec::new(),
    );
    let response = engine.find_availability(&request).unwrap();
    assert!(response.offers.is_empty(), "{:?}", response.offers);
    assert!(response.excluded.is_empty());
    let err = out_of_range(&engine, &request, &assignment(start, &["rich"]));
    assert!(
        err.to_string().contains("1975-01-01T00:00Z"),
        "the message names the edge: {err}"
    );
}

/// A booking whose own span runs past the last supported instant is diagnosed on
/// that span rather than blamed on a participant's zero travel: the failure names
/// the booking and its length, and names nobody.
#[test]
fn a_bookings_own_overrun_is_diagnosed_on_the_booking_not_on_a_participants_travel() {
    let engine = engine();
    let start = Instant::from_minutes(Instant::MAX.minutes() - 29).unwrap();
    let request = candidate_request(
        start,
        Travel::default(),
        vec![host("rich", "UTC", Vec::new())],
        Vec::new(),
    );

    let err = out_of_range(&engine, &request, &assignment(start, &["rich"]));
    let message = err.to_string();
    assert!(
        message.contains("lasts 30m") && message.contains("2100-01-01T00:00Z"),
        "the booking's own span is what crossed: {message}"
    );
    assert!(
        !message.contains("needs") && !message.contains("rich"),
        "nobody's travel is to blame, so nobody is named: {message}"
    );
}

/// The default grid search — no explicit candidate list — applies the
/// request-wide travel envelope, not the part's base travel: an unassigned host and
/// an attendee with longer overrides withhold exactly the starts their travel would
/// push past the last supported instant, and every start still offered confirms
/// with the holds it reports.
#[test]
fn the_grid_path_withholds_what_an_unassigned_hosts_override_cannot_represent() {
    let engine = engine();
    let top = Instant::MAX.minutes();
    let at = |minutes: i64| Instant::from_minutes(minutes).unwrap();
    let mut travel = Travel::default();
    for (id, after) in [("backup", 60), ("zoe", 30)] {
        travel.by_participant.insert(
            id.to_owned(),
            ParticipantTravel {
                before: Duration::ZERO,
                after: Duration::from_minutes(after),
            },
        );
    }
    let mut goal = Goal::single(
        name("UTC"),
        span(at(top - 3 * 60), Instant::MAX),
        Duration::from_minutes(30),
        Requirement::new(vec![Term::at_least(1, Selector::Role(Role::Host))]),
    );
    goal.candidates = None;
    goal.parts[0].travel = travel;
    goal.grid = Some(
        Grid::new(
            Duration::from_minutes(30),
            TimeOfDay::MIDNIGHT,
            GridZone::Named(name("UTC")),
        )
        .expect("a half-hour step is a valid grid"),
    );
    let mut request = Request::new(Instant::MIN, goal);
    request.participants = vec![
        host("primary", "UTC", Vec::new()),
        host("backup", "UTC", Vec::new()),
        Participant::attendee("zoe", name("UTC")),
    ];
    request.explain = Some(Explain::new(1_000));

    let response = engine.find_availability(&request).unwrap();
    let starts: Vec<Instant> = response
        .offers
        .iter()
        .filter_map(opportune::Offer::start)
        .collect();
    assert_eq!(
        starts.last().copied(),
        Some(at(top - 90)),
        "the backup's hour of travel ends at the last supported instant: {starts:?}"
    );
    assert!(
        response.excluded.is_empty(),
        "a withheld start invents no reason"
    );

    // Every offer the grid still yields confirms, with the holds it reports.
    for offer in &response.offers {
        let event = &offer.events[0];
        let check = engine
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
        assert!(
            check.valid,
            "{} was offered but cannot be confirmed: {:?}",
            event.start, check.reasons
        );
        assert_eq!(check.reserve, event.reserve, "{}", event.start);
    }

    // The primary host and the attendee both fit the start the backup's travel
    // withholds, so the withholding is the request-wide envelope's rather than the
    // assignment the search would pick.
    let withheld = at(top - 60);
    assert!(
        !starts.contains(&withheld),
        "the envelope is request-wide: {starts:?}"
    );
    let check = engine
        .check_slot(&request, &assignment(withheld, &["primary"]))
        .unwrap();
    assert!(check.valid, "{:?}", check.reasons);
}

/// The candidates-limit guard counts the starts a search can actually offer: a
/// start the before-travel envelope withholds at the first supported instant
/// must not consume the bounded walk, or a request over the configured maximum
/// would pass validation and search past it.
#[test]
fn the_candidate_limit_counts_only_starts_the_envelope_can_represent() {
    let engine = Engine::builder()
        .limits(opportune::Limits::default().with_max_candidate_starts(5))
        .build()
        .unwrap();
    // A two-day window on a half-hour grid yields 96 starts; the hour of travel
    // before withholds the two that begin within it of the first supported
    // instant, and the 94 that remain are still far above the maximum of 5.
    let mut goal = Goal::single(
        name("UTC"),
        span(
            Instant::MIN,
            Instant::from_minutes(Instant::MIN.minutes() + 2 * 24 * 60).unwrap(),
        ),
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("rich")]),
    );
    goal.grid = Some(
        Grid::new(
            Duration::from_minutes(30),
            TimeOfDay::MIDNIGHT,
            GridZone::Named(name("UTC")),
        )
        .unwrap(),
    );
    goal.parts[0].travel = Travel {
        before: Duration::from_minutes(60),
        after: Duration::ZERO,
        by_participant: BTreeMap::new(),
    };
    let mut request = Request::new(Instant::MIN, goal);
    request.participants = vec![host("rich", "UTC", Vec::new())];
    let diagnostics = engine.validate(&request);
    let codes: Vec<&str> = diagnostics
        .errors
        .iter()
        .map(opportune::Diagnostic::code)
        .collect();
    assert!(
        codes.contains(&"candidates_limit"),
        "the withheld starts must not shrink the count below the maximum: {codes:?}"
    );
}

/// A part's default travel every request participant overrides away must not
/// withhold the first supported instant: the request-wide envelope is each
/// participant's effective travel, and with every override zero a booking at
/// `Instant::MIN` needs no travel at all — while an unused default used to push
/// both the explicit candidate and the first grid start off the range.
#[test]
fn a_default_every_participant_overrides_away_offers_the_first_supported_instant() {
    let engine = engine();
    let travel = Travel::new(Duration::from_minutes(1), Duration::ZERO).with_participant(
        "rich",
        Duration::ZERO,
        Duration::ZERO,
    );

    let request = candidate_request(
        Instant::MIN,
        travel.clone(),
        vec![host("rich", "UTC", Vec::new())],
        Vec::new(),
    );
    let response = engine.find_availability(&request).unwrap();
    let starts: Vec<Instant> = response
        .offers
        .iter()
        .filter_map(opportune::Offer::start)
        .collect();
    assert_eq!(
        starts,
        vec![Instant::MIN],
        "the unused default withholds nothing: {starts:?}"
    );
    let check = engine
        .check_slot(&request, &assignment(Instant::MIN, &["rich"]))
        .unwrap();
    assert!(check.valid, "{:?}", check.reasons);
    assert!(check.reserve.is_empty());

    // The grid path: the first start the bounded walk offers is the window's
    // first minute, not one the unused default pushed inward.
    let mut goal = Goal::single(
        name("UTC"),
        span(
            Instant::MIN,
            Instant::from_minutes(Instant::MIN.minutes() + 3 * 60).unwrap(),
        ),
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("rich")]),
    );
    goal.candidates = None;
    goal.parts[0].travel = travel;
    let mut request = Request::new(Instant::MIN, goal);
    request.participants = vec![host("rich", "UTC", Vec::new())];
    let response = engine.find_availability(&request).unwrap();
    let starts: Vec<Instant> = response
        .offers
        .iter()
        .filter_map(opportune::Offer::start)
        .collect();
    assert_eq!(
        starts.first(),
        Some(&Instant::MIN),
        "the grid's first start is the first supported minute: {starts:?}"
    );
    for start in &starts {
        let check = engine
            .check_slot(&request, &assignment(*start, &["rich"]))
            .unwrap();
        assert!(check.valid, "{start:?}: {:?}", check.reasons);
    }
}

/// The mirror at the last supported instant: a default after-travel every
/// participant overrides to zero leaves the last explicit candidate and the last
/// grid start bookable, and every offer confirms with no travel to reserve.
#[test]
fn a_default_every_participant_overrides_away_offers_the_last_supported_instant() {
    let engine = engine();
    let top = Instant::MAX.minutes();
    let at = |minutes: i64| Instant::from_minutes(minutes).unwrap();
    let travel = Travel::new(Duration::ZERO, Duration::from_minutes(1)).with_participant(
        "rich",
        Duration::ZERO,
        Duration::ZERO,
    );

    let request = candidate_request(
        at(top - 30),
        travel.clone(),
        vec![host("rich", "UTC", Vec::new())],
        Vec::new(),
    );
    let response = engine.find_availability(&request).unwrap();
    let starts: Vec<Instant> = response
        .offers
        .iter()
        .filter_map(opportune::Offer::start)
        .collect();
    assert_eq!(
        starts,
        vec![at(top - 30)],
        "the unused default withholds nothing: {starts:?}"
    );
    let check = engine
        .check_slot(&request, &assignment(at(top - 30), &["rich"]))
        .unwrap();
    assert!(check.valid, "{:?}", check.reasons);
    assert!(check.reserve.is_empty());

    let mut goal = Goal::single(
        name("UTC"),
        span(at(top - 3 * 60), Instant::MAX),
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("rich")]),
    );
    goal.candidates = None;
    goal.parts[0].travel = travel;
    let mut request = Request::new(Instant::MIN, goal);
    request.participants = vec![host("rich", "UTC", Vec::new())];
    let response = engine.find_availability(&request).unwrap();
    let starts: Vec<Instant> = response
        .offers
        .iter()
        .filter_map(opportune::Offer::start)
        .collect();
    assert_eq!(
        starts.last(),
        Some(&at(top - 30)),
        "the last grid start is bookable: {starts:?}"
    );
    for start in &starts {
        let check = engine
            .check_slot(&request, &assignment(*start, &["rich"]))
            .unwrap();
        assert!(check.valid, "{start:?}: {:?}", check.reasons);
    }
}

/// An attendee who inherits the part's default travel — no override of their own
/// — still widens the request-wide envelope when every host overrides theirs to
/// zero: their travel is occupied time, so the search withholds what it pushes
/// past the edge and the confirmation names them.
#[test]
fn an_attendee_who_inherits_the_default_still_widens_the_envelope() {
    let engine = engine();
    let top = Instant::MAX.minutes();
    let at = |minutes: i64| Instant::from_minutes(minutes).unwrap();
    let travel = Travel::new(Duration::ZERO, Duration::from_minutes(60)).with_participant(
        "primary",
        Duration::ZERO,
        Duration::ZERO,
    );
    let participants = vec![
        host("primary", "UTC", Vec::new()),
        Participant::attendee("zoe", name("UTC")),
    ];

    let request = candidate_request(at(top - 30), travel, participants, Vec::new());
    let response = engine.find_availability(&request).unwrap();
    assert!(
        response.offers.is_empty(),
        "the attendee's inherited default travel withholds the start: {:?}",
        response
            .offers
            .iter()
            .filter_map(opportune::Offer::start)
            .collect::<Vec<_>>()
    );
    let err = out_of_range(&engine, &request, &assignment(at(top - 30), &["primary"]));
    assert!(
        err.to_string().contains("zoe"),
        "the attendee whose inherited travel crossed is named: {err}"
    );
}

/// The fetch envelope is request-wide too: a day of default travel no request
/// participant uses does not widen `required_ranges`' calendar hull, while an
/// attendee who inherits the default widens it by exactly one local day either
/// side, and an empty participant list falls back to the part-wide maximum.
#[test]
fn a_default_no_participant_uses_does_not_widen_the_fetch_envelope() {
    let engine = engine();
    let day = Instant::MIN.minutes() + 100 * 24 * 60;
    let at = |minutes: i64| Instant::from_minutes(minutes).unwrap();
    let window = span(at(day), at(day + 23 * 60));
    let default = Duration::from_minutes(24 * 60);
    let goal = |travel: Travel| {
        let mut goal = Goal::single(
            name("UTC"),
            window,
            Duration::from_minutes(30),
            Requirement::new(vec![Term::person("rich")]),
        );
        goal.parts[0].travel = travel;
        goal
    };
    let query = |goal: &Goal, participants: Vec<Participant>| {
        RangesQuery::new(Instant::MIN, goal.clone())
            .with_participants(participants)
            .with_rules(vec![presets::cap_per_day(Selector::True, 10)])
    };

    // The host overrides the default away in both directions: the hull covers
    // the padded window and its neighbouring days, nothing more.
    let overridden =
        Travel::new(default, default).with_participant("rich", Duration::ZERO, Duration::ZERO);
    let narrow = engine
        .required_ranges(&query(
            &goal(overridden.clone()),
            vec![host("rich", "UTC", Vec::new())],
        ))
        .unwrap();

    // The same host keeping the default widens the hull by exactly one local
    // day either side — the widening the overridden request must not inherit.
    let kept = overridden
        .clone()
        .with_participant("rich", default, default);
    let wide = engine
        .required_ranges(&query(&goal(kept), vec![host("rich", "UTC", Vec::new())]))
        .unwrap();
    assert!(narrow.occurrences.is_some());
    assert_eq!(
        narrow.occurrences, wide.occurrences,
        "travel changes only the calendar hull, never a cap's occurrence hull"
    );
    assert_eq!(
        narrow.calendar.start().minutes() - wide.calendar.start().minutes(),
        24 * 60,
        "the unused default does not widen the hull's start"
    );
    assert_eq!(
        wide.calendar.end().minutes() - narrow.calendar.end().minutes(),
        24 * 60,
        "the unused default does not widen the hull's end"
    );

    // An attendee who inherits the default while the host overrides it away
    // widens the fetch envelope exactly as the host keeping it did.
    let inherited = engine
        .required_ranges(&query(
            &goal(overridden),
            vec![
                host("rich", "UTC", Vec::new()),
                Participant::attendee("zoe", name("UTC")),
            ],
        ))
        .unwrap();
    assert_eq!(inherited.calendar.start(), wide.calendar.start());
    assert_eq!(inherited.calendar.end(), wide.calendar.end());

    // With no participant to read, the fallback is the part-wide maximum,
    // default and override map alike: the exact padded window, unrestricted by
    // any zone's day rounding.
    let empty = engine
        .required_ranges(&query(&goal(Travel::new(default, default)), Vec::new()))
        .unwrap();
    assert_eq!(
        empty.calendar,
        span(at(day - 24 * 60), at(day + 23 * 60 + 30 + 24 * 60))
    );
}

/// Narrowing the fetch envelope must not drop rest activity that still excludes
/// an overridden, zero-travel booking. The older late block lies only in the
/// default-inflated hull and is stripped; the recent one remains necessary.
#[test]
fn an_overridden_fetch_envelope_preserves_rest_exclusions_after_stripping() {
    let engine = engine();
    let day = Instant::MIN.minutes() + 100 * 24 * 60;
    let at = |minutes: i64| Instant::from_minutes(minutes).unwrap();
    let start = at(day + 9 * 60);
    let default = Duration::from_minutes(24 * 60);
    let travel =
        Travel::new(default, default).with_participant("rich", Duration::ZERO, Duration::ZERO);
    let old = block(
        at(day - 7 * 24 * 60 + 18 * 60),
        at(day - 7 * 24 * 60 + 19 * 60),
    );
    let recent = block(at(day - 24 * 60 + 18 * 60), at(day - 24 * 60 + 19 * 60));
    let request = candidate_request(
        start,
        travel,
        vec![host("rich", "UTC", vec![old, recent])],
        vec![presets::rest(
            TimeOfDay::new(18, 0).unwrap(),
            TimeOfDay::new(10, 0).unwrap(),
        )],
    );
    let query = RangesQuery::new(request.now, request.goal.clone())
        .with_participants(request.participants.clone())
        .with_rules(request.rules.clone());
    let ranges = engine.required_ranges(&query).unwrap();

    let full = engine.find_availability(&request).unwrap();
    assert!(full.offers.is_empty());
    assert_eq!(full.excluded[0].reasons[0].code, ReasonCode::Rest);

    let mut stripped = request.clone();
    for participant in &mut stripped.participants {
        if let Some(blocks) = &mut participant.calendar {
            blocks.retain(|block| {
                block.span.start() < ranges.calendar.end()
                    && ranges.calendar.start() < block.span.end()
            });
        }
    }
    assert_eq!(
        stripped.participants[0].blocks().len(),
        1,
        "only the default-inflated day's block is removed: calendar {:?}, retained {:?}",
        ranges.calendar,
        stripped.participants[0].blocks()
    );
    let fetched = engine.find_availability(&stripped).unwrap();
    assert_eq!(fetched.offers, full.offers);
    assert_eq!(fetched.excluded, full.excluded);
    assert_eq!(fetched.dates, full.dates);
}

/// A cap still decides a confirmation whose search window is longer than the
/// configured maximum — a request only a confirmation can run, and the shape the
/// scope split newly admits. The cap's bucket for the proposed start is what
/// decides, so the answer must not depend on how far the window reaches.
#[test]
fn a_cap_still_decides_a_confirmation_the_search_would_reject() {
    let day = Instant::from_minutes(Instant::MIN.minutes() + 200 * 24 * 60).unwrap();
    let at = |minutes: i64| Instant::from_minutes(day.minutes() + minutes).unwrap();
    let mut taken = host("rich", "UTC", vec![block(at(10 * 60), at(11 * 60))]);
    taken.calendar.as_mut().unwrap()[0].event_key = None;
    let mut goal = Goal::single(
        name("UTC"),
        span(day, at(366 * 24 * 60)),
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("rich")]),
    );
    goal.candidates = None;
    let mut request = Request::new(day, goal);
    request.participants = vec![taken];
    request.rules = vec![presets::cap_per_day(opportune::tags::Selector::True, 1)];

    // The search cannot run this request at all, which is what makes the
    // confirmation the only way to ask the question.
    let errors: Vec<&str> = engine()
        .validate(&request)
        .errors
        .iter()
        .map(opportune::Diagnostic::code)
        .collect();
    assert!(errors.contains(&"search_window_limit"), "{errors:?}");
    assert!(engine().find_availability(&request).is_err());

    // A start on the day the cap is already spent is still excluded.
    let check = engine()
        .check_slot(&request, &assignment(at(12 * 60), &["rich"]))
        .unwrap();
    assert!(!check.valid);
    assert_eq!(
        check
            .reasons
            .iter()
            .map(|reason| (reason.code, reason.rule.as_deref()))
            .collect::<Vec<_>>(),
        vec![(ReasonCode::CapReached, Some("cap_per_day"))]
    );

    // A start a day later, where the cap is not spent, is confirmed.
    let check = engine()
        .check_slot(&request, &assignment(at(24 * 60 + 12 * 60), &["rich"]))
        .unwrap();
    assert!(
        check.valid,
        "{:?}",
        check
            .reasons
            .iter()
            .map(|reason| (reason.code, reason.rule.clone()))
            .collect::<Vec<_>>()
    );
}
