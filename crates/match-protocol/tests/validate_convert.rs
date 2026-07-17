use bigdecimal::BigDecimal;
use match_protocol::{check_mq_order, encode_symbol_key, type_convert, MqOrder};
use std::str::FromStr;

fn valid_mq() -> MqOrder {
    MqOrder {
        user_id: Some(1),
        uid: Some(100),
        c_type: 1,
        deal_type: Some(1),
        r#type: Some(1),
        order_type: Some(1),
        market_id: Some(1),
        coin_id: Some(2),
        symbol_key: Some("btcusdt".into()),
        coin_market: Some("BTC/USDT".into()),
        trust_order_no: Some("10001".into()),
        close_position: Some(1),
        start_deposit: Some("10".into()),
        position_type: Some(0),
        taker_rate: Some("0.0005".into()),
        order_status: Some(0),
        order_form: Some(1),
        gear: None,
        lever_times: Some(10),
        trust_number: Some("1".into()),
        trust_price: Some("50000".into()),
        create_time: Some(1_700_000_000_000),
        face_value: Some(BigDecimal::from_str("0.001").unwrap()),
        handicap_type: None,
    }
}

#[test]
fn check_mq_order_accepts_valid_limit() {
    assert!(check_mq_order(&valid_mq()));
}

#[test]
fn check_mq_order_rejects_market_without_gear() {
    let mut o = valid_mq();
    o.order_form = Some(2);
    o.gear = None;
    assert!(!check_mq_order(&o));
}

#[test]
fn type_convert_normalizes_symbol_and_remaining() {
    let bb = type_convert(&valid_mq()).expect("convert");
    assert_eq!(bb.symbol_key, "btcusdt");
    assert_eq!(bb.remaining_number, BigDecimal::from_str("1").unwrap());
    assert_eq!(bb.trust_price, BigDecimal::from_str("50000").unwrap());
}

#[test]
fn encode_symbol_key_ascii_passthrough() {
    assert_eq!(encode_symbol_key("btcusdt"), "btcusdt");
}
