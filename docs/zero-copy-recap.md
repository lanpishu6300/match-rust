# match-rust 0-copy 全链路优化复盘（代码级）

> 分支：
>
> `perf/zero-copy-p0`
>
> 　基准文档：《Rust 减少 clone 机制详解_交易撮合实战.md》（豆包云盘）　审计报告：
>
> `docs/zero-copy-audit.md`
> 结论：
>
> **收包 0 拷贝、订单 / 回报热路径 0 堆分配、发包 0 中间拷贝**
>
> ；余下拷贝均属协议栈固有或有意的单份存储设计。

## 1. 优化前全链路拷贝 / 分配清单



| # | 环节          | 位置（优化前）                                                  | 每次开销                          |
| - | ----------- | -------------------------------------------------------- | ----------------------------- |
| ① | 收包缓冲        | `dpdk/port.rs` `rx_burst`                                | 0（mbuf 直传切片，原本达标）             |
| ② | 会话事件载荷      | `udp-order/session.rs` `ServerEvent::Order`              | 5 处 `to_vec` 堆拷贝              |
| ③ | 回报 / NAK 存储 | `session.rs` `pending.insert`、`soupbintcp/session.rs:99` | 每回报 1 次拷贝                     |
| ④ | 订单字符串字段     | `MqOrder`（`String`×5）                                    | 每单 5 次堆分配（`SmolStr` 后 0）      |
| ⑤ | 回报文本        | `report_text`/`format!`                                  | 每成交 / 挂单 1 次分配 + 1 次二次封装      |
| ⑥ | 发包          | `build_reply` + `tx_frame`                               | 2 次拷贝（Vec 组装 + memcpy 入 mbuf） |
| ⑦ | 行情共享        | `publisher.rs:66`/`ring_buf.rs:89`/`subscriber.rs:162`   | 发布 1 拷贝 + NAK / 订阅每消息 1 拷贝    |
| ⑧ | 行情生产端传输     | `publisher.rs:144/200` `UdpSocket::send_to`              | 内核栈 2 次拷贝                     |
| ⑨ | 转换侧         | `convert.rs:39-40`/`spot_convert.rs:40-41`               | 每单 2 次 `to_string`            |
| ⑩ | 引擎内部        | `match_limit.rs` `symbol_key.clone()` 等                  | 每次事件构造 1 次 String 深拷贝         |

## 2. 已实施优化（6 项，按 commit 顺序）

### P0-a　会话事件借用化（commit `64a5f3b`）



```
// crates/match-udp-order/src/session.rs

// 前：ServerEvent::Order(u64, Vec\<u8>)  ← 收包侧 5 处 payload.to\_vec()

// 后：ServerEvent::Order(u64, &'a \[u8])  ← datagram 切片直传
```



* `ServerEvent<'a>`/`ClientEvent<'a>` 载荷改借用 `&'a [u8]`；`NAK`/`HELLO`/`HELLO_ACK` 控制帧改栈上 `[u8;8]/[u8;13]`；

* `udp_order_gw`/`dpdk_tap_order` 直传切片，收包侧不再产生 owned 拷贝；

* 消除：收包后每单 5 处堆拷贝（②）。

### P0-b　MqOrder 热字段 SmolStr 化（commit `98599ee`）



```
// crates/match-protocol/src/mq\_order.rs

symbol\_key:   Option\<String> → Option\<SmolStr>

coin\_market:  Option\<String> → Option\<SmolStr>

trust\_order\_no: Option\<String> → Option\<SmolStr>

trust\_number: Option\<String> → Option\<SmolStr>

trust\_price:  Option\<String> → Option\<SmolStr>
```



* 短字段（<23B）栈内联，无堆分配；`is_blank` 泛型化 `S: AsRef<str>`；RPC 恢复构造 `map(Into::into)`；

* 消除：下单热路径每单 6–8 次 String 分配 → 0（④）。

### P1-a　MoldUDP64 重传缓存 Arc 化（commit `b85efc9`）



```
// crates/match-moldudp64/src/ring\_buf.rs

pub struct CachedMessage {

&#x20;   pub seq: u64,

&#x20;   pub payload: Arc<\[u8]>,   // 前：Vec\<u8>

}

pub fn push(\&mut self, seq: u64, payload: Vec\<u8>) {

&#x20;   self.slots\[self.write\_pos] = Some(CachedMessage { seq, payload: Arc::from(payload), ... });

}
```



* `push` 一次 `Arc::from` 分配；NAK 重传 `get_range` 的 `m.clone()` 变引用计数递增；

* 多订阅者并发 NAK：每消息深拷贝 → 0（⑦ 前半）。

### P1-b　订阅解析借用化 + 回报文本复用缓冲（本 commit）



```
// crates/match-moldudp64/src/subscriber.rs

pub struct ParseOutcome<'a> { pub messages: Vec<(u64, &'a \[u8])> }  // 前：Vec<(u64, Vec\<u8>)>

pub fn parse\_packet<'a>(\&mut self, buf: &'a \[u8]) -> ParseOutcome<'a>

pub fn recv<'a>(\&mut self, buf: &'a mut \[u8]) -> io::Result\<Option\<ParseOutcome<'a>>>

// 解析时：messages.push((seq, \&ptr\[..len]))  —— 不再 to\_vec
```



```
// crates/match-udp-order/src/bin/udp\_order\_gw.rs（dpdk\_tap\_order.rs 同构）

fn report\_text(ev: \&MatchEvent, out: \&mut Vec\<u8>) {

&#x20;   use std::io::Write as \_;

&#x20;   out.clear();

&#x20;   match ev {

&#x20;       MatchEvent::Fill { .. } => { out.push(REPORT\_EXECUTED);

&#x20;           let \_ = write!(out, "E|{taker\_order\_no}|{maker\_order\_no}|{price}|{qty}"); }

&#x20;       // ...

&#x20;   }

}

// 调用处：循环外一次 \`let mut rp = Vec::with\_capacity(64);\`，每次 clear 后重写
```



* 订阅消息块借用入站 datagram（`ParseOutcome<'a>`，零拷贝）；测试辅助 `drain` 保留 owned 语义；

* `write!` 直写复用缓冲，去掉 `format!` 中间 String 与二次 `Vec::with_capacity+extend`；

* 消除：订阅每消息 1 拷贝、每成交 / 挂单 1 分配 + 1 二次封装（⑤、⑦ 后半）。

### P2　发包直写 mbuf data room（本 commit，⚠ Linux 验证）



```
// crates/match-dpdk-io/src/dpdk/port.rs —— 新增

pub unsafe fn tx\_frame\_from(

&#x20;   port: u16, pool: \*mut rte\_mempool, capacity: u16,

&#x20;   build: impl FnOnce(\&mut \[u8]) -> usize,

) -> Result<(), String> {

&#x20;   // alloc → append → 直接写 data room → 收窄 pkt\_len/data\_len → tx\_burst

}
```



```
// crates/match-dpdk-io/src/bin/dpdk\_tap\_order.rs —— build\_reply → tx\_reply

unsafe fn tx\_reply(port: u16, pool: \*mut rte\_mempool, f: \&Frame, payload: &\[u8]) -> Result<(), String> {

&#x20;   let total = 14 + 20 + 8 + payload.len();

&#x20;   tx\_frame\_from(port, pool, total as u16, |b| {

&#x20;       // Eth/IPv4/UDP 头原位写；IP 校验和 1's complement 计算后回填 b\[24..26]

&#x20;       b\[42..42 + payload.len()].copy\_from\_slice(payload);

&#x20;       total

&#x20;   })

}
```



* 原 `build_reply`（组装 `Vec<u8>`）+ `tx_frame`（`copy_nonoverlapping` 入 mbuf）= 2 次拷贝 → 0 次中间拷贝；

* `tx_frame` 保留（`mold_dpdk_tx` 等行情 bin 仍用），`tx_frame_from` 供热路径直写（⑥）。

### 架构级　BbOrder / MatchEvent 热字段 SmolStr 化（本 commit）



```
// crates/match-protocol/src/order.rs

pub symbol\_key: SmolStr;      // 前：String

pub coin\_market: SmolStr;

pub trust\_order\_no: SmolStr;

// crates/match-core/src/event.rs —— MatchEvent 热字段

Fill   { symbol, taker\_order\_no, maker\_order\_no }: SmolStr

Revoke { order\_no, symbol }: SmolStr

// price/qty/remaining/reason 保留 String（来自 BigDecimal 格式化，无可消除分配）
```



* `convert.rs`/`spot_convert.rs`：`map(|s| s.to_string())?` → `clone()?`（栈拷贝）；`symbol_key` 生成 `...into()`；

* `engine.rs`：`books: HashMap<SmolStr, OrderBook>`（`get(&str)` 走 `Borrow<str>`，`entry` 直传）；

* `height_buy/sell.rs`：loop 参数 `order_no: SmolStr`，`revoke_by_no(book, order_no.as_str(), …)`；

* `match_limit.rs`：`symbol: symbol.into()`；`match-replay`、`match-contract`、`match-spot` 适配 `as_str()`/`to_string()`（业务出站结构保持 `String`）；

* 消除：转换侧每单 2 次 `to_string` 分配、事件构造每次 `symbol_key` String 深拷贝（⑨、⑩）。

## 3. 文档对照 P0 三项（对齐云盘《Rust 减少 clone 机制详解》新版 2.5/5.2/5.15/8.3 节）

> 本轮（perf/zero-copy-p0 收尾后）对照云盘 1364 行新版基准文档逐项核查，剩余三类"每单/每档都发生"的深拷贝与分配已清除：

### P0-①　撮合产出 `Vec<MatchEvent>` → `SmallVec<[MatchEvent; 8]>`

* 文档依据：§8.3（无堆分配集合，P99 容量取 2 的幂）/ §2.5 最佳实践 3（成交对手方通常 ≤ 5，选 8 内联）；
* 改动：`match_limit.rs` / `match_market.rs` / `handlers/height_buy.rs` / `height_sell.rs` / `fok_buy.rs` / `fok_sell.rs` / `engine.rs::on_order` 全部签名 `Vec<MatchEvent>` → `SmallVec<[MatchEvent; 8]>`；单元素构造用 `smallvec![…]`；
* 消除：**每笔订单 1 次事件列表堆分配**（成交数 ≤ 8 时全程栈内联）；
* 下游：`symbol_worker` `&events` 经 `Deref<Target=[MatchEvent]>` 零改动兼容；bench 标注改为推断类型。

### P0-②　深度/挂单快照全量 clone → 引用聚合

* 文档依据：§5.2 陷阱 5（"为快照 clone 整个订单簿"是点名反模式）；
* 改动：`depth_levels_from_orders(impl IntoIterator<Item = &'a BbOrder>)`；`OrderBook::depth_levels` 传 `iter().map(|e| &e.0)`，聚合时只对**输出档位** clone price/qty 两个 BigDecimal；
* 消除：**每档位深拷贝整单（9 个 BigDecimal + 3 SmolStr）** → 每档仅 2 个 BigDecimal 输出拷贝；`match-contract/outbound.rs:178` 行情深度生产链路直接受益。

### P0-③　撤单整单 clone → 排序字段轻量键 + `BTreeSet::take`

* 文档依据：§5.15（不派生 Clone / 不必要深拷贝）、§2.7 索引思想；
* 改动：`BbOrder::removal_key()` 只克隆排序字段（`trust_price` + `create_time` + `trust_order_no`，其余 `Default`）；`remove` / `remove_by_order_no` 用 `BTreeSet::take(&BuyEntry(key))` 将挂单**原样 move 出**（O(log n) 移除）；
* 消除：**每次撤单/Revoke 整单深拷贝（9 BigDecimal）** → 1 个 BigDecimal（排序键）clone；附带 `take` 比 `find+clone+remove` 少一次整单 clone 且移除路径 O(log n)。

## 4. 有意保留（不视为遗留）



| 项                                                                         | 理由                                                                |
| ------------------------------------------------------------------------- | ----------------------------------------------------------------- |
| `udp_order/session.rs` store/pending、`soupbintcp/session.rs` 窗口 `Vec<u8>` | 单份存储；encode 出站帧的拷贝不可避免，改 `Arc` 无增量收益，避免公共 API 无谓变更                |
| `MatchEvent.price/qty/taker_remaining/...` 保持 `String`                    | 来自 `BigDecimal` 格式化（`dec_str`），生成本身需 1 次分配，无规避路径                  |
| 内核 UDP 收包（`udp_order_gw` 非 DPDK 路径）                                       | 内核→用户 1 次拷贝属协议栈固有；`SO_BUSY_POLL`/DPDK 用户态收包已缓解（生产用 DPDK）          |
| 行情生产端 `publisher.rs` 内核 `send_to`                                         | 库保持传输无关；DPDK 出口由 `match-dpdk-io/src/bin/mold_dpdk_tx.rs` 承担（见 §5） |

## 4. 收益量化（每笔订单全链路）



| 阶段              | 优化前分配 / 拷贝            | 优化后                         |
| --------------- | --------------------- | --------------------------- |
| 收包（DPDK）        | 0（mbuf 直传）            | 0                           |
| 会话事件（②）         | 5 次堆拷贝                | 0（借用切片）                     |
| 订单字符串（④）        | 6–8 次 String 分配       | 0（SmolStr 栈内）               |
| 转换侧（⑨）          | 2 次 to\_string 分配     | 0                           |
| 事件构造（⑩）         | 1 次 symbol String 深拷贝 | 0（SmolStr clone）            |
| 回报编码（⑤）         | 1 分配 + 1 二次封装         | 0（write! 复用）                |
| 发包（⑥）           | 2 次拷贝                 | 0（直写 mbuf）                  |
| NAK 重传（⑦）       | 每订阅者 1 深拷贝            | 0（Arc 计数）                   |
| **合计（下单→回报闭环）** | **约 15–19 次堆分配 / 拷贝** | **0 次（除 BigDecimal 数值格式化）** |

## 5. 部署提示



* **行情 DPDK 出口**：`match-dpdk-io/src/bin/mold_dpdk_tx.rs` 已具备「MoldUDP64 报文 → 真实 DPDK tx（net\_pcap PMD / 物理 NIC）」完整链路（构造 DownstreamHeader 帧 → `setup_port` → `tx_frame`）；生产部署将行情发布切到该路径即可，内核 `MoldPublisher` 保留为可移植回退。

* **Linux 待验证**（macOS 不可编译 `match-dpdk-io`）：`port.rs::tx_frame_from`、`dpdk_tap_order.rs::tx_reply` 与 report\_text `write!` 改动，建议在 Linux / 容器（`DPDK_EAL_NATIVE=1` 或 pcap vdev）跑 `mold_dpdk_bench`/`dpdk_tap_order` 回归。

## 6. 验证矩阵



| 验证                                                                   | 结果                      |
| -------------------------------------------------------------------- | ----------------------- |
| `cargo test`（workspace default-members，macOS）                        | 76 目标全绿、0 failed        |
| `cargo check --workspace --exclude match-dpdk-io`                    | 0 error（存量 warning 未列入） |
| `match-moldudp64`（ring Arc + subscriber 借用 + 集成测试适配）                 | 40 + 6 + 1 全绿           |
| `match-spot`/`match-core`/`match-contract`/`match-replay`（SmolStr 链） | 全部通过                    |
| `match-dpdk-io`（Linux-only）                                          | ⚠ 待 Linux 编译与 pcap 回归   |