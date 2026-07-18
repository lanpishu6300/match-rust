# match-rust

面向合约（及现货）路径的 Rust **撮合引擎**工作区：双轨设计——Java 可观测等价核 + 实验性极致低延迟核。

布局与工程化参考 [perpetual_exchange / crypto-exchange](https://github.com/lanpishu6300/crypto-exchange)（C++ 研发仓），目标是与现网 Java 服务 **Topic/JSON 兼容** 灰度切换。本仓为其姊妹项目。

**许可：** [Apache License 2.0](LICENSE)  
**English：** [README.md](README.md)  
**Wiki：** [中文](docs/wiki/zh/Home.md) · [English](docs/wiki/en/Home.md)

---

## 特性

### 等价轨（`match-core`）
- 与 `java-contract-match` 可观测结果对齐的价时优先簿
- 限价 / 市价 / 档位、PostOnly / IOC / FOK（含需保留的 Java quirk）
- `match-replay` 黄金 NDJSON 回放

### 性能轨（`match-core-hp`）
- 定点 `price_tick` / `qty_lot`
- 价位簿 + FIFO、最优价缓存、Level 池
- 可选 ART 风格 radix（`--features art`）
- SPSC worker、伪共享隔离、可配置等待策略
- 异步 WAL 实验（`match-wal`）

### 进程壳（`match-contract`）
- 配置 → RPC 恢复 → Redis → 按 symbol worker
- Memory（及后续 RocketMQ）传输适配
- `/healthz` `/readyz` `/metrics`
- 可选 `--features hp-engine`（L2/L3/L1 span）

---

## 快速开始

```bash
git clone https://github.com/lanpishu6300/match-rust.git
cd match-rust
cargo test --workspace
# 或
make test
make ci
```

本地合约壳（内存传输）：

```bash
export MATCH_CONTRACT_CONFIG=crates/match-contract/config.example.yaml
export MATCH_CONTRACT_LOCAL_SYMBOLS=btcusdt
cargo run -p match-contract
```

公平微基准（强制 `fill_rate > 0`）：

```bash
make fair
```

分支覆盖门禁（需 nightly）：

```bash
make cov
```

---

## 文档导航

| 文档 | 说明 |
|------|------|
| [Wiki 首页（中文）](docs/wiki/zh/Home.md) · [EN](docs/wiki/en/Home.md) | 入门、架构、FAQ、路线图 |
| [完整索引（中文）](docs/README.zh-CN.md) · [EN](docs/README.md) | specs / plans / 运维 |
| [架构说明](docs/ARCHITECTURE.zh-CN.md) · [EN](docs/ARCHITECTURE.md) | Crate 边界与双轨规则 |
| [覆盖率策略](docs/COVERAGE.zh-CN.md) · [EN](docs/COVERAGE.md) | 100% branch 门禁范围 |
| [等价设计](docs/specs/2026-07-17-rust-match-engines-design.zh-CN.md) · [EN](docs/specs/2026-07-17-rust-match-engines-design.md) | 协议 / 切流 |
| [HP 设计](docs/specs/2026-07-18-match-core-hp-design.zh-CN.md) · [EN](docs/specs/2026-07-18-match-core-hp-design.md) | 定点 / 价位簿 |
| [PE 优化](docs/specs/2026-07-18-pe-optimizations-design.zh-CN.md) · [EN](docs/specs/2026-07-18-pe-optimizations-design.md) | 缓存 / ART / wal |
| [切流手册](docs/cutover-runbook.zh-CN.md) · [EN](docs/cutover-runbook.md) | 按 symbol 灰度 |

---

## 状态

| 区域 | 状态 |
|------|------|
| `match-core` 等价 | 进行中 / 黄金回放 |
| `match-core-hp` | 可用实验轨 |
| `match-contract` | Memory 传输；RMQ 待接通 |
| 现货壳 | Stub |
| 生产默认引擎 | **仅 `match-core`** |

---

## 贡献与安全

- [贡献指南（中文）](CONTRIBUTING.zh-CN.md) · [EN](CONTRIBUTING.md)
- [安全策略（中文）](SECURITY.zh-CN.md) · [EN](SECURITY.md)
- [行为准则（中文）](CODE_OF_CONDUCT.zh-CN.md) · [EN](CODE_OF_CONDUCT.md)
- [支持（中文）](SUPPORT.zh-CN.md) · [EN](SUPPORT.md)
- [变更日志（中文）](CHANGELOG.zh-CN.md) · [EN](CHANGELOG.md)

## 致谢

- Java 基线：`java-contract-match`、`java-spot-match`
- [crypto-exchange](https://github.com/lanpishu6300/crypto-exchange) 的 ART / 持久化研发思路
- LMAX Disruptor、Aeron、交易所价位簿通识
