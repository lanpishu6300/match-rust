# RocketMQ 客户端探查（Task 12）

**English：** [rmq-spike.md](./rmq-spike.md)

**日期：** 2026-07-17  
**目标 NameServer：** `192.168.0.241:9876`（来自 `config.example.yaml`）  
**结果：** **FAIL** — 生产 RocketMQ 适配器**尚未接通**

## 尝试内容

1. 以 2–3s 超时 TCP 连接 `192.168.0.241:9876`（Python `socket`、bash `/dev/tcp`、`nc -z`）。
2. 结果：**`No route to host`（os error 65）** / 本机不可达。无 NameServer 可达性时，30s 内无法完成收发。
3. crates.io 可拉取 `rocketmq-client-v4` `0.4.2`，但未接线 — 无 NS 则无握手。
4. 面向 Apache RocketMQ **4.x** broker 的候选 crate：
   - [`rocketmq-client-v4`](https://crates.io/crates/rocketmq-client-v4) `0.4.2` — 协议面向 RMQ 4.x
   - [`rocketmq-client-rust`](https://crates.io/crates/rocketmq-client-rust) / mxsm 栈 — 较新，MSRV 1.85+，主要对齐 Rust RocketMQ server 谱系

## 决策（DONE_WITH_CONCERNS）

在可达 NameServer + 确认 crate↔broker 握手之前：

| 部件 | 状态 |
|------|------|
| `OrderSink` / `MessageSource` traits | 已实现（`mq/traits.rs`） |
| Memory / file-channel 适配器 | 已实现（`mq/memory.rs`） |
| Topics、入站、出站、bootstrap、workers | 已接到 Engine + Redis + RPC |
| 线上 RocketMQ consumer/producer | **阻塞** |

`config.rocketmq.transport` 默认为 `memory`。设为 `rocketmq` 时当前会打日志并回退 memory。

## 解阻清单

1. 从部署/开发机可达 NameServer（`nc -vz 192.168.0.241 9876`）。
2. 选定能在 scratch topic 上 30s 内完成 producer send + consumer receive 的 crate。
3. 在 `MqTransport::Rocketmq` 后实现 `mq/rocketmq.rs` 并切换配置。
4. 重跑冒烟：restore 计数日志 + place/cancel → 编码 symbol topic 上的 `push_order` body。
