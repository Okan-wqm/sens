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

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;
use tracing::{debug, info, warn};

/// Message priority levels (higher value = higher priority)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(u8)]
pub enum MessagePriority {
    /// Low priority - background data, can be delayed
    Low = 0,
    /// Normal priority - regular telemetry
    Normal = 1,
    /// High priority - important events
    High = 2,
    /// Critical priority - alarms, safety events
    Critical = 3,
}

impl Default for MessagePriority {
    fn default() -> Self {
        MessagePriority::Normal
    }
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
    /// Total bytes used
    pub total_bytes: usize,
}

/// Offline message queue with SQLite persistence
///
/// # Thread Safety
/// Uses interior mutability with Mutex for safe concurrent access.
pub struct OfflineQueue {
    /// Database connection (protected by mutex for sync access)
    conn: Mutex<Connection>,
    /// Maximum queue size (message count)
    max_size: usize,
    /// Maximum message age before expiration (seconds)
    max_age_secs: u64,
}

impl OfflineQueue {
    /// Create a new offline queue with file-based persistence
    ///
    /// # Arguments
    /// * `db_path` - Path to SQLite database file
    /// * `max_size` - Maximum number of messages to store
    /// * `max_age_secs` - Maximum age before messages expire (0 = no expiration)
    pub fn new(db_path: &Path, max_size: usize, max_age_secs: u64) -> Result<Self> {
        let conn = Connection::open(db_path)
            .with_context(|| format!("Failed to open queue database: {}", db_path.display()))?;

        let queue = Self {
            conn: Mutex::new(conn),
            max_size,
            max_age_secs,
        };

        queue.init_schema()?;
        info!(
            "Offline queue initialized: max_size={}, max_age_secs={}",
            max_size, max_age_secs
        );

        Ok(queue)
    }

    /// Create an in-memory queue (for testing)
    pub fn in_memory(max_size: usize) -> Result<Self> {
        let conn = Connection::open_in_memory().context("Failed to create in-memory database")?;

        let queue = Self {
            conn: Mutex::new(conn),
            max_size,
            max_age_secs: 0, // No expiration
        };

        queue.init_schema()?;
        Ok(queue)
    }

    /// Initialize database schema
    fn init_schema(&self) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{}", e))?;

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
    /// If queue is at capacity, the oldest low-priority message is removed.
    pub fn enqueue(
        &self,
        topic: &str,
        payload: &str,
        priority: MessagePriority,
        qos: u8,
        retain: bool,
    ) -> Result<i64> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{}", e))?;

        // Check current queue size
        let current_size: usize = conn
            .query_row("SELECT COUNT(*) FROM message_queue", [], |row| row.get(0))
            .unwrap_or(0);

        // If at capacity, remove oldest low-priority message
        if current_size >= self.max_size {
            self.evict_one(&conn)?;
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
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{}", e))?;

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
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{}", e))?;

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
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{}", e))?;

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
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{}", e))?;

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

        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{}", e))?;

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

        let cutoff = chrono::Utc::now().timestamp_millis() - (self.max_age_secs as i64 * 1000);

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
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{}", e))?;

        let total_messages: usize = conn
            .query_row("SELECT COUNT(*) FROM message_queue", [], |row| row.get(0))
            .unwrap_or(0);

        // Count by priority
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

        // Total bytes
        let total_bytes: usize = conn
            .query_row(
                "SELECT COALESCE(SUM(LENGTH(topic) + LENGTH(payload)), 0) FROM message_queue",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        Ok(QueueStats {
            total_messages,
            by_priority,
            oldest_message_age_secs,
            total_bytes,
        })
    }

    /// Clear all messages from queue
    pub fn clear(&self) -> Result<usize> {
        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{}", e))?;

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
            Err(_) => return 0,
        };

        conn.query_row("SELECT COUNT(*) FROM message_queue", [], |row| row.get(0))
            .unwrap_or(0)
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
}
