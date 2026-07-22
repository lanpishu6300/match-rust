# 分档压测设计（2026-07-22）

**English：** [2026-07-22-tier-sweep-design.md](./2026-07-22-tier-sweep-design.md)

## 目标

在可控矩阵 **常驻深度 × 流长度 × 成交强度** 下测量 `match-core-hp`，把优化花在真正占 wall 的格子上。

结果与瓶颈说明：[`../perf-tier-sweep.zh-CN.md`](../perf-tier-sweep.zh-CN.md)。

## 入口

```bash
make tier-quick    # 4 格
make tier-sweep    # 默认 27 格
# 或：
cargo run -p match-bench --release --bin tier_sweep -- \
  --preset default --runs 5 --out docs/bench-results/tier-sweep-final.csv
```

| 参数 | 默认 | 含义 |
|------|------|------|
| `--preset` | `quick` | `quick`（4 格）或 `default`（27 格） |
| `--runs` | 5 | 每格计时次数；取中位数 |
| `--warmup` | 1 | 计时前丢弃的轮次 |
| `--out` | — | 可选 CSV（列与 stdout 相同） |
| `--loose` | false | 门禁失败仍打印行 |

本矩阵只测 HP。core 与 HP 对打用 `fair_compare`。

## 负载

1. **Warm（不计时）：** 灌 `rest` 档非穿越买单。high 另灌 `2×stream` 卖单，保证扫档有流动性。
2. **Timed：** 按成交带打 `stream` 条命令（见下表）。
3. **输出：** 中位数 `elapsed_ns` → `ns/order`、`orders/s`、`fills/s`、`fill_rate`、`peak_mapped`（计时阶段 `client_to_id` 峰值）。

## 预设矩阵

| rest | stream | 档位 | 形态 |
|------|--------|------|------|
| 1k / 10k / 100k | 10k / 50k / 200k | low | 挂单为主 + 稀疏 1:1 交叉 → `fill_rate ≈ 0.10` |
| 同左 | 同左 | mid | 半卖半买同价（`fair_cross` 风格） → `≈ 0.50` |
| 同左 | 同左 | high | 主动买（qty 2 手）吃种子卖单 → `fill_rate ≥ 1.5`（本次 2.0） |

**门禁：** low/mid 相对目标 ±0.05；high 要求 `fill_rate ≥ 1.5`。禁止零成交峰值。

`quick`：`(1k,10k,low)`、`(10k,50k,mid)`、`(10k,50k,high)`、`(100k,50k,mid)`。

## 读数约定

- 只在**同一成交带内**横比（形态相同）。跨带比较 ns/order 不当作排名。
- 安静机器上取多轮中位数；单轮尖峰当噪声。
- 发布时记下主机、rustc、CSV 路径，并在 [`../bench-results.zh-CN.md`](../bench-results.zh-CN.md) 挂链。

## 优化循环

1. 打基线 CSV（`default` 或 `quick`）。
2. 用矩阵归因（流长 / 深度 / 成交带），少靠直觉。
3. 每次只改一类成本（map、store、档位索引、事件缓冲），同预设复测。
4. 以最差格决定保留或回退；增量写在 [`../perf-tier-sweep.zh-CN.md`](../perf-tier-sweep.zh-CN.md)。
