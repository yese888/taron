//! TARON Benchmarking — Real performance measurements for PoSC and finality.
//!
//! This module provides accurate, reproducible benchmarks for:
//! - PoSC proof computation time
//! - Transaction validation time
//! - Network finality (when peers available)
//!
//! ## Usage
//! ```bash
//! taron bench              # Run default benchmarks
//! taron bench --count 100  # Run 100 iterations
//! taron bench --save       # Save results to benchmarks/
//! ```

use std::time::Instant;
use std::path::PathBuf;
use serde::{Deserialize, Serialize};
use taron_core::{Wallet, TxBuilder, Sequal256, POSC_STEPS, MINING_STEPS};

/// Benchmark result for a single operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchResult {
    pub operation: String,
    pub iterations: u32,
    pub total_ms: f64,
    pub mean_ms: f64,
    pub min_ms: f64,
    pub max_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub throughput: f64,  // ops/sec
}

/// Full benchmark report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchReport {
    pub timestamp: String,
    pub hostname: String,
    pub cpu_info: String,
    pub rust_version: String,
    pub taron_version: String,
    pub results: Vec<BenchResult>,
}

impl BenchReport {
    pub fn new() -> Self {
        Self {
            timestamp: chrono::Utc::now().to_rfc3339(),
            hostname: hostname::get()
                .map(|h| h.to_string_lossy().to_string())
                .unwrap_or_else(|_| "unknown".into()),
            cpu_info: get_cpu_info(),
            rust_version: env!("CARGO_PKG_RUST_VERSION").to_string(),
            taron_version: env!("CARGO_PKG_VERSION").to_string(),
            results: Vec::new(),
        }
    }

    pub fn add(&mut self, result: BenchResult) {
        self.results.push(result);
    }

    pub fn save(&self, path: &PathBuf) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)
    }
}

/// Run benchmark for a single operation.
pub fn bench_operation<F>(name: &str, iterations: u32, mut op: F) -> BenchResult
where
    F: FnMut() -> (),
{
    let mut times: Vec<f64> = Vec::with_capacity(iterations as usize);

    // Warmup: 3 iterations
    for _ in 0..3 {
        op();
    }

    // Actual measurement
    for _ in 0..iterations {
        let start = Instant::now();
        op();
        let elapsed = start.elapsed().as_secs_f64() * 1000.0; // ms
        times.push(elapsed);
    }

    // Sort for percentiles
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let total_ms: f64 = times.iter().sum();
    let mean_ms = total_ms / iterations as f64;
    let min_ms = times[0];
    let max_ms = times[times.len() - 1];

    let p50_idx = (iterations as f64 * 0.50) as usize;
    let p95_idx = (iterations as f64 * 0.95) as usize;
    let p99_idx = (iterations as f64 * 0.99) as usize;

    BenchResult {
        operation: name.to_string(),
        iterations,
        total_ms,
        mean_ms,
        min_ms,
        max_ms,
        p50_ms: times[p50_idx.min(times.len() - 1)],
        p95_ms: times[p95_idx.min(times.len() - 1)],
        p99_ms: times[p99_idx.min(times.len() - 1)],
        throughput: 1000.0 / mean_ms, // ops/sec
    }
}

/// Benchmark PoSC proof computation.
pub fn bench_posc_proof(iterations: u32) -> BenchResult {
    let wallet = Wallet::generate();
    let recipient = Wallet::generate();

    bench_operation("posc_proof", iterations, || {
        let _tx = TxBuilder::new(&wallet)
            .recipient(recipient.public_key())
            .amount(1_000_000)
            .sequence(1)
            .build_and_prove()
            .unwrap();
    })
}

/// Benchmark SEQUAL-256 hash (full 4MB scratchpad).
pub fn bench_sequal256_full(iterations: u32) -> BenchResult {
    let seed = b"benchmark_seed_for_sequal256_testing";

    bench_operation("sequal256_full", iterations, || {
        let _hash = Sequal256::hash(seed, POSC_STEPS);
    })
}

/// Benchmark SEQUAL-256 hash (256KB scratchpad, mining variant).
pub fn bench_sequal256_fast(iterations: u32) -> BenchResult {
    let seed = b"benchmark_seed_for_sequal256_fast_testing";

    bench_operation("sequal256_fast", iterations, || {
        let _hash = Sequal256::hash_fast(seed, MINING_STEPS);
    })
}

/// Benchmark mining hash rate (hashes per second).
pub fn bench_mining_hashrate(duration_secs: u64) -> BenchResult {
    let header = b"benchmark_block_header_data_12345678";
    let start = Instant::now();
    let mut count: u64 = 0;

    while start.elapsed().as_secs() < duration_secs {
        for nonce in 0..1000u64 {
            let _hash = Sequal256::mine_step_fast(header, nonce + count * 1000, MINING_STEPS);
        }
        count += 1;
    }

    let total_hashes = count * 1000;
    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
    let hashrate = total_hashes as f64 / (elapsed_ms / 1000.0);

    BenchResult {
        operation: "mining_hashrate".to_string(),
        iterations: total_hashes as u32,
        total_ms: elapsed_ms,
        mean_ms: elapsed_ms / total_hashes as f64,
        min_ms: elapsed_ms / total_hashes as f64,
        max_ms: elapsed_ms / total_hashes as f64,
        p50_ms: elapsed_ms / total_hashes as f64,
        p95_ms: elapsed_ms / total_hashes as f64,
        p99_ms: elapsed_ms / total_hashes as f64,
        throughput: hashrate,
    }
}

/// Benchmark transaction validation (PoSC verification).
pub fn bench_tx_validation(iterations: u32) -> BenchResult {
    let wallet = Wallet::generate();
    let recipient = Wallet::generate();

    // Pre-generate transactions
    let txs: Vec<_> = (0..iterations)
        .map(|i| {
            TxBuilder::new(&wallet)
                .recipient(recipient.public_key())
                .amount(1_000_000)
                .sequence(i as u64 + 1)
                .build_and_prove()
                .unwrap()
        })
        .collect();

    let mut idx = 0;
    bench_operation("tx_validation", iterations, || {
        let tx = &txs[idx % txs.len()];
        let _ = tx.verify_signature();
        let _ = taron_core::PoscVerifier::verify(tx);
        idx += 1;
    })
}

/// Run all benchmarks and return a report.
pub fn run_all_benchmarks(iterations: u32) -> BenchReport {
    let mut report = BenchReport::new();

    println!(" ⏱  Running TARON benchmarks...\n");
    println!(" Iterations: {}", iterations);
    println!(" CPU: {}\n", report.cpu_info);

    // PoSC proof computation
    print!(" [1/5] PoSC proof computation... ");
    std::io::Write::flush(&mut std::io::stdout()).ok();
    let result = bench_posc_proof(iterations.min(50)); // PoSC is slow, limit iterations
    println!("✓  mean: {:.2}ms", result.mean_ms);
    report.add(result);

    // SEQUAL-256 full (4MB)
    print!(" [2/5] SEQUAL-256 (4MB)... ");
    std::io::Write::flush(&mut std::io::stdout()).ok();
    let result = bench_sequal256_full(iterations.min(50));
    println!("✓  mean: {:.2}ms", result.mean_ms);
    report.add(result);

    // SEQUAL-256 fast (256KB)
    print!(" [3/5] SEQUAL-256 (256KB)... ");
    std::io::Write::flush(&mut std::io::stdout()).ok();
    let result = bench_sequal256_fast(iterations);
    println!("✓  mean: {:.3}ms", result.mean_ms);
    report.add(result);

    // Mining hashrate
    print!(" [4/5] Mining hashrate (5s)... ");
    std::io::Write::flush(&mut std::io::stdout()).ok();
    let result = bench_mining_hashrate(5);
    let (hr, unit) = if result.throughput >= 1_000_000.0 {
        (result.throughput / 1_000_000.0, "MH/s")
    } else if result.throughput >= 1000.0 {
        (result.throughput / 1000.0, "kH/s")
    } else {
        (result.throughput, "H/s")
    };
    println!("✓  {:.2} {}", hr, unit);
    report.add(result);

    // Transaction validation
    print!(" [5/5] TX validation... ");
    std::io::Write::flush(&mut std::io::stdout()).ok();
    let result = bench_tx_validation(iterations);
    println!("✓  mean: {:.3}ms", result.mean_ms);
    report.add(result);

    println!();
    report
}

/// Print benchmark results in a nice table.
pub fn print_results(report: &BenchReport) {
    println!(" ┌────────────────────────────────────────────────────────────────────┐");
    println!(" │  TARON Benchmark Results                                           │");
    println!(" ├────────────────────────────────────────────────────────────────────┤");
    println!(" │  Date: {}                                       │", &report.timestamp[..19]);
    println!(" │  Host: {:<55}│", &report.hostname[..report.hostname.len().min(55)]);
    println!(" │  CPU:  {:<55}│", &report.cpu_info[..report.cpu_info.len().min(55)]);
    println!(" ├────────────────────────────────────────────────────────────────────┤");
    println!(" │  Operation           │  Mean    │  p50     │  p99     │  Throughput│");
    println!(" ├──────────────────────┼──────────┼──────────┼──────────┼────────────┤");

    for r in &report.results {
        let name = format!("{:<20}", &r.operation[..r.operation.len().min(20)]);
        let mean = format!("{:>6.2}ms", r.mean_ms);
        let p50 = format!("{:>6.2}ms", r.p50_ms);
        let p99 = format!("{:>6.2}ms", r.p99_ms);
        let throughput = if r.throughput >= 1_000_000.0 {
            format!("{:>6.1}M/s", r.throughput / 1_000_000.0)
        } else if r.throughput >= 1000.0 {
            format!("{:>6.1}k/s", r.throughput / 1000.0)
        } else {
            format!("{:>6.1}/s ", r.throughput)
        };
        println!(" │  {} │ {} │ {} │ {} │  {}│", name, mean, p50, p99, throughput);
    }

    println!(" └────────────────────────────────────────────────────────────────────┘");

    // Summary
    if let Some(posc) = report.results.iter().find(|r| r.operation == "posc_proof") {
        println!();
        println!(" 📊 Summary:");
        println!("    • PoSC proof computation: {:.1}ms mean", posc.mean_ms);
        println!("    • Theoretical tx finality: {:.1}ms (local only)", posc.mean_ms);
        if let Some(mining) = report.results.iter().find(|r| r.operation == "mining_hashrate") {
            let (hr, unit) = if mining.throughput >= 1000.0 {
                (mining.throughput / 1000.0, "kH/s")
            } else {
                (mining.throughput, "H/s")
            };
            println!("    • Mining hashrate: {:.2} {}", hr, unit);
        }
        
        // Check if < 100ms claim is valid
        if posc.p99_ms < 100.0 {
            println!("    ✓ PoSC proof < 100ms (p99) — claim VALIDATED");
        } else {
            println!("    ✗ PoSC proof >= 100ms (p99) — claim needs adjustment");
        }
    }
}

/// Get CPU info (best effort).
fn get_cpu_info() -> String {
    #[cfg(target_os = "linux")]
    {
        if let Ok(content) = std::fs::read_to_string("/proc/cpuinfo") {
            for line in content.lines() {
                if line.starts_with("model name") {
                    if let Some(value) = line.split(':').nth(1) {
                        return value.trim().to_string();
                    }
                }
            }
        }
    }
    "Unknown CPU".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bench_operation() {
        let result = bench_operation("test", 10, || {
            std::thread::sleep(Duration::from_micros(100));
        });
        assert_eq!(result.iterations, 10);
        assert!(result.mean_ms > 0.0);
    }

    #[test]
    fn test_bench_report() {
        let report = BenchReport::new();
        assert!(!report.hostname.is_empty());
    }
}
