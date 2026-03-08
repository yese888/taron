//! Transaction — TARON's fundamental unit of value transfer.
//!
//! Every transaction in TARON carries its own PoSC proof. There are no blocks.
//! A transaction is valid the moment its proof and signature are verified.

use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::TaronError;
use crate::hash::sha3_256;
use crate::wallet::{verify_signature, Wallet};

/// Maximum transaction size in bytes (excluding PoSC proof and signature).
pub const MAX_TX_DATA_BYTES: usize = 256;

/// Timestamp tolerance: ±30 seconds from node's local clock.
pub const TIMESTAMP_TOLERANCE_MS: u64 = 30_000;

/// Protocol version.
pub const TX_VERSION: u8 = 1;

/// A complete TARON transaction, ready for broadcast.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Transaction {
    /// Protocol version.
    pub version: u8,

    /// Sender's Ed25519 public key (32 bytes, hex-encoded for readability).
    pub sender: [u8; 32],

    /// Recipient's Ed25519 public key (32 bytes).
    pub recipient: [u8; 32],

    /// Amount in micro-TAR (1 TAR = 1_000_000 µTAR).
    pub amount: u64,

    /// Fee in micro-TAR. Minimum: 1 µTAR. Fees are burned.
    pub fee: u64,

    /// Sender's sequence number (must be sender's current sequence + 1).
    pub sequence: u64,

    /// Unix timestamp in milliseconds (within ±30s of node clock).
    pub timestamp_ms: u64,

    /// Hash of the sender's previous transaction (or [0u8; 32] for first tx).
    /// Used as seed for PoSC chain to prevent precomputation.
    pub prev_tx_hash: [u8; 32],

    /// Optional memo field (max 256 bytes).
    pub data: Vec<u8>,

    /// PoSC proof: final SEQUAL-256 output over the transaction's unsigned bytes.
    pub posc_proof: [u8; 32],

    /// Number of sequential SEQUAL-256 steps performed (for audit).
    pub posc_steps: u32,

    /// Ed25519 signature over all above fields (including posc_proof).
    #[serde(with = "BigArray")]
    pub signature: [u8; 64],
}

impl Transaction {
    /// Compute the canonical hash of this transaction (used as `prev_tx_hash` in next tx).
    pub fn hash(&self) -> [u8; 32] {
        let bytes = self.to_bytes_for_hash();
        sha3_256(&bytes)
    }

    /// Serialize all fields for hashing (excludes signature to allow signing after PoSC).
    pub fn to_bytes_for_posc(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.push(self.version);
        buf.extend_from_slice(&self.sender);
        buf.extend_from_slice(&self.recipient);
        buf.extend_from_slice(&self.amount.to_le_bytes());
        buf.extend_from_slice(&self.fee.to_le_bytes());
        buf.extend_from_slice(&self.sequence.to_le_bytes());
        buf.extend_from_slice(&self.timestamp_ms.to_le_bytes());
        buf.extend_from_slice(&self.prev_tx_hash);
        buf.push(self.data.len() as u8);
        buf.extend_from_slice(&self.data);
        buf
    }

    /// Serialize all fields for signing (includes posc_proof, excludes signature).
    pub fn to_bytes_for_signing(&self) -> Vec<u8> {
        let mut buf = self.to_bytes_for_posc();
        buf.extend_from_slice(&self.posc_proof);
        buf.extend_from_slice(&self.posc_steps.to_le_bytes());
        buf
    }

    /// Serialize all fields for hashing (full canonical representation).
    pub fn to_bytes_for_hash(&self) -> Vec<u8> {
        let mut buf = self.to_bytes_for_signing();
        buf.extend_from_slice(&self.signature);
        buf
    }

    /// Verify the transaction's signature.
    pub fn verify_signature(&self) -> Result<(), TaronError> {
        let msg = self.to_bytes_for_signing();
        verify_signature(&self.sender, &msg, &self.signature)
    }

    /// Validate structural constraints (size, fee, timestamp tolerance).
    pub fn validate_structure(&self) -> Result<(), TaronError> {
        // Data size check
        if self.data.len() > MAX_TX_DATA_BYTES {
            return Err(TaronError::TransactionTooLarge {
                size: self.data.len(),
            });
        }

        // Minimum fee
        if self.fee == 0 {
            return Err(TaronError::Serialization("fee must be at least 1 µTAR".into()));
        }

        // Timestamp tolerance
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let delta = if self.timestamp_ms > now_ms {
            self.timestamp_ms - now_ms
        } else {
            now_ms - self.timestamp_ms
        };
        if delta > TIMESTAMP_TOLERANCE_MS {
            return Err(TaronError::InvalidTimestamp);
        }

        Ok(())
    }

    /// Hex-encoded transaction hash.
    pub fn hash_hex(&self) -> String {
        hex::encode(self.hash())
    }

    /// Sender's TAR address.
    pub fn sender_address(&self) -> String {
        crate::wallet::address_from_pubkey(&self.sender)
    }

    /// Recipient's TAR address.
    pub fn recipient_address(&self) -> String {
        crate::wallet::address_from_pubkey(&self.recipient)
    }

    /// Total amount deducted from sender: amount + fee.
    pub fn total_cost(&self) -> u64 {
        self.amount.saturating_add(self.fee)
    }
}

/// Builder for constructing and proving a transaction.
///
/// # Example
/// ```rust,no_run
/// use taron_core::{TxBuilder, Wallet};
///
/// let sender = Wallet::generate();
/// let recipient = Wallet::generate();
///
/// let tx = TxBuilder::new(&sender)
///     .recipient(recipient.public_key())
///     .amount(1_000_000)   // 1 TAR
///     .sequence(1)
///     .prev_tx_hash([0u8; 32])
///     .build_and_prove()
///     .unwrap();
/// ```
pub struct TxBuilder<'a> {
    wallet: &'a Wallet,
    recipient: [u8; 32],
    amount: u64,
    fee: u64,
    sequence: u64,
    prev_tx_hash: [u8; 32],
    data: Vec<u8>,
}

impl<'a> TxBuilder<'a> {
    pub fn new(wallet: &'a Wallet) -> Self {
        Self {
            wallet,
            recipient: [0u8; 32],
            amount: 0,
            fee: 1,
            sequence: 1,
            prev_tx_hash: [0u8; 32],
            data: Vec::new(),
        }
    }

    pub fn recipient(mut self, pubkey: [u8; 32]) -> Self {
        self.recipient = pubkey;
        self
    }

    pub fn amount(mut self, utar: u64) -> Self {
        self.amount = utar;
        self
    }

    pub fn fee(mut self, utar: u64) -> Self {
        self.fee = utar.max(1);
        self
    }

    pub fn sequence(mut self, seq: u64) -> Self {
        self.sequence = seq;
        self
    }

    pub fn prev_tx_hash(mut self, hash: [u8; 32]) -> Self {
        self.prev_tx_hash = hash;
        self
    }

    pub fn memo(mut self, data: Vec<u8>) -> Self {
        self.data = data;
        self
    }

    /// Build and prove the transaction (computes PoSC + signs).
    ///
    /// This is the expensive step (~100ms for POSC_STEPS iterations).
    pub fn build_and_prove(self) -> Result<Transaction, TaronError> {
        use crate::hash::{Sequal256, POSC_STEPS};

        if self.data.len() > MAX_TX_DATA_BYTES {
            return Err(TaronError::TransactionTooLarge { size: self.data.len() });
        }

        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        // Partially construct transaction (without proof/signature yet)
        let mut tx = Transaction {
            version: TX_VERSION,
            sender: self.wallet.public_key(),
            recipient: self.recipient,
            amount: self.amount,
            fee: self.fee,
            sequence: self.sequence,
            timestamp_ms,
            prev_tx_hash: self.prev_tx_hash,
            data: self.data,
            posc_proof: [0u8; 32],
            posc_steps: POSC_STEPS,
            signature: [0u8; 64],
        };

        // Compute PoSC proof over unsigned transaction bytes
        let posc_input = tx.to_bytes_for_posc();
        tx.posc_proof = Sequal256::hash(&posc_input, POSC_STEPS);

        // Sign (includes posc_proof)
        let signing_bytes = tx.to_bytes_for_signing();
        tx.signature = self.wallet.sign(&signing_bytes);

        Ok(tx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::Sequal256;

    fn make_tx(steps_override: u32) -> Transaction {
        let sender = Wallet::generate();
        let recipient = Wallet::generate();

        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let mut tx = Transaction {
            version: TX_VERSION,
            sender: sender.public_key(),
            recipient: recipient.public_key(),
            amount: 1_000_000,
            fee: 1,
            sequence: 1,
            timestamp_ms,
            prev_tx_hash: [0u8; 32],
            data: Vec::new(),
            posc_proof: [0u8; 32],
            posc_steps: steps_override,
            signature: [0u8; 64],
        };

        let posc_input = tx.to_bytes_for_posc();
        tx.posc_proof = Sequal256::hash(&posc_input, steps_override);
        let signing_bytes = tx.to_bytes_for_signing();
        tx.signature = sender.sign(&signing_bytes);
        tx
    }

    #[test]
    fn test_transaction_signature_valid() {
        let tx = make_tx(100);
        assert!(tx.verify_signature().is_ok());
    }

    #[test]
    fn test_transaction_hash_deterministic() {
        let tx = make_tx(100);
        assert_eq!(tx.hash(), tx.hash());
    }

    #[test]
    fn test_transaction_tamper_detected() {
        let mut tx = make_tx(100);
        tx.amount += 1; // tamper
        assert!(tx.verify_signature().is_err(), "tampered tx must fail verification");
    }

    #[test]
    fn test_transaction_total_cost() {
        let tx = make_tx(100);
        assert_eq!(tx.total_cost(), tx.amount + tx.fee);
    }

    #[test]
    fn test_address_format() {
        let tx = make_tx(100);
        assert!(tx.sender_address().starts_with("tar1"));
        assert!(tx.recipient_address().starts_with("tar1"));
    }
}
