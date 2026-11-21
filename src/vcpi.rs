//! VCPI 1.0 (INT 67h AX=DE00h-DE0Ch), which the built-in virtual-8086
//! monitor (v86mon.rs) provides with expanded memory, as EMM386 does: DOS
//! extenders that find no DPMI host switch to protected mode through it,
//! with page tables, descriptor tables and a TSS of their own, and back
//! to virtual-8086 mode for DOS and the BIOS.
//!
//! The interface is there only in virtual-8086 mode under the monitor:
//! in real mode (a state saved before the monitor, a test) AX=DE00h says
//! it isn't, as with an EMS board and no memory manager. The 4 KB pages
//! (DE04h) come from the expanded memory's pool, 16 KB at a time, and go
//! back to it when no program runs any more, with its pages.

use crate::bus::Bus;
use crate::cpu::{CR0_PG, Cpu, CpuFlags, CpuState, DescTable, Seg, SegCache};

const OK: u8 = 0x00;
const UNDEFINED_FUNCTION: u8 = 0x84;
const NOT_ENOUGH_PAGES: u8 = 0x88;
const BAD_PAGE: u8 = 0x8A;
const PHYSICAL_PAGE: u8 = 0x8B;
const SUBFUNCTION: u8 = 0x8F;

const PAGE: u32 = 0x1000;
/// The page table entries DE01h gives: the first megabyte and the HMA,
/// mapped where they are.
const MAPPED_PAGES: u32 = 0x110;
/// The entries' bits: present, writable, user, accessed, dirty.
const PTE_BITS: u32 = 0x67;

/// The far service of the protected-mode entry point (`bios::FAR_VCPI`).
pub fn install_entry(bus: &mut Bus) {
    let at = 0xF0000 + crate::v86mon::VCPI_ENTRY as usize;
    bus.write_rom(at, &[0xFE, 0x3A, crate::bios::FAR_VCPI, 0xCB]);
}

/// What the interface keeps between calls.
#[derive(Clone, Debug)]
pub struct Vcpi {
    /// The 4 KB pages handed out (DE04h), by physical address.
    pages: Vec<u32>,
    /// Pages of the 16 KB taken from the pool for them, not handed out.
    spare: Vec<u32>,
    /// The base vectors of the master and the slave PIC, as the client
    /// set them (DE0Bh).
    pic: (u8, u8),
    /// The A20 gate DOS had, while a client runs in protected mode with
    /// the gate open.
    v86_a20: Option<bool>,
}

impl Default for Vcpi {
    fn default() -> Self {
        Self {
            pages: Vec::new(),
            spare: Vec::new(),
            pic: (0x08, 0x70),
            v86_a20: None,
        }
    }
}

impl Vcpi {
    /// The 4 KB pages handed out, for debuggers.
    pub fn pages(&self) -> usize {
        self.pages.len()
    }
}

/// INT 67h AH=DEh from virtual-8086 mode, AL the function: the status.
pub fn call(cpu: &mut Cpu) -> u8 {
    if !crate::v86mon::owns(cpu) {
        return UNDEFINED_FUNCTION;
    }
    match cpu.get_al() {
        0x00 => {
            cpu.set_bx(0x0100);
            OK
        }
        0x01 => interface(cpu),
        0x02 => {
            let end = cpu.bus.ram().len() as u32;
            cpu.set_edx((end - 1) & !(PAGE - 1));
            OK
        }
        0x03 => {
            cpu.set_edx(free_pages(&cpu.bus));
            OK
        }
        0x04 => allocate(cpu),
        0x05 => free(cpu),
        0x06 => {
            let page = cpu.cx() as u32;
            if page >= 0x100 {
                return PHYSICAL_PAGE;
            }
            cpu.set_edx(page * PAGE);
            OK
        }
        0x07 => {
            let cr0 = crate::v86mon::cr0(cpu);
            cpu.set_ebx(cr0);
            OK
        }
        0x08 => {
            let at = cpu.real_linear(cpu.es(), cpu.di());
            for n in 0..8 {
                let value = cpu.dr[n];
                cpu.bus.guest_write_32(at + 4 * n as u32, value);
            }
            OK
        }
        0x09 => {
            let at = cpu.real_linear(cpu.es(), cpu.di());
            for n in [0, 1, 2, 3, 6, 7] {
                cpu.dr[n] = cpu.bus.guest_read_32(at + 4 * n as u32);
            }
            OK
        }
        0x0A => {
            let (master, slave) = cpu.bus.vcpi.pic;
            cpu.set_bx(master as u16);
            cpu.set_cx(slave as u16);
            OK
        }
        0x0B => {
            cpu.bus.vcpi.pic = (cpu.bx() as u8, cpu.cx() as u8);
            OK
        }
        0x0C => {
            // The INT 67h's frame (or that of a far call with the flags
            // pushed, from a hook chained to it): the call doesn't return.
            let sp = cpu.sp().wrapping_add(6);
            cpu.set_sp(sp);
            switch_to_pm(cpu);
            OK
        }
        _ => SUBFUNCTION,
    }
}

/// DE01h: the client's page table 0 at ES:DI, mapping the first megabyte
/// and the HMA where they are, and the monitor's three descriptors at
/// DS:SI: its code (the ROM, 32-bit) and two flat data segments. DI ends
/// past the entries, EBX has the entry point's offset.
fn interface(cpu: &mut Cpu) -> u8 {
    let table = cpu.real_linear(cpu.es(), cpu.di());
    for page in 0..MAPPED_PAGES {
        cpu.bus
            .guest_write_32(table + 4 * page, (page * PAGE) | PTE_BITS);
    }
    cpu.set_di(cpu.di().wrapping_add((MAPPED_PAGES * 4) as u16));
    let descriptors = [
        crate::dpmi::descriptor(0xF0000, 0xFFFF, 0x9B, 0x4),
        crate::dpmi::descriptor(0, 0xFFFF_FFFF, 0x93, 0x4),
        crate::dpmi::descriptor(0, 0xFFFF_FFFF, 0x93, 0x4),
    ];
    let at = cpu.real_linear(cpu.ds(), cpu.si());
    for (i, d) in descriptors.iter().enumerate() {
        cpu.bus.guest_write_32(at + 8 * i as u32, *d as u32);
        cpu.bus
            .guest_write_32(at + 8 * i as u32 + 4, (*d >> 32) as u32);
    }
    cpu.set_ebx(crate::v86mon::VCPI_ENTRY as u32);
    OK
}

/// The 4 KB pages free: those the pool's 16 KB pages hold, and those
/// left of the ones taken.
fn free_pages(bus: &Bus) -> u32 {
    let end = bus.ram().len() as u32;
    bus.xms.free_pages(end) * 4 + bus.vcpi.spare.len() as u32
}

/// DE04h: a 4 KB page, its physical address in EDX.
fn allocate(cpu: &mut Cpu) -> u8 {
    if cpu.bus.vcpi.spare.is_empty() {
        let end = cpu.bus.ram().len() as u32;
        let Some(block) = cpu.bus.xms.take_page(end) else {
            return NOT_ENOUGH_PAGES;
        };
        cpu.bus
            .vcpi
            .spare
            .extend((0..4).rev().map(|i| block + i * PAGE));
    }
    let page = cpu.bus.vcpi.spare.pop().expect("a spare page");
    cpu.bus.fill_ram(page as usize..(page + PAGE) as usize, 0);
    cpu.bus.vcpi.pages.push(page);
    cpu.set_edx(page);
    OK
}

/// DE05h: free the page at EDX.
fn free(cpu: &mut Cpu) -> u8 {
    let page = cpu.edx() & !(PAGE - 1);
    let vcpi = &mut cpu.bus.vcpi;
    let Some(i) = vcpi.pages.iter().position(|&p| p == page) else {
        return BAD_PAGE;
    };
    vcpi.pages.swap_remove(i);
    vcpi.spare.push(page);
    OK
}

/// No program runs any more: its pages went back to the pool with the
/// expanded memory's (`Xms::program_ended`).
pub fn program_ended(bus: &mut Bus) {
    let pic = bus.vcpi.pic;
    bus.vcpi = Vcpi {
        pic,
        ..Vcpi::default()
    };
}

/// DE0Ch from virtual-8086 mode: on to the client's protected mode, with
/// the structure at the linear address ESI: CR3, the linear addresses of
/// its GDTR's and IDTR's values, its LDTR and TR, and CS:EIP. Interrupts
/// stay off; SS:ESP is a stack of the monitor's, the caller's own stack
/// below its top (16 bytes are all the client may use), and DS, ES, FS
/// and GS are null.
fn switch_to_pm(cpu: &mut Cpu) {
    let at = cpu.esi();
    let read32 = |cpu: &mut Cpu, a: u32| cpu.bus.guest_read_32(a);
    let read16 = |cpu: &mut Cpu, a: u32| cpu.bus.guest_read_16(a);
    let cr3 = read32(&mut *cpu, at);
    let (gdt_at, idt_at) = (read32(&mut *cpu, at + 4), read32(&mut *cpu, at + 8));
    let (ldt, tr) = (read16(&mut *cpu, at + 0x0C), read16(&mut *cpu, at + 0x0E));
    let (eip, cs) = (read32(&mut *cpu, at + 0x10), read16(&mut *cpu, at + 0x14));
    let gdtr = DescTable {
        base: read32(&mut *cpu, gdt_at + 2),
        limit: read16(&mut *cpu, gdt_at),
    };
    let idtr = DescTable {
        base: read32(&mut *cpu, idt_at + 2),
        limit: read16(&mut *cpu, idt_at),
    };
    let stack = cpu.real_linear(cpu.ss(), cpu.sp());

    let a20 = cpu.bus.a20();
    cpu.bus.vcpi.v86_a20 = Some(a20);
    cpu.bus.set_a20(true);
    cpu.set_cpu_flag(
        CpuFlags::VM | CpuFlags::NT | CpuFlags::RF | CpuFlags::IF | CpuFlags::TF,
        false,
    );
    cpu.cpl = 0;
    cpu.cr3 = cr3;
    cpu.cr0 |= CR0_PG;
    cpu.tlb.flush();
    cpu.gdtr = gdtr;
    cpu.idtr = idtr;
    cpu.note_mode_switch(true);
    if let Err(what) = load_client(cpu, ldt, tr, cs, eip) {
        cpu.bus
            .log_string(&format!("[VCPI] Can't switch to protected mode: {what}"));
        crate::video::print_string(
            cpu,
            "\r\nVCPI: the program's switch to protected mode failed.\r\n",
        );
        cpu.state = CpuState::RebootShell;
        return;
    }
    cpu.set_seg_cache(
        Seg::SS,
        SegCache::from_descriptor(crate::v86mon::DATA0, 0, 0xFFFF_FFFF, 0xC093),
    );
    cpu.set_esp(stack);
    for seg in [Seg::DS, Seg::ES, Seg::FS, Seg::GS] {
        cpu.set_seg_cache(seg, SegCache::null(0));
    }
}

/// Load the client's LDTR, TR (its TSS marked busy) and CS:EIP from its
/// GDT, which is loaded.
fn load_client(cpu: &mut Cpu, ldt: u16, tr: u16, cs: u16, eip: u32) -> Result<(), String> {
    let fault = |what: &str, f| format!("{what}: {f:?}");
    cpu.ldtr = if ldt & !3 == 0 {
        SegCache::null(ldt)
    } else {
        cpu.fetch_descriptor(ldt, 0)
            .map_err(|f| fault("LDT", f))?
            .cache(ldt)
    };
    if tr & !3 != 0 {
        let mut desc = cpu.fetch_descriptor(tr, 0).map_err(|f| fault("TSS", f))?;
        desc.0 |= 2 << 40;
        let addr = cpu.gdtr.base.wrapping_add((tr & 0xFFF8) as u32 + 5);
        cpu.sys_write_u8(addr, (desc.0 >> 40) as u8)
            .map_err(|f| fault("TSS", f))?;
        cpu.tr = desc.cache(tr);
    }
    let mut desc = cpu.fetch_descriptor(cs, 0).map_err(|f| fault("CS", f))?;
    if !desc.is_code() {
        return Err(format!("CS {cs:04X} isn't a code segment"));
    }
    cpu.load_cs_pm(cs, &mut desc, 0)
        .map_err(|f| fault("CS", f))?;
    cpu.set_eip(eip);
    Ok(())
}

/// A far call of the protected-mode entry point, AX the function: DE03h,
/// DE04h, DE05h, or DE0Ch, the switch back to virtual-8086 mode.
pub fn pm_entry(cpu: &mut Cpu) {
    let status = match cpu.ax() {
        0xDE03 => {
            cpu.set_edx(free_pages(&cpu.bus));
            OK
        }
        0xDE04 => allocate(cpu),
        0xDE05 => free(cpu),
        0xDE0C => {
            switch_to_v86(cpu);
            return;
        }
        _ => SUBFUNCTION,
    };
    cpu.set_reg8(iced_x86::Register::AH, status);
}

/// DE0Ch from protected mode: on in virtual-8086 mode under the monitor,
/// with the registers the client's stack has after the far call's return
/// address: EIP, CS, (EFLAGS), ESP, SS, ES, DS, FS and GS. Interrupts
/// stay off; DOS gets its A20 gate back.
fn switch_to_v86(cpu: &mut Cpu) {
    let top = cpu.stack_ptr();
    let mut word = |i: u32| {
        cpu.read_u32(Seg::SS, top.wrapping_add(8 + 4 * i))
            .unwrap_or(0)
    };
    let (eip, cs, esp, ss) = (word(0), word(1), word(3), word(4));
    let (es, ds, fs, gs) = (word(5), word(6), word(7), word(8));
    if let Some(a20) = cpu.bus.vcpi.v86_a20.take() {
        cpu.bus.set_a20(a20);
    }
    cpu.set_cpu_flag(CpuFlags::IF | CpuFlags::TF, false);
    // In `Seg::ALL`'s order: ES, CS, SS, DS, FS, GS.
    crate::v86mon::enter_with(cpu, [es, cs, ss, ds, fs, gs].map(|s| s as u16));
    cpu.set_eip(eip & 0xFFFF);
    cpu.set_esp(esp);
    cpu.note_mode_switch(false);
}

crate::state_fields!(Vcpi {
    pages,
    spare,
    pic,
    v86_a20
});
