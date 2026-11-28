//! STORAGE, 16-bit OLE 2's compound files (docfiles): StgCreateDocfile,
//! StgOpenStorage and StgIsStorageFile, and the IStorage, IStream and
//! IEnumSTATSTG objects a program gets from them.
//!
//! The docfiles are the built-in OLE32's (win32/ole32/storage.rs), which
//! reads and writes the compound file format: a 16-bit object here holds
//! an object of OLE32's, and each of its methods calls that object's
//! method with the arguments made 32-bit ones. The 16-bit interfaces
//! have the 32-bit ones' methods in the same order, and pass their
//! arguments in other forms:
//!
//! - The methods are far cdecl functions, the object's 16:16 pointer
//!   first (ole2.rs), here stubs of STORAGE that aren't exported.
//! - Pointers are 16:16; they are passed on as linear addresses.
//! - Names are ANSI text. A name is passed on as Unicode in a block of
//!   the process heap that goes after the call.
//! - A STATSTG has the same layout, but its name is ANSI text in a block
//!   of the 16-bit task allocator, which may be the program's own
//!   (`ole2::task_blocks`).
//! - An interface pointer the method gives is a 16-bit object of the
//!   32-bit object OLE32 gave.
//!
//! A 16-bit object is a block of the built-in task allocator: the 16:16
//! pointer to its vtable, the 32-bit object's pointer, and the vtable,
//! the addresses of its methods' stubs. QueryInterface gives the same
//! object, as OLE32's does, so one 16-bit object stands for one 32-bit
//! object, and it goes when the 32-bit one's last Release frees it.
//!
//! A target that a program implements itself, an IStorage of CopyTo and
//! MoveElementTo, is refused as OLE32 refuses one
//! (STG_E_INVALIDFUNCTION). IStream::CopyTo into a stream the program
//! implements fails with E_NOTIMPL, said in the log. The ILockBytes
//! functions and StgSetTimes, which OLE32 doesn't have either, fail with
//! E_NOTIMPL.

use super::builtin::{Def, cdecl, rust};
use super::ole2::{self, no_storage, not_implemented};
use crate::cpu::Cpu;
use crate::win32::com::{E_INVALIDARG, E_NOTIMPL, E_OUTOFMEMORY, S_OK};
use crate::win32::ole32::storage as stg;
use crate::win32::ole32::{task_alloc, task_free};

pub static STORAGE: Def = Def {
    name: "STORAGE",
    file: "STORAGE.DLL",
    apis: &[
        rust("StgCreateDocfile", 1, "lllp", stg_create_docfile),
        rust("StgCreateDocfileOnILockBytes", 2, "lllp", no_storage),
        rust("StgOpenStorage", 3, "lllllp", stg_open_storage),
        rust("StgOpenStorageOnILockBytes", 4, "lllllp", no_storage),
        rust("StgIsStorageFile", 5, "l", stg_is_storage_file),
        rust("StgIsStorageILockBytes", 6, "l", not_implemented),
        rust("StgSetTimes", 7, "pppp", not_implemented),
        // The methods of the objects, which aren't exported.
        cdecl(
            "IStorage16_QueryInterface",
            0,
            "lpp",
            storage_query_interface,
        ),
        cdecl("IStorage16_AddRef", 0, "l", storage_add_ref),
        cdecl("IStorage16_Release", 0, "l", storage_release),
        cdecl("IStorage16_CreateStream", 0, "llllll", create_stream),
        cdecl("IStorage16_OpenStream", 0, "lllllp", open_stream),
        cdecl("IStorage16_CreateStorage", 0, "llllll", create_storage),
        cdecl("IStorage16_OpenStorage", 0, "llllllp", open_storage),
        cdecl("IStorage16_CopyTo", 0, "lllll", copy_to),
        cdecl("IStorage16_MoveElementTo", 0, "lllll", move_element_to),
        cdecl("IStorage16_Commit", 0, "ll", commit),
        cdecl("IStorage16_Revert", 0, "l", revert),
        cdecl("IStorage16_EnumElements", 0, "llllp", enum_elements),
        cdecl("IStorage16_DestroyElement", 0, "ll", destroy_element),
        cdecl("IStorage16_RenameElement", 0, "lll", rename_element),
        cdecl("IStorage16_SetElementTimes", 0, "llppp", set_element_times),
        cdecl("IStorage16_SetClass", 0, "lp", set_class),
        cdecl("IStorage16_SetStateBits", 0, "lll", set_state_bits),
        cdecl("IStorage16_Stat", 0, "lll", storage_stat),
        cdecl("IStream16_QueryInterface", 0, "lpp", stream_query_interface),
        cdecl("IStream16_AddRef", 0, "l", stream_add_ref),
        cdecl("IStream16_Release", 0, "l", stream_release),
        cdecl("IStream16_Read", 0, "lplp", stream_read),
        cdecl("IStream16_Write", 0, "lplp", stream_write),
        cdecl("IStream16_Seek", 0, "llllp", stream_seek),
        cdecl("IStream16_SetSize", 0, "lll", stream_set_size),
        cdecl("IStream16_CopyTo", 0, "llllpp", stream_copy_to),
        cdecl("IStream16_Commit", 0, "ll", stream_nothing),
        cdecl("IStream16_Revert", 0, "l", stream_nothing),
        cdecl("IStream16_LockRegion", 0, "llllll", stream_lock_region),
        cdecl("IStream16_UnlockRegion", 0, "llllll", stream_lock_region),
        cdecl("IStream16_Stat", 0, "lll", stream_stat),
        cdecl("IStream16_Clone", 0, "lp", stream_clone),
        cdecl(
            "IEnumSTATSTG16_QueryInterface",
            0,
            "lpp",
            enum_query_interface,
        ),
        cdecl("IEnumSTATSTG16_AddRef", 0, "l", enum_add_ref),
        cdecl("IEnumSTATSTG16_Release", 0, "l", enum_release),
        cdecl("IEnumSTATSTG16_Next", 0, "lllp", enum_next),
        cdecl("IEnumSTATSTG16_Skip", 0, "ll", enum_skip),
        cdecl("IEnumSTATSTG16_Reset", 0, "l", enum_reset),
        cdecl("IEnumSTATSTG16_Clone", 0, "lp", enum_clone),
    ],
    library: true,
};

/// A function or method of OLE32's.
type Method = fn(&mut Cpu, &[u32]) -> Option<u32>;

/// The three kinds of object, and their methods' stubs in vtable order.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Storage,
    Stream,
    Enum,
}

const STORAGE_METHODS: [&str; 18] = [
    "IStorage16_QueryInterface",
    "IStorage16_AddRef",
    "IStorage16_Release",
    "IStorage16_CreateStream",
    "IStorage16_OpenStream",
    "IStorage16_CreateStorage",
    "IStorage16_OpenStorage",
    "IStorage16_CopyTo",
    "IStorage16_MoveElementTo",
    "IStorage16_Commit",
    "IStorage16_Revert",
    "IStorage16_EnumElements",
    "IStorage16_DestroyElement",
    "IStorage16_RenameElement",
    "IStorage16_SetElementTimes",
    "IStorage16_SetClass",
    "IStorage16_SetStateBits",
    "IStorage16_Stat",
];

const STREAM_METHODS: [&str; 14] = [
    "IStream16_QueryInterface",
    "IStream16_AddRef",
    "IStream16_Release",
    "IStream16_Read",
    "IStream16_Write",
    "IStream16_Seek",
    "IStream16_SetSize",
    "IStream16_CopyTo",
    "IStream16_Commit",
    "IStream16_Revert",
    "IStream16_LockRegion",
    "IStream16_UnlockRegion",
    "IStream16_Stat",
    "IStream16_Clone",
];

const ENUM_METHODS: [&str; 7] = [
    "IEnumSTATSTG16_QueryInterface",
    "IEnumSTATSTG16_AddRef",
    "IEnumSTATSTG16_Release",
    "IEnumSTATSTG16_Next",
    "IEnumSTATSTG16_Skip",
    "IEnumSTATSTG16_Reset",
    "IEnumSTATSTG16_Clone",
];

impl Kind {
    fn methods(self) -> &'static [&'static str] {
        match self {
            Kind::Storage => &STORAGE_METHODS,
            Kind::Stream => &STREAM_METHODS,
            Kind::Enum => &ENUM_METHODS,
        }
    }

    /// OLE32's Release of an object of this kind: what it returned.
    fn release32(self, cpu: &mut Cpu, this: u32) -> u32 {
        let args = [this];
        match self {
            Kind::Storage => stg::storage_release(cpu, &args),
            Kind::Stream => stg::stream_release(cpu, &args),
            Kind::Enum => stg::enum_release(cpu, &args),
        }
        .unwrap_or(0)
    }
}

const STG_E_INVALIDFUNCTION: u32 = stg::STG_E_INVALIDFUNCTION;
const STG_E_INVALIDPOINTER: u32 = stg::STG_E_INVALIDPOINTER;
const STATFLAG_NONAME: u32 = 1;
/// A STATSTG's size, the same as Win32's.
const STATSTG_SIZE: u32 = 72;
/// The longest name or path read from the program, in bytes.
const MAX_TEXT: usize = 1024;

// The 16-bit objects.

/// A new 16-bit object of the kind `kind` for OLE32's object `this`:
/// its 16:16 pointer, or the HRESULT to fail with. A STORAGE of a state
/// saved by a build before its objects has no stubs for their methods
/// (`builtin::stub_offset`), and a vtable of NULLs would end the program
/// at its first call: E_NOTIMPL then, as that build answered.
fn wrap(cpu: &mut Cpu, this: u32, kind: Kind) -> Result<u32, u32> {
    let module = super::builtin::find("STORAGE").ok_or(E_NOTIMPL)?;
    let methods = kind.methods();
    let stubs: Option<Vec<u32>> = methods
        .iter()
        .map(|name| super::builtin::stub_address(cpu, module, name))
        .collect();
    let Some(stubs) = stubs else {
        cpu.bus.log_string(
            "[W16] STORAGE: the module of a state saved by an older build has no \
             stubs for its objects' methods: E_NOTIMPL",
        );
        return Err(E_NOTIMPL);
    };
    let p = ole2::alloc(cpu, 8 + 4 * methods.len() as u32);
    let base = super::linear(cpu, p);
    if base == 0 {
        return Err(E_OUTOFMEMORY);
    }
    super::write_32(cpu, base, p + 8);
    super::write_32(cpu, base + 4, this);
    for (i, stub) in stubs.into_iter().enumerate() {
        super::write_32(cpu, base + 8 + 4 * i as u32, stub);
    }
    Ok(p)
}

/// OLE32's object of the 16-bit object `this`, if `this` is one of these
/// of `kind`.
fn target(cpu: &mut Cpu, this: u32, kind: Kind) -> Option<u32> {
    let module = super::builtin::find("STORAGE")?;
    let at = super::linear(cpu, this);
    if at == 0 {
        return None;
    }
    let vtable = super::linear(cpu, super::read_32(cpu, at));
    if vtable == 0 {
        return None;
    }
    let first = super::builtin::stub_address(cpu, module, kind.methods()[0])?;
    if super::read_32(cpu, vtable) != first {
        return None;
    }
    Some(super::read_32(cpu, at + 4)).filter(|&p| p != 0)
}

/// OLE32's method gave `result`, and on S_OK its object at the linear
/// `out`: put a 16-bit object of it there instead.
fn give(cpu: &mut Cpu, result: Option<u32>, out: u32, kind: Kind) -> Option<u32> {
    let result = result?;
    if result != S_OK || out == 0 {
        return Some(result);
    }
    let this = super::read_32(cpu, out);
    match wrap(cpu, this, kind) {
        Ok(p) => {
            super::write_32(cpu, out, p);
            Some(S_OK)
        }
        Err(e) => {
            super::write_32(cpu, out, 0);
            kind.release32(cpu, this);
            Some(e)
        }
    }
}

/// The ANSI text at the 16:16 `p` in Unicode, in a block of the process
/// heap for OLE32 to read: its address, 0 for a NULL `p`.
fn wide(cpu: &mut Cpu, p: u32) -> Result<u32, u32> {
    if p == 0 {
        return Ok(0);
    }
    let at = super::linear(cpu, p);
    if at == 0 {
        return Err(STG_E_INVALIDPOINTER);
    }
    let bytes = super::string_bytes(cpu, at, MAX_TEXT);
    let units = crate::win32::kernel32::ansi_to_wide(cpu, &bytes);
    let block = task_alloc(cpu, 2 * units.len() as u32 + 2);
    if block == 0 {
        return Err(E_OUTOFMEMORY);
    }
    for (i, &c) in units.iter().chain(&[0]).enumerate() {
        cpu.bus.guest_write_16(block + 2 * i as u32, c);
    }
    Ok(block)
}

/// Call OLE32's `f` with the names at the 16:16 `names` made Unicode,
/// in `args` at the places `at` says, then free them.
fn with_names(cpu: &mut Cpu, names: &[(usize, u32)], mut args: Vec<u32>, f: Method) -> Option<u32> {
    let mut made = Vec::new();
    for &(at, p) in names {
        match wide(cpu, p) {
            Ok(block) => {
                args[at] = block;
                made.push(block);
            }
            Err(e) => {
                for block in made {
                    task_free(cpu, block);
                }
                return Some(e);
            }
        }
    }
    let result = f(cpu, &args);
    for block in made {
        task_free(cpu, block);
    }
    result
}

/// OLE32 wrote STATSTGs at the linear addresses of `stats`, each with
/// its 16:16 address: their names, Unicode in blocks of the process
/// heap, go to the program as ANSI text in blocks of its task allocator,
/// then `value` returns.
fn names_to_program(cpu: &mut Cpu, value: u32, stats: &[(u32, u32)]) -> Option<u32> {
    let mut blocks = Vec::new();
    for &(p, at) in stats {
        let name = cpu.bus.guest_read_32(at);
        if name == 0 {
            continue;
        }
        let mut units = Vec::new();
        while units.len() < MAX_TEXT {
            let c = cpu.bus.guest_read_16(name + 2 * units.len() as u32);
            if c == 0 {
                break;
            }
            units.push(c);
        }
        task_free(cpu, name);
        cpu.bus.guest_write_32(at, 0);
        let mut text = crate::win32::kernel32::wide_to_ansi(cpu, &units);
        text.push(0);
        blocks.push((p, text));
    }
    ole2::task_blocks(cpu, value, &blocks)
}

/// Stat of IStorage and IStream: (this, STATSTG (16:16), flag).
fn stat(cpu: &mut Cpu, args: &[u32], kind: Kind) -> Option<u32> {
    let &[this, out, flag] = args else {
        unreachable!()
    };
    let Some(this) = target(cpu, this, kind) else {
        return Some(E_INVALIDARG);
    };
    let at = super::linear(cpu, out);
    let f = match kind {
        Kind::Storage => stg::storage_stat,
        _ => stg::stream_stat,
    };
    let result = f(cpu, &[this, at, flag])?;
    if result != S_OK || flag & STATFLAG_NONAME != 0 {
        return Some(result);
    }
    names_to_program(cpu, result, &[(out, at)])
}

// IUnknown of the three.

fn query(cpu: &mut Cpu, args: &[u32], kind: Kind) -> Option<u32> {
    let &[this16, iid, out] = args else {
        unreachable!()
    };
    let Some(this) = target(cpu, this16, kind) else {
        return Some(E_INVALIDARG);
    };
    let f = match kind {
        Kind::Storage => stg::storage_query_interface,
        Kind::Stream => stg::stream_query_interface,
        Kind::Enum => stg::enum_query_interface,
    };
    let result = f(cpu, &[this, iid, out])?;
    if result == S_OK {
        super::write_32(cpu, out, this16);
    }
    Some(result)
}

fn add_ref(cpu: &mut Cpu, args: &[u32], kind: Kind) -> Option<u32> {
    let Some(this) = target(cpu, args[0], kind) else {
        return Some(0);
    };
    let f = match kind {
        Kind::Storage => stg::storage_add_ref,
        Kind::Stream => stg::stream_add_ref,
        Kind::Enum => stg::enum_add_ref,
    };
    f(cpu, &[this])
}

/// Release: OLE32's object's, and the 16-bit object goes when that one
/// went. The value is what OLE32's Release returned.
fn release(cpu: &mut Cpu, args: &[u32], kind: Kind) -> Option<u32> {
    let this16 = args[0];
    let Some(this) = target(cpu, this16, kind) else {
        return Some(0);
    };
    let refs = kind.release32(cpu, this);
    if !stg::is_object(cpu, this) {
        // Its vtable no longer, so a call through a pointer the program
        // kept fails as one to an object that is gone.
        let at = super::linear(cpu, this16);
        super::write_32(cpu, at, 0);
        ole2::free(cpu, this16);
    }
    Some(refs)
}

fn storage_query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    query(cpu, args, Kind::Storage)
}

fn storage_add_ref(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    add_ref(cpu, args, Kind::Storage)
}

fn storage_release(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    release(cpu, args, Kind::Storage)
}

fn stream_query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    query(cpu, args, Kind::Stream)
}

fn stream_add_ref(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    add_ref(cpu, args, Kind::Stream)
}

fn stream_release(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    release(cpu, args, Kind::Stream)
}

fn enum_query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    query(cpu, args, Kind::Enum)
}

fn enum_add_ref(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    add_ref(cpu, args, Kind::Enum)
}

fn enum_release(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    release(cpu, args, Kind::Enum)
}

// The API.

/// Where a function or method puts the object it gives, a linear
/// address: NULL there first, as OLE32 puts it.
fn clear_out(cpu: &mut Cpu, out: u32) {
    if out != 0 {
        super::write_32(cpu, out, 0);
    }
}

/// StgCreateDocfile(name, mode, reserved, out): a new docfile in the
/// file `name`, or in memory for a NULL name.
fn stg_create_docfile(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[name, mode, reserved, out] = args else {
        unreachable!()
    };
    clear_out(cpu, out);
    let result = with_names(
        cpu,
        &[(0, name)],
        vec![0, mode, reserved, out],
        stg::stg_create_docfile,
    );
    give(cpu, result, out, Kind::Storage)
}

/// StgOpenStorage(name, priority, mode, excluded, reserved, out): the
/// docfile in the file `name`. OLE32 takes no priority storage or
/// excluded names either.
fn stg_open_storage(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[name, _, mode, _, reserved, out] = args else {
        unreachable!()
    };
    clear_out(cpu, out);
    let result = with_names(
        cpu,
        &[(0, name)],
        vec![0, 0, mode, 0, reserved, out],
        stg::stg_open_storage,
    );
    give(cpu, result, out, Kind::Storage)
}

/// StgIsStorageFile(name): S_OK for a compound file, S_FALSE for another.
fn stg_is_storage_file(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    with_names(cpu, &[(0, args[0])], vec![0], stg::stg_is_storage_file)
}

// IStorage.

/// The method `f` of OLE32's storage of `args[0]`, with `args` after it.
fn on_storage(cpu: &mut Cpu, args: &[u32], f: Method) -> Option<u32> {
    let Some(this) = target(cpu, args[0], Kind::Storage) else {
        return Some(E_INVALIDARG);
    };
    let mut all = vec![this];
    all.extend_from_slice(&args[1..]);
    f(cpu, &all)
}

/// CreateStream and CreateStorage(this, name, mode, reserved, reserved,
/// out).
fn create_element(cpu: &mut Cpu, args: &[u32], storage: bool) -> Option<u32> {
    let &[this, name, mode, r1, r2, out] = args else {
        unreachable!()
    };
    let out = super::linear(cpu, out);
    clear_out(cpu, out);
    let Some(this) = target(cpu, this, Kind::Storage) else {
        return Some(E_INVALIDARG);
    };
    let (f, kind): (Method, Kind) = if storage {
        (stg::create_storage, Kind::Storage)
    } else {
        (stg::create_stream, Kind::Stream)
    };
    let result = with_names(cpu, &[(1, name)], vec![this, 0, mode, r1, r2, out], f);
    give(cpu, result, out, kind)
}

fn create_stream(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    create_element(cpu, args, false)
}

fn create_storage(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    create_element(cpu, args, true)
}

/// OpenStream(this, name, reserved, mode, reserved, out).
fn open_stream(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, name, _, mode, r2, out] = args else {
        unreachable!()
    };
    clear_out(cpu, out);
    let Some(this) = target(cpu, this, Kind::Storage) else {
        return Some(E_INVALIDARG);
    };
    let result = with_names(
        cpu,
        &[(1, name)],
        vec![this, 0, 0, mode, r2, out],
        stg::open_stream,
    );
    give(cpu, result, out, Kind::Stream)
}

/// OpenStorage(this, name, priority, mode, excluded, reserved, out).
fn open_storage(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, name, _, mode, _, reserved, out] = args else {
        unreachable!()
    };
    clear_out(cpu, out);
    let Some(this) = target(cpu, this, Kind::Storage) else {
        return Some(E_INVALIDARG);
    };
    let result = with_names(
        cpu,
        &[(1, name)],
        vec![this, 0, 0, mode, 0, reserved, out],
        stg::open_storage,
    );
    give(cpu, result, out, Kind::Storage)
}

/// CopyTo(this, excluded IIDs count, IIDs, excluded names, target).
fn copy_to(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, count, _, _, to] = args else {
        unreachable!()
    };
    let Some(this) = target(cpu, this, Kind::Storage) else {
        return Some(E_INVALIDARG);
    };
    if to == 0 {
        return Some(STG_E_INVALIDPOINTER);
    }
    let Some(to) = target(cpu, to, Kind::Storage) else {
        return Some(STG_E_INVALIDFUNCTION);
    };
    stg::copy_to(cpu, &[this, count, 0, 0, to])
}

/// MoveElementTo(this, name, target, new name, flags).
fn move_element_to(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, name, to, new_name, flags] = args else {
        unreachable!()
    };
    let Some(this) = target(cpu, this, Kind::Storage) else {
        return Some(E_INVALIDARG);
    };
    let Some(to) = target(cpu, to, Kind::Storage) else {
        return Some(STG_E_INVALIDFUNCTION);
    };
    with_names(
        cpu,
        &[(1, name), (3, new_name)],
        vec![this, 0, to, 0, flags],
        stg::move_element_to,
    )
}

fn commit(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    on_storage(cpu, args, stg::commit)
}

fn revert(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    on_storage(cpu, args, stg::revert)
}

/// EnumElements(this, reserved, reserved, reserved, out).
fn enum_elements(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, r1, _, r3, out] = args else {
        unreachable!()
    };
    clear_out(cpu, out);
    let result = on_storage(cpu, &[this, r1, 0, r3, out], stg::enum_elements);
    give(cpu, result, out, Kind::Enum)
}

/// DestroyElement(this, name).
fn destroy_element(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(this) = target(cpu, args[0], Kind::Storage) else {
        return Some(E_INVALIDARG);
    };
    with_names(cpu, &[(1, args[1])], vec![this, 0], stg::destroy_element)
}

/// RenameElement(this, old, new).
fn rename_element(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(this) = target(cpu, args[0], Kind::Storage) else {
        return Some(E_INVALIDARG);
    };
    with_names(
        cpu,
        &[(1, args[1]), (2, args[2])],
        vec![this, 0, 0],
        stg::rename_element,
    )
}

/// SetElementTimes(this, name, created, accessed, modified): a NULL
/// name is the storage's own.
fn set_element_times(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, name, created, accessed, modified] = args else {
        unreachable!()
    };
    let Some(this) = target(cpu, this, Kind::Storage) else {
        return Some(E_INVALIDARG);
    };
    with_names(
        cpu,
        &[(1, name)],
        vec![this, 0, created, accessed, modified],
        stg::set_element_times,
    )
}

fn set_class(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    on_storage(cpu, args, stg::set_class)
}

fn set_state_bits(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    on_storage(cpu, args, stg::set_state_bits)
}

fn storage_stat(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    stat(cpu, args, Kind::Storage)
}

// IStream.

/// The method `f` of OLE32's stream of `args[0]`, with `args` after it.
fn on_stream(cpu: &mut Cpu, args: &[u32], f: Method) -> Option<u32> {
    let Some(this) = target(cpu, args[0], Kind::Stream) else {
        return Some(E_INVALIDARG);
    };
    let mut all = vec![this];
    all.extend_from_slice(&args[1..]);
    f(cpu, &all)
}

fn stream_read(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    on_stream(cpu, args, stg::stream_read)
}

fn stream_write(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    on_stream(cpu, args, stg::stream_write)
}

fn stream_seek(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    on_stream(cpu, args, stg::stream_seek)
}

fn stream_set_size(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    on_stream(cpu, args, stg::stream_set_size)
}

/// CopyTo(this, target, count (64 bits), read, written): into another
/// stream of these. One the program implements would need its Write
/// called with the bytes in a block of its own, which isn't done.
fn stream_copy_to(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, to, low, high, read, written] = args else {
        unreachable!()
    };
    let Some(this) = target(cpu, this, Kind::Stream) else {
        return Some(E_INVALIDARG);
    };
    if to == 0 {
        return Some(STG_E_INVALIDPOINTER);
    }
    let Some(to) = target(cpu, to, Kind::Stream) else {
        cpu.bus.log_string(
            "[W16] STORAGE: IStream::CopyTo into a stream the program implements \
             isn't implemented: E_NOTIMPL",
        );
        return Some(E_NOTIMPL);
    };
    // OLE32 copies into a stream of its own that is reverted as into one
    // the program implements, with a call of its Write; here it fails as
    // that stream's methods do.
    match stg::stream_state(cpu, to) {
        S_OK => stg::stream_copy_to(cpu, &[this, to, low, high, read, written]),
        e => Some(e),
    }
}

fn stream_nothing(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    on_stream(cpu, &args[..1], stg::stream_nothing)
}

fn stream_lock_region(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    on_stream(cpu, args, stg::stream_lock_region)
}

fn stream_stat(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    stat(cpu, args, Kind::Stream)
}

/// Clone(this, out).
fn stream_clone(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let out = args[1];
    clear_out(cpu, out);
    let result = on_stream(cpu, args, stg::stream_clone);
    give(cpu, result, out, Kind::Stream)
}

// IEnumSTATSTG.

/// Next(this, count, STATSTGs (16:16), fetched).
fn enum_next(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, count, out, fetched] = args else {
        unreachable!()
    };
    let Some(this) = target(cpu, this, Kind::Enum) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 || (count != 1 && fetched == 0) {
        return Some(STG_E_INVALIDPOINTER);
    }
    // OLE32 writes its count where this function can read it.
    let got = task_alloc(cpu, 4);
    if got == 0 {
        return Some(E_OUTOFMEMORY);
    }
    cpu.bus.guest_write_32(got, 0);
    let at = super::linear(cpu, out);
    let result = stg::enum_next(cpu, &[this, count, at, got]);
    let n = cpu.bus.guest_read_32(got);
    task_free(cpu, got);
    let result = result?;
    if fetched != 0 {
        super::write_32(cpu, fetched, n);
    }
    let (sel, offset) = (out & 0xFFFF_0000, out & 0xFFFF);
    let stats: Vec<(u32, u32)> = (0..n)
        .map(|i| {
            let p = sel | (offset + i * STATSTG_SIZE) & 0xFFFF;
            (p, at + i * STATSTG_SIZE)
        })
        .collect();
    names_to_program(cpu, result, &stats)
}

fn enum_skip(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(this) = target(cpu, args[0], Kind::Enum) else {
        return Some(E_INVALIDARG);
    };
    stg::enum_skip(cpu, &[this, args[1]])
}

fn enum_reset(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(this) = target(cpu, args[0], Kind::Enum) else {
        return Some(E_INVALIDARG);
    };
    stg::enum_reset(cpu, &[this])
}

/// Clone(this, out).
fn enum_clone(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let out = args[1];
    clear_out(cpu, out);
    let Some(this) = target(cpu, args[0], Kind::Enum) else {
        return Some(E_INVALIDARG);
    };
    let result = stg::enum_clone(cpu, &[this, out]);
    give(cpu, result, out, Kind::Enum)
}
