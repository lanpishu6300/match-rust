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
    #[serde(default)]
    pub health: HealthConfig,
    pub rocketmq: RocketMqConfig,
    pub redis: RedisConfig,
    pub rpc: RpcConfig,
    pub r#match: MatchConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HealthConfig {
    /// HTTP port for `/healthz`, `/readyz`, `/metrics` (Java `server.port` = 31015).
    #[serde(default = "default_health_port")]
    pub port: u16,
    #[serde(default = "default_health_enabled")]
    pub enabled: bool,
}

fn default_health_port() -> u16 {
    31015
}

fn default_health_enabled() -> bool {
    true
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            port: default_health_port(),
            enabled: default_health_enabled(),
        }
    }
}

/// MQ transport backend. Production RocketMQ is blocked pending client compatibility;
/// default to `memory` for local runs (see `docs/rmq-spike.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MqTransport {
    #[default]
    Memory,
    /// Reserved — not wired until NameServer spike passes.
    Rocketmq,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RocketMqConfig {
    pub name_server: String,
    pub consumer_group: String,
    #[serde(default)]
    pub transport: MqTransport,
    /// Optional directory for memory file-channel (`out/` writes, `in/` reads).
    #[serde(default)]
    pub memory_dir: Option<String>,
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
    /// Per-symbol inbound queue capacity (bounded backpressure).
    #[serde(default = "default_queue_capacity")]
    pub queue_capacity: usize,
    /// Default fixed-point scales for hp-engine (overridable per symbol).
    #[serde(default = "default_price_scale")]
    pub default_price_scale: u32,
    #[serde(default = "default_qty_scale")]
    pub default_qty_scale: u32,
    #[serde(default)]
    pub symbol_scales: std::collections::HashMap<String, SymbolScaleConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SymbolScaleConfig {
    pub price_scale: u32,
    pub qty_scale: u32,
}

fn default_queue_capacity() -> usize {
    10_000
}

fn default_price_scale() -> u32 {
    2
}

fn default_qty_scale() -> u32 {
    6
}

impl MatchConfig {
    pub fn scale_for(&self, symbol_key: &str) -> (u32, u32) {
        if let Some(s) = self.symbol_scales.get(symbol_key) {
            (s.price_scale, s.qty_scale)
        } else {
            (self.default_price_scale, self.default_qty_scale)
        }
    }
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
