# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Apache-2.0 licensing (`LICENSE`, `NOTICE`)
- GitHub CI workflow, `Makefile`, `CONTRIBUTING.md`, self-contained `docs/`
- `match-core-hp`: best-price cache, level pool, `LevelIndex`, optional `art` feature
- `match-contract`: optional `hp-engine` feature with L2/L3/L1 span metrics
- `match-wal`: async batched WAL + `wal_bench`
- `match-bench`: `fair_compare` binary (rejects zero fill-rate)

### Changed

- Workspace `license` field set to `Apache-2.0`

## [0.1.0] - 2026-07-18

### Added

- Initial public-ready workspace layout: protocol, core, core-hp, contract shell, replay, bench
- Dual-track design (equivalence + HP) and PE-inspired optimization track
