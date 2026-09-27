//! Networking: the LAN tunnel that joins rust-dos instances through a relay
//! over UDP (`tunnel`), and the Ethernet frames it carries (`frame`).
//!
//! Everything that crosses the tunnel is an Ethernet frame, so the IPX
//! driver of the built-in DOS and a network card of a booted system talk
//! to each other the way machines on one Ethernet segment would.

pub mod frame;
pub mod tunnel;

/// Fill `buf` with random bytes: from the operating system, or in the
/// browser, where there is none to ask, from the clock.
pub fn fill_random(buf: &mut [u8]) {
    #[cfg(not(target_arch = "wasm32"))]
    if getrandom::fill(buf).is_ok() {
        return;
    }
    let nanos =
        web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
    // SplitMix64 over the time and the buffer's address.
    let mut state = nanos ^ (buf.as_ptr() as u64).rotate_left(32);
    for chunk in buf.chunks_mut(8) {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        chunk.copy_from_slice(&z.to_le_bytes()[..chunk.len()]);
    }
}

/// A random 64-bit number.
pub fn random_u64() -> u64 {
    let mut bytes = [0; 8];
    fill_random(&mut bytes);
    u64::from_le_bytes(bytes)
}
