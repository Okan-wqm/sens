//! Script execution context
//!
//! Provides access to sensor data, GPIO, variables, and system info
//! for script condition evaluation and action execution.
//!
//! v2.2: Optimized regex compilation with lazy static initialization

use chrono::{Datelike, FixedOffset, TimeZone, Timelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::OnceLock;

/// Lazily compiled regex for variable interpolation (${var_name} syntax)
/// This avoids recompiling the regex on every interpolate() call (v2.2 performance fix)
static INTERPOLATION_REGEX: OnceLock<regex::Regex> = OnceLock::new();

/// Get or initialize the interpolation regex
fn get_interpolation_regex() -> &'static regex::Regex {
    INTERPOLATION_REGEX.get_or_init(|| {
        regex::Regex::new(r"\$\{([^}]+)\}")
            .expect("Invalid interpolation regex pattern - this is a bug")
    })
}

/// Script execution context - provides data for condition evaluation
#[derive(Debug, Clone)]
pub struct ScriptContext {
    /// Sensor/register values (name -> value)
    pub sensors: HashMap<String, f64>,

    /// GPIO pin states (pin -> state as bool)
    pub gpio: HashMap<u8, bool>,

    /// User-defined variables
    pub variables: HashMap<String, Value>,

    /// System metrics
    pub system: SystemContext,

    /// Current timestamp
    pub timestamp: i64,

    /// Timezone offset in seconds from UTC (v1.2.1 - issue #22)
    /// Positive = east of UTC, negative = west of UTC
    /// E.g., +3600 = UTC+1, -18000 = UTC-5
    pub timezone_offset_secs: i32,
}

impl Default for ScriptContext {
    fn default() -> Self {
        Self {
            sensors: HashMap::new(),
            gpio: HashMap::new(),
            variables: HashMap::new(),
            system: SystemContext::default(),
            timestamp: Utc::now().timestamp(),
            timezone_offset_secs: 0, // Default: UTC
        }
    }
}

/// System context for scripts
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SystemContext {
    pub cpu_usage: f32,
    pub memory_usage: f32,
    pub disk_usage: f32,
    pub temperature: Option<f32>,
    pub uptime_seconds: u64,
}

impl ScriptContext {
    /// Create a new context with UTC timezone (default)
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new context with specified timezone offset (v1.2.1 - issue #22)
    ///
    /// # Arguments
    /// * `offset_secs` - Timezone offset in seconds from UTC
    ///   - Positive values = east of UTC (e.g., +3600 = UTC+1)
    ///   - Negative values = west of UTC (e.g., -18000 = UTC-5)
    pub fn with_timezone(offset_secs: i32) -> Self {
        Self {
            timezone_offset_secs: offset_secs,
            ..Self::default()
        }
    }

    /// Set timezone offset (v1.2.1 - issue #22)
    pub fn set_timezone(&mut self, offset_secs: i32) {
        self.timezone_offset_secs = offset_secs;
    }

    /// Update timestamp to current time
    pub fn refresh_time(&mut self) {
        self.timestamp = Utc::now().timestamp();
    }

    /// Set sensor value
    pub fn set_sensor(&mut self, name: &str, value: f64) {
        self.sensors.insert(name.to_string(), value);
    }

    /// Get sensor value
    pub fn get_sensor(&self, name: &str) -> Option<f64> {
        self.sensors.get(name).copied()
    }

    /// Set GPIO state
    pub fn set_gpio(&mut self, pin: u8, state: bool) {
        self.gpio.insert(pin, state);
    }

    /// Get GPIO state
    pub fn get_gpio(&self, pin: u8) -> Option<bool> {
        self.gpio.get(&pin).copied()
    }

    /// Set variable
    pub fn set_variable(&mut self, name: &str, value: Value) {
        self.variables.insert(name.to_string(), value);
    }

    /// Get variable
    pub fn get_variable(&self, name: &str) -> Option<&Value> {
        self.variables.get(name)
    }

    /// Get value by source string (sensor:name, gpio:pin, var:name, time:hour, etc.)
    pub fn get_value(&self, source: &str) -> Option<Value> {
        let parts: Vec<&str> = source.split(':').collect();

        if parts.len() == 1 {
            // Direct sensor name
            self.sensors.get(source).map(|v| Value::from(*v))
        } else if parts.len() >= 2 {
            let source_type = parts[0];
            let source_name = parts[1];

            match source_type {
                "sensor" | "register" => self.sensors.get(source_name).map(|v| Value::from(*v)),
                "gpio" | "pin" => source_name
                    .parse::<u8>()
                    .ok()
                    .and_then(|pin| self.gpio.get(&pin))
                    .map(|v| Value::from(*v)),
                "var" | "variable" => self.variables.get(source_name).cloned(),
                "time" => {
                    // v1.2.1: Apply timezone offset for local time calculations
                    let utc_now = Utc::now();
                    let local_time = if self.timezone_offset_secs != 0 {
                        let offset = FixedOffset::east_opt(self.timezone_offset_secs)
                            .unwrap_or_else(|| FixedOffset::east_opt(0).unwrap());
                        offset.from_utc_datetime(&utc_now.naive_utc())
                    } else {
                        FixedOffset::east_opt(0).unwrap().from_utc_datetime(&utc_now.naive_utc())
                    };
                    match source_name {
                        "hour" => Some(Value::from(local_time.hour())),
                        "minute" => Some(Value::from(local_time.minute())),
                        "second" => Some(Value::from(local_time.second())),
                        "day" => Some(Value::from(local_time.day())),
                        "month" => Some(Value::from(local_time.month())),
                        "year" => Some(Value::from(local_time.year())),
                        "weekday" => Some(Value::from(local_time.weekday().num_days_from_monday())),
                        "timestamp" => Some(Value::from(utc_now.timestamp())), // Always UTC
                        "offset" => Some(Value::from(self.timezone_offset_secs)), // v1.2.1
                        "utc_hour" => Some(Value::from(utc_now.hour())), // v1.2.1: explicit UTC
                        _ => None,
                    }
                }
                "system" | "sys" => match source_name {
                    "cpu" | "cpu_usage" => Some(Value::from(self.system.cpu_usage)),
                    "memory" | "mem" | "memory_usage" => {
                        Some(Value::from(self.system.memory_usage))
                    }
                    "disk" | "disk_usage" => Some(Value::from(self.system.disk_usage)),
                    "temp" | "temperature" => self.system.temperature.map(Value::from),
                    "uptime" => Some(Value::from(self.system.uptime_seconds)),
                    _ => None,
                },
                _ => None,
            }
        } else {
            None
        }
    }

    /// Interpolate variables in a string (${var_name} syntax)
    ///
    /// v2.2: Uses lazily compiled static regex for better performance
    pub fn interpolate(&self, template: &str) -> String {
        let mut result = template.to_string();

        // Use pre-compiled regex for ${...} patterns (v2.2 performance fix)
        let re = get_interpolation_regex();

        for cap in re.captures_iter(template) {
            let full_match = cap.get(0).unwrap().as_str();
            let var_name = &cap[1];

            if let Some(value) = self.get_value(var_name) {
                let replacement = match value {
                    Value::Number(n) => {
                        if let Some(f) = n.as_f64() {
                            format!("{:.2}", f)
                        } else {
                            n.to_string()
                        }
                    }
                    Value::String(s) => s,
                    Value::Bool(b) => b.to_string(),
                    _ => value.to_string(),
                };
                result = result.replace(full_match, &replacement);
            }
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_sensor_access() {
        let mut ctx = ScriptContext::new();
        ctx.set_sensor("water_temp", 25.5);
        ctx.set_sensor("do_level", 7.2);

        assert_eq!(ctx.get_sensor("water_temp"), Some(25.5));
        assert_eq!(ctx.get_sensor("do_level"), Some(7.2));
        assert_eq!(ctx.get_sensor("unknown"), None);
    }

    #[test]
    fn test_context_get_value() {
        let mut ctx = ScriptContext::new();
        ctx.set_sensor("water_temp", 25.5);
        ctx.set_gpio(17, true);
        ctx.set_variable("threshold", Value::from(28.0));

        // Direct sensor
        assert_eq!(ctx.get_value("water_temp"), Some(Value::from(25.5)));

        // Prefixed sensor
        assert_eq!(ctx.get_value("sensor:water_temp"), Some(Value::from(25.5)));

        // GPIO
        assert_eq!(ctx.get_value("gpio:17"), Some(Value::from(true)));

        // Variable
        assert_eq!(ctx.get_value("var:threshold"), Some(Value::from(28.0)));
    }

    #[test]
    fn test_interpolation() {
        let mut ctx = ScriptContext::new();
        ctx.set_sensor("water_temp", 25.5);
        ctx.set_sensor("ph", 7.2);

        let template = "Temperature: ${water_temp}°C, pH: ${ph}";
        let result = ctx.interpolate(template);

        assert!(result.contains("25.50"));
        assert!(result.contains("7.20"));
    }
}
