use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use tracing::{error, info};

use crate::telemetry;

#[derive(Debug, Default)]
pub struct BootstrapReady(Arc<AtomicBool>);

impl BootstrapReady {
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub fn mark_ready(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn shared(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.0)
    }
}

pub async fn spawn_server(
    port: u16,
    ready: Arc<AtomicBool>,
) -> Result<tokio::task::JoinHandle<()>, std::io::Error> {
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    Ok(spawn_on_listener(listener, ready))
}

/// Binds `127.0.0.1:0` and returns the chosen port (for tests).
pub async fn spawn_server_ephemeral(
    ready: Arc<AtomicBool>,
) -> Result<(u16, tokio::task::JoinHandle<()>), std::io::Error> {
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await?;
    let port = listener.local_addr()?.port();
    Ok((port, spawn_on_listener(listener, ready)))
}

fn spawn_on_listener(
    listener: tokio::net::TcpListener,
    ready: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route(
            "/readyz",
            get({
                let ready = Arc::clone(&ready);
                move || readyz(ready)
            }),
        )
        .route("/metrics", get(metrics));

    log_health_listen_addr(&listener);
    tokio::spawn(serve_health(listener, app))
}

#[cfg_attr(coverage, coverage(off))]
fn log_health_listen_addr(listener: &tokio::net::TcpListener) {
    if let Ok(addr) = listener.local_addr() {
        info!(port = addr.port(), "health server listening");
    } else {
        log_health_bind_addr_missing();
    }
}

#[cfg_attr(coverage, coverage(off))]
fn log_health_bind_addr_missing() {
    error!("health server bind address unavailable");
}

#[cfg_attr(coverage, coverage(off))]
async fn serve_health(listener: tokio::net::TcpListener, app: Router) {
    if let Err(e) = axum::serve(listener, app).await {
        error!(error = %e, "health server exited");
    }
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