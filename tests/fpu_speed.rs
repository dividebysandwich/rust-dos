//! How fast translated code runs a value that FADD left with more than a
//! double's precision through FIST, FCOM and FST m32, against FST m64,
//! which takes the double as it is (exact mode, rounding to nearest):
//!
//! `cargo test --release --test fpu_speed -- --ignored --nocapture`

mod dyndiff;
mod pmrig;

use iced_x86::code_asm::*;
use pmrig::*;
use rust_dos::cpu::CoreMode;
use rust_dos::exec::StopReason;
use std::time::Instant;

const LOOPS: u32 = 2_000_000;

/// Seconds for `LOOPS` rounds of `x += 0.1`, then `op` on the sum, and
/// the blocks run (a round is one block, where the handler runs nothing).
fn time(op: &str) -> (f64, u64) {
    let mut rig = Rig::new();
    rig.cpu.core = CoreMode::Dynamic;
    let b = 1.0f64.to_bits();
    rig.write32(DATA, b as u32);
    rig.write32(DATA + 4, (b >> 32) as u32);
    let b = 0.1f64.to_bits();
    rig.write32(DATA + 8, b as u32);
    rig.write32(DATA + 8 + 4, (b >> 32) as u32);
    let b = 1e300f64.to_bits();
    rig.write32(DATA + 16, b as u32);
    rig.write32(DATA + 16 + 4, (b >> 32) as u32);
    rig.load(CODE, &asm32(CODE, |a| {
        let mut top = a.create_label();
        a.fninit()?;
        a.mov(esi, DATA)?;
        a.mov(ecx, LOOPS)?;
        a.set_label(&mut top)?;
        a.fld(qword_ptr(esi))?;
        a.fadd(qword_ptr(esi + 8))?;
        a.fst(qword_ptr(esi))?;
        match op {
            "fistp m32" => a.fistp(dword_ptr(esi + 32))?,
            "fcomp m64" => {
                a.fcomp(qword_ptr(esi + 16))?;
                a.fnstsw(ax)?;
            }
            "fstp m32" => a.fstp(dword_ptr(esi + 32))?,
            _ => a.fstp(qword_ptr(esi + 32))?,
        }
        a.dec(ecx)?;
        a.jnz(top)?;
        a.hlt()
    }));
    rig.enter_pm();
    let start = Instant::now();
    while dyndiff::batch(&mut rig.cpu, 1_000_000, true) != StopReason::Paused {}
    (start.elapsed().as_secs_f64(), rig.cpu.dynrec.stats().runs)
}

#[test]
#[ignore]
fn rounding_speed() {
    for op in ["fstp m64", "fistp m32", "fcomp m64", "fstp m32"] {
        let (secs, runs) = (0..3).map(|_| time(op)).min_by(|a, b| a.0.total_cmp(&b.0)).unwrap();
        println!(
            "fld, fadd, fst m64, {:9}: {:6.1} ns a round, {:.3} blocks a round",
            op,
            secs * 1e9 / LOOPS as f64,
            runs as f64 / LOOPS as f64
        );
    }
}
