# L3 shadow validation for match-contract

L3 is the pre-production equivalence gate: run Rust against **real or recorded inbound traffic** without affecting downstream consumers. See design spec [§4.2 L3](../../../docs/superpowers/specs/2026-07-17-rust-match-engines-design.md#42-match-replay-三层).

L2 (`cargo test -p match-replay`) must be green before any L3 work.

---

## Mode A — Offline replay (preferred for CI / repeatability)

1. **Record** inbound JSON arrays from `usdt_contract_match_order_{symbol}` for a low-traffic symbol over a representative window (e.g. 1–4 hours).
   - Store raw message bodies as NDJSON or one JSON file per batch (same format Java consumer receives).
2. **Java golden export:** run the recorded sequence through Java reference (or existing golden exporter in `java-contract-match` JUnit) → `GoldenTrace` NDJSON (fills, book snapshots, depth).
3. **Rust replay:** feed the same `MqOrder` sequence into `match-replay` / local `MemoryMessageSource`:
   ```bash
   export MATCH_CONTRACT_CONFIG=crates/match-contract/config.example.yaml
   export MATCH_CONTRACT_LOCAL_SYMBOLS=btcusdt
   # preload recorded batches under {memory_dir}/in/ when using file-channel transport
   cargo run -p match-contract
   ```
4. **Diff:** compare Rust engine events / outbound payloads against golden; fail on fill price/qty, remaining, depth level mismatch.

**Properties:** no production side effects; fully repeatable; no RocketMQ group coordination needed.

---

## Mode B — Live shadow consume (read-only)

Rust runs the full shell (parse → validate → per-symbol worker → engine) but **must not produce** to downstream topics.

### Configuration (documented intent; wire when RMQ adapter lands)

```yaml
rocketmq:
  consumer_group: "usdt_contract_match_channel_rust_shadow_group"  # NOT the production group
  shadow_consume: true
  producers_enabled: false
```

| Setting | Production cutover | L3 shadow |
|---------|-------------------|-----------|
| Consumer group | `usdt_contract_match_channel_one_group` | `usdt_contract_match_channel_rust_shadow_group` |
| Producers | enabled | **disabled** |
| Competes with Java | yes (must be single active) | **no** — different group, duplicate read |

### Procedure

1. Deploy Rust with shadow group on **low-traffic symbol(s)** only.
2. Java remains the **sole producer** to outbound topics (production path unchanged).
3. Rust consumes the same inbound topic in parallel (duplicate delivery is expected across groups).
4. Periodically compare:
   - In-memory book depth (Rust engine) vs Java snapshot (Redis depth keys or admin API if available)
   - `/metrics` counters: `match.order.events.total`, `match.trades.deals.total`, `match.orders.inbound.invalid.total`
5. Log diffs at WARN; do **not** write to `push_order`, `push_market`, `no_deal`, `deeps`, or `robot` topics.

### What shadow proves

- Inbound validation parity with Java
- Engine state divergence detection under live traffic
- Bootstrap + restore correctness when combined with a recorded entrust snapshot

### What shadow does not prove

- Outbound serialization byte-for-byte match (disabled by design)
- MQ send failure / error-queue behavior (no producers)
- Consumer offset coordination with production group

---

## Mode C — Record-then-shadow offline

Hybrid for symbols where live shadow diff tooling is immature:

1. Record inbound during shadow window (Mode B without diff tooling).
2. Take entrust snapshot at recording start boundary.
3. Offline replay both Java golden and Rust from recording + snapshot.
4. Promote symbol to grey cutover only after offline diff passes.

---

## Exit criteria (symbol ready for cutover)

- [ ] L2 replay green for symbol's recorded corpus
- [ ] L3 shadow/offline: zero fill/depth mismatches over agreed window
- [ ] `match.orders.inbound.invalid.total` rate matches Java ± agreed tolerance
- [ ] Ops sign-off on [`cutover-runbook.md`](cutover-runbook.md) checklist

---

## Related docs

- [Grey cutover runbook](cutover-runbook.md)
- [Design spec §4.2–4.3](../../../docs/superpowers/specs/2026-07-17-rust-match-engines-design.md)
- [Implementation plan Task 14](../../../docs/superpowers/plans/2026-07-17-rust-match-engines.md)
