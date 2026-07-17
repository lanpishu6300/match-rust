use bigdecimal::BigDecimal;
use serde::{Deserialize, Serialize};

/// Inbound MQ order payload aligned with Java `MqOrder`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MqOrder {
    pub user_id: Option<i32>,
    pub uid: Option<i32>,
    pub c_type: i8,
    pub deal_type: Option<i8>,
    pub r#type: Option<i8>,
    pub order_type: Option<i8>,
    pub market_id: Option<i32>,
    pub coin_id: Option<i32>,
    pub symbol_key: Option<String>,
    pub coin_market: Option<String>,
    pub trust_order_no: Option<String>,
    pub close_position: Option<i8>,
    pub start_deposit: Option<String>,
    pub position_type: Option<i8>,
    pub taker_rate: Option<String>,
    pub order_status: Option<i8>,
    pub order_form: Option<i8>,
    pub gear: Option<i32>,
    pub lever_times: Option<i32>,
    pub trust_number: Option<String>,
    pub trust_price: Option<String>,
    pub create_time: Option<i64>,
    pub face_value: Option<BigDecimal>,
    pub handicap_type: Option<i8>,
}
