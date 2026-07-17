# match-rust

Rust port of the  contract matching engine (`java-contract-match`), structured as a Cargo workspace with shared `match-core`, `match-protocol`, and `match-replay` crates.

## Documentation

| Doc | Description |
|-----|-------------|
| [Design spec](../docs/superpowers/specs/2026-07-17-rust-match-engines-design.md) | Architecture, protocol alignment, milestones |
| [HP dual-track design](../docs/superpowers/specs/2026-07-18-match-core-hp-design.md) | `match-core-hp` fixed-point / price-level book / bench |
| [Implementation plan](../docs/superpowers/plans/2026-07-17-rust-match-engines.md) | Equivalence-track task breakdown |
| [HP implementation plan](../docs/superpowers/plans/2026-07-18-match-core-hp.md) | High-performance dual-track tasks (H0–H3) |
| [OSS best practices](docs/best-practices.md) | Disruptor / Aeron / Seastar → code mapping |
| [L3 shadow validation](docs/l3-shadow.md) | Pre-prod shadow consume / offline replay |
| [Symbol cutover runbook](docs/cutover-runbook.md) | Per-symbol grey cut and rollback |
| [RMQ spike notes](docs/rmq-spike.md) | NameServer client compatibility status |
| [Java OTel metrics](../java-contract-match/docs/opentelemetry-metrics.md) | Metric names Rust counters align with |

## Build / test

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test --workspace
```

## match-core-hp (dual-track, experimental)

`match-core-hp` is a **high-performance experimental** matching core (fixed-point ticks/lots, price-level book, optional SPSC worker). It is **not** Java-equivalent and is **not** used by production `match-contract`.

| Track | Crate | Production default |
|-------|-------|--------------------|
| Equivalence | `match-core` | Yes (`match-contract`) |
| Performance | `match-core-hp` | No |

Protocol/`BbOrder` conversion lives only in `match_core_hp::adapter` (boundary). Hot path stays in `i64` tick/lot space.

Industry practices (LMAX Disruptor, Aeron, Seastar, exchange-style books) are documented in [`docs/best-practices.md`](docs/best-practices.md) and wired into `SpscRing` (cache-line padded cursors, batch `pop_n`), `WaitStrategy`, and optional `--features affinity` CPU pinning.

### Bench vs match-core

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo bench -p match-bench --bench engine_cmp -- --sample-size 20
```

Published numbers: [`docs/bench-results.md`](docs/bench-results.md) (target ≥5× on `cross_full` / `partial_walk`).

## match-contract

Binary shell: config → bootstrap (RPC restore, Redis wipe/link, per-symbol workers) → inbound/outbound messaging.

### RocketMQ status

**Production RocketMQ is not wired yet.** NameServer spike against `192.168.0.241:9876` timed out; see [`docs/rmq-spike.md`](docs/rmq-spike.md). Runtime uses `OrderSink` / `MessageSource` with an in-memory (optional file-channel) adapter (`rocketmq.transport: memory`).

### Local run (memory transport)

```bash
export MATCH_CONTRACT_CONFIG=crates/match-contract/config.example.yaml
# Skip RPC/Redis; start workers for listed symbols only:
export MATCH_CONTRACT_LOCAL_SYMBOLS=btcusdt
cargo run -p match-contract
```

Publish inbound JSON arrays via the `MemoryMessageSource` API in tests, or drop `*.json` files under `{memory_dir}/in/` when configured.

### Health and metrics

When `health.enabled: true` (default), the process serves:

| Path | Meaning |
|------|---------|
| `GET /healthz` | Process up |
| `GET /readyz` | Bootstrap finished (RPC restore, workers, consumers) |
| `GET /metrics` | Prometheus text counters aligned with Java `match.*` OTel names |

Default port `31015` mirrors Java `java-contract-match` `server.port`.

### Test-env smoke (when RPC/Redis/RMQ reachable)

1. Point `config` at test RPC / Redis / RMQ; set `transport: memory` until RMQ adapter lands (or `rocketmq` after spike passes).
2. Prefer `match.symbols_whitelist: ["one-low-traffic-symbol"]`.
3. Confirm restore count logs; place + cancel should produce `usdt_contract_match_order_push_order_{encoded}` payloads (memory sink or live MQ).
