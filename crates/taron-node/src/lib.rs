//! TARON Node — P2P networking, mempool, gossip protocol, and chain sync.

pub mod protocol;
pub mod mempool;
pub mod peer;
pub mod discovery;
pub mod node;
pub mod seeds;
pub mod sync;
pub mod state_file;
pub mod finality;
pub mod rpc;

pub use node::TaronNode;
pub use mempool::Mempool;
pub use protocol::Message;
pub use seeds::{TESTNET_SEEDS, resolve_seeds};
pub use state_file::NodeStateFile;
pub use finality::NodeFinalityManager;
