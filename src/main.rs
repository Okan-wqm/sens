//! Suderra Edge Agent
//!
//! Industrial IoT agent for aquaculture monitoring and control.
//! Handles device provisioning, MQTT communication, telemetry,
//! and PLC/sensor integration.
//!
//! Architecture v2.0:
//! - Actor pattern for GPIO and Modbus (thread-safe handles)
//! - Circuit breaker for fault tolerance
//! - Graceful shutdown coordinator
//! - Granular state management

mod bounded;
mod commands;
mod config;
mod error;
mod gpio;
mod health;
mod interning;
mod modbus;
mod mqtt;
mod offline_queue;
mod provisioning;
mod resilience;
mod scripting;
mod shutdown;
mod telemetry;

use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::commands::CommandHandler;
use crate::config::AgentConfig;
use crate::gpio::GpioHandle;
use crate::modbus::ModbusHandle;
use crate::mqtt::MqttClient;
use crate::provisioning::ProvisioningClient;
use crate::scripting::{ScriptEngine, ScriptStorage, SqlitePersistence};
use crate::shutdown::ShutdownCoordinator;
use crate::telemetry::TelemetryCollector;

/// Activation state - minimal mutable state
pub struct ActivationState {
    pub is_activated: bool,
    pub tenant_id: Option<String>,
    pub device_id: String,
}

/// Application state shared across components (v2.0 - Granular, v2.2 - Shared ScriptStorage)
///
/// Architecture:
/// - Config is immutable after init (Arc)
/// - Hardware handles use actor pattern (thread-safe)
/// - ScriptStorage is shared (v2.2 - singleton pattern for data consistency)
/// - Only activation state needs RwLock
pub struct AppState {
    /// Configuration (immutable after init)
    pub config: AgentConfig,

    /// MQTT client
    pub mqtt_client: Option<MqttClient>,

    /// Modbus handle (actor pattern - thread-safe)
    pub modbus_handle: Option<ModbusHandle>,

    /// GPIO handle (actor pattern - thread-safe, v2.0)
    pub gpio_handle: Option<GpioHandle>,

    /// Shared script storage (v2.2 - singleton for CommandHandler and ScriptEngine)
    /// This ensures both components see the same script state
    pub script_storage: Arc<tokio::sync::RwLock<ScriptStorage>>,

    /// Activation state
    pub is_activated: bool,
    pub tenant_id: Option<String>,
}

impl AppState {
    /// Create new AppState (v2.2 - with shared ScriptStorage)
    ///
    /// Note: Hardware handles will be initialized in LocalSet context
    pub fn new(config: AgentConfig) -> Self {
        // Initialize shared script storage (v2.2 singleton pattern)
        let mut script_storage = ScriptStorage::new(None);
        if let Err(e) = script_storage.init() {
            warn!("Script storage initialization failed: {}", e);
        }

        Self {
            config,
            mqtt_client: None,
            modbus_handle: None,
            gpio_handle: None,
            script_storage: Arc::new(tokio::sync::RwLock::new(script_storage)),
            is_activated: false,
            tenant_id: None,
        }
    }

    /// Initialize hardware handles (must be called within LocalSet context)
    pub fn init_hardware_handles(&mut self) {
        // Initialize Modbus actor
        if !self.config.modbus.is_empty() {
            self.modbus_handle = Some(ModbusHandle::new(self.config.modbus.clone()));
            info!(
                "Modbus actor initialized with {} devices",
                self.config.modbus.len()
            );
        }

        // Initialize GPIO actor (v2.0 - actor pattern)
        if !self.config.gpio.is_empty() {
            self.gpio_handle = Some(GpioHandle::new(
                self.config.gpio.clone(),
                self.config.runtime.gpio_timeout_secs,
            ));
            info!(
                "GPIO actor initialized with {} pins",
                self.config.gpio.len()
            );
        }
    }

    /// Legacy: Initialize Modbus handle only
    #[deprecated(note = "Use init_hardware_handles instead")]
    pub fn init_modbus(&mut self) {
        if !self.config.modbus.is_empty() {
            self.modbus_handle = Some(ModbusHandle::new(self.config.modbus.clone()));
            info!(
                "Modbus actor initialized with {} devices",
                self.config.modbus.len()
            );
        }
    }
}

/// Main entry point with optimized Tokio runtime for edge devices
///
/// Uses custom runtime builder instead of #[tokio::main] macro for:
/// - Worker thread count tuned for edge hardware (2 cores typical)
/// - Blocking thread pool limited for SQLite operations
/// - Stack size optimized for embedded environments
fn main() {
    // Build optimized runtime for edge devices
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2) // Edge devices typically have 2 cores
        .max_blocking_threads(8) // Limit for SQLite blocking ops
        .thread_stack_size(128 * 1024) // 128 KB per thread (embedded friendly)
        .enable_all()
        .build()
        .expect("Failed to build Tokio runtime");

    // Run async main within the custom runtime
    if let Err(e) = runtime.block_on(async_main()) {
        // Use tracing for error output (tracing is initialized in async_main, but on fatal
        // errors we need to output before that, so we allow stderr here)
        #[allow(clippy::print_stderr)]
        {
            eprintln!("Fatal error: {}", e);
        }
        std::process::exit(1);
    }
}

/// Async main function with application logic
async fn async_main() -> Result<()> {
    // Initialize logging
    init_logging();

    info!("======================================");
    info!("  Suderra Edge Agent v{}", env!("CARGO_PKG_VERSION"));
    info!("======================================");

    // Load configuration
    let config = match AgentConfig::load() {
        Ok(cfg) => {
            info!("Configuration loaded successfully");
            info!("  Device ID: {}", cfg.device_id);
            info!("  Device Code: {}", cfg.device_code);
            info!("  API URL: {}", cfg.api_url);
            cfg
        }
        Err(e) => {
            error!("Failed to load configuration: {}", e);
            error!("Please ensure /etc/suderra/config.yaml exists and is valid");
            std::process::exit(1);
        }
    };

    // Initialize OpenTelemetry OTLP export (if configured and feature enabled)
    // This adds distributed tracing support for production observability
    let _otel_provider = init_opentelemetry(&config);

    // Create shared state
    let state = Arc::new(RwLock::new(AppState::new(config.clone())));

    // Setup graceful shutdown
    let shutdown = match setup_shutdown_handler() {
        Ok(rx) => rx,
        Err(e) => {
            error!("Failed to setup shutdown handler: {}", e);
            std::process::exit(1);
        }
    };

    // Notify systemd that we're ready (IEC 62443 SL2 FR6: Timely Response)
    #[cfg(target_os = "linux")]
    {
        if let Err(e) = notify_systemd_ready() {
            warn!("Failed to notify systemd ready: {}", e);
        }
    }

    // Use LocalSet to allow non-Send futures (required for Modbus client)
    let local = tokio::task::LocalSet::new();
    let result = local.run_until(run_agent(state, shutdown)).await;

    if let Err(e) = result {
        error!("Agent error: {}", e);
        std::process::exit(1);
    }

    info!("Agent shutdown complete");
    Ok(())
}

/// Initialize logging with tracing
///
/// Note: OpenTelemetry OTLP export is initialized separately in `init_opentelemetry()`
/// after configuration is loaded.
fn init_logging() {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(fmt::layer().with_target(true))
        .with(filter)
        .init();
}

/// Initialize OpenTelemetry OTLP exporter (if configured)
///
/// This must be called after configuration is loaded. When the `telemetry` feature
/// is enabled and an OTLP endpoint is configured, traces will be exported to the
/// specified collector (e.g., Jaeger, Tempo, OpenTelemetry Collector).
///
/// # IEC 62443 SL2 FR6: Continuous Monitoring
/// OpenTelemetry provides distributed tracing for observability and debugging.
#[cfg(feature = "telemetry")]
fn init_opentelemetry(config: &config::AgentConfig) -> Option<opentelemetry_sdk::trace::TracerProvider> {
    use opentelemetry::trace::TracerProvider;
    use opentelemetry_otlp::WithExportConfig;
    use opentelemetry_sdk::trace::Sampler;

    let endpoint = config.telemetry.otlp.endpoint.as_ref()?;

    info!("Initializing OpenTelemetry OTLP export to {}", endpoint);

    let exporter = opentelemetry_otlp::new_exporter()
        .tonic()
        .with_endpoint(endpoint);

    match opentelemetry_otlp::new_pipeline()
        .tracing()
        .with_exporter(exporter)
        .with_trace_config(
            opentelemetry_sdk::trace::Config::default()
                .with_sampler(Sampler::TraceIdRatioBased(
                    config.telemetry.otlp.sample_ratio
                ))
                .with_resource(opentelemetry_sdk::Resource::new(vec![
                    opentelemetry::KeyValue::new(
                        "service.name",
                        config.telemetry.otlp.service_name.clone()
                    ),
                    opentelemetry::KeyValue::new(
                        "service.version",
                        env!("CARGO_PKG_VERSION")
                    ),
                    opentelemetry::KeyValue::new(
                        "device.id",
                        config.device_id.clone()
                    ),
                ]))
        )
        .install_batch(opentelemetry_sdk::runtime::Tokio)
    {
        Ok(provider) => {
            info!(
                "OpenTelemetry OTLP enabled: endpoint={}, service={}, sample_ratio={}",
                endpoint,
                config.telemetry.otlp.service_name,
                config.telemetry.otlp.sample_ratio
            );
            Some(provider)
        }
        Err(e) => {
            warn!("Failed to initialize OpenTelemetry: {}", e);
            None
        }
    }
}

/// Stub for when telemetry feature is disabled
#[cfg(not(feature = "telemetry"))]
fn init_opentelemetry(_config: &config::AgentConfig) -> Option<()> {
    None
}

/// Notify systemd that the service is ready (IEC 62443 SL2 FR6)
///
/// This function:
/// - Sends READY=1 to systemd when initialization is complete
/// - Starts a watchdog heartbeat task if WatchdogSec is configured
/// - Allows systemd to monitor service health and restart on failure
#[cfg(target_os = "linux")]
fn notify_systemd_ready() -> Result<()> {
    use sd_notify::NotifyState;

    // Notify systemd that we're ready
    sd_notify::notify(true, &[NotifyState::Ready])
        .context("Failed to send READY notification to systemd")?;
    info!("Notified systemd: service ready");

    // Check if watchdog is enabled and start heartbeat task
    let mut watchdog_usec: u64 = 0;
    if sd_notify::watchdog_enabled(false, &mut watchdog_usec) && watchdog_usec > 0 {
        // Ping at half the timeout interval
        let interval_usec = watchdog_usec / 2;
        let interval = Duration::from_micros(interval_usec);
        info!(
            "Systemd watchdog enabled (timeout: {}ms, heartbeat: {}ms)",
            watchdog_usec / 1000,
            interval_usec / 1000
        );

        // Spawn watchdog heartbeat task
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                if let Err(e) = sd_notify::notify(false, &[NotifyState::Watchdog]) {
                    warn!("Failed to send watchdog heartbeat: {}", e);
                } else {
                    debug!("Watchdog heartbeat sent");
                }
            }
        });
    }

    Ok(())
}

/// Setup shutdown signal handlers for graceful shutdown
///
/// # Platform Support
/// - All platforms: Ctrl+C (SIGINT)
/// - Unix only: SIGTERM, SIGHUP
fn setup_shutdown_handler() -> Result<tokio::sync::watch::Receiver<bool>> {
    let (tx, rx) = tokio::sync::watch::channel(false);

    // Clone tx for the ctrlc handler
    let tx_ctrlc = tx.clone();
    ctrlc::set_handler(move || {
        info!("SIGINT (Ctrl+C) received, initiating graceful shutdown...");
        let _ = tx_ctrlc.send(true);
    })
    .map_err(|e| {
        anyhow::anyhow!(
            "Failed to set Ctrl-C handler: {}. This may occur if a handler was already set.",
            e
        )
    })?;

    // Setup Unix-specific signal handlers (SIGTERM, SIGHUP)
    #[cfg(unix)]
    {
        let tx_term = tx.clone();
        let tx_hup = tx;

        // Spawn async task to handle Unix signals
        tokio::spawn(async move {
            use tokio::signal::unix::{signal, SignalKind};

            let mut sigterm = match signal(SignalKind::terminate()) {
                Ok(s) => s,
                Err(e) => {
                    warn!("Failed to setup SIGTERM handler: {}", e);
                    return;
                }
            };

            let mut sighup = match signal(SignalKind::hangup()) {
                Ok(s) => s,
                Err(e) => {
                    warn!("Failed to setup SIGHUP handler: {}", e);
                    return;
                }
            };

            tokio::select! {
                _ = sigterm.recv() => {
                    info!("SIGTERM received, initiating graceful shutdown...");
                    let _ = tx_term.send(true);
                }
                _ = sighup.recv() => {
                    info!("SIGHUP received, initiating graceful shutdown...");
                    let _ = tx_hup.send(true);
                }
            }
        });

        info!("Signal handlers registered: SIGINT, SIGTERM, SIGHUP");
    }

    #[cfg(not(unix))]
    {
        info!("Signal handlers registered: SIGINT (Ctrl+C)");
    }

    Ok(rx)
}

/// Shutdown timeout for graceful task termination
const SHUTDOWN_TIMEOUT_SECS: u64 = 30;

/// Main agent loop
async fn run_agent(
    state: Arc<RwLock<AppState>>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    // Step 1: Check if already activated (MQTT credentials in config)
    let needs_activation = {
        let state_guard = state.read().await;
        state_guard.config.mqtt.username.is_none()
    };

    if needs_activation {
        info!("Device not activated, starting provisioning...");

        // Step 2: Activate with cloud platform
        let provisioning_client = ProvisioningClient::new(state.clone())
            .context("Failed to create provisioning client")?;

        match provisioning_client.activate().await {
            Ok(response) => {
                info!("Device activated successfully!");
                info!("  MQTT Broker: {}", response.mqtt_broker);
                info!("  Tenant ID: {}", response.tenant_id);

                // Update state with activation response
                let mut state_guard = state.write().await;
                state_guard.config.mqtt.broker = Some(response.mqtt_broker);
                state_guard.config.mqtt.port = response.mqtt_port;
                state_guard.config.mqtt.username = Some(response.mqtt_username);
                state_guard.config.mqtt.password = Some(response.mqtt_password);
                state_guard.tenant_id = Some(response.tenant_id.clone());
                state_guard.is_activated = true;

                // SECURITY: Clear provisioning token from memory after successful activation
                // This prevents the token from being leaked in logs or memory dumps
                state_guard.config.provisioning_token = None;
                info!("Provisioning token cleared from memory");

                // Save updated config to disk (token will not be saved due to skip_serializing_if)
                if let Err(e) = state_guard.config.save() {
                    warn!("Failed to save config after activation: {}", e);
                }
            }
            Err(e) => {
                error!("Activation failed: {}", e);
                error!("Will retry on next restart");
                return Err(e.into());
            }
        }
    } else {
        info!("Device already activated, using stored credentials");
        let mut state_guard = state.write().await;
        state_guard.is_activated = true;
    }

    // Step 3: Connect to MQTT
    info!("Connecting to MQTT broker...");
    let mqtt_client = {
        let state_guard = state.read().await;
        MqttClient::new(&state_guard.config).await?
    };

    {
        let mut state_guard = state.write().await;
        state_guard.mqtt_client = Some(mqtt_client);
    }
    info!("MQTT connected successfully");

    // Step 4: Initialize hardware interfaces
    info!("Initializing hardware interfaces...");
    init_hardware(&state).await;

    // Step 5: Create shutdown coordinator for graceful termination
    let mut shutdown_coordinator = ShutdownCoordinator::new();

    // Step 6: Start telemetry collector with shutdown awareness
    let telemetry_collector = TelemetryCollector::new(state.clone());
    let telemetry_shutdown = shutdown_coordinator.subscribe();
    let telemetry_handle = tokio::spawn(async move {
        shutdown::run_until_shutdown(telemetry_collector.run(), telemetry_shutdown).await;
    });
    shutdown_coordinator.register_task("telemetry", telemetry_handle);

    // Step 7: Start command handler with shutdown awareness
    let command_handler = CommandHandler::new(state.clone()).await;
    let command_shutdown = shutdown_coordinator.subscribe();
    let command_handle = tokio::spawn(async move {
        shutdown::run_until_shutdown(command_handler.run(), command_shutdown).await;
    });
    shutdown_coordinator.register_task("command", command_handle);

    // Step 8: Initialize SQLite persistence for RETAIN variables (IEC 61131-3)
    let persistence = {
        let data_dir =
            std::env::var("SUDERRA_DATA_DIR").unwrap_or_else(|_| "/var/lib/suderra".to_string());
        let db_path = format!("{}/retain.db", data_dir);

        match SqlitePersistence::new(&db_path) {
            Ok(p) => {
                info!("SQLite persistence initialized: {}", db_path);
                Some(Arc::new(p))
            }
            Err(e) => {
                warn!(
                    "Failed to initialize persistence (RETAIN variables disabled): {}",
                    e
                );
                None
            }
        }
    };

    // Step 9: Start script engine (with persistence if available)
    // v2.2: ScriptEngine constructors are now async to get shared storage from AppState
    info!("Starting script engine...");
    let mut script_engine = match &persistence {
        Some(p) => ScriptEngine::with_persistence(state.clone(), p.clone()).await,
        None => ScriptEngine::new(state.clone()).await,
    };

    if let Err(e) = script_engine.init().await {
        warn!("Script engine initialization failed: {}", e);
    } else {
        info!(
            "Script engine initialized with {} scripts",
            script_engine.script_count().await
        );
    }

    // Keep reference for graceful shutdown persistence stats
    let script_persistence = persistence.clone();

    // Start script engine with shutdown awareness
    let script_shutdown = shutdown_coordinator.subscribe();
    let script_handle = tokio::spawn(async move {
        shutdown::run_until_shutdown(script_engine.run(), script_shutdown).await;
    });
    shutdown_coordinator.register_task("script_engine", script_handle);

    info!(
        "Shutdown coordinator initialized with {} tasks",
        shutdown_coordinator.task_count()
    );

    // Step 10: Main loop - wait for shutdown signal
    info!("Agent running. Press Ctrl+C to stop.");

    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    info!("Shutdown signal received");
                    break;
                }
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_secs(1)) => {
                // Periodic health check could go here
            }
        }
    }

    // Step 11: Graceful shutdown - signal all tasks and wait for completion
    info!(
        "Initiating graceful shutdown with {}s timeout...",
        SHUTDOWN_TIMEOUT_SECS
    );
    shutdown_coordinator
        .shutdown(Duration::from_secs(SHUTDOWN_TIMEOUT_SECS))
        .await;

    // Log persistence statistics on shutdown (IEC 61131-3 compliance)
    if let Some(ref persistence) = script_persistence {
        match persistence.get_stats() {
            Ok(stats) => {
                info!(
                    "Persistence stats: {} variables, {} FB states, {} executions, {} bytes",
                    stats.variable_count,
                    stats.fb_state_count,
                    stats.execution_history_count,
                    stats.database_size_bytes
                );
            }
            Err(e) => {
                warn!("Failed to get persistence stats: {}", e);
            }
        }
    }

    // Disconnect hardware interfaces
    // Disconnect Modbus devices via handle
    let modbus_handle = {
        let state_guard = state.read().await;
        state_guard.modbus_handle.clone()
    };

    if let Some(handle) = modbus_handle {
        handle.disconnect_all().await;
        info!("Modbus devices disconnected");
    }

    // Disconnect MQTT gracefully
    {
        let mut state_guard = state.write().await;
        if let Some(mqtt) = state_guard.mqtt_client.take() {
            if let Err(e) = mqtt.disconnect().await {
                warn!("Error disconnecting MQTT: {}", e);
            }
        }
    }

    Ok(())
}

/// Initialize hardware interfaces (Modbus, GPIO) v2.0
///
/// Uses actor pattern for both Modbus and GPIO
async fn init_hardware(state: &Arc<RwLock<AppState>>) {
    // Initialize hardware actors (must be done in LocalSet context)
    {
        let mut state_guard = state.write().await;
        state_guard.init_hardware_handles();
    }

    // Initialize GPIO via actor handle
    let gpio_handle = {
        let state_guard = state.read().await;
        state_guard.gpio_handle.clone()
    };

    if let Some(handle) = gpio_handle {
        let pin_count = handle.pin_count().await;
        info!("Initializing GPIO with {} pins configured", pin_count);

        match handle.init().await {
            Ok(()) => {
                info!("GPIO initialized successfully");
                if handle.is_available().await {
                    info!("  GPIO hardware is available");
                } else {
                    info!("  GPIO running in simulation mode");
                }
            }
            Err(e) => {
                warn!("GPIO initialization failed: {}", e);
            }
        }
    } else {
        debug!("No GPIO pins configured");
    }

    // Connect to Modbus devices via handle
    let modbus_handle = {
        let state_guard = state.read().await;
        state_guard.modbus_handle.clone()
    };

    if let Some(handle) = modbus_handle {
        info!("Connecting to Modbus devices...");
        let errors = handle.connect_all().await;

        if errors.is_empty() {
            info!("All Modbus devices connected successfully");
        } else {
            for err in &errors {
                warn!("Modbus connection error: {}", err);
            }
        }

        // Log connected device info
        let results = handle.read_all().await;
        for result in results {
            if result.errors.is_empty() {
                info!(
                    "  {} - {} registers available",
                    result.device_name,
                    result.values.len()
                );
                for value in &result.values {
                    debug!(
                        "    {}: {:.2} {}",
                        value.name,
                        value.scaled_value,
                        value.unit.as_deref().unwrap_or("")
                    );
                }
            } else {
                warn!("  {} - errors: {:?}", result.device_name, result.errors);
            }
        }
    } else {
        debug!("No Modbus devices configured");
    }

    // Log hardware summary
    let state_guard = state.read().await;
    let gpio_count = state_guard.config.gpio.len();
    let modbus_count = state_guard.config.modbus.len();

    info!(
        "Hardware summary: {} GPIO pins, {} Modbus devices",
        gpio_count, modbus_count
    );
}
