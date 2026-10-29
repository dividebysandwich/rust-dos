//! What INT 31h AX=0300h, 0301h and 0302h do, as DPMI 0.9 has them, for
//! both of the emulator's DPMI hosts: the DOS one (int31.rs) and the one
//! the built-in Windows gives its Win16 tasks (win16/dpmi.rs). The hosts
//! differ in how they switch modes and keep what they wait for; what goes
//! into real mode and what comes back is the same, and is here: the real
//! mode call structure, the stack the code runs on, what is pushed there
//! under it and the flags it starts with.
//!
//! 0300h runs the handler the real-mode vector table has for interrupt
//! BL, 0301h and 0302h the procedure at the structure's CS:IP. The code
//! runs with the structure's registers and segments, on the structure's
//! SS:SP or, when that is 0:0, a stack the host gives, with CX words of
//! the client's stack copied on top. 0300h and 0302h push an IRET frame
//! with the structure's flags and start the code with IF and TF clear;
//! 0301h pushes a far return address only. When the code returns, the
//! structure gets its general registers, flags and segments (but not
//! CS:IP and SS:SP), and the client goes on with CF clear.

use super::Context;
use crate::cpu::{CpuFlags, Seg};

/// The bytes of a real mode call structure.
pub const SIZE: usize = 0x32;

/// The index of ESP in `Context::gpr`.
const ESP: usize = 4;

/// The general registers in the structure's order, EDI first.
const ORDER: [usize; 8] = [7, 6, 5, 4, 3, 2, 1, 0];

/// The data segments' places in the structure.
const SEGMENTS: [(usize, Seg); 4] = [
    (0x22, Seg::ES),
    (0x24, Seg::DS),
    (0x26, Seg::FS),
    (0x28, Seg::GS),
];

/// The real-mode registers a call structure's `bytes` hold: the general
/// registers, the flags, the segments, CS:IP and SS:SP.
pub fn decode(bytes: &[u8; SIZE]) -> Context {
    let dword = |i: usize| u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
    let word = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]);
    let mut rm = Context::default();
    for (i, reg) in ORDER.into_iter().enumerate() {
        rm.gpr[reg] = dword(4 * i);
    }
    rm.gpr[ESP] = word(0x2E) as u32;
    rm.eflags = word(0x20) as u32;
    rm.eip = word(0x2A) as u32;
    rm.seg = [
        word(0x22),
        word(0x2C),
        word(0x30),
        word(0x24),
        word(0x26),
        word(0x28),
    ];
    rm
}

/// Write the real-mode registers `rm` into the call structure `bytes`:
/// the general registers, the flags and the data segments, and with `all`
/// CS:IP and SS:SP too, as a callback's procedure gets them. The
/// structure's ESP field is reserved: it stays as it is.
pub fn encode(rm: &Context, bytes: &mut [u8; SIZE], all: bool) {
    let mut put = |at: usize, value: &[u8]| bytes[at..at + value.len()].copy_from_slice(value);
    for (i, reg) in ORDER.into_iter().enumerate() {
        if reg != ESP {
            put(4 * i, &rm.gpr[reg].to_le_bytes());
        }
    }
    put(0x20, &(rm.eflags as u16).to_le_bytes());
    for (at, seg) in SEGMENTS {
        put(at, &rm.seg[seg as usize].to_le_bytes());
    }
    if all {
        put(0x2A, &(rm.eip as u16).to_le_bytes());
        put(0x2C, &rm.seg[Seg::CS as usize].to_le_bytes());
        put(0x2E, &(rm.gpr[ESP] as u16).to_le_bytes());
        put(0x30, &rm.seg[Seg::SS as usize].to_le_bytes());
    }
}

/// The three ways of calling real-mode code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// 0300h: the real-mode interrupt BL, through the vector table.
    Interrupt,
    /// 0301h: the procedure at CS:IP, which returns with a RETF.
    FarCall,
    /// 0302h: the procedure at CS:IP, which returns with an IRET.
    IretCall,
}

impl Kind {
    /// The kind of the function `function` (AX), if it is one of them.
    pub fn of(function: u16) -> Option<Kind> {
        match function {
            0x0300 => Some(Kind::Interrupt),
            0x0301 => Some(Kind::FarCall),
            0x0302 => Some(Kind::IretCall),
            _ => None,
        }
    }

    /// The words to push on the real-mode stack under the code, the first
    /// at the highest address: the client's `params` as its stack has
    /// them (the first at the lowest address), then, unless the code
    /// returns with a RETF, an IRET frame's flags, then the return
    /// address `ret`. The frame's flags are the structure's `flags`, but
    /// for TF: set there (Descent's serial driver passes FFFFh), the
    /// code's IRET would trap on the host's return, which DOS/4GW takes
    /// for a debug exception.
    pub fn frame(self, flags: u32, params: &[u16], (seg, off): (u16, u16)) -> Vec<u16> {
        let mut words: Vec<u16> = params.iter().rev().copied().collect();
        if self != Kind::FarCall {
            words.push((flags & 0xFFFF & !CpuFlags::TF.bits()) as u16);
        }
        words.extend([seg, off]);
        words
    }

    /// The flags the code starts with: the structure's `flags`, with TF
    /// clear, and IF too where an interrupt's handler starts.
    pub fn entry_flags(self, flags: u32) -> u32 {
        let cleared = match self {
            Kind::FarCall => CpuFlags::TF,
            Kind::Interrupt | Kind::IretCall => CpuFlags::IF | CpuFlags::TF,
        };
        flags & 0xFFFF & !cleared.bits()
    }
}

/// The real-mode stack of a call with the registers `rm`: the
/// structure's SS:SP, or the host's `host` when that is 0:0.
pub fn stack(rm: &Context, host: (u16, u32)) -> (u16, u32) {
    match (rm.seg[Seg::SS as usize], rm.gpr[ESP] & 0xFFFF) {
        (0, 0) => host,
        (ss, sp) => (ss, sp),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn structure() -> [u8; SIZE] {
        let mut bytes = [0u8; SIZE];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = i as u8;
        }
        bytes
    }

    #[test]
    fn a_structure_decodes_and_encodes_back() {
        let bytes = structure();
        let rm = decode(&bytes);
        assert_eq!(rm.gpr[7], 0x0302_0100, "EDI first");
        assert_eq!(rm.gpr[0], 0x1F1E_1D1C, "EAX last");
        assert_eq!(rm.eflags, 0x2120);
        assert_eq!(rm.seg[Seg::ES as usize], 0x2322);
        assert_eq!(rm.seg[Seg::SS as usize], 0x3130);
        let mut out = [0u8; SIZE];
        encode(&rm, &mut out, true);
        // All but the reserved ESP field.
        assert_eq!(out[..0x0C], bytes[..0x0C]);
        assert_eq!(out[0x0C..0x10], [0; 4]);
        assert_eq!(out[0x10..], bytes[0x10..]);
    }

    #[test]
    fn a_call_returning_leaves_cs_ip_and_ss_sp() {
        let mut bytes = structure();
        let mut rm = decode(&bytes);
        rm.eip = 0;
        rm.seg = [0; 6];
        rm.gpr[ESP] = 0;
        encode(&rm, &mut bytes, false);
        assert_eq!(bytes[0x22..0x2A], [0; 8], "the data segments");
        assert_eq!(bytes[0x2A..], structure()[0x2A..], "CS:IP and SS:SP");
    }

    #[test]
    fn the_frames_of_the_three_calls() {
        let params = [1, 2];
        let ret = (0xF000, 0x2004);
        let flags = 0x0302 | CpuFlags::TF.bits();
        assert_eq!(
            Kind::Interrupt.frame(flags, &params, ret),
            [2, 1, 0x0202, 0xF000, 0x2004]
        );
        assert_eq!(
            Kind::IretCall.frame(flags, &[], ret),
            [0x0202, 0xF000, 0x2004]
        );
        assert_eq!(
            Kind::FarCall.frame(flags, &params, ret),
            [2, 1, 0xF000, 0x2004]
        );
        assert_eq!(Kind::Interrupt.entry_flags(flags), 0x0002);
        assert_eq!(Kind::IretCall.entry_flags(flags), 0x0002);
        assert_eq!(Kind::FarCall.entry_flags(flags), 0x0202);
    }

    #[test]
    fn a_stack_of_0_0_is_the_hosts() {
        let mut rm = Context::default();
        assert_eq!(stack(&rm, (0x1234, 0x0800)), (0x1234, 0x0800));
        rm.seg[Seg::SS as usize] = 0x2000;
        rm.gpr[ESP] = 0x0100;
        assert_eq!(stack(&rm, (0x1234, 0x0800)), (0x2000, 0x0100));
    }
}
