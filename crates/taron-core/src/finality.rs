//! Transaction Finality — Status tracking and ACK-based confirmation.
//!
//! TARON achieves instant finality through a peer quorum mechanism:
//! 1. Sender computes PoSC proof (~2ms)
//! 2. Broadcasts transaction to all known peers
//! 3. Each peer validates and sends back a signed ACK
//! 4. Transaction is FINAL when quorum of ACKs received
//!
//! ## Quorum Definition
//! For Byzantine fault tolerance, we need f+1 ACKs where f = floor(n/3)
//! In practice for small testnets: quorum = min(3, peer_count)
//!
//! ## Double-Spend Prevention
//! Each account has a monotonic sequence_number. Two transactions with
//! the same (sender, sequence) are conflicting — only first-seen wins.

use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Transaction status in the finality pipeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransactionStatus {
    /// Transaction created but not yet broadcast.
    Created,
    /// Transaction broadcast, awaiting peer ACKs.
    Pending {
        /// Number of ACKs received so far.
        acks: u32,
        /// Required ACKs for finality.
        quorum: u32,
    },
    /// Transaction confirmed by quorum — considered FINAL.
    Confirmed {
        /// Total ACKs received.
        acks: u32,
        /// Time from broadcast to finality (milliseconds).
        finality_ms: u64,
    },
    /// Transaction rejected (double-spend, invalid sequence, etc.)
    Rejected {
        /// Reason for rejection.
        reason: String,
    },
}

impl TransactionStatus {
    /// Check if transaction has reached finality.
    pub fn is_final(&self) -> bool {
        matches!(self, TransactionStatus::Confirmed { .. })
    }

    /// Check if transaction was rejected.
    pub fn is_rejected(&self) -> bool {
        matches!(self, TransactionStatus::Rejected { .. })
    }

    /// Get ACK count if pending or confirmed.
    pub fn ack_count(&self) -> u32 {
        match self {
            TransactionStatus::Pending { acks, .. } => *acks,
            TransactionStatus::Confirmed { acks, .. } => *acks,
            _ => 0,
        }
    }
}

/// A signed acknowledgment from a peer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxAck {
    /// Hash of the acknowledged transaction.
    pub tx_hash: [u8; 32],
    /// Public key of the acknowledging peer.
    pub peer_pubkey: [u8; 32],
    /// Ed25519 signature over (tx_hash || peer_pubkey || timestamp).
    #[serde(with = "BigArray")]
    pub signature: [u8; 64],
    /// Timestamp when ACK was created (Unix ms).
    pub timestamp_ms: u64,
}

impl TxAck {
    /// Create a new ACK for a transaction.
    pub fn new(tx_hash: [u8; 32], peer_wallet: &crate::Wallet) -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        
        let peer_pubkey = peer_wallet.public_key();
        
        // Sign: tx_hash || peer_pubkey || timestamp
        let mut msg = Vec::with_capacity(32 + 32 + 8);
        msg.extend_from_slice(&tx_hash);
        msg.extend_from_slice(&peer_pubkey);
        msg.extend_from_slice(&timestamp_ms.to_le_bytes());
        
        let signature = peer_wallet.sign(&msg);
        
        Self {
            tx_hash,
            peer_pubkey,
            signature,
            timestamp_ms,
        }
    }
    
    /// Verify the ACK signature.
    pub fn verify(&self) -> Result<(), crate::TaronError> {
        let mut msg = Vec::with_capacity(32 + 32 + 8);
        msg.extend_from_slice(&self.tx_hash);
        msg.extend_from_slice(&self.peer_pubkey);
        msg.extend_from_slice(&self.timestamp_ms.to_le_bytes());
        
        crate::wallet::verify_signature(&self.peer_pubkey, &msg, &self.signature)
    }
    
    /// Get hex-encoded tx hash.
    pub fn tx_hash_hex(&self) -> String {
        hex::encode(self.tx_hash)
    }
}

/// Pending transaction awaiting finality.
#[derive(Debug)]
struct PendingTx {
    /// When the transaction was first seen/broadcast.
    submitted_at: Instant,
    /// Peer public keys that have ACKed this transaction.
    acks: HashMap<[u8; 32], TxAck>,
    /// Required quorum for finality.
    quorum: u32,
    /// Callback data (optional).
    #[allow(dead_code)]
    callback_data: Option<String>,
}

/// Tracks transaction finality via peer ACKs.
///
/// ## Usage
/// ```rust,ignore
/// let mut tracker = FinalityTracker::new(3); // quorum = 3
/// tracker.register_tx(tx_hash, None);
/// tracker.record_ack(ack);
/// if let Some(status) = tracker.get_status(&tx_hash) {
///     if status.is_final() {
///         println!("Transaction finalized!");
///     }
/// }
/// ```
pub struct FinalityTracker {
    /// Pending transactions awaiting finality.
    pending: HashMap<[u8; 32], PendingTx>,
    /// Finalized transactions (kept for status queries).
    finalized: HashMap<[u8; 32], TransactionStatus>,
    /// Default quorum requirement.
    default_quorum: u32,
    /// Timeout for pending transactions (default: 30 seconds).
    timeout: Duration,
    /// Callback for finality events.
    on_final_callback: Option<Box<dyn Fn([u8; 32], TransactionStatus) + Send + Sync>>,
}

impl std::fmt::Debug for FinalityTracker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FinalityTracker")
            .field("pending_count", &self.pending.len())
            .field("finalized_count", &self.finalized.len())
            .field("default_quorum", &self.default_quorum)
            .field("timeout", &self.timeout)
            .field("has_callback", &self.on_final_callback.is_some())
            .finish()
    }
}

impl FinalityTracker {
    /// Create a new finality tracker with specified default quorum.
    pub fn new(default_quorum: u32) -> Self {
        Self {
            pending: HashMap::new(),
            finalized: HashMap::new(),
            default_quorum: default_quorum.max(1),
            timeout: Duration::from_secs(30),
            on_final_callback: None,
        }
    }
    
    /// Set the timeout for pending transactions.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
    
    /// Set a callback for finality events.
    pub fn on_final<F>(mut self, callback: F) -> Self
    where
        F: Fn([u8; 32], TransactionStatus) + Send + Sync + 'static,
    {
        self.on_final_callback = Some(Box::new(callback));
        self
    }
    
    /// Register a transaction for finality tracking.
    /// Returns false if already registered.
    pub fn register_tx(&mut self, tx_hash: [u8; 32], callback_data: Option<String>) -> bool {
        if self.pending.contains_key(&tx_hash) || self.finalized.contains_key(&tx_hash) {
            return false;
        }
        
        self.pending.insert(tx_hash, PendingTx {
            submitted_at: Instant::now(),
            acks: HashMap::new(),
            quorum: self.default_quorum,
            callback_data,
        });
        true
    }
    
    /// Register with custom quorum.
    pub fn register_tx_with_quorum(&mut self, tx_hash: [u8; 32], quorum: u32, callback_data: Option<String>) -> bool {
        if self.pending.contains_key(&tx_hash) || self.finalized.contains_key(&tx_hash) {
            return false;
        }
        
        self.pending.insert(tx_hash, PendingTx {
            submitted_at: Instant::now(),
            acks: HashMap::new(),
            quorum: quorum.max(1),
            callback_data,
        });
        true
    }
    
    /// Record an ACK for a transaction.
    /// Returns the updated status, or None if tx not tracked.
    pub fn record_ack(&mut self, ack: TxAck) -> Option<TransactionStatus> {
        // Verify ACK signature
        if ack.verify().is_err() {
            return None;
        }
        
        let tx_hash = ack.tx_hash;
        
        // Check if already finalized
        if let Some(status) = self.finalized.get(&tx_hash) {
            return Some(status.clone());
        }
        
        // Get pending entry
        let pending = self.pending.get_mut(&tx_hash)?;
        
        // Deduplicate: one ACK per peer
        if pending.acks.contains_key(&ack.peer_pubkey) {
            let ack_count = pending.acks.len() as u32;
            return Some(TransactionStatus::Pending {
                acks: ack_count,
                quorum: pending.quorum,
            });
        }
        
        pending.acks.insert(ack.peer_pubkey, ack);
        let ack_count = pending.acks.len() as u32;
        let quorum = pending.quorum;
        
        // Check if quorum reached
        if ack_count >= quorum {
            let finality_ms = pending.submitted_at.elapsed().as_millis() as u64;
            let status = TransactionStatus::Confirmed {
                acks: ack_count,
                finality_ms,
            };
            
            // Move to finalized
            self.pending.remove(&tx_hash);
            self.finalized.insert(tx_hash, status.clone());
            
            // Trigger callback
            if let Some(ref cb) = self.on_final_callback {
                cb(tx_hash, status.clone());
            }
            
            Some(status)
        } else {
            Some(TransactionStatus::Pending {
                acks: ack_count,
                quorum,
            })
        }
    }
    
    /// Get the current status of a transaction.
    pub fn get_status(&self, tx_hash: &[u8; 32]) -> Option<TransactionStatus> {
        if let Some(status) = self.finalized.get(tx_hash) {
            return Some(status.clone());
        }
        
        if let Some(pending) = self.pending.get(tx_hash) {
            return Some(TransactionStatus::Pending {
                acks: pending.acks.len() as u32,
                quorum: pending.quorum,
            });
        }
        
        None
    }
    
    /// Mark a transaction as rejected.
    pub fn reject(&mut self, tx_hash: [u8; 32], reason: String) {
        self.pending.remove(&tx_hash);
        self.finalized.insert(tx_hash, TransactionStatus::Rejected { reason });
    }
    
    /// Clean up timed-out pending transactions.
    /// Returns list of timed-out tx hashes.
    pub fn cleanup_timeouts(&mut self) -> Vec<[u8; 32]> {
        let now = Instant::now();
        let timed_out: Vec<[u8; 32]> = self.pending
            .iter()
            .filter(|(_, p)| now.duration_since(p.submitted_at) > self.timeout)
            .map(|(h, _)| *h)
            .collect();
        
        for hash in &timed_out {
            self.pending.remove(hash);
            self.finalized.insert(*hash, TransactionStatus::Rejected {
                reason: "timeout".to_string(),
            });
        }
        
        timed_out
    }
    
    /// Get count of pending transactions.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
    
    /// Get count of finalized transactions.
    pub fn finalized_count(&self) -> usize {
        self.finalized.len()
    }
    
    /// Update default quorum (e.g., when peer count changes).
    pub fn set_quorum(&mut self, quorum: u32) {
        self.default_quorum = quorum.max(1);
    }
    
    /// Calculate appropriate quorum for given peer count.
    /// Uses min(3, peers) for testnet, or f+1 for larger networks.
    pub fn calculate_quorum(peer_count: usize) -> u32 {
        if peer_count <= 3 {
            peer_count.max(1) as u32
        } else {
            // Byzantine: f+1 where f = floor(n/3)
            let f = peer_count / 3;
            (f + 1) as u32
        }
    }
}

/// Seen sequences cache for double-spend prevention.
/// Tracks (sender_pubkey, sequence_number) pairs to detect conflicts.
#[derive(Debug, Default)]
pub struct SeenSequences {
    /// Map from sender pubkey to highest seen sequence.
    seen: HashMap<[u8; 32], u64>,
    /// Pending sequences: (sender, seq) → tx_hash of first-seen tx.
    pending: HashMap<([u8; 32], u64), [u8; 32]>,
}

impl SeenSequences {
    pub fn new() -> Self {
        Self::default()
    }
    
    /// Check if a (sender, sequence) pair would be a double-spend.
    /// Returns Some(original_tx_hash) if this is a duplicate.
    pub fn check_double_spend(&self, sender: &[u8; 32], sequence: u64) -> Option<[u8; 32]> {
        self.pending.get(&(*sender, sequence)).copied()
    }
    
    /// Record a new (sender, sequence) pair.
    /// Returns false if this is a double-spend (already seen).
    pub fn record(&mut self, sender: [u8; 32], sequence: u64, tx_hash: [u8; 32]) -> bool {
        let key = (sender, sequence);
        if self.pending.contains_key(&key) {
            return false;
        }
        self.pending.insert(key, tx_hash);
        
        // Update highest seen
        let highest = self.seen.entry(sender).or_insert(0);
        if sequence > *highest {
            *highest = sequence;
        }
        true
    }
    
    /// Confirm a transaction (remove from pending, it's now in ledger).
    pub fn confirm(&mut self, sender: [u8; 32], sequence: u64) {
        self.pending.remove(&(sender, sequence));
    }
    
    /// Get highest seen sequence for a sender.
    pub fn highest_sequence(&self, sender: &[u8; 32]) -> u64 {
        self.seen.get(sender).copied().unwrap_or(0)
    }
    
    /// Clear old entries (for memory management).
    pub fn clear_below(&mut self, sender: &[u8; 32], min_sequence: u64) {
        self.pending.retain(|(s, seq), _| s != sender || *seq >= min_sequence);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Wallet;
    
    #[test]
    fn test_transaction_status_states() {
        let created = TransactionStatus::Created;
        assert!(!created.is_final());
        assert!(!created.is_rejected());
        
        let pending = TransactionStatus::Pending { acks: 1, quorum: 3 };
        assert!(!pending.is_final());
        assert_eq!(pending.ack_count(), 1);
        
        let confirmed = TransactionStatus::Confirmed { acks: 3, finality_ms: 45 };
        assert!(confirmed.is_final());
        assert_eq!(confirmed.ack_count(), 3);
        
        let rejected = TransactionStatus::Rejected { reason: "double-spend".into() };
        assert!(rejected.is_rejected());
    }
    
    #[test]
    fn test_tx_ack_creation_and_verification() {
        let wallet = Wallet::generate();
        let tx_hash = [42u8; 32];
        
        let ack = TxAck::new(tx_hash, &wallet);
        assert_eq!(ack.tx_hash, tx_hash);
        assert_eq!(ack.peer_pubkey, wallet.public_key());
        assert!(ack.verify().is_ok());
        
        // Tampered ACK should fail
        let mut bad_ack = ack.clone();
        bad_ack.tx_hash[0] ^= 1;
        assert!(bad_ack.verify().is_err());
    }
    
    #[test]
    fn test_finality_tracker_basic() {
        let mut tracker = FinalityTracker::new(2);
        let tx_hash = [1u8; 32];
        
        // Register
        assert!(tracker.register_tx(tx_hash, None));
        assert!(!tracker.register_tx(tx_hash, None)); // duplicate
        
        // Initial status
        let status = tracker.get_status(&tx_hash).unwrap();
        assert!(matches!(status, TransactionStatus::Pending { acks: 0, quorum: 2 }));
        
        // First ACK
        let wallet1 = Wallet::generate();
        let ack1 = TxAck::new(tx_hash, &wallet1);
        let status = tracker.record_ack(ack1).unwrap();
        assert!(matches!(status, TransactionStatus::Pending { acks: 1, quorum: 2 }));
        
        // Second ACK (quorum reached)
        let wallet2 = Wallet::generate();
        let ack2 = TxAck::new(tx_hash, &wallet2);
        let status = tracker.record_ack(ack2).unwrap();
        assert!(status.is_final());
        if let TransactionStatus::Confirmed { acks, finality_ms } = status {
            assert_eq!(acks, 2);
            assert!(finality_ms < 1000); // Should be very fast in test
        }
    }
    
    #[test]
    fn test_finality_tracker_dedup_acks() {
        let mut tracker = FinalityTracker::new(3);
        let tx_hash = [2u8; 32];
        tracker.register_tx(tx_hash, None);
        
        let wallet = Wallet::generate();
        let ack1 = TxAck::new(tx_hash, &wallet);
        let ack2 = TxAck::new(tx_hash, &wallet); // Same peer
        
        tracker.record_ack(ack1);
        let status = tracker.record_ack(ack2).unwrap();
        
        // Should still be 1 ACK (deduplicated)
        assert!(matches!(status, TransactionStatus::Pending { acks: 1, .. }));
    }
    
    #[test]
    fn test_finality_tracker_rejection() {
        let mut tracker = FinalityTracker::new(2);
        let tx_hash = [3u8; 32];
        tracker.register_tx(tx_hash, None);
        
        tracker.reject(tx_hash, "double-spend detected".into());
        
        let status = tracker.get_status(&tx_hash).unwrap();
        assert!(status.is_rejected());
    }
    
    #[test]
    fn test_seen_sequences_double_spend() {
        let mut seen = SeenSequences::new();
        let sender = [10u8; 32];
        let tx1 = [1u8; 32];
        let tx2 = [2u8; 32];
        
        // First transaction
        assert!(seen.record(sender, 1, tx1));
        assert!(seen.check_double_spend(&sender, 1).is_some());
        
        // Second with same sequence = double spend
        assert!(!seen.record(sender, 1, tx2));
        assert_eq!(seen.check_double_spend(&sender, 1), Some(tx1));
        
        // Different sequence = OK
        assert!(seen.record(sender, 2, tx2));
        assert_eq!(seen.highest_sequence(&sender), 2);
    }
    
    #[test]
    fn test_quorum_calculation() {
        assert_eq!(FinalityTracker::calculate_quorum(1), 1);
        assert_eq!(FinalityTracker::calculate_quorum(2), 2);
        assert_eq!(FinalityTracker::calculate_quorum(3), 3);
        assert_eq!(FinalityTracker::calculate_quorum(4), 2); // f=1, f+1=2
        assert_eq!(FinalityTracker::calculate_quorum(10), 4); // f=3, f+1=4
        assert_eq!(FinalityTracker::calculate_quorum(100), 34); // f=33, f+1=34
    }
}
