# 分支覆盖率策略

**English：** [COVERAGE.md](./COVERAGE.md)

## 范围（硬门禁：分支 100%）

| Crate | 门禁 |
|-------|------|
| `match-protocol` | **100%** 分支 |
| `match-core` | **100%** 分支 |
| `match-core-hp`（默认 + `--features art`） | **100%** 分支 |

## 软门禁（高覆盖，含竞态臂）

| Crate | 门禁 |
|-------|------|
| `match-wal`（lib；忽略 `bin/`） | 行覆盖 ≥98%；分支约 85%+（flusher `Disconnected` 竞态臂） |

## 不在门禁 / 较软目标

| Crate | 说明 |
|-------|------|
| `match-replay` | Golden + diff；CLI `main.rs` 忽略 |
| `match-contract` | I/O 壳（Redis/RPC/MQ）；不进分支门禁 |
| `match-bench` / `match-spot` | 基准夹具 / 桩 |

防御性或仅竞态可达的臂可在 `cfg(coverage)` 下使用 `#[coverage(off)]` 并附简短注释（运行时行为不变）。

## 如何度量

需要 **nightly**（`rustup toolchain install nightly -c llvm-tools-preview`）。

```bash
export PATH="$HOME/.cargo/bin:$PATH"
make cov          # 门禁 crate 摘要
make cov-html     # HTML 报告在 target/llvm-cov/html
```

等价命令：

```bash
cargo +nightly llvm-cov -p match-protocol -p match-core -p match-core-hp \
  --branch --ignore-filename-regex '(tests/)' --summary-only
# expect TOTAL Branches Cover 100.00%

cargo +nightly llvm-cov -p match-core-hp --features art \
  --branch --ignore-filename-regex '(tests/)' --summary-only
# expect TOTAL Branches Cover 100.00%
```

门禁运行**不要**传 `--no-cfg-coverage`（防御臂的 `#[coverage(off)]` 需要）。

CI 脚本对 TOTAL 行 grep `Branches` / `100.00%`（`scripts/check-branch-coverage.sh`）。
