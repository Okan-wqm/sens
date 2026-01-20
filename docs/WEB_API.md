# Suderra Edge Agent - Web API Reference

**Version**: 1.3.4 (High Availability Edition)
**Platform**: Raspberry Pi / Revolution Pi / Generic Linux
**Protocol**: MQTT 3.1.1 + HTTP Health API

---

## Table of Contents

1. [MQTT Communication](#1-mqtt-communication)
2. [Remote Commands](#2-remote-commands)
3. [Telemetry](#3-telemetry)
4. [Hardware Interfaces](#4-hardware-interfaces)
5. [PLC Programming (v1.3.0)](#5-plc-programming-v130)
6. [Scripting Engine](#6-scripting-engine)
7. [Function Blocks (IEC 61131-3)](#7-function-blocks-iec-61131-3)
8. [Alarm Management (IEC 62682)](#8-alarm-management-iec-62682)
9. [Backup & Restore](#9-backup--restore)
10. [Offline Queue](#10-offline-queue)
11. [Resilience Patterns](#11-resilience-patterns)
12. [MQTT Broker Failover (v1.3.4)](#12-mqtt-broker-failover-v134)
13. [HTTP Health API](#13-http-health-api)
14. [Security](#14-security)
15. [Configuration](#15-configuration)
16. [Environment Variables](#16-environment-variables)
17. [Feature Flags](#17-feature-flags)
18. [Error Types](#18-error-types)
19. [Provisioning](#19-provisioning)
20. [Limits & Defaults](#20-limits--defaults)

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

### 1.4 Last Will & Testament

MQTT Last Will message support for detecting unexpected disconnections.

```yaml
mqtt:
  lastWillTopic: "tenants/{tenant_id}/devices/{device_id}/status"
  lastWillMessage: '{"status": "offline", "reason": "unexpected"}'
  lastWillQos: 1
  lastWillRetain: true
```

When the device disconnects unexpectedly, the broker automatically publishes the Last Will message.

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

### 4.3 I2C (Inter-Integrated Circuit)

I2C support for sensors, displays, and other I2C peripherals.

#### I2C Buses (Raspberry Pi)

| Bus | SDA | SCL | Usage |
|-----|-----|-----|-------|
| I2C1 | GPIO 2 | GPIO 3 | Primary bus |
| I2C0 | GPIO 0 | GPIO 1 | Reserved for HAT EEPROM |

#### Supported Devices

| Device | Address | Description |
|--------|---------|-------------|
| BME280 | 0x76/0x77 | Temperature, humidity, pressure |
| SHT31 | 0x44/0x45 | Temperature, humidity |
| ADS1115 | 0x48-0x4B | 16-bit ADC |
| PCA9685 | 0x40-0x7F | 16-channel PWM |

#### Configuration

```yaml
i2c:
  - name: "temp_sensor"
    address: 0x76
    bus: 1
    clockSpeedHz: 100000
    description: "BME280 temperature sensor"
```

| Parameter | Default | Range |
|-----------|---------|-------|
| bus | 1 | 0-1 |
| clockSpeedHz | 100000 | 100000-400000 |
| address | - | 0x03-0x77 |

#### Operations

| Operation | Description |
|-----------|-------------|
| `read_register` | Read bytes from a register |
| `write_register` | Write bytes to a register |
| `read_direct` | Read without register address |
| `write_direct` | Write without register address |
| `scan` | Scan bus for devices |
| `probe` | Check if device is present |

### 4.4 PWM (Pulse Width Modulation)

PWM support for motor control, LED dimming, and servo control.

#### Hardware PWM Channels

| Channel | GPIO Pins |
|---------|-----------|
| PWM0 | GPIO 12 (Alt0), GPIO 18 (Alt5) |
| PWM1 | GPIO 13 (Alt0), GPIO 19 (Alt5) |

#### Configuration

```yaml
pwm:
  - name: "motor1"
    pin: 18
    frequencyHz: 25000.0
    initialDutyCycle: 0.0
    hardware: true
    servoMode: false
```

| Parameter | Default | Range |
|-----------|---------|-------|
| frequencyHz | 1000.0 | 1-100000 |
| initialDutyCycle | 0.0 | 0.0-1.0 |
| hardware | true | - |
| servoMode | false | - |

#### Servo Mode

When `servoMode: true`:
- Frequency: 50 Hz
- Pulse width: 1ms (0%) to 2ms (100%)
- Position 0.0-1.0 maps to servo angle

#### Operations

| Operation | Description |
|-----------|-------------|
| `set_duty_cycle` | Set duty cycle (0.0-1.0) |
| `set_frequency` | Change frequency |
| `set_servo_position` | Set servo position (0.0-1.0) |
| `set_enabled` | Enable/disable channel |

### 4.5 SPI (Serial Peripheral Interface)

SPI support for high-speed peripherals like ADCs, DACs, and flash memory.

#### SPI Buses (Raspberry Pi)

| Bus | CE0 | CE1 | MOSI | MISO | SCLK |
|-----|-----|-----|------|------|------|
| SPI0 | GPIO 8 | GPIO 7 | GPIO 10 | GPIO 9 | GPIO 11 |
| SPI1 | GPIO 18 | GPIO 17 | GPIO 20 | GPIO 19 | GPIO 21 |

#### Supported Devices

| Device | Description |
|--------|-------------|
| MCP3008 | 8-channel 10-bit ADC |
| MCP3208 | 8-channel 12-bit ADC |
| MAX31855 | Thermocouple interface |
| W25Q series | Flash memory |

#### SPI Modes

| Mode | CPOL | CPHA | Description |
|------|------|------|-------------|
| Mode0 | 0 | 0 | Clock idle low, sample on rising edge |
| Mode1 | 0 | 1 | Clock idle low, sample on falling edge |
| Mode2 | 1 | 0 | Clock idle high, sample on falling edge |
| Mode3 | 1 | 1 | Clock idle high, sample on rising edge |

#### Configuration

```yaml
spi:
  - name: "adc"
    bus: 0
    chipSelect: 0
    clockSpeedHz: 1000000
    mode: "Mode0"
    bitOrder: "MsbFirst"
    bitsPerWord: 8
```

| Parameter | Default | Range |
|-----------|---------|-------|
| bus | 0 | 0-1 |
| chipSelect | 0 | 0-2 |
| clockSpeedHz | 1000000 | 100000-32000000 |
| mode | Mode0 | Mode0-Mode3 |
| bitOrder | MsbFirst | MsbFirst, LsbFirst |
| bitsPerWord | 8 | 8 |

#### Operations

| Operation | Description |
|-----------|-------------|
| `transfer` | Full-duplex read/write |
| `write` | Write only (discard received) |
| `read` | Read only (send zeros) |
| `set_clock_speed` | Change clock speed |

---

## 5. PLC Programming (v1.3.0)

Upload IEC 61131-3 programs (Structured Text, Ladder, FBD) to external PLCs via industrial protocols.

### 5.1 Supported Protocols

| Protocol | PLCs | Port | Features |
|----------|------|------|----------|
| **Codesys Gateway** | WAGO, Festo, Schneider M241/M251, Codesys V3 | 1217 | Program upload, start/stop, status |
| **S7comm** | Siemens S7-300/400/1200/1500 | 102 | Block upload, CPU control, status |
| **OPC UA** | IEC 62541 compliant PLCs | 4840 | Program transfer via File nodes |
| **EtherNet/IP CIP** | Allen-Bradley CompactLogix, ControlLogix | 44818 | Program upload, status |
| **ADS/AMS** | Beckhoff TwinCAT 2/3, CX series | 48898 | Program transfer, boot project |

### 5.2 PLC Programming Commands

#### `plc_upload`
Upload program to external PLC.

```json
// Request
{
  "command": "plc_upload",
  "params": {
    "protocol": "s7",
    "address": "192.168.1.100",
    "port": 102,
    "rack": 0,
    "slot": 1,
    "program": {
      "name": "MainProgram",
      "language": "st",
      "source": "VAR counter : INT := 0; END_VAR counter := counter + 1;",
      "variables": [],
      "function_blocks": []
    }
  }
}

// Response
{
  "success": true,
  "program_name": "MainProgram",
  "program_id": "OB1",
  "warnings": [],
  "timestamp": "2026-01-19T12:00:00Z"
}
```

#### `plc_status`
Get PLC connection status and run mode.

```json
// Request
{
  "command": "plc_status",
  "params": {
    "protocol": "s7",
    "address": "192.168.1.100",
    "rack": 0,
    "slot": 1
  }
}

// Response
{
  "connected": true,
  "run_mode": "Run",
  "model": "S7-1200",
  "firmware": "4.5.0",
  "current_program": "MainProgram",
  "last_modified": "2026-01-19T10:00:00Z"
}
```

#### `plc_start` / `plc_stop`
Start or stop PLC execution.

```json
// Request
{
  "command": "plc_start",
  "params": {
    "protocol": "s7",
    "address": "192.168.1.100",
    "rack": 0,
    "slot": 1
  }
}

// Response
{ "success": true, "action": "started" }
```

#### `plc_list`
List programs on PLC.

```json
// Response
{
  "programs": ["OB1", "FB1", "DB1"],
  "count": 3
}
```

#### `plc_download`
Download program from PLC.

```json
// Request
{
  "command": "plc_download",
  "params": {
    "protocol": "s7",
    "address": "192.168.1.100",
    "program_name": "OB1"
  }
}
```

#### `plc_delete`
Delete program from PLC.

```json
// Request
{
  "command": "plc_delete",
  "params": {
    "protocol": "s7",
    "address": "192.168.1.100",
    "program_name": "OB1"
  }
}
```

### 5.3 Protocol-Specific Parameters

#### Codesys Gateway
```json
{
  "protocol": "codesys",
  "address": "192.168.1.100",
  "port": 1217,
  "device_name": "Device1",
  "application": "Application",
  "username": "admin",
  "password": "secret",
  "encrypted": true
}
```

#### Siemens S7comm
```json
{
  "protocol": "s7",
  "address": "192.168.1.100",
  "port": 102,
  "rack": 0,
  "slot": 1
}
```

#### OPC UA
```json
{
  "protocol": "opcua",
  "address": "192.168.1.100",
  "port": 4840,
  "username": "admin",
  "password": "secret",
  "client_cert_path": "/path/to/cert.pem",
  "client_key_path": "/path/to/key.pem"
}
```

#### Allen-Bradley EtherNet/IP
```json
{
  "protocol": "ethernet_ip",
  "address": "192.168.1.100",
  "port": 44818,
  "slot": 0,
  "connection_path": "1/0/2/192.168.1.200"
}
```

#### Beckhoff ADS
```json
{
  "protocol": "ads",
  "address": "192.168.1.100",
  "port": 48898,
  "ams_net_id": "192.168.1.100.1.1",
  "target_ams_port": 851
}
```

### 5.4 IEC 61131-3 Program Languages

| Language | Code | Description |
|----------|------|-------------|
| Structured Text | `st` | Pascal-like programming |
| Ladder Diagram | `ld` | Relay logic graphical |
| Function Block Diagram | `fbd` | Graphical blocks |
| Instruction List | `il` | Assembly-like |
| Sequential Function Chart | `sfc` | State machine |

### 5.5 Data Types (IEC 61131-3)

| Type | Size | Range |
|------|------|-------|
| BOOL | 1 byte | TRUE/FALSE |
| BYTE | 1 byte | 0..255 |
| SINT | 1 byte | -128..127 |
| INT | 2 bytes | -32768..32767 |
| DINT | 4 bytes | -2³¹..2³¹-1 |
| REAL | 4 bytes | IEEE 754 float |
| LREAL | 8 bytes | IEEE 754 double |
| TIME | 4 bytes | Duration |
| STRING | 256 bytes | Text |

### 5.6 Security Considerations

- **Authentication**: Use protocol-specific credentials
- **Network**: Ensure proper firewall rules for PLC ports
- **Audit**: All program uploads are logged with timestamp and user
- **IEC 62443**: Compliance for industrial network security
- **No Hardcoded Credentials**: Anonymous login when credentials not configured (with warning)

### 5.7 Protocol Implementation Notes (v1.3.2)

#### Codesys V3 Gateway
- Header structure: 16 bytes (magic[4] + length[4] + service_id[2] + reserved[2] + payload_len[4])
- Response parsing uses bytes 12-15 for payload length
- Warnings collected during upload are properly returned in response
- **v1.3.3**: Anonymous login allowed but logs security warning with IEC 62443 compliance note
- **v1.3.3**: Connection properly rolled back on login failure (prevents resource leak)

#### Siemens S7comm
- Start CPU uses `P_PROGRAM` parameter
- Stop CPU uses `_STOP` parameter
- Supports S7-300/400/1200/1500 series

#### OPC UA
- IPv6 addresses supported with RFC 3986 bracket notation: `opc.tcp://[::1]:4840`
- IPv4 and hostnames use standard `host:port` format
- **v1.3.2**: Secure channel response parsing now correctly handles responses of 12+ bytes (was incorrectly requiring 17+ bytes)

#### EtherNet/IP CIP
- Connection path format: `slot/connection_path`
- Assembly instance uploads for program transfer
- **v1.3.2**: Tag names are now validated to not exceed 255 bytes (CIP symbolic segment limit)

#### ADS/AMS
- AMS Net ID format: `x.x.x.x.x.x` (6 octets)
- Boot project deployment via ADS file transfer

### 5.8 Security Hardening (v1.3.2)

All PLC protocols include memory exhaustion protection:

| Protocol | Max Packet Size | Validation |
|----------|-----------------|------------|
| OPC UA | 16 MB | Message size field |
| S7comm | 65536 bytes | PDU size |
| EtherNet/IP | 65536 bytes | Data length |
| ADS/AMS | 1 MB | AMS header |
| Codesys | 65536 bytes | Payload length |

---

## 6. Scripting Engine

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
| `emergency` | 255 | Highest priority, system override |
| `critical` | 200 | Safety-critical |
| `high` | 100 | Important |
| `normal` | 50 | Default |
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

| Type | Source Format | Example |
|------|---------------|---------|
| `sensor` | `sensor:register_name` | `sensor:temperature` |
| `gpio` | `gpio:pin_number` | `gpio:17` |
| `variable` | `var:variable_name` | `var:counter` |
| `time` | `time:component` | `time:hour`, `time:minute`, `time:weekday` |
| `system` | `system:metric` | `system:uptime` |

#### Time Condition

Check time-based conditions:

```json
{
  "type": "time",
  "source": "time:hour",
  "operator": "between",
  "value": [8, 17]
}
```

| Time Source | Range | Description |
|-------------|-------|-------------|
| `time:hour` | 0-23 | Hour of day |
| `time:minute` | 0-59 | Minute of hour |
| `time:second` | 0-59 | Second of minute |
| `time:weekday` | 0-6 | Day of week (0=Sunday) |
| `time:day` | 1-31 | Day of month |
| `time:month` | 1-12 | Month of year |

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

#### `noop`
No operation - useful for conditional skipping.
```json
{
  "type": "noop"
}
```

### 5.7 Action Conditions

Actions can have conditions to control execution:

```json
{
  "type": "set_gpio",
  "target": "17",
  "value": true,
  "condition": {
    "source": "var:override",
    "operator": "eq",
    "value": false
  }
}
```

### 5.8 Alert Levels

| Level | Description |
|-------|-------------|
| `info` | Informational |
| `warning` | Warning (default) |
| `error` | Error condition |
| `critical` | Critical alert |

### 5.9 Variable Interpolation

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

### 5.9 Script Context Management (v1.3.2)

When scripts call other scripts via `call_script` action, the engine properly manages execution context:

- **Context Preservation**: Previous script ID and priority are saved before nested calls
- **Context Restoration**: On early return (e.g., conditions not met), context is properly restored
- **Nested Call Safety**: Prevents context corruption in deeply nested script chains

This ensures reliable behavior when using complex script hierarchies with conditional execution.

---

## 7. Function Blocks (IEC 61131-3)

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

Counts up on each rising edge of CU input. Q becomes TRUE when CV >= PV.

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

| Input | Type | Description |
|-------|------|-------------|
| CU | bool | Count up (rising edge) |
| R | bool | Reset (sets CV to 0) |
| PV | i32 | Preset value |

| Output | Type | Description |
|--------|------|-------------|
| Q | bool | CV >= PV |
| CV | i32 | Current count |

#### CTD (Count Down)

Counts down on each rising edge of CD input. Q becomes TRUE when CV <= 0.

```json
{
  "id": "countdown",
  "type": "CTD",
  "params": {
    "pv": 10
  },
  "inputs": {
    "CD": "gpio:18",
    "LD": "var:load"
  },
  "outputs": {
    "Q": "var:done",
    "CV": "var:remaining"
  }
}
```

| Input | Type | Description |
|-------|------|-------------|
| CD | bool | Count down (rising edge) |
| LD | bool | Load (sets CV to PV) |
| PV | i32 | Preset value |

| Output | Type | Description |
|--------|------|-------------|
| Q | bool | CV <= 0 |
| CV | i32 | Current count |

#### CTUD (Count Up/Down)

Bidirectional counter with separate up and down inputs.

```json
{
  "id": "bidirectional",
  "type": "CTUD",
  "params": {
    "pv": 100
  },
  "inputs": {
    "CU": "gpio:17",
    "CD": "gpio:18",
    "R": "var:reset",
    "LD": "var:load"
  },
  "outputs": {
    "QU": "var:upper_limit",
    "QD": "var:lower_limit",
    "CV": "var:count"
  }
}
```

| Input | Type | Description |
|-------|------|-------------|
| CU | bool | Count up (rising edge) |
| CD | bool | Count down (rising edge) |
| R | bool | Reset (sets CV to 0) |
| LD | bool | Load (sets CV to PV) |
| PV | i32 | Preset value |

| Output | Type | Description |
|--------|------|-------------|
| QU | bool | CV >= PV (upper limit) |
| QD | bool | CV <= 0 (lower limit) |
| CV | i32 | Current count |

### 6.3 Edge & Flip-Flop Blocks

#### R_TRIG (Rising Edge)
#### F_TRIG (Falling Edge)
#### SR (Set-Reset, Set dominant)
#### RS (Reset-Set, Reset dominant)

### 6.4 Controller Blocks

#### PID Controller

Standard PID controller with anti-windup and output limiting.

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

| Input | Type | Description |
|-------|------|-------------|
| SP | f64 | Setpoint (desired value) |
| PV | f64 | Process Variable (measured value) |
| KP | f64 | Proportional gain |
| KI | f64 | Integral gain (1/s) |
| KD | f64 | Derivative gain (s) |
| OUT_MIN | f64 | Minimum output limit |
| OUT_MAX | f64 | Maximum output limit |
| MANUAL | bool | Manual mode enable |
| MAN_OUT | f64 | Manual output value |
| RESET | bool | Reset integrator |

| Output | Type | Description |
|--------|------|-------------|
| OUT | f64 | Controller output |
| ERROR | f64 | Current error (SP - PV) |
| P_TERM | f64 | Proportional term |
| I_TERM | f64 | Integral term |
| D_TERM | f64 | Derivative term |
| SATURATED | bool | Output is at limit |

#### MAVG (Moving Average Filter)

Smooths noisy sensor readings with configurable window.

```json
{
  "id": "smooth_temp",
  "type": "MAVG",
  "params": {
    "n": 10
  },
  "inputs": {
    "IN": "sensor:raw_temperature"
  },
  "outputs": {
    "OUT": "sensor:filtered_temperature",
    "VALID": "var:filter_ready"
  }
}
```

| Input | Type | Description |
|-------|------|-------------|
| IN | f64 | Input value to filter |
| N | u32 | Window size (1-1000, default: 10) |
| RESET | bool | Clear the filter buffer |

| Output | Type | Description |
|--------|------|-------------|
| OUT | f64 | Filtered output (moving average) |
| VALID | bool | True when buffer is full |
| COUNT | u32 | Current samples in buffer |

#### HYSTERESIS (Schmitt Trigger)

Prevents oscillation around a setpoint.

```json
{
  "id": "thermostat",
  "type": "HYSTERESIS",
  "params": {
    "high": 25.0,
    "low": 22.0
  },
  "inputs": {
    "IN": "sensor:temperature"
  },
  "outputs": {
    "OUT": "var:cooling_on"
  }
}
```

| Input | Type | Description |
|-------|------|-------------|
| IN | f64 | Input value |
| HIGH | f64 | High threshold (turn on) |
| LOW | f64 | Low threshold (turn off) |

| Output | Type | Description |
|--------|------|-------------|
| OUT | bool | Output state |

Behavior:
- When OFF: turns ON when IN > HIGH
- When ON: turns OFF when IN < LOW
- Stays unchanged when LOW <= IN <= HIGH

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

## 8. Alarm Management (IEC 62682)

Alarm management following IEC 62682 standard for industrial automation.

> **Note**: Alarm management is an **internal API** only. Alarm functions are not exposed as remote MQTT commands. They are used internally by the scripting engine and can be accessed programmatically.

### 7.1 Alarm Priority Levels

| Priority | Value | Description |
|----------|-------|-------------|
| `diagnostic` | 0 | Informational only, no action required |
| `low` | 1 | Action required within extended timeframe |
| `medium` | 2 | Action required within normal timeframe |
| `high` | 3 | Immediate action required |
| `critical` | 4 | Emergency, safety-related |

### 7.2 Alarm States

| State | Description |
|-------|-------------|
| `Normal` | Alarm condition not present |
| `Active` | Alarm condition present, not acknowledged |
| `Acknowledged` | Alarm condition present, operator acknowledged |
| `ReturnedUnack` | Condition cleared but not acknowledged |
| `Shelved` | Temporarily suppressed by operator |
| `Suppressed` | Suppressed by design (maintenance) |
| `OutOfService` | Alarm point disabled |

### 7.3 Alarm Types

| Type | Description |
|------|-------------|
| `High` | High limit exceeded |
| `HighHigh` | Critical high limit exceeded |
| `Low` | Low limit exceeded |
| `LowLow` | Critical low limit exceeded |
| `Deviation` | Deviation from setpoint |
| `RateOfChange` | Rate of change exceeded |
| `Digital` | Digital state change |
| `Fault` | Equipment fault |
| `Communication` | Communication failure |

### 7.4 Alarm Configuration

```yaml
alarms:
  - id: "temp_high"
    name: "Temperature High"
    description: "Water temperature exceeded safe limit"
    alarmType: "high"
    priority: "high"
    source: "sensor:water_temp"
    setpoint: 85.0
    deadband: 2.0
    delayMs: 1000
    enabled: true
    requireAck: true
```

### 7.5 Dead-band Support

Dead-band prevents alarm chatter around the setpoint:
- Alarm activates when value exceeds setpoint
- Alarm clears when value drops below (setpoint - deadband)

### 7.6 Alarm Events

| Event | Description |
|-------|-------------|
| `Activated` | Alarm became active |
| `Acknowledged` | Operator acknowledged |
| `Returned` | Returned to normal (unacknowledged) |
| `Cleared` | Fully cleared (acknowledged + returned) |
| `Reactivated` | Condition returned while unacknowledged |
| `Shelved` | Temporarily suppressed |
| `Unshelved` | Removed from shelf |

### 7.7 Alarm Journal

Alarm history is maintained with maximum 1000 entries.

```json
{
  "timestamp": "2026-01-19T12:00:00.123Z",
  "event": {
    "type": "activated",
    "alarm_id": "temp_high",
    "value": 86.5,
    "priority": "high"
  },
  "alarm_name": "Temperature High",
  "priority": "high"
}
```

---

## 9. Backup & Restore

Backup and restore functionality for disaster recovery (IEC 62443 SL2 FR7 compliance).

> **Note**: Backup/Restore is an **internal API** only. These functions are not exposed as remote MQTT commands. Backups are triggered internally (e.g., on deploy) or via local system access.

### 8.1 Backup Contents

| Item | Description |
|------|-------------|
| Configuration | YAML configuration (sanitized) |
| Scripts | All script definitions |
| FB States | Function block states |
| Variables | Persisted variables |
| Triggers | Trigger states |

### 8.2 Backup File Format

- Extension: `.sdb` (Suderra Database Backup)
- Magic header: `SUDERRA\x00`
- Compression: gzip
- Max size: 100 MB

### 8.3 Backup Configuration

```yaml
backup:
  directory: "/var/lib/suderra/backups"
  maxBackups: 10
  autoBackupOnDeploy: true
```

### 8.4 Backup Commands

#### Create Backup
```json
{
  "command": "create_backup",
  "params": {
    "description": "Pre-upgrade backup"
  }
}
```

Response:
```json
{
  "success": true,
  "result": {
    "path": "/var/lib/suderra/backups/backup_20260119_120000_123.sdb",
    "size_bytes": 524288
  }
}
```

#### List Backups
```json
{
  "command": "list_backups"
}
```

Response:
```json
{
  "success": true,
  "result": {
    "backups": [
      {
        "path": "/var/lib/suderra/backups/backup_20260119_120000_123.sdb",
        "size_bytes": 524288,
        "created_at": "2026-01-19T12:00:00.123Z",
        "agent_version": "1.2.6",
        "description": "Pre-upgrade backup",
        "script_count": 5,
        "variable_count": 12
      }
    ]
  }
}
```

#### Restore Backup
```json
{
  "command": "restore_backup",
  "params": {
    "path": "/var/lib/suderra/backups/backup_20260119_120000_123.sdb",
    "verifyDeviceId": true
  }
}
```

### 8.5 Device ID Verification

Backups include the device ID. On restore:
- `verifyDeviceId: true` - Fails if device ID doesn't match
- `verifyDeviceId: false` - Allows cross-device restore

### 8.6 Rolling Backups

When `maxBackups` is exceeded, oldest backups are automatically deleted.

---

## 10. Offline Queue

### 9.1 Priority Levels

| Priority | Value | Use Case |
|----------|-------|----------|
| Critical | 3 | Alarms, safety |
| High | 2 | Important events |
| Normal | 1 | Regular telemetry |
| Low | 0 | Background data |

### 9.2 Configuration

| Parameter | Default | Range |
|-----------|---------|-------|
| Max Size | 1000 msgs | 100-1000000 |
| Max Age | 3600s | - |
| Max Disk | 50 MB | 1 MB-unlimited |

### 9.3 Behavior

- **Enqueue**: Priority-based eviction when full
- **Dequeue**: Priority DESC, FIFO within priority
- **Disk Limit**: Auto-evict when 80%+ usage
- **Persistence**: SQLite with WAL mode

### 9.4 Statistics

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

### 9.5 Advanced Operations

| Operation | Description |
|-----------|-------------|
| `vacuum()` | Reclaim disk space, returns freed bytes |
| `vacuum_if_needed()` | Auto-vacuum when 80%+ disk used |
| `backup_to(path)` | Backup queue to file |
| `backup_rolling(dir, max)` | Rolling backup with retention |
| `integrity_check()` | Full database integrity verification |
| `quick_check()` | Fast sanity check |
| `nack(id)` | Negative acknowledge (requeue message) |
| `peek_batch(count)` | Peek multiple messages |
| `ack_batch(ids)` | Acknowledge multiple messages |

### 9.6 Async Support

`AsyncOfflineQueue` wrapper provides Tokio-compatible async API for all operations.

---

## 11. Resilience Patterns

### 10.1 Circuit Breaker

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

### 10.2 Rate Limiting

| Resource | Limit | Window |
|----------|-------|--------|
| Commands | 60 | 60s |
| Modbus ops | 10/s | Burst: 20 |
| Scripts | Per-script | 60s |

### 10.3 Retry Logic

| Operation | Max Retries | Backoff |
|-----------|-------------|---------|
| MQTT Channel | 3 | 10ms exp |
| GPIO | 3 | 10/20/40ms |
| Modbus | Circuit breaker | - |

---

## 12. MQTT Broker Failover (v1.3.4)

High availability support through automatic failover to backup MQTT broker.

### 12.1 Configuration

```yaml
mqtt:
  broker: "mqtt-primary.example.com"
  port: 8883
  failover:
    enabled: true
    backup_broker: "mqtt-backup.example.com"
    backup_port: 8883              # Optional, defaults to primary port
    timeout_secs: 10               # Time before failover triggers
    health_check_interval_secs: 60 # How often to check if primary is back
    max_failures: 3                # Consecutive failures before failover
    recovery_delay_secs: 5         # Delay before switching back to primary
```

### 12.2 Failover States

| State | Description |
|-------|-------------|
| `PRIMARY_ACTIVE` | Connected to primary broker (normal operation) |
| `CONNECTING_TO_BACKUP` | Primary failed, connecting to backup |
| `BACKUP_ACTIVE` | Connected to backup broker |
| `CHECKING_PRIMARY` | On backup, checking if primary is back |
| `SWITCHING_TO_PRIMARY` | Transitioning from backup to primary |
| `DISCONNECTED` | Both brokers unavailable |

### 12.3 State Machine

```
┌──────────────┐  connect fail   ┌───────────────┐
│   PRIMARY    │ ───────────────▶│  CONNECTING   │
│   ACTIVE     │                 │  TO BACKUP    │
└──────▲───────┘                 └───────┬───────┘
       │                                 │
       │ primary                         │ backup
       │ recovered                       │ connected
       │                                 ▼
┌──────┴───────┐  health check   ┌───────────────┐
│   CHECKING   │ ◀───────────────│    BACKUP     │
│   PRIMARY    │   (periodic)    │    ACTIVE     │
└──────────────┘                 └───────────────┘
```

### 12.4 Failover Commands

#### `failover_status`

Get current failover state and configuration.

**Request:**
```json
{
  "command_id": "cmd_123",
  "command": "failover_status",
  "params": {},
  "timestamp": "2026-01-20T12:00:00Z"
}
```

**Response (failover enabled):**
```json
{
  "command_id": "cmd_123",
  "device_id": "device-abc",
  "success": true,
  "result": {
    "enabled": true,
    "primary_broker": "mqtt-primary.example.com:8883",
    "backup_broker": "mqtt-backup.example.com:8883",
    "config": {
      "timeout_secs": 10,
      "health_check_interval_secs": 60,
      "max_failures": 3,
      "recovery_delay_secs": 5
    }
  },
  "timestamp": "2026-01-20T12:00:01Z"
}
```

**Response (failover disabled):**
```json
{
  "command_id": "cmd_123",
  "device_id": "device-abc",
  "success": true,
  "result": {
    "enabled": false,
    "message": "Failover is not enabled. Configure mqtt.failover in config.yaml"
  },
  "timestamp": "2026-01-20T12:00:01Z"
}
```

#### `failover_force`

Manually trigger failover to backup broker.

**Request:**
```json
{
  "command_id": "cmd_124",
  "command": "failover_force",
  "params": {},
  "timestamp": "2026-01-20T12:00:00Z"
}
```

**Response:**
```json
{
  "command_id": "cmd_124",
  "device_id": "device-abc",
  "success": true,
  "result": {
    "action": "failover_initiated",
    "target": "mqtt-backup.example.com",
    "message": "Failover to backup broker has been initiated"
  },
  "timestamp": "2026-01-20T12:00:01Z"
}
```

#### `failover_recover`

Manually trigger recovery to primary broker.

**Request:**
```json
{
  "command_id": "cmd_125",
  "command": "failover_recover",
  "params": {},
  "timestamp": "2026-01-20T12:00:00Z"
}
```

**Response:**
```json
{
  "command_id": "cmd_125",
  "device_id": "device-abc",
  "success": true,
  "result": {
    "action": "recovery_initiated",
    "target": "mqtt-primary.example.com",
    "message": "Recovery to primary broker has been initiated"
  },
  "timestamp": "2026-01-20T12:00:01Z"
}
```

### 12.5 Default Values

| Parameter | Default | Description |
|-----------|---------|-------------|
| `enabled` | `false` | Failover disabled by default |
| `backup_port` | Same as primary | Backup broker port |
| `timeout_secs` | `10` | Seconds before triggering failover |
| `health_check_interval_secs` | `60` | How often to check primary |
| `max_failures` | `3` | Failures before failover |
| `recovery_delay_secs` | `5` | Delay before switching back |

### 12.6 Offline Queue Integration

When both brokers are unavailable:
- Messages are stored in SQLite-backed offline queue
- Queue is flushed when any broker becomes available
- Priority ordering preserved (QoS 1 > QoS 0)
- No message loss guaranteed

---

## 13. HTTP Health API

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

Comprehensive system diagnostics including all component details:

```json
{
  "system": {
    "os": "Linux",
    "os_version": "6.1.0-rpi",
    "architecture": "aarch64",
    "cpu_cores": 4,
    "total_memory_mb": 8192,
    "load_average": [0.5, 0.4, 0.3]
  },
  "disk": {
    "total_bytes": 32000000000,
    "free_bytes": 28000000000,
    "usage_percent": 12.5
  },
  "process": {
    "pid": 12345,
    "threads": 8,
    "memory_rss_mb": 45,
    "uptime_secs": 3600
  },
  "mqtt": {
    "connected": true,
    "messages_sent": 1000,
    "messages_received": 500,
    "circuit_breaker_state": "CLOSED"
  },
  "modbus": {
    "plc1": {
      "connected": true,
      "circuit_breaker_state": "CLOSED",
      "read_count": 1500,
      "write_count": 200,
      "error_count": 0
    }
  },
  "scripts": {
    "loaded_count": 5,
    "active_count": 3,
    "execution_count": 12500,
    "error_count": 0
  },
  "function_blocks": {
    "instance_count": 12,
    "types": {"TON": 3, "PID": 2, "CTU": 4, "MAVG": 3}
  },
  "offline_queue": {
    "size": 0,
    "capacity": 10000,
    "db_size_bytes": 1048576
  },
  "config": {
    "device_id": "xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx",
    "telemetry_interval_secs": 60,
    "modbus_device_count": 2,
    "gpio_pin_count": 5
  },
  "recent_errors": [
    {
      "timestamp": "2026-01-19T12:00:00Z",
      "component": "modbus",
      "message": "Connection timeout"
    }
  ]
}
```

---

## 13. Security

### 12.1 TLS/mTLS

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

### 12.2 Credential Protection

- Secrets stored with `secrecy` crate
- Auto-zeroized on drop
- Masked in logs: `[REDACTED]`
- File permissions: 0600 required

### 12.3 Input Validation

| Field | Validation |
|-------|------------|
| Device ID | UUID format |
| API URL | http/https, valid host |
| GPIO Pin | Platform range |
| Modbus Slave | 1-247 |
| Telemetry Interval | 5-3600s |

### 12.4 IEC 62443 SL2 Compliance

| Requirement | Implementation |
|-------------|----------------|
| FR1 Access Control | Device ID, UUID validation |
| FR3 Whitelisting | Modbus FC whitelist |
| FR4 Confidentiality | TLS 1.2+, mTLS |
| FR5 Availability | Bounded queue, circuit breaker |
| FR6 Monitoring | Health API, diagnostics |

---

## 14. Configuration

### 13.1 Config File Path

1. `$SUDERRA_CONFIG` environment variable
2. `/etc/suderra/config.yaml`
3. `./config.yaml`

### 13.2 Additional Configuration Options

#### Telemetry Options

| Parameter | Default | Description |
|-----------|---------|-------------|
| `includeSystem` | true | Include system metrics (uptime, load) |

#### Cache Configuration

```yaml
cache:
  maxCapacity: 10000
  ttlSecs: 300
  ttiSecs: 60
```

| Parameter | Default | Description |
|-----------|---------|-------------|
| `maxCapacity` | 10000 | Maximum cache entries |
| `ttlSecs` | 300 | Time-to-live (0 = no TTL) |
| `ttiSecs` | 60 | Time-to-idle (0 = no TTI) |

#### Circuit Breaker Configuration

```yaml
circuitBreaker:
  failureThreshold: 3
  successThreshold: 2
  recoverySecs: 30
  halfOpenPermits: 1
```

| Parameter | Default | Description |
|-----------|---------|-------------|
| `failureThreshold` | 3 | Failures before opening circuit |
| `successThreshold` | 2 | Successes to close circuit |
| `recoverySecs` | 30 | Wait time before recovery |
| `halfOpenPermits` | 1 | Max concurrent in half-open |

#### OpenTelemetry (OTLP) Configuration

```yaml
telemetry:
  otlp:
    endpoint: "http://localhost:4317"
    serviceName: "suderra-agent"
    sampleRatio: 1.0
```

| Parameter | Default | Description |
|-----------|---------|-------------|
| `endpoint` | - | OTLP endpoint URL |
| `serviceName` | suderra-agent | Service name for traces |
| `sampleRatio` | 1.0 | Trace sample ratio (0.0-1.0) |

#### Runtime Timeouts

```yaml
runtime:
  gpioTimeoutSecs: 5
  modbusTimeoutSecs: 5
  modbusConnectTimeoutSecs: 5
  provisioningTimeoutSecs: 30
  shutdownTimeoutSecs: 10
  mqttReconnectMinSecs: 1
  mqttReconnectMaxSecs: 60
```

#### Scripting Limits

```yaml
scripting:
  minScanCycleMs: 10
  maxScanCycleMs: 10000
```

#### GPIO Options

```yaml
gpio:
  - name: "button1"
    pin: 18
    direction: "input"
    debounceMs: 50
```

#### Modbus TLS Options

```yaml
modbus:
  - tls:
      serverName: "plc.local"
      insecureSkipVerify: false
```

### 13.3 Full Configuration Schema

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

## 15. Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `SUDERRA_CONFIG` | - | Path to configuration file |
| `SUDERRA_DATA_DIR` | `/var/lib/suderra` | Directory for program state, backups, scripts |
| `SUDERRA_LOG_LEVEL` | `info` | Log level (trace, debug, info, warn, error) |
| `RUST_LOG` | - | Alternative log level (tracing crate) |

### 14.1 Data Directory Structure

```
/var/lib/suderra/
├── state.json           # Program state persistence
├── backups/             # Backup files (.sdb)
├── scripts/             # Persisted scripts
├── variables/           # Persisted variables
└── offline_queue.db     # SQLite offline queue
```

---

## 16. Feature Flags

Cargo features that enable optional functionality:

| Feature | Description | Default |
|---------|-------------|---------|
| `gpio` | GPIO support (Linux only, requires rppal) | disabled |
| `health` | HTTP health check endpoint (/health, /ready, /metrics) | disabled |
| `telemetry` | OpenTelemetry OTLP tracing export | disabled |
| `metrics` | Prometheus metrics export at /metrics | disabled |
| `strict-security` | Additional security checks for production | disabled |

### 15.1 Build Examples

```bash
# Basic build
cargo build --release

# With GPIO and health endpoint
cargo build --release --features "gpio,health"

# Full featured build
cargo build --release --features "gpio,health,telemetry,metrics"

# Production with strict security
cargo build --release --features "gpio,health,strict-security"
```

### 15.2 Feature Dependencies

- `metrics` requires `health` feature
- `gpio` only works on Linux with rppal-compatible hardware
- `strict-security` enables additional runtime checks

---

## 17. Error Types

### 16.1 ModbusError

Granular Modbus error types (v1.2.0):

| Error | Description | Recoverable |
|-------|-------------|-------------|
| `Connection` | Connection failed | Yes |
| `ConnectionTimeout` | Connection timeout (ms) | Yes |
| `OperationTimeout` | Operation timeout (ms) | Yes |
| `InvalidSlaveId` | Invalid Modbus slave ID (1-247) | No |
| `FunctionCodeNotAllowed` | Security violation (IEC 62443 FR3) | No |
| `RegisterOutOfRange` | Register address out of range | No |
| `RegisterCountExceeded` | Register count exceeds limit | No |
| `ChecksumError` | CRC/LRC validation failed | Yes |
| `ModbusException` | Modbus exception response | Depends |
| `RateLimited` | Rate limit exceeded (IEC 62443 FR5) | Yes |
| `CircuitBreakerOpen` | Circuit breaker is open | Yes |
| `WriteNotAllowed` | Write operations not allowed | No |
| `NotConnected` | Device not connected | Yes |
| `DeviceNotFound` | Modbus device not found | No |
| `SerialPort` | Serial port error (RTU) | Yes |
| `Protocol` | Generic protocol error | Depends |

### 16.2 AgentError

Top-level agent errors:

| Error | Description |
|-------|-------------|
| `Modbus` | Modbus operation error |
| `Mqtt` | MQTT connection/publish error |
| `Persistence` | Database/SQLite error |
| `Http` | HTTP client error (provisioning) |
| `Io` | I/O operation error |
| `Config` | Configuration error |
| `TokenExpired` | Provisioning token expired |
| `TokenAlreadyUsed` | Provisioning token already used |
| `DeviceDecommissioned` | Device has been decommissioned |
| `Timeout` | Operation timeout |
| `RateLimited` | Rate limit exceeded |

### 16.3 Script Errors

| Error | Description |
|-------|-------------|
| `ExecutionDepthExceeded` | Max call depth exceeded |
| `ExecutionTimeExceeded` | Max execution time exceeded |
| `ActionLimitExceeded` | Max actions per cycle exceeded |
| `InvalidCondition` | Invalid condition syntax |
| `InvalidAction` | Invalid action configuration |
| `VariableNotFound` | Referenced variable not found |
| `FunctionBlockError` | Function block execution error |

### 16.4 Error Response Format

Command errors return structured responses:

```json
{
  "command_id": "cmd_123",
  "device_id": "device-uuid",
  "success": false,
  "error": {
    "code": "MODBUS_TIMEOUT",
    "message": "Operation timeout after 5000ms",
    "recoverable": true,
    "details": {
      "device": "plc1",
      "register": 100,
      "timeout_ms": 5000
    }
  },
  "timestamp": "2026-01-19T12:00:00Z"
}
```

---

## 18. Provisioning

### 17.1 Activation Flow

```
1. Agent starts with provisioning token
2. Collect device fingerprint
3. POST to /api/devices/activate
4. Receive MQTT credentials
5. Store credentials (0600 perms)
6. Connect to MQTT
7. Publish online status
```

### 17.2 Activation Request

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

### 17.3 Activation Response

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

## 19. Limits & Defaults

### Core Limits

| Parameter | Default | Min | Max |
|-----------|---------|-----|-----|
| Telemetry Interval | 30s | 5s | 3600s |
| MQTT Keep-alive | 30s | 1s | 3600s |
| MQTT Channel | 500 msgs | - | - |
| MQTT Reconnect Min | 1s | - | - |
| MQTT Reconnect Max | 60s | - | - |
| Scan Cycle | 100ms | 10ms | 10000ms |
| Script Max Depth | 10 | 1 | 1000 |
| Script Max Actions | 100 | 1 | 10000 |
| Script Max Time | 30s | 1s | 300s |
| Circuit Breaker Recovery | 30s | 1s | 3600s |
| Circuit Breaker Failures | 3 | 1 | 100 |
| Command Rate Limit | 60/min | 1 | 1000 |
| Modbus Rate Limit | 10/s | 1 | 100 |
| Modbus Timeout | 5s | 1s | 60s |
| GPIO Timeout | 5s | 1s | 60s |
| Provisioning Timeout | 30s | 5s | 300s |
| Shutdown Timeout | 10s | 1s | 60s |

### Queue & Storage

| Parameter | Default | Min | Max |
|-----------|---------|-----|-----|
| Offline Queue Max | 1000 | 100 | 1000000 |
| Offline Queue Disk | 50 MB | 1 MB | - |
| Max FBs | 100 | 1 | 1000 |
| Max Backups | 10 | 1 | 100 |
| Backup Max Size | 100 MB | - | - |
| Alarm Journal Max | 1000 | 10 | 10000 |

### Cache

| Parameter | Default | Min | Max |
|-----------|---------|-----|-----|
| Cache Max Capacity | 10000 | 100 | 1000000 |
| Cache TTL | 300s | 0 | - |
| Cache TTI | 60s | 0 | - |

### Hardware Interfaces

| Parameter | Default | Min | Max |
|-----------|---------|-----|-----|
| I2C Clock Speed | 100kHz | 100kHz | 400kHz |
| I2C Address Range | - | 0x03 | 0x77 |
| PWM Frequency | 1kHz | 1Hz | 100kHz |
| PWM Duty Cycle | 0.0 | 0.0 | 1.0 |
| SPI Clock Speed | 1MHz | 100kHz | 32MHz |
| MAVG Window Size | 10 | 1 | 1000 |

### Priority Values

| Priority Level | Script | Offline Queue | Alarm |
|----------------|--------|---------------|-------|
| Emergency | 255 | - | - |
| Critical | 200 | 3 | 4 |
| High | 100 | 2 | 3 |
| Normal | 50 | 1 | 2 |
| Low | 0 | 0 | 1 |
| Diagnostic | - | - | 0 |

---

*Generated by Suderra AS - v1.3.3*
