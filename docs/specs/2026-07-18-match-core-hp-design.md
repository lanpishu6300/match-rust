# match-core-hp Extreme Low-Latency Dual-Track Design

**中文：** [2026-07-18-match-core-hp-design.zh-CN.md](./2026-07-18-match-core-hp-design.zh-CN.md)

**Date:** 2026-07-18  
**Status:** Implemented (H0–H3) — 2026-07-18; production default remains `match-core`  
**Prerequisite:** [2026-07-17-rust-match-engines-design.md](./2026-07-17-rust-match-engines-design.md) (equivalence track)  
**Code root:** this repository (`match-rust`)

---

## 0. Decision Summary

| Item | Choice |
|------|--------|
| Strategy | **Dual-track C**: keep `match-core` equivalence track; add `match-core-hp` for extreme optimization |
| Landing | **Independent crate** (not feature entanglement, not premature unified trait) |
| Production | `match-contract` **still defaults to `match-core`**; phase one does not cut over to hp |
| Industry alignment | LMAX single-writer + preallocation; mechanical sympathy; price-level book; `i64` fixed-point |
| Semantics | hp uses **clean** limit/market/cancel semantics; does **not** replicate Java quirks |

---

## 1. Background and Goals

The equivalence track `match-core` targets Java-observable equivalence (`BigDecimal`, `BTreeSet`, Handler control flow) and explicitly avoids performance-oriented rewrites. This design adds an experimental track that adopts common ideas from industry low-latency matching/messaging kernels, proves gains with measurable numbers, and does not break the production default path.

### 1.1 Goals

1. Deliver `match-core-hp`: fixed-point + price-level book + single-writer hot path.  
2. Deliver `match-bench`: throughput and latency comparison of core vs hp.  
3. Document “adopted ideas / non-goals / differences from the equivalence track.”

### 1.2 Non-Goals (Phase One)

- Do not replace the production default engine.  
- Do not connect to real RocketMQ / do not change production Topics.  
- Do not replicate known Java defect behavior for PostOnly/IOC/FOK.  
- Do not build full Aeron-grade IPC; in-process SPSC is enough.

---

## 2. Architecture and Crate Boundaries

```text
crates/
├── match-core/       # Equivalence track (frozen behavior, production default)
├── match-core-hp/    # New: high-performance match core
├── match-bench/      # New: criterion / comparison benchmarks
├── match-protocol/   # Shared DTOs; used only at adapter boundaries
└── match-contract/   # Defaults to depending on match-core
```

| Crate | Responsibility | Forbidden |
|-------|----------------|-----------|
| `match-core-hp` | Fixed-point orders, price-level book, preallocated events, optional SPSC worker | Tokio/MQ/JSON; Java golden |
| `match-bench` | Same-sequence load test core vs hp | Production traffic |
| hp `adapter` module | `BbOrder` ↔ tick/lot; optional `HpEvent` → decimal display | `BigDecimal`/string math on the hot path |

### 2.1 Industry Idea Mapping

| Idea | Source | Landing |
|------|--------|---------|
| Single Writer | LMAX Disruptor | One thread per symbol owns the book |
| Preallocate | Disruptor / Aeron | Order slots, Cmd/Event rings |
| Mechanical sympathy | Aeron etc. | Price-level structures, few locks/allocations |
| Price-level book | Exchange match cores | Levels + same-price FIFO |
| Fixed-point | Low-latency trading systems | `i64` price_tick / qty_lot |

### 2.2 Data Flow

```text
Production:  MQ → match-core (unchanged)

Experiment:  logical order sequence
               ├─→ match-core::Engine
               └─→ adapter → match-core-hp::HpEngine
             match-bench aggregates throughput / p50 / p99
```

---

## 3. Data Model and Price-Level Book

### 3.1 Fixed-Point

Per symbol, fixed:

- `price_scale`: `tick = round(price * 10^price_scale)`  
- `qty_scale`: `lot = round(qty * 10^qty_scale)`  

Hot-path structures (logical shape):

- `HpOrder`: `id`, `side`, `price_tick`, `qty_lot`, `open_lot`, `ts` — no `String` / `BigDecimal`  
- `HpEvent`: `Fill` / `Revoke` (reason as `u8`)

Adapter does scale conversion and overflow checks only at the boundary.

### 3.2 Price-Level Book

```text
bids / asks: Level map indexed by tick (best price O(1)/O(log levels))
Level: total_lot + same-price FIFO (order-id linked list or slot index)
orders: generational / preallocated slot array (id → HpOrder)
```

| Operation | Target complexity |
|-----------|-------------------|
| Best price | O(1) or O(log levels) |
| Same-price enqueue/dequeue | O(1) |
| Cancel | O(1) locate + remove empty level |
| Take / walk levels | Along opposite best until no cross or qty exhausted |

### 3.3 Single Writer and Queues

- Per symbol: `HpWorker` exclusively owns the book.  
- Inbound: preallocated SPSC ring (length 2^n); full → `Busy` backpressure.  
- Outbound: fixed-cap event buffer, batch drain.  
- Hot path does not use `Mutex` / `tokio::mpsc`.  
- Bench may call `HpEngine::on_order` on the same thread to measure pure match core.

### 3.4 Phase-One Features

| Capability | Phase one |
|------------|-----------|
| Limit rest/take, partial fill, price-time priority | ✅ |
| Cancel | ✅ |
| Market (exhaust or optional max_fills / gear) | ✅ |
| Depth top N levels | ✅ |
| PostOnly / IOC / FOK | Phase two |
| Java quirk | ❌ |

Limit cross (clean semantics): buy can take when `ask_tick <= bid_tick`; market by default does not force the Java `gear=0` quirk.

---

## 4. Benchmarks, Acceptance, and Risks

### 4.1 Scenarios

`rest_only` / `cross_full` / `partial_walk` / `cancel_hot` / `mixed`

### 4.2 Metrics

- Throughput: orders/sec, fills/sec  
- Latency: p50 / p99 / p999 of a single `on_order` (excluding MQ)  
- Optional: hot-path heap allocation sampling  

Results go to `docs/bench-results.md` (or CI artifact).

### 4.3 Acceptance

| Item | Standard |
|------|----------|
| Correctness | hp unit tests cover price-time priority, partial fill, cancel, depth |
| Regression | Equivalence-track existing tests all green |
| Performance | On `cross_full` + `partial_walk`, hp relative to core **≥ 5×** throughput (local machine; if not met, record bottleneck and iterate) |
| Isolation | contract does not depend on hp by default |
| Docs | This spec + bench notes + differences vs equivalence track table |

### 4.4 Risks

| Risk | Mitigation |
|------|------------|
| Semantic drift | Shared logical scenario vectors; differences table lists uncovered items |
| Fixed-point overflow | Adapter validation + boundary unit tests |
| Ring full | Explicit Busy; bench statistics |
| Accidental production cutover | Code review + no default switch |

### 4.5 Milestones

| Phase | Deliverable |
|-------|-------------|
| H0 | hp skeleton + fixed-point + price-level limit/cancel + unit tests |
| H1 | Market + depth + adapter |
| H2 | match-bench five scenarios + first comparison report |
| H3 | SPSC worker + preallocated event buffer |
| H4 | Advanced order types; contract explicit `engine=hp` experimental switch |

---

## 5. Differences vs Equivalence Track (Contract)

| | match-core | match-core-hp |
|--|------------|---------------|
| Numbers | `BigDecimal` | `i64` tick/lot |
| Book | `BTreeSet` whole-order sort | Price levels + FIFO |
| Semantics | Java-equivalent (including quirks) | Clean matching semantics |
| I/O | Wired to MQ by contract | None; bench / future optional shell |
| Production default | ✅ | ❌ |

---

## 6. Rejected

1. Mixing equivalence/performance via features inside `match-core` — regression and cognitive load too high.  
2. Premature unified `MatchingEngine` trait bound to decimal APIs — would cripple the hp hot path.  
3. Making production RMQ connectivity a phase-one gate — conflicts with “prove match-core numbers first” priority.

---

## 7. OSS Best-Practices Index (Post-Landing)

Full mapping is in-repo at [`docs/best-practices.md`](../best-practices.md) (Disruptor / Aeron / Seastar / match-core common knowledge → module paths and how to enable).
