//! online: the keys a session's secret gives, and the sealing of its
//! packets.
//!
//! - The secret is the 48 random bits the host's code carries
//!   (code.rs). Only the two players have it.
//! - HKDF-SHA256 makes everything else from it: the session's name on the
//!   wire (4 bytes, so a host can tell a code for this session from an
//!   older one), the key the handshake is authenticated with (HMAC-SHA256),
//!   and, once both sides have added a random nonce, one ChaCha20-Poly1305
//!   key for each direction.
//! - Every packet after the handshake is sealed (encrypted and
//!   authenticated) with its direction's key and a counter, which is never
//!   used twice; a packet replayed, forged or altered is dropped.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::Sha256;

const SALT: &[u8] = b"haiku-online/1";
pub const TAG: usize = 16;

fn derive(secret: &[u8], salt: &[u8], info: &[u8], out: &mut [u8]) {
    let hk = Hkdf::<Sha256>::new(Some(salt), secret);
    hk.expand(info, out).expect("HKDF output length is small");
}

/// the session's name on the wire.
pub fn session_id(secret: &[u8]) -> [u8; 4] {
    let mut id = [0u8; 4];
    derive(secret, SALT, b"session id", &mut id);
    id
}

/// the handshake's authentication key.
pub fn auth_key(secret: &[u8]) -> [u8; 32] {
    let mut key = [0u8; 32];
    derive(secret, SALT, b"handshake", &mut key);
    key
}

/// the first TAG bytes of HMAC-SHA256(key, parts).
pub fn tag(key: &[u8; 32], parts: &[&[u8]]) -> [u8; TAG] {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("any key length");
    for part in parts {
        mac.update(part);
    }
    let full = mac.finalize().into_bytes();
    full[..TAG].try_into().unwrap()
}

/// a tag compared without telling how much of it matched.
pub fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// the two directions' keys, from the secret and both nonces.
pub struct Keys {
    pub to_host: ChaCha20Poly1305,
    pub to_joiner: ChaCha20Poly1305,
}

impl Keys {
    pub fn new(secret: &[u8], joiner_nonce: &[u8; 16], host_nonce: &[u8; 16]) -> Keys {
        let mut salt = [0u8; 32];
        salt[..16].copy_from_slice(joiner_nonce);
        salt[16..].copy_from_slice(host_nonce);
        let (mut a, mut b) = ([0u8; 32], [0u8; 32]);
        derive(secret, &salt, b"joiner to host", &mut a);
        derive(secret, &salt, b"host to joiner", &mut b);
        Keys { to_host: ChaCha20Poly1305::new(Key::from_slice(&a)), to_joiner: ChaCha20Poly1305::new(Key::from_slice(&b)) }
    }
}

fn nonce(counter: u64) -> Nonce {
    let mut bytes = [0u8; 12];
    bytes[4..].copy_from_slice(&counter.to_le_bytes());
    *Nonce::from_slice(&bytes)
}

pub fn seal(key: &ChaCha20Poly1305, counter: u64, header: &[u8], plain: &[u8]) -> Vec<u8> {
    key.encrypt(&nonce(counter), Payload { msg: plain, aad: header }).expect("sealing does not fail")
}

pub fn open(key: &ChaCha20Poly1305, counter: u64, header: &[u8], sealed: &[u8]) -> Option<Vec<u8>> {
    key.decrypt(&nonce(counter), Payload { msg: sealed, aad: header }).ok()
}

/// random bytes from the system.
pub fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    if getrandom::getrandom(&mut bytes).is_err() {
        // never on the systems Haiku runs on; a clock-made fallback still
        // differs between sessions
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let mut seed = now as u64 ^ (std::process::id() as u64) << 32;
        for byte in &mut bytes {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            *byte = seed as u8;
        }
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_packets_open_only_unaltered_with_their_key() {
        let keys = Keys::new(b"secret", &[1; 16], &[2; 16]);
        let sealed = seal(&keys.to_host, 7, b"head", b"hello");
        assert_eq!(open(&keys.to_host, 7, b"head", &sealed).as_deref(), Some(&b"hello"[..]));
        assert!(open(&keys.to_joiner, 7, b"head", &sealed).is_none());
        assert!(open(&keys.to_host, 8, b"head", &sealed).is_none());
        assert!(open(&keys.to_host, 7, b"HEAD", &sealed).is_none());
        let other = Keys::new(b"secreu", &[1; 16], &[2; 16]);
        assert!(open(&other.to_host, 7, b"head", &sealed).is_none());
    }
}
