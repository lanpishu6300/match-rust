# multiraft × match-rust 撮合复制（RAFT 整合文档）

> 对应 multiraft README 的**二期**："在下游应用中接入 RMQ Leader propose 与可插拔撮合 FSM"。
> 本仓库是下游（撮合引擎），multiraft 是被整合的高可用库。
> 仓库位置：`~/bifinance/match-rust`（本仓库，FSM 适配层 + 文档）、`~/bifinance/multiraft`（multiraft 本体 + 整合 demo）。

## 1. 为什么用 multiraft（而非自研复制 / 引入 Aeron Cluster）

| 维度 | multiraft | Aeron Cluster / 自研 |
|---|---|---|
| 复制模型 | openraft-multi：**每交易对（symbol）一个 Raft Group**，组间互不阻塞 | 单 Cluster 全量复制，扩 symbol 需扩整个集群 |
| 连接 | 节点间连接复用，O(节点) 而非 O(节点×group) | 每 group 独立流 |
| FSM | **可插拔**（`StateMachine` trait），本次接入 HpEngine | 需按 Cluster 语义重写 |
| sync 档位 | 0（page-cache）/1（fdatasync）/2（fsync），与 Aeron 对齐 | Aeron 默认 mmap 不主动落盘 |
| 运维 | Standby（learner）/ promote / demote，快照卸载 | 集群成员管理重 |

Multi-Raft 的关键收益：**故障/慢节点只影响该 symbol 的 group**，其余 symbol 的撮合不阻塞；扩展 symbol 只加 group，不加物理节点。

## 2. 架构

```text
                         ┌──────────────────────────────┐
  下单接入（DPDK UDP / SoupBinTCP / TCP RPC）
        │  序列化撮合命令（Limit/Cancel/Market + idem）
        ▼
  ┌──────────────────┐   propose (quorum 提交 + apply)   ┌──────────────────────┐
  │  RMQ Leader      │ ────────────────────────────────► │  multiraft 节点 (×N)  │
  │   （任一组有 leader）│                                   │  每 group 一个 Raft   │
  └──────────────────┘                                   │  Group（= symbol）    │
                                                         │                      │
                                                         │  FSM: MatchFsm       │
                                                         │   └─ HpEngine         │
                                                         │      （确定性重放）     │
                                                         └──────────────────────┘
                                                                   │
                                              ApplyOut.effects（Fill/Rest/Revoke 序列化）
                                                                   ▼
                                                            成交回报订阅（后续）
```

- **日志条目** = 序列化撮合命令（`crates/match-raft-fsm` 的 `encode_command`，定长编码，含客户端幂等键 `idem`）。
- **apply** = `engine.on_order(cmd)`；多节点对同一命令序列确定性重放 → 各节点撮合簿状态逐命令一致。
- **线性一致写**：`propose` Ok = quorum 提交 + 本节点 apply。
- **幂等**：`(group, idem)` 集合去重，防止 propose 超时重试 / leader 重放导致重复撮合（超时 outcome 未知，客户端必须带同一 idem 重试）。

## 3. 代码布局

### 3.1 本仓库（match-rust）

- `crates/match-raft-fsm/` —— **FSM 适配层**（核心整合代码）
  - `MatchFsm`：`multiraft_fsm::StateMachine` 实现，每 group 一个 `HpEngine`
  - `encode_command / decode_command`：撮合命令 ⇄ 日志条目（42B Limit / 25B Cancel / 34B Market）
  - `encode_events`：撮合事件（Fill/Rest/Revoke）→ `ApplyOut.effects`（propose 回报通道）
  - `snapshot`（一期）：累积命令字节 + 幂等集合（bincode），restore 全量重放重建引擎
  - `summary()`：`applied_orders / fills / revokes / best_bid / best_ask / live_orders`，多节点一致性核对
- 跨 workspace 依赖显式化（`match-core-hp`、`match-protocol` 的 Cargo.toml 去掉 `workspace = true`，使 multiraft 侧可 path 引用）

### 3.2 multiraft 仓库（被整合方）

- `crates/multiraft-fsm/` Cargo.toml 显式化（外部 path 引用所需）
- `crates/multiraft-net/src/multiraft.rs` 新增 `MultiRaft::start_cluster_with`（自定义 FSM 工厂的进程内多节点集群）；`wait_for_leader` 泛型化
- `crates/multiraft-demo-match/` —— **整合 demo**（本仓库的 demo bin 在 multiraft 仓内，因为 multiraft-net 依赖在其 workspace 内）
  - `--nodes 3 --groups 4 --orders N --sync 0|1|2 --pipeline P`
  - phase-1：leader 顺序/深流水线 propose + 吞吐延迟 + 全节点一致性核对
  - phase-2：shutdown leader → 重选 → 续 propose → 存活节点一致性核对

## 4. 多节点一致性验证（实测）

### 4.1 语义正确性（小规模，orders=3000/group）

- 3 节点 × 4 group，每 group 3000 单（B/S 交替 + 每 4 单撤 1 单）
- phase-1：全部节点 `MatchSummary` 逐 group 完全一致（`applied_orders=3000, fills=258, revokes=750`）
- phase-2：shutdown group-0 leader 后重选，续 propose 300 单/group，存活节点仍一致（`applied_orders=3300`，revokes=825=3300/4 精确符合"每 4 单撤 1"）
- 结果：`ALL CHECKS PASSED`

### 4.2 三档 sync 吞吐/延迟（orders=20000×4 group，Mac 本机）

| sync | 档位 | pipeline | TPS | p50 | p99 |
|---|---|---|---|---|---|
| 0 | 内存 log | 1（单飞） | 20.9k | 46µs | 94µs |
| 0 | 内存 log | 64（深流水线） | 待补 | 待补 | 待补 |
| 1 | Os（page-cache，Aeron 0） | 1（单飞） | 35 | — | — |
| 1 | Os | 64 | 待补 | 待补 | 待补 |
| 2 | All（fsync，Aeron 2） | 64 | 待补 | 待补 | 待补 |

> 单飞档 = 每单一次 quorum round-trip（延迟最优）；深流水线档 = `propose_batch` 并发等待（吞吐最优，生产形态）。
> sync=1 单飞 35 TPS 对应"每单一次 fdatasync + quorum"的真实持久路径；生产用深流水线 + group-commit 摊薄落盘成本。

## 5. 快照语义与一期边界（诚实标注）

- **一期快照**：`bincode(累积命令 + 幂等集合)`，restore 全量重放。空间 ∝ 命令量（20k 单/group ≈ 0.9MB），**非生产紧凑快照**。
- **生产快照**（后续）：导出 HpEngine 状态（book 各级别数量/FIFO + order store + client 在途映射），restore 直接装载。当前 `HpEngine` 无状态导出接口，需新增 `export_state()/import_state()`（不触碰热路径）。
- **回报通道**（后续）：`ApplyOut.effects` 目前仅承载序列化事件，未接回调/订阅；生产需在 FSM apply 后广播一致性事件流（或 propose 侧另设回报链路）。

## 6. 运行方式

```bash
# match-rust（FSM 层）
cargo build -p match-raft-fsm

# multiraft（demo，需要两仓相对位置 ~/bifinance/{match-rust,multiraft}）
cd ~/bifinance/multiraft
cargo run -p multiraft-demo-match --release -- \
  --nodes 3 --groups 4 --orders 20000 --sync 0 --pipeline 64
# 落盘档：
cargo run -p multiraft-demo-match --release -- \
  --nodes 3 --groups 4 --orders 20000 --sync 1 --pipeline 64 --data-dir /tmp/mr
```

## 7. 未决问题 / 后续

1. 跨 workspace path 引用 vs 发布/复制 crate：当前要求两仓同机相对位置 `~/bifinance/`；生产化建议把 `match-raft-fsm` 独立发布（crates.io / 内部 registry）。
2. `propose_batch` 部分失败语义：返回 Err 时部分条目已提交，客户端靠 idem 重试——需在客户端封装"整批成功后推进水位"。
3. 故障转移测试仅覆盖 leader shutdown（进程内）；需补充分区（partitioner）、慢 follower、跨进程 gRPC（`start_grpc` + 自定义 FSM 的泛型版本）验证。
4. 快照生产化 + Standby（learner）读卸载组合验证。
