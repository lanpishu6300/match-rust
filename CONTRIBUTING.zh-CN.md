# 贡献指南

感谢参与 **match-rust**。  
**English：** [CONTRIBUTING.md](CONTRIBUTING.md)

## 开发环境

```bash
# 工具链见 rust-toolchain.toml
cargo test --workspace
make ci
```

## 分支

- 默认集成分支：`main`
- 每个 PR 尽量只做一件事；等价轨与性能轨变更尽量分开

## 提交 PR 前

1. `cargo fmt --all`
2. `cargo clippy --workspace --all-targets`
3. `cargo test --workspace`
4. 若改动 `match-core-hp` 索引：`cargo test -p match-core-hp --features art`
5. 若改动撮合热路径：`make fair`（必须 exit 0，且 `fill_rate > 0`）
6. 触及门禁 crate 时：`make cov`（需 nightly）

## 设计文档

行为或架构变更应更新 `docs/specs/`（或新增带日期的设计）。仓库需**自包含**，不要依赖仓外路径。

双语 Wiki：`docs/wiki/zh/` · `docs/wiki/en/`。

## 双轨护栏

- **不要**让 `match-contract` 默认走 `match-core-hp`
- **不要**用 `fill_rate == 0` 的负载宣称性能胜利
- 变更 `match-core` 的 Java quirk 时必须在 PR / 文档中写明

## 提交信息

推荐 Conventional Commits：

- `feat(match-core-hp): …`
- `fix(match-contract): …`
- `docs: …`
- `chore: …`

## 文风

注释、文档与 PR 描述用正常维护者口吻。不要把编辑器/Agent 产品名、“AI 生成”声明或技能路由横幅写进仓库。详见 [AGENTS.md](AGENTS.md)。

## 许可

贡献即表示同意以 **Apache License 2.0** 授权（见 `LICENSE`）。
