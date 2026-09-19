# MoldUDP64 × DPDK 接入指南（match-moldudp64）

> 目标：在 Linux 生产环境用 DPDK 替代内核 UDP 栈收发 MoldUDP64 行情，达到零拷贝、低延迟、批量 burst 收发的数据面要求。协议层（`types` / `publisher` / `subscriber` / `retransmit` 的编解码与状态机）**不需要任何改动**，只需替换 I/O 后端。

## 1. 定位

- 本仓库 `match-moldudp64` 是**纯协议 + 内核 UDP 参考实现**，macOS 开发/测试直接可用。
- DPDK 是 Linux-only 的 I/O 后端。当前 crate 未编译 DPDK 代码（`Cargo.toml` 中 `match-dpdk-sys` 为注释预留）。
- 设计原则：**协议状态机与 I/O 解耦**。`parse_packet` 已经是纯函数式解析（输入字节切片，不依赖 socket），DPDK 模式下直接从 mbuf 内存解析，零拷贝。

## 2. 建议的模块切分（新增 `match-dpdk-io` crate）

```text
match-dpdk-io（新 crate，仅 Linux 编译）
├── sys/          # bindgen 生成的 DPDK FFI（rte_eth_rx_burst / rte_eth_tx_burst / mbuf）
├── rx.rs         # DPDK 收包线程：burst 取 mbuf → 组播过滤 → 交 parse_packet
├── tx.rs         # DPDK 发包：把 Mold Downstream 包写入 mbuf → tx_burst
├── mempool.rs    # mbuf 池、大页初始化、端口/队列配置
└── channel.rs    # mbuf → 业务核的无锁传递（rte_ring 或 crossbeam）
```

依赖：`match-moldudp64`（协议层）+ DPDK 静态库（22.11+ LTS）。

## 3. 生产端（撮合 → DPDK 组播发包）

`MoldPublisher` 当前用 `std::net::UdpSocket`。DPDK 模式替换为：

```text
撮合引擎 ──publish_tagged()──► 协议层打包（types::DownstreamHeader 编码）
                                     │
                                     ▼
                        tx.rs: 报文写入 mbuf（rte_pktmbuf_append）
                                     │
                                     ▼
                        rte_eth_tx_burst(port, qid, mbufs, n)  ← 组播 MAC/IGMP 由网卡/内核路由处理
```

注意点：
- 组播发包仍需主机加入组播组（可用内核 socket 只做 IGMP join，或配置网卡 multicast 过滤 + 静态组播路由）。
- `send_packet` 里 `packet` 是 `Vec<u8>`；DPDK 版改为直接把 header + blocks 顺序 `rte_pktmbuf_append` 进 mbuf，避免一次拷贝。
- 心跳与批量打包逻辑不变（`publish_batch` 按 MTU 截断的逻辑直接复用）。

## 4. 消费端（DPDK → 协议解析）

`MoldSubscriber::parse_packet(&mut self, buf: &[u8])` 是传输无关的，DPDK 收包线程直接喂 mbuf 数据指针：

```text
rte_eth_rx_burst(port, qid, mbufs, 32)
   │
   ▼
for each mbuf:
    udp_payload = mbuf_data + eth_hdr + ip_hdr + udp_hdr   ← 零拷贝定位
    sub.parse_packet(udp_payload)                           ← 复用现有解析
       │
       ├─ out.gap 存在 → 构造 NAK（RequestHeader 编码）→ tx_burst 单播发重传服务器
       └─ out.messages → 投递业务核（订单簿/行情网关）
```

关键点：
- mbuf 需保证**单段**承载完整 UDP 报文（MTU 1500，禁止 jumbo 分片），与协议 `MOLD_MAX_DATAGRAM = 1472` 对齐。
- 数据面核禁止 `malloc` / 打印 / 系统调用；`parse_packet` 内部分配的 `Vec` 在超高频下建议改为预分配缓冲或 arena。
- 组播接收：`rte_eth_dev_set_mc_addr_list` 加入组播组；双路 A/B 冗余 feed 用双队列 RSS 或双端口，在业务核合并去重。

## 5. 重传服务在 DPDK 下

`MoldRetransmitServer` 收的是**单播 NAK**（小报文、低频）。两种做法：
- 低延迟方案：与组播同核，用同一个 rx/tx burst 循环处理（NAK 与行情同队列）；
- 简单方案：NAK 走内核 UDP（专用小 socket），补发内容从共享缓存取——`reply_range` 不碰网络，把取出的消息交给 DPDK tx 或内核 socket 均可。

## 6. 性能预算参考（生产调优目标）

| 指标 | 内核 UDP | DPDK 目标 |
|---|---|---|
| 收包路径拷贝 | 2~3 次 | 0（mbuf 原地解析） |
| 单核收包吞吐（1472 B 包） | ~0.5–1 Mpps | 3–10 Mpps（视网卡） |
| P99 延迟（收→解析完成） | 5–15 µs | 1–3 µs |

## 7. 验证步骤（DPDK 环境）

```bash
# 1) 编译 match-moldudp64（协议层，当前即可验证）
cargo test -p match-moldudp64

# 2) 有 DPDK 的 Linux 机器：启用 match-dpdk-io 后
cargo build --release -p match-dpdk-io
sudo ./target/release/mold_dpdk_rx --port 0 --queue 0 --group 239.255.0.1

# 3) 对拍验证：内核 UDP subscriber 与 DPDK subscriber 同时收同一组播流
#    Wireshark 抓包 + 对比 seq 连续性与 payload 一致性
```

## 8. 实测验证结果（macOS + Docker Desktop, arm64, DPDK 23.11.4, 2026-09-19）

**`DPDK_VERIFY_ALL: PASS`** —— 全链路断言全部通过：

```
gen_pcap  → input.pcap (5 帧: seq 1,2,3 + 心跳@4 + seq 5,6)
DPDK rx   → 5 帧 / 5 消息 / seqs=[1,2,3,5,6] / gap=(4,1) / last_seq=6   ✅
DPDK tx   → output.pcap (2 数据 + 1 心跳, 3 帧)
DPDK rx   → 3 帧 / 3 消息 / seqs=[1,2,3] / 无 gap / last_seq=3           ✅
```

**验证中修复的 3 个真实缺陷**（FFI 与文件格式，均有 C 对照程序实测定位）：

| # | 缺陷 | 根因（实测） | 修复 |
|---|---|---|---|
| 1 | `rte_eth_dev_configure` 返回 -EINVAL + 假 "does not support lsc" | Rust FFI 的 `rte_eth_conf` 只给 512B，真实 `sizeof(struct rte_eth_conf)=2280`，DPDK 读到栈上垃圾 | 扩为 `[u64; 285]` |
| 2 | pcap 文件 libpcap 读错（len=0xFFFF/错位） | gen_pcap 记录头写错：`ts(8B)+usec(4B)+incl(4B)`，缺 `orig_len`，导致 incl_len 读到帧数据 | 标准 4+4+4+4 |
| 3 | mbuf 字段全部错位（pkt_len=0/数据全零） | 手写 `rte_mbuf` 用了 x86_64 布局（16B cacheline 前缀），aarch64 实测 `buf_addr@0, data_off@16, pkt_len@36, data_len@40` | 按实测偏移重写 |

**经验**：手写 DPDK FFI 时，结构体大小与字段偏移必须用 `sizeof()`/`offsetof()` 在目标架构实测核对，不要照搬 x86 文档布局；`rte_eth_conf` 这类大结构尤其容易踩。

## 9. 已知限制

- macOS 无 DPDK，本仓库集成测试全部基于内核 UDP（已覆盖协议正确性）。
- DPDK 环境需 root / 大页内存 / 绑定网卡（VFIO），属部署配置，不在此代码库内解决。
- `parse_packet` 当前返回 `Vec`；如需极致零分配可后续加 `no_alloc` feature（回调式消费，不收集消息）。
