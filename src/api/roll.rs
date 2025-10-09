//! `POST /roll`.

use axum::Json;
use axum::extract::rejection::JsonRejection;
use serde::Deserialize;

use super::error::ApiError;
use crate::dice::{self, RollOutcome};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RollRequest {
    pub expr: String,
    pub seed: Option<u64>,
}

pub async fn roll(
    body: Result<Json<RollRequest>, JsonRejection>,
) -> Result<Json<RollOutcome>, ApiError> {
    let Json(req) = body.map_err(|e| ApiError::BadRequest(e.body_text()))?;
    let outcome = dice::roll(&req.expr, req.seed)?;
    metrics::counter!("tk_rolls_total").increment(1);
    Ok(Json(outcome))
}
