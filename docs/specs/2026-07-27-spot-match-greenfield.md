# Spot Matching Rust Migration (Greenfield 0→1)

**中文：** [2026-07-27-spot-match-greenfield.zh-CN.md](./2026-07-27-spot-match-greenfield.zh-CN.md)

**Date:** 2026-07-27  
**Status:** Draft — pending review  
**Scope:** **Spot only** — full-service replacement of Java `bf-match`  
**Baseline:** `bf-match` (Java/Spring Boot + RocketMQ) — **sole** behavioral reference  
**Implementation plan:** [../plans/2026-07-27-spot-match-greenfield.md](../plans/2026-07-27-spot-match-greenfield.md)

**Related:** [现货合约MQ-Topic对照.md](../../../docs/现货合约MQ-Topic对照.md), [2026-07-27-spot-match-shell-design.md](./2026-07-27-spot-match-shell-design.md) (shell detail appendix)

---

## 0. Document scope

Greenfield plan assuming:

| Assumption | Meaning |
|------------|---------|
| **No existing Rust match code** | Do not inherit `match-rust` implementations; crate names may reuse, code is new |
| **No C++ baseline** | `crypto-exchange`, clearing-match are out of scope |
| **No contract Rust dependency** | `bf_contract_match` stays Java |
| **Acceptance** | Observable equivalence to `bf-match` via golden replay |

Differs from [2026-07-17-rust-match-engines-design.md](./2026-07-17-rust-match-engines-design.md) (“contract first, then spot”): **this doc is spot-only** and can be staffed independently.

---

## 1. Decision summary

| Item | Choice |
|------|--------|
| Delivery | Full process replacement: `match-spot` owns MQ, restore, depth/fill push |
| Protocol | Topic/JSON compatible with `bf-match`; namespace `contract_match_*` (**spot only**) |
| Core | New `match-core`: limit / market / cancel only |
| Shell | New `match-spot`: shard consume, symbol demux, global depth topics, spot Redis/RPC |
| Acceptance | L1 unit + L2 golden (Java export) + L3 shadow/grey |
| Defects | Preserve `bf-match` observable behavior by default |
| Cutover unit | **Shard / process** (`mainStream`), not per-symbol input topic |
| Performance | No HP/ART/WAL in this phase |

---

## 2. Architecture

### 2.1 Workspace (new)

```text
match-rust/
├── crates/match-protocol/   # spot validate/convert/constants
├── crates/match-core/       # pure engine
├── crates/match-replay/     # golden diff
└── crates/match-spot/       # production bin
```

### 2.2 Data flow

```text
RMQ shard topic (contract_match_order[_main_coin_N])
  → parse List<MqOrder> → check_mq_order_spot → type_convert_spot
  → demux by symbolKey → per-symbol queue → single worker
  → match-core::Engine
  → outbound (push_order / entrust / global no_deal & deeps)
  → Redis exchange_depth:* / error queue
```

### 2.3 Concurrency

- One writer per symbol (mpsc + dedicated task)
- Parallel across symbols
- Depth push throttled (~50ms)

---

## 3. Protocol (bf-match)

See [现货合约MQ-Topic对照.md](../../../docs/现货合约MQ-Topic对照.md).

| Direction | Spot topic |
|-----------|------------|
| Ingress | `contract_match_order`, `contract_match_order_main_coin_{N}` |
| Group | `contract_match_group` (exact string from Java) |
| Fills | `contract_match_order_push_order_{symbol}` |
| No-deal / deeps / robot | **Global** topics (no symbol suffix) |
| New pair | `market_client_new_coin_market` |

| Rule | Spot |
|------|------|
| orderForm | 1 limit, 2 market |
| Market price ≤ 0 | Allowed at ingress |
| NO_DEAL_NUMBER | 25 |
| SEND_MAX_DATA | 1 |
| Redis depth prefix | `exchange_depth:` |
| Start dedupe | **None** (no START_QUEUE/BigNo) |

Restore RPC: `coinMarkets()` + `getEntrustList` — paths from `InitLoadData` archaeology, **not** contract URLs.

---

## 4. Core (match-core)

- Order book: price-time priority aligned with `BBOrder.compareTo`
- Same price: `createTime` asc → `trustOrderNo` string asc
- Handlers ported from `BuyHandler` / `SellHandler` / RatherThan / Equals / LessThan
- Decimals: `bigdecimal`, no `f64` for price/qty

---

## 5. Shell (match-spot)

Modules: `bootstrap`, `inbound` (shard + optional mm), `symbol_worker`, `outbound/*`, `redis_store`, `error_queue`, `rpc/*`, `mq/*`, `health`, `telemetry`.

MQ traits: `MessageSource` / `OrderSink` — `memory` for dev/CI, `rocketmq` for prod.

ACK: align Java `BaseConsumer` — always ACK after attempt.

---

## 6. Testing

| Layer | Gate |
|-------|------|
| L1 | Hand-written limit cross, time priority, partial fill, cancel, market zero price |
| L2 | Java `GoldenTrace.ndjson` → `match-replay` zero unexplained diffs |
| L3 | Shadow or offline dual-run before cutover |

Golden source: **`bf-match` only** — not contract, not C++.

---

## 7. Cutover

Per **mainStream** shard: stop Java → drain → Rust same group → restore → enable outbound → observe ≥72h → roll remaining shards.

Runbook: `docs/cutover-runbook-spot.md` (deliver in S5).

---

## 8. Schedule (2 Rust engineers + 0.3 Java)

| Phase | Weeks | Deliverable |
|-------|-------|-------------|
| S0 Archaeology + scaffold | W1–2 | Topic/RPC table, protocol crate |
| S1 Core + L1 | W3–5 | limit/market/cancel |
| S2 L2 golden | W4–6 | Java exporter, replay CI |
| S3 Shell (memory) | W6–9 | full in-process path |
| S-RMQ | W8–11 | RocketMQ integration |
| S4 Restore E2E | W10–11 | bootstrap + RPC |
| S5 L3 + canary | W12–14 | one shard grey |
| S6 Full rollout | W15–16 | all shards |

**Total: ~14–16 weeks** to production-ready spot replacement.

With AI-assisted codegen: **12–14 weeks**. Raw Java translation demo: **hours–days** — **not** production delivery.

Task WBS: [implementation plan](../plans/2026-07-27-spot-match-greenfield.md).

---

## 9. Out of scope

- Contract Rust migration
- C++ crypto-exchange baseline
- user/mm topic split (optional follow-up)
- HP engine / performance rewrite
- Fixing Java bugs unless separate task + golden update

---

## 10. Rejected approaches

| Approach | Why |
|----------|-----|
| C++ as baseline | Not aligned with production `bf-match` |
| Reuse Rust without golden | Cannot prove equivalence |
| Translate-only | Missing shell + L2/L3 |
| Per-symbol spot ingress topics | Contract change |

---

## 11. Acceptance checklist

1. Ingress validation matches `BBConstants.checkMqOrder`
2. Market zero price path
3. Restore pagination (buy + sell)
4. Global no_deal topic
5. Redis `exchange_depth:` prefix
6. Same-price depth merge
7. `SEND_MAX_DATA=1`
8. No START_QUEUE
9. New coin broadcast without restart
10. L2 golden zero diff
