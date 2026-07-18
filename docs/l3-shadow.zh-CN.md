# match-contract 的 L3 影子验证

**English：** [l3-shadow.md](./l3-shadow.md)

L3 是上线前的等价门禁：用**真实或录制的入站流量**跑 Rust，且不影响下游消费者。见设计规格 [§4.2 L3](./specs/2026-07-17-rust-match-engines-design.zh-CN.md#42-match-replay-三层)。

任何 L3 工作前，L2（`cargo test -p match-replay`）必须绿灯。

---

## 模式 A — 离线回放（CI / 可复现优先）

1. **录制**低流量 symbol 在代表性窗口（如 1–4 小时）内 `usdt_contract_match_order_{symbol}` 的入站 JSON 数组。
   - 原始消息体存为 NDJSON，或每批一个 JSON 文件（与 Java consumer 收到的格式一致）。
2. **Java golden 导出：** 将录制序列跑过 Java 参考（或 `java-contract-match` JUnit 既有导出）→ `GoldenTrace` NDJSON（成交、簿快照、深度）。
3. **Rust 回放：** 同一 `MqOrder` 序列喂给 `match-replay` / 本地 `MemoryMessageSource`：
   ```bash
   export MATCH_CONTRACT_CONFIG=crates/match-contract/config.example.yaml
   export MATCH_CONTRACT_LOCAL_SYMBOLS=btcusdt
   # file-channel 时把录制批次预置到 {memory_dir}/in/
   cargo run -p match-contract
   ```
4. **Diff：** 对比 Rust 引擎事件 / 出站载荷与 golden；成交价量、余量、深度档位不一致则失败。

**特性：** 无生产副作用；完全可复现；无需 RocketMQ group 协调。

---

## 模式 B — 在线影子消费（只读）

Rust 跑完整进程壳（解析 → 校验 → 每 symbol worker → 引擎），但**不得**向下游 Topic 生产。

### 配置（文档意图；RMQ 适配器落地后再接线）

```yaml
rocketmq:
  consumer_group: "usdt_contract_match_channel_rust_shadow_group"  # 非生产组
  shadow_consume: true
  producers_enabled: false
```

| 设置 | 生产切流 | L3 影子 |
|------|----------|---------|
| Consumer group | `usdt_contract_match_channel_one_group` | `usdt_contract_match_channel_rust_shadow_group` |
| Producers | 启用 | **禁用** |
| 与 Java 抢消息 | 是（必须单一活跃） | **否** — 不同组，重复读 |

### 步骤

1. 仅对**低流量 symbol** 部署带影子组的 Rust。
2. Java 仍是出站 Topic 的**唯一生产者**（生产路径不变）。
3. Rust 并行消费同一入站 Topic（跨组重复投递属预期）。
4. 定期对比：
   - Rust 引擎内存深度 vs Java 快照（Redis depth key 或管理 API）
   - `/metrics`：`match.order.events.total`、`match.trades.deals.total`、`match.orders.inbound.invalid.total`
5. Diff 打 WARN；**不要**写 `push_order`、`push_market`、`no_deal`、`deeps`、`robot` Topic。

### 影子能证明

- 入站校验与 Java 对等
- 实盘流量下引擎状态发散检测
- 结合录制挂单快照时的 bootstrap + restore 正确性

### 影子不能证明

- 出站序列化字节级一致（设计上禁用生产）
- MQ 发送失败 / 错误队列行为（无 producer）
- 与生产组的 consumer offset 协调

---

## 模式 C — 先录制再离线影子

适合在线 diff 工具尚不成熟的 symbol：

1. 影子窗口内录制入站（模式 B，无 diff 工具）。
2. 在录制起点取挂单快照。
3. 用录制 + 快照离线回放 Java golden 与 Rust。
4. 仅当离线 diff 通过后再晋升到灰度切流。

---

## 退出标准（symbol 可切流）

- [ ] 该 symbol 录制语料 L2 回放绿灯
- [ ] L3 影子/离线：约定窗口内零成交/深度不一致
- [ ] `match.orders.inbound.invalid.total` 速率与 Java 在约定容差内
- [ ] 运维在 [`cutover-runbook.zh-CN.md`](cutover-runbook.zh-CN.md) 清单上签字

---

## 相关文档

- [灰度切流手册](cutover-runbook.zh-CN.md)
- [设计规格 §4.2–4.3](./specs/2026-07-17-rust-match-engines-design.zh-CN.md)
- [实现计划 Task 14](./plans/2026-07-17-rust-match-engines.zh-CN.md)
