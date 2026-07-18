# Performance

**中文：** [zh/Performance.md](../zh/Performance.md)

## Principles

1. **Never rank engines with `fill_rate == 0`** — mark INVALID ([fair-compare](../../fair-compare.md)).
2. Separate **L1 microkernel** latency from **end-to-end** (MQ/JSON often dominates) — [e2e-budget](../../e2e-budget.md).
3. Published numbers: [bench-results](../../bench-results.md).

## Commands

```bash
make fair                          # fair_compare CSV, exit 1 if fill_rate ≈ 0
make bench                         # criterion engine_cmp
cargo test -p match-core-hp --features art --test art_parity
cargo run -p match-wal --release --bin wal_bench -- 100000
```

## Companion C++ project

[crypto-exchange](https://github.com/lanpishu6300/crypto-exchange) reports ART/SIMD microbenches. Compare only under the same fair-cross protocol and non-zero fill rate.
