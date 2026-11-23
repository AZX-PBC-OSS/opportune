//! Property tests: the A1 engine must agree with the whole-request oracle over
//! generated single-participant requests, and every offer must satisfy the
//! specification's invariants.
//!
//! The oracle (`opportune_testkit::a1`) decides each candidate start from the
//! specification text, minute by minute and straight through `jiff` local time,
//! sharing none of the engine's interval or breakpoint code. The properties here
//! cover every A1 rule kind, travel, zone periods, grids and supplied candidates.

use std::sync::OnceLock;

use opportune::Engine;
use opportune::model::{RangesQuery, Response};
use opportune::time::{Date, Instant, TimeZoneDatabase};
use opportune_testkit::a1::{A1Case, A1Oracle, BlockPlan, RulePlan, compare_with_oracle};
use opportune_testkit::generators::a1_case;
use proptest::prelude::*;

fn database() -> &'static TimeZoneDatabase {
    static DATABASE: OnceLock<TimeZoneDatabase> = OnceLock::new();
    DATABASE.get_or_init(|| TimeZoneDatabase::bundled().expect("the bundled database is present"))
}

fn engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        Engine::builder()
            .build()
            .expect("the bundled database builds")
    })
}

fn assert_engine_and_oracle_agree(case: &A1Case) -> Result<(), TestCaseError> {
    compare_with_oracle(case, database(), engine())
}

/// Every offer's occupied span lies inside every applicable window, clear of
/// blackouts and of blocking events both on their own span and at the
/// clearance-widened distance.
fn assert_offer_invariants(case: &A1Case) -> Result<(), TestCaseError> {
    let request = case.to_request();
    let response = engine().find_availability(&request).unwrap();
    let oracle = A1Oracle::new(case, database());
    for offer in &response.offers {
        let start = offer.start().expect("an offer has an event");
        prop_assert!(
            case.search.0 <= start && start < case.search.1,
            "{} is outside the search window",
            start
        );
        let occupied = oracle.occupied_span(start);
        prop_assert!(
            oracle.inside_windows(occupied),
            "occupied span {:?} at {} leaves an applicable window",
            occupied,
            start
        );
        prop_assert_eq!(
            oracle.blackout_overlap(occupied),
            None,
            "occupied span at {} overlaps a blackout",
            start
        );
        prop_assert!(
            oracle.block_overlap(occupied).is_none(),
            "occupied span at {} overlaps a blocking event",
            start
        );
        prop_assert!(
            oracle.clearance_overlap(occupied).is_none(),
            "occupied span at {} is inside a blocking event's clearance",
            start
        );
        prop_assert_eq!(
            offer.calendar_unknown(),
            oracle.calendar_unknown(),
            "calendar_unknown flag at {}",
            start
        );
    }
    Ok(())
}

/// A variant with one more block: a busy block over the window's first hour.
fn with_block(case: &A1Case) -> A1Case {
    let mut variant = case.clone();
    let start = case.search.0;
    let end = Instant::from_minutes(start.minutes() + 60).expect("the window is supported");
    let block = BlockPlan {
        span: (start, end),
        status: opportune::model::BlockStatus::Busy,
        all_day: false,
        tags: Vec::new(),
        event_key: None,
        travel_before: 0,
        travel_after: 0,
    };
    match &mut variant.participant.calendar {
        Some(blocks) => blocks.push(block),
        None => variant.participant.calendar = Some(vec![block]),
    }
    variant
}

/// A variant with one more restrictive rule of the selected kind. `status_policy`
/// is deliberately absent: an ignoring entry may add offers, which is the one
/// documented exception to monotonicity.
fn with_rule(case: &A1Case, kind: u8) -> A1Case {
    let mut variant = case.clone();
    let event = database()
        .zone(&case.event)
        .expect("the case's event zone is in the database");
    let rule = match kind % 4 {
        0 => RulePlan::Window(opportune_testkit::a1::WindowPlan {
            id: "extra".to_owned(),
            zone: opportune_testkit::a1::RuleZonePlan::Event,
            weekly: vec![(0b111_1111, vec![(0, 60)])],
            dates: Vec::new(),
            applies_to_host: true,
            applies_to_part: true,
        }),
        1 => {
            let date: Date = event.zoned(case.search.0).date();
            let next = date.plus_days(1).expect("the window is supported");
            RulePlan::Blackout(opportune_testkit::a1::BlackoutPlan {
                id: "extra".to_owned(),
                zone: opportune_testkit::a1::RuleZonePlan::Event,
                spans: vec![((date, 0), (next, 0))],
                applies_to_host: true,
                applies_to_part: true,
            })
        }
        2 => RulePlan::Clearance {
            id: "extra".to_owned(),
            clearance: opportune_testkit::a1::ClearancePlan {
                before: 120,
                after: 120,
                min_gap: 120,
                applies_to_host: true,
                applies_to_part: true,
            },
        },
        _ => RulePlan::LeadTime(opportune_testkit::a1::LeadTimePlan {
            id: "extra".to_owned(),
            min: 24 * 60,
            max: None,
            applies_to_host: true,
            applies_to_part: true,
        }),
    };
    variant.rules.push(rule);
    variant
}

fn offered(response: &Response) -> Vec<Instant> {
    response
        .offers
        .iter()
        .map(|offer| offer.start().expect("an offer has an event"))
        .collect()
}

/// How far from the search window [`with_data_outside_the_window`] puts its
/// irrelevant data.
///
/// The hull `required_ranges` returns reaches further than the search window on
/// both sides: it pads by the part's travel, clearance and duration, then takes
/// the local-day hull a day out at each edge in the home zone and in every zone
/// a leg puts the participant in, and a day's start can sit almost two days
/// before an instant whose local date it precedes. Measured over 4096 generated
/// cases, that reach peaks at 2972 minutes before the window and 3092 past it,
/// against a structural ceiling of about 3000 (48 hours of day-start rounding
/// plus under two hours of generated travel and clearance). A cap adds a civil
/// month or week to that reach, so the fixture's data sits two months out — well
/// outside the widest hull a generated case can produce while staying inside the
/// supported range: the earliest generated window starts 1975-02-27 and the
/// latest ends 2099-11-11, against `Instant::MIN` of 1975-01-01 and `Instant::MAX`
/// of 2100-01-01, so the injected span is clamped at the edges.
const FAR_MINUTES: i64 = 60 * 1_440;

/// A variant carrying data outside the search window in both of the places that
/// matter.
///
/// Every block and zone leg a generator emits sits inside the window, so a case
/// taken as generated carries nothing outside `required_ranges` and stripping
/// would remove nothing: the property would compare a response with a copy of
/// itself and pass whatever range `required_ranges` returned. This adds
///
/// - a zone leg and two blocks two months out, which no answer can depend on
///   and which the strip must remove harmlessly, and
/// - two blocks in the margin either side of the window: outside the search,
///   but inside the travel, duration and clearance the range has to pad by,
///   which is exactly the data a range that forgot to pad would drop.
///
/// A calendar of `None` becomes a known one here, so every case carries the
/// margin blocks; `every_offer_satisfies_the_a1_invariants` is the property that
/// pins the `calendar_unknown` flag against the oracle.
fn with_data_outside_the_window(case: &A1Case) -> A1Case {
    let mut variant = case.clone();
    let early =
        Instant::from_minutes((case.search.0.minutes() - FAR_MINUTES).max(Instant::MIN.minutes()))
            .expect("a two-month-earlier instant is supported");
    let early_end = Instant::from_minutes(early.minutes() + 60).expect("the leg is supported");
    let late = Instant::from_minutes(
        (case.search.1.minutes() + FAR_MINUTES).min(Instant::MAX.minutes() - 60),
    )
    .expect("a two-month-later instant is supported");
    let late_end = Instant::from_minutes(late.minutes() + 60).expect("the block is supported");

    // A leg that would change every window read in the participant's zone if it
    // covered the search, and is disjoint from the legs the generator emitted.
    variant
        .participant
        .zones
        .periods
        .insert(0, ("Asia/Tokyo".to_owned(), early, early_end));

    let minute = |minutes: i64| Instant::from_minutes(minutes).expect("the range is supported");
    let blocks = variant.participant.calendar.get_or_insert_with(Vec::new);
    // Each injected block carries its own key. A key shared between an event the
    // range keeps and one two months away would make the two one merged event
    // whose start the strip moves, which is not the unrelated data this fixture
    // is about: a real event instance's copies share a span, not a season.
    blocks.push(busy_block("far-past", early, early_end, 60, 60));
    blocks.push(busy_block("far-future", late, late_end, 60, 60));
    // Ending where the window begins, so only the padding before the window can
    // reach it.
    blocks.push(busy_block(
        "margin-before",
        minute(case.search.0.minutes() - 90),
        case.search.0,
        0,
        0,
    ));
    // Beginning one minute past the window, so only the duration, travel and
    // clearance the range pads by after the window can reach it.
    blocks.push(busy_block(
        "margin-after",
        minute(case.search.1.minutes() + 1),
        minute(case.search.1.minutes() + 61),
        0,
        0,
    ));
    variant
}

/// A busy, tagged block over `[span_start, span_end)` with the given travel,
/// which would exclude every start near it.
fn busy_block(
    key: &str,
    span_start: Instant,
    span_end: Instant,
    travel_before: u32,
    travel_after: u32,
) -> BlockPlan {
    BlockPlan {
        span: (span_start, span_end),
        status: opportune::model::BlockStatus::Busy,
        all_day: false,
        tags: vec!["status/focus".to_owned()],
        event_key: Some(key.to_owned()),
        travel_before,
        travel_after,
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn the_engine_and_the_oracle_agree_on_every_candidate(case in a1_case()) {
        assert_engine_and_oracle_agree(&case)?;
    }

    #[test]
    fn every_offer_satisfies_the_a1_invariants(case in a1_case()) {
        assert_offer_invariants(&case)?;
    }

    #[test]
    fn adding_a_block_or_a_restrictive_rule_never_adds_offers(
        case in a1_case(),
        kind in 0u8..5,
    ) {
        let original = engine().find_availability(&case.to_request()).unwrap();
        let variant = if kind == 4 { with_block(&case) } else { with_rule(&case, kind) };
        let changed = engine().find_availability(&variant.to_request()).unwrap();
        for start in offered(&changed) {
            prop_assert!(
                offered(&original).contains(&start),
                "adding kind {kind} added the offer at {start}"
            );
        }
    }

    #[test]
    fn reordering_rules_changes_nothing(case in a1_case()) {
        let original = engine().find_availability(&case.to_request()).unwrap();
        let mut reordered = case.clone();
        reordered.rules.reverse();
        let changed = engine().find_availability(&reordered.to_request()).unwrap();
        prop_assert_eq!(offered(&changed), offered(&original));
        let codes = |response: &Response| -> Vec<Option<opportune::reasons::ReasonCode>> {
            response
                .excluded
                .iter()
                .map(|entry| entry.reasons.first().map(|reason| reason.code))
                .collect()
        };
        prop_assert_eq!(codes(&changed), codes(&original));
    }

    #[test]
    fn data_outside_required_ranges_changes_nothing(case in a1_case()) {
        let padded = with_data_outside_the_window(&case);
        let request = padded.to_request();
        let response = engine().find_availability(&request).unwrap();

        let mut query = RangesQuery::new(request.now, request.goal.clone());
        query.rules.clone_from(&request.rules);
        query.participants.clone_from(&request.participants);
        let ranges = engine().required_ranges(&query).unwrap();

        let mut stripped = request.clone();
        let mut removed = 0usize;
        // Zone periods are not fetched data: the caller supplies the whole
        // schedule, and `required_ranges` never asks for one. Only blocks and
        // occurrences are fetched over the returned ranges, so only those are
        // stripped here.
        for participant in &mut stripped.participants {
            if let Some(blocks) = &mut participant.calendar {
                let count = blocks.len();
                blocks.retain(|block| {
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
                    let inside_calendar = start < ranges.calendar.end().minutes()
                        && ranges.calendar.start().minutes() < end;
                    // A cap counts events over the occurrence hull, so a block a
                    // touched bucket holds has to stay even when it lies outside
                    // the calendar hull.
                    let inside_occurrences = ranges.occurrences.as_ref().is_some_and(|span| {
                        start < span.end().minutes() && span.start().minutes() < end
                    });
                    inside_calendar || inside_occurrences
                });
                removed += count - blocks.len();
            }
        }
        // Without this the property can quietly stop proving anything: a
        // narrower generator, or a hull that grows to cover the whole supported
        // range, would leave nothing to strip and the comparison below would
        // pass on a response compared with itself.
        prop_assert!(
            removed > 0,
            "nothing lay outside {}..{}, so stripping proved nothing",
            ranges.calendar.start(),
            ranges.calendar.end()
        );
        let stripped_response = engine().find_availability(&stripped).unwrap();
        prop_assert_eq!(stripped_response.offers, response.offers);
        prop_assert_eq!(stripped_response.dates, response.dates);
        prop_assert_eq!(stripped_response.excluded, response.excluded);
        prop_assert_eq!(stripped_response.truncated, response.truncated);
        prop_assert_eq!(stripped_response.excluded_truncated, response.excluded_truncated);
    }

    #[test]
    fn date_counts_are_the_same_with_and_without_limit(case in a1_case(), limit in 0usize..5) {
        let request = case.to_request();
        let full = engine().find_availability(&request).unwrap();
        let mut limited = request.clone();
        limited.limit = Some(limit);
        let cut = engine().find_availability(&limited).unwrap();
        prop_assert_eq!(&cut.dates, &full.dates);
        prop_assert_eq!(cut.offers.as_slice(), &full.offers[..limit.min(full.offers.len())]);
        prop_assert_eq!(cut.truncated, full.offers.len() > limit);
    }
}

fn utc_june(day: i8, hour: u8, minute: u8) -> Instant {
    use opportune::time::{LocalResolution, TimeOfDay, Zone};
    let LocalResolution::Unique(instant) = Zone::utc()
        .resolve_local(
            Date::new(2027, 6, day).expect("valid date"),
            TimeOfDay::new(hour, minute).expect("valid time"),
        )
        .expect("UTC resolves")
    else {
        panic!("UTC has no transitions")
    };
    instant
}

fn a1_exclude_fixture() -> (A1Case, Instant, Instant, Instant) {
    use opportune::model::{BlockStatus, CapMeasure, CapPer, CapPeriod};
    use opportune::time::Weekday;
    use opportune_testkit::a1::{CapPlan, ParticipantPlan, RestPlan, RuleZonePlan, TravelPlan};
    use opportune_testkit::specs::ZonePlan;

    let slot_cap = utc_june(14, 12, 0);
    let slot_blocked = utc_june(14, 20, 0);
    let slot_rest = utc_june(15, 9, 0);
    let case = A1Case {
        now: utc_june(14, 8, 0),
        search: (utc_june(14, 8, 0), utc_june(15, 12, 0)),
        event: "UTC".to_owned(),
        display: "UTC".to_owned(),
        duration: 30,
        grid: None,
        candidates: Some(vec![slot_cap, slot_blocked, slot_rest]),
        travel: TravelPlan {
            before: 0,
            after: 0,
        },
        part_tags: vec!["interview".to_owned()],
        participant: ParticipantPlan {
            id: "host".to_owned(),
            zones: ZonePlan {
                home: "UTC".to_owned(),
                periods: Vec::new(),
            },
            calendar: Some(vec![BlockPlan {
                span: (utc_june(14, 20, 0), utc_june(14, 21, 0)),
                status: BlockStatus::Busy,
                all_day: false,
                tags: vec!["interview".to_owned()],
                event_key: Some("move-me".to_owned()),
                travel_before: 0,
                travel_after: 0,
            }]),
        },
        rules: vec![
            RulePlan::Rest(RestPlan {
                id: "rest".to_owned(),
                late_after: 19 * 60,
                not_before: 10 * 60,
                applies_to_host: true,
                applies_to_part: true,
            }),
            RulePlan::Cap(CapPlan {
                id: "cap".to_owned(),
                count: vec!["interview".to_owned()],
                measure: CapMeasure::Count,
                limit: 1,
                period: CapPeriod::Day,
                per: CapPer::Each,
                zone: RuleZonePlan::Participant,
                week_starts: Weekday::Monday,
                applies_to_host: true,
                applies_to_part: true,
            }),
        ],
        exclude_event_keys: Vec::new(),
    };
    (case, slot_cap, slot_blocked, slot_rest)
}

/// Deterministic, non-vacuous guard that `A1Oracle` applies `exclude_event_keys`
/// across blocking (`resolve_blocks`), `rest` activity, and caps (`counted_events`),
/// matching `Engine` both with and without the exclusion.
#[test]
fn exclude_event_keys_updates_blocking_rest_and_cap_in_a1_oracle_and_engine() {
    use opportune::reasons::ReasonCode;
    use opportune_testkit::a1::Decision;

    let (baseline, slot_cap, slot_blocked, slot_rest) = a1_exclude_fixture();

    // Baseline: oracle and engine agree, and all three slots fail for distinct causes.
    assert_engine_and_oracle_agree(&baseline).expect("baseline matches engine");
    let baseline_oracle = A1Oracle::new(&baseline, database());
    assert_eq!(
        baseline_oracle.decide(slot_cap),
        Decision::Excluded(ReasonCode::CapReached),
        "without exclusion, the keyed block consumes the daily cap"
    );
    assert_eq!(
        baseline_oracle.decide(slot_blocked),
        Decision::Excluded(ReasonCode::Blocked),
        "without exclusion, the keyed block occupies its own slot"
    );
    assert_eq!(
        baseline_oracle.decide(slot_rest),
        Decision::Excluded(ReasonCode::Rest),
        "without exclusion, the late keyed block triggers rest the next morning"
    );

    // Excluding the key releases all three constraints in both oracle and engine.
    let mut excluded = baseline;
    excluded.exclude_event_keys = vec!["move-me".to_owned()];
    assert_engine_and_oracle_agree(&excluded).expect("excluded case matches engine");
    let excluded_oracle = A1Oracle::new(&excluded, database());
    for slot in [slot_cap, slot_blocked, slot_rest] {
        assert_eq!(
            excluded_oracle.decide(slot),
            Decision::Offered,
            "excluding `move-me` must make {slot} offered in A1Oracle"
        );
    }
}

/// Pins that the generated `a1_case()` corpus really exercises all three
/// `exclude_event_keys` shapes: the empty list, a list naming a block key the
/// case carries, and a list carrying a key nothing carries.
///
/// The parity property above is only a differential test of the exclusion path
/// while the corpus names keys the case carries: with every list empty the oracle
/// and the engine agree by construction. This fails if that stops being true.
///
/// The thresholds sit well under the counts measured on this fixed 256-case
/// sample — 153 empty lists, 62 cases excluding a carried block key and 66
/// carrying an unknown key — so the generator's own randomness cannot flake them.
#[test]
fn the_generated_corpus_exercises_exclude_event_keys() {
    use proptest::test_runner::{Config, TestRunner};
    use std::cell::Cell;

    let empty = Cell::new(0_u32);
    let hits_a_carried_key = Cell::new(0_u32);
    let has_unknown_key = Cell::new(0_u32);
    let add = |counter: &Cell<u32>, by: u32| counter.set(counter.get() + by);

    // A deterministic sample, so the thresholds below describe a fixed corpus
    // rather than a random one.
    TestRunner::new_with_rng(
        Config::with_cases(256),
        proptest::test_runner::TestRng::deterministic_rng(
            proptest::test_runner::Config::default().rng_algorithm,
        ),
    )
    .run(&a1_case(), |case| {
        if case.exclude_event_keys.is_empty() {
            add(&empty, 1);
            return Ok(());
        }
        let carried: std::collections::BTreeSet<&str> = case
            .participant
            .calendar
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .filter_map(|block| block.event_key.as_deref())
            .collect();
        if case
            .exclude_event_keys
            .iter()
            .any(|key| carried.contains(key.as_str()))
        {
            add(&hits_a_carried_key, 1);
        }
        if case
            .exclude_event_keys
            .iter()
            .any(|key| !carried.contains(key.as_str()))
        {
            add(&has_unknown_key, 1);
        }
        Ok(())
    })
    .expect("the exclude_event_keys corpus probe runs");

    assert!(
        empty.get() >= 80,
        "only {} empty exclusion list(s)",
        empty.get()
    );
    assert!(
        hits_a_carried_key.get() >= 30,
        "only {} case(s) excluded a block key the case carries",
        hits_a_carried_key.get()
    );
    assert!(
        has_unknown_key.get() >= 30,
        "only {} case(s) carried an unknown excluded key",
        has_unknown_key.get()
    );
}
