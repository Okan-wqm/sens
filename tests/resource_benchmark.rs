//! Resource Usage Benchmark
//!
//! Measures actual memory and CPU usage of core components.
//! Run with: cargo test --test resource_benchmark --release -- --ignored --nocapture

use std::time::{Duration, Instant};
use sysinfo::{ProcessRefreshKind, RefreshKind, System};

/// Get current process memory usage in KB
fn get_memory_kb() -> u64 {
    let pid = sysinfo::get_current_pid().expect("Failed to get PID");
    let mut sys = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::everything()),
    );
    sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);

    sys.process(pid).map(|p| p.memory() / 1024).unwrap_or(0)
}

/// Measure memory delta for an operation
fn measure_memory<F: FnOnce()>(name: &str, op: F) -> u64 {
    let before = get_memory_kb();
    op();
    let after = get_memory_kb();
    let delta = after.saturating_sub(before);
    println!(
        "  {}: {} KB -> {} KB (delta: {} KB)",
        name, before, after, delta
    );
    delta
}

#[test]
#[ignore]
fn resource_benchmark_baseline() {
    println!("\n========== RESOURCE BENCHMARK ==========\n");

    // 1. Baseline memory
    let baseline = get_memory_kb();
    println!(
        "1. BASELINE MEMORY: {} KB ({:.1} MB)\n",
        baseline,
        baseline as f64 / 1024.0
    );

    // 2. Tokio runtime
    println!("2. TOKIO RUNTIME:");
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let after_tokio = get_memory_kb();
    println!(
        "   After tokio::Runtime: {} KB (delta: {} KB)\n",
        after_tokio,
        after_tokio.saturating_sub(baseline)
    );

    // 3. Channel allocations
    println!("3. BOUNDED CHANNELS:");
    rt.block_on(async {
        measure_memory("mpsc(100)", || {
            let (_tx, _rx) = tokio::sync::mpsc::channel::<Vec<u8>>(100);
        });
        measure_memory("mpsc(500)", || {
            let (_tx, _rx) = tokio::sync::mpsc::channel::<Vec<u8>>(500);
        });
        measure_memory("mpsc(1000)", || {
            let (_tx, _rx) = tokio::sync::mpsc::channel::<Vec<u8>>(1000);
        });
        measure_memory("broadcast(16)", || {
            let (_tx, _rx) = tokio::sync::broadcast::channel::<()>(16);
        });
    });
    println!();

    // 4. HashMap allocations (simulating sensor storage)
    println!("4. SENSOR STORAGE (HashMap<String, f64>):");
    measure_memory("Empty HashMap", || {
        let _map: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    });
    measure_memory("100 sensors", || {
        let mut map: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        for i in 0..100 {
            map.insert(format!("sensor_{}", i), i as f64 * 0.1);
        }
        std::hint::black_box(map);
    });
    measure_memory("200 sensors", || {
        let mut map: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        for i in 0..200 {
            map.insert(format!("sensor_{}", i), i as f64 * 0.1);
        }
        std::hint::black_box(map);
    });
    println!();

    // 5. String interning (lasso)
    println!("5. STRING INTERNING (lasso::ThreadedRodeo):");
    measure_memory("Empty interner", || {
        let _interner = lasso::ThreadedRodeo::default();
    });
    measure_memory("100 interned strings", || {
        let interner = lasso::ThreadedRodeo::default();
        for i in 0..100 {
            interner.get_or_intern(format!("device_{}", i));
        }
        std::hint::black_box(interner);
    });
    println!();

    // 6. SQLite connection
    println!("6. SQLITE CONNECTION:");
    measure_memory("In-memory SQLite", || {
        let _conn = rusqlite::Connection::open_in_memory().unwrap();
    });
    println!();

    // 7. Final summary
    let final_mem = get_memory_kb();
    println!("==========================================");
    println!(
        "FINAL MEMORY: {} KB ({:.1} MB)",
        final_mem,
        final_mem as f64 / 1024.0
    );
    println!("TOTAL GROWTH: {} KB", final_mem.saturating_sub(baseline));
    println!("==========================================\n");
}

#[tokio::test]
#[ignore]
async fn resource_benchmark_async_workload() {
    println!("\n========== ASYNC WORKLOAD BENCHMARK ==========\n");

    let baseline = get_memory_kb();
    println!("Baseline: {} KB\n", baseline);

    // Simulate 100 concurrent tasks
    println!("1. SPAWNING 100 CONCURRENT TASKS:");
    let before = get_memory_kb();

    let handles: Vec<_> = (0..100)
        .map(|i| {
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                i * 2
            })
        })
        .collect();

    let after_spawn = get_memory_kb();
    println!(
        "   After spawn: {} KB (delta: {} KB)",
        after_spawn,
        after_spawn.saturating_sub(before)
    );

    // Wait for completion
    for handle in handles {
        let _ = handle.await;
    }

    let after_complete = get_memory_kb();
    println!(
        "   After complete: {} KB (delta from spawn: {} KB)\n",
        after_complete,
        after_complete.saturating_sub(after_spawn)
    );

    // Simulate message passing
    println!("2. MESSAGE PASSING (10000 messages through channel):");
    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(100);

    let sender = tokio::spawn(async move {
        for i in 0..10000 {
            tx.send(format!("message_{}", i)).await.ok();
        }
        // tx is dropped here, signaling completion
    });

    let receiver = tokio::spawn(async move {
        let mut count = 0;
        while rx.recv().await.is_some() {
            count += 1;
        }
        count
    });

    let start = Instant::now();
    let _ = sender.await;
    let count = receiver.await.unwrap();
    let elapsed = start.elapsed();

    println!("   Processed {} messages in {:?}", count, elapsed);
    println!(
        "   Throughput: {} msg/sec\n",
        (count as f64 / elapsed.as_secs_f64()) as u64
    );

    // Final memory
    let final_mem = get_memory_kb();
    println!("==========================================");
    println!(
        "FINAL MEMORY: {} KB ({:.1} MB)",
        final_mem,
        final_mem as f64 / 1024.0
    );
    println!(
        "GROWTH FROM BASELINE: {} KB",
        final_mem.saturating_sub(baseline)
    );
    println!("==========================================\n");
}

#[tokio::test]
#[ignore]
async fn resource_benchmark_sustained_load() {
    println!("\n========== SUSTAINED LOAD BENCHMARK (30s) ==========\n");

    let baseline = get_memory_kb();
    let start = Instant::now();

    println!("Baseline memory: {} KB", baseline);
    println!("Running sustained workload for 30 seconds...\n");

    let (tx, mut rx) = tokio::sync::mpsc::channel::<u64>(500);

    // Producer: continuous message generation
    let producer = tokio::spawn(async move {
        let mut count = 0u64;
        while start.elapsed() < Duration::from_secs(30) {
            if tx.try_send(count).is_ok() {
                count += 1;
            }
            // Small yield to prevent busy loop
            if count % 1000 == 0 {
                tokio::task::yield_now().await;
            }
        }
        count
    });

    // Consumer: process messages
    let consumer = tokio::spawn(async move {
        let mut processed = 0u64;
        let mut last_report = Instant::now();
        let mut peak_memory = get_memory_kb();

        while let Ok(msg) = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
            if msg.is_none() {
                break;
            }
            processed += 1;

            // Report every 5 seconds
            if last_report.elapsed() >= Duration::from_secs(5) {
                let current_mem = get_memory_kb();
                peak_memory = peak_memory.max(current_mem);
                println!(
                    "  [{}s] Processed: {}, Memory: {} KB",
                    start.elapsed().as_secs(),
                    processed,
                    current_mem
                );
                last_report = Instant::now();
            }
        }

        (processed, peak_memory)
    });

    let sent = producer.await.unwrap();
    let (processed, peak) = consumer.await.unwrap();

    let elapsed = start.elapsed();
    let final_mem = get_memory_kb();

    println!("\n==========================================");
    println!("RESULTS:");
    println!("  Duration:       {:?}", elapsed);
    println!("  Messages sent:  {}", sent);
    println!("  Messages proc:  {}", processed);
    println!(
        "  Throughput:     {} msg/sec",
        (processed as f64 / elapsed.as_secs_f64()) as u64
    );
    println!("  Baseline mem:   {} KB", baseline);
    println!("  Peak memory:    {} KB", peak);
    println!("  Final memory:   {} KB", final_mem);
    println!(
        "  Memory growth:  {} KB",
        final_mem.saturating_sub(baseline)
    );
    println!("==========================================\n");

    // Assert no significant memory leak
    let growth = final_mem.saturating_sub(baseline);
    assert!(
        growth < 10000, // Less than 10 MB growth
        "Possible memory leak: {} KB growth",
        growth
    );
}
