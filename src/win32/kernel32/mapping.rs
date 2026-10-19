//! File mappings: CreateFileMappingA, OpenFileMappingA, MapViewOfFile and
//! UnmapViewOfFile, as Windows 95 has them (see win32/mapping.rs). Every
//! view of a mapping is at the same address, in each process and in one
//! process however often it maps it, and views look to VirtualQuery as
//! private memory reserved with PAGE_NOACCESS. A mapping of a file has
//! the file's contents, written back to it when a view goes and with
//! FlushViewOfFile. Windows 95 doesn't check a handle's access; the
//! mapping's protection says what views may do.

use super::{
    ERROR_ACCESS_DENIED, ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER, ERROR_NOT_ENOUGH_MEMORY,
    FALSE, INVALID_HANDLE_VALUE, TRUE, set_error, string_at,
};
use crate::cpu::Cpu;
use crate::win32::mapping::{ARENA, Section};
use crate::win32::memory::{
    self, GRANULARITY, MEM_MAPPED, PAGE, PAGE_NOACCESS, PAGE_READONLY, PAGE_READWRITE,
    PAGE_WRITECOPY, SHARED, SYSTEM, protection_bits,
};
use crate::win32::object::{self, Kind};
use crate::win32::process;

/// CreateFileMapping's section attributes.
const SEC_RESERVE: u32 = 0x0400_0000;
/// MapViewOfFile's access.
const FILE_MAP_COPY: u32 = 0x01;
const FILE_MAP_WRITE: u32 = 0x02;

const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_ALREADY_EXISTS: u32 = 183;
const ERROR_INVALID_ADDRESS: u32 = 487;
const ERROR_FILE_INVALID: u32 = 1006;
const ERROR_MAPPED_ALIGNMENT: u32 = 1132;

/// The section a mapping's handle names.
fn section_of(cpu: &mut Cpu, handle: u32) -> Option<usize> {
    match object::kind_of(cpu, handle)? {
        Kind::Mapping { section } => Some(section as usize),
        _ => None,
    }
}

/// DOS's open file (its SFT entry) of a DOS handle.
fn sft(cpu: &mut Cpu, dos: u16) -> Option<u16> {
    let psp = cpu.current_psp;
    crate::dos_files::sft_of(&mut cpu.bus, psp, dos)
}

/// Write a file mapping's pages back to its file, if it may write it.
fn write_back(cpu: &mut Cpu, index: usize) {
    let Some(Some(section)) = cpu.bus.win32.sections.get(index) else {
        return;
    };
    let (Some((_, dos)), PAGE_READWRITE) = (section.file, section.protect) else {
        return;
    };
    let mut data = Vec::with_capacity(section.size as usize);
    for (i, &phys) in section.pages.iter().enumerate() {
        let len = (section.size - i as u32 * PAGE).min(PAGE);
        data.extend((0..len).map(|b| cpu.bus.read_8((phys + b) as usize)));
    }
    let Some(sft) = sft(cpu, dos) else {
        return;
    };
    let disk = &mut cpu.bus.disk;
    let Ok(position) = disk.seek_file(sft, 0, 1) else {
        return;
    };
    if disk.seek_file(sft, 0, 0).is_ok() {
        let _ = disk.write_file(sft, &data);
    }
    let _ = disk.seek_file(sft, position as i64, 0);
}

/// Let go of the mappings whose objects are gone and that no running
/// process has a view of: their pages go back to the pool, their files
/// written back.
fn sweep(cpu: &mut Cpu) {
    let running: Vec<u32> = cpu.bus.win32.processes.iter().map(|p| p.id).collect();
    for index in 0..cpu.bus.win32.sections.len() {
        let win32 = &mut cpu.bus.win32;
        let Some(section) = win32.sections[index].as_mut() else {
            continue;
        };
        section.views.retain(|(id, _)| running.contains(id));
        let alive = matches!(
            win32.objects.get(section.object).map(|o| &o.kind),
            Some(Kind::Mapping { section }) if *section as usize == index
        );
        if alive || !section.views.is_empty() {
            continue;
        }
        write_back(cpu, index);
        let section = cpu.bus.win32.sections[index].take().expect("checked");
        for phys in section.pages.into_iter().filter(|&p| p != 0) {
            memory::free_page(&mut cpu.bus, phys);
        }
        if let Some((file, _)) = section.file {
            cpu.bus.win32.objects.release(file);
        }
    }
}

/// Where a mapping of `len` bytes can go: a place in the mappings' part of
/// the shared arena that no mapping and no process has.
fn place(cpu: &mut Cpu, len: u32) -> Option<u32> {
    let win32 = &cpu.bus.win32;
    let mut at = ARENA.0;
    loop {
        let end = at.checked_add(len)?;
        if end > ARENA.1 {
            return None;
        }
        let taken = win32
            .sections
            .iter()
            .flatten()
            .map(|s| (s.base, s.end()))
            .chain(win32.processes.iter().flat_map(|p| {
                p.memory
                    .regions()
                    .iter()
                    .map(|r| (r.start, r.end()))
                    .collect::<Vec<_>>()
            }))
            .filter(|&(start, stop)| start < end && at < stop)
            .map(|(_, stop)| stop)
            .max();
        match taken {
            Some(stop) => at = stop.next_multiple_of(GRANULARITY),
            None => return Some(at),
        }
    }
}

/// The contents of the file `dos` (DOS's handle) from its start, `len`
/// bytes of it, the file made that long first when `grow` (with zeroes);
/// its size when `len` is None.
fn read_file(cpu: &mut Cpu, dos: u16, len: Option<u32>, grow: bool) -> Result<Vec<u8>, u32> {
    let sft = sft(cpu, dos).ok_or(ERROR_INVALID_HANDLE)?;
    let disk = &mut cpu.bus.disk;
    let position = disk
        .seek_file(sft, 0, 1)
        .map_err(|_| ERROR_INVALID_HANDLE)?;
    let size = disk
        .seek_file(sft, 0, 2)
        .map_err(|_| ERROR_INVALID_HANDLE)? as u32;
    let len = len.unwrap_or(size);
    if len > size {
        if !grow {
            let _ = disk.seek_file(sft, position as i64, 0);
            return Err(ERROR_ACCESS_DENIED);
        }
        let zeros = vec![0; (len - size) as usize];
        if disk.write_file(sft, &zeros).is_err() {
            let _ = disk.seek_file(sft, position as i64, 0);
            return Err(ERROR_ACCESS_DENIED);
        }
    }
    let _ = disk.seek_file(sft, 0, 0);
    let data = disk.read_file(sft, len as usize).unwrap_or_default();
    let _ = disk.seek_file(sft, position as i64, 0);
    Ok(data)
}

/// CreateFileMappingA(file, attributes, protect, size high, size low,
/// name): a mapping of the file, or with INVALID_HANDLE_VALUE of the swap
/// file, `size` bytes (a file's size with 0). A name another mapping has
/// gives that one, with ERROR_ALREADY_EXISTS.
pub(super) fn create_file_mapping_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[file, _, protect, size_high, size, name_at] = args else {
        unreachable!()
    };
    let name = match name_at {
        0 => None,
        at => Some(string_at(cpu, at)?),
    };
    sweep(cpu);
    if let Some(name) = &name
        && let Some(id) = object::objects(cpu).named(name)
    {
        if !matches!(
            object::objects(cpu).get(id).map(|o| &o.kind),
            Some(Kind::Mapping { .. })
        ) {
            set_error(cpu, ERROR_INVALID_HANDLE);
            return Some(0);
        }
        object::objects(cpu).hold(id);
        let handle = object::new_handle(cpu, id);
        set_error(cpu, ERROR_ALREADY_EXISTS);
        return Some(handle);
    }
    let access = protect & 0xFF;
    if ![PAGE_NOACCESS, PAGE_READONLY, PAGE_READWRITE, PAGE_WRITECOPY].contains(&access) {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    if size_high != 0 {
        set_error(cpu, ERROR_NOT_ENOUGH_MEMORY);
        return Some(0);
    }
    let (contents, file) = if file == INVALID_HANDLE_VALUE {
        if size == 0 {
            set_error(cpu, ERROR_INVALID_PARAMETER);
            return Some(0);
        }
        (None, None)
    } else {
        let Some(id) = object::resolve(cpu, file) else {
            set_error(cpu, ERROR_INVALID_HANDLE);
            return Some(0);
        };
        let Some(Kind::Dos { handle: dos }) = object::kind_of(cpu, file) else {
            set_error(cpu, ERROR_INVALID_HANDLE);
            return Some(0);
        };
        // What the file was opened for: 0 reading, 1 writing, 2 both.
        let mode = sft(cpu, dos)
            .and_then(|sft| cpu.bus.disk.open_mode(sft))
            .map_or(0, |m| m & 3);
        let allowed = match access {
            PAGE_READWRITE => mode == 2,
            _ => mode != 1,
        };
        if !allowed {
            set_error(cpu, ERROR_ACCESS_DENIED);
            return Some(0);
        }
        let len = (size != 0).then_some(size);
        let data = match read_file(cpu, dos, len, access == PAGE_READWRITE) {
            Ok(data) => data,
            Err(error) => {
                set_error(cpu, error);
                return Some(0);
            }
        };
        if data.is_empty() {
            set_error(cpu, ERROR_FILE_INVALID);
            return Some(0);
        }
        (Some(data), Some((id, dos)))
    };
    let size = contents.as_ref().map_or(size, |c| c.len() as u32);
    let count = size.div_ceil(PAGE);
    let reserve = protect & SEC_RESERVE != 0 && contents.is_none();
    let Some(base) = place(cpu, count * PAGE) else {
        set_error(cpu, ERROR_NOT_ENOUGH_MEMORY);
        return Some(0);
    };
    let mut pages = Vec::with_capacity(count as usize);
    if !reserve {
        for i in 0..count {
            let Some(phys) = memory::pool_page(&mut cpu.bus) else {
                for phys in pages {
                    memory::free_page(&mut cpu.bus, phys);
                }
                set_error(cpu, ERROR_NOT_ENOUGH_MEMORY);
                return Some(0);
            };
            if let Some(data) = &contents {
                let from = (i * PAGE) as usize;
                let to = (from + PAGE as usize).min(data.len());
                cpu.bus.load_bytes(phys as usize, &data[from..to]);
            }
            pages.push(phys);
        }
    } else {
        pages.resize(count as usize, 0);
    }
    if let Some((id, _)) = file {
        object::objects(cpu).hold(id);
    }
    let win32 = &mut cpu.bus.win32;
    let index = match win32.sections.iter().position(Option::is_none) {
        Some(i) => i,
        None => {
            win32.sections.push(None);
            win32.sections.len() - 1
        }
    };
    let object = win32.objects.add(
        Kind::Mapping {
            section: index as u32,
        },
        name,
    );
    win32.sections[index] = Some(Section {
        object,
        base,
        size,
        protect: access,
        reserve,
        pages,
        file,
        views: Vec::new(),
    });
    let handle = object::new_handle(cpu, object);
    set_error(cpu, 0);
    Some(handle)
}

/// OpenFileMappingA(access, inherit, name): a handle of the mapping named
/// `name`.
pub(super) fn open_file_mapping_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[2] == 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    let name = string_at(cpu, args[2])?;
    sweep(cpu);
    let objects = object::objects(cpu);
    match objects.named(&name) {
        Some(id) if matches!(objects.get(id).map(|o| &o.kind), Some(Kind::Mapping { .. })) => {
            objects.hold(id);
            Some(object::new_handle(cpu, id))
        }
        _ => {
            set_error(cpu, ERROR_FILE_NOT_FOUND);
            Some(0)
        }
    }
}

/// Map the pages of the section `index` into the running process, those
/// committed with `protect`, unless it has them already; with a view
/// that may write, the pages it has read only become writable.
fn map_into(cpu: &mut Cpu, index: usize, protect: u32) -> bool {
    let section = cpu.bus.win32.sections[index].clone().expect("a section");
    crate::win32::with_process(cpu, |p, bus| {
        let region = p.memory.region(section.base).cloned();
        match region {
            Some(r) if r.start == section.base && r.kind == MEM_MAPPED => {}
            Some(_) => return false,
            None => {
                let len = section.pages.len() as u32 * PAGE;
                let reserved = p.memory.reserve_region(
                    Some(section.base),
                    len,
                    SHARED,
                    SYSTEM,
                    PAGE_NOACCESS,
                    MEM_MAPPED,
                );
                if reserved.is_none() {
                    return false;
                }
            }
        }
        for (i, &phys) in section.pages.iter().enumerate() {
            let lin = section.base + i as u32 * PAGE;
            let entry = p.memory.entry(bus, lin).unwrap_or(0);
            if phys == 0 || (entry != 0 && protect != PAGE_READWRITE) {
                continue;
            }
            p.memory
                .set_entry(bus, lin, phys | protection_bits(protect));
        }
        true
    })
}

/// MapViewOfFile(mapping, access, offset high, offset low, size): a view
/// of the mapping from the offset (a multiple of 64 KB), to its end with a
/// size of 0. FILE_MAP_WRITE needs a mapping with PAGE_READWRITE, and
/// FILE_MAP_COPY one with PAGE_WRITECOPY.
pub(super) fn map_view_of_file(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, access, high, low, size] = args else {
        unreachable!()
    };
    map_view(cpu, handle, access, high, low, size, 0)
}

/// MapViewOfFileEx(..., address): the same, where the view goes, which on
/// Windows 95 can only be the mapping's own place.
pub(super) fn map_view_of_file_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, access, high, low, size, at] = args else {
        unreachable!()
    };
    map_view(cpu, handle, access, high, low, size, at)
}

fn map_view(
    cpu: &mut Cpu,
    handle: u32,
    access: u32,
    high: u32,
    offset: u32,
    size: u32,
    at: u32,
) -> Option<u32> {
    sweep(cpu);
    let Some(index) = section_of(cpu, handle) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    };
    let section = cpu.bus.win32.sections[index].clone().expect("a section");
    let copy = access == FILE_MAP_COPY;
    let write = !copy && access & FILE_MAP_WRITE != 0;
    let error = if (copy && section.protect != PAGE_WRITECOPY)
        || (write && section.protect != PAGE_READWRITE)
        || high != 0
        || offset >= section.size
        || size > section.size - offset
    {
        Some(ERROR_INVALID_PARAMETER)
    } else if !offset.is_multiple_of(GRANULARITY) {
        Some(ERROR_MAPPED_ALIGNMENT)
    } else if at != 0 && at != section.base + offset {
        Some(ERROR_INVALID_ADDRESS)
    } else {
        None
    };
    if let Some(error) = error {
        set_error(cpu, error);
        return Some(0);
    }
    let protect = match section.protect {
        PAGE_NOACCESS => PAGE_NOACCESS,
        _ if write || copy => PAGE_READWRITE,
        _ => PAGE_READONLY,
    };
    if !map_into(cpu, index, protect) {
        set_error(cpu, ERROR_NOT_ENOUGH_MEMORY);
        return Some(0);
    }
    let address = section.base + offset;
    let id = process(cpu).id;
    if let Some(section) = cpu.bus.win32.sections[index].as_mut() {
        section.views.push((id, address));
    }
    cpu.tlb.flush();
    Some(address)
}

/// The section with a view of the running process at `at`.
fn view_at(cpu: &mut Cpu, at: u32) -> Option<usize> {
    let id = process(cpu).id;
    cpu.bus
        .win32
        .sections
        .iter()
        .position(|s| s.as_ref().is_some_and(|s| s.views.contains(&(id, at))))
}

/// UnmapViewOfFile(address): the view there goes, written back to its
/// file; the mapping's pages leave the process with its last view.
pub(super) fn unmap_view_of_file(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    let Some(index) = view_at(cpu, at) else {
        set_error(cpu, ERROR_INVALID_ADDRESS);
        return Some(FALSE);
    };
    let id = process(cpu).id;
    let section = cpu.bus.win32.sections[index].as_mut().expect("found");
    let i = section
        .views
        .iter()
        .position(|&v| v == (id, at))
        .expect("found");
    section.views.remove(i);
    let last = !section.views.iter().any(|&(p, _)| p == id);
    let (base, len) = (section.base, section.pages.len() as u32 * PAGE);
    write_back(cpu, index);
    if last {
        crate::win32::with_process(cpu, |p, bus| {
            for page in (base..base + len).step_by(PAGE as usize) {
                p.memory.unmap(bus, page);
            }
            p.memory.unreserve(base);
        });
        cpu.tlb.flush();
    }
    sweep(cpu);
    Some(TRUE)
}

/// FlushViewOfFile(address, size): write the mapping's pages to its file.
pub(super) fn flush_view_of_file(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    let id = process(cpu).id;
    let found = cpu.bus.win32.sections.iter().position(|s| {
        s.as_ref().is_some_and(|s| {
            (s.base..s.end()).contains(&at) && s.views.iter().any(|&(p, _)| p == id)
        })
    });
    match found {
        Some(index) => {
            write_back(cpu, index);
            Some(TRUE)
        }
        None => {
            set_error(cpu, ERROR_INVALID_ADDRESS);
            Some(FALSE)
        }
    }
}

/// VirtualAlloc's MEM_COMMIT of the pages from `first` to `end` of a view
/// of a SEC_RESERVE mapping: the mapping gets pages there, which every
/// process with a view of it sees, the running one with `protect`.
pub(super) fn commit(cpu: &mut Cpu, first: u32, end: u32, protect: u32) -> Result<u32, u32> {
    let id = process(cpu).id;
    let index = cpu
        .bus
        .win32
        .sections
        .iter()
        .position(|s| {
            s.as_ref().is_some_and(|s| {
                s.base <= first && end <= s.end() && s.views.iter().any(|&(p, _)| p == id)
            })
        })
        .ok_or(ERROR_INVALID_ADDRESS)?;
    let base = cpu.bus.win32.sections[index].as_ref().expect("found").base;
    let mut new = Vec::new();
    for page in (first..end).step_by(PAGE as usize) {
        let i = ((page - base) / PAGE) as usize;
        if cpu.bus.win32.sections[index].as_ref().expect("found").pages[i] != 0 {
            continue;
        }
        let Some(phys) = memory::pool_page(&mut cpu.bus) else {
            for (i, phys) in new {
                cpu.bus.win32.sections[index].as_mut().expect("found").pages[i] = 0;
                memory::free_page(&mut cpu.bus, phys);
            }
            return Err(ERROR_NOT_ENOUGH_MEMORY);
        };
        cpu.bus.win32.sections[index].as_mut().expect("found").pages[i] = phys;
        new.push((i, phys));
    }
    let section = cpu.bus.win32.sections[index].clone().expect("found");
    let others = match section.protect {
        PAGE_NOACCESS => PAGE_NOACCESS,
        PAGE_READWRITE | PAGE_WRITECOPY => PAGE_READWRITE,
        _ => PAGE_READONLY,
    };
    let running = cpu.bus.win32.running;
    for p in 0..cpu.bus.win32.processes.len() {
        let pid = cpu.bus.win32.processes[p].id;
        if !section.views.iter().any(|&(v, _)| v == pid) {
            continue;
        }
        let protect = if p == running { protect } else { others };
        let mut process = std::mem::take(&mut cpu.bus.win32.processes[p]);
        for page in (first..end).step_by(PAGE as usize) {
            let i = ((page - base) / PAGE) as usize;
            let entry = process.memory.entry(&cpu.bus, page).unwrap_or(0);
            if entry == 0 || p == running {
                let phys = section.pages[i];
                process
                    .memory
                    .set_entry(&mut cpu.bus, page, phys | protection_bits(protect));
            }
        }
        cpu.bus.win32.processes[p] = process;
    }
    cpu.tlb.flush();
    Ok(first)
}
