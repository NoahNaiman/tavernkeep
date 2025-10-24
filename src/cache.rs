//! Stale-while-revalidate cache over `moka` with per-key request coalescing.
//!
//! ```text
//! age < fresh_ttl              -> serve (hit)
//! fresh_ttl <= age < stale_ttl -> serve now, refresh in background (stale)
//! negative and age < neg_ttl   -> serve the cached 404 (negative_hit)
//! otherwise                    -> fetch, coalesced per key (miss)
//! ```
//!
//! Ages come from an injected [`Clock`](crate::clock::Clock); moka's own TTL
//! (set to `stale_ttl`) only bounds memory.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;

use crate::clock::SharedClock;
use crate::open5e::{Fetched, UpstreamError};

/// TTLs for one cache.
#[derive(Debug, Clone, Copy)]
pub struct Ttls {
    pub fresh: Duration,
    pub stale: Duration,
    pub negative: Duration,
}

#[derive(Debug, Clone)]
struct Entry<V> {
    /// `None` records an upstream 404.
    value: Option<V>,
    stored_at: Duration,
    /// Age the entry already had when it was imported from a snapshot.
    prior_age: Duration,
}

impl<V> Entry<V> {
    fn age(&self, now: Duration) -> Duration {
        now.saturating_sub(self.stored_at) + self.prior_age
    }
}

/// What a lookup produced.
#[derive(Debug, Clone, PartialEq)]
pub enum Cached<V> {
    /// Within the fresh TTL; `remaining` is how long it stays fresh.
    Fresh { value: V, remaining: Duration },
    /// Past the fresh TTL; a background refresh has been requested.
    Stale { value: V },
    /// Upstream said 404 (possibly cached).
    NotFound,
}

type Shared<V> = Result<Entry<V>, UpstreamError>;
type Inflight<V> = Mutex<HashMap<String, watch::Receiver<Option<Shared<V>>>>>;

struct Inner<V> {
    store: moka::future::Cache<String, Entry<V>>,
    inflight: Inflight<V>,
    ttls: Ttls,
    clock: SharedClock,
}

/// Cloning is cheap and shares the same cache.
pub struct SwrCache<V> {
    inner: Arc<Inner<V>>,
}

impl<V> Clone for SwrCache<V> {
    fn clone(&self) -> Self {
        SwrCache {
            inner: self.inner.clone(),
        }
    }
}

enum State<V> {
    Fresh(V, Duration),
    Stale(V),
    Negative,
    Missing,
}

fn event(result: &'static str) {
    metrics::counter!("tk_cache_events_total", "result" => result).increment(1);
}

impl<V: Clone + Send + Sync + 'static> SwrCache<V> {
    pub fn new(ttls: Ttls, capacity: u64, clock: SharedClock) -> Self {
        let store = moka::future::Cache::builder()
            .max_capacity(capacity)
            .time_to_live(ttls.stale.max(ttls.negative))
            .build();
        SwrCache {
            inner: Arc::new(Inner {
                store,
                inflight: Mutex::new(HashMap::new()),
                ttls,
                clock,
            }),
        }
    }

    /// Approximate number of stored entries (positive and negative).
    pub async fn entry_count(&self) -> u64 {
        self.inner.store.run_pending_tasks().await;
        self.inner.store.entry_count()
    }

    /// Every positive entry with its current age (for snapshots).
    pub fn export(&self) -> Vec<(String, V, Duration)> {
        let now = self.inner.clock.now();
        self.inner
            .store
            .iter()
            .filter_map(|(k, e)| {
                let age = e.age(now);
                e.value.map(|v| (k.to_string(), v, age))
            })
            .collect()
    }

    /// Insert `value` as if it had been fetched `age` ago. Returns false (and
    /// stores nothing) when that age is already past the stale TTL.
    pub async fn import(&self, key: &str, value: V, age: Duration) -> bool {
        if age >= self.inner.ttls.stale {
            return false;
        }
        let entry = Entry {
            value: Some(value),
            stored_at: self.inner.clock.now(),
            prior_age: age,
        };
        self.inner.store.insert(key.to_string(), entry).await;
        true
    }

    /// Whether a fetch or refresh for `key` is running.
    pub fn is_inflight(&self, key: &str) -> bool {
        self.inflight().contains_key(key)
    }

    fn inflight(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<String, watch::Receiver<Option<Shared<V>>>>> {
        self.inner
            .inflight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    async fn state(&self, key: &str) -> State<V> {
        let Some(entry) = self.inner.store.get(key).await else {
            return State::Missing;
        };
        let age = entry.age(self.inner.clock.now());
        let t = self.inner.ttls;
        match entry.value {
            None if age < t.negative => State::Negative,
            None => State::Missing,
            Some(v) if age < t.fresh => State::Fresh(v, t.fresh - age),
            Some(v) if age < t.stale => State::Stale(v),
            Some(_) => State::Missing,
        }
    }

    /// Look `key` up, calling `fetch` only when needed. Concurrent callers
    /// for the same key share one `fetch`; stale entries are served at once
    /// while at most one background refresh per key runs.
    pub async fn get<F, Fut>(&self, key: &str, fetch: F) -> Result<Cached<V>, UpstreamError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Fetched<V>, UpstreamError>> + Send + 'static,
    {
        match self.state(key).await {
            State::Fresh(value, remaining) => {
                event("hit");
                Ok(Cached::Fresh { value, remaining })
            }
            State::Negative => {
                event("negative_hit");
                Ok(Cached::NotFound)
            }
            State::Stale(value) => {
                event("stale");
                if !self.is_inflight(key) {
                    // Detached: the result lands in the cache; errors keep the
                    // stale copy in place.
                    drop(self.coalesced(key, fetch));
                }
                Ok(Cached::Stale { value })
            }
            State::Missing => {
                event("miss");
                let entry = self.coalesced(key, fetch).wait().await?;
                Ok(self.to_cached(entry))
            }
        }
    }

    fn to_cached(&self, entry: Entry<V>) -> Cached<V> {
        let age = entry.age(self.inner.clock.now());
        match entry.value {
            None => Cached::NotFound,
            Some(value) => Cached::Fresh {
                value,
                remaining: self.inner.ttls.fresh.saturating_sub(age),
            },
        }
    }

    /// Join the in-flight fetch for `key`, or start one in a spawned task so
    /// that a cancelled caller never cancels the shared fetch.
    fn coalesced<F, Fut>(&self, key: &str, fetch: F) -> Waiter<V>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Fetched<V>, UpstreamError>> + Send + 'static,
    {
        let mut map = self.inflight();
        if let Some(rx) = map.get(key) {
            return Waiter(rx.clone());
        }
        let (tx, rx) = watch::channel(None);
        map.insert(key.to_string(), rx.clone());
        drop(map);

        let this = self.clone();
        let key = key.to_string();
        let fut = fetch();
        tokio::spawn(async move {
            let result = this.run_fetch(&key, fut).await;
            // Store, then deregister, then publish: a caller that misses the
            // in-flight entry is guaranteed to see the stored value instead.
            this.inflight().remove(&key);
            let _ = tx.send(Some(result));
        });
        Waiter(rx)
    }

    async fn run_fetch<Fut>(&self, key: &str, fut: Fut) -> Shared<V>
    where
        Fut: Future<Output = Result<Fetched<V>, UpstreamError>>,
    {
        // Another flight may have completed between our cache check and
        // registering this one; don't fetch again if so.
        if let Some(entry) = self.inner.store.get(key).await {
            let age = entry.age(self.inner.clock.now());
            let ttl = if entry.value.is_some() {
                self.inner.ttls.fresh
            } else {
                self.inner.ttls.negative
            };
            if age < ttl {
                return Ok(entry);
            }
        }
        let result = fut.await;
        let entry = match result {
            Ok(Fetched::Found(v)) => Entry {
                value: Some(v),
                stored_at: self.inner.clock.now(),
                prior_age: Duration::ZERO,
            },
            Ok(Fetched::NotFound) => Entry {
                value: None,
                stored_at: self.inner.clock.now(),
                prior_age: Duration::ZERO,
            },
            Err(e) => return Err(e),
        };
        self.inner
            .store
            .insert(key.to_string(), entry.clone())
            .await;
        Ok(entry)
    }
}

/// Waits for a coalesced fetch to finish.
struct Waiter<V>(watch::Receiver<Option<Shared<V>>>);

impl<V: Clone> Waiter<V> {
    async fn wait(mut self) -> Shared<V> {
        match self.0.wait_for(Option::is_some).await {
            Ok(v) => v.clone().expect("wait_for guarantees Some"),
            Err(_) => Err(UpstreamError::Other(
                "upstream fetch was aborted".to_string(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::clock::MockClock;

    const TTLS: Ttls = Ttls {
        fresh: Duration::from_secs(3600),
        stale: Duration::from_secs(7 * 24 * 3600),
        negative: Duration::from_secs(60),
    };

    fn cache() -> (SwrCache<u32>, Arc<MockClock>) {
        let clock = MockClock::new();
        (SwrCache::new(TTLS, 100, clock.clone()), clock)
    }

    type FetchResult = Result<Fetched<u32>, UpstreamError>;
    type BoxedFetch = std::pin::Pin<Box<dyn Future<Output = FetchResult> + Send>>;

    /// A fetch that counts calls and returns `result`.
    fn counting(calls: &Arc<AtomicUsize>, result: FetchResult) -> impl FnOnce() -> BoxedFetch {
        let calls = calls.clone();
        move || {
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(20)).await;
                result
            })
        }
    }

    async fn settle(c: &SwrCache<u32>, key: &str) {
        for _ in 0..200 {
            if !c.is_inflight(key) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("refresh for {key} never finished");
    }

    #[tokio::test]
    async fn miss_then_fresh_hit() {
        let (c, clock) = cache();
        let calls = Arc::new(AtomicUsize::new(0));
        let r = c
            .get("k", counting(&calls, Ok(Fetched::Found(1))))
            .await
            .unwrap();
        assert_eq!(
            r,
            Cached::Fresh {
                value: 1,
                remaining: TTLS.fresh
            }
        );
        clock.advance(Duration::from_secs(3599));
        let r = c
            .get("k", counting(&calls, Ok(Fetched::Found(2))))
            .await
            .unwrap();
        assert_eq!(
            r,
            Cached::Fresh {
                value: 1,
                remaining: Duration::from_secs(1)
            }
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn stale_is_served_then_refreshed_in_background() {
        let (c, clock) = cache();
        let calls = Arc::new(AtomicUsize::new(0));
        c.get("k", counting(&calls, Ok(Fetched::Found(1))))
            .await
            .unwrap();
        clock.advance(Duration::from_secs(3600));
        let r = c
            .get("k", counting(&calls, Ok(Fetched::Found(2))))
            .await
            .unwrap();
        assert_eq!(r, Cached::Stale { value: 1 });
        settle(&c, "k").await;
        let r = c
            .get("k", counting(&calls, Ok(Fetched::Found(3))))
            .await
            .unwrap();
        assert!(matches!(r, Cached::Fresh { value: 2, .. }), "{r:?}");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn at_most_one_refresh_per_key() {
        let (c, clock) = cache();
        let calls = Arc::new(AtomicUsize::new(0));
        c.get("k", counting(&calls, Ok(Fetched::Found(1))))
            .await
            .unwrap();
        clock.advance(Duration::from_secs(4000));
        for _ in 0..50 {
            let r = c
                .get("k", counting(&calls, Ok(Fetched::Found(2))))
                .await
                .unwrap();
            assert_eq!(r, Cached::Stale { value: 1 });
        }
        settle(&c, "k").await;
        assert_eq!(calls.load(Ordering::SeqCst), 2, "1 initial + 1 refresh");
    }

    #[tokio::test]
    async fn failed_refresh_keeps_the_stale_copy() {
        let (c, clock) = cache();
        let calls = Arc::new(AtomicUsize::new(0));
        c.get("k", counting(&calls, Ok(Fetched::Found(1))))
            .await
            .unwrap();
        clock.advance(Duration::from_secs(4000));
        c.get("k", counting(&calls, Err(UpstreamError::Status(500))))
            .await
            .unwrap();
        settle(&c, "k").await;
        let r = c
            .get("k", counting(&calls, Err(UpstreamError::Status(500))))
            .await
            .unwrap();
        assert_eq!(r, Cached::Stale { value: 1 });
    }

    #[tokio::test]
    async fn entries_past_stale_ttl_are_refetched() {
        let (c, clock) = cache();
        let calls = Arc::new(AtomicUsize::new(0));
        c.get("k", counting(&calls, Ok(Fetched::Found(1))))
            .await
            .unwrap();
        clock.advance(TTLS.stale);
        let err = c
            .get("k", counting(&calls, Err(UpstreamError::Status(503))))
            .await
            .unwrap_err();
        assert_eq!(err, UpstreamError::Status(503));
    }

    #[tokio::test]
    async fn not_found_is_cached_for_the_negative_ttl() {
        let (c, clock) = cache();
        let calls = Arc::new(AtomicUsize::new(0));
        assert_eq!(
            c.get("k", counting(&calls, Ok(Fetched::NotFound)))
                .await
                .unwrap(),
            Cached::NotFound
        );
        clock.advance(Duration::from_secs(59));
        assert_eq!(
            c.get("k", counting(&calls, Ok(Fetched::Found(9))))
                .await
                .unwrap(),
            Cached::NotFound
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        clock.advance(Duration::from_secs(1));
        assert!(matches!(
            c.get("k", counting(&calls, Ok(Fetched::Found(9))))
                .await
                .unwrap(),
            Cached::Fresh { value: 9, .. }
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn imported_entries_keep_their_age() {
        let (c, clock) = cache();
        let calls = Arc::new(AtomicUsize::new(0));
        assert!(c.import("young", 1, Duration::from_secs(600)).await);
        assert!(c.import("old", 2, Duration::from_secs(2 * 3600)).await);
        assert!(!c.import("ancient", 3, TTLS.stale).await);
        assert_eq!(
            c.get("young", counting(&calls, Ok(Fetched::Found(9))))
                .await
                .unwrap(),
            Cached::Fresh {
                value: 1,
                remaining: Duration::from_secs(3000)
            }
        );
        assert_eq!(
            c.get("old", counting(&calls, Err(UpstreamError::Timeout)))
                .await
                .unwrap(),
            Cached::Stale { value: 2 }
        );
        clock.advance(Duration::from_secs(3000));
        assert!(matches!(
            c.get("young", counting(&calls, Err(UpstreamError::Timeout)))
                .await
                .unwrap(),
            Cached::Stale { value: 1 }
        ));
        settle(&c, "young").await;
        settle(&c, "old").await;
        let mut exported = c.export();
        exported.sort();
        assert_eq!(
            exported,
            vec![
                ("old".to_string(), 2, Duration::from_secs(2 * 3600 + 3000)),
                ("young".to_string(), 1, Duration::from_secs(3600)),
            ]
        );
    }

    #[tokio::test]
    async fn export_skips_negative_entries() {
        let (c, _) = cache();
        let calls = Arc::new(AtomicUsize::new(0));
        c.get("gone", counting(&calls, Ok(Fetched::NotFound)))
            .await
            .unwrap();
        c.get("here", counting(&calls, Ok(Fetched::Found(1))))
            .await
            .unwrap();
        let keys: Vec<String> = c.export().into_iter().map(|(k, _, _)| k).collect();
        assert_eq!(keys, vec!["here".to_string()]);
    }

    #[tokio::test]
    async fn errors_are_not_cached() {
        let (c, _) = cache();
        let calls = Arc::new(AtomicUsize::new(0));
        assert!(
            c.get("k", counting(&calls, Err(UpstreamError::Timeout)))
                .await
                .is_err()
        );
        assert!(
            c.get("k", counting(&calls, Ok(Fetched::Found(4))))
                .await
                .is_ok()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_misses_share_one_fetch() {
        let (c, _) = cache();
        let calls = Arc::new(AtomicUsize::new(0));
        let tasks: Vec<_> = (0..100)
            .map(|_| {
                let c = c.clone();
                let calls = calls.clone();
                tokio::spawn(
                    async move { c.get("k", counting(&calls, Ok(Fetched::Found(5)))).await },
                )
            })
            .collect();
        for t in tasks {
            assert!(matches!(
                t.await.unwrap().unwrap(),
                Cached::Fresh { value: 5, .. }
            ));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cancelled_leader_does_not_cancel_the_fetch() {
        let (c, _) = cache();
        let calls = Arc::new(AtomicUsize::new(0));
        let leader = {
            let c = c.clone();
            let calls = calls.clone();
            tokio::spawn(async move { c.get("k", counting(&calls, Ok(Fetched::Found(7)))).await })
        };
        tokio::time::sleep(Duration::from_millis(5)).await;
        leader.abort();
        let r = c
            .get("k", counting(&calls, Ok(Fetched::Found(8))))
            .await
            .unwrap();
        assert!(matches!(r, Cached::Fresh { value: 7, .. }), "{r:?}");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
