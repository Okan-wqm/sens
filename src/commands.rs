//! Command handler for remote commands
//!
//! Receives and executes commands from the cloud platform.
//! Supports: ping, reboot, get_config, update_config, scripts, etc.
//!
//! v2.1 Features:
//! - deploy_program: IEC 61131-3 program deployment with FBs

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::mqtt::{CommandMessage, CommandResponse, IncomingMessage};
use crate::scripting::{ExecutionMode, FBDefinition, ScriptDefinition, ScriptStorage};
use crate::AppState;

/// Simple sliding window rate limiter
struct RateLimiter {
    /// Timestamps of recent commands
    timestamps: VecDeque<Instant>,
    /// Maximum allowed commands in window
    max_commands: usize,
    /// Window duration
    window: Duration,
}

impl RateLimiter {
    fn new(max_commands: usize, window: Duration) -> Self {
        Self {
            timestamps: VecDeque::with_capacity(max_commands),
            max_commands,
            window,
        }
    }

    /// Check if a command should be allowed
    /// Returns true if allowed, false if rate limited
    fn check(&mut self) -> bool {
        let now = Instant::now();

        // Remove timestamps outside the window
        while let Some(&oldest) = self.timestamps.front() {
            if now.duration_since(oldest) > self.window {
                self.timestamps.pop_front();
            } else {
                break;
            }
        }

        // Check if under limit
        if self.timestamps.len() < self.max_commands {
            self.timestamps.push_back(now);
            true
        } else {
            false
        }
    }

    /// Get current command count in window
    fn current_count(&self) -> usize {
        self.timestamps.len()
    }
}

// ============================================================================
// Parameter Extraction Helpers
// ============================================================================
// These helpers are provided for future command handlers.
// Currently unused but kept for consistency and future use.

/// Helper to extract a required string parameter from JSON params
#[allow(dead_code)]
fn require_str_param<'a>(
    params: &'a Value,
    key: &str,
) -> Result<&'a str, (bool, Value, Option<String>)> {
    params.get(key).and_then(|v| v.as_str()).ok_or_else(|| {
        (
            false,
            json!(null),
            Some(format!("Missing required parameter: {}", key)),
        )
    })
}

/// Helper to extract a required u64 parameter from JSON params
#[allow(dead_code)]
fn require_u64_param(params: &Value, key: &str) -> Result<u64, (bool, Value, Option<String>)> {
    params.get(key).and_then(|v| v.as_u64()).ok_or_else(|| {
        (
            false,
            json!(null),
            Some(format!("Missing required parameter: {}", key)),
        )
    })
}

/// Helper to extract an optional string parameter from JSON params
#[allow(dead_code)]
fn get_str_param<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params.get(key).and_then(|v| v.as_str())
}

/// Helper to extract an optional u64 parameter from JSON params
#[allow(dead_code)]
fn get_u64_param(params: &Value, key: &str) -> Option<u64> {
    params.get(key).and_then(|v| v.as_u64())
}

/// Helper to extract an optional bool parameter from JSON params
#[allow(dead_code)]
fn get_bool_param(params: &Value, key: &str) -> Option<bool> {
    params.get(key).and_then(|v| v.as_bool())
}

// ============================================================================
// IEC 61131-3 Program Definition (v2.1)
// ============================================================================

/// IEC 61131-3 Program definition received from cloud
/// Contains everything needed to run a program on the edge device
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgramDefinition {
    /// Unique program ID
    pub id: String,
    /// Program name
    pub name: String,
    /// Program version
    #[serde(default = "default_version")]
    pub version: u32,
    /// Description
    #[serde(default)]
    pub description: String,
    /// Execution mode
    #[serde(default)]
    pub execution_mode: ExecutionMode,
    /// Scan cycle time in milliseconds (for ScanCycle mode)
    #[serde(default = "default_scan_cycle")]
    pub scan_cycle_ms: u64,
    /// Function block definitions
    #[serde(default)]
    pub function_blocks: Vec<FBDefinition>,
    /// Script definition (triggers, conditions, actions)
    pub script: ScriptDefinition,
    /// Whether to replace existing program with same ID
    #[serde(default)]
    pub replace_existing: bool,
}

fn default_version() -> u32 {
    1
}

fn default_scan_cycle() -> u64 {
    100 // 100ms default
}

/// Persisted program state (for reload after restart)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgramState {
    /// Currently deployed program
    pub program: Option<ProgramDefinition>,
    /// Deployment timestamp
    pub deployed_at: Option<String>,
    /// Previous version (for rollback)
    pub previous_version: Option<Box<ProgramDefinition>>,
}

impl Default for ProgramState {
    fn default() -> Self {
        Self {
            program: None,
            deployed_at: None,
            previous_version: None,
        }
    }
}

// ============================================================================
// Command Handler
// ============================================================================

/// Command handler
///
/// v2.2: Uses shared ScriptStorage from AppState for data consistency
pub struct CommandHandler {
    state: Arc<RwLock<AppState>>,
    /// Shared script storage (v2.2 - from AppState singleton)
    script_storage: Arc<tokio::sync::RwLock<ScriptStorage>>,
    rate_limiter: RateLimiter,
    /// Path to program state file
    program_state_path: PathBuf,
}

impl CommandHandler {
    /// Create a new command handler (v2.2 - uses shared storage from AppState)
    pub async fn new(state: Arc<RwLock<AppState>>) -> Self {
        // Get shared script storage and runtime config from AppState (v2.2 singleton)
        let (script_storage, rate_limit_max, rate_limit_window_secs) = {
            let state_guard = state.read().await;
            (
                state_guard.script_storage.clone(),
                state_guard.config.runtime.rate_limit_max_commands,
                state_guard.config.runtime.rate_limit_window_secs,
            )
        };

        // Program state file location
        let data_dir =
            std::env::var("SUDERRA_DATA_DIR").unwrap_or_else(|_| "/var/lib/suderra".to_string());
        let program_state_path = PathBuf::from(&data_dir).join("program.json");

        Self {
            state,
            script_storage,
            rate_limiter: RateLimiter::new(
                rate_limit_max,
                Duration::from_secs(rate_limit_window_secs),
            ),
            program_state_path,
        }
    }

    /// Run the command handler loop
    pub async fn run(mut self) {
        info!("Command handler started");

        loop {
            // Wait a bit before checking for messages
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

            // Check for incoming messages
            let message = {
                let mut state = self.state.write().await;
                if let Some(ref mut mqtt) = state.mqtt_client {
                    mqtt.try_recv()
                } else {
                    None
                }
            };

            if let Some(msg) = message {
                // Rate limit check - protect against command flooding
                if !self.rate_limiter.check() {
                    warn!(
                        "Command rate limit exceeded ({} commands in {} seconds). Dropping message.",
                        self.rate_limiter.max_commands,
                        self.rate_limiter.window.as_secs()
                    );
                    continue;
                }

                if let Err(e) = self.handle_message(msg).await {
                    error!("Failed to handle message: {}", e);
                }
            }
        }
    }

    /// Handle incoming message
    async fn handle_message(&mut self, message: IncomingMessage) -> anyhow::Result<()> {
        let state = self.state.read().await;
        let topics = state.mqtt_client.as_ref().map(|m| m.topics().clone());
        drop(state);

        let topics = match topics {
            Some(t) => t,
            None => return Ok(()),
        };

        // Check if this is a command message
        if message.topic == topics.commands {
            debug!("Received command message");

            // Parse command
            let command: CommandMessage = match serde_json::from_slice(&message.payload) {
                Ok(cmd) => cmd,
                Err(e) => {
                    warn!("Failed to parse command: {}", e);
                    return Ok(());
                }
            };

            info!(
                "Executing command: {} (id: {})",
                command.command, command.command_id
            );

            // Execute command
            let response = self.execute_command(&command).await;

            // Publish response
            let state = self.state.read().await;
            if let Some(ref mqtt) = state.mqtt_client {
                mqtt.publish_response(response).await?;
            }
        } else if message.topic == topics.config {
            debug!("Received config update");
            self.handle_config_update(&message.payload).await?;
        }

        Ok(())
    }

    /// Execute a command and return response
    async fn execute_command(&mut self, command: &CommandMessage) -> CommandResponse {
        let device_id = {
            let state = self.state.read().await;
            state.config.device_id.clone()
        };

        let (success, result, error) = match command.command.as_str() {
            "ping" => self.cmd_ping().await,
            "get_info" => self.cmd_get_info().await,
            "get_config" => self.cmd_get_config().await,
            "get_hardware" => self.cmd_get_hardware().await,
            "read_modbus" => self.cmd_read_modbus(&command.params).await,
            "write_modbus" => self.cmd_write_modbus(&command.params).await,
            "read_gpio" => self.cmd_read_gpio().await,
            "write_gpio" => self.cmd_write_gpio(&command.params).await,
            // Script commands
            "list_scripts" => self.cmd_list_scripts().await,
            "get_script" => self.cmd_get_script(&command.params).await,
            "deploy_script" => self.cmd_deploy_script(&command.params).await,
            "delete_script" => self.cmd_delete_script(&command.params).await,
            "enable_script" => self.cmd_enable_script(&command.params).await,
            "disable_script" => self.cmd_disable_script(&command.params).await,
            // IEC 61131-3 Program commands (v2.1)
            "deploy_program" => self.cmd_deploy_program(&command.params).await,
            "get_program" => self.cmd_get_program().await,
            "rollback_program" => self.cmd_rollback_program().await,
            // System commands
            "reboot" => self.cmd_reboot(&command.params).await,
            "restart_agent" => self.cmd_restart_agent().await,
            "set_log_level" => self.cmd_set_log_level(&command.params).await,
            _ => {
                warn!("Unknown command: {}", command.command);
                (
                    false,
                    json!(null),
                    Some(format!("Unknown command: {}", command.command)),
                )
            }
        };

        CommandResponse {
            command_id: command.command_id.clone(),
            device_id,
            success,
            result,
            timestamp: Utc::now().to_rfc3339(),
            error,
        }
    }

    /// Ping command - simple health check
    async fn cmd_ping(&self) -> (bool, Value, Option<String>) {
        info!("Executing ping command");
        (
            true,
            json!({"pong": true, "timestamp": Utc::now().to_rfc3339()}),
            None,
        )
    }

    /// Get device info
    async fn cmd_get_info(&self) -> (bool, Value, Option<String>) {
        info!("Executing get_info command");

        let state = self.state.read().await;

        let info = json!({
            "device_id": state.config.device_id,
            "device_code": state.config.device_code,
            "agent_version": env!("CARGO_PKG_VERSION"),
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "tenant_id": state.tenant_id,
            "mqtt_broker": state.config.mqtt.broker,
            "is_activated": state.is_activated,
        });

        (true, info, None)
    }

    /// Get current config
    async fn cmd_get_config(&self) -> (bool, Value, Option<String>) {
        info!("Executing get_config command");

        let state = self.state.read().await;

        // Return safe subset of config (no secrets)
        let config = json!({
            "device_id": state.config.device_id,
            "device_code": state.config.device_code,
            "api_url": state.config.api_url,
            "telemetry": {
                "interval_seconds": state.config.telemetry.interval_seconds,
                "include_cpu": state.config.telemetry.include_cpu,
                "include_memory": state.config.telemetry.include_memory,
                "include_disk": state.config.telemetry.include_disk,
                "include_temperature": state.config.telemetry.include_temperature,
            },
            "logging": {
                "level": state.config.logging.level,
            },
            "modbus_devices": state.config.modbus.len(),
            "gpio_pins": state.config.gpio.len(),
        });

        (true, config, None)
    }

    /// Reboot the device
    async fn cmd_reboot(&self, params: &Value) -> (bool, Value, Option<String>) {
        info!("Executing reboot command");

        // Check for delay parameter
        let delay_secs = params
            .get("delay_seconds")
            .and_then(|v| v.as_u64())
            .unwrap_or(5);

        // Schedule reboot
        #[cfg(target_os = "linux")]
        {
            info!("Scheduling reboot in {} seconds", delay_secs);

            // Use tokio spawn to not block the response
            tokio::spawn(async move {
                tokio::time::sleep(tokio::time::Duration::from_secs(delay_secs)).await;

                // Execute reboot
                let status = std::process::Command::new("shutdown")
                    .args(["-r", "now"])
                    .status();

                match status {
                    Ok(s) if s.success() => info!("Reboot initiated"),
                    Ok(s) => error!("Reboot command failed with status: {}", s),
                    Err(e) => error!("Failed to execute reboot: {}", e),
                }
            });

            (
                true,
                json!({"scheduled": true, "delay_seconds": delay_secs}),
                None,
            )
        }

        #[cfg(not(target_os = "linux"))]
        {
            warn!("Reboot not supported on this platform");
            (
                false,
                json!(null),
                Some("Reboot not supported on this platform".to_string()),
            )
        }
    }

    /// Restart the agent service
    async fn cmd_restart_agent(&self) -> (bool, Value, Option<String>) {
        info!("Executing restart_agent command");

        #[cfg(target_os = "linux")]
        {
            // Schedule restart
            tokio::spawn(async {
                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

                let status = std::process::Command::new("systemctl")
                    .args(["restart", "suderra-agent"])
                    .status();

                match status {
                    Ok(s) if s.success() => info!("Agent restart initiated"),
                    Ok(s) => error!("Restart command failed with status: {}", s),
                    Err(e) => error!("Failed to execute restart: {}", e),
                }
            });

            (true, json!({"scheduled": true}), None)
        }

        #[cfg(not(target_os = "linux"))]
        {
            warn!("Restart not supported on this platform");
            (
                false,
                json!(null),
                Some("Restart not supported on this platform".to_string()),
            )
        }
    }

    /// Set log level
    async fn cmd_set_log_level(&self, params: &Value) -> (bool, Value, Option<String>) {
        let level = match params.get("level").and_then(|v| v.as_str()) {
            Some(l) => l,
            None => {
                return (
                    false,
                    json!(null),
                    Some("Missing 'level' parameter".to_string()),
                )
            }
        };

        // Validate level
        let valid_levels = ["trace", "debug", "info", "warn", "error"];
        if !valid_levels.contains(&level.to_lowercase().as_str()) {
            return (
                false,
                json!(null),
                Some(format!("Invalid level. Valid: {:?}", valid_levels)),
            );
        }

        info!("Setting log level to: {}", level);

        // Update config
        let mut state = self.state.write().await;
        state.config.logging.level = level.to_lowercase();

        // Note: Actually changing the tracing level at runtime requires more setup
        // For now, we just update the config (effective after restart)

        (
            true,
            json!({"level": level, "note": "Effective after agent restart"}),
            None,
        )
    }

    /// Get hardware info - lists all connected devices and sensors
    async fn cmd_get_hardware(&self) -> (bool, Value, Option<String>) {
        info!("Executing get_hardware command");

        let state = self.state.read().await;

        // Collect Modbus device info
        let modbus_devices: Vec<Value> = state
            .config
            .modbus
            .iter()
            .map(|device| {
                json!({
                    "name": device.name,
                    "connection_type": device.connection_type,
                    "address": device.address,
                    "slave_id": device.slave_id,
                    "registers": device.registers.iter().map(|r| {
                        json!({
                            "name": r.name,
                            "address": r.address,
                            "type": r.register_type,
                            "data_type": r.data_type,
                            "unit": r.unit
                        })
                    }).collect::<Vec<_>>()
                })
            })
            .collect();

        // Collect GPIO pin info
        let gpio_pins: Vec<Value> = state
            .config
            .gpio
            .iter()
            .map(|pin| {
                json!({
                    "name": pin.name,
                    "pin": pin.pin,
                    "direction": pin.direction,
                    "pull": pin.pull,
                    "invert": pin.invert
                })
            })
            .collect();

        // Check hardware availability
        let modbus_connected = state.modbus_handle.is_some();
        // v2.2: Use gpio_handle instead of deprecated gpio_manager
        let gpio_available = state.gpio_handle.is_some();

        let hardware_info = json!({
            "modbus": {
                "configured": !modbus_devices.is_empty(),
                "connected": modbus_connected,
                "devices": modbus_devices
            },
            "gpio": {
                "configured": !gpio_pins.is_empty(),
                "available": gpio_available,
                "pins": gpio_pins
            },
            "platform": {
                "os": std::env::consts::OS,
                "arch": std::env::consts::ARCH
            }
        });

        (true, hardware_info, None)
    }

    /// Read all Modbus registers or specific device
    async fn cmd_read_modbus(&self, params: &Value) -> (bool, Value, Option<String>) {
        info!("Executing read_modbus command");

        let _device_name = params.get("device").and_then(|v| v.as_str());

        // Get modbus handle (thread-safe)
        let modbus_handle = {
            let state = self.state.read().await;
            state.modbus_handle.clone()
        };

        let handle = match modbus_handle {
            Some(h) => h,
            None => {
                return (
                    false,
                    json!(null),
                    Some("No Modbus devices configured".to_string()),
                )
            }
        };

        // Read all devices (device filtering can be added via handle if needed)
        let results = handle.read_all().await;
        let data: Vec<Value> = results
            .iter()
            .map(|result| {
                json!({
                    "device": result.device_name,
                    "values": result.values.iter().map(|v| {
                        json!({
                            "name": v.name,
                            "address": v.address,
                            "raw_value": v.raw_value,
                            "scaled_value": v.scaled_value,
                            "unit": v.unit,
                            "timestamp": v.timestamp
                        })
                    }).collect::<Vec<_>>(),
                    "errors": result.errors.clone()
                })
            })
            .collect();

        (true, json!({"devices": data}), None)
    }

    /// Write to Modbus register
    async fn cmd_write_modbus(&self, params: &Value) -> (bool, Value, Option<String>) {
        info!("Executing write_modbus command");

        let device_name = match params.get("device").and_then(|v| v.as_str()) {
            Some(d) => d,
            None => {
                return (
                    false,
                    json!(null),
                    Some("Missing 'device' parameter".to_string()),
                )
            }
        };

        let address = match params.get("address").and_then(|v| v.as_u64()) {
            Some(a) if a <= u16::MAX as u64 => a as u16,
            Some(a) => {
                return (
                    false,
                    json!(null),
                    Some(format!(
                        "Address {} exceeds maximum u16 value ({})",
                        a,
                        u16::MAX
                    )),
                )
            }
            None => {
                return (
                    false,
                    json!(null),
                    Some("Missing 'address' parameter".to_string()),
                )
            }
        };

        let value = match params.get("value").and_then(|v| v.as_u64()) {
            Some(v) if v <= u16::MAX as u64 => v as u16,
            Some(v) => {
                return (
                    false,
                    json!(null),
                    Some(format!(
                        "Value {} exceeds maximum u16 value ({})",
                        v,
                        u16::MAX
                    )),
                )
            }
            None => {
                return (
                    false,
                    json!(null),
                    Some("Missing 'value' parameter".to_string()),
                )
            }
        };

        // Get modbus handle (thread-safe)
        let modbus_handle = {
            let state = self.state.read().await;
            state.modbus_handle.clone()
        };

        let handle = match modbus_handle {
            Some(h) => h,
            None => {
                return (
                    false,
                    json!(null),
                    Some("No Modbus devices configured".to_string()),
                )
            }
        };

        match handle.write_register(device_name, address, value).await {
            Ok(()) => {
                info!("Wrote {} to register {} on {}", value, address, device_name);
                (
                    true,
                    json!({"device": device_name, "address": address, "value": value}),
                    None,
                )
            }
            Err(e) => {
                error!("Failed to write Modbus register: {}", e);
                (false, json!(null), Some(format!("Write failed: {}", e)))
            }
        }
    }

    /// Read all GPIO pins (v2.2: uses gpio_handle actor pattern)
    async fn cmd_read_gpio(&self) -> (bool, Value, Option<String>) {
        info!("Executing read_gpio command");

        // Get gpio_handle from state (clone to release lock)
        let gpio_handle = {
            let state = self.state.read().await;
            state.gpio_handle.clone()
        };

        let gpio_handle = match gpio_handle {
            Some(h) => h,
            None => {
                return (
                    false,
                    json!(null),
                    Some("No GPIO pins configured".to_string()),
                )
            }
        };

        // v2.2: Use async gpio_handle.read_all() instead of sync gpio_manager
        let result = gpio_handle.read_all().await;

        let pins: Vec<Value> = result
            .values
            .iter()
            .map(|v| {
                json!({
                    "name": v.name,
                    "pin": v.pin,
                    "direction": v.direction,
                    "state": format!("{:?}", v.state).to_lowercase(),
                    "timestamp": v.timestamp
                })
            })
            .collect();

        if result.errors.is_empty() {
            (true, json!({"pins": pins}), None)
        } else {
            (true, json!({"pins": pins, "errors": result.errors}), None)
        }
    }

    /// Write to GPIO pin (v2.2: uses gpio_handle actor pattern)
    async fn cmd_write_gpio(&self, params: &Value) -> (bool, Value, Option<String>) {
        info!("Executing write_gpio command");

        let pin = match params.get("pin").and_then(|v| v.as_u64()) {
            Some(p) => p as u8,
            None => {
                return (
                    false,
                    json!(null),
                    Some("Missing 'pin' parameter".to_string()),
                )
            }
        };

        let state_value = match params.get("state").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => {
                return (
                    false,
                    json!(null),
                    Some("Missing 'state' parameter (high/low)".to_string()),
                )
            }
        };

        // v2.2: Convert to bool for gpio_handle API
        let pin_value = match state_value.to_lowercase().as_str() {
            "high" | "1" | "true" | "on" => true,
            "low" | "0" | "false" | "off" => false,
            _ => {
                return (
                    false,
                    json!(null),
                    Some("Invalid state. Use 'high' or 'low'".to_string()),
                )
            }
        };

        // Get gpio_handle from state (clone to release lock)
        let gpio_handle = {
            let state = self.state.read().await;
            state.gpio_handle.clone()
        };

        let gpio_handle = match gpio_handle {
            Some(h) => h,
            None => {
                return (
                    false,
                    json!(null),
                    Some("No GPIO pins configured".to_string()),
                )
            }
        };

        // v2.2: Use async gpio_handle.write_pin() instead of sync gpio_manager
        match gpio_handle.write_pin(pin, pin_value).await {
            Ok(()) => {
                info!("Set GPIO pin {} to {}", pin, state_value);
                (true, json!({"pin": pin, "state": state_value}), None)
            }
            Err(e) => {
                error!("Failed to write GPIO pin: {}", e);
                (false, json!(null), Some(format!("Write failed: {}", e)))
            }
        }
    }

    // === Script Commands ===

    /// List all scripts (v2.2 - uses shared storage)
    async fn cmd_list_scripts(&self) -> (bool, Value, Option<String>) {
        info!("Executing list_scripts command");

        let storage = self.script_storage.read().await;
        let scripts: Vec<Value> = storage
            .get_all()
            .iter()
            .map(|s| {
                json!({
                    "id": s.definition.id,
                    "name": s.definition.name,
                    "description": s.definition.description,
                    "enabled": s.definition.enabled,
                    "status": format!("{:?}", s.status).to_lowercase(),
                    "triggers": s.definition.triggers.len(),
                    "actions": s.definition.actions.len(),
                    "last_run": s.last_run,
                    "last_result": s.last_result,
                    "error_count": s.error_count
                })
            })
            .collect();

        (
            true,
            json!({"scripts": scripts, "count": scripts.len()}),
            None,
        )
    }

    /// Get a specific script
    async fn cmd_get_script(&self, params: &Value) -> (bool, Value, Option<String>) {
        let script_id = match params.get("id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => {
                return (
                    false,
                    json!(null),
                    Some("Missing 'id' parameter".to_string()),
                )
            }
        };

        info!("Executing get_script command for: {}", script_id);

        let storage = self.script_storage.read().await;
        match storage.get(script_id) {
            Some(script) => {
                let data = json!({
                    "id": script.definition.id,
                    "name": script.definition.name,
                    "description": script.definition.description,
                    "version": script.definition.version,
                    "enabled": script.definition.enabled,
                    "status": format!("{:?}", script.status).to_lowercase(),
                    "triggers": script.definition.triggers,
                    "conditions": script.definition.conditions,
                    "actions": script.definition.actions,
                    "on_error": script.definition.on_error,
                    "last_run": script.last_run,
                    "last_result": script.last_result,
                    "error_count": script.error_count,
                    "created_at": script.created_at,
                    "updated_at": script.updated_at
                });
                (true, data, None)
            }
            None => (
                false,
                json!(null),
                Some(format!("Script '{}' not found", script_id)),
            ),
        }
    }

    /// Deploy (add/update) a script
    async fn cmd_deploy_script(&mut self, params: &Value) -> (bool, Value, Option<String>) {
        info!("Executing deploy_script command");

        // Parse script definition from params
        let definition: ScriptDefinition = match serde_json::from_value(params.clone()) {
            Ok(def) => def,
            Err(e) => {
                return (
                    false,
                    json!(null),
                    Some(format!("Invalid script definition: {}", e)),
                );
            }
        };

        let script_id = definition.id.clone();
        let script_name = definition.name.clone();

        let mut storage = self.script_storage.write().await;
        match storage.add_script(definition) {
            Ok(()) => {
                info!("Script deployed: {} ({})", script_name, script_id);
                (
                    true,
                    json!({
                        "id": script_id,
                        "name": script_name,
                        "message": "Script deployed successfully"
                    }),
                    None,
                )
            }
            Err(e) => {
                error!("Failed to deploy script: {}", e);
                (false, json!(null), Some(format!("Deploy failed: {}", e)))
            }
        }
    }

    /// Delete a script
    async fn cmd_delete_script(&mut self, params: &Value) -> (bool, Value, Option<String>) {
        let script_id = match params.get("id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => {
                return (
                    false,
                    json!(null),
                    Some("Missing 'id' parameter".to_string()),
                )
            }
        };

        info!("Executing delete_script command for: {}", script_id);

        let mut storage = self.script_storage.write().await;
        match storage.delete(script_id) {
            Ok(true) => (true, json!({"id": script_id, "deleted": true}), None),
            Ok(false) => (
                false,
                json!(null),
                Some(format!("Script '{}' not found", script_id)),
            ),
            Err(e) => (false, json!(null), Some(format!("Delete failed: {}", e))),
        }
    }

    /// Enable a script
    async fn cmd_enable_script(&mut self, params: &Value) -> (bool, Value, Option<String>) {
        let script_id = match params.get("id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => {
                return (
                    false,
                    json!(null),
                    Some("Missing 'id' parameter".to_string()),
                )
            }
        };

        info!("Executing enable_script command for: {}", script_id);

        let mut storage = self.script_storage.write().await;
        match storage.enable(script_id) {
            Ok(true) => (true, json!({"id": script_id, "enabled": true}), None),
            Ok(false) => (
                false,
                json!(null),
                Some(format!("Script '{}' not found", script_id)),
            ),
            Err(e) => (false, json!(null), Some(format!("Enable failed: {}", e))),
        }
    }

    /// Disable a script
    async fn cmd_disable_script(&mut self, params: &Value) -> (bool, Value, Option<String>) {
        let script_id = match params.get("id").and_then(|v| v.as_str()) {
            Some(id) => id,
            None => {
                return (
                    false,
                    json!(null),
                    Some("Missing 'id' parameter".to_string()),
                )
            }
        };

        info!("Executing disable_script command for: {}", script_id);

        let mut storage = self.script_storage.write().await;
        match storage.disable(script_id) {
            Ok(true) => (true, json!({"id": script_id, "enabled": false}), None),
            Ok(false) => (
                false,
                json!(null),
                Some(format!("Script '{}' not found", script_id)),
            ),
            Err(e) => (false, json!(null), Some(format!("Disable failed: {}", e))),
        }
    }

    // ========================================================================
    // IEC 61131-3 Program Commands (v2.1)
    // ========================================================================

    /// Deploy an IEC 61131-3 program
    ///
    /// This command:
    /// 1. Validates the program definition
    /// 2. Saves previous version for rollback
    /// 3. Persists the program to disk
    /// 4. Deploys the script portion
    /// 5. Engine will pick up FB definitions on next reload
    async fn cmd_deploy_program(&mut self, params: &Value) -> (bool, Value, Option<String>) {
        info!("Executing deploy_program command");

        // Get scripting limits from config
        let (max_fbs, min_scan, max_scan) = {
            let state = self.state.read().await;
            (
                state.config.scripting.max_function_blocks,
                state.config.scripting.min_scan_cycle_ms,
                state.config.scripting.max_scan_cycle_ms,
            )
        };

        // Parse program definition
        let program: ProgramDefinition = match serde_json::from_value(params.clone()) {
            Ok(p) => p,
            Err(e) => {
                error!("Failed to parse program definition: {}", e);
                return (
                    false,
                    json!(null),
                    Some(format!("Invalid program definition: {}", e)),
                );
            }
        };

        // Validate
        if program.function_blocks.len() > max_fbs {
            return (
                false,
                json!(null),
                Some(format!("Too many function blocks (max {})", max_fbs)),
            );
        }

        if program.scan_cycle_ms < min_scan || program.scan_cycle_ms > max_scan {
            return (
                false,
                json!(null),
                Some(format!(
                    "Scan cycle must be between {}ms and {}ms",
                    min_scan, max_scan
                )),
            );
        }

        // Load current state (for rollback)
        let mut state = self.load_program_state();
        let previous = state.program.take();

        // Save previous version for rollback
        if let Some(prev) = previous {
            if prev.id == program.id {
                state.previous_version = Some(Box::new(prev));
            }
        }

        // Deploy script portion (v2.2 - uses shared storage)
        {
            let mut storage = self.script_storage.write().await;
            if let Err(e) = storage.add_script(program.script.clone()) {
                error!("Failed to deploy script: {}", e);
                return (
                    false,
                    json!(null),
                    Some(format!("Failed to deploy script: {}", e)),
                );
            }
        }

        // Update state
        state.program = Some(program.clone());
        state.deployed_at = Some(Utc::now().to_rfc3339());

        // Persist to disk
        if let Err(e) = self.save_program_state(&state) {
            error!("Failed to save program state: {}", e);
            return (
                false,
                json!(null),
                Some(format!("Failed to persist program: {}", e)),
            );
        }

        info!(
            program_id = %program.id,
            program_name = %program.name,
            version = program.version,
            fb_count = program.function_blocks.len(),
            execution_mode = ?program.execution_mode,
            "Program deployed successfully"
        );

        (
            true,
            json!({
                "id": program.id,
                "name": program.name,
                "version": program.version,
                "functionBlockCount": program.function_blocks.len(),
                "executionMode": format!("{:?}", program.execution_mode),
                "scanCycleMs": program.scan_cycle_ms,
                "message": "Program deployed successfully. Engine will reload on next cycle."
            }),
            None,
        )
    }

    /// Get currently deployed program
    async fn cmd_get_program(&self) -> (bool, Value, Option<String>) {
        info!("Executing get_program command");

        let state = self.load_program_state();

        match state.program {
            Some(program) => (
                true,
                json!({
                    "id": program.id,
                    "name": program.name,
                    "version": program.version,
                    "description": program.description,
                    "executionMode": format!("{:?}", program.execution_mode),
                    "scanCycleMs": program.scan_cycle_ms,
                    "functionBlockCount": program.function_blocks.len(),
                    "functionBlocks": program.function_blocks.iter()
                        .map(|fb| json!({
                            "id": fb.id,
                            "type": fb.fb_type
                        }))
                        .collect::<Vec<_>>(),
                    "deployedAt": state.deployed_at,
                    "hasPreviousVersion": state.previous_version.is_some()
                }),
                None,
            ),
            None => (
                true,
                json!({
                    "program": null,
                    "message": "No program deployed"
                }),
                None,
            ),
        }
    }

    /// Rollback to previous program version
    async fn cmd_rollback_program(&mut self) -> (bool, Value, Option<String>) {
        info!("Executing rollback_program command");

        let mut state = self.load_program_state();

        let previous = match state.previous_version.take() {
            Some(prev) => *prev,
            None => {
                return (
                    false,
                    json!(null),
                    Some("No previous version available for rollback".to_string()),
                );
            }
        };

        let prev_id = previous.id.clone();
        let prev_name = previous.name.clone();
        let prev_version = previous.version;

        // Deploy previous version's script (v2.2 - uses shared storage)
        {
            let mut storage = self.script_storage.write().await;
            if let Err(e) = storage.add_script(previous.script.clone()) {
                error!("Rollback failed - script deployment error: {}", e);
                return (false, json!(null), Some(format!("Rollback failed: {}", e)));
            }
        }

        // Update state
        state.program = Some(previous);
        state.deployed_at = Some(Utc::now().to_rfc3339());
        state.previous_version = None; // Clear - can't rollback twice

        // Persist
        if let Err(e) = self.save_program_state(&state) {
            error!("Rollback state save failed: {}", e);
            return (
                false,
                json!(null),
                Some(format!("Rollback state save failed: {}", e)),
            );
        }

        info!(
            program_id = %prev_id,
            version = prev_version,
            "Rolled back to previous version"
        );

        (
            true,
            json!({
                "id": prev_id,
                "name": prev_name,
                "version": prev_version,
                "message": "Rolled back to previous version successfully"
            }),
            None,
        )
    }

    /// Load program state from disk
    fn load_program_state(&self) -> ProgramState {
        match fs::read_to_string(&self.program_state_path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => ProgramState::default(),
        }
    }

    /// Save program state to disk
    fn save_program_state(&self, state: &ProgramState) -> anyhow::Result<()> {
        // Ensure parent directory exists
        if let Some(parent) = self.program_state_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let content = serde_json::to_string_pretty(state)?;
        fs::write(&self.program_state_path, content)?;

        debug!(path = ?self.program_state_path, "Program state saved");
        Ok(())
    }

    /// Handle config update from cloud
    async fn handle_config_update(&self, payload: &[u8]) -> anyhow::Result<()> {
        let config_update: Value = serde_json::from_slice(payload)?;
        info!("Received config update: {:?}", config_update);

        let mut state = self.state.write().await;
        let mut config_changed = false;

        // Update telemetry interval if provided
        if let Some(telemetry) = config_update.get("telemetry") {
            if let Some(interval) = telemetry.get("interval_seconds").and_then(|v| v.as_u64()) {
                // Validate: minimum 5 seconds, maximum 3600 seconds (1 hour)
                if interval >= 5 && interval <= 3600 {
                    state.config.telemetry.interval_seconds = interval;
                    config_changed = true;
                    info!("Updated telemetry interval to {} seconds", interval);
                } else {
                    warn!(
                        "Invalid telemetry interval {}: must be between 5 and 3600 seconds",
                        interval
                    );
                }
            }

            // Update telemetry include flags
            if let Some(include_system) = telemetry.get("include_system").and_then(|v| v.as_bool())
            {
                state.config.telemetry.include_system = include_system;
                config_changed = true;
            }
            if let Some(include_modbus) = telemetry.get("include_modbus").and_then(|v| v.as_bool())
            {
                state.config.telemetry.include_modbus = include_modbus;
                config_changed = true;
            }
            if let Some(include_gpio) = telemetry.get("include_gpio").and_then(|v| v.as_bool()) {
                state.config.telemetry.include_gpio = include_gpio;
                config_changed = true;
            }
        }

        // Update scripting enabled flag if provided
        if let Some(scripting) = config_update.get("scripting") {
            if let Some(enabled) = scripting.get("enabled").and_then(|v| v.as_bool()) {
                state.config.scripting.enabled = enabled;
                config_changed = true;
                info!("Updated scripting enabled to {}", enabled);
            }
        }

        // Save config to disk if changed
        if config_changed {
            if let Err(e) = state.config.save() {
                error!("Failed to save config after update: {}", e);
                return Err(anyhow::anyhow!("Failed to persist config changes: {}", e));
            }
            info!("Config update applied and saved successfully");
        } else {
            info!("No applicable config changes found in update");
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_response_serialization() {
        let response = CommandResponse {
            command_id: "cmd-123".to_string(),
            device_id: "device-456".to_string(),
            success: true,
            result: json!({"pong": true}),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            error: None,
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("command_id"));
        assert!(json.contains("pong"));
        assert!(!json.contains("error")); // None fields skipped
    }
}
