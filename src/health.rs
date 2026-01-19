//! Health Check HTTP Endpoint
//!
//! Provides a lightweight HTTP server for health checks and readiness probes.
//! Used by orchestrators (Docker, Kubernetes, systemd) to monitor agent status.
//!
//! # Endpoints
//! - `GET /health` - Basic health check (always returns 200 if server is running)
//! - `GET /ready` - Readiness check (returns 200 only when fully initialized)
//! - `GET /metrics` - Basic metrics (queue size, uptime, connections)
//! - `GET /diagnostics` - Comprehensive diagnostics for remote troubleshooting (v1.2.4)
//!
//! # Configuration
//! Enable with the `health` feature flag in Cargo.toml.
//!
//! # IEC 62443 SL2 Compliance
//! - FR6: Timely Response to Events (health monitoring)
//! - FR7: Resource Availability (diagnostics for troubleshooting)

// v1.2.4: API reserved for health feature - silence dead_code warnings
#![allow(dead_code)]

use serde::Serialize;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tracing::{error, info};

/// Health status response
#[derive(Debug, Clone, Serialize)]
pub struct HealthResponse {
    /// Service status ("healthy", "degraded", "unhealthy")
    pub status: &'static str,
    /// Service version
    pub version: &'static str,
    /// Uptime in seconds
    pub uptime_secs: u64,
}

/// Readiness status response
#[derive(Debug, Clone, Serialize)]
pub struct ReadinessResponse {
    /// Whether the service is ready to accept traffic
    pub ready: bool,
    /// Individual component checks
    pub checks: ReadinessChecks,
}

/// Individual readiness checks
#[derive(Debug, Clone, Serialize)]
pub struct ReadinessChecks {
    /// Configuration loaded
    pub config_loaded: bool,
    /// MQTT connected
    pub mqtt_connected: bool,
    /// Device activated (provisioned)
    pub device_activated: bool,
}

/// Basic metrics response
#[derive(Debug, Clone, Serialize)]
pub struct MetricsResponse {
    /// Uptime in seconds
    pub uptime_secs: u64,
    /// MQTT messages sent
    pub mqtt_messages_sent: u64,
    /// MQTT messages received
    pub mqtt_messages_received: u64,
    /// Modbus read count
    pub modbus_reads: u64,
    /// Script executions
    pub script_executions: u64,
    /// Offline queue size
    pub offline_queue_size: u64,
}

/// Comprehensive diagnostics response for troubleshooting (v1.2.4)
#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticsResponse {
    /// Timestamp of diagnostics collection
    pub timestamp: String,
    /// Agent version
    pub version: &'static str,
    /// Uptime in seconds
    pub uptime_secs: u64,
    /// System information
    pub system: SystemDiagnostics,
    /// Process information
    pub process: ProcessDiagnostics,
    /// Component status
    pub components: ComponentDiagnostics,
    /// Configuration summary (sanitized)
    pub config: ConfigDiagnostics,
    /// Recent errors (last 10)
    pub recent_errors: Vec<String>,
}

/// System-level diagnostics
#[derive(Debug, Clone, Serialize)]
pub struct SystemDiagnostics {
    /// Operating system
    pub os: String,
    /// Hostname
    pub hostname: String,
    /// CPU count
    pub cpu_count: usize,
    /// CPU usage percentage
    pub cpu_usage_percent: f32,
    /// Total memory (bytes)
    pub memory_total_bytes: u64,
    /// Used memory (bytes)
    pub memory_used_bytes: u64,
    /// Memory usage percentage
    pub memory_usage_percent: f32,
    /// Disk information
    pub disk: DiskDiagnostics,
}

/// Disk diagnostics
#[derive(Debug, Clone, Serialize)]
pub struct DiskDiagnostics {
    /// Total disk space (bytes)
    pub total_bytes: u64,
    /// Available disk space (bytes)
    pub available_bytes: u64,
    /// Usage percentage
    pub usage_percent: f32,
}

/// Process-level diagnostics
#[derive(Debug, Clone, Serialize)]
pub struct ProcessDiagnostics {
    /// Process ID
    pub pid: u32,
    /// Process memory usage (bytes)
    pub memory_bytes: u64,
    /// Number of threads
    pub thread_count: u32,
    /// Process start time (ISO 8601)
    pub start_time: String,
}

/// Component status diagnostics
#[derive(Debug, Clone, Serialize)]
pub struct ComponentDiagnostics {
    /// MQTT connection status
    pub mqtt: MqttDiagnostics,
    /// Modbus clients status
    pub modbus: ModbusDiagnostics,
    /// Script engine status
    pub scripts: ScriptDiagnostics,
    /// Function blocks status
    pub function_blocks: FunctionBlockDiagnostics,
    /// Offline queue status
    pub offline_queue: OfflineQueueDiagnostics,
}

/// MQTT connection diagnostics
#[derive(Debug, Clone, Serialize)]
pub struct MqttDiagnostics {
    /// Whether connected
    pub connected: bool,
    /// Messages sent
    pub messages_sent: u64,
    /// Messages received
    pub messages_received: u64,
    /// Last connection time
    pub last_connected: Option<String>,
}

/// Modbus diagnostics
#[derive(Debug, Clone, Serialize)]
pub struct ModbusDiagnostics {
    /// Number of configured clients
    pub client_count: usize,
    /// Total reads performed
    pub total_reads: u64,
    /// Read errors
    pub read_errors: u64,
    /// Circuit breaker states
    pub circuit_states: Vec<(String, String)>,
}

/// Script engine diagnostics
#[derive(Debug, Clone, Serialize)]
pub struct ScriptDiagnostics {
    /// Number of loaded scripts
    pub loaded_count: usize,
    /// Number of active scripts
    pub active_count: usize,
    /// Total executions
    pub total_executions: u64,
    /// Execution errors
    pub execution_errors: u64,
}

/// Function block diagnostics
#[derive(Debug, Clone, Serialize)]
pub struct FunctionBlockDiagnostics {
    /// Total FB instances
    pub instance_count: usize,
    /// FB types and counts
    pub type_counts: std::collections::HashMap<String, usize>,
}

/// Offline queue diagnostics
#[derive(Debug, Clone, Serialize)]
pub struct OfflineQueueDiagnostics {
    /// Current queue size
    pub size: u64,
    /// Queue capacity
    pub capacity: u64,
    /// Total messages queued
    pub total_queued: u64,
    /// Total messages sent from queue
    pub total_sent: u64,
}

/// Configuration diagnostics (sanitized - no secrets)
#[derive(Debug, Clone, Serialize)]
pub struct ConfigDiagnostics {
    /// Device ID (masked)
    pub device_id: String,
    /// MQTT broker host
    pub mqtt_host: String,
    /// Number of Modbus devices
    pub modbus_device_count: usize,
    /// Number of GPIO mappings
    pub gpio_mapping_count: usize,
    /// Telemetry interval (seconds)
    pub telemetry_interval_secs: u64,
}

/// Health check state shared with the main application
#[derive(Clone)]
pub struct HealthState {
    inner: Arc<HealthStateInner>,
}

struct HealthStateInner {
    /// When the service started
    start_time: Instant,
    /// Whether config is loaded
    config_loaded: AtomicBool,
    /// Whether MQTT is connected
    mqtt_connected: AtomicBool,
    /// Whether device is activated
    device_activated: AtomicBool,
    /// MQTT messages sent counter
    mqtt_sent: AtomicU64,
    /// MQTT messages received counter
    mqtt_received: AtomicU64,
    /// Modbus reads counter
    modbus_reads: AtomicU64,
    /// Modbus read errors counter (v1.2.4)
    modbus_errors: AtomicU64,
    /// Script executions counter
    script_executions: AtomicU64,
    /// Script execution errors (v1.2.4)
    script_errors: AtomicU64,
    /// Current offline queue size
    offline_queue_size: AtomicU64,
    /// Offline queue capacity (v1.2.4)
    offline_queue_capacity: AtomicU64,
    /// Total messages queued offline (v1.2.4)
    offline_total_queued: AtomicU64,
    /// Total messages sent from offline queue (v1.2.4)
    offline_total_sent: AtomicU64,
    /// Number of Modbus clients (v1.2.4)
    modbus_client_count: AtomicU64,
    /// Number of loaded scripts (v1.2.4)
    script_loaded_count: AtomicU64,
    /// Number of active scripts (v1.2.4)
    script_active_count: AtomicU64,
    /// Number of FB instances (v1.2.4)
    fb_instance_count: AtomicU64,
    /// Recent errors buffer (v1.2.4)
    /// v1.2.6: Changed to VecDeque for O(1) removal from front
    recent_errors: std::sync::RwLock<VecDeque<String>>,
    /// Config diagnostics (v1.2.4)
    config_diagnostics: std::sync::RwLock<Option<ConfigDiagnostics>>,
    /// MQTT last connected timestamp (v1.2.5)
    mqtt_last_connected: AtomicI64,
    /// Modbus circuit breaker states (v1.2.5)
    modbus_circuit_states: std::sync::RwLock<Vec<(String, String)>>,
    /// Function block type counts (v1.2.5)
    fb_type_counts: std::sync::RwLock<std::collections::HashMap<String, usize>>,
}

impl HealthState {
    /// Create a new health state
    pub fn new() -> Self {
        Self {
            inner: Arc::new(HealthStateInner {
                start_time: Instant::now(),
                config_loaded: AtomicBool::new(false),
                mqtt_connected: AtomicBool::new(false),
                device_activated: AtomicBool::new(false),
                mqtt_sent: AtomicU64::new(0),
                mqtt_received: AtomicU64::new(0),
                modbus_reads: AtomicU64::new(0),
                modbus_errors: AtomicU64::new(0),
                script_executions: AtomicU64::new(0),
                script_errors: AtomicU64::new(0),
                offline_queue_size: AtomicU64::new(0),
                offline_queue_capacity: AtomicU64::new(1000),
                offline_total_queued: AtomicU64::new(0),
                offline_total_sent: AtomicU64::new(0),
                modbus_client_count: AtomicU64::new(0),
                script_loaded_count: AtomicU64::new(0),
                script_active_count: AtomicU64::new(0),
                fb_instance_count: AtomicU64::new(0),
                recent_errors: std::sync::RwLock::new(VecDeque::with_capacity(10)),
                config_diagnostics: std::sync::RwLock::new(None),
                mqtt_last_connected: AtomicI64::new(0),
                modbus_circuit_states: std::sync::RwLock::new(Vec::new()),
                fb_type_counts: std::sync::RwLock::new(std::collections::HashMap::new()),
            }),
        }
    }

    /// Get uptime in seconds
    pub fn uptime_secs(&self) -> u64 {
        self.inner.start_time.elapsed().as_secs()
    }

    /// Set config loaded status
    pub fn set_config_loaded(&self, loaded: bool) {
        self.inner.config_loaded.store(loaded, Ordering::Release);
    }

    /// Set MQTT connected status
    pub fn set_mqtt_connected(&self, connected: bool) {
        self.inner
            .mqtt_connected
            .store(connected, Ordering::Release);
        // Track last connected time (v1.2.5)
        if connected {
            let now = chrono::Utc::now().timestamp();
            self.inner.mqtt_last_connected.store(now, Ordering::Release);
        }
    }

    /// Set device activated status
    pub fn set_device_activated(&self, activated: bool) {
        self.inner
            .device_activated
            .store(activated, Ordering::Release);
    }

    /// Increment MQTT sent counter
    pub fn inc_mqtt_sent(&self) {
        self.inner.mqtt_sent.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment MQTT received counter
    pub fn inc_mqtt_received(&self) {
        self.inner.mqtt_received.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment Modbus reads counter
    pub fn inc_modbus_reads(&self) {
        self.inner.modbus_reads.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment script executions counter
    pub fn inc_script_executions(&self) {
        self.inner.script_executions.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment Modbus errors counter (v1.2.4)
    pub fn inc_modbus_errors(&self) {
        self.inner.modbus_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment script errors counter (v1.2.4)
    pub fn inc_script_errors(&self) {
        self.inner.script_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Set Modbus client count (v1.2.4)
    pub fn set_modbus_client_count(&self, count: usize) {
        self.inner
            .modbus_client_count
            .store(count as u64, Ordering::Release);
    }

    /// Set script counts (v1.2.4)
    pub fn set_script_counts(&self, loaded: usize, active: usize) {
        self.inner
            .script_loaded_count
            .store(loaded as u64, Ordering::Release);
        self.inner
            .script_active_count
            .store(active as u64, Ordering::Release);
    }

    /// Set FB instance count (v1.2.4)
    pub fn set_fb_instance_count(&self, count: usize) {
        self.inner
            .fb_instance_count
            .store(count as u64, Ordering::Release);
    }

    /// Set Modbus circuit breaker states (v1.2.5)
    pub fn set_modbus_circuit_states(&self, states: Vec<(String, String)>) {
        if let Ok(mut guard) = self.inner.modbus_circuit_states.write() {
            *guard = states;
        }
    }

    /// Set function block type counts (v1.2.5)
    pub fn set_fb_type_counts(&self, counts: std::collections::HashMap<String, usize>) {
        if let Ok(mut guard) = self.inner.fb_type_counts.write() {
            *guard = counts;
        }
    }

    /// Set offline queue size
    pub fn set_offline_queue_size(&self, size: u64) {
        self.inner.offline_queue_size.store(size, Ordering::Release);
    }

    /// Set offline queue capacity (v1.2.4)
    pub fn set_offline_queue_capacity(&self, capacity: u64) {
        self.inner
            .offline_queue_capacity
            .store(capacity, Ordering::Release);
    }

    /// Increment offline messages queued (v1.2.4)
    pub fn inc_offline_queued(&self) {
        self.inner
            .offline_total_queued
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Increment offline messages sent (v1.2.4)
    pub fn inc_offline_sent(&self) {
        self.inner
            .offline_total_sent
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Add an error to the recent errors buffer (v1.2.4)
    /// v1.2.6: Uses VecDeque::pop_front() for O(1) removal instead of Vec::remove(0)
    pub fn add_error(&self, error: impl Into<String>) {
        if let Ok(mut errors) = self.inner.recent_errors.write() {
            if errors.len() >= 10 {
                errors.pop_front(); // O(1) instead of O(n)
            }
            let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
            errors.push_back(format!("[{}] {}", timestamp, error.into()));
        }
    }

    /// Set configuration diagnostics (v1.2.4)
    pub fn set_config_diagnostics(&self, diag: ConfigDiagnostics) {
        if let Ok(mut config) = self.inner.config_diagnostics.write() {
            *config = Some(diag);
        }
    }

    /// Check if ready (all components initialized)
    pub fn is_ready(&self) -> bool {
        self.inner.config_loaded.load(Ordering::Acquire)
            && self.inner.device_activated.load(Ordering::Acquire)
    }

    /// Get health response
    pub fn health(&self) -> HealthResponse {
        let status = if self.is_ready() {
            "healthy"
        } else if self.inner.config_loaded.load(Ordering::Acquire) {
            "degraded"
        } else {
            "unhealthy"
        };

        HealthResponse {
            status,
            version: env!("CARGO_PKG_VERSION"),
            uptime_secs: self.uptime_secs(),
        }
    }

    /// Get readiness response
    pub fn readiness(&self) -> ReadinessResponse {
        ReadinessResponse {
            ready: self.is_ready(),
            checks: ReadinessChecks {
                config_loaded: self.inner.config_loaded.load(Ordering::Acquire),
                mqtt_connected: self.inner.mqtt_connected.load(Ordering::Acquire),
                device_activated: self.inner.device_activated.load(Ordering::Acquire),
            },
        }
    }

    /// Get metrics response
    pub fn metrics(&self) -> MetricsResponse {
        MetricsResponse {
            uptime_secs: self.uptime_secs(),
            mqtt_messages_sent: self.inner.mqtt_sent.load(Ordering::Acquire),
            mqtt_messages_received: self.inner.mqtt_received.load(Ordering::Acquire),
            modbus_reads: self.inner.modbus_reads.load(Ordering::Acquire),
            script_executions: self.inner.script_executions.load(Ordering::Acquire),
            offline_queue_size: self.inner.offline_queue_size.load(Ordering::Acquire),
        }
    }

    /// Get comprehensive diagnostics response (v1.2.4)
    pub fn diagnostics(&self) -> DiagnosticsResponse {
        use sysinfo::{Disks, System};

        // Collect system information
        let mut sys = System::new_all();
        sys.refresh_all();

        let cpu_usage = sys.global_cpu_usage();
        let total_memory = sys.total_memory();
        let used_memory = sys.used_memory();

        // Get disk info for root partition
        let disks = Disks::new_with_refreshed_list();
        let (disk_total, disk_available) = disks
            .iter()
            .find(|d| d.mount_point().to_string_lossy() == "/")
            .map(|d| (d.total_space(), d.available_space()))
            .unwrap_or((0, 0));

        let disk_usage_percent = if disk_total > 0 {
            ((disk_total - disk_available) as f32 / disk_total as f32) * 100.0
        } else {
            0.0
        };

        // Get process info
        let pid = std::process::id();
        let (proc_memory, thread_count) = sys
            .process(sysinfo::Pid::from_u32(pid))
            .map(|p| (p.memory(), 0u32)) // thread count not directly available
            .unwrap_or((0, 0));

        // Recent errors (v1.2.6: convert VecDeque to Vec for serialization)
        let recent_errors: Vec<String> = self
            .inner
            .recent_errors
            .read()
            .map(|e| e.iter().cloned().collect())
            .unwrap_or_default();

        // Config diagnostics
        let config = self
            .inner
            .config_diagnostics
            .read()
            .map(|c| c.clone())
            .unwrap_or(None)
            .unwrap_or(ConfigDiagnostics {
                device_id: "not_configured".to_string(),
                mqtt_host: "not_configured".to_string(),
                modbus_device_count: 0,
                gpio_mapping_count: 0,
                telemetry_interval_secs: 0,
            });

        DiagnosticsResponse {
            timestamp: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            version: env!("CARGO_PKG_VERSION"),
            uptime_secs: self.uptime_secs(),
            system: SystemDiagnostics {
                os: System::long_os_version().unwrap_or_else(|| "unknown".to_string()),
                hostname: System::host_name().unwrap_or_else(|| "unknown".to_string()),
                cpu_count: sys.cpus().len(),
                cpu_usage_percent: cpu_usage,
                memory_total_bytes: total_memory,
                memory_used_bytes: used_memory,
                memory_usage_percent: if total_memory > 0 {
                    (used_memory as f32 / total_memory as f32) * 100.0
                } else {
                    0.0
                },
                disk: DiskDiagnostics {
                    total_bytes: disk_total,
                    available_bytes: disk_available,
                    usage_percent: disk_usage_percent,
                },
            },
            process: ProcessDiagnostics {
                pid,
                memory_bytes: proc_memory,
                thread_count,
                start_time: chrono::Utc::now()
                    .checked_sub_signed(chrono::Duration::seconds(self.uptime_secs() as i64))
                    .map(|t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
            },
            components: ComponentDiagnostics {
                mqtt: MqttDiagnostics {
                    connected: self.inner.mqtt_connected.load(Ordering::Acquire),
                    messages_sent: self.inner.mqtt_sent.load(Ordering::Acquire),
                    messages_received: self.inner.mqtt_received.load(Ordering::Acquire),
                    last_connected: {
                        let ts = self.inner.mqtt_last_connected.load(Ordering::Acquire);
                        if ts > 0 {
                            chrono::DateTime::from_timestamp(ts, 0)
                                .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
                        } else {
                            None
                        }
                    },
                },
                modbus: ModbusDiagnostics {
                    client_count: self.inner.modbus_client_count.load(Ordering::Acquire) as usize,
                    total_reads: self.inner.modbus_reads.load(Ordering::Acquire),
                    read_errors: self.inner.modbus_errors.load(Ordering::Acquire),
                    circuit_states: self
                        .inner
                        .modbus_circuit_states
                        .read()
                        .map(|g| g.clone())
                        .unwrap_or_default(),
                },
                scripts: ScriptDiagnostics {
                    loaded_count: self.inner.script_loaded_count.load(Ordering::Acquire) as usize,
                    active_count: self.inner.script_active_count.load(Ordering::Acquire) as usize,
                    total_executions: self.inner.script_executions.load(Ordering::Acquire),
                    execution_errors: self.inner.script_errors.load(Ordering::Acquire),
                },
                function_blocks: FunctionBlockDiagnostics {
                    instance_count: self.inner.fb_instance_count.load(Ordering::Acquire) as usize,
                    type_counts: self
                        .inner
                        .fb_type_counts
                        .read()
                        .map(|g| g.clone())
                        .unwrap_or_default(),
                },
                offline_queue: OfflineQueueDiagnostics {
                    size: self.inner.offline_queue_size.load(Ordering::Acquire),
                    capacity: self.inner.offline_queue_capacity.load(Ordering::Acquire),
                    total_queued: self.inner.offline_total_queued.load(Ordering::Acquire),
                    total_sent: self.inner.offline_total_sent.load(Ordering::Acquire),
                },
            },
            config,
            recent_errors,
        }
    }
}

impl Default for HealthState {
    fn default() -> Self {
        Self::new()
    }
}

/// Start the health check HTTP server
///
/// This function spawns a background task that listens for HTTP requests.
/// It should be called early in the application startup.
///
/// # Arguments
/// * `addr` - Socket address to bind to (e.g., "127.0.0.1:8080")
/// * `state` - Health state shared with the main application
///
/// # Returns
/// A join handle that can be used to wait for the server to stop.
#[cfg(feature = "health")]
pub async fn start_health_server(
    addr: SocketAddr,
    state: HealthState,
) -> tokio::task::JoinHandle<()> {
    use axum::{
        Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::get,
    };

    // Build the router
    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/ready", get(ready_handler))
        .route("/metrics", get(metrics_handler))
        .route("/diagnostics", get(diagnostics_handler))
        .with_state(state);

    info!("Starting health check server on {}", addr);

    // Spawn the server in a background task
    tokio::spawn(async move {
        let listener = match tokio::net::TcpListener::bind(addr).await {
            Ok(l) => l,
            Err(e) => {
                error!("Failed to bind health server to {}: {}", addr, e);
                return;
            }
        };

        if let Err(e) = axum::serve(listener, app).await {
            error!("Health server error: {}", e);
        }
    })
}

#[cfg(feature = "health")]
async fn health_handler(
    State(state): axum::extract::State<HealthState>,
) -> impl axum::response::IntoResponse {
    let health = state.health();
    let status_code = match health.status {
        "healthy" => axum::http::StatusCode::OK,
        "degraded" => axum::http::StatusCode::OK, // Still return 200 for degraded
        _ => axum::http::StatusCode::SERVICE_UNAVAILABLE,
    };
    (status_code, axum::Json(health))
}

#[cfg(feature = "health")]
async fn ready_handler(
    State(state): axum::extract::State<HealthState>,
) -> impl axum::response::IntoResponse {
    let readiness = state.readiness();
    let status_code = if readiness.ready {
        axum::http::StatusCode::OK
    } else {
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    };
    (status_code, axum::Json(readiness))
}

#[cfg(feature = "health")]
async fn metrics_handler(
    State(state): axum::extract::State<HealthState>,
) -> impl axum::response::IntoResponse {
    (axum::http::StatusCode::OK, axum::Json(state.metrics()))
}

#[cfg(feature = "health")]
async fn diagnostics_handler(
    State(state): axum::extract::State<HealthState>,
) -> impl axum::response::IntoResponse {
    (axum::http::StatusCode::OK, axum::Json(state.diagnostics()))
}

/// Simple TCP health check (no HTTP, just connection test)
///
/// This is a fallback for when the `health` feature is not enabled.
/// It simply accepts and closes connections to indicate the service is alive.
#[cfg(not(feature = "health"))]
pub async fn start_health_server(
    addr: SocketAddr,
    _state: HealthState,
) -> tokio::task::JoinHandle<()> {
    info!("Starting simple TCP health check on {}", addr);

    tokio::spawn(async move {
        let listener = match tokio::net::TcpListener::bind(addr).await {
            Ok(l) => l,
            Err(e) => {
                error!("Failed to bind health server to {}: {}", addr, e);
                return;
            }
        };

        loop {
            match listener.accept().await {
                Ok((socket, peer)) => {
                    // Just close the connection immediately - connection success = healthy
                    drop(socket);
                    tracing::trace!("Health check from {}", peer);
                }
                Err(e) => {
                    error!("Health server accept error: {}", e);
                    // Brief pause before retrying
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_state_default() {
        let state = HealthState::new();

        assert!(!state.is_ready());
        let health = state.health();
        assert_eq!(health.status, "unhealthy");
    }

    #[test]
    fn test_health_state_transitions() {
        let state = HealthState::new();

        // Config loaded -> degraded
        state.set_config_loaded(true);
        let health = state.health();
        assert_eq!(health.status, "degraded");
        assert!(!state.is_ready());

        // Device activated -> healthy
        state.set_device_activated(true);
        let health = state.health();
        assert_eq!(health.status, "healthy");
        assert!(state.is_ready());
    }

    #[test]
    fn test_readiness_checks() {
        let state = HealthState::new();

        let readiness = state.readiness();
        assert!(!readiness.ready);
        assert!(!readiness.checks.config_loaded);
        assert!(!readiness.checks.mqtt_connected);
        assert!(!readiness.checks.device_activated);

        state.set_config_loaded(true);
        state.set_mqtt_connected(true);
        state.set_device_activated(true);

        let readiness = state.readiness();
        assert!(readiness.ready);
        assert!(readiness.checks.config_loaded);
        assert!(readiness.checks.mqtt_connected);
        assert!(readiness.checks.device_activated);
    }

    #[test]
    fn test_metrics_counters() {
        let state = HealthState::new();

        state.inc_mqtt_sent();
        state.inc_mqtt_sent();
        state.inc_mqtt_received();
        state.inc_modbus_reads();
        state.inc_modbus_reads();
        state.inc_modbus_reads();
        state.inc_script_executions();
        state.set_offline_queue_size(42);

        let metrics = state.metrics();
        assert_eq!(metrics.mqtt_messages_sent, 2);
        assert_eq!(metrics.mqtt_messages_received, 1);
        assert_eq!(metrics.modbus_reads, 3);
        assert_eq!(metrics.script_executions, 1);
        assert_eq!(metrics.offline_queue_size, 42);
    }

    #[test]
    fn test_uptime() {
        let state = HealthState::new();

        // Uptime should be 0 or very small
        assert!(state.uptime_secs() < 2);

        std::thread::sleep(std::time::Duration::from_millis(100));

        // Still very small
        assert!(state.uptime_secs() < 2);
    }
}
