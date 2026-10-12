use bigdecimal::BigDecimal;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

/// Inbound MQ order payload aligned with Java `MqOrder`.
///
/// 0-copy 优化（见 docs/zero-copy-audit.md §4 P0-b）：高频短文本字段
/// （symbol/coin_market/order_no/price/qty）用 `SmolStr`——≤23B 栈内存储、
/// 零堆分配；`Some(x.into())` 构造点自动兼容（&str/String → SmolStr）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MqOrder {
    pub user_id: Option<i32>,
    pub uid: Option<i32>,
    #[serde(default)]
    pub c_type: i8,
    pub deal_type: Option<i8>,
    pub r#type: Option<i8>,
    pub order_type: Option<i8>,
    pub market_id: Option<i32>,
    pub coin_id: Option<i32>,
    pub symbol_key: Option<SmolStr>,
    pub coin_market: Option<SmolStr>,
    pub trust_order_no: Option<SmolStr>,
    pub close_position: Option<i8>,
    pub start_deposit: Option<String>,
    pub position_type: Option<i8>,
    pub taker_rate: Option<String>,
    pub order_status: Option<i8>,
    pub order_form: Option<i8>,
    pub gear: Option<i32>,
    pub lever_times: Option<i32>,
    pub trust_number: Option<SmolStr>,
    pub trust_price: Option<SmolStr>,
    pub create_time: Option<i64>,
    pub face_value: Option<BigDecimal>,
    pub handicap_type: Option<i8>,
}
