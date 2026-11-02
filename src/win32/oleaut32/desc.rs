//! The descriptions ITypeLib and ITypeInfo give the program: TLIBATTR,
//! TYPEATTR, FUNCDESC and VARDESC, each in one block of the task
//! allocator with the TYPEDESCs, ELEMDESCs and default values it points
//! to, as ReleaseTypeAttr, ReleaseFuncDesc and ReleaseVarDesc free them.
//! Their layouts are those of the 32-bit oaidl.h.

use super::model::*;
use crate::cpu::Cpu;

/// The sizes of the structures.
pub const TLIBATTR_SIZE: u32 = 32;
const TYPEATTR_SIZE: u32 = 76;
const FUNCDESC_SIZE: u32 = 52;
const VARDESC_SIZE: u32 = 36;
const ELEMDESC_SIZE: u32 = 16;
const TYPEDESC_SIZE: u32 = 8;
/// PARAMDESCEX: its size, then a VARIANT at 8.
const PARAMDESCEX_SIZE: u32 = 24;
const VARIANT_SIZE: u32 = 16;

/// A block being laid out: its bytes from `base`, and the BSTRs to make
/// once it is in the guest's memory, by where their pointers go.
struct Block {
    base: u32,
    bytes: Vec<u8>,
    strings: Vec<(u32, Option<String>)>,
}

impl Block {
    fn new(base: u32, size: u32) -> Block {
        Block {
            base,
            bytes: vec![0; size as usize],
            strings: Vec::new(),
        }
    }

    /// `size` more bytes, at an address aligned to `align`: that address.
    fn reserve(&mut self, size: u32, align: u32) -> u32 {
        let start = (self.bytes.len() as u32).next_multiple_of(align);
        self.bytes.resize((start + size) as usize, 0);
        self.base + start
    }

    fn put16(&mut self, at: u32, value: u16) {
        let i = (at - self.base) as usize;
        self.bytes[i..i + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn put32(&mut self, at: u32, value: u32) {
        let i = (at - self.base) as usize;
        self.bytes[i..i + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn put64(&mut self, at: u32, value: u64) {
        let i = (at - self.base) as usize;
        self.bytes[i..i + 8].copy_from_slice(&value.to_le_bytes());
    }

    /// The TYPEDESC `t` at `at`, what it points to after the rest.
    fn typedesc(&mut self, at: u32, t: &TypeDesc) {
        let first = match t {
            TypeDesc::Base(_) => 0,
            TypeDesc::Ptr(inner) | TypeDesc::SafeArray(inner) => {
                let child = self.reserve(TYPEDESC_SIZE, 4);
                self.typedesc(child, inner);
                child
            }
            TypeDesc::CArray(element, bounds) => {
                // ARRAYDESC: the element's TYPEDESC, the number of
                // dimensions, and a SAFEARRAYBOUND for each.
                let n = bounds.len() as u32;
                let array = self.reserve(12 + 8 * n.max(1), 4);
                self.typedesc(array, element);
                self.put16(array + 8, n as u16);
                for (i, &(elements, lower)) in bounds.iter().enumerate() {
                    let b = array + 12 + 8 * i as u32;
                    self.put32(b, elements);
                    self.put32(b + 4, lower as u32);
                }
                array
            }
            TypeDesc::User(href) => *href,
        };
        self.put32(at, first);
        self.put16(at + 4, t.vt());
    }

    /// The VARIANT `value` at `at`.
    fn variant(&mut self, at: u32, value: &Value) {
        match value {
            Value::Bits(vt, bits) => {
                self.put16(at, *vt);
                self.put64(at + 8, *bits);
            }
            Value::Str(s) => {
                self.put16(at, VT_BSTR);
                self.strings.push((at + 8, s.clone()));
            }
        }
    }

    /// The ELEMDESC of the parameter or the return type at `at`.
    fn elemdesc(&mut self, at: u32, ty: &TypeDesc, flags: u16, default: Option<&Value>) {
        self.typedesc(at, ty);
        if let Some(value) = default.filter(|_| flags & PARAMFLAG_FHASDEFAULT != 0) {
            let ex = self.reserve(PARAMDESCEX_SIZE, 8);
            self.put32(ex, PARAMDESCEX_SIZE);
            self.variant(ex + 8, value);
            self.put32(at + 8, ex);
        }
        self.put16(at + 12, flags);
    }
}

/// Lay out a block twice: first to learn its size, then at the address
/// the allocator gave. The block's address, or 0 when memory is out.
fn place(cpu: &mut Cpu, size: u32, fill: impl Fn(&mut Block)) -> u32 {
    let mut sizing = Block::new(0, size);
    fill(&mut sizing);
    let at = crate::win32::ole32::task_alloc(cpu, sizing.bytes.len() as u32);
    if at == 0 {
        return 0;
    }
    let mut block = Block::new(at, size);
    fill(&mut block);
    cpu.bus.guest_write_bytes(at, &block.bytes);
    for (slot, s) in block.strings {
        let bstr = s.map_or(0, |s| super::bstr(cpu, &s));
        cpu.bus.guest_write_32(slot, bstr);
    }
    at
}

/// The TLIBATTR of `lib`.
pub fn lib_attr(cpu: &mut Cpu, lib: &TypeLib) -> u32 {
    place(cpu, TLIBATTR_SIZE, |b| {
        let at = b.base;
        b.bytes[..16].copy_from_slice(&lib.guid.0);
        b.put32(at + 16, lib.lcid);
        b.put32(at + 20, lib.syskind);
        b.put16(at + 24, lib.major);
        b.put16(at + 26, lib.minor);
        b.put16(at + 28, lib.flags);
    })
}

/// What TYPEATTR says of a type, as the program sees it (typelib.rs's
/// views of a dual interface differ from the type in the file).
pub struct Attr<'a> {
    pub info: &'a TypeInfo,
    pub kind: u32,
    pub flags: u16,
    pub funcs: usize,
    pub impls: usize,
    pub vft: u16,
}

pub fn type_attr(cpu: &mut Cpu, a: &Attr) -> u32 {
    place(cpu, TYPEATTR_SIZE, |b| {
        let at = b.base;
        let t = a.info;
        b.bytes[..16].copy_from_slice(&t.guid.0);
        b.put32(at + 16, t.lcid);
        b.put32(at + 24, MEMBERID_NIL as u32);
        b.put32(at + 28, MEMBERID_NIL as u32);
        b.put32(at + 36, t.size);
        b.put32(at + 40, a.kind);
        b.put16(at + 44, a.funcs as u16);
        b.put16(at + 46, t.vars.len() as u16);
        b.put16(at + 48, a.impls as u16);
        b.put16(at + 50, a.vft);
        b.put16(at + 52, t.align);
        b.put16(at + 54, a.flags);
        b.put16(at + 56, t.major);
        b.put16(at + 58, t.minor);
        if let Some(alias) = &t.alias {
            b.typedesc(at + 60, alias);
        }
    })
}

pub fn func_desc(cpu: &mut Cpu, f: &Func) -> u32 {
    place(cpu, FUNCDESC_SIZE, |b| {
        let at = b.base;
        let n = f.params.len() as u32;
        let params = if n > 0 {
            b.reserve(ELEMDESC_SIZE * n, 4)
        } else {
            0
        };
        b.put32(at, f.memid as u32);
        b.put32(at + 8, params);
        b.put32(at + 12, f.kind);
        b.put32(at + 16, f.invkind);
        b.put32(at + 20, f.callconv);
        b.put16(at + 24, n as u16);
        b.put16(at + 26, f.opt as u16);
        b.put16(at + 28, f.vft as u16);
        b.elemdesc(at + 32, &f.ret, 0, None);
        b.put16(at + 48, f.flags);
        for (i, p) in f.params.iter().enumerate() {
            let e = params + ELEMDESC_SIZE * i as u32;
            b.elemdesc(e, &p.ty, p.flags, p.default.as_ref());
        }
    })
}

pub fn var_desc(cpu: &mut Cpu, v: &Var) -> u32 {
    place(cpu, VARDESC_SIZE, |b| {
        let at = b.base;
        b.put32(at, v.memid as u32);
        match &v.value {
            VarValue::Offset(offset) => b.put32(at + 8, *offset),
            VarValue::Const(value) => {
                let variant = b.reserve(VARIANT_SIZE, 8);
                b.variant(variant, value);
                b.put32(at + 8, variant);
            }
        }
        b.elemdesc(at + 12, &v.ty, 0, None);
        b.put16(at + 28, v.flags);
        b.put32(at + 32, v.kind);
    })
}

/// Free a FUNCDESC this module made, and the BSTRs of its defaults.
pub fn free_func_desc(cpu: &mut Cpu, at: u32) {
    if at == 0 || !cpu.bus.guest_probe(at, FUNCDESC_SIZE as usize, false) {
        return;
    }
    let params = cpu.bus.guest_read_32(at + 8);
    let n = cpu.bus.guest_read_16(at + 24) as u32;
    for i in 0..n {
        let e = params.wrapping_add(ELEMDESC_SIZE * i);
        if !cpu.bus.guest_probe(e, ELEMDESC_SIZE as usize, false) {
            break;
        }
        let ex = cpu.bus.guest_read_32(e + 8);
        if ex != 0 {
            free_variant(cpu, ex + 8);
        }
    }
    crate::win32::ole32::task_free(cpu, at);
}

/// Free a VARDESC this module made, and its constant's BSTR.
pub fn free_var_desc(cpu: &mut Cpu, at: u32) {
    if at == 0 || !cpu.bus.guest_probe(at, VARDESC_SIZE as usize, false) {
        return;
    }
    if cpu.bus.guest_read_32(at + 32) == VAR_CONST {
        let variant = cpu.bus.guest_read_32(at + 8);
        free_variant(cpu, variant);
    }
    crate::win32::ole32::task_free(cpu, at);
}

fn free_variant(cpu: &mut Cpu, at: u32) {
    if cpu.bus.guest_probe(at, VARIANT_SIZE as usize, false) && cpu.bus.guest_read_16(at) == VT_BSTR
    {
        let s = cpu.bus.guest_read_32(at + 8);
        super::free(cpu, s);
    }
}
