# 开源低延迟最佳实践 → match-rust 映射

本文总结交易所/消息/存储领域出名开源（及经典）项目的设计理念，并标明在本仓库中的落地位置。  
**生产默认仍是等价轨 `match-core`；性能实践集中在 `match-core-hp`。**

---

## 1. 实践总表

| 理念 | 代表项目 | 本仓库落地 | 状态 |
|------|----------|------------|------|
| Single Writer Principle | [LMAX Disruptor](https://lmax-exchange.github.io/disruptor/) | 每 symbol 一 `HpWorker` / 一 `HpEngine` 独占簿 | ✅ |
| 预分配环形缓冲 | Disruptor / [Aeron](https://github.com/real-logic/aeron) | `SpscRing` 固定 2^n 槽；`HpEngine` 事件 `Vec` 预留容量 | ✅ |
| 批量消费（batching） | Disruptor `BatchEventProcessor` | `SpscRing::pop_n` 一次 Acquire/Release 批量出队 | ✅ |
| 避免伪共享 | Aeron / Disruptor | `head`/`tail` 各占独立 cache line（`CachePadded`） | ✅ |
| Mechanical sympathy | Martin Thompson / Aeron | 定点 `i64`、价位簿、热路径无锁/无 JSON | ✅ |
| Share-nothing 分片 | [Seastar](https://github.com/scylladb/seastar) | 按 symbol 分片；跨片不共享可变状态 | ✅（架构） |
| 定点算术 | 主流撮核 / HFT | `SymbolScale` + tick/lot | ✅ |
| 价位簿 + 同价 FIFO | 交易所撮核通识 | `Book` + `Level` + `VecDeque<id>` | ✅ |
| 背压而非丢弃/扩容 | Disruptor / reactive streams | `try_submit` → `Busy` | ✅ |
| 等待策略可配置 | Aeron `IdleStrategy` | `WaitStrategy::{BusySpin,Yield}` + `poll` | ✅ |
| 观测与正确性分离 | 工程通识 | `match-core` golden；`match-bench` 测 hp | ✅ |
| CPU 绑核 | DPDK / Aeron 部署实践 | `affinity` 模块 + 运维说明（可选 `core_affinity`） | ✅ 文档/API |
| 零拷贝 IPC | Aeron Media Driver | 未做（同进程 SPSC 即可；跨进程二期） | ⏳ |
| 内核旁路网卡 | DPDK / io_uring | 运维层，非本 crate 范围 | ⏳ |
| 业务 quirk 复刻 | — | 刻意不做（干净语义轨） | N/A |

---

## 2. 分项目要点（浓缩）

### LMAX Disruptor
- **单一写者**改共享结构，读者用序列号协调。  
- **预先分配**整个 ring，运行期不 `new` 事件对象。  
- **批量处理**降低内存屏障与分支频率。  

→ 对应：`HpWorker` + `SpscRing` + `pop_n` + 预分配 `events`。

### Aeron
- **机械同情**：数据结构对齐缓存、减少伪共享、可控空转。  
- **IdleStrategy**：BusySpin / Yield / Sleeping 按延迟与 CPU 权衡。  

→ 对应：`CachePadded` 游标、`WaitStrategy`、`HpWorker::poll`。

### Seastar / Scylla
- **每核一 shard**，无跨核锁；消息用显式队列。  

→ 对应：每交易对独立引擎；禁止在热路径对簿加 `Mutex`。

### 交易所撮核通识
- **价格档 + FIFO**，而非全市场 `TreeSet` 整单比较。  
- **整数 tick/lot**，热路径不做任意精度十进制。  

→ 对应：`match-core-hp` 的 `Book` / `scale`；等价轨 `match-core` 保留 `BTreeSet`+`BigDecimal`。

---

## 3. 代码索引

| 模块 | 路径 |
|------|------|
| SPSC + 伪共享隔离 + 批量出队 | `crates/match-core-hp/src/spsc.rs` |
| Worker + 等待策略 | `crates/match-core-hp/src/worker.rs` |
| 价位簿 | `crates/match-core-hp/src/book.rs` |
| 定点 | `crates/match-core-hp/src/scale.rs` |
| 绑核说明 | `crates/match-core-hp/src/affinity.rs` |
| 等价轨（对照） | `crates/match-core/` |
| 基准 | `crates/match-bench/`、`docs/bench-results.md` |

---

## 4. 使用建议（性能轨）

1. Bench / 延迟敏感路径：用 `HpEngine` 或 `HpWorker` + `WaitStrategy::BusySpin`（独占核时）。  
2. 与其他任务共享 CPU：用 `WaitStrategy::Yield`。  
3. 生产切流前：先 L3/shadow（见 `docs/l3-shadow.md`），且默认仍走 `match-core`。  
4. 绑核：进程启动后对 worker 线程调用 `affinity::pin_current_thread(core_id)`（需启用 feature `affinity`）。

---

## 5. 刻意不做的「反模式」

| 反模式 | 原因 |
|--------|------|
| 热路径 `BigDecimal` / JSON | 分配与解析主导延迟 |
| 多线程共写同一订单簿 | 锁与伪共享 |
| Ring 满了自动扩容 | 隐藏延迟尖峰 |
| 为追 Java quirk 牺牲干净热路径 | 双轨策略下由 `match-core` 承担等价 |
