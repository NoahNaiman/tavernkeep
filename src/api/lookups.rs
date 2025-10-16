//! `GET /spells/{slug}`, `GET /monsters/{slug}` and `GET /search`.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use super::{ApiError, AppState};
use crate::service::{Kind, Lookup};

pub async fn spell(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    Ok(json_response(&headers, state.service.spell(&slug).await?))
}

pub async fn monster(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    Ok(json_response(&headers, state.service.monster(&slug).await?))
}

#[derive(Debug, Deserialize)]
pub struct SearchParams {
    q: Option<String>,
    kind: Option<String>,
}

pub async fn search(
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let q = params
        .q
        .ok_or_else(|| ApiError::BadRequest("missing query parameter q".to_string()))?;
    let kind = params
        .kind
        .as_deref()
        .and_then(Kind::parse)
        .ok_or_else(|| ApiError::BadRequest("kind must be spell or monster".to_string()))?;
    Ok(json_response(
        &headers,
        state.service.search(kind, &q).await?,
    ))
}

/// Whether an `If-None-Match` header value matches `etag` (weak comparison,
/// as RFC 9110 requires for `If-None-Match`).
pub fn etag_matches(if_none_match: &str, etag: &str) -> bool {
    let bare = |t: &str| t.trim().trim_start_matches("W/").to_string();
    let ours = bare(etag);
    if_none_match
        .split(',')
        .any(|candidate| candidate.trim() == "*" || bare(candidate) == ours)
}

fn json_response<T>(req_headers: &HeaderMap, lookup: Lookup<T>) -> Response {
    let cache_control = format!("public, max-age={}", lookup.max_age.as_secs());
    let not_modified = req_headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| etag_matches(v, &lookup.doc.etag));
    let mut resp = if not_modified {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        let mut r = (StatusCode::OK, lookup.doc.body.clone()).into_response();
        r.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        r
    };
    let headers = resp.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&lookup.doc.etag) {
        headers.insert(header::ETAG, v);
    }
    if let Ok(v) = HeaderValue::from_str(&cache_control) {
        headers.insert(header::CACHE_CONTROL, v);
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::etag_matches;

    #[test]
    fn if_none_match_parsing() {
        let tag = "\"abc\"";
        assert!(etag_matches("\"abc\"", tag));
        assert!(etag_matches("W/\"abc\"", tag));
        assert!(etag_matches("\"x\", \"abc\"", tag));
        assert!(etag_matches("*", tag));
        assert!(!etag_matches("\"abd\"", tag));
        assert!(!etag_matches("abc", tag));
    }
}
