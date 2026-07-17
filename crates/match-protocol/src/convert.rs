use bigdecimal::{BigDecimal, Zero};
use std::str::FromStr;

use crate::mq_order::MqOrder;
use crate::order::BbOrder;

fn parse_decimal(value: &str) -> Option<BigDecimal> {
    BigDecimal::from_str(value.trim()).ok()
}

/// Converts a validated `MqOrder` into a `BbOrder`, mirroring Java `BBConstants.typeConvert`.
pub fn type_convert(mq_order: &MqOrder) -> Option<BbOrder> {
    let symbol_key = mq_order
        .symbol_key
        .as_ref()?
        .replace('/', "")
        .to_lowercase();

    let trust_number = parse_decimal(mq_order.trust_number.as_ref()?)?;
    let trust_price = parse_decimal(mq_order.trust_price.as_ref()?)?;

    if trust_number <= BigDecimal::zero() || trust_price <= BigDecimal::zero() {
        return None;
    }

    Some(BbOrder {
        user_id: mq_order.user_id?,
        uid: mq_order.uid.unwrap_or(0),
        r#type: mq_order.r#type?,
        order_type: mq_order.order_type?,
        market_id: mq_order.market_id?,
        coin_id: mq_order.coin_id?,
        symbol_key,
        coin_market: mq_order.coin_market.clone()?,
        trust_order_no: mq_order.trust_order_no.clone()?,
        order_form: mq_order.order_form?,
        gear: mq_order.gear.unwrap_or(0),
        close_position: mq_order.close_position?,
        start_deposit: parse_decimal(mq_order.start_deposit.as_ref()?)?,
        target_rate: parse_decimal(mq_order.taker_rate.as_ref()?)?,
        position_type: mq_order.position_type?,
        lever_times: mq_order.lever_times.unwrap_or(0),
        order_status: mq_order.order_status?,
        consumer_all_number: BigDecimal::zero(),
        current_deal_number: BigDecimal::zero(),
        trust_number: trust_number.clone(),
        trust_price: trust_price.clone(),
        remaining_number: trust_number,
        create_time: mq_order.create_time?,
        face_value: mq_order.face_value.clone(),
        average_price: BigDecimal::zero(),
    })
}
