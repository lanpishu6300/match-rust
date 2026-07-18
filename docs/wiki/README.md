# In-repo Wiki (bilingual)

Versioned wiki pages (GitHub Wiki alternative that stays in git).

| Language | Home |
|----------|------|
| English | [en/Home.md](./en/Home.md) |
| 中文 | [zh/Home.md](./zh/Home.md) |

### Publishing to GitHub Wiki (optional)

Maintainers can sync these files to the GitHub Wiki tab:

```bash
# example: clone wiki and copy
git clone https://github.com/lanpishu6300/match-rust.wiki.git
cp docs/wiki/en/*.md match-rust.wiki/
# rename Home.md → Home.md etc., commit & push
```

Prefer keeping **this directory** as the source of truth so PRs can review doc changes with code.
