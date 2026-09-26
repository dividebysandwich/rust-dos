//! File control blocks (FCBs): how DOS 1 programs name files, and what
//! programs of every DOS since still find in their PSP at 5Ch and 6Ch.
//! INT 21h AH=29h parses a name into one, and AH=11h and 12h search for
//! files with one.
//!
//! An FCB is a drive byte (0 the default drive, 1 A: and so on), the name
//! in 8 characters and the extension in 3, padded with spaces, and then
//! the fields the file functions use. An extended FCB comes after a 7-byte
//! header: FFh, five reserved bytes and the attributes to search for.

use iced_x86::Register;

use super::utils::read_dta_template;
use crate::bus::Bus;
use crate::cpu::Cpu;
use crate::disk::drive_letter;

/// A file name read from a command line for an FCB.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ParsedName {
    /// The bytes it took, separators and all.
    pub consumed: usize,
    /// The drive it names (0 for A:), if it names one.
    pub drive: Option<u8>,
    /// The name, if there is one, and the extension, if there is a dot.
    pub name: Option<[u8; 8]>,
    pub ext: Option<[u8; 3]>,
    /// Whether it has wildcards ('*' becomes '?' to the end of the part).
    pub wildcards: bool,
}

/// Whether a byte ends a file name in an FCB.
fn terminator(b: u8) -> bool {
    b <= 0x20 || b"./\\\"[]<>|+=;,:".contains(&b)
}

/// Read one part of a name (the name or the extension) into `out`, upper
/// case and padded with spaces. Characters past its length are skipped.
/// Returns where it ends and whether it had wildcards.
fn parse_part(text: &[u8], mut i: usize, out: &mut [u8]) -> (usize, bool) {
    out.fill(b' ');
    let mut wildcards = false;
    let mut n = 0;
    while i < text.len() && !terminator(text[i]) {
        match text[i] {
            b'*' => {
                let len = out.len();
                out[n.min(len)..].fill(b'?');
                n = len;
                wildcards = true;
            }
            c if n < out.len() => {
                out[n] = c.to_ascii_uppercase();
                wildcards |= c == b'?';
                n += 1;
            }
            _ => {}
        }
        i += 1;
    }
    (i, wildcards)
}

/// Parse a file name from `text` the way INT 21h AH=29h does:
/// separators (":.;,=+", spaces and tabs) are skipped first with
/// `skip_separators` (spaces and tabs always), then comes an optional
/// drive, the name and an extension after a dot.
pub fn parse_name(text: &[u8], skip_separators: bool) -> ParsedName {
    let mut i = 0;
    let skipped: &[u8] = if skip_separators { b" \t:.;,=+" } else { b" \t" };
    while i < text.len() && skipped.contains(&text[i]) {
        i += 1;
    }
    let mut parsed = ParsedName::default();
    if i + 1 < text.len() && text[i + 1] == b':' && text[i].is_ascii_alphabetic() {
        parsed.drive = Some(text[i].to_ascii_uppercase() - b'A');
        i += 2;
    }
    let mut name = [b' '; 8];
    let start = i;
    let (end, wild) = parse_part(text, i, &mut name);
    if end > start {
        parsed.name = Some(name);
    }
    parsed.wildcards |= wild;
    i = end;
    if text.get(i) == Some(&b'.') {
        let mut ext = [b' '; 3];
        let (end, wild) = parse_part(text, i + 1, &mut ext);
        parsed.ext = Some(ext);
        parsed.wildcards |= wild;
        i = end;
    }
    parsed.consumed = i;
    parsed
}

/// Put a parsed name into the FCB at `fcb`, as AH=29h with the option bits
/// `flags` does: without a drive, name or extension in the text, bits 1,
/// 3 and 2 leave the FCB's alone, else it gets the default drive and
/// blanks. Returns AL: FFh for a drive that isn't there, 01h for
/// wildcards, else 00h.
pub fn fill_fcb(bus: &mut Bus, fcb: usize, parsed: &ParsedName, flags: u8) -> u8 {
    match parsed.drive {
        Some(d) => {
            bus.write_8(fcb, d + 1);
        }
        None if flags & 0x02 == 0 => {
            bus.write_8(fcb, 0);
        }
        None => {}
    }
    match parsed.name {
        Some(name) => bus.load_bytes(fcb + 1, &name),
        None if flags & 0x08 == 0 => bus.load_bytes(fcb + 1, &[b' '; 8]),
        None => {}
    }
    match parsed.ext {
        Some(ext) => bus.load_bytes(fcb + 9, &ext),
        None if flags & 0x04 == 0 => bus.load_bytes(fcb + 9, &[b' '; 3]),
        None => {}
    }
    if parsed.drive.is_some_and(|d| !bus.disk.is_mounted(d)) {
        0xFF
    } else if parsed.wildcards {
        0x01
    } else {
        0x00
    }
}

/// INT 21h AH=29h: parse the file name at DS:SI into the FCB at ES:DI, with
/// the option bits in AL. SI ends after it.
pub fn parse_filename(cpu: &mut Cpu) {
    let flags = cpu.get_al();
    let at = cpu.get_physical_addr(cpu.ds(), cpu.si());
    let text: Vec<u8> = (0..128).map(|i| cpu.bus.read_8(at + i)).collect();
    let parsed = parse_name(&text, flags & 0x01 != 0);
    let fcb = cpu.get_physical_addr(cpu.es(), cpu.di());
    let al = fill_fcb(&mut cpu.bus, fcb, &parsed, flags);
    cpu.set_reg8(Register::AL, al);
    cpu.set_si(cpu.si().wrapping_add(parsed.consumed as u16));
}

/// The FCBs a program finds in its PSP at 5Ch and 6Ch: its first two
/// parameters, as COMMAND.COM parses them. Returns the AX the program
/// starts with: AL FFh if the first names a drive that isn't there, AH if
/// the second does.
pub fn set_psp_fcbs(bus: &mut Bus, psp: u16, args: &[u8]) -> u16 {
    let base = psp as usize * 16;
    set_fcbs(bus, base + 0x5C, base + 0x6C, args)
}

/// The FCBs of the first two parameters in `args` at `fcb1` and `fcb2`, as
/// `set_psp_fcbs` makes them.
pub fn set_fcbs(bus: &mut Bus, fcb1: usize, fcb2: usize, args: &[u8]) -> u16 {
    let mut ax = 0;
    let mut words = args
        .split(|&b| b == b' ' || b == b'\t')
        .filter(|w| !w.is_empty() && !w.starts_with(b"/"));
    for (i, fcb) in [fcb1, fcb2].into_iter().enumerate() {
        bus.load_bytes(fcb, &[0; 12]);
        bus.load_bytes(fcb + 1, &[b' '; 11]);
        let parsed = words.next().map(|w| parse_name(w, true)).unwrap_or_default();
        if fill_fcb(bus, fcb, &parsed, 0) == 0xFF {
            ax |= 0xFF << (8 * i);
        }
    }
    ax
}

/// Where an FCB's name is, and the attributes to search with: an extended
/// FCB's, or normal files for a plain one.
fn fcb_at(cpu: &Cpu) -> (usize, bool, u16) {
    let input = cpu.get_physical_addr(cpu.ds(), cpu.dx());
    if cpu.bus.read_8(input) == 0xFF {
        (input + 7, true, cpu.bus.read_8(input + 6) as u16)
    } else {
        (input, false, 0)
    }
}

/// The drive an FCB's drive byte stands for (0 the default).
fn fcb_drive(cpu: &Cpu, fcb: usize) -> u8 {
    match cpu.bus.read_8(fcb) {
        0 => cpu.bus.disk.get_current_drive(),
        d => d - 1,
    }
}

/// INT 21h AH=11h (find first) and 12h (find next): the next file the
/// FCB at DS:DX matches, put in the DTA as an FCB with the drive byte and
/// the file's 32-byte directory entry (behind an extended FCB's header if
/// the search was made with one). AL=00h, or FFh when there are no more.
/// The search goes on from where the FCB's current block field (0Ch)
/// says.
pub fn find(cpu: &mut Cpu, first: bool) {
    let dta = cpu.get_physical_addr(cpu.bus.dta_segment, cpu.bus.dta_offset);
    let (fcb, extended, attr) = fcb_at(cpu);
    let index = if first { 0 } else { cpu.bus.read_16(fcb + 0x0C) as usize };
    let name = read_dta_template(&cpu.bus, fcb);
    let drive = fcb_drive(cpu, fcb);
    let result = if cpu.bus.disk.is_mounted(drive) {
        let pattern = format!("{}:{}", drive_letter(drive), name);
        cpu.bus.disk.find_directory_entry(&pattern, index, attr)
    } else {
        Err(0x0F)
    };
    let Ok(entry) = result else {
        cpu.set_reg8(Register::AL, 0xFF);
        return;
    };
    cpu.set_reg8(Register::AL, 0x00);
    let out = if extended {
        cpu.bus.load_bytes(dta, &[0xFF, 0, 0, 0, 0, 0, attr as u8]);
        dta + 7
    } else {
        dta
    };
    cpu.bus.write_8(out, drive + 1);
    let mut dir_entry = [0u8; 32];
    dir_entry[..11].copy_from_slice(&super::utils::pattern_to_fcb(&entry.filename));
    dir_entry[0x0B] = entry.attr;
    dir_entry[0x16..0x18].copy_from_slice(&entry.dos_time.to_le_bytes());
    dir_entry[0x18..0x1A].copy_from_slice(&entry.dos_date.to_le_bytes());
    dir_entry[0x1C..0x20].copy_from_slice(&entry.size.to_le_bytes());
    cpu.bus.load_bytes(out + 1, &dir_entry);
    cpu.bus.write_16(fcb + 0x0C, (index + 1) as u16);
}

/// Where the file functions keep the DOS handle of an FCB's open file:
/// in its reserved bytes (18h-1Fh), with a mark that says it is one.
const HANDLE_FIELD: usize = 0x18;
const HANDLE_MARK: u16 = 0x4346;

/// Where an FCB's fields are for the file functions (after an extended
/// FCB's header).
fn fcb_start(cpu: &Cpu) -> usize {
    fcb_at(cpu).0
}

/// The DOS path an FCB names: its drive, name and extension.
fn fcb_path(cpu: &Cpu, fcb: usize) -> String {
    format!("{}:{}", drive_letter(fcb_drive(cpu, fcb)), read_dta_template(&cpu.bus, fcb))
}

fn record_size(bus: &Bus, fcb: usize) -> u16 {
    bus.read_16(fcb + 0x0E)
}

/// The FCB's open file, if the file functions opened one for it.
fn fcb_handle(cpu: &Cpu, fcb: usize) -> Option<u16> {
    (cpu.bus.read_16(fcb + HANDLE_FIELD + 2) == HANDLE_MARK)
        .then(|| cpu.bus.read_16(fcb + HANDLE_FIELD))
        .filter(|&h| cpu.bus.disk.is_open(h))
}

/// Fill in an FCB for the file just opened as `handle`: its drive, a
/// record size of 128 bytes, the file's size, date and time, block 0 and
/// the handle.
fn set_up_fcb(cpu: &mut Cpu, fcb: usize, handle: u16) {
    let drive = fcb_drive(cpu, fcb);
    let size = cpu.bus.disk.seek_file(handle, 0, 2).unwrap_or(0) as u32;
    let _ = cpu.bus.disk.seek_file(handle, 0, 0);
    let (time, date) = cpu.bus.disk.file_time(handle).unwrap_or((0, 0));
    let bus = &mut cpu.bus;
    bus.write_8(fcb, drive + 1);
    bus.write_16(fcb + 0x0C, 0);
    bus.write_16(fcb + 0x0E, 128);
    bus.write_32(fcb + 0x10, size);
    bus.write_16(fcb + 0x14, date);
    bus.write_16(fcb + 0x16, time);
    bus.write_16(fcb + HANDLE_FIELD, handle);
    bus.write_16(fcb + HANDLE_FIELD + 2, HANDLE_MARK);
}

/// INT 21h AH=0Fh: open the file the FCB names, read/write where the
/// drive lets it be written. AH=16h (`create`): make it, empty.
pub fn open(cpu: &mut Cpu, create: bool) {
    let fcb = fcb_start(cpu);
    let path = fcb_path(cpu, fcb);
    let owner = cpu.current_psp;
    if create && cpu.bus.disk.write_whole_file(&path, &[], None).is_err() {
        cpu.set_reg8(Register::AL, 0xFF);
        return;
    }
    match cpu.bus.disk.open_file(&path, 0x02, owner) {
        Ok(handle) => {
            cpu.bus.disk.set_fcb(handle);
            set_up_fcb(cpu, fcb, handle);
            cpu.set_reg8(Register::AL, 0x00);
        }
        Err(_) => cpu.set_reg8(Register::AL, 0xFF),
    }
}

/// INT 21h AH=10h: close the FCB's file.
pub fn close(cpu: &mut Cpu) {
    let fcb = fcb_start(cpu);
    let closed = match fcb_handle(cpu, fcb) {
        Some(handle) => {
            cpu.bus.write_16(fcb + HANDLE_FIELD + 2, 0);
            cpu.bus.disk.close_file(handle)
        }
        None => false,
    };
    cpu.set_reg8(Register::AL, if closed { 0x00 } else { 0xFF });
}

/// INT 21h AH=13h: delete the files the FCB names, wildcards and all.
pub fn delete(cpu: &mut Cpu) {
    let fcb = fcb_start(cpu);
    let path = fcb_path(cpu, fcb);
    let files = cpu.bus.disk.matching_files(&path).unwrap_or_default();
    let deleted = files.iter().filter(|(file, _)| cpu.bus.disk.delete_file(file).is_ok()).count();
    cpu.set_reg8(Register::AL, if deleted > 0 { 0x00 } else { 0xFF });
}

/// INT 21h AH=17h: rename the files the FCB names (at 01h) to the name at
/// 11h, whose '?'s keep the old name's characters.
pub fn rename(cpu: &mut Cpu) {
    let fcb = fcb_start(cpu);
    let path = fcb_path(cpu, fcb);
    let template = read_dta_template(&cpu.bus, fcb + 0x10);
    let dir = format!("{}:", drive_letter(fcb_drive(cpu, fcb)));
    let files = cpu.bus.disk.matching_files(&path).unwrap_or_default();
    let mut renamed = 0;
    for (file, entry) in files {
        let target = format!("{}{}", dir, crate::file_commands::apply_template(&entry.filename, &template));
        if cpu.bus.disk.rename_file(&file, &target).is_ok() {
            renamed += 1;
        }
    }
    cpu.set_reg8(Register::AL, if renamed > 0 { 0x00 } else { 0xFF });
}

/// INT 21h AH=23h: the size of the file the FCB names, in records of its
/// record size, rounded up, in its random record field.
pub fn file_size(cpu: &mut Cpu) {
    let fcb = fcb_start(cpu);
    let path = fcb_path(cpu, fcb);
    let owner = cpu.current_psp;
    let Ok(handle) = cpu.bus.disk.open_file(&path, 0, owner) else {
        cpu.set_reg8(Register::AL, 0xFF);
        return;
    };
    let size = cpu.bus.disk.seek_file(handle, 0, 2).unwrap_or(0);
    cpu.bus.disk.close_file(handle);
    let record = record_size(&cpu.bus, fcb).max(1) as u64;
    cpu.bus.write_32(fcb + 0x21, size.div_ceil(record) as u32);
    cpu.set_reg8(Register::AL, 0x00);
}

/// The sequential record of an FCB: its current block (0Ch) and record in
/// it (20h).
fn current_record(bus: &Bus, fcb: usize) -> u32 {
    bus.read_16(fcb + 0x0C) as u32 * 128 + bus.read_8(fcb + 0x20) as u32
}

fn set_current_record(bus: &mut Bus, fcb: usize, record: u32) {
    bus.write_16(fcb + 0x0C, (record / 128) as u16);
    bus.write_8(fcb + 0x20, (record % 128) as u8);
}

/// The random record field (21h): four bytes, three for records of 64
/// bytes or more.
fn random_record(bus: &Bus, fcb: usize) -> u32 {
    let value = bus.read_32(fcb + 0x21);
    if record_size(bus, fcb) >= 64 { value & 0x00FF_FFFF } else { value }
}

fn set_random_record(bus: &mut Bus, fcb: usize, record: u32) {
    if record_size(bus, fcb) >= 64 {
        bus.write_16(fcb + 0x21, record as u16);
        bus.write_8(fcb + 0x23, (record >> 16) as u8);
    } else {
        bus.write_32(fcb + 0x21, record);
    }
}

/// INT 21h AH=24h: set the random record field to the sequential record.
pub fn set_random(cpu: &mut Cpu) {
    let fcb = fcb_start(cpu);
    let record = current_record(&cpu.bus, fcb);
    set_random_record(&mut cpu.bus, fcb, record);
}

/// Read `count` records from `record` on of the FCB's file into the DTA.
/// Returns the records read (a partial last one, padded with zeros,
/// counts) and AL: 00h, 01h at the end of the file with nothing read, 02h
/// if they don't fit in the DTA's segment, 03h for a partial record.
fn read_records(cpu: &mut Cpu, fcb: usize, record: u32, count: u32) -> (u32, u8) {
    let Some(handle) = fcb_handle(cpu, fcb) else { return (0, 0x01) };
    let size = record_size(&cpu.bus, fcb).max(1) as u32;
    if cpu.bus.dta_offset as u32 + size * count > 0x1_0000 {
        return (0, 0x02);
    }
    if cpu.bus.disk.seek_file(handle, record as i64 * size as i64, 0).is_err() {
        return (0, 0x01);
    }
    let dta = cpu.get_physical_addr(cpu.bus.dta_segment, cpu.bus.dta_offset);
    let data = cpu.bus.disk.read_file(handle, (size * count) as usize).unwrap_or_default();
    if data.is_empty() {
        return (0, 0x01);
    }
    let whole = data.len() as u32 / size;
    let partial = !(data.len() as u32).is_multiple_of(size);
    let mut padded = data;
    padded.resize(padded.len().next_multiple_of(size as usize), 0);
    cpu.bus.load_bytes(dta, &padded);
    if partial { (whole + 1, 0x03) } else { (whole, if whole < count { 0x01 } else { 0x00 }) }
}

/// Write `count` records from the DTA to the FCB's file from `record` on;
/// with no records, make the file end there. Returns the records written
/// and AL: 00h, 01h if the disk is full, 02h if they don't fit in the
/// DTA's segment.
fn write_records(cpu: &mut Cpu, fcb: usize, record: u32, count: u32) -> (u32, u8) {
    let Some(handle) = fcb_handle(cpu, fcb) else { return (0, 0x01) };
    let size = record_size(&cpu.bus, fcb).max(1) as u32;
    if cpu.bus.dta_offset as u32 + size * count > 0x1_0000 {
        return (0, 0x02);
    }
    let at = record as u64 * size as u64;
    if count == 0 {
        let al = if cpu.bus.disk.set_file_size(handle, at).is_ok() { 0x00 } else { 0x01 };
        cpu.bus.write_32(fcb + 0x10, at as u32);
        return (0, al);
    }
    if cpu.bus.disk.seek_file(handle, at as i64, 0).is_err() {
        return (0, 0x01);
    }
    let dta = cpu.get_physical_addr(cpu.bus.dta_segment, cpu.bus.dta_offset);
    let data: Vec<u8> = (0..(size * count) as usize).map(|i| cpu.bus.read_8(dta + i)).collect();
    let written = cpu.bus.disk.write_file(handle, &data).unwrap_or(0) as u32;
    let end = at + written as u64;
    if end > cpu.bus.read_32(fcb + 0x10) as u64 {
        cpu.bus.write_32(fcb + 0x10, end as u32);
    }
    (written / size, if written < size * count { 0x01 } else { 0x00 })
}

/// INT 21h AH=14h and 15h (`write`): read or write the sequential record
/// and go on to the next.
pub fn sequential(cpu: &mut Cpu, write: bool) {
    let fcb = fcb_start(cpu);
    let record = current_record(&cpu.bus, fcb);
    let (done, al) = if write { write_records(cpu, fcb, record, 1) } else { read_records(cpu, fcb, record, 1) };
    set_current_record(&mut cpu.bus, fcb, record + done);
    cpu.set_reg8(Register::AL, al);
}

/// INT 21h AH=21h and 22h (`write`): read or write the random record,
/// which becomes the sequential one.
pub fn random(cpu: &mut Cpu, write: bool) {
    let fcb = fcb_start(cpu);
    let record = random_record(&cpu.bus, fcb);
    set_current_record(&mut cpu.bus, fcb, record);
    let (_, al) = if write { write_records(cpu, fcb, record, 1) } else { read_records(cpu, fcb, record, 1) };
    cpu.set_reg8(Register::AL, al);
}

/// INT 21h AH=27h and 28h (`write`): read or write CX records from the
/// random record on, CX those done; the random and sequential records go
/// on after them.
pub fn random_block(cpu: &mut Cpu, write: bool) {
    let fcb = fcb_start(cpu);
    let record = random_record(&cpu.bus, fcb);
    let count = cpu.cx() as u32;
    let (done, al) = if write { write_records(cpu, fcb, record, count) } else { read_records(cpu, fcb, record, count) };
    set_random_record(&mut cpu.bus, fcb, record + done);
    set_current_record(&mut cpu.bus, fcb, record + done);
    cpu.set_cx(done as u16);
    cpu.set_reg8(Register::AL, al);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_parsed_as_dos_parses_them() {
        let parsed = parse_name(b"  a:game.exe rest", false);
        assert_eq!(parsed.drive, Some(0));
        assert_eq!(parsed.name, Some(*b"GAME    "));
        assert_eq!(parsed.ext, Some(*b"EXE"));
        assert_eq!(parsed.consumed, 12);
        assert!(!parsed.wildcards);

        let parsed = parse_name(b";*.D?T", true);
        assert_eq!((parsed.name, parsed.ext, parsed.wildcards), (Some(*b"????????"), Some(*b"D?T"), true));
        assert_eq!(parsed.consumed, 6);

        let parsed = parse_name(b"VERYLONGNAME.TEXT,", false);
        assert_eq!((parsed.name, parsed.ext, parsed.consumed), (Some(*b"VERYLONG"), Some(*b"TEX"), 17));
        let parsed = parse_name(b"C:", false);
        assert_eq!((parsed.drive, parsed.name, parsed.ext), (Some(2), None, None));
    }
}
