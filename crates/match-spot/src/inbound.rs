//! Inbound path: validate → convert → enqueue (no START_QUEUE dedupe).

use std::collections::HashMap;
use std::sync::Mutex;

use match_protocol::{check_mq_order_spot, type_convert_spot, BbOrder, MqOrder};
use thiserror::Error;
use tokio::sync::mpsc;
use tracing::warn;

use crate::telemetry;

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
}

impl InboundRouter {
    pub fn new() -> Self {
        Self {
            queues: Mutex::new(HashMap::new()),
        }
    }

    pub fn register_queue(&self, symbol_key: &str, tx: mpsc::Sender<BbOrder>) {
        self.queues
            .lock()
            .expect("queues lock")
            .insert(symbol_key.to_string(), tx);
    }

    /// Drops all senders so symbol workers can exit (tests / graceful shutdown).
    pub fn shutdown_queues(&self) {
        self.queues.lock().expect("queues lock").clear();
    }

    pub fn handle_body(&self, body: &[u8]) -> Result<(), InboundError> {
        let orders: Vec<MqOrder> = serde_json::from_slice(body)?;
        for mq in orders {
            let _ = self.handle_mq_order(&mq);
        }
        Ok(())
    }

    /// Port of `BaseConsumer.handleMqData`.
    pub fn handle_mq_order(&self, mq_order: &MqOrder) -> Result<(), InboundError> {
        if !check_mq_order_spot(mq_order) {
            telemetry::record_inbound_invalid();
            warn!(
                symbol = ?mq_order.symbol_key,
                order_no = ?mq_order.trust_order_no,
                "inbound validation failed"
            );
            return Ok(());
        }

        let Some(order) = type_convert_spot(mq_order) else {
            telemetry::record_inbound_invalid();
            warn!(
                symbol = ?mq_order.symbol_key,
                order_no = ?mq_order.trust_order_no,
                "inbound type_convert failed"
            );
            return Ok(());
        };

        self.enqueue(order)?;
        telemetry::record_order_placed();
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

impl Default for InboundRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use match_protocol::SPOT_ORDER_FORM_MARKET_PRICE;

    fn sample_mq(order_no: &str) -> MqOrder {
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
            close_position: None,
            start_deposit: None,
            position_type: None,
            taker_rate: None,
            order_status: Some(0),
            order_form: Some(1),
            gear: None,
            lever_times: None,
            trust_number: Some("1".into()),
            trust_price: Some("50000".into()),
            create_time: Some(1_700_000_000),
            face_value: None,
            handicap_type: None,
        }
    }

    #[tokio::test]
    async fn accepts_valid_limit_order() {
        let router = InboundRouter::new();
        let (tx, mut rx) = mpsc::channel(64);
        router.register_queue("btcusdt", tx);
        router.handle_mq_order(&sample_mq("101")).unwrap();
        assert_eq!(rx.recv().await.unwrap().trust_order_no, "101");
    }

    #[tokio::test]
    async fn market_zero_price_converts() {
        let router = InboundRouter::new();
        let (tx, mut rx) = mpsc::channel(64);
        router.register_queue("btcusdt", tx);
        let mut mq = sample_mq("102");
        mq.order_form = Some(SPOT_ORDER_FORM_MARKET_PRICE);
        mq.trust_price = Some("0".into());
        mq.gear = Some(5);
        router.handle_mq_order(&mq).unwrap();
        let got = rx.recv().await.unwrap();
        assert_eq!(got.order_form, SPOT_ORDER_FORM_MARKET_PRICE);
    }

    #[tokio::test]
    async fn empty_json_batch_ok() {
        let router = InboundRouter::new();
        assert!(router.handle_body(b"[]").is_ok());
    }

    #[tokio::test]
    async fn invalid_limit_price_dropped_silently() {
        let router = InboundRouter::new();
        let (tx, mut rx) = mpsc::channel(4);
        router.register_queue("btcusdt", tx);
        let mut mq = sample_mq("bad-price");
        mq.trust_price = Some("0".into());
        router.handle_mq_order(&mq).unwrap();
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn validation_failure_does_not_enqueue() {
        let router = InboundRouter::new();
        let (tx, mut rx) = mpsc::channel(4);
        router.register_queue("btcusdt", tx);
        let mut mq = sample_mq("bad-status");
        mq.order_status = Some(99);
        router.handle_mq_order(&mq).unwrap();
        assert!(rx.try_recv().is_err());
    }
}
