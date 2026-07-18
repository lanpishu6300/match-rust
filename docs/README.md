# Documentation index

Self-contained docs for the **match-rust** GitHub repository (no links outside this tree).

## Start here

| Doc | Audience |
|-----|----------|
| [wiki/zh/Home.md](./wiki/zh/Home.md) | 中文 Wiki（入门 / FAQ / 路线图） |
| [wiki/en/Home.md](./wiki/en/Home.md) | English Wiki |
| [../README.zh-CN.md](../README.zh-CN.md) | 中文 README |
| [../README.md](../README.md) | English README |
| [ARCHITECTURE.md](./ARCHITECTURE.md) | Contributors — crate boundaries |
| [COVERAGE.md](./COVERAGE.md) | 100% branch coverage gate |
| [../CONTRIBUTING.md](../CONTRIBUTING.md) · [../CONTRIBUTING.zh-CN.md](../CONTRIBUTING.zh-CN.md) | How to change code |
| [../SECURITY.md](../SECURITY.md) · [../SUPPORT.md](../SUPPORT.md) | Security & support |
| [../CHANGELOG.md](../CHANGELOG.md) | Releases |

## Designs (specs)

| Spec | Topic |
|------|-------|
| [2026-07-17-rust-match-engines-design.md](./specs/2026-07-17-rust-match-engines-design.md) | Equivalence track + cutover |
| [2026-07-18-match-core-hp-design.md](./specs/2026-07-18-match-core-hp-design.md) | HP dual-track |
| [2026-07-18-pe-optimizations-design.md](./specs/2026-07-18-pe-optimizations-design.md) | PE-inspired A→B→C |

## Plans

| Plan | Topic |
|------|-------|
| [2026-07-17-rust-match-engines.md](./plans/2026-07-17-rust-match-engines.md) | Equivalence tasks |
| [2026-07-18-match-core-hp.md](./plans/2026-07-18-match-core-hp.md) | HP tasks |
| [2026-07-18-pe-optimizations.md](./plans/2026-07-18-pe-optimizations.md) | Cache / ART / wal |

## Operations & performance

| Doc | Topic |
|-----|-------|
| [best-practices.md](./best-practices.md) | OSS → code map |
| [e2e-budget.md](./e2e-budget.md) | L1–L5 latency budget |
| [fair-compare.md](./fair-compare.md) | Non-zero fill-rate protocol |
| [bench-results.md](./bench-results.md) | Published numbers |
| [l3-shadow.md](./l3-shadow.md) | Shadow validation |
| [cutover-runbook.md](./cutover-runbook.md) | Grey cutover |
| [rmq-spike.md](./rmq-spike.md) | RocketMQ wiring status |
