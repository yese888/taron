//! Node-level finality tracking integration.
//!
//! This module wraps taron-core's FinalityTracker for use in the P2P node,
//! providing thread-safe access and network-aware quorum calculation.

use std::sync::Arc;
use tokio::sync::RwLock;
use taron_core::{FinalityTracker, SeenSequences, TransactionStatus, TxAck, Transaction, Wallet};

/// Node finality manager — wraps FinalityTracker for async/thread-safe use.
#[derive(Clone)]
pub struct NodeFinalityManager {
    /// Core finality tracker.
    tracker: Arc<RwLock<FinalityTracker>>,
    /// Seen sequences for double-spend detection.
    seen_sequences: Arc<RwLock<SeenSequences>>,
    /// Node's wallet for signing ACKs.
    node_wallet: Arc<Wallet>,
}

impl NodeFinalityManager {
    /// Create a new finality manager with given quorum and node wallet.
    pub fn new(default_quorum: u32, node_wallet: Wallet) -> Self {
        Self {
            tracker: Arc::new(RwLock::new(FinalityTracker::new(default_quorum))),
            seen_sequences: Arc::new(RwLock::new(SeenSequences::new())),
            node_wallet: Arc::new(node_wallet),
        }
    }

    /// Register a transaction for finality tracking.
    pub async fn register_tx(&self, tx: &Transaction) -> bool {
        let tx_hash = tx.hash();
        let mut tracker = self.tracker.write().await;
        tracker.register_tx(tx_hash, None)
    }

    /// Check for double-spend before accepting a transaction.
    /// Returns Some(original_tx_hash) if this would be a double-spend.
    pub async fn check_double_spend(&self, tx: &Transaction) -> Option<[u8; 32]> {
        let seen = self.seen_sequences.read().await;
        seen.check_double_spend(&tx.sender, tx.sequence)
    }

    /// Record a transaction as seen (for double-spend prevention).
    /// Returns false if this is a double-spend.
    pub async fn record_seen(&self, tx: &Transaction) -> bool {
        let tx_hash = tx.hash();
        let mut seen = self.seen_sequences.write().await;
        seen.record(tx.sender, tx.sequence, tx_hash)
    }

    /// Create an ACK for a transaction using the node's wallet.
    pub fn create_ack(&self, tx_hash: [u8; 32]) -> TxAck {
        TxAck::new(tx_hash, &self.node_wallet)
    }

    /// Record an ACK from a peer.
    pub async fn record_ack(&self, ack: TxAck) -> Option<TransactionStatus> {
        let mut tracker = self.tracker.write().await;
        tracker.record_ack(ack)
    }

    /// Get the status of a transaction.
    pub async fn get_status(&self, tx_hash: &[u8; 32]) -> Option<TransactionStatus> {
        let tracker = self.tracker.read().await;
        tracker.get_status(tx_hash)
    }

    /// Reject a transaction (e.g., due to double-spend).
    pub async fn reject(&self, tx_hash: [u8; 32], reason: String) {
        let mut tracker = self.tracker.write().await;
        tracker.reject(tx_hash, reason);
    }

    /// Update quorum based on current peer count.
    pub async fn update_quorum(&self, peer_count: usize) {
        let quorum = FinalityTracker::calculate_quorum(peer_count);
        let mut tracker = self.tracker.write().await;
        tracker.set_quorum(quorum);
    }

    /// Clean up timed-out pending transactions.
    pub async fn cleanup_timeouts(&self) -> Vec<[u8; 32]> {
        let mut tracker = self.tracker.write().await;
        tracker.cleanup_timeouts()
    }

    /// Get counts for status display.
    pub async fn counts(&self) -> (usize, usize) {
        let tracker = self.tracker.read().await;
        (tracker.pending_count(), tracker.finalized_count())
    }

    /// Get the node's public key (for identifying our own ACKs).
    pub fn node_pubkey(&self) -> [u8; 32] {
        self.node_wallet.public_key()
    }
}

/// Result of validating a transaction for finality.
#[derive(Debug)]
pub enum FinalityValidation {
    /// Transaction is valid, proceed with ACK.
    Valid,
    /// Double-spend detected.
    DoubleSpend { original_tx: [u8; 32] },
    /// Invalid sequence number.
    InvalidSequence { expected: u64, got: u64 },
    /// Other validation error.
    Error(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use taron_core::{Wallet, TxBuilder};

    #[tokio::test]
    async fn test_finality_manager_basic() {
        let node_wallet = Wallet::generate();
        let manager = NodeFinalityManager::new(2, node_wallet);

        let sender = Wallet::generate();
        let recipient = Wallet::generate();
        let tx = TxBuilder::new(&sender)
            .recipient(recipient.public_key())
            .amount(1_000_000)
            .sequence(1)
            .build_and_prove()
            .unwrap();

        // Register
        assert!(manager.register_tx(&tx).await);
        assert!(!manager.register_tx(&tx).await); // duplicate

        // Check status
        let status = manager.get_status(&tx.hash()).await.unwrap();
        assert!(matches!(status, TransactionStatus::Pending { acks: 0, quorum: 2 }));
    }

    #[tokio::test]
    async fn test_double_spend_detection() {
        let node_wallet = Wallet::generate();
        let manager = NodeFinalityManager::new(2, node_wallet);

        let sender = Wallet::generate();
        let recipient1 = Wallet::generate();
        let recipient2 = Wallet::generate();

        // First transaction
        let tx1 = TxBuilder::new(&sender)
            .recipient(recipient1.public_key())
            .amount(1_000_000)
            .sequence(1)
            .build_and_prove()
            .unwrap();

        // Double-spend attempt (same sender, same sequence)
        let tx2 = TxBuilder::new(&sender)
            .recipient(recipient2.public_key())
            .amount(500_000)
            .sequence(1)
            .build_and_prove()
            .unwrap();

        // Record first
        assert!(manager.record_seen(&tx1).await);
        assert!(manager.check_double_spend(&tx1).await.is_some());

        // Second should be detected as double-spend
        assert!(!manager.record_seen(&tx2).await);
        let original = manager.check_double_spend(&tx2).await;
        assert_eq!(original, Some(tx1.hash()));
    }

    #[tokio::test]
    async fn test_ack_creation() {
        let node_wallet = Wallet::generate();
        let expected_pubkey = node_wallet.public_key();
        let manager = NodeFinalityManager::new(2, node_wallet);

        let tx_hash = [42u8; 32];
        let ack = manager.create_ack(tx_hash);

        assert_eq!(ack.tx_hash, tx_hash);
        assert_eq!(ack.peer_pubkey, expected_pubkey);
        assert!(ack.verify().is_ok());
    }
}
