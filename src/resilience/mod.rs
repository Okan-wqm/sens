//! Resilience patterns for fault tolerance
//!
//! Provides:
//! - Circuit Breaker pattern for failing services
//! - Timeout wrappers for async operations
//! - Rate limiting

mod circuit_breaker;
mod timeout;

pub use circuit_breaker::CircuitBreaker;
pub use timeout::{with_timeout, TimeoutError};
