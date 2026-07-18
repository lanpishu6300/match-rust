# match-core-hp 极致低延迟双轨设计

**日期：** 2026-07-18  
**状态：** Implemented (H0–H3) — 2026-07-18；生产默认仍为 `match-core`  
**前置：** [2026-07-17-rust-match-engines-design.md](./2026-07-17-rust-match-engines-design.md)（等价轨）  
**代码根：** this repository (`match-rust`)

---

## 0. 决策摘要

| 项 | 选择 |
|----|------|
| 策略 | **双轨 C**：保留 `match-core` 等价轨；新建 `match-core-hp` 做极致优化 |
| 落地 | **独立 crate**（非 feature 缠绕、非过早统一 trait） |
| 生产 | `match-contract` **默认仍用 `match-core`**；一期不切 hp |
| 业界对齐 | LMAX 单写者 + 预分配；机械同情；价位簿；`i64` 定点 |
| 语义 | hp 使用**干净**限价/市价/撤单语义；**不**复刻 Java quirk |

---

## 1. 背景与目标

等价轨 `match-core` 以 Java 可观测等价为目标（`BigDecimal`、`BTreeSet`、Handler 控制流），明确不做性能向重写。本设计增加实验轨，吸收业界低延迟撮合/消息内核的共性理念，用可测数字证明收益，同时不破坏生产默认路径。

### 1.1 目标

1. 交付 `match-core-hp`：定点 + 价位簿 + 单写者热路径。  
2. 交付 `match-bench`：core vs hp 吞吐与延迟对比。  
3. 在文档中固定「采纳的理念 / 非目标 / 与等价轨差异」。

### 1.2 非目标（一期）

- 不替换生产默认引擎。  
- 不接真实 RocketMQ / 不改现网 Topic。  
- 不复刻 PostOnly/IOC/FOK 的 Java 已知缺陷行为。  
- 不做完整 Aeron 级 IPC；同进程 SPSC 即可。

---

## 2. 架构与 crate 边界

```text
match-rust/crates/
├── match-core/       # 等价轨（冻结行为，生产默认）
├── match-core-hp/    # 新增：高性能撮核
├── match-bench/      # 新增：criterion / 对比基准
├── match-protocol/   # 共享 DTO；仅 adapter 边界使用
└── match-contract/   # 默认依赖 match-core
```

| Crate | 职责 | 禁止 |
|-------|------|------|
| `match-core-hp` | 定点订单、价位簿、预分配事件、可选 SPSC worker | Tokio/MQ/JSON；Java golden |
| `match-bench` | 同序列压测 core vs hp | 生产流量 |
| hp `adapter` 模块 | `BbOrder` ↔ tick/lot；可选 `HpEvent` → 十进制展示 | 热路径内 `BigDecimal`/字符串运算 |

### 2.1 业界理念映射

| 理念 | 来源 | 落点 |
|------|------|------|
| Single Writer | LMAX Disruptor | 每 symbol 一线程独占簿 |
| Preallocate | Disruptor / Aeron | 订单槽、Cmd/Event ring |
| Mechanical sympathy | Aeron 等 | 价位结构、少锁少分配 |
| Price-level book | 交易所撮核 | 档位 + 同价 FIFO |
| Fixed-point | 低延迟交易系统 | `i64` price_tick / qty_lot |

### 2.2 数据流

```text
生产:  MQ → match-core（不变）

实验:  逻辑订单序列
         ├─→ match-core::Engine
         └─→ adapter → match-core-hp::HpEngine
       match-bench 汇总吞吐 / p50 / p99
```

---

## 3. 数据模型与价位簿

### 3.1 定点

每 symbol 固定：

- `price_scale`：`tick = round(price * 10^price_scale)`  
- `qty_scale`：`lot = round(qty * 10^qty_scale)`  

热路径结构（逻辑形状）：

- `HpOrder`：`id`, `side`, `price_tick`, `qty_lot`, `open_lot`, `ts` — 无 `String` / `BigDecimal`  
- `HpEvent`：`Fill` / `Revoke`（原因用 `u8`）

Adapter 仅在边界做 scale 转换与溢出校验。

### 3.2 价位簿

```text
bids / asks: 按 tick 索引的 Level map（最优价可 O(1)/O(log 档位数)）
Level: total_lot + 同价 FIFO（订单 id 链表或 slot 索引）
orders: generational / 预分配 slot 数组（id → HpOrder）
```

| 操作 | 目标复杂度 |
|------|------------|
| 最优价 | O(1) 或 O(log 档位数) |
| 同价入出队 | O(1) |
| 撤单 | O(1) 定位 + 空档删除 |
| 吃单扫档 | 沿对手最优向下直到不交叉或量尽 |

### 3.3 单写者与队列

- 每 symbol：`HpWorker` 独占簿。  
- 入站：预分配 SPSC ring（长度 2^n）；满则 `Busy` 背压。  
- 出站：固定 cap 的 event buffer，批量 drain。  
- 热路径不使用 `Mutex` / `tokio::mpsc`。  
- Bench 可同线程直接调 `HpEngine::on_order` 测纯撮核。

### 3.4 一期功能

| 能力 | 一期 |
|------|------|
| 限价挂/吃、部成、价时优先 | ✅ |
| 撤单 | ✅ |
| 市价（吃尽或可选 max_levels） | ✅ |
| 深度前 N 档 | ✅ |
| PostOnly / IOC / FOK | 二期 |
| Java quirk | ❌ |

限价交叉（干净语义）：买可吃 `ask_tick <= bid_tick`；市价默认不强制 Java `gear=0` quirk。

---

## 4. 基准、验收与风险

### 4.1 场景

`rest_only` / `cross_full` / `partial_walk` / `cancel_hot` / `mixed`

### 4.2 指标

- 吞吐：orders/sec、fills/sec  
- 延迟：单次 `on_order` 的 p50 / p99 / p999（排除 MQ）  
- 可选：热路径堆分配抽样  

结果写入 `match-rust/docs/bench-results.md`（或 CI artifact）。

### 4.3 验收

| 项 | 标准 |
|----|------|
| 正确性 | hp 单测覆盖价时优先、部成、撤单、深度 |
| 回归 | 等价轨现有测试全绿 |
| 性能 | `cross_full` + `partial_walk` 上 hp 相对 core **≥ 5×** 吞吐（本机；未达则记瓶颈再迭代） |
| 隔离 | contract 默认不依赖 hp |
| 文档 | 本 spec + bench 说明 + 与等价轨差异表 |

### 4.4 风险

| 风险 | 缓解 |
|------|------|
| 语义漂移 | 共享逻辑场景向量；差异表写明不覆盖项 |
| 定点溢出 | adapter 校验 + 边界单测 |
| Ring 满 | 显式 Busy；bench 统计 |
| 误切生产 | 代码审查 + 无默认开关 |

### 4.5 里程碑

| 阶段 | 交付 |
|------|------|
| H0 | hp 骨架 + 定点 + 价位簿限价/撤单 + 单测 |
| H1 | 市价 + 深度 + adapter |
| H2 | match-bench 五场景 + 首份对比报告 |
| H3 | SPSC worker + 预分配 event buffer |
| H4 | 高级单；contract 显式 `engine=hp` 实验开关 |

---

## 5. 与等价轨差异（契约）

| | match-core | match-core-hp |
|--|------------|---------------|
| 数字 | `BigDecimal` | `i64` tick/lot |
| 簿 | `BTreeSet` 整单排序 | 价位档 + FIFO |
| 语义 | Java 等价（含 quirk） | 干净撮合语义 |
| I/O | 由 contract 接 MQ | 无；bench / 未来可选壳 |
| 生产默认 | ✅ | ❌ |

---

## 6. 已否决

1. 在 `match-core` 内用 feature 混等价/性能 — 回归与心智负担过大。  
2. 过早统一 `MatchingEngine` trait 且绑定十进制 API — 拖垮 hp 热路径。  
3. 以接生产 RMQ 为一期门槛 — 与「先证明撮核数字」优先级不符。

---

## 7. OSS 最佳实践索引（落地后）

完整映射见仓库内 [`match-rust/docs/best-practices.md`](../../../match-rust/docs/best-practices.md)（Disruptor / Aeron / Seastar / 撮核通识 → 模块路径与启用方式）。
