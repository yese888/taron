use thiserror::Error;

#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("Ed25519 signature verification failed")]
    Ed25519VerifyFailed,

    #[error("Dilithium signature verification failed")]
    DilithiumVerifyFailed,

    #[error("Hybrid signature verification failed: {0}")]
    HybridVerifyFailed(String),

    #[error("Invalid key material: {0}")]
    InvalidKey(String),

    #[error("Noise handshake error: {0}")]
    NoiseHandshake(String),

    #[error("Noise transport error: {0}")]
    NoiseTransport(String),

    #[error("Serialization error: {0}")]
    Serialization(String),
}
