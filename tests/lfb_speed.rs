//! How fast translated code copies into the VESA linear frame buffer, as
//! DirectDraw's own copy loop does (`mov eax,[edi]; mov [esi],eax`, a dword
//! at a time), against the same loop copying into RAM:
//!
//! `cargo test --release --test lfb_speed -- --ignored --nocapture`

mod dyndiff;
mod pmrig;

use iced_x86::code_asm::*;
use pmrig::*;
use rust_dos::cpu::CoreMode;
use rust_dos::exec::StopReason;
use std::time::Instant;

const LFB: u32 = 0xE000_0000;
/// A 640x480 16-bit frame, in dwords.
const FRAME: u32 = 640 * 480 * 2 / 4;

/// Seconds for `frames` copies of a frame from RAM to `dst`.
fn time(dst: u32, frames: u32) -> f64 {
    let mut rig = Rig::new();
    rig.cpu.core = CoreMode::Dynamic;
    rig.cpu.set_ax(0x4F02);
    rig.cpu.set_reg16(iced_x86::Register::BX, 0x4111);
    rust_dos::interrupts::int10::handle(&mut rig.cpu);
    assert_eq!(rig.cpu.ax(), 0x004F, "no VESA mode");
    rig.load(CODE, &asm32(CODE, |a| {
        let mut frame = a.create_label();
        let mut dword = a.create_label();
        a.mov(ebx, frames)?;
        a.set_label(&mut frame)?;
        a.mov(edi, 0x20_0000u32)?;
        a.mov(esi, dst)?;
        a.lea(edx, dword_ptr(esi + FRAME * 4))?;
        a.set_label(&mut dword)?;
        a.mov(eax, dword_ptr(edi))?;
        a.add(edi, 4)?;
        a.mov(dword_ptr(esi), eax)?;
        a.add(esi, 4)?;
        a.cmp(esi, edx)?;
        a.jne(dword)?;
        a.dec(ebx)?;
        a.jnz(frame)?;
        a.hlt()
    }));
    rig.enter_pm();
    let start = Instant::now();
    while dyndiff::batch(&mut rig.cpu, 1_000_000, true) != StopReason::Paused {}
    start.elapsed().as_secs_f64()
}

#[test]
#[ignore]
fn frame_buffer_copy_speed() {
    let frames = 60;
    for (name, dst) in [("RAM", 0x40_0000u32), ("frame buffer", LFB)] {
        let secs = (0..3).map(|_| time(dst, frames)).fold(f64::MAX, f64::min);
        println!(
            "into {:12}: {:6.1} ms a frame, {:5.2} ns a dword",
            name,
            secs * 1e3 / frames as f64,
            secs * 1e9 / (frames as f64 * FRAME as f64)
        );
    }
}
