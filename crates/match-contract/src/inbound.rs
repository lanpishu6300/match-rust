//! Inbound path: validate → convert → START_QUEUE / BigNo dedupe → enqueue.
//!
//! Ports Java `BaseConsumer.handleMqData` and restore-side bookkeeping from `InitLoadData`.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use match_protocol::{check_mq_order, type_convert, BbOrder, MqOrder, ORDER_STATUS_REVOKE};
use thiserror::Error;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::telemetry;

/// Startup dedupe state (`START_QUEUE_MAP` + `GlobalVariables.BigNo`).
#[derive(Debug, Default)]
pub struct StartQueueState {
    /// Per-symbol trust order nos restored at startup.
    restored: Mutex<HashMap<String, HashSet<String>>>,
    /// Max restored trust order no (numeric).
    big_no: AtomicI64,
    /// When false, dedupe is inactive (cleared after TTL).
    active: Mutex<bool>,
}

impl StartQueueState {
    pub fn new() -> Self {
        Self {
            restored: Mutex::new(HashMap::new()),
            big_no: AtomicI64::new(0),
            active: Mutex::new(true),
        }
    }

    pub fn big_no(&self) -> i64 {
        self.big_no.load(Ordering::SeqCst)
    }

    pub fn is_active(&self) -> bool {
        *self.active.lock().expect("start queue active lock")
    }

    /// Record a restored order no and bump BigNo (InitLoadData restore loop).
    pub fn record_restored(&self, symbol_key: &str, trust_order_no: &str) {
        {
            let mut map = self.restored.lock().expect("start queue lock");
            map.entry(symbol_key.to_string())
                .or_default()
                .insert(trust_order_no.to_string());
        }
        if let Ok(n) = trust_order_no.parse::<i64>() {
            let mut cur = self.big_no.load(Ordering::SeqCst);
            while n > cur {
                match self
                    .big_no
                    .compare_exchange(cur, n, Ordering::SeqCst, Ordering::SeqCst)
                {
                    Ok(_) => break,
                    Err(actual) => cur = actual,
                }
            }
        }
    }

    /// Clear START_QUEUE_MAP and BigNo after TTL.
    pub fn clear(&self) {
        self.restored.lock().expect("start queue lock").clear();
        self.big_no.store(0, Ordering::SeqCst);
        *self.active.lock().expect("start queue active lock") = false;
    }

    /// Returns true if the order should be dropped as a startup duplicate.
    ///
    /// Java skips dedupe for revoke (`ORDER_STATUS_REVOKE`) and when START_QUEUE_MAP is empty.
    pub fn should_dedupe(&self, order: &BbOrder) -> bool {
        if !self.is_active() {
            return false;
        }
        if order.order_status == ORDER_STATUS_REVOKE {
            return false;
        }

        let map = self.restored.lock().expect("start queue lock");
        if map.is_empty() {
            return false;
        }

        if let Some(set) = map.get(&order.symbol_key) {
            if set.contains(&order.trust_order_no) {
                debug!(
                    symbol = %order.symbol_key,
                    order_no = %order.trust_order_no,
                    "startup dedupe: restored order no"
                );
                return true;
            }
        }

        let big_no = self.big_no.load(Ordering::SeqCst);
        if big_no > 0 {
            if let Ok(n) = order.trust_order_no.parse::<i64>() {
                if big_no >= n {
                    debug!(
                        order_no = %order.trust_order_no,
                        big_no,
                        "startup dedupe: BigNo"
                    );
                    return true;
                }
            }
        }
        false
    }
}

#[derive(Debug, Error)]
pub enum InboundError {
    #[error("invalid json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("symbol queue missing: {0}")]
    MissingQueue(String),
    #[error("enqueue failed: {0}")]
    Enqueue(String),
}

/// Routes inbound MQ bodies onto per-symbol channels.
pub struct InboundRouter {
    queues: Mutex<HashMap<String, mpsc::Sender<BbOrder>>>,
    start_queue: Arc<StartQueueState>,
}

impl InboundRouter {
    pub fn new(start_queue: Arc<StartQueueState>) -> Self {
        Self {
            queues: Mutex::new(HashMap::new()),
            start_queue,
        }
    }

    pub fn start_queue(&self) -> &Arc<StartQueueState> {
        &self.start_queue
    }

    pub fn register_queue(&self, symbol_key: &str, tx: mpsc::Sender<BbOrder>) {
        self.queues
            .lock()
            .expect("queues lock")
            .insert(symbol_key.to_string(), tx);
    }

    /// Parse JSON array of `MqOrder` and handle each (always "ACK" at caller).
    pub fn handle_body(&self, body: &[u8]) -> Result<(), InboundError> {
        let orders: Vec<MqOrder> = serde_json::from_slice(body)?;
        for mq in orders {
            let _ = self.handle_mq_order(&mq);
        }
        Ok(())
    }

    /// Port of `BaseConsumer.handleMqData` (with START_QUEUE / BigNo dedupe).
    pub fn handle_mq_order(&self, mq_order: &MqOrder) -> Result<(), InboundError> {
        if !check_mq_order(mq_order) {
            telemetry::record_inbound_invalid();
            warn!(
                symbol = ?mq_order.symbol_key,
                order_no = ?mq_order.trust_order_no,
                "inbound validation failed"
            );
            return Ok(());
        }

        let Some(order) = type_convert(mq_order) else {
            telemetry::record_inbound_invalid();
            warn!(
                symbol = ?mq_order.symbol_key,
                order_no = ?mq_order.trust_order_no,
                "inbound type_convert failed"
            );
            return Ok(());
        };

        if self.start_queue.should_dedupe(&order) {
            return Ok(());
        }

        self.enqueue(order)?;
        telemetry::record_order_placed();
        Ok(())
    }

    /// Restore path (`InitLoadData.handleMqData`): no START_QUEUE dedupe on the way in;
    /// records restored nos after enqueue.
    pub fn handle_restore_order(&self, mq_order: &MqOrder) -> Result<(), InboundError> {
        if !check_mq_order(mq_order) {
            warn!(
                symbol = ?mq_order.symbol_key,
                order_no = ?mq_order.trust_order_no,
                "restore validation failed"
            );
            return Ok(());
        }

        let Some(order) = type_convert(mq_order) else {
            warn!(
                symbol = ?mq_order.symbol_key,
                order_no = ?mq_order.trust_order_no,
                "restore type_convert failed"
            );
            return Ok(());
        };

        let symbol = order.symbol_key.clone();
        let trust_order_no = order.trust_order_no.clone();
        self.enqueue(order)?;
        self.start_queue.record_restored(&symbol, &trust_order_no);
        Ok(())
    }

    fn enqueue(&self, order: BbOrder) -> Result<(), InboundError> {
        let symbol = order.symbol_key.clone();
        let tx = {
            let queues = self.queues.lock().expect("queues lock");
            queues.get(&symbol).cloned()
        };
        match tx {
            Some(tx) => tx.try_send(order).map_err(|e| match e {
                mpsc::error::TrySendError::Full(_) => InboundError::Enqueue("channel full".into()),
                mpsc::error::TrySendError::Closed(_) => {
                    InboundError::Enqueue("channel closed".into())
                }
            }),
            None => {
                warn!(
                    symbol = %symbol,
                    order_no = %order.trust_order_no,
                    "symbol queue missing"
                );
                Err(InboundError::MissingQueue(symbol))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bigdecimal::BigDecimal;
    use std::str::FromStr;

    fn sample_mq(order_no: &str, status: i8) -> MqOrder {
        MqOrder {
            user_id: Some(1),
            uid: Some(100),
            c_type: 1,
            deal_type: None,
            r#type: Some(1),
            order_type: Some(1),
            market_id: Some(1),
            coin_id: Some(2),
            symbol_key: Some("btcusdt".into()),
            coin_market: Some("BTC/USDT".into()),
            trust_order_no: Some(order_no.into()),
            close_position: Some(1),
            start_deposit: Some("100".into()),
            position_type: Some(1),
            taker_rate: Some("0.001".into()),
            order_status: Some(status),
            order_form: Some(1),
            gear: None,
            lever_times: Some(10),
            trust_number: Some("1".into()),
            trust_price: Some("50000".into()),
            create_time: Some(1_700_000_000),
            face_value: Some(BigDecimal::from_str("1").unwrap()),
            handicap_type: None,
        }
    }

    #[tokio::test]
    async fn dedupe_by_restored_order_no() {
        let state = Arc::new(StartQueueState::new());
        state.record_restored("btcusdt", "100");
        let router = Arc::new(InboundRouter::new(Arc::clone(&state)));
        let (tx, mut rx) = mpsc::channel(64);
        router.register_queue("btcusdt", tx);

        router.handle_mq_order(&sample_mq("100", 0)).unwrap();
        assert!(rx.try_recv().is_err(), "duplicate restored order must drop");

        router.handle_mq_order(&sample_mq("101", 0)).unwrap();
        let got = rx.recv().await.expect("new order");
        assert_eq!(got.trust_order_no, "101");
    }

    #[tokio::test]
    async fn dedupe_by_big_no() {
        let state = Arc::new(StartQueueState::new());
        // Non-empty map activates BigNo path.
        state.record_restored("ethusdt", "50");
        assert_eq!(state.big_no(), 50);

        let router = Arc::new(InboundRouter::new(Arc::clone(&state)));
        let (tx, mut rx) = mpsc::channel(64);
        router.register_queue("btcusdt", tx);

        // big_no=50 >= 40 → drop
        router.handle_mq_order(&sample_mq("40", 0)).unwrap();
        assert!(rx.try_recv().is_err());

        // 51 > 50 → accept
        router.handle_mq_order(&sample_mq("51", 0)).unwrap();
        assert_eq!(rx.recv().await.unwrap().trust_order_no, "51");
    }

    #[tokio::test]
    async fn revoke_bypasses_dedupe() {
        let state = Arc::new(StartQueueState::new());
        state.record_restored("btcusdt", "100");
        let router = Arc::new(InboundRouter::new(Arc::clone(&state)));
        let (tx, mut rx) = mpsc::channel(64);
        router.register_queue("btcusdt", tx);

        router
            .handle_mq_order(&sample_mq("100", ORDER_STATUS_REVOKE))
            .unwrap();
        assert_eq!(rx.recv().await.unwrap().trust_order_no, "100");
    }

    #[tokio::test]
    async fn clear_disables_dedupe() {
        let state = Arc::new(StartQueueState::new());
        state.record_restored("btcusdt", "100");
        state.clear();
        assert!(!state.is_active());
        assert_eq!(state.big_no(), 0);

        let router = Arc::new(InboundRouter::new(Arc::clone(&state)));
        let (tx, mut rx) = mpsc::channel(64);
        router.register_queue("btcusdt", tx);
        router.handle_mq_order(&sample_mq("100", 0)).unwrap();
        assert_eq!(rx.recv().await.unwrap().trust_order_no, "100");
    }
}
