# Rust 合约/现货撮合引擎等价重构设计

**English：** [2026-07-17-rust-match-engines-design.md](./2026-07-17-rust-match-engines-design.md)

**日期：** 2026-07-17  
**状态：** Approved — 2026-07-17  
**基线代码：** `java-contract-match` / `java-spot-match`（Java main）  
**关联文档：** [合约撮合已知问题梳理.md](../../合约撮合已知问题梳理.md)、[现货撮合Topic拆分分片方案.md](../../现货撮合Topic拆分分片方案.md)

---

## 0. 决策摘要

| 项 | 选择 |
|----|------|
| 交付形态 | **整服务替换**：Rust 进程承接 MQ 入出、启动恢复、深度/成交推送；Topic/JSON **语义兼容**，可灰度替换 Java |
| 实施顺序 | **先合约** `java-contract-match`，核心复用后再做现货 `java-spot-match` |
| 等价验收 | **可观测结果严格一致**（成交价量、余量、撤单路径、深度档位）；允许 JSON 编码细节差异 |
| 周边能力 | **全量对齐 Java**：恢复 RPC、Redis、深度/机器人 Topic、错误重发、健康检查/指标 |
| 架构方案 | **共享撮核 + 双壳**（`match-core` + `match-contract` / `match-spot` + `match-replay`） |
| 缺陷策略 | 默认 **原样保留** Java main 行为（含已知缺陷）；修 bug 需单独任务 |

---

## 1. 背景与目标

### 1.1 现状

生产撮合为两套近亲 Java/Spring Boot + RocketMQ 服务：

- **现货：** `java-spot-match` — 内存 `TreeSet` 订单簿，按交易对单线程撮合，价时优先；Topic 命名含历史 `contract_match_*` 前缀与 mainstream 分片。
- **合约：** `java-contract-match` — 同构，另支持 PostOnly / IOC / FOK；Topic 为 `usdt_contract_match_*`，按 symbol 订阅；启动经 RPC 恢复挂单。

仓库内 `crypto-exchange`（C++）与 `clearing-match` 为实验/研发路径，**不是**现网替换目标。当前无 Rust 撮合代码。

### 1.2 目标

1. 用 Rust 实现与现网 **逻辑完全等价** 的合约撮合引擎进程，可按 symbol 灰度顶替 `java-contract-match`。
2. 抽出可复用的 `match-core`，二期交付现货 `match-spot` 整服务替换。
3. 用 `match-replay`（golden / 对打）锁死验收标准，避免「看起来像」的假等价。

### 1.3 非目标（一期）

- 不改 Topic 命名与下游消费契约语义。
- 不做性能导向的数据结构重写（内部实现可换，对外结果必须过 replay）。
- 不在本期修复 [合约撮合已知问题梳理](../../合约撮合已知问题梳理.md) 中的 P0/P1（除非另开任务）。
- 不在本期切现货生产流量。
- 不引入 JNI 中间态作为最终形态（曾评估方案 3，已否决）。

---

## 2. 架构与 crate 边界

仓库路径：本仓库根目录（Cargo workspace）。

```text
match-rust/
├── Cargo.toml
└── crates/
    ├── match-core/         # 纯逻辑：订单簿 + 撮合状态机
    ├── match-protocol/     # MqOrder/BBOrder 字段、校验、decimal 解析
    ├── match-contract/     # 合约可执行进程（bin）
    ├── match-spot/         # 现货 bin（一期 stub，二期填满）
    └── match-replay/       # 录制回放 / Java↔Rust diff
```

| Crate | 职责 | 禁止 |
|-------|------|------|
| `match-core` | 价时优先簿、限价/市价/撤单、PostOnly/IOC/FOK、产出成交与簿变更事件 | 感知 Topic / Redis / RPC |
| `match-protocol` | 与现网一致的 DTO、`checkMqOrder`、十进制字符串解析 | 持有撮合状态 |
| `match-contract` | RocketMQ、按 symbol 单线程队列、拉交易对、恢复挂单、Redis、出站 Producer、指标/健康检查 | 私自改撮合规则 |
| `match-spot` | 现货 Topic/分片与恢复（二期） | 复制一份分叉核心 |
| `match-replay` | 同输入跑 core，对比成交/余量/深度 | 接生产出站流量 |

### 2.1 进程内数据流

```text
RMQ usdt_contract_match_order_{symbol}
  → parse/validate (match-protocol)
  → per-symbol queue → single worker
  → match-core::Engine
  → fills / book updates / revoke
  → producers: push_order / push_market / no_deal / deeps / robot
  → Redis error queue on send fail
```

### 2.2 小数与排序

- 金额/数量使用与 Java `BigDecimal` **语义一致** 的十进制类型；入出站以字符串为主。
- 订单簿排序对齐 `BBOrder.compareTo`：买高→低、卖低→高；同价按 `createTime` 再 `trustOrderNo`。
- 比较与 golden 使用数值语义 + 约定 scale，禁止 IEEE float。

---

## 3. 数据流与协议对齐（合约一期）

### 3.1 RocketMQ

| 方向 | Topic / Group | 说明 |
|------|----------------|------|
| 入站 | `usdt_contract_match_order_{symbol}` | symbol 小写无 `/`；ORDERLY + CLUSTERING |
| Group | `usdt_contract_match_channel_one_group` | 同 group；**同一 symbol 同时只能有一个 active 引擎**（Java 或 Rust） |
| 新交易对 | `usdt_market_add_new_coin` / `usdt_market_add_new_coin_group` | 动态建队列、线程、订阅 |
| → 订单 | `usdt_contract_match_order_push_order_{encodedSymbol}` | 批大小对齐 `SEND_MAX_DATA=10` |
| → 行情成交 | `usdt_contract_match_market_push_order_{encodedSymbol}` | |
| 盘口 | `usdt_contract_match_market_push_no_deal_*` | |
| 深度 | `usdt_contract_match_market_push_deeps_*` | 深度档位 20 |
| 机器人 | `usdt_contract_match_market_push_robot` | 无 symbol 后缀 |

- 入站 body：JSON **数组** `List<MqOrder>`。
- 出站：字段语义对齐现有 Producer；编码细节允许差异（见 §4）。
- `encodedSymbol` 行为对齐 `CoinMarketEncode.encodeSymbolKey`。

### 3.2 入站模型与校验

对齐 `MqOrder` / `BBOrder` 字段，至少包括：

`userId`, `uid`, `cType`, `dealType`, `type`, `orderType`, `marketId`, `coinId`, `symbolKey`, `coinMarket`, `trustOrderNo`, `closePosition`, `startDeposit`, `positionType`, `takerRate`, `orderStatus`, `orderForm`, `gear`, `leverTimes`, `trustNumber`, `trustPrice`, `createTime`, `faceValue`, `handicapType`

- 校验对齐 `BBConstants.checkMqOrder`（含市价 `gear`、合约必填字段）。
- `orderForm`：`1` 限价、`2` 市价、`3` PostOnly、`4` IOC、`5` FOK。
- 入站接受的 `orderStatus`：`0/2/3`（与 Java `ORDER_STATUS` 列表一致）。

### 3.3 启动与恢复

顺序对齐 `InitLoadData`：

1. 延迟启动（现网约 10s）。
2. RPC/HTTP：`getAllContractCoinMarket`，过滤 `mainStream == SHARD(0)`。
3. 每 symbol：删除 Redis 深度相关 key 与 `redis_poc_link_list_key{symbol}`；建 `ORDER_QUEUE`；启动单线程 `take → onEvent`。
4. 分页 `getContractEntrustList` 恢复挂单 → `MqOrder` → 入队/入簿。
5. 维护 `START_QUEUE_MAP` + `BigNo`：启动窗口内丢弃已恢复单号及 `≤ BigNo` 的重复 MQ；约 720s 后清空（与 Java 一致）。
6. 注册 per-symbol consumer 后拉流。

Rust 通过 HTTP 调用现有 Feign 暴露的同一 REST 路径（从 `contract-order` / `contractmarket` RPC 反查），不依赖 JVM。

### 3.4 Redis

| Key / 用途 | 行为 |
|------------|------|
| `MATCH_KEY` + `redis_poc_link_list_key{symbol}` | 启动占位；存在则不重复起撮合线程 |
| `MARKET_KEY` + `contract_exchange_depth:{origin}{detail\|trade\|paint}` | 启动删除 |
| `poc_redis_send_mq_error_data_queue` | 发送失败入队；对齐 `SendErrorData` 重发 |

### 3.5 Handler 路由（契约）

```text
validate → typeConvert → (START_QUEUE / BigNo 去重) → ORDER_QUEUE[symbol]
  → EventOrderHandler
       ├─ form 1/2 → Buy/Sell (+ Market / RatherThan / Equals / LessThan)
       └─ form 3/4/5 → Height* (+ Fok*)
  → producers
```

单 symbol 单线程；跨 symbol 并行。异常与 ACK 行为默认对齐 Java（含已知「异常仍 ACK」等），一期不「顺便修」。

### 3.6 配置与观测

映射现有配置：RocketMQ NameServer、Redis、订单/行情 RPC base URL、`SHARD`、线程池大小、深度档位、盘口节流（main 已有则对齐）。指标名尽量对齐 `ContractMatchTelemetry` / `docs/opentelemetry-metrics.md`，便于复用看板。

---

## 4. 等价测试与灰度

### 4.1 等价定义（验收 B）

同输入序列下，以下必须一致：

| 维度 | 内容 |
|------|------|
| 成交 | taker/maker 单号、价、量、双方剩余、顺序 |
| 状态 | 部成/全成/撤成功；撤单触发路径与 Java 同源 |
| 订单簿 | 事件后买卖盘价位与挂单量（同价保留时间序） |
| 深度/盘口 | 档位数与各档价量；有节流时比节流后快照 |

**不强制：** 日志文案、指标时间戳、MQ msgId、JSON key 序、小数文本尾零/科学计数法。

Golden 期望值来自 **Java 参考跑数**，不是理想正确撮合。

### 4.2 `match-replay` 三层

- **L1 CI：** 手写限价交叉、同价时间优先、市价+gear、PostOnly、IOC、FOK、撤单、空簿 → 断言 `match-core` 事件流。
- **L2 CI 门禁：** 录制/合成 `MqOrder` 序列 → Java 导出 `GoldenTrace`（NDJSON）→ Rust 回放 diff；成交/余量/深度不一致则 fail。
- **L3 上线前：** 低流量 symbol 录制入站离线双跑，或 Rust 只读影子消费（**不生产**）比对簿状态。

### 4.3 灰度切换

约束：同一入站 Topic + 同一 consumer group，**同一时刻仅一个 active 引擎** 消费该 symbol。

1. **预热：** Rust 部署但不订生产；L2 全绿；测试环境跑通恢复/Redis/出站。
2. **按 symbol 切：** 停 Java 该 symbol 消费 → 队列排空或短暂双停 → Rust 等价恢复并订阅 → 观察成交/深度/错误队列。
3. **扩大至全量**；保留 Java 包快速回滚。
4. **回滚触发：** 出站错误率、深度长时间空白、与订单对账不一致、簿 remove 失败类指标显著差于基线。

部署建议：`match.engine.impl=java|rust`、`match.engine.symbols` 白名单。切流窗口必须保留 `START_QUEUE_MAP` / `BigNo` / 720s 逻辑。

### 4.4 现货二期

`match-spot` 复用 `match-core`，协议/Topic 对齐 `java-spot-match`（含历史命名与分片）。验收 L1–L3 与切流策略镜像合约。一期不切现货生产。

---

## 5. 里程碑

| 阶段 | 交付 |
|------|------|
| M0 | workspace + `match-protocol` + `match-core` 骨架 + L1 框架 |
| M1 | 合约限价/市价/撤单等价 + L2 golden 基线 |
| M2 | PostOnly/IOC/FOK + 深度/盘口推送逻辑 |
| M3 | `match-contract` 全量壳：MQ / Redis / 恢复 RPC / 指标 |
| M4 | L3 对打 + 单 symbol 灰度 + 回滚演练 |
| M5 | 现货 `match-spot`（二期） |

---

## 6. 风险与开放项

| 风险 | 缓解 |
|------|------|
| Java 已知缺陷被原样带入 | 文档标明；修缺陷另开任务与 golden 更新 |
| RocketMQ Rust 客户端与框架封装差异 | 消费模式/重试/ACK 用集成测试钉死；对照 `BaseConsumer` |
| 恢复 RPC 路径/鉴权与 Feign 不一致 | 从 rpc 模块反查 URL 与请求体；测试环境启动恢复用例 |
| 切流双消费导致簿分裂 | 运维清单强制单 active；自动化检查 group 在线实例 |
| `BigDecimal` 边界（scale、除法） | L2 覆盖市价拆单、FOK 回滚等热路径 |

**开放项：** 已在实现计划 [`docs/plans/2026-07-17-rust-match-engines.md`](../plans/2026-07-17-rust-match-engines.md) 闭合（`bigdecimal` / `redis` cluster / Apache RocketMQ Rust + ping 门禁；Golden 导出器在 `java-contract-match` JUnit；恢复 path：`/contract-market/contractcoinMarketList`、`/contract/entrust-list`）。

---

## 7. 已否决方案

1. **纯镜像逐文件移植（无共享 core）：** 现货二期易分叉，维护成本高。
2. **JNI 内核 + Java 壳长期共存：** 与整服务替换目标叠床架屋，最终仍要做双壳。
3. **以 C++ `crypto-exchange` 或 clearing-match 为生产基线：** 与现网协议/行为不对齐，等价成本更高。
