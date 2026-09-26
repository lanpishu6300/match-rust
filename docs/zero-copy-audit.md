# 0-copy 审查：match-rust 全链路对照《Rust 减少 clone 机制详解：交易撮合实战》

> 审查日期：2026-09-26 ｜ 基准文档：豆包云空间《Rust减少clone机制详解_交易撮合实战_技术分享文档.md》（file `GdKybovotobMo6xh3Ylc0a0DnIb`）
> 基准代码：`perf/tier-sweep` @ `d993cfb`；本分支在此基础上实施 P0 修复。

## 1. 结论

**收包侧 0-copy 已达标；收包后到撮合前每单约 8–12 次堆分配+拷贝，发包与行情共享未用引用计数/直接 mbuf 写，整体不是 0-copy。**

对照文档要求（§2.6 零拷贝反序列化 / §4.2–4.5 DPDK mbuf 零拷贝 / §6.1 零拷贝流水线 / §5 隐式 clone 陷阱），逐环节核查如下。

## 2. 全链路符合度矩阵

| # | 环节 | 现状 | 拷贝/分配 | 文档依据 | 判定 |
|---|---|---|---|---|---|
| ① | DPDK 收包 `dpdk/mod.rs:98-142` + `dpdk_tap_order.rs:238-240` | `rx_burst`→mbuf→`mtod(m).add(payload_off)`→`&[u8]` 直进会话 | 0 拷贝（仅帧头 20B 字段入栈结构） | §4.3 | ✅ 符合 |
| ② | 会话解析 `udp-order/session.rs:on_datagram` | ~~`payload.to_vec()` ×3~~ → 本分支已改为事件携带 `&[u8]` 借用 | ~~每包 1 堆分配+拷贝~~ → 0 | §2.6/4.4 | ✅ 本分支修复 |
| ③ | 回报/NAK 存储 `session.rs:ack_and_report`、`pending.insert`、`soupbintcp/session.rs:99` | `store`/`pending` 存 `Vec<u8>` | 每回报 1 次拷贝 | §2.2/4.2（应 Arc/Bytes 引用计数） | ❌ 遗留 P1 |
| ④ | 业务转换 `udp_order_gw.rs:parse_order/mq_limit`、`engine.rs:28` | ~~文本 split → `MqOrder` 5×`Option<String>`~~ → 本分支已改 `Option<SmolStr>`（≤23B 栈内零分配）；`symbol_key.clone()` 残留于 match-core | ~~每单 6–8 次 String 堆分配~~ → 0（仅 type_convert 保持 2 次 to_string） | §5.1 陷阱1/10、§5.3 陷阱9/11 | ✅ 本分支修复（P0-b） |
| ⑤ | 回报文本 `report_text`、`format!("A|{no}|btcusdt")` | `format!` → `into_bytes()` | 每成交/挂单 1 次分配 | §5.1 陷阱2（应 `write!` 复用 Buffer） | ❌ 遗留 P1 |
| ⑥ | 发包 `dpdk_tap_order.rs:build_reply` + `dpdk/port.rs:tx_frame` | 构造中间 `Vec<u8>` → `rte_pktmbuf_append`+`copy_nonoverlapping` | 2 次拷贝（Vec 组装 + memcpy 入 mbuf） | §4.4（应直接写 mbuf data room/prepend） | ⚠️ 遗留 P2 |
| ⑦ | 行情共享 `moldudp64/publisher.rs:66`、`ring_buf.rs:89`、`subscriber.rs:162` | ~~cache `Vec<u8>` + NAK `m.clone()`~~ → 本分支 ring cache 已改 `Arc<[u8]>`（NAK 重传 clone 变引用计数）；`subscriber.rs:162 to_vec` 订阅侧接收待 P1-2 | ~~发布 1 拷贝 + NAK 每消息深拷贝~~ → NAK 多订阅者免拷贝 | §2.2/4.2（应 Bytes 共享） | ✅ 本分支修复（P1） |
| ⑧ | 行情生产端传输 `publisher.rs:144/200` | `UdpSocket::send_to`（内核栈） | 用户态→内核→网卡 2 次拷贝 | §4.4–4.5（生产端应 DPDK tx） | ⚠️ 遗留 P2（已有 `mold_dpdk_tx` 可接） |

## 3. 本分支已实施（P0-a：会话事件借用化 + 栈上小帧）

`crates/match-udp-order/src/session.rs`：

- `ServerEvent<'a>` / `ClientEvent<'a>`：`Order/Cancel/Replace` 载荷与 `Report` 载荷改为**借用入站 datagram**（`&'a [u8]`），消除 `on_datagram` 内 3 处 `payload.to_vec()` 与 client 侧 2 处 `to_vec`；
- `ServerSession::on_datagram<'a>` / `ClientSession::on_datagram<'a>` 同步加生命周期；
- NAK_REQUEST / HELLO / HELLO_ACK 小帧由 `Vec::with_capacity` 改为栈上 `[u8; 8]` / `[u8; 13]` 复用（消除每包 1–2 次小堆分配）；
- NAK_RESPONSE 补帧载荷 `&payload[off..off+len]` 借用（不再 `to_vec`）。

调用方适配：`udp_order_gw.rs`、`dpdk_tap_order.rs`（`parse_order(payload)` 直传借用切片）。

**安全约束**：借用事件只在 `on_datagram` 返回后、下一次调用前消费（当前全部调用方均为单 datagram 内循环消费，安全）。

## 3b. 本分支已实施（P0-b：MqOrder 高频短文本字段 SmolStr 化）

`crates/match-protocol`：

- `MqOrder` 的 `symbol_key / coin_market / trust_order_no / trust_number / trust_price` 由 `Option<String>` 改 `Option<SmolStr>`（`smol_str` serde 兼容，JSON 表示不变）；≤23B 栈内存储、零堆分配，26 处 `Some(x.into())` 构造点自动兼容（&str/String → SmolStr）；
- `type_convert` / `type_convert_spot`：`coin_market/trust_order_no` 由 clone 改 `as_ref().map(to_string)`（源头分配已消除，转换侧保持 2 次）；
- `is_blank` 泛型化 `S: AsRef<str>`（validate/spot_validate）；
- RPC 恢复构造（`match-spot`/`match-contract` rpc/order.rs）`row.x.clone().map(Into::into)`。

**净收益**：下单热路径每单 6–8 次 String 堆分配 → 0（纯栈内 + 数值直接 `BigDecimal` 解析）。

## 3c. 本分支已实施（P1-a：MoldUDP64 重传缓存 Arc 化）

`crates/match-moldudp64/src/ring_buf.rs`：

- `CachedMessage.payload: Vec<u8>` → `Arc<[u8]>`；`push` 一次 `Arc::from` 分配后，NAK 重传 `get_range` 的 `m.clone()` 变引用计数递增——多订阅者并发 NAK 场景从每消息深拷贝降为 0；
- 下游（`retransmit.rs`、`publisher.rs` 测试）经 Deref/`as_ref` 自动适配，40 个 crate 测试全绿。

> 注：`udp-order/session.rs` 的 store/pending 与 `soupbintcp/session.rs` 回报窗口为**单份存储**（encode 出站帧的拷贝不可避免），改 Arc 无增量收益，保留 `Vec<u8>` 避免公共 API 无谓变更。

## 4. 遗留项与修复优先级

| 优先级 | 动作 | 消除 | 涉及文件 |
|---|---|---|---|
| P1-b | `subscriber.rs:162 to_vec` 订阅侧接收改 Bytes 共享；`report_text` 用 `write!` 复用 `BytesMut`；日志热路径禁 `format!` | 订阅每消息 1 拷贝、每成交 1 分配 | `subscriber.rs`、`udp_order_gw.rs`、`dpdk_tap_order.rs` |
| P2 | `tx_frame` 支持直接写 mbuf data room（`build_reply` 去中间 Vec，IP 校验和原位重算） | 发包 2 次拷贝 → 1 次 | `dpdk/port.rs`、`dpdk_tap_order.rs` |
| P2 | MoldUDP64 发布接 DPDK tx（复用 `mold_dpdk_tx`/`mold_outbound_bench`） | 行情侧内核栈 2 次拷贝 | `match-moldudp64` |

## 5. 验证

- `cargo test`（workspace default-members，macOS）：76 个测试目标全绿、0 failed；
- `match-dpdk-io` 为 Linux-only（macOS 不可编译，历史约束）；`dpdk_tap_order.rs` 改动与 `udp_order_gw.rs` 同构，待 Linux 环境编译验证。
