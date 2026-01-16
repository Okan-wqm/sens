//! Device provisioning and activation
//!
//! Handles the zero-touch provisioning flow:
//! 1. Collect device fingerprint
//! 2. Send activation request to cloud API
//! 3. Receive MQTT credentials
//! 4. Update local config

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::error::{ActivationErrorCode, AgentError};
use crate::AppState;

/// Provisioning client for device activation
pub struct ProvisioningClient {
    state: Arc<RwLock<AppState>>,
    http_client: reqwest::Client,
}

/// Device fingerprint collected from hardware
#[derive(Debug, Clone, Serialize)]
pub struct DeviceFingerprint {
    /// CPU serial number (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_serial: Option<String>,

    /// MAC addresses of network interfaces
    pub mac_addresses: Vec<String>,

    /// Machine ID (from /etc/machine-id)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub machine_id: Option<String>,

    /// Hostname
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
}

/// Activation request sent to cloud API
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationRequest {
    pub device_id: String,
    pub token: String,
    pub fingerprint: DeviceFingerprint,
    pub agent_version: String,
}

/// Activation response from cloud API (snake_case per v1.1 spec)
#[derive(Debug, Deserialize)]
pub struct ActivationResponse {
    pub success: bool,
    pub mqtt_broker: String,
    pub mqtt_port: u16,
    pub mqtt_username: String,
    pub mqtt_password: String,
    pub tenant_id: String,
    pub device_code: String,
    #[serde(default)]
    pub config: Option<serde_json::Value>,
}

/// Error response from cloud API
#[derive(Debug, Deserialize)]
pub struct ActivationErrorResponse {
    pub success: bool,
    pub error: String,
    #[serde(rename = "errorCode")]
    pub error_code: String,
}

impl ProvisioningClient {
    /// Create a new provisioning client
    pub fn new(state: Arc<RwLock<AppState>>) -> Result<Self> {
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .context("Failed to create HTTP client. This typically indicates a TLS/SSL configuration issue.")?;

        Ok(Self { state, http_client })
    }

    /// Activate device with cloud platform
    pub async fn activate(&self) -> Result<ActivationResponse> {
        let (api_url, device_id, token) = {
            let state = self.state.read().await;
            let token = state
                .config
                .provisioning_token
                .clone()
                .ok_or_else(|| AgentError::NotActivated)?;

            (
                state.config.api_url.clone(),
                state.config.device_id.clone(),
                token,
            )
        };

        // Collect device fingerprint
        info!("Collecting device fingerprint...");
        let fingerprint = self.collect_fingerprint().await;
        debug!("Fingerprint: {:?}", fingerprint);

        // Build activation request
        let request = ActivationRequest {
            device_id: device_id.clone(),
            token,
            fingerprint,
            agent_version: env!("CARGO_PKG_VERSION").to_string(),
        };

        // Send activation request
        let url = format!("{}/api/devices/activate", api_url);
        info!("Sending activation request to {}", url);

        let response = self
            .http_client
            .post(&url)
            .json(&request)
            .send()
            .await
            .context("Failed to send activation request")?;

        let status = response.status();
        let body = response
            .text()
            .await
            .context("Failed to read response body")?;

        debug!("Response status: {}, body: {}", status, body);

        // Handle response
        if status.is_success() {
            let activation: ActivationResponse =
                serde_json::from_str(&body).context("Failed to parse activation response")?;

            if activation.success {
                info!("Activation successful for device {}", device_id);
                return Ok(activation);
            }
        }

        // Try to parse error response
        if let Ok(error_response) = serde_json::from_str::<ActivationErrorResponse>(&body) {
            warn!(
                "Activation failed: {} ({})",
                error_response.error, error_response.error_code
            );

            if let Some(code) = ActivationErrorCode::from_str(&error_response.error_code) {
                return Err(AgentError::from(code).into());
            }

            return Err(AgentError::Provisioning(error_response.error).into());
        }

        // Unknown error
        error!(
            "Activation failed with status {} and body: {}",
            status, body
        );
        Err(AgentError::Unknown(format!("HTTP {}: {}", status, body)).into())
    }

    /// Collect device fingerprint
    async fn collect_fingerprint(&self) -> DeviceFingerprint {
        DeviceFingerprint {
            cpu_serial: Self::get_cpu_serial(),
            mac_addresses: Self::get_mac_addresses(),
            machine_id: Self::get_machine_id(),
            hostname: Self::get_hostname(),
        }
    }

    /// Get CPU serial number (Raspberry Pi specific)
    fn get_cpu_serial() -> Option<String> {
        #[cfg(target_os = "linux")]
        {
            // Try to read from /proc/cpuinfo (Raspberry Pi)
            if let Ok(content) = std::fs::read_to_string("/proc/cpuinfo") {
                for line in content.lines() {
                    if line.starts_with("Serial") {
                        if let Some(serial) = line.split(':').nth(1) {
                            return Some(serial.trim().to_string());
                        }
                    }
                }
            }
        }
        None
    }

    /// Get MAC addresses of all network interfaces
    fn get_mac_addresses() -> Vec<String> {
        let mut addresses = Vec::new();

        if let Ok(mac) = mac_address::get_mac_address() {
            if let Some(addr) = mac {
                addresses.push(addr.to_string());
            }
        }

        // Also try to get all interfaces
        if let Ok(macs) = mac_address::mac_address_by_name("eth0") {
            if let Some(addr) = macs {
                if !addresses.contains(&addr.to_string()) {
                    addresses.push(addr.to_string());
                }
            }
        }

        if let Ok(macs) = mac_address::mac_address_by_name("wlan0") {
            if let Some(addr) = macs {
                if !addresses.contains(&addr.to_string()) {
                    addresses.push(addr.to_string());
                }
            }
        }

        addresses
    }

    /// Get machine ID from /etc/machine-id
    fn get_machine_id() -> Option<String> {
        // Try machine-uid crate first
        if let Ok(uid) = machine_uid::get() {
            return Some(uid);
        }

        // Fallback to reading /etc/machine-id directly
        #[cfg(target_os = "linux")]
        {
            if let Ok(id) = std::fs::read_to_string("/etc/machine-id") {
                return Some(id.trim().to_string());
            }
        }

        None
    }

    /// Get hostname
    fn get_hostname() -> Option<String> {
        hostname::get().ok().and_then(|h| h.into_string().ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fingerprint_serialization() {
        let fingerprint = DeviceFingerprint {
            cpu_serial: Some("0000000012345678".to_string()),
            mac_addresses: vec!["AA:BB:CC:DD:EE:FF".to_string()],
            machine_id: Some("abc123".to_string()),
            hostname: Some("edge-device".to_string()),
        };

        let json = serde_json::to_string(&fingerprint).unwrap();
        assert!(json.contains("cpu_serial"));
        assert!(json.contains("mac_addresses"));
    }

    #[test]
    fn test_activation_request_serialization() {
        let request = ActivationRequest {
            device_id: "device-123".to_string(),
            token: "secret-token".to_string(),
            fingerprint: DeviceFingerprint {
                cpu_serial: None,
                mac_addresses: vec![],
                machine_id: None,
                hostname: None,
            },
            agent_version: "1.0.0".to_string(),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("deviceId")); // camelCase for request
        assert!(json.contains("agentVersion"));
    }
}
