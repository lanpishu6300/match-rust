# 现货撮合壳层设计（M5）

**English：** [2026-07-27-spot-match-shell-design.md](./2026-07-27-spot-match-shell-design.md)

**日期：** 2026-07-27  
**状态：** Draft — 待评审  
**父设计：** [2026-07-17-rust-match-engines-design.zh-CN.md](./2026-07-17-rust-match-engines-design.zh-CN.md) §4.4 / M5  
**基线代码：** Java 现货撮合 `bf-match`（文档中曾称 `java-spot-match`）  
**关联：** [现货合约MQ-Topic对照.md](../../../docs/现货合约MQ-Topic对照.md)、[现货撮合Topic拆分分片方案.md](../../../docs/现货撮合Topic拆分分片方案.md)

---

## 0. 决策摘要

| 项 | 选择 |
|----|------|
| 目标 | **现货**整服务替换：Rust 承接 MQ 入出、启动恢复、深度/成交推送；Topic/JSON 与 `bf-match` 语义兼容 |
| 撮核 | **复用 `match-core`** 限价/市价/撤单；**禁止**再分叉一套订单簿 |
| 协议 | 在 `match-protocol` 增加 **现货边界**（校验/转换/常量/Topic）；合约路径保持不变 |
| 壳层 | 填满 `match-spot`，参照 `match-contract` 模式；现货 Topic、Redis、RPC、分片 |
| 默认引擎 | 生产默认 = `match-core`（可观测等价）。可选 `hp-engine` 仅 feature，且靠后 |
| 缺陷策略 | 默认保留 `bf-match` 可观测行为；修 bug 另开任务并更新 golden |
| 切流单元 | **分片 / 进程实例**（`mainStream`），不是合约那种按 symbol 独占 Topic |
| 本期非目标 | 真 RocketMQ 客户端（与合约共用、另轨）、user/mm Topic 拆分上线（可选后续）、顺手修现货 Java 缺陷 |

---

## 1. 背景

### 1.1 现状

| 组件 | 状态（2026-07-27） |
|------|-------------------|
| `match-core` | 合约形态下 L1 等价：限价/市价/撤单（另含 PostOnly/IOC/FOK） |
| `match-protocol` | 合约 `check_mq_order` / `type_convert` / `usdt_contract_*` |
| `match-contract` | 完整壳：内存 MQ、bootstrap、按币 worker、Redis、恢复 RPC、指标 |
| `match-spot` | 仅 stub |
| RocketMQ | 未接通（仅 memory）— 生产切流共同阻塞点 |

父设计已定 **先合约、后现货**，共享撮核 + 双壳。M5 原先只有一段占位，本文给出可落地的现货设计。

### 1.2 为何「合约形态协议」挡现货

`match-core` 的价时优先簿可以服务现货限价/市价/撤单。挡住现货的是 **入站边界**：

- `type_convert` 强依赖合约字段，且拒绝 `trust_price ≤ 0`
- `TYPES` / `ORDER_FORMS` 按合约（owner 1–8；含 PostOnly 等）
- Topic / Redis 前缀是 `usdt_contract_*` / `contract_exchange_depth:`
- 恢复 RPC 是合约行情/委托接口

现货 Java（`bf-match`）使用历史名 `contract_match_*`（**仅现货命名空间**，与永续无关）、更松校验、按分片消费。

### 1.3 目标

1. 交付可运行的 `match-spot`，在 L1–L3 通过后可按分片顶替 `bf-match`。
2. 保持 **一套** `match-core`；现货差异落在协议 + 壳。
3. 用现货形态 golden / replay 对齐 `bf-match`，不用合约 trace。

### 1.4 非目标

- 不改现货生产 Topic 名与下游 JSON 语义。
- 不把 `match-core-hp` 设为现货切流默认。
- 不合并现货与合约 Topic 命名空间。
- 不把 user/mm 拆分当作 M5 硬依赖（可预留，后上）。
- 不在本里程碑顺手修现货 Java 已知缺陷（除非另开任务）。

---

## 2. 架构

### 2.1 Crate 职责（双壳规则不变）

```text
match-protocol   — 共享 DTO + contract_* / spot_* 边界
match-core       — 订单簿 + 限价/市价/撤单（合约高级表单现货入站不用）
match-contract   — 合约进程（已有）
match-spot       — 现货进程（本里程碑）
match-replay     — golden / 对打（扩展现货夹具）
match-core-hp    — 可选实验引擎（切流后、feature）
```

| Crate | 现货工作 | 禁止 |
|-------|----------|------|
| `match-protocol` | 现货常量、`check_mq_order_spot`、`type_convert_spot`、深度批量 | 持有簿状态 |
| `match-core` | 仅当 L1 与 `bf-match` 有行为漂移时改 | 感知现货 Topic / Redis / RPC |
| `match-spot` | 配置、MQ、bootstrap、worker、出站、Redis、恢复、健康检查 | 私自改撮合规则 |
| `match-replay` | 现货 golden 包 + `bf-match` 导出说明 | 接生产出站 |

### 2.2 进程内数据流（现货）

```text
RMQ 分片 Topic（contract_match_order | contract_match_order_main_coin[_N]）
  → 解析 JSON → check_mq_order_spot → type_convert_spot
  → 按 symbolKey 路由 → 每币队列 → 单 worker
  → match-core::Engine（入站仅限价/市价/撤单）
  → 成交 / 簿变更 / 撤单
  → Producer：push_order_{symbol}、行情/深度 Topic（现货命名）
  → Redis 错误队列 / 深度键（exchange_depth:*）
```

仍遵守 **每币单写者**。与合约差异：进程消费的是 **跨多币的分片 Topic**，再 demux 到每币队列（对齐 `bf-match` 的 `ORDER_QUEUE_MAP`）。

### 2.3 方案选择

| 方案 | 优点 | 缺点 |
|------|------|------|
| A. 复制 `match-contract` 只改 Topic 名 | 上手快 | 分片/RPC/校验全错，返工大 |
| B. 抽共享 shell crate + 瘦 bin | 长期 DRY | 先大拆已工作的合约壳 |
| **C. 模块适配拷贝进 `match-spot` + 协议分叉（推荐）** | 风险隔离；符合「双壳」；合约保持绿 | 短期有重复，可后再抽 |

**推荐 C。**

---

## 3. 协议对齐（现货）

### 3.1 Topic / Group

对齐 [现货合约MQ-Topic对照.md](../../../docs/现货合约MQ-Topic对照.md)。名为历史遗留，**仅现货使用**。

| 方向 | 现货（bf-match） | 合约（当前 Rust） |
|------|------------------|-------------------|
| 入站 | `contract_match_order`（及 `main_coin` 分片） | `usdt_contract_match_order_{symbol}` |
| 消费组 | `contract_match_group`（实现前对照 Java 精确串） | `usdt_contract_match_channel_one_group{symbol}` |
| 成交回传 | `contract_match_order_push_order_{symbol}` | `usdt_contract_match_order_push_order_{symbol}` |
| 深度 / 未成交 / 机器人 | 多为**无** symbol 后缀的全局 Topic（以 `bf-match` `BBConstants` 为准） | 按 symbol 的 `usdt_contract_match_market_push_*_{symbol}` |
| 新交易对 | `market_client_new_coin_market` | `usdt_market_add_new_coin` |

实现：现货名放在 `match-spot` 的 `mq/topics.rs` 或 `match-protocol::spot_topics`，**不要**重载合约 helper。

### 3.2 校验 / 转换

| 规则 | 现货 | 合约（当前） |
|------|------|--------------|
| owner `type` | `{1,2,3}`（以 `bf-match` 为准） | `{1..8}` |
| 表单 | 仅限价 + 市价 | 另含 PostOnly / IOC / FOK |
| 合约字段 | 入站不强制 | `type_convert` 强制 |
| 市价 `trust_price ≤ 0` | 允许（Java 在引擎内改买价） | `type_convert` 拒绝 |

API 示意：

```rust
pub fn check_mq_order_spot(mq: &MqOrder) -> bool { ... }
pub fn type_convert_spot(mq: &MqOrder) -> Option<BbOrder> { ... }
```

`BbOrder` 可保留合约字段槽位，但现货 convert **不得**捏造影响成交的保证金/杠杆语义；默认值仅对齐 `bf-match` 的 `buildMqOrder`。

### 3.3 深度 / 批量常量

现货 Java 档位与合约 Rust 常量（`NO_DEAL_NUMBER=20`、`SEND_MAX_DATA=10`）不同。引入 **现货专用常量**（如 `SPOT_NO_DEAL_NUMBER`），仅 `match-spot` outbound 使用；数值以 `bf-match` / `BBConstants` 为准。

### 3.4 Redis

| 现货 | 合约 |
|------|------|
| `exchange_depth:{symbol}…` | `contract_exchange_depth:…` |

错误队列 / link key 思路可参照合约，**前缀必须现货**。

### 3.5 恢复 RPC

合约现状：

- 行情：`/contract-market/contractcoinMarketList`
- 委托：`/contract/entrust-list`

现货必须从 `bf-match` `InitLoadData` 反查 Feign 路径；**禁止**复用合约 URL。精确路径写入实现计划（M5.0 考古）。

Bootstrap 按本实例配置的 **`mainStream` / 分片 id** 过滤交易对。

### 3.6 启动去重

合约壳有 `START_QUEUE` / `BigNo` / 720s。现货 Java 默认无此模型。**默认不做**；若考古发现有，再补，并在 bootstrap 旁注释决策。

---

## 4. `match-spot` 壳层模块

布局对齐 `match-contract`，替换现货专用部分：

```text
crates/match-spot/
├── Cargo.toml
└── src/
    ├── main.rs
    ├── config.rs       # shard/mainStream、RPC、redis、transport
    ├── bootstrap.rs
    ├── inbound.rs      # 现货 Topic → validate/convert_spot → 队列
    ├── outbound.rs
    ├── symbol_worker.rs
    ├── redis_store.rs  # exchange_depth: 前缀
    ├── rpc/
    ├── mq/
    ├── health.rs
    └── telemetry.rs
```

### 4.1 配置示意

```yaml
shard:
  main_stream: 0
transport: memory
rpc:
  market_base: ...
  order_base: ...
redis: {}
```

### 4.2 引擎接线

- 默认仅 `match-core`。
- 可选 feature `hp-engine`：与合约相同双轨规则，切流不得默认。

### 4.3 MQ

复用合约 `mq/traits` + memory 的**模式**（拷贝或日后抽共享）。真 RocketMQ 与合约共轨；未接通前不宣称生产切流。

---

## 5. 等价与验收

### 5.1 L1 — 撮核行为（对 `bf-match`）

- 限价交叉 + 时间优先
- 部分成交 / 余量
- 撤单
- 市价买卖含 **入站价格为 0** 路径
- 现货验收 **不含** PostOnly/IOC/FOK（入站应拒或不出现）

### 5.2 L2 — golden / replay

- 从 `bf-match` 测试/录制导出
- `match-replay` 增加现货 profile
- 门禁：golden 包无未解释差异

### 5.3 L3 — 测试环境壳

- 恢复 → 入站 → 出站 → Redis 深度
- 单分片对打 / shadow 后再金丝雀

### 5.4 切流要点

1. 预热：Rust 起、不接生产消费；L2 绿；恢复演练通过。
2. **按分片**切：停该分片 Java 消费 → 排空或短暂停双活 → Rust 恢复并订阅 → 盯成交/深度/错误队列。
3. 扩分片；保留 Java 包回滚。
4. 回滚触发：出站错误率、深度空白、与订单中心对账失败。

---

## 6. 工作分解

| 阶段 | 交付 | 依赖 |
|------|------|------|
| M5.0 | 本设计评审通过；`bf-match` Topic/Group/RPC 考古笔记 | — |
| M5.1 | `match-protocol` 现货边界 + 单测 | M5.0 |
| M5.2 | 现货 L1 用例 | M5.1 |
| M5.3 | `match-spot` 二进制：配置、健康、memory MQ、worker、Topic/出站/Redis | M5.1 |
| M5.4 | Bootstrap + 现货恢复 RPC | M5.3 + 考古 |
| M5.5 | 现货 L2 golden + replay profile | M5.2 |
| M5.6 | 现货切流 runbook（`docs/cutover-runbook-spot.md`） | M5.4–M5.5 |
| M5.7 | 真 RocketMQ（与合约共轨） | 另轨 |
| M5.8（可选） | user/mm 双消费进同一币队列 | [Topic 拆分方案](../../../docs/现货撮合Topic拆分分片方案.md) |
| M5.9（可选） | spot 壳挂 `hp-engine` | L2 绿之后 |

---

## 7. 风险与待决

| 风险 | 缓解 |
|------|------|
| `contract_match_*` 被当成永续 | 文档/注释标明「仅现货」；模块分离 |
| 深度 Topic 误抄合约 | 出站 PR 前对照 `BBConstants` |
| convert 默认值改变成交 | golden；仅对齐 Java 默认 |
| 市价零价与 core 漂移 | 专用 L1；仅确认差异后改 core |
| 同分片双活拆簿 | 运维清单：单活消费组 |
| RocketMQ 延期 | 先 memory + L1/L2；M5.7 前不宣称生产 |

**M5.0 待决：**

1. `bf-match` 精确消费组串与全部深度/机器人 Topic 名  
2. 恢复 HTTP 路径与分页字段  
3. 线上实际出现的 `TYPES` / 表单枚举  
4. 现货是否存在类 START_QUEUE 去重  

---

## 8. 已否决

1. 分叉 `match-core-spot`  
2. 在合约 `check_mq_order` 上打补丁硬撑现货  
3. HP 优先切现货  
4. 把现货改成每 symbol 入站 Topic（改生产命名，超范围）

---

**详细设计：** [`2026-07-27-spot-match-shell-design.zh-CN.md`](./2026-07-27-spot-match-shell-design.zh-CN.md)。

**Greenfield 0→1 独立方案（忽略既有 Rust/C++）：** [`2026-07-27-spot-match-greenfield.zh-CN.md`](./2026-07-27-spot-match-greenfield.zh-CN.md)。

---

## 10. 现货 Java 模块对照（bf-match）

基线包：`bf-match/bztex-match-server-provider`。与 Rust 的映射关系如下。

### 10.1 进程拓扑差异（核心）

```text
合约 match-contract（现状）                现货 bf-match（目标）
─────────────────────────                ─────────────────────
每 symbol 一个入站 Topic                  每分片 1～2 个入站 Topic（全币混流）
  usdt_contract_match_order_{sym}           contract_match_order
                                            contract_match_order_mm（做市）
每 symbol 一个 ConsumerGroup              全分片共用 contract_match_group
demux：Topic 已按币隔离                   demux：JSON 内 symbolKey → ORDER_QUEUE_MAP
深度 Topic 带 symbol 后缀                 深度 Topic 全局（payload 带 symbolKey）
恢复：contract 行情 + 合约委托             恢复：coinMarkets + getEntrustList(mainStream)
启动去重：START_QUEUE / BigNo             无（现货 InitLoadData 未实现）
```

### 10.2 Java 类 → Rust 模块映射

| bf-match（Java） | 职责 | Rust 落点 | 改造要点 |
|------------------|------|-----------|----------|
| `BBConstants` | Topic 常量、`checkMqOrder`、`typeConvert` | `match-protocol::spot::*` | 现货专用校验/转换；TYPES `{1,2,3}`；市价允许 `price≤0` |
| `Constants` | `NO_DEAL=25`、`SEND_MAX_DATA=1` 等 | `match-protocol::spot_constants` | 与合约 `constants.rs` 数值分叉 |
| `BaseConsumer` | 普通单入站 → 校验 → 入队 | `match-spot::inbound` + `shard_consumer` | 消费 `contract_match_order`；解析 **List&lt;MqOrder&gt;** |
| `BaseMarketConsumer` | 做市单入站（Topic 后缀 `_mm`） | `match-spot::inbound::mm_consumer` | 同 `handleMqData`，Topic 不同 |
| `InitLoadData` | 启动：拉交易对、建队列/worker、恢复 | `match-spot::bootstrap` | `coinMarkets()` + `mainStream` 过滤；**无** StartQueue |
| `AddCoinMarketConsumer` | 广播上新交易对 | `match-spot::mq::new_market` | Topic `market_client_new_coin_market`；BROADCAST |
| `EventOrderHandler` | 买/卖分发 + 深度节流 | `match-spot::symbol_worker` + `outbound` | 深度 `interval.ms` 默认 50 |
| `BuyHandler` / `SellHandler` | 限价/市价/撤单撮合 | **`match-core::Engine`** | 逻辑已移植；现货只走 limit/market/cancel |
| `RatherThanHandler` 等 | 成交细节 | **`match-core`** 内部 | 不复制到 spot 壳 |
| `OrderProducer` | 成交/撤单回 order | `match-spot::outbound::push_order` | Topic 前缀 + 可选 `_mm` 后缀 |
| `MarketProducer` | 委托变更推 market | `match-spot::outbound::push_entrust` | `contract_match_market_entrust_order_{sym}` |
| `NoDealProducer` | 盘口快照 | `match-spot::outbound::push_no_deal` | **全局** Topic `contract_match_market_push_no_deal` |
| `DepthMapProducer` | 深度图 + Redis | `match-spot::outbound::deeps` + `redis_store` | 全局 Topic + `exchange_depth:{sym}*` |
| `SendErrorData` | 失败 MQ 重试 | `match-spot::error_queue` | Redis key `poc_redis_send_mq_error_data_queue` |

---

## 11. Rust 模块架构（match-spot）

### 11.1 Crate 依赖图

```text
                    ┌─────────────────┐
                    │   match-spot    │  bin: match-spot
                    │   (进程壳)       │
                    └────────┬────────┘
         ┌───────────────────┼───────────────────┐
         ▼                   ▼                   ▼
  match-protocol         match-core          match-replay
  (spot 边界)            (Engine)            (L2 golden)
         │
         └── contract 路径不动（check_mq_order / type_convert 保留）
```

### 11.2 match-spot 内部模块

```text
crates/match-spot/src/
├── main.rs                 # tokio 入口、配置加载、bootstrap::run
├── config.rs               # shard.main_stream、RPC、redis、queue_capacity、depth.interval
├── bootstrap.rs            # 对齐 InitLoadData.initMain + initData
│                           # coinMarkets → 过滤 mainStream → 建队列/worker → 恢复买卖委托
├── inbound/
│   mod.rs                  # handle_mq_data（对齐 BaseConsumer.handleMqData）
│   router.rs               # symbolKey → mpsc::Sender<BbOrder>
│   shard_consumer.rs       # 分片 Topic 订阅（非 per-symbol）
│   mm_consumer.rs          # contract_match_order_mm（可选 M5.8）
│   new_market.rs           # AddCoinMarketConsumer：运行时加币
├── symbol_worker.rs        # 单币单线程：Engine.on_order → outbound
├── outbound/
│   mod.rs                  # 事件分发、深度节流（EventOrderHandler.throttledDepthPush）
│   push_order.rs           # OrderProducer
│   push_entrust.rs         # MarketProducer（限价非市价时）
│   push_no_deal.rs         # NoDealProducer（NO_DEAL=25）
│   push_deeps.rs           # DepthMapProducer（DEEPS=30, ROBOT=50）
│   depth_agg.rs            # 同价合并档位（NoDealProducer.getDepth）
├── redis_store.rs          # exchange_depth: / link key / error queue
├── rpc/
│   market.rs               # CoinMarketRemoteServiceClient.coinMarkets()
│   order.rs                  # EntrustRemoteServiceClient.getEntrustList(type, mainStream, page)
│   response.rs
├── mq/
│   topics.rs               # 现货 BBConstants Topic 串（禁止复用 contract topics.rs）
│   traits.rs               # MessageSource / OrderSink（拷贝合约模式）
│   memory.rs
│   consumer.rs             # 按分片注册订阅（非 subscriptions_for_symbols）
│   producer.rs
├── error_queue.rs          # SendErrorData
├── health.rs
└── telemetry.rs            # 对齐 MatchTelemetry / MatchMetricsRecorder
```

### 11.3 运行时数据流

```mermaid
flowchart TB
  subgraph ingress [入站]
    RMQ1[contract_match_order]
    RMQ2[contract_match_order_mm]
    NM[market_client_new_coin_market]
  end

  subgraph spot_shell [match-spot]
    IN[inbound::handle_mq_data]
    RT[InboundRouter symbolKey demux]
    Q1[per-symbol mpsc queue]
    W[symbol_worker + match-core Engine]
    OB[outbound fills / depth / entrust]
  end

  subgraph egress [出站]
    PO[push_order_{symbol}]
    PM[market_entrust_{symbol}]
    ND[market_push_no_deal 全局]
    DP[market_push_deeps / robot 全局]
    RD[(Redis exchange_depth:*)]
  end

  RMQ1 --> IN
  RMQ2 --> IN
  IN --> RT --> Q1 --> W --> OB
  OB --> PO & PM & ND & DP & RD
  NM --> bootstrap
```

### 11.4 与 match-contract 壳的差异（实现时禁止混用）

| 模块 | match-contract | match-spot |
|------|----------------|------------|
| `bootstrap` | `StartQueueState` + TTL 清 BigNo | **删除**；恢复后直接入队 |
| `inbound` | `check_mq_order` + dedupe | `check_mq_order_spot`；**无 dedupe** |
| `mq/consumer` | `subscriptions_for_symbols` | **按分片** 1～2 个 Subscription |
| `mq/topics` | `usdt_contract_*` | `contract_match_*`（现货命名空间） |
| `outbound` 深度 | per-symbol Topic 后缀 | **全局 Topic**，body 含 `symbolKey` |
| `redis_store` | `contract_exchange_depth:` | `exchange_depth:` |
| `rpc/market` | `/contract-market/contractcoinMarketList` | **`coinMarkets()`** |
| `rpc/order` | `/contract/entrust-list` | **`getEntrustList`** + `mainStream` |

---

## 12. 详细改造点（按层）

### 12.1 match-protocol（M5.1）

**新增文件（建议）：**

```text
crates/match-protocol/src/
├── spot_constants.rs     # NO_DEAL=25, SEND_MAX_DATA=1, DEEPS=30, ROBOT=50
├── spot_topics.rs        # BBConstants 中 MQ_* 字符串
├── spot_validate.rs      # check_mq_order_spot
└── spot_convert.rs       # type_convert_spot
```

**对照 bf-match `BBConstants.checkMqOrder` / `typeConvert`：**

| 规则 | bf-match | 当前 Rust `validate.rs` | 现货改造 |
|------|----------|-------------------------|----------|
| `type` ∈ TYPES | `{1,2,3}` | `{1..8}` | 现货 `SPOT_TYPES` |
| `orderForm` | 未显式枚举 3–5 | 允许 PostOnly/IOC/FOK | 现货仅 `{1,2}`；3–5 拒单 |
| `gear` 市价必填 | 是 | 是 | 保持 |
| `trustPrice` 空 | 不允许空串 | 不允许空串 | 保持 |
| `trustPrice ≤ 0` | **市价允许**（form=2） | 一律拒绝 | `type_convert_spot` 放行 |
| `closePosition` 等 | **不要求** | **必填** | 现货校验 **删除** 合约字段 |
| `uid` | 可选 | convert 必填 | 现货 restore 有则填；convert 对齐 Java NPE 行为 |

**`type_convert_spot` 关键逻辑（对齐 Java 136–163 行）：**

- `symbolKey`：`replace("/", "").toLowerCase()`
- `gear`：`None → 0`（Java `gear == null ? 0 : gear`）
- `trustNumber > 0` 必填
- `trustPrice > 0` **或** `orderForm == MARKET(2)`
- 合约字段：`close_position` / `start_deposit` / `taker_rate` / `position_type` / `lever_times` 用 Java 等价默认（restore 路径从 `EntrustListBO` 映射，MQ 路径缺失时不捏造影响成交的值）

**Topic 常量（对齐 `BBConstants` 40–75 行）：**

| 常量 | 值 |
|------|-----|
| 入站 | `contract_match_order` |
| 做市入站 | `contract_match_order_mm`（= 入站 + `_mm`） |
| Group | `contract_match_group` |
| 成交回传 | `contract_match_order_push_order_{encodedSymbol}` |
| 行情委托 | `contract_match_market_entrust_order_{encodedSymbol}` |
| 盘口 | `contract_match_market_push_no_deal`（**无后缀**） |
| 深度图 | `contract_match_market_push_deeps` |
| 机器人 | `contract_match_market_push_robot` |
| 新币 | `market_client_new_coin_market` / group `market_client_new_coin_market_group` |

### 12.2 match-core（M5.2，按需）

**可直接复用（bf-match 已有 Rust 移植注释）：**

| bf-match | match-core |
|----------|------------|
| `BuyHandler` 限价交叉循环 | `handlers/limit_buy.rs` |
| `SellHandler` 限价交叉循环 | `handlers/limit_sell.rs` |
| 市价买 `trustPrice = MAX` | `match_market.rs` `MARKET_BUY_TRUST_PRICE = i32::MAX` |
| 市价卖 `trustPrice = 0` | `match_market.rs` sell path |
| `gear` 档位数停止 | `match_market.rs` `gear_of` |
| `SEND_MAX_DATA=1` 批量发 | outbound 层按事件逐条发即可 |

**需 L1 验证、可能微调：**

- 机器人撤单 `ORDER_ROBOT` 在 TreeSet 上的 `equals` 行为（Java 特殊 iterator 查找）
- 同价档位聚合深度（在 **outbound** 实现，不在 core）
- PostOnly/IOC/FOK：**现货入站不应到达**；core 保留即可

**明确不改：**

- 不为现货分叉 `match-core` crate
- 不把 HP 引擎设为现货默认

### 12.3 match-spot 壳层（M5.3–M5.4）

#### bootstrap（对齐 `InitLoadData`）

1. `startup_delay_ms`（Java `Thread.sleep(3000)`）
2. RPC `coinMarkets()` → 过滤 `mainStream == config.shard.main_stream`
3. 对每个 symbol：
   - Redis DEL `exchange_depth:{symbolKey}{detail|trade|paint}`
   - Redis DEL/SET link key `redis_poc_link_list_key{symbolKey}`
   - `router.register_queue(symbolKey, tx)`
   - `spawn_symbol_worker(...)`
4. 恢复：`getEntrustList(type=1)` 分页 → `handle_mq_data`；再 `type=2`
5. 启动 consumer：`contract_match_order`（+ 可选 `_mm`）
6. **不做** `start_queue_ttl` / BigNo 清理任务

#### inbound（对齐 `BaseConsumer.handleMqData`）

```rust
// 伪代码 — 对齐 Java 54–83 行
fn handle_mq_data(mq: MqOrder, router: &InboundRouter) {
    if !check_mq_order_spot(&mq) { record_invalid; return; }
    let Some(order) = type_convert_spot(&mq) else { record_invalid; return; };
    let Some(tx) = router.queue(&order.symbol_key) else { record_symbol_not_found; return; };
    tx.send(order).await; // Java BlockingQueue.put
}
```

- MQ body：Java 解析为 **`List<MqOrder>`**（`BaseConsumer.process` 30–31 行），Rust consumer 需同样支持数组 JSON
- **无** `StartQueueState::should_dedupe`

#### symbol_worker（对齐 `EventOrderHandler` + worker 线程）

- 单 task 从 `mpsc` 取 `BbOrder` → `Engine::on_order`
- 每笔完成后调用 `outbound.on_match_events` + 深度节流
- 深度节流：`depth_push_interval_ms` 默认 **50**（Java `@Value` 49 行）

#### outbound（对齐 Producer 链）

| 事件 | Java 触发 | Rust |
|------|-----------|------|
| 成交/撤单 | `BaseProducer` → `OrderProducer` | `push_order`；Topic 前缀 + encodeSymbolKey；做市单加 `_mm` |
| 限价挂单 | `MarketProducer` | `push_entrust`（市价单不发，BuyHandler 59–61 行） |
| 盘口 | `NoDealProducer` | 全局 Topic；`NO_DEAL=25`；同价合并 |
| 深度图 | `DepthMapProducer` | 全局 deeps + Redis `exchange_depth:` |
| 发送失败 | Redis error queue | `error_queue.rs` |

#### rpc（对齐 Feign）

| Java Client | 方法 | Rust |
|-------------|------|------|
| `CoinMarketRemoteServiceClient` | `coinMarkets()` | `rpc/market.rs` |
| `EntrustRemoteServiceClient` | `getEntrustList(EntrustBO)` | `rpc/order.rs` |

`buildMqOrder(EntrustListBO)` 字段映射（`InitLoadData` 187–204 行）→ Rust `build_mq_order_spot`：

- `priceType` → `orderForm`
- `userType` → `type`
- `remainAmount` → `trustNumber`
- `price` → `trustPrice`
- `entrustNo` → `trustOrderNo`

### 12.4 match-replay（M5.5）

- Golden 来源：`bf-match` 单测 / 录制，**禁止**用合约 golden
- Profile：`--engine spot` 走 `type_convert_spot`
- 对比项：成交价量、余量、撤单状态、深度档位（`NO_DEAL=25`）

### 12.5 可选后续（M5.8 user/mm 拆分）

对齐 [现货撮合Topic拆分分片方案.md](../../../docs/现货撮合Topic拆分分片方案.md)：

- 入站从 1 个 Topic 扩到 user/mm 两个 Consumer
- 两路都调同一个 `handle_mq_data` → 同一 `ORDER_QUEUE_MAP`
- **不改** per-symbol 单 worker 规则

---

## 13. 配置示例（完整）

```yaml
# config.spot.example.yaml
shard:
  main_stream: 0          # 0=contract_match_order; 1..11=contract_match_order_main_coin_N

startup_delay_ms: 3000
depth_push_interval_ms: 50

match:
  queue_capacity: 4096
  scales: {}              # 同 contract：symbol → (price_scale, qty_scale)

rpc:
  market_base_url: "http://market-service"
  order_base_url: "http://order-center"

redis:
  cluster_nodes: ["redis://127.0.0.1:6379"]

rocketmq:
  transport: memory       # 生产前换 rocketmq（M5.7）
  consumer_group: contract_match_group
  enable_mm_consumer: true   # contract_match_order_mm

telemetry:
  enabled: true
```

---

## 14. 验收检查表

| # | 项 | 对照 |
|---|-----|------|
| 1 | 入站拒单规则 | `BBConstants.checkMqOrder` 全分支 |
| 2 | 市价零价 | `typeConvert` 159–162 行 + `BuyHandler` 48 行 MAX |
| 3 | 恢复分页 | `getEntrustList` buy/sell 各扫完 |
| 4 | 深度 Topic 无 symbol 后缀 | `MQ_PRODUCER_MATCH_MARKET_PUSH_NO_DEAL_TOPIC` |
| 5 | Redis 前缀 | `exchange_depth:` 非 `contract_exchange_depth:` |
| 6 | 同价深度合并 | `NoDealProducer.getDepth` 99–112 行 |
| 7 | 成交批量 | `SEND_MAX_DATA=1` |
| 8 | 无 START_QUEUE | 现货 `InitLoadData` 无 BigNo |
| 9 | 新币广播 | `AddCoinMarketConsumer` 加队列不重启 |
| 10 | L2 golden | bf-match 导出 trace 零差异 |

---
