# Suderra Edge Agent v1.2.0

Industrial IoT Edge Agent for aquaculture monitoring and control systems. Built with Rust for reliability, safety, and performance on resource-constrained edge devices.

## What's New in v1.2.0

- **Modbus TLS/mTLS Support**: Encrypted Modbus TCP with rodbus (IEC 62443 SL2 FR4)
- **Parallel Modbus Reads**: Concurrent device polling with `read_all_parallel()`
- **Circuit Breaker Half-Open Permits**: Limited concurrent requests during recovery
- **Cron Wrap-Around Ranges**: Night schedules like `22-6` now work correctly
- **Offline Queue Disk Limits**: Configurable max disk usage (default: 50MB)
- **GPIO Invert Consistency**: Invert flag now applied on both read and write
- **Thread-Safe Script Storage**: `tokio::sync::RwLock` for concurrent access
- **Systemd Watchdog**: Automatic heartbeat for service health monitoring

## Table of Contents

- [Overview](#overview)
- [IEC 62443 Security Compliance](#iec-62443-security-compliance)
- [Architecture](#architecture)
- [Features](#features)
- [Target Platforms](#target-platforms)
- [Building](#building)
- [Configuration](#configuration)
- [Protocol Support](#protocol-support)
- [Script Engine](#script-engine)
- [Function Blocks](#function-blocks-iec-61131-3)
- [MQTT Commands](#mqtt-commands)
- [Resilience Patterns](#resilience-patterns)
- [Security Hardening](#security-hardening)
- [Development](#development)
- [CI/CD Pipeline](#cicd-pipeline)
- [License](#license)

---

## Overview

Suderra Edge Agent is a production-ready IoT edge agent designed for industrial aquaculture environments. It provides:

- **Real-time Monitoring**: Continuous sensor data collection via Modbus TCP/RTU
- **Automated Control**: PLC-like scripting with IEC 61131-3 function blocks
- **Secure Communication**: MQTT with TLS/mTLS support
- **Zero-Touch Provisioning**: Single command deployment
- **Fault Tolerance**: Circuit breakers, rate limiting, offline message queuing

### Why Rust?

- **Memory Safety**: No buffer overflows, null pointer dereferences, or data races
- **Performance**: Near-C performance with zero-cost abstractions
- **Reliability**: Extensive compile-time checks prevent runtime errors
- **Small Footprint**: Optimized release builds (~3-5MB binary)
- **Cross-Compilation**: Easy builds for ARM64, ARMv7, x86_64

---

## IEC 62443 Security Compliance

This agent is designed to meet **IEC 62443 Security Level 2 (SL2)** requirements for industrial control systems.

### Foundational Requirements (FR) Implementation

| FR | Requirement | Implementation |
|----|-------------|----------------|
| **FR1** | Identification & Authentication | MQTT mTLS, device certificates, machine-uid fingerprinting |
| **FR2** | Use Control | Role-based command authorization, tenant isolation |
| **FR3** | System Integrity | Input validation, Clippy lints, cargo-audit, cargo-deny, fuzz testing |
| **FR4** | Data Confidentiality | TLS 1.2+ for MQTT, rustls (no OpenSSL) |
| **FR5** | Restricted Data Flow | Rate limiting, circuit breakers, network segmentation |
| **FR6** | Timely Response | Systemd watchdog, health endpoints, graceful shutdown |
| **FR7** | Resource Availability | Bounded collections, memory limits, offline queue |

### Security Tools

```bash
# Vulnerability scanning (CI automated)
cargo audit

# License and dependency policy (CI automated)
cargo deny check

# Fuzz testing for input validation
cargo +nightly fuzz run config_parse
cargo +nightly fuzz run mqtt_payload
cargo +nightly fuzz run modbus_response
```

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                         Suderra Edge Agent                          │
├─────────────────────────────────────────────────────────────────────┤
│                                                                     │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌────────────┐ │
│  │   MQTT      │  │   Script    │  │   Modbus    │  │    GPIO    │ │
│  │   Client    │  │   Engine    │  │   Actor     │  │   Actor    │ │
│  │             │  │             │  │             │  │            │ │
│  │ - TLS/mTLS  │  │ - Event     │  │ - TCP/RTU   │  │ - rppal    │ │
│  │ - Backoff   │  │ - Scan      │  │ - Circuit   │  │ - Read/    │ │
│  │ - LWT       │  │   Cycle     │  │   Breaker   │  │   Write    │ │
│  │ - Offline   │  │ - FB Exec   │  │ - Rate      │  │            │ │
│  │   Queue     │  │ - Conflict  │  │   Limit     │  │            │ │
│  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘  └─────┬──────┘ │
│         │                │                │               │        │
│         └────────────────┼────────────────┼───────────────┘        │
│                          │                │                        │
│                    ┌─────┴────────────────┴─────┐                  │
│                    │       App State            │                  │
│                    │   (Arc&lt;RwLock&lt;..&gt;&gt;)       │                  │
│                    └─────────────┬──────────────┘                  │
│                                  │                                 │
│  ┌─────────────┐  ┌──────────────┴───────────────┐  ┌────────────┐ │
│  │  Shutdown   │  │       Persistence            │  │  Telemetry │ │
│  │ Coordinator │  │                              │  │            │ │
│  │             │  │ - SQLite WAL mode            │  │ - CPU/Mem  │ │
│  │ - Broadcast │  │ - RETAIN variables           │  │ - Disk     │ │
│  │ - Timeout   │  │ - FB state                   │  │ - Temp     │ │
│  │ - Tasks     │  │ - Offline queue              │  │ - Modbus   │ │
│  └─────────────┘  └──────────────────────────────┘  └────────────┘ │
│                                                                     │
└─────────────────────────────────────────────────────────────────────┘
```

### Key Design Patterns

| Pattern | Purpose | Implementation |
|---------|---------|----------------|
| **Actor Pattern** | Thread-safe hardware access | `ModbusHandle`, `GpioHandle` with mpsc channels |
| **Circuit Breaker** | Fault isolation | Atomic state machine (Closed→Open→HalfOpen) |
| **Token Bucket** | Rate limiting | Atomic counter with time-based refill |
| **Singleton** | Shared script storage | `Arc<RwLock<ScriptStorage>>` |
| **Broadcast** | Graceful shutdown | `tokio::sync::broadcast` for coordinated termination |

### Thread Safety

- **No `std::sync::Mutex` in async code**: Uses `tokio::sync::RwLock` for async contexts
- **SQLite via `spawn_blocking`**: Prevents blocking the async runtime
- **Atomic Circuit Breaker**: Lock-free state transitions with `compare_exchange`

---

## Features

### Core Capabilities

| Feature | Description |
|---------|-------------|
| **Zero-Touch Provisioning** | Single curl command installation with device code |
| **MQTT Communication** | QoS 1/2, TLS/mTLS, Last Will Testament, clean session control |
| **System Telemetry** | CPU, memory, disk, temperature (configurable interval) |
| **Modbus TCP/RTU** | Actor-pattern client with byte order support |
| **GPIO Support** | Raspberry Pi / Revolution Pi (Linux only, optional feature) |
| **Offline Queue** | SQLite-backed priority queue for network outages |
| **Health Endpoints** | HTTP `/health`, `/ready`, `/metrics` (optional feature) |

### Script Engine (v2.0+)

| Feature | Description |
|---------|-------------|
| **Event-Driven Mode** | Trigger-based execution (checked every 1 second) |
| **Scan Cycle Mode** | IEC 61131-3 PLC-like deterministic execution (10-10000ms) |
| **Priority System** | Scripts execute in priority order (Critical→High→Normal→Low→Background) |
| **Conflict Detection** | Automatic GPIO/Modbus write conflict resolution |
| **Rate Limiting** | Configurable per-script execution rate |
| **Execution Limits** | Max depth, time, and action count per run |
| **RETAIN Variables** | SQLite-persisted variables restored on restart |

### IEC 61131-3 Function Blocks

| Block | Type | Description |
|-------|------|-------------|
| **TON** | Timer | On-Delay Timer (Q=true after PT elapsed while IN=true) |
| **TOF** | Timer | Off-Delay Timer (Q=false after PT elapsed while IN=false) |
| **RS** | Flip-Flop | Reset-dominant Set-Reset |
| **SR** | Flip-Flop | Set-dominant Set-Reset |
| **CTU** | Counter | Count Up (Q=true when CV >= PV) |
| **CTD** | Counter | Count Down (Q=true when CV <= 0) |
| **R_TRIG** | Edge | Rising Edge Detector |
| **F_TRIG** | Edge | Falling Edge Detector |

---

## Target Platforms

| Platform | Architecture | Target Triple | Binary Name |
|----------|--------------|---------------|-------------|
| x86_64 Linux | amd64 | `x86_64-unknown-linux-gnu` | `suderra-agent-amd64` |
| ARM64 Linux | arm64 | `aarch64-unknown-linux-gnu` | `suderra-agent-arm64` |
| ARMv7 Linux | armhf | `armv7-unknown-linux-gnueabihf` | `suderra-agent-armhf` |

### Supported Hardware

- Revolution Pi Connect 4 / Compact (ARM)
- Raspberry Pi 4 / 5 (ARM64)
- Industrial PCs (x86_64)
- Any Linux system with systemd

---

## Building

### Prerequisites

- **Rust**: 1.70+ (2021 edition)
- **cargo-deny**: For dependency policy checks
- **cargo-fuzz**: For security testing (nightly toolchain)

### Quick Build

```bash
# Clone repository
git clone https://github.com/suderra/edge-agent.git
cd edge-agent

# Build release
cargo build --release

# Binary location
ls -la target/release/suderra-agent
```

### Feature Flags

```bash
# Default build (no optional features)
cargo build --release

# With GPIO support (Linux only)
cargo build --release --features gpio

# With health HTTP endpoints
cargo build --release --features health

# With OpenTelemetry tracing
cargo build --release --features telemetry

# With Prometheus metrics
cargo build --release --features metrics

# All features (Linux only)
cargo build --release --features "gpio,health,telemetry,metrics"
```

### Cross-Compilation

```bash
# Install cross-compilation tools
rustup target add aarch64-unknown-linux-gnu
rustup target add armv7-unknown-linux-gnueabihf

# ARM64 build
cargo build --release --target aarch64-unknown-linux-gnu

# ARMv7 build
cargo build --release --target armv7-unknown-linux-gnueabihf
```

### Release Profile

The `Cargo.toml` includes optimized release settings:

```toml
[profile.release]
opt-level = "z"     # Optimize for size
lto = true          # Link-time optimization
codegen-units = 1   # Better optimization
panic = "abort"     # Smaller binary
strip = true        # Strip symbols
```

---

## Configuration

### Config File Location

- Default: `/etc/suderra/config.yaml`
- Override: `SUDERRA_CONFIG=/path/to/config.yaml`

### Full Configuration Example

```yaml
# Device identification
device_id: "550e8400-e29b-41d4-a716-446655440000"
device_code: "RPI-A1B2C3D4"

# Provisioning API
api_url: "https://api.your-platform.com"

# MQTT Configuration
mqtt:
  broker: "mqtt.your-platform.com"
  port: 8883
  username: "device_${device_id}"
  password: "${mqtt_password}"
  client_id: "suderra-${device_code}"
  clean_session: false  # For QoS 1/2 persistence
  keep_alive_secs: 30

  # TLS Configuration (IEC 62443 FR4)
  tls:
    enabled: true
    ca_cert_path: "/etc/suderra/certs/ca.pem"
    client_cert_path: "/etc/suderra/certs/client.pem"  # mTLS
    client_key_path: "/etc/suderra/certs/client.key"   # mTLS
    verify_hostname: true

  # Topic templates
  topics:
    status: "tenants/{tenant_id}/devices/{device_id}/status"
    telemetry: "tenants/{tenant_id}/devices/{device_id}/telemetry"
    commands: "tenants/{tenant_id}/devices/{device_id}/commands"
    responses: "tenants/{tenant_id}/devices/{device_id}/responses"
    config: "tenants/{tenant_id}/devices/{device_id}/config"

# Telemetry Configuration
telemetry:
  interval_seconds: 30
  include_cpu: true
  include_memory: true
  include_disk: true
  include_temperature: true
  include_network: false

# Modbus Devices
modbus:
  - name: "PLC-Main"
    connection_type: "tcp"
    address: "192.168.1.100:502"
    slave_id: 1

    # Security settings (IEC 62443 FR3, FR5)
    security:
      enabled: true
      allowed_function_codes: [1, 2, 3, 4]  # Read-only by default
      allow_writes: false
      rate_limit_ops_per_sec: 10.0
      rate_limit_burst: 20
      max_register_count: 125

    # Register definitions
    registers:
      - name: "water_temperature"
        address: 100
        register_type: "input"
        data_type: "i16"
        scale: 0.1
        byte_order: "big_endian"
        unit: "C"

      - name: "dissolved_oxygen"
        address: 102
        register_type: "input"
        data_type: "u16"
        scale: 0.01
        unit: "mg/L"

      - name: "ph_level"
        address: 104
        register_type: "input"
        data_type: "f32"
        byte_order: "big_endian"
        scale: 1.0
        unit: "pH"

  - name: "PLC-Backup"
    connection_type: "rtu"
    address: "/dev/ttyUSB0"
    slave_id: 2
    baud_rate: 9600
    registers: []

# Logging Configuration
logging:
  level: "info"  # trace, debug, info, warn, error
  format: "json"  # json or pretty

# Script Engine
scripting:
  enabled: true
  execution_mode: "event_driven"  # or "scan_cycle"
  scan_cycle_ms: 100  # Only for scan_cycle mode

  # Execution limits (security)
  limits:
    max_call_depth: 10
    max_execution_time_ms: 5000
    max_actions_per_run: 100
    max_delay_ms: 60000
    rate_limit_per_minute: 60
```

### Byte Order Options

Different PLCs use different byte orders for 32-bit values:

| Byte Order | Description | Example |
|------------|-------------|---------|
| `big_endian` | AB CD (Siemens) | Standard Modbus |
| `little_endian` | CD AB (Schneider) | Reversed registers |
| `big_endian_byte_swap` | BA DC | Swapped bytes per word |
| `little_endian_byte_swap` | DC BA | Both swapped |

---

## Protocol Support

### MQTT

- **Protocol**: MQTT 3.1.1 / 5.0
- **QoS Levels**: 0, 1, 2
- **TLS**: rustls (TLS 1.2/1.3, no OpenSSL dependency)
- **mTLS**: Client certificate authentication
- **Reconnection**: Exponential backoff (1s → 2s → 4s → ... → 60s max)
- **Offline Queue**: SQLite-backed, priority-ordered

### Modbus

- **TCP**: Port 502 (standard) or custom
- **RTU**: Serial (Linux only, 9600-115200 baud)
- **Function Codes**: FC1-4 (read), FC5-6, FC15-16 (write, if enabled)
- **Data Types**: u16, i16, u32, i32, f32, coil, discrete
- **Circuit Breaker**: 3 failures → open → 30s recovery

### GPIO (Linux, optional)

- **Library**: rppal (Raspberry Pi Peripheral Access Library)
- **Supported**: Raspberry Pi 3/4/5, Revolution Pi
- **Modes**: Input, Output, PWM
- **Thread Safety**: Actor pattern with message passing

---

## Script Engine

### Execution Modes

#### Event-Driven Mode (Default)

Scripts execute when triggers fire. Triggers are checked every 1 second.

```yaml
scripting:
  execution_mode: "event_driven"
```

#### Scan Cycle Mode (IEC 61131-3)

Deterministic PLC-like execution with configurable cycle time.

```yaml
scripting:
  execution_mode: "scan_cycle"
  scan_cycle_ms: 100  # 10ms - 10000ms
```

**Scan Cycle Phases:**

1. **Input Scan**: Read all Modbus registers and GPIO pins
2. **Wire FB Inputs**: Map sensor values to function block inputs
3. **Execute FBs**: Run all function blocks
4. **Wire FB Outputs**: Map outputs to actuators/variables
5. **Evaluate Scripts**: Check triggers and execute matching scripts
6. **Periodic Tasks**: Persist state, log statistics
7. **Wait**: Sleep until next cycle

### Script Definition

```json
{
  "id": "high-temp-alarm",
  "name": "High Temperature Alarm",
  "description": "Alert when water temperature exceeds threshold",
  "version": "1.0",
  "enabled": true,
  "priority": "high",

  "triggers": [
    {
      "trigger_type": "threshold",
      "source": "water_temperature",
      "operator": "gt",
      "value": 28.0
    }
  ],

  "conditions": [
    {
      "condition_type": "sensor",
      "source": "pump_running",
      "operator": "eq",
      "value": true
    }
  ],

  "actions": [
    {
      "action_type": "alert",
      "level": "warning",
      "message": "Water temperature high: ${water_temperature}C"
    },
    {
      "action_type": "set_gpio",
      "target": "17",
      "value": true
    }
  ],

  "on_error": [
    {
      "action_type": "log",
      "message": "Script failed: ${error}"
    }
  ]
}
```

### Trigger Types

| Type | Description | Required Fields |
|------|-------------|-----------------|
| `threshold` | Value crosses threshold | `source`, `operator`, `value` |
| `change` | Any value change | `source` |
| `schedule` | Cron expression | `cron` (min hour day month weekday) |
| `interval` | Every N seconds | `interval_secs` (minimum: 1) |
| `gpio_change` | GPIO pin state change | `source` (pin number) |
| `startup` | On agent start | - |
| `manual` | API trigger only | - |

### Action Types

| Type | Description | Required Fields |
|------|-------------|-----------------|
| `set_gpio` | Set GPIO pin | `target` (pin), `value` (bool) |
| `write_modbus` | Write register | `device`, `address`, `value` |
| `write_coil` | Write coil | `device`, `address`, `value` |
| `alert` | Send alert | `level`, `message` |
| `set_variable` | Set variable | `target`, `value`, `scope` |
| `log` | Log message | `message` |
| `delay` | Wait | `delay_ms` |
| `publish_mqtt` | Publish message | `target` (topic), `message` |
| `call_script` | Call another script | `script_id` |

### Priority Levels

| Priority | Value | Use Case |
|----------|-------|----------|
| `critical` | 90 | Emergency shutoffs, safety interlocks |
| `high` | 70 | Important control logic |
| `normal` | 50 | Standard automation (default) |
| `low` | 30 | Logging, notifications |
| `background` | 10 | Housekeeping, cleanup |

### Variable Scopes

| Scope | Description | Persistence |
|-------|-------------|-------------|
| `local` | Script-local (default) | Memory only |
| `global` | Shared across scripts | Memory only |
| `retain` | Survives restart | SQLite |
| `persistent` | Same as retain | SQLite |

### Variable Interpolation

Use `${source}` in messages:

| Pattern | Example | Description |
|---------|---------|-------------|
| `${sensor_name}` | `${water_temperature}` | Sensor value |
| `${gpio:N}` | `${gpio:17}` | GPIO pin state |
| `${var:name}` | `${var:counter}` | Variable value |
| `${time:hour}` | `${time:hour}` | Current hour (0-23) |
| `${time:minute}` | `${time:minute}` | Current minute |
| `${time:weekday}` | `${time:weekday}` | Day (0=Monday) |
| `${system:cpu}` | `${system:cpu}` | CPU usage % |
| `${system:memory}` | `${system:memory}` | Memory usage % |

---

## Function Blocks (IEC 61131-3)

### Timer Blocks

#### TON (On-Delay Timer)

Output Q becomes TRUE after input IN has been TRUE for time PT.

```
IN: ─┐       ┌───────────────
     └───────┘

Q:  ─────┐       ┌───────────
         └───────┘
         |<-PT-->|
```

#### TOF (Off-Delay Timer)

Output Q becomes FALSE after input IN has been FALSE for time PT.

### Flip-Flops

#### RS (Reset-Set, Reset Dominant)

```
If R=TRUE: Q=FALSE
Else if S=TRUE: Q=TRUE
```

#### SR (Set-Reset, Set Dominant)

```
If S=TRUE: Q=TRUE
Else if R=TRUE: Q=FALSE
```

### Counters

#### CTU (Count Up)

Increments CV on rising edge of CU. Q=TRUE when CV >= PV.

#### CTD (Count Down)

Decrements CV on rising edge of CD. Q=TRUE when CV <= 0.

### Edge Detectors

#### R_TRIG (Rising Edge)

Q=TRUE for one cycle when CLK transitions FALSE→TRUE.

#### F_TRIG (Falling Edge)

Q=TRUE for one cycle when CLK transitions TRUE→FALSE.

### FB Wiring

```json
{
  "function_blocks": [
    {
      "id": "pump_timer",
      "fb_type": "TON",
      "params": { "pt_ms": 5000 },
      "inputs": {
        "IN": "sensor:water_temp_high"
      },
      "outputs": {
        "Q": "var:pump_enabled"
      },
      "retain": true
    }
  ]
}
```

**Input Sources:**
- `sensor:name` - Sensor value
- `gpio:pin` - GPIO state (bool)
- `var:name` - Variable value
- `fb:id.output` - Another FB's output
- Literal value - Number or boolean

**Output Targets:**
- `var:name` - Store in variable
- `sensor:name` - Virtual sensor value

---

## MQTT Commands

### System Commands

| Command | Description | Parameters |
|---------|-------------|------------|
| `ping` | Health check | - |
| `get_info` | Device info | - |
| `get_config` | Current config | - |
| `get_hardware` | Hardware list | - |
| `reboot` | Reboot device | `delay_seconds` |
| `restart_agent` | Restart service | - |
| `set_log_level` | Change log level | `level` |

### Hardware Commands

| Command | Description | Parameters |
|---------|-------------|------------|
| `read_modbus` | Read registers | `device` (optional) |
| `write_modbus` | Write register | `device`, `address`, `value` |
| `read_gpio` | Read all pins | - |
| `write_gpio` | Write pin | `pin`, `state` |

### Script Commands

| Command | Description | Parameters |
|---------|-------------|------------|
| `list_scripts` | List scripts | - |
| `get_script` | Get script | `id` |
| `deploy_script` | Deploy script | Script JSON |
| `delete_script` | Delete script | `id` |
| `enable_script` | Enable script | `id` |
| `disable_script` | Disable script | `id` |

### Program Commands

| Command | Description | Parameters |
|---------|-------------|------------|
| `deploy_program` | Deploy program | Program JSON |
| `get_program` | Get program | - |
| `clear_program` | Remove program | - |
| `get_fb_states` | FB states | - |
| `get_scan_stats` | Scan statistics | - |

---

## Resilience Patterns

### Circuit Breaker

Prevents cascading failures by stopping requests to failing services.

```
States: CLOSED → OPEN → HALF_OPEN → CLOSED

CLOSED: Normal operation, track failures
        → OPEN after 3 consecutive failures

OPEN: Reject all requests
      → HALF_OPEN after 30 seconds

HALF_OPEN: Allow test requests
           → CLOSED after 2 successes
           → OPEN on any failure
```

Implementation: Lock-free atomic state machine (`src/resilience/circuit_breaker.rs`)

### Rate Limiter (Token Bucket)

Prevents resource exhaustion and DoS attacks.

```
Configuration:
  - capacity: Maximum burst size
  - refill_rate: Tokens per second

Example: 10 ops/sec, burst of 20
  - 10 tokens added per second
  - Can burst up to 20 requests
  - Then limited to 10/sec steady state
```

### Offline Message Queue

SQLite-backed priority queue for network outages.

- **Persistence**: Messages survive restart
- **Priority**: Critical messages sent first
- **Deduplication**: Optional message coalescing
- **Retry**: Exponential backoff on failures

### Graceful Shutdown

Coordinated shutdown sequence:

1. Signal all tasks via broadcast channel
2. Wait for in-flight operations (configurable timeout)
3. Save RETAIN variables and FB states
4. Flush offline queue
5. Publish offline status
6. Disconnect MQTT

---

## Security Hardening

### Input Validation

- **Script IDs**: Alphanumeric + hyphen + underscore, max 64 chars
- **Path Traversal**: No `..` or absolute paths in file references
- **Config Values**: Bounds checking on all numeric fields
- **Modbus Addresses**: Validated against configured range

### Resource Limits

| Limit | Default | Purpose |
|-------|---------|---------|
| Max call depth | 10 | Prevent infinite recursion |
| Max execution time | 5000ms | Prevent runaway scripts |
| Max actions/run | 100 | Prevent action floods |
| Max delay | 60000ms | Prevent long blocks |
| Rate limit | 60/min | Prevent abuse |
| Scan cycle | 10-10000ms | Bounded timing |

### Clippy Lints

```toml
[lints.clippy]
unwrap_used = "warn"         # Prefer explicit error handling
expect_used = "warn"         # Document panic conditions
indexing_slicing = "warn"    # Use .get() for bounds safety
large_stack_arrays = "warn"  # Prevent stack overflow
todo = "warn"                # Track incomplete code
unimplemented = "warn"       # No unimplemented panics
dbg_macro = "warn"           # No debug macros
print_stdout = "warn"        # Use tracing instead
print_stderr = "warn"        # Use tracing instead
```

### Dependency Policy (cargo-deny)

```toml
[licenses]
allow = ["MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Zlib", "CC0-1.0", "Unicode-DFS-2016", "Unlicense", "MPL-2.0"]

[bans]
deny = [
  { name = "openssl", reason = "Use rustls for cross-platform TLS" },
  { name = "openssl-sys", reason = "Use rustls for cross-platform TLS" }
]
```

---

## Development

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `RUST_LOG` | Log level filter | `info` |
| `SUDERRA_DATA_DIR` | Data directory | `/var/lib/suderra` |
| `SUDERRA_CONFIG` | Config file path | `/etc/suderra/config.yaml` |

### Running Locally

```bash
# Debug build with logs
RUST_LOG=debug cargo run

# Specific module logging
RUST_LOG=suderra_agent=debug,modbus=info cargo run

# Release build
cargo run --release
```

### Testing

```bash
# Run all tests
cargo test

# Run with output
cargo test -- --nocapture

# Run specific test
cargo test test_circuit_breaker

# Run integration tests
cargo test --test '*'
```

### Code Quality

```bash
# Format code
cargo fmt

# Run lints
cargo clippy

# Check without building
cargo check

# Security audit
cargo audit

# Dependency check
cargo deny check
```

### Fuzz Testing

```bash
# Install cargo-fuzz (requires nightly)
rustup install nightly
cargo +nightly install cargo-fuzz

# Run fuzz targets
cargo +nightly fuzz run config_parse
cargo +nightly fuzz run mqtt_payload
cargo +nightly fuzz run modbus_response
```

---

## CI/CD Pipeline

### GitHub Actions Workflow

The CI pipeline runs on every push and PR:

1. **Format Check**: `cargo fmt --check`
2. **Lint**: `cargo clippy -- -D warnings`
3. **Test**: `cargo test`
4. **Build**: Release builds for all targets
5. **Security Audit**: `cargo audit`
6. **Dependency Check**: `cargo deny check`

### Build Matrix

| Target | OS | Features |
|--------|-----|----------|
| x86_64-unknown-linux-gnu | Ubuntu | Default |
| aarch64-unknown-linux-gnu | Ubuntu (cross) | Default |
| armv7-unknown-linux-gnueabihf | Ubuntu (cross) | Default |

### Release Process

1. Tag with semver: `git tag v1.0.0`
2. Push tag: `git push origin v1.0.0`
3. GitHub Actions builds release binaries
4. Binaries attached to GitHub Release

---

## Data Directories

| Path | Purpose |
|------|---------|
| `/etc/suderra/` | Configuration files |
| `/etc/suderra/certs/` | TLS certificates |
| `/etc/suderra/scripts/` | Script definitions |
| `/var/lib/suderra/` | Persistent data |
| `/var/lib/suderra/retain.db` | RETAIN variable storage |
| `/var/lib/suderra/offline.db` | Offline message queue |
| `/var/lib/suderra/program.json` | Deployed program state |

---

## Systemd Service

```ini
[Unit]
Description=Suderra Edge Agent
After=network-online.target
Wants=network-online.target

[Service]
Type=notify
ExecStart=/usr/local/bin/suderra-agent
Restart=on-failure
RestartSec=5s
WatchdogSec=30s
MemoryMax=128M
CPUQuota=50%
NoNewPrivileges=true
ProtectSystem=strict
ReadWritePaths=/var/lib/suderra

[Install]
WantedBy=multi-user.target
```

### Service Commands

```bash
# Status
systemctl status suderra-agent

# Logs (follow)
journalctl -u suderra-agent -f

# Recent logs
journalctl -u suderra-agent -n 100

# Restart
systemctl restart suderra-agent

# Stop
systemctl stop suderra-agent
```

---

## License

Proprietary - Suderra

Copyright (c) 2024 Suderra. All rights reserved.
