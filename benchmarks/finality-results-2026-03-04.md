# TARON Finality Benchmarks — March 4, 2026

> Run: `cargo bench -p taron-core --bench finality_bench`
> Platform: Linux 6.12 x86_64

## Results

### TxAck

| Benchmark | Median time | Notes |
|-----------|------------|-------|
| `txack_create_and_sign` | **28.4 µs** | Ed25519 sign over tx_hash + pubkey + timestamp |
| `txack_verify_signature` | **67.3 µs** | Ed25519 verify |

> Theoretical throughput: ~35,000 ACK/s (creation) / ~15,000 ACK/s (verification)

### Transaction PoSC

| Benchmark | Median time |
|-----------|------------|
| `tx_build_and_prove` | **101.7 ms** |

> ~10 tx/s per thread (CPU-bound by design — PoSC proof)

### Confirmation Quorum

| Peers | Quorum | Median time | Throughput |
|-------|--------|------------|-----------|
| 3     | 3      | 441 µs     | 6.8 Kelem/s |
| 10    | 4      | 565 µs     | 7.1 Kelem/s |
| 50    | 17     | 2.50 ms    | 6.8 Kelem/s |
| 100   | 34     | 4.93 ms    | 6.9 Kelem/s |

> Finality time (3 peers, 2 ACKs required): **288 µs** wall-clock

### Double-Spend Detection

| Benchmark | Median time | Throughput |
|-----------|------------|-----------|
| `seen_sequences_record/100` | 19.6 µs | 5.09 Melem/s |
| `seen_sequences_record/1000` | 222 µs | 4.50 Melem/s |
| `seen_sequences_record/10000` | 2.06 ms | 4.85 Melem/s |
| `double_spend_check_existing` | ~61 ns | ~82 Melem/s (HashMap O(1)) |

> Double-spend detection: near-instant (hashmap lookup)

## Analysis

- **Finality < 1ms** locally (testnet with 3 nodes) — consistent with the "instant finality" goal
- **PoSC at 100ms** is by design (lightweight proof of computation, not ASIC-friendly)
- **Double-spend check** = O(1) hashmap, no degradation at 10,000 seen txs
- **Quorum with 100 peers** = ~5ms — acceptable for a global distributed network

## Phase 3 Recommendations

1. Parallelize ACK verification (currently sequential in `record_ack`)
2. Add LRU cache for `SeenSequences` in production (prevent OOM on long mempool)
3. Benchmark on live network once seed node is active
