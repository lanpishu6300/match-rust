use bigdecimal::{BigDecimal, Zero};
use std::str::FromStr;

use crate::mq_order::MqOrder;
use crate::order::BbOrder;
use crate::spot_constants::SPOT_ORDER_FORM_MARKET_PRICE;

fn parse_decimal(value: &str) -> Option<BigDecimal> {
    BigDecimal::from_str(value).ok()
}

/// Converts a validated spot `MqOrder` into `BbOrder` (`BBConstants.typeConvert`).
pub fn type_convert_spot(mq_order: &MqOrder) -> Option<BbOrder> {
    let symbol_key = mq_order
        .symbol_key
        .as_ref()?
        .replace('/', "")
        .to_lowercase();

    let trust_number = parse_decimal(mq_order.trust_number.as_ref()?)?;
    let trust_price = parse_decimal(mq_order.trust_price.as_ref()?)?;

    if trust_number <= BigDecimal::zero() {
        return None;
    }

    let order_form = mq_order.order_form?;
    if trust_price <= BigDecimal::zero() && order_form != SPOT_ORDER_FORM_MARKET_PRICE {
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
        order_form,
        gear: Some(mq_order.gear.unwrap_or(0)),
        close_position: mq_order.close_position.unwrap_or(0),
        start_deposit: mq_order
            .start_deposit
            .as_ref()
            .and_then(|s| parse_decimal(s))
            .unwrap_or_else(BigDecimal::zero),
        target_rate: mq_order
            .taker_rate
            .as_ref()
            .and_then(|s| parse_decimal(s))
            .unwrap_or_else(BigDecimal::zero),
        position_type: mq_order.position_type.unwrap_or(0),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MqOrder;

    fn base_mq() -> MqOrder {
        MqOrder {
            user_id: Some(1),
            uid: Some(2),
            c_type: 1,
            deal_type: None,
            r#type: Some(1),
            order_type: Some(1),
            market_id: Some(1),
            coin_id: Some(2),
            symbol_key: Some("BTC/USDT".into()),
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
            trust_number: Some("1.5".into()),
            trust_price: Some("50000".into()),
            create_time: Some(1_700_000_000),
            face_value: None,
            handicap_type: None,
        }
    }

    #[test]
    fn normalizes_symbol_key() {
        let order = type_convert_spot(&base_mq()).expect("convert");
        assert_eq!(order.symbol_key, "btcusdt");
    }

    #[test]
    fn market_zero_price_allowed() {
        let mut mq = base_mq();
        mq.order_form = Some(SPOT_ORDER_FORM_MARKET_PRICE);
        mq.trust_price = Some("0".into());
        mq.gear = Some(3);
        let order = type_convert_spot(&mq).expect("market convert");
        assert_eq!(order.trust_price, BigDecimal::zero());
        assert_eq!(order.gear, Some(3));
    }

    #[test]
    fn limit_zero_price_rejected() {
        let mut mq = base_mq();
        mq.trust_price = Some("0".into());
        assert!(type_convert_spot(&mq).is_none());
    }

    #[test]
    fn gear_defaults_to_zero() {
        let order = type_convert_spot(&base_mq()).expect("convert");
        assert_eq!(order.gear, Some(0));
    }
}
