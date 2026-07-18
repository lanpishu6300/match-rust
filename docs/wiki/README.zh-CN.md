# 仓库内 Wiki（双语）

**English：** [README.md](./README.md)

随代码版本化的 Wiki 页面（许多项目 GitHub Wiki 的替代，且可随 PR 审阅）。

| 语言 | 首页 |
|------|------|
| English | [en/Home.md](./en/Home.md) |
| 中文 | [zh/Home.md](./zh/Home.md) |

### 发布到 GitHub Wiki（可选）

维护者可将这些文件同步到 GitHub Wiki 标签页：

```bash
git clone https://github.com/lanpishu6300/match-rust.wiki.git
# 复制扁平英文页 + Zh-* 中文页 + _Sidebar.md，再 commit & push
```

优先以**本目录**为事实来源，便于文档与代码同 PR 审阅。

其余项目文档（规格、手册、社区文件）使用仓库根与 `docs/` 下的 `Foo.md` / `Foo.zh-CN.md` 成对约定 — 见 [../README.zh-CN.md](../README.zh-CN.md)。
