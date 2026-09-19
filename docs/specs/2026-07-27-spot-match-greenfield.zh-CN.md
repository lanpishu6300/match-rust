# 现货撮合 Rust 化方案（Greenfield 0→1）

**English：** [2026-07-27-spot-match-greenfield.md](./2026-07-27-spot-match-greenfield.md)

**日期：** 2026-07-27  
**状态：** Draft — 待评审  
**范围：** **仅现货**整服务替换 Java `bf-match`  
**基线代码：** `bf-match`（Java/Spring Boot + RocketMQ）— **唯一**行为与协议参考  
**实现计划：** [../plans/2026-07-27-spot-match-greenfield.zh-CN.md](../plans/2026-07-27-spot-match-greenfield.zh-CN.md)

**关联：** [现货合约MQ-Topic对照.md](../../../docs/现货合约MQ-Topic对照.md)、[现货撮合Topic拆分分片方案.md](../../../docs/现货撮合Topic拆分分片方案.md)、[2026-07-27-spot-match-shell-design.zh-CN.md](./2026-07-27-spot-match-shell-design.zh-CN.md)（壳层细节附录）

---

## 0. 文档定位

本文是 **从 0 到 1** 的独立方案，假设：

| 假设 | 说明 |
|------|------|
| **无既有 Rust 撮合代码** | 不继承、不迁移任何 `match-rust` 仓库内已有实现；crate 名可复用，代码按本文新建 |
| **无 C++ 基线** | `crypto-exchange`、clearing-match 等为实验路径，**不是**验收标准 |
| **无合约 Rust 依赖** | 本期不交付 `match-contract`；合约继续跑 Java `bf_contract_match` |
| **验收标准** | 与现网 `bf-match` **可观测等价**（golden replay），不是「理论正确撮合」 |

与 [2026-07-17-rust-match-engines-design.zh-CN.md](./2026-07-17-rust-match-engines-design.zh-CN.md) 的差异：父设计为「先合约后现货 + 双壳」；**本文仅现货**，可并行立项，不阻塞合约 Java。

---

## 1. 决策摘要

| 项 | 选择 |
|----|------|
| 交付形态 | **整服务替换**：Rust 进程 `match-spot` 承接 MQ 入出、启动恢复、深度/成交推送 |
| 协议 | Topic/JSON 与 `bf-match` 语义兼容；命名空间 `contract_match_*`（**现货专用**，与永续无关） |
| 撮核 | 新建 `match-core`：**限价 / 市价 / 撤单**；不含 PostOnly/IOC/FOK |
| 壳层 | 新建 `match-spot`：分片消费、symbol demux、全局深度 Topic、现货 Redis/RPC |
| 验收 | L1 单测 + L2 golden（Java 导出）+ L3 影子/灰度 |
| 缺陷策略 | 默认 **原样保留** `bf-match` 可观测行为；修 bug 另开任务并更新 golden |
| 切流单元 | **分片 / 进程实例**（`mainStream` 0..N），非 per-symbol Topic |
| 性能轨 | 本期不做 HP/ART/WAL；内部结构可换，对外结果必须过 replay |

---

## 2. 背景

### 2.1 现网现货撮合（Java）

生产服务 **`bf-match`**（`bztex-match-server-provider`）：

- Spring Boot + RocketMQ + Redis
- 内存 `TreeSet<BBOrder>` 价时优先订单簿
- **每交易对单线程**撮合（`ORDER_QUEUE_MAP`）
- 入站：**分片 Topic 混流**（多币 JSON 数组），按 `symbolKey` demux
- 订单类型：**限价、市价、撤单**（无 PostOnly/IOC/FOK）
- 启动：**无**合约侧 `START_QUEUE` / `BigNo` 去重模型

Java 模块规模（参考复杂度）：provider ~3,700 LOC；API/RPC 另计。

### 2.2 上下游

```text
bf_order（订单中心）
    │  contract_match_order[_main_coin_N]
    ▼
bf-match（Java 现货撮合）  ←── 本期替换目标
    │  push_order / entrust / no_deal / deeps / robot
    ▼
订单中心 / 行情 / Redis（exchange_depth:*）
```

合约链路（`usdt_contract_match_*`）**不在本期范围**。

### 2.3 目标

1. 交付可生产运行的 Rust 进程 `match-spot`，L1–L3 通过后按 **分片** 灰度顶替 `bf-match`。
2. 撮核与壳严格分层；协议差异仅在 `match-protocol` 现货边界。
3. 用 **`bf-match` 导出的 golden** 锁死验收；禁止借用合约 trace。

### 2.4 非目标

- 不改现货生产 Topic 名与下游 JSON 字段语义。
- 不迁移合约撮合、不引用 C++ `crypto-exchange`。
- 不合并现货与合约 Topic 命名空间。
- 不把 user/mm Topic 拆分作为硬依赖（可预留接口，见 §12）。
- 不在本期「顺手修」Java 已知缺陷。
- 不以「直译 Java 几小时跑通」作为交付定义。

---

## 3. 总体架构

### 3.1 仓库布局（新建 workspace）

```text
match-rust/                          # 或独立 repo match-spot-rust/
├── Cargo.toml                       # workspace
├── testdata/golden/spot/            # L2 黄金轨迹 (*.ndjson)
├── docs/
│   ├── cutover-runbook-spot.md
│   └── l3-shadow-spot.md
└── crates/
    ├── match-protocol/              # MqOrder / BbOrder / spot 校验转换
    ├── match-core/                  # 纯撮核（无 IO）
    ├── match-replay/                # golden diff CLI
    └── match-spot/                  # 唯一生产 bin
```

**本期不包含：** `match-contract`、`match-core-hp`、Aeron/WAL 等性能实验轨。

### 3.2 Crate 职责

| Crate | 职责 | 禁止 |
|-------|------|------|
| `match-protocol` | DTO、`check_mq_order_spot`、`type_convert_spot`、Topic/常量、decimal 解析 | 持有订单簿 |
| `match-core` | 价时优先簿、限价/市价/撤单、`MatchEvent` | 感知 MQ/Redis/RPC/Topic |
| `match-spot` | 配置、bootstrap、MQ、worker、outbound、Redis、RPC、指标、健康检查 | 私自改撮合规则 |
| `match-replay` | 输入序列 → 引擎 → 与 golden diff | 接生产出站 |

### 3.3 依赖图

```text
                    ┌─────────────┐
                    │ match-spot  │  bin
                    └──────┬──────┘
           ┌───────────────┼───────────────┐
           ▼               ▼               ▼
    match-protocol    match-core      match-replay
```

### 3.4 技术栈

| 组件 | 选型 |
|------|------|
| Rust | 1.78+，edition 2021 |
| 小数 | `bigdecimal`（对齐 Java `BigDecimal` 语义） |
| 序列化 | `serde` / `serde_json` |
| 异步运行时 | `tokio` |
| HTTP 恢复 | `reqwest` |
| Redis | `redis` crate，`cluster` feature |
| RocketMQ | Apache RocketMQ Rust 客户端（生产前 ping 门禁） |
| 观测 | `tracing` + OpenTelemetry（指标名对齐 `MatchTelemetry`） |
| 配置 | YAML（`figment` 或 `config`） |

---

## 4. 运行时架构

### 4.1 进程拓扑

每个 **`mainStream` 分片** 部署一个 `match-spot` 实例（与 Java 分片部署一致）：

| mainStream | 典型入站 Topic |
|------------|----------------|
| 0 | `contract_match_order` |
| 1..N | `contract_match_order_main_coin_{N}` |

同一 consumer group（`contract_match_group`，实现前对照 Java 精确串）下，**同一分片同一时刻仅一个 active 引擎**（Java 或 Rust）。

### 4.2 进程内数据流

```text
RMQ 分片 Topic（contract_match_order | contract_match_order_main_coin[_N]）
  │  可选：contract_match_order_mm（做市）
  ▼
parse JSON List<MqOrder>
  → check_mq_order_spot → type_convert_spot
  → InboundRouter：symbolKey → per-symbol mpsc 队列
  → symbol_worker（每币单线程）
  → match-core::Engine::on_order
  → MatchEvent 流
  → outbound（push_order / entrust / no_deal / deeps + Redis）
  → 发送失败 → Redis error queue → 后台重试
```

```mermaid
flowchart TB
  subgraph ingress [入站]
    RMQ1[contract_match_order]
    RMQ2[contract_match_order_mm]
    NM[market_client_new_coin_market]
  end

  subgraph spot [match-spot]
    IN[inbound::handle_mq_data]
    RT[router symbolKey demux]
    Q[per-symbol queue]
    W[symbol_worker + Engine]
    OB[outbound]
  end

  subgraph egress [出站]
    PO[push_order_{symbol}]
    PM[market_entrust_{symbol}]
    ND[全局 no_deal]
    DP[全局 deeps / robot]
    RD[(Redis exchange_depth:*)]
  end

  RMQ1 --> IN
  RMQ2 --> IN
  IN --> RT --> Q --> W --> OB
  OB --> PO & PM & ND & DP & RD
  NM --> bootstrap
```

### 4.3 并发模型

| 规则 | 实现 |
|------|------|
| 每 symbol 单写者 | 独立 `mpsc` + 单 task 循环 `recv → on_order → outbound` |
| 跨 symbol 并行 | 每币一个 worker task |
| 入站 demux | 多 consumer 线程可并行 parse；**入队后**按 symbol 串行 |
| 深度推送 | 节流 `depth_push_interval_ms`（默认 **50**，对齐 Java `@Value` ~49ms） |

---

## 5. 协议对齐（bf-match）

> Topic 对照详见 [现货合约MQ-Topic对照.md](../../../docs/现货合约MQ-Topic对照.md)。  
> 前缀 `contract_match_*` **仅现货**；禁止与 `usdt_contract_match_*` 混用。

### 5.1 Topic / Group

| 方向 | Topic / Group | 说明 |
|------|---------------|------|
| 入站（普通） | `contract_match_order` 及 `contract_match_order_main_coin_{N}` | 分片混流 |
| 入站（做市） | `contract_match_order_mm`（及分片变体，以 Java 为准） | 可选 consumer |
| 消费组 | `contract_match_group` | 考古精确串 |
| 成交回传 | `contract_match_order_push_order_{symbol}` | 做市可加 `_mm` 后缀 |
| 委托推送 | `contract_match_market_entrust_order_{symbol}` | 限价挂单；市价不发 |
| 盘口 | `contract_match_market_push_no_deal` | **全局**，无 symbol 后缀 |
| 深度 | `contract_match_market_push_deeps` | **全局** |
| 机器人 | `contract_match_market_push_robot` | **全局** |
| 新交易对 | `market_client_new_coin_market` | BROADCAST |

入站 body：JSON **数组** `List<MqOrder>`。

### 5.2 校验与转换

| 规则 | 现货（bf-match） |
|------|------------------|
| owner `type` | `{1,2,3}`（以 `BBConstants` 为准） |
| `orderForm` | `1` 限价、`2` 市价 |
| `orderStatus` 入站 | `0/2/3`（待成交/部成/撤单中） |
| 市价 `trust_price ≤ 0` | **允许**；引擎内改价（如买单用 `MAX`） |
| 合约专用字段 | 入站不强制；convert 不得捏造影响成交的保证金/杠杆 |

```rust
// match-protocol 现货边界（示意）
pub fn check_mq_order_spot(mq: &MqOrder) -> bool;
pub fn type_convert_spot(mq: &MqOrder) -> Option<BbOrder>;
```

### 5.3 常量（以 bf-match 为准）

| 常量 | 值 | 用途 |
|------|-----|------|
| `NO_DEAL_NUMBER` | **25** | 盘口档位数 |
| `SEND_MAX_DATA` | **1** | 成交批量发送 |
| 深度 Redis 前缀 | `exchange_depth:` | 非 `contract_exchange_depth:` |

### 5.4 Redis

| Key / 用途 | 行为 |
|------------|------|
| link key | 启动占位；防重复起 worker（前缀以 Java `RedisKey` 为准） |
| `exchange_depth:{symbol}detail\|trade\|paint` | 启动删除；深度写入 |
| `poc_redis_send_mq_error_data_queue` | MQ 发送失败入队；对齐 `SendErrorData` 重试 |

### 5.5 恢复 RPC（M0 考古必填）

从 `bf-match` `InitLoadData` 反查 Feign → HTTP 路径，**禁止**套用合约 URL：

| Java Client | 方法 | Rust 模块 |
|-------------|------|-----------|
| `CoinMarketRemoteServiceClient` | `coinMarkets()` | `match-spot/src/rpc/market.rs` |
| `EntrustRemoteServiceClient` | `getEntrustList(EntrustBO)` | `match-spot/src/rpc/order.rs` |

Bootstrap 过滤 **`mainStream == 本实例 shard`**。恢复字段映射对齐 `buildMqOrder(EntrustListBO)`：

- `priceType` → `orderForm`
- `userType` → `type`
- `remainAmount` → `trustNumber`
- `price` → `trustPrice`
- `entrustNo` → `trustOrderNo`

### 5.6 启动去重

现货 `InitLoadData` **无**合约 `START_QUEUE` / `BigNo` / 720s 模型。**默认不实现**；M0 考古若发现分支，再补并文档化。

---

## 6. 撮核设计（match-core）

### 6.1 订单簿排序

对齐 Java `BBOrder.compareTo`：

| 侧 | 价格 | 同价 tie-break |
|----|------|----------------|
| 买 | 高 → 低 | `createTime` 升序 → `trustOrderNo` **字符串**升序 |
| 卖 | 低 → 高 | 同上 |

数据结构：`BTreeSet` + 显式 `compare_buy` / `compare_sell`（便于单测与 golden 对照）。

### 6.2 引擎 API

```rust
pub struct Engine { /* symbolKey → OrderBook */ }

pub enum MatchEvent {
    Fill { /* taker/maker 单号、价、量、余量、status */ },
    Resting { /* 限价剩余挂簿 */ },
    Revoke { /* 撤单成功 */ },
}

impl Engine {
    pub fn on_order(&mut self, order: BbOrder) -> Vec<MatchEvent>;
}
```

### 6.3 路由（对齐 EventOrderHandler）

```text
orderStatus = 3           → 撤单
orderForm = 1             → 限价买/卖 → RatherThan / Equals / LessThan
orderForm = 2             → 市价买/卖（gear 挡位限制）
其他 orderForm            → 拒单 / 无事件（与 Java 一致）
```

Handler 逻辑以 `bf-match` `BuyHandler` / `SellHandler` / `RatherThanHandler` / `EqualsHandler` / `LessThanHandler` 为参考移植，**可观测输出**为准。

### 6.4 小数

- 内部演算：`bigdecimal`
- 禁止 `f64` 参与价量比较
- Golden 比较：数值语义一致即可；JSON 尾零/科学计数法不强制

---

## 7. 壳层设计（match-spot）

### 7.1 模块结构

```text
crates/match-spot/src/
├── main.rs
├── config.rs
├── bootstrap.rs            # InitLoadData 等价
├── inbound/
│   mod.rs                  # handle_mq_data
│   router.rs               # symbolKey → Sender
│   shard_consumer.rs
│   mm_consumer.rs          # 可选
│   new_market.rs           # AddCoinMarketConsumer
├── symbol_worker.rs
├── outbound/
│   mod.rs                  # 深度节流
│   push_order.rs
│   push_entrust.rs
│   push_no_deal.rs
│   push_deeps.rs
│   depth_agg.rs
├── redis_store.rs
├── error_queue.rs
├── rpc/{market,order,response}.rs
├── mq/{topics,traits,memory,consumer,producer}.rs
├── health.rs
└── telemetry.rs
```

### 7.2 Bootstrap 顺序

1. `startup_delay_ms`（Java 约 3–10s，以考古为准）
2. RPC `coinMarkets()` → 过滤 `mainStream`
3. 每 symbol：清 Redis 深度键 + link key → 建 queue → 启动 worker
4. 分页 `getEntrustList`（买/卖分别扫完）→ 转 `BbOrder` → 入队/入簿
5. 注册 RMQ consumer（及 optional mm / new_market）
6. `/readyz` 置 ready

### 7.3 MQ 抽象

```rust
trait MessageSource {
    async fn recv_batch(&mut self) -> Vec<RawMessage>;
    fn ack(&mut self, msg: &RawMessage);
}

trait OrderSink {
    async fn send(&mut self, topic: &str, body: &[u8]) -> Result<()>;
}
```

| 实现 | 阶段 |
|------|------|
| `mq/memory.rs` | S0–S4 开发、L1/L2 CI |
| `mq/rocketmq.rs` | S-RMQ 生产 |

**ACK 策略：** 对齐 Java `BaseConsumer` — 处理尝试后 **总是 ACK**（含异常路径，一期不顺便修）。

### 7.4 配置示例

```yaml
# config.spot.example.yaml
shard:
  main_stream: 0

startup_delay_ms: 3000
depth_push_interval_ms: 50

match:
  queue_capacity: 4096

rpc:
  market_base_url: "http://market-service"
  order_base_url: "http://order-center"

redis:
  cluster_nodes: ["redis://127.0.0.1:6379"]

rocketmq:
  transport: memory          # 生产前改为 rocketmq
  name_server: "host:9876"
  consumer_group: contract_match_group
  enable_mm_consumer: true

telemetry:
  enabled: true
```

---

## 8. 等价测试与验收

### 8.1 等价定义

同输入 `MqOrder` 序列下，以下 **必须一致**：

| 维度 | 内容 |
|------|------|
| 成交 | taker/maker 单号、价、量、双方剩余、顺序 |
| 状态 | 部成/全成/撤成功 |
| 订单簿 | 事件后买卖盘价位与挂单量 |
| 深度/盘口 | 档位数与各档价量（节流后快照） |

**不强制：** 日志、msgId、JSON key 序、小数文本格式。

Golden 期望值来自 **`bf-match` Java 跑数**，不是理想正确撮合。

### 8.2 三层门禁

| 层 | 内容 | 门禁 |
|----|------|------|
| **L1** | 手写限价交叉、同价时间优先、部分成交、撤单、市价零价 | CI 必过 |
| **L2** | Java 导出 `GoldenTrace.ndjson` → `match-replay` diff | 合并必过 |
| **L3** | 录制入站离线双跑，或 shadow 消费（**禁生产 outbound**） | 切流前 |

### 8.3 Golden 导出器（Java 侧，只读）

在 `bf-match` 增加 JUnit 夹具（不改生产路径）：

- 进程内驱动 handler / 或录制 `MqOrder` 序列
- 输出 NDJSON：`GoldenTrace`（逐步成交、余量、深度快照）

Rust **禁止**用合约 golden 或 C++ trace。

---

## 9. 部署与灰度

### 9.1 部署

```text
K8s/VM：match-spot × M（每 mainStream 一片）
  → RocketMQ NameServer
  → Redis Cluster
  → order-center / market HTTP
```

### 9.2 切流步骤（按分片）

1. **预热：** Rust 起、不接生产；L2 全绿；测试环境 restore 演练
2. **单分片：** 停 Java 该分片消费 → 队列排空 → Rust 同 group 订阅 → 恢复 → 开 outbound
3. **观察：** 错误队列、深度新鲜度、订单对账（≥72h）
4. **滚动：** mainStream 1..N 逐片重复
5. **回滚：** 停 Rust → 启 Java（同 restore 路径）

详细清单见 `docs/cutover-runbook-spot.md`（S6 交付）。

---

## 10. 里程碑与排期

**团队基线：** 2 名 Rust 工程师 + 0.3 FTE Java（`bf-match` golden/考古）+ QA/运维配合灰度。

| 阶段 | 名称 | 周次 | 交付 |
|------|------|------|------|
| **S0** | 考古 + 脚手架 | W1–W2 | Topic/RPC 精确表、`match-protocol`、workspace |
| **S1** | 撮核 + L1 | W3–W5 | `match-core` 限/市/撤、L1 全绿 |
| **S2** | L2 golden | W4–W6 | Java 导出器、首包 golden、replay CI |
| **S3** | 壳层（memory） | W6–W9 | bootstrap/inbound/worker/outbound/redis |
| **S-RMQ** | RocketMQ | W8–W11 | ping、consumer/producer、集成测试 |
| **S4** | restore + E2E | W10–W11 | RPC 恢复、测试环境冒烟 |
| **S5** | L3 + 灰度 | W12–W14 | 单分片金丝雀、runbook、回滚演练 |
| **S6** | 全量 | W15–W16 | 其余分片滚动、观察期 |

**合计：约 14–16 周（3.5–4 月）** 到生产全分片可切流。

| 加速条件 | 压缩后 |
|----------|--------|
| AI 辅助 codegen + 2 人 | **12–14 周** |
| + 第 3 人专 RMQ/壳层 | **10–12 周** |
| 仅「直译 Java」demo（无 L2/RMQ/灰度） | **1–3 天**（**非生产交付**） |

任务级 WBS 见 [实现计划](../plans/2026-07-27-spot-match-greenfield.zh-CN.md)。

---

## 11. 人力与成本（估算）

| 角色 | 投入 |
|------|------|
| Rust 高级工程师 ×2 | ~28–32 人周 |
| Java 撮合专家 | ~8 人周 |
| QA + 运维 | ~8 人周 |
| **合计** | **~44–48 人周（11–12 人月）** |

按 ¥60k/人月：约 **¥70万–75万**（不含测试环境硬件）。

---

## 12. 风险

| 风险 | 缓解 |
|------|------|
| Golden 对不齐（BigDecimal / 同价排序 / 市价零价） | L2 每日跑；专项 L1 case |
| `contract_match_*` 误当合约 Topic | 文档 + code review + Topic 单测 |
| 全局深度 Topic 抄成 per-symbol | 对照 `BBConstants` PR checklist |
| RMQ Rust 客户端行为差异 | ping 门禁 + ACK 集成测试 |
| 同分片双活消费拆簿 | 运维 checklist：单 active |
| 直译陷阱（几小时「写完」） | 以 L2 绿为 Done，不以 compile 为 Done |

---

## 13. 可选后续（本期不做）

| 项 | 说明 |
|----|------|
| user/mm Topic 拆分 | [现货撮合Topic拆分分片方案.md](../../../docs/现货撮合Topic拆分分片方案.md) |
| HP 引擎 / 性能重构 | 切流稳定后再开 |
| 合约 Rust 化 | 独立项目；可复用本 workspace 的 `match-core` 并扩展高级 orderForm |

---

## 14. 已否决方案

| 方案 | 原因 |
|------|------|
| 以 C++ `crypto-exchange` 为基线 | 与现网 `bf-match` 协议/行为不对齐 |
| 继承现有 Rust 代码不跑 golden | 无法证明与 Java 等价 |
| 直译 Java 即上线 | 缺 MQ/Redis/RPC 壳与 L2/L3 |
| 现货改用 per-symbol 入站 Topic | 改生产契约，超范围 |
| 分叉 `match-core-spot` | 长期维护两套簿逻辑 |

---

## 15. 验收检查表

| # | 项 | 对照 |
|---|-----|------|
| 1 | 入站拒单 | `BBConstants.checkMqOrder` 全分支 |
| 2 | 市价零价 | `typeConvert` + `BuyHandler` MAX 路径 |
| 3 | 恢复分页 | buy/sell `getEntrustList` 各扫完 |
| 4 | 深度 Topic | 全局 `contract_match_market_push_no_deal` |
| 5 | Redis 前缀 | `exchange_depth:` |
| 6 | 同价深度合并 | `NoDealProducer.getDepth` |
| 7 | 成交批量 | `SEND_MAX_DATA=1` |
| 8 | 无 START_QUEUE | 现货 bootstrap |
| 9 | 新币广播 | 运行时加队列不重启 |
| 10 | L2 golden | bf-match trace 零未解释 diff |

---

## 16. Java → Rust 映射速查

| bf-match（Java） | Rust 落点 |
|------------------|-----------|
| `BBConstants` | `match-protocol::spot_*` |
| `BaseConsumer` | `match-spot::inbound` |
| `InitLoadData` | `match-spot::bootstrap` |
| `EventOrderHandler` + `*Handler` | `match-core` + `symbol_worker` / `outbound` |
| `OrderProducer` 等 | `match-spot::outbound::*` |
| `SendErrorData` | `match-spot::error_queue` |
| `AddCoinMarketConsumer` | `match-spot::inbound::new_market` |
