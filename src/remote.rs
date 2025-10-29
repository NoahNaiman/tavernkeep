//! A small client for a running Tavernkeep server, used by the terminal
//! subcommands (`tavernkeep spell fireball`, `tavernkeep roll adv+3`, ...).

use std::time::Duration;

use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::api::AttackRoll;
use crate::dice::RollOutcome;
use crate::open5e::{Monster, SearchResults, Spell};

/// An error from the server (its JSON error body) or from reaching it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RemoteError {
    #[error("{message}")]
    Server {
        status: u16,
        message: String,
        position: Option<usize>,
        retry_after: Option<u64>,
    },
    #[error("cannot reach tavernkeep at {url}: {reason}")]
    Unreachable { url: String, reason: String },
    #[error("unexpected response from tavernkeep: {0}")]
    BadResponse(String),
}

#[derive(Deserialize)]
struct ErrorBody {
    message: String,
    position: Option<usize>,
}

pub struct Remote {
    http: reqwest::Client,
    base: String,
}

impl Remote {
    pub fn new(base: &str) -> Self {
        Remote {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .expect("static reqwest configuration is valid"),
            base: base.trim_end_matches('/').to_string(),
        }
    }

    pub async fn spell(&self, slug: &str) -> Result<Spell, RemoteError> {
        self.get(&format!("/spells/{slug}"), &[]).await
    }

    pub async fn monster(&self, slug: &str) -> Result<Monster, RemoteError> {
        self.get(&format!("/monsters/{slug}"), &[]).await
    }

    pub async fn search(&self, q: &str, kind: &str) -> Result<SearchResults, RemoteError> {
        self.get("/search", &[("q", q), ("kind", kind)]).await
    }

    pub async fn attack(
        &self,
        slug: &str,
        index: usize,
        seed: Option<u64>,
    ) -> Result<AttackRoll, RemoteError> {
        let seed = seed.map(|s| s.to_string());
        let query: Vec<(&str, &str)> = seed.iter().map(|s| ("seed", s.as_str())).collect();
        self.get(&format!("/monsters/{slug}/attack/{index}"), &query)
            .await
    }

    pub async fn roll(&self, expr: &str, seed: Option<u64>) -> Result<RollOutcome, RemoteError> {
        let body = serde_json::json!({ "expr": expr, "seed": seed });
        let url = format!("{}/roll", self.base);
        let resp = self.http.post(&url).json(&body).send().await;
        self.decode(&url, resp).await
    }

    async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T, RemoteError> {
        let url = format!("{}{path}", self.base);
        let resp = self.http.get(&url).query(query).send().await;
        self.decode(&url, resp).await
    }

    async fn decode<T: DeserializeOwned>(
        &self,
        url: &str,
        resp: Result<reqwest::Response, reqwest::Error>,
    ) -> Result<T, RemoteError> {
        let resp = resp.map_err(|e| RemoteError::Unreachable {
            url: self.base.clone(),
            reason: if e.is_connect() {
                "connection refused (is `tavernkeep serve` running?)".to_string()
            } else {
                e.to_string()
            },
        })?;
        let status = resp.status();
        let retry_after = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok());
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| RemoteError::BadResponse(format!("{url}: {e}")))?;
        if status.is_success() {
            return serde_json::from_slice(&bytes)
                .map_err(|e| RemoteError::BadResponse(format!("{url}: {e}")));
        }
        let body: Option<ErrorBody> = serde_json::from_slice(&bytes).ok();
        Err(RemoteError::Server {
            status: status.as_u16(),
            message: body.as_ref().map_or_else(
                || format!("HTTP {}", status.as_u16()),
                |b| b.message.clone(),
            ),
            position: body.and_then(|b| b.position),
            retry_after,
        })
    }
}
