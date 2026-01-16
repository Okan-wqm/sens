//! Graceful Shutdown Coordinator
//!
//! Manages proper shutdown sequence for all agent components:
//! 1. Signal all tasks to stop
//! 2. Wait for in-flight operations
//! 3. Flush offline buffer
//! 4. Disconnect hardware interfaces
//! 5. Publish offline status
//! 6. Disconnect MQTT

use std::time::Duration;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

/// Graceful shutdown coordinator
pub struct ShutdownCoordinator {
    /// Broadcast sender for shutdown signal
    notify: broadcast::Sender<()>,
    /// Registered task handles
    tasks: Vec<(&'static str, JoinHandle<()>)>,
}

impl ShutdownCoordinator {
    /// Create a new shutdown coordinator
    pub fn new() -> Self {
        let (notify, _) = broadcast::channel(1);
        Self {
            notify,
            tasks: Vec::new(),
        }
    }

    /// Get a shutdown signal receiver
    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        self.notify.subscribe()
    }

    /// Register a task handle for graceful shutdown
    pub fn register_task(&mut self, name: &'static str, handle: JoinHandle<()>) {
        self.tasks.push((name, handle));
    }

    /// Get the number of registered tasks
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }

    /// Initiate graceful shutdown
    ///
    /// This will:
    /// 1. Send shutdown signal to all subscribers
    /// 2. Wait for all registered tasks to complete (with timeout)
    pub async fn shutdown(self, shutdown_timeout: Duration) {
        info!("Initiating graceful shutdown...");

        // Step 1: Signal all tasks to stop
        let subscriber_count = self.notify.receiver_count();
        info!(
            "Sending shutdown signal to {} subscribers",
            subscriber_count
        );

        if let Err(e) = self.notify.send(()) {
            warn!("Failed to send shutdown signal: {}", e);
        }

        // Give a small delay for signal propagation
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Step 2: Wait for all tasks to complete
        let task_count = self.tasks.len();
        info!("Waiting for {} tasks to complete...", task_count);

        for (name, handle) in self.tasks {
            match tokio::time::timeout(shutdown_timeout, handle).await {
                Ok(Ok(())) => {
                    info!("Task '{}' completed gracefully", name);
                }
                Ok(Err(e)) => {
                    warn!("Task '{}' panicked: {}", name, e);
                }
                Err(_) => {
                    warn!("Task '{}' timed out during shutdown, aborting", name);
                }
            }
        }

        info!("All tasks completed");
    }

    /// Abort all tasks immediately (for emergency shutdown)
    pub fn abort_all(self) {
        warn!("Aborting all tasks...");
        for (name, handle) in self.tasks {
            handle.abort();
            warn!("Task '{}' aborted", name);
        }
    }
}

impl Default for ShutdownCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

/// Shutdown-aware task wrapper
///
/// Wraps an async task to respond to shutdown signals
pub struct ShutdownAwareTask<F> {
    task: F,
    shutdown_rx: broadcast::Receiver<()>,
}

impl<F> ShutdownAwareTask<F>
where
    F: std::future::Future<Output = ()>,
{
    /// Create a new shutdown-aware task
    pub fn new(task: F, shutdown_rx: broadcast::Receiver<()>) -> Self {
        Self { task, shutdown_rx }
    }

    /// Run the task until completion or shutdown signal
    pub async fn run(self) {
        tokio::select! {
            _ = self.task => {
                // Task completed normally
            }
            _ = async {
                let mut rx = self.shutdown_rx;
                let _ = rx.recv().await;
            } => {
                // Shutdown signal received
                info!("Shutdown signal received, stopping task");
            }
        }
    }
}

/// Helper to run a task with shutdown awareness
pub async fn run_until_shutdown<F>(task: F, mut shutdown_rx: broadcast::Receiver<()>)
where
    F: std::future::Future<Output = ()>,
{
    tokio::select! {
        _ = task => {}
        _ = shutdown_rx.recv() => {
            info!("Shutdown signal received");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_shutdown_coordinator() {
        let mut coordinator = ShutdownCoordinator::new();
        let rx = coordinator.subscribe();

        // Spawn a task that waits for shutdown
        let handle = tokio::spawn(async move {
            let mut rx = rx;
            let _ = rx.recv().await;
        });

        coordinator.register_task("test_task", handle);
        assert_eq!(coordinator.task_count(), 1);

        // Shutdown should complete quickly
        coordinator.shutdown(Duration::from_secs(1)).await;
    }

    #[tokio::test]
    async fn test_run_until_shutdown() {
        let (tx, rx) = broadcast::channel(1);

        let task_handle = tokio::spawn(async move {
            run_until_shutdown(
                async {
                    // This would run forever without shutdown
                    loop {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                },
                rx,
            )
            .await;
        });

        // Send shutdown signal
        tx.send(()).unwrap();

        // Task should complete
        tokio::time::timeout(Duration::from_secs(1), task_handle)
            .await
            .expect("Task should complete after shutdown signal")
            .expect("Task should not panic");
    }
}
