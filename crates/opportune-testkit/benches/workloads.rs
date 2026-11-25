//! Criterion benchmarks for the engine's scaling with input, and for the latency
//! targets of its standard workload.
//!
//! Only the `spec10` group has latency targets; [`opportune_testkit::workloads`]
//! states them. The `limits` and `adversarial` groups are scaling readings with no
//! target and no threshold, and no wall-clock assertion runs in CI: a shared runner
//! moves absolute milliseconds further than a change to this code does, so the
//! readings are taken by hand and recorded here.
//!
//! The `spec10` group covers the standard 50-participant, 14-day, 5-minute-grid
//! shape measured through the public [`Engine`]: a search for a single-term
//! requirement, a search for one whose pools overlap, and the confirmation of
//! one offered slot. `opportune_testkit::workloads` builds the requests and
//! documents what they carry; `tests/workloads_test.rs` pins their shape so a
//! degenerate input cannot flatter a measurement.
//!
//! The `limits` group is the same calendars with about three times the blocks per
//! host and, once, an offer limit that truncates almost every offer. Both readings
//! are ratios rather than absolute milliseconds, which a shared host moves more than
//! the code does. `limits/dense_blocks` against `spec10/single_term`, the same shape
//! at the base density, grows when the block scan stops being proportional to
//! the blocks; the two `limits` benchmarks against each other, identical but for the
//! offer limit, grow when the post-limit feasibility check does.
//!
//! The `adversarial` group is one participant whose zone periods shuttle across the
//! date line, at each size in [`REST_ZONE_PERIOD_SIZES`]. Only the schedule grows
//! with the size, so the readings show how `rest`'s self-conflict construction
//! scales with the number of late and early segments per local date. The busiest
//! date pair presents 81, 289, 1,089, 4,225, 16,770 and 51,062 late/early segment
//! pairs at those sizes — quadratic in the legs — which is exactly the population a
//! build that pairs them off one against another multiplies into work.
//!
//! # Recorded reading
//!
//! This group has no latency target. It has one recorded before/after comparison,
//! taken when the rewrite landed and not reproducible without reverting the
//! algorithm, which is why it lives here rather than in a gate. On one development
//! host the rewrite — one widen per side, two sorts and one merge, replacing the
//! pairwise product — dropped the measured call by about 7% at 512 zone periods and
//! 23% at 1,000, while below 512 the linear cost of building the many-slice sets
//! dominated and the readings sat inside run-to-run noise. A full run of the current
//! tree (default sample count) read mean per-call times of about 16 µs, 27 µs,
//! 45 µs, 78 µs, 151 µs and 315 µs at 32, 64, 128, 256, 512 and 1,000 zone periods.
//!
//! `--quick` is a smoke run, not a comparison: a run of this group in this tree
//! reported 32-740 µs across the six sizes, including a spurious "+79% change" at
//! 1,000 zone periods against the previous full run, all inside its own confidence
//! intervals. Only the full sample count makes a reading stable, and even then a
//! Criterion `base`/`new` directory under `target/` is not a committed comparison;
//! no test asserts a duration. The deterministic guard is the structural cost
//! invariant in `rest.rs` — at most `L + E` spans per date pair, where the pairwise
//! product emitted up to `L × E` — which fails if the product returns.
//!
//! Run them with `cargo bench -p opportune-testkit`, or with `-- --quick` for a
//! smoke run.

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use opportune::Engine;
use opportune_testkit::workloads::{
    REST_ZONE_PERIOD_SIZES, limit_workloads, rest_zone_period_workload, slot_workload,
    spec_workloads,
};

fn engine() -> Engine {
    Engine::builder()
        .build()
        .expect("the bundled time zone database builds")
}

fn search(c: &mut Criterion) {
    let engine = engine();
    let mut group = c.benchmark_group("spec10");
    for workload in spec_workloads(&engine) {
        let request = &workload.request;
        group.bench_function(workload.id, |bencher| {
            bencher.iter(|| {
                engine
                    .find_availability(request)
                    .expect("a workload request is valid")
            });
        });
    }
    let confirm = slot_workload(&engine);
    group.bench_function(confirm.id, |bencher| {
        bencher.iter(|| {
            engine
                .check_slot(&confirm.request, &confirm.proposal)
                .expect("a found slot is a valid proposal")
        });
    });
    group.finish();
}

fn adversarial(c: &mut Criterion) {
    let engine = engine();
    let mut group = c.benchmark_group("adversarial");
    for zone_periods in REST_ZONE_PERIOD_SIZES {
        let request = rest_zone_period_workload(&engine, zone_periods);
        group.bench_with_input(
            BenchmarkId::from_parameter(zone_periods),
            &request,
            |bencher, request| {
                bencher.iter(|| {
                    engine
                        .find_availability(request)
                        .expect("the rest-scaling workload is a valid request")
                });
            },
        );
    }
    group.finish();
}

fn limits(c: &mut Criterion) {
    let engine = engine();
    let mut group = c.benchmark_group("limits");
    for workload in limit_workloads(&engine) {
        let request = &workload.request;
        group.bench_function(workload.id, |bencher| {
            bencher.iter(|| {
                engine
                    .find_availability(request)
                    .expect("a workload request is valid")
            });
        });
    }
    group.finish();
}

criterion_group!(benches, search, limits, adversarial);
criterion_main!(benches);
