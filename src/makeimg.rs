//! New disk images, as DOSBox Staging's MAKEIMG makes them: a floppy in one
//! of the standard formats, or a hard disk of a preset size, of a size in
//! MB or of a geometry, with a partition table. Formatted, they hold an
//! empty FAT12, FAT16 or FAT32 file system (the type goes by the size, as
//! DOS 5 picks it, unless asked for), with boot code that says the disk
//! isn't bootable until a system is put on it; unformatted, they are all
//! zeros.

use crate::asm16::Asm;
use crate::diskimage::{Chs, SECTOR_SIZE};
use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

const SECTOR: u64 = SECTOR_SIZE as u64;
const MB: u64 = 1 << 20;

/// A disk MAKEIMG knows by name (`-t`): its geometry and, for a floppy,
/// the layout DOS's FORMAT gives it.
#[derive(Debug, PartialEq, Eq)]
pub struct Preset {
    pub name: &'static str,
    pub description: &'static str,
    pub chs: Chs,
    pub floppy: bool,
    media: u8,
    root_entries: u32,
    sectors_per_fat: u32,
    sectors_per_cluster: u32,
}

const fn floppy(name: &'static str, description: &'static str, geometry: (u32, u32, u32), layout: (u8, u32, u32, u32)) -> Preset {
    let (cylinders, heads, sectors) = geometry;
    let (media, root_entries, sectors_per_fat, sectors_per_cluster) = layout;
    Preset { name, description, chs: Chs { cylinders, heads, sectors }, floppy: true, media, root_entries, sectors_per_fat, sectors_per_cluster }
}

const fn hard_disk(name: &'static str, description: &'static str, cylinders: u32, heads: u32) -> Preset {
    let chs = Chs { cylinders, heads, sectors: 63 };
    Preset { name, description, chs, floppy: false, media: 0xF8, root_entries: 512, sectors_per_fat: 0, sectors_per_cluster: 0 }
}

/// DOSBox Staging's disk types, the floppies from the smallest.
pub static PRESETS: [Preset; 16] = [
    floppy("fd_160kb", "160 KB floppy (5\u{00BC}\", one side)", (40, 1, 8), (0xFE, 64, 1, 1)),
    floppy("fd_180kb", "180 KB floppy (5\u{00BC}\", one side)", (40, 1, 9), (0xFC, 64, 2, 1)),
    floppy("fd_320kb", "320 KB floppy (5\u{00BC}\")", (40, 2, 8), (0xFF, 112, 1, 2)),
    floppy("fd_360kb", "360 KB floppy (5\u{00BC}\")", (40, 2, 9), (0xFD, 112, 2, 2)),
    floppy("fd_720kb", "720 KB floppy (3\u{00BD}\")", (80, 2, 9), (0xF9, 112, 3, 2)),
    floppy("fd_1200kb", "1.2 MB floppy (5\u{00BC}\")", (80, 2, 15), (0xF9, 224, 7, 1)),
    floppy("fd_1440kb", "1.44 MB floppy (3\u{00BD}\")", (80, 2, 18), (0xF0, 224, 9, 1)),
    floppy("fd_2880kb", "2.88 MB floppy (3\u{00BD}\")", (80, 2, 36), (0xF0, 240, 9, 2)),
    hard_disk("hd_20mb", "20 MB hard disk", 40, 16),
    hard_disk("hd_40mb", "40 MB hard disk", 81, 16),
    hard_disk("hd_80mb", "80 MB hard disk", 162, 16),
    hard_disk("hd_120mb", "120 MB hard disk", 243, 16),
    hard_disk("hd_250mb", "250 MB hard disk", 489, 16),
    hard_disk("hd_520mb", "520 MB hard disk", 1023, 16),
    hard_disk("hd_1gb", "1 GB hard disk", 1023, 32),
    hard_disk("hd_2gb", "2 GB hard disk", 1023, 64),
];

/// The disk type called `name`, in any case.
pub fn preset(name: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.name.eq_ignore_ascii_case(name))
}

/// What to make.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImageSpec {
    /// A disk type, or (None) a hard disk of `size_mb` or `chs`.
    pub preset: Option<&'static Preset>,
    pub size_mb: Option<u64>,
    pub chs: Option<Chs>,
    /// FAT12, 16 or 32, rather than what the size calls for.
    pub fat: Option<u8>,
    pub sectors_per_cluster: Option<u32>,
    /// The volume label.
    pub label: Option<String>,
    /// All zeros, without a partition table or a file system.
    pub unformatted: bool,
}

/// A FAT file system as `plan` lays it out, in sectors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Volume {
    /// 12, 16 or 32.
    pub bits: u8,
    /// Where it starts on the disk (after the partition table's track of
    /// a hard disk), and its size.
    pub start: u64,
    pub sectors: u64,
    pub reserved: u32,
    pub fats: u32,
    pub root_entries: u32,
    pub sectors_per_fat: u32,
    pub sectors_per_cluster: u32,
    pub media: u8,
    /// The clusters it has for files.
    pub clusters: u64,
    pub label: Option<[u8; 11]>,
}

impl Volume {
    fn root_sectors(&self) -> u64 {
        (self.root_entries as u64 * 32).div_ceil(SECTOR)
    }

    /// Where the root directory starts on the disk: after the FATs, which
    /// for FAT32 is its first cluster.
    fn root_start(&self) -> u64 {
        self.start + self.reserved as u64 + self.fats as u64 * self.sectors_per_fat as u64
    }
}

/// An image as `plan` lays it out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub chs: Chs,
    /// The file's size.
    pub bytes: u64,
    pub floppy: bool,
    /// None: unformatted.
    pub volume: Option<Volume>,
}

impl Plan {
    /// "80 cylinders, 2 heads, 18 sectors: FAT12, 2847 clusters of 512
    /// bytes".
    pub fn describe(&self) -> String {
        let Chs { cylinders, heads, sectors } = self.chs;
        let geometry = format!("{} cylinders, {} heads, {} sectors", cylinders, heads, sectors);
        match &self.volume {
            Some(v) => format!(
                "{}: FAT{}, {} clusters of {}",
                geometry,
                v.bits,
                v.clusters,
                match v.sectors_per_cluster as u64 * SECTOR {
                    bytes if bytes < 1024 => format!("{} bytes", bytes),
                    bytes => format!("{} KB", bytes / 1024),
                }
            ),
            None => format!("{}: unformatted", geometry),
        }
    }
}

/// The most clusters each FAT type has, and the fewest FAT16 and FAT32
/// have: the count of clusters is what tells them apart.
const FAT12_CLUSTERS: u64 = 4084;
const FAT16_CLUSTERS: u64 = 65524;
const FAT32_CLUSTERS: u64 = 0x0FFF_FFF4;

/// A hard disk's geometry for `bytes`, as DOSBox Staging picks it: 63
/// sectors a track, and more heads for bigger disks, so the cylinders stay
/// within the BIOS's 1024.
fn geometry_for(bytes: u64) -> Chs {
    let heads = match bytes {
        _ if bytes > 4096 * MB => 255,
        _ if bytes > 1024 * MB => 128,
        _ if bytes > 528 * MB => 64,
        _ => 16,
    };
    let cylinders = (bytes / SECTOR / (heads as u64 * 63)).min(1023) as u32;
    Chs { cylinders, heads, sectors: 63 }
}

/// Work out the image `spec` asks for.
pub fn plan(spec: &ImageSpec) -> Result<Plan, String> {
    let (chs, bytes, floppy) = match (spec.preset, spec.chs, spec.size_mb) {
        (Some(preset), ..) => (preset.chs, preset.chs.total() * SECTOR, preset.floppy),
        (None, Some(chs), _) => {
            if chs.cylinders == 0 || !(1..=255).contains(&chs.heads) || !(1..=63).contains(&chs.sectors) {
                return Err("Invalid -chs: 1 or more cylinders, 1 to 255 heads, 1 to 63 sectors".to_string());
            }
            (chs, chs.total() * SECTOR, false)
        }
        (None, None, Some(mb)) if mb > 0 => (geometry_for(mb * MB), mb * MB, false),
        (None, None, Some(_)) => return Err("Invalid disk size".to_string()),
        (None, None, None) => return Err("A hard disk of the hd type needs -size or -chs".to_string()),
    };
    let sectors = bytes / SECTOR;
    if sectors > u32::MAX as u64 {
        return Err("The disk is too big: at most 2 TB".to_string());
    }
    if spec.unformatted {
        return Ok(Plan { chs, bytes, floppy, volume: None });
    }
    // A hard disk's partition starts on the second track, after the
    // partition table's.
    let start = if floppy { 0 } else { chs.sectors as u64 };
    let volume_sectors = sectors.checked_sub(start).filter(|&s| s >= 16).ok_or("The disk is too small")?;
    let bits = match spec.fat {
        Some(bits @ (12 | 16 | 32)) => bits,
        Some(bits) => return Err(format!("Invalid -fat {} (12, 16 or 32)", bits)),
        None if floppy || volume_sectors < 4085 * 8 => 12,
        None if volume_sectors >= 2 << 21 => 32,
        None => 16,
    };
    if let Some(spc) = spec.sectors_per_cluster
        && !(spc.is_power_of_two() && spc <= 128)
    {
        return Err(format!("Invalid -spc {} (1, 2, 4, 8 ... 128)", spc));
    }
    let label = match spec.label.as_deref().map(str::trim).filter(|l| !l.is_empty()) {
        Some(label) => Some(volume_label(label)?),
        None => None,
    };
    let preset = spec.preset.filter(|p| p.floppy);
    let media = spec.preset.map_or(0xF8, |p| p.media);

    // A floppy of FAT12 has the layout DOS's FORMAT gives it.
    if let Some(p) = preset
        && bits == 12
        && spec.sectors_per_cluster.is_none_or(|spc| spc == p.sectors_per_cluster)
    {
        let mut volume = Volume {
            bits,
            start,
            sectors: volume_sectors,
            reserved: 1,
            fats: 2,
            root_entries: p.root_entries,
            sectors_per_fat: p.sectors_per_fat,
            sectors_per_cluster: p.sectors_per_cluster,
            media,
            clusters: 0,
            label,
        };
        volume.clusters = (volume_sectors - volume.root_start() - volume.root_sectors()) / p.sectors_per_cluster as u64;
        return Ok(Plan { chs, bytes, floppy, volume: Some(volume) });
    }

    let reserved = if bits == 32 { 32 } else { 1 };
    let root_entries = if bits == 32 { 0 } else { preset.map_or(512, |p| p.root_entries) };
    let root_sectors = (root_entries as u64 * 32).div_ceil(SECTOR);
    let most = match bits {
        12 => FAT12_CLUSTERS,
        16 => FAT16_CLUSTERS,
        _ => FAT32_CLUSTERS,
    };
    let data = volume_sectors.checked_sub(reserved as u64 + root_sectors).ok_or("The disk is too small")?;
    // The clusters: Windows 98's sizes for FAT32, else the smallest that
    // keep their count within the type's, as DOS's FORMAT picks them.
    let mut spc = spec.sectors_per_cluster.unwrap_or(match bits {
        32 if volume_sectors >= 32 << 21 => 64,
        32 if volume_sectors >= 16 << 21 => 32,
        32 if volume_sectors >= 8 << 21 => 16,
        32 => 8,
        _ if volume_sectors > 4085 * 8 => 4,
        _ => 1,
    });
    while spc < 128 && data / spc as u64 > most {
        spc <<= 1;
    }
    // FAT32 on a smaller disk: smaller clusters, enough for FAT32.
    if bits == 32 && spec.sectors_per_cluster.is_none() {
        while spc > 1 && data / (spc as u64) <= FAT16_CLUSTERS {
            spc >>= 1;
        }
    }
    // The FATs, with an entry for each cluster that is left after them
    // and the two first ones.
    let mut sectors_per_fat = 1;
    loop {
        let clusters = data.saturating_sub(2 * sectors_per_fat) / spc as u64;
        let need = ((clusters + 2) * bits as u64).div_ceil(8).div_ceil(SECTOR);
        if need <= sectors_per_fat {
            break;
        }
        sectors_per_fat = need;
    }
    let clusters = data.saturating_sub(2 * sectors_per_fat) / spc as u64;
    let fewest = match bits {
        12 => 1,
        16 => FAT12_CLUSTERS + 1,
        _ => FAT16_CLUSTERS + 1,
    };
    if clusters > most {
        return Err(format!("The disk is too big for FAT{}", bits));
    }
    if clusters < fewest {
        return Err(format!("The disk is too small for FAT{}", bits));
    }
    let volume = Volume {
        bits,
        start,
        sectors: volume_sectors,
        reserved,
        fats: 2,
        root_entries,
        sectors_per_fat: sectors_per_fat as u32,
        sectors_per_cluster: spc,
        media,
        clusters,
        label,
    };
    Ok(Plan { chs, bytes, floppy, volume: Some(volume) })
}

/// A volume label as the boot sector and the root directory hold it:
/// upper case, padded to 11 characters.
fn volume_label(label: &str) -> Result<[u8; 11], String> {
    let mut raw = [b' '; 11];
    if label.chars().count() > 11 {
        return Err("The label has at most 11 characters".to_string());
    }
    for (i, c) in label.chars().enumerate() {
        if !c.is_ascii() || c.is_ascii_control() || "*?/\\|.,;:+=<>[]\"".contains(c) {
            return Err(format!("A label can't have '{}'", c));
        }
        raw[i] = c.to_ascii_uppercase() as u8;
    }
    Ok(raw)
}

/// The partition table's form of a sector's number: head, sector and
/// cylinder's high bits, cylinder, or where the BIOS's addresses end for
/// sectors past them.
fn chs_bytes(lba: u64, chs: Chs) -> [u8; 3] {
    let (cylinders, heads, sectors) = (chs.cylinders as u64, chs.heads as u64, chs.sectors as u64);
    let (c, h, s) = if lba < cylinders * heads * sectors {
        ((lba / sectors / heads).min(1023), lba / sectors % heads, lba % sectors + 1)
    } else {
        (1023, heads - 1, sectors)
    };
    [h as u8, (s as u8 & 0x3F) | ((c >> 2) as u8 & 0xC0), c as u8]
}

/// A hard disk's first sector: code that loads the active partition's
/// first sector and runs it, as DOS's FDISK puts there, and the partition
/// table with the volume in its first entry.
fn master_boot_record(plan: &Plan, v: &Volume) -> [u8; SECTOR_SIZE] {
    let mut a = Asm::new(0x0600);
    a.op(&[0xFA, 0x31, 0xC0, 0x8E, 0xD0]); // CLI; XOR AX, AX; MOV SS, AX
    a.op(&[0xBC, 0x00, 0x7C]); // MOV SP, 7C00h
    a.op(&[0x8E, 0xD8, 0x8E, 0xC0, 0xFB, 0xFC]); // MOV DS, AX; MOV ES, AX; STI; CLD
    // Out of the way of the sector it loads: to 0000:0600h.
    a.op(&[0xBE, 0x00, 0x7C, 0xBF, 0x00, 0x06]); // MOV SI, 7C00h; MOV DI, 0600h
    a.op(&[0xB9, 0x00, 0x01, 0xF3, 0xA5]); // MOV CX, 100h; REP MOVSW
    a.address(&[0xEA], "MOVED"); // JMP FAR 0000:MOVED
    a.op(&[0x00, 0x00]);
    a.label("MOVED");
    a.op(&[0xBE, 0xBE, 0x07, 0xB9, 0x04, 0x00]); // MOV SI, 07BEh; MOV CX, 4
    a.label("FIND");
    a.op(&[0x80, 0x3C, 0x80]); // CMP BYTE [SI], 80h: active?
    a.jump(0x74, "FOUND"); // JE
    a.op(&[0x83, 0xC6, 0x10]); // ADD SI, 16
    a.jump(0xE2, "FIND"); // LOOP
    a.address(&[0xBE], "NO_PARTITION"); // MOV SI, message
    a.jump(0xEB, "PRINT");
    // Its first sector, where the entry says, to 0000:7C00h: five tries.
    a.label("FOUND");
    a.op(&[0xBF, 0x05, 0x00]); // MOV DI, 5
    a.label("READ");
    a.op(&[0x8B, 0x14, 0x8B, 0x4C, 0x02]); // MOV DX, [SI]; MOV CX, [SI+2]
    a.op(&[0xBB, 0x00, 0x7C, 0xB8, 0x01, 0x02, 0xCD, 0x13]); // MOV BX, 7C00h; MOV AX, 0201h; INT 13h
    a.jump(0x73, "LOADED"); // JNC
    a.op(&[0x31, 0xC0, 0xCD, 0x13, 0x4F]); // XOR AX, AX; INT 13h (reset); DEC DI
    a.jump(0x75, "READ"); // JNZ
    a.address(&[0xBE], "READ_ERROR");
    a.jump(0xEB, "PRINT");
    a.label("LOADED");
    a.op(&[0x81, 0x3E, 0xFE, 0x7D, 0x55, 0xAA]); // CMP WORD [7DFEh], AA55h
    a.jump(0x75, "NO_SYSTEM"); // JNE
    // DS:SI on its entry and DL its drive, as the boot sector expects.
    a.op(&[0xEA, 0x00, 0x7C, 0x00, 0x00]); // JMP FAR 0000:7C00h
    a.label("NO_SYSTEM");
    a.address(&[0xBE], "MISSING");
    print_and_hang(&mut a);
    a.label("NO_PARTITION");
    a.op(b"\r\nNo active partition\0");
    a.label("READ_ERROR");
    a.op(b"\r\nError loading operating system\0");
    a.label("MISSING");
    a.op(b"\r\nMissing operating system\0");
    let mut sector = [0u8; SECTOR_SIZE];
    let code = a.finish();
    sector[..code.len()].copy_from_slice(&code);

    let entry = &mut sector[0x1BE..0x1CE];
    entry[0] = 0x80;
    entry[1..4].copy_from_slice(&chs_bytes(v.start, plan.chs));
    entry[4] = match v.bits {
        12 => 0x01,
        32 => 0x0C,
        _ if v.sectors < 65536 => 0x04,
        _ if plan.bytes > 528 * MB => 0x0E,
        _ => 0x06,
    };
    entry[5..8].copy_from_slice(&chs_bytes(plan.bytes / SECTOR - 1, plan.chs));
    entry[8..12].copy_from_slice(&(v.start as u32).to_le_bytes());
    entry[12..16].copy_from_slice(&(v.sectors as u32).to_le_bytes());
    sector[510..].copy_from_slice(&[0x55, 0xAA]);
    sector
}

/// Print the ASCIIZ text at DS:SI with the BIOS (from PRINT), and wait for
/// the next interrupt forever.
fn print_and_hang(a: &mut Asm) {
    a.label("PRINT");
    a.op(&[0xAC, 0x08, 0xC0]); // LODSB; OR AL, AL
    a.jump(0x74, "HANG"); // JZ
    a.op(&[0xB4, 0x0E, 0xBB, 0x07, 0x00, 0xCD, 0x10]); // MOV AH, 0Eh; MOV BX, 7; INT 10h
    a.jump(0xEB, "PRINT");
    a.label("HANG");
    a.op(&[0xFB, 0xF4]); // STI; HLT
    a.jump(0xEB, "HANG");
}

/// The volume serial number DOS's FORMAT makes of the date and time.
fn serial_number() -> u32 {
    use chrono::{Datelike, Timelike};
    let now = crate::hosttime::now();
    let low = now.year() as u32 + (now.month() << 8) + now.day();
    let high = (now.second() << 8) + now.minute() + now.hour();
    (high << 16).wrapping_add(low)
}

/// The volume's boot sector: its BIOS parameter block, and code that says
/// the disk isn't bootable and restarts the boot with the next key.
fn boot_sector(plan: &Plan, v: &Volume) -> [u8; SECTOR_SIZE] {
    let mut s = [0u8; SECTOR_SIZE];
    let fat32 = v.bits == 32;
    // The code after the BPB, whose size tells where it starts.
    let code_at = if fat32 { 0x5A } else { 0x3E };
    s[..3].copy_from_slice(&[0xEB, code_at as u8 - 2, 0x90]);
    s[3..11].copy_from_slice(b"RUST-DOS");
    s[11..13].copy_from_slice(&(SECTOR_SIZE as u16).to_le_bytes());
    s[13] = v.sectors_per_cluster as u8;
    s[14..16].copy_from_slice(&(v.reserved as u16).to_le_bytes());
    s[16] = v.fats as u8;
    s[17..19].copy_from_slice(&(v.root_entries as u16).to_le_bytes());
    let small = !fat32 && v.sectors < 65536;
    s[19..21].copy_from_slice(&(if small { v.sectors as u16 } else { 0 }).to_le_bytes());
    s[21] = v.media;
    s[22..24].copy_from_slice(&(if fat32 { 0 } else { v.sectors_per_fat as u16 }).to_le_bytes());
    s[24..26].copy_from_slice(&(plan.chs.sectors as u16).to_le_bytes());
    s[26..28].copy_from_slice(&(plan.chs.heads as u16).to_le_bytes());
    s[28..32].copy_from_slice(&(v.start as u32).to_le_bytes());
    s[32..36].copy_from_slice(&(if small { 0 } else { v.sectors as u32 }).to_le_bytes());
    // The extended BPB: after FAT32's fields of its own.
    let ext = if fat32 {
        s[36..40].copy_from_slice(&v.sectors_per_fat.to_le_bytes());
        s[44..48].copy_from_slice(&2u32.to_le_bytes()); // the root's cluster
        s[48..50].copy_from_slice(&1u16.to_le_bytes()); // FSInfo
        s[50..52].copy_from_slice(&6u16.to_le_bytes()); // the boot sector's copy
        0x40
    } else {
        0x24
    };
    s[ext] = if plan.floppy { 0x00 } else { 0x80 };
    s[ext + 2] = 0x29;
    s[ext + 3..ext + 7].copy_from_slice(&serial_number().to_le_bytes());
    s[ext + 7..ext + 18].copy_from_slice(&v.label.unwrap_or(*b"NO NAME    "));
    s[ext + 18..ext + 26].copy_from_slice(format!("FAT{:<5}", v.bits).as_bytes());

    let mut a = Asm::new(0x7C00 + code_at as u16);
    a.op(&[0xFA, 0x31, 0xC0, 0x8E, 0xD0]); // CLI; XOR AX, AX; MOV SS, AX
    a.op(&[0xBC, 0x00, 0x7C, 0x8E, 0xD8, 0xFB, 0xFC]); // MOV SP, 7C00h; MOV DS, AX; STI; CLD
    a.address(&[0xBE], "MESSAGE"); // MOV SI, message
    a.label("PRINT");
    a.op(&[0xAC, 0x08, 0xC0]); // LODSB; OR AL, AL
    a.jump(0x74, "KEY"); // JZ
    a.op(&[0xB4, 0x0E, 0xBB, 0x07, 0x00, 0xCD, 0x10]); // MOV AH, 0Eh; MOV BX, 7; INT 10h
    a.jump(0xEB, "PRINT");
    a.label("KEY");
    a.op(&[0x31, 0xC0, 0xCD, 0x16, 0xCD, 0x19]); // XOR AX, AX; INT 16h; INT 19h
    a.label("MESSAGE");
    a.op(b"\r\nNon-system disk or disk error\r\nReplace and press any key when ready\r\n\0");
    let code = a.finish();
    s[code_at..code_at + code.len()].copy_from_slice(&code);
    s[510..].copy_from_slice(&[0x55, 0xAA]);
    s
}

/// FAT32's FSInfo sector: the free clusters, all but the root's, and the
/// first free one.
fn fs_info(v: &Volume) -> [u8; SECTOR_SIZE] {
    let mut s = [0u8; SECTOR_SIZE];
    s[..4].copy_from_slice(&0x4161_5252u32.to_le_bytes());
    s[484..488].copy_from_slice(&0x6141_7272u32.to_le_bytes());
    s[488..492].copy_from_slice(&(v.clusters as u32 - 1).to_le_bytes());
    s[492..496].copy_from_slice(&3u32.to_le_bytes());
    s[508..].copy_from_slice(&0xAA55_0000u32.to_le_bytes());
    s
}

/// The first sector of each FAT: the media byte and the reserved entries,
/// and FAT32's root directory's cluster, which is all of it.
fn fat_start(v: &Volume) -> [u8; SECTOR_SIZE] {
    let mut s = [0u8; SECTOR_SIZE];
    match v.bits {
        12 => s[..3].copy_from_slice(&[v.media, 0xFF, 0xFF]),
        16 => s[..4].copy_from_slice(&[v.media, 0xFF, 0xFF, 0xFF]),
        _ => {
            s[..4].copy_from_slice(&(0x0FFF_FF00 | v.media as u32).to_le_bytes());
            s[4..8].copy_from_slice(&0x0FFF_FFFFu32.to_le_bytes());
            s[8..12].copy_from_slice(&0x0FFF_FFFFu32.to_le_bytes());
        }
    }
    s
}

/// Make the image `plan` lays out at `path`. An existing file is an error
/// unless `overwrite`.
pub fn write(path: &Path, plan: &Plan, overwrite: bool) -> Result<(), String> {
    let shown = path.display();
    let mut options = OpenOptions::new();
    options.write(true);
    if overwrite {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }
    let mut file = options.open(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => format!("File {} already exists. Use -force to overwrite.", shown),
        _ => format!("Cannot open file {} for writing: {}", shown, e),
    })?;
    let full = |e: std::io::Error| format!("Disk full or cannot allocate image size: {}", e);
    file.set_len(plan.bytes).map_err(full)?;
    let Some(v) = &plan.volume else { return Ok(()) };
    let mut put = |sector: u64, bytes: &[u8]| -> Result<(), String> {
        file.seek(SeekFrom::Start(sector * SECTOR)).map_err(full)?;
        file.write_all(bytes).map_err(full)
    };
    if !plan.floppy {
        put(0, &master_boot_record(plan, v))?;
    }
    let boot = boot_sector(plan, v);
    put(v.start, &boot)?;
    if v.bits == 32 {
        let info = fs_info(v);
        put(v.start + 1, &info)?;
        put(v.start + 6, &boot)?;
        put(v.start + 7, &info)?;
    }
    for fat in 0..v.fats as u64 {
        put(v.start + v.reserved as u64 + fat * v.sectors_per_fat as u64, &fat_start(v))?;
    }
    if let Some(label) = v.label {
        put(v.root_start(), &crate::fat::FatVolume::new_entry(&label, crate::fat::ATTR_VOLUME, 0))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diskimage::DiskImage;
    use crate::fat::FatVolume;

    fn spec(name: &str) -> ImageSpec {
        ImageSpec { preset: preset(name), ..Default::default() }
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("rust-dos-makeimg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn floppies_have_the_formats_dos_gives_them() {
        let plan = plan(&spec("FD_1440KB")).unwrap();
        assert_eq!((plan.bytes, plan.floppy), (1_474_560, true));
        let v = plan.volume.as_ref().unwrap();
        assert_eq!((v.bits, v.media, v.root_entries, v.sectors_per_fat, v.sectors_per_cluster), (12, 0xF0, 224, 9, 1));
        assert_eq!(v.clusters, 2847);
        assert_eq!(plan.describe(), "80 cylinders, 2 heads, 18 sectors: FAT12, 2847 clusters of 512 bytes");
        let v = super::plan(&spec("fd_360kb")).unwrap().volume.unwrap();
        assert_eq!((v.sectors_per_fat, v.sectors_per_cluster, v.clusters), (2, 2, 354));
        for p in PRESETS.iter().filter(|p| p.floppy) {
            assert!(crate::diskimage::floppy_geometry(p.chs.total() * SECTOR).is_some(), "{} mounts as a floppy", p.name);
        }
    }

    #[test]
    fn the_fat_type_goes_by_the_size() {
        let bits = |spec: ImageSpec| plan(&spec).map(|p| p.volume.unwrap().bits);
        let hd = |mb: u64| ImageSpec { size_mb: Some(mb), ..Default::default() };
        assert_eq!(bits(hd(10)), Ok(12));
        assert_eq!(bits(spec("hd_20mb")), Ok(16));
        assert_eq!(bits(spec("hd_2gb")), Ok(16));
        assert_eq!(bits(hd(3000)), Ok(32));
        // Clusters as DOS's FORMAT sizes them: 2 KB to 127 MB, 8 KB at 500.
        assert_eq!(plan(&spec("hd_120mb")).unwrap().volume.unwrap().sectors_per_cluster, 4);
        assert_eq!(plan(&hd(500)).unwrap().volume.unwrap().sectors_per_cluster, 16);
        assert_eq!(plan(&spec("hd_2gb")).unwrap().volume.unwrap().sectors_per_cluster, 64);
        // Asked for.
        assert_eq!(bits(ImageSpec { fat: Some(32), ..hd(600) }), Ok(32));
        let small = plan(&ImageSpec { fat: Some(32), ..spec("hd_120mb") }).unwrap().volume.unwrap();
        assert_eq!((small.bits, small.sectors_per_cluster), (32, 2), "enough clusters for FAT32");
        assert_eq!(bits(ImageSpec { fat: Some(16), ..hd(10) }), Ok(16));
        assert!(bits(ImageSpec { fat: Some(32), ..spec("hd_20mb") }).unwrap_err().contains("too small for FAT32"));
        assert!(bits(ImageSpec { fat: Some(12), ..hd(1000) }).unwrap_err().contains("too big for FAT12"));
        assert!(bits(ImageSpec { fat: Some(14), ..hd(10) }).is_err());
        assert!(bits(ImageSpec::default()).unwrap_err().contains("-size or -chs"));
        assert!(bits(ImageSpec { sectors_per_cluster: Some(3), ..hd(10) }).is_err());
    }

    #[test]
    fn hard_disks_get_a_geometry_for_their_size() {
        let p = plan(&ImageSpec { size_mb: Some(500), ..Default::default() }).unwrap();
        assert_eq!((p.chs, p.bytes), (Chs { cylinders: 1015, heads: 16, sectors: 63 }, 500 * MB));
        assert_eq!(geometry_for(1000 * MB).heads, 64);
        assert_eq!(geometry_for(2000 * MB).heads, 128);
        assert_eq!(geometry_for(8000 * MB), Chs { cylinders: 1019, heads: 255, sectors: 63 });
        let chs = Chs { cylinders: 100, heads: 4, sectors: 17 };
        let p = plan(&ImageSpec { chs: Some(chs), unformatted: true, ..Default::default() }).unwrap();
        assert_eq!((p.chs, p.bytes, p.volume), (chs, 100 * 4 * 17 * 512, None));
        assert!(plan(&ImageSpec { chs: Some(Chs { heads: 0, ..chs }), ..Default::default() }).is_err());
        // The partition ends where the disk's addresses do.
        assert_eq!(chs_bytes(63, Chs { cylinders: 40, heads: 16, sectors: 63 }), [1, 1, 0]);
        assert_eq!(chs_bytes(40319, Chs { cylinders: 40, heads: 16, sectors: 63 }), [15, 63, 39]);
        assert_eq!(chs_bytes(1 << 30, Chs { cylinders: 1023, heads: 64, sectors: 63 }), [63, 63 | 0xC0, 0xFF]);
    }

    #[test]
    fn labels_are_upper_case_and_short() {
        assert_eq!(&volume_label("Games").unwrap(), b"GAMES      ");
        assert!(volume_label("MUCH TOO LONG").is_err());
        assert!(volume_label("A*B").is_err());
    }

    #[test]
    fn images_mount_and_take_files() {
        for (name, spec) in [
            ("floppy.img", ImageSpec { label: Some("disk1".into()), ..spec("fd_720kb") }),
            ("hdd.img", ImageSpec { label: Some("HARDDISK".into()), ..spec("hd_20mb") }),
            ("small.img", ImageSpec { size_mb: Some(4), ..Default::default() }),
        ] {
            let path = scratch(name);
            let plan = plan(&spec).unwrap();
            write(&path, &plan, false).unwrap();
            assert_eq!(std::fs::metadata(&path).unwrap().len(), plan.bytes);
            assert!(write(&path, &plan, false).unwrap_err().contains("already exists"));
            write(&path, &plan, true).unwrap();

            let bytes = std::fs::read(&path).unwrap();
            assert_eq!(&bytes[510..512], [0x55, 0xAA]);
            let floppy = spec.preset.is_some_and(|p| p.floppy);
            let image = DiskImage::open(&path, floppy, None, false).unwrap();
            let (start, sectors) = image.fat_volume().unwrap();
            let v = plan.volume.as_ref().unwrap();
            assert_eq!((start, sectors), (v.start, v.sectors), "{}", name);
            let volume = FatVolume::open(std::rc::Rc::new(image), start, sectors).unwrap();
            assert_eq!(volume.label(), spec.label.as_ref().map(|l| l.to_uppercase()), "{}", name);
            assert_eq!(volume.free_clusters() as u64, v.clusters, "{}", name);
            volume.put_file(&["README.TXT"], b"hello", 0, 0).unwrap();
            assert!(volume.find(&["README.TXT"]).is_ok(), "{}", name);
            assert_eq!(volume.free_clusters() as u64, v.clusters - 1, "{}", name);
        }
    }

    #[test]
    fn the_boot_code_says_the_disk_is_not_bootable() {
        let plan = plan(&spec("hd_40mb")).unwrap();
        let v = plan.volume.as_ref().unwrap();
        let mbr = master_boot_record(&plan, v);
        assert_eq!(mbr[0x1BE..0x1C3], [0x80, 1, 1, 0, 0x06]);
        assert_eq!(u32::from_le_bytes(mbr[0x1C6..0x1CA].try_into().unwrap()), 63);
        assert!(mbr.windows(24).any(|w| w == b"Missing operating system"));
        let boot = boot_sector(&plan, v);
        assert_eq!(boot[..3], [0xEB, 0x3C, 0x90]);
        assert_eq!(&boot[0x36..0x3E], b"FAT16   ");
        assert!(boot.windows(15).any(|w| w == b"Non-system disk"));
        assert!(crate::diskimage::Bpb::parse(&boot).is_some());
        assert!(crate::diskimage::Bpb::parse(&mbr).is_none(), "the partition table isn't taken for a BPB");
        let plan = super::plan(&ImageSpec { size_mb: Some(3000), ..Default::default() }).unwrap();
        let boot = boot_sector(&plan, plan.volume.as_ref().unwrap());
        assert_eq!((boot[1], &boot[0x52..0x5A]), (0x58, &b"FAT32   "[..]));
    }
}
