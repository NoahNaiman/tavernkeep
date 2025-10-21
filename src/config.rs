//! Configuration from environment variables, validated at startup.

use std::net::SocketAddr;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid configuration: {var}: {reason}")]
pub struct ConfigError {
    pub var: &'static str,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// `TK_BIND`: address to listen on.
    pub bind: SocketAddr,
    /// `TK_UPSTREAM_URL`: Open5e v1 base URL, without trailing slash.
    pub upstream_url: String,
    /// `TK_UPSTREAM_TIMEOUT_MS`: per-attempt upstream timeout.
    pub upstream_timeout: Duration,
    /// `TK_UPSTREAM_ATTEMPTS`: total attempts per upstream call.
    pub upstream_attempts: u32,
    /// `TK_BACKOFF_BASE_MS`: first retry's maximum backoff.
    pub backoff_base: Duration,
    /// `TK_BACKOFF_MAX_MS`: cap on any single backoff.
    pub backoff_max: Duration,
    /// `TK_FRESH_TTL_SECS`: how long a cached entry is served without refresh.
    pub fresh_ttl: Duration,
    /// `TK_STALE_TTL_SECS`: how long a cached entry may be served at all.
    pub stale_ttl: Duration,
    /// `TK_NEGATIVE_TTL_SECS`: how long an upstream 404 is cached.
    pub negative_ttl: Duration,
    /// `TK_SEARCH_TTL_SECS`: how long a search response is cached.
    pub search_ttl: Duration,
    /// `TK_CACHE_CAPACITY`: maximum entries per cache.
    pub cache_capacity: u64,
    /// `TK_BREAKER_THRESHOLD`: consecutive failures that open the breaker.
    pub breaker_threshold: u32,
    /// `TK_BREAKER_COOLDOWN_SECS`: how long the breaker stays open.
    pub breaker_cooldown: Duration,
    /// `TK_READY_WINDOW_SECS`: upstream contact this recent counts as ready.
    pub ready_window: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            bind: SocketAddr::from(([0, 0, 0, 0], 8080)),
            upstream_url: "https://api.open5e.com/v1".to_string(),
            upstream_timeout: Duration::from_secs(3),
            upstream_attempts: 3,
            backoff_base: Duration::from_millis(100),
            backoff_max: Duration::from_secs(2),
            fresh_ttl: Duration::from_secs(3600),
            stale_ttl: Duration::from_secs(7 * 24 * 3600),
            negative_ttl: Duration::from_secs(60),
            search_ttl: Duration::from_secs(300),
            cache_capacity: 10_000,
            breaker_threshold: 5,
            breaker_cooldown: Duration::from_secs(30),
            ready_window: Duration::from_secs(60),
        }
    }
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// Build a config from any key lookup (used by tests).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let d = Config::default();
        let num =
            |var: &'static str, default: u64, min: u64, max: u64| -> Result<u64, ConfigError> {
                match get(var) {
                    None => Ok(default),
                    Some(raw) => {
                        let v: u64 = raw.trim().parse().map_err(|_| ConfigError {
                            var,
                            reason: format!("expected an integer, got {raw:?}"),
                        })?;
                        if (min..=max).contains(&v) {
                            Ok(v)
                        } else {
                            Err(ConfigError {
                                var,
                                reason: format!("must be between {min} and {max}, got {v}"),
                            })
                        }
                    }
                }
            };
        let ms = |var, default: Duration, min, max| {
            num(var, default.as_millis() as u64, min, max).map(Duration::from_millis)
        };
        let secs = |var, default: Duration, min, max| {
            num(var, default.as_secs(), min, max).map(Duration::from_secs)
        };

        let bind = match get("TK_BIND") {
            None => d.bind,
            Some(raw) => raw.trim().parse().map_err(|_| ConfigError {
                var: "TK_BIND",
                reason: format!("expected an IP address and port like 0.0.0.0:8080, got {raw:?}"),
            })?,
        };
        let upstream_url = match get("TK_UPSTREAM_URL") {
            None => d.upstream_url,
            Some(raw) => validate_url(raw.trim())?,
        };
        const WEEK_X52: u64 = 52 * 7 * 24 * 3600;
        let cfg = Config {
            bind,
            upstream_url,
            upstream_timeout: ms("TK_UPSTREAM_TIMEOUT_MS", d.upstream_timeout, 1, 60_000)?,
            upstream_attempts: num("TK_UPSTREAM_ATTEMPTS", d.upstream_attempts.into(), 1, 10)?
                as u32,
            backoff_base: ms("TK_BACKOFF_BASE_MS", d.backoff_base, 0, 60_000)?,
            backoff_max: ms("TK_BACKOFF_MAX_MS", d.backoff_max, 0, 60_000)?,
            fresh_ttl: secs("TK_FRESH_TTL_SECS", d.fresh_ttl, 1, WEEK_X52)?,
            stale_ttl: secs("TK_STALE_TTL_SECS", d.stale_ttl, 1, WEEK_X52)?,
            negative_ttl: secs("TK_NEGATIVE_TTL_SECS", d.negative_ttl, 1, WEEK_X52)?,
            search_ttl: secs("TK_SEARCH_TTL_SECS", d.search_ttl, 1, WEEK_X52)?,
            cache_capacity: num("TK_CACHE_CAPACITY", d.cache_capacity, 1, 10_000_000)?,
            breaker_threshold: num("TK_BREAKER_THRESHOLD", d.breaker_threshold.into(), 1, 1000)?
                as u32,
            breaker_cooldown: secs("TK_BREAKER_COOLDOWN_SECS", d.breaker_cooldown, 1, 3600)?,
            ready_window: secs("TK_READY_WINDOW_SECS", d.ready_window, 1, 3600)?,
        };
        if cfg.stale_ttl < cfg.fresh_ttl {
            return Err(ConfigError {
                var: "TK_STALE_TTL_SECS",
                reason: "must be at least TK_FRESH_TTL_SECS".to_string(),
            });
        }
        if cfg.backoff_max < cfg.backoff_base {
            return Err(ConfigError {
                var: "TK_BACKOFF_MAX_MS",
                reason: "must be at least TK_BACKOFF_BASE_MS".to_string(),
            });
        }
        Ok(cfg)
    }
}

fn validate_url(raw: &str) -> Result<String, ConfigError> {
    let err = |reason: &str| ConfigError {
        var: "TK_UPSTREAM_URL",
        reason: format!("{reason}, got {raw:?}"),
    };
    let url = reqwest::Url::parse(raw).map_err(|_| err("not a valid URL"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(err("scheme must be http or https"));
    }
    if url.host_str().is_none() {
        return Err(err("missing host"));
    }
    Ok(raw.trim_end_matches('/').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn with(pairs: &[(&str, &str)]) -> Result<Config, ConfigError> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Config::from_lookup(|k| map.get(k).cloned())
    }

    #[test]
    fn defaults_match_spec() {
        let c = with(&[]).unwrap();
        assert_eq!(c.upstream_timeout, Duration::from_secs(3));
        assert_eq!(c.upstream_attempts, 3);
        assert_eq!(c.fresh_ttl, Duration::from_secs(3600));
        assert_eq!(c.stale_ttl, Duration::from_secs(604_800));
        assert_eq!(c.negative_ttl, Duration::from_secs(60));
        assert_eq!(c.search_ttl, Duration::from_secs(300));
        assert_eq!(c.breaker_threshold, 5);
        assert_eq!(c.breaker_cooldown, Duration::from_secs(30));
    }

    #[test]
    fn overrides_are_applied_and_trailing_slash_trimmed() {
        let c = with(&[
            ("TK_BIND", "127.0.0.1:9000"),
            ("TK_UPSTREAM_URL", "http://localhost:1234/v1/"),
            ("TK_BREAKER_THRESHOLD", "2"),
        ])
        .unwrap();
        assert_eq!(c.bind.port(), 9000);
        assert_eq!(c.upstream_url, "http://localhost:1234/v1");
        assert_eq!(c.breaker_threshold, 2);
    }

    #[test]
    fn invalid_values_name_the_variable() {
        assert_eq!(with(&[("TK_BIND", "nope")]).unwrap_err().var, "TK_BIND");
        assert_eq!(
            with(&[("TK_UPSTREAM_URL", "ftp://x")]).unwrap_err().var,
            "TK_UPSTREAM_URL"
        );
        assert_eq!(
            with(&[("TK_UPSTREAM_TIMEOUT_MS", "0")]).unwrap_err().var,
            "TK_UPSTREAM_TIMEOUT_MS"
        );
        assert_eq!(
            with(&[("TK_UPSTREAM_ATTEMPTS", "abc")]).unwrap_err().var,
            "TK_UPSTREAM_ATTEMPTS"
        );
        assert_eq!(
            with(&[("TK_FRESH_TTL_SECS", "100"), ("TK_STALE_TTL_SECS", "10")])
                .unwrap_err()
                .var,
            "TK_STALE_TTL_SECS"
        );
    }
}
