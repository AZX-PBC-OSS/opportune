//! Behavioural probes for the fetch contract a generated client reads.
//!
//! `RequiredRanges` in the domain crate and its JSON Schema description are the
//! caller's fetch obligation. A restatement that drops a rule is worse than no
//! restatement at all: a caller who implements the fetch from it fetches too
//! little, and the engine answers with a false offer or a false exclusion on
//! records the caller never sent.
//!
//! These probes run the engine against full data and against a fetch built from
//! the published rule, so they fail on the consequence rather than on the wording
//! of any document.

use opportune::Engine;
use opportune::model::{
    Block, Explain, Goal, Participant, RangesQuery, Request, Requirement, Span, Term,
};
use opportune::reasons::ReasonCode;
use opportune::tags::{Role, Selector};
use opportune::time::{Date, Duration, Instant, LocalResolution, TimeOfDay, Zone, ZoneName};

fn zone(name: &str) -> ZoneName {
    ZoneName::new(name).expect("a fixture zone name")
}

/// The instant of a UTC date and time.
fn utc(day: (i16, i8, i8), time: (u8, u8)) -> Instant {
    let date = Date::new(day.0, day.1, day.2).expect("a fixture date");
    let time = TimeOfDay::new(time.0, time.1).expect("a fixture time");
    let LocalResolution::Unique(instant) = Zone::utc()
        .resolve_local(date, time)
        .expect("the fixture instant is inside the supported range")
    else {
        panic!("UTC has no local transitions");
    };
    instant
}

/// The starts `response` offers.
fn offers(response: &opportune::Response) -> Vec<Instant> {
    response
        .offers
        .iter()
        .filter_map(opportune::Offer::start)
        .collect()
}

/// The calendar hull is a range of dates and the fetch instruction is about blocks,
/// so the two only agree through the travel the caller put on a block: a block
/// whose own span sits before the hull can still occupy minutes inside it. A
/// caller who fetches the blocks whose own span overlaps the hull reads those
/// minutes as free and is offered a booking the full data excludes.
#[test]
fn a_block_whose_travel_reaches_the_hull_still_blocks_a_booking_inside_it() {
    let engine = Engine::builder()
        .build()
        .expect("the bundled database builds");
    let window =
        Span::new(utc((2027, 3, 9), (0, 0)), utc((2027, 3, 10), (0, 0))).expect("a fixture window");
    let start = utc((2027, 3, 9), (9, 0));

    // A two-day travel hold after an event that ended before the hull begins: the
    // block itself is outside the calendar range, the time it holds is inside it.
    let mut hold = Block::new(
        Span::new(utc((2027, 3, 7), (12, 0)), utc((2027, 3, 7), (13, 0)))
            .expect("a fixture block span"),
    );
    hold.travel_after = Duration::from_minutes(2 * 24 * 60);
    let mut ana = Participant::new("ana", zone("UTC"));
    ana.calendar = Some(vec![hold.clone()]);

    let mut goal = Goal::single(
        zone("UTC"),
        window,
        Duration::from_minutes(60),
        Requirement::new(vec![Term::at_least(1, Selector::Role(Role::Host))]),
    );
    goal.candidates = Some(vec![start]);
    let mut request = Request::new(utc((2027, 3, 8), (0, 0)), goal);
    request.participants = vec![ana];
    request.explain = Some(Explain::new(1_000));

    let mut query = RangesQuery::new(request.now, request.goal.clone());
    query.participants = request.participants.clone();
    let ranges = engine
        .required_ranges(&query)
        .expect("the fixture query is answered");
    assert_eq!(
        ranges.calendar.start(),
        utc((2027, 3, 8), (0, 0)),
        "the hull is the window's civil days plus one either side"
    );
    assert!(
        hold.span.end() < ranges.calendar.start(),
        "the fixture block's own span must sit before the calendar hull, or the \
         own-span fetch would retrieve it: block ends {:?}, hull starts {:?}",
        hold.span.end(),
        ranges.calendar.start()
    );
    let occupied_end =
        Instant::from_minutes(hold.span.end().minutes() + hold.travel_after.minutes())
            .expect("a supported instant");
    assert!(
        occupied_end > ranges.calendar.start(),
        "the travel the caller put on the block must reach inside the hull, or the \
         fixture proves nothing"
    );

    let full = engine
        .find_availability(&request)
        .expect("the fixture request is answered");
    assert!(
        !offers(&full).contains(&start),
        "the block's occupied span covers the booking, so the full data excludes it"
    );
    assert!(
        full.excluded.iter().any(|excluded| excluded
            .reasons
            .iter()
            .any(|reason| reason.code == ReasonCode::Blocked)),
        "the out-of-hull hold, not some other rule, must be what excludes the start"
    );

    // The fetch an own-span reading of the calendar rule produces: the blocks whose
    // own span overlaps the range. It drops the hold, and the engine answers with a
    // booking the full data excludes.
    let own_span_fetch: Vec<Block> = vec![hold]
        .into_iter()
        .filter(|block| {
            block.span.start() < ranges.calendar.end() && block.span.end() > ranges.calendar.start()
        })
        .collect();
    assert!(
        own_span_fetch.is_empty(),
        "the fixture proves nothing unless the own-span fetch drops the hold"
    );
    let mut stripped_request = request.clone();
    stripped_request.participants[0].calendar = Some(own_span_fetch);
    let stripped = engine
        .find_availability(&stripped_request)
        .expect("the stripped request is answered");
    assert!(
        offers(&stripped).contains(&start),
        "an own-span fetch must offer the booking, which is the false offer a \
         calendar rule that omits occupied spans leaves a caller open to"
    );
}
