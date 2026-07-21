# Agent / editor guidance for match-rust

This file is for anyone (human or tool) editing the repo. Prefer it over generic assistant defaults.

## Voice

- No AI or Cursor product traces in committed code, comments, docs, commit messages, or PR text.
- No “agentic worker” / skill-routing banners in plans.
- Comments explain *why*; they do not narrate the change history (“fixes CR #3”).

## Architecture guardrails

- Production default engine: `match-core` (Java-observable). `match-core-hp` only via `hp-engine`.
- Do not silently degrade `transport: rocketmq` to memory; exit until a real adapter exists.
- Fair benches must keep `fill_rate > 0`.

## Verify

```bash
cargo fmt --all
cargo clippy --workspace --all-targets
cargo test --workspace
# if touching hp indexes:
cargo test -p match-core-hp --features art
```

See [CONTRIBUTING.md](CONTRIBUTING.md) and [.cursor/rules/human-prose.mdc](.cursor/rules/human-prose.mdc).
