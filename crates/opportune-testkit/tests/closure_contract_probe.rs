//! Probe: does the documented two-phase fetch reproduce the full answer across
//! cap shapes the shipped closure property does not generate?
//!
//! The shipped closure property fixes one in-hull record, one far out-of-hull
//! copy, two participants in one zone and no zone periods. This probe keeps the
//! divergent shape — an in-hull record that seeds an event key and an out-of-hull
//! copy of the same key with a different own span — and varies everything the
//! `RequiredRanges` contract's load-bearing premise has to survive: a cap read in
//! the event zone, a named zone or the participant's own piecewise schedule; day,
//! week and month buckets; count and minutes; pooled and per-participant; trips
//! across the date line; three participants in different zones; all-day copies; a
//! copy on a participant no cap covers; occurrences naming only outsiders.
//!
//! The case set is required to make the one-pass fetch disagree somewhere, so a
//! generator that never exercises the closure fails rather than passing
//! vacuously.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use opportune::model::{
    Block, Cap, CapMeasure, CapPer, CapPeriod, Explain, Goal, Occurrence, OfferFlag, Participant,
    Proposal, RangesQuery, Request, RequiredRanges, Requirement, ReserveSpan, Rule, RuleKind, Span,
    Term, ZonePeriod,
};
use opportune::reasons::{Reason, ReasonCode};
use opportune::tags::{Selector, Tag};
use opportune::time::{Date, Duration, Instant, RuleZone, TimeOfDay, Weekday, ZoneName};
use opportune::{Diagnostics, Engine, Limits};
use opportune_testkit::fetch::{key_closed_request, one_pass_request};
use proptest::prelude::*;

const ZONE_NAMES: [&str; 4] = [
    "UTC",
    "Asia/Tokyo",
    "America/New_York",
    "Pacific/Kiritimati",
];
const NAMES: [&str; 3] = ["ana", "bo", "cy"];
const DAY_MINUTES: i64 = 1_440;

fn engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        Engine::builder()
            .build()
            .expect("the bundled database builds")
    })
}

fn tag() -> Tag {
    Tag::new("interview").expect("a valid tag")
}

fn zone(name: &str) -> ZoneName {
    ZoneName::new(name).expect("a fixture zone name")
}

fn utc(day: Date, hour: u8, minute: u8) -> Instant {
    Instant::from_utc(day, TimeOfDay::new(hour, minute).expect("a time"))
        .expect("a supported instant")
}

fn plus(instant: Instant, minutes: i64) -> Instant {
    Instant::from_minutes(instant.minutes() + minutes).expect("a supported instant")
}

fn span(start: Instant, minutes: i64) -> Span {
    Span::new(start, plus(start, minutes)).expect("a forward span")
}

/// Whether the in-hull record that seeds the shared key is an occurrence or a
/// block on `ana`.
#[derive(Clone, Copy, Debug)]
enum Anchor {
    Occurrence,
    Block,
}

/// Which of the two same-key records carries the counted tag.
#[derive(Clone, Copy, Debug)]
enum Tagging {
    Neither,
    Anchor,
    Far,
    Both,
}

/// The extra records a case carries beside the shared-key pair.
#[derive(Clone, Copy, Debug)]
enum Extras {
    None,
    /// A counted event whose only record is out of hull, under a key nothing else
    /// shares.
    Unrelated,
    /// An occurrence naming only someone outside the request, whose same-key block
    /// sits out of hull.
    Outsider,
    Both,
}

/// Where the out-of-hull copy of the counted event sits.
#[derive(Clone, Copy, Debug)]
enum Far {
    /// A day before the window, in the same local week and month.
    EarlierDay,
    /// A day after the window.
    LaterDay,
    /// Weeks away: the same bucket only for a monthly cap.
    Distant,
}

#[derive(Clone, Debug)]
struct Case {
    period: CapPeriod,
    measure: CapMeasure,
    scope: CapPer,
    /// `0` = event (goal) zone, `1` = a named zone, `2` = the participant's zone.
    cap_zone: u8,
    week_starts: Weekday,
    zones: [usize; 3],
    /// Whether `ana` spends the window in `zones[1]`.
    trip: bool,
    anchor: Anchor,
    tagging: Tagging,
    far: Far,
    /// `0..3` names a participant; `3` puts the copy on nobody.
    far_owner: usize,
    far_all_day: bool,
    extras: Extras,
}

fn cases() -> impl Strategy<Value = Case> {
    let shape = (
        prop_oneof![
            Just(CapPeriod::Day),
            Just(CapPeriod::Week),
            Just(CapPeriod::Month)
        ],
        prop_oneof![Just(CapMeasure::Count), Just(CapMeasure::Minutes)],
        prop_oneof![Just(CapPer::Pooled), Just(CapPer::Each)],
        0u8..3,
        prop_oneof![Just(Weekday::Monday), Just(Weekday::Sunday)],
    );
    let place = (
        prop::array::uniform3(0usize..4),
        any::<bool>(),
        prop_oneof![Just(Anchor::Occurrence), Just(Anchor::Block)],
        prop_oneof![
            Just(Tagging::Neither),
            Just(Tagging::Anchor),
            Just(Tagging::Far),
            Just(Tagging::Both)
        ],
        prop_oneof![
            Just(Far::EarlierDay),
            Just(Far::LaterDay),
            Just(Far::Distant)
        ],
        0usize..4,
    );
    let extra = (
        any::<bool>(),
        prop_oneof![
            Just(Extras::None),
            Just(Extras::Unrelated),
            Just(Extras::Outsider),
            Just(Extras::Both)
        ],
    );
    (shape, place, extra).prop_map(
        |(
            (period, measure, scope, cap_zone, week_starts),
            (zones, trip, anchor, tagging, far, far_owner),
            (far_all_day, extras),
        )| Case {
            period,
            measure,
            scope,
            cap_zone,
            week_starts,
            zones,
            trip,
            anchor,
            tagging,
            far,
            far_owner,
            far_all_day,
            extras,
        },
    )
}

/// The probe request. `days == 0` keeps the six-hour window and the three fixed
/// candidates; a positive `days` widens the window across bucket boundaries, and
/// `grid` leaves the candidates to the engine so a search walks its own grid.
fn build_with(case: &Case, days: i64, grid: bool) -> Request {
    let day = Date::new(2027, 6, 16).expect("a fixture date");
    let base = utc(day, 9, 0);
    let window = if days == 0 {
        Span::new(base, utc(day, 15, 0)).expect("a window")
    } else {
        Span::new(base, plus(base, days * DAY_MINUTES)).expect("a window")
    };

    let mut goal = Goal::single(
        zone(ZONE_NAMES[0]),
        window,
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("ana")]),
    );
    if !grid {
        goal.candidates = Some((0..3).map(|index| plus(base, 60 * index)).collect());
    }
    goal.parts[0].tags = vec![tag()];

    let mut request = Request::new(utc(day, 0, 0), goal);
    request.explain = Some(Explain::new(1_000));
    add_participants(&mut request, case, base);
    add_records(&mut request, case, base);
    request.rules.push(cap_rule(case));
    request
}

fn build(case: &Case) -> Request {
    build_with(case, 0, false)
}

fn add_participants(request: &mut Request, case: &Case, base: Instant) {
    for (index, name) in NAMES.iter().enumerate() {
        let participant = if index == 0 {
            Participant::new(*name, zone(ZONE_NAMES[case.zones[index]]))
        } else {
            Participant::attendee(*name, zone(ZONE_NAMES[case.zones[index]]))
        };
        let participant = if index == 0 && case.trip {
            participant.with_zone_periods(vec![
                ZonePeriod::new(zone(ZONE_NAMES[1]), plus(base, -180), plus(base, 180))
                    .expect("a trip over the window"),
            ])
        } else {
            participant
        };
        request
            .participants
            .push(participant.with_calendar(Vec::new()));
    }
}

fn add_records(request: &mut Request, case: &Case, base: Instant) {
    // The in-hull record that seeds the shared key.
    let anchor = span(plus(base, 120), 60);
    let anchor_tagged = matches!(case.tagging, Tagging::Anchor | Tagging::Both);
    match case.anchor {
        Anchor::Occurrence => {
            let mut occurrence = Occurrence::new(
                "evt_shared",
                anchor,
                vec![NAMES[1].to_owned(), NAMES[2].to_owned()],
            )
            .expect("the occurrence names participants");
            if anchor_tagged {
                occurrence = occurrence.with_tags(vec![tag()]);
            }
            request.occurrences.push(occurrence);
        }
        Anchor::Block => {
            let mut block = Block::new(anchor);
            block.event_key = Some("evt_shared".to_owned());
            if anchor_tagged {
                block.tags = vec![tag()];
            }
            request.participants[0]
                .calendar
                .get_or_insert_with(Vec::new)
                .push(block);
        }
    }

    // The out-of-hull copy of the same key, with a different own span.
    if case.far_owner < NAMES.len() {
        let offset = match case.far {
            Far::EarlierDay => -DAY_MINUTES - 120,
            Far::LaterDay => DAY_MINUTES + 120,
            Far::Distant => 30 * DAY_MINUTES,
        };
        let mut block = Block::new(span(plus(base, offset), 30));
        block.event_key = Some("evt_shared".to_owned());
        block.all_day = case.far_all_day;
        if matches!(case.tagging, Tagging::Far | Tagging::Both) {
            block.tags = vec![tag()];
        }
        request.participants[case.far_owner]
            .calendar
            .get_or_insert_with(Vec::new)
            .push(block);
    }

    if matches!(case.extras, Extras::Unrelated | Extras::Both) {
        let mut block = Block::new(span(plus(base, 5 * DAY_MINUTES), 30));
        block.event_key = Some("evt_unrelated".to_owned());
        block.tags = vec![tag()];
        request.participants[2]
            .calendar
            .get_or_insert_with(Vec::new)
            .push(block);
    }

    if matches!(case.extras, Extras::Outsider | Extras::Both) {
        // An occurrence naming only people outside the request, with a same-key
        // block out of hull: dropping the occurrence would charge the block to its
        // own participant instead.
        request.occurrences.push(
            Occurrence::new(
                "evt_outsider",
                span(plus(base, 300), 60),
                vec!["zed".to_owned()],
            )
            .expect("an occurrence may name someone outside the request")
            .with_tags(vec![tag()]),
        );
        let mut block = Block::new(span(plus(base, DAY_MINUTES + 300), 30));
        block.event_key = Some("evt_outsider".to_owned());
        block.tags = vec![tag()];
        request.participants[1]
            .calendar
            .get_or_insert_with(Vec::new)
            .push(block);
    }
}

fn cap_rule(case: &Case) -> Rule {
    let (per, cap_zone) = match (case.scope, case.cap_zone) {
        // A pooled cap cannot read the participant zone; that is a validation
        // error rather than a shape to probe.
        (CapPer::Pooled, 0 | 2) => (CapPer::Pooled, RuleZone::Event),
        (CapPer::Pooled, _) => (CapPer::Pooled, RuleZone::Named(zone("Asia/Tokyo"))),
        (CapPer::Each, 0) => (CapPer::Each, RuleZone::Event),
        (CapPer::Each, 1) => (CapPer::Each, RuleZone::Named(zone("Asia/Tokyo"))),
        (CapPer::Each, _) => (CapPer::Each, RuleZone::Participant),
    };
    Rule::new(
        "cap",
        RuleKind::Cap(Cap::new(
            Selector::Tag(tag()),
            case.measure,
            // For minutes, the anchor's 60 plus the booking's 30 exactly reaches
            // the limit, so only an out-of-hull extension pushes it over. For a
            // count, one existing event plus the booking exceeds it.
            match case.measure {
                CapMeasure::Minutes => 90,
                CapMeasure::Count => 1,
            },
            case.period,
            per,
            cap_zone,
            case.week_starts,
        )),
    )
}

fn ranges_of(request: &Request) -> RequiredRanges {
    let mut query = RangesQuery::new(request.now, request.goal.clone());
    query.rules.clone_from(&request.rules);
    query.participants.clone_from(&request.participants);
    engine()
        .required_ranges(&query)
        .expect("the probe query resolves")
}

/// The same plan from a different search window: how a caller replans a
/// confirmation whose start lies outside the search's own window.
fn ranges_over(request: &Request, window: Span) -> RequiredRanges {
    let mut goal = request.goal.clone();
    goal.search_window = window;
    let mut query = RangesQuery::new(request.now, goal);
    query.rules.clone_from(&request.rules);
    query.participants.clone_from(&request.participants);
    engine()
        .required_ranges(&query)
        .expect("the replanned query resolves")
}

type SlotAnswer = (bool, Vec<Reason>, Vec<OfferFlag>, Vec<ReserveSpan>);

fn slot_answer(request: &Request, start: Instant) -> SlotAnswer {
    let check = engine()
        .check_slot(
            request,
            &Proposal::single("main", start, vec![vec!["ana".to_owned()]]),
        )
        .expect("the probe request and proposal are valid");
    (check.valid, check.reasons, check.flags, check.reserve)
}

fn answers(request: &Request) -> (usize, usize, String, Vec<SlotAnswer>) {
    let response = engine()
        .find_availability(request)
        .expect("a valid probe request");
    let slots = request
        .goal
        .candidates
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|start| slot_answer(request, start))
        .collect();
    (
        response.offers.len(),
        response.dates.iter().map(|date| date.offers).sum(),
        format!("{:?}", response.excluded),
        slots,
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(600))]

    #[test]
    fn key_closure_matches_full_data_across_cap_shapes(case in cases()) {
        let request = build(&case);
        prop_assume!(engine().validate(&request).errors.is_empty());

        let ranges = ranges_of(&request);
        let full = answers(&request);
        let closed = answers(&key_closed_request(&request, &ranges));
        prop_assert_eq!(&closed.0, &full.0, "offer count differs for {:?}", case);
        prop_assert_eq!(&closed.1, &full.1, "date counts differ for {:?}", case);
        prop_assert_eq!(&closed.2, &full.2, "exclusions differ for {:?}", case);
        prop_assert_eq!(&closed.3, &full.3, "SlotCheck differs for {:?}", case);
    }
}

// The same closure assertion across a multi-day window with the engine's own
// candidate grid, so week and month bucket boundaries are crossed and many more
// starts are judged than the three fixed candidates.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(120))]

    #[test]
    fn key_closure_matches_full_data_across_a_multi_day_window(case in cases()) {
        let request = build_with(&case, 5, true);
        prop_assume!(engine().validate(&request).errors.is_empty());
        let ranges = ranges_of(&request);
        let full = engine().find_availability(&request).expect("a valid request");
        let closed = engine()
            .find_availability(&key_closed_request(&request, &ranges))
            .expect("a valid request");
        prop_assert_eq!(&closed.offers, &full.offers, "offers differ for {:?}", case);
        prop_assert_eq!(&closed.dates, &full.dates, "date counts differ for {:?}", case);
        prop_assert_eq!(&closed.excluded, &full.excluded, "exclusions differ for {:?}", case);
    }
}

/// The same case set must make the one-pass fetch disagree somewhere, or the
/// closure assertions above are vacuous.
#[test]
fn the_generated_cases_exercise_the_one_pass_divergence() {
    let valid = AtomicUsize::new(0);
    let divergences = AtomicUsize::new(0);
    let mut runner = proptest::test_runner::TestRunner::new(ProptestConfig::with_cases(600));
    runner
        .run(&cases(), |case| {
            let request = build(&case);
            if engine().validate(&request).errors.is_empty() {
                valid.fetch_add(1, Ordering::Relaxed);
                let ranges = ranges_of(&request);
                if answers(&one_pass_request(&request, &ranges)) != answers(&request) {
                    divergences.fetch_add(1, Ordering::Relaxed);
                }
            }
            Ok(())
        })
        .expect("the case set generates valid requests");
    let valid = valid.load(Ordering::Relaxed);
    assert!(valid > 0, "no generated case was valid");
    assert!(
        divergences.load(Ordering::Relaxed) > 0,
        "the case set never made the one-pass fetch disagree ({valid} valid cases), so the \
         closure assertions prove nothing"
    );
}

// ---------------------------------------------------------------------------
// The two scope probes for the fetch contract.
//
// The hulls are planned from the request's own search window, so they do not
// travel with a confirmation outside it: the contract says to replan. And the
// full-domain fallback is only evaluable with limits raised to cover it. Both are
// behavioural: they run the engine against full data and against a fetch built
// from the documented plan, not against the documentation's wording.
// ---------------------------------------------------------------------------

/// The contracted confirmation flow for a start outside the search window: plan
/// from the request's own goal, observe that those hulls answer the outer slot
/// differently from full data, then replan with a window containing the slot and
/// apply the same key closure — which answers it as full data does.
#[test]
fn a_replanned_fetch_answers_a_slot_outside_the_search_window_as_full_data() {
    let day = Date::new(2027, 6, 16).expect("a fixture date");
    let later = Date::new(2027, 6, 20).expect("a fixture date");
    let base = utc(day, 9, 0);

    let mut goal = Goal::single(
        zone("UTC"),
        Span::new(base, utc(day, 15, 0)).expect("a window"),
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("ana")]),
    );
    goal.candidates = Some(vec![plus(base, 60)]);
    goal.parts[0].tags = vec![tag()];
    let mut request = Request::new(utc(day, 0, 0), goal);
    request.explain = Some(Explain::new(1_000));
    request
        .participants
        .push(Participant::new("ana", zone("UTC")).with_calendar(Vec::new()));
    request.occurrences.push(
        Occurrence::new("evt_1", span(utc(later, 10, 0), 60), vec!["ana".to_owned()])
            .expect("the occurrence names ana")
            .with_tags(vec![tag()]),
    );
    request.rules.push(Rule::new(
        "cap",
        RuleKind::Cap(Cap::new(
            Selector::Tag(tag()),
            CapMeasure::Count,
            1,
            CapPeriod::Day,
            CapPer::Each,
            RuleZone::Participant,
            Weekday::Monday,
        )),
    ));

    // The search's own plan is scoped to its window: those hulls do not answer a
    // confirmation outside it the way full data does.
    let slot = utc(later, 14, 0);
    let closed = key_closed_request(&request, &ranges_of(&request));
    assert_ne!(
        slot_answer(&closed, slot),
        slot_answer(&request, slot),
        "the search's hulls must not be assumed to travel with an out-of-window \
         confirmation"
    );

    // The contracted repair: plan again from a window containing the slot, then
    // apply the same initial fetch and one-round key closure.
    let window = Span::new(utc(later, 13, 0), utc(later, 15, 0)).expect("a window");
    let replanned = ranges_over(&request, window);
    assert!(
        replanned.occurrences.is_some(),
        "a replanned window that touches the capped bucket still needs its \
         occurrence hull"
    );
    let replanned_closed = key_closed_request(&request, &replanned);
    assert_eq!(
        slot_answer(&replanned_closed, slot),
        slot_answer(&request, slot),
        "a fetch planned from a window containing the confirmed start must answer \
         it as full data does"
    );
}

/// The documented full-domain fallback needs limits raised to cover it. A
/// calendar can exceed `max_blocks_per_participant` and an occurrence list can
/// exceed `max_occurrences`; each shape is refused at the published defaults and
/// evaluable through `find_availability` and `check_slot` once limits cover it.
fn fallback_request() -> (Request, Instant) {
    let day = Date::new(2027, 6, 16).expect("a fixture date");
    let base = utc(day, 9, 0);
    let start = plus(base, 60);
    let mut goal = Goal::single(
        zone("UTC"),
        Span::new(base, utc(day, 15, 0)).expect("a window"),
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("ana")]),
    );
    goal.candidates = Some(vec![start]);
    goal.parts[0].tags = vec![tag()];
    let mut request = Request::new(utc(day, 0, 0), goal);
    request.explain = Some(Explain::new(1_000));
    request.rules.push(Rule::new(
        "cap",
        RuleKind::Cap(Cap::new(
            Selector::Tag(tag()),
            CapMeasure::Count,
            1,
            CapPeriod::Day,
            CapPer::Each,
            RuleZone::Participant,
            Weekday::Monday,
        )),
    ));
    (request, start)
}

/// An engine whose limits cover both fallback shapes.
fn raised_fallback_engine() -> Engine {
    Engine::builder()
        .limits(
            Limits::default()
                .with_max_blocks_per_participant(20_000)
                .with_max_occurrences(60_000),
        )
        .build()
        .expect("the bundled database builds")
}

fn assert_refused(diagnostics: &Diagnostics, expected: &str, shape: &str) {
    assert!(
        diagnostics
            .errors
            .iter()
            .any(|error| error.code() == expected),
        "the published defaults must refuse the {shape} fallback with \
         `{expected}`: {:?}",
        diagnostics
            .errors
            .iter()
            .map(|error| (error.code(), error.path()))
            .collect::<Vec<_>>()
    );
}

fn assert_admissible(diagnostics: &Diagnostics, shape: &str) {
    assert!(
        diagnostics.errors.is_empty(),
        "the {shape} fallback must be admissible once limits cover it: {:?}",
        diagnostics
            .errors
            .iter()
            .map(|error| (error.code(), error.path()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn the_block_heavy_full_domain_fallback_is_evaluable_with_raised_limits() {
    let (mut request, start) = fallback_request();

    // Two tagged blocks on every weekday for twenty years — 10,438 records for
    // one participant, a long-tenured calendar and still a fifth of the 125-year
    // domain the fallback asks the caller to fetch.
    let mut blocks: Vec<Block> = Vec::new();
    let mut date = Date::new(2005, 1, 3).expect("a Monday");
    let end = Date::new(2025, 1, 3).expect("a fixture date");
    while date < end {
        if !matches!(date.weekday(), Weekday::Saturday | Weekday::Sunday) {
            for hour in [9u8, 11] {
                let mut block = Block::new(span(
                    Instant::from_utc(date, TimeOfDay::new(hour, 0).expect("a time"))
                        .expect("a supported instant"),
                    60,
                ));
                block.tags = vec![tag()];
                blocks.push(block);
            }
        }
        date = date.plus_days(1).expect("a supported date");
    }
    request
        .participants
        .push(Participant::new("ana", zone("UTC")).with_calendar(blocks));

    assert_refused(&engine().validate(&request), "blocks_limit", "block-heavy");

    let raised = raised_fallback_engine();
    assert_admissible(&raised.validate(&request), "block-heavy");
    let availability = raised
        .find_availability(&request)
        .expect("the block-heavy fallback is evaluated");
    assert!(
        availability
            .offers
            .iter()
            .any(|offer| offer.start() == Some(start)),
        "the block-heavy fallback leaves the candidate free, so a search must \
         offer it"
    );
    let check = raised
        .check_slot(
            &request,
            &Proposal::single("main", start, vec![vec!["ana".to_owned()]]),
        )
        .expect("the block-heavy fallback confirms a proposal");
    assert!(
        check.valid,
        "the block-heavy fallback leaves the candidate free, so confirming it \
         must be valid: {:?}",
        check.reasons
    );
}

#[test]
fn the_occurrence_heavy_full_domain_fallback_is_evaluable_with_raised_limits() {
    let (mut request, start) = fallback_request();
    // No calendar blocks: the occurrence list alone has to reach the fallback's
    // record count. 50,001 unique tagged occurrences, one minute apart, exceed
    // the published `max_occurrences`; the first falls on the candidate's day, so
    // evaluation must charge it to the cap rather than read the list as empty.
    request
        .participants
        .push(Participant::new("ana", zone("UTC")).with_calendar(Vec::new()));
    let day = Date::new(2027, 6, 16).expect("a fixture date");
    let base = utc(day, 9, 0);
    for index in 0..50_001i64 {
        request.occurrences.push(
            Occurrence::new(
                format!("evt_{index}"),
                span(plus(base, -(index + 1)), 30),
                vec!["ana".to_owned()],
            )
            .expect("the occurrence names ana")
            .with_tags(vec![tag()]),
        );
    }

    assert_refused(
        &engine().validate(&request),
        "occurrences_limit",
        "occurrence-heavy",
    );

    let raised = raised_fallback_engine();
    assert_admissible(&raised.validate(&request), "occurrence-heavy");
    let availability = raised
        .find_availability(&request)
        .expect("the occurrence-heavy fallback is evaluated");
    assert!(
        availability.excluded.iter().any(|excluded| excluded
            .reasons
            .iter()
            .any(|reason| reason.code == ReasonCode::CapReached)),
        "the occurrence-heavy fallback must charge the candidate's day to the \
         cap, not read the occurrence list as empty"
    );
    let check = raised
        .check_slot(
            &request,
            &Proposal::single("main", start, vec![vec!["ana".to_owned()]]),
        )
        .expect("the occurrence-heavy fallback confirms a proposal");
    assert!(
        !check.valid
            && check
                .reasons
                .iter()
                .any(|reason| reason.code == ReasonCode::CapReached),
        "the occurrence-heavy fallback must confirm the candidate's day is at its \
         cap: {:?}",
        check.reasons
    );
}
