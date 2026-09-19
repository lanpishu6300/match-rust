# SoupBinTCP 协议详解与 match-soupbintcp 实现

> NASDAQ 官方口径：SoupBinTCP 是 MoldUDP64 的 TCP 孪生兄弟 —— 把"序列号 + 会话快照"的组播语义搬到单条 TCP 连接上，是 OUCH 下单报文的承载层。
> 本文档基于 SoupTCP 2.00 官方 PDF、SoupBinTCP 3.x/4.x 二进制帧格式（由两个独立开源实现交叉确认）、OUCH 4.2 官方 PDF，以及本仓库 `crates/match-soupbintcp` 的落地实现。

## 1. 协议定位

| 层级 | 协议 | 方向 | 承载内容 |
|---|---|---|---|
| 会话/传输 | **SoupBinTCP** | 双向 TCP | OUCH 下单（in）、ITCH 行情（out） |
| 会话/传输 | **MoldUDP64** | 组播单向 | ITCH 行情（out） |
| 业务消息 | **OUCH** | in/out | 订单 Enter/Cancel/Replace + 回报 |
| 业务消息 | **ITCH** | out | 行情快照/增量 |

关键结论：**OUCH / ITCH 是业务消息协议，不是传输层**。OUCH 放在 SoupBinTCP 里（因为它需要可靠双向 + 确认/拒绝），ITCH 放在 MoldUDP64 里（单向组播天然适配行情扇出）。

## 2. 二进制帧格式（SoupBinTCP 3.x / 4.x）

```
+--------+------+---------+
| Length | Type | Payload |
|  2B BE | 1 B  |  ...    |
+--------+------+---------+
Length = 1 + payload_len        // 不含 Length 自身
```

TCP 是字节流，包可在任意位置拆分/合并 —— Length 前缀解决粘包/拆包。

### 2.1 包类型表

| Type | 名称 | Payload | 方向 |
|---|---|---|---|
| `L` | Login Request | user(6) + password(10) + requested_session(10) + requested_sequence(20, ASCII) | client→server |
| `A` | Login Accepted | session(10) + sequence(20, ASCII) | server→client |
| `J` | Login Rejected | reason(1) | server→client |
| `S` | Sequenced Data | 业务消息（OUCH 回报 / ITCH） | server→client |
| `H` | Server Heartbeat | 空 | server→client |
| `Z` | End of Session | 空（或尾部消息） | server→client |
| `U` | Unsequenced Data | 业务消息（OUCH 下单） | client→server |
| `R` | Client Heartbeat | 空 | client→server |
| `O` | Logout Request | 空 | client→server |
| `+` | Debug | 文本 | 双向 |

> SoupBinTCP 3.0 登录请求 46B payload；4.1 增加 5B heartbeat timeout 字段。
> 用户名/密码/会话 ASCII 右对齐空格填充；**sequence 是 20 位 ASCII 十进制右对齐**。
> 拒绝原因：`A`=Not Authorized，`S`=Session Unavailable。

### 2.2 序列号语义（与 MoldUDP64 的对照）

**SoupBinTCP 的 Sequenced Data 包没有显式序列号字段** —— 这是与 MoldUDP64 最本质的区别：

| 维度 | MoldUDP64 | SoupBinTCP |
|---|---|---|
| 序列号载体 | 每个消息头显式 4B BE | 隐式：首包=1，逐包+1 |
| 恢复机制 | 客户端 NAK（request retransmission） | 重登录：`requested_sequence` → 服务端重放 |
| 会话快照 | 无（消息级重传） | 登录即快照：`Login Accepted` 告知起始 seq |
| 缺口检测 | `expected_seq != msg_seq` | 客户端自维护 expected_seq，缺口→重登录 |

**重登录语义**（"会话快照"）：
- `requested_sequence = 0` 或超出高水位 → 从最新开始；
- `requested_sequence` 有效 → 服务端从该 seq 起重放消息存储；
- 请求过旧（超出重传窗口）→ 服务端从**可提供的最早**开始（`start = max(requested, store_start)`，本实现 `ServerSession::handle_login` 已 clamp）。

### 2.3 心跳与会话结束

- 双方必须每 **1s** 内至少发一个包（任意类型）；超时约 15s 判定链路死亡。
- 客户端心跳 `R`、服务端心跳 `H` 均为空 payload。
- 空 payload 的 `S` = End of Session 标记（SoupTCP 2.00 文本版语义，4.x 用 `Z`）。

## 3. OUCH 4.2 消息集（本实现子集）

定长消息，整数 big-endian，alpha 左对齐空格填充，价格 6.4 定点（如 195.5 → `1955000`），TIF 特殊值：`0`=IOC、`99996`=延展收盘、`99998`=市场小时、`99999`=系统小时。

| 消息 | 类型 | 长度 | 关键字段 |
|---|---|---|---|
| Enter Order | `O` | 49B | token(14) side(1) shares(4) stock(8) price(4) tif(4) firm(4) display(1) capacity(1) ise(1) minqty(4) crosstype(1) custtype(1) |
| Cancel Order | `X` | 19B | token(14) shares(4) |
| Replace Order | `U` | 47B | existing(14) replacement(14) shares(4) price(4) tif(4) display(1) ise(1) minqty(4) crosstype(1) custtype(1) |
| Modify Order | `M` | 21B | token(14) side(1) shares(4) |
| Accepted | `A` | 66B | timestamp(8) token(14) side(1) shares(4) stock(8) price(4) tif(4) firm(4) display(1) ref(8) … state(1) |
| Executed | `E` | 34B | timestamp(8) token(14) shares(4) match(4) price(4) |
| Canceled | `C` | 27B | timestamp(8) token(14) decrement(4) reason(1) |

**冗余语义**：入站（下单）消息可被展示性重发（故障恢复时防资金损失）；出站（回报）必须由下层 SoupBinTCP 保证有序 —— 这正是本实现"回报全部走 Sequenced Data"的原因。

## 4. 实现架构（`crates/match-soupbintcp`）

```
src/
  lib.rs     模块导出 + 协议定位说明
  packet.rs  [len 2BE][type][payload] 编解码；parse_stream 处理 TCP 粘包/拆包
             login_request/login_accepted/login_rejected/heartbeat/end_of_session/logout_request
  session.rs ServerSession（session_id、next_seq、BTreeMap 消息存储+容量上限、
             handle_login 重放、handle_packet U/H/O）
             ClientSession（expected_seq/start_seq、handle_packet A/J/S/H/Z）
             SessionError（手写 Display/Error，零外部依赖）
  ouch.rs    OUCH 4.2 编解码：Enter/Cancel/Replace/Accepted/Executed/Canceled
```

设计原则：
1. **纯 std、零外部依赖** —— 容器离线可构建（DPDK 验证镜像无网络）。
2. **传输无关** —— 只处理字节流，不碰 socket：内核 TCP、DPDK+用户态 TCP 栈、pcap 回放共用同一解析器。
3. 测试：帧编解码 roundtrip、粘包/拆包、字段宽度 —— macOS `cargo test` 6/6 通过。

## 5. 实测结果（容器 `mold-dpdk-verify`，arm64）

### 5.1 内核 TCP 端到端（`soup_tcp_bench`，loopback 单连接）

| 指标 | N=50,000 |
|---|---|
| login → Accepted RTT | **95.7 µs** |
| order→accept RTT p50 | **56.4 µs** |
| order→accept RTT p99 | **125 µs** |
| 吞吐（单连接串行） | **16.6k 对/s** |

吞吐受限于"每笔订单一次 syscall 往返"（写 + 读 各 2 次），是内核 TCP 小包延迟主导 —— 生产吞吐优化方向是批处理（见 §6）。

### 5.2 重登录快照重放

| 请求 seq | granted | 重放条数 | 语义 |
|---|---|---|---|
| 25000（store 窗口内，N=50000 前半） | 25000 | 25000 | 完整重放 |
| 25000（store 8192 窗口外） | **41809** | 8192 | clamp 到可提供最早（等价 MoldUDP64 重传窗口溢出） |

### 5.3 DPDK 收流 + 解析（`mold_soup_dpdk`，net_pcap 回放 10,000 帧）

```
login_ok=true, seq=10001 == 10001, reports=10000   PASS
```

DPDK rx_burst → udp_payload → parse_stream → ClientSession，序列号连续断言通过。

## 6. 生产路径建议（下单链路）

1. **生产订单接入默认走内核 TCP**：SoupBinTCP 是会话层协议，依赖 TCP 的可靠有序 + 重登录重放，DPDK 在订单链路的收益主要在**接收端的批量解析**而非连接管理。
2. 若追求极致（≤µs 级接收抖动），把 DPDK 放在**接收侧**：网卡→DPDK 轮询→用户态 TCP 栈（如 mTCP/F-Stack）→ SoupBinTCP 解析器（本 crate 已解耦，可直接嵌入）。
3. **发送侧批量**：MoldUDP64 出站已验证 publish_batch 打包 58.1ns/msg（vs 单条 1127ns，19.4×）；OUCH 回报同样应按"每 TCP 段多消息"聚合（本实现 `ServerSession::enqueue` 已支持批量取数）。
4. 心跳：1s 保活由应用层定时器驱动（本 crate 暴露 `last_rx_ns` 供 deadline 判断）。

## 7. 诚实边界

- 二进制帧格式来自两套独立实现（node-soupbintcp 3.00 / go-finproto 4.1）交叉确认 + SoupTCP 2.00 官方 PDF；NASDAQ 官方二进制版 PDF 未公开获取，若对接真实 NASDAQ 网关请以交易所接口文档为准核对登录字段宽度。
- pcap 回放吞吐 ≠ 线速；真实 NIC 收流需 Linux 真机（MLX5/i40e + vfio-pci），见 §10 of `docs/moldudp64-dpdk-integration.md`。
- `soup_tcp_bench` 是"回声"模式：Accepted 报告由 bench 构造，未接真实撮合引擎。

## 8. 相关文件

- 实现：`crates/match-soupbintcp/{lib,packet,session,ouch}.rs`
- 验证：`crates/match-dpdk-io/src/bin/{soup_tcp_bench,mold_soup_dpdk}.rs`
- 对照阅读：`docs/moldudp64-dpdk-integration.md`（MoldUDP64/DPDK 实测）

## 9. 多维性能测试与故障注入（soup_perf_matrix，容器实测 2026-09-20）

内核 TCP 回环、单进程（server 线程 + client）、Linux 容器 `mold-dpdk-verify`。吞吐为"下单→全回报闭环"速率。

### 9.1 订单数量扫描（fills=1, batch=1, conns=1）

| 订单数 | 耗时 | 订单/回报速率 |
|---|---|---|
| 1,000 | 0.058 s | 17,273/s |
| 10,000 | 0.582 s | 17,184/s |
| 50,000 | 2.841 s | 17,602/s |
| 100,000 | 5.850 s | 17,093/s |

结论：速率与量级无关（≈17.2k/s），纯 syscall 往返瓶颈。

### 9.2 成交数量扩展（orders=20,000）

| 每单回报数 | 订单速率 | 回报总量 | 回报侧速率 |
|---|---|---|---|
| 1 | 17,653/s | 20,000 | 17,653/s |
| 2 | 17,220/s | 40,000 | 34,440/s |
| 5 | 15,193/s | 100,000 | 75,965/s |
| 10 | 16,071/s | 200,000 | 160,710/s |

结论：订单速率仅降 ~9%，回报侧吞吐线性扩展（→160.7k reports/s）。

### 9.3 下单速度（orders=50,000）

批量（conns=1）：batch=1/10/100 → 17,990 / 149,637 / **352,868 orders/s（19.6×）**
并发（batch=1）：conns=1/2/4 → 17,048 / 32,185 / **48,203 orders/s（2.83×）**

### 9.4 故障注入（orders=20,000）

| 场景 | 速率 | 行为 |
|---|---|---|
| 基线 | 17,298/s | 全闭环 |
| mid-drop 断连重连 | 17,512/s | 断点重连续跑；断连瞬间在途回报未确认（race），重连按 requested_seq 补 |

其他故障语义（soup_tcp_bench 断言 PASS）：重放窗口 clamp（requested=25000→granted=41809→重放 8192）、too-old→最早可提供、心跳超时→EOF→重登录恢复。

### 9.5 结论

吞吐优先级：批量打包 > 并发连接 > 单连接优化；故障恢复由"重登录 + requested_seq 重放"统一覆盖。
