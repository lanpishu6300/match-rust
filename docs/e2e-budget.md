# End-to-end latency budget and bottleneck breakdown

**中文：** [e2e-budget.zh-CN.md](./e2e-budget.zh-CN.md)

Goal: separate “matching kernel is fast” from “user-perceived latency” so we do not over-optimize `on_order` when it is no longer the bottleneck.

## 1. Layered model

```text
┌─────────────────────────────────────────────────────────────┐
│ L5  Client / API / gateway                                  │
├─────────────────────────────────────────────────────────────┤
│ L4  RocketMQ inbound (network + broker + JSON deserialize)  │  ← production main path
├─────────────────────────────────────────────────────────────┤
│ L3  Gateway: validate / BbOrder→HpCommand / enqueue         │  ← optional split
├─────────────────────────────────────────────────────────────┤
│ L2  In-process queue (tokio mpsc or SpscRing)               │
├─────────────────────────────────────────────────────────────┤
│ L1  Match kernel on_order (match-core or match-core-hp)     │  ← microbenches exist
├─────────────────────────────────────────────────────────────┤
│ L2' Outbound queue + serialize                              │
├─────────────────────────────────────────────────────────────┤
│ L4' RocketMQ outbound (push_order / depth)                  │
└─────────────────────────────────────────────────────────────┘
```

| Layer | Typical order of magnitude (empirical; measure locally) | Status here |
|-------|----------------------------------------------------------|-------------|
| L1 hp microkernel | tens–hundreds ns/order (synthetic) | ✅ `match-bench` / `fair_compare` |
| L1 core (BigDecimal) | µs | ✅ vs hp |
| L2 in-process queue | hundreds ns–few µs | ✅ SPSC; contract still tokio |
| L3 validate+adapt | µs | adapter + `match.span.l3_adapt_ns_total` (`hp-engine`) |
| L4 MQ+JSON | often **hundreds µs–ms** | ⚠️ RMQ not wired (`rmq-spike.md`) |

**Implication:** Before L4 is optimized, shaving L1 from 20ns to 10ns barely moves e2e. Next cut should **measure L4/L3** or **split the gateway**.

## 2. Example budget table (fill after measurement)

Scenario: single symbol, aggressive limit, local/test. Units: µs, p99.

| Segment | Budget (example) | Measured | Share | Action |
|---------|------------------|----------|-------|--------|
| L4 inbound | 200 | TBD | | batch, skip JSON, or bypass |
| L3 adapt | 5 | TBD | | fixed-point at boundary |
| L2 enqueue | 2 | TBD | | SPSC + pin cores |
| L1 match | 1 | TBD | | hp |
| L2' out buffer | 2 | TBD | | batch drain |
| L4' outbound | 200 | TBD | | depth throttle exists; coalesce fills |
| **Total** | **~410** | | | |

How: stamp `Instant` (or tracing spans) at each boundary with a shared order id.

## 3. Decomposition checklist

1. [ ] Pure L1: `cargo run -p match-bench --bin fair_compare -- --n 50000`
2. [ ] L1+L2: same load via `HpWorker` (`hp_cross_full_spsc` criterion)
3. [ ] L3+L1: JSON/`BbOrder` → adapter → hp (spans)
4. [ ] Full L4: after RMQ lands, record p99 on a shadow symbol

## 4. Relation to fair compare

- **This doc:** answers “which layer is slow?”
- **`docs/fair-compare.md` + `fair_compare` binary:** answers “under the same kernel load, is hp vs core (and external C++) fairly comparable?”

Do 1→2 before chasing Aeron/SIMD.
