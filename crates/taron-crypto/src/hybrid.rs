//! Hybrid Signatures — Ed25519 + CRYSTALS-Dilithium3
//!
//! A hybrid signature scheme that produces BOTH an Ed25519 signature AND a
//! Dilithium3 signature over the same message. Verification requires BOTH
//! signatures to be valid.
//!
//! **Why hybrid?**
//! - Ed25519 protects against classical attacks TODAY (proven, fast, small)
//! - Dilithium3 protects against quantum attacks TOMORROW (NIST standard)
//! - If either scheme is broken, the other still provides security
//! - This is the conservative, belt-and-suspenders approach
//!
//! **Signature format:**
//! `[Ed25519 sig (64 bytes)] || [Dilithium3 sig (3293 bytes)]`
//! Total: 3,357 bytes per signature

use ed25519_dalek::{Signer, SigningKey, VerifyingKey, Verifier};
use pqcrypto_dilithium::dilithium3;
use pqcrypto_traits::sign::{
    PublicKey as PqPublicKey,
    SignedMessage,
};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};

use crate::error::CryptoError;

/// Size of Dilithium3 public key in bytes.
pub const DILITHIUM_PK_SIZE: usize = 1952;
/// Size of Dilithium3 signature in bytes.
pub const DILITHIUM_SIG_SIZE: usize = 3293;
/// Size of Ed25519 signature in bytes.
pub const ED25519_SIG_SIZE: usize = 64;
/// Total hybrid signature size.
pub const HYBRID_SIG_SIZE: usize = ED25519_SIG_SIZE + DILITHIUM_SIG_SIZE;

/// A hybrid keypair containing both Ed25519 and Dilithium3 keys.
pub struct HybridKeypair {
    ed_signing: SigningKey,
    dil_public: dilithium3::PublicKey,
    dil_secret: dilithium3::SecretKey,
}

/// Serializable hybrid public key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridPublicKey {
    /// Ed25519 public key (32 bytes, hex-encoded).
    pub ed25519_pk: Vec<u8>,
    /// Dilithium3 public key (1952 bytes, hex-encoded).
    pub dilithium_pk: Vec<u8>,
}

/// A hybrid signature containing both Ed25519 and Dilithium3 signatures.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridSignature {
    /// Ed25519 signature (64 bytes).
    pub ed25519_sig: Vec<u8>,
    /// Dilithium3 detached signature.
    pub dilithium_sig: Vec<u8>,
}

impl HybridKeypair {
    /// Generate a new hybrid keypair.
    pub fn generate() -> Self {
        let ed_signing = SigningKey::generate(&mut OsRng);
        let (dil_public, dil_secret) = dilithium3::keypair();
        Self {
            ed_signing,
            dil_public,
            dil_secret,
        }
    }

    /// Get the Ed25519 public key bytes.
    pub fn ed25519_public_key(&self) -> [u8; 32] {
        self.ed_signing.verifying_key().to_bytes()
    }

    /// Get the Dilithium3 public key bytes.
    pub fn dilithium_public_key(&self) -> Vec<u8> {
        self.dil_public.as_bytes().to_vec()
    }

    /// Get the full hybrid public key.
    pub fn public_key(&self) -> HybridPublicKey {
        HybridPublicKey {
            ed25519_pk: self.ed25519_public_key().to_vec(),
            dilithium_pk: self.dilithium_public_key(),
        }
    }

    /// Sign a message with BOTH Ed25519 and Dilithium3.
    pub fn sign(&self, message: &[u8]) -> HybridSignature {
        // Ed25519 signature
        let ed_sig = self.ed_signing.sign(message);

        // Dilithium3 detached signature
        let dil_signed = dilithium3::sign(message, &self.dil_secret);
        // Extract the detached signature (signed message = sig || message)
        let dil_sig_bytes = &dil_signed.as_bytes()[..dil_signed.as_bytes().len() - message.len()];

        HybridSignature {
            ed25519_sig: ed_sig.to_bytes().to_vec(),
            dilithium_sig: dil_sig_bytes.to_vec(),
        }
    }

    /// Export Ed25519 private key hex (for backward compatibility).
    pub fn ed25519_private_key_hex(&self) -> String {
        hex::encode(self.ed_signing.to_bytes())
    }

    /// Derive a TAR address from the Ed25519 public key (backward compatible).
    pub fn address(&self) -> String {
        use sha3::{Digest, Sha3_256};
        let pubkey = self.ed25519_public_key();
        let mut hasher = Sha3_256::new();
        hasher.update(&pubkey);
        let hash: [u8; 32] = hasher.finalize().into();
        format!("tar1{}", hex::encode(&hash[..16]))
    }
}

impl HybridSignature {
    /// Verify a hybrid signature. BOTH components must be valid.
    pub fn verify(
        &self,
        message: &[u8],
        public_key: &HybridPublicKey,
    ) -> Result<(), CryptoError> {
        // Verify Ed25519
        self.verify_ed25519(message, &public_key.ed25519_pk)?;

        // Verify Dilithium3
        self.verify_dilithium(message, &public_key.dilithium_pk)?;

        Ok(())
    }

    /// Verify only the Ed25519 component (for legacy/fast verification).
    pub fn verify_ed25519(
        &self,
        message: &[u8],
        ed_pk_bytes: &[u8],
    ) -> Result<(), CryptoError> {
        if ed_pk_bytes.len() != 32 {
            return Err(CryptoError::InvalidKey("Ed25519 PK must be 32 bytes".into()));
        }
        if self.ed25519_sig.len() != ED25519_SIG_SIZE {
            return Err(CryptoError::Ed25519VerifyFailed);
        }

        let pk_arr: [u8; 32] = ed_pk_bytes.try_into().unwrap();
        let vk = VerifyingKey::from_bytes(&pk_arr)
            .map_err(|_| CryptoError::InvalidKey("invalid Ed25519 public key".into()))?;

        let sig_arr: [u8; 64] = self.ed25519_sig[..].try_into().unwrap();
        let sig = ed25519_dalek::Signature::from_bytes(&sig_arr);

        vk.verify(message, &sig)
            .map_err(|_| CryptoError::Ed25519VerifyFailed)
    }

    /// Verify only the Dilithium3 component (quantum-resistant verification).
    pub fn verify_dilithium(
        &self,
        message: &[u8],
        dil_pk_bytes: &[u8],
    ) -> Result<(), CryptoError> {
        let dil_pk = dilithium3::PublicKey::from_bytes(dil_pk_bytes)
            .map_err(|_| CryptoError::InvalidKey("invalid Dilithium3 public key".into()))?;

        // Reconstruct signed message (sig || message) for pqcrypto verify
        let mut signed_msg = Vec::with_capacity(self.dilithium_sig.len() + message.len());
        signed_msg.extend_from_slice(&self.dilithium_sig);
        signed_msg.extend_from_slice(message);

        let sm = dilithium3::SignedMessage::from_bytes(&signed_msg)
            .map_err(|_| CryptoError::DilithiumVerifyFailed)?;

        dilithium3::open(&sm, &dil_pk)
            .map_err(|_| CryptoError::DilithiumVerifyFailed)?;

        Ok(())
    }

    /// Total size in bytes.
    pub fn size(&self) -> usize {
        self.ed25519_sig.len() + self.dilithium_sig.len()
    }
}

impl HybridPublicKey {
    /// Total size in bytes.
    pub fn size(&self) -> usize {
        self.ed25519_pk.len() + self.dilithium_pk.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hybrid_keygen() {
        let kp = HybridKeypair::generate();
        let pk = kp.public_key();
        assert_eq!(pk.ed25519_pk.len(), 32);
        assert_eq!(pk.dilithium_pk.len(), DILITHIUM_PK_SIZE);
    }

    #[test]
    fn test_hybrid_sign_verify() {
        let kp = HybridKeypair::generate();
        let pk = kp.public_key();
        let message = b"send 100 TAR to tar1abc";

        let sig = kp.sign(message);
        assert!(sig.verify(message, &pk).is_ok());
    }

    #[test]
    fn test_hybrid_wrong_message() {
        let kp = HybridKeypair::generate();
        let pk = kp.public_key();

        let sig = kp.sign(b"original message");
        assert!(sig.verify(b"tampered message", &pk).is_err());
    }

    #[test]
    fn test_hybrid_wrong_key() {
        let kp1 = HybridKeypair::generate();
        let kp2 = HybridKeypair::generate();
        let message = b"test";

        let sig = kp1.sign(message);
        assert!(sig.verify(message, &kp2.public_key()).is_err());
    }

    #[test]
    fn test_ed25519_component_independently() {
        let kp = HybridKeypair::generate();
        let pk = kp.public_key();
        let message = b"just ed25519";

        let sig = kp.sign(message);
        assert!(sig.verify_ed25519(message, &pk.ed25519_pk).is_ok());
    }

    #[test]
    fn test_dilithium_component_independently() {
        let kp = HybridKeypair::generate();
        let pk = kp.public_key();
        let message = b"just dilithium";

        let sig = kp.sign(message);
        assert!(sig.verify_dilithium(message, &pk.dilithium_pk).is_ok());
    }

    #[test]
    fn test_hybrid_address_backward_compatible() {
        let kp = HybridKeypair::generate();
        let addr = kp.address();
        assert!(addr.starts_with("tar1"));
        assert_eq!(addr.len(), 4 + 32);
    }

    #[test]
    fn test_signature_size() {
        let kp = HybridKeypair::generate();
        let sig = kp.sign(b"size test");
        // Ed25519 (64) + Dilithium3 (~3293)
        assert_eq!(sig.ed25519_sig.len(), 64);
        assert!(sig.dilithium_sig.len() > 3000);
        assert!(sig.dilithium_sig.len() < 4000);
    }

    #[test]
    fn test_deterministic_address() {
        let kp = HybridKeypair::generate();
        assert_eq!(kp.address(), kp.address());
    }
}
