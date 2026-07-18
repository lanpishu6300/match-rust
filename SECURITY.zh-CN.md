# 安全策略

**English：** [SECURITY.md](SECURITY.md)

## 支持的版本

| 版本 | 支持 |
|------|------|
| `main`（0.1.x） | 是 |
| 更旧 tag | 尽力而为 |

## 报告漏洞

请**不要**为安全漏洞开公开 GitHub Issue。

1. 邮件：**lanpishu6300@gmail.com**，主题 `[SECURITY] match-rust`
2. 或使用仓库的 GitHub **Security Advisories**：[lanpishu6300/match-rust](https://github.com/lanpishu6300/match-rust/security/advisories/new)（若可用）

请包含：

- 受影响 crate / 组件
- 复现步骤或 PoC（私下）
- 影响评估（鉴权绕过、DoS、数据泄漏等）

我们目标在 **72 小时内**确认，并给出修复方案或时间表。

## 范围说明

- 撮合引擎在生产路径处理不可信订单 JSON — 校验缺陷在范围内。
- 实验 crate（`match-core-hp`、`match-wal`）若可在进程壳启用，仍在范围内。
- 依赖 CVE：优先提交升版本 PR，并附简短风险说明。
