//! Deterministic weighted assignment of distinct hosts to requirement terms.
//!
//! A requirement asks for `n` distinct hosts per term out of the hosts that can
//! fill that term, and every host carries a weight. The chosen assignment has
//! the highest total weight and, among those, is lexicographically smallest
//! when each term's chosen host IDs are sorted ascending and compared term by
//! term. This module implements that rule directly: it never folds ID ranks
//! into costs, because minimizing a sum of ranks orders assignments differently
//! from the term-ordered comparison.
//!
//! With `S` the total number of slots, the algorithm runs in three steps:
//!
//! 1. **Prune.** In each term, only the top `S` candidates ranked by weight
//!    descending then ID ascending matter. An assignment uses at most `S` hosts,
//!    so a term holding a lower-ranked candidate always leaves one of its top `S`
//!    unused; swapping it in cannot lower the total weight, and when the weights
//!    tie the replacement's ID is smaller, so it cannot make the term's sorted ID
//!    list lexicographically larger either. The optimum therefore lies inside the
//!    pruned graph.
//! 2. **Optimize weight.** One successive-shortest-path pass on weight alone
//!    decides whether every slot can be filled and finds the best total weight
//!    `W*`.
//! 3. **Break ties.** Terms in order, and each term's candidates in ascending ID
//!    order, pinning a host exactly when the remaining slots can still be filled
//!    with total weight `W*`.
//!
//! Weights are accumulated and compared as `i64`, so the full `i32` weight range —
//! including `i32::MIN` combined with `i32::MAX` — sums without overflow.
//!
//! ```
//! use opportune::matching::{assign, Candidate, Term};
//!
//! // Two terms, one host each. Ana and Bo weigh the same, so the tie goes to the
//! // smaller ID in the first term, which then forces Bo into the second.
//! let first = [
//!     Candidate { id: "ana", weight: 3 },
//!     Candidate { id: "bo", weight: 3 },
//!     Candidate { id: "cy", weight: 1 },
//! ];
//! let second = [Candidate { id: "ana", weight: 3 }, Candidate { id: "bo", weight: 3 }];
//! let terms = [
//!     Term { count: 1, candidates: &first },
//!     Term { count: 1, candidates: &second },
//! ];
//! let assignment = assign(&terms).unwrap();
//! assert_eq!(assignment.terms, [vec!["ana".to_owned()], vec!["bo".to_owned()]]);
//! ```

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};

/// A participant that can fill a term, with the weight their assignment carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Candidate<'a> {
    /// The participant's ID.
    pub id: &'a str,
    /// The participant's weight. Assignments maximise the total.
    pub weight: i32,
}

/// One requirement term: how many distinct hosts it needs, and who can fill it.
///
/// A host's weight belongs to the host, so it is the same in every term that
/// lists it. A host listed more than once is one candidate; the greatest weight
/// listed for it applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Term<'a> {
    /// The number of distinct hosts the term needs.
    pub count: usize,
    /// The hosts that can fill it.
    pub candidates: &'a [Candidate<'a>],
}

/// The chosen hosts: one list per term, in term order, each sorted by ascending
/// host ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assignment {
    /// The chosen host IDs, one sorted list per term.
    pub terms: Vec<Vec<String>>,
}

/// Whether distinct hosts can fill every term jointly, ignoring weights
/// entirely.
///
/// This is the static check `validate` reports as a structurally impossible
/// request: each term may have enough candidates on its own while overlapping
/// pools cannot supply enough distinct hosts across the whole requirement.
///
/// ```
/// use opportune::matching::{jointly_fillable, Candidate, Term};
///
/// // Each term has one candidate, but both need the same host.
/// let only = [Candidate { id: "ana", weight: 0 }];
/// let terms = [Term { count: 1, candidates: &only }, Term { count: 1, candidates: &only }];
/// assert!(!jointly_fillable(&terms));
/// ```
#[must_use]
pub fn jointly_fillable(terms: &[Term<'_>]) -> bool {
    let instance = Instance::new(terms);
    let Some(demands) = demands(terms, &instance) else {
        return false;
    };
    let unused = vec![false; instance.hosts.len()];
    let mut scratch = FlowScratch::default();
    instance
        .solve_flow(&instance.eligible, &demands, &unused, &mut scratch)
        .is_some()
}

/// The highest-weight assignment of distinct hosts to `terms`, with ties broken
/// by the lexicographically smallest per-term sorted ID list, or `None` when the
/// terms cannot be filled jointly.
///
/// Terms needing no hosts are accepted and come back as empty lists.
///
/// ```
/// use opportune::matching::{assign, Candidate, Term};
///
/// let pool = [
///     Candidate { id: "ana", weight: 10 },
///     Candidate { id: "bo", weight: 4 },
/// ];
/// let terms = [Term { count: 1, candidates: &pool }];
/// let assignment = assign(&terms).unwrap();
/// assert_eq!(assignment.terms, [vec!["ana".to_owned()]]);
///
/// let impossible = [Term { count: 2, candidates: &pool[..1] }];
/// assert!(assign(&impossible).is_none());
/// ```
#[must_use]
pub fn assign(terms: &[Term<'_>]) -> Option<Assignment> {
    if let [term] = terms {
        // One term asking for one host needs no graph: the single heaviest
        // candidate wins, and the smallest ID among equals is the sorted
        // one-element list the tie-break compares. A search runs this shape once
        // per candidate start, so it is worth answering directly.
        if term.count == 1 {
            let mut best: Option<Candidate<'_>> = None;
            for candidate in term.candidates {
                best = match best {
                    Some(held)
                        if held.weight > candidate.weight
                            || (held.weight == candidate.weight && held.id <= candidate.id) =>
                    {
                        Some(held)
                    }
                    _ => Some(*candidate),
                };
            }
            return best.map(|chosen| Assignment {
                terms: vec![vec![chosen.id.to_owned()]],
            });
        }
    }
    let instance = Instance::new(terms);
    let demands = demands(terms, &instance)?;
    let slots = demands.iter().copied().fold(0_usize, usize::saturating_add);
    let eligible = instance.pruned(slots);
    let mut used = vec![false; instance.hosts.len()];
    let mut scratch = FlowScratch::default();
    let best = instance.solve_flow(&eligible, &demands, &used, &mut scratch)?;
    // The tie-break holds the witness here, and every probe writes into the
    // scratch instead: a rejected probe must not disturb the assignment the loop
    // is treating as optimal, and a swap on acceptance keeps both buffers'
    // allocations for the next probe.
    let mut witness = std::mem::take(&mut scratch.witness);

    let mut remaining = demands;
    let mut cursors = vec![0_usize; terms.len()];
    let mut chosen: Vec<Vec<String>> = vec![Vec::new(); terms.len()];
    let mut pinned = 0_i64;
    for (term, candidates) in eligible.iter().enumerate() {
        for _ in 0..terms[term].count {
            loop {
                let Some(&host) = candidates.get(cursors[term]) else {
                    // Unreachable while pruning and the weight bound hold: the
                    // pinned hosts are part of some optimal assignment, so one of
                    // this term's remaining candidates always keeps the rest at
                    // `W*`. A break in that argument is a solver bug, not an
                    // infeasible request, so it fails loudly where assertions are
                    // on; a release build reports infeasibility rather than
                    // inventing an assignment.
                    debug_assert!(
                        false,
                        "an optimal completion always has a candidate left to pin"
                    );
                    return None;
                };
                cursors[term] += 1;
                if used[host] {
                    continue;
                }
                // If the current optimal witness already assigns `host` to `term`,
                // pinning it preserves optimality with no flow re-solve.
                if let Some(pos) = witness[term].iter().position(|&assigned| assigned == host) {
                    // `remove` rather than `swap_remove`: the witness stays
                    // ascending by host, which a debug build asserts.
                    witness[term].remove(pos);
                    used[host] = true;
                    remaining[term] -= 1;
                    pinned += instance.weights[host];
                    chosen[term].push(instance.hosts[host].to_owned());
                    break;
                }
                used[host] = true;
                remaining[term] -= 1;
                let required = best - pinned - instance.weights[host];
                if let Some(total) = instance.solve_flow(&eligible, &remaining, &used, &mut scratch)
                    && total == required
                {
                    std::mem::swap(&mut witness, &mut scratch.witness);
                    pinned += instance.weights[host];
                    chosen[term].push(instance.hosts[host].to_owned());
                    break;
                }
                used[host] = false;
                remaining[term] += 1;
            }
        }
    }
    Some(Assignment { terms: chosen })
}

/// Each term's demand, or `None` when some term has fewer candidates than the
/// hosts it needs and the whole requirement is therefore infeasible.
fn demands(terms: &[Term<'_>], instance: &Instance<'_>) -> Option<Vec<usize>> {
    terms
        .iter()
        .zip(&instance.eligible)
        .map(|(term, eligible)| (eligible.len() >= term.count).then_some(term.count))
        .collect()
}

/// The requirement as dense host indices, so the graph search never looks a host
/// up by name and the host order is deterministic.
struct Instance<'a> {
    /// Every host's ID, by host number.
    hosts: Vec<&'a str>,
    /// Each host's weight, by host number.
    weights: Vec<i64>,
    /// For each term, the host numbers that can fill it, ascending.
    eligible: Vec<Vec<usize>>,
}

impl<'a> Instance<'a> {
    /// Builds the host table and each term's eligible hosts. Hosts are numbered
    /// in ascending ID order and duplicate listings collapse to the greatest
    /// weight for that ID.
    fn new(terms: &[Term<'a>]) -> Self {
        let mut weights: BTreeMap<&'a str, i64> = BTreeMap::new();
        for term in terms {
            for candidate in term.candidates {
                let weight = i64::from(candidate.weight);
                weights
                    .entry(candidate.id)
                    .and_modify(|held| *held = (*held).max(weight))
                    .or_insert(weight);
            }
        }
        let hosts: Vec<&'a str> = weights.keys().copied().collect();
        let weights: Vec<i64> = weights.values().copied().collect();
        let eligible = terms
            .iter()
            .map(|term| {
                let mut hosts_of: Vec<usize> = term
                    .candidates
                    .iter()
                    .filter_map(|candidate| hosts.binary_search(&candidate.id).ok())
                    .collect();
                hosts_of.sort_unstable();
                hosts_of.dedup();
                hosts_of
            })
            .collect();
        Self {
            hosts,
            weights,
            eligible,
        }
    }

    /// Each term's candidates ranked by weight descending and ID ascending, then
    /// cut to `slots`, the total number of slots in the whole requirement. At
    /// most that many hosts are ever used, so a candidate below the cut is part
    /// of no optimal assignment. The result is in ascending ID order for
    /// tie-breaking.
    fn pruned(&self, slots: usize) -> Vec<Vec<usize>> {
        self.eligible
            .iter()
            .map(|term| {
                let mut ranked = term.clone();
                ranked.sort_by(|left, right| {
                    self.weights[*right]
                        .cmp(&self.weights[*left])
                        .then(left.cmp(right))
                });
                ranked.truncate(slots);
                ranked.sort_unstable();
                ranked
            })
            .collect()
    }

    /// The greatest total weight of an assignment that fills `demands` from
    /// `eligible`, with `used` hosts excluded, or `None` when fewer slots can be
    /// filled than demanded.
    ///
    /// One witness assignment is left in [`FlowScratch::witness`] — per term, the
    /// host numbers the optimal flow assigned, ascending — so tie-breaking can
    /// confirm an already-assigned candidate without re-solving. Every buffer the
    /// solve needs comes from `scratch`, so a tie-break probe reuses the previous
    /// solve's allocations instead of building them again.
    fn solve_flow(
        &self,
        eligible: &[Vec<usize>],
        demands: &[usize],
        used: &[bool],
        scratch: &mut FlowScratch,
    ) -> Option<i64> {
        let mut desired = 0_i64;
        for &demand in demands {
            desired = desired.saturating_add(i64::try_from(demand).unwrap_or(i64::MAX));
        }
        let term_count = demands.len();
        if scratch.witness.len() != term_count {
            scratch.witness.resize_with(term_count, Vec::new);
        }
        for assigned in &mut scratch.witness {
            assigned.clear();
        }
        if desired <= 0 {
            return Some(0);
        }

        // Compact the host side to only the unused hosts referenced by an
        // unfilled term: after top-S pruning that is at most `S * terms` nodes
        // even when the request holds hundreds of participants.
        scratch.host_slot.clear();
        scratch.host_slot.resize(self.hosts.len(), usize::MAX);
        scratch.active_hosts.clear();
        for (term, hosts) in eligible.iter().enumerate() {
            if demands[term] == 0 {
                continue;
            }
            for &host in hosts {
                if !used[host] && scratch.host_slot[host] == usize::MAX {
                    scratch.host_slot[host] = scratch.active_hosts.len();
                    scratch.active_hosts.push(host);
                }
            }
        }
        if i64::try_from(scratch.active_hosts.len()).unwrap_or(i64::MAX) < desired {
            return None;
        }

        let active_count = scratch.active_hosts.len();
        let source = 0;
        let sink = term_count + active_count + 1;
        scratch.flow.reset(sink + 1);
        for (term, &demand) in demands.iter().enumerate() {
            if demand == 0 {
                continue;
            }
            let capacity = i64::try_from(demand).unwrap_or(i64::MAX);
            scratch.flow.add(source, 1 + term, capacity, 0);
        }
        // Closed-form exact shortest distances from `source` over this initial
        // DAG: every edge into `host` carries the same cost `-weight[host]`, so
        // Bellman-Ford is unnecessary.
        scratch.potential.clear();
        scratch.potential.resize(sink + 1, 0);
        let mut sink_dist = FAR;
        for (local, &host) in scratch.active_hosts.iter().enumerate() {
            let cost = -self.weights[host];
            let node = 1 + term_count + local;
            scratch.potential[node] = cost;
            sink_dist = sink_dist.min(cost);
            scratch.flow.add(node, sink, 1, 0);
        }
        if sink_dist < FAR {
            scratch.potential[sink] = sink_dist;
        }
        for (term, hosts) in eligible.iter().enumerate() {
            if demands[term] == 0 {
                continue;
            }
            for &host in hosts {
                let local = scratch.host_slot[host];
                if local == usize::MAX {
                    continue;
                }
                // A host costs the negation of its weight, so the cheapest flow
                // is the heaviest assignment.
                scratch
                    .flow
                    .add(1 + term, 1 + term_count + local, 1, -self.weights[host]);
            }
        }
        let cost = {
            let FlowScratch {
                flow, potential, ..
            } = scratch;
            flow.min_cost(source, sink, desired, potential)?
        };
        // Flow only ever moves on term->host edges, so a forward term->host edge
        // with no capacity left is one host assigned to that term.
        for (term, &demand) in demands.iter().enumerate() {
            if demand == 0 {
                continue;
            }
            let assigned = &mut scratch.witness[term];
            for &edge_index in &scratch.flow.adjacency[1 + term] {
                let edge = &scratch.flow.edges[edge_index];
                // Forward term->host edges have even indices, capacity 0 when
                // saturated, and point to `1 + term_count + local`.
                if edge_index % 2 == 0
                    && edge.capacity == 0
                    && edge.to > term_count
                    && edge.to < 1 + term_count + active_count
                {
                    assigned.push(scratch.active_hosts[edge.to - (1 + term_count)]);
                }
            }
            // Host numbers ascend with IDs, and the edges are added in the
            // pruned list's ascending order, so the witness is a sorted ID list.
            debug_assert!(
                assigned.is_sorted(),
                "a term's witness is ascending by host, which is ascending by ID"
            );
        }
        Some(-cost)
    }
}

/// Every buffer one flow solve reuses: the residual graph, the host compaction
/// and potentials of the probe, and the witness assignment it finds.
///
/// One `FlowScratch` lives for a whole [`assign`], so the tie-breaking loop's
/// probes reuse the first solve's allocations instead of rebuilding a graph per
/// candidate. The tie-break holds its own witness buffer and swaps it with
/// [`Self::witness`] on acceptance, so a rejected probe cannot disturb the
/// assignment the loop is still holding.
#[derive(Default)]
struct FlowScratch {
    /// The residual graph, emptied and refilled by each solve.
    flow: Flow,
    /// Each host's position on the compacted host side, `usize::MAX` when this
    /// solve does not use that host.
    host_slot: Vec<usize>,
    /// The hosts this solve keeps, by host number.
    active_hosts: Vec<usize>,
    /// Each node's potential: the offset that makes residual costs non-negative.
    potential: Vec<i64>,
    /// One optimal assignment, by term: the host numbers it assigns, ascending.
    witness: Vec<Vec<usize>>,
}

/// A node is unreachable at this distance; every real path is far shorter.
const FAR: i64 = i64::MAX / 4;

/// A residual graph for successive-shortest-path min-cost flow, with the Dijkstra
/// state a solve keeps.
///
/// [`Flow::reset`] rewinds the graph to an empty one with room for `nodes` and
/// keeps every allocation — adjacency lists are cleared rather than dropped,
/// which is what makes a tie-break probe cheap.
#[derive(Default)]
struct Flow {
    /// The residual edges, each forward edge immediately followed by its reverse.
    edges: Vec<FlowEdge>,
    /// Each node's outgoing edge indices. Only the first `nodes` lists are live;
    /// the rest are pooled for a later, larger graph.
    adjacency: Vec<Vec<usize>>,
    /// How many nodes this graph has.
    nodes: usize,
    /// Dijkstra's best known distance to each node, `FAR` when unreachable.
    distances: Vec<i64>,
    /// The edge each node was reached by, `None` for the source and unreachable
    /// nodes.
    previous: Vec<Option<usize>>,
    /// Dijkstra's frontier, ordered by distance then node.
    queue: BinaryHeap<Reverse<(i64, usize)>>,
}

/// One residual edge. Its reverse is the edge at the next index.
struct FlowEdge {
    to: usize,
    capacity: i64,
    cost: i64,
}

impl Flow {
    /// Empties the graph, keeping the allocations, and gives it `nodes` nodes.
    fn reset(&mut self, nodes: usize) {
        self.nodes = nodes;
        self.edges.clear();
        if self.adjacency.len() < nodes {
            self.adjacency.resize_with(nodes, Vec::new);
        }
        for outgoing in &mut self.adjacency[..nodes] {
            outgoing.clear();
        }
    }

    /// Adds a forward edge and its zero-capacity reverse.
    fn add(&mut self, from: usize, to: usize, capacity: i64, cost: i64) {
        let forward = self.edges.len();
        self.edges.push(FlowEdge { to, capacity, cost });
        self.edges.push(FlowEdge {
            to: from,
            capacity: 0,
            cost: -cost,
        });
        self.adjacency[from].push(forward);
        self.adjacency[to].push(forward + 1);
    }

    /// The minimum cost of sending `desired` units from `source` to `sink`, or
    /// `None` when fewer can be sent.
    fn min_cost(
        &mut self,
        source: usize,
        sink: usize,
        desired: i64,
        potential: &mut [i64],
    ) -> Option<i64> {
        if desired <= 0 {
            return Some(0);
        }
        let mut sent = 0_i64;
        let mut total = 0_i64;
        while sent < desired {
            if !self.shortest_paths(source, sink, potential) {
                return None;
            }
            for (node, distance) in self.distances.iter().enumerate() {
                if *distance < FAR {
                    potential[node] += distance;
                }
            }
            let mut pushed = desired - sent;
            let mut node = sink;
            while let Some(edge) = self.previous[node] {
                pushed = pushed.min(self.edges[edge].capacity);
                node = self.edges[edge ^ 1].to;
            }
            let mut node = sink;
            while let Some(edge) = self.previous[node] {
                total += self.edges[edge].cost * pushed;
                self.edges[edge].capacity -= pushed;
                self.edges[edge ^ 1].capacity += pushed;
                node = self.edges[edge ^ 1].to;
            }
            sent += pushed;
        }
        Some(total)
    }

    /// Dijkstra on reduced costs, resetting and writing the Dijkstra state in
    /// place. Returns whether `sink` is reachable.
    fn shortest_paths(&mut self, source: usize, sink: usize, potential: &[i64]) -> bool {
        let Self {
            edges,
            adjacency,
            nodes,
            distances,
            previous,
            queue,
        } = self;
        distances.clear();
        distances.resize(*nodes, FAR);
        previous.clear();
        previous.resize(*nodes, None);
        queue.clear();
        distances[source] = 0;
        queue.push(Reverse((0_i64, source)));
        while let Some(Reverse((distance, node))) = queue.pop() {
            if distance > distances[node] {
                continue;
            }
            for &edge_index in &adjacency[node] {
                let edge = &edges[edge_index];
                if edge.capacity == 0 {
                    continue;
                }
                let reduced = edge.cost + potential[node] - potential[edge.to];
                let candidate = distance + reduced;
                if candidate < distances[edge.to] {
                    distances[edge.to] = candidate;
                    previous[edge.to] = Some(edge_index);
                    queue.push(Reverse((candidate, edge.to)));
                }
            }
        }
        distances[sink] < FAR
    }
}

#[cfg(test)]
mod tests {
    use super::{Candidate, Term, assign, jointly_fillable};

    /// Builds terms from `(count, &[(id, weight)])`.
    fn solve(terms: &[(usize, &[(&str, i32)])]) -> Option<Vec<Vec<String>>> {
        let candidates: Vec<Vec<Candidate<'_>>> = terms
            .iter()
            .map(|(_, hosts)| {
                hosts
                    .iter()
                    .map(|&(id, weight)| Candidate { id, weight })
                    .collect()
            })
            .collect();
        let built: Vec<Term<'_>> = terms
            .iter()
            .zip(&candidates)
            .map(|((count, _), candidates)| Term {
                count: *count,
                candidates,
            })
            .collect();
        assign(&built).map(|assignment| assignment.terms)
    }

    fn fillable(terms: &[(usize, &[(&str, i32)])]) -> bool {
        let candidates: Vec<Vec<Candidate<'_>>> = terms
            .iter()
            .map(|(_, hosts)| {
                hosts
                    .iter()
                    .map(|&(id, weight)| Candidate { id, weight })
                    .collect()
            })
            .collect();
        let built: Vec<Term<'_>> = terms
            .iter()
            .zip(&candidates)
            .map(|((count, _), candidates)| Term {
                count: *count,
                candidates,
            })
            .collect();
        jointly_fillable(&built)
    }

    fn ids(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| (*id).to_owned()).collect()
    }

    #[test]
    fn the_heaviest_assignment_wins_before_ids() {
        // Ana and Bo tie on weight, so term one takes the smaller ID and term two
        // is forced to the other host rather than taking the heavier Cy.
        let first = [("ana", 3), ("bo", 3), ("cy", 1)];
        let second = [("ana", 3), ("bo", 3)];
        assert_eq!(
            solve(&[(1, &first), (1, &second)]),
            Some(vec![ids(&["ana"]), ids(&["bo"])])
        );
    }

    #[test]
    fn the_tie_break_is_term_ordered_lexicographic_not_a_rank_sum() {
        // Two equally heavy full assignments exist for these zero weights. A
        // heuristic that added ID ranks to costs would give term one {a, c} or
        // {b, c} and leave d to term two (rank sum 3), but term-ordered
        // tie-breaking compares term one's sorted list first, so {a, b} wins
        // even though that costs the later term the largest ID (rank sum 4).
        let first = [("a", 0), ("b", 0), ("c", 0)];
        let second = [("a", 0), ("b", 0), ("d", 0)];
        assert_eq!(
            solve(&[(2, &first), (1, &second)]),
            Some(vec![ids(&["a", "b"]), ids(&["d"])])
        );
    }

    #[test]
    fn each_term_may_have_enough_candidates_yet_the_pools_cannot_fill_jointly() {
        // One host, wanted by two terms: every term is individually satisfiable,
        // no assignment is.
        let only = [("ana", 1)];
        assert!(!fillable(&[(1, &only), (1, &only)]));
        assert_eq!(solve(&[(1, &only), (1, &only)]), None);

        // Hall's bottleneck: the two terms between them want three hosts and the
        // shared pool holds two.
        let shared = [("ana", 1), ("bo", 1)];
        assert!(!fillable(&[(2, &shared), (1, &shared)]));
        assert_eq!(solve(&[(2, &shared), (1, &shared)]), None);
    }

    #[test]
    fn zero_redundancy_pools_are_still_fillable() {
        let ana = [("ana", 1)];
        let bo = [("bo", 2)];
        assert!(fillable(&[(1, &ana), (1, &bo)]));
        assert_eq!(
            solve(&[(1, &ana), (1, &bo)]),
            Some(vec![ids(&["ana"]), ids(&["bo"])])
        );
    }

    #[test]
    fn a_term_with_fewer_candidates_than_it_needs_is_rejected_immediately() {
        let pool = [("ana", 1), ("bo", 2)];
        assert_eq!(solve(&[(3, &pool)]), None);
        assert!(!fillable(&[(3, &pool)]));
        // A count no pool could ever satisfy is rejected rather than overflowing
        // or allocating on the caller's `n`.
        assert_eq!(solve(&[(usize::MAX, &pool)]), None);
        assert!(!fillable(&[(usize::MAX, &pool)]));
    }

    #[test]
    fn candidates_beyond_the_top_slots_do_not_change_the_winner() {
        // Three candidates for the first term and two for the second, with two
        // slots in total. The pruned graph keeps the top two of each; the third
        // candidate could only ever be chosen by a lighter assignment.
        let first = [("h0", 5), ("h1", 4), ("h2", 3)];
        let second = [("h0", 5), ("h1", 4)];
        assert_eq!(
            solve(&[(1, &first), (1, &second)]),
            Some(vec![ids(&["h0"]), ids(&["h1"])])
        );
        // With one slot in total, the pruning drops the lower-weighted candidate
        // even though its ID sorts first: weight is decided before IDs.
        let early_low = [("aaa", 0), ("zzz", 7)];
        assert_eq!(solve(&[(1, &early_low)]), Some(vec![ids(&["zzz"])]));
    }

    #[test]
    fn a_witness_replaced_mid_loop_still_ends_at_the_lexicographic_answer() {
        // Six hosts tied on weight, so the three slots in total cut each term's
        // list to three. The optimal flow hands the first term `hb` and the second
        // `ha` and `hc`, so the first term's scan starts at `ha` — a host the
        // witness does not hold — and pinning it is still optimal. The loop has to
        // take that probe's witness before it settles on `ha`, then `hc` and `hd`.
        let pool = [
            ("ha", 0),
            ("hb", 0),
            ("hc", 0),
            ("hd", 0),
            ("he", 0),
            ("hf", 0),
        ];
        let narrower = [("ha", 0), ("hc", 0), ("hd", 0), ("he", 0), ("hf", 0)];
        assert_eq!(
            solve(&[(1, &pool), (2, &narrower)]),
            Some(vec![ids(&["ha"]), ids(&["hc", "hd"])])
        );
    }

    #[test]
    fn one_term_needing_one_host_takes_the_heaviest_then_the_smallest_id() {
        // The single-slot shape is answered without the graph, so its contract is
        // pinned directly: the heaviest host wins, and an equal weight goes to the
        // smaller ID whichever order the pool lists them in.
        let forward = [("ana", 4), ("bo", 4), ("cy", 1)];
        let reversed = [("cy", 1), ("bo", 4), ("ana", 4)];
        assert_eq!(solve(&[(1, &forward)]), Some(vec![ids(&["ana"])]));
        assert_eq!(solve(&[(1, &reversed)]), Some(vec![ids(&["ana"])]));

        let empty: [(&str, i32); 0] = [];
        assert_eq!(solve(&[(1, &empty)]), None);
    }

    #[test]
    fn extreme_weights_sum_exactly() {
        let pool = [("low", i32::MIN), ("high", i32::MAX)];
        assert_eq!(solve(&[(2, &pool)]), Some(vec![ids(&["high", "low"])]));
        // The heavier of two extreme hosts wins outright rather than overflowing.
        assert_eq!(solve(&[(1, &pool)]), Some(vec![ids(&["high"])]));
    }

    #[test]
    fn terms_needing_no_hosts_are_empty_lists() {
        assert_eq!(solve(&[]), Some(Vec::new()));
        let pool = [("ana", 1)];
        assert_eq!(
            solve(&[(0, &pool), (1, &pool)]),
            Some(vec![Vec::new(), ids(&["ana"])])
        );
        assert!(fillable(&[(0, &pool)]));
    }

    #[test]
    fn duplicate_listings_collapse_to_one_candidate() {
        let pool = [
            Candidate {
                id: "ana",
                weight: 1,
            },
            Candidate {
                id: "ana",
                weight: 5,
            },
        ];
        let terms = [Term {
            count: 1,
            candidates: &pool,
        }];
        assert_eq!(
            assign(&terms),
            Some(super::Assignment {
                terms: vec![ids(&["ana"])]
            })
        );
        assert!(jointly_fillable(&terms));

        // The greatest weight listed for an ID decides, not the first listing's
        // or the last: `ana` beats `bo` on 10 however the pool orders its two
        // `ana` rows. A single term needing one host is answered without the
        // graph, so the collapse has to hold on that path too.
        let heavy_first = [("ana", 10), ("ana", 5), ("bo", 7)];
        let heavy_last = [("ana", 5), ("bo", 7), ("ana", 10)];
        assert_eq!(solve(&[(1, &heavy_first)]), Some(vec![ids(&["ana"])]));
        assert_eq!(solve(&[(1, &heavy_last)]), Some(vec![ids(&["ana"])]));

        // One host listed twice is still one candidate, so it cannot fill two
        // distinct slots.
        let two_slots = [Term {
            count: 2,
            candidates: &pool,
        }];
        assert_eq!(assign(&two_slots), None);
        assert!(!jointly_fillable(&two_slots));
    }
}
