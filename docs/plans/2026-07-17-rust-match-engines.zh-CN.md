# Rust 合约撮合引擎实现计划

**English：** [2026-07-17-rust-match-engines.md](./2026-07-17-rust-match-engines.md)

> **致代理执行者：** 必用子技能：使用 superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans，按任务逐步实现本计划。步骤使用复选框（`- [ ]`）语法跟踪。

**目标：** 交付可替换 Java `java-contract-match` 的 Rust 进程，在可观测结果上等价（成交、剩余量、撤单路径、深度档位），经黄金回放验证，再按交易对灰度切换。

**架构：** Cargo workspace `match-rust`，共享 `match-core` + `match-protocol`，合约二进制 `match-contract`（MQ/Redis/RPC 外壳），以及用于 L1/L2 门禁的 `match-replay`。从 Java handler 移植控制流；除非另有独立修复任务，否则保留已知 Java bug。

**技术栈：** Rust 1.78+（edition 2021），`bigdecimal`，`serde`/`serde_json`，`tokio`，`redis`（cluster），Apache RocketMQ Rust 客户端对接 NameServer（测试：`192.168.0.241:9876`），`reqwest`，`tracing`/`opentelemetry`，`clap`/`figment` 或 `config` 读 YAML。

**规格：** `docs/specs/2026-07-17-rust-match-engines-design.md`

**本计划范围：** 仅合约里程碑 **M0–M4**。现货 `match-spot`（M5）为**独立计划**。

**Java 移植源（只读参考）：**
- `java-contract-match/contract-match-provider/src/main/java/com/example/match/`
- `java-contract-match/contract-match-api/src/main/java/com/example/contract/match/api/`

---

## 文件地图

| 路径 | 职责 |
|------|----------------|
| `Cargo.toml` | Workspace members + 共享依赖 |
| `crates/match-protocol/` | `MqOrder`、常量、`check_mq_order`、`type_convert`、`encode_symbol_key`、JSON 解析 |
| `crates/match-core/` | `OrderBook`、`Engine`、限价/市价/高级/FOK 撮合、深度快照、`MatchEvent` |
| `crates/match-replay/` | CLI + 库：跑输入 → `GoldenTrace` diff |
| `crates/match-contract/` | 二进制：配置、bootstrap、Redis、HTTP 恢复、RMQ 进/出、指标、健康检查 |
| `crates/match-spot/` | 占位 crate（`lib.rs` 空模块），保证 workspace 完整 |
| `testdata/golden/` | 入库的 `*.ndjson` 黄金轨迹 |
| `java-contract-match/.../test/.../GoldenTraceExporterTest.java`（或 `tools/java-golden/`） | Java 侧导出器，生成黄金文件 |

**规格中已关闭的开放项：**
- 小数：`bigdecimal` crate（Java `BigDecimal` 语义）。
- Redis：`redis` crate，启用 `cluster` feature。
- RocketMQ：Apache `rocketmq` Rust 客户端（NameServer 模式）；Task 12 在接线生产者前先对测试 NS 验证。
- 恢复 HTTP：`POST {market_base}/contract-market/contractcoinMarketList`，`POST {order_base}/contract/entrust-list`。
- 黄金导出器：`java-contract-match` 下的 JUnit 测试夹具，进程内驱动 handler 并写 NDJSON（不改生产代码路径）。

---

### Task 1: Workspace 脚手架

**文件：**
- 创建：`Cargo.toml`
- 创建：`crates/match-protocol/Cargo.toml`
- 创建：`crates/match-protocol/src/lib.rs`
- 创建：`crates/match-core/Cargo.toml`
- 创建：`crates/match-core/src/lib.rs`
- 创建：`crates/match-replay/Cargo.toml`
- 创建：`crates/match-replay/src/lib.rs`
- 创建：`crates/match-contract/Cargo.toml`
- 创建：`crates/match-contract/src/main.rs`
- 创建：`crates/match-spot/Cargo.toml`
- 创建：`crates/match-spot/src/lib.rs`
- 创建：`README.md`

- [ ] **Step 1: 创建 workspace 根**

```toml
# match-rust/Cargo.toml
[workspace]
resolver = "2"
members = [
  "crates/match-protocol",
  "crates/match-core",
  "crates/match-replay",
  "crates/match-contract",
  "crates/match-spot",
]

[workspace.package]
edition = "2021"
license = "UNLICENSED"
version = "0.1.0"

[workspace.dependencies]
bigdecimal = { version = "0.4", features = ["serde"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "1"
tracing = "0.1"
```

- [ ] **Step 2: 创建占位 crate**

`match-protocol/Cargo.toml`:
```toml
[package]
name = "match-protocol"
version.workspace = true
edition.workspace = true

[dependencies]
bigdecimal = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
```

`match-protocol/src/lib.rs`:
```rust
//! Wire models and validation aligned with Java java-contract-match.
```

`match-core/Cargo.toml`: 依赖 `match-protocol`、`bigdecimal`、`thiserror`、`tracing`。

`match-core/src/lib.rs`:
```rust
//! Pure matching engine (no MQ/Redis/HTTP).
```

`match-replay/Cargo.toml`: 依赖 `match-core`、`match-protocol`、`serde_json`、`clap`。

`match-contract/Cargo.toml`:
```toml
[package]
name = "match-contract"
version.workspace = true
edition.workspace = true

[[bin]]
name = "match-contract"
path = "src/main.rs"

[dependencies]
match-core = { path = "../match-core" }
match-protocol = { path = "../match-protocol" }
tokio = { version = "1", features = ["full"] }
tracing = { workspace = true }
```

`match-contract/src/main.rs`:
```rust
fn main() {
    println!("match-contract stub — not production ready");
}
```

`match-spot/src/lib.rs`:
```rust
//! Spot engine shell — filled in a separate plan (M5).
```

`README.md`: 一段话指向设计规格与 `cargo test --workspace`。

- [ ] **Step 3: 验证构建**

运行：`cd . && cargo build --workspace`

预期：成功（占位二进制/库）。

- [ ] **Step 4: 提交**

```bash
cd .
git add -A
git commit -m "$(cat <<'EOF'
chore: scaffold match-rust workspace for contract match port

EOF
)"
```

（若 `match-rust` 不是独立 git 根，从 monorepo 根提交并包含这些路径。）

---

### Task 2: 协议常量、MqOrder、校验、type_convert

**文件：**
- 创建：`crates/match-protocol/src/constants.rs`
- 创建：`crates/match-protocol/src/mq_order.rs`
- 创建：`crates/match-protocol/src/order.rs`
- 创建：`crates/match-protocol/src/validate.rs`
- 创建：`crates/match-protocol/src/convert.rs`
- 创建：`crates/match-protocol/src/encode.rs`
- 修改：`crates/match-protocol/src/lib.rs`
- 创建：`crates/match-protocol/tests/validate_convert.rs`

**Java 参考：** `Constants.java`、`BBConstants.checkMqOrder` / `typeConvert`、`CoinMarketEncode.encodeSymbolKey`

- [ ] **Step 1: 编写会失败的测试**

```rust
// crates/match-protocol/tests/validate_convert.rs
use bigdecimal::BigDecimal;
use match_protocol::{check_mq_order, encode_symbol_key, type_convert, MqOrder};
use std::str::FromStr;

fn valid_mq() -> MqOrder {
    MqOrder {
        user_id: Some(1),
        uid: Some(100),
        c_type: 1,
        deal_type: Some(1),
        r#type: Some(1),
        order_type: Some(1),
        market_id: Some(1),
        coin_id: Some(2),
        symbol_key: Some("btcusdt".into()),
        coin_market: Some("BTC/USDT".into()),
        trust_order_no: Some("10001".into()),
        close_position: Some(1),
        start_deposit: Some("10".into()),
        position_type: Some(0),
        taker_rate: Some("0.0005".into()),
        order_status: Some(0),
        order_form: Some(1),
        gear: None,
        lever_times: Some(10),
        trust_number: Some("1".into()),
        trust_price: Some("50000".into()),
        create_time: Some(1_700_000_000_000),
        face_value: Some(BigDecimal::from_str("0.001").unwrap()),
        handicap_type: None,
    }
}

#[test]
fn check_mq_order_accepts_valid_limit() {
    assert!(check_mq_order(&valid_mq()));
}

#[test]
fn check_mq_order_rejects_market_without_gear() {
    let mut o = valid_mq();
    o.order_form = Some(2);
    o.gear = None;
    assert!(!check_mq_order(&o));
}

#[test]
fn type_convert_normalizes_symbol_and_remaining() {
    let bb = type_convert(&valid_mq()).expect("convert");
    assert_eq!(bb.symbol_key, "btcusdt");
    assert_eq!(bb.remaining_number, BigDecimal::from_str("1").unwrap());
    assert_eq!(bb.trust_price, BigDecimal::from_str("50000").unwrap());
}

#[test]
fn encode_symbol_key_ascii_passthrough() {
    assert_eq!(encode_symbol_key("btcusdt"), "btcusdt");
}
```

- [ ] **Step 2: 跑测试 — 预期 FAIL**

运行：`cd . && cargo test -p match-protocol --test validate_convert`

预期：编译错误（类型/函数缺失）。

- [ ] **Step 3: 实现协议模块**

实现 serde `MqOrder`，使用 `#[serde(rename_all = "camelCase")]`，字段名与 Java JSON 一致（`userId`、`trustOrderNo`、`cType` 等）。

`check_mq_order`：精确移植 `BBConstants.checkMqOrder` 规则（types 列表、orderStatus ∈ {0,2,3}、市价需 gear、closePosition/startDeposit/takerRate/positionType 必填）。

`type_convert`：移植 `BBConstants.typeConvert` — `symbol_key` = 去掉 `/` + 小写；`remaining_number = trust_number`；拒绝 `trust_number <= 0` 或 `trust_price <= 0`。

`encode_symbol_key`：移植 `CoinMarketEncode.encodeSymbolKey`（非 ASCII 部分用无 padding 的 Base64 URL）。

在 `lib.rs` 中用 `pub use` 导出公共项。

- [ ] **Step 4: 跑测试 — 预期 PASS**

运行：`cargo test -p match-protocol --test validate_convert`

- [ ] **Step 5: 提交**

```bash
git add crates/match-protocol
git commit -m "$(cat <<'EOF'
feat(match-protocol): add MqOrder validation and type_convert

EOF
)"
```

---

### Task 3: 订单簿（价时优先）

**文件：**
- 创建：`crates/match-core/src/book.rs`
- 创建：`crates/match-core/src/order.rs`（引擎侧 `BbOrder`，包装/再导出协议订单）
- 修改：`crates/match-core/src/lib.rs`
- 创建：`crates/match-core/tests/order_book_order.rs`

**Java 参考：** `BBOrder.compareTo`（买 高→低，卖 低→高；并列时 `createTime` 再 `trustOrderNo`）

- [ ] **Step 1: 编写会失败的测试**

```rust
// crates/match-core/tests/order_book_order.rs
use bigdecimal::BigDecimal;
use match_core::{OrderBook, Side, BbOrder};
use std::str::FromStr;

fn order(side: Side, price: &str, no: &str, t: i64) -> BbOrder {
    BbOrder::test_limit(side, BigDecimal::from_str(price).unwrap(), no, t, "1")
}

#[test]
fn buy_book_best_is_highest_price_then_earliest_time() {
    let mut book = OrderBook::new();
    book.insert(order(Side::Buy, "100", "3", 200));
    book.insert(order(Side::Buy, "101", "1", 300));
    book.insert(order(Side::Buy, "101", "2", 100));
    let best = book.best(Side::Buy).unwrap();
    assert_eq!(best.trust_order_no, "2"); // price 101, earlier time
}

#[test]
fn sell_book_best_is_lowest_price() {
    let mut book = OrderBook::new();
    book.insert(order(Side::Sell, "100", "1", 1));
    book.insert(order(Side::Sell, "99", "2", 1));
    assert_eq!(book.best(Side::Sell).unwrap().trust_order_no, "2");
}
```

- [ ] **Step 2: 跑 — 预期 FAIL**

运行：`cargo test -p match-core --test order_book_order`

- [ ] **Step 3: 实现 `OrderBook`**

使用带与 Java `compareTo` 一致比较器的 `BTreeSet`（或买卖分设，在包装类型上实现 `Ord`）。提供 `insert`、`remove`（按完整订单身份，类似 TreeSet.remove）、`best`/`first`、`is_empty`。

`remove` 必须使用 Java TreeSet 成员同一相等键（同价同 `trustOrderNo` 时 `compareTo == 0` — 注意 Java 无 `equals`/`hashCode`；保留 TreeSet remove 语义）。

- [ ] **Step 4: 跑 — 预期 PASS**

- [ ] **Step 5: 提交**

```bash
git commit -am "$(cat <<'EOF'
feat(match-core): add price-time order book

EOF
)"
```

---

### Task 4: Engine 门面 + MatchEvent + L1 夹具

**文件：**
- 创建：`crates/match-core/src/engine.rs`
- 创建：`crates/match-core/src/event.rs`
- 创建：`crates/match-core/src/id.rs`
- 修改：`crates/match-core/src/lib.rs`
- 创建：`crates/match-core/tests/l1_limit_cross.rs`

- [ ] **Step 1: 定义事件**

```rust
// event.rs (sketch — implement fully)
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MatchEvent {
    Fill {
        symbol: String,
        taker_order_no: String,
        maker_order_no: String,
        price: String,
        qty: String,
        taker_remaining: String,
        maker_remaining: String,
        taker_status: u8,
        maker_status: u8,
    },
    BookUpdate { /* optional; or derive from fills */ },
    Revoke {
        order_no: String,
        symbol: String,
        remaining: String,
        reason: String, // e.g. "user", "market_gear", "ioc_remainder", "fok_fail", "post_only"
    },
}
```

`Engine` API：
```rust
pub struct Engine { /* per-symbol books inside HashMap */ }
impl Engine {
    pub fn new() -> Self;
    pub fn on_order(&mut self, order: BbOrder) -> Vec<MatchEvent>;
    pub fn depth_levels(&self, symbol: &str, side: Side, limit: usize) -> Vec<(BigDecimal, BigDecimal)>;
}
```

成交 ID：注入 `IdGenerator` trait；默认实现用 atomic u64 供测试（Java 用 `IDGenerator.generatorId()` — 生产外壳可稍后调同一 snowflake HTTP 或本地等价；**成交**等价性上，golden 优先比较订单号/数量/价格；golden 含 `dealNo` 时再比）。

- [ ] **Step 2: 会失败的 L1 测试 — 空簿限价挂单**

```rust
#[test]
fn limit_buy_rests_on_empty_book() {
    let mut eng = Engine::new();
    let events = eng.on_order(BbOrder::test_limit(
        Side::Buy, dec("100"), "1", 1, "2",
    ));
    assert!(events.iter().all(|e| !matches!(e, MatchEvent::Fill { .. })));
    let depth = eng.depth_levels("btcusdt", Side::Buy, 20);
    assert_eq!(depth.len(), 1);
    assert_eq!(depth[0].1, dec("2"));
}
```

- [ ] **Step 3: 最小 `on_order` — 仅挂簿（尚无撮合）**

足以通过 Step 2 测试。

- [ ] **Step 4: 提交**

```bash
git commit -am "$(cat <<'EOF'
feat(match-core): engine facade and MatchEvent for L1 harness

EOF
)"
```

---

### Task 5: 限价撮合 — RatherThan / Equals / LessThan + Buy/Sell 循环

**文件：**
- 创建：`crates/match-core/src/match_limit.rs`
- 创建：`crates/match-core/src/handlers/buy.rs`
- 创建：`crates/match-core/src/handlers/sell.rs`
- 创建：`crates/match-core/src/handlers/mod.rs`
- 创建：`crates/match-core/src/price_utils.rs`
- 修改：`crates/match-core/src/engine.rs`
- 创建：`crates/match-core/tests/l1_limit_fill.rs`

**Java 参考（控制流 1:1 移植）：**
- `RatherThanHandler.java`、`EqualsHandler.java`、`LessThanHandler.java`
- `BuyHandler.java`、`SellHandler.java`
- `PriceUtils.java`、`BaseHandler.addToBook` / `removeFromOrderBook`
- 当 `bbOrders.size() >= SEND_MAX_DATA`（10）时批量刷出 — 按相同批次边界发出事件

- [ ] **Step 1: 编写会失败的交叉成交测试**

```rust
#[test]
fn limit_buy_fully_fills_resting_sell() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Sell, dec("100"), "s1", 1, "1"));
    let events = eng.on_order(BbOrder::test_limit(Side::Buy, dec("100"), "b1", 2, "1"));
    let fills: Vec<_> = events.iter().filter(|e| matches!(e, MatchEvent::Fill { .. })).collect();
    assert_eq!(fills.len(), 1);
    if let MatchEvent::Fill { price, qty, taker_remaining, maker_remaining, .. } = &fills[0] {
        assert_eq!(price, "100");
        assert_eq!(qty, "1");
        assert_eq!(taker_remaining, "0");
        assert_eq!(maker_remaining, "0");
    }
    assert!(eng.depth_levels("btcusdt", Side::Buy, 20).is_empty());
    assert!(eng.depth_levels("btcusdt", Side::Sell, 20).is_empty());
}

#[test]
fn price_time_priority_older_maker_first() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Sell, dec("100"), "s_old", 1, "1"));
    eng.on_order(BbOrder::test_limit(Side::Sell, dec("100"), "s_new", 2, "1"));
    let events = eng.on_order(BbOrder::test_limit(Side::Buy, dec("100"), "b1", 3, "1"));
    if let MatchEvent::Fill { maker_order_no, .. } = &events[0] {
        assert_eq!(maker_order_no, "s_old");
    } else {
        panic!("expected fill");
    }
}
```

- [ ] **Step 2: 跑 — 预期 FAIL**

- [ ] **Step 3: 移植撮合辅助 + buy/sell 循环**

将 Java 方法移植为 Rust 函数。保持分支顺序一致（ratherThan → equals/lessThan）。保留 `removeFromOrderBook` 失败行为（日志/指标钩子；不要自创重试）。

将 `Engine::on_order` 对 `order_form == 1`（及默认非高级）接到 buy/sell handler。`order_status == REVOKE (3)` → Buy/SellHandler 的撤单路径。

- [ ] **Step 4: 跑 — 预期 PASS**

运行：`cargo test -p match-core --test l1_limit_fill`

- [ ] **Step 5: 提交**

```bash
git commit -am "$(cat <<'EOF'
feat(match-core): port limit match RatherThan/Equals/LessThan

EOF
)"
```

---

### Task 6: 市价单 + 档位撤单

**文件：**
- 创建：`crates/match-core/src/handlers/market.rs`
- 修改：`crates/match-core/src/handlers/buy.rs` / `sell.rs`
- 创建：`crates/match-core/tests/l1_market_gear.rs`

**Java 参考：** `MarketBuyHandler.java`、`BuyHandler`/`SellHandler` 中的市价分支（档位停止；**保留**已知问题文档中的 P0-1/P0-3 行为）

- [ ] **Step 1: 会失败的测试**

```rust
#[test]
fn market_buy_stops_at_gear_levels() {
    let mut eng = Engine::new();
    for i in 0..5 {
        eng.on_order(BbOrder::test_limit(
            Side::Sell, dec(&(100 + i).to_string()), &format!("s{i}"), i, "1",
        ));
    }
    let mut taker = BbOrder::test_market(Side::Buy, "b_mkt", 10, "10");
    taker.gear = Some(2);
    let events = eng.on_order(taker);
    let fill_count = events.iter().filter(|e| matches!(e, MatchEvent::Fill { .. })).count();
    assert_eq!(fill_count, 2);
    assert!(events.iter().any(|e| matches!(e, MatchEvent::Revoke { reason, .. } if reason == "market_gear")));
}
```

另加 `gear = 0` 对齐 Java 的用例（已知 P0-3：可能立即撤单）— 断言 **Java 等价** 结果，而非“正确”结果。

- [ ] **Step 2–4: 移植 MarketBuyHandler + 市价循环；通过测试；提交**

```bash
git commit -am "$(cat <<'EOF'
feat(match-core): port market match and gear revoke (Java-equivalent)

EOF
)"
```

---

### Task 7: PostOnly / IOC / FOK

**文件：**
- 创建：`crates/match-core/src/handlers/height_buy.rs`
- 创建：`crates/match-core/src/handlers/height_sell.rs`
- 创建：`crates/match-core/src/handlers/fok_buy.rs`
- 创建：`crates/match-core/src/handlers/fok_sell.rs`
- 创建：`crates/match-core/tests/l1_advanced.rs`

**Java 参考：** `HeightBuyHandler.java`、`HeightSellHandler.java`、`FokBuyHandler.java`、`FokSellHandler.java`、`EventOrderHandler` 路由

- [ ] **Step 1: 会失败的测试（最小集）**

1. PostOnly 若会吃单 → revoke/`post_only`，无成交、不挂簿（仅当 Height handler 在撤前推深度时保留 Java flash 行为 — 对齐 Java）。
2. IOC 部分成交 → 剩余以 `ioc_remainder` 撤掉。
3. FOK 成功 → 全成，无剩余。
4. FOK 失败 → 簿净变化为零 + revoke `fok_fail`（移植回滚路径）。

- [ ] **Step 2: 1:1 移植 Height*/Fok* 控制流，含已知 IOC 循环 quirks（P0-2）**

在模块 rustdoc 中注明：`// intentional Java parity: see docs/合约撮合已知问题梳理.md P0-2`。

- [ ] **Step 3: `cargo test -p match-core --test l1_advanced` PASS**

- [ ] **Step 4: 提交**

```bash
git commit -am "$(cat <<'EOF'
feat(match-core): port PostOnly IOC FOK handlers with Java parity

EOF
)"
```

---

### Task 8: 深度 / 未成交快照构建

**文件：**
- 创建：`crates/match-core/src/depth.rs`
- 创建：`crates/match-core/tests/l1_depth.rs`

**Java 参考：** `NoDealProducer` / 深度聚合（`NO_DEAL_NUMBER=20`）、`DepthMapProducer`（`DEEPS_NUMBER`）、价格聚合规则

- [ ] **Step 1: 测试同价聚合档位**

```rust
#[test]
fn depth_aggregates_same_price() {
    let mut eng = Engine::new();
    eng.on_order(BbOrder::test_limit(Side::Buy, dec("100"), "1", 1, "1"));
    eng.on_order(BbOrder::test_limit(Side::Buy, dec("100"), "2", 2, "2"));
    let levels = eng.depth_levels("btcusdt", Side::Buy, 20);
    assert_eq!(levels.len(), 1);
    assert_eq!(levels[0].1, dec("3"));
}
```

- [ ] **Step 2: 实现与 Java getDepth/getDeeps 行为一致的聚合**

- [ ] **Step 3: 提交**

```bash
git commit -am "$(cat <<'EOF'
feat(match-core): depth level aggregation for no-deal/deeps

EOF
)"
```

---

### Task 9: GoldenTrace 格式 + match-replay + Java 导出器

**文件：**
- 创建：`crates/match-replay/src/trace.rs`
- 创建：`crates/match-replay/src/diff.rs`
- 创建：`crates/match-replay/src/main.rs`
- 创建：`testdata/golden/limit_cross.ndjson`（生成）
- 创建：`java-contract-match/contract-match-provider/src/test/java/com/example/match/golden/GoldenTraceExporterTest.java`
- 创建：`java-contract-match/contract-match-provider/src/test/java/com/example/match/golden/GoldenTraceWriter.java`

**黄金行 schema（NDJSON）：**
```json
{"seq":0,"op":"input","order":{/* MqOrder camelCase */}}
{"seq":1,"op":"fill","takerOrderNo":"b1","makerOrderNo":"s1","price":"100","qty":"1","takerRemaining":"0","makerRemaining":"0","takerStatus":1,"makerStatus":1}
{"seq":2,"op":"depth","symbol":"btcusdt","bids":[["100","0"]],"asks":[]}
{"seq":3,"op":"revoke","orderNo":"x","remaining":"1","reason":"ioc_remainder"}
```

小数按数值比较；忽略 JSON 键顺序。

- [ ] **Step 1: 实现 `GoldenTrace` serde + `diff_traces(expected, actual) -> Vec<String>`**

- [ ] **Step 2: Java 导出器测试**

进程内：建 `TreeSet` 簿 / 用构造的 `BBOrder` 列表调 handler（场景同 L1）。将 NDJSON 写到 `testdata/golden/`。

运行（在 `java-contract-match` 模块下）：
`mvn -pl contract-match-provider -Dtest=GoldenTraceExporterTest test`

预期：文件已写出。

- [ ] **Step 3: `match-replay` CLI**

```bash
cargo run -p match-replay -- --input testdata/golden/limit_cross.ndjson --engine rust
# reads input ops, runs Engine, diffs fill/depth/revoke lines
```

匹配时退出码 0，不匹配时 1 并打印 diff。

- [ ] **Step 4: 提交 golden + replay**

```bash
git add crates/match-replay match-rust/testdata \
  java-contract-match/contract-match-provider/src/test/java/com/example/match/golden
git commit -m "$(cat <<'EOF'
feat(match-replay): golden NDJSON format and Java exporter harness

EOF
)"
```

- [ ] **Step 5: 扩展黄金集**

导出并通过回放：limit cross、partial fill、market+gear、postonly、ioc、fok ok/fail、cancel。增加 `cargo test -p match-replay`，跑 `testdata/golden/` 下全部文件。

---

### Task 10: match-contract 配置 + HTTP 恢复客户端

**文件：**
- 创建：`crates/match-contract/src/config.rs`
- 创建：`crates/match-contract/src/rpc/market.rs`
- 创建：`crates/match-contract/src/rpc/order.rs`
- 创建：`crates/match-contract/src/rpc/mod.rs`
- 创建：`crates/match-contract/config.example.yaml`
- 创建：`crates/match-contract/tests/rpc_urls.rs`

**HTTP 路径（来自 Feign API）：**
- Market：`POST {market_base_url}/contract-market/contractcoinMarketList` → 含 `coinMarket`、`originCoinMarket`、`mainStream` 的 VO 列表
- Order：`POST {order_base_url}/contract/entrust-list` body `{ "trustOrderNo": "<BigNo>", "mainStream": 0 }` → `ResponseData` code==1，分页 `rows`

响应信封：对齐 `com.example.common.model.ResponseData`（`code`、`data`，…）。成功 `code == 1`。

- [ ] **Step 1: 配置结构**

```yaml
# config.example.yaml
shard: 0
startup_delay_ms: 10000
start_queue_ttl_ms: 720000
depth_push_interval_ms: 50
symbol_workers: 30
rocketmq:
  name_server: "192.168.0.241:9876"
  consumer_group: "usdt_contract_match_channel_one_group"
redis:
  cluster_nodes: ["192.168.0.241:7001"]
  password: ""
rpc:
  market_base_url: "http://contract-market-host"
  order_base_url: "http://contract-order-host"
match:
  symbols_whitelist: []  # empty = all shard symbols
```

- [ ] **Step 2: 单元测试 URL 拼接辅助**

```rust
#[test]
fn market_list_path() {
    assert_eq!(
        market::list_url("http://m"),
        "http://m/contract-market/contractcoinMarketList"
    );
}
```

- [ ] **Step 3: 实现 reqwest 客户端 + 反序列化 bootstrap 所需 VO/BO 字段**

- [ ] **Step 4: 提交**

```bash
git commit -am "$(cat <<'EOF'
feat(match-contract): config and restore RPC HTTP clients

EOF
)"
```

---

### Task 11: Redis 键 + 错误队列

**文件：**
- 创建：`crates/match-contract/src/redis_store.rs`
- 创建：`crates/match-contract/src/error_queue.rs`

**键（对齐）：**
- 撮合链路：前缀 + `redis_poc_link_list_key{symbol}`（与 Java 相同 RedisKeyPrefixEnum 行为 — 从前缀字符串确认 `RedisKey` / `RedisKeyPrefixEnum` in java-cache；实现时读 Java `RedisKey.toString()` 用法，复制精确键格式）。
- 深度清理：`contract_exchange_depth:{origin}detail|trade|paint`，在 MARKET_KEY 前缀下。
- 错误队列：`poc_redis_send_mq_error_data_queue`

- [ ] **Step 1: 阅读 Java `RedisKey` / `RedisTemplateMatch`，在 `redis_store.rs` 模块文档中写明精确键字符串**

- [ ] **Step 2: 实现 del/set/exists + 错误队列的 list push/pop**

- [ ] **Step 3: 带 `#[ignore]` 的集成测试，需 Redis（文档化 `cargo test -p match-contract -- --ignored`）

- [ ] **Step 4: 提交**

```bash
git commit -am "$(cat <<'EOF'
feat(match-contract): Redis link keys, depth wipe, MQ error queue

EOF
)"
```

---

### Task 12: Bootstrap + 按交易对 worker + RocketMQ 进/出

**文件：**
- 创建：`crates/match-contract/src/bootstrap.rs`
- 创建：`crates/match-contract/src/symbol_worker.rs`
- 创建：`crates/match-contract/src/mq/consumer.rs`
- 创建：`crates/match-contract/src/mq/producer.rs`
- 创建：`crates/match-contract/src/mq/topics.rs`
- 创建：`crates/match-contract/src/inbound.rs`
- 创建：`crates/match-contract/src/outbound.rs`
- 修改：`crates/match-contract/src/main.rs`

**Topic 常量：**
```rust
pub const PULL_ORDER_PREFIX: &str = "usdt_contract_match_order_";
pub const PUSH_ORDER_PREFIX: &str = "usdt_contract_match_order_push_order_";
pub const PUSH_MARKET_PREFIX: &str = "usdt_contract_match_market_push_order_";
pub const PUSH_NO_DEAL_PREFIX: &str = "usdt_contract_match_market_push_no_deal_";
pub const PUSH_DEEPS_PREFIX: &str = "usdt_contract_match_market_push_deeps_";
pub const PUSH_ROBOT: &str = "usdt_contract_match_market_push_robot";
pub const NEW_COIN: &str = "usdt_market_add_new_coin";
pub const NEW_COIN_GROUP: &str = "usdt_market_add_new_coin_group";
pub const PULL_GROUP: &str = "usdt_contract_match_channel_one_group";
```

- [ ] **Step 1: 对测试 NameServer 做 RocketMQ 客户端 spike**

写 `examples/rmq_ping.rs` 或 ignored 测试：建生产者，发到 scratch topic，消费一次。  
对 `192.168.0.241:9876` 运行。  
**通过标准：** 30s 内 send+receive。  
若客户端协议不匹配：换到与 broker 匹配的 Apache rocketmq-rust 版本（在 `Cargo.toml` 中文档化 pin）；ping 成功前不要接撮合 topic。

- [ ] **Step 2: 实现 `inbound` 路径**

移植 `BaseConsumer.handleMqData`：校验 → 转换 → START_QUEUE/BigNo 去重 → 入队。  
入站 body：`MqOrder` 的 JSON 数组。  
**ACK 策略：** 处理尝试后总是 ACK（Java `finally return true`）— 保持对齐。

- [ ] **Step 3: 实现 `bootstrap` 序列**

移植 `InitLoadData.initMain` 顺序：delay → list markets → filter shard → redis wipe/link → spawn worker → `initData` restore → build consumers → sleep 720s → clear START_QUEUE/BigNo。

Worker：`queue.recv() → engine.on_order → outbound`。

- [ ] **Step 4: 实现出站生产者**

将 `MatchEvent` + 深度节流（`depth_push_interval_ms`，默认 50）映射为 Java 生产者载荷（下游已解析的 `BBOrder` JSON 字段）。发送失败 → Redis 错误队列。后台任务按 `SendErrorData` 方式重试。

- [ ] **Step 5: 接线 `main`**

加载配置 → tracing → bootstrap → park。

- [ ] **Step 6: 测试环境手工冒烟（清单写在 README）**

1. 配置指向测试 RPC/Redis/RMQ。  
2. 若支持则用 `symbols_whitelist: ["one low-traffic symbol"]`，或隔离 NS。  
3. 确认恢复数量日志；一笔挂+撤能产出 push_order 消息。

- [ ] **Step 7: 提交**

```bash
git commit -am "$(cat <<'EOF'
feat(match-contract): bootstrap, symbol workers, RocketMQ in/out

EOF
)"
```

---

### Task 13: 指标 + 健康 HTTP

**文件：**
- 创建：`crates/match-contract/src/telemetry.rs`
- 创建：`crates/match-contract/src/health.rs`
- 参考：`java-contract-match/docs/opentelemetry-metrics.md`

- [ ] **Step 1: 暴露 `/healthz`（进程存活）与 `/readyz`（bootstrap 完成）**

- [ ] **Step 2: 尽可能按 Java 名称对齐发射计数器**

`match.order_events`、入站非法、order_book remove_failed、fill 直方图 — 见 `ContractMatchMetricsRecorder`。

- [ ] **Step 3: 提交**

```bash
git commit -am "$(cat <<'EOF'
feat(match-contract): health endpoints and OTel-aligned metrics

EOF
)"
```

---

### Task 14: L3 影子 + 灰度切换 runbook

**文件：**
- 创建：`docs/cutover-runbook.md`
- 创建：`docs/l3-shadow.md`
- 修改：`README.md`

- [ ] **Step 1: 文档化 L3 模式**

1. 录制交易对入站 MQ → 离线 Java golden + Rust 回放。  
2. Rust `shadow_consume: true` 配置：用**不同**消费组 `usdt_contract_match_channel_rust_shadow_group` 消费，跑引擎，**禁用全部生产者**；相对 Redis/Java 快照（若有）周期性 log/diff 深度。

- [ ] **Step 2: 切换检查清单**

1. L2 `cargo test -p match-replay` 全绿。  
2. 测试环境完整外壳冒烟。  
3. 停止交易对 S 的 Java 消费（或单交易对部署时停 Java 进程）。  
4. 确认 `usdt_contract_match_channel_one_group` 对 S 无其他消费者。  
5. 启动 Rust；等待恢复；启用生产者。  
6. 盯错误队列、深度新鲜度、订单推送延迟。  
7. 回滚：停 Rust，用相同恢复路径启 Java。

- [ ] **Step 3: 提交文档**

```bash
git commit -am "$(cat <<'EOF'
docs: L3 shadow and symbol grey cutover runbook for match-contract

EOF
)"
```

---

## 规格覆盖检查清单

| 规格项 | Task |
|-----------|------|
| Workspace / crates | 1 |
| match-protocol DTO/validate/convert | 2 |
| 价时优先簿 | 3 |
| Engine + L1 | 4–8 |
| 限价/市价/高级对齐 | 5–7 |
| 深度档位 | 8 |
| Golden L2 + replay | 9 |
| 完整外壳 MQ/Redis/RPC | 10–12 |
| Metrics/health | 13 |
| 灰度切换 / L3 | 14 |
| Spot M5 | **范围外** — 新计划 |
| 保留 Java bugs | 6–7 说明 + 来自 Java 的 golden |

---

## 自审备注

- 无 TBD 步骤；RocketMQ 风险由 Task 12 Step 1 的显式 ping 门禁处理。  
- Redis 键前缀必须在 Task 11 Step 1 从 Java `RedisKey` 复制（可执行，非开放式）。  
- 类型（`BbOrder`、`MatchEvent`、`Engine::on_order`）在 Task 2–4 引入并在后续一致复用。  
- Handler 移植有意引用 Java 文件承载主体逻辑；测试锁定可观测结果。
