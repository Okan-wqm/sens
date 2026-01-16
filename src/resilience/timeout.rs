//! Timeout utilities for async operations

use std::future::Future;
use std::time::Duration;
use thiserror::Error;

/// Timeout error
#[derive(Debug, Error)]
#[error("{operation} timed out after {duration:?}")]
pub struct TimeoutError {
    pub operation: String,
    pub duration: Duration,
}

/// Execute an async operation with a timeout
///
/// # Arguments
/// * `operation` - The async operation to execute
/// * `timeout` - Maximum duration to wait
/// * `operation_name` - Name for error messages
///
/// # Returns
/// * `Ok(T)` - Operation completed successfully
/// * `Err(TimeoutError)` - Operation timed out
pub async fn with_timeout<T, F>(
    operation: F,
    timeout: Duration,
    operation_name: &str,
) -> Result<T, TimeoutError>
where
    F: Future<Output = T>,
{
    tokio::time::timeout(timeout, operation)
        .await
        .map_err(|_| TimeoutError {
            operation: operation_name.to_string(),
            duration: timeout,
        })
}

/// Execute an async operation with timeout, flattening Result
///
/// Useful when the operation itself returns a Result
pub async fn with_timeout_result<T, E, F>(
    operation: F,
    timeout: Duration,
    operation_name: &str,
) -> Result<T, anyhow::Error>
where
    F: Future<Output = Result<T, E>>,
    E: std::error::Error + Send + Sync + 'static,
{
    match tokio::time::timeout(timeout, operation).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => Err(anyhow::Error::from(e)),
        Err(_) => Err(TimeoutError {
            operation: operation_name.to_string(),
            duration: timeout,
        }
        .into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_timeout_success() {
        let result = with_timeout(async { 42 }, Duration::from_secs(1), "test_op").await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_timeout_failure() {
        let result = with_timeout(
            async {
                tokio::time::sleep(Duration::from_secs(10)).await;
                42
            },
            Duration::from_millis(10),
            "slow_op",
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.operation, "slow_op");
    }
}
