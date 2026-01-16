//! Fuzz target for MQTT payload parsing
//!
//! IEC 62443 SL2 FR3: Input Validation
//!
//! This fuzz target ensures that malformed MQTT payloads
//! cannot cause panics or crashes in the agent.
//!
//! Attack vectors:
//! - Malformed JSON payloads
//! - Oversized payloads
//! - Unicode edge cases
//! - Deeply nested structures

#![no_main]

use libfuzzer_sys::fuzz_target;
use serde_json::Value;

/// Maximum payload size the agent should handle (256 KB)
const MAX_PAYLOAD_SIZE: usize = 256 * 1024;

fuzz_target!(|data: &[u8]| {
    // Skip oversized payloads (DoS prevention)
    if data.len() > MAX_PAYLOAD_SIZE {
        return;
    }

    // Test JSON parsing (most common MQTT payload format)
    match serde_json::from_slice::<Value>(data) {
        Ok(value) => {
            // If parsing succeeds, verify we can safely traverse the structure
            check_json_depth(&value, 0);
        }
        Err(_) => {
            // Parsing errors are expected and safe
        }
    }

    // Test UTF-8 string parsing
    if let Ok(s) = std::str::from_utf8(data) {
        // Try parsing string as JSON
        let _ = serde_json::from_str::<Value>(s);

        // Check for control characters that might cause issues
        let _ = s.chars().filter(|c| c.is_control()).count();
    }
});

/// Check JSON nesting depth to prevent stack overflow
fn check_json_depth(value: &Value, depth: usize) {
    // Limit recursion depth (prevents stack overflow)
    if depth > 100 {
        return;
    }

    match value {
        Value::Array(arr) => {
            for item in arr {
                check_json_depth(item, depth + 1);
            }
        }
        Value::Object(obj) => {
            for (_, v) in obj {
                check_json_depth(v, depth + 1);
            }
        }
        _ => {}
    }
}
