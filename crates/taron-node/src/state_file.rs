//! Node state file — written by the running node so that `taron status` can
//! display live information without requiring an RPC connection.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Snapshot of node state written to disk by the running node.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NodeStateFile {
    /// Current chain height.
    pub chain_height: u64,
    /// Hex hash of the best block.
    pub best_hash: String,
    /// Total connected peers.
    pub peer_count: usize,
    /// Inbound peer count.
    pub inbound_count: usize,
    /// Outbound peer count.
    pub outbound_count: usize,
    /// Current mempool transaction count.
    pub mempool_size: usize,
    /// Total supply in µTAR.
    pub total_supply: u64,
    /// Node uptime in seconds.
    pub uptime_secs: u64,
    /// Unix timestamp of last write.
    pub updated_at: u64,
}

impl NodeStateFile {
    /// Load state from a JSON file, returning `None` if unavailable.
    pub fn load(path: &Path) -> Option<Self> {
        let data = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&data).ok()
    }

    /// Persist state to a JSON file.
    pub fn save(&self, path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        if let Ok(data) = serde_json::to_string(self) {
            std::fs::write(path, data).ok();
        }
    }
}
