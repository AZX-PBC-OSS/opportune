//! Probes for the request-wide travel envelope: three questions that reading the
//! diff could not settle.
//!
//! 1. The search now offers starts the part-wide envelope withheld, so a grid
//!    search that produced exactly `max_candidate_starts` starts before can now
//!    exceed the cap and be rejected. Pinned at a lowered cap (cheap) and at the
//!    published default (the same arithmetic one order of magnitude up).
//! 2. `judged_range` still pads by the part-wide envelope while `required_ranges`
//!    now pads by the request-wide one. A part default of a week that every
//!    request participant overrides to zero therefore leaves the judged range
//!    days wider than the fetched hull. Probed with blocks in that difference and
//!    a `rest` rule that excludes starts: the fetched (stripped) request must
//!    answer exactly as the full one.
//! 3. `range_edge_probes::an_overridden_fetch_envelope_preserves_rest_exclusions_after_stripping`
//!    only discriminates if its older block lies between the two hulls, so that
//!    geometry is pinned here against both hulls computed from the same fixture.

#![cfg(feature = "bundled-tzdb")]

use opportune::Engine;
use opportune::Limits;
use opportune::model::{
    Block, Goal, Participant, RangesQuery, Request, Requirement, Span, Term, Travel,
};
use opportune::presets;
use opportune::time::{Duration, Instant, TimeOfDay, ZoneName};

fn name(zone: &str) -> ZoneName {
    ZoneName::new(zone).unwrap()
}

fn span(start: Instant, end: Instant) -> Span {
    Span::new(start, end).unwrap()
}

fn block(start: Instant, end: Instant) -> Block {
    Block::new(span(start, end))
}

fn host(id: &str, zone: &str, blocks: Vec<Block>) -> Participant {
    let mut participant = Participant::new(id, name(zone));
    participant.calendar = Some(blocks);
    participant
}

fn at(minutes: i64) -> Instant {
    Instant::from_minutes(minutes).unwrap()
}

/// A request whose only difference from `main`'s behaviour is the envelope the
/// candidate walk reads: `before` is the part default and `override_before` is
/// what the one participant asks for. On `origin/main` the envelope was
/// `largest_before()`, the part-wide maximum, which equals the default here.
fn minute_grid_request(
    default_before: Duration,
    override_before: Duration,
    minutes: i64,
) -> Request {
    let mut goal = Goal::single(
        name("UTC"),
        span(Instant::MIN, at(Instant::MIN.minutes() + minutes)),
        Duration::from_minutes(1),
        Requirement::new(vec![Term::person("rich")]),
    );
    // No explicit list: the default grid, whose step is the one-minute duration.
    goal.candidates = None;
    goal.parts[0].travel = Travel::new(default_before, Duration::ZERO).with_participant(
        "rich",
        override_before,
        Duration::ZERO,
    );
    let mut request = Request::new(Instant::MIN, goal);
    request.participants = vec![host("rich", "UTC", Vec::new())];
    request
}

/// The candidate cap counts the starts the engine can actually offer. Narrowing
/// the envelope to the request-wide travel stops withholding the starts an unused
/// part default pushed off the front of the range, so a request that validated
/// before can now exceed `max_candidate_starts` and be rejected outright.
///
/// The control below is the same request with the participant keeping the part
/// default — exactly the envelope `origin/main` computed — so the pair isolates
/// the narrowing as the cause of the verdict flip.
#[test]
fn narrowing_the_envelope_flips_the_candidate_limit_verdict() {
    let engine = Engine::builder()
        .limits(Limits::default().with_max_candidate_starts(10))
        .build()
        .unwrap();
    let default = Duration::from_minutes(1);

    // Every start is representable once the default is overridden away, so an
    // eleven-minute window produces eleven starts: one above the cap.
    let narrowed = minute_grid_request(default, Duration::ZERO, 11);
    let err = engine
        .find_availability(&narrowed)
        .expect_err("eleven representable starts exceed a cap of ten");
    assert!(
        err.to_string().contains("candidate starts"),
        "the rejection is the candidate cap: {err}"
    );

    // The participant keeping the default withholds the first minute only, which
    // is what `origin/main` did for the overridden request too: ten starts, valid.
    let withheld = minute_grid_request(default, default, 11);
    let response = engine
        .find_availability(&withheld)
        .expect("ten starts are within the cap");
    let starts: Vec<Instant> = response
        .offers
        .iter()
        .filter_map(opportune::Offer::start)
        .collect();
    assert_eq!(starts.len(), 10, "{starts:?}");
    assert_eq!(starts.first(), Some(&at(Instant::MIN.minutes() + 1)));

    // The same arithmetic at the published cap: a window of one more minute than
    // the cap, and the minute the old envelope withheld was exactly the one that
    // kept the count at the limit.
    let published = Engine::builder().build().unwrap();
    let at_the_cap = minute_grid_request(default, Duration::ZERO, 100_001);
    let err = published
        .find_availability(&at_the_cap)
        .expect_err("100,001 representable starts exceed the published cap");
    assert!(
        err.to_string().contains("more than 100000"),
        "the refusal is the candidate cap at its published value: {err}"
    );
}

/// Whether a block's occupied span — its own span widened by its travel — overlaps
/// `hull`, which is the fetch obligation `RequiredRanges` states for calendar
/// blocks.
fn occupied_overlaps(block: &Block, hull: Span) -> bool {
    let start = block
        .span
        .start()
        .minutes()
        .saturating_sub(block.travel_before.minutes());
    let end = block
        .span
        .end()
        .minutes()
        .saturating_add(block.travel_after.minutes());
    start < hull.end().minutes() && hull.start().minutes() < end
}

/// The judged range stays part-wide while the fetched hull is now request-wide, so
/// a part default of a week every participant overrides to zero leaves the engine
/// judging minutes the caller was never asked to fetch. Both answers must still
/// match: the starts a search can offer have their occupied spans inside the
/// padded window, and every record that can reach one has an occupied span
/// overlapping the hull.
#[test]
fn the_narrower_fetch_still_answers_for_the_untouched_wider_judged_range() {
    let engine = Engine::builder().build().unwrap();
    let day = Instant::MIN.minutes() + 100 * 24 * 60;
    let week = Duration::from_hours(24 * 7);
    let mut goal = Goal::single(
        name("UTC"),
        span(at(day), at(day + 3 * 60)),
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("rich")]),
    );
    goal.candidates = None;
    goal.parts[0].travel =
        Travel::new(week, week).with_participant("rich", Duration::ZERO, Duration::ZERO);
    let rules = vec![presets::rest(
        TimeOfDay::new(18, 0).unwrap(),
        TimeOfDay::new(10, 0).unwrap(),
    )];

    // `in_the_band` sits six and a half days before the window: inside the
    // part-wide judged range and inside the hull the unused default used to widen,
    // outside the request-wide hull. `late_before` is the evening activity a `rest`
    // rule reads for a start on the window's first local date.
    let in_the_band = block(
        at(day - 6 * 24 * 60 - 12 * 60),
        at(day - 6 * 24 * 60 - 11 * 60),
    );
    let late_before = block(at(day - 24 * 60 + 18 * 60), at(day - 24 * 60 + 19 * 60));
    let mut request = Request::new(Instant::MIN, goal.clone());
    request.participants = vec![host(
        "rich",
        "UTC",
        vec![in_the_band.clone(), late_before.clone()],
    )];
    request.rules = rules.clone();
    request.explain = Some(opportune::model::Explain::new(1_000));

    let query = RangesQuery::new(request.now, goal.clone())
        .with_participants(request.participants.clone())
        .with_rules(rules.clone());
    let ranges = engine.required_ranges(&query).unwrap();

    // The scenario is the one the change describes: the hull is a whole day
    // narrower than the part-wide envelope would have made it.
    let mut wide_goal = goal.clone();
    wide_goal.parts[0].travel = Travel::new(week, week);
    let wide = engine
        .required_ranges(
            &RangesQuery::new(request.now, wide_goal)
                .with_participants(request.participants.clone())
                .with_rules(rules),
        )
        .unwrap();
    assert_eq!(
        ranges.calendar.start().minutes() - wide.calendar.start().minutes(),
        7 * 24 * 60,
        "the unused default still widens the part-wide envelope by its own length"
    );

    let full = engine.find_availability(&request).unwrap();
    assert!(
        full.excluded.iter().any(|excluded| excluded
            .reasons
            .iter()
            .any(|reason| reason.code == opportune::reasons::ReasonCode::Rest)),
        "the previous evening's rest activity excludes the early starts: {:?}",
        full.excluded
    );

    // The fetch the contract states: blocks whose occupied span overlaps the hull.
    let mut stripped = request.clone();
    for participant in &mut stripped.participants {
        if let Some(blocks) = &mut participant.calendar {
            blocks.retain(|block| occupied_overlaps(block, ranges.calendar));
        }
    }
    assert_eq!(
        stripped.participants[0].blocks().len(),
        1,
        "the band block is outside the narrowed hull and the late block inside it: {:?}",
        stripped.participants[0].blocks()
    );
    assert!(
        !occupied_overlaps(&in_the_band, ranges.calendar),
        "the band block is what the narrowing drops"
    );

    let fetched = engine.find_availability(&stripped).unwrap();
    assert_eq!(fetched.offers, full.offers);
    assert_eq!(fetched.excluded, full.excluded);
    assert_eq!(fetched.dates, full.dates);
}

/// The fixture in
/// `range_edge_probes::an_overridden_fetch_envelope_preserves_rest_exclusions_after_stripping`
/// is only discriminating if its older block lies in the one-day band between the
/// two hulls: inside the default-inflated hull and outside the narrowed one. This
/// pins that geometry — a block outside both hulls would let the fixture's
/// `blocks().len() == 1` control pass while proving nothing about the narrowing.
#[test]
fn the_rest_strip_fixture_places_its_older_block_in_the_band_the_narrowing_drops() {
    let engine = Engine::builder().build().unwrap();
    let day = Instant::MIN.minutes() + 100 * 24 * 60;
    let start = at(day + 9 * 60);
    let default = Duration::from_minutes(24 * 60);
    let travel =
        Travel::new(default, default).with_participant("rich", Duration::ZERO, Duration::ZERO);

    // The fixture's window, and its two blocks, reproduced exactly.
    let window = span(at(day + 6 * 60), at(day + 12 * 60));
    let old = block(
        at(day - 7 * 24 * 60 + 18 * 60),
        at(day - 7 * 24 * 60 + 19 * 60),
    );
    let mut goal = Goal::single(
        name("UTC"),
        window,
        Duration::from_minutes(30),
        Requirement::new(vec![Term::at_least(
            1,
            opportune::tags::Selector::Role(opportune::tags::Role::Host),
        )]),
    );
    goal.candidates = Some(vec![start]);
    goal.parts[0].travel = travel;
    let rules = vec![presets::rest(
        TimeOfDay::new(18, 0).unwrap(),
        TimeOfDay::new(10, 0).unwrap(),
    )];
    let mut request = Request::new(Instant::MIN, goal.clone());
    request.participants = vec![host("rich", "UTC", vec![old.clone()])];
    request.rules = rules.clone();

    let narrow = engine
        .required_ranges(
            &RangesQuery::new(request.now, goal.clone())
                .with_participants(request.participants.clone())
                .with_rules(rules.clone()),
        )
        .unwrap();
    let mut wide_goal = goal.clone();
    wide_goal.parts[0].travel = Travel::new(default, default);
    let wide = engine
        .required_ranges(
            &RangesQuery::new(request.now, wide_goal)
                .with_participants(request.participants.clone())
                .with_rules(rules),
        )
        .unwrap();

    assert!(
        occupied_overlaps(&old, wide.calendar),
        "the fixture's older block is inside the default-inflated hull: block {:?}, hull {:?}",
        old.span,
        wide.calendar
    );
    assert!(
        !occupied_overlaps(&old, narrow.calendar),
        "and outside the narrowed one: block {:?}, hull {:?}",
        old.span,
        narrow.calendar
    );
    // The five-day rest reach moves both hulls back; the default's own day is the
    // whole difference between them, and the band it opens is `day - 7d .. day - 6d`.
    assert_eq!(narrow.calendar.start().minutes() - day, -6 * 24 * 60);
    assert_eq!(wide.calendar.start().minutes() - day, -7 * 24 * 60);
    assert_eq!(
        narrow.calendar.start().minutes() - wide.calendar.start().minutes(),
        24 * 60,
        "the band the narrowing drops is one local day wide"
    );
}
