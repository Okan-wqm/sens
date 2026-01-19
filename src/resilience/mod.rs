//! Resilience patterns for fault tolerance
//!
//! NOTE: Some helper methods are API-complete but not yet used by all consumers.
#![allow(dead_code)]
//!
//! Provides:
//! - Circuit Breaker pattern for failing services
//! - Timeout wrappers for async operations
//! - Rate limiting (Token Bucket)
//!
//! # IEC 62443 SL2 Compliance
//! - FR3: Input validation via circuit breakers
//! - FR5: Resource availability via rate limiting

mod circuit_breaker;
mod rate_limiter;
mod timeout;

pub use circuit_breaker::CircuitBreaker;
pub use rate_limiter::RateLimiter;
pub use timeout::with_timeout;
