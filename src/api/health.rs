//! `GET /healthz`, `GET /readyz` and `GET /metrics`.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde_json::json;

use super::AppState;

pub async fn healthz() -> impl IntoResponse {
    Json(json!({"status": "ok"}))
}

/// Ready when Open5e answered recently or the cache can serve something.
pub async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    let upstream = state.service.upstream_recently_reachable();
    let cached = state.service.cached_entries().await;
    let ready = upstream || cached > 0;
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(json!({
            "status": if ready { "ready" } else { "not_ready" },
            "upstream_reachable": upstream,
            "cached_entries": cached,
        })),
    )
}

pub async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    (
        [("content-type", "text/plain; version=0.0.4")],
        state.metrics.render(),
    )
}
