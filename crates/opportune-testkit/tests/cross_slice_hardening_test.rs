//! Cross-slice probe: one request flow that exercises all three hardened paths
//! together, answered through the engine's public entry points.
//!
//! Each of the three concerns has its own focused coverage; this probe is about
//! their composition, because they meet in one evaluation:
//!
//! * the search window ends at the last supported instant, and the part's default
//!   travel every participant overrides to zero must not withhold the boundary
//!   start (`candidate_starts_limited` and `required_ranges` use the request-wide
//!   envelope, not `Travel::largest_before`/`largest_after`);
//! * a `rest` rule is active through a booking that crosses a local midnight inside
//!   a zone period, so the self-conflict construction decides the exclusion;
//! * a workload cap counts an event whose only complete copy sits outside the
//!   occurrence hull, so the answer is only right after the documented one-round
//!   event-key closure.
//!
//! One test per claim: what data the request needs, what the search and the
//! confirmation answer, and whether the documented two-phase fetch reproduces that
//! answer where a one-pass fetch does not.

use std::sync::OnceLock;

use opportune::Engine;
use opportune::model::{
    Block, Cap, CapMeasure, CapPer, CapPeriod, Explain, Goal, OfferFlag, Participant, Proposal,
    RangesQuery, Request, RequiredRanges, Requirement, ReserveSpan, Rest, Rule, RuleKind, Span,
    Term, Travel, ZonePeriod,
};
use opportune::reasons::{Reason, ReasonCode};
use opportune::tags::{Selector, Tag};
use opportune::time::{
    Date, Duration, Instant, LocalResolution, RuleZone, TimeOfDay, Weekday, Zone, ZoneName,
};
use opportune_testkit::fetch::{key_closed_request, one_pass_request};

fn engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        Engine::builder()
            .build()
            .expect("the bundled database builds")
    })
}

fn zone(name: &str) -> ZoneName {
    ZoneName::new(name).expect("a fixture zone name")
}

fn tag(path: &str) -> Tag {
    Tag::new(path).expect("a fixture tag is a valid tag")
}

/// The instant of a UTC date-time.
fn utc(day: Date, hour: u8, minute: u8) -> Instant {
    let LocalResolution::Unique(instant) = Zone::utc()
        .resolve_local(day, TimeOfDay::new(hour, minute).expect("a fixture time"))
        .expect("the fixture instant is inside the supported range")
    else {
        panic!("UTC has no transitions");
    };
    instant
}

/// `base` shifted by `minutes`.
fn at(base: Instant, minutes: i64) -> Instant {
    Instant::from_minutes(base.minutes() + minutes).expect("a supported instant")
}

/// The fixture's dates and instants: the last supported local day, the far date the
/// out-of-hull copy of the counted event sits on, the window, and the two candidate
/// starts whose fates the three paths decide.
struct Fixture {
    day: Date,
    far: Date,
    window: Span,
    /// The boundary start the unused default travel would withhold.
    boundary: Instant,
    /// The start whose booking itself crosses the local midnight the zone period
    /// creates, so `rest` excludes it.
    rest_blocked: Instant,
}

fn fixture() -> Fixture {
    let day = Date::new(2099, 12, 31).expect("a fixture date");
    let far = Date::new(2099, 11, 1).expect("a fixture date");
    let window = Span::new(utc(day, 18, 0), Instant::MAX).expect("a window ending at the edge");
    Fixture {
        day,
        far,
        window,
        // Thirty minutes ending exactly at the last supported instant: no minute
        // of travel after it exists to reserve.
        boundary: at(Instant::MAX, -30),
        rest_blocked: utc(day, 19, 0),
    }
}

/// A keyed block carrying the counted tag.
fn keyed(span: Span, key: &str) -> Block {
    let mut block = Block::new(span);
    block.event_key = Some(key.to_owned());
    block.tags = vec![tag("interview")];
    block
}

/// The assembled request: an overridden-away default travel, an active `rest` rule
/// that a zone period makes self-conflicting, and a `Count` cap whose event is
/// completed only by a far out-of-hull copy on an attendee.
fn assembled_request() -> Request {
    let Fixture {
        day,
        far,
        window,
        boundary,
        rest_blocked,
    } = fixture();

    // The counted event: a copy inside the occurrence hull and a copy weeks
    // earlier. Both are blocks, so the event's span is their hull and its earliest
    // start lands in an earlier bucket than either candidate's — the cap reaches
    // its limit nowhere the search can start.
    let mut ana = Participant::new("ana", zone("UTC"));
    ana.calendar = Some(vec![keyed(
        Span::new(utc(day, 14, 0), utc(day, 15, 0)).expect("a span"),
        "evt_shared",
    )]);
    // A local midnight inside the window: Kiritimati reads 01-01 from 19:10Z, so a
    // booking that spans that minute reaches a late minute of 12-31 and a
    // protected minute of 01-01 at once.
    ana.zone_periods = vec![
        ZonePeriod::new(
            zone("Pacific/Kiritimati"),
            at(rest_blocked, 10),
            at(rest_blocked, 20),
        )
        .expect("the stay is non-empty"),
    ];
    // The out-of-hull copy of the counted event, on a participant no cap covers.
    let mut bo = Participant::attendee("bo", zone("UTC"));
    bo.calendar = Some(vec![keyed(
        Span::new(utc(far, 9, 0), utc(far, 10, 0)).expect("a span"),
        "evt_shared",
    )]);

    let mut goal = Goal::single(
        zone("UTC"),
        window,
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("ana")]),
    );
    goal.candidates = Some(vec![rest_blocked, boundary]);
    goal.parts[0].tags = vec![tag("interview")];
    // The default is the whole reason the boundary start is interesting: a day of
    // travel before and an hour after, both of which every request participant
    // overrides to nothing.
    goal.parts[0].travel = Travel::new(Duration::from_minutes(24 * 60), Duration::from_minutes(60))
        .with_participant("ana", Duration::ZERO, Duration::ZERO)
        .with_participant("bo", Duration::ZERO, Duration::ZERO);

    let mut request = Request::new(Instant::MIN, goal);
    request.participants = vec![ana, bo];
    request.rules = vec![
        Rule::new(
            "rest",
            RuleKind::Rest(Rest::new(
                TimeOfDay::new(17, 0).expect("a fixture time"),
                TimeOfDay::new(12, 0).expect("a fixture time"),
            )),
        ),
        Rule::new(
            "cap",
            RuleKind::Cap(Cap::new(
                Selector::Tag(tag("interview")),
                CapMeasure::Count,
                1,
                CapPeriod::Day,
                CapPer::Each,
                RuleZone::Participant,
                Weekday::Monday,
            )),
        ),
    ];
    request.explain = Some(Explain::new(100));
    request
}

/// The ranges the engine asks for `request`.
fn required_ranges_of(request: &Request) -> RequiredRanges {
    let mut query = RangesQuery::new(request.now, request.goal.clone());
    query.rules.clone_from(&request.rules);
    query.participants.clone_from(&request.participants);
    engine()
        .required_ranges(&query)
        .expect("the fixture query resolves")
}

/// What `SlotCheck` reports for `start`, without the warning list: a stripped
/// keyless record can legitimately change a warning, but never the answer.
type SlotAnswer = (bool, Vec<Reason>, Vec<OfferFlag>, Vec<ReserveSpan>);

fn slot_answer(request: &Request, start: Instant) -> SlotAnswer {
    let check = engine()
        .check_slot(
            request,
            &Proposal::single("main", start, vec![vec!["ana".to_owned()]]),
        )
        .expect("the fixture request and proposal are valid");
    (check.valid, check.reasons, check.flags, check.reserve)
}

fn offered(response: &opportune::Response) -> Vec<Instant> {
    response
        .offers
        .iter()
        .filter_map(opportune::Offer::start)
        .collect()
}

/// `request` with the part's default travel left in place for `bo` to inherit: the
/// same extra day before and hour after, now a participant's effective travel
/// rather than an unused default.
fn with_inherited_default(request: &Request) -> Request {
    let mut inherited = request.clone();
    inherited.goal.parts[0].travel = Travel::new(
        Duration::from_minutes(24 * 60),
        Duration::from_minutes(60),
    )
    .with_participant("ana", Duration::ZERO, Duration::ZERO);
    inherited
}

/// The request-wide envelopes, not the part's unused default travel, decide what
/// data the fetch needs.
#[test]
fn the_assembled_request_asks_for_the_request_wide_envelope() {
    let request = assembled_request();
    let ranges = required_ranges_of(&request);

    // A request identical but for a zero default asks for the same data: each
    // participant's override is its whole effective travel.
    let mut no_default = request.clone();
    no_default.goal.parts[0].travel = Travel::new(Duration::ZERO, Duration::ZERO)
        .with_participant("ana", Duration::ZERO, Duration::ZERO)
        .with_participant("bo", Duration::ZERO, Duration::ZERO);
    assert_eq!(
        required_ranges_of(&no_default),
        ranges,
        "the default travel every participant overrides must not widen the fetch"
    );

    // Non-vacuity control: the same default the hull ignores is visible in it as
    // soon as a participant inherits it, so the equality above is the request-wide
    // envelope's doing rather than travel never mattering. That inherited hour also
    // withholds the boundary start the search otherwise offers.
    let inherited = with_inherited_default(&request);
    assert!(
        required_ranges_of(&inherited).calendar.start() < ranges.calendar.start(),
        "an inherited default must reach the hull"
    );
    assert!(
        offered(
            &engine()
                .find_availability(&inherited)
                .expect("the control request is valid")
        )
        .is_empty(),
        "an inherited hour of travel after the booking withholds the boundary start"
    );
}

/// The assembled request answers at both ends of the range: the boundary start is
/// offered and confirms, and the booking that crosses its own rest midnight is
/// excluded.
#[test]
fn the_assembled_request_offers_the_boundary_start_and_excludes_the_rest_crossing() {
    let request = assembled_request();
    let Fixture {
        boundary,
        rest_blocked,
        ..
    } = fixture();
    let full = engine()
        .find_availability(&request)
        .expect("the fixture request is valid");

    assert_eq!(
        offered(&full),
        vec![boundary],
        "the start at the end of the range is the one offer: {:?}",
        offered(&full)
    );
    let excluded: Vec<Instant> = full.excluded.iter().map(|entry| entry.start).collect();
    assert_eq!(
        excluded,
        vec![rest_blocked],
        "the self-conflicting booking is the one exclusion"
    );
    assert!(
        full.excluded[0]
            .reasons
            .iter()
            .any(|reason| reason.code == ReasonCode::Rest),
        "the exclusion cites the rest rule: {:?}",
        full.excluded[0].reasons
    );
    assert!(
        slot_answer(&request, boundary).0,
        "the boundary start confirms"
    );
}

/// The documented two-phase fetch answers exactly as the full request did, and the
/// one-pass fetch it replaces would not: the cap needs the out-of-hull copy of the
/// counted event.
#[test]
fn the_key_closed_fetch_matches_full_data_where_one_pass_diverges() {
    let request = assembled_request();
    let Fixture { boundary, .. } = fixture();
    let ranges = required_ranges_of(&request);

    let full = engine()
        .find_availability(&request)
        .expect("the full fixture request is valid");
    let one_pass_request = one_pass_request(&request, &ranges);
    let closed_request = key_closed_request(&request, &ranges);
    let one_pass = engine()
        .find_availability(&one_pass_request)
        .expect("the one-pass fixture request is valid");
    let closed = engine()
        .find_availability(&closed_request)
        .expect("the key-closed fixture request is valid");

    assert_eq!(closed.offers, full.offers, "offers differ");
    assert_eq!(closed.excluded, full.excluded, "exclusions differ");
    assert_eq!(closed.dates, full.dates, "date counts differ");
    assert_eq!(
        slot_answer(&closed_request, boundary),
        slot_answer(&request, boundary),
        "the boundary start's confirmation differs"
    );

    // The closure retrieved the far shared-key record and kept it: it starts before
    // the calendar hull, so only the observed key could have fetched it, and without
    // it the counted event's earliest start is the in-hull copy in the candidates'
    // own bucket.
    assert!(
        closed_request
            .participants
            .iter()
            .flat_map(Participant::blocks)
            .any(|block| {
                block.event_key.as_deref() == Some("evt_shared")
                    && block.span.start() < ranges.calendar.start()
            }),
        "closure must retrieve the out-of-hull copy of the counted event"
    );

    // A one-pass fetch does not have it and rejects the boundary start on the cap,
    // so the parity above is the closure's doing rather than a boundary that is
    // offered whatever the fetch.
    assert!(
        one_pass.offers.is_empty(),
        "the one-pass fetch must diverge: {:?}",
        offered(&one_pass)
    );
    assert!(
        one_pass.excluded.iter().any(|entry| entry.start == boundary
            && entry
                .reasons
                .iter()
                .any(|reason| reason.code == ReasonCode::CapReached)),
        "the one-pass fetch rejects the boundary start on the cap: {:?}",
        one_pass.excluded
    );
    assert!(
        !slot_answer(&one_pass_request, boundary).0,
        "the one-pass request rejects the boundary start"
    );
}

/// The rest exclusion is the self-conflict construction's, not another rule's:
/// without the zone period the booking no longer crosses a local midnight, so the
/// `rest` rule passes and the same start is offered.
#[test]
fn the_rest_exclusion_is_the_crossing_the_zone_period_creates() {
    let mut request = assembled_request();
    let Fixture { rest_blocked, .. } = fixture();
    for participant in &mut request.participants {
        participant.zone_periods.clear();
    }
    let response = engine()
        .find_availability(&request)
        .expect("the fixture request is valid");
    assert!(
        offered(&response).contains(&rest_blocked),
        "without the crossing the start is offered: {:?}",
        offered(&response)
    );
    assert!(
        response
            .excluded
            .iter()
            .all(|entry| entry.start != rest_blocked),
        "no rest exclusion without a midnight crossing"
    );
}
