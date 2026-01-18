//! Edge Scripting Engine
//!
//! Provides a safe DSL for automation scripts on edge devices.
//! Supports:
//! - Condition-based rules (if/then/else)
//! - Time-based triggers (cron-like scheduling)
//! - Threshold-based triggers (sensor values)
//! - Actions (GPIO, Modbus, alerts, logging)
//!
//! v2.0 Features:
//! - Execution limits (time, actions, depth)
//! - Rate limiting per script
//! - Sandboxed execution context

mod actions;
mod conflict;
mod context;
mod engine;
mod fb_registry;
pub mod function_blocks;
mod limits;
mod parallel;
mod persistence;
mod storage;
mod triggers;

pub use actions::{Action, ActionResult, ActionType, AlertLevel};
pub use conflict::{ConflictDetector, ConflictResult};
pub use context::ScriptContext;
pub use engine::ScriptEngine;
#[allow(unused_imports)]  // FBParams is part of public API, may not be used internally
pub use fb_registry::{FBDefinition, FBParams, FBRegistry, FBRegistryError};
pub use limits::{ExecutionContext, LimitError, ScriptLimits, ScriptRateLimiter};
pub use persistence::{PersistenceError, SqlitePersistence, VariableScope, VariableStore};
pub use storage::{Script, ScriptStatus, ScriptStorage};
pub use triggers::{Trigger, TriggerManager, TriggerType};

use serde::{Deserialize, Serialize};

/// Script execution mode (v2.1 - IEC 61131-3)
/// Determines how the script engine operates
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    /// Event-driven mode (default): Scripts run when triggers fire
    /// Suitable for simple threshold-based rules
    EventDriven,
    /// Scan cycle mode: PLC-like deterministic execution
    /// All function blocks execute every scan cycle (10-1000ms)
    /// Suitable for complex IEC 61131-3 programs
    ScanCycle,
}

impl Default for ExecutionMode {
    fn default() -> Self {
        ExecutionMode::EventDriven
    }
}

/// Script priority levels (v2.0)
/// Higher values = higher priority = executes first
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScriptPriority {
    /// Lowest priority (default) - runs last
    Low = 0,
    /// Normal priority
    Normal = 50,
    /// High priority - runs before normal scripts
    High = 100,
    /// Critical priority - runs first, wins conflicts
    Critical = 200,
    /// Emergency priority - absolute highest, for safety scripts
    Emergency = 255,
}

impl Default for ScriptPriority {
    fn default() -> Self {
        ScriptPriority::Normal
    }
}

impl ScriptPriority {
    /// Get numeric value for comparison
    pub fn value(&self) -> u8 {
        *self as u8
    }
}

/// Script definition - the DSL structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptDefinition {
    /// Unique script ID
    pub id: String,

    /// Human-readable name
    pub name: String,

    /// Description
    #[serde(default)]
    pub description: String,

    /// Script version
    #[serde(default = "default_version")]
    pub version: String,

    /// Whether script is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Script priority (v2.0) - higher priority scripts execute first
    /// and win in conflict situations
    #[serde(default)]
    pub priority: ScriptPriority,

    /// Trigger conditions
    pub triggers: Vec<Trigger>,

    /// Conditions to check
    #[serde(default)]
    pub conditions: Vec<Condition>,

    /// Actions to execute
    pub actions: Vec<Action>,

    /// Error handling actions
    #[serde(default)]
    pub on_error: Vec<Action>,
}

/// Condition for script execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Condition {
    /// Condition type
    #[serde(rename = "type")]
    pub condition_type: ConditionType,

    /// Source of value (sensor name, variable, etc.)
    pub source: String,

    /// Comparison operator
    pub operator: ComparisonOperator,

    /// Value to compare against
    pub value: serde_json::Value,
}

/// Condition types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConditionType {
    /// Sensor/register value
    Sensor,
    /// GPIO pin state
    Gpio,
    /// Variable value
    Variable,
    /// Time-based (hour, minute, day_of_week)
    Time,
    /// System metric (cpu, memory, etc.)
    System,
}

/// Comparison operators
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComparisonOperator {
    Eq,       // ==
    Ne,       // !=
    Gt,       // >
    Gte,      // >=
    Lt,       // <
    Lte,      // <=
    Contains, // string contains
    Between,  // value between [min, max]
    In,       // value in list
}

fn default_version() -> String {
    "1.0".to_string()
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_script_definition_parsing() {
        let json = r#"{
            "id": "script-001",
            "name": "High Temperature Alert",
            "description": "Alert when water temp exceeds threshold",
            "triggers": [
                {"type": "threshold", "source": "water_temp", "operator": "gt", "value": 28.0}
            ],
            "conditions": [],
            "actions": [
                {"type": "alert", "level": "warning", "message": "Water temperature high: ${water_temp}°C"}
            ]
        }"#;

        let script: ScriptDefinition = serde_json::from_str(json).unwrap();
        assert_eq!(script.id, "script-001");
        assert_eq!(script.name, "High Temperature Alert");
        assert!(script.enabled);
    }

    #[test]
    fn test_script_priority_parsing() {
        let json = r#"{
            "id": "emergency-shutdown",
            "name": "Emergency Shutdown",
            "priority": "emergency",
            "triggers": [
                {"type": "threshold", "source": "water_temp", "operator": "gt", "value": 35.0}
            ],
            "conditions": [],
            "actions": [
                {"type": "set_gpio", "target": "17", "value": false}
            ]
        }"#;

        let script: ScriptDefinition = serde_json::from_str(json).unwrap();
        assert_eq!(script.priority, ScriptPriority::Emergency);
        assert_eq!(script.priority.value(), 255);
    }

    #[test]
    fn test_script_priority_default() {
        let json = r#"{
            "id": "normal-script",
            "name": "Normal Script",
            "triggers": [],
            "conditions": [],
            "actions": []
        }"#;

        let script: ScriptDefinition = serde_json::from_str(json).unwrap();
        assert_eq!(script.priority, ScriptPriority::Normal);
        assert_eq!(script.priority.value(), 50);
    }

    #[test]
    fn test_priority_ordering() {
        // Higher value = higher priority
        assert!(ScriptPriority::Emergency > ScriptPriority::Critical);
        assert!(ScriptPriority::Critical > ScriptPriority::High);
        assert!(ScriptPriority::High > ScriptPriority::Normal);
        assert!(ScriptPriority::Normal > ScriptPriority::Low);

        // Numeric values
        assert_eq!(ScriptPriority::Low.value(), 0);
        assert_eq!(ScriptPriority::Normal.value(), 50);
        assert_eq!(ScriptPriority::High.value(), 100);
        assert_eq!(ScriptPriority::Critical.value(), 200);
        assert_eq!(ScriptPriority::Emergency.value(), 255);
    }
}
