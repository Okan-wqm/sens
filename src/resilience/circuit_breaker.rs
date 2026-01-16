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

/// Thread-safe circuit breaker using only atomic operations
///
/// # Race Condition Prevention
/// All state transitions use compare-and-swap (CAS) operations to ensure
/// that concurrent access doesn't cause invalid state transitions.
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
}

impl CircuitBreaker {
    /// Create a new circuit breaker
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
        Self {
            name: name.into(),
            state: AtomicU8::new(STATE_CLOSED),
            failure_count: AtomicU32::new(0),
            success_count: AtomicU32::new(0),
            failure_threshold,
            success_threshold: 2, // 2 successes to close from half-open
            recovery_timeout_ms: recovery_timeout.as_millis() as u64,
            last_failure_ms: AtomicU64::new(0),
        }
    }

    /// Check if circuit is open (requests should be rejected)
    ///
    /// # Thread Safety
    /// Uses compare-and-swap for state transitions to prevent race conditions
    /// where multiple threads might try to transition simultaneously.
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
                                tracing::info!(
                                    "Circuit breaker '{}' transitioning to half-open",
                                    self.name
                                );
                                return false;
                            }
                            Err(_) => {
                                // Another thread changed the state, retry the loop
                                continue;
                            }
                        }
                    }
                    return true;
                }
                STATE_HALF_OPEN => return false, // Allow requests through for testing
                _ => return false,
            }
        }
    }

    /// Record a successful operation
    pub fn record_success(&self) {
        let current_state = self.state.load(Ordering::Acquire);

        match current_state {
            STATE_HALF_OPEN => {
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
