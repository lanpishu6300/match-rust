# 金融组播/订单接入规范全景对比与 DPDK 适配性

> 覆盖：MoldUDP64、SoupBinTCP、iLink 3.0（CME）、FAST、ITCH、OUCH、FIX/QuickFIX 及 UDP 数据报协议（17）相关变体；
> 重点回答两个问题：①业界还有哪些类似的金融组播规范；②哪些有 DPDK 社区支持 / 适合 DPDK。

## 1. 规范地图：谁在传输层，谁在业务层

```
会话/传输层（带序列号、心跳、重传语义）
├── MoldUDP64      NASDAQ 行情组播（单向，显式 seq，NAK 重传）
├── SoupBinTCP     NASDAQ 订单接入（TCP 双向，隐式 seq，重登录重放）  ← 本仓库已实现
├── iLink 3.0      CME 订单接入（TCP + SBE 编码 + 序列号 + 丢包重传 + 心跳）
└── FAST (FIX Adapted for STreaming)  FIX 组织的高效编码（多为 UDP 组播载体）

业务消息层（可放上面任意传输）
├── ITCH           NASDAQ 行情消息（放 MoldUDP64）
├── OUCH           NASDAQ 下单消息（放 SoupBinTCP）
├── SBE (Simple Binary Encoding)  iLink 3.0 的编码格式（FIX 组织二进制化）
└── FIX/FAST 消息   各所通用

传统/其余
├── FIX 4.x (TCP)   最广泛但不适合极低延迟（文本、会话状态复杂）
├── OUCH 4.1/4.2 (SoupBinTCP 3.x 之前用 4.x 文本版 SoupTCP)
└── 各交易所私有组播：CME MDP 3.0、Eurex T7 (MDI)、LSE Millennium、OSE、HKEX ORS
```

## 2. 逐规范详表

### 2.1 MoldUDP64（NASDAQ，行情组播）— 已实现 + DPDK 验证

| 维度 | 细节 |
|---|---|
| 传输 | UDP 组播（单向），消息头 `[session 10][seq 4BE][msg_count 1][msg_len 2BE]*n` |
| 序列号 | 每消息显式 4B BE，首包=1 |
| 可靠性 | 客户端 NAK（request retransmission）请求缺失区间，服务端单播补发 |
| 心跳 | 空 payload 消息（不占 seq） |
| 典型承载 | ITCH 5.0 行情（FPGA 版仅支持 MoldUDP64） |
| **DPDK 适配** | **极佳**：单向无状态、包级 seq 校验、多队列轮询天然匹配；本仓库实测 rx 810 万 msg/s（123ns/帧） |
| 社区支持 | 无官方 DPDK 库，但协议极简，自研实现普遍（本仓库为范例） |

### 2.2 SoupBinTCP（NASDAQ，订单接入）— 本仓库已实现

| 维度 | 细节 |
|---|---|
| 传输 | TCP 双向；帧 `[len 2BE][type][payload]` |
| 序列号 | 隐式（Sequenced Data 逐包+1），Login Accepted 给起始 |
| 可靠性 | 重登录 `requested_sequence` → 服务端重放消息存储（会话快照） |
| 心跳 | 双向 1s 保活（R/H） |
| 典型承载 | OUCH 下单、ITCH 行情（TCP 模式） |
| **DPDK 适配** | **间接**：需用户态 TCP 栈（mTCP/F-Stack/Seastar）后接入本 crate 解析器；连接管理本身内核 TCP 更稳 |
| 社区支持 | 多语言开源实现（node/go/java），DPDK 侧无专用库 |

### 2.3 iLink 3.0（CME，订单接入）

| 维度 | 细节 |
|---|---|
| 传输 | TCP（CME 订单接入标准），**SBE 编码**（FIX 组织 Simple Binary Encoding） |
| 会话 | 序列号（会话级）、**丢包重传**（消息级重传请求）、心跳 |
| 可靠模型 | 与 SoupBinTCP 同类：TCP 可靠流 + 会话级快照/重传，但用 SBE 提高编码密度 |
| 登录 | 二进制登录/Logon（含 NextSeqNo、LastSeqNo，类似 FIX 会话恢复） |
| **DPDK 适配** | 同 SoupBinTCP：间接（需 TCP 栈）；**SBE 解码器适合 DPDK 收包后批量解码**（无锁、定长字段） |
| 社区支持 | CME 官方提供 SBE Java/C++ 编解码库；DPDK 无官方集成 |

### 2.4 FAST（FIX Adapted for STreaming）

| 维度 | 细节 |
|---|---|
| 传输 | 常用于 UDP 组播（行情扇出），也可 TCP |
| 编码 | 字段级压缩（模板 + 增量 + 前值引用），解码有状态（需维护前值表） |
| 序列号/心跳 | 由外层协议（MoldUDP64 等）提供 |
| **DPDK 适配** | 差：解码器有状态、模板查找、位级操作 —— 状态维护破坏无状态轮询流水线；DPDK 侧收益低 |
| 社区支持 | 无 DPDK 专用库 |

### 2.5 ITCH / OUCH（业务消息，非传输层）

| 协议 | 消息特征 | DPDK 适配 |
|---|---|---|
| ITCH 5.0 | 定长/变长混合，BE 整数 | 极佳（配合 MoldUDP64） |
| OUCH 4.2 | 全部定长（19–66B） | **极佳**：定长 + BE 整数 = SIMD/无分支解析；本仓库 `ouch.rs` 已实现 |

### 2.6 其他交易所私有规范（参考）

| 交易所 | 协议 | 特征 | DPDK 适配 |
|---|---|---|---|
| CME | MDP 3.0 | 行情组播 + 增量刷新，SBE 编码 | 佳（MoldUDP64 风格） |
| Eurex | T7 MDI | 组播 + 快照/增量，二进制 | 佳 |
| LSE | Millennium | 组播行情 | 佳 |
| HKEX | ORS | 订单路由（TCP）+ 行情组播 | 佳（组播侧） |
| 上交所/深交所 | SSE/Level-1/2 组播 | 私有组播行情 | 佳 |

## 3. DPDK 适配性矩阵（结论）

| 协议 | 传输 | 有 DPDK 官方支持? | 适合 DPDK? | 方式 |
|---|---|---|---|---|
| MoldUDP64 | UDP 组播 | 无官方 | ★★★★★ | 直接 rx_burst 轮询（已实现） |
| SoupBinTCP | TCP | 无官方 | ★★★（需 TCP 栈） | DPDK 收包 + 用户态 TCP + 本 crate 解析 |
| iLink 3.0 / SBE | TCP | 无官方（SBE 库官方有） | ★★★☆ | DPDK 收包 + 用户态 TCP + SBE 批量解码 |
| FAST | UDP/TCP | 无 | ★★ | 有状态解码器，收益低 |
| ITCH | 组播/TCP | 无 | ★★★★★ | 定长 + BE，无状态解析 |
| OUCH | TCP | 无 | ★★★★★（解码器） | 定长消息，DPDK 侧批量解码 |
| FIX 4.x | TCP | 无 | ★ | 文本解析 + 复杂状态机，不值当 |

**一句话结论**：**DPDK 的价值在"无状态、定长、批量化"的收包与解码路径上 —— 组播行情（MoldUDP64+ITCH）全链路受益；订单接入（SoupBinTCP/iLink）需要用户态 TCP 栈搭桥，收益集中在解码层，连接与会话可靠性仍建议交给内核 TCP + 重登录/重传。**

## 4. 本仓库落地对照

| 链路 | 已实现 | 验证 |
|---|---|---|
| 行情入站 | MoldUDP64 解析 + DPDK rx（810 万 msg/s） | 容器 DPDK_VERIFY_ALL: PASS |
| 行情出站 | publish 单条 88.7 万 / 批量 1722 万 msg/s | 容器实测 |
| 订单入站 | SoupBinTCP（packet/session/ouch）+ DPDK pcap 回放 | 容器 PASS（10001 帧） |
| 订单出站 | `soup_tcp_bench` 内核 TCP 端到端（p50 56.4µs） | 容器 PASS |

## 5. 参考资料

- NASDAQ ITCH 5.0 规范（含传输架构：SoupBinTCP / 压缩 SoupBinTCP / MoldUDP64 三种选项）
- NASDAQ OUCH 4.2 规范（2025-10 更新）
- SoupTCP 2.00 官方 PDF（文本行版协议，序列号/快照语义源头）
- go-finproto（SoupBinTCP 4.1 实现）、node-soupbintcp（3.00 实现）—— 二进制帧格式交叉确认
- CME iLink 3.0 规范（SBE 编码、会话重传）

## 6. body 大小对比实测（2026-09-21，云 VM 同机同引擎）

| body | DPDK 处理延迟 (p50/p99) | DPDK 吞吐 | TCP 端到端 RTT (p50/p99) | TCP 批量吞吐 |
|---|---|---|---|---|
| 32B | 1.0 / 2.0µs | 240k/s | 24 / 52µs | 163k/s |
| 64B | 1.0 / 2.0µs | 252k/s | — | 193k/s |
| 256B | 1.0 / 2.0µs | 244k/s | 24 / 43µs | 198k/s |
| 1024B | 1.0 / 2.0µs | 229k/s | 24 / 42µs | 198k/s |

- **body 大小非瓶颈**：DPDK 处理延迟恒定（p50 1.0µs）、TCP RTT 恒定（p50 24µs）；带宽不构成限制。
- TCP 小 body 吞吐最低（163k/s @32B）：小包内核处理开销（NAPI/软中断/唤醒）占比高，body 增大略升（198k/s）。
- 口径提醒：DPDK 列为**处理侧**延迟（订单→REPORT，pcap 回放），TCP 列为**端到端**（本机回环）；两者不可直接相减，真实公平对比为 DPDK 端到端（同机房估 5-12µs）vs TCP 端到端（真机 20-50µs）。
- 工具：`pcap_order_loopback.py gen` 第 5 参数 body_size；`tcp_order_bench --body N`（body padding 追加第 6 段，parse 忽略）。

## 7. DPDK UDP 下单 vs TCP RPC：提升与必要性（2026-09-21）

### 7.1 提升量化
- **处理侧**：DPDK 1.0µs（实测）vs gRPC 本机 50-200µs（业界公开量级）→ **50-200×**（口径不完全对等：RPC 为端到端）。
- **端到端公平比**（同机房）：DPDK 5-12µs vs gRPC 50-200µs → **约 10-20×**。
- **吞吐**：DPDK 230-250k 单/s（单核，pcap IO 瓶颈）；gRPC 本机典型 10-50k/s。
- 差距来源：①无序列化（二进制 9B 帧头直解）②无内核栈（用户态 PMD，无 syscall/唤醒）③无 RPC 框架开销（会话层直连）。

### 7.2 优化阶梯（成本递增）
| 阶段 | 手段 | 端到端（同机房，估算） |
|---|---|---|
| 0 | JSON-RPC/HTTP | 100-500µs |
| 1 | 二进制协议+持久连接/批量 | 30-80µs |
| 2 | 内核调优（SO_BUSY_POLL/RSS 绑核）+批量 | 10-30µs |
| 3 | 用户态 TCP 栈（Onload/VMA） | 5-15µs |
| 4 | DPDK UDP（形态 D，已实现） | 5-12µs |
| 5 | FPGA/智能网卡 | 2-5µs |

### 7.3 必要性判断
- **做**：自营做市/高频（延迟=收入函数）、内部撮合→执行网关高频链路、瓶颈在序列化/框架。
- **暂不做/先吃中间档**：外部交易所接入（协议强制 TCP/SoupBinTCP/iLink）、订单量 <5k/s、延迟预算 >50µs、无自营低延迟场景。
- **建议**：先做阶梯 1-2（改二进制协议 + 内核调优，ROI 最高），有自营高频需求再启用形态 D（已最小成本验证：2 核 VM 即可跑通，处理侧 1-2µs 实证）。
