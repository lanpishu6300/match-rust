//! RocketMQ topic / group names for spot (`bf-match` `BBConstants`).

pub const SPOT_PULL_ORDER: &str = "contract_match_order";
pub const SPOT_PULL_ORDER_MM_SUFFIX: &str = "_mm";
pub const SPOT_PULL_GROUP: &str = "contract_match_group";
pub const SPOT_PULL_GROUP_MM_SUFFIX: &str = "_mm";

pub const SPOT_PUSH_ORDER_PREFIX: &str = "contract_match_order_push_order_";
pub const SPOT_PUSH_MARKET_PREFIX: &str = "contract_match_market_push_order_";
pub const SPOT_PUSH_ENTRUST_PREFIX: &str = "contract_match_market_entrust_order_";
pub const SPOT_PUSH_NO_DEAL: &str = "contract_match_market_push_no_deal";
pub const SPOT_PUSH_DEEPS: &str = "contract_match_market_push_deeps";
pub const SPOT_PUSH_ROBOT: &str = "contract_match_market_push_robot";

pub const SPOT_NEW_COIN: &str = "market_client_new_coin_market";
pub const SPOT_NEW_COIN_GROUP: &str = "market_client_new_coin_market_group";
