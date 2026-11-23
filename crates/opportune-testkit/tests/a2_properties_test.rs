//! Property tests: the engine must agree with the multi-participant oracle over
//! generated requests with hosts, attendees, multi-term requirements and weights,
//! and reweighting must move only the assignment. Rest rules are generated too,
//! and a rest-specific variant pins that adding one never adds offers, that
//! moving it changes nothing, and that data outside `required_ranges` cannot
//! change a rest outcome.
//!
//! The oracle (`opportune_testkit::a2`) decides each candidate start from the
//! specification text — minute by minute for time, and by brute-force enumeration
//! for assignment — sharing none of the engine's interval, breakpoint or matching
//! code. Where a generated requirement cannot be filled jointly at all, the
//! property pins `validate`'s rejection instead of an answer.

use std::sync::OnceLock;

use opportune::Engine;
use opportune::model::{BlockStatus, CapMeasure, CapPer, CapPeriod, RangesQuery, Response};
use opportune::reasons::ReasonCode;
use opportune::time::{Instant, TimeZoneDatabase, Weekday};
use opportune_testkit::a1::{BlockPlan, CapPlan, OccurrencePlan, RestPlan, RulePlan, RuleZonePlan};
use opportune_testkit::a2::{
    A2Case, A2Rule, compare_with_oracle, excluded_reasons, offered_starts,
};
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

fn assert_engine_and_oracle_agree(case: &A2Case) -> Result<(), TestCaseError> {
    compare_with_oracle(case, database(), engine())
}

/// A variant of `case` with different weights, keeping every participant.
fn reweighted(case: &A2Case, step: i32) -> A2Case {
    let mut variant = case.clone();
    for (index, participant) in variant.participants.iter_mut().enumerate() {
        let index = i32::try_from(index).expect("a case holds a handful of participants");
        participant.weight = index * step - step;
    }
    variant
}

/// A variant carrying one more rest rule for every participant, either before
/// every generated rule or after them, so a property can rely on rest being
/// present whatever the generator produced.
fn with_rest(case: &A2Case, late_after: u16, not_before: u16, at_front: bool) -> A2Case {
    let mut variant = case.clone();
    let rule = A2Rule {
        plan: RulePlan::Rest(RestPlan {
            id: "rest-extra".to_owned(),
            late_after,
            not_before,
            applies_to_host: true,
            applies_to_part: true,
        }),
        who: vec![true; variant.participants.len()],
        applies_to_part: true,
    };
    if at_front {
        variant.rules.insert(0, rule);
    } else {
        variant.rules.push(rule);
    }
    variant
}

/// The `(participant, code)` pairs each exclusion reports, in report order.
type ReportedReasons = Vec<(Option<String>, ReasonCode)>;

/// The `(participant, code)` pairs each exclusion reports, in report order.
fn exclusion_codes(response: &Response) -> Vec<(Instant, ReportedReasons)> {
    response
        .excluded
        .iter()
        .map(|entry| (entry.start, excluded_reasons(entry)))
        .collect()
}

/// How far outside the search window [`with_data_outside_the_range`] puts its
/// irrelevant data, in minutes: two months, which is beyond the day either side
/// `required_ranges` reaches for rest and beyond a cap's month bucket either side
/// of the window.
const FAR_MINUTES: i64 = 60 * 1_440;

/// A variant carrying zone legs and blocks two months either side of the search
/// window, plus a block in each margin just outside it.
///
/// Every block and leg a generator emits sits inside the window, so a case taken
/// as generated carries nothing outside `required_ranges` and stripping would
/// remove nothing: the property would compare a response with a copy of itself.
fn with_data_outside_the_range(case: &A2Case) -> A2Case {
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
    let minute = |minutes: i64| Instant::from_minutes(minutes).expect("the range is supported");
    let busy = |key: &str, span: (Instant, Instant)| BlockPlan {
        span,
        status: BlockStatus::Busy,
        all_day: false,
        tags: Vec::new(),
        event_key: Some(key.to_owned()),
        travel_before: 0,
        travel_after: 0,
    };
    if let Some(participant) = variant.participants.first_mut() {
        participant
            .zones
            .periods
            .insert(0, ("Asia/Tokyo".to_owned(), early, early_end));
        let blocks = participant.calendar.get_or_insert_with(Vec::new);
        // Each injected block carries its own key: one key shared between a block
        // the range keeps and one two months away would make them one merged
        // event whose start the strip moves, which is not the unrelated data this
        // fixture is about.
        blocks.push(busy("far-past", (early, early_end)));
        blocks.push(busy("far-future", (late, late_end)));
        // Ending where the window begins, and beginning one minute past it: the
        // two margin blocks a range that forgot to pad would drop.
        blocks.push(busy(
            "margin-before",
            (minute(case.search.0.minutes() - 90), case.search.0),
        ));
        blocks.push(busy(
            "margin-after",
            (
                minute(case.search.1.minutes() + 1),
                minute(case.search.1.minutes() + 61),
            ),
        ));
    }
    variant
}

/// A case whose first participant is late on the day before the search window
/// begins, so a rest rule has activity to act on even when the generator put
/// none there.
fn with_activity_before_the_window(case: &A2Case) -> A2Case {
    let mut variant = case.clone();
    if let Some(participant) = variant.participants.first_mut() {
        let start = Instant::from_minutes(case.search.0.minutes() - 120)
            .expect("two hours before the generated window is supported");
        let end = Instant::from_minutes(start.minutes() + 120).expect("the block is supported");
        participant
            .calendar
            .get_or_insert_with(Vec::new)
            .push(BlockPlan {
                span: (start, end),
                status: BlockStatus::Busy,
                all_day: false,
                tags: Vec::new(),
                event_key: None,
                travel_before: 0,
                travel_after: 0,
            });
    }
    variant
}

/// A variant whose first participant holds an all-day copy of an occurrence: one
/// key, the occurrence fixing the span and the block carrying the tag the cap
/// counts. The union of the two is what makes the event count at all, so a model
/// that skipped all-day blocks entirely would answer differently, and one that
/// merged the block's span would report a day of minutes.
fn with_an_all_day_copy_of_an_occurrence(case: &A2Case) -> A2Case {
    let mut variant = case.clone();
    let Some(first) = variant
        .participants
        .first()
        .map(|participant| participant.id.clone())
    else {
        return variant;
    };
    let start = variant.search.0;
    let hour_end = Instant::from_minutes(start.minutes() + 60).expect("the window's first hour");
    let day_end = Instant::from_minutes(start.minutes() + 1_440).expect("a day from the window");
    variant.occurrences = vec![OccurrencePlan {
        event_key: "occ_shared".to_owned(),
        span: (start, hour_end),
        tags: Vec::new(),
        participants: vec![first],
    }];
    variant.part_tags = vec!["interview".to_owned()];
    variant.rules.insert(
        0,
        A2Rule {
            plan: RulePlan::Cap(CapPlan {
                id: "shared-cap".to_owned(),
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
            who: vec![true; variant.participants.len()],
            applies_to_part: true,
        },
    );
    variant.participants[0]
        .calendar
        .get_or_insert_with(Vec::new)
        .push(BlockPlan {
            span: (start, day_end),
            status: BlockStatus::Busy,
            all_day: true,
            tags: vec!["interview".to_owned()],
            event_key: Some("occ_shared".to_owned()),
            travel_before: 0,
            travel_after: 0,
        });
    variant
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn an_all_day_copy_of_an_occurrence_counts_the_same_in_both_models(case in a2_case()) {
        // The generator never draws a block key equal to an occurrence key, so
        // this variant is the only shape that exercises what an all-day block
        // sharing an occurrence's key contributes: tags, and no span or minutes.
        assert_engine_and_oracle_agree(&with_an_all_day_copy_of_an_occurrence(&case))?;
    }

    #[test]
    fn the_engine_and_the_oracle_agree_on_every_candidate(case in a2_case()) {
        assert_engine_and_oracle_agree(&case)?;
    }

    #[test]
    fn changing_weights_never_changes_which_starts_are_offered(case in a2_case(), step in -3i32..=3) {
        let baseline = engine().find_availability(&case.to_request());
        let changed = engine().find_availability(&reweighted(&case, step).to_request());
        match (baseline, changed) {
            (Ok(baseline), Ok(changed)) => {
                prop_assert_eq!(offered_starts(&changed), offered_starts(&baseline));
                // Reweighting moves who is chosen, never who could be: the
                // eligible pools are a property of the request, not the weights.
                let eligible = |response: &Response| -> Vec<Vec<Vec<String>>> {
                    response
                        .offers
                        .iter()
                        .map(|offer| offer.events[0].eligible.clone())
                        .collect()
                };
                prop_assert_eq!(eligible(&changed), eligible(&baseline));
            }
            (Err(baseline), Err(changed)) => prop_assert_eq!(changed.code(), baseline.code()),
            (baseline, changed) => prop_assert!(
                false,
                "reweighting changed whether the request is answerable: {:?} / {:?}",
                baseline.map(|_| ()).map_err(|err| err.code()),
                changed.map(|_| ()).map_err(|err| err.code())
            ),
        }
    }

    #[test]
    fn the_engine_and_the_oracle_agree_with_a_rest_rule(
        case in a2_case(),
        late_after in 17u16..=22,
        not_before in 5u16..=11,
    ) {
        assert_engine_and_oracle_agree(&with_rest(&case, late_after * 60, not_before * 60, false))?;
    }

    #[test]
    fn adding_a_rest_rule_never_adds_offers(
        case in a2_case(),
        late_after in 17u16..=22,
        not_before in 5u16..=11,
    ) {
        let baseline = engine().find_availability(&case.to_request());
        let changed = engine().find_availability(
            &with_rest(&case, late_after * 60, not_before * 60, false).to_request(),
        );
        match (baseline, changed) {
            (Ok(baseline), Ok(changed)) => {
                for start in offered_starts(&changed) {
                    prop_assert!(
                        offered_starts(&baseline).contains(&start),
                        "adding a rest rule added the offer at {start}"
                    );
                }
            }
            (Err(baseline), Err(changed)) => prop_assert_eq!(changed.code(), baseline.code()),
            (baseline, changed) => prop_assert!(
                false,
                "a rest rule changed whether the request is answerable: {:?} / {:?}",
                baseline.map(|_| ()).map_err(|err| err.code()),
                changed.map(|_| ()).map_err(|err| err.code())
            ),
        }
    }

    #[test]
    fn moving_a_rest_rule_changes_neither_offers_nor_reasons(case in a2_case()) {
        let front = engine().find_availability(&with_rest(&case, 18 * 60, 10 * 60, true).to_request());
        let back = engine().find_availability(&with_rest(&case, 18 * 60, 10 * 60, false).to_request());
        match (front, back) {
            (Ok(front), Ok(back)) => {
                prop_assert_eq!(offered_starts(&front), offered_starts(&back));
                prop_assert_eq!(exclusion_codes(&front), exclusion_codes(&back));
            }
            (Err(front), Err(back)) => prop_assert_eq!(front.code(), back.code()),
            (front, back) => prop_assert!(
                false,
                "reordering a rest rule changed whether the request is answerable: {:?} / {:?}",
                front.map(|_| ()).map_err(|err| err.code()),
                back.map(|_| ()).map_err(|err| err.code())
            ),
        }
    }

    #[test]
    fn data_outside_required_ranges_changes_no_rest_outcome(case in a2_case()) {
        // `with_activity_before_the_window` guarantees the rest rule has activity
        // on the neighbouring day, so this compares real answers rather than two
        // copies of a request rest never touches.
        let padded = with_data_outside_the_range(&with_activity_before_the_window(&with_rest(
            &case,
            18 * 60,
            10 * 60,
            false,
        )));
        let request = padded.to_request();
        // A generated requirement can be unfillable, which `validate` rejects; the
        // agreement property pins that rejection, so this one compares answers.
        let Ok(response) = engine().find_availability(&request) else {
            return Ok(());
        };

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
        if let Some(span) = &ranges.occurrences {
            let count = stripped.occurrences.len();
            stripped.occurrences.retain(|occurrence| {
                occurrence.span.start().minutes() < span.end().minutes()
                    && span.start().minutes() < occurrence.span.end().minutes()
            });
            removed += count - stripped.occurrences.len();
        }
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
}

/// The agreement property above returns early for a requirement that cannot be
/// filled at all, pinning `validate`'s rejection instead of an answer. Nothing
/// else would notice if the generator drifted until every case took that branch
/// and the property stopped comparing the engine with the oracle at all, so this
/// pins the shape of a fixed sample: both branches are reached, and the
/// multi-participant paths the property is cited for — several terms, an
/// attendee, a multi-host assignment and each start-level reason — really occur.
///
/// The sample is fixed — a deterministic runner replays the same 512 generated
/// cases — so the thresholds describe one corpus rather than a random one. It
/// holds 127 answerable cases, 385 rejected ones, 349 multi-term, 253 with an
/// attendee, 313 offers holding a two-host term, 12 `term_unfillable` reasons
/// and 320 `attendee_unavailable` reasons; each threshold sits about a quarter
/// under its count.
#[allow(clippy::too_many_lines)] // one corpus walk: splitting it would scatter the counters from their thresholds
#[test]
fn the_generated_corpus_reaches_every_path_the_property_compares() {
    use proptest::test_runner::{Config, TestRunner};
    use std::cell::Cell;
    let answerable = Cell::new(0_u32);
    let rejected = Cell::new(0_u32);
    let multi_term = Cell::new(0_u32);
    let with_attendee = Cell::new(0_u32);
    let multi_host_offers = Cell::new(0_u32);
    let term_unfillable = Cell::new(0_u32);
    let attendee_unavailable = Cell::new(0_u32);
    let offering_and_excluding = Cell::new(0_u32);
    let add = |counter: &Cell<u32>, by: u32| counter.set(counter.get() + by);

    // A deterministic runner keeps this shape guard stable across runs: it
    // samples the same cases every time, so the thresholds below describe a
    // fixed corpus rather than a random one. The agreement properties
    // themselves still use fresh randomness.
    TestRunner::new_with_rng(
        Config::with_cases(512),
        proptest::test_runner::TestRng::deterministic_rng(
            proptest::test_runner::Config::default().rng_algorithm,
        ),
    )
    .run(&a2_case(), |case| {
        if case
            .participants
            .iter()
            .any(|participant| participant.role == opportune::tags::Role::Attendee)
        {
            add(&with_attendee, 1);
        }
        if case.terms.len() >= 2 {
            add(&multi_term, 1);
        }
        if !case.statically_fillable() {
            add(&rejected, 1);
            return Ok(());
        }
        add(&answerable, 1);
        let response = engine().find_availability(&case.to_request()).unwrap();
        for offer in &response.offers {
            if offer.events[0]
                .assignment
                .iter()
                .any(|term| term.len() >= 2)
            {
                add(&multi_host_offers, 1);
            }
        }
        for entry in &response.excluded {
            for reason in &entry.reasons {
                if reason.participant.is_some() {
                    continue;
                }
                match reason.code {
                    ReasonCode::TermUnfillable => add(&term_unfillable, 1),
                    ReasonCode::AttendeeUnavailable => add(&attendee_unavailable, 1),
                    _ => {}
                }
            }
        }
        if !response.offers.is_empty() && !response.excluded.is_empty() {
            add(&offering_and_excluding, 1);
        }
        Ok(())
    })
    .expect("the corpus shape probe runs");

    assert!(
        answerable.get() >= 90,
        "only {} case(s) reached the engine: the agreement property would be \
         comparing nothing",
        answerable.get()
    );
    assert!(
        rejected.get() >= 280,
        "only {} case(s) reached the joint-fillability rejection",
        rejected.get()
    );
    assert!(
        multi_term.get() >= 250,
        "{} multi-term case(s)",
        multi_term.get()
    );
    assert!(
        with_attendee.get() >= 180,
        "{} case(s) carried an attendee",
        with_attendee.get()
    );
    assert!(
        multi_host_offers.get() >= 200,
        "{} offer(s) assigned two hosts to one term",
        multi_host_offers.get()
    );
    assert!(
        offering_and_excluding.get() >= 60,
        "{} case(s) proved both an offer and an exclusion",
        offering_and_excluding.get()
    );
    assert!(
        term_unfillable.get() >= 8,
        "{} start(s) reported `term_unfillable`",
        term_unfillable.get()
    );
    assert!(
        attendee_unavailable.get() >= 220,
        "{} start(s) reported `attendee_unavailable`",
        attendee_unavailable.get()
    );
}

/// The rest properties above would pass vacuously if the generator stopped
/// producing rest rules, or if no generated case ever had activity that a rest
/// rule turns into an exclusion. This pins the shape of a fixed sample so that
/// cannot happen silently.
///
/// The thresholds sit well under the counts measured over repeated 512-case
/// samples (about 59 cases carrying a generated rest rule, 23 to 29 losing an
/// offer to an appended one, and 34 to 60 starts reporting `rest`), so the
/// generator's own randomness cannot flake them.
#[test]
fn the_generated_corpus_reaches_the_rest_paths() {
    use proptest::test_runner::{Config, TestRunner};
    use std::cell::Cell;

    let generated_rule = Cell::new(0_u32);
    let appended_excludes = Cell::new(0_u32);
    let rest_reasons = Cell::new(0_u32);
    let add = |counter: &Cell<u32>, by: u32| counter.set(counter.get() + by);

    // The same fixed sample as the probe above, so the rest paths are counted
    // over a stable corpus.
    TestRunner::new_with_rng(
        Config::with_cases(512),
        proptest::test_runner::TestRng::deterministic_rng(
            proptest::test_runner::Config::default().rng_algorithm,
        ),
    )
    .run(&a2_case(), |case| {
        if case
            .rules
            .iter()
            .any(|rule| matches!(rule.plan, RulePlan::Rest(_)))
        {
            add(&generated_rule, 1);
        }
        let Ok(response) = engine().find_availability(&case.to_request()) else {
            return Ok(());
        };
        for entry in &response.excluded {
            for reason in &entry.reasons {
                if reason.code == ReasonCode::Rest {
                    add(&rest_reasons, 1);
                }
            }
        }
        // Appending a rule the generator may not have produced, with activity
        // guaranteed on the neighbouring day, proves an offer is really
        // removed by rest rather than the property passing on an inert rule.
        let with_rule = with_rest(&case, 18 * 60, 10 * 60, false);
        let with_activity = with_activity_before_the_window(&with_rule);
        let Ok(before) = engine().find_availability(&with_rule.to_request()) else {
            return Ok(());
        };
        let Ok(after) = engine().find_availability(&with_activity.to_request()) else {
            return Ok(());
        };
        if after.offers.len() < before.offers.len() {
            add(&appended_excludes, 1);
        }
        Ok(())
    })
    .expect("the corpus shape probe runs");

    assert!(
        generated_rule.get() >= 30,
        "only {} case(s) carried a generated rest rule",
        generated_rule.get()
    );
    assert!(
        appended_excludes.get() >= 10,
        "only {} case(s) lost an offer to rest",
        appended_excludes.get()
    );
    assert!(
        rest_reasons.get() >= 20,
        "only {} start(s) reported `rest` in a generated case",
        rest_reasons.get()
    );
}

/// The cap properties above would pass vacuously if the generator stopped
/// producing caps, or if no generated case ever had usage a cap turns into an
/// exclusion. This pins the shape of a fixed sample so that cannot happen
/// silently, and counts the minute-measured caps a count cap cannot stand in
/// for, and the capped cases carrying an all-day block, which is the one shape
/// that tells a cap counting every block apart from one that skips them.
#[test]
fn the_generated_corpus_reaches_the_cap_paths() {
    use proptest::test_runner::{Config, TestRunner};
    use std::cell::Cell;

    let generated_rule = Cell::new(0_u32);
    let minute_rule = Cell::new(0_u32);
    let cap_reasons = Cell::new(0_u32);
    let counted_outside = Cell::new(0_u32);
    let capped_all_day = Cell::new(0_u32);
    let add = |counter: &Cell<u32>, by: u32| counter.set(counter.get() + by);

    TestRunner::new_with_rng(
        Config::with_cases(512),
        proptest::test_runner::TestRng::deterministic_rng(
            proptest::test_runner::Config::default().rng_algorithm,
        ),
    )
    .run(&a2_case(), |case| {
        let capped = case
            .rules
            .iter()
            .any(|rule| matches!(rule.plan, RulePlan::Cap(_)));
        for rule in &case.rules {
            if let RulePlan::Cap(plan) = &rule.plan {
                add(&generated_rule, 1);
                if plan.measure == opportune::model::CapMeasure::Minutes {
                    add(&minute_rule, 1);
                }
            }
        }
        // A cap buckets an existing event by its own start in the zone in force
        // there, so a counted event outside the search window is the input shape
        // that tells a hull computed over the window's zones from one computed
        // over the bucket's. Pin that the corpus really carries it.
        if capped
            && (case.participants.iter().any(|participant| {
                participant.calendar.as_ref().is_some_and(|blocks| {
                    blocks
                        .iter()
                        .any(|block| block.span.0 < case.search.0 || block.span.1 > case.search.1)
                })
            }) || case.occurrences.iter().any(|occurrence| {
                occurrence.span.0 < case.search.0 || occurrence.span.1 > case.search.1
            }))
        {
            add(&counted_outside, 1);
        }
        // A cap that skipped the wrong blocks would still be exercised by any
        // calendar; only a capped case holding an all-day block tells the two
        // readings of the counting rule apart.
        if capped
            && case.participants.iter().any(|participant| {
                participant
                    .calendar
                    .as_ref()
                    .is_some_and(|blocks| blocks.iter().any(|block| block.all_day))
            })
        {
            add(&capped_all_day, 1);
        }
        let Ok(response) = engine().find_availability(&case.to_request()) else {
            return Ok(());
        };
        for entry in &response.excluded {
            for reason in &entry.reasons {
                if reason.code == ReasonCode::CapReached {
                    add(&cap_reasons, 1);
                }
            }
        }
        Ok(())
    })
    .expect("the corpus shape probe runs");

    assert!(
        generated_rule.get() >= 60,
        "only {} case(s) carried a generated cap rule",
        generated_rule.get()
    );
    assert!(
        minute_rule.get() >= 10,
        "only {} case(s) carried a minute-measured cap",
        minute_rule.get()
    );
    assert!(
        cap_reasons.get() >= 20,
        "only {} start(s) reported `cap_reached` in a generated case",
        cap_reasons.get()
    );
    assert!(
        counted_outside.get() >= 10,
        "only {} capped case(s) counted an event outside the search window",
        counted_outside.get()
    );
    assert!(
        capped_all_day.get() >= 10,
        "only {} capped case(s) carried an all-day block",
        capped_all_day.get()
    );
}

fn utc_june(day: i8, hour: u8, minute: u8) -> Instant {
    use opportune::time::{Date, LocalResolution, TimeOfDay, Zone};
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

/// Builds a two-host case where `occ-move` merges an untagged occurrence with an
/// all-day block's tag to exhaust a pooled daily cap, while `late-move` on `bo`
/// blocks 20:00 and triggers `rest` the next morning at 09:00.
fn a2_exclude_fixture() -> (A2Case, Instant, Instant, Instant) {
    use opportune::tags::Role;
    use opportune_testkit::a1::{CapPlan, TravelPlan};
    use opportune_testkit::a2::{A2Participant, TermPlan};
    use opportune_testkit::specs::ZonePlan;

    let slot_cap = utc_june(14, 12, 0);
    let slot_blocked = utc_june(14, 20, 0);
    let slot_rest = utc_june(15, 9, 0);
    let case = A2Case {
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
        participants: vec![
            A2Participant {
                id: "ana".to_owned(),
                role: Role::Host,
                zones: ZonePlan {
                    home: "UTC".to_owned(),
                    periods: Vec::new(),
                },
                calendar: Some(vec![BlockPlan {
                    span: (utc_june(14, 0, 0), utc_june(15, 0, 0)),
                    status: BlockStatus::Free,
                    all_day: true,
                    tags: vec!["interview".to_owned()],
                    event_key: Some("occ-move".to_owned()),
                    travel_before: 0,
                    travel_after: 0,
                }]),
                tags: Vec::new(),
                weight: 0,
            },
            A2Participant {
                id: "bo".to_owned(),
                role: Role::Host,
                zones: ZonePlan {
                    home: "UTC".to_owned(),
                    periods: Vec::new(),
                },
                calendar: Some(vec![BlockPlan {
                    span: (utc_june(14, 20, 0), utc_june(14, 21, 0)),
                    status: BlockStatus::Busy,
                    all_day: false,
                    tags: Vec::new(),
                    event_key: Some("late-move".to_owned()),
                    travel_before: 0,
                    travel_after: 0,
                }]),
                tags: Vec::new(),
                weight: 0,
            },
        ],
        terms: vec![TermPlan::Person("bo".to_owned())],
        rules: vec![
            A2Rule {
                plan: RulePlan::Rest(RestPlan {
                    id: "rest".to_owned(),
                    late_after: 19 * 60,
                    not_before: 10 * 60,
                    applies_to_host: true,
                    applies_to_part: true,
                }),
                who: vec![true, true],
                applies_to_part: true,
            },
            A2Rule {
                plan: RulePlan::Cap(CapPlan {
                    id: "pool-cap".to_owned(),
                    count: vec!["interview".to_owned()],
                    measure: CapMeasure::Count,
                    limit: 1,
                    period: CapPeriod::Day,
                    per: CapPer::Pooled,
                    zone: RuleZonePlan::Event,
                    week_starts: Weekday::Monday,
                    applies_to_host: true,
                    applies_to_part: true,
                }),
                who: vec![true, true],
                applies_to_part: true,
            },
        ],
        occurrences: vec![OccurrencePlan {
            event_key: "occ-move".to_owned(),
            span: (utc_june(14, 10, 0), utc_june(14, 11, 0)),
            tags: Vec::new(),
            participants: vec!["ana".to_owned()],
        }],
        exclude_event_keys: Vec::new(),
    };
    (case, slot_cap, slot_blocked, slot_rest)
}

/// Deterministic, non-vacuous guard that `A2Oracle` applies `exclude_event_keys`
/// in `member_events` (blocking, and the `rest` activity read from it) and in
/// `counted_events` (an excluded occurrence, whose event counts only because an
/// all-day block sharing its key lends it a tag), matching `Engine` on every
/// variant.
///
/// The block half of `counted_events` is not discriminated here, and no fixture
/// can: one keyed list excludes an occurrence and any block sharing its key
/// together, so "the excluded block was dropped" and "it had no event left to
/// join" answer identically. The parity properties and their persisted seeds pin
/// that half.
#[test]
fn exclude_event_keys_updates_blocking_rest_and_merged_caps_in_a2_oracle_and_engine() {
    use opportune_testkit::a2::{A2Oracle, Decision};

    let (baseline, slot_cap, slot_blocked, slot_rest) = a2_exclude_fixture();

    assert_engine_and_oracle_agree(&baseline).expect("baseline matches engine");
    let baseline_oracle = A2Oracle::new(&baseline, database());
    assert_eq!(
        baseline_oracle.decide(slot_cap),
        Decision::Excluded(vec![(Some("bo".to_owned()), ReasonCode::CapReached)]),
        "without exclusion, the merged occurrence + all-day tag exhausts the pooled cap"
    );
    assert_eq!(
        baseline_oracle.decide(slot_blocked),
        Decision::Excluded(vec![(Some("bo".to_owned()), ReasonCode::Blocked)]),
        "without exclusion, `late-move` blocks `bo` at 20:00"
    );
    assert_eq!(
        baseline_oracle.decide(slot_rest),
        Decision::Excluded(vec![(Some("bo".to_owned()), ReasonCode::Rest)]),
        "without exclusion, `late-move` triggers rest for `bo` at 09:00"
    );

    // Excluding only `occ-move` lifts the pooled cap on day 14 (12:00 opens),
    // while `late-move` still blocks 20:00 and triggers rest at 09:00 on day 15.
    let mut only_cap = baseline.clone();
    only_cap.exclude_event_keys = vec!["occ-move".to_owned()];
    assert_engine_and_oracle_agree(&only_cap).expect("cap-only exclusion matches engine");
    let only_cap_oracle = A2Oracle::new(&only_cap, database());
    assert!(matches!(
        only_cap_oracle.decide(slot_cap),
        Decision::Offered { .. }
    ));
    assert_eq!(
        only_cap_oracle.decide(slot_blocked),
        Decision::Excluded(vec![(Some("bo".to_owned()), ReasonCode::Blocked)]),
    );
    assert_eq!(
        only_cap_oracle.decide(slot_rest),
        Decision::Excluded(vec![(Some("bo".to_owned()), ReasonCode::Rest)]),
    );

    // Excluding both keys opens every candidate start in both oracle and engine.
    let mut both = baseline;
    both.exclude_event_keys = vec!["occ-move".to_owned(), "late-move".to_owned()];
    assert_engine_and_oracle_agree(&both).expect("both exclusions match engine");
    let both_oracle = A2Oracle::new(&both, database());
    for slot in [slot_cap, slot_blocked, slot_rest] {
        assert!(
            matches!(both_oracle.decide(slot), Decision::Offered { .. }),
            "excluding both keys must make {slot} offered in A2Oracle"
        );
    }
}

/// Pins that the generated `a2_case()` corpus genuinely exercises every
/// `exclude_event_keys` shape: empty lists, lists hitting a block the case
/// carries, lists hitting an occurrence the case carries, and lists carrying a
/// key nothing carries.
///
/// The parity property is only a differential test of the exclusion path while
/// the corpus names keys the case carries: with every list empty the oracle and
/// the engine agree by construction, and a corpus that named only occurrence keys
/// would stop covering an excluded block's blocking and `rest` half. The two
/// carried shapes are counted apart for that reason.
///
/// The thresholds sit well under the counts measured on this fixed 256-case
/// sample — 136 empty lists, 81 naming a carried block key, 67 naming a carried
/// occurrence key and 61 carrying an unknown key — so the generator's own
/// randomness cannot flake them.
#[test]
fn the_generated_corpus_exercises_exclude_event_keys() {
    use proptest::test_runner::{Config, TestRunner};
    use std::cell::Cell;

    let empty = Cell::new(0_u32);
    let hits_a_block_key = Cell::new(0_u32);
    let hits_an_occurrence_key = Cell::new(0_u32);
    let has_unknown_key = Cell::new(0_u32);
    let add = |counter: &Cell<u32>, by: u32| counter.set(counter.get() + by);

    TestRunner::new_with_rng(
        Config::with_cases(256),
        proptest::test_runner::TestRng::deterministic_rng(
            proptest::test_runner::Config::default().rng_algorithm,
        ),
    )
    .run(&a2_case(), |case| {
        if case.exclude_event_keys.is_empty() {
            add(&empty, 1);
            return Ok(());
        }
        let blocks: std::collections::BTreeSet<&str> = case
            .participants
            .iter()
            .filter_map(|p| p.calendar.as_deref())
            .flatten()
            .filter_map(|b| b.event_key.as_deref())
            .collect();
        let occurrences: std::collections::BTreeSet<&str> = case
            .occurrences
            .iter()
            .map(|occ| occ.event_key.as_str())
            .collect();
        let excluded = &case.exclude_event_keys;
        if excluded.iter().any(|key| blocks.contains(key.as_str())) {
            add(&hits_a_block_key, 1);
        }
        if excluded
            .iter()
            .any(|key| occurrences.contains(key.as_str()))
        {
            add(&hits_an_occurrence_key, 1);
        }
        if excluded
            .iter()
            .any(|key| !blocks.contains(key.as_str()) && !occurrences.contains(key.as_str()))
        {
            add(&has_unknown_key, 1);
        }
        Ok(())
    })
    .expect("the exclude_event_keys corpus probe runs");

    assert!(
        empty.get() >= 50,
        "only {} empty exclusion list(s)",
        empty.get()
    );
    assert!(
        hits_a_block_key.get() >= 30,
        "only {} case(s) excluded a block key the case carries",
        hits_a_block_key.get()
    );
    assert!(
        hits_an_occurrence_key.get() >= 30,
        "only {} case(s) excluded an occurrence key the case carries",
        hits_an_occurrence_key.get()
    );
    assert!(
        has_unknown_key.get() >= 30,
        "only {} case(s) carried an unknown excluded key",
        has_unknown_key.get()
    );
}
