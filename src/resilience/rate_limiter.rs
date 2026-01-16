//! Token Bucket Rate Limiter implementation
//!
//! Provides rate limiting for Modbus operations to prevent:
//! - DoS attacks on PLC devices
//! - Excessive polling that could overload network
//! - Compliance with IEC 62443 SL2 FR5: Resource Availability
//!
//! # Thread Safety
//! Uses atomic operations for lock-free, concurrent access.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Get current timestamp in milliseconds since UNIX epoch
fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Token Bucket Rate Limiter
///
/// Allows bursts up to bucket capacity while enforcing average rate.
/// Tokens are refilled at a constant rate up to the maximum capacity.
///
/// # Example
/// ```ignore
/// let limiter = RateLimiter::new(10, Duration::from_secs(1)); // 10 ops/sec
/// if limiter.try_acquire() {
///     // Operation allowed
/// } else {
///     // Rate limited
/// }
/// ```
pub struct RateLimiter {
    name: String,
    /// Maximum tokens (burst capacity)
    capacity: u64,
    /// Tokens added per refill interval
    refill_amount: u64,
    /// Refill interval in milliseconds
    refill_interval_ms: u64,
    /// Current token count (atomic for thread safety)
    tokens: AtomicU64,
    /// Last refill timestamp in milliseconds
    last_refill_ms: AtomicU64,
}

impl RateLimiter {
    /// Create a new rate limiter
    ///
    /// # Arguments
    /// * `name` - Identifier for logging
    /// * `capacity` - Maximum tokens (burst size)
    /// * `refill_rate` - Tokens per second
    ///
    /// # Panics
    /// Panics if capacity or refill_rate is zero
    pub fn new(name: impl Into<String>, capacity: u64, refill_rate: u64) -> Self {
        assert!(capacity > 0, "capacity must be > 0");
        assert!(refill_rate > 0, "refill_rate must be > 0");

        Self {
            name: name.into(),
            capacity,
            refill_amount: refill_rate,
            refill_interval_ms: 1000, // 1 second
            tokens: AtomicU64::new(capacity),
            last_refill_ms: AtomicU64::new(now_millis()),
        }
    }

    /// Create a rate limiter with custom refill interval
    ///
    /// # Arguments
    /// * `name` - Identifier for logging
    /// * `capacity` - Maximum tokens (burst size)
    /// * `refill_amount` - Tokens added per interval
    /// * `refill_interval` - Time between refills
    pub fn with_interval(
        name: impl Into<String>,
        capacity: u64,
        refill_amount: u64,
        refill_interval: Duration,
    ) -> Self {
        assert!(capacity > 0, "capacity must be > 0");
        assert!(refill_amount > 0, "refill_amount must be > 0");
        assert!(
            !refill_interval.is_zero(),
            "refill_interval must be non-zero"
        );

        Self {
            name: name.into(),
            capacity,
            refill_amount,
            refill_interval_ms: refill_interval.as_millis() as u64,
            tokens: AtomicU64::new(capacity),
            last_refill_ms: AtomicU64::new(now_millis()),
        }
    }

    /// Refill tokens based on elapsed time
    ///
    /// Uses compare-and-swap to safely update last_refill_ms
    fn refill(&self) {
        let now = now_millis();
        let last = self.last_refill_ms.load(Ordering::Acquire);
        let elapsed = now.saturating_sub(last);

        if elapsed >= self.refill_interval_ms {
            // Calculate how many intervals have passed
            let intervals = elapsed / self.refill_interval_ms;
            let tokens_to_add = intervals * self.refill_amount;

            // Try to update last_refill_ms atomically
            let new_last = last + (intervals * self.refill_interval_ms);
            if self
                .last_refill_ms
                .compare_exchange(last, new_last, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                // Successfully claimed the refill, add tokens
                let current = self.tokens.load(Ordering::Acquire);
                let new_tokens = (current + tokens_to_add).min(self.capacity);
                self.tokens.store(new_tokens, Ordering::Release);

                tracing::trace!(
                    "Rate limiter '{}' refilled: {} -> {} tokens",
                    self.name,
                    current,
                    new_tokens
                );
            }
            // If compare_exchange failed, another thread did the refill
        }
    }

    /// Try to acquire a token (non-blocking)
    ///
    /// Returns true if token was acquired, false if rate limited.
    pub fn try_acquire(&self) -> bool {
        self.try_acquire_n(1)
    }

    /// Try to acquire N tokens (non-blocking)
    ///
    /// Returns true if all tokens were acquired, false if rate limited.
    pub fn try_acquire_n(&self, n: u64) -> bool {
        // First, refill based on elapsed time
        self.refill();

        // Try to decrement tokens atomically
        loop {
            let current = self.tokens.load(Ordering::Acquire);
            if current < n {
                tracing::debug!(
                    "Rate limiter '{}' denied: need {} tokens, have {}",
                    self.name,
                    n,
                    current
                );
                return false;
            }

            let new_tokens = current - n;
            match self.tokens.compare_exchange(
                current,
                new_tokens,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    tracing::trace!(
                        "Rate limiter '{}' acquired {} tokens ({} remaining)",
                        self.name,
                        n,
                        new_tokens
                    );
                    return true;
                }
                Err(_) => {
                    // Another thread modified tokens, retry
                    continue;
                }
            }
        }
    }

    /// Get current available tokens
    pub fn available_tokens(&self) -> u64 {
        self.refill();
        self.tokens.load(Ordering::Acquire)
    }

    /// Get capacity
    pub fn capacity(&self) -> u64 {
        self.capacity
    }

    /// Reset the rate limiter to full capacity
    pub fn reset(&self) {
        self.tokens.store(self.capacity, Ordering::Release);
        self.last_refill_ms.store(now_millis(), Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_basic_rate_limiting() {
        let limiter = RateLimiter::new("test", 3, 1);

        // Should be able to acquire capacity tokens
        assert!(limiter.try_acquire());
        assert!(limiter.try_acquire());
        assert!(limiter.try_acquire());

        // Should be rate limited
        assert!(!limiter.try_acquire());
    }

    #[test]
    fn test_refill() {
        let limiter = RateLimiter::with_interval("test", 2, 2, Duration::from_millis(50));

        // Exhaust tokens
        assert!(limiter.try_acquire());
        assert!(limiter.try_acquire());
        assert!(!limiter.try_acquire());

        // Wait for refill
        thread::sleep(Duration::from_millis(60));

        // Should have tokens again
        assert!(limiter.try_acquire());
    }

    #[test]
    fn test_acquire_n() {
        let limiter = RateLimiter::new("test", 10, 5);

        // Acquire multiple at once
        assert!(limiter.try_acquire_n(5));
        assert!(limiter.try_acquire_n(5));
        assert!(!limiter.try_acquire_n(1));
    }

    #[test]
    fn test_reset() {
        let limiter = RateLimiter::new("test", 5, 1);

        // Exhaust tokens
        for _ in 0..5 {
            assert!(limiter.try_acquire());
        }
        assert!(!limiter.try_acquire());

        // Reset
        limiter.reset();
        assert_eq!(limiter.available_tokens(), 5);
        assert!(limiter.try_acquire());
    }

    #[test]
    fn test_concurrent_access() {
        use std::sync::Arc;

        // Use very long refill interval so no refill during test
        let limiter = Arc::new(RateLimiter::with_interval(
            "concurrent-test",
            100,
            1,
            Duration::from_secs(3600), // 1 hour, effectively no refill during test
        ));
        let mut handles = vec![];

        // Spawn threads to compete for tokens
        for _ in 0..10 {
            let limiter_clone = limiter.clone();
            handles.push(thread::spawn(move || {
                let mut acquired = 0;
                for _ in 0..20 {
                    if limiter_clone.try_acquire() {
                        acquired += 1;
                    }
                }
                acquired
            }));
        }

        let total: u64 = handles.into_iter().map(|h| h.join().unwrap()).sum();

        // Total acquired should equal capacity (no refill during test)
        assert_eq!(total, 100);
    }

    #[test]
    fn test_capacity_bounds() {
        let limiter = RateLimiter::with_interval("test", 5, 10, Duration::from_millis(10));

        // Wait for multiple refill intervals
        thread::sleep(Duration::from_millis(50));

        // Tokens should be capped at capacity
        assert_eq!(limiter.available_tokens(), 5);
    }
}
