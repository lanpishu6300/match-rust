//! Protocol constants aligned with Java `Constants` and `BBConstants`.

/// User order.
pub const ORDER_USER: i8 = 1;
/// Robot order.
pub const ORDER_ROBOT: i8 = 2;
/// Fee user.
pub const FEE_USER: i8 = 3;
/// Internal user.
pub const INTER_USER: i8 = 4;
/// Market user.
pub const MARKET_USER: i8 = 5;
/// Other user.
pub const OTHER_USER: i8 = 6;
/// Other user variant 1.
pub const OTHER1_USER: i8 = 7;
/// Other user variant 2.
pub const OTHER2_USER: i8 = 8;

/// Valid order owner types.
pub const TYPES: &[i8] = &[
    ORDER_USER,
    ORDER_ROBOT,
    FEE_USER,
    INTER_USER,
    MARKET_USER,
    OTHER_USER,
    OTHER1_USER,
    OTHER2_USER,
];

/// Buy side.
pub const ORDER_TYPE_BUY: i8 = 1;
/// Sell side.
pub const ORDER_TYPE_SELL: i8 = 2;

/// Valid order sides.
pub const ORDER_TYPES: &[i8] = &[ORDER_TYPE_BUY, ORDER_TYPE_SELL];

/// Waiting to trade.
pub const ORDER_STATUS_WAIT: i8 = 0;
/// Fully filled.
pub const ORDER_STATUS_SUCCESS: i8 = 1;
/// Partially filled.
pub const ORDER_STATUS_SUCCESS_PART: i8 = 2;
/// Revoke requested.
pub const ORDER_STATUS_REVOKE: i8 = 3;
/// Revoke succeeded.
pub const ORDER_STATUS_REVOKE_SUCCESS: i8 = 4;

/// Order statuses accepted by the match engine.
pub const ORDER_STATUS: &[i8] = &[
    ORDER_STATUS_WAIT,
    ORDER_STATUS_SUCCESS_PART,
    ORDER_STATUS_REVOKE,
];

/// Limit / normal order form.
pub const ORDER_FORM_LIMIT: i8 = 1;
/// Market order form.
pub const ORDER_FORM_MARKET_PRICE: i8 = 2;
/// Post-only order form.
pub const ORDER_FORM_POST_ONLY: i8 = 3;
/// IOC order form.
pub const ORDER_FORM_IOC: i8 = 4;
/// FOK order form.
pub const ORDER_FORM_FOK: i8 = 5;

/// Depth snapshot size for no-deal topic.
pub const NO_DEAL_NUMBER: i32 = 20;
/// Depth snapshot size for deeps topic.
pub const DEEPS_NUMBER: i32 = 30;
/// Depth snapshot size for robot topic.
pub const ROBOT_NUMBER: i32 = 20;
/// Batch size when sending loop-match data.
pub const SEND_MAX_DATA: i32 = 10;

/// System forced liquidation close position type.
pub const CLOSE_POSITION_ORDER: i8 = 1;

/// Consumer pull topic prefix.
pub const MQ_CONSUMER_MATCH_PULL_ORDER: &str = "usdt_contract_match_order_";
/// Consumer group.
pub const MQ_CONSUMER_MATCH_PULL_GROUP: &str = "usdt_contract_match_channel_one_group";
/// Order push topic prefix after match.
pub const MQ_PRODUCER_MATCH_ORDER_PUSH_TOPIC: &str = "usdt_contract_match_order_push_order_";
/// Market push topic prefix after match.
pub const MQ_PRODUCER_MATCH_MARKET_PUSH_TOPIC: &str = "usdt_contract_match_market_push_order_";
/// No-deal depth topic prefix.
pub const MQ_PRODUCER_MATCH_MARKET_PUSH_NO_DEAL_TOPIC: &str =
    "usdt_contract_match_market_push_no_deal_";
/// Deeps topic prefix.
pub const MQ_PRODUCER_MATCH_MARKET_PUSH_DEEPS_TOPIC: &str =
    "usdt_contract_match_market_push_deeps_";
/// Robot topic.
pub const MQ_PRODUCER_MATCH_MARKET_PUSH_ROBOT_TOPIC: &str = "usdt_contract_match_market_push_robot";
/// Redis key for failed MQ sends.
pub const REDIS_SEND_MQ_ERROR_DATA_QUEUE: &str = "poc_redis_send_mq_error_data_queue";
/// Redis linked-list key.
pub const REDIS_LINK_LIST_KEY: &str = "redis_poc_link_list_key";
/// New coin market topic.
pub const MQ_MARKET_CLIENT_NEW_COIN_MARKET: &str = "usdt_market_add_new_coin";
/// New coin market consumer group.
pub const MQ_CONSUMER_MATCH_PULL_ORDER_GROUP: &str = "usdt_market_add_new_coin_group";
