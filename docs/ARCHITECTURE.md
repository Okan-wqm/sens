# Suderra Edge Agent Architecture v2.0

## Overview

The Suderra Edge Agent is a Rust-based industrial IoT agent designed for aquaculture monitoring and control systems. It runs on Linux-based edge devices (Raspberry Pi, Revolution Pi) and provides:

- Zero-touch device provisioning
- MQTT-based cloud communication
- Modbus TCP/RTU for PLC and sensor integration
- GPIO control for digital I/O
- Edge scripting for local automation
- System telemetry collection
- **v2.0: Circuit breaker for fault tolerance**
- **v2.0: Graceful shutdown coordination**
- **v2.0: Script execution limits (infinite loop protection)**
- **v2.0: Script conflict detection (GPIO/Modbus write conflicts)**

## Architecture Diagram (v2.0)

```
┌─────────────────────────────────────────────────────────────────────┐
│                         SUDERRA AGENT v2.0                           │
│                                                                      │
│  ┌────────────────────────────────────────────────────────────────┐  │
│  │                     Shared State (Granular)                     │  │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐         │  │
│  │  │ AgentConfig  │  │ MqttClient   │  │Arc<RwLock<>> │         │  │
│  │  │  (immutable) │  │  (optional)  │  │(activation)  │         │  │
│  │  └──────────────┘  └──────────────┘  └──────────────┘         │  │
│  └────────────────────────────────────────────────────────────────┘  │
│                                                                      │
│  ┌─────────────────────────────────────────────────────────────┐    │
│  │                   tokio::task::LocalSet                      │    │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐       │    │
│  │  │ ModbusActor  │  │  GpioActor   │  │ ScriptEngine │       │    │
│  │  │ +CircuitBrkr │  │  (v2.0)      │  │ +Limits      │       │    │
│  │  │ +Timeout     │  │  +Timeout    │  │ +RateLimiter │       │    │
│  │  └──────────────┘  └──────────────┘  └──────────────┘       │    │
│  └─────────────────────────────────────────────────────────────┘    │
│                                                                      │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐              │
│  │  Telemetry   │  │   Command    │  │    MQTT      │              │
│  │  Collector   │  │   Handler    │  │   Client     │──────────────┼──▶ Cloud
│  └──────────────┘  └──────────────┘  └──────────────┘              │
│                                                                      │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐              │
│  │  Resilience  │  │   Shutdown   │  │   Health     │              │
│  │  (CircuitBkr)│  │ Coordinator  │  │  (planned)   │              │
│  └──────────────┘  └──────────────┘  └──────────────┘              │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

## Core Design Patterns

### 1. Actor Pattern for Modbus and GPIO (v2.0)

Both Modbus and GPIO use the **Actor Pattern** to handle non-Send types:

```
┌─────────────────────────┐      ┌─────────────────────────┐
│     Other Components    │      │   Hardware Actors       │
│  (TelemetryCollector,   │      │  (run in LocalSet)      │
│   CommandHandler, etc.) │      │                         │
│                         │      │  ┌─────────────────┐    │
│  ┌─────────────────┐    │      │  │  ModbusActor    │    │
│  │  ModbusHandle   │◀───┼─mpsc─┼──│  +CircuitBreaker│    │
│  │  (Send+Sync)    │    │      │  └─────────────────┘    │
│  └─────────────────┘    │      │                         │
│                         │      │  ┌─────────────────┐    │
│  ┌─────────────────┐    │      │  │   GpioActor     │    │
│  │  GpioHandle     │◀───┼─mpsc─┼──│   (v2.0)        │    │
│  │  (Send+Sync)    │    │      │  └─────────────────┘    │
│  └─────────────────┘    │      │                         │
└─────────────────────────┘      └─────────────────────────┘
```

**ModbusHandle Commands:**
```rust
enum ModbusCommand {
    ConnectAll { response: oneshot::Sender<Vec<String>> },
    DisconnectAll { response: oneshot::Sender<()> },
    ReadAll { response: oneshot::Sender<Vec<ModbusReadResult>> },
    WriteRegister { device, address, value, response },
    WriteCoil { device, address, value, response },
    DeviceCount { response: oneshot::Sender<usize> },
}
```

**GpioHandle Commands (v2.0):**
```rust
enum GpioCommand {
    Init { response: oneshot::Sender<Result<()>> },
    ReadAll { response: oneshot::Sender<GpioReadResult> },
    ReadPin { pin: u8, response: oneshot::Sender<Result<PinState>> },
    WritePin { pin: u8, value: bool, response: oneshot::Sender<Result<()>> },
    GetPinCount { response: oneshot::Sender<usize> },
    IsAvailable { response: oneshot::Sender<bool> },
}
```

### 2. Circuit Breaker Pattern (v2.0)

Protects against cascading failures in Modbus communication:

```rust
pub struct CircuitBreaker {
    name: String,
    state: AtomicU8,        // 0=Closed, 1=Open, 2=HalfOpen
    failure_count: AtomicU32,
    failure_threshold: u32,  // Default: 3
    recovery_timeout: Duration,  // Default: 30s
    last_failure: RwLock<Option<Instant>>,
}
```

**States:**
- **Closed**: Normal operation, requests pass through
- **Open**: Requests blocked, circuit "tripped" after threshold failures
- **HalfOpen**: After recovery timeout, allow one test request

**Configuration:**
```rust
const CIRCUIT_BREAKER_THRESHOLD: u32 = 3;
const CIRCUIT_BREAKER_RECOVERY: Duration = Duration::from_secs(30);
```

### 3. Timeout Wrapper (v2.0)

All hardware operations are wrapped with timeouts:

```rust
const MODBUS_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const GPIO_TIMEOUT: Duration = Duration::from_secs(5);

pub async fn with_timeout<T, F>(
    operation: F,
    timeout: Duration,
    operation_name: &str,
) -> Result<T, TimeoutError>
```

### 4. Script Execution Limits (v2.0)

Prevents runaway scripts and infinite loops:

```rust
pub struct ScriptLimits {
    pub max_execution_time: Duration,    // Default: 30s
    pub max_actions_per_run: usize,      // Default: 50
    pub max_call_depth: usize,           // Default: 5 (infinite loop protection)
    pub max_delay_ms: u64,               // Default: 60000ms
    pub rate_limit_per_minute: usize,    // Default: 60
}
```

**Execution Context:**
```rust
pub struct ExecutionContext {
    pub start_time: Instant,
    pub call_depth: usize,
    pub actions_executed: usize,
    pub limits: ScriptLimits,
}
```

**Rate Limiter:**
```rust
pub struct ScriptRateLimiter {
    windows: RwLock<HashMap<String, RateLimitWindow>>,
    default_limit: usize,
}
```

### 5. Script Conflict Detection with Priority (v2.0)

Detects when multiple scripts attempt to write different values to the same GPIO pin or Modbus register. Uses script priority for conflict resolution:

```rust
pub struct PendingWrite {
    pub script_id: String,
    pub priority: u8,  // Script priority for conflict resolution
    pub value: WriteValue,
}

pub struct ConflictDetector {
    gpio_writes: HashMap<u8, PendingWrite>,
    modbus_writes: HashMap<(String, u16), PendingWrite>,
    coil_writes: HashMap<(String, u16), PendingWrite>,
}

pub enum ConflictResult {
    NoConflict,                      // Proceed with write
    ConflictWon { message: String }, // Higher priority wins, proceed
    ConflictLost { message: String }, // Lower priority, BLOCKED
    Duplicate,                        // Same value, skip redundant write
}
```

**Priority Levels:**
```rust
pub enum ScriptPriority {
    Low = 0,        // Runs last
    Normal = 50,    // Default
    High = 100,     // Runs before normal scripts
    Critical = 200, // Runs first, wins most conflicts
    Emergency = 255, // Absolute highest, for safety scripts
}
```

**Behavior:**
- **NoConflict**: First script to write a value proceeds normally
- **ConflictWon**: Higher priority script overrides lower priority → proceed with write
- **ConflictLost**: Lower priority script blocked → action fails, existing value preserved
- **Duplicate**: Same value from different scripts → write skipped for efficiency
- **Same Priority**: Last-write-wins (original behavior)

**Example Logs:**
```
WARN: GPIO CONFLICT WON: Pin 17 - Script 'emergency_shutdown' (priority 255)
      overrides 'cooling_control' (priority 50): HIGH -> LOW

WARN: GPIO CONFLICT LOST: Pin 17 - Script 'cooling_control' (priority 50)
      blocked by 'emergency_shutdown' (priority 255): keeping LOW
```

### 6. Graceful Shutdown (v2.0)

Coordinated shutdown sequence:

```rust
pub struct ShutdownCoordinator {
    notify: broadcast::Sender<()>,
    tasks: Vec<(&'static str, JoinHandle<()>)>,
}
```

**Shutdown Sequence:**
1. Signal all tasks to stop (broadcast)
2. Wait for in-flight operations (with timeout)
3. Flush offline buffer (if implemented)
4. Disconnect hardware interfaces
5. Publish offline status
6. Disconnect MQTT

### 7. Endianness Support (v2.0)

Supports different PLC byte orders for 32-bit values:

```rust
pub enum ByteOrder {
    BigEndian,           // AB CD - Siemens S7, standard Modbus
    LittleEndian,        // CD AB - Schneider PLCs
    BigEndianByteSwap,   // BA DC - Some devices
    LittleEndianByteSwap, // DC BA - Special cases
}
```

**Configuration:**
```yaml
modbus:
  - name: "PLC-1"
    registers:
      - name: "flow_rate"
        address: 100
        data_type: "f32"
        byte_order: "little_endian"  # For Schneider PLC
```

### 8. Arc-Based Register Sharing (v2.0)

Registers are shared via Arc to avoid cloning on every read:

```rust
pub struct ModbusClient {
    config: ModbusDeviceConfig,
    registers: Arc<Vec<ModbusRegisterConfig>>,  // Shared, not cloned
    ctx: Option<client::Context>,
    circuit_breaker: CircuitBreaker,
}
```

## Module Structure (v2.0)

```
edge-agent/
├── src/
│   ├── main.rs              # Entry point, runtime setup
│   ├── config.rs            # Configuration + ByteOrder enum
│   ├── error.rs             # Error types
│   ├── provisioning.rs      # Device activation
│   ├── mqtt.rs              # MQTT client wrapper
│   ├── telemetry.rs         # System metrics (uses GpioHandle)
│   ├── commands.rs          # Remote command handling
│   ├── modbus.rs            # Modbus + Actor + CircuitBreaker
│   ├── gpio.rs              # GPIO Actor Pattern (v2.0)
│   ├── shutdown.rs          # Graceful shutdown coordinator (v2.0)
│   ├── resilience/          # Resilience patterns (v2.0)
│   │   ├── mod.rs
│   │   ├── circuit_breaker.rs
│   │   └── timeout.rs
│   └── scripting/
│       ├── mod.rs           # Module exports
│       ├── storage.rs       # Script persistence
│       ├── context.rs       # Execution context
│       ├── triggers.rs      # Trigger evaluation
│       ├── actions.rs       # Action definitions
│       ├── limits.rs        # Execution limits (v2.0)
│       ├── conflict.rs      # Conflict detection (v2.0)
│       └── engine.rs        # Script engine + depth tracking + conflicts
```

## Thread Safety Model (v2.0)

| Component | Thread Safety | Access Pattern |
|-----------|--------------|----------------|
| AppState | Arc<RwLock<T>> | Shared, short locks |
| ModbusHandle | Send + Sync | Clone freely |
| GpioHandle | Send + Sync | Clone freely (v2.0) |
| ModbusManager | !Send | Actor-owned only |
| GpioActor | !Send | Actor-owned only (v2.0) |
| MqttClient | Send + Sync | Via AppState lock |
| ScriptEngine | !Send | Single task + limits |
| CircuitBreaker | Send + Sync | Atomic operations |
| ScriptRateLimiter | Send + Sync | RwLock per-script |
| ConflictDetector | !Send | ScriptEngine-owned, reset per cycle |

## Configuration (v2.0)

```yaml
# /etc/suderra/config.yaml
device_id: "edge-12345678"
device_code: "ABC123"
api_url: "https://api.example.com"

mqtt:
  broker: "mqtt.example.com"
  port: 1883
  username: "device_12345678"
  password: "secret"
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
      - name: "water_temp"
        address: 100
        register_type: "holding"
        data_type: "f32"
        byte_order: "big_endian"   # v2.0: Endianness support
        scale: 0.1
        unit: "°C"

gpio:
  - name: "pump_status"
    pin: 17
    direction: "input"
    pull: "up"
    invert: false

scripting:
  enabled: true
  scripts_dir: "/etc/suderra/scripts"
```

## Build Targets

| Target | Architecture | Use Case |
|--------|-------------|----------|
| x86_64-unknown-linux-gnu | x86-64 | Development, VMs |
| aarch64-unknown-linux-gnu | ARM64 | Raspberry Pi 4/5 |
| armv7-unknown-linux-gnueabihf | ARMv7 | Raspberry Pi 3 |

## Dependencies (v2.0)

| Crate | Purpose |
|-------|---------|
| tokio | Async runtime |
| rumqttc | MQTT client |
| tokio-modbus | Modbus TCP/RTU |
| rppal | GPIO (Linux ARM) |
| sysinfo | System metrics |
| serde/serde_yaml | Config parsing |
| tracing | Logging |
| chrono | Time handling |
| anyhow | Error handling |

## Error Handling (v2.0)

- All errors use `anyhow::Result` for rich context
- **Circuit breaker** prevents cascading Modbus failures
- **Timeouts** on all hardware operations
- **Rate limiting** prevents script abuse
- **Depth tracking** prevents infinite recursion in scripts
- **Conflict detection** warns when scripts write conflicting values
- MQTT reconnection with exponential backoff
- Graceful shutdown on SIGTERM/SIGINT

## Security Considerations

1. **Config File Permissions**: `/etc/suderra/config.yaml` should be `chmod 600`
2. **MQTT Credentials**: Stored in config after provisioning, per-device unique
3. **Token-Based Provisioning**: Single-use tokens, 24h expiry
4. **No Arbitrary Shell Commands**: Only whitelisted commands executed
5. **Tenant Isolation**: MQTT topics prefixed with tenant ID
6. **Script Sandboxing**: Execution limits prevent resource exhaustion
