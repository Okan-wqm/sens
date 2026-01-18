//! Error types for Suderra Edge Agent
//!
//! ## Modbus Error Granularity (v1.2.0)
//! Added detailed Modbus error types for better diagnostics and error handling.
//! This enables callers to handle specific error conditions appropriately.

use thiserror::Error;

use crate::scripting::PersistenceError;

// ============================================================================
// Modbus Error Types (v1.2.0)
// ============================================================================

/// Detailed Modbus error types for granular error handling (v1.2.0)
///
/// # Security Relevance (IEC 62443)
/// - `FunctionCodeNotAllowed`: FR3 violation (unauthorized operation)
/// - `RateLimited`: FR5 rate limiting in effect
/// - `CircuitBreakerOpen`: Fault isolation active
#[derive(Error, Debug, Clone)]
pub enum ModbusError {
    /// Connection failed to establish
    #[error("Connection failed: {0}")]
    Connection(String),

    /// Connection attempt timed out
    #[error("Connection timeout after {0}ms")]
    ConnectionTimeout(u64),

    /// Operation timed out (read/write)
    #[error("Operation timeout after {0}ms")]
    OperationTimeout(u64),

    /// Invalid slave/unit ID
    #[error("Invalid slave ID: {0}")]
    InvalidSlaveId(u8),

    /// Function code not in security whitelist (IEC 62443 FR3)
    #[error("Function code {0} not allowed by security policy")]
    FunctionCodeNotAllowed(u8),

    /// Register address out of range
    #[error("Register {0} out of range (max: {1})")]
    RegisterOutOfRange(u16, u16),

    /// Register count exceeds security limit
    #[error("Register count {0} exceeds maximum {1}")]
    RegisterCountExceeded(u16, u16),

    /// CRC/LRC validation failed
    #[error("Checksum error (CRC/LRC validation failed)")]
    ChecksumError,

    /// Modbus exception response from device
    #[error("Modbus exception code {0}: {1}")]
    ModbusException(u8, String),

    /// Rate limited (IEC 62443 FR5)
    #[error("Rate limited: too many operations")]
    RateLimited,

    /// Circuit breaker is open
    #[error("Circuit breaker open (device temporarily unavailable)")]
    CircuitBreakerOpen,

    /// Write operation not allowed by security policy
    #[error("Write operations not allowed by security policy")]
    WriteNotAllowed,

    /// Device not connected
    #[error("Device not connected")]
    NotConnected,

    /// Device not found
    #[error("Device not found: {0}")]
    DeviceNotFound(String),

    /// Serial port error (RTU mode)
    #[error("Serial port error: {0}")]
    SerialPort(String),

    /// Generic protocol error
    #[error("Protocol error: {0}")]
    Protocol(String),
}

impl ModbusError {
    /// Create connection error
    pub fn connection(msg: impl Into<String>) -> Self {
        Self::Connection(msg.into())
    }

    /// Create connection timeout error
    pub fn connection_timeout(ms: u64) -> Self {
        Self::ConnectionTimeout(ms)
    }

    /// Create operation timeout error
    pub fn operation_timeout(ms: u64) -> Self {
        Self::OperationTimeout(ms)
    }

    /// Create Modbus exception error with description
    pub fn exception(code: u8) -> Self {
        let desc = match code {
            0x01 => "Illegal function",
            0x02 => "Illegal data address",
            0x03 => "Illegal data value",
            0x04 => "Slave device failure",
            0x05 => "Acknowledge",
            0x06 => "Slave device busy",
            0x08 => "Memory parity error",
            0x0A => "Gateway path unavailable",
            0x0B => "Gateway target failed to respond",
            _ => "Unknown exception",
        };
        Self::ModbusException(code, desc.to_string())
    }

    /// Check if this error is recoverable (retry might help)
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            ModbusError::ConnectionTimeout(_)
                | ModbusError::OperationTimeout(_)
                | ModbusError::RateLimited
                | ModbusError::ModbusException(0x05 | 0x06, _) // ACK or Busy
        )
    }

    /// Check if this error indicates a security policy violation
    pub fn is_security_violation(&self) -> bool {
        matches!(
            self,
            ModbusError::FunctionCodeNotAllowed(_)
                | ModbusError::WriteNotAllowed
                | ModbusError::RegisterCountExceeded(_, _)
        )
    }
}

// ============================================================================
// Agent Error Types
// ============================================================================

/// Agent error types
#[derive(Error, Debug)]
pub enum AgentError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Provisioning error: {0}")]
    Provisioning(String),

    #[error("MQTT error: {0}")]
    Mqtt(String),

    /// Modbus error with granular types (v1.2.0)
    #[error("Modbus error: {0}")]
    Modbus(#[from] ModbusError),

    /// Legacy Modbus error (string wrapper, for backwards compatibility)
    #[error("Modbus error: {0}")]
    ModbusLegacy(String),

    #[error("GPIO error: {0}")]
    Gpio(String),

    #[error("Persistence error: {0}")]
    Persistence(#[from] PersistenceError),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Device not activated")]
    NotActivated,

    #[error("Token expired")]
    TokenExpired,

    #[error("Token already used")]
    TokenAlreadyUsed,

    #[error("Device not found")]
    DeviceNotFound,

    #[error("Device decommissioned")]
    DeviceDecommissioned,

    #[error("Invalid token")]
    InvalidToken,

    #[error("Rate limited")]
    RateLimited,

    #[error("Internal server error")]
    InternalServerError,

    #[error("Unknown error: {0}")]
    Unknown(String),
}

impl AgentError {
    /// Create a legacy Modbus error from string (backwards compatibility)
    pub fn modbus_legacy(msg: impl Into<String>) -> Self {
        Self::ModbusLegacy(msg.into())
    }
}

/// Activation error codes (matches backend)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationErrorCode {
    InvalidToken,
    TokenExpired,
    TokenAlreadyUsed,
    DeviceNotFound,
    DeviceDecommissioned,
    RateLimited,
    InternalError,
}

impl ActivationErrorCode {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "INVALID_TOKEN" => Some(Self::InvalidToken),
            "TOKEN_EXPIRED" => Some(Self::TokenExpired),
            "TOKEN_ALREADY_USED" => Some(Self::TokenAlreadyUsed),
            "DEVICE_NOT_FOUND" => Some(Self::DeviceNotFound),
            "DEVICE_DECOMMISSIONED" => Some(Self::DeviceDecommissioned),
            "RATE_LIMITED" => Some(Self::RateLimited),
            "INTERNAL_ERROR" => Some(Self::InternalError),
            _ => None,
        }
    }
}

impl From<ActivationErrorCode> for AgentError {
    fn from(code: ActivationErrorCode) -> Self {
        match code {
            ActivationErrorCode::InvalidToken => AgentError::InvalidToken,
            ActivationErrorCode::TokenExpired => AgentError::TokenExpired,
            ActivationErrorCode::TokenAlreadyUsed => AgentError::TokenAlreadyUsed,
            ActivationErrorCode::DeviceNotFound => AgentError::DeviceNotFound,
            ActivationErrorCode::DeviceDecommissioned => AgentError::DeviceDecommissioned,
            ActivationErrorCode::RateLimited => AgentError::RateLimited,
            ActivationErrorCode::InternalError => AgentError::InternalServerError,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_modbus_error_display() {
        let err = ModbusError::Connection("host unreachable".to_string());
        assert!(err.to_string().contains("Connection failed"));

        let err = ModbusError::ConnectionTimeout(5000);
        assert!(err.to_string().contains("5000ms"));

        let err = ModbusError::FunctionCodeNotAllowed(6);
        assert!(err.to_string().contains("Function code 6"));
    }

    #[test]
    fn test_modbus_exception_descriptions() {
        let err = ModbusError::exception(0x01);
        assert!(err.to_string().contains("Illegal function"));

        let err = ModbusError::exception(0x02);
        assert!(err.to_string().contains("Illegal data address"));

        let err = ModbusError::exception(0x04);
        assert!(err.to_string().contains("Slave device failure"));

        let err = ModbusError::exception(0xFF); // Unknown
        assert!(err.to_string().contains("Unknown exception"));
    }

    #[test]
    fn test_modbus_error_recoverable() {
        // Recoverable errors
        assert!(ModbusError::ConnectionTimeout(1000).is_recoverable());
        assert!(ModbusError::OperationTimeout(1000).is_recoverable());
        assert!(ModbusError::RateLimited.is_recoverable());
        assert!(ModbusError::exception(0x05).is_recoverable()); // ACK
        assert!(ModbusError::exception(0x06).is_recoverable()); // Busy

        // Non-recoverable errors
        assert!(!ModbusError::FunctionCodeNotAllowed(6).is_recoverable());
        assert!(!ModbusError::WriteNotAllowed.is_recoverable());
        assert!(!ModbusError::NotConnected.is_recoverable());
        assert!(!ModbusError::exception(0x01).is_recoverable()); // Illegal function
    }

    #[test]
    fn test_modbus_error_security_violation() {
        // Security violations
        assert!(ModbusError::FunctionCodeNotAllowed(6).is_security_violation());
        assert!(ModbusError::WriteNotAllowed.is_security_violation());
        assert!(ModbusError::RegisterCountExceeded(200, 100).is_security_violation());

        // Not security violations
        assert!(!ModbusError::ConnectionTimeout(1000).is_security_violation());
        assert!(!ModbusError::RateLimited.is_security_violation());
        assert!(!ModbusError::NotConnected.is_security_violation());
    }

    #[test]
    fn test_modbus_error_into_agent_error() {
        let modbus_err = ModbusError::CircuitBreakerOpen;
        let agent_err: AgentError = modbus_err.into();

        match agent_err {
            AgentError::Modbus(ModbusError::CircuitBreakerOpen) => {}
            _ => panic!("Expected AgentError::Modbus(CircuitBreakerOpen)"),
        }
    }

    #[test]
    fn test_modbus_error_clone() {
        let err1 = ModbusError::Connection("test".to_string());
        let err2 = err1.clone();
        assert_eq!(err1.to_string(), err2.to_string());
    }
}
