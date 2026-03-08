//! TachyonGuard — rotating 3-bucket duplicate share detector
//!
//! Maintains three HashSet<u128> buckets in a circular arrangement.
//! Every BUCKET_TTL_MS the oldest bucket is cleared and reused, providing
//! a rolling 3 × BUCKET_TTL_MS (180 seconds) deduplication window.
//!
//! Properties:
//! - O(1) insert and lookup
//! - Bounded memory: max 3 × N × 16 bytes per bucket, regardless of total throughput
//! - Zero per-entry TTL bookkeeping — expiry is implicit via bucket rotation
//! - Key space encodes (block_index, nonce, worker): two rigs on the same
//!   machine with the same nonce produce distinct keys
//!
//! Key derivation:
//!   key = (block_index as u128) << 64 | (nonce XOR fnv1a64(worker))

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

/// How long each bucket covers (milliseconds). 3 buckets → 180s total.
const BUCKET_TTL_MS: u64 = 60_000;

/// Rotating 3-bucket duplicate share detector.
pub struct TachyonGuard {
    buckets: [HashSet<u128>; 3],
    /// Index (0–2) of the bucket currently being written to.
    current: usize,
    /// Timestamp (ms) when `current` was last rotated.
    last_rotate_ms: u64,
}

impl TachyonGuard {
    pub fn new() -> Self {
        Self {
            buckets: [HashSet::new(), HashSet::new(), HashSet::new()],
            current: 0,
            last_rotate_ms: now_ms(),
        }
    }

    /// Check whether `(block_index, nonce, worker)` has already been seen
    /// within the 180-second window.
    ///
    /// Returns `true` if this is a duplicate (reject the share).
    /// Returns `false` and records the key if this is a new share.
    pub fn is_duplicate(&mut self, block_index: u64, nonce: u64, worker: &str) -> bool {
        self.maybe_rotate();

        let key = make_key(block_index, nonce, worker);

        // Check all three buckets.
        for i in 0..3 {
            if self.buckets[i].contains(&key) {
                return true;
            }
        }

        // New share — insert into current bucket.
        self.buckets[self.current].insert(key);
        false
    }

    /// Rotate to the next bucket if BUCKET_TTL_MS has elapsed.
    fn maybe_rotate(&mut self) {
        let now = now_ms();
        if now.saturating_sub(self.last_rotate_ms) >= BUCKET_TTL_MS {
            // Advance and clear the bucket we're about to write into.
            self.current = (self.current + 1) % 3;
            self.buckets[self.current].clear();
            self.last_rotate_ms = now;
        }
    }
}

impl Default for TachyonGuard {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the 128-bit dedup key.
fn make_key(block_index: u64, nonce: u64, worker: &str) -> u128 {
    let worker_hash = fnv1a_64(worker.as_bytes());
    let nonce_mixed = nonce ^ worker_hash;
    ((block_index as u128) << 64) | (nonce_mixed as u128)
}

/// FNV-1a 64-bit hash (fast, non-cryptographic — suitable for key mixing only).
fn fnv1a_64(data: &[u8]) -> u64 {
    const FNV_PRIME: u64 = 0x00000100000001B3;
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    let mut hash = FNV_OFFSET;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
