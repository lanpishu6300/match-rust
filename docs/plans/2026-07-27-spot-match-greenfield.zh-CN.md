# 现货撮合 Rust 化实现计划（Greenfield 0→1）

**English：** [2026-07-27-spot-match-greenfield.md](./2026-07-27-spot-match-greenfield.md)

**目标：** 从空仓库交付可替换 Java `bf-match` 的 Rust 进程 `match-spot`，经 golden replay 验证后按分片灰度切流。

**规格：** [../specs/2026-07-27-spot-match-greenfield.zh-CN.md](../specs/2026-07-27-spot-match-greenfield.zh-CN.md)

**基线（只读）：** `bf-match/bztex-match-server-provider`、`bf-match/bztex-match-server-api`

**范围外：** 合约 `bf_contract_match`、C++ `crypto-exchange`、仓库内任何已有 Rust 实现（本计划按新建执行）。

**技术栈：** Rust 1.78+，`bigdecimal`，`serde`/`serde_json`，`tokio`，`reqwest`，`redis`（cluster），RocketMQ Rust 客户端，`tracing`/OTel，YAML 配置。

---

## 文件地图（目标态）

| 路径 | 职责 |
|------|------|
| `Cargo.toml` | Workspace members |
| `crates/match-protocol/` | `MqOrder`、`BbOrder`、`check_mq_order_spot`、`type_convert_spot`、`spot_constants`、`spot_topics` |
| `crates/match-core/` | `OrderBook`、`Engine`、限价/市价/撤单、`MatchEvent` |
| `crates/match-replay/` | CLI + 库：跑输入 → diff `GoldenTrace` |
| `crates/match-spot/` | 生产 bin：config、bootstrap、MQ、worker、outbound、Redis、RPC |
| `testdata/golden/spot/` | 入库 `*.ndjson` |
| `bf-match/.../GoldenTraceExporterTest.java` | Java 导出器（新建，不改生产路径） |
| `docs/cutover-runbook-spot.md` | 分片灰度清单 |
| `docs/l3-shadow-spot.md` | 影子验证 |

---

## S0：考古 + 脚手架（W1–W2）

### Task S0.1 — 考古笔记（阻塞后续 RPC/Topic）

**产出：** `docs/spot-archaeology.md`

- [ ] 从 `BBConstants` 抄录全部 Topic/Group 精确字符串
- [ ] 从 `InitLoadData` 反查 `coinMarkets`、`getEntrustList` HTTP 路径与分页参数
- [ ] 确认 `mainStream` 与 Topic 映射（0 → `contract_match_order`；N → `_main_coin_N`）
- [ ] 确认是否存在类 START_QUEUE 逻辑（默认无）
- [ ] 列出 `TYPES`、`ORDER_FORMS`、`ORDER_STATUS` 枚举

### Task S0.2 — Workspace 脚手架

**创建：**

- `Cargo.toml`（members: protocol, core, replay, spot）
- 各 crate `Cargo.toml` + 占位 `lib.rs` / `main.rs`
- `README.md` 指向规格

**验证：**

```bash
cargo build --workspace
cargo test --workspace
```

### Task S0.3 — `match-protocol` 基础 DTO

**文件：**

- `crates/match-protocol/src/mq_order.rs`
- `crates/match-protocol/src/order.rs`
- `crates/match-protocol/src/decimal.rs`
- `crates/match-protocol/src/spot_validate.rs`
- `crates/match-protocol/src/spot_convert.rs`
- `crates/match-protocol/src/spot_constants.rs`
- `crates/match-protocol/src/spot_topics.rs`

**单测：**

- [ ] 合法/非法 `MqOrder` 分支覆盖 `checkMqOrder`
- [ ] 市价 `trust_price=0` convert 成功
- [ ] `createTime` 缺失拒单

---

## S1：撮核 + L1（W3–W5）

### Task S1.1 — 订单簿排序

**文件：** `crates/match-core/src/order.rs`、`book.rs`

- [ ] `compare_buy` / `compare_sell` 对齐 `BBOrder.compareTo`
- [ ] 同价：`createTime` → `trustOrderNo` 字符串
- [ ] 测试：同 orderNo 重复 insert 拒绝

### Task S1.2 — 限价撮合

**文件：** `match_limit.rs`、`handlers/rather_than.rs`、`equals.rs`、`less_than.rs`

- [ ] 买卖交叉、部分成交、余量挂簿
- [ ] L1：`price_time_priority_older_maker_first`

### Task S1.3 — 撤单

**文件：** `match_limit.rs` 或 `revoke.rs`

- [ ] `orderStatus=3` 路径
- [ ] 簿内 remove_by_order_no

### Task S1.4 — 市价

**文件：** `match_market.rs`

- [ ] 市价买/卖 + `gear` 挡位
- [ ] 入站 `trust_price≤0` 改价路径（对照 `BuyHandler`）

### Task S1.5 — Engine 门面

**文件：** `engine.rs`

- [ ] `Engine::on_order` 按 symbolKey 路由多簿
- [ ] 聚合 `MatchEvent`

**L1 门禁：**

```bash
cargo test -p match-core
```

---

## S2：L2 Golden（W4–W6，与 S1 后期并行）

### Task S2.1 — Java GoldenTrace 导出器

**文件：** `bf-match/.../test/.../GoldenTraceExporterTest.java`

- [ ] 进程内喂 `MqOrder` 序列
- [ ] 输出 NDJSON：逐步 fill、revoke、depth snapshot
- [ ] 首包 ≥10 场景：限价交叉、同价双单、部成、撤单、市价零价

### Task S2.2 — `match-replay`

**文件：** `crates/match-replay/src/lib.rs`、`bin/replay.rs`

- [ ] 读入 golden + 输入序列
- [ ] 调 `match-core` + `type_convert_spot`
- [ ] diff 成交/余量/深度；非零 exit code

### Task S2.3 — Golden 入库

- [ ] `testdata/golden/spot/*.ndjson`
- [ ] CI：`cargo test -p match-replay`

---

## S3：壳层 memory 路径（W6–W9）

### Task S3.1 — 配置

**文件：** `match-spot/src/config.rs`、`config.spot.example.yaml`

### Task S3.2 — MQ trait + memory

**文件：** `match-spot/src/mq/{traits,memory,topics}.rs`

- [ ] `MessageSource` / `OrderSink`
- [ ] 现货 Topic 常量（禁止合约名）

### Task S3.3 — inbound + router

**文件：** `match-spot/src/inbound/{mod,router,shard_consumer}.rs`

- [ ] `handle_mq_data`：parse 数组 → validate → convert → route
- [ ] ACK 策略注释对齐 Java

### Task S3.4 — symbol_worker

**文件：** `match-spot/src/symbol_worker.rs`

- [ ] `recv → engine.on_order → outbound.on_match_events`

### Task S3.5 — outbound

**文件：** `match-spot/src/outbound/*.rs`

- [ ] `push_order`（`SEND_MAX_DATA=1`）
- [ ] `push_entrust`（限价非市价）
- [ ] `push_no_deal`（全局 Topic，`NO_DEAL=25`）
- [ ] `push_deeps` + 深度节流 50ms
- [ ] 同价档位合并（对照 `NoDealProducer.getDepth`）

### Task S3.6 — redis + error_queue

**文件：** `redis_store.rs`、`error_queue.rs`

- [ ] 前缀 `exchange_depth:`
- [ ] 错误队列 key 对齐 Java

### Task S3.7 — health + telemetry

**文件：** `health.rs`、`telemetry.rs`

- [ ] `/healthz`、`/readyz`
- [ ] 指标名对齐 `MatchMetricsRecorder`

**冒烟：** memory transport 跑通「挂单 → 成交 → push_order 消息」。

---

## S4：Bootstrap + Restore（W10–W11）

### Task S4.1 — RPC 客户端

**文件：** `rpc/market.rs`、`rpc/order.rs`

- [ ] `coinMarkets()` + mainStream 过滤
- [ ] `getEntrustList` 分页（买/卖 type 各扫完）
- [ ] `build_mq_order_spot(EntrustListBO)`

### Task S4.2 — bootstrap

**文件：** `bootstrap.rs`

- [ ] 顺序对齐 `InitLoadData.initMain` + `initData`
- [ ] 无 StartQueue
- [ ] 上新 `new_market.rs`（可选 mm consumer）

### Task S4.3 — main 接线

**文件：** `main.rs`

- [ ] 加载配置 → tracing → bootstrap → park

**测试环境清单（README）：**

1. 指向测试 RPC/Redis
2. 确认恢复数量日志
3. 一笔挂+撤产出 push_order

---

## S-RMQ：RocketMQ（W8–W11，与 S3/S4 并行）

### Task RMQ.1 — Spike

**文件：** `examples/rmq_ping.rs` 或 ignored test

- [ ] 测试 NameServer send+recv 30s 内成功
- [ ] 失败则 pin 兼容客户端版本，文档化

### Task RMQ.2 — Consumer

**文件：** `mq/consumer.rs`

- [ ] ORDERLY + CLUSTERING
- [ ] 分片 Topic 订阅（非 per-symbol）

### Task RMQ.3 — Producer

**文件：** `mq/producer.rs`

- [ ] 全局/带 symbol 后缀 Topic
- [ ] 失败 → error_queue

---

## S5：L3 + 灰度（W12–W14）

### Task S5.1 — L3 文档与配置

**文件：** `docs/l3-shadow-spot.md`

- [ ] 离线录制 replay
- [ ] 或 shadow group 消费、禁用 outbound

### Task S5.2 — Runbook

**文件：** `docs/cutover-runbook-spot.md`

- [ ] 单分片切流 10 步 checklist
- [ ] 回滚步骤

### Task S5.3 — 金丝雀

- [ ] mainStream=0 低流量分片 72h
- [ ] 错误队列、深度、对账无异常

---

## S6：全量（W15–W16）

- [ ] mainStream 1..N 滚动
- [ ] 7×24 观察期报告
- [ ] 保留 Java 包回滚能力至稳定 2 周

---

## 规格覆盖检查清单

| 规格项 | Task |
|--------|------|
| 考古 | S0.1 |
| match-protocol spot | S0.3 |
| match-core 限/市/撤 | S1 |
| L1 | S1.5 |
| L2 golden | S2 |
| 壳层 memory | S3 |
| bootstrap/restore | S4 |
| RocketMQ | S-RMQ |
| L3/灰度 | S5–S6 |

---

## 自审备注

- 「直译 bf-match」可在 S1 用 AI 加速首版，但 **S2 L2 绿**才是 S1 的 Done 定义。
- Redis 键前缀必须在 S3.6 从 Java `RedisKey` 复制，不可猜。
- Golden 必须来自 `bf-match`，禁止用合约或 C++ trace。
