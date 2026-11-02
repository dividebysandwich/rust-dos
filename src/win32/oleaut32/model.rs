//! A type library as OLEAUT32's objects show it, whichever format it was
//! read from: the readers of the two formats (msft.rs, sltg.rs) and the
//! built-in STDOLE2.TLB (stdole.rs) make one of these, and ITypeLib and
//! ITypeInfo (typelib.rs) answer from it.
//!
//! Strings are in Unicode, as the methods return them; the files have
//! them in the ANSI code page.

use crate::win32::com::Guid;

/// TYPEKIND.
pub const TKIND_ENUM: u32 = 0;
pub const TKIND_RECORD: u32 = 1;
pub const TKIND_MODULE: u32 = 2;
pub const TKIND_INTERFACE: u32 = 3;
pub const TKIND_DISPATCH: u32 = 4;
pub const TKIND_COCLASS: u32 = 5;
pub const TKIND_ALIAS: u32 = 6;
pub const TKIND_UNION: u32 = 7;

/// TYPEFLAGS.
pub const TYPEFLAG_FDUAL: u16 = 0x40;
pub const TYPEFLAG_FOLEAUTOMATION: u16 = 0x100;

/// FUNCKIND, INVOKEKIND and CALLCONV.
pub const FUNC_PUREVIRTUAL: u32 = 1;
pub const FUNC_DISPATCH: u32 = 4;
pub const INVOKE_FUNC: u32 = 1;
pub const INVOKE_PROPERTYGET: u32 = 2;
pub const INVOKE_PROPERTYPUT: u32 = 4;
pub const INVOKE_PROPERTYPUTREF: u32 = 8;
pub const CC_STDCALL: u32 = 4;

/// VARKIND.
pub const VAR_PERINSTANCE: u32 = 0;
pub const VAR_CONST: u32 = 2;
pub const VAR_DISPATCH: u32 = 3;

/// PARAMFLAGS.
pub const PARAMFLAG_FIN: u16 = 0x1;
pub const PARAMFLAG_FOUT: u16 = 0x2;
pub const PARAMFLAG_FLCID: u16 = 0x4;
pub const PARAMFLAG_FRETVAL: u16 = 0x8;
pub const PARAMFLAG_FOPT: u16 = 0x10;
pub const PARAMFLAG_FHASDEFAULT: u16 = 0x20;

/// LIBFLAGS: the library came from a file.
pub const LIBFLAG_FHASDISKIMAGE: u16 = 0x8;

/// VARTYPEs the type descriptions use.
pub const VT_EMPTY: u16 = 0;
pub const VT_I2: u16 = 2;
pub const VT_I4: u16 = 3;
pub const VT_BSTR: u16 = 8;
pub const VT_DISPATCH: u16 = 9;
pub const VT_ERROR: u16 = 10;
pub const VT_BOOL: u16 = 11;
pub const VT_VARIANT: u16 = 12;
pub const VT_UNKNOWN: u16 = 13;
pub const VT_I1: u16 = 16;
pub const VT_UI1: u16 = 17;
pub const VT_UI2: u16 = 18;
pub const VT_UI4: u16 = 19;
pub const VT_INT: u16 = 22;
pub const VT_UINT: u16 = 23;
pub const VT_VOID: u16 = 24;
pub const VT_HRESULT: u16 = 25;
pub const VT_PTR: u16 = 26;
pub const VT_SAFEARRAY: u16 = 27;
pub const VT_CARRAY: u16 = 28;
pub const VT_USERDEFINED: u16 = 29;
pub const VT_LPSTR: u16 = 30;
pub const VT_LPWSTR: u16 = 31;

/// MEMBERID_NIL.
pub const MEMBERID_NIL: i32 = -1;

/// The format a library was read from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// ICreateTypeLib2's, MIDL's: references are HREFTYPEs of the whole
    /// library.
    Msft,
    /// ICreateTypeLib's, MkTypLib's: each type has a table of its own
    /// references, which its HREFTYPEs number by fours.
    Sltg,
    /// The built-in STDOLE2.TLB, whose references are as MSFT's.
    Builtin,
}

/// A TYPEDESC.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeDesc {
    Base(u16),
    Ptr(Box<TypeDesc>),
    SafeArray(Box<TypeDesc>),
    /// The element type and the bounds: (elements, lower bound) each.
    CArray(Box<TypeDesc>, Vec<(u32, i32)>),
    /// A type of the library or of one it imports, by its HREFTYPE.
    User(u32),
}

impl TypeDesc {
    pub fn vt(&self) -> u16 {
        match self {
            TypeDesc::Base(vt) => *vt,
            TypeDesc::Ptr(_) => VT_PTR,
            TypeDesc::SafeArray(_) => VT_SAFEARRAY,
            TypeDesc::CArray(..) => VT_CARRAY,
            TypeDesc::User(_) => VT_USERDEFINED,
        }
    }

    pub fn ptr(to: TypeDesc) -> TypeDesc {
        TypeDesc::Ptr(Box::new(to))
    }
}

/// A constant's value or a parameter's default: a VARIANT.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// One of up to 8 bytes, as the VARIANT holds it.
    Bits(u16, u64),
    /// A BSTR, None for a null one.
    Str(Option<String>),
}

impl Value {
    pub fn int(vt: u16, value: i32) -> Value {
        Value::Bits(vt, value as u32 as u64)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Param {
    pub name: Option<String>,
    pub ty: TypeDesc,
    pub flags: u16,
    pub default: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Func {
    pub memid: i32,
    pub name: Option<String>,
    pub doc: Option<String>,
    pub help_context: u32,
    pub kind: u32,
    pub invkind: u32,
    pub callconv: u32,
    pub opt: i16,
    /// Its place in the vtable, in bytes.
    pub vft: i16,
    pub flags: u16,
    pub ret: TypeDesc,
    pub params: Vec<Param>,
    /// A module's function: its entry point in the DLL, a name or an
    /// ordinal.
    pub entry: Option<Entry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Name(String),
    Ordinal(u16),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VarValue {
    /// A member's offset in its structure.
    Offset(u32),
    Const(Value),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Var {
    pub memid: i32,
    pub name: Option<String>,
    pub doc: Option<String>,
    pub help_context: u32,
    pub ty: TypeDesc,
    pub flags: u16,
    pub kind: u32,
    pub value: VarValue,
}

/// An interface a class implements or an interface inherits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Impl {
    pub href: u32,
    pub flags: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeInfo {
    pub kind: u32,
    pub guid: Guid,
    pub flags: u16,
    pub major: u16,
    pub minor: u16,
    pub lcid: u32,
    pub size: u32,
    pub align: u16,
    /// The vtable's size in bytes, as the file has it.
    pub vft: u16,
    pub name: Option<String>,
    pub doc: Option<String>,
    pub help_context: u32,
    /// A module's DLL.
    pub dll: Option<String>,
    pub alias: Option<TypeDesc>,
    pub impls: Vec<Impl>,
    pub funcs: Vec<Func>,
    pub vars: Vec<Var>,
    /// Its own HREFTYPE in an MSFT library.
    pub href: u32,
    /// An SLTG type's references: HREFTYPE 4 * i is the i-th.
    pub refs: Vec<Ref>,
}

impl TypeInfo {
    pub fn is_dual(&self) -> bool {
        self.flags & TYPEFLAG_FDUAL != 0 && matches!(self.kind, TKIND_DISPATCH | TKIND_INTERFACE)
    }
}

/// A library a library takes types from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportLib {
    pub guid: Guid,
    pub major: u16,
    pub minor: u16,
    pub lcid: u32,
    /// Its file, as the library that imports it names it.
    pub file: String,
}

/// What a HREFTYPE refers to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ref {
    /// The type at this place in the same library.
    Local(usize),
    /// A type of the imported library `lib` (a place in
    /// `TypeLib::imports`).
    Import { lib: usize, target: Target },
}

/// A type of an imported library: by its place or its GUID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Index(u32),
    Guid(Guid),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeLib {
    pub format: Format,
    pub guid: Guid,
    pub lcid: u32,
    pub syskind: u32,
    pub major: u16,
    pub minor: u16,
    pub flags: u16,
    pub name: Option<String>,
    pub doc: Option<String>,
    pub help_file: Option<String>,
    pub help_context: u32,
    pub infos: Vec<TypeInfo>,
    pub imports: Vec<ImportLib>,
    /// An MSFT library's references to imported types, by their
    /// HREFTYPEs without the low two bits.
    pub refs: Vec<(u32, Ref)>,
    /// The HREFTYPE of IDispatch, which dispinterfaces inherit from.
    pub dispatch_href: Option<u32>,
}

/// The bit of a HREFTYPE that asks for the interface side of a dual
/// interface.
pub const HREF_INTERFACE: u32 = 0x2;

impl TypeLib {
    /// The type the HREFTYPE `href` of the type at `info` refers to. In
    /// an MSFT library the low bit tells an imported type's (the place of
    /// its reference) from a type of the library's own (the place of its
    /// entry).
    pub fn find_ref(&self, info: usize, href: u32) -> Option<Ref> {
        let place = href & !3;
        if self.format == Format::Sltg {
            let refs = &self.infos.get(info)?.refs;
            return refs.get((place / 4) as usize).cloned();
        }
        if href & 1 != 0 {
            let (_, r) = self.refs.iter().find(|(h, _)| *h == place)?;
            return Some(r.clone());
        }
        self.infos
            .iter()
            .position(|t| t.href == place)
            .map(Ref::Local)
    }

    /// The type with the GUID `guid`.
    pub fn find_guid(&self, guid: &Guid) -> Option<usize> {
        self.infos.iter().position(|t| t.guid == *guid)
    }
}

/// The Unicode of the ANSI string `bytes`.
pub fn ansi(bytes: &[u8]) -> String {
    String::from_utf16_lossy(&crate::win32::kernel32::ansi_to_wide(bytes))
}

/// The case-insensitive comparison names are found by, as
/// LHashValOfNameSys and lstrcmpiW have it for the ANSI code page's
/// letters.
pub fn same_name(a: &str, b: &str) -> bool {
    let fold = |s: &str| -> Vec<char> { s.chars().flat_map(char::to_lowercase).collect() };
    fold(a) == fold(b)
}
