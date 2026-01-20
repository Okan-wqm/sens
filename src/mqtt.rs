//! MQTT client for cloud communication
//!
//! Handles connection to MQTT broker, publishing telemetry/status,
//! and subscribing to commands/config topics.
//!
//! ## IEC 62443 SL2 Security Features
//! - TLS 1.2+ encryption for data confidentiality (FR4)
//! - mTLS for device authentication (FR1)
//! - Last Will for device status monitoring
//!
//! ## v1.2.3 Improvements
//! - Increased message channel capacity (100 -> 500)
//! - Added backpressure handling with retry logic
//! - Improved error reporting for channel full conditions
//!
//! ## v1.3.4 Failover Support
//! - Automatic failover to backup broker on primary failure
//! - Health checks for primary broker recovery
//! - Zero message loss with offline queue integration

use anyhow::{Context, Result};
use chrono::Utc;
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS, Transport};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch, RwLock};
use tracing::{debug, error, info, trace, warn};

use crate::mqtt_failover::{BrokerEndpoint, FailoverManager, FailoverState};

/// Message channel capacity (v1.2.3: increased from 100 to 500)
/// Higher capacity reduces message loss during burst traffic
const MESSAGE_CHANNEL_CAPACITY: usize = 500;

/// Internal MQTT event loop buffer size (v1.2.6)
/// Should match MESSAGE_CHANNEL_CAPACITY for consistent backpressure behavior
const INTERNAL_MQTT_BUFFER_SIZE: usize = 500;

/// Maximum retry attempts when channel is full (v1.2.3)
const CHANNEL_SEND_MAX_RETRIES: u32 = 3;

/// Delay between retry attempts in milliseconds (v1.2.3)
const CHANNEL_SEND_RETRY_DELAY_MS: u64 = 10;

use crate::config::{AgentConfig, ResolvedTopics};
use crate::error::AgentError;

/// MQTT client wrapper
pub struct MqttClient {
    client: AsyncClient,
    topics: ResolvedTopics,
    device_id: String,
    device_code: String,
    /// Channel to receive incoming messages
    message_rx: mpsc::Receiver<IncomingMessage>,
    /// Event loop task handle for graceful shutdown (v1.2.6)
    event_loop_handle: Option<tokio::task::JoinHandle<()>>,
}

/// Incoming message from MQTT
#[derive(Debug, Clone)]
pub struct IncomingMessage {
    pub topic: String,
    pub payload: Vec<u8>,
}

/// Device status message
#[derive(Debug, Serialize)]
pub struct StatusMessage {
    pub device_id: String,
    pub device_code: String,
    pub status: DeviceStatus,
    pub timestamp: String,
    pub agent_version: String,
    pub uptime_seconds: u64,
}

/// Device status enum
#[derive(Debug, Serialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum DeviceStatus {
    Online,
    Offline,
    Maintenance,
    Error,
}

/// Telemetry message
#[derive(Debug, Serialize)]
pub struct TelemetryMessage {
    pub device_id: String,
    pub device_code: String,
    pub timestamp: String,
    pub metrics: TelemetryMetrics,
}

/// Telemetry metrics
#[derive(Debug, Serialize, Default)]
pub struct TelemetryMetrics {
    // System metrics
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_usage_percent: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_usage_percent: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_used_mb: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_total_mb: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_usage_percent: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_used_gb: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_total_gb: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature_celsius: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network_rx_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network_tx_bytes: Option<u64>,

    // Hardware metrics (PLC/Sensors via Modbus)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modbus: Option<Vec<ModbusDeviceData>>,

    // GPIO pin states
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpio: Option<Vec<GpioPinData>>,
}

/// Modbus device data for telemetry
#[derive(Debug, Serialize, Clone)]
pub struct ModbusDeviceData {
    pub device_name: String,
    pub registers: Vec<ModbusRegisterData>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

/// Modbus register value
#[derive(Debug, Serialize, Clone)]
pub struct ModbusRegisterData {
    pub name: String,
    pub address: u16,
    pub value: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

/// GPIO pin data for telemetry
#[derive(Debug, Serialize, Clone)]
pub struct GpioPinData {
    pub name: String,
    pub pin: u8,
    pub direction: String,
    pub state: String, // "high" or "low"
}

/// Command message (received from cloud)
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct CommandMessage {
    pub command_id: String,
    pub command: String,
    #[serde(default)]
    pub params: serde_json::Value,
    pub timestamp: String,
}

/// Command response message
#[derive(Debug, Serialize)]
pub struct CommandResponse {
    pub command_id: String,
    pub device_id: String,
    pub success: bool,
    pub result: serde_json::Value,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl MqttClient {
    /// Create and connect MQTT client
    pub async fn new(config: &AgentConfig) -> Result<Self> {
        // Get MQTT settings
        let broker = config
            .mqtt
            .broker
            .as_ref()
            .ok_or_else(|| AgentError::Mqtt("MQTT broker not configured".into()))?;
        let username = config
            .mqtt
            .username
            .as_ref()
            .ok_or_else(|| AgentError::Mqtt("MQTT username not configured".into()))?;
        let password = config
            .mqtt
            .password
            .as_ref()
            .ok_or_else(|| AgentError::Mqtt("MQTT password not configured".into()))?;

        // Resolve topics
        let tenant_id = config
            .tenant_id
            .as_ref()
            .ok_or_else(|| AgentError::Mqtt("Tenant ID not configured".into()))?;
        let topics = config.mqtt.topics.resolve(tenant_id, &config.device_id);

        // Create MQTT options
        let mut options = MqttOptions::new(
            username, // Use username as client ID
            broker,
            config.mqtt.port,
        );

        // v1.2.2: Use expose_secret() to access password (zeroize on drop)
        options.set_credentials(username, password.expose_secret());
        options.set_keep_alive(Duration::from_secs(config.mqtt.keepalive_secs));
        options.set_clean_session(config.mqtt.clean_session);

        // Configure TLS transport if enabled (IEC 62443 SL2 FR4)
        if config.mqtt.tls.enabled {
            let tls_config = Self::configure_tls(&config.mqtt.tls)?;
            options.set_transport(tls_config);
            info!("MQTT TLS enabled");
        }

        // Set last will (offline status)
        let last_will_payload = serde_json::to_vec(&StatusMessage {
            device_id: config.device_id.clone(),
            device_code: config.device_code.clone(),
            status: DeviceStatus::Offline,
            timestamp: Utc::now().to_rfc3339(),
            agent_version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_seconds: 0,
        })?;

        options.set_last_will(rumqttc::LastWill {
            topic: topics.status.clone(),
            message: last_will_payload.into(),
            qos: QoS::AtLeastOnce,
            retain: true,
        });

        // Create client (v1.2.6: use constant for buffer size)
        let (client, mut eventloop) = AsyncClient::new(options, INTERNAL_MQTT_BUFFER_SIZE);

        // Create message channel (v1.2.3: increased capacity)
        let (message_tx, message_rx) = mpsc::channel(MESSAGE_CHANNEL_CAPACITY);

        // Spawn event loop handler with exponential backoff config (v1.2.6: track handle)
        let topics_clone = topics.clone();
        let min_backoff = config.runtime.mqtt_reconnect_min_secs;
        let max_backoff = config.runtime.mqtt_reconnect_max_secs;
        let event_loop_handle = tokio::spawn(async move {
            Self::handle_events(
                &mut eventloop,
                message_tx,
                topics_clone,
                min_backoff,
                max_backoff,
            )
            .await;
        });

        let mqtt_client = Self {
            client,
            topics,
            device_id: config.device_id.clone(),
            device_code: config.device_code.clone(),
            message_rx,
            event_loop_handle: Some(event_loop_handle),
        };

        // Subscribe to command and config topics
        mqtt_client.subscribe().await?;

        // Publish online status
        mqtt_client.publish_status(DeviceStatus::Online, 0).await?;

        Ok(mqtt_client)
    }

    /// Handle MQTT events with exponential backoff on errors
    async fn handle_events(
        eventloop: &mut rumqttc::EventLoop,
        message_tx: mpsc::Sender<IncomingMessage>,
        _topics: ResolvedTopics, // Available for future topic filtering
        min_backoff_secs: u64,
        max_backoff_secs: u64,
    ) {
        let mut consecutive_errors: u32 = 0;

        loop {
            match eventloop.poll().await {
                Ok(Event::Incoming(Packet::Publish(publish))) => {
                    consecutive_errors = 0; // Reset on success
                    // v1.2.6: Enhanced logging with message details
                    info!(
                        "📥 MQTT message received: topic='{}', size={} bytes, qos={:?}, retain={}",
                        publish.topic,
                        publish.payload.len(),
                        publish.qos,
                        publish.retain
                    );

                    let topic_for_log = publish.topic.clone();
                    let mut msg = IncomingMessage {
                        topic: publish.topic,
                        payload: publish.payload.to_vec(),
                    };

                    // v1.2.3: Retry logic with backpressure handling
                    // Optimized: reuse msg from TrySendError::Full instead of cloning
                    let mut send_attempts = 0u32;
                    loop {
                        match message_tx.try_send(msg) {
                            Ok(()) => {
                                if send_attempts > 0 {
                                    debug!(
                                        "Message sent after {} retries (topic: {})",
                                        send_attempts, topic_for_log
                                    );
                                }
                                break;
                            }
                            Err(mpsc::error::TrySendError::Full(returned_msg)) => {
                                send_attempts = send_attempts.saturating_add(1);
                                if send_attempts >= CHANNEL_SEND_MAX_RETRIES {
                                    error!(
                                        "Message channel full after {} retries. \
                                        Message dropped (topic: {}). Consider increasing \
                                        MESSAGE_CHANNEL_CAPACITY or processing messages faster.",
                                        send_attempts, topic_for_log
                                    );
                                    break;
                                }
                                warn!(
                                    "Message channel full (attempt {}/{}), retrying...",
                                    send_attempts, CHANNEL_SEND_MAX_RETRIES
                                );
                                // Reuse the returned message instead of cloning
                                msg = returned_msg;
                                tokio::time::sleep(Duration::from_millis(
                                    CHANNEL_SEND_RETRY_DELAY_MS,
                                ))
                                .await;
                            }
                            Err(mpsc::error::TrySendError::Closed(_)) => {
                                error!(
                                    "Message channel closed. Receiver dropped. \
                                    This indicates a critical error in the command handler."
                                );
                                break;
                            }
                        }
                    }
                }
                Ok(Event::Incoming(Packet::ConnAck(connack))) => {
                    consecutive_errors = 0; // Reset on successful connection
                    // v1.2.6: Enhanced connection logging
                    info!(
                        "🟢 MQTT CONNECTED: code={:?}, session_present={}",
                        connack.code, connack.session_present
                    );
                }
                Ok(Event::Incoming(Packet::SubAck(suback))) => {
                    // v1.2.6: Log subscription acknowledgment with QoS
                    info!(
                        "📋 MQTT subscription acknowledged: qos={:?}",
                        suback.return_codes
                    );
                }
                Ok(Event::Incoming(Packet::PingResp)) => {
                    trace!("🏓 MQTT ping response received (connection alive)");
                }
                Ok(Event::Incoming(Packet::Disconnect)) => {
                    // v1.2.6: Log disconnection events
                    warn!("🔴 MQTT DISCONNECTED by broker");
                }
                Ok(Event::Outgoing(outgoing)) => {
                    // v1.2.6: Log outgoing events at trace level
                    trace!("📤 MQTT outgoing event: {:?}", outgoing);
                }
                Ok(event) => {
                    // v1.2.6: Log other events at trace level
                    trace!("MQTT event: {:?}", event);
                }
                Err(e) => {
                    consecutive_errors = consecutive_errors.saturating_add(1);

                    // v1.2.6: Clearer exponential backoff: min * 2^(errors-1), capped at max
                    // Shift amount capped at 6 (max multiplier = 64x)
                    let shift_amount = consecutive_errors.saturating_sub(1).min(6) as u32;
                    let multiplier = 1u64 << shift_amount;
                    let backoff_secs = min_backoff_secs
                        .saturating_mul(multiplier)
                        .min(max_backoff_secs);

                    error!(
                        "MQTT error (attempt {}): {:?}. Retrying in {}s",
                        consecutive_errors, e, backoff_secs
                    );

                    tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
                }
            }
        }
    }

    /// Subscribe to command and config topics
    async fn subscribe(&self) -> Result<()> {
        info!("Subscribing to topics:");
        info!("  Commands: {}", self.topics.commands);
        info!("  Config: {}", self.topics.config);

        self.client
            .subscribe(&self.topics.commands, QoS::AtLeastOnce)
            .await
            .context("Failed to subscribe to commands topic")?;

        self.client
            .subscribe(&self.topics.config, QoS::AtLeastOnce)
            .await
            .context("Failed to subscribe to config topic")?;

        Ok(())
    }

    /// Publish device status
    pub async fn publish_status(&self, status: DeviceStatus, uptime_seconds: u64) -> Result<()> {
        let message = StatusMessage {
            device_id: self.device_id.clone(),
            device_code: self.device_code.clone(),
            status,
            timestamp: Utc::now().to_rfc3339(),
            agent_version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_seconds,
        };

        let payload = serde_json::to_vec(&message)?;
        let payload_len = payload.len();

        self.client
            .publish(&self.topics.status, QoS::AtLeastOnce, true, payload)
            .await
            .context("Failed to publish status")?;

        // v1.2.6: Enhanced publish logging
        info!(
            "📤 MQTT status published: topic='{}', status={:?}, size={} bytes",
            self.topics.status, status, payload_len
        );
        Ok(())
    }

    /// Publish telemetry data
    pub async fn publish_telemetry(&self, metrics: TelemetryMetrics) -> Result<()> {
        let message = TelemetryMessage {
            device_id: self.device_id.clone(),
            device_code: self.device_code.clone(),
            timestamp: Utc::now().to_rfc3339(),
            metrics,
        };

        let payload = serde_json::to_vec(&message)?;
        let payload_len = payload.len();

        self.client
            .publish(&self.topics.telemetry, QoS::AtMostOnce, false, payload)
            .await
            .context("Failed to publish telemetry")?;

        // v1.2.6: Enhanced telemetry logging
        info!(
            "📤 MQTT telemetry published: topic='{}', size={} bytes",
            self.topics.telemetry, payload_len
        );
        Ok(())
    }

    /// Publish command response
    pub async fn publish_response(&self, response: CommandResponse) -> Result<()> {
        let payload = serde_json::to_vec(&response)?;
        let payload_len = payload.len();
        let command_id = response.command_id.clone();
        let success = response.success;

        self.client
            .publish(&self.topics.responses, QoS::AtLeastOnce, false, payload)
            .await
            .context("Failed to publish response")?;

        // v1.2.6: Enhanced response logging
        info!(
            "📤 MQTT response published: topic='{}', command_id='{}', success={}, size={} bytes",
            self.topics.responses, command_id, success, payload_len
        );
        Ok(())
    }

    /// Receive next incoming message
    pub async fn recv(&mut self) -> Option<IncomingMessage> {
        self.message_rx.recv().await
    }

    /// Try to receive incoming message without blocking
    pub fn try_recv(&mut self) -> Option<IncomingMessage> {
        self.message_rx.try_recv().ok()
    }

    /// Disconnect from broker
    pub async fn disconnect(mut self) -> Result<()> {
        // Publish offline status before disconnecting
        let _ = self.publish_status(DeviceStatus::Offline, 0).await;

        self.client
            .disconnect()
            .await
            .context("Failed to disconnect MQTT")?;

        // v1.2.6: Abort event loop task for graceful shutdown
        if let Some(handle) = self.event_loop_handle.take() {
            handle.abort();
            // Wait briefly for task to terminate
            let _ = tokio::time::timeout(std::time::Duration::from_millis(100), handle).await;
        }

        info!("MQTT disconnected");
        Ok(())
    }

    /// Get topics reference
    pub fn topics(&self) -> &ResolvedTopics {
        &self.topics
    }

    /// Configure TLS transport (IEC 62443 SL2 FR4: Data Confidentiality)
    ///
    /// Supports:
    /// - Server certificate verification via CA cert
    /// - Client certificate authentication (mTLS) for FR1 compliance
    fn configure_tls(tls_config: &crate::config::MqttTlsConfig) -> Result<Transport> {
        use rumqttc::TlsConfiguration;

        // Read CA certificate for server verification
        let ca_cert = if let Some(ref ca_path) = tls_config.ca_cert_path {
            let ca_bytes = std::fs::read(ca_path)
                .with_context(|| format!("Failed to read CA certificate: {}", ca_path))?;
            info!("Loaded CA certificate from: {}", ca_path);
            ca_bytes
        } else {
            // Use system root certificates if no CA specified
            info!("Using system root certificates for TLS");
            Vec::new()
        };

        // Read client certificate and key for mTLS (optional)
        // v1.2.4: Rust 2024 edition - implicit borrowing in patterns
        let client_auth = if let (Some(cert_path), Some(key_path)) =
            (&tls_config.client_cert_path, &tls_config.client_key_path)
        {
            let cert_bytes = std::fs::read(cert_path)
                .with_context(|| format!("Failed to read client certificate: {}", cert_path))?;
            let key_bytes = std::fs::read(key_path)
                .with_context(|| format!("Failed to read client key: {}", key_path))?;
            info!("Loaded client certificate from: {}", cert_path);
            Some((cert_bytes, key_bytes))
        } else {
            None
        };

        // Build TLS configuration
        // Note: rumqttc requires explicit CA certificate for TLS validation
        // If no CA cert is provided, return error (security requirement)
        if ca_cert.is_empty() {
            return Err(anyhow::anyhow!(
                "TLS enabled but no CA certificate provided. Set mqtt.tls.ca_cert_path in config."
            ));
        }

        let tls = TlsConfiguration::Simple {
            ca: ca_cert,
            alpn: Some(vec![b"mqtt".to_vec()]),
            client_auth,
        };

        Ok(Transport::Tls(tls))
    }

    /// Create MQTT options for a specific broker endpoint
    pub fn create_mqtt_options(
        config: &crate::config::AgentConfig,
        broker: &BrokerEndpoint,
    ) -> Result<MqttOptions> {
        let username = config
            .mqtt
            .username
            .as_ref()
            .ok_or_else(|| crate::error::AgentError::Mqtt("MQTT username not configured".into()))?;
        let password = config
            .mqtt
            .password
            .as_ref()
            .ok_or_else(|| crate::error::AgentError::Mqtt("MQTT password not configured".into()))?;

        let mut options = MqttOptions::new(username, &broker.host, broker.port);
        options.set_credentials(username, password.expose_secret());
        options.set_keep_alive(Duration::from_secs(config.mqtt.keepalive_secs));
        options.set_clean_session(config.mqtt.clean_session);

        Ok(options)
    }
}

// ============================================================================
// Failover MQTT Client (v1.3.4)
// ============================================================================

/// MQTT client with automatic failover support
///
/// Wraps the standard MqttClient and adds:
/// - Automatic failover to backup broker on failure
/// - Health checks for primary broker recovery
/// - Seamless reconnection handling
pub struct FailoverMqttClient {
    /// Inner MQTT client (current active connection)
    inner: Arc<RwLock<Option<MqttClient>>>,
    /// Failover manager
    failover_manager: Arc<FailoverManager>,
    /// State change receiver
    state_rx: watch::Receiver<FailoverState>,
    /// Configuration reference
    config: Arc<crate::config::AgentConfig>,
    /// Health check task handle
    health_check_handle: Option<tokio::task::JoinHandle<()>>,
    /// Reconnection task handle
    reconnect_handle: Option<tokio::task::JoinHandle<()>>,
    /// Message receiver (proxied from inner client)
    message_rx: mpsc::Receiver<IncomingMessage>,
    /// Message sender for proxying
    message_tx: mpsc::Sender<IncomingMessage>,
}

impl FailoverMqttClient {
    /// Create a new failover-enabled MQTT client
    pub async fn new(config: Arc<crate::config::AgentConfig>) -> Result<Self> {
        let primary_broker = config
            .mqtt
            .broker
            .as_ref()
            .ok_or_else(|| crate::error::AgentError::Mqtt("MQTT broker not configured".into()))?
            .clone();

        // Create failover manager
        let (failover_manager, state_rx) = FailoverManager::new(
            primary_broker,
            config.mqtt.port,
            config.mqtt.failover.clone(),
        );
        let failover_manager = Arc::new(failover_manager);

        // Create message channel for proxying
        let (message_tx, message_rx) = mpsc::channel(MESSAGE_CHANNEL_CAPACITY);

        // Create initial MQTT client connected to primary
        let inner_client = MqttClient::new(&config).await?;

        let mut client = Self {
            inner: Arc::new(RwLock::new(Some(inner_client))),
            failover_manager: failover_manager.clone(),
            state_rx,
            config,
            health_check_handle: None,
            reconnect_handle: None,
            message_rx,
            message_tx,
        };

        // Start health check task if failover is enabled
        if failover_manager.is_enabled() {
            let handle = failover_manager.start_health_check_task();
            client.health_check_handle = Some(handle);
            info!(
                "🔄 Failover enabled: primary={}, backup={}",
                failover_manager.get_primary().address(),
                failover_manager
                    .get_backup()
                    .map(|b| b.address())
                    .unwrap_or_else(|| "none".to_string())
            );
        }

        // Start message proxy task
        client.start_message_proxy();

        Ok(client)
    }

    /// Start message proxy task that forwards messages from inner client
    fn start_message_proxy(&self) {
        let inner = self.inner.clone();
        let message_tx = self.message_tx.clone();

        tokio::spawn(async move {
            loop {
                // Try to receive from inner client
                let msg = {
                    let mut guard = inner.write().await;
                    if let Some(ref mut client) = *guard {
                        client.try_recv()
                    } else {
                        None
                    }
                };

                if let Some(msg) = msg {
                    if message_tx.send(msg).await.is_err() {
                        break; // Receiver dropped
                    }
                } else {
                    // No message available, wait a bit
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        });
    }

    /// Get the failover manager for status/control
    pub fn failover_manager(&self) -> &Arc<FailoverManager> {
        &self.failover_manager
    }

    /// Get current failover state
    pub async fn get_failover_state(&self) -> FailoverState {
        self.failover_manager.get_state().await
    }

    /// Check if currently connected to backup broker
    pub async fn is_on_backup(&self) -> bool {
        matches!(
            self.failover_manager.get_state().await,
            FailoverState::BackupActive
        )
    }

    /// Publish device status
    pub async fn publish_status(&self, status: DeviceStatus, uptime_seconds: u64) -> Result<()> {
        let guard = self.inner.read().await;
        if let Some(ref client) = *guard {
            client.publish_status(status, uptime_seconds).await
        } else {
            Err(anyhow::anyhow!("MQTT client not connected"))
        }
    }

    /// Publish telemetry data
    pub async fn publish_telemetry(&self, metrics: TelemetryMetrics) -> Result<()> {
        let guard = self.inner.read().await;
        if let Some(ref client) = *guard {
            client.publish_telemetry(metrics).await
        } else {
            Err(anyhow::anyhow!("MQTT client not connected"))
        }
    }

    /// Publish command response
    pub async fn publish_response(&self, response: CommandResponse) -> Result<()> {
        let guard = self.inner.read().await;
        if let Some(ref client) = *guard {
            client.publish_response(response).await
        } else {
            Err(anyhow::anyhow!("MQTT client not connected"))
        }
    }

    /// Receive next incoming message
    pub async fn recv(&mut self) -> Option<IncomingMessage> {
        self.message_rx.recv().await
    }

    /// Try to receive incoming message without blocking
    pub fn try_recv(&mut self) -> Option<IncomingMessage> {
        self.message_rx.try_recv().ok()
    }

    /// Get topics reference
    pub async fn topics(&self) -> Option<ResolvedTopics> {
        let guard = self.inner.read().await;
        guard.as_ref().map(|c| c.topics().clone())
    }

    /// Handle connection failure - may trigger failover
    pub async fn handle_connection_failure(&self) -> bool {
        let should_failover = self.failover_manager.record_failure().await;

        if should_failover {
            info!("🔄 Initiating failover to backup broker...");
            if let Err(e) = self.reconnect_to_backup().await {
                error!("Failed to connect to backup broker: {}", e);
                return false;
            }
            return true;
        }

        false
    }

    /// Handle successful connection
    pub async fn handle_connection_success(&self) {
        self.failover_manager.record_success().await;
    }

    /// Reconnect to backup broker
    async fn reconnect_to_backup(&self) -> Result<()> {
        let backup = self
            .failover_manager
            .get_backup()
            .ok_or_else(|| anyhow::anyhow!("No backup broker configured"))?;

        info!(
            "🔄 Connecting to backup broker: {}:{}",
            backup.host, backup.port
        );

        // Create new config with backup broker
        let mut backup_config = (*self.config).clone();
        backup_config.mqtt.broker = Some(backup.host.clone());
        backup_config.mqtt.port = backup.port;

        // Disconnect old client
        {
            let mut guard = self.inner.write().await;
            if let Some(client) = guard.take() {
                let _ = client.disconnect().await;
            }
        }

        // Connect to backup
        let new_client = MqttClient::new(&backup_config).await?;

        {
            let mut guard = self.inner.write().await;
            *guard = Some(new_client);
        }

        self.failover_manager.record_success().await;
        info!("✅ Connected to backup broker");

        Ok(())
    }

    /// Reconnect to primary broker (called during recovery)
    pub async fn reconnect_to_primary(&self) -> Result<()> {
        let primary = self.failover_manager.get_primary();

        info!(
            "🔄 Reconnecting to primary broker: {}:{}",
            primary.host, primary.port
        );

        // Disconnect old client
        {
            let mut guard = self.inner.write().await;
            if let Some(client) = guard.take() {
                let _ = client.disconnect().await;
            }
        }

        // Connect to primary
        let new_client = MqttClient::new(&self.config).await?;

        {
            let mut guard = self.inner.write().await;
            *guard = Some(new_client);
        }

        self.failover_manager.record_success().await;
        info!("✅ Reconnected to primary broker");

        Ok(())
    }

    /// Disconnect and cleanup
    pub async fn disconnect(mut self) -> Result<()> {
        // Shutdown failover manager
        self.failover_manager.shutdown();

        // Cancel health check task
        if let Some(handle) = self.health_check_handle.take() {
            handle.abort();
        }

        // Cancel reconnect task
        if let Some(handle) = self.reconnect_handle.take() {
            handle.abort();
        }

        // Disconnect inner client
        let mut guard = self.inner.write().await;
        if let Some(client) = guard.take() {
            client.disconnect().await?;
        }

        info!("Failover MQTT client disconnected");
        Ok(())
    }

    /// Get failover status report (JSON)
    pub async fn get_failover_status(&self) -> serde_json::Value {
        self.failover_manager.get_status_report().await
    }
}
