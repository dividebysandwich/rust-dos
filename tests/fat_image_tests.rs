//! Drives mounted from floppy and hard disk images: DOS file access, the
//! BIOS disk services and DOS absolute sector access on their FAT file
//! systems.

mod fatimage;

use fatimage::{FLOPPY_720, FLOPPY_1440, HARD_DISK, SECTOR};
use iced_x86::Register;
use rust_dos::cpu::{Cpu, CpuFlags};
use rust_dos::disk::{DriveKind, MountOptions};
use rust_dos::exec::{self, NoHook};
use rust_dos::interrupts::{int13, int21};
use std::fs;
use std::path::{Path, PathBuf};

const DRIVE_A: u8 = 0;
const DRIVE_C: u8 = 2;
const DRIVE_D: u8 = 3;
const DTA: usize = 0x40000;
const BUFFER: usize = 0x50000;

fn cpu(dir: &Path) -> Cpu {
    fs::create_dir_all(dir.join("c")).unwrap();
    Cpu::new(dir.join("c"))
}

fn mount(cpu: &mut Cpu, drive: u8, path: &Path) {
    cpu.bus.mount_drive(drive, path, MountOptions::default(), true).unwrap();
}

fn cf(cpu: &Cpu) -> bool {
    cpu.get_cpu_flag(CpuFlags::CF)
}

fn int21(cpu: &mut Cpu, ax: u16) {
    cpu.set_ax(ax);
    int21::handle(cpu);
}

/// Put an ASCIIZ string at 2000:0000 and point DS:DX at it.
fn name(cpu: &mut Cpu, s: &str) {
    for (i, b) in s.bytes().chain(std::iter::once(0)).enumerate() {
        cpu.bus.write_8(0x20000 + i, b);
    }
    cpu.set_ds(0x2000);
    cpu.set_dx(0);
}

fn open(cpu: &mut Cpu, path: &str, mode: u8) -> Result<u16, u16> {
    name(cpu, path);
    int21(cpu, 0x3D00 | mode as u16);
    if cf(cpu) { Err(cpu.ax()) } else { Ok(cpu.ax()) }
}

fn create(cpu: &mut Cpu, path: &str) -> Result<u16, u16> {
    name(cpu, path);
    cpu.set_cx(0);
    int21(cpu, 0x3C00);
    if cf(cpu) { Err(cpu.ax()) } else { Ok(cpu.ax()) }
}

fn read(cpu: &mut Cpu, handle: u16, count: u16) -> Vec<u8> {
    cpu.set_bx(handle);
    cpu.set_cx(count);
    cpu.set_ds(0x5000);
    cpu.set_dx(0);
    int21(cpu, 0x3F00);
    assert!(!cf(cpu), "read failed: {:04X}", cpu.ax());
    (0..cpu.ax() as usize).map(|i| cpu.bus.read_8(BUFFER + i)).collect()
}

fn write(cpu: &mut Cpu, handle: u16, data: &[u8]) -> Result<u16, u16> {
    cpu.bus.load_bytes(BUFFER, data);
    cpu.set_bx(handle);
    cpu.set_cx(data.len() as u16);
    cpu.set_ds(0x5000);
    cpu.set_dx(0);
    int21(cpu, 0x4000);
    if cf(cpu) { Err(cpu.ax()) } else { Ok(cpu.ax()) }
}

fn close(cpu: &mut Cpu, handle: u16) {
    cpu.set_bx(handle);
    int21(cpu, 0x3E00);
    assert!(!cf(cpu));
}

fn read_file(cpu: &mut Cpu, path: &str) -> Vec<u8> {
    let handle = open(cpu, path, 0).unwrap_or_else(|e| panic!("open {}: {:02X}", path, e));
    let mut data = Vec::new();
    loop {
        let chunk = read(cpu, handle, 0x8000);
        if chunk.is_empty() {
            break;
        }
        data.extend(chunk);
    }
    close(cpu, handle);
    data
}

/// FindFirst/FindNext: every name matching `spec`.
fn find_all(cpu: &mut Cpu, spec: &str, attr: u16) -> Vec<String> {
    cpu.set_ds(0x4000);
    cpu.set_dx(0);
    int21(cpu, 0x1A00);
    name(cpu, spec);
    cpu.set_cx(attr);
    int21(cpu, 0x4E00);
    let mut names = Vec::new();
    while !cf(cpu) {
        names.push(
            (0..13)
                .map(|i| cpu.bus.read_8(DTA + 0x1E + i))
                .take_while(|&b| b != 0)
                .map(|b| b as char)
                .collect(),
        );
        int21(cpu, 0x4F00);
    }
    names
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect()
}

fn floppy_with_files(dir: &Path) -> PathBuf {
    let doom = pattern(3000, 1);
    let bytes = fatimage::image(
        FLOPPY_1440,
        Some("GAMEDISK"),
        &["EMPTY"],
        &[("README.TXT", b"hello floppy"), ("GAMES\\DOOM.EXE", &doom)],
    );
    fatimage::write(dir, "disk.img", &bytes)
}

#[test]
fn floppy_images_are_dos_drives() {
    let dir = fatimage::scratch("dos");
    let image = floppy_with_files(&dir);
    let mut cpu = cpu(&dir);
    mount(&mut cpu, DRIVE_A, &image);
    assert_eq!(cpu.bus.disk.drive_kind(DRIVE_A), Some(DriveKind::Floppy));
    assert_eq!(cpu.bus.disk.volume_label(DRIVE_A).as_deref(), Some("GAMEDISK"));

    assert_eq!(find_all(&mut cpu, "A:\\*.*", 0x16), ["EMPTY", "README.TXT", "GAMES"]);
    assert_eq!(find_all(&mut cpu, "A:\\GAMES\\*.EXE", 0), ["DOOM.EXE"]);
    assert_eq!(read_file(&mut cpu, "A:\\README.TXT"), b"hello floppy");
    assert_eq!(read_file(&mut cpu, "a:\\games\\doom.exe"), pattern(3000, 1));
    assert_eq!(open(&mut cpu, "A:\\NOPE.TXT", 0), Err(0x02));
    assert_eq!(open(&mut cpu, "A:\\NOPE\\X.TXT", 0), Err(0x03));
    assert_eq!(open(&mut cpu, "A:\\GAMES", 0), Err(0x05));

    // Seeking: from the end, then read the last bytes.
    let handle = open(&mut cpu, "A:\\GAMES\\DOOM.EXE", 0).unwrap();
    cpu.set_bx(handle);
    cpu.set_cx(0xFFFF);
    cpu.set_dx(-10i16 as u16);
    int21(&mut cpu, 0x4202);
    assert_eq!((cpu.dx(), cpu.ax()), (0, 2990));
    assert_eq!(read(&mut cpu, handle, 100), &pattern(3000, 1)[2990..]);
    close(&mut cpu, handle);

    // Allocation (AH=1Ch) and the DPB (AH=32h) are the disk's.
    cpu.set_reg8(Register::DL, 1);
    int21(&mut cpu, 0x1C00);
    assert_eq!((cpu.get_al(), cpu.cx(), cpu.dx()), (1, 512, 2847));
    cpu.set_reg8(Register::DL, 1);
    int21(&mut cpu, 0x3200);
    let dpb = cpu.get_physical_addr(cpu.ds(), cpu.bx());
    assert_eq!(cpu.bus.read_16(dpb + 0x0B), 33); // first data sector
    assert_eq!(cpu.bus.read_16(dpb + 0x0F), 9); // sectors per FAT
    assert_eq!(cpu.bus.read_8(dpb + 0x17), 0xF0);
}

#[test]
fn writes_land_on_the_image() {
    let dir = fatimage::scratch("write");
    let image = floppy_with_files(&dir);
    let mut cpu = cpu(&dir);
    mount(&mut cpu, DRIVE_A, &image);

    cpu.set_reg8(Register::DL, 1);
    int21(&mut cpu, 0x3600);
    let free_before = cpu.bx();

    let data = pattern(20_000, 7);
    let handle = create(&mut cpu, "A:\\SAVE.DAT").unwrap();
    assert_eq!(write(&mut cpu, handle, &data[..5000]), Ok(5000));
    assert_eq!(write(&mut cpu, handle, &data[5000..]), Ok(15000));
    close(&mut cpu, handle);

    cpu.set_reg8(Register::DL, 1);
    int21(&mut cpu, 0x3600);
    assert_eq!(free_before - cpu.bx(), 40);

    // Directories, renaming and deleting.
    name(&mut cpu, "A:\\SAVES");
    int21(&mut cpu, 0x3900);
    assert!(!cf(&cpu));
    name(&mut cpu, "A:\\SAVES");
    int21(&mut cpu, 0x3900);
    assert_eq!((cf(&cpu), cpu.ax()), (true, 0x05));
    name(&mut cpu, "A:\\SAVE.DAT");
    cpu.set_es(0x2000);
    cpu.set_di(0x100);
    for (i, b) in b"A:\\SAVES\\GAME1.SAV\0".iter().enumerate() {
        cpu.bus.write_8(0x20100 + i, *b);
    }
    int21(&mut cpu, 0x5600);
    assert!(!cf(&cpu), "rename: {:02X}", cpu.ax());
    name(&mut cpu, "A:\\SAVES");
    int21(&mut cpu, 0x3B00);
    assert!(!cf(&cpu));
    cpu.set_reg8(Register::DL, 1);
    cpu.set_ds(0x3000);
    cpu.set_si(0);
    int21(&mut cpu, 0x4700);
    let cwd: Vec<u8> = (0..5).map(|i| cpu.bus.read_8(0x30000 + i)).collect();
    assert_eq!(cwd, b"SAVES");
    assert_eq!(read_file(&mut cpu, "A:GAME1.SAV"), data);

    // Everything is still there on a fresh mount of the image.
    cpu.bus.unmount_drive(DRIVE_A).unwrap();
    mount(&mut cpu, DRIVE_A, &image);
    assert_eq!(read_file(&mut cpu, "A:\\SAVES\\GAME1.SAV"), data);
    assert_eq!(find_all(&mut cpu, "A:\\*.*", 0x10), ["EMPTY", "README.TXT", "GAMES", "SAVES"]);

    // Attributes: a read-only file can't be deleted or written.
    name(&mut cpu, "A:\\README.TXT");
    cpu.set_cx(0x01);
    int21(&mut cpu, 0x4301);
    assert!(!cf(&cpu));
    int21(&mut cpu, 0x4300);
    assert_eq!(cpu.cx(), 0x01);
    int21(&mut cpu, 0x4100);
    assert_eq!((cf(&cpu), cpu.ax()), (true, 0x05));
    assert_eq!(open(&mut cpu, "A:\\README.TXT", 2), Err(0x05));

    name(&mut cpu, "A:\\SAVES\\GAME1.SAV");
    int21(&mut cpu, 0x4100);
    assert!(!cf(&cpu));
    name(&mut cpu, "A:\\");
    int21(&mut cpu, 0x3B00);
    name(&mut cpu, "A:\\SAVES");
    int21(&mut cpu, 0x3A00);
    assert!(!cf(&cpu), "rmdir: {:02X}", cpu.ax());
    assert_eq!(find_all(&mut cpu, "A:\\*.*", 0x10), ["EMPTY", "README.TXT", "GAMES"]);
}

#[test]
fn file_times_are_kept() {
    let dir = fatimage::scratch("time");
    let image = floppy_with_files(&dir);
    let mut cpu = cpu(&dir);
    mount(&mut cpu, DRIVE_A, &image);
    let handle = open(&mut cpu, "A:\\README.TXT", 2).unwrap();
    cpu.set_bx(handle);
    int21(&mut cpu, 0x5700);
    assert_eq!((cpu.cx(), cpu.dx()), (0x6000, 0x5021));
    cpu.set_cx(0x1234);
    cpu.set_dx(0x2345);
    int21(&mut cpu, 0x5701);
    assert!(!cf(&cpu));
    int21(&mut cpu, 0x5700);
    assert_eq!((cpu.cx(), cpu.dx()), (0x1234, 0x2345));
    close(&mut cpu, handle);
}

#[test]
fn write_protected_images() {
    let dir = fatimage::scratch("ro");
    let image = floppy_with_files(&dir);
    let mut cpu = cpu(&dir);
    let ro = MountOptions { read_only: true, ..Default::default() };
    cpu.bus.mount_drive(DRIVE_A, &image, ro, false).unwrap();
    assert!(!cpu.bus.disk.is_writable(DRIVE_A));
    assert_eq!(create(&mut cpu, "A:\\NEW.TXT"), Err(0x05));
    // Read/write opens are downgraded, as on CD-ROMs.
    let handle = open(&mut cpu, "A:\\README.TXT", 2).unwrap();
    assert_eq!(write(&mut cpu, handle, b"x"), Err(0x05));
    close(&mut cpu, handle);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&image, fs::Permissions::from_mode(0o444)).unwrap();
        mount(&mut cpu, DRIVE_A, &image);
        assert!(!cpu.bus.disk.is_writable(DRIVE_A));
        assert_eq!(read_file(&mut cpu, "A:\\README.TXT"), b"hello floppy");
        fs::set_permissions(&image, fs::Permissions::from_mode(0o644)).unwrap();
    }
}

#[test]
fn hard_disk_images_can_be_c() {
    let dir = fatimage::scratch("hdd");
    let big = pattern(100_000, 3);
    let bytes = fatimage::image(HARD_DISK, Some("HARDDISK"), &[], &[("DOS\\BIG.BIN", &big), ("AUTOEXEC.BAT", b"@ECHO OFF\r\n")]);
    let image = fatimage::write(&dir, "hdd.img", &bytes);
    let mut cpu = cpu(&dir);
    mount(&mut cpu, DRIVE_C, &image);
    assert_eq!(cpu.bus.disk.drive_kind(DRIVE_C), Some(DriveKind::HardDisk));
    assert_eq!(read_file(&mut cpu, "C:\\DOS\\BIG.BIN"), big);
    assert_eq!(cpu.bus.disk.file_data("C:\\AUTOEXEC.BAT").unwrap().read().unwrap().to_vec(), b"@ECHO OFF\r\n");

    let layout = cpu.bus.disk.layout(DRIVE_C).unwrap();
    assert_eq!((layout.hidden_sectors, layout.media, layout.fs_type()), (63, 0xF8, b"FAT16   "));

    // 440Dh/0860h returns the BPB of the boot sector.
    cpu.set_reg8(Register::BL, 3);
    cpu.set_cx(0x0860);
    cpu.set_ds(0x5000);
    cpu.set_dx(0);
    int21(&mut cpu, 0x440D);
    assert!(!cf(&cpu));
    let bpb: Vec<u8> = (0..25).map(|i| cpu.bus.read_8(BUFFER + 7 + i)).collect();
    assert_eq!(bpb, &bytes[63 * SECTOR + 0x0B..63 * SECTOR + 0x0B + 25]);
    assert_eq!(cpu.bus.read_8(BUFFER + 1), 0x05); // fixed disk

    let handle = create(&mut cpu, "C:\\DOS\\NEW.TXT").unwrap();
    write(&mut cpu, handle, b"on the hard disk").unwrap();
    close(&mut cpu, handle);
    assert_eq!(read_file(&mut cpu, "C:\\DOS\\NEW.TXT"), b"on the hard disk");

    // The same image can't be on two drives.
    assert!(cpu.bus.mount_drive(DRIVE_D, &image, MountOptions::default(), false).is_err());
}

#[test]
fn programs_load_from_images() {
    let dir = fatimage::scratch("exec");
    let program = pattern(700, 9);
    let bytes = fatimage::image(FLOPPY_720, None, &[], &[("GAME.COM", &program)]);
    let image = fatimage::write(&dir, "game.ima", &bytes);
    let mut cpu = cpu(&dir);
    mount(&mut cpu, DRIVE_A, &image);
    let data = cpu.bus.disk.file_data("A:\\GAME.COM").unwrap().read().unwrap();
    assert_eq!(data.to_vec(), program);
    assert_eq!(cpu.bus.disk.layout(DRIVE_A).unwrap().sectors_per_cluster, 2);
}

/// INT 13h with AL = count, CX = cylinder/sector, DH = head, ES:BX = the
/// buffer at 5000:0000.
fn int13(cpu: &mut Cpu, ah: u8, dl: u8, count: u8, cylinder: u16, head: u8, sector: u8) -> (bool, u8) {
    cpu.set_reg8(Register::AH, ah);
    cpu.set_reg8(Register::AL, count);
    cpu.set_reg8(Register::DL, dl);
    cpu.set_reg8(Register::DH, head);
    cpu.set_reg8(Register::CH, cylinder as u8);
    cpu.set_reg8(Register::CL, sector | ((cylinder >> 2) as u8 & 0xC0));
    cpu.set_es(0x5000);
    cpu.set_bx(0);
    int13::handle(cpu);
    (cf(cpu), cpu.get_reg8(Register::AH))
}

fn buffer(cpu: &Cpu, len: usize) -> Vec<u8> {
    (0..len).map(|i| cpu.bus.read_8(BUFFER + i)).collect()
}

#[test]
fn bios_reads_and_writes_image_sectors() {
    let dir = fatimage::scratch("int13");
    let image = floppy_with_files(&dir);
    let bytes = fs::read(&image).unwrap();
    let mut cpu = cpu(&dir);
    mount(&mut cpu, DRIVE_A, &image);

    // The boot sector, then the last sectors of a track: head 0, sectors
    // 17 and 18 go on to head 1.
    assert_eq!(int13(&mut cpu, 0x02, 0, 1, 0, 0, 1), (false, 0));
    assert_eq!(buffer(&cpu, SECTOR), &bytes[..SECTOR]);
    assert_eq!(int13(&mut cpu, 0x02, 0, 3, 0, 0, 17), (false, 0));
    assert_eq!(cpu.get_al(), 3);
    assert_eq!(buffer(&cpu, 3 * SECTOR), &bytes[16 * SECTOR..19 * SECTOR]);

    // A write to cylinder 1, head 1, sector 5: sector (1*2+1)*18+4 = 58.
    cpu.bus.load_bytes(BUFFER, &[0x5A; SECTOR]);
    assert_eq!(int13(&mut cpu, 0x03, 0, 1, 1, 1, 5), (false, 0));
    assert_eq!(&fs::read(&image).unwrap()[58 * SECTOR..59 * SECTOR], &[0x5A; SECTOR]);

    // Sector 19 isn't on the track; the status stays for AH=01h.
    assert_eq!(int13(&mut cpu, 0x02, 0, 1, 0, 0, 19), (true, 0x04));
    assert_eq!(cpu.bus.read_8(0x0441), 0x04);
    cpu.set_reg8(Register::AH, 0x01);
    int13::handle(&mut cpu);
    assert_eq!((cf(&cpu), cpu.get_reg8(Register::AH)), (true, 0x04));
    assert_eq!(int13(&mut cpu, 0x02, 0, 1, 80, 0, 1), (true, 0x04));

    // A sector write the file system sees at once: clear the root
    // directory (sector 19) and README.TXT is gone.
    assert_eq!(open(&mut cpu, "A:\\README.TXT", 0).map(|_| ()), Ok(()));
    cpu.bus.load_bytes(BUFFER, &[0; SECTOR]);
    assert_eq!(int13(&mut cpu, 0x03, 0, 1, 0, 1, 2), (false, 0));
    assert_eq!(open(&mut cpu, "A:\\README.TXT", 0), Err(0x02));

    // Write-protected.
    let ro = MountOptions { read_only: true, ..Default::default() };
    cpu.bus.mount_drive(DRIVE_A, &image, ro, true).unwrap();
    assert_eq!(int13(&mut cpu, 0x03, 0, 1, 0, 0, 1), (true, 0x03));
}

#[test]
fn bios_reports_image_geometry() {
    let dir = fatimage::scratch("geometry");
    let floppy = fatimage::write(&dir, "d720.img", &fatimage::image(FLOPPY_720, None, &[], &[]));
    let hdd_bytes = fatimage::image(HARD_DISK, None, &[], &[]);
    let hdd = fatimage::write(&dir, "hdd.img", &hdd_bytes);
    let mut cpu = cpu(&dir);
    mount(&mut cpu, DRIVE_A, &floppy);
    mount(&mut cpu, DRIVE_D, &hdd);

    assert_eq!(int13(&mut cpu, 0x08, 0, 0, 0, 0, 0), (false, 0));
    assert_eq!(
        (cpu.get_reg8(Register::CH), cpu.get_reg8(Register::CL), cpu.get_reg8(Register::DH)),
        (79, 9, 1)
    );
    assert_eq!((cpu.get_reg8(Register::BL), cpu.get_reg8(Register::DL)), (3, 1));

    // C: (a host directory) is 80h, D: 81h.
    assert_eq!(int13(&mut cpu, 0x08, 0x81, 0, 0, 0, 0), (false, 0));
    assert_eq!(
        (cpu.get_reg8(Register::CH), cpu.get_reg8(Register::CL), cpu.get_reg8(Register::DH)),
        (4, 63, 15)
    );
    assert_eq!(cpu.get_reg8(Register::DL), 2);
    cpu.set_reg8(Register::AH, 0x15);
    cpu.set_reg8(Register::DL, 0x81);
    int13::handle(&mut cpu);
    assert_eq!(cpu.get_reg8(Register::AH), 0x03);
    assert_eq!(((cpu.cx() as u32) << 16) | cpu.dx() as u32, 5040);

    // The MBR is the hard disk's first sector.
    assert_eq!(int13(&mut cpu, 0x02, 0x81, 1, 0, 0, 1), (false, 0));
    assert_eq!(buffer(&cpu, SECTOR), &hdd_bytes[..SECTOR]);
    // Cylinder 0, head 1, sector 1 is the partition's boot sector.
    assert_eq!(int13(&mut cpu, 0x02, 0x81, 1, 0, 1, 1), (false, 0));
    assert_eq!(buffer(&cpu, SECTOR), &hdd_bytes[63 * SECTOR..64 * SECTOR]);
}

#[test]
fn image_lists_swap_disks() {
    let dir = fatimage::scratch("swap");
    let d1 = fatimage::write(&dir, "disk1.img", &fatimage::image(FLOPPY_1440, Some("DISK1"), &["SUB"], &[("ONE.TXT", b"one")]));
    let d2 = fatimage::write(&dir, "disk2.img", &fatimage::image(FLOPPY_1440, Some("DISK2"), &[], &[("TWO.TXT", b"two")]));
    let mut cpu = cpu(&dir);
    let opts = MountOptions { more_images: vec![d2.clone()], ..Default::default() };
    cpu.bus.mount_drive(DRIVE_A, &d1, opts, false).unwrap();
    let info = cpu.bus.disk.drive_info(DRIVE_A).unwrap();
    assert_eq!((info.images.len(), info.image_index, info.label.as_str()), (2, 0, "DISK1"));

    // A new disk: the change line says so once.
    cpu.set_reg8(Register::AH, 0x16);
    cpu.set_reg8(Register::DL, 0);
    int13::handle(&mut cpu);
    assert_eq!((cf(&cpu), cpu.get_reg8(Register::AH)), (true, 0x06));
    cpu.set_reg8(Register::AH, 0x16);
    int13::handle(&mut cpu);
    assert_eq!((cf(&cpu), cpu.get_reg8(Register::AH)), (false, 0));

    name(&mut cpu, "A:\\SUB");
    int21(&mut cpu, 0x3B00);
    let handle = open(&mut cpu, "A:\\ONE.TXT", 0).unwrap();
    let messages = cpu.bus.swap_images();
    assert_eq!(messages, ["Drive A: disk 2 of 2: disk2.img"]);
    assert_eq!(read_file(&mut cpu, "A:\\TWO.TXT"), b"two");
    assert_eq!(open(&mut cpu, "A:\\ONE.TXT", 0), Err(0x02));
    // Files opened on the first disk still read it; the current directory
    // isn't on the second.
    assert_eq!(read(&mut cpu, handle, 10), b"one");
    assert_eq!(cpu.bus.disk.get_current_directory_of(DRIVE_A).as_deref(), Some(""));
    assert_eq!(cpu.bus.disk.volume_label(DRIVE_A).as_deref(), Some("DISK2"));
    cpu.set_reg8(Register::AH, 0x16);
    cpu.set_reg8(Register::DL, 0);
    int13::handle(&mut cpu);
    assert_eq!(cpu.get_reg8(Register::AH), 0x06);

    // And round again.
    assert_eq!(cpu.bus.swap_images(), ["Drive A: disk 1 of 2: disk1.img"]);
    assert_eq!(read_file(&mut cpu, "A:\\ONE.TXT"), b"one");
}

/// Run the code at 2000:0000 until it halts at its end, with interrupts on.
fn run(cpu: &mut Cpu, code: &[u8]) {
    cpu.bus.load_bytes(0x20000, code);
    cpu.set_cs(0x2000);
    cpu.set_ip(0);
    cpu.set_ss(0x3000);
    cpu.set_sp(0x0100);
    cpu.set_cpu_flag(CpuFlags::IF, true);
    let start = cpu.bus.clock.now_micros();
    while cpu.ip() != code.len() as u16 {
        let end = cpu.bus.clock.icount + 1000;
        cpu.bus.start_batch(end);
        exec::run_batch(cpu, &mut NoHook, false);
        assert!(
            cpu.bus.clock.now_micros() - start < 5_000_000,
            "the code ends: {:04X}:{:04X} IVT 25h = {:08X}",
            cpu.cs(),
            cpu.ip(),
            cpu.bus.read_32(0x94)
        );
    }
}

#[test]
fn absolute_sector_reads_and_writes() {
    let dir = fatimage::scratch("int25");
    let image = floppy_with_files(&dir);
    let bytes = fs::read(&image).unwrap();
    let mut cpu = cpu(&dir);
    cpu.bus.set_cycles_per_ms(1000);
    mount(&mut cpu, DRIVE_A, &image);

    // MOV AX,5000h; MOV DS,AX; MOV AL,0; MOV CX,2; MOV DX,19; XOR BX,BX;
    // INT 25h; POP DX; HLT: the root directory's first two sectors. The
    // flags INT 25h leaves on the stack are the caller's to discard, without
    // losing CF.
    let code = [
        0xB8, 0x00, 0x50, 0x8E, 0xD8, 0xB0, 0x00, 0xB9, 0x02, 0x00, 0xBA, 0x13, 0x00, 0x31, 0xDB, 0xCD, 0x25, 0x5A,
        0xF4,
    ];
    run(&mut cpu, &code);
    assert_eq!(cpu.sp(), 0x0100, "the flags are popped by the caller");
    assert!(!cf(&cpu));
    assert_eq!(buffer(&cpu, 2 * SECTOR), &bytes[19 * SECTOR..21 * SECTOR]);

    // The packet form (CX=FFFFh): sector 0, one sector, to 5000:0000.
    cpu.bus.load_bytes(0x60000, &[0, 0, 0, 0, 1, 0, 0x00, 0x00, 0x00, 0x50]);
    let packet = [
        0xB8, 0x00, 0x60, 0x8E, 0xD8, 0xB0, 0x00, 0xB9, 0xFF, 0xFF, 0x31, 0xDB, 0xCD, 0x25, 0x5A, 0xF4,
    ];
    run(&mut cpu, &packet);
    assert!(!cf(&cpu));
    assert_eq!(buffer(&cpu, SECTOR), &bytes[..SECTOR]);

    // INT 26h writes sector 100.
    cpu.bus.load_bytes(BUFFER, &[0xA5; SECTOR]);
    let write = [
        0xB8, 0x00, 0x50, 0x8E, 0xD8, 0xB0, 0x00, 0xB9, 0x01, 0x00, 0xBA, 0x64, 0x00, 0x31, 0xDB, 0xCD, 0x26, 0x5A,
        0xF4,
    ];
    run(&mut cpu, &write);
    assert!(!cf(&cpu));
    assert_eq!(&fs::read(&image).unwrap()[100 * SECTOR..101 * SECTOR], &[0xA5; SECTOR]);

    // Write-protected: AX=0300h.
    let ro = MountOptions { read_only: true, ..Default::default() };
    cpu.bus.mount_drive(DRIVE_A, &image, ro, true).unwrap();
    run(&mut cpu, &write);
    assert_eq!((cf(&cpu), cpu.ax()), (true, 0x0300));

    // A floppy drive without an image: not ready.
    fs::create_dir_all(dir.join("b")).unwrap();
    cpu.bus.mount_drive(1, &dir.join("b"), MountOptions::default(), false).unwrap();
    let host = [0xB0, 0x01, 0xB9, 0x01, 0x00, 0x31, 0xD2, 0xCD, 0x25, 0x5A, 0xF4];
    run(&mut cpu, &host);
    assert_eq!((cf(&cpu), cpu.ax()), (true, 0x8002));
}

fn tool(name: &str) -> bool {
    std::process::Command::new(name).arg("--help").output().is_ok()
}

#[test]
fn images_from_mkfs_and_checked_by_fsck() {
    if !tool("mkfs.fat") || !tool("fsck.fat") {
        eprintln!("mkfs.fat or fsck.fat is missing: skipped");
        return;
    }
    let dir = fatimage::scratch("dosfstools");
    let image = dir.join("mkfs.img");
    let status = std::process::Command::new("mkfs.fat")
        .args(["-C", image.to_str().unwrap(), "1440", "-n", "MKFS"])
        .output()
        .unwrap();
    assert!(status.status.success(), "{}", String::from_utf8_lossy(&status.stderr));

    let mut cpu = cpu(&dir);
    mount(&mut cpu, DRIVE_A, &image);
    assert_eq!(cpu.bus.disk.volume_label(DRIVE_A).as_deref(), Some("MKFS"));
    for d in ["A:\\ONE", "A:\\ONE\\TWO"] {
        name(&mut cpu, d);
        int21(&mut cpu, 0x3900);
        assert!(!cf(&cpu));
    }
    // Enough files to grow a subdirectory past its first cluster.
    for i in 0..40 {
        let handle = create(&mut cpu, &format!("A:\\ONE\\F{}.DAT", i)).unwrap();
        write(&mut cpu, handle, &pattern(1000 + i * 100, i as u8)).unwrap();
        close(&mut cpu, handle);
    }
    for i in (0..40).step_by(3) {
        name(&mut cpu, &format!("A:\\ONE\\F{}.DAT", i));
        int21(&mut cpu, 0x4100);
        assert!(!cf(&cpu));
    }
    let handle = create(&mut cpu, "A:\\ONE\\TWO\\BIG.BIN").unwrap();
    for _ in 0..10 {
        write(&mut cpu, handle, &pattern(30_000, 5)).unwrap();
    }
    close(&mut cpu, handle);
    cpu.bus.unmount_drive(DRIVE_A).unwrap();

    let check = std::process::Command::new("fsck.fat").args(["-n", image.to_str().unwrap()]).output().unwrap();
    assert!(
        check.status.success(),
        "fsck.fat: {}{}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    );
    mount(&mut cpu, DRIVE_A, &image);
    assert_eq!(read_file(&mut cpu, "A:\\ONE\\F4.DAT"), pattern(1400, 4));
    assert_eq!(read_file(&mut cpu, "A:\\ONE\\TWO\\BIG.BIN").len(), 300_000);
}

#[test]
fn shell_file_commands_work_on_disk_images() {
    use rust_dos::command::{CommandDispatcher, split_command};
    let dir = fatimage::scratch("shell_files");
    let image = floppy_with_files(&dir);
    let mut cpu = cpu(&dir);
    fs::write(dir.join("c/SAVE.DAT"), pattern(5000, 3)).unwrap();
    mount(&mut cpu, DRIVE_A, &image);
    let run = |cpu: &mut Cpu, line: &str| {
        let (command, args) = split_command(line);
        assert!(CommandDispatcher::new().dispatch(cpu, command, args));
    };

    run(&mut cpu, "COPY SAVE.DAT A:\\");
    assert_eq!(read_file(&mut cpu, "A:\\SAVE.DAT"), pattern(5000, 3));
    // A shorter file over it leaves only its bytes.
    run(&mut cpu, "COPY A:\\README.TXT A:\\SAVE.DAT");
    assert_eq!(read_file(&mut cpu, "A:\\SAVE.DAT"), b"hello floppy");
    run(&mut cpu, "MD A:\\NEW");
    run(&mut cpu, "COPY A:\\GAMES\\*.* A:\\NEW");
    assert_eq!(read_file(&mut cpu, "A:\\NEW\\DOOM.EXE"), pattern(3000, 1));
    run(&mut cpu, "REN A:\\NEW\\DOOM.EXE DOOM.BAK");
    assert_eq!(read_file(&mut cpu, "A:\\NEW\\DOOM.BAK"), pattern(3000, 1));
    run(&mut cpu, "DEL A:\\NEW\\*.*");
    run(&mut cpu, "RD A:\\NEW");
    assert!(!cpu.bus.disk.exists("A:\\NEW"));
    run(&mut cpu, "COPY A:\\README.TXT C:\\README.TXT");
    assert_eq!(fs::read(dir.join("c/README.TXT")).unwrap(), b"hello floppy");
}

#[test]
fn fcb_files_work_on_disk_images() {
    let dir = fatimage::scratch("fcb");
    let image = floppy_with_files(&dir);
    let mut cpu = cpu(&dir);
    mount(&mut cpu, DRIVE_A, &image);
    let (fcb, dta) = (0x30000, 0x40000);
    cpu.set_ds(0x4000);
    cpu.set_dx(0);
    int21(&mut cpu, 0x1A00);
    let set_fcb = |cpu: &mut Cpu, name: &[u8; 11]| {
        cpu.bus.load_bytes(fcb, &[0; 0x25]);
        cpu.bus.write_8(fcb, 1);
        cpu.bus.load_bytes(fcb + 1, name);
        cpu.set_ds(0x3000);
        cpu.set_dx(0);
    };
    set_fcb(&mut cpu, b"FCB     DAT");
    int21(&mut cpu, 0x1600);
    assert_eq!(cpu.get_al(), 0);
    cpu.bus.load_bytes(dta, &pattern(128, 5));
    int21(&mut cpu, 0x1500);
    int21(&mut cpu, 0x1000);
    assert_eq!(read_file(&mut cpu, "A:\\FCB.DAT"), pattern(128, 5));

    set_fcb(&mut cpu, b"README  TXT");
    int21(&mut cpu, 0x0F00);
    assert_eq!((cpu.get_al(), cpu.bus.read_32(fcb + 0x10)), (0, 12));
    int21(&mut cpu, 0x1400);
    assert_eq!(cpu.get_al(), 3, "a partial record");
    assert_eq!(&(0..14).map(|i| cpu.bus.read_8(dta + i)).collect::<Vec<u8>>(), b"hello floppy\0\0");
    int21(&mut cpu, 0x1000);
}
