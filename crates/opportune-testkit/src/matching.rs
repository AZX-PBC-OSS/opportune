//! Brute-force reference matching: the specification's assignment rule applied
//! by enumerating every valid distinct-host assignment directly.
//!
//! This shares no code with the engine's [`opportune::matching`] module. It keeps
//! candidates as owned strings, tracks used hosts with a flag per host, and
//! compares whole assignments with `Vec`'s lexicographic `Ord`, so a bug in the
//! engine's top-`S` pruning, successive-shortest-path weight optimisation or
//! greedy pinning shows up as a disagreement rather than a shared blind spot.
//!
//! ```
//! use opportune_testkit::matching::{brute_assign, BruteTerm};
//!
//! let terms = [
//!     BruteTerm { count: 1, candidates: vec![("ana".to_owned(), 3), ("bo".to_owned(), 3)] },
//!     BruteTerm { count: 1, candidates: vec![("bo".to_owned(), 3)] },
//! ];
//! assert_eq!(
//!     brute_assign(&terms).unwrap(),
//!     [vec!["ana".to_owned()], vec!["bo".to_owned()]],
//! );
//! ```

use std::collections::BTreeMap;

/// One term of a brute-force matching problem.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BruteTerm {
    /// The number of distinct hosts the term needs.
    pub count: usize,
    /// The hosts that can fill it, with their weights.
    pub candidates: Vec<(String, i32)>,
}

/// The assignment with the greatest total weight and, among those, the
/// lexicographically smallest per-term sorted ID list, or `None` when the terms
/// cannot be filled jointly.
#[must_use]
pub fn brute_assign(terms: &[BruteTerm]) -> Option<Vec<Vec<String>>> {
    let problem = Problem::new(terms);
    let mut enumeration = Enumeration {
        used: vec![false; problem.hosts.len()],
        chosen: Vec::new(),
        best: None,
        problem: &problem,
    };
    enumeration.term(0, 0);
    enumeration.best.map(|(_, chosen)| chosen)
}

/// Whether distinct hosts can fill every term jointly, judged by the same
/// enumeration: feasibility does not depend on weights.
#[must_use]
pub fn brute_jointly_fillable(terms: &[BruteTerm]) -> bool {
    brute_assign(terms).is_some()
}

/// The problem with candidates reduced to host numbers, one number per distinct
/// ID, and each term's candidates ascending by ID so combinations enumerate as
/// sorted ID lists.
struct Problem {
    terms: Vec<Vec<(usize, i32)>>,
    counts: Vec<usize>,
    hosts: Vec<String>,
}

impl Problem {
    fn new(terms: &[BruteTerm]) -> Self {
        let mut numbers: BTreeMap<&str, usize> = BTreeMap::new();
        let mut weights: BTreeMap<usize, i32> = BTreeMap::new();
        for term in terms {
            for (id, weight) in &term.candidates {
                let next = numbers.len();
                let number = *numbers.entry(id.as_str()).or_insert(next);
                weights
                    .entry(number)
                    .and_modify(|held| *held = (*held).max(*weight))
                    .or_insert(*weight);
            }
        }
        let mut hosts: Vec<String> = vec![String::new(); numbers.len()];
        for (id, number) in &numbers {
            (*id).clone_into(&mut hosts[*number]);
        }
        // A host's weight belongs to the host, so it is the same in every term
        // that lists it; duplicates collapse to the greatest weight listed.
        let counts: Vec<usize> = terms.iter().map(|term| term.count).collect();
        let terms: Vec<Vec<(usize, i32)>> = terms
            .iter()
            .map(|term| {
                let mut listed: Vec<usize> = term
                    .candidates
                    .iter()
                    .map(|(id, _)| numbers[id.as_str()])
                    .collect();
                listed.sort_unstable();
                listed.dedup();
                listed.sort_by(|left, right| hosts[*left].cmp(&hosts[*right]));
                listed
                    .into_iter()
                    .map(|number| (number, weights[&number]))
                    .collect()
            })
            .collect();
        Self {
            terms,
            counts,
            hosts,
        }
    }
}

/// One walk over every valid assignment.
struct Enumeration<'a> {
    problem: &'a Problem,
    used: Vec<bool>,
    chosen: Vec<Vec<String>>,
    best: Option<(i64, Vec<Vec<String>>)>,
}

impl Enumeration<'_> {
    /// Completes the assignment for `term` onwards and records it when it beats
    /// the best so far.
    fn term(&mut self, term: usize, total: i64) {
        if term == self.problem.terms.len() {
            let complete = self.chosen.clone();
            let better = match &self.best {
                None => true,
                Some((best_total, best_chosen)) => {
                    total > *best_total || (total == *best_total && complete < *best_chosen)
                }
            };
            if better {
                self.best = Some((total, complete));
            }
            return;
        }
        let count = self.problem.counts[term];
        self.chosen.push(Vec::new());
        self.choose(term, 0, count, total);
        self.chosen.pop();
    }

    /// Enumerates the term's combinations from candidate `start` on, taking
    /// `remaining` more hosts.
    fn choose(&mut self, term: usize, start: usize, remaining: usize, total: i64) {
        if remaining == 0 {
            self.term(term + 1, total);
            return;
        }
        for position in start..self.problem.terms[term].len() {
            let (host, weight) = self.problem.terms[term][position];
            if self.used[host] {
                continue;
            }
            self.used[host] = true;
            self.chosen[term].push(self.problem.hosts[host].clone());
            self.choose(term, position + 1, remaining - 1, total + i64::from(weight));
            self.chosen[term].pop();
            self.used[host] = false;
        }
    }
}
