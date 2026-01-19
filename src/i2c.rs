//! I2C (Inter-Integrated Circuit) support for Raspberry Pi
//!
//! Provides I2C communication capabilities for sensors, displays,
//! and other I2C peripherals.
//!
//! ## Raspberry Pi I2C Buses
//! - I2C1: GPIO 2 (SDA), GPIO 3 (SCL) - primary bus
//! - I2C0: GPIO 0 (SDA), GPIO 1 (SCL) - reserved for HAT EEPROM
//!
//! ## Common I2C Devices
//! - BME280: Temperature, humidity, pressure (0x76/0x77)
//! - SHT31: Temperature, humidity (0x44/0x45)
//! - ADS1115: 16-bit ADC (0x48-0x4B)
//! - PCA9685: 16-channel PWM (0x40-0x7F)
//!
//! ## v1.2.4 Features
//! - Actor pattern for thread safety
//! - Read/write operations
//! - Block read/write support
//! - Device scanning

#![allow(dead_code)]

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tracing::{debug, info, warn};

// ============================================================================
// Configuration
// ============================================================================

/// Default I2C channel buffer size
const DEFAULT_I2C_CHANNEL_SIZE: usize = 64;

/// I2C device configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct I2cDeviceConfig {
    /// Device name (for identification)
    pub name: String,
    /// I2C address (7-bit, typically 0x00-0x7F)
    pub address: u8,
    /// I2C bus number (default: 1 for Raspberry Pi)
    #[serde(default = "default_bus")]
    pub bus: u8,
    /// Clock speed in Hz (default: 100000)
    #[serde(default = "default_clock_speed")]
    pub clock_speed_hz: u32,
    /// Device description
    #[serde(default)]
    pub description: String,
}

fn default_bus() -> u8 {
    1
}

fn default_clock_speed() -> u32 {
    100_000 // 100 kHz standard mode
}

impl Default for I2cDeviceConfig {
    fn default() -> Self {
        Self {
            name: "i2c_device".to_string(),
            address: 0x00,
            bus: 1,
            clock_speed_hz: 100_000,
            description: String::new(),
        }
    }
}

/// I2C read result
#[derive(Debug, Clone, Serialize)]
pub struct I2cReadResult {
    pub device: String,
    pub address: u8,
    pub register: u8,
    pub data: Vec<u8>,
    pub success: bool,
    pub error: Option<String>,
}

/// I2C scan result
#[derive(Debug, Clone, Serialize)]
pub struct I2cScanResult {
    pub bus: u8,
    pub devices: Vec<u8>,
}

// ============================================================================
// Actor Pattern Types
// ============================================================================

/// Commands sent to the I2C actor
#[derive(Debug)]
pub enum I2cCommand {
    /// Initialize I2C bus
    Init {
        response: oneshot::Sender<Result<()>>,
    },
    /// Read bytes from a register
    ReadRegister {
        device: String,
        register: u8,
        length: usize,
        response: oneshot::Sender<I2cReadResult>,
    },
    /// Write bytes to a register
    WriteRegister {
        device: String,
        register: u8,
        data: Vec<u8>,
        response: oneshot::Sender<Result<()>>,
    },
    /// Read bytes directly (no register)
    ReadDirect {
        device: String,
        length: usize,
        response: oneshot::Sender<I2cReadResult>,
    },
    /// Write bytes directly (no register)
    WriteDirect {
        device: String,
        data: Vec<u8>,
        response: oneshot::Sender<Result<()>>,
    },
    /// Scan I2C bus for devices
    Scan {
        bus: u8,
        response: oneshot::Sender<I2cScanResult>,
    },
    /// Check if device is present
    Probe {
        device: String,
        response: oneshot::Sender<bool>,
    },
    /// Shutdown
    Shutdown,
}

// ============================================================================
// I2C Handle (Public API)
// ============================================================================

/// Thread-safe handle to communicate with the I2C actor
#[derive(Clone)]
pub struct I2cHandle {
    sender: mpsc::Sender<I2cCommand>,
}

impl I2cHandle {
    /// Create a new handle and spawn the actor
    pub fn new(devices: Vec<I2cDeviceConfig>) -> Self {
        let (sender, receiver) = mpsc::channel(DEFAULT_I2C_CHANNEL_SIZE);

        // Spawn the actor
        tokio::spawn(async move {
            let mut actor = I2cActor::new(devices, receiver);
            actor.run().await;
        });

        Self { sender }
    }

    /// Initialize I2C bus
    pub async fn init(&self) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_command(I2cCommand::Init { response: tx }).await?;
        rx.await
            .map_err(|_| anyhow::anyhow!("Actor disconnected"))?
    }

    /// Read bytes from a device register
    pub async fn read_register(&self, device: &str, register: u8, length: usize) -> I2cReadResult {
        let (tx, rx) = oneshot::channel();
        if self
            .send_command(I2cCommand::ReadRegister {
                device: device.to_string(),
                register,
                length,
                response: tx,
            })
            .await
            .is_err()
        {
            return I2cReadResult {
                device: device.to_string(),
                address: 0,
                register,
                data: vec![],
                success: false,
                error: Some("Failed to send command".to_string()),
            };
        }
        rx.await.unwrap_or_else(|_| I2cReadResult {
            device: device.to_string(),
            address: 0,
            register,
            data: vec![],
            success: false,
            error: Some("Actor disconnected".to_string()),
        })
    }

    /// Write bytes to a device register
    pub async fn write_register(&self, device: &str, register: u8, data: &[u8]) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_command(I2cCommand::WriteRegister {
            device: device.to_string(),
            register,
            data: data.to_vec(),
            response: tx,
        })
        .await?;
        rx.await
            .map_err(|_| anyhow::anyhow!("Actor disconnected"))?
    }

    /// Read bytes directly from device (no register address)
    pub async fn read_direct(&self, device: &str, length: usize) -> I2cReadResult {
        let (tx, rx) = oneshot::channel();
        if self
            .send_command(I2cCommand::ReadDirect {
                device: device.to_string(),
                length,
                response: tx,
            })
            .await
            .is_err()
        {
            return I2cReadResult {
                device: device.to_string(),
                address: 0,
                register: 0,
                data: vec![],
                success: false,
                error: Some("Failed to send command".to_string()),
            };
        }
        rx.await.unwrap_or_else(|_| I2cReadResult {
            device: device.to_string(),
            address: 0,
            register: 0,
            data: vec![],
            success: false,
            error: Some("Actor disconnected".to_string()),
        })
    }

    /// Write bytes directly to device (no register address)
    pub async fn write_direct(&self, device: &str, data: &[u8]) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_command(I2cCommand::WriteDirect {
            device: device.to_string(),
            data: data.to_vec(),
            response: tx,
        })
        .await?;
        rx.await
            .map_err(|_| anyhow::anyhow!("Actor disconnected"))?
    }

    /// Scan I2C bus for devices
    pub async fn scan(&self, bus: u8) -> I2cScanResult {
        let (tx, rx) = oneshot::channel();
        if self
            .send_command(I2cCommand::Scan { bus, response: tx })
            .await
            .is_err()
        {
            return I2cScanResult {
                bus,
                devices: vec![],
            };
        }
        rx.await.unwrap_or_else(|_| I2cScanResult {
            bus,
            devices: vec![],
        })
    }

    /// Check if a device is present
    pub async fn probe(&self, device: &str) -> bool {
        let (tx, rx) = oneshot::channel();
        if self
            .send_command(I2cCommand::Probe {
                device: device.to_string(),
                response: tx,
            })
            .await
            .is_err()
        {
            return false;
        }
        rx.await.unwrap_or(false)
    }

    /// Send command to actor (blocking until channel has space)
    async fn send_command(&self, cmd: I2cCommand) -> Result<()> {
        self.sender
            .send(cmd)
            .await
            .map_err(|_| anyhow::anyhow!("I2C actor disconnected"))
    }
}

// ============================================================================
// I2C Actor Implementation
// ============================================================================

/// I2C actor that manages I2C bus communication
struct I2cActor {
    devices: Vec<I2cDeviceConfig>,
    receiver: mpsc::Receiver<I2cCommand>,
    device_map: HashMap<String, I2cDeviceConfig>,
    #[cfg(all(target_os = "linux", feature = "gpio"))]
    i2c_buses: HashMap<u8, rppal::i2c::I2c>,
}

impl I2cActor {
    fn new(devices: Vec<I2cDeviceConfig>, receiver: mpsc::Receiver<I2cCommand>) -> Self {
        let mut device_map = HashMap::new();
        for device in &devices {
            device_map.insert(device.name.clone(), device.clone());
        }

        Self {
            devices,
            receiver,
            device_map,
            #[cfg(all(target_os = "linux", feature = "gpio"))]
            i2c_buses: HashMap::new(),
        }
    }

    async fn run(&mut self) {
        info!(
            "I2C actor started with {} devices configured",
            self.devices.len()
        );

        while let Some(cmd) = self.receiver.recv().await {
            match cmd {
                I2cCommand::Init { response } => {
                    let result = self.init_buses();
                    let _ = response.send(result);
                }
                I2cCommand::ReadRegister {
                    device,
                    register,
                    length,
                    response,
                } => {
                    let result = self.read_register(&device, register, length);
                    let _ = response.send(result);
                }
                I2cCommand::WriteRegister {
                    device,
                    register,
                    data,
                    response,
                } => {
                    let result = self.write_register(&device, register, &data);
                    let _ = response.send(result);
                }
                I2cCommand::ReadDirect {
                    device,
                    length,
                    response,
                } => {
                    let result = self.read_direct(&device, length);
                    let _ = response.send(result);
                }
                I2cCommand::WriteDirect {
                    device,
                    data,
                    response,
                } => {
                    let result = self.write_direct(&device, &data);
                    let _ = response.send(result);
                }
                I2cCommand::Scan { bus, response } => {
                    let result = self.scan_bus(bus);
                    let _ = response.send(result);
                }
                I2cCommand::Probe { device, response } => {
                    let result = self.probe_device(&device);
                    let _ = response.send(result);
                }
                I2cCommand::Shutdown => {
                    info!("I2C actor shutting down");
                    break;
                }
            }
        }

        info!("I2C actor stopped");
    }

    #[cfg(all(target_os = "linux", feature = "gpio"))]
    fn init_buses(&mut self) -> Result<()> {
        use rppal::i2c::I2c;

        // Collect unique bus numbers
        let buses: std::collections::HashSet<u8> = self.devices.iter().map(|d| d.bus).collect();

        for bus in buses {
            match I2c::with_bus(bus) {
                Ok(i2c) => {
                    info!("Initialized I2C bus {}", bus);
                    self.i2c_buses.insert(bus, i2c);
                }
                Err(e) => {
                    error!("Failed to initialize I2C bus {}: {}", bus, e);
                    return Err(anyhow::anyhow!(
                        "Failed to initialize I2C bus {}: {}",
                        bus,
                        e
                    ));
                }
            }
        }

        Ok(())
    }

    #[cfg(not(all(target_os = "linux", feature = "gpio")))]
    fn init_buses(&mut self) -> Result<()> {
        warn!("I2C running in simulation mode");
        Ok(())
    }

    #[cfg(all(target_os = "linux", feature = "gpio"))]
    fn read_register(&mut self, device: &str, register: u8, length: usize) -> I2cReadResult {
        let config = match self.device_map.get(device) {
            Some(c) => c.clone(),
            None => {
                return I2cReadResult {
                    device: device.to_string(),
                    address: 0,
                    register,
                    data: vec![],
                    success: false,
                    error: Some(format!("Device '{}' not found", device)),
                };
            }
        };

        let i2c = match self.i2c_buses.get_mut(&config.bus) {
            Some(i) => i,
            None => {
                return I2cReadResult {
                    device: device.to_string(),
                    address: config.address,
                    register,
                    data: vec![],
                    success: false,
                    error: Some(format!("I2C bus {} not initialized", config.bus)),
                };
            }
        };

        // Set slave address
        if let Err(e) = i2c.set_slave_address(config.address as u16) {
            return I2cReadResult {
                device: device.to_string(),
                address: config.address,
                register,
                data: vec![],
                success: false,
                error: Some(format!("Failed to set address: {}", e)),
            };
        }

        let mut buffer = vec![0u8; length];
        match i2c.block_read(register, &mut buffer) {
            Ok(_) => {
                debug!(
                    "I2C read from '{}' reg 0x{:02X}: {:02X?}",
                    device, register, buffer
                );
                I2cReadResult {
                    device: device.to_string(),
                    address: config.address,
                    register,
                    data: buffer,
                    success: true,
                    error: None,
                }
            }
            Err(e) => I2cReadResult {
                device: device.to_string(),
                address: config.address,
                register,
                data: vec![],
                success: false,
                error: Some(format!("Read failed: {}", e)),
            },
        }
    }

    #[cfg(not(all(target_os = "linux", feature = "gpio")))]
    fn read_register(&mut self, device: &str, register: u8, length: usize) -> I2cReadResult {
        let config = self.device_map.get(device);
        let address = config.map(|c| c.address).unwrap_or(0);

        debug!(
            "I2C read from '{}' reg 0x{:02X} length {} (simulated)",
            device, register, length
        );

        I2cReadResult {
            device: device.to_string(),
            address,
            register,
            data: vec![0u8; length], // Simulated zeros
            success: true,
            error: None,
        }
    }

    #[cfg(all(target_os = "linux", feature = "gpio"))]
    fn write_register(&mut self, device: &str, register: u8, data: &[u8]) -> Result<()> {
        let config = self
            .device_map
            .get(device)
            .ok_or_else(|| anyhow::anyhow!("Device '{}' not found", device))?
            .clone();

        let i2c = self
            .i2c_buses
            .get_mut(&config.bus)
            .ok_or_else(|| anyhow::anyhow!("I2C bus {} not initialized", config.bus))?;

        i2c.set_slave_address(config.address as u16)?;
        i2c.block_write(register, data)?;

        debug!(
            "I2C write to '{}' reg 0x{:02X}: {:02X?}",
            device, register, data
        );
        Ok(())
    }

    #[cfg(not(all(target_os = "linux", feature = "gpio")))]
    fn write_register(&mut self, device: &str, register: u8, data: &[u8]) -> Result<()> {
        debug!(
            "I2C write to '{}' reg 0x{:02X}: {:02X?} (simulated)",
            device, register, data
        );
        Ok(())
    }

    #[cfg(all(target_os = "linux", feature = "gpio"))]
    fn read_direct(&mut self, device: &str, length: usize) -> I2cReadResult {
        let config = match self.device_map.get(device) {
            Some(c) => c.clone(),
            None => {
                return I2cReadResult {
                    device: device.to_string(),
                    address: 0,
                    register: 0,
                    data: vec![],
                    success: false,
                    error: Some(format!("Device '{}' not found", device)),
                };
            }
        };

        let i2c = match self.i2c_buses.get_mut(&config.bus) {
            Some(i) => i,
            None => {
                return I2cReadResult {
                    device: device.to_string(),
                    address: config.address,
                    register: 0,
                    data: vec![],
                    success: false,
                    error: Some(format!("I2C bus {} not initialized", config.bus)),
                };
            }
        };

        if let Err(e) = i2c.set_slave_address(config.address as u16) {
            return I2cReadResult {
                device: device.to_string(),
                address: config.address,
                register: 0,
                data: vec![],
                success: false,
                error: Some(format!("Failed to set address: {}", e)),
            };
        }

        let mut buffer = vec![0u8; length];
        match i2c.read(&mut buffer) {
            Ok(_) => I2cReadResult {
                device: device.to_string(),
                address: config.address,
                register: 0,
                data: buffer,
                success: true,
                error: None,
            },
            Err(e) => I2cReadResult {
                device: device.to_string(),
                address: config.address,
                register: 0,
                data: vec![],
                success: false,
                error: Some(format!("Read failed: {}", e)),
            },
        }
    }

    #[cfg(not(all(target_os = "linux", feature = "gpio")))]
    fn read_direct(&mut self, device: &str, length: usize) -> I2cReadResult {
        let config = self.device_map.get(device);
        let address = config.map(|c| c.address).unwrap_or(0);

        I2cReadResult {
            device: device.to_string(),
            address,
            register: 0,
            data: vec![0u8; length],
            success: true,
            error: None,
        }
    }

    #[cfg(all(target_os = "linux", feature = "gpio"))]
    fn write_direct(&mut self, device: &str, data: &[u8]) -> Result<()> {
        let config = self
            .device_map
            .get(device)
            .ok_or_else(|| anyhow::anyhow!("Device '{}' not found", device))?
            .clone();

        let i2c = self
            .i2c_buses
            .get_mut(&config.bus)
            .ok_or_else(|| anyhow::anyhow!("I2C bus {} not initialized", config.bus))?;

        i2c.set_slave_address(config.address as u16)?;
        i2c.write(data)?;

        Ok(())
    }

    #[cfg(not(all(target_os = "linux", feature = "gpio")))]
    fn write_direct(&mut self, _device: &str, _data: &[u8]) -> Result<()> {
        Ok(())
    }

    #[cfg(all(target_os = "linux", feature = "gpio"))]
    fn scan_bus(&mut self, bus: u8) -> I2cScanResult {
        use rppal::i2c::I2c;

        let mut devices = Vec::new();

        match I2c::with_bus(bus) {
            Ok(mut i2c) => {
                // Scan addresses 0x03 to 0x77 (valid 7-bit addresses)
                for addr in 0x03..=0x77 {
                    if i2c.set_slave_address(addr).is_ok() {
                        // Try a quick read to detect device
                        let mut buf = [0u8; 1];
                        if i2c.read(&mut buf).is_ok() {
                            devices.push(addr as u8);
                            debug!("I2C device found at 0x{:02X}", addr);
                        }
                    }
                }
            }
            Err(e) => {
                warn!("Failed to scan I2C bus {}: {}", bus, e);
            }
        }

        I2cScanResult { bus, devices }
    }

    #[cfg(not(all(target_os = "linux", feature = "gpio")))]
    fn scan_bus(&mut self, bus: u8) -> I2cScanResult {
        warn!("I2C scan on bus {} (simulated)", bus);
        I2cScanResult {
            bus,
            devices: vec![],
        }
    }

    fn probe_device(&mut self, device: &str) -> bool {
        let result = self.read_direct(device, 1);
        result.success
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_i2c_device_config_default() {
        let config = I2cDeviceConfig::default();
        assert_eq!(config.bus, 1);
        assert_eq!(config.clock_speed_hz, 100_000);
    }

    #[test]
    fn test_i2c_device_config_serialization() {
        let config = I2cDeviceConfig {
            name: "bme280".to_string(),
            address: 0x76,
            bus: 1,
            clock_speed_hz: 400_000,
            description: "Temperature sensor".to_string(),
        };

        let yaml = serde_yaml::to_string(&config).unwrap();
        assert!(yaml.contains("address: 118")); // 0x76 = 118
        assert!(yaml.contains("bus: 1"));

        let parsed: I2cDeviceConfig = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed.address, 0x76);
    }

    #[tokio::test]
    async fn test_i2c_handle_creation() {
        let devices = vec![I2cDeviceConfig {
            name: "test_sensor".to_string(),
            address: 0x48,
            bus: 1,
            ..Default::default()
        }];

        let handle = I2cHandle::new(devices);

        // Initialize should work (simulation mode)
        let result = handle.init().await;
        assert!(result.is_ok());

        // Read should return simulated data
        let read_result = handle.read_register("test_sensor", 0x00, 2).await;
        assert!(read_result.success);
        assert_eq!(read_result.data.len(), 2);
    }

    #[test]
    fn test_common_i2c_addresses() {
        // Common I2C device addresses
        let bme280 = 0x76_u8;
        let sht31 = 0x44_u8;
        let ads1115 = 0x48_u8;
        let pca9685 = 0x40_u8;

        assert!(bme280 >= 0x03 && bme280 <= 0x77);
        assert!(sht31 >= 0x03 && sht31 <= 0x77);
        assert!(ads1115 >= 0x03 && ads1115 <= 0x77);
        assert!(pca9685 >= 0x03 && pca9685 <= 0x77);
    }
}
