# Getting Started

**中文：** [zh/Getting-Started.md](../zh/Getting-Started.md)

## Prerequisites

- Rust **1.97.1+** (`rust-toolchain.toml`)
- macOS or Linux
- Optional: Docker, nightly + `cargo-llvm-cov` for `make cov`

## Clone and test

```bash
git clone https://github.com/lanpishu6300/match-rust.git
cd match-rust
cargo test --workspace
make ci    # fmt + clippy + tests + art + fair_compare
```

## Run match-contract locally

Uses **memory** transport until RocketMQ is wired ([rmq-spike](../../rmq-spike.md)).

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

## Docker

```bash
docker build -t match-rust:local .
docker run --rm -p 31015:31015 \
  -e MATCH_CONTRACT_LOCAL_SYMBOLS=btcusdt \
  match-rust:local
```

## Useful Make targets

| Target | Purpose |
|--------|---------|
| `make test` | Workspace tests |
| `make fair` | Fair microbench (`fill_rate > 0`) |
| `make cov` | 100% branch gate (nightly) |
| `make bench` | Criterion `engine_cmp` |

Next: [Architecture](./Architecture.md)
