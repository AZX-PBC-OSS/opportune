//! Differential probe: generated multi-participant cases with a rest rule whose
//! parameters, booking length and activity shape the generator never emits.
//!
//! The generator's own rest rules are a narrow evening/morning window, so the
//! engine's handling of the awkward parameters a caller can still pass —
//! `late_after` before noon, `not_before` in the evening, a booking long enough to
//! cross midnight, an all-day block's travel, activity that sits around the
//! window's edges, and a clock that shuttles across the date line every few
//! minutes — is only ever exercised by hand-written fixtures. This compares the
//! engine with the independent minute-by-minute oracle on all of it.

use std::sync::OnceLock;

use opportune::Engine;
use opportune::Limits;
use opportune::model::{BlockStatus, Explain, Response};
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

/// The engine this probe runs on: the default limits, with the explanation
/// maximum raised. A one-minute booking over a five-day window produces more
/// reasons than the default budget, and this probe compares the whole exclusion
/// list with the oracle; truncation is the response contract's own behaviour,
/// tested elsewhere, not a difference in rest semantics.
fn probe_engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        Engine::builder()
            .limits(Limits::default().with_max_explain_limit(1_000_000))
            .build()
            .expect("the bundled database builds")
    })
}

fn offered_starts(response: &Response) -> Vec<Instant> {
    response
        .offers
        .iter()
        .map(|offer| offer.start().expect("an offer has an event"))
        .collect()
}

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

/// `case` with an extra rest rule of the given parameters and the booking length
/// replaced.
fn with_rest(case: &A2Case, late_after: u16, not_before: u16, duration: u32) -> A2Case {
    let mut variant = case.clone();
    variant.duration = duration;
    variant.rules.push(A2Rule {
        plan: RulePlan::Rest(RestPlan {
            id: "rest-probe".to_owned(),
            late_after,
            not_before,
            applies_to_host: true,
            applies_to_part: true,
        }),
        who: vec![true; variant.participants.len()],
        applies_to_part: true,
    });
    variant
}

/// How far apart the shuttle's legs start, and how long each one lasts.
const PROBE_LEG_PITCH: i64 = 8;
const PROBE_LEG_MINUTES: i64 = 3;

/// `case` whose first participant's clock shuttles across the date line: `legs`
/// short legs alternating between the two sides of it, so successive legs move
/// the local reading across the date line and cut each local date's `rest`
/// minutes into many segments.
fn shuttled(case: &A2Case, legs: u8) -> A2Case {
    let mut variant = case.clone();
    let start = case.search.0.minutes();
    if let Some(participant) = variant.participants.first_mut() {
        "UTC".clone_into(&mut participant.zones.home);
        participant.zones.periods = (0..legs)
            .map(|leg| {
                let minute =
                    |value: i64| Instant::from_minutes(value).expect("the range is supported");
                let from = start + i64::from(leg) * PROBE_LEG_PITCH;
                let zone = if leg % 2 == 0 {
                    "Etc/GMT+12"
                } else {
                    "Pacific/Kiritimati"
                };
                (
                    zone.to_owned(),
                    minute(from),
                    minute(from + PROBE_LEG_MINUTES),
                )
            })
            .collect();
    }
    variant
}

/// `case` with one extra block (or none) whose shape is chosen by `shape`.
fn mutated(case: &A2Case, late_after: u16, not_before: u16, duration: u32, shape: u8) -> A2Case {
    let mut variant = with_rest(case, late_after, not_before, duration);
    let minute = |value: i64| Instant::from_minutes(value).expect("the range is supported");
    let block = |span: (Instant, Instant), all_day: bool, travel: u32| BlockPlan {
        span,
        status: BlockStatus::Busy,
        all_day,
        tags: Vec::new(),
        event_key: Some("probe".to_owned()),
        travel_before: travel,
        travel_after: travel,
    };
    if let Some(participant) = variant.participants.first_mut() {
        let (start, end) = (case.search.0, case.search.1);
        // A midpoint leg needs a window wide enough that it is non-empty.
        let roomy = end.minutes() - start.minutes() >= 240;
        let extra = match shape {
            1 => {
                // A five-hour evening meeting two days before the window.
                let from = minute(start.minutes() - 2 * 1_440 - 300);
                Some(block((from, minute(from.minutes() + 300)), false, 120))
            }
            2 => {
                // An all-day block on the day before the window, with travel that
                // reaches well past the window's start.
                let from = minute(start.minutes() - 600);
                Some(block((from, minute(from.minutes() + 240)), true, 300))
            }
            3 => {
                // A block straddling the window's start, reaching late in the day
                // through travel.
                Some(block(
                    (minute(start.minutes() - 30), minute(start.minutes() + 30)),
                    false,
                    180,
                ))
            }
            4 if roomy => {
                // A date-line leg over most of the window, with the home zone on
                // the far side of the line and activity before the leg.
                "Etc/GMT+12".clone_into(&mut participant.zones.home);
                participant.zones.periods = vec![(
                    "Pacific/Kiritimati".to_owned(),
                    minute(start.minutes() + 60),
                    minute(start.minutes() + (end.minutes() - start.minutes()) / 2),
                )];
                let from = minute(start.minutes() - 240);
                Some(block((from, minute(from.minutes() + 240)), false, 0))
            }
            5 => {
                // The opposite leg direction: home east of the line, a leg west of
                // it across the window, and activity inside the leg.
                "Pacific/Kiritimati".clone_into(&mut participant.zones.home);
                participant.zones.periods = vec![(
                    "Etc/GMT+11".to_owned(),
                    minute(start.minutes() + 60),
                    minute(start.minutes() + 1_440),
                )];
                let from = minute(start.minutes() + 120);
                Some(block((from, minute(from.minutes() + 180)), false, 60))
            }
            _ => None,
        };
        if let Some(extra) = extra {
            participant
                .calendar
                .get_or_insert_with(Vec::new)
                .push(extra);
        }
    }
    variant
}

/// Runs `variant` through the engine and through the oracle, and asserts the two
/// agree on every offered start and on every exclusion with its reasons.
///
/// A case whose requirement no assignment could ever fill decides nothing, so it
/// is skipped: the oracle's decisions only describe a fillable case.
fn assert_oracle_agrees(variant: &A2Case) -> Result<(), TestCaseError> {
    if !variant.statically_fillable() {
        return Ok(());
    }
    let request = {
        let mut request = variant.to_request();
        request.explain = Some(Explain::new(1_000_000));
        request
    };
    let oracle = A2Oracle::new(variant, database());
    let decisions = oracle.decisions();

    let response = probe_engine()
        .find_availability(&request)
        .expect("a generated case is a valid request");
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

    prop_assert_eq!(
        offered_starts(&response),
        expected_offered,
        "offered starts"
    );
    prop_assert_eq!(
        excluded_reasons(&response),
        expected_excluded,
        "excluded reasons"
    );
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(192))]

    #[test]
    fn the_engine_and_the_oracle_agree_on_awkward_rest_parameters(
        case in a2_case(),
        late_after in prop_oneof![0u16..=2, 300u16..=900, 1_435u16..=1_439],
        not_before in prop_oneof![0u16..=2, 1_100u16..=1_439],
        duration in prop_oneof![1u32..=120, 600u32..=1_500, 2_400u32..=3_000],
        shape in 0u8..=5,
    ) {
        assert_oracle_agrees(&mutated(&case, late_after, not_before, duration, shape))?;
    }

    /// The same comparison over a schedule that crosses the date line every few
    /// minutes, with bookings long enough to span several of the fragments the
    /// engines' late and early sets are cut into.
    #[test]
    fn the_engine_and_the_oracle_agree_across_an_alternating_date_line(
        case in a2_case(),
        late_after in prop_oneof![0u16..=2, 300u16..=900, 1_435u16..=1_439],
        not_before in prop_oneof![0u16..=2, 1_100u16..=1_439],
        duration in prop_oneof![1u32..=120, 600u32..=1_500, 2_400u32..=3_000],
        legs in 1u8..=60,
    ) {
        assert_oracle_agrees(&shuttled(
            &with_rest(&case, late_after, not_before, duration),
            legs,
        ))?;
    }
}
