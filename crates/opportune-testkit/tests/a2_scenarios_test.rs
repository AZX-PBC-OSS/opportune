//! End-to-end multi-participant scenarios: U3 to U10, U15, U17, U19 and U20 run
//! through the real `Engine` in all nine zones of the specification's matrix.
//!
//! Each scenario names every start it must offer with the assignment that start
//! carries, and every start it must exclude with the reasons it must report, so
//! the whole search window is accounted for. Every offer is re-checked with
//! `check_slot`, and every exclusion is confirmed with a well-formed assignment
//! the search would never choose.

use std::sync::OnceLock;

use opportune::Engine;
use opportune::model::{Proposal, ProposalEvent, Response};
use opportune::reasons::{Excluded, ReasonCode};
use opportune::time::{Instant, TimeZoneDatabase};
use opportune_testkit::scenarios::zone_matrix;
use opportune_testkit::scenarios_a2::{MultiScenario, a2_scenarios};

/// What an exclusion entry must report: one `(participant, code)` pair per
/// reported participant, then a start-level pair with no participant.
type ReportedReasons<'a> = Vec<(Option<&'a str>, ReasonCode)>;

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

fn proposal(start: Instant, assignment: Vec<Vec<String>>) -> Proposal {
    Proposal {
        events: vec![ProposalEvent {
            part: "main".to_owned(),
            start,
            assignment,
        }],
    }
}

/// The `(participant, code)` pairs an exclusion entry reports, in report order.
fn reasons(entry: &Excluded) -> Vec<(Option<&str>, ReasonCode)> {
    entry
        .reasons
        .iter()
        .map(|reason| (reason.participant.as_deref(), reason.code))
        .collect()
}

fn assert_scenario(scenario: &MultiScenario) {
    let engine = engine();
    let context = format!("{} in {}", scenario.id, scenario.zone);
    let diagnostics = engine.validate(&scenario.request);
    assert!(
        diagnostics.is_valid(),
        "{context}: invalid request: {:?}",
        diagnostics.errors
    );
    let response: Response = engine.find_availability(&scenario.request).unwrap();

    let offered: Vec<(Instant, Vec<Vec<String>>)> = response
        .offers
        .iter()
        .map(|offer| {
            (
                offer.start().expect("an offer has an event"),
                offer.events[0].assignment.clone(),
            )
        })
        .collect();
    assert_eq!(
        offered, scenario.offered,
        "{context}: offered starts and their assignments"
    );

    let excluded: Vec<(Instant, ReportedReasons<'_>)> = response
        .excluded
        .iter()
        .map(|entry| (entry.start, reasons(entry)))
        .collect();
    assert_eq!(
        excluded, scenario.excluded,
        "{context}: excluded starts and their reasons"
    );

    // Every offer lists one eligible pool per term, sorted by ID, holding the
    // assignment, and confirms with `check_slot`.
    for offer in &response.offers {
        let event = &offer.events[0];
        let terms = scenario.request.goal.parts[0].requirement.terms.len();
        assert_eq!(event.eligible.len(), terms, "{context}: one pool per term");
        assert_eq!(
            event.assignment.len(),
            terms,
            "{context}: one list per term"
        );
        for (eligible, assigned) in event.eligible.iter().zip(&event.assignment) {
            let mut sorted = eligible.clone();
            sorted.sort();
            assert_eq!(*eligible, sorted, "{context}: eligible hosts are sorted");
            assert!(
                assigned.iter().all(|id| eligible.contains(id)),
                "{context}: every assigned host is eligible"
            );
        }
        let check = engine
            .check_slot(
                &scenario.request,
                &proposal(event.start, event.assignment.clone()),
            )
            .unwrap();
        assert!(
            check.valid,
            "{context}: offered {} fails check_slot",
            event.start
        );
        assert_eq!(check.reserve, event.reserve, "{context}: reserve");
        assert_eq!(check.flags, offer.flags, "{context}: flags");
    }

    // Every excluded start is confirmed invalid with a well-formed assignment,
    // and reports the participant-level reason the search did.
    for (start, expected) in &scenario.excluded {
        let check = engine
            .check_slot(&scenario.request, &proposal(*start, scenario.probe.clone()))
            .unwrap();
        assert!(
            !check.valid,
            "{context}: excluded {start} passes check_slot"
        );
        let participant_reason = expected
            .iter()
            .find(|(participant, _)| participant.is_some())
            .map(|(_, code)| *code);
        if let Some(code) = participant_reason {
            assert!(
                check.reasons.iter().any(|reason| reason.code == code),
                "{context}: check_slot at {start} does not report {code:?}"
            );
        }
    }
}

#[test]
fn every_multi_participant_use_case_passes_in_every_zone() {
    for zone in zone_matrix() {
        let scenarios = a2_scenarios(database(), zone);
        let use_cases: Vec<&str> = scenarios.iter().map(|scenario| scenario.use_case).collect();
        for use_case in [
            "U3", "U4", "U5", "U6", "U7", "U8", "U9", "U10", "U11", "U12", "U13", "U15", "U17",
            "U19", "U20",
        ] {
            assert!(
                use_cases.contains(&use_case),
                "{use_case} has a scenario in {zone}"
            );
        }
        for scenario in scenarios {
            assert_scenario(&scenario);
        }
    }
}

#[test]
fn every_scenario_proves_both_an_offer_and_an_exclusion() {
    // A scenario that offers nothing, or excludes nothing, would pass vacuously
    // on that side and stop proving the use case it names.
    for zone in zone_matrix() {
        for scenario in a2_scenarios(database(), zone) {
            assert!(!scenario.offered.is_empty(), "{} offers", scenario.id);
            if matches!(scenario.id, "u9" | "u10") {
                // These two carry no excluded start by design: every candidate
                // is bookable, and their negative case is the assignment they
                // must choose instead.
                continue;
            }
            assert!(
                !scenario.offered.is_empty() && !scenario.excluded.is_empty(),
                "{} in {zone} proves both answers",
                scenario.id
            );
        }
    }
}
