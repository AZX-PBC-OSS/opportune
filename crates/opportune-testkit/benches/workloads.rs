//! Criterion benchmarks for the engine's latency targets and for its scaling
//! with the input the configured limits allow.
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
//! Run them with `cargo bench -p opportune-testkit`, or with `-- --quick` for a
//! smoke run. CI does not measure them: a shared runner moves absolute
//! milliseconds further than a change to this code does, so the readings are taken
//! by hand and recorded against the latency targets.

use criterion::{Criterion, criterion_group, criterion_main};
use opportune::Engine;
use opportune_testkit::workloads::{limit_workloads, slot_workload, spec_workloads};

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

criterion_group!(benches, search, limits);
criterion_main!(benches);
