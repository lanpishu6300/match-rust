# 架构说明

**English：** [ARCHITECTURE.md](./ARCHITECTURE.md)

## 双轨规则

1. **`match-contract` 生产默认**始终是 `match-core`（与 Java 可观测结果等价）。
2. **`match-core-hp`** 为实验轨：语义更干净、定点热路径。仅在进程壳通过 `--features hp-engine` 启用。
3. 不要把 Java quirk 测试混进 `match-core-hp`，也不要把 HP 分配带进 `match-core` 热路径。

## Crate 职责

```text
crates/
├── match-protocol/   # BbOrder / MqOrder DTO、校验、symbol key 编码
├── match-core/       # 等价引擎（BigDecimal / BTreeSet 形态）
├── match-core-hp/    # HP 引擎（tick/lot、LevelIndex、SPSC、可选 art）
├── match-contract/   # 进程：配置、恢复 RPC、Redis、worker、健康检查
├── match-spot/       # 现货壳桩
├── match-replay/     # 黄金 NDJSON 回放 CLI
├── match-bench/      # Criterion + fair_compare 二进制
└── match-wal/        # 异步批写 WAL（非默认路径）
```

## 热路径约束（`match-core-hp`）

- `on_order` 上无 JSON / Tokio / `Mutex`
- 优先 `i64` tick/lot；仅在 `adapter` 边界转换
- 每个 symbol 簿单一写者（`HpWorker` / 单线程 worker）

## 消息

入站/出站 Topic 名与 JSON 形态面向与 Java 灰度切流。RocketMQ 落地前使用 `rocketmq.transport: memory`（见 `docs/rmq-spike.md`）。

## 可观测性

`match-contract` 在 `/metrics` 暴露与 Java OTel 对齐的 Prometheus 文本（`match.order.events.total` 等）。HP 路径另累计 `match.span.l{1,2,3}_*_ns_total`。
