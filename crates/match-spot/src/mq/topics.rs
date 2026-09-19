use match_protocol::{
    encode_symbol_key, SPOT_PULL_GROUP_MM_SUFFIX, SPOT_PULL_ORDER, SPOT_PULL_ORDER_MM_SUFFIX,
    SPOT_PUSH_DEEPS, SPOT_PUSH_NO_DEAL, SPOT_PUSH_ORDER_PREFIX, SPOT_PUSH_MARKET_PREFIX,
    SPOT_PUSH_ROBOT,
};

pub fn pull_order_topic() -> &'static str {
    SPOT_PULL_ORDER
}

pub fn pull_order_mm_topic() -> String {
    format!("{SPOT_PULL_ORDER}{SPOT_PULL_ORDER_MM_SUFFIX}")
}

pub fn pull_group(base: &str) -> String {
    base.to_string()
}

pub fn pull_mm_group(base: &str) -> String {
    format!("{base}{SPOT_PULL_GROUP_MM_SUFFIX}")
}

pub fn push_order_topic(symbol_key: &str, mm_suffix: bool) -> String {
    let mut topic = format!("{SPOT_PUSH_ORDER_PREFIX}{}", encode_symbol_key(symbol_key));
    if mm_suffix {
        topic.push_str(SPOT_PULL_ORDER_MM_SUFFIX);
    }
    topic
}

pub fn push_market_topic(symbol_key: &str, mm_suffix: bool) -> String {
    let mut topic = format!("{SPOT_PUSH_MARKET_PREFIX}{}", encode_symbol_key(symbol_key));
    if mm_suffix {
        topic.push_str(SPOT_PULL_ORDER_MM_SUFFIX);
    }
    topic
}

pub fn push_no_deal_topic() -> &'static str {
    SPOT_PUSH_NO_DEAL
}

pub fn push_deeps_topic() -> &'static str {
    SPOT_PUSH_DEEPS
}

pub fn push_robot_topic() -> &'static str {
    SPOT_PUSH_ROBOT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spot_topic_names() {
        assert_eq!(pull_order_topic(), "contract_match_order");
        assert_eq!(pull_order_mm_topic(), "contract_match_order_mm");
        assert_eq!(
            push_order_topic("btcusdt", false),
            "contract_match_order_push_order_btcusdt"
        );
        assert_eq!(
            push_order_topic("btcusdt", true),
            "contract_match_order_push_order_btcusdt_mm"
        );
        assert_eq!(
            push_market_topic("btcusdt", true),
            "contract_match_market_push_order_btcusdt_mm"
        );
        assert_eq!(push_no_deal_topic(), "contract_match_market_push_no_deal");
        assert_eq!(pull_group("contract_match_group"), "contract_match_group");
    }
}
