# 公平对打协议（match-core-hp vs match-core vs perpetual_exchange）

**English：** [fair-compare.md](./fair-compare.md)

## 规则（必须满足）

1. **同一命令序列**：先挂后吃，价时优先可复现（固定 tick/qty）。  
2. **成交率 > 0**：报告 `fill_rate = fills / orders`；为 0 的结果标为 **INVALID**（`crypto-exchange` ART+SIMD 报告曾出现 0% 成交，不得与有效结果横比）。  
3. **只测撮核**：关闭限流、日志落盘、HTTP、账户强平。  
4. **同机同编译优化**：`-O3` / `cargo --release`。  
5. **统一指标**（CSV 列见下）。

## 运行（本仓库）

```bash
export PATH="$HOME/.cargo/bin:$PATH"
# from repo root
cargo run -p match-bench --release --bin fair_compare -- --n 50000
# ART index path (same fill_rate expected):
cargo test -p match-core-hp --features art --test art_parity
```

输出：stdout CSV + 校验 `fill_rate > 0`（否则 exit 1）。

## CSV 列

```text
engine,scenario,n_orders,n_fills,fill_rate,elapsed_ns,orders_per_sec,fills_per_sec,ns_per_order
```

- `engine`: `match-core` | `match-core-hp`  
- `scenario`: `fair_cross`（50% sell rest + 50% buy cross，期望 fill_rate ≈ 0.5）

## 对打 perpetual_exchange（C++）

1. 在 `crypto-exchange` 用**相同语义**构造：N/2 卖 @100、N/2 买 @100，qty=1，确认成交笔数 ≈ N/2。  
2. 只跑 matching_engine / orderbook 微基准，不用 Production 限流版。  
3. 把吞吐/延迟填进同一 CSV 模板，`engine=perpetual_exchange_<variant>`。  
4. 若某 variant `fill_rate==0`，标注 INVALID，不参与排名。

## 解读

| 对比 | 含义 |
|------|------|
| hp vs core | 定点价位簿 vs BigDecimal/BTreeSet（本仓库已自动化） |
| hp vs C++ Original | 相近「真实成交」微核 |
| hp vs C++ ART+SIMD（fill_rate=0） | **禁止**当作更快 |

端到端预算见 [`e2e-budget.md`](./e2e-budget.md)。
