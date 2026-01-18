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

/// Mask a sensitive token for logging purposes (IEC 62443 SL2 FR3)
///
/// Shows first 4 and last 4 characters with ellipsis in between.
/// For short tokens (< 12 chars), shows only asterisks.
///
/// # Security
/// Prevents token leakage in log files while allowing debugging.
fn mask_token(token: &str) -> String {
    if token.len() >= 12 {
        format!("{}...{}", &token[..4], &token[token.len() - 4..])
    } else if token.len() > 0 {
        "*".repeat(token.len().min(8))
    } else {
        "(empty)".to_string()
    }
}

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
///
/// # Security Note (v1.2.0)
/// Custom Debug implementation masks the token to prevent log leakage.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationRequest {
    pub device_id: String,
    pub token: String,
    pub fingerprint: DeviceFingerprint,
    pub agent_version: String,
}

impl std::fmt::Debug for ActivationRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActivationRequest")
            .field("device_id", &self.device_id)
            .field("token", &mask_token(&self.token))
            .field("fingerprint", &self.fingerprint)
            .field("agent_version", &self.agent_version)
            .finish()
    }
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
        // Log fingerprint collection without exposing full hardware IDs (v1.2.0 security)
        debug!(
            "Fingerprint collected: {} MAC address(es), hostname={:?}",
            fingerprint.mac_addresses.len(),
            fingerprint.hostname.as_deref().unwrap_or("(none)")
        );

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

        // Log response status only - body may contain MQTT credentials (v1.2.0 security)
        debug!(
            "Response status: {}, body_len: {} bytes",
            status,
            body.len()
        );

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

        // Unknown error - truncate body in logs to prevent credential leakage (v1.2.0 security)
        let truncated_body = if body.len() > 100 {
            format!("{}...(truncated)", &body[..100])
        } else {
            body.clone()
        };
        error!(
            "Activation failed with status {}: {}",
            status, truncated_body
        );
        Err(AgentError::Unknown(format!("HTTP {}", status)).into())
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

    #[test]
    fn test_mask_token() {
        // Long token shows first 4 and last 4
        assert_eq!(mask_token("1234567890abcdef"), "1234...cdef");

        // Short token (< 12 chars) shows asterisks
        assert_eq!(mask_token("short"), "*****");

        // Empty token
        assert_eq!(mask_token(""), "(empty)");

        // Exactly 12 chars
        assert_eq!(mask_token("123456789012"), "1234...9012");
    }

    #[test]
    fn test_activation_request_debug_masks_token() {
        let request = ActivationRequest {
            device_id: "device-123".to_string(),
            token: "super-secret-token-12345".to_string(),
            fingerprint: DeviceFingerprint {
                cpu_serial: None,
                mac_addresses: vec![],
                machine_id: None,
                hostname: None,
            },
            agent_version: "1.0.0".to_string(),
        };

        let debug_output = format!("{:?}", request);

        // Debug should NOT contain the actual token
        assert!(!debug_output.contains("super-secret-token-12345"));

        // But should contain the masked version
        assert!(debug_output.contains("supe...2345"));
    }
}
