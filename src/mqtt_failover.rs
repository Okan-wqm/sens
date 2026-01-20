//! MQTT Broker Failover for High Availability (v1.3.4)
//!
//! Provides automatic failover between primary and backup MQTT brokers
//! to ensure continuous connectivity in case of broker failures.
//!
//! ## Features
//! - Automatic failover to backup broker on primary failure
//! - Health checks to detect primary broker recovery
//! - Graceful switchback to primary when available
//! - Offline queue integration for zero message loss
//!
//! ## State Machine
//! ```text
//! ┌──────────────┐  connect fail   ┌───────────────┐
//! │   PRIMARY    │ ───────────────▶│  CONNECTING   │
//! │   ACTIVE     │                 │  TO BACKUP    │
//! └──────▲───────┘                 └───────┬───────┘
//!        │                                 │
//!        │ primary                         │ backup
//!        │ recovered                       │ connected
//!        │                                 ▼
//! ┌──────┴───────┐  health check   ┌───────────────┐
//! │   CHECKING   │ ◀───────────────│    BACKUP     │
//! │   PRIMARY    │   (periodic)    │    ACTIVE     │
//! └──────────────┘                 └───────────────┘
//! ```

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, watch, RwLock};
use tracing::{debug, error, info, warn};

use crate::config::MqttFailoverConfig;

/// Failover state representing current broker connection status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailoverState {
    /// Connected to primary broker (normal operation)
    PrimaryActive,
    /// Primary failed, attempting to connect to backup
    ConnectingToBackup,
    /// Connected to backup broker
    BackupActive,
    /// On backup, checking if primary is back online
    CheckingPrimary,
    /// Transitioning from backup to primary
    SwitchingToPrimary,
    /// Both brokers unavailable
    Disconnected,
}

impl std::fmt::Display for FailoverState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FailoverState::PrimaryActive => write!(f, "PRIMARY_ACTIVE"),
            FailoverState::ConnectingToBackup => write!(f, "CONNECTING_TO_BACKUP"),
            FailoverState::BackupActive => write!(f, "BACKUP_ACTIVE"),
            FailoverState::CheckingPrimary => write!(f, "CHECKING_PRIMARY"),
            FailoverState::SwitchingToPrimary => write!(f, "SWITCHING_TO_PRIMARY"),
            FailoverState::Disconnected => write!(f, "DISCONNECTED"),
        }
    }
}

/// Broker endpoint information
#[derive(Debug, Clone)]
pub struct BrokerEndpoint {
    pub host: String,
    pub port: u16,
    pub is_primary: bool,
}

impl BrokerEndpoint {
    pub fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

/// Statistics for failover operations
#[derive(Debug, Default)]
pub struct FailoverStats {
    /// Number of times failover to backup occurred
    pub failover_count: AtomicU32,
    /// Number of times recovered back to primary
    pub recovery_count: AtomicU32,
    /// Total time spent on backup broker (seconds)
    pub backup_time_secs: AtomicU64,
    /// Last failover timestamp (unix epoch)
    pub last_failover_timestamp: AtomicU64,
    /// Last recovery timestamp (unix epoch)
    pub last_recovery_timestamp: AtomicU64,
    /// Consecutive failures on current broker
    pub consecutive_failures: AtomicU32,
}

impl FailoverStats {
    pub fn record_failover(&self) {
        self.failover_count.fetch_add(1, Ordering::Relaxed);
        self.last_failover_timestamp.store(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            Ordering::Relaxed,
        );
        self.consecutive_failures.store(0, Ordering::Relaxed);
    }

    pub fn record_recovery(&self) {
        self.recovery_count.fetch_add(1, Ordering::Relaxed);
        self.last_recovery_timestamp.store(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            Ordering::Relaxed,
        );
        self.consecutive_failures.store(0, Ordering::Relaxed);
    }

    pub fn record_failure(&self) -> u32 {
        self.consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub fn reset_failures(&self) {
        self.consecutive_failures.store(0, Ordering::Relaxed);
    }

    pub fn get_consecutive_failures(&self) -> u32 {
        self.consecutive_failures.load(Ordering::Relaxed)
    }
}

/// Failover manager that coordinates broker switching
pub struct FailoverManager {
    /// Current failover state
    state: Arc<RwLock<FailoverState>>,
    /// State change notification channel
    state_tx: watch::Sender<FailoverState>,
    /// Primary broker endpoint
    primary: BrokerEndpoint,
    /// Backup broker endpoint (optional)
    backup: Option<BrokerEndpoint>,
    /// Failover configuration
    config: MqttFailoverConfig,
    /// Failover statistics
    stats: Arc<FailoverStats>,
    /// Shutdown signal
    shutdown_tx: broadcast::Sender<()>,
    /// Time when switched to backup
    backup_start_time: Arc<RwLock<Option<Instant>>>,
}

impl FailoverManager {
    /// Create a new failover manager
    pub fn new(
        primary_host: String,
        primary_port: u16,
        config: MqttFailoverConfig,
    ) -> (Self, watch::Receiver<FailoverState>) {
        let primary = BrokerEndpoint {
            host: primary_host,
            port: primary_port,
            is_primary: true,
        };

        let backup = if config.enabled {
            config.backup_broker.as_ref().map(|host| BrokerEndpoint {
                host: host.clone(),
                port: config.backup_port.unwrap_or(primary_port),
                is_primary: false,
            })
        } else {
            None
        };

        let initial_state = FailoverState::PrimaryActive;
        let (state_tx, state_rx) = watch::channel(initial_state);
        let (shutdown_tx, _) = broadcast::channel(1);

        let manager = Self {
            state: Arc::new(RwLock::new(initial_state)),
            state_tx,
            primary,
            backup,
            config,
            stats: Arc::new(FailoverStats::default()),
            shutdown_tx,
            backup_start_time: Arc::new(RwLock::new(None)),
        };

        (manager, state_rx)
    }

    /// Check if failover is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled && self.backup.is_some()
    }

    /// Get current state
    pub async fn get_state(&self) -> FailoverState {
        *self.state.read().await
    }

    /// Get current active broker endpoint
    pub async fn get_active_broker(&self) -> &BrokerEndpoint {
        let state = self.state.read().await;
        match *state {
            FailoverState::PrimaryActive
            | FailoverState::CheckingPrimary
            | FailoverState::SwitchingToPrimary => &self.primary,
            FailoverState::BackupActive | FailoverState::ConnectingToBackup => {
                self.backup.as_ref().unwrap_or(&self.primary)
            }
            FailoverState::Disconnected => &self.primary, // Try primary first when disconnected
        }
    }

    /// Get primary broker endpoint
    pub fn get_primary(&self) -> &BrokerEndpoint {
        &self.primary
    }

    /// Get backup broker endpoint
    pub fn get_backup(&self) -> Option<&BrokerEndpoint> {
        self.backup.as_ref()
    }

    /// Get failover statistics
    pub fn get_stats(&self) -> &Arc<FailoverStats> {
        &self.stats
    }

    /// Get failover configuration
    pub fn get_config(&self) -> &MqttFailoverConfig {
        &self.config
    }

    /// Record a connection failure and determine if failover should occur
    pub async fn record_failure(&self) -> bool {
        let failures = self.stats.record_failure();
        let state = self.state.read().await;

        info!(
            "🔴 Connection failure recorded: {} consecutive failures (max: {}), state: {}",
            failures, self.config.max_failures, state
        );

        // Only trigger failover if:
        // 1. Failover is enabled
        // 2. We're on primary
        // 3. Failures exceed threshold
        if self.is_enabled()
            && *state == FailoverState::PrimaryActive
            && failures >= self.config.max_failures
        {
            drop(state);
            self.transition_to(FailoverState::ConnectingToBackup).await;
            return true;
        }

        false
    }

    /// Record a successful connection
    pub async fn record_success(&self) {
        self.stats.reset_failures();
        let state = *self.state.read().await;

        debug!("🟢 Connection success recorded, state: {}", state);

        match state {
            FailoverState::ConnectingToBackup => {
                self.transition_to(FailoverState::BackupActive).await;
            }
            FailoverState::CheckingPrimary => {
                // Primary is back! Start switching
                self.transition_to(FailoverState::SwitchingToPrimary).await;
            }
            FailoverState::SwitchingToPrimary => {
                self.transition_to(FailoverState::PrimaryActive).await;
            }
            FailoverState::Disconnected => {
                // Reconnected to primary
                self.transition_to(FailoverState::PrimaryActive).await;
            }
            _ => {}
        }
    }

    /// Transition to a new state
    async fn transition_to(&self, new_state: FailoverState) {
        let mut state = self.state.write().await;
        let old_state = *state;

        if old_state == new_state {
            return;
        }

        info!(
            "🔄 Failover state transition: {} -> {}",
            old_state, new_state
        );

        // Record statistics
        match (&old_state, &new_state) {
            (FailoverState::PrimaryActive, FailoverState::ConnectingToBackup) => {
                self.stats.record_failover();
                *self.backup_start_time.write().await = Some(Instant::now());
                warn!("⚠️  FAILOVER: Primary broker failed, switching to backup");
            }
            (_, FailoverState::BackupActive) => {
                info!("✅ FAILOVER COMPLETE: Now connected to backup broker");
            }
            (FailoverState::BackupActive, FailoverState::CheckingPrimary) => {
                debug!("🔍 Checking if primary broker is back online...");
            }
            (_, FailoverState::PrimaryActive) => {
                if old_state == FailoverState::SwitchingToPrimary
                    || old_state == FailoverState::BackupActive
                {
                    self.stats.record_recovery();
                    // Calculate backup time
                    if let Some(start) = *self.backup_start_time.read().await {
                        let backup_duration = start.elapsed().as_secs();
                        self.stats
                            .backup_time_secs
                            .fetch_add(backup_duration, Ordering::Relaxed);
                        info!(
                            "✅ RECOVERY COMPLETE: Back to primary broker (was on backup for {}s)",
                            backup_duration
                        );
                    }
                    *self.backup_start_time.write().await = None;
                }
            }
            _ => {}
        }

        *state = new_state;
        let _ = self.state_tx.send(new_state);
    }

    /// Manually trigger failover to backup
    pub async fn force_failover(&self) {
        if !self.is_enabled() {
            warn!("Cannot force failover: backup broker not configured");
            return;
        }

        let state = *self.state.read().await;
        if state == FailoverState::PrimaryActive {
            info!("🔧 Manual failover triggered");
            self.transition_to(FailoverState::ConnectingToBackup).await;
        }
    }

    /// Manually trigger recovery to primary
    pub async fn force_recovery(&self) {
        let state = *self.state.read().await;
        if state == FailoverState::BackupActive {
            info!("🔧 Manual recovery triggered");
            self.transition_to(FailoverState::CheckingPrimary).await;
        }
    }

    /// Start health check task that periodically checks primary broker
    pub fn start_health_check_task(&self) -> tokio::task::JoinHandle<()> {
        let state = self.state.clone();
        let state_tx = self.state_tx.clone();
        let primary = self.primary.clone();
        let interval = Duration::from_secs(self.config.health_check_interval_secs);
        let timeout = Duration::from_secs(self.config.timeout_secs);
        let recovery_delay = Duration::from_secs(self.config.recovery_delay_secs);
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        let backup_start_time = self.backup_start_time.clone();
        let stats = self.stats.clone();

        tokio::spawn(async move {
            info!(
                "🏥 Health check task started (interval: {}s, timeout: {}s)",
                interval.as_secs(),
                timeout.as_secs()
            );

            loop {
                tokio::select! {
                    _ = shutdown_rx.recv() => {
                        info!("Health check task shutting down");
                        break;
                    }
                    _ = tokio::time::sleep(interval) => {
                        let current_state = *state.read().await;

                        // Only check primary when on backup
                        if current_state != FailoverState::BackupActive {
                            continue;
                        }

                        debug!("🔍 Health check: testing primary broker {}...", primary.address());

                        // Try to connect to primary broker
                        match tokio::time::timeout(
                            timeout,
                            tokio::net::TcpStream::connect(primary.address()),
                        )
                        .await
                        {
                            Ok(Ok(_stream)) => {
                                info!("✅ Health check: Primary broker {} is reachable!", primary.address());

                                // Wait recovery delay before switching
                                tokio::time::sleep(recovery_delay).await;

                                // Double-check state hasn't changed
                                let current = *state.read().await;
                                if current == FailoverState::BackupActive {
                                    let mut state_guard = state.write().await;
                                    *state_guard = FailoverState::CheckingPrimary;
                                    let _ = state_tx.send(FailoverState::CheckingPrimary);
                                    drop(state_guard);

                                    // Record recovery
                                    stats.record_recovery();
                                    if let Some(start) = *backup_start_time.read().await {
                                        let duration = start.elapsed().as_secs();
                                        stats.backup_time_secs.fetch_add(duration, Ordering::Relaxed);
                                    }
                                    *backup_start_time.write().await = None;

                                    info!("🔄 Health check: Initiating switchback to primary broker");
                                }
                            }
                            Ok(Err(e)) => {
                                debug!("Health check: Primary broker unreachable: {}", e);
                            }
                            Err(_) => {
                                debug!("Health check: Connection to primary broker timed out");
                            }
                        }
                    }
                }
            }
        })
    }

    /// Shutdown the failover manager
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(());
    }

    /// Get a JSON-serializable status report
    pub async fn get_status_report(&self) -> serde_json::Value {
        let state = *self.state.read().await;
        let stats = &self.stats;

        serde_json::json!({
            "enabled": self.is_enabled(),
            "state": state.to_string(),
            "primary_broker": self.primary.address(),
            "backup_broker": self.backup.as_ref().map(|b| b.address()),
            "statistics": {
                "failover_count": stats.failover_count.load(Ordering::Relaxed),
                "recovery_count": stats.recovery_count.load(Ordering::Relaxed),
                "backup_time_total_secs": stats.backup_time_secs.load(Ordering::Relaxed),
                "consecutive_failures": stats.consecutive_failures.load(Ordering::Relaxed),
                "last_failover_timestamp": stats.last_failover_timestamp.load(Ordering::Relaxed),
                "last_recovery_timestamp": stats.last_recovery_timestamp.load(Ordering::Relaxed),
            },
            "config": {
                "timeout_secs": self.config.timeout_secs,
                "health_check_interval_secs": self.config.health_check_interval_secs,
                "max_failures": self.config.max_failures,
                "recovery_delay_secs": self.config.recovery_delay_secs,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> MqttFailoverConfig {
        MqttFailoverConfig {
            enabled: true,
            backup_broker: Some("backup.example.com".to_string()),
            backup_port: Some(8883),
            timeout_secs: 5,
            health_check_interval_secs: 30,
            max_failures: 3,
            recovery_delay_secs: 2,
        }
    }

    #[tokio::test]
    async fn test_failover_manager_creation() {
        let config = test_config();
        let (manager, _rx) = FailoverManager::new(
            "primary.example.com".to_string(),
            8883,
            config,
        );

        assert!(manager.is_enabled());
        assert_eq!(manager.get_state().await, FailoverState::PrimaryActive);
        assert_eq!(manager.get_primary().host, "primary.example.com");
        assert_eq!(manager.get_backup().unwrap().host, "backup.example.com");
    }

    #[tokio::test]
    async fn test_failover_disabled_without_backup() {
        let config = MqttFailoverConfig {
            enabled: true,
            backup_broker: None,
            ..Default::default()
        };
        let (manager, _rx) = FailoverManager::new(
            "primary.example.com".to_string(),
            8883,
            config,
        );

        assert!(!manager.is_enabled());
    }

    #[tokio::test]
    async fn test_failure_counting() {
        let config = test_config();
        let (manager, _rx) = FailoverManager::new(
            "primary.example.com".to_string(),
            8883,
            config,
        );

        // First two failures shouldn't trigger failover
        assert!(!manager.record_failure().await);
        assert_eq!(manager.get_stats().get_consecutive_failures(), 1);

        assert!(!manager.record_failure().await);
        assert_eq!(manager.get_stats().get_consecutive_failures(), 2);

        // Third failure should trigger failover
        assert!(manager.record_failure().await);
        assert_eq!(manager.get_state().await, FailoverState::ConnectingToBackup);
    }

    #[tokio::test]
    async fn test_success_resets_failures() {
        let config = test_config();
        let (manager, _rx) = FailoverManager::new(
            "primary.example.com".to_string(),
            8883,
            config,
        );

        manager.record_failure().await;
        manager.record_failure().await;
        assert_eq!(manager.get_stats().get_consecutive_failures(), 2);

        manager.record_success().await;
        assert_eq!(manager.get_stats().get_consecutive_failures(), 0);
    }

    #[tokio::test]
    async fn test_state_transitions() {
        let config = test_config();
        let (manager, mut rx) = FailoverManager::new(
            "primary.example.com".to_string(),
            8883,
            config,
        );

        // Initial state
        assert_eq!(manager.get_state().await, FailoverState::PrimaryActive);

        // Force failover
        manager.force_failover().await;
        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow(), FailoverState::ConnectingToBackup);

        // Successful backup connection
        manager.record_success().await;
        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow(), FailoverState::BackupActive);
    }

    #[tokio::test]
    async fn test_status_report() {
        let config = test_config();
        let (manager, _rx) = FailoverManager::new(
            "primary.example.com".to_string(),
            8883,
            config,
        );

        let report = manager.get_status_report().await;

        assert_eq!(report["enabled"], true);
        assert_eq!(report["state"], "PRIMARY_ACTIVE");
        assert_eq!(report["primary_broker"], "primary.example.com:8883");
        assert_eq!(report["backup_broker"], "backup.example.com:8883");
    }
}
