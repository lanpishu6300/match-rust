# Rust Contract Match Engine Implementation Plan

**中文：** [2026-07-17-rust-match-engines.zh-CN.md](./2026-07-17-rust-match-engines.zh-CN.md)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a Rust process that can replace Java `java-contract-match` with observable-result equivalence (fills, remainders, revoke paths, depth levels), verified by golden replay, then grey-cut by symbol.

**Architecture:** Cargo workspace `match-rust` with shared `match-core` + `match-protocol`, contract binary `match-contract` (MQ/Redis/RPC shell), and `match-replay` for L1/L2 gates. Port control flow from Java handlers; preserve known Java bugs unless a separate fix task says otherwise.

**Tech Stack:** Rust 1.78+ (edition 2021), `bigdecimal`, `serde`/`serde_json`, `tokio`, `redis` (cluster), Apache RocketMQ Rust client against NameServer (test: `192.168.0.241:9876`), `reqwest`, `tracing`/`opentelemetry`, `clap`/`figment` or `config` for YAML.

**Spec:** `docs/specs/2026-07-17-rust-match-engines-design.md`

**Scope of this plan:** Contract milestones **M0–M4** only. Spot `match-spot` (M5) is a **separate plan**.

**Java port sources (read-only reference):**
- `java-contract-match/contract-match-provider/src/main/java/com/example/match/`
- `java-contract-match/contract-match-api/src/main/java/com/example/contract/match/api/`

---

## File map

| Path | Responsibility |
|------|----------------|
| `Cargo.toml` | Workspace members + shared deps |
| `crates/match-protocol/` | `MqOrder`, constants, `check_mq_order`, `type_convert`, `encode_symbol_key`, JSON parse |
| `crates/match-core/` | `OrderBook`, `Engine`, limit/market/height/fok match, depth snapshot, `MatchEvent` |
| `crates/match-replay/` | CLI + library: run input → `GoldenTrace` diff |
| `crates/match-contract/` | Binary: config, bootstrap, Redis, HTTP restore, RMQ in/out, metrics, health |
| `crates/match-spot/` | Stub crate (`lib.rs` empty module) for workspace completeness |
| `testdata/golden/` | Checked-in `*.ndjson` golden traces |
| `java-contract-match/.../test/.../GoldenTraceExporterTest.java` (or `tools/java-golden/`) | Java-side exporter producing golden files |

**Closed open items from spec:**
- Decimal: `bigdecimal` crate (Java `BigDecimal` semantics).
- Redis: `redis` crate with `cluster` feature.
- RocketMQ: Apache `rocketmq` Rust client (NameServer mode); Task 12 verifies against test NS before wiring producers.
- Restore HTTP: `POST {market_base}/contract-market/contractcoinMarketList`, `POST {order_base}/contract/entrust-list`.
- Golden exporter: JUnit test harness under `java-contract-match` that drives handlers in-process and writes NDJSON (no production code path change).

---

### Task 1: Workspace scaffold

**Files:**
- Create: `Cargo.toml`
- Create: `crates/match-protocol/Cargo.toml`
- Create: `crates/match-protocol/src/lib.rs`
- Create: `crates/match-core/Cargo.toml`
- Create: `crates/match-core/src/lib.rs`
- Create: `crates/match-replay/Cargo.toml`
- Create: `crates/match-replay/src/lib.rs`
- Create: `crates/match-contract/Cargo.toml`
- Create: `crates/match-contract/src/main.rs`
- Create: `crates/match-spot/Cargo.toml`
- Create: `crates/match-spot/src/lib.rs`
- Create: `README.md`

- [ ] **Step 1: Create workspace root**

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

- [ ] **Step 2: Create stub crates**

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

`match-core/Cargo.toml`: depends on `match-protocol`, `bigdecimal`, `thiserror`, `tracing`.

`match-core/src/lib.rs`:
```rust
//! Pure matching engine (no MQ/Redis/HTTP).
```

`match-replay/Cargo.toml`: depends on `match-core`, `match-protocol`, `serde_json`, `clap`.

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

`README.md`: one paragraph pointing at the design spec and `cargo test --workspace`.

- [ ] **Step 3: Verify build**

Run: `cd . && cargo build --workspace`

Expected: success (stub binaries/libs).

- [ ] **Step 4: Commit**

```bash
cd .
git add -A
git commit -m "$(cat <<'EOF'
chore: scaffold match-rust workspace for contract match port

EOF
)"
```

(If `match-rust` is not its own git root, commit from monorepo root including these paths.)

---

### Task 2: Protocol constants, MqOrder, validation, type_convert

**Files:**
- Create: `crates/match-protocol/src/constants.rs`
- Create: `crates/match-protocol/src/mq_order.rs`
- Create: `crates/match-protocol/src/order.rs`
- Create: `crates/match-protocol/src/validate.rs`
- Create: `crates/match-protocol/src/convert.rs`
- Create: `crates/match-protocol/src/encode.rs`
- Modify: `crates/match-protocol/src/lib.rs`
- Create: `crates/match-protocol/tests/validate_convert.rs`

**Java reference:** `Constants.java`, `BBConstants.checkMqOrder` / `typeConvert`, `CoinMarketEncode.encodeSymbolKey`

- [ ] **Step 1: Write failing tests**

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

- [ ] **Step 2: Run tests — expect FAIL**

Run: `cd . && cargo test -p match-protocol --test validate_convert`

Expected: compile errors (types/functions missing).

- [ ] **Step 3: Implement protocol modules**

Implement serde `MqOrder` with `#[serde(rename_all = "camelCase")]` field names matching Java JSON (`userId`, `trustOrderNo`, `cType`, etc.).

`check_mq_order`: port `BBConstants.checkMqOrder` rules exactly (types list, orderStatus ∈ {0,2,3}, market gear required, closePosition/startDeposit/takerRate/positionType required).

`type_convert`: port `BBConstants.typeConvert` — `symbol_key` = remove `/` + lowercase; `remaining_number = trust_number`; reject `trust_number <= 0` or `trust_price <= 0`.

`encode_symbol_key`: port `CoinMarketEncode.encodeSymbolKey` (Base64 URL without padding for non-ASCII parts).

Wire `lib.rs` with `pub use` of public items.

- [ ] **Step 4: Run tests — expect PASS**

Run: `cargo test -p match-protocol --test validate_convert`

- [ ] **Step 5: Commit**

```bash
git add crates/match-protocol
git commit -m "$(cat <<'EOF'
feat(match-protocol): add MqOrder validation and type_convert

EOF
)"
```

---

### Task 3: Order book (price-time priority)

**Files:**
- Create: `crates/match-core/src/book.rs`
- Create: `crates/match-core/src/order.rs` (engine-owned `BbOrder` wrapping/re-exporting protocol order)
- Modify: `crates/match-core/src/lib.rs`
- Create: `crates/match-core/tests/order_book_order.rs`

**Java reference:** `BBOrder.compareTo` (buy high→low, sell low→high; tie `createTime` then `trustOrderNo`)

- [ ] **Step 1: Write failing test**

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

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test -p match-core --test order_book_order`

- [ ] **Step 3: Implement `OrderBook`**

Use `BTreeSet` with comparator matching Java `compareTo` (or separate buy/sell sets with `Ord` impl on a wrapper). Provide `insert`, `remove` (by full order identity like TreeSet.remove), `best`/`first`, `is_empty`.

`remove` must use the same equality key Java uses for TreeSet membership (`compareTo == 0` when same `trustOrderNo` at same price — note Java lacks `equals`/`hashCode`; preserve TreeSet remove semantics).

- [ ] **Step 4: Run — expect PASS**

- [ ] **Step 5: Commit**

```bash
git commit -am "$(cat <<'EOF'
feat(match-core): add price-time order book

EOF
)"
```

---

### Task 4: Engine facade + MatchEvent + L1 harness

**Files:**
- Create: `crates/match-core/src/engine.rs`
- Create: `crates/match-core/src/event.rs`
- Create: `crates/match-core/src/id.rs`
- Modify: `crates/match-core/src/lib.rs`
- Create: `crates/match-core/tests/l1_limit_cross.rs`

- [ ] **Step 1: Define events**

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

`Engine` API:
```rust
pub struct Engine { /* per-symbol books inside HashMap */ }
impl Engine {
    pub fn new() -> Self;
    pub fn on_order(&mut self, order: BbOrder) -> Vec<MatchEvent>;
    pub fn depth_levels(&self, symbol: &str, side: Side, limit: usize) -> Vec<(BigDecimal, BigDecimal)>;
}
```

Deal IDs: inject `IdGenerator` trait; default impl uses atomic u64 for tests (Java uses `IDGenerator.generatorId()` — production shell can call same snowflake HTTP or local equivalent later; for equivalence of **fills**, golden compares order nos/qty/price first; `dealNo` compared when present in golden).

- [ ] **Step 2: Failing L1 test — empty book limit rests**

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

- [ ] **Step 3: Minimal `on_order` — only rest on book (no match yet)**

Enough to pass Step 2 test.

- [ ] **Step 4: Commit**

```bash
git commit -am "$(cat <<'EOF'
feat(match-core): engine facade and MatchEvent for L1 harness

EOF
)"
```

---

### Task 5: Limit match — RatherThan / Equals / LessThan + Buy/Sell loops

**Files:**
- Create: `crates/match-core/src/match_limit.rs`
- Create: `crates/match-core/src/handlers/buy.rs`
- Create: `crates/match-core/src/handlers/sell.rs`
- Create: `crates/match-core/src/handlers/mod.rs`
- Create: `crates/match-core/src/price_utils.rs`
- Modify: `crates/match-core/src/engine.rs`
- Create: `crates/match-core/tests/l1_limit_fill.rs`

**Java reference (port control flow 1:1):**
- `RatherThanHandler.java`, `EqualsHandler.java`, `LessThanHandler.java`
- `BuyHandler.java`, `SellHandler.java`
- `PriceUtils.java`, `BaseHandler.addToBook` / `removeFromOrderBook`
- Batch flush when `bbOrders.size() >= SEND_MAX_DATA` (10) — emit events in same batch boundaries

- [ ] **Step 1: Write failing cross-fill test**

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

- [ ] **Step 2: Run — expect FAIL**

- [ ] **Step 3: Port match helpers + buy/sell loops**

Port Java methods into Rust functions. Keep branch order identical (ratherThan → equals/lessThan). Preserve `removeFromOrderBook` failure behavior (log/metric hook; do not invent retries).

Wire `Engine::on_order` for `order_form == 1` (and default non-advanced) to buy/sell handlers. `order_status == REVOKE (3)` → revoke path from Buy/SellHandler.

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p match-core --test l1_limit_fill`

- [ ] **Step 5: Commit**

```bash
git commit -am "$(cat <<'EOF'
feat(match-core): port limit match RatherThan/Equals/LessThan

EOF
)"
```

---

### Task 6: Market orders + gear revoke

**Files:**
- Create: `crates/match-core/src/handlers/market.rs`
- Modify: `crates/match-core/src/handlers/buy.rs` / `sell.rs`
- Create: `crates/match-core/tests/l1_market_gear.rs`

**Java reference:** `MarketBuyHandler.java`, market branches in `BuyHandler`/`SellHandler` (gear stop; **preserve** P0-1/P0-3 behaviors from known-issues doc)

- [ ] **Step 1: Failing tests**

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

Also add case for `gear = 0` matching Java (known P0-3: may revoke immediately) — assert **Java-equivalent** outcome, not “correct” outcome.

- [ ] **Step 2–4: Port MarketBuyHandler + market loops; pass tests; commit**

```bash
git commit -am "$(cat <<'EOF'
feat(match-core): port market match and gear revoke (Java-equivalent)

EOF
)"
```

---

### Task 7: PostOnly / IOC / FOK

**Files:**
- Create: `crates/match-core/src/handlers/height_buy.rs`
- Create: `crates/match-core/src/handlers/height_sell.rs`
- Create: `crates/match-core/src/handlers/fok_buy.rs`
- Create: `crates/match-core/src/handlers/fok_sell.rs`
- Create: `crates/match-core/tests/l1_advanced.rs`

**Java reference:** `HeightBuyHandler.java`, `HeightSellHandler.java`, `FokBuyHandler.java`, `FokSellHandler.java`, `EventOrderHandler` routing

- [ ] **Step 1: Failing tests (minimum set)**

1. PostOnly that would take → revoke/`post_only`, no fill, no resting (preserve Java flash behavior only if Height handler pushes depth before revoke — match Java).
2. IOC partial fill → remainder revoked `ioc_remainder`.
3. FOK success → full fill, no remainder.
4. FOK fail → no net book change + revoke `fok_fail` (port rollback path).

- [ ] **Step 2: Port Height*/Fok* control flow 1:1 including known IOC loop quirks (P0-2)**

Document in module rustdoc: `// intentional Java parity: see docs/合约撮合已知问题梳理.md P0-2`.

- [ ] **Step 3: `cargo test -p match-core --test l1_advanced` PASS**

- [ ] **Step 4: Commit**

```bash
git commit -am "$(cat <<'EOF'
feat(match-core): port PostOnly IOC FOK handlers with Java parity

EOF
)"
```

---

### Task 8: Depth / no-deal snapshot builder

**Files:**
- Create: `crates/match-core/src/depth.rs`
- Create: `crates/match-core/tests/l1_depth.rs`

**Java reference:** `NoDealProducer` / depth aggregation (`NO_DEAL_NUMBER=20`), `DepthMapProducer` (`DEEPS_NUMBER`), price aggregation rules

- [ ] **Step 1: Test aggregated levels**

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

- [ ] **Step 2: Implement aggregation matching Java getDepth/getDeeps behavior**

- [ ] **Step 3: Commit**

```bash
git commit -am "$(cat <<'EOF'
feat(match-core): depth level aggregation for no-deal/deeps

EOF
)"
```

---

### Task 9: GoldenTrace format + match-replay + Java exporter

**Files:**
- Create: `crates/match-replay/src/trace.rs`
- Create: `crates/match-replay/src/diff.rs`
- Create: `crates/match-replay/src/main.rs`
- Create: `testdata/golden/limit_cross.ndjson` (generated)
- Create: `java-contract-match/contract-match-provider/src/test/java/com/example/match/golden/GoldenTraceExporterTest.java`
- Create: `java-contract-match/contract-match-provider/src/test/java/com/example/match/golden/GoldenTraceWriter.java`

**Golden line schema (NDJSON):**
```json
{"seq":0,"op":"input","order":{/* MqOrder camelCase */}}
{"seq":1,"op":"fill","takerOrderNo":"b1","makerOrderNo":"s1","price":"100","qty":"1","takerRemaining":"0","makerRemaining":"0","takerStatus":1,"makerStatus":1}
{"seq":2,"op":"depth","symbol":"btcusdt","bids":[["100","0"]],"asks":[]}
{"seq":3,"op":"revoke","orderNo":"x","remaining":"1","reason":"ioc_remainder"}
```

Compare numerically for decimals; ignore JSON key order.

- [ ] **Step 1: Implement `GoldenTrace` serde + `diff_traces(expected, actual) -> Vec<String>`**

- [ ] **Step 2: Java exporter test**

In-process: build `TreeSet` books / call handlers with constructed `BBOrder` list (same scenarios as L1). Write NDJSON under `testdata/golden/`.

Run (from `java-contract-match` module):
`mvn -pl contract-match-provider -Dtest=GoldenTraceExporterTest test`

Expected: files written.

- [ ] **Step 3: `match-replay` CLI**

```bash
cargo run -p match-replay -- --input testdata/golden/limit_cross.ndjson --engine rust
# reads input ops, runs Engine, diffs fill/depth/revoke lines
```

Exit code 0 on match, 1 on mismatch with printed diff.

- [ ] **Step 4: Commit golden + replay**

```bash
git add crates/match-replay match-rust/testdata \
  java-contract-match/contract-match-provider/src/test/java/com/example/match/golden
git commit -m "$(cat <<'EOF'
feat(match-replay): golden NDJSON format and Java exporter harness

EOF
)"
```

- [ ] **Step 5: Expand goldens**

Export and pass replay for: limit cross, partial fill, market+gear, postonly, ioc, fok ok/fail, cancel. Add `cargo test -p match-replay` that runs all files in `testdata/golden/`.

---

### Task 10: match-contract config + HTTP restore clients

**Files:**
- Create: `crates/match-contract/src/config.rs`
- Create: `crates/match-contract/src/rpc/market.rs`
- Create: `crates/match-contract/src/rpc/order.rs`
- Create: `crates/match-contract/src/rpc/mod.rs`
- Create: `crates/match-contract/config.example.yaml`
- Create: `crates/match-contract/tests/rpc_urls.rs`

**HTTP paths (from Feign APIs):**
- Market: `POST {market_base_url}/contract-market/contractcoinMarketList` → list VO with `coinMarket`, `originCoinMarket`, `mainStream`
- Order: `POST {order_base_url}/contract/entrust-list` body `{ "trustOrderNo": "<BigNo>", "mainStream": 0 }` → `ResponseData` code==1, paginated `rows`

Response envelope: align with `com.example.common.model.ResponseData` (`code`, `data`, …). Success `code == 1`.

- [ ] **Step 1: Config struct**

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

- [ ] **Step 2: Unit-test URL join helpers**

```rust
#[test]
fn market_list_path() {
    assert_eq!(
        market::list_url("http://m"),
        "http://m/contract-market/contractcoinMarketList"
    );
}
```

- [ ] **Step 3: Implement reqwest clients + deserialize VO/BO fields needed for bootstrap**

- [ ] **Step 4: Commit**

```bash
git commit -am "$(cat <<'EOF'
feat(match-contract): config and restore RPC HTTP clients

EOF
)"
```

---

### Task 11: Redis keys + error queue

**Files:**
- Create: `crates/match-contract/src/redis_store.rs`
- Create: `crates/match-contract/src/error_queue.rs`

**Keys (parity):**
- Match link: prefix + `redis_poc_link_list_key{symbol}` (same RedisKeyPrefixEnum behavior as Java — confirm prefix string from `RedisKey` / `RedisKeyPrefixEnum` in java-cache; copy exact key format in implementation by reading Java `RedisKey.toString()` usage).
- Depth wipe: `contract_exchange_depth:{origin}detail|trade|paint` under MARKET_KEY prefix.
- Error queue: `poc_redis_send_mq_error_data_queue`

- [ ] **Step 1: Read Java `RedisKey` / `RedisTemplateMatch` and document exact key strings in `redis_store.rs` module docs**

- [ ] **Step 2: Implement del/set/exists + list push/pop for error queue**

- [ ] **Step 3: Integration test behind `#[ignore]` requiring Redis (document `cargo test -p match-contract -- --ignored`)

- [ ] **Step 4: Commit**

```bash
git commit -am "$(cat <<'EOF'
feat(match-contract): Redis link keys, depth wipe, MQ error queue

EOF
)"
```

---

### Task 12: Bootstrap + per-symbol workers + RocketMQ in/out

**Files:**
- Create: `crates/match-contract/src/bootstrap.rs`
- Create: `crates/match-contract/src/symbol_worker.rs`
- Create: `crates/match-contract/src/mq/consumer.rs`
- Create: `crates/match-contract/src/mq/producer.rs`
- Create: `crates/match-contract/src/mq/topics.rs`
- Create: `crates/match-contract/src/inbound.rs`
- Create: `crates/match-contract/src/outbound.rs`
- Modify: `crates/match-contract/src/main.rs`

**Topics constants:**
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

- [ ] **Step 1: Spike RocketMQ client against test NameServer**

Write `examples/rmq_ping.rs` or ignored test: create producer, send to a scratch topic, consume once.  
Run against `192.168.0.241:9876`.  
**Pass criteria:** send+receive within 30s.  
If client protocol mismatch: switch to the Apache rocketmq-rust version that matches broker (document pin in `Cargo.toml`); do not proceed to wire match topics until ping works.

- [ ] **Step 2: Implement `inbound` path**

Port `BaseConsumer.handleMqData`: validate → convert → START_QUEUE/BigNo dedupe → enqueue.  
Inbound body: JSON array of `MqOrder`.  
**ACK policy:** always ACK after process attempt (Java `finally return true`) — preserve parity.

- [ ] **Step 3: Implement `bootstrap` sequence**

Port `InitLoadData.initMain` order: delay → list markets → filter shard → redis wipe/link → spawn worker → `initData` restore → build consumers → sleep 720s → clear START_QUEUE/BigNo.

Worker: `queue.recv() → engine.on_order → outbound`.

- [ ] **Step 4: Implement outbound producers**

Map `MatchEvent` + depth throttle (`depth_push_interval_ms`, default 50) to Java producer payloads (`BBOrder` JSON fields consumers already parse). On send failure → Redis error queue. Background task retries like `SendErrorData`.

- [ ] **Step 5: Wire `main`**

Load config → tracing → bootstrap → park.

- [ ] **Step 6: Manual test-env smoke (checklist in README)**

1. Point config at test RPC/Redis/RMQ.  
2. Run with `symbols_whitelist: ["one low-traffic symbol"]` if supported, or isolated NS.  
3. Confirm restore count logs, one place+cancel produces push_order message.

- [ ] **Step 7: Commit**

```bash
git commit -am "$(cat <<'EOF'
feat(match-contract): bootstrap, symbol workers, RocketMQ in/out

EOF
)"
```

---

### Task 13: Metrics + health HTTP

**Files:**
- Create: `crates/match-contract/src/telemetry.rs`
- Create: `crates/match-contract/src/health.rs`
- Reference: `java-contract-match/docs/opentelemetry-metrics.md`

- [ ] **Step 1: Expose `/healthz` (process up) and `/readyz` (bootstrap finished)**

- [ ] **Step 2: Emit counters aligned with Java names where possible**

`match.order_events`, inbound invalid, order_book remove_failed, fill histograms — see `ContractMatchMetricsRecorder`.

- [ ] **Step 3: Commit**

```bash
git commit -am "$(cat <<'EOF'
feat(match-contract): health endpoints and OTel-aligned metrics

EOF
)"
```

---

### Task 14: L3 shadow + grey cutover runbook

**Files:**
- Create: `docs/cutover-runbook.md`
- Create: `docs/l3-shadow.md`
- Modify: `README.md`

- [ ] **Step 1: Document L3 modes**

1. Record inbound MQ for symbol → offline Java golden + Rust replay.  
2. Rust `shadow_consume: true` config: consume with **different** group `usdt_contract_match_channel_rust_shadow_group`, run engine, **disable all producers**; log/diff depth periodically vs Redis/Java snapshot if available.

- [ ] **Step 2: Cutover checklist**

1. L2 `cargo test -p match-replay` green.  
2. Test-env full shell smoke.  
3. Stop Java consumption for symbol S (or stop Java process if single-symbol deploy).  
4. Ensure no other consumer in `usdt_contract_match_channel_one_group` for S.  
5. Start Rust; wait restore; enable producers.  
6. Watch error queue, depth freshness, order push lag.  
7. Rollback: stop Rust, start Java with same restore path.

- [ ] **Step 3: Commit docs**

```bash
git commit -am "$(cat <<'EOF'
docs: L3 shadow and symbol grey cutover runbook for match-contract

EOF
)"
```

---

## Spec coverage checklist

| Spec item | Task |
|-----------|------|
| Workspace / crates | 1 |
| match-protocol DTO/validate/convert | 2 |
| Price-time book | 3 |
| Engine + L1 | 4–8 |
| Limit/market/advanced parity | 5–7 |
| Depth levels | 8 |
| Golden L2 + replay | 9 |
| Full shell MQ/Redis/RPC | 10–12 |
| Metrics/health | 13 |
| Grey cutover / L3 | 14 |
| Spot M5 | **Out of scope** — new plan |
| Preserve Java bugs | 6–7 notes + golden from Java |

---

## Self-review notes

- No TBD steps; RocketMQ risk handled by explicit ping gate in Task 12 Step 1.  
- Redis key prefix must be copied from Java `RedisKey` during Task 11 Step 1 (actionable, not open-ended).  
- Types (`BbOrder`, `MatchEvent`, `Engine::on_order`) introduced in Tasks 2–4 and reused consistently later.  
- Handler ports intentionally reference Java files for bulk logic; tests lock observable outcomes.
