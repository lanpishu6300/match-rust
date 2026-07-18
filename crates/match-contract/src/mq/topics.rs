//! RocketMQ topic / group name constants aligned with Java `BBConstants`.

use match_protocol::encode_symbol_key;

pub const PULL_ORDER_PREFIX: &str = "usdt_contract_match_order_";
pub const PUSH_ORDER_PREFIX: &str = "usdt_contract_match_order_push_order_";
pub const PUSH_MARKET_PREFIX: &str = "usdt_contract_match_market_push_order_";
pub const PUSH_NO_DEAL_PREFIX: &str = "usdt_contract_match_market_push_no_deal_";
pub const PUSH_DEEPS_PREFIX: &str = "usdt_contract_match_market_push_deeps_";
pub const PUSH_ROBOT: &str = "usdt_contract_match_market_push_robot";
pub const NEW_COIN: &str = "usdt_market_add_new_coin";
pub const NEW_COIN_GROUP: &str = "usdt_market_add_new_coin_group";
pub const PULL_GROUP: &str = "usdt_contract_match_channel_one_group";

/// Inbound pull topic for a symbol (`usdt_contract_match_order_{encoded}`).
pub fn pull_order_topic(symbol_key: &str) -> String {
    format!("{PULL_ORDER_PREFIX}{}", encode_symbol_key(symbol_key))
}

/// Consumer group for a symbol (`usdt_contract_match_channel_one_group{encoded}`).
pub fn pull_order_group(symbol_key: &str) -> String {
    format!("{PULL_GROUP}{}", encode_symbol_key(symbol_key))
}

pub fn push_order_topic(symbol_key: &str) -> String {
    format!("{PUSH_ORDER_PREFIX}{}", encode_symbol_key(symbol_key))
}

pub fn push_market_topic(symbol_key: &str) -> String {
    format!("{PUSH_MARKET_PREFIX}{}", encode_symbol_key(symbol_key))
}

pub fn push_no_deal_topic(symbol_key: &str) -> String {
    format!("{PUSH_NO_DEAL_PREFIX}{}", encode_symbol_key(symbol_key))
}

pub fn push_deeps_topic(symbol_key: &str) -> String {
    format!("{PUSH_DEEPS_PREFIX}{}", encode_symbol_key(symbol_key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_names_for_ascii_symbol() {
        assert_eq!(
            pull_order_topic("btcusdt"),
            "usdt_contract_match_order_btcusdt"
        );
        assert_eq!(
            pull_order_group("btcusdt"),
            "usdt_contract_match_channel_one_groupbtcusdt"
        );
        assert_eq!(
            push_order_topic("btcusdt"),
            "usdt_contract_match_order_push_order_btcusdt"
        );
        assert_eq!(
            push_market_topic("btcusdt"),
            "usdt_contract_match_market_push_order_btcusdt"
        );
        assert_eq!(
            push_no_deal_topic("btcusdt"),
            "usdt_contract_match_market_push_no_deal_btcusdt"
        );
        assert_eq!(
            push_deeps_topic("btcusdt"),
            "usdt_contract_match_market_push_deeps_btcusdt"
        );
        assert_eq!(PUSH_ROBOT, "usdt_contract_match_market_push_robot");
        assert_eq!(NEW_COIN, "usdt_market_add_new_coin");
        assert_eq!(NEW_COIN_GROUP, "usdt_market_add_new_coin_group");
        assert_eq!(PULL_GROUP, "usdt_contract_match_channel_one_group");
    }
}
