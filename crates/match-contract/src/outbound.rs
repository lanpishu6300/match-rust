//! Outbound path: map `MatchEvent` → push payloads; depth throttle; error queue on send fail.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use match_core::{Engine, MatchEvent, Side};
use match_protocol::BbOrder;
use serde::Serialize;
use tracing::{debug, error, warn};

use crate::error_queue::ErrorQueue;
use crate::mq::producer::Producer;
use crate::redis_store::RedisStore;
use crate::telemetry;

/// Minimal BBOrder-like push DTO for fill / revoke consumers.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PushOrder {
    pub symbol_key: String,
    pub trust_order_no: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_trust_order_no: Option<String>,
    pub trust_price: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deal_price: Option<String>,
    pub remaining_number: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_remaining_number: Option<String>,
    pub order_status: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_order_status: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_deal_number: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Depth level for no_deal / deeps topics.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DepthLevel {
    pub trust_price: String,
    pub cumulative_commission_quantity: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DepthSnapshot {
    pub symbol_key: String,
    pub bids: Vec<DepthLevel>,
    pub asks: Vec<DepthLevel>,
}

const DEPTH_LEVELS: usize = 20;

/// Handles match results: push fills/revokes and throttled depth.
pub struct Outbound {
    producer: Producer,
    redis: Option<Mutex<RedisStore>>,
    depth_push_interval_ms: u64,
    last_depth_push_ms: Mutex<HashMap<String, u64>>,
}

impl Outbound {
    pub fn new(producer: Producer, redis: Option<RedisStore>, depth_push_interval_ms: u64) -> Self {
        Self {
            producer,
            redis: redis.map(Mutex::new),
            depth_push_interval_ms,
            last_depth_push_ms: Mutex::new(HashMap::new()),
        }
    }

    pub fn producer(&self) -> &Producer {
        &self.producer
    }

    /// Emit outbound messages for match events and maybe depth.
    pub fn handle_events(&self, symbol: &str, events: &[MatchEvent], engine: &Engine) {
        let mut push_batch: Vec<PushOrder> = Vec::new();

        for event in events {
            match event {
                MatchEvent::Fill {
                    symbol,
                    taker_order_no,
                    maker_order_no,
                    price,
                    qty,
                    taker_remaining,
                    maker_remaining,
                    taker_status,
                    maker_status,
                } => {
                    telemetry::record_fill();
                    push_batch.push(PushOrder {
                        symbol_key: symbol.clone(),
                        trust_order_no: taker_order_no.clone(),
                        target_trust_order_no: Some(maker_order_no.clone()),
                        trust_price: price.clone(),
                        deal_price: Some(price.clone()),
                        remaining_number: taker_remaining.clone(),
                        target_remaining_number: Some(maker_remaining.clone()),
                        order_status: *taker_status,
                        target_order_status: Some(*maker_status),
                        current_deal_number: Some(qty.clone()),
                        reason: None,
                    });
                }
                MatchEvent::Revoke {
                    order_no,
                    symbol,
                    remaining,
                    reason,
                } => {
                    telemetry::record_order_cancelled();
                    push_batch.push(PushOrder {
                        symbol_key: symbol.clone(),
                        trust_order_no: order_no.clone(),
                        target_trust_order_no: None,
                        trust_price: "0".into(),
                        deal_price: None,
                        remaining_number: remaining.clone(),
                        target_remaining_number: None,
                        order_status: match_protocol::ORDER_STATUS_REVOKE_SUCCESS as u8,
                        target_order_status: None,
                        current_deal_number: None,
                        reason: Some(reason.clone()),
                    });
                }
            }
        }

        if !push_batch.is_empty() {
            self.send_push_orders(symbol, &push_batch);
        }

        self.maybe_push_depth(symbol, engine);
    }

    fn send_push_orders(&self, symbol: &str, orders: &[PushOrder]) {
        let body = match serde_json::to_vec(orders) {
            Ok(b) => b,
            Err(e) => {
                error!(error = %e, "serialize push orders failed");
                return;
            }
        };

        if let Err(e) = self.producer.send_push_order(symbol, &body) {
            warn!(error = %e, symbol, "push_order send failed → error_queue");
            self.on_send_fail(&body);
        }
        // Market trade push uses the same payload family (Java OrderProducer → MarketProducer).
        if let Err(e) = self.producer.send_push_market(symbol, &body) {
            warn!(error = %e, symbol, "push_market send failed → error_queue");
            self.on_send_fail(&body);
        }
    }

    fn maybe_push_depth(&self, symbol: &str, engine: &Engine) {
        let interval = self.depth_push_interval_ms;
        let now = now_ms();
        if interval > 0 {
            let mut last = self.last_depth_push_ms.lock().expect("depth throttle lock");
            if let Some(prev) = last.get(symbol) {
                if now.saturating_sub(*prev) < interval {
                    debug!(symbol, "depth push throttled");
                    return;
                }
            }
            last.insert(symbol.to_string(), now);
        }

        let bids = levels_to_dto(engine.depth_levels(symbol, Side::Buy, DEPTH_LEVELS));
        let asks = levels_to_dto(engine.depth_levels(symbol, Side::Sell, DEPTH_LEVELS));
        let snap = DepthSnapshot {
            symbol_key: symbol.to_string(),
            bids,
            asks,
        };
        let body = match serde_json::to_vec(&snap) {
            Ok(b) => b,
            Err(e) => {
                error!(error = %e, "serialize depth failed");
                return;
            }
        };

        if let Err(e) = self.producer.send_no_deal(symbol, &body) {
            warn!(error = %e, symbol, "no_deal send failed → error_queue");
            self.on_send_fail(&body);
        }
        if let Err(e) = self.producer.send_deeps(symbol, &body) {
            warn!(error = %e, symbol, "deeps send failed → error_queue");
            self.on_send_fail(&body);
        }
    }

    fn on_send_fail(&self, body: &[u8]) {
        let Some(redis) = &self.redis else {
            return;
        };
        match redis.lock() {
            Ok(mut store) => {
                let mut q = ErrorQueue::new(&mut store);
                if let Err(e) = q.push_raw(body) {
                    error!(error = %e, "error_queue push failed");
                }
            }
            Err(e) => error!(error = %e, "redis lock poisoned"),
        }
    }

    /// Experimental hp-engine outbound: map [`HpEvent`] → push JSON (Topic-compatible shape).
    #[cfg(feature = "hp-engine")]
    pub fn handle_hp_events(
        &self,
        symbol: &str,
        events: &[match_core_hp::HpEvent],
        engine: &match_core_hp::HpEngine,
        scale: &match_core_hp::SymbolScale,
    ) {
        use match_core_hp::{from_lot, from_tick, HpEvent};

        let mut push_batch: Vec<PushOrder> = Vec::new();
        for event in events {
            match event {
                HpEvent::Fill {
                    maker_id,
                    taker_id,
                    price_tick,
                    qty_lot,
                } => {
                    telemetry::record_fill();
                    let price = from_tick(scale, *price_tick);
                    let qty = from_lot(scale, *qty_lot);
                    let taker_rem = engine
                        .book
                        .store()
                        .get(*taker_id)
                        .map(|o| from_lot(scale, o.open_lot))
                        .unwrap_or_else(|| "0".into());
                    let maker_rem = engine
                        .book
                        .store()
                        .get(*maker_id)
                        .map(|o| from_lot(scale, o.open_lot))
                        .unwrap_or_else(|| "0".into());
                    push_batch.push(PushOrder {
                        symbol_key: symbol.to_string(),
                        trust_order_no: taker_id.to_string(),
                        target_trust_order_no: Some(maker_id.to_string()),
                        trust_price: price.clone(),
                        deal_price: Some(price),
                        remaining_number: taker_rem,
                        target_remaining_number: Some(maker_rem),
                        order_status: 2,
                        target_order_status: Some(2),
                        current_deal_number: Some(qty),
                        reason: None,
                    });
                }
                HpEvent::Revoke { id, .. } => {
                    telemetry::record_order_cancelled();
                    push_batch.push(PushOrder {
                        symbol_key: symbol.to_string(),
                        trust_order_no: id.to_string(),
                        target_trust_order_no: None,
                        trust_price: "0".into(),
                        deal_price: None,
                        remaining_number: "0".into(),
                        target_remaining_number: None,
                        order_status: match_protocol::ORDER_STATUS_REVOKE_SUCCESS as u8,
                        target_order_status: None,
                        current_deal_number: None,
                        reason: Some("cancel".into()),
                    });
                }
                HpEvent::Rest { .. } => {
                    telemetry::record_order_placed();
                }
            }
        }

        if !push_batch.is_empty() {
            self.send_push_orders(symbol, &push_batch);
        }

        self.maybe_push_hp_depth(symbol, engine, scale);
    }

    #[cfg(feature = "hp-engine")]
    fn maybe_push_hp_depth(
        &self,
        symbol: &str,
        engine: &match_core_hp::HpEngine,
        scale: &match_core_hp::SymbolScale,
    ) {
        use match_core_hp::{from_lot, from_tick, Side as HpSide};

        let interval = self.depth_push_interval_ms;
        let now = now_ms();
        if interval > 0 {
            let mut last = self.last_depth_push_ms.lock().expect("depth throttle lock");
            if let Some(prev) = last.get(symbol) {
                if now.saturating_sub(*prev) < interval {
                    debug!(symbol, "depth push throttled");
                    return;
                }
            }
            last.insert(symbol.to_string(), now);
        }

        let bids: Vec<DepthLevel> = engine
            .book
            .depth(HpSide::Buy, DEPTH_LEVELS)
            .into_iter()
            .map(|(tick, lot)| DepthLevel {
                trust_price: from_tick(scale, tick),
                cumulative_commission_quantity: from_lot(scale, lot),
            })
            .collect();
        let asks: Vec<DepthLevel> = engine
            .book
            .depth(HpSide::Sell, DEPTH_LEVELS)
            .into_iter()
            .map(|(tick, lot)| DepthLevel {
                trust_price: from_tick(scale, tick),
                cumulative_commission_quantity: from_lot(scale, lot),
            })
            .collect();
        let snap = DepthSnapshot {
            symbol_key: symbol.to_string(),
            bids,
            asks,
        };
        let body = match serde_json::to_vec(&snap) {
            Ok(b) => b,
            Err(e) => {
                error!(error = %e, "serialize hp depth failed");
                return;
            }
        };
        if let Err(e) = self.producer.send_no_deal(symbol, &body) {
            warn!(error = %e, symbol, "no_deal send failed → error_queue");
            self.on_send_fail(&body);
        }
        if let Err(e) = self.producer.send_deeps(symbol, &body) {
            warn!(error = %e, symbol, "deeps send failed → error_queue");
            self.on_send_fail(&body);
        }
    }
}

fn levels_to_dto(levels: Vec<(bigdecimal::BigDecimal, bigdecimal::BigDecimal)>) -> Vec<DepthLevel> {
    levels
        .into_iter()
        .map(|(price, qty)| DepthLevel {
            trust_price: price.to_string(),
            cumulative_commission_quantity: qty.to_string(),
        })
        .collect()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Helper used by tests / restore-less local runs: build push from a resting order snapshot.
#[allow(dead_code)]
pub fn push_from_bb_order(order: &BbOrder) -> PushOrder {
    PushOrder {
        symbol_key: order.symbol_key.clone(),
        trust_order_no: order.trust_order_no.clone(),
        target_trust_order_no: None,
        trust_price: order.trust_price.to_string(),
        deal_price: None,
        remaining_number: order.remaining_number.to_string(),
        target_remaining_number: None,
        order_status: order.order_status as u8,
        target_order_status: None,
        current_deal_number: Some(order.current_deal_number.to_string()),
        reason: None,
    }
}
