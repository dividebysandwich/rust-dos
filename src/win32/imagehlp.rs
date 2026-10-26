//! IMAGEHLP.DLL: the symbol and stack walking functions games of the
//! time call from their crash handlers, to write where they crashed into
//! a log (Grom and Zanzarah do). No module has debug symbols here, as
//! on a machine without the game's .DBG or .PDB files: modules are
//! found and described, symbols aren't, and a stack is walked by its
//! frame pointers.

use super::kernel32::virtual_memory::allows;
use super::process;
use super::stubs::{Api, Conv, Dll};
use crate::cpu::Cpu;

pub static DLL: Dll = Dll {
    name: "IMAGEHLP.DLL",
    base: 0xBF70_0000,
    apis: APIS,
    vars: &[],
    init: None,
    vtables: &[],
};

const fn stdcall(
    name: &'static str,
    ordinal: u16,
    args: u8,
    handler: super::stubs::Handler,
) -> Api {
    Api {
        name,
        ordinal,
        args,
        conv: Conv::Stdcall,
        handler,
    }
}

/// The functions. Append only (see `stubs`).
static APIS: &[Api] = &[
    stdcall("ImagehlpApiVersion", 1, 0, imagehlp_api_version),
    stdcall("StackWalk", 2, 9, stack_walk),
    stdcall("SymCleanup", 3, 1, true_),
    stdcall("SymFunctionTableAccess", 4, 2, null),
    stdcall("SymGetModuleBase", 5, 2, sym_get_module_base),
    stdcall("SymGetModuleInfo", 6, 3, sym_get_module_info),
    stdcall("SymGetOptions", 7, 0, sym_get_options),
    stdcall("SymGetSymFromAddr", 8, 4, sym_get_sym_from_addr),
    stdcall("SymInitialize", 9, 3, true_),
    stdcall("SymLoadModule", 10, 6, sym_load_module),
    stdcall("SymSetOptions", 11, 1, sym_set_options),
    stdcall("SymUnDName", 12, 3, sym_und_name),
    stdcall("SymUnloadModule", 13, 2, true_),
    stdcall("UnDecorateSymbolName", 14, 4, undecorate_symbol_name),
    stdcall("ImageNtHeader", 15, 1, image_nt_header),
];

const TRUE: u32 = 1;
const FALSE: u32 = 0;

const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_MOD_NOT_FOUND: u32 = 126;
const ERROR_INVALID_ADDRESS: u32 = 487;

/// SymSetOptions' SYMOPT_UNDNAME, which the options start with.
const SYMOPT_UNDNAME: u32 = 2;

/// IMAGEHLP_MODULE's size, and where its names are: ModuleName (32
/// bytes), ImageName and LoadedImageName (256 each).
const IMAGEHLP_MODULE: u32 = 572;
const MODULE_NAME: u32 = 28;
const IMAGE_NAME: u32 = 60;
const LOADED_IMAGE_NAME: u32 = 316;

/// What a process has of IMAGEHLP: the options SymSetOptions set, None
/// until it does, and the API_VERSION ImagehlpApiVersion gives.
#[derive(Clone, Debug, Default)]
pub struct State {
    options: Option<u32>,
    api_version: u32,
}

crate::state_fields!(State {
    options,
    api_version
});

fn true_(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(TRUE)
}

/// SymFunctionTableAccess(process, address): x86 code has no function
/// tables, and there are no FPO records without symbols.
fn null(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// ImagehlpApiVersion(): an API_VERSION of 4.0, the version Windows 98's
/// IMAGEHLP reports, in a block of the process heap that stays.
fn imagehlp_api_version(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let kept = super::process(cpu).imagehlp.api_version;
    if kept != 0 {
        return Some(kept);
    }
    let Some(at) = super::with_process(cpu, |p, bus| p.heap.alloc(&mut p.memory, bus, 8)) else {
        return Some(0);
    };
    super::process(cpu).imagehlp.api_version = at;
    // MajorVersion, MinorVersion, Revision, Reserved.
    for (i, v) in [4u16, 0, 5, 0].iter().enumerate() {
        cpu.bus.guest_write_16(at + 2 * i as u32, *v);
    }
    Some(at)
}

fn sym_get_options(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(
        super::process(cpu)
            .imagehlp
            .options
            .unwrap_or(SYMOPT_UNDNAME),
    )
}

fn sym_set_options(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    super::process(cpu).imagehlp.options = Some(args[0]);
    Some(args[0])
}

/// The module of the running process that `address` is in: its base,
/// size, file name and path. Images mapped as data files aren't modules
/// (as for GetModuleHandle).
fn module_at(cpu: &mut Cpu, address: u32) -> Option<(u32, u32, String, String)> {
    process(cpu)
        .modules
        .iter()
        .filter(|m| m.flags & super::loader::DATAFILE == 0)
        .find(|m| address >= m.base && address - m.base < m.size)
        .map(|m| (m.base, m.size, m.name.clone(), m.path.clone()))
}

/// SymGetModuleBase(process, address): the base of the module the
/// address is in, or 0.
fn sym_get_module_base(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(module_at(cpu, args[1]).map_or(0, |m| m.0))
}

/// SymLoadModule(process, file, image, module, base, size): the base of
/// the module, which is loaded already: `base`, or else the base of the
/// loaded module of that image's name. 0 for a module that isn't there.
fn sym_load_module(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[_, _, image, _, base, _] = args else {
        unreachable!()
    };
    if base != 0 {
        return Some(base);
    }
    let name = if image == 0 {
        String::new()
    } else {
        super::user32::string_at(cpu, image)?
    };
    let found = process(cpu).module(&name);
    if found.is_none() {
        process(cpu).last_error = ERROR_MOD_NOT_FOUND;
    }
    Some(found.unwrap_or(0))
}

/// Write `text` at `at` as a string of at most `room` bytes with its
/// zero.
fn write_string(cpu: &mut Cpu, at: u32, room: usize, text: &str) {
    write_bytes(cpu, at, room, crate::dosstr::to_bytes(text));
}

/// Write the string `bytes` at `at`, cut to at most `room` bytes with its
/// zero. How many bytes of it were written, without the zero.
fn write_bytes(cpu: &mut Cpu, at: u32, room: usize, mut bytes: Vec<u8>) -> usize {
    bytes.truncate(room - 1);
    let len = bytes.len();
    bytes.push(0);
    cpu.bus.guest_write_bytes(at, &bytes);
    len
}

/// SymGetModuleInfo(process, address, info): IMAGEHLP_MODULE of the
/// module the address is in, without symbols (SymNone).
fn sym_get_module_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[_, address, info] = args else {
        unreachable!()
    };
    if info == 0 || !allows(cpu, info, IMAGEHLP_MODULE, true) {
        process(cpu).last_error = ERROR_INVALID_PARAMETER;
        return Some(FALSE);
    }
    if cpu.bus.guest_read_32(info) < IMAGEHLP_MODULE {
        process(cpu).last_error = ERROR_INVALID_PARAMETER;
        return Some(FALSE);
    }
    let Some((base, size, name, path)) = module_at(cpu, address) else {
        process(cpu).last_error = ERROR_MOD_NOT_FOUND;
        return Some(FALSE);
    };
    cpu.bus
        .guest_fill(info + 4, (IMAGEHLP_MODULE - 4) as usize, 0);
    cpu.bus.guest_write_32(info + 4, base);
    cpu.bus.guest_write_32(info + 8, size);
    let stem = name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s);
    write_string(cpu, info + MODULE_NAME, 32, stem);
    write_string(cpu, info + IMAGE_NAME, 256, &path);
    write_string(cpu, info + LOADED_IMAGE_NAME, 256, &path);
    Some(TRUE)
}

/// SymGetSymFromAddr(process, address, displacement, symbol): there are
/// no symbols to find.
fn sym_get_sym_from_addr(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    process(cpu).last_error = ERROR_INVALID_ADDRESS;
    Some(FALSE)
}

/// SymUnDName(symbol, name, length): the IMAGEHLP_SYMBOL's name (at 20,
/// after five DWORDs) as it is: names aren't undecorated (see
/// UnDecorateSymbolName).
fn sym_und_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[symbol, out, length] = args else {
        unreachable!()
    };
    if symbol == 0 || out == 0 || length == 0 {
        return Some(FALSE);
    }
    let name = super::user32::bytes_at(cpu, symbol.wrapping_add(20))?;
    write_bytes(cpu, out, length as usize, name);
    Some(TRUE)
}

/// UnDecorateSymbolName(name, out, length, flags): the name, copied as
/// it is. C++ names aren't undecorated: only crash logs read them. The
/// length of what was written.
fn undecorate_symbol_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[name, out, length, _] = args else {
        unreachable!()
    };
    if name == 0 || out == 0 || length == 0 {
        process(cpu).last_error = ERROR_INVALID_PARAMETER;
        return Some(0);
    }
    let text = super::user32::bytes_at(cpu, name)?;
    Some(write_bytes(cpu, out, length as usize, text) as u32)
}

/// ImageNtHeader(base): the PE header of the image at `base`, or NULL.
/// Memory the program can't read gives NULL, as it does on Windows,
/// which catches the fault.
fn image_nt_header(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let base = args[0];
    if base == 0 || !allows(cpu, base, 0x40, false) {
        return Some(0);
    }
    if cpu.bus.guest_read_16(base) != 0x5A4D {
        return Some(0);
    }
    let at = base.wrapping_add(cpu.bus.guest_read_32(base + 0x3C));
    if !allows(cpu, at, 4, false) || cpu.bus.guest_read_32(at) != 0x4550 {
        return Some(0);
    }
    Some(at)
}

/// STACKFRAME's ADDRESSes (12 bytes each: offset, segment, mode), and
/// where its Params are.
const ADDR_PC: u32 = 0;
const ADDR_RETURN: u32 = 12;
const ADDR_FRAME: u32 = 24;
const ADDR_STACK: u32 = 36;
const PARAMS: u32 = 52;
/// STACKFRAME's size up to and with Reserved (KdHelp follows).
const STACKFRAME: u32 = 88;
/// ADDRESS's AddrModeFlat.
const ADDR_MODE_FLAT: u32 = 3;
const IMAGE_FILE_MACHINE_I386: u32 = 0x14C;

/// StackWalk(machine, process, thread, frame, context, read, table,
/// base, translate): the next frame of a stack, by its frame pointers.
/// The first call takes the PC, frame and stack the program set in the
/// STACKFRAME, and gives the return address and parameters of that
/// frame; each later one moves to the frame the saved EBP points to.
/// FALSE at the end of the chain, or when a frame can't be read, which
/// it checks without touching the memory: the chain of a crashed
/// program may lead anywhere. The program's memory callbacks aren't
/// called: the memory is the process's own.
fn stack_walk(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[machine, _, _, frame, ..] = args else {
        unreachable!()
    };
    if machine != IMAGE_FILE_MACHINE_I386
        || frame == 0
        || !allows(cpu, frame, STACKFRAME, true)
    {
        process(cpu).last_error = ERROR_INVALID_PARAMETER;
        return Some(FALSE);
    }
    let mut ebp = cpu.bus.guest_read_32(frame + ADDR_FRAME);
    let ret = cpu.bus.guest_read_32(frame + ADDR_RETURN);
    if ret != 0 {
        // A later call: the caller's frame.
        if !allows(cpu, ebp, 4, false) {
            return Some(FALSE);
        }
        let next = cpu.bus.guest_read_32(ebp);
        if next <= ebp {
            return Some(FALSE);
        }
        cpu.bus.guest_write_32(frame + ADDR_PC, ret);
        cpu.bus
            .guest_write_32(frame + ADDR_STACK, ebp.wrapping_add(8));
        ebp = next;
        cpu.bus.guest_write_32(frame + ADDR_FRAME, ebp);
    }
    if ebp == 0 || !allows(cpu, ebp, 24, false) {
        return Some(FALSE);
    }
    let ret = cpu.bus.guest_read_32(ebp + 4);
    if ret == 0 {
        return Some(FALSE);
    }
    cpu.bus.guest_write_32(frame + ADDR_RETURN, ret);
    for at in [ADDR_PC, ADDR_RETURN, ADDR_FRAME, ADDR_STACK] {
        cpu.bus.guest_write_32(frame + at + 8, ADDR_MODE_FLAT);
    }
    for i in 0..4 {
        let p = cpu.bus.guest_read_32(ebp + 8 + 4 * i);
        cpu.bus.guest_write_32(frame + PARAMS + 4 * i, p);
    }
    Some(TRUE)
}
