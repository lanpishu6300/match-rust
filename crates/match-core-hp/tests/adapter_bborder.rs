use bigdecimal::BigDecimal;
use match_core_hp::{
    adapter::{from_bb_order, AdapterError},
    HpCommand, Side, SymbolScale,
};
use match_protocol::{
    BbOrder, ORDER_FORM_LIMIT, ORDER_FORM_MARKET_PRICE, ORDER_STATUS_REVOKE, ORDER_STATUS_WAIT,
    ORDER_TYPE_BUY, ORDER_TYPE_SELL,
};
use std::str::FromStr;

fn scale() -> SymbolScale {
    SymbolScale {
        price_scale: 2,
        qty_scale: 4,
    }
}

fn base_order() -> BbOrder {
    BbOrder {
        user_id: 1,
        uid: 1,
        r#type: 1,
        order_type: ORDER_TYPE_BUY,
        market_id: 1,
        coin_id: 1,
        symbol_key: "btcusdt".into(),
        coin_market: "BTC/USDT".into(),
        trust_order_no: "42".into(),
        order_form: ORDER_FORM_LIMIT,
        gear: None,
        close_position: 1,
        start_deposit: BigDecimal::from(0),
        target_rate: BigDecimal::from(0),
        position_type: 0,
        lever_times: 1,
        order_status: ORDER_STATUS_WAIT,
        consumer_all_number: BigDecimal::from(0),
        current_deal_number: BigDecimal::from(0),
        trust_number: BigDecimal::from_str("1.5").unwrap(),
        trust_price: BigDecimal::from_str("100.05").unwrap(),
        remaining_number: BigDecimal::from_str("1.5").unwrap(),
        create_time: 99,
        face_value: None,
        average_price: BigDecimal::from(0),
    }
}

#[test]
fn adapts_limit_buy() {
    let o = base_order();
    let cmd = from_bb_order(&o, &scale()).unwrap();
    assert_eq!(
        cmd,
        HpCommand::Limit {
            side: Side::Buy,
            price_tick: 10005,
            qty_lot: 15000,
            ts: 99,
            client_id: 42,
        }
    );
}

#[test]
fn adapts_cancel() {
    let mut o = base_order();
    o.order_status = ORDER_STATUS_REVOKE;
    o.trust_order_no = "7".into();
    let cmd = from_bb_order(&o, &scale()).unwrap();
    assert_eq!(cmd, HpCommand::Cancel { id: 7 });
}

#[test]
fn adapts_market_with_max_fills_from_gear() {
    let mut o = base_order();
    o.order_type = ORDER_TYPE_SELL;
    o.order_form = ORDER_FORM_MARKET_PRICE;
    o.gear = Some(3);
    o.trust_price = BigDecimal::from(0);
    let cmd = from_bb_order(&o, &scale()).unwrap();
    assert_eq!(
        cmd,
        HpCommand::Market {
            side: Side::Sell,
            qty_lot: 15000,
            ts: 99,
            max_fills: Some(3),
            client_id: 42,
        }
    );
}

#[test]
fn rejects_excess_price_scale() {
    let mut o = base_order();
    o.trust_price = BigDecimal::from_str("100.999").unwrap();
    let err = from_bb_order(&o, &scale()).unwrap_err();
    assert!(matches!(err, AdapterError::Scale(_)));
}

#[test]
fn rejects_unsupported_side_and_form() {
    let mut o = base_order();
    o.order_type = 0;
    assert_eq!(
        from_bb_order(&o, &scale()).unwrap_err(),
        AdapterError::UnsupportedSide
    );

    o = base_order();
    o.order_form = match_protocol::ORDER_FORM_POST_ONLY;
    assert_eq!(
        from_bb_order(&o, &scale()).unwrap_err(),
        AdapterError::UnsupportedForm
    );
}

#[test]
fn rejects_invalid_id_and_timestamp() {
    let mut o = base_order();
    o.trust_order_no = "not-a-number".into();
    assert_eq!(
        from_bb_order(&o, &scale()).unwrap_err(),
        AdapterError::InvalidOrderId
    );

    o = base_order();
    o.create_time = -1;
    assert_eq!(
        from_bb_order(&o, &scale()).unwrap_err(),
        AdapterError::InvalidTimestamp
    );
}

#[test]
fn market_gear_zero_or_negative_rejected() {
    let mut o = base_order();
    o.order_form = ORDER_FORM_MARKET_PRICE;
    o.trust_price = BigDecimal::from(0);
    o.gear = Some(0);
    assert_eq!(
        from_bb_order(&o, &scale()).unwrap_err(),
        AdapterError::InvalidGear
    );
    o.gear = Some(-1);
    assert_eq!(
        from_bb_order(&o, &scale()).unwrap_err(),
        AdapterError::InvalidGear
    );
    o.gear = None;
    assert_eq!(
        from_bb_order(&o, &scale()).unwrap_err(),
        AdapterError::InvalidGear
    );
}
