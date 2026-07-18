# match-core-hp Implementation Plan

**中文：** [2026-07-18-match-core-hp.zh-CN.md](./2026-07-18-match-core-hp.zh-CN.md)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a dual-track high-performance matching core (`match-core-hp`) with fixed-point price-level books and a `match-bench` crate that proves ≥5× throughput vs `match-core` on hot scenarios, without changing production defaults.

**Architecture:** New crate `match-core-hp` owns `HpEngine` (i64 ticks/lots, price-level + FIFO, preallocated order slots). Thin `adapter` converts protocol/`BbOrder` only at boundaries. `match-bench` runs identical logical sequences on core vs hp. `match-contract` stays on `match-core`.

**Tech Stack:** Rust 2021, `criterion` (dev), optional `rtrb` for SPSC in H3, existing `match-protocol` / `match-core` for comparison only.

**Spec:** `docs/specs/2026-07-18-match-core-hp-design.md`

**Workdir:** `.` on `feature/rust-match-engines` (or new branch `feature/match-core-hp`). Always `export PATH="$HOME/.cargo/bin:$PATH"`.

---

## File map

| Path | Responsibility |
|------|----------------|
| `crates/match-core-hp/Cargo.toml` | HP crate |
| `crates/match-core-hp/src/lib.rs` | Exports |
| `crates/match-core-hp/src/types.rs` | `Side`, `HpOrder`, `HpEvent`, `SymbolScale` |
| `crates/match-core-hp/src/book.rs` | Price-level book + FIFO + order slots |
| `crates/match-core-hp/src/engine.rs` | `HpEngine::on_order` limit/market/cancel |
| `crates/match-core-hp/src/adapter.rs` | Decimal/string ↔ tick/lot; optional from `match_protocol::BbOrder` |
| `crates/match-core-hp/src/spsc.rs` | H3: SPSC cmd ring + worker loop |
| `crates/match-core-hp/tests/*.rs` | Correctness tests |
| `crates/match-bench/Cargo.toml` | Bench binary crate |
| `crates/match-bench/benches/engine_cmp.rs` | criterion core vs hp |
| `docs/bench-results.md` | First published numbers |
| `Cargo.toml` (workspace) | Add members |
| `README.md` | Link hp + bench |

---

### Task 1: Scaffold `match-core-hp` + workspace member

**Files:**
- Create: `crates/match-core-hp/Cargo.toml`
- Create: `crates/match-core-hp/src/lib.rs`
- Modify: `Cargo.toml` (workspace members)
- Modify: `README.md` (one line under docs)

- [ ] **Step 1: Add crate**

```toml
# crates/match-core-hp/Cargo.toml
[package]
name = "match-core-hp"
version.workspace = true
edition.workspace = true

[dependencies]
thiserror = { workspace = true }
```

```rust
// crates/match-core-hp/src/lib.rs
//! High-performance matching core (fixed-point, price-level book).
//! Not Java-equivalent; not used by match-contract by default.
```

Add `"crates/match-core-hp"` to workspace `members`.

- [ ] **Step 2: Build**

Run: `cargo build -p match-core-hp`  
Expected: success

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml crates/match-core-hp README.md
git commit -m "$(cat <<'EOF'
chore: scaffold match-core-hp crate

EOF
)"
```

---

### Task 2: Types + scale conversion (TDD)

**Files:**
- Create: `crates/match-core-hp/src/types.rs`
- Create: `crates/match-core-hp/src/scale.rs`
- Modify: `crates/match-core-hp/src/lib.rs`
- Create: `crates/match-core-hp/tests/scale_convert.rs`

- [ ] **Step 1: Failing tests**

```rust
use match_core_hp::{SymbolScale, to_tick, to_lot, from_tick};

#[test]
fn price_to_tick_scale_2() {
    let s = SymbolScale { price_scale: 2, qty_scale: 6 };
    assert_eq!(to_tick(&s, "100.05").unwrap(), 10005);
    assert_eq!(from_tick(&s, 10005), "100.05");
}

#[test]
fn rejects_overflow_digits() {
    let s = SymbolScale { price_scale: 2, qty_scale: 6 };
    assert!(to_tick(&s, "100.999").is_err()); // more fractional digits than scale
}
```

Implement `to_tick`/`to_lot` with exact scale (reject excess fractional digits or define banker's round — **pick reject excess** for determinism).

- [ ] **Step 2: FAIL → implement → PASS**

- [ ] **Step 3: Commit** `feat(match-core-hp): fixed-point scale conversion`

---

### Task 3: Order slots + price-level book (TDD)

**Files:**
- Create: `crates/match-core-hp/src/book.rs`
- Create: `crates/match-core-hp/src/order_store.rs`
- Create: `crates/match-core-hp/tests/book_order.rs`

**API sketch:**

```rust
pub struct OrderStore { /* Vec<Option<HpOrder>> + free list */ }
pub struct Book {
    bids: BTreeMap<Reverse<i64>, Level>,
    asks: BTreeMap<i64, Level>,
    store: OrderStore,
}
impl Book {
    pub fn insert_limit(&mut self, order: HpOrder) -> u64; // returns id
    pub fn cancel(&mut self, id: u64) -> bool;
    pub fn best_ask(&self) -> Option<i64>;
    pub fn best_bid(&self) -> Option<i64>;
    pub fn depth(&self, side: Side, n: usize) -> Vec<(i64, i64)>; // tick, lot
}
```

`Level`: `total_lot`, `VecDeque<u64>` order ids (FIFO).

- [ ] **Step 1: Tests**

```rust
#[test]
fn bid_best_is_highest_tick() {
    let mut b = Book::new();
    b.insert_limit(HpOrder::limit(Side::Buy, 10000, 1, 1));
    b.insert_limit(HpOrder::limit(Side::Buy, 10100, 1, 2));
    assert_eq!(b.best_bid(), Some(10100));
}

#[test]
fn same_price_fifo_cancel_middle() {
    // insert two at same tick; cancel first; best still second
}
```

- [ ] **Step 2: Implement → PASS → Commit** `feat(match-core-hp): price-level book and order slots`

---

### Task 4: HpEngine limit match + cancel (TDD)

**Files:**
- Create: `crates/match-core-hp/src/engine.rs`
- Create: `crates/match-core-hp/tests/limit_match.rs`
- Modify: `lib.rs`

```rust
pub struct HpEngine { book: Book }
impl HpEngine {
    pub fn on_order(&mut self, cmd: HpCommand) -> &[HpEvent]; // or drain into caller buffer
}
pub enum HpCommand {
    Limit { side: Side, price_tick: i64, qty_lot: i64, ts: u64, client_id: u64 },
    Cancel { id: u64 },
    Market { side: Side, qty_lot: i64, ts: u64, max_levels: Option<u32>, client_id: u64 },
}
```

Hot path: write fills into a reusable `Vec<HpEvent>` with `clear()` + capacity reserve (preallocate at construction, e.g. 64).

- [ ] **Step 1: Tests** (mirror clean semantics, not Java quirks)

```rust
#[test]
fn limit_buy_fills_resting_sell() {
    let mut e = HpEngine::new();
    e.on_order(HpCommand::Limit { side: Side::Sell, price_tick: 100, qty_lot: 5, ts: 1, client_id: 1 });
    let ev = e.on_order(HpCommand::Limit { side: Side::Buy, price_tick: 100, qty_lot: 5, ts: 2, client_id: 2 });
    assert!(matches!(ev[0], HpEvent::Fill { qty_lot: 5, price_tick: 100, .. }));
    assert!(e.book.best_ask().is_none());
}

#[test]
fn price_time_older_maker_first() { /* two sells same tick; buy 1 lot hits older */ }

#[test]
fn partial_fill_leaves_remainder() { /* buy 3 vs sell 1 → 2 left on bid */ }

#[test]
fn cancel_removes_resting() { … }
```

Match price = **maker** tick (resting order price).

- [ ] **Step 2: Implement limit/cancel only (market → Task 5)**

- [ ] **Step 3: Commit** `feat(match-core-hp): HpEngine limit match and cancel`

---

### Task 5: Market + depth + adapter (TDD)

**Files:**
- Modify: `engine.rs` (market)
- Create: `crates/match-core-hp/src/adapter.rs`
- Create: `crates/match-core-hp/tests/market_depth.rs`
- Create: `crates/match-core-hp/tests/adapter_bborder.rs`
- Add dep: `match-protocol`, `bigdecimal` (adapter only)

- [ ] **Step 1: Market tests** — market buy walks asks until qty done or book empty; optional `max_levels`.

- [ ] **Step 2: Depth test** — two bids same tick aggregate lots.

- [ ] **Step 3: Adapter** — `fn from_bb_order(o: &match_protocol::BbOrder, scale: &SymbolScale) -> Result<HpCommand>` for Limit/Cancel; used by bench later.

- [ ] **Step 4: Commit** `feat(match-core-hp): market, depth, and protocol adapter`

---

### Task 6: `match-bench` criterion comparison

**Files:**
- Create: `crates/match-bench/Cargo.toml` (`[[bench]]` harness false)
- Create: `crates/match-bench/benches/engine_cmp.rs`
- Create: `crates/match-bench/src/workload.rs` — generate N commands for five scenarios
- Modify: workspace members
- Create: `docs/bench-results.md` (fill after first run)

- [ ] **Step 1: Workload helpers** producing parallel inputs:
  - For hp: `Vec<HpCommand>`
  - For core: `Vec<match_core::BbOrder>` via `BbOrder::test_limit` / market helpers

Scenarios: `rest_only`, `cross_full`, `partial_walk`, `cancel_hot`, `mixed` (each ≥ 10_000 orders).

- [ ] **Step 2: Criterion benches**

```rust
fn bench_cross_full(c: &mut Criterion) {
    let (core_orders, hp_cmds) = workload::cross_full(50_000);
    c.bench_function("core_cross_full", |b| {
        b.iter(|| {
            let mut eng = Engine::new();
            for o in &core_orders { eng.on_order(o.clone()); }
        })
    });
    c.bench_function("hp_cross_full", |b| {
        b.iter(|| {
            let mut eng = HpEngine::new();
            for c in &hp_cmds { eng.on_order(*c); /* or ref */ }
        })
    });
}
```

Prefer `HpCommand: Copy` where possible to avoid clone noise favoring hp unfairly; for core, cloning `BbOrder` is part of its real cost — document that.

- [ ] **Step 3: Run**

```bash
cargo bench -p match-bench --bench engine_cmp -- --sample-size 20
```

Paste summary into `docs/bench-results.md` with machine info + ratio. If ratio &lt; 5× on `cross_full`/`partial_walk`, profile (reduce Level `VecDeque` alloc, denser slots) in a follow-up commit before closing Task 6.

- [ ] **Step 4: Commit** `feat(match-bench): compare match-core vs match-core-hp`

---

### Task 7: SPSC worker + preallocated event buffer (H3)

**Files:**
- Create: `crates/match-core-hp/src/spsc.rs` (or use `rtrb` dependency)
- Create: `crates/match-core-hp/src/worker.rs`
- Create: `crates/match-core-hp/tests/spsc_worker.rs`

- [ ] **Step 1: API**

```rust
pub struct HpWorker { /* ring + engine + event_buf */ }
impl HpWorker {
    pub fn try_submit(&self, cmd: HpCommand) -> Result<(), Busy>;
    pub fn run_once(&mut self) -> usize; // process available cmds, return fill count
}
```

Single-threaded test: submit N → `run_once` loop → assert fills.

- [ ] **Step 2: Preallocate** `event_buf: Vec<HpEvent>` with capacity; `on_order` clears and reuses (refactor engine if needed).

- [ ] **Step 3: Optional bench path** `hp_cross_full_spsc` in match-bench.

- [ ] **Step 4: Commit** `feat(match-core-hp): SPSC worker and preallocated events`

---

### Task 8: Docs + guardrails

**Files:**
- Modify: `README.md` — hp section: dual-track warning, how to bench
- Modify: `docs/superpowers/specs/2026-07-18-match-core-hp-design.md` status if needed
- Grep: ensure `match-contract` Cargo.toml has **no** `match-core-hp` dependency

- [ ] **Step 1: README** note production default unchanged

- [ ] **Step 2: `cargo test --workspace` + `cargo bench -p match-bench` smoke**

- [ ] **Step 3: Commit** `docs: match-core-hp dual-track usage and bench results`

---

## Spec coverage

| Spec item | Task |
|-----------|------|
| Crate isolation | 1, 8 |
| Fixed-point | 2 |
| Price-level book | 3 |
| Limit/cancel | 4 |
| Market/depth/adapter | 5 |
| Bench ≥5× goal | 6 |
| SPSC + prealloc | 7 |
| Production default untouched | 8 |

---

## Self-review notes

- No production cutover tasks (H4 deferred).  
- Advanced orders deferred.  
- Types (`HpCommand`, `HpEngine`, `SymbolScale`) consistent across tasks.  
- 5× is a target with explicit “profile then iterate” escape in Task 6.
