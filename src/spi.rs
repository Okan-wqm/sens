//! SPI (Serial Peripheral Interface) support for Raspberry Pi
//!
//! Provides SPI communication capabilities for high-speed peripherals
//! like ADCs, DACs, displays, and flash memory.
//!
//! ## Raspberry Pi SPI Buses
//! - SPI0: CE0 (GPIO 8), CE1 (GPIO 7), MOSI (GPIO 10), MISO (GPIO 9), SCLK (GPIO 11)
//! - SPI1: CE0 (GPIO 18), CE1 (GPIO 17), CE2 (GPIO 16), MOSI (GPIO 20), MISO (GPIO 19), SCLK (GPIO 21)
//!
//! ## Common SPI Devices
//! - MCP3008: 8-channel 10-bit ADC
//! - MCP3208: 8-channel 12-bit ADC
//! - MAX31855: Thermocouple interface
//! - W25Q series: Flash memory
//!
//! ## v1.2.4 Features
//! - Actor pattern for thread safety
//! - Configurable clock speed, mode, bit order
//! - Full-duplex transfer support
//! - Multiple chip select support

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

/// Default SPI channel buffer size
const DEFAULT_SPI_CHANNEL_SIZE: usize = 64;

/// SPI mode (clock polarity and phase)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SpiMode {
    /// CPOL=0, CPHA=0 - Clock idle low, sample on rising edge
    #[default]
    Mode0,
    /// CPOL=0, CPHA=1 - Clock idle low, sample on falling edge
    Mode1,
    /// CPOL=1, CPHA=0 - Clock idle high, sample on falling edge
    Mode2,
    /// CPOL=1, CPHA=1 - Clock idle high, sample on rising edge
    Mode3,
}

/// Bit order
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum BitOrder {
    /// Most significant bit first
    #[default]
    MsbFirst,
    /// Least significant bit first
    LsbFirst,
}

/// SPI device configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpiDeviceConfig {
    /// Device name (for identification)
    pub name: String,
    /// SPI bus number (0 or 1)
    #[serde(default)]
    pub bus: u8,
    /// Chip select line (0, 1, or 2 for SPI1)
    #[serde(default)]
    pub chip_select: u8,
    /// Clock speed in Hz (default: 1 MHz)
    #[serde(default = "default_spi_clock")]
    pub clock_speed_hz: u32,
    /// SPI mode
    #[serde(default)]
    pub mode: SpiMode,
    /// Bit order
    #[serde(default)]
    pub bit_order: BitOrder,
    /// Bits per word (typically 8)
    #[serde(default = "default_bits_per_word")]
    pub bits_per_word: u8,
    /// Device description
    #[serde(default)]
    pub description: String,
}

fn default_spi_clock() -> u32 {
    1_000_000 // 1 MHz
}

fn default_bits_per_word() -> u8 {
    8
}

impl Default for SpiDeviceConfig {
    fn default() -> Self {
        Self {
            name: "spi_device".to_string(),
            bus: 0,
            chip_select: 0,
            clock_speed_hz: 1_000_000,
            mode: SpiMode::Mode0,
            bit_order: BitOrder::MsbFirst,
            bits_per_word: 8,
            description: String::new(),
        }
    }
}

/// SPI transfer result
#[derive(Debug, Clone, Serialize)]
pub struct SpiTransferResult {
    pub device: String,
    pub tx_data: Vec<u8>,
    pub rx_data: Vec<u8>,
    pub success: bool,
    pub error: Option<String>,
}

// ============================================================================
// Actor Pattern Types
// ============================================================================

/// Commands sent to the SPI actor
#[derive(Debug)]
pub enum SpiCommand {
    /// Initialize SPI bus
    Init {
        response: oneshot::Sender<Result<()>>,
    },
    /// Full-duplex transfer (simultaneous read/write)
    Transfer {
        device: String,
        tx_data: Vec<u8>,
        response: oneshot::Sender<SpiTransferResult>,
    },
    /// Write only (discard received data)
    Write {
        device: String,
        data: Vec<u8>,
        response: oneshot::Sender<Result<()>>,
    },
    /// Read only (send zeros)
    Read {
        device: String,
        length: usize,
        response: oneshot::Sender<SpiTransferResult>,
    },
    /// Set clock speed for a device
    SetClockSpeed {
        device: String,
        clock_speed_hz: u32,
        response: oneshot::Sender<Result<()>>,
    },
    /// Shutdown
    Shutdown,
}

// ============================================================================
// SPI Handle (Public API)
// ============================================================================

/// Thread-safe handle to communicate with the SPI actor
#[derive(Clone)]
pub struct SpiHandle {
    sender: mpsc::Sender<SpiCommand>,
}

impl SpiHandle {
    /// Create a new handle and spawn the actor
    pub fn new(devices: Vec<SpiDeviceConfig>) -> Self {
        let (sender, receiver) = mpsc::channel(DEFAULT_SPI_CHANNEL_SIZE);

        // Spawn the actor
        tokio::spawn(async move {
            let mut actor = SpiActor::new(devices, receiver);
            actor.run().await;
        });

        Self { sender }
    }

    /// Initialize SPI buses
    pub async fn init(&self) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_command(SpiCommand::Init { response: tx }).await?;
        rx.await
            .map_err(|_| anyhow::anyhow!("Actor disconnected"))?
    }

    /// Full-duplex transfer
    pub async fn transfer(&self, device: &str, tx_data: &[u8]) -> SpiTransferResult {
        let (tx, rx) = oneshot::channel();
        if self
            .send_command(SpiCommand::Transfer {
                device: device.to_string(),
                tx_data: tx_data.to_vec(),
                response: tx,
            })
            .await
            .is_err()
        {
            return SpiTransferResult {
                device: device.to_string(),
                tx_data: tx_data.to_vec(),
                rx_data: vec![],
                success: false,
                error: Some("Failed to send command".to_string()),
            };
        }
        rx.await.unwrap_or_else(|_| SpiTransferResult {
            device: device.to_string(),
            tx_data: tx_data.to_vec(),
            rx_data: vec![],
            success: false,
            error: Some("Actor disconnected".to_string()),
        })
    }

    /// Write data to device
    pub async fn write(&self, device: &str, data: &[u8]) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_command(SpiCommand::Write {
            device: device.to_string(),
            data: data.to_vec(),
            response: tx,
        })
        .await?;
        rx.await
            .map_err(|_| anyhow::anyhow!("Actor disconnected"))?
    }

    /// Read data from device
    pub async fn read(&self, device: &str, length: usize) -> SpiTransferResult {
        let (tx, rx) = oneshot::channel();
        if self
            .send_command(SpiCommand::Read {
                device: device.to_string(),
                length,
                response: tx,
            })
            .await
            .is_err()
        {
            return SpiTransferResult {
                device: device.to_string(),
                tx_data: vec![],
                rx_data: vec![],
                success: false,
                error: Some("Failed to send command".to_string()),
            };
        }
        rx.await.unwrap_or_else(|_| SpiTransferResult {
            device: device.to_string(),
            tx_data: vec![],
            rx_data: vec![],
            success: false,
            error: Some("Actor disconnected".to_string()),
        })
    }

    /// Set clock speed for a device
    pub async fn set_clock_speed(&self, device: &str, clock_speed_hz: u32) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_command(SpiCommand::SetClockSpeed {
            device: device.to_string(),
            clock_speed_hz,
            response: tx,
        })
        .await?;
        rx.await
            .map_err(|_| anyhow::anyhow!("Actor disconnected"))?
    }

    /// Send command to actor (blocking until channel has space)
    async fn send_command(&self, cmd: SpiCommand) -> Result<()> {
        self.sender
            .send(cmd)
            .await
            .map_err(|_| anyhow::anyhow!("SPI actor disconnected"))
    }
}

// ============================================================================
// SPI Actor Implementation
// ============================================================================

/// Internal device state
struct SpiDeviceInternal {
    config: SpiDeviceConfig,
    #[cfg(all(target_os = "linux", feature = "gpio"))]
    spi: Option<rppal::spi::Spi>,
}

/// SPI actor that manages SPI bus communication
struct SpiActor {
    devices: Vec<SpiDeviceConfig>,
    receiver: mpsc::Receiver<SpiCommand>,
    device_map: HashMap<String, SpiDeviceInternal>,
}

impl SpiActor {
    fn new(devices: Vec<SpiDeviceConfig>, receiver: mpsc::Receiver<SpiCommand>) -> Self {
        let mut device_map = HashMap::new();
        for device in &devices {
            device_map.insert(
                device.name.clone(),
                SpiDeviceInternal {
                    config: device.clone(),
                    #[cfg(all(target_os = "linux", feature = "gpio"))]
                    spi: None,
                },
            );
        }

        Self {
            devices,
            receiver,
            device_map,
        }
    }

    async fn run(&mut self) {
        info!(
            "SPI actor started with {} devices configured",
            self.devices.len()
        );

        while let Some(cmd) = self.receiver.recv().await {
            match cmd {
                SpiCommand::Init { response } => {
                    let result = self.init_devices();
                    let _ = response.send(result);
                }
                SpiCommand::Transfer {
                    device,
                    tx_data,
                    response,
                } => {
                    let result = self.transfer(&device, &tx_data);
                    let _ = response.send(result);
                }
                SpiCommand::Write {
                    device,
                    data,
                    response,
                } => {
                    let result = self.write(&device, &data);
                    let _ = response.send(result);
                }
                SpiCommand::Read {
                    device,
                    length,
                    response,
                } => {
                    let result = self.read(&device, length);
                    let _ = response.send(result);
                }
                SpiCommand::SetClockSpeed {
                    device,
                    clock_speed_hz,
                    response,
                } => {
                    let result = self.set_clock_speed(&device, clock_speed_hz);
                    let _ = response.send(result);
                }
                SpiCommand::Shutdown => {
                    info!("SPI actor shutting down");
                    break;
                }
            }
        }

        info!("SPI actor stopped");
    }

    #[cfg(all(target_os = "linux", feature = "gpio"))]
    fn init_devices(&mut self) -> Result<()> {
        use rppal::spi::{Bus, Mode, SlaveSelect, Spi};

        for (name, internal) in &mut self.device_map {
            let config = &internal.config;

            let bus = match config.bus {
                0 => Bus::Spi0,
                1 => Bus::Spi1,
                _ => {
                    error!("Invalid SPI bus {} for device '{}'", config.bus, name);
                    continue;
                }
            };

            let slave_select = match config.chip_select {
                0 => SlaveSelect::Ss0,
                1 => SlaveSelect::Ss1,
                2 => SlaveSelect::Ss2,
                _ => {
                    error!(
                        "Invalid chip select {} for device '{}'",
                        config.chip_select, name
                    );
                    continue;
                }
            };

            let mode = match config.mode {
                SpiMode::Mode0 => Mode::Mode0,
                SpiMode::Mode1 => Mode::Mode1,
                SpiMode::Mode2 => Mode::Mode2,
                SpiMode::Mode3 => Mode::Mode3,
            };

            match Spi::new(bus, slave_select, config.clock_speed_hz, mode) {
                Ok(spi) => {
                    info!(
                        "Initialized SPI device '{}' on bus {}, CS {}, {}Hz",
                        name, config.bus, config.chip_select, config.clock_speed_hz
                    );
                    internal.spi = Some(spi);
                }
                Err(e) => {
                    error!("Failed to initialize SPI device '{}': {}", name, e);
                }
            }
        }

        Ok(())
    }

    #[cfg(not(all(target_os = "linux", feature = "gpio")))]
    fn init_devices(&mut self) -> Result<()> {
        warn!("SPI running in simulation mode");
        Ok(())
    }

    #[cfg(all(target_os = "linux", feature = "gpio"))]
    fn transfer(&mut self, device: &str, tx_data: &[u8]) -> SpiTransferResult {
        let internal = match self.device_map.get_mut(device) {
            Some(i) => i,
            None => {
                return SpiTransferResult {
                    device: device.to_string(),
                    tx_data: tx_data.to_vec(),
                    rx_data: vec![],
                    success: false,
                    error: Some(format!("Device '{}' not found", device)),
                };
            }
        };

        let spi = match &mut internal.spi {
            Some(s) => s,
            None => {
                return SpiTransferResult {
                    device: device.to_string(),
                    tx_data: tx_data.to_vec(),
                    rx_data: vec![],
                    success: false,
                    error: Some("SPI not initialized".to_string()),
                };
            }
        };

        let mut rx_data = vec![0u8; tx_data.len()];
        match spi.transfer(&mut rx_data, tx_data) {
            Ok(_) => {
                debug!(
                    "SPI '{}' transfer: TX {:02X?} RX {:02X?}",
                    device, tx_data, rx_data
                );
                SpiTransferResult {
                    device: device.to_string(),
                    tx_data: tx_data.to_vec(),
                    rx_data,
                    success: true,
                    error: None,
                }
            }
            Err(e) => SpiTransferResult {
                device: device.to_string(),
                tx_data: tx_data.to_vec(),
                rx_data: vec![],
                success: false,
                error: Some(format!("Transfer failed: {}", e)),
            },
        }
    }

    #[cfg(not(all(target_os = "linux", feature = "gpio")))]
    fn transfer(&mut self, device: &str, tx_data: &[u8]) -> SpiTransferResult {
        debug!("SPI '{}' transfer: TX {:02X?} (simulated)", device, tx_data);
        SpiTransferResult {
            device: device.to_string(),
            tx_data: tx_data.to_vec(),
            rx_data: vec![0u8; tx_data.len()],
            success: true,
            error: None,
        }
    }

    #[cfg(all(target_os = "linux", feature = "gpio"))]
    fn write(&mut self, device: &str, data: &[u8]) -> Result<()> {
        let internal = self
            .device_map
            .get_mut(device)
            .ok_or_else(|| anyhow::anyhow!("Device '{}' not found", device))?;

        let spi = internal
            .spi
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("SPI not initialized"))?;

        spi.write(data)?;
        debug!("SPI '{}' write: {:02X?}", device, data);
        Ok(())
    }

    #[cfg(not(all(target_os = "linux", feature = "gpio")))]
    fn write(&mut self, device: &str, data: &[u8]) -> Result<()> {
        debug!("SPI '{}' write: {:02X?} (simulated)", device, data);
        Ok(())
    }

    #[cfg(all(target_os = "linux", feature = "gpio"))]
    fn read(&mut self, device: &str, length: usize) -> SpiTransferResult {
        // Send zeros to receive data
        let tx_data = vec![0u8; length];
        self.transfer(device, &tx_data)
    }

    #[cfg(not(all(target_os = "linux", feature = "gpio")))]
    fn read(&mut self, device: &str, length: usize) -> SpiTransferResult {
        SpiTransferResult {
            device: device.to_string(),
            tx_data: vec![0u8; length],
            rx_data: vec![0u8; length],
            success: true,
            error: None,
        }
    }

    #[cfg(all(target_os = "linux", feature = "gpio"))]
    fn set_clock_speed(&mut self, device: &str, clock_speed_hz: u32) -> Result<()> {
        let internal = self
            .device_map
            .get_mut(device)
            .ok_or_else(|| anyhow::anyhow!("Device '{}' not found", device))?;

        let spi = internal
            .spi
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("SPI not initialized"))?;

        spi.set_clock_speed(clock_speed_hz)?;
        internal.config.clock_speed_hz = clock_speed_hz;

        debug!("SPI '{}' clock speed set to {}Hz", device, clock_speed_hz);
        Ok(())
    }

    #[cfg(not(all(target_os = "linux", feature = "gpio")))]
    fn set_clock_speed(&mut self, device: &str, clock_speed_hz: u32) -> Result<()> {
        if let Some(internal) = self.device_map.get_mut(device) {
            internal.config.clock_speed_hz = clock_speed_hz;
        }
        debug!(
            "SPI '{}' clock speed set to {}Hz (simulated)",
            device, clock_speed_hz
        );
        Ok(())
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spi_mode_default() {
        let mode = SpiMode::default();
        assert_eq!(mode, SpiMode::Mode0);
    }

    #[test]
    fn test_spi_device_config_default() {
        let config = SpiDeviceConfig::default();
        assert_eq!(config.bus, 0);
        assert_eq!(config.chip_select, 0);
        assert_eq!(config.clock_speed_hz, 1_000_000);
        assert_eq!(config.mode, SpiMode::Mode0);
        assert_eq!(config.bits_per_word, 8);
    }

    #[test]
    fn test_spi_device_config_serialization() {
        let config = SpiDeviceConfig {
            name: "mcp3008".to_string(),
            bus: 0,
            chip_select: 0,
            clock_speed_hz: 3_600_000, // 3.6 MHz max for MCP3008
            mode: SpiMode::Mode0,
            bit_order: BitOrder::MsbFirst,
            bits_per_word: 8,
            description: "8-channel ADC".to_string(),
        };

        let yaml = serde_yaml::to_string(&config).unwrap();
        assert!(yaml.contains("clock_speed_hz: 3600000"));

        let parsed: SpiDeviceConfig = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed.clock_speed_hz, 3_600_000);
    }

    #[tokio::test]
    async fn test_spi_handle_creation() {
        let devices = vec![SpiDeviceConfig {
            name: "test_adc".to_string(),
            bus: 0,
            chip_select: 0,
            ..Default::default()
        }];

        let handle = SpiHandle::new(devices);

        // Initialize should work (simulation mode)
        let result = handle.init().await;
        assert!(result.is_ok());

        // Transfer should return simulated data
        let transfer_result = handle.transfer("test_adc", &[0x01, 0x80, 0x00]).await;
        assert!(transfer_result.success);
        assert_eq!(transfer_result.rx_data.len(), 3);
    }

    #[test]
    fn test_mcp3008_read_sequence() {
        // MCP3008 read sequence: start bit, single-ended, channel
        // For channel 0: 0x01, 0x80, 0x00
        let start_bit = 0x01;
        let single_ended_ch0 = 0x80; // D2=1, D1=0, D0=0

        assert_eq!(start_bit, 0b00000001);
        assert_eq!(single_ended_ch0 & 0xF0, 0x80); // Top nibble = 1000
    }
}
