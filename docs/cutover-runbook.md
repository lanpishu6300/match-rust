# match-contract symbol grey cutover runbook

Per-symbol cutover from Java `java-contract-match` to Rust `match-contract`. Aligns with design spec [§4.3](../../../docs/superpowers/specs/2026-07-17-rust-match-engines-design.md#43-灰度切换).

**Hard constraint:** For a given symbol, only **one active consumer** may read `usdt_contract_match_order_{symbol}` on group `usdt_contract_match_channel_one_group` at any time. Dual consumption splits the order book.

---

## Preconditions

- [ ] L2 golden replay green: `cargo test -p match-replay`
- [ ] L3 shadow or offline replay completed for target symbol(s) — see [`l3-shadow.md`](l3-shadow.md)
- [ ] Test-env full shell smoke: restore RPC, Redis wipe/link, inbound/outbound, error queue
- [ ] Rust image/config deployed with `match.symbols_whitelist` set to cutover symbol(s) only
- [ ] Dashboards wired to Rust `/metrics` (OTel-aligned names) and Java baselines for comparison
- [ ] On-call briefed on rollback steps below

---

## Phase 1 — Warmup (no production traffic)

1. Deploy Rust `match-contract` **without** subscribing to production inbound topics (or with empty whitelist / no consumers).
2. Confirm `/healthz` returns `200` and `/readyz` returns `503` until bootstrap finishes, then `200`.
3. Run L2 + L3 validation for target symbols.
4. Verify RPC restore paths against test env:
   - `GET {market_base_url}/contract-market/contractcoinMarketList`
   - `GET {order_base_url}/contract/entrust-list`

---

## Phase 2 — Per-symbol cutover

Repeat for each symbol `S` (lowercase key, e.g. `btcusdt`):

### 2.1 Stop Java consumption for S

- [ ] Stop or reconfigure Java so **no instance** in `usdt_contract_match_channel_one_group` consumes `usdt_contract_match_order_{S}`.
- [ ] If Java runs all symbols in one process, options:
  - Temporarily remove `S` from Java's active symbol set (if supported), **or**
  - Stop Java entirely when cutting the last symbol in that instance.
- [ ] Wait for in-flight messages on `S` to drain (or accept a brief dual-stop window with no consumers).

### 2.2 Verify single consumer

- [ ] RocketMQ console / ops check: **zero** Java consumers on group `usdt_contract_match_channel_one_group` for topic `usdt_contract_match_order_{S}`.
- [ ] No other shadow/staging consumer accidentally using the production group.

### 2.3 Start Rust for S

- [ ] Set `match.symbols_whitelist: ["S"]` (or add `S` to existing whitelist).
- [ ] Start Rust process; confirm bootstrap sequence:
  1. `startup_delay_ms` elapsed
  2. Markets fetched; shard filter applied
  3. Redis depth keys + link key reset for `S`
  4. Entrust restore via RPC → START_QUEUE / BigNo populated
  5. Consumer subscribed; `/readyz` → `200`
- [ ] Producers enabled (default): push_order, push_market, no_deal, deeps, robot.

### 2.4 Post-cutover observation (≥ 30 min)

Watch for regressions vs Java baseline:

| Signal | Action if bad |
|--------|----------------|
| `match.orders.inbound.invalid.total` spike | Check payload/schema drift; consider rollback |
| Redis `poc_redis_send_mq_error_data_queue` growth | MQ send failures; rollback if sustained |
| Depth topics stale / empty | Check worker logs, throttle config |
| Order push lag vs contract-order | RPC/MQ latency; compare trust order state |
| `match.order_book.remove_failed.total` (when wired) | Book inconsistency; rollback |

- [ ] Spot-check: place limit, partial fill, cancel, market order on `S`.
- [ ] Confirm downstream contract-order / market services consume Rust outbound normally.

### 2.5 Expand

- [ ] Add next symbol to whitelist or deploy dedicated Rust instance per symbol group.
- [ ] Keep Java package/instance available for rollback until all symbols stable ≥ 24h.

---

## Rollback

**Triggers:** sustained outbound error queue growth, depth blank > N minutes, order reconciliation mismatch, book remove failures significantly above baseline.

1. **Stop Rust** for symbol `S` (remove from whitelist or kill process).
2. **Ensure queue idle** — brief pause with no consumers is acceptable; avoid dual active consumers.
3. **Start Java** for `S` with the same restore path (`InitLoadData` equivalent):
   - Redis depth wipe + link key on startup
   - Paginated entrust restore
   - START_QUEUE / BigNo / 720s TTL active
4. **Subscribe Java** to `usdt_contract_match_order_{S}` on `usdt_contract_match_channel_one_group`.
5. Verify Java `/health` (port `31015`) and production metrics return to baseline.
6. File incident note; preserve Rust logs and MQ offsets for postmortem.

---

## Configuration reference

| Key | Purpose |
|-----|---------|
| `match.symbols_whitelist` | Limit active symbols during grey cut |
| `shard` | Must match Java `SHARD` for market filter |
| `start_queue_ttl_ms` | Default `720000` — keep during cut window |
| `health.port` | Default `31015` (mirrors Java `server.port`) |
| `rocketmq.consumer_group` | Must remain `usdt_contract_match_channel_one_group` for production cutover |

---

## Related docs

- [Rust match engines design spec](../../../docs/superpowers/specs/2026-07-17-rust-match-engines-design.md)
- [Implementation plan §4.3](../../../docs/superpowers/plans/2026-07-17-rust-match-engines.md)
- [L3 shadow modes](l3-shadow.md)
- [Java OTel metrics](../../java-contract-match/docs/opentelemetry-metrics.md)
