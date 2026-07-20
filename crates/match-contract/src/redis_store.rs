//! Redis key construction and store operations aligned with Java `java-contract-match`.
//!
//! # Key format (Java `RedisKey` = prefix + suffix)
//!
//! Source: `java-framework/cache/.../RedisKeyPrefixEnum.java`, `BBConstants.java`,
//! `InitLoadData.java`, `AddCoinMarketConsumer.java`.
//!
//! | Purpose | Full Redis key |
//! |---------|----------------|
//! | Match link (per symbol) | `match:redis_poc_link_list_key{symbolKey}` |
//! | Depth detail wipe | `market:contract_exchange_depth:{originCoinMarket}detail` |
//! | Depth trade wipe | `market:contract_exchange_depth:{originCoinMarket}trade` |
//! | Depth paint wipe | `market:contract_exchange_depth:{originCoinMarket}paint` |
//! | MQ send-failure queue | `match:poc_redis_send_mq_error_data_queue` |
//!
//! - `symbolKey` = `coinMarket.replace("/", "").toLowerCase()` (e.g. `btcusdt`).
//! - `originCoinMarket` = same normalization on `originCoinMarket` field.
//! - Prefixes: `MATCH_KEY` → `"match:"`, `MARKET_KEY` → `"market:"`.
//! - Link suffix is `REDIS_LINK_LIST_KEY` (`redis_poc_link_list_key`) concatenated **without**
//!   a separator before `symbolKey` (Java: `REDIS_LINK_LIST_KEY + symbolKey`).
//!
//! Java stores link-key values and error-queue list elements with Kryo (`SerializerFactory`).
//! `set_link_value` writes UTF-8 bytes for the coin-market string; only key existence is
//! checked at runtime (`exists`), matching operational use in `InitLoadData`.

use crate::config::RedisConfig;
use match_protocol::{REDIS_LINK_LIST_KEY, REDIS_SEND_MQ_ERROR_DATA_QUEUE};
use redis::cluster::{ClusterClient, ClusterConnection};
use redis::{Client, Commands, Connection, RedisError};
use thiserror::Error;

/// Java `RedisKeyPrefixEnum.MATCH_KEY`.
pub const MATCH_KEY_PREFIX: &str = "match:";
/// Java `RedisKeyPrefixEnum.MARKET_KEY`.
pub const MARKET_KEY_PREFIX: &str = "market:";
/// Depth key body prefix (before origin + suffix).
pub const CONTRACT_EXCHANGE_DEPTH_PREFIX: &str = "contract_exchange_depth:";

#[derive(Debug, Error)]
pub enum RedisStoreError {
    #[error("redis: {0}")]
    Redis(#[from] RedisError),
    #[error("no redis nodes configured")]
    NoNodes,
}

enum RedisBackend {
    Cluster(ClusterConnection),
    Single(Connection),
}

/// Blocking Redis client (cluster when `cluster_nodes.len() > 1`, else standalone).
pub struct RedisStore {
    backend: RedisBackend,
}

impl RedisStore {
    pub fn connect(config: &RedisConfig) -> Result<Self, RedisStoreError> {
        if config.cluster_nodes.is_empty() {
            return Err(RedisStoreError::NoNodes);
        }

        let backend = if config.cluster_nodes.len() > 1 {
            let urls = node_urls(config);
            let client = ClusterClient::new(urls)?;
            RedisBackend::Cluster(client.get_connection()?)
        } else {
            let client = Client::open(node_urls(config)[0].as_str())?;
            RedisBackend::Single(client.get_connection()?)
        };

        Ok(Self { backend })
    }

    pub fn del(&mut self, key: &str) -> Result<bool, RedisStoreError> {
        let deleted: i32 = match &mut self.backend {
            RedisBackend::Cluster(c) => c.del(key)?,
            RedisBackend::Single(c) => c.del(key)?,
        };
        Ok(deleted > 0)
    }

    pub fn exists(&mut self, key: &str) -> Result<bool, RedisStoreError> {
        let exists: bool = match &mut self.backend {
            RedisBackend::Cluster(c) => c.exists(key)?,
            RedisBackend::Single(c) => c.exists(key)?,
        };
        Ok(exists)
    }

    /// Set a string value (link key stores coin-market label; Java Kryo-encodes the same string).
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), RedisStoreError> {
        match &mut self.backend {
            RedisBackend::Cluster(c) => {
                c.set::<_, _, ()>(key, value)?;
            }
            RedisBackend::Single(c) => {
                c.set::<_, _, ()>(key, value)?;
            }
        }
        Ok(())
    }

    /// Delete link key, then optionally set if absent — mirrors `InitLoadData` startup sequence.
    pub fn reset_link_key(
        &mut self,
        symbol_key: &str,
        coin_market: &str,
    ) -> Result<(), RedisStoreError> {
        let key = link_list_key(symbol_key);
        self.del(&key)?;
        if !self.exists(&key)? {
            self.set(&key, coin_market)?;
        }
        Ok(())
    }

    /// Wipe the three depth keys for a symbol (`detail`, `trade`, `paint`).
    pub fn wipe_depth_keys(&mut self, origin_coin_market: &str) -> Result<(), RedisStoreError> {
        for suffix in DepthSuffix::ALL {
            self.del(&depth_key(origin_coin_market, suffix))?;
        }
        Ok(())
    }

    pub fn lpush_bytes(&mut self, key: &str, value: &[u8]) -> Result<i64, RedisStoreError> {
        let len: i64 = match &mut self.backend {
            RedisBackend::Cluster(c) => c.lpush(key, value)?,
            RedisBackend::Single(c) => c.lpush(key, value)?,
        };
        Ok(len)
    }

    pub fn rpop_bytes(&mut self, key: &str) -> Result<Option<Vec<u8>>, RedisStoreError> {
        let value: Option<Vec<u8>> = match &mut self.backend {
            RedisBackend::Cluster(c) => c.rpop(key, None)?,
            RedisBackend::Single(c) => c.rpop(key, None)?,
        };
        Ok(value)
    }
}

fn node_urls(config: &RedisConfig) -> Vec<String> {
    config
        .cluster_nodes
        .iter()
        .map(|node| {
            if config.password.is_empty() {
                format!("redis://{node}/")
            } else {
                let password = urlencoding_encode(&config.password);
                format!("redis://:{password}@{node}/")
            }
        })
        .collect()
}

/// Minimal URL-encode for Redis password authority (encode `@`, `:`, `/`, `%`, space).
fn urlencoding_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push('%');
                out.push(char::from(b"0123456789ABCDEF"[(b >> 4) as usize]));
                out.push(char::from(b"0123456789ABCDEF"[(b & 0xf) as usize]));
            }
        }
    }
    out
}

/// `coinMarket.replace("/", "").toLowerCase()`.
pub fn normalize_symbol_key(coin_market: &str) -> String {
    coin_market.replace('/', "").to_lowercase()
}

/// `match:redis_poc_link_list_key{symbolKey}`.
pub fn link_list_key(symbol_key: &str) -> String {
    format!("{MATCH_KEY_PREFIX}{REDIS_LINK_LIST_KEY}{symbol_key}")
}

/// `market:contract_exchange_depth:{origin}{suffix}`.
pub fn depth_key(origin_coin_market: &str, suffix: DepthSuffix) -> String {
    let origin = normalize_symbol_key(origin_coin_market);
    format!(
        "{MARKET_KEY_PREFIX}{CONTRACT_EXCHANGE_DEPTH_PREFIX}{origin}{}",
        suffix.as_str()
    )
}

/// `match:poc_redis_send_mq_error_data_queue`.
pub fn mq_error_queue_key() -> String {
    format!("{MATCH_KEY_PREFIX}{REDIS_SEND_MQ_ERROR_DATA_QUEUE}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepthSuffix {
    Detail,
    Trade,
    Paint,
}

impl DepthSuffix {
    pub const ALL: [DepthSuffix; 3] = [Self::Detail, Self::Trade, Self::Paint];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Detail => "detail",
            Self::Trade => "trade",
            Self::Paint => "paint",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_list_key_format() {
        assert_eq!(
            link_list_key("btcusdt"),
            "match:redis_poc_link_list_keybtcusdt"
        );
    }

    #[test]
    fn depth_keys_use_market_prefix_and_origin() {
        assert_eq!(
            depth_key("BTC/USDT", DepthSuffix::Detail),
            "market:contract_exchange_depth:btcusdtdetail"
        );
        assert_eq!(
            depth_key("ethusdt", DepthSuffix::Trade),
            "market:contract_exchange_depth:ethusdttrade"
        );
        assert_eq!(
            depth_key("ETH/USDT", DepthSuffix::Paint),
            "market:contract_exchange_depth:ethusdtpaint"
        );
    }

    #[test]
    fn mq_error_queue_key_format() {
        assert_eq!(
            mq_error_queue_key(),
            "match:poc_redis_send_mq_error_data_queue"
        );
    }

    #[test]
    fn normalize_symbol_key_strips_slash_and_lowercases() {
        assert_eq!(normalize_symbol_key("BTC/USDT"), "btcusdt");
        assert_eq!(normalize_symbol_key("ethusdt"), "ethusdt");
    }
}
