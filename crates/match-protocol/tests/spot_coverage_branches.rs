//! Exhaustive branch coverage for spot validate / convert.

use bigdecimal::BigDecimal;
use match_protocol::{check_mq_order_spot, type_convert_spot, MqOrder, SPOT_ORDER_FORM_MARKET_PRICE};
use std::str::FromStr;
use bigdecimal::Zero;

fn valid_mq() -> MqOrder {
    MqOrder {
        user_id: Some(1),
        uid: Some(2),
        c_type: 1,
        deal_type: None,
        r#type: Some(1),
        order_type: Some(1),
        market_id: Some(1),
        coin_id: Some(2),
        symbol_key: Some("btcusdt".into()),
        coin_market: Some("BTC/USDT".into()),
        trust_order_no: Some("100".into()),
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

#[test]
fn spot_check_rejects_each_required_field() {
    let cases: Vec<Box<dyn Fn(&mut MqOrder)>> = vec![
        Box::new(|o| o.user_id = None),
        Box::new(|o| o.r#type = None),
        Box::new(|o| o.r#type = Some(99)),
        Box::new(|o| o.order_type = None),
        Box::new(|o| o.order_type = Some(99)),
        Box::new(|o| o.market_id = None),
        Box::new(|o| o.coin_id = None),
        Box::new(|o| o.order_form = None),
        Box::new(|o| o.order_form = Some(3)),
        Box::new(|o| {
            o.order_form = Some(SPOT_ORDER_FORM_MARKET_PRICE);
            o.gear = None;
        }),
        Box::new(|o| o.symbol_key = None),
        Box::new(|o| o.symbol_key = Some("  ".into())),
        Box::new(|o| o.coin_market = None),
        Box::new(|o| o.coin_market = Some("".into())),
        Box::new(|o| o.trust_order_no = None),
        Box::new(|o| o.order_status = None),
        Box::new(|o| o.order_status = Some(99)),
        Box::new(|o| o.trust_number = None),
        Box::new(|o| o.trust_number = Some("".into())),
        Box::new(|o| o.trust_price = None),
        Box::new(|o| o.trust_price = Some("".into())),
        Box::new(|o| o.create_time = None),
        Box::new(|o| o.create_time = Some(0)),
        Box::new(|o| o.create_time = Some(-1)),
    ];
    for (i, mutate) in cases.into_iter().enumerate() {
        let mut o = valid_mq();
        mutate(&mut o);
        assert!(!check_mq_order_spot(&o), "case {i} should reject");
    }
}

#[test]
fn spot_check_accepts_spot_user_types() {
    for t in [1_i8, 2, 3] {
        let mut o = valid_mq();
        o.r#type = Some(t);
        assert!(check_mq_order_spot(&o), "type {t}");
    }
}

#[test]
fn spot_check_accepts_market_with_gear() {
    let mut o = valid_mq();
    o.order_form = Some(SPOT_ORDER_FORM_MARKET_PRICE);
    o.gear = Some(2);
    o.trust_price = Some("0".into());
    assert!(check_mq_order_spot(&o));
}

#[test]
fn spot_convert_rejects_missing_required_fields() {
    let cases: Vec<Box<dyn Fn(&mut MqOrder)>> = vec![
        Box::new(|o| o.symbol_key = None),
        Box::new(|o| o.trust_number = None),
        Box::new(|o| o.trust_price = None),
        Box::new(|o| o.order_form = None),
        Box::new(|o| o.user_id = None),
        Box::new(|o| o.r#type = None),
        Box::new(|o| o.order_type = None),
        Box::new(|o| o.market_id = None),
        Box::new(|o| o.coin_id = None),
        Box::new(|o| o.coin_market = None),
        Box::new(|o| o.trust_order_no = None),
        Box::new(|o| o.order_status = None),
        Box::new(|o| o.create_time = None),
    ];
    for (i, mutate) in cases.into_iter().enumerate() {
        let mut o = valid_mq();
        mutate(&mut o);
        assert!(type_convert_spot(&o).is_none(), "case {i}");
    }
}

#[test]
fn spot_convert_paths() {
    let mut o = valid_mq();
    o.trust_number = Some("0".into());
    assert!(type_convert_spot(&o).is_none());

    o = valid_mq();
    o.trust_price = Some("0".into());
    assert!(type_convert_spot(&o).is_none());

    o = valid_mq();
    o.order_form = Some(SPOT_ORDER_FORM_MARKET_PRICE);
    o.trust_price = Some("0".into());
    o.gear = Some(1);
    assert!(type_convert_spot(&o).is_some());

    o = valid_mq();
    o.trust_number = Some("bad".into());
    assert!(type_convert_spot(&o).is_none());

    o = valid_mq();
    o.trust_number = Some("-1".into());
    assert!(type_convert_spot(&o).is_none());

    o = valid_mq();
    o.uid = None;
    let bb = type_convert_spot(&o).expect("uid defaults to 0");
    assert_eq!(bb.uid, 0);

    o = valid_mq();
    o.start_deposit = Some("10.5".into());
    o.taker_rate = Some("0.001".into());
    o.close_position = Some(1);
    o.position_type = Some(2);
    o.lever_times = Some(5);
    let bb = type_convert_spot(&o).unwrap();
    assert_eq!(bb.start_deposit, BigDecimal::from_str("10.5").unwrap());
    assert_eq!(bb.lever_times, 5);

    o = valid_mq();
    o.start_deposit = Some("bad".into());
    o.taker_rate = Some("bad".into());
    o.face_value = Some(BigDecimal::from_str("1").unwrap());
    let bb = type_convert_spot(&o).unwrap();
    assert_eq!(bb.start_deposit, BigDecimal::zero());
    assert_eq!(bb.target_rate, BigDecimal::zero());
    assert_eq!(bb.face_value, Some(BigDecimal::from_str("1").unwrap()));

    o = valid_mq();
    o.symbol_key = Some("ETH/USDT".into());
    let bb = type_convert_spot(&o).unwrap();
    assert_eq!(bb.symbol_key, "ethusdt");
}

#[test]
fn spot_check_rejects_success_and_unknown_status() {
    let mut o = valid_mq();
    o.order_status = Some(1);
    assert!(!check_mq_order_spot(&o));
    o.order_status = Some(4);
    assert!(!check_mq_order_spot(&o));
}

#[test]
fn spot_check_accepts_wait_part_revoke_status() {
    for status in [0_i8, 2, 3] {
        let mut o = valid_mq();
        o.order_status = Some(status);
        assert!(check_mq_order_spot(&o), "status {status}");
    }
}

#[test]
fn spot_limit_zero_price_validate_ok_convert_none() {
    let mut o = valid_mq();
    o.trust_price = Some("0".into());
    assert!(check_mq_order_spot(&o));
    assert!(type_convert_spot(&o).is_none());
}

#[test]
fn spot_constants_depth_batch_sizes() {
    use match_protocol::{
        SPOT_DEEPS_NUMBER, SPOT_NO_DEAL_NUMBER, SPOT_ROBOT_NUMBER, SPOT_SEND_MAX_DATA,
    };
    assert_eq!(SPOT_NO_DEAL_NUMBER, 25);
    assert_eq!(SPOT_DEEPS_NUMBER, 30);
    assert_eq!(SPOT_ROBOT_NUMBER, 50);
    assert_eq!(SPOT_SEND_MAX_DATA, 1);
}
