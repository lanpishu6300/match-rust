# Spot Matching Shell Design (M5)

**中文：** [2026-07-27-spot-match-shell-design.zh-CN.md](./2026-07-27-spot-match-shell-design.zh-CN.md)

**Date:** 2026-07-27  
**Status:** Draft — pending review  
**Parent design:** [2026-07-17-rust-match-engines-design.md](./2026-07-17-rust-match-engines-design.md) §4.4 / M5  
**Baseline code:** Java spot matcher `bf-match` (docs historically `java-spot-match`)  
**Related:** [现货合约MQ-Topic对照.md](../../../docs/现货合约MQ-Topic对照.md), [现货撮合Topic拆分分片方案.md](../../../docs/现货撮合Topic拆分分片方案.md)

---

## 0. Decision Summary

| Item | Choice |
|------|--------|
| Goal | Full-service **spot** replacement: Rust owns MQ in/out, startup recovery, depth/fill push; Topic/JSON semantically compatible with `bf-match` |
| Core reuse | **Reuse `match-core`** for limit / market / cancel; **do not fork** a second book |
| Protocol | Add a **spot boundary** in `match-protocol` (validate / convert / constants / Topics); keep contract paths unchanged |
| Shell | Fill `match-spot` by adapting `match-contract` patterns; spot Topics, Redis keys, RPC, sharding |
| Default engine | Production default = `match-core` (Java-observable). Optional `hp-engine` later, feature-flagged only |
| Defect policy | Preserve `bf-match` observable behavior by default; bug fixes are separate tasks + golden updates |
| Cutover unit | **Shard / process instance** (`mainStream`), not per-symbol Topic ownership (unlike contract) |
| Out of scope (this design) | Live RocketMQ client (shared with contract; tracked separately), user/mm Topic split go-live (optional phase), fixing spot Java defects |

---

## 1. Background

### 1.1 Where we are

| Piece | State (2026-07-27) |
|-------|-------------------|
| `match-core` | Contract-shaped L1 equivalence for limit/market/cancel (+ PostOnly/IOC/FOK) |
| `match-protocol` | Contract `check_mq_order` / `type_convert` / `usdt_contract_*` constants |
| `match-contract` | Full shell: memory MQ, bootstrap, per-symbol workers, Redis, restore RPC, metrics |
| `match-spot` | Stub only (`//! Spot engine shell — filled in a separate plan (M5).`) |
| RocketMQ | Not wired (memory transport only) — shared blocker for any production cutover |

Parent design already ordered **contract first, spot second**, with shared core + dual shells. M5 was a one-paragraph placeholder; this document is the concrete spot design.

### 1.2 Why “contract-shaped protocol” blocks spot today

The book logic in `match-core` is price-time priority and can serve spot limit/market/cancel. What fails for spot is the **ingress boundary**:

- `type_convert` requires contract fields (`close_position`, `start_deposit`, `taker_rate`, `position_type`, `lever_times`) and rejects `trust_price ≤ 0`.
- `TYPES` / `ORDER_FORMS` follow contract (owners 1–8; forms include PostOnly/IOC/FOK).
- Topic helpers and Redis prefixes are `usdt_contract_*` / `contract_exchange_depth:`.
- Restore RPC paths are contract market/entrust APIs.

Spot Java (`bf-match`) uses historical Topic names under `contract_match_*` (spot-only namespace — not perpetual), softer validation, and shard-based consume.

### 1.3 Goals

1. Deliver a runnable `match-spot` process that can replace `bf-match` for a shard after L1–L3 gates.
2. Keep **one** `match-core`; isolate spot differences in protocol + shell.
3. Lock acceptance with spot-shaped golden / replay against `bf-match`, not contract traces.

### 1.4 Non-Goals

- Do not change production Topic names or downstream JSON semantics for spot.
- Do not make `match-core-hp` the spot cutover default.
- Do not merge spot and contract Topic namespaces.
- Do not implement user/mm split as a hard dependency of M5 (design for it; ship later if needed).
- Do not fix known spot Java defects in this milestone unless a separate task says so.

---

## 2. Architecture

### 2.1 Crate roles (unchanged dual-shell rule)

```text
match-protocol   — shared DTOs + contract_* and spot_* boundaries
match-core       — book + limit/market/cancel (+ contract advanced forms unused by spot ingress)
match-contract   — contract process (existing)
match-spot       — spot process (this milestone)
match-replay     — golden / dual-run (extend with spot fixtures)
match-core-hp    — optional experimental engine behind feature flag (post-cutover)
```

| Crate | Spot work | Forbidden |
|-------|-----------|-----------|
| `match-protocol` | Spot constants, `check_mq_order_spot`, `type_convert_spot`, depth batch sizes | Holding book state |
| `match-core` | Only if L1 vs `bf-match` shows behavioral drift | Spot Topic / Redis / RPC awareness |
| `match-spot` | Config, MQ, bootstrap, workers, outbound, Redis, restore, health | Forking match rules |
| `match-replay` | Spot golden packs + exporter notes from `bf-match` | Production outbound |

### 2.2 Process data flow (spot)

```text
RMQ shard Topics (contract_match_order | contract_match_order_main_coin[_N])
  → parse JSON → check_mq_order_spot → type_convert_spot
  → route by symbolKey → per-symbol queue → single worker
  → match-core::Engine (limit / market / cancel only from ingress)
  → fills / book updates / revoke
  → producers: push_order_{symbol}, market/depth Topics (spot naming)
  → Redis error / depth keys (exchange_depth:*)
```

Same **per-symbol single writer** rule as contract. Difference: **one process consumes a shard Topic that carries many symbols**, then demuxes into per-symbol queues (mirror `bf-match` `ORDER_QUEUE_MAP`).

### 2.3 Approach choice

| Approach | Pros | Cons |
|----------|------|------|
| A. Clone `match-contract` → rename Topics only | Fast start | Wrong sharding/RPC/validation; high rework |
| B. Shared shell crate + thin spot/contract bins | DRY long-term | Large refactor of working contract shell |
| **C. Adapt-by-copy modules into `match-spot` + protocol fork (recommended)** | Isolates risk; matches parent “dual shells”; keeps contract green | Some duplication until a later extract |

**Recommendation: C** for M5. Extract shared MQ traits later if duplication hurts.

---

## 3. Protocol alignment (spot)

### 3.1 Topic / group map

Align with [现货合约MQ-Topic对照.md](../../../docs/现货合约MQ-Topic对照.md). Names are historical; **they are spot-only**.

| Direction | Spot (bf-match) | Contract (today’s Rust) |
|-----------|-----------------|-------------------------|
| Inbound orders | `contract_match_order` (+ `contract_match_order_main_coin` …) | `usdt_contract_match_order_{symbol}` |
| Consumer group | `contract_match_group` (confirm exact Java string in impl) | `usdt_contract_match_channel_one_group{symbol}` |
| Push fills | `contract_match_order_push_order_{symbol}` | `usdt_contract_match_order_push_order_{symbol}` |
| Depth / no-deal / robot | Global Topics **without** symbol suffix (confirm against `bf-match` `BBConstants`) | Per-symbol `usdt_contract_match_market_push_*_{symbol}` |
| New market | `market_client_new_coin_market` | `usdt_market_add_new_coin` |

Implementation note: put spot names in `match-spot` `mq/topics.rs` **or** `match-protocol::spot_topics` — do not overload contract helpers.

### 3.2 Validation / convert

| Rule | Spot | Contract (current) |
|------|------|--------------------|
| Owner `type` | `{1,2,3}` (user / robot / fee — confirm in `bf-match`) | `{1..8}` |
| Order forms | Limit + market only | + PostOnly / IOC / FOK |
| Contract fields | Not required on ingress | Required in `type_convert` |
| Market `trust_price ≤ 0` | Allowed when form=market (Java overwrites buy price in engine) | Rejected in `type_convert` |

API sketch (names illustrative):

```rust
// match-protocol
pub fn check_mq_order_spot(mq: &MqOrder) -> bool { ... }
pub fn type_convert_spot(mq: &MqOrder) -> Option<BbOrder> { ... }
```

`BbOrder` may keep optional contract fields as defaults for unused paths; spot convert must not invent fake leverage/margin semantics that change fills. Prefer `Default` / zero / `None` only where `bf-match` `buildMqOrder` does the same.

### 3.3 Depth / batch constants

Spot Java uses different snapshot sizes than contract Rust constants (`NO_DEAL_NUMBER=20`, `SEND_MAX_DATA=10`). Introduce **spot-named constants** (e.g. `SPOT_NO_DEAL_NUMBER`, `SPOT_SEND_MAX_DATA`) sourced from `bf-match` / `BBConstants`, and use them only in `match-spot` outbound.

### 3.4 Redis

| Spot | Contract |
|------|----------|
| `exchange_depth:{symbol}{detail\|trade\|paint}` (confirm suffix set in Java) | `contract_exchange_depth:...` |

Error-queue / link-key ideas can mirror contract; **prefixes must stay spot**.

### 3.5 Restore RPC

Contract today:

- Markets: `/contract-market/contractcoinMarketList`
- Entrust: `/contract/entrust-list`

Spot must reverse-lookup `bf-match` `InitLoadData` Feign paths (order-center / market). Document exact URLs and request bodies in the implementation plan after code archaeology; do not reuse contract paths.

Bootstrap filters markets by **`mainStream` / shard id** configured for this process instance.

### 3.6 Startup dedupe

Contract shell carries `START_QUEUE` / `BigNo` / 720s window. Spot Java does **not** use that model by default. **Omit** unless archaeology proves otherwise. Document the decision in code comments next to bootstrap.

---

## 4. `match-spot` shell modules

Mirror `match-contract` layout; replace spot-specific pieces:

```text
crates/match-spot/
├── Cargo.toml          # bin + deps (protocol, core, …)
└── src/
    ├── main.rs
    ├── config.rs       # shard/mainStream, RPC bases, redis, transport
    ├── bootstrap.rs    # markets by shard → restore → workers → consumers
    ├── inbound.rs      # spot Topics → validate/convert_spot → queues
    ├── outbound.rs     # spot Topics + spot depth batching
    ├── symbol_worker.rs
    ├── redis_store.rs  # exchange_depth: prefix
    ├── rpc/            # spot market + entrust clients
    ├── mq/             # topics, traits, memory; later rocketmq
    ├── health.rs
    └── telemetry.rs
```

### 4.1 Config (illustrative)

```yaml
shard:
  main_stream: 0          # 0 = ordinary Topic; 1..N = main_coin shards
transport: memory         # memory | rocketmq (when ready)
rpc:
  market_base: ...
  order_base: ...
redis:
  # keys under exchange_depth:
```

### 4.2 Engine wiring

- Default: `match-core` only.
- Optional feature `hp-engine`: same dual-track rules as contract (`ARCHITECTURE.md`) — never default for cutover.

### 4.3 Shared MQ traits

Reuse the **pattern** of `match-contract` `mq/traits.rs` + memory adapter (copy or thin shared crate later). Live RocketMQ remains a **shared** milestone; spot cutover to production waits on it the same as contract.

---

## 5. Equivalence and acceptance

### 5.1 L1 — core behavior (`bf-match`)

Spot-shaped fixtures on `match-core` (via `type_convert_spot` or hand-built `BbOrder`):

- Limit cross + time priority
- Partial fill / remaining
- Cancel
- Market buy/sell including **zero inbound price** path
- Explicitly **no** PostOnly/IOC/FOK as spot acceptance (ingress must reject or never emit)

### 5.2 L2 — golden / replay

- Export NDJSON (or existing replay format) from `bf-match` tests / recordings
- Extend `match-replay` with a spot profile (protocol convert + compare fills / remaining / depth)
- Gate: zero unexplained diffs on the golden pack

### 5.3 L3 — shell on test env

- Memory or real MQ: restore → inbound → outbound → Redis depth
- Dual-run / shadow vs Java for one shard before canary

### 5.4 Cutover sketch

1. Warm-up: Rust up, no production consume; L2 green; restore drill on test env.
2. Per-**shard** cut: stop Java consume for that shard Topic → drain or brief dual-stop → Rust restore + subscribe → watch fills/depth/error queue.
3. Expand shards; keep Java package for rollback.
4. Rollback triggers: outbound error rate, blank depth, order-center reconciliation mismatch.

Deploy knobs (names illustrative): `match.engine.impl=java|rust`, `match.engine.main_stream` / shard whitelist.

---

## 6. Work breakdown

| Phase | Deliverable | Depends on |
|-------|-------------|------------|
| M5.0 | This design approved; archaeology note for spot RPC URLs + exact Topic/group strings from `bf-match` | — |
| M5.1 | `match-protocol` spot boundary + unit tests | M5.0 |
| M5.2 | Spot L1 tests vs `bf-match` behavior | M5.1 |
| M5.3 | `match-spot` binary: config, health, memory MQ, workers, spot Topics/outbound/Redis | M5.1 |
| M5.4 | Bootstrap + spot restore RPC | M5.3 + archaeology |
| M5.5 | Spot L2 golden + `match-replay` profile | M5.2 |
| M5.6 | Spot cutover runbook (`docs/cutover-runbook-spot.md`) | M5.4–M5.5 |
| M5.7 | Live RocketMQ (shared with contract) | Separate track |
| M5.8 (optional) | user/mm Topic consumers → same per-symbol queue | [Topic split plan](../../../docs/现货撮合Topic拆分分片方案.md) |
| M5.9 (optional) | `hp-engine` on spot shell | After L2 green |

---

## 7. Risks and open items

| Risk | Mitigation |
|------|------------|
| Historical `contract_match_*` confused with perpetual | Docs + code comments label “spot-only”; separate modules |
| Spot depth Topic shape mis-copied from contract | Archaeology against `bf-match` `BBConstants` before outbound PR |
| `type_convert_spot` defaults change fills | Golden compare; only default fields Java defaults |
| Market zero-price drift in `match-core` | Dedicated L1 cases; fix core only if spot Java differs |
| Dual consume on a shard splits the book | Ops checklist: single active consumer group instance |
| RocketMQ delayed | Ship memory path + L1/L2 first; no production claim until M5.7 |

**Open items (resolve in M5.0 archaeology):**

1. Exact consumer group string(s) and all depth/robot Topic names from `bf-match`.
2. Exact restore HTTP paths and pagination fields.
3. Spot `TYPES` / form enums vs fee/robot codes actually seen on the wire.
4. Whether any START_QUEUE-like dedupe exists in current spot main.

---

## 8. Rejected options

1. **Fork `match-core` into `match-core-spot`:** Parent design forbids duplicate cores; maintenance cost dominates.
2. **Reuse contract `check_mq_order` / `type_convert` with “optional fields” hacks:** Rejects real spot payloads or invents contract semantics.
3. **HP-first spot cutover:** Violates dual-track rule; spot acceptance is Java equivalence first.
4. **Per-symbol inbound Topics for spot:** Would change production naming; out of scope.

---

## 10. bf-match Java module map

Baseline: `bf-match/bztex-match-server-provider`. Full tables: [zh-CN §10–14](./2026-07-27-spot-match-shell-design.zh-CN.md).

**Topology:** spot uses **shard Topics** (`contract_match_order` + optional `_mm`) with **symbolKey demux**; contract uses **per-symbol Topics**. Spot depth Topics are **global**; Redis uses `exchange_depth:` not `contract_exchange_depth:`.

**Java → Rust:** `BaseConsumer` → `inbound`; `InitLoadData` → `bootstrap`; `BuyHandler`/`SellHandler` → **`match-core`**; producers → `outbound`; `BBConstants` check/convert → `match-protocol::spot_*`.

**Shell deltas vs match-contract:** remove StartQueue/BigNo; shard consumers not `subscriptions_for_symbols`; spot RPC (`coinMarkets`, `getEntrustList`).

---

## 11–14. Module architecture, layer changes, config, checklist

See [2026-07-27-spot-match-shell-design.zh-CN.md](./2026-07-27-spot-match-shell-design.zh-CN.md) §11–14 for:

- `match-spot` internal module tree and mermaid dataflow
- Per-layer transformation (`match-protocol`, `match-core`, shell, replay)
- Full `config.spot.example.yaml`
- 10-item acceptance checklist vs `bf-match` source lines

---

## 9. Doc / index updates (when approved)

- Link this spec from `docs/README.md` / `README.zh-CN.md`
- Point parent design §4.4 / M5 here
- Wiki Roadmap: M5 = spot shell (this design)
- New runbook: `docs/cutover-runbook-spot.md` (M5.6)
