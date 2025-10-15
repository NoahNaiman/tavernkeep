//! Open5e calls with retries, backoff and the circuit breaker.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rand::rngs::StdRng;
use serde::de::DeserializeOwned;

use crate::breaker::{Breaker, BreakerState};
use crate::clock::SharedClock;
use crate::config::Config;
use crate::open5e::{Client, Fetched, UpstreamError};

#[derive(Debug)]
pub struct Upstream {
    client: Client,
    breaker: Breaker,
    attempts: u32,
    backoff_base: Duration,
    backoff_max: Duration,
    rng: Mutex<StdRng>,
    clock: SharedClock,
    ready_window: Duration,
    /// Clock reading (ms) + 1 of the last upstream success; 0 means never.
    last_contact_ms: AtomicU64,
}

impl Upstream {
    pub fn new(config: &Config, clock: SharedClock) -> Arc<Self> {
        Arc::new(Upstream {
            client: Client::new(config.upstream_url.clone(), config.upstream_timeout),
            breaker: Breaker::new(
                config.breaker_threshold,
                config.breaker_cooldown,
                clock.clone(),
            ),
            attempts: config.upstream_attempts.max(1),
            backoff_base: config.backoff_base,
            backoff_max: config.backoff_max,
            rng: Mutex::new(crate::dice::rng_for(None)),
            clock,
            ready_window: config.ready_window,
            last_contact_ms: AtomicU64::new(0),
        })
    }

    pub fn breaker_state(&self) -> BreakerState {
        self.breaker.state()
    }

    /// One logical upstream call: breaker admission, then up to `attempts`
    /// tries with full-jitter backoff on retryable errors. The whole call
    /// counts as one success or failure for the breaker.
    pub async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<Fetched<T>, UpstreamError> {
        let permit = match self.breaker.acquire() {
            Ok(p) => p,
            Err(retry_after) => {
                metrics::counter!("tk_upstream_requests_total", "outcome" => "breaker_open")
                    .increment(1);
                return Err(UpstreamError::BreakerOpen { retry_after });
            }
        };
        let mut retry = 0;
        loop {
            match self.client.get_json(path, query).await {
                Ok(found) => {
                    permit.success();
                    self.mark_contact();
                    return Ok(found);
                }
                Err(e) if e.is_retryable() && retry + 1 < self.attempts => {
                    let wait = {
                        let mut rng = self.rng.lock().unwrap_or_else(|p| p.into_inner());
                        crate::backoff::delay(retry, self.backoff_base, self.backoff_max, &mut *rng)
                    };
                    tracing::debug!(path, retry, wait_ms = wait.as_millis() as u64, error = %e, "retrying upstream");
                    tokio::time::sleep(wait).await;
                    retry += 1;
                }
                Err(e) => {
                    permit.failure();
                    return Err(e);
                }
            }
        }
    }

    fn mark_contact(&self) {
        let now = u64::try_from(self.clock.now().as_millis()).unwrap_or(u64::MAX);
        self.last_contact_ms
            .store(now.saturating_add(1), Ordering::Relaxed);
    }

    /// Whether Open5e answered within the readiness window.
    pub fn recently_reachable(&self) -> bool {
        let last = self.last_contact_ms.load(Ordering::Relaxed);
        if last == 0 {
            return false;
        }
        let now = u64::try_from(self.clock.now().as_millis()).unwrap_or(u64::MAX);
        let window = u64::try_from(self.ready_window.as_millis()).unwrap_or(u64::MAX);
        now.saturating_sub(last - 1) <= window
    }
}
