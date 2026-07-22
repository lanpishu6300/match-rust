# match-core vs match-core-hp 基准结果

**English：** [bench-results.md](./bench-results.md)

首次发布的 `match-bench`（`engine_cmp`）数字。

## 环境

| 项 | 值 |
|----|-----|
| 日期 | 2026-07-18 |
| 主机 | macOS 14.4.1, Apple M1 Pro (arm64) |
| Rust | rustc 1.97.1 (8bab26f4f 2026-07-14) |
| 命令 | `cargo bench -p match-bench --bench engine_cmp -- --sample-size 20` |
| 负载规模 | 每场景 50_000 条命令 |
| Criterion | 0.5，sample size 20 |

## 说明

- Core 路径每次 `on_order` 克隆 `BbOrder`（Java 形态 DTO 成本是刻意的）。
- HP 路径使用 `Copy` 的 `HpCommand`（不因额外 clone 噪声不公平地偏袒 HP）。
- HP 语义为干净的限价/市价/撤单 — **非** Java 等价。
- Ratio = `core_median_time / hp_median_time`（越大 ⇒ HP 越快）。

## 结果（整段负载 wall time 中位数）

| 场景 | core | hp | Ratio (core/hp) |
|------|------|-----|-----------------|
| `rest_only` | 39.229 ms | 904.95 µs | **~43.3×** |
| `cross_full` | 60.028 ms | 899.35 µs | **~66.7×** |
| `partial_walk` | 104.56 ms | 1.6452 ms | **~63.6×** |
| `cancel_hot` | 524.18 ms | 9.9348 ms | **~52.8×** |
| `mixed` | 49.617 ms | 628.37 µs | **~79.0×** |

## 验收

规格目标：`cross_full` 与 `partial_walk` ≥5×。

| 场景 | 目标 | 实测 | 状态 |
|------|------|------|------|
| `cross_full` | ≥5× | ~66.7× | PASS |
| `partial_walk` | ≥5× | ~63.6× | PASS |

## 公平对打（`fair_cross`，fill_rate 必须 > 0）

```bash
cargo run -p match-bench --release --bin fair_compare -- --n 50000
```

| engine | n_orders | n_fills | fill_rate | orders/s | ns/order | notes |
|--------|----------|---------|-----------|----------|----------|-------|
| match-core | 50000 | 25000 | 0.50 | ~764K | ~1308 | 同上机 |
| match-core-hp | 50000 | 25000 | 0.50 | ~55M | ~18 | 相对 core wall ~72× |

协议：[`fair-compare.zh-CN.md`](fair-compare.zh-CN.md)。不要与零成交的 ART/SIMD 峰值横比。

## Phase A 增量（2026-07-18 晚）

- 最优价缓存 + level pool + `LevelIndex`；可选 `--features art`（字节 radix）。
- 对等：`cargo test -p match-core-hp --features art --test art_parity`
- `match-contract` feature `hp-engine`：`/metrics` 中 L2/L3/L1 span 计数
- `match-wal` 异步基准（样例）：M1 Pro 上 append+flush ~11M records/s（`wal_bench 100000`）

说明：client_id map 之后，`fair_compare` hp 相对 core 约 25–70×（视机器负载）；始终要求 fill_rate≈0.5。

## HP L1 热路径（2026-07-21）

撤单 O(1) map、撮合环收紧、档位 `get_or_insert_with`。细节与压测指标：[`perf-hotpath-2026-07.zh-CN.md`](./perf-hotpath-2026-07.zh-CN.md)。

`fair_cross`（HP，n=50000，fill_rate=0.5，8 次中位数）：约 54.3 ns/order（约 18–19M orders/s）。同日改动前基线约 56.3 ns/order。

## 分层压测（2026-07-22）

挂单深度 × 流长 × 成交强度（`tier_sweep`）。方法、结果与瓶颈：[`perf-tier-sweep.zh-CN.md`](./perf-tier-sweep.zh-CN.md)。Quick high 格约 143 → 81 ns/order；最慢仍是 high × 200k stream（约 180–190 ns/order）。
