//! Hardcoded testnet seed nodes for TARON.
//!
//! Seed nodes help new peers bootstrap into the testnet when they have
//! no prior peer list.

use std::net::{SocketAddr, ToSocketAddrs};

/// Known TARON testnet seed nodes (hostname:port or ip:port).
pub const TESTNET_SEEDS: &[&str] = &[
    "seed.taron.network:8333",
];

/// Resolve the effective seed-node list.
///
/// Rules (in priority order):
/// 1. If `config_seeds` is non-empty, use those.
/// 2. Else resolve TESTNET_SEEDS via DNS (supports both hostnames and IPs).
/// 3. Otherwise, return an empty vec (LAN discovery is the fallback).
pub fn resolve_seeds(config_seeds: &[SocketAddr]) -> Vec<SocketAddr> {
    if !config_seeds.is_empty() {
        return config_seeds.to_vec();
    }
    TESTNET_SEEDS
        .iter()
        .flat_map(|s| s.to_socket_addrs().unwrap_or_default())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_seeds_empty() {
        // No config seeds, no hardcoded seeds → empty
        let result = resolve_seeds(&[]);
        // TESTNET_SEEDS is currently empty, so this should be empty too
        assert_eq!(result.len(), TESTNET_SEEDS.len());
    }

    #[test]
    fn test_resolve_seeds_config_takes_priority() {
        let addr: SocketAddr = "127.0.0.1:8333".parse().unwrap();
        let result = resolve_seeds(&[addr]);
        assert_eq!(result, vec![addr]);
    }
}
