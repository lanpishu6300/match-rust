# 架构

**English：** [en/Architecture.md](../en/Architecture.md)

完整说明见：[docs/ARCHITECTURE.md](../../ARCHITECTURE.md)

## 双轨规则

1. **`match-contract` 生产默认**始终为 `match-core`（Java 可观测等价）。
2. **`match-core-hp`** 为实验轨（定点、干净语义），仅通过 `--features hp-engine` 启用。
3. 不要把 Java quirk 测试混进 HP，也不要在等价轨热路径做 HP 式分配优化。

## Crate 地图

```text
match-protocol   → 共享 DTO / 校验
match-core       → 等价引擎（默认）
match-core-hp    → 高性能引擎（tick/lot、LevelIndex、可选 art）
match-contract   → 进程壳（配置、恢复、Redis、worker、健康检查）
match-spot       → 现货壳（占位）
match-replay     → 黄金 NDJSON 回放
match-bench      → criterion + fair_compare
match-wal        → 异步批写 WAL（实验）
```

## 入站路径（合约）

```text
MQ/JSON（或 memory）→ 校验/转换 → 按 symbol worker
  → match-core::Engine（默认）
  → 出站 push / 深度 / 指标
```

开启 `hp-engine`：adapter → `HpEngine` / `HpWorker`，并在 `/metrics` 上报 L2/L3/L1 span。

## 相关设计

- [等价轨设计](../../specs/2026-07-17-rust-match-engines-design.md)
- [HP 设计](../../specs/2026-07-18-match-core-hp-design.md)
- [PE 优化设计](../../specs/2026-07-18-pe-optimizations-design.md)
