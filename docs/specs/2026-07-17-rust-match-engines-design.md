# Rust Contract/Spot Matching Engine Equivalence Redesign

**中文：** [2026-07-17-rust-match-engines-design.zh-CN.md](./2026-07-17-rust-match-engines-design.zh-CN.md)

**Date:** 2026-07-17  
**Status:** Approved — 2026-07-17  
**Baseline code:** `java-contract-match` / `java-spot-match` (Java main)  
**Related docs:** [合约撮合已知问题梳理.md](../../合约撮合已知问题梳理.md) (Chinese), [现货撮合Topic拆分分片方案.md](../../现货撮合Topic拆分分片方案.md) (Chinese)

---

## 0. Decision Summary

| Item | Choice |
|------|--------|
| Delivery form | **Full service replacement**: Rust process owns MQ in/out, startup recovery, depth/fill push; Topic/JSON **semantically compatible**, can replace Java via canary |
| Implementation order | **Contract first** `java-contract-match`, then reuse core for spot `java-spot-match` |
| Equivalence acceptance | **Observable results strictly identical** (fill price/qty, remaining size, cancel paths, depth levels); JSON encoding detail differences allowed |
| Surrounding capabilities | **Fully align with Java**: recovery RPC, Redis, depth/robot Topics, error resend, health checks/metrics |
| Architecture | **Shared match core + dual shells** (`match-core` + `match-contract` / `match-spot` + `match-replay`) |
| Defect policy | Default **preserve as-is** Java main behavior (including known defects); bug fixes require separate tasks |

---

## 1. Background and Goals

### 1.1 Current State

Production matching consists of two closely related Java/Spring Boot + RocketMQ services:

- **Spot:** `java-spot-match` — in-memory `TreeSet` order book, single-threaded matching per symbol, price-time priority; Topic naming includes historical `contract_match_*` prefixes and mainstream sharding.
- **Contract:** `java-contract-match` — isomorphic, additionally supports PostOnly / IOC / FOK; Topics are `usdt_contract_match_*`, subscribed per symbol; startup recovers open orders via RPC.

In-repo `crypto-exchange` (C++) and `clearing-match` are experimental/R&D paths and are **not** the production replacement target. There is currently no Rust matching code.

### 1.2 Goals

1. Implement a Rust contract matching engine process that is **fully logically equivalent** to production, and can replace `java-contract-match` per symbol via canary.
2. Extract a reusable `match-core`; phase two delivers full-service replacement for spot `match-spot`.
3. Lock acceptance criteria with `match-replay` (golden / dual-run), avoiding false equivalence that only “looks right.”

### 1.3 Non-Goals (Phase One)

- Do not change Topic naming or downstream consumer contract semantics.
- Do not do performance-oriented data-structure rewrites (internal implementation may change; external results must pass replay).
- Do not fix P0/P1 items in [合约撮合已知问题梳理](../../合约撮合已知问题梳理.md) in this phase (unless a separate task is opened).
- Do not cut over spot production traffic in this phase.
- Do not introduce a JNI intermediate state as the final form (option 3 was evaluated and rejected).

---

## 2. Architecture and Crate Boundaries

Repo path: this repository root (Cargo workspace).

```text
match-rust/
├── Cargo.toml
└── crates/
    ├── match-core/         # Pure logic: order book + matching state machine
    ├── match-protocol/     # MqOrder/BBOrder fields, validation, decimal parsing
    ├── match-contract/     # Contract executable process (bin)
    ├── match-spot/         # Spot bin (phase-one stub, filled in phase two)
    └── match-replay/       # Record/replay / Java↔Rust diff
```

| Crate | Responsibility | Forbidden |
|-------|----------------|-----------|
| `match-core` | Price-time priority book, limit/market/cancel, PostOnly/IOC/FOK, emit fills and book-change events | Awareness of Topic / Redis / RPC |
| `match-protocol` | Production-aligned DTOs, `checkMqOrder`, decimal string parsing | Holding match state |
| `match-contract` | RocketMQ, per-symbol single-thread queues, fetch markets, recover open orders, Redis, outbound Producers, metrics/health | Privately changing match rules |
| `match-spot` | Spot Topic/sharding and recovery (phase two) | Forking a duplicate core |
| `match-replay` | Run core on the same inputs; compare fills / remaining / depth | Connecting to production outbound traffic |

### 2.1 In-Process Data Flow

```text
RMQ usdt_contract_match_order_{symbol}
  → parse/validate (match-protocol)
  → per-symbol queue → single worker
  → match-core::Engine
  → fills / book updates / revoke
  → producers: push_order / push_market / no_deal / deeps / robot
  → Redis error queue on send fail
```

### 2.2 Decimals and Ordering

- Amounts/quantities use a decimal type with **semantic equivalence** to Java `BigDecimal`; inbound/outbound are primarily strings.
- Order-book ordering aligns with `BBOrder.compareTo`: bids high→low, asks low→high; same price by `createTime` then `trustOrderNo`.
- Comparisons and golden use numeric semantics + agreed scale; IEEE float is forbidden.

---

## 3. Data Flow and Protocol Alignment (Contract Phase One)

### 3.1 RocketMQ

| Direction | Topic / Group | Notes |
|-----------|---------------|-------|
| Inbound | `usdt_contract_match_order_{symbol}` | symbol lowercase, no `/`; ORDERLY + CLUSTERING |
| Group | `usdt_contract_match_channel_one_group` | Same group; **only one active engine per symbol at a time** (Java or Rust) |
| New markets | `usdt_market_add_new_coin` / `usdt_market_add_new_coin_group` | Dynamically create queues, threads, subscriptions |
| → Orders | `usdt_contract_match_order_push_order_{encodedSymbol}` | Batch size aligns with `SEND_MAX_DATA=10` |
| → Market fills | `usdt_contract_match_market_push_order_{encodedSymbol}` | |
| Book (no-deal) | `usdt_contract_match_market_push_no_deal_*` | |
| Depth | `usdt_contract_match_market_push_deeps_*` | Depth levels: 20 |
| Robot | `usdt_contract_match_market_push_robot` | No symbol suffix |

- Inbound body: JSON **array** `List<MqOrder>`.
- Outbound: field semantics align with existing Producers; encoding details may differ (see §4).
- `encodedSymbol` behavior aligns with `CoinMarketEncode.encodeSymbolKey`.

### 3.2 Inbound Model and Validation

Align `MqOrder` / `BBOrder` fields, including at least:

`userId`, `uid`, `cType`, `dealType`, `type`, `orderType`, `marketId`, `coinId`, `symbolKey`, `coinMarket`, `trustOrderNo`, `closePosition`, `startDeposit`, `positionType`, `takerRate`, `orderStatus`, `orderForm`, `gear`, `leverTimes`, `trustNumber`, `trustPrice`, `createTime`, `faceValue`, `handicapType`

- Validation aligns with `BBConstants.checkMqOrder` (including market `gear` and required contract fields).
- `orderForm`: `1` limit, `2` market, `3` PostOnly, `4` IOC, `5` FOK.
- Accepted inbound `orderStatus`: `0/2/3` (same as Java `ORDER_STATUS` list).

### 3.3 Startup and Recovery

Order aligns with `InitLoadData`:

1. Delayed start (production ~10s).
2. RPC/HTTP: `getAllContractCoinMarket`, filter `mainStream == SHARD(0)`.
3. Per symbol: delete Redis depth-related keys and `redis_poc_link_list_key{symbol}`; create `ORDER_QUEUE`; start single-thread `take → onEvent`.
4. Paginated `getContractEntrustList` recover open orders → `MqOrder` → enqueue / into book.
5. Maintain `START_QUEUE_MAP` + `BigNo`: during the startup window, drop recovered order numbers and duplicate MQ with `≤ BigNo`; clear after ~720s (same as Java).
6. Register per-symbol consumers, then pull traffic.

Rust calls the same REST paths exposed by existing Feign clients over HTTP (reverse-looked up from `contract-order` / `contractmarket` RPC), with no JVM dependency.

### 3.4 Redis

| Key / Purpose | Behavior |
|---------------|----------|
| `MATCH_KEY` + `redis_poc_link_list_key{symbol}` | Startup placeholder; if present, do not start a duplicate match thread |
| `MARKET_KEY` + `contract_exchange_depth:{origin}{detail\|trade\|paint}` | Deleted at startup |
| `poc_redis_send_mq_error_data_queue` | Enqueue on send failure; align with `SendErrorData` resend |

### 3.5 Handler Routing (Contract)

```text
validate → typeConvert → (START_QUEUE / BigNo dedupe) → ORDER_QUEUE[symbol]
  → EventOrderHandler
       ├─ form 1/2 → Buy/Sell (+ Market / RatherThan / Equals / LessThan)
       └─ form 3/4/5 → Height* (+ Fok*)
  → producers
```

One thread per symbol; symbols run in parallel. Exception and ACK behavior default to Java alignment (including known “ACK even on exception,” etc.); phase one does not “fix along the way.”

### 3.6 Configuration and Observability

Map existing config: RocketMQ NameServer, Redis, order/market RPC base URLs, `SHARD`, thread-pool size, depth levels, book throttling (align where main already has it). Metric names should align with `ContractMatchTelemetry` / `docs/opentelemetry-metrics.md` where practical, to reuse dashboards.

---

## 4. Equivalence Testing and Canary

### 4.1 Equivalence Definition (Acceptance B)

Under the same input sequence, the following must match:

| Dimension | Content |
|-----------|---------|
| Fills | taker/maker order nos, price, qty, remaining on both sides, order |
| Status | Partial fill / full fill / cancel success; cancel trigger paths same origin as Java |
| Order book | Bid/ask price levels and resting sizes after events (same-price time order preserved) |
| Depth / book | Level count and price/qty per level; when throttled, compare post-throttle snapshots |

**Not required:** log wording, metric timestamps, MQ msgId, JSON key order, decimal text trailing zeros / scientific notation.

Golden expected values come from **Java reference runs**, not from ideally correct matching.

### 4.2 Three Layers of `match-replay`

- **L1 CI:** Hand-written limit cross, same-price time priority, market+gear, PostOnly, IOC, FOK, cancel, empty book → assert `match-core` event stream.
- **L2 CI gate:** Recorded/synthetic `MqOrder` sequences → Java exports `GoldenTrace` (NDJSON) → Rust replay diff; fail on fill/remaining/depth mismatch.
- **L3 pre-prod:** Offline dual-run of low-traffic symbol inbound recordings, or Rust read-only shadow consume (**no produce**) comparing book state.

### 4.3 Canary Cutover

Constraint: same inbound Topic + same consumer group, **only one active engine at a time** may consume that symbol.

1. **Warm-up:** Deploy Rust without production subscribe; L2 all green; test env exercises recovery/Redis/outbound.
2. **Per-symbol cut:** Stop Java consume for that symbol → drain queue or brief dual-stop → Rust equivalent recovery and subscribe → observe fills/depth/error queue.
3. **Expand to full**; keep Java package for fast rollback.
4. **Rollback triggers:** outbound error rate, prolonged blank depth, reconciliation mismatch with orders, book-remove-failure style metrics significantly worse than baseline.

Deploy suggestion: `match.engine.impl=java|rust`, `match.engine.symbols` whitelist. Cutover window must preserve `START_QUEUE_MAP` / `BigNo` / 720s logic.

### 4.4 Spot Phase Two

`match-spot` reuses `match-core`; protocol/Topics align with `java-spot-match` (including historical naming and sharding). Acceptance L1–L3 and cutover strategy mirror contract. Phase one does not cut spot production.

---

## 5. Milestones

| Phase | Deliverable |
|-------|-------------|
| M0 | workspace + `match-protocol` + `match-core` skeleton + L1 framework |
| M1 | Contract limit/market/cancel equivalence + L2 golden baseline |
| M2 | PostOnly/IOC/FOK + depth/book push logic |
| M3 | `match-contract` full shell: MQ / Redis / recovery RPC / metrics |
| M4 | L3 dual-run + single-symbol canary + rollback drill |
| M5 | Spot `match-spot` (phase two) |

---

## 6. Risks and Open Items

| Risk | Mitigation |
|------|------------|
| Known Java defects carried over as-is | Document clearly; fix defects in separate tasks with golden updates |
| RocketMQ Rust client vs framework wrapper differences | Nail consume mode/retry/ACK with integration tests; compare against `BaseConsumer` |
| Recovery RPC path/auth differs from Feign | Reverse-look up URLs and request bodies from rpc modules; test-env startup recovery cases |
| Dual consume on cutover splits the book | Ops checklist enforces single active; automate check of group online instances |
| `BigDecimal` edge cases (scale, division) | L2 covers market split, FOK rollback, and other hot paths |

**Open items:** Closed in the implementation plan [`docs/plans/2026-07-17-rust-match-engines.md`](../plans/2026-07-17-rust-match-engines.md) (`bigdecimal` / `redis` cluster / Apache RocketMQ Rust + ping gate; Golden exporter in `java-contract-match` JUnit; recovery paths: `/contract-market/contractcoinMarketList`, `/contract/entrust-list`).

---

## 7. Rejected Options

1. **Pure mirror file-by-file port (no shared core):** Spot phase two easily forks; high maintenance cost.
2. **Long-term JNI kernel + Java shell coexistence:** Stacked redundancy against full-service replacement; dual shells still needed eventually.
3. **Use C++ `crypto-exchange` or clearing-match as production baseline:** Misaligned with production protocol/behavior; higher equivalence cost.
