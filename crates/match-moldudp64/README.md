# match-moldudp64

MoldUDP64（Nasdaq 1.00）组播行情传输协议在 match-rust 工作区内的实现：**生产端（Publisher）+ 消费端（Subscriber）+ 重传服务（Retransmit Server）**，零外部依赖、全大端编码、可直接在 UDP 上运行，并为 DPDK 后端预留接口（见 `docs/moldudp64-dpdk-integration.md`）。

## 协议速览

```text
Downstream 包（20 B 固定头 + N 个消息块）：
  session_id[10]  ASCII 会话标识，同一条流恒定
  seq[8]          uint64 BE，本包第一条消息的全局序列号
  msg_count[2]    uint16 BE，消息块数量；0 = 心跳包（无消息块）
  └ 每个消息块：len[2] BE  +  payload[len]

Request 包（客户端 → 重传服务器，单播 UDP）：
  session_id[10] + start_seq[8] BE + msg_count[2] BE
```

核心机制：一条业务消息 = 一个递增全局 seq；消费端跟踪 `last_seq`，收到包首 seq 大于 `last_seq+1` 即判定丢包（gap），向重传服务器发 NAK，服务器从 publisher 共享的环形缓存补发（单播 Downstream 包，格式与组播一致）。

## 模块

| 模块 | 角色 | 说明 |
|---|---|---|
| `types` | 协议编解码 | 大端读写、会话 ID 打包、Downstream/Request 头 |
| `publisher` | 生产端 | seq 分配、单条/批量打包、心跳、环形重传缓存 |
| `subscriber` | 消费端 | 组播/单播收包、解析、seq 跟踪、gap 检测、NAK 发送 |
| `retransmit` | 重传服务 | 收 NAK，从共享缓存补发可用消息 |
| `ring_buf` | 共享缓存 | 定长环形历史，`(seq - base) % capacity` 索引 |

## 快速上手

```rust
use match_moldudp64::{MoldPublisher, MoldSubscriber, MoldRetransmitServer};
use std::net::SocketAddr;

let group: SocketAddr = "239.255.0.1:50000".parse().unwrap();
let rt: SocketAddr = "127.0.0.1:50001".parse().unwrap();

// 生产端（撮合引擎）
let pubr = MoldPublisher::new("MATCH_RUST", group, 4096).unwrap();
pubr.publish_tagged(MoldPublisher::TAG_FILL, fill_json.as_bytes()); // 成交推送
pubr.publish_tagged(MoldPublisher::TAG_DEPTH, depth_json.as_bytes()); // 深度快照
pubr.send_heartbeat().unwrap(); // 每秒一次

// 重传服务（与 publisher 共享缓存）
let server = MoldRetransmitServer::new("MATCH_RUST", rt, pubr.shared_cache()).unwrap();

// 消费端（行情网关）
let mut sub = MoldSubscriber::new(
    "MATCH_RUST",
    "0.0.0.0:50000".parse().unwrap(),
    Some("239.255.0.1".parse().unwrap()),
    Some(rt),
)?;
let mut buf = [0u8; 1500];
loop {
    if let Some(out) = sub.recv(&mut buf)? {
        if let Some(gap) = out.gap {
            sub.send_nak(gap.expected, gap.lost.min(100) as u16)?; // 补洞
        }
        for (seq, payload) in out.messages {
            // payload[0] 是消息类型 tag，后续为业务字节（match-spot 序列化的 JSON）
        }
    }
}
```

## 与 match-spot 的集成

`match-spot::Outbound` 增加可选 Mold 发布通道（不改变现有构造签名）：

```rust
// 装配时（bootstrap 或测试）：
outbound.attach_mold(MoldPublisher::new("MATCH_RUST", group, 8192)?);

// 之后每次 handle_order_result 自动组播两路：
//   MSG_TAG_FILL_ORDER = 0x01  成交/撤单推送（PushOrder JSON）
//   MSG_TAG_DEPTH      = 0x02  深度快照（HandicapDepthData JSON）
```

`attach_mold` 是 `&self` 方法，`Arc<Outbound>` 可直接调用；未 attach 时零开销（`Mutex<Option<...>>` 空判断）。

## 测试

```bash
cargo test -p match-moldudp64   # 18 项：编解码、环缓存、gap 检测、组播回环、NAK→重传
cargo test -p match-spot        # 44 项：含 attached_mold_publisher_broadcasts_fill_and_depth
```

覆盖场景：
- Publisher ↔ Subscriber 回环（单播 + 本地组播 loopback）
- 心跳识别与 seq 同步
- gap 检测（解析层，直接喂字节包）
- **gap → NAK → 重传全链路**（真 UDP：发 1..10 → 收 1..3 → NAK 4..6 → 重传补发）
- 重传只补缓存内消息（被逐出的旧消息不重放）
- 批量打包按 MTU 上限截断

## 生产注意点

1. **seq 从 1 开始**，严格递增；多进程 publish 需外部串行化（当前 Mutex 保护单进程）。
2. **环形缓存容量**决定 NAK 可回溯窗口：`capacity` 过小则旧消息被逐出，重传服务对 `eviction_floor` 之前的请求返回空。
3. **MTU**：单包默认上限 1472 B（IPv4 组播），禁止 IP 分片；超大消息走单条独立包并在业务层分片。
4. **心跳 seq 语义**：publisher 心跳携带"下一条待发 seq"；subscriber 同步为 `seq - 1`，与数据包推进的 last_seq 一致。
5. **DPDK**：macOS 开发机走内核 UDP；Linux 生产可把 `publisher.socket` / `subscriber.socket` 换成 DPDK mbuf 收发（FFI），协议层零改动，见 DPDK 文档。
