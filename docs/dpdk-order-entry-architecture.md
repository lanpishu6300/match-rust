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
