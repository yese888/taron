//! PoSC — Proof of Sequential Chain verification.
//!
//! A PoSC proof is the output of SEQUAL-256 computed over a transaction's
//! canonical bytes. Verification recomputes the same hash and compares.
//! Verification is fast (~0.1ms) since it only needs to confirm the output
//! matches — full recomputation is required but cached per-transaction.

use crate::error::TaronError;
use crate::hash::{Sequal256, POSC_STEPS};
use crate::transaction::Transaction;

/// A PoSC proof (32-byte SEQUAL-256 output).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoscProof(pub [u8; 32]);

impl PoscProof {
    /// Compute a PoSC proof for raw transaction bytes.
    pub fn compute(tx_bytes: &[u8]) -> Self {
        Self(Sequal256::hash(tx_bytes, POSC_STEPS))
    }

    /// Compute a PoSC proof with custom step count (for testing or difficulty adjustment).
    pub fn compute_with_steps(tx_bytes: &[u8], steps: u32) -> Self {
        Self(Sequal256::hash(tx_bytes, steps))
    }

    /// Get the raw 32-byte proof.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Hex-encoded proof string.
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
}

/// PoSC proof verifier for incoming transactions.
pub struct PoscVerifier;

impl PoscVerifier {
    /// Verify the PoSC proof embedded in a transaction.
    ///
    /// This recomputes SEQUAL-256 over the transaction's `to_bytes_for_posc()`
    /// and checks it matches `tx.posc_proof`.
    ///
    /// **Cost**: O(steps) — same as generating the proof (~100ms for POSC_STEPS).
    /// In production, verification results are cached by transaction hash.
    pub fn verify(tx: &Transaction) -> Result<(), TaronError> {
        let input = tx.to_bytes_for_posc();
        let expected = Sequal256::hash(&input, tx.posc_steps);

        if expected != tx.posc_proof {
            return Err(TaronError::InvalidPosc);
        }

        Ok(())
    }

    /// Fast structural check: verify the proof has non-zero content.
    /// This is a pre-filter before expensive full verification.
    pub fn is_non_trivial(tx: &Transaction) -> bool {
        tx.posc_proof != [0u8; 32] && tx.posc_steps >= 100
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wallet::Wallet;
    use crate::hash::Sequal256;
    use crate::transaction::{Transaction, TX_VERSION};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn make_proven_tx(steps: u32) -> Transaction {
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
            amount: 500_000,
            fee: 1,
            sequence: 1,
            timestamp_ms,
            prev_tx_hash: [0u8; 32],
            data: Vec::new(),
            posc_proof: [0u8; 32],
            posc_steps: steps,
            signature: [0u8; 64],
        };

        let input = tx.to_bytes_for_posc();
        tx.posc_proof = Sequal256::hash(&input, steps);
        let signing_bytes = tx.to_bytes_for_signing();
        tx.signature = sender.sign(&signing_bytes);
        tx
    }

    #[test]
    fn test_posc_verify_valid() {
        let tx = make_proven_tx(500);
        assert!(PoscVerifier::verify(&tx).is_ok());
    }

    #[test]
    fn test_posc_verify_tampered_amount() {
        let mut tx = make_proven_tx(500);
        tx.amount += 1; // tamper after proof
        assert!(PoscVerifier::verify(&tx).is_err());
    }

    #[test]
    fn test_posc_verify_tampered_proof() {
        let mut tx = make_proven_tx(500);
        tx.posc_proof[0] ^= 0xFF; // corrupt the proof
        assert!(PoscVerifier::verify(&tx).is_err());
    }

    #[test]
    fn test_posc_non_trivial() {
        let tx = make_proven_tx(500);
        assert!(PoscVerifier::is_non_trivial(&tx));

        let mut zero_tx = tx.clone();
        zero_tx.posc_proof = [0u8; 32];
        assert!(!PoscVerifier::is_non_trivial(&zero_tx));
    }

    #[test]
    fn test_posc_proof_compute() {
        let data = b"test transaction bytes";
        let p1 = PoscProof::compute_with_steps(data, 100);
        let p2 = PoscProof::compute_with_steps(data, 100);
        assert_eq!(p1, p2, "PoSC proof must be deterministic");
        assert_ne!(p1.as_bytes(), &[0u8; 32]);
    }
}
