//! taron-crypto — Post-quantum cryptographic primitives for TARON.
//!
//! Provides:
//! - **Hybrid signatures**: Ed25519 + CRYSTALS-Dilithium3 (NIST PQC standard)
//! - **Noise Protocol transport**: Encrypted, authenticated P2P channels
//! - **Stealth address** primitives (planned)

pub mod hybrid;
pub mod noise_transport;
pub mod error;

pub use hybrid::{HybridKeypair, HybridSignature};
pub use noise_transport::NoiseTransport;
pub use error::CryptoError;
