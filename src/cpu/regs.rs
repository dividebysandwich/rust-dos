//! Register file: general-purpose registers, the instruction pointer and the
//! segment registers with their descriptor caches.
//!
//! The general-purpose registers are stored as 32-bit values in x86 encoding
//! order. The 16-bit and 8-bit accessors read and write the low parts and
//! leave the rest of the register alone, as the hardware does: a real-mode
//! service that sets AX must not clobber the upper half of EAX.

use iced_x86::Register;

use super::Cpu;

pub const EAX: usize = 0;
pub const ECX: usize = 1;
pub const EDX: usize = 2;
pub const EBX: usize = 3;
pub const ESP: usize = 4;
pub const EBP: usize = 5;
pub const ESI: usize = 6;
pub const EDI: usize = 7;

/// A segment register, in x86 encoding order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seg {
    ES = 0,
    CS = 1,
    SS = 2,
    DS = 3,
    FS = 4,
    GS = 5,
}

impl Seg {
    pub const ALL: [Seg; 6] = [Seg::ES, Seg::CS, Seg::SS, Seg::DS, Seg::FS, Seg::GS];

    /// The segment register named by an iced `Register`, if it is one.
    pub fn from_register(reg: Register) -> Option<Seg> {
        match reg {
            Register::ES => Some(Seg::ES),
            Register::CS => Some(Seg::CS),
            Register::SS => Some(Seg::SS),
            Register::DS => Some(Seg::DS),
            Register::FS => Some(Seg::FS),
            Register::GS => Some(Seg::GS),
            _ => None,
        }
    }
}

/// Access rights of a present, writable, accessed data segment: the state a
/// segment register's hidden part has after reset.
pub const AR_DATA_RW: u16 = 0x0093;
/// Descriptor flag: default operand size / stack pointer size is 32 bits.
pub const ATTR_DB: u16 = 0x4000;
/// Descriptor flag: the limit counts 4 KB pages.
pub const ATTR_G: u16 = 0x8000;

/// A segment register: the selector the program loaded and the descriptor
/// cache (base, limit, access rights) the CPU uses for every access through
/// it. In real mode a load only changes the selector and the base, so
/// limits set in protected mode survive a switch back ("unreal mode").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SegCache {
    pub selector: u16,
    pub base: u32,
    pub limit: u32,
    /// Descriptor access byte (bits 0-7: type, S, DPL, P) and flags
    /// (bits 12-15: AVL, 0, D/B, G), as in the descriptor's bytes 5 and 6.
    pub attr: u16,
}

impl SegCache {
    /// The register after reset, or after a real-mode load of `selector`
    /// into a register that was never loaded in protected mode.
    pub const fn real(selector: u16) -> Self {
        Self {
            selector,
            base: (selector as u32) << 4,
            limit: 0xFFFF,
            attr: AR_DATA_RW,
        }
    }
}

/// Getter and setter pairs for the 32-bit, 16-bit and 8-bit views of the
/// general-purpose registers.
macro_rules! gpr_accessors {
    ($($idx:ident: $get32:ident $set32:ident, $get16:ident $set16:ident;)*) => {
        $(
            #[inline(always)]
            pub fn $get32(&self) -> u32 {
                self.gpr[$idx]
            }
            #[inline(always)]
            pub fn $set32(&mut self, value: u32) {
                self.gpr[$idx] = value;
            }
            #[inline(always)]
            pub fn $get16(&self) -> u16 {
                self.gpr[$idx] as u16
            }
            #[inline(always)]
            pub fn $set16(&mut self, value: u16) {
                self.gpr[$idx] = (self.gpr[$idx] & 0xFFFF_0000) | value as u32;
            }
        )*
    };
}

/// Getter and real-mode setter pairs for the segment registers.
macro_rules! seg_accessors {
    ($($seg:ident: $get:ident $set:ident;)*) => {
        $(
            /// The selector (in real mode: the segment) in the register.
            #[inline(always)]
            pub fn $get(&self) -> u16 {
                self.seg[Seg::$seg as usize].selector
            }
            /// Load the register as a real-mode segment.
            #[inline(always)]
            pub fn $set(&mut self, value: u16) {
                self.load_seg_real(Seg::$seg, value);
            }
        )*
    };
}

impl Cpu {
    gpr_accessors! {
        EAX: eax set_eax, ax set_ax;
        ECX: ecx set_ecx, cx set_cx;
        EDX: edx set_edx, dx set_dx;
        EBX: ebx set_ebx, bx set_bx;
        ESP: esp set_esp, sp set_sp;
        EBP: ebp set_ebp, bp set_bp;
        ESI: esi set_esi, si set_si;
        EDI: edi set_edi, di set_di;
    }

    seg_accessors! {
        ES: es set_es;
        CS: cs set_cs;
        SS: ss set_ss;
        DS: ds set_ds;
        FS: fs set_fs;
        GS: gs set_gs;
    }

    #[inline(always)]
    pub fn eip(&self) -> u32 {
        self.eip
    }
    #[inline(always)]
    pub fn set_eip(&mut self, value: u32) {
        self.eip = value;
    }
    /// The instruction pointer of 16-bit code.
    #[inline(always)]
    pub fn ip(&self) -> u16 {
        self.eip as u16
    }
    /// Set the instruction pointer of 16-bit code. The upper half of EIP is
    /// cleared, as a 16-bit jump does.
    #[inline(always)]
    pub fn set_ip(&mut self, value: u16) {
        self.eip = value as u32;
    }

    /// A segment register's selector and descriptor cache.
    #[inline(always)]
    pub fn seg_cache(&self, seg: Seg) -> &SegCache {
        &self.seg[seg as usize]
    }

    /// Load a segment register the way real mode does: the selector and a
    /// base of `value << 4`. Limit and access rights keep their values.
    #[inline(always)]
    pub fn load_seg_real(&mut self, seg: Seg, value: u16) {
        let cache = &mut self.seg[seg as usize];
        cache.selector = value;
        cache.base = (value as u32) << 4;
    }

    /// Value of a general-purpose (8, 16 or 32-bit) or segment register,
    /// zero-extended.
    #[inline(always)]
    pub fn reg(&self, reg: Register) -> u32 {
        // iced numbers the registers AL..BH (1-8), AX..DI (21-28),
        // EAX..EDI (37-44) and ES..GS (71-76) in x86 encoding order.
        let r = reg as usize;
        match r {
            1..=4 => self.gpr[r - 1] & 0xFF,
            5..=8 => (self.gpr[r - 5] >> 8) & 0xFF,
            21..=28 => self.gpr[r - 21] & 0xFFFF,
            37..=44 => self.gpr[r - 37],
            71..=76 => self.seg[r - 71].selector as u32,
            _ => 0,
        }
    }

    /// Write a general-purpose register: the low 8 or 16 bits for the
    /// smaller registers, leaving the rest alone. Segment registers are not
    /// written here (see `load_seg_real`).
    #[inline(always)]
    pub fn set_reg(&mut self, reg: Register, value: u32) {
        let r = reg as usize;
        match r {
            1..=4 => self.gpr[r - 1] = (self.gpr[r - 1] & !0xFF) | (value & 0xFF),
            5..=8 => self.gpr[r - 5] = (self.gpr[r - 5] & !0xFF00) | ((value & 0xFF) << 8),
            21..=28 => self.gpr[r - 21] = (self.gpr[r - 21] & !0xFFFF) | (value & 0xFFFF),
            37..=44 => self.gpr[r - 37] = value,
            _ => debug_assert!(false, "set_reg on {:?}", reg),
        }
    }

    // Extract High byte (AH)
    pub fn get_ah(&self) -> u8 {
        (self.gpr[EAX] >> 8) as u8
    }
    // Extract Low byte (AL)
    pub fn get_al(&self) -> u8 {
        self.gpr[EAX] as u8
    }

    /// Index into `gpr` and bit shift of an 8-bit register.
    fn reg8_slot(reg: Register) -> Option<(usize, u32)> {
        match reg {
            Register::AL => Some((EAX, 0)),
            Register::CL => Some((ECX, 0)),
            Register::DL => Some((EDX, 0)),
            Register::BL => Some((EBX, 0)),
            Register::AH => Some((EAX, 8)),
            Register::CH => Some((ECX, 8)),
            Register::DH => Some((EDX, 8)),
            Register::BH => Some((EBX, 8)),
            _ => None,
        }
    }

    /// Index into `gpr` of a 16-bit or 32-bit general-purpose register.
    fn gpr_index(reg: Register) -> Option<usize> {
        match reg {
            Register::AX | Register::EAX => Some(EAX),
            Register::CX | Register::ECX => Some(ECX),
            Register::DX | Register::EDX => Some(EDX),
            Register::BX | Register::EBX => Some(EBX),
            Register::SP | Register::ESP => Some(ESP),
            Register::BP | Register::EBP => Some(EBP),
            Register::SI | Register::ESI => Some(ESI),
            Register::DI | Register::EDI => Some(EDI),
            _ => None,
        }
    }

    // Set 8-bit Register
    pub fn set_reg8(&mut self, reg: Register, value: u8) {
        if let Some((idx, shift)) = Self::reg8_slot(reg) {
            self.gpr[idx] = (self.gpr[idx] & !(0xFF << shift)) | ((value as u32) << shift);
        }
    }

    // Get 8-bit Register
    pub fn get_reg8(&self, reg: Register) -> u8 {
        match Self::reg8_slot(reg) {
            Some((idx, shift)) => (self.gpr[idx] >> shift) as u8,
            None => 0,
        }
    }

    // Set 16-bit Register
    pub fn set_reg16(&mut self, reg: Register, value: u16) {
        if let Some(idx) = Self::gpr_index(reg) {
            self.gpr[idx] = (self.gpr[idx] & 0xFFFF_0000) | value as u32;
        } else if let Some(seg) = Seg::from_register(reg) {
            self.load_seg_real(seg, value);
        } else {
            panic!("Unimplemented register write: {:?}", reg);
        }
    }

    // Get 16-bit Register
    pub fn get_reg16(&self, reg: Register) -> u16 {
        if let Some(idx) = Self::gpr_index(reg) {
            self.gpr[idx] as u16
        } else if let Some(seg) = Seg::from_register(reg) {
            self.seg[seg as usize].selector
        } else {
            0 // Panic or return 0 for unhandled registers
        }
    }
}
