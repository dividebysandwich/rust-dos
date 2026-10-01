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

/// A booted system's state takes its disks back with its memory: through
/// the journal of the writes since, in the same run, and from the copy
/// beside a state file when the journal doesn't reach back (another run).
#[test]
fn a_booted_systems_state_takes_its_disk_back() {
    use rust_dos::savestate::{disks, machine as states};
    let mut cpu = machine("state");
    exec::run_command_line(&mut cpu, "BOOT -l C");
    assert!(run_until(&mut cpu, 1000, |cpu| cpu.bus.read_8(WAITING) == 1));
    let disk = cpu.bus.disk.bios_image(2).unwrap();
    let sector = |disk: &DiskImage| {
        let mut buf = vec![0u8; SECTOR_SIZE];
        disk.read(1, &mut buf).unwrap();
        buf
    };
    let pattern = sector(&disk);
    let state = states::save(&cpu);
    let file = PathBuf::from("target/test_boot/state/booted.state");
    disks::save_copies(&cpu, &file).unwrap();

    // The system writes its disk; the state brings the sector back.
    disk.write(1, &[0xEE; SECTOR_SIZE]).unwrap();
    states::load(&mut cpu, &state).unwrap();
    assert_eq!(sector(&disk), pattern, "reverted through the journal");
    assert!(cpu.bus.boot.is_some());

    // In another run the journal starts again: without the copy the state
    // is refused and the disk stays as it is.
    disk.write(1, &[0xDD; SECTOR_SIZE]).unwrap();
    disk.keep_journal(false);
    disk.keep_journal(true);
    assert!(states::load(&mut cpu, &state).is_err());
    assert_eq!(sector(&disk)[0], 0xDD);
    // With the copy beside the state file it comes back.
    disks::offer_copies(&mut cpu, &file);
    let loaded = states::load(&mut cpu, &state);
    disks::withdraw(&mut cpu);
    loaded.unwrap();
    assert_eq!(sector(&disk), pattern, "put back from the copy");
    disks::delete_copies(&file);
}

#[test]
fn windows_hands_its_machines_their_keys_through_their_bios() {
    use rust_dos::interrupts::int2f;
    let base = PathBuf::from("target/test_boot").join("windows");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let mut cpu = Cpu::new(base);
    cpu.bus.set_cycles_per_ms(1000);
    cpu.load_shell();
    // Windows' 386 enhanced mode starts (INT 2Fh AX=1605h, DX bit 0
    // clear): its keyboard driver hands each machine its keys through the
    // keyboard controller, and the machine's BIOS makes the keystrokes.
    cpu.set_ax(0x1605);
    cpu.set_dx(0);
    int2f::handle(&mut cpu);
    assert!(cpu.bus.kbd.windows);
    // STI; DS=0; INT 16h AH=00h; MOV [KEY], AX; JMP $
    let code = [0xFB, 0x31, 0xC0, 0x8E, 0xD8, 0xB4, 0x00, 0xCD, 0x16, 0xA3, KEY as u8, (KEY >> 8) as u8, 0xEB, 0xFE];
    cpu.bus.load_bytes(0x20000, &code);
    cpu.set_cs(0x2000);
    cpu.set_ip(0);
    press(&mut cpu, 0x1E, false);
    release(&mut cpu, 0x1E, false);
    assert!(cpu.bus.keyboard_buffer.is_empty(), "the host's keys go to the controller alone");
    assert!(run_until(&mut cpu, 1000, |cpu| cpu.bus.read_16(KEY) != 0), "INT 16h returns the key");
    assert_eq!(cpu.bus.read_16(KEY), 0x1E61);
    // Windows exits (AX=1606h): the keys are the built-in DOS's again.
    cpu.set_ax(0x1606);
    cpu.set_dx(0);
    int2f::handle(&mut cpu);
    assert!(!cpu.bus.kbd.windows);
    assert_eq!((0..4).map(|i| cpu.bus.read_8(0xF1004 + i)).collect::<Vec<_>>(), [0xFE, 0x38, 0x09, 0xCF]);
    press(&mut cpu, 0x1E, false);
    assert_eq!(cpu.bus.keyboard_buffer.front(), Some(&0x1E61));
}

/// Run the machine until the primary channel's status has `want` in
/// `mask`.
fn wait_disk(cpu: &mut Cpu, mask: u8, want: u8) {
    for _ in 0..10 {
        if cpu.bus.io_read(0x3F6) & mask == want {
            return;
        }
        run_until(cpu, 1, |_| false);
    }
    panic!("the disk's status stays {:02X}", cpu.bus.io_read(0x3F6));
}

/// The hard disk of a booted machine is an ATA disk on the primary IDE
/// channel too, the same image INT 13h reads, and the CMOS says it's
/// there.
#[test]
fn booted_hard_disks_are_ata_disks_on_the_primary_channel() {
    use rust_dos::ide::Device;
    let mut cpu = machine("ata");
    exec::run_command_line(&mut cpu, "BOOT -l C");
    assert!(run_until(&mut cpu, 1000, |cpu| cpu.bus.read_8(WAITING) == 1));
    let channel = cpu.bus.ide[0].as_ref().expect("the primary channel");
    assert!(matches!(channel.devices, [Some(Device::Ata(_)), None]));
    // An empty CD-ROM drive on the first free letter, for a disc later.
    let channel = cpu.bus.ide[1].as_ref().expect("the secondary channel");
    assert!(matches!(&channel.devices, [Some(Device::Atapi(cd)), None] if cd.drive == 3));
    assert_eq!(cpu.bus.booted_cd_drive(), Some(3));
    let cmos = |cpu: &mut Cpu, reg: u8| {
        cpu.bus.io_write(0x70, reg);
        cpu.bus.io_read(0x71)
    };
    assert_eq!((cmos(&mut cpu, 0x12), cmos(&mut cpu, 0x19), cmos(&mut cpu, 0x1A)), (0xF0, 47, 0));

    // Sector 1 through the ports, by LBA.
    let bus = &mut cpu.bus;
    bus.io_write(0x1F6, 0xE0);
    bus.io_write(0x1F2, 1);
    bus.io_write(0x1F3, 1);
    bus.io_write(0x1F4, 0);
    bus.io_write(0x1F5, 0);
    bus.io_write(0x1F7, 0x20);
    wait_disk(&mut cpu, 0x88, 0x08);
    let sector: Vec<u8> = (0..256).flat_map(|_| (cpu.bus.io_read_wide(0x1F0, 2) as u16).to_le_bytes()).collect();
    assert_eq!(sector, (0..SECTOR_SIZE).map(|i| i as u8).collect::<Vec<_>>());
    // Written through the ports, INT 13h's disk has it.
    let bus = &mut cpu.bus;
    bus.io_write(0x1F2, 1);
    bus.io_write(0x1F3, 1);
    bus.io_write(0x1F7, 0x30);
    for _ in 0..256 {
        bus.io_write_wide(0x1F0, 0xBEEF, 2);
    }
    wait_disk(&mut cpu, 0x80, 0);
    let mut buf = [0u8; SECTOR_SIZE];
    cpu.bus.disk.bios_image(2).unwrap().read(1, &mut buf).unwrap();
    assert_eq!(&buf[..2], &[0xEF, 0xBE]);

    // Back at the built-in DOS, the channels and the CMOS's disks go.
    cpu.load_shell();
    assert!(!cpu.bus.has_ide());
    assert_eq!(cmos(&mut cpu, 0x12), 0);
}

/// `ide_hard_disks=false` leaves the hard disks to INT 13h alone, and
/// MOUNT's `-ide` puts one where it says.
#[test]
fn hard_disks_go_where_they_are_asked_to() {
    use rust_dos::ide::{ChannelId, Device, IdeSlot};
    let mut cpu = machine("ide_off");
    cpu.bus.ide_hard_disks = false;
    cpu.bus.boot_cdrom = false;
    exec::run_command_line(&mut cpu, "BOOT -l C");
    assert!(run_until(&mut cpu, 1000, |cpu| cpu.bus.read_8(WAITING) == 1));
    assert!(!cpu.bus.has_ide());

    // The disk from a file, as MOUNT has it, and a slot for it.
    let mut cpu = machine("ide_slot");
    cpu.bus.boot_cdrom = false;
    let disk = cpu.bus.disk.bios_image(2).unwrap();
    let mut bytes = vec![0u8; disk.sectors() as usize * SECTOR_SIZE];
    disk.read(0, &mut bytes).unwrap();
    let path = std::fs::canonicalize("target/test_boot/ide_slot").unwrap().join("slave.img");
    std::fs::write(&path, bytes).unwrap();
    let opts = MountOptions { ide: Some(IdeSlot::new(ChannelId::Secondary, true)), ..Default::default() };
    cpu.bus.mount_drive(2, &path, opts, true).unwrap();
    exec::run_command_line(&mut cpu, "BOOT -l C");
    assert!(run_until(&mut cpu, 1000, |cpu| cpu.bus.read_8(WAITING) == 1));
    assert!(cpu.bus.ide[0].is_none());
    let channel = cpu.bus.ide[1].as_ref().expect("the secondary channel");
    assert!(matches!(channel.devices, [None, Some(Device::Ata(_))]));
}

/// A host folder mounted as a CD is a disc made from it in a booted
/// system's CD-ROM drive; one mounted while it runs goes in its empty
/// drive, whatever the letter, and the built-in DOS reads the folder again.
#[test]
fn host_folders_go_in_the_cd_rom_drive_as_discs() {
    use rust_dos::disk::DriveKind;
    let mut cpu = machine("folder_cd");
    let folder = std::fs::canonicalize("target/test_boot/folder_cd").unwrap().join("cd");
    std::fs::create_dir_all(folder.join("Some Folder")).unwrap();
    std::fs::write(folder.join("A long name.txt"), b"from the host").unwrap();
    let cd = MountOptions { kind: DriveKind::CdRom, ..Default::default() };
    cpu.bus.mount_drive(4, &folder, cd.clone(), false).unwrap();
    exec::run_command_line(&mut cpu, "BOOT -l C");
    assert!(run_until(&mut cpu, 1000, |cpu| cpu.bus.read_8(WAITING) == 1));
    assert_eq!(cpu.bus.booted_cd_drive(), Some(4));
    let disc = cpu.bus.disk.boot_cd_image(4).expect("a disc made from the folder");
    let volume = rust_dos::cdrom::iso9660::read_volume(&disc).unwrap();
    assert_eq!(volume.label, "CD");
    assert!(volume.files.file("ALONGN~1.TXT").is_some());
    assert!(cpu.bus.disk.cd_image(4).is_none(), "MSCDEX has the folder");

    // Taken out, and another folder mounted on another letter goes in.
    cpu.bus.unmount_drive(4).unwrap();
    assert!(cpu.bus.disk.boot_cd_image(4).is_none());
    cpu.bus.mount_drive(5, &folder.join("Some Folder"), cd, false).unwrap();
    assert_eq!(cpu.bus.booted_cd_drive(), Some(5));
    assert!(cpu.bus.disk.boot_cd_image(5).is_some());
    assert!(cpu.bus.reinsert_cd(5).is_ok());

    cpu.load_shell();
    assert!(cpu.bus.disk.boot_cd_image(5).is_none());
}

/// A host directory on D: is a booted system's hard disk after its own
/// ones, and what the system wrote on it is in the directory when it's
/// off; while it runs, the drive stays.
#[test]
fn host_directories_are_hard_disks_of_a_booted_system() {
    let mut cpu = machine("shared");
    let folder = std::fs::canonicalize("target/test_boot/shared").unwrap().join("share");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("From the host.txt"), b"host").unwrap();
    cpu.bus.mount_drive(3, &folder, MountOptions::default(), false).unwrap();
    exec::run_command_line(&mut cpu, "BOOT -l C");
    assert!(run_until(&mut cpu, 1000, |cpu| cpu.bus.read_8(WAITING) == 1));
    assert_eq!(cpu.bus.read_8(HARD_DISKS), 2, "two hard disks");
    assert_eq!(rust_dos::boot::unit_drive(&cpu.bus, 0x81), Some(3));
    let channel = cpu.bus.ide[0].as_ref().expect("the primary channel");
    assert!(matches!(channel.devices, [Some(rust_dos::ide::Device::Ata(_)), Some(rust_dos::ide::Device::Ata(_))]));
    assert!(cpu.bus.unmount_drive(3).is_err());

    // The system writes a file.
    let disk = cpu.bus.disk.bios_image(3).unwrap();
    let (start, sectors) = disk.fat_volume().unwrap();
    let volume = rust_dos::fat::FatVolume::open(disk, start, sectors).unwrap();
    let file = volume.create_long(&[], "Written by the guest.txt", 0).unwrap();
    volume.write(file.at.unwrap(), 0, b"guest").unwrap();
    assert!(cpu.bus.sync_shared(None)[0].ends_with("1 written"));
    assert_eq!(std::fs::read(folder.join("Written by the guest.txt")).unwrap(), b"guest");
    volume.remove(&["FROMTH~1.TXT"]).unwrap();

    cpu.load_shell();
    assert!(!folder.join("From the host.txt").exists());
    assert!(!cpu.bus.disk.is_shared(3));
    assert!(cpu.bus.disk_notices.iter().any(|n| n.contains("1 deleted")));
    cpu.bus.unmount_drive(3).unwrap();
}

/// A state of a booted system with a shared host directory takes the
/// directory's disk back with it, and its record of what was copied back
/// when: in the same run through the journal, in another from the copy
/// beside the state file.
#[test]
fn a_booted_systems_state_takes_its_shared_disk_back() {
    use rust_dos::savestate::{disks, machine as states};
    // Made here: this test may run before any other has made it.
    std::fs::create_dir_all("target/test_boot").unwrap();
    let folder = std::fs::canonicalize("target/test_boot").unwrap().join("shared_state_folder");
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).unwrap();
    let boot = |name: &str| {
        let mut cpu = machine(name);
        cpu.bus.mount_drive(3, &folder, MountOptions::default(), false).unwrap();
        cpu
    };
    let volume = |cpu: &Cpu| {
        let disk = cpu.bus.disk.bios_image(3).unwrap();
        let (start, sectors) = disk.fat_volume().unwrap();
        rust_dos::fat::FatVolume::open(disk, start, sectors).unwrap()
    };
    let put = |cpu: &Cpu, name: &str| {
        let file = volume(cpu).create_long(&[], name, 0).unwrap();
        volume(cpu).write(file.at.unwrap(), 0, name.as_bytes()).unwrap();
    };
    let names = |cpu: &Cpu| -> Vec<String> {
        volume(cpu).list(&[]).unwrap().into_iter().filter_map(|e| e.long_name).collect()
    };

    let mut cpu = boot("shared_state");
    exec::run_command_line(&mut cpu, "BOOT -l C");
    assert!(run_until(&mut cpu, 1000, |cpu| cpu.bus.read_8(WAITING) == 1));
    put(&cpu, "Before the state.txt");
    let state = states::save(&cpu);
    let file = PathBuf::from("target/test_boot/shared_state/booted.state");
    disks::save_copies(&cpu, &file).unwrap();
    let copy = disks::copy_path(&file, 3);
    assert!(std::fs::metadata(&copy).unwrap().len() > 1 << 30, "the whole disk");

    // Written and copied back after the state: the state takes the disk
    // back, and the host keeps the file.
    put(&cpu, "After the state.txt");
    cpu.bus.sync_shared(None);
    states::load(&mut cpu, &state).unwrap();
    assert_eq!(names(&cpu), ["Before the state.txt"]);
    cpu.load_shell();
    assert!(folder.join("Before the state.txt").is_file());
    assert!(folder.join("After the state.txt").is_file());

    // Another run, with the folder mounted but no system booted.
    let mut cpu = boot("shared_state_again");
    disks::offer_copies(&mut cpu, &file);
    let loaded = states::load(&mut cpu, &state);
    disks::withdraw(&mut cpu);
    loaded.unwrap();
    assert!(cpu.bus.boot.is_some());
    assert_eq!(names(&cpu), ["Before the state.txt"]);
    disks::delete_copies(&file);
}
