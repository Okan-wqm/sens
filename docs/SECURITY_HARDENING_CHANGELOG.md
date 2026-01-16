# Edge-Agent Security Hardening Changelog

**Date**: 2026-01-13
**Version**: 2.1.1-security
**Author**: Claude Code

---

## Summary

This document details the security hardening and bug fixes applied to the Suderra Edge Agent. The changes address critical vulnerabilities, improve thread safety, and add input validation across the codebase.

---

## PHASE 1: Critical Bug Fixes

### 1.1 Division by Zero Panic Fix
**File**: `src/scripting/triggers.rs:243`
**Severity**: CRITICAL
**Issue**: Cron pattern `*/0` caused panic via division by zero
**Fix**: Added guard clause to return false for n=0

```rust
// Before (PANICS!)
return value % n == 0;

// After (Safe)
if n == 0 { return false; }
return value % n == 0;
```

**Test Added**: `test_cron_field_division_by_zero`

---

### 1.2 Path Traversal Vulnerability Fix
**File**: `src/scripting/storage.rs`
**Severity**: HIGH
**Issue**: Script IDs like `../../../etc/passwd` could write to arbitrary locations
**Fix**: Added `validate_script_id()` function with comprehensive checks

**Validation Rules**:
- No `..`, `/`, or `\` characters
- Max 64 characters
- Only alphanumeric, hyphen, underscore allowed
- Cannot start with `.`
- Cannot be empty

**Functions Protected**:
- `save()`
- `delete()`
- `enable()`
- `disable()`

**Tests Added**: 4 new test functions covering path traversal, special chars, edge cases

---

### 1.3 Integer Overflow Fix
**Files**: `src/commands.rs:539-581`, `src/scripting/engine.rs:610-621`
**Severity**: MEDIUM
**Issue**: Silent truncation when casting u64 to u16 for Modbus address/value
**Fix**: Added bounds checking before cast

```rust
// Before (Silent truncation - 65536 becomes 0!)
Some(a) => a as u16,

// After (Explicit error)
Some(a) if a <= u16::MAX as u64 => a as u16,
Some(a) => return error("Address {} exceeds maximum u16 value", a),
```

---

### 1.4 Panic Elimination (expect → proper error handling)
**Files**: `src/main.rs`, `src/provisioning.rs`
**Severity**: HIGH
**Issue**: `.expect()` calls caused unrecoverable panics
**Fix**: Converted to `Result<>` returns with proper error propagation

**Changes**:
- `setup_shutdown_handler()` now returns `Result<Receiver<bool>>`
- `ProvisioningClient::new()` now returns `Result<Self>`
- Added `anyhow::Context` for error context

---

### 1.5 Config Update Implementation
**File**: `src/commands.rs:914-973`
**Severity**: MEDIUM
**Issue**: `handle_config_update()` was a TODO stub
**Fix**: Implemented full config update handling

**Supported Updates**:
- `telemetry.interval_seconds` (validated: 5-3600 seconds)
- `telemetry.include_system`
- `telemetry.include_modbus`
- `telemetry.include_gpio`
- `scripting.enabled`

**Error Handling**: Invalid values are logged with warnings and ignored

---

## PHASE 2: Security Hardening

### 2.1 File Permission Enforcement
**File**: `src/config.rs:512-524`
**Issue**: Config file with credentials was world-readable (644)
**Fix**: Set permissions to 0600 (owner read/write only) on Unix

```rust
#[cfg(unix)]
{
    let permissions = fs::Permissions::from_mode(0o600);
    fs::set_permissions(&path, permissions)?;
}
```

---

### 2.2 Provisioning Token Memory Cleanup
**File**: `src/main.rs:289-292`
**Issue**: Provisioning token remained in memory after activation
**Fix**: Token is explicitly set to `None` after successful activation

```rust
// SECURITY: Clear provisioning token from memory after successful activation
state_guard.config.provisioning_token = None;
info!("Provisioning token cleared from memory");
```

---

### 2.3 Input Validation Layer
**File**: `src/config.rs:385-492`
**Issue**: No validation of config values
**Fix**: Added comprehensive `validate()` method called on config load

**Validations**:
| Field | Validation |
|-------|------------|
| `device_id` | Non-empty (trimmed) |
| `device_code` | Non-empty (trimmed) |
| `api_url` | Must start with `http://` or `https://` |
| `mqtt.port` | Must be > 0 if broker configured |
| `gpio[].pin` | Must be 0-27 (Raspberry Pi GPIO range) |
| `gpio[].direction` | Must be `input`, `output`, `in`, or `out` |
| `gpio[].pull` | Must be `up`, `down`, `none`, or empty |
| `modbus[].slave_id` | Must be 1-247 (Modbus protocol range) |
| `modbus[].connection_type` | Must be `tcp` or `rtu` |
| `modbus[].address` | Non-empty (trimmed) |
| `telemetry.interval_seconds` | Must be 5-3600 |

**Note**: `gpio` and `modbus` are Vec<GpioConfig> and Vec<ModbusDeviceConfig> respectively

---

### 2.4 Command Rate Limiting
**File**: `src/commands.rs:18-69, 113-120`
**Issue**: No protection against command flooding (DoS)
**Fix**: Added sliding window rate limiter

**Configuration**:
- Max 60 commands per minute
- Sliding window implementation
- Drops messages when limit exceeded (with warning log)

```rust
const RATE_LIMIT_MAX_COMMANDS: usize = 60;
const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);
```

---

### 2.5 SIGTERM/SIGHUP Signal Handling
**File**: `src/main.rs:189-254`
**Issue**: Only SIGINT handled; SIGTERM caused immediate termination
**Fix**: Added Unix signal handlers for SIGTERM and SIGHUP

**Signals Handled**:
- `SIGINT` (Ctrl+C) - All platforms
- `SIGTERM` - Unix only (graceful termination)
- `SIGHUP` - Unix only (hangup)

All signals trigger graceful shutdown via the same channel.

---

## PHASE 3: Race Condition Fixes

### 3.1 Circuit Breaker Thread Safety
**File**: `src/resilience/circuit_breaker.rs`
**Issue**: Mixed `RwLock` and `AtomicU8` caused TOCTOU race conditions
**Fix**: Complete rewrite using only atomic operations

**Before**:
```rust
last_failure: RwLock<Option<Instant>>,  // Race condition!
```

**After**:
```rust
last_failure_ms: AtomicU64,  // Fully atomic
```

**Key Changes**:
- Replaced `RwLock<Option<Instant>>` with `AtomicU64` timestamp
- All state transitions use `compare_exchange` (CAS)
- Removed `.unwrap()` calls on lock operations
- Added retry loops for concurrent access handling
- Added concurrent stress test

---

## Files Modified

| File | Changes |
|------|---------|
| `src/scripting/triggers.rs` | Division by zero fix, test added |
| `src/scripting/storage.rs` | Path traversal validation, 4 tests added |
| `src/commands.rs` | Integer overflow fix, rate limiter, config update impl |
| `src/scripting/engine.rs` | Integer overflow fix for Modbus value |
| `src/main.rs` | Error handling, token cleanup, signal handlers |
| `src/provisioning.rs` | Error handling for HTTP client creation |
| `src/config.rs` | File permissions, validation layer |
| `src/resilience/circuit_breaker.rs` | Complete rewrite for thread safety |

---

## Security Impact Summary

| Vulnerability | Severity | Status |
|--------------|----------|--------|
| Division by zero panic | CRITICAL | Fixed |
| Path traversal | HIGH | Fixed |
| Credential file permissions | HIGH | Fixed |
| Provisioning token exposure | HIGH | Fixed |
| Integer overflow/truncation | MEDIUM | Fixed |
| expect() panics | MEDIUM | Fixed |
| Missing config validation | MEDIUM | Fixed |
| Command flooding (DoS) | MEDIUM | Fixed |
| Missing signal handlers | MEDIUM | Fixed |
| Circuit breaker race condition | MEDIUM | Fixed |

---

## PHASE 4: Compile-Time & Runtime Fixes (v2.1.1)

**Date**: 2026-01-13
**Version**: 2.1.1-hotfix

### 4.1 Config Validation Field Access Fix
**File**: `src/config.rs:417, 447`
**Severity**: CRITICAL (Code would not compile)
**Issue**: Incorrect field access for Vec types

```rust
// Before (COMPILE ERROR - field doesn't exist)
for gpio in &self.gpio.pins {
for device in &self.modbus.devices {

// After (Correct - Vec is directly iterable)
for gpio in &self.gpio {
for device in &self.modbus {
```

### 4.2 Missing TelemetryConfig Fields
**File**: `src/config.rs:180-190`
**Severity**: CRITICAL (Code would not compile)
**Issue**: Commands referenced non-existent config fields

**Added Fields**:
```rust
pub struct TelemetryConfig {
    // ... existing fields ...
    pub include_system: bool,   // NEW
    pub include_modbus: bool,   // NEW
    pub include_gpio: bool,     // NEW
}
```

### 4.3 Missing ScriptingConfig
**File**: `src/config.rs:220-226, 53-55`
**Severity**: CRITICAL (Code would not compile)
**Issue**: Commands referenced `config.scripting.enabled` which didn't exist

**Added**:
```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ScriptingConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

// Added to AgentConfig
pub scripting: ScriptingConfig,
```

### 4.4 Interval Trigger Underflow Fix
**File**: `src/scripting/triggers.rs:279`
**Severity**: HIGH (Runtime panic)
**Issue**: Subtraction underflow when SystemTime goes backwards (NTP sync)

```rust
// Before (PANIC if now_ms < last_triggered!)
now_ms - state.last_triggered >= interval_ms

// After (Safe - saturates to 0)
now_ms.saturating_sub(state.last_triggered) >= interval_ms
```

---

## v2.1.1 Files Modified

| File | Changes |
|------|---------|
| `src/config.rs` | Fixed field access, added TelemetryConfig fields, added ScriptingConfig |
| `src/scripting/triggers.rs` | Underflow fix with saturating_sub |

---

## Remaining Work

### PHASE 3 (Advanced - Optional)
- Script engine current_script_id thread safety (requires API changes)
- ScriptStorage singleton pattern (requires architectural changes)

### PHASE 4 (Testing)
- Integration tests for provisioning flow
- Hardware abstraction layer for testing
- Property-based testing (fuzzing)

### PHASE 5 (Cleanup)
- Remove unused `notify` dependency
- Remove unused `uuid` dependency
- Dead code removal

---

## Testing Recommendations

```bash
# Run unit tests
cargo test

# Run specific security-related tests
cargo test test_cron_field_division_by_zero
cargo test test_validate_script_id
cargo test test_concurrent_state_transitions

# Check for security advisories
cargo audit

# Verify no panics in release mode
cargo build --release
```

---

## Deployment Notes

1. **Config Migration**: Existing config files will be validated on load. Invalid configs will fail to load with descriptive error messages.

2. **File Permissions**: Config file permissions will be automatically set to 0600 on save (Unix only).

3. **Rate Limiting**: Default rate limit is 60 commands/minute. Adjust `RATE_LIMIT_MAX_COMMANDS` if needed for high-frequency deployments.

4. **Signal Handling**: Applications using `kill -TERM <pid>` will now trigger graceful shutdown instead of immediate termination.

---

*Generated by Claude Code on 2026-01-13*
