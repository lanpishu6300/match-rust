use serde::Deserialize;
use std::path::Path;
use thiserror::Error;

/// Minimum spacing between handicap depth pushes per symbol (1s default).
pub const DEFAULT_DEPTH_PUSH_INTERVAL_MS: u64 = 1000;

fn default_depth_push_interval_ms() -> u64 {
    DEFAULT_DEPTH_PUSH_INTERVAL_MS
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub shard: ShardConfig,
    pub startup_delay_ms: u64,
    #[serde(default = "default_depth_push_interval_ms")]
    pub depth_push_interval_ms: u64,
    pub symbol_workers: u32,
    #[serde(default)]
    pub health: HealthConfig,
    pub rocketmq: RocketMqConfig,
    pub redis: RedisConfig,
    pub rpc: RpcConfig,
    pub r#match: MatchConfig,
    #[serde(default)]
    pub topic_split: TopicSplitConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShardConfig {
    /// `InitLoadData.SHARD` / `EntrustBO.mainStream`.
    pub main_stream: i32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HealthConfig {
    #[serde(default = "default_health_port")]
    pub port: u16,
    #[serde(default = "default_health_enabled")]
    pub enabled: bool,
}

fn default_health_port() -> u16 {
    31016
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MqTransport {
    #[default]
    Memory,
    Rocketmq,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RocketMqConfig {
    pub name_server: String,
    pub consumer_group: String,
    #[serde(default)]
    pub transport: MqTransport,
    #[serde(default)]
    pub memory_dir: Option<String>,
    /// Subscribe to `contract_match_order_mm` in addition to user topic.
    #[serde(default = "default_true")]
    pub enable_mm_consumer: bool,
}

fn default_true() -> bool {
    true
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

#[derive(Debug, Clone, Deserialize, Default)]
pub struct TopicSplitConfig {
    #[serde(default)]
    pub topic_enable: bool,
    #[serde(default)]
    pub topic_coin_markets: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MatchConfig {
    #[serde(default)]
    pub symbols_whitelist: Vec<String>,
    #[serde(default = "default_queue_capacity")]
    pub queue_capacity: usize,
}

fn default_queue_capacity() -> usize {
    10_000
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

impl TopicSplitConfig {
    /// Mirrors Java `MqSplitProperties.isOtherTopic` (`type` / `targetType` both non-user).
    pub fn uses_mm_suffix(
        &self,
        coin_market: &str,
        taker_type: i8,
        maker_type: Option<i8>,
    ) -> bool {
        if taker_type == match_protocol::SPOT_ORDER_USER {
            return false;
        }
        if maker_type == Some(match_protocol::SPOT_ORDER_USER) {
            return false;
        }
        if self.topic_enable {
            return true;
        }
        self.topic_coin_markets.iter().any(|m| m == coin_market)
    }
}

impl Config {
    pub fn filter_markets<'a>(
        &self,
        markets: &'a [crate::rpc::SpotCoinMarket],
    ) -> Vec<&'a crate::rpc::SpotCoinMarket> {
        markets
            .iter()
            .filter(|m| m.main_stream == Some(self.shard.main_stream))
            .filter(|m| {
                self.r#match.symbols_whitelist.is_empty()
                    || m.coin_market
                        .as_deref()
                        .map(|s| self.r#match.symbols_whitelist.iter().any(|w| w == s))
                        .unwrap_or(false)
            })
            .collect()
    }

    pub fn uses_mm_topic_suffix(
        &self,
        coin_market: &str,
        order_type: i8,
        target_type: Option<i8>,
    ) -> bool {
        self.topic_split
            .uses_mm_suffix(coin_market, order_type, target_type)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_queue_capacity_when_yaml_omits_field() {
        let yaml = r#"
shard:
  main_stream: 1
startup_delay_ms: 0
symbol_workers: 1
rocketmq:
  name_server: "ns"
  consumer_group: "g"
redis:
  cluster_nodes: ["127.0.0.1:6379"]
rpc:
  market_base_url: "http://m"
  order_base_url: "http://o"
match: {}
"#;
        let cfg: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.r#match.queue_capacity, 10_000);
        assert_eq!(cfg.depth_push_interval_ms, 1000);
    }

    #[test]
    fn uses_mm_topic_suffix_delegates_to_split() {
        let mut cfg = Config {
            shard: ShardConfig { main_stream: 1 },
            startup_delay_ms: 0,
            depth_push_interval_ms: 1000,
            symbol_workers: 1,
            health: HealthConfig::default(),
            rocketmq: RocketMqConfig {
                name_server: String::new(),
                consumer_group: String::new(),
                transport: MqTransport::Memory,
                memory_dir: None,
                enable_mm_consumer: true,
            },
            redis: RedisConfig {
                cluster_nodes: vec![],
                password: String::new(),
            },
            rpc: RpcConfig {
                market_base_url: String::new(),
                order_base_url: String::new(),
            },
            r#match: MatchConfig {
                symbols_whitelist: vec![],
                queue_capacity: 1,
            },
            topic_split: TopicSplitConfig {
                topic_enable: true,
                topic_coin_markets: vec![],
            },
        };
        assert!(cfg.uses_mm_topic_suffix(
            "BTC/USDT",
            match_protocol::ORDER_ROBOT,
            None
        ));
    }
}
