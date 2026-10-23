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
    /// No return: the stub jumps back to its trap, which runs again (the
    /// idle loop).
    Loop,
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
    /// Its variables, in a writable data section after the code: the C
    /// runtime's `_iob` and `_environ`, which programs import as data.
    pub vars: &'static [Var],
    /// What fills the variables of a process the DLL is loaded in, once
    /// the process has its memory, heap and environment.
    pub init: Option<Init>,
    /// The tables of methods of its COM interfaces (com.rs), after the
    /// stubs in its code section.
    pub vtables: &'static [Vtable],
}

/// A COM interface's table of methods, which the DLL's objects of it
/// point to: each method a function of the DLL, by its name in `apis`.
/// Methods are stdcall, and take the object as their first argument.
pub struct Vtable {
    pub name: &'static str,
    pub methods: &'static [&'static str],
}

/// A variable of a built-in DLL: `size` bytes of its data section, zeroed
/// until `Dll::init` fills them. Exported by `name` unless its ordinal is
/// 0, as the DLL's own state is.
pub struct Var {
    pub name: &'static str,
    pub ordinal: u16,
    pub size: u32,
}

/// A DLL's `init`: the process, which isn't the running one yet.
pub type Init = fn(&mut crate::bus::Bus, &mut super::Process) -> Option<()>;

/// The built-in DLLs. Append only (see the module's notes).
pub static DLLS: &[&Dll] = &[
    &super::kernel32::DLL,
    &super::advapi32::DLL,
    &super::msvcrt::DLL,
    &super::user32::DLL,
    &super::gdi32::DLL,
    &super::ole32::DLL,
    &super::ddraw::DLL,
    &super::dsound::DLL,
    &super::dinput::DLL,
    &super::winmm::DLL,
    &super::glide::DLL,
    &super::dplayx::DLL,
    &super::shell32::DLL,
    &super::comctl32::DLL,
    &super::comctl32::COMDLG32,
    &super::wsock32::DLL,
    &super::lz32::DLL,
    &super::version::DLL,
    &super::oleaut32::DLL,
    &super::advpack::DLL,
];

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

/// Variables start at multiples of this in the data section.
const VAR_ALIGN: u32 = 8;

/// Bytes the stubs of the DLL `dll` take, and the vtables after them.
fn code_size(dll: &Dll) -> u32 {
    let methods: usize = dll.vtables.iter().map(|v| v.methods.len()).sum();
    dll.apis.len() as u32 * STUB + 4 * methods as u32
}

/// Where the data section of the DLL `dll` starts: the page after its
/// code.
fn data_rva(dll: &Dll) -> u32 {
    CODE_RVA + code_size(dll).next_multiple_of(0x1000)
}

/// The address of the vtable `vtable` of the DLL `dll`.
pub fn vtable_address(dll: usize, vtable: usize) -> u32 {
    let d = DLLS[dll];
    let before: usize = d.vtables[..vtable].iter().map(|v| v.methods.len()).sum();
    d.base + CODE_RVA + d.apis.len() as u32 * STUB + 4 * before as u32
}

/// The place in `apis` of each method of the vtables of `dll`.
fn vtable_methods(dll: &Dll) -> impl Iterator<Item = usize> + '_ {
    dll.vtables.iter().flat_map(move |v| {
        v.methods.iter().map(move |&m| {
            dll.apis
                .iter()
                .position(|a| a.name == m)
                .unwrap_or_else(|| panic!("{} has no {} for {}", dll.name, m, v.name))
        })
    })
}

/// The offset of each variable of `dll` in its data section, and the
/// section's size.
fn var_offsets(dll: &Dll) -> (Vec<u32>, u32) {
    let mut at = 0;
    let offsets = dll
        .vars
        .iter()
        .map(|var| {
            let offset = at;
            at = (at + var.size).next_multiple_of(VAR_ALIGN);
            offset
        })
        .collect();
    (offsets, at)
}

/// The address of the variable `var` of the DLL `dll`.
pub fn var_address(dll: usize, var: usize) -> u32 {
    let dll = DLLS[dll];
    dll.base + data_rva(dll) + var_offsets(dll).0[var]
}

/// Fill the variables of the built-in DLLs `process` has loaded, for it
/// (`Dll::init`). None when it runs out of memory.
pub fn init_process(bus: &mut crate::bus::Bus, process: &mut super::Process) -> Option<()> {
    for dll in DLLS {
        let loaded = process.modules.iter().any(|m| m.base == dll.base);
        if let Some(init) = dll.init.filter(|_| loaded) {
            init(bus, process)?;
        }
    }
    Some(())
}

/// The stub of the function with id `id`, which takes `args` arguments
/// the way `conv` says.
pub fn stub(id: u16, args: u8, conv: Conv) -> [u8; STUB as usize] {
    let [lo, hi] = id.to_le_bytes();
    let mut code = [0x90; STUB as usize];
    code[..4].copy_from_slice(&[0xFE, TRAP, lo, hi]);
    if conv == Conv::Loop {
        // JMP back to the trap.
        code[4..6].copy_from_slice(&[0xEB, 0xFA]);
    } else if conv == Conv::Stdcall && args > 0 {
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
    let mut code: Vec<u8> = dll
        .apis
        .iter()
        .enumerate()
        .flat_map(|(i, api)| stub(id(index, i), api.args, api.conv))
        .collect();
    for method in vtable_methods(dll) {
        code.extend_from_slice(&stub_address(index, method).to_le_bytes());
    }
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
    if !dll.vars.is_empty() {
        let (offsets, size) = var_offsets(dll);
        let data = b.section_sized(
            ".data",
            &[],
            size,
            pe::SCN_INITIALIZED_DATA | pe::SCN_READ | pe::SCN_WRITE,
        );
        debug_assert_eq!(data, data_rva(dll));
        for (var, offset) in dll.vars.iter().zip(offsets) {
            if var.ordinal != 0 {
                b.export(Some(var.name), var.ordinal, data + offset);
            }
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
    // Where it was called from, for the log: read without a page fault,
    // as a function the entry point returns to has no caller there.
    let caller = cpu.bus.logs().then(|| super::peek_32(cpu, esp));
    let caller = caller.flatten().unwrap_or(0);
    let result = (api.handler)(cpu, &args);
    // File mappings no handle or view holds go, then the files whose
    // last handle the function closed close in DOS.
    if !cpu.bus.win32.sections.is_empty() {
        super::kernel32::sweep_mappings(cpu);
    }
    super::object::close_files(&mut cpu.bus);
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
