//! Expanded memory (LIM EMS 4.0, INT 67h): how programs find it, the page
//! frame, handles, maps, moves, and the extended memory it shares with XMS.

use iced_x86::Register;
use rust_dos::dos_data::{NUL_DEVICE, address};
use rust_dos::cpu::{Cpu, CpuFlags};
use rust_dos::ems;
use rust_dos::interrupts::int21;
use rust_dos::xms;
use std::path::PathBuf;

const FRAME: usize = 0xE0000;
const PAGE: usize = 0x4000;

fn machine() -> Cpu {
    let mut cpu = Cpu::new(PathBuf::from("."));
    ems::set_enabled(&mut cpu.bus, true);
    cpu
}

/// Call INT 67h with AX; returns the status in AH.
fn ems(cpu: &mut Cpu, ax: u16) -> u8 {
    cpu.set_ax(ax);
    ems::handle(cpu);
    cpu.get_ah()
}

/// A new handle with `pages` pages.
fn allocate(cpu: &mut Cpu, pages: u16) -> u16 {
    cpu.set_bx(pages);
    assert_eq!(ems(cpu, 0x4300), 0);
    cpu.dx()
}

/// Map logical page `page` of `handle` into physical page `slot`.
fn map(cpu: &mut Cpu, handle: u16, page: u16, slot: u8) -> u8 {
    cpu.set_bx(page);
    cpu.set_dx(handle);
    ems(cpu, 0x4400 | slot as u16)
}

/// Run from CS:IP until a HLT, through service traps.
fn run_to_hlt(cpu: &mut Cpu) {
    for _ in 0..1000 {
        let at = cpu.get_physical_addr(cpu.cs(), cpu.ip());
        if cpu.bus.read_8(at) == 0xF4 {
            return;
        }
        cpu.step();
    }
    panic!("no HLT reached");
}

/// Run the code `code` at 0000:0100 up to its HLT.
fn run(cpu: &mut Cpu, code: &[u8]) {
    cpu.set_ss(0);
    cpu.set_sp(0x8000);
    cpu.set_ds(0);
    cpu.set_cs(0);
    cpu.set_ip(0x100);
    cpu.bus.load_bytes(0x100, code);
    run_to_hlt(cpu);
}

fn int21(cpu: &mut Cpu, ax: u16) -> bool {
    cpu.set_ax(ax);
    int21::handle(cpu);
    !cpu.get_cpu_flag(CpuFlags::CF)
}

/// Point DS:DX at an ASCIIZ `name` at 2000:0000.
fn set_name(cpu: &mut Cpu, name: &str) {
    cpu.bus.load_bytes(0x20000, name.as_bytes());
    cpu.bus.write_8(0x20000 + name.len(), 0);
    cpu.set_ds(0x2000);
    cpu.set_dx(0);
}

#[test]
fn ems_is_found_by_vector_and_by_device() {
    let mut cpu = machine();
    // The name at offset 0Ah of INT 67h's segment.
    let segment = cpu.bus.read_16(0x67 * 4 + 2) as usize;
    let name: Vec<u8> = (0..8).map(|i| cpu.bus.read_8(segment * 16 + 0x0A + i)).collect();
    assert_eq!(&name, b"EMMXXXX0");
    // Through INT 67h itself: get status, then the page frame.
    #[rustfmt::skip]
    run(&mut cpu, &[
        0xB4, 0x40,             // MOV AH, 40h
        0xCD, 0x67,             // INT 67h
        0x88, 0x26, 0x00, 0x06, // MOV [0600h], AH
        0xB4, 0x41,             // MOV AH, 41h
        0xCD, 0x67,             // INT 67h
        0x89, 0x1E, 0x02, 0x06, // MOV [0602h], BX
        0xF4,                   // HLT
    ]);
    assert_eq!(cpu.bus.read_8(0x600), 0);
    assert_eq!(cpu.bus.read_16(0x602), 0xE000);

    // Opened, it is a character device that is ready.
    set_name(&mut cpu, "EMMXXXX0");
    assert!(int21(&mut cpu, 0x3D00));
    let handle = cpu.ax();
    cpu.set_bx(handle);
    assert!(int21(&mut cpu, 0x4400));
    assert_eq!(cpu.dx() & 0x80, 0x80, "a character device");
    cpu.set_bx(handle);
    assert!(int21(&mut cpu, 0x4407));
    assert_eq!(cpu.get_al(), 0xFF);
    cpu.set_bx(handle);
    assert!(int21(&mut cpu, 0x3E00));

    // In the device chain, after NUL.
    assert_eq!(&after_nul(&cpu), b"EMMXXXX0");
}

/// The name of the device driver after NUL.
fn after_nul(cpu: &Cpu) -> Vec<u8> {
    let nul = address(NUL_DEVICE);
    let next = cpu.get_physical_addr(cpu.bus.read_16(nul + 2), cpu.bus.read_16(nul));
    (0..8).map(|i| cpu.bus.read_8(next + 0x0A + i)).collect()
}

#[test]
fn without_ems_int67_says_84_and_the_device_is_gone() {
    let mut cpu = machine();
    ems::set_enabled(&mut cpu.bus, false);
    assert_eq!(ems(&mut cpu, 0x4000), 0x84);
    let segment = cpu.bus.read_16(0x67 * 4 + 2) as usize;
    let name: Vec<u8> = (0..8).map(|i| cpu.bus.read_8(segment * 16 + 0x0A + i)).collect();
    assert_ne!(&name, b"EMMXXXX0");
    set_name(&mut cpu, "EMMXXXX0");
    assert!(!int21(&mut cpu, 0x3D00));
    assert_eq!(&after_nul(&cpu), b"CON     ", "NUL leads to DOS's own devices again");
}

#[test]
fn status_frame_counts_and_version() {
    let mut cpu = machine();
    assert_eq!(ems(&mut cpu, 0x4000), 0);
    assert_eq!(ems(&mut cpu, 0x4100), 0);
    assert_eq!(cpu.bx(), 0xE000);
    assert_eq!(ems(&mut cpu, 0x4200), 0);
    // 16 MB, less the first megabyte and the HMA.
    assert_eq!((cpu.bx(), cpu.dx()), (956, 956));
    assert_eq!(ems(&mut cpu, 0x4600), 0);
    assert_eq!(cpu.get_al(), 0x40, "version 4.0");
    // Handles and their pages.
    let handle = allocate(&mut cpu, 3);
    assert_ne!(handle, 0, "handle 0 is the system's");
    assert_eq!(ems(&mut cpu, 0x4200), 0);
    assert_eq!(cpu.bx(), 953);
    assert_eq!(ems(&mut cpu, 0x4B00), 0);
    assert_eq!(cpu.bx(), 2);
    cpu.set_dx(handle);
    assert_eq!(ems(&mut cpu, 0x4C00), 0);
    assert_eq!(cpu.bx(), 3);
    cpu.set_es(0x3000);
    cpu.set_di(0);
    assert_eq!(ems(&mut cpu, 0x4D00), 0);
    assert_eq!(cpu.bx(), 2);
    assert_eq!((cpu.bus.read_16(0x30004), cpu.bus.read_16(0x30006)), (handle, 3));
    // Zero pages, too many, and freeing.
    cpu.set_bx(0);
    assert_eq!(ems(&mut cpu, 0x4300), 0x89);
    cpu.set_bx(2000);
    assert_eq!(ems(&mut cpu, 0x4300), 0x87);
    cpu.set_dx(handle);
    assert_eq!(ems(&mut cpu, 0x4500), 0);
    cpu.set_dx(handle);
    assert_eq!(ems(&mut cpu, 0x4500), 0x83);
    assert_eq!(ems(&mut cpu, 0x4200), 0);
    assert_eq!(cpu.bx(), 956);
}

#[test]
fn mapped_pages_keep_their_contents_across_remaps() {
    let mut cpu = machine();
    let handle = allocate(&mut cpu, 4);
    assert_eq!(map(&mut cpu, handle, 0, 0), 0);
    // Code for page 0, then for page 1, through the frame.
    cpu.bus.load_bytes(FRAME, &[0xB0, 0x01, 0xF4]); // MOV AL, 1; HLT
    assert_eq!(map(&mut cpu, handle, 1, 0), 0);
    assert_eq!(cpu.bus.read_8(FRAME), 0, "a new page is blank");
    cpu.bus.load_bytes(FRAME, &[0xB0, 0x02, 0xF4]); // MOV AL, 2; HLT
    let run_frame = |cpu: &mut Cpu| {
        cpu.set_cs(0xE000);
        cpu.set_ip(0);
        cpu.set_ax(0);
        cpu.step();
        cpu.get_al()
    };
    assert_eq!(run_frame(&mut cpu), 2);
    assert_eq!(map(&mut cpu, handle, 0, 0), 0);
    assert_eq!(run_frame(&mut cpu), 1, "the code of the page mapped now runs");
    // The same page in another physical page shows what was written.
    assert_eq!(map(&mut cpu, handle, 1, 3), 0);
    assert_eq!(cpu.bus.read_8(FRAME + 3 * PAGE + 1), 2);
    // Unmapping (FFFFh), and mistakes.
    assert_eq!(map(&mut cpu, handle, 0xFFFF, 3), 0);
    assert_eq!(map(&mut cpu, handle, 4, 0), 0x8A);
    assert_eq!(map(&mut cpu, handle, 0, 4), 0x8B);
    assert_eq!(map(&mut cpu, 99, 0, 0), 0x83);
    // A second handle's page, in the frame by segment (function 50h).
    let other = allocate(&mut cpu, 1);
    cpu.bus.load_bytes(0x31000, &[0, 0, 0x00, 0xE8]); // page 0 at E800h
    cpu.set_ds(0x3100);
    cpu.set_si(0);
    cpu.set_cx(1);
    cpu.set_dx(other);
    assert_eq!(ems(&mut cpu, 0x5001), 0);
    cpu.bus.write_8(FRAME + 2 * PAGE, 0x77);
    assert_eq!(map(&mut cpu, handle, 2, 2), 0);
    assert_eq!(map(&mut cpu, other, 0, 1), 0);
    assert_eq!(cpu.bus.read_8(FRAME + PAGE), 0x77);
}

#[test]
fn move_and_exchange_reach_mapped_and_unmapped_pages() {
    let mut cpu = machine();
    let handle = allocate(&mut cpu, 3);
    assert_eq!(map(&mut cpu, handle, 1, 0), 0);
    // Conventional memory at 4000:0000 to handle's page 1 at 3FFEh, over
    // into page 2: the mapped page is written in the frame.
    cpu.bus.load_bytes(0x40000, b"ABCD");
    let describe = |cpu: &mut Cpu, len: u32, source: (u8, u16, u16, u16), dest: (u8, u16, u16, u16)| {
        let at = 0x50000;
        cpu.bus.write_32(at, len);
        for (base, (kind, handle, offset, segment)) in [(at + 4, source), (at + 0x0B, dest)] {
            cpu.bus.write_8(base, kind);
            cpu.bus.write_16(base + 1, handle);
            cpu.bus.write_16(base + 3, offset);
            cpu.bus.write_16(base + 5, segment);
        }
        cpu.set_ds(0x5000);
        cpu.set_si(0);
    };
    describe(&mut cpu, 4, (0, 0, 0, 0x4000), (1, handle, 0x3FFE, 1));
    assert_eq!(ems(&mut cpu, 0x5700), 0);
    assert_eq!(cpu.bus.read_8(FRAME + 0x3FFE), b'A');
    assert_eq!(cpu.bus.read_8(FRAME + 0x3FFF), b'B');
    assert_eq!(map(&mut cpu, handle, 2, 1), 0);
    assert_eq!(cpu.bus.read_8(FRAME + PAGE), b'C');

    // Exchange page 2's start with conventional memory.
    cpu.bus.load_bytes(0x40000, b"wxyz");
    describe(&mut cpu, 2, (1, handle, 0, 2), (0, 0, 0, 0x4000));
    assert_eq!(ems(&mut cpu, 0x5701), 0);
    assert_eq!(cpu.bus.read_8(0x40000), b'C');
    assert_eq!(cpu.bus.read_8(0x40001), b'D');
    assert_eq!(cpu.bus.read_8(FRAME + PAGE), b'w');

    // To video memory.
    describe(&mut cpu, 2, (1, handle, 0x3FFE, 1), (0, 0, 0, 0xB800));
    assert_eq!(ems(&mut cpu, 0x5700), 0);
    assert_eq!(cpu.bus.read_8(0xB8000), b'A');

    // Overlapping moves within a handle work and say so; exchanges don't.
    describe(&mut cpu, 8, (1, handle, 0x3FF0, 1), (1, handle, 0x3FF4, 1));
    assert_eq!(ems(&mut cpu, 0x5700), 0x92);
    assert_eq!(ems(&mut cpu, 0x5701), 0x97);
    // Past the handle's pages, and an offset past a page.
    describe(&mut cpu, 0x10, (1, handle, 0x3FF8, 2), (0, 0, 0, 0x4000));
    assert_eq!(ems(&mut cpu, 0x5700), 0x93);
    describe(&mut cpu, 1, (1, handle, 0x4000, 0), (0, 0, 0, 0x4000));
    assert_eq!(ems(&mut cpu, 0x5700), 0x95);
}

#[test]
fn page_maps_save_restore_and_4e_4f() {
    let mut cpu = machine();
    let handle = allocate(&mut cpu, 4);
    for page in 0..4 {
        assert_eq!(map(&mut cpu, handle, page, page as u8), 0);
        cpu.bus.write_8(FRAME + page as usize * PAGE, 0x10 + page as u8);
    }
    // 47h/48h, as an interrupt handler saves and restores the map.
    cpu.set_dx(handle);
    assert_eq!(ems(&mut cpu, 0x4700), 0);
    cpu.set_dx(handle);
    assert_eq!(ems(&mut cpu, 0x4700), 0x8D);
    assert_eq!(map(&mut cpu, handle, 3, 0), 0);
    assert_eq!(cpu.bus.read_8(FRAME), 0x13);
    cpu.set_dx(handle);
    assert_eq!(ems(&mut cpu, 0x4800), 0);
    assert_eq!(cpu.bus.read_8(FRAME), 0x10);
    cpu.set_dx(handle);
    assert_eq!(ems(&mut cpu, 0x4800), 0x8E);

    // 4Eh: the whole map to ES:DI and back from DS:SI.
    assert_eq!(ems(&mut cpu, 0x4E03), 0);
    assert_eq!(cpu.get_al(), 16);
    cpu.set_es(0x3000);
    cpu.set_di(0);
    assert_eq!(ems(&mut cpu, 0x4E00), 0);
    assert_eq!((cpu.bus.read_16(0x30008), cpu.bus.read_16(0x3000A)), (handle, 2));
    assert_eq!(map(&mut cpu, handle, 0xFFFF, 2), 0);
    assert_eq!(map(&mut cpu, handle, 0, 1), 0);
    cpu.set_ds(0x3000);
    cpu.set_si(0);
    assert_eq!(ems(&mut cpu, 0x4E01), 0);
    for page in 0..4 {
        assert_eq!(cpu.bus.read_8(FRAME + page * PAGE), 0x10 + page as u8);
    }

    // 4Fh: the map of the pages at E400h and EC00h only.
    cpu.bus.load_bytes(0x31000, &[2, 0, 0x00, 0xE4, 0x00, 0xEC]);
    cpu.set_ds(0x3100);
    cpu.set_si(0);
    cpu.set_es(0x3200);
    cpu.set_di(0);
    assert_eq!(ems(&mut cpu, 0x4F00), 0);
    cpu.set_bx(2);
    assert_eq!(ems(&mut cpu, 0x4F02), 0);
    assert_eq!(cpu.get_al(), 14);
    assert_eq!(map(&mut cpu, handle, 0, 1), 0);
    assert_eq!(map(&mut cpu, handle, 0, 3), 0);
    cpu.set_ds(0x3200);
    cpu.set_si(0);
    assert_eq!(ems(&mut cpu, 0x4F01), 0);
    assert_eq!(cpu.bus.read_8(FRAME + PAGE), 0x11);
    assert_eq!(cpu.bus.read_8(FRAME + 3 * PAGE), 0x13);

    // 58h: the mappable segments.
    cpu.set_es(0x3300);
    cpu.set_di(0);
    assert_eq!(ems(&mut cpu, 0x5800), 0);
    assert_eq!(cpu.cx(), 4);
    assert_eq!((cpu.bus.read_16(0x3300C), cpu.bus.read_16(0x3300E)), (0xEC00, 3));
}

#[test]
fn reallocate_names_and_directory() {
    let mut cpu = machine();
    let handle = allocate(&mut cpu, 2);
    assert_eq!(map(&mut cpu, handle, 1, 0), 0);
    cpu.bus.write_8(FRAME, 0x55);
    // Growing keeps the pages, shrinking unmaps what goes.
    cpu.set_dx(handle);
    cpu.set_bx(5);
    assert_eq!(ems(&mut cpu, 0x5100), 0);
    assert_eq!(cpu.bx(), 5);
    assert_eq!(map(&mut cpu, handle, 4, 1), 0);
    assert_eq!(map(&mut cpu, handle, 1, 1), 0);
    assert_eq!(cpu.bus.read_8(FRAME + PAGE), 0x55);
    cpu.set_dx(handle);
    cpu.set_bx(1);
    assert_eq!(ems(&mut cpu, 0x5100), 0);
    assert_eq!(map(&mut cpu, handle, 1, 0), 0x8A);

    // Names: set, find, list, and no two alike.
    cpu.bus.load_bytes(0x30000, b"GAMEDATA");
    cpu.set_ds(0x3000);
    cpu.set_si(0);
    cpu.set_dx(handle);
    assert_eq!(ems(&mut cpu, 0x5301), 0);
    let other = allocate(&mut cpu, 0x1);
    cpu.set_dx(other);
    assert_eq!(ems(&mut cpu, 0x5301), 0xA1);
    assert_eq!(ems(&mut cpu, 0x5401), 0);
    assert_eq!(cpu.dx(), handle);
    cpu.set_es(0x3100);
    cpu.set_di(0);
    cpu.set_dx(handle);
    assert_eq!(ems(&mut cpu, 0x5300), 0);
    assert_eq!(cpu.bus.read_8(0x31000), b'G');
    cpu.set_es(0x3200);
    cpu.set_di(0);
    assert_eq!(ems(&mut cpu, 0x5400), 0);
    assert_eq!(cpu.get_al(), 3);
    assert_eq!(cpu.bus.read_16(0x3200A), handle);
    assert_eq!(cpu.bus.read_8(0x3200C), b'G');
    assert_eq!(ems(&mut cpu, 0x5402), 0);
    assert_eq!(cpu.bx(), 255);
    // Every handle is volatile.
    cpu.set_dx(handle);
    assert_eq!(ems(&mut cpu, 0x5200), 0);
    assert_eq!(cpu.get_al(), 0);
    cpu.set_dx(handle);
    cpu.set_reg8(Register::BL, 1);
    assert_eq!(ems(&mut cpu, 0x5201), 0x91);
    // 5Ah takes zero pages.
    cpu.set_bx(0);
    assert_eq!(ems(&mut cpu, 0x5A00), 0);
}

#[test]
fn ems_and_xms_share_extended_memory() {
    let mut cpu = machine();
    let xms_free = |cpu: &mut Cpu| {
        cpu.set_reg8(Register::AH, 0x08);
        xms::call(cpu);
        cpu.dx()
    };
    let before = xms_free(&mut cpu);
    allocate(&mut cpu, 4);
    assert_eq!(xms_free(&mut cpu), before - 64);
    // XMS taking all of it leaves EMS without pages.
    cpu.set_reg8(Register::AH, 0x08);
    xms::call(&mut cpu);
    let largest = cpu.ax();
    cpu.set_dx(largest);
    cpu.set_reg8(Register::AH, 0x09);
    xms::call(&mut cpu);
    assert_eq!(cpu.ax(), 1);
    assert_eq!(ems(&mut cpu, 0x4200), 0);
    assert_eq!(cpu.bx(), 0);
    cpu.set_bx(1);
    assert_eq!(ems(&mut cpu, 0x4300), 0x88);
}

#[test]
fn vcpi_is_not_there() {
    let mut cpu = machine();
    assert_eq!(ems(&mut cpu, 0xDE00), 0x84);
    assert_eq!(ems(&mut cpu, 0x5500), 0x84);
}

#[test]
fn the_shell_frees_ems() {
    let mut cpu = machine();
    allocate(&mut cpu, 10);
    cpu.load_shell();
    assert_eq!(ems(&mut cpu, 0x4B00), 0);
    assert_eq!(cpu.bx(), 1);
    assert_eq!(ems(&mut cpu, 0x4200), 0);
    assert_eq!(cpu.bx(), cpu.dx());
}
