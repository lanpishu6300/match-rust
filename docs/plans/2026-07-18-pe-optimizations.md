# perpetual_exchange-inspired Optimizations Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land Phase A→B→C from [2026-07-18-pe-optimizations-design.md](../specs/2026-07-18-pe-optimizations-design.md): best-price cache + level pool + optional ART, then `hp-engine` spans, then async `match-wal`.

**Architecture:** Keep default `BTreeMap` book; add `LevelIndex` trait and `art` feature. Production `match-contract` stays on `match-core` unless `hp-engine`. WAL is a separate crate, async-only on the hot path.

**Tech Stack:** Rust workspace `match-rust`, `match-core-hp`, criterion/`fair_compare`, optional ART (self-contained), tracing/`Instant` spans.

**Spec:** `docs/specs/2026-07-18-pe-optimizations-design.md`.

---

## File map

| Path | Role |
|------|------|
| `crates/match-core-hp/src/book.rs` | Best-price cache; level pool; uses `LevelIndex` |
| `crates/match-core-hp/src/level_index.rs` | Trait + `BTreeLevelIndex` |
| `crates/match-core-hp/src/art_index.rs` | `ArtLevelIndex` behind `feature = "art"` |
| `crates/match-core-hp/Cargo.toml` | `art` feature |
| `crates/match-core-hp/tests/best_price_cache.rs` | Cache / empty-best tests |
| `crates/match-core-hp/tests/art_parity.rs` | Same fills default vs art (cfg) |
| `crates/match-contract/...` | `hp-engine` feature + spans |
| `crates/match-wal/` | New async WAL crate |
| `docs/bench-results.md`, `e2e-budget.md`, `best-practices.md` | Numbers + mapping |

---

### Task 1: Best-price cache

**Files:**
- Modify: `match-rust/crates/match-core-hp/src/book.rs`
- Create: `match-rust/crates/match-core-hp/tests/best_price_cache.rs`

- [ ] **Step 1: Failing tests** — cancel/fill emptying best level updates `best_*`; insert better price updates cache.

- [ ] **Step 2: Implement** — fields `best_bid_tick` / `best_ask_tick`; update on `level_mut` first push, `remove_empty_level`, and after fill empties level. `best_bid`/`best_ask` read cache. `debug_assert` vs map in debug.

- [ ] **Step 3: `cargo test -p match-core-hp`** — all green.

- [ ] **Step 4: Commit** `feat(match-core-hp): O(1) best bid/ask cache`

---

### Task 2: Level object pool

**Files:**
- Modify: `book.rs` (pool `Vec<Level>`, recycle on `remove_empty_level`, take on new level)

- [ ] **Step 1: Test** — insert many distinct ticks then cancel all; second wave of inserts still correct (FIFO/best).

- [ ] **Step 2: Implement** — `level_pool: Vec<Level>`, cap e.g. 256; `or_insert_with` from pool; on remove push cleared Level back.

- [ ] **Step 3: Test + commit** `feat(match-core-hp): recycle empty price levels`

---

### Task 3: `LevelIndex` + BTree backend (behavior-preserving)

**Files:**
- Create: `level_index.rs`
- Modify: `book.rs`, `lib.rs`

- [ ] **Step 1: Extract trait** with `insert_level` / `get` / `get_mut` / `remove` / `best_tick` / `iter_depth(n)`.

- [ ] **Step 2: `BTreeLevelIndex` for asks (`BTreeMap<i64, Level>`) and bids (`BTreeMap<Reverse<i64>, Level>` or wrapper that maps max-bid).

- [ ] **Step 3: Book holds two indexes; existing tests pass unchanged.

- [ ] **Step 4: Commit** `refactor(match-core-hp): LevelIndex trait over BTreeMap`

---

### Task 4: ART index behind `art` feature

**Files:**
- Create: `art_index.rs` (minimal ordered map: insert/remove/min/max/iter by `i64` key)
- Modify: `Cargo.toml` `art = []`, `lib.rs` cfg
- Create: `tests/art_parity.rs`

- [ ] **Step 1: Minimal ART or radix map for `i64` → Level** (correctness first; Node16 SIMD optional later).

- [ ] **Step 2: Book type alias / cfg to select index impl.

- [ ] **Step 3: Parity test** — run same limit/cross sequence; assert equal `n_fills` and depth snapshots (test compiled twice or dual-run helper).

- [ ] **Step 4: `cargo test -p match-core-hp --features art` + default.

- [ ] **Step 5: Commit** `feat(match-core-hp): optional ART LevelIndex`

- [ ] **Step 6: Update** `docs/bench-results.md`, `docs/fair-compare.md`, `docs/best-practices.md`

---

### Task 5: Phase B — `hp-engine` + spans

**Files:**
- `match-contract/Cargo.toml` feature `hp-engine`
- `symbol_worker.rs` (or inbound path): adapter → HpWorker when feature on
- spans: `L3_adapt`, `L2_queue`, `L1_on_order`
- Update `e2e-budget.md` with measured placeholders filled if runnable

- [ ] Default build unchanged; feature build memory-transport smoke.
- [ ] Commit `feat(match-contract): optional hp-engine path with latency spans`

---

### Task 6: Phase C — `match-wal`

**Files:**
- New crate `match-wal`: SPSC/mpsc buffer, background flusher, `Async` mode
- Optional hook from hp engine/events
- Bench binary or criterion for records/sec
- Commit `feat(match-wal): async batched trade/order log`

---

### Task 7: Docs sync + fair_compare regression

- [ ] `cargo run -p match-bench --release --bin fair_compare -- --n 50000` exit 0
- [ ] Sync design status Approved in both spec copies
- [ ] Final commit if docs pending

---

## Spec coverage

| Spec section | Tasks |
|--------------|-------|
| §3.1 best cache | T1 |
| §3.2 level pool | T2 |
| §3.3 LevelIndex + art | T3, T4 |
| §4 Phase B | T5 |
| §5 Phase C | T6 |
| §7 docs | T4.6, T5, T7 |
