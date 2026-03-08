//! TachyonVarDiff — per-worker adaptive share difficulty
//!
//! Adjusts each worker's share difficulty independently using an exponential
//! moving average (EMA) of observed inter-arrival times between shares.
//!
//! Properties:
//! - Per-worker state: every connected rig has its own difficulty assignment
//! - EWIAT (EMA, α = 0.3): converges smoothly without sudden jumps
//! - Target: one share every TARGET_INTERVAL_MS (8 seconds by default)
//! - Smooth adjustment factor: ratio^0.8 — dampens oscillation
//! - Continuous bit-range [4, 62]: finer granularity than power-of-2 rounding
//! - State persists across rounds — difficulty is a property of the worker,
//!   not of the mining round

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// Target inter-arrival time between shares (milliseconds).
const TARGET_INTERVAL_MS: f64 = 8_000.0;

/// EMA smoothing factor. α=0.3 → converges in ~3 shares.
const ALPHA: f64 = 0.3;

/// Maximum single-step adjustment ratio. Difficulty can change by at most 3×
/// (or 1/3×) per share event.
const MAX_RATIO: f64 = 3.0;

/// Smooth exponent applied to the ratio. 0.8 < 1.0 → gentler than linear.
const SMOOTH_EXP: f64 = 0.8;

/// Per-worker difficulty state.
#[derive(Clone, Debug)]
pub struct WorkerVarDiff {
    /// Current assigned share difficulty for this worker.
    pub difficulty: u32,
    /// Exponential weighted inter-arrival time estimate (ms).
    ewiat_ms: f64,
    /// Timestamp of the last share received from this worker (ms since epoch).
    last_share_ms: u64,
}

impl WorkerVarDiff {
    fn new(initial_difficulty: u32) -> Self {
        Self {
            difficulty: initial_difficulty,
            ewiat_ms: TARGET_INTERVAL_MS,
            last_share_ms: now_ms(),
        }
    }

    /// Record a share arrival, update EWIAT, and recompute difficulty.
    /// `max_difficulty` caps vardiff to never exceed block difficulty.
    /// Returns the new (possibly unchanged) difficulty.
    pub fn on_share(&mut self, max_difficulty: u32) -> u32 {
        let now = now_ms();
        let elapsed = now.saturating_sub(self.last_share_ms) as f64;
        self.last_share_ms = now;

        // EMA update: blend new sample into the running estimate.
        self.ewiat_ms = ALPHA * elapsed + (1.0 - ALPHA) * self.ewiat_ms;

        // Ratio: actual interval vs target. >1 means too slow → lower diff.
        let raw_ratio = TARGET_INTERVAL_MS / self.ewiat_ms.max(1.0);

        // Clamp to prevent runaway adjustments.
        let ratio = raw_ratio.clamp(1.0 / MAX_RATIO, MAX_RATIO);

        // Smooth adjustment factor.
        let factor = ratio.powf(SMOOTH_EXP);

        // Apply and clamp: never go below 4, never exceed block difficulty.
        let ceiling = max_difficulty.min(62);
        let new_diff = ((self.difficulty as f64 * factor).round() as u32).clamp(4, ceiling);
        self.difficulty = new_diff;
        new_diff
    }
}

/// Registry mapping worker keys → per-worker VarDiff state.
pub struct VarDiffRegistry {
    workers: HashMap<String, WorkerVarDiff>,
    /// Current base pool difficulty (updated when chain difficulty changes).
    base_difficulty: u32,
}

impl VarDiffRegistry {
    pub fn new(base_difficulty: u32) -> Self {
        Self {
            workers: HashMap::new(),
            base_difficulty,
        }
    }

    /// Return the current share difficulty for `worker_key`.
    /// Creates a fresh entry at base difficulty if first seen.
    pub fn get_difficulty(&mut self, worker_key: &str) -> u32 {
        let base = self.base_difficulty;
        self.workers
            .entry(worker_key.to_string())
            .or_insert_with(|| WorkerVarDiff::new(base))
            .difficulty
    }

    /// Record a share from `worker_key`, update EWIAT, return new difficulty.
    /// `block_difficulty` is the current chain difficulty — vardiff never exceeds it.
    pub fn on_share(&mut self, worker_key: &str, block_difficulty: u32) -> u32 {
        let base = self.base_difficulty;
        self.workers
            .entry(worker_key.to_string())
            .or_insert_with(|| WorkerVarDiff::new(base))
            .on_share(block_difficulty)
    }

    /// Called when the chain block difficulty changes.
    pub fn set_base_difficulty(&mut self, difficulty: u32) {
        self.base_difficulty = difficulty;
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
