//! Tavernkeep: a fast, outage-proof D&D 5e SRD lookup API.

pub mod api;
pub mod backoff;
pub mod breaker;
pub mod cache;
pub mod clock;
pub mod config;
pub mod dice;
pub mod open5e;
pub mod service;
pub mod snapshot;
pub mod telemetry;
pub mod upstream;
pub mod warm;

use std::sync::Arc;

use api::AppState;
use clock::SharedClock;
use config::Config;

/// Build the application router for `config`, using `clock` for all timing.
pub fn app(config: Config, clock: SharedClock) -> (axum::Router, AppState) {
    let state = AppState {
        service: Arc::new(service::Service::new(Arc::new(config), clock)),
        metrics: telemetry::metrics_handle(),
    };
    (api::router(state.clone()), state)
}
