//! Spot protocol constants aligned with bf-match `Constants` and `BBConstants`.

/// User order.
pub const SPOT_ORDER_USER: i8 = 1;
/// Robot order.
pub const SPOT_ORDER_ROBOT: i8 = 2;
/// Market user order.
pub const SPOT_ORDER_MARKET_USER: i8 = 3;

/// Valid spot order owner types (`Constants.TYPES`).
pub const SPOT_TYPES: &[i8] = &[SPOT_ORDER_USER, SPOT_ORDER_ROBOT, SPOT_ORDER_MARKET_USER];

/// Limit order form.
pub const SPOT_ORDER_FORM_LIMIT: i8 = 1;
/// Market order form.
pub const SPOT_ORDER_FORM_MARKET_PRICE: i8 = 2;

/// Spot accepts limit and market only (PostOnly/IOC/FOK are contract-only).
pub const SPOT_ORDER_FORMS: &[i8] = &[SPOT_ORDER_FORM_LIMIT, SPOT_ORDER_FORM_MARKET_PRICE];

/// Depth snapshot size for no-deal topic (`Constants.NO_DEAL_NUMBER`).
pub const SPOT_NO_DEAL_NUMBER: i32 = 25;
/// Depth snapshot size for deeps topic.
pub const SPOT_DEEPS_NUMBER: i32 = 30;
/// Depth snapshot size for robot topic.
pub const SPOT_ROBOT_NUMBER: i32 = 50;
/// Batch size when sending loop-match data (`Constants.SEND_MAX_DATA`).
pub const SPOT_SEND_MAX_DATA: i32 = 1;

pub use crate::constants::{ORDER_STATUS, ORDER_TYPES};

/// Redis key prefix enums mirrored from Java (`RedisKeyPrefixEnum`).
pub const SPOT_MATCH_KEY_PREFIX: &str = "match:";
pub const SPOT_MARKET_KEY_PREFIX: &str = "market:";
pub const SPOT_EXCHANGE_DEPTH_PREFIX: &str = "exchange_depth:";
