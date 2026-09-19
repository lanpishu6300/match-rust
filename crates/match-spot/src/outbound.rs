use match_protocol::{
    ORDER_STATUS_REVOKE, ORDER_STATUS_REVOKE_SUCCESS, ORDER_FORM_MARKET_PRICE, ORDER_ROBOT,
    SPOT_NO_DEAL_NUMBER, SPOT_ORDER_USER,
};
use match_core::{Engine, MatchEvent, Side};
use match_protocol::BbOrder;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, error, warn};

use crate::config::TopicSplitConfig;
use crate::error_queue::ErrorQueue;
use crate::mq::producer::Producer;
use crate::redis_store::RedisStore;
use crate::spot_depth::build_depth_levels;
use crate::telemetry;

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

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DepthLevel {
    pub trust_price: String,
    pub cumulative_transaction_volume: String,
    pub cumulative_commission_quantity: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HandicapDepthData {
    pub symbol_key: String,
    pub bids: Vec<DepthLevel>,
    pub asks: Vec<DepthLevel>,
    pub buy_sum_all_entrust_number: String,
    pub sell_sum_all_entrust_number: String,
}

pub struct Outbound {
    producer: Producer,
    redis: Option<Mutex<RedisStore>>,
    depth_push_interval_ms: u64,
    last_depth_push_ms: Mutex<HashMap<String, u64>>,
    topic_split: TopicSplitConfig,
    /// Optional MoldUDP64 multicast publisher: when attached, every fill /
    /// revoke push and depth snapshot is also broadcast on the market-data
    /// feed (see `attach_mold`).
    mold: Mutex<Option<match_moldudp64::MoldPublisher>>,
}

impl Outbound {
    pub fn new(
        producer: Producer,
        redis: Option<RedisStore>,
        depth_push_interval_ms: u64,
        topic_split: TopicSplitConfig,
    ) -> Self {
        Self {
            producer,
            redis: redis.map(Mutex::new),
            depth_push_interval_ms,
            last_depth_push_ms: Mutex::new(HashMap::new()),
            topic_split,
            mold: Mutex::new(None),
        }
    }

    /// Attach a MoldUDP64 publisher; every order-result push and depth
    /// snapshot is then also multicast on the Mold feed. Safe to call on an
    /// `Arc<Outbound>` (locks internally).
    pub fn attach_mold(&self, publisher: match_moldudp64::MoldPublisher) {
        *self.mold.lock().expect("mold lock") = Some(publisher);
    }

    /// Broadcast `body` (tagged payload) on the Mold feed when attached.
    fn publish_mold(&self, tag: u8, body: &[u8]) {
        let guard = self.mold.lock().expect("mold lock");
        let Some(publisher) = guard.as_ref() else {
            return;
        };
        if let Err(e) = publisher.publish_tagged(tag, body) {
            warn!(error = %e, "moldudp64 publish failed");
        }
    }

    pub fn handle_order_result(
        &self,
        symbol: &str,
        order: &BbOrder,
        events: &[MatchEvent],
        engine: &Engine,
    ) {
        if order.order_status != ORDER_STATUS_REVOKE && order.order_form != ORDER_FORM_MARKET_PRICE {
            self.send_market_entrust(order);
        }

        for event in events {
            self.send_one_event(symbol, order, event);
        }

        self.maybe_push_depth(symbol, engine);
    }

    fn send_market_entrust(&self, order: &BbOrder) {
        let body = serialize_push_body_required(&[order]);
        if let Err(e) = self
            .producer
            .send_push_market(&order.symbol_key, false, &body)
        {
            warn!(error = %e, symbol = %order.symbol_key, "market entrust send failed");
            self.on_send_fail(&body);
        }
    }

    /// Java `SEND_MAX_DATA=1`: one fill/revoke per MQ message batch.
    fn send_one_event(&self, symbol: &str, taker: &BbOrder, event: &MatchEvent) {
        let push = match event {
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
                ..
            } => {
                telemetry::record_fill();
                PushOrder {
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
                }
            }
            MatchEvent::Revoke {
                order_no,
                symbol,
                remaining,
                reason,
                ..
            } => {
                telemetry::record_order_cancelled();
                PushOrder {
                    symbol_key: symbol.clone(),
                    trust_order_no: order_no.clone(),
                    target_trust_order_no: None,
                    trust_price: "0".into(),
                    deal_price: None,
                    remaining_number: remaining.clone(),
                    target_remaining_number: None,
                    order_status: ORDER_STATUS_REVOKE_SUCCESS as u8,
                    target_order_status: None,
                    current_deal_number: None,
                    reason: Some(reason.clone()),
                }
            }
        };

        let maker_type = match event {
            MatchEvent::Fill {
                maker_user_type, ..
            } => Some(*maker_user_type),
            MatchEvent::Revoke { .. } => None,
        };

        let mm_suffix = self
            .topic_split
            .uses_mm_suffix(&taker.coin_market, taker.r#type, maker_type);

        let batch = vec![push];
        let body = serialize_push_body_required(&batch);

        if let Err(e) = self
            .producer
            .send_push_market(symbol, mm_suffix, &body)
        {
            warn!(error = %e, symbol, "market fill send failed");
            self.on_send_fail(&body);
        }

        // MoldUDP64 market-data feed: broadcast the same fill/revoke payload.
        self.publish_mold(match_moldudp64::MSG_TAG_FILL_ORDER, &body);

        if self.should_send_order_push(taker, event) {
            try_send_order_push(&self.producer, symbol, mm_suffix, &body, self);
        }
    }

    /// Java `OrderProducer`: skip order module when robot×robot fill or robot revoke.
    pub(crate) fn should_send_order_push(&self, taker: &BbOrder, event: &MatchEvent) -> bool {
        match event {
            MatchEvent::Fill {
                taker_user_type,
                maker_user_type,
                ..
            } => !(*taker_user_type == ORDER_ROBOT && *maker_user_type == ORDER_ROBOT),
            MatchEvent::Revoke { .. } => taker.r#type != ORDER_ROBOT,
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

        let limit = SPOT_NO_DEAL_NUMBER as usize;
        let (bids, buy_sum) = build_depth_levels(
            engine
                .resting_orders(symbol, Side::Buy)
                .into_iter()
                .map(|o| o.0),
            limit,
        );
        let (asks, sell_sum) = build_depth_levels(
            engine
                .resting_orders(symbol, Side::Sell)
                .into_iter()
                .map(|o| o.0),
            limit,
        );

        let snap = HandicapDepthData {
            symbol_key: symbol.to_string(),
            bids,
            asks,
            buy_sum_all_entrust_number: buy_sum.to_string(),
            sell_sum_all_entrust_number: sell_sum.to_string(),
        };
        let body = serialize_push_body_required(&snap);

        if let Err(e) = self.producer.send_no_deal(&body) {
            warn!(error = %e, symbol, "no_deal send failed");
            self.on_send_fail(&body);
        }
        if let Err(e) = self.producer.send_deeps(&body) {
            warn!(error = %e, symbol, "deeps send failed");
            self.on_send_fail(&body);
        }

        // MoldUDP64 market-data feed: broadcast the depth snapshot.
        self.publish_mold(match_moldudp64::MSG_TAG_DEPTH, &body);
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
            Err(e) => log_redis_lock_poisoned(e),
        }
    }
}

#[cfg_attr(coverage, coverage(off))]
fn try_send_order_push(
    producer: &Producer,
    symbol: &str,
    mm_suffix: bool,
    body: &[u8],
    outbound: &Outbound,
) {
    if let Err(e) = producer.send_push_order(symbol, mm_suffix, body) {
        warn!(error = %e, symbol, "order push send failed");
        outbound.on_send_fail(body);
    }
}

fn serialize_push_body<T: serde::Serialize>(value: &T) -> Option<Vec<u8>> {
    match serde_json::to_vec(value) {
        Ok(body) => Some(body),
        Err(e) => {
            log_serialize_error(e);
            None
        }
    }
}

#[cfg_attr(coverage, coverage(off))]
fn serialize_push_body_required<T: serde::Serialize>(value: &T) -> Vec<u8> {
    serialize_push_body(value).expect("push payload must serialize")
}

#[cfg_attr(coverage, coverage(off))]
fn log_serialize_error(e: serde_json::Error) {
    error!(error = %e, "serialize push payload failed");
}

#[cfg_attr(coverage, coverage(off))]
fn log_redis_lock_poisoned(e: std::sync::PoisonError<std::sync::MutexGuard<'_, crate::redis_store::RedisStore>>) {
    error!(error = %e, "redis lock poisoned");
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bigdecimal::{BigDecimal, Zero};
    use match_core::MatchEvent;
    use std::sync::Arc;

    fn test_outbound(sink: std::sync::Arc<crate::mq::memory::MemoryOrderSink>) -> Outbound {
        Outbound {
            producer: Producer::new(sink),
            redis: None,
            depth_push_interval_ms: 0,
            last_depth_push_ms: Mutex::new(HashMap::new()),
            topic_split: TopicSplitConfig::default(),
            mold: Mutex::new(None),
        }
    }

    fn sample_taker(r#type: i8) -> BbOrder {
        BbOrder {
            user_id: 1,
            uid: 1,
            r#type,
            order_type: 1,
            market_id: 1,
            coin_id: 1,
            symbol_key: "btcusdt".into(),
            coin_market: "BTC/USDT".into(),
            trust_order_no: "t1".into(),
            order_form: 1,
            gear: Some(0),
            close_position: 0,
            start_deposit: BigDecimal::zero(),
            target_rate: BigDecimal::zero(),
            position_type: 0,
            lever_times: 0,
            order_status: 0,
            consumer_all_number: BigDecimal::zero(),
            current_deal_number: BigDecimal::zero(),
            trust_number: BigDecimal::from(1),
            trust_price: BigDecimal::from(1),
            remaining_number: BigDecimal::zero(),
            create_time: 1,
            face_value: None,
            average_price: BigDecimal::zero(),
        }
    }

    #[test]
    fn handicap_depth_serializes_camel_case() {
        let snap = HandicapDepthData {
            symbol_key: "btcusdt".into(),
            bids: vec![DepthLevel {
                trust_price: "1".into(),
                cumulative_transaction_volume: "0".into(),
                cumulative_commission_quantity: "2".into(),
            }],
            asks: vec![],
            buy_sum_all_entrust_number: "2".into(),
            sell_sum_all_entrust_number: "0".into(),
        };
        let json = serde_json::to_string(&snap).unwrap();
        assert!(json.contains("symbolKey"));
        assert!(json.contains("buySumAllEntrustNumber"));
    }

    #[test]
    fn robot_robot_fill_skips_order_push() {
        let outbound = test_outbound(std::sync::Arc::new(
            crate::mq::memory::MemoryOrderSink::new(),
        ));
        let taker = sample_taker(ORDER_ROBOT);
        let event = MatchEvent::Fill {
            symbol: "btcusdt".into(),
            taker_order_no: "t1".into(),
            maker_order_no: "m1".into(),
            taker_user_type: ORDER_ROBOT,
            maker_user_type: ORDER_ROBOT,
            price: "1".into(),
            qty: "1".into(),
            taker_remaining: "0".into(),
            maker_remaining: "0".into(),
            taker_status: 1,
            maker_status: 1,
        };
        assert!(!outbound.should_send_order_push(&taker, &event));
    }

    #[test]
    fn robot_user_fill_sends_order_push() {
        let outbound = test_outbound(std::sync::Arc::new(
            crate::mq::memory::MemoryOrderSink::new(),
        ));
        let taker = sample_taker(ORDER_ROBOT);
        let event = MatchEvent::Fill {
            symbol: "btcusdt".into(),
            taker_order_no: "t1".into(),
            maker_order_no: "m1".into(),
            taker_user_type: ORDER_ROBOT,
            maker_user_type: SPOT_ORDER_USER,
            price: "1".into(),
            qty: "1".into(),
            taker_remaining: "0".into(),
            maker_remaining: "0".into(),
            taker_status: 1,
            maker_status: 1,
        };
        assert!(outbound.should_send_order_push(&taker, &event));
    }

    #[test]
    fn robot_revoke_success_skips_order_push() {
        let outbound = test_outbound(std::sync::Arc::new(
            crate::mq::memory::MemoryOrderSink::new(),
        ));
        let mut taker = sample_taker(ORDER_ROBOT);
        taker.order_status = ORDER_STATUS_REVOKE;
        let event = MatchEvent::Revoke {
            order_no: "t1".into(),
            symbol: "btcusdt".into(),
            remaining: "0".into(),
            reason: "user".into(),
        };
        assert!(!outbound.should_send_order_push(&taker, &event));
    }

    #[test]
    fn send_one_event_per_fill() {
        let sink = std::sync::Arc::new(crate::mq::memory::MemoryOrderSink::new());
        let outbound = test_outbound(Arc::clone(&sink));
        let taker = sample_taker(SPOT_ORDER_USER);
        let e1 = MatchEvent::Fill {
            symbol: "btcusdt".into(),
            taker_order_no: "t1".into(),
            maker_order_no: "m1".into(),
            taker_user_type: SPOT_ORDER_USER,
            maker_user_type: SPOT_ORDER_USER,
            price: "1".into(),
            qty: "1".into(),
            taker_remaining: "0".into(),
            maker_remaining: "0".into(),
            taker_status: 1,
            maker_status: 1,
        };
        let e2 = MatchEvent::Fill {
            symbol: "btcusdt".into(),
            taker_order_no: "t1".into(),
            maker_order_no: "m2".into(),
            taker_user_type: SPOT_ORDER_USER,
            maker_user_type: SPOT_ORDER_USER,
            price: "1".into(),
            qty: "1".into(),
            taker_remaining: "0".into(),
            maker_remaining: "0".into(),
            taker_status: 1,
            maker_status: 1,
        };
        outbound.send_one_event("btcusdt", &taker, &e1);
        outbound.send_one_event("btcusdt", &taker, &e2);
        let order_pushes: Vec<_> = sink
            .sent()
            .into_iter()
            .filter(|(t, _)| t.contains("order_push"))
            .collect();
        assert_eq!(order_pushes.len(), 2, "SEND_MAX_DATA=1 → one push per fill");
    }

    fn sample_fill() -> MatchEvent {
        MatchEvent::Fill {
            symbol: "btcusdt".into(),
            taker_order_no: "t1".into(),
            maker_order_no: "m1".into(),
            taker_user_type: SPOT_ORDER_USER,
            maker_user_type: SPOT_ORDER_USER,
            price: "1".into(),
            qty: "1".into(),
            taker_remaining: "0".into(),
            maker_remaining: "0".into(),
            taker_status: 1,
            maker_status: 1,
        }
    }

    #[test]
    fn entrust_and_depth_send_success() {
        let sink = Arc::new(crate::mq::memory::MemoryOrderSink::new());
        test_outbound(sink).handle_order_result(
            "btcusdt",
            &sample_taker(SPOT_ORDER_USER),
            &[],
            &match_core::Engine::new(),
        );
    }

    #[test]
    fn order_push_send_failure_only() {
        let sink = Arc::new(crate::mq::memory::MemoryOrderSink::new());
        sink.fail_topic_containing("push_order_btcusdt");
        let outbound = Outbound::new(
            Producer::new(Arc::clone(&sink) as Arc<dyn crate::mq::traits::OrderSink>),
            Some(RedisStore::mock()),
            0,
            TopicSplitConfig::default(),
        );
        outbound.handle_order_result(
            "btcusdt",
            &sample_taker(SPOT_ORDER_USER),
            &[sample_fill()],
            &match_core::Engine::new(),
        );
    }

    #[test]
    fn depth_throttle_blocks_rapid_repush() {
        let sink = Arc::new(crate::mq::memory::MemoryOrderSink::new());
        let outbound = Outbound::new(
            Producer::new(Arc::clone(&sink) as Arc<dyn crate::mq::traits::OrderSink>),
            None,
            60_000,
            TopicSplitConfig::default(),
        );
        let mut taker = sample_taker(SPOT_ORDER_USER);
        taker.order_status = ORDER_STATUS_REVOKE;
        let engine = match_core::Engine::new();
        outbound.handle_order_result("x", &taker, &[], &engine);
        outbound.handle_order_result("x", &taker, &[], &engine);
    }

    #[test]
    fn depth_throttle_elapsed_interval() {
        let sink = Arc::new(crate::mq::memory::MemoryOrderSink::new());
        let outbound = Outbound::new(
            Producer::new(Arc::clone(&sink) as Arc<dyn crate::mq::traits::OrderSink>),
            None,
            10,
            TopicSplitConfig::default(),
        );
        let mut taker = sample_taker(SPOT_ORDER_USER);
        taker.order_status = ORDER_STATUS_REVOKE;
        let engine = match_core::Engine::new();
        outbound.handle_order_result("ethusdt", &taker, &[], &engine);
        std::thread::sleep(std::time::Duration::from_millis(15));
        outbound.handle_order_result("ethusdt", &taker, &[], &engine);
    }

    #[test]
    fn mq_send_failure_paths() {
        let engine = match_core::Engine::new();
        let mut revoked = sample_taker(SPOT_ORDER_USER);
        revoked.order_status = ORDER_STATUS_REVOKE;

        let sink = Arc::new(crate::mq::memory::MemoryOrderSink::new());
        sink.fail_topic_containing("market_push_order");
        test_outbound(Arc::clone(&sink)).handle_order_result(
            "btcusdt",
            &sample_taker(SPOT_ORDER_USER),
            &[],
            &engine,
        );

        sink.clear_fail_topics();
        sink.fail_topic_containing("no_deal");
        Outbound::new(
            Producer::new(Arc::clone(&sink) as Arc<dyn crate::mq::traits::OrderSink>),
            Some(RedisStore::mock()),
            0,
            TopicSplitConfig::default(),
        )
        .handle_order_result("btcusdt", &revoked, &[], &engine);

        sink.clear_fail_topics();
        sink.fail_topic_containing("deeps");
        Outbound::new(
            Producer::new(Arc::clone(&sink) as Arc<dyn crate::mq::traits::OrderSink>),
            Some(RedisStore::mock()),
            0,
            TopicSplitConfig::default(),
        )
        .handle_order_result("btcusdt", &revoked, &[], &engine);

        let outbound = test_outbound(Arc::clone(&sink));
        let taker = sample_taker(SPOT_ORDER_USER);
        outbound.send_one_event("btcusdt", &taker, &sample_fill());

        sink.clear_fail_topics();
        sink.fail_topic_containing("market_push_order");
        outbound.send_one_event("btcusdt", &taker, &sample_fill());

        sink.clear_fail_topics();
        sink.fail_topic_containing("contract_match_order_push_order");
        outbound.send_one_event("btcusdt", &taker, &sample_fill());

        sink.clear_fail_topics();
        sink.fail_topic_containing("market_push_order");
        test_outbound(Arc::clone(&sink)).handle_order_result(
            "btcusdt",
            &sample_taker(SPOT_ORDER_USER),
            &[],
            &engine,
        );

        sink.clear_fail_topics();
        sink.fail_topic_containing("no_deal");
        let mut store = RedisStore::mock();
        store.test_set_fail_lpush(true);
        Outbound::new(
            Producer::new(Arc::clone(&sink) as Arc<dyn crate::mq::traits::OrderSink>),
            Some(store),
            0,
            TopicSplitConfig::default(),
        )
        .handle_order_result("btcusdt", &revoked, &[], &engine);

        let skip = test_outbound(Arc::new(crate::mq::memory::MemoryOrderSink::new()));
        let mut market = sample_taker(SPOT_ORDER_USER);
        market.order_form = ORDER_FORM_MARKET_PRICE;
        skip.handle_order_result("btcusdt", &market, &[], &engine);
        skip.handle_order_result("btcusdt", &revoked, &[], &engine);

        let robot_sink = Arc::new(crate::mq::memory::MemoryOrderSink::new());
        let robot_out = test_outbound(Arc::clone(&robot_sink));
        let robot = sample_taker(ORDER_ROBOT);
        let robot_fill = MatchEvent::Fill {
            symbol: "btcusdt".into(),
            taker_order_no: "t1".into(),
            maker_order_no: "m1".into(),
            taker_user_type: ORDER_ROBOT,
            maker_user_type: ORDER_ROBOT,
            price: "1".into(),
            qty: "1".into(),
            taker_remaining: "0".into(),
            maker_remaining: "0".into(),
            taker_status: 1,
            maker_status: 1,
        };
        robot_out.send_one_event("btcusdt", &robot, &robot_fill);
        assert!(
            !robot_sink
                .sent()
                .iter()
                .any(|(t, _)| t.contains("order_push"))
        );
    }

    #[test]
    fn attached_mold_publisher_broadcasts_fill_and_depth() {
        use match_moldudp64::{MoldPublisher, MoldSubscriber, MSG_TAG_DEPTH, MSG_TAG_FILL_ORDER};
        use std::time::Duration;

        // Grab a free loopback port for the Mold feed, then release it.
        let port = {
            let probe = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
            probe.local_addr().unwrap().port()
        };
        let sub_addr: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();

        let sink = Arc::new(crate::mq::memory::MemoryOrderSink::new());
        let outbound = Outbound {
            producer: Producer::new(
                Arc::clone(&sink) as Arc<dyn crate::mq::traits::OrderSink>
            ),
            redis: None,
            depth_push_interval_ms: 0,
            last_depth_push_ms: Mutex::new(HashMap::new()),
            topic_split: TopicSplitConfig::default(),
            mold: Mutex::new(Some(
                MoldPublisher::new("MATCH_RUST", sub_addr, 64).unwrap(),
            )),
        };
        let mut sub = MoldSubscriber::new_unicast("MATCH_RUST", sub_addr, None).unwrap();

        // One fill: should emit both a fill/revoke payload and a depth snapshot.
        outbound.handle_order_result(
            "btcusdt",
            &sample_taker(SPOT_ORDER_USER),
            &[sample_fill()],
            &match_core::Engine::new(),
        );

        let mut buf = [0u8; 8192];
        let mut got_fill = false;
        let mut got_depth = false;
        let deadline = std::time::Instant::now() + Duration::from_millis(600);
        while std::time::Instant::now() < deadline {
            match sub.recv(&mut buf).unwrap() {
                Some(out) => {
                    for (_, payload) in out.messages {
                        match payload.first() {
                            Some(&MSG_TAG_FILL_ORDER) => got_fill = true,
                            Some(&MSG_TAG_DEPTH) => got_depth = true,
                            _ => {}
                        }
                    }
                }
                None => std::thread::sleep(Duration::from_millis(1)),
            }
            if got_fill && got_depth {
                break;
            }
        }
        assert!(got_fill, "fill payload must be broadcast on the Mold feed");
        assert!(got_depth, "depth snapshot must be broadcast on the Mold feed");
    }
}
