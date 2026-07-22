# Tier sweep bench design (2026-07-22)

**中文：** [2026-07-22-tier-sweep-design.zh-CN.md](./2026-07-22-tier-sweep-design.zh-CN.md)

## Goal

Measure `match-core-hp` under a controlled matrix of **resting depth × stream length × fill intensity**, then spend optimization effort on the cells that actually dominate wall time.

Results and bottleneck notes: [`../perf-tier-sweep.md`](../perf-tier-sweep.md).

## Binary

```bash
make tier-quick    # 4 cells
make tier-sweep    # 27 cells (default preset)
# or:
cargo run -p match-bench --release --bin tier_sweep -- \
  --preset default --runs 5 --out docs/bench-results/tier-sweep-final.csv
```

| Flag | Default | Meaning |
|------|---------|---------|
| `--preset` | `quick` | `quick` (4 cells) or `default` (27) |
| `--runs` | 5 | Timed runs per cell; report median |
| `--warmup` | 1 | Discarded timed passes before measuring |
| `--out` | — | Optional CSV path (same columns as stdout) |
| `--loose` | false | Print rows even if a fill gate fails |

Engine under test: HP only. Core is out of scope for this matrix (use `fair_compare` for core vs HP).

## Workload

1. **Warm (untimed):** seed `rest` non-crossing buy limits across many ticks. High tier also seeds `2×stream` asks so walks have liquidity.
2. **Timed:** run `stream` commands shaped for the fill band (see table).
3. **Report:** median `elapsed_ns` → `ns/order`, `orders/s`, `fills/s`, `fill_rate`, `peak_mapped` (peak `client_to_id` size during the timed stream).

## Preset matrix

| rest | stream | tier | Shape |
|------|--------|------|--------|
| 1k / 10k / 100k | 10k / 50k / 200k | low | Mostly rest + sparse 1:1 crosses → `fill_rate ≈ 0.10` |
| same | same | mid | Half sell / half buy at one tick (`fair_cross`-style) → `≈ 0.50` |
| same | same | high | Aggressive buys (qty 2 lots) against seeded asks → `fill_rate ≥ 1.5` (here 2.0) |

**Gates:** low/mid within ±0.05 of target; high requires `fill_rate ≥ 1.5`. Reject zero-fill peaks.

`quick` preset: `(1k,10k,low)`, `(10k,50k,mid)`, `(10k,50k,high)`, `(100k,50k,mid)`.

## How to use the numbers

- Compare cells **within one tier** (same fill shape). Cross-tier ns/order is not a fairness ranking.
- Prefer multi-run median on a quiet host; treat single-run spikes as noise.
- Record host, rustc, and CSV path when publishing. Link new CSVs from [`../bench-results.md`](../bench-results.md).

## Optimization loop

1. Capture baseline CSV (`--preset default` or `quick`).
2. Attribute heat from the matrix (stream vs rest vs fill band), not from intuition alone.
3. Change one cost class (map, store, level index, event buffer); re-run the same preset.
4. Keep or revert based on the worst cells; write deltas in [`../perf-tier-sweep.md`](../perf-tier-sweep.md).
