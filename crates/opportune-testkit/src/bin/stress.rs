//! The stress run: the generated engine-versus-oracle comparison at tens of
//! thousands of cases, which is far more than the test gate runs.
//!
//! `make stress` drives this binary. It runs the same comparison the property
//! suites run — [`opportune_testkit::a1::compare_with_oracle`] and
//! [`opportune_testkit::a2::compare_with_oracle`], the single implementation of
//! each — over generated single-participant and multi-participant corpora, and
//! fails on the first disagreement with the shrunk case that produced it.
//!
//! The case budget is split across the host's cores because each case costs
//! milliseconds: the comparison is pure and every case is independent, so the
//! threads share nothing but the read-only database and engine. Each thread seeds
//! its own generator, and the totals below are what was actually compared, not
//! what was asked for.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::process::ExitCode;

use opportune::Engine;
use opportune::time::TimeZoneDatabase;
use opportune_testkit::a1::{A1Case, compare_with_oracle as compare_single};
use opportune_testkit::a2::{A2Case, compare_with_oracle as compare_multi};
use opportune_testkit::generators::{a1_case, a2_case};
use opportune_testkit::scenarios::RULE_KINDS;
use proptest::test_runner::{Config, RngSeed, TestRunner};

/// The comparisons this run must perform: about fifty thousand, which is far
/// more than the property gate's few hundred per property and is what makes the
/// run worth doing before a release.
const SINGLE_CASES: u64 = 20_000;
/// The multi-participant comparisons, which also carry the caps, occurrences,
/// attendees, weights and rest rules.
const MULTI_CASES: u64 = 30_000;

/// What a shard of the run observed, so the summary can prove the corpus reached
/// every rule kind rather than assuming it did.
#[derive(Default)]
struct Coverage {
    /// How many rules of each kind the corpus carried, keyed by the kind's name.
    kinds: BTreeMap<&'static str, u64>,
    occurrences: u64,
    zone_legs: u64,
    attendees: u64,
    weighted: u64,
}

impl Coverage {
    fn add(&mut self, other: &Self) {
        for (kind, count) in &other.kinds {
            *self.kinds.entry(kind).or_default() += count;
        }
        self.occurrences += other.occurrences;
        self.zone_legs += other.zone_legs;
        self.attendees += other.attendees;
        self.weighted += other.weighted;
    }

    fn record_single(&mut self, case: &A1Case) {
        for rule in &case.rules {
            *self.kinds.entry(rule.kind()).or_default() += 1;
        }
        if !case.participant.zones.periods.is_empty() {
            self.zone_legs += 1;
        }
    }

    fn record_multi(&mut self, case: &A2Case) {
        for rule in &case.rules {
            *self.kinds.entry(rule.plan.kind()).or_default() += 1;
        }
        if !case.occurrences.is_empty() {
            self.occurrences += 1;
        }
        if case
            .participants
            .iter()
            .any(|participant| !participant.zones.periods.is_empty())
        {
            self.zone_legs += 1;
        }
        if case
            .participants
            .iter()
            .any(|participant| participant.role == opportune::tags::Role::Attendee)
        {
            self.attendees += 1;
        }
        if case
            .participants
            .iter()
            .any(|participant| participant.weight != 0)
        {
            self.weighted += 1;
        }
    }
}

/// One thread's share of the run.
#[derive(Default)]
struct Shard {
    single: u64,
    multi: u64,
    coverage: Coverage,
}

fn main() -> ExitCode {
    let db = TimeZoneDatabase::bundled().expect("the bundled database is present");
    let engine = Engine::builder()
        .build()
        .expect("the bundled database builds an engine");
    let threads = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);

    let shards = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|index| {
                let single = share(SINGLE_CASES, threads, index);
                let multi = share(MULTI_CASES, threads, index);
                let db = &db;
                let engine = &engine;
                scope.spawn(move || run_shard(index, single, multi, db, engine))
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .expect("a stress thread stopped before finishing its shard, above")
            })
            .collect::<Vec<Shard>>()
    });

    let mut total = Shard::default();
    for shard in &shards {
        total.single += shard.single;
        total.multi += shard.multi;
        total.coverage.add(&shard.coverage);
    }
    let comparisons = total.single + total.multi;

    println!(
        "stress: {comparisons} oracle comparisons on {threads} threads \
         ({} single-participant, {} multi-participant)",
        total.single, total.multi
    );
    let kinds: Vec<String> = RULE_KINDS
        .iter()
        .map(|kind| {
            format!(
                "{kind} {}",
                total.coverage.kinds.get(kind).copied().unwrap_or(0)
            )
        })
        .collect();
    println!("stress: rule kinds exercised: {}", kinds.join(", "));
    println!(
        "stress: occurrences {} cases, zone legs {} cases, attendees {} cases, \
         non-zero weights {} cases",
        total.coverage.occurrences,
        total.coverage.zone_legs,
        total.coverage.attendees,
        total.coverage.weighted
    );

    let mut missing = Vec::new();
    if total.single != SINGLE_CASES || total.multi != MULTI_CASES {
        missing.push(format!(
            "compared {} + {} cases, not the {SINGLE_CASES} + {MULTI_CASES} asked for",
            total.single, total.multi
        ));
    }
    for kind in RULE_KINDS {
        if total.coverage.kinds.get(kind).copied().unwrap_or(0) == 0 {
            missing.push(format!("the corpus never generated a `{kind}` rule"));
        }
    }
    // A kind the corpus carried but this list does not name would be tallied
    // silently and never checked, so the tally's keys are the contract too.
    for kind in total.coverage.kinds.keys() {
        if !RULE_KINDS.contains(kind) {
            missing.push(format!("the corpus carried an unlisted rule kind `{kind}`"));
        }
    }
    for (what, count) in [
        ("an occurrence", total.coverage.occurrences),
        ("a zone leg", total.coverage.zone_legs),
        ("an attendee", total.coverage.attendees),
        ("a non-zero weight", total.coverage.weighted),
    ] {
        if count == 0 {
            missing.push(format!("the corpus never generated {what}"));
        }
    }
    if missing.is_empty() {
        ExitCode::SUCCESS
    } else {
        for problem in missing {
            eprintln!("stress: {problem}");
        }
        ExitCode::FAILURE
    }
}

/// How many of `total` cases thread `index` takes: the remainder is spread one
/// case at a time over the first threads, so the shares always sum to `total`.
fn share(total: u64, threads: usize, index: usize) -> u64 {
    let threads = u64::try_from(threads).expect("a thread count fits in u64");
    let index = u64::try_from(index).expect("a thread index fits in u64");
    total / threads + u64::from(index < total % threads)
}

fn run_shard(
    index: usize,
    single: u64,
    multi: u64,
    db: &TimeZoneDatabase,
    engine: &Engine,
) -> Shard {
    let seed = u64::try_from(index).expect("a thread index fits in u64");
    let (single, single_coverage) = run_single(seed * 2, single, db, engine);
    let (multi, multi_coverage) = run_multi(seed * 2 + 1, multi, db, engine);
    let mut coverage = single_coverage;
    coverage.add(&multi_coverage);
    Shard {
        single,
        multi,
        coverage,
    }
}

/// Runs `cases` single-participant comparisons seeded by `seed`, stopping at the
/// first disagreement with the shrunk case that produced it.
fn run_single(seed: u64, cases: u64, db: &TimeZoneDatabase, engine: &Engine) -> (u64, Coverage) {
    let compared = Cell::new(0_u64);
    let coverage = RefCell::new(Coverage::default());
    TestRunner::new(config(cases, seed))
        .run(&a1_case(), |case| {
            let outcome = compare_single(&case, db, engine);
            coverage.borrow_mut().record_single(&case);
            if outcome.is_ok() {
                compared.set(compared.get() + 1);
            }
            outcome
        })
        .expect("every generated single-participant case must agree with the oracle");
    (compared.get(), coverage.into_inner())
}

/// Runs `cases` multi-participant comparisons seeded by `seed`.
fn run_multi(seed: u64, cases: u64, db: &TimeZoneDatabase, engine: &Engine) -> (u64, Coverage) {
    let compared = Cell::new(0_u64);
    let coverage = RefCell::new(Coverage::default());
    TestRunner::new(config(cases, seed))
        .run(&a2_case(), |case| {
            let outcome = compare_multi(&case, db, engine);
            coverage.borrow_mut().record_multi(&case);
            if outcome.is_ok() {
                compared.set(compared.get() + 1);
            }
            outcome
        })
        .expect("every generated multi-participant case must agree with the oracle");
    (compared.get(), coverage.into_inner())
}

/// A runner that performs exactly `cases` comparisons with a reproducible seed
/// and writes no regression file: the shrink output is the report.
fn config(cases: u64, seed: u64) -> Config {
    Config {
        cases: u32::try_from(cases).expect("a case budget fits in u32"),
        rng_seed: RngSeed::Fixed(seed),
        failure_persistence: None,
        ..Config::default()
    }
}
