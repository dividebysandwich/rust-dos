//! DOS's file tables in memory: the System File Table (SFT), with an entry
//! for each open file, and the job file table (JFT) of each process, in
//! its PSP, which maps the process's handles to those entries.
//!
//! The emulator keeps the open files themselves (`DiskController`, which
//! numbers them by their SFT entry) and writes the SFT into DOS memory for
//! the programs that read it: Windows finds the size of an entry from the
//! names of files it opened, and takes the table from the List of Lists.
//! The JFTs are only in memory, as in DOS, so that a program can switch
//! processes (AH=50h), build PSPs of its own (AH=55h) and move its table
//! (AH=67h, or its PSP's pointer at 34h) and find its handles there.

use crate::bus::Bus;
use crate::disk::{CharDevice, FILES, SFT_AUX, SFT_CON, SFT_PRN};

/// The size of an entry, as DOS 4 and later have them.
pub const ENTRY_SIZE: usize = 0x3B;
/// A JFT slot that refers to no file.
pub const UNUSED: u8 = 0xFF;
/// The handles a PSP has room for itself (at 18h).
const PSP_HANDLES: u16 = 20;
/// Handles 0 to 4 of a process started from the shell: standard input,
/// output and error on CON, AUX and PRN.
const STANDARD_HANDLES: [u16; 5] = [SFT_CON, SFT_CON, SFT_CON, SFT_AUX, SFT_PRN];

/// The entries in the first of the two blocks, which DOS keeps in its data
/// segment; the rest of FILES= are in a second block after it. Windows for
/// Workgroups' VSHARE.386 walks the chain until the next block is the last,
/// so a single block sends it past the end marker into memory forever.
const FIRST_BLOCK_FILES: u16 = 5;

/// Where the table is (`dos_data::Layout::sft`): in the DOS area below
/// the first MCB, below 512 KB where Windows looks for it, with the system
/// JFT after it. The first block is the one the List of Lists points to.
fn table(bus: &Bus) -> usize {
    crate::dos_data::layout(bus).sft as usize * 16
}

/// The second block, the last, right after the first, from the table.
const SECOND_TABLE: usize = 6 + FIRST_BLOCK_FILES as usize * ENTRY_SIZE;

/// The entry `sft`, from the table.
const fn entry_offset(sft: u16) -> usize {
    if sft < FIRST_BLOCK_FILES {
        6 + sft as usize * ENTRY_SIZE
    } else {
        SECOND_TABLE + 6 + (sft - FIRST_BLOCK_FILES) as usize * ENTRY_SIZE
    }
}

/// The address of the entry `sft`.
pub fn entry_address(bus: &Bus, sft: u16) -> usize {
    table(bus) + entry_offset(sft)
}

/// The handles of code that runs with no process (PSP 0), as the shell's
/// prompt and what interrupts it have: a JFT after the file table, as
/// COMMAND.COM's in DOS. From the table.
const SYSTEM_JFT: usize = entry_offset(FILES).next_multiple_of(16);

/// The bytes the table takes with the system JFT.
pub const TABLE_BYTES: usize = SYSTEM_JFT + PSP_HANDLES as usize;

/// The JFT of the process `psp`: where it is and how many handles it has
/// room for. The PSPs and their tables are reached as DOS's code does,
/// through the page tables while a service runs with paging on.
fn jft(bus: &mut Bus, psp: u16) -> Option<(u32, u16)> {
    if psp == 0 {
        return Some(((table(bus) + SYSTEM_JFT) as u32, PSP_HANDLES));
    }
    let base = psp as u32 * 16;
    let (offset, segment) = (bus.guest_read_16(base + 0x34), bus.guest_read_16(base + 0x36));
    Some((segment as u32 * 16 + offset as u32, bus.guest_read_16(base + 0x32)))
}

/// The SFT entry that handle `handle` of the process `psp` refers to, if
/// it refers to an open file.
pub fn sft_of(bus: &mut Bus, psp: u16, handle: u16) -> Option<u16> {
    let (at, size) = jft(bus, psp)?;
    if handle >= size {
        return None;
    }
    let sft = bus.guest_read_8(at + handle as u32) as u16;
    (sft != UNUSED as u16 && bus.disk.is_open(sft)).then_some(sft)
}

fn set_slot(bus: &mut Bus, psp: u16, handle: u16, sft: u8) {
    if let Some((at, size)) = jft(bus, psp)
        && handle < size
    {
        bus.guest_write_8(at + handle as u32, sft);
    }
}

/// The lowest handle of `psp` that refers to nothing.
fn free_handle(bus: &mut Bus, psp: u16) -> Option<u16> {
    let (at, size) = jft(bus, psp)?;
    (0..size).find(|&h| bus.guest_read_8(at + h as u32) == UNUSED)
}

/// Give the process `psp` a handle for the file just opened at `sft`,
/// which takes the open's reference: the lowest free one, as DOS does.
/// With none free the file is closed again: too many open files.
pub fn attach(bus: &mut Bus, psp: u16, sft: u16) -> Result<u16, u8> {
    match free_handle(bus, psp) {
        Some(handle) => {
            set_slot(bus, psp, handle, sft as u8);
            Ok(handle)
        }
        None => {
            bus.disk.close_file(sft);
            Err(0x04)
        }
    }
}

/// INT 21h AH=3Eh: close handle `handle` of `psp`, and the file with it
/// when no other handle refers to it.
pub fn close(bus: &mut Bus, psp: u16, handle: u16) -> Result<(), u8> {
    let sft = sft_of(bus, psp, handle).ok_or(0x06)?;
    set_slot(bus, psp, handle, UNUSED);
    bus.disk.close_file(sft);
    Ok(())
}

/// INT 21h AH=45h: another handle for the file of `handle`, sharing its
/// position.
pub fn duplicate(bus: &mut Bus, psp: u16, handle: u16) -> Result<u16, u8> {
    let sft = sft_of(bus, psp, handle).ok_or(0x06)?;
    let new = free_handle(bus, psp).ok_or(0x04)?;
    bus.disk.add_ref(sft);
    set_slot(bus, psp, new, sft as u8);
    Ok(new)
}

/// INT 21h AH=46h: make handle `target` refer to the file of `handle`,
/// closing what it referred to.
pub fn force_duplicate(bus: &mut Bus, psp: u16, handle: u16, target: u16) -> Result<(), u8> {
    let sft = sft_of(bus, psp, handle).ok_or(0x06)?;
    let (_, size) = jft(bus, psp).ok_or(0x06)?;
    if target >= size {
        return Err(0x06);
    }
    if target != handle {
        replace(bus, psp, target, sft);
        bus.disk.add_ref(sft);
    }
    Ok(())
}

/// Make handle `handle` of `psp` refer to the open file `sft`, whose
/// reference it takes, closing what it referred to: how the shell
/// redirects a program's input and output.
pub fn replace(bus: &mut Bus, psp: u16, handle: u16, sft: u16) {
    if let Some(old) = sft_of(bus, psp, handle) {
        bus.disk.close_file(old);
    }
    set_slot(bus, psp, handle, sft as u8);
}

/// Point handles 0 and 1 of `psp` at CON again, after a program ran with
/// them redirected.
pub fn restore_console(bus: &mut Bus, psp: u16) {
    for handle in 0..2 {
        bus.disk.add_ref(SFT_CON);
        replace(bus, psp, handle, SFT_CON);
    }
}

/// Close the handles of the process `psp`, which is ending, and the files
/// it opened for FCBs.
pub fn close_all(bus: &mut Bus, psp: u16) {
    if let Some((_, size)) = jft(bus, psp) {
        for handle in 0..size {
            let _ = close(bus, psp, handle);
        }
    }
    bus.disk.close_fcb_files(psp);
}

/// Give the PSP at `psp` its JFT, in the PSP itself: the handles of the
/// process `parent` that it inherits (those not opened with the no-inherit
/// bit), or without one, the standard handles of a program the shell
/// starts. Either way the files get a reference for each.
pub fn init_psp(bus: &mut Bus, psp: u16, parent: Option<u16>) {
    let base = psp as u32 * 16;
    let handles: Vec<Option<u16>> = (0..PSP_HANDLES)
        .map(|h| match parent {
            Some(parent) => sft_of(bus, parent, h).filter(|&sft| bus.disk.inheritable(sft)),
            None => STANDARD_HANDLES.get(h as usize).copied(),
        })
        .collect();
    for (h, sft) in handles.into_iter().enumerate() {
        let slot = match sft {
            Some(sft) => {
                bus.disk.add_ref(sft);
                sft as u8
            }
            None => UNUSED,
        };
        bus.guest_write_8(base + 0x18 + h as u32, slot);
    }
    psp_fields(bus, psp);
}

/// The fields of a PSP that say where its JFT is, and the ones DOS fills
/// in the same for every process.
fn psp_fields(bus: &mut Bus, psp: u16) {
    let base = psp as u32 * 16;
    bus.guest_write_16(base + 0x32, PSP_HANDLES);
    bus.guest_write_16(base + 0x34, 0x18);
    bus.guest_write_16(base + 0x36, psp);
    // No previous PSP (for SHARE), the DOS version, and the INT 21h RETF
    // that CP/M style calls of PSP:0050 go through.
    bus.guest_write_32(base + 0x38, 0xFFFF_FFFF);
    bus.guest_write_16(base + 0x40, u16::from_le_bytes([bus.dos_version.major, bus.dos_version.minor]));
    bus.guest_write_bytes(base + 0x50, &[0xCD, 0x21, 0xCB]);
}

/// INT 21h AH=26h (`inherit` false) and AH=55h (true): a PSP at `segment`
/// for a child of the process `parent`, a copy of its PSP whose memory
/// ends at `top`. AH=55h's child inherits its parent's handles as EXEC's
/// does; AH=26h's gets its parent's table as it is.
pub fn new_psp(bus: &mut Bus, segment: u16, parent: u16, top: u16, inherit: bool) {
    let (from, to) = (parent as u32 * 16, segment as u32 * 16);
    let table: Vec<u8> = (0..PSP_HANDLES)
        .map(|h| match jft(bus, parent) {
            Some((at, size)) if h < size => bus.guest_read_8(at + h as u32),
            _ => UNUSED,
        })
        .collect();
    if parent != 0 {
        let mut copy = [0u8; 0x100];
        bus.guest_read_bytes(from, &mut copy);
        bus.guest_write_bytes(to, &copy);
    }
    if inherit {
        init_psp(bus, segment, Some(parent));
    } else {
        // The table as it is, without references of its own.
        bus.guest_write_bytes(to + 0x18, &table);
        psp_fields(bus, segment);
    }
    bus.guest_write_bytes(to, &[0xCD, 0x20]);
    bus.guest_write_16(to + 0x02, top);
    // Where it returns to, Ctrl-Break and critical errors go: the vectors
    // as they are now.
    for (i, vector) in [0x22u32, 0x23, 0x24].into_iter().enumerate() {
        let handler = bus.guest_read_32(vector * 4);
        bus.guest_write_32(to + 0x0A + 4 * i as u32, handler);
    }
    bus.guest_write_16(to + 0x16, parent);
}

/// INT 21h AH=67h: room for `count` handles in the JFT of `psp`. A table
/// that has to grow moves to a memory block of the process's own, which
/// goes with its other memory when it ends.
pub fn set_handle_count(bus: &mut Bus, psp: u16, count: u16) -> Result<(), u8> {
    let (at, size) = jft(bus, psp).ok_or(0x06)?;
    if count <= size {
        return Ok(());
    }
    let segment = crate::mcb::alloc(bus, psp, count.div_ceil(16)).map_err(|_| 0x08u8)?;
    let to = segment as u32 * 16;
    for h in 0..count as u32 {
        let slot = if h < size as u32 { bus.guest_read_8(at + h) } else { UNUSED };
        bus.guest_write_8(to + h, slot);
    }
    let base = psp as u32 * 16;
    let (old_offset, old_segment) = (bus.guest_read_16(base + 0x34), bus.guest_read_16(base + 0x36));
    if old_offset == 0 && old_segment != psp {
        // A table this call made before.
        let _ = crate::mcb::free(bus, old_segment);
    }
    bus.guest_write_16(base + 0x32, count);
    bus.guest_write_16(base + 0x34, 0);
    bus.guest_write_16(base + 0x36, segment);
    Ok(())
}

// The file table itself is DOS's, the same for every Windows virtual
// machine, where its addresses say.

/// Write the whole table into memory: the headers of its two blocks, of
/// `FILES` entries between them, and every entry; and give the code that
/// runs without a process the standard handles.
pub fn write_table(bus: &mut Bus) {
    for h in 0..PSP_HANDLES {
        let slot = STANDARD_HANDLES.get(h as usize).map_or(UNUSED, |&sft| sft as u8);
        bus.write_8(table(bus) + SYSTEM_JFT + h as usize, slot);
    }
    let (table, second) = (table(bus), table(bus) + SECOND_TABLE);
    bus.write_32(table, (crate::dos_data::layout(bus).sft as u32) << 16 | SECOND_TABLE as u32);
    bus.write_16(table + 4, FIRST_BLOCK_FILES);
    bus.write_32(second, 0xFFFF_FFFF);
    bus.write_16(second + 4, FILES - FIRST_BLOCK_FILES);
    for sft in 0..FILES {
        write_entry(bus, sft);
    }
    bus.disk.take_dirty();
}

/// Bring the entries of the files opened, closed, read or written since
/// the last call up to date in memory, after the DOS call that did it.
/// Runs after every emulator service, most of which touch no file: only
/// the dirty entries are visited.
pub fn flush(bus: &mut Bus) {
    // All of them after a state load, and none past the table, where the
    // system JFT is.
    let (mut whole, mut moved) = bus.disk.take_dirty();
    whole &= (1 << FILES) - 1;
    while whole != 0 {
        write_entry(bus, whole.trailing_zeros() as u16);
        whole &= whole - 1;
    }
    while moved != 0 {
        let sft = moved.trailing_zeros() as u16;
        moved &= moved - 1;
        let position = bus.disk.position(sft).unwrap_or(0);
        bus.write_32(entry_address(bus, sft) + 0x15, position.min(u32::MAX as u64) as u32);
    }
}

/// Write the entry `sft` into memory as DOS 4 and later have them; a
/// free one is all zeros, with no handle referring to it.
fn write_entry(bus: &mut Bus, sft: u16) {
    let at = entry_address(bus, sft);
    // A device has no file time: DOS stamps its entry as it is opened. An
    // entry written again for the device already open there keeps that
    // stamp, so writing the whole table after a state load leaves the
    // devices' entries as the state has them, at any time.
    let kept = (bus.read_16(at + 0x05) & 0x80 != 0)
        .then(|| (bus.read_16(at + 0x0D), bus.read_16(at + 0x0F), (0..11).map(|i| bus.read_8(at + 0x20 + i)).collect::<Vec<u8>>()));
    for i in 0..ENTRY_SIZE {
        bus.write_8(at + i, 0);
    }
    let Some(mut entry) = bus.disk.sft_entry(sft) else { return };
    if let Some((time, date, name)) = kept
        && entry.device.is_some()
        && name == entry.name
    {
        (entry.time, entry.date) = (time, date);
    }
    let position = bus.disk.position(sft).unwrap_or(0);
    // The device information word (as IOCTL 4400h has it), and the
    // device's driver or the drive's parameter block.
    let (info, driver) = match entry.device {
        Some(device) => {
            let info = match device {
                CharDevice::Con => 0x80D3,
                CharDevice::Emm => 0xC080,
                CharDevice::Nul if sft == SFT_AUX || sft == SFT_PRN => 0x80C0,
                CharDevice::Prn => 0x80C0,
                CharDevice::Nul => 0x8084,
                CharDevice::Com1 | CharDevice::Com2 | CharDevice::Com3 | CharDevice::Com4 => 0x80C0,
            };
            (info, crate::dos_data::char_device(device, sft))
        }
        None => (0x0040 | entry.drive as u16 & 0x3F, crate::dos_data::far(crate::dos_data::dpb(entry.drive))),
    };
    bus.write_16(at, entry.refs);
    bus.write_16(at + 0x02, entry.mode);
    bus.write_8(at + 0x04, if entry.device.is_some() { 0 } else { 0x20 });
    bus.write_16(at + 0x05, info);
    bus.write_32(at + 0x07, driver);
    bus.write_16(at + 0x0D, entry.time);
    bus.write_16(at + 0x0F, entry.date);
    bus.write_32(at + 0x11, entry.size);
    bus.write_32(at + 0x15, position.min(u32::MAX as u64) as u32);
    for (i, &b) in entry.name.iter().enumerate() {
        bus.write_8(at + 0x20 + i, b);
    }
    bus.write_16(at + 0x31, entry.owner);
}

/// The address of handle `handle`'s slot in the JFT of `psp` (INT 2Fh
/// AX=1220h).
pub fn slot_address(bus: &mut Bus, psp: u16, handle: u16) -> Option<usize> {
    let (at, size) = jft(bus, psp)?;
    (handle < size).then_some(at as usize + handle as usize)
}
