//! HTTP routes.

pub mod error;
mod roll;

use axum::Router;
use axum::routing::post;

pub use error::ApiError;

pub fn router() -> Router {
    Router::new().route("/roll", post(roll::roll))
}
