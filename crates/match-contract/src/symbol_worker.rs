//! Per-symbol single-threaded worker: `recv → engine.on_order → outbound`.

use std::sync::Arc;

use match_protocol::BbOrder;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::info;

use crate::outbound::Outbound;
use crate::telemetry;

#[cfg(not(feature = "hp-engine"))]
use match_core::{BbOrder as CoreOrder, Engine};

#[cfg(feature = "hp-engine")]
use std::time::Instant;

#[cfg(feature = "hp-engine")]
use match_core_hp::{adapter::from_bb_order, HpEngine, HpEvent, SymbolScale};
#[cfg(feature = "hp-engine")]
use tracing::warn;

/// Spawn a worker that owns a matching engine for `symbol`.
pub fn spawn_symbol_worker(
    symbol: String,
    mut rx: mpsc::Receiver<BbOrder>,
    outbound: Arc<Outbound>,
    price_scale: u32,
    qty_scale: u32,
) -> JoinHandle<()> {
    #[cfg(not(feature = "hp-engine"))]
    {
        let _ = (price_scale, qty_scale);
        tokio::spawn(async move {
            let mut engine = Engine::new();
            info!(symbol = %symbol, "symbol worker started (match-core)");
            while let Some(order) = rx.recv().await {
                let order_no = order.trust_order_no.clone();
                telemetry::record_order_event();
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    engine.on_order(CoreOrder(order))
                })) {
                    Ok(events) => {
                        outbound.handle_events(&symbol, &events, &engine);
                    }
                    Err(_) => {
                        tracing::error!(symbol = %symbol, order_no = %order_no, "engine panic on order");
                    }
                }
            }
            info!(symbol = %symbol, "symbol worker stopped");
        })
    }

    #[cfg(feature = "hp-engine")]
    {
        tokio::spawn(async move {
            let mut engine = HpEngine::with_capacity(4096, 64);
            let scale = SymbolScale {
                price_scale,
                qty_scale,
            };
            info!(
                symbol = %symbol,
                price_scale,
                qty_scale,
                "symbol worker started (hp-engine)"
            );
            while let Some(order) = rx.recv().await {
                let t_recv = Instant::now();
                let order_no = order.trust_order_no.clone();
                telemetry::record_order_event();

                let t_adapt0 = Instant::now();
                let cmd = match from_bb_order(&order, &scale) {
                    Ok(c) => c,
                    Err(e) => {
                        warn!(symbol = %symbol, order_no = %order_no, error = %e, "L3_adapt failed");
                        telemetry::record_inbound_invalid();
                        continue;
                    }
                };
                let adapt_ns = t_adapt0.elapsed().as_nanos() as u64;
                // L2: time spent waiting in the channel is not stamped at send; approximate as
                // recv→adapt start (near-zero when worker is idle).
                let queue_ns = t_recv.elapsed().as_nanos() as u64;

                let t_l1 = Instant::now();
                let events: Vec<HpEvent> = engine.on_order(cmd).to_vec();
                let l1_ns = t_l1.elapsed().as_nanos() as u64;
                telemetry::record_span_ns(queue_ns, adapt_ns, l1_ns);

                outbound.handle_hp_events(&symbol, &events, &engine, &scale);
            }
            info!(symbol = %symbol, "symbol worker stopped");
        })
    }
}
