//! Bounded Collections Module
//!
//! Provides stack-allocated, bounded collections using heapless crate.
//! These prevent unbounded memory growth and are suitable for embedded
//! environments where heap allocation should be minimized.
//!
//! # IEC 62443 SL2 FR5: Resource Availability
//! Bounded collections prevent memory exhaustion attacks by enforcing
//! strict capacity limits at compile time.
//!
//! # Memory Safety
//! All collections use stack allocation and have known maximum sizes,
//! making them ideal for safety-critical IoT applications.

use heapless::Vec as HVec;

/// Maximum number of register readings per Modbus read operation
pub const MAX_REGISTER_READINGS: usize = 128;

/// Maximum number of errors to track per operation
pub const MAX_ERRORS: usize = 16;

/// Maximum number of GPIO pin states
pub const MAX_GPIO_PINS: usize = 32;

/// Maximum batch size for sensor data
pub const MAX_SENSOR_BATCH: usize = 64;

/// Bounded collection of strings (error messages, etc.)
pub type BoundedStrings<const N: usize> = HVec<String, N>;

/// Bounded collection of f64 values (sensor readings)
pub type BoundedReadings<const N: usize> = HVec<f64, N>;

/// Bounded collection of u16 values (raw register values)
pub type BoundedRegisters<const N: usize> = HVec<u16, N>;

/// Result type for bounded push operations
pub type BoundedPushResult<T> = Result<(), T>;

/// Extension trait for bounded collections
pub trait BoundedExt<T> {
    /// Push an item, logging if the collection is full
    fn push_bounded(&mut self, item: T) -> bool;

    /// Check if the collection is full
    fn is_full(&self) -> bool;

    /// Get remaining capacity
    fn remaining_capacity(&self) -> usize;
}

impl<T, const N: usize> BoundedExt<T> for HVec<T, N> {
    fn push_bounded(&mut self, item: T) -> bool {
        if self.push(item).is_ok() {
            true
        } else {
            tracing::warn!(
                "Bounded collection full (capacity: {}), item dropped",
                N
            );
            false
        }
    }

    fn is_full(&self) -> bool {
        self.len() >= N
    }

    fn remaining_capacity(&self) -> usize {
        N.saturating_sub(self.len())
    }
}

/// A bounded buffer for sensor readings with timestamp
#[derive(Debug, Clone)]
pub struct BoundedSensorBuffer<const N: usize> {
    readings: HVec<SensorReading, N>,
}

/// A single sensor reading with metadata
#[derive(Debug, Clone)]
pub struct SensorReading {
    /// Sensor/register name
    pub name: String,
    /// Value
    pub value: f64,
    /// Optional unit
    pub unit: Option<String>,
    /// Timestamp (Unix millis)
    pub timestamp_ms: u64,
}

impl<const N: usize> BoundedSensorBuffer<N> {
    /// Create a new empty buffer
    pub fn new() -> Self {
        Self {
            readings: HVec::new(),
        }
    }

    /// Add a reading to the buffer
    pub fn push(&mut self, reading: SensorReading) -> bool {
        self.readings.push_bounded(reading)
    }

    /// Get all readings
    pub fn readings(&self) -> &[SensorReading] {
        &self.readings
    }

    /// Clear all readings
    pub fn clear(&mut self) {
        self.readings.clear();
    }

    /// Get number of readings
    pub fn len(&self) -> usize {
        self.readings.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.readings.is_empty()
    }

    /// Get capacity
    pub fn capacity(&self) -> usize {
        N
    }
}

impl<const N: usize> Default for BoundedSensorBuffer<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// A bounded error collector
#[derive(Debug, Clone)]
pub struct BoundedErrors<const N: usize> {
    errors: HVec<String, N>,
}

impl<const N: usize> BoundedErrors<N> {
    /// Create a new empty error collector
    pub fn new() -> Self {
        Self {
            errors: HVec::new(),
        }
    }

    /// Add an error message
    pub fn push(&mut self, error: String) -> bool {
        self.errors.push_bounded(error)
    }

    /// Add an error from a Display type
    pub fn push_error<E: std::fmt::Display>(&mut self, error: E) -> bool {
        self.push(error.to_string())
    }

    /// Get all errors
    pub fn errors(&self) -> &[String] {
        &self.errors
    }

    /// Check if there are any errors
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    /// Get number of errors
    pub fn len(&self) -> usize {
        self.errors.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.errors.is_empty()
    }

    /// Clear all errors
    pub fn clear(&mut self) {
        self.errors.clear();
    }
}

impl<const N: usize> Default for BoundedErrors<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bounded_push() {
        let mut vec: HVec<i32, 3> = HVec::new();

        assert!(vec.push_bounded(1));
        assert!(vec.push_bounded(2));
        assert!(vec.push_bounded(3));
        assert!(!vec.push_bounded(4)); // Should fail, collection full

        assert_eq!(vec.len(), 3);
    }

    #[test]
    fn test_bounded_sensor_buffer() {
        let mut buffer: BoundedSensorBuffer<2> = BoundedSensorBuffer::new();

        assert!(buffer.push(SensorReading {
            name: "temp".to_string(),
            value: 25.5,
            unit: Some("C".to_string()),
            timestamp_ms: 1234567890,
        }));

        assert!(buffer.push(SensorReading {
            name: "ph".to_string(),
            value: 7.2,
            unit: None,
            timestamp_ms: 1234567891,
        }));

        // Buffer is full, this should fail
        assert!(!buffer.push(SensorReading {
            name: "o2".to_string(),
            value: 8.0,
            unit: Some("mg/L".to_string()),
            timestamp_ms: 1234567892,
        }));

        assert_eq!(buffer.len(), 2);
        assert_eq!(buffer.readings()[0].name, "temp");
        assert_eq!(buffer.readings()[1].name, "ph");
    }

    #[test]
    fn test_bounded_errors() {
        let mut errors: BoundedErrors<2> = BoundedErrors::new();

        assert!(!errors.has_errors());
        assert!(errors.push("Error 1".to_string()));
        assert!(errors.has_errors());
        assert!(errors.push_error("Error 2"));
        assert!(!errors.push("Error 3".to_string())); // Full

        assert_eq!(errors.len(), 2);
    }

    #[test]
    fn test_remaining_capacity() {
        let mut vec: HVec<i32, 5> = HVec::new();

        assert_eq!(vec.remaining_capacity(), 5);
        vec.push(1).ok();
        vec.push(2).ok();
        assert_eq!(vec.remaining_capacity(), 3);
    }
}
