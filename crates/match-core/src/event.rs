use serde::{Deserialize, Serialize};

/// Outcome events emitted by the matching engine (fills, revokes, etc.).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MatchEvent {
    Fill {
        symbol: String,
        taker_order_no: String,
        maker_order_no: String,
        /// Java `BBOrder.type` on the taker leg.
        taker_user_type: i8,
        /// Java `BBOrder.targetType` / maker `type`.
        maker_user_type: i8,
        price: String,
        qty: String,
        taker_remaining: String,
        maker_remaining: String,
        taker_status: u8,
        maker_status: u8,
    },
    Revoke {
        order_no: String,
        symbol: String,
        remaining: String,
        reason: String,
    },
}
