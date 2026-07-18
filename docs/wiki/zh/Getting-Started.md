# 快速开始

**English：** [en/Getting-Started.md](../en/Getting-Started.md)

## 环境要求

- Rust **1.97.1+**（见 `rust-toolchain.toml`）
- macOS 或 Linux
- 可选：Docker；跑 `make cov` 需 nightly + `cargo-llvm-cov`

## 克隆与测试

```bash
git clone https://github.com/lanpishu6300/match-rust.git
cd match-rust
cargo test --workspace
make ci    # fmt + clippy + 测试 + art + fair_compare
```

## 本地运行 match-contract

在接通 RocketMQ 前使用 **memory** 传输（见 [rmq-spike](../../rmq-spike.md)）。

```bash
export MATCH_CONTRACT_CONFIG=crates/match-contract/config.example.yaml
export MATCH_CONTRACT_LOCAL_SYMBOLS=btcusdt
cargo run -p match-contract
# make run-local
```

健康检查（默认端口 `31015`）：

| 路径 | 含义 |
|------|------|
| `GET /healthz` | 进程存活 |
| `GET /readyz` | 启动完成 |
| `GET /metrics` | Prometheus 指标 |

## Docker

```bash
docker build -t match-rust:local .
docker run --rm -p 31015:31015 \
  -e MATCH_CONTRACT_LOCAL_SYMBOLS=btcusdt \
  match-rust:local
```

## 常用 Make 目标

| 目标 | 作用 |
|------|------|
| `make test` | 工作区测试 |
| `make fair` | 公平微基准（`fill_rate > 0`） |
| `make cov` | 100% 分支覆盖门禁（nightly） |
| `make bench` | Criterion `engine_cmp` |

下一步：[架构](./Architecture.md)
