//! Finality benchmarks — TxAck throughput, quorum confirmation, double-spend detection.
//!
//! Run with: cargo bench -p taron-core --bench finality_bench

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::time::Duration;
use taron_core::{FinalityTracker, SeenSequences, TxAck, TxBuilder, Wallet};

// ── TxAck creation ──────────────────────────────────────────────────────────

fn bench_txack_creation(c: &mut Criterion) {
    let wallet = Wallet::generate();
    let tx_hash = [0xABu8; 32];

    c.bench_function("txack_create_and_sign", |b| {
        b.iter(|| TxAck::new(tx_hash, &wallet))
    });
}

fn bench_txack_verify(c: &mut Criterion) {
    let wallet = Wallet::generate();
    let tx_hash = [0xCDu8; 32];
    let ack = TxAck::new(tx_hash, &wallet);

    c.bench_function("txack_verify_signature", |b| {
        b.iter(|| ack.verify().is_ok())
    });
}

// ── Transaction build + PoSC proof ──────────────────────────────────────────

fn bench_tx_build_and_prove(c: &mut Criterion) {
    let sender = Wallet::generate();
    let recipient = Wallet::generate();

    c.bench_function("tx_build_and_prove", |b| {
        let mut seq = 1u64;
        b.iter(|| {
            let tx = TxBuilder::new(&sender)
                .recipient(recipient.public_key())
                .amount(1_000_000)
                .sequence(seq)
                .build_and_prove()
                .unwrap();
            seq += 1;
            tx
        })
    });
}

// ── Quorum confirmation throughput ──────────────────────────────────────────

fn bench_quorum_confirmation(c: &mut Criterion) {
    let mut group = c.benchmark_group("quorum_confirmation");
    group.measurement_time(Duration::from_secs(5));

    for peer_count in [3usize, 10, 50, 100] {
        let quorum = FinalityTracker::calculate_quorum(peer_count);
        group.throughput(Throughput::Elements(quorum as u64));

        group.bench_with_input(
            BenchmarkId::new("peers", peer_count),
            &(peer_count, quorum),
            |b, &(_, q)| {
                b.iter(|| {
                    let tx_hash = [0x01u8; 32];
                    let mut tracker = FinalityTracker::new(q);
                    tracker.register_tx(tx_hash, None);

                    // Simulate quorum of ACKs from distinct peers
                    for _ in 0..q {
                        let peer = Wallet::generate();
                        let ack = TxAck::new(tx_hash, &peer);
                        tracker.record_ack(ack);
                    }
                })
            },
        );
    }
    group.finish();
}

// ── Finality time distribution ───────────────────────────────────────────────

fn bench_finality_time_3_peers(c: &mut Criterion) {
    // Measures wall-clock finality_ms reported by the tracker (quorum=2 for 3 peers).
    let quorum = FinalityTracker::calculate_quorum(3);

    c.bench_function("finality_time_3_peers", |b| {
        b.iter(|| {
            let tx_hash = [0x02u8; 32];
            let mut tracker = FinalityTracker::new(quorum);
            tracker.register_tx(tx_hash, None);

            let w1 = Wallet::generate();
            let w2 = Wallet::generate();
            tracker.record_ack(TxAck::new(tx_hash, &w1));
            let status = tracker.record_ack(TxAck::new(tx_hash, &w2));
            status.map(|s| s.is_final()).unwrap_or(false)
        })
    });
}

// ── Double-spend detection ───────────────────────────────────────────────────

fn bench_double_spend_detection(c: &mut Criterion) {
    let mut group = c.benchmark_group("double_spend");

    // First: record N unique (sender, seq) pairs, then probe for double-spend
    for tx_count in [100usize, 1_000, 10_000] {
        group.throughput(Throughput::Elements(tx_count as u64));

        group.bench_with_input(
            BenchmarkId::new("seen_sequences_record", tx_count),
            &tx_count,
            |b, &n| {
                b.iter(|| {
                    let mut seen = SeenSequences::new();
                    let sender = [0x10u8; 32];
                    for seq in 1..=(n as u64) {
                        seen.record(sender, seq, [seq as u8; 32]);
                    }
                    seen
                })
            },
        );
    }

    // Probe check_double_spend on a hot cache
    group.bench_function("double_spend_check_existing", |b| {
        let mut seen = SeenSequences::new();
        let sender = [0x20u8; 32];
        // Pre-populate with 1000 entries
        for seq in 1..=1000u64 {
            seen.record(sender, seq, [seq as u8; 32]);
        }
        b.iter(|| seen.check_double_spend(&sender, 500))
    });

    group.bench_function("double_spend_check_new", |b| {
        let mut seen = SeenSequences::new();
        let sender = [0x30u8; 32];
        for seq in 1..=1000u64 {
            seen.record(sender, seq, [seq as u8; 32]);
        }
        b.iter(|| seen.check_double_spend(&sender, 9999)) // sequence not seen
    });

    group.finish();
}

// ── Tracker cleanup (GC) ─────────────────────────────────────────────────────

fn bench_tracker_cleanup(c: &mut Criterion) {
    c.bench_function("tracker_cleanup_1000_pending", |b| {
        b.iter(|| {
            let mut tracker = FinalityTracker::new(10); // High quorum → no auto-confirm
            for i in 0u64..1000 {
                let mut hash = [0u8; 32];
                hash[..8].copy_from_slice(&i.to_le_bytes());
                tracker.register_tx(hash, None);
            }
            // All 1000 will time out (timeout is 30s but cleanup_timeouts uses elapsed)
            // In practice returns vec of hashes — just measure the scan cost
            tracker.cleanup_timeouts()
        })
    });
}

criterion_group!(
    name = finality_benches;
    config = Criterion::default().measurement_time(Duration::from_secs(5));
    targets =
        bench_txack_creation,
        bench_txack_verify,
        bench_tx_build_and_prove,
        bench_quorum_confirmation,
        bench_finality_time_3_peers,
        bench_double_spend_detection,
        bench_tracker_cleanup,
);
criterion_main!(finality_benches);
