//! IEC 61131-3 Edge Detection Function Blocks
//!
//! Standard edge detection implementations:
//! - R_TRIG: Rising Edge Trigger (detects FALSE → TRUE transition)
//! - F_TRIG: Falling Edge Trigger (detects TRUE → FALSE transition)
//!
//! These are fundamental building blocks for event-driven logic in PLCs.

use super::FunctionBlock;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// ============================================================================
// R_TRIG - Rising Edge Trigger
// ============================================================================

/// R_TRIG (Rising Trigger) - IEC 61131-3
///
/// Detects rising edge (FALSE → TRUE transition) on CLK input.
/// Q is TRUE for exactly one scan cycle when rising edge detected.
///
/// Timing Diagram:
/// ```text
/// CLK: ___|‾‾‾‾‾‾|_____|‾‾‾|_____
/// Q:   ___|‾|_________|‾|_______
///         ↑           ↑
///     Rising edges detected
/// ```
// IEC 61131-3 standard function block name
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct R_TRIG {
    /// Clock input
    clk: bool,
    /// Output (TRUE for one cycle on rising edge)
    q: bool,
    /// Previous clock state for edge detection
    prev_clk: bool,
}

impl R_TRIG {
    /// Create new R_TRIG
    pub fn new() -> Self {
        Self {
            clk: false,
            q: false,
            prev_clk: false,
        }
    }

    /// Get output state
    pub fn q(&self) -> bool {
        self.q
    }

    /// Check if rising edge was detected this cycle
    pub fn is_triggered(&self) -> bool {
        self.q
    }
}

impl Default for R_TRIG {
    fn default() -> Self {
        Self::new()
    }
}

impl FunctionBlock for R_TRIG {
    fn fb_type(&self) -> &'static str {
        "R_TRIG"
    }

    fn execute(&mut self) {
        // Detect rising edge: was FALSE, now TRUE
        self.q = self.clk && !self.prev_clk;
        self.prev_clk = self.clk;
    }

    fn get_output(&self, name: &str) -> Option<Value> {
        match name {
            "Q" | "q" | "output" => Some(Value::Bool(self.q)),
            _ => None,
        }
    }

    fn set_input(&mut self, name: &str, value: Value) -> bool {
        match name {
            "CLK" | "clk" | "clock" | "IN" | "in" | "input" => {
                if let Some(v) = value.as_bool() {
                    self.clk = v;
                    return true;
                }
            }
            _ => {}
        }
        false
    }

    fn serialize_state(&self) -> Value {
        json!({
            "clk": self.clk,
            "q": self.q,
            "prev_clk": self.prev_clk
        })
    }

    fn deserialize_state(&mut self, state: &Value) -> bool {
        if let Some(obj) = state.as_object() {
            if let Some(clk) = obj.get("clk").and_then(|v| v.as_bool()) {
                self.clk = clk;
            }
            if let Some(q) = obj.get("q").and_then(|v| v.as_bool()) {
                self.q = q;
            }
            if let Some(prev) = obj.get("prev_clk").and_then(|v| v.as_bool()) {
                self.prev_clk = prev;
            }
            return true;
        }
        false
    }

    fn reset(&mut self) {
        self.clk = false;
        self.q = false;
        self.prev_clk = false;
    }

    fn input_names(&self) -> Vec<&'static str> {
        vec!["CLK"]
    }

    fn output_names(&self) -> Vec<&'static str> {
        vec!["Q"]
    }
}

// ============================================================================
// F_TRIG - Falling Edge Trigger
// ============================================================================

/// F_TRIG (Falling Trigger) - IEC 61131-3
///
/// Detects falling edge (TRUE → FALSE transition) on CLK input.
/// Q is TRUE for exactly one scan cycle when falling edge detected.
///
/// Timing Diagram:
/// ```text
/// CLK: ___|‾‾‾‾‾‾|_____|‾‾‾|_____
/// Q:   _________|‾|________|‾|___
///               ↑          ↑
///         Falling edges detected
/// ```
// IEC 61131-3 standard function block name
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct F_TRIG {
    /// Clock input
    clk: bool,
    /// Output (TRUE for one cycle on falling edge)
    q: bool,
    /// Previous clock state for edge detection
    prev_clk: bool,
}

impl F_TRIG {
    /// Create new F_TRIG
    pub fn new() -> Self {
        Self {
            clk: false,
            q: false,
            prev_clk: false,
        }
    }

    /// Get output state
    pub fn q(&self) -> bool {
        self.q
    }

    /// Check if falling edge was detected this cycle
    pub fn is_triggered(&self) -> bool {
        self.q
    }
}

impl Default for F_TRIG {
    fn default() -> Self {
        Self::new()
    }
}

impl FunctionBlock for F_TRIG {
    fn fb_type(&self) -> &'static str {
        "F_TRIG"
    }

    fn execute(&mut self) {
        // Detect falling edge: was TRUE, now FALSE
        self.q = !self.clk && self.prev_clk;
        self.prev_clk = self.clk;
    }

    fn get_output(&self, name: &str) -> Option<Value> {
        match name {
            "Q" | "q" | "output" => Some(Value::Bool(self.q)),
            _ => None,
        }
    }

    fn set_input(&mut self, name: &str, value: Value) -> bool {
        match name {
            "CLK" | "clk" | "clock" | "IN" | "in" | "input" => {
                if let Some(v) = value.as_bool() {
                    self.clk = v;
                    return true;
                }
            }
            _ => {}
        }
        false
    }

    fn serialize_state(&self) -> Value {
        json!({
            "clk": self.clk,
            "q": self.q,
            "prev_clk": self.prev_clk
        })
    }

    fn deserialize_state(&mut self, state: &Value) -> bool {
        if let Some(obj) = state.as_object() {
            if let Some(clk) = obj.get("clk").and_then(|v| v.as_bool()) {
                self.clk = clk;
            }
            if let Some(q) = obj.get("q").and_then(|v| v.as_bool()) {
                self.q = q;
            }
            if let Some(prev) = obj.get("prev_clk").and_then(|v| v.as_bool()) {
                self.prev_clk = prev;
            }
            return true;
        }
        false
    }

    fn reset(&mut self) {
        self.clk = false;
        self.q = false;
        self.prev_clk = false;
    }

    fn input_names(&self) -> Vec<&'static str> {
        vec!["CLK"]
    }

    fn output_names(&self) -> Vec<&'static str> {
        vec!["Q"]
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_r_trig_basic() {
        let mut r_trig = R_TRIG::new();

        // Initially no output
        r_trig.execute();
        assert!(!r_trig.q());

        // Rising edge
        r_trig.set_input("CLK", Value::Bool(true));
        r_trig.execute();
        assert!(r_trig.q()); // Rising edge detected

        // Next cycle - no edge
        r_trig.execute();
        assert!(!r_trig.q()); // Output only lasts one cycle

        // Still high - no edge
        r_trig.execute();
        assert!(!r_trig.q());

        // Falling edge - R_TRIG doesn't detect this
        r_trig.set_input("CLK", Value::Bool(false));
        r_trig.execute();
        assert!(!r_trig.q());

        // Another rising edge
        r_trig.set_input("CLK", Value::Bool(true));
        r_trig.execute();
        assert!(r_trig.q()); // Rising edge detected again
    }

    #[test]
    fn test_f_trig_basic() {
        let mut f_trig = F_TRIG::new();

        // Initially no output
        f_trig.execute();
        assert!(!f_trig.q());

        // Rising edge - F_TRIG doesn't detect this
        f_trig.set_input("CLK", Value::Bool(true));
        f_trig.execute();
        assert!(!f_trig.q());

        // Still high
        f_trig.execute();
        assert!(!f_trig.q());

        // Falling edge
        f_trig.set_input("CLK", Value::Bool(false));
        f_trig.execute();
        assert!(f_trig.q()); // Falling edge detected

        // Next cycle - no edge
        f_trig.execute();
        assert!(!f_trig.q()); // Output only lasts one cycle
    }

    #[test]
    fn test_r_trig_multiple_edges() {
        let mut r_trig = R_TRIG::new();
        let mut edge_count = 0;

        // Simulate multiple cycles with edges
        let sequence = [false, true, true, false, true, false, true, true, false];

        for &val in &sequence {
            r_trig.set_input("CLK", Value::Bool(val));
            r_trig.execute();
            if r_trig.q() {
                edge_count += 1;
            }
        }

        // Should have detected 3 rising edges (false→true)
        assert_eq!(edge_count, 3);
    }

    #[test]
    fn test_f_trig_multiple_edges() {
        let mut f_trig = F_TRIG::new();
        let mut edge_count = 0;

        // Simulate multiple cycles with edges
        let sequence = [false, true, true, false, true, false, true, true, false];

        for &val in &sequence {
            f_trig.set_input("CLK", Value::Bool(val));
            f_trig.execute();
            if f_trig.q() {
                edge_count += 1;
            }
        }

        // Should have detected 3 falling edges (true→false)
        assert_eq!(edge_count, 3);
    }

    #[test]
    fn test_edge_serialize_deserialize() {
        let mut r_trig1 = R_TRIG::new();
        r_trig1.set_input("CLK", Value::Bool(true));
        r_trig1.execute();

        let state = r_trig1.serialize_state();

        let mut r_trig2 = R_TRIG::new();
        r_trig2.deserialize_state(&state);

        assert_eq!(r_trig2.q(), r_trig1.q());
    }

    #[test]
    fn test_function_block_trait() {
        let r_trig: Box<dyn FunctionBlock> = Box::new(R_TRIG::new());
        assert_eq!(r_trig.fb_type(), "R_TRIG");
        assert_eq!(r_trig.input_names(), vec!["CLK"]);
        assert_eq!(r_trig.output_names(), vec!["Q"]);
    }

    #[test]
    fn test_edge_reset() {
        let mut r_trig = R_TRIG::new();

        // Trigger
        r_trig.set_input("CLK", Value::Bool(true));
        r_trig.execute();
        assert!(r_trig.q());

        // Reset
        r_trig.reset();
        assert!(!r_trig.q());

        // After reset, rising edge should work again
        r_trig.set_input("CLK", Value::Bool(true));
        r_trig.execute();
        assert!(r_trig.q()); // Edge detected from reset state
    }
}
