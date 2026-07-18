//! Bootstrap sequence porting Java `InitLoadData.initMain`.

use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::config::Config;
use crate::inbound::{InboundRouter, StartQueueState};
use crate::mq::consumer::start_consumers;
use crate::mq::producer::Producer;
use crate::mq::traits::{MessageSource, OrderSink};
use crate::outbound::Outbound;
use crate::redis_store::{normalize_symbol_key, RedisStore, RedisStoreError};
use crate::rpc::order::{build_mq_order, OrderClient};
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
}

/// Handles kept alive for the process lifetime.
pub struct Running {
    pub symbols: Vec<String>,
    pub start_queue: Arc<StartQueueState>,
    pub router: Arc<InboundRouter>,
    pub outbound: Arc<Outbound>,
    pub workers: Vec<JoinHandle<()>>,
    _ttl_task: JoinHandle<()>,
}

/// Run the InitLoadData-equivalent bootstrap, then return running handles.
///
/// Sequence:
/// 1. `startup_delay_ms` sleep
/// 2. fetch markets via RPC, filter shard + whitelist
/// 3. per symbol: redis wipe depth + link key; create channel; spawn worker
/// 4. restore entrusts via RPC → inbound restore handle
/// 5. start consumers (real or stub `MessageSource`)
/// 6. after `start_queue_ttl_ms` clear START_QUEUE / BigNo
pub async fn run(
    config: Config,
    sink: Arc<dyn OrderSink>,
    source: Arc<dyn MessageSource>,
) -> Result<Running, BootstrapError> {
    info!(
        delay_ms = config.startup_delay_ms,
        transport = ?config.rocketmq.transport,
        "match-contract bootstrap starting"
    );
    tokio::time::sleep(Duration::from_millis(config.startup_delay_ms)).await;

    let market_client = MarketClient::new(&config.rpc.market_base_url);
    let markets = market_client.fetch_markets().await?;
    let filtered = config.filter_markets(&markets);
    if filtered.is_empty() {
        return Err(BootstrapError::NoMarkets);
    }
    info!(count = filtered.len(), "markets selected for shard");

    let mut redis = RedisStore::connect(&config.redis)?;
    let start_queue = Arc::new(StartQueueState::new());
    let router = Arc::new(InboundRouter::new(Arc::clone(&start_queue)));

    let mut pending: Vec<(String, mpsc::UnboundedReceiver<match_protocol::BbOrder>)> = Vec::new();
    let mut symbols = Vec::new();

    for market in &filtered {
        let coin_market = market.coin_market.as_deref().unwrap_or_default();
        let symbol_key = normalize_symbol_key(coin_market);
        if symbol_key.is_empty() {
            warn!("skipping market with empty coinMarket");
            continue;
        }
        let origin = market.origin_coin_market.as_deref().unwrap_or(coin_market);

        redis.wipe_depth_keys(origin)?;
        redis.reset_link_key(&symbol_key, coin_market)?;

        let (tx, rx) = mpsc::unbounded_channel();
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
    ));

    let mut workers = Vec::new();
    for (symbol_key, rx) in pending {
        info!(symbol = %symbol_key, "symbol worker + queue ready");
        workers.push(spawn_symbol_worker(symbol_key, rx, Arc::clone(&outbound)));
    }

    let order_client = OrderClient::new(&config.rpc.order_base_url);
    let restored = order_client.fetch_all_entrusts("0", config.shard).await?;
    info!(count = restored.len(), "restoring entrusts");
    for row in &restored {
        let mq = build_mq_order(row);
        if let Err(e) = router.handle_restore_order(&mq) {
            warn!(error = %e, "restore enqueue failed");
        }
    }

    start_consumers(source.as_ref(), &symbols, Arc::clone(&router))?;
    info!(symbols = symbols.len(), "consumers started");

    let ttl = config.start_queue_ttl_ms;
    let start_queue_ttl = Arc::clone(&start_queue);
    let ttl_task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(ttl)).await;
        start_queue_ttl.clear();
        info!("START_QUEUE / BigNo cleared after ttl");
    });

    Ok(Running {
        symbols,
        start_queue,
        router,
        outbound,
        workers,
        _ttl_task: ttl_task,
    })
}

/// Local / test bootstrap that skips RPC + Redis (memory transport only).
pub async fn run_local(
    config: &Config,
    symbols: Vec<String>,
    sink: Arc<dyn OrderSink>,
    source: Arc<dyn MessageSource>,
) -> Result<Running, BootstrapError> {
    info!(?symbols, "local bootstrap (no rpc/redis)");
    if config.startup_delay_ms > 0 {
        tokio::time::sleep(Duration::from_millis(config.startup_delay_ms.min(100))).await;
    }

    let start_queue = Arc::new(StartQueueState::new());
    let router = Arc::new(InboundRouter::new(Arc::clone(&start_queue)));
    let outbound = Arc::new(Outbound::new(
        Producer::new(sink),
        None,
        config.depth_push_interval_ms,
    ));

    let mut workers = Vec::new();
    for symbol_key in &symbols {
        let (tx, rx) = mpsc::unbounded_channel();
        router.register_queue(symbol_key, tx);
        workers.push(spawn_symbol_worker(
            symbol_key.clone(),
            rx,
            Arc::clone(&outbound),
        ));
    }

    start_consumers(source.as_ref(), &symbols, Arc::clone(&router))?;

    let ttl = config.start_queue_ttl_ms;
    let start_queue_ttl = Arc::clone(&start_queue);
    let ttl_task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(ttl)).await;
        start_queue_ttl.clear();
        info!("START_QUEUE / BigNo cleared after ttl");
    });

    Ok(Running {
        symbols,
        start_queue,
        router,
        outbound,
        workers,
        _ttl_task: ttl_task,
    })
}
