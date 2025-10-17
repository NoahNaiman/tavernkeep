//! A thin Open5e HTTP client: one attempt per call, outcomes classified.

use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;

/// What one successful upstream exchange produced.
#[derive(Debug, Clone, PartialEq)]
pub enum Fetched<T> {
    Found(T),
    NotFound,
}

impl<T> Fetched<T> {
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Fetched<U> {
        match self {
            Fetched::Found(t) => Fetched::Found(f(t)),
            Fetched::NotFound => Fetched::NotFound,
        }
    }
}

/// Why an upstream call failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UpstreamError {
    #[error("upstream timed out")]
    Timeout,
    #[error("could not connect to upstream: {0}")]
    Connect(String),
    #[error("upstream connection failed mid-response: {0}")]
    Transport(String),
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
            UpstreamError::Timeout | UpstreamError::Connect(_) | UpstreamError::Transport(_) => {
                true
            }
            UpstreamError::Status(s) => *s >= 500,
            _ => false,
        }
    }

    /// Label for `tk_upstream_requests_total{outcome}`.
    pub fn outcome(&self) -> &'static str {
        match self {
            UpstreamError::Timeout => "timeout",
            UpstreamError::Connect(_) => "connect_error",
            UpstreamError::Transport(_) => "transport_error",
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
        // Failing to read the body is a transport problem (reset, truncated
        // stream), not bad JSON, so it stays retryable.
        let bytes = resp.bytes().await.map_err(|e| {
            if e.is_timeout() {
                UpstreamError::Timeout
            } else {
                UpstreamError::Transport(e.to_string())
            }
        })?;
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
    } else if e.is_body() || e.is_request() {
        UpstreamError::Transport(e.to_string())
    } else {
        UpstreamError::Other(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn get(base: &str, timeout_ms: u64) -> Result<Fetched<serde_json::Value>, UpstreamError> {
        Client::new(base, Duration::from_millis(timeout_ms))
            .get_json("spells/x/", &[])
            .await
    }

    async fn serve(template: ResponseTemplate) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(path("/spells/x/"))
            .respond_with(template)
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn ok_and_not_found() {
        let s = serve(ResponseTemplate::new(200).set_body_json(serde_json::json!({"a": 1}))).await;
        assert_eq!(
            get(&s.uri(), 500).await,
            Ok(Fetched::Found(serde_json::json!({"a": 1})))
        );
        let s = serve(ResponseTemplate::new(404)).await;
        assert_eq!(get(&s.uri(), 500).await, Ok(Fetched::NotFound));
    }

    #[tokio::test]
    async fn server_error_is_retryable_client_error_is_not() {
        let s = serve(ResponseTemplate::new(503)).await;
        let e = get(&s.uri(), 500).await.unwrap_err();
        assert_eq!(e, UpstreamError::Status(503));
        assert!(e.is_retryable());
        let s = serve(ResponseTemplate::new(429)).await;
        let e = get(&s.uri(), 500).await.unwrap_err();
        assert_eq!(e, UpstreamError::Status(429));
        assert!(!e.is_retryable());
    }

    #[tokio::test]
    async fn invalid_json_is_a_non_retryable_decode_error() {
        let s = serve(ResponseTemplate::new(200).set_body_string("not json")).await;
        let e = get(&s.uri(), 500).await.unwrap_err();
        assert!(matches!(e, UpstreamError::Decode(_)), "{e:?}");
        assert!(!e.is_retryable());
    }

    #[tokio::test]
    async fn slow_response_is_a_retryable_timeout() {
        let s = serve(ResponseTemplate::new(200).set_delay(Duration::from_millis(300))).await;
        let e = get(&s.uri(), 50).await.unwrap_err();
        assert_eq!(e, UpstreamError::Timeout);
        assert!(e.is_retryable());
    }

    #[tokio::test]
    async fn refused_connection_is_retryable() {
        let e = get("http://127.0.0.1:9", 500).await.unwrap_err();
        assert!(matches!(e, UpstreamError::Connect(_)), "{e:?}");
        assert!(e.is_retryable());
    }

    #[tokio::test]
    async fn truncated_body_is_a_retryable_transport_error() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = tokio::io::AsyncReadExt::read(&mut sock, &mut buf).await;
            sock.write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 1000\r\n\r\n{\"slug\":",
            )
            .await
            .unwrap();
            // Close mid-body.
        });
        let e = get(&format!("http://{addr}"), 500).await.unwrap_err();
        assert!(matches!(e, UpstreamError::Transport(_)), "{e:?}");
        assert!(e.is_retryable());
    }
}
