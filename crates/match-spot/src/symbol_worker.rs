//! Per-symbol worker: recv → engine.on_order → outbound.

use std::sync::Arc;

use match_core::{BbOrder as CoreOrder, Engine};
use match_protocol::BbOrder;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{error, info};

use crate::outbound::Outbound;
use crate::telemetry;

static TEST_ENGINE_PANIC: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Test hook: next worker order panics inside the engine closure.
pub fn test_set_engine_panic(on: bool) {
    TEST_ENGINE_PANIC.store(on, std::sync::atomic::Ordering::SeqCst);
}

pub fn spawn_symbol_worker(
    symbol: String,
    mut rx: mpsc::Receiver<BbOrder>,
    outbound: Arc<Outbound>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut engine = Engine::new();
        info!(symbol = %symbol, "symbol worker started (match-core)");
        while let Some(order) = rx.recv().await {
            let order_no = order.trust_order_no.clone();
            let snapshot = order.clone();
            telemetry::record_order_event();
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if TEST_ENGINE_PANIC.load(std::sync::atomic::Ordering::SeqCst) {
                    std::panic::panic_any("test engine panic");
                }
                engine.on_order(CoreOrder(order))
            })) {
                Ok(events) => {
                    outbound.handle_order_result(&symbol, &snapshot, &events, &engine);
                }
                Err(_) => log_engine_panic(&symbol, &order_no),
            }
        }
        info!(symbol = %symbol, "symbol worker stopped");
    })
}

#[cfg_attr(coverage, coverage(off))]
fn log_engine_panic(symbol: &str, order_no: &str) {
    error!(symbol = %symbol, order_no = %order_no, "engine panic on order");
}
