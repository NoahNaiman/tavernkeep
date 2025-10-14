//! Lookups against Open5e, shaped into internal models.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use bytes::Bytes;
use serde::Serialize;

use crate::api::ApiError;
use crate::clock::SharedClock;
use crate::config::Config;
use crate::open5e::models::{RawMonster, RawSearchPage, RawSpell};
use crate::open5e::{Client, Fetched, Monster, SearchResults, Spell, UpstreamError};

/// A serialized model with its ETag, ready to serve.
#[derive(Debug)]
pub struct Doc<T> {
    pub value: T,
    pub body: Bytes,
    pub etag: String,
}

impl<T: Serialize> Doc<T> {
    pub fn new(value: T) -> Self {
        let body = Bytes::from(serde_json::to_vec(&value).expect("models serialize"));
        let etag = etag_for(&body);
        Doc { value, body, etag }
    }
}

/// A strong ETag from a 64-bit FNV-1a hash of the body.
pub fn etag_for(body: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in body {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("\"{h:016x}\"")
}

/// A lookup result plus how long clients may cache it.
#[derive(Debug, Clone)]
pub struct Lookup<T> {
    pub doc: Arc<Doc<T>>,
    pub max_age: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Spell,
    Monster,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        match s {
            "spell" => Some(Kind::Spell),
            "monster" => Some(Kind::Monster),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Spell => "spell",
            Kind::Monster => "monster",
        }
    }

    fn collection(self) -> &'static str {
        match self {
            Kind::Spell => "spells",
            Kind::Monster => "monsters",
        }
    }
}

/// Slugs are lowercase ASCII letters, digits and hyphens, 1–100 chars.
pub fn validate_slug(slug: &str) -> Result<(), ApiError> {
    let ok = (1..=100).contains(&slug.len())
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(ApiError::BadRequest(
            "slug must be 1-100 lowercase letters, digits or hyphens".to_string(),
        ))
    }
}

/// Search queries must be 2–64 characters after trimming.
pub fn validate_query(q: &str) -> Result<&str, ApiError> {
    let q = q.trim();
    let len = q.chars().count();
    if (2..=64).contains(&len) {
        Ok(q)
    } else {
        Err(ApiError::BadRequest(
            "q must be between 2 and 64 characters".to_string(),
        ))
    }
}

/// Map an upstream failure to the API error clients see.
pub fn upstream_to_api(e: &UpstreamError) -> ApiError {
    match e {
        UpstreamError::BreakerOpen { retry_after } => ApiError::Unavailable {
            message: "Open5e is unavailable and this entry is not cached".to_string(),
            retry_after: retry_after.as_secs().max(1),
        },
        other => ApiError::BadGateway(other.to_string()),
    }
}

pub struct Service {
    config: Arc<Config>,
    client: Client,
    clock: SharedClock,
    /// Clock reading (ms) + 1 of the last upstream success; 0 means never.
    last_contact_ms: AtomicU64,
}

impl Service {
    pub fn new(config: Arc<Config>, clock: SharedClock) -> Self {
        let client = Client::new(config.upstream_url.clone(), config.upstream_timeout);
        Service {
            config,
            client,
            clock,
            last_contact_ms: AtomicU64::new(0),
        }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub async fn spell(&self, slug: &str) -> Result<Lookup<Spell>, ApiError> {
        validate_slug(slug)?;
        let raw: RawSpell = self.fetch_one(Kind::Spell, slug).await?;
        Ok(self.fresh(Spell::from(raw)))
    }

    pub async fn monster(&self, slug: &str) -> Result<Lookup<Monster>, ApiError> {
        validate_slug(slug)?;
        let raw: RawMonster = self.fetch_one(Kind::Monster, slug).await?;
        Ok(self.fresh(Monster::from(raw)))
    }

    pub async fn search(&self, kind: Kind, q: &str) -> Result<Lookup<SearchResults>, ApiError> {
        let q = validate_query(q)?;
        let path = format!("{}/", kind.collection());
        let page: RawSearchPage = match self
            .record(
                self.client
                    .get_json(&path, &[("search", q), ("limit", "20")])
                    .await,
            )
            .map_err(|e| upstream_to_api(&e))?
        {
            Fetched::Found(p) => p,
            Fetched::NotFound => return Err(ApiError::NotFound("search not found".to_string())),
        };
        Ok(Lookup {
            doc: Arc::new(Doc::new(page.into_results(kind.as_str(), q))),
            max_age: self.config.search_ttl,
        })
    }

    async fn fetch_one<R: serde::de::DeserializeOwned>(
        &self,
        kind: Kind,
        slug: &str,
    ) -> Result<R, ApiError> {
        let path = format!("{}/{slug}/", kind.collection());
        match self
            .record(self.client.get_json(&path, &[]).await)
            .map_err(|e| upstream_to_api(&e))?
        {
            Fetched::Found(r) => Ok(r),
            Fetched::NotFound => Err(ApiError::NotFound(format!(
                "no {} named {slug:?}",
                kind.as_str()
            ))),
        }
    }

    fn fresh<T: Serialize>(&self, value: T) -> Lookup<T> {
        Lookup {
            doc: Arc::new(Doc::new(value)),
            max_age: self.config.fresh_ttl,
        }
    }

    fn record<T>(&self, r: Result<T, UpstreamError>) -> Result<T, UpstreamError> {
        if r.is_ok() {
            let now = u64::try_from(self.clock.now().as_millis()).unwrap_or(u64::MAX);
            self.last_contact_ms
                .store(now.saturating_add(1), Ordering::Relaxed);
        }
        r
    }

    /// Whether Open5e answered within the readiness window.
    pub fn upstream_recently_reachable(&self) -> bool {
        let last = self.last_contact_ms.load(Ordering::Relaxed);
        if last == 0 {
            return false;
        }
        let now = u64::try_from(self.clock.now().as_millis()).unwrap_or(u64::MAX);
        now.saturating_sub(last - 1) <= self.config.ready_window.as_millis() as u64
    }

    /// Number of cached entries (lookups are not cached yet).
    pub async fn cached_entries(&self) -> u64 {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_validation() {
        assert!(validate_slug("adult-red-dragon").is_ok());
        assert!(validate_slug("").is_err());
        assert!(validate_slug("Fireball").is_err());
        assert!(validate_slug("../etc").is_err());
        assert!(validate_slug(&"a".repeat(101)).is_err());
    }

    #[test]
    fn query_validation() {
        assert_eq!(validate_query(" fi ").unwrap(), "fi");
        assert!(validate_query("f").is_err());
        assert!(validate_query(&"x".repeat(65)).is_err());
        assert!(validate_query(&"x".repeat(64)).is_ok());
    }

    #[test]
    fn etag_is_stable_and_content_sensitive() {
        assert_eq!(etag_for(b"abc"), etag_for(b"abc"));
        assert_ne!(etag_for(b"abc"), etag_for(b"abd"));
        assert!(etag_for(b"abc").starts_with('"'));
    }
}
