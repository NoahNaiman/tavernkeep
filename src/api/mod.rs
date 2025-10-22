//! HTTP routes.

mod attack;
pub mod error;
mod health;
mod lookups;
mod roll;

use std::sync::Arc;

use axum::Router;
use axum::middleware;
use axum::routing::{get, post};
use metrics_exporter_prometheus::PrometheusHandle;

pub use attack::{AttackRoll, ToHit, roll_attack};
pub use error::ApiError;

use crate::service::Service;

#[derive(Clone)]
pub struct AppState {
    pub service: Arc<Service>,
    pub metrics: PrometheusHandle,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/spells/{slug}", get(lookups::spell))
        .route("/monsters/{slug}", get(lookups::monster))
        .route(
            "/monsters/{slug}/attack/{action_index}",
            get(attack::attack),
        )
        .route("/search", get(lookups::search))
        .route("/roll", post(roll::roll))
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz))
        .route("/metrics", get(health::metrics))
        .layer(middleware::from_fn(crate::telemetry::track_requests))
        .with_state(state)
}
