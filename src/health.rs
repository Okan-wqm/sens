//! Health Check HTTP Endpoint
//!
//! Provides a lightweight HTTP server for health checks and readiness probes.
//! Used by orchestrators (Docker, Kubernetes, systemd) to monitor agent status.
//!
//! # Endpoints
//! - `GET /health` - Basic health check (always returns 200 if server is running)
//! - `GET /ready` - Readiness check (returns 200 only when fully initialized)
//! - `GET /metrics` - Basic metrics (queue size, uptime, connections)
//!
//! # Configuration
//! Enable with the `health` feature flag in Cargo.toml.
//!
//! # IEC 62443 SL2 Compliance
//! - FR6: Timely Response to Events (health monitoring)

use serde::Serialize;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
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
    /// Script executions counter
    script_executions: AtomicU64,
    /// Current offline queue size
    offline_queue_size: AtomicU64,
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
                script_executions: AtomicU64::new(0),
                offline_queue_size: AtomicU64::new(0),
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
        self.inner.mqtt_connected.store(connected, Ordering::Release);
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

    /// Set offline queue size
    pub fn set_offline_queue_size(&self, size: u64) {
        self.inner
            .offline_queue_size
            .store(size, Ordering::Release);
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
        extract::State,
        http::StatusCode,
        response::IntoResponse,
        routing::get,
        Json, Router,
    };

    // Build the router
    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/ready", get(ready_handler))
        .route("/metrics", get(metrics_handler))
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
