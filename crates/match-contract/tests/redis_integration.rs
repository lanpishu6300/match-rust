//! Live Redis integration tests (optional).
//!
//! Run with: `cargo test -p match-contract --test redis_integration -- --ignored`

use match_contract::config::RedisConfig;
use match_contract::error_queue::ErrorQueue;
use match_contract::redis_store::{link_list_key, RedisStore};

fn test_config() -> RedisConfig {
    RedisConfig {
        cluster_nodes: vec![
            std::env::var("MATCH_CONTRACT_REDIS").unwrap_or_else(|_| "127.0.0.1:6379".into())
        ],
        password: std::env::var("MATCH_CONTRACT_REDIS_PASSWORD").unwrap_or_default(),
    }
}

#[test]
#[ignore = "requires live Redis; set MATCH_CONTRACT_REDIS=host:port"]
fn link_key_exists_set_del_roundtrip() {
    let mut store = RedisStore::connect(&test_config()).expect("connect redis");
    let key = link_list_key("itestbtcusdt");

    let _ = store.del(&key);
    assert!(!store.exists(&key).unwrap());

    store.set(&key, "BTC/USDT").unwrap();
    assert!(store.exists(&key).unwrap());

    store.del(&key).unwrap();
    assert!(!store.exists(&key).unwrap());
}

#[test]
#[ignore = "requires live Redis; set MATCH_CONTRACT_REDIS=host:port"]
fn error_queue_push_pop_roundtrip() {
    let mut store = RedisStore::connect(&test_config()).expect("connect redis");
    let mut queue = ErrorQueue::new(&mut store);

    let payload = b"integration-test-payload";
    queue.push_raw(payload).unwrap();
    let popped = queue.pop_raw().unwrap().expect("payload");
    assert_eq!(popped, payload);
}
