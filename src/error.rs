//! Error types for Suderra Edge Agent

use thiserror::Error;

use crate::scripting::PersistenceError;

/// Agent error types
#[derive(Error, Debug)]
pub enum AgentError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Provisioning error: {0}")]
    Provisioning(String),

    #[error("MQTT error: {0}")]
    Mqtt(String),

    #[error("Modbus error: {0}")]
    Modbus(String),

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
