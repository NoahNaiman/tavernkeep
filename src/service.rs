//! Lookups against Open5e, shaped into internal models.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde::Serialize;

use crate::api::ApiError;
use crate::breaker::BreakerState;
use crate::cache::{Cached, SwrCache, Ttls};
use crate::clock::SharedClock;
use crate::config::Config;
use crate::open5e::models::{RawMonster, RawSearchPage, RawSpell};
use crate::open5e::{Monster, SearchResults, Spell, UpstreamError};
use crate::upstream::Upstream;

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

pub type SpellDoc = Arc<Doc<Spell>>;
pub type MonsterDoc = Arc<Doc<Monster>>;
pub type SearchDoc = Arc<Doc<SearchResults>>;

pub struct Service {
    config: Arc<Config>,
    upstream: Arc<Upstream>,
    spells: SwrCache<SpellDoc>,
    monsters: SwrCache<MonsterDoc>,
    searches: SwrCache<SearchDoc>,
}

impl Service {
    pub fn new(config: Arc<Config>, clock: SharedClock) -> Self {
        let upstream = Upstream::new(&config, clock.clone());
        let lookup_ttls = Ttls {
            fresh: config.fresh_ttl,
            stale: config.stale_ttl,
            negative: config.negative_ttl,
        };
        // Search results are only cached while fresh; stale search pages are
        // not served.
        let search_ttls = Ttls {
            fresh: config.search_ttl,
            stale: config.search_ttl,
            negative: config.negative_ttl,
        };
        let cap = config.cache_capacity;
        Service {
            spells: SwrCache::new(lookup_ttls, cap, clock.clone()),
            monsters: SwrCache::new(lookup_ttls, cap, clock.clone()),
            searches: SwrCache::new(search_ttls, cap, clock),
            upstream,
            config,
        }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn breaker_state(&self) -> BreakerState {
        self.upstream.breaker_state()
    }

    pub async fn spell(&self, slug: &str) -> Result<Lookup<Spell>, ApiError> {
        validate_slug(slug)?;
        let up = self.upstream.clone();
        let path = format!("spells/{slug}/");
        let fetch = move || async move {
            up.get::<RawSpell>(&path, &[])
                .await
                .map(|f| f.map(|raw| Arc::new(Doc::new(Spell::from(raw)))))
        };
        let cached = self.spells.get(slug, fetch).await;
        self.finish(Kind::Spell, slug, cached)
    }

    pub async fn monster(&self, slug: &str) -> Result<Lookup<Monster>, ApiError> {
        validate_slug(slug)?;
        let up = self.upstream.clone();
        let path = format!("monsters/{slug}/");
        let fetch = move || async move {
            up.get::<RawMonster>(&path, &[])
                .await
                .map(|f| f.map(|raw| Arc::new(Doc::new(Monster::from(raw)))))
        };
        let cached = self.monsters.get(slug, fetch).await;
        self.finish(Kind::Monster, slug, cached)
    }

    pub async fn search(&self, kind: Kind, q: &str) -> Result<Lookup<SearchResults>, ApiError> {
        let q = validate_query(q)?.to_string();
        let key = format!("{}:{}", kind.as_str(), q.to_lowercase());
        let up = self.upstream.clone();
        let path = format!("{}/", kind.collection());
        let query = q.clone();
        let fetch = move || async move {
            up.get::<RawSearchPage>(&path, &[("search", query.as_str()), ("limit", "20")])
                .await
                .map(|f| f.map(|page| Arc::new(Doc::new(page.into_results(kind.as_str(), &query)))))
        };
        let cached = self.searches.get(&key, fetch).await;
        match cached.map_err(|e| upstream_to_api(&e))? {
            Cached::Fresh { value, remaining } => Ok(Lookup {
                doc: value,
                max_age: remaining,
            }),
            Cached::Stale { value } => Ok(Lookup {
                doc: value,
                max_age: Duration::ZERO,
            }),
            Cached::NotFound => Err(ApiError::NotFound(format!("no results for {q:?}"))),
        }
    }

    fn finish<T>(
        &self,
        kind: Kind,
        slug: &str,
        cached: Result<Cached<Arc<Doc<T>>>, UpstreamError>,
    ) -> Result<Lookup<T>, ApiError> {
        match cached.map_err(|e| upstream_to_api(&e))? {
            Cached::Fresh { value, remaining } => Ok(Lookup {
                doc: value,
                max_age: remaining,
            }),
            Cached::Stale { value } => Ok(Lookup {
                doc: value,
                max_age: Duration::ZERO,
            }),
            Cached::NotFound => Err(ApiError::NotFound(format!(
                "no {} named {slug:?}",
                kind.as_str()
            ))),
        }
    }

    /// Whether Open5e answered within the readiness window.
    pub fn upstream_recently_reachable(&self) -> bool {
        self.upstream.recently_reachable()
    }

    /// Readiness check for Open5e that probes when contact is stale.
    pub async fn upstream_reachable_now(&self) -> bool {
        self.upstream.probe_if_stale().await
    }

    /// Number of cached entries across all caches.
    pub async fn cached_entries(&self) -> u64 {
        self.spells.entry_count().await
            + self.monsters.entry_count().await
            + self.searches.entry_count().await
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
