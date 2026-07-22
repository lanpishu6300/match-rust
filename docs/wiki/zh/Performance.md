# 性能

**English：** [en/Performance.md](../en/Performance.md)

## 原则

1. **禁止用 `fill_rate == 0` 的结果排名** — 标为 INVALID（见 [fair-compare](../../fair-compare.md)）。
2. 区分 **L1 撮核** 与 **端到端**（MQ/JSON 常占主导）— 见 [e2e-budget](../../e2e-budget.md)。
3. 已发布数字：[bench-results](../../bench-results.md)。

## 命令

```bash
make fair                          # fair_compare CSV；fill_rate≈0 则 exit 1
make tier-quick                    # 深度×流长×成交强度冒烟（4 格）
make tier-sweep                    # 完整 27 格矩阵
make bench                         # criterion engine_cmp
cargo test -p match-core-hp --features art --test art_parity
cargo run -p match-wal --release --bin wal_bench -- 100000
```

矩阵方法与数字见：[perf-tier-sweep](../../perf-tier-sweep.zh-CN.md)。
## 姊妹 C++ 项目

[crypto-exchange](https://github.com/lanpishu6300/crypto-exchange) 有 ART/SIMD 微基准。仅在同一 fair-cross 协议且成交率非零时横比。
