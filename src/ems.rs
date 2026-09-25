//! Expanded memory (LIM EMS 4.0, INT 67h), as EMM386 provides it: 16 KB
//! logical pages in handles, mapped four at a time into the page frame at
//! E000h. Programs find the driver by the name "EMMXXXX0" in the device
//! header at offset 0Ah of INT 67h's segment, or by opening that device.
//!
//! The logical pages live in extended memory, taken from the same pool as
//! XMS (see xms.rs). Mapping a page copies it into the frame, and mapping
//! another over it copies it back: instruction fetch and DMA read RAM
//! directly, so the frame has to hold the page itself. The one thing this
//! doesn't do as the hardware does is alias: a logical page mapped into two
//! physical pages at once is two copies, and the one mapped last wins.
//!
//! VCPI isn't there (AX=DE00h says so), as the CPU runs DOS in real mode,
//! not virtual-8086 mode: DOS extenders then use XMS or raw mode.

use crate::bus::Bus;
use crate::cpu::Cpu;
use iced_x86::Register;
use std::collections::HashMap;

/// The page frame's segment, and its four physical pages.
pub const FRAME_SEGMENT: u16 = 0xE000;
const FRAME: usize = (FRAME_SEGMENT as usize) << 4;
pub const PAGE: usize = 0x4000;
pub const FRAME_PAGES: usize = 4;
/// The driver's device name, which programs look for.
pub const DEVICE_NAME: &[u8; 8] = b"EMMXXXX0";
/// Where the device header is, in the BIOS ROM at F000:0000, so the name
/// is at F000:000A, INT 67h's segment.
const DEVICE_HEADER: usize = 0xF0000;
/// A RETF for the device's strategy and interrupt entries, after its header.
const DEVICE_RETF: u16 = 0x0012;
/// Handles, the system's handle 0 among them.
const MAX_HANDLES: usize = 255;
/// The size of a page map (functions 4Eh): a handle and a logical page for
/// each physical page.
const MAP_SIZE: u8 = (FRAME_PAGES * 4) as u8;

// Status codes, returned in AH.
const OK: u8 = 0x00;
const INVALID_HANDLE: u8 = 0x83;
const UNDEFINED_FUNCTION: u8 = 0x84;
const NO_HANDLES: u8 = 0x85;
const MAP_CONTEXT: u8 = 0x86;
const TOO_MANY_PAGES: u8 = 0x87;
const NOT_ENOUGH_PAGES: u8 = 0x88;
const ZERO_PAGES: u8 = 0x89;
const LOGICAL_PAGE: u8 = 0x8A;
const PHYSICAL_PAGE: u8 = 0x8B;
const MAP_SAVED: u8 = 0x8D;
const MAP_NOT_SAVED: u8 = 0x8E;
const SUBFUNCTION: u8 = 0x8F;
const NON_VOLATILE: u8 = 0x91;
const MOVE_OVERLAP: u8 = 0x92;
const REGION_TOO_LONG: u8 = 0x93;
const PAGE_OFFSET: u8 = 0x95;
const REGION_OVER_1MB: u8 = 0x96;
const EXCHANGE_OVERLAP: u8 = 0x97;
const MEMORY_TYPE: u8 = 0x98;
const NAME_NOT_FOUND: u8 = 0xA0;
const NAME_EXISTS: u8 = 0xA1;
const ADDRESS_WRAP: u8 = 0xA2;

/// A handle's logical pages, by their addresses in extended memory, and
/// its name (function 53h).
#[derive(Clone, Debug, Default)]
struct Handle {
    pages: Vec<u32>,
    name: [u8; 8],
}

/// What each physical page shows: a handle's logical page, or nothing.
type Map = [Option<(u16, u16)>; FRAME_PAGES];

/// The expanded memory manager.
#[derive(Clone, Debug)]
pub struct Ems {
    /// Handles by number; handle 0, the system's, is always there.
    handles: Vec<Option<Handle>>,
    mapped: Map,
    /// The maps saved with function 47h, by the handle they were saved for.
    saved: HashMap<u16, Map>,
}

impl Default for Ems {
    fn default() -> Self {
        Self::new()
    }
}

impl Ems {
    pub fn new() -> Self {
        Self { handles: vec![Some(Handle::default())], mapped: [None; FRAME_PAGES], saved: HashMap::new() }
    }

    /// The open handles: number, logical pages and name, for debuggers.
    pub fn handles(&self) -> Vec<(u16, usize, String)> {
        self.handles
            .iter()
            .enumerate()
            .filter_map(|(i, h)| {
                h.as_ref().map(|h| (i as u16, h.pages.len(), String::from_utf8_lossy(&h.name).trim_end_matches('\0').to_string()))
            })
            .collect()
    }

    fn handle(&self, handle: u16) -> Option<&Handle> {
        self.handles.get(handle as usize)?.as_ref()
    }

    fn handle_mut(&mut self, handle: u16) -> Option<&mut Handle> {
        self.handles.get_mut(handle as usize)?.as_mut()
    }

    fn open_handles(&self) -> usize {
        self.handles.iter().flatten().count()
    }

    /// Where logical page `page` of `handle` is now: in the page frame
    /// while mapped, else in extended memory.
    fn live_addr(&self, handle: u16, page: u16) -> Option<usize> {
        if let Some(slot) = self.mapped.iter().position(|&m| m == Some((handle, page))) {
            return Some(FRAME + slot * PAGE);
        }
        self.handle(handle)?.pages.get(page as usize).map(|&addr| addr as usize)
    }

    /// Copy what physical page `slot` shows back to its logical page.
    fn write_back(&self, bus: &mut Bus, slot: usize) {
        if let Some((handle, page)) = self.mapped[slot]
            && let Some(&addr) = self.handle(handle).and_then(|h| h.pages.get(page as usize))
        {
            bus.copy_ram(FRAME + slot * PAGE, addr as usize, PAGE);
        }
    }

    /// Show `target` in physical page `slot`, or nothing: the page shown
    /// goes back to extended memory, and the new one comes into the frame.
    fn map(&mut self, bus: &mut Bus, slot: usize, target: Option<(u16, u16)>) {
        if self.mapped[slot] == target {
            return;
        }
        self.write_back(bus, slot);
        self.mapped[slot] = None;
        if let Some((handle, page)) = target {
            // Shown in another physical page too: its latest contents.
            for other in 0..FRAME_PAGES {
                if self.mapped[other] == target {
                    self.write_back(bus, other);
                }
            }
            if let Some(&addr) = self.handle(handle).and_then(|h| h.pages.get(page as usize)) {
                bus.copy_ram(addr as usize, FRAME + slot * PAGE, PAGE);
            }
        }
        self.mapped[slot] = target;
    }

    /// Show the pages of `map`.
    fn restore(&mut self, bus: &mut Bus, map: Map) {
        for (slot, &target) in map.iter().enumerate() {
            self.map(bus, slot, target);
        }
    }

    /// Forget the physical pages showing `handle`'s pages from `from` on,
    /// without copying them back: they are being freed.
    fn drop_mapped(&mut self, handle: u16, from: u16) {
        for entry in self.mapped.iter_mut() {
            if entry.is_some_and(|(h, p)| h == handle && p >= from) {
                *entry = None;
            }
        }
    }
}

/// The free and total logical pages, as extended memory has room for them.
fn page_counts(bus: &Bus) -> (u16, u16) {
    let end = bus.ram().len() as u32;
    (bus.xms.free_pages(end).min(0xFFFF) as u16, bus.xms.total_pages(end).min(0xFFFF) as u16)
}

/// Take `count` logical pages from extended memory.
fn take_pages(bus: &mut Bus, count: usize) -> Result<Vec<u32>, u8> {
    let (free, total) = page_counts(bus);
    if count > total as usize {
        return Err(TOO_MANY_PAGES);
    }
    if count > free as usize {
        return Err(NOT_ENOUGH_PAGES);
    }
    let end = bus.ram().len() as u32;
    let pages: Vec<u32> = (0..count).filter_map(|_| bus.xms.take_page(end)).collect();
    if pages.len() < count {
        for &page in &pages {
            bus.xms.release_page(page);
        }
        return Err(NOT_ENOUGH_PAGES);
    }
    // New pages hold zeros, not what an earlier program left.
    for &page in &pages {
        bus.fill_ram(page as usize..page as usize + PAGE, 0);
    }
    Ok(pages)
}

/// Make EMS there or not, from the next INT 67h call on, with the device
/// programs look for.
pub fn set_enabled(bus: &mut Bus, on: bool) {
    if on == bus.ems.is_some() {
        return;
    }
    if on {
        bus.ems = Some(Ems::new());
    } else {
        free_all(bus);
        bus.ems = None;
    }
    bus.disk.emm_device = on;
    bus.sync_drive_bda();
}

/// Free every handle's pages, as when no program runs any more.
pub fn free_all(bus: &mut Bus) {
    let Some(ems) = bus.ems.take() else { return };
    for handle in ems.handles.iter().flatten() {
        for &page in &handle.pages {
            bus.xms.release_page(page);
        }
    }
    bus.ems = Some(Ems::new());
}

/// Put the driver's device header in the ROM, and chain it after NUL (at
/// `nul`) while EMS is there. Without EMS, the name is gone, so the
/// vector check finds no driver.
pub fn install_device(bus: &mut Bus, nul: usize) {
    let header = DEVICE_HEADER;
    bus.write_16(header + 0x04, 0xC000); // character device, IOCTL
    bus.write_16(header + 0x06, DEVICE_RETF);
    bus.write_16(header + 0x08, DEVICE_RETF);
    bus.write_8(DEVICE_HEADER + DEVICE_RETF as usize, 0xCB);
    let name: &[u8; 8] = if bus.ems.is_some() { DEVICE_NAME } else { &[0; 8] };
    for (i, &b) in name.iter().enumerate() {
        bus.write_8(header + 0x0A + i, b);
    }
    if bus.ems.is_some() {
        // In front of whatever NUL led to.
        let next = bus.read_32(nul);
        bus.write_32(header, next);
        bus.write_16(nul, (header - 0xF0000) as u16);
        bus.write_16(nul + 2, 0xF000);
    } else {
        bus.write_32(header, 0xFFFF_FFFF);
    }
}

/// INT 67h: the function in AH, the status back in AH.
pub fn handle(cpu: &mut Cpu) {
    let function = cpu.get_ah();
    let Some(mut ems) = cpu.bus.ems.take() else {
        cpu.set_reg8(Register::AH, UNDEFINED_FUNCTION);
        return;
    };
    let status = call(cpu, &mut ems, function);
    cpu.bus.ems = Some(ems);
    if status != OK {
        cpu.bus.log_string(&format!("[EMS] AX={:04X}: status {:02X}", cpu.ax(), status));
    }
    cpu.set_reg8(Register::AH, status);
}

fn call(cpu: &mut Cpu, ems: &mut Ems, function: u8) -> u8 {
    let al = cpu.get_al();
    let dx = cpu.dx();
    match function {
        // Get status.
        0x40 => OK,
        // The page frame's segment.
        0x41 => {
            cpu.set_bx(FRAME_SEGMENT);
            OK
        }
        // Free and total logical pages.
        0x42 => {
            let (free, total) = page_counts(&cpu.bus);
            cpu.set_bx(free);
            cpu.set_dx(total);
            OK
        }
        // Allocate BX pages (at least one) to a new handle in DX.
        0x43 if cpu.bx() == 0 => ZERO_PAGES,
        0x43 => allocate(cpu, ems),
        // Map logical page BX (FFFFh: unmap) of handle DX into physical
        // page AL.
        0x44 => {
            let target = match check_page(ems, dx, cpu.bx()) {
                Ok(target) => target,
                Err(e) => return e,
            };
            if al as usize >= FRAME_PAGES {
                return PHYSICAL_PAGE;
            }
            ems.map(&mut cpu.bus, al as usize, target);
            OK
        }
        // Deallocate handle DX and its pages.
        0x45 => {
            if ems.handle(dx).is_none() {
                return INVALID_HANDLE;
            }
            if ems.saved.contains_key(&dx) {
                return MAP_CONTEXT;
            }
            ems.drop_mapped(dx, 0);
            let pages = std::mem::take(&mut ems.handle_mut(dx).unwrap().pages);
            for page in pages {
                cpu.bus.xms.release_page(page);
            }
            // The system handle stays, without pages.
            if dx != 0 {
                ems.handles[dx as usize] = None;
            }
            OK
        }
        // Version 4.0.
        0x46 => {
            cpu.set_reg8(Register::AL, 0x40);
            OK
        }
        // Save and restore the page map for handle DX (an interrupt
        // handler's).
        0x47 => {
            if ems.handle(dx).is_none() {
                return INVALID_HANDLE;
            }
            if ems.saved.contains_key(&dx) {
                return MAP_SAVED;
            }
            ems.saved.insert(dx, ems.mapped);
            OK
        }
        0x48 => {
            if ems.handle(dx).is_none() {
                return INVALID_HANDLE;
            }
            match ems.saved.remove(&dx) {
                Some(map) => {
                    ems.restore(&mut cpu.bus, map);
                    OK
                }
                None => MAP_NOT_SAVED,
            }
        }
        // Open handles.
        0x4B => {
            cpu.set_bx(ems.open_handles() as u16);
            OK
        }
        // Pages of handle DX.
        0x4C => match ems.handle(dx) {
            Some(h) => {
                cpu.set_bx(h.pages.len() as u16);
                OK
            }
            None => INVALID_HANDLE,
        },
        // Each open handle and its pages, at ES:DI.
        0x4D => {
            let mut at = es_di(cpu);
            let list: Vec<(u16, u16)> = ems
                .handles
                .iter()
                .enumerate()
                .filter_map(|(i, h)| h.as_ref().map(|h| (i as u16, h.pages.len() as u16)))
                .collect();
            for &(handle, pages) in &list {
                cpu.bus.write_16(at, handle);
                cpu.bus.write_16(at + 2, pages);
                at += 4;
            }
            cpu.set_bx(list.len() as u16);
            OK
        }
        0x4E => page_map(cpu, ems, al),
        0x4F => partial_page_map(cpu, ems, al),
        0x50 => map_multiple(cpu, ems, al),
        // Reallocate handle DX to BX pages.
        0x51 => reallocate(cpu, ems),
        // Handle attributes: every handle is volatile.
        0x52 => match al {
            0x00 if ems.handle(dx).is_none() => INVALID_HANDLE,
            0x00 | 0x02 => {
                cpu.set_reg8(Register::AL, 0);
                OK
            }
            0x01 if ems.handle(dx).is_none() => INVALID_HANDLE,
            0x01 if cpu.get_reg8(Register::BL) == 0 => OK,
            0x01 => NON_VOLATILE,
            _ => SUBFUNCTION,
        },
        0x53 => handle_name(cpu, ems, al),
        0x54 => handle_directory(cpu, ems, al),
        0x57 => move_or_exchange(cpu, ems, al),
        // The physical pages' segments.
        0x58 => match al {
            0x00 => {
                let mut at = es_di(cpu);
                for slot in 0..FRAME_PAGES {
                    cpu.bus.write_16(at, FRAME_SEGMENT + (slot * PAGE >> 4) as u16);
                    cpu.bus.write_16(at + 2, slot as u16);
                    at += 4;
                }
                cpu.set_cx(FRAME_PAGES as u16);
                OK
            }
            0x01 => {
                cpu.set_cx(FRAME_PAGES as u16);
                OK
            }
            _ => SUBFUNCTION,
        },
        // Hardware information: 16 KB raw pages, no alternate map
        // registers, no DMA register sets.
        0x59 => match al {
            0x00 => {
                let at = es_di(cpu);
                for (i, word) in [(PAGE >> 4) as u16, 0, MAP_SIZE as u16, 0, 0].into_iter().enumerate() {
                    cpu.bus.write_16(at + 2 * i, word);
                }
                OK
            }
            0x01 => {
                let (free, total) = page_counts(&cpu.bus);
                cpu.set_bx(free);
                cpu.set_dx(total);
                OK
            }
            _ => SUBFUNCTION,
        },
        // Allocate standard or raw pages: as 43h, but zero pages will do.
        0x5A if al <= 1 => allocate(cpu, ems),
        0x5A => SUBFUNCTION,
        // VCPI and everything else: not there.
        _ => UNDEFINED_FUNCTION,
    }
}

/// ES:DI as a physical address.
fn es_di(cpu: &Cpu) -> usize {
    cpu.get_physical_addr(cpu.es(), cpu.di())
}

fn ds_si(cpu: &Cpu) -> usize {
    cpu.get_physical_addr(cpu.ds(), cpu.si())
}

/// Logical page `page` of `handle` to map, or None for FFFFh (unmap).
fn check_page(ems: &Ems, handle: u16, page: u16) -> Result<Option<(u16, u16)>, u8> {
    let h = ems.handle(handle).ok_or(INVALID_HANDLE)?;
    if page == 0xFFFF {
        return Ok(None);
    }
    if page as usize >= h.pages.len() {
        return Err(LOGICAL_PAGE);
    }
    Ok(Some((handle, page)))
}

/// Functions 43h and 5Ah: a new handle with BX pages, in DX.
fn allocate(cpu: &mut Cpu, ems: &mut Ems) -> u8 {
    let slot = match ems.handles.iter().skip(1).position(Option::is_none) {
        Some(i) => i + 1,
        None if ems.handles.len() < MAX_HANDLES => ems.handles.len(),
        None => return NO_HANDLES,
    };
    let count = cpu.bx() as usize;
    let pages = match take_pages(&mut cpu.bus, count) {
        Ok(pages) => pages,
        Err(e) => return e,
    };
    let handle = Handle { pages, name: [0; 8] };
    if slot == ems.handles.len() {
        ems.handles.push(Some(handle));
    } else {
        ems.handles[slot] = Some(handle);
    }
    cpu.set_dx(slot as u16);
    OK
}

/// Function 51h: handle DX gets BX pages, keeping the first ones.
fn reallocate(cpu: &mut Cpu, ems: &mut Ems) -> u8 {
    let (handle, count) = (cpu.dx(), cpu.bx() as usize);
    let Some(have) = ems.handle(handle).map(|h| h.pages.len()) else {
        return INVALID_HANDLE;
    };
    if count > have {
        match take_pages(&mut cpu.bus, count - have) {
            Ok(more) => ems.handle_mut(handle).unwrap().pages.extend(more),
            Err(e) => {
                cpu.set_bx(have as u16);
                return e;
            }
        }
    } else {
        ems.drop_mapped(handle, count as u16);
        let freed = ems.handle_mut(handle).unwrap().pages.split_off(count);
        for page in freed {
            cpu.bus.xms.release_page(page);
        }
    }
    cpu.set_bx(count as u16);
    OK
}

/// A page map in memory: for each physical page, its handle and logical
/// page, FFFFh for none.
fn write_map(bus: &mut Bus, at: usize, map: &Map) {
    for (slot, entry) in map.iter().enumerate() {
        let (handle, page) = entry.unwrap_or((0xFFFF, 0xFFFF));
        bus.write_16(at + slot * 4, handle);
        bus.write_16(at + slot * 4 + 2, page);
    }
}

fn read_map(bus: &Bus, ems: &Ems, at: usize) -> Map {
    std::array::from_fn(|slot| {
        let handle = bus.read_16(at + slot * 4);
        let page = bus.read_16(at + slot * 4 + 2);
        ems.handle(handle).filter(|h| (page as usize) < h.pages.len()).map(|_| (handle, page))
    })
}

/// Function 4Eh: get the page map to ES:DI, set it from DS:SI, both, or
/// its size.
fn page_map(cpu: &mut Cpu, ems: &mut Ems, al: u8) -> u8 {
    match al {
        0x00 => {
            let at = es_di(cpu);
            write_map(&mut cpu.bus, at, &ems.mapped);
        }
        0x01 => {
            let map = read_map(&cpu.bus, ems, ds_si(cpu));
            ems.restore(&mut cpu.bus, map);
        }
        0x02 => {
            let at = es_di(cpu);
            write_map(&mut cpu.bus, at, &ems.mapped);
            let map = read_map(&cpu.bus, ems, ds_si(cpu));
            ems.restore(&mut cpu.bus, map);
        }
        0x03 => cpu.set_reg8(Register::AL, MAP_SIZE),
        _ => return SUBFUNCTION,
    }
    OK
}

/// The physical page at `segment`, one of the frame's four.
fn slot_of_segment(segment: u16) -> Option<usize> {
    let offset = segment.wrapping_sub(FRAME_SEGMENT) as usize;
    (segment >= FRAME_SEGMENT && offset % (PAGE >> 4) == 0 && offset / (PAGE >> 4) < FRAME_PAGES)
        .then_some(offset / (PAGE >> 4))
}

/// Function 4Fh: save the map of the physical pages whose segments DS:SI
/// lists (a count, then the segments) to ES:DI, restore such a saved map
/// from DS:SI, or give the size of one for BX pages.
fn partial_page_map(cpu: &mut Cpu, ems: &mut Ems, al: u8) -> u8 {
    match al {
        0x00 => {
            let list = ds_si(cpu);
            let count = cpu.bus.read_16(list) as usize;
            let mut slots = Vec::new();
            for i in 0..count {
                match slot_of_segment(cpu.bus.read_16(list + 2 + 2 * i)) {
                    Some(slot) => slots.push(slot),
                    None => return PHYSICAL_PAGE,
                }
            }
            let at = es_di(cpu);
            cpu.bus.write_16(at, count as u16);
            for (i, &slot) in slots.iter().enumerate() {
                let (handle, page) = ems.mapped[slot].unwrap_or((0xFFFF, 0xFFFF));
                let entry = at + 2 + 6 * i;
                cpu.bus.write_16(entry, FRAME_SEGMENT + (slot * PAGE >> 4) as u16);
                cpu.bus.write_16(entry + 2, handle);
                cpu.bus.write_16(entry + 4, page);
            }
            OK
        }
        0x01 => {
            let at = ds_si(cpu);
            let count = cpu.bus.read_16(at) as usize;
            for i in 0..count {
                let entry = at + 2 + 6 * i;
                let Some(slot) = slot_of_segment(cpu.bus.read_16(entry)) else { return PHYSICAL_PAGE };
                let (handle, page) = (cpu.bus.read_16(entry + 2), cpu.bus.read_16(entry + 4));
                let target = ems.handle(handle).filter(|h| (page as usize) < h.pages.len()).map(|_| (handle, page));
                ems.map(&mut cpu.bus, slot, target);
            }
            OK
        }
        0x02 => {
            if cpu.bx() as usize > FRAME_PAGES {
                return PHYSICAL_PAGE;
            }
            cpu.set_reg8(Register::AL, (2 + 6 * cpu.bx()) as u8);
            OK
        }
        _ => SUBFUNCTION,
    }
}

/// Function 50h: map CX logical pages of handle DX, listed at DS:SI with
/// the physical pages (AL=0) or their segments (AL=1).
fn map_multiple(cpu: &mut Cpu, ems: &mut Ems, al: u8) -> u8 {
    if al > 1 {
        return SUBFUNCTION;
    }
    let handle = cpu.dx();
    if ems.handle(handle).is_none() {
        return INVALID_HANDLE;
    }
    let list = ds_si(cpu);
    for i in 0..cpu.cx() as usize {
        let page = cpu.bus.read_16(list + 4 * i);
        let physical = cpu.bus.read_16(list + 4 * i + 2);
        let slot = if al == 0 { Some(physical as usize).filter(|&s| s < FRAME_PAGES) } else { slot_of_segment(physical) };
        let Some(slot) = slot else { return PHYSICAL_PAGE };
        let target = match check_page(ems, handle, page) {
            Ok(target) => target,
            Err(e) => return e,
        };
        ems.map(&mut cpu.bus, slot, target);
    }
    OK
}

/// Function 53h: get handle DX's name to ES:DI, or set it from DS:SI.
fn handle_name(cpu: &mut Cpu, ems: &mut Ems, al: u8) -> u8 {
    let handle = cpu.dx();
    if ems.handle(handle).is_none() {
        return INVALID_HANDLE;
    }
    match al {
        0x00 => {
            let at = es_di(cpu);
            let name = ems.handle(handle).unwrap().name;
            for (i, b) in name.into_iter().enumerate() {
                cpu.bus.write_8(at + i, b);
            }
            OK
        }
        0x01 => {
            let at = ds_si(cpu);
            let name: [u8; 8] = std::array::from_fn(|i| cpu.bus.read_8(at + i));
            let taken = name != [0; 8]
                && ems.handles.iter().enumerate().any(|(i, h)| i != handle as usize && h.as_ref().is_some_and(|h| h.name == name));
            if taken {
                return NAME_EXISTS;
            }
            ems.handle_mut(handle).unwrap().name = name;
            OK
        }
        _ => SUBFUNCTION,
    }
}

/// Function 54h: every handle with its name to ES:DI, the handle with the
/// name at DS:SI, or how many handles there can be.
fn handle_directory(cpu: &mut Cpu, ems: &mut Ems, al: u8) -> u8 {
    match al {
        0x00 => {
            let mut at = es_di(cpu);
            let mut count = 0;
            for (i, h) in ems.handles.iter().enumerate() {
                if let Some(h) = h {
                    cpu.bus.write_16(at, i as u16);
                    for (j, &b) in h.name.iter().enumerate() {
                        cpu.bus.write_8(at + 2 + j, b);
                    }
                    at += 10;
                    count += 1;
                }
            }
            cpu.set_reg8(Register::AL, count);
            OK
        }
        0x01 => {
            let at = ds_si(cpu);
            let name: [u8; 8] = std::array::from_fn(|i| cpu.bus.read_8(at + i));
            if name == [0; 8] {
                return NAME_EXISTS;
            }
            match ems.handles.iter().position(|h| h.as_ref().is_some_and(|h| h.name == name)) {
                Some(i) => {
                    cpu.set_dx(i as u16);
                    OK
                }
                None => NAME_NOT_FOUND,
            }
        }
        0x02 => {
            cpu.set_bx(MAX_HANDLES as u16);
            OK
        }
        _ => SUBFUNCTION,
    }
}

/// One end of a function 57h move: conventional memory from a physical
/// address, or a handle's pages from an offset in its first one.
enum Region {
    Conventional(usize),
    Expanded { handle: u16, page: u16, offset: usize },
}

impl Region {
    /// The address of the region's byte `i`, where it is now.
    fn addr(&self, ems: &Ems, i: usize) -> usize {
        match *self {
            Region::Conventional(base) => base + i,
            Region::Expanded { handle, page, offset } => {
                let at = offset + i;
                ems.live_addr(handle, page + (at / PAGE) as u16).unwrap_or(0) + at % PAGE
            }
        }
    }

    /// Where the region starts and ends, as (handle or None, first, last)
    /// in a space where two regions of one kind can be compared.
    fn span(&self, len: usize) -> (Option<u16>, usize, usize) {
        match *self {
            Region::Conventional(base) => (None, base, base + len),
            Region::Expanded { handle, page, offset } => {
                let start = page as usize * PAGE + offset;
                (Some(handle), start, start + len)
            }
        }
    }
}

/// Read one end of a move from the descriptor at `at`: its type byte,
/// handle, offset and segment or logical page.
fn region(cpu: &Cpu, ems: &Ems, at: usize, len: usize) -> Result<Region, u8> {
    let kind = cpu.bus.read_8(at);
    let handle = cpu.bus.read_16(at + 1);
    let offset = cpu.bus.read_16(at + 3) as usize;
    let segment = cpu.bus.read_16(at + 5);
    match kind {
        0 => {
            let base = ((segment as usize) << 4) + offset;
            if base + len > 0x10_0000 {
                return Err(ADDRESS_WRAP);
            }
            Ok(Region::Conventional(base))
        }
        1 => {
            let h = ems.handle(handle).ok_or(INVALID_HANDLE)?;
            if offset >= PAGE {
                return Err(PAGE_OFFSET);
            }
            if segment as usize >= h.pages.len() {
                return Err(LOGICAL_PAGE);
            }
            if segment as usize * PAGE + offset + len > h.pages.len() * PAGE {
                return Err(REGION_TOO_LONG);
            }
            Ok(Region::Expanded { handle, page: segment, offset })
        }
        _ => Err(MEMORY_TYPE),
    }
}

/// Function 57h: move (AL=0) or exchange (AL=1) the region the structure
/// at DS:SI describes: its length, then the source and the destination.
fn move_or_exchange(cpu: &mut Cpu, ems: &mut Ems, al: u8) -> u8 {
    if al > 1 {
        return SUBFUNCTION;
    }
    let at = ds_si(cpu);
    let len = cpu.bus.read_32(at) as usize;
    if len > 0x10_0000 {
        return REGION_OVER_1MB;
    }
    let (source, dest) = match (region(cpu, ems, at + 4, len), region(cpu, ems, at + 0x0B, len)) {
        (Ok(s), Ok(d)) => (s, d),
        (Err(e), _) | (_, Err(e)) => return e,
    };
    let (sh, s0, s1) = source.span(len);
    let (dh, d0, d1) = dest.span(len);
    let overlap = sh == dh && s0 < d1 && d0 < s1 && len > 0;
    if overlap && al == 1 {
        return EXCHANGE_OVERLAP;
    }
    // Through the bus, so video memory is a destination as well.
    let from: Vec<u8> = (0..len).map(|i| cpu.bus.read_8(source.addr(ems, i))).collect();
    if al == 1 {
        let to: Vec<u8> = (0..len).map(|i| cpu.bus.read_8(dest.addr(ems, i))).collect();
        for (i, b) in to.into_iter().enumerate() {
            cpu.bus.write_8(source.addr(ems, i), b);
        }
    }
    for (i, b) in from.into_iter().enumerate() {
        cpu.bus.write_8(dest.addr(ems, i), b);
    }
    if overlap { MOVE_OVERLAP } else { OK }
}

crate::state_fields!(Handle { pages, name });
crate::state_fields!(Ems { handles, mapped, saved });
