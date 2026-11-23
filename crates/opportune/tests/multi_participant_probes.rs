//! Adversarial probes for multi-participant evaluation: reserve reporting for
//! attendees, the `available_only` midnight boundary, explanation truncation
//! with a start-level reason, and exact multi-slot assignment through the real
//! engine.
//!
//! Every test builds a request from the public model types and asserts what a
//! caller acts on.

#![cfg(feature = "bundled-tzdb")]

use std::collections::BTreeMap;

use opportune::Engine;
use opportune::model::{
    Block, Explain, Goal, Offer, Part, Participant, Proposal, ProposalEvent, Request, Requirement,
    Rule, RuleKind, Span, Term, Travel,
};
use opportune::presets;
use opportune::reasons::ReasonCode;
use opportune::tags::{Role, Selector};
use opportune::time::{
    Date, Duration, Instant, LocalResolution, RuleZone, TimeOfDay, WallEnd, WallRange, Weekday,
    Weekdays, WeeklyKey, WindowSpec, Zone, ZoneName,
};

/// A minute-aligned instant on the fixture's UTC day.
fn utc(hour: u8, minute: u8) -> Instant {
    let date = Date::new(2027, 6, 14).expect("the fixture date is real");
    let time = TimeOfDay::new(hour, minute).expect("the fixture time is real");
    let LocalResolution::Unique(instant) = Zone::utc()
        .resolve_local(date, time)
        .expect("the fixture instant is supported")
    else {
        panic!("UTC has no transitions");
    };
    instant
}

fn name(zone: &str) -> ZoneName {
    ZoneName::new(zone).expect("the fixture zone is a zone name")
}

fn engine() -> Engine {
    Engine::builder()
        .build()
        .expect("the bundled database builds")
}

/// A host whose calendar is known to be empty.
fn host(id: &str) -> Participant {
    let mut participant = Participant::new(id, name("UTC"));
    participant.calendar = Some(Vec::new());
    participant
}

/// An attendee with no calendar at all, which is normal outside the
/// organisation.
fn attendee(id: &str) -> Participant {
    let mut participant = Participant::new(id, name("UTC"));
    participant.role = Role::Attendee;
    participant
}

fn blocked(from: Instant, until: Instant) -> Block {
    Block::new(Span::new(from, until).expect("the fixture block is non-empty"))
}

/// A `window` rule open `from`–`until` every day, applying to `zz`.
fn attendee_hours(from: u8, until: u8) -> Rule {
    let spec = WindowSpec::new(
        RuleZone::Participant,
        vec![
            WeeklyKey::new(
                Weekdays::range(Weekday::Monday, Weekday::Sunday),
                vec![
                    WallRange::new(
                        TimeOfDay::new(from, 0).expect("a real time"),
                        WallEnd::Time(TimeOfDay::new(until, 0).expect("a real time")),
                    )
                    .expect("the fixture range is not empty"),
                ],
            )
            .expect("one key covers every weekday"),
        ],
        Vec::new(),
    )
    .expect("the fixture window is unambiguous");
    Rule::new("hours", RuleKind::Window(spec)).with_who(Selector::Ids(vec!["zz".to_owned()]))
}

fn request_over(window: Span, part: Part, participants: Vec<Participant>) -> Request {
    let mut request = Request::new(utc(0, 0), Goal::new(name("UTC"), window, vec![part]));
    request.participants = participants;
    request.explain = Some(Explain::new(10_000));
    request
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

/// The reserve of an offer as `(participant, start, end)`.
fn reserve(offer: &Offer) -> Vec<(String, Instant, Instant)> {
    offer.events[0]
        .reserve
        .iter()
        .map(|span| (span.participant.clone(), span.span.start(), span.span.end()))
        .collect()
}

/// A part needing one named host, with the booking's own travel.
fn travelling_part(host_id: &str, before: u32, after: u32) -> Part {
    let mut part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person(host_id)]),
    );
    part.travel = Travel {
        before: Duration::from_minutes(before),
        after: Duration::from_minutes(after),
        by_participant: BTreeMap::default(),
    };
    part
}

// ---------------------------------------------------------------------------
// A booking's travel is occupied time for every participant, attendees
// included, so the offer must return an attendee's travel spans to reserve.
// ---------------------------------------------------------------------------

#[test]
fn reserve_holds_an_attendees_travel_like_an_assigned_hosts() {
    let engine = engine();
    let window = Span::new(utc(9, 0), utc(12, 0)).expect("the fixture window is non-empty");

    // The engine treats the attendee's travel as occupied: bounded to 09:00
    // onwards, a 09:00 start is excluded because the 20 minutes before it fall
    // outside the attendee's stated availability.
    let mut judged = request_over(
        window,
        travelling_part("aa", 20, 10),
        vec![host("aa"), attendee("zz")],
    );
    judged.rules.push(attendee_hours(9, 12));
    let response = engine
        .find_availability(&judged)
        .expect("the request is valid");
    let excluded = response
        .excluded
        .iter()
        .find(|entry| entry.start == utc(9, 0))
        .expect("09:00 is excluded by the attendee's travel");
    let reasons: Vec<(Option<&str>, opportune::reasons::ReasonCode)> = excluded
        .reasons
        .iter()
        .map(|reason| (reason.participant.as_deref(), reason.code))
        .collect();
    assert_eq!(
        reasons,
        [
            (Some("zz"), opportune::reasons::ReasonCode::Travel),
            (None, opportune::reasons::ReasonCode::AttendeeUnavailable),
        ],
        "the attendee's travel is the reason the offer is withheld"
    );

    // So the same booking must return the attendee's travel to reserve, beside
    // the assigned host's: an offer carries every participant's travel spans to
    // reserve.
    let request = request_over(
        window,
        travelling_part("aa", 20, 10),
        vec![host("aa"), attendee("zz")],
    );
    let response = engine
        .find_availability(&request)
        .expect("the request is valid");
    let offer = &response.offers[0];
    assert_eq!(offer.start(), Some(utc(9, 0)));
    assert_eq!(
        reserve(offer),
        [
            ("aa".to_owned(), utc(8, 40), utc(9, 0)),
            ("aa".to_owned(), utc(9, 30), utc(9, 40)),
            ("zz".to_owned(), utc(8, 40), utc(9, 0)),
            ("zz".to_owned(), utc(9, 30), utc(9, 40)),
        ],
        "the attendee's travel is occupied time the caller must hold"
    );

    // A confirmation reports the same reserve as the offer it confirms.
    let check = engine
        .check_slot(
            &request,
            &proposal(utc(9, 0), offer.events[0].assignment.clone()),
        )
        .expect("the offer's assignment is well formed");
    assert_eq!(check.reserve, offer.events[0].reserve);
}

// ---------------------------------------------------------------------------
// `available_only`: a stated span covering whole UTC days.
// ---------------------------------------------------------------------------

#[test]
fn available_only_writes_whole_utc_days_as_00_00_to_24_00() {
    // 22:00 on one day to 02:00 two days later: a partial day, a whole day and a
    // partial day, so the interior key is the full-day form.
    let from = utc(22, 0);
    let until = Instant::from_minutes(from.minutes() + 28 * 60).expect("inside the range");
    let spans = [Span::new(from, until).expect("the fixture span is non-empty")];
    let (candidate, rule) =
        presets::available_only("cand", &spans).expect("the stated spans fit the ceiling");
    let window = rule.window().expect("the preset writes a window");
    let rendered: Vec<String> = window
        .dates()
        .iter()
        .flat_map(|key| {
            key.hours().iter().map(move |hours| {
                format!("{} {}-{}", key.dates().start(), hours.start(), hours.end())
            })
        })
        .collect();
    assert_eq!(
        rendered,
        [
            "2027-06-14 22:00-24:00",
            "2027-06-15 00:00-24:00",
            "2027-06-16 00:00-02:00",
        ],
        "a whole UTC day is 00:00-24:00, never an empty range"
    );

    // The whole middle day is bookable and the days outside are not.
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("xx")]),
    );
    let mut request = request_over(
        Span::new(
            Instant::from_minutes(from.minutes() - 24 * 60).expect("inside the range"),
            Instant::from_minutes(until.minutes() + 24 * 60).expect("inside the range"),
        )
        .expect("the fixture window is non-empty"),
        part,
        vec![host("xx"), candidate],
    );
    request.rules.push(rule);
    let response = engine()
        .find_availability(&request)
        .expect("the request is valid");
    let offered: Vec<Instant> = response
        .offers
        .iter()
        .map(|offer| offer.start().expect("an offer has an event"))
        .collect();
    // 22:00-24:00 is two hours (four starts), the whole middle day is 48, and
    // 00:00-02:00 is another four.
    assert_eq!(
        offered.len(),
        4 + 48 + 4,
        "22:00-24:00, a whole day, then 00:00-02:00"
    );
    assert_eq!(offered.first(), Some(&utc(22, 0)));
    assert_eq!(
        offered.last(),
        Some(&Instant::from_minutes(until.minutes() - 30).expect("inside the range"))
    );
}

// ---------------------------------------------------------------------------
// Explanation truncation counts a start-level reason like any other.
// ---------------------------------------------------------------------------

#[test]
fn explain_limit_counts_a_start_level_reason() {
    let mut cand = attendee("zz");
    cand.calendar = Some(vec![blocked(utc(0, 0), utc(23, 0))]);
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("xx")]),
    );
    let mut request = request_over(
        Span::new(utc(9, 0), utc(12, 0)).expect("the fixture window is non-empty"),
        part,
        vec![host("xx"), cand],
    );
    // Six half-hour starts are all excluded, each carrying two reasons: the
    // attendee's own and the start-level code. A limit of zero reports nothing
    // and still says so.
    for (limit, entries, truncated) in [
        (0, 0, true),
        (2, 1, true),
        (3, 1, true),
        (4, 2, true),
        (11, 5, true),
        (12, 6, false),
    ] {
        request.explain = Some(Explain::new(limit));
        let response = engine()
            .find_availability(&request)
            .expect("the request is valid");
        assert_eq!(response.excluded.len(), entries, "limit {limit}");
        assert_eq!(response.excluded_truncated, truncated, "limit {limit}");
        assert!(
            response
                .excluded
                .iter()
                .all(|entry| entry.reasons.len() == 2),
            "every reported entry is whole"
        );
    }
}

// ---------------------------------------------------------------------------
// An exclusion names every attendee's own reason once, then a single
// start-level reason — not one per attendee.
// ---------------------------------------------------------------------------

#[test]
fn two_blocked_attendees_share_one_start_level_reason() {
    let mut aa = attendee("aa");
    aa.calendar = Some(vec![blocked(utc(0, 0), utc(23, 0))]);
    let mut zz = attendee("zz");
    zz.calendar = Some(vec![blocked(utc(0, 0), utc(23, 0))]);
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("xx")]),
    );
    let request = request_over(
        Span::new(utc(9, 0), utc(10, 0)).expect("the fixture window is non-empty"),
        part,
        vec![host("xx"), zz, aa],
    );
    let response = engine()
        .find_availability(&request)
        .expect("the request is valid");
    let entry = &response.excluded[0];
    let reasons: Vec<(Option<&str>, ReasonCode)> = entry
        .reasons
        .iter()
        .map(|reason| (reason.participant.as_deref(), reason.code))
        .collect();
    assert_eq!(
        reasons,
        [
            (Some("aa"), ReasonCode::Blocked),
            (Some("zz"), ReasonCode::Blocked),
            (None, ReasonCode::AttendeeUnavailable),
        ],
        "each attendee explains itself, and the start carries one start-level reason"
    );
}

// ---------------------------------------------------------------------------
// Exact weighted assignment through the engine, beyond the two slots the
// generated oracle reaches.
// ---------------------------------------------------------------------------

const POOL: [&str; 4] = ["ha", "hb", "hc", "hd"];

/// Every `count`-sized combination of `pool`, in ascending order.
fn combinations(pool: &[usize], count: usize) -> Vec<Vec<usize>> {
    if count == 0 {
        return vec![Vec::new()];
    }
    let mut out = Vec::new();
    for (index, &first) in pool.iter().enumerate() {
        for rest in combinations(&pool[index + 1..], count - 1) {
            let mut one = vec![first];
            one.extend(rest);
            out.push(one);
        }
    }
    out
}

/// One term's demand: how many hosts, and which of `POOL` can fill it.
type Demand = (u32, Vec<usize>);

/// Every way to fill the terms from `eligible` with distinct hosts.
fn fillings(
    terms: &[Demand],
    eligible: &[bool],
    term: usize,
    used: &mut Vec<bool>,
    chosen: &mut Vec<Vec<usize>>,
    out: &mut Vec<Vec<Vec<usize>>>,
) {
    if term == terms.len() {
        out.push(chosen.clone());
        return;
    }
    let (count, pool) = &terms[term];
    let pool: Vec<usize> = pool
        .iter()
        .copied()
        .filter(|host| eligible[*host])
        .collect();
    for combination in combinations(&pool, usize::try_from(*count).expect("a small count")) {
        if combination.iter().any(|host| used[*host]) {
            continue;
        }
        for host in &combination {
            used[*host] = true;
        }
        chosen.push(combination.clone());
        fillings(terms, eligible, term + 1, used, chosen, out);
        chosen.pop();
        for host in &combination {
            used[*host] = false;
        }
    }
}

/// The specification's assignment: the highest total weight, then the
/// lexicographically smallest per-term sorted ID list in term order.
fn expected_assignment(
    terms: &[Demand],
    eligible: &[bool],
    weights: &[i32],
) -> Option<Vec<Vec<String>>> {
    let mut all = Vec::new();
    fillings(
        terms,
        eligible,
        0,
        &mut vec![false; POOL.len()],
        &mut Vec::new(),
        &mut all,
    );
    let mut best: Option<(i64, Vec<Vec<String>>)> = None;
    for filling in all {
        let total: i64 = filling
            .iter()
            .flatten()
            .map(|host| i64::from(weights[*host]))
            .sum();
        let key: Vec<Vec<String>> = filling
            .iter()
            .map(|term| term.iter().map(|host| POOL[*host].to_owned()).collect())
            .collect();
        match &best {
            Some((best_total, best_key))
                if *best_total > total || (*best_total == total && *best_key <= key) => {}
            _ => best = Some((total, key)),
        }
    }
    best.map(|(_, key)| key)
}

/// A request whose host `index` is eligible exactly where `eligible` says.
fn shaped_request(terms: &[Demand], eligible: &[bool], weights: &[i32]) -> Request {
    let requirement: Vec<Term> = terms
        .iter()
        .map(|(count, pool)| {
            if *count == 1 && pool.len() == 1 {
                Term::person(POOL[pool[0]])
            } else {
                Term::at_least(
                    *count,
                    Selector::Any(
                        pool.iter()
                            .map(|host| Selector::Ids(vec![POOL[*host].to_owned()]))
                            .collect(),
                    ),
                )
            }
        })
        .collect();
    let part = Part::new(Duration::from_minutes(30), Requirement::new(requirement));
    let mut request = request_over(
        Span::new(utc(9, 0), utc(10, 0)).expect("the fixture window is non-empty"),
        part,
        Vec::new(),
    );
    for (index, id) in POOL.iter().enumerate() {
        let mut participant = Participant::new(*id, name("UTC"));
        participant.calendar = Some(if eligible[index] {
            Vec::new()
        } else {
            vec![blocked(utc(0, 0), utc(23, 0))]
        });
        request.participants.push(participant);
        request.weights.insert((*id).to_owned(), weights[index]);
    }
    request.limit = Some(1);
    request
}

#[test]
fn multi_slot_assignments_agree_with_brute_force() {
    let engine = engine();
    let shapes: [Vec<Demand>; 4] = [
        // A two-slot term sharing a pool with a one-slot term.
        vec![(2, vec![0, 1, 2]), (1, vec![0, 1, 2, 3])],
        // One term needing three hosts.
        vec![(3, vec![0, 1, 2, 3])],
        // A named host plus a two-slot pool it does not share.
        vec![(1, vec![0]), (2, vec![1, 2, 3])],
        // Three one-slot terms over overlapping pools.
        vec![(1, vec![0, 1]), (1, vec![1, 2]), (1, vec![2, 3])],
    ];
    let mut checked = 0_u32;
    for terms in &shapes {
        for mask in 0_u32..16 {
            let eligible: Vec<bool> = (0..POOL.len()).map(|bit| mask & (1 << bit) != 0).collect();
            for a in [-2_i32, 0, 3] {
                for b in [-2_i32, 0, 3] {
                    for c in [-2_i32, 0, 3] {
                        for d in [-2_i32, 0, 3] {
                            let weights = [a, b, c, d];
                            let request = shaped_request(terms, &eligible, &weights);
                            let response = engine
                                .find_availability(&request)
                                .expect("the request is valid");
                            let got = response
                                .offers
                                .first()
                                .map(|offer| offer.events[0].assignment.clone());
                            assert_eq!(
                                got,
                                expected_assignment(terms, &eligible, &weights),
                                "terms {terms:?} eligible {mask:04b} weights {weights:?}"
                            );
                            checked += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(checked, 4 * 16 * 81);
}

// ---------------------------------------------------------------------------
// Overlapping multi-slot terms: the tie-break's witness, prune depth and the
// extreme weight range, through the real engine.
// ---------------------------------------------------------------------------

/// A selector listing `ids`, so a term's pool is exactly those hosts.
fn listing(ids: &[&str]) -> Selector {
    Selector::Any(
        ids.iter()
            .map(|id| Selector::Ids(vec![(*id).to_owned()]))
            .collect(),
    )
}

/// A requirement whose terms overlap and each ask for more than one host, over a
/// pool wider than the slots the whole requirement needs.
///
/// The matcher cuts every term to `slots` candidates, and the optimal flow's
/// witness is not the lexicographic answer: the first term's scan meets `ha`
/// while the witness holds `hb`, so the tie-break has to take a probe's witness
/// mid-loop before it settles. Every host is tied on weight, so only the
/// term-ordered ID comparison decides.
#[test]
fn overlapping_multi_slot_terms_replace_the_witness_to_reach_the_lexicographic_answer() {
    let engine = engine();
    let ids = ["ha", "hb", "hc", "hd", "he", "hf"];
    let day = Span::new(utc(0, 0), utc(23, 0)).expect("the fixture day is non-empty");
    let mut goal = Goal::single(
        name("UTC"),
        day,
        Duration::from_minutes(30),
        Requirement::new(vec![
            Term::at_least(1, listing(&ids)),
            Term::at_least(2, listing(&["ha", "hc", "hd", "he", "hf"])),
        ]),
    );
    goal.candidates = Some(vec![utc(9, 0)]);
    let mut request = Request::new(utc(0, 0), goal);
    request.participants = ids.iter().map(|id| host(id)).collect();
    request.explain = Some(Explain::new(10_000));
    request.limit = Some(1);

    let response = engine
        .find_availability(&request)
        .expect("the search succeeds");
    assert_eq!(response.offers.len(), 1);
    assert_eq!(
        response.offers[0].events[0].assignment,
        vec![
            vec!["ha".to_owned()],
            vec!["hc".to_owned(), "hd".to_owned()]
        ],
        "the smallest host fills the first term, then the next two the second"
    );

    // The confirmation path must reach the same answer for the same proposal.
    let checked = engine
        .check_slot(
            &request,
            &proposal(
                utc(9, 0),
                vec![
                    vec!["ha".to_owned()],
                    vec!["hc".to_owned(), "hd".to_owned()],
                ],
            ),
        )
        .expect("check_slot succeeds");
    assert!(checked.valid, "{:?}", checked.reasons);
}

/// The whole `i32` weight range across overlapping multi-slot terms: the two
/// heaviest hosts must fill the two-slot term even though every host is worth
/// either the largest or the smallest weight the model can carry, and the total is
/// compared as `i64` so nothing wraps.
#[test]
fn extreme_weights_across_overlapping_multi_slot_terms_keep_the_heaviest_pair() {
    let engine = engine();
    let ids = ["hi1", "hi2", "lo1", "lo2"];
    let day = Span::new(utc(0, 0), utc(23, 0)).expect("the fixture day is non-empty");
    let mut goal = Goal::single(
        name("UTC"),
        day,
        Duration::from_minutes(30),
        Requirement::new(vec![
            Term::at_least(2, listing(&ids)),
            Term::at_least(1, listing(&["lo1", "lo2"])),
        ]),
    );
    goal.candidates = Some(vec![utc(9, 0)]);
    let mut request = Request::new(utc(0, 0), goal);
    request.participants = ids.iter().map(|id| host(id)).collect();
    request.explain = Some(Explain::new(10_000));
    request.limit = Some(1);
    for id in ["hi1", "hi2"] {
        request.weights.insert(id.to_owned(), i32::MAX);
    }
    for id in ["lo1", "lo2"] {
        request.weights.insert(id.to_owned(), i32::MIN);
    }

    let response = engine
        .find_availability(&request)
        .expect("the search succeeds");
    assert_eq!(response.offers.len(), 1);
    assert_eq!(
        response.offers[0].events[0].assignment,
        vec![
            vec!["hi1".to_owned(), "hi2".to_owned()],
            vec!["lo1".to_owned()]
        ],
        "both i32::MAX hosts outrank every i32::MIN host, and the tie goes to `lo1`"
    );

    let checked = engine
        .check_slot(
            &request,
            &proposal(
                utc(9, 0),
                vec![
                    vec!["hi1".to_owned(), "hi2".to_owned()],
                    vec!["lo1".to_owned()],
                ],
            ),
        )
        .expect("check_slot succeeds");
    assert!(checked.valid, "{:?}", checked.reasons);
}
