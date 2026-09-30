# Spot Matching Greenfield Implementation Plan

**中文：** [2026-07-27-spot-match-greenfield.zh-CN.md](./2026-07-27-spot-match-greenfield.zh-CN.md)

**Goal:** Deliver `match-spot` to replace Java `bf-match`, golden-verified, grey cutover by shard.

**Spec:** [../specs/2026-07-27-spot-match-greenfield.md](../specs/2026-07-27-spot-match-greenfield.md)

**Baseline (read-only):** `bf-match` Java modules only.

**Out of scope:** `bf_contract_match`, C++ `crypto-exchange`, any existing Rust code in repo.

---

## File map

| Path | Role |
|------|------|
| `crates/match-protocol/` | spot DTO, validate, convert, constants, topics |
| `crates/match-core/` | OrderBook, Engine, limit/market/cancel |
| `crates/match-replay/` | golden diff |
| `crates/match-spot/` | production binary |
| `testdata/golden/spot/` | NDJSON traces |
| `docs/spot-archaeology.md` | Topic/RPC exact strings |
| `docs/cutover-runbook-spot.md` | shard cutover |

---

## S0 — Archaeology + scaffold (W1–2)

- [ ] S0.1 Write `docs/spot-archaeology.md` from `BBConstants` + `InitLoadData`
- [ ] S0.2 Cargo workspace + empty crates
- [ ] S0.3 `match-protocol` spot boundary + unit tests

## S1 — Core + L1 (W3–5)

- [ ] S1.1 Book ordering (`compare_buy` / `compare_sell`)
- [ ] S1.2 Limit match handlers
- [ ] S1.3 Revoke
- [ ] S1.4 Market + gear + zero price
- [ ] S1.5 `Engine::on_order`

## S2 — L2 golden (W4–6)

- [ ] S2.1 Java `GoldenTraceExporterTest` in `bf-match`
- [ ] S2.2 `match-replay` diff tool
- [ ] S2.3 Golden pack in CI

## S3 — Shell memory path (W6–9)

- [ ] S3.1 config
- [ ] S3.2 mq traits + memory + spot topics
- [ ] S3.3 inbound + router
- [ ] S3.4 symbol_worker
- [ ] S3.5 outbound (global topics, NO_DEAL=25, SEND_MAX=1)
- [ ] S3.6 redis + error_queue
- [ ] S3.7 health + telemetry

## S4 — Bootstrap + restore (W10–11)

- [ ] S4.1 rpc market + order
- [ ] S4.2 bootstrap (no StartQueue)
- [ ] S4.3 main wiring + test env smoke

## S-RMQ — RocketMQ (W8–11, parallel)

- [ ] RMQ.1 ping spike
- [ ] RMQ.2 shard consumer
- [ ] RMQ.3 producers + error queue

## S5 — L3 + grey (W12–14)

- [ ] S5.1 l3-shadow-spot.md
- [ ] S5.2 cutover-runbook-spot.md
- [ ] S5.3 mainStream=0 canary 72h

## S6 — Full rollout (W15–16)

- [ ] Remaining shards + observation report

---

## Done definition

- **Not done:** `cargo build` or Java line-for-line translation
- **Done:** L2 green + test env E2E + one shard 72h canary + runbook signed off
