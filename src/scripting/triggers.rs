//! Script triggers
//!
//! Defines when scripts should be executed:
//! - Time-based (cron-like scheduling)
//! - Threshold-based (sensor value crosses threshold)
//! - Event-based (GPIO change, etc.)
//! - Interval-based (every N seconds)

use chrono::{Datelike, Timelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, warn};

use super::{ComparisonOperator, ScriptContext};

/// Trigger definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trigger {
    /// Trigger type
    #[serde(rename = "type")]
    pub trigger_type: TriggerType,

    /// Source (sensor name, gpio pin, etc.) - required for threshold/event
    #[serde(default)]
    pub source: String,

    /// Comparison operator (for threshold triggers)
    #[serde(default)]
    pub operator: Option<ComparisonOperator>,

    /// Value to compare against (for threshold triggers)
    #[serde(default)]
    pub value: Option<Value>,

    /// Cron expression (for schedule triggers)
    #[serde(default)]
    pub cron: Option<String>,

    /// Interval in seconds (for interval triggers)
    #[serde(default)]
    pub interval_secs: Option<u64>,

    /// Debounce time in milliseconds (prevents rapid re-triggering)
    #[serde(default)]
    pub debounce_ms: Option<u64>,
}

/// Trigger types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TriggerType {
    /// Time-based (cron expression)
    Schedule,
    /// Value crosses threshold
    Threshold,
    /// Value changes
    Change,
    /// Periodic interval
    Interval,
    /// GPIO pin state change
    GpioChange,
    /// Manual trigger only
    Manual,
    /// On startup
    Startup,
}

/// Trigger state for tracking
#[derive(Debug, Clone, Default)]
pub struct TriggerState {
    /// Last trigger time (unix timestamp ms)
    pub last_triggered: u64,
    /// Previous value (for change detection)
    pub previous_value: Option<Value>,
    /// Whether currently in triggered state (for threshold hysteresis)
    pub is_triggered: bool,
}

/// Trigger manager
pub struct TriggerManager {
    /// Trigger states by script_id:trigger_index
    states: std::collections::HashMap<String, TriggerState>,
}

impl TriggerManager {
    pub fn new() -> Self {
        Self {
            states: std::collections::HashMap::new(),
        }
    }

    /// Check if a trigger should fire
    pub fn check_trigger(
        &mut self,
        script_id: &str,
        trigger_index: usize,
        trigger: &Trigger,
        context: &ScriptContext,
    ) -> bool {
        let state_key = format!("{}:{}", script_id, trigger_index);
        let now_ms = Utc::now().timestamp_millis() as u64;

        // Get or create state
        if !self.states.contains_key(&state_key) {
            self.states
                .insert(state_key.clone(), TriggerState::default());
        }

        // Check debounce first (read-only)
        {
            let state = self.states.get(&state_key).unwrap();
            if let Some(debounce) = trigger.debounce_ms {
                if now_ms - state.last_triggered < debounce {
                    return false;
                }
            }
        }

        // Evaluate trigger type
        let should_trigger = match trigger.trigger_type {
            TriggerType::Threshold => Self::check_threshold_static(
                trigger,
                context,
                self.states.get_mut(&state_key).unwrap(),
            ),
            TriggerType::Change => Self::check_change_static(
                trigger,
                context,
                self.states.get_mut(&state_key).unwrap(),
            ),
            TriggerType::Schedule => Self::check_schedule_static(trigger),
            TriggerType::Interval => {
                let state = self.states.get(&state_key).unwrap();
                Self::check_interval_static(trigger, state, now_ms)
            }
            TriggerType::GpioChange => Self::check_gpio_change_static(
                trigger,
                context,
                self.states.get_mut(&state_key).unwrap(),
            ),
            TriggerType::Manual => false,  // Only triggered via command
            TriggerType::Startup => false, // Handled separately on startup
        };

        if should_trigger {
            if let Some(state) = self.states.get_mut(&state_key) {
                state.last_triggered = now_ms;
            }
        }

        should_trigger
    }

    /// Check threshold trigger (static version)
    fn check_threshold_static(
        trigger: &Trigger,
        context: &ScriptContext,
        state: &mut TriggerState,
    ) -> bool {
        let current_value = match context.get_value(&trigger.source) {
            Some(v) => v,
            None => {
                debug!("Threshold source '{}' not found in context", trigger.source);
                return false;
            }
        };

        let threshold = match &trigger.value {
            Some(v) => v,
            None => {
                warn!("Threshold trigger missing value");
                return false;
            }
        };

        let operator = trigger.operator.as_ref().unwrap_or(&ComparisonOperator::Gt);

        let condition_met = Self::compare_values_static(&current_value, operator, threshold);

        // Edge detection - only trigger on transition from false to true
        let was_triggered = state.is_triggered;
        state.is_triggered = condition_met;

        condition_met && !was_triggered
    }

    /// Check change trigger (static version)
    fn check_change_static(
        trigger: &Trigger,
        context: &ScriptContext,
        state: &mut TriggerState,
    ) -> bool {
        let current_value = match context.get_value(&trigger.source) {
            Some(v) => v,
            None => return false,
        };

        let changed = match &state.previous_value {
            Some(prev) => prev != &current_value,
            None => true, // First time
        };

        state.previous_value = Some(current_value);

        changed
    }

    /// Check schedule trigger (static version - simplified cron)
    fn check_schedule_static(trigger: &Trigger) -> bool {
        let cron = match &trigger.cron {
            Some(c) => c,
            None => return false,
        };

        let now = Utc::now();
        let parts: Vec<&str> = cron.split_whitespace().collect();

        if parts.len() < 5 {
            warn!("Invalid cron expression: {}", cron);
            return false;
        }

        // Format: minute hour day month weekday
        let minute_match = Self::match_cron_field_static(parts[0], now.minute());
        let hour_match = Self::match_cron_field_static(parts[1], now.hour());
        let day_match = Self::match_cron_field_static(parts[2], now.day());
        let month_match = Self::match_cron_field_static(parts[3], now.month());
        let weekday_match =
            Self::match_cron_field_static(parts[4], now.weekday().num_days_from_sunday());

        minute_match && hour_match && day_match && month_match && weekday_match
    }

    /// Match a cron field (static version)
    fn match_cron_field_static(pattern: &str, value: u32) -> bool {
        if pattern == "*" {
            return true;
        }

        // Handle */N (every N)
        if pattern.starts_with("*/") {
            if let Ok(n) = pattern[2..].parse::<u32>() {
                // Guard against division by zero - */0 is invalid cron syntax
                if n == 0 {
                    return false;
                }
                return value % n == 0;
            }
        }

        // Handle comma-separated values
        if pattern.contains(',') {
            return pattern
                .split(',')
                .any(|p| p.trim().parse::<u32>().map(|v| v == value).unwrap_or(false));
        }

        // Handle range (N-M)
        if pattern.contains('-') {
            let parts: Vec<&str> = pattern.split('-').collect();
            if parts.len() == 2 {
                if let (Ok(start), Ok(end)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
                    return value >= start && value <= end;
                }
            }
        }

        // Simple number match
        pattern.parse::<u32>().map(|v| v == value).unwrap_or(false)
    }

    /// Check interval trigger (static version)
    ///
    /// # Safety
    /// Uses saturating_sub to prevent underflow panic when SystemTime goes backwards
    /// (e.g., NTP sync, manual time change, or time zone updates)
    fn check_interval_static(trigger: &Trigger, state: &TriggerState, now_ms: u64) -> bool {
        let interval_ms = trigger.interval_secs.unwrap_or(60) * 1000;
        now_ms.saturating_sub(state.last_triggered) >= interval_ms
    }

    /// Check GPIO change trigger (static version)
    fn check_gpio_change_static(
        trigger: &Trigger,
        context: &ScriptContext,
        state: &mut TriggerState,
    ) -> bool {
        // Parse pin number from source
        let pin: u8 = match trigger.source.parse() {
            Ok(p) => p,
            Err(_) => {
                // Try gpio:N format
                if trigger.source.starts_with("gpio:") {
                    match trigger.source[5..].parse() {
                        Ok(p) => p,
                        Err(_) => return false,
                    }
                } else {
                    return false;
                }
            }
        };

        let current_state = match context.get_gpio(pin) {
            Some(s) => Value::from(s),
            None => return false,
        };

        let changed = match &state.previous_value {
            Some(prev) => prev != &current_state,
            None => false, // Don't trigger on first read
        };

        state.previous_value = Some(current_state);

        changed
    }

    /// Compare two JSON values (static version)
    fn compare_values_static(left: &Value, op: &ComparisonOperator, right: &Value) -> bool {
        // Try numeric comparison first
        if let (Some(l), Some(r)) = (left.as_f64(), right.as_f64()) {
            return match op {
                ComparisonOperator::Eq => (l - r).abs() < f64::EPSILON,
                ComparisonOperator::Ne => (l - r).abs() >= f64::EPSILON,
                ComparisonOperator::Gt => l > r,
                ComparisonOperator::Gte => l >= r,
                ComparisonOperator::Lt => l < r,
                ComparisonOperator::Lte => l <= r,
                ComparisonOperator::Between => {
                    // Right should be an array [min, max]
                    if let Some(arr) = right.as_array() {
                        if arr.len() >= 2 {
                            let min = arr[0].as_f64().unwrap_or(f64::MIN);
                            let max = arr[1].as_f64().unwrap_or(f64::MAX);
                            return l >= min && l <= max;
                        }
                    }
                    false
                }
                ComparisonOperator::In => {
                    if let Some(arr) = right.as_array() {
                        return arr.iter().any(|v| {
                            v.as_f64()
                                .map(|r| (l - r).abs() < f64::EPSILON)
                                .unwrap_or(false)
                        });
                    }
                    false
                }
                ComparisonOperator::Contains => false, // Not applicable to numbers
            };
        }

        // String comparison
        if let (Some(l), Some(r)) = (left.as_str(), right.as_str()) {
            return match op {
                ComparisonOperator::Eq => l == r,
                ComparisonOperator::Ne => l != r,
                ComparisonOperator::Contains => l.contains(r),
                ComparisonOperator::Gt => l > r,
                ComparisonOperator::Gte => l >= r,
                ComparisonOperator::Lt => l < r,
                ComparisonOperator::Lte => l <= r,
                _ => false,
            };
        }

        // Boolean comparison
        if let (Some(l), Some(r)) = (left.as_bool(), right.as_bool()) {
            return match op {
                ComparisonOperator::Eq => l == r,
                ComparisonOperator::Ne => l != r,
                _ => false,
            };
        }

        false
    }

    /// Reset trigger state for a script
    pub fn reset_script(&mut self, script_id: &str) {
        let prefix = format!("{}:", script_id);
        self.states.retain(|k, _| !k.starts_with(&prefix));
    }
}

impl Default for TriggerManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_threshold_trigger() {
        let mut manager = TriggerManager::new();
        let mut context = ScriptContext::new();
        context.set_sensor("temp", 25.0);

        let trigger = Trigger {
            trigger_type: TriggerType::Threshold,
            source: "temp".to_string(),
            operator: Some(ComparisonOperator::Gt),
            value: Some(Value::from(28.0)),
            cron: None,
            interval_secs: None,
            debounce_ms: None,
        };

        // Below threshold - should not trigger
        assert!(!manager.check_trigger("test", 0, &trigger, &context));

        // Above threshold - should trigger
        context.set_sensor("temp", 30.0);
        assert!(manager.check_trigger("test", 0, &trigger, &context));

        // Still above threshold - should NOT trigger (edge detection)
        assert!(!manager.check_trigger("test", 0, &trigger, &context));
    }

    #[test]
    fn test_cron_field_match() {
        // Wildcard
        assert!(TriggerManager::match_cron_field_static("*", 5));

        // Exact match
        assert!(TriggerManager::match_cron_field_static("5", 5));
        assert!(!TriggerManager::match_cron_field_static("5", 6));

        // Every N
        assert!(TriggerManager::match_cron_field_static("*/5", 0));
        assert!(TriggerManager::match_cron_field_static("*/5", 15));
        assert!(!TriggerManager::match_cron_field_static("*/5", 7));

        // Range
        assert!(TriggerManager::match_cron_field_static("5-10", 7));
        assert!(!TriggerManager::match_cron_field_static("5-10", 3));

        // List
        assert!(TriggerManager::match_cron_field_static("1,5,10", 5));
        assert!(!TriggerManager::match_cron_field_static("1,5,10", 7));
    }

    #[test]
    fn test_cron_field_division_by_zero() {
        // */0 should NOT panic, should return false
        assert!(!TriggerManager::match_cron_field_static("*/0", 0));
        assert!(!TriggerManager::match_cron_field_static("*/0", 5));
        assert!(!TriggerManager::match_cron_field_static("*/0", 100));
    }
}
