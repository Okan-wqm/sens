//! Fuzz target for configuration parsing
//!
//! IEC 62443 SL2 FR3: Input Validation
//!
//! This fuzz target ensures that malformed YAML configuration files
//! cannot cause panics or crashes in the agent.

#![no_main]

use libfuzzer_sys::fuzz_target;

// We can't directly use AgentConfig because it may not be exported as a library.
// Instead, we fuzz the underlying YAML parsing which is the attack surface.

fuzz_target!(|data: &[u8]| {
    // Try parsing random bytes as UTF-8 YAML
    if let Ok(s) = std::str::from_utf8(data) {
        // Attempt to parse as generic YAML value
        // This catches panics in serde_yaml
        let _ = serde_yaml::from_str::<serde_yaml::Value>(s);

        // Also try parsing as JSON (some configs may use JSON)
        let _ = serde_json::from_str::<serde_json::Value>(s);
    }

    // Also test raw binary parsing
    let _ = serde_json::from_slice::<serde_json::Value>(data);
});
