//! DOS's data segment, in low memory below the first MCB where DOS keeps
//! it: the List of Lists (SYSVARS) and what it points to, the device
//! drivers' headers, the FCB table, the drive parameter blocks, the current
//! directory structures and a disk buffer, and the swappable data area
//! (SDA) with the InDOS flag, at the offsets MS-DOS 5 and 6 have them.
//! Windows' DOSMGR takes the List of Lists' segment for the DOS data
//! segment, which it requires in the first 64 KB, walks the tables from
//! there, and finds the rest through the patch table INT 2Fh AX=1607h
//! BX=0015h hands it.

use crate::bus::Bus;
use crate::disk::{DriveKind, LASTDRIVE};

/// Where it is: above the shell's stack (`cpu::SHELL_STACK`), below the
/// file table (`dos_files::SFT_SEGMENT`).
pub const SEGMENT: u16 = 0x0160;
/// Which format the SDA has, for SHARE and other DOS utilities: 01h for
/// DOS 4.0 to 6.0.
const SDA_FORMAT: u16 = 0x0004;
/// The List of Lists, with the first MCB's segment in the word below it.
pub const SYSVARS: u16 = 0x0026;
/// The disk buffer information, inside the List of Lists in DOS 5.
const BUFFER_INFO: u16 = SYSVARS + 0x47;
/// The patch table of the DOSMGR interface (INT 2Fh AX=1607h BX=0015h).
pub const DOSMGR_PATCHES: u16 = 0x00A0;
/// The headers of the character devices DOS has built in, and of the
/// disk driver, in the order DOS chains them after NUL (`DEVICES`).
const DEVICE_HEADERS: u16 = 0x00B0;
const DEVICE_HEADER_SIZE: u16 = 0x12;
/// A RETF, the strategy and interrupt entry of every device here: the
/// emulator does their I/O.
const DEVICE_RETF: u16 = SYSVARS + 0x6C;
/// The FCB table (FCBS=4), an SFT block of its own.
const FCB_TABLE: u16 = 0x0140;
const FCBS: u16 = 4;
/// The zero-terminated list of the places to patch for critical sections
/// (INT 2Ah AH=80h), before the SDA as in DOS 4 to 6. DOS services run in
/// one step here and need none.
const CRIT_PATCHES: u16 = SDA - 0x0B;
/// The swappable data area (INT 21h AX=5D06h), its size, and the part of
/// it at the start that is swapped even outside DOS.
pub const SDA: u16 = 0x0320;
pub const SDA_SIZE: u16 = 0x078C;
pub const SDA_ALWAYS: u16 = 0x001A;
/// The InDOS flag (AH=34h), after the critical error flag.
pub const INDOS: u16 = SDA + 0x01;
const SDA_DTA: u16 = SDA + 0x0C;
const SDA_PSP: u16 = SDA + 0x10;
const SDA_DRIVE: u16 = SDA + 0x16;
const SDA_AX: u16 = SDA + 0x1A;
/// The machine number SHARE tells processes apart by.
const SDA_USER_ID: u16 = SDA + 0x1E;
/// The media ID byte AH=1Bh and 1Ch point DS:BX at.
pub const MEDIA_ID: u16 = SDA + 0x278;
/// The caller's BX and DS, saved on entry to INT 21h.
const SDA_SAVE_BX: u16 = SDA + 0x2CA;
const SDA_SAVE_DS: u16 = SDA + 0x2CC;
/// The drive parameter blocks, a `DPB_SIZE` slot for each drive letter.
const DPBS: u16 = 0x0B00;
const DPB_SIZE: u16 = 0x40;
/// The current directory structures, one for each drive letter.
const CDS: u16 = DPBS + LASTDRIVE as u16 * DPB_SIZE;
const CDS_SIZE: u16 = 0x58;
/// The one disk buffer, a 14h-byte header and a sector.
const DISK_BUFFER: u16 = CDS + LASTDRIVE as u16 * CDS_SIZE;
const END: u16 = DISK_BUFFER + 0x14 + 0x200;

const _: () = assert!(FCB_TABLE + 6 + FCBS * crate::dos_files::ENTRY_SIZE as u16 <= CRIT_PATCHES);
const _: () = assert!(
    DEVICE_HEADERS + DEVICES.len() as u16 * DEVICE_HEADER_SIZE <= FCB_TABLE
);
const _: () = assert!(SEGMENT as usize * 16 + END as usize <= crate::dos_files::SFT_SEGMENT as usize * 16);

/// A device DOS has built in: its name and attributes.
struct Device {
    name: &'static [u8; 8],
    attributes: u16,
}

/// The devices after NUL, in the order DOS chains them. The disk driver's
/// "name" holds the number of drives it serves.
const DEVICES: [Device; 7] = [
    Device { name: b"CON     ", attributes: 0x8013 },
    Device { name: b"AUX     ", attributes: 0x8000 },
    Device { name: b"PRN     ", attributes: 0xA0C0 },
    Device { name: b"CLOCK$  ", attributes: 0x8008 },
    Device { name: b"\0\0\0\0\0\0\0\0", attributes: 0x08C2 },
    Device { name: b"COM1    ", attributes: 0x8000 },
    Device { name: b"LPT1    ", attributes: 0xA0C0 },
];
const CON_DEVICE: usize = 0;
const CLOCK_DEVICE: usize = 3;
const DISK_DEVICE: usize = 4;

/// The header of the built-in device `index` in `DEVICES`.
const fn device_header(index: usize) -> u16 {
    DEVICE_HEADERS + index as u16 * DEVICE_HEADER_SIZE
}

/// The header of a character device's driver, for the file table's
/// entries.
pub fn char_device(device: crate::disk::CharDevice, sft: u16) -> u32 {
    use crate::disk::{CharDevice, SFT_AUX, SFT_PRN};
    match device {
        CharDevice::Con => far(device_header(CON_DEVICE)),
        CharDevice::Emm => crate::ems::DEVICE_POINTER,
        CharDevice::Nul if sft == SFT_AUX => far(device_header(1)),
        CharDevice::Nul if sft == SFT_PRN => far(device_header(2)),
        CharDevice::Nul => far(NUL_DEVICE),
    }
}

/// The address of `offset` in the DOS data segment.
pub const fn address(offset: u16) -> usize {
    SEGMENT as usize * 16 + offset as usize
}

/// A far pointer (segment in the high word) to `offset` in it.
pub const fn far(offset: u16) -> u32 {
    (SEGMENT as u32) << 16 | offset as u32
}

/// The offset of drive `drive`'s DPB.
pub const fn dpb(drive: u8) -> u16 {
    DPBS + drive as u16 * DPB_SIZE
}

/// The NUL device's header, in the List of Lists.
pub const NUL_DEVICE: u16 = SYSVARS + 0x22;

/// Write the DOS data segment afresh for the mounted drives: the List of
/// Lists and its tables, the drive parameter blocks, the current directory
/// structures and the SDA.
pub fn write(bus: &mut Bus) {
    // CD-ROMs are redirector drives and have no DPB.
    let with_dpb: Vec<u8> = (0..LASTDRIVE)
        .filter(|&d| bus.disk.drive_kind(d).is_some_and(|k| k != DriveKind::CdRom))
        .collect();
    for drive in 0..LASTDRIVE {
        let base = address(dpb(drive));
        bus.fill_ram(base..base + DPB_SIZE as usize, 0);
    }
    for (i, &drive) in with_dpb.iter().enumerate() {
        if let Some(layout) = bus.disk.layout(drive) {
            write_dpb(bus, drive, layout, with_dpb.get(i + 1).copied());
        }
    }
    write_list_of_lists(bus, &with_dpb);
    write_fcb_table(bus);
    write_disk_buffer(bus, with_dpb.first().copied());
    for drive in 0..LASTDRIVE {
        write_cds(bus, drive);
    }
    bus.write_8(address(SDA_FORMAT), 0x01);
    bus.write_16(address(CRIT_PATCHES), 0);
    write_dosmgr_patches(bus);
}

/// Fill in the DOS 5 List of Lists for INT 21h AH=52h. Programs mostly
/// read the first MCB segment at offset -2 to walk the memory chain;
/// Windows' DOSMGR walks the file tables and the device chain.
fn write_list_of_lists(bus: &mut Bus, with_dpb: &[u8]) {
    let base = address(SYSVARS);
    bus.fill_ram(base..base + 0x6C, 0);
    bus.write_16(base - 2, crate::mcb::FIRST_MCB_SEG); // -2: first MCB
    // 00: far pointer to the first DPB
    match with_dpb.first() {
        Some(&d) => bus.write_32(base, far(dpb(d))),
        None => bus.write_32(base, 0xFFFF_FFFF),
    }
    // 04: far pointer to the System File Table (dos_files.rs)
    bus.write_16(base + 0x04, 0);
    bus.write_16(base + 0x06, crate::dos_files::SFT_SEGMENT);
    // 08, 0C: the CLOCK$ and CON devices
    bus.write_32(base + 0x08, far(device_header(CLOCK_DEVICE)));
    bus.write_32(base + 0x0C, far(device_header(CON_DEVICE)));
    let max_sector = with_dpb.iter().filter_map(|&d| bus.disk.layout(d)).map(|l| l.bytes_per_sector).max();
    bus.write_16(base + 0x10, max_sector.unwrap_or(512)); // 10: max bytes per sector
    bus.write_32(base + 0x12, far(BUFFER_INFO)); // 12: disk buffer info
    bus.write_32(base + 0x16, far(CDS)); // 16: current directory structures
    bus.write_32(base + 0x1A, far(FCB_TABLE)); // 1A: FCB table
    bus.write_8(base + 0x20, with_dpb.len() as u8); // 20: block devices
    bus.write_8(base + 0x21, LASTDRIVE); // 21: LASTDRIVE
    // 22: NUL device header, the first driver in the chain, followed by
    // the ones DOS has built in.
    for (i, device) in DEVICES.iter().enumerate() {
        let at = address(device_header(i));
        let next = if i + 1 < DEVICES.len() { far(device_header(i + 1)) } else { 0xFFFF_FFFF };
        write_device_header(bus, at, next, device.attributes, device.name);
    }
    bus.write_8(address(device_header(DISK_DEVICE)) + 0x0A, with_dpb.len() as u8);
    let nul = address(NUL_DEVICE);
    write_device_header(bus, nul, far(device_header(0)), 0x8004, b"NUL     ");
    // The CD-ROM driver follows NUL when there are CD drives, and the
    // expanded memory manager's device comes before it.
    crate::interrupts::mscdex::install_device(bus, nul);
    crate::ems::install_device(bus, nul);
    bus.write_8(base + 0x43, 3); // 43: boot drive C:
    // 63: whether upper memory is linked; 66: its first MCB, the one
    // that covers the memory below it; 68: where allocations search from.
    bus.write_8(base + 0x63, bus.umb.is_some_and(|u| u.linked) as u8);
    bus.write_16(base + 0x66, if bus.umb.is_some() { crate::mcb::umb_cover_seg(bus) } else { 0xFFFF });
    bus.write_16(base + 0x68, crate::mcb::FIRST_MCB_SEG);
    bus.write_8(address(DEVICE_RETF), 0xCB); // RETF for the driver entries, past the table
}

/// A device driver's header: the next driver, its attributes, its
/// strategy and interrupt entries, and its name.
fn write_device_header(bus: &mut Bus, at: usize, next: u32, attributes: u16, name: &[u8; 8]) {
    bus.write_32(at, next);
    bus.write_16(at + 0x04, attributes);
    bus.write_16(at + 0x06, DEVICE_RETF);
    bus.write_16(at + 0x08, DEVICE_RETF);
    for (i, &b) in name.iter().enumerate() {
        bus.write_8(at + 0x0A + i, b);
    }
}

/// The FCB table: an SFT block of free entries, the last block.
fn write_fcb_table(bus: &mut Bus) {
    let at = address(FCB_TABLE);
    bus.fill_ram(at..at + 6 + FCBS as usize * crate::dos_files::ENTRY_SIZE, 0);
    bus.write_32(at, 0xFFFF_FFFF);
    bus.write_16(at + 4, FCBS);
}

/// The disk buffer information in the List of Lists, and the one buffer,
/// unused, in its chain of one.
fn write_disk_buffer(bus: &mut Bus, first_drive: Option<u8>) {
    let info = address(BUFFER_INFO);
    bus.write_32(info, far(DISK_BUFFER)); // 00: the least recently used buffer
    bus.write_16(info + 0x04, 0); // 04: dirty buffers
    bus.write_32(info + 0x06, 0); // 06: no lookahead buffer
    bus.write_16(info + 0x0A, 0); // 0A: lookahead sectors
    bus.write_8(info + 0x0C, 0); // 0C: buffers in base memory
    let at = address(DISK_BUFFER);
    bus.fill_ram(at..at + 0x14, 0);
    bus.write_16(at, DISK_BUFFER); // 00, 02: next and previous, itself
    bus.write_16(at + 0x02, DISK_BUFFER);
    bus.write_8(at + 0x04, 0xFF); // 04: not in use
    bus.write_32(at + 0x0D, first_drive.map_or(0xFFFF_FFFF, |d| far(dpb(d)))); // 0D: a DPB
}

/// Drive `drive`'s current directory structure: its current directory,
/// what kind of drive it is and its DPB.
pub fn write_cds(bus: &mut Bus, drive: u8) {
    let at = address(CDS + drive as u16 * CDS_SIZE);
    bus.fill_ram(at..at + CDS_SIZE as usize, 0);
    let letter = crate::disk::drive_letter(drive) as u8;
    let dir = bus.disk.get_current_directory_of(drive).unwrap_or_default();
    let mut path = vec![letter, b':', b'\\'];
    path.extend(dir.bytes().take(0x43 - 4));
    for (i, &b) in path.iter().enumerate() {
        bus.write_8(at + i, b);
    }
    // 43: physical drive; with the network redirector's bit and
    // MSCDEX's for a CD-ROM; nothing for a drive that isn't there.
    let attributes = match bus.disk.drive_kind(drive) {
        None => 0x0000,
        Some(DriveKind::CdRom) => 0xC080,
        Some(_) => 0x4000,
    };
    bus.write_16(at + 0x43, attributes);
    if attributes == 0x4000 {
        bus.write_32(at + 0x45, far(dpb(drive))); // 45: DPB
    }
    bus.write_16(at + 0x49, 0xFFFF); // 49: current directory's cluster unknown
    bus.write_16(at + 0x4B, 0xFFFF);
    bus.write_16(at + 0x4D, 0xFFFF);
    bus.write_16(at + 0x4F, 2); // 4F: the backslash of the root
}

/// Say in the List of Lists whether upper memory is linked.
pub fn set_upper_linked(bus: &mut Bus, linked: bool) {
    bus.write_8(address(SYSVARS + 0x63), linked as u8);
}

/// Fill in a DOS 4+ style Drive Parameter Block with the drive's FAT
/// layout: a disk image's own, or a plausible one for its geometry.
fn write_dpb(bus: &mut Bus, drive: u8, layout: crate::disk::FatLayout, next: Option<u8>) {
    let spc = layout.sectors_per_cluster;
    let base = address(dpb(drive));

    bus.write_8(base, drive); // 00: drive number (0=A)
    bus.write_8(base + 0x01, drive); // 01: unit within driver
    bus.write_16(base + 0x02, layout.bytes_per_sector); // 02: bytes per sector
    bus.write_8(base + 0x04, (spc - 1) as u8); // 04: sectors per cluster - 1
    bus.write_8(base + 0x05, spc.trailing_zeros() as u8); // 05: cluster shift
    bus.write_16(base + 0x06, layout.reserved_sectors); // 06: reserved sectors
    bus.write_8(base + 0x08, layout.fats as u8); // 08: number of FATs
    bus.write_16(base + 0x09, layout.root_entries); // 09: root directory entries
    bus.write_16(base + 0x0B, layout.first_data_sector()); // 0B: first data sector
    bus.write_16(base + 0x0D, layout.clusters.saturating_add(1)); // 0D: highest cluster
    bus.write_16(base + 0x0F, layout.sectors_per_fat); // 0F: sectors per FAT
    bus.write_16(base + 0x11, layout.first_dir_sector()); // 11: first directory sector
    bus.write_8(base + 0x17, layout.media); // 17: media ID
    bus.write_8(base + 0x18, 0x00); // 18: disk accessed
    // 19: far pointer to the next DPB, FFFF:FFFF ends the chain
    bus.write_32(base + 0x19, next.map_or(0xFFFF_FFFF, |n| far(dpb(n))));
    bus.write_16(base + 0x1D, 2); // 1D: cluster to start free search
    bus.write_16(base + 0x1F, 0xFFFF); // 1F: free clusters unknown
}

/// The patch table of MS-DOS 5's DOSMGR interface: the DOS version and
/// where in the DOS data segment the INT 21h dispatcher saves the
/// caller's DS and BX, the InDOS flag, the user ID, the critical section
/// patches and the last MCB of conventional memory (UMB_HEAD) are.
fn write_dosmgr_patches(bus: &mut Bus) {
    let at = address(DOSMGR_PATCHES);
    bus.write_8(at, 5);
    bus.write_8(at + 0x01, 0);
    for (i, offset) in [SDA_SAVE_DS, SDA_SAVE_BX, INDOS, SDA_USER_ID, CRIT_PATCHES, SYSVARS + 0x66].into_iter().enumerate() {
        bus.write_16(at + 0x02 + 2 * i, offset);
    }
}

/// What DOS's INT 21h dispatcher keeps in the SDA on entry: the caller's
/// AX, BX and DS.
pub fn enter_dos(bus: &mut Bus, ax: u16, bx: u16, ds: u16) {
    bus.write_16(address(SDA_AX), ax);
    bus.write_16(address(SDA_SAVE_BX), bx);
    bus.write_16(address(SDA_SAVE_DS), ds);
}

/// The state of DOS the SDA holds, as it is after a DOS call: the DTA, the
/// running process and the current drive, with DOS left (InDOS 0).
pub fn leave_dos(bus: &mut Bus, psp: u16) {
    let (segment, offset) = (bus.dta_segment, bus.dta_offset);
    bus.write_16(address(SDA_DTA), offset);
    bus.write_16(address(SDA_DTA + 2), segment);
    bus.write_16(address(SDA_PSP), psp);
    let drive = bus.disk.get_current_drive();
    bus.write_8(address(SDA_DRIVE), drive);
    bus.write_8(address(INDOS), 0);
}
