use crate::constants::{ORDER_FORMS, ORDER_FORM_MARKET_PRICE, ORDER_STATUS, ORDER_TYPES, TYPES};
use crate::mq_order::MqOrder;

fn is_blank(value: &Option<String>) -> bool {
    match value {
        None => true,
        Some(s) => s.trim().is_empty(),
    }
}

fn contains_value(values: &[i8], value: i8) -> bool {
    values.contains(&value)
}

/// Validates an inbound `MqOrder` using the same rules as Java `BBConstants.checkMqOrder`.
pub fn check_mq_order(mq_order: &MqOrder) -> bool {
    if mq_order.user_id.is_none() {
        return false;
    }

    let Some(order_type_flag) = mq_order.r#type else {
        return false;
    };
    if !contains_value(TYPES, order_type_flag) {
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
    if !contains_value(ORDER_FORMS, order_form) {
        return false;
    }
    if order_form == ORDER_FORM_MARKET_PRICE {
        match mq_order.gear {
            Some(g) if g >= 1 => {}
            _ => return false,
        }
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

    if mq_order.close_position.is_none() {
        return false;
    }

    if is_blank(&mq_order.start_deposit) || is_blank(&mq_order.taker_rate) {
        return false;
    }

    if mq_order.position_type.is_none() {
        return false;
    }

    true
}
