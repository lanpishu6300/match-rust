# RocketMQ client spike (Task 12)

**中文：** [rmq-spike.zh-CN.md](./rmq-spike.zh-CN.md)

**Date:** 2026-07-17  
**Target NameServer:** `192.168.0.241:9876` (from `config.example.yaml`)  
**Result:** **FAIL** — production RocketMQ adapter **not wired**

## What we tried

1. TCP connect to `192.168.0.241:9876` with a 2–3s timeout (Python `socket`, bash `/dev/tcp`, `nc -z`).
2. Outcome: **`No route to host` (os error 65)** / unreachable from this host. No send/receive within 30s is possible without NameServer reachability.
3. `rocketmq-client-v4` `0.4.2` was fetchable from crates.io but not wired — no live handshake without NS.
4. Candidate crates for Apache RocketMQ **4.x** brokers:
   - [`rocketmq-client-v4`](https://crates.io/crates/rocketmq-client-v4) `0.4.2` — protocol aimed at RMQ 4.x
   - [`rocketmq-client-rust`](https://crates.io/crates/rocketmq-client-rust) / mxsm stack — newer, MSRV 1.85+, primarily aligned with the Rust RocketMQ server lineage

## Decision (DONE_WITH_CONCERNS)

Until a reachable NameServer + confirmed crate↔broker handshake exists:

| Piece | Status |
|-------|--------|
| `OrderSink` / `MessageSource` traits | Implemented (`mq/traits.rs`) |
| Memory / file-channel adapter | Implemented (`mq/memory.rs`) |
| Topics, inbound, outbound, bootstrap, workers | Wired to Engine + Redis + RPC |
| Live RocketMQ consumer/producer | **Blocked** |

`config.rocketmq.transport` defaults to `memory`. Setting `rocketmq` currently logs and falls back to memory.

## Unblock checklist

1. Reach NameServer from the deploy/dev host (`nc -vz 192.168.0.241 9876`).
2. Pin a crate that completes producer send + consumer receive on a scratch topic within 30s.
3. Implement `mq/rocketmq.rs` behind `MqTransport::Rocketmq` and flip config.
4. Re-run smoke: restore count logs + place/cancel → `push_order` body on the encoded-symbol topic.
