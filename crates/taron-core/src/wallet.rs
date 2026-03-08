//! Wallet — Ed25519 keypair generation, address derivation, signing.

use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};

use crate::error::TaronError;

/// A TARON wallet: Ed25519 keypair + derived address.
#[derive(Debug)]
pub struct Wallet {
    signing_key: SigningKey,
}

/// Serializable wallet data (private key bytes).
/// Store at ~/.taron/wallet.key with chmod 600.
#[derive(Serialize, Deserialize)]
pub struct WalletFile {
    pub private_key_hex: String,
}

impl Wallet {
    /// Generate a new random wallet.
    pub fn generate() -> Self {
        let signing_key = SigningKey::generate(&mut OsRng);
        Self { signing_key }
    }

    /// Restore a wallet from 32 raw private key bytes.
    pub fn from_bytes(bytes: &[u8; 32]) -> Result<Self, TaronError> {
        let signing_key = SigningKey::from_bytes(bytes);
        Ok(Self { signing_key })
    }

    /// Restore a wallet from a hex-encoded private key string.
    pub fn from_hex(hex_str: &str) -> Result<Self, TaronError> {
        let bytes = hex::decode(hex_str.trim())
            .map_err(|e| TaronError::KeyError(e.to_string()))?;
        if bytes.len() != 32 {
            return Err(TaronError::KeyError("private key must be 32 bytes".into()));
        }
        let arr: [u8; 32] = bytes.try_into().unwrap();
        Self::from_bytes(&arr)
    }

    /// Public key (32 bytes).
    pub fn public_key(&self) -> [u8; 32] {
        self.signing_key.verifying_key().to_bytes()
    }

    /// Derive a TAR address from the public key.
    /// Format: "tar1" + hex(pubkey) — pubkey is encoded directly for reversibility.
    pub fn address(&self) -> String {
        address_from_pubkey(&self.public_key())
    }

    /// Sign arbitrary bytes. Returns 64-byte Ed25519 signature.
    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.signing_key.sign(message).to_bytes()
    }

    /// Export private key as hex string.
    pub fn private_key_hex(&self) -> String {
        hex::encode(self.signing_key.to_bytes())
    }

    /// Serialize to WalletFile for storage.
    pub fn to_file(&self) -> WalletFile {
        WalletFile {
            private_key_hex: self.private_key_hex(),
        }
    }
}

/// Verify an Ed25519 signature against a public key.
pub fn verify_signature(
    public_key_bytes: &[u8; 32],
    message: &[u8],
    signature_bytes: &[u8; 64],
) -> Result<(), TaronError> {
    use ed25519_dalek::Verifier;

    let vk = VerifyingKey::from_bytes(public_key_bytes)
        .map_err(|e| TaronError::KeyError(e.to_string()))?;
    let sig = ed25519_dalek::Signature::from_bytes(signature_bytes);
    vk.verify(message, &sig)
        .map_err(|_| TaronError::InvalidSignature)
}

/// Derive TAR address from a raw 32-byte public key.
/// Format: "tar1" + hex(pubkey) → 68 chars total, fully reversible.
pub fn address_from_pubkey(pubkey: &[u8; 32]) -> String {
    format!("tar1{}", hex::encode(pubkey))
}

/// Decode a tar1 address back to the 32-byte public key.
/// Returns None if the address is malformed.
pub fn pubkey_from_address(address: &str) -> Option<[u8; 32]> {
    let hex_part = address.strip_prefix("tar1")?;
    if hex_part.len() != 64 { return None; }
    let bytes = hex::decode(hex_part).ok()?;
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Some(arr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wallet_generate() {
        let w = Wallet::generate();
        let addr = w.address();
        assert!(addr.starts_with("tar1"), "address must start with tar1");
        assert_eq!(addr.len(), 4 + 64, "address must be tar1 + 64 hex chars");
    }

    #[test]
    fn test_wallet_roundtrip() {
        let w1 = Wallet::generate();
        let hex = w1.private_key_hex();
        let w2 = Wallet::from_hex(&hex).unwrap();
        assert_eq!(w1.public_key(), w2.public_key());
        assert_eq!(w1.address(), w2.address());
    }

    #[test]
    fn test_sign_and_verify() {
        let wallet = Wallet::generate();
        let message = b"send 10 TAR to tar1abc";
        let sig = wallet.sign(message);
        let pubkey = wallet.public_key();
        assert!(verify_signature(&pubkey, message, &sig).is_ok());
    }

    #[test]
    fn test_invalid_signature() {
        let w1 = Wallet::generate();
        let w2 = Wallet::generate();
        let message = b"test message";
        let sig = w1.sign(message);
        let w2_pubkey = w2.public_key();
        // Signature from w1, verifying with w2's key — must fail
        assert!(verify_signature(&w2_pubkey, message, &sig).is_err());
    }

    #[test]
    fn test_different_wallets_different_addresses() {
        let w1 = Wallet::generate();
        let w2 = Wallet::generate();
        assert_ne!(w1.address(), w2.address());
    }
}
