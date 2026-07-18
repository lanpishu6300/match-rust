# Branch coverage policy

**中文：** [COVERAGE.zh-CN.md](./COVERAGE.zh-CN.md)

## Scope (hard gate: 100% branches)

| Crate | Gate |
|-------|------|
| `match-protocol` | **100%** branches |
| `match-core` | **100%** branches |
| `match-core-hp` (default + `--features art`) | **100%** branches |

## Soft gate (high coverage, race arms)

| Crate | Gate |
|-------|------|
| `match-wal` (lib; ignore `bin/`) | ≥98% lines; branch ~85%+ (flusher `Disconnected` race arm) |

## Out of scope / softer targets

| Crate | Notes |
|-------|-------|
| `match-replay` | Golden + diff; CLI `main.rs` ignored |
| `match-contract` | I/O shell (Redis/RPC/MQ); not in branch gate |
| `match-bench` / `match-spot` | Bench harness / stub |

Defensive or race-only arms may use `#[coverage(off)]` under `cfg(coverage)` with a short comment (runtime behavior unchanged).

## How to measure

Requires **nightly** (`rustup toolchain install nightly -c llvm-tools-preview`).

```bash
export PATH="$HOME/.cargo/bin:$PATH"
make cov          # summary for gated crates
make cov-html     # HTML report under target/llvm-cov/html
```

Equivalent:

```bash
cargo +nightly llvm-cov -p match-protocol -p match-core -p match-core-hp \
  --branch --ignore-filename-regex '(tests/)' --summary-only
# expect TOTAL Branches Cover 100.00%

cargo +nightly llvm-cov -p match-core-hp --features art \
  --branch --ignore-filename-regex '(tests/)' --summary-only
# expect TOTAL Branches Cover 100.00%
```

Do **not** pass `--no-cfg-coverage` for the gate run (needed for `#[coverage(off)]` on defensive arms).

CI script greps `Branches` / `100.00%` on the TOTAL line (`scripts/check-branch-coverage.sh`).
