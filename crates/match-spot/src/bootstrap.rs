//! Bootstrap sequence porting Java `InitLoadData`.

use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::config::Config;
use crate::inbound::InboundRouter;
use crate::mq::consumer::start_shard_consumers;
use crate::mq::producer::Producer;
use crate::mq::traits::{MessageSource, OrderSink};
use crate::outbound::Outbound;
use crate::redis_store::{RedisStore, RedisStoreError};
use crate::rpc::order::{build_mq_order_spot, OrderClient};
use crate::rpc::{MarketClient, RpcError};
use crate::symbol_worker::spawn_symbol_worker;

#[derive(Debug, Error)]
pub enum BootstrapError {
    #[error("rpc: {0}")]
    Rpc(#[from] RpcError),
    #[error("redis: {0}")]
    Redis(#[from] RedisStoreError),
    #[error("mq source: {0}")]
    Source(#[from] crate::mq::SourceError),
    #[error("no markets after shard/whitelist filter")]
    NoMarkets,
    #[error("restore failed: {0}")]
    Restore(String),
}

pub struct Running {
    pub symbols: Vec<String>,
    pub router: Arc<InboundRouter>,
    pub outbound: Arc<Outbound>,
    pub workers: Vec<JoinHandle<()>>,
}

#[cfg_attr(coverage, coverage(off))]
pub async fn run(
    config: Config,
    sink: Arc<dyn OrderSink>,
    source: Arc<dyn MessageSource>,
) -> Result<Running, BootstrapError> {
    let redis = RedisStore::connect(&config.redis)?;
    run_with_redis(config, sink, source, redis).await
}

pub async fn run_with_redis(
    config: Config,
    sink: Arc<dyn OrderSink>,
    source: Arc<dyn MessageSource>,
    mut redis: RedisStore,
) -> Result<Running, BootstrapError> {
    info!(
        delay_ms = config.startup_delay_ms,
        main_stream = config.shard.main_stream,
        "match-spot bootstrap starting"
    );
    tokio::time::sleep(Duration::from_millis(config.startup_delay_ms)).await;

    let market_client = MarketClient::new(&config.rpc.market_base_url);
    let markets = market_client.fetch_markets().await?;
    let filtered = config.filter_markets(&markets);
    if filtered.is_empty() {
        return Err(BootstrapError::NoMarkets);
    }
    info!(count = filtered.len(), "markets selected for shard");

    let router = Arc::new(InboundRouter::new());

    let queue_capacity = config.r#match.queue_capacity.max(1);
    let mut pending: Vec<(String, mpsc::Receiver<match_protocol::BbOrder>)> = Vec::new();
    let mut symbols = Vec::new();

    for market in &filtered {
        let symbol_key = market
            .symbol_key
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| {
                market
                    .coin_market
                    .as_deref()
                    .filter(|cm| !cm.is_empty())
                    .map(|cm| cm.replace('/', "").to_lowercase())
            })
            .filter(|s| !s.is_empty());
        let Some(symbol_key) = symbol_key else {
            warn!("skipping market with empty symbolKey");
            continue;
        };
        let label = market_label(market, symbol_key.as_str());

        redis.wipe_depth_keys(&symbol_key)?;
        redis.reset_link_key(&symbol_key, &label)?;

        let (tx, rx) = mpsc::channel(queue_capacity);
        router.register_queue(&symbol_key, tx);
        pending.push((symbol_key.clone(), rx));
        symbols.push(symbol_key);
    }

    if symbols.is_empty() {
        return Err(BootstrapError::NoMarkets);
    }

    let outbound = Arc::new(Outbound::new(
        Producer::new(sink),
        Some(redis),
        config.depth_push_interval_ms,
        config.topic_split.clone(),
    ));

    let mut workers = Vec::new();
    for (symbol_key, rx) in pending {
        info!(symbol = %symbol_key, "symbol worker + queue ready");
        workers.push(spawn_symbol_worker(
            symbol_key,
            rx,
            Arc::clone(&outbound),
        ));
    }

    let order_client = OrderClient::new(&config.rpc.order_base_url);
    let restored = order_client
        .fetch_all_entrusts(config.shard.main_stream)
        .await?;
    info!(count = restored.len(), "restoring entrusts");
    restore_entrusts(&router, &restored)?;

    start_shard_consumers(
        source.as_ref(),
        &config.rocketmq.consumer_group,
        config.rocketmq.enable_mm_consumer,
        Arc::clone(&router),
    )?;
    info!(symbols = symbols.len(), "shard consumers started");

    Ok(Running {
        symbols,
        router,
        outbound,
        workers,
    })
}

#[cfg_attr(coverage, coverage(off))]
fn restore_entrusts(
    router: &InboundRouter,
    rows: &[crate::rpc::order::EntrustListRow],
) -> Result<(), BootstrapError> {
    for row in rows {
        let mq = build_mq_order_spot(row);
        if let Err(e) = router.handle_mq_order(&mq) {
            return Err(BootstrapError::Restore(e.to_string()));
        }
    }
    Ok(())
}

#[cfg_attr(coverage, coverage(off))]
async fn maybe_local_startup_delay(ms: u64) {
    if ms > 0 {
        tokio::time::sleep(Duration::from_millis(ms.min(100))).await;
    }
}

#[cfg_attr(coverage, coverage(off))]
fn market_label(market: &crate::rpc::market::SpotCoinMarket, derived: &str) -> String {
    market
        .symbol_key
        .as_deref()
        .unwrap_or(derived)
        .to_string()
}

pub async fn run_local(
    config: &Config,
    symbols: Vec<String>,
    sink: Arc<dyn OrderSink>,
    source: Arc<dyn MessageSource>,
) -> Result<Running, BootstrapError> {
    info!(?symbols, "local bootstrap (no rpc/redis)");
    maybe_local_startup_delay(config.startup_delay_ms).await;

    let router = Arc::new(InboundRouter::new());
    let outbound = Arc::new(Outbound::new(
        Producer::new(sink),
        None,
        config.depth_push_interval_ms,
        config.topic_split.clone(),
    ));

    let queue_capacity = config.r#match.queue_capacity.max(1);
    let mut workers = Vec::new();
    for symbol_key in &symbols {
        let (tx, rx) = mpsc::channel(queue_capacity);
        router.register_queue(symbol_key, tx);
        workers.push(spawn_symbol_worker(
            symbol_key.clone(),
            rx,
            Arc::clone(&outbound),
        ));
    }

    start_shard_consumers(
        source.as_ref(),
        &config.rocketmq.consumer_group,
        config.rocketmq.enable_mm_consumer,
        Arc::clone(&router),
    )?;

    Ok(Running {
        symbols,
        router,
        outbound,
        workers,
    })
}
