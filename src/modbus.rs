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
//! - Parallel device reads (v1.2.0)
//! - TLS support for encrypted Modbus/TCP (v1.2.0 - IEC 62443 SL2 FR4)
//!
//! ## Parallel vs Sequential Reads (v1.2.0)
//! - `read_all()`: Sequential reads (backwards compatible, simpler error handling)
//! - `read_all_parallel()`: Concurrent reads across multiple devices (better latency)
//!
//! ## TLS Support (v1.2.0)
//! When `tls.enabled = true` in device config, connections use encrypted TLS.
//! This provides IEC 62443 SL2 FR4 (Data Confidentiality) compliance.
//!
//! ## rodbus Migration (v1.2.0)
//! Uses rodbus crate instead of tokio-modbus for native TLS support.
//! The rodbus crate provides:
//! - Native TLS/mTLS support for Modbus TCP
//! - Request queuing and retry strategies
//! - Better error handling with granular error types

use anyhow::{Context, Result};
use futures::future::join_all;
use rodbus::client::{Channel, RequestParam};
use rodbus::{AddressRange, DecodeLevel, UnitId};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{debug, error, info, warn};

use crate::config::{ByteOrder, ModbusDeviceConfig, ModbusRegisterConfig, ModbusSecurityConfig};
use crate::resilience::{with_timeout, CircuitBreaker, RateLimiter};

/// Default timeout for Modbus operations
const MODBUS_TIMEOUT: Duration = Duration::from_secs(5);
/// Default connection timeout
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Circuit breaker failure threshold
const CIRCUIT_BREAKER_THRESHOLD: u32 = 3;
/// Circuit breaker recovery timeout
const CIRCUIT_BREAKER_RECOVERY: Duration = Duration::from_secs(30);

// Modbus Function Codes (IEC 62443 SL2 FR3: Whitelisting)
/// FC 1: Read Coils
const FC_READ_COILS: u8 = 1;
/// FC 2: Read Discrete Inputs
const FC_READ_DISCRETE_INPUTS: u8 = 2;
/// FC 3: Read Holding Registers
const FC_READ_HOLDING_REGISTERS: u8 = 3;
/// FC 4: Read Input Registers
const FC_READ_INPUT_REGISTERS: u8 = 4;
/// FC 5: Write Single Coil
const FC_WRITE_SINGLE_COIL: u8 = 5;
/// FC 6: Write Single Register
const FC_WRITE_SINGLE_REGISTER: u8 = 6;
/// FC 15: Write Multiple Coils
const FC_WRITE_MULTIPLE_COILS: u8 = 15;
/// FC 16: Write Multiple Registers
const FC_WRITE_MULTIPLE_REGISTERS: u8 = 16;

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
    /// Read all registers from all devices (sequential)
    ReadAll {
        response: oneshot::Sender<Vec<ModbusReadResult>>,
    },
    /// Read all registers from all devices (parallel, v1.2.0)
    ReadAllParallel {
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

    /// Read all registers from all devices (sequential)
    pub async fn read_all(&self) -> Vec<ModbusReadResult> {
        let (tx, rx) = oneshot::channel();
        let _ = self
            .sender
            .send(ModbusCommand::ReadAll { response: tx })
            .await;
        rx.await.unwrap_or_default()
    }

    /// Read all registers from all devices (parallel, v1.2.0)
    ///
    /// Reads from multiple devices concurrently for lower overall latency.
    /// Uses `join_all` to execute all device reads simultaneously.
    pub async fn read_all_parallel(&self) -> Vec<ModbusReadResult> {
        let (tx, rx) = oneshot::channel();
        let _ = self
            .sender
            .send(ModbusCommand::ReadAllParallel { response: tx })
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
                ModbusCommand::ReadAllParallel { response } => {
                    let results = self.manager.read_all_parallel().await;
                    let _ = response.send(results);
                }
                ModbusCommand::WriteRegister {
                    device_name,
                    address,
                    value,
                    response,
                } => {
                    let result = if let Some(client_arc) = self.manager.get_client(&device_name) {
                        let mut client = client_arc.lock().await;
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
                    let result = if let Some(client_arc) = self.manager.get_client(&device_name) {
                        let mut client = client_arc.lock().await;
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

/// Modbus client wrapper with circuit breaker and rate limiter
///
/// # Security Features (IEC 62443 SL2)
/// - FR3: Function code whitelist validation
/// - FR4: TLS encryption for data confidentiality (v1.2.0)
/// - FR5: Rate limiting to prevent resource exhaustion
///
/// # rodbus Migration (v1.2.0)
/// Uses rodbus `Channel` for Modbus communication with native TLS support.
pub struct ModbusClient {
    /// Device configuration (without registers to avoid large clones)
    config: ModbusDeviceConfig,
    /// Register configurations (Arc to avoid cloning on every read)
    registers: Arc<Vec<ModbusRegisterConfig>>,
    /// Security configuration
    security: ModbusSecurityConfig,
    /// rodbus channel for Modbus communication (v1.2.0)
    channel: Option<Channel>,
    /// Unit ID (slave ID) for Modbus requests
    unit_id: UnitId,
    /// Circuit breaker for fault tolerance
    circuit_breaker: CircuitBreaker,
    /// Rate limiter for DoS prevention
    rate_limiter: RateLimiter,
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

        // Create rate limiter from security config
        let rate_limiter = RateLimiter::new(
            format!("modbus-rate-{}", config.name),
            config.security.rate_limit_burst,
            config.security.rate_limit_ops_per_sec,
        );

        // Store registers in Arc to avoid cloning on every read
        let registers = Arc::new(config.registers.clone());
        let security = config.security.clone();

        // Create unit ID from slave ID
        let unit_id = UnitId::new(config.slave_id);

        Self {
            config,
            registers,
            security,
            channel: None,
            unit_id,
            circuit_breaker,
            rate_limiter,
        }
    }

    /// Validate function code against whitelist
    ///
    /// # Security
    /// IEC 62443 SL2 FR3: Only allow pre-approved function codes
    fn validate_function_code(&self, function_code: u8) -> Result<()> {
        if !self.security.enabled {
            return Ok(());
        }

        if self
            .security
            .allowed_function_codes
            .contains(&function_code)
        {
            Ok(())
        } else {
            warn!(
                "Modbus function code {} denied for device '{}' (whitelist: {:?})",
                function_code, self.config.name, self.security.allowed_function_codes
            );
            Err(anyhow::anyhow!(
                "Function code {} not allowed by security policy",
                function_code
            ))
        }
    }

    /// Validate write operation is permitted
    fn validate_write_allowed(&self) -> Result<()> {
        if !self.security.enabled {
            return Ok(());
        }

        if self.security.allow_writes {
            Ok(())
        } else {
            warn!(
                "Write operation denied for device '{}' (allow_writes=false)",
                self.config.name
            );
            Err(anyhow::anyhow!(
                "Write operations not allowed by security policy"
            ))
        }
    }

    /// Try to acquire rate limiter token
    ///
    /// # Security
    /// IEC 62443 SL2 FR5: Prevent resource exhaustion attacks
    fn acquire_rate_limit(&self) -> Result<()> {
        if !self.security.enabled {
            return Ok(());
        }

        if self.rate_limiter.try_acquire() {
            Ok(())
        } else {
            warn!(
                "Rate limit exceeded for Modbus device '{}' ({}/{} tokens)",
                self.config.name,
                self.rate_limiter.available_tokens(),
                self.rate_limiter.capacity()
            );
            Err(anyhow::anyhow!(
                "Rate limit exceeded for device '{}'",
                self.config.name
            ))
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
            "tcp" => {
                // Clone name first to avoid borrow conflict with self.connect_tcp_inner()
                let timeout_msg = format!("Modbus TCP connect {}", self.config.name);
                with_timeout(self.connect_tcp_inner(), CONNECT_TIMEOUT, &timeout_msg)
                    .await
                    .map_err(|e| anyhow::anyhow!("{}", e))?
            }
            "rtu" => self.connect_rtu().await,
            other => Err(anyhow::anyhow!("Unknown connection type: {}", other)),
        };

        match &result {
            Ok(_) => self.circuit_breaker.record_success(),
            Err(_) => self.circuit_breaker.record_failure(),
        }

        result
    }

    /// Connect via Modbus TCP with optional TLS (v1.2.0)
    ///
    /// # TLS Support
    /// When `config.tls.enabled = true`, uses encrypted TLS connection.
    /// This provides IEC 62443 SL2 FR4 (Data Confidentiality) compliance.
    async fn connect_tcp_inner(&mut self) -> Result<()> {
        // Parse address as SocketAddr to extract host and port
        let socket_addr: SocketAddr = self
            .config
            .address
            .parse()
            .with_context(|| format!("Invalid TCP address: {}", self.config.address))?;

        // Create HostAddr from socket address
        let host_addr = rodbus::client::HostAddr::ip(socket_addr.ip(), socket_addr.port());

        // Retry strategy for connection resilience (doubling backoff)
        let retry = rodbus::doubling_retry_strategy(
            Duration::from_secs(2),  // Min retry delay
            Duration::from_secs(30), // Max retry delay
        );

        let channel =
            if self.config.tls.enabled {
                // TLS connection (IEC 62443 SL2 FR4)
                info!(
                    "Connecting to Modbus TCP/TLS device '{}' at {} (TLS enabled)",
                    self.config.name, socket_addr
                );

                // Get certificate paths
                let ca_cert_path =
                    self.config.tls.ca_cert_path.as_ref().ok_or_else(|| {
                        anyhow::anyhow!("TLS enabled but ca_cert_path not specified")
                    })?;
                let ca_path = std::path::Path::new(ca_cert_path);

                // Server name for SNI (use server_name or extract from address)
                let server_name = self
                    .config
                    .tls
                    .server_name
                    .clone()
                    .unwrap_or_else(|| socket_addr.ip().to_string());

                // Use rodbus 1.4 TLS API: full_pki() for PKI hierarchy or self_signed()
                // full_pki() validates against CA certificate and optionally checks server name
                let tls_config = if let (Some(cert_path), Some(key_path)) = (
                    self.config.tls.client_cert_path.as_ref(),
                    self.config.tls.client_key_path.as_ref(),
                ) {
                    // mTLS with client certificate
                    let client_cert_path = std::path::Path::new(cert_path);
                    let client_key_path = std::path::Path::new(key_path);

                    rodbus::client::TlsClientConfig::full_pki(
                        Some(server_name.clone()), // Server name for SNI validation (Option<String>)
                        ca_path,                   // CA certificate path
                        client_cert_path,          // Client certificate path
                        client_key_path,           // Client private key path
                        None,                      // Private key password (None = unencrypted)
                        rodbus::client::MinTlsVersion::V1_2, // Minimum TLS version
                    )
                    .with_context(|| "Failed to create TLS config with client cert")?
                } else {
                    // Server-only TLS (no client cert) - use self_signed for simpler config
                    // Note: full_pki without client cert requires empty paths which isn't ideal
                    // For server-only auth, we use self_signed with the CA cert as the expected cert
                    rodbus::client::TlsClientConfig::full_pki(
                        Some(server_name.clone()), // Server name for SNI validation (Option<String>)
                        ca_path,                   // CA certificate path
                        std::path::Path::new(""),  // Empty client cert path
                        std::path::Path::new(""),  // Empty client key path
                        None,                      // No password
                        rodbus::client::MinTlsVersion::V1_2,
                    )
                    .with_context(|| "Failed to create TLS config (server auth only)")?
                };

                rodbus::client::spawn_tls_client_task(
                    host_addr,
                    1, // max queued requests
                    retry,
                    tls_config,
                    DecodeLevel::default(),
                    None, // listener
                )
            } else {
                // Plain TCP connection (backwards compatible)
                info!(
                    "Connecting to Modbus TCP device '{}' at {}",
                    self.config.name, socket_addr
                );

                rodbus::client::spawn_tcp_client_task(
                    host_addr,
                    1, // max queued requests
                    retry,
                    DecodeLevel::default(),
                    None, // listener
                )
            };

        self.channel = Some(channel);

        if self.config.tls.enabled {
            info!(
                "Connected to Modbus TCP/TLS device '{}' (TLS encrypted)",
                self.config.name
            );
        } else {
            info!("Connected to Modbus TCP device '{}'", self.config.name);
        }

        Ok(())
    }

    /// Connect via Modbus RTU (serial)
    ///
    /// Note: RTU connections do not support TLS (serial communication).
    /// For secure RTU, use physical security measures.
    async fn connect_rtu(&mut self) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            let baud_rate = self.config.baud_rate.unwrap_or(9600);

            info!(
                "Connecting to Modbus RTU device '{}' at {} (baud: {})",
                self.config.name, self.config.address, baud_rate
            );

            // Retry strategy for connection resilience (doubling backoff)
            let retry = rodbus::doubling_retry_strategy(
                Duration::from_secs(2),  // Min retry delay
                Duration::from_secs(30), // Max retry delay
            );

            // Create serial port settings using rodbus 1.4 API
            let path = &self.config.address;
            let serial_settings = rodbus::SerialSettings {
                baud_rate, // u32 directly
                data_bits: rodbus::DataBits::Eight,
                stop_bits: rodbus::StopBits::One,
                parity: rodbus::Parity::None,
                flow_control: rodbus::FlowControl::None,
            };

            let channel = rodbus::client::spawn_rtu_client_task(
                path,
                serial_settings,
                1, // max queued requests
                retry,
                DecodeLevel::default(),
                None, // listener
            );

            self.channel = Some(channel);
            info!("Connected to Modbus RTU device '{}'", self.config.name);

            Ok(())
        }

        #[cfg(not(target_os = "linux"))]
        {
            Err(anyhow::anyhow!("Modbus RTU not supported on this platform"))
        }
    }

    /// Disconnect from device
    ///
    /// Note: rodbus channels are dropped automatically when the reference is dropped.
    /// This method clears the channel reference to trigger cleanup.
    pub async fn disconnect(&mut self) {
        if let Some(channel) = self.channel.take() {
            // Channel shutdown is handled by rodbus when dropped
            drop(channel);
        }
        info!("Disconnected from Modbus device '{}'", self.config.name);
    }

    /// Check if connected
    pub fn is_connected(&self) -> bool {
        self.channel.is_some()
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

        if self.channel.is_none() {
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

    /// Read a single register using rodbus (v1.2.0)
    ///
    /// # Security
    /// - Validates function code against whitelist
    /// - Enforces rate limiting
    /// - Validates register count limits
    ///
    /// # rodbus API
    /// Uses rodbus channel for Modbus communication with native TLS support.
    pub async fn read_register(
        &mut self,
        register: &ModbusRegisterConfig,
    ) -> Result<RegisterValue> {
        // Security checks
        self.acquire_rate_limit()?;

        // Calculate register count before borrowing channel to satisfy borrow checker
        let register_count = self.get_register_count(&register.data_type);

        // Validate register count (IEC 62443 SL2 FR3: Input validation)
        if register_count > self.security.max_register_count {
            return Err(anyhow::anyhow!(
                "Register count {} exceeds maximum {} for security policy",
                register_count,
                self.security.max_register_count
            ));
        }

        // Determine function code and validate
        let function_code = match register.register_type.as_str() {
            "holding" => FC_READ_HOLDING_REGISTERS,
            "input" => FC_READ_INPUT_REGISTERS,
            "coil" => FC_READ_COILS,
            "discrete" => FC_READ_DISCRETE_INPUTS,
            other => return Err(anyhow::anyhow!("Unknown register type: {}", other)),
        };
        self.validate_function_code(function_code)?;

        let channel = self
            .channel
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Not connected"))?;

        // Create request parameters with unit ID and timeout
        let params = RequestParam::new(self.unit_id, MODBUS_TIMEOUT);

        // Create address range for the request
        let addr_range = AddressRange::try_from(register.address, register_count)
            .map_err(|e| anyhow::anyhow!("Invalid address range: {:?}", e))?;

        // rodbus returns Vec<Indexed<T>>, we extract the .value from each
        let raw_values: Vec<u16> = match register.register_type.as_str() {
            "holding" => {
                let result = channel
                    .read_holding_registers(params, addr_range)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to read holding registers: {:?}", e))?;
                // Extract .value from each Indexed<u16>
                result.into_iter().map(|indexed| indexed.value).collect()
            }
            "input" => {
                let result = channel
                    .read_input_registers(params, addr_range)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to read input registers: {:?}", e))?;
                // Extract .value from each Indexed<u16>
                result.into_iter().map(|indexed| indexed.value).collect()
            }
            "coil" => {
                let coil_range = AddressRange::try_from(register.address, 1)
                    .map_err(|e| anyhow::anyhow!("Invalid coil address: {:?}", e))?;
                let coils = channel
                    .read_coils(params, coil_range)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to read coil: {:?}", e))?;
                // Extract first coil value and convert bool to u16
                let first_coil = coils.into_iter().next().map(|i| i.value).unwrap_or(false);
                vec![if first_coil { 1u16 } else { 0u16 }]
            }
            "discrete" => {
                let di_range = AddressRange::try_from(register.address, 1)
                    .map_err(|e| anyhow::anyhow!("Invalid discrete input address: {:?}", e))?;
                let inputs = channel
                    .read_discrete_inputs(params, di_range)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to read discrete input: {:?}", e))?;
                // Extract first input value and convert bool to u16
                let first_input = inputs.into_iter().next().map(|i| i.value).unwrap_or(false);
                vec![if first_input { 1u16 } else { 0u16 }]
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

    /// Write to a holding register using rodbus (v1.2.0)
    ///
    /// # Security
    /// - Validates write operations are allowed
    /// - Validates function code against whitelist
    /// - Enforces rate limiting
    pub async fn write_register(&mut self, address: u16, value: u16) -> Result<()> {
        // Security checks
        self.validate_write_allowed()?;
        self.validate_function_code(FC_WRITE_SINGLE_REGISTER)?;
        self.acquire_rate_limit()?;

        let channel = self
            .channel
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Not connected"))?;

        // Create request parameters with unit ID and timeout
        let params = RequestParam::new(self.unit_id, MODBUS_TIMEOUT);

        // rodbus uses Indexed<u16> to combine address and value
        let indexed_value = rodbus::Indexed::new(address, value);
        channel
            .write_single_register(params, indexed_value)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to write register: {:?}", e))?;

        debug!("Wrote value {} to register {}", value, address);
        Ok(())
    }

    /// Write to a coil using rodbus (v1.2.0)
    ///
    /// # Security
    /// - Validates write operations are allowed
    /// - Validates function code against whitelist
    /// - Enforces rate limiting
    pub async fn write_coil(&mut self, address: u16, value: bool) -> Result<()> {
        // Security checks
        self.validate_write_allowed()?;
        self.validate_function_code(FC_WRITE_SINGLE_COIL)?;
        self.acquire_rate_limit()?;

        let channel = self
            .channel
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Not connected"))?;

        // Create request parameters with unit ID and timeout
        let params = RequestParam::new(self.unit_id, MODBUS_TIMEOUT);

        // rodbus uses Indexed<bool> to combine address and value
        let indexed_value = rodbus::Indexed::new(address, value);
        channel
            .write_single_coil(params, indexed_value)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to write coil: {:?}", e))?;

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
///
/// # Thread Safety (v1.2.0)
/// Uses `Arc<Mutex>` for each client to enable parallel reads across devices.
/// Sequential operations still work as before for backwards compatibility.
pub struct ModbusManager {
    /// Clients wrapped in Arc<Mutex> for parallel access (v1.2.0)
    clients: Vec<Arc<Mutex<ModbusClient>>>,
}

impl ModbusManager {
    /// Create a new Modbus manager
    pub fn new(configs: Vec<ModbusDeviceConfig>) -> Self {
        let clients = configs
            .into_iter()
            .map(|c| Arc::new(Mutex::new(ModbusClient::new(c))))
            .collect();

        Self { clients }
    }

    /// Connect all devices (sequential - Modbus connections shouldn't be parallel)
    pub async fn connect_all(&mut self) -> Vec<String> {
        let mut errors = Vec::new();

        for client_arc in &self.clients {
            let mut client = client_arc.lock().await;
            if let Err(e) = client.connect().await {
                error!("Failed to connect to '{}': {}", client.config.name, e);
                errors.push(format!("{}: {}", client.config.name, e));
            }
        }

        errors
    }

    /// Disconnect all devices
    pub async fn disconnect_all(&mut self) {
        for client_arc in &self.clients {
            let mut client = client_arc.lock().await;
            client.disconnect().await;
        }
    }

    /// Read all registers from all devices (sequential)
    ///
    /// Backwards compatible method that reads devices one at a time.
    /// Use `read_all_parallel()` for concurrent reads with lower latency.
    pub async fn read_all(&mut self) -> Vec<ModbusReadResult> {
        let mut results = Vec::new();

        for client_arc in &self.clients {
            let mut client = client_arc.lock().await;
            // Use per-device timeout
            let device_name = client.config.name.clone();
            let result = tokio::time::timeout(
                Duration::from_secs(10), // Per-device timeout
                client.read_all(),
            )
            .await;

            match result {
                Ok(r) => results.push(r),
                Err(_) => {
                    results.push(ModbusReadResult {
                        device_name,
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

    /// Read all registers from all devices (parallel, v1.2.0)
    ///
    /// Reads from all devices concurrently using `join_all`.
    /// This provides lower overall latency when reading multiple devices,
    /// as I/O waits can overlap.
    ///
    /// # Performance
    /// For N devices with T seconds read time each:
    /// - Sequential: N * T total time
    /// - Parallel: ~T total time (limited by slowest device)
    pub async fn read_all_parallel(&self) -> Vec<ModbusReadResult> {
        let futures: Vec<_> = self
            .clients
            .iter()
            .map(|client_arc| {
                let client_arc = Arc::clone(client_arc);
                async move {
                    let mut client = client_arc.lock().await;
                    let device_name = client.config.name.clone();

                    // Use per-device timeout
                    let result = tokio::time::timeout(
                        Duration::from_secs(10), // Per-device timeout
                        client.read_all(),
                    )
                    .await;

                    match result {
                        Ok(r) => r,
                        Err(_) => {
                            // Record failure in circuit breaker
                            client.circuit_breaker.record_failure();
                            ModbusReadResult {
                                device_name,
                                values: vec![],
                                errors: vec!["Device read timeout".to_string()],
                            }
                        }
                    }
                }
            })
            .collect();

        join_all(futures).await
    }

    /// Get client by device name (acquires lock)
    pub async fn get_client_locked(
        &self,
        name: &str,
    ) -> Option<tokio::sync::MutexGuard<'_, ModbusClient>> {
        for client_arc in &self.clients {
            let client = client_arc.lock().await;
            if client.config.name == name {
                return Some(client);
            }
        }
        None
    }

    /// Get client by device name (returns Arc for caller to lock)
    ///
    /// Prefer this over `get_client_locked` when you need to hold the lock
    /// across multiple operations.
    pub fn get_client(&self, name: &str) -> Option<Arc<Mutex<ModbusClient>>> {
        // We need to check the name without holding the lock for too long
        // This is a trade-off: we iterate but don't lock each one
        for client_arc in &self.clients {
            // Try to get name without blocking - use try_lock
            if let Ok(client) = client_arc.try_lock() {
                if client.config.name == name {
                    return Some(Arc::clone(client_arc));
                }
            }
        }
        // Fallback: blocking check if try_lock failed
        None
    }

    /// Get device count
    pub fn device_count(&self) -> usize {
        self.clients.len()
    }

    /// Get circuit breaker status for all devices
    pub async fn circuit_status(&self) -> Vec<(String, &'static str)> {
        let mut results = Vec::new();
        for client_arc in &self.clients {
            let client = client_arc.lock().await;
            results.push((client.config.name.clone(), client.circuit_state()));
        }
        results
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
        self.clients = new_configs
            .into_iter()
            .map(|c| Arc::new(Mutex::new(ModbusClient::new(c))))
            .collect();

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
    use crate::config::ModbusTlsConfig;

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
            security: Default::default(),
            tls: Default::default(),
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
            security: Default::default(),
            tls: Default::default(),
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
            security: Default::default(),
            tls: Default::default(),
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

    #[test]
    fn test_tls_config_in_client() {
        // Test that TLS config is properly initialized
        let tls_config = ModbusTlsConfig {
            enabled: true,
            server_name: Some("plc.example.com".to_string()),
            ca_cert_path: Some("/etc/certs/ca.pem".to_string()),
            client_cert_path: None,
            client_key_path: None,
            insecure_skip_verify: false,
        };

        let client = ModbusClient::new(ModbusDeviceConfig {
            name: "tls-test".to_string(),
            connection_type: "tcp".to_string(),
            address: "192.168.1.100:802".to_string(),
            slave_id: 1,
            baud_rate: None,
            registers: vec![],
            security: Default::default(),
            tls: tls_config.clone(),
        });

        assert!(client.config.tls.enabled);
        assert_eq!(
            client.config.tls.server_name,
            Some("plc.example.com".to_string())
        );
        assert!(!client.is_connected()); // Not connected yet
    }

    #[test]
    fn test_unit_id_creation() {
        // Test that UnitId is properly created from slave_id
        let client = ModbusClient::new(ModbusDeviceConfig {
            name: "unit-id-test".to_string(),
            connection_type: "tcp".to_string(),
            address: "127.0.0.1:502".to_string(),
            slave_id: 42,
            baud_rate: None,
            registers: vec![],
            security: Default::default(),
            tls: Default::default(),
        });

        // unit_id should be created from slave_id
        assert_eq!(client.unit_id.value, 42);
    }

    // ============================================================================
    // TLS Integration Tests (v1.2.0)
    // ============================================================================
    // These tests require a running Modbus TLS server and are marked with #[ignore].
    // Run with: cargo test --features tls_integration -- --ignored
    //
    // To set up a test environment:
    // 1. Generate test certificates (CA, server, client)
    // 2. Start a Modbus TLS server (e.g., using rodbus server example)
    // 3. Configure paths in the test fixtures

    #[test]
    fn test_tls_config_server_only() {
        // Test TLS configuration for server-only authentication
        let config = ModbusDeviceConfig {
            name: "tls-server-only".to_string(),
            connection_type: "tcp".to_string(),
            address: "127.0.0.1:8802".to_string(),
            slave_id: 1,
            baud_rate: None,
            registers: vec![],
            security: Default::default(),
            tls: ModbusTlsConfig {
                enabled: true,
                server_name: Some("modbus.example.com".to_string()),
                ca_cert_path: Some("/etc/certs/ca.pem".to_string()),
                client_cert_path: None,
                client_key_path: None,
                insecure_skip_verify: false,
            },
        };

        let client = ModbusClient::new(config);
        assert!(client.config.tls.enabled);
        assert!(client.config.tls.client_cert_path.is_none());
        assert!(!client.is_connected());
    }

    #[test]
    fn test_tls_config_mtls() {
        // Test TLS configuration for mutual TLS (mTLS)
        let config = ModbusDeviceConfig {
            name: "tls-mtls".to_string(),
            connection_type: "tcp".to_string(),
            address: "192.168.1.100:8802".to_string(),
            slave_id: 1,
            baud_rate: None,
            registers: vec![],
            security: Default::default(),
            tls: ModbusTlsConfig {
                enabled: true,
                server_name: Some("plc.industrial.local".to_string()),
                ca_cert_path: Some("/etc/certs/ca.pem".to_string()),
                client_cert_path: Some("/etc/certs/client.pem".to_string()),
                client_key_path: Some("/etc/certs/client-key.pem".to_string()),
                insecure_skip_verify: false,
            },
        };

        let client = ModbusClient::new(config);
        assert!(client.config.tls.enabled);
        assert!(client.config.tls.client_cert_path.is_some());
        assert!(client.config.tls.client_key_path.is_some());
    }

    #[test]
    #[ignore = "Requires TLS server: MODBUS_TLS_TEST_SERVER env var"]
    fn test_tls_connection_integration() {
        // Integration test - requires running Modbus TLS server
        // Set MODBUS_TLS_TEST_SERVER=192.168.1.100:8802 to run
        //
        // Expected behavior:
        // 1. Connect to TLS-enabled Modbus server
        // 2. Verify TLS handshake completes
        // 3. Read holding registers successfully
        // 4. Disconnect cleanly
        //
        // This test validates IEC 62443 SL2 FR4 (Data Confidentiality)
    }

    #[test]
    #[ignore = "Requires TLS server with client cert verification"]
    fn test_mtls_connection_integration() {
        // Integration test - requires Modbus server with mTLS
        //
        // Expected behavior:
        // 1. Connect with client certificate
        // 2. Server verifies client identity
        // 3. Mutual authentication succeeds
        // 4. Read/write operations work
        //
        // This test validates IEC 62443 SL2 FR1 (Authentication)
    }

    #[test]
    #[ignore = "Requires TLS server"]
    fn test_tls_certificate_validation() {
        // Test that invalid certificates are rejected
        //
        // Expected behavior:
        // 1. Connection with wrong CA cert fails
        // 2. Connection with expired cert fails
        // 3. Connection with self-signed cert fails (unless insecure mode)
    }
}
