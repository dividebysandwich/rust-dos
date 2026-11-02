//! The SLTG format of type libraries, which ICreateTypeLib and the
//! MkTypLib of Windows 95's time write, as ActiveMovie's AMOVIE.OCX,
//! QUARTZ.DLL and OLEPRO32.DLL have them: a directory of blocks, one for
//! each type and the library's last, a table of names, and help strings
//! packed with a table of words.
//!
//! The layout follows Wine's oleaut32/typelib.c and typelib.h (LGPL 2.1
//! or later), with what Windows' own OLEAUT32 shows for the same file
//! where the two differ: a type's references are its own, numbered by
//! fours from 0, and a constant's value is a VT_I4.
//!
//! Copyright 1997 Marcus Meissner
//! Copyright 1999 Rein Klazes
//! Copyright 2000 Francois Jacques
//! Copyright 2001 Huw D M Davies for CodeWeavers
//! Copyright 2004 Alastair Bridgewater
//! Copyright 2005 Robert Shearman, for CodeWeavers

use super::model::*;
use crate::win32::com::Guid;

pub const SIGNATURE: &[u8; 4] = b"SLTG";

const LIBBLK_MAGIC: u16 = 0x51CC;
const TIHEADER_MAGIC: u16 = 0x0501;
const IMPL_MAGIC: u16 = 0x004A;
const REF_MAGIC: u8 = 0xDF;
const FUNCTION_FLAGS_PRESENT: u8 = 0x20;
const VAR_MAGIC: u8 = 0x0A;
const VAR_WITH_FLAGS_MAGIC: u8 = 0x2A;

/// The header: its size, and where the block directory starts.
const HEADER: usize = 0x24;

struct Reader<'a> {
    b: &'a [u8],
    /// Where the names' table starts.
    names: usize,
    /// The help strings' table of words, from its 16-bit length on.
    help: usize,
}

impl Reader<'_> {
    fn u8(&self, at: usize) -> Option<u8> {
        self.b.get(at).copied()
    }

    fn u16(&self, at: usize) -> Option<u16> {
        Some(u16::from_le_bytes(self.b.get(at..at + 2)?.try_into().ok()?))
    }

    fn u32(&self, at: usize) -> Option<u32> {
        Some(u32::from_le_bytes(self.b.get(at..at + 4)?.try_into().ok()?))
    }

    /// The zero-terminated string at `at`.
    fn cstr(&self, at: usize) -> Option<&[u8]> {
        let rest = self.b.get(at..)?;
        Some(&rest[..rest.iter().position(|&c| c == 0)?])
    }

    /// The name at `offset` in the names' table.
    fn name(&self, offset: u16) -> Option<String> {
        self.cstr(self.names + offset as usize).map(ansi)
    }

    /// A string as SLTG keeps them: its length, or FFFFh for none, then
    /// its bytes. The string and the bytes it takes.
    fn string(&self, at: usize) -> Option<(Option<String>, usize)> {
        let len = self.u16(at)?;
        if len == 0xFFFF {
            return Some((None, 2));
        }
        let bytes = self.b.get(at + 2..at + 2 + len as usize)?;
        Some((Some(ansi(bytes)), 2 + len as usize))
    }

    /// A help string: words of the table, picked by the bits of the
    /// `len` bytes at `at`, with spaces between them.
    fn help_string(&self, at: usize, len: usize) -> Option<String> {
        if len == 0 {
            return None;
        }
        let size = self.u32(self.help + 2)? as usize;
        let table = self.b.get(self.help + 6..self.help + 6 + size)?;
        let stream = self.b.get(at..(at + len).min(self.b.len()))?;
        let mut bits = Bits { stream, current: 0 };
        let mut out = String::new();
        while let Some(word) = lookup_code(table, &mut bits) {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(&ansi(word));
        }
        Some(out)
    }
}

struct Bits<'a> {
    stream: &'a [u8],
    current: u16,
}

/// The next word of the help strings' table, by walking its tree with
/// the stream's bits.
fn lookup_code<'t>(table: &'t [u8], bits: &mut Bits) -> Option<&'t [u8]> {
    let mut p = 0;
    while p < table.len() && table[p] == 0x80 {
        if p + 2 >= table.len() {
            return None;
        }
        if bits.current & 0xFF == 0 {
            let (&next, rest) = bits.stream.split_first()?;
            bits.current = (next as u16) << 8 | 1;
            bits.stream = rest;
        }
        if bits.current & 0x8000 != 0 {
            p += 3;
        } else {
            p = (table[p + 2] as usize) | (table[p + 1] as usize) << 8;
        }
        bits.current <<= 1;
    }
    let word = table.get(p + 1..)?;
    let end = word.iter().position(|&c| c == 0)?;
    (end > 0).then(|| &word[..end])
}

/// Read the SLTG library `b`; None if it isn't one.
pub fn read(b: &[u8]) -> Option<TypeLib> {
    if b.get(..4)? != SIGNATURE {
        return None;
    }
    let mut r = Reader {
        b,
        names: 0,
        help: 0,
    };
    let blocks = r.u16(4)? as usize;
    let first = r.u16(10)? as usize;
    if blocks < 2 {
        return None;
    }
    let entry = |i: usize| HEADER + 8 * i;
    let magic = HEADER + 8 * (blocks - 1);
    if b.get(magic..magic + 9)? != b"\x01CompObj\0" || b.get(magic + 9..magic + 12)? != b"dir" {
        return None;
    }
    let first_block = magic + 13 + 11 * (blocks - 2) + 9;
    // The blocks in their order, by the directory's links: the types',
    // then the library's.
    let mut order = Vec::new();
    let (mut at, mut i) = (first_block, first.checked_sub(1)?);
    loop {
        let len = r.u32(entry(i))? as usize;
        let next = r.u16(entry(i) + 6)? as usize;
        order.push(at);
        if next == 0 || order.len() > blocks {
            break;
        }
        at += len;
        i = next - 1;
    }
    let lib_block = order.pop()?;

    // The library's block.
    if r.u16(lib_block)? != LIBBLK_MAGIC {
        return None;
    }
    let name_offset = r.u16(lib_block + 4)?;
    let mut p = lib_block + 6;
    let skip = r.u16(p)?;
    if skip != 0xFFFF {
        p += skip as usize;
    }
    p += 2;
    let (doc, n) = r.string(p)?;
    p += n;
    let (help_file, n) = r.string(p)?;
    p += n;
    let help_context = r.u32(p)?;
    let syskind = r.u16(p + 4)? as u32;
    let lcid_word = r.u16(p + 6)? as u32;
    let flags = r.u16(p + 12)?;
    let major = r.u16(p + 14)?;
    let minor = r.u16(p + 16)?;
    let guid = Guid(b.get(p + 18..p + 34)?.try_into().ok()?);
    p += 34 + 0x40;
    // A library's LCID is the language of its block's when that is a
    // neutral one, else 0, as Wine found.
    let lcid = if lcid_word >> 10 == 0 {
        lcid_word & 0x3FF
    } else {
        0
    };
    let count = r.u16(p)? as usize;
    p += 2;
    let mut others = Vec::new();
    for _ in 0..count {
        let other = read_other(&r, p)?;
        p = other.end;
        others.push(other);
    }
    r.help = p + 4;
    let mut names = lib_block + r.u32(p)? as usize;
    if r.u16(names)? == 0x0200 {
        names += 0x20;
    }
    r.names = names + 0x216 + 2;

    let mut lib = TypeLib {
        format: Format::Sltg,
        guid,
        lcid,
        syskind,
        major,
        minor,
        flags,
        name: r.name(name_offset),
        doc,
        help_file,
        help_context,
        infos: Vec::new(),
        imports: Vec::new(),
        refs: Vec::new(),
        dispatch_href: None,
    };
    if order.len() != others.len() {
        return None;
    }
    for (block, other) in order.into_iter().zip(&others) {
        let mut info = read_type(&r, &mut lib, block)?;
        info.name = r.name(other.name);
        info.doc = r.help_string(other.extra, other.extra_len);
        info.help_context = other.help_context;
        info.guid = other.guid;
        info.lcid = lcid_word;
        lib.infos.push(info);
    }
    Some(lib)
}

/// A type's entry in the library's block.
struct Other {
    name: u16,
    /// Its help string's bits, and how many bytes they take.
    extra: usize,
    extra_len: usize,
    help_context: u32,
    guid: Guid,
    /// Where the next entry starts.
    end: usize,
}

fn read_other(r: &Reader, p: usize) -> Option<Other> {
    let mut len = 0;
    let w = r.u16(p)?;
    if w != 0xFFFF {
        len += w as usize;
    }
    let w = r.u16(p + 2 + len)?;
    if w != 0xFFFF {
        len += w as usize;
    }
    let name = r.u16(p + len + 6)?;
    let extra_len = r.u16(p + len + 8)? as usize;
    let extra = p + 10 + len;
    len += extra_len;
    Some(Other {
        name,
        extra,
        extra_len,
        help_context: r.u32(p + 12 + len)?,
        guid: Guid(r.b.get(p + 18 + len..p + 34 + len)?.try_into().ok()?),
        end: p + len + 36,
    })
}

/// The type whose block is at `block`.
fn read_type(r: &Reader, lib: &mut TypeLib, block: usize) -> Option<TypeInfo> {
    if r.u16(block)? != TIHEADER_MAGIC {
        return None;
    }
    let href_table = r.u32(block + 2)?;
    let elem_table = r.u32(block + 0x0A)? as usize;
    let flags = (r.u8(block + 0x1A)? >> 3) as u16 | (r.u8(block + 0x1B)? as u16) << 5;
    let stored_kind = r.u8(block + 0x1D)? as u32;
    let members_header = block + elem_table;
    let members = members_header + 9;
    let tail = members + r.u32(members_header + 5)? as usize;
    let t = |offset: usize| r.u16(tail + offset);
    let mut info = TypeInfo {
        kind: if flags & TYPEFLAG_FDUAL != 0 {
            TKIND_DISPATCH
        } else {
            stored_kind
        },
        guid: Guid::default(),
        flags,
        major: r.u16(block + 0x12)?,
        minor: r.u16(block + 0x14)?,
        lcid: 0,
        size: t(0x20)? as u32,
        align: t(0x22)?,
        vft: t(0x28)?,
        name: None,
        doc: None,
        help_context: 0,
        dll: None,
        alias: None,
        impls: Vec::new(),
        funcs: Vec::new(),
        vars: Vec::new(),
        href: 0,
        refs: Vec::new(),
    };
    if href_table != 0xFFFF_FFFF {
        info.refs = read_refs(r, lib, block + href_table as usize)?;
    }
    let (funcs, vars) = (t(0)? as usize, t(2)? as usize);
    let (funcs_off, vars_off, impls_off) = (t(8)?, t(0x0A)?, t(0x0C)?);
    let m = Members { r, block: members };
    match stored_kind {
        TKIND_ENUM | TKIND_RECORD | TKIND_UNION => {
            if vars_off != 0xFFFF {
                info.vars = m.vars(members + vars_off as usize, vars)?;
            }
        }
        TKIND_INTERFACE => {
            if r.u16(members)? == IMPL_MAGIC {
                info.impls = m.impls(members)?;
            }
            if funcs_off != 0xFFFF {
                info.funcs = m.funcs(members + funcs_off as usize, funcs)?;
            }
        }
        TKIND_COCLASS => {
            if r.u16(members)? == IMPL_MAGIC {
                info.impls = m.impls(members)?;
            }
        }
        TKIND_ALIAS => {
            let vt = t(0x14)?;
            info.alias = Some(if t(0x1C)? != 0 {
                TypeDesc::Base(vt)
            } else {
                m.typedesc(members + vt as usize)?.0
            });
        }
        TKIND_DISPATCH | TKIND_MODULE => {
            if vars_off != 0xFFFF {
                info.vars = m.vars(members + vars_off as usize, vars)?;
            }
            if funcs_off != 0xFFFF {
                info.funcs = m.funcs(members + funcs_off as usize, funcs)?;
            }
            if stored_kind == TKIND_DISPATCH && impls_off != 0xFFFF {
                // A dispinterface's record keeps its count of variables
                // where an interface's keeps the reference (OLEPRO32's
                // Font has 8 there, Picture 5), and Windows takes the
                // type's first reference, IDispatch, for it.
                info.impls = m.impls(members + impls_off as usize)?;
                for i in &mut info.impls {
                    i.href = 0;
                }
            }
        }
        _ => {}
    }
    Some(info)
}

/// A type's references, at `at`: "*\Rxxxx*#n" each, the n-th type of
/// this library when xxxx is FFFF, else of the library whose
/// "*\G{guid}#1.0#0#file#" is at xxxx in the names' table.
fn read_refs(r: &Reader, lib: &mut TypeLib, at: usize) -> Option<Vec<Ref>> {
    if r.u8(at)? != REF_MAGIC {
        return None;
    }
    let number = r.u32(at + 0x44)? as usize;
    let mut p = at + 0x4F + number;
    let mut refs = Vec::new();
    for _ in 0..number / 8 {
        let (name, n) = r.string(p)?;
        p += n;
        let name = name.unwrap_or_default();
        let rest = name.strip_prefix("*\\R")?;
        let (lib_offset, num) = rest.split_once("*#")?;
        let lib_offset = u32::from_str_radix(lib_offset, 16).ok()?;
        let num = u32::from_str_radix(num, 16).ok()?;
        if lib_offset == 0xFFFF {
            refs.push(Ref::Local(num as usize));
            continue;
        }
        let import = read_import(r, r.names + lib_offset as usize)?;
        let index = match lib.imports.iter().position(|i| *i == import) {
            Some(i) => i,
            None => {
                lib.imports.push(import);
                lib.imports.len() - 1
            }
        };
        refs.push(Ref::Import {
            lib: index,
            target: Target::Index(num),
        });
    }
    Some(refs)
}

/// The imported library "*\G{guid}#major.minor#lcid#file#" at `at`.
fn read_import(r: &Reader, at: usize) -> Option<ImportLib> {
    let text = ansi(r.cstr(at)?);
    let rest = text.strip_prefix("*\\G")?;
    let (guid, rest) = rest.split_at(rest.find('}')? + 1);
    let mut parts = rest.strip_prefix('#')?.split('#');
    let (major, minor) = parts.next()?.split_once('.')?;
    let lcid = u32::from_str_radix(parts.next()?, 16).ok()?;
    let file = parts.next()?.to_string();
    Some(ImportLib {
        guid: Guid::parse(guid)?,
        major: major.parse().ok()?,
        minor: minor.parse().ok()?,
        lcid,
        file,
    })
}

/// A type's member block, from which the offsets in it count.
struct Members<'a, 'b> {
    r: &'a Reader<'b>,
    block: usize,
}

impl Members<'_, '_> {
    /// The type description whose words start at `at`, and where the
    /// words after it start.
    fn typedesc(&self, at: usize) -> Option<(TypeDesc, usize)> {
        self.typedesc_at(at, 0)
    }

    fn typedesc_at(&self, at: usize, depth: u32) -> Option<(TypeDesc, usize)> {
        if depth > 16 {
            return None;
        }
        let w = self.r.u16(at)?;
        let pointer = w & 0xE00 == 0xE00;
        let (desc, next) = match w & 0x3F {
            VT_PTR => {
                let (inner, next) = self.typedesc_at(at + 2, depth + 1)?;
                (TypeDesc::ptr(inner), next)
            }
            VT_USERDEFINED => (TypeDesc::User(self.r.u16(at + 2)? as u32 & !3), at + 4),
            VT_CARRAY => {
                let array = self.block + self.r.u16(at + 2)? as usize;
                let dims = self.r.u16(array)? as usize;
                let bounds = (0..dims)
                    .map(|i| {
                        let b = array + 16 + 8 * i;
                        Some((self.r.u32(b)?, self.r.u32(b + 4)? as i32))
                    })
                    .collect::<Option<Vec<_>>>()?;
                let (elem, next) = self.typedesc_at(at + 4, depth + 1)?;
                (TypeDesc::CArray(Box::new(elem), bounds), next)
            }
            VT_SAFEARRAY => {
                let (elem, next) = self.typedesc_at(at + 4, depth + 1)?;
                (TypeDesc::SafeArray(Box::new(elem)), next)
            }
            vt => (TypeDesc::Base(vt), at + 2),
        };
        Some((if pointer { TypeDesc::ptr(desc) } else { desc }, next))
    }

    /// A parameter's or a return's type and its flags, whose words start
    /// at `at`.
    fn elem(&self, at: usize) -> Option<(TypeDesc, u16, usize)> {
        let w = self.r.u16(at)?;
        let mut flags = if w & 0xC000 == 0xC000 {
            0
        } else if w & 0x8000 != 0 {
            PARAMFLAG_FIN | PARAMFLAG_FOUT
        } else if w & 0x4000 != 0 {
            PARAMFLAG_FOUT
        } else {
            PARAMFLAG_FIN
        };
        if w & 0x2000 != 0 {
            flags |= PARAMFLAG_FLCID;
        }
        if w & 0x80 != 0 {
            flags |= PARAMFLAG_FRETVAL;
        }
        let (desc, next) = self.typedesc(at)?;
        Some((desc, flags, next))
    }

    /// The implemented interfaces, a chain from `at`.
    fn impls(&self, at: usize) -> Option<Vec<Impl>> {
        let mut out = Vec::new();
        let mut p = at;
        loop {
            out.push(Impl {
                href: self.r.u16(p + 0x0A)? as u32 * 4,
                flags: self.r.u8(p + 6)? as i32,
            });
            let next = self.r.u16(p + 2)?;
            if next == 0xFFFF || out.len() > 0x400 {
                break;
            }
            p = self.block + next as usize;
        }
        Some(out)
    }

    /// `count` functions, a chain from `at`.
    fn funcs(&self, at: usize, count: usize) -> Option<Vec<Func>> {
        let r = self.r;
        let mut out = Vec::new();
        let mut p = at;
        for _ in 0..count {
            let magic = r.u8(p)?;
            let kind = match magic & !FUNCTION_FLAGS_PRESENT {
                0x4C => FUNC_PUREVIRTUAL,
                0xCB => FUNC_DISPATCH,
                0x8B => 3,
                _ => return None,
            };
            let nacc = r.u8(p + 0x10)?;
            let retnextopt = r.u8(p + 0x11)?;
            let help = r.u16(p + 0x0C)?;
            let name = r.name(r.u16(p + 4)?);
            let ret_at = if retnextopt & 0x80 != 0 {
                p + 0x12
            } else {
                self.block + r.u16(p + 0x12)? as usize
            };
            let mut func = Func {
                memid: r.u32(p + 6)? as i32,
                name: name.clone(),
                doc: if help != 0xFFFF {
                    r.help_string(
                        self.block + help as usize,
                        r.names.saturating_sub(self.block),
                    )
                } else {
                    None
                },
                help_context: 0,
                kind,
                invkind: (r.u8(p + 1)? >> 4) as u32,
                callconv: (nacc & 7) as u32,
                opt: ((retnextopt & 0x7E) >> 1) as i16,
                vft: if kind == FUNC_DISPATCH {
                    0
                } else {
                    (r.u16(p + 0x14)? & !1) as i16
                },
                flags: if magic & FUNCTION_FLAGS_PRESENT != 0 {
                    r.u16(p + 0x16)?
                } else {
                    0
                },
                ret: self.elem(ret_at)?.0,
                params: Vec::new(),
                entry: None,
            };
            let params = (nacc >> 3) as usize;
            let mut arg = self.block + r.u16(p + 0x0E)? as usize;
            for j in 0..params {
                let w = r.u16(arg)?;
                let param_name = if w & !1 == 0xFFFE {
                    name.clone()
                } else {
                    r.name(w & !1)
                };
                arg += 2;
                let (ty, mut flags) = if w & 1 == 0 {
                    // The next word is the type's offset.
                    let (ty, flags, _) = self.elem(self.block + r.u16(arg)? as usize)?;
                    arg += 2;
                    (ty, flags)
                } else {
                    let (ty, flags, next) = self.elem(arg)?;
                    arg = next;
                    (ty, flags)
                };
                if (params - j) as i16 <= func.opt {
                    flags |= PARAMFLAG_FOPT;
                }
                func.params.push(Param {
                    name: param_name,
                    ty,
                    flags,
                    default: None,
                });
            }
            out.push(func);
            let next = r.u16(p + 2)?;
            if next == 0xFFFF {
                break;
            }
            p = self.block + next as usize;
        }
        Some(out)
    }

    /// `count` variables, a chain from `at`.
    fn vars(&self, at: usize, count: usize) -> Option<Vec<Var>> {
        let r = self.r;
        let mut out: Vec<Var> = Vec::new();
        let mut p = at;
        for _ in 0..count {
            let magic = r.u8(p)?;
            if magic != VAR_MAGIC && magic != VAR_WITH_FLAGS_MAGIC {
                return None;
            }
            let flags = r.u8(p + 1)?;
            let name = r.u16(p + 4)?;
            let byte_offs = r.u16(p + 6)?;
            let ty_at = if flags & 0x02 != 0 {
                p + 8
            } else {
                self.block + r.u16(p + 8)? as usize
            };
            let (ty, _, _) = self.elem(ty_at)?;
            let help = r.u16(p + 0x10)?;
            let (kind, value) = if flags & 0x40 != 0 {
                (VAR_DISPATCH, VarValue::Offset(byte_offs as u32))
            } else if flags & 0x10 != 0 {
                let at = self.block + byte_offs as usize;
                let value = if flags & 0x08 != 0 {
                    Value::int(VT_I4, byte_offs as i32)
                } else {
                    match ty.vt() {
                        VT_LPSTR | VT_LPWSTR | VT_BSTR => Value::Str(r.string(at)?.0),
                        _ => Value::int(VT_I4, r.u32(at)? as i32),
                    }
                };
                (VAR_CONST, VarValue::Const(value))
            } else {
                (VAR_PERINSTANCE, VarValue::Offset(byte_offs as u32))
            };
            let mut var_flags = if magic == VAR_WITH_FLAGS_MAGIC {
                r.u16(p + 0x12)?
            } else {
                0
            };
            if flags & 0x80 != 0 {
                var_flags |= 1;
            }
            out.push(Var {
                memid: r.u32(p + 0x0A)? as i32,
                name: match (name, out.last()) {
                    (0xFFFE, Some(prev)) => prev.name.clone(),
                    _ => r.name(name),
                },
                doc: if help != 0xFFFF {
                    r.help_string(
                        self.block + help as usize,
                        r.names.saturating_sub(self.block),
                    )
                } else {
                    None
                },
                help_context: 0,
                ty,
                flags: var_flags,
                kind,
                value,
            });
            let next = r.u16(p + 2)?;
            if next == 0xFFFF {
                break;
            }
            p = self.block + next as usize;
        }
        Some(out)
    }
}
