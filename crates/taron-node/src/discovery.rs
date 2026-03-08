//! UDP Local Peer Discovery — find TARON peers on the LAN.
//!
//! Broadcasts `TARON_DISCOVERY` on port 8334 and listens for responses
//! containing the TCP address of other nodes.
#![allow(dead_code)]

use std::net::SocketAddr;
use tokio::net::UdpSocket;
use tracing::{debug, warn};

/// UDP discovery port.
pub const DISCOVERY_PORT: u16 = 8334;

/// Magic bytes for discovery messages.
const DISCOVERY_REQUEST: &[u8] = b"TARON_DISCOVERY";
const DISCOVERY_RESPONSE_PREFIX: &[u8] = b"TARON_NODE:";

/// Run the UDP discovery listener. Responds to discovery requests with our TCP address.
pub async fn run_discovery_listener(tcp_port: u16) -> std::io::Result<()> {
    let socket = UdpSocket::bind(format!("0.0.0.0:{}", DISCOVERY_PORT)).await?;
    socket.set_broadcast(true)?;
    debug!("UDP discovery listener on port {}", DISCOVERY_PORT);

    let mut buf = [0u8; 256];
    loop {
        let (len, src) = socket.recv_from(&mut buf).await?;
        if len == DISCOVERY_REQUEST.len() && &buf[..len] == DISCOVERY_REQUEST {
            let response = format!("TARON_NODE:{}", tcp_port);
            if let Err(e) = socket.send_to(response.as_bytes(), src).await {
                warn!("Failed to respond to discovery from {}: {}", src, e);
            } else {
                debug!("Responded to discovery from {}", src);
            }
        }
    }
}

/// Send a discovery broadcast and collect responding peers.
/// Returns a list of TCP addresses that responded within the timeout.
pub async fn discover_peers(timeout_ms: u64) -> Vec<SocketAddr> {
    let mut found = Vec::new();

    let socket = match UdpSocket::bind("0.0.0.0:0").await {
        Ok(s) => s,
        Err(e) => {
            warn!("Failed to bind discovery socket: {}", e);
            return found;
        }
    };
    if socket.set_broadcast(true).is_err() {
        return found;
    }

    let broadcast_addr = format!("255.255.255.255:{}", DISCOVERY_PORT);
    if socket.send_to(DISCOVERY_REQUEST, &broadcast_addr).await.is_err() {
        return found;
    }

    let mut buf = [0u8; 256];
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_millis(timeout_ms);

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }

        match tokio::time::timeout(remaining, socket.recv_from(&mut buf)).await {
            Ok(Ok((len, src))) => {
                if let Some(port_str) = std::str::from_utf8(&buf[..len])
                    .ok()
                    .and_then(|s| s.strip_prefix("TARON_NODE:"))
                {
                    if let Ok(port) = port_str.parse::<u16>() {
                        let peer_addr = SocketAddr::new(src.ip(), port);
                        if !found.contains(&peer_addr) {
                            debug!("Discovered peer: {}", peer_addr);
                            found.push(peer_addr);
                        }
                    }
                }
            }
            _ => break,
        }
    }

    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_discovery_request_response() {
        // Start a discovery listener on a random port (we override DISCOVERY_PORT behavior)
        let listener_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let listener_addr = listener_socket.local_addr().unwrap();
        listener_socket.set_broadcast(true).unwrap();

        let tcp_port: u16 = 8333;

        // Simulate listener in background
        let handle = tokio::spawn(async move {
            let mut buf = [0u8; 256];
            let (len, src) = listener_socket.recv_from(&mut buf).await.unwrap();
            assert_eq!(&buf[..len], DISCOVERY_REQUEST);
            let response = format!("TARON_NODE:{}", tcp_port);
            listener_socket.send_to(response.as_bytes(), src).await.unwrap();
        });

        // Client sends discovery request directly to the listener
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        client.send_to(DISCOVERY_REQUEST, listener_addr).await.unwrap();

        let mut buf = [0u8; 256];
        let (len, _) = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            client.recv_from(&mut buf),
        ).await.unwrap().unwrap();

        let response = std::str::from_utf8(&buf[..len]).unwrap();
        assert!(response.starts_with("TARON_NODE:"));
        let port: u16 = response.strip_prefix("TARON_NODE:").unwrap().parse().unwrap();
        assert_eq!(port, 8333);

        handle.await.unwrap();
    }
}
