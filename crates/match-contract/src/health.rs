//! HTTP health and metrics endpoints (`/healthz`, `/readyz`, `/metrics`).

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use tracing::{error, info};

use crate::telemetry;

/// Set to `true` after bootstrap completes (RPC restore, workers, consumers).
#[derive(Debug, Default)]
pub struct BootstrapReady(Arc<AtomicBool>);

impl BootstrapReady {
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub fn mark_ready(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_ready(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    pub fn shared(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.0)
    }
}

/// Spawn the health/metrics HTTP server. Returns a join handle for graceful shutdown wiring.
pub fn spawn_server(port: u16, ready: Arc<AtomicBool>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let app = Router::new()
            .route("/healthz", get(healthz))
            .route(
                "/readyz",
                get({
                    let ready = Arc::clone(&ready);
                    move || readyz(ready)
                }),
            )
            .route("/metrics", get(metrics));

        let addr = SocketAddr::from(([0, 0, 0, 0], port));
        let listener = match tokio::net::TcpListener::bind(addr).await {
            Ok(l) => l,
            Err(e) => {
                error!(error = %e, port, "health server bind failed");
                return;
            }
        };
        info!(port, "health server listening");
        if let Err(e) = axum::serve(listener, app).await {
            error!(error = %e, "health server exited");
        }
    })
}

async fn healthz() -> &'static str {
    "ok"
}

async fn readyz(ready: Arc<AtomicBool>) -> Response {
    if ready.load(Ordering::SeqCst) {
        (StatusCode::OK, "ready").into_response()
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "not ready").into_response()
    }
}

async fn metrics() -> (StatusCode, String) {
    (StatusCode::OK, telemetry::render_prometheus())
}
