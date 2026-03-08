use criterion::{criterion_group, criterion_main, Criterion, BenchmarkId};
use taron_core::hash::{Sequal256, MINING_STEPS};
use taron_miner::{OptimizedSequal256, BurstConfig, BurstController};
use std::time::Duration;

fn bench_sequal256_baseline(c: &mut Criterion) {
    let input = b"benchmark_input_data_for_sequal256";
    
    c.bench_function("sequal256_baseline_100_steps", |b| {
        b.iter(|| Sequal256::hash(input, 100))
    });
    
    c.bench_function("sequal256_baseline_1000_steps", |b| {
        b.iter(|| Sequal256::hash(input, 1000))
    });
}

fn bench_optimized_sequal256(c: &mut Criterion) {
    let mut hasher = OptimizedSequal256::new().expect("Failed to create optimized hasher");
    let input = b"benchmark_input_data_for_optimized_sequal256";
    
    c.bench_function("sequal256_optimized_100_steps", |b| {
        b.iter(|| hasher.hash_optimized(input, 100))
    });
    
    c.bench_function("sequal256_optimized_1000_steps", |b| {
        b.iter(|| hasher.hash_optimized(input, 1000))
    });
}

fn bench_mining_steps(c: &mut Criterion) {
    let block_header = b"benchmark_block_header_data_12345678";
    let mut hasher = OptimizedSequal256::new().expect("Failed to create optimized hasher");
    
    let nonces = [12345u64, 67890u64, 11111u64, 99999u64];
    
    for nonce in &nonces {
        c.bench_with_input(
            BenchmarkId::new("mining_step", nonce),
            nonce,
            |b, &nonce| {
                b.iter(|| hasher.mine_step_with_steps(block_header, nonce, 1000))
            }
        );
    }
}

fn bench_burst_patterns(c: &mut Criterion) {
    let mut group = c.benchmark_group("burst_patterns");
    group.measurement_time(Duration::from_secs(10));
    
    let configs = vec![
        ("low_power", BurstConfig::low_power()),
        ("balanced", BurstConfig::balanced()),
        ("performance", BurstConfig::performance()),
    ];
    
    for (name, config) in configs {
        group.bench_function(name, |b| {
            let mut controller = BurstController::with_config(config);
            b.iter(|| {
                // Simulate mining work during burst phase
                if controller.cycle_blocking() {
                    // Simulate hash computation
                    let _result = Sequal256::hash(b"burst_test", 100);
                }
            })
        });
    }
    
    group.finish();
}

criterion_group!(
    benches,
    bench_sequal256_baseline,
    bench_optimized_sequal256,
    bench_mining_steps,
    bench_burst_patterns
);
criterion_main!(benches);