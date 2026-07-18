# FAQ

**中文：** [zh/FAQ.md](../zh/FAQ.md)

### Is this a drop-in replacement for Java `java-contract-match` today?

Not yet for production MQ. The shell supports restore/health/metrics and **memory** transport; RocketMQ wiring is tracked in [rmq-spike](../../rmq-spike.md). Equivalence is validated via golden replay and shadow plans.

### Why two engines (`match-core` vs `match-core-hp`)?

**Equivalence** preserves Java-observable behavior (including known quirks). **HP** optimizes for latency with fixed-point semantics and must not become the silent production default.

### Why is my “faster” number rejected?

`fair_compare` and the performance policy require **non-zero fill rate**. Zero-fill peaks (seen in some ART+SIMD demos) are INVALID for ranking.

### Can I enable ART by default?

No. ART is behind `--features art` and must pass parity tests against the BTree index.

### How do I report a security issue?

See [SECURITY.md](../../../SECURITY.md). Do not file public issues for vulnerabilities.

### Where is the Chinese documentation?

- [README.zh-CN.md](../../../README.zh-CN.md)
- [Wiki 中文](../zh/Home.md)
- [CONTRIBUTING.zh-CN.md](../../../CONTRIBUTING.zh-CN.md)
