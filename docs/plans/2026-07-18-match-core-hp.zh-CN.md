# match-core-hp 实现计划

**English：** [2026-07-18-match-core-hp.md](./2026-07-18-match-core-hp.md)

> **致代理执行者：** 必用子技能：使用 superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans，按任务逐步实现本计划。步骤使用复选框（`- [ ]`）语法跟踪。

**目标：** 增加双轨高性能撮合核（`match-core-hp`），采用定点价格档位簿，以及 `match-bench` crate，在热场景上证明相对 `match-core` ≥5× 吞吐，且不改变生产默认路径。

**架构：** 新 crate `match-core-hp` 拥有 `HpEngine`（i64 ticks/lots、价格档位 + FIFO、预分配订单槽）。薄 `adapter` 仅在边界转换 protocol/`BbOrder`。`match-bench` 对 core 与 hp 跑相同逻辑序列。`match-contract` 仍走 `match-core`。

**技术栈：** Rust 2021，`criterion`（dev），H3 可选 `rtrb` 做 SPSC，现有 `match-protocol` / `match-core` 仅作对比。

**规格：** `docs/specs/2026-07-18-match-core-hp-design.md`

**工作目录：** `.`，分支 `feature/rust-match-engines`（或新分支 `feature/match-core-hp`）。始终 `export PATH="$HOME/.cargo/bin:$PATH"`。

---

## 文件地图

| 路径 | 职责 |
|------|----------------|
| `crates/match-core-hp/Cargo.toml` | HP crate |
| `crates/match-core-hp/src/lib.rs` | 导出 |
| `crates/match-core-hp/src/types.rs` | `Side`、`HpOrder`、`HpEvent`、`SymbolScale` |
| `crates/match-core-hp/src/book.rs` | 价格档位簿 + FIFO + 订单槽 |
| `crates/match-core-hp/src/engine.rs` | `HpEngine::on_order` 限价/市价/撤单 |
| `crates/match-core-hp/src/adapter.rs` | Decimal/string ↔ tick/lot；可选自 `match_protocol::BbOrder` |
| `crates/match-core-hp/src/spsc.rs` | H3：SPSC 命令环 + worker 循环 |
| `crates/match-core-hp/tests/*.rs` | 正确性测试 |
| `crates/match-bench/Cargo.toml` | Bench 二进制 crate |
| `crates/match-bench/benches/engine_cmp.rs` | criterion core vs hp |
| `docs/bench-results.md` | 首次发布的数字 |
| `Cargo.toml`（workspace） | 增加 members |
| `README.md` | 链接 hp + bench |

---

### Task 1: 脚手架 `match-core-hp` + workspace member

**文件：**
- 创建：`crates/match-core-hp/Cargo.toml`
- 创建：`crates/match-core-hp/src/lib.rs`
- 修改：`Cargo.toml`（workspace members）
- 修改：`README.md`（文档下一行）

- [ ] **Step 1: 添加 crate**

```toml
# crates/match-core-hp/Cargo.toml
[package]
name = "match-core-hp"
version.workspace = true
edition.workspace = true

[dependencies]
thiserror = { workspace = true }
```

```rust
// crates/match-core-hp/src/lib.rs
//! High-performance matching core (fixed-point, price-level book).
//! Not Java-equivalent; not used by match-contract by default.
```

将 `"crates/match-core-hp"` 加入 workspace `members`。

- [ ] **Step 2: 构建**

运行：`cargo build -p match-core-hp`  
预期：成功

- [ ] **Step 3: 提交**

```bash
git add Cargo.toml crates/match-core-hp README.md
git commit -m "$(cat <<'EOF'
chore: scaffold match-core-hp crate

EOF
)"
```

---

### Task 2: 类型 + 精度转换（TDD）

**文件：**
- 创建：`crates/match-core-hp/src/types.rs`
- 创建：`crates/match-core-hp/src/scale.rs`
- 修改：`crates/match-core-hp/src/lib.rs`
- 创建：`crates/match-core-hp/tests/scale_convert.rs`

- [ ] **Step 1: 会失败的测试**

```rust
use match_core_hp::{SymbolScale, to_tick, to_lot, from_tick};

#[test]
fn price_to_tick_scale_2() {
    let s = SymbolScale { price_scale: 2, qty_scale: 6 };
    assert_eq!(to_tick(&s, "100.05").unwrap(), 10005);
    assert_eq!(from_tick(&s, 10005), "100.05");
}

#[test]
fn rejects_overflow_digits() {
    let s = SymbolScale { price_scale: 2, qty_scale: 6 };
    assert!(to_tick(&s, "100.999").is_err()); // more fractional digits than scale
}
```

实现精确 scale 的 `to_tick`/`to_lot`（拒绝多余小数位，或定义银行家舍入 — **选择拒绝多余位** 以保证确定性）。

- [ ] **Step 2: FAIL → 实现 → PASS**

- [ ] **Step 3: 提交** `feat(match-core-hp): fixed-point scale conversion`

---

### Task 3: 订单槽 + 价格档位簿（TDD）

**文件：**
- 创建：`crates/match-core-hp/src/book.rs`
- 创建：`crates/match-core-hp/src/order_store.rs`
- 创建：`crates/match-core-hp/tests/book_order.rs`

**API 草图：**

```rust
pub struct OrderStore { /* Vec<Option<HpOrder>> + free list */ }
pub struct Book {
    bids: BTreeMap<Reverse<i64>, Level>,
    asks: BTreeMap<i64, Level>,
    store: OrderStore,
}
impl Book {
    pub fn insert_limit(&mut self, order: HpOrder) -> u64; // returns id
    pub fn cancel(&mut self, id: u64) -> bool;
    pub fn best_ask(&self) -> Option<i64>;
    pub fn best_bid(&self) -> Option<i64>;
    pub fn depth(&self, side: Side, n: usize) -> Vec<(i64, i64)>; // tick, lot
}
```

`Level`：`total_lot`、`VecDeque<u64>` 订单 id（FIFO）。

- [ ] **Step 1: 测试**

```rust
#[test]
fn bid_best_is_highest_tick() {
    let mut b = Book::new();
    b.insert_limit(HpOrder::limit(Side::Buy, 10000, 1, 1));
    b.insert_limit(HpOrder::limit(Side::Buy, 10100, 1, 2));
    assert_eq!(b.best_bid(), Some(10100));
}

#[test]
fn same_price_fifo_cancel_middle() {
    // insert two at same tick; cancel first; best still second
}
```

- [ ] **Step 2: 实现 → PASS → 提交** `feat(match-core-hp): price-level book and order slots`

---

### Task 4: HpEngine 限价撮合 + 撤单（TDD）

**文件：**
- 创建：`crates/match-core-hp/src/engine.rs`
- 创建：`crates/match-core-hp/tests/limit_match.rs`
- 修改：`lib.rs`

```rust
pub struct HpEngine { book: Book }
impl HpEngine {
    pub fn on_order(&mut self, cmd: HpCommand) -> &[HpEvent]; // or drain into caller buffer
}
pub enum HpCommand {
    Limit { side: Side, price_tick: i64, qty_lot: i64, ts: u64, client_id: u64 },
    Cancel { id: u64 },
    Market { side: Side, qty_lot: i64, ts: u64, max_levels: Option<u32>, client_id: u64 },
}
```

热路径：将成交写入可复用的 `Vec<HpEvent>`，配合 `clear()` + capacity reserve（构造时预分配，例如 64）。

- [ ] **Step 1: 测试**（镜像干净语义，非 Java quirks）

```rust
#[test]
fn limit_buy_fills_resting_sell() {
    let mut e = HpEngine::new();
    e.on_order(HpCommand::Limit { side: Side::Sell, price_tick: 100, qty_lot: 5, ts: 1, client_id: 1 });
    let ev = e.on_order(HpCommand::Limit { side: Side::Buy, price_tick: 100, qty_lot: 5, ts: 2, client_id: 2 });
    assert!(matches!(ev[0], HpEvent::Fill { qty_lot: 5, price_tick: 100, .. }));
    assert!(e.book.best_ask().is_none());
}

#[test]
fn price_time_older_maker_first() { /* two sells same tick; buy 1 lot hits older */ }

#[test]
fn partial_fill_leaves_remainder() { /* buy 3 vs sell 1 → 2 left on bid */ }

#[test]
fn cancel_removes_resting() { … }
```

成交价 = **maker** tick（挂簿订单价格）。

- [ ] **Step 2: 仅实现限价/撤单（市价 → Task 5）**

- [ ] **Step 3: 提交** `feat(match-core-hp): HpEngine limit match and cancel`

---

### Task 5: 市价 + 深度 + adapter（TDD）

**文件：**
- 修改：`engine.rs`（市价）
- 创建：`crates/match-core-hp/src/adapter.rs`
- 创建：`crates/match-core-hp/tests/market_depth.rs`
- 创建：`crates/match-core-hp/tests/adapter_bborder.rs`
- 增加依赖：`match-protocol`、`bigdecimal`（仅 adapter）

- [ ] **Step 1: 市价测试** — 市价买沿 ask 吃到 qty 完成或簿空；可选 `max_levels`。

- [ ] **Step 2: 深度测试** — 两笔同 tick 买单聚合 lots。

- [ ] **Step 3: Adapter** — `fn from_bb_order(o: &match_protocol::BbOrder, scale: &SymbolScale) -> Result<HpCommand>`，用于 Limit/Cancel；供后续 bench 使用。

- [ ] **Step 4: 提交** `feat(match-core-hp): market, depth, and protocol adapter`

---

### Task 6: `match-bench` criterion 对比

**文件：**
- 创建：`crates/match-bench/Cargo.toml`（`[[bench]]` harness false）
- 创建：`crates/match-bench/benches/engine_cmp.rs`
- 创建：`crates/match-bench/src/workload.rs` — 为五种场景生成 N 条命令
- 修改：workspace members
- 创建：`docs/bench-results.md`（首次跑完后填写）

- [ ] **Step 1: Workload 辅助**，产出并行输入：
  - hp：`Vec<HpCommand>`
  - core：`Vec<match_core::BbOrder>`，经 `BbOrder::test_limit` / market helpers

场景：`rest_only`、`cross_full`、`partial_walk`、`cancel_hot`、`mixed`（各 ≥ 10_000 单）。

- [ ] **Step 2: Criterion benches**

```rust
fn bench_cross_full(c: &mut Criterion) {
    let (core_orders, hp_cmds) = workload::cross_full(50_000);
    c.bench_function("core_cross_full", |b| {
        b.iter(|| {
            let mut eng = Engine::new();
            for o in &core_orders { eng.on_order(o.clone()); }
        })
    });
    c.bench_function("hp_cross_full", |b| {
        b.iter(|| {
            let mut eng = HpEngine::new();
            for c in &hp_cmds { eng.on_order(*c); /* or ref */ }
        })
    });
}
```

尽量让 `HpCommand: Copy`，避免 clone 噪音不公平偏向 hp；对 core，克隆 `BbOrder` 是其真实成本的一部分 — 文档说明这一点。

- [ ] **Step 3: 运行**

```bash
cargo bench -p match-bench --bench engine_cmp -- --sample-size 20
```

将摘要贴到 `docs/bench-results.md`，含机器信息 + 倍率。若 `cross_full`/`partial_walk` 倍率 &lt; 5×，在关闭 Task 6 前用后续提交做 profile（减少 Level `VecDeque` 分配、更密的槽）。

- [ ] **Step 4: 提交** `feat(match-bench): compare match-core vs match-core-hp`

---

### Task 7: SPSC worker + 预分配事件缓冲（H3）

**文件：**
- 创建：`crates/match-core-hp/src/spsc.rs`（或用 `rtrb` 依赖）
- 创建：`crates/match-core-hp/src/worker.rs`
- 创建：`crates/match-core-hp/tests/spsc_worker.rs`

- [ ] **Step 1: API**

```rust
pub struct HpWorker { /* ring + engine + event_buf */ }
impl HpWorker {
    pub fn try_submit(&self, cmd: HpCommand) -> Result<(), Busy>;
    pub fn run_once(&mut self) -> usize; // process available cmds, return fill count
}
```

单线程测试：submit N → `run_once` 循环 → 断言成交数。

- [ ] **Step 2: 预分配** `event_buf: Vec<HpEvent>` 带 capacity；`on_order` clear 并复用（必要时重构 engine）。

- [ ] **Step 3: 可选 bench 路径** `hp_cross_full_spsc`（在 match-bench 中）。

- [ ] **Step 4: 提交** `feat(match-core-hp): SPSC worker and preallocated events`

---

### Task 8: 文档 + 护栏

**文件：**
- 修改：`README.md` — hp 小节：双轨警告、如何 bench
- 修改：`docs/superpowers/specs/2026-07-18-match-core-hp-design.md` 状态（若需要）
- Grep：确认 `match-contract` Cargo.toml **没有** `match-core-hp` 依赖

- [ ] **Step 1: README** 注明生产默认不变

- [ ] **Step 2: `cargo test --workspace` + `cargo bench -p match-bench` 冒烟**

- [ ] **Step 3: 提交** `docs: match-core-hp dual-track usage and bench results`

---

## 规格覆盖

| 规格项 | Task |
|-----------|------|
| Crate 隔离 | 1, 8 |
| 定点 | 2 |
| 价格档位簿 | 3 |
| 限价/撤单 | 4 |
| 市价/深度/adapter | 5 |
| Bench ≥5× 目标 | 6 |
| SPSC + 预分配 | 7 |
| 生产默认不动 | 8 |

---

## 自审备注

- 无生产切换任务（H4 延后）。  
- 高级订单延后。  
- 类型（`HpCommand`、`HpEngine`、`SymbolScale`）跨任务一致。  
- 5× 为目标，Task 6 有明确的「先 profile 再迭代」退路。
