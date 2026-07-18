# Phased Optimizations Inspired by perpetual_exchange

**中文：** [2026-07-18-pe-optimizations-design.zh-CN.md](./2026-07-18-pe-optimizations-design.zh-CN.md)

**Date:** 2026-07-18  
**Status:** Approved / Implemented (A–C skeleton) — 2026-07-18  
**Prerequisite:** [2026-07-18-match-core-hp-design.md](./2026-07-18-match-core-hp-design.md)  
**Reference code:** external `crypto-exchange` (perpetual_exchange R&D); this repository (`match-rust`)  

**Decision summary:** Order **A → B → C**; Phase A chooses **A2**; landing approach chooses **Option 1** (`LevelIndex` trait + feature `art`)

---

## 0. Decision Summary

| Item | Choice |
|------|--------|
| Order | Phase A (L1) → Phase B (L2/L3) → Phase C (async WAL) |
| Phase A scope | Best-price cache + Level pooling + **optional ART** (feature `art`) |
| Default book index | Remains `BTreeMap` (fallbackable, default-testable) |
| ART | `--features art`; fill counts on the same sequence must match the default path |
| SIMD | **Do not port** C++ PnL / inflated-path numbers; ART Node16 SIMD only as optional micro-opt inside art, not required for A |
| Production default | `match-contract` still defaults to `match-core`; hp / art / wal are all experimental switches |
| Benchmark discipline | Every micro-benchmark must have `fill_rate > 0` (`fair_compare` protocol) |

---

## 1. Background and Goals

`perpetual_exchange` (`crypto-exchange`) offers an optimization narrative around memory pools, ART, SIMD, async persistence, etc.; `match-core-hp` already has fixed-point, price-level book, SPSC, false-sharing isolation, and more. This design lands the **borrowable and verifiable** parts layer by layer, avoiding treating a few more L1 nanoseconds as an end-to-end win (see `match-rust/docs/e2e-budget.md`).

### 1.1 Goals

1. **A**: L1 closer to mechanical sympathy (best price O(1), Level reuse, optional ART).  
2. **B**: Contract experimental path can use `HpWorker` + adapter, and emit L3/L2/L1 segmented latency.  
3. **C**: Async batched WAL, comparable in order of magnitude to C++ persistence, without blocking the match hot path.

### 1.2 Non-Goals

- Do not replace the production default engine; do not connect to real RocketMQ.  
- Do not port accounts / liquidation / rate limiting / a full ART+SIMD engine shell.  
- Do not build Aeron IPC (separate item).  
- Do not put 0% fill ART+SIMD “peak” numbers into the valid comparison table.

---

## 2. Architecture Landing Points

```text
match-rust/crates/
├── match-core/          # Equivalence track (unchanged)
├── match-core-hp/       # Phase A: LevelIndex / best_* / level pool / feature art
├── match-bench/         # fair_compare + art on/off comparison
├── match-contract/      # Phase B: feature hp-engine + spans
├── match-wal/           # Phase C: async batch write (new small crate)
└── match-protocol/      # Unchanged
```

| Component | Responsibility | Forbidden |
|-----------|----------------|-----------|
| `LevelIndex` | tick → Level insert/delete/best-price iteration | Hot-path string / BigDecimal allocation |
| `art` feature | ART implementation replaces default BTree index | Changing match semantics |
| `hp-engine` feature | Contract worker may optionally use hp | Enabled by default; default deps still core |
| `match-wal` | Buffer + background flush of fill/order logs | Sync fsync blocking inside `on_order` (async mode) |

---

## 3. Phase A — L1 (match-core-hp)

### 3.1 Best-Price Cache

Maintain on `Book`:

- `best_bid_tick: Option<i64>`
- `best_ask_tick: Option<i64>`

Maintenance rules:

- Level goes empty→non-empty, or a better price appears: update cache.  
- Best level emptied by cancel / take: fetch next best from index (or `None`).  
- `best_bid()` / `best_ask()` read the cache on the hot path; debug asserts may cross-check the index.

### 3.2 Level Pooling

- `OrderStore` already has a slot free-list; this phase focuses on: **recycle empty `Level`s (including `VecDeque`)** to cut level create/destroy allocation.  
- Pool size is capped; beyond the cap, discard empty Levels and let the allocator reclaim.

### 3.3 `LevelIndex` + feature `art`

```text
trait LevelIndex {
    fn get_mut(&mut self, tick: i64) -> &mut Level;
    fn remove_if_empty(&mut self, tick: i64);
    fn best_tick(&self) -> Option<i64>;   // asks: min; bids: max
    fn next_after_best(&self) -> Option<i64>; // recompute after cache invalidate
    // + ordered iteration needed for depth traversal (top N levels)
}
```

- Default: `BTreeLevelIndex` (today’s `BTreeMap` behavior).  
- `art`: `ArtLevelIndex`, key = big-endian / comparable byte form of `i64` tick (implementation may be a slim in-house ART, or evaluate a mature crate; **the semantic layer does not expose crate details**).  
- `Book` holds one `LevelIndex` each for bids/asks (bids side uses “higher price is better” comparison convention, handleable in a wrapper layer).

**SIMD:** Not in A’s acceptance scope. If the art implementation uses SIMD for Node16 lookup, it must still pass same-sequence fill consistency tests; do not advertise throughput with no fills in isolation.

### 3.4 Acceptance (A)

| Item | Standard |
|------|----------|
| Correctness | Existing hp integration tests all green |
| Fair benchmark | `fair_compare --n 50000` → fill_rate≈0.5, exit 0 |
| art consistency | Same workload, default vs `--features art` have identical `n_fills` |
| Performance | Relative to pre-A baseline: `fair_cross` no significant regression; cache-hit path has documented numbers |

---

## 4. Phase B — L2/L3 (match-contract)

### 4.1 Feature `hp-engine`

- Off (default): behavior unchanged from today (`match-core`).  
- On: symbol worker inbound goes `adapter` → `HpCommand` → `HpWorker` (or same-thread `HpEngine`, selectable by config).  
- `match-contract` **must not** force-pull art in the default dependency graph.

### 4.2 Segmented Timing

Instrument the single-order path (`Instant` or tracing span):

| Span | Meaning |
|------|---------|
| `L3_adapt` | BbOrder / JSON boundary → HpCommand |
| `L2_queue` | Enqueue → worker dequeue |
| `L1_on_order` | `HpEngine::on_order` |

Results backfill the measured columns in `match-rust/docs/e2e-budget.md` (template already exists).

### 4.3 Acceptance (B)

- Feature off: existing contract unit tests / startup paths unbroken.  
- Feature on: under memory transport, rest/take/cancel works; logs or metrics show the three latency segments.  
- Real NameServer still not required.

---

## 5. Phase C — Async WAL (match-wal)

### 5.1 Model (Inspired by C++ Persistence Buffer)

```text
Match thread --append(record)--> lock-free/SPSC buffer --background thread--> batch write(+optional fsync)
```

- Record types: minimal set `OrderAccepted` / `Fill` / `Cancel` (binary or length-prefixed; phase one may use bincode / hand layout).  
- Modes: `Async` (default experiment) and optional `Sync` (correctness testing).  
- Buffer full: backpressure is `Busy`/blocking append (document clearly); **silent log drop is forbidden** (unless explicit `BestEffort` config, off by default).

### 5.2 Attachment

- Optional mount only on the hp experimental path; core default path is not forced.  
- WAL failure: metrics + logs; Async mode does not roll back already-matched results (consistent with “match first, then record”; must be called out in ops docs).

### 5.3 Acceptance (C)

- Standalone micro-benchmark: records/sec, average flush latency (use `crypto-exchange` persistence order-of-magnitude as reference; do not mix into L1 CSV).  
- Async: `on_order` hot path has no synchronous disk wait (assertable via hooks/counters).  
- Restart replay is **out of scope** this phase (write-ahead only; no full event-sourcing replay).

---

## 6. Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| ART bugs scramble level order | Same-sequence fill compare + depth snapshot compare tests |
| Best-price cache out of sync | debug assert; unit tests specifically cancel best / take empty level |
| hp-engine accidentally enabled in prod | Feature off by default; README warning |
| WAL backpressure stalls matching | Buffer sizing + metrics; stress-test docs |
| Inflated C++ comparison numbers | Enforce fair_compare discipline |

---

## 7. Docs and Benchmark Updates

- `match-rust/docs/best-practices.md`: add perpetual_exchange → this-repo mapping rows.  
- `match-rust/docs/bench-results.md`: after A, append cache/art numbers.  
- `match-rust/docs/e2e-budget.md`: after B, fill measured values.  
- `match-rust/docs/fair-compare.md`: note art feature comparison usage.

---

## 8. Implementation Order (Refined at Planning Stage)

1. A1 best-price cache + tests  
2. A1 Level pool + tests  
3. A2 `LevelIndex` abstraction (default BTree, behavior unchanged)  
4. A2 ART implementation + feature + consistency tests  
5. B `hp-engine` + spans  
6. C `match-wal` + micro-benchmarks  

Each step can land as an independent commit; do not start B’s production wiring before A completes (WAL crate skeleton may be written in parallel, but not attached to contract).

---

## 9. Open Items (Closed at Implementation-Plan Stage)

- ART: in-house slim vs external crate (judge by license, `i64` key, ordered min/max support).  
- WAL record encoding: hand-fixed layout vs bincode.  
- Under `hp-engine`, whether outbound events first convert back to Java-shaped JSON (recommendation: yes, keep Topic compatibility; conversion lives at L2′).
