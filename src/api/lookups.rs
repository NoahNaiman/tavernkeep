//! `GET /spells/{slug}`, `GET /monsters/{slug}` and `GET /search`.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use super::{ApiError, AppState};
use crate::service::{Kind, Lookup};

pub async fn spell(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> Result<Response, ApiError> {
    Ok(json_response(state.service.spell(&slug).await?))
}

pub async fn monster(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> Result<Response, ApiError> {
    Ok(json_response(state.service.monster(&slug).await?))
}

#[derive(Debug, Deserialize)]
pub struct SearchParams {
    q: Option<String>,
    kind: Option<String>,
}

pub async fn search(
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
) -> Result<Response, ApiError> {
    let q = params
        .q
        .ok_or_else(|| ApiError::BadRequest("missing query parameter q".to_string()))?;
    let kind = params
        .kind
        .as_deref()
        .and_then(Kind::parse)
        .ok_or_else(|| ApiError::BadRequest("kind must be spell or monster".to_string()))?;
    Ok(json_response(state.service.search(kind, &q).await?))
}

fn json_response<T: Serialize>(lookup: Lookup<T>) -> Response {
    let cache_control = format!("public, max-age={}", lookup.max_age.as_secs());
    let mut resp = (StatusCode::OK, lookup.doc.body.clone()).into_response();
    let headers = resp.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    if let Ok(v) = HeaderValue::from_str(&cache_control) {
        headers.insert(header::CACHE_CONTROL, v);
    }
    resp
}
