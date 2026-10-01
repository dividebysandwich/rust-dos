//! MEM, as MS-DOS 6.22 has it: how much conventional, upper, extended
//! (XMS) and expanded (EMS) memory there is, and how much of it is used
//! and free.
//!
//! ```text
//! MEM [/CLASSIFY | /DEBUG | /FREE | /MODULE name] [/PAGE]
//! ```
//!
//! /C lists the programs in memory below 1 MB, /F the free blocks, /M the
//! blocks of one program, and /D every block. The numbers are the MCB
//! chains' (mcb.rs), the XMS driver's (xms.rs) and the EMS driver's
//! (ems.rs). Below the first MCB are the interrupt vectors, the BIOS's
//! data, DOS's data and the shell, which MEM counts as MSDOS. A program's
//! blocks go by the name in the MCB of its PSP's block, which EXEC writes.
//! /P, which pauses after each screen, is taken and ignored, as DIR's is.

use crate::bus::Bus;
use crate::command::{ShellCommand, format_size};
use crate::cpu::Cpu;
use crate::mcb::{self, DOS_OWNER, Mcb, UMB_START};
use crate::video::print_string;

const USAGE: &str = concat!(
    "Displays the amount of used and free memory in your system.\r\n",
    "\r\n",
    "MEM [/CLASSIFY | /DEBUG | /FREE | /MODULE modulename] [/PAGE]\r\n",
    "\r\n",
    "  /CLASSIFY or /C  Classifies programs by memory usage. Lists the size of\r\n",
    "                   programs, provides a summary of memory in use, and lists\r\n",
    "                   largest memory block available.\r\n",
    "  /DEBUG or /D     Displays status of all modules in memory, internal drivers,\r\n",
    "                   and other information.\r\n",
    "  /FREE or /F      Displays information about the amount of free memory left\r\n",
    "                   in both conventional and upper memory.\r\n",
    "  /MODULE or /M    Displays a detailed listing of a module's memory use.\r\n",
    "                   This option must be followed by the name of a module,\r\n",
    "                   optionally separated from /M by a colon.\r\n",
    "  /PAGE or /P      Pauses after each screenful of information.\r\n",
);

/// What MEM shows.
#[derive(Debug, PartialEq, Eq)]
enum Report {
    Summary,
    Classify,
    Debug,
    Free,
    Module(String),
    Help,
}

/// The report the switches ask for, or the message for switches that
/// don't go together.
fn parse(args: &str) -> Result<Report, String> {
    let mut report = Report::Summary;
    let mut words = args.split_whitespace().flat_map(split_switches).peekable();
    while let Some(word) = words.next() {
        let Some(switch) = word.strip_prefix('/') else {
            return Err(format!("Invalid parameter - {}", word));
        };
        let (name, value) = match switch.split_once(':') {
            Some((name, value)) => (name, Some(value)),
            None => (switch, None),
        };
        let next = match name.to_ascii_uppercase().as_str() {
            "?" => return Ok(Report::Help),
            "P" | "PAGE" => continue,
            "C" | "CLASSIFY" => Report::Classify,
            "D" | "DEBUG" => Report::Debug,
            "F" | "FREE" => Report::Free,
            "M" | "MODULE" => {
                let module = match value.filter(|v| !v.is_empty()) {
                    Some(module) => module.to_string(),
                    None => match words.next_if(|w| !w.starts_with('/')) {
                        Some(module) => module,
                        None => return Err("Required parameter missing - /MODULE".to_string()),
                    },
                };
                Report::Module(module.to_ascii_uppercase())
            }
            _ => return Err(format!("Invalid switch - {}", word)),
        };
        if report != Report::Summary {
            return Err(format!("Too many switches - {}", word));
        }
        report = next;
    }
    Ok(report)
}

/// A word of the command line split before each '/', as DOS takes "/C/P".
fn split_switches(word: &str) -> Vec<String> {
    let mut parts = Vec::new();
    for (i, part) in word.split('/').enumerate() {
        match i {
            0 if part.is_empty() => {}
            0 => parts.push(part.to_string()),
            _ => parts.push(format!("/{}", part)),
        }
    }
    parts
}

pub struct MemCommand;
impl ShellCommand for MemCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        let text = text(&mut cpu.bus, args);
        print_string(cpu, &text);
    }
}

/// What MEM with the switches `args` shows.
pub fn text(bus: &mut Bus, args: &str) -> String {
    match parse(args) {
        Ok(Report::Summary) => summary(&Memory::read(bus)),
        Ok(Report::Classify) => classify(&Memory::read(bus)),
        Ok(Report::Debug) => debug(&Memory::read(bus)),
        Ok(Report::Free) => free(&Memory::read(bus)),
        Ok(Report::Module(name)) => module(&Memory::read(bus), &name),
        Ok(Report::Help) => USAGE.to_string(),
        Err(e) => format!("{}\r\n", e),
    }
}

/// Where a block is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Region {
    Conventional,
    Upper,
}

/// What a block holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// Below DOS: the interrupt vectors, the BIOS's data and DOS's.
    Vectors,
    RomArea,
    DosArea,
    /// DOS's own: its data and the shell below the first MCB, and DOS's
    /// blocks.
    System,
    /// A program's block, with its PSP.
    Program,
    /// A program's environment.
    Environment,
    /// Another block a program allocated.
    Data,
    Free,
}

impl Kind {
    /// What /D and /M call it.
    fn name(self) -> &'static str {
        match self {
            Kind::Vectors => "Interrupt Vector",
            Kind::RomArea => "ROM Communication Area",
            Kind::DosArea => "DOS Communication Area",
            Kind::System => "System Data",
            Kind::Program => "Program",
            Kind::Environment => "Environment",
            Kind::Data => "Data",
            Kind::Free => "-- Free --",
        }
    }
}

/// The memory below the first MCB, by segment, all of it MSDOS's.
fn low_areas(first_mcb: u16) -> [(u16, u16, Kind); 4] {
    [
        (0x0000, 0x0040, Kind::Vectors),
        (0x0040, 0x0050, Kind::RomArea),
        (0x0050, 0x0070, Kind::DosArea),
        (0x0070, first_mcb, Kind::System),
    ]
}

/// A block of memory: where it or its MCB is, its size with the MCB in
/// bytes, who has it and what it holds.
#[derive(Clone, Debug)]
struct Block {
    segment: u16,
    bytes: u32,
    region: Region,
    name: String,
    kind: Kind,
}

/// The memory there is: the blocks below 1 MB, and the totals.
struct Memory {
    blocks: Vec<Block>,
    /// Conventional memory, and upper memory (0 without), in bytes.
    conventional: u32,
    upper: u32,
    /// The rest of the first megabyte, the adapters', so that all of it
    /// adds up to the RAM, as MS-DOS's MEM counts it.
    reserved: u32,
    /// Extended memory above 1 MB, and how much of it XMS has free.
    extended: u32,
    xms_free: u32,
    /// Expanded memory's total and free bytes, with expanded memory on.
    ems: Option<(u32, u32)>,
    /// The largest free blocks of conventional and upper memory, in bytes.
    largest_conventional: u32,
    largest_upper: u32,
    /// Whether programs can have the HMA: DOS isn't in it.
    hma_free: bool,
}

impl Memory {
    fn read(bus: &mut Bus) -> Memory {
        let conventional = mcb::conventional_end(bus) as u32 * 16;
        let block = |segment, bytes, kind| Block { segment, bytes, region: Region::Conventional, name: "MSDOS".to_string(), kind };
        let mut blocks: Vec<Block> =
            low_areas(mcb::first_mcb(bus)).iter().map(|&(from, to, kind)| block(from, (to - from) as u32 * 16, kind)).collect();
        // The cover of the adapters' memory: its MCB is DOS's, in
        // conventional memory, and what it covers is reserved. The chain
        // reaches it only with upper memory linked.
        let cover = bus.umb.map(|_| mcb::umb_cover_seg(bus));
        if let Some(segment) = cover {
            blocks.push(block(segment, 16, Kind::System));
        }
        for (segment, m) in mcb::walk_all(bus) {
            if Some(segment) == cover {
                continue;
            }
            let (region, end) = if segment >= UMB_START {
                (Region::Upper, mcb::UMB_END as u32 * 16)
            } else {
                (Region::Conventional, conventional)
            };
            let bytes = ((m.size as u32 + 1) * 16).min(end.saturating_sub(segment as u32 * 16));
            let (name, kind) = owner(bus, segment, &m);
            blocks.push(Block { segment, bytes, region, name, kind });
        }
        blocks.sort_by_key(|b| b.segment);
        let upper = bus.umb.map_or(0, |umb| umb.size as u32 * 16);
        let ram = bus.ram().len() as u32;
        let largest = |region| {
            let free = blocks.iter().filter(|b| b.kind == Kind::Free && b.region == region);
            free.map(|b| b.bytes.saturating_sub(16)).max().unwrap_or(0)
        };
        Memory {
            conventional,
            upper,
            reserved: 0x10_0000u32.saturating_sub(conventional + upper),
            extended: ram.saturating_sub(0x10_0000),
            xms_free: bus.xms.free_kb(ram).1 * 1024,
            ems: crate::ems::pages(bus).map(|(free, total)| (total as u32 * 0x4000, free as u32 * 0x4000)),
            largest_conventional: largest(Region::Conventional),
            largest_upper: largest(Region::Upper),
            hma_free: !bus.xms.hma_allocated(),
            blocks,
        }
    }

    /// The bytes free in a region.
    fn free(&self, region: Region) -> u32 {
        self.free_blocks(region).map(|b| b.bytes).sum()
    }

    fn free_blocks(&self, region: Region) -> impl Iterator<Item = &Block> {
        self.blocks.iter().filter(move |b| b.kind == Kind::Free && b.region == region)
    }

    /// The rows of the summary: name, total and free, with the used between
    /// them the difference.
    fn rows(&self) -> [(&'static str, u32, u32); 4] {
        // EMM386 simulates expanded memory with extended memory as this
        // does: the asterisk points at the note that says so.
        let xms = if self.ems.is_some() { "Extended (XMS)*" } else { "Extended (XMS)" };
        [
            ("Conventional", self.conventional, self.free(Region::Conventional)),
            ("Upper", self.upper, self.free(Region::Upper)),
            ("Reserved", self.reserved, 0),
            (xms, self.extended, self.xms_free),
        ]
    }

    /// What the last line says of the HMA. DOS isn't in it (INT 21h
    /// AX=3306h says so), so programs can have it from the XMS driver.
    fn hma(&self) -> &'static str {
        if self.hma_free { "The high memory area is available." } else { "The high memory area is not available." }
    }
}

/// Who has the block whose MCB `m` is at `segment`, and what it holds.
fn owner(bus: &mut Bus, segment: u16, m: &Mcb) -> (String, Kind) {
    match m.owner {
        mcb::FREE_OWNER => (String::new(), Kind::Free),
        DOS_OWNER => ("MSDOS".to_string(), Kind::System),
        psp if psp == segment.wrapping_add(1) => (program_name(bus, psp), Kind::Program),
        psp => {
            let environment = bus.guest_read_16(psp as u32 * 16 + 0x2C) == segment.wrapping_add(1);
            (program_name(bus, psp), if environment { Kind::Environment } else { Kind::Data })
        }
    }
}

/// The name of the program whose PSP is at `psp`: its block's, or where
/// that has none, the file name at the end of its environment.
fn program_name(bus: &mut Bus, psp: u16) -> String {
    let name = mcb::read_name(bus, psp.wrapping_sub(1));
    if !name.is_empty() {
        return name;
    }
    environment_program(bus, psp).unwrap_or_else(|| "(unknown)".to_string())
}

/// The program file's name without its extension, from the path after
/// the environment of the program whose PSP is at `psp`, if the program
/// has an environment block of its own.
fn environment_program(bus: &mut Bus, psp: u16) -> Option<String> {
    let environment = bus.guest_read_16(psp as u32 * 16 + 0x2C);
    let block = mcb::read_mcb(bus, environment.wrapping_sub(1));
    if environment == 0 || !block.is_valid() || block.owner != psp {
        return None;
    }
    let (base, limit) = (environment as u32 * 16, block.size as u32 * 16);
    // The variables end at an empty one; a count of 1 and the path follow.
    let mut at = 0;
    while at + 1 < limit && (bus.guest_read_8(base + at) != 0 || bus.guest_read_8(base + at + 1) != 0) {
        at += 1;
    }
    if bus.guest_read_16(base + at + 2) != 1 {
        return None;
    }
    let path: Vec<u8> = (base + at + 4..base + limit).map(|a| bus.guest_read_8(a)).take_while(|&b| b != 0).collect();
    let file = path.rsplit(|&b| b == b'\\' || b == b':').next()?;
    let stem = file.split(|&b| b == b'.').next()?;
    (!stem.is_empty()).then(|| String::from_utf8_lossy(stem).to_ascii_uppercase())
}

/// Bytes in KB, rounded as MEM rounds them.
fn kb(bytes: u32) -> u32 {
    (bytes + 512) / 1024
}

/// "1,234K".
fn k(kbytes: u32) -> String {
    format!("{}K", format_size(kbytes as u64))
}

/// "(1,234K)".
fn paren_k(bytes: u32) -> String {
    format!("({}K)", format_size(kb(bytes) as u64))
}

/// "1,234".
fn n(bytes: u32) -> String {
    format_size(bytes as u64)
}

/// A size in bytes and in KB in a column 16 wide: "  17,197   (17K)".
fn bytes_k(bytes: u32) -> String {
    format!("{:>8}{:>8}", n(bytes), paren_k(bytes))
}

/// A line of the summary in KB with the size in bytes after it, as the
/// largest blocks and expanded memory have it. MS-DOS 6.22 cuts off what
/// doesn't fit its columns, tens of megabytes of expanded memory; here
/// they widen.
fn kb_line(label: &str, bytes: u32) -> String {
    format!("{:<36}{:>6} {:>15}  \r\n", label, k(kb(bytes)), format!("({} bytes)", n(bytes)))
}

/// The same in /C's summary, in bytes with the KB after them.
fn bytes_line(label: &str, bytes: u32) -> String {
    format!("  {:<36}{:>10} {:>8}\r\n", label, n(bytes), paren_k(bytes))
}

/// Expanded memory comes out of extended memory, as EMM386's does.
const EMS_NOTE: [&str; 2] =
    ["* EMM386 is using XMS memory to simulate EMS memory as needed.", "  Free EMS memory may change as free XMS memory changes."];

/// MEM: the memory in KB.
fn summary(memory: &Memory) -> String {
    let mut out = String::from("\r\nMemory Type        Total  =   Used  +   Free\r\n");
    let rule = "----------------  -------   -------   -------\r\n";
    out.push_str(rule);
    let row = |label: &str, (total, free): (u32, u32)| {
        format!("{:<16}  {:>7}   {:>7}   {:>7}\r\n", label, k(total), k(total - free), k(free))
    };
    // In KB, the used the difference, so that the columns add up.
    let rows = memory.rows().map(|(label, total, free)| (label, kb(total), kb(free)));
    let sum = |rows: &[(&str, u32, u32)]| rows.iter().fold((0, 0), |(t, f), &(_, total, free)| (t + total, f + free));
    for (label, total, free) in rows {
        out.push_str(&row(label, (total, free)));
    }
    out.push_str(rule);
    out.push_str(&row("Total memory", sum(&rows)));
    out.push_str("\r\n");
    out.push_str(&row("Total under 1 MB", sum(&rows[..2])));
    out.push_str("\r\n");
    if let Some((total, free)) = memory.ems {
        out.push_str(&kb_line("Total Expanded (EMS)", total));
        out.push_str(&kb_line("Free Expanded (EMS)*", free));
        out.push_str("\r\n");
        for line in EMS_NOTE {
            out.push_str(&format!("{}\r\n", line));
        }
        out.push_str("\r\n");
    }
    out.push_str(&kb_line("Largest executable program size", memory.largest_conventional));
    out.push_str(&kb_line("Largest free upper memory block", memory.largest_upper));
    out.push_str(&format!("{}\r\n\r\n", memory.hma()));
    out
}

/// The programs by name, in the order their first blocks are in, MSDOS
/// first, each with its bytes in conventional and in upper memory.
fn modules(memory: &Memory) -> Vec<(&str, u32, u32)> {
    let mut modules: Vec<(&str, u32, u32)> = Vec::new();
    for block in memory.blocks.iter().filter(|b| b.kind != Kind::Free) {
        let i = match modules.iter().position(|&(name, _, _)| name == block.name) {
            Some(i) => i,
            None => {
                modules.push((&block.name, 0, 0));
                modules.len() - 1
            }
        };
        match block.region {
            Region::Conventional => modules[i].1 += block.bytes,
            Region::Upper => modules[i].2 += block.bytes,
        }
    }
    modules
}

/// MEM /C: the programs in memory below 1 MB, then the summary in bytes.
fn classify(memory: &Memory) -> String {
    let mut out = String::from("\r\nModules using memory below 1 MB:\r\n\r\n");
    out.push_str("  Name           Total       =   Conventional   +   Upper Memory\r\n");
    out.push_str("  --------  ----------------   ----------------   ----------------\r\n");
    let row = |name: &str, low: u32, high: u32| {
        format!("  {:<8}  {}   {}   {}\r\n", name, bytes_k(low + high), bytes_k(low), bytes_k(high))
    };
    for (name, low, high) in modules(memory) {
        out.push_str(&row(name, low, high));
    }
    out.push_str(&row("Free", memory.free(Region::Conventional), memory.free(Region::Upper)));
    out.push_str("\r\nMemory Summary:\r\n\r\n");
    out.push_str(&bytes_summary(memory));
    out.push_str("\r\n");
    out
}

/// The summary in bytes, which /C and /D end with.
fn bytes_summary(memory: &Memory) -> String {
    let mut out = String::from("  Type of Memory       Total   =    Used    +    Free\r\n");
    let rule = "  ----------------  ----------   ----------   ----------\r\n";
    out.push_str(rule);
    let row = |label: &str, (total, free): (u32, u32)| {
        format!("  {:<16}  {:>10}   {:>10}   {:>10}\r\n", label, n(total), n(total - free), n(free))
    };
    let rows = memory.rows();
    let sum = |rows: &[(&str, u32, u32)]| rows.iter().fold((0, 0), |(t, f), &(_, total, free)| (t + total, f + free));
    for (label, total, free) in rows {
        out.push_str(&row(label, (total, free)));
    }
    out.push_str(rule);
    out.push_str(&row("Total memory", sum(&rows)));
    out.push_str("\r\n");
    out.push_str(&row("Total under 1 MB", sum(&rows[..2])));
    out.push_str("\r\n");
    if let Some((total, free)) = memory.ems {
        out.push_str(&bytes_line("Total Expanded (EMS)", total));
        out.push_str(&bytes_line("Free Expanded (EMS)*", free));
        out.push_str("\r\n");
        for line in EMS_NOTE {
            out.push_str(&format!("  {}\r\n", line));
        }
        out.push_str("\r\n");
    }
    out.push_str(&bytes_line("Largest executable program size", memory.largest_conventional));
    out.push_str(&bytes_line("Largest free upper memory block", memory.largest_upper));
    out.push_str(&format!("  {}\r\n", memory.hma()));
    out
}

/// MEM /F: the free blocks of conventional memory, and upper memory's.
fn free(memory: &Memory) -> String {
    let mut out = String::from("\r\nFree Conventional Memory:\r\n\r\n");
    out.push_str("  Segment         Total\r\n");
    out.push_str("  -------   -----------------\r\n");
    for block in memory.free_blocks(Region::Conventional) {
        out.push_str(&format!("   {:05X}{:>13}{:>8}\r\n", block.segment, n(block.bytes), paren_k(block.bytes)));
    }
    let total = memory.free(Region::Conventional);
    out.push_str(&format!("\r\n  Total Free:{:>8}{:>8}\r\n", n(total), paren_k(total)));
    out.push_str("\r\nFree Upper Memory:\r\n\r\n");
    if memory.upper == 0 {
        out.push_str("  No upper memory available\r\n\r\n");
        return out;
    }
    out.push_str("  Region   Largest Free     Total Free      Total Size\r\n");
    out.push_str("  ------  --------------  --------------  --------------\r\n");
    // Upper memory is one region, from D000h on.
    let column = |bytes: u32| format!("{:>7}{:>7}", n(bytes), paren_k(bytes));
    out.push_str(&format!(
        "  {:>5}   {}  {}  {}\r\n\r\n",
        1,
        column(memory.largest_upper),
        column(memory.free(Region::Upper)),
        column(memory.upper)
    ));
    out
}

/// The region column of /M and /D: 1 for upper memory's one region.
fn region(block: &Block) -> &'static str {
    match block.region {
        Region::Conventional => "",
        Region::Upper => "1",
    }
}

/// MEM /M name: the blocks of one program.
fn module(memory: &Memory, name: &str) -> String {
    let blocks: Vec<&Block> = memory.blocks.iter().filter(|b| b.kind != Kind::Free && b.name == name).collect();
    if blocks.is_empty() {
        return format!("\r\n{} is not currently in memory.\r\n\r\n", name);
    }
    let mut out = format!("\r\n{} is using the following memory:\r\n\r\n", name);
    out.push_str("  Segment  Region       Total        Type\r\n");
    out.push_str("  -------  ------  ----------------  --------\r\n");
    for block in &blocks {
        out.push_str(&format!("   {:05X}{:>8}   {}  {}\r\n", block.segment, region(block), bytes_k(block.bytes), block.kind.name()));
    }
    out.push_str("                   ----------------\r\n");
    out.push_str(&format!("  Total Size:      {}\r\n\r\n", bytes_k(blocks.iter().map(|b| b.bytes).sum())));
    out
}

/// MEM /D: every block of conventional and upper memory, the summary,
/// and the drivers' versions.
fn debug(memory: &Memory) -> String {
    // The name only for the blocks that have one.
    let name = |b: &Block| match b.kind {
        Kind::Vectors | Kind::RomArea | Kind::DosArea | Kind::Free => String::new(),
        _ => b.name.clone(),
    };
    let mut out = String::from("\r\nConventional Memory Detail:\r\n\r\n");
    out.push_str("  Segment               Total        Name         Type\r\n");
    out.push_str("  -------          ----------------  -----------  --------\r\n");
    for b in memory.blocks.iter().filter(|b| b.region == Region::Conventional) {
        out.push_str(&format!("   {:05X}           {}  {:<11}  {}\r\n", b.segment, bytes_k(b.bytes), name(b), b.kind.name()));
    }
    if memory.upper != 0 {
        out.push_str("\r\nUpper Memory Detail:\r\n\r\n");
        out.push_str("  Segment  Region       Total        Name         Type\r\n");
        out.push_str("  -------  ------  ----------------  -----------  --------\r\n");
        for b in memory.blocks.iter().filter(|b| b.region == Region::Upper) {
            out.push_str(&format!("   {:05X}{:>8}   {}  {:<11}  {}\r\n", b.segment, region(b), bytes_k(b.bytes), name(b), b.kind.name()));
        }
    }
    out.push_str("\r\nMemory Summary:\r\n\r\n");
    out.push_str(&bytes_summary(memory));
    // The versions INT 67h AH=46h and the XMS driver's function 00h give.
    out.push_str("\r\n  XMS version  3.00; driver version  3.10\r\n");
    if memory.ems.is_some() {
        out.push_str("  EMS version  4.00\r\n");
    }
    out.push_str("\r\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switches() {
        assert_eq!(parse(""), Ok(Report::Summary));
        assert_eq!(parse("/p"), Ok(Report::Summary));
        assert_eq!(parse("/c /p"), Ok(Report::Classify));
        assert_eq!(parse("/C/P"), Ok(Report::Classify));
        assert_eq!(parse("/CLASSIFY"), Ok(Report::Classify));
        assert_eq!(parse("/debug"), Ok(Report::Debug));
        assert_eq!(parse("/f"), Ok(Report::Free));
        assert_eq!(parse("/m mouse"), Ok(Report::Module("MOUSE".into())));
        assert_eq!(parse("/m:mouse /p"), Ok(Report::Module("MOUSE".into())));
        assert_eq!(parse("/?"), Ok(Report::Help));
        assert!(parse("/m").is_err());
        assert!(parse("/c /f").is_err());
        assert!(parse("/x").is_err());
        assert!(parse("junk").is_err());
    }

    #[test]
    fn columns() {
        assert_eq!(kb(511), 0);
        assert_eq!(kb(512), 1);
        assert_eq!(kb(17_181), 17);
        assert_eq!(bytes_k(17_213), "  17,213   (17K)");
        assert_eq!(bytes_k(675_616), " 675,616  (660K)");
        // As MS-DOS 6.22 has them.
        assert_eq!(kb_line("Largest executable program size", 401_392), "Largest executable program size       392K (401,392 bytes)  \r\n");
        assert_eq!(kb_line("Largest free upper memory block", 0), "Largest free upper memory block         0K       (0 bytes)  \r\n");
        assert_eq!(kb_line("Largest free upper memory block", 70_304), "Largest free upper memory block        69K  (70,304 bytes)  \r\n");
        assert_eq!(bytes_line("Largest executable program size", 628_704), "  Largest executable program size        628,704   (614K)\r\n");
        assert_eq!(bytes_line("Largest free upper memory block", 46_400), "  Largest free upper memory block         46,400    (45K)\r\n");
    }
}
