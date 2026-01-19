//! IEC 61131-3 Control Function Blocks
//!
//! Advanced control function blocks for industrial automation:
//! - PID: Proportional-Integral-Derivative controller
//! - MAVG: Moving Average filter for signal smoothing
//!
//! # PID Controller
//! Implements a standard PID control algorithm with:
//! - Anti-windup protection
//! - Output clamping
//! - Manual/Auto mode switching
//! - Derivative filtering
//!
//! # v1.2.4: New function blocks for control applications

use super::FunctionBlock;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::VecDeque;

/// PID Controller Function Block (IEC 61131-3)
///
/// Standard PID controller with anti-windup and output limiting.
///
/// # Inputs
/// - `SP` (f64): Setpoint - desired value
/// - `PV` (f64): Process Variable - measured value
/// - `KP` (f64): Proportional gain
/// - `KI` (f64): Integral gain (1/s)
/// - `KD` (f64): Derivative gain (s)
/// - `OUT_MIN` (f64): Minimum output limit
/// - `OUT_MAX` (f64): Maximum output limit
/// - `MANUAL` (bool): Manual mode enable
/// - `MAN_OUT` (f64): Manual output value
/// - `RESET` (bool): Reset integrator
///
/// # Outputs
/// - `OUT` (f64): Controller output
/// - `ERROR` (f64): Current error (SP - PV)
/// - `P_TERM` (f64): Proportional term
/// - `I_TERM` (f64): Integral term
/// - `D_TERM` (f64): Derivative term
/// - `SATURATED` (bool): Output is at limit
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PID {
    // Inputs
    sp: f64,      // Setpoint
    pv: f64,      // Process variable
    kp: f64,      // Proportional gain
    ki: f64,      // Integral gain
    kd: f64,      // Derivative gain
    out_min: f64, // Output minimum
    out_max: f64, // Output maximum
    manual: bool, // Manual mode
    man_out: f64, // Manual output
    reset: bool,  // Reset integrator

    // Internal state
    integral: f64,    // Integral accumulator
    prev_error: f64,  // Previous error for derivative
    prev_pv: f64,     // Previous PV (for derivative on PV)
    last_output: f64, // Last output value
    saturated: bool,  // Output saturation flag
    first_run: bool,  // First execution flag

    // Timing
    sample_time_ms: u64, // Sample time in milliseconds
    last_time_ms: u64,   // Last execution time

    // Outputs (cached)
    output: f64,
    error: f64,
    p_term: f64,
    i_term: f64,
    d_term: f64,
}

impl Default for PID {
    fn default() -> Self {
        Self::new(1.0, 0.0, 0.0) // Default: P-only controller
    }
}

impl PID {
    /// Create a new PID controller with specified gains
    pub fn new(kp: f64, ki: f64, kd: f64) -> Self {
        Self {
            sp: 0.0,
            pv: 0.0,
            kp,
            ki,
            kd,
            out_min: 0.0,
            out_max: 100.0,
            manual: false,
            man_out: 0.0,
            reset: false,
            integral: 0.0,
            prev_error: 0.0,
            prev_pv: 0.0,
            last_output: 0.0,
            saturated: false,
            first_run: true,
            sample_time_ms: 100, // Default 100ms sample time
            last_time_ms: 0,
            output: 0.0,
            error: 0.0,
            p_term: 0.0,
            i_term: 0.0,
            d_term: 0.0,
        }
    }

    /// Create PID with output limits
    pub fn with_limits(kp: f64, ki: f64, kd: f64, out_min: f64, out_max: f64) -> Self {
        let mut pid = Self::new(kp, ki, kd);
        pid.out_min = out_min;
        pid.out_max = out_max;
        pid
    }

    /// Clamp value to output limits
    fn clamp(&self, value: f64) -> f64 {
        value.clamp(self.out_min, self.out_max)
    }

    /// Get current output
    pub fn output(&self) -> f64 {
        self.output
    }

    /// Get current error
    pub fn error(&self) -> f64 {
        self.error
    }

    /// Check if output is saturated
    pub fn is_saturated(&self) -> bool {
        self.saturated
    }
}

impl FunctionBlock for PID {
    fn fb_type(&self) -> &'static str {
        "PID"
    }

    fn execute(&mut self) {
        // Handle reset
        if self.reset {
            self.integral = 0.0;
            self.prev_error = 0.0;
            self.prev_pv = self.pv;
            self.first_run = true;
            self.reset = false;
        }

        // Manual mode - bypass controller
        if self.manual {
            self.output = self.clamp(self.man_out);
            self.saturated = self.output != self.man_out;
            self.p_term = 0.0;
            self.i_term = 0.0;
            self.d_term = 0.0;
            self.error = self.sp - self.pv;
            return;
        }

        // Calculate error
        self.error = self.sp - self.pv;

        // Get current time
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        // Calculate dt in seconds
        let dt = if self.first_run || self.last_time_ms == 0 {
            self.sample_time_ms as f64 / 1000.0
        } else {
            (now_ms.saturating_sub(self.last_time_ms)) as f64 / 1000.0
        };

        // Prevent division by zero or huge dt
        let dt = dt.clamp(0.001, 10.0);

        // Proportional term
        self.p_term = self.kp * self.error;

        // Integral term with anti-windup
        // Only integrate if output is not saturated (or error is reducing saturation)
        // Skip integration on first run to allow bumpless transfer
        let should_integrate = !self.first_run
            && (!self.saturated
                || (self.error > 0.0 && self.last_output <= self.out_min)
                || (self.error < 0.0 && self.last_output >= self.out_max));

        if should_integrate && self.ki != 0.0 {
            self.integral += self.error * dt;
            // Clamp integral to prevent excessive windup
            let i_max = (self.out_max - self.out_min) / self.ki.abs().max(0.001);
            self.integral = self.integral.clamp(-i_max, i_max);
        }
        self.i_term = self.ki * self.integral;

        // Derivative term (on PV to avoid derivative kick on SP change)
        if self.first_run {
            self.d_term = 0.0;
            self.prev_pv = self.pv;
        } else if self.kd != 0.0 {
            // Derivative on PV (negative because we want d(error)/dt = -d(PV)/dt when SP constant)
            let d_pv = (self.pv - self.prev_pv) / dt;
            self.d_term = -self.kd * d_pv;
        } else {
            self.d_term = 0.0;
        }

        // Calculate output
        let raw_output = self.p_term + self.i_term + self.d_term;
        self.output = self.clamp(raw_output);
        self.saturated = self.output != raw_output;

        // Update state for next cycle
        self.prev_error = self.error;
        self.prev_pv = self.pv;
        self.last_output = self.output;
        self.last_time_ms = now_ms;
        self.first_run = false;
    }

    fn get_output(&self, name: &str) -> Option<Value> {
        match name.to_uppercase().as_str() {
            "OUT" | "OUTPUT" | "CV" => Some(json!(self.output)),
            "ERROR" | "E" | "ERR" => Some(json!(self.error)),
            "P_TERM" | "P" | "PTERM" => Some(json!(self.p_term)),
            "I_TERM" | "I" | "ITERM" => Some(json!(self.i_term)),
            "D_TERM" | "D" | "DTERM" => Some(json!(self.d_term)),
            "SATURATED" | "SAT" | "LIM" => Some(json!(self.saturated)),
            _ => None,
        }
    }

    fn set_input(&mut self, name: &str, value: Value) -> bool {
        match name.to_uppercase().as_str() {
            "SP" | "SETPOINT" | "SV" => {
                if let Some(v) = value.as_f64() {
                    self.sp = v;
                    return true;
                }
            }
            "PV" | "PROCESS_VALUE" | "INPUT" | "IN" => {
                if let Some(v) = value.as_f64() {
                    self.pv = v;
                    return true;
                }
            }
            "KP" | "P_GAIN" | "PGAIN" => {
                if let Some(v) = value.as_f64() {
                    self.kp = v.max(0.0);
                    return true;
                }
            }
            "KI" | "I_GAIN" | "IGAIN" => {
                if let Some(v) = value.as_f64() {
                    self.ki = v.max(0.0);
                    return true;
                }
            }
            "KD" | "D_GAIN" | "DGAIN" => {
                if let Some(v) = value.as_f64() {
                    self.kd = v.max(0.0);
                    return true;
                }
            }
            "OUT_MIN" | "MIN" | "LO_LIM" => {
                if let Some(v) = value.as_f64() {
                    self.out_min = v;
                    return true;
                }
            }
            "OUT_MAX" | "MAX" | "HI_LIM" => {
                if let Some(v) = value.as_f64() {
                    self.out_max = v;
                    return true;
                }
            }
            "MANUAL" | "MAN" | "AUTO" => {
                if let Some(v) = value.as_bool() {
                    // AUTO input is inverted
                    self.manual = if name.to_uppercase() == "AUTO" { !v } else { v };
                    return true;
                }
            }
            "MAN_OUT" | "MANUAL_VALUE" | "MV" => {
                if let Some(v) = value.as_f64() {
                    self.man_out = v;
                    return true;
                }
            }
            "RESET" | "RST" | "CLEAR" => {
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
            "integral": self.integral,
            "prev_error": self.prev_error,
            "prev_pv": self.prev_pv,
            "last_output": self.last_output,
            "last_time_ms": self.last_time_ms,
            "first_run": self.first_run
        })
    }

    fn deserialize_state(&mut self, state: &Value) -> bool {
        if let Some(obj) = state.as_object() {
            if let Some(v) = obj.get("integral").and_then(|v| v.as_f64()) {
                self.integral = v;
            }
            if let Some(v) = obj.get("prev_error").and_then(|v| v.as_f64()) {
                self.prev_error = v;
            }
            if let Some(v) = obj.get("prev_pv").and_then(|v| v.as_f64()) {
                self.prev_pv = v;
            }
            if let Some(v) = obj.get("last_output").and_then(|v| v.as_f64()) {
                self.last_output = v;
            }
            if let Some(v) = obj.get("last_time_ms").and_then(|v| v.as_u64()) {
                self.last_time_ms = v;
            }
            if let Some(v) = obj.get("first_run").and_then(|v| v.as_bool()) {
                self.first_run = v;
            }
            return true;
        }
        false
    }

    fn reset(&mut self) {
        self.integral = 0.0;
        self.prev_error = 0.0;
        self.prev_pv = self.pv;
        self.last_output = 0.0;
        self.saturated = false;
        self.first_run = true;
        self.output = 0.0;
        self.error = 0.0;
        self.p_term = 0.0;
        self.i_term = 0.0;
        self.d_term = 0.0;
    }

    fn input_names(&self) -> Vec<&'static str> {
        vec![
            "SP", "PV", "KP", "KI", "KD", "OUT_MIN", "OUT_MAX", "MANUAL", "MAN_OUT", "RESET",
        ]
    }

    fn output_names(&self) -> Vec<&'static str> {
        vec!["OUT", "ERROR", "P_TERM", "I_TERM", "D_TERM", "SATURATED"]
    }
}

/// Moving Average Filter Function Block
///
/// Calculates the moving average of input values over a configurable window.
/// Useful for smoothing noisy sensor readings.
///
/// # Inputs
/// - `IN` (f64): Input value to filter
/// - `N` (u32): Window size (number of samples, default: 10)
/// - `RESET` (bool): Clear the filter buffer
///
/// # Outputs
/// - `OUT` (f64): Filtered output (moving average)
/// - `VALID` (bool): True when buffer is full (steady-state)
/// - `COUNT` (u32): Current number of samples in buffer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MAVG {
    // Inputs
    input: f64,
    window_size: usize,
    reset: bool,

    // Internal state
    buffer: VecDeque<f64>,
    sum: f64,

    // Outputs (cached)
    output: f64,
    valid: bool,
}

impl Default for MAVG {
    fn default() -> Self {
        Self::new(10) // Default 10-sample window
    }
}

impl MAVG {
    /// Create a new moving average filter with specified window size
    pub fn new(window_size: usize) -> Self {
        let window_size = window_size.max(1).min(1000); // Clamp to reasonable range
        Self {
            input: 0.0,
            window_size,
            reset: false,
            buffer: VecDeque::with_capacity(window_size),
            sum: 0.0,
            output: 0.0,
            valid: false,
        }
    }

    /// Get current output
    pub fn output(&self) -> f64 {
        self.output
    }

    /// Check if filter has reached steady state
    pub fn is_valid(&self) -> bool {
        self.valid
    }
}

impl FunctionBlock for MAVG {
    fn fb_type(&self) -> &'static str {
        "MAVG"
    }

    fn execute(&mut self) {
        // Handle reset
        if self.reset {
            self.buffer.clear();
            self.sum = 0.0;
            self.output = 0.0;
            self.valid = false;
            self.reset = false;
            return;
        }

        // Add new value to buffer
        self.buffer.push_back(self.input);
        self.sum += self.input;

        // Remove oldest value if buffer is full
        if self.buffer.len() > self.window_size {
            if let Some(old) = self.buffer.pop_front() {
                self.sum -= old;
            }
        }

        // Calculate average
        if !self.buffer.is_empty() {
            self.output = self.sum / self.buffer.len() as f64;
        }

        // Check if buffer is full (steady state)
        self.valid = self.buffer.len() >= self.window_size;
    }

    fn get_output(&self, name: &str) -> Option<Value> {
        match name.to_uppercase().as_str() {
            "OUT" | "OUTPUT" | "AVG" | "AVERAGE" => Some(json!(self.output)),
            "VALID" | "READY" | "FULL" => Some(json!(self.valid)),
            "COUNT" | "N" | "SIZE" => Some(json!(self.buffer.len())),
            _ => None,
        }
    }

    fn set_input(&mut self, name: &str, value: Value) -> bool {
        match name.to_uppercase().as_str() {
            "IN" | "INPUT" | "VALUE" => {
                if let Some(v) = value.as_f64() {
                    self.input = v;
                    return true;
                }
            }
            "N" | "WINDOW" | "SIZE" | "SAMPLES" => {
                if let Some(v) = value.as_u64() {
                    let new_size = (v as usize).max(1).min(1000);
                    if new_size != self.window_size {
                        self.window_size = new_size;
                        // Trim buffer if new size is smaller
                        while self.buffer.len() > self.window_size {
                            if let Some(old) = self.buffer.pop_front() {
                                self.sum -= old;
                            }
                        }
                    }
                    return true;
                }
            }
            "RESET" | "RST" | "CLEAR" => {
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
            "buffer": self.buffer.iter().collect::<Vec<_>>(),
            "sum": self.sum,
            "window_size": self.window_size
        })
    }

    fn deserialize_state(&mut self, state: &Value) -> bool {
        if let Some(obj) = state.as_object() {
            if let Some(arr) = obj.get("buffer").and_then(|v| v.as_array()) {
                self.buffer.clear();
                self.sum = 0.0;
                for v in arr {
                    if let Some(f) = v.as_f64() {
                        self.buffer.push_back(f);
                        self.sum += f;
                    }
                }
            }
            if let Some(v) = obj.get("window_size").and_then(|v| v.as_u64()) {
                self.window_size = (v as usize).max(1).min(1000);
            }
            self.valid = self.buffer.len() >= self.window_size;
            if !self.buffer.is_empty() {
                self.output = self.sum / self.buffer.len() as f64;
            }
            return true;
        }
        false
    }

    fn reset(&mut self) {
        self.buffer.clear();
        self.sum = 0.0;
        self.output = 0.0;
        self.valid = false;
    }

    fn input_names(&self) -> Vec<&'static str> {
        vec!["IN", "N", "RESET"]
    }

    fn output_names(&self) -> Vec<&'static str> {
        vec!["OUT", "VALID", "COUNT"]
    }
}

/// Hysteresis Function Block
///
/// Implements a Schmitt trigger with configurable high/low thresholds.
/// Useful for preventing oscillation around a setpoint.
///
/// # Inputs
/// - `IN` (f64): Input value
/// - `HIGH` (f64): High threshold (turn on)
/// - `LOW` (f64): Low threshold (turn off)
///
/// # Outputs
/// - `OUT` (bool): Output state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HYSTERESIS {
    // Inputs
    input: f64,
    high_threshold: f64,
    low_threshold: f64,

    // State
    output: bool,
}

impl Default for HYSTERESIS {
    fn default() -> Self {
        Self::new(60.0, 40.0) // Default: 60/40 thresholds
    }
}

impl HYSTERESIS {
    /// Create a new hysteresis block with specified thresholds
    pub fn new(high: f64, low: f64) -> Self {
        Self {
            input: 0.0,
            high_threshold: high,
            low_threshold: low,
            output: false,
        }
    }
}

impl FunctionBlock for HYSTERESIS {
    fn fb_type(&self) -> &'static str {
        "HYSTERESIS"
    }

    fn execute(&mut self) {
        if self.output {
            // Currently ON - turn off if below low threshold
            if self.input < self.low_threshold {
                self.output = false;
            }
        } else {
            // Currently OFF - turn on if above high threshold
            if self.input > self.high_threshold {
                self.output = true;
            }
        }
    }

    fn get_output(&self, name: &str) -> Option<Value> {
        match name.to_uppercase().as_str() {
            "OUT" | "OUTPUT" | "Q" => Some(json!(self.output)),
            _ => None,
        }
    }

    fn set_input(&mut self, name: &str, value: Value) -> bool {
        match name.to_uppercase().as_str() {
            "IN" | "INPUT" | "VALUE" => {
                if let Some(v) = value.as_f64() {
                    self.input = v;
                    return true;
                }
            }
            "HIGH" | "HI" | "ON" | "HIGH_THRESHOLD" => {
                if let Some(v) = value.as_f64() {
                    self.high_threshold = v;
                    return true;
                }
            }
            "LOW" | "LO" | "OFF" | "LOW_THRESHOLD" => {
                if let Some(v) = value.as_f64() {
                    self.low_threshold = v;
                    return true;
                }
            }
            _ => {}
        }
        false
    }

    fn serialize_state(&self) -> Value {
        json!({
            "output": self.output
        })
    }

    fn deserialize_state(&mut self, state: &Value) -> bool {
        if let Some(obj) = state.as_object() {
            if let Some(v) = obj.get("output").and_then(|v| v.as_bool()) {
                self.output = v;
            }
            return true;
        }
        false
    }

    fn reset(&mut self) {
        self.output = false;
    }

    fn input_names(&self) -> Vec<&'static str> {
        vec!["IN", "HIGH", "LOW"]
    }

    fn output_names(&self) -> Vec<&'static str> {
        vec!["OUT"]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pid_proportional_only() {
        let mut pid = PID::new(1.0, 0.0, 0.0);
        pid.set_input("SP", json!(100.0));
        pid.set_input("PV", json!(50.0));
        pid.set_input("OUT_MIN", json!(0.0));
        pid.set_input("OUT_MAX", json!(100.0));

        pid.execute();

        // P-only: error = 100 - 50 = 50, output = 1.0 * 50 = 50
        assert_eq!(pid.get_output("ERROR"), Some(json!(50.0)));
        assert_eq!(pid.get_output("OUT"), Some(json!(50.0)));
    }

    #[test]
    fn test_pid_output_clamping() {
        let mut pid = PID::with_limits(10.0, 0.0, 0.0, 0.0, 100.0);
        pid.set_input("SP", json!(100.0));
        pid.set_input("PV", json!(0.0));

        pid.execute();

        // P-only: error = 100, output = 10 * 100 = 1000, clamped to 100
        assert_eq!(pid.get_output("OUT"), Some(json!(100.0)));
        assert_eq!(pid.get_output("SATURATED"), Some(json!(true)));
    }

    #[test]
    fn test_pid_manual_mode() {
        let mut pid = PID::new(1.0, 0.0, 0.0);
        pid.set_input("MANUAL", json!(true));
        pid.set_input("MAN_OUT", json!(75.0));
        pid.set_input("OUT_MAX", json!(100.0));

        pid.execute();

        assert_eq!(pid.get_output("OUT"), Some(json!(75.0)));
    }

    #[test]
    fn test_pid_reset() {
        let mut pid = PID::new(1.0, 1.0, 0.0);

        // Run a few cycles to build up integral
        for _ in 0..5 {
            pid.set_input("SP", json!(100.0));
            pid.set_input("PV", json!(50.0));
            pid.execute();
        }

        // Reset
        pid.set_input("RESET", json!(true));
        pid.execute();

        // Integral should be zero
        assert_eq!(pid.get_output("I_TERM"), Some(json!(0.0)));
    }

    #[test]
    fn test_mavg_basic() {
        let mut mavg = MAVG::new(5);

        // Add 5 values: 10, 20, 30, 40, 50
        for v in [10.0, 20.0, 30.0, 40.0, 50.0] {
            mavg.set_input("IN", json!(v));
            mavg.execute();
        }

        // Average should be (10+20+30+40+50)/5 = 30
        assert_eq!(mavg.get_output("OUT"), Some(json!(30.0)));
        assert_eq!(mavg.get_output("VALID"), Some(json!(true)));
    }

    #[test]
    fn test_mavg_sliding_window() {
        let mut mavg = MAVG::new(3);

        // Fill window
        for v in [10.0, 20.0, 30.0] {
            mavg.set_input("IN", json!(v));
            mavg.execute();
        }
        assert_eq!(mavg.get_output("OUT"), Some(json!(20.0))); // (10+20+30)/3

        // Add new value - oldest (10) should be removed
        mavg.set_input("IN", json!(40.0));
        mavg.execute();
        assert_eq!(mavg.get_output("OUT"), Some(json!(30.0))); // (20+30+40)/3
    }

    #[test]
    fn test_mavg_reset() {
        let mut mavg = MAVG::new(5);

        for v in [10.0, 20.0, 30.0] {
            mavg.set_input("IN", json!(v));
            mavg.execute();
        }

        mavg.set_input("RESET", json!(true));
        mavg.execute();

        assert_eq!(mavg.get_output("OUT"), Some(json!(0.0)));
        assert_eq!(mavg.get_output("VALID"), Some(json!(false)));
        assert_eq!(mavg.get_output("COUNT"), Some(json!(0)));
    }

    #[test]
    fn test_hysteresis_basic() {
        let mut hyst = HYSTERESIS::new(60.0, 40.0);

        // Start below low - output should be off
        hyst.set_input("IN", json!(30.0));
        hyst.execute();
        assert_eq!(hyst.get_output("OUT"), Some(json!(false)));

        // Go above high - output should turn on
        hyst.set_input("IN", json!(65.0));
        hyst.execute();
        assert_eq!(hyst.get_output("OUT"), Some(json!(true)));

        // Stay between thresholds - output should stay on
        hyst.set_input("IN", json!(50.0));
        hyst.execute();
        assert_eq!(hyst.get_output("OUT"), Some(json!(true)));

        // Go below low - output should turn off
        hyst.set_input("IN", json!(35.0));
        hyst.execute();
        assert_eq!(hyst.get_output("OUT"), Some(json!(false)));
    }

    #[test]
    fn test_pid_function_block_trait() {
        let pid = PID::new(1.0, 0.5, 0.1);

        assert_eq!(pid.fb_type(), "PID");
        assert!(pid.input_names().contains(&"SP"));
        assert!(pid.input_names().contains(&"PV"));
        assert!(pid.output_names().contains(&"OUT"));
        assert!(pid.output_names().contains(&"ERROR"));
    }

    #[test]
    fn test_mavg_function_block_trait() {
        let mavg = MAVG::new(10);

        assert_eq!(mavg.fb_type(), "MAVG");
        assert!(mavg.input_names().contains(&"IN"));
        assert!(mavg.output_names().contains(&"OUT"));
        assert!(mavg.output_names().contains(&"VALID"));
    }

    #[test]
    fn test_hysteresis_function_block_trait() {
        let hyst = HYSTERESIS::new(60.0, 40.0);

        assert_eq!(hyst.fb_type(), "HYSTERESIS");
        assert!(hyst.input_names().contains(&"IN"));
        assert!(hyst.input_names().contains(&"HIGH"));
        assert!(hyst.input_names().contains(&"LOW"));
        assert!(hyst.output_names().contains(&"OUT"));
    }
}
