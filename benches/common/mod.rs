//! Helpers shared by the benchmarks in benches/: scratch folders, machines
//! with a drive mounted, and DOS calls made the way a program makes them.
//!
//! Each bench file pulls this in with `mod common;`. Not every file uses
//! every helper, hence the `dead_code` allowance.
#![allow(dead_code)]

use iced_x86::Register;
use rust_dos::cpu::{Cpu, CpuFlags};
use rust_dos::interrupts::int21;
use std::fs;
use std::path::{Path, PathBuf};

/// A fresh, empty folder for one benchmark under the target directory's
/// scratch space (`CARGO_TARGET_TMPDIR`), so a run never touches the source
/// tree.
pub fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("bench").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A machine with `root` mounted as C:, current drive C:, current
/// directory the root.
pub fn machine(root: &Path) -> Cpu {
    Cpu::new(root.to_path_buf())
}

/// INT 21h with function `ah`; the other registers as the caller set them.
pub fn int21(cpu: &mut Cpu, ah: u8) {
    cpu.set_reg8(Register::AH, ah);
    int21::handle(cpu);
}

/// Whether the last call failed (CF set).
pub fn cf(cpu: &Cpu) -> bool {
    cpu.get_cpu_flag(CpuFlags::CF)
}

/// Where `set_dsdx_string` puts its strings (2000:0000).
const STRING_SEGMENT: u16 = 0x2000;
/// Where `set_dta` puts the DTA (4000:0000).
pub const DTA_SEGMENT: u16 = 0x4000;

/// An ASCIIZ string at 2000:0000, with DS:DX pointing at it.
pub fn set_dsdx_string(cpu: &mut Cpu, s: &str) {
    let base = (STRING_SEGMENT as usize) << 4;
    for (i, b) in s.bytes().chain(std::iter::once(0)).enumerate() {
        cpu.bus.write_8(base + i, b);
    }
    cpu.set_ds(STRING_SEGMENT);
    cpu.set_dx(0);
}

/// The DTA at 4000:0000 (INT 21h AH=1Ah).
pub fn set_dta(cpu: &mut Cpu) {
    cpu.set_ds(DTA_SEGMENT);
    cpu.set_dx(0);
    int21(cpu, 0x1A);
}

/// FindFirst (AH=4Eh) on `spec` with attributes `attr`, then FindNext
/// (AH=4Fh) until it fails, as DIR or a game's file scan does. Returns how
/// many entries were found. The DTA must be set (`set_dta`).
pub fn find_all(cpu: &mut Cpu, spec: &str, attr: u16) -> usize {
    set_dsdx_string(cpu, spec);
    cpu.set_cx(attr);
    cpu.set_cpu_flag(CpuFlags::CF, false);
    int21(cpu, 0x4E);
    let mut found = 0;
    while !cf(cpu) {
        found += 1;
        int21(cpu, 0x4F);
    }
    found
}

/// Opens `path` for reading (AH=3Dh) and closes it again (AH=3Eh), as a
/// game does to check for or load a data file. Panics if the open or the
/// close fails, so a broken scenario can't benchmark the error path by
/// accident, and a handle left open can't use up the 20 a process has.
pub fn open_close(cpu: &mut Cpu, path: &str) {
    set_dsdx_string(cpu, path);
    cpu.set_reg8(Register::AL, 0);
    cpu.set_cpu_flag(CpuFlags::CF, false);
    int21(cpu, 0x3D);
    assert!(!cf(cpu), "open {} failed with {:02X}h", path, cpu.ax());
    let handle = cpu.ax();
    cpu.set_bx(handle);
    int21(cpu, 0x3E);
    assert!(!cf(cpu), "close {} failed with {:02X}h", path, cpu.ax());
}

/// Names like those of a DOS game's folder: `count` 8.3 data files
/// ("RGN001.GFF", "SND012.VOC", ...) spread over a few extensions, plus a
/// handful of long names as GOG's installers leave beside them ("Launch
/// ... .lnk", "Manual.pdf"), which get generated short names.
pub fn game_names(count: usize) -> Vec<String> {
    const STEMS: [&str; 6] = ["RGN", "SND", "PIC", "MAP", "TXT", "OBJ"];
    const EXTS: [&str; 6] = ["GFF", "VOC", "PCX", "DAT", "TXT", "BIN"];
    const LONG: [&str; 5] = [
        "Launch The Game - Long Title.lnk",
        "Manual.pdf",
        "Cluebook.pdf",
        "goggame-1207661913.info",
        "dosbox_game_single.conf",
    ];
    let long = LONG.len().min(count);
    let mut names: Vec<String> = LONG[..long].iter().map(|s| s.to_string()).collect();
    for i in 0..count - long {
        let kind = i % STEMS.len();
        names.push(format!("{}{:03X}.{}", STEMS[kind], i, EXTS[kind]));
    }
    names
}

/// Writes `names` as small files into `dir` (created if missing), then
/// dates the folder as `age` does.
pub fn write_files(dir: &Path, names: &[String]) {
    fs::create_dir_all(dir).unwrap();
    for name in names {
        fs::write(dir.join(name), b"bench").unwrap();
    }
    age(dir);
}

/// Sets a folder's modification time to one in 2020, as a game installed
/// earlier has it. The emulator reads a folder changed in the last few
/// seconds again on every lookup, so a folder made just now would measure
/// that case only. Call it again after adding to a folder.
pub fn age(dir: &Path) {
    let mut options = fs::OpenOptions::new();
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_WRITE_ATTRIBUTES, and FILE_FLAG_BACKUP_SEMANTICS to open a
        // folder.
        options.access_mode(0x100).custom_flags(0x0200_0000);
    }
    #[cfg(not(windows))]
    options.read(true);
    let at = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
    options.open(dir).unwrap().set_modified(at).unwrap();
}

/// A folder of `count` files named as by `game_names`, in a fresh scratch
/// folder `name`; returns the folder.
pub fn game_folder(name: &str, count: usize) -> PathBuf {
    let dir = scratch(name);
    write_files(&dir, &game_names(count));
    dir
}
