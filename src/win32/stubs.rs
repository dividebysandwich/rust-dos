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
//!
//! A few functions are x86 code instead (`Conv::Code`), as Windows has
//! them: the code goes after the stubs and vtables, the export points at
//! it, and the function's stub jumps to it. Called many times a frame, a
//! trap leaves translated code for the interpreter on every call, where
//! code of the DLL's own is translated with the program's.

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
    /// The function is this x86 code, which returns to its caller itself.
    /// The DLL's code section has it after the stubs and vtables, its
    /// export points at it, and its stub jumps to it. The `handler` runs
    /// only in a state saved when the function was a trap, whose memory
    /// still has that stub. A function that has been code since it was
    /// added has no such state, and its handler is `never_a_trap`.
    Code(&'static [u8]),
}

/// The handler of a function that has been x86 code (`Conv::Code`) since
/// it was added: its stub has never been a trap, so this doesn't run
/// unless the code section is overwritten with one. The program then ends
/// as for a call of an unknown function.
pub fn never_a_trap(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    super::fatal(cpu, "a trap of a function that is x86 code", None);
    None
}

/// A function's Rust side: the arguments as the program passed them;
/// what it returns in EAX, or None for a function that doesn't return to
/// the program (ExitProcess).
pub type Handler = fn(&mut Cpu, &[u32]) -> Option<u32>;

pub struct Api {
    pub name: &'static str,
    /// The ordinal it is exported under, or 0 for the DLL's own code,
    /// which it doesn't export: Windows 98 SE's ordinal for it, or one
    /// after Windows' last for a function Windows doesn't have
    /// (ordinals.rs).
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
    /// What gives back what `init` took, at the last FreeLibrary.
    pub term: Option<Term>,
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

/// A DLL's `term`: the process it is going from.
pub type Term = fn(&mut crate::bus::Bus, &mut super::Process);

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
    &super::comdlg32::DLL,
    &super::wsock32::DLL,
    &super::ntdll::DLL,
    &super::dmusic::DMUSIC,
    &super::dmusic::DMIME,
    &super::dmusic::DMLOADER,
    &super::dmusic::DMSTYLE,
    &super::dmusic::DMBAND,
    &super::dmusic::DMCOMPOS,
    &super::lz32::DLL,
    &super::verdll::DLL,
    &super::oleaut32::DLL,
    &super::advpack::DLL,
    &super::winspool::DLL,
    &super::mpr::DLL,
    &super::d3d8::DLL,
    &super::imagehlp::DLL,
    &super::ws2_32::DLL,
    &super::msimg32::DLL,
    &super::opengl32::DLL,
    &super::wow32::DLL,
    &super::vfw::msvfw32::DLL,
    &super::vfw::avifil32::DLL,
    &super::dinput::DLL8,
    &super::msacm32::DLL,
    &super::dciman32::DLL,
    &super::wnaspi32::DLL,
    &super::dshow::QUARTZ,
    &super::dshow::AMSTREAM,
    &super::glide::DLL3,
    &super::dplayx::DPLAY,
    &super::tapi32::DLL,
    &super::imm32::DLL,
    &super::glu32::DLL,
    &super::oledlg::DLL,
    &super::olepro32::DLL,
];

/// Bytes each function's stub takes.
pub const STUB: u32 = 8;
/// Where the stubs start in a built-in DLL: its first section.
const CODE_RVA: u32 = 0x1000;

/// The built-in DLL a program's import of `name` names: "KERNEL32.DLL",
/// "kernel32.dll" and "KERNEL32" all do, and a driver's full name,
/// "WINSPOOL.DRV".
pub fn find_dll(name: &str) -> Option<usize> {
    let name = name.trim();
    let split = name.len().saturating_sub(4);
    let base = match name.get(split..) {
        Some(ext) if ext.eq_ignore_ascii_case(".dll") => &name[..split],
        _ => name,
    };
    DLLS.iter().position(|dll| {
        dll.name[..dll.name.len() - 4].eq_ignore_ascii_case(base)
            || dll.name.eq_ignore_ascii_case(name)
    })
}

pub fn id(dll: usize, api: usize) -> u16 {
    (dll << 10 | api) as u16
}

/// Whether the function of the id `id`, of a built-in DLL (16-bit modules
/// have ids of their own), only reads the emulated clock in milliseconds:
/// GetTickCount, timeGetTime, or the system's time (GetSystemTime,
/// GetLocalTime, GetSystemTimeAsFileTime), which programs call in a loop
/// to wait (`idle::clock_read`).
pub fn reads_clock(id: u16) -> bool {
    static IDS: std::sync::OnceLock<[Option<u16>; 5]> = std::sync::OnceLock::new();
    let find = |dll: &str, name: &str| {
        let dll = find_dll(dll)?;
        let api = DLLS[dll].apis.iter().position(|api| api.name == name)?;
        Some(self::id(dll, api))
    };
    IDS.get_or_init(|| {
        [
            find("KERNEL32", "GetTickCount"),
            find("WINMM", "timeGetTime"),
            find("KERNEL32", "GetSystemTimeAsFileTime"),
            find("KERNEL32", "GetSystemTime"),
            find("KERNEL32", "GetLocalTime"),
        ]
    })
    .contains(&Some(id))
}

/// Whether the function of the id `id` is one of
/// `screen::api_settles`: a bit for every id, worked out once.
fn settles(id: u16) -> bool {
    static BITS: std::sync::OnceLock<Vec<u64>> = std::sync::OnceLock::new();
    let bits = BITS.get_or_init(|| {
        let mut bits = vec![0u64; DLLS.len() << 4];
        for (d, dll) in DLLS.iter().enumerate() {
            for (a, api) in dll.apis.iter().enumerate() {
                if super::screen::api_settles(dll.name, api.name) {
                    let id = self::id(d, a) as usize;
                    bits[id >> 6] |= 1 << (id & 63);
                }
            }
        }
        bits
    });
    bits.get(id as usize >> 6)
        .is_some_and(|word| word >> (id & 63) & 1 != 0)
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

/// Bytes the stubs of the DLL `dll` take, and the vtables after them:
/// where the functions given as code start in the code section.
fn stubs_size(dll: &Dll) -> u32 {
    let methods: usize = dll.vtables.iter().map(|v| v.methods.len()).sum();
    dll.apis.len() as u32 * STUB + 4 * methods as u32
}

/// The code of each function of `dll` given as code (`Conv::Code`): its
/// place in `apis`, its offset in the code section, and the code.
fn functions_as_code(dll: &Dll) -> impl Iterator<Item = (usize, u32, &'static [u8])> + '_ {
    let mut at = stubs_size(dll);
    dll.apis
        .iter()
        .enumerate()
        .filter_map(move |(i, api)| match api.conv {
            Conv::Code(code) => {
                let offset = at;
                at += code.len() as u32;
                Some((i, offset, code))
            }
            _ => None,
        })
}

/// Bytes the code section of the DLL `dll` takes: the stubs, the vtables
/// and the functions given as code.
fn code_size(dll: &Dll) -> u32 {
    let code: u32 = functions_as_code(dll)
        .map(|(_, _, code)| code.len() as u32)
        .sum();
    stubs_size(dll) + code
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
/// the way `conv` says. For a function given as code (`Conv::Code`) this
/// is the trap and `RET` it had before, which `builder` doesn't write: its
/// stub there is a jump to its code (`jump`).
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

/// The stub of a function given as code: a JMP from the stub at RVA
/// `from` to its code at RVA `to`.
fn jump(from: u32, to: u32) -> [u8; STUB as usize] {
    let mut code = [0x90; STUB as usize];
    code[0] = 0xE9;
    code[1..5].copy_from_slice(&to.wrapping_sub(from + 5).to_le_bytes());
    code
}

/// The image of the built-in DLL `dll`: its stubs and export directory,
/// with the functions `version` of Windows has (version.rs).
pub fn image(index: usize, version: super::version::WindowsVersion) -> pe::Image {
    pe::Image::load(&builder(index, version).build()).expect("a built-in DLL's image")
}

/// The file of the built-in DLL `dll` in the system directory
/// (dll_files.rs): its image, and the version resource GetFileVersionInfo
/// gives for it (verdll.rs).
pub fn file(index: usize, version: super::version::WindowsVersion) -> Vec<u8> {
    let mut b = builder(index, version);
    let resource = super::verdll::builtin_version(DLLS[index].name, version);
    b.resource(RT_VERSION, VS_VERSION_INFO, LANG_ENGLISH_US, &resource);
    b.build()
}

/// RT_VERSION, the version resource's name, and the language it is in.
const RT_VERSION: u16 = 16;
const VS_VERSION_INFO: u16 = 1;
const LANG_ENGLISH_US: u16 = 0x0409;

/// The image of the built-in DLL `dll` as `image` has it, to build.
fn builder(index: usize, version: super::version::WindowsVersion) -> pe::Builder {
    let dll = DLLS[index];
    let mut b = pe::Builder::dll(dll.base, dll.name);
    // Where each function's export points: its stub, or its code.
    let mut entries: Vec<u32> = (0..dll.apis.len() as u32)
        .map(|i| CODE_RVA + i * STUB)
        .collect();
    for (i, offset, _) in functions_as_code(dll) {
        entries[i] = CODE_RVA + offset;
    }
    let mut code: Vec<u8> = dll
        .apis
        .iter()
        .enumerate()
        .flat_map(|(i, api)| match api.conv {
            Conv::Code(_) => jump(CODE_RVA + i as u32 * STUB, entries[i]),
            conv => stub(id(index, i), api.args, conv),
        })
        .collect();
    for method in vtable_methods(dll) {
        code.extend_from_slice(&stub_address(index, method).to_le_bytes());
    }
    for (_, offset, function) in functions_as_code(dll) {
        debug_assert_eq!(code.len() as u32, offset);
        code.extend_from_slice(function);
    }
    let rva = b.section(
        ".text",
        &code,
        pe::SCN_CODE | pe::SCN_EXECUTE | pe::SCN_READ,
    );
    debug_assert_eq!(rva, CODE_RVA);
    for (i, api) in dll.apis.iter().enumerate() {
        if api.ordinal != 0 && super::version::exports(version, dll.name, api.name) {
            b.export(Some(api.name), api.ordinal, entries[i]);
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
    b
}

/// Run the function the trap with id `id` is the stub of, on the
/// program's stack: its return address, then its arguments.
pub fn call(cpu: &mut Cpu, id: u16) {
    let Some((dll, api)) = api(id) else {
        // Win32 code runs in flat segments, so EIP is the linear address
        // the process's modules are found by.
        let eip = cpu.eip();
        let module = super::running(cpu).and_then(|p| {
            p.modules
                .iter()
                .find(|m| (m.base..m.base.saturating_add(m.size)).contains(&eip))
                .map(|m| m.name.clone())
        });
        super::fatal(cpu, &unknown_message(id, module.as_deref()), None);
        return;
    };
    cpu.set_eip(cpu.eip().wrapping_add(4));
    let esp = cpu.esp();
    let profile = crate::profile::zone(crate::profile::Zone::Win32Args);
    // The arguments, in an array on the stack unless there are more than
    // 16: this runs for every call the program makes.
    let mut few = [0u32; 16];
    let mut many = Vec::new();
    let args: &mut [u32] = match few.get_mut(..api.args as usize) {
        Some(args) => args,
        None => {
            many.resize(api.args as usize, 0);
            &mut many
        }
    };
    // Read in one piece: the page tables are walked once for each
    // page the arguments are on, which is one for nearly every call.
    let mut raw = [0u8; 64];
    let mut more = Vec::new();
    let raw: &mut [u8] = match raw.get_mut(..4 * args.len()) {
        Some(raw) => raw,
        None => {
            more.resize(4 * args.len(), 0);
            &mut more
        }
    };
    cpu.bus.guest_read_bytes(esp.wrapping_add(4), raw);
    for (arg, word) in args.iter_mut().zip(raw.as_chunks::<4>().0) {
        *arg = u32::from_le_bytes(*word);
    }
    drop(profile);
    let args = &*args;
    if cpu.bus.guest_faulted() {
        return;
    }
    // Where it was called from, for the log: read without a page fault,
    // as a function the entry point returns to has no caller there.
    let traced = super::trace::on(cpu);
    let caller = traced.then(|| super::peek_32(cpu, esp));
    let caller = caller.flatten().unwrap_or(0);
    // What a ToadForce's driver queued is drawn before a function that
    // may read video memory or let the program read it.
    if cpu.bus.toadforce_queued() && settles(id) {
        super::screen::settle(cpu);
    }
    let result = {
        let _profile = std::ptr::eq(dll, &super::opengl32::DLL)
            .then(|| crate::profile::zone(crate::profile::Zone::OpenGlCall));
        (api.handler)(cpu, args)
    };
    let _profile = crate::profile::zone(crate::profile::Zone::Win32After);
    // File mappings no handle or view holds go, then the files whose
    // last handle the function closed close in DOS.
    if !cpu.bus.win32.sections.is_empty() && super::kernel32::sweep_mappings(cpu) {
        crate::idle::not_pure(cpu);
    }
    super::object::close_files(&mut cpu.bus);
    if cpu.bus.guest_faulted() {
        return;
    }
    if let Some(value) = result {
        cpu.set_eax(value);
    }
    let module = &dll.name[..dll.name.len() - 4];
    if super::trace3d::on(cpu) {
        super::trace3d::call(cpu, module, api.name, args, result);
    }
    let call = super::trace::Call {
        kind: super::trace::Kind::W32,
        caller,
        id,
        module,
        name: api.name,
    };
    if traced && super::trace::admit(cpu, call) {
        let args: Vec<String> = args.iter().map(|a| format!("{:08X}", a)).collect();
        let result = result.map_or("(no return)".to_string(), |r| format!("{:08X}", r));
        cpu.bus.log_string(&format!(
            "[W32] {:08X}: {}.{}({}) = {}",
            caller,
            module,
            api.name,
            args.join(", "),
            result
        ));
    }
}

/// The fatal message for a call of the stub `id` whose code is in the
/// module `module`: the table's entry for the id's DLL index, so that a
/// stale id (saved states keep their stub ids) shows which DLL it was.
fn unknown_message(id: u16, module: Option<&str>) -> String {
    let from = module.unwrap_or("code outside any module");
    let (index, function) = ((id >> 10) as usize, (id & 0x3FF) as usize);
    let table = match DLLS.get(index) {
        Some(dll) => format!(
            "DLL index {} is {}, which has {} functions",
            index,
            dll.name,
            dll.apis.len()
        ),
        None => format!("DLL index {} is not in the table", index),
    };
    format!(
        "a call of the unknown function {:04X}h (from {}; {}; no function {})",
        id, from, table, function
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The DLLs' names in the order of `DLLS`. Saved states keep the
    /// stubs' ids (DLL index << 10 | function), so a DLL goes at the end
    /// of this list, never in the middle: a DLL inserted before others
    /// moves their index, and a state saved before the insert then calls
    /// a different DLL's functions ("unknown function 881Ah": DINPUT8's
    /// function 26 was WOW32's index 34 once WOW32 came before it).
    /// Add the new DLL's name here at the end.
    const ORDER: &[&str] = &[
        "KERNEL32.DLL",
        "ADVAPI32.DLL",
        "MSVCRT.DLL",
        "USER32.DLL",
        "GDI32.DLL",
        "OLE32.DLL",
        "DDRAW.DLL",
        "DSOUND.DLL",
        "DINPUT.DLL",
        "WINMM.DLL",
        "GLIDE2X.DLL",
        "DPLAYX.DLL",
        "SHELL32.DLL",
        "COMCTL32.DLL",
        "COMDLG32.DLL",
        "WSOCK32.DLL",
        "NTDLL.DLL",
        "DMUSIC.DLL",
        "DMIME.DLL",
        "DMLOADER.DLL",
        "DMSTYLE.DLL",
        "DMBAND.DLL",
        "DMCOMPOS.DLL",
        "LZ32.DLL",
        "VERSION.DLL",
        "OLEAUT32.DLL",
        "ADVPACK.DLL",
        "WINSPOOL.DRV",
        "MPR.DLL",
        "D3D8.DLL",
        "IMAGEHLP.DLL",
        "WS2_32.DLL",
        "MSIMG32.DLL",
        "OPENGL32.DLL",
        "WOW32.DLL",
        "MSVFW32.DLL",
        "AVIFIL32.DLL",
        "DINPUT8.DLL",
        "MSACM32.DLL",
        "DCIMAN32.DLL",
        "WNASPI32.DLL",
        "QUARTZ.DLL",
        "AMSTREAM.DLL",
        "GLIDE3X.DLL",
        "DPLAY.DLL",
        "TAPI32.DLL",
        "IMM32.DLL",
        "GLU32.DLL",
        "OLEDLG.DLL",
        "OLEPRO32.DLL",
    ];

    #[test]
    fn dlls_are_only_appended() {
        let names: Vec<&str> = DLLS.iter().map(|dll| dll.name).collect();
        assert_eq!(names, ORDER, "a DLL inserted before the end moves every later stub id");
    }

    /// Two DLLs whose images overlap can't both be loaded: the second
    /// would be mapped over the first.
    #[test]
    fn dll_images_dont_overlap() {
        let version = super::super::version::WindowsVersion::Win98Se;
        let mut spans: Vec<(u32, u32, &str)> = (0..DLLS.len())
            .map(|i| {
                let size = image(i, version).headers.size_of_image;
                (DLLS[i].base, DLLS[i].base + size, DLLS[i].name)
            })
            .collect();
        spans.sort();
        for pair in spans.windows(2) {
            let ((_, end, a), (start, _, b)) = (pair[0], pair[1]);
            assert!(end <= start, "{a} ends at {end:08X}, past {b}'s base {start:08X}");
        }
    }

    /// MSVCRT's _ftol is code of its own (`Conv::Code`): its export points
    /// at the code, after the stubs, and its stub jumps there.
    #[test]
    fn a_function_given_as_code_is_exported_and_its_stub_jumps_to_it() {
        let msvcrt = find_dll("MSVCRT.DLL").unwrap();
        let dll = DLLS[msvcrt];
        let index = dll.apis.iter().position(|a| a.name == "_ftol").unwrap();
        let Conv::Code(code) = dll.apis[index].conv else {
            panic!("_ftol is a trap");
        };
        let image = image(msvcrt, super::super::version::WindowsVersion::Win98Se);
        let export = image.exports().unwrap().unwrap();
        let pe::ExportTarget::Rva(rva) = export.by_name("_ftol").unwrap().target else {
            panic!("_ftol is forwarded");
        };
        assert!(
            rva >= CODE_RVA + stubs_size(dll),
            "{rva:X} is among the stubs"
        );
        assert_eq!(&image.data[rva as usize..][..code.len()], code);
        let stub = (stub_address(msvcrt, index) - dll.base) as usize;
        assert_eq!(image.data[stub], 0xE9, "a JMP");
        let rel = u32::from_le_bytes(image.data[stub + 1..stub + 5].try_into().unwrap());
        assert_eq!(stub as u32 + 5 + rel, rva);
        // The variables start after it.
        assert!(data_rva(dll) >= rva + code.len() as u32);
    }

    #[test]
    fn stub_id_is_dll_index_and_function() {
        let dinput8 = ORDER.iter().position(|n| *n == "DINPUT8.DLL").unwrap();
        assert_eq!(id(dinput8, 26), (dinput8 as u16) << 10 | 26);
        assert_eq!(id(34, 26), 0x881A);
    }

    #[test]
    fn unknown_message_names_the_module_and_the_table_entry() {
        let text = unknown_message(0x881A, Some("DINPUT8.DLL"));
        assert!(text.contains("881Ah"), "{text}");
        assert!(text.contains("from DINPUT8.DLL"), "{text}");
        assert!(text.contains("DLL index 34 is WOW32.DLL"), "{text}");
        assert!(text.contains("no function 26"), "{text}");
        assert!(unknown_message(0x881A, None).contains("outside any module"));
    }
}
