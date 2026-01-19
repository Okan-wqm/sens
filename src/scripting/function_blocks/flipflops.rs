//! IEC 61131-3 Flip-Flop Function Blocks
//!
//! Standard bistable (flip-flop) implementations:
//! - RS: Reset-Set Flip-Flop (Reset dominant)
//! - SR: Set-Reset Flip-Flop (Set dominant)
//!
//! These are fundamental building blocks for latching logic in PLCs.
//! The difference is in dominance when both inputs are TRUE:
//! - RS: Reset wins (Q = FALSE when R=TRUE and S=TRUE)
//! - SR: Set wins (Q = TRUE when S=TRUE and R=TRUE)
//!
//! v1.2.3: Initial implementation for IEC 61131-3 compliance

use super::FunctionBlock;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

// ============================================================================
// RS - Reset-Set Flip-Flop (Reset Dominant)
// ============================================================================

/// RS (Reset-Set) Flip-Flop - IEC 61131-3
///
/// Bistable function block with reset dominance.
/// When both SET and RESET are TRUE, RESET wins (Q = FALSE).
///
/// Truth Table:
/// ```text
/// | S | R | Q (output) |
/// |---|---|------------|
/// | 0 | 0 | Q (no change) |
/// | 1 | 0 | 1 (set)    |
/// | 0 | 1 | 0 (reset)  |
/// | 1 | 1 | 0 (reset dominant) |
/// ```
///
/// Timing Diagram:
/// ```text
/// S:  ___|‾‾‾|___|‾‾‾‾‾‾|___
/// R:  _________|‾‾‾|________
/// Q:  ___|‾‾‾‾‾|___|‾‾‾‾|___
///              ↑ Reset dominates
/// ```
// IEC 61131-3 standard function block name
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RS {
    /// Set input
    set: bool,
    /// Reset input (dominant)
    reset: bool,
    /// Output state (Q1)
    q1: bool,
}

impl RS {
    /// Create new RS flip-flop
    pub fn new() -> Self {
        Self {
            set: false,
            reset: false,
            q1: false,
        }
    }

    /// Create RS flip-flop with initial output state
    pub fn with_initial_state(initial_q: bool) -> Self {
        Self {
            set: false,
            reset: false,
            q1: initial_q,
        }
    }

    /// Get output state
    pub fn q1(&self) -> bool {
        self.q1
    }

    /// Alias for q1() - common PLC naming
    pub fn q(&self) -> bool {
        self.q1
    }

    /// Check if currently set
    pub fn is_set(&self) -> bool {
        self.q1
    }

    /// Set the SET input
    pub fn set_s(&mut self, value: bool) {
        self.set = value;
    }

    /// Set the RESET input
    pub fn set_r(&mut self, value: bool) {
        self.reset = value;
    }
}

impl Default for RS {
    fn default() -> Self {
        Self::new()
    }
}

impl FunctionBlock for RS {
    fn fb_type(&self) -> &'static str {
        "RS"
    }

    fn execute(&mut self) {
        // RS logic: Reset dominant
        // If R=TRUE: Q=FALSE (regardless of S)
        // Else if S=TRUE: Q=TRUE
        // Else: Q unchanged
        if self.reset {
            self.q1 = false;
        } else if self.set {
            self.q1 = true;
        }
        // If neither S nor R, Q1 retains its value (bistable)
    }

    fn get_output(&self, name: &str) -> Option<Value> {
        match name {
            "Q1" | "q1" | "Q" | "q" | "output" => Some(Value::Bool(self.q1)),
            // v1.2.3: Added Q_NOT output for IEC 61131-3 compliance
            "Q1_NOT" | "q1_not" | "QN" | "qn" | "NOT_Q" | "not_q" => Some(Value::Bool(!self.q1)),
            _ => None,
        }
    }

    fn set_input(&mut self, name: &str, value: Value) -> bool {
        match name {
            "S" | "s" | "SET" | "set" => {
                if let Some(v) = value.as_bool() {
                    self.set = v;
                    return true;
                }
            }
            "R" | "r" | "R1" | "r1" | "RESET" | "reset" | "RESET1" | "reset1" => {
                if let Some(v) = value.as_bool() {
                    self.reset = v;
                    return true;
                }
            }
            _ => {}
        }
        false
    }

    fn serialize_state(&self) -> Value {
        json!({
            "set": self.set,
            "reset": self.reset,
            "q1": self.q1
        })
    }

    fn deserialize_state(&mut self, state: &Value) -> bool {
        if let Some(obj) = state.as_object() {
            if let Some(set) = obj.get("set").and_then(|v| v.as_bool()) {
                self.set = set;
            }
            if let Some(reset) = obj.get("reset").and_then(|v| v.as_bool()) {
                self.reset = reset;
            }
            if let Some(q1) = obj.get("q1").and_then(|v| v.as_bool()) {
                self.q1 = q1;
            }
            return true;
        }
        false
    }

    fn reset(&mut self) {
        self.set = false;
        self.reset = false;
        self.q1 = false;
    }

    fn input_names(&self) -> Vec<&'static str> {
        vec!["S", "R1"]
    }

    fn output_names(&self) -> Vec<&'static str> {
        vec!["Q1", "Q1_NOT"]
    }
}

// ============================================================================
// SR - Set-Reset Flip-Flop (Set Dominant)
// ============================================================================

/// SR (Set-Reset) Flip-Flop - IEC 61131-3
///
/// Bistable function block with set dominance.
/// When both SET and RESET are TRUE, SET wins (Q = TRUE).
///
/// Truth Table:
/// ```text
/// | S | R | Q (output) |
/// |---|---|------------|
/// | 0 | 0 | Q (no change) |
/// | 1 | 0 | 1 (set)    |
/// | 0 | 1 | 0 (reset)  |
/// | 1 | 1 | 1 (set dominant) |
/// ```
///
/// Timing Diagram:
/// ```text
/// S:  ___|‾‾‾|___|‾‾‾‾‾‾|___
/// R:  _________|‾‾‾|________
/// Q:  ___|‾‾‾‾‾‾‾‾‾|‾‾‾‾|___
///              ↑ Set dominates
/// ```
// IEC 61131-3 standard function block name
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SR {
    /// Set input (dominant)
    set: bool,
    /// Reset input
    reset: bool,
    /// Output state (Q1)
    q1: bool,
}

impl SR {
    /// Create new SR flip-flop
    pub fn new() -> Self {
        Self {
            set: false,
            reset: false,
            q1: false,
        }
    }

    /// Create SR flip-flop with initial output state
    pub fn with_initial_state(initial_q: bool) -> Self {
        Self {
            set: false,
            reset: false,
            q1: initial_q,
        }
    }

    /// Get output state
    pub fn q1(&self) -> bool {
        self.q1
    }

    /// Alias for q1() - common PLC naming
    pub fn q(&self) -> bool {
        self.q1
    }

    /// Check if currently set
    pub fn is_set(&self) -> bool {
        self.q1
    }

    /// Set the SET input
    pub fn set_s(&mut self, value: bool) {
        self.set = value;
    }

    /// Set the RESET input
    pub fn set_r(&mut self, value: bool) {
        self.reset = value;
    }
}

impl Default for SR {
    fn default() -> Self {
        Self::new()
    }
}

impl FunctionBlock for SR {
    fn fb_type(&self) -> &'static str {
        "SR"
    }

    fn execute(&mut self) {
        // SR logic: Set dominant
        // If S=TRUE: Q=TRUE (regardless of R)
        // Else if R=TRUE: Q=FALSE
        // Else: Q unchanged
        if self.set {
            self.q1 = true;
        } else if self.reset {
            self.q1 = false;
        }
        // If neither S nor R, Q1 retains its value (bistable)
    }

    fn get_output(&self, name: &str) -> Option<Value> {
        match name {
            "Q1" | "q1" | "Q" | "q" | "output" => Some(Value::Bool(self.q1)),
            // v1.2.3: Added Q_NOT output for IEC 61131-3 compliance
            "Q1_NOT" | "q1_not" | "QN" | "qn" | "NOT_Q" | "not_q" => Some(Value::Bool(!self.q1)),
            _ => None,
        }
    }

    fn set_input(&mut self, name: &str, value: Value) -> bool {
        match name {
            "S1" | "s1" | "S" | "s" | "SET" | "set" | "SET1" | "set1" => {
                if let Some(v) = value.as_bool() {
                    self.set = v;
                    return true;
                }
            }
            "R" | "r" | "RESET" | "reset" => {
                if let Some(v) = value.as_bool() {
                    self.reset = v;
                    return true;
                }
            }
            _ => {}
        }
        false
    }

    fn serialize_state(&self) -> Value {
        json!({
            "set": self.set,
            "reset": self.reset,
            "q1": self.q1
        })
    }

    fn deserialize_state(&mut self, state: &Value) -> bool {
        if let Some(obj) = state.as_object() {
            if let Some(set) = obj.get("set").and_then(|v| v.as_bool()) {
                self.set = set;
            }
            if let Some(reset) = obj.get("reset").and_then(|v| v.as_bool()) {
                self.reset = reset;
            }
            if let Some(q1) = obj.get("q1").and_then(|v| v.as_bool()) {
                self.q1 = q1;
            }
            return true;
        }
        false
    }

    fn reset(&mut self) {
        self.set = false;
        self.reset = false;
        self.q1 = false;
    }

    fn input_names(&self) -> Vec<&'static str> {
        vec!["S1", "R"]
    }

    fn output_names(&self) -> Vec<&'static str> {
        vec!["Q1", "Q1_NOT"]
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // RS Tests
    // ========================================================================

    #[test]
    fn test_rs_initial_state() {
        let rs = RS::new();
        assert!(!rs.q1());
        assert!(!rs.is_set());
    }

    #[test]
    fn test_rs_with_initial_state() {
        let rs = RS::with_initial_state(true);
        assert!(rs.q1());
    }

    #[test]
    fn test_rs_set_only() {
        let mut rs = RS::new();

        // Set S=TRUE
        rs.set_input("S", Value::Bool(true));
        rs.execute();
        assert!(rs.q1()); // Should be set

        // Release S
        rs.set_input("S", Value::Bool(false));
        rs.execute();
        assert!(rs.q1()); // Should remain set (bistable)
    }

    #[test]
    fn test_rs_reset_only() {
        let mut rs = RS::with_initial_state(true);

        // Reset R=TRUE
        rs.set_input("R1", Value::Bool(true));
        rs.execute();
        assert!(!rs.q1()); // Should be reset

        // Release R
        rs.set_input("R1", Value::Bool(false));
        rs.execute();
        assert!(!rs.q1()); // Should remain reset (bistable)
    }

    #[test]
    fn test_rs_reset_dominant() {
        let mut rs = RS::new();

        // Both S and R TRUE - R should dominate
        rs.set_input("S", Value::Bool(true));
        rs.set_input("R1", Value::Bool(true));
        rs.execute();
        assert!(!rs.q1()); // Reset dominant - Q should be FALSE
    }

    #[test]
    fn test_rs_sequence() {
        let mut rs = RS::new();

        // Initially FALSE
        assert!(!rs.q1());

        // Set
        rs.set_input("S", Value::Bool(true));
        rs.execute();
        assert!(rs.q1());

        // Release S, should stay set
        rs.set_input("S", Value::Bool(false));
        rs.execute();
        assert!(rs.q1());

        // Reset
        rs.set_input("R1", Value::Bool(true));
        rs.execute();
        assert!(!rs.q1());

        // Release R, should stay reset
        rs.set_input("R1", Value::Bool(false));
        rs.execute();
        assert!(!rs.q1());
    }

    #[test]
    fn test_rs_serialize_deserialize() {
        let mut rs1 = RS::new();
        rs1.set_input("S", Value::Bool(true));
        rs1.execute();

        let state = rs1.serialize_state();

        let mut rs2 = RS::new();
        assert!(rs2.deserialize_state(&state));
        assert_eq!(rs2.q1(), rs1.q1());
    }

    #[test]
    fn test_rs_function_block_trait() {
        let rs: Box<dyn FunctionBlock> = Box::new(RS::new());
        assert_eq!(rs.fb_type(), "RS");
        assert_eq!(rs.input_names(), vec!["S", "R1"]);
        // v1.2.3: Added Q1_NOT output
        assert_eq!(rs.output_names(), vec!["Q1", "Q1_NOT"]);
    }

    #[test]
    fn test_rs_reset_method() {
        let mut rs = RS::new();
        rs.set_input("S", Value::Bool(true));
        rs.execute();
        assert!(rs.q1());

        rs.reset();
        assert!(!rs.q1());
    }

    // ========================================================================
    // SR Tests
    // ========================================================================

    #[test]
    fn test_sr_initial_state() {
        let sr = SR::new();
        assert!(!sr.q1());
        assert!(!sr.is_set());
    }

    #[test]
    fn test_sr_with_initial_state() {
        let sr = SR::with_initial_state(true);
        assert!(sr.q1());
    }

    #[test]
    fn test_sr_set_only() {
        let mut sr = SR::new();

        // Set S=TRUE
        sr.set_input("S1", Value::Bool(true));
        sr.execute();
        assert!(sr.q1()); // Should be set

        // Release S
        sr.set_input("S1", Value::Bool(false));
        sr.execute();
        assert!(sr.q1()); // Should remain set (bistable)
    }

    #[test]
    fn test_sr_reset_only() {
        let mut sr = SR::with_initial_state(true);

        // Reset R=TRUE
        sr.set_input("R", Value::Bool(true));
        sr.execute();
        assert!(!sr.q1()); // Should be reset

        // Release R
        sr.set_input("R", Value::Bool(false));
        sr.execute();
        assert!(!sr.q1()); // Should remain reset (bistable)
    }

    #[test]
    fn test_sr_set_dominant() {
        let mut sr = SR::new();

        // Both S and R TRUE - S should dominate
        sr.set_input("S1", Value::Bool(true));
        sr.set_input("R", Value::Bool(true));
        sr.execute();
        assert!(sr.q1()); // Set dominant - Q should be TRUE
    }

    #[test]
    fn test_sr_sequence() {
        let mut sr = SR::new();

        // Initially FALSE
        assert!(!sr.q1());

        // Set
        sr.set_input("S1", Value::Bool(true));
        sr.execute();
        assert!(sr.q1());

        // Release S, should stay set
        sr.set_input("S1", Value::Bool(false));
        sr.execute();
        assert!(sr.q1());

        // Reset
        sr.set_input("R", Value::Bool(true));
        sr.execute();
        assert!(!sr.q1());

        // Release R, should stay reset
        sr.set_input("R", Value::Bool(false));
        sr.execute();
        assert!(!sr.q1());
    }

    #[test]
    fn test_sr_serialize_deserialize() {
        let mut sr1 = SR::new();
        sr1.set_input("S1", Value::Bool(true));
        sr1.execute();

        let state = sr1.serialize_state();

        let mut sr2 = SR::new();
        assert!(sr2.deserialize_state(&state));
        assert_eq!(sr2.q1(), sr1.q1());
    }

    #[test]
    fn test_sr_function_block_trait() {
        let sr: Box<dyn FunctionBlock> = Box::new(SR::new());
        assert_eq!(sr.fb_type(), "SR");
        assert_eq!(sr.input_names(), vec!["S1", "R"]);
        // v1.2.3: Added Q1_NOT output
        assert_eq!(sr.output_names(), vec!["Q1", "Q1_NOT"]);
    }

    #[test]
    fn test_sr_reset_method() {
        let mut sr = SR::new();
        sr.set_input("S1", Value::Bool(true));
        sr.execute();
        assert!(sr.q1());

        sr.reset();
        assert!(!sr.q1());
    }

    // ========================================================================
    // Dominance Comparison Tests
    // ========================================================================

    #[test]
    fn test_rs_vs_sr_dominance() {
        let mut rs = RS::new();
        let mut sr = SR::new();

        // Both S and R TRUE
        rs.set_input("S", Value::Bool(true));
        rs.set_input("R1", Value::Bool(true));
        sr.set_input("S1", Value::Bool(true));
        sr.set_input("R", Value::Bool(true));

        rs.execute();
        sr.execute();

        // RS: Reset dominant -> Q=FALSE
        // SR: Set dominant -> Q=TRUE
        assert!(!rs.q1());
        assert!(sr.q1());
    }

    /// v1.2.3: Test Q_NOT output (IEC 61131-3 compliance)
    #[test]
    fn test_q_not_output() {
        let mut rs = RS::new();

        // Initially Q=false, Q_NOT=true
        assert_eq!(rs.get_output("Q1"), Some(Value::Bool(false)));
        assert_eq!(rs.get_output("Q1_NOT"), Some(Value::Bool(true)));

        // Set
        rs.set_input("S", Value::Bool(true));
        rs.execute();

        // Q=true, Q_NOT=false
        assert_eq!(rs.get_output("Q1"), Some(Value::Bool(true)));
        assert_eq!(rs.get_output("Q1_NOT"), Some(Value::Bool(false)));
        assert_eq!(rs.get_output("QN"), Some(Value::Bool(false))); // Alias

        // Reset
        rs.set_input("S", Value::Bool(false));
        rs.set_input("R1", Value::Bool(true));
        rs.execute();

        // Q=false, Q_NOT=true
        assert_eq!(rs.get_output("Q1"), Some(Value::Bool(false)));
        assert_eq!(rs.get_output("Q1_NOT"), Some(Value::Bool(true)));
    }

    #[test]
    fn test_bistable_memory() {
        let mut rs = RS::new();

        // Set
        rs.set_input("S", Value::Bool(true));
        rs.execute();
        assert!(rs.q1());

        // Release both inputs
        rs.set_input("S", Value::Bool(false));
        rs.set_input("R1", Value::Bool(false));

        // Multiple cycles with no input - should retain state
        for _ in 0..10 {
            rs.execute();
            assert!(rs.q1()); // Memory retained
        }
    }

    #[test]
    fn test_alternating_set_reset() {
        let mut sr = SR::new();
        let expected = [true, true, false, false, true, false, true];
        let set_sequence = [true, false, false, false, true, false, true];
        let reset_sequence = [false, false, true, false, false, true, false];

        for i in 0..expected.len() {
            sr.set_input("S1", Value::Bool(set_sequence[i]));
            sr.set_input("R", Value::Bool(reset_sequence[i]));
            sr.execute();
            assert_eq!(
                sr.q1(),
                expected[i],
                "Failed at step {}: S={}, R={}, expected Q={}",
                i,
                set_sequence[i],
                reset_sequence[i],
                expected[i]
            );
        }
    }
}
