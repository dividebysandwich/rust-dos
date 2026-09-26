//! BOOT: a disk image's boot sector runs on a machine with the BIOS alone,
//! reading its disk through INT 13h, the memory size through INT 15h and
//! keystrokes that the BIOS's keyboard interrupt made from scan codes, and
//! Ctrl+Alt+Del boots it again.

use rust_dos::asm16::Asm;
use rust_dos::cpu::Cpu;
use rust_dos::diskimage::{DiskImage, SECTOR_SIZE};
use rust_dos::disk::MountOptions;
use rust_dos::exec::{self, NoHook};
use rust_dos::keyboard::key_event;
use std::path::PathBuf;

/// Where the boot sector leaves what it found.
const UNIT: usize = 0x0600;
const HARD_DISKS: usize = 0x0601;
const EXTENSIONS: usize = 0x0602;
const EXTENDED_KB: usize = 0x0604;
const KEY: usize = 0x0606;
const WAITING: usize = 0x0610;
const SECTOR: usize = 0x0800;

/// A boot sector that stores its unit, the hard disk count (INT 13h
/// AH=08h), the extensions' signature (AH=41h), reads sector 1 to 0:0800
/// (AH=42h), the extended memory (INT 15h AH=88h), and then a key (INT 16h
/// AH=00h).
fn boot_sector() -> [u8; SECTOR_SIZE] {
    let mut a = Asm::new(0x7C00);
    a.op(&[0x31, 0xC0, 0x8E, 0xD8, 0x8E, 0xC0]); // XOR AX, AX; MOV DS, AX; MOV ES, AX
    a.op(&[0x88, 0x16, 0x00, 0x06]); // MOV [0600h], DL
    a.op(&[0xB4, 0x08, 0xCD, 0x13]); // MOV AH, 08h; INT 13h
    a.op(&[0x88, 0x16, 0x01, 0x06]); // MOV [0601h], DL
    a.op(&[0xB4, 0x41, 0xBB, 0xAA, 0x55, 0xB2, 0x80, 0xCD, 0x13]); // AH=41h, BX=55AAh, DL=80h
    a.op(&[0x89, 0x1E, 0x02, 0x06]); // MOV [0602h], BX
    // The disk address packet at 0:0700: 16 bytes, one sector to 0000:0800
    // from sector 1.
    for (offset, value) in [(0u8, 0x0010u16), (2, 1), (4, 0x0800), (6, 0), (8, 1), (10, 0), (12, 0), (14, 0)] {
        let [lo, hi] = value.to_le_bytes();
        a.op(&[0xC7, 0x06, offset, 0x07, lo, hi]); // MOV WORD [0700h+offset], value
    }
    a.op(&[0xBE, 0x00, 0x07, 0xB4, 0x42, 0xB2, 0x80, 0xCD, 0x13]); // SI=0700h, AH=42h, DL=80h
    a.op(&[0xB4, 0x88, 0xCD, 0x15]); // MOV AH, 88h; INT 15h
    a.op(&[0xA3, 0x04, 0x06]); // MOV [0604h], AX
    a.op(&[0xC6, 0x06, 0x10, 0x06, 0x01]); // MOV BYTE [0610h], 1
    a.op(&[0xFB, 0x30, 0xE4, 0xCD, 0x16]); // STI; XOR AH, AH; INT 16h
    a.op(&[0xA3, 0x06, 0x06]); // MOV [0606h], AX
    a.label("hang");
    a.jump(0xEB, "hang");
    let code = a.finish();
    let mut sector = [0u8; SECTOR_SIZE];
    sector[..code.len()].copy_from_slice(&code);
    sector[510] = 0x55;
    sector[511] = 0xAA;
    sector
}

/// A machine with a hard disk image on C: whose master boot record is
/// `boot_sector` and whose sector 1 holds the bytes 0 to 255 twice.
fn machine(name: &str) -> Cpu {
    let base = PathBuf::from("target/test_boot").join(name);
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let mut cpu = Cpu::new(base);
    cpu.bus.set_cycles_per_ms(1000);
    cpu.load_shell();
    let disk = DiskImage::blank_hard_disk("boot.img", 8 << 20, None).unwrap();
    // The code in place of the master boot record's, before its partition
    // table.
    let mut mbr = [0u8; SECTOR_SIZE];
    disk.read(0, &mut mbr).unwrap();
    mbr[..0x1BE].copy_from_slice(&boot_sector()[..0x1BE]);
    disk.write(0, &mbr).unwrap();
    let pattern: Vec<u8> = (0..SECTOR_SIZE).map(|i| i as u8).collect();
    disk.write(1, &pattern).unwrap();
    cpu.bus.mount_disk_image(2, disk, MountOptions::default()).unwrap();
    cpu
}

/// Run the machine for up to `ms` ms of emulated time, until `stop`.
fn run_until(cpu: &mut Cpu, ms: u64, stop: impl Fn(&Cpu) -> bool) -> bool {
    for _ in 0..ms {
        if stop(cpu) {
            return true;
        }
        let end = cpu.bus.clock.icount + 1000;
        cpu.bus.start_batch(end);
        exec::run_batch(cpu, &mut NoHook, false);
    }
    stop(cpu)
}

fn press(cpu: &mut Cpu, scan: u8, extended: bool) {
    key_event(&mut cpu.bus, scan, extended, true, None);
}

fn release(cpu: &mut Cpu, scan: u8, extended: bool) {
    key_event(&mut cpu.bus, scan, extended, false, None);
}

#[test]
fn the_boot_sector_runs_on_the_bios() {
    let mut cpu = machine("bios");
    exec::run_command_line(&mut cpu, "BOOT -l C");
    assert!(cpu.bus.boot.is_some());
    assert!(run_until(&mut cpu, 1000, |cpu| cpu.bus.read_8(WAITING) == 1), "the boot sector runs");
    assert_eq!(cpu.bus.read_8(UNIT), 0x80);
    assert_eq!(cpu.bus.read_8(HARD_DISKS), 1);
    assert_eq!(cpu.bus.read_16(EXTENSIONS), 0xAA55);
    let sector: Vec<u8> = (0..SECTOR_SIZE).map(|i| cpu.bus.read_8(SECTOR + i)).collect();
    assert_eq!(sector, (0..SECTOR_SIZE).map(|i| i as u8).collect::<Vec<_>>());
    assert_eq!(cpu.bus.read_16(EXTENDED_KB), 15 * 1024);
    // No built-in DOS: no shell, whatever runs at 0070h.
    assert!(!cpu.shell_idle());

    // A key: its scan codes through the keyboard controller, IRQ 1 and the
    // BIOS's keyboard interrupt, into the buffer INT 16h reads.
    press(&mut cpu, 0x1E, false);
    release(&mut cpu, 0x1E, false);
    assert!(run_until(&mut cpu, 1000, |cpu| cpu.bus.read_16(KEY) != 0), "INT 16h returns the key");
    assert_eq!(cpu.bus.read_16(KEY), 0x1E61);
}

#[test]
fn shift_and_ctrl_change_the_keystroke() {
    let mut cpu = machine("shift");
    exec::run_command_line(&mut cpu, "BOOT -l C");
    assert!(run_until(&mut cpu, 1000, |cpu| cpu.bus.read_8(WAITING) == 1));
    press(&mut cpu, 0x2A, false);
    press(&mut cpu, 0x1E, false);
    release(&mut cpu, 0x1E, false);
    release(&mut cpu, 0x2A, false);
    assert!(run_until(&mut cpu, 1000, |cpu| cpu.bus.read_16(KEY) != 0));
    assert_eq!(cpu.bus.read_16(KEY), 0x1E41, "Shift+A");
}

#[test]
fn ctrl_alt_del_boots_again() {
    let mut cpu = machine("reboot");
    exec::run_command_line(&mut cpu, "BOOT -l C");
    assert!(run_until(&mut cpu, 1000, |cpu| cpu.bus.read_8(WAITING) == 1));
    cpu.bus.write_8(UNIT, 0x55);
    cpu.bus.write_8(WAITING, 0);
    press(&mut cpu, 0x1D, false);
    press(&mut cpu, 0x38, false);
    press(&mut cpu, 0x53, true);
    assert!(run_until(&mut cpu, 1000, |cpu| cpu.bus.read_8(WAITING) == 1), "the boot sector runs again");
    assert_eq!(cpu.bus.read_8(UNIT), 0x80);
    assert!(cpu.bus.boot.is_some());
}

#[test]
fn turning_off_brings_dos_back() {
    let mut cpu = machine("off");
    exec::run_command_line(&mut cpu, "BOOT -l C");
    assert!(run_until(&mut cpu, 1000, |cpu| cpu.bus.read_8(WAITING) == 1));
    rust_dos::boot::power_off(&mut cpu, "test");
    assert!(run_until(&mut cpu, 1000, |cpu| cpu.shell_idle()), "the prompt is back");
    assert!(cpu.bus.boot.is_none());
    // DOS's vectors are there again.
    let int21 = ((cpu.bus.read_16(0x86) as usize) << 4) + cpu.bus.read_16(0x84) as usize;
    assert_eq!(cpu.bus.read_8(int21), 0xFE);
}
