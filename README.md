# match-rust

Rust port of the  contract matching engine (`java-contract-match`), structured as a Cargo workspace with shared `match-core`, `match-protocol`, and `match-replay` crates. See the design spec at [`docs/superpowers/specs/2026-07-17-rust-match-engines-design.md`](../docs/superpowers/specs/2026-07-17-rust-match-engines-design.md) for architecture, milestones, and acceptance criteria. Run `cargo test --workspace` to verify the workspace builds and tests pass.
