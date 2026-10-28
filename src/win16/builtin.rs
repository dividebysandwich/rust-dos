//! The system's 16-bit modules: KERNEL, USER, GDI and the others, NE
//! images the emulator builds when the Win16 process starts and loads as
//! it loads any module (module.rs). Each has one code segment of stubs,
//! one for each function, of a service trap and the return that takes
//! the function's arguments off the stack:
//!
//! ```text
//! GetVersion:   FE 3E 03 00     service trap, function 0003h
//!               CB 90 90 90     retf      (no arguments)
//! IsWindow:     FE 3E 3E 04
//!               CA 02 00 90     retf 2    (Pascal, one word)
//! ```
//!
//! A function's id is its module's place in `MODULES` and its own in the
//! module's table. Its arguments, read from the 16-bit stack as its
//! signature says (`Api16::args`), go to Rust (`Run::Rust`), or go on to
//! the function of a built-in DLL that does what it does (`Run::Win32`),
//! translated: 16:16 pointers become flat ones, handles USER32's.

use super::thunk;
use crate::cpu::{Cpu, Seg};
use crate::dpmi::Context;
use rust_dos_ne as ne;

/// How a function takes its arguments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conv16 {
    /// Pushed first to last, taken off by the function.
    Pascal,
    /// Pushed last to first, left for the caller (wsprintf).
    Cdecl,
    /// In the registers, which it returns its results in (InitTask,
    /// DOS3Call).
    Register,
}

/// A function's Rust side: the arguments, converted as the signature
/// says; what it returns in DX:AX, or None for one that sets the
/// registers itself (or doesn't return).
pub type Handler16 = fn(&mut Cpu, &[u32]) -> Option<u32>;

#[derive(Clone, Copy)]
pub enum Run {
    Rust(Handler16),
    /// The built-in DLL's function that does the work, by the DLL's and
    /// its names, and what the 16-bit function returns (`ret` letters):
    /// w a word or long the 32-bit value's low part, b a BOOL, i an icon
    /// or cursor handle, m a menu handle, a an accelerator table's handle,
    /// n an instance handle.
    Win32(&'static str, &'static str, u8),
    /// A constant: its value (as __AHINCR is).
    Value(fn(&Cpu) -> u16),
}

/// A function of a system module.
///
/// The signature's letters, one for each argument, first to last:
/// w a word, s a signed word, l a long, p a 16:16 pointer the function
/// gets flat, S one it gets as it is, r a resource name (a 16:16
/// pointer, or a number when its selector is 0), i an icon or cursor
/// handle, m a menu handle, a an accelerator table's handle, n an
/// instance or module handle (the 32-bit functions get the image of its
/// resources), h a window to go after (HWND_TOPMOST and HWND_NOTOPMOST
/// widened to their 32-bit values), R a pointer to a RECT and P one to
/// a POINT, which the 32-bit functions get 32-bit copies of, written
/// back when they return.
pub struct Api16 {
    pub name: &'static str,
    /// Its ordinal, or 0 for the system's own stubs, which aren't
    /// exported.
    pub ordinal: u16,
    pub args: &'static str,
    pub conv: Conv16,
    pub run: Run,
}

pub const fn rust(name: &'static str, ordinal: u16, args: &'static str, h: Handler16) -> Api16 {
    Api16 {
        name,
        ordinal,
        args,
        conv: Conv16::Pascal,
        run: Run::Rust(h),
    }
}

pub const fn cdecl(name: &'static str, ordinal: u16, args: &'static str, h: Handler16) -> Api16 {
    Api16 {
        name,
        ordinal,
        args,
        conv: Conv16::Cdecl,
        run: Run::Rust(h),
    }
}

pub const fn register(name: &'static str, ordinal: u16, h: Handler16) -> Api16 {
    Api16 {
        name,
        ordinal,
        args: "",
        conv: Conv16::Register,
        run: Run::Rust(h),
    }
}

pub const fn win32(
    name: &'static str,
    ordinal: u16,
    args: &'static str,
    dll: &'static str,
    function: &'static str,
    ret: u8,
) -> Api16 {
    Api16 {
        name,
        ordinal,
        args,
        conv: Conv16::Pascal,
        run: Run::Win32(dll, function, ret),
    }
}

pub const fn value(name: &'static str, ordinal: u16, f: fn(&Cpu) -> u16) -> Api16 {
    Api16 {
        name,
        ordinal,
        args: "",
        conv: Conv16::Pascal,
        run: Run::Value(f),
    }
}

/// A system module.
pub struct Def {
    /// Its module name, and its file's name.
    pub name: &'static str,
    pub file: &'static str,
    pub apis: &'static [Api16],
    /// Whether it is a library (all of them but KERNEL, USER and GDI,
    /// which Windows 3.1 has as programs).
    pub library: bool,
}

/// The system modules. Append only (see the module's notes).
pub static MODULES: &[&Def] = &[
    &super::kernel::MODULE,
    &super::user::MODULE,
    &super::gdi::MODULE,
    &super::misc::KEYBOARD,
    &super::misc::SOUND,
    &super::misc::SYSTEM,
    &super::misc::WIN87EM,
    &super::misc::TOOLHELP,
    &super::misc::SHELL,
    &super::misc::COMMDLG,
    &super::misc::MMSYSTEM,
    &super::misc::LZEXPAND,
    &super::misc::VER,
    &super::misc::WING,
    &super::misc::DDEML,
    &super::misc::DISPLAY,
];

/// The place in `MODULES` of the module named `name` ("USER",
/// "USER.EXE", "user").
pub fn find(name: &str) -> Option<usize> {
    let name = name.trim();
    let base = name
        .rsplit(['\\', '/', ':'])
        .next()
        .unwrap_or(name)
        .split('.')
        .next()
        .unwrap_or(name);
    MODULES
        .iter()
        .position(|m| m.name.eq_ignore_ascii_case(base))
}

pub fn id(module: usize, api: usize) -> u16 {
    (module << 10 | api) as u16
}

/// Bytes each function's stub takes.
pub const STUB: u32 = 8;

/// The stubs at the end of KERNEL's code segment that imports nothing
/// has go to: one for each such import, calls of which end the program.
pub const MISSING: usize = 64;

/// The stub of `api`, whose id is `id`.
fn stub(id: u16, api: &Api16) -> [u8; STUB as usize] {
    let [lo, hi] = id.to_le_bytes();
    let mut code = [0x90; STUB as usize];
    code[..4].copy_from_slice(&[0xFE, crate::win32::TRAP, lo, hi]);
    let bytes = arg_bytes(api.args);
    if api.conv == Conv16::Pascal && bytes > 0 {
        let [lo, hi] = bytes.to_le_bytes();
        code[4..7].copy_from_slice(&[0xCA, lo, hi]);
    } else {
        code[4] = 0xCB;
    }
    code
}

/// The bytes of arguments the signature `args` says.
pub fn arg_bytes(args: &str) -> u16 {
    args.bytes()
        .map(|c| match c {
            b'l' | b'p' | b'S' | b'r' | b'R' | b'P' => 4,
            _ => 2,
        })
        .sum()
}

/// The NE image of the system module `index`: its code segment of stubs
/// and its entry table, the constants' values `cpu` has.
pub fn image(cpu: &Cpu, index: usize) -> Vec<u8> {
    let def = MODULES[index];
    let flags = if def.library {
        ne::LIBMODULE | ne::SINGLEDATA | ne::PROTMODE
    } else {
        ne::SINGLEDATA | ne::PROTMODE
    };
    let mut b = ne::Builder::new(def.name, flags);
    let mut code: Vec<u8> = def
        .apis
        .iter()
        .enumerate()
        .flat_map(|(i, api)| stub(id(index, i), api))
        .collect();
    if index == 0 {
        for k in 0..MISSING {
            let [lo, hi] = id(0, def.apis.len() + k).to_le_bytes();
            code.extend_from_slice(&[0xFE, crate::win32::TRAP, lo, hi, 0xCB, 0x90, 0x90, 0x90]);
        }
    }
    b.segment(ne::SegmentSpec {
        data: code,
        flags: ne::SEG_READONLY,
        alloc: 0,
        relocations: Vec::new(),
    });
    b.description(&format!("{} (built-in)", def.file));
    b.expected_version(0x030A);
    for (i, api) in def.apis.iter().enumerate() {
        if api.ordinal == 0 {
            continue;
        }
        let entry = match api.run {
            Run::Value(f) => ne::Entry::Constant {
                value: f(cpu),
                flags: ne::ENTRY_EXPORTED,
            },
            _ => ne::Entry::Code {
                segment: 1,
                offset: (i as u32 * STUB) as u16,
                flags: ne::ENTRY_EXPORTED,
                moveable: false,
            },
        };
        b.entry(api.ordinal, entry);
        b.export(api.name, api.ordinal, true);
    }
    b.build()
}

/// The module and function of the id `id`.
pub fn api(id: u16) -> Option<(&'static Def, &'static Api16)> {
    let def = MODULES.get((id >> 10) as usize)?;
    Some((def, def.apis.get((id & 0x3FF) as usize)?))
}

/// "USER.IsWindow", for the log and debuggers.
pub fn function_name(id: u16) -> Option<String> {
    let (def, api) = api(id)?;
    Some(format!("{}.{}", def.name, api.name))
}

/// The 16:16 address of the stub of the function `name` of the system
/// module `module`, once that is loaded.
pub fn stub_address(cpu: &Cpu, module: usize, name: &str) -> Option<u32> {
    let i = MODULES[module].apis.iter().position(|a| a.name == name)?;
    let sel = super::module::builtin_selector(cpu, module)?;
    Some(((sel as u32) << 16) | (i as u32 * STUB))
}

/// The 16:16 address of the stub that stands for the import `name`
/// nothing has: 0 when there are no more of them.
pub fn missing_address(cpu: &mut Cpu, name: String) -> u32 {
    let missing = &mut cpu.bus.win32.win16.missing;
    let k = match missing.iter().position(|m| *m == name) {
        Some(k) => k,
        None => {
            missing.push(name);
            missing.len() - 1
        }
    };
    if k >= MISSING {
        return 0;
    }
    let Some(sel) = super::module::builtin_selector(cpu, 0) else {
        return 0;
    };
    ((sel as u32) << 16) | ((MODULES[0].apis.len() + k) as u32 * STUB)
}

/// Where 16-bit code 32-bit code called returns to: KERNEL's
/// `CallReturn16`.
pub fn call_return16(cpu: &Cpu) -> u32 {
    stub_address(cpu, 0, "CallReturn16").unwrap_or(0)
}

/// The trap id of KERNEL's `Thunk16`, which the 16-bit slots have.
pub fn thunk16_id() -> u16 {
    let i = MODULES[0]
        .apis
        .iter()
        .position(|a| a.name == "Thunk16")
        .expect("Thunk16");
    id(0, i)
}

/// The address of the stub of the 32-bit function `function` of the
/// built-in DLL `dll`.
pub fn win32_address(dll: &str, function: &str) -> Option<u32> {
    let index = crate::win32::stubs::find_dll(dll)?;
    let api = crate::win32::stubs::DLLS[index]
        .apis
        .iter()
        .position(|a| a.name == function)?;
    Some(crate::win32::stubs::stub_address(index, api))
}

/// Read the arguments of a function with the signature `args` from the
/// 16-bit stack, as the caller pushed them: the first at the highest
/// address for Pascal's convention, at the lowest for C's. Each one as it
/// is (the 16:16 pointers too).
pub fn read_args(cpu: &mut Cpu, args: &str, conv: Conv16) -> Vec<u32> {
    let base = cpu.seg_cache(Seg::SS).base;
    let sp = cpu.sp() as u32;
    let sizes: Vec<u32> = args.bytes().map(arg_bytes_of).collect();
    let mut values = vec![0u32; sizes.len()];
    let mut at = sp + 4;
    let order: Vec<usize> = match conv {
        Conv16::Cdecl => (0..sizes.len()).collect(),
        _ => (0..sizes.len()).rev().collect(),
    };
    for i in order {
        let lin = base.wrapping_add(at & 0xFFFF);
        values[i] = if sizes[i] == 4 {
            cpu.bus.guest_read_32(lin)
        } else {
            cpu.bus.guest_read_16(lin) as u32
        };
        at += sizes[i];
    }
    values
}

fn arg_bytes_of(c: u8) -> u32 {
    arg_bytes(std::str::from_utf8(&[c]).unwrap_or("w")) as u32
}

/// Run the function the trap with id `id` is the stub of, called by 16-bit
/// code: its far return address on the stack, its arguments above.
pub fn call(cpu: &mut Cpu, id: u16) {
    let kernel = MODULES[0].apis.len();
    if id >> 10 == 0 && (id & 0x3FF) as usize >= kernel {
        let k = (id & 0x3FF) as usize - kernel;
        let name = cpu
            .bus
            .win32
            .win16
            .missing
            .get(k)
            .cloned()
            .unwrap_or_default();
        let what = format!(
            "a call of {}, which the built-in Windows doesn't have yet",
            name
        );
        crate::win32::fatal(cpu, &what, None);
        return;
    }
    let Some((def, api)) = api(id) else {
        crate::win32::fatal(
            cpu,
            &format!("a call of the unknown 16-bit function {:04X}h", id),
            None,
        );
        return;
    };
    cpu.set_eip(cpu.eip().wrapping_add(4));
    let raw = read_args(cpu, api.args, api.conv);
    if cpu.bus.guest_faulted() {
        return;
    }
    let logs = crate::win32::trace::on(cpu);
    let caller = if logs {
        let base = cpu.seg_cache(Seg::SS).base;
        cpu.bus.guest_read_32(base.wrapping_add(cpu.sp() as u32))
    } else {
        0
    };
    let result = match api.run {
        Run::Rust(handler) => {
            let args = convert_plain(cpu, api.args, &raw);
            handler(cpu, &args)
        }
        Run::Win32(dll, function, ret) => {
            match win32_address(dll, function) {
                Some(target) => auto_thunk(cpu, target, api.args, &raw, ret),
                None => {
                    crate::win32::fatal(
                        cpu,
                        &format!(
                            "{}.{} goes on to {}.{}, which isn't there",
                            def.name, api.name, dll, function
                        ),
                        None,
                    );
                }
            }
            None
        }
        Run::Value(f) => Some(f(cpu) as u32),
    };
    crate::win32::object::close_files(&mut cpu.bus);
    if cpu.bus.guest_faulted() {
        return;
    }
    if let Some(value) = result {
        cpu.set_ax(value as u16);
        cpu.set_dx((value >> 16) as u16);
    }
    let call = crate::win32::trace::Call {
        kind: crate::win32::trace::Kind::W16,
        caller,
        id,
        module: def.name,
        name: api.name,
    };
    if logs && crate::win32::trace::admit(cpu, call) {
        let args: Vec<String> = raw.iter().map(|a| format!("{:X}", a)).collect();
        let result = result.map_or("...".to_string(), |r| format!("{:X}", r));
        cpu.bus.log_string(&format!(
            "[W16] {:04X}:{:04X}: {}.{}({}) = {}",
            caller >> 16,
            caller & 0xFFFF,
            def.name,
            api.name,
            args.join(", "),
            result
        ));
    }
}

/// The arguments for a Rust handler: the pointers flat, handles as they
/// are, signed words widened.
fn convert_plain(cpu: &Cpu, args: &str, raw: &[u32]) -> Vec<u32> {
    args.bytes()
        .zip(raw)
        .map(|(c, &v)| match c {
            b's' => v as u16 as i16 as i32 as u32,
            b'p' | b'R' | b'P' => super::linear(cpu, v),
            b'r' => {
                if v >> 16 == 0 {
                    v
                } else {
                    super::linear(cpu, v)
                }
            }
            _ => v,
        })
        .collect()
}

/// The structures a 32-bit function gets copies of, to write back.
const COPY_RECT: u32 = 1;
const COPY_POINT: u32 = 2;

/// Call the 32-bit function at `target` for a 16-bit function with the
/// signature `args`, called with `raw`.
fn auto_thunk(cpu: &mut Cpu, target: u32, args: &str, raw: &[u32], ret: u8) {
    let copies = args.bytes().filter(|c| matches!(c, b'R' | b'P')).count() as u32;
    let top = thunk::scratch32(cpu, 16 * copies);
    let mut scratch = top;
    let mut data = vec![ret as u32];
    let mut out = Vec::new();
    for (c, &v) in args.bytes().zip(raw) {
        out.push(match c {
            b's' => v as u16 as i16 as i32 as u32,
            b'p' => super::linear(cpu, v),
            b'r' => {
                if v >> 16 == 0 {
                    v
                } else {
                    super::linear(cpu, v)
                }
            }
            b'i' => super::handles::icon32(v as u16),
            b'm' => super::handles::menu32(v as u16),
            b'a' => super::handles::accel32(v as u16),
            b'n' => super::handles::instance32(cpu, v as u16),
            b'h' if v as u16 >= 0xFFFE => v as u16 as i16 as i32 as u32,
            b'R' | b'P' => {
                let lin = super::linear(cpu, v);
                if lin == 0 {
                    0
                } else {
                    let words = if c == b'R' { 4 } else { 2 };
                    for k in 0..words {
                        let w = cpu.bus.guest_read_16(lin + 2 * k) as i16 as i32 as u32;
                        cpu.bus.guest_write_32(scratch + 4 * k, w);
                    }
                    data.extend_from_slice(&[
                        if c == b'R' { COPY_RECT } else { COPY_POINT },
                        lin,
                        scratch,
                    ]);
                    let at = scratch;
                    scratch += 16;
                    at
                }
            }
            _ => v,
        });
    }
    let ctx = Context::of(cpu);
    thunk::enter32_at(cpu, ctx, top, target, &out, thunk::POST_AUTO, data);
}

/// An automatic thunk's 32-bit function returned `value`: the copies
/// written back, the value converted as `data[0]` says.
pub fn post_auto(cpu: &mut Cpu, value: u32, data: &[u32], _: &mut Context) -> Option<u32> {
    // Last first: where an output (which comes first, as in IntersectRect)
    // is an input too, the result is what stays.
    for copy in data[1..].as_chunks::<3>().0.iter().rev() {
        let words = if copy[0] == COPY_RECT { 4 } else { 2 };
        for k in 0..words {
            let v = cpu.bus.guest_read_32(copy[2] + 4 * k);
            cpu.bus.guest_write_16(copy[1] + 2 * k, v as u16);
        }
    }
    Some(match data[0] as u8 {
        b'i' => super::handles::icon16(value) as u32,
        b'm' => super::handles::menu16(value) as u32,
        b'a' => super::handles::accel16(value) as u32,
        b'n' => super::handles::instance16(cpu, value) as u32,
        b'b' => (value != 0) as u32,
        _ => value,
    })
}
