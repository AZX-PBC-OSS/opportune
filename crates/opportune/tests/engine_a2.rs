//! End-to-end tests for multi-participant evaluation over the bundled database:
//! roles, requirement terms, weights, joint fillability, the `available_only`
//! preset, and what `explain` and `check_slot` answer for them.
//!
//! Every test builds a real request through the public model types, runs it on a
//! real `Engine` and asserts what a caller acts on, so deleting the behaviour it
//! covers turns it red.

#![cfg(feature = "bundled-tzdb")]

use std::collections::BTreeMap;

use opportune::model::{
    Block, Explain, Goal, Part, Participant, Proposal, ProposalEvent, RangesQuery, Request,
    Requirement, Rule, RuleKind, Span, Term, Travel, ZonePeriod,
};
use opportune::presets;
use opportune::reasons::ReasonCode;
use opportune::tags::{Role, Selector, Tag};
use opportune::time::{
    Date, Duration, Instant, LocalResolution, RuleZone, TimeOfDay, WallEnd, WallRange, Weekday,
    Weekdays, WeeklyKey, WindowSpec, Zone, ZoneName,
};
use opportune::{Engine, Limits};

fn instant(hour: u8, minute: u8) -> Instant {
    // A fixed UTC day, so every expectation is arithmetic on whole hours.
    let date = Date::new(2027, 6, 14).unwrap();
    let time = TimeOfDay::new(hour, minute).unwrap();
    let LocalResolution::Unique(instant) = Zone::utc().resolve_local(date, time).unwrap() else {
        panic!("UTC has no transitions");
    };
    instant
}

fn name(zone: &str) -> ZoneName {
    ZoneName::new(zone).unwrap()
}

fn tag(path: &str) -> Tag {
    Tag::new(path).unwrap()
}

fn engine() -> Engine {
    Engine::builder().build().unwrap()
}

fn day() -> Span {
    Span::new(instant(9, 0), instant(12, 0)).unwrap()
}

/// A host with an empty calendar: known to be free.
fn host(id: &str) -> Participant {
    let mut participant = Participant::new(id, name("UTC"));
    participant.calendar = Some(Vec::new());
    participant
}

/// An attendee with no calendar at all, which is normal for someone outside the
/// organisation.
fn attendee(id: &str) -> Participant {
    attendee_in(id, "UTC")
}

/// An attendee with no calendar at all, whose home zone is `zone`.
fn attendee_in(id: &str, zone: &str) -> Participant {
    let mut participant = Participant::new(id, name(zone));
    participant.role = Role::Attendee;
    participant
}

fn block(from: Instant, until: Instant) -> Block {
    Block::new(Span::new(from, until).unwrap())
}

/// A `window` rule open `from`–`until` every day, read in `zone`.
fn hours(zone: RuleZone, from: u8, until: u8) -> Rule {
    let spec = WindowSpec::new(
        zone,
        vec![
            WeeklyKey::new(
                Weekdays::range(Weekday::Monday, Weekday::Sunday),
                vec![
                    WallRange::new(
                        TimeOfDay::new(from, 0).unwrap(),
                        WallEnd::Time(TimeOfDay::new(until, 0).unwrap()),
                    )
                    .unwrap(),
                ],
            )
            .unwrap(),
        ],
        Vec::new(),
    )
    .unwrap();
    Rule::new("hours", RuleKind::Window(spec))
}

/// A request booking `part` over the fixed day, with an explanation so every
/// exclusion is reported.
fn request_for(part: Part, participants: Vec<Participant>) -> Request {
    let mut request = Request::new(instant(0, 0), Goal::new(name("UTC"), day(), vec![part]));
    request.participants = participants;
    request.explain = Some(Explain::new(10_000));
    request
}

fn offer_starts(response: &opportune::model::Response) -> Vec<Instant> {
    response
        .offers
        .iter()
        .map(|offer| offer.start().unwrap())
        .collect()
}

fn proposal(start: Instant, assignment: Vec<Vec<&str>>) -> Proposal {
    Proposal {
        events: vec![ProposalEvent {
            part: "main".to_owned(),
            start,
            assignment: assignment
                .into_iter()
                .map(|ids| ids.into_iter().map(str::to_owned).collect())
                .collect(),
        }],
    }
}

/// The `(participant, code)` pairs of an exclusion entry's reasons, in report
/// order.
fn reasons(excluded: &opportune::reasons::Excluded) -> Vec<(Option<&str>, ReasonCode)> {
    excluded
        .reasons
        .iter()
        .map(|reason| (reason.participant.as_deref(), reason.code))
        .collect()
}

/// The exclusion entry for `start`.
fn excluded_at(
    response: &opportune::model::Response,
    start: Instant,
) -> &opportune::reasons::Excluded {
    response
        .excluded
        .iter()
        .find(|entry| entry.start == start)
        .unwrap_or_else(|| panic!("{start} must be excluded; got {:?}", offer_starts(response)))
}

// ------------------------------------------------------------- attendees ---

#[test]
fn an_attendee_is_never_offered_or_assigned_and_gates_every_offer() {
    // `xx` is free all day, `zz` (the attendee) is busy from 10:00, and the
    // attendee sorts last so the eligible list would show them if they were
    // treated as a host.
    let mut busy = attendee("zz");
    busy.calendar = Some(vec![block(instant(10, 0), instant(12, 0))]);
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::at_least(1, Selector::True)]),
    );
    let request = request_for(part, vec![host("xx"), busy]);
    let response = engine().find_availability(&request).unwrap();

    assert_eq!(
        offer_starts(&response),
        [instant(9, 0), instant(9, 30)],
        "the attendee's block removes every start from 10:00 on"
    );
    for offer in &response.offers {
        let event = &offer.events[0];
        assert_eq!(event.eligible, [vec!["xx".to_owned()]]);
        assert_eq!(event.assignment, [vec!["xx".to_owned()]]);
    }
}

#[test]
fn an_unavailable_attendee_excludes_every_start_with_a_start_level_reason() {
    let mut busy = attendee("zz");
    busy.calendar = Some(vec![block(instant(0, 0), instant(23, 0))]);
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("xx")]),
    );
    let request = request_for(part, vec![host("xx"), busy]);
    let response = engine().find_availability(&request).unwrap();

    assert!(response.offers.is_empty());
    let excluded = excluded_at(&response, instant(9, 0));
    assert_eq!(
        reasons(excluded),
        [
            (Some("zz"), ReasonCode::Blocked),
            (None, ReasonCode::AttendeeUnavailable)
        ],
        "the attendee's own reason, then the start-level one"
    );

    // A confirmation judges the attendee too, even though nobody assigns them.
    let check = engine()
        .check_slot(&request, &proposal(instant(9, 0), vec![vec!["xx"]]))
        .unwrap();
    assert!(!check.valid);
    assert_eq!(
        check
            .reasons
            .iter()
            .map(|reason| (reason.participant.as_deref(), reason.code))
            .collect::<Vec<_>>(),
        [(Some("zz"), ReasonCode::Blocked)]
    );
}

#[test]
fn an_attendee_is_constrained_only_by_rules_whose_who_selects_them() {
    // The attendee is home in Kolkata, so a rule read in each participant's own
    // zone would empty the offer list if its default `who` reached them: 09:00
    // to 12:00 there is 03:30 to 06:30 UTC, which covers no candidate start.
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("xx")]),
    );
    let mut request = request_for(part, vec![host("xx"), attendee_in("zz", "Asia/Kolkata")]);
    request.rules.push(hours(RuleZone::Participant, 9, 12));

    let response = engine().find_availability(&request).unwrap();
    assert_eq!(
        response.offers.len(),
        6,
        "the default `who` reaches hosts, never the attendee"
    );
    assert!(
        response
            .offers
            .iter()
            .all(|offer| offer.events[0].assignment == [vec!["xx".to_owned()]]),
        "the attendee is never assigned"
    );

    // The same window narrowed to 10:00-11:00 UTC and scoped to the attendee by
    // role. Nothing else changed, so the starts it removes are the proof the
    // selector reached them.
    request.rules.push(
        Rule::new(
            "attendee-hours",
            RuleKind::Window(
                hours(RuleZone::Named(name("UTC")), 10, 11)
                    .window()
                    .unwrap()
                    .clone(),
            ),
        )
        .with_who(Selector::Role(Role::Attendee))
        .with_when(Selector::True),
    );
    let narrowed = engine().find_availability(&request).unwrap();
    assert_eq!(
        offer_starts(&narrowed),
        [instant(10, 0), instant(10, 30)],
        "the attendee's own window decides which starts are offered"
    );
    let excluded = excluded_at(&narrowed, instant(9, 0));
    assert_eq!(
        reasons(excluded),
        [
            (Some("zz"), ReasonCode::OutsideWindow),
            (None, ReasonCode::AttendeeUnavailable)
        ],
        "the attendee explains itself, then the start does"
    );
    assert_eq!(
        excluded.reasons[0].rule.as_deref(),
        Some("attendee-hours"),
        "the reason cites the rule that selected the attendee"
    );
}

#[test]
fn a_calendar_unknown_flag_marks_an_assigned_host_only() {
    // No calendar at all: free within their rules, and flagged when assigned.
    let bare = Participant::new("xx", name("UTC"));
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("xx")]),
    );
    let mut request = request_for(part, vec![bare, attendee("zz")]);
    request.limit = Some(1);
    let response = engine().find_availability(&request).unwrap();
    assert_eq!(
        response.offers[0].flags,
        [opportune::model::OfferFlag::CalendarUnknown]
    );

    // An unassigned calendarless host and a calendarless attendee raise nothing.
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("yy")]),
    );
    let bare_host = Participant::new("xx", name("UTC"));
    let request = request_for(part, vec![host("yy"), bare_host, attendee("zz")]);
    let response = engine().find_availability(&request).unwrap();
    assert!(response.offers[0].flags.is_empty());
}

// --------------------------------------------------------------- terms ---

#[test]
fn a_collective_meeting_needs_every_named_host_free() {
    let mut ana = host("ana");
    ana.calendar = Some(vec![block(instant(10, 0), instant(11, 0))]);
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![
            Term::person("ana"),
            Term::person("bo"),
            Term::person("cy"),
        ]),
    );
    let request = request_for(part, vec![ana, host("bo"), host("cy")]);
    let response = engine().find_availability(&request).unwrap();

    assert_eq!(
        offer_starts(&response),
        [
            instant(9, 0),
            instant(9, 30),
            instant(11, 0),
            instant(11, 30)
        ]
    );
    let offer = &response.offers[0].events[0];
    assert_eq!(
        offer.eligible,
        [
            vec!["ana".to_owned()],
            vec!["bo".to_owned()],
            vec!["cy".to_owned()]
        ]
    );
    assert_eq!(offer.assignment, offer.eligible);

    // At 10:00 ana is busy, and her reason is the one that explains the start.
    let excluded = excluded_at(&response, instant(10, 0));
    assert_eq!(reasons(excluded), [(Some("ana"), ReasonCode::Blocked)]);
}

#[test]
fn a_quota_term_needs_its_count_but_offers_every_host_that_could_take_it() {
    let mut bo = host("bo");
    bo.tags = vec![tag("pool/backend")];
    bo.calendar = Some(vec![block(instant(9, 0), instant(10, 0))]);
    let mut ana = host("ana");
    ana.tags = vec![tag("pool/backend")];
    let mut cy = host("cy");
    cy.tags = vec![tag("pool/backend")];
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::at_least(2, Selector::Tag(tag("pool/backend")))]),
    );
    let request = request_for(part, vec![ana, bo.clone(), cy]);
    let response = engine().find_availability(&request).unwrap();

    // Only ana and cy are free at 9:00, which fills the two slots.
    assert_eq!(response.offers[0].start(), Some(instant(9, 0)));
    let offer = &response.offers[0].events[0];
    assert_eq!(offer.eligible, [vec!["ana".to_owned(), "cy".to_owned()]]);
    assert_eq!(offer.assignment, [vec!["ana".to_owned(), "cy".to_owned()]]);

    // With only one of the three free, the term cannot be filled; the blocked
    // hosts' own reasons explain it, so no start-level reason is added.
    let mut ana = host("ana");
    ana.tags = vec![tag("pool/backend")];
    ana.calendar = Some(vec![block(instant(9, 0), instant(9, 30))]);
    let mut cy = host("cy");
    cy.tags = vec![tag("pool/backend")];
    cy.calendar = Some(vec![block(instant(9, 0), instant(9, 30))]);
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::at_least(2, Selector::Tag(tag("pool/backend")))]),
    );
    let request = request_for(part, vec![ana, bo, cy]);
    let response = engine().find_availability(&request).unwrap();
    assert!(!offer_starts(&response).contains(&instant(9, 0)));
    let excluded = excluded_at(&response, instant(9, 0));
    assert_eq!(
        reasons(excluded),
        [
            (Some("ana"), ReasonCode::Blocked),
            (Some("bo"), ReasonCode::Blocked),
            (Some("cy"), ReasonCode::Blocked)
        ],
        "each blocked host explains itself, and no start-level reason repeats it"
    );
}

#[test]
fn overlapping_pools_that_cannot_fill_jointly_report_term_unfillable() {
    // Two terms over the same two hosts. `ana` is busy at 9:00, so each term has
    // one eligible host and no assignment gives them distinct people: the
    // shortfall is not any one host's unavailability, so the start carries a
    // start-level reason.
    let mut ana = host("ana");
    ana.tags = vec![tag("pool/shared")];
    ana.calendar = Some(vec![block(instant(9, 0), instant(9, 30))]);
    let mut bo = host("bo");
    bo.tags = vec![tag("pool/shared")];
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![
            Term::at_least(1, Selector::Tag(tag("pool/shared"))),
            Term::at_least(1, Selector::Tag(tag("pool/shared"))),
        ]),
    );
    let request = request_for(part, vec![ana, bo]);
    assert!(
        engine().validate(&request).is_valid(),
        "the requirement is statically satisfiable: both terms have two candidates"
    );
    let response = engine().find_availability(&request).unwrap();
    assert!(!offer_starts(&response).contains(&instant(9, 0)));
    let excluded = excluded_at(&response, instant(9, 0));
    assert_eq!(
        reasons(excluded),
        [
            (Some("ana"), ReasonCode::Blocked),
            (None, ReasonCode::TermUnfillable)
        ],
        "bo has no reason of his own; the start-level reason explains the start"
    );
}

#[test]
fn past_the_offer_limit_a_start_is_counted_only_when_the_terms_can_be_filled_jointly() {
    // Two terms over the same four hosts, two each: the requirement needs every
    // one of them, and it is statically satisfiable. `dave` is busy across the
    // middle of the search window, so the starts at 10:30 and 11:00 have three
    // eligible hosts — enough for each term on its own, never for both together —
    // while the starts either side of them have all four.
    //
    // With one offer asked for, those mid-window starts are answered by the
    // feasibility check alone, so the truncation flag, the date counts and the
    // exclusions all depend on that check agreeing with the full assignment.
    let mut dave = host("dave");
    dave.calendar = Some(vec![block(instant(10, 30), instant(11, 15))]);
    let hosts = vec![host("ana"), host("bo"), host("cy"), dave];
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![
            Term::at_least(2, Selector::True),
            Term::at_least(2, Selector::True),
        ]),
    );

    let mut cut = request_for(part, hosts);
    cut.limit = Some(1);
    assert!(engine().validate(&cut).is_valid());
    let response = engine().find_availability(&cut).unwrap();
    assert_eq!(
        offer_starts(&response),
        [instant(9, 0)],
        "one offer, and it needs all four hosts"
    );
    assert_eq!(
        response.offers[0].events[0].assignment,
        [
            vec!["ana".to_owned(), "bo".to_owned()],
            vec!["cy".to_owned(), "dave".to_owned()]
        ],
        "the term-ordered tie-break over zero weights, and the third and fourth \
         ranked hosts are part of the only assignment"
    );
    assert!(
        response.truncated,
        "the starts from 11:30 on are fillable, so the answer is truncated"
    );
    for start in [instant(10, 30), instant(11, 0)] {
        assert_eq!(
            reasons(excluded_at(&response, start)),
            [
                (Some("dave"), ReasonCode::Blocked),
                (None, ReasonCode::TermUnfillable)
            ],
            "{start} cannot fill both terms with three hosts, and is not truncated away"
        );
    }
    assert_eq!(
        response
            .dates
            .iter()
            .map(|count| count.offers)
            .collect::<Vec<_>>(),
        [4],
        "four bookable starts, one offered"
    );

    // Raising the limit reports the same starts as offers and changes nothing
    // else, so the feasibility check answered as the full assignment would.
    let mut full = cut.clone();
    full.limit = Some(1_000);
    let full = engine().find_availability(&full).unwrap();
    assert_eq!(
        offer_starts(&full),
        [
            instant(9, 0),
            instant(9, 30),
            instant(10, 0),
            instant(11, 30)
        ]
    );
    assert!(!full.truncated);
    assert_eq!(full.dates, response.dates);
    assert_eq!(full.excluded, response.excluded);
    assert_eq!(full.offers[..1], response.offers);
}

#[test]
fn past_the_offer_limit_an_unavailable_attendee_is_excluded_and_never_offered() {
    // Two terms over the same two hosts, so the requirement is joint, and an
    // attendee busy from 10:00. With one offer asked for, 9:00 is offered and
    // 9:30 is counted past the limit, while every start from 10:00 on is excluded
    // by somebody no choice of host can move. Past the limit nothing is offered,
    // so those starts are answered by the feasibility check alone: the answer has
    // to stay the attendee's own reason and then `attendee_unavailable`, not
    // `term_unfillable`, and not an offer with no host assigned to it.
    let mut busy = attendee("zz");
    busy.calendar = Some(vec![block(instant(10, 0), instant(23, 0))]);
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![
            Term::at_least(1, Selector::True),
            Term::at_least(1, Selector::True),
        ]),
    );
    let mut request = request_for(part, vec![host("xx"), host("yy"), busy]);
    request.limit = Some(1);
    assert!(engine().validate(&request).is_valid());
    let response = engine().find_availability(&request).unwrap();

    assert_eq!(offer_starts(&response), [instant(9, 0)]);
    assert_eq!(
        response.offers[0].events[0].assignment,
        [vec!["xx".to_owned()], vec!["yy".to_owned()]],
        "both terms are filled, by a distinct host each"
    );
    assert!(
        response.truncated,
        "9:30 is fillable, so the limit is what dropped it"
    );
    assert_eq!(
        response
            .dates
            .iter()
            .map(|count| count.offers)
            .sum::<usize>(),
        2,
        "9:00 and 9:30 are both counted before the limit drops one"
    );
    for start in [
        instant(10, 0),
        instant(10, 30),
        instant(11, 0),
        instant(11, 30),
    ] {
        assert_eq!(
            reasons(excluded_at(&response, start)),
            [
                (Some("zz"), ReasonCode::Blocked),
                (None, ReasonCode::AttendeeUnavailable)
            ],
            "{start} is excluded by the attendee rather than offered or dropped"
        );
    }
}

#[test]
fn an_unfillable_requirement_outranks_an_unavailable_attendee_in_the_start_level_reason() {
    // The same shape as the test above, two terms of two over four hosts with
    // `dave` busy across the middle, plus an attendee blocked over that same
    // middle. At 10:30 and 11:00 both start-level reasons apply at once, and the
    // requirement nobody can fill is the one reported: `dave` and the attendee
    // each explain themselves, then the start says `term_unfillable` rather than
    // `attendee_unavailable`. Past the offer limit that answer comes from the
    // feasibility check alone, so this also pins that a joined requirement the
    // check cannot fill is not mistaken for one an attendee merely blocks.
    let mut dave = host("dave");
    dave.calendar = Some(vec![block(instant(10, 30), instant(11, 15))]);
    let mut vip = attendee("zed");
    vip.calendar = Some(vec![block(instant(10, 30), instant(11, 15))]);
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![
            Term::at_least(2, Selector::True),
            Term::at_least(2, Selector::True),
        ]),
    );
    let mut request = request_for(part, vec![host("ana"), host("bo"), host("cy"), dave, vip]);
    request.limit = Some(1);
    assert!(engine().validate(&request).is_valid());
    let response = engine().find_availability(&request).unwrap();

    assert_eq!(
        offer_starts(&response),
        [instant(9, 0)],
        "the attendee is free at the start of the window, so it is offered"
    );
    assert!(
        response.truncated,
        "the starts either side of the middle are fillable"
    );
    for start in [instant(10, 30), instant(11, 0)] {
        assert_eq!(
            reasons(excluded_at(&response, start)),
            [
                (Some("dave"), ReasonCode::Blocked),
                (Some("zed"), ReasonCode::Blocked),
                (None, ReasonCode::TermUnfillable)
            ],
            "{start} reports the requirement it cannot fill, after both blocked \
             participants and instead of the attendee"
        );
    }
}

#[test]
fn a_pool_wider_than_the_total_demand_is_cut_without_losing_the_winning_hosts() {
    // Three overlapping terms wanting four hosts in total out of six, with distinct
    // weights. Only each term's four heaviest hosts can be part of an optimal
    // assignment, so the pools are wider than the cut has to keep: the winning
    // assignment therefore has to survive the two lighter hosts being dropped —
    // and it still needs the third and fourth ranked host of the wide terms.
    let ids = ["h0", "h1", "h2", "h3", "h4", "h5"];
    let mut hosts: Vec<Participant> = ids.iter().map(|id| host(id)).collect();
    // A narrow pool that only two hosts fill, so one term cannot use the heaviest
    // host the others can and the choice costs the total.
    hosts[0].tags = vec![tag("narrow")];
    hosts[4].tags = vec![tag("narrow")];
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![
            Term::at_least(1, Selector::True),
            Term::at_least(2, Selector::True),
            Term::at_least(1, Selector::Tag(tag("narrow"))),
        ]),
    );
    let mut request = request_for(part, hosts);
    request.limit = Some(1);
    for (id, weight) in ids.iter().zip([10, 9, 8, 7, 6, 5]) {
        request.weights.insert((*id).to_owned(), weight);
    }

    let response = engine().find_availability(&request).unwrap();
    let event = &response
        .offers
        .first()
        .expect("a start every term can fill is offered")
        .events[0];
    assert_eq!(
        event.assignment,
        [
            vec!["h1".to_owned()],
            vec!["h2".to_owned(), "h3".to_owned()],
            vec!["h0".to_owned()],
        ],
        "thirty-four is the most four distinct hosts weigh, and the term order breaks \
         the tie"
    );
    // The cut is the search's own bookkeeping: an offer still lists every host
    // each term could have used.
    let every = ids
        .iter()
        .map(|id| (*id).to_owned())
        .collect::<Vec<String>>();
    assert_eq!(
        event.eligible,
        [every.clone(), every, vec!["h0".to_owned(), "h4".to_owned()]]
    );
}

#[test]
fn explain_reports_the_hosts_the_terms_select_and_not_the_rest() {
    let mut ana = host("ana");
    ana.tags = vec![tag("pool/frontend")];
    // `spare` is a host no term selects, so an exclusion does not report him,
    // however unavailable he is.
    let mut spare = host("spare");
    spare.tags = vec![tag("pool/other")];
    spare.calendar = Some(vec![block(instant(0, 0), instant(23, 0))]);
    let mut yan = attendee("yan");
    yan.calendar = Some(vec![block(instant(0, 0), instant(23, 0))]);
    let mut zed = attendee("zed");
    zed.calendar = Some(vec![block(instant(0, 0), instant(23, 0))]);
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::at_least(1, Selector::Tag(tag("pool/frontend")))]),
    );
    let request = request_for(part, vec![zed, spare, ana, yan]);
    let response = engine().find_availability(&request).unwrap();

    let excluded = excluded_at(&response, instant(9, 0));
    assert_eq!(
        reasons(excluded),
        [
            (Some("yan"), ReasonCode::Blocked),
            (Some("zed"), ReasonCode::Blocked),
            (None, ReasonCode::AttendeeUnavailable)
        ],
        "attendees and the selected hosts only, sorted by participant ID"
    );
}

// -------------------------------------------------------------- weights ---

#[test]
fn weights_choose_the_assignment_and_ids_break_the_tie() {
    let mut ana = host("ana");
    ana.tags = vec![tag("pool/sales")];
    let mut bo = host("bo");
    bo.tags = vec![tag("pool/sales")];
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::at_least(1, Selector::Tag(tag("pool/sales")))]),
    );
    let mut request = request_for(part, vec![ana, bo]);
    request.limit = Some(1);

    let unweighted = engine().find_availability(&request).unwrap();
    assert_eq!(
        unweighted.offers[0].events[0].assignment,
        [vec!["ana".to_owned()]]
    );

    request.weights.insert("bo".to_owned(), 5);
    let weighted = engine().find_availability(&request).unwrap();
    assert_eq!(
        weighted.offers[0].events[0].assignment,
        [vec!["bo".to_owned()]]
    );

    // A tie goes to the smaller ID, and the eligible list is unaffected.
    request.weights.insert("ana".to_owned(), 5);
    let tied = engine().find_availability(&request).unwrap();
    assert_eq!(
        tied.offers[0].events[0].assignment,
        [vec!["ana".to_owned()]]
    );
    assert_eq!(
        tied.offers[0].events[0].eligible,
        [vec!["ana".to_owned(), "bo".to_owned()]]
    );
}

#[test]
fn weights_never_change_which_starts_are_offered() {
    let mut ana = host("ana");
    ana.tags = vec![tag("pool/sales")];
    ana.calendar = Some(vec![block(instant(10, 0), instant(11, 0))]);
    let mut bo = host("bo");
    bo.tags = vec![tag("pool/sales")];
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::at_least(1, Selector::Tag(tag("pool/sales")))]),
    );
    let mut request = request_for(part, vec![ana, bo]);
    let baseline = offer_starts(&engine().find_availability(&request).unwrap());

    for (ana_weight, bo_weight) in [(0, 10), (10, 0), (-5, 5), (i32::MIN, i32::MAX)] {
        request.weights.insert("ana".to_owned(), ana_weight);
        request.weights.insert("bo".to_owned(), bo_weight);
        assert_eq!(
            offer_starts(&engine().find_availability(&request).unwrap()),
            baseline,
            "weights {ana_weight}/{bo_weight} moved the offered starts"
        );
    }
}

#[test]
fn a_start_whose_eligible_hosts_changed_is_matched_again() {
    // Two terms over one pool, so the assignment is solved jointly and the solve is
    // what a search reuses across a run of starts. The first start is in the middle
    // of `ana`'s block, so only `bo` and `cy` can take it; by the second she is free
    // again and the heaviest assignment includes her. An answer reused from the
    // previous start would name hosts who cannot take this one.
    let mut ana = host("ana");
    ana.tags = vec![tag("pool/sales")];
    ana.calendar = Some(vec![block(instant(10, 30), instant(11, 30))]);
    let mut bo = host("bo");
    bo.tags = vec![tag("pool/sales")];
    let mut cy = host("cy");
    cy.tags = vec![tag("pool/sales")];
    let pool = Selector::Tag(tag("pool/sales"));
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![
            Term::at_least(1, pool.clone()),
            Term::at_least(1, pool),
        ]),
    );
    let mut request = request_for(part, vec![ana, bo, cy]);
    request.goal.candidates = Some(vec![instant(10, 30), instant(11, 30)]);
    request.weights.insert("ana".to_owned(), 10);
    request.weights.insert("bo".to_owned(), 5);
    request.weights.insert("cy".to_owned(), 1);

    let response = engine().find_availability(&request).unwrap();
    let assignments: Vec<Vec<Vec<String>>> = response
        .offers
        .iter()
        .map(|offer| offer.events[0].assignment.clone())
        .collect();
    assert_eq!(
        assignments,
        [
            vec![vec!["bo".to_owned()], vec!["cy".to_owned()]],
            vec![vec!["ana".to_owned()], vec!["bo".to_owned()]],
        ]
    );
}

// ------------------------------------------------------------- validate ---

fn error_codes(request: &Request) -> Vec<&'static str> {
    let mut codes: Vec<&'static str> = engine()
        .validate(request)
        .errors
        .iter()
        .map(opportune::Diagnostic::code)
        .collect();
    codes.sort_unstable();
    codes
}

#[test]
fn a_weight_for_an_absent_participant_is_a_validation_error() {
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("xx")]),
    );
    let mut request = request_for(part, vec![host("xx")]);
    request.weights.insert("nobody".to_owned(), 4);
    assert_eq!(error_codes(&request), ["participant_unknown"]);
    assert_eq!(
        engine().find_availability(&request).unwrap_err().code(),
        "invalid_request"
    );
}

#[test]
fn a_requirement_that_cannot_be_filled_jointly_is_rejected_even_when_every_term_could() {
    // Each term has two candidates and needs one; the pools are the same two
    // people, so two distinct hosts cannot fill both terms.
    let mut ana = host("ana");
    ana.tags = vec![tag("pool/shared")];
    let mut bo = host("bo");
    bo.tags = vec![tag("pool/shared")];
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![
            Term::at_least(1, Selector::Tag(tag("pool/shared"))),
            Term::at_least(2, Selector::Tag(tag("pool/shared"))),
        ]),
    );
    let request = request_for(part, vec![ana, bo]);
    assert_eq!(error_codes(&request), ["requirement_unfillable"]);
}

#[test]
fn a_part_asking_for_no_host_is_refused_so_no_cut_is_ever_empty() {
    // The search apportions each term's pool against the requirement's total host
    // demand, so a requirement whose demand is zero would cut every pool to
    // nothing and quietly offer nothing at all. Validation refuses it first, and
    // it counts the same `Term::slots()` the apportionment does, so the two cannot
    // disagree about zero.
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::at_least(0, Selector::True)]),
    );
    let request = request_for(part, vec![host("ana")]);
    assert_eq!(error_codes(&request), ["requirement_unfillable"]);
    assert!(engine().find_availability(&request).is_err());

    // A zero-count term beside a real one leaves the demand to the real term and
    // contributes an empty list to the assignment rather than swallowing a host.
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![
            Term::at_least(0, Selector::True),
            Term::at_least(1, Selector::True),
        ]),
    );
    let request = request_for(part, vec![host("ana"), host("bo")]);
    assert!(engine().validate(&request).is_valid());
    let response = engine().find_availability(&request).unwrap();
    assert_eq!(
        response.offers[0].events[0].assignment,
        [Vec::new(), vec!["ana".to_owned()]]
    );
}

#[test]
fn a_count_no_pool_could_fill_is_rejected_whatever_the_target_width() {
    // A caller's `at_least` count is a `u32` the engine adds up across terms, so
    // two of them at the bound exceed a 32-bit `usize`. The sum saturates rather
    // than wrapping or panicking, so the request is answered by the size cap
    // before any matching runs.
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![
            Term::at_least(u32::MAX, Selector::True),
            Term::at_least(u32::MAX, Selector::True),
        ]),
    );
    let request = request_for(part, vec![host("xx")]);
    assert_eq!(
        error_codes(&request),
        ["requirement_slots_limit"],
        "the size cap answers a requirement past it without matching anything"
    );

    // With the cap raised past the demand, the saturated count reaches the per-term
    // check, which refuses each term for asking for more hosts than exist.
    let unlimited = Engine::builder()
        .limits(Limits::default().with_max_requirement_slots(usize::MAX))
        .build()
        .unwrap();
    assert_eq!(
        error_codes_for(&unlimited, &request),
        ["requirement_unfillable", "requirement_unfillable"],
        "one error per term that asks for more hosts than exist"
    );
    assert_eq!(
        engine().find_availability(&request).unwrap_err().code(),
        "invalid_request"
    );
}

#[test]
fn an_attendee_cannot_fill_a_requirement_term() {
    let mut zed = attendee("zed");
    zed.tags = vec![tag("pool/shared")];
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::at_least(1, Selector::Tag(tag("pool/shared")))]),
    );
    let request = request_for(part, vec![zed]);
    assert_eq!(error_codes(&request), ["requirement_unfillable"]);
}

// ----------------------------------------------------------- check_slot ---

#[test]
fn check_slot_rejects_a_host_repeated_across_terms() {
    let mut ana = host("ana");
    ana.tags = vec![tag("pool/shared")];
    let mut bo = host("bo");
    bo.tags = vec![tag("pool/shared")];
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![
            Term::at_least(1, Selector::Tag(tag("pool/shared"))),
            Term::at_least(1, Selector::Tag(tag("pool/shared"))),
        ]),
    );
    let request = request_for(part, vec![ana, bo]);
    let err = engine()
        .check_slot(
            &request,
            &proposal(instant(9, 0), vec![vec!["ana"], vec!["ana"]]),
        )
        .unwrap_err();
    assert_eq!(err.code(), "proposal_host_duplicated");

    // Two distinct hosts are a well-formed proposal, and the booking is valid.
    let check = engine()
        .check_slot(
            &request,
            &proposal(instant(9, 0), vec![vec!["ana"], vec!["bo"]]),
        )
        .unwrap();
    assert!(check.valid);
}

#[test]
fn check_slot_judges_every_assigned_host_and_every_attendee_in_id_order() {
    let mut ana = host("ana");
    ana.tags = vec![tag("pool/shared")];
    ana.calendar = Some(vec![block(instant(9, 0), instant(9, 30))]);
    let mut bo = host("bo");
    bo.tags = vec![tag("pool/shared")];
    bo.calendar = Some(vec![block(instant(9, 0), instant(9, 30))]);
    let mut zed = attendee("zed");
    zed.calendar = Some(vec![block(instant(9, 0), instant(9, 30))]);
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![
            Term::at_least(1, Selector::Tag(tag("pool/shared"))),
            Term::at_least(1, Selector::Tag(tag("pool/shared"))),
        ]),
    );
    let request = request_for(part, vec![bo, zed, ana]);
    let check = engine()
        .check_slot(
            &request,
            &proposal(instant(9, 0), vec![vec!["bo"], vec!["ana"]]),
        )
        .unwrap();
    assert!(!check.valid);
    assert_eq!(
        check
            .reasons
            .iter()
            .map(|reason| reason.participant.as_deref())
            .collect::<Vec<_>>(),
        [Some("ana"), Some("bo"), Some("zed")],
        "every failing participant, in ascending ID order"
    );
}

// ------------------------------------------------------- available_only ---

#[test]
fn available_only_turns_stated_spans_into_one_attendee_and_a_utc_window() {
    let spans = [
        Span::new(instant(10, 0), instant(11, 0)).unwrap(),
        // Overlapping the first, so the two coalesce.
        Span::new(instant(10, 30), instant(12, 0)).unwrap(),
    ];
    let (candidate, rule) =
        presets::available_only("cand", &spans).expect("the stated spans fit the ceiling");

    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("xx")]),
    );
    let mut request = request_for(part, vec![host("xx"), candidate]);
    request.rules.push(rule);
    let response = engine().find_availability(&request).unwrap();

    assert_eq!(
        offer_starts(&response),
        [
            instant(10, 0),
            instant(10, 30),
            instant(11, 0),
            instant(11, 30)
        ],
        "only the stated availability is bookable"
    );
    // A stated span is a window, not a meeting: it gets no clearance and no
    // rest, and an exclusion cites the preset's rule.
    let excluded = excluded_at(&response, instant(9, 30));
    assert_eq!(
        reasons(excluded),
        [
            (Some("cand"), ReasonCode::OutsideWindow),
            (None, ReasonCode::AttendeeUnavailable)
        ]
    );
    assert_eq!(
        excluded.reasons[0].rule.as_deref(),
        Some("available_only:cand"),
        "the reason cites the preset's rule ID"
    );
}

#[test]
fn available_only_for_two_attendees_composes_without_a_duplicate_rule_id() {
    // Each attendee states different availability, so the offers are the
    // intersection: either rule alone would offer a start the other forbids.
    let ana_spans = [Span::new(instant(10, 0), instant(11, 0)).unwrap()];
    let bob_spans = [Span::new(instant(10, 30), instant(11, 30)).unwrap()];
    let (ana, ana_rule) =
        presets::available_only("ana", &ana_spans).expect("the stated spans fit the ceiling");
    let (bob, bob_rule) =
        presets::available_only("bob", &bob_spans).expect("the stated spans fit the ceiling");
    assert_eq!(ana_rule.id, "available_only:ana");
    assert_eq!(bob_rule.id, "available_only:bob");

    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("xx")]),
    );
    let mut request = request_for(part, vec![host("xx"), ana, bob]);
    request.rules.push(ana_rule);
    request.rules.push(bob_rule);

    assert!(
        engine().validate(&request).errors.is_empty(),
        "two attendees' availability is two rules, not one ID twice"
    );
    assert_eq!(
        offer_starts(&engine().find_availability(&request).unwrap()),
        [instant(10, 30)],
        "both attendees' stated availability is in force, not just one of them"
    );
}

#[test]
fn available_only_splits_a_multi_day_span_at_utc_midnight() {
    // 22:00 to 02:00 the next day.
    let from = instant(22, 0);
    let until = Instant::from_minutes(instant(22, 0).minutes() + 4 * 60).unwrap();
    let spans = [Span::new(from, until).unwrap()];
    let (_candidate, rule) =
        presets::available_only("cand", &spans).expect("the stated spans fit the ceiling");
    let window = rule.window().unwrap();

    assert_eq!(window.dates().len(), 2, "one key per UTC date");
    assert_eq!(window.dates()[0].hours()[0].start().to_string(), "22:00");
    assert_eq!(
        window.dates()[0].hours()[0].end().to_string(),
        "24:00",
        "a span ending at midnight writes 24:00, never 00:00"
    );
    assert_eq!(window.dates()[1].hours()[0].start().to_string(), "00:00");
    assert_eq!(window.dates()[1].hours()[0].end().to_string(), "02:00");
}

#[test]
fn available_only_with_no_stated_spans_offers_nothing() {
    // A candidate who submitted nothing is available at no time. The preset
    // writes a window closed everywhere rather than no rule at all, so the
    // attendee blocks every start instead of being silently ignored.
    let (candidate, rule) =
        presets::available_only("cand", &[]).expect("no stated span is too long");
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("xx")]),
    );
    let mut request = request_for(part, vec![host("xx"), candidate]);
    request.rules.push(rule);
    let response = engine().find_availability(&request).unwrap();

    assert!(response.offers.is_empty());
    assert!(!response.excluded.is_empty());
    for entry in &response.excluded {
        assert_eq!(
            reasons(entry),
            [
                (Some("cand"), ReasonCode::OutsideWindow),
                (None, ReasonCode::AttendeeUnavailable)
            ]
        );
    }
}

#[test]
fn available_only_keeps_separate_spans_on_one_date_apart() {
    let spans = [
        Span::new(instant(9, 0), instant(10, 0)).unwrap(),
        Span::new(instant(14, 0), instant(15, 0)).unwrap(),
    ];
    let (_candidate, rule) =
        presets::available_only("cand", &spans).expect("the stated spans fit the ceiling");
    let window = rule.window().unwrap();
    assert_eq!(window.dates().len(), 1);
    assert_eq!(window.dates()[0].hours().len(), 2);
}

#[test]
fn available_only_rejects_stated_availability_past_the_ceiling() {
    // One dated entry is written per UTC day of the stated spans, so an availability
    // without a ceiling turns one argument into millions of entries written before
    // the engine is ever called. The ceiling counts the whole availability: ten
    // years still models, and a minute more does not.
    let from = instant(0, 0);
    let ceiling = Instant::from_minutes(from.minutes() + 3660 * 24 * 60).expect("inside");
    let (_, accepted) = presets::available_only(
        "cand",
        &[Span::new(from, ceiling).expect("ten years is non-empty")],
    )
    .expect("ten years is the most stated availability accepted");
    assert_eq!(
        accepted.window().unwrap().dates().len(),
        3660,
        "one key per UTC day of the span, and no more"
    );

    let over = Instant::from_minutes(ceiling.minutes() + 1).expect("inside");
    let rejected = presets::available_only("cand", &[Span::new(from, over).unwrap()])
        .expect_err("a minute over the ceiling is refused");
    assert_eq!(rejected.code(), "window_availability_too_long");

    // Spans that each fit the ceiling still add up to twice as many keys, and the
    // preset pays for every one of them before the engine sees the rule.
    let later = Instant::from_minutes(ceiling.minutes() + 24 * 60).expect("inside");
    let later_end = Instant::from_minutes(later.minutes() + 3660 * 24 * 60).expect("inside");
    let rejected = presets::available_only(
        "cand",
        &[
            Span::new(from, ceiling).expect("ten years is non-empty"),
            Span::new(later, later_end).expect("ten years is non-empty"),
        ],
    )
    .expect_err("two ten-year spans are twice the availability accepted at once");
    assert_eq!(rejected.code(), "window_availability_too_long");
    assert!(
        rejected.to_string().contains("7320"),
        "the message names the entries the stated availability needs: {rejected}"
    );

    // The ceiling counts the entries written, one per UTC day of every span, not
    // the distinct dates they land on: 611 one-minute slots on each of six days is
    // 3,666 entries over six keys, and the six whole days are the same availability
    // stated compactly. Counting dates instead would let a caller write unbounded
    // entries before the check.
    let at = |day: i64, minute: i64| {
        Instant::from_minutes(from.minutes() + day * 24 * 60 + minute).expect("inside")
    };
    let slots: Vec<Span> = (0..6_i64)
        .flat_map(|day| {
            (0..1222_i64).step_by(2).map(move |minute| {
                Span::new(at(day, minute), at(day, minute + 1)).expect("one minute")
            })
        })
        .collect();
    assert_eq!(
        slots.len(),
        3666,
        "611 one-minute slots on each of six days"
    );
    let rejected = presets::available_only("cand", &slots)
        .expect_err("3,666 entries is over the ceiling, whatever dates they land on");
    assert!(
        rejected.to_string().contains("3666"),
        "the message names the entries the stated availability needs: {rejected}"
    );
    let compact: Vec<Span> = (0..6_i64)
        .map(|day| Span::new(at(day, 0), at(day, 24 * 60)).expect("a whole day"))
        .collect();
    let (_, accepted) = presets::available_only("cand", &compact).expect("six days as whole spans");
    assert_eq!(accepted.window().unwrap().dates().len(), 6);
    assert_eq!(presets::MAX_AVAILABLE_ONLY_DAYS, 3660);
}

// ------------------------------------------------------- required_ranges ---

#[test]
fn required_ranges_cover_every_host_and_attendee_zone_and_leg() {
    let mut zed = attendee("zed");
    zed.zone = name("America/Anchorage");
    zed.zone_periods =
        vec![ZonePeriod::new(name("Asia/Tokyo"), instant(0, 0), instant(23, 0)).unwrap()];
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("xx")]),
    );
    let request = request_for(part, vec![host("xx"), zed]);

    let mut query = RangesQuery::new(request.now, request.goal.clone());
    query.rules.clone_from(&request.rules);
    query.participants.clone_from(&request.participants);
    let ranges = engine().required_ranges(&query).unwrap();

    // The day before the search window's first local date, in every zone either
    // participant occupies, has to be inside the range.
    let window = request.goal.search_window;
    let anchorage = engine()
        .time_zone_database()
        .zone("America/Anchorage")
        .unwrap();
    let tokyo = engine().time_zone_database().zone("Asia/Tokyo").unwrap();
    for zone in [&anchorage, &tokyo] {
        let first_date = zone.zoned(window.start()).date();
        let LocalResolution::Unique(day_before) = zone
            .resolve_local(first_date.plus_days(-1).unwrap(), TimeOfDay::MIDNIGHT)
            .unwrap()
        else {
            panic!("the fixture midnights exist in these zones");
        };
        assert!(
            ranges.calendar.start() <= day_before,
            "the range starts before {day_before}"
        );
    }
}

// ------------------------------------------------------------- ordering ---

#[test]
fn assignment_and_eligible_lists_are_sorted_by_id_within_each_term() {
    let mut hosts = Vec::new();
    for id in ["mia", "bo", "cy"] {
        let mut participant = host(id);
        participant.tags = vec![tag("pool/x")];
        hosts.push(participant);
    }
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![
            Term::at_least(2, Selector::Tag(tag("pool/x"))),
            Term::at_least(1, Selector::Tag(tag("pool/x"))),
        ]),
    );
    let request = request_for(part, hosts);
    let response = engine().find_availability(&request).unwrap();
    let event = &response.offers[0].events[0];
    assert_eq!(
        event.eligible,
        [
            vec!["bo".to_owned(), "cy".to_owned(), "mia".to_owned()],
            vec!["bo".to_owned(), "cy".to_owned(), "mia".to_owned()]
        ]
    );
    assert_eq!(
        event.assignment,
        [
            vec!["bo".to_owned(), "cy".to_owned()],
            vec!["mia".to_owned()]
        ]
    );
}

#[test]
fn reserve_spans_cover_every_assigned_host_sorted_by_participant_then_start() {
    let part = {
        let mut part = Part::new(
            Duration::from_minutes(30),
            Requirement::new(vec![
                Term::person("bo"),
                Term::at_least(1, Selector::Tag(tag("kind/room"))),
            ]),
        );
        part.travel = Travel {
            before: Duration::from_minutes(15),
            after: Duration::from_minutes(20),
            by_participant: BTreeMap::new(),
        };
        part
    };
    let mut room = host("room");
    room.tags = vec![tag("kind/room")];
    let request = request_for(part, vec![host("bo"), room]);
    let response = engine().find_availability(&request).unwrap();
    let reserve = &response.offers[0].events[0].reserve;

    let described: Vec<(&str, Instant)> = reserve
        .iter()
        .map(|entry| (entry.participant.as_str(), entry.span.start()))
        .collect();
    assert_eq!(
        described,
        [
            (
                "bo",
                Instant::from_minutes(instant(9, 0).minutes() - 15).unwrap()
            ),
            (
                "bo",
                Instant::from_minutes(instant(9, 30).minutes()).unwrap()
            ),
            (
                "room",
                Instant::from_minutes(instant(9, 0).minutes() - 15).unwrap()
            ),
            (
                "room",
                Instant::from_minutes(instant(9, 30).minutes()).unwrap()
            ),
        ],
        "travel for every assigned host, by participant then start"
    );
    assert_eq!(
        response.offers[0].events[0].assignment,
        [vec!["bo".to_owned()], vec!["room".to_owned()]]
    );
}

// ---------------------------------------------------------------- limits ---

#[test]
fn limits_still_bound_a_multi_participant_request() {
    let engine = Engine::builder()
        .limits(Limits::default().with_max_participants(2))
        .build()
        .unwrap();
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("xx")]),
    );
    let request = request_for(part, vec![host("xx"), host("yy"), host("zz")]);
    assert_eq!(error_codes_for(&engine, &request), ["participants_limit"]);
}

fn error_codes_for(engine: &Engine, request: &Request) -> Vec<&'static str> {
    let mut codes: Vec<&'static str> = engine
        .validate(request)
        .errors
        .iter()
        .map(opportune::Diagnostic::code)
        .collect();
    codes.sort_unstable();
    codes
}
