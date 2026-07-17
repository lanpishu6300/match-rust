use serde::Deserialize;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub shard: i32,
    pub startup_delay_ms: u64,
    pub start_queue_ttl_ms: u64,
    pub depth_push_interval_ms: u64,
    pub symbol_workers: u32,
    pub rocketmq: RocketMqConfig,
    pub redis: RedisConfig,
    pub rpc: RpcConfig,
    pub r#match: MatchConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RocketMqConfig {
    pub name_server: String,
    pub consumer_group: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RedisConfig {
    pub cluster_nodes: Vec<String>,
    #[serde(default)]
    pub password: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RpcConfig {
    pub market_base_url: String,
    pub order_base_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MatchConfig {
    #[serde(default)]
    pub symbols_whitelist: Vec<String>,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config file: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse config YAML: {0}")]
    Parse(#[from] serde_yaml::Error),
}

pub fn load_from_path(path: impl AsRef<Path>) -> Result<Config, ConfigError> {
    let content = std::fs::read_to_string(path)?;
    Ok(serde_yaml::from_str(&content)?)
}

impl Config {
    /// Markets on this shard, optionally restricted by whitelist (empty = all).
    pub fn filter_markets<'a>(
        &self,
        markets: &'a [crate::rpc::ContractCoinMarket],
    ) -> Vec<&'a crate::rpc::ContractCoinMarket> {
        markets
            .iter()
            .filter(|m| m.main_stream == Some(self.shard))
            .filter(|m| {
                self.r#match.symbols_whitelist.is_empty()
                    || m.coin_market
                        .as_deref()
                        .map(|s| self.r#match.symbols_whitelist.iter().any(|w| w == s))
                        .unwrap_or(false)
            })
            .collect()
    }
}
