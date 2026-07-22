# 分层压测：深度 × 流长 × 成交强度（2026-07-22）

**English：** [perf-tier-sweep.md](./perf-tier-sweep.md)

设计：[specs/2026-07-22-tier-sweep-design.zh-CN.md](./specs/2026-07-22-tier-sweep-design.zh-CN.md)。  
CSV：[bench-results/tier-sweep-final.csv](./bench-results/tier-sweep-final.csv)。  
改动前 quick 日志：[bench-results/tier-quick-pre-opt.txt](./bench-results/tier-quick-pre-opt.txt)。

---

## 环境

| 项 | 值 |
|----|-----|
| 日期 | 2026-07-22 |
| 主机 | macOS 14.4.1，Apple Silicon（`arm64`） |
| Rust | rustc 1.97.1，release |
| 引擎 | `match-core-hp`（BTree 档位索引；未开 ART） |
| 命令 | `tier_sweep --preset default --runs 5` |

Warm 不计时；只计时 stream；warmup 1 次后取 5 次中位数。门禁：low `0.10±0.05`，mid `0.50±0.05`，high `≥1.5`。

```bash
make tier-quick
make tier-sweep
```

---

## 门禁

已发布 CSV 上 27 格全部通过：

| 带 | 规则 | 实测 |
|----|------|------|
| low | 0.10 ± 0.05 | 全部 `0.10` |
| mid | 0.50 ± 0.05 | 全部 `0.50` |
| high | ≥ 1.5 | 全部 `2.00` |

---

## 结果（中位数 `ns/order`）

### 按常驻深度

**rest = 1 000**

| stream | low | mid | high | high `peak_mapped` |
|--------|-----|-----|------|--------------------|
| 10k | 43.9 | 19.4 | 69.2 | 21 000 |
| 50k | 38.4 | 23.6 | 75.6 | 101 000 |
| 200k | 37.4 | 31.6 | 190.7 | 401 000 |

**rest = 10 000**

| stream | low | mid | high | high `peak_mapped` |
|--------|-----|-----|------|--------------------|
| 10k | 49.1 | 22.5 | 62.2 | 30 000 |
| 50k | 41.2 | 24.4 | 81.5 | 110 000 |
| 200k | 38.3 | 36.8 | 182.8 | 410 000 |

**rest = 100 000**

| stream | low | mid | high | high `peak_mapped` |
|--------|-----|-----|------|--------------------|
| 10k | 50.3 | 27.1 | 68.1 | 120 000 |
| 50k | 38.5 | 30.5 | 103.5 | 200 000 |
| 200k | 39.9 | 39.3 | 189.6 | 500 000 |

### 对 `rest` 取平均

| stream | low | mid | high |
|--------|-----|-----|------|
| 10k | 47.8 | 23.0 | 66.5 |
| 50k | 39.3 | 26.2 | 86.8 |
| 200k | 38.5 | 35.9 | 187.7 |

mid 始终最便宜；low 几乎不随流长变差；high 随流长陡增，而不是随 rest 深度。

### 改动前后（quick 格）

`rest=10k stream=50k high`，同机：

| 阶段 | ns/order | orders/s |
|------|----------|----------|
| 改动前 | 143.1 | 7.0M |
| 改动后 | 81.5 | 12.3M（单笔 −43%） |

其余 quick 格同向改善（low/mid 大约 −25%～−45%）。  
改动后 `fair_compare --n 50000`：HP `fair_cross` ≈ 25.6 ns/order（fill_rate 0.5，仅作同日核对）。

---

## 本次代码改动

1. `client_to_id` 使用 `FxHashMap`。
2. 仅在订单挂簿时写入外部 id map；完全成交的限价与市价不插入。
3. Match 循环本地累计 `taker_open`，走完后写回 store 一次。
4. `fill_order` 只对已有档位 `get_mut`（不经 `level_mut` 分配）。
5. 更大 free-list、预填 level pool、O(1) `live_len`、bench `event_cap=256`。

Level FIFO 试过 `Vec`+head，high×200k 相对 `VecDeque` 回退，已还原。

---

## 瓶颈分析

### 矩阵说明了什么

| 结论 | 证据 |
|------|------|
| rest 深度不是 high 扫档主因 | high×200k：rest 1k / 10k / 100k → 190.7 / 182.8 / 189.6 ns |
| 流长才是 high 扫档主因 | high 平均 ns：66.5 → 86.8 → **187.7**（stream 10k → 50k → 200k） |
| mid 轻度吃深簿 | mid@50k：23.6 → 30.5 ns（rest 1k → 100k） |
| low 偏插入成本 | low 平均约 38–48 ns，对流长平坦 |
| high 的 map 占地跟卖单种子走 | `peak_mapped ≈ rest + 2×stream` |

### High 单笔成本

计时买单 qty=2 手 → 两笔 fill。路径：taker 入 store → 两次 maker 成交（FIFO 扣减、可能删空档、`client_to_id.remove`）→ 去掉耗尽的 taker。完全成交的 taker 不进 map。

近似每 fill 纳秒（`ns/order ÷ 2`）：

| stream | ns/fill | `peak_mapped`（rest≈1k） |
|--------|---------|--------------------------|
| 10k | ~31–35 | 21k |
| 50k | ~38–52 | 101k |
| 200k | ~91–95 | 401k |

形态不变、200k 时每 fill 明显变贵：大活 map + store 拆除，不是另一套算法。

### 成本排序（本轮之后）

1. **长 high 扫档上的 maker map/store 拆除** — warm 把 `2×stream` 卖单写入 map；计时路径逐个摘掉。stream=200k 时约 40 万次 map remove 与 store free，峰值 map 约 40–50 万项。主导 high×200k；rest 几乎不动针。
2. **扫约 2k tick 时的空档 / 最优价维护** — 当前种子跨度下次要；扫档更宽时会抬头。
3. **深 rest 旁的 mid（及 high@50k）** — 闲置买盘抢缓存；相对 (1) 是二阶。
4. **事件 `Vec` push** — 2 fills/order + `event_cap=256` 可忽略。

推迟 taker 入 map 与 `FxHashMap` 对短 high 帮助大，消不掉长扫档上的 maker remove。再往下需要：不可撤流动性少进 map、更密的 store slot，或 profile 确认空档成本后再上更密卖盘索引。

扫档型簿上优先动 **流长 × 成交强度**，不要把 rest 深度当第一优化目标。

---

## 小结

门禁全绿；同带内横比。剩余断崖是 **高成交 × 长 stream**（大活 map 下的 maker `client_to_id` + store 拆除）。rest 深度是二阶。短 mid/high 在 quick high 格上约改善 40%；长 high 还要表示层或种子策略改动，单靠撮合环微优化不够。
