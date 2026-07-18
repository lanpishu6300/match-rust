# match-contract 按 symbol 灰度切流手册

**English：** [cutover-runbook.md](./cutover-runbook.md)

按交易对从 Java `java-contract-match` 切到 Rust `match-contract`。对齐设计规格 [§4.3](./specs/2026-07-17-rust-match-engines-design.zh-CN.md#43-灰度切换)。

**硬约束：** 对给定 symbol，在任意时刻只能有**一个活跃消费者**以组 `usdt_contract_match_channel_one_group` 读取 `usdt_contract_match_order_{symbol}`。双消费会撕裂订单簿。

---

## 前置条件

- [ ] L2 黄金回放绿灯：`cargo test -p match-replay`
- [ ] 目标 symbol 已完成 L3 影子或离线回放 — 见 [`l3-shadow.zh-CN.md`](l3-shadow.zh-CN.md)
- [ ] 测试环境全壳冒烟：restore RPC、Redis 清理/链路、入出站、错误队列
- [ ] Rust 镜像/配置已部署，`match.symbols_whitelist` 仅含切流 symbol
- [ ] 看板已接 Rust `/metrics`（OTel 对齐名）与 Java 基线对比
- [ ] On-call 已熟悉下方回滚步骤

---

## 阶段 1 — 预热（无生产流量）

1. 部署 Rust `match-contract`，**不**订阅生产入站 Topic（或空白名单 / 无 consumer）。
2. 确认 `/healthz` 返回 `200`；bootstrap 完成前 `/readyz` 为 `503`，完成后 `200`。
3. 对目标 symbol 跑 L2 + L3 验证。
4. 在测试环境核对 RPC 恢复路径：
   - `GET {market_base_url}/contract-market/contractcoinMarketList`
   - `GET {order_base_url}/contract/entrust-list`

---

## 阶段 2 — 按 symbol 切流

对每个 symbol `S`（小写 key，如 `btcusdt`）重复：

### 2.1 停止 Java 对 S 的消费

- [ ] 停止或重配 Java，使 `usdt_contract_match_channel_one_group` 内**没有任何实例**消费 `usdt_contract_match_order_{S}`。
- [ ] 若 Java 单进程跑全部 symbol，可选：
  - 临时从 Java 活跃集合移除 `S`（若支持），**或**
  - 切该实例最后一个 symbol 时停掉整个 Java。
- [ ] 等待 `S` 在途消息排空（或接受短暂双停、无消费者窗口）。

### 2.2 确认单一消费者

- [ ] RocketMQ 控制台 / 运维：生产组 `usdt_contract_match_channel_one_group` 上 topic `usdt_contract_match_order_{S}` 的 Java consumer 为 **零**。
- [ ] 无其他影子/预发误用生产组。

### 2.3 为 S 启动 Rust

- [ ] 设置 `match.symbols_whitelist: ["S"]`（或把 `S` 加入现有白名单）。
- [ ] 启动 Rust；确认 bootstrap 顺序：
  1. `startup_delay_ms` 结束
  2. 拉取 markets；应用 shard 过滤
  3. 重置 `S` 的 Redis depth key + link key
  4. 经 RPC 恢复挂单 → START_QUEUE / BigNo 填充
  5. Consumer 已订阅；`/readyz` → `200`
- [ ] Producers 启用（默认）：push_order、push_market、no_deal、deeps、robot。

### 2.4 切后观察（≥ 30 分钟）

相对 Java 基线关注回归：

| 信号 | 异常时动作 |
|------|------------|
| `match.orders.inbound.invalid.total` 飙升 | 查 payload/schema 漂移；考虑回滚 |
| Redis `poc_redis_send_mq_error_data_queue` 增长 | MQ 发送失败；持续则回滚 |
| Depth Topic 陈旧 / 空 | 查 worker 日志、节流配置 |
| 相对 contract-order 的报单推送滞后 | RPC/MQ 延迟；核对委托状态 |
| `match.order_book.remove_failed.total`（接好后） | 簿不一致；回滚 |

- [ ] 抽检：在 `S` 下限价、部分成交、撤单、市价。
- [ ] 确认下游 contract-order / market 正常消费 Rust 出站。

### 2.5 扩面

- [ ] 白名单加下一 symbol，或按 symbol 组部署独立 Rust 实例。
- [ ] 全部 symbol 稳定 ≥ 24h 前保留 Java 包/实例以便回滚。

---

## 回滚

**触发：** 出站错误队列持续增长、深度空白 > N 分钟、订单对账不一致、簿 remove 失败显著高于基线。

1. **停 Rust**（白名单移除 `S` 或杀进程）。
2. **确保队列空闲** — 短暂无消费者可接受；避免双活跃消费者。
3. **启 Java** 对 `S`，走同等恢复路径（`InitLoadData` 等价）：
   - 启动时 Redis depth wipe + link key
   - 分页挂单恢复
   - START_QUEUE / BigNo / 720s TTL 生效
4. **Java 订阅** `usdt_contract_match_order_{S}`，组 `usdt_contract_match_channel_one_group`。
5. 核对 Java `/health`（端口 `31015`）与生产指标回到基线。
6. 记事故说明；保留 Rust 日志与 MQ offset 供复盘。

---

## 配置参考

| Key | 用途 |
|-----|------|
| `match.symbols_whitelist` | 灰度期间限制活跃 symbol |
| `shard` | 须与 Java `SHARD` 一致以过滤 market |
| `start_queue_ttl_ms` | 默认 `720000` — 切流窗口保持 |
| `health.port` | 默认 `31015`（对齐 Java `server.port`） |
| `rocketmq.consumer_group` | 生产切流须保持 `usdt_contract_match_channel_one_group` |

---

## 相关文档

- [Rust 撮合引擎设计规格](./specs/2026-07-17-rust-match-engines-design.zh-CN.md)
- [实现计划 §4.3](./plans/2026-07-17-rust-match-engines.zh-CN.md)
- [L3 影子模式](l3-shadow.zh-CN.md)
