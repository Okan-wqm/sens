//! IEC 61131-3 Timer Function Blocks
//!
//! Standard timer implementations:
//! - TON: On-Delay Timer (output turns ON after delay when input is ON)
//! - TOF: Off-Delay Timer (output stays ON for delay after input turns OFF)
//! - TP: Pulse Timer (output stays ON for fixed duration after trigger)
//!
//! ## Timer Modes (v1.2.0)
//! Timers support two timing modes:
//! - **WallClock**: Uses actual elapsed time via `Instant::now()` (default)
//! - **ScanCycle**: Counts scan cycles for deterministic PLC-style behavior
//!
//! Scan-cycle mode is useful for:
//! - Deterministic behavior independent of actual execution time
//! - Testing with predictable timing
//! - PLC compatibility where cycle time is known and consistent

use super::FunctionBlock;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

// ============================================================================
// Timer Mode (v1.2.0)
// ============================================================================

/// Timer timing mode (v1.2.0)
///
/// Determines how elapsed time is calculated:
/// - WallClock: Real elapsed time using system clock
/// - ScanCycle: Count scan cycles and convert using cycle time
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TimerMode {
    /// Use actual elapsed time (default, original behavior)
    #[default]
    WallClock,
    /// Count scan cycles (deterministic, PLC-style)
    ScanCycle,
}

/// Default scan cycle time in milliseconds (10ms = 100Hz)
const DEFAULT_CYCLE_TIME_MS: u64 = 10;

// ============================================================================
// TON - On-Delay Timer
// ============================================================================

/// TON (Timer On-Delay) - IEC 61131-3
///
/// When IN becomes TRUE, timer starts counting.
/// After PT (preset time) elapses, Q becomes TRUE.
/// When IN becomes FALSE, timer resets and Q becomes FALSE.
///
/// ## Timer Modes (v1.2.0)
/// - WallClock: Uses `Instant::now()` for real elapsed time
/// - ScanCycle: Counts execute() calls and multiplies by cycle time
///
/// Timing Diagram:
/// ```text
/// IN:  _____|‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾|_____
/// Q:   ___________|‾‾‾‾‾‾‾‾‾‾‾|_____
///              PT→|
/// ET:  ___/‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾\_____
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TON {
    /// Preset time in milliseconds
    pt_ms: u64,
    /// Current elapsed time in milliseconds
    et_ms: u64,
    /// Input signal
    input: bool,
    /// Output signal
    output: bool,
    /// Timer start instant (not serialized, WallClock mode)
    #[serde(skip)]
    start_instant: Option<Instant>,
    /// Previous input state for edge detection
    prev_input: bool,
    /// Timer mode (v1.2.0)
    #[serde(default)]
    mode: TimerMode,
    /// Scan cycle count (v1.2.0, ScanCycle mode)
    #[serde(default)]
    scan_count: u64,
    /// Cycle time in milliseconds (v1.2.0, ScanCycle mode)
    #[serde(default = "default_cycle_time")]
    cycle_time_ms: u64,
}

/// Default cycle time for serde
fn default_cycle_time() -> u64 {
    DEFAULT_CYCLE_TIME_MS
}

impl TON {
    /// Create new TON timer with preset time (WallClock mode)
    pub fn new(preset_ms: u64) -> Self {
        Self {
            pt_ms: preset_ms,
            et_ms: 0,
            input: false,
            output: false,
            start_instant: None,
            prev_input: false,
            mode: TimerMode::WallClock,
            scan_count: 0,
            cycle_time_ms: DEFAULT_CYCLE_TIME_MS,
        }
    }

    /// Create new TON timer with scan-cycle mode (v1.2.0)
    ///
    /// # Arguments
    /// * `preset_ms` - Preset time in milliseconds
    /// * `cycle_time_ms` - Expected scan cycle time in milliseconds
    pub fn with_scan_cycle(preset_ms: u64, cycle_time_ms: u64) -> Self {
        Self {
            pt_ms: preset_ms,
            et_ms: 0,
            input: false,
            output: false,
            start_instant: None,
            prev_input: false,
            mode: TimerMode::ScanCycle,
            scan_count: 0,
            cycle_time_ms: cycle_time_ms.max(1), // Minimum 1ms
        }
    }

    /// Get preset time in milliseconds
    pub fn preset(&self) -> u64 {
        self.pt_ms
    }

    /// Get elapsed time in milliseconds
    pub fn elapsed(&self) -> u64 {
        self.et_ms
    }

    /// Get output state
    pub fn q(&self) -> bool {
        self.output
    }

    /// Get timer mode (v1.2.0)
    pub fn mode(&self) -> TimerMode {
        self.mode
    }

    /// Set timer mode (v1.2.0)
    pub fn set_mode(&mut self, mode: TimerMode) {
        self.mode = mode;
    }

    /// Set cycle time for scan-cycle mode (v1.2.0)
    pub fn set_cycle_time(&mut self, ms: u64) {
        self.cycle_time_ms = ms.max(1);
    }
}

impl FunctionBlock for TON {
    fn fb_type(&self) -> &'static str {
        "TON"
    }

    fn execute(&mut self) {
        if self.input {
            // Input is ON
            if !self.prev_input {
                // Rising edge - start timer
                match self.mode {
                    TimerMode::WallClock => {
                        self.start_instant = Some(Instant::now());
                    }
                    TimerMode::ScanCycle => {
                        self.scan_count = 0;
                    }
                }
                self.et_ms = 0;
            }

            // Update elapsed time based on mode
            match self.mode {
                TimerMode::WallClock => {
                    if let Some(start) = self.start_instant {
                        self.et_ms = start.elapsed().as_millis() as u64;
                    }
                }
                TimerMode::ScanCycle => {
                    self.scan_count += 1;
                    self.et_ms = self.scan_count * self.cycle_time_ms;
                }
            }

            // Check if preset time reached
            if self.et_ms >= self.pt_ms {
                self.output = true;
                self.et_ms = self.pt_ms; // Cap at preset
            }
        } else {
            // Input is OFF - reset timer
            self.start_instant = None;
            self.scan_count = 0;
            self.et_ms = 0;
            self.output = false;
        }

        self.prev_input = self.input;
    }

    fn get_output(&self, name: &str) -> Option<Value> {
        match name {
            "Q" | "q" | "output" => Some(Value::Bool(self.output)),
            "ET" | "et" | "elapsed" => Some(Value::Number(self.et_ms.into())),
            _ => None,
        }
    }

    fn set_input(&mut self, name: &str, value: Value) -> bool {
        match name {
            "IN" | "in" | "input" => {
                if let Some(v) = value.as_bool() {
                    self.input = v;
                    return true;
                }
            }
            "PT" | "pt" | "preset" => {
                if let Some(v) = value.as_u64() {
                    self.pt_ms = v;
                    return true;
                }
            }
            // v1.2.0: Mode and cycle time inputs
            "MODE" | "mode" => {
                if let Some(s) = value.as_str() {
                    match s {
                        "wall_clock" | "wallclock" => {
                            self.mode = TimerMode::WallClock;
                            return true;
                        }
                        "scan_cycle" | "scancycle" => {
                            self.mode = TimerMode::ScanCycle;
                            return true;
                        }
                        _ => {}
                    }
                }
            }
            "CYCLE_TIME" | "cycle_time" => {
                if let Some(v) = value.as_u64() {
                    self.cycle_time_ms = v.max(1);
                    return true;
                }
            }
            _ => {}
        }
        false
    }

    fn serialize_state(&self) -> Value {
        json!({
            "pt_ms": self.pt_ms,
            "et_ms": self.et_ms,
            "input": self.input,
            "output": self.output,
            "prev_input": self.prev_input,
            "mode": self.mode,
            "scan_count": self.scan_count,
            "cycle_time_ms": self.cycle_time_ms
        })
    }

    fn deserialize_state(&mut self, state: &Value) -> bool {
        if let Some(obj) = state.as_object() {
            if let Some(pt) = obj.get("pt_ms").and_then(|v| v.as_u64()) {
                self.pt_ms = pt;
            }
            if let Some(et) = obj.get("et_ms").and_then(|v| v.as_u64()) {
                self.et_ms = et;
            }
            if let Some(inp) = obj.get("input").and_then(|v| v.as_bool()) {
                self.input = inp;
            }
            if let Some(out) = obj.get("output").and_then(|v| v.as_bool()) {
                self.output = out;
            }
            if let Some(prev) = obj.get("prev_input").and_then(|v| v.as_bool()) {
                self.prev_input = prev;
            }
            // v1.2.0: Restore mode and scan-cycle state
            if let Some(mode_str) = obj.get("mode").and_then(|v| v.as_str()) {
                self.mode = match mode_str {
                    "scan_cycle" => TimerMode::ScanCycle,
                    _ => TimerMode::WallClock,
                };
            }
            if let Some(sc) = obj.get("scan_count").and_then(|v| v.as_u64()) {
                self.scan_count = sc;
            }
            if let Some(ct) = obj.get("cycle_time_ms").and_then(|v| v.as_u64()) {
                self.cycle_time_ms = ct.max(1);
            }
            // Restore timer if was running (WallClock mode only)
            if self.mode == TimerMode::WallClock
                && self.input
                && self.et_ms > 0
                && self.et_ms < self.pt_ms
            {
                // Approximate: assume just restored, set start to now - elapsed
                self.start_instant = Some(Instant::now() - Duration::from_millis(self.et_ms));
            }
            return true;
        }
        false
    }

    fn reset(&mut self) {
        self.et_ms = 0;
        self.output = false;
        self.start_instant = None;
        self.scan_count = 0;
        self.input = false;
        self.prev_input = false;
    }

    fn input_names(&self) -> Vec<&'static str> {
        vec!["IN", "PT", "MODE", "CYCLE_TIME"]
    }

    fn output_names(&self) -> Vec<&'static str> {
        vec!["Q", "ET"]
    }
}

// ============================================================================
// TOF - Off-Delay Timer
// ============================================================================

/// TOF (Timer Off-Delay) - IEC 61131-3
///
/// When IN becomes TRUE, Q immediately becomes TRUE.
/// When IN becomes FALSE, timer starts counting.
/// After PT elapses, Q becomes FALSE.
///
/// Timing Diagram:
/// ```text
/// IN:  _____|‾‾‾‾‾‾‾‾‾|_______________
/// Q:   _____|‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾‾|_____
///                      PT→|
/// ET:  _______________/‾‾‾‾‾‾‾\_____
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TOF {
    /// Preset time in milliseconds
    pt_ms: u64,
    /// Current elapsed time in milliseconds
    et_ms: u64,
    /// Input signal
    input: bool,
    /// Output signal
    output: bool,
    /// Timer start instant (not serialized)
    #[serde(skip)]
    start_instant: Option<Instant>,
    /// Previous input state for edge detection
    prev_input: bool,
}

impl TOF {
    /// Create new TOF timer with preset time
    pub fn new(preset_ms: u64) -> Self {
        Self {
            pt_ms: preset_ms,
            et_ms: 0,
            input: false,
            output: false,
            start_instant: None,
            prev_input: false,
        }
    }

    /// Get preset time in milliseconds
    pub fn preset(&self) -> u64 {
        self.pt_ms
    }

    /// Get elapsed time in milliseconds
    pub fn elapsed(&self) -> u64 {
        self.et_ms
    }

    /// Get output state
    pub fn q(&self) -> bool {
        self.output
    }
}

impl FunctionBlock for TOF {
    fn fb_type(&self) -> &'static str {
        "TOF"
    }

    fn execute(&mut self) {
        if self.input {
            // Input is ON - output ON, reset timer
            self.output = true;
            self.start_instant = None;
            self.et_ms = 0;
        } else {
            // Input is OFF
            if self.prev_input {
                // Falling edge - start timer
                self.start_instant = Some(Instant::now());
                self.et_ms = 0;
            }

            // Update elapsed time
            if let Some(start) = self.start_instant {
                self.et_ms = start.elapsed().as_millis() as u64;

                // Check if preset time reached
                if self.et_ms >= self.pt_ms {
                    self.output = false;
                    self.et_ms = self.pt_ms;
                    self.start_instant = None;
                }
            } else if !self.output {
                // Timer finished or never started
                self.et_ms = 0;
            }
        }

        self.prev_input = self.input;
    }

    fn get_output(&self, name: &str) -> Option<Value> {
        match name {
            "Q" | "q" | "output" => Some(Value::Bool(self.output)),
            "ET" | "et" | "elapsed" => Some(Value::Number(self.et_ms.into())),
            _ => None,
        }
    }

    fn set_input(&mut self, name: &str, value: Value) -> bool {
        match name {
            "IN" | "in" | "input" => {
                if let Some(v) = value.as_bool() {
                    self.input = v;
                    return true;
                }
            }
            "PT" | "pt" | "preset" => {
                if let Some(v) = value.as_u64() {
                    self.pt_ms = v;
                    return true;
                }
            }
            _ => {}
        }
        false
    }

    fn serialize_state(&self) -> Value {
        json!({
            "pt_ms": self.pt_ms,
            "et_ms": self.et_ms,
            "input": self.input,
            "output": self.output,
            "prev_input": self.prev_input
        })
    }

    fn deserialize_state(&mut self, state: &Value) -> bool {
        if let Some(obj) = state.as_object() {
            if let Some(pt) = obj.get("pt_ms").and_then(|v| v.as_u64()) {
                self.pt_ms = pt;
            }
            if let Some(et) = obj.get("et_ms").and_then(|v| v.as_u64()) {
                self.et_ms = et;
            }
            if let Some(inp) = obj.get("input").and_then(|v| v.as_bool()) {
                self.input = inp;
            }
            if let Some(out) = obj.get("output").and_then(|v| v.as_bool()) {
                self.output = out;
            }
            if let Some(prev) = obj.get("prev_input").and_then(|v| v.as_bool()) {
                self.prev_input = prev;
            }
            // Restore timer if was running
            if !self.input && self.output && self.et_ms > 0 && self.et_ms < self.pt_ms {
                self.start_instant = Some(Instant::now() - Duration::from_millis(self.et_ms));
            }
            return true;
        }
        false
    }

    fn reset(&mut self) {
        self.et_ms = 0;
        self.output = false;
        self.start_instant = None;
        self.input = false;
        self.prev_input = false;
    }

    fn input_names(&self) -> Vec<&'static str> {
        vec!["IN", "PT"]
    }

    fn output_names(&self) -> Vec<&'static str> {
        vec!["Q", "ET"]
    }
}

// ============================================================================
// TP - Pulse Timer
// ============================================================================

/// TP (Timer Pulse) - IEC 61131-3
///
/// On rising edge of IN, Q becomes TRUE for exactly PT duration.
/// Additional IN signals during pulse are ignored.
///
/// Timing Diagram:
/// ```text
/// IN:  __|‾|_____|‾‾‾‾|___|‾|______
/// Q:   __|‾‾‾‾‾‾‾|____|‾‾‾‾‾‾‾|____
///         |←PT→|      |←PT→|
/// ET:  __/‾‾‾‾‾‾\____/‾‾‾‾‾‾‾\____
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TP {
    /// Preset time (pulse duration) in milliseconds
    pt_ms: u64,
    /// Current elapsed time in milliseconds
    et_ms: u64,
    /// Input signal
    input: bool,
    /// Output signal
    output: bool,
    /// Timer start instant (not serialized)
    #[serde(skip)]
    start_instant: Option<Instant>,
    /// Previous input state for edge detection
    prev_input: bool,
    /// Whether pulse is active
    pulse_active: bool,
}

impl TP {
    /// Create new TP timer with preset time (pulse duration)
    pub fn new(preset_ms: u64) -> Self {
        Self {
            pt_ms: preset_ms,
            et_ms: 0,
            input: false,
            output: false,
            start_instant: None,
            prev_input: false,
            pulse_active: false,
        }
    }

    /// Get preset time in milliseconds
    pub fn preset(&self) -> u64 {
        self.pt_ms
    }

    /// Get elapsed time in milliseconds
    pub fn elapsed(&self) -> u64 {
        self.et_ms
    }

    /// Get output state
    pub fn q(&self) -> bool {
        self.output
    }
}

impl FunctionBlock for TP {
    fn fb_type(&self) -> &'static str {
        "TP"
    }

    fn execute(&mut self) {
        // Check for rising edge and not already in pulse
        if self.input && !self.prev_input && !self.pulse_active {
            // Rising edge - start pulse
            self.start_instant = Some(Instant::now());
            self.pulse_active = true;
            self.output = true;
            self.et_ms = 0;
        }

        // Update elapsed time if pulse is active
        if self.pulse_active {
            if let Some(start) = self.start_instant {
                self.et_ms = start.elapsed().as_millis() as u64;

                // Check if pulse duration reached
                if self.et_ms >= self.pt_ms {
                    self.output = false;
                    self.et_ms = self.pt_ms;
                    self.pulse_active = false;
                    self.start_instant = None;
                }
            }
        } else {
            self.et_ms = 0;
        }

        self.prev_input = self.input;
    }

    fn get_output(&self, name: &str) -> Option<Value> {
        match name {
            "Q" | "q" | "output" => Some(Value::Bool(self.output)),
            "ET" | "et" | "elapsed" => Some(Value::Number(self.et_ms.into())),
            _ => None,
        }
    }

    fn set_input(&mut self, name: &str, value: Value) -> bool {
        match name {
            "IN" | "in" | "input" => {
                if let Some(v) = value.as_bool() {
                    self.input = v;
                    return true;
                }
            }
            "PT" | "pt" | "preset" => {
                if let Some(v) = value.as_u64() {
                    self.pt_ms = v;
                    return true;
                }
            }
            _ => {}
        }
        false
    }

    fn serialize_state(&self) -> Value {
        json!({
            "pt_ms": self.pt_ms,
            "et_ms": self.et_ms,
            "input": self.input,
            "output": self.output,
            "prev_input": self.prev_input,
            "pulse_active": self.pulse_active
        })
    }

    fn deserialize_state(&mut self, state: &Value) -> bool {
        if let Some(obj) = state.as_object() {
            if let Some(pt) = obj.get("pt_ms").and_then(|v| v.as_u64()) {
                self.pt_ms = pt;
            }
            if let Some(et) = obj.get("et_ms").and_then(|v| v.as_u64()) {
                self.et_ms = et;
            }
            if let Some(inp) = obj.get("input").and_then(|v| v.as_bool()) {
                self.input = inp;
            }
            if let Some(out) = obj.get("output").and_then(|v| v.as_bool()) {
                self.output = out;
            }
            if let Some(prev) = obj.get("prev_input").and_then(|v| v.as_bool()) {
                self.prev_input = prev;
            }
            if let Some(pulse) = obj.get("pulse_active").and_then(|v| v.as_bool()) {
                self.pulse_active = pulse;
            }
            // Restore timer if pulse was active
            if self.pulse_active && self.et_ms > 0 && self.et_ms < self.pt_ms {
                self.start_instant = Some(Instant::now() - Duration::from_millis(self.et_ms));
            }
            return true;
        }
        false
    }

    fn reset(&mut self) {
        self.et_ms = 0;
        self.output = false;
        self.start_instant = None;
        self.input = false;
        self.prev_input = false;
        self.pulse_active = false;
    }

    fn input_names(&self) -> Vec<&'static str> {
        vec!["IN", "PT"]
    }

    fn output_names(&self) -> Vec<&'static str> {
        vec!["Q", "ET"]
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;
    use std::time::Duration;

    #[test]
    fn test_ton_basic() {
        let mut ton = TON::new(100); // 100ms delay

        // Initially off
        assert!(!ton.q());
        assert_eq!(ton.elapsed(), 0);

        // Turn on input
        ton.set_input("IN", Value::Bool(true));
        ton.execute();

        // Output should still be off (not enough time)
        assert!(!ton.q());

        // Wait and execute
        sleep(Duration::from_millis(150));
        ton.execute();

        // Now output should be on
        assert!(ton.q());
        assert!(ton.elapsed() >= 100);
    }

    #[test]
    fn test_ton_reset_on_input_off() {
        let mut ton = TON::new(100);

        // Start timer
        ton.set_input("IN", Value::Bool(true));
        ton.execute();
        sleep(Duration::from_millis(50));
        ton.execute();

        // Turn off before preset reached
        ton.set_input("IN", Value::Bool(false));
        ton.execute();

        // Timer should reset
        assert!(!ton.q());
        assert_eq!(ton.elapsed(), 0);
    }

    #[test]
    fn test_tof_basic() {
        let mut tof = TOF::new(100); // 100ms delay

        // Turn on input - output should immediately turn on
        tof.set_input("IN", Value::Bool(true));
        tof.execute();
        assert!(tof.q());

        // Turn off input - output should stay on
        tof.set_input("IN", Value::Bool(false));
        tof.execute();
        assert!(tof.q());

        // Wait for delay
        sleep(Duration::from_millis(150));
        tof.execute();

        // Now output should be off
        assert!(!tof.q());
    }

    #[test]
    fn test_tp_pulse() {
        let mut tp = TP::new(100); // 100ms pulse

        // Initially off
        assert!(!tp.q());

        // Rising edge triggers pulse
        tp.set_input("IN", Value::Bool(true));
        tp.execute();
        assert!(tp.q());

        // Input going off doesn't stop pulse
        tp.set_input("IN", Value::Bool(false));
        tp.execute();
        assert!(tp.q()); // Still in pulse

        // Wait for pulse to end
        sleep(Duration::from_millis(150));
        tp.execute();
        assert!(!tp.q());
    }

    #[test]
    fn test_tp_retrigger_ignored() {
        let mut tp = TP::new(100);

        // Start pulse
        tp.set_input("IN", Value::Bool(true));
        tp.execute();
        assert!(tp.q());

        // Turn off
        tp.set_input("IN", Value::Bool(false));
        tp.execute();

        // Try to retrigger while pulse active
        tp.set_input("IN", Value::Bool(true));
        tp.execute();

        // Should still be in original pulse
        let et_before = tp.elapsed();
        sleep(Duration::from_millis(10));
        tp.execute();
        let et_after = tp.elapsed();

        // Time should have progressed (not reset)
        assert!(et_after > et_before);
    }

    #[test]
    fn test_ton_serialize_deserialize() {
        let mut ton1 = TON::new(1000);
        ton1.set_input("IN", Value::Bool(true));
        ton1.execute();
        sleep(Duration::from_millis(100));
        ton1.execute();

        // Serialize state
        let state = ton1.serialize_state();

        // Create new timer and restore state
        let mut ton2 = TON::new(0);
        ton2.deserialize_state(&state);

        assert_eq!(ton2.preset(), 1000);
        assert!(ton2.elapsed() > 0);
    }

    #[test]
    fn test_function_block_trait() {
        let mut ton: Box<dyn FunctionBlock> = Box::new(TON::new(100));

        assert_eq!(ton.fb_type(), "TON");
        assert_eq!(ton.input_names(), vec!["IN", "PT", "MODE", "CYCLE_TIME"]);
        assert_eq!(ton.output_names(), vec!["Q", "ET"]);

        ton.set_input("IN", Value::Bool(true));
        ton.execute();

        assert_eq!(ton.get_output("Q"), Some(Value::Bool(false)));
    }

    // ========================================================================
    // Scan-Cycle Mode Tests (v1.2.0)
    // ========================================================================

    #[test]
    fn test_ton_scan_cycle_basic() {
        // 100ms preset, 10ms cycle time = 10 cycles to trigger
        let mut ton = TON::with_scan_cycle(100, 10);

        assert_eq!(ton.mode(), TimerMode::ScanCycle);
        assert!(!ton.q());
        assert_eq!(ton.elapsed(), 0);

        // Turn on input
        ton.set_input("IN", Value::Bool(true));

        // Execute 9 cycles - should NOT trigger yet
        for _ in 0..9 {
            ton.execute();
        }
        assert!(!ton.q());
        assert_eq!(ton.elapsed(), 90); // 9 * 10ms

        // 10th cycle - should trigger
        ton.execute();
        assert!(ton.q());
        assert_eq!(ton.elapsed(), 100); // Capped at preset
    }

    #[test]
    fn test_ton_scan_cycle_deterministic() {
        // Test that scan-cycle mode is completely deterministic
        // (no dependency on actual elapsed time)
        let mut ton1 = TON::with_scan_cycle(50, 5);
        let mut ton2 = TON::with_scan_cycle(50, 5);

        ton1.set_input("IN", Value::Bool(true));
        ton2.set_input("IN", Value::Bool(true));

        // Execute both the same number of times
        for _ in 0..10 {
            ton1.execute();
            ton2.execute();
        }

        // Both should have identical state
        assert_eq!(ton1.elapsed(), ton2.elapsed());
        assert_eq!(ton1.q(), ton2.q());
    }

    #[test]
    fn test_ton_scan_cycle_reset_on_input_off() {
        let mut ton = TON::with_scan_cycle(100, 10);

        // Start timer
        ton.set_input("IN", Value::Bool(true));
        for _ in 0..5 {
            ton.execute();
        }
        assert_eq!(ton.elapsed(), 50);

        // Turn off - should reset
        ton.set_input("IN", Value::Bool(false));
        ton.execute();

        assert!(!ton.q());
        assert_eq!(ton.elapsed(), 0);
    }

    #[test]
    fn test_ton_mode_switch_via_input() {
        let mut ton = TON::new(100);
        assert_eq!(ton.mode(), TimerMode::WallClock);

        // Switch to scan cycle mode
        ton.set_input("MODE", Value::String("scan_cycle".to_string()));
        assert_eq!(ton.mode(), TimerMode::ScanCycle);

        // Switch back to wall clock
        ton.set_input("MODE", Value::String("wall_clock".to_string()));
        assert_eq!(ton.mode(), TimerMode::WallClock);

        // Also test alternative names
        ton.set_input("mode", Value::String("scancycle".to_string()));
        assert_eq!(ton.mode(), TimerMode::ScanCycle);

        ton.set_input("mode", Value::String("wallclock".to_string()));
        assert_eq!(ton.mode(), TimerMode::WallClock);
    }

    #[test]
    fn test_ton_cycle_time_setting() {
        let mut ton = TON::with_scan_cycle(100, 10);

        // Change cycle time via input
        ton.set_input("CYCLE_TIME", Value::Number(20.into()));

        ton.set_input("IN", Value::Bool(true));

        // 5 cycles * 20ms = 100ms
        for _ in 0..5 {
            ton.execute();
        }

        assert!(ton.q());
        assert_eq!(ton.elapsed(), 100);
    }

    #[test]
    fn test_ton_cycle_time_minimum() {
        let mut ton = TON::with_scan_cycle(100, 0); // 0 should become 1

        ton.set_input("IN", Value::Bool(true));
        ton.execute();

        // Should use minimum of 1ms per cycle
        assert_eq!(ton.elapsed(), 1);

        // Also test via set_input
        ton.set_input("CYCLE_TIME", Value::Number(0.into()));
        ton.reset();
        ton.set_input("IN", Value::Bool(true));
        ton.execute();
        assert_eq!(ton.elapsed(), 1);
    }

    #[test]
    fn test_ton_scan_cycle_serialize_deserialize() {
        let mut ton1 = TON::with_scan_cycle(200, 20);
        ton1.set_input("IN", Value::Bool(true));

        // Execute 5 cycles
        for _ in 0..5 {
            ton1.execute();
        }

        // Serialize
        let state = ton1.serialize_state();

        // Verify state contains scan-cycle fields
        let obj = state.as_object().unwrap();
        assert_eq!(obj.get("mode").and_then(|v| v.as_str()), Some("scan_cycle"));
        assert_eq!(obj.get("scan_count").and_then(|v| v.as_u64()), Some(5));
        assert_eq!(obj.get("cycle_time_ms").and_then(|v| v.as_u64()), Some(20));

        // Deserialize into new timer
        let mut ton2 = TON::new(0);
        ton2.deserialize_state(&state);

        assert_eq!(ton2.mode(), TimerMode::ScanCycle);
        assert_eq!(ton2.preset(), 200);
        assert_eq!(ton2.elapsed(), 100); // 5 * 20ms

        // Continue execution - should complete after 5 more cycles
        for _ in 0..5 {
            ton2.execute();
        }
        assert!(ton2.q());
    }

    #[test]
    fn test_ton_reset_clears_scan_count() {
        let mut ton = TON::with_scan_cycle(100, 10);

        ton.set_input("IN", Value::Bool(true));
        for _ in 0..5 {
            ton.execute();
        }

        ton.reset();

        assert_eq!(ton.elapsed(), 0);
        assert!(!ton.q());

        // Start again - should count from 0
        ton.set_input("IN", Value::Bool(true));
        ton.execute();
        assert_eq!(ton.elapsed(), 10); // First cycle
    }

    #[test]
    fn test_timer_mode_default() {
        // Default mode should be WallClock
        assert_eq!(TimerMode::default(), TimerMode::WallClock);

        // New timer should use WallClock
        let ton = TON::new(100);
        assert_eq!(ton.mode(), TimerMode::WallClock);
    }
}
