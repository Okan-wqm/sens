//! Configuration management for Suderra Edge Agent
//!
//! Handles loading and saving of agent configuration from YAML files.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tracing::{debug, info};

/// Default config file path
const DEFAULT_CONFIG_PATH: &str = "/etc/suderra/config.yaml";

/// Agent configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Unique device identifier (UUID)
    pub device_id: String,

    /// Human-readable device code (e.g., "RPI-A1B2C3D4")
    pub device_code: String,

    /// Provisioning token (cleared after activation)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provisioning_token: Option<String>,

    /// Cloud API URL
    pub api_url: String,

    /// MQTT configuration
    pub mqtt: MqttConfig,

    /// Telemetry configuration
    #[serde(default)]
    pub telemetry: TelemetryConfig,

    /// Logging configuration
    #[serde(default)]
    pub logging: LoggingConfig,

    /// Tenant ID (set after activation)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant_id: Option<String>,

    /// Modbus configuration
    #[serde(default)]
    pub modbus: Vec<ModbusDeviceConfig>,

    /// GPIO configuration
    #[serde(default)]
    pub gpio: Vec<GpioConfig>,

    /// Scripting configuration
    #[serde(default)]
    pub scripting: ScriptingConfig,

    /// Runtime/resilience configuration
    #[serde(default)]
    pub runtime: RuntimeConfig,
}

/// MQTT TLS configuration (IEC 62443 SL2 FR4: Data Confidentiality)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MqttTlsConfig {
    /// Enable TLS encryption
    #[serde(default)]
    pub enabled: bool,

    /// CA certificate path for server verification
    pub ca_cert_path: Option<String>,

    /// Client certificate path (for mTLS)
    pub client_cert_path: Option<String>,

    /// Client private key path (for mTLS)
    pub client_key_path: Option<String>,

    /// Verify server hostname against certificate
    #[serde(default = "default_true")]
    pub verify_hostname: bool,
}

/// MQTT configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MqttConfig {
    /// MQTT broker hostname
    pub broker: Option<String>,

    /// MQTT broker port (1883 for plain, 8883 for TLS)
    #[serde(default = "default_mqtt_port")]
    pub port: u16,

    /// MQTT username (set after activation)
    pub username: Option<String>,

    /// MQTT password (set after activation)
    pub password: Option<String>,

    /// TLS configuration (optional, IEC 62443 SL2)
    #[serde(default)]
    pub tls: MqttTlsConfig,

    /// Topic patterns (v1.1 - tenant-prefixed)
    #[serde(default)]
    pub topics: MqttTopics,

    /// Keep-alive interval in seconds
    #[serde(default = "default_keepalive")]
    pub keepalive_secs: u64,

    /// Clean session flag (false to preserve QoS 1/2 messages)
    #[serde(default = "default_true")]
    pub clean_session: bool,

    /// Last Will topic for device status (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_will_topic: Option<String>,
}

/// MQTT topic patterns
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MqttTopics {
    /// Status topic pattern
    #[serde(default = "default_status_topic")]
    pub status: String,

    /// Telemetry topic pattern
    #[serde(default = "default_telemetry_topic")]
    pub telemetry: String,

    /// Responses topic pattern
    #[serde(default = "default_responses_topic")]
    pub responses: String,

    /// Commands topic pattern (subscribe)
    #[serde(default = "default_commands_topic")]
    pub commands: String,

    /// Config topic pattern (subscribe)
    #[serde(default = "default_config_topic")]
    pub config: String,
}

impl Default for MqttTopics {
    fn default() -> Self {
        Self {
            status: default_status_topic(),
            telemetry: default_telemetry_topic(),
            responses: default_responses_topic(),
            commands: default_commands_topic(),
            config: default_config_topic(),
        }
    }
}

impl MqttTopics {
    /// Resolve topic pattern with actual tenant_id and device_id
    pub fn resolve(&self, tenant_id: &str, device_id: &str) -> ResolvedTopics {
        ResolvedTopics {
            status: self
                .status
                .replace("{tenant_id}", tenant_id)
                .replace("{device_id}", device_id),
            telemetry: self
                .telemetry
                .replace("{tenant_id}", tenant_id)
                .replace("{device_id}", device_id),
            responses: self
                .responses
                .replace("{tenant_id}", tenant_id)
                .replace("{device_id}", device_id),
            commands: self
                .commands
                .replace("{tenant_id}", tenant_id)
                .replace("{device_id}", device_id),
            config: self
                .config
                .replace("{tenant_id}", tenant_id)
                .replace("{device_id}", device_id),
        }
    }
}

/// Resolved MQTT topics with actual values
#[derive(Debug, Clone)]
pub struct ResolvedTopics {
    pub status: String,
    pub telemetry: String,
    pub responses: String,
    pub commands: String,
    pub config: String,
}

/// Telemetry configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryConfig {
    /// Telemetry interval in seconds
    #[serde(default = "default_telemetry_interval")]
    pub interval_seconds: u64,

    /// Include CPU metrics
    #[serde(default = "default_true")]
    pub include_cpu: bool,

    /// Include memory metrics
    #[serde(default = "default_true")]
    pub include_memory: bool,

    /// Include disk metrics
    #[serde(default = "default_true")]
    pub include_disk: bool,

    /// Include temperature metrics
    #[serde(default = "default_true")]
    pub include_temperature: bool,

    /// Include system metrics (uptime, load average)
    #[serde(default = "default_true")]
    pub include_system: bool,

    /// Include Modbus device readings
    #[serde(default = "default_true")]
    pub include_modbus: bool,

    /// Include GPIO pin states
    #[serde(default = "default_true")]
    pub include_gpio: bool,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            interval_seconds: default_telemetry_interval(),
            include_cpu: true,
            include_memory: true,
            include_disk: true,
            include_temperature: true,
            include_system: true,
            include_modbus: true,
            include_gpio: true,
        }
    }
}

/// Logging configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// Log level (trace, debug, info, warn, error)
    #[serde(default = "default_log_level")]
    pub level: String,

    /// Log file path
    #[serde(default = "default_log_file")]
    pub file: String,
}

/// Scripting configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptingConfig {
    /// Enable script execution
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Default scan cycle interval in milliseconds (10-10000)
    #[serde(default = "default_scan_cycle_ms")]
    pub default_scan_cycle_ms: u64,

    /// Minimum allowed scan cycle (ms)
    #[serde(default = "default_min_scan_cycle_ms")]
    pub min_scan_cycle_ms: u64,

    /// Maximum allowed scan cycle (ms)
    #[serde(default = "default_max_scan_cycle_ms")]
    pub max_scan_cycle_ms: u64,

    /// Maximum function blocks per program
    #[serde(default = "default_max_function_blocks")]
    pub max_function_blocks: usize,

    /// Maximum script execution depth (nested calls)
    #[serde(default = "default_max_execution_depth")]
    pub max_execution_depth: usize,

    /// Maximum actions per script execution
    #[serde(default = "default_max_actions")]
    pub max_actions: usize,

    /// Maximum execution time per script (seconds)
    #[serde(default = "default_max_execution_time_secs")]
    pub max_execution_time_secs: u64,
}

impl Default for ScriptingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_scan_cycle_ms: default_scan_cycle_ms(),
            min_scan_cycle_ms: default_min_scan_cycle_ms(),
            max_scan_cycle_ms: default_max_scan_cycle_ms(),
            max_function_blocks: default_max_function_blocks(),
            max_execution_depth: default_max_execution_depth(),
            max_actions: default_max_actions(),
            max_execution_time_secs: default_max_execution_time_secs(),
        }
    }
}

/// Runtime/resilience configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfig {
    /// Command rate limit: max commands per window
    #[serde(default = "default_rate_limit_max")]
    pub rate_limit_max_commands: usize,

    /// Command rate limit: window in seconds
    #[serde(default = "default_rate_limit_window_secs")]
    pub rate_limit_window_secs: u64,

    /// GPIO operation timeout in seconds
    #[serde(default = "default_gpio_timeout_secs")]
    pub gpio_timeout_secs: u64,

    /// Modbus operation timeout in seconds
    #[serde(default = "default_modbus_timeout_secs")]
    pub modbus_timeout_secs: u64,

    /// Modbus connection timeout in seconds
    #[serde(default = "default_modbus_connect_timeout_secs")]
    pub modbus_connect_timeout_secs: u64,

    /// Circuit breaker recovery time in seconds
    #[serde(default = "default_circuit_breaker_recovery_secs")]
    pub circuit_breaker_recovery_secs: u64,

    /// Provisioning API timeout in seconds
    #[serde(default = "default_provisioning_timeout_secs")]
    pub provisioning_timeout_secs: u64,

    /// Shutdown timeout in seconds
    #[serde(default = "default_shutdown_timeout_secs")]
    pub shutdown_timeout_secs: u64,

    /// MQTT reconnect minimum delay in seconds
    #[serde(default = "default_mqtt_reconnect_min_secs")]
    pub mqtt_reconnect_min_secs: u64,

    /// MQTT reconnect maximum delay in seconds
    #[serde(default = "default_mqtt_reconnect_max_secs")]
    pub mqtt_reconnect_max_secs: u64,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            rate_limit_max_commands: default_rate_limit_max(),
            rate_limit_window_secs: default_rate_limit_window_secs(),
            gpio_timeout_secs: default_gpio_timeout_secs(),
            modbus_timeout_secs: default_modbus_timeout_secs(),
            modbus_connect_timeout_secs: default_modbus_connect_timeout_secs(),
            circuit_breaker_recovery_secs: default_circuit_breaker_recovery_secs(),
            provisioning_timeout_secs: default_provisioning_timeout_secs(),
            shutdown_timeout_secs: default_shutdown_timeout_secs(),
            mqtt_reconnect_min_secs: default_mqtt_reconnect_min_secs(),
            mqtt_reconnect_max_secs: default_mqtt_reconnect_max_secs(),
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            file: default_log_file(),
        }
    }
}

/// Modbus device configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModbusDeviceConfig {
    /// Device name/identifier
    pub name: String,

    /// Connection type: "tcp" or "rtu"
    pub connection_type: String,

    /// TCP: hostname:port, RTU: serial port path
    pub address: String,

    /// Modbus slave ID
    #[serde(default = "default_slave_id")]
    pub slave_id: u8,

    /// Baud rate (RTU only)
    pub baud_rate: Option<u32>,

    /// Registers to poll
    #[serde(default)]
    pub registers: Vec<ModbusRegisterConfig>,
}

/// Byte order for multi-register values
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ByteOrder {
    /// Big Endian (AB CD) - Most common for Modbus
    BigEndian,
    /// Little Endian (CD AB)
    LittleEndian,
    /// Big Endian byte swap (BA DC)
    BigEndianByteSwap,
    /// Little Endian byte swap (DC BA)
    LittleEndianByteSwap,
}

impl Default for ByteOrder {
    fn default() -> Self {
        ByteOrder::BigEndian
    }
}

/// Modbus register configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModbusRegisterConfig {
    /// Register name/tag
    pub name: String,

    /// Register address
    pub address: u16,

    /// Register type: "holding", "input", "coil", "discrete"
    pub register_type: String,

    /// Data type: "u16", "i16", "u32", "i32", "f32"
    #[serde(default = "default_data_type")]
    pub data_type: String,

    /// Byte order for multi-register values (u32, i32, f32)
    /// Options: big_endian, little_endian, big_endian_byte_swap, little_endian_byte_swap
    #[serde(default)]
    pub byte_order: ByteOrder,

    /// Scale factor
    #[serde(default = "default_scale")]
    pub scale: f64,

    /// Engineering unit
    pub unit: Option<String>,

    /// Poll interval in milliseconds (overrides device default)
    pub poll_interval_ms: Option<u64>,
}

/// GPIO pin configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpioConfig {
    /// Pin name/tag
    pub name: String,

    /// GPIO pin number
    pub pin: u8,

    /// Direction: "input" or "output"
    pub direction: String,

    /// Pull-up/down: "up", "down", "none"
    #[serde(default = "default_pull")]
    pub pull: String,

    /// Invert value
    #[serde(default)]
    pub invert: bool,

    /// Debounce time in milliseconds (input only)
    pub debounce_ms: Option<u64>,
}

// Default value functions
fn default_mqtt_port() -> u16 {
    1883
}
fn default_keepalive() -> u64 {
    30
}
fn default_true() -> bool {
    true
}
fn default_telemetry_interval() -> u64 {
    30
}
fn default_log_level() -> String {
    "info".to_string()
}
fn default_log_file() -> String {
    "/var/log/suderra-agent.log".to_string()
}
fn default_slave_id() -> u8 {
    1
}
fn default_data_type() -> String {
    "u16".to_string()
}
fn default_scale() -> f64 {
    1.0
}
fn default_pull() -> String {
    "none".to_string()
}

// Scripting defaults
fn default_scan_cycle_ms() -> u64 {
    100
}
fn default_min_scan_cycle_ms() -> u64 {
    10
}
fn default_max_scan_cycle_ms() -> u64 {
    10000
}
fn default_max_function_blocks() -> usize {
    100
}
fn default_max_execution_depth() -> usize {
    10
}
fn default_max_actions() -> usize {
    100
}
fn default_max_execution_time_secs() -> u64 {
    30
}

// Runtime/resilience defaults
fn default_rate_limit_max() -> usize {
    60
}
fn default_rate_limit_window_secs() -> u64 {
    60
}
fn default_gpio_timeout_secs() -> u64 {
    5
}
fn default_modbus_timeout_secs() -> u64 {
    5
}
fn default_modbus_connect_timeout_secs() -> u64 {
    10
}
fn default_circuit_breaker_recovery_secs() -> u64 {
    30
}
fn default_provisioning_timeout_secs() -> u64 {
    30
}
fn default_shutdown_timeout_secs() -> u64 {
    10
}
fn default_mqtt_reconnect_min_secs() -> u64 {
    1
}
fn default_mqtt_reconnect_max_secs() -> u64 {
    60
}

// Default topic patterns (v1.1 spec)
fn default_status_topic() -> String {
    "tenants/{tenant_id}/devices/{device_id}/status".to_string()
}
fn default_telemetry_topic() -> String {
    "tenants/{tenant_id}/devices/{device_id}/telemetry".to_string()
}
fn default_responses_topic() -> String {
    "tenants/{tenant_id}/devices/{device_id}/responses".to_string()
}
fn default_commands_topic() -> String {
    "tenants/{tenant_id}/devices/{device_id}/commands".to_string()
}
fn default_config_topic() -> String {
    "tenants/{tenant_id}/devices/{device_id}/config".to_string()
}

impl AgentConfig {
    /// Load configuration from file
    pub fn load() -> Result<Self> {
        Self::load_from(DEFAULT_CONFIG_PATH)
    }

    /// Load configuration from specified path
    pub fn load_from(path: &str) -> Result<Self> {
        let path = PathBuf::from(path);

        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;

        let config: AgentConfig = serde_yaml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", path.display()))?;

        // Validate configuration
        config.validate()?;

        Ok(config)
    }

    /// Validate configuration values
    ///
    /// # Security
    /// This validates all configuration parameters to prevent:
    /// - Invalid device IDs that could cause issues
    /// - Invalid GPIO pins that don't exist on hardware
    /// - Invalid Modbus slave IDs outside protocol range
    /// - Invalid port numbers
    pub fn validate(&self) -> Result<()> {
        // Validate device_id is not empty
        if self.device_id.trim().is_empty() {
            anyhow::bail!("device_id cannot be empty");
        }

        // Validate device_code is not empty
        if self.device_code.trim().is_empty() {
            anyhow::bail!("device_code cannot be empty");
        }

        // Validate API URL format
        if !self.api_url.starts_with("http://") && !self.api_url.starts_with("https://") {
            anyhow::bail!("api_url must start with http:// or https://");
        }

        // Validate MQTT port if configured
        if let Some(ref _broker) = self.mqtt.broker {
            if self.mqtt.port == 0 {
                anyhow::bail!("MQTT port cannot be 0");
            }
        }

        // Validate GPIO pins (Raspberry Pi has GPIO 0-27)
        for gpio in &self.gpio {
            if gpio.pin > 27 {
                anyhow::bail!(
                    "Invalid GPIO pin {}: Raspberry Pi only has GPIO 0-27",
                    gpio.pin
                );
            }

            // Validate direction
            let valid_directions = ["input", "output", "in", "out"];
            if !valid_directions.contains(&gpio.direction.to_lowercase().as_str()) {
                anyhow::bail!(
                    "Invalid GPIO direction '{}' for pin {}: must be 'input' or 'output'",
                    gpio.direction,
                    gpio.pin
                );
            }

            // Validate pull mode
            let valid_pulls = ["up", "down", "none", ""];
            if !valid_pulls.contains(&gpio.pull.to_lowercase().as_str()) {
                anyhow::bail!(
                    "Invalid GPIO pull mode '{}' for pin {}: must be 'up', 'down', or 'none'",
                    gpio.pull,
                    gpio.pin
                );
            }
        }

        // Validate Modbus devices
        for device in &self.modbus {
            // Validate slave_id (Modbus uses 1-247, 0 is broadcast)
            if device.slave_id == 0 || device.slave_id > 247 {
                anyhow::bail!(
                    "Invalid Modbus slave_id {} for device '{}': must be 1-247",
                    device.slave_id,
                    device.name
                );
            }

            // Validate connection_type
            let valid_types = ["tcp", "rtu"];
            if !valid_types.contains(&device.connection_type.to_lowercase().as_str()) {
                anyhow::bail!(
                    "Invalid Modbus connection_type '{}' for device '{}': must be 'tcp' or 'rtu'",
                    device.connection_type,
                    device.name
                );
            }

            // Validate address is not empty
            if device.address.trim().is_empty() {
                anyhow::bail!("Modbus device '{}' has empty address", device.name);
            }
        }

        // Validate telemetry interval (minimum 5 seconds, maximum 1 hour)
        if self.telemetry.interval_seconds < 5 {
            anyhow::bail!(
                "Telemetry interval {} is too low: minimum is 5 seconds",
                self.telemetry.interval_seconds
            );
        }
        if self.telemetry.interval_seconds > 3600 {
            anyhow::bail!(
                "Telemetry interval {} is too high: maximum is 3600 seconds (1 hour)",
                self.telemetry.interval_seconds
            );
        }

        debug!("Configuration validation passed");
        Ok(())
    }

    /// Save configuration to file
    pub fn save(&self) -> Result<()> {
        self.save_to(DEFAULT_CONFIG_PATH)
    }

    /// Save configuration to specified path
    ///
    /// # Security
    /// On Unix systems, this sets file permissions to 0600 (owner read/write only)
    /// to protect sensitive credentials stored in the config file.
    pub fn save_to(&self, path: &str) -> Result<()> {
        let path = PathBuf::from(path);

        let content = serde_yaml::to_string(self).context("Failed to serialize config")?;

        fs::write(&path, &content)
            .with_context(|| format!("Failed to write config file: {}", path.display()))?;

        // Set restrictive permissions on Unix to protect credentials
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let permissions = fs::Permissions::from_mode(0o600);
            fs::set_permissions(&path, permissions).with_context(|| {
                format!(
                    "Failed to set permissions on config file: {}",
                    path.display()
                )
            })?;
            debug!("Set config file permissions to 0600");
        }

        info!("Configuration saved to {}", path.display());
        Ok(())
    }

    /// Get resolved MQTT topics
    pub fn get_resolved_topics(&self) -> Option<ResolvedTopics> {
        let tenant_id = self.tenant_id.as_ref()?;
        Some(self.mqtt.topics.resolve(tenant_id, &self.device_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_topic_resolution() {
        let topics = MqttTopics::default();
        let resolved = topics.resolve("tenant-123", "device-456");

        assert_eq!(
            resolved.status,
            "tenants/tenant-123/devices/device-456/status"
        );
        assert_eq!(
            resolved.telemetry,
            "tenants/tenant-123/devices/device-456/telemetry"
        );
        assert_eq!(
            resolved.commands,
            "tenants/tenant-123/devices/device-456/commands"
        );
    }
}
