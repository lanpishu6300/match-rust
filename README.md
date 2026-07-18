# match-rust

High-performance cryptocurrency **matching engines** in Rust for contract (and spot) paths — dual-track design with a Java-equivalent core and an experimental low-latency core.

Inspired in layout and packaging by [perpetual_exchange / crypto-exchange](https://github.com/lanpishu6300/matching-engine) (C++ R&D), while targeting **Topic/JSON-compatible** cutover against live Java services.

**License:** [Apache License 2.0](LICENSE)

---

## Features

### Equivalence track (`match-core`)
- Price-time priority book aligned with `java-contract-match` observables
- Limit / market / gear, PostOnly / IOC / FOK (including documented Java quirks where required)
- Golden NDJSON replay via `match-replay`

### Performance track (`match-core-hp`)
- Fixed-point `price_tick` / `qty_lot`
- Price-level book + FIFO, best-price cache, level pool
- Optional ART-style radix index (`--features art`)
- SPSC worker, cache-line padded ring, configurable wait strategy
- Async WAL experiment (`match-wal`)

### Process shell (`match-contract`)
- Config → RPC restore → Redis → per-symbol workers
- Memory (and future RocketMQ) transport adapters
- `/healthz` `/readyz` `/metrics` (Prometheus / OTel-aligned names)
- Optional `--features hp-engine` for hp workers + L2/L3/L1 span counters

---

## Architecture

```text
┌─────────────────────────────────────────────────────────────┐
│ match-contract / match-spot (shells)                        │
│  MQ/RPC/Redis/health  →  per-symbol worker                  │
└───────────────┬─────────────────────────────┬───────────────┘
                │ default                     │ feature hp-engine
                ▼                             ▼
         match-core                    match-core-hp
         (Java-shaped)                 (tick/lot + LevelIndex)
                │                             │
                └──────────┬──────────────────┘
                           ▼
                    match-protocol (DTO)
                           │
              match-replay / match-bench / match-wal
```

| Crate | Role | Production default |
|-------|------|--------------------|
| `match-protocol` | Shared order DTOs / checks | — |
| `match-core` | Equivalence engine | **Yes** |
| `match-core-hp` | HP experimental engine | No |
| `match-contract` | Contract process shell | uses `match-core` |
| `match-spot` | Spot shell (stub) | — |
| `match-replay` | Golden replay CLI | — |
| `match-bench` | Criterion + `fair_compare` | — |
| `match-wal` | Async batched WAL | experimental |

---

## Quick Start

### Prerequisites

- Rust **1.97.1+** (see `rust-toolchain.toml`)
- macOS / Linux

### Build & test

```bash
git clone <your-fork-or-remote>/match-rust.git
cd match-rust
cargo test --workspace

# or
make test
make ci          # fmt + clippy + tests + art + fair_compare
```

### Local contract shell (memory transport)

```bash
export MATCH_CONTRACT_CONFIG=crates/match-contract/config.example.yaml
export MATCH_CONTRACT_LOCAL_SYMBOLS=btcusdt
cargo run -p match-contract
# make run-local
```

Health (default port `31015`):

| Path | Meaning |
|------|---------|
| `GET /healthz` | Process up |
| `GET /readyz` | Bootstrap finished |
| `GET /metrics` | Prometheus counters |

---

## Performance

Fair microbench (**fill_rate must be > 0** — rejects zero-fill “fake peaks”):

```bash
make fair
# cargo run -p match-bench --release --bin fair_compare -- --n 50000
```

Criterion suite:

```bash
make bench
```

ART parity:

```bash
make test-art
```

WAL throughput:

```bash
make wal-bench
```

Published numbers and methodology: [`docs/bench-results.md`](docs/bench-results.md), [`docs/fair-compare.md`](docs/fair-compare.md), [`docs/e2e-budget.md`](docs/e2e-budget.md).

> End-to-end latency is usually dominated by MQ/JSON (L4), not the L1 microkernel. See the e2e budget doc before chasing nanoseconds.

---

## Documentation

Full index: **[`docs/README.md`](docs/README.md)**

| Doc | Description |
|-----|-------------|
| [Architecture notes](docs/ARCHITECTURE.md) | Crate map & dual-track rules |
| [Equivalence design](docs/specs/2026-07-17-rust-match-engines-design.md) | Protocol / cutover |
| [HP design](docs/specs/2026-07-18-match-core-hp-design.md) | Fixed-point / price-level |
| [PE optimizations](docs/specs/2026-07-18-pe-optimizations-design.md) | Cache / ART / wal A→B→C |
| [OSS best practices](docs/best-practices.md) | Disruptor / Aeron / Seastar mapping |
| [Cutover runbook](docs/cutover-runbook.md) | Per-symbol grey release |
| [RMQ spike](docs/rmq-spike.md) | RocketMQ status |

---

## Configuration

See [`crates/match-contract/config.example.yaml`](crates/match-contract/config.example.yaml).

RocketMQ production adapter is **not** wired yet (`transport: memory`). Details: [`docs/rmq-spike.md`](docs/rmq-spike.md).

---

## Docker

```bash
docker build -t match-rust:local .
docker run --rm -p 31015:31015 \
  -e MATCH_CONTRACT_LOCAL_SYMBOLS=btcusdt \
  match-rust:local
```

---

## Contributing & release

- [`CONTRIBUTING.md`](CONTRIBUTING.md) — branch, test, PR expectations  
- [`CHANGELOG.md`](CHANGELOG.md) — version history  
- CI: `.github/workflows/ci.yml` on push/PR  

Suggested release tags: `v0.1.0`, `v0.2.0`, …

---

## Status

| Area | Status |
|------|--------|
| `match-core` equivalence | In progress / golden replay |
| `match-core-hp` | Usable experimental |
| `match-contract` shell | Memory transport; RMQ TBD |
| Spot shell | Stub |
| Production default engine | **`match-core` only** |

---

## Acknowledgments

- Java baseline engines: `java-contract-match`, `java-spot-match`
- Layout/performance ideas from perpetual_exchange (`crypto-exchange`) ART / persistence research
- Industry patterns: LMAX Disruptor, Aeron, exchange price-level books
