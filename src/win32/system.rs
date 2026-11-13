//! The processor's side of a Win32 process: its descriptor tables, the
//! switches between the program's protected mode and DOS's real mode, and
//! what comes through the IDT: hardware interrupts, which their real-mode
//! handlers (the BIOS's, DOS's, resident programs') get as they would
//! under Windows 95, and exceptions.
//!
//! The tables are in a block of the process's memory mapped at the start
//! of the system arena (`SYSTEM_BLOCK`). Each of the IDT's gates leads to
//! a handler in the block that jumps to the vector's service trap in the
//! ROM, run at level 0 on the TSS's stack.
//!
//! As on Windows 95 and 98, the program can read and write the block:
//! there the system arena's pages are open to level 3, and programs use
//! that. Copy protections such as SafeDisc's find the IDT with SIDT,
//! write a few instructions over the handler of an exception's gate and
//! raise the exception to run their own code at level 0, then put the
//! handler back. So each gate leads to a handler of its own in the block,
//! a jump to the vector's trap, which a program can change and put back
//! as it could the system's handler. The handlers are 32 bytes apart,
//! room for code like SafeDisc's (which clears five debug registers), so
//! that it doesn't run into the next vectors' handlers while it is
//! there: the timer's (vector 8) is a few after the exceptions'.
//!
//! The block is VMM's locked code object as well, and ends with VMM's
//! code (vmm.rs): the services that a program's level 0 code calls
//! through INT 20h run in Rust here (`vxd_service`) or as that code.

use super::memory::{Memory, PAGE, SYSTEM, USER, WRITABLE};
use super::{Context, EAX, ECX, EDI, EDX, ESP, ID_IDT, IDT_TRAPS, ROM_BASE, realmode, vmm};
use crate::bus::Bus;
use crate::cpu::{CR0_PE, CR0_PG, CR4_OSFXSR, Cpu, CpuFlags, DescTable, Seg, SegCache, alu};
use crate::dpmi::descriptor;

/// The system block's parts, as offsets from where it is mapped.
const GDT: u32 = 0x0000;
const IDT: u32 = 0x1000;
const TSS: u32 = 0x2000;
/// The TSS's I/O permission bitmap: every port open, as Windows 95 leaves
/// those no driver of its own claims; a byte of ones ends it.
const IOPB: u32 = TSS + 0x68;
const IOPB_END: u32 = IOPB + 0x2000;
const LDT: u32 = 0x5000;
/// The LDT's offset in the system block, for the Win16 selectors there.
pub const LDT_OFFSET: u32 = LDT;
pub const LDT_ENTRIES: u32 = 0x2000;
const RING0_STACK: u32 = LDT + 8 * LDT_ENTRIES;
const RING0_STACK_TOP: u32 = RING0_STACK + 2 * PAGE;
/// VMM's client register structure (VMM.INC's Client_Reg_Struc, 48h
/// bytes), the program's state as VMM's services see it, at the top of
/// the level 0 stack as on Windows 9x (`vmm`).
const CLIENT_REGS: u32 = RING0_STACK_TOP - 0x48;
/// The level 0 stack's top, the TSS's ESP0, at Client_ES: the frame of
/// an interrupt or exception from level 3 fills Client_EIP to Client_SS,
/// and an exception's error code Client_Error.
const RING0_ESP: u32 = CLIENT_REGS + 0x38;
/// The handlers the IDT's gates lead to, `HANDLER_SIZE` bytes each: a
/// jump to the vector's trap in the ROM, and room for a program's own
/// code in its place.
const HANDLERS: u32 = RING0_STACK_TOP;
const HANDLER_SIZE: u32 = 32;
/// VMM's data for the services a program's level 0 code may call through
/// INT 20h (`vxd_service`): VMM's DDB, the list of where the VxDs are in
/// memory, which VMM_GetVxDLocationList returns, and the VxDs loaded
/// since the start (`DYNAMIC_VXDS`), which VXDLDR_GetDeviceList returns.
const VMM_DATA: u32 = HANDLERS + 256 * HANDLER_SIZE;
const VMM_DDB: u32 = VMM_DATA;
const DDB_SIZE: u32 = 0x50;
const VXD_LOCATIONS: u32 = VMM_DDB + DDB_SIZE;
/// The VxD location list's length: VMM's record with one object.
const VXD_LOCATIONS_LEN: u32 = 5 + 9;
/// VXDLDR's variable with the head of its list of DeviceInfo blocks,
/// which VXDLDR_GetDeviceList returns: 0 while it has loaded none. The
/// service reads it here, so the process of a state saved before the
/// built-in Windows had these VxDs, whose block has 0 here, keeps the
/// empty list it had.
const DEVICE_LIST: u32 = (VXD_LOCATIONS + VXD_LOCATIONS_LEN).next_multiple_of(4);
/// VMM's code (`vmm`): the routines of the services the built-in Windows
/// runs as guest code, VMM's service table and the VM's control block.
/// It ends VMM's locked code object. VMM's data before it stays where a
/// state saved before VMM had code has it.
const VMM_CODE: u32 = (DEVICE_LIST + 4).next_multiple_of(16);
/// The dynamically loaded VxDs, `VXD_SIZE` bytes each, in load order:
/// the VxD's locked code object (`VXD_OBJECT_SIZE` bytes: its control
/// procedure and its DDB), then VXDLDR's DeviceInfo with its ObjectInfo
/// array and the module's name.
const VXDS: u32 = VMM_CODE + vmm::SIZE;
const VXD_CONTROL: u32 = 0x00;
const VXD_DDB: u32 = 0x10;
const VXD_OBJECT_SIZE: u32 = VXD_DDB + DDB_SIZE;
const VXD_INFO: u32 = VXD_OBJECT_SIZE;
/// The DeviceInfo's parts, from its start. Windows 98 SE's VXDLDR puts
/// the ObjectInfo array 30h bytes after the DeviceInfo, as here, for
/// most of its VxDs.
const INFO_OBJECTS: u32 = 0x30;
const INFO_NAME: u32 = 0x40;
const VXD_SIZE: u32 = VXD_INFO + INFO_NAME + 0x10;
pub const SYSTEM_BLOCK_PAGES: u32 = (VXDS + VXD_SIZE * DYNAMIC_VXDS.len() as u32).div_ceil(PAGE);

/// The GDT's selectors: the TSS, the LDT, and level 0 code and data
/// covering everything at 28h and 30h, where Windows 98's VMM has its
/// flat segments. Programs that run their own code at level 0 through
/// the IDT build its gate with those numbers: SafeDisc 2.30 (Anachronox)
/// writes 28h into the gate rather than copying the system's.
const TSS_SEL: u16 = 0x18;
const LDT_SEL: u16 = 0x20;
const CODE0: u16 = 0x28;
const DATA0: u16 = 0x30;
const GDT_ENTRIES: u32 = 7;

/// The program's selectors, in the LDT where Windows 95 has its flat code
/// and data segments (0137h and 013Fh), and the thread's TIB after them.
pub const CODE3: u16 = 0x0137;
pub const DATA3: u16 = 0x013F;
pub const TIB3: u16 = 0x0147;

/// Descriptor access bytes and the flags (G, D/B) in `SegCache::attr`'s
/// form.
const ACCESS_CODE0: u8 = 0x9B;
const ACCESS_DATA0: u8 = 0x93;
const ACCESS_CODE3: u8 = 0xFB;
const ACCESS_DATA3: u8 = 0xF3;
const FLAT: u8 = 0xC;
const BIG: u8 = 0x4;

fn attr(access: u8, flags: u8) -> u16 {
    access as u16 | (flags as u16) << 12
}

fn write_descriptor(cpu: &mut Cpu, at: u32, desc: u64) {
    cpu.bus.write_32(at as usize, desc as u32);
    cpu.bus.write_32(at as usize + 4, (desc >> 32) as u32);
}

/// Make the system block (`pages` in a row at `phys`) and map it, open to
/// level 3 as Windows 95's system arena is: the GDT, the IDT and the
/// handlers its gates lead to, the TSS with the level 0 stack and the LDT
/// with the program's selectors, the TIB's at `tib`, and VMM's data and
/// code.
pub fn build_tables(cpu: &mut Cpu, memory: &mut Memory, phys: u32, tib: u32) -> Option<()> {
    for page in 0..SYSTEM_BLOCK_PAGES {
        memory.map(
            &mut cpu.bus,
            SYSTEM + page * PAGE,
            phys + page * PAGE,
            WRITABLE | USER,
        )?;
    }
    let gdt = [
        (CODE0, descriptor(0, 0xFFFF_FFFF, ACCESS_CODE0, FLAT)),
        (DATA0, descriptor(0, 0xFFFF_FFFF, ACCESS_DATA0, FLAT)),
        (TSS_SEL, descriptor(SYSTEM + TSS, IOPB_END - TSS, 0x8B, 0)),
        (
            LDT_SEL,
            descriptor(SYSTEM + LDT, 8 * LDT_ENTRIES - 1, 0x82, 0),
        ),
    ];
    for (selector, desc) in gdt {
        write_descriptor(cpu, phys + GDT + selector as u32, desc);
    }
    // Every vector a 32-bit interrupt gate to its handler, at level 0,
    // which jumps to its trap in the ROM. INT 3 and INTO may be used at
    // level 3.
    for vector in 0..256u32 {
        let offset = SYSTEM + HANDLERS + HANDLER_SIZE * vector;
        let trap = ROM_BASE as u32 + IDT_TRAPS as u32 + 4 * vector;
        let mut handler = [0x90u8; HANDLER_SIZE as usize];
        handler[0] = 0xE9;
        handler[1..5].copy_from_slice(&trap.wrapping_sub(offset + 5).to_le_bytes());
        let at = phys + HANDLERS + HANDLER_SIZE * vector;
        cpu.bus.load_bytes(at as usize, &handler);
        let dpl3 = matches!(vector, 3 | 4);
        let access: u64 = if dpl3 { 0xEE } else { 0x8E };
        let gate = (offset as u64 & 0xFFFF)
            | (CODE0 as u64) << 16
            | access << 40
            | ((offset as u64) >> 16) << 48;
        write_descriptor(cpu, phys + IDT + 8 * vector, gate);
    }
    let tss = (phys + TSS) as usize;
    cpu.bus.write_32(tss + 4, SYSTEM + RING0_ESP);
    cpu.bus.write_32(tss + 8, DATA0 as u32);
    cpu.bus.write_16(tss + 0x66, (IOPB - TSS) as u16);
    cpu.bus.write_8((phys + IOPB_END) as usize, 0xFF);
    write_vmm_data(cpu, phys);
    let ldt = phys + LDT;
    let ldt_desc = |selector: u16| ldt + (selector & !7) as u32;
    write_descriptor(
        cpu,
        ldt_desc(CODE3),
        descriptor(0, 0xFFFF_FFFF, ACCESS_CODE3, FLAT),
    );
    write_descriptor(
        cpu,
        ldt_desc(DATA3),
        descriptor(0, 0xFFFF_FFFF, ACCESS_DATA3, FLAT),
    );
    write_descriptor(
        cpu,
        ldt_desc(TIB3),
        descriptor(tib, PAGE - 1, ACCESS_DATA3, BIG),
    );
    Some(())
}

/// A VxD's DDB (VMM.INC's VxD_Desc_Block), as offsets from its start.
const DDB_NEXT: u32 = 0x00;
const DDB_SDK_VERSION: u32 = 0x04;
const DDB_DEVICE_ID: u32 = 0x06;
const DDB_VERSION: u32 = 0x08;
const DDB_FLAGS: u32 = 0x0A;
const DDB_NAME: u32 = 0x0C;
const DDB_INIT_ORDER: u32 = 0x14;
const DDB_CONTROL_PROC: u32 = 0x18;
const DDB_SERVICE_TABLE: u32 = 0x30;
const DDB_SERVICE_TABLE_SIZE: u32 = 0x34;
const DDB_PREV: u32 = 0x3C;
const DDB_SIZE_FIELD: u32 = 0x40;
const DDB_RESERVED: u32 = 0x44;

/// A VxD that Windows 98 SE's VXDLDR loads after the start and that the
/// built-in Windows stands for, with the fields its DDB and DeviceInfo
/// have there.
struct DynamicVxd {
    /// The DDB's name, padded with spaces.
    name: &'static [u8; 8],
    /// The module's name in the DeviceInfo, shorter than 16 bytes.
    module: &'static str,
    id: u16,
    /// The DDK version the VxD was built for.
    sdk: u16,
    version: (u8, u8),
    init_order: u32,
    /// Whether the machine has what the VxD serves, so that Windows 98
    /// loads it.
    present: fn(&Bus) -> bool,
}

/// `DynamicVxd::present` of a VxD that every machine has a use for.
fn always(_: &Bus) -> bool {
    true
}

/// The dynamically loaded VxDs of the parts of a Windows 98 machine that
/// the built-in Windows provides, in the order Windows 98 SE loads them:
/// the configuration manager's enumerators of the BIOS's devices, ISA
/// Plug and Play and, on a machine with one, the PCI bus (a machine with
/// no S3, 3dfx or other PCI card has none, its PnP BIOS lists none, and
/// Windows 98 doesn't load PCI's VxD there); then IOS's drivers for the
/// disks and the CD-ROM drive, which the built-in Windows serves through
/// its files, INT 13h and ASPI (APIX, the ATAPI layer under ASPI, CDFS
/// and the CD-ROM's type and volume drivers, the disks' type driver,
/// voltrack, and ESDI_506, the IDE controller's port driver). Each has
/// the DDB fields Windows 98 SE's has. VxDs of hardware a machine may
/// lack (sound cards, displays, floppy drives) and NTKERN's WDM drivers,
/// which the built-in Windows has no counterpart of, aren't listed.
const DYNAMIC_VXDS: [DynamicVxd; 10] = [
    DynamicVxd {
        name: b"BIOS    ",
        module: "BIOS",
        id: 0x003D,
        sdk: 0x040A,
        version: (4, 10),
        init_order: 0x1680_0001,
        present: always,
    },
    DynamicVxd {
        name: b"ISAPNP  ",
        module: "ISAPNP",
        id: 0x003C,
        sdk: 0x040A,
        version: (4, 10),
        init_order: 0x1680_0000,
        present: always,
    },
    DynamicVxd {
        name: b"PCI     ",
        module: "PCI",
        id: 0x0043,
        sdk: 0x040A,
        version: (4, 10),
        init_order: 0x1680_0000,
        present: Bus::pci_present,
    },
    DynamicVxd {
        name: b"APIX    ",
        module: "APIX",
        id: 0x0022,
        sdk: 0x0400,
        version: (3, 10),
        init_order: UNDEFINED_INIT_ORDER,
        present: always,
    },
    DynamicVxd {
        name: b"CDFS    ",
        module: "CDFS",
        id: 0,
        sdk: 0x040A,
        version: (3, 0),
        init_order: 0xA001_0100,
        present: always,
    },
    DynamicVxd {
        name: b"CDTSD   ",
        module: "CDTSD",
        id: 0,
        sdk: 0x0400,
        version: (3, 10),
        init_order: UNDEFINED_INIT_ORDER,
        present: always,
    },
    DynamicVxd {
        name: b"CDVSD   ",
        module: "CDVSD",
        id: 0,
        sdk: 0x040A,
        version: (3, 10),
        init_order: UNDEFINED_INIT_ORDER,
        present: always,
    },
    DynamicVxd {
        name: b"DiskTSD ",
        module: "DiskTSD",
        id: 0,
        sdk: 0x040A,
        version: (3, 10),
        init_order: UNDEFINED_INIT_ORDER,
        present: always,
    },
    DynamicVxd {
        name: b"voltrack",
        module: "voltrack",
        id: 0x0090,
        sdk: 0x0400,
        version: (3, 10),
        init_order: UNDEFINED_INIT_ORDER,
        present: always,
    },
    DynamicVxd {
        name: b"ESDI_506",
        module: "ESDI_506",
        id: 0x008D,
        sdk: 0x0400,
        version: (3, 10),
        init_order: UNDEFINED_INIT_ORDER,
        present: always,
    },
];

/// The init order of a VxD that has no place of its own among the
/// others: it comes after them.
const UNDEFINED_INIT_ORDER: u32 = 0x8000_0000;
/// The flags of a dynamically loaded VxD's DDB on Windows 98 SE, once it
/// has started: bit 15, which VXDLDR sets on the VxDs it loads, and bits
/// 1 and 2.
const DYNAMIC_DDB_FLAGS: u16 = 0x8006;
/// The signature of a DeviceInfo, "XVLD" read as a dword.
const DEVICE_INFO_SIGNATURE: u32 = 0x444C_5658;

/// The DDB's three reserved dwords, which VMM.INC's
/// Declare_Virtual_Device fills with the signatures 'Rsv1' to 'Rsv3',
/// as MASM stores a dword of four characters: the first character in
/// the high byte, so that memory reads "1vsR" and so on, as on Windows
/// 98 SE.
const DDB_RESERVED_BYTES: &[u8; 12] = b"1vsR2vsR3vsR";

/// VMM's DDB, the VxD location list and the dynamically loaded VxDs, in
/// the system block at `phys`.
///
/// VMM's DDB has the fields of Windows 98 SE's VMM that say what it is:
/// the name, the id 0001h, version 4.10 for both the DDK and VMM, its
/// size and the three reserved signatures, and VMM's service table
/// (`vmm`), with an entry point in VMM's object for each service the
/// built-in Windows has. It has no control procedure: the built-in
/// Windows sends VMM no system control messages.
///
/// The list is packed records, one for each VxD in load order: the DDB's
/// address, a byte with the number of objects, and for each object its
/// address, its size and a byte with its type (01h locked code, 03h
/// pageable code, 04h pageable data and so on, VXDLDR's object types).
/// Windows 98 SE lists some 50 VxDs there, VMM first: those loaded at
/// the start, before VXDLDR's. The built-in Windows has VMM alone, whose
/// locked code is this block up to the dynamically loaded VxDs: the
/// IDT's gates lead to its handlers, as Windows 98's lead into VMM's
/// objects, and VMM's code (`VMM_CODE`) ends it.
///
/// The VxDs loaded since the start are `write_dynamic_vxds`'s, each with
/// an object of its own after VMM's, as no two VxDs' objects overlap on
/// Windows 98. VMM's DDB heads the list of all DDBs (VMM_GetDDBList),
/// theirs included.
fn write_vmm_data(cpu: &mut Cpu, phys: u32) {
    let ddb = (phys + VMM_DDB) as usize;
    write_ddb(&mut cpu.bus, ddb, b"VMM     ", 0x0001, 0x040A, (4, 10));
    let rust_services = RUST_VMM_SERVICES.map(|service| service as u16);
    let code = vmm::build(SYSTEM + VMM_CODE, SYSTEM + CLIENT_REGS, &rust_services);
    cpu.bus.load_bytes((phys + VMM_CODE) as usize, &code.bytes);
    cpu.bus.write_32(ddb + DDB_SERVICE_TABLE as usize, code.service_table);
    cpu.bus.write_32(ddb + DDB_SERVICE_TABLE_SIZE as usize, code.services);
    write_dynamic_vxds(cpu, phys);
    let mut list = Vec::with_capacity(VXD_LOCATIONS_LEN as usize);
    list.extend_from_slice(&(SYSTEM + VMM_DDB).to_le_bytes());
    list.push(1);
    list.extend_from_slice(&SYSTEM.to_le_bytes());
    list.extend_from_slice(&VXDS.to_le_bytes());
    list.push(OBJECT_LOCKED_CODE);
    debug_assert_eq!(list.len(), VXD_LOCATIONS_LEN as usize);
    cpu.bus.load_bytes((phys + VXD_LOCATIONS) as usize, &list);
}

/// The fields of the DDB at `ddb` (physical) that say what the VxD is:
/// its name, id, DDK and VxD versions, the DDB's size and the reserved
/// signatures.
fn write_ddb(bus: &mut Bus, ddb: usize, name: &[u8; 8], id: u16, sdk: u16, version: (u8, u8)) {
    bus.write_16(ddb + DDB_SDK_VERSION as usize, sdk);
    bus.write_16(ddb + DDB_DEVICE_ID as usize, id);
    bus.write_8(ddb + DDB_VERSION as usize, version.0);
    bus.write_8(ddb + DDB_VERSION as usize + 1, version.1);
    bus.load_bytes(ddb + DDB_NAME as usize, name);
    bus.write_32(ddb + DDB_SIZE_FIELD as usize, DDB_SIZE);
    bus.load_bytes(ddb + DDB_RESERVED as usize, DDB_RESERVED_BYTES);
}

/// The address of the `index`th of `DYNAMIC_VXDS` in the system arena.
fn vxd_at(index: usize) -> u32 {
    SYSTEM + VXDS + VXD_SIZE * index as u32
}

/// The dynamically loaded VxDs (`DYNAMIC_VXDS`) the machine has, in the
/// system block at `phys`, VXDLDR's list of them, and the DDB list that
/// starts at VMM's DDB. A VxD the machine has no use for keeps its place
/// in the block, empty and in neither list.
///
/// Each VxD has one object, of locked code: a control procedure that
/// answers every system control message with success (carry clear), as
/// a VxD does for the messages it doesn't handle, and the DDB, which
/// points at it. The built-in Windows does the VxD's work itself and
/// sends it no messages, but the DDB has a working procedure as every
/// VxD's does.
///
/// VXDLDR describes each VxD it loads with a DeviceInfo (VXDLDR.H, and
/// Geoff Chappell's notes on VXDLDR), packed: the next DeviceInfo (+00h,
/// the one of the VxD loaded before), the load count (+04h, a byte), the
/// DDB (+05h), the device id (+09h, a word), the module's name (+0Bh),
/// the signature "XVLD" (+0Fh), the number of objects (+13h), the
/// ObjectInfo array (+17h) and the V86 and protected mode API entry
/// points (+1Bh, +1Fh), 0 for these VxDs. An ObjectInfo is four dwords:
/// the object's address, its size, its type and 0, the object being of
/// this load.
///
/// On Windows 98 SE the DDBs of the VxDs VXDLDR loads are in VMM's DDB
/// list too, each put in by its init order after those of the same or a
/// lower one, and each DDB's DDB_Prev points at the one before it, VMM's
/// at the last.
fn write_dynamic_vxds(cpu: &mut Cpu, phys: u32) {
    let bus = &mut cpu.bus;
    let ddb_of = |index: usize| vxd_at(index) + VXD_DDB;
    let loaded: Vec<usize> = (0..DYNAMIC_VXDS.len())
        .filter(|&index| (DYNAMIC_VXDS[index].present)(bus))
        .collect();
    // Each DeviceInfo links to the one of the VxD loaded before.
    let mut previous = 0;
    for &index in &loaded {
        let vxd = &DYNAMIC_VXDS[index];
        debug_assert!(vxd.module.len() < 0x10);
        let at = vxd_at(index);
        let base = (phys + (at - SYSTEM)) as usize;
        // clc ; ret
        bus.load_bytes(base + VXD_CONTROL as usize, &[0xF8, 0xC3]);
        let ddb = base + VXD_DDB as usize;
        write_ddb(bus, ddb, vxd.name, vxd.id, vxd.sdk, vxd.version);
        bus.write_16(ddb + DDB_FLAGS as usize, DYNAMIC_DDB_FLAGS);
        bus.write_32(ddb + DDB_INIT_ORDER as usize, vxd.init_order);
        bus.write_32(ddb + DDB_CONTROL_PROC as usize, at + VXD_CONTROL);

        let info = base + VXD_INFO as usize;
        let info_at = at + VXD_INFO;
        bus.write_32(info, previous);
        bus.write_8(info + 0x04, 1);
        bus.write_32(info + 0x05, ddb_of(index));
        bus.write_16(info + 0x09, vxd.id);
        bus.write_32(info + 0x0B, info_at + INFO_NAME);
        bus.write_32(info + 0x0F, DEVICE_INFO_SIGNATURE);
        bus.write_32(info + 0x13, 1);
        bus.write_32(info + 0x17, info_at + INFO_OBJECTS);
        let object = info + INFO_OBJECTS as usize;
        bus.write_32(object, at);
        bus.write_32(object + 4, VXD_OBJECT_SIZE);
        bus.write_32(object + 8, OBJECT_LOCKED_CODE as u32);
        let mut name = vxd.module.as_bytes().to_vec();
        name.push(0);
        bus.load_bytes(info + INFO_NAME as usize, &name);
        previous = info_at;
    }
    // The VxD loaded last heads VXDLDR's list.
    bus.write_32((phys + DEVICE_LIST) as usize, previous);
    // The DDB list: VMM's, then the others by init order, those of the
    // same order in load order.
    let mut order = loaded;
    order.sort_by_key(|&index| DYNAMIC_VXDS[index].init_order);
    let mut chain = vec![SYSTEM + VMM_DDB];
    chain.extend(order.iter().map(|&index| ddb_of(index)));
    let phys_of = |ddb: u32| (phys + (ddb - SYSTEM)) as usize;
    for (k, &ddb) in chain.iter().enumerate() {
        let next = chain.get(k + 1).copied().unwrap_or(0);
        let prev = match k {
            0 => chain[chain.len() - 1],
            _ => chain[k - 1],
        };
        bus.write_32(phys_of(ddb) + DDB_NEXT as usize, next);
        bus.write_32(phys_of(ddb) + DDB_PREV as usize, prev);
    }
}

/// VXDLDR's object type of locked code (LCODE).
const OBJECT_LOCKED_CODE: u8 = 0x01;

/// The VxD services that a program's level 0 code calls through INT 20h,
/// the VxD's id in the high word and the service in the low one.
const VMM_GET_VXD_LOCATION_LIST: u32 = 0x0001_013E;
const VMM_GET_DDB_LIST: u32 = 0x0001_013F;
const GET_DDB: u32 = 0x0001_0146;
const VXDLDR_GET_DEVICE_LIST: u32 = 0x0027_0005;

/// The VMM services of those `vxd_service` runs, which VMM's service
/// table has a stub for in VMM's code (`vmm`). A VMM service added to
/// `vxd_service` goes here too.
const RUST_VMM_SERVICES: [u32; 3] = [VMM_GET_VXD_LOCATION_LIST, VMM_GET_DDB_LIST, GET_DDB];

/// The most DDBs Get_DDB looks at, so that a list a program has made
/// circular can't keep it going.
const DDB_LIST_LIMIT: usize = 1024;

/// Run the VxD service `service` for a program's level 0 code, in its
/// context `ctx`, if the built-in Windows has it. SafeDisc 2.30
/// (Anachronox) calls these from its level 0 code, and walks the lists
/// they return.
///
/// VMM_GetVxDLocationList returns the list's address in EAX, the number
/// of VxDs in EDX and the list's size in bytes in ECX, and leaves the
/// other registers. Windows 98 SE's VMM keeps the three from its start
/// and loads them, and its flags are those of a test of EAX.
///
/// VMM_GetDDBList returns in EAX the first DDB of the list of all VxDs,
/// VMM's, which the others follow through DDB_Next.
///
/// Get_DDB finds a VxD's DDB in that list: by its id in EAX, or with EAX
/// 0 (Undefined_Device_ID) by the 8 bytes of its name at EDI. It returns
/// the DDB in ECX, or 0 if there is none, and leaves the other registers
/// as they were. As Windows 98 SE's, it returns with CF clear when it
/// finds the DDB and set when it doesn't, and the other arithmetic flags
/// as its search leaves them: all clear for a DDB found by id, ZF and PF
/// set for one found by name, and SF set when there is none.
///
/// VXDLDR_GetDeviceList returns in EAX the first of the DeviceInfo
/// blocks of the VxDs loaded since the start (dynamically loaded), each
/// linked to the next, and changes nothing else, flags included: Windows
/// 98 SE's VXDLDR loads the head of the list, as this does from
/// `DEVICE_LIST`. The built-in Windows has those of `DYNAMIC_VXDS` that
/// the machine has.
///
/// The VMM services here (`RUST_VMM_SERVICES`) are reached through VMM's
/// service table, whose entries for them are stubs in VMM's code that
/// call here; the services VMM runs as guest code aren't here at all
/// (`vmm_entry`).
///
/// Windows 98's VMM also rewrites the INT 20h and its dword into a call
/// through the VxD's service table, so that the next call goes there
/// directly. The built-in Windows leaves the code as it is: each call
/// comes back to `idt`, which looks the table up each time.
fn vxd_service(cpu: &mut Cpu, service: u32, ctx: &mut Context) -> bool {
    match service {
        VMM_GET_VXD_LOCATION_LIST => {
            let list = SYSTEM + VXD_LOCATIONS;
            ctx.gpr[EAX] = list;
            ctx.gpr[ECX] = VXD_LOCATIONS_LEN;
            ctx.gpr[EDX] = 1;
            // The flags of its `or eax, eax`. AF, which OR leaves
            // undefined, is clear, as the processors leave it.
            ctx.eflags = (ctx.eflags & !alu::ARITH) | alu::logic(4, list);
            true
        }
        VMM_GET_DDB_LIST => {
            ctx.gpr[EAX] = SYSTEM + VMM_DDB;
            true
        }
        GET_DDB => {
            let by_name = ctx.gpr[EAX] == 0;
            let ddb = find_ddb(cpu, ctx.gpr[EAX], ctx.gpr[EDI]);
            ctx.gpr[ECX] = ddb;
            let flags = match (ddb, by_name) {
                (0, _) => alu::CF | alu::SF,
                (_, true) => alu::ZF | alu::PF,
                (_, false) => 0,
            };
            ctx.eflags = (ctx.eflags & !alu::ARITH) | flags;
            true
        }
        VXDLDR_GET_DEVICE_LIST => {
            ctx.gpr[EAX] = cpu.bus.guest_read_32(SYSTEM + DEVICE_LIST);
            true
        }
        _ => false,
    }
}

/// The entry point of VMM's service `service` (VxD id and service
/// number, as INT 20h's dword has them without bit 15), from the service
/// table of VMM's DDB as memory has it, a program's hook if it replaced
/// the entry: the routines VMM runs as guest code (`vmm`) and the stubs
/// of those run in Rust. None for another VxD's service, a service past
/// the table's end or with no entry, and in the process of a state saved
/// before VMM had code, whose DDB has no table.
fn vmm_entry(cpu: &mut Cpu, service: u32) -> Option<u32> {
    if service >> 16 != 1 {
        return None;
    }
    let table = cpu.bus.guest_read_32(SYSTEM + VMM_DDB + DDB_SERVICE_TABLE);
    let size = cpu.bus.guest_read_32(SYSTEM + VMM_DDB + DDB_SERVICE_TABLE_SIZE);
    let number = service & 0xFFFF;
    if table == 0 || number >= size {
        return None;
    }
    match cpu.bus.guest_read_32(table.wrapping_add(4 * number)) {
        0 => None,
        entry => Some(entry),
    }
}

/// The DDB with the id `id` in VMM's DDB list, or with `id` 0 the one
/// whose name is the 8 bytes at `name`; 0 if there is none. The list is
/// read from memory, as a program may have changed it.
fn find_ddb(cpu: &mut Cpu, id: u32, name: u32) -> u32 {
    let read_name = |cpu: &mut Cpu, at: u32| {
        let mut bytes = [0; 8];
        cpu.bus.guest_read_bytes(at, &mut bytes);
        bytes
    };
    // EDI is read only for a search by name: by id it may be anything.
    let wanted = match id {
        0 => read_name(cpu, name),
        _ => [0; 8],
    };
    let mut ddb = SYSTEM + VMM_DDB;
    for _ in 0..DDB_LIST_LIMIT {
        if ddb == 0 {
            break;
        }
        let found = match id {
            0 => read_name(cpu, ddb + DDB_NAME) == wanted,
            _ => cpu.bus.guest_read_16(ddb + DDB_DEVICE_ID) as u32 == id,
        };
        if found {
            return ddb;
        }
        ddb = cpu.bus.guest_read_32(ddb + DDB_NEXT);
    }
    0
}

/// Let level 3 use INT n for the vectors from 10h on, in the IDT of the
/// system block at `phys`: the Win16 process's, whose tasks call DOS,
/// DPMI and the BIOS so (src/win16/dos.rs). The exceptions' vectors below
/// stay the system's.
pub fn open_soft_ints(cpu: &mut Cpu, phys: u32) {
    for vector in 0x10..256u32 {
        let at = (phys + IDT + 8 * vector + 5) as usize;
        let access = cpu.bus.read_8(at);
        cpu.bus.write_8(at, access | 0x60);
    }
}

/// Write the descriptor of a thread's TIB, at `tib`, with the selector
/// `selector`, in the LDT of the system block at `phys`.
pub fn set_tib(cpu: &mut Cpu, phys: u32, selector: u16, tib: u32) {
    write_descriptor(
        cpu,
        phys + LDT + (selector & !7) as u32,
        descriptor(tib, PAGE - 1, ACCESS_DATA3, BIG),
    );
}

/// Put the processor in the program's context `ctx`, from wherever it is:
/// protected mode with paging, the process's tables, level 3, or level 0
/// for a context in the system's code segment (`is_system`).
pub fn enter_user(cpu: &mut Cpu, ctx: &Context) {
    let process = super::running(cpu).expect("a Win32 process");
    let cr3 = process.memory.cr3;
    // The threads' TIB selectors.
    let tibs: Vec<(u16, u32)> = process
        .threads
        .iter()
        .map(|t| (t.selector, t.tib))
        .collect();
    let system = process.system;
    if !cpu.pe() {
        cpu.note_mode_switch(true);
    }
    cpu.cr3 = cr3;
    cpu.cr0 |= CR0_PE | CR0_PG;
    if cpu.model.has_sse() {
        cpu.cr4 |= CR4_OSFXSR;
    }
    cpu.tlb.flush();
    cpu.gdtr = DescTable {
        base: SYSTEM + GDT,
        limit: (GDT_ENTRIES * 8 - 1) as u16,
    };
    cpu.idtr = DescTable {
        base: SYSTEM + IDT,
        limit: 0x7FF,
    };
    cpu.ldtr = SegCache::from_descriptor(LDT_SEL, SYSTEM + LDT, 8 * LDT_ENTRIES - 1, 0x82);
    cpu.tr = SegCache::from_descriptor(TSS_SEL, SYSTEM + TSS, IOPB_END - TSS, 0x8B);
    // A program's level 0 code goes on at level 0 (`Entry::System`).
    cpu.cpl = if is_system(ctx) { 0 } else { 3 };
    load_segments(cpu, ctx, &tibs, system);
    load_registers(cpu, ctx);
}

/// Put the program's context `ctx` in the processor when it runs the
/// process's code already, at level 3: as `enter_user` without loading
/// the tables and flushing the TLB, for the switches between a Win16
/// task's 16-bit code and the 32-bit code it calls.
pub fn load_context(cpu: &mut Cpu, ctx: &Context) {
    let process = super::running(cpu).expect("a Win32 process");
    let tibs: Vec<(u16, u32)> = process
        .threads
        .iter()
        .map(|t| (t.selector, t.tib))
        .collect();
    let system = process.system;
    // From the system's handlers too (a Win16 task's INT 21h), which run
    // at level 0.
    cpu.cpl = 3;
    load_segments(cpu, ctx, &tibs, system);
    load_registers(cpu, ctx);
}

/// Load the segment registers of `ctx`: the flat selectors, the threads'
/// TIB selectors (`tibs`), and the other selectors of the LDT of the
/// system block at `system` (a Win16 task's) as their descriptors say.
fn load_segments(cpu: &mut Cpu, ctx: &Context, tibs: &[(u16, u32)], system: u32) {
    let code = SegCache::from_descriptor(CODE3, 0, 0xFFFF_FFFF, attr(ACCESS_CODE3, FLAT));
    let data = SegCache::from_descriptor(DATA3, 0, 0xFFFF_FFFF, attr(ACCESS_DATA3, FLAT));
    let system_ctx = is_system(ctx);
    for seg in Seg::ALL {
        let cache = match ctx.seg[seg as usize] {
            CODE3 => code,
            DATA3 => data,
            // The system's segments, for a program's level 0 code only.
            CODE0 if system_ctx => {
                SegCache::from_descriptor(CODE0, 0, 0xFFFF_FFFF, attr(ACCESS_CODE0, FLAT))
            }
            DATA0 if system_ctx => {
                SegCache::from_descriptor(DATA0, 0, 0xFFFF_FFFF, attr(ACCESS_DATA0, FLAT))
            }
            selector => match tibs.iter().find(|&&(s, _)| s == selector) {
                Some(&(_, tib)) => {
                    SegCache::from_descriptor(selector, tib, PAGE - 1, attr(ACCESS_DATA3, BIG))
                }
                None => ldt_segment(cpu, system, seg, selector),
            },
        };
        cpu.set_seg_cache(seg, cache);
    }
}

/// The segment `selector` selects in the LDT of the system block at
/// `system`, if it is a present segment at level 3; else a null one.
///
/// A selector freed since the context was saved (its descriptor 0)
/// comes back as 0 in a data segment register `seg`, as the DPMI host
/// of Windows zeroes the registers that hold a selector it frees
/// (`win16::ldt::write` does it for the processor's). The context of an
/// INT, of a call into 32-bit code or back to 16-bit code, or of
/// another task was saved before the free, and the selector in it
/// would fault when the program loads it again (`push es` ... `pop es`).
fn ldt_segment(cpu: &Cpu, system: u32, seg: Seg, selector: u16) -> SegCache {
    let index = selector as u32 >> 3;
    if selector & 4 == 0 || index >= LDT_ENTRIES || system == 0 {
        return SegCache::null(selector);
    }
    let at = (system + LDT + 8 * index) as usize;
    let raw = cpu.bus.read_32(at) as u64 | (cpu.bus.read_32(at + 4) as u64) << 32;
    if raw == 0 && matches!(seg, Seg::ES | Seg::DS | Seg::FS | Seg::GS) {
        return SegCache::null(0);
    }
    let desc = crate::cpu::Descriptor(raw);
    let access = desc.attr() as u8;
    if access & 0x80 == 0 || access & 0x60 != 0x60 || !desc.is_segment() {
        return SegCache::null(selector);
    }
    SegCache::from_descriptor(selector, desc.base(), desc.limit(), desc.attr())
}

/// The general registers, EIP and the flags of `ctx`.
fn load_registers(cpu: &mut Cpu, ctx: &Context) {
    let mut snapshot = cpu.snapshot();
    snapshot.gpr = ctx.gpr;
    cpu.restore(&snapshot);
    cpu.set_eip(ctx.eip);
    // IOPL 0: CLI, STI and the like are the system's. The program's code
    // at level 3 runs with interrupts on, its CLI and STI simulated
    // (seh.rs). Its level 0 code keeps the flag it had: there CLI and STI
    // change it, and Windows 98's VMM services called by INT 20h don't.
    let mut flags =
        ctx.eflags & !(CpuFlags::VM | CpuFlags::NT | CpuFlags::RF | CpuFlags::IOPL).bits();
    if !is_system(ctx) {
        flags |= CpuFlags::IF.bits();
    }
    cpu.load_eflags(flags);
    // RF, which a handler sets in a context to go on past the instruction
    // breakpoint it was called for, skips that breakpoint once. With no
    // instruction breakpoint enabled it has nothing to skip, and is left
    // clear so that it can't linger into later contexts.
    let rf = ctx.eflags & CpuFlags::RF.bits() != 0;
    cpu.set_cpu_flag(CpuFlags::RF, rf && cpu.code_breakpoints());
}

/// Put the processor in real mode at `cs:ip`, on the stack `ss:sp`, with
/// DS and ES the process's PSP and the flags `flags`.
pub fn enter_real(cpu: &mut Cpu, (cs, ip): (u16, u16), (ss, sp): (u16, u16), flags: u32) {
    let psp = dos_psp(cpu);
    let cr4 = if cpu.bus.win32.psp != 0 {
        cpu.bus.win32.cr4
    } else {
        cpu.cr4
    };
    if cpu.pe() {
        cpu.note_mode_switch(false);
    }
    cpu.cr0 &= !(CR0_PE | CR0_PG);
    cpu.cr4 = cr4;
    cpu.tlb.flush();
    cpu.cpl = 0;
    cpu.idtr = DescTable {
        base: 0,
        limit: 0x3FF,
    };
    for seg in Seg::ALL {
        let selector = match seg {
            Seg::CS => cs,
            Seg::SS => ss,
            Seg::DS | Seg::ES => psp,
            Seg::FS | Seg::GS => 0,
        };
        cpu.set_seg_cache(seg, SegCache::real(selector));
    }
    cpu.set_esp(sp as u32);
    cpu.set_eip(ip as u32);
    cpu.load_eflags(flags & !(CpuFlags::VM | CpuFlags::NT | CpuFlags::IOPL).bits());
}

/// The DOS process the programs run in: its PSP.
fn dos_psp(cpu: &Cpu) -> u16 {
    match cpu.bus.win32.psp {
        0 => cpu.current_psp,
        psp => psp,
    }
}

/// Where the processor entered an IDT's gate, from the frame it pushed.
enum Entry {
    /// The program's code at level 3, its context and the error code if
    /// one came with it: the frame is at the top of the level 0 stack.
    User(Context, Option<u32>),
    /// Code at level 0 in the system's code segment, a program's own as
    /// SafeDisc runs through the IDT, with no error code: the frame is on
    /// the stack that code was using.
    System(Context),
}

/// Where the processor entered the IDT's gate, from the frame on the
/// stack. None if it is neither of `Entry`'s: an exception with an error
/// code at level 0, or a frame that is no gate's.
///
/// The frame's CS tells them apart. A program's level 0 code that SafeDisc
/// runs uses the level 0 stack right below the frame of the exception it
/// came in by, so an interrupt there can push its frame where one from
/// level 3 would be, with or without an error code. Such a frame's second
/// dword is its CS, 28h, pushed with the upper half 0, and its first its
/// EIP, in a module's image or the system arena, above the first 64K. A
/// frame from level 3 has its CS there, with RPL 3, or the EIP of an
/// exception with an error code. That EIP is 28h when a Win16 task faults
/// at offset 28h of its segment, but the frame's first dword is then the
/// error code, which has its upper half 0. The CS is checked first, since
/// the third dword of a frame from level 0, its EFLAGS, reads as a CS with
/// RPL 3 when CF is set.
fn gate_entry(cpu: &mut Cpu) -> Option<Entry> {
    // The TSS's ESP0, where the processor put the frame from level 3:
    // `RING0_ESP`, or the stack's end in a state saved before the client
    // register structure was there.
    let top = cpu.bus.guest_read_32(SYSTEM + TSS + 4);
    let sp = cpu.esp();
    let read = |cpu: &mut Cpu, at: u32| cpu.bus.guest_read_32(at);
    if read(cpu, sp + 4) == CODE0 as u32 && read(cpu, sp) > 0xFFFF {
        let mut ctx = Context::of(cpu);
        ctx.eip = read(cpu, sp);
        ctx.seg[Seg::CS as usize] = CODE0;
        ctx.eflags = read(cpu, sp + 8);
        ctx.gpr[ESP] = sp.wrapping_add(12);
        return Some(Entry::System(ctx));
    }
    let user = |cpu: &mut Cpu, frame: u32| read(cpu, frame + 4) & 3 == 3;
    let (error, frame) = match top.wrapping_sub(sp) {
        20 if user(cpu, sp) => (None, sp),
        24 if user(cpu, sp + 4) => (Some(read(cpu, sp)), sp + 4),
        _ => return None,
    };
    let mut ctx = Context::of(cpu);
    ctx.eip = read(cpu, frame);
    ctx.seg[Seg::CS as usize] = read(cpu, frame + 4) as u16;
    ctx.eflags = read(cpu, frame + 8);
    ctx.gpr[ESP] = read(cpu, frame + 12);
    ctx.seg[Seg::SS as usize] = read(cpu, frame + 16) as u16;
    Some(Entry::User(ctx, error))
}

/// Whether `ctx` runs at level 0, in the system's code segment.
pub fn is_system(ctx: &Context) -> bool {
    ctx.seg[Seg::CS as usize] == CODE0
}

/// The vectors of the hardware interrupts, as the PICs are programmed.
fn is_irq(cpu: &Cpu, vector: u8) -> bool {
    (0..16).any(|irq| cpu.bus.pic.vector(irq) == vector)
}

/// A vector's trap in the ROM: an interrupt or exception at level 3, or a
/// hardware interrupt in a program's level 0 code.
pub fn idt(cpu: &mut Cpu, vector: u8) {
    let (ctx, error) = match gate_entry(cpu) {
        Some(Entry::User(ctx, error)) => (ctx, error),
        // A hardware interrupt while a program's level 0 code runs with
        // interrupts on, as SafeDisc's does between its two divide errors:
        // Windows 98 serves it and goes back to that code.
        Some(Entry::System(ctx)) if is_irq(cpu, vector) => {
            reflect(cpu, vector, ctx);
            return;
        }
        // INT 20h at level 0 is a VxD's call of a VMM or VxD service, its
        // number in the dword after the INT, as SafeDisc 2.30's level 0
        // code makes it. The service's bit 15 marks a VxDJmp, which
        // doesn't come back; the number is in bits 0-14. A VMM service
        // with an entry in VMM's service table is called there, as
        // Windows 98's VMM calls it through the table once it has
        // rewritten the INT 20h, so that an entry a program has replaced
        // with a hook of its own is the one that runs: returning after
        // the dword, or for a VxDJmp jumped to. The services the built-in
        // Windows runs in Rust run (`vxd_service`) when VMM's code calls
        // them, from the stubs that are their entries in the table, or
        // when the table has no entry for them (a state saved before VMM
        // had code), and the code goes on after the dword, or for a
        // VxDJmp at the return address on the stack. For any other, there
        // being no VxDs, it ends the program, naming the service, as for
        // VxDCall from level 3, and the INT, 2 bytes before the dword.
        Some(Entry::System(mut ctx)) if vector == 0x20 => {
            let service = cpu.bus.guest_read_32(ctx.eip);
            let from_vmm = (SYSTEM + VMM_CODE..SYSTEM + VXDS).contains(&ctx.eip);
            let entry = vmm_entry(cpu, service & !0x8000).filter(|_| !from_vmm);
            if let Some(entry) = entry {
                if service & 0x8000 == 0 {
                    let esp = ctx.gpr[ESP].wrapping_sub(4);
                    cpu.bus.guest_write_32(esp, ctx.eip.wrapping_add(4));
                    ctx.gpr[ESP] = esp;
                }
                ctx.eip = entry;
                enter_user(cpu, &ctx);
                return;
            }
            if vxd_service(cpu, service & !0x8000, &mut ctx) {
                if service & 0x8000 != 0 {
                    // VxDJmp: the service returns to the caller's caller,
                    // through the address at the top of the stack.
                    let esp = ctx.gpr[ESP];
                    ctx.eip = cpu.bus.guest_read_32(esp);
                    ctx.gpr[ESP] = esp.wrapping_add(4);
                } else {
                    ctx.eip = ctx.eip.wrapping_add(4);
                }
                enter_user(cpu, &ctx);
                return;
            }
            ctx.eip = ctx.eip.wrapping_sub(2);
            super::fatal(
                cpu,
                &format!(
                    "an invalid VxD call at level 0: VxD {:04X}h's service {:04X}h, \
                     which the built-in Windows doesn't have",
                    service >> 16,
                    service & 0x7FFF
                ),
                Some(&ctx),
            );
            return;
        }
        _ => {
            super::fatal(
                cpu,
                &format!("exception {:02X}h in the system", vector),
                None,
            );
            return;
        }
    };
    // A Win16 task's INT n: DOS, DPMI and the BIOS (src/win16/dos.rs).
    if error.is_none() && crate::win16::is_soft_int(cpu, vector, &ctx) {
        crate::win16::dos::interrupt(cpu, vector, ctx);
        return;
    }
    // The first PIC's vectors are exceptions 8-15 too; those push an
    // error code (#DF, #TS, #NP, #SS, #GP, #PF), the interrupts don't.
    let exception = vector < 0x20 && (vector < 8 || error.is_some() || !is_irq(cpu, vector));
    if exception {
        exception_entry(cpu, vector, error, ctx);
    } else if is_irq(cpu, vector) {
        reflect(cpu, vector, ctx);
    } else {
        // INT n the program may not use: #GP, as for a gate of level 0.
        exception_entry(cpu, 13, Some((vector as u32) << 3 | 2), ctx);
    }
}

/// An exception in the program: its handlers get it (seh.rs).
fn exception_entry(cpu: &mut Cpu, vector: u8, error: Option<u32>, ctx: Context) {
    if crate::win16::is_task_context(cpu, &ctx) {
        crate::win16::task::fault(cpu, vector, error, ctx);
        return;
    }
    super::seh::fault(cpu, vector, error, ctx);
}

/// A hardware interrupt that came while the program ran: its real-mode
/// handler runs, on the host's real-mode stack, with DS and ES the DOS
/// process's PSP, and returns to the trap at `realmode::RETURN`, which
/// goes on with the program.
fn reflect(cpu: &mut Cpu, vector: u8, ctx: Context) {
    let psp = dos_psp(cpu);
    let rm = realmode::handler_context(cpu, vector, ctx.gpr, psp, 0x0002);
    // The handler's IRET comes back to the trap, interrupts still off.
    let (seg, off) = realmode::RETURN;
    let frame = realmode::Frame::new(realmode::Kind::Irq, ctx);
    realmode::call(cpu, frame, &rm, &[0x0002, seg, off]);
}

/// The trap ids of the IDT's vectors.
pub fn idt_id(vector: u8) -> u16 {
    ID_IDT | vector as u16
}

#[cfg(test)]
mod tests {
    use super::super::memory::{Memory, Pool};
    use super::super::vmm::compared;
    use super::*;

    /// The SHA-256 digest of `data` in lowercase hex, for the strings
    /// VMM's object must not hold, which are known only by their digests.
    fn sha256_hex(data: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        Sha256::digest(data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// VMM's locked code object, as the location list reports it, of a
    /// system block built as a process's is.
    fn vmm_object() -> Vec<u8> {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        cpu.bus.win32.pool = Pool::take(&mut cpu.bus, 0x40_0000);
        let mut memory = Memory::new(&mut cpu.bus).expect("a page directory");
        let phys = memory
            .pages(&mut cpu.bus, SYSTEM_BLOCK_PAGES)
            .expect("the system block's pages");
        build_tables(&mut cpu, &mut memory, phys, 0).expect("the system block");
        let mut ctx = Context::default();
        assert!(vxd_service(&mut cpu, VMM_GET_VXD_LOCATION_LIST, &mut ctx));
        // The block's linear addresses, at its physical ones.
        let at = |lin: u32| (phys + (lin - SYSTEM)) as usize;
        let list = at(ctx.gpr[EAX]);
        assert_eq!(cpu.bus.read_32(list), SYSTEM + VMM_DDB, "VMM's record first");
        assert!(cpu.bus.read_8(list + 4) >= 1, "VMM has an object");
        let (address, size) = (cpu.bus.read_32(list + 5), cpu.bus.read_32(list + 9));
        assert_eq!(cpu.bus.read_8(list + 13), OBJECT_LOCKED_CODE);
        (0..size as usize).map(|i| cpu.bus.read_8(at(address) + i)).collect()
    }

    /// The bytes of Windows 98 SE's VMM that programs compare exactly
    /// (`vmm::compared`) are in VMM's locked code object, where programs
    /// scan it: at a start more than 128 bytes before its end. The four
    /// strings of Windows' debugger and debug builds that programs expect
    /// not to find there, known here by their lengths and SHA-256
    /// digests, aren't.
    #[test]
    fn vmm_s_object_holds_what_programs_compare_and_not_what_they_reject() {
        let object = vmm_object();
        let window = object.len() - 128;
        let found = |item: &[u8]| (0..window).any(|at| object[at..].starts_with(item));
        assert!(found(&compared::SIMULATE_IRET_PM16), "Simulate_Iret's code");
        assert!(found(compared::DEBUGGER_WELCOME), "the debugger's greeting");
        assert!(found(compared::DEBUGGER_PROMPT), "the debugger's prompt");
        assert!(found(&compared::READY_QUEUE_SIGNATURE), "the ready queue");
        let absent: [(usize, &str); 4] = [
            (
                17,
                "8244268bddd3881d7e99bbbf245a510e83672d0b98c05245faeb209a0c90433f",
            ),
            (
                20,
                "8dbf0ddc1f1ba7b7f5c625e76b4e603674b65fc9915a856da860d9e78a9d83e0",
            ),
            (
                26,
                "7cf1d01d4a19410eb1d27b399c050cd224cb4b370cedb618a9f2a972fb871d75",
            ),
            (
                27,
                "d0d59e32a74b9d305962a1c69e4284388d31beefc4d302936d110d2ea327b707",
            ),
        ];
        for (len, digest) in absent {
            let at = (0..window)
                .filter(|&at| at + len <= object.len())
                .find(|&at| sha256_hex(&object[at..at + len]) == digest);
            assert_eq!(at, None, "a string of {len} bytes that must be absent");
        }
    }
}
