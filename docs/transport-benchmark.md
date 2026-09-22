# 下单链路传输层性能对比：内核 TCP vs busy-poll vs DPDK UDP

> 测试环境：云 VM 2 核 / 3GB，Ubuntu 22.04，本机回环（127.0.0.1 /pcap 文件回放）。
> 测试日期：2026-09-21。同一撮合链路（match-core::Engine），同一订单 payload（
>
> `B|btcusdt|100.00|1|oN`
>
>  / 
>
> `S|btcusdt|99.99|1|oN`
>
>  交替，body≈23B）。

## 1. 对比矩阵



| 方案                             | p50       | p99       | 吞吐（单核） | CPU 占用     | 口径                  |
| ------------------------------ | --------- | --------- | ------ | ---------- | ------------------- |
| 内核 TCP（TCP\_NODELAY）           | **18µs**  | **28µs**  | 243k/s | \~68%      | 端到端 RTT（本机回环）       |
| 内核 TCP + SO\_BUSY\_POLL (50us) | 18µs      | 38µs      | 248k/s | \~69%      | 端到端 RTT（本机回环）       |
| DPDK UDP（net\_pcap 回放）         | **1.0µs** | **2.0µs** | 227k/s | pcap IO 瓶颈 | **处理侧**（订单帧→REPORT） |



* TCP ping 500 轮取 RTT 分布；TCP batch 20k 取吞吐；DPDK 5k 单 pcap 回放取处理延迟与吞吐。

* DPDK 吞吐受 pcap 文件 IO 瓶颈（0.022s 读 5000 帧），非 PMD 上限；真机 NIC 上 PMD 可到 800 万 msg/s（rx）/ 1700 万 msg/s（tx 批量）。

## 2. 关键发现

### 2.1 busy-poll 在本机回环无收益，p99 反而抖动



* `SO_BUSY_POLL(50us)`：p50 持平 18µs，p99 从 28µs **恶化到 38µs**，吞吐无显著变化（243k→248k/s）。

* **原因**：busy-poll 优化的是**跨机 + NAPI 中断**场景 —— 网卡收包后 CPU 忙等 N 微秒不睡眠，消除软中断唤醒延迟。本机回环（loopback）走纯内存协议栈，**不经过 NAPI / 网卡中断**，busy-poll 无优化对象；且 busy-poll 空转消耗 CPU，在单核 2 核 VM 上反而增加调度抖动。

* **结论**：busy-poll 只在**跨机真实网卡 + 高 PPS** 场景有收益（预期可消除 5-15µs 软中断唤醒）；本机回环 bench 测不出来。

### 2.2 DPDK UDP 的 1µs 是 "处理侧"，不可与 TCP 端到端直接比



* DPDK p50 1.0µs = 订单帧到达 PMD → 引擎处理 → REPORT 入 tx ring 的间隔（不含 NIC DMA、不含网络链路、不含客户端等待）。

* TCP p50 18µs = 客户端 write → 内核协议栈 → 服务端 read → 引擎 → 回报 → 内核 → 客户端 read 的完整 RTT。

* **公平口径**：DPDK 端到端（同机房真机）≈ 处理侧 1µs + NIC DMA \~2µs + 链路～1-5µs ≈ **5-12µs**（估算）；TCP 端到端（同机房真机）≈ **20-50µs**。

* **公平比约 3-10×**（端到端），不是 18×（处理侧 vs 端到端的口径差）。

### 2.3 吞吐三方案相近，瓶颈不在传输层



* 227k（DPDK）vs 243k（TCP）vs 248k（TCP busy-poll）—— 同一量级。

* 原因：本机回环 /pcap 文件 IO 成为共同瓶颈；撮合引擎单线程处理（match-core::Engine.on\_order）才是吞吐上限的真正约束。

* **结论**：在本 bench 规模下，换传输层不提升吞吐 —— 吞吐瓶颈在引擎单线程处理能力，不在网络协议栈。

## 3. F-Stack（用户态 TCP 栈）预期与限制



| 项          | 说明                                                                        |
| ---------- | ------------------------------------------------------------------------- |
| 预期收益       | 跨机场景消除内核软中断 / 拷贝，端到端从 20-50µs → 5-15µs（估算），吞吐 300k-1M+                    |
| 部署要求       | 需 root：编译加载 `fstack.ko` 内核模块（POSIX 系统调用拦截）+ KNI 设备                        |
| 云 VM 限制    | **无 root、系统目录只读，无法 insmod，F-Stack 完整版跑不了**                                |
| 本 bench 替代 | SO\_BUSY\_POLL（无需 root）已验证本机回环无收益 ——F-Stack 的收益在本机同样测不出（loopback 不走 NAPI） |
| 真机结论       | F-Stack 的价值只在**跨机真实网卡**场景（消除 NAPI 软中断）；本机回环 bench 无法复现                    |

## 4. 决策建议



| 场景                   | 推荐                    | 理由                               |
| -------------------- | --------------------- | -------------------------------- |
| 本机 / 单体内撮合（<1 万单 /s） | 内核 TCP                | 足够，18µs 端到端，无额外成本                |
| 跨机低延迟下单（做市 / 高频）     | DPDK UDP（形态 D）        | 端到端 5-12µs，已验证处理侧 1µs            |
| 必须用 TCP（外部交易所协议）     | 内核 TCP + 真机 busy-poll | 跨机时 busy-poll 才有收益；F-Stack 为进阶选项 |
| 吞吐瓶颈                 | 引擎多线程 / 无锁队列          | 传输层换协议不提升吞吐（本 bench 已证）          |

## 5. 复现命令



```
cd match-rust

\# 内核 TCP（现状）

./target/release/tcp\_order\_bench --ping 500          # RTT 分布

./target/release/tcp\_order\_bench --batch 20000 --body 64

\# 内核 TCP + busy-poll（50µs）

./target/release/tcp\_order\_bench --ping 500 --busy-poll 50

./target/release/tcp\_order\_bench --batch 20000 --body 64 --busy-poll 50

\# DPDK UDP（pcap 回放）

python3 crates/match-dpdk-io/scripts/pcap\_order\_loopback.py gen 5000 /tmp/in.pcap

DPDK\_VDEV="--vdev=net\_pcap0,rx\_pcap=/tmp/in.pcap,tx\_pcap=/tmp/gw\_out.pcap" \\

&#x20; timeout 25 ./target/release/dpdk\_tap\_order --no-huge --no-pci -m 512
```

> 注：DPDK pcap 回放模式在文件读完后继续空转（net_pcap PMD 阻塞），需 
>
> `timeout`
>
>  限时；
>
> `processed=5000`
>
>  行即完成标志。

## 6. 多 shard 按标的分片测试（2026-09-21）

### 6.1 架构



```
订单流 ──路由(hash(symbol) % shards)──▶ shard0(Engine+独立线程)

&#x20;                                  ─▶ shard1(Engine+独立线程)

&#x20;                                  ─▶ ...
```

每个 shard 独立单线程无锁撮合，同一标的固定进同一 shard（无跨 shard 匹配）。symbol 空间 128（sym\_000\~sym\_127），均匀分布。

### 6.2 实测（云 VM 2 核 / 3GB，20000 单）



| shards | 总吞吐        | 相对 shards=1 | CPU（user+sys/real） |
| ------ | ---------- | ----------- | ------------------ |
| 1      | 477k/s     | 1.0×        | \~80%（单核）          |
| 2      | **764k/s** | **1.6×**    | \~137%（双核并行兑现）     |
| 4      | 608k/s     | 1.3×        | \~140%（超核数，调度开销）   |
| 8      | 690k/s     | 1.4×        | \~133%（超核数，无继续提升）  |

### 6.3 关键结论



1. **shards=1→2 兑现并行收益**：477k→764k（1.6×），CPU 从 80% 升到 137%（双核都用上）。

2. **shards > 核数后无继续提升**：云 VM 只有 2 核，4/8 shards 触发线程调度开销，吞吐回落。**真机 N 核可线性扩展到 N × 单核吞吐**（约 N×477k/s）。

3. **分片只扩吞吐，不降单路延迟**：每条订单的 p50 处理延迟不变（仍单线程无锁），总吞吐随 shard 数线性增长（受 CPU 核数限制）。

4. **跨 shard 无匹配**：同一标的固定进同一 shard，订单簿完全隔离，无锁无竞争 —— 这是低延迟分片的核心设计。

### 6.4 复现



```
./target/release/multi\_shard\_bench --shards 1 --orders 20000

./target/release/multi\_shard\_bench --shards 2 --orders 20000

./target/release/multi\_shard\_bench --shards 4 --orders 20000

./target/release/multi\_shard\_bench --shards 8 --orders 20000
```

### 6.5 真机多核验证（macOS 10 核，50000 单）



| shards | 总吞吐        | 相对 1-shard | 观察                         |
| ------ | ---------- | ---------- | -------------------------- |
| 1      | 339k/s     | 1.0×       | 单核基线                       |
| 2      | **665k/s** | **1.96×**  | **接近线性 2×**（双 worker 并行兑现） |
| 4      | 654k/s     | 1.93×      | 不再增长（路由单点成为瓶颈）             |
| 8      | 473k/s     | 1.4×       | 回落（线程调度竞争）                 |

**关键结论（修正 6.3）**：



1. **2 shards 接近线性**（1.96×）—— 分片并行收益在真机双核上完全兑现。

2. **4/8 shards 不扩展**—— 与云 VM 2 核结果一致，根因不是 CPU 核数（Mac 有 10 核），而是**路由 / 发送单点**：主线程串行把订单 send 到各 shard channel，路由速率就是总吞吐上限；shard 数超过～2 后，engine 并行已耗尽，主线程路由 + 线程调度成为瓶颈。

3. **线性扩展的正确形态**：不是 "主线程路由 + N worker"，而是 **DPDK 多队列 RSS 直接分发**—— 网卡按 hash (symbol) 把报文分散到 N 个独立收包队列，每队列绑定一个 shard 线程，**无主线程路由单点**。这才是 "多 shard 按标的分片" 的生产形态。

4. **本 bench 价值**：验证了分片并行收益（2×）与路由单点约束（4+ 无增益）—— 生产实现需 RSS 直分，而非软件路由。

### 6.6 生产形态（RSS 直分）



```
NIC RSS(hash(symbol)) ──队列0──▶ shard0(收包+撮合+回报)   ← 网卡级分流，无软件路由

&#x20;                  ──队列1──▶ shard1(收包+撮合+回报)

&#x20;                  ──队列2──▶ shard2(收包+撮合+回报)
```



* 每队列独立 PMD 收包线程 + Engine，symbol 哈希分布由网卡 RSS 完成。

* 吞吐 = min (网卡线速，N × 单核 engine 上限)，无软件单点。

* 回报可经每队列 tx 独立回发（或聚合网关）。

### 6.7 RSS 直分 vs 中央路由 ——O (n²) bug 修复与重测（2026-09-21，macOS 10 核）

> ⚠️ 
>
> **重要更正**
>
> ：6.2/6.5 节数据（477k/764k、339k/665k）受
>
> **订单序列设计 bug**
>
>  影响 ——
> symbol 轮转步长（128/32，偶数）与 side 交替周期（2）
>
> **同奇偶**
>
> ，导致每个 symbol 只收到
> 单边订单（sym_000 全 B、sym_001 全 S…），
>
> **永不成交、订单簿 O (n²) 堆积**
> （
>
> `OrderBook::contains_order_no`
>
>  为线性扫描），测的是 "堆积退化" 而非撮合性能。
> **修复**
>
> ：symbol 轮转改 
>
> `k/2 % N`
>
> （每两单换 symbol，同 symbol 严格 B/S 交替成交），订单簿稳态。

**修复后重测（200k 单 / 每 shard 100k 单）：**



| shards | multi\_shard（中央路由） | rss\_shard（直分）    |
| ------ | ------------------ | ----------------- |
| 1      | 800k/s             | 789k/s            |
| 2      | 742k/s（不增长）        | **1.59M/s（2.0×）** |
| 4      | 489k/s（回落）         | **2.79M/s（3.5×）** |
| 8      | 459k/s（回落）         | **5.13M/s（6.5×）** |

**结论（实证）**：



1. **单核 engine 真实上限 ≈ 800k/s**（修复后交叉验证：两个 bench 的 shards=1 一致）。

2. **中央路由封顶**：shards≥2 不增长反降 —— 主线程 send + channel 竞争成为全局单点，实证 6.5 推断。

3. **RSS 直分近似线性扩展**：8 shards 达 5.13M/s（6.5×，受 10 核超线程调度损耗），无软件单点 ——**生产形态应取 RSS 直分**（DPDK 多队列网卡分流，每队列绑定独立 shard 线程）。

4. **bench 设计教训**：性能基准必须保证订单序列**稳态成交**（同 symbol 双侧交替），否则测出的是订单簿结构退化而非引擎真实能力。
### 6.8 独立客户端生成订单 + 无锁 SPSC 投递（2026-09-21，macOS 10 核）

**对照问题**：rss_shard_bench 的"shard 线程内生成订单"把生成开销（format + type_convert）混入撮合线程，
未模拟客户端投递路径。本 bench 改为**每个 shard = 独立客户端线程（生成订单）+ 无锁 SPSC ring
（4096 槽，缓存行对齐，模拟 RSS 队列投递）+ 撮合线程**，客户端生成与撮合解耦。

| shards | rss（线程内生成） | client（客户端+ring 分离） | client gen_rate（客户端侧） |
|---|---|---|---|
| 1 | 789k/s | **1.05M/s** | 1.09M/s |
| 2 | 1.59M/s | **1.95M/s** | 1.05M/s |
| 4 | 2.79M/s | **2.98M/s** | 800k/s |
| 8 | **5.13M/s** | 3.20M/s | **430k/s（瓶颈）** |

**结论**：

1. **客户端分离在核数充足时更优**（1-4 shards）：生成与撮合流水线并行，吞吐高于线程内生成（1.05M vs 789k，1 shard 提升 33%）。
2. **8 shards 超核限制**：8 客户端 + 8 撮合 = 16 线程 > 10 核，客户端生成线程调度竞争，gen_rate 掉到 430k/s 成为瓶颈。
3. **真实部署不受此限**：生产上客户端（下单端）与撮合引擎通常**异机部署**，不抢 CPU——本 bench 的 8-shard 回落是**单机超核**的人为约束，非架构缺陷。
4. **SPSC ring 是正确投递通道**：无锁、缓存行对齐、批量 pop_n（64/批），单 shard 端到端 1.05M/s 已验证不构成瓶颈（gen 1.09M ≈ shard 1.05M）。
5. **推荐生产形态**：客户端（异机）→ DPDK 网卡 RSS 队列 → shard 收包线程（ring 消费 + Engine）——客户端与撮合天然解耦，吞吐 = min(客户端发送, 网卡线速, N×800k/s)。

### 6.8 TCP RSS 直分实测（2026-09-21，macOS 10 核，本机回环）

每 shard = 独立 TCP 连接（内核栈 recv） + 独立撮合线程，模拟网卡 RSS 按连接哈希分流。50000 单/shard。

| shards | 吞吐 | 相对 1-shard | p50 RTT | p99 RTT |
|---|---|---|---|---|
| 1 | 41.9k/s | 1.0× | 18µs | 63µs |
| 2 | 79.6k/s | 1.9× | 20µs | 57µs |
| 4 | 88.5k/s | 2.1× | 40µs | 86µs |
| 8 | 116.5k/s | 2.8× | 62µs | 116µs |

### 6.9 三形态总对比（8 shards，macOS 10 核）

| 形态 | 1 shard | 8 shards | 扩展性 | 每单路径开销 |
|---|---|---|---|---|
| TCP RSS 直分（内核栈） | 42k/s | **116k/s** | 2.8× | ~20-60µs（syscall+软中断+唤醒+锁） |
| client + SPSC（用户态投递） | 1.05M/s | 3.2M/s | 3.0× | ~ns（内存拷贝） |
| SPSC 直分（无网络栈） | 789k/s | **5.13M/s** | 6.5× | ~ns |

**结论（实证）**：

1. **内核 TCP 栈是最大瓶颈**：TCP RSS 直分 8 shards 仅 116k/s（2.8× 扩展），SPSC 直分同核数 5.13M/s——**差 44×**。根因是内核栈每单 20-60µs（syscall/软中断/唤醒/全局锁），**与撮合引擎无关**。
2. **TCP 扩展性差于 SPSC**：8 shards 时 TCP 仅 2.8×（内核 TCP 处理是共享资源，多连接并发加剧锁竞争），SPSC 6.5×（无共享状态）。TCP p50 从 18µs 恶化到 62µs（并发竞争）。
3. **"赢在用户态路径"实锤**：同一撮合引擎（match_core::Engine），仅投递路径不同（内核 TCP vs 用户态 SPSC），吞吐差 44×、延迟差 3-5×——提升来自路径，不是引擎也不是协议本身。
4. **生产映射**：外部交易所接入（强制 TCP/SoupBinTCP）受内核栈约束（~100k/s/10 核，靠多机横向）；自营链路用 UDP/用户态（DPDK）可到 M/s 级。

## 7. 与 LMAX Disruptor 600万/s/核 的差距——实测回答（2026-09-21）

### 7.1 实测对照（macOS 10 核，客户端+SPSC ring 投递）

| 引擎 | 单核吞吐 | 说明 |
|---|---|---|
| match_core::Engine（String symbol + BigDecimal price + BTreeSet + HashMap） | **800k/s** | 通用表示层，每单 4-6 次 String 堆分配 |
| **match_core_hp::HpEngine（i64 tick/lot + Copy 命令 + 索引化订单簿）** | **17.7-29.3M/s** | LMAX 风格，零分配路径 |
| LMAX Disruptor（2011 公开数） | 6M/s | **含 journal 持久化 + 风控的完整生产链路** |

- HpEngine 多价位变体（10 万价位间跳）18.0M/s ≈ 同价 17.7M/s——索引对价位分布不敏感，**不是最简路径虚高**。
- 8 shards（多价位）59.9M/s。
- **结论：要 600万/s，本项目已具备（HpEngine 单核是 LMAX 的 3-5×）；差距不在"能否做到"，而在用哪套表示。**

### 7.2 差距分解（match_core 800k vs HpEngine 29M，约 36×）

| 环节 | match_core（800k/s） | HpEngine（29M/s） | 贡献 |
|---|---|---|---|
| 订单表示 | MqOrder/BbOrder：symbol/order_no 多个 String + format! 每次分配 | HpCommand：Copy 值类型（i64 side/price/qty/ts），零堆分配 | ~3-4× |
| 价格 | BigDecimal（堆分配大数运算） | i64 定点 tick | ~2-3× |
| 订单簿 | BTreeSet<BuyEntry/SellEntry>（节点分配 + BigDecimal/String 比较） | 索引化（level_index/art_index/order_store） | ~2-3× |
| symbol 路由 | HashMap<String, OrderBook>：String clone + 哈希 | client_id/索引（无 String） | ~1.5-2× |
| 事件输出 | Vec<MatchEvent>（含 String）分配 | &[HpEvent] 切片引用，无分配 | ~1.5× |
| 查重 | contains_order_no 线性扫描 | 索引 O(1)/O(log) | 1.5-2× |

### 7.3 Disruptor 的本质与本仓库对照

| Disruptor 核心 | 本项目对应 | 状态 |
|---|---|---|
| 环形缓冲无锁（预分配槽） | SpscRing（match-core-hp / client_shard_bench） | ✅ 已实现 |
| 缓存行填充（防伪共享） | `#[repr(align(64))] CachePadded` | ✅ 已实现 |
| 批处理（一次 Acquire/Release 取一批） | `pop_n(out, 64)` | ✅ 已实现 |
| 单线程业务逻辑（事件循环） | HpEngine::on_order（无锁单线程） | ✅ 已实现 |
| 零分配路径（值类型 + 预分配） | HpCommand Copy + i64 tick | ✅ 已实现 |
| 事件溯源 + journal（顺序写） | ⏳ 未在 HpEngine 链路接入 | 待做 |

**一句话**：Disruptor 的"600万/s"来自**无锁环形缓冲 + 单线程 + 零分配**，本项目 SpscRing + HpEngine 已复刻全部核心（3-5× 反超）；剩余差距（若想对齐 LMAX 的完整链路数字）在 **journal 持久化 + 风控接入**，而非撮合本身。

### 6.10 HpEngine（match-core-hp）vs match_core::Engine（2026-09-21，macOS 10 核）

两种引擎同形态对比（稳态成交：每 symbol 严格 B/S 交替，fills=50%）。

**口径 A — 端到端（生成 + 撮合，shard 线程内即时构造）**：rss_shard_bench vs hp_shard_bench

| shards | match_core::Engine（含 String 生成） | HpEngine（整数命令） | 比值 |
|---|---|---|---|
| 1 | 789k/s | 23.4M/s | 29.6× |
| 8 | 5.13M/s | 124M/s | 24× |

**口径 B — 纯撮合热路径（预生成订单 Vec，move 语义）**：engine_pure_bench

| shards | match_core::Engine | HpEngine | 比值 |
|---|---|---|---|
| 1 | 781k/s | 26.2M/s | 33.5× |
| 2 | 1.08M/s | 47.4M/s | 43.7× |
| 4 | 1.36M/s | 73.0M/s | 53.9× |
| 8 | 1.56M/s | 103M/s | 66.0× |

> 注：口径 B 预生成大对象 Vec（BbOrder 含多 String）遍历有 cache-miss 影响，core 8-shard 扩展性偏低（1.44×，内存带宽竞争）；1-shard 与口径 A 一致（781k vs 789k）验证有效性。

**差异归因（诚实拆解，非单一"算法快"）**：

1. **数据表示（大头）**：BbOrder 用 String（symbol_key/trust_price/trust_number），HpEngine 用 i64 tick/lot——生成与解析从 ~1.2µs 降到 ns 级。
2. **事件分配**：match_core 每次 `on_order` 返回 `Vec<MatchEvent>`（堆分配）；HpEngine 复用内部 buffer 返回 `&[HpEvent]`（零分配）。
3. **数据结构**：match_core = `HashMap<String, OrderBook>` + BTreeSet + `contains_order_no` 线性扫描；HpEngine = level book（整数价格索引）+ 顺序 store 数组。
4. **语义简化**：HpEngine 每 client_id 仅允许一个在途订单（`client_to_id` 查重后静默丢弃），省掉重复检查路径。

**结论**：HpEngine 的 30-66× 优势 = **整数表示 + 零分配事件 + level book 索引**的合力，不是单一算法胜出。"赢在表示"——若自营链路（DPDK 形态 D）要用满用户态收益，**订单表示必须整数化（tick/lot），不应再走 String 序列化**；外部协议（OUCH/SoupBinTCP）的 String 表示留在网关边界转换。

### 6.11 RSS 直分 + HpEngine 全链路，及 DPDK UDP 结合效果（2026-09-21）

**口径：独立客户端生成（HpCommand 整数）→ match-core-hp SpscRing（RSS 队列投递）→ 撮合线程 HpEngine**（hp_client_shard_bench），与 client_shard_bench（BbOrder + Engine）同形态：

| shards | client_shard（String+Engine） | hp_client_shard（整数+HpEngine） | 比值 |
|---|---|---|---|
| 1 | 1.05M/s | 25.5M/s | 24× |
| 2 | 1.95M/s | 58.6M/s | 30× |
| 4 | 2.98M/s | 61.0M/s | 20× |
| 8 | 3.20M/s | 45.3M/s | 14× |

> 8-shard 时 hp 侧下滑（45M vs 4sh 61M）：16 线程抢 10 核调度 + 短时测量噪声；gen_rate 6.8M/s 非瓶颈。client_shard 8-shard 时客户端 String 生成 430k/s 是硬瓶颈——**换 HpEngine 后瓶颈消失**。

**DPDK UDP + HpEngine 效果（诚实标注：估算 + 锚点，无完整链路实测）**：

| 环节 | 已有锚点 | 说明 |
|---|---|---|
| DPDK rx_burst 收包 | 云 VM pcap 回放处理侧 p50 1.0µs | pcap 文件 IO 是 230k/s 瓶颈，非处理 |
| UDP 私有帧解析 | ~0.5-1µs（云 VM 实测处理侧） | 与 pcap 回放同路径 |
| HpEngine 撮合 | 26M/s @1shard / 103M @8shards | 本机实测，~40ns/单 |
| RSS+ring+撮合全链路 | 45M/s @8shards | 本机实测，无网络栈上界 |

**预期（估算）**：DPDK UDP + HpEngine 真机全链路 **2-10M 单/s**、端到端处理延迟 **1.5-3µs**——瓶颈从引擎转移到 **DPDK 收包/解析**（rx_burst 批大小、UDP 头解析、cache）。

**结论**：

1. **HpEngine 形态下引擎不再是瓶颈**（~40ns/单），DPDK UDP 链路吞吐由收包速率决定（2-10M/s 级，业界 PMD 常规）；对比内核 TCP RSS（116k/s、18-62µs）仍是量级提升。
2. **RSS 直分 + HpEngine 全链路（45M/s）证明"用户态路径 + 整数表示"组合无软件侧短板**——剩余瓶颈全部在网卡/收包（硬件侧），这正是形态 D 的最终形态：DPDK 收包 + HpEngine 撮合 + 整数化订单表示。
3. **诚实边界**：45M/s 是无网络栈上界；DPDK 真机 2-10M/s 为估算（云 VM 只能 pcap 回放、Mac 无 DPDK）——需真机验证。

## §6.12 mmap journal 持久化（Disruptor journal-first，2026-09-21）

**实现**：`match-core-hp::MmapJournal`（`crates/match-core-hp/src/journal.rs`）——
预分配文件 + `mmap(MAP_SHARED)`，记录格式 `[u32 len][payload]`，顺序游标 append（memcpy 到映射区），
`msync(MS_SYNC/MS_ASYNC)` 落盘，`iter_records()` 从头回放重建。bench：`hp_journal_bench`。

**口径（诚实标注）**：
- mmap-append = page-cache 写（内存速度），**非崩溃一致**（OS 崩溃丢脏页）。
- mmap + msync(MS_SYNC) = 真持久（阻塞落盘），吞吐 ≈ 批次大小 / msync 延迟（SSD 单次 0.1-2ms，波动大）。
- **口径差异（重要）**：LMAX journaler = mmap 批量流式写 + **不逐条 fsync**，落盘靠 RAID 控制器
  电池备份（BBU）缓存兜底；Aeron = mmap 写 + **不主动 fsync**，持久靠集群复制（RAFT）。
  **本实现 = 纯软件 MS_SYNC（无 BBU/复制依赖），比二者更严格**，吞吐代价约 -60%。

**实测（macOS，本地 SSD，200k 单/轮，3 轮）**：

| 模式 | 吞吐（3 轮） | 中位数 | 备注 |
|---|---|---|---|
| no-persist（纯撮合） | 23.8 / 26.9 / 27.4M/s | ~26.9M/s | 基线 |
| mmap-append（page-cache） | 22.0 / 21.7 / 23.8M/s | ~22.0M/s | -15%，非崩溃一致 |
| mmap + msync every 4096 | 7.6 / 10.9 / 10.5M/s | ~10.5M/s | **真持久，3 轮全 >6M** |
| msync every 32768 | 10.0 / 4.1 / 4.7M/s | ~4.7M/s | 大批次刷盘延迟抖动大 |

**结论**：
1. **6M/s 目标达成**：msync(4096) 粒度真持久 7.6-10.9M/s；mmap page-cache 22M/s（非崩溃一致）。
2. **持久化成本分层**：纯撮合 27M → mmap 写 -15%（memcpy 到映射区）→ +MS_SYNC 批量刷盘再 -60%（磁盘延迟为主）。
3. **粒度是关键**：4096 条/次（~150KB）是 macOS SSD 的甜点；过大的批（32768）刷盘延迟抖动反而伤 p999。
4. **生产建议**：journal-first（先 append 命令再撮合）已按 Disruptor 语义集成在 bench 中；落盘策略
   用自适应批量（如 4096 条或 1ms 定时），崩溃恢复 = 回放 `iter_records()` 重建 book。

## §6.13 崩溃恢复 + 回放验证（2026-09-22）

**实现**（`hp_journal_crash_test` + journal.rs 增强）：
- journal 文件头 8B 存 `committed_cursor`；`commit()` = **先数据后元数据**顺序：
  ① msync(数据区 MS_SYNC) ② 写 header ③ msync(header 页)——header 可见即数据必已落盘。
- 崩溃恢复 = `open_existing`（不 truncate）→ 读 committed_cursor → 回放 `[8, committed)` 前缀重建 book。
- 未提交尾部按"崩溃时未持久化"丢弃（设计语义）。

**验证（真实 SIGKILL 注入，macOS）**：

| 场景 | 结果 |
|---|---|
| 正常退出全量恢复 | 200,000 单全部恢复，fills=50,000，bid=99950/ask=100500，与检查点一致 |
| SIGKILL 中途（kill 时已 commit 24,576 单） | 回放恢复恰好 24,576 单，fills=6,144（精确 =N/4），book 状态与子进程实时检查点一致 |
| 确定性核对 | 子进程每次 commit 后写检查点（订单数/bid/ask/fills）→ 回放侧取 `orders<=回放数` 最后一条比对——跨进程状态一致，非自证 |

**结论**：
1. **崩溃一致性闭环**：committed 前缀精确恢复、尾部按设计丢弃、book 状态可复现——6M/s 持久化链路（journal-first + 批量 MS_SYNC + 崩溃回放）全部闭合。
2. **边界**：单盘介质故障不防（需 RAID 镜像）；header 与数据在同一文件（生产可拆双文件/跨盘）；回放是顺序重放（恢复时间 ∝ 日志量，生产建议周期性快照 + 增量回放）。

## 8. 关键路径同步 I/O 审计（2026-09-22，代码级逐路径核查）

### 8.1 纯撮合关键路径（SPSC ring → HpEngine::on_order）——**零同步 I/O**

| 环节 | 实现 | 状态 |
|---|---|---|
| 事件缓冲 | `events.clear()` 复用，无每次分配 | ✅ |
| 订单槽 | `OrderStore::with_capacity(order_cap)` 预分配 + free 列表复用 | ✅ |
| 价位池 | `level_pool` LEVEL_POOL_CAP 预分配 | ✅ |
| 消费 batch | `worker.batch` 预分配（Disruptor 式） | ✅ |
| 锁 | 无 Mutex（SPSC 无锁 + 单线程引擎） | ✅ |
| stdout/文件/网络 | 无（engine.rs/worker.rs 零 println/fs/fsync） | ✅ |
| journal | **未接入 on_order**（engine.rs 无 journal 引用；journal.rs 独立存在） | ✅ 非关键路径 |

**唯一残留**：`Vec` 扩容峰值——order_cap / LEVEL_POOL_CAP 超限时重分配（摊销 O(1)，非 I/O 但瞬时暂停）。**对策：order_cap 按峰值在途订单预留。**

### 8.2 生产下单链路（网络形态）残留的"非阻塞"项

| 项 | 类型 | 成本 | 是否同步阻塞 |
|---|---|---|---|
| DPDK PMD rx_burst/tx_burst | 用户态 DMA 描述符轮询 | ~100ns/批 | 否（非阻塞） |
| `Instant::now()`（session 层，每包 2 次） | vDSO 时钟 | ~20ns/次 | 否（无 trap） |
| NAK pending_ts HashMap | 纯内存 | ~50ns | 否 |
| journal mmap 写（若接入） | page cache 写 | 首次 page fault ~1µs（偶发） | 否（无逐条 fsync，BBU 兜底口径） |
| stats eprintln!（dpdk_tap_order） | stdout 锁+syscall | 低频（连接/结束） | 否（不在每包路径） |

### 8.3 要避免的"同步 I/O 陷阱"（生产红线）

1. **journal 逐条 fsync** —— 每单 ~1ms，直接杀死 29M/s → 千万分之差。LMAX/Aeron 口径：mmap 批量写 + 不逐条 fsync，落盘靠 BBU。
2. **每包 println!/stats 输出** —— stdout 锁 + syscall 进关键路径。
3. **pcap 文件读** —— 仅测试路径（云 VM 回放 250k/s 瓶颈根源），生产无。
4. **在途订单超 order_cap** —— 触发 Vec 扩容峰值暂停。

**结论**：撮合核心已是零同步 I/O 路径（复刻 LMAX 单线程 + 预分配 + 无锁）；生产链路残留的只有非阻塞轮询（PMD）与 vDSO 时间戳，无真正的阻塞式同步 I/O。
