use crate::mq_order::MqOrder;
use crate::spot_constants::{
    ORDER_STATUS, ORDER_TYPES, SPOT_ORDER_FORM_MARKET_PRICE, SPOT_ORDER_FORMS, SPOT_TYPES,
};

fn is_blank(value: &Option<String>) -> bool {
    match value {
        None => true,
        Some(s) => s.trim().is_empty(),
    }
}

fn contains_value(values: &[i8], value: i8) -> bool {
    values.contains(&value)
}

/// Validates inbound spot `MqOrder` (`BBConstants.checkMqOrder`).
pub fn check_mq_order_spot(mq_order: &MqOrder) -> bool {
    if mq_order.user_id.is_none() {
        return false;
    }

    let Some(order_type_flag) = mq_order.r#type else {
        return false;
    };
    if !contains_value(SPOT_TYPES, order_type_flag) {
        return false;
    }

    let Some(order_type) = mq_order.order_type else {
        return false;
    };
    if !contains_value(ORDER_TYPES, order_type) {
        return false;
    }

    if mq_order.market_id.is_none() || mq_order.coin_id.is_none() {
        return false;
    }

    let Some(order_form) = mq_order.order_form else {
        return false;
    };
    if !contains_value(SPOT_ORDER_FORMS, order_form) {
        return false;
    }
    if order_form == SPOT_ORDER_FORM_MARKET_PRICE && mq_order.gear.is_none() {
        return false;
    }

    if is_blank(&mq_order.symbol_key)
        || is_blank(&mq_order.coin_market)
        || is_blank(&mq_order.trust_order_no)
    {
        return false;
    }

    let Some(order_status) = mq_order.order_status else {
        return false;
    };
    if !contains_value(ORDER_STATUS, order_status) {
        return false;
    }

    if is_blank(&mq_order.trust_number) || is_blank(&mq_order.trust_price) {
        return false;
    }

    match mq_order.create_time {
        None | Some(0) | Some(..=0) => return false,
        Some(_) => {}
    }

    true
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
    fn valid_limit_order_passes() {
        assert!(check_mq_order_spot(&base_mq()));
    }

    #[test]
    fn market_order_requires_gear() {
        let mut mq = base_mq();
        mq.order_form = Some(SPOT_ORDER_FORM_MARKET_PRICE);
        mq.trust_price = Some("0".into());
        assert!(!check_mq_order_spot(&mq));
        mq.gear = Some(5);
        assert!(check_mq_order_spot(&mq));
    }

    #[test]
    fn rejects_contract_only_order_form() {
        let mut mq = base_mq();
        mq.order_form = Some(3);
        assert!(!check_mq_order_spot(&mq));
    }

    #[test]
    fn rejects_extended_user_types() {
        let mut mq = base_mq();
        mq.r#type = Some(4);
        assert!(!check_mq_order_spot(&mq));
    }

    #[test]
    fn rejects_missing_contract_fields_ok() {
        let mq = base_mq();
        assert!(check_mq_order_spot(&mq));
    }
}
