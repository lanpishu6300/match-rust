# Open-source low-latency practices → match-rust map

**中文：** [best-practices.zh-CN.md](./best-practices.zh-CN.md)

This note maps well-known exchange / messaging / storage design ideas onto this repo.  
**Production default remains the equivalence track `match-core`; performance practices concentrate in `match-core-hp`.**

---

## 1. Practice table

| Idea | Exemplar | Landing here | Status |
|------|----------|--------------|--------|
| Single Writer Principle | [LMAX Disruptor](https://lmax-exchange.github.io/disruptor/) | One `HpWorker` / `HpEngine` per symbol owns the book | ✅ |
| Preallocated ring | Disruptor / [Aeron](https://github.com/real-logic/aeron) | `SpscRing` power-of-two slots; reserved `HpEngine` event `Vec` | ✅ |
| Batch consume | Disruptor `BatchEventProcessor` | `SpscRing::pop_n` one Acquire/Release for a batch | ✅ |
| Avoid false sharing | Aeron / Disruptor | `head`/`tail` on separate cache lines (`CachePadded`) | ✅ |
| Mechanical sympathy | Martin Thompson / Aeron | Fixed-point `i64`, level book, no lock/JSON on hot path | ✅ |
| Share-nothing shards | [Seastar](https://github.com/scylladb/seastar) | Shard by symbol; no shared mutable state across shards | ✅ (arch) |
| Fixed-point arithmetic | Exchange / HFT kernels | `SymbolScale` + tick/lot | ✅ |
| Price levels + FIFO | Exchange common sense | `Book` + `Level` + `VecDeque<id>` | ✅ |
| Backpressure, not drop/grow | Disruptor / reactive streams | `try_submit` → `Busy` | ✅ |
| Configurable wait strategy | Aeron `IdleStrategy` | `WaitStrategy::{BusySpin,Yield}` + `poll` | ✅ |
| Observe vs correctness split | Engineering common sense | `match-core` golden; `match-bench` for hp | ✅ |
| CPU pinning | DPDK / Aeron ops | `affinity` module + ops notes (optional `core_affinity`) | ✅ docs/API |
| Best-price cache + level pool | perpetual_exchange / kernels | `Book` best_* + level_pool | ✅ |
| ART / byte-radix level index | perpetual_exchange ART | `--features art` (`ArtAskIndex`/`ArtBidIndex`) | ✅ optional |
| Async batched persistence | perpetual_exchange persistence | `match-wal` async buffer + flush | ✅ experiment |
| Zero-copy IPC | Aeron Media Driver | Not done (in-process SPSC enough; cross-process later) | ⏳ |
| Kernel-bypass NIC | DPDK / io_uring | Ops layer, out of crate scope | ⏳ |
| Business quirk replica | — | Intentionally not (clean semantics track) | N/A |

---

## 2. Project highlights (condensed)

### LMAX Disruptor
- **Single writer** mutates shared structure; readers coordinate via sequences.
- **Preallocate** the whole ring; no `new` for events at runtime.
- **Batch** to cut barriers and branch frequency.

→ Maps to: `HpWorker` + `SpscRing` + `pop_n` + reserved `events`.

### Aeron
- **Mechanical sympathy:** cache-aligned structures, less false sharing, controlled spinning.
- **IdleStrategy:** BusySpin / Yield / Sleeping trade latency vs CPU.

→ Maps to: `CachePadded` cursors, `WaitStrategy`, `HpWorker::poll`.

### Seastar / Scylla
- **One shard per core**, no cross-core locks; explicit queues for messages.

→ Maps to: per-symbol engines; no `Mutex` on the book hot path.

### Exchange matching common sense
- **Price level + FIFO**, not a market-wide `TreeSet` of whole orders.
- **Integer tick/lot** on the hot path — no arbitrary-precision decimal.

→ Maps to: `match-core-hp` `Book` / `scale`; equivalence `match-core` keeps `BTreeSet`+`BigDecimal`.

---

## 3. Code index

| Module | Path |
|--------|------|
| SPSC + false-sharing isolation + batch pop | `crates/match-core-hp/src/spsc.rs` |
| Worker + wait strategy | `crates/match-core-hp/src/worker.rs` |
| Level book | `crates/match-core-hp/src/book.rs` |
| Fixed-point | `crates/match-core-hp/src/scale.rs` |
| Affinity notes | `crates/match-core-hp/src/affinity.rs` |
| Equivalence track (control) | `crates/match-core/` |
| Benches | `crates/match-bench/`, `docs/bench-results.md` |

---

## 4. Usage tips (performance track)

1. Bench / latency-sensitive: `HpEngine` or `HpWorker` + `WaitStrategy::BusySpin` (dedicated core).
2. Shared CPU with other work: `WaitStrategy::Yield`.
3. Before production cutover: L3/shadow ([`l3-shadow.md`](./l3-shadow.md)); default path stays `match-core`.
4. Pinning: after start, call `affinity::pin_current_thread(core_id)` on the worker (feature `affinity`).

---

## 5. Deliberate anti-patterns

| Anti-pattern | Why |
|--------------|-----|
| Hot-path `BigDecimal` / JSON | Alloc + parse dominate latency |
| Multi-thread writers on one book | Locks + false sharing |
| Auto-grow a full ring | Hides latency spikes |
| Sacrifice clean hot path for Java quirks | Dual-track: `match-core` owns equivalence |

## client 语义（2026-09-21 修复）：支持每 client 多在途订单

**原设计缺陷**：`client_to_id: FxHashMap<u64, u64>`（client → 唯一在途订单 slot），
`on_limit`/`on_market` 开头 `contains_key` 即**静默丢弃**新单。真实撮合中一个 client
（尤其做市/高频）同 symbol 多档挂单、多 symbol 各挂多单是常态——单在途语义会拦掉
正常下单；静默丢弃无错误返回，生产上等于订单静默丢失。

**修复后语义**：
1. `client_to_id: FxHashMap<u64, Vec<u64>>`（client → 在途订单 id 列表），
   rest 时 `entry(client_id).or_default().push(id)`；fully-fill / 被 taker 吃光 / cancel 时从列表移除。
2. 去掉 `on_limit`/`on_market` 的单在途拦截——同 client 可在途多单（含自成交路径）。
3. `Cancel`：主路径 = 显式订单 id（store slot，优先解析）；便利路径 = client_id → 撤该 client 全部在途单。

**测试**：新增 `same_client_multi_resting_orders_allowed` / `cancel_by_order_id_removes_only_that_order` /
`maker_fully_filled_clears_client_entry`；改写原 `duplicate_*_client_id_is_rejected` 两个集成测试为
多在途语义。性能回归：hp_engine_bench 4sh 12.8M/s（云 VM 2 核），无回退。

**已知边界**：订单 id = store slot（1-based），slot 会被复用（free list）——外部持有旧 slot
长时间后再 Cancel 可能撤到新单。生产若用外部订单号引用，应在网关层维护 外部号 → slot 映射，
或给 HpOrder 增加独立单调 id 字段。
