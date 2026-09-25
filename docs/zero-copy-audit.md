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
| ④ | 业务转换 `udp_order_gw.rs:parse_order/mq_limit`、`engine.rs:28` | 文本 split → `MqOrder` 5×`Option<String>` + `symbol_key.clone()` | 每单 6–8 次 String 堆分配 | §5.1 陷阱1/10、§5.3 陷阱9/11 | ❌ 遗留 P0-b/P1 |
| ⑤ | 回报文本 `report_text`、`format!("A|{no}|btcusdt")` | `format!` → `into_bytes()` | 每成交/挂单 1 次分配 | §5.1 陷阱2（应 `write!` 复用 Buffer） | ❌ 遗留 P1 |
| ⑥ | 发包 `dpdk_tap_order.rs:build_reply` + `dpdk/port.rs:tx_frame` | 构造中间 `Vec<u8>` → `rte_pktmbuf_append`+`copy_nonoverlapping` | 2 次拷贝（Vec 组装 + memcpy 入 mbuf） | §4.4（应直接写 mbuf data room/prepend） | ⚠️ 遗留 P2 |
| ⑦ | 行情共享 `moldudp64/publisher.rs:66`、`ring_buf.rs:89`、`subscriber.rs:162` | cache `Vec<u8>` + NAK `m.clone()` + 订阅 `to_vec` | 发布 1 拷贝 + NAK/订阅每消息 1 拷贝 | §2.2/4.2（应 Bytes 共享） | ❌ 遗留 P1 |
| ⑧ | 行情生产端传输 `publisher.rs:144/200` | `UdpSocket::send_to`（内核栈） | 用户态→内核→网卡 2 次拷贝 | §4.4–4.5（生产端应 DPDK tx） | ⚠️ 遗留 P2（已有 `mold_dpdk_tx` 可接） |

## 3. 本分支已实施（P0-a：会话事件借用化 + 栈上小帧）

`crates/match-udp-order/src/session.rs`：

- `ServerEvent<'a>` / `ClientEvent<'a>`：`Order/Cancel/Replace` 载荷与 `Report` 载荷改为**借用入站 datagram**（`&'a [u8]`），消除 `on_datagram` 内 3 处 `payload.to_vec()` 与 client 侧 2 处 `to_vec`；
- `ServerSession::on_datagram<'a>` / `ClientSession::on_datagram<'a>` 同步加生命周期；
- NAK_REQUEST / HELLO / HELLO_ACK 小帧由 `Vec::with_capacity` 改为栈上 `[u8; 8]` / `[u8; 13]` 复用（消除每包 1–2 次小堆分配）；
- NAK_RESPONSE 补帧载荷 `&payload[off..off+len]` 借用（不再 `to_vec`）。

调用方适配：`udp_order_gw.rs`、`dpdk_tap_order.rs`（`parse_order(payload)` 直传借用切片）。

**安全约束**：借用事件只在 `on_datagram` 返回后、下一次调用前消费（当前全部调用方均为单 datagram 内循环消费，安全）。

## 4. 遗留项与修复优先级

| 优先级 | 动作 | 消除 | 涉及文件 |
|---|---|---|---|
| P0-b | `parse_order`/`MqOrder` 去 String 链：`&str` 切分 + SmolStr/内联 symbol；`BbOrder` 直构或 symbol intern | 每单 6–8 次 String | `udp_order_gw.rs`、`dpdk_tap_order.rs`、`match-protocol`、`match-core` |
| P1 | 重传窗口/NAK 缓存改 `Arc<[u8]>`/`bytes::Bytes`（clone 变引用计数） | 回报/NAK 每消息拷贝 | `udp-order/session.rs`、`soupbintcp/session.rs`、`moldudp64/ring_buf.rs` |
| P1 | `report_text` 用 `write!` 复用 `BytesMut`；日志热路径禁 `format!` | 每成交 1 分配 | `udp_order_gw.rs`、`dpdk_tap_order.rs` |
| P2 | `tx_frame` 支持直接写 mbuf data room（`build_reply` 去中间 Vec，IP 校验和原位重算） | 发包 2 次拷贝 → 1 次 | `dpdk/port.rs`、`dpdk_tap_order.rs` |
| P2 | MoldUDP64 发布接 DPDK tx（复用 `mold_dpdk_tx`/`mold_outbound_bench`） | 行情侧内核栈 2 次拷贝 | `match-moldudp64` |

## 5. 验证

- `cargo test`（workspace default-members，macOS）：76 个测试目标全绿、0 failed；
- `match-dpdk-io` 为 Linux-only（macOS 不可编译，历史约束）；`dpdk_tap_order.rs` 改动与 `udp_order_gw.rs` 同构，待 Linux 环境编译验证。
