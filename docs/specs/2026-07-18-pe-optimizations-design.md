# 借鉴 perpetual_exchange 的分阶段优化设计

**日期：** 2026-07-18  
**状态：** Approved / Implemented (A–C skeleton) — 2026-07-18  
**前置：** [2026-07-18-match-core-hp-design.md](./2026-07-18-match-core-hp-design.md)  
**参考代码：** external `crypto-exchange` (perpetual_exchange R&D); this repository (`match-rust`)  

**决策摘要：** 顺序 **A → B → C**；Phase A 选 **A2**；落地方式选 **方案 1**（`LevelIndex` trait + feature `art`）

---

## 0. 决策摘要

| 项 | 选择 |
|----|------|
| 顺序 | Phase A（L1）→ Phase B（L2/L3）→ Phase C（异步 WAL） |
| Phase A 范围 | 最优价缓存 + Level 池化 + **可选 ART**（feature `art`） |
| 默认簿索引 | 仍为 `BTreeMap`（可回退、默认可测） |
| ART | `--features art`；与默认路径同序列 fill 数必须一致 |
| SIMD | **不移植** C++ PnL/虚高路径；ART Node16 SIMD 仅作 art 内可选微优化，非 A 必做 |
| 生产默认 | `match-contract` 仍默认 `match-core`；hp / art / wal 均为实验开关 |
| 基准纪律 | 凡微基准必须 `fill_rate > 0`（`fair_compare` 协议） |

---

## 1. 背景与目标

`perpetual_exchange`（`crypto-exchange`）提供了内存池、ART、SIMD、异步落盘等优化叙事；`match-core-hp` 已具备定点、价位簿、SPSC、伪共享隔离等。本设计把**可借鉴且可验证**的部分按层落地，避免把 L1 再抠几个 ns 当成端到端胜利（见 `match-rust/docs/e2e-budget.md`）。

### 1.1 目标

1. **A**：L1 更贴机械同情（最优价 O(1)、Level 复用、可选 ART）。  
2. **B**：contract 实验路径可走 `HpWorker` + adapter，并打出 L3/L2/L1 分段延迟。  
3. **C**：异步批写 WAL，对照 C++ persistence 数量级，且不阻塞撮核热路径。

### 1.2 非目标

- 不替换生产默认引擎；不接真实 RocketMQ。  
- 不移植账户 / 强平 / 限流 / 完整 ART+SIMD 引擎外壳。  
- 不做 Aeron IPC（另项）。  
- 不把 0% fill 的 ART+SIMD「峰值」写入有效对比表。

---

## 2. 架构落点

```text
match-rust/crates/
├── match-core/          # 等价轨（不变）
├── match-core-hp/       # Phase A：LevelIndex / best_* / level pool / feature art
├── match-bench/         # fair_compare + art on/off 对照
├── match-contract/      # Phase B：feature hp-engine + spans
├── match-wal/           # Phase C：异步批写（新建小 crate）
└── match-protocol/      # 不变
```

| 组件 | 职责 | 禁止 |
|------|------|------|
| `LevelIndex` | tick → Level 的插入/删除/最优价迭代 | 热路径分配字符串 / BigDecimal |
| `art` feature | ART 实现替换默认 BTree 索引 | 改变撮合语义 |
| `hp-engine` feature | contract worker 可选走 hp | 默认打开；默认依赖仍 core |
| `match-wal` | 缓冲 + 后台 flush 成交/订单日志 | 同步 fsync 挡在 `on_order` 内（异步模式） |

---

## 3. Phase A — L1（match-core-hp）

### 3.1 最优价缓存

在 `Book` 上维护：

- `best_bid_tick: Option<i64>`
- `best_ask_tick: Option<i64>`

维护规则：

- 档位从空→非空、或更优价出现：更新缓存。  
- 最优档被撤空 / 吃空：从索引取下一最优（或 `None`）。  
- `best_bid()` / `best_ask()` 热路径读缓存；debug assert 可与索引核对。

### 3.2 Level 池化

- `OrderStore` 已有 slot free-list；本期重点：**空 `Level`（含 `VecDeque`）回收复用**，减少档位创建/销毁分配。  
- 池大小有上限；超出则丢弃空 Level 让分配器回收。

### 3.3 `LevelIndex` + feature `art`

```text
trait LevelIndex {
    fn get_mut(&mut self, tick: i64) -> &mut Level;
    fn remove_if_empty(&mut self, tick: i64);
    fn best_tick(&self) -> Option<i64>;   // asks: min；bids: max
    fn next_after_best(&self) -> Option<i64>; // 用于缓存失效后重算
    // + 深度遍历所需的有序迭代（前 N 档）
}
```

- 默认：`BTreeLevelIndex`（今日 `BTreeMap` 行为）。  
- `art`：`ArtLevelIndex`，key = 大端/可比较字节化的 `i64` tick（实现可自研精简 ART，或评估成熟 crate；**语义层不暴露 crate 细节**）。  
- `Book` 对 bids/asks 各持一个 `LevelIndex`（bids 侧用「更高价更优」的比较约定，可在包装层处理）。

**SIMD：** 不在 A 的验收范围内。若 art 实现内对 Node16 查找使用 SIMD，须仍通过同序列 fill 一致性测试；不得单独宣传无 fill 的吞吐。

### 3.4 验收（A）

| 项 | 标准 |
|----|------|
| 正确性 | 现有 hp 集成测试全绿 |
| 公平基准 | `fair_compare --n 50000` → fill_rate≈0.5，exit 0 |
| art 一致性 | 同 workload，默认 vs `--features art` 的 `n_fills` 相同 |
| 性能 | 相对 A 前基线：`fair_cross` 不显著回归；缓存命中路径有文档数字 |

---

## 4. Phase B — L2/L3（match-contract）

### 4.1 Feature `hp-engine`

- 关闭（默认）：行为与今相同（`match-core`）。  
- 开启：symbol worker 入站经 `adapter` → `HpCommand` → `HpWorker`（或同线程 `HpEngine`，由配置选）。  
- `match-contract` **不得**在默认依赖图中强制拉起 art。

### 4.2 分段计时

在单订单路径打点（`Instant` 或 tracing span）：

| Span | 含义 |
|------|------|
| `L3_adapt` | BbOrder / JSON 边界 → HpCommand |
| `L2_queue` | 入队→worker 取出 |
| `L1_on_order` | `HpEngine::on_order` |

结果回填 `match-rust/docs/e2e-budget.md` 实测列（模板已有）。

### 4.3 验收（B）

- feature 关：现有 contract 单测 / 启动路径不破坏。  
- feature 开：memory transport 下可跑通挂/吃/撤；日志或 metrics 可见三段延迟。  
- 仍不要求真实 NameServer。

---

## 5. Phase C — 异步 WAL（match-wal）

### 5.1 模型（借鉴 C++ persistence buffer）

```text
撮核线程 --append(record)--> 无锁/SPSC 缓冲 --后台线程--> 批量 write(+可选 fsync)
```

- 记录类型：最小集 `OrderAccepted` / `Fill` / `Cancel`（二进制或长度前缀，一期可用 bincode/手工布局）。  
- 模式：`Async`（默认实验）与可选 `Sync`（测试正确性）。  
- 缓冲满：背压策略为 `Busy`/阻塞 append（文档写清），**禁止静默丢日志**（除非显式 `BestEffort` 配置，默认关闭）。

### 5.2 挂接

- 仅 hp 实验路径可选挂载；core 默认路径不强制。  
- WAL 失败：metrics + 日志；Async 模式不回滚已撮合结果（与「先撮后记」一致，需在运维文档标明）。

### 5.3 验收（C）

- 独立微基准：记录/秒、平均 flush 延迟（对照 `crypto-exchange` persistence 量级作参考，不混入 L1 CSV）。  
- Async：`on_order` 热路径无同步磁盘 wait（可用 hook/计数断言）。  
- 重启回放**不在本期**（只写前、不建完整 event-sourcing 回放）。

---

## 6. 风险与缓解

| 风险 | 缓解 |
|------|------|
| ART 实现错误导致档位顺序错 | 同序列 fill 对比 + 深度快照对比测试 |
| 最优价缓存不同步 | debug assert；单测专门打撤最优/吃空档 |
| hp-engine 误开上生产 | feature 默认关；README 警告 |
| WAL 背压拖垮撮合 | 缓冲 sizing + metrics；压力测试文档 |
| 对标 C++ 虚高数字 | 强制 fair_compare 纪律 |

---

## 7. 文档与基准更新

- `match-rust/docs/best-practices.md`：增加 perpetual_exchange → 本仓库映射行。  
- `match-rust/docs/bench-results.md`：A 完成后追加缓存/art 数字。  
- `match-rust/docs/e2e-budget.md`：B 完成后填实测。  
- `match-rust/docs/fair-compare.md`：注明 art feature 对照用法。

---

## 8. 实施顺序（计划阶段细化）

1. A1 最优价缓存 + 测试  
2. A1 Level 池 + 测试  
3. A2 `LevelIndex` 抽象（默认 BTree，行为不变）  
4. A2 ART 实现 + feature + 一致性测试  
5. B `hp-engine` + spans  
6. C `match-wal` + 微基准  

每步可独立提交；A 完成前不开始 B 的生产接线（可并行写 wal crate 骨架，但不挂 contract）。

---

## 9. 开放项（实现计划阶段关闭）

- ART：自研精简 vs 外部 crate（以许可证、`i64` key、有序最小/最大支持为准）。  
- WAL 记录编码：手工固定布局 vs bincode。  
- `hp-engine` 下出站事件是否先转回 Java 形 JSON（建议：是，保持 Topic 兼容；转换放 L2′）。
