//! A DPMI 0.9 host: the DOS Protected Mode Interface that memory managers
//! and Windows provide for DOS extenders. Programs find it with INT 2Fh
//! AX=1687h and far-call its entry point to become its clients: they then
//! run in protected mode at privilege level 3 (with IOPL 3, so CLI, STI and
//! port I/O work as they do for a DOS program), and ask the host for
//! descriptors, memory and real-mode services through INT 31h (int31.rs).
//! DOS/4GW, PMODE/W, DOS/32A and CWSDPMI's programs use a host they find
//! instead of switching the processor themselves, so a DOS/4GW program can
//! start a PMODE/W one and both have memory.
//!
//! The host is the emulator's: its code is traps (`FE 3B nn`) in the BIOS
//! ROM at F000:2000, which land in `service`, and its tables are real ones
//! the processor uses, in extended memory taken from the XMS pool (xms.rs):
//! a GDT, an IDT whose gates lead every vector to a trap at level 0, a TSS
//! with the level 0 stack, and each client's LDT and locked stack. It
//! switches modes by setting the processor's state: into protected mode by
//! an IRET from level 0 (`resume`), which checks the client's selectors as
//! the processor would, and back to real mode with 64 KB segments
//! (`enter_rm`).
//!
//! Interrupts in protected mode go to the client's handler (INT 31h
//! AX=0205h), or else down to real mode through the vector table
//! (`reflect`), as INT 31h AX=0300h calls real-mode code for the client.
//! Hardware interrupts that come while the processor is in real mode reach
//! a protected-mode handler through the host's stubs in the vector table
//! (`hook`). Clients nest: a DOS/4GW program's child becomes a client of
//! its own while the parent waits in real mode for EXEC to return, and
//! the host forgets a client when DOS ends its process (`process_ended`).
//! DOS calls a client makes with INT 21h in protected mode reach DOS
//! through the host's translation of their pointers (dos.rs), as under
//! Windows.

mod dos;
mod int31;

use crate::bus::Bus;
use crate::cpu::{CR0_PE, Cpu, CpuFlags, CpuModel, CpuResult, CpuState, DescTable, Seg, SegCache};

/// The host's code in the BIOS ROM: segment F000, which its protected-mode
/// code segments have as their base too.
const ROM_SEG: u16 = 0xF000;
const ROM_BASE: u32 = 0xF0000;

/// Real-mode entry points (offsets in F000).
/// The mode switch (INT 2Fh AX=1687h's ES:DI), far-called.
pub const ENTRY: u16 = 0x2000;
/// Where real-mode code the host called for a client returns to, by IRET
/// or RETF.
const RM_RETURN: u16 = 0x2004;
/// The raw switch to protected mode (INT 31h AX=0306h), jumped to.
const RAW_TO_PM: u16 = 0x2008;
/// Saving and restoring the state (INT 31h AX=0305h), far-called.
const SAVE_RM: u16 = 0x200C;
/// Ending a client's program: MOV AX,4CFFh; INT 21h.
const TERMINATE: u16 = 0x2010;
/// Protected-mode entry points at level 3 (offsets in `HOST_CODE3`).
/// Where a handler called for an interrupt that came in real mode returns
/// to (IRET).
const RET_HWINT: u16 = 0x2020;
/// Where a real-mode callback's procedure returns to (IRET).
const RET_CALLBACK: u16 = 0x2024;
/// Where an exception handler returns to (RETF).
const RET_EXCEPTION: u16 = 0x2028;
/// The raw switch to real mode (INT 31h AX=0306h), jumped to.
const RAW_TO_RM: u16 = 0x202C;
/// Saving and restoring the state (INT 31h AX=0305h), far-called.
const SAVE_PM: u16 = 0x2030;
/// The "MS-DOS" extensions' entry point (INT 2Fh AX=168Ah), far-called.
const MSDOS_API: u16 = 0x2034;
/// Per vector: the host's handler in the real-mode vector table for the
/// interrupts it takes to protected mode (`hook`).
const RM_INT: u16 = 0x2400;
/// Per slot: the real-mode callbacks (INT 31h AX=0303h).
const CALLBACKS: u16 = 0x2800;
pub const CALLBACK_SLOTS: usize = 64;
/// Per vector: where the IDT's gates lead, at level 0.
const PM_IDT: u16 = 0x2C00;
/// Per vector: the default protected-mode interrupt handlers (INT 31h
/// AX=0204h), which reflect the interrupt to real mode.
const PM_DEFAULT_INT: u16 = 0x3000;
/// Per exception: the default exception handlers (INT 31h AX=0202h).
const PM_DEFAULT_EXC: u16 = 0x3400;
const EXCEPTIONS: usize = 32;

/// The traps (`FE 3B nn`), by nn.
const T_ENTRY: u8 = 0x01;
const T_RM_RETURN: u8 = 0x02;
const T_RAW_TO_PM: u8 = 0x03;
const T_SAVE_RM: u8 = 0x04;
const T_RM_INT: u8 = 0x05;
const T_CALLBACK: u8 = 0x06;
const T_IDT: u8 = 0x10;
const T_DEFAULT_INT: u8 = 0x11;
const T_DEFAULT_EXC: u8 = 0x12;
const T_RET_HWINT: u8 = 0x13;
const T_RET_CALLBACK: u8 = 0x14;
const T_RET_EXCEPTION: u8 = 0x15;
const T_RAW_TO_RM: u8 = 0x16;
const T_SAVE_PM: u8 = 0x17;
const T_MSDOS_API: u8 = 0x18;

/// The host's GDT: its code at level 0 (the IDT's gates lead there), a
/// flat data segment for its stack, the TSS, the running client's LDT, its
/// code at level 3 (the entry points clients call and return to), and the
/// BIOS data area at selector 0040h, which programs load as they would
/// segment 0040h (DOS/4GW reads the timer ticks through it), as Windows
/// has it.
const HOST_CODE0: u16 = 0x08;
const HOST_DATA0: u16 = 0x10;
const HOST_TSS: u16 = 0x18;
const HOST_LDT: u16 = 0x20;
pub const HOST_CODE3: u16 = 0x2B;
const BIOS_DATA: u16 = 0x40;
const GDT_ENTRIES: u32 = 9;

/// The host's block of memory: GDT, IDT, TSS and the level 0 stack.
const HOST_GDT: u32 = 0x0000;
const HOST_IDT: u32 = 0x0100;
const TSS_OFFSET: u32 = 0x0900;
const HOST_STACK_TOP: u32 = 0x2000;
const HOST_BLOCK: u32 = 0x2000;

/// A client's block: its LDT, then its locked stack, on which the host
/// calls its handlers for interrupts from real mode, its real-mode
/// callbacks and its exception handlers.
pub const LDT_ENTRIES: usize = 8192;
const LDT_SIZE: u32 = LDT_ENTRIES as u32 * 8;
const LPMS_SIZE: u32 = 0x4000;
const CLIENT_BLOCK: u32 = LDT_SIZE + LPMS_SIZE;
/// LDT entries INT 31h AX=000Dh hands out by number; AX=0000h allocates
/// above them.
const RESERVED_LDT: usize = 16;
/// The paragraphs of conventional memory a client gives the host (INT
/// 2Fh AX=1687h's SI): the transfer buffer of its DOS calls (dos.rs), and
/// above it the real-mode stack the host calls real-mode code on for it.
const PRIVATE_PARAS: u16 = 0x0300;

/// What an LDT entry is used for.
const LDT_FREE: u8 = 0;
/// A client's descriptor.
const LDT_CLIENT: u8 = 1;
/// One the client may not free: the host's own, and those of INT 31h
/// AX=0002h.
const LDT_FIXED: u8 = 2;

/// The general-purpose registers' places in `Context::gpr`.
const EAX: usize = 0;
const ECX: usize = 1;
const EDX: usize = 2;
const EBX: usize = 3;
const ESP: usize = 4;
const ESI: usize = 6;
const EDI: usize = 7;

/// The flags an interrupt handler hands back to its caller from real mode:
/// CF, PF, AF, ZF, SF and OF.
const RESULT_FLAGS: u32 = 0x08D5;
const IOPL3: u32 = 0x3000;

/// A processor context: the registers, and the segment registers'
/// selectors (or real-mode segments).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Context {
    pub gpr: [u32; 8],
    pub eip: u32,
    pub eflags: u32,
    /// ES, CS, SS, DS, FS, GS, as `Seg` numbers them.
    pub seg: [u16; 6],
}

impl Context {
    /// The processor's context as it is.
    fn of(cpu: &Cpu) -> Self {
        Context {
            gpr: cpu.snapshot().gpr,
            eip: cpu.eip(),
            eflags: cpu.eflags_image(),
            seg: Seg::ALL.map(|seg| cpu.seg_cache(seg).selector),
        }
    }

    fn sel(&self, seg: Seg) -> u16 {
        self.seg[seg as usize]
    }

    fn set_sel(&mut self, seg: Seg, value: u16) {
        self.seg[seg as usize] = value;
    }

    fn reg16(&self, i: usize) -> u16 {
        self.gpr[i] as u16
    }

    fn set_reg16(&mut self, i: usize, value: u16) {
        self.gpr[i] = (self.gpr[i] & 0xFFFF_0000) | value as u32;
    }

    fn set_cf(&mut self, on: bool) {
        self.eflags = if on { self.eflags | 1 } else { self.eflags & !1 };
    }
}

/// A client: a program in protected mode, with its LDT and locked stack
/// (`block`), handlers, memory and real-mode callbacks.
#[derive(Clone, Debug, Default)]
pub struct Client {
    id: u32,
    /// The process it runs in; DOS ending it ends the client.
    psp: u16,
    /// A 32-bit client, whose handlers get 32-bit frames.
    bits32: bool,
    block: u32,
    /// What each LDT entry is used for (`LDT_FREE`, ...).
    ldt: Vec<u8>,
    /// The selectors INT 31h AX=0002h made, by real-mode segment.
    segments: Vec<(u16, u16)>,
    /// The protected-mode interrupt vectors (selector, offset).
    vectors: Vec<(u16, u32)>,
    /// The exception handlers (selector, offset).
    exceptions: Vec<(u16, u32)>,
    /// Memory blocks (INT 31h AX=0501h): (handle, address, bytes).
    memory: Vec<(u32, u32, u32)>,
    /// DOS memory blocks (INT 31h AX=0100h): (segment, first selector,
    /// selectors).
    dos_blocks: Vec<(u16, u16, u16)>,
    /// The real-mode stack in the client's private data (its segment), and
    /// where the host starts using it: its top, or below real-mode code
    /// running on it that went to protected mode.
    rm_stack: u16,
    rm_sp: u32,
    /// The locked stack's selector, and where the host starts using it.
    lpms_sel: u16,
    lpms_esp: u32,
    /// The selector of the real-mode stack a callback's procedure gets in
    /// DS.
    cb_sel: u16,
    /// ES, DS, FS and GS as the client last had them in protected mode,
    /// which the host gives the code it calls there from real mode (and a
    /// raw switch its FS and GS): extenders such as Tran's PMODE keep a
    /// selector in GS all along.
    segs: [u16; 4],
    /// The environment's segment, which PSP:2Ch holds as a selector while
    /// the client runs.
    env: u16,
    /// The DTA the client set in protected mode (INT 21h AH=1Ah), if it
    /// did: DOS's own is somewhere else.
    dta: Option<(u16, u32)>,
    /// The selector of its LDT the "MS-DOS" extensions gave it, or 0.
    ldt_alias: u16,
    /// Memory blocks of INT 21h AH=48h: the first of their selectors, how
    /// many, and their handle in `memory`.
    blocks: Vec<(u16, u16, u32)>,
}

impl Client {
    fn selector(index: usize) -> u16 {
        (index as u16) << 3 | 7
    }

    /// The LDT index of a selector of the client's that is in use.
    fn index(&self, selector: u16) -> Option<usize> {
        let index = (selector >> 3) as usize;
        (selector & 4 != 0 && self.ldt.get(index).is_some_and(|&use_| use_ != LDT_FREE)).then_some(index)
    }

    /// `count` free LDT entries in a row, from `RESERVED_LDT` up.
    fn free_entries(&self, count: usize) -> Option<usize> {
        if count == 0 {
            return None;
        }
        let mut run = 0;
        for i in RESERVED_LDT..LDT_ENTRIES {
            run = if self.ldt[i] == LDT_FREE { run + 1 } else { 0 };
            if run == count {
                return Some(i + 1 - count);
            }
        }
        None
    }

    fn default_vector(vector: u8) -> (u16, u32) {
        (HOST_CODE3, (PM_DEFAULT_INT + 4 * vector as u16) as u32)
    }

    fn default_exception(exception: u8) -> (u16, u32) {
        (HOST_CODE3, (PM_DEFAULT_EXC + 4 * exception as u16) as u32)
    }

    /// The client's handler of `vector`, if it has one of its own.
    fn handler(&self, vector: u8) -> Option<(u16, u32)> {
        let handler = self.vectors[vector as usize];
        (handler != Self::default_vector(vector)).then_some(handler)
    }

    fn lpms_base(&self) -> u32 {
        self.block + LDT_SIZE
    }

    fn frame_size(&self) -> u32 {
        if self.bits32 { 4 } else { 2 }
    }
}

/// A real-mode callback (INT 31h AX=0303h): the client's procedure and the
/// real-mode call structure it gets.
#[derive(Clone, Copy, Debug, Default)]
pub struct Callback {
    client: u32,
    proc_sel: u16,
    proc_off: u32,
    struct_sel: u16,
    struct_off: u32,
}

/// What the host is in the middle of, waiting for code in the other mode
/// to come back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    kind: u8,
    client: u32,
    /// The context to go on with afterwards.
    ctx: Context,
    vector: u8,
    /// `F_TRANSLATE`: the real-mode call structure (selector, offset).
    /// `F_CALLBACK`: the slot. `F_DOS`: `dos::DIRECT` or
    /// `dos::SWAPPED_DTA`, and the bytes read or written so far.
    sel: u16,
    off: u32,
    /// The client's `rm_sp`, `lpms_esp` and callback stack base before;
    /// for `F_DOS`, `base` is DOS's DTA before a search (segment:offset).
    rm_sp: u32,
    lpms: u32,
    base: u32,
}

/// Frame kinds. In real mode: an interrupt reflected there, a real-mode
/// call for INT 31h AX=0300h-0302h, and a DOS call translated (dos.rs). In
/// protected mode: a handler called for an interrupt in real mode, and a
/// callback's procedure.
const F_REFLECT: u8 = 0;
const F_TRANSLATE: u8 = 1;
const F_HWINT: u8 = 2;
const F_CALLBACK: u8 = 3;
const F_DOS: u8 = 4;

/// The host's state.
#[derive(Debug)]
pub struct Dpmi {
    /// Whether programs find the host (the `dpmi` setting).
    pub enabled: bool,
    /// The host's block (GDT, IDT, TSS, level 0 stack), while it has
    /// clients.
    host: Option<u32>,
    /// The clients, the one running last.
    clients: Vec<Client>,
    frames: Vec<Frame>,
    callbacks: Vec<Option<Callback>>,
    /// Real-mode vectors pointing at the host's stubs, and what they held.
    hooks: Vec<(u8, u32)>,
    /// The A20 gate before the first client opened it.
    a20: bool,
    next_id: u32,
    next_handle: u32,
}

impl Default for Dpmi {
    fn default() -> Self {
        Dpmi {
            enabled: true,
            host: None,
            clients: Vec::new(),
            frames: Vec::new(),
            callbacks: vec![None; CALLBACK_SLOTS],
            hooks: Vec::new(),
            a20: false,
            next_id: 1,
            next_handle: 1,
        }
    }
}

impl Dpmi {
    /// What the real-mode vector `vector` held before the host's stub took
    /// it over for a client's handler, if it did: where the interrupt goes
    /// on when the client passes it down.
    pub(crate) fn hooked_original_mut(&mut self, vector: u8) -> Option<&mut u32> {
        self.hooks.iter_mut().find(|(v, _)| *v == vector).map(|(_, original)| original)
    }

    /// No clients: as after a program ends. The setting stays.
    pub fn reset(&mut self) {
        *self = Dpmi { enabled: self.enabled, ..Dpmi::default() };
    }

    /// Whether a client is running.
    pub fn active(&self) -> bool {
        !self.clients.is_empty()
    }

    /// The clients, as (PSP, 32-bit), the running one last, for debuggers.
    pub fn clients(&self) -> Vec<(u16, bool)> {
        self.clients.iter().map(|c| (c.psp, c.bits32)).collect()
    }

    /// The extended memory the host holds, as (address, bytes): for the
    /// XMS pool after a state is loaded.
    pub fn reservations(&self) -> Vec<(u32, u32)> {
        let mut held: Vec<(u32, u32)> = self.host.map(|base| (base, HOST_BLOCK)).into_iter().collect();
        for client in &self.clients {
            held.push((client.block, CLIENT_BLOCK));
            held.extend(client.memory.iter().map(|&(_, base, len)| (base, len)));
        }
        held
    }
}

/// Write the host's code into the BIOS ROM: every entry point is a trap,
/// but TERMINATE's.
pub fn install_rom(bus: &mut Bus) {
    let at = |offset: u16| ROM_BASE as usize + offset as usize;
    if bus.read_8(at(PM_DEFAULT_EXC + 4 * (EXCEPTIONS as u16 - 1)) + 2) == T_DEFAULT_EXC
        && bus.read_8(at(MSDOS_API) + 2) == T_MSDOS_API
    {
        return;
    }
    let trap = |kind: u8| [0xFE, 0x3B, kind, 0x90];
    for (offset, kind) in [
        (ENTRY, T_ENTRY),
        (RM_RETURN, T_RM_RETURN),
        (RAW_TO_PM, T_RAW_TO_PM),
        (SAVE_RM, T_SAVE_RM),
        (RET_HWINT, T_RET_HWINT),
        (RET_CALLBACK, T_RET_CALLBACK),
        (RET_EXCEPTION, T_RET_EXCEPTION),
        (RAW_TO_RM, T_RAW_TO_RM),
        (SAVE_PM, T_SAVE_PM),
        (MSDOS_API, T_MSDOS_API),
    ] {
        bus.write_rom(at(offset), &trap(kind));
    }
    // MOV AX,4CFFh; INT 21h; and should DOS come back, again.
    bus.write_rom(at(TERMINATE), &[0xB8, 0xFF, 0x4C, 0xCD, 0x21, 0xEB, 0xF9]);
    let table = |count: usize, kind: u8| (0..count).flat_map(|_| trap(kind)).collect::<Vec<u8>>();
    bus.write_rom(at(RM_INT), &table(256, T_RM_INT));
    bus.write_rom(at(CALLBACKS), &table(CALLBACK_SLOTS, T_CALLBACK));
    bus.write_rom(at(PM_IDT), &table(256, T_IDT));
    bus.write_rom(at(PM_DEFAULT_INT), &table(256, T_DEFAULT_INT));
    bus.write_rom(at(PM_DEFAULT_EXC), &table(EXCEPTIONS, T_DEFAULT_EXC));
}

/// INT 2Fh AX=1687h: the host is there, unless it is off or a V86 monitor
/// (Windows' 386 enhanced mode) or a booted system runs, which have their
/// own. Returns whether it answered.
pub fn installation_check(cpu: &mut Cpu) -> bool {
    if !cpu.bus.dpmi.enabled || cpu.v86() || cpu.bus.boot.is_some() {
        return false;
    }
    // (Again, for a state saved before the host was in the ROM.)
    install_rom(&mut cpu.bus);
    cpu.set_ax(0);
    // 32-bit programs are supported.
    cpu.set_bx(1);
    cpu.set_cx((cpu.cx() & 0xFF00) | cpu.model.family() as u16);
    // Version 0.90.
    cpu.set_dx(0x005A);
    cpu.set_si(PRIVATE_PARAS);
    cpu.set_es(ROM_SEG);
    cpu.set_di(ENTRY);
    true
}

/// Whether the trap `kind` at the processor's CS:EIP is one the host takes
/// in the mode the processor is in; otherwise it is an invalid opcode. Its
/// real-mode stubs that a vector may still reach after the last client
/// (behind another program's hook) return, as do its callbacks and its
/// return address.
pub fn accepts(cpu: &Cpu, kind: u8) -> bool {
    let active = cpu.bus.dpmi.active();
    match kind {
        T_ENTRY | T_RM_RETURN | T_RM_INT | T_CALLBACK => !cpu.pe(),
        T_RAW_TO_PM | T_SAVE_RM => !cpu.pe() && active,
        T_IDT => cpu.pm() && cpu.cpl == 0 && active,
        T_DEFAULT_INT | T_DEFAULT_EXC | T_RET_HWINT | T_RET_CALLBACK | T_RET_EXCEPTION | T_RAW_TO_RM
        | T_SAVE_PM | T_MSDOS_API => cpu.pm() && cpu.cpl == 3 && active,
        _ => false,
    }
}

/// Run the host's trap `kind` at CS:EIP (see `accepts`).
pub fn service(cpu: &mut Cpu, kind: u8) {
    let slot = |base: u16| (cpu.eip() as u16).wrapping_sub(base) as usize / 4;
    match kind {
        T_ENTRY => enter(cpu),
        T_RM_RETURN => rm_return(cpu),
        T_RAW_TO_PM => raw_to_pm(cpu),
        T_SAVE_RM => {
            let ip = cpu.pop();
            let cs = cpu.pop();
            cpu.set_cs(cs);
            cpu.set_ip(ip);
        }
        T_RM_INT => {
            let vector = slot(RM_INT) as u8;
            rm_interrupt(cpu, vector);
        }
        T_CALLBACK => {
            let index = slot(CALLBACKS);
            callback(cpu, index);
        }
        T_IDT => {
            let vector = slot(PM_IDT) as u8;
            idt(cpu, vector);
        }
        T_DEFAULT_INT => {
            let vector = slot(PM_DEFAULT_INT) as u8;
            default_handler(cpu, vector);
        }
        T_DEFAULT_EXC => {
            let exception = slot(PM_DEFAULT_EXC) as u8;
            default_exception_handler(cpu, exception);
        }
        T_RET_HWINT => ret_hwint(cpu),
        T_RET_CALLBACK => ret_callback(cpu),
        T_RET_EXCEPTION => ret_exception(cpu),
        T_RAW_TO_RM => raw_to_rm(cpu),
        T_SAVE_PM => {
            let size = frame_size(cpu);
            if let Err(fault) = cpu.ret_far_pm(size as u8, 0) {
                cpu.raise(fault);
            }
        }
        T_MSDOS_API => msdos_api(cpu),
        _ => {}
    }
}

fn frame_size(cpu: &Cpu) -> u32 {
    cpu.bus.dpmi.clients.last().map_or(2, Client::frame_size)
}

fn client(cpu: &mut Cpu) -> &mut Client {
    cpu.bus.dpmi.clients.last_mut().expect("a DPMI client")
}

/// An 8-byte segment descriptor.
fn descriptor(base: u32, limit: u32, access: u8, flags: u8) -> u64 {
    let (limit, flags) = if limit > 0xF_FFFF { (limit >> 12, flags | 0x8) } else { (limit, flags & !0x8) };
    (limit as u64 & 0xFFFF)
        | ((base as u64 & 0xFF_FFFF) << 16)
        | ((access as u64) << 40)
        | (((limit as u64 >> 16) & 0xF) << 48)
        | ((flags as u64 & 0xF) << 52)
        | (((base as u64 >> 24) & 0xFF) << 56)
}

/// Access rights of the client's data and code segments: present, level 3.
const DATA3: u8 = 0xF2;
const CODE3: u8 = 0xFA;
/// The descriptor flags nibble: D/B.
const BIG: u8 = 0x4;

/// The descriptor `index` of the LDT at `ldt`.
fn read_desc(bus: &Bus, ldt: u32, index: usize) -> u64 {
    let at = ldt as usize + index * 8;
    bus.read_32(at) as u64 | (bus.read_32(at + 4) as u64) << 32
}

fn write_desc(bus: &mut Bus, ldt: u32, index: usize, desc: u64) {
    let at = ldt as usize + index * 8;
    bus.write_32(at, desc as u32);
    bus.write_32(at + 4, (desc >> 32) as u32);
}

/// The descriptor `selector` names in the host's tables: the running
/// client's LDT or the GDT.
fn descriptor_of(cpu: &Cpu, selector: u16) -> Option<u64> {
    let dpmi = &cpu.bus.dpmi;
    let index = (selector >> 3) as usize;
    if selector & 4 != 0 {
        let client = dpmi.clients.last()?;
        return Some(read_desc(&cpu.bus, client.block, client.index(selector)?));
    }
    if index == 0 || index >= GDT_ENTRIES as usize {
        return None;
    }
    let at = (dpmi.host? + HOST_GDT) as usize + index * 8;
    Some(cpu.bus.read_32(at) as u64 | (cpu.bus.read_32(at + 4) as u64) << 32)
}

/// The base address of the segment `selector` names (`descriptor_of`).
fn selector_base(cpu: &Cpu, selector: u16) -> Option<u32> {
    descriptor_of(cpu, selector).map(|desc| crate::cpu::Descriptor(desc).base())
}

/// Allocate `count` LDT entries in a row for the running client, each
/// `desc`. Returns the first index.
fn allocate(cpu: &mut Cpu, count: usize, desc: u64, use_: u8) -> Option<usize> {
    let client = client(cpu);
    let first = client.free_entries(count)?;
    for i in first..first + count {
        client.ldt[i] = use_;
    }
    let ldt = client.block;
    for i in first..first + count {
        write_desc(&mut cpu.bus, ldt, i, desc);
    }
    Some(first)
}

fn free_entry(cpu: &mut Cpu, index: usize) {
    let client = client(cpu);
    client.ldt[index] = LDT_FREE;
    let ldt = client.block;
    write_desc(&mut cpu.bus, ldt, index, 0);
}

fn read_ivt(bus: &Bus, vector: u8) -> u32 {
    bus.read_32(vector as usize * 4)
}

fn write_ivt(bus: &mut Bus, vector: u8, value: u32) {
    bus.write_32(vector as usize * 4, value);
}

fn rm_stub(vector: u8) -> u32 {
    (ROM_SEG as u32) << 16 | (RM_INT + 4 * vector as u16) as u32
}

/// The far address a real-mode interrupt reaches: the vector, or what it
/// held before the host's stub when it points there.
fn real_mode_handler(cpu: &Cpu, vector: u8) -> u32 {
    let ivt = read_ivt(&cpu.bus, vector);
    if ivt == rm_stub(vector)
        && let Some(&(_, original)) = cpu.bus.dpmi.hooks.iter().find(|&&(v, _)| v == vector)
    {
        return original;
    }
    ivt
}

/// The vectors of the hardware interrupts (as the PICs are programmed).
fn is_irq(cpu: &Cpu, vector: u8) -> bool {
    (0..16).any(|irq| cpu.bus.pic.vector(irq) == vector)
}

/// Interrupts a protected-mode handler gets when they come in real mode
/// too: the hardware interrupts, and the timer tick the BIOS calls.
fn goes_to_pm(cpu: &Cpu, vector: u8) -> bool {
    vector == 0x1C || is_irq(cpu, vector)
}

/// Point the real-mode vector at the host's stub, which calls the
/// protected-mode handler, unless it does already.
fn hook(cpu: &mut Cpu, vector: u8) {
    if cpu.bus.dpmi.hooks.iter().any(|&(v, _)| v == vector) {
        return;
    }
    let original = read_ivt(&cpu.bus, vector);
    cpu.bus.dpmi.hooks.push((vector, original));
    write_ivt(&mut cpu.bus, vector, rm_stub(vector));
}

/// Take the host's stub out of the real-mode vector again when no client
/// has a handler for it any more.
fn unhook(cpu: &mut Cpu, vector: u8) {
    let dpmi = &cpu.bus.dpmi;
    if dpmi.clients.iter().any(|c| c.handler(vector).is_some()) {
        return;
    }
    let Some(i) = dpmi.hooks.iter().position(|&(v, _)| v == vector) else { return };
    let (_, original) = cpu.bus.dpmi.hooks.remove(i);
    if read_ivt(&cpu.bus, vector) == rm_stub(vector) {
        write_ivt(&mut cpu.bus, vector, original);
    }
}

/// Set the running client's protected-mode handler of `vector`.
fn set_vector(cpu: &mut Cpu, vector: u8, handler: (u16, u32)) {
    client(cpu).vectors[vector as usize] = handler;
    if goes_to_pm(cpu, vector) {
        if handler != Client::default_vector(vector) {
            hook(cpu, vector);
        } else {
            unhook(cpu, vector);
        }
    }
}

/// Set up the host's tables for its first client.
fn start_host(cpu: &mut Cpu) -> Option<u32> {
    let end = cpu.bus.ram().len() as u32;
    let host = cpu.bus.xms.take_dpmi(HOST_BLOCK, end)?;
    cpu.bus.fill_ram(host as usize..(host + HOST_BLOCK) as usize, 0);
    let gdt = [
        (HOST_CODE0, descriptor(ROM_BASE, 0xFFFF, 0x9B, BIG)),
        (HOST_DATA0, descriptor(0, 0xFFFF_FFFF, 0x93, BIG)),
        (HOST_TSS, descriptor(host + TSS_OFFSET, 0x67, 0x8B, 0)),
        (HOST_LDT, descriptor(0, LDT_SIZE - 1, 0x82, 0)),
        (HOST_CODE3, descriptor(ROM_BASE, 0xFFFF, 0xFB, 0)),
        (BIOS_DATA, descriptor(0x400, 0xFFFF, 0xF3, 0)),
    ];
    for (selector, desc) in gdt {
        let at = (host + HOST_GDT) as usize + (selector & !7) as usize;
        cpu.bus.write_32(at, desc as u32);
        cpu.bus.write_32(at + 4, (desc >> 32) as u32);
    }
    // Every vector: a 32-bit interrupt gate that level 3 may use, to the
    // host's code at level 0.
    for vector in 0..256u32 {
        let offset = PM_IDT as u32 + 4 * vector;
        let gate = (offset as u64 & 0xFFFF) | (HOST_CODE0 as u64) << 16 | 0xEEu64 << 40 | ((offset as u64) >> 16) << 48;
        let at = (host + HOST_IDT + vector * 8) as usize;
        cpu.bus.write_32(at, gate as u32);
        cpu.bus.write_32(at + 4, (gate >> 32) as u32);
    }
    // The TSS: the level 0 stack, and no I/O permission bitmap.
    let tss = (host + TSS_OFFSET) as usize;
    cpu.bus.write_32(tss + 4, host + HOST_STACK_TOP);
    cpu.bus.write_32(tss + 8, HOST_DATA0 as u32);
    cpu.bus.write_16(tss + 0x66, 0x68);
    let a20 = cpu.bus.a20();
    let dpmi = &mut cpu.bus.dpmi;
    dpmi.host = Some(host);
    dpmi.a20 = a20;
    cpu.bus.set_a20(true);
    cpu.bus.log_string("[DPMI] Host started");
    Some(host)
}

/// The mode switch (INT 2Fh AX=1687h's entry point), far-called in real
/// mode with AX bit 0 set for a 32-bit client and ES its private data:
/// the caller goes on in protected mode as a new client, with selectors
/// for its CS, DS and SS, ES its PSP and the PSP's environment pointer a
/// selector too. CF says whether that failed.
fn enter(cpu: &mut Cpu) {
    let ip = cpu.pop();
    let cs = cpu.pop();
    let bits32 = cpu.ax() & 1 != 0;
    let fail = |cpu: &mut Cpu, error: u16| {
        cpu.set_ax(error);
        cpu.set_cpu_flag(CpuFlags::CF, true);
        cpu.set_cs(cs);
        cpu.set_ip(ip);
    };
    let end = cpu.bus.ram().len() as u32;
    if cpu.bus.dpmi.host.is_none() && start_host(cpu).is_none() {
        cpu.bus.log_string("[DPMI] No memory for the host");
        return fail(cpu, 0x8011);
    }
    let Some(block) = cpu.bus.xms.take_dpmi(CLIENT_BLOCK, end) else {
        cpu.bus.log_string("[DPMI] No memory for a client");
        if !cpu.bus.dpmi.active() {
            stop_host(cpu);
        }
        return fail(cpu, 0x8011);
    };
    cpu.bus.fill_ram(block as usize..(block + CLIENT_BLOCK) as usize, 0);
    let dpmi = &mut cpu.bus.dpmi;
    let id = dpmi.next_id;
    dpmi.next_id += 1;
    let psp = cpu.current_psp;
    let rm_stack = cpu.es();
    let dpmi = &mut cpu.bus.dpmi;
    dpmi.clients.push(Client {
        id,
        psp,
        bits32,
        block,
        ldt: vec![LDT_FREE; LDT_ENTRIES],
        segments: Vec::new(),
        vectors: (0..=255).map(Client::default_vector).collect(),
        exceptions: (0..EXCEPTIONS as u8).map(Client::default_exception).collect(),
        memory: Vec::new(),
        dos_blocks: Vec::new(),
        rm_stack,
        rm_sp: PRIVATE_PARAS as u32 * 16,
        lpms_sel: 0,
        lpms_esp: LPMS_SIZE,
        cb_sel: 0,
        segs: [0; 4],
        env: 0,
        dta: None,
        ldt_alias: 0,
        blocks: Vec::new(),
    });
    let real = |segment: u16, access: u8, limit: u32| descriptor((segment as u32) << 4, limit, access, 0);
    let (ds, ss) = (cpu.ds(), cpu.ss());
    let lpms = descriptor(block + LDT_SIZE, LPMS_SIZE - 1, DATA3, if bits32 { BIG } else { 0 });
    let lpms = allocate(cpu, 1, lpms, LDT_FIXED).unwrap();
    let cb = allocate(cpu, 1, real(0, DATA3, 0xFFFF), LDT_FIXED).unwrap();
    let code = allocate(cpu, 1, real(cs, CODE3, 0xFFFF), LDT_CLIENT).unwrap();
    let data = allocate(cpu, 1, real(ds, DATA3, 0xFFFF), LDT_CLIENT).unwrap();
    let stack = allocate(cpu, 1, real(ss, DATA3, 0xFFFF), LDT_CLIENT).unwrap();
    let psp_sel = allocate(cpu, 1, real(psp, DATA3, 0xFF), LDT_CLIENT).unwrap();
    let env = cpu.bus.read_16(psp as usize * 16 + 0x2C);
    if env != 0 {
        let mcb = crate::mcb::read_mcb(&mut cpu.bus, env.wrapping_sub(1));
        let limit = if mcb.is_valid() { (mcb.size as u32 * 16).clamp(1, 0x10000) - 1 } else { 0xFFFF };
        let env_sel = allocate(cpu, 1, real(env, DATA3, limit), LDT_CLIENT).unwrap();
        cpu.bus.write_16(psp as usize * 16 + 0x2C, Client::selector(env_sel));
    }
    let c = client(cpu);
    c.lpms_sel = Client::selector(lpms);
    c.cb_sel = Client::selector(cb);
    c.segs = [Client::selector(psp_sel), Client::selector(data), 0, 0];
    c.env = env;
    cpu.bus.log_string(&format!(
        "[DPMI] {}-bit client for PSP {:04X} entered protected mode at {:04X}:{:04X}",
        if bits32 { 32 } else { 16 },
        psp,
        cs,
        ip
    ));
    let mut ctx = Context::of(cpu);
    ctx.gpr[ESP] &= 0xFFFF;
    ctx.eip = ip as u32;
    ctx.set_sel(Seg::CS, Client::selector(code));
    ctx.set_sel(Seg::DS, Client::selector(data));
    ctx.set_sel(Seg::SS, Client::selector(stack));
    ctx.set_sel(Seg::ES, Client::selector(psp_sel));
    ctx.set_sel(Seg::FS, 0);
    ctx.set_sel(Seg::GS, 0);
    ctx.set_cf(false);
    resume(cpu, &ctx);
}

/// Put the processor at level 0 in the host's code, in protected mode with
/// the host's tables and the running client's LDT, on the level 0 stack.
fn host_ring0(cpu: &mut Cpu) {
    let host = cpu.bus.dpmi.host.expect("the DPMI host");
    let ldt = cpu.bus.dpmi.clients.last().map_or(0, |c| c.block);
    // The GDT's LDT descriptor is the running client's.
    let desc = descriptor(ldt, LDT_SIZE - 1, 0x82, 0);
    let at = (host + HOST_GDT) as usize + (HOST_LDT >> 3) as usize * 8;
    cpu.bus.write_32(at, desc as u32);
    cpu.bus.write_32(at + 4, (desc >> 32) as u32);
    if !cpu.pe() {
        cpu.cr0 |= CR0_PE;
        cpu.note_mode_switch(true);
    }
    cpu.gdtr = DescTable { base: host + HOST_GDT, limit: (GDT_ENTRIES * 8 - 1) as u16 };
    cpu.idtr = DescTable { base: host + HOST_IDT, limit: 0x7FF };
    cpu.ldtr = SegCache::from_descriptor(HOST_LDT, ldt, LDT_SIZE - 1, 0x82);
    cpu.tr = SegCache::from_descriptor(HOST_TSS, host + TSS_OFFSET, 0x67, 0x8B);
    cpu.set_seg_cache(Seg::CS, SegCache::from_descriptor(HOST_CODE0, ROM_BASE, 0xFFFF, 0x409B));
    cpu.set_seg_cache(Seg::SS, SegCache::from_descriptor(HOST_DATA0, 0, 0xFFFF_FFFF, 0xC093));
    cpu.cpl = 0;
    let flags = cpu.get_cpu_flags() - (CpuFlags::VM | CpuFlags::NT | CpuFlags::RF | CpuFlags::IF | CpuFlags::TF);
    cpu.load_eflags(flags.bits());
    cpu.set_esp(host + HOST_STACK_TOP);
}

/// Go on with the client's protected-mode context `ctx`, from wherever the
/// processor is, through an IRET from the host's level 0: a selector the
/// client left that doesn't do raises the exception the processor would.
fn resume(cpu: &mut Cpu, ctx: &Context) {
    host_ring0(cpu);
    let top = cpu.esp();
    let mut snapshot = cpu.snapshot();
    snapshot.gpr = ctx.gpr;
    cpu.restore(&snapshot);
    cpu.set_esp(top);
    // Data segments that can't be loaded (freed, or changed) are null.
    for seg in [Seg::ES, Seg::DS, Seg::FS, Seg::GS] {
        let cache = cpu.check_data_segment(seg, ctx.sel(seg)).unwrap_or(SegCache::null(0));
        cpu.set_seg_cache(seg, cache);
    }
    let flags = (ctx.eflags | IOPL3) & !(CpuFlags::VM | CpuFlags::NT | CpuFlags::RF).bits();
    let frame = [ctx.eip, ctx.sel(Seg::CS) as u32 | 3, flags, ctx.gpr[ESP], ctx.sel(Seg::SS) as u32 | 3];
    let sp = top - 4 * frame.len() as u32;
    for (i, value) in frame.iter().enumerate() {
        cpu.bus.write_32(sp as usize + 4 * i, *value);
    }
    cpu.set_esp(sp);
    if let Err(fault) = cpu.iret_pm(4) {
        cpu.bus.log_string(&format!(
            "[DPMI] Can't go on at {:04X}:{:08X} (stack {:04X}:{:08X}): {:?}",
            ctx.sel(Seg::CS),
            ctx.eip,
            ctx.sel(Seg::SS),
            ctx.gpr[ESP],
            fault
        ));
        abort(cpu, "invalid context");
    }
}

/// Switch to real mode with the context `ctx` (segments, not selectors),
/// with 64 KB segments and the real-mode vector table.
fn enter_rm(cpu: &mut Cpu, ctx: &Context) {
    if cpu.pe() {
        cpu.cr0 &= !CR0_PE;
        cpu.note_mode_switch(false);
    }
    cpu.cpl = 0;
    cpu.idtr = DescTable { base: 0, limit: 0x3FF };
    for seg in Seg::ALL {
        cpu.set_seg_cache(seg, SegCache::real(ctx.sel(seg)));
    }
    let mut snapshot = cpu.snapshot();
    snapshot.gpr = ctx.gpr;
    cpu.restore(&snapshot);
    cpu.set_eip(ctx.eip & 0xFFFF);
    cpu.set_cpu_flag(CpuFlags::VM, false);
    cpu.load_eflags(ctx.eflags & !(IOPL3 | CpuFlags::NT.bits()));
}

/// Return from a real-mode interrupt handler, as its IRET would.
fn iret_real(cpu: &mut Cpu) {
    let ip = cpu.pop();
    let cs = cpu.pop();
    let flags = cpu.pop();
    cpu.set_cs(cs);
    cpu.set_ip(ip);
    cpu.load_flags16(flags);
}

/// The client's context where it entered the IDT's gate: the frame on the
/// level 0 stack (and an error code, when there is one), the registers as
/// they are.
fn gate_context(cpu: &mut Cpu) -> Option<(Context, Option<u32>)> {
    let top = cpu.bus.dpmi.host? + HOST_STACK_TOP;
    let sp = cpu.esp();
    let (error, at) = match top.wrapping_sub(sp) {
        20 => (None, sp),
        24 => (Some(cpu.bus.read_32(sp as usize)), sp + 4),
        _ => return None,
    };
    let read = |i: u32| cpu.bus.read_32((at + 4 * i) as usize);
    let mut ctx = Context::of(cpu);
    ctx.eip = read(0);
    ctx.set_sel(Seg::CS, read(1) as u16);
    ctx.eflags = read(2);
    ctx.gpr[ESP] = read(3);
    ctx.set_sel(Seg::SS, read(4) as u16);
    cpu.set_esp(top);
    Some((ctx, error))
}

/// An interrupt or exception through the IDT.
fn idt(cpu: &mut Cpu, vector: u8) {
    let Some((mut ctx, error)) = gate_context(cpu) else {
        // Not from the client's level: the host's own IRET failed.
        cpu.bus.log_string(&format!("[DPMI] Exception {:02X}h in the host", vector));
        abort(cpu, "exception in the host");
        return;
    };
    // The vectors of exceptions 8-15 are the first PIC's too; exceptions
    // there push an error code (#DF, #TS, #NP, #SS, #GP, #PF).
    let exception = (vector < 8 && vector != 2) || ((8..16).contains(&vector) && error.is_some());
    if vector == 13 && error == Some(0) && emulate_mov_system(cpu, &mut ctx) {
        resume(cpu, &ctx);
    } else if exception {
        exception_entry(cpu, vector, error.unwrap_or(0), ctx);
    } else {
        interrupt(cpu, vector, ctx);
    }
}

/// A MOV to or from a control or debug register (0F 20h-23h), which level 3
/// may not run, in the client's context `ctx`: the host does it for the
/// client before its exception handler sees the #GP, as Windows does.
/// Clients read the control registers as they are and can't change them;
/// the debug registers are theirs to set (DOS/4GW Professional's NULLP
/// option sets DR0-DR3 and DR7 itself), with no breakpoints behind them.
/// Returns whether the instruction was one.
fn emulate_mov_system(cpu: &mut Cpu, ctx: &mut Context) -> bool {
    let Some(base) = selector_base(cpu, ctx.sel(Seg::CS)) else { return false };
    let byte = |i: u32| cpu.bus.read_8(base.wrapping_add(ctx.eip).wrapping_add(i) as usize);
    // Operand and address size prefixes change nothing.
    let mut at = 0;
    while at < 4 && matches!(byte(at), 0x66 | 0x67) {
        at += 1;
    }
    let (escape, opcode, modrm) = (byte(at), byte(at + 1), byte(at + 2));
    if escape != 0x0F || !(0x20..=0x23).contains(&opcode) {
        return false;
    }
    let (n, reg) = ((modrm >> 3 & 7) as usize, (modrm & 7) as usize);
    let pentium = cpu.model >= CpuModel::Pentium;
    match opcode {
        0x20 => {
            ctx.gpr[reg] = match n {
                0 => cpu.cr0,
                2 => cpu.cr2,
                3 => cpu.cr3,
                4 if pentium => cpu.cr4,
                _ => return false,
            }
        }
        0x22 if !matches!(n, 0 | 2 | 3) && !(n == 4 && pentium) => return false,
        0x22 => {}
        0x21 => ctx.gpr[reg] = cpu.dr[n],
        _ => cpu.dr[n] = ctx.gpr[reg],
    }
    ctx.eip = ctx.eip.wrapping_add(at + 3);
    true
}

/// Interrupt `vector` in the client's context `ctx`: to its handler, or
/// the default one's.
fn interrupt(cpu: &mut Cpu, vector: u8, ctx: Context) {
    match client(cpu).handler(vector) {
        Some(handler) => call_handler(cpu, ctx, handler),
        None => default_interrupt(cpu, vector, ctx),
    }
}

/// Go on with `ctx` in the interrupt handler `handler`, as an interrupt
/// gate at level 3 would: its IRET frame on the client's stack, interrupts
/// and tracing off.
fn call_handler(cpu: &mut Cpu, ctx: Context, (sel, off): (u16, u32)) {
    resume(cpu, &ctx);
    if !cpu.pm() || cpu.cpl != 3 {
        return;
    }
    let size = frame_size(cpu) as u8;
    let (eip, esp, cs, flags) = (cpu.eip(), cpu.esp(), cpu.cs(), cpu.eflags_image());
    let entered = (|| -> CpuResult {
        cpu.push_sized(size, flags)?;
        cpu.push_sized(size, cs as u32)?;
        cpu.push_sized(size, eip)?;
        cpu.jmp_far_pm(sel, if size == 2 { off & 0xFFFF } else { off })
    })();
    match entered {
        Ok(()) => {
            cpu.set_cpu_flag(CpuFlags::IF, false);
            cpu.set_cpu_flag(CpuFlags::TF, false);
            cpu.set_cpu_flag(CpuFlags::NT, false);
        }
        Err(fault) => {
            cpu.set_eip(eip);
            cpu.set_esp(esp);
            cpu.raise(fault);
        }
    }
}

/// The client's handler jumped to the default handler of `vector`, with the
/// interrupt's frame on its stack.
fn default_handler(cpu: &mut Cpu, vector: u8) {
    let size = frame_size(cpu) as u8;
    let mut ctx = Context::of(cpu);
    let popped = (|| -> CpuResult<(u32, u32, u32)> {
        let eip = cpu.pop_sized(size)?;
        let cs = cpu.pop_sized(size)?;
        let flags = cpu.pop_sized(size)?;
        Ok((eip, cs, flags))
    })();
    match popped {
        Ok((eip, cs, flags)) => {
            ctx.eip = eip;
            ctx.set_sel(Seg::CS, cs as u16);
            ctx.eflags = if size == 2 { (ctx.eflags & 0xFFFF_0000) | flags } else { flags };
            ctx.gpr[ESP] = cpu.esp();
            default_interrupt(cpu, vector, ctx);
        }
        Err(fault) => cpu.raise(fault),
    }
}

/// What the host does with interrupt `vector` itself: INT 31h's services,
/// INT 2Fh's in protected mode, and for everything else the handler in
/// real mode.
fn default_interrupt(cpu: &mut Cpu, vector: u8, mut ctx: Context) {
    match (vector, ctx.reg16(EAX)) {
        (0x31, _) => int31::call(cpu, ctx),
        // Whether this is protected mode: yes.
        (0x2F, 0x1686) => {
            ctx.set_reg16(EAX, 0);
            resume(cpu, &ctx);
        }
        // Vendor extensions: Windows' "MS-DOS" ones, which Borland's RTM
        // needs; for another vendor AL stays 8Ah.
        (0x2F, 0x168A) => {
            if vendor(cpu, &ctx) == b"MS-DOS" {
                let bits32 = client(cpu).bits32;
                ctx.gpr[EAX] &= !0xFF;
                ctx.set_sel(Seg::ES, HOST_CODE3);
                ctx.gpr[EDI] = if bits32 { MSDOS_API as u32 } else { (ctx.gpr[EDI] & !0xFFFF) | MSDOS_API as u32 };
            }
            resume(cpu, &ctx);
        }
        (0x21, _) if !is_irq(cpu, 0x21) => dos::int21(cpu, ctx),
        _ => reflect(cpu, vector, ctx),
    }
}

/// The vendor name at DS:(E)SI in the client's context `ctx`, without its
/// terminating zero (at most 32 bytes of it).
fn vendor(cpu: &Cpu, ctx: &Context) -> Vec<u8> {
    let bits32 = cpu.bus.dpmi.clients.last().is_some_and(|c| c.bits32);
    let Some(base) = selector_base(cpu, ctx.sel(Seg::DS)) else { return Vec::new() };
    let at = base.wrapping_add(if bits32 { ctx.gpr[ESI] } else { ctx.gpr[ESI] & 0xFFFF });
    (0..32).map(|i| cpu.bus.read_8(at.wrapping_add(i) as usize)).take_while(|&b| b != 0).collect()
}

/// The "MS-DOS" extensions' entry point, far-called: AX=0100h gives the
/// client a selector for its LDT in AX, with which it changes descriptors
/// itself, as Windows lets it; there is nothing else (CF set).
fn msdos_api(cpu: &mut Cpu) {
    let alias = if cpu.ax() == 0x0100 { ldt_alias(cpu) } else { None };
    if let Some(selector) = alias {
        cpu.set_ax(selector);
    }
    cpu.set_cpu_flag(CpuFlags::CF, alias.is_none());
    if let Err(fault) = cpu.ret_far_pm(frame_size(cpu) as u8, 0) {
        cpu.raise(fault);
    }
}

/// The running client's selector for its LDT, made the first time.
fn ldt_alias(cpu: &mut Cpu) -> Option<u16> {
    let c = client(cpu);
    if c.ldt_alias == 0 {
        let desc = descriptor(c.block, LDT_SIZE - 1, DATA3, 0);
        let index = allocate(cpu, 1, desc, LDT_FIXED)?;
        client(cpu).ldt_alias = Client::selector(index);
    }
    Some(client(cpu).ldt_alias)
}

/// The running client's context `ctx` in protected mode goes on after
/// code the host calls (in real mode, or an exception handler): the part of
/// the locked stack it is on stays in use, and its data segments are what
/// the host gives protected-mode code it calls from real mode.
fn keep_lpms(client: &mut Client, ctx: &Context) {
    if ctx.sel(Seg::SS) | 3 == client.lpms_sel {
        client.lpms_esp = ctx.gpr[ESP] & if client.bits32 { !0 } else { 0xFFFF };
    }
    client.segs = [Seg::ES, Seg::DS, Seg::FS, Seg::GS].map(|seg| ctx.sel(seg));
}

/// Reflect interrupt `vector` to its real-mode handler, and go on with
/// `ctx` afterwards: with the registers and flags the handler returns, but
/// for a hardware interrupt, whose code the interrupt stopped anywhere.
fn reflect(cpu: &mut Cpu, vector: u8, ctx: Context) {
    let target = real_mode_handler(cpu, vector);
    if target == 0 {
        // No handler: nothing happens, as for a real-mode INT.
        cpu.note_null_interrupt(vector);
        resume(cpu, &ctx);
        return;
    }
    let ss = client(cpu).rm_stack;
    let mut rm = ctx;
    rm.seg = [ss, 0, ss, ss, 0, 0];
    call_real(cpu, Frame { kind: F_REFLECT, ctx, vector, ..Frame::default() }, target, rm);
}

/// Call the real-mode interrupt handler at `target` for the running client
/// in `frame.ctx`, with the registers and data segments of `rm`, on the
/// client's real-mode stack: when it returns, `rm_return` goes on as
/// `frame` says.
fn call_real(cpu: &mut Cpu, mut frame: Frame, target: u32, mut rm: Context) {
    let client = client(cpu);
    frame.client = client.id;
    frame.rm_sp = client.rm_sp;
    frame.lpms = client.lpms_esp;
    keep_lpms(client, &frame.ctx);
    let (ss, sp) = (client.rm_stack, client.rm_sp);
    cpu.bus.dpmi.frames.push(frame);
    let flags = frame.ctx.eflags & 0xFFFF & !IOPL3;
    rm.gpr[ESP] = push_real(cpu, ss, sp, &[flags as u16, ROM_SEG, RM_RETURN]);
    rm.eip = target & 0xFFFF;
    rm.eflags = flags & !(CpuFlags::IF | CpuFlags::TF).bits();
    rm.set_sel(Seg::CS, (target >> 16) as u16);
    rm.set_sel(Seg::SS, ss);
    enter_rm(cpu, &rm);
}

/// Push `values` (the first at the highest address) on the real-mode stack
/// `ss`:`sp`. Returns the new SP.
fn push_real(cpu: &mut Cpu, ss: u16, sp: u32, values: &[u16]) -> u32 {
    let mut sp = sp;
    for &value in values {
        sp = sp.wrapping_sub(2) & 0xFFFF;
        let at = (ss as u32 * 16 + sp) & cpu.bus.a20_mask();
        cpu.bus.write_16(at as usize, value);
    }
    sp
}

/// Real-mode code the host called for a client returned (to `RM_RETURN`).
fn rm_return(cpu: &mut Cpu) {
    let Some(frame) = take_frame(cpu, &[F_REFLECT, F_TRANSLATE, F_DOS]) else { return };
    let rm = Context::of(cpu);
    let mut ctx = frame.ctx;
    if frame.kind == F_DOS {
        return dos::returned(cpu, frame, &rm);
    } else if frame.kind == F_TRANSLATE {
        int31::store_call_structure(cpu, frame.sel, frame.off, &rm);
        ctx.set_cf(false);
    } else if !is_irq(cpu, frame.vector) {
        let esp = ctx.gpr[ESP];
        ctx.gpr = rm.gpr;
        ctx.gpr[ESP] = esp;
        ctx.eflags = (ctx.eflags & !RESULT_FLAGS) | (rm.eflags & RESULT_FLAGS);
    }
    resume(cpu, &ctx);
}

/// Take the innermost frame, which must be one of `kinds` and the running
/// client's, and put back the client's stacks as they were before it.
fn take_frame(cpu: &mut Cpu, kinds: &[u8]) -> Option<Frame> {
    let dpmi = &mut cpu.bus.dpmi;
    let frame = dpmi.frames.pop();
    let client = dpmi.clients.last_mut();
    match (frame, client) {
        (Some(frame), Some(client)) if kinds.contains(&frame.kind) && frame.client == client.id => {
            client.rm_sp = frame.rm_sp;
            client.lpms_esp = frame.lpms;
            Some(frame)
        }
        (frame, _) => {
            cpu.bus.log_string(&format!("[DPMI] Nothing to return to at {:04X}:{:08X} ({:?})", cpu.cs(), cpu.eip(), frame));
            abort(cpu, "return without a call");
            None
        }
    }
}

/// An interrupt of the host's real-mode stub for `vector` (`hook`): to the
/// running client's protected-mode handler, on its locked stack, unless it
/// is reflecting that interrupt down to real mode itself; otherwise on to
/// the handler the vector had.
fn rm_interrupt(cpu: &mut Cpu, vector: u8) {
    let dpmi = &cpu.bus.dpmi;
    let reflecting = dpmi.frames.iter().any(|f| f.kind == F_REFLECT && f.vector == vector);
    let handler = dpmi.clients.last().and_then(|c| c.handler(vector));
    match handler {
        Some(handler) if !reflecting => {
            let rm = Context::of(cpu);
            let c = client(cpu);
            let frame = Frame {
                kind: F_HWINT,
                client: c.id,
                ctx: rm,
                vector,
                rm_sp: c.rm_sp,
                lpms: c.lpms_esp,
                ..Frame::default()
            };
            if rm.sel(Seg::SS) == c.rm_stack {
                c.rm_sp = rm.gpr[ESP] & 0xFFFF;
            }
            cpu.bus.dpmi.frames.push(frame);
            let mut ctx = rm;
            ctx.gpr[ESP] = push_lpms(cpu, &[rm.eflags & !CpuFlags::IF.bits(), HOST_CODE3 as u32, RET_HWINT as u32]);
            let c = client(cpu);
            let [es, ds, fs, gs] = c.segs;
            ctx.seg = [es, handler.0, c.lpms_sel, ds, fs, gs];
            ctx.eip = handler.1;
            ctx.eflags = rm.eflags & !(CpuFlags::IF | CpuFlags::TF).bits();
            resume(cpu, &ctx);
        }
        _ => {
            let target = real_mode_handler(cpu, vector);
            if target == 0 || target == rm_stub(vector) {
                iret_real(cpu);
            } else {
                cpu.set_cs((target >> 16) as u16);
                cpu.set_ip(target as u16);
            }
        }
    }
}

/// Push `values` (the first at the highest address) in the client's size
/// on its locked stack, where the host starts using it, and use that much
/// of it. Returns the new stack pointer.
fn push_lpms(cpu: &mut Cpu, values: &[u32]) -> u32 {
    let c = client(cpu);
    let size = c.frame_size();
    let mut sp = c.lpms_esp;
    let base = c.lpms_base();
    for &value in values {
        sp -= size;
        if size == 4 {
            cpu.bus.write_32((base + sp) as usize, value);
        } else {
            cpu.bus.write_16((base + sp) as usize, value as u16);
        }
    }
    client(cpu).lpms_esp = sp;
    sp
}

/// A handler called for an interrupt in real mode returned: back to real
/// mode, where the interrupt's IRET goes on.
fn ret_hwint(cpu: &mut Cpu) {
    let Some(frame) = take_frame(cpu, &[F_HWINT]) else { return };
    enter_rm(cpu, &frame.ctx);
    iret_real(cpu);
}

/// A real-mode callback (INT 31h AX=0303h) was called: its procedure runs
/// on the locked stack with the real-mode registers in its call structure
/// at ES:(E)DI and the real-mode stack at DS:(E)SI, and returns with an
/// IRET to `RET_CALLBACK`.
fn callback(cpu: &mut Cpu, slot: usize) {
    let dpmi = &cpu.bus.dpmi;
    let callback = dpmi.callbacks.get(slot).copied().flatten();
    let running = dpmi.clients.last().map(|c| c.id);
    let Some(cb) = callback.filter(|cb| Some(cb.client) == running) else {
        // A callback freed, or another client's: return to the caller.
        cpu.bus.log_string(&format!("[DPMI] Real-mode callback {} isn't there", slot));
        let ip = cpu.pop();
        let cs = cpu.pop();
        cpu.set_cs(cs);
        cpu.set_ip(ip);
        return;
    };
    let rm = Context::of(cpu);
    let c = client(cpu);
    let (id, ldt, cb_sel, lpms_sel) = (c.id, c.block, c.cb_sel, c.lpms_sel);
    let cb_index = (cb_sel >> 3) as usize;
    let frame = Frame {
        kind: F_CALLBACK,
        client: id,
        ctx: rm,
        off: slot as u32,
        rm_sp: c.rm_sp,
        lpms: c.lpms_esp,
        base: crate::cpu::Descriptor(read_desc(&cpu.bus, ldt, cb_index)).base(),
        ..Frame::default()
    };
    let c = client(cpu);
    if rm.sel(Seg::SS) == c.rm_stack {
        c.rm_sp = rm.gpr[ESP] & 0xFFFF;
    }
    cpu.bus.dpmi.frames.push(frame);
    write_desc(&mut cpu.bus, ldt, cb_index, descriptor((rm.sel(Seg::SS) as u32) << 4, 0xFFFF, DATA3, 0));
    int31::write_call_structure(cpu, cb.struct_sel, cb.struct_off, &rm, true);
    let mut ctx = rm;
    ctx.gpr[ESP] = push_lpms(cpu, &[rm.eflags & !CpuFlags::IF.bits(), HOST_CODE3 as u32, RET_CALLBACK as u32]);
    ctx.gpr[ESI] = rm.gpr[ESP] & 0xFFFF;
    ctx.gpr[EDI] = cb.struct_off;
    let [_, _, fs, gs] = client(cpu).segs;
    ctx.seg = [cb.struct_sel, cb.proc_sel, lpms_sel, cb_sel, fs, gs];
    ctx.eip = cb.proc_off;
    ctx.eflags = rm.eflags & !(CpuFlags::IF | CpuFlags::TF).bits();
    resume(cpu, &ctx);
}

/// A callback's procedure returned: real mode goes on with the registers
/// in the call structure at ES:(E)DI.
fn ret_callback(cpu: &mut Cpu) {
    let Some(frame) = take_frame(cpu, &[F_CALLBACK]) else { return };
    let c = client(cpu);
    let (cb_index, ldt, bits32) = ((c.cb_sel >> 3) as usize, c.block, c.bits32);
    let old = read_desc(&cpu.bus, ldt, cb_index);
    let restored = (old & !0xFF00_00FF_FFFF_0000) | descriptor(frame.base, 0, 0, 0) & 0xFF00_00FF_FFFF_0000;
    write_desc(&mut cpu.bus, ldt, cb_index, restored);
    let off = if bits32 { cpu.edi() } else { cpu.di() as u32 };
    match int31::read_call_structure(cpu, Seg::ES, off) {
        Ok(rm) => enter_rm(cpu, &rm),
        Err(fault) => cpu.raise(fault),
    }
}

/// An exception in the client's context `ctx`: to its exception handler,
/// or the default one's.
fn exception_entry(cpu: &mut Cpu, exception: u8, error: u32, ctx: Context) {
    let handler = client(cpu).exceptions[exception as usize];
    if handler == Client::default_exception(exception) {
        default_exception(cpu, exception, error, ctx);
        return;
    }
    // The exception's frame on the locked stack (below where the client is
    // on it): the client's SS:ESP, EFLAGS, CS:EIP and the error code, and
    // the handler's far return to the host.
    let c = client(cpu);
    let saved = c.lpms_esp;
    keep_lpms(c, &ctx);
    let esp = push_lpms(
        cpu,
        &[
            ctx.sel(Seg::SS) as u32,
            ctx.gpr[ESP],
            ctx.eflags,
            ctx.sel(Seg::CS) as u32,
            ctx.eip,
            error,
            HOST_CODE3 as u32,
            RET_EXCEPTION as u32,
        ],
    );
    let c = client(cpu);
    c.lpms_esp = saved;
    let mut entry = ctx;
    entry.gpr[ESP] = esp;
    entry.set_sel(Seg::SS, c.lpms_sel);
    entry.set_sel(Seg::CS, handler.0);
    entry.eip = handler.1;
    entry.eflags = ctx.eflags & !(CpuFlags::IF | CpuFlags::TF).bits();
    resume(cpu, &entry);
}

/// An exception handler returned (RETF) to the host: the client goes on
/// with the context in the exception's frame, which the handler may have
/// changed.
fn ret_exception(cpu: &mut Cpu) {
    let size = frame_size(cpu) as u8;
    let read = |cpu: &mut Cpu, i: u32| cpu.stack_read(i * size as u32, size);
    let frame = (|| -> CpuResult<[u32; 6]> {
        Ok([read(cpu, 0)?, read(cpu, 1)?, read(cpu, 2)?, read(cpu, 3)?, read(cpu, 4)?, read(cpu, 5)?])
    })();
    match frame {
        Ok([_error, eip, cs, flags, esp, ss]) => {
            let mut ctx = Context::of(cpu);
            ctx.eip = eip;
            ctx.set_sel(Seg::CS, cs as u16);
            ctx.eflags = if size == 2 { (ctx.eflags & 0xFFFF_0000) | flags } else { flags };
            ctx.gpr[ESP] = esp;
            ctx.set_sel(Seg::SS, ss as u16);
            resume(cpu, &ctx);
        }
        Err(fault) => cpu.raise(fault),
    }
}

/// The client's exception handler jumped to the default one's, with the
/// exception's frame on its stack.
fn default_exception_handler(cpu: &mut Cpu, exception: u8) {
    let size = frame_size(cpu) as u8;
    let read = |cpu: &mut Cpu, i: u32| cpu.stack_read(i * size as u32, size);
    let frame = (|| -> CpuResult<[u32; 6]> {
        Ok([read(cpu, 2)?, read(cpu, 3)?, read(cpu, 4)?, read(cpu, 5)?, read(cpu, 6)?, read(cpu, 7)?])
    })();
    match frame {
        Ok([error, eip, cs, flags, esp, ss]) => {
            let mut ctx = Context::of(cpu);
            ctx.eip = eip;
            ctx.set_sel(Seg::CS, cs as u16);
            ctx.eflags = if size == 2 { (ctx.eflags & 0xFFFF_0000) | flags } else { flags };
            ctx.gpr[ESP] = esp;
            ctx.set_sel(Seg::SS, ss as u16);
            default_exception(cpu, exception, error, ctx);
        }
        Err(fault) => cpu.raise(fault),
    }
}

/// What the host does with an exception the client doesn't handle:
/// exceptions 0-5 and 7 go to the protected-mode interrupt handler of
/// that number; a HLT (which level 3 may not run) waits for an interrupt;
/// anything else ends the program.
fn default_exception(cpu: &mut Cpu, exception: u8, error: u32, mut ctx: Context) {
    match exception {
        0..=5 | 7 => interrupt(cpu, exception, ctx),
        13 if error == 0 && instruction_byte(cpu, &ctx) == Some(0xF4) => {
            ctx.eip = ctx.eip.wrapping_add(1);
            resume(cpu, &ctx);
            cpu.state = CpuState::Halted;
        }
        _ => {
            let message = format!(
                "exception {:02X}h, error code {:04X}, at {:04X}:{:08X}",
                exception,
                error,
                ctx.sel(Seg::CS),
                ctx.eip
            );
            abort(cpu, &message);
        }
    }
}

/// The first byte of the instruction at the context's CS:EIP.
fn instruction_byte(cpu: &Cpu, ctx: &Context) -> Option<u8> {
    let base = selector_base(cpu, ctx.sel(Seg::CS))?;
    Some(cpu.bus.read_8(base.wrapping_add(ctx.eip) as usize))
}

/// The client can't go on: say why, and end its program with exit code
/// FFh, from real mode.
fn abort(cpu: &mut Cpu, why: &str) {
    cpu.bus.log_string(&format!("[DPMI] Ending the client's program: {}", why));
    let Some(client) = cpu.bus.dpmi.clients.last().cloned() else {
        cpu.state = CpuState::RebootShell;
        return;
    };
    let mut rm = Context::default();
    rm.seg = [client.rm_stack, ROM_SEG, client.rm_stack, client.rm_stack, 0, 0];
    rm.gpr[ESP] = PRIVATE_PARAS as u32 * 16;
    rm.eip = TERMINATE as u32;
    rm.eflags = 0x0202;
    enter_rm(cpu, &rm);
    crate::video::print_string(cpu, &format!("\r\nDPMI host: {}\r\n", why));
}

/// The raw switch to protected mode (INT 31h AX=0306h), jumped to in real
/// mode: AX, CX, DX, (E)BX, SI and (E)DI are the new DS, ES, SS, (E)SP, CS
/// and (E)IP; FS and GS are as the client left them (`Client::segs`).
fn raw_to_pm(cpu: &mut Cpu) {
    let c = client(cpu);
    let (bits32, [_, _, fs, gs]) = (c.bits32, c.segs);
    let mut ctx = Context::of(cpu);
    ctx.seg = [cpu.cx(), cpu.si(), cpu.dx(), cpu.ax(), fs, gs];
    ctx.gpr[ESP] = if bits32 { cpu.ebx() } else { cpu.bx() as u32 };
    ctx.eip = if bits32 { cpu.edi() } else { cpu.di() as u32 };
    resume(cpu, &ctx);
}

/// The raw switch to real mode, jumped to in protected mode: the same
/// registers, with real-mode segments.
fn raw_to_rm(cpu: &mut Cpu) {
    let mut ctx = Context::of(cpu);
    keep_lpms(client(cpu), &ctx);
    ctx.seg = [cpu.cx(), cpu.si(), cpu.dx(), cpu.ax(), 0, 0];
    ctx.gpr[ESP] = cpu.bx() as u32;
    ctx.eip = cpu.di() as u32;
    enter_rm(cpu, &ctx);
}

/// DOS ended the process `psp`: the clients running in it end with it,
/// their memory, descriptors and callbacks freed, and with the last one
/// the host's tables. It happens in real mode, in DOS's INT 21h.
pub fn process_ended(cpu: &mut Cpu, psp: u16) {
    while cpu.bus.dpmi.clients.iter().any(|c| c.psp == psp) {
        let i = cpu.bus.dpmi.clients.iter().rposition(|c| c.psp == psp).unwrap();
        let client = cpu.bus.dpmi.clients.remove(i);
        let dpmi = &mut cpu.bus.dpmi;
        dpmi.frames.retain(|f| f.client != client.id);
        for slot in dpmi.callbacks.iter_mut() {
            if slot.is_some_and(|cb| cb.client == client.id) {
                *slot = None;
            }
        }
        for &(_, base, _) in &client.memory {
            cpu.bus.xms.release_dpmi(base);
        }
        cpu.bus.xms.release_dpmi(client.block);
        if client.env != 0 {
            cpu.bus.write_16(psp as usize * 16 + 0x2C, client.env);
        }
        let hooked: Vec<u8> = cpu.bus.dpmi.hooks.iter().map(|&(v, _)| v).collect();
        for vector in hooked {
            unhook(cpu, vector);
        }
        cpu.bus.log_string(&format!("[DPMI] Client for PSP {:04X} ended", psp));
    }
    if cpu.bus.dpmi.host.is_some() && !cpu.bus.dpmi.active() {
        stop_host(cpu);
    }
}

/// The last client ended: the host's tables go, and the A20 gate is as it
/// was before.
fn stop_host(cpu: &mut Cpu) {
    let dpmi = &mut cpu.bus.dpmi;
    for (vector, original) in std::mem::take(&mut dpmi.hooks) {
        if read_ivt(&cpu.bus, vector) == rm_stub(vector) {
            write_ivt(&mut cpu.bus, vector, original);
        }
    }
    let dpmi = &mut cpu.bus.dpmi;
    if let Some(host) = dpmi.host.take() {
        cpu.bus.xms.release_dpmi(host);
    }
    let a20 = cpu.bus.dpmi.a20;
    cpu.bus.dpmi.reset();
    cpu.bus.set_a20(a20);
    if !cpu.pe() {
        cpu.gdtr = DescTable { base: 0, limit: 0xFFFF };
        cpu.ldtr = SegCache::null(0);
        cpu.tr = SegCache::null(0);
    }
    cpu.bus.log_string("[DPMI] Host stopped");
}

crate::state_fields!(Context { gpr, eip, eflags, seg });
crate::state_fields!(Client {
    id, psp, bits32, block, ldt, segments, vectors, exceptions, memory, dos_blocks,
    rm_stack, rm_sp, lpms_sel, lpms_esp, cb_sel, segs, env, dta, ldt_alias, blocks,
});
crate::state_fields!(Callback { client, proc_sel, proc_off, struct_sel, struct_off });
crate::state_fields!(Frame { kind, client, ctx, vector, sel, off, rm_sp, lpms, base });
crate::state_fields!(Dpmi { host, clients, frames, callbacks, hooks, a20, next_id, next_handle } skip { enabled });
