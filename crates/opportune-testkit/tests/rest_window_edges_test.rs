//! Adversarial property: `rest` must read activity that sits *outside* the search
//! window exactly as it reads activity inside it.
//!
//! The generator only emits blocks inside the search window, so the engine's real
//! exposure — a late finish the day before the window, or an early start the day
//! after it — is only covered by hand-written fixtures. This variant places
//! blocking spans up to three days either side of the window, including spans
//! ending one minute before it begins and starting one minute after it ends, and
//! compares the engine with the per-minute oracle that walks every block minute
//! regardless of any range.

use std::sync::OnceLock;

use opportune::Engine;
use opportune::model::{BlockStatus, Proposal, ProposalEvent, Response};
use opportune::reasons::ReasonCode;
use opportune::time::{Instant, TimeZoneDatabase};
use opportune_testkit::a1::{BlockPlan, RestPlan, RulePlan};
use opportune_testkit::a2::{A2Case, A2Oracle, A2Rule, Decision};
use opportune_testkit::generators::a2_case;
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

/// A blocking, non-all-day span of `length` minutes placed `offset` minutes from
/// the given anchor edge of the search window.
fn outside_block(edge: Instant, offset: i64, length: i64) -> BlockPlan {
    let start = Instant::from_minutes(edge.minutes() + offset).expect("the span is supported");
    let end = Instant::from_minutes(start.minutes() + length).expect("the span is supported");
    BlockPlan {
        span: (start, end),
        status: BlockStatus::Busy,
        all_day: false,
        tags: Vec::new(),
        event_key: None,
        travel_before: 0,
        travel_after: 0,
    }
}

/// `case` with blocking activity at the window's edges and days either side, and
/// one rest rule that protects a morning after a late evening.
fn with_activity_around_the_window(case: &A2Case, reach_days: i64) -> A2Case {
    let mut variant = case.clone();
    let reach = reach_days * 1_440;
    let (start, end) = (case.search.0, case.search.1);
    // Ending one minute before the window begins; starting one minute after it
    // ends; and one block on each neighbouring day, far enough out that only a
    // wide read of the calendar can see it.
    let blocks = vec![
        outside_block(start, -61, 60),
        outside_block(end, 1, 60),
        outside_block(start, -reach - 60, 60),
        outside_block(end, reach, 60),
    ];
    if let Some(participant) = variant.participants.first_mut() {
        participant
            .calendar
            .get_or_insert_with(Vec::new)
            .extend(blocks);
    }
    variant.rules.push(A2Rule {
        plan: RulePlan::Rest(RestPlan {
            id: "rest-edge".to_owned(),
            late_after: 18 * 60,
            not_before: 10 * 60,
            applies_to_host: true,
            applies_to_part: true,
        }),
        who: vec![true; variant.participants.len()],
        applies_to_part: true,
    });
    variant
}

fn offered_starts(response: &Response) -> Vec<Instant> {
    response
        .offers
        .iter()
        .map(|offer| offer.start().expect("an offer has an event"))
        .collect()
}

/// The `(participant, code)` pairs each exclusion reports, in report order.
type ReportedReasons = Vec<(Option<String>, ReasonCode)>;

fn excluded_reasons(response: &Response) -> Vec<(Instant, ReportedReasons)> {
    response
        .excluded
        .iter()
        .map(|entry| {
            (
                entry.start,
                entry
                    .reasons
                    .iter()
                    .map(|reason| (reason.participant.clone(), reason.code))
                    .collect(),
            )
        })
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(192))]

    #[test]
    fn activity_either_side_of_the_window_agrees_with_the_oracle(
        case in a2_case(),
        reach_days in 1i64..=3,
    ) {
        let variant = with_activity_around_the_window(&case, reach_days);
        let request = variant.to_request();
        let oracle = A2Oracle::new(&variant, database());
        let decisions = oracle.decisions();

        if !variant.statically_fillable() {
            // A rejected requirement never reaches an answer, so there is nothing
            // for this property to compare.
            return Ok(());
        }

        let response = engine().find_availability(&request).unwrap();
        let expected_offered: Vec<Instant> = decisions
            .iter()
            .filter(|(_, decision)| matches!(decision, Decision::Offered { .. }))
            .map(|(start, _)| *start)
            .collect();
        let expected_excluded: Vec<(Instant, ReportedReasons)> = decisions
            .iter()
            .filter_map(|(start, decision)| match decision {
                Decision::Excluded(reasons) => Some((*start, reasons.clone())),
                Decision::Offered { .. } => None,
            })
            .collect();

        prop_assert_eq!(offered_starts(&response), expected_offered, "offered starts");
        prop_assert_eq!(excluded_reasons(&response), expected_excluded, "excluded reasons");

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
            prop_assert!(
                check.valid,
                "an offered start outside the window's own day failed check_slot at {}",
                event.start
            );
        }
    }
}

#[test]
fn the_outside_activity_really_shapes_the_answer() {
    // The agreement property would pass vacuously if the four blocks it adds never
    // changed an answer. This replays the same fixed corpus and asserts that they
    // do: rest reasons are reported, and removing the outside blocks moves at
    // least one case's offers or exclusions.
    use proptest::test_runner::{Config, TestRng, TestRunner};
    use std::cell::Cell;
    let rest_reasons = Cell::new(0u32);
    let matters = Cell::new(0u32);
    let reached = Cell::new(0u32);
    TestRunner::new_with_rng(
        Config::with_cases(192),
        TestRng::deterministic_rng(Config::default().rng_algorithm),
    )
    .run(&a2_case(), |case| {
        reached.set(reached.get() + 1);
        let variant = with_activity_around_the_window(&case, 2);
        if !variant.statically_fillable() {
            return Ok(());
        }
        let response = engine().find_availability(&variant.to_request()).unwrap();
        for entry in &response.excluded {
            for reason in &entry.reasons {
                if reason.code == ReasonCode::Rest {
                    rest_reasons.set(rest_reasons.get() + 1);
                }
            }
        }
        // Now the same case without the four outside blocks: does the answer move?
        let mut trimmed = variant.clone();
        if let Some(participant) = trimmed.participants.first_mut()
            && let Some(blocks) = &mut participant.calendar
        {
            let before = blocks.len();
            blocks.retain(|b| {
                let (s, e) = (b.span.0.minutes(), b.span.1.minutes());
                s >= case.search.0.minutes() && e <= case.search.1.minutes()
            });
            if blocks.len() != before {
                let trimmed_response = engine().find_availability(&trimmed.to_request()).unwrap();
                if trimmed_response.offers != response.offers
                    || trimmed_response.excluded != response.excluded
                {
                    matters.set(matters.get() + 1);
                }
            }
        }
        Ok(())
    })
    .expect("the corpus runs");
    assert!(reached.get() >= 100, "only {} case(s) ran", reached.get());
    assert!(
        rest_reasons.get() >= 100,
        "only {} rest reason(s) were reported, so the property compared nothing",
        rest_reasons.get()
    );
    assert!(
        matters.get() >= 1,
        "the blocks outside the window never changed an answer"
    );
}
