//! Script Execution Limits
//!
//! Provides safety limits for edge scripts to prevent:
//! - Runaway scripts (execution time limit)
//! - Action spam (rate limiting)
//! - Resource exhaustion (max actions per run)
//! - Infinite recursion (call depth limit)

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::RwLock;
use std::time::{Duration, Instant};

/// Execution limits for scripts
#[derive(Debug, Clone)]
pub struct ScriptLimits {
    /// Maximum execution time per script run
    pub max_execution_time: Duration,

    /// Maximum actions per single script execution
    pub max_actions_per_run: usize,

    /// Maximum call depth for nested script calls
    pub max_call_depth: usize,

    /// Maximum delay allowed in scripts (ms)
    pub max_delay_ms: u64,

    /// Maximum script executions per minute (rate limit)
    pub rate_limit_per_minute: usize,
}

impl Default for ScriptLimits {
    fn default() -> Self {
        Self {
            max_execution_time: Duration::from_secs(30),
            max_actions_per_run: 50,
            max_call_depth: 5,
            max_delay_ms: 60_000, // 1 minute max delay
            rate_limit_per_minute: 60,
        }
    }
}

impl ScriptLimits {
    /// Create limits for a high-frequency script
    pub fn high_frequency() -> Self {
        Self {
            max_execution_time: Duration::from_secs(5),
            max_actions_per_run: 10,
            max_call_depth: 2,
            max_delay_ms: 5_000,
            rate_limit_per_minute: 120,
        }
    }

    /// Create limits for a low-frequency script
    pub fn low_frequency() -> Self {
        Self {
            max_execution_time: Duration::from_secs(60),
            max_actions_per_run: 100,
            max_call_depth: 10,
            max_delay_ms: 300_000, // 5 minutes
            rate_limit_per_minute: 10,
        }
    }
}

/// Per-script rate limiter
pub struct ScriptRateLimiter {
    /// Map of script_id -> (execution_count, window_start)
    windows: RwLock<HashMap<String, RateLimitWindow>>,
    /// Default limit per minute
    default_limit: usize,
}

struct RateLimitWindow {
    count: AtomicU32,
    window_start: Instant,
}

impl ScriptRateLimiter {
    /// Create a new rate limiter
    pub fn new(default_limit: usize) -> Self {
        Self {
            windows: RwLock::new(HashMap::new()),
            default_limit,
        }
    }

    /// Check if a script can execute (and increment counter)
    pub fn check(&self, script_id: &str) -> bool {
        let mut windows = self.windows.write().unwrap();

        let window = windows
            .entry(script_id.to_string())
            .or_insert_with(|| RateLimitWindow {
                count: AtomicU32::new(0),
                window_start: Instant::now(),
            });

        // Reset window if minute has passed
        if window.window_start.elapsed() >= Duration::from_secs(60) {
            window.count.store(0, Ordering::SeqCst);
            window.window_start = Instant::now();
        }

        let current = window.count.fetch_add(1, Ordering::SeqCst);
        current < self.default_limit as u32
    }

    /// Get current rate for a script
    pub fn current_rate(&self, script_id: &str) -> u32 {
        let windows = self.windows.read().unwrap();
        windows
            .get(script_id)
            .map(|w| w.count.load(Ordering::SeqCst))
            .unwrap_or(0)
    }

    /// Reset rate limit for a script
    pub fn reset(&self, script_id: &str) {
        let mut windows = self.windows.write().unwrap();
        if let Some(window) = windows.get_mut(script_id) {
            window.count.store(0, Ordering::SeqCst);
            window.window_start = Instant::now();
        }
    }

    /// Clear all rate limit windows
    pub fn clear_all(&self) {
        let mut windows = self.windows.write().unwrap();
        windows.clear();
    }
}

/// Execution context for tracking limits during script run
pub struct ExecutionContext {
    /// Start time of execution
    pub start_time: Instant,

    /// Current call depth
    pub call_depth: usize,

    /// Actions executed so far
    pub actions_executed: usize,

    /// Limits for this execution
    pub limits: ScriptLimits,
}

impl ExecutionContext {
    /// Create a new execution context
    pub fn new(limits: ScriptLimits) -> Self {
        Self {
            start_time: Instant::now(),
            call_depth: 0,
            actions_executed: 0,
            limits,
        }
    }

    /// Check if execution time limit exceeded
    pub fn is_time_exceeded(&self) -> bool {
        self.start_time.elapsed() > self.limits.max_execution_time
    }

    /// Check if action limit exceeded
    pub fn is_action_limit_exceeded(&self) -> bool {
        self.actions_executed >= self.limits.max_actions_per_run
    }

    /// Check if call depth exceeded
    pub fn is_depth_exceeded(&self) -> bool {
        self.call_depth >= self.limits.max_call_depth
    }

    /// Check if a delay is allowed
    pub fn is_delay_allowed(&self, delay_ms: u64) -> bool {
        delay_ms <= self.limits.max_delay_ms
    }

    /// Increment action counter, returns error if limit exceeded
    pub fn record_action(&mut self) -> Result<(), LimitError> {
        if self.is_time_exceeded() {
            return Err(LimitError::TimeExceeded);
        }
        if self.is_action_limit_exceeded() {
            return Err(LimitError::ActionLimitExceeded);
        }
        self.actions_executed += 1;
        Ok(())
    }

    /// Enter a nested call, returns error if depth exceeded
    pub fn enter_nested(&mut self) -> Result<(), LimitError> {
        if self.is_depth_exceeded() {
            return Err(LimitError::CallDepthExceeded);
        }
        self.call_depth += 1;
        Ok(())
    }

    /// Exit a nested call
    pub fn exit_nested(&mut self) {
        if self.call_depth > 0 {
            self.call_depth -= 1;
        }
    }

    /// Get remaining time
    pub fn remaining_time(&self) -> Duration {
        self.limits
            .max_execution_time
            .saturating_sub(self.start_time.elapsed())
    }
}

/// Limit violation errors
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LimitError {
    /// Execution time exceeded
    TimeExceeded,
    /// Too many actions
    ActionLimitExceeded,
    /// Call depth too deep
    CallDepthExceeded,
    /// Rate limit exceeded
    RateLimitExceeded,
    /// Delay too long
    DelayTooLong,
}

impl std::fmt::Display for LimitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LimitError::TimeExceeded => write!(f, "Execution time limit exceeded"),
            LimitError::ActionLimitExceeded => write!(f, "Action limit exceeded"),
            LimitError::CallDepthExceeded => write!(f, "Call depth limit exceeded"),
            LimitError::RateLimitExceeded => write!(f, "Rate limit exceeded"),
            LimitError::DelayTooLong => write!(f, "Delay exceeds maximum allowed"),
        }
    }
}

impl std::error::Error for LimitError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_limits() {
        let limits = ScriptLimits::default();
        assert_eq!(limits.max_execution_time, Duration::from_secs(30));
        assert_eq!(limits.max_actions_per_run, 50);
        assert_eq!(limits.max_call_depth, 5);
    }

    #[test]
    fn test_rate_limiter() {
        let limiter = ScriptRateLimiter::new(5);

        for i in 0..5 {
            assert!(limiter.check("script1"), "Attempt {} should succeed", i);
        }

        // 6th attempt should fail
        assert!(!limiter.check("script1"), "6th attempt should fail");
    }

    #[test]
    fn test_execution_context_action_limit() {
        let limits = ScriptLimits {
            max_actions_per_run: 3,
            ..Default::default()
        };
        let mut ctx = ExecutionContext::new(limits);

        assert!(ctx.record_action().is_ok());
        assert!(ctx.record_action().is_ok());
        assert!(ctx.record_action().is_ok());
        assert!(ctx.record_action().is_err());
    }

    #[test]
    fn test_execution_context_depth() {
        let limits = ScriptLimits {
            max_call_depth: 2,
            ..Default::default()
        };
        let mut ctx = ExecutionContext::new(limits);

        assert!(ctx.enter_nested().is_ok());
        assert!(ctx.enter_nested().is_ok());
        assert!(ctx.enter_nested().is_err());

        ctx.exit_nested();
        assert!(ctx.enter_nested().is_ok());
    }

    // ============================================
    // P2: Infinite Loop Protection Tests
    // ============================================

    #[test]
    fn test_infinite_loop_protection_via_depth() {
        // Simulates script A -> calls script B -> calls script A (recursion)
        // Default max_call_depth is 5
        let limits = ScriptLimits::default();
        let mut ctx = ExecutionContext::new(limits);

        // Simulate call stack: main -> script1 -> script2 -> script3 -> script4 -> script5
        assert!(ctx.enter_nested().is_ok()); // depth 1
        assert!(ctx.enter_nested().is_ok()); // depth 2
        assert!(ctx.enter_nested().is_ok()); // depth 3
        assert!(ctx.enter_nested().is_ok()); // depth 4
        assert!(ctx.enter_nested().is_ok()); // depth 5

        // 6th call should be blocked (infinite loop protection)
        let result = ctx.enter_nested();
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), LimitError::CallDepthExceeded);
    }

    #[test]
    fn test_infinite_loop_protection_strict_limit() {
        // Very strict limit for safety-critical scenarios
        let limits = ScriptLimits {
            max_call_depth: 1, // Only allow direct call, no nesting
            ..Default::default()
        };
        let mut ctx = ExecutionContext::new(limits);

        // First nested call should work
        assert!(ctx.enter_nested().is_ok());

        // Second nested call should fail immediately
        let result = ctx.enter_nested();
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), LimitError::CallDepthExceeded);
    }

    #[test]
    fn test_infinite_loop_protection_action_spam() {
        // A script that tries to execute 1000 actions in a loop
        // should be stopped after max_actions_per_run
        let limits = ScriptLimits {
            max_actions_per_run: 10,
            ..Default::default()
        };
        let mut ctx = ExecutionContext::new(limits);

        let mut executed = 0;
        for _ in 0..1000 {
            match ctx.record_action() {
                Ok(()) => executed += 1,
                Err(LimitError::ActionLimitExceeded) => break,
                Err(e) => panic!("Unexpected error: {:?}", e),
            }
        }

        // Should have executed exactly 10 actions, then stopped
        assert_eq!(executed, 10);
        assert!(ctx.is_action_limit_exceeded());
    }

    #[test]
    fn test_combined_protections() {
        // Test that multiple protections work together
        let limits = ScriptLimits {
            max_call_depth: 3,
            max_actions_per_run: 5,
            ..Default::default()
        };
        let mut ctx = ExecutionContext::new(limits);

        // Nest some calls
        assert!(ctx.enter_nested().is_ok());
        assert!(ctx.enter_nested().is_ok());

        // Execute some actions
        assert!(ctx.record_action().is_ok());
        assert!(ctx.record_action().is_ok());

        // Can still enter one more depth
        assert!(ctx.enter_nested().is_ok());

        // But depth is now maxed
        assert!(ctx.is_depth_exceeded());
        assert!(ctx.enter_nested().is_err());

        // Actions still have room
        assert!(!ctx.is_action_limit_exceeded());
        assert!(ctx.record_action().is_ok());
    }

    #[test]
    fn test_rate_limiter_per_script_isolation() {
        // Ensure rate limiting is per-script, not global
        let limiter = ScriptRateLimiter::new(3);

        // Script A uses its quota
        assert!(limiter.check("script_a"));
        assert!(limiter.check("script_a"));
        assert!(limiter.check("script_a"));
        assert!(!limiter.check("script_a")); // Blocked

        // Script B should still work
        assert!(limiter.check("script_b"));
        assert!(limiter.check("script_b"));
        assert!(limiter.check("script_b"));
        assert!(!limiter.check("script_b")); // Now blocked too
    }

    #[test]
    fn test_delay_limit_protection() {
        let limits = ScriptLimits {
            max_delay_ms: 5000, // 5 seconds max
            ..Default::default()
        };
        let ctx = ExecutionContext::new(limits);

        // Normal delays should be allowed
        assert!(ctx.is_delay_allowed(1000));
        assert!(ctx.is_delay_allowed(5000));

        // Excessive delays should be blocked
        assert!(!ctx.is_delay_allowed(5001));
        assert!(!ctx.is_delay_allowed(60_000)); // 1 minute
        assert!(!ctx.is_delay_allowed(u64::MAX)); // Extreme case
    }
}
