//! Exponential backoff with full jitter.

use std::time::Duration;

use rand::{Rng, RngExt};

/// Delay before retry number `retry` (0-based): uniform in
/// `[0, min(max, base * 2^retry)]` ("full jitter").
pub fn delay<R: Rng + ?Sized>(retry: u32, base: Duration, max: Duration, rng: &mut R) -> Duration {
    let ceiling = base
        .checked_mul(1u32.checked_shl(retry).unwrap_or(u32::MAX))
        .unwrap_or(max)
        .min(max);
    let ceiling_us = u64::try_from(ceiling.as_micros()).unwrap_or(u64::MAX);
    if ceiling_us == 0 {
        return Duration::ZERO;
    }
    Duration::from_micros(rng.random_range(0..=ceiling_us))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dice::rng_for;

    const BASE: Duration = Duration::from_millis(100);
    const MAX: Duration = Duration::from_millis(1000);

    #[test]
    fn delays_stay_under_the_exponential_ceiling() {
        let mut rng = rng_for(Some(9));
        for retry in 0..12 {
            let ceiling = (BASE * 2u32.pow(retry.min(10))).min(MAX);
            for _ in 0..500 {
                assert!(
                    delay(retry, BASE, MAX, &mut rng) <= ceiling,
                    "retry {retry}"
                );
            }
        }
    }

    #[test]
    fn jitter_spreads_across_the_range() {
        let mut rng = rng_for(Some(1));
        let samples: Vec<Duration> = (0..1000).map(|_| delay(2, BASE, MAX, &mut rng)).collect();
        let below_half = samples
            .iter()
            .filter(|d| **d < Duration::from_millis(200))
            .count();
        assert!((300..700).contains(&below_half), "{below_half}");
        assert!(samples.iter().any(|d| *d > Duration::from_millis(350)));
    }

    #[test]
    fn huge_retry_counts_do_not_overflow() {
        let mut rng = rng_for(Some(2));
        assert!(delay(u32::MAX, BASE, MAX, &mut rng) <= MAX);
    }

    #[test]
    fn zero_base_means_no_delay() {
        let mut rng = rng_for(Some(3));
        assert_eq!(delay(3, Duration::ZERO, MAX, &mut rng), Duration::ZERO);
    }

    #[test]
    fn same_seed_same_delays() {
        let a: Vec<_> = (0..5)
            .map(|i| delay(i, BASE, MAX, &mut rng_for(Some(4))))
            .collect();
        let b: Vec<_> = (0..5)
            .map(|i| delay(i, BASE, MAX, &mut rng_for(Some(4))))
            .collect();
        assert_eq!(a, b);
    }
}
