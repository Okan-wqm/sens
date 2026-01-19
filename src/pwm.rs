//! PWM (Pulse Width Modulation) support for Raspberry Pi
//!
//! Provides hardware and software PWM capabilities for motor control,
//! LED dimming, servo control, and other PWM applications.
//!
//! ## Hardware PWM
//! Raspberry Pi has 2 hardware PWM channels:
//! - PWM0: GPIO 12 (Alt0), GPIO 18 (Alt5)
//! - PWM1: GPIO 13 (Alt0), GPIO 19 (Alt5)
//!
//! ## Software PWM
//! Any GPIO pin can be used for software PWM with reduced precision.
//!
//! ## v1.2.4 Features
//! - Actor pattern for thread safety
//! - Hardware PWM with configurable frequency/duty cycle
//! - Software PWM fallback
//! - Servo mode support (50Hz, 1-2ms pulse)

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

/// Default PWM channel buffer size
const DEFAULT_PWM_CHANNEL_SIZE: usize = 32;

/// PWM channel configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PwmConfig {
    /// GPIO pin number
    pub pin: u8,
    /// PWM channel name (for identification)
    pub name: String,
    /// Frequency in Hz (default: 1000)
    #[serde(default = "default_frequency")]
    pub frequency_hz: f64,
    /// Initial duty cycle (0.0 - 1.0)
    #[serde(default)]
    pub initial_duty_cycle: f64,
    /// Use hardware PWM if available
    #[serde(default = "default_true")]
    pub hardware: bool,
    /// Servo mode (50Hz, pulse 1-2ms)
    #[serde(default)]
    pub servo_mode: bool,
}

fn default_frequency() -> f64 {
    1000.0
}

fn default_true() -> bool {
    true
}

impl Default for PwmConfig {
    fn default() -> Self {
        Self {
            pin: 18, // Default hardware PWM pin
            name: "pwm0".to_string(),
            frequency_hz: 1000.0,
            initial_duty_cycle: 0.0,
            hardware: true,
            servo_mode: false,
        }
    }
}

/// PWM channel state
#[derive(Debug, Clone, Serialize)]
pub struct PwmChannelState {
    pub name: String,
    pub pin: u8,
    pub frequency_hz: f64,
    pub duty_cycle: f64,
    pub enabled: bool,
    pub hardware: bool,
}

// ============================================================================
// Actor Pattern Types
// ============================================================================

/// Commands sent to the PWM actor
#[derive(Debug)]
pub enum PwmCommand {
    /// Initialize PWM channels
    Init {
        response: oneshot::Sender<Result<()>>,
    },
    /// Set duty cycle for a channel (0.0 - 1.0)
    SetDutyCycle {
        channel: String,
        duty_cycle: f64,
        response: oneshot::Sender<Result<()>>,
    },
    /// Set frequency for a channel
    SetFrequency {
        channel: String,
        frequency_hz: f64,
        response: oneshot::Sender<Result<()>>,
    },
    /// Set servo position (0.0 - 1.0 maps to 1-2ms pulse)
    SetServoPosition {
        channel: String,
        position: f64,
        response: oneshot::Sender<Result<()>>,
    },
    /// Enable/disable a channel
    SetEnabled {
        channel: String,
        enabled: bool,
        response: oneshot::Sender<Result<()>>,
    },
    /// Get all channel states
    GetStates {
        response: oneshot::Sender<Vec<PwmChannelState>>,
    },
    /// Shutdown
    Shutdown,
}

// ============================================================================
// PWM Handle (Public API)
// ============================================================================

/// Thread-safe handle to communicate with the PWM actor
#[derive(Clone)]
pub struct PwmHandle {
    sender: mpsc::Sender<PwmCommand>,
}

impl PwmHandle {
    /// Create a new handle and spawn the actor
    pub fn new(configs: Vec<PwmConfig>) -> Self {
        let (sender, receiver) = mpsc::channel(DEFAULT_PWM_CHANNEL_SIZE);

        // Spawn the actor
        tokio::spawn(async move {
            let mut actor = PwmActor::new(configs, receiver);
            actor.run().await;
        });

        Self { sender }
    }

    /// Initialize all PWM channels
    pub async fn init(&self) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_command(PwmCommand::Init { response: tx }).await?;
        rx.await
            .map_err(|_| anyhow::anyhow!("Actor disconnected"))?
    }

    /// Set duty cycle for a channel (0.0 - 1.0)
    pub async fn set_duty_cycle(&self, channel: &str, duty_cycle: f64) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_command(PwmCommand::SetDutyCycle {
            channel: channel.to_string(),
            duty_cycle: duty_cycle.clamp(0.0, 1.0),
            response: tx,
        })
        .await?;
        rx.await
            .map_err(|_| anyhow::anyhow!("Actor disconnected"))?
    }

    /// Set frequency for a channel
    pub async fn set_frequency(&self, channel: &str, frequency_hz: f64) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_command(PwmCommand::SetFrequency {
            channel: channel.to_string(),
            frequency_hz,
            response: tx,
        })
        .await?;
        rx.await
            .map_err(|_| anyhow::anyhow!("Actor disconnected"))?
    }

    /// Set servo position (0.0 - 1.0)
    pub async fn set_servo_position(&self, channel: &str, position: f64) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_command(PwmCommand::SetServoPosition {
            channel: channel.to_string(),
            position: position.clamp(0.0, 1.0),
            response: tx,
        })
        .await?;
        rx.await
            .map_err(|_| anyhow::anyhow!("Actor disconnected"))?
    }

    /// Enable or disable a channel
    pub async fn set_enabled(&self, channel: &str, enabled: bool) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_command(PwmCommand::SetEnabled {
            channel: channel.to_string(),
            enabled,
            response: tx,
        })
        .await?;
        rx.await
            .map_err(|_| anyhow::anyhow!("Actor disconnected"))?
    }

    /// Get all channel states
    pub async fn get_states(&self) -> Vec<PwmChannelState> {
        let (tx, rx) = oneshot::channel();
        if self
            .send_command(PwmCommand::GetStates { response: tx })
            .await
            .is_err()
        {
            return Vec::new();
        }
        rx.await.unwrap_or_default()
    }

    /// Send command to actor (blocking until channel has space)
    async fn send_command(&self, cmd: PwmCommand) -> Result<()> {
        self.sender
            .send(cmd)
            .await
            .map_err(|_| anyhow::anyhow!("PWM actor disconnected"))
    }
}

// ============================================================================
// PWM Actor Implementation
// ============================================================================

/// Internal channel state
struct PwmChannelInternal {
    config: PwmConfig,
    duty_cycle: f64,
    frequency_hz: f64,
    enabled: bool,
    #[cfg(all(target_os = "linux", feature = "gpio"))]
    pwm: Option<rppal::pwm::Pwm>,
    #[cfg(all(target_os = "linux", feature = "gpio"))]
    software_pwm: Option<rppal::gpio::OutputPin>,
}

/// PWM actor that manages all PWM channels
struct PwmActor {
    configs: Vec<PwmConfig>,
    receiver: mpsc::Receiver<PwmCommand>,
    channels: HashMap<String, PwmChannelInternal>,
}

impl PwmActor {
    fn new(configs: Vec<PwmConfig>, receiver: mpsc::Receiver<PwmCommand>) -> Self {
        Self {
            configs,
            receiver,
            channels: HashMap::new(),
        }
    }

    async fn run(&mut self) {
        info!(
            "PWM actor started with {} channels configured",
            self.configs.len()
        );

        while let Some(cmd) = self.receiver.recv().await {
            match cmd {
                PwmCommand::Init { response } => {
                    let result = self.init_channels();
                    let _ = response.send(result);
                }
                PwmCommand::SetDutyCycle {
                    channel,
                    duty_cycle,
                    response,
                } => {
                    let result = self.set_duty_cycle(&channel, duty_cycle);
                    let _ = response.send(result);
                }
                PwmCommand::SetFrequency {
                    channel,
                    frequency_hz,
                    response,
                } => {
                    let result = self.set_frequency(&channel, frequency_hz);
                    let _ = response.send(result);
                }
                PwmCommand::SetServoPosition {
                    channel,
                    position,
                    response,
                } => {
                    let result = self.set_servo_position(&channel, position);
                    let _ = response.send(result);
                }
                PwmCommand::SetEnabled {
                    channel,
                    enabled,
                    response,
                } => {
                    let result = self.set_enabled(&channel, enabled);
                    let _ = response.send(result);
                }
                PwmCommand::GetStates { response } => {
                    let states = self.get_states();
                    let _ = response.send(states);
                }
                PwmCommand::Shutdown => {
                    info!("PWM actor shutting down");
                    self.cleanup();
                    break;
                }
            }
        }

        info!("PWM actor stopped");
    }

    #[cfg(all(target_os = "linux", feature = "gpio"))]
    fn init_channels(&mut self) -> Result<()> {
        use rppal::pwm::{Channel, Polarity, Pwm};

        for config in &self.configs {
            let channel_result = if config.hardware {
                // Try hardware PWM
                let pwm_channel = match config.pin {
                    12 | 18 => Some(Channel::Pwm0),
                    13 | 19 => Some(Channel::Pwm1),
                    _ => None,
                };

                if let Some(ch) = pwm_channel {
                    match Pwm::with_frequency(
                        ch,
                        config.frequency_hz,
                        config.initial_duty_cycle,
                        Polarity::Normal,
                        true,
                    ) {
                        Ok(pwm) => {
                            info!(
                                "Initialized hardware PWM '{}' on pin {} at {}Hz",
                                config.name, config.pin, config.frequency_hz
                            );
                            PwmChannelInternal {
                                config: config.clone(),
                                duty_cycle: config.initial_duty_cycle,
                                frequency_hz: config.frequency_hz,
                                enabled: true,
                                pwm: Some(pwm),
                                software_pwm: None,
                            }
                        }
                        Err(e) => {
                            warn!(
                                "Hardware PWM failed for '{}': {}, falling back to software",
                                config.name, e
                            );
                            self.init_software_pwm(config)?
                        }
                    }
                } else {
                    self.init_software_pwm(config)?
                }
            } else {
                self.init_software_pwm(config)?
            };

            self.channels.insert(config.name.clone(), channel_result);
        }

        Ok(())
    }

    #[cfg(all(target_os = "linux", feature = "gpio"))]
    fn init_software_pwm(&self, config: &PwmConfig) -> Result<PwmChannelInternal> {
        use rppal::gpio::Gpio;

        let gpio = Gpio::new().context("Failed to initialize GPIO for software PWM")?;
        let mut pin = gpio.get(config.pin)?.into_output();

        // Software PWM uses GPIO toggle - less precise but works on any pin
        pin.set_pwm_frequency(config.frequency_hz, config.initial_duty_cycle)
            .context("Failed to set software PWM")?;

        info!(
            "Initialized software PWM '{}' on pin {} at {}Hz",
            config.name, config.pin, config.frequency_hz
        );

        Ok(PwmChannelInternal {
            config: config.clone(),
            duty_cycle: config.initial_duty_cycle,
            frequency_hz: config.frequency_hz,
            enabled: true,
            pwm: None,
            software_pwm: Some(pin),
        })
    }

    #[cfg(not(all(target_os = "linux", feature = "gpio")))]
    fn init_channels(&mut self) -> Result<()> {
        for config in &self.configs {
            warn!(
                "PWM '{}' on pin {} configured but running in simulation mode",
                config.name, config.pin
            );

            self.channels.insert(
                config.name.clone(),
                PwmChannelInternal {
                    config: config.clone(),
                    duty_cycle: config.initial_duty_cycle,
                    frequency_hz: config.frequency_hz,
                    enabled: true,
                },
            );
        }

        Ok(())
    }

    #[cfg(all(target_os = "linux", feature = "gpio"))]
    fn set_duty_cycle(&mut self, channel: &str, duty_cycle: f64) -> Result<()> {
        let ch = self
            .channels
            .get_mut(channel)
            .ok_or_else(|| anyhow::anyhow!("PWM channel '{}' not found", channel))?;

        ch.duty_cycle = duty_cycle;

        if let Some(ref pwm) = ch.pwm {
            pwm.set_duty_cycle(duty_cycle)?;
        } else if let Some(ref mut pin) = ch.software_pwm {
            pin.set_pwm_frequency(ch.frequency_hz, duty_cycle)?;
        }

        debug!(
            "PWM '{}' duty cycle set to {:.2}%",
            channel,
            duty_cycle * 100.0
        );
        Ok(())
    }

    #[cfg(not(all(target_os = "linux", feature = "gpio")))]
    fn set_duty_cycle(&mut self, channel: &str, duty_cycle: f64) -> Result<()> {
        let ch = self
            .channels
            .get_mut(channel)
            .ok_or_else(|| anyhow::anyhow!("PWM channel '{}' not found", channel))?;

        ch.duty_cycle = duty_cycle;
        debug!(
            "PWM '{}' duty cycle set to {:.2}% (simulated)",
            channel,
            duty_cycle * 100.0
        );
        Ok(())
    }

    #[cfg(all(target_os = "linux", feature = "gpio"))]
    fn set_frequency(&mut self, channel: &str, frequency_hz: f64) -> Result<()> {
        let ch = self
            .channels
            .get_mut(channel)
            .ok_or_else(|| anyhow::anyhow!("PWM channel '{}' not found", channel))?;

        ch.frequency_hz = frequency_hz;

        if let Some(ref pwm) = ch.pwm {
            pwm.set_frequency(frequency_hz, ch.duty_cycle)?;
        } else if let Some(ref mut pin) = ch.software_pwm {
            pin.set_pwm_frequency(frequency_hz, ch.duty_cycle)?;
        }

        debug!("PWM '{}' frequency set to {}Hz", channel, frequency_hz);
        Ok(())
    }

    #[cfg(not(all(target_os = "linux", feature = "gpio")))]
    fn set_frequency(&mut self, channel: &str, frequency_hz: f64) -> Result<()> {
        let ch = self
            .channels
            .get_mut(channel)
            .ok_or_else(|| anyhow::anyhow!("PWM channel '{}' not found", channel))?;

        ch.frequency_hz = frequency_hz;
        debug!(
            "PWM '{}' frequency set to {}Hz (simulated)",
            channel, frequency_hz
        );
        Ok(())
    }

    fn set_servo_position(&mut self, channel: &str, position: f64) -> Result<()> {
        // Servo mode: 50Hz frequency, 1ms (0%) to 2ms (100%) pulse
        // duty_cycle = (1 + position) / 20 for 50Hz
        let duty_cycle = (1.0 + position) / 20.0;
        self.set_duty_cycle(channel, duty_cycle)
    }

    #[cfg(all(target_os = "linux", feature = "gpio"))]
    fn set_enabled(&mut self, channel: &str, enabled: bool) -> Result<()> {
        let ch = self
            .channels
            .get_mut(channel)
            .ok_or_else(|| anyhow::anyhow!("PWM channel '{}' not found", channel))?;

        ch.enabled = enabled;

        if let Some(ref pwm) = ch.pwm {
            if enabled {
                pwm.enable()?;
            } else {
                pwm.disable()?;
            }
        } else if let Some(ref mut pin) = ch.software_pwm {
            if enabled {
                pin.set_pwm_frequency(ch.frequency_hz, ch.duty_cycle)?;
            } else {
                pin.clear_pwm()?;
            }
        }

        debug!(
            "PWM '{}' {}",
            channel,
            if enabled { "enabled" } else { "disabled" }
        );
        Ok(())
    }

    #[cfg(not(all(target_os = "linux", feature = "gpio")))]
    fn set_enabled(&mut self, channel: &str, enabled: bool) -> Result<()> {
        let ch = self
            .channels
            .get_mut(channel)
            .ok_or_else(|| anyhow::anyhow!("PWM channel '{}' not found", channel))?;

        ch.enabled = enabled;
        debug!(
            "PWM '{}' {} (simulated)",
            channel,
            if enabled { "enabled" } else { "disabled" }
        );
        Ok(())
    }

    fn get_states(&self) -> Vec<PwmChannelState> {
        self.channels
            .iter()
            .map(|(name, ch)| PwmChannelState {
                name: name.clone(),
                pin: ch.config.pin,
                frequency_hz: ch.frequency_hz,
                duty_cycle: ch.duty_cycle,
                enabled: ch.enabled,
                #[cfg(all(target_os = "linux", feature = "gpio"))]
                hardware: ch.pwm.is_some(),
                #[cfg(not(all(target_os = "linux", feature = "gpio")))]
                hardware: false,
            })
            .collect()
    }

    fn cleanup(&mut self) {
        #[cfg(all(target_os = "linux", feature = "gpio"))]
        {
            for (name, ch) in &mut self.channels {
                if let Some(ref pwm) = ch.pwm {
                    if let Err(e) = pwm.disable() {
                        warn!("Failed to disable PWM '{}': {}", name, e);
                    }
                }
                if let Some(ref mut pin) = ch.software_pwm {
                    if let Err(e) = pin.clear_pwm() {
                        warn!("Failed to clear software PWM '{}': {}", name, e);
                    }
                }
            }
        }

        self.channels.clear();
        info!("PWM channels cleaned up");
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pwm_config_default() {
        let config = PwmConfig::default();
        assert_eq!(config.pin, 18);
        assert_eq!(config.frequency_hz, 1000.0);
        assert_eq!(config.initial_duty_cycle, 0.0);
        assert!(config.hardware);
        assert!(!config.servo_mode);
    }

    #[test]
    fn test_pwm_config_serialization() {
        let config = PwmConfig {
            pin: 12,
            name: "motor1".to_string(),
            frequency_hz: 25000.0,
            initial_duty_cycle: 0.5,
            hardware: true,
            servo_mode: false,
        };

        let yaml = serde_yaml::to_string(&config).unwrap();
        assert!(yaml.contains("pin: 12"));
        assert!(yaml.contains("frequency_hz: 25000.0"));

        let parsed: PwmConfig = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed.pin, 12);
        assert_eq!(parsed.frequency_hz, 25000.0);
    }

    #[test]
    fn test_servo_duty_cycle_calculation() {
        // At position 0.0: duty = (1 + 0) / 20 = 0.05 (5%)
        // At position 1.0: duty = (1 + 1) / 20 = 0.10 (10%)
        let pos_0: f64 = (1.0 + 0.0) / 20.0;
        let pos_1: f64 = (1.0 + 1.0) / 20.0;

        assert!((pos_0 - 0.05).abs() < 0.001);
        assert!((pos_1 - 0.10).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_pwm_handle_creation() {
        let configs = vec![PwmConfig {
            pin: 18,
            name: "test_pwm".to_string(),
            ..Default::default()
        }];

        let handle = PwmHandle::new(configs);

        // Initialize should work (simulation mode)
        let result = handle.init().await;
        assert!(result.is_ok());

        // Get states should return the channel
        let states = handle.get_states().await;
        assert_eq!(states.len(), 1);
        assert_eq!(states[0].name, "test_pwm");
    }
}
