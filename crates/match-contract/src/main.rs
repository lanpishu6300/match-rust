use std::path::PathBuf;
use std::sync::Arc;

use match_contract::bootstrap::{self, BootstrapError};
use match_contract::config::{load_from_path, Config, MqTransport};
use match_contract::health::{spawn_server, BootstrapReady};
use match_contract::mq::{MemoryMessageSource, MemoryOrderSink, MessageSource, OrderSink};
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config_path = std::env::var("MATCH_CONTRACT_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("crates/match-contract/config.example.yaml"));

    let config = match load_from_path(&config_path) {
        Ok(c) => c,
        Err(e) => {
            error!(error = %e, path = %config_path.display(), "failed to load config");
            std::process::exit(1);
        }
    };

    info!(
        shard = config.shard,
        transport = ?config.rocketmq.transport,
        name_server = %config.rocketmq.name_server,
        health_port = config.health.port,
        "match-contract starting"
    );

    let bootstrap_ready = BootstrapReady::new();
    let _health_task = if config.health.enabled {
        Some(spawn_server(config.health.port, bootstrap_ready.shared()))
    } else {
        None
    };

    let (sink, source) = build_transport(&config);

    let local_symbols: Option<Vec<String>> =
        std::env::var("MATCH_CONTRACT_LOCAL_SYMBOLS").ok().map(|s| {
            s.split(',')
                .map(str::trim)
                .filter(|x| !x.is_empty())
                .map(str::to_string)
                .collect()
        });

    let running = if let Some(symbols) = local_symbols {
        match bootstrap::run_local(&config, symbols, sink, source).await {
            Ok(r) => r,
            Err(e) => {
                error!(error = %e, "local bootstrap failed");
                std::process::exit(1);
            }
        }
    } else {
        match bootstrap::run(config, sink, source).await {
            Ok(r) => r,
            Err(e) => {
                error!(error = %e, "bootstrap failed");
                exit_for_bootstrap(e);
            }
        }
    };

    bootstrap_ready.mark_ready();
    info!(
        symbols = running.symbols.len(),
        "bootstrap complete; awaiting shutdown (ctrl_c)"
    );
    tokio::signal::ctrl_c()
        .await
        .expect("failed to listen for ctrl_c");
    info!("shutdown signal received");
}

fn build_transport(config: &Config) -> (Arc<dyn OrderSink>, Arc<dyn MessageSource>) {
    match config.rocketmq.transport {
        MqTransport::Memory => {
            let sink: Arc<dyn OrderSink> = if let Some(dir) = &config.rocketmq.memory_dir {
                Arc::new(MemoryOrderSink::with_out_dir(
                    PathBuf::from(dir).join("out"),
                ))
            } else {
                Arc::new(MemoryOrderSink::new())
            };
            let source = Arc::new(MemoryMessageSource::new());
            if let Some(dir) = &config.rocketmq.memory_dir {
                let in_dir = PathBuf::from(dir).join("in");
                if let Err(e) = source.load_dir(&in_dir) {
                    error!(error = %e, "memory inbound dir load failed");
                }
            }
            (sink, source)
        }
        MqTransport::Rocketmq => {
            error!(
                "RocketMQ transport is not available yet (see docs/rmq-spike.md); falling back to memory"
            );
            (
                Arc::new(MemoryOrderSink::new()),
                Arc::new(MemoryMessageSource::new()),
            )
        }
    }
}

fn exit_for_bootstrap(e: BootstrapError) -> ! {
    error!(error = %e, "exiting");
    std::process::exit(1);
}
