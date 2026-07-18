# Architecture

**中文：** [zh/Architecture.md](../zh/Architecture.md)

Full detail: [docs/ARCHITECTURE.md](../../ARCHITECTURE.md)

## Dual-track rule

1. **Production default** for `match-contract` is always `match-core` (Java-observable equivalence).
2. **`match-core-hp`** is experimental (fixed-point, cleaner semantics). Enable only via `--features hp-engine`.
3. Do not mix Java-quirk tests into HP, or HP allocations into the equivalence hot path.

## Crate map

```text
match-protocol   → shared DTOs / validation
match-core       → equivalence engine (default)
match-core-hp    → HP engine (tick/lot, LevelIndex, optional art)
match-contract   → process shell (config, restore, Redis, workers, health)
match-spot       → spot shell stub
match-replay     → golden NDJSON replay
match-bench      → criterion + fair_compare
match-wal        → async batched WAL (experimental)
```

## Inbound path (contract)

```text
MQ/JSON (or memory) → validate/convert → per-symbol worker
  → match-core::Engine (default)
  → outbound push / depth / metrics
```

With `hp-engine`: adapter → `HpEngine` / `HpWorker` + L2/L3/L1 spans on `/metrics`.

## Related designs

- [Equivalence design](../../specs/2026-07-17-rust-match-engines-design.md)
- [HP design](../../specs/2026-07-18-match-core-hp-design.md)
- [PE optimizations](../../specs/2026-07-18-pe-optimizations-design.md)
