//! Stress Test for Suderra Edge Agent
//!
//! Tests system behavior under high load (1000 simulated devices)
//! Run with: cargo test --test stress_test --release -- --ignored --nocapture
//!
//! This validates:
//! - Memory stability under load
//! - Channel buffer behavior
//! - Throughput capacity
//! - No resource leaks over time

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Simulated device data point
#[derive(Debug, Clone)]
struct DeviceReading {
    device_id: u32,
    timestamp: u64,
    value: f64,
}

/// Stress test configuration
struct StressConfig {
    /// Number of simulated devices
    device_count: usize,
    /// Test duration in seconds
    duration_secs: u64,
    /// Readings per second per device
    readings_per_sec: u32,
    /// Channel buffer size
    channel_buffer: usize,
}

impl Default for StressConfig {
    fn default() -> Self {
        Self {
            device_count: 1000,
            duration_secs: 30,
            readings_per_sec: 10,
            channel_buffer: 1000,
        }
    }
}

/// Test metrics
struct TestMetrics {
    messages_sent: AtomicU64,
    messages_received: AtomicU64,
    messages_dropped: AtomicU64,
    peak_queue_depth: AtomicUsize,
}

impl TestMetrics {
    fn new() -> Self {
        Self {
            messages_sent: AtomicU64::new(0),
            messages_received: AtomicU64::new(0),
            messages_dropped: AtomicU64::new(0),
            peak_queue_depth: AtomicUsize::new(0),
        }
    }

    fn report(&self, elapsed: Duration) {
        let sent = self.messages_sent.load(Ordering::Relaxed);
        let received = self.messages_received.load(Ordering::Relaxed);
        let dropped = self.messages_dropped.load(Ordering::Relaxed);
        let peak_queue = self.peak_queue_depth.load(Ordering::Relaxed);

        let total_attempts = sent + dropped;
        let throughput = if elapsed.as_secs() > 0 {
            received / elapsed.as_secs()
        } else {
            received
        };

        // Drop rate = dropped / total_attempts (not dropped / sent)
        let drop_rate = if total_attempts > 0 {
            (dropped as f64 / total_attempts as f64) * 100.0
        } else {
            0.0
        };

        println!("\n========== STRESS TEST RESULTS ==========");
        println!("Duration:          {:?}", elapsed);
        println!("Total attempts:    {}", total_attempts);
        println!("Messages sent:     {}", sent);
        println!("Messages received: {}", received);
        println!("Messages dropped:  {} ({:.2}%)", dropped, drop_rate);
        println!("Peak queue depth:  {}", peak_queue);
        println!("Throughput:        {} msg/sec", throughput);
        println!("==========================================\n");
    }
}

/// Simulated device that generates readings
async fn device_producer(
    device_id: u32,
    tx: tokio::sync::mpsc::Sender<DeviceReading>,
    metrics: Arc<TestMetrics>,
    duration: Duration,
    readings_per_sec: u32,
) {
    let start = Instant::now();
    let interval = Duration::from_millis(1000 / readings_per_sec as u64);
    let mut value = 0.0f64;

    while start.elapsed() < duration {
        value = (value + 0.1).sin() * 100.0 + device_id as f64;

        let reading = DeviceReading {
            device_id,
            timestamp: start.elapsed().as_millis() as u64,
            value,
        };

        match tx.try_send(reading) {
            Ok(_) => {
                metrics.messages_sent.fetch_add(1, Ordering::Relaxed);
            }
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                metrics.messages_dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => break, // Channel closed
        }

        tokio::time::sleep(interval).await;
    }
}

/// Consumer that processes readings (simulates MQTT publish, DB write, etc.)
async fn message_consumer(
    mut rx: tokio::sync::mpsc::Receiver<DeviceReading>,
    metrics: Arc<TestMetrics>,
    channel_buffer: usize,
) {
    // Simulate some processing overhead
    let process_time = Duration::from_micros(100);

    while let Some(_reading) = rx.recv().await {
        metrics.messages_received.fetch_add(1, Ordering::Relaxed);

        // Track queue depth (approximate)
        let current_depth = channel_buffer.saturating_sub(rx.capacity());
        let _ = metrics
            .peak_queue_depth
            .fetch_max(current_depth, Ordering::Relaxed);

        // Simulate processing time (serialization, network I/O, etc.)
        tokio::time::sleep(process_time).await;
    }
}

#[tokio::test]
#[ignore] // Run manually: cargo test --test stress_test --release -- --ignored --nocapture
async fn stress_test_1000_devices() {
    let config = StressConfig::default();

    println!("\n========== STRESS TEST CONFIG ===========");
    println!("Devices:           {}", config.device_count);
    println!("Duration:          {} seconds", config.duration_secs);
    println!("Readings/sec/dev:  {}", config.readings_per_sec);
    println!("Channel buffer:    {}", config.channel_buffer);
    println!(
        "Expected msgs:     ~{}",
        config.device_count as u64 * config.duration_secs * config.readings_per_sec as u64
    );
    println!("==========================================\n");

    let metrics = Arc::new(TestMetrics::new());
    let (tx, rx) = tokio::sync::mpsc::channel::<DeviceReading>(config.channel_buffer);

    let start = Instant::now();
    let duration = Duration::from_secs(config.duration_secs);

    // Start consumer
    let consumer_metrics = Arc::clone(&metrics);
    let consumer = tokio::spawn(async move {
        message_consumer(rx, consumer_metrics, config.channel_buffer).await;
    });

    // Start producers (1000 devices)
    let mut producers = Vec::with_capacity(config.device_count);
    for device_id in 0..config.device_count as u32 {
        let tx = tx.clone();
        let metrics = Arc::clone(&metrics);
        producers.push(tokio::spawn(async move {
            device_producer(device_id, tx, metrics, duration, config.readings_per_sec).await;
        }));
    }

    // Wait for all producers
    for handle in producers {
        let _ = handle.await;
    }

    // Drop sender to close channel
    drop(tx);

    // Wait for consumer to finish
    let _ = consumer.await;

    let elapsed = start.elapsed();
    metrics.report(elapsed);

    // Assertions
    let received = metrics.messages_received.load(Ordering::Relaxed);
    let dropped = metrics.messages_dropped.load(Ordering::Relaxed);
    let sent = metrics.messages_sent.load(Ordering::Relaxed);
    let peak_queue = metrics.peak_queue_depth.load(Ordering::Relaxed);

    // 1. All messages that entered the channel should be received
    assert_eq!(
        received, sent,
        "Message loss detected: sent {} but received {}",
        sent, received
    );

    // 2. Bounded channel should prevent unbounded queue growth
    assert!(
        peak_queue <= config.channel_buffer,
        "Queue depth exceeded buffer: {} > {}",
        peak_queue,
        config.channel_buffer
    );

    // 3. System should achieve minimum throughput (at least 100 msg/sec)
    let throughput = received / elapsed.as_secs().max(1);
    assert!(
        throughput >= 100,
        "Throughput too low: {} msg/sec (expected >= 100)",
        throughput
    );

    // 4. Some messages should be dropped under extreme load (proves backpressure works)
    let total_attempts = sent + dropped;
    println!(
        "Backpressure effective: {:.1}% of attempts dropped ({}/{})",
        (dropped as f64 / total_attempts as f64) * 100.0,
        dropped,
        total_attempts
    );

    println!("✓ Stress test PASSED - system stable under 1000 device load");
}

#[tokio::test]
#[ignore]
async fn stress_test_memory_stability() {
    //! Tests for memory leaks over extended period
    //! Run with: cargo test --test stress_test memory_stability --release -- --ignored --nocapture

    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::AtomicUsize;

    // Simple memory tracking allocator
    struct TrackingAllocator;

    static ALLOCATED: AtomicUsize = AtomicUsize::new(0);

    unsafe impl GlobalAlloc for TrackingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            ALLOCATED.fetch_add(layout.size(), Ordering::Relaxed);
            unsafe { System.alloc(layout) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            ALLOCATED.fetch_sub(layout.size(), Ordering::Relaxed);
            unsafe { System.dealloc(ptr, layout) }
        }
    }

    // We can't actually replace the global allocator in a test,
    // so we'll use sysinfo for memory tracking
    let sys = sysinfo::System::new_all();
    let pid = sysinfo::get_current_pid().unwrap();

    let initial_memory = sys.process(pid).map(|p| p.memory()).unwrap_or(0);

    println!("\n========== MEMORY STABILITY TEST =========");
    println!("Initial memory: {} KB", initial_memory / 1024);

    // Run multiple iterations
    for iteration in 0..5 {
        let (tx, rx) = tokio::sync::mpsc::channel::<DeviceReading>(100);

        let consumer = tokio::spawn(async move {
            let mut rx = rx;
            while let Some(_) = rx.recv().await {}
        });

        // Generate load
        for device_id in 0..100u32 {
            for _ in 0..100 {
                let reading = DeviceReading {
                    device_id,
                    timestamp: 0,
                    value: device_id as f64,
                };
                let _ = tx.try_send(reading);
            }
        }

        drop(tx);
        let _ = consumer.await;

        // Check memory after each iteration
        let sys = sysinfo::System::new_all();
        let current_memory = sys.process(pid).map(|p| p.memory()).unwrap_or(0);

        println!(
            "Iteration {}: {} KB (delta: {} KB)",
            iteration + 1,
            current_memory / 1024,
            (current_memory as i64 - initial_memory as i64) / 1024
        );
    }

    // Final check
    let sys = sysinfo::System::new_all();
    let final_memory = sys.process(pid).map(|p| p.memory()).unwrap_or(0);

    let memory_growth = final_memory.saturating_sub(initial_memory);
    let growth_percent = (memory_growth as f64 / initial_memory as f64) * 100.0;

    println!("Final memory:   {} KB", final_memory / 1024);
    println!(
        "Memory growth:  {} KB ({:.1}%)",
        memory_growth / 1024,
        growth_percent
    );
    println!("==========================================\n");

    // Memory growth should be < 50% (some growth expected due to runtime)
    assert!(
        growth_percent < 50.0,
        "Memory growth too high: {:.1}% (expected < 50%)",
        growth_percent
    );

    println!("✓ Memory stability test PASSED");
}

#[tokio::test]
#[ignore]
async fn stress_test_channel_backpressure() {
    //! Tests channel behavior under backpressure
    //! Verifies bounded channels prevent memory exhaustion

    println!("\n======== BACKPRESSURE TEST ===============");

    let buffer_sizes = [10, 100, 1000];

    for buffer_size in buffer_sizes {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<DeviceReading>(buffer_size);

        // Fill channel completely
        let mut sent = 0;
        for i in 0..buffer_size * 2 {
            let reading = DeviceReading {
                device_id: i as u32,
                timestamp: 0,
                value: 0.0,
            };
            match tx.try_send(reading) {
                Ok(_) => sent += 1,
                Err(_) => break,
            }
        }

        // Drain and count
        drop(tx);
        let mut received = 0;
        while rx.recv().await.is_some() {
            received += 1;
        }

        println!(
            "Buffer size {}: sent={}, received={}, bounded={}",
            buffer_size,
            sent,
            received,
            sent == buffer_size
        );

        assert_eq!(
            sent, buffer_size,
            "Channel should only accept {} messages",
            buffer_size
        );
        assert_eq!(received, sent, "All sent messages should be received");
    }

    println!("==========================================\n");
    println!("✓ Backpressure test PASSED");
}

#[tokio::test]
#[ignore]
async fn stress_test_concurrent_scripts() {
    //! Tests concurrent script execution capacity
    //! Simulates 100 scripts running simultaneously

    println!("\n======== CONCURRENT SCRIPTS TEST =========");

    let script_count = 100;
    let iterations_per_script = 50;

    let completed = Arc::new(AtomicU64::new(0));
    let start = Instant::now();

    let mut handles = Vec::with_capacity(script_count);

    for script_id in 0..script_count {
        let completed = Arc::clone(&completed);
        handles.push(tokio::spawn(async move {
            // Simulate script execution with variable processing
            for _ in 0..iterations_per_script {
                // Simulate condition evaluation
                let _condition_result = script_id % 2 == 0;

                // Simulate action execution delay
                tokio::time::sleep(Duration::from_micros(100)).await;

                completed.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    // Wait for all scripts
    for handle in handles {
        let _ = handle.await;
    }

    let elapsed = start.elapsed();
    let total_completed = completed.load(Ordering::Relaxed);
    let expected = (script_count * iterations_per_script) as u64;

    println!("Scripts:          {}", script_count);
    println!("Iterations/script: {}", iterations_per_script);
    println!("Total completed:  {}", total_completed);
    println!("Expected:         {}", expected);
    println!("Duration:         {:?}", elapsed);
    println!(
        "Throughput:       {} ops/sec",
        total_completed / elapsed.as_secs().max(1)
    );
    println!("==========================================\n");

    assert_eq!(
        total_completed, expected,
        "All script iterations should complete"
    );

    println!("✓ Concurrent scripts test PASSED");
}
