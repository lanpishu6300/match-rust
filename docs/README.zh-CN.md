# 文档索引

**English：** [README.md](./README.md)

**match-rust** GitHub 仓库的自包含文档（除明确标注的配套仓库外，不链出本树）。

约定：英文 `Foo.md` + 中文 `Foo.zh-CN.md`，文首互链语言切换。

## 从这里开始

| 文档 | 读者 |
|------|------|
| [wiki/zh/Home.md](./wiki/zh/Home.md) · [wiki/en/Home.md](./wiki/en/Home.md) | Wiki（入门 / FAQ / 路线图） |
| [../README.zh-CN.md](../README.zh-CN.md) · [../README.md](../README.md) | 项目 README |
| [ARCHITECTURE.zh-CN.md](./ARCHITECTURE.zh-CN.md) · [ARCHITECTURE.md](./ARCHITECTURE.md) | 贡献者 — crate 边界 |
| [COVERAGE.zh-CN.md](./COVERAGE.zh-CN.md) · [COVERAGE.md](./COVERAGE.md) | 100% 分支覆盖率门禁 |
| [../CONTRIBUTING.zh-CN.md](../CONTRIBUTING.zh-CN.md) · [../CONTRIBUTING.md](../CONTRIBUTING.md) | 如何改代码 |
| [../SECURITY.zh-CN.md](../SECURITY.zh-CN.md) · [../SECURITY.md](../SECURITY.md) | 安全策略 |
| [../SUPPORT.zh-CN.md](../SUPPORT.zh-CN.md) · [../SUPPORT.md](../SUPPORT.md) | 支持渠道 |
| [../CODE_OF_CONDUCT.zh-CN.md](../CODE_OF_CONDUCT.zh-CN.md) · [../CODE_OF_CONDUCT.md](../CODE_OF_CONDUCT.md) | 社区准则 |
| [../CHANGELOG.zh-CN.md](../CHANGELOG.zh-CN.md) · [../CHANGELOG.md](../CHANGELOG.md) | 版本记录 |

## 设计规格（specs）

| 规格 | 主题 |
|------|------|
| [中文](./specs/2026-07-17-rust-match-engines-design.zh-CN.md) · [EN](./specs/2026-07-17-rust-match-engines-design.md) | 等价轨 + 切流 |
| [中文](./specs/2026-07-18-match-core-hp-design.zh-CN.md) · [EN](./specs/2026-07-18-match-core-hp-design.md) | HP 双轨 |
| [中文](./specs/2026-07-18-pe-optimizations-design.zh-CN.md) · [EN](./specs/2026-07-18-pe-optimizations-design.md) | PE 启发的 A→B→C |

## 实现计划（plans）

| 计划 | 主题 |
|------|------|
| [中文](./plans/2026-07-17-rust-match-engines.zh-CN.md) · [EN](./plans/2026-07-17-rust-match-engines.md) | 等价任务 |
| [中文](./plans/2026-07-18-match-core-hp.zh-CN.md) · [EN](./plans/2026-07-18-match-core-hp.md) | HP 任务 |
| [中文](./plans/2026-07-18-pe-optimizations.zh-CN.md) · [EN](./plans/2026-07-18-pe-optimizations.md) | 缓存 / ART / wal |

## 运维与性能

| 文档 | 主题 |
|------|------|
| [中文](./best-practices.zh-CN.md) · [EN](./best-practices.md) | 开源实践 → 代码映射 |
| [中文](./e2e-budget.zh-CN.md) · [EN](./e2e-budget.md) | L1–L5 延迟预算 |
| [中文](./fair-compare.zh-CN.md) · [EN](./fair-compare.md) | 非零成交率对打协议 |
| [中文](./bench-results.zh-CN.md) · [EN](./bench-results.md) | 已发布数字 |
| [中文](./l3-shadow.zh-CN.md) · [EN](./l3-shadow.md) | 影子验证 |
| [中文](./cutover-runbook.zh-CN.md) · [EN](./cutover-runbook.md) | 灰度切流 |
| [中文](./rmq-spike.zh-CN.md) · [EN](./rmq-spike.md) | RocketMQ 接通状态 |
