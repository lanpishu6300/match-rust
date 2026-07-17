use criterion::{criterion_group, criterion_main, Criterion};
use match_bench::workload;
use match_core::Engine;
use match_core_hp::HpEngine;

const N: usize = 50_000;

fn run_core(orders: &[match_core::BbOrder]) {
    let mut eng = Engine::new();
    for o in orders {
        eng.on_order(o.clone());
    }
}

fn run_hp(cmds: &[match_core_hp::HpCommand]) {
    let mut eng = HpEngine::with_capacity(cmds.len() + 8, 64);
    for c in cmds {
        eng.on_order(*c);
    }
}

fn bench_rest_only(c: &mut Criterion) {
    let (core_orders, hp_cmds) = workload::rest_only(N);
    c.bench_function("core_rest_only", |b| b.iter(|| run_core(&core_orders)));
    c.bench_function("hp_rest_only", |b| b.iter(|| run_hp(&hp_cmds)));
}

fn bench_cross_full(c: &mut Criterion) {
    let (core_orders, hp_cmds) = workload::cross_full(N);
    c.bench_function("core_cross_full", |b| b.iter(|| run_core(&core_orders)));
    c.bench_function("hp_cross_full", |b| b.iter(|| run_hp(&hp_cmds)));
}

fn bench_partial_walk(c: &mut Criterion) {
    let (core_orders, hp_cmds) = workload::partial_walk(N);
    c.bench_function("core_partial_walk", |b| b.iter(|| run_core(&core_orders)));
    c.bench_function("hp_partial_walk", |b| b.iter(|| run_hp(&hp_cmds)));
}

fn bench_cancel_hot(c: &mut Criterion) {
    let (core_orders, hp_cmds) = workload::cancel_hot(N);
    c.bench_function("core_cancel_hot", |b| b.iter(|| run_core(&core_orders)));
    c.bench_function("hp_cancel_hot", |b| b.iter(|| run_hp(&hp_cmds)));
}

fn bench_mixed(c: &mut Criterion) {
    let (core_orders, hp_cmds) = workload::mixed(N);
    c.bench_function("core_mixed", |b| b.iter(|| run_core(&core_orders)));
    c.bench_function("hp_mixed", |b| b.iter(|| run_hp(&hp_cmds)));
}

fn configure() -> Criterion {
    Criterion::default().sample_size(20)
}

criterion_group! {
    name = benches;
    config = configure();
    targets =
        bench_rest_only,
        bench_cross_full,
        bench_partial_walk,
        bench_cancel_hot,
        bench_mixed
}
criterion_main!(benches);
