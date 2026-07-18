# 变更日志

**English：** [CHANGELOG.md](CHANGELOG.md)

本项目的重要变更记录于此。

格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，并遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [Unreleased]

### 新增

- 双语 Wiki（`docs/wiki/en`、`docs/wiki/zh`）与 `README.zh-CN.md`
- `SECURITY.md`、`CODE_OF_CONDUCT.md`、`SUPPORT.md`、`CONTRIBUTING.zh-CN.md`
- GitHub Issue 模板（bug / feature）与联系链接
- Apache-2.0 许可（`LICENSE`、`NOTICE`）
- GitHub CI、`Makefile`、覆盖率门禁、自包含 `docs/`
- `match-core-hp`：最优价缓存、level pool、`LevelIndex`、可选 `art` feature
- `match-contract`：可选 `hp-engine` feature 与 L2/L3/L1 span 指标
- `match-wal`：异步批写 WAL + `wal_bench`
- `match-bench`：`fair_compare` 二进制（拒绝零成交率）
- 文档全面双语（`*.zh-CN.md` 与英文原文成对）

### 变更

- Workspace `license` 字段设为 `Apache-2.0`

## [0.1.0] - 2026-07-18

### 新增

- 初始可公开工作区布局：protocol、core、core-hp、contract 壳、replay、bench
- 双轨设计（等价 + HP）与受 PE 启发的优化轨
