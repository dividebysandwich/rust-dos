//! Joining a relay: the cookie a relay hands out without keeping any state,
//! and the proof that a client knows the room's password, which never
//! crosses the network itself.

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use std::net::SocketAddr;

type HmacSha256 = Hmac<Sha256>;

/// How long a cookie stays good: from its time slot to the end of the next.
pub const COOKIE_SLOT_MS: u64 = 10_000;

fn hmac(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC takes keys of any length");
    for part in parts {
        // Length-prefixed, so the parts can't be shifted into each other.
        mac.update(&(part.len() as u32).to_be_bytes());
        mac.update(part);
    }
    mac.finalize().into_bytes().into()
}

/// The cookie of a client at `addr` with `client_id`, in time slot `slot`.
pub fn cookie(secret: &[u8; 32], addr: SocketAddr, client_id: u64, slot: u64) -> [u8; 16] {
    let full = hmac(secret, &[b"cookie", addr.to_string().as_bytes(), &client_id.to_be_bytes(), &slot.to_be_bytes()]);
    full[..16].try_into().unwrap()
}

/// Whether `cookie` is one handed to this client in the time slot of `now`
/// (ms) or the one before.
pub fn cookie_is_good(secret: &[u8; 32], addr: SocketAddr, client_id: u64, now: u64, cookie: &[u8; 16]) -> bool {
    let slot = now / COOKIE_SLOT_MS;
    [slot, slot.wrapping_sub(1)].into_iter().any(|s| equal(&self::cookie(secret, addr, client_id, s), cookie))
}

/// What JOIN carries to show the client knows `password`. Zeros for a room
/// without one.
pub fn proof(password: Option<&str>, room: &str, client_id: u64, cookie: &[u8; 16]) -> [u8; 32] {
    match password {
        Some(password) if !password.is_empty() => {
            hmac(password.as_bytes(), &[b"rust-dos lan v1", room.as_bytes(), &client_id.to_be_bytes(), cookie])
        }
        _ => [0; 32],
    }
}

/// Compare without leaking where the first difference is.
pub fn equal(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookies_are_bound_to_client_and_time() {
        let secret = [1; 32];
        let addr: SocketAddr = "192.0.2.1:4000".parse().unwrap();
        let other: SocketAddr = "192.0.2.1:4001".parse().unwrap();
        let now = 123_456_789;
        let c = cookie(&secret, addr, 7, now / COOKIE_SLOT_MS);
        assert!(cookie_is_good(&secret, addr, 7, now, &c));
        assert!(cookie_is_good(&secret, addr, 7, now + COOKIE_SLOT_MS, &c));
        assert!(!cookie_is_good(&secret, addr, 7, now + 2 * COOKIE_SLOT_MS, &c));
        assert!(!cookie_is_good(&secret, other, 7, now, &c));
        assert!(!cookie_is_good(&secret, addr, 8, now, &c));
        assert!(!cookie_is_good(&[2; 32], addr, 7, now, &c));
    }

    #[test]
    fn proofs_need_the_password() {
        let c = [5; 16];
        let p = proof(Some("swordfish"), "doom", 1, &c);
        assert_ne!(p, [0; 32]);
        assert_eq!(p, proof(Some("swordfish"), "doom", 1, &c));
        assert_ne!(p, proof(Some("swordfisH"), "doom", 1, &c));
        assert_ne!(p, proof(Some("swordfish"), "duke", 1, &c));
        assert_ne!(p, proof(Some("swordfish"), "doom", 2, &c));
        assert_ne!(p, proof(Some("swordfish"), "doom", 1, &[6; 16]));
        assert_eq!(proof(None, "doom", 1, &c), [0; 32]);
        assert_eq!(proof(Some(""), "doom", 1, &c), [0; 32]);
        assert!(equal(&p, &p) && !equal(&p, &[0; 32]) && !equal(&p, &p[..31]));
    }
}
