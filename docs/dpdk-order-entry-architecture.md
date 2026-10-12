# 下单链路 DPDK 接入架构（SoupBinTCP / OUCH）

> 结论先行：**生产订单接入默认走内核 TCP + 本仓库 `match-soupbintcp`；DPDK 的价值在接收端批量解码与延迟抖动消除。**
> 本文档给出三种落地形态，并标注已验证/待验证边界。

## 1. 三种接入形态

### 形态 A：内核 TCP（推荐生产默认）—— 已验证 ✅

```
交易所 ──TCP──▶ 内核协议栈 ──epoll──▶ match-soupbintcp 解析 ──▶ 撮合引擎
                                          │
                                          └─ ServerSession.enqueue → Sequenced Data 回报
```

- 优点：可靠、省事、会话重传由重登录语义天然覆盖；TCP 栈成熟度碾压用户态方案。
- 实测（loopback）：login RTT 95.7µs，order→accept p50 56.4µs，单连接串行 16.6k 对/s。
- 适用：订单量 < 数万笔/s 的绝大多数场景。

### 形态 B：DPDK 收包 + 用户态 TCP 栈 —— 部分验证 ✅（DPDK 收包侧）

```
交易所 ──TCP──▶ 网卡 ──DPDK rx_burst──▶ 用户态 TCP 栈(mTCP/F-Stack) ──▶ match-soupbintcp 解析
                                                                              │
                                                                              └─ 回报经 DPDK tx 或内核回发
```

- 已验证：`mold_soup_dpdk`（net_pcap 回放 10,000 帧，DPDK rx → parse_stream → ClientSession，seq 断言 PASS）。
- 未验证：真实用户态 TCP 栈集成（mTCP/F-Stack 与 DPDK 的绑定、TCP 重传/拥塞控制）。
- 收益：消除内核中断/锁竞争带来的接收延迟抖动（µs 级），适合对下单延迟尾延迟敏感的高频场景。
- 成本：TCP 栈维护成本高；连接可靠性（重传/拥塞）需要自行兜底。

### 形态 C：FPGA / 智能网卡 —— 超出本仓库范围

交易所（如 NASDAQ）对 FPGA 方案强制要求 10/40GbE 与专用硬件，软件侧无法覆盖，略。

## 2. 推荐落地方案（最小成本路径）

```
阶段1（生产可用，已完成 ✅）
  match-soupbintcp crate（纯 std 协议层）
  + 内核 TCP 接入：TcpListener/TcpStream + parse_stream 粘包处理 + ServerSession
  + 重登录重放：store 窗口 8192，clamp 语义与 MoldUDP64 一致
  └─ 单连接 16.6k 对/s，p50 56µs —— 已容器实测

阶段2（性能翻倍，协议层已就绪）
  回报批量：ServerSession.enqueue 已支持批量取数
  → 每 TCP 段聚合 N 条 Accepted（参考 mold_outbound_bench：批量 1722 万 vs 单条 88.7 万 msg/s）

阶段3（延迟抖动消除，需 Linux 真机）
  DPDK rx + 用户态 TCP 栈 + match-soupbintcp
  → 需真机 MLX5/i40e 验证（macOS 无条件），见 docs/moldudp64-dpdk-integration.md §10
```

## 3. 代码路径（本仓库）

```
下单入站（新）
  crates/match-soupbintcp/src/packet.rs    帧解析（粘包/拆包）
  crates/match-soupbintcp/src/session.rs   Server/Client 会话状态机（登录/seq/心跳/重放）
  crates/match-soupbintcp/src/ouch.rs      OUCH 4.2 定长消息编解码
  crates/match-dpdk-io/src/bin/soup_tcp_bench.rs   内核 TCP 端到端验证（Linux）
  crates/match-dpdk-io/src/bin/mold_soup_dpdk.rs   DPDK rx + SoupBinTCP 解析验证（Linux）

行情入站（已有，对照）
  crates/match-moldudp64/                  MoldUDP64 解析
  crates/match-dpdk-io/src/bin/mold_dpdk_rx.rs   DPDK 收流（--nic 真机模式）
```

## 4. 验证矩阵与边界

| 项 | 状态 | 位置 |
|---|---|---|
| SoupBinTCP 帧编解码/粘包/拆包 | ✅ macOS 6/6 单测 | match-soupbintcp |
| 内核 TCP 端到端（登录/订单/回报/心跳/重放） | ✅ 容器 N=50,000 | soup_tcp_bench |
| 重放窗口 clamp（too-old 语义） | ✅ 容器（granted=41809, 重放 8192） | soup_tcp_bench |
| DPDK rx + SoupBinTCP 解析 | ✅ 容器 10,000 帧 | mold_soup_dpdk |
| 用户态 TCP 栈集成 | ⏳ 未实现（需 Linux 真机评估 mTCP/F-Stack） | — |
| 真实 NIC 收流 | ⏳ 需真机（代码已就绪：--nic + realnic.sh） | mold_dpdk_rx |
| 与真实撮合引擎对接 | ⏳ bench 为回声模式 | — |

## 5. 一句话设计哲学

**MoldUDP64 链路的 DPDK 是"零成本直通"（协议就是为组播设计的）；
SoupBinTCP 链路的 DPDK 是"搭桥后的解码加速"—— 别为了 DPDK 而 DPDK，先吃透会话重放语义，再谈微秒。**

## 6. 形态 D：DPDK + 私有 UDP 下单（match-udp-order）—— 已验证 ✅（2026-09-21）

```
客户端 ──UDP──▶ [DPDK net_pcap/virtio PMD] ──rx_burst──▶ ServerSession(seq/ACK/NAK) ──▶ 撮合引擎
                                                                                        │
   ◀──UDP── [DPDK tx_burst] ◀── REPORT / ORDER_ACK / NAK_REQUEST ────────────────────────┘
```

- 私有 9B 帧头（session u32BE | seq u32BE | type u8），双向独立 seq + 显式 ACK/NAK，无 length 前缀（UDP datagram 即消息边界）。
- 实测（云 VM 2 核 3GB，net_pcap0 文件回放，处理侧延迟 = 订单帧 → REPORT 时间戳间隔）：
  - 5k / 20k / 50k 单：**100% 交付**（15001 / 60001 / 150001 帧全回），p50 1.0µs / p99 2.0µs 恒定，吞吐 165~252k/s（pcap 文件 IO 瓶颈，非 PMD）。
  - body 32B→1024B：延迟/吞吐几乎不变（处理侧与 body 无关）。
  - 故障注入（drop/corrupt 1 帧）：缺帧订单不处理（金融安全）+ **NAK_REQUEST(0x20) 主动请求重传**（同 gap 只发一次防风暴）——已修并验证。
- 真实环境口径：**p99 2.0µs 是处理侧（引擎内部）承诺**；端到端同机房约 5-12µs（估算，含 NIC DMA + 链路，需真机验证），跨机房加网络 RTT。
- 无特权部署要点（云 VM 用户级编译）：build.rs 支持 `DPDK_HOME`；bridge.c 需 `-mssse3`；EAL `--no-huge --no-pci -m 512`；libpcap 头取自源码 + 自写 pcap.pc。

> 与形态 B 的关系：形态 D 是**协议层即 UDP**（无 TCP 状态机），DPDK 直通收益最完整；形态 B 的 SoupBinTCP 仍需用户态 TCP 栈搭桥。生产选型：低延迟自营链路可评估形态 D，外部交易所接入仍走形态 A（内核 TCP + SoupBinTCP/iLink）。
