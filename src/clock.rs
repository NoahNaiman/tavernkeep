//! A monotonic clock that tests can control.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Monotonic time since an arbitrary origin.
pub trait Clock: Send + Sync + std::fmt::Debug {
    fn now(&self) -> Duration;
}

pub type SharedClock = Arc<dyn Clock>;

/// The real clock.
#[derive(Debug)]
pub struct SystemClock {
    origin: Instant,
}

impl SystemClock {
    pub fn shared() -> SharedClock {
        Arc::new(SystemClock {
            origin: Instant::now(),
        })
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
}

/// A clock that only moves when told to.
#[derive(Debug, Default)]
pub struct MockClock {
    millis: AtomicU64,
}

impl MockClock {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn advance(&self, by: Duration) {
        self.millis.fetch_add(
            u64::try_from(by.as_millis()).unwrap_or(u64::MAX),
            Ordering::SeqCst,
        );
    }
}

impl Clock for MockClock {
    fn now(&self) -> Duration {
        Duration::from_millis(self.millis.load(Ordering::SeqCst))
    }
}
