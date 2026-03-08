use thiserror::Error;

#[derive(Debug, Error)]
pub enum TaronError {
    #[error("Invalid signature")]
    InvalidSignature,

    #[error("Invalid PoSC proof")]
    InvalidPosc,

    #[error("Insufficient balance: have {have}, need {need}")]
    InsufficientBalance { have: u64, need: u64 },

    #[error("Invalid sequence number: expected {expected}, got {got}")]
    InvalidSequence { expected: u64, got: u64 },

    #[error("Invalid timestamp: too old or too far in future")]
    InvalidTimestamp,

    #[error("Transaction too large: {size} bytes (max 512)")]
    TransactionTooLarge { size: usize },

    #[error("Unknown sender: {address}")]
    UnknownSender { address: String },

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Key error: {0}")]
    KeyError(String),

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Invalid block: index or prev_hash mismatch, or hash doesn't meet difficulty")]
    InvalidBlock,
}
