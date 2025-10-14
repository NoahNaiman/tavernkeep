//! Logging and Prometheus metrics setup.

use std::sync::OnceLock;
use std::time::Instant;

use axum::extract::{MatchedPath, Request};
use axum::middleware::Next;
use axum::response::Response;
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};

static HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

/// Install the global Prometheus recorder once and return its handle.
pub fn metrics_handle() -> PrometheusHandle {
    HANDLE
        .get_or_init(|| {
            PrometheusBuilder::new()
                .set_buckets_for_metric(
                    Matcher::Full("tk_upstream_duration_seconds".to_string()),
                    &[0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0],
                )
                .expect("bucket list is non-empty")
                .install_recorder()
                .expect("metrics recorder is installed once")
        })
        .clone()
}

/// JSON logs to stdout, filtered by `TK_LOG` (default `info`).
pub fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_env("TK_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .json()
        .with_env_filter(filter)
        .with_current_span(false)
        .try_init();
}

/// Middleware recording `tk_requests_total{route,status}`.
pub async fn track_requests(req: Request, next: Next) -> Response {
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| "unmatched".to_string());
    let method = req.method().clone();
    let started = Instant::now();
    let resp = next.run(req).await;
    let status = resp.status().as_u16().to_string();
    tracing::debug!(
        %method,
        route = %route,
        status = %status,
        elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
        "request"
    );
    metrics::counter!("tk_requests_total", "route" => route, "status" => status).increment(1);
    resp
}
