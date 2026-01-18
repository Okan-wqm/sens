//! Configuration management for Suderra Edge Agent
//!
//! Handles loading and saving of agent configuration from YAML files.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tracing::{debug, info, warn};

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

    /// Cache configuration (v1.2.0)
    #[serde(default)]
    pub cache: CacheConfig,

    /// Circuit breaker configuration (v1.2.0)
    #[serde(default)]
    pub circuit_breaker: CircuitBreakerConfig,
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

/// OpenTelemetry OTLP configuration (optional, requires "telemetry" feature)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OtlpConfig {
    /// OTLP endpoint URL (e.g., "http://localhost:4317")
    /// If not set, OpenTelemetry export is disabled
    pub endpoint: Option<String>,

    /// Service name for traces
    #[serde(default = "default_service_name")]
    pub service_name: String,

    /// Sample ratio (0.0 to 1.0, default 1.0 = sample all)
    #[serde(default = "default_sample_ratio")]
    pub sample_ratio: f64,
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

    /// OpenTelemetry OTLP export configuration (optional)
    #[serde(default)]
    pub otlp: OtlpConfig,
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
            otlp: OtlpConfig::default(),
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

/// Cache configuration for Moka (v1.2.0)
///
/// Used for caching sensor readings, computed values, and script outputs
/// to reduce latency and prevent excessive recomputation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    /// Maximum number of entries in the cache
    #[serde(default = "default_cache_max_capacity")]
    pub max_capacity: u64,

    /// Time-to-live for cache entries in seconds (0 = no TTL)
    #[serde(default = "default_cache_ttl_secs")]
    pub ttl_secs: u64,

    /// Time-to-idle for cache entries in seconds (0 = no TTI)
    /// Entry expires if not accessed within this time
    #[serde(default = "default_cache_tti_secs")]
    pub tti_secs: u64,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            max_capacity: default_cache_max_capacity(),
            ttl_secs: default_cache_ttl_secs(),
            tti_secs: default_cache_tti_secs(),
        }
    }
}

/// Circuit breaker configuration (v1.2.0)
///
/// Controls fault isolation behavior for external service calls
/// (Modbus devices, MQTT, APIs).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerConfig {
    /// Number of failures before opening the circuit
    #[serde(default = "default_cb_failure_threshold")]
    pub failure_threshold: u32,

    /// Number of successes in half-open state to close the circuit
    #[serde(default = "default_cb_success_threshold")]
    pub success_threshold: u32,

    /// Time in seconds to wait before attempting recovery (half-open)
    #[serde(default = "default_circuit_breaker_recovery_secs")]
    pub recovery_secs: u64,

    /// Maximum concurrent requests allowed in half-open state
    #[serde(default = "default_cb_half_open_permits")]
    pub half_open_permits: u32,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: default_cb_failure_threshold(),
            success_threshold: default_cb_success_threshold(),
            recovery_secs: default_circuit_breaker_recovery_secs(),
            half_open_permits: default_cb_half_open_permits(),
        }
    }
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

/// Modbus security configuration (IEC 62443 SL2 FR3/FR5)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModbusSecurityConfig {
    /// Enable security checks
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Allowed Modbus function codes (whitelist)
    /// Default: [1, 2, 3, 4] (read coils, discrete inputs, holding/input registers)
    #[serde(default = "default_modbus_function_whitelist")]
    pub allowed_function_codes: Vec<u8>,

    /// Rate limit: maximum operations per second
    #[serde(default = "default_modbus_rate_limit")]
    pub rate_limit_ops_per_sec: u64,

    /// Rate limit: burst capacity (max concurrent ops)
    #[serde(default = "default_modbus_burst_capacity")]
    pub rate_limit_burst: u64,

    /// Maximum register count per read operation
    #[serde(default = "default_max_register_count")]
    pub max_register_count: u16,

    /// Allow write operations (coils and registers)
    #[serde(default)]
    pub allow_writes: bool,
}

impl Default for ModbusSecurityConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allowed_function_codes: default_modbus_function_whitelist(),
            rate_limit_ops_per_sec: default_modbus_rate_limit(),
            rate_limit_burst: default_modbus_burst_capacity(),
            max_register_count: default_max_register_count(),
            allow_writes: false,
        }
    }
}

/// Modbus TLS configuration (v1.2.0 - IEC 62443 SL2 FR4: Data Confidentiality)
///
/// Enables encrypted Modbus/TCP communication using TLS.
/// Supports both server authentication and mutual TLS (mTLS).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModbusTlsConfig {
    /// Enable TLS encryption for Modbus TCP
    #[serde(default)]
    pub enabled: bool,

    /// Server name for SNI (Server Name Indication)
    /// Required when connecting to TLS-enabled Modbus servers
    pub server_name: Option<String>,

    /// CA certificate path for server verification
    pub ca_cert_path: Option<String>,

    /// Client certificate path (for mutual TLS / mTLS)
    pub client_cert_path: Option<String>,

    /// Client private key path (for mutual TLS / mTLS)
    pub client_key_path: Option<String>,

    /// Skip server certificate verification (NOT recommended for production)
    #[serde(default)]
    pub insecure_skip_verify: bool,
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

    /// Security configuration (optional, uses global defaults if not specified)
    #[serde(default)]
    pub security: ModbusSecurityConfig,

    /// TLS configuration for encrypted Modbus TCP (v1.2.0)
    #[serde(default)]
    pub tls: ModbusTlsConfig,
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

// OpenTelemetry OTLP defaults
fn default_service_name() -> String {
    "suderra-agent".to_string()
}
fn default_sample_ratio() -> f64 {
    1.0 // Sample all traces by default
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

// Modbus security defaults (IEC 62443 SL2)
fn default_modbus_function_whitelist() -> Vec<u8> {
    // Only allow read operations by default:
    // FC 1: Read Coils
    // FC 2: Read Discrete Inputs
    // FC 3: Read Holding Registers
    // FC 4: Read Input Registers
    vec![1, 2, 3, 4]
}
fn default_modbus_rate_limit() -> u64 {
    10 // 10 operations per second (conservative default)
}
fn default_modbus_burst_capacity() -> u64 {
    20 // Allow burst of 20 operations
}
fn default_max_register_count() -> u16 {
    125 // Modbus protocol max is 125 for holding/input registers
}

// Cache defaults (v1.2.0)
fn default_cache_max_capacity() -> u64 {
    1000
}
fn default_cache_ttl_secs() -> u64 {
    3600 // 1 hour
}
fn default_cache_tti_secs() -> u64 {
    1800 // 30 minutes
}

// Circuit breaker defaults (v1.2.0)
fn default_cb_failure_threshold() -> u32 {
    3
}
fn default_cb_success_threshold() -> u32 {
    2
}
fn default_cb_half_open_permits() -> u32 {
    1
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

            // Validate Modbus TLS configuration (v1.2.0 - IEC 62443 SL2 FR4)
            if device.tls.enabled {
                // TLS only supported for TCP connections
                if device.connection_type.to_lowercase() != "tcp" {
                    anyhow::bail!(
                        "Modbus device '{}': TLS is only supported for TCP connections",
                        device.name
                    );
                }

                // Server name required for TLS
                if device.tls.server_name.is_none() && !device.tls.insecure_skip_verify {
                    anyhow::bail!(
                        "Modbus device '{}': server_name required for TLS (or set insecure_skip_verify)",
                        device.name
                    );
                }

                // Validate certificate paths if provided
                if let Some(ref ca_path) = device.tls.ca_cert_path {
                    if !std::path::Path::new(ca_path).exists() {
                        anyhow::bail!(
                            "Modbus device '{}': CA certificate not found: {}",
                            device.name,
                            ca_path
                        );
                    }
                }
                if let Some(ref cert_path) = device.tls.client_cert_path {
                    if !std::path::Path::new(cert_path).exists() {
                        anyhow::bail!(
                            "Modbus device '{}': client certificate not found: {}",
                            device.name,
                            cert_path
                        );
                    }
                }
                if let Some(ref key_path) = device.tls.client_key_path {
                    if !std::path::Path::new(key_path).exists() {
                        anyhow::bail!(
                            "Modbus device '{}': client key not found: {}",
                            device.name,
                            key_path
                        );
                    }
                }
                // Validate mTLS consistency
                if device.tls.client_cert_path.is_some() != device.tls.client_key_path.is_some() {
                    anyhow::bail!(
                        "Modbus device '{}': mTLS requires both client_cert_path and client_key_path",
                        device.name
                    );
                }

                // Warn about insecure configuration
                if device.tls.insecure_skip_verify {
                    warn!(
                        "Modbus device '{}': TLS verification disabled - NOT recommended for production",
                        device.name
                    );
                }
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

        // Validate MQTT TLS certificate paths exist (IEC 62443 SL2 FR4)
        // v1.2.0: Fail-fast if TLS is enabled but certificates are missing
        if self.mqtt.tls.enabled {
            if let Some(ref ca_path) = self.mqtt.tls.ca_cert_path {
                if !std::path::Path::new(ca_path).exists() {
                    anyhow::bail!("MQTT CA certificate not found: {}", ca_path);
                }
            } else {
                // No custom CA - system CA store will be used
                warn!("MQTT TLS enabled without custom CA certificate - using system CA store");
            }
            if let Some(ref cert_path) = self.mqtt.tls.client_cert_path {
                if !std::path::Path::new(cert_path).exists() {
                    anyhow::bail!("MQTT client certificate not found: {}", cert_path);
                }
            }
            if let Some(ref key_path) = self.mqtt.tls.client_key_path {
                if !std::path::Path::new(key_path).exists() {
                    anyhow::bail!("MQTT client key not found: {}", key_path);
                }
            }
            // Validate mTLS consistency - if one is set, both must be set
            if self.mqtt.tls.client_cert_path.is_some() != self.mqtt.tls.client_key_path.is_some() {
                anyhow::bail!(
                    "MQTT mTLS requires both client_cert_path and client_key_path to be set"
                );
            }
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

    // ========================================================================
    // Cache Config Tests (v1.2.0)
    // ========================================================================

    #[test]
    fn test_cache_config_defaults() {
        let config = CacheConfig::default();

        assert_eq!(config.max_capacity, 1000);
        assert_eq!(config.ttl_secs, 3600); // 1 hour
        assert_eq!(config.tti_secs, 1800); // 30 minutes
    }

    #[test]
    fn test_cache_config_serialization() {
        let config = CacheConfig {
            max_capacity: 5000,
            ttl_secs: 7200,
            tti_secs: 3600,
        };

        let yaml = serde_yaml::to_string(&config).unwrap();
        assert!(yaml.contains("max_capacity: 5000"));
        assert!(yaml.contains("ttl_secs: 7200"));
        assert!(yaml.contains("tti_secs: 3600"));

        // Deserialize back
        let parsed: CacheConfig = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed.max_capacity, 5000);
        assert_eq!(parsed.ttl_secs, 7200);
        assert_eq!(parsed.tti_secs, 3600);
    }

    // ========================================================================
    // Circuit Breaker Config Tests (v1.2.0)
    // ========================================================================

    #[test]
    fn test_circuit_breaker_config_defaults() {
        let config = CircuitBreakerConfig::default();

        assert_eq!(config.failure_threshold, 3);
        assert_eq!(config.success_threshold, 2);
        assert_eq!(config.recovery_secs, 30);
        assert_eq!(config.half_open_permits, 1);
    }

    #[test]
    fn test_circuit_breaker_config_serialization() {
        let config = CircuitBreakerConfig {
            failure_threshold: 5,
            success_threshold: 3,
            recovery_secs: 60,
            half_open_permits: 2,
        };

        let yaml = serde_yaml::to_string(&config).unwrap();
        assert!(yaml.contains("failure_threshold: 5"));
        assert!(yaml.contains("success_threshold: 3"));
        assert!(yaml.contains("recovery_secs: 60"));
        assert!(yaml.contains("half_open_permits: 2"));

        // Deserialize back
        let parsed: CircuitBreakerConfig = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed.failure_threshold, 5);
        assert_eq!(parsed.success_threshold, 3);
        assert_eq!(parsed.recovery_secs, 60);
        assert_eq!(parsed.half_open_permits, 2);
    }

    #[test]
    fn test_circuit_breaker_config_partial_yaml() {
        // Test that missing fields use defaults
        let yaml = "failure_threshold: 10\n";
        let config: CircuitBreakerConfig = serde_yaml::from_str(yaml).unwrap();

        assert_eq!(config.failure_threshold, 10);
        assert_eq!(config.success_threshold, 2); // default
        assert_eq!(config.recovery_secs, 30); // default
        assert_eq!(config.half_open_permits, 1); // default
    }

    // ========================================================================
    // Modbus TLS Config Tests (v1.2.0)
    // ========================================================================

    #[test]
    fn test_modbus_tls_config_defaults() {
        let config = ModbusTlsConfig::default();

        assert!(!config.enabled);
        assert!(config.server_name.is_none());
        assert!(config.ca_cert_path.is_none());
        assert!(config.client_cert_path.is_none());
        assert!(config.client_key_path.is_none());
        assert!(!config.insecure_skip_verify);
    }

    #[test]
    fn test_modbus_tls_config_serialization() {
        let config = ModbusTlsConfig {
            enabled: true,
            server_name: Some("plc.example.com".to_string()),
            ca_cert_path: Some("/etc/certs/ca.pem".to_string()),
            client_cert_path: Some("/etc/certs/client.pem".to_string()),
            client_key_path: Some("/etc/certs/client.key".to_string()),
            insecure_skip_verify: false,
        };

        let yaml = serde_yaml::to_string(&config).unwrap();
        assert!(yaml.contains("enabled: true"));
        assert!(yaml.contains("server_name: plc.example.com"));
        assert!(yaml.contains("ca_cert_path:"));

        // Deserialize back
        let parsed: ModbusTlsConfig = serde_yaml::from_str(&yaml).unwrap();
        assert!(parsed.enabled);
        assert_eq!(parsed.server_name, Some("plc.example.com".to_string()));
    }

    #[test]
    fn test_modbus_device_config_with_tls() {
        let yaml = r#"
name: "PLC-001"
connection_type: "tcp"
address: "192.168.1.100:502"
slave_id: 1
tls:
  enabled: true
  server_name: "plc.local"
  insecure_skip_verify: true
"#;

        let config: ModbusDeviceConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.name, "PLC-001");
        assert!(config.tls.enabled);
        assert_eq!(config.tls.server_name, Some("plc.local".to_string()));
        assert!(config.tls.insecure_skip_verify);
    }

    #[test]
    fn test_modbus_device_config_without_tls() {
        let yaml = r#"
name: "PLC-002"
connection_type: "tcp"
address: "192.168.1.101:502"
"#;

        let config: ModbusDeviceConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.name, "PLC-002");
        assert!(!config.tls.enabled); // Default: TLS disabled
    }
}
