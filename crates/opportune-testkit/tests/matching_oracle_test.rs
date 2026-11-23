//! Property tests: the engine's weighted matcher must agree exactly with the
//! brute-force oracle on feasibility, the best total weight and the per-term host
//! lists, including ties and the whole `i32` weight range.
//!
//! The oracle (`opportune_testkit::matching`) enumerates every valid distinct-host
//! assignment directly and shares no code with `opportune::matching`, so a bug in
//! top-`S` pruning, the successive-shortest-path weight pass or the greedy pinning
//! shows up as a disagreement rather than a shared blind spot.

use opportune::matching::{self, Candidate, Term};
use opportune_testkit::generators::{
    MatchingCase, matching_case, matching_pruning_case, matching_weight,
};
use opportune_testkit::matching::{BruteTerm, brute_assign, brute_jointly_fillable};
use proptest::prelude::*;

/// The engine's answer for a generated case: its assignment, and whether the
/// terms are jointly fillable.
fn engine_answer(case: &MatchingCase) -> (Option<Vec<Vec<String>>>, bool) {
    let candidates: Vec<Vec<Candidate<'_>>> = case
        .iter()
        .map(|(_, hosts)| {
            hosts
                .iter()
                .map(|(id, weight)| Candidate {
                    id,
                    weight: *weight,
                })
                .collect()
        })
        .collect();
    let terms: Vec<Term<'_>> = case
        .iter()
        .zip(&candidates)
        .map(|((count, _), candidates)| Term {
            count: *count,
            candidates,
        })
        .collect();
    (
        matching::assign(&terms).map(|assignment| assignment.terms),
        matching::jointly_fillable(&terms),
    )
}

/// The brute-force oracle's answer for the same case.
fn brute_answer(case: &MatchingCase) -> (Option<Vec<Vec<String>>>, bool) {
    let terms: Vec<BruteTerm> = case
        .iter()
        .map(|(count, hosts)| BruteTerm {
            count: *count,
            candidates: hosts.clone(),
        })
        .collect();
    (brute_assign(&terms), brute_jointly_fillable(&terms))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]

    #[test]
    fn assignments_match_the_brute_force_oracle(case in matching_case()) {
        let (engine, _) = engine_answer(&case);
        let (brute, _) = brute_answer(&case);
        prop_assert_eq!(engine, brute, "case: {:?}", case);
    }

    #[test]
    fn joint_fillability_matches_the_brute_force_oracle(case in matching_case()) {
        let (engine_assignment, engine_fillable) = engine_answer(&case);
        let (brute_assignment, brute_fillable) = brute_answer(&case);
        prop_assert_eq!(engine_fillable, brute_fillable, "case: {:?}", case);
        prop_assert_eq!(engine_fillable, engine_assignment.is_some(), "case: {:?}", case);
        prop_assert_eq!(brute_fillable, brute_assignment.is_some(), "case: {:?}", case);
    }

    #[test]
    fn pruning_preserves_the_exact_winner(case in matching_pruning_case()) {
        let (engine, _) = engine_answer(&case);
        let (brute, _) = brute_answer(&case);
        prop_assert_eq!(engine, brute, "case: {:?}", case);
    }

    #[test]
    fn reweighting_changes_assignments_never_feasibility(
        case in matching_case(),
        replacement in proptest::collection::vec(matching_weight(), 1..=8),
    ) {
        // The same terms and candidate sets, with every host's weight replaced:
        // feasibility is a property of the edges alone, so only the chosen
        // assignment moves.
        let mut ids: Vec<&str> = case
            .iter()
            .flat_map(|(_, hosts)| hosts.iter().map(|(id, _)| id.as_str()))
            .collect();
        ids.sort_unstable();
        ids.dedup();
        let reweighted: MatchingCase = case
            .iter()
            .map(|(count, hosts)| {
                let hosts = hosts
                    .iter()
                    .map(|(id, _)| {
                        let number = ids
                            .binary_search(&id.as_str())
                            .expect("every ID came from the case");
                        (id.clone(), replacement[number % replacement.len()])
                    })
                    .collect();
                (*count, hosts)
            })
            .collect();
        let (engine_before, fillable_before) = engine_answer(&case);
        let (engine_after, fillable_after) = engine_answer(&reweighted);
        prop_assert_eq!(fillable_before, fillable_after, "case: {:?}", case);
        prop_assert_eq!(fillable_before, engine_before.is_some(), "case: {:?}", case);
        prop_assert_eq!(
            fillable_after,
            engine_after.is_some(),
            "case: {:?}",
            reweighted
        );
        prop_assert_eq!(engine_before, brute_answer(&case).0, "case: {:?}", case);
        prop_assert_eq!(
            engine_after,
            brute_answer(&reweighted).0,
            "case: {:?}",
            reweighted
        );
    }
}

/// Every matching problem over three hosts, two terms needing up to two hosts
/// each, and per-host weights in `{-1, 0, 1}`, compared exhaustively rather than
/// sampled: ties, partial overlap, infeasibility and empty terms all appear.
#[test]
fn every_small_problem_agrees_with_the_brute_force_oracle() {
    let weights = [-1_i32, 0, 1];
    let term_options: Vec<(usize, u8)> = (0..=2_usize)
        .flat_map(|count| (1..8_u8).map(move |mask| (count, mask)))
        .collect();
    let mut checked = 0_usize;
    for &first_weight in &weights {
        for &second_weight in &weights {
            for &third_weight in &weights {
                let host_weights = [first_weight, second_weight, third_weight];
                for &first in &term_options {
                    for &second in &term_options {
                        let case = small_case(host_weights, first, second);
                        let (engine, fillable) = engine_answer(&case);
                        let (brute, brute_fillable) = brute_answer(&case);
                        assert_eq!(engine, brute, "case: {case:?}");
                        assert_eq!(fillable, brute_fillable, "case: {case:?}");
                        checked += 1;
                    }
                }
            }
        }
    }
    assert_eq!(checked, weights.len().pow(3) * term_options.len().pow(2));
}

/// A two-term problem over hosts `h0`, `h1` and `h2`: each term's mask lists the
/// hosts it can use, all carrying the host's own weight.
fn small_case(weights: [i32; 3], first: (usize, u8), second: (usize, u8)) -> MatchingCase {
    let listed = |mask: u8| {
        (0..3_usize)
            .filter(|index| mask & (1 << index) != 0)
            .map(|index| (format!("h{index}"), weights[index]))
            .collect()
    };
    vec![(first.0, listed(first.1)), (second.0, listed(second.1))]
}

/// The joint-fillability edge cases from the specification, judged by both sides:
/// every term individually satisfiable while the pools cannot be filled jointly,
/// Hall's bottleneck, exact zero-redundancy fillability, and a term with fewer
/// candidates than it needs.
#[test]
fn fillability_edge_cases_agree_with_brute_force() {
    let host = |id: &str, weight: i32| (id.to_owned(), weight);
    let cases: Vec<(&str, MatchingCase, bool)> = vec![
        (
            "both terms want the same single host",
            vec![(1, vec![host("ana", 0)]), (1, vec![host("ana", 0)])],
            false,
        ),
        (
            "hall bottleneck: three slots over a shared pool of two",
            vec![
                (2, vec![host("ana", 0), host("bo", 0)]),
                (1, vec![host("ana", 0), host("bo", 0)]),
            ],
            false,
        ),
        (
            "zero redundancy: each term has exactly one host of its own",
            vec![(1, vec![host("ana", 0)]), (1, vec![host("bo", 0)])],
            true,
        ),
        (
            "a term with fewer candidates than it needs",
            vec![(2, vec![host("ana", 0)]), (1, vec![host("bo", 0)])],
            false,
        ),
    ];
    for (label, case, expected) in cases {
        let (assignment, fillable) = engine_answer(&case);
        let (brute_assignment, brute_fillable) = brute_answer(&case);
        assert_eq!(fillable, expected, "{label}");
        assert_eq!(brute_fillable, expected, "{label}");
        assert_eq!(assignment.is_some(), expected, "{label}");
        assert_eq!(assignment, brute_assignment, "{label}");
    }
}

/// A single term needing three or more hosts. No other property here reaches that
/// shape: the exhaustive sweep tops out at two hosts per term and
/// `matching_pruning_case` always asks for one, so the pinning loop's behaviour
/// above two slots would otherwise be unverified. Both `i32` bounds and a pruning
/// cut that must keep exactly the optimal set are included.
#[test]
fn a_term_needing_three_or_more_hosts_agrees_with_the_brute_force_oracle() {
    const WEIGHTS: [[i32; 6]; 4] = [
        [0, 0, 0, 0, 0, 0],
        [1, 2, 2, 1, 3, 0],
        [-1, -1, 0, 0, 0, 1],
        [i32::MIN, 0, i32::MAX, 0, 1, -1],
    ];
    let mut checked = 0_usize;
    for weights in WEIGHTS {
        for count in 3..=4_usize {
            for mask in 1..64_u8 {
                let case: MatchingCase = vec![(
                    count,
                    (0..6_usize)
                        .filter(|index| mask & (1 << index) != 0)
                        .map(|index| (format!("h{index}"), weights[index]))
                        .collect(),
                )];
                let (engine, fillable) = engine_answer(&case);
                let (brute, brute_fillable) = brute_answer(&case);
                assert_eq!(engine, brute, "case: {case:?}");
                assert_eq!(fillable, brute_fillable, "case: {case:?}");
                checked += 1;
            }
        }
    }
    assert_eq!(checked, WEIGHTS.len() * 2 * 63);
}

/// Multi-slot terms while a top-`S` pruning cut is active: two terms over four
/// hosts needing up to three each, every candidate mask and weight vectors that
/// tie, mix and go negative. The pruning generator's terms only ever need one
/// host, so choosing several hosts out of a pruned list is untested there.
#[test]
fn multi_slot_terms_agree_with_the_oracle_where_pruning_is_active() {
    const WEIGHTS: [[i32; 4]; 3] = [[0, 0, 0, 0], [1, 0, -1, 2], [-2, 3, 3, 0]];
    let mut checked = 0_usize;
    for weights in WEIGHTS {
        for first_count in 0..=3_usize {
            for second_count in 0..=2_usize {
                for first_mask in 0..16_u8 {
                    for second_mask in 0..16_u8 {
                        let listed = |mask: u8| {
                            (0..4_usize)
                                .filter(|index| mask & (1 << index) != 0)
                                .map(|index| (format!("h{index}"), weights[index]))
                                .collect()
                        };
                        let case: MatchingCase = vec![
                            (first_count, listed(first_mask)),
                            (second_count, listed(second_mask)),
                        ];
                        let (engine, fillable) = engine_answer(&case);
                        let (brute, brute_fillable) = brute_answer(&case);
                        assert_eq!(engine, brute, "case: {case:?}");
                        assert_eq!(fillable, brute_fillable, "case: {case:?}");
                        checked += 1;
                    }
                }
            }
        }
    }
    assert_eq!(checked, WEIGHTS.len() * 4 * 3 * 256);
}
