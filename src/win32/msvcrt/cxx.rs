//! The C++ classes of Visual C++ 6's MSVCRT.DLL and its run-time type
//! information: `exception` and the exceptions the runtime throws itself
//! (`bad_cast`, `bad_typeid`, `__non_rtti_object`), `type_info`, and the
//! functions `dynamic_cast` and `typeid` compile to (`__RTDynamicCast`,
//! `__RTtypeid`, `__RTCastToVoid`), with `__unDName`, the undecorator
//! `type_info::name` uses (undname.rs).
//!
//! Members are thiscall: `this` in ECX, their arguments on the stack,
//! which they take off. An exception object is three words, as MSVCRT's:
//! the vtable, the message and whether the object owns the message
//! (allocated with `operator new`, freed by the destructor). Every
//! destructor of the four classes is `exception`'s, which sets the
//! vtable back to `exception`'s; assignment destroys and copies, so it
//! leaves `exception`'s vtable too. The program's own classes derived
//! from them keep their own vtables, which their own code sets.
//!
//! The vtables are exported as data (`??_7exception@@6B@` and the
//! others), each after a word that points to its class's complete object
//! locator, as the compiler lays out a class's vtable. The type
//! descriptors, class hierarchies and throw information they and the
//! runtime's throws refer to are in the DLL's variable `rtti` (`Rtti`),
//! laid out as Visual C++ 6 lays them out. The exceptions the runtime
//! throws have their object below the stub's return address, where the
//! real runtime's own frame has it, and are raised from there as
//! `_CxxThrowException` raises them.

use super::call::{self, Call};
use super::{
    C_UNDNAME, Writer, cstr, cstr_n, dll_index, heap_alloc, heap_free, put, put_cstr, read32,
    var_at, write32,
};
use crate::cpu::Cpu;
use crate::win32::kernel32::virtual_memory::allows;

/// The classes, by their place in `CLASSES`.
const TYPE_INFO: usize = 0;
const EXCEPTION: usize = 1;
const BAD_CAST: usize = 2;
const BAD_TYPEID: usize = 3;
const NON_RTTI: usize = 4;

struct Class {
    /// The decorated name in its type descriptor.
    name: &'static [u8],
    /// The class and its bases, the class first, as its class hierarchy
    /// lists them (all single inheritance).
    hierarchy: &'static [usize],
    /// The functions in its vtable, by name in `apis`.
    vtable: &'static [&'static str],
    /// Its copy constructor, which a catch by value calls.
    copy: &'static str,
    /// The variables of the word before its vtable and of the vtable.
    header_var: usize,
    vtable_var: usize,
}

static CLASSES: [Class; 5] = [
    Class {
        name: b".?AVtype_info@@",
        hierarchy: &[TYPE_INFO],
        vtable: &["??_Gtype_info@@UAEPAXI@Z"],
        copy: "",
        header_var: super::V_TYPE_INFO_COL,
        vtable_var: super::V_TYPE_INFO_VTABLE,
    },
    Class {
        name: b".?AVexception@@",
        hierarchy: &[EXCEPTION],
        vtable: &["??_Eexception@@UAEPAXI@Z", "?what@exception@@UBEPBDXZ"],
        copy: "??0exception@@QAE@ABV0@@Z",
        header_var: super::V_EXCEPTION_COL,
        vtable_var: super::V_EXCEPTION_VTABLE,
    },
    Class {
        name: b".?AVbad_cast@@",
        hierarchy: &[BAD_CAST, EXCEPTION],
        vtable: &["??_Ebad_cast@@UAEPAXI@Z", "?what@exception@@UBEPBDXZ"],
        copy: "??0bad_cast@@QAE@ABV0@@Z",
        header_var: super::V_BAD_CAST_COL,
        vtable_var: super::V_BAD_CAST_VTABLE,
    },
    Class {
        name: b".?AVbad_typeid@@",
        hierarchy: &[BAD_TYPEID, EXCEPTION],
        vtable: &["??_Ebad_typeid@@UAEPAXI@Z", "?what@exception@@UBEPBDXZ"],
        copy: "??0bad_typeid@@QAE@ABV0@@Z",
        header_var: super::V_BAD_TYPEID_COL,
        vtable_var: super::V_BAD_TYPEID_VTABLE,
    },
    Class {
        name: b".?AV__non_rtti_object@@",
        hierarchy: &[NON_RTTI, BAD_TYPEID, EXCEPTION],
        vtable: &[
            "??_E__non_rtti_object@@UAEPAXI@Z",
            "?what@exception@@UBEPBDXZ",
        ],
        copy: "??0__non_rtti_object@@QAE@ABV0@@Z",
        header_var: super::V_NON_RTTI_COL,
        vtable_var: super::V_NON_RTTI_VTABLE,
    },
];

/// An exception object's size and fields.
const OBJECT_SIZE: u32 = 12;
const O_NAME: u32 = 4;
const O_DO_FREE: u32 = 8;

/// What `what()` gives for an exception with no message.
const UNKNOWN: &[u8] = b"Unknown exception";

/// The messages of the exceptions the runtime throws.
const BAD_DYNAMIC_CAST: &[u8] = b"Bad dynamic_cast!";
const TYPEID_OF_NULL: &[u8] = b"Attempted a typeid of NULL pointer!";
const BAD_READ_POINTER: &[u8] = b"Bad read pointer - no RTTI data!";
const ACCESS_VIOLATION: &[u8] = b"Access violation - no RTTI data!";

/// The bytes of the variable `rtti` before `init` fills it.
pub const RTTI_SIZE: u32 = 0x300;

/// Where the structures of each class are in `rtti`, at `base`, and
/// the bytes to put there.
struct Rtti {
    bytes: Vec<u8>,
    base: u32,
    /// Each class's type descriptor and complete object locator.
    td: [u32; 5],
    col: [u32; 5],
    /// The throw information of the classes the runtime throws (none for
    /// type_info and exception).
    throw: [u32; 5],
}

impl Rtti {
    fn at(&self) -> u32 {
        self.base + self.bytes.len() as u32
    }

    fn word(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn text(&mut self, text: &[u8]) {
        self.bytes.extend_from_slice(text);
        self.bytes.push(0);
        while !self.bytes.len().is_multiple_of(4) {
            self.bytes.push(0);
        }
    }

    /// The layout at `base`: the text what() gives for an exception with
    /// no message first, so that it is at `base`, then type descriptors,
    /// base class descriptors, class hierarchies, complete object
    /// locators, catchable types, their arrays and the throw information,
    /// as Visual C++ 6 makes them. A pointer to member's displacements
    /// are (0, -1, 0): the classes are bases at offset 0, none of them
    /// virtual.
    fn new(base: u32) -> Rtti {
        let mut r = Rtti {
            bytes: Vec::new(),
            base,
            td: [0; 5],
            col: [0; 5],
            throw: [0; 5],
        };
        r.text(UNKNOWN);
        let type_info_vtable = var_at(CLASSES[TYPE_INFO].vtable_var);
        for (i, class) in CLASSES.iter().enumerate() {
            r.td[i] = r.at();
            r.word(type_info_vtable);
            r.word(0);
            r.text(class.name);
        }
        let mut bcd = [0; 5];
        for (i, class) in CLASSES.iter().enumerate() {
            bcd[i] = r.at();
            r.word(r.td[i]);
            r.word(class.hierarchy.len() as u32 - 1);
            r.word(0);
            r.word(u32::MAX);
            r.word(0);
            r.word(0);
        }
        for (i, class) in CLASSES.iter().enumerate() {
            let array = r.at();
            for &base in class.hierarchy {
                r.word(bcd[base]);
            }
            let hierarchy = r.at();
            r.word(0);
            r.word(0);
            r.word(class.hierarchy.len() as u32);
            r.word(array);
            r.col[i] = r.at();
            r.word(0);
            r.word(0);
            r.word(0);
            r.word(r.td[i]);
            r.word(hierarchy);
        }
        let mut catchable = [0; 5];
        for (i, class) in CLASSES.iter().enumerate().skip(EXCEPTION) {
            catchable[i] = r.at();
            r.word(0);
            r.word(r.td[i]);
            r.word(0);
            r.word(u32::MAX);
            r.word(0);
            r.word(OBJECT_SIZE);
            r.word(api_address(class.copy));
        }
        for i in [BAD_CAST, BAD_TYPEID, NON_RTTI] {
            let array = r.at();
            let hierarchy = CLASSES[i].hierarchy;
            r.word(hierarchy.len() as u32);
            for &base in hierarchy {
                r.word(catchable[base]);
            }
            r.throw[i] = r.at();
            r.word(0);
            r.word(api_address("??1exception@@UAE@XZ"));
            r.word(0);
            r.word(array);
        }
        r
    }
}

/// The address of the stub of the function `name` of MSVCRT.
fn api_address(name: &str) -> u32 {
    let index = super::apis::APIS
        .iter()
        .position(|api| api.name == name)
        .unwrap_or_else(|| panic!("MSVCRT has no {}", name));
    super::super::stubs::stub_address(dll_index(), index)
}

fn vtable(class: usize) -> u32 {
    var_at(CLASSES[class].vtable_var)
}

/// Fill the classes' structures and vtables for a process (`init`).
pub(super) fn init(w: &mut Writer) -> Option<()> {
    let rtti = Rtti::new(var_at(super::V_RTTI));
    debug_assert!(rtti.bytes.len() as u32 <= RTTI_SIZE);
    w.bytes(rtti.base, &rtti.bytes)?;
    for (i, class) in CLASSES.iter().enumerate() {
        w.u32(var_at(class.header_var) + 4, rtti.col[i])?;
        for (j, name) in class.vtable.iter().enumerate() {
            w.u32(vtable(i) + 4 * j as u32, api_address(name))?;
        }
    }
    Some(())
}

// The exception classes.

/// A copy of `text` and a 0 from the runtime's heap, for an object that
/// owns its message; 0 when there is no memory for it.
fn new_text(cpu: &mut Cpu, text: &[u8]) -> u32 {
    match heap_alloc(cpu, text.len() as u32 + 1) {
        Some(to) if put_cstr(cpu, to, text).is_some() => to,
        _ => 0,
    }
}

/// Set up the object `this` of the class `class`: its vtable, its
/// message and whether it owns it. None, having changed nothing, if a
/// page of it isn't there.
fn set_object(cpu: &mut Cpu, this: u32, class: usize, name: u32, do_free: u32) -> Option<()> {
    let mut data = Vec::with_capacity(OBJECT_SIZE as usize);
    for word in [vtable(class), name, do_free] {
        data.extend_from_slice(&word.to_le_bytes());
    }
    put(cpu, this, &data)
}

/// Whether the object `this` can be written: a constructor checks before
/// it takes memory for the message, so that a page fault, after which
/// the call runs again, leaves nothing behind.
fn writable(cpu: &mut Cpu, this: u32) -> bool {
    cpu.bus.guest_probe(this, OBJECT_SIZE as usize, true)
}

/// exception(const char *const &message) and the constructors that pass
/// a message on to it: the object owns a copy of the message.
fn construct_with(cpu: &mut Cpu, class: usize, message: u32) -> Option<u32> {
    let this = cpu.ecx();
    let text = cstr(cpu, message)?;
    if !writable(cpu, this) {
        return None;
    }
    let name = new_text(cpu, &text);
    set_object(cpu, this, class, name, 1)?;
    Some(this)
}

/// What a copy of the exception `other` takes from it: its message
/// pointer, whether it owns it, and the text when it does.
struct Source {
    name: u32,
    do_free: u32,
    text: Option<Vec<u8>>,
}

fn read_source(cpu: &mut Cpu, other: u32) -> Option<Source> {
    let name = read32(cpu, other.wrapping_add(O_NAME))?;
    let do_free = read32(cpu, other.wrapping_add(O_DO_FREE))?;
    let text = match (do_free, name) {
        (0, _) | (_, 0) => None,
        _ => Some(cstr(cpu, name)?),
    };
    Some(Source {
        name,
        do_free,
        text,
    })
}

/// Make `this`, which `writable` passed, a copy of `source` of the class
/// `class`: a copy of the message if the source owns its own, else the
/// same pointer.
fn copy_into(cpu: &mut Cpu, this: u32, class: usize, source: Source) -> Option<()> {
    let name = match source.text {
        Some(text) => new_text(cpu, &text),
        None => source.name,
    };
    set_object(cpu, this, class, name, source.do_free)
}

/// The copy constructors.
fn construct_copy(cpu: &mut Cpu, class: usize, other: u32) -> Option<u32> {
    let this = cpu.ecx();
    let source = read_source(cpu, other)?;
    if !writable(cpu, this) {
        return None;
    }
    copy_into(cpu, this, class, source)?;
    Some(this)
}

/// ??0exception@@QAE@XZ: no message.
pub fn exception_new(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let this = cpu.ecx();
    set_object(cpu, this, EXCEPTION, 0, 0)?;
    Some(this)
}

/// ??0exception@@QAE@ABQBD@Z(const char *const &message).
pub fn exception_with(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let message = read32(cpu, args[0])?;
    construct_with(cpu, EXCEPTION, message)
}

pub fn exception_copy(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    construct_copy(cpu, EXCEPTION, args[0])
}

/// ??0bad_cast@@QAE@ABQBD@Z(const char *const &message).
pub fn bad_cast_with(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let message = read32(cpu, args[0])?;
    construct_with(cpu, BAD_CAST, message)
}

pub fn bad_cast_copy(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    construct_copy(cpu, BAD_CAST, args[0])
}

/// ??0bad_typeid@@QAE@PBD@Z(const char *message).
pub fn bad_typeid_with(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    construct_with(cpu, BAD_TYPEID, args[0])
}

pub fn bad_typeid_copy(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    construct_copy(cpu, BAD_TYPEID, args[0])
}

/// ??0__non_rtti_object@@QAE@PBD@Z(const char *message).
pub fn non_rtti_with(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    construct_with(cpu, NON_RTTI, args[0])
}

pub fn non_rtti_copy(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    construct_copy(cpu, NON_RTTI, args[0])
}

/// The destructor of `this`, which every one of the four classes has:
/// `exception`'s vtable again, and the message freed if the object owns
/// it.
fn destroy(cpu: &mut Cpu, this: u32) -> Option<()> {
    let name = read32(cpu, this.wrapping_add(O_NAME))?;
    let do_free = read32(cpu, this.wrapping_add(O_DO_FREE))?;
    write32(cpu, this, vtable(EXCEPTION))?;
    if do_free != 0 && name != 0 {
        heap_free(cpu, name);
    }
    Some(())
}

/// ??1exception@@UAE@XZ and the other classes' destructors.
pub fn exception_destroy(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let this = cpu.ecx();
    destroy(cpu, this)?;
    Some(this)
}

/// operator=(const exception &other) of the four classes: unless it is
/// the object itself, `this` is destroyed and made a copy of `other`, as
/// `exception`, whatever class it was. Everything is read and `this`
/// probed before the destructor frees its message, so that a page fault,
/// after which the call runs again, doesn't free it twice.
pub fn exception_assign(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let this = cpu.ecx();
    let other = args[0];
    if this == other {
        return Some(this);
    }
    let source = read_source(cpu, other)?;
    if !writable(cpu, this) {
        return None;
    }
    destroy(cpu, this)?;
    copy_into(cpu, this, EXCEPTION, source)?;
    Some(this)
}

/// The scalar deleting destructor (`??_G`): the object destroyed, and
/// freed with bit 0 of `flags`.
pub fn exception_delete(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let this = cpu.ecx();
    destroy(cpu, this)?;
    if args[0] & 1 != 0 {
        heap_free(cpu, this);
    }
    Some(this)
}

/// The vector deleting destructor (`??_E`), which the vtables point to:
/// with bit 1 of `flags`, `this` is an array whose count is in the word
/// before it, each element destroyed from the last, and the block from
/// that word freed with bit 0. Else it is the scalar one. The whole
/// array is probed first, so that a page fault, after which the call
/// runs again, comes before any element's message is freed.
pub fn exception_vector_delete(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let flags = args[0];
    if flags & 2 == 0 {
        return exception_delete(cpu, args);
    }
    let this = cpu.ecx();
    let block = this.wrapping_sub(4);
    let count = read32(cpu, block)?;
    let size = (count as u64 * OBJECT_SIZE as u64).min(u32::MAX as u64) as usize;
    if !cpu.bus.guest_probe(this, size, true) {
        return None;
    }
    for i in (0..count).rev() {
        destroy(cpu, this.wrapping_add(i.wrapping_mul(OBJECT_SIZE)))?;
    }
    if flags & 1 != 0 {
        heap_free(cpu, block);
    }
    Some(block)
}

/// ?what@exception@@UBEPBDXZ: the message, or "Unknown exception".
pub fn what(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let this = cpu.ecx();
    match read32(cpu, this.wrapping_add(O_NAME))? {
        0 => Some(var_at(super::V_RTTI)),
        name => Some(name),
    }
}

// type_info.

/// A type_info's fields: the name `name()` made, and the decorated
/// name.
const TI_NAME: u32 = 4;
const TI_RAW: u32 = 8;

/// The decorated name of the type_info at `at`, past its first
/// character (the '.'), as the comparisons read it.
fn raw_name(cpu: &mut Cpu, at: u32) -> Option<Vec<u8>> {
    cstr(cpu, at.wrapping_add(TI_RAW + 1))
}

/// ?name@type_info@@QBEPBDXZ: the undecorated name, made the first time
/// and kept in the type_info, on the runtime's heap.
pub fn type_info_name(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let this = cpu.ecx();
    let made = read32(cpu, this.wrapping_add(TI_NAME))?;
    if made != 0 {
        return Some(made);
    }
    let raw = raw_name(cpu, this)?;
    let name = super::undname::type_name(&raw);
    cpu.bus
        .guest_probe(this.wrapping_add(TI_NAME), 4, true)
        .then_some(())?;
    let at = new_text(cpu, &name);
    write32(cpu, this.wrapping_add(TI_NAME), at)?;
    Some(at)
}

/// ?raw_name@type_info@@QBEPBDXZ: the decorated name.
pub fn type_info_raw_name(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(cpu.ecx().wrapping_add(TI_RAW))
}

/// The comparison of two type_infos' decorated names.
fn compare(cpu: &mut Cpu, other: u32) -> Option<std::cmp::Ordering> {
    let this = cpu.ecx();
    let a = raw_name(cpu, this)?;
    let b = raw_name(cpu, other)?;
    Some(a.cmp(&b))
}

/// ??8type_info@@QBEHABV0@@Z: operator==.
pub fn type_info_eq(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(compare(cpu, args[0])?.is_eq() as u32)
}

/// ??9type_info@@QBEHABV0@@Z: operator!=.
pub fn type_info_ne(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(compare(cpu, args[0])?.is_ne() as u32)
}

/// ?before@type_info@@QBEHABV1@@Z: whether this type's name sorts first.
pub fn type_info_before(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(compare(cpu, args[0])?.is_lt() as u32)
}

/// ??1type_info@@UAE@XZ: the runtime's type_info vtable again, and the
/// name `name()` made freed (the field keeps the pointer).
pub fn type_info_destroy(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let this = cpu.ecx();
    let made = read32(cpu, this.wrapping_add(TI_NAME))?;
    write32(cpu, this, vtable(TYPE_INFO))?;
    if made != 0 {
        heap_free(cpu, made);
    }
    Some(this)
}

/// ??_Gtype_info@@UAEPAXI@Z, the scalar deleting destructor its vtable
/// points to.
pub fn type_info_delete(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let this = type_info_destroy(cpu, &[])?;
    if args[0] & 1 != 0 {
        heap_free(cpu, this);
    }
    Some(this)
}

// Throwing.

/// Throw an object of the class `class` (bad_cast, bad_typeid or
/// __non_rtti_object) with `message`, from the function the running stub
/// is of: the object below the stub's return address, where the real
/// runtime's frame has it, owning a copy of the message on the heap. The
/// catch gets a copy from the class's copy constructor, and the object
/// is destroyed after the catch, as `_CxxThrowException`'s.
fn throw(cpu: &mut Cpu, class: usize, message: &[u8]) -> Option<u32> {
    let object = cpu.esp().wrapping_sub(OBJECT_SIZE);
    if !writable(cpu, object) {
        return None;
    }
    let name = new_text(cpu, message);
    set_object(cpu, object, class, name, 1)?;
    let info = Rtti::new(var_at(super::V_RTTI)).throw[class];
    let params = [super::startup::CXX_MAGIC, object, info];
    super::super::seh::raise_at(cpu, object, super::startup::CXX_EXCEPTION, 1, &params)
}

// Run-time type information.

/// The word at `at`, if the program may read it. The real functions
/// read inside a __try, and an access violation is a
/// __non_rtti_object; these read nothing that isn't there.
fn peek(cpu: &mut Cpu, at: u32) -> Option<u32> {
    allows(cpu, at, 4, false).then(|| cpu.bus.guest_read_32(at))
}

/// The decorated name at `at`, if the program may read it.
fn peek_name(cpu: &mut Cpu, at: u32) -> Option<Vec<u8>> {
    let mut name = Vec::new();
    loop {
        let lin = at.wrapping_add(name.len() as u32);
        if name.len() >= 0x1000 || !allows(cpu, lin, 1, false) {
            return None;
        }
        match cpu.bus.guest_read_8(lin) {
            0 => return Some(name),
            b => name.push(b),
        }
    }
}

/// Whether the type descriptors at `a` and `b` are of the same type: the
/// same descriptor, or ones with the same name, as each module has its
/// own.
fn same_type(cpu: &mut Cpu, a: u32, b: u32) -> Option<bool> {
    if a == b {
        return Some(true);
    }
    Some(peek_name(cpu, a.wrapping_add(8))? == peek_name(cpu, b.wrapping_add(8))?)
}

/// Whether the base class descriptor at `bcd` is of the type whose
/// descriptor is at `td`.
fn is_type(cpu: &mut Cpu, bcd: u32, td: u32) -> Option<bool> {
    let ty = peek(cpu, bcd.wrapping_add(B_TYPE))?;
    same_type(cpu, ty, td)
}

/// The complete object locator of the object at `object`, from the word
/// before its vtable.
fn locator(cpu: &mut Cpu, object: u32) -> Option<u32> {
    let vtable = peek(cpu, object)?;
    peek(cpu, vtable.wrapping_sub(4))
}

/// The whole object the polymorphic object at `object` is part of: back
/// by the locator's offset, and by the constructor displacement for a
/// part in a virtual base.
fn complete_object(cpu: &mut Cpu, object: u32) -> Option<u32> {
    let col = locator(cpu, object)?;
    let offset = peek(cpu, col.wrapping_add(4))?;
    let cd_offset = peek(cpu, col.wrapping_add(8))?;
    let mut complete = object.wrapping_sub(offset);
    if cd_offset != 0 {
        complete = complete.wrapping_add(peek(cpu, object.wrapping_sub(cd_offset))?);
    }
    Some(complete)
}

/// Where the base a pointer to member describes is in the object at
/// `object`: its offset, through the virtual base table when the base is
/// virtual (a displacement in the vbtable at `pdisp`).
fn member_offset(cpu: &mut Cpu, object: u32, pmd: u32) -> Option<u32> {
    let mdisp = peek(cpu, pmd)?;
    let pdisp = peek(cpu, pmd.wrapping_add(4))?;
    let vdisp = peek(cpu, pmd.wrapping_add(8))?;
    let mut offset = 0u32;
    if (pdisp as i32) >= 0 {
        let table = peek(cpu, object.wrapping_add(pdisp))?;
        offset = pdisp.wrapping_add(peek(cpu, table.wrapping_add(vdisp))?);
    }
    Some(offset.wrapping_add(mdisp))
}

/// A base class descriptor's fields.
const B_TYPE: u32 = 0;
const B_CONTAINED: u32 = 4;
const B_WHERE: u32 = 8;
const B_ATTRIBUTES: u32 = 20;
/// A base that is private or protected (not public), or is a base more
/// than once (ambiguous).
const BCD_NOT_VISIBLE: u32 = 1;
const BCD_AMBIGUOUS: u32 = 2;

/// The class hierarchy's attributes: multiple and virtual inheritance.
const CHD_MULTIPLE: u32 = 1;
const CHD_VIRTUAL: u32 = 2;

/// The base class descriptors of the hierarchy at `hierarchy`.
fn bases(cpu: &mut Cpu, hierarchy: u32) -> Option<Vec<u32>> {
    let count = peek(cpu, hierarchy.wrapping_add(8))?;
    let array = peek(cpu, hierarchy.wrapping_add(12))?;
    (0..count.min(0x1000))
        .map(|i| peek(cpu, array.wrapping_add(4 * i)))
        .collect()
}

/// The search of __RTDynamicCast: the base class descriptor of the
/// target in the complete object whose hierarchy is `hierarchy`, which
/// the source at `src_offset` in it can reach, or 0.
struct Cast {
    complete: u32,
    src: u32,
    target: u32,
    src_offset: u32,
}

impl Cast {
    /// Single inheritance: the first target that is a public base.
    fn single(&self, cpu: &mut Cpu, bases: &[u32]) -> Option<u32> {
        for &bcd in bases {
            if is_type(cpu, bcd, self.target)?
                && peek(cpu, bcd.wrapping_add(B_ATTRIBUTES))? & BCD_NOT_VISIBLE == 0
            {
                return Some(bcd);
            }
        }
        Some(0)
    }

    /// Whether the target `bcd`, the `i`th base, contains the source at
    /// its offset in the object.
    fn contains_source(&self, cpu: &mut Cpu, bases: &[u32], i: usize) -> Option<bool> {
        let contained = peek(cpu, bases[i].wrapping_add(B_CONTAINED))? as usize;
        for &sub in bases.iter().skip(i + 1).take(contained) {
            if is_type(cpu, sub, self.src)?
                && member_offset(cpu, self.complete, sub.wrapping_add(B_WHERE))? == self.src_offset
            {
                return Some(true);
            }
        }
        Some(false)
    }

    /// The target that is a public base reached once, for a cast that
    /// isn't down from the source.
    fn cross(&self, cpu: &mut Cpu, bases: &[u32]) -> Option<u32> {
        for &bcd in bases {
            if is_type(cpu, bcd, self.target)?
                && peek(cpu, bcd.wrapping_add(B_ATTRIBUTES))? & (BCD_NOT_VISIBLE | BCD_AMBIGUOUS)
                    == 0
            {
                return Some(bcd);
            }
        }
        Some(0)
    }

    /// Multiple inheritance: a target containing the source, else a
    /// cross cast.
    fn multiple(&self, cpu: &mut Cpu, bases: &[u32]) -> Option<u32> {
        for (i, &bcd) in bases.iter().enumerate() {
            if is_type(cpu, bcd, self.target)? && self.contains_source(cpu, bases, i)? {
                return Some(bcd);
            }
        }
        self.cross(cpu, bases)
    }

    /// Virtual inheritance: as multiple, but targets containing the
    /// source at different places in the object make the cast fail.
    fn virtual_(&self, cpu: &mut Cpu, bases: &[u32]) -> Option<u32> {
        let mut found = 0u32;
        for (i, &bcd) in bases.iter().enumerate() {
            if !is_type(cpu, bcd, self.target)? {
                continue;
            }
            let contained = peek(cpu, bcd.wrapping_add(B_CONTAINED))? as usize;
            for &sub in bases.iter().skip(i + 1).take(contained) {
                if !is_type(cpu, sub, self.src)?
                    || member_offset(cpu, self.complete, sub.wrapping_add(B_WHERE))?
                        != self.src_offset
                {
                    continue;
                }
                if found != 0
                    && member_offset(cpu, self.complete, bcd.wrapping_add(B_WHERE))?
                        != member_offset(cpu, self.complete, found.wrapping_add(B_WHERE))?
                {
                    return Some(0);
                }
                found = bcd;
            }
        }
        if found != 0 {
            return Some(found);
        }
        self.cross(cpu, bases)
    }
}

/// The access violation the real functions turn into a
/// __non_rtti_object.
enum Rt {
    Value(u32),
    Throw(usize, &'static [u8]),
}

fn finish(cpu: &mut Cpu, result: Option<Rt>) -> Option<u32> {
    match result {
        Some(Rt::Value(value)) => Some(value),
        Some(Rt::Throw(class, message)) => throw(cpu, class, message),
        None => throw(cpu, NON_RTTI, ACCESS_VIOLATION),
    }
}

/// __RTDynamicCast(object, vfdelta, src, target, is_reference): the
/// object at `object`, of the type whose descriptor is at `src` (`vfdelta`
/// bytes into it is the vtable pointer it was found by), as the type at
/// `target`: the address of that base of the complete object, or 0 (a
/// bad_cast for a reference) when it isn't one or the cast is ambiguous.
pub fn rt_dynamic_cast(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[object, vfdelta, src, target, is_reference] = args else {
        unreachable!()
    };
    if object == 0 {
        return Some(0);
    }
    let result = (|| {
        let complete = complete_object(cpu, object)?;
        let col = locator(cpu, object)?;
        let hierarchy = peek(cpu, col.wrapping_add(16))?;
        let attributes = peek(cpu, hierarchy.wrapping_add(4))?;
        let bases = bases(cpu, hierarchy)?;
        let cast = Cast {
            complete,
            src,
            target,
            src_offset: object.wrapping_sub(vfdelta).wrapping_sub(complete),
        };
        let bcd = if attributes & CHD_MULTIPLE == 0 {
            cast.single(cpu, &bases)?
        } else if attributes & CHD_VIRTUAL == 0 {
            cast.multiple(cpu, &bases)?
        } else {
            cast.virtual_(cpu, &bases)?
        };
        Some(match bcd {
            0 if is_reference != 0 => Rt::Throw(BAD_CAST, BAD_DYNAMIC_CAST),
            0 => Rt::Value(0),
            bcd => Rt::Value(complete.wrapping_add(member_offset(
                cpu,
                complete,
                bcd.wrapping_add(B_WHERE),
            )?)),
        })
    })();
    finish(cpu, result)
}

/// __RTtypeid(object): the type descriptor of the polymorphic object at
/// `object`, from its complete object locator. A bad_typeid for a null
/// pointer, and a __non_rtti_object when the descriptor can't be read.
pub fn rt_typeid(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let object = args[0];
    if object == 0 {
        return throw(cpu, BAD_TYPEID, TYPEID_OF_NULL);
    }
    let result = (|| {
        let col = locator(cpu, object)?;
        let td = peek(cpu, col.wrapping_add(12))?;
        Some(if allows(cpu, td, 8, false) {
            Rt::Value(td)
        } else {
            Rt::Throw(NON_RTTI, BAD_READ_POINTER)
        })
    })();
    finish(cpu, result)
}

/// __RTCastToVoid(object): `dynamic_cast<void *>`, the complete object.
pub fn rt_cast_to_void(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let object = args[0];
    if object == 0 {
        return Some(0);
    }
    let result = complete_object(cpu, object).map(Rt::Value);
    finish(cpu, result)
}

// The undecorator.

/// __unDName(buffer, mangled, size, alloc, free, flags): the
/// undecorated `mangled` (or the name itself, when it can't be
/// undecorated) in `buffer`, cut to `size` bytes with its 0, or with no
/// buffer in a block from the program's `alloc(length + 1)`. The result
/// is the buffer, or 0. Visual C++ 6's returns 0 at once when `alloc` is
/// NULL, buffer or not: its undecorator takes its own memory from
/// `alloc` too.
pub fn undname(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    undname_ex(
        cpu,
        &[args[0], args[1], args[2], args[3], args[4], 0, args[5]],
    )
}

/// __unDNameEx(buffer, mangled, size, alloc, free, get_parameter, flags):
/// as __unDName; the function that names template parameters goes
/// unused.
pub fn undname_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[buffer, mangled, size, alloc, _, _, flags] = args else {
        unreachable!()
    };
    if alloc == 0 || mangled == 0 {
        return Some(0);
    }
    let name = cstr_n(cpu, mangled, 0x1000)?;
    let text = super::undname::undecorate(&name, flags).map_or(name, String::into_bytes);
    if buffer != 0 {
        if size == 0 {
            return Some(buffer);
        }
        let len = text.len().min(size as usize - 1);
        put_cstr(cpu, buffer, &text[..len])?;
        return Some(buffer);
    }
    // The text goes in the call's data, four bytes a word, after its
    // length.
    let mut data = vec![text.len() as u32];
    data.extend(
        text.chunks(4)
            .map(|c| c.iter().rev().fold(0, |w, &b| w << 8 | b as u32)),
    );
    let resume = call::here(cpu);
    if !super::call_program(
        cpu,
        alloc,
        &[text.len() as u32 + 1],
        C_UNDNAME,
        resume,
        &data,
    ) {
        return Some(0);
    }
    None
}

/// The program's `alloc` returned the block for __unDName's text.
pub fn undname_allocated(cpu: &mut Cpu, value: u32, call: Call) {
    let Some((&len, words)) = call.data.split_first() else {
        return;
    };
    if value != 0 {
        let text: Vec<u8> = words
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .take(len as usize)
            .collect();
        put_cstr(cpu, value, &text);
    }
    call::finish(cpu, call.resume, value);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The structures fit their variable, and each vtable comes right
    /// after the word that points to its locator.
    #[test]
    fn layout() {
        let rtti = Rtti::new(0x1000);
        assert!(rtti.bytes.len() as u32 <= RTTI_SIZE);
        for class in &CLASSES {
            assert_eq!(var_at(class.vtable_var), var_at(class.header_var) + 8);
        }
    }
}
