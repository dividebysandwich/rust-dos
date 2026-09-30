//! The PCI bus, through configuration mechanism #1 (the address at CF8h,
//! the data at CFCh-CFFh) and the PCI BIOS (INT 1Ah AH=B1h), with the
//! cards DOSBox-X had where it had them, so that a Windows 95 installed
//! there knows them (BUS_00&DEV_00&FUNC_00 and BUS_00&DEV_01&FUNC_00):
//!
//! * device 0: the 3dfx Voodoo Graphics (`voodoo`), when there is one;
//! * device 1: the S3 Trio64 of `machine=svga_s3` (PCI\VEN_5333&DEV_8811),
//!   or the ViRGE (DEV_5631) or ViRGE/VX (DEV_883D) of `svga_s3virge` and
//!   `svga_s3virgevx`, whose BAR0 is the chip's linear frame buffer
//!   address, the same register as CR59/CR5Ah.
//!
//! A machine with neither has no PCI bus.

use crate::bus::Bus;
use crate::cpu::{Cpu, CpuFlags};
use iced_x86::Register;

/// Where the cards are.
const VOODOO_DEVICE: u32 = 0;
const S3_DEVICE: u32 = 1;

/// The configuration address latch (CF8h).
#[derive(Clone, Debug)]
pub struct Pci {
    pub address: u32,
    /// The S3's writable configuration: the command register.
    command: u16,
}

impl Default for Pci {
    /// As after a reset: the S3's I/O and memory decoding and palette
    /// snoop on, as its BIOS leaves them.
    fn default() -> Self {
        Self { address: 0, command: 0x0023 }
    }
}

crate::state_fields!(Pci { address, command });

/// The card a configuration access reaches.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Voodoo,
    S3,
}

impl Bus {
    /// Whether the machine has a PCI bus: an S3 or a 3dfx card on it.
    pub fn pci_present(&self) -> bool {
        self.s3() || self.voodoo.is_some()
    }

    /// The card and register the address latch selects, if it is enabled
    /// and names one.
    fn pci_target(&self) -> Option<(Target, u8)> {
        let a = self.pci.address;
        let (enabled, bus, device, function) = (a & 0x8000_0000 != 0, a >> 16 & 0xFF, a >> 11 & 0x1F, a >> 8 & 7);
        if !enabled || bus != 0 || function != 0 {
            return None;
        }
        let target = match device {
            VOODOO_DEVICE if self.voodoo.is_some() => Target::Voodoo,
            S3_DEVICE if self.s3() => Target::S3,
            _ => return None,
        };
        Some((target, a as u8 & 0xFC))
    }

    /// The S3's BAR0 size: 8 MB on the Trio64; 64 MB on a ViRGE, whose
    /// window holds the frame buffer and, 16 MB up, the registers.
    fn s3_bar_mask(&self) -> u32 {
        if self.virge() { 0xFC00_0000 } else { 0xFF80_0000 }
    }

    /// A byte of the S3's configuration space: an S3 Trio64 (5333h:8811h),
    /// ViRGE (5631h) or ViRGE/VX (883Dh), revision 0, a VGA-compatible
    /// display controller, BAR0 its linear frame buffer (prefetchable), no
    /// interrupt.
    fn s3_config(&self, reg: u8) -> u8 {
        let window = (self.vga.s3.crtc(0x59) as u32) << 24 | (self.vga.s3.crtc(0x5A) as u32) << 16;
        let bar0 = window & self.s3_bar_mask() | 0x08;
        let device = match self.vga.adapter {
            crate::video::adapter::Adapter::S3Virge => 0x5631,
            crate::video::adapter::Adapter::S3VirgeVx => 0x883D,
            _ => 0x8811,
        };
        let dword = match reg & 0xFC {
            0x00 => device << 16 | 0x5333,
            0x04 => 0x0280_0000 | self.pci.command as u32,
            0x08 => 0x0300_0000,
            0x10 => bar0,
            0x3C => 0x0000_00FF,
            _ => 0,
        };
        (dword >> (8 * (reg & 3))) as u8
    }

    /// A configuration data port (CFCh-CFFh) read.
    pub(crate) fn pci_read(&self, port: u16) -> u8 {
        match self.pci_target() {
            Some((Target::S3, reg)) => self.s3_config(reg + (port & 3) as u8),
            Some((Target::Voodoo, reg)) => self.voodoo.as_ref().map_or(0xFF, |v| v.config_read(reg + (port & 3) as u8)),
            None => 0xFF,
        }
    }

    /// A configuration data port written: for the S3, the command
    /// register's memory and I/O enables and palette snoop, and BAR0,
    /// which moves the frame buffer; for the 3dfx card, its registers.
    pub(crate) fn pci_write(&mut self, port: u16, value: u8) {
        let Some((target, reg)) = self.pci_target() else { return };
        let reg = reg + (port & 3) as u8;
        if target == Target::Voodoo {
            if let Some(v) = &mut self.voodoo
                && v.config_write(reg, value)
            {
                self.voodoo_moved();
            }
            return;
        }
        match reg {
            0x04 => self.pci.command = (self.pci.command & 0xFF00) | (value & 0x23) as u16,
            // Base bits 23-31 (26-31 on a ViRGE): CR5Ah bit 7, CR59h.
            0x12 if !self.virge() => {
                let low = (self.vga.s3.crtc(0x5A) & 0x7F) | (value & 0x80);
                self.s3_crtc_write(0x5A, low);
            }
            0x13 => self.s3_crtc_write(0x59, value & (self.s3_bar_mask() >> 24) as u8),
            _ => {}
        }
    }
}

/// INT 1Ah AH=B1h, the PCI BIOS 2.10: its installation check, finding
/// devices by ID or class, and reading and writing configuration space by
/// bus and device (BX) and register (DI), as DOSBox-X has it.
pub fn bios(cpu: &mut Cpu) {
    if !cpu.bus.pci_present() {
        cpu.set_reg8(Register::AH, 0x81);
        cpu.set_cpu_flag(CpuFlags::CF, true);
        return;
    }
    let config = |cpu: &mut Cpu, reg: u8| {
        cpu.bus.pci.address = 0x8000_0000 | (cpu.bx() as u32) << 8 | reg as u32 & 0xFC;
    };
    let ok = |cpu: &mut Cpu| {
        cpu.set_reg8(Register::AH, 0);
        cpu.set_cpu_flag(CpuFlags::CF, false);
    };
    let read = |cpu: &mut Cpu, reg: u8, len: u8| -> u32 {
        (0..len).map(|i| (cpu.bus.pci_read(0xCFC + ((reg & 3) + i) as u16) as u32) << (8 * i)).sum()
    };
    let reg = cpu.di() as u8;
    match cpu.get_al() {
        // Installation check: mechanism #1, version 2.10, one bus, "PCI ".
        0x01 => {
            cpu.set_ax(0x0001);
            cpu.set_bx(0x0210);
            cpu.set_cx(0x0000);
            cpu.set_edx(0x2049_4350);
            cpu.set_edi(0);
            cpu.set_cpu_flag(CpuFlags::CF, false);
        }
        // Find the SI'th device with ID CX:DX, or of class ECX.
        al @ (0x02 | 0x03) => {
            let wanted = if al == 0x02 { (cpu.cx() as u32) << 16 | cpu.dx() as u32 } else { cpu.ecx() & 0xFF_FFFF };
            let index = cpu.si();
            let mut found = None;
            let mut count = 0;
            for devfn in 0..=0xFFu16 {
                cpu.bus.pci.address = 0x8000_0000 | (devfn as u32) << 8;
                let id = read(cpu, 0, 4);
                if id == 0xFFFF_FFFF {
                    continue;
                }
                cpu.bus.pci.address |= 0x08;
                let value = if al == 0x02 { id } else { read(cpu, 0, 4) >> 8 };
                if value == wanted {
                    if count == index {
                        found = Some(devfn);
                        break;
                    }
                    count += 1;
                }
            }
            match found {
                Some(devfn) => {
                    cpu.set_bx(devfn);
                    ok(cpu);
                }
                None => {
                    cpu.set_reg8(Register::AH, 0x86);
                    cpu.set_cpu_flag(CpuFlags::CF, true);
                }
            }
        }
        // Read a configuration byte, word or doubleword.
        al @ 0x08..=0x0A => {
            config(cpu, reg);
            let len = 1 << (al - 0x08);
            let value = read(cpu, reg & !(len - 1), len);
            match len {
                1 => cpu.set_reg8(Register::CL, value as u8),
                2 => cpu.set_cx(value as u16),
                _ => cpu.set_ecx(value),
            }
            ok(cpu);
        }
        // Write one.
        al @ 0x0B..=0x0D => {
            config(cpu, reg);
            let len = 1 << (al - 0x0B);
            let value = cpu.ecx();
            let at = reg & 3 & !(len - 1);
            for i in 0..len {
                cpu.bus.pci_write(0xCFC + (at + i) as u16, (value >> (8 * i)) as u8);
            }
            ok(cpu);
        }
        _ => {
            cpu.set_reg8(Register::AH, 0x81);
            cpu.set_cpu_flag(CpuFlags::CF, true);
        }
    }
}
