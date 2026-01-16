//! Modbus TCP/RTU client for PLC communication
//!
//! Supports reading/writing Modbus registers from industrial PLCs
//! and sensor controllers.
//!
//! Uses actor pattern to isolate non-Send Modbus client types.
//! Components communicate with the actor via channels.
//!
//! Features:
//! - Circuit breaker for fault tolerance
//! - Timeouts on all operations
//! - Parallel device reads

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio_modbus::client::tcp;
use tokio_modbus::prelude::*;
use tracing::{debug, error, info, warn};

use crate::config::{ByteOrder, ModbusDeviceConfig, ModbusRegisterConfig};
use crate::resilience::{with_timeout, CircuitBreaker};

/// Default timeout for Modbus operations
const MODBUS_TIMEOUT: Duration = Duration::from_secs(5);
/// Default connection timeout
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Circuit breaker failure threshold
const CIRCUIT_BREAKER_THRESHOLD: u32 = 3;
/// Circuit breaker recovery timeout
const CIRCUIT_BREAKER_RECOVERY: Duration = Duration::from_secs(30);

// ============================================================================
// Actor Pattern Types
// ============================================================================

/// Commands sent to the Modbus actor
#[derive(Debug)]
pub enum ModbusCommand {
    /// Connect to all configured devices
    ConnectAll {
        response: oneshot::Sender<Vec<String>>,
    },
    /// Disconnect all devices
    DisconnectAll { response: oneshot::Sender<()> },
    /// Read all registers from all devices
    ReadAll {
        response: oneshot::Sender<Vec<ModbusReadResult>>,
    },
    /// Write a register value
    WriteRegister {
        device_name: String,
        address: u16,
        value: u16,
        response: oneshot::Sender<Result<()>>,
    },
    /// Write a coil value
    WriteCoil {
        device_name: String,
        address: u16,
        value: bool,
        response: oneshot::Sender<Result<()>>,
    },
    /// Get device count
    DeviceCount { response: oneshot::Sender<usize> },
}

/// Thread-safe handle to communicate with the Modbus actor
#[derive(Clone)]
pub struct ModbusHandle {
    sender: mpsc::Sender<ModbusCommand>,
}

impl ModbusHandle {
    /// Create a new handle and spawn the actor
    pub fn new(configs: Vec<ModbusDeviceConfig>) -> Self {
        let (sender, receiver) = mpsc::channel(32);

        // Spawn the actor in a local task (will be run via LocalSet)
        tokio::task::spawn_local(async move {
            let mut actor = ModbusActor::new(configs, receiver);
            actor.run().await;
        });

        Self { sender }
    }

    /// Connect to all devices
    pub async fn connect_all(&self) -> Vec<String> {
        let (tx, rx) = oneshot::channel();
        let _ = self
            .sender
            .send(ModbusCommand::ConnectAll { response: tx })
            .await;
        rx.await
            .unwrap_or_else(|_| vec!["Actor disconnected".to_string()])
    }

    /// Disconnect all devices
    pub async fn disconnect_all(&self) {
        let (tx, rx) = oneshot::channel();
        let _ = self
            .sender
            .send(ModbusCommand::DisconnectAll { response: tx })
            .await;
        let _ = rx.await;
    }

    /// Read all registers from all devices
    pub async fn read_all(&self) -> Vec<ModbusReadResult> {
        let (tx, rx) = oneshot::channel();
        let _ = self
            .sender
            .send(ModbusCommand::ReadAll { response: tx })
            .await;
        rx.await.unwrap_or_default()
    }

    /// Write a register value
    pub async fn write_register(&self, device_name: &str, address: u16, value: u16) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let _ = self
            .sender
            .send(ModbusCommand::WriteRegister {
                device_name: device_name.to_string(),
                address,
                value,
                response: tx,
            })
            .await;
        rx.await
            .map_err(|_| anyhow::anyhow!("Actor disconnected"))?
    }

    /// Write a coil value
    pub async fn write_coil(&self, device_name: &str, address: u16, value: bool) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let _ = self
            .sender
            .send(ModbusCommand::WriteCoil {
                device_name: device_name.to_string(),
                address,
                value,
                response: tx,
            })
            .await;
        rx.await
            .map_err(|_| anyhow::anyhow!("Actor disconnected"))?
    }

    /// Get device count
    pub async fn device_count(&self) -> usize {
        let (tx, rx) = oneshot::channel();
        let _ = self
            .sender
            .send(ModbusCommand::DeviceCount { response: tx })
            .await;
        rx.await.unwrap_or(0)
    }
}

/// Modbus actor that owns the non-Send client types
struct ModbusActor {
    manager: ModbusManager,
    receiver: mpsc::Receiver<ModbusCommand>,
}

impl ModbusActor {
    fn new(configs: Vec<ModbusDeviceConfig>, receiver: mpsc::Receiver<ModbusCommand>) -> Self {
        Self {
            manager: ModbusManager::new(configs),
            receiver,
        }
    }

    async fn run(&mut self) {
        info!("Modbus actor started");

        while let Some(cmd) = self.receiver.recv().await {
            match cmd {
                ModbusCommand::ConnectAll { response } => {
                    let errors = self.manager.connect_all().await;
                    let _ = response.send(errors);
                }
                ModbusCommand::DisconnectAll { response } => {
                    self.manager.disconnect_all().await;
                    let _ = response.send(());
                }
                ModbusCommand::ReadAll { response } => {
                    let results = self.manager.read_all().await;
                    let _ = response.send(results);
                }
                ModbusCommand::WriteRegister {
                    device_name,
                    address,
                    value,
                    response,
                } => {
                    let result = if let Some(client) = self.manager.get_client(&device_name) {
                        client.write_register(address, value).await
                    } else {
                        Err(anyhow::anyhow!("Device not found: {}", device_name))
                    };
                    let _ = response.send(result);
                }
                ModbusCommand::WriteCoil {
                    device_name,
                    address,
                    value,
                    response,
                } => {
                    let result = if let Some(client) = self.manager.get_client(&device_name) {
                        client.write_coil(address, value).await
                    } else {
                        Err(anyhow::anyhow!("Device not found: {}", device_name))
                    };
                    let _ = response.send(result);
                }
                ModbusCommand::DeviceCount { response } => {
                    let _ = response.send(self.manager.device_count());
                }
            }
        }

        info!("Modbus actor stopped");
    }
}

// ============================================================================
// Core Modbus Types
// ============================================================================

/// Modbus client wrapper with circuit breaker
pub struct ModbusClient {
    /// Device configuration (without registers to avoid large clones)
    config: ModbusDeviceConfig,
    /// Register configurations (Arc to avoid cloning on every read)
    registers: Arc<Vec<ModbusRegisterConfig>>,
    /// Modbus context
    ctx: Option<client::Context>,
    /// Circuit breaker for fault tolerance
    circuit_breaker: CircuitBreaker,
}

/// Register value with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterValue {
    pub name: String,
    pub address: u16,
    pub raw_value: u16,
    pub scaled_value: f64,
    pub unit: Option<String>,
    pub timestamp: String,
}

/// Modbus read result
#[derive(Debug, Clone, Serialize)]
pub struct ModbusReadResult {
    pub device_name: String,
    pub values: Vec<RegisterValue>,
    pub errors: Vec<String>,
}

impl ModbusClient {
    /// Create a new Modbus client (not connected)
    pub fn new(config: ModbusDeviceConfig) -> Self {
        let circuit_breaker = CircuitBreaker::new(
            format!("modbus-{}", config.name),
            CIRCUIT_BREAKER_THRESHOLD,
            CIRCUIT_BREAKER_RECOVERY,
        );

        // Store registers in Arc to avoid cloning on every read
        let registers = Arc::new(config.registers.clone());

        Self {
            config,
            registers,
            ctx: None,
            circuit_breaker,
        }
    }

    /// Check if circuit breaker is open
    pub fn is_circuit_open(&self) -> bool {
        self.circuit_breaker.is_open()
    }

    /// Get circuit breaker state name
    pub fn circuit_state(&self) -> &'static str {
        self.circuit_breaker.state_name()
    }

    /// Connect to Modbus device with timeout
    pub async fn connect(&mut self) -> Result<()> {
        // Reset circuit breaker on reconnection attempt
        self.circuit_breaker.reset();

        let result = match self.config.connection_type.as_str() {
            "tcp" => with_timeout(
                self.connect_tcp_inner(),
                CONNECT_TIMEOUT,
                &format!("Modbus TCP connect {}", self.config.name),
            )
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?,
            "rtu" => self.connect_rtu().await,
            other => Err(anyhow::anyhow!("Unknown connection type: {}", other)),
        };

        match &result {
            Ok(_) => self.circuit_breaker.record_success(),
            Err(_) => self.circuit_breaker.record_failure(),
        }

        result
    }

    /// Connect via Modbus TCP (inner implementation)
    async fn connect_tcp_inner(&mut self) -> Result<()> {
        let addr: SocketAddr = self
            .config
            .address
            .parse()
            .with_context(|| format!("Invalid TCP address: {}", self.config.address))?;

        info!(
            "Connecting to Modbus TCP device '{}' at {}",
            self.config.name, addr
        );

        let ctx = tcp::connect_slave(addr, Slave(self.config.slave_id))
            .await
            .with_context(|| format!("Failed to connect to Modbus TCP at {}", addr))?;

        self.ctx = Some(ctx);
        info!("Connected to Modbus TCP device '{}'", self.config.name);

        Ok(())
    }

    /// Connect via Modbus RTU (serial)
    async fn connect_rtu(&mut self) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            use tokio_serial::SerialPortBuilderExt;

            let baud_rate = self.config.baud_rate.unwrap_or(9600);

            info!(
                "Connecting to Modbus RTU device '{}' at {} ({})",
                self.config.name, self.config.address, baud_rate
            );

            let builder = tokio_serial::new(&self.config.address, baud_rate);
            let port = builder
                .open_native_async()
                .with_context(|| format!("Failed to open serial port: {}", self.config.address))?;

            let ctx = tokio_modbus::client::rtu::attach_slave(port, Slave(self.config.slave_id));

            self.ctx = Some(ctx);
            info!("Connected to Modbus RTU device '{}'", self.config.name);

            Ok(())
        }

        #[cfg(not(target_os = "linux"))]
        {
            Err(anyhow::anyhow!("Modbus RTU not supported on this platform"))
        }
    }

    /// Disconnect from device
    pub async fn disconnect(&mut self) {
        self.ctx = None;
        info!("Disconnected from Modbus device '{}'", self.config.name);
    }

    /// Check if connected
    pub fn is_connected(&self) -> bool {
        self.ctx.is_some()
    }

    /// Read all configured registers with circuit breaker protection
    pub async fn read_all(&mut self) -> ModbusReadResult {
        let mut result = ModbusReadResult {
            device_name: self.config.name.clone(),
            values: Vec::new(),
            errors: Vec::new(),
        };

        // Check circuit breaker first
        if self.circuit_breaker.is_open() {
            result.errors.push(format!(
                "Circuit breaker open for device '{}' (state: {})",
                self.config.name,
                self.circuit_breaker.state_name()
            ));
            return result;
        }

        if self.ctx.is_none() {
            result.errors.push("Not connected".to_string());
            return result;
        }

        let mut had_failure = false;
        let mut had_success = false;

        // Use Arc reference instead of cloning (v2.0 optimization)
        let registers = Arc::clone(&self.registers);
        for register in registers.iter() {
            match self.read_register_with_timeout(register).await {
                Ok(value) => {
                    result.values.push(value);
                    had_success = true;
                }
                Err(e) => {
                    warn!("Failed to read register {}: {}", register.name, e);
                    result.errors.push(format!("{}: {}", register.name, e));
                    had_failure = true;
                }
            }
        }

        // Update circuit breaker based on overall result
        if had_failure && !had_success {
            self.circuit_breaker.record_failure();
        } else if had_success {
            self.circuit_breaker.record_success();
        }

        result
    }

    /// Read a register with timeout
    async fn read_register_with_timeout(
        &mut self,
        register: &ModbusRegisterConfig,
    ) -> Result<RegisterValue> {
        with_timeout(
            self.read_register(register),
            MODBUS_TIMEOUT,
            &format!("Modbus read {}", register.name),
        )
        .await
        .map_err(|e| anyhow::anyhow!("{}", e))?
    }

    /// Read a single register
    pub async fn read_register(
        &mut self,
        register: &ModbusRegisterConfig,
    ) -> Result<RegisterValue> {
        // Calculate register count before borrowing ctx to satisfy borrow checker
        let register_count = self.get_register_count(&register.data_type);

        let ctx = self
            .ctx
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Not connected"))?;

        let raw_values = match register.register_type.as_str() {
            "holding" => ctx
                .read_holding_registers(register.address, register_count)
                .await
                .context("Failed to read holding registers")?
                .map_err(|e| anyhow::anyhow!("Modbus exception: {:?}", e))?,
            "input" => ctx
                .read_input_registers(register.address, register_count)
                .await
                .context("Failed to read input registers")?
                .map_err(|e| anyhow::anyhow!("Modbus exception: {:?}", e))?,
            "coil" => {
                let coils = ctx
                    .read_coils(register.address, 1)
                    .await
                    .context("Failed to read coil")?
                    .map_err(|e| anyhow::anyhow!("Modbus exception: {:?}", e))?;
                vec![if coils.first().copied().unwrap_or(false) {
                    1
                } else {
                    0
                }]
            }
            "discrete" => {
                let inputs = ctx
                    .read_discrete_inputs(register.address, 1)
                    .await
                    .context("Failed to read discrete input")?
                    .map_err(|e| anyhow::anyhow!("Modbus exception: {:?}", e))?;
                vec![if inputs.first().copied().unwrap_or(false) {
                    1
                } else {
                    0
                }]
            }
            other => return Err(anyhow::anyhow!("Unknown register type: {}", other)),
        };

        // Convert raw value based on data type and byte order
        let raw_value = raw_values.first().copied().unwrap_or(0);
        let scaled_value =
            self.convert_value(&raw_values, &register.data_type, register.byte_order)
                * register.scale;

        debug!(
            "Read {}: raw={}, scaled={:.2}{}",
            register.name,
            raw_value,
            scaled_value,
            register.unit.as_deref().unwrap_or("")
        );

        Ok(RegisterValue {
            name: register.name.clone(),
            address: register.address,
            raw_value,
            scaled_value,
            unit: register.unit.clone(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        })
    }

    /// Write to a holding register
    pub async fn write_register(&mut self, address: u16, value: u16) -> Result<()> {
        let ctx = self
            .ctx
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Not connected"))?;

        ctx.write_single_register(address, value)
            .await
            .context("Failed to write register")?
            .map_err(|e| anyhow::anyhow!("Modbus exception: {:?}", e))?;

        debug!("Wrote value {} to register {}", value, address);
        Ok(())
    }

    /// Write to a coil
    pub async fn write_coil(&mut self, address: u16, value: bool) -> Result<()> {
        let ctx = self
            .ctx
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Not connected"))?;

        ctx.write_single_coil(address, value)
            .await
            .context("Failed to write coil")?
            .map_err(|e| anyhow::anyhow!("Modbus exception: {:?}", e))?;

        debug!("Wrote value {} to coil {}", value, address);
        Ok(())
    }

    /// Get register count for data type
    fn get_register_count(&self, data_type: &str) -> u16 {
        match data_type {
            "u32" | "i32" | "f32" => 2,
            "u64" | "i64" | "f64" => 4,
            _ => 1, // u16, i16
        }
    }

    /// Convert raw register values to f64 with byte order support
    ///
    /// Supported byte orders for 32-bit values:
    /// - BigEndian: AB CD (registers as-is, most significant first)
    /// - LittleEndian: CD AB (swap register order)
    /// - BigEndianByteSwap: BA DC (swap bytes within each register)
    /// - LittleEndianByteSwap: DC BA (swap both)
    fn convert_value(&self, values: &[u16], data_type: &str, byte_order: ByteOrder) -> f64 {
        match data_type {
            "u16" => values.first().copied().unwrap_or(0) as f64,
            "i16" => values.first().copied().unwrap_or(0) as i16 as f64,
            "u32" if values.len() >= 2 => {
                let bits = self.combine_u32(values[0], values[1], byte_order);
                bits as f64
            }
            "i32" if values.len() >= 2 => {
                let bits = self.combine_u32(values[0], values[1], byte_order);
                (bits as i32) as f64
            }
            "f32" if values.len() >= 2 => {
                let bits = self.combine_u32(values[0], values[1], byte_order);
                f32::from_bits(bits) as f64
            }
            _ => values.first().copied().unwrap_or(0) as f64,
        }
    }

    /// Combine two u16 registers into u32 based on byte order
    ///
    /// Different PLCs use different byte orders:
    /// - Siemens S7: BigEndian (AB CD)
    /// - Schneider: LittleEndian (CD AB)
    /// - Some devices: BigEndianByteSwap (BA DC)
    fn combine_u32(&self, reg_a: u16, reg_b: u16, byte_order: ByteOrder) -> u32 {
        match byte_order {
            ByteOrder::BigEndian => {
                // AB CD - standard Modbus, most significant word first
                ((reg_a as u32) << 16) | (reg_b as u32)
            }
            ByteOrder::LittleEndian => {
                // CD AB - swap register order
                ((reg_b as u32) << 16) | (reg_a as u32)
            }
            ByteOrder::BigEndianByteSwap => {
                // BA DC - swap bytes within each register
                let a_swapped = reg_a.swap_bytes();
                let b_swapped = reg_b.swap_bytes();
                ((a_swapped as u32) << 16) | (b_swapped as u32)
            }
            ByteOrder::LittleEndianByteSwap => {
                // DC BA - swap register order AND swap bytes
                let a_swapped = reg_a.swap_bytes();
                let b_swapped = reg_b.swap_bytes();
                ((b_swapped as u32) << 16) | (a_swapped as u32)
            }
        }
    }
}

/// Modbus device manager - handles multiple Modbus connections
pub struct ModbusManager {
    clients: Vec<ModbusClient>,
}

impl ModbusManager {
    /// Create a new Modbus manager
    pub fn new(configs: Vec<ModbusDeviceConfig>) -> Self {
        let clients = configs.into_iter().map(ModbusClient::new).collect();

        Self { clients }
    }

    /// Connect all devices (sequential - Modbus connections shouldn't be parallel)
    pub async fn connect_all(&mut self) -> Vec<String> {
        let mut errors = Vec::new();

        for client in &mut self.clients {
            if let Err(e) = client.connect().await {
                error!("Failed to connect to '{}': {}", client.config.name, e);
                errors.push(format!("{}: {}", client.config.name, e));
            }
        }

        errors
    }

    /// Disconnect all devices
    pub async fn disconnect_all(&mut self) {
        for client in &mut self.clients {
            client.disconnect().await;
        }
    }

    /// Read all registers from all devices (sequential)
    ///
    /// Note: We use sequential reads because:
    /// 1. Modbus connections are shared mutable state
    /// 2. Each client needs mutable access for read operations
    /// 3. The actor pattern already provides safe concurrent access from outside
    pub async fn read_all(&mut self) -> Vec<ModbusReadResult> {
        let mut results = Vec::new();

        for client in &mut self.clients {
            // Use per-device timeout
            let result = tokio::time::timeout(
                Duration::from_secs(10), // Per-device timeout
                client.read_all(),
            )
            .await;

            match result {
                Ok(r) => results.push(r),
                Err(_) => {
                    results.push(ModbusReadResult {
                        device_name: client.config.name.clone(),
                        values: vec![],
                        errors: vec!["Device read timeout".to_string()],
                    });
                    // Record failure in circuit breaker
                    client.circuit_breaker.record_failure();
                }
            }
        }

        results
    }

    /// Get client by device name
    pub fn get_client(&mut self, name: &str) -> Option<&mut ModbusClient> {
        self.clients.iter_mut().find(|c| c.config.name == name)
    }

    /// Get device count
    pub fn device_count(&self) -> usize {
        self.clients.len()
    }

    /// Get circuit breaker status for all devices
    pub fn circuit_status(&self) -> Vec<(&str, &'static str)> {
        self.clients
            .iter()
            .map(|c| (c.config.name.as_str(), c.circuit_state()))
            .collect()
    }

    /// Reconfigure with new device configs (hot-reload)
    pub async fn reconfigure(&mut self, new_configs: Vec<ModbusDeviceConfig>) {
        info!(
            "Reconfiguring Modbus manager with {} devices",
            new_configs.len()
        );

        // Disconnect existing clients
        self.disconnect_all().await;

        // Create new clients
        self.clients = new_configs.into_iter().map(ModbusClient::new).collect();

        // Connect new clients
        let errors = self.connect_all().await;
        if !errors.is_empty() {
            warn!(
                "Some Modbus devices failed to connect during reconfigure: {:?}",
                errors
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_value_serialization() {
        let value = RegisterValue {
            name: "temperature".to_string(),
            address: 100,
            raw_value: 2500,
            scaled_value: 25.0,
            unit: Some("°C".to_string()),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
        };

        let json = serde_json::to_string(&value).unwrap();
        assert!(json.contains("temperature"));
        assert!(json.contains("25.0"));
    }

    #[test]
    fn test_convert_u32_big_endian() {
        let client = ModbusClient::new(ModbusDeviceConfig {
            name: "test".to_string(),
            connection_type: "tcp".to_string(),
            address: "127.0.0.1:502".to_string(),
            slave_id: 1,
            baud_rate: None,
            registers: vec![],
        });

        // Test u32 BigEndian: 0x0001 0x0000 = 65536 (AB CD)
        let values = vec![0x0001, 0x0000];
        let result = client.convert_value(&values, "u32", ByteOrder::BigEndian);
        assert_eq!(result, 65536.0);
    }

    #[test]
    fn test_convert_u32_little_endian() {
        let client = ModbusClient::new(ModbusDeviceConfig {
            name: "test".to_string(),
            connection_type: "tcp".to_string(),
            address: "127.0.0.1:502".to_string(),
            slave_id: 1,
            baud_rate: None,
            registers: vec![],
        });

        // Test u32 LittleEndian: 0x0001 0x0000 = 1 (CD AB = 0x0000 0x0001)
        let values = vec![0x0001, 0x0000];
        let result = client.convert_value(&values, "u32", ByteOrder::LittleEndian);
        assert_eq!(result, 1.0);
    }

    #[test]
    fn test_byte_order_combinations() {
        let client = ModbusClient::new(ModbusDeviceConfig {
            name: "test".to_string(),
            connection_type: "tcp".to_string(),
            address: "127.0.0.1:502".to_string(),
            slave_id: 1,
            baud_rate: None,
            registers: vec![],
        });

        // Test with 0x1234 0x5678
        let values = vec![0x1234, 0x5678];

        // BigEndian: 0x12345678
        let be = client.combine_u32(values[0], values[1], ByteOrder::BigEndian);
        assert_eq!(be, 0x12345678);

        // LittleEndian: 0x56781234
        let le = client.combine_u32(values[0], values[1], ByteOrder::LittleEndian);
        assert_eq!(le, 0x56781234);

        // BigEndianByteSwap: 0x34127856
        let bebs = client.combine_u32(values[0], values[1], ByteOrder::BigEndianByteSwap);
        assert_eq!(bebs, 0x34127856);

        // LittleEndianByteSwap: 0x78563412
        let lebs = client.combine_u32(values[0], values[1], ByteOrder::LittleEndianByteSwap);
        assert_eq!(lebs, 0x78563412);
    }
}
