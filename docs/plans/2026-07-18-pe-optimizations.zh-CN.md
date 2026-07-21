# 受 perpetual_exchange 启发的优化实现计划

**English：** [2026-07-18-pe-optimizations.md](./2026-07-18-pe-optimizations.md)

**目标：** 落地 [2026-07-18-pe-optimizations-design.md](../specs/2026-07-18-pe-optimizations-design.md) 的 Phase A→B→C：最优价缓存 + 档位对象池 + 可选 ART，然后是 `hp-engine` spans，再是异步 `match-wal`。

**架构：** 默认保留 `BTreeMap` 簿；增加 `LevelIndex` trait 与 `art` feature。生产 `match-contract` 仍走 `match-core`，除非启用 `hp-engine`。WAL 为独立 crate，热路径仅异步。

**技术栈：** Rust workspace `match-rust`，`match-core-hp`，criterion/`fair_compare`，可选 ART（自包含），tracing/`Instant` spans。

**规格：** `docs/specs/2026-07-18-pe-optimizations-design.md`。

---

## 文件地图

| 路径 | 角色 |
|------|------|
| `crates/match-core-hp/src/book.rs` | 最优价缓存；档位池；使用 `LevelIndex` |
| `crates/match-core-hp/src/level_index.rs` | Trait + `BTreeLevelIndex` |
| `crates/match-core-hp/src/art_index.rs` | `ArtLevelIndex`，在 `feature = "art"` 后 |
| `crates/match-core-hp/Cargo.toml` | `art` feature |
| `crates/match-core-hp/tests/best_price_cache.rs` | 缓存 / 空最优价测试 |
| `crates/match-core-hp/tests/art_parity.rs` | 默认 vs art 相同成交（cfg） |
| `crates/match-contract/...` | `hp-engine` feature + spans |
| `crates/match-wal/` | 新异步 WAL crate |
| `docs/bench-results.md`、`e2e-budget.md`、`best-practices.md` | 数字 + 映射 |

---

### Task 1: 最优价缓存

**文件：**
- 修改：`crates/match-core-hp/src/book.rs`
- 创建：`crates/match-core-hp/tests/best_price_cache.rs`

- [ ] **Step 1: 会失败的测试** — 撤单/成交清空最优档时更新 `best_*`；插入更优价格时更新缓存。

- [ ] **Step 2: 实现** — 字段 `best_bid_tick` / `best_ask_tick`；在 `level_mut` 首次 push、`remove_empty_level`、以及成交清空档后更新。`best_bid`/`best_ask` 读缓存。debug 下对 map 做 `debug_assert`。

- [ ] **Step 3: `cargo test -p match-core-hp`** — 全绿。

- [ ] **Step 4: 提交** `feat(match-core-hp): O(1) best bid/ask cache`

---

### Task 2: 档位对象池

**文件：**
- 修改：`book.rs`（池 `Vec<Level>`，在 `remove_empty_level` 回收，新建档时取用）

- [ ] **Step 1: 测试** — 插入大量不同 tick 再全部撤掉；第二波插入仍正确（FIFO/最优价）。

- [ ] **Step 2: 实现** — `level_pool: Vec<Level>`，上限例如 256；`or_insert_with` 从池取；remove 时把清空的 Level 推回。

- [ ] **Step 3: 测试 + 提交** `feat(match-core-hp): recycle empty price levels`

---

### Task 3: `LevelIndex` + BTree 后端（行为保持）

**文件：**
- 创建：`level_index.rs`
- 修改：`book.rs`、`lib.rs`

- [ ] **Step 1: 抽出 trait**，含 `insert_level` / `get` / `get_mut` / `remove` / `best_tick` / `iter_depth(n)`。

- [ ] **Step 2: `BTreeLevelIndex`** — asks（`BTreeMap<i64, Level>`）与 bids（`BTreeMap<Reverse<i64>, Level>` 或映射 max-bid 的包装）。

- [ ] **Step 3: Book 持有两个 index；现有测试不变通过。

- [ ] **Step 4: 提交** `refactor(match-core-hp): LevelIndex trait over BTreeMap`

---

### Task 4: `art` feature 后的 ART index

**文件：**
- 创建：`art_index.rs`（最小有序 map：按 `i64` 键 insert/remove/min/max/iter）
- 修改：`Cargo.toml` `art = []`、`lib.rs` cfg
- 创建：`tests/art_parity.rs`

- [ ] **Step 1: 最小 ART 或 radix map：`i64` → Level**（正确性优先；Node16 SIMD 可后做）。

- [ ] **Step 2: Book 类型别名 / cfg 选择 index 实现。

- [ ] **Step 3: 对齐测试** — 跑相同限价/交叉序列；断言 `n_fills` 与深度快照相等（测试编两次或双跑辅助）。

- [ ] **Step 4: `cargo test -p match-core-hp --features art` + 默认。

- [ ] **Step 5: 提交** `feat(match-core-hp): optional ART LevelIndex`

- [ ] **Step 6: 更新** `docs/bench-results.md`、`docs/fair-compare.md`、`docs/best-practices.md`

---

### Task 5: Phase B — `hp-engine` + spans

**文件：**
- `match-contract/Cargo.toml` feature `hp-engine`
- `symbol_worker.rs`（或 inbound 路径）：feature 开启时 adapter → HpWorker
- spans：`L3_adapt`、`L2_queue`、`L1_on_order`
- 若可跑，用测得占位值更新 `e2e-budget.md`

- [ ] 默认构建不变；feature 构建做 memory-transport 冒烟。
- [ ] 提交 `feat(match-contract): optional hp-engine path with latency spans`

---

### Task 6: Phase C — `match-wal`

**文件：**
- 新 crate `match-wal`：SPSC/mpsc 缓冲、后台 flusher、`Async` 模式
- 可选从 hp engine/events 挂钩
- Bench 二进制或 criterion 测 records/sec
- 提交 `feat(match-wal): async batched trade/order log`

---

### Task 7: 文档同步 + fair_compare 回归

- [ ] `cargo run -p match-bench --release --bin fair_compare -- --n 50000` 退出码 0
- [ ] 两份规格副本中设计状态同步为 Approved
- [ ] 若文档有待提交则最终提交

---

## 规格覆盖

| 规格章节 | Tasks |
|--------------|-------|
| §3.1 最优价缓存 | T1 |
| §3.2 档位池 | T2 |
| §3.3 LevelIndex + art | T3, T4 |
| §4 Phase B | T5 |
| §5 Phase C | T6 |
| §7 文档 | T4.6, T5, T7 |
