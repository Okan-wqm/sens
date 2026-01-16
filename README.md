# Suderra Edge Agent v2.2

Industrial IoT agent for aquaculture monitoring and control with IEC 61131-3 PLC-like scripting capabilities.

## Features

### Core Capabilities
- **Zero-Touch Provisioning**: Single curl command installation
- **MQTT Communication**: Real-time telemetry and command handling
- **System Telemetry**: CPU, memory, disk, temperature monitoring
- **Modbus TCP/RTU**: PLC and sensor integration via actor pattern
- **GPIO Support**: Raspberry Pi / Revolution Pi I/O (optional feature)

### IEC 61131-3 Compliance (v2.1)
- **Scan Cycle Mode**: Deterministic PLC-like execution (10-10000ms configurable)
- **Function Blocks**: TON, TOF, RS, SR, CTU, CTD, R_TRIG, F_TRIG
- **RETAIN Variables**: SQLite-backed persistent variables
- **Program Deployment**: Full program configuration via MQTT commands

### Script Engine (v2.0)
- **Event-Driven Mode**: Traditional trigger-based execution
- **Priority System**: Scripts execute in priority order (0-100)
- **Conflict Detection**: Automatic detection and resolution of GPIO/Modbus write conflicts
- **Rate Limiting**: Configurable rate limits per script
- **Execution Limits**: Depth, time, and action count limits

### Architecture (v2.2)
- **Actor Pattern**: Thread-safe Modbus and GPIO handles
- **Shared Storage**: Singleton ScriptStorage pattern
- **Graceful Shutdown**: Coordinated task termination with configurable timeout
- **Async-Safe Persistence**: spawn_blocking for SQLite operations

## Target Platforms

This agent is designed for Linux-based edge devices:

| Platform | Architecture | Binary |
|----------|--------------|--------|
| x86_64 Linux | amd64 | `suderra-agent-amd64` |
| ARM64 Linux | arm64 | `suderra-agent-arm64` |
| ARMv7 Linux | arm | `suderra-agent-arm` |

Supported hardware:
- Revolution Pi Connect 4 / Compact
- Raspberry Pi 4 / 5
- Industrial PCs (x86_64)
- Any Linux system with systemd

## Building

### Prerequisites

- Rust 1.70+ (2021 edition)
- For GPIO support: Linux only

### On Linux (Recommended)

```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Build
cd edge-agent
cargo build --release

# Build with GPIO support (Linux only)
cargo build --release --features gpio

# Cross-compile for ARM64
rustup target add aarch64-unknown-linux-gnu
cargo build --release --target aarch64-unknown-linux-gnu

# Cross-compile for ARMv7
rustup target add armv7-unknown-linux-gnueabihf
cargo build --release --target armv7-unknown-linux-gnueabihf
```

### GitHub Actions Build

The repository includes CI/CD workflows for automated builds:
- Builds for all three architectures on every push
- Creates releases with pre-built binaries
- Cross-compilation handled by GitHub Actions

## Installation

On target Linux device:

```bash
curl -sSL http://your-api-server/install/DEVICE-CODE | sudo sh
```

## Configuration

Config file: `/etc/suderra/config.yaml`

```yaml
device_id: "uuid-here"
device_code: "RPI-A1B2C3D4"
api_url: "http://your-api-server"

mqtt:
  broker: "mqtt.your-server.com"
  port: 1883
  topics:
    status: "tenants/{tenant_id}/devices/{device_id}/status"
    telemetry: "tenants/{tenant_id}/devices/{device_id}/telemetry"
    commands: "tenants/{tenant_id}/devices/{device_id}/commands"

telemetry:
  interval_seconds: 30
  include_cpu: true
  include_memory: true
  include_disk: true
  include_temperature: true

modbus:
  - name: "PLC-1"
    connection_type: "tcp"
    address: "192.168.1.100:502"
    slave_id: 1
    registers:
      - name: "water_temperature"
        address: 100
        register_type: "input"
        data_type: "i16"
        scale: 0.1
        unit: "°C"
```

## Execution Modes

### Event-Driven Mode (Default)

Scripts execute when triggers fire. Checked every 1 second.

```json
{
  "command": "deploy_program",
  "params": {
    "id": "aqua-control",
    "execution_mode": "event_driven"
  }
}
```

### Scan Cycle Mode (IEC 61131-3)

PLC-like deterministic execution with configurable cycle time.

```json
{
  "command": "deploy_program",
  "params": {
    "id": "aqua-control",
    "execution_mode": "scan_cycle",
    "scan_cycle_ms": 100,
    "function_blocks": [
      {
        "id": "pump_timer",
        "fb_type": "TON",
        "inputs": {
          "IN": "sensor:water_temp > 28",
          "PT": 5000
        },
        "outputs": {
          "Q": "gpio:17"
        },
        "retain": true
      }
    ]
  }
}
```

#### Scan Cycle Phases

1. **Input Scan**: Read all sensors and GPIO
2. **Context Update**: Update script context with latest values
3. **FB Execution**: Execute all function blocks
4. **Wire FB Outputs**: Connect FB outputs to actuators
5. **Script Evaluation**: Check triggers and execute scripts
6. **Periodic Tasks**: Persist FB states, log statistics
7. **Wait**: Sleep until next cycle

## Function Blocks (IEC 61131-3)

### Timer Blocks

| Block | Description | Inputs | Outputs |
|-------|-------------|--------|---------|
| `TON` | On-Delay Timer | IN, PT | Q, ET |
| `TOF` | Off-Delay Timer | IN, PT | Q, ET |

### Flip-Flops

| Block | Description | Inputs | Outputs |
|-------|-------------|--------|---------|
| `RS` | Reset-Set (Reset dominant) | R, S | Q |
| `SR` | Set-Reset (Set dominant) | S, R | Q |

### Counters

| Block | Description | Inputs | Outputs |
|-------|-------------|--------|---------|
| `CTU` | Count Up | CU, R, PV | Q, CV |
| `CTD` | Count Down | CD, LD, PV | Q, CV |

### Edge Triggers

| Block | Description | Inputs | Outputs |
|-------|-------------|--------|---------|
| `R_TRIG` | Rising Edge Detector | CLK | Q |
| `F_TRIG` | Falling Edge Detector | CLK | Q |

### FB Input Sources

- `sensor:name` - Sensor value
- `gpio:pin` - GPIO pin state
- `fb:id.output` - Another FB's output
- Numeric literal - Constant value
- Boolean expression - e.g., `sensor:temp > 25`

### RETAIN Variables

Function blocks marked with `"retain": true` persist their state to SQLite and restore on agent restart.

## Commands

### System Commands

| Command | Description | Parameters |
|---------|-------------|------------|
| `ping` | Health check | - |
| `get_info` | Device information | - |
| `get_config` | Current configuration | - |
| `get_hardware` | List all connected hardware | - |
| `reboot` | Reboot device | `delay_seconds` |
| `restart_agent` | Restart agent service | - |
| `set_log_level` | Change log level | `level` |

### Hardware Commands

| Command | Description | Parameters |
|---------|-------------|------------|
| `read_modbus` | Read Modbus registers | `device` (optional) |
| `write_modbus` | Write Modbus register | `device`, `address`, `value` |
| `read_gpio` | Read all GPIO pins | - |
| `write_gpio` | Write GPIO pin | `pin`, `state` (high/low) |

### Script Commands

| Command | Description | Parameters |
|---------|-------------|------------|
| `list_scripts` | List all deployed scripts | - |
| `get_script` | Get script details | `id` |
| `deploy_script` | Deploy a new script | Script definition JSON |
| `delete_script` | Delete a script | `id` |
| `enable_script` | Enable a script | `id` |
| `disable_script` | Disable a script | `id` |

### Program Commands (IEC 61131-3)

| Command | Description | Parameters |
|---------|-------------|------------|
| `deploy_program` | Deploy full program configuration | Program definition JSON |
| `get_program` | Get current program configuration | - |
| `clear_program` | Remove program configuration | - |
| `get_fb_states` | Get all function block states | - |
| `get_scan_stats` | Get scan cycle statistics | - |

## Script DSL Reference

### Trigger Types

| Type | Description | Fields |
|------|-------------|--------|
| `threshold` | Value crosses threshold | `source`, `operator`, `value` |
| `change` | Value changes | `source` |
| `schedule` | Cron-like schedule | `cron` (minute hour day month weekday) |
| `interval` | Every N seconds | `interval_secs` (minimum: 1) |
| `gpio_change` | GPIO pin changes | `source` (pin number) |
| `startup` | On agent startup | - |
| `manual` | Manual trigger only | - |

### Action Types

| Type | Description | Fields |
|------|-------------|--------|
| `set_gpio` | Set GPIO pin state | `target` (pin), `value` (true/false) |
| `write_modbus` | Write Modbus register | `device`, `address`, `value` |
| `write_coil` | Write Modbus coil | `device`, `address`, `value` |
| `alert` | Send alert notification | `level`, `message` |
| `set_variable` | Set a variable | `target`, `value`, `scope` |
| `log` | Log a message | `message` |
| `delay` | Delay execution | `delay_ms` |
| `call_script` | Call another script | `script_id` |

### Script Priority (v2.0)

Scripts execute in priority order (higher priority first):

| Priority | Value | Use Case |
|----------|-------|----------|
| `critical` | 90 | Emergency shutoffs |
| `high` | 70 | Safety interlocks |
| `normal` | 50 | Standard automation (default) |
| `low` | 30 | Logging, notifications |
| `background` | 10 | Housekeeping tasks |

### Conflict Detection (v2.0)

When multiple scripts attempt to write the same resource in one cycle:
- Higher priority script wins
- Lower priority script's action is blocked
- Conflict is logged with details

### Variable Interpolation

Use `${source}` syntax in messages:
- `${sensor_name}` - Sensor value
- `${gpio:17}` - GPIO pin state
- `${var:my_var}` - Variable value
- `${time:hour}` - Current hour (0-23)
- `${time:minute}` - Current minute
- `${time:weekday}` - Day of week (0=Monday)
- `${system:cpu}` - CPU usage percent
- `${system:memory}` - Memory usage percent

## Security Hardening (v2.1.1)

### Input Validation
- Script ID validation (alphanumeric, hyphen, underscore only)
- Path traversal protection
- Maximum ID length enforcement (64 chars)
- Config value bounds checking

### Resource Limits
- Scan cycle bounds: 10ms - 10000ms
- Script execution depth limit
- Action count limits per execution
- Rate limiting per script

### Safe Defaults
- Secure file permissions for persistence
- No hidden file creation
- Validated Modbus/GPIO addresses

## MQTT Topics

| Topic | Direction | Description |
|-------|-----------|-------------|
| `tenants/{tid}/devices/{did}/status` | Publish | Device online/offline status |
| `tenants/{tid}/devices/{did}/telemetry` | Publish | System and sensor metrics |
| `tenants/{tid}/devices/{did}/responses` | Publish | Command execution results |
| `tenants/{tid}/devices/{did}/commands` | Subscribe | Remote commands |
| `tenants/{tid}/devices/{did}/config` | Subscribe | Configuration updates |

## Data Directories

| Path | Purpose |
|------|---------|
| `/etc/suderra/` | Configuration files |
| `/etc/suderra/scripts/` | Script definition files |
| `/var/lib/suderra/` | Persistent data |
| `/var/lib/suderra/retain.db` | SQLite persistence for RETAIN variables |
| `/var/lib/suderra/program.json` | Deployed program configuration |

## Systemd Service

The installer creates `/etc/systemd/system/suderra-agent.service`:

```bash
# Status
systemctl status suderra-agent

# Logs
journalctl -u suderra-agent -f

# Restart
systemctl restart suderra-agent

# View recent logs with context
journalctl -u suderra-agent -n 100 --no-pager
```

## Development

```bash
# Run locally (Linux)
RUST_LOG=debug cargo run

# Run with specific log filter
RUST_LOG=suderra_agent=debug,modbus=info cargo run

# Run tests
cargo test

# Check without building
cargo check

# Format code
cargo fmt

# Run clippy lints
cargo clippy
```

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `RUST_LOG` | Log level filter | `info` |
| `SUDERRA_DATA_DIR` | Data directory | `/var/lib/suderra` |
| `SUDERRA_CONFIG` | Config file path | `/etc/suderra/config.yaml` |

## Changelog

### v2.2.0
- Shared ScriptStorage (singleton pattern) for data consistency
- Graceful shutdown with ShutdownCoordinator
- Optimized regex compilation with OnceLock
- Async-safe SQLite persistence with spawn_blocking
- Script context save/restore for nested calls

### v2.1.0
- IEC 61131-3 scan cycle execution mode
- Function blocks (TON, TOF, RS, SR, CTU, CTD, R_TRIG, F_TRIG)
- RETAIN variable persistence
- Program deployment support
- Security hardening

### v2.0.0
- Script priority system
- Conflict detection for resource writes
- Rate limiting and execution limits
- Actor pattern for Modbus/GPIO

### v1.1.0
- Script DSL with triggers and actions
- GPIO and Modbus integration
- Variable interpolation

### v1.0.0
- Initial release
- MQTT communication
- System telemetry
- Zero-touch provisioning

## License

Proprietary - Suderra
