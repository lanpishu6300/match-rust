//! Spot match boundary / edge-case integration tests (shell + protocol paths).

use std::sync::Arc;

use bigdecimal::BigDecimal;
use match_core::{BbOrder, Engine, MatchEvent, Side};
use match_protocol::{
    check_mq_order_spot, type_convert_spot, MqOrder, ORDER_FORM_LIMIT, ORDER_FORM_MARKET_PRICE,
    ORDER_STATUS_REVOKE, ORDER_STATUS_REVOKE_SUCCESS, ORDER_STATUS_SUCCESS,
    ORDER_STATUS_SUCCESS_PART, ORDER_STATUS_WAIT, ORDER_TYPE_BUY, ORDER_TYPE_SELL,
    SPOT_NO_DEAL_NUMBER, SPOT_ORDER_MARKET_USER, SPOT_ORDER_ROBOT, SPOT_ORDER_USER,
};
use match_spot::config::{TopicSplitConfig, DEFAULT_DEPTH_PUSH_INTERVAL_MS};
use match_spot::inbound::{InboundError, InboundRouter};
use match_spot::mq::memory::MemoryOrderSink;
use match_spot::mq::producer::Producer;
use match_spot::mq::OrderSink;
use match_spot::outbound::Outbound;
use match_spot::symbol_worker::spawn_symbol_worker;
use tokio::sync::mpsc;

fn mq_order(side: i8, no: &str, price: &str, qty: &str) -> MqOrder {
    MqOrder {
        user_id: Some(1),
        uid: Some(1),
        c_type: 1,
        deal_type: None,
        r#type: Some(SPOT_ORDER_USER),
        order_type: Some(side),
        market_id: Some(1),
        coin_id: Some(2),
        symbol_key: Some("btcusdt".into()),
        coin_market: Some("BTC/USDT".into()),
        trust_order_no: Some(no.into()),
        close_position: None,
        start_deposit: None,
        position_type: None,
        taker_rate: None,
        order_status: Some(ORDER_STATUS_WAIT),
        order_form: Some(ORDER_FORM_LIMIT),
        gear: None,
        lever_times: None,
        trust_number: Some(qty.into()),
        trust_price: Some(price.into()),
        create_time: Some(1_700_000_000),
        face_value: None,
        handicap_type: None,
    }
}

fn to_bb(mq: &MqOrder) -> BbOrder {
    BbOrder(type_convert_spot(mq).expect("convert"))
}

fn shell_sink() -> (Arc<MemoryOrderSink>, Arc<Outbound>, Arc<InboundRouter>) {
    let sink = Arc::new(MemoryOrderSink::new());
    let outbound = Arc::new(Outbound::new(
        Producer::new(Arc::clone(&sink) as Arc<dyn OrderSink>),
        None,
        DEFAULT_DEPTH_PUSH_INTERVAL_MS,
        TopicSplitConfig::default(),
    ));
    let router = Arc::new(InboundRouter::new());
    (sink, outbound, router)
}

async fn run_through_shell(router: &InboundRouter, outbound: Arc<Outbound>, orders: &[MqOrder]) {
    let (tx, rx) = mpsc::channel(orders.len().max(64));
    router.register_queue("btcusdt", tx);
    let worker = spawn_symbol_worker("btcusdt".into(), rx, outbound);
    for mq in orders {
        let _ = router.handle_mq_order(mq);
    }
    router.shutdown_queues();
    let _ = worker.await;
}

// --- Protocol boundaries ---

#[test]
fn spot_validate_rejects_terminal_and_unknown_status() {
    for status in [ORDER_STATUS_SUCCESS, 99_i8] {
        let mut mq = mq_order(ORDER_TYPE_BUY, "s", "1", "1");
        mq.order_status = Some(status);
        assert!(!check_mq_order_spot(&mq), "status {status}");
    }
    for status in [ORDER_STATUS_WAIT, ORDER_STATUS_SUCCESS_PART, ORDER_STATUS_REVOKE] {
        let mut mq = mq_order(ORDER_TYPE_BUY, "s", "1", "1");
        mq.order_status = Some(status);
        assert!(check_mq_order_spot(&mq), "status {status}");
    }
}

#[test]
fn spot_validate_accepts_market_user_type() {
    let mut mq = mq_order(ORDER_TYPE_BUY, "mu", "100", "1");
    mq.r#type = Some(SPOT_ORDER_MARKET_USER);
    assert!(check_mq_order_spot(&mq));
    assert!(type_convert_spot(&mq).is_some());
}

#[test]
fn spot_limit_zero_price_passes_validate_but_fails_convert() {
    let mq = mq_order(ORDER_TYPE_BUY, "z", "0", "1");
    assert!(check_mq_order_spot(&mq));
    assert!(type_convert_spot(&mq).is_none());
}

#[test]
fn spot_symbol_key_slash_normalized() {
    let mut mq = mq_order(ORDER_TYPE_BUY, "sym", "100", "1");
    mq.symbol_key = Some("ETH/USDT".into());
    let bb = type_convert_spot(&mq).unwrap();
    assert_eq!(bb.symbol_key, "ethusdt");
}

// --- Inbound boundaries ---

#[tokio::test]
async fn inbound_empty_batch_is_ok() {
    let router = InboundRouter::new();
    assert!(router.handle_body(b"[]").is_ok());
}

#[tokio::test]
async fn inbound_mixed_batch_enqueues_only_valid() {
    let router = InboundRouter::new();
    let (tx, mut rx) = mpsc::channel(4);
    router.register_queue("btcusdt", tx);

    let mut bad = mq_order(ORDER_TYPE_BUY, "bad", "100", "1");
    bad.trust_number = Some("not-a-number".into());
    let body = serde_json::to_vec(&[mq_order(ORDER_TYPE_BUY, "good", "100", "1"), bad]).unwrap();
    router.handle_body(&body).unwrap();

    let got = rx.try_recv().unwrap();
    assert_eq!(got.trust_order_no, "good");
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn inbound_limit_zero_price_silent_drop() {
    let router = InboundRouter::new();
    let (tx, mut rx) = mpsc::channel(4);
    router.register_queue("btcusdt", tx);

    let mut mq = mq_order(ORDER_TYPE_BUY, "zero", "0", "1");
    assert!(router.handle_mq_order(&mq).is_ok());
    assert!(rx.try_recv().is_err());

    mq.trust_price = Some("100".into());
    router.handle_mq_order(&mq).unwrap();
    assert_eq!(rx.try_recv().unwrap().trust_order_no, "zero");
}

#[tokio::test]
async fn inbound_missing_symbol_queue_returns_err() {
    let router = InboundRouter::new();
    let mut mq = mq_order(ORDER_TYPE_BUY, "orphan", "100", "1");
    mq.symbol_key = Some("unknown".into());
    assert!(matches!(
        router.handle_mq_order(&mq).unwrap_err(),
        InboundError::MissingQueue(ref s) if s == "unknown"
    ));
}

// --- Engine + outbound boundaries ---

#[test]
fn duplicate_trust_order_no_second_order_rest_only() {
    let mut engine = Engine::new();
    let first = to_bb(&mq_order(ORDER_TYPE_BUY, "dup", "100", "1"));
    let second = to_bb(&mq_order(ORDER_TYPE_BUY, "dup", "101", "1"));
    engine.on_order(first);
    assert!(engine.on_order(second).is_empty());
    assert_eq!(engine.resting_orders("btcusdt", Side::Buy).len(), 1);
}

#[test]
fn revoke_unknown_order_emits_no_events() {
    let mut engine = Engine::new();
    let mut mq = mq_order(ORDER_TYPE_BUY, "missing", "100", "1");
    mq.order_status = Some(ORDER_STATUS_REVOKE);
    let events = engine.on_order(to_bb(&mq));
    assert!(events.is_empty());
}

#[test]
fn partial_fill_leaves_remainder_on_book() {
    let mut engine = Engine::new();
    let sell = to_bb(&mq_order(ORDER_TYPE_SELL, "s", "100", "1"));
    engine.on_order(sell);

    let buy_mq = mq_order(ORDER_TYPE_BUY, "b", "100", "3");
    let buy = to_bb(&buy_mq);
    let events = engine.on_order(buy);
    assert!(events.iter().any(|e| matches!(e, MatchEvent::Fill { .. })));
    let levels = engine.depth_levels("btcusdt", Side::Buy, 5);
    assert_eq!(levels.len(), 1);
    assert_eq!(levels[0].1, BigDecimal::from(2));
}

#[test]
fn market_order_skips_entrust_push() {
    let sink = Arc::new(MemoryOrderSink::new());
    let outbound = Outbound::new(
        Producer::new(Arc::clone(&sink) as Arc<dyn OrderSink>),
        None,
        0,
        TopicSplitConfig::default(),
    );
    let mut engine = Engine::new();

    let sell = to_bb(&mq_order(ORDER_TYPE_SELL, "ms", "100", "5"));
    engine.on_order(sell);

    let mut market_mq = mq_order(ORDER_TYPE_BUY, "mb", "0", "1");
    market_mq.order_form = Some(ORDER_FORM_MARKET_PRICE);
    market_mq.gear = Some(1);
    market_mq.trust_price = Some("0".into());
    let market = to_bb(&market_mq);
    let snap = market.0.clone();
    let events = engine.on_order(market);
    outbound.handle_order_result("btcusdt", &snap, &events, &engine);

    let sent = sink.sent();
    let market_pushes: Vec<_> = sent
        .iter()
        .filter(|(t, _)| t.contains("market_push_order"))
        .collect();
    assert_eq!(
        market_pushes.len(),
        1,
        "market taker: fill push only, no entrust"
    );
    let json: serde_json::Value = serde_json::from_slice(&market_pushes[0].1).unwrap();
    assert!(json[0]["targetTrustOrderNo"].is_string());
}

#[tokio::test]
async fn shell_market_buy_fills_resting_sell() {
    let (sink, outbound, router) = shell_sink();
    let sell = mq_order(ORDER_TYPE_SELL, "shell_s", "100", "2");
    let mut market = mq_order(ORDER_TYPE_BUY, "shell_m", "0", "1");
    market.order_form = Some(ORDER_FORM_MARKET_PRICE);
    market.gear = Some(1);
    market.trust_price = Some("0".into());
    run_through_shell(&router, Arc::clone(&outbound), &[sell, market]).await;

    assert!(
        sink.sent()
            .iter()
            .any(|(t, _)| t.contains("order_push_order_btcusdt")),
        "market fill should push order"
    );
}

#[tokio::test]
async fn shell_robot_robot_fill_skips_order_push() {
    let (sink, outbound, router) = shell_sink();
    let mut sell = mq_order(ORDER_TYPE_SELL, "rs", "100", "1");
    sell.r#type = Some(SPOT_ORDER_ROBOT);
    let mut buy = mq_order(ORDER_TYPE_BUY, "rb", "100", "1");
    buy.r#type = Some(SPOT_ORDER_ROBOT);
    run_through_shell(&router, Arc::clone(&outbound), &[sell, buy]).await;

    assert!(
        !sink
            .sent()
            .iter()
            .any(|(t, _)| t.contains("order_push_order")),
        "robot×robot should skip order_push"
    );
}

#[tokio::test]
async fn shell_revoke_success_emits_revoke_push() {
    let (sink, outbound, router) = shell_sink();
    let rest = mq_order(ORDER_TYPE_BUY, "rc", "50", "2");
    let mut cancel = mq_order(ORDER_TYPE_BUY, "rc", "50", "2");
    cancel.order_status = Some(ORDER_STATUS_REVOKE);
    run_through_shell(&router, Arc::clone(&outbound), &[rest, cancel]).await;

    let push_bodies: Vec<_> = sink
        .sent()
        .iter()
        .filter(|(t, _)| t.contains("order_push_order"))
        .map(|(_, b)| b.clone())
        .collect();
    assert!(!push_bodies.is_empty());
    let json: serde_json::Value = serde_json::from_slice(&push_bodies[0]).unwrap();
    let row = &json[0];
    assert_eq!(
        row["orderStatus"].as_u64(),
        Some(ORDER_STATUS_REVOKE_SUCCESS as u64)
    );
}

#[tokio::test]
async fn shell_mm_suffix_on_robot_fill_when_split_enabled() {
    let sink = Arc::new(MemoryOrderSink::new());
    let mut split = TopicSplitConfig::default();
    split.topic_enable = true;
    let outbound = Arc::new(Outbound::new(
        Producer::new(Arc::clone(&sink) as Arc<dyn OrderSink>),
        None,
        DEFAULT_DEPTH_PUSH_INTERVAL_MS,
        split,
    ));
    let router = Arc::new(InboundRouter::new());

    let mut sell = mq_order(ORDER_TYPE_SELL, "mms", "100", "1");
    sell.r#type = Some(SPOT_ORDER_ROBOT);
    let mut buy = mq_order(ORDER_TYPE_BUY, "mmb", "100", "1");
    buy.r#type = Some(SPOT_ORDER_ROBOT);
    run_through_shell(&router, Arc::clone(&outbound), &[sell, buy]).await;

    assert!(
        sink.sent()
            .iter()
            .any(|(t, _)| t.ends_with("_mm") && t.contains("market_push_order")),
        "robot×robot fill uses mm market topic when split enabled"
    );
}

#[tokio::test]
async fn shell_depth_snapshot_respects_no_deal_limit() {
    let sink = Arc::new(MemoryOrderSink::new());
    let outbound = Arc::new(Outbound::new(
        Producer::new(Arc::clone(&sink) as Arc<dyn OrderSink>),
        None,
        0,
        TopicSplitConfig::default(),
    ));
    let router = Arc::new(InboundRouter::new());
    let orders: Vec<MqOrder> = (0..30)
        .map(|i| {
            let tick = 10_000 + i;
            mq_order(
                ORDER_TYPE_BUY,
                &format!("d{i}"),
                &format!("{}.{:02}", tick / 100, tick % 100),
                "1",
            )
        })
        .collect();
    run_through_shell(&router, outbound, &orders).await;

    let depth_body = sink
        .sent()
        .iter()
        .filter(|(t, _)| *t == "contract_match_market_push_no_deal")
        .last()
        .map(|(_, b)| b.clone())
        .expect("depth push");
    let snap: serde_json::Value = serde_json::from_slice(&depth_body).unwrap();
    assert_eq!(
        snap["bids"].as_array().unwrap().len(),
        SPOT_NO_DEAL_NUMBER as usize
    );
}

#[test]
fn no_deal_constant_matches_java_handicap_limit() {
    assert_eq!(SPOT_NO_DEAL_NUMBER, 25);
}

#[test]
fn spot_validate_rejects_revoke_success_status() {
    let mut mq = mq_order(ORDER_TYPE_BUY, "rs", "100", "1");
    mq.order_status = Some(ORDER_STATUS_REVOKE_SUCCESS);
    assert!(!check_mq_order_spot(&mq));
}

#[test]
fn revoke_filled_order_emits_no_events() {
    let mut engine = Engine::new();
    let sell = to_bb(&mq_order(ORDER_TYPE_SELL, "s", "100", "1"));
    engine.on_order(sell);
    let buy = to_bb(&mq_order(ORDER_TYPE_BUY, "b", "100", "1"));
    engine.on_order(buy);
    assert!(engine.resting_orders("btcusdt", Side::Buy).is_empty());

    let mut cancel_mq = mq_order(ORDER_TYPE_BUY, "b", "100", "1");
    cancel_mq.order_status = Some(ORDER_STATUS_REVOKE);
    assert!(engine.on_order(to_bb(&cancel_mq)).is_empty());
}

#[test]
fn same_price_time_priority_fifo() {
    let mut engine = Engine::new();
    engine.on_order(to_bb(&mq_order(ORDER_TYPE_SELL, "first", "100", "1")));
    engine.on_order(to_bb(&mq_order(ORDER_TYPE_SELL, "second", "100", "1")));
    let events = engine.on_order(to_bb(&mq_order(ORDER_TYPE_BUY, "taker", "100", "1")));
    assert_eq!(events.len(), 1);
    if let MatchEvent::Fill { maker_order_no, .. } = &events[0] {
        assert_eq!(maker_order_no, "first");
    } else {
        panic!("expected fill");
    }
}

#[test]
fn market_sell_sweeps_resting_buys() {
    let mut engine = Engine::new();
    engine.on_order(to_bb(&mq_order(ORDER_TYPE_BUY, "bid", "100", "2")));
    let mut market_mq = mq_order(ORDER_TYPE_SELL, "ms", "0", "1");
    market_mq.order_form = Some(ORDER_FORM_MARKET_PRICE);
    market_mq.gear = Some(1);
    market_mq.trust_price = Some("0".into());
    let events = engine.on_order(to_bb(&market_mq));
    assert!(events.iter().any(|e| matches!(e, MatchEvent::Fill { .. })));
    let levels = engine.depth_levels("btcusdt", Side::Buy, 5);
    assert_eq!(levels.len(), 1);
    assert_eq!(levels[0].1, BigDecimal::from(1));
}

#[tokio::test]
async fn shell_depth_push_throttled_within_interval() {
    let sink = Arc::new(MemoryOrderSink::new());
    let outbound = Arc::new(Outbound::new(
        Producer::new(Arc::clone(&sink) as Arc<dyn OrderSink>),
        None,
        DEFAULT_DEPTH_PUSH_INTERVAL_MS,
        TopicSplitConfig::default(),
    ));
    let router = Arc::new(InboundRouter::new());
    let o1 = mq_order(ORDER_TYPE_BUY, "t1", "100", "1");
    let o2 = mq_order(ORDER_TYPE_BUY, "t2", "101", "1");
    run_through_shell(&router, Arc::clone(&outbound), &[o1, o2]).await;

    let no_deal_count = sink
        .sent()
        .iter()
        .filter(|(t, _)| *t == "contract_match_market_push_no_deal")
        .count();
    assert_eq!(
        no_deal_count, 1,
        "depth_push_interval_ms={DEFAULT_DEPTH_PUSH_INTERVAL_MS} should throttle rapid repush"
    );
}
