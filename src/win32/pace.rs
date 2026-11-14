//! The frame rate cap (`fps_cap`, `crate::frame_cap`) at the built-in
//! Windows' presents: DirectDraw's Flip and blits to the primary surface,
//! Direct3D 8's Present, SwapBuffers and Glide's grBufferSwap. A present
//! the cap holds back blocks its thread until the cap's turn, as a wait
//! for the retrace does (`directdraw::wait_until`), and runs again then:
//! the program's other threads, its timers and its sound go on.
//!
//! A present calls `turn` (or `check`) before it changes anything, and
//! returns what it gives back while the thread waits; once its frame
//! shows, it tells the cap with `shown`. The 3dfx card's swaps tell the
//! cap themselves (`Voodoo::swap`): a present on the card asks
//! `card_turn`, which holds the thread where the card would hold its
//! swap.

use super::ddraw::directdraw;
use crate::cpu::Cpu;

/// A frame the running thread is about to present, which shows at `at`
/// (emulated ns) and, with `refresh_ns`, at a retrace of a display of
/// that frame time (`FrameCap::wait`). Ok with the time it counts as
/// shown at, for `shown`, to go ahead; Err with the time the thread
/// waits for, after which the call runs again.
pub fn check(cpu: &Cpu, at: u64, refresh_ns: Option<u64>) -> Result<u64, u64> {
    match cpu.bus.frame_cap.wait(at, refresh_ns) {
        None => Ok(at),
        // The turn comes within this microsecond, the clock's step for a
        // thread's wait: it counts from its turn.
        Some(until) if until.div_ceil(1000) <= cpu.bus.clock.now_micros() => Ok(until),
        Some(until) => Err(until),
    }
}

/// `check`, the thread waiting for the cap's turn if it has to: Err with
/// what the call returns then (`wait`).
pub fn turn(
    cpu: &mut Cpu,
    at: u64,
    refresh_ns: Option<u64>,
    result: u32,
) -> Result<u64, Option<u32>> {
    check(cpu, at, refresh_ns).map_err(|until| wait(cpu, until, result))
}

/// The thread waits until `until` (ns), and the call it is in runs again
/// then; what the call returns now.
pub fn wait(cpu: &mut Cpu, until: u64, result: u32) -> Option<u32> {
    cpu.set_eip(cpu.eip().wrapping_sub(4));
    directdraw::wait_until(cpu, until, result)
}

/// The frame presented showed at `at`, as `check` gave it.
pub fn shown(cpu: &mut Cpu, at: u64) {
    cpu.bus.frame_cap.shown(at);
}

/// `turn` for a swap of the 3dfx card with the swapbufferCMD `cmd`,
/// which the card makes and counts. Ok to go ahead.
pub fn card_turn(cpu: &mut Cpu, cmd: u32, result: u32) -> Result<(), Option<u32>> {
    match card_check(cpu, cmd) {
        Some(until) => Err(wait(cpu, until, result)),
        None => Ok(()),
    }
}

/// When the thread waits for before it makes a swap of the 3dfx card
/// with the swapbufferCMD `cmd`, if it has to.
pub fn card_check(cpu: &Cpu, cmd: u32) -> Option<u64> {
    let now = cpu.bus.voodoo_now();
    let v = cpu.bus.voodoo.as_ref()?;
    let (at, refresh) = (v.swap_shows_at(cmd, now), v.swap_refresh_ns(cmd));
    // A turn within this microsecond the card holds itself, for less.
    cpu.bus
        .frame_cap
        .wait(at, refresh)
        .filter(|until| until.div_ceil(1000) > cpu.bus.clock.now_micros())
}
