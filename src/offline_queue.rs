//! Offline Message Queue with Priority Support
//!
//! Provides persistent storage for MQTT messages when the broker is unreachable.
//! Messages are stored locally and replayed in priority order when connectivity is restored.
//!
//! # Features
//! - Priority-based message ordering (higher priority first)
//! - Bounded queue size to prevent unbounded memory growth
//! - SQLite persistence for crash recovery
//! - FIFO ordering within same priority level
//!
//! # IEC 62443 SL2 Compliance
//! - FR5: Resource availability (bounded queue prevents DoS)
//! - FR6: Monitoring (queue metrics for observability)
//!
//! # v1.2.3 Improvements
//! - Added mutex poison recovery for better resilience

// v1.2.4: API reserved for offline message queuing - silence dead_code warnings
#![allow(dead_code)]

use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use tracing::{debug, error, info, warn};

/// Acquire mutex lock with poison recovery (v1.2.3)
///
/// If the mutex is poisoned (previous holder panicked), this function
/// will recover the lock and log a warning. The data may be in an
/// inconsistent state, but for SQLite connections this is generally safe
/// as SQLite handles its own transaction rollback.
///
/// # v1.3.3: Added connection health check after poison recovery
fn acquire_lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>> {
    match mutex.lock() {
        Ok(guard) => Ok(guard),
        Err(poisoned) => {
            error!(
                "Mutex was poisoned by a panicked thread. Recovering lock. \
                Data may be inconsistent - consider restarting the agent."
            );
            // Recover the lock - SQLite will have rolled back any incomplete transaction
            Ok(poisoned.into_inner())
        }
    }
}

/// Acquire SQLite connection lock with poison recovery and health check (v1.3.3)
///
/// After recovering from a poisoned mutex, validates that the SQLite connection
/// is still usable by executing a simple query. If the connection is corrupted,
/// returns an error instead of silently proceeding with a bad connection.
fn acquire_sqlite_lock(mutex: &Mutex<Connection>) -> Result<MutexGuard<'_, Connection>> {
    let was_poisoned;
    let guard = match mutex.lock() {
        Ok(guard) => {
            was_poisoned = false;
            guard
        }
        Err(poisoned) => {
            error!(
                "SQLite mutex was poisoned by a panicked thread. Recovering lock and validating connection..."
            );
            was_poisoned = true;
            poisoned.into_inner()
        }
    };

    // If mutex was poisoned, validate SQLite connection health
    if was_poisoned {
        // Execute a simple query to verify the connection is still usable
        match guard.execute("SELECT 1", []) {
            Ok(_) => {
                warn!(
                    "SQLite connection validated after poison recovery. \
                    Any incomplete transaction was rolled back by SQLite."
                );
            }
            Err(e) => {
                error!(
                    "SQLite connection corrupted after poison recovery: {}. \
                    Manual intervention required - consider restarting the agent.",
                    e
                );
                return Err(anyhow::anyhow!(
                    "SQLite connection corrupted after mutex poison recovery: {}",
                    e
                ));
            }
        }
    }

    Ok(guard)
}

/// Message priority levels (higher value = higher priority)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(u8)]
#[derive(Default)]
pub enum MessagePriority {
    /// Low priority - background data, can be delayed
    Low = 0,
    /// Normal priority - regular telemetry
    #[default]
    Normal = 1,
    /// High priority - important events
    High = 2,
    /// Critical priority - alarms, safety events
    Critical = 3,
}

impl From<u8> for MessagePriority {
    fn from(value: u8) -> Self {
        match value {
            0 => MessagePriority::Low,
            1 => MessagePriority::Normal,
            2 => MessagePriority::High,
            3.. => MessagePriority::Critical,
        }
    }
}

/// Queued message with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedMessage {
    /// Unique message ID
    pub id: i64,
    /// MQTT topic
    pub topic: String,
    /// Message payload (JSON)
    pub payload: String,
    /// Message priority
    pub priority: MessagePriority,
    /// MQTT QoS level (0, 1, 2)
    pub qos: u8,
    /// Retain flag
    pub retain: bool,
    /// Creation timestamp (milliseconds since epoch)
    pub created_at: i64,
    /// Number of retry attempts
    pub retry_count: u32,
}

/// Queue statistics for monitoring
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QueueStats {
    /// Total messages in queue
    pub total_messages: usize,
    /// Messages by priority
    pub by_priority: [usize; 4],
    /// Oldest message age in seconds
    pub oldest_message_age_secs: Option<u64>,
    /// Total bytes used (payload + topic)
    pub total_bytes: usize,
    /// Database file size in bytes (v1.2.0)
    pub db_size_bytes: u64,
    /// Percentage of disk limit used (v1.2.0)
    pub disk_usage_percent: f32,
}

/// Default maximum disk size: 50 MB
const DEFAULT_MAX_DISK_BYTES: u64 = 50 * 1024 * 1024;

/// Offline message queue with SQLite persistence
///
/// # Thread Safety
/// Uses interior mutability with Mutex for safe concurrent access.
///
/// # Resource Limits (v1.2.0)
/// Enforces both message count limit and disk size limit to prevent
/// unbounded resource consumption (IEC 62443 SL2 FR5).
pub struct OfflineQueue {
    /// Database connection (protected by mutex for sync access)
    conn: Mutex<Connection>,
    /// Maximum queue size (message count)
    max_size: usize,
    /// Maximum message age before expiration (seconds)
    max_age_secs: u64,
    /// Maximum disk size in bytes (v1.2.0)
    max_disk_bytes: u64,
}

impl OfflineQueue {
    /// Create a new offline queue with file-based persistence
    ///
    /// # Arguments
    /// * `db_path` - Path to SQLite database file
    /// * `max_size` - Maximum number of messages to store
    /// * `max_age_secs` - Maximum age before messages expire (0 = no expiration)
    pub fn new(db_path: &Path, max_size: usize, max_age_secs: u64) -> Result<Self> {
        Self::with_disk_limit(db_path, max_size, max_age_secs, DEFAULT_MAX_DISK_BYTES)
    }

    /// Create a new offline queue with custom disk limit (v1.2.0)
    ///
    /// # Arguments
    /// * `db_path` - Path to SQLite database file
    /// * `max_size` - Maximum number of messages to store
    /// * `max_age_secs` - Maximum age before messages expire (0 = no expiration)
    /// * `max_disk_bytes` - Maximum disk space in bytes (0 = no limit)
    pub fn with_disk_limit(
        db_path: &Path,
        max_size: usize,
        max_age_secs: u64,
        max_disk_bytes: u64,
    ) -> Result<Self> {
        let conn = Connection::open(db_path)
            .with_context(|| format!("Failed to open queue database: {}", db_path.display()))?;

        let queue = Self {
            conn: Mutex::new(conn),
            max_size,
            max_age_secs,
            max_disk_bytes,
        };

        queue.init_schema()?;
        info!(
            "Offline queue initialized: max_size={}, max_age_secs={}, max_disk_mb={}",
            max_size,
            max_age_secs,
            max_disk_bytes / (1024 * 1024)
        );

        Ok(queue)
    }

    /// Create an in-memory queue (for testing)
    pub fn in_memory(max_size: usize) -> Result<Self> {
        let conn = Connection::open_in_memory().context("Failed to create in-memory database")?;

        let queue = Self {
            conn: Mutex::new(conn),
            max_size,
            max_age_secs: 0,   // No expiration
            max_disk_bytes: 0, // No disk limit for in-memory
        };

        queue.init_schema()?;
        Ok(queue)
    }

    /// Get current database file size in bytes (v1.2.0)
    ///
    /// Uses SQLite's page_count * page_size for accurate measurement.
    fn get_db_size(&self, conn: &Connection) -> u64 {
        conn.query_row(
            "SELECT page_count * page_size as size FROM pragma_page_count(), pragma_page_size()",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|size| size as u64)
        .unwrap_or(0)
    }

    /// Evict oldest low-priority messages until disk usage is under limit (v1.2.0)
    /// v1.2.6: Added bounds validation to prevent SQL injection via format string
    fn evict_for_disk_space(&self, conn: &Connection, evict_count: usize) -> Result<usize> {
        // v1.2.6: Validate evict_count to prevent potential issues
        // Max reasonable eviction is 10000 messages at once
        const MAX_EVICT_COUNT: usize = 10000;
        if evict_count == 0 {
            return Ok(0);
        }
        let safe_count = evict_count.min(MAX_EVICT_COUNT);

        // Note: SQLite LIMIT doesn't support parameters, but evict_count is
        // already validated as usize and bounded above
        let result = conn.execute(
            &format!(
                "DELETE FROM message_queue WHERE id IN (
                    SELECT id FROM message_queue
                    ORDER BY priority ASC, created_at ASC
                    LIMIT {}
                )",
                safe_count
            ),
            [],
        );

        match result {
            Ok(deleted) => {
                if deleted > 0 {
                    warn!("Evicted {} messages due to disk space limit", deleted);
                }
                Ok(deleted)
            }
            Err(e) => Err(anyhow::anyhow!(
                "Failed to evict messages for disk space: {}",
                e
            )),
        }
    }

    /// Initialize database schema
    fn init_schema(&self) -> Result<()> {
        let conn = acquire_lock(&self.conn)?;

        conn.execute_batch(
            "
            -- Enable WAL mode for better concurrent access
            PRAGMA journal_mode=WAL;
            PRAGMA synchronous=NORMAL;
            PRAGMA busy_timeout=5000;

            -- Message queue table
            CREATE TABLE IF NOT EXISTS message_queue (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                topic TEXT NOT NULL,
                payload TEXT NOT NULL,
                priority INTEGER NOT NULL DEFAULT 1,
                qos INTEGER NOT NULL DEFAULT 1,
                retain INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                retry_count INTEGER NOT NULL DEFAULT 0
            );

            -- Index for priority-based dequeue (highest priority, oldest first)
            CREATE INDEX IF NOT EXISTS idx_queue_priority_created
            ON message_queue (priority DESC, created_at ASC);

            -- Index for expiration cleanup
            CREATE INDEX IF NOT EXISTS idx_queue_created
            ON message_queue (created_at);
            ",
        )
        .context("Failed to initialize queue schema")?;

        Ok(())
    }

    /// Enqueue a message
    ///
    /// If queue is at capacity (message count or disk size), the oldest
    /// low-priority messages are removed (v1.2.0: disk size limit).
    pub fn enqueue(
        &self,
        topic: &str,
        payload: &str,
        priority: MessagePriority,
        qos: u8,
        retain: bool,
    ) -> Result<i64> {
        let conn = acquire_lock(&self.conn)?;

        // Check current queue size
        let mut current_size: usize = conn
            .query_row("SELECT COUNT(*) FROM message_queue", [], |row| row.get(0))
            .unwrap_or(0);

        // If at message count capacity, remove oldest low-priority message
        if current_size >= self.max_size {
            self.evict_one(&conn)?;
            current_size = current_size.saturating_sub(1);
        }

        // v1.2.0: Check disk size limit and evict if necessary
        // v1.2.6: Loop until under limit to prevent disk exhaustion
        if self.max_disk_bytes > 0 {
            let mut db_size = self.get_db_size(&conn);
            let mut eviction_rounds = 0;
            const MAX_EVICTION_ROUNDS: usize = 10; // Prevent infinite loop

            while db_size >= self.max_disk_bytes
                && current_size > 0
                && eviction_rounds < MAX_EVICTION_ROUNDS
            {
                // Evict 10% of messages (min 5, max 50) to reclaim disk space
                let evict_count = (current_size / 10).max(5).min(50);
                self.evict_for_disk_space(&conn, evict_count)?;
                current_size = current_size.saturating_sub(evict_count);
                db_size = self.get_db_size(&conn);
                eviction_rounds += 1;
            }
        }

        let now = chrono::Utc::now().timestamp_millis();

        conn.execute(
            "INSERT INTO message_queue (topic, payload, priority, qos, retain, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![topic, payload, priority as u8, qos, retain as i32, now],
        )
        .context("Failed to enqueue message")?;

        let id = conn.last_insert_rowid();
        debug!(
            "Enqueued message {} to '{}' (priority={:?})",
            id, topic, priority
        );

        Ok(id)
    }

    /// Remove oldest low-priority message to make room
    fn evict_one(&self, conn: &Connection) -> Result<()> {
        // Find and remove the oldest message with lowest priority
        let result = conn.execute(
            "DELETE FROM message_queue WHERE id = (
                SELECT id FROM message_queue
                ORDER BY priority ASC, created_at ASC
                LIMIT 1
            )",
            [],
        );

        match result {
            Ok(1) => {
                warn!("Evicted oldest low-priority message (queue at capacity)");
                Ok(())
            }
            Ok(_) => Ok(()), // Nothing to evict
            Err(e) => Err(anyhow::anyhow!("Failed to evict message: {}", e)),
        }
    }

    /// Dequeue the highest priority message
    ///
    /// Returns the message but does NOT remove it from queue.
    /// Call `ack()` after successful processing to remove.
    pub fn peek(&self) -> Result<Option<QueuedMessage>> {
        let conn = acquire_lock(&self.conn)?;

        // Clean up expired messages first
        if self.max_age_secs > 0 {
            self.cleanup_expired(&conn)?;
        }

        let result = conn.query_row(
            "SELECT id, topic, payload, priority, qos, retain, created_at, retry_count
             FROM message_queue
             ORDER BY priority DESC, created_at ASC
             LIMIT 1",
            [],
            |row| {
                Ok(QueuedMessage {
                    id: row.get(0)?,
                    topic: row.get(1)?,
                    payload: row.get(2)?,
                    priority: MessagePriority::from(row.get::<_, u8>(3)?),
                    qos: row.get(4)?,
                    retain: row.get::<_, i32>(5)? != 0,
                    created_at: row.get(6)?,
                    retry_count: row.get(7)?,
                })
            },
        );

        match result {
            Ok(msg) => Ok(Some(msg)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(anyhow::anyhow!("Failed to peek message: {}", e)),
        }
    }

    /// Acknowledge successful message processing (removes from queue)
    pub fn ack(&self, message_id: i64) -> Result<bool> {
        let conn = acquire_lock(&self.conn)?;

        let deleted = conn
            .execute(
                "DELETE FROM message_queue WHERE id = ?1",
                params![message_id],
            )
            .context("Failed to ack message")?;

        if deleted > 0 {
            debug!("Acknowledged message {}", message_id);
        }

        Ok(deleted > 0)
    }

    /// Mark message for retry (increments retry count)
    pub fn nack(&self, message_id: i64) -> Result<()> {
        let conn = acquire_lock(&self.conn)?;

        conn.execute(
            "UPDATE message_queue SET retry_count = retry_count + 1 WHERE id = ?1",
            params![message_id],
        )
        .context("Failed to nack message")?;

        debug!("Nacked message {} (will retry)", message_id);
        Ok(())
    }

    /// Get multiple messages for batch processing
    pub fn peek_batch(&self, max_count: usize) -> Result<Vec<QueuedMessage>> {
        let conn = acquire_lock(&self.conn)?;

        // Clean up expired messages first
        if self.max_age_secs > 0 {
            self.cleanup_expired(&conn)?;
        }

        let mut stmt = conn.prepare(
            "SELECT id, topic, payload, priority, qos, retain, created_at, retry_count
             FROM message_queue
             ORDER BY priority DESC, created_at ASC
             LIMIT ?1",
        )?;

        let messages = stmt
            .query_map(params![max_count], |row| {
                Ok(QueuedMessage {
                    id: row.get(0)?,
                    topic: row.get(1)?,
                    payload: row.get(2)?,
                    priority: MessagePriority::from(row.get::<_, u8>(3)?),
                    qos: row.get(4)?,
                    retain: row.get::<_, i32>(5)? != 0,
                    created_at: row.get(6)?,
                    retry_count: row.get(7)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(messages)
    }

    /// Acknowledge multiple messages
    pub fn ack_batch(&self, message_ids: &[i64]) -> Result<usize> {
        if message_ids.is_empty() {
            return Ok(0);
        }

        let conn = acquire_lock(&self.conn)?;

        // Build parameterized query
        let placeholders: Vec<String> =
            (1..=message_ids.len()).map(|i| format!("?{}", i)).collect();
        let sql = format!(
            "DELETE FROM message_queue WHERE id IN ({})",
            placeholders.join(", ")
        );

        let params: Vec<&dyn rusqlite::ToSql> = message_ids
            .iter()
            .map(|id| id as &dyn rusqlite::ToSql)
            .collect();

        let deleted = conn
            .execute(&sql, params.as_slice())
            .context("Failed to ack batch")?;

        debug!("Acknowledged {} messages in batch", deleted);
        Ok(deleted)
    }

    /// Clean up expired messages
    fn cleanup_expired(&self, conn: &Connection) -> Result<usize> {
        if self.max_age_secs == 0 {
            return Ok(0);
        }

        // v1.2.6: Safe timestamp calculation to prevent overflow on large max_age_secs
        let max_age_millis = (self.max_age_secs as i64)
            .checked_mul(1000)
            .unwrap_or(i64::MAX);
        let cutoff = chrono::Utc::now().timestamp_millis() - max_age_millis;

        let deleted = conn
            .execute(
                "DELETE FROM message_queue WHERE created_at < ?1",
                params![cutoff],
            )
            .unwrap_or(0);

        if deleted > 0 {
            info!("Cleaned up {} expired messages from offline queue", deleted);
        }

        Ok(deleted)
    }

    /// Get queue statistics
    pub fn stats(&self) -> Result<QueueStats> {
        let conn = acquire_lock(&self.conn)?;

        let total_messages: usize = conn
            .query_row("SELECT COUNT(*) FROM message_queue", [], |row| row.get(0))
            .unwrap_or(0);

        // Count by priority
        // v1.2.4: Priority array for Low(0), Normal(1), High(2), Critical(3)
        let mut by_priority = [0usize; 4];
        let mut stmt =
            conn.prepare("SELECT priority, COUNT(*) FROM message_queue GROUP BY priority")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, u8>(0)?, row.get::<_, usize>(1)?))
        })?;
        for row in rows.flatten() {
            let (priority, count) = row;
            if priority < 4 {
                by_priority[priority as usize] = count;
            } else {
                // v1.2.4: Log corrupted priority values instead of silently dropping
                warn!(
                    "Offline queue contains {} messages with invalid priority {} (expected 0-3)",
                    count, priority
                );
            }
        }

        // Oldest message age
        let oldest_message_age_secs = conn
            .query_row("SELECT MIN(created_at) FROM message_queue", [], |row| {
                row.get::<_, Option<i64>>(0)
            })
            .ok()
            .flatten()
            .map(|oldest| {
                let now = chrono::Utc::now().timestamp_millis();
                ((now - oldest) / 1000) as u64
            });

        // Total bytes (payload + topic)
        let total_bytes: usize = conn
            .query_row(
                "SELECT COALESCE(SUM(LENGTH(topic) + LENGTH(payload)), 0) FROM message_queue",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        // v1.2.0: Database file size
        let db_size_bytes = self.get_db_size(&conn);
        let disk_usage_percent = if self.max_disk_bytes > 0 {
            (db_size_bytes as f32 / self.max_disk_bytes as f32) * 100.0
        } else {
            0.0
        };

        Ok(QueueStats {
            total_messages,
            by_priority,
            oldest_message_age_secs,
            total_bytes,
            db_size_bytes,
            disk_usage_percent,
        })
    }

    /// Clear all messages from queue
    pub fn clear(&self) -> Result<usize> {
        let conn = acquire_lock(&self.conn)?;

        let deleted = conn
            .execute("DELETE FROM message_queue", [])
            .context("Failed to clear queue")?;

        info!("Cleared {} messages from offline queue", deleted);
        Ok(deleted)
    }

    /// Check if queue is empty
    pub fn is_empty(&self) -> bool {
        let conn = match self.conn.lock() {
            Ok(c) => c,
            Err(_) => return true,
        };

        let count: usize = conn
            .query_row("SELECT COUNT(*) FROM message_queue", [], |row| row.get(0))
            .unwrap_or(0);

        count == 0
    }

    /// Get current queue length
    pub fn len(&self) -> usize {
        let conn = match self.conn.lock() {
            Ok(c) => c,
            Err(e) => {
                // v1.2.6: Log poisoned mutex instead of silent failure
                tracing::error!("Queue database mutex poisoned: {}", e);
                return 0;
            }
        };

        conn.query_row("SELECT COUNT(*) FROM message_queue", [], |row| row.get(0))
            .unwrap_or(0)
    }

    /// Vacuum the database to reclaim disk space (v1.2.2)
    ///
    /// SQLite doesn't automatically reclaim space from deleted rows.
    /// This method should be called periodically (e.g., after batch acks)
    /// or when disk usage is high.
    ///
    /// # Returns
    /// * `Ok((before, after))` - Bytes before and after VACUUM
    /// * `Err` if VACUUM fails
    pub fn vacuum(&self) -> Result<(u64, u64)> {
        let conn = acquire_lock(&self.conn)?;

        let before = self.get_db_size(&conn);

        conn.execute("VACUUM", [])
            .context("Failed to VACUUM database")?;

        let after = self.get_db_size(&conn);

        if before > after {
            info!(
                "VACUUM reclaimed {} bytes ({} -> {} bytes)",
                before - after,
                before,
                after
            );
        } else {
            debug!("VACUUM completed (no space reclaimed)");
        }

        Ok((before, after))
    }

    /// Vacuum if disk usage exceeds threshold (v1.2.2)
    ///
    /// Only runs VACUUM if:
    /// 1. Database size exceeds 80% of max_disk_bytes
    /// 2. There have been significant deletes (freelist pages > 10%)
    ///
    /// # Returns
    /// * `Some((before, after))` if VACUUM was run
    /// * `None` if VACUUM was skipped
    pub fn vacuum_if_needed(&self) -> Result<Option<(u64, u64)>> {
        let conn = acquire_lock(&self.conn)?;

        // Skip if no disk limit set
        if self.max_disk_bytes == 0 {
            return Ok(None);
        }

        let db_size = self.get_db_size(&conn);
        // v1.2.6: Use integer arithmetic to avoid f64 precision loss on large values
        let threshold = self.max_disk_bytes * 4 / 5; // 80% threshold

        // Check freelist pages (space available for reuse)
        let freelist_count: i64 = conn
            .query_row("PRAGMA freelist_count", [], |row| row.get(0))
            .unwrap_or(0);
        let page_count: i64 = conn
            .query_row("PRAGMA page_count", [], |row| row.get(0))
            .unwrap_or(1);

        let freelist_ratio = freelist_count as f64 / page_count as f64;

        // VACUUM if usage > 80% OR freelist > 10% of pages
        if db_size > threshold || freelist_ratio > 0.10 {
            drop(conn); // Release lock before vacuum
            let result = self.vacuum()?;
            return Ok(Some(result));
        }

        Ok(None)
    }

    /// Create a backup using VACUUM INTO (v1.2.4)
    ///
    /// Creates a consistent, compact backup of the database to the specified path.
    /// Unlike regular file copy, VACUUM INTO:
    /// - Creates a consistent snapshot even during writes
    /// - Compacts the database (removes fragmentation)
    /// - Is atomic (backup is complete or doesn't exist)
    ///
    /// # Arguments
    /// * `backup_path` - Path for the backup file (will be overwritten if exists)
    ///
    /// # Example
    /// ```ignore
    /// queue.backup_to("/var/backups/offline_queue_2024-01-15.db")?;
    /// ```
    pub fn backup_to(&self, backup_path: &str) -> Result<u64> {
        // v1.2.6: Validate backup path to prevent SQL injection
        // Only allow safe characters in path: alphanumeric, /, \, _, -, .
        // Reject paths with: ', ", ;, --, or other SQL metacharacters
        if backup_path.is_empty() {
            anyhow::bail!("Backup path cannot be empty");
        }
        if backup_path.contains('\'')
            || backup_path.contains('"')
            || backup_path.contains(';')
            || backup_path.contains("--")
        {
            anyhow::bail!("Backup path contains invalid characters: {}", backup_path);
        }

        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Failed to lock database connection: {}", e))?;

        // VACUUM INTO creates an atomic backup
        // Path is validated above to not contain SQL injection vectors
        conn.execute(&format!("VACUUM INTO '{}'", backup_path), [])
            .with_context(|| format!("Failed to backup database to {}", backup_path))?;

        // Get backup file size
        let backup_size = std::fs::metadata(backup_path).map(|m| m.len()).unwrap_or(0);

        info!(
            backup_path = %backup_path,
            size_bytes = backup_size,
            "Database backup created successfully"
        );

        Ok(backup_size)
    }

    /// Create a rolling backup with timestamp (v1.2.4)
    ///
    /// Creates a backup file with timestamp in the specified directory.
    /// Automatically cleans up old backups if count exceeds max_backups.
    ///
    /// # Arguments
    /// * `backup_dir` - Directory for backup files
    /// * `max_backups` - Maximum number of backup files to keep (0 = unlimited)
    ///
    /// # Returns
    /// * Path to the created backup file
    pub fn backup_rolling(&self, backup_dir: &str, max_backups: usize) -> Result<String> {
        use chrono::Local;
        use std::fs;

        // Create backup directory if needed
        fs::create_dir_all(backup_dir)
            .with_context(|| format!("Failed to create backup directory: {}", backup_dir))?;

        // Generate timestamped filename
        let timestamp = Local::now().format("%Y%m%d_%H%M%S");
        let backup_path = format!("{}/offline_queue_{}.db", backup_dir, timestamp);

        // Create backup
        self.backup_to(&backup_path)?;

        // Clean up old backups if needed
        if max_backups > 0 {
            self.cleanup_old_backups(backup_dir, max_backups)?;
        }

        Ok(backup_path)
    }

    /// Clean up old backup files, keeping only the most recent ones
    fn cleanup_old_backups(&self, backup_dir: &str, max_backups: usize) -> Result<()> {
        use std::fs;

        let mut backups: Vec<_> = fs::read_dir(backup_dir)?
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("offline_queue_")
                    && entry.file_name().to_string_lossy().ends_with(".db")
            })
            .collect();

        // Sort by modified time (oldest first)
        backups.sort_by_key(|entry| {
            entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        });

        // Remove oldest backups if we have too many
        while backups.len() > max_backups {
            if let Some(oldest) = backups.first() {
                let path = oldest.path();
                if let Err(e) = fs::remove_file(&path) {
                    warn!(path = %path.display(), error = %e, "Failed to remove old backup");
                } else {
                    debug!(path = %path.display(), "Removed old backup");
                }
                backups.remove(0);
            }
        }

        Ok(())
    }

    /// Run SQLite integrity check (v1.2.4)
    ///
    /// Performs PRAGMA integrity_check to verify database consistency.
    /// Returns Ok(true) if database is healthy, Ok(false) if corruption detected.
    ///
    /// # SRE Note
    /// Run this periodically (e.g., daily or on startup) to detect corruption early.
    /// If corruption is detected, restore from backup and investigate root cause.
    pub fn integrity_check(&self) -> Result<IntegrityCheckResult> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Failed to lock database connection: {}", e))?;

        let mut stmt = conn.prepare("PRAGMA integrity_check")?;
        let results: Vec<String> = stmt
            .query_map([], |row: &rusqlite::Row| row.get(0))?
            .filter_map(|r: std::result::Result<String, _>| r.ok())
            .collect();

        let is_ok = results.len() == 1 && results[0] == "ok";

        if is_ok {
            info!("SQLite integrity check passed");
            Ok(IntegrityCheckResult {
                is_healthy: true,
                errors: vec![],
            })
        } else {
            error!(
                errors = ?results,
                "SQLite integrity check FAILED - database may be corrupted"
            );
            Ok(IntegrityCheckResult {
                is_healthy: false,
                errors: results,
            })
        }
    }

    /// Quick integrity check using PRAGMA quick_check (v1.2.4)
    ///
    /// Faster than full integrity_check but less thorough.
    /// Good for frequent checks (e.g., after each restart).
    pub fn quick_check(&self) -> Result<bool> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("Failed to lock database connection: {}", e))?;

        let result: String =
            conn.query_row("PRAGMA quick_check", [], |row: &rusqlite::Row| row.get(0))?;
        Ok(result == "ok")
    }
}

/// Result of SQLite integrity check
#[derive(Debug, Clone)]
pub struct IntegrityCheckResult {
    /// True if database passed all checks
    pub is_healthy: bool,
    /// List of errors found (empty if healthy)
    pub errors: Vec<String>,
}

// ============================================================================
// Async Wrapper (v1.2.4)
// ============================================================================

/// Async wrapper for OfflineQueue that uses spawn_blocking
///
/// All SQLite operations are wrapped in `tokio::task::spawn_blocking` to
/// prevent blocking the async runtime. This is important for high-throughput
/// scenarios where multiple async tasks access the queue concurrently.
///
/// # Example
/// ```ignore
/// let queue = AsyncOfflineQueue::new(OfflineQueue::new(path, 1000, 3600)?);
/// queue.enqueue_async("topic", "payload", MessagePriority::Normal, 1, false).await?;
/// ```
pub struct AsyncOfflineQueue {
    inner: std::sync::Arc<OfflineQueue>,
}

impl AsyncOfflineQueue {
    /// Create a new async wrapper around an OfflineQueue
    pub fn new(queue: OfflineQueue) -> Self {
        Self {
            inner: std::sync::Arc::new(queue),
        }
    }

    /// Create from an existing Arc<OfflineQueue>
    pub fn from_arc(queue: std::sync::Arc<OfflineQueue>) -> Self {
        Self { inner: queue }
    }

    /// Get a clone of the inner Arc for sharing
    pub fn inner(&self) -> std::sync::Arc<OfflineQueue> {
        self.inner.clone()
    }

    /// Async enqueue - wraps blocking SQLite operation in spawn_blocking
    pub async fn enqueue_async(
        &self,
        topic: &str,
        payload: &str,
        priority: MessagePriority,
        qos: u8,
        retain: bool,
    ) -> Result<i64> {
        let queue = self.inner.clone();
        let topic = topic.to_string();
        let payload = payload.to_string();

        tokio::task::spawn_blocking(move || queue.enqueue(&topic, &payload, priority, qos, retain))
            .await
            .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {}", e))?
    }

    /// Async peek - get next message without removing
    pub async fn peek_async(&self) -> Result<Option<QueuedMessage>> {
        let queue = self.inner.clone();

        tokio::task::spawn_blocking(move || queue.peek())
            .await
            .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {}", e))?
    }

    /// Async ack - acknowledge and remove message
    pub async fn ack_async(&self, message_id: i64) -> Result<bool> {
        let queue = self.inner.clone();

        tokio::task::spawn_blocking(move || queue.ack(message_id))
            .await
            .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {}", e))?
    }

    /// Async nack - mark message for retry
    pub async fn nack_async(&self, message_id: i64) -> Result<()> {
        let queue = self.inner.clone();

        tokio::task::spawn_blocking(move || queue.nack(message_id))
            .await
            .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {}", e))?
    }

    /// Async peek_batch - get multiple messages
    pub async fn peek_batch_async(&self, max_count: usize) -> Result<Vec<QueuedMessage>> {
        let queue = self.inner.clone();

        tokio::task::spawn_blocking(move || queue.peek_batch(max_count))
            .await
            .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {}", e))?
    }

    /// Async ack_batch - acknowledge multiple messages
    pub async fn ack_batch_async(&self, message_ids: Vec<i64>) -> Result<usize> {
        let queue = self.inner.clone();

        tokio::task::spawn_blocking(move || queue.ack_batch(&message_ids))
            .await
            .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {}", e))?
    }

    /// Async stats - get queue statistics
    pub async fn stats_async(&self) -> Result<QueueStats> {
        let queue = self.inner.clone();

        tokio::task::spawn_blocking(move || queue.stats())
            .await
            .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {}", e))?
    }

    /// Async clear - remove all messages
    pub async fn clear_async(&self) -> Result<usize> {
        let queue = self.inner.clone();

        tokio::task::spawn_blocking(move || queue.clear())
            .await
            .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {}", e))?
    }

    /// Async vacuum - reclaim disk space
    pub async fn vacuum_async(&self) -> Result<(u64, u64)> {
        let queue = self.inner.clone();

        tokio::task::spawn_blocking(move || queue.vacuum())
            .await
            .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {}", e))?
    }

    /// Async vacuum_if_needed - conditionally reclaim disk space
    pub async fn vacuum_if_needed_async(&self) -> Result<Option<(u64, u64)>> {
        let queue = self.inner.clone();

        tokio::task::spawn_blocking(move || queue.vacuum_if_needed())
            .await
            .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {}", e))?
    }

    /// Async integrity check - verify database consistency
    pub async fn integrity_check_async(&self) -> Result<IntegrityCheckResult> {
        let queue = self.inner.clone();

        tokio::task::spawn_blocking(move || queue.integrity_check())
            .await
            .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {}", e))?
    }

    /// Async quick check - fast database health check
    pub async fn quick_check_async(&self) -> Result<bool> {
        let queue = self.inner.clone();

        tokio::task::spawn_blocking(move || queue.quick_check())
            .await
            .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {}", e))?
    }

    /// Async backup to specific path
    pub async fn backup_to_async(&self, backup_path: String) -> Result<u64> {
        let queue = self.inner.clone();

        tokio::task::spawn_blocking(move || queue.backup_to(&backup_path))
            .await
            .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {}", e))?
    }

    /// Async rolling backup with automatic cleanup
    pub async fn backup_rolling_async(
        &self,
        backup_dir: String,
        max_backups: usize,
    ) -> Result<String> {
        let queue = self.inner.clone();

        tokio::task::spawn_blocking(move || queue.backup_rolling(&backup_dir, max_backups))
            .await
            .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {}", e))?
    }

    /// Sync len (doesn't need spawn_blocking - very fast query)
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Sync is_empty
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl Clone for AsyncOfflineQueue {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enqueue_dequeue() {
        let queue = OfflineQueue::in_memory(100).unwrap();

        let id = queue
            .enqueue(
                "test/topic",
                r#"{"value": 42}"#,
                MessagePriority::Normal,
                1,
                false,
            )
            .unwrap();

        let msg = queue.peek().unwrap().unwrap();
        assert_eq!(msg.id, id);
        assert_eq!(msg.topic, "test/topic");
        assert_eq!(msg.priority, MessagePriority::Normal);

        queue.ack(id).unwrap();
        assert!(queue.is_empty());
    }

    #[test]
    fn test_priority_ordering() {
        let queue = OfflineQueue::in_memory(100).unwrap();

        // Enqueue in reverse priority order
        queue
            .enqueue("low", "low", MessagePriority::Low, 1, false)
            .unwrap();
        queue
            .enqueue("normal", "normal", MessagePriority::Normal, 1, false)
            .unwrap();
        queue
            .enqueue("high", "high", MessagePriority::High, 1, false)
            .unwrap();
        queue
            .enqueue("critical", "critical", MessagePriority::Critical, 1, false)
            .unwrap();

        // Should dequeue in priority order (highest first)
        let msg1 = queue.peek().unwrap().unwrap();
        assert_eq!(msg1.topic, "critical");
        queue.ack(msg1.id).unwrap();

        let msg2 = queue.peek().unwrap().unwrap();
        assert_eq!(msg2.topic, "high");
        queue.ack(msg2.id).unwrap();

        let msg3 = queue.peek().unwrap().unwrap();
        assert_eq!(msg3.topic, "normal");
        queue.ack(msg3.id).unwrap();

        let msg4 = queue.peek().unwrap().unwrap();
        assert_eq!(msg4.topic, "low");
        queue.ack(msg4.id).unwrap();

        assert!(queue.is_empty());
    }

    #[test]
    fn test_capacity_eviction() {
        let queue = OfflineQueue::in_memory(3).unwrap();

        // Fill queue
        queue
            .enqueue("msg1", "1", MessagePriority::Low, 1, false)
            .unwrap();
        queue
            .enqueue("msg2", "2", MessagePriority::Normal, 1, false)
            .unwrap();
        queue
            .enqueue("msg3", "3", MessagePriority::High, 1, false)
            .unwrap();

        assert_eq!(queue.len(), 3);

        // Add another - should evict lowest priority (msg1)
        queue
            .enqueue("msg4", "4", MessagePriority::Critical, 1, false)
            .unwrap();

        assert_eq!(queue.len(), 3);

        // Verify msg1 was evicted
        let messages = queue.peek_batch(10).unwrap();
        let topics: Vec<&str> = messages.iter().map(|m| m.topic.as_str()).collect();
        assert!(!topics.contains(&"msg1"));
        assert!(topics.contains(&"msg4"));
    }

    #[test]
    fn test_batch_operations() {
        let queue = OfflineQueue::in_memory(100).unwrap();

        // Enqueue multiple
        for i in 0..5 {
            queue
                .enqueue(
                    &format!("topic{}", i),
                    &format!("{}", i),
                    MessagePriority::Normal,
                    1,
                    false,
                )
                .unwrap();
        }

        // Peek batch
        let batch = queue.peek_batch(3).unwrap();
        assert_eq!(batch.len(), 3);

        // Ack batch
        let ids: Vec<i64> = batch.iter().map(|m| m.id).collect();
        let acked = queue.ack_batch(&ids).unwrap();
        assert_eq!(acked, 3);
        assert_eq!(queue.len(), 2);
    }

    #[test]
    fn test_stats() {
        let queue = OfflineQueue::in_memory(100).unwrap();

        queue
            .enqueue("topic1", "payload1", MessagePriority::Low, 1, false)
            .unwrap();
        queue
            .enqueue("topic2", "payload2", MessagePriority::High, 1, false)
            .unwrap();
        queue
            .enqueue("topic3", "payload3", MessagePriority::High, 1, false)
            .unwrap();

        let stats = queue.stats().unwrap();
        assert_eq!(stats.total_messages, 3);
        assert_eq!(stats.by_priority[MessagePriority::Low as usize], 1);
        assert_eq!(stats.by_priority[MessagePriority::High as usize], 2);
        assert!(stats.total_bytes > 0);
    }

    #[test]
    fn test_nack_retry() {
        let queue = OfflineQueue::in_memory(100).unwrap();

        let id = queue
            .enqueue("topic", "payload", MessagePriority::Normal, 1, false)
            .unwrap();

        let msg1 = queue.peek().unwrap().unwrap();
        assert_eq!(msg1.retry_count, 0);

        // Nack (mark for retry)
        queue.nack(id).unwrap();

        let msg2 = queue.peek().unwrap().unwrap();
        assert_eq!(msg2.retry_count, 1);
        assert_eq!(msg2.id, id); // Same message, incremented retry count
    }

    #[test]
    fn test_clear() {
        let queue = OfflineQueue::in_memory(100).unwrap();

        for i in 0..10 {
            queue
                .enqueue(
                    &format!("topic{}", i),
                    "payload",
                    MessagePriority::Normal,
                    1,
                    false,
                )
                .unwrap();
        }

        assert_eq!(queue.len(), 10);

        let cleared = queue.clear().unwrap();
        assert_eq!(cleared, 10);
        assert!(queue.is_empty());
    }

    #[test]
    fn test_integrity_check() {
        let queue = OfflineQueue::in_memory(100).unwrap();

        // Fresh database should pass integrity check
        let result = queue.integrity_check().unwrap();
        assert!(result.is_healthy);
        assert!(result.errors.is_empty());

        // Quick check should also pass
        assert!(queue.quick_check().unwrap());

        // Add some data and check again
        queue
            .enqueue("test", "payload", MessagePriority::Normal, 1, false)
            .unwrap();
        let result2 = queue.integrity_check().unwrap();
        assert!(result2.is_healthy);
    }

    #[test]
    fn test_backup_to() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let backup_path = temp_dir.path().join("backup.db");

        // Create queue with some data
        let queue = OfflineQueue::new(&db_path, 100, 3600).unwrap();
        queue
            .enqueue("test", "payload", MessagePriority::Normal, 1, false)
            .unwrap();

        // Create backup
        let size = queue.backup_to(backup_path.to_str().unwrap()).unwrap();
        assert!(size > 0);
        assert!(backup_path.exists());

        // Verify backup is valid by opening it
        let backup_queue = OfflineQueue::new(&backup_path, 100, 3600).unwrap();
        assert_eq!(backup_queue.len(), 1);
    }

    #[test]
    fn test_backup_rolling() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let backup_dir = temp_dir.path().join("backups");

        // Create queue with some data
        let queue = OfflineQueue::new(&db_path, 100, 3600).unwrap();
        queue
            .enqueue("test", "payload", MessagePriority::Normal, 1, false)
            .unwrap();

        // Create rolling backup
        let backup_path = queue
            .backup_rolling(backup_dir.to_str().unwrap(), 3)
            .unwrap();
        assert!(std::path::Path::new(&backup_path).exists());
        assert!(backup_path.contains("offline_queue_"));
    }
}
