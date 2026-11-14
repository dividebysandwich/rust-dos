//! The MSFT format of type libraries, which ICreateTypeLib2 and MIDL
//! write: a header, a directory of segments (the types' table, the
//! names, strings, GUIDs, type descriptions and the references to
//! imported libraries), and each type's members in a block of its own.
//!
//! The layout follows Wine's oleaut32/typelib.c and typelib.h (LGPL 2.1
//! or later):
//!
//! Copyright 1997 Marcus Meissner
//! Copyright 1999 Rein Klazes
//! Copyright 2000 Francois Jacques
//! Copyright 2001 Huw D M Davies for CodeWeavers
//! Copyright 2004 Alastair Bridgewater
//! Copyright 2005 Robert Shearman, for CodeWeavers
//! Copyright 2013 Andrew Eikum for CodeWeavers

use super::model::*;
use crate::win32::com::Guid;

pub const SIGNATURE: &[u8; 4] = b"MSFT";

/// The header's size, and its flag that a help DLL's name follows it.
const HEADER: usize = 0x54;
const HELPDLLFLAG: u32 = 0x100;
/// A type's entry in the types' table.
const TYPEINFO: usize = 0x64;
/// A reference's flag that it names its type by GUID.
const IMPINFO_OFFSET_IS_GUID: u32 = 0x10000;

/// A segment of the file: where it starts and how long it is.
#[derive(Clone, Copy, Default)]
struct Seg {
    offset: usize,
    len: usize,
}

/// The segments, by their places in the directory.
const TYPEINFO_TAB: usize = 0;
const IMP_INFO: usize = 1;
const IMP_FILES: usize = 2;
const REF_TAB: usize = 3;
const GUID_TAB: usize = 5;
const NAME_TAB: usize = 7;
const STRING_TAB: usize = 8;
const TYPDESC_TAB: usize = 9;
const ARRAY_DESC: usize = 10;
const CUST_DATA: usize = 11;

struct Reader<'a> {
    b: &'a [u8],
    segs: [Seg; 15],
    /// The code page of the library's strings.
    page: CodePage,
}

impl Reader<'_> {
    fn u16(&self, at: usize) -> Option<u16> {
        Some(u16::from_le_bytes(self.b.get(at..at + 2)?.try_into().ok()?))
    }

    fn u32(&self, at: usize) -> Option<u32> {
        Some(u32::from_le_bytes(self.b.get(at..at + 4)?.try_into().ok()?))
    }

    fn i32(&self, at: usize) -> Option<i32> {
        self.u32(at).map(|v| v as i32)
    }

    fn seg(&self, which: usize) -> Seg {
        self.segs[which]
    }

    /// The name at `offset` in the names' table.
    fn name(&self, offset: i32) -> Option<String> {
        let seg = self.seg(NAME_TAB);
        if offset < 0 || offset as usize >= seg.len {
            return None;
        }
        let at = seg.offset + offset as usize;
        let len = (self.u32(at + 8)? & 0xFF) as usize;
        Some(ansi(self.page, self.b.get(at + 12..at + 12 + len)?))
    }

    /// The string at `offset` in the strings' table.
    fn string(&self, offset: i32) -> Option<String> {
        let seg = self.seg(STRING_TAB);
        if offset < 0 || offset as usize >= seg.len {
            return None;
        }
        let at = seg.offset + offset as usize;
        let len = self.u16(at)? as usize;
        Some(ansi(self.page, self.b.get(at + 2..at + 2 + len)?))
    }

    /// The GUID at `offset` in the GUIDs' table.
    fn guid(&self, offset: i32) -> Option<Guid> {
        let seg = self.seg(GUID_TAB);
        if offset < 0 || offset as usize + 16 > seg.len {
            return None;
        }
        let at = seg.offset + offset as usize;
        Some(Guid(self.b.get(at..at + 16)?.try_into().ok()?))
    }

    /// The type a member's or a type description's data type is: a
    /// simple type when it is negative, else the description at that
    /// offset in the table of them.
    fn typedesc(&self, datatype: i32, depth: u32) -> TypeDesc {
        if datatype < 0 {
            return TypeDesc::Base((datatype & 0xFFF) as u16);
        }
        let seg = self.seg(TYPDESC_TAB);
        let at = seg.offset + datatype as usize;
        let (Some(vt), Some(arg)) = (self.u16(at), self.i32(at + 4)) else {
            return TypeDesc::Base(VT_EMPTY);
        };
        if depth > 16 || datatype as usize >= seg.len {
            return TypeDesc::Base(VT_EMPTY);
        }
        match vt & 0xFFF {
            VT_PTR => TypeDesc::ptr(self.typedesc(arg, depth + 1)),
            VT_SAFEARRAY => TypeDesc::SafeArray(Box::new(self.typedesc(arg, depth + 1))),
            VT_USERDEFINED => TypeDesc::User(arg as u32),
            VT_CARRAY => self.array(arg & 0xFFFF, depth),
            vt => TypeDesc::Base(vt),
        }
    }

    /// The array description at `offset` in the table of them.
    fn array(&self, offset: i32, depth: u32) -> TypeDesc {
        let seg = self.seg(ARRAY_DESC);
        let at = seg.offset + offset as usize;
        let (Some(elem), Some(dims)) = (self.i32(at), self.u16(at + 4)) else {
            return TypeDesc::Base(VT_EMPTY);
        };
        let bounds = (0..dims as usize)
            .map_while(|i| Some((self.u32(at + 8 + 8 * i)?, self.i32(at + 12 + 8 * i)?)))
            .collect();
        TypeDesc::CArray(Box::new(self.typedesc(elem, depth + 1)), bounds)
    }

    /// A value: packed into `offset` itself when that is negative, else
    /// at it in the custom data's table.
    fn value(&self, offset: i32) -> Option<Value> {
        if offset < 0 {
            let vt = ((offset as u32 & 0x7C00_0000) >> 26) as u16;
            return Some(Value::Bits(vt, (offset as u32 & 0x3FF_FFFF) as u64));
        }
        let at = self.seg(CUST_DATA).offset + offset as usize;
        let vt = self.u16(at)?;
        let size = match vt {
            0..=4 | 10 | 11 | 16..=19 | 22..=25 => 4,
            5..=7 | 14 | 20 | 21 | 64 => 8,
            VT_BSTR => {
                let len = self.i32(at + 2)?;
                if len < 0 {
                    return Some(Value::Str(None));
                }
                let bytes = self.b.get(at + 6..at + 6 + len as usize)?;
                return Some(Value::Str(Some(ansi(self.page, bytes))));
            }
            _ => return Some(Value::Bits(vt, 0)),
        };
        let mut bytes = [0; 8];
        bytes[..size].copy_from_slice(self.b.get(at + 2..at + 2 + size)?);
        Some(Value::Bits(vt, u64::from_le_bytes(bytes)))
    }
}

/// Read the MSFT library `b`; None if it isn't one.
/// The library in `b`, its strings in the code page `page`.
pub fn read(b: &[u8], page: CodePage) -> Option<TypeLib> {
    if b.get(..4)? != SIGNATURE {
        return None;
    }
    let mut r = Reader {
        b,
        segs: [Seg::default(); 15],
        page,
    };
    let h = |i: usize| -> Option<i32> {
        Some(i32::from_le_bytes(
            b.get(4 * i..4 * i + 4)?.try_into().ok()?,
        ))
    };
    let infos = h(8)?.max(0) as usize;
    let varflags = h(5)? as u32;
    let mut dir = HEADER + 4 * infos;
    if varflags & HELPDLLFLAG != 0 {
        dir += 4;
    }
    for i in 0..15 {
        let at = dir + 16 * i;
        let (offset, len) = (r.i32(at)?, r.i32(at + 4)?);
        r.segs[i] = Seg {
            offset: offset.max(0) as usize,
            len: if offset < 0 { 0 } else { len.max(0) as usize },
        };
    }
    // The first two segments say 0x0F in their last field, as Wine checks
    // to tell it is the directory.
    if r.u32(dir + 12)? != 0x0F || r.u32(dir + 16 + 12)? != 0x0F {
        return None;
    }
    let mut lib = TypeLib {
        format: Format::Msft,
        guid: r.guid(h(2)?).unwrap_or_default(),
        lcid: h(4)? as u32,
        syskind: varflags & 0xF,
        major: h(6)? as u16,
        minor: (h(6)? as u32 >> 16) as u16,
        flags: h(7)? as u16 | LIBFLAG_FHASDISKIMAGE,
        name: r.name(h(14)?),
        doc: r.string(h(9)?),
        help_file: r.string(h(15)?),
        help_context: h(11)? as u32,
        infos: Vec::new(),
        imports: Vec::new(),
        refs: Vec::new(),
        dispatch_href: Some(h(19)? as u32).filter(|&d| d as i32 != -1),
    };
    read_imports(&r, &mut lib);
    for i in 0..infos {
        lib.infos.push(read_type(&r, i)?);
    }
    Some(lib)
}

/// The imported libraries, and the references to their types.
fn read_imports(r: &Reader, lib: &mut TypeLib) {
    let files = r.seg(IMP_FILES);
    // Each file: its GUID's offset, its LCID, version, and its name's
    // length (times 4) and name, padded to 4 bytes.
    let mut offsets = Vec::new();
    let mut at = files.offset;
    while at + 14 <= files.offset + files.len {
        let (Some(guid), Some(lcid), Some(major), Some(minor), Some(size)) = (
            r.i32(at),
            r.u32(at + 4),
            r.u16(at + 8),
            r.u16(at + 10),
            r.u16(at + 12),
        ) else {
            break;
        };
        let size = (size >> 2) as usize;
        let name = r.b.get(at + 14..at + 14 + size).unwrap_or_default();
        offsets.push(at - files.offset);
        lib.imports.push(ImportLib {
            guid: r.guid(guid).unwrap_or_default(),
            major,
            minor,
            lcid,
            file: ansi(r.page, name),
        });
        at = (at + 14 + size + 3) & !3;
    }
    let infos = r.seg(IMP_INFO);
    let mut offset = 0;
    while offset + 12 <= infos.len {
        let at = infos.offset + offset;
        let (Some(flags), Some(file), Some(target)) = (r.u32(at), r.i32(at + 4), r.i32(at + 8))
        else {
            break;
        };
        if let Some(lib_index) = offsets.iter().position(|&o| o as i32 == file) {
            let target = if flags & IMPINFO_OFFSET_IS_GUID != 0 {
                Target::Guid(r.guid(target).unwrap_or_default())
            } else {
                Target::Index(target as u32)
            };
            lib.refs.push((
                offset as u32,
                Ref::Import {
                    lib: lib_index,
                    target,
                },
            ));
        }
        offset += 12;
    }
}

/// The type at `index` in the types' table.
fn read_type(r: &Reader, index: usize) -> Option<TypeInfo> {
    let base = r.seg(TYPEINFO_TAB).offset + index * TYPEINFO;
    let f = |i: usize| r.i32(base + 4 * i);
    let typekind = f(0)? as u32;
    let elements = f(6)? as u32;
    let (funcs, vars) = ((elements & 0xFFFF) as usize, (elements >> 16) as usize);
    let impls_vft = f(19)? as u32;
    let impl_count = impls_vft & 0xFFFF;
    let kind = typekind & 0xF;
    let datatype1 = f(21)?;
    let mut info = TypeInfo {
        kind,
        guid: r.guid(f(11)?).unwrap_or_default(),
        flags: f(12)? as u16,
        major: f(14)? as u16,
        minor: (f(14)? as u32 >> 16) as u16,
        lcid: 0,
        size: f(20)? as u32,
        align: ((typekind >> 11) & 0x1F) as u16,
        vft: (impls_vft >> 16) as u16,
        name: r.name(f(13)?),
        doc: r.string(f(15)?),
        help_context: f(17)? as u32,
        dll: None,
        alias: None,
        impls: Vec::new(),
        funcs: Vec::new(),
        vars: Vec::new(),
        href: (index * TYPEINFO) as u32,
        refs: Vec::new(),
    };
    match kind {
        TKIND_ALIAS => info.alias = Some(r.typedesc(datatype1, 0)),
        TKIND_MODULE => info.dll = r.string(datatype1),
        _ => {}
    }
    if impl_count > 0 {
        match kind {
            TKIND_COCLASS => {
                // A chain of references in the references' table.
                let tab = r.seg(REF_TAB);
                let mut offset = datatype1;
                for _ in 0..impl_count {
                    if offset < 0 {
                        break;
                    }
                    let at = tab.offset + offset as usize;
                    info.impls.push(Impl {
                        href: r.u32(at)?,
                        flags: r.i32(at + 4)?,
                    });
                    offset = r.i32(at + 12)?;
                }
            }
            TKIND_DISPATCH if datatype1 == -1 => {}
            _ => info.impls.push(Impl {
                href: datatype1 as u32,
                flags: 0,
            }),
        }
    }
    let members = f(1)?;
    if funcs + vars > 0 && members >= 0 {
        read_members(r, &mut info, members as usize, funcs, vars)?;
    }
    Some(info)
}

/// A type's members, in their block at `offset`: the records, then the
/// members' ids, their names' offsets and their records' offsets.
fn read_members(
    r: &Reader,
    info: &mut TypeInfo,
    offset: usize,
    funcs: usize,
    vars: usize,
) -> Option<()> {
    let records = r.u32(offset)? as usize;
    let tables = offset + 4 + records;
    let memid = |i: usize| r.i32(tables + 4 * i);
    let name = |i: usize| r.i32(tables + 4 * (funcs + vars + i));
    let record = |i: usize| r.u32(tables + 4 * (2 * (funcs + vars) + i));
    let mut at = offset + 4;
    for i in 0..funcs {
        let len = (r.u32(at)? & 0xFFFF) as usize;
        let mut func = read_func(r, at, len)?;
        func.memid = memid(i)?;
        let name_offset = name(i)?;
        let previous = info.funcs.last();
        // The second of a property's get and put may have no name: the
        // first's is its.
        func.name = match previous {
            Some(p) if name_offset == -1 && is_property(p.invkind) && is_property(func.invkind) => {
                p.name.clone()
            }
            _ => r.name(name_offset),
        };
        info.funcs.push(func);
        at += len.max(4);
    }
    for i in 0..vars {
        let at = offset + 4 + record(funcs + i)? as usize;
        let mut var = read_var(r, at)?;
        var.memid = memid(funcs + i)?;
        var.name = r.name(name(funcs + i)?);
        info.vars.push(var);
    }
    Some(())
}

fn is_property(invkind: u32) -> bool {
    matches!(
        invkind,
        INVOKE_PROPERTYGET | INVOKE_PROPERTYPUT | INVOKE_PROPERTYPUTREF
    )
}

/// The function whose record of `len` bytes is at `at`.
fn read_func(r: &Reader, at: usize, len: usize) -> Option<Func> {
    let field = |offset: usize| r.i32(at + offset);
    let fkccic = field(16)? as u32;
    let args = r.u16(at + 20)? as usize;
    let opt = r.u16(at + 22)? as i16;
    let has_defaults = fkccic & 0x1000 != 0;
    // The record's optional fields end where the defaults and the
    // parameters' records start.
    let mut optional = len.saturating_sub(12 * args);
    if has_defaults {
        optional = optional.saturating_sub(4 * args);
    }
    let present = |offset: usize| optional > offset;
    let kind = fkccic & 7;
    let entry = if present(32) {
        let entry = field(32)?;
        if fkccic & 0x2000 != 0 {
            Some(Entry::Ordinal(entry as u16))
        } else {
            r.string(entry).map(Entry::Name)
        }
    } else {
        None
    };
    let mut func = Func {
        memid: 0,
        name: None,
        doc: if present(28) {
            r.string(field(28)?)
        } else {
            None
        },
        help_context: if present(24) { field(24)? as u32 } else { 0 },
        kind,
        invkind: (fkccic >> 3) & 0xF,
        callconv: (fkccic >> 8) & 0xF,
        opt,
        vft: if kind == FUNC_DISPATCH {
            0
        } else {
            (r.u16(at + 12)? & !1) as i16
        },
        flags: field(8)? as u16,
        ret: r.typedesc(field(4)?, 0),
        params: Vec::new(),
        entry,
    };
    // The parameters' records end the record; a record too short for
    // them isn't one.
    let params = at + len.checked_sub(12 * args)?;
    let defaults = if has_defaults {
        at + len.checked_sub(16 * args)?
    } else {
        params
    };
    for j in 0..args {
        let p = params + 12 * j;
        let flags = r.u32(p + 8)? as u16;
        let default = if flags & PARAMFLAG_FHASDEFAULT != 0 && has_defaults {
            r.value(r.i32(defaults + 4 * j)?)
        } else {
            None
        };
        func.params.push(Param {
            name: r.name(r.i32(p + 4)?),
            ty: r.typedesc(r.i32(p)?, 0),
            flags,
            default,
        });
    }
    Some(func)
}

/// The variable whose record is at `at`.
fn read_var(r: &Reader, at: usize) -> Option<Var> {
    let len = (r.u32(at)? & 0xFF) as usize;
    let field = |offset: usize| r.i32(at + offset);
    let kind = r.u16(at + 12)? as u32;
    let ty = r.typedesc(field(4)?, 0);
    let value = if kind == VAR_CONST {
        VarValue::Const(r.value(field(16)?)?)
    } else {
        VarValue::Offset(field(16)? as u32)
    };
    Some(Var {
        memid: 0,
        name: None,
        doc: if len > 24 { r.string(field(24)?) } else { None },
        help_context: if len > 20 { field(20)? as u32 } else { 0 },
        ty,
        flags: field(8)? as u16,
        kind,
        value,
    })
}
