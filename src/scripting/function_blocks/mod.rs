//! IEC 61131-3 Function Blocks
//!
//! Standard function blocks for industrial automation:
//! - Timers: TON (On-Delay), TOF (Off-Delay), TP (Pulse)
//! - Counters: CTU (Up), CTD (Down), CTUD (Up/Down)
//! - Edge Detection: R_TRIG (Rising), F_TRIG (Falling)
//!
//! All function blocks:
//! - Maintain internal state between scan cycles
//! - Support persistence via FBState serialization
//! - Implement the FunctionBlock trait for unified handling

pub mod counters;
pub mod edge_triggers;
pub mod timers;

pub use counters::{CTD, CTU, CTUD};
pub use edge_triggers::{F_TRIG, R_TRIG};
pub use timers::{TOF, TON, TP};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Trait for all function blocks
/// Enables unified handling and persistence
pub trait FunctionBlock: Send + Sync {
    /// Get the function block type name (e.g., "TON", "CTU")
    fn fb_type(&self) -> &'static str;

    /// Execute one scan cycle
    fn execute(&mut self);

    /// Get an output value by name
    fn get_output(&self, name: &str) -> Option<Value>;

    /// Set an input value by name
    fn set_input(&mut self, name: &str, value: Value) -> bool;

    /// Serialize internal state for persistence
    fn serialize_state(&self) -> Value;

    /// Deserialize internal state from persistence
    fn deserialize_state(&mut self, state: &Value) -> bool;

    /// Reset to initial state
    fn reset(&mut self);

    /// Get all input names
    fn input_names(&self) -> Vec<&'static str>;

    /// Get all output names
    fn output_names(&self) -> Vec<&'static str>;
}

/// Function block instance wrapper with ID
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FBInstance {
    /// Unique instance ID
    pub id: String,
    /// Function block type
    pub fb_type: String,
    /// Initial parameters (for recreation)
    pub params: Value,
}

/// Common result type for FB operations
#[derive(Debug, Clone)]
pub struct FBResult {
    pub success: bool,
    pub message: String,
}

impl FBResult {
    pub fn ok() -> Self {
        Self {
            success: true,
            message: String::new(),
        }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            success: false,
            message: msg.into(),
        }
    }
}
