//! A thin Open5e HTTP client: one attempt per call, outcomes classified.

use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;

/// What one successful upstream exchange produced.
#[derive(Debug, Clone, PartialEq)]
pub enum Fetched<T> {
    Found(T),
    NotFound,
}

/// Why an upstream call failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UpstreamError {
    #[error("upstream timed out")]
    Timeout,
    #[error("could not connect to upstream: {0}")]
    Connect(String),
    #[error("upstream returned HTTP {0}")]
    Status(u16),
    #[error("upstream response could not be decoded: {0}")]
    Decode(String),
    #[error("upstream request failed: {0}")]
    Other(String),
    #[error("upstream circuit breaker is open")]
    BreakerOpen { retry_after: Duration },
}

impl UpstreamError {
    /// Only 5xx, timeouts and connection errors are worth retrying.
    pub fn is_retryable(&self) -> bool {
        match self {
            UpstreamError::Timeout | UpstreamError::Connect(_) => true,
            UpstreamError::Status(s) => *s >= 500,
            _ => false,
        }
    }

    /// Label for `tk_upstream_requests_total{outcome}`.
    pub fn outcome(&self) -> &'static str {
        match self {
            UpstreamError::Timeout => "timeout",
            UpstreamError::Connect(_) => "connect_error",
            UpstreamError::Status(s) if *s >= 500 => "server_error",
            UpstreamError::Status(_) => "client_error",
            UpstreamError::Decode(_) => "decode_error",
            UpstreamError::Other(_) => "other_error",
            UpstreamError::BreakerOpen { .. } => "breaker_open",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
}

impl Client {
    pub fn new(base: impl Into<String>, timeout: Duration) -> Self {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(timeout)
            .user_agent(concat!("tavernkeep/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("static reqwest configuration is valid");
        Client {
            http,
            base: base.into().trim_end_matches('/').to_string(),
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// GET `{base}/{path}` once and decode the JSON body.
    ///
    /// `path` must already be URL-safe (slugs are validated by the caller).
    pub async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<Fetched<T>, UpstreamError> {
        let url = format!("{}/{}", self.base, path.trim_start_matches('/'));
        let started = Instant::now();
        let result = self.send(&url, query).await;
        metrics::histogram!("tk_upstream_duration_seconds").record(started.elapsed().as_secs_f64());
        let outcome = match &result {
            Ok(Fetched::Found(_)) => "ok",
            Ok(Fetched::NotFound) => "not_found",
            Err(e) => e.outcome(),
        };
        metrics::counter!("tk_upstream_requests_total", "outcome" => outcome).increment(1);
        if let Err(e) = &result {
            tracing::warn!(%url, error = %e, "upstream request failed");
        }
        result
    }

    async fn send<T: DeserializeOwned>(
        &self,
        url: &str,
        query: &[(&str, &str)],
    ) -> Result<Fetched<T>, UpstreamError> {
        let resp = self
            .http
            .get(url)
            .query(query)
            .send()
            .await
            .map_err(classify)?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(Fetched::NotFound);
        }
        if !status.is_success() {
            return Err(UpstreamError::Status(status.as_u16()));
        }
        let bytes = resp.bytes().await.map_err(classify)?;
        serde_json::from_slice(&bytes)
            .map(Fetched::Found)
            .map_err(|e| UpstreamError::Decode(e.to_string()))
    }
}

fn classify(e: reqwest::Error) -> UpstreamError {
    if e.is_timeout() {
        UpstreamError::Timeout
    } else if e.is_connect() {
        UpstreamError::Connect(e.to_string())
    } else if e.is_decode() || e.is_body() {
        UpstreamError::Decode(e.to_string())
    } else {
        UpstreamError::Other(e.to_string())
    }
}
