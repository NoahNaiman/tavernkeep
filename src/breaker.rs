//! A consecutive-failure circuit breaker with a single half-open probe.
//!
//! ```text
//! Closed --(threshold consecutive failures)--> Open
//! Open --(cooldown elapsed, next caller)--> HalfOpen (that caller is the probe)
//! HalfOpen --(probe succeeds)--> Closed
//! HalfOpen --(probe fails or is abandoned)--> Open
//! ```

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::clock::SharedClock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerState {
    Closed { failures: u32 },
    Open { until: Duration },
    HalfOpen,
}

#[derive(Debug)]
struct Inner {
    state: BreakerState,
    threshold: u32,
    cooldown: Duration,
    clock: SharedClock,
}

#[derive(Debug, Clone)]
pub struct Breaker {
    inner: Arc<Mutex<Inner>>,
}

/// Permission to make one upstream call. Report the result with
/// [`Permit::success`] or [`Permit::failure`]; dropping an unreported probe
/// counts as a failure so the breaker can never get stuck half-open.
#[derive(Debug)]
pub struct Permit {
    breaker: Breaker,
    probe: bool,
    reported: bool,
}

impl Breaker {
    pub fn new(threshold: u32, cooldown: Duration, clock: SharedClock) -> Self {
        let b = Breaker {
            inner: Arc::new(Mutex::new(Inner {
                state: BreakerState::Closed { failures: 0 },
                threshold: threshold.max(1),
                cooldown,
                clock,
            })),
        };
        b.publish(false);
        b
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn state(&self) -> BreakerState {
        self.lock().state
    }

    pub fn is_open(&self) -> bool {
        !matches!(self.state(), BreakerState::Closed { .. })
    }

    /// Ask to make a call. `Err` carries how long until a retry may succeed.
    pub fn acquire(&self) -> Result<Permit, Duration> {
        let mut inner = self.lock();
        let now = inner.clock.now();
        match inner.state {
            BreakerState::Closed { .. } => Ok(self.permit(false)),
            BreakerState::Open { until } if now >= until => {
                inner.state = BreakerState::HalfOpen;
                tracing::info!("circuit breaker half-open, sending probe");
                Ok(self.permit(true))
            }
            BreakerState::Open { until } => Err(until - now),
            // A probe is already in flight; ask callers to come back shortly.
            BreakerState::HalfOpen => Err(Duration::from_secs(1)),
        }
    }

    fn permit(&self, probe: bool) -> Permit {
        Permit {
            breaker: self.clone(),
            probe,
            reported: false,
        }
    }

    fn on_success(&self, probe: bool) {
        let mut inner = self.lock();
        match inner.state {
            BreakerState::Closed { .. } => inner.state = BreakerState::Closed { failures: 0 },
            BreakerState::HalfOpen if probe => {
                inner.state = BreakerState::Closed { failures: 0 };
                drop(inner);
                tracing::info!("circuit breaker closed");
                self.publish(false);
            }
            // Late results from calls admitted before the breaker opened
            // don't change an open breaker; only the probe decides.
            _ => {}
        }
    }

    fn on_failure(&self, probe: bool) {
        let mut inner = self.lock();
        let now = inner.clock.now();
        let open_until = now + inner.cooldown;
        match inner.state {
            BreakerState::Closed { failures } => {
                let failures = failures + 1;
                if failures >= inner.threshold {
                    inner.state = BreakerState::Open { until: open_until };
                    drop(inner);
                    tracing::warn!(failures, "circuit breaker opened");
                    self.publish(true);
                } else {
                    inner.state = BreakerState::Closed { failures };
                }
            }
            BreakerState::HalfOpen if probe => {
                inner.state = BreakerState::Open { until: open_until };
                drop(inner);
                tracing::warn!("circuit breaker probe failed, reopening");
                self.publish(true);
            }
            _ => {}
        }
    }

    fn publish(&self, open: bool) {
        metrics::gauge!("tk_breaker_open").set(if open { 1.0 } else { 0.0 });
    }
}

impl Permit {
    pub fn success(mut self) {
        self.reported = true;
        self.breaker.on_success(self.probe);
    }

    pub fn failure(mut self) {
        self.reported = true;
        self.breaker.on_failure(self.probe);
    }

    pub fn is_probe(&self) -> bool {
        self.probe
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        if !self.reported && self.probe {
            self.breaker.on_failure(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::MockClock;

    fn breaker() -> (Breaker, Arc<MockClock>) {
        let clock = MockClock::new();
        (
            Breaker::new(5, Duration::from_secs(30), clock.clone()),
            clock,
        )
    }

    fn fail(b: &Breaker, n: usize) {
        for _ in 0..n {
            b.acquire().unwrap().failure();
        }
    }

    #[test]
    fn opens_after_threshold_consecutive_failures() {
        let (b, _) = breaker();
        fail(&b, 4);
        assert_eq!(b.state(), BreakerState::Closed { failures: 4 });
        fail(&b, 1);
        assert!(matches!(b.state(), BreakerState::Open { .. }));
        assert_eq!(b.acquire().unwrap_err(), Duration::from_secs(30));
    }

    #[test]
    fn success_resets_the_failure_count() {
        let (b, _) = breaker();
        fail(&b, 4);
        b.acquire().unwrap().success();
        fail(&b, 4);
        assert_eq!(b.state(), BreakerState::Closed { failures: 4 });
    }

    #[test]
    fn retry_after_counts_down_with_the_clock() {
        let (b, clock) = breaker();
        fail(&b, 5);
        clock.advance(Duration::from_secs(12));
        assert_eq!(b.acquire().unwrap_err(), Duration::from_secs(18));
    }

    #[test]
    fn half_open_allows_exactly_one_probe() {
        let (b, clock) = breaker();
        fail(&b, 5);
        clock.advance(Duration::from_secs(30));
        let probe = b.acquire().unwrap();
        assert!(probe.is_probe());
        assert_eq!(b.state(), BreakerState::HalfOpen);
        assert!(b.acquire().is_err(), "second caller must not get through");
        probe.success();
        assert_eq!(b.state(), BreakerState::Closed { failures: 0 });
        assert!(b.acquire().is_ok());
    }

    #[test]
    fn failed_probe_reopens_for_a_full_cooldown() {
        let (b, clock) = breaker();
        fail(&b, 5);
        clock.advance(Duration::from_secs(31));
        b.acquire().unwrap().failure();
        assert_eq!(
            b.state(),
            BreakerState::Open {
                until: Duration::from_secs(61)
            }
        );
        assert_eq!(b.acquire().unwrap_err(), Duration::from_secs(30));
    }

    #[test]
    fn abandoned_probe_counts_as_failure() {
        let (b, clock) = breaker();
        fail(&b, 5);
        clock.advance(Duration::from_secs(30));
        drop(b.acquire().unwrap());
        assert!(matches!(b.state(), BreakerState::Open { .. }));
    }

    #[test]
    fn late_results_do_not_close_an_open_breaker() {
        let (b, _) = breaker();
        let early = b.acquire().unwrap();
        fail(&b, 5);
        early.success();
        assert!(matches!(b.state(), BreakerState::Open { .. }));
    }

    #[test]
    fn dropping_a_closed_permit_is_neutral() {
        let (b, _) = breaker();
        fail(&b, 2);
        drop(b.acquire().unwrap());
        assert_eq!(b.state(), BreakerState::Closed { failures: 2 });
    }
}
