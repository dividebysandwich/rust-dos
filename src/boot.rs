//! Booting an operating system from a disk image, as a PC's BIOS does
//! (BOOT): the machine starts over as at power-on, with the BIOS's services
//! and no built-in DOS, and runs the boot sector of the disk in the unit it
//! boots from. DOSBox's BOOT does the same (DOSBox-X dos_programs.cpp).
//!
//! The BIOS units of a booted machine are the disk images: 00h and 01h the
//! floppy drives A: and B:, with or without a disk in them, and 80h up the
//! hard disk images in drive-letter order. Disks mounted by number take
//! their units first: 0 and 1 are 00h and 01h, 2 and 3 are 80h and 81h
//! (`DiskController::hard_disk_units`). Host directories on D: and up
//! become hard disks after those (`shared_disk`), and the first CD-ROM
//! drive with an image or a host directory is an ATAPI drive on the
//! secondary IDE channel (`ide`). The operating system has the
//! machine until it turns it off or its disk can't be booted any more;
//! then the built-in DOS starts again (`Cpu::load_shell`).

use crate::bus::Bus;
use crate::cpu::{Cpu, CpuFlags, CpuState};
use crate::diskimage::SECTOR_SIZE;
use crate::disk::{DRIVE_C, drive_name, drive_number, numbered_drive};

/// D:, which BOOT -l takes for the disk mounted as 3.
const DRIVE_D: u8 = 3;

/// A machine an operating system booted on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BootState {
    /// The BIOS unit it booted from: 00h for A:, 80h for the first hard
    /// disk. Restarts boot from it again.
    pub unit: u8,
    /// How the system connected to the APM BIOS (`apm`).
    pub apm: crate::apm::Connection,
}

crate::state_fields!(BootState { unit, apm });

/// The floppy units of a booted machine, A: and B:, which are there with
/// or without a disk.
pub const FLOPPY_UNITS: u8 = 2;

/// Where the boot sector goes, and where its code starts.
const BOOT_SECTOR: usize = 0x7C00;

/// The drives with hard disk images, which are BIOS units 80h up in this
/// order: the images, then the disks made of shared host directories.
pub fn hard_disk_drives(bus: &Bus) -> Vec<u8> {
    let mut drives =
        bus.disk.hard_disk_units(|drive| bus.disk.bios_image(drive).is_some() && !bus.disk.is_shared(drive));
    drives.extend(bus.disk.shared_drives());
    drives
}

/// The drive behind BIOS unit `unit` of a booted machine, if it has one:
/// the disk mounted as 0 or 1, or else A: or B:, for 00h and 01h (which
/// may be empty), a hard disk image's for 80h up.
pub fn unit_drive(bus: &Bus, unit: u8) -> Option<u8> {
    if unit < 0x80 {
        (unit < FLOPPY_UNITS).then(|| bus.disk.floppy_unit(unit))
    } else {
        hard_disk_drives(bus).get((unit - 0x80) as usize).copied()
    }
}

/// The BIOS unit a drive is, if it is one.
pub fn drive_unit(bus: &Bus, drive: u8) -> Option<u8> {
    if let Some(unit) = (0..FLOPPY_UNITS).find(|&unit| bus.disk.floppy_unit(unit) == drive) {
        return Some(unit);
    }
    let index = hard_disk_drives(bus).iter().position(|&d| d == drive)?;
    Some(0x80 + index as u8)
}

/// The BIOS unit BOOT -l `drive` boots from: A: and B: are the floppy
/// units whatever is in them, and a drive with a disk image its unit. C:
/// and D:, where they aren't disk images, are the disks mounted as 2 and
/// 3, as DOSBox's `IMGMOUNT 2 disk.img` then `BOOT -l C` has it.
pub fn boot_unit(bus: &Bus, drive: u8) -> Option<u8> {
    if drive < FLOPPY_UNITS {
        return Some(drive);
    }
    if let Some(number) = drive_number(drive).filter(|&n| n < FLOPPY_UNITS) {
        return Some(number);
    }
    drive_unit(bus, drive).or_else(|| match drive {
        DRIVE_C | DRIVE_D => drive_unit(bus, numbered_drive(drive)),
        _ => None,
    })
}

/// The boot sector of the disk in `unit`, if it can boot: a floppy's first
/// sector, or a hard disk's master boot record with its signature.
fn boot_sector(bus: &Bus, unit: u8) -> Result<[u8; SECTOR_SIZE], String> {
    let name = unit_drive(bus, unit).map_or("?".to_string(), drive_name);
    let image = unit_drive(bus, unit)
        .and_then(|drive| bus.disk.bios_image(drive))
        .ok_or_else(|| format!("There is no disk image in drive {}", name))?;
    let mut sector = [0u8; SECTOR_SIZE];
    image.read(0, &mut sector).map_err(|_| format!("Can't read the boot sector of drive {}", name))?;
    if unit >= 0x80 && sector[510..512] != [0x55, 0xAA] {
        return Err(format!("The disk in drive {} isn't bootable", name));
    }
    Ok(sector)
}

/// Boot the operating system on the disk in BIOS unit `unit`. The machine
/// starts over and runs its boot sector; an error says why it can't, with
/// the machine as it was.
pub fn boot(cpu: &mut Cpu, unit: u8) -> Result<(), String> {
    let sector = boot_sector(&cpu.bus, unit)?;
    let name = unit_drive(&cpu.bus, unit).map_or("?".to_string(), drive_name);
    cpu.bus.log_string(&format!("[BOOT] Booting from drive {} (unit {:02X}h)", name, unit));
    power_on(cpu, unit);
    start(cpu, unit, &sector);
    Ok(())
}

/// Boot from `drive`, as BOOT -l does: the drive's BIOS unit
/// (`boot_unit`). Not while a program of the built-in DOS runs; a system
/// booted before starts over from the new disk, as after a reset.
pub fn boot_drive(cpu: &mut Cpu, drive: u8) -> Result<(), String> {
    if !cpu.process_stack.is_empty() || cpu.secondary.is_some() {
        return Err("A system can't be booted while a program runs".to_string());
    }
    let unit = boot_unit(&cpu.bus, drive)
        .ok_or_else(|| format!("Drive {} can't be booted: it isn't a disk image", drive_name(drive)))?;
    boot(cpu, unit)
}

/// Start the booted system over from its disk, as after a reset or
/// INT 19h. If the disk can't boot any more, the machine is turned off.
pub fn restart(cpu: &mut Cpu) {
    let Some(unit) = cpu.bus.boot.as_ref().map(|b| b.unit) else {
        return;
    };
    match boot_sector(&cpu.bus, unit) {
        Ok(sector) => {
            cpu.bus.log_string("[BOOT] Restarting");
            power_on(cpu, unit);
            start(cpu, unit, &sector);
        }
        Err(e) => power_off(cpu, &e),
    }
}

/// Turn the booted machine off: the built-in DOS starts again, as the
/// execution loop reloads the shell (`Cpu::load_shell`).
pub fn power_off(cpu: &mut Cpu, why: &str) {
    cpu.bus.log_string(&format!("[BOOT] Turning the machine off: {}", why));
    cpu.state = CpuState::RebootShell;
}

/// Put the machine in the state a PC's BIOS leaves it in for the system it
/// boots from `unit`: nothing of the built-in DOS left, memory cleared, the
/// BIOS's vector table and data area, the devices reset and the screen in
/// text mode.
pub fn power_on(cpu: &mut Cpu, unit: u8) {
    // No program, batch file or prompt of the built-in DOS goes on.
    cpu.batch.clear();
    cpu.pending_command = None;
    cpu.shell_wait = None;
    cpu.shell_prompt_at = None;
    cpu.shell_completion = None;
    cpu.secondary_shells.clear();
    cpu.secondary = None;
    cpu.stdout_capture = None;
    cpu.stdin_redirect = None;
    cpu.process_stack.clear();
    cpu.current_psp = 0;
    cpu.bios_wait_until = None;
    cpu.con_pending_scan = None;
    cpu.con_line = None;
    cpu.con_pending.clear();
    cpu.hle_retry = false;
    cpu.idle = false;
    cpu.pm_latched = false;
    cpu.dynrec.flush();
    let name = unit_drive(&cpu.bus, unit).map_or("?".to_string(), drive_name);
    cpu.program = format!("BOOT {}", name);
    cpu.bus.disk.close_all_files();
    // Host directories shared with the system become hard disks, before
    // the journals start: what goes on them isn't the system's writing.
    for line in cpu.bus.disk.prepare_shared_disks() {
        cpu.bus.log_string(&format!("[BOOT] {}", line));
    }
    cpu.bus.boot = Some(BootState { unit, ..Default::default() });
    // Its states and rewind take its disks back with its memory.
    cpu.bus.disk.keep_journals(true);

    let bus = &mut cpu.bus;
    // All of memory cleared, as the power-on self test leaves it.
    let len = bus.ram().len();
    bus.fill_ram(0..crate::video::ADDR_VGA_GRAPHICS, 0);
    bus.fill_ram(0xC8000..0xF0000, 0);
    bus.fill_ram(0x10_0000..len, 0);
    bus.freezes.clear();

    // The devices as they come up.
    bus.pic = crate::pic::Pic::new();
    bus.dma = crate::dma::Dma::new();
    bus.kbc = crate::kbc::Kbc::new();
    bus.set_a20(false);
    bus.reset_requested = false;
    bus.port_accesses.clear();
    crate::keyboard::reset_keystrokes(bus);
    bus.reset_timers();
    bus.reset_sound();
    bus.reset_network();
    bus.reset_ne2000();
    bus.reset_serial();
    bus.reset_voodoo();
    bus.mouse.remove_callback();
    crate::mouse::clear_callback_busy(bus);
    bus.mouse.ps2 = crate::mouse::Ps2Mouse::default();
    bus.xms = crate::xms::Xms::new();
    crate::ems::hide_device(bus);
    bus.cmos.set(crate::cmos::SHUTDOWN_STATUS, 0);

    // The BIOS's vector table and data area.
    let hard_disks: Vec<_> = hard_disk_drives(bus)
        .into_iter()
        .filter_map(|drive| bus.disk.bios_image(drive).map(|image| image.geometry()))
        .collect();
    crate::bios::install_for_boot(bus, &hard_disks);
    bus.cmos.set_hard_disks(&hard_disks);
    // The hard disks and a CD image reach the system on the IDE channels
    // too.
    bus.attach_ide();
    bus.refresh_irq();

    // The processor as after a reset, and the screen in text mode.
    cpu.reset_to_real_mode();
    crate::instructions::fpu::control::fninit(cpu);
    crate::interrupts::int10::set_mode(cpu, 0x03);
}

/// Run the boot sector `sector` of `unit`: at 0000:7C00 with the unit in
/// DL, as a BIOS's INT 19h leaves it.
fn start(cpu: &mut Cpu, unit: u8, sector: &[u8; SECTOR_SIZE]) {
    cpu.bus.load_bytes(BOOT_SECTOR, sector);
    cpu.set_ds(0);
    cpu.set_fs(0);
    cpu.set_gs(0);
    cpu.set_eax(0);
    cpu.set_ebx(BOOT_SECTOR as u32);
    cpu.set_ecx(1);
    cpu.set_edx(unit as u32);
    cpu.set_esi(0);
    // ES:DI: the Plug and Play BIOS's installation structure, which a boot
    // sector may look for there.
    let (segment, offset) = crate::pnpbios::header_pointer();
    cpu.set_es(segment);
    cpu.set_edi(offset as u32);
    cpu.set_ebp(0);
    // The stack the IBM BIOS starts the boot sector with.
    cpu.set_ss(0x0030);
    cpu.set_esp(0x0100);
    cpu.set_cs(0x0000);
    cpu.set_eip(BOOT_SECTOR as u32);
    cpu.set_cpu_flags(CpuFlags::from_bits_truncate(0x0202));
    cpu.state = CpuState::Running;
}
