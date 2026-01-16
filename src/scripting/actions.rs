//! Script actions
//!
//! Defines what scripts can do:
//! - Set GPIO pin
//! - Write Modbus register
//! - Send alert/notification
//! - Set variable
//! - Log message
//! - Delay execution
//! - Call another script

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::VariableScope;

/// Action definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Action {
    /// Action type
    #[serde(rename = "type")]
    pub action_type: ActionType,

    /// Target (gpio pin, modbus device, variable name, etc.)
    #[serde(default)]
    pub target: String,

    /// Value to set/write
    #[serde(default)]
    pub value: Option<Value>,

    /// Message (for alert, log actions)
    #[serde(default)]
    pub message: Option<String>,

    /// Alert level (for alert actions)
    #[serde(default)]
    pub level: Option<AlertLevel>,

    /// Delay in milliseconds (for delay actions)
    #[serde(default)]
    pub delay_ms: Option<u64>,

    /// Device name (for modbus actions)
    #[serde(default)]
    pub device: Option<String>,

    /// Register address (for modbus actions)
    #[serde(default)]
    pub address: Option<u16>,

    /// Script ID (for call_script actions)
    #[serde(default)]
    pub script_id: Option<String>,

    /// Condition for this action (optional - if false, skip action)
    #[serde(default)]
    pub condition: Option<ActionCondition>,

    /// Variable scope for set_variable action (IEC 61131-3)
    /// - local: Lost on script restart (default)
    /// - global: Shared across scripts, lost on agent restart
    /// - retain: Persisted across power cycles (SQLite)
    #[serde(default)]
    pub scope: Option<VariableScope>,
}

/// Action types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ActionType {
    /// Set GPIO pin state
    SetGpio,
    /// Write Modbus register
    WriteModbus,
    /// Write Modbus coil
    WriteCoil,
    /// Send alert notification
    Alert,
    /// Set a variable
    SetVariable,
    /// Log a message
    Log,
    /// Delay execution
    Delay,
    /// Call another script
    CallScript,
    /// Publish MQTT message
    PublishMqtt,
    /// No operation (for conditional skipping)
    Noop,
}

/// Alert levels
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AlertLevel {
    Info,
    Warning,
    Error,
    Critical,
}

impl Default for AlertLevel {
    fn default() -> Self {
        AlertLevel::Warning
    }
}

/// Inline condition for an action
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionCondition {
    /// Source of value
    pub source: String,
    /// Comparison operator
    pub operator: super::ComparisonOperator,
    /// Value to compare
    pub value: Value,
}

/// Action execution result
#[derive(Debug, Clone)]
pub struct ActionResult {
    pub success: bool,
    pub action_type: ActionType,
    pub message: String,
    pub details: Option<Value>,
}

impl ActionResult {
    pub fn success(action_type: ActionType, message: impl Into<String>) -> Self {
        Self {
            success: true,
            action_type,
            message: message.into(),
            details: None,
        }
    }

    pub fn failure(action_type: ActionType, message: impl Into<String>) -> Self {
        Self {
            success: false,
            action_type,
            message: message.into(),
            details: None,
        }
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_parsing() {
        let json = r#"{
            "type": "set_gpio",
            "target": "17",
            "value": true
        }"#;

        let action: Action = serde_json::from_str(json).unwrap();
        assert_eq!(action.action_type, ActionType::SetGpio);
        assert_eq!(action.target, "17");
        assert_eq!(action.value, Some(Value::Bool(true)));
    }

    #[test]
    fn test_alert_action() {
        let json = r#"{
            "type": "alert",
            "level": "warning",
            "message": "Temperature high: ${water_temp}°C"
        }"#;

        let action: Action = serde_json::from_str(json).unwrap();
        assert_eq!(action.action_type, ActionType::Alert);
        assert_eq!(action.level, Some(AlertLevel::Warning));
        assert!(action.message.unwrap().contains("${water_temp}"));
    }

    #[test]
    fn test_modbus_action() {
        let json = r#"{
            "type": "write_modbus",
            "device": "PLC-1",
            "address": 100,
            "value": 500
        }"#;

        let action: Action = serde_json::from_str(json).unwrap();
        assert_eq!(action.action_type, ActionType::WriteModbus);
        assert_eq!(action.device, Some("PLC-1".to_string()));
        assert_eq!(action.address, Some(100));
    }
}
