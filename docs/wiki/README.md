# In-repo Wiki (bilingual)

**中文：** [README.zh-CN.md](./README.zh-CN.md)

Versioned wiki pages (GitHub Wiki alternative that stays in git).

| Language | Home |
|----------|------|
| English | [en/Home.md](./en/Home.md) |
| 中文 | [zh/Home.md](./zh/Home.md) |

### Publishing to GitHub Wiki (optional)

Maintainers can sync these files to the GitHub Wiki tab:

```bash
git clone https://github.com/lanpishu6300/match-rust.wiki.git
# copy flat EN pages + Zh-* ZH pages + _Sidebar.md, then commit & push
```

Prefer keeping **this directory** as the source of truth so PRs can review doc changes with code.

Other project docs (specs, runbooks, community files) use the `Foo.md` / `Foo.zh-CN.md` pair under `docs/` and the repo root — see [../README.md](../README.md).
