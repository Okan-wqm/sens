//! IEC 61131-3 Counter Function Blocks
//!
//! Standard counter implementations:
//! - CTU: Up Counter (counts up on rising edge of CU)
//! - CTD: Down Counter (counts down on rising edge of CD)
//! - CTUD: Up/Down Counter (bidirectional)
//!
//! All counters are edge-triggered (count on rising edge only).
//!
//! ## Overflow Protection (v1.2.2)
//! All counters are protected against overflow/underflow:
//! - CTU: Stops at i32::MAX with warning
//! - CTD: Stops at i32::MIN with warning
//! - CTUD: Stops at respective limits with warning

use super::FunctionBlock;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::warn;

// ============================================================================
// CTU - Up Counter
// ============================================================================

/// CTU (Counter Up) - IEC 61131-3
///
/// Counts up on each rising edge of CU input.
/// Q becomes TRUE when CV >= PV.
/// R (reset) sets CV to 0.
///
/// ```text
/// CU:  ___|‾|___|‾|___|‾|___|‾|___
/// CV:   0   1   2   3   4   (counts up)
/// Q:   ____________|‾‾‾‾‾‾‾‾‾‾‾‾ (when CV >= PV)
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CTU {
    /// Preset value (threshold for Q output)
    pv: i32,
    /// Current count value
    cv: i32,
    /// Count up input
    cu: bool,
    /// Reset input
    reset: bool,
    /// Output (CV >= PV)
    q: bool,
    /// Previous CU state for edge detection
    prev_cu: bool,
}

impl CTU {
    /// Create new CTU counter with preset value
    pub fn new(preset: i32) -> Self {
        Self {
            pv: preset,
            cv: 0,
            cu: false,
            reset: false,
            q: false,
            prev_cu: false,
        }
    }

    /// Get current count value
    pub fn count(&self) -> i32 {
        self.cv
    }

    /// Get preset value
    pub fn preset(&self) -> i32 {
        self.pv
    }

    /// Get output state
    pub fn q(&self) -> bool {
        self.q
    }
}

impl FunctionBlock for CTU {
    fn fb_type(&self) -> &'static str {
        "CTU"
    }

    fn execute(&mut self) {
        // Reset takes priority
        if self.reset {
            self.cv = 0;
        } else if self.cu && !self.prev_cu {
            // Rising edge on CU - count up
            if self.cv < i32::MAX {
                self.cv += 1;
            } else {
                // v1.2.2: Overflow detection with warning
                warn!(
                    "CTU overflow: counter at maximum value {} (PV={})",
                    i32::MAX,
                    self.pv
                );
            }
        }

        // Update output
        self.q = self.cv >= self.pv;
        self.prev_cu = self.cu;
    }

    fn get_output(&self, name: &str) -> Option<Value> {
        match name {
            "Q" | "q" | "output" => Some(Value::Bool(self.q)),
            "CV" | "cv" | "count" => Some(Value::Number(self.cv.into())),
            _ => None,
        }
    }

    fn set_input(&mut self, name: &str, value: Value) -> bool {
        match name {
            "CU" | "cu" | "count_up" => {
                if let Some(v) = value.as_bool() {
                    self.cu = v;
                    return true;
                }
            }
            "R" | "r" | "reset" => {
                if let Some(v) = value.as_bool() {
                    self.reset = v;
                    return true;
                }
            }
            "PV" | "pv" | "preset" => {
                if let Some(v) = value.as_i64() {
                    self.pv = v as i32;
                    return true;
                }
            }
            _ => {}
        }
        false
    }

    fn serialize_state(&self) -> Value {
        json!({
            "pv": self.pv,
            "cv": self.cv,
            "cu": self.cu,
            "reset": self.reset,
            "q": self.q,
            "prev_cu": self.prev_cu
        })
    }

    fn deserialize_state(&mut self, state: &Value) -> bool {
        if let Some(obj) = state.as_object() {
            if let Some(pv) = obj.get("pv").and_then(|v| v.as_i64()) {
                self.pv = pv as i32;
            }
            if let Some(cv) = obj.get("cv").and_then(|v| v.as_i64()) {
                self.cv = cv as i32;
            }
            if let Some(cu) = obj.get("cu").and_then(|v| v.as_bool()) {
                self.cu = cu;
            }
            if let Some(reset) = obj.get("reset").and_then(|v| v.as_bool()) {
                self.reset = reset;
            }
            if let Some(q) = obj.get("q").and_then(|v| v.as_bool()) {
                self.q = q;
            }
            if let Some(prev) = obj.get("prev_cu").and_then(|v| v.as_bool()) {
                self.prev_cu = prev;
            }
            return true;
        }
        false
    }

    fn reset(&mut self) {
        self.cv = 0;
        self.q = false;
        self.cu = false;
        self.reset = false;
        self.prev_cu = false;
    }

    fn input_names(&self) -> Vec<&'static str> {
        vec!["CU", "R", "PV"]
    }

    fn output_names(&self) -> Vec<&'static str> {
        vec!["Q", "CV"]
    }
}

// ============================================================================
// CTD - Down Counter
// ============================================================================

/// CTD (Counter Down) - IEC 61131-3
///
/// Counts down on each rising edge of CD input.
/// Q becomes TRUE when CV <= 0.
/// LD (load) sets CV to PV.
///
/// ```text
/// CD:  ___|‾|___|‾|___|‾|___|‾|___
/// CV:   5   4   3   2   1   0  (counts down)
/// Q:   ________________________|‾‾ (when CV <= 0)
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CTD {
    /// Preset value (initial value when loaded)
    pv: i32,
    /// Current count value
    cv: i32,
    /// Count down input
    cd: bool,
    /// Load input (sets CV to PV)
    load: bool,
    /// Output (CV <= 0)
    q: bool,
    /// Previous CD state for edge detection
    prev_cd: bool,
}

impl CTD {
    /// Create new CTD counter with preset value
    pub fn new(preset: i32) -> Self {
        Self {
            pv: preset,
            cv: preset, // Start at preset
            cd: false,
            load: false,
            q: false,
            prev_cd: false,
        }
    }

    /// Get current count value
    pub fn count(&self) -> i32 {
        self.cv
    }

    /// Get preset value
    pub fn preset(&self) -> i32 {
        self.pv
    }

    /// Get output state
    pub fn q(&self) -> bool {
        self.q
    }
}

impl FunctionBlock for CTD {
    fn fb_type(&self) -> &'static str {
        "CTD"
    }

    fn execute(&mut self) {
        // Load takes priority
        if self.load {
            self.cv = self.pv;
        } else if self.cd && !self.prev_cd {
            // Rising edge on CD - count down
            if self.cv > i32::MIN {
                self.cv -= 1;
            } else {
                // v1.2.2: Underflow detection with warning
                warn!(
                    "CTD underflow: counter at minimum value {} (PV={})",
                    i32::MIN,
                    self.pv
                );
            }
        }

        // Update output
        self.q = self.cv <= 0;
        self.prev_cd = self.cd;
    }

    fn get_output(&self, name: &str) -> Option<Value> {
        match name {
            "Q" | "q" | "output" => Some(Value::Bool(self.q)),
            "CV" | "cv" | "count" => Some(Value::Number(self.cv.into())),
            _ => None,
        }
    }

    fn set_input(&mut self, name: &str, value: Value) -> bool {
        match name {
            "CD" | "cd" | "count_down" => {
                if let Some(v) = value.as_bool() {
                    self.cd = v;
                    return true;
                }
            }
            "LD" | "ld" | "load" => {
                if let Some(v) = value.as_bool() {
                    self.load = v;
                    return true;
                }
            }
            "PV" | "pv" | "preset" => {
                if let Some(v) = value.as_i64() {
                    self.pv = v as i32;
                    return true;
                }
            }
            _ => {}
        }
        false
    }

    fn serialize_state(&self) -> Value {
        json!({
            "pv": self.pv,
            "cv": self.cv,
            "cd": self.cd,
            "load": self.load,
            "q": self.q,
            "prev_cd": self.prev_cd
        })
    }

    fn deserialize_state(&mut self, state: &Value) -> bool {
        if let Some(obj) = state.as_object() {
            if let Some(pv) = obj.get("pv").and_then(|v| v.as_i64()) {
                self.pv = pv as i32;
            }
            if let Some(cv) = obj.get("cv").and_then(|v| v.as_i64()) {
                self.cv = cv as i32;
            }
            if let Some(cd) = obj.get("cd").and_then(|v| v.as_bool()) {
                self.cd = cd;
            }
            if let Some(load) = obj.get("load").and_then(|v| v.as_bool()) {
                self.load = load;
            }
            if let Some(q) = obj.get("q").and_then(|v| v.as_bool()) {
                self.q = q;
            }
            if let Some(prev) = obj.get("prev_cd").and_then(|v| v.as_bool()) {
                self.prev_cd = prev;
            }
            return true;
        }
        false
    }

    fn reset(&mut self) {
        self.cv = self.pv;
        self.q = false;
        self.cd = false;
        self.load = false;
        self.prev_cd = false;
    }

    fn input_names(&self) -> Vec<&'static str> {
        vec!["CD", "LD", "PV"]
    }

    fn output_names(&self) -> Vec<&'static str> {
        vec!["Q", "CV"]
    }
}

// ============================================================================
// CTUD - Up/Down Counter
// ============================================================================

/// CTUD (Counter Up/Down) - IEC 61131-3
///
/// Bidirectional counter with separate up and down inputs.
/// QU becomes TRUE when CV >= PV.
/// QD becomes TRUE when CV <= 0.
/// R (reset) sets CV to 0.
/// LD (load) sets CV to PV.
///
/// ```text
/// CU:  _|‾|___|‾|___|‾|_________
/// CD:  ___________|‾|___|‾|_____
/// CV:   0  1   2   3   2   1
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CTUD {
    /// Preset value
    pv: i32,
    /// Current count value
    cv: i32,
    /// Count up input
    cu: bool,
    /// Count down input
    cd: bool,
    /// Reset input (sets CV to 0)
    reset: bool,
    /// Load input (sets CV to PV)
    load: bool,
    /// Up output (CV >= PV)
    qu: bool,
    /// Down output (CV <= 0)
    qd: bool,
    /// Previous CU state for edge detection
    prev_cu: bool,
    /// Previous CD state for edge detection
    prev_cd: bool,
}

impl CTUD {
    /// Create new CTUD counter with preset value
    pub fn new(preset: i32) -> Self {
        Self {
            pv: preset,
            cv: 0,
            cu: false,
            cd: false,
            reset: false,
            load: false,
            qu: false,
            qd: true, // CV = 0, so QD is true
            prev_cu: false,
            prev_cd: false,
        }
    }

    /// Get current count value
    pub fn count(&self) -> i32 {
        self.cv
    }

    /// Get preset value
    pub fn preset(&self) -> i32 {
        self.pv
    }

    /// Get up output state
    pub fn qu(&self) -> bool {
        self.qu
    }

    /// Get down output state
    pub fn qd(&self) -> bool {
        self.qd
    }
}

impl FunctionBlock for CTUD {
    fn fb_type(&self) -> &'static str {
        "CTUD"
    }

    fn execute(&mut self) {
        // Reset takes highest priority
        if self.reset {
            self.cv = 0;
        } else if self.load {
            // Load takes second priority
            self.cv = self.pv;
        } else {
            // Count up on rising edge of CU
            if self.cu && !self.prev_cu {
                if self.cv < i32::MAX {
                    self.cv += 1;
                } else {
                    // v1.2.2: Overflow detection with warning
                    warn!(
                        "CTUD overflow: counter at maximum value {} (PV={})",
                        i32::MAX,
                        self.pv
                    );
                }
            }
            // Count down on rising edge of CD
            if self.cd && !self.prev_cd {
                if self.cv > i32::MIN {
                    self.cv -= 1;
                } else {
                    // v1.2.2: Underflow detection with warning
                    warn!(
                        "CTUD underflow: counter at minimum value {} (PV={})",
                        i32::MIN,
                        self.pv
                    );
                }
            }
        }

        // Update outputs
        self.qu = self.cv >= self.pv;
        self.qd = self.cv <= 0;

        self.prev_cu = self.cu;
        self.prev_cd = self.cd;
    }

    fn get_output(&self, name: &str) -> Option<Value> {
        match name {
            "QU" | "qu" | "up_output" => Some(Value::Bool(self.qu)),
            "QD" | "qd" | "down_output" => Some(Value::Bool(self.qd)),
            "CV" | "cv" | "count" => Some(Value::Number(self.cv.into())),
            _ => None,
        }
    }

    fn set_input(&mut self, name: &str, value: Value) -> bool {
        match name {
            "CU" | "cu" | "count_up" => {
                if let Some(v) = value.as_bool() {
                    self.cu = v;
                    return true;
                }
            }
            "CD" | "cd" | "count_down" => {
                if let Some(v) = value.as_bool() {
                    self.cd = v;
                    return true;
                }
            }
            "R" | "r" | "reset" => {
                if let Some(v) = value.as_bool() {
                    self.reset = v;
                    return true;
                }
            }
            "LD" | "ld" | "load" => {
                if let Some(v) = value.as_bool() {
                    self.load = v;
                    return true;
                }
            }
            "PV" | "pv" | "preset" => {
                if let Some(v) = value.as_i64() {
                    self.pv = v as i32;
                    return true;
                }
            }
            _ => {}
        }
        false
    }

    fn serialize_state(&self) -> Value {
        json!({
            "pv": self.pv,
            "cv": self.cv,
            "cu": self.cu,
            "cd": self.cd,
            "reset": self.reset,
            "load": self.load,
            "qu": self.qu,
            "qd": self.qd,
            "prev_cu": self.prev_cu,
            "prev_cd": self.prev_cd
        })
    }

    fn deserialize_state(&mut self, state: &Value) -> bool {
        if let Some(obj) = state.as_object() {
            if let Some(pv) = obj.get("pv").and_then(|v| v.as_i64()) {
                self.pv = pv as i32;
            }
            if let Some(cv) = obj.get("cv").and_then(|v| v.as_i64()) {
                self.cv = cv as i32;
            }
            if let Some(cu) = obj.get("cu").and_then(|v| v.as_bool()) {
                self.cu = cu;
            }
            if let Some(cd) = obj.get("cd").and_then(|v| v.as_bool()) {
                self.cd = cd;
            }
            if let Some(reset) = obj.get("reset").and_then(|v| v.as_bool()) {
                self.reset = reset;
            }
            if let Some(load) = obj.get("load").and_then(|v| v.as_bool()) {
                self.load = load;
            }
            if let Some(qu) = obj.get("qu").and_then(|v| v.as_bool()) {
                self.qu = qu;
            }
            if let Some(qd) = obj.get("qd").and_then(|v| v.as_bool()) {
                self.qd = qd;
            }
            if let Some(prev_cu) = obj.get("prev_cu").and_then(|v| v.as_bool()) {
                self.prev_cu = prev_cu;
            }
            if let Some(prev_cd) = obj.get("prev_cd").and_then(|v| v.as_bool()) {
                self.prev_cd = prev_cd;
            }
            return true;
        }
        false
    }

    fn reset(&mut self) {
        self.cv = 0;
        self.qu = false;
        self.qd = true;
        self.cu = false;
        self.cd = false;
        self.reset = false;
        self.load = false;
        self.prev_cu = false;
        self.prev_cd = false;
    }

    fn input_names(&self) -> Vec<&'static str> {
        vec!["CU", "CD", "R", "LD", "PV"]
    }

    fn output_names(&self) -> Vec<&'static str> {
        vec!["QU", "QD", "CV"]
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ctu_basic() {
        let mut ctu = CTU::new(3); // Preset = 3

        // Count up 3 times
        for i in 0..3 {
            ctu.set_input("CU", Value::Bool(true));
            ctu.execute();
            assert_eq!(ctu.count(), i + 1);
            ctu.set_input("CU", Value::Bool(false));
            ctu.execute();
        }

        // Output should be true (CV >= PV)
        assert!(ctu.q());
        assert_eq!(ctu.count(), 3);
    }

    #[test]
    fn test_ctu_reset() {
        let mut ctu = CTU::new(5);

        // Count up
        ctu.set_input("CU", Value::Bool(true));
        ctu.execute();
        ctu.set_input("CU", Value::Bool(false));
        ctu.execute();
        ctu.set_input("CU", Value::Bool(true));
        ctu.execute();
        assert_eq!(ctu.count(), 2);

        // Reset
        ctu.set_input("R", Value::Bool(true));
        ctu.execute();
        assert_eq!(ctu.count(), 0);
        assert!(!ctu.q());
    }

    #[test]
    fn test_ctu_edge_trigger() {
        let mut ctu = CTU::new(10);

        // Holding CU high should only count once
        ctu.set_input("CU", Value::Bool(true));
        ctu.execute();
        ctu.execute();
        ctu.execute();
        assert_eq!(ctu.count(), 1); // Only counted on first rising edge
    }

    #[test]
    fn test_ctd_basic() {
        let mut ctd = CTD::new(3); // Start at 3

        assert_eq!(ctd.count(), 3);
        assert!(!ctd.q());

        // Count down
        for i in 0..3 {
            ctd.set_input("CD", Value::Bool(true));
            ctd.execute();
            assert_eq!(ctd.count(), 2 - i as i32);
            ctd.set_input("CD", Value::Bool(false));
            ctd.execute();
        }

        // Output should be true (CV <= 0)
        assert!(ctd.q());
        assert_eq!(ctd.count(), 0);
    }

    #[test]
    fn test_ctd_load() {
        let mut ctd = CTD::new(10);

        // Count down
        ctd.set_input("CD", Value::Bool(true));
        ctd.execute();
        ctd.set_input("CD", Value::Bool(false));
        ctd.execute();
        assert_eq!(ctd.count(), 9);

        // Load
        ctd.set_input("LD", Value::Bool(true));
        ctd.execute();
        assert_eq!(ctd.count(), 10);
    }

    #[test]
    fn test_ctud_bidirectional() {
        let mut ctud = CTUD::new(5);

        // Count up 3 times
        for _ in 0..3 {
            ctud.set_input("CU", Value::Bool(true));
            ctud.execute();
            ctud.set_input("CU", Value::Bool(false));
            ctud.execute();
        }
        assert_eq!(ctud.count(), 3);

        // Count down 2 times
        for _ in 0..2 {
            ctud.set_input("CD", Value::Bool(true));
            ctud.execute();
            ctud.set_input("CD", Value::Bool(false));
            ctud.execute();
        }
        assert_eq!(ctud.count(), 1);
    }

    #[test]
    fn test_ctud_outputs() {
        let mut ctud = CTUD::new(2);

        // Initially CV = 0, so QD is true
        assert!(ctud.qd());
        assert!(!ctud.qu());

        // Count up to 2
        for _ in 0..2 {
            ctud.set_input("CU", Value::Bool(true));
            ctud.execute();
            ctud.set_input("CU", Value::Bool(false));
            ctud.execute();
        }

        // CV >= PV, so QU is true
        assert!(ctud.qu());
        assert!(!ctud.qd());
    }

    #[test]
    fn test_counter_serialize_deserialize() {
        let mut ctu1 = CTU::new(10);

        // Count up
        for _ in 0..5 {
            ctu1.set_input("CU", Value::Bool(true));
            ctu1.execute();
            ctu1.set_input("CU", Value::Bool(false));
            ctu1.execute();
        }

        // Serialize
        let state = ctu1.serialize_state();

        // Deserialize into new counter
        let mut ctu2 = CTU::new(0);
        ctu2.deserialize_state(&state);

        assert_eq!(ctu2.preset(), 10);
        assert_eq!(ctu2.count(), 5);
    }
}
