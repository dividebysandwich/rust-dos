//! Making a process of a program: its memory and tables, the built-in DLLs
//! it imports from, the program's image at its image base (or relocated),
//! its imports bound, its static TLS, its stack and its thread's TIB.

use super::memory::{GRANULARITY, Memory, PAGE, PRIVATE, SHARED, SYSTEM, USER, WRITABLE};
use super::stubs::{self, STUB};
use super::system::{self, SYSTEM_BLOCK_PAGES};
use super::{FIRST_MISSING, MAX_MISSING, Module, Process};
use crate::cpu::Cpu;
use rust_dos_pe as pe;

/// A process needs at least this much extended memory to start.
const MIN_MEMORY: u32 = 0x40_0000;
/// The largest stack committed when the process starts; a larger reserve
/// stays reserved, with no pages behind it.
const MAX_STACK_COMMIT: u32 = 0x10_0000;
/// Where the TLS slot array is in the TIB's page (`fs:[2Ch]` points at
/// it). Windows 95 has 80 slots.
const TLS_ARRAY: u32 = 0x100;

/// The TIB's fields.
const TIB_EXCEPTION_LIST: u32 = 0x00;
const TIB_STACK_TOP: u32 = 0x04;
const TIB_STACK_LIMIT: u32 = 0x08;
const TIB_SELF: u32 = 0x18;
const TIB_TLS_ARRAY: u32 = 0x2C;

/// What the loader made: the process, and where the program starts.
pub struct Loaded {
    pub process: Process,
    pub entry: u32,
    pub stack_top: u32,
}

/// Make the process of the program `path`, whose file is `file`. Fails
/// with a message for the user; then nothing is left taken.
pub fn load(cpu: &mut Cpu, path: &str, file: &[u8]) -> Result<Loaded, String> {
    let name = path
        .rsplit(['\\', '/', ':'])
        .next()
        .unwrap_or(path)
        .to_string();
    let mut image =
        pe::Image::load(file).map_err(|e| format!("{} can't be loaded: {}.", name, e))?;
    if image.headers.is_dll() {
        return Err(format!("{} is a DLL, not a program.", name));
    }
    let imports = image
        .imports()
        .map_err(|e| format!("{}'s imports can't be read: {}.", name, e))?;
    let mut dlls = vec![0];
    for dll in &imports {
        let index = stubs::find_dll(&dll.name).ok_or_else(|| {
            format!(
                "{} needs {}, which the built-in Windows doesn't have yet.",
                name, dll.name
            )
        })?;
        if !dlls.contains(&index) {
            dlls.push(index);
        }
    }
    let mut memory = Memory::new(&mut cpu.bus, MIN_MEMORY).ok_or_else(|| {
        format!(
            "There isn't enough extended memory to run {} (it needs 4 MB).",
            name
        )
    })?;
    let loaded = build(cpu, &mut memory, &name, path, &mut image, &imports, &dlls);
    match loaded {
        Some(Ok(mut loaded)) => {
            loaded.process.memory = memory;
            Ok(loaded)
        }
        result => {
            memory.release(&mut cpu.bus);
            Err(match result {
                Some(Err(message)) => message,
                _ => format!("There isn't enough extended memory to run {}.", name),
            })
        }
    }
}

/// `load`'s work once the memory is taken. None when it runs out.
fn build(
    cpu: &mut Cpu,
    memory: &mut Memory,
    name: &str,
    path: &str,
    image: &mut pe::Image,
    imports: &[pe::ImportedDll],
    dlls: &[usize],
) -> Option<Result<Loaded, String>> {
    let bus = &mut cpu.bus;
    let mut process = Process {
        name: name.to_string(),
        ..Process::default()
    };

    // The thread's TIB, and the system's tables.
    let tib = memory.reserve(None, GRANULARITY, SHARED, SYSTEM)?;
    memory.commit(bus, tib, PAGE, USER | WRITABLE)?;
    process.tib = tib;
    process.system = memory.pages(bus, SYSTEM_BLOCK_PAGES)?;
    system::build_tables(cpu, memory, process.system, tib)?;
    let bus = &mut cpu.bus;

    // The built-in DLLs, read only.
    let mut exports = Vec::new();
    for &index in dlls {
        let dll = stubs::DLLS[index];
        let built = stubs::image(index);
        let size = built.headers.size_of_image;
        memory.reserve(Some(dll.base), size, SHARED, SYSTEM)?;
        memory.commit(bus, dll.base, size.next_multiple_of(PAGE), USER)?;
        memory.write(bus, dll.base, &built.data)?;
        for page in (dll.base..dll.base + size).step_by(PAGE as usize) {
            process.stub_pages.push(memory.translate(bus, page)? >> 12);
        }
        process.modules.push(Module {
            name: dll.name.to_string(),
            base: dll.base,
            size,
        });
        exports.push((index, built.exports().ok().flatten().unwrap_or_default()));
    }

    // The program, where it wants to be, or where there is room when its
    // relocations let it move.
    let (preferred, size) = (image.headers.image_base, image.headers.size_of_image);
    let base = match memory.reserve(Some(preferred), size, PRIVATE, SHARED) {
        Some(base) => base,
        None if image.headers.relocatable() => {
            let base = memory.reserve(None, size, PRIVATE, SHARED)?;
            if let Err(e) = image.relocate(base) {
                return Some(Err(format!("{} can't be relocated: {}.", name, e)));
            }
            base
        }
        None => {
            return Some(Err(format!(
                "{} must be loaded at {:08X}h, where Windows 95 has no room for it.",
                name, preferred
            )));
        }
    };
    process.image_base = base;
    process.modules.insert(
        0,
        Module {
            name: name.to_string(),
            base,
            size,
        },
    );

    // Its imports: the built-in functions' stubs, and for the functions
    // the built-in DLLs don't have yet, stubs that say so.
    for dll in imports {
        let index = stubs::find_dll(&dll.name).expect("checked by load");
        let (_, table) = exports
            .iter()
            .find(|(i, _)| *i == index)
            .expect("loaded above");
        for (slot, import) in &dll.functions {
            let found = match import {
                pe::Import::Name { name, .. } => table.by_name(name),
                pe::Import::Ordinal(ordinal) => table.by_ordinal(*ordinal),
            };
            let address = match found.map(|e| &e.target) {
                Some(pe::ExportTarget::Rva(rva)) => stubs::DLLS[index].base + rva,
                _ => {
                    let what = match import {
                        pe::Import::Name { name, .. } => name.clone(),
                        pe::Import::Ordinal(ordinal) => format!("#{}", ordinal),
                    };
                    missing_stub(cpu, memory, &mut process, stubs::DLLS[index].name, &what)?
                }
            };
            if image.set_u32(*slot, address).is_err() {
                return Some(Err(format!("{}'s import table is damaged.", name)));
            }
        }
    }
    let bus = &mut cpu.bus;

    // Static TLS: the template's copy for the thread, in slot 0, which the
    // program finds in its index variable.
    let mut tls_block = None;
    match image.tls() {
        Ok(Some(tls)) => {
            let template = match image.tls_template(&tls) {
                Ok(template) => template,
                Err(e) => return Some(Err(format!("{}'s TLS directory is damaged: {}.", name, e))),
            };
            let len = (template.len() as u32).max(1).next_multiple_of(PAGE);
            let block = memory.reserve(None, len, PRIVATE, SHARED)?;
            memory.commit(bus, block, len, USER | WRITABLE)?;
            memory.write(bus, block, &template)?;
            // (Relocated with the image, as the directory's addresses are.)
            let index = tls.index_address.wrapping_sub(base);
            if image.set_u32(index, 0).is_err() {
                return Some(Err(format!("{}'s TLS index is outside it.", name)));
            }
            if !tls.callbacks.is_empty() {
                bus.log_string("[W32] The program's TLS callbacks aren't called yet");
            }
            process.tls_used = 1;
            tls_block = Some(block);
        }
        Ok(None) => {}
        Err(e) => return Some(Err(format!("{}'s TLS directory is damaged: {}.", name, e))),
    }

    // The image's pages, writable where a section is.
    for page in (0..size).step_by(PAGE as usize) {
        let writable = image
            .headers
            .sections
            .iter()
            .any(|s| s.writable() && page < s.rva + s.mapped_size() && s.rva < page + PAGE);
        let phys = memory.page(bus)?;
        memory.map(
            bus,
            base + page,
            phys,
            USER | if writable { WRITABLE } else { 0 },
        )?;
    }
    memory.write(bus, base, &image.data)?;

    // The stack: the reserve, its top committed.
    let reserve = image.headers.stack_reserve.clamp(GRANULARITY, 0x100_0000);
    let stack = memory.reserve(None, reserve, PRIVATE, SHARED)?;
    let reserve = reserve.next_multiple_of(GRANULARITY);
    let commit = reserve.min(MAX_STACK_COMMIT);
    let stack_top = stack + reserve;
    memory.commit(bus, stack_top - commit, commit, USER | WRITABLE)?;

    // The command line, in a block of the process's own.
    let data = memory.reserve(None, GRANULARITY, PRIVATE, SHARED)?;
    memory.commit(bus, data, PAGE, USER | WRITABLE)?;
    let mut line = crate::dosstr::to_bytes(&command_line(cpu, path));
    let bus = &mut cpu.bus;
    line.truncate(PAGE as usize - 1);
    line.push(0);
    memory.write(bus, data, &line)?;
    process.command_line = data;

    // The TIB.
    for (field, value) in [
        (TIB_EXCEPTION_LIST, 0xFFFF_FFFF),
        (TIB_STACK_TOP, stack_top),
        (TIB_STACK_LIMIT, stack_top - commit),
        (TIB_SELF, tib),
        (TIB_TLS_ARRAY, tib + TLS_ARRAY),
    ] {
        memory.write_32(bus, tib + field, value)?;
    }
    if let Some(block) = tls_block {
        memory.write_32(bus, tib + TLS_ARRAY, block)?;
    }

    if image.headers.entry == 0 {
        return Some(Err(format!("{} has no entry point.", name)));
    }
    Some(Ok(Loaded {
        process,
        entry: base + image.headers.entry,
        stack_top,
    }))
}

/// The command line: the program's path and the PSP's command tail.
fn command_line(cpu: &mut Cpu, path: &str) -> String {
    let psp = cpu.current_psp as usize * 16;
    let len = (cpu.bus.read_8(psp + 0x80) as usize).min(0x7E);
    let tail: Vec<u8> = (0..len).map(|i| cpu.bus.read_8(psp + 0x81 + i)).collect();
    format!("{}{}", path, crate::dosstr::from_bytes(&tail))
}

/// A stub for `function` of the built-in DLL `dll`, which it doesn't have
/// yet: calling it ends the program, saying what it needed. Returns the
/// stub's address.
fn missing_stub(
    cpu: &mut Cpu,
    memory: &mut Memory,
    process: &mut Process,
    dll: &str,
    function: &str,
) -> Option<u32> {
    let bus = &mut cpu.bus;
    let index = process.missing.len() as u32;
    if index >= MAX_MISSING {
        return None;
    }
    let at = index * STUB;
    if at.is_multiple_of(PAGE) {
        if index == 0 {
            process.missing_base = memory.reserve(None, MAX_MISSING * STUB, SHARED, SYSTEM)?;
        }
        memory.commit(bus, process.missing_base + at, PAGE, USER)?;
        let page = memory.translate(bus, process.missing_base + at)?;
        process.stub_pages.push(page >> 12);
    }
    let address = process.missing_base + at;
    let stub = stubs::stub(FIRST_MISSING + index as u16, 0, stubs::Conv::Cdecl);
    memory.write(bus, address, &stub)?;
    process
        .missing
        .push((dll.to_string(), function.to_string()));
    Some(address)
}
