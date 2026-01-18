//! Circuit Breaker implementation
//!
//! Prevents cascading failures by temporarily stopping requests to failing services.
//! States: Closed -> Open -> HalfOpen -> Closed
//!
//! # Thread Safety
//! This implementation uses only atomic operations to ensure thread-safe state transitions
//! without the overhead and potential deadlocks of mutexes.

use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Circuit breaker states
const STATE_CLOSED: u8 = 0;
const STATE_OPEN: u8 = 1;
const STATE_HALF_OPEN: u8 = 2;

/// Get current timestamp in milliseconds since UNIX epoch
fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Default maximum half-open permits (v1.2.0)
const DEFAULT_MAX_HALF_OPEN_PERMITS: u32 = 1;

/// Thread-safe circuit breaker using only atomic operations
///
/// # Race Condition Prevention
/// All state transitions use compare-and-swap (CAS) operations to ensure
/// that concurrent access doesn't cause invalid state transitions.
///
/// # Half-Open Limit (v1.2.0)
/// In half-open state, only a limited number of concurrent requests are allowed
/// to prevent overwhelming a recovering service. Excess requests are rejected.
pub struct CircuitBreaker {
    name: String,
    state: AtomicU8,
    failure_count: AtomicU32,
    success_count: AtomicU32,
    failure_threshold: u32,
    success_threshold: u32,
    recovery_timeout_ms: u64,
    /// Last failure timestamp in milliseconds since UNIX epoch
    last_failure_ms: AtomicU64,
    /// Current number of requests in half-open state (v1.2.0)
    half_open_permits: AtomicU32,
    /// Maximum concurrent requests in half-open state (v1.2.0)
    max_half_open_permits: u32,
}

impl CircuitBreaker {
    /// Create a new circuit breaker with default half-open permit limit
    ///
    /// # Arguments
    /// * `name` - Identifier for logging
    /// * `failure_threshold` - Number of failures before opening
    /// * `recovery_timeout` - Time to wait before trying again
    pub fn new(
        name: impl Into<String>,
        failure_threshold: u32,
        recovery_timeout: Duration,
    ) -> Self {
        Self::with_half_open_limit(
            name,
            failure_threshold,
            recovery_timeout,
            DEFAULT_MAX_HALF_OPEN_PERMITS,
        )
    }

    /// Create a circuit breaker with custom half-open permit limit (v1.2.0)
    ///
    /// # Arguments
    /// * `name` - Identifier for logging
    /// * `failure_threshold` - Number of failures before opening
    /// * `recovery_timeout` - Time to wait before trying again
    /// * `max_half_open_permits` - Maximum concurrent requests in half-open state
    pub fn with_half_open_limit(
        name: impl Into<String>,
        failure_threshold: u32,
        recovery_timeout: Duration,
        max_half_open_permits: u32,
    ) -> Self {
        Self {
            name: name.into(),
            state: AtomicU8::new(STATE_CLOSED),
            failure_count: AtomicU32::new(0),
            success_count: AtomicU32::new(0),
            failure_threshold,
            success_threshold: 2, // 2 successes to close from half-open
            recovery_timeout_ms: recovery_timeout.as_millis() as u64,
            last_failure_ms: AtomicU64::new(0),
            half_open_permits: AtomicU32::new(0),
            max_half_open_permits: max_half_open_permits.max(1), // At least 1 permit
        }
    }

    /// Check if circuit is open (requests should be rejected)
    ///
    /// # Thread Safety
    /// Uses compare-and-swap for state transitions to prevent race conditions
    /// where multiple threads might try to transition simultaneously.
    ///
    /// # Half-Open Permit Limit (v1.2.0)
    /// In half-open state, acquires a permit before allowing the request.
    /// If no permits are available, returns true (rejecting the request).
    /// Call `release_permit()` after processing the request.
    pub fn is_open(&self) -> bool {
        loop {
            let current_state = self.state.load(Ordering::Acquire);

            match current_state {
                STATE_CLOSED => return false,
                STATE_OPEN => {
                    let last_failure = self.last_failure_ms.load(Ordering::Acquire);
                    let now = now_millis();

                    // Check if recovery timeout has passed
                    if last_failure > 0
                        && now.saturating_sub(last_failure) >= self.recovery_timeout_ms
                    {
                        // Try to transition to half-open using CAS
                        match self.state.compare_exchange(
                            STATE_OPEN,
                            STATE_HALF_OPEN,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        ) {
                            Ok(_) => {
                                // Successfully transitioned to half-open
                                self.success_count.store(0, Ordering::Release);
                                self.half_open_permits.store(0, Ordering::Release); // Reset permits
                                tracing::info!(
                                    "Circuit breaker '{}' transitioning to half-open",
                                    self.name
                                );
                                // Fall through to half-open permit check
                            }
                            Err(_) => {
                                // Another thread changed the state, retry the loop
                                continue;
                            }
                        }
                    } else {
                        return true;
                    }
                }
                STATE_HALF_OPEN => {
                    // v1.2.0: Check permit availability before allowing request
                    let current_permits = self.half_open_permits.load(Ordering::Acquire);
                    if current_permits >= self.max_half_open_permits {
                        // No permits available, reject request
                        tracing::debug!(
                            "Circuit breaker '{}' half-open limit reached ({}/{})",
                            self.name,
                            current_permits,
                            self.max_half_open_permits
                        );
                        return true;
                    }

                    // Try to acquire a permit using CAS
                    match self.half_open_permits.compare_exchange(
                        current_permits,
                        current_permits + 1,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    ) {
                        Ok(_) => return false, // Permit acquired, allow request
                        Err(_) => continue,    // Another thread acquired, retry
                    }
                }
                _ => return false,
            }
        }
    }

    /// Release a half-open permit after request completes (v1.2.0)
    ///
    /// Call this after `is_open()` returned false and request processing is done.
    pub fn release_permit(&self) {
        let current_state = self.state.load(Ordering::Acquire);
        if current_state == STATE_HALF_OPEN {
            // Decrement permits, ensuring we don't underflow
            loop {
                let current = self.half_open_permits.load(Ordering::Acquire);
                if current == 0 {
                    break;
                }
                if self
                    .half_open_permits
                    .compare_exchange(current, current - 1, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    break;
                }
            }
        }
    }

    /// Record a successful operation
    ///
    /// # v1.2.0: Releases permit in half-open state
    pub fn record_success(&self) {
        let current_state = self.state.load(Ordering::Acquire);

        match current_state {
            STATE_HALF_OPEN => {
                // v1.2.0: Release permit first
                self.release_permit();

                let count = self.success_count.fetch_add(1, Ordering::AcqRel) + 1;
                if count >= self.success_threshold {
                    // Try to transition to closed using CAS
                    if self
                        .state
                        .compare_exchange(
                            STATE_HALF_OPEN,
                            STATE_CLOSED,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_ok()
                    {
                        self.failure_count.store(0, Ordering::Release);
                        self.half_open_permits.store(0, Ordering::Release); // Reset permits
                        tracing::info!("Circuit breaker '{}' closed after recovery", self.name);
                    }
                }
            }
            STATE_CLOSED => {
                // Reset failure count on success
                self.failure_count.store(0, Ordering::Release);
            }
            _ => {}
        }
    }

    /// Record a failed operation
    ///
    /// # v1.2.0: Releases permit in half-open state before opening
    pub fn record_failure(&self) {
        let current_state = self.state.load(Ordering::Acquire);

        match current_state {
            STATE_CLOSED => {
                let count = self.failure_count.fetch_add(1, Ordering::AcqRel) + 1;
                if count >= self.failure_threshold {
                    self.open_circuit();
                }
            }
            STATE_HALF_OPEN => {
                // v1.2.0: Release permit before opening
                self.release_permit();
                // Immediately open on failure in half-open state
                self.open_circuit();
            }
            _ => {}
        }
    }

    /// Open the circuit
    fn open_circuit(&self) {
        // Record the timestamp before changing state
        self.last_failure_ms.store(now_millis(), Ordering::Release);

        // Transition to open state
        let prev = self.state.swap(STATE_OPEN, Ordering::AcqRel);

        // Only log if we actually changed the state
        if prev != STATE_OPEN {
            tracing::warn!(
                "Circuit breaker '{}' opened after {} failures",
                self.name,
                self.failure_count.load(Ordering::Acquire)
            );
        }
    }

    /// Get current state as string (for debugging/metrics)
    pub fn state_name(&self) -> &'static str {
        match self.state.load(Ordering::Acquire) {
            STATE_CLOSED => "closed",
            STATE_OPEN => "open",
            STATE_HALF_OPEN => "half_open",
            _ => "unknown",
        }
    }

    /// Get failure count
    pub fn failure_count(&self) -> u32 {
        self.failure_count.load(Ordering::Acquire)
    }

    /// Reset the circuit breaker
    pub fn reset(&self) {
        self.state.store(STATE_CLOSED, Ordering::Release);
        self.failure_count.store(0, Ordering::Release);
        self.success_count.store(0, Ordering::Release);
        self.last_failure_ms.store(0, Ordering::Release);
        self.half_open_permits.store(0, Ordering::Release); // v1.2.0
    }

    /// Get current half-open permit count (v1.2.0, for monitoring)
    pub fn half_open_permit_count(&self) -> u32 {
        self.half_open_permits.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_circuit_breaker_opens_after_threshold() {
        let cb = CircuitBreaker::new("test", 3, Duration::from_secs(30));

        assert!(!cb.is_open());
        cb.record_failure();
        cb.record_failure();
        assert!(!cb.is_open()); // Still 2 failures

        cb.record_failure();
        assert!(cb.is_open()); // Now 3, should open
    }

    #[test]
    fn test_success_resets_failure_count() {
        let cb = CircuitBreaker::new("test", 3, Duration::from_secs(30));

        cb.record_failure();
        cb.record_failure();
        cb.record_success();

        assert_eq!(cb.failure_count(), 0);
        assert!(!cb.is_open());
    }

    #[test]
    fn test_half_open_closes_after_successes() {
        // Use longer timeout to avoid timing issues in tests
        let cb = CircuitBreaker::new("test", 1, Duration::from_millis(100));

        // Open the circuit
        cb.record_failure();
        assert!(cb.is_open());

        // Wait for recovery timeout
        std::thread::sleep(Duration::from_millis(150));

        // Should be half-open now
        assert!(!cb.is_open());

        // Record successes to close
        cb.record_success();
        cb.record_success();

        assert_eq!(cb.state_name(), "closed");
    }

    #[test]
    fn test_concurrent_state_transitions() {
        use std::sync::Arc;
        use std::thread;

        let cb = Arc::new(CircuitBreaker::new(
            "concurrent-test",
            3,
            Duration::from_millis(10),
        ));

        // Spawn multiple threads to cause failures concurrently
        let mut handles = vec![];
        for _ in 0..10 {
            let cb_clone = cb.clone();
            handles.push(thread::spawn(move || {
                for _ in 0..5 {
                    cb_clone.record_failure();
                    thread::sleep(Duration::from_micros(100));
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // Circuit should be open after many failures
        assert!(cb.is_open());
    }

    #[test]
    fn test_reset() {
        let cb = CircuitBreaker::new("test", 1, Duration::from_secs(30));

        cb.record_failure();
        assert!(cb.is_open());

        cb.reset();
        assert!(!cb.is_open());
        assert_eq!(cb.failure_count(), 0);
        assert_eq!(cb.state_name(), "closed");
    }
}
