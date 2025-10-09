//! The JSON error type every handler returns.

use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApiError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{message}")]
    Unprocessable {
        message: String,
        position: Option<usize>,
    },
    #[error("{message}")]
    Unavailable { message: String, retry_after: u64 },
    #[error("{0}")]
    BadGateway(String),
}

#[derive(Serialize)]
struct Body<'a> {
    error: &'a str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    position: Option<usize>,
}

impl ApiError {
    pub fn status(&self) -> StatusCode {
        match self {
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::NotFound(_) => StatusCode::NOT_FOUND,
            ApiError::Unprocessable { .. } => StatusCode::UNPROCESSABLE_ENTITY,
            ApiError::Unavailable { .. } => StatusCode::SERVICE_UNAVAILABLE,
            ApiError::BadGateway(_) => StatusCode::BAD_GATEWAY,
        }
    }

    fn code(&self) -> &'static str {
        match self {
            ApiError::BadRequest(_) => "bad_request",
            ApiError::NotFound(_) => "not_found",
            ApiError::Unprocessable { .. } => "unprocessable",
            ApiError::Unavailable { .. } => "unavailable",
            ApiError::BadGateway(_) => "bad_gateway",
        }
    }
}

impl From<crate::dice::DiceError> for ApiError {
    fn from(e: crate::dice::DiceError) -> Self {
        ApiError::Unprocessable {
            message: e.to_string(),
            position: Some(e.position),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let position = match &self {
            ApiError::Unprocessable { position, .. } => *position,
            _ => None,
        };
        let body = Body {
            error: self.code(),
            message: self.to_string(),
            position,
        };
        let mut resp = (self.status(), Json(body)).into_response();
        if let ApiError::Unavailable { retry_after, .. } = self {
            resp.headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from(retry_after.max(1)));
        }
        resp
    }
}
