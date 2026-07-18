# Contributing

Thanks for helping improve **match-rust**.

## Development setup

```bash
# Toolchain pinned in rust-toolchain.toml
cargo test --workspace
make ci
```

## Branching

- Default integration branch: `main` (or current `feature/*` until first public cut)
- One logical change per PR; keep equivalence and HP tracks separate when possible

## Before you open a PR

1. `cargo fmt --all`
2. `cargo clippy --workspace --all-targets`
3. `cargo test --workspace`
4. If you touch `match-core-hp` indexes: `cargo test -p match-core-hp --features art`
5. If you touch matching hot paths: `make fair` (must exit 0, fill_rate > 0)

## Design docs

Behavioral or architectural changes should update the relevant file under `docs/specs/` (or add a dated design). Keep the repo **self-contained** — do not rely on paths outside this Git repository.

## Dual-track guardrails

- Do **not** make `match-contract` default to `match-core-hp`
- Do **not** claim performance wins from workloads with `fill_rate == 0`
- Preserve Java-observable behavior in `match-core` unless the PR explicitly changes a quirk (document it)

## Commit messages

Prefer Conventional Commits style:

- `feat(match-core-hp): …`
- `fix(match-contract): …`
- `docs: …`
- `chore: …`

## License

By contributing, you agree that your contributions are licensed under the **Apache License 2.0** (see `LICENSE`).
