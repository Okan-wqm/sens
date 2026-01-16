//! Telemetry collection for system metrics
//!
//! Collects CPU, memory, disk, temperature, network metrics,
//! and hardware data (Modbus, GPIO) and publishes them to the cloud via MQTT.

use std::sync::Arc;
use std::time::{Duration, Instant};
use sysinfo::{Components, Disks, Networks, System};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::gpio::PinState;
use crate::mqtt::{
    DeviceStatus, GpioPinData, ModbusDeviceData, ModbusRegisterData, TelemetryMetrics,
};
use crate::AppState;

/// Telemetry collector
pub struct TelemetryCollector {
    state: Arc<RwLock<AppState>>,
    system: System,
    networks: Networks,
    disks: Disks,
    components: Components,
    start_time: Instant,
}

impl TelemetryCollector {
    /// Create a new telemetry collector
    pub fn new(state: Arc<RwLock<AppState>>) -> Self {
        Self {
            state,
            system: System::new_all(),
            networks: Networks::new_with_refreshed_list(),
            disks: Disks::new_with_refreshed_list(),
            components: Components::new_with_refreshed_list(),
            start_time: Instant::now(),
        }
    }

    /// Run the telemetry collection loop
    pub async fn run(mut self) {
        info!("Telemetry collector started");

        let interval = {
            let state = self.state.read().await;
            Duration::from_secs(state.config.telemetry.interval_seconds)
        };

        let mut status_counter = 0u32;

        loop {
            tokio::time::sleep(interval).await;

            // Collect and publish telemetry
            if let Err(e) = self.collect_and_publish().await {
                warn!("Failed to publish telemetry: {}", e);
            }

            // Publish status every 3rd telemetry cycle (to reduce traffic)
            status_counter += 1;
            if status_counter >= 3 {
                status_counter = 0;
                if let Err(e) = self.publish_status().await {
                    warn!("Failed to publish status: {}", e);
                }
            }
        }
    }

    /// Collect metrics and publish via MQTT
    async fn collect_and_publish(&mut self) -> anyhow::Result<()> {
        // Refresh system info
        self.system.refresh_all();
        self.networks.refresh();
        self.disks.refresh();
        self.components.refresh();

        // Get config for what to include
        let config = {
            let state = self.state.read().await;
            state.config.telemetry.clone()
        };

        // Build metrics
        let mut metrics = TelemetryMetrics::default();

        // CPU metrics
        if config.include_cpu {
            let cpus = self.system.cpus();
            if !cpus.is_empty() {
                let total_usage: f32 = cpus.iter().map(|c| c.cpu_usage()).sum();
                metrics.cpu_usage_percent = Some(total_usage / cpus.len() as f32);
            }
        }

        // Memory metrics
        if config.include_memory {
            let total_mem = self.system.total_memory();
            let used_mem = self.system.used_memory();

            if total_mem > 0 {
                metrics.memory_usage_percent = Some((used_mem as f32 / total_mem as f32) * 100.0);
                metrics.memory_used_mb = Some(used_mem / 1024 / 1024);
                metrics.memory_total_mb = Some(total_mem / 1024 / 1024);
            }
        }

        // Disk metrics (root partition)
        if config.include_disk {
            if let Some(disk) = self.disks.list().first() {
                let total = disk.total_space();
                let available = disk.available_space();
                let used = total.saturating_sub(available);

                if total > 0 {
                    metrics.disk_usage_percent = Some((used as f32 / total as f32) * 100.0);
                    metrics.disk_used_gb = Some(used as f64 / 1024.0 / 1024.0 / 1024.0);
                    metrics.disk_total_gb = Some(total as f64 / 1024.0 / 1024.0 / 1024.0);
                }
            }
        }

        // Temperature (CPU temp if available)
        if config.include_temperature {
            metrics.temperature_celsius = self.get_cpu_temperature();
        }

        // Network metrics (aggregate all interfaces)
        let (rx_bytes, tx_bytes) = self.get_network_bytes();
        if rx_bytes > 0 || tx_bytes > 0 {
            metrics.network_rx_bytes = Some(rx_bytes);
            metrics.network_tx_bytes = Some(tx_bytes);
        }

        // Collect hardware data (Modbus, GPIO)
        self.collect_hardware_data(&mut metrics).await;

        // Publish via MQTT
        let state = self.state.read().await;
        if let Some(ref mqtt) = state.mqtt_client {
            mqtt.publish_telemetry(metrics).await?;
            debug!("Telemetry published");
        }

        Ok(())
    }

    /// Publish device status
    async fn publish_status(&self) -> anyhow::Result<()> {
        let uptime = self.start_time.elapsed().as_secs();

        let state = self.state.read().await;
        if let Some(ref mqtt) = state.mqtt_client {
            mqtt.publish_status(DeviceStatus::Online, uptime).await?;
            debug!("Status published");
        }

        Ok(())
    }

    /// Get CPU temperature (Linux-specific)
    fn get_cpu_temperature(&self) -> Option<f32> {
        // Try using sysinfo components
        for component in self.components.iter() {
            let label = component.label().to_lowercase();
            if label.contains("cpu") || label.contains("core") || label.contains("package") {
                return Some(component.temperature());
            }
        }

        // Fallback: Try reading from thermal zone (Raspberry Pi, etc.)
        #[cfg(target_os = "linux")]
        {
            if let Ok(content) = std::fs::read_to_string("/sys/class/thermal/thermal_zone0/temp") {
                if let Ok(millidegrees) = content.trim().parse::<i32>() {
                    return Some(millidegrees as f32 / 1000.0);
                }
            }
        }

        None
    }

    /// Get network bytes (rx, tx) for all interfaces
    fn get_network_bytes(&self) -> (u64, u64) {
        let mut total_rx = 0u64;
        let mut total_tx = 0u64;

        for (interface_name, data) in self.networks.iter() {
            // Skip loopback and virtual interfaces
            if interface_name.starts_with("lo")
                || interface_name.starts_with("veth")
                || interface_name.starts_with("docker")
                || interface_name.starts_with("br-")
            {
                continue;
            }

            total_rx += data.total_received();
            total_tx += data.total_transmitted();
        }

        (total_rx, total_tx)
    }

    /// Collect hardware data (Modbus, GPIO) into metrics
    ///
    /// v2.0: Uses actor pattern for both GPIO and Modbus
    async fn collect_hardware_data(&self, metrics: &mut TelemetryMetrics) {
        // Collect GPIO data via actor handle (v2.0)
        let gpio_handle = {
            let state = self.state.read().await;
            state.gpio_handle.clone()
        };

        if let Some(handle) = gpio_handle {
            let gpio_result = handle.read_all().await;
            let gpio_data: Vec<GpioPinData> = gpio_result
                .values
                .iter()
                .map(|v| GpioPinData {
                    name: v.name.clone(),
                    pin: v.pin,
                    direction: v.direction.clone(),
                    state: match v.state {
                        PinState::High => "high".to_string(),
                        PinState::Low => "low".to_string(),
                    },
                })
                .collect();

            if !gpio_data.is_empty() {
                metrics.gpio = Some(gpio_data);
            }

            // Log GPIO errors if any
            for error in &gpio_result.errors {
                warn!("GPIO error: {}", error);
            }
        }

        // Collect Modbus data via thread-safe handle (actor pattern)
        let modbus_handle = {
            let state = self.state.read().await;
            state.modbus_handle.clone()
        };

        if let Some(handle) = modbus_handle {
            let modbus_results = handle.read_all().await;
            let mut modbus_data = Vec::new();

            for result in modbus_results {
                let registers: Vec<ModbusRegisterData> = result
                    .values
                    .iter()
                    .map(|v| ModbusRegisterData {
                        name: v.name.clone(),
                        address: v.address,
                        value: v.scaled_value,
                        unit: v.unit.clone(),
                    })
                    .collect();

                modbus_data.push(ModbusDeviceData {
                    device_name: result.device_name,
                    registers,
                    errors: result.errors,
                });
            }

            if !modbus_data.is_empty() {
                metrics.modbus = Some(modbus_data);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_telemetry_metrics_serialization() {
        let metrics = TelemetryMetrics {
            cpu_usage_percent: Some(45.5),
            memory_usage_percent: Some(62.3),
            memory_used_mb: Some(4096),
            memory_total_mb: Some(8192),
            disk_usage_percent: None,
            disk_used_gb: None,
            disk_total_gb: None,
            temperature_celsius: Some(55.0),
            network_rx_bytes: None,
            network_tx_bytes: None,
            modbus: None,
            gpio: None,
        };

        let json = serde_json::to_string(&metrics).unwrap();
        assert!(json.contains("cpu_usage_percent"));
        assert!(json.contains("45.5"));
        assert!(!json.contains("disk_usage_percent")); // None fields skipped
        assert!(!json.contains("modbus")); // None fields skipped
    }

    #[test]
    fn test_telemetry_with_hardware_data() {
        let metrics = TelemetryMetrics {
            cpu_usage_percent: Some(50.0),
            memory_usage_percent: None,
            memory_used_mb: None,
            memory_total_mb: None,
            disk_usage_percent: None,
            disk_used_gb: None,
            disk_total_gb: None,
            temperature_celsius: None,
            network_rx_bytes: None,
            network_tx_bytes: None,
            modbus: Some(vec![ModbusDeviceData {
                device_name: "PLC-1".to_string(),
                registers: vec![ModbusRegisterData {
                    name: "water_temp".to_string(),
                    address: 100,
                    value: 22.5,
                    unit: Some("°C".to_string()),
                }],
                errors: vec![],
            }]),
            gpio: Some(vec![GpioPinData {
                name: "pump_status".to_string(),
                pin: 17,
                direction: "input".to_string(),
                state: "high".to_string(),
            }]),
        };

        let json = serde_json::to_string(&metrics).unwrap();
        assert!(json.contains("modbus"));
        assert!(json.contains("PLC-1"));
        assert!(json.contains("water_temp"));
        assert!(json.contains("gpio"));
        assert!(json.contains("pump_status"));
    }
}
