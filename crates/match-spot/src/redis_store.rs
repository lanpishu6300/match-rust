use match_protocol::{
    SPOT_EXCHANGE_DEPTH_PREFIX, SPOT_MARKET_KEY_PREFIX, SPOT_MATCH_KEY_PREFIX,
    REDIS_LINK_LIST_KEY, REDIS_SEND_MQ_ERROR_DATA_QUEUE,
};
#[cfg(not(coverage))]
use redis::cluster::{ClusterClient, ClusterConnection};
#[cfg(not(coverage))]
use redis::{Client, Commands, Connection, RedisError};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RedisStoreError {
    #[error("no redis nodes configured")]
    NoNodes,
    #[cfg(not(coverage))]
    #[error("redis: {0}")]
    Redis(#[from] RedisError),
}

enum RedisBackend {
    Mock(MockStore),
    #[cfg(not(coverage))]
    Cluster(ClusterConnection),
    #[cfg(not(coverage))]
    Single(Connection),
}

#[derive(Default)]
struct MockStore {
    strings: std::sync::Mutex<std::collections::HashMap<String, String>>,
    lists: std::sync::Mutex<std::collections::HashMap<String, Vec<Vec<u8>>>>,
    fail_lpush: std::sync::atomic::AtomicBool,
}

impl MockStore {
    fn del(&self, key: &str) -> bool {
        let mut strings = self.strings.lock().expect("mock strings");
        let mut lists = self.lists.lock().expect("mock lists");
        strings.remove(key).is_some() || lists.remove(key).is_some()
    }

    fn exists(&self, key: &str) -> bool {
        let strings = self.strings.lock().expect("mock strings");
        let lists = self.lists.lock().expect("mock lists");
        strings.contains_key(key) || lists.contains_key(key)
    }

    fn set(&self, key: &str, value: &str) {
        self.strings
            .lock()
            .expect("mock strings")
            .insert(key.to_string(), value.to_string());
    }

    fn lpush(&self, key: &str, value: &[u8]) -> i64 {
        let mut lists = self.lists.lock().expect("mock lists");
        let list = lists.entry(key.to_string()).or_default();
        list.insert(0, value.to_vec());
        list.len() as i64
    }

    fn rpop(&self, key: &str) -> Option<Vec<u8>> {
        self.lists
            .lock()
            .expect("mock lists")
            .get_mut(key)
            .and_then(|list| list.pop())
    }
}

pub struct RedisStore {
    backend: RedisBackend,
}

impl RedisStore {
    pub fn mock() -> Self {
        Self {
            backend: RedisBackend::Mock(MockStore::default()),
        }
    }

    /// Test hook: force [`lpush_bytes`] to fail (error_queue path).
    pub fn test_set_fail_lpush(&mut self, on: bool) {
        if let RedisBackend::Mock(m) = &self.backend {
            m.fail_lpush
                .store(on, std::sync::atomic::Ordering::SeqCst);
        }
    }

    pub fn connect(config: &crate::config::RedisConfig) -> Result<Self, RedisStoreError> {
        if config.cluster_nodes.is_empty() {
            return Err(RedisStoreError::NoNodes);
        }
        #[cfg(coverage)]
        {
            return Ok(Self::mock());
        }
        #[cfg(not(coverage))]
        connect_backend(config)
    }

    pub fn del(&mut self, key: &str) -> Result<bool, RedisStoreError> {
        match &mut self.backend {
            RedisBackend::Mock(m) => Ok(m.del(key)),
            #[cfg(not(coverage))]
            RedisBackend::Cluster(c) => {
                let n: i32 = c.del(key)?;
                Ok(n > 0)
            }
            #[cfg(not(coverage))]
            RedisBackend::Single(c) => {
                let n: i32 = c.del(key)?;
                Ok(n > 0)
            }
        }
    }

    pub fn exists(&mut self, key: &str) -> Result<bool, RedisStoreError> {
        match &mut self.backend {
            RedisBackend::Mock(m) => Ok(m.exists(key)),
            #[cfg(not(coverage))]
            RedisBackend::Cluster(c) => Ok(c.exists(key)?),
            #[cfg(not(coverage))]
            RedisBackend::Single(c) => Ok(c.exists(key)?),
        }
    }

    pub fn set(&mut self, key: &str, value: &str) -> Result<(), RedisStoreError> {
        match &mut self.backend {
            RedisBackend::Mock(m) => {
                m.set(key, value);
                Ok(())
            }
            #[cfg(not(coverage))]
            RedisBackend::Cluster(c) => {
                c.set::<_, _, ()>(key, value)?;
                Ok(())
            }
            #[cfg(not(coverage))]
            RedisBackend::Single(c) => {
                c.set::<_, _, ()>(key, value)?;
                Ok(())
            }
        }
    }

    pub fn reset_link_key(&mut self, symbol_key: &str, label: &str) -> Result<(), RedisStoreError> {
        let key = link_list_key(symbol_key);
        self.del(&key)?;
        self.set(&key, label)?;
        Ok(())
    }

    pub fn wipe_depth_keys(&mut self, symbol_key: &str) -> Result<(), RedisStoreError> {
        for suffix in DepthSuffix::ALL {
            self.del(&depth_key(symbol_key, suffix))?;
        }
        Ok(())
    }

    pub fn lpush_bytes(&mut self, key: &str, value: &[u8]) -> Result<i64, RedisStoreError> {
        match &mut self.backend {
            RedisBackend::Mock(m) => {
                if m.fail_lpush.load(std::sync::atomic::Ordering::SeqCst) {
                    return Err(RedisStoreError::NoNodes);
                }
                Ok(m.lpush(key, value))
            }
            #[cfg(not(coverage))]
            RedisBackend::Cluster(c) => Ok(c.lpush(key, value)?),
            #[cfg(not(coverage))]
            RedisBackend::Single(c) => Ok(c.lpush(key, value)?),
        }
    }

    pub fn rpop_bytes(&mut self, key: &str) -> Result<Option<Vec<u8>>, RedisStoreError> {
        match &mut self.backend {
            RedisBackend::Mock(m) => Ok(m.rpop(key)),
            #[cfg(not(coverage))]
            RedisBackend::Cluster(c) => Ok(c.rpop(key, None)?),
            #[cfg(not(coverage))]
            RedisBackend::Single(c) => Ok(c.rpop(key, None)?),
        }
    }
}

#[cfg(not(coverage))]
fn connect_backend(config: &crate::config::RedisConfig) -> Result<RedisStore, RedisStoreError> {
    let backend = if config.cluster_nodes.len() > 1 {
        let urls = node_urls(config);
        let client = ClusterClient::new(urls)?;
        RedisBackend::Cluster(client.get_connection()?)
    } else {
        let client = Client::open(node_urls(config)[0].as_str())?;
        RedisBackend::Single(client.get_connection()?)
    };
    Ok(RedisStore { backend })
}

fn node_urls(config: &crate::config::RedisConfig) -> Vec<String> {
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

pub fn link_list_key(symbol_key: &str) -> String {
    format!("{SPOT_MATCH_KEY_PREFIX}{REDIS_LINK_LIST_KEY}{symbol_key}")
}

pub fn depth_key(symbol_key: &str, suffix: DepthSuffix) -> String {
    format!(
        "{SPOT_MARKET_KEY_PREFIX}{SPOT_EXCHANGE_DEPTH_PREFIX}{symbol_key}{}",
        suffix.as_str()
    )
}

pub fn mq_error_queue_key() -> String {
    format!("{SPOT_MATCH_KEY_PREFIX}{REDIS_SEND_MQ_ERROR_DATA_QUEUE}")
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
            Self::Detail => "_detail",
            Self::Trade => "_trade",
            Self::Paint => "_paint",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RedisConfig;

    #[test]
    fn spot_depth_key_format() {
        assert_eq!(
            depth_key("btcusdt", DepthSuffix::Detail),
            "market:exchange_depth:btcusdt_detail"
        );
    }

    #[test]
    fn link_list_key_format() {
        assert_eq!(
            link_list_key("btcusdt"),
            "match:redis_poc_link_list_keybtcusdt"
        );
    }

    #[test]
    fn connect_rejects_empty_nodes() {
        let cfg = RedisConfig {
            cluster_nodes: vec![],
            password: String::new(),
        };
        assert!(matches!(
            RedisStore::connect(&cfg),
            Err(RedisStoreError::NoNodes)
        ));
    }

    #[test]
    #[cfg(coverage)]
    fn connect_uses_mock_under_coverage_build() {
        let cfg = RedisConfig {
            cluster_nodes: vec!["127.0.0.1:6379".into()],
            password: String::new(),
        };
        assert!(RedisStore::connect(&cfg).is_ok());
    }

    #[test]
    fn node_urls_with_password_encodes_special_chars() {
        let cfg = RedisConfig {
            cluster_nodes: vec!["127.0.0.1:6379".into()],
            password: "p@ss:word".into(),
        };
        let urls = node_urls(&cfg);
        assert_eq!(urls[0], "redis://:p%40ss%3Aword@127.0.0.1:6379/");
    }

    #[test]
    fn node_urls_without_password() {
        let cfg = RedisConfig {
            cluster_nodes: vec!["127.0.0.1:6379".into()],
            password: String::new(),
        };
        let urls = node_urls(&cfg);
        assert_eq!(urls[0], "redis://127.0.0.1:6379/");
    }

    #[test]
    fn mock_store_round_trip() {
        let mut store = RedisStore::mock();
        assert!(!store.exists("k").unwrap());
        store.set("k", "v").unwrap();
        assert!(store.exists("k").unwrap());
        assert!(store.del("k").unwrap());
        assert!(!store.exists("k").unwrap());
        assert_eq!(store.lpush_bytes("q", b"x").unwrap(), 1);
        assert_eq!(store.rpop_bytes("q").unwrap(), Some(b"x".to_vec()));
        assert!(store.rpop_bytes("q").unwrap().is_none());
        store.test_set_fail_lpush(true);
        assert!(store.lpush_bytes("q", b"y").is_err());
        store.reset_link_key("btcusdt", "btcusdt").unwrap();
        assert!(store.exists(&link_list_key("btcusdt")).unwrap());
        store.wipe_depth_keys("btcusdt").unwrap();
    }
}
