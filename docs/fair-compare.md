# Fair compare protocol (match-core-hp vs match-core vs perpetual_exchange)

**中文：** [fair-compare.zh-CN.md](./fair-compare.zh-CN.md)

## Rules (must hold)

1. **Same command sequence:** rest then cross; price-time priority reproducible (fixed tick/qty).
2. **Fill rate > 0:** report `fill_rate = fills / orders`; zero fill → **INVALID** (`crypto-exchange` ART+SIMD reports have shown 0% fills — do not rank against valid runs).
3. **Kernel only:** disable rate limits, disk logging, HTTP, account liquidation.
4. **Same machine / same optimization:** `-O3` / `cargo --release`.
5. **Unified metrics** (CSV columns below).

## Run (this repo)

```bash
export PATH="$HOME/.cargo/bin:$PATH"
# from repo root
cargo run -p match-bench --release --bin fair_compare -- --n 50000
# ART index path (same fill_rate expected):
cargo test -p match-core-hp --features art --test art_parity
```

Output: stdout CSV + assert `fill_rate > 0` (else exit 1).

## CSV columns

```text
engine,scenario,n_orders,n_fills,fill_rate,elapsed_ns,orders_per_sec,fills_per_sec,ns_per_order
```

- `engine`: `match-core` | `match-core-hp`
- `scenario`: `fair_cross` (50% sell rest + 50% buy cross; expect fill_rate ≈ 0.5)

## Comparing perpetual_exchange (C++)

1. In `crypto-exchange`, build the **same semantics**: N/2 sell @100, N/2 buy @100, qty=1; confirm fills ≈ N/2.
2. Run matching_engine / orderbook microbench only — not the production rate-limited path.
3. Put throughput/latency into the same CSV template with `engine=perpetual_exchange_<variant>`.
4. If a variant has `fill_rate==0`, mark INVALID; do not rank it.

## How to read results

| Compare | Meaning |
|---------|---------|
| hp vs core | Fixed-point level book vs BigDecimal/BTreeSet (automated here) |
| hp vs C++ Original | Similar “real fill” microkernel |
| hp vs C++ ART+SIMD (fill_rate=0) | **Forbidden** as “faster” |

End-to-end budget: [`e2e-budget.md`](./e2e-budget.md).
