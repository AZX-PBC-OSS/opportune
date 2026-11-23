//! Property tests: the engine's interval set must agree with the point-by-point
//! oracle over bounded inputs, and every result must stay canonical.

use opportune::intervals::{IntervalSet, Span};
use opportune_testkit::MinuteSet;
use opportune_testkit::generators::{self, MINUTE_DOMAIN_END, span, span_list, widen_amounts};
use proptest::prelude::*;

fn engine_set(spans: &[(i64, i64)]) -> IntervalSet {
    IntervalSet::from_spans(
        spans
            .iter()
            .map(|&(start, end)| Span::new(start, end).unwrap())
            .collect(),
    )
}

fn oracle_set(spans: &[(i64, i64)]) -> MinuteSet {
    let mut set = MinuteSet::new();
    for (start, end) in spans {
        set.insert_span(*start, *end);
    }
    set
}

/// The engine set and the oracle set must cover the same minutes, and the
/// engine's span form must be the oracle's canonical form: strictly sorted,
/// non-overlapping, non-adjacent.
fn assert_agree(engine: &IntervalSet, oracle: &MinuteSet, domain_end: i64) {
    for minute in 0..domain_end {
        assert_eq!(
            engine.contains_minute(minute),
            oracle.contains(minute),
            "minute {minute}: engine vs oracle membership"
        );
    }
    let oracle_spans = oracle.spans();
    assert_eq!(
        engine.spans().len(),
        oracle_spans.len(),
        "span forms must be identical"
    );
    for (engine_span, oracle_span) in engine.spans().iter().zip(oracle_spans) {
        assert_eq!(engine_span.start(), oracle_span.0);
        assert_eq!(engine_span.end(), oracle_span.1);
    }
    // The canonical invariant itself: sorted, non-adjacent, non-empty spans.
    for pair in engine.spans().windows(2) {
        assert!(
            pair[0].end() < pair[1].start(),
            "adjacent or unordered spans"
        );
    }
}

proptest! {
    #[test]
    fn union_agrees_with_the_oracle(a in span_list(), b in span_list()) {
        let engine = engine_set(&a).union(&engine_set(&b));
        let oracle = oracle_set(&a).union(&oracle_set(&b));
        assert_agree(&engine, &oracle, MINUTE_DOMAIN_END);
    }

    #[test]
    fn intersection_agrees_with_the_oracle(a in span_list(), b in span_list()) {
        let engine = engine_set(&a).intersection(&engine_set(&b));
        let oracle = oracle_set(&a).intersection(&oracle_set(&b));
        assert_agree(&engine, &oracle, MINUTE_DOMAIN_END);
    }

    #[test]
    fn difference_agrees_with_the_oracle(a in span_list(), b in span_list()) {
        let engine = engine_set(&a).difference(&engine_set(&b));
        let oracle = oracle_set(&a).difference(&oracle_set(&b));
        assert_agree(&engine, &oracle, MINUTE_DOMAIN_END);
    }

    #[test]
    fn widening_agrees_with_the_oracle(
        spans in span_list(),
        (before, after) in widen_amounts(),
    ) {
        let engine = engine_set(&spans).widened(before, after).unwrap();
        let oracle = oracle_set(&spans).widened(before, after);
        // Widening can push spans past the domain edge, so compare there too.
        assert_agree(&engine, &oracle, MINUTE_DOMAIN_END + generators::WIDEN_MAX);
    }

    #[test]
    fn span_queries_agree_with_the_oracle(spans in span_list(), (start, end) in span()) {
        let engine = engine_set(&spans);
        let oracle = oracle_set(&spans);
        let query = Span::new(start, end).unwrap();
        prop_assert_eq!(engine.overlaps(&query), oracle.overlaps_span(start, end));
        prop_assert_eq!(engine.contains_span(&query), oracle.contains_span(start, end));
        for minute in start..end {
            prop_assert_eq!(engine.contains_minute(minute), oracle.contains(minute));
        }
    }

    #[test]
    fn algebra_relations_hold_between_the_operations(
        a in span_list(),
        b in span_list(),
    ) {
        // (A ∖ B) ∪ (A ∩ B) ∪ (B ∖ A) = A ∪ B, partitioned into disjoint pieces.
        let engine_a = engine_set(&a);
        let engine_b = engine_set(&b);
        let left = engine_a
            .difference(&engine_b)
            .union(&engine_a.intersection(&engine_b))
            .union(&engine_b.difference(&engine_a));
        let oracle_union = oracle_set(&a).union(&oracle_set(&b));
        for minute in 0..MINUTE_DOMAIN_END {
            prop_assert_eq!(
                left.contains_minute(minute),
                oracle_union.contains(minute),
                "minute {}: partition disagrees with the union", minute
            );
        }
    }
}

/// Every set over a tiny minute domain, as both an engine set and an oracle set:
/// one entry per subset of the domain's minutes, built from its maximal runs.
fn every_set_in_domain(minutes: i64) -> Vec<(IntervalSet, MinuteSet)> {
    (0..1u32 << minutes)
        .map(|mask| {
            let mut spans: Vec<(i64, i64)> = Vec::new();
            let mut run_start = None;
            for minute in 0..minutes {
                match (mask & (1 << minute) != 0, run_start) {
                    (true, None) => run_start = Some(minute),
                    (false, Some(from)) => {
                        spans.push((from, minute));
                        run_start = None;
                    }
                    _ => {}
                }
            }
            if let Some(from) = run_start {
                spans.push((from, minutes));
            }
            (engine_set(&spans), oracle_set(&spans))
        })
        .collect()
}

/// Every query span within and just past a tiny domain.
fn every_span_in_domain(minutes: i64) -> Vec<(i64, i64)> {
    (0..=minutes)
        .flat_map(|start| ((start + 1)..=minutes).map(move |end| (start, end)))
        .collect()
}

/// Exhaustive rather than sampled: across every set in a six-minute domain and
/// every pair of them, each operation, each widening amount and each query span
/// is compared with the point-by-point model, including the composition the
/// evaluation pipeline uses (widened blocked time subtracted from allowed time).
#[test]
fn every_set_pair_in_a_tiny_domain_agrees_with_the_oracle() {
    const MINUTES: i64 = 6;
    // Membership and span forms are compared past the domain edge so widened
    // spans that leave it are checked rather than clipped out of the comparison.
    const COMPARED_END: i64 = MINUTES + 3;

    let sets = every_set_in_domain(MINUTES);
    let queries = every_span_in_domain(MINUTES);

    for (engine_a, oracle_a) in &sets {
        for (start, end) in &queries {
            let query = Span::new(*start, *end).unwrap();
            assert_eq!(
                engine_a.contains_span(&query),
                oracle_a.contains_span(*start, *end),
                "contains_span {start}..{end}"
            );
            assert_eq!(
                engine_a.overlaps(&query),
                oracle_a.overlaps_span(*start, *end),
                "overlaps {start}..{end}"
            );
        }

        for (before, after) in [(0, 0), (1, 0), (0, 1), (2, 2)] {
            assert_agree(
                &engine_a.widened(before, after).unwrap(),
                &oracle_a.widened(before, after),
                COMPARED_END,
            );
        }

        for (engine_b, oracle_b) in &sets {
            assert_agree(
                &engine_a.union(engine_b),
                &oracle_a.union(oracle_b),
                COMPARED_END,
            );
            assert_agree(
                &engine_a.intersection(engine_b),
                &oracle_a.intersection(oracle_b),
                COMPARED_END,
            );
            assert_agree(
                &engine_a.difference(engine_b),
                &oracle_a.difference(oracle_b),
                COMPARED_END,
            );
            // The pipeline's shape: blocked time widened by clearance, then
            // removed from allowed time.
            assert_agree(
                &engine_a
                    .widened(1, 1)
                    .unwrap()
                    .difference(&engine_b.widened(1, 1).unwrap().union(engine_b)),
                &oracle_a
                    .widened(1, 1)
                    .difference(&oracle_b.widened(1, 1).union(oracle_b)),
                COMPARED_END,
            );
        }
    }
}
