//! Adversarial probes for the benchmark workloads and the two-pass eligibility
//! split the search uses.
//!
//! A search decides who can take a start without building a reason and reports
//! the exclusion from a second pass over the same data; `check_slot` builds every
//! reason in one pass and shares no state with the search. These tests run both
//! answers over the exact fifty-participant inputs the latency targets are
//! measured on, so a benchmark cannot be measuring an engine whose own
//! confirmation contradicts what it offers and explains.

use opportune::Engine;
use opportune::model::{Proposal, ProposalEvent};
use opportune::time::Instant;
use opportune_testkit::workloads::spec_workloads;

fn engine() -> Engine {
    Engine::builder()
        .build()
        .expect("the bundled database builds")
}

/// A proposal putting `assignment` on `start`, in term order.
fn proposal(start: Instant, assignment: &[&[&str]]) -> Proposal {
    Proposal {
        events: vec![ProposalEvent {
            part: "main".to_owned(),
            start,
            assignment: assignment
                .iter()
                .map(|hosts| hosts.iter().map(|id| (*id).to_owned()).collect())
                .collect(),
        }],
    }
}

#[test]
fn every_start_the_benchmark_workloads_report_is_one_the_engine_confirms() {
    let engine = engine();
    for workload in spec_workloads(&engine) {
        let request = &workload.request;
        let response = engine
            .find_availability(request)
            .expect("a workload request is valid");
        assert!(!response.offers.is_empty(), "{} offers", workload.id);

        // Every offer confirms, carrying the same flags and the same reserved
        // travel — ten minutes either side of the booking for each assigned host
        // on these inputs, so a participant's reserve is not silently dropped.
        for offer in response.offers.iter().step_by(13) {
            let event = &offer.events[0];
            let check = engine
                .check_slot(
                    request,
                    &Proposal {
                        events: vec![ProposalEvent {
                            part: event.part.clone(),
                            start: event.start,
                            assignment: event.assignment.clone(),
                        }],
                    },
                )
                .expect("an offer's assignment is a shape the request accepts");
            assert!(
                check.valid,
                "{}: {} was offered but cannot be confirmed: {:?}",
                workload.id, event.start, check.reasons
            );
            assert_eq!(check.flags, offer.flags, "{}: {}", workload.id, event.start);
            assert_eq!(
                check.reserve, event.reserve,
                "{}: {}",
                workload.id, event.start
            );
        }

        // A start the single-term workload excludes depends on all fifty hosts,
        // so its first reason is the smallest ID's — which is also the only host a
        // one-slot proposal needs. The confirmation therefore judges the same
        // participant and must report the same first reason, in the same code and
        // with the same citation.
        if workload.id != "single_term" {
            continue;
        }
        assert!(!response.excluded.is_empty(), "single_term explains");
        let mut checked = 0_usize;
        for excluded in response.excluded.iter().step_by(3) {
            checked += 1;
            let check = engine
                .check_slot(request, &proposal(excluded.start, &[&["host-00"]]))
                .expect("a one-host proposal fits the single term");
            assert!(
                !check.valid,
                "{}: {} was excluded but confirms",
                workload.id, excluded.start
            );
            let explained = excluded
                .reasons
                .first()
                .expect("an exclusion carries a reason");
            let confirmed = check
                .reasons
                .first()
                .expect("an invalid slot carries a reason");
            assert_eq!(
                (
                    confirmed.participant.as_deref(),
                    confirmed.code,
                    confirmed.rule.as_deref()
                ),
                (
                    explained.participant.as_deref(),
                    explained.code,
                    explained.rule.as_deref()
                ),
                "single_term: the search and the confirmation judge {} differently",
                excluded.start
            );
        }
        assert!(
            checked > 20,
            "single_term: {checked} exclusion(s) confirmed, too few to mean anything"
        );
    }
}
