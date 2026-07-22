# Tier sweep: depth × stream × fill (2026-07-22)

**中文：** [perf-tier-sweep.zh-CN.md](./perf-tier-sweep.zh-CN.md)

Design: [`specs/2026-07-22-tier-sweep-design.md`](./specs/2026-07-22-tier-sweep-design.md).  
CSV: [`bench-results/tier-sweep-final.csv`](./bench-results/tier-sweep-final.csv).  
Pre-change quick log: [`bench-results/tier-quick-pre-opt.txt`](./bench-results/tier-quick-pre-opt.txt).

---

## Environment

| Item | Value |
|------|-------|
| Date | 2026-07-22 |
| Host | macOS 14.4.1, Apple Silicon (`arm64`) |
| Rust | rustc 1.97.1, release |
| Engine | `match-core-hp` (BTree level index; ART off) |
| Command | `tier_sweep --preset default --runs 5` |

Warm is untimed; timed stream only; median of 5 runs after 1 warmup. Fill gates: low `0.10±0.05`, mid `0.50±0.05`, high `≥1.5`.

```bash
make tier-quick
make tier-sweep
```

---

## Gate check

All 27 default cells passed on the published CSV:

| Tier | Rule | Observed |
|------|------|----------|
| low | 0.10 ± 0.05 | `0.10` everywhere |
| mid | 0.50 ± 0.05 | `0.50` everywhere |
| high | ≥ 1.5 | `2.00` everywhere |

---

## Results (median `ns/order`)

### By resting depth

**rest = 1 000**

| stream | low | mid | high | high `peak_mapped` |
|--------|-----|-----|------|--------------------|
| 10k | 43.9 | 19.4 | 69.2 | 21 000 |
| 50k | 38.4 | 23.6 | 75.6 | 101 000 |
| 200k | 37.4 | 31.6 | 190.7 | 401 000 |

**rest = 10 000**

| stream | low | mid | high | high `peak_mapped` |
|--------|-----|-----|------|--------------------|
| 10k | 49.1 | 22.5 | 62.2 | 30 000 |
| 50k | 41.2 | 24.4 | 81.5 | 110 000 |
| 200k | 38.3 | 36.8 | 182.8 | 410 000 |

**rest = 100 000**

| stream | low | mid | high | high `peak_mapped` |
|--------|-----|-----|------|--------------------|
| 10k | 50.3 | 27.1 | 68.1 | 120 000 |
| 50k | 38.5 | 30.5 | 103.5 | 200 000 |
| 200k | 39.9 | 39.3 | 189.6 | 500 000 |

### Mean across `rest`

| stream | low | mid | high |
|--------|-----|-----|------|
| 10k | 47.8 | 23.0 | 66.5 |
| 50k | 39.3 | 26.2 | 86.8 |
| 200k | 38.5 | 35.9 | 187.7 |

Mid stays cheapest. Low is nearly flat in stream length. High grows sharply with stream length, not with resting depth.

### Before / after (quick cell)

`rest=10k stream=50k high`, same host:

| Stage | ns/order | orders/s |
|-------|----------|----------|
| Before | 143.1 | 7.0M |
| After | 81.5 | 12.3M (−43% per order) |

Other quick cells moved in the same direction (low/mid roughly −25% to −45%).  
`fair_compare --n 50000` after these changes: HP `fair_cross` ≈ 25.6 ns/order at fill_rate 0.5 (same-day check only).

---

## Code changes in this run

1. `client_to_id` uses `FxHashMap`.
2. External-id map is updated only when an order rests; fully filled limits and markets never insert.
3. Match loop keeps `taker_open` local and writes the store once after the walk.
4. `fill_order` updates existing levels via `get_mut` (no allocate-on-miss through `level_mut`).
5. Larger order free-list reserve, prefilled level pool, O(1) `live_len`, bench `event_cap=256`.

Level FIFO as `Vec`+head was tried on high×200k and regressed vs `VecDeque`; reverted.

---

## Bottleneck analysis

### What the matrix shows

| Finding | Evidence |
|---------|----------|
| Resting depth is not the high-walk driver | high×200k at rest 1k / 10k / 100k → 190.7 / 182.8 / 189.6 ns |
| Stream length is the high-walk driver | high mean ns: 66.5 → 86.8 → **187.7** as stream 10k → 50k → 200k |
| Mid feels deep books mildly | mid@50k: 23.6 → 30.5 ns as rest 1k → 100k |
| Low looks insert-bound | low mean stays ~38–48 ns across streams |
| High map footprint follows ask seed | `peak_mapped ≈ rest + 2×stream` |

### High-tier cost per order

Each timed buy is qty 2 lots → two fills. Path: store insert taker → two maker fills (FIFO debit, possible empty-level remove, `client_to_id.remove` for each maker) → remove spent taker. Fully filled takers do not touch the map.

Approx ns per fill (`ns/order ÷ 2`):

| stream | ns/fill | `peak_mapped` (rest≈1k) |
|--------|---------|-------------------------|
| 10k | ~31–35 | 21k |
| 50k | ~38–52 | 101k |
| 200k | ~91–95 | 401k |

Same fill shape, much higher per-fill cost at 200k: large live map + store teardown, not a different algorithm.

### Ranked costs (after this work)

1. **Maker map/store teardown on long high walks** — warm maps `2×stream` asks; timed path removes them. At stream=200k that is ~400k map removes and store frees on a map peaking near 400k–500k entries. Dominates high×200k; rest depth barely moves the needle.
2. **Empty-level / best-price updates while walking ~2k ticks** — secondary at current seed span; grows if walks cover more prices.
3. **Deep rest beside mid (and high@50k)** — idle bid map pollutes cache; second-order vs (1).
4. **Event `Vec` pushes** — negligible at 2 fills/order with `event_cap=256`.

Deferring taker map insert and using `FxHashMap` cut short high cells hard; they do not remove maker removes on long walks. Further wins need fewer mapped makers for non-cancellable liquidity, denser store slots, or a denser ask index if empty-level cost shows up in profiles.

For walk-heavy books, optimize stream×fill intensity first; resting depth alone is the wrong primary target.

---

## Summary

Gates green; compare within a fill band. Remaining cliff is **high fill × long stream** (maker `client_to_id` + store drain under a large live map). Resting depth is second-order. Short mid/high cells improved ~40% on the quick high cell; long high walks still need a representation or seeding change, not only match-loop micro-tweaks.
