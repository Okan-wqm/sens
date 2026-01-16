//! Script Conflict Detection (v2.0)
//!
//! Detects when multiple scripts attempt to write different values
//! to the same GPIO pin or Modbus register in the same execution cycle.
//!
//! v2.0: Priority-aware conflict resolution
//! - Higher priority scripts win conflicts
//! - Lower priority scripts are blocked from overwriting

use std::collections::HashMap;
use tracing::{debug, warn};

/// Pending write operation for conflict detection
#[derive(Debug, Clone)]
pub struct PendingWrite {
    pub script_id: String,
    pub priority: u8,
    pub value: WriteValue,
}

/// Value being written
#[derive(Debug, Clone, PartialEq)]
pub enum WriteValue {
    Bool(bool),
    U16(u16),
}

/// Conflict detection result
#[derive(Debug)]
pub enum ConflictResult {
    /// No conflict, proceed with write
    NoConflict,
    /// Conflict detected but current script has higher priority, proceed
    ConflictWon { message: String },
    /// Conflict detected and current script has lower priority, BLOCKED
    ConflictLost { message: String },
    /// Same value, can be deduplicated
    Duplicate,
}

/// Conflict detector for GPIO and Modbus writes
///
/// Tracks pending writes within an execution cycle to detect
/// when multiple scripts try to write different values to the same target.
/// Uses priority to determine which script wins conflicts.
pub struct ConflictDetector {
    /// Pending GPIO writes: pin -> PendingWrite
    gpio_writes: HashMap<u8, PendingWrite>,
    /// Pending Modbus register writes: (device, address) -> PendingWrite
    modbus_writes: HashMap<(String, u16), PendingWrite>,
    /// Pending Modbus coil writes: (device, address) -> PendingWrite
    coil_writes: HashMap<(String, u16), PendingWrite>,
}

impl ConflictDetector {
    /// Create a new conflict detector
    pub fn new() -> Self {
        Self {
            gpio_writes: HashMap::new(),
            modbus_writes: HashMap::new(),
            coil_writes: HashMap::new(),
        }
    }

    /// Reset detector for a new execution cycle
    pub fn reset(&mut self) {
        self.gpio_writes.clear();
        self.modbus_writes.clear();
        self.coil_writes.clear();
    }

    /// Check and register a GPIO write with priority
    ///
    /// Returns ConflictResult indicating if there's a conflict and who wins
    pub fn check_gpio_write(
        &mut self,
        script_id: &str,
        priority: u8,
        pin: u8,
        value: bool,
    ) -> ConflictResult {
        let new_write = PendingWrite {
            script_id: script_id.to_string(),
            priority,
            value: WriteValue::Bool(value),
        };

        if let Some(existing) = self.gpio_writes.get(&pin) {
            // Same script updating same pin is OK
            if existing.script_id == script_id {
                self.gpio_writes.insert(pin, new_write);
                return ConflictResult::NoConflict;
            }

            // Different script - check value
            if existing.value == WriteValue::Bool(value) {
                // Same value from different scripts - duplicate
                return ConflictResult::Duplicate;
            }

            // Different value from different scripts - CONFLICT!
            let conflict_value = match &existing.value {
                WriteValue::Bool(v) => {
                    if *v {
                        "HIGH"
                    } else {
                        "LOW"
                    }
                }
                _ => "unknown",
            };
            let new_value = if value { "HIGH" } else { "LOW" };

            // Priority-based resolution
            if priority > existing.priority {
                // Current script has HIGHER priority - it wins
                let message = format!(
                    "GPIO CONFLICT WON: Pin {} - Script '{}' (priority {}) overrides '{}' (priority {}): {} -> {}",
                    pin, script_id, priority, existing.script_id, existing.priority, conflict_value, new_value
                );
                warn!("{}", message);
                self.gpio_writes.insert(pin, new_write);
                return ConflictResult::ConflictWon { message };
            } else if priority < existing.priority {
                // Current script has LOWER priority - it loses, BLOCKED
                let message = format!(
                    "GPIO CONFLICT LOST: Pin {} - Script '{}' (priority {}) blocked by '{}' (priority {}): keeping {}",
                    pin, script_id, priority, existing.script_id, existing.priority, conflict_value
                );
                warn!("{}", message);
                // Don't update the write - existing higher priority wins
                return ConflictResult::ConflictLost { message };
            } else {
                // Same priority - last write wins (original behavior)
                let message = format!(
                    "GPIO CONFLICT: Pin {} - Script '{}' wants {}, but '{}' already set {} (same priority {})",
                    pin, script_id, new_value, existing.script_id, conflict_value, priority
                );
                warn!("{}", message);
                self.gpio_writes.insert(pin, new_write);
                return ConflictResult::ConflictWon { message };
            }
        }

        // No existing write, register it
        self.gpio_writes.insert(pin, new_write);
        ConflictResult::NoConflict
    }

    /// Check and register a Modbus register write with priority
    pub fn check_modbus_write(
        &mut self,
        script_id: &str,
        priority: u8,
        device: &str,
        address: u16,
        value: u16,
    ) -> ConflictResult {
        let key = (device.to_string(), address);
        let new_write = PendingWrite {
            script_id: script_id.to_string(),
            priority,
            value: WriteValue::U16(value),
        };

        if let Some(existing) = self.modbus_writes.get(&key) {
            if existing.script_id == script_id {
                self.modbus_writes.insert(key, new_write);
                return ConflictResult::NoConflict;
            }

            if existing.value == WriteValue::U16(value) {
                return ConflictResult::Duplicate;
            }

            let existing_val = match &existing.value {
                WriteValue::U16(v) => *v,
                _ => 0,
            };

            // Priority-based resolution
            if priority > existing.priority {
                let message = format!(
                    "MODBUS CONFLICT WON: {}:{} - Script '{}' (priority {}) overrides '{}' (priority {}): {} -> {}",
                    device, address, script_id, priority, existing.script_id, existing.priority, existing_val, value
                );
                warn!("{}", message);
                self.modbus_writes.insert(key, new_write);
                return ConflictResult::ConflictWon { message };
            } else if priority < existing.priority {
                let message = format!(
                    "MODBUS CONFLICT LOST: {}:{} - Script '{}' (priority {}) blocked by '{}' (priority {}): keeping {}",
                    device, address, script_id, priority, existing.script_id, existing.priority, existing_val
                );
                warn!("{}", message);
                return ConflictResult::ConflictLost { message };
            } else {
                let message = format!(
                    "MODBUS CONFLICT: {}:{} - Script '{}' wants {}, but '{}' already set {} (same priority {})",
                    device, address, script_id, value, existing.script_id, existing_val, priority
                );
                warn!("{}", message);
                self.modbus_writes.insert(key, new_write);
                return ConflictResult::ConflictWon { message };
            }
        }

        self.modbus_writes.insert(key, new_write);
        ConflictResult::NoConflict
    }

    /// Check and register a Modbus coil write with priority
    pub fn check_coil_write(
        &mut self,
        script_id: &str,
        priority: u8,
        device: &str,
        address: u16,
        value: bool,
    ) -> ConflictResult {
        let key = (device.to_string(), address);
        let new_write = PendingWrite {
            script_id: script_id.to_string(),
            priority,
            value: WriteValue::Bool(value),
        };

        if let Some(existing) = self.coil_writes.get(&key) {
            if existing.script_id == script_id {
                self.coil_writes.insert(key, new_write);
                return ConflictResult::NoConflict;
            }

            if existing.value == WriteValue::Bool(value) {
                return ConflictResult::Duplicate;
            }

            let existing_val = match &existing.value {
                WriteValue::Bool(v) => *v,
                _ => false,
            };

            // Priority-based resolution
            if priority > existing.priority {
                let message = format!(
                    "COIL CONFLICT WON: {}:{} - Script '{}' (priority {}) overrides '{}' (priority {}): {} -> {}",
                    device, address, script_id, priority, existing.script_id, existing.priority, existing_val, value
                );
                warn!("{}", message);
                self.coil_writes.insert(key, new_write);
                return ConflictResult::ConflictWon { message };
            } else if priority < existing.priority {
                let message = format!(
                    "COIL CONFLICT LOST: {}:{} - Script '{}' (priority {}) blocked by '{}' (priority {}): keeping {}",
                    device, address, script_id, priority, existing.script_id, existing.priority, existing_val
                );
                warn!("{}", message);
                return ConflictResult::ConflictLost { message };
            } else {
                let message = format!(
                    "COIL CONFLICT: {}:{} - Script '{}' wants {}, but '{}' already set {} (same priority {})",
                    device, address, script_id, value, existing.script_id, existing_val, priority
                );
                warn!("{}", message);
                self.coil_writes.insert(key, new_write);
                return ConflictResult::ConflictWon { message };
            }
        }

        self.coil_writes.insert(key, new_write);
        ConflictResult::NoConflict
    }

    /// Get summary of all pending writes (for debugging)
    pub fn get_pending_summary(&self) -> String {
        let gpio_count = self.gpio_writes.len();
        let modbus_count = self.modbus_writes.len();
        let coil_count = self.coil_writes.len();

        format!(
            "Pending writes: {} GPIO, {} Modbus registers, {} coils",
            gpio_count, modbus_count, coil_count
        )
    }
}

impl Default for ConflictDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_conflict_single_script() {
        let mut detector = ConflictDetector::new();

        let result = detector.check_gpio_write("script1", 50, 17, true);
        assert!(matches!(result, ConflictResult::NoConflict));

        // Same script can update same pin
        let result = detector.check_gpio_write("script1", 50, 17, false);
        assert!(matches!(result, ConflictResult::NoConflict));
    }

    #[test]
    fn test_high_priority_wins() {
        let mut detector = ConflictDetector::new();

        // Low priority script sets pin 17 to HIGH
        let result = detector.check_gpio_write("low_priority", 10, 17, true);
        assert!(matches!(result, ConflictResult::NoConflict));

        // High priority script tries to set pin 17 to LOW - should WIN
        let result = detector.check_gpio_write("high_priority", 100, 17, false);
        assert!(matches!(result, ConflictResult::ConflictWon { .. }));
    }

    #[test]
    fn test_low_priority_loses() {
        let mut detector = ConflictDetector::new();

        // High priority script sets pin 17 to HIGH
        let result = detector.check_gpio_write("high_priority", 100, 17, true);
        assert!(matches!(result, ConflictResult::NoConflict));

        // Low priority script tries to set pin 17 to LOW - should be BLOCKED
        let result = detector.check_gpio_write("low_priority", 10, 17, false);
        assert!(matches!(result, ConflictResult::ConflictLost { .. }));
    }

    #[test]
    fn test_same_priority_last_wins() {
        let mut detector = ConflictDetector::new();

        // First script sets pin 17 to HIGH
        let result = detector.check_gpio_write("script1", 50, 17, true);
        assert!(matches!(result, ConflictResult::NoConflict));

        // Second script with same priority sets pin 17 to LOW - last wins
        let result = detector.check_gpio_write("script2", 50, 17, false);
        assert!(matches!(result, ConflictResult::ConflictWon { .. }));
    }

    #[test]
    fn test_duplicate_same_value() {
        let mut detector = ConflictDetector::new();

        // Script1 sets pin 17 to HIGH
        detector.check_gpio_write("script1", 50, 17, true);

        // Script2 also wants to set pin 17 to HIGH - duplicate, not conflict
        let result = detector.check_gpio_write("script2", 100, 17, true);
        assert!(matches!(result, ConflictResult::Duplicate));
    }

    #[test]
    fn test_modbus_priority_conflict() {
        let mut detector = ConflictDetector::new();

        // Emergency script sets register
        detector.check_modbus_write("emergency", 255, "PLC-1", 100, 0);

        // Normal script tries to write different value - should be BLOCKED
        let result = detector.check_modbus_write("normal", 50, "PLC-1", 100, 1234);
        assert!(matches!(result, ConflictResult::ConflictLost { .. }));
    }

    #[test]
    fn test_reset_clears_state() {
        let mut detector = ConflictDetector::new();

        detector.check_gpio_write("script1", 100, 17, true);
        detector.reset();

        // After reset, no conflict should exist
        let result = detector.check_gpio_write("script2", 10, 17, false);
        assert!(matches!(result, ConflictResult::NoConflict));
    }
}
