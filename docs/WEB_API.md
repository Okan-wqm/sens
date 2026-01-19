# Suderra Edge Agent - Web API Reference

**Version**: 1.2.6
**Platform**: Raspberry Pi / Revolution Pi / Generic Linux
**Protocol**: MQTT 3.1.1 + HTTP Health API

---

## Table of Contents

1. [MQTT Communication](#1-mqtt-communication)
2. [Remote Commands](#2-remote-commands)
3. [Telemetry](#3-telemetry)
4. [Hardware Interfaces](#4-hardware-interfaces)
5. [Scripting Engine](#5-scripting-engine)
6. [Function Blocks (IEC 61131-3)](#6-function-blocks-iec-61131-3)
7. [Offline Queue](#7-offline-queue)
8. [Resilience Patterns](#8-resilience-patterns)
9. [HTTP Health API](#9-http-health-api)
10. [Security](#10-security)
11. [Configuration](#11-configuration)
12. [Provisioning](#12-provisioning)
13. [Limits & Defaults](#13-limits--defaults)

---

## 1. MQTT Communication

### 1.1 Topic Structure

```
tenants/{tenant_id}/devices/{device_id}/status      # Device status (publish)
tenants/{tenant_id}/devices/{device_id}/telemetry   # Metrics data (publish)
tenants/{tenant_id}/devices/{device_id}/responses   # Command responses (publish)
tenants/{tenant_id}/devices/{device_id}/commands    # Incoming commands (subscribe)
tenants/{tenant_id}/devices/{device_id}/config      # Config updates (subscribe)
```

### 1.2 QoS Levels

| Topic | QoS | Retain | Interval |
|-------|-----|--------|----------|
| status | 1 (At Least Once) | true | ~90s |
| telemetry | 0 (At Most Once) | false | 30s (configurable) |
| responses | 1 (At Least Once) | false | On command |
| commands | 1 (At Least Once) | - | Subscribe |
| config | 1 (At Least Once) | - | Subscribe |

### 1.3 Connection Parameters

| Parameter | Default | Range |
|-----------|---------|-------|
| Port (plain) | 1883 | - |
| Port (TLS) | 8883 | - |
| Keep-alive | 30s | 1-3600s |
| Clean Session | true | - |
| Channel Capacity | 500 messages | - |
| Reconnect Min | 1s | - |
| Reconnect Max | 60s | - |

---

## 2. Remote Commands

### 2.1 Command Message Format

**Request:**
```json
{
  "command_id": "cmd_unique_123",
  "command": "ping",
  "params": {},
  "timestamp": "2026-01-19T12:00:00Z"
}
```

**Response:**
```json
{
  "command_id": "cmd_unique_123",
  "device_id": "device-uuid",
  "success": true,
  "result": { "pong": true },
  "timestamp": "2026-01-19T12:00:01Z",
  "error": null
}
```

### 2.2 System Commands

#### `ping`
Bağlantı testi.

```json
// Response
{ "pong": true, "timestamp": "2026-01-19T12:00:00Z" }
```

#### `get_info`
Cihaz bilgileri.

```json
// Response
{
  "device_id": "uuid",
  "device_code": "RPI-A1B2C3D4",
  "agent_version": "1.2.6",
  "os": "Linux 5.15.0",
  "arch": "aarch64",
  "activated": true,
  "uptime_secs": 86400
}
```

#### `get_config`
Aktif konfigürasyon.

```json
// Response
{
  "telemetry_interval_secs": 30,
  "log_level": "info",
  "modbus_device_count": 3,
  "gpio_pin_count": 8,
  "scripts_enabled": true
}
```

#### `get_hardware`
Donanım durumu.

```json
// Response
{
  "platform": "raspberry_pi",
  "modbus_devices": [
    { "name": "inverter1", "connected": true, "registers": 12 }
  ],
  "gpio_pins": [
    { "pin": 17, "name": "relay1", "direction": "output", "state": "high" }
  ]
}
```

#### `reboot`
Sistemi yeniden başlat.

```json
// Request
{ "params": { "delay_seconds": 5 } }

// Response
{ "scheduled": true, "delay_seconds": 5 }
```

#### `restart_agent`
Agent servisini yeniden başlat.

```json
// Response
{ "scheduled": true }
```

#### `set_log_level`
Log seviyesi değiştir.

```json
// Request
{ "params": { "level": "debug" } }

// Levels: trace, debug, info, warn, error
```

### 2.3 Modbus Commands

#### `read_modbus`
Tüm register değerlerini oku.

```json
// Request (optional device filter)
{ "params": { "device": "inverter1" } }

// Response
{
  "devices": [
    {
      "name": "inverter1",
      "registers": [
        {
          "name": "voltage",
          "address": 100,
          "raw_value": 2305,
          "scaled_value": 230.5,
          "unit": "V",
          "timestamp": "2026-01-19T12:00:00Z"
        }
      ],
      "errors": []
    }
  ]
}
```

#### `write_modbus`
Register değeri yaz.

```json
// Request
{
  "params": {
    "device": "inverter1",
    "address": 100,
    "value": 2300
  }
}

// Response
{ "written": true, "address": 100, "value": 2300 }
```

### 2.4 GPIO Commands

#### `read_gpio`
Tüm pin durumlarını oku.

```json
// Response
{
  "pins": [
    {
      "pin": 17,
      "name": "relay1",
      "direction": "output",
      "state": "high",
      "invert": false
    },
    {
      "pin": 18,
      "name": "button1",
      "direction": "input",
      "state": "low",
      "pull": "up"
    }
  ]
}
```

#### `write_gpio`
Pin değeri yaz.

```json
// Request
{ "params": { "pin": 17, "state": "high" } }

// State values: "high", "low", "1", "0", "true", "false", "on", "off"

// Response
{ "pin": 17, "state": "high", "previous": "low" }
```

### 2.5 Script Commands

#### `list_scripts`
```json
// Response
{
  "scripts": [
    {
      "id": "alarm_handler",
      "name": "Alarm Handler",
      "enabled": true,
      "priority": "high",
      "trigger_count": 3,
      "action_count": 5
    }
  ]
}
```

#### `get_script`
```json
// Request
{ "params": { "id": "alarm_handler" } }

// Response: Full ScriptDefinition object
```

#### `deploy_script`
```json
// Request
{
  "params": {
    "script": {
      "id": "alarm_handler",
      "name": "Alarm Handler",
      "enabled": true,
      "priority": "high",
      "triggers": [...],
      "conditions": [...],
      "actions": [...],
      "onError": [...]
    }
  }
}

// Response
{ "deployed": true, "script_id": "alarm_handler" }
```

#### `delete_script`, `enable_script`, `disable_script`
```json
// Request
{ "params": { "id": "script_id" } }
```

### 2.6 IEC 61131-3 Program Commands

#### `deploy_program`
```json
// Request
{
  "params": {
    "program": {
      "id": "main_control",
      "name": "Main Control Program",
      "version": 1,
      "executionMode": "ScanCycle",
      "scanCycleMs": 100,
      "functionBlocks": [...],
      "script": {...},
      "replaceExisting": true
    }
  }
}

// Response
{
  "deployed": true,
  "program_id": "main_control",
  "fb_count": 12,
  "execution_mode": "ScanCycle"
}
```

#### `get_program`
```json
// Response
{
  "program": {
    "id": "main_control",
    "name": "Main Control Program",
    "version": 1,
    "fb_count": 12
  }
}
```

#### `rollback_program`
Önceki program versiyonuna geri dön.

---

## 3. Telemetry

### 3.1 Telemetry Message Format

```json
{
  "device_id": "device-uuid",
  "device_code": "RPI-A1B2C3D4",
  "timestamp": "2026-01-19T12:00:00Z",
  "metrics": {
    "cpu_usage_percent": 25.5,
    "memory_usage_percent": 45.2,
    "memory_used_mb": 1843,
    "memory_total_mb": 4096,
    "disk_usage_percent": 62.3,
    "disk_used_gb": 19.92,
    "disk_total_gb": 32.0,
    "temperature_celsius": 55.0,
    "network_rx_bytes": 123456789,
    "network_tx_bytes": 987654321,
    "modbus": [...],
    "gpio": [...]
  }
}
```

### 3.2 Modbus Device Data

```json
{
  "device_name": "inverter1",
  "registers": [
    {
      "name": "voltage",
      "value": 230.5,
      "unit": "V"
    },
    {
      "name": "current",
      "value": 15.2,
      "unit": "A"
    }
  ],
  "errors": []
}
```

### 3.3 GPIO Pin Data

```json
{
  "pin": 17,
  "name": "relay1",
  "state": "high",
  "direction": "output"
}
```

### 3.4 Configuration

| Parameter | Default | Range |
|-----------|---------|-------|
| interval_seconds | 30 | 5-3600 |
| include_cpu | true | - |
| include_memory | true | - |
| include_disk | true | - |
| include_temperature | true | - |
| include_modbus | true | - |
| include_gpio | true | - |

---

## 4. Hardware Interfaces

### 4.1 Modbus

#### Connection Types

| Type | Format | Example |
|------|--------|---------|
| TCP | `host:port` | `192.168.1.100:502` |
| TCP+TLS | `host:port` | `192.168.1.100:62502` |
| RTU | `/dev/ttyUSB0` | Serial port |

#### Data Types

| Type | Size | Range |
|------|------|-------|
| `u16` | 2 bytes | 0-65535 |
| `i16` | 2 bytes | -32768 to 32767 |
| `u32` | 4 bytes | 0-4294967295 |
| `i32` | 4 bytes | -2147483648 to 2147483647 |
| `f32` | 4 bytes | IEEE 754 float |

#### Byte Order Options

- `big_endian` (default)
- `little_endian`
- `big_endian_byte_swap`
- `little_endian_byte_swap`

#### Register Types

| Type | Function Code | Description |
|------|---------------|-------------|
| `holding` | FC 3/6/16 | Read/Write registers |
| `input` | FC 4 | Read-only registers |
| `coil` | FC 1/5/15 | Digital outputs |
| `discrete` | FC 2 | Digital inputs |

#### Security

| Parameter | Default |
|-----------|---------|
| Allowed Function Codes | [1, 2, 3, 4] (read-only) |
| Rate Limit | 10 ops/sec |
| Burst Capacity | 20 ops |
| Max Register Count | 125 |
| Allow Writes | false |
| Slave ID Range | 1-247 |

#### Register Configuration

```yaml
registers:
  - name: "water_temp"
    address: 100
    registerType: "holding"
    dataType: "f32"
    byteOrder: "big_endian"
    scale: 0.1
    offset: 0
    unit: "°C"
    pollIntervalMs: 1000
```

### 4.2 GPIO

#### Pin Configuration

```yaml
gpio:
  - name: "pump_relay"
    pin: 17
    direction: "output"
    pull: "none"
    invert: false
    debounceMs: 50
```

#### Platform Limits

| Platform | Pin Range |
|----------|-----------|
| Raspberry Pi | 0-27 |
| Revolution Pi | 0-127 |
| Generic Linux | 0-255 |

#### Direction & Pull Options

| Direction | Pull Options |
|-----------|-------------|
| `input` | `up`, `down`, `none` |
| `output` | `none` |

---

## 5. Scripting Engine

### 5.1 Execution Modes

#### Event-Driven (Default)
- Triggers checked every 1 second
- Scripts execute on trigger match
- Non-deterministic timing

#### Scan-Cycle (PLC Mode)
```
1. READ INPUTS (Modbus, GPIO)
2. WIRE FB INPUTS
3. EXECUTE FUNCTION BLOCKS
4. WIRE FB OUTPUTS
5. EVALUATE TRIGGERS
6. EXECUTE ACTIONS
7. PERSIST STATES
8. WAIT FOR NEXT CYCLE
```

| Parameter | Default | Range |
|-----------|---------|-------|
| Scan Cycle | 100ms | 10-10000ms |

### 5.2 Script Definition

```json
{
  "id": "alarm_handler",
  "name": "Alarm Handler",
  "description": "Handle temperature alarms",
  "version": "1.0.0",
  "enabled": true,
  "priority": "high",
  "triggers": [...],
  "conditions": [...],
  "actions": [...],
  "onError": [...]
}
```

#### Priority Levels

| Priority | Value | Description |
|----------|-------|-------------|
| `critical` | 3 | Safety-critical |
| `high` | 2 | Important |
| `normal` | 1 | Default |
| `low` | 0 | Background |

### 5.3 Trigger Types

#### `threshold`
Değer eşik kontrolü.

```json
{
  "type": "threshold",
  "source": "sensor:temperature",
  "operator": "gt",
  "value": 80,
  "debounce_ms": 1000
}
```

#### `change`
Değer değişikliği.

```json
{
  "type": "change",
  "source": "sensor:water_level",
  "debounce_ms": 500
}
```

#### `schedule`
Cron zamanlama.

```json
{
  "type": "schedule",
  "cron": "0 8 * * 1-5"
}
```

**Cron Format:** `minute hour day month weekday`
- Supports: `*`, `N`, `N-M`, `*/N`, `1,2,3`
- Sunday: 0 or 7

#### `interval`
Periyodik çalıştırma.

```json
{
  "type": "interval",
  "interval_secs": 60
}
```

#### `gpio_change`
GPIO pin değişikliği.

```json
{
  "type": "gpio_change",
  "source": "gpio:17"
}
```

#### `manual`
Sadece komut ile tetikleme.

#### `startup`
Agent başlangıcında çalıştır.

### 5.4 Operators

| Operator | Description | Example |
|----------|-------------|---------|
| `eq` | Eşit | `value == 100` |
| `ne` | Eşit değil | `value != 100` |
| `gt` | Büyük | `value > 100` |
| `gte` | Büyük eşit | `value >= 100` |
| `lt` | Küçük | `value < 100` |
| `lte` | Küçük eşit | `value <= 100` |
| `between` | Aralık | `[10, 90]` |
| `in` | Liste içinde | `[1, 2, 3]` |
| `contains` | İçerir (string) | `"error"` |

### 5.5 Condition Types

```json
{
  "type": "sensor",
  "source": "sensor:pressure",
  "operator": "lt",
  "value": 5.0
}
```

| Type | Source Format |
|------|---------------|
| `sensor` | `sensor:register_name` |
| `gpio` | `gpio:pin_number` |
| `variable` | `var:variable_name` |
| `system` | `system:uptime` |

### 5.6 Action Types

#### `set_gpio`
```json
{
  "type": "set_gpio",
  "target": "17",
  "value": true
}
```

#### `write_modbus`
```json
{
  "type": "write_modbus",
  "device": "inverter1",
  "address": 100,
  "value": 2300
}
```

#### `write_coil`
```json
{
  "type": "write_coil",
  "device": "plc1",
  "address": 0,
  "value": true
}
```

#### `set_variable`
```json
{
  "type": "set_variable",
  "target": "alarm_count",
  "value": "${alarm_count} + 1",
  "scope": "retain"
}
```

| Scope | Persistence |
|-------|-------------|
| `local` | Memory only |
| `retain` | SQLite (survives restart) |
| `persistent` | Same as retain |

#### `alert`
```json
{
  "type": "alert",
  "message": "Temperature critical: ${sensor:temperature}°C",
  "level": "critical"
}
```

#### `log`
```json
{
  "type": "log",
  "message": "Script executed at ${system:timestamp}"
}
```

#### `delay`
```json
{
  "type": "delay",
  "delay_ms": 1000
}
```
Max: 30000ms

#### `publish_mqtt`
```json
{
  "type": "publish_mqtt",
  "target": "custom/topic",
  "message": "{\"temp\": ${sensor:temperature}}"
}
```

#### `webhook`
```json
{
  "type": "webhook",
  "url": "https://api.example.com/alert",
  "method": "POST",
  "message": "{\"alert\": \"${message}\"}"
}
```

#### `call_script`
```json
{
  "type": "call_script",
  "script_id": "cleanup_handler"
}
```

### 5.7 Variable Interpolation

Template syntax: `${source:name}`

| Source | Example |
|--------|---------|
| Sensor | `${sensor:temperature}` |
| GPIO | `${gpio:17}` |
| Variable | `${var:counter}` |
| FB Output | `${fb:timer1.Q}` |
| System | `${system:timestamp}` |

### 5.8 Limits

| Parameter | Default | Max |
|-----------|---------|-----|
| Max call depth | 10 | 1000 |
| Max actions | 100 | 10000 |
| Max execution time | 30s | 300s |
| Max delay | 30000ms | - |

---

## 6. Function Blocks (IEC 61131-3)

### 6.1 Timer Function Blocks

#### TON (Timer On-Delay)
IN true olduktan PT ms sonra Q true olur.

```json
{
  "id": "delay_timer",
  "type": "TON",
  "params": {
    "pt_ms": 5000,
    "mode": "wall_clock"
  },
  "inputs": {
    "IN": "sensor:start_signal"
  },
  "outputs": {
    "Q": "var:delayed_output",
    "ET": "var:elapsed_time"
  }
}
```

| Input | Type | Description |
|-------|------|-------------|
| IN | bool | Start signal |
| PT | u64 | Preset time (ms) |

| Output | Type | Description |
|--------|------|-------------|
| Q | bool | Timer done |
| ET | u64 | Elapsed time (ms) |

#### TOF (Timer Off-Delay)
IN false olduktan PT ms sonra Q false olur.

#### TP (Pulse Timer)
IN rising edge'de tam PT ms boyunca Q true.

#### Timer Modes

| Mode | Description |
|------|-------------|
| `wall_clock` | Gerçek zaman (Instant) |
| `scan_cycle` | Cycle sayısı × cycle_time |

### 6.2 Counter Function Blocks

#### CTU (Count Up)
```json
{
  "id": "piece_counter",
  "type": "CTU",
  "params": {
    "pv": 100
  },
  "inputs": {
    "CU": "gpio:17",
    "R": "var:reset_counter"
  },
  "outputs": {
    "Q": "var:batch_complete",
    "CV": "sensor:piece_count"
  }
}
```

#### CTD (Count Down)

### 6.3 Edge & Flip-Flop Blocks

#### R_TRIG (Rising Edge)
#### F_TRIG (Falling Edge)
#### SR (Set-Reset, Set dominant)
#### RS (Reset-Set, Reset dominant)

### 6.4 Controller Blocks

#### PID Controller
```json
{
  "id": "temp_pid",
  "type": "PID",
  "params": {
    "kp": 1.0,
    "ki": 0.1,
    "kd": 0.05,
    "out_min": 0,
    "out_max": 100,
    "setpoint": 75.0
  },
  "inputs": {
    "PV": "sensor:temperature",
    "SP": "var:target_temp"
  },
  "outputs": {
    "OUT": "var:heater_power"
  }
}
```

### 6.5 Input Wiring Sources

| Source | Format | Example |
|--------|--------|---------|
| Sensor | `sensor:name` | `sensor:temperature` |
| GPIO | `gpio:pin` | `gpio:17` |
| Variable | `var:name` | `var:setpoint` |
| FB Output | `fb:id.output` | `fb:timer1.Q` |
| Literal | JSON value | `100`, `true` |

### 6.6 Output Wiring Targets

| Target | Format | Example |
|--------|--------|---------|
| Variable | `var:name` | `var:result` |
| Virtual Sensor | `sensor:name` | `sensor:calculated` |

---

## 7. Offline Queue

### 7.1 Priority Levels

| Priority | Value | Use Case |
|----------|-------|----------|
| Critical | 3 | Alarms, safety |
| High | 2 | Important events |
| Normal | 1 | Regular telemetry |
| Low | 0 | Background data |

### 7.2 Configuration

| Parameter | Default | Range |
|-----------|---------|-------|
| Max Size | 1000 msgs | 100-1000000 |
| Max Age | 3600s | - |
| Max Disk | 50 MB | 1 MB-unlimited |

### 7.3 Behavior

- **Enqueue**: Priority-based eviction when full
- **Dequeue**: Priority DESC, FIFO within priority
- **Disk Limit**: Auto-evict when 80%+ usage
- **Persistence**: SQLite with WAL mode

### 7.4 Statistics

```json
{
  "total_messages": 156,
  "by_priority": [10, 50, 80, 16],
  "oldest_message_age_secs": 120,
  "total_bytes": 524288,
  "db_size_bytes": 1048576,
  "disk_usage_percent": 2.0
}
```

---

## 8. Resilience Patterns

### 8.1 Circuit Breaker

#### States

```
CLOSED ─(N failures)─→ OPEN
    ↑                    │
    │                    ↓ (recovery_timeout)
    └──(M successes)── HALF-OPEN
           ↑               │
           └─(failure)─────┘
```

#### Configuration

| Parameter | Default |
|-----------|---------|
| Failure Threshold | 3 |
| Success Threshold | 2 |
| Recovery Timeout | 30s |
| Half-Open Permits | 1 |

### 8.2 Rate Limiting

| Resource | Limit | Window |
|----------|-------|--------|
| Commands | 60 | 60s |
| Modbus ops | 10/s | Burst: 20 |
| Scripts | Per-script | 60s |

### 8.3 Retry Logic

| Operation | Max Retries | Backoff |
|-----------|-------------|---------|
| MQTT Channel | 3 | 10ms exp |
| GPIO | 3 | 10/20/40ms |
| Modbus | Circuit breaker | - |

---

## 9. HTTP Health API

**Build:** `cargo build --features health`
**Port:** 8080 (configurable)

### GET /health

```json
{
  "status": "healthy",
  "version": "1.2.6",
  "uptime_secs": 86400
}
```

| Status | HTTP | Description |
|--------|------|-------------|
| healthy | 200 | All systems normal |
| degraded | 200 | Some components failing |
| unhealthy | 503 | Critical failure |

### GET /ready

```json
{
  "ready": true,
  "checks": {
    "config_loaded": true,
    "mqtt_connected": true,
    "device_activated": true
  }
}
```

### GET /metrics

```json
{
  "uptime_secs": 86400,
  "mqtt_messages_sent": 1523,
  "mqtt_messages_received": 89,
  "modbus_reads": 45230,
  "script_executions": 12500,
  "offline_queue_size": 0
}
```

### GET /diagnostics

Comprehensive system diagnostics including:
- System info (OS, CPU, memory, disk)
- Process info (PID, threads, memory)
- Component status (MQTT, Modbus, Scripts, FBs, Queue)
- Configuration summary (sanitized)
- Recent errors (last 10)

---

## 10. Security

### 10.1 TLS/mTLS

#### MQTT TLS
```yaml
mqtt:
  tls:
    enabled: true
    caCertPath: "/etc/suderra/ca.pem"
    clientCertPath: "/etc/suderra/client.pem"
    clientKeyPath: "/etc/suderra/client.key"
    verifyHostname: true
```

#### Modbus TLS
```yaml
modbus:
  - tls:
      enabled: true
      serverName: "plc.local"
      caCertPath: "/etc/suderra/modbus-ca.pem"
      insecureSkipVerify: false
```

### 10.2 Credential Protection

- Secrets stored with `secrecy` crate
- Auto-zeroized on drop
- Masked in logs: `[REDACTED]`
- File permissions: 0600 required

### 10.3 Input Validation

| Field | Validation |
|-------|------------|
| Device ID | UUID format |
| API URL | http/https, valid host |
| GPIO Pin | Platform range |
| Modbus Slave | 1-247 |
| Telemetry Interval | 5-3600s |

### 10.4 IEC 62443 SL2 Compliance

| Requirement | Implementation |
|-------------|----------------|
| FR1 Access Control | Device ID, UUID validation |
| FR3 Whitelisting | Modbus FC whitelist |
| FR4 Confidentiality | TLS 1.2+, mTLS |
| FR5 Availability | Bounded queue, circuit breaker |
| FR6 Monitoring | Health API, diagnostics |

---

## 11. Configuration

### 11.1 Config File Path

1. `$SUDERRA_CONFIG` environment variable
2. `/etc/suderra/config.yaml`
3. `./config.yaml`

### 11.2 Full Configuration Schema

```yaml
# Device Identity
deviceId: "xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"
deviceCode: "RPI-A1B2C3D4"
tenantId: "tenant-123"

# Provisioning (cleared after activation)
provisioningToken: "token123"
apiUrl: "https://api.example.com"

# MQTT
mqtt:
  broker: "mqtt.example.com"
  port: 8883
  username: "device"
  password: "secret"
  keepaliveSecs: 30
  cleanSession: true
  tls:
    enabled: true
    caCertPath: "/etc/suderra/ca.pem"
    clientCertPath: "/etc/suderra/client.pem"
    clientKeyPath: "/etc/suderra/client.key"

# Telemetry
telemetry:
  intervalSeconds: 30
  includeCpu: true
  includeMemory: true
  includeDisk: true
  includeTemperature: true
  includeModbus: true
  includeGpio: true
  otlp:
    endpoint: "http://jaeger:4317"
    serviceName: "suderra-agent"
    sampleRatio: 1.0

# Logging
logging:
  level: "info"
  file: "/var/log/suderra-agent.log"

# Hardware
modbus:
  - name: "inverter1"
    connectionType: "tcp"
    address: "192.168.1.100:502"
    slaveId: 1
    security:
      allowedFunctionCodes: [1, 2, 3, 4]
      rateLimitOpsPerSec: 10
      allowWrites: false
    registers:
      - name: "voltage"
        address: 100
        registerType: "holding"
        dataType: "f32"
        scale: 0.1
        unit: "V"

gpio:
  - name: "relay1"
    pin: 17
    direction: "output"
    invert: false

# Scripting
scripting:
  enabled: true
  defaultScanCycleMs: 100
  maxFunctionBlocks: 100
  maxExecutionDepth: 10

# Runtime
runtime:
  rateLimitMaxCommands: 60
  rateLimitWindowSecs: 60
  circuitBreakerRecoverySecs: 30
```

---

## 12. Provisioning

### 12.1 Activation Flow

```
1. Agent starts with provisioning token
2. Collect device fingerprint
3. POST to /api/devices/activate
4. Receive MQTT credentials
5. Store credentials (0600 perms)
6. Connect to MQTT
7. Publish online status
```

### 12.2 Activation Request

```json
{
  "deviceId": "device-uuid",
  "token": "provisioning-token",
  "fingerprint": {
    "cpuSerial": "00000000abcd1234",
    "macAddresses": ["00:11:22:33:44:55"],
    "machineId": "abc123...",
    "hostname": "edge-device-01"
  },
  "agentVersion": "1.2.6"
}
```

### 12.3 Activation Response

```json
{
  "success": true,
  "mqttBroker": "mqtt.example.com",
  "mqttPort": 8883,
  "mqttUsername": "device-123",
  "mqttPassword": "secret",
  "tenantId": "tenant-123",
  "deviceCode": "RPI-A1B2C3D4"
}
```

---

## 13. Limits & Defaults

| Parameter | Default | Min | Max |
|-----------|---------|-----|-----|
| Telemetry Interval | 30s | 5s | 3600s |
| MQTT Keep-alive | 30s | 1s | 3600s |
| MQTT Channel | 500 msgs | - | - |
| Scan Cycle | 100ms | 10ms | 10000ms |
| Script Max Depth | 10 | 1 | 1000 |
| Script Max Actions | 100 | 1 | 10000 |
| Script Max Time | 30s | 1s | 300s |
| Circuit Breaker Recovery | 30s | 1s | 3600s |
| Command Rate Limit | 60/min | 1 | 1000 |
| Modbus Rate Limit | 10/s | 1 | 100 |
| Modbus Timeout | 5s | 1s | 60s |
| GPIO Timeout | 5s | 1s | 60s |
| Offline Queue Max | 1000 | 100 | 1000000 |
| Offline Queue Disk | 50 MB | 1 MB | - |
| Max FBs | 100 | 1 | 1000 |

---

*Generated by Suderra AS*
