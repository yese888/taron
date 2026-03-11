//! Noise Protocol Transport — Encrypted P2P Communication
//!
//! Uses the Noise_XX_25519_ChaChaPoly_BLAKE2s pattern:
//! - **XX**: Mutual authentication (both sides prove identity)
//! - **25519**: X25519 Diffie-Hellman key exchange
//! - **ChaChaPoly**: ChaCha20-Poly1305 AEAD encryption
//! - **BLAKE2s**: Fast hash function for key derivation
//!
//! This is the same pattern used by WireGuard and similar to Signal Protocol.
//!
//! ## Handshake Flow
//! 1. Initiator → Responder: ephemeral key
//! 2. Responder → Initiator: ephemeral key + static key (encrypted)
//! 3. Initiator → Responder: static key (encrypted)
//! After 3 messages, both sides have an encrypted transport channel.

use snow::{Builder, HandshakeState, TransportState};
use crate::error::CryptoError;

/// Noise protocol pattern used by TARON.
const NOISE_PATTERN: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";

/// Maximum Noise message size (65535 bytes per spec).
const MAX_NOISE_MSG_SIZE: usize = 65535;

/// Represents a Noise Protocol transport channel.
///
/// After the 3-message XX handshake completes, this provides
/// authenticated encryption for all subsequent messages.
pub struct NoiseTransport {
    state: TransportState,
    /// The remote peer's static public key (learned during handshake).
    remote_static_key: Option<Vec<u8>>,
}

/// Builder for establishing a Noise handshake.
pub struct NoiseHandshake {
    state: HandshakeState,
    is_initiator: bool,
}

/// A static Noise keypair for node identity.
pub struct NoiseIdentity {
    pub private_key: Vec<u8>,
    pub public_key: Vec<u8>,
}

impl NoiseIdentity {
    /// Generate a new X25519 keypair for Noise protocol.
    pub fn generate() -> Result<Self, CryptoError> {
        let builder = Builder::new(NOISE_PATTERN.parse().unwrap());
        let keypair = builder.generate_keypair()
            .map_err(|e| CryptoError::NoiseHandshake(format!("keygen failed: {}", e)))?;
        Ok(Self {
            private_key: keypair.private.to_vec(),
            public_key: keypair.public.to_vec(),
        })
    }

    /// Public key hex for display/storage.
    pub fn public_key_hex(&self) -> String {
        hex::encode(&self.public_key)
    }
}

impl NoiseHandshake {
    /// Create a new handshake as the initiator (connecting party).
    pub fn initiator(identity: &NoiseIdentity) -> Result<Self, CryptoError> {
        let state = Builder::new(NOISE_PATTERN.parse().unwrap())
            .local_private_key(&identity.private_key)
            .build_initiator()
            .map_err(|e| CryptoError::NoiseHandshake(format!("init failed: {}", e)))?;

        Ok(Self {
            state,
            is_initiator: true,
        })
    }

    /// Create a new handshake as the responder (accepting party).
    pub fn responder(identity: &NoiseIdentity) -> Result<Self, CryptoError> {
        let state = Builder::new(NOISE_PATTERN.parse().unwrap())
            .local_private_key(&identity.private_key)
            .build_responder()
            .map_err(|e| CryptoError::NoiseHandshake(format!("resp failed: {}", e)))?;

        Ok(Self {
            state,
            is_initiator: false,
        })
    }

    /// Whether this side is the initiator.
    pub fn is_initiator(&self) -> bool {
        self.is_initiator
    }

    /// Whether the handshake is complete.
    pub fn is_finished(&self) -> bool {
        self.state.is_handshake_finished()
    }

    /// Write the next handshake message (our turn to send).
    /// Returns the bytes to send to the remote peer.
    pub fn write_message(&mut self, payload: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let mut buf = vec![0u8; MAX_NOISE_MSG_SIZE];
        let len = self.state.write_message(payload, &mut buf)
            .map_err(|e| CryptoError::NoiseHandshake(format!("write: {}", e)))?;
        buf.truncate(len);
        Ok(buf)
    }

    /// Read a handshake message from the remote peer.
    /// Returns any payload included in the handshake message.
    pub fn read_message(&mut self, message: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let mut buf = vec![0u8; MAX_NOISE_MSG_SIZE];
        let len = self.state.read_message(message, &mut buf)
            .map_err(|e| CryptoError::NoiseHandshake(format!("read: {}", e)))?;
        buf.truncate(len);
        Ok(buf)
    }

    /// Finalize the handshake into an encrypted transport.
    /// Call this after all 3 handshake messages have been exchanged.
    pub fn into_transport(self) -> Result<NoiseTransport, CryptoError> {
        let remote_key = self.state.get_remote_static()
            .map(|k| k.to_vec());

        let transport = self.state.into_transport_mode()
            .map_err(|e| CryptoError::NoiseHandshake(format!("transport: {}", e)))?;

        Ok(NoiseTransport {
            state: transport,
            remote_static_key: remote_key,
        })
    }
}

impl NoiseTransport {
    /// Encrypt a message for sending to the remote peer.
    pub fn encrypt(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        // Noise adds 16 bytes of AEAD tag
        let mut buf = vec![0u8; plaintext.len() + 16];
        let len = self.state.write_message(plaintext, &mut buf)
            .map_err(|e| CryptoError::NoiseTransport(format!("encrypt: {}", e)))?;
        buf.truncate(len);
        Ok(buf)
    }

    /// Decrypt a message received from the remote peer.
    pub fn decrypt(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let mut buf = vec![0u8; ciphertext.len()];
        let len = self.state.read_message(ciphertext, &mut buf)
            .map_err(|e| CryptoError::NoiseTransport(format!("decrypt: {}", e)))?;
        buf.truncate(len);
        Ok(buf)
    }

    /// Get the remote peer's static public key (their identity).
    pub fn remote_public_key(&self) -> Option<&[u8]> {
        self.remote_static_key.as_deref()
    }

    /// Get the remote peer's static public key as hex.
    pub fn remote_public_key_hex(&self) -> Option<String> {
        self.remote_static_key.as_ref().map(|k| hex::encode(k))
    }
}

/// Perform a complete XX handshake between two parties.
/// Returns (initiator_transport, responder_transport).
///
/// This is a convenience function for testing and simple use cases.
/// In production, the handshake messages are sent over TCP.
pub fn perform_handshake(
    initiator_identity: &NoiseIdentity,
    responder_identity: &NoiseIdentity,
) -> Result<(NoiseTransport, NoiseTransport), CryptoError> {
    let mut initiator = NoiseHandshake::initiator(initiator_identity)?;
    let mut responder = NoiseHandshake::responder(responder_identity)?;

    // Message 1: Initiator → Responder (ephemeral key)
    let msg1 = initiator.write_message(&[])?;
    responder.read_message(&msg1)?;

    // Message 2: Responder → Initiator (ephemeral + static)
    let msg2 = responder.write_message(&[])?;
    initiator.read_message(&msg2)?;

    // Message 3: Initiator → Responder (static, encrypted)
    let msg3 = initiator.write_message(&[])?;
    responder.read_message(&msg3)?;

    // Both sides transition to transport mode
    let i_transport = initiator.into_transport()?;
    let r_transport = responder.into_transport()?;

    Ok((i_transport, r_transport))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_noise_identity_generation() {
        let id = NoiseIdentity::generate().unwrap();
        assert_eq!(id.public_key.len(), 32); // X25519
        assert_eq!(id.private_key.len(), 32);
    }

    #[test]
    fn test_full_handshake() {
        let id_a = NoiseIdentity::generate().unwrap();
        let id_b = NoiseIdentity::generate().unwrap();

        let (transport_a, transport_b) = perform_handshake(&id_a, &id_b).unwrap();

        // A can see B's public key and vice versa
        assert!(transport_a.remote_public_key().is_some());
        assert!(transport_b.remote_public_key().is_some());
        assert_eq!(transport_a.remote_public_key().unwrap(), &id_b.public_key);
        assert_eq!(transport_b.remote_public_key().unwrap(), &id_a.public_key);
    }

    #[test]
    fn test_encrypted_communication() {
        let id_a = NoiseIdentity::generate().unwrap();
        let id_b = NoiseIdentity::generate().unwrap();

        let (mut transport_a, mut transport_b) = perform_handshake(&id_a, &id_b).unwrap();

        // A sends to B
        let plaintext = b"send 100 TAR to tar1abc";
        let ciphertext = transport_a.encrypt(plaintext).unwrap();
        assert_ne!(&ciphertext, plaintext.as_slice()); // must be encrypted
        assert!(ciphertext.len() > plaintext.len()); // includes AEAD tag

        let decrypted = transport_b.decrypt(&ciphertext).unwrap();
        assert_eq!(decrypted, plaintext);

        // B sends to A
        let response = b"confirmed: 100 TAR sent";
        let ct2 = transport_b.encrypt(response).unwrap();
        let dt2 = transport_a.decrypt(&ct2).unwrap();
        assert_eq!(dt2, response);
    }

    #[test]
    fn test_tampering_detected() {
        let id_a = NoiseIdentity::generate().unwrap();
        let id_b = NoiseIdentity::generate().unwrap();

        let (mut transport_a, mut transport_b) = perform_handshake(&id_a, &id_b).unwrap();

        let plaintext = b"important transaction";
        let mut ciphertext = transport_a.encrypt(plaintext).unwrap();

        // Tamper with ciphertext
        if let Some(byte) = ciphertext.last_mut() {
            *byte ^= 0xFF;
        }

        // Decryption must fail (AEAD tag mismatch)
        assert!(transport_b.decrypt(&ciphertext).is_err());
    }

    #[test]
    fn test_multiple_messages() {
        let id_a = NoiseIdentity::generate().unwrap();
        let id_b = NoiseIdentity::generate().unwrap();

        let (mut transport_a, mut transport_b) = perform_handshake(&id_a, &id_b).unwrap();

        for i in 0..10 {
            let msg = format!("message {}", i);
            let ct = transport_a.encrypt(msg.as_bytes()).unwrap();
            let pt = transport_b.decrypt(&ct).unwrap();
            assert_eq!(pt, msg.as_bytes());

            let reply = format!("reply {}", i);
            let ct2 = transport_b.encrypt(reply.as_bytes()).unwrap();
            let pt2 = transport_a.decrypt(&ct2).unwrap();
            assert_eq!(pt2, reply.as_bytes());
        }
    }

    #[test]
    fn test_replay_protection() {
        let id_a = NoiseIdentity::generate().unwrap();
        let id_b = NoiseIdentity::generate().unwrap();

        let (mut transport_a, mut transport_b) = perform_handshake(&id_a, &id_b).unwrap();

        let ct1 = transport_a.encrypt(b"first").unwrap();
        let ct2 = transport_a.encrypt(b"second").unwrap();

        // Must decrypt in order
        let _ = transport_b.decrypt(&ct1).unwrap();
        let _ = transport_b.decrypt(&ct2).unwrap();

        // Replaying ct1 must fail (nonce already used / out of order)
        assert!(transport_b.decrypt(&ct1).is_err());
    }

    #[test]
    fn test_handshake_step_by_step() {
        let id_a = NoiseIdentity::generate().unwrap();
        let id_b = NoiseIdentity::generate().unwrap();

        let mut init = NoiseHandshake::initiator(&id_a).unwrap();
        let mut resp = NoiseHandshake::responder(&id_b).unwrap();

        assert!(init.is_initiator());
        assert!(!resp.is_initiator());
        assert!(!init.is_finished());
        assert!(!resp.is_finished());

        // Step 1
        let msg1 = init.write_message(&[]).unwrap();
        resp.read_message(&msg1).unwrap();

        // Step 2
        let msg2 = resp.write_message(&[]).unwrap();
        init.read_message(&msg2).unwrap();

        // Step 3
        let msg3 = init.write_message(&[]).unwrap();
        resp.read_message(&msg3).unwrap();

        assert!(init.is_finished());
        assert!(resp.is_finished());

        // Transition to transport
        let mut t_init = init.into_transport().unwrap();
        let mut t_resp = resp.into_transport().unwrap();

        let ct = t_init.encrypt(b"hello after handshake").unwrap();
        let pt = t_resp.decrypt(&ct).unwrap();
        assert_eq!(pt, b"hello after handshake");
    }
}
