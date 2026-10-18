//! The built-in DLLs: real PE images the emulator writes into the
//! process's memory, each export a stub of the code below that traps into
//! the function written in Rust. `LoadLibrary`, `GetProcAddress`, programs
//! that read export tables and debuggers see them as any other DLL.
//!
//! ```text
//! GetTickCount:  FE 3E 04 00     service trap, function 0004h
//!                C3              ret       (no arguments, or cdecl)
//! Sleep:         FE 3E 05 00
//!                C2 04 00        ret 4     (stdcall, one argument)
//! ```
//!
//! A function's id is its DLL's place in `DLLS` and its own in the DLL's
//! table (`id`). Saved states keep the stubs in memory, so tables are only
//! added to at their ends.

use super::TRAP;
use crate::cpu::Cpu;
use rust_dos_pe as pe;

/// How a function takes its arguments: stdcall functions take them off
/// the stack when they return, cdecl ones leave them to the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conv {
    Stdcall,
    Cdecl,
}

/// A function's Rust side: the arguments as the program passed them;
/// what it returns in EAX, or None for a function that doesn't return to
/// the program (ExitProcess).
pub type Handler = fn(&mut Cpu, &[u32]) -> Option<u32>;

pub struct Api {
    pub name: &'static str,
    /// Its ordinal, or 0 for the DLL's own code, which it doesn't export.
    pub ordinal: u16,
    /// How many 32-bit arguments it takes.
    pub args: u8,
    pub conv: Conv,
    pub handler: Handler,
}

pub struct Dll {
    /// Its file name, as programs import it.
    pub name: &'static str,
    /// Where it is loaded: where Windows 95 has it.
    pub base: u32,
    pub apis: &'static [Api],
}

/// The built-in DLLs. Append only (see the module's notes).
pub static DLLS: &[&Dll] = &[&super::kernel32::DLL];

/// Bytes each function's stub takes.
pub const STUB: u32 = 8;
/// Where the stubs start in a built-in DLL: its first section.
const CODE_RVA: u32 = 0x1000;

/// The built-in DLL a program's import of `name` names: "KERNEL32.DLL",
/// "kernel32.dll" and "KERNEL32" all do.
pub fn find_dll(name: &str) -> Option<usize> {
    let name = name.trim();
    let split = name.len().saturating_sub(4);
    let base = match name.get(split..) {
        Some(ext) if ext.eq_ignore_ascii_case(".dll") => &name[..split],
        _ => name,
    };
    DLLS.iter()
        .position(|dll| dll.name[..dll.name.len() - 4].eq_ignore_ascii_case(base))
}

pub fn id(dll: usize, api: usize) -> u16 {
    (dll << 10 | api) as u16
}

/// The DLL and function of the id `id`.
pub fn api(id: u16) -> Option<(&'static Dll, &'static Api)> {
    let dll = DLLS.get((id >> 10) as usize)?;
    Some((dll, dll.apis.get((id & 0x3FF) as usize)?))
}

/// "KERNEL32.WriteFile", for the trace and debuggers.
pub fn function_name(id: u16) -> Option<String> {
    let (dll, api) = api(id)?;
    Some(format!("{}.{}", &dll.name[..dll.name.len() - 4], api.name))
}

/// The address of the stub of function `api` of the DLL `dll`.
pub fn stub_address(dll: usize, api: usize) -> u32 {
    DLLS[dll].base + CODE_RVA + api as u32 * STUB
}

/// The stub of the function with id `id`, which takes `args` arguments
/// the way `conv` says.
pub fn stub(id: u16, args: u8, conv: Conv) -> [u8; STUB as usize] {
    let [lo, hi] = id.to_le_bytes();
    let mut code = [0x90; STUB as usize];
    code[..4].copy_from_slice(&[0xFE, TRAP, lo, hi]);
    if conv == Conv::Stdcall && args > 0 {
        let [lo, hi] = (args as u16 * 4).to_le_bytes();
        code[4..7].copy_from_slice(&[0xC2, lo, hi]);
    } else {
        code[4] = 0xC3;
    }
    code
}

/// The image of the built-in DLL `dll`: its stubs and export directory.
pub fn image(index: usize) -> pe::Image {
    let dll = DLLS[index];
    let mut b = pe::Builder::dll(dll.base, dll.name);
    let code: Vec<u8> = dll
        .apis
        .iter()
        .enumerate()
        .flat_map(|(i, api)| stub(id(index, i), api.args, api.conv))
        .collect();
    let rva = b.section(
        ".text",
        &code,
        pe::SCN_CODE | pe::SCN_EXECUTE | pe::SCN_READ,
    );
    debug_assert_eq!(rva, CODE_RVA);
    for (i, api) in dll.apis.iter().enumerate() {
        if api.ordinal != 0 {
            b.export(Some(api.name), api.ordinal, rva + i as u32 * STUB);
        }
    }
    pe::Image::load(&b.build()).expect("a built-in DLL's image")
}

/// Run the function the trap with id `id` is the stub of, on the
/// program's stack: its return address, then its arguments.
pub fn call(cpu: &mut Cpu, id: u16) {
    let Some((dll, api)) = api(id) else {
        super::fatal(
            cpu,
            &format!("a call of the unknown function {:04X}h", id),
            None,
        );
        return;
    };
    cpu.set_eip(cpu.eip().wrapping_add(4));
    let esp = cpu.esp();
    let args: Vec<u32> = (0..api.args as u32)
        .map(|i| cpu.bus.guest_read_32(esp.wrapping_add(4 + 4 * i)))
        .collect();
    if cpu.bus.guest_faulted() {
        return;
    }
    let caller = cpu.bus.guest_read_32(esp);
    let result = (api.handler)(cpu, &args);
    if cpu.bus.guest_faulted() {
        return;
    }
    if let Some(value) = result {
        cpu.set_eax(value);
    }
    if cpu.bus.logs() {
        let args: Vec<String> = args.iter().map(|a| format!("{:08X}", a)).collect();
        let result = result.map_or("(no return)".to_string(), |r| format!("{:08X}", r));
        cpu.bus.log_string(&format!(
            "[W32] {:08X}: {}.{}({}) = {}",
            caller,
            &dll.name[..dll.name.len() - 4],
            api.name,
            args.join(", "),
            result
        ));
    }
}
