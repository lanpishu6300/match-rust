use bigdecimal::BigDecimal;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

/// Internal match-engine order aligned with Java `BBOrder`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BbOrder {
    pub user_id: i32,
    pub uid: i32,
    pub r#type: i8,
    pub order_type: i8,
    pub market_id: i32,
    pub coin_id: i32,
    /// 内联短字符串：热路径零堆分配（symbol/coin_market/order_no 均 < 23B 栈内）。
    pub symbol_key: SmolStr,
    pub coin_market: SmolStr,
    pub trust_order_no: SmolStr,
    pub order_form: i8,
    pub gear: Option<i32>,
    pub close_position: i8,
    pub start_deposit: BigDecimal,
    pub target_rate: BigDecimal,
    pub position_type: i8,
    pub lever_times: i32,
    pub order_status: i8,
    pub consumer_all_number: BigDecimal,
    pub current_deal_number: BigDecimal,
    pub trust_number: BigDecimal,
    pub trust_price: BigDecimal,
    pub remaining_number: BigDecimal,
    pub create_time: i64,
    pub face_value: Option<BigDecimal>,
    pub average_price: BigDecimal,
}
