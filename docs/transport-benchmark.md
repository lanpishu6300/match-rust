# 下单链路传输层性能对比：内核 TCP vs busy-poll vs DPDK UDP

> 测试环境：云 VM 2 核 / 3GB，Ubuntu 22.04，本机回环（127.0.0.1 / pcap 文件回放）。
> 测试日期：2026-09-21。同一撮合链路（match-core::Engine），同一订单 payload（`B|btcusdt|100.00|1|oN` / `S|btcusdt|99.99|1|oN` 交替，body≈23B）。

## 1. 对比矩阵

| 方案 | p50 | p99 | 吞吐（单核） | CPU 占用 | 口径 |
|---|---|---|---|---|---|
| 内核 TCP（TCP_NODELAY） | **18µs** | **28µs** | 243k/s | ~68% | 端到端 RTT（本机回环） |
| 内核 TCP + SO_BUSY_POLL(50us) | 18µs | 38µs | 248k/s | ~69% | 端到端 RTT（本机回环） |
| DPDK UDP（net_pcap 回放） | **1.0µs** | **2.0µs** | 227k/s | pcap IO 瓶颈 | **处理侧**（订单帧→REPORT） |

- TCP ping 500 轮取 RTT 分布；TCP batch 20k 取吞吐；DPDK 5k 单 pcap 回放取处理延迟与吞吐。
- DPDK 吞吐受 pcap 文件 IO 瓶颈（0.022s 读 5000 帧），非 PMD 上限；真机 NIC 上 PMD 可到 800 万 msg/s（rx）/ 1700 万 msg/s（tx 批量）。

## 2. 关键发现

### 2.1 busy-poll 在本机回环无收益，p99 反而抖动

- `SO_BUSY_POLL(50us)`：p50 持平 18µs，p99 从 28µs **恶化到 38µs**，吞吐无显著变化（243k→248k/s）。
- **原因**：busy-poll 优化的是**跨机 + NAPI 中断**场景——网卡收包后 CPU 忙等 N 微秒不睡眠，消除软中断唤醒延迟。本机回环（loopback）走纯内存协议栈，**不经过 NAPI/网卡中断**，busy-poll 无优化对象；且 busy-poll 空转消耗 CPU，在单核 2 核 VM 上反而增加调度抖动。
- **结论**：busy-poll 只在**跨机真实网卡 + 高 PPS** 场景有收益（预期可消除 5-15µs 软中断唤醒）；本机回环 bench 测不出来。

### 2.2 DPDK UDP 的 1µs 是"处理侧"，不可与 TCP 端到端直接比

- DPDK p50 1.0µs = 订单帧到达 PMD → 引擎处理 → REPORT 入 tx ring 的间隔（不含 NIC DMA、不含网络链路、不含客户端等待）。
- TCP p50 18µs = 客户端 write → 内核协议栈 → 服务端 read → 引擎 → 回报 → 内核 → 客户端 read 的完整 RTT。
- **公平口径**：DPDK 端到端（同机房真机）≈ 处理侧 1µs + NIC DMA ~2µs + 链路 ~1-5µs ≈ **5-12µs**（估算）；TCP 端到端（同机房真机）≈ **20-50µs**。
- **公平比约 3-10×**（端到端），不是 18×（处理侧 vs 端到端的口径差）。

### 2.3 吞吐三方案相近，瓶颈不在传输层

- 227k（DPDK）vs 243k（TCP）vs 248k（TCP busy-poll）——同一量级。
- 原因：本机回环 / pcap 文件 IO 成为共同瓶颈；撮合引擎单线程处理（match-core::Engine.on_order）才是吞吐上限的真正约束。
- **结论**：在本 bench 规模下，换传输层不提升吞吐——吞吐瓶颈在引擎单线程处理能力，不在网络协议栈。

## 3. F-Stack（用户态 TCP 栈）预期与限制

| 项 | 说明 |
|---|---|
| 预期收益 | 跨机场景消除内核软中断/拷贝，端到端从 20-50µs → 5-15µs（估算），吞吐 300k-1M+ |
| 部署要求 | 需 root：编译加载 `fstack.ko` 内核模块（POSIX 系统调用拦截）+ KNI 设备 |
| 云 VM 限制 | **无 root、系统目录只读，无法 insmod，F-Stack 完整版跑不了** |
| 本 bench 替代 | SO_BUSY_POLL（无需 root）已验证本机回环无收益——F-Stack 的收益在本机同样测不出（loopback 不走 NAPI） |
| 真机结论 | F-Stack 的价值只在**跨机真实网卡**场景（消除 NAPI 软中断）；本机回环 bench 无法复现 |

## 4. 决策建议

| 场景 | 推荐 | 理由 |
|---|---|---|
| 本机/单体内撮合（<1 万单/s） | 内核 TCP | 足够，18µs 端到端，无额外成本 |
| 跨机低延迟下单（做市/高频） | DPDK UDP（形态 D） | 端到端 5-12µs，已验证处理侧 1µs |
| 必须用 TCP（外部交易所协议） | 内核 TCP + 真机 busy-poll | 跨机时 busy-poll 才有收益；F-Stack 为进阶选项 |
| 吞吐瓶颈 | 引擎多线程 / 无锁队列 | 传输层换协议不提升吞吐（本 bench 已证） |

## 5. 复现命令

```bash
cd match-rust

# 内核 TCP（现状）
./target/release/tcp_order_bench --ping 500          # RTT 分布
./target/release/tcp_order_bench --batch 20000 --body 64

# 内核 TCP + busy-poll（50µs）
./target/release/tcp_order_bench --ping 500 --busy-poll 50
./target/release/tcp_order_bench --batch 20000 --body 64 --busy-poll 50

# DPDK UDP（pcap 回放）
python3 crates/match-dpdk-io/scripts/pcap_order_loopback.py gen 5000 /tmp/in.pcap
DPDK_VDEV="--vdev=net_pcap0,rx_pcap=/tmp/in.pcap,tx_pcap=/tmp/gw_out.pcap" \
  timeout 25 ./target/release/dpdk_tap_order --no-huge --no-pci -m 512
```

> 注：DPDK pcap 回放模式在文件读完后继续空转（net_pcap PMD 阻塞），需 `timeout` 限时；`processed=5000` 行即完成标志。

## 6. 多 shard 按标的分片测试（2026-09-21）

### 6.1 架构

```
订单流 ──路由(hash(symbol) % shards)──▶ shard0(Engine+独立线程)
                                   ─▶ shard1(Engine+独立线程)
                                   ─▶ ...
```
每个 shard 独立单线程无锁撮合，同一标的固定进同一 shard（无跨 shard 匹配）。symbol 空间 128（sym_000~sym_127），均匀分布。

### 6.2 实测（云 VM 2 核 / 3GB，20000 单）

| shards | 总吞吐 | 相对 shards=1 | CPU（user+sys/real） |
|---|---|---|---|
| 1 | 477k/s | 1.0× | ~80%（单核） |
| 2 | **764k/s** | **1.6×** | ~137%（双核并行兑现） |
| 4 | 608k/s | 1.3× | ~140%（超核数，调度开销） |
| 8 | 690k/s | 1.4× | ~133%（超核数，无继续提升） |

### 6.3 关键结论

1. **shards=1→2 兑现并行收益**：477k→764k（1.6×），CPU 从 80% 升到 137%（双核都用上）。
2. **shards>核数后无继续提升**：云 VM 只有 2 核，4/8 shards 触发线程调度开销，吞吐回落。**真机 N 核可线性扩展到 N × 单核吞吐**（约 N×477k/s）。
3. **分片只扩吞吐，不降单路延迟**：每条订单的 p50 处理延迟不变（仍单线程无锁），总吞吐随 shard 数线性增长（受 CPU 核数限制）。
4. **跨 shard 无匹配**：同一标的固定进同一 shard，订单簿完全隔离，无锁无竞争——这是低延迟分片的核心设计。

### 6.4 复现

```bash
./target/release/multi_shard_bench --shards 1 --orders 20000
./target/release/multi_shard_bench --shards 2 --orders 20000
./target/release/multi_shard_bench --shards 4 --orders 20000
./target/release/multi_shard_bench --shards 8 --orders 20000
```

### 6.5 真机多核验证（macOS 10 核，50000 单）

| shards | 总吞吐 | 相对 1-shard | 观察 |
|---|---|---|---|
| 1 | 339k/s | 1.0× | 单核基线 |
| 2 | **665k/s** | **1.96×** | **接近线性 2×**（双 worker 并行兑现） |
| 4 | 654k/s | 1.93× | 不再增长（路由单点成为瓶颈） |
| 8 | 473k/s | 1.4× | 回落（线程调度竞争） |

**关键结论（修正 6.3）**：

1. **2 shards 接近线性**（1.96×）——分片并行收益在真机双核上完全兑现。
2. **4/8 shards 不扩展**——与云 VM 2 核结果一致，根因不是 CPU 核数（Mac 有 10 核），而是**路由/发送单点**：主线程串行把订单 send 到各 shard channel，路由速率就是总吞吐上限；shard 数超过 ~2 后，engine 并行已耗尽，主线程路由 + 线程调度成为瓶颈。
3. **线性扩展的正确形态**：不是"主线程路由 + N worker"，而是 **DPDK 多队列 RSS 直接分发**——网卡按 hash(symbol) 把报文分散到 N 个独立收包队列，每队列绑定一个 shard 线程，**无主线程路由单点**。这才是"多 shard 按标的分片"的生产形态。
4. **本 bench 价值**：验证了分片并行收益（2×）与路由单点约束（4+ 无增益）——生产实现需 RSS 直分，而非软件路由。

### 6.6 生产形态（RSS 直分）

```
NIC RSS(hash(symbol)) ──队列0──▶ shard0(收包+撮合+回报)   ← 网卡级分流，无软件路由
                   ──队列1──▶ shard1(收包+撮合+回报)
                   ──队列2──▶ shard2(收包+撮合+回报)
```
- 每队列独立 PMD 收包线程 + Engine，symbol 哈希分布由网卡 RSS 完成。
- 吞吐 = min(网卡线速, N × 单核 engine 上限)，无软件单点。
- 回报可经每队列 tx 独立回发（或聚合网关）。

### 6.7 RSS 直分 vs 中央路由——O(n²) bug 修复与重测（2026-09-21，macOS 10 核）

> ⚠️ **重要更正**：6.2/6.5 节数据（477k/764k、339k/665k）受**订单序列设计 bug** 影响——
> symbol 轮转步长（128/32，偶数）与 side 交替周期（2）**同奇偶**，导致每个 symbol 只收到
> 单边订单（sym_000 全 B、sym_001 全 S…），**永不成交、订单簿 O(n²) 堆积**
> （`OrderBook::contains_order_no` 为线性扫描），测的是"堆积退化"而非撮合性能。
> **修复**：symbol 轮转改 `k/2 % N`（每两单换 symbol，同 symbol 严格 B/S 交替成交），订单簿稳态。

**修复后重测（200k 单 / 每 shard 100k 单）：**

| shards | multi_shard（中央路由） | rss_shard（直分） |
|---|---|---|
| 1 | 800k/s | 789k/s |
| 2 | 742k/s（不增长） | **1.59M/s（2.0×）** |
| 4 | 489k/s（回落） | **2.79M/s（3.5×）** |
| 8 | 459k/s（回落） | **5.13M/s（6.5×）** |

**结论（实证）**：

1. **单核 engine 真实上限 ≈ 800k/s**（修复后交叉验证：两个 bench 的 shards=1 一致）。
2. **中央路由封顶**：shards≥2 不增长反降——主线程 send + channel 竞争成为全局单点，实证 6.5 推断。
3. **RSS 直分近似线性扩展**：8 shards 达 5.13M/s（6.5×，受 10 核超线程调度损耗），无软件单点——**生产形态应取 RSS 直分**（DPDK 多队列网卡分流，每队列绑定独立 shard 线程）。
4. **bench 设计教训**：性能基准必须保证订单序列**稳态成交**（同 symbol 双侧交替），否则测出的是订单簿结构退化而非引擎真实能力。
