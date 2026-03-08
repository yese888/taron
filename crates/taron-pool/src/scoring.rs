//! PSWA — Proportional Share-Weight Algorithm
//!
//! Each accepted share is assigned a weight proportional to its hash proximity
//! to the block target: weight = 2^(leading_zero_bits(hash) - share_difficulty).
//!
//! Payout formula:
//!   miner_payout = (miner_total_weight / round_total_weight) × reward
//!
//! This rewards the actual proof-of-work value of each hash, rather than
//! treating every share as equal regardless of difficulty. Shares that come
//! closer to the block target contribute more weight to the round.
//!
//! Note: the pool currently uses count-based payouts for income stability.
//! This module is available for future use or pool operator configuration.

use std::collections::HashMap;

/// A single weighted share in the current round.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct WeightedShare {
    pub miner_address: String,
    /// 2^(leading_zeros(hash) - share_diff). Always >= 1.0.
    pub weight: f64,
}

/// Accumulated weighted contributions for one round.
#[derive(Default, Clone, Debug)]
#[allow(dead_code)]
pub struct WeightedRound {
    pub shares: Vec<WeightedShare>,
    pub total_weight: f64,
}

#[allow(dead_code)]
impl WeightedRound {
    /// Record a new share.
    ///
    /// `hash` — raw 32-byte block header hash (big-endian).
    /// `share_diff` — the pool share difficulty this share was accepted against.
    pub fn add_share(&mut self, miner_address: String, hash: &[u8; 32], share_diff: u32) {
        let lz = leading_zeros_256(hash);
        // lz >= share_diff is guaranteed by the caller (share was accepted).
        let excess = lz.saturating_sub(share_diff);
        // Cap at 62 bits to stay within f64 integer precision range.
        let weight = (1u64 << excess.min(62)) as f64;
        self.total_weight += weight;
        self.shares.push(WeightedShare { miner_address, weight });
    }

    /// Compute µTAR payout per miner given `total_reward`.
    ///
    /// Any integer rounding dust is given to the top-weight miner to ensure
    /// `sum(payouts) == total_reward` exactly.
    pub fn compute_payouts(&self, total_reward: u64) -> HashMap<String, u64> {
        if self.total_weight == 0.0 || self.shares.is_empty() {
            return HashMap::new();
        }

        // Aggregate weight per miner address.
        let mut weight_by_miner: HashMap<String, f64> = HashMap::new();
        for share in &self.shares {
            *weight_by_miner
                .entry(share.miner_address.clone())
                .or_insert(0.0) += share.weight;
        }

        // Proportional payout (truncating).
        let mut payouts: HashMap<String, u64> = HashMap::new();
        let mut distributed: u64 = 0;
        for (addr, weight) in &weight_by_miner {
            let amount = ((weight / self.total_weight) * total_reward as f64) as u64;
            payouts.insert(addr.clone(), amount);
            distributed += amount;
        }

        // Dust → largest-weight miner.
        let dust = total_reward.saturating_sub(distributed);
        if dust > 0 {
            if let Some((top_addr, _)) = weight_by_miner
                .iter()
                .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            {
                *payouts.entry(top_addr.clone()).or_insert(0) += dust;
            }
        }

        payouts
    }
}

/// Count leading zero bits in a 256-bit big-endian hash.
#[allow(dead_code)]
pub fn leading_zeros_256(hash: &[u8; 32]) -> u32 {
    let mut count = 0u32;
    for &byte in hash {
        let lz = byte.leading_zeros();
        count += lz;
        if lz < 8 {
            break;
        }
    }
    count
}
