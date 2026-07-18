# Architecture notes

**中文：** [ARCHITECTURE.zh-CN.md](./ARCHITECTURE.zh-CN.md)

## Dual-track rule

1. **Production default** for `match-contract` is always `match-core` (Java-observable equivalence).
2. **`match-core-hp`** is experimental: cleaner semantics, fixed-point hot path. Enable in the shell only via `--features hp-engine`.
3. Do not mix Java quirk tests into `match-core-hp`, or HP allocations into `match-core` hot paths.

## Crate responsibilities

```text
crates/
├── match-protocol/   # BbOrder / MqOrder DTOs, validators, symbol key encode
├── match-core/       # Equivalence engine (BigDecimal / BTreeSet-shaped)
├── match-core-hp/    # HP engine (tick/lot, LevelIndex, SPSC, optional art)
├── match-contract/   # Process: config, restore RPC, Redis, workers, health
├── match-spot/       # Spot shell stub
├── match-replay/     # Golden NDJSON replay CLI
├── match-bench/      # Criterion + fair_compare binary
└── match-wal/        # Async batched WAL (not on default path)
```

## Hot-path constraints (`match-core-hp`)

- No JSON / Tokio / `Mutex` on `on_order`
- Prefer `i64` ticks/lots; convert at `adapter` boundary only
- One writer per symbol book (`HpWorker` / single-threaded worker)

## Messaging

Inbound/outbound Topic names and JSON shapes aim for grey cutover with Java. Until RocketMQ lands, use `rocketmq.transport: memory` (see `docs/rmq-spike.md`).

## Observability

`match-contract` exposes Prometheus text at `/metrics` with names aligned to Java OTel (`match.order.events.total`, …). HP path also accumulates `match.span.l{1,2,3}_*_ns_total`.
