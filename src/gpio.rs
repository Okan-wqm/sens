//! GPIO support for Raspberry Pi / Revolution Pi
//!
//! Provides digital I/O capabilities for edge devices with GPIO pins.
//! Uses actor pattern to isolate non-Send rppal types.
//! Components communicate with the actor via channels.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

use crate::config::GpioConfig;
use crate::resilience::with_timeout;

// ============================================================================
// Public Types
// ============================================================================

/// GPIO pin state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PinState {
    High,
    #[default]
    Low,
}

impl From<bool> for PinState {
    fn from(value: bool) -> Self {
        if value {
            PinState::High
        } else {
            PinState::Low
        }
    }
}

impl From<PinState> for bool {
    fn from(state: PinState) -> Self {
        matches!(state, PinState::High)
    }
}

/// GPIO pin value with metadata
#[derive(Debug, Clone, Serialize)]
pub struct GpioPinValue {
    pub name: String,
    pub pin: u8,
    pub direction: String,
    pub state: PinState,
    pub timestamp: String,
}

/// GPIO read result
#[derive(Debug, Clone, Serialize, Default)]
pub struct GpioReadResult {
    pub values: Vec<GpioPinValue>,
    pub errors: Vec<String>,
}

// ============================================================================
// Actor Pattern Types
// ============================================================================

/// Commands sent to the GPIO actor
#[derive(Debug)]
pub enum GpioCommand {
    /// Initialize GPIO hardware
    Init {
        response: oneshot::Sender<Result<()>>,
    },
    /// Read all configured input pins
    ReadAll {
        response: oneshot::Sender<GpioReadResult>,
    },
    /// Read a single pin
    ReadPin {
        pin: u8,
        response: oneshot::Sender<Result<PinState>>,
    },
    /// Write to an output pin
    WritePin {
        pin: u8,
        value: bool,
        response: oneshot::Sender<Result<()>>,
    },
    /// Get pin count
    GetPinCount { response: oneshot::Sender<usize> },
    /// Check if GPIO is available
    IsAvailable { response: oneshot::Sender<bool> },
    /// Reconfigure GPIO (hot-reload)
    Reconfigure {
        configs: Vec<GpioConfig>,
        response: oneshot::Sender<Result<()>>,
    },
}

/// Thread-safe handle to communicate with the GPIO actor
#[derive(Clone)]
pub struct GpioHandle {
    sender: mpsc::Sender<GpioCommand>,
    /// Operation timeout (configurable)
    timeout: Duration,
}

impl GpioHandle {
    /// Create a new handle and spawn the GPIO actor
    ///
    /// Must be called within a LocalSet context on Linux
    /// `timeout_secs` configures the operation timeout (default: 5 seconds)
    pub fn new(configs: Vec<GpioConfig>, timeout_secs: u64) -> Self {
        let (sender, receiver) = mpsc::channel(32);

        // Spawn the actor in a local task (for non-Send rppal types)
        tokio::task::spawn_local(async move {
            let mut actor = GpioActor::new(configs, receiver);
            actor.run().await;
        });

        Self {
            sender,
            timeout: Duration::from_secs(timeout_secs),
        }
    }

    /// Initialize GPIO hardware
    pub async fn init(&self) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(GpioCommand::Init { response: tx })
            .await
            .map_err(|_| anyhow::anyhow!("GPIO actor dead"))?;

        with_timeout(rx, self.timeout, "GPIO init")
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?
            .map_err(|_| anyhow::anyhow!("GPIO actor response error"))?
    }

    /// Read all configured input pins
    pub async fn read_all(&self) -> GpioReadResult {
        let (tx, rx) = oneshot::channel();
        if self
            .sender
            .send(GpioCommand::ReadAll { response: tx })
            .await
            .is_err()
        {
            return GpioReadResult {
                values: vec![],
                errors: vec!["GPIO actor dead".to_string()],
            };
        }

        match with_timeout(rx, self.timeout, "GPIO read_all").await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => GpioReadResult {
                values: vec![],
                errors: vec!["GPIO actor response error".to_string()],
            },
            Err(_) => GpioReadResult {
                values: vec![],
                errors: vec!["GPIO read timeout".to_string()],
            },
        }
    }

    /// Read a single pin
    pub async fn read_pin(&self, pin: u8) -> Result<PinState> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(GpioCommand::ReadPin { pin, response: tx })
            .await
            .map_err(|_| anyhow::anyhow!("GPIO actor dead"))?;

        with_timeout(rx, self.timeout, &format!("GPIO read pin {}", pin))
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?
            .map_err(|_| anyhow::anyhow!("GPIO actor response error"))?
    }

    /// Write to an output pin
    pub async fn write_pin(&self, pin: u8, value: bool) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(GpioCommand::WritePin {
                pin,
                value,
                response: tx,
            })
            .await
            .map_err(|_| anyhow::anyhow!("GPIO actor dead"))?;

        with_timeout(rx, self.timeout, &format!("GPIO write pin {}", pin))
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?
            .map_err(|_| anyhow::anyhow!("GPIO actor response error"))?
    }

    /// Get configured pin count
    pub async fn pin_count(&self) -> usize {
        let (tx, rx) = oneshot::channel();
        if self
            .sender
            .send(GpioCommand::GetPinCount { response: tx })
            .await
            .is_err()
        {
            return 0;
        }
        rx.await.unwrap_or(0)
    }

    /// Check if GPIO hardware is available
    pub async fn is_available(&self) -> bool {
        let (tx, rx) = oneshot::channel();
        if self
            .sender
            .send(GpioCommand::IsAvailable { response: tx })
            .await
            .is_err()
        {
            return false;
        }
        rx.await.unwrap_or(false)
    }

    /// Reconfigure GPIO pins (hot-reload)
    pub async fn reconfigure(&self, configs: Vec<GpioConfig>) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(GpioCommand::Reconfigure {
                configs,
                response: tx,
            })
            .await
            .map_err(|_| anyhow::anyhow!("GPIO actor dead"))?;

        rx.await
            .map_err(|_| anyhow::anyhow!("GPIO actor response error"))?
    }
}

// ============================================================================
// GPIO Actor Implementation
// ============================================================================

/// GPIO actor that owns the non-Send rppal types
struct GpioActor {
    configs: Vec<GpioConfig>,
    receiver: mpsc::Receiver<GpioCommand>,
    /// Simulated pin states for non-Linux platforms or testing
    simulated_states: HashMap<u8, PinState>,
    #[cfg(all(target_os = "linux", feature = "gpio"))]
    gpio: Option<rppal::gpio::Gpio>,
    #[cfg(all(target_os = "linux", feature = "gpio"))]
    input_pins: HashMap<u8, rppal::gpio::InputPin>,
    #[cfg(all(target_os = "linux", feature = "gpio"))]
    output_pins: HashMap<u8, rppal::gpio::OutputPin>,
}

impl GpioActor {
    fn new(configs: Vec<GpioConfig>, receiver: mpsc::Receiver<GpioCommand>) -> Self {
        let mut simulated_states = HashMap::new();
        for config in &configs {
            simulated_states.insert(config.pin, PinState::Low);
        }

        Self {
            configs,
            receiver,
            simulated_states,
            #[cfg(all(target_os = "linux", feature = "gpio"))]
            gpio: None,
            #[cfg(all(target_os = "linux", feature = "gpio"))]
            input_pins: HashMap::new(),
            #[cfg(all(target_os = "linux", feature = "gpio"))]
            output_pins: HashMap::new(),
        }
    }

    async fn run(&mut self) {
        info!(
            "GPIO actor started with {} pins configured",
            self.configs.len()
        );

        while let Some(cmd) = self.receiver.recv().await {
            match cmd {
                GpioCommand::Init { response } => {
                    let result = self.init_gpio();
                    let _ = response.send(result);
                }
                GpioCommand::ReadAll { response } => {
                    let result = self.read_all_pins();
                    let _ = response.send(result);
                }
                GpioCommand::ReadPin { pin, response } => {
                    let result = self.read_single_pin(pin);
                    let _ = response.send(result);
                }
                GpioCommand::WritePin {
                    pin,
                    value,
                    response,
                } => {
                    let result = self.write_single_pin(pin, value);
                    let _ = response.send(result);
                }
                GpioCommand::GetPinCount { response } => {
                    let _ = response.send(self.configs.len());
                }
                GpioCommand::IsAvailable { response } => {
                    let _ = response.send(self.is_gpio_available());
                }
                GpioCommand::Reconfigure { configs, response } => {
                    let result = self.reconfigure_pins(configs);
                    let _ = response.send(result);
                }
            }
        }

        info!("GPIO actor stopped");
    }

    // Linux implementation with rppal
    #[cfg(all(target_os = "linux", feature = "gpio"))]
    fn init_gpio(&mut self) -> Result<()> {
        use rppal::gpio::Gpio;

        info!(
            "Initializing GPIO with {} pins configured",
            self.configs.len()
        );

        let gpio = Gpio::new().context("Failed to initialize GPIO")?;

        for config in &self.configs.clone() {
            info!(
                "Configuring GPIO pin {} ({}) as {}",
                config.pin, config.name, config.direction
            );

            let pin = gpio
                .get(config.pin)
                .with_context(|| format!("Failed to get GPIO pin {}", config.pin))?;

            match config.direction.as_str() {
                "input" => {
                    let mut input_pin = pin.into_input();
                    match config.pull.as_str() {
                        "up" => input_pin.set_pullupdown(rppal::gpio::PullUpDown::PullUp),
                        "down" => input_pin.set_pullupdown(rppal::gpio::PullUpDown::PullDown),
                        _ => input_pin.set_pullupdown(rppal::gpio::PullUpDown::Off),
                    }
                    self.input_pins.insert(config.pin, input_pin);
                }
                "output" => {
                    let output_pin = pin.into_output();
                    self.output_pins.insert(config.pin, output_pin);
                }
                other => {
                    warn!(
                        "Unknown direction '{}' for pin {}, skipping",
                        other, config.pin
                    );
                    continue;
                }
            }
        }

        self.gpio = Some(gpio);
        info!("GPIO initialized successfully");
        Ok(())
    }

    #[cfg(not(all(target_os = "linux", feature = "gpio")))]
    fn init_gpio(&mut self) -> Result<()> {
        if !self.configs.is_empty() {
            warn!("GPIO configured but not available on this platform (simulation mode)");
        }
        Ok(())
    }

    #[cfg(all(target_os = "linux", feature = "gpio"))]
    fn read_all_pins(&self) -> GpioReadResult {
        let mut result = GpioReadResult::default();

        for config in &self.configs {
            if config.direction != "input" {
                continue;
            }

            if let Some(pin) = self.input_pins.get(&config.pin) {
                let raw_state = if pin.is_high() {
                    PinState::High
                } else {
                    PinState::Low
                };
                let final_state = if config.invert {
                    match raw_state {
                        PinState::High => PinState::Low,
                        PinState::Low => PinState::High,
                    }
                } else {
                    raw_state
                };

                result.values.push(GpioPinValue {
                    name: config.name.clone(),
                    pin: config.pin,
                    direction: config.direction.clone(),
                    state: final_state,
                    timestamp: chrono::Utc::now().to_rfc3339(),
                });
            } else {
                result
                    .errors
                    .push(format!("{}: Pin not initialized", config.name));
            }
        }

        result
    }

    #[cfg(not(all(target_os = "linux", feature = "gpio")))]
    fn read_all_pins(&self) -> GpioReadResult {
        let mut result = GpioReadResult::default();

        for config in &self.configs {
            if config.direction != "input" {
                continue;
            }

            let raw_state = self
                .simulated_states
                .get(&config.pin)
                .copied()
                .unwrap_or(PinState::Low);

            let final_state = if config.invert {
                match raw_state {
                    PinState::High => PinState::Low,
                    PinState::Low => PinState::High,
                }
            } else {
                raw_state
            };

            result.values.push(GpioPinValue {
                name: config.name.clone(),
                pin: config.pin,
                direction: config.direction.clone(),
                state: final_state,
                timestamp: chrono::Utc::now().to_rfc3339(),
            });
        }

        result
    }

    #[cfg(all(target_os = "linux", feature = "gpio"))]
    fn read_single_pin(&self, pin: u8) -> Result<PinState> {
        if let Some(input_pin) = self.input_pins.get(&pin) {
            let state = if input_pin.is_high() {
                PinState::High
            } else {
                PinState::Low
            };
            Ok(state)
        } else {
            Err(anyhow::anyhow!("Pin {} not configured as input", pin))
        }
    }

    #[cfg(not(all(target_os = "linux", feature = "gpio")))]
    fn read_single_pin(&self, pin: u8) -> Result<PinState> {
        self.simulated_states
            .get(&pin)
            .copied()
            .ok_or_else(|| anyhow::anyhow!("Pin {} not configured", pin))
    }

    #[cfg(all(target_os = "linux", feature = "gpio"))]
    fn write_single_pin(&mut self, pin: u8, value: bool) -> Result<()> {
        if let Some(output_pin) = self.output_pins.get_mut(&pin) {
            if value {
                output_pin.set_high();
            } else {
                output_pin.set_low();
            }
            debug!("Set GPIO pin {} to {}", pin, value);
            Ok(())
        } else {
            Err(anyhow::anyhow!("Pin {} not configured as output", pin))
        }
    }

    #[cfg(not(all(target_os = "linux", feature = "gpio")))]
    fn write_single_pin(&mut self, pin: u8, value: bool) -> Result<()> {
        if self
            .configs
            .iter()
            .any(|c| c.pin == pin && c.direction == "output")
        {
            let state = if value { PinState::High } else { PinState::Low };
            self.simulated_states.insert(pin, state);
            debug!("Simulated GPIO pin {} set to {:?}", pin, state);
            Ok(())
        } else {
            Err(anyhow::anyhow!("Pin {} not configured as output", pin))
        }
    }

    fn is_gpio_available(&self) -> bool {
        #[cfg(all(target_os = "linux", feature = "gpio"))]
        {
            self.gpio.is_some()
        }
        #[cfg(not(all(target_os = "linux", feature = "gpio")))]
        {
            false
        }
    }

    fn reconfigure_pins(&mut self, new_configs: Vec<GpioConfig>) -> Result<()> {
        info!("Reconfiguring GPIO with {} pins", new_configs.len());

        // Clear existing state
        #[cfg(all(target_os = "linux", feature = "gpio"))]
        {
            self.input_pins.clear();
            self.output_pins.clear();
            self.gpio = None;
        }

        self.simulated_states.clear();
        self.configs = new_configs;

        // Reinitialize
        for config in &self.configs {
            self.simulated_states.insert(config.pin, PinState::Low);
        }

        self.init_gpio()
    }
}

// ============================================================================
// Legacy GpioManager (for backwards compatibility during migration)
// ============================================================================

/// Legacy GPIO manager - deprecated, use GpioHandle instead
#[deprecated(note = "Use GpioHandle (actor pattern) instead")]
pub struct GpioManager {
    configs: Vec<GpioConfig>,
    simulated_states: HashMap<u8, PinState>,
    #[cfg(all(target_os = "linux", feature = "gpio"))]
    gpio: Option<rppal::gpio::Gpio>,
    #[cfg(all(target_os = "linux", feature = "gpio"))]
    input_pins: HashMap<u8, rppal::gpio::InputPin>,
    #[cfg(all(target_os = "linux", feature = "gpio"))]
    output_pins: HashMap<u8, rppal::gpio::OutputPin>,
}

#[allow(deprecated)]
impl GpioManager {
    pub fn new(configs: Vec<GpioConfig>) -> Self {
        let mut simulated_states = HashMap::new();
        for config in &configs {
            simulated_states.insert(config.pin, PinState::Low);
        }

        Self {
            configs,
            simulated_states,
            #[cfg(all(target_os = "linux", feature = "gpio"))]
            gpio: None,
            #[cfg(all(target_os = "linux", feature = "gpio"))]
            input_pins: HashMap::new(),
            #[cfg(all(target_os = "linux", feature = "gpio"))]
            output_pins: HashMap::new(),
        }
    }

    #[cfg(all(target_os = "linux", feature = "gpio"))]
    pub fn init(&mut self) -> Result<()> {
        use rppal::gpio::Gpio;
        let gpio = Gpio::new().context("Failed to initialize GPIO")?;

        for config in &self.configs.clone() {
            let pin = gpio.get(config.pin)?;
            match config.direction.as_str() {
                "input" => {
                    let mut input_pin = pin.into_input();
                    match config.pull.as_str() {
                        "up" => input_pin.set_pullupdown(rppal::gpio::PullUpDown::PullUp),
                        "down" => input_pin.set_pullupdown(rppal::gpio::PullUpDown::PullDown),
                        _ => {}
                    }
                    self.input_pins.insert(config.pin, input_pin);
                }
                "output" => {
                    self.output_pins.insert(config.pin, pin.into_output());
                }
                _ => {}
            }
        }
        self.gpio = Some(gpio);
        Ok(())
    }

    #[cfg(not(all(target_os = "linux", feature = "gpio")))]
    pub fn init(&mut self) -> Result<()> {
        Ok(())
    }

    pub fn read_all(&self) -> GpioReadResult {
        let mut result = GpioReadResult::default();
        for config in &self.configs {
            if config.direction != "input" {
                continue;
            }

            #[cfg(all(target_os = "linux", feature = "gpio"))]
            let state = self
                .input_pins
                .get(&config.pin)
                .map(|p| {
                    if p.is_high() {
                        PinState::High
                    } else {
                        PinState::Low
                    }
                })
                .unwrap_or(PinState::Low);

            #[cfg(not(all(target_os = "linux", feature = "gpio")))]
            let state = self
                .simulated_states
                .get(&config.pin)
                .copied()
                .unwrap_or(PinState::Low);

            let final_state = if config.invert {
                match state {
                    PinState::High => PinState::Low,
                    PinState::Low => PinState::High,
                }
            } else {
                state
            };

            result.values.push(GpioPinValue {
                name: config.name.clone(),
                pin: config.pin,
                direction: config.direction.clone(),
                state: final_state,
                timestamp: chrono::Utc::now().to_rfc3339(),
            });
        }
        result
    }

    pub fn is_available(&self) -> bool {
        #[cfg(all(target_os = "linux", feature = "gpio"))]
        {
            self.gpio.is_some()
        }
        #[cfg(not(all(target_os = "linux", feature = "gpio")))]
        {
            false
        }
    }

    pub fn pin_count(&self) -> usize {
        self.configs.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pin_state_conversion() {
        assert_eq!(PinState::from(true), PinState::High);
        assert_eq!(PinState::from(false), PinState::Low);
        assert!(bool::from(PinState::High));
        assert!(!bool::from(PinState::Low));
    }

    #[test]
    fn test_gpio_read_result_default() {
        let result = GpioReadResult::default();
        assert!(result.values.is_empty());
        assert!(result.errors.is_empty());
    }
}
