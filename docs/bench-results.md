# match-core vs match-core-hp bench results

**中文：** [bench-results.zh-CN.md](./bench-results.zh-CN.md)

First published numbers from `match-bench` (`engine_cmp`).

## Environment

| Item | Value |
|------|-------|
| Date | 2026-07-18 |
| Host | macOS 14.4.1, Apple M1 Pro (arm64) |
| Rust | rustc 1.97.1 (8bab26f4f 2026-07-14) |
| Command | `cargo bench -p match-bench --bench engine_cmp -- --sample-size 20` |
| Workload size | 50_000 commands per scenario |
| Criterion | 0.5, sample size 20 |

## Notes

- Core path clones `BbOrder` each `on_order` (Java-shaped DTO cost is intentional).
- HP path uses `Copy` `HpCommand` (no clone noise favoring HP unfairly beyond real API shape).
- HP semantics are clean limit/market/cancel — not Java-equivalent.
- Ratio = `core_median_time / hp_median_time` (higher ⇒ HP faster).

## Results (median wall time per full workload)

| Scenario | core | hp | Ratio (core/hp) |
|----------|------|-----|-----------------|
| `rest_only` | 39.229 ms | 904.95 µs | **~43.3×** |
| `cross_full` | 60.028 ms | 899.35 µs | **~66.7×** |
| `partial_walk` | 104.56 ms | 1.6452 ms | **~63.6×** |
| `cancel_hot` | 524.18 ms | 9.9348 ms | **~52.8×** |
| `mixed` | 49.617 ms | 628.37 µs | **~79.0×** |

## Acceptance

Spec target: ≥5× on `cross_full` and `partial_walk`.

| Scenario | Target | Observed | Status |
|----------|--------|----------|--------|
| `cross_full` | ≥5× | ~66.7× | PASS |
| `partial_walk` | ≥5× | ~63.6× | PASS |

## Fair compare (`fair_cross`, fill_rate must be > 0)

```bash
cargo run -p match-bench --release --bin fair_compare -- --n 50000
```

| engine | n_orders | n_fills | fill_rate | orders/s | ns/order | notes |
|--------|----------|---------|-----------|----------|----------|-------|
| match-core | 50000 | 25000 | 0.50 | ~764K | ~1308 | same machine as above |
| match-core-hp | 50000 | 25000 | 0.50 | ~55M | ~18 | wall speedup ~72× vs core |

Protocol: [`fair-compare.md`](fair-compare.md). Do not compare against zero-fill ART/SIMD peaks.

## Phase A add-ons (2026-07-18 evening)

- Best-price cache + level pool + `LevelIndex`; optional `--features art` (byte-radix).
- Parity: `cargo test -p match-core-hp --features art --test art_parity`
- `match-contract` feature `hp-engine`: L2/L3/L1 span counters in `/metrics`
- `match-wal` async bench (sample): ~11M records/s append+flush on M1 Pro (`wal_bench 100000`)

Note: post–client_id map, `fair_compare` hp ~25–70× vs core depending on machine load; always require fill_rate≈0.5.

## HP L1 hot-path (2026-07-21)

Cancel O(1) map ops, tighter match loop, `get_or_insert_with` on levels. Details and pressure metrics: [`perf-hotpath-2026-07.md`](./perf-hotpath-2026-07.md).

`fair_cross` (HP, n=50000, fill_rate=0.5, median of 8 runs): ~54.3 ns/order (~18–19M orders/s). Same-day baseline before changes ~56.3 ns/order.

## Tier sweep (2026-07-22)

Resting depth × stream length × fill band (`tier_sweep`). Method, results, and bottlenecks: [`perf-tier-sweep.md`](./perf-tier-sweep.md). Quick high cell ~143 → ~81 ns/order; slowest cells remain high × 200k stream (~180–190 ns/order).
