//! Spot match integration tests.

use match_core::{BbOrder, Engine, Side};
use bigdecimal::Zero;
use match_protocol::{
    check_mq_order_spot, type_convert_spot, ORDER_FORM_LIMIT, ORDER_FORM_MARKET_PRICE,
    ORDER_STATUS_REVOKE, ORDER_TYPE_BUY, ORDER_TYPE_SELL, SPOT_NO_DEAL_NUMBER,
};
use match_spot::mq::memory::MemoryOrderSink;
use match_spot::mq::producer::Producer;
use match_spot::outbound::Outbound;
use match_spot::config::TopicSplitConfig;
use std::sync::Arc;

fn limit_order(side: i8, no: &str, price: &str, qty: &str) -> BbOrder {
    BbOrder(type_convert_spot(&mq_limit(side, no, price, qty)).expect("convert"))
}

fn mq_limit(side: i8, no: &str, price: &str, qty: &str) -> match_protocol::MqOrder {
    match_protocol::MqOrder {
        user_id: Some(1),
        uid: Some(1),
        c_type: 1,
        deal_type: None,
        r#type: Some(1),
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
        order_status: Some(0),
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

#[test]
fn spot_protocol_market_validation_and_convert() {
    let mut mq = mq_limit(ORDER_TYPE_BUY, "m1", "0", "1");
    mq.order_form = Some(ORDER_FORM_MARKET_PRICE);
    mq.gear = Some(3);
    assert!(check_mq_order_spot(&mq));
    let order = type_convert_spot(&mq).expect("market");
    assert_eq!(order.trust_price, bigdecimal::BigDecimal::zero());
}

#[test]
fn spot_limit_cross_emits_fill_and_entrust() {
    let sink = Arc::new(MemoryOrderSink::new());
    let outbound = Outbound::new(
        Producer::new(Arc::clone(&sink) as Arc<dyn match_spot::mq::OrderSink>),
        None,
        1000,
        TopicSplitConfig::default(),
    );
    let mut engine = Engine::new();

    let sell = limit_order(ORDER_TYPE_SELL, "s1", "100", "1");
    let sell_snap = sell.0.clone();
    let sell_events = engine.on_order(sell);
    outbound.handle_order_result("btcusdt", &sell_snap, &sell_events, &engine);

    let buy = limit_order(ORDER_TYPE_BUY, "b1", "100", "1");
    let buy_snap = buy.0.clone();
    let buy_events = engine.on_order(buy);
    outbound.handle_order_result("btcusdt", &buy_snap, &buy_events, &engine);

    let sent = sink.sent();
    assert!(sent.iter().any(|(t, _)| t.contains("market_push_order")), "entrust");
    assert!(sent.iter().any(|(t, _)| t.contains("order_push_order")), "fill");
    assert!(sent.iter().any(|(t, _)| t == "contract_match_market_push_no_deal"), "depth");
    assert!(!buy_events.is_empty(), "should fill");
}

#[test]
fn spot_cancel_revokes_book() {
    let mut engine = Engine::new();
    let resting = limit_order(ORDER_TYPE_BUY, "b2", "50", "2");
    engine.on_order(resting);

    let mut cancel_mq = mq_limit(ORDER_TYPE_BUY, "b2", "50", "2");
    cancel_mq.order_status = Some(ORDER_STATUS_REVOKE);
    let cancel = BbOrder(type_convert_spot(&cancel_mq).expect("cancel"));
    let events = engine.on_order(cancel);
    assert_eq!(events.len(), 1);
    assert!(engine.depth_levels("btcusdt", Side::Buy, 5).is_empty());
}

#[test]
fn spot_no_deal_constant_matches_java() {
    assert_eq!(SPOT_NO_DEAL_NUMBER, 25);
}
