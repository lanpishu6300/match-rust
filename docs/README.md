# Documentation index

**中文：** [README.zh-CN.md](./README.zh-CN.md)

Self-contained docs for the **match-rust** GitHub repository (no links outside this tree, except companion repos called out explicitly).

Convention: English `Foo.md` + Chinese `Foo.zh-CN.md`, with a language switcher at the top of each file.

## Start here

| Doc | Audience |
|-----|----------|
| [wiki/en/Home.md](./wiki/en/Home.md) · [wiki/zh/Home.md](./wiki/zh/Home.md) | Wiki (getting started / FAQ / roadmap) |
| [../README.md](../README.md) · [../README.zh-CN.md](../README.zh-CN.md) | Project README |
| [ARCHITECTURE.md](./ARCHITECTURE.md) · [ARCHITECTURE.zh-CN.md](./ARCHITECTURE.zh-CN.md) | Contributors — crate boundaries |
| [COVERAGE.md](./COVERAGE.md) · [COVERAGE.zh-CN.md](./COVERAGE.zh-CN.md) | 100% branch coverage gate |
| [../CONTRIBUTING.md](../CONTRIBUTING.md) · [../CONTRIBUTING.zh-CN.md](../CONTRIBUTING.zh-CN.md) | How to change code |
| [../SECURITY.md](../SECURITY.md) · [../SECURITY.zh-CN.md](../SECURITY.zh-CN.md) | Security policy |
| [../SUPPORT.md](../SUPPORT.md) · [../SUPPORT.zh-CN.md](../SUPPORT.zh-CN.md) | Support channels |
| [../CODE_OF_CONDUCT.md](../CODE_OF_CONDUCT.md) · [../CODE_OF_CONDUCT.zh-CN.md](../CODE_OF_CONDUCT.zh-CN.md) | Community standards |
| [../CHANGELOG.md](../CHANGELOG.md) · [../CHANGELOG.zh-CN.md](../CHANGELOG.zh-CN.md) | Releases |

## Designs (specs)

| Spec | Topic |
|------|-------|
| [2026-07-17-rust-match-engines-design.md](./specs/2026-07-17-rust-match-engines-design.md) · [中文](./specs/2026-07-17-rust-match-engines-design.zh-CN.md) | Equivalence track + cutover |
| [2026-07-18-match-core-hp-design.md](./specs/2026-07-18-match-core-hp-design.md) · [中文](./specs/2026-07-18-match-core-hp-design.zh-CN.md) | HP dual-track |
| [2026-07-18-pe-optimizations-design.md](./specs/2026-07-18-pe-optimizations-design.md) · [中文](./specs/2026-07-18-pe-optimizations-design.zh-CN.md) | PE-inspired A→B→C |

## Plans

| Plan | Topic |
|------|-------|
| [2026-07-17-rust-match-engines.md](./plans/2026-07-17-rust-match-engines.md) · [中文](./plans/2026-07-17-rust-match-engines.zh-CN.md) | Equivalence tasks |
| [2026-07-18-match-core-hp.md](./plans/2026-07-18-match-core-hp.md) · [中文](./plans/2026-07-18-match-core-hp.zh-CN.md) | HP tasks |
| [2026-07-18-pe-optimizations.md](./plans/2026-07-18-pe-optimizations.md) · [中文](./plans/2026-07-18-pe-optimizations.zh-CN.md) | Cache / ART / wal |

## Operations & performance

| Doc | Topic |
|-----|-------|
| [best-practices.md](./best-practices.md) · [中文](./best-practices.zh-CN.md) | OSS → code map |
| [e2e-budget.md](./e2e-budget.md) · [中文](./e2e-budget.zh-CN.md) | L1–L5 latency budget |
| [fair-compare.md](./fair-compare.md) · [中文](./fair-compare.zh-CN.md) | Non-zero fill-rate protocol |
| [bench-results.md](./bench-results.md) · [中文](./bench-results.zh-CN.md) | Published numbers |
| [l3-shadow.md](./l3-shadow.md) · [中文](./l3-shadow.zh-CN.md) | Shadow validation |
| [cutover-runbook.md](./cutover-runbook.md) · [中文](./cutover-runbook.zh-CN.md) | Grey cutover |
| [rmq-spike.md](./rmq-spike.md) · [中文](./rmq-spike.zh-CN.md) | RocketMQ wiring status |
