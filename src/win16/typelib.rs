//! TYPELIB, 16-bit OLE automation's type libraries: LoadTypeLib and the
//! functions around it, and the ITypeLib and ITypeInfo objects that show
//! a library to the program. The ordinals and names are Windows 98 SE's
//! TYPELIB.DLL's (version 2.03.3024, which OaBuildVersion gives), as
//! Wine's typelib.dll16 spec file has them; the arguments are those of
//! the Windows 3.1 SDK's dispatch.h.
//!
//! The libraries are read by the 32-bit OLEAUT32's readers
//! (win32/oleaut32: sltg.rs for MkTypLib's and Visual Basic's format,
//! msft.rs, and STDOLE.TLB built in, from stdole.rs), into its model, and
//! the 16-bit structures are made from that. The objects live in the
//! program's memory, blocks of the built-in task allocator (ole2.rs): a
//! library's holds the library's bytes, a type's its library object and
//! its place, so the machine's saved state has them with no state of
//! this module's own. The model is read again from the bytes for each
//! call; the libraries programs bring are a few kilobytes.
//!
//! Windows 95 and 98 have TYPELIB.DLL in the system directory and name
//! it among Known16DLLs, so a program's older copy beside it doesn't
//! load in its place: Visual Basic 4's setup programs bring version
//! 2.02, whose LoadTypeLib Visual Basic 4's runtime refuses on Windows
//! 95. This module is that system copy, and module.rs finds a built-in
//! module before any file of the name.
//!
//! ITypeInfo's Invoke is in typelib_invoke.rs.
//!
//! What isn't here: CreateTypeLib and the ICreateTypeLib and
//! ICreateTypeInfo interfaces, ITypeComp, ITypeInfo's AddressOfMember
//! and CreateInstance (E_NOTIMPL), and a dual interface's dispinterface
//! side, which the 32-bit ITypeInfo makes: a 16-bit type's functions are
//! those the library has for it.

use super::builtin::{Def, cdecl, missing, rust, variable};
use super::ole2::{self, ole_guid};
use super::ole2disp::alloc_bstr;
use crate::cpu::Cpu;
use crate::win32::com::{E_INVALIDARG, E_NOINTERFACE, E_NOTIMPL, E_OUTOFMEMORY, Guid, S_OK};
use crate::win32::nls::CodePage;
use crate::win32::oleaut32::model::*;
use crate::win32::oleaut32::stdole;
use crate::win32::oleaut32::typelib::{
    self as tl, DISP_E_UNKNOWNNAME, TYPE_E_CANTLOADLIBRARY, TYPE_E_ELEMENTNOTFOUND,
};
use rust_dos_ne as ne;

/// OaBuildVersion: Windows 98 SE's TYPELIB.DLL, 2.03.3024, as its major
/// version 10 and its build 3024.
const BUILD_VERSION: u32 = 0x000A_0BD0;

const IID_IUNKNOWN: [u8; 16] = ole_guid(0);
const IID_ITYPEINFO: [u8; 16] = ole_guid(0x0002_0401);
const IID_ITYPELIB: [u8; 16] = ole_guid(0x0002_0402);

/// The 16-bit STDOLE, which the built-in model serves when there is no
/// file of it.
const STDOLE16: &str = "STDOLE.TLB";

/// TYPE_E_BADMODULEKIND.
const TYPE_E_BADMODULEKIND: u32 = 0x8002_88BD;

/// The most interfaces a name is looked for in, one inheriting from the
/// next: a malformed library can make an interface its own base.
const MAX_BASES: usize = 16;

pub static TYPELIB: Def = Def {
    name: "TYPELIB",
    file: "TYPELIB.DLL",
    apis: &[
        rust("CreateTypeLib", 2, "wpp", create_type_lib),
        rust("LoadTypeLib", 3, "pp", load_type_lib),
        rust("LHashValOfNameSys", 4, "wlp", lhash_val_of_name_sys),
        variable("_IID_ICreateTypeInfo", 5, &ole_guid(0x0002_0405)),
        variable("_IID_ICreateTypeLib", 6, &ole_guid(0x0002_0406)),
        variable("_IID_ITypeComp", 7, &ole_guid(0x0002_0403)),
        variable("_IID_ITypeInfo", 8, &IID_ITYPEINFO),
        variable("_IID_ITypeLib", 9, &IID_ITYPELIB),
        rust("RegisterTypeLib", 10, "lpp", register_type_lib),
        rust("LoadRegTypeLib", 11, "pwwlp", load_reg_type_lib),
        missing("___EXPORTEDSTUB", 12),
        rust(
            "QueryPathOfRegTypeLib",
            14,
            "pwwlp",
            query_path_of_reg_type_lib,
        ),
        rust("OaBuildVersion", 15, "", oa_build_version),
        // ITypeLib's methods, which aren't exported.
        cdecl("ITypeLib_QueryInterface", 0, "lpp", lib_query_interface),
        cdecl("ITypeLib_AddRef", 0, "l", lib_add_ref),
        cdecl("ITypeLib_Release", 0, "l", lib_release),
        cdecl("ITypeLib_GetTypeInfoCount", 0, "l", get_type_info_count),
        cdecl("ITypeLib_GetTypeInfo", 0, "lwp", get_type_info),
        cdecl("ITypeLib_GetTypeInfoType", 0, "lwp", get_type_info_type),
        cdecl(
            "ITypeLib_GetTypeInfoOfGuid",
            0,
            "lpp",
            get_type_info_of_guid,
        ),
        cdecl("ITypeLib_GetLibAttr", 0, "lp", get_lib_attr),
        cdecl("ITypeLib_GetTypeComp", 0, "lp", get_type_comp),
        cdecl(
            "ITypeLib_GetDocumentation",
            0,
            "lwpppp",
            lib_get_documentation,
        ),
        cdecl("ITypeLib_IsName", 0, "lplp", is_name),
        cdecl("ITypeLib_FindName", 0, "lplppp", find_name),
        cdecl("ITypeLib_ReleaseTLibAttr", 0, "ll", release_block),
        // ITypeInfo's.
        cdecl("ITypeInfo_QueryInterface", 0, "lpp", info_query_interface),
        cdecl("ITypeInfo_AddRef", 0, "l", info_add_ref),
        cdecl("ITypeInfo_Release", 0, "l", info_release),
        cdecl("ITypeInfo_GetTypeAttr", 0, "lp", get_type_attr),
        cdecl("ITypeInfo_GetTypeComp", 0, "lp", get_type_comp),
        cdecl("ITypeInfo_GetFuncDesc", 0, "lwp", get_func_desc),
        cdecl("ITypeInfo_GetVarDesc", 0, "lwp", get_var_desc),
        cdecl("ITypeInfo_GetNames", 0, "llpwp", get_names),
        cdecl(
            "ITypeInfo_GetRefTypeOfImplType",
            0,
            "lwp",
            get_ref_type_of_impl_type,
        ),
        cdecl("ITypeInfo_GetImplTypeFlags", 0, "lwp", get_impl_type_flags),
        cdecl("ITypeInfo_GetIDsOfNames", 0, "lpwp", get_ids_of_names),
        cdecl(
            "ITypeInfo_Invoke",
            0,
            "lllwllll",
            super::typelib_invoke::invoke,
        ),
        cdecl(
            "ITypeInfo_GetDocumentation",
            0,
            "llpppp",
            info_get_documentation,
        ),
        cdecl("ITypeInfo_GetDllEntry", 0, "llwppp", get_dll_entry),
        cdecl("ITypeInfo_GetRefTypeInfo", 0, "llp", get_ref_type_info),
        cdecl("ITypeInfo_AddressOfMember", 0, "llwp", address_of_member),
        cdecl("ITypeInfo_CreateInstance", 0, "llpp", create_instance),
        cdecl("ITypeInfo_GetMops", 0, "llp", get_mops),
        cdecl(
            "ITypeInfo_GetContainingTypeLib",
            0,
            "lpp",
            get_containing_type_lib,
        ),
        cdecl("ITypeInfo_ReleaseTypeAttr", 0, "ll", release_block),
        cdecl("ITypeInfo_ReleaseFuncDesc", 0, "ll", release_block),
        cdecl("ITypeInfo_ReleaseVarDesc", 0, "ll", release_block),
    ],
    library: true,
};

const LIB_METHODS: [&str; 13] = [
    "ITypeLib_QueryInterface",
    "ITypeLib_AddRef",
    "ITypeLib_Release",
    "ITypeLib_GetTypeInfoCount",
    "ITypeLib_GetTypeInfo",
    "ITypeLib_GetTypeInfoType",
    "ITypeLib_GetTypeInfoOfGuid",
    "ITypeLib_GetLibAttr",
    "ITypeLib_GetTypeComp",
    "ITypeLib_GetDocumentation",
    "ITypeLib_IsName",
    "ITypeLib_FindName",
    "ITypeLib_ReleaseTLibAttr",
];

const INFO_METHODS: [&str; 22] = [
    "ITypeInfo_QueryInterface",
    "ITypeInfo_AddRef",
    "ITypeInfo_Release",
    "ITypeInfo_GetTypeAttr",
    "ITypeInfo_GetTypeComp",
    "ITypeInfo_GetFuncDesc",
    "ITypeInfo_GetVarDesc",
    "ITypeInfo_GetNames",
    "ITypeInfo_GetRefTypeOfImplType",
    "ITypeInfo_GetImplTypeFlags",
    "ITypeInfo_GetIDsOfNames",
    "ITypeInfo_Invoke",
    "ITypeInfo_GetDocumentation",
    "ITypeInfo_GetDllEntry",
    "ITypeInfo_GetRefTypeInfo",
    "ITypeInfo_AddressOfMember",
    "ITypeInfo_CreateInstance",
    "ITypeInfo_GetMops",
    "ITypeInfo_GetContainingTypeLib",
    "ITypeInfo_ReleaseTypeAttr",
    "ITypeInfo_ReleaseFuncDesc",
    "ITypeInfo_ReleaseVarDesc",
];

// The objects. Each starts with the 16:16 address of its vtable, which
// follows its fields in the same block, and its count of references.

const REFS: u32 = 4;
/// A library's: the built-in STDOLE's version, 0 for one read from a
/// file, and the block of the library's bytes and their count.
const LIB_BUILTIN: u32 = 6;
const LIB_DATA: u32 = 8;
const LIB_LEN: u32 = 12;
const LIB_FIELDS: u32 = 16;
/// A type's: its place in the library, and the library's object, of
/// which it holds a reference.
const INFO_INDEX: u32 = 6;
const INFO_LIB: u32 = 8;
const INFO_FIELDS: u32 = 12;

/// Where a library comes from: the built-in STDOLE of a version, or the
/// bytes of a file or of a module's TYPELIB resource.
pub(super) enum Source {
    Builtin(u16),
    Bytes(Vec<u8>),
}

pub(super) fn page(cpu: &Cpu) -> CodePage {
    crate::win32::nls::ansi(cpu)
}

/// `s` in the ANSI code page.
pub(super) fn ansi(page: CodePage, s: &str) -> Vec<u8> {
    let units: Vec<u16> = s.encode_utf16().collect();
    page.encode(&units)
}

/// The library `source` holds. The built-in STDOLE is the 16-bit one,
/// SYS_WIN16's.
fn model_of(cpu: &Cpu, source: &Source) -> Option<TypeLib> {
    match source {
        Source::Builtin(major) => {
            let mut m = tl::parse(*major, &[], page(cpu))?;
            m.syskind = 0;
            Some(m)
        }
        Source::Bytes(data) => tl::parse(0, data, page(cpu)),
    }
}

/// A block of the task allocator holding `size` bytes, its 16:16 address
/// and its linear one.
fn block(cpu: &mut Cpu, size: u32) -> Option<(u32, u32)> {
    let p = ole2::alloc(cpu, size);
    let at = super::linear(cpu, p);
    (at != 0).then_some((p, at))
}

/// A new object with the fields `fields` (from REFS on) and the
/// vtable of the methods `methods` after them, one reference: its 16:16
/// address, or 0 when memory is out.
fn new_object(cpu: &mut Cpu, fields: &[u8], methods: &[&str]) -> u32 {
    let Some(module) = super::builtin::find("TYPELIB") else {
        return 0;
    };
    let vtable = REFS + fields.len() as u32;
    let Some((p, at)) = block(cpu, vtable + 4 * methods.len() as u32) else {
        return 0;
    };
    super::write_32(cpu, at, p + vtable);
    super::poke(cpu, at + REFS, fields);
    super::write_16(cpu, at + REFS, 1);
    for (i, name) in methods.iter().enumerate() {
        let stub = super::builtin::stub_address(cpu, module, name).unwrap_or(0);
        super::write_32(cpu, at + vtable + 4 * i as u32, stub);
    }
    p
}

/// A library object of `source`: Err with TYPE_E_CANTLOADLIBRARY when it
/// isn't a library, E_OUTOFMEMORY when memory is out.
fn new_lib(cpu: &mut Cpu, source: Source) -> Result<u32, u32> {
    model_of(cpu, &source).ok_or(TYPE_E_CANTLOADLIBRARY)?;
    let mut fields = vec![0u8; (LIB_FIELDS - REFS) as usize];
    match source {
        Source::Builtin(major) => {
            fields[(LIB_BUILTIN - REFS) as usize..][..2].copy_from_slice(&major.to_le_bytes());
        }
        Source::Bytes(data) => {
            let (p, at) = block(cpu, data.len() as u32).ok_or(E_OUTOFMEMORY)?;
            super::poke(cpu, at, &data);
            fields[(LIB_DATA - REFS) as usize..][..4].copy_from_slice(&p.to_le_bytes());
            let len = data.len() as u32;
            fields[(LIB_LEN - REFS) as usize..][..4].copy_from_slice(&len.to_le_bytes());
        }
    }
    match new_object(cpu, &fields, &LIB_METHODS) {
        0 => Err(E_OUTOFMEMORY),
        lib => Ok(lib),
    }
}

/// The source of the library object `lib`.
fn source_of(cpu: &Cpu, lib: u32) -> Option<Source> {
    let at = super::linear(cpu, lib);
    if at == 0 {
        return None;
    }
    let builtin = super::read_16(cpu, at + LIB_BUILTIN);
    if builtin != 0 {
        return Some(Source::Builtin(builtin));
    }
    let data = super::linear(cpu, super::read_32(cpu, at + LIB_DATA));
    let len = super::read_32(cpu, at + LIB_LEN);
    if data == 0 {
        return None;
    }
    super::peek(cpu, data, len as usize).map(Source::Bytes)
}

/// The library of the library object `lib`.
fn lib_model(cpu: &Cpu, lib: u32) -> Option<TypeLib> {
    model_of(cpu, &source_of(cpu, lib)?)
}

/// Add a reference to the object `object`: the count.
fn add_ref(cpu: &mut Cpu, object: u32) -> u32 {
    let at = super::linear(cpu, object);
    if at == 0 {
        return 0;
    }
    let refs = super::read_16(cpu, at + REFS).saturating_add(1);
    super::write_16(cpu, at + REFS, refs);
    refs as u32
}

/// Take a reference from the object `object`: the count left, and
/// whether that was the last one.
fn take_ref(cpu: &mut Cpu, object: u32) -> (u32, bool) {
    let at = super::linear(cpu, object);
    if at == 0 {
        return (0, false);
    }
    let refs = super::read_16(cpu, at + REFS).saturating_sub(1);
    super::write_16(cpu, at + REFS, refs);
    (refs as u32, refs == 0)
}

fn release_lib(cpu: &mut Cpu, lib: u32) -> u32 {
    let (refs, last) = take_ref(cpu, lib);
    if last {
        let at = super::linear(cpu, lib);
        let data = super::read_32(cpu, at + LIB_DATA);
        if data != 0 {
            ole2::free(cpu, data);
        }
        ole2::free(cpu, lib);
    }
    refs
}

/// A type object of the type at `index` of the library object `lib`,
/// whose reference it takes over: its 16:16 address, or 0 when memory is
/// out.
fn new_info(cpu: &mut Cpu, lib: u32, index: usize) -> u32 {
    let mut fields = vec![0u8; (INFO_FIELDS - REFS) as usize];
    fields[(INFO_INDEX - REFS) as usize..][..2].copy_from_slice(&(index as u16).to_le_bytes());
    fields[(INFO_LIB - REFS) as usize..][..4].copy_from_slice(&lib.to_le_bytes());
    let info = new_object(cpu, &fields, &INFO_METHODS);
    if info == 0 {
        release_lib(cpu, lib);
    }
    info
}

/// The library object, the place and the library of the type object
/// `info`.
pub(super) fn info_parts(cpu: &Cpu, info: u32) -> Option<(u32, usize, TypeLib)> {
    let at = super::linear(cpu, info);
    if at == 0 {
        return None;
    }
    let index = super::read_16(cpu, at + INFO_INDEX) as usize;
    let lib = super::read_32(cpu, at + INFO_LIB);
    let m = lib_model(cpu, lib)?;
    (index < m.infos.len()).then_some((lib, index, m))
}

/// Give the program a type object of the type at `index` of `lib`, a
/// reference to which the caller added, at `out`: S_OK, or
/// E_OUTOFMEMORY.
fn give_info(cpu: &mut Cpu, lib: u32, index: usize, out: u32) -> u32 {
    let info = new_info(cpu, lib, index);
    super::write_32(cpu, out, info);
    if info == 0 { E_OUTOFMEMORY } else { S_OK }
}

// Finding libraries.

/// The library in the file `file`: its TYPELIB resource `number` in a
/// Windows module (resource 1 if no number is given, else the first),
/// or the whole file.
fn library_bytes(file: Vec<u8>, number: Option<u16>) -> Option<Vec<u8>> {
    if !file.starts_with(b"MZ") {
        return Some(file);
    }
    let module = ne::Module::parse(&file).ok()?;
    let kind = ne::ResId::Name("TYPELIB".into());
    let found = match number {
        Some(n) => module.find_resource(&kind, &ne::ResId::Int(n)),
        None => module.find_resource(&kind, &ne::ResId::Int(1)).or_else(|| {
            module
                .resources
                .iter()
                .find(|t| t.type_id.matches(&kind))
                .and_then(|t| t.resources.first())
        }),
    }?;
    module.resource_data(&file, found).ok().map(<[u8]>::to_vec)
}

/// The library LoadTypeLib loads for `name`: the file the search finds by
/// the name, or, when the last part of the name is a number, that
/// TYPELIB resource of the file before it. STDOLE.TLB is the built-in
/// one when there is no such file.
fn find_library(cpu: &mut Cpu, name: &str) -> Result<Source, u32> {
    let mut found = crate::win32::kernel32::search_file(cpu, name).map(|f| (f, None));
    if found.is_none()
        && let Some((file, number)) = name.rsplit_once('\\')
        && let Ok(number) = number.parse::<u16>()
    {
        found = crate::win32::kernel32::search_file(cpu, file).map(|f| (f, Some(number)));
    }
    let Some((full, number)) = found else {
        let file = name.rsplit(['\\', '/', ':']).next().unwrap_or(name);
        if file.eq_ignore_ascii_case(STDOLE16) {
            return Ok(Source::Builtin(1));
        }
        return Err(TYPE_E_CANTLOADLIBRARY);
    };
    let (file, _, _) =
        crate::win32::kernel32::read_path(cpu, &full).map_err(|_| TYPE_E_CANTLOADLIBRARY)?;
    library_bytes(file, number)
        .map(Source::Bytes)
        .ok_or(TYPE_E_CANTLOADLIBRARY)
}

/// The library `import` names: the 16-bit one the registry has, else
/// the built-in STDOLE for STDOLE's, else the file the import names.
fn import_source(cpu: &mut Cpu, import: &ImportLib) -> Result<Source, u32> {
    if let Ok(path) = tl::query_path_on(
        cpu,
        &import.guid,
        import.major,
        import.minor,
        import.lcid,
        "win16",
    ) && let Ok(source) = find_library(cpu, &path)
    {
        return Ok(source);
    }
    if import.guid == stdole::LIBID {
        return Ok(Source::Builtin(1));
    }
    find_library(cpu, &import.file)
}

/// The type the HREFTYPE `href` of the type at `index` of `m` refers to:
/// its library, None for `m` itself, and its place.
pub(super) fn find_ref(
    cpu: &mut Cpu,
    m: &TypeLib,
    index: usize,
    href: u32,
) -> Result<(Option<(Source, TypeLib)>, usize), u32> {
    match m.find_ref(index, href).ok_or(TYPE_E_ELEMENTNOTFOUND)? {
        Ref::Local(i) if i < m.infos.len() => Ok((None, i)),
        Ref::Local(_) => Err(TYPE_E_ELEMENTNOTFOUND),
        Ref::Import { lib, target } => {
            let import = m.imports.get(lib).ok_or(TYPE_E_ELEMENTNOTFOUND)?;
            let source = import_source(cpu, import)?;
            let other = model_of(cpu, &source).ok_or(TYPE_E_CANTLOADLIBRARY)?;
            let i = match target {
                Target::Index(n) => n as usize,
                Target::Guid(g) => other.find_guid(&g).ok_or(TYPE_E_ELEMENTNOTFOUND)?,
            };
            if i >= other.infos.len() {
                return Err(TYPE_E_ELEMENTNOTFOUND);
            }
            Ok((Some((source, other)), i))
        }
    }
}

/// Look `find` up in the type at `index` of `m`, and if it isn't there in
/// the first interface it implements or inherits from, and so on. `find`
/// is given the library, the type's place in it and the type.
pub(super) fn search<T>(
    cpu: &mut Cpu,
    m: TypeLib,
    index: usize,
    find: impl Fn(&TypeLib, usize, &TypeInfo) -> Option<T>,
) -> Option<T> {
    let (mut m, mut index) = (m, index);
    for _ in 0..MAX_BASES {
        let t = m.infos.get(index)?;
        if let Some(found) = find(&m, index, t) {
            return Some(found);
        }
        let href = t.impls.first()?.href;
        let (other, i) = find_ref(cpu, &m, index, href).ok()?;
        if let Some((_, other)) = other {
            m = other;
        }
        index = i;
    }
    None
}

// The structures the program gets.

/// A structure the program gets in one block of the task allocator,
/// with what it points to after it: the pointers are places in `bytes`
/// until `place` adds the block's address, and one ReleaseTypeAttr,
/// ReleaseFuncDesc or ReleaseVarDesc frees all of it.
struct Block {
    bytes: Vec<u8>,
    pointers: Vec<(usize, usize)>,
    page: CodePage,
}

impl Block {
    fn new(size: usize, page: CodePage) -> Block {
        Block {
            bytes: vec![0; size],
            pointers: Vec::new(),
            page,
        }
    }

    /// Room for `n` more bytes at the end: where they start.
    fn reserve(&mut self, n: usize) -> usize {
        let at = self.bytes.len();
        self.bytes.resize(at + n, 0);
        at
    }

    fn u16(&mut self, at: usize, value: u16) {
        self.bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn u32(&mut self, at: usize, value: u32) {
        self.bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    /// A pointer at `at` to the place `target`.
    fn pointer(&mut self, at: usize, target: usize) {
        self.pointers.push((at, target));
    }

    /// The TYPEDESC `td` at `at`: a union of a pointer and a HREFTYPE,
    /// then the VARTYPE.
    fn type_desc(&mut self, at: usize, td: &TypeDesc) {
        self.u16(at + 4, td.vt());
        match td {
            TypeDesc::Base(_) => {}
            TypeDesc::Ptr(to) | TypeDesc::SafeArray(to) => {
                let inner = self.reserve(6);
                self.pointer(at, inner);
                self.type_desc(inner, to);
            }
            TypeDesc::CArray(element, bounds) => {
                // ARRAYDESC: the element's TYPEDESC, cDims, then a
                // SAFEARRAYBOUND (count, lower bound) for each.
                let array = self.reserve(8 + 8 * bounds.len());
                self.pointer(at, array);
                self.type_desc(array, element);
                self.u16(array + 6, bounds.len() as u16);
                for (i, &(count, low)) in bounds.iter().enumerate() {
                    self.u32(array + 8 + 8 * i, count);
                    self.u32(array + 12 + 8 * i, low as u32);
                }
            }
            TypeDesc::User(href) => self.u32(at, *href),
        }
    }

    /// The ELEMDESC at `at`: the TYPEDESC, then the IDLDESC, whose
    /// wIDLFlags are the parameter's flags.
    fn elem_desc(&mut self, at: usize, td: &TypeDesc, flags: u16) {
        self.type_desc(at, td);
        self.u16(at + 10, flags);
    }

    /// The 16-bit VARIANT of `value` at `at`: the VARTYPE, the value at 8.
    fn variant(&mut self, at: usize, value: &Value) {
        match value {
            Value::Bits(vt, bits) => {
                self.u16(at, *vt);
                self.bytes[at + 8..at + 16].copy_from_slice(&bits.to_le_bytes());
            }
            Value::Str(s) => {
                self.u16(at, VT_BSTR);
                if let Some(s) = s {
                    let text = ansi(self.page, s);
                    let bstr = self.reserve(text.len() + 5);
                    self.u32(bstr, text.len() as u32);
                    self.bytes[bstr + 4..bstr + 4 + text.len()].copy_from_slice(&text);
                    self.pointer(at + 8, bstr + 4);
                }
            }
        }
    }

    /// Give the block to the program at `out`: S_OK, or E_OUTOFMEMORY.
    fn give(mut self, cpu: &mut Cpu, out: u32) -> u32 {
        let Some((p, at)) = block(cpu, self.bytes.len() as u32) else {
            super::write_32(cpu, out, 0);
            return E_OUTOFMEMORY;
        };
        for &(place, target) in &self.pointers.clone() {
            self.u32(place, p + target as u32);
        }
        super::poke(cpu, at, &self.bytes);
        super::write_32(cpu, out, p);
        S_OK
    }
}

/// TLIBATTR: the GUID, the LCID, the SYSKIND, the version and the flags.
fn lib_attr(m: &TypeLib, page: CodePage) -> Block {
    let mut b = Block::new(28, page);
    b.bytes[..16].copy_from_slice(&m.guid.0);
    b.u32(16, m.lcid);
    b.u16(20, m.syskind as u16);
    b.u16(22, m.major);
    b.u16(24, m.minor);
    b.u16(26, m.flags);
    b
}

/// TYPEATTR, 70 bytes.
fn type_attr(t: &TypeInfo, page: CodePage) -> Block {
    let mut b = Block::new(70, page);
    b.bytes[..16].copy_from_slice(&t.guid.0);
    b.u32(16, t.lcid);
    b.u32(24, MEMBERID_NIL as u32); // memidConstructor
    b.u32(28, MEMBERID_NIL as u32); // memidDestructor
    b.u32(36, t.size);
    b.u16(40, t.kind as u16);
    b.u16(42, t.funcs.len() as u16);
    b.u16(44, t.vars.len() as u16);
    b.u16(46, t.impls.len() as u16);
    b.u16(48, t.vft);
    b.u16(50, t.align);
    b.u16(52, t.flags);
    b.u16(54, t.major);
    b.u16(56, t.minor);
    match &t.alias {
        Some(alias) => b.type_desc(58, alias),
        None => b.u16(62, VT_EMPTY),
    }
    b
}

/// FUNCDESC, 40 bytes, and its parameters' ELEMDESCs after it.
fn func_desc(f: &Func, page: CodePage) -> Block {
    let mut b = Block::new(40, page);
    b.u32(0, f.memid as u32);
    if !f.params.is_empty() {
        let params = b.reserve(12 * f.params.len());
        b.pointer(8, params);
        for (i, p) in f.params.iter().enumerate() {
            b.elem_desc(params + 12 * i, &p.ty, p.flags);
        }
    }
    b.u16(12, f.kind as u16);
    b.u16(14, f.invkind as u16);
    b.u16(16, f.callconv as u16);
    b.u16(18, f.params.len() as u16);
    b.u16(20, f.opt as u16);
    b.u16(22, f.vft as u16);
    b.elem_desc(26, &f.ret, 0);
    b.u16(38, f.flags);
    b
}

/// VARDESC, 28 bytes, and a constant's VARIANT after it.
fn var_desc(v: &Var, page: CodePage) -> Block {
    let mut b = Block::new(28, page);
    b.u32(0, v.memid as u32);
    match &v.value {
        VarValue::Offset(offset) => b.u32(8, *offset),
        VarValue::Const(value) => {
            let at = b.reserve(16);
            b.pointer(8, at);
            b.variant(at, value);
        }
    }
    b.elem_desc(12, &v.ty, 0);
    b.u16(24, v.flags);
    b.u16(26, v.kind as u16);
    b
}

/// Put a BSTR of `s` at `out` unless `out` is NULL: NULL for None.
fn out_bstr(cpu: &mut Cpu, out: u32, s: Option<&str>) {
    if out == 0 {
        return;
    }
    let bstr = match s {
        Some(s) => {
            let text = ansi(page(cpu), s);
            alloc_bstr(cpu, &text)
        }
        None => 0,
    };
    super::write_32(cpu, out, bstr);
}

fn out_32(cpu: &mut Cpu, out: u32, value: u32) {
    if out != 0 {
        super::write_32(cpu, out, value);
    }
}

fn out_16(cpu: &mut Cpu, out: u32, value: u16) {
    if out != 0 {
        super::write_16(cpu, out, value);
    }
}

/// The ANSI string at `at` as the model has names.
fn name_at(cpu: &Cpu, at: u32) -> String {
    let bytes = super::string_bytes(cpu, at, 0x1_0000);
    crate::win32::oleaut32::model::ansi(page(cpu), &bytes)
}

// The functions.

/// CreateTypeLib(syskind, file, out): making libraries isn't done.
fn create_type_lib(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    out_32(cpu, args[2], 0);
    Some(E_NOTIMPL)
}

/// LoadTypeLib(file, out).
fn load_type_lib(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [name, out] = args[..] else {
        unreachable!()
    };
    if name == 0 || out == 0 {
        return Some(E_INVALIDARG);
    }
    super::write_32(cpu, out, 0);
    let name = super::string(cpu, name);
    let lib = find_library(cpu, &name).and_then(|source| new_lib(cpu, source));
    Some(match lib {
        Ok(lib) => {
            super::write_32(cpu, out, lib);
            S_OK
        }
        Err(e) => e,
    })
}

/// LHashValOfNameSys(syskind, lcid, name): a hash of the name in any
/// case, its low word, with the language's low byte in bits 16 to 23,
/// which IsHashValCompatible compares. The programs give it back to
/// IsName and FindName, which compare the names themselves.
fn lhash_val_of_name_sys(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [_, lcid, name] = args[..] else {
        unreachable!()
    };
    if name == 0 {
        return Some(0);
    }
    let name = name_at(cpu, name);
    let mut hash: u32 = 0x1505;
    for c in name.chars().flat_map(char::to_lowercase) {
        hash = hash.wrapping_mul(33) ^ c as u32;
    }
    Some((lcid & 0xFF) << 16 | (hash ^ hash >> 16) & 0xFFFF)
}

/// RegisterTypeLib(lib, path, help directory).
fn register_type_lib(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [lib, path, help] = args[..] else {
        unreachable!()
    };
    if path == 0 {
        return Some(E_INVALIDARG);
    }
    let Some(m) = lib_model(cpu, lib) else {
        return Some(E_INVALIDARG);
    };
    let path = name_at(cpu, path);
    let help = (help != 0).then(|| name_at(cpu, help));
    Some(tl::register_model(cpu, &m, &path, help.as_deref()))
}

/// The registry's 16-bit path of the library `guid` of the version.
fn query_path(cpu: &mut Cpu, guid: u32, major: u32, minor: u32, lcid: u32) -> Result<String, u32> {
    let guid = Guid(ole2::read_guid(cpu, guid));
    tl::query_path_on(cpu, &guid, major as u16, minor as u16, lcid, "win16")
}

/// LoadRegTypeLib(guid, major, minor, lcid, out).
fn load_reg_type_lib(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [guid, major, minor, lcid, out] = args[..] else {
        unreachable!()
    };
    if guid == 0 || out == 0 {
        return Some(E_INVALIDARG);
    }
    super::write_32(cpu, out, 0);
    let lib = query_path(cpu, guid, major, minor, lcid)
        .and_then(|path| find_library(cpu, &path))
        .and_then(|source| new_lib(cpu, source));
    Some(match lib {
        Ok(lib) => {
            super::write_32(cpu, out, lib);
            S_OK
        }
        Err(e) => e,
    })
}

/// QueryPathOfRegTypeLib(guid, major, minor, lcid, out): a BSTR.
fn query_path_of_reg_type_lib(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [guid, major, minor, lcid, out] = args[..] else {
        unreachable!()
    };
    if guid == 0 || out == 0 {
        return Some(E_INVALIDARG);
    }
    match query_path(cpu, guid, major, minor, lcid) {
        Ok(path) => {
            out_bstr(cpu, out, Some(&path));
            Some(S_OK)
        }
        Err(e) => Some(e),
    }
}

fn oa_build_version(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(BUILD_VERSION)
}

// ITypeLib.

fn lib_query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, iid, out] = args[..] else {
        unreachable!()
    };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    let iid = ole2::read_guid(cpu, iid);
    if iid == IID_IUNKNOWN || iid == IID_ITYPELIB {
        add_ref(cpu, this);
        super::write_32(cpu, out, this);
        return Some(S_OK);
    }
    super::write_32(cpu, out, 0);
    Some(E_NOINTERFACE)
}

fn lib_add_ref(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(add_ref(cpu, args[0]))
}

fn lib_release(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(release_lib(cpu, args[0]))
}

fn get_type_info_count(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(lib_model(cpu, args[0]).map_or(0, |m| m.infos.len() as u32))
}

/// GetTypeInfo(index, out).
fn get_type_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, index, out] = args[..] else {
        unreachable!()
    };
    let Some(m) = lib_model(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    if index as usize >= m.infos.len() {
        super::write_32(cpu, out, 0);
        return Some(TYPE_E_ELEMENTNOTFOUND);
    }
    add_ref(cpu, this);
    Some(give_info(cpu, this, index as usize, out))
}

/// GetTypeInfoType(index, out): a TYPEKIND, a word.
fn get_type_info_type(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, index, out] = args[..] else {
        unreachable!()
    };
    let Some(m) = lib_model(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    let Some(t) = m.infos.get(index as usize) else {
        return Some(TYPE_E_ELEMENTNOTFOUND);
    };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    super::write_16(cpu, out, t.kind as u16);
    Some(S_OK)
}

/// GetTypeInfoOfGuid(guid, out).
fn get_type_info_of_guid(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, guid, out] = args[..] else {
        unreachable!()
    };
    let Some(m) = lib_model(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if guid == 0 || out == 0 {
        return Some(E_INVALIDARG);
    }
    let guid = Guid(ole2::read_guid(cpu, guid));
    let Some(index) = m.find_guid(&guid) else {
        super::write_32(cpu, out, 0);
        return Some(TYPE_E_ELEMENTNOTFOUND);
    };
    add_ref(cpu, this);
    Some(give_info(cpu, this, index, out))
}

/// GetLibAttr(out): a TLIBATTR, which ReleaseTLibAttr frees.
fn get_lib_attr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, out] = args[..] else {
        unreachable!()
    };
    let Some(m) = lib_model(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    let page = page(cpu);
    Some(lib_attr(&m, page).give(cpu, out))
}

/// GetTypeComp, ITypeInfo's too: binding names isn't done.
fn get_type_comp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    out_32(cpu, args[1], 0);
    Some(E_NOTIMPL)
}

/// GetDocumentation(index, name, doc, help context, help file): of the
/// library for -1.
fn lib_get_documentation(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, index, name, doc, context, file] = args[..] else {
        unreachable!()
    };
    let Some(m) = lib_model(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    let (n, d, c) = if index as u16 as i16 == -1 {
        (&m.name, &m.doc, m.help_context)
    } else {
        let Some(t) = m.infos.get(index as usize) else {
            return Some(TYPE_E_ELEMENTNOTFOUND);
        };
        (&t.name, &t.doc, t.help_context)
    };
    out_bstr(cpu, name, n.as_deref());
    out_bstr(cpu, doc, d.as_deref());
    out_32(cpu, context, c);
    out_bstr(cpu, file, m.help_file.as_deref());
    Some(S_OK)
}

/// The names of the library's types, their members and their
/// parameters: the type's place and the member's id for each.
fn names(m: &TypeLib) -> Vec<(&str, usize, i32)> {
    let mut all = Vec::new();
    for (i, t) in m.infos.iter().enumerate() {
        if let Some(n) = &t.name {
            all.push((n.as_str(), i, MEMBERID_NIL));
        }
        for f in &t.funcs {
            if let Some(n) = &f.name {
                all.push((n.as_str(), i, f.memid));
            }
            for p in &f.params {
                if let Some(n) = &p.name {
                    all.push((n.as_str(), i, f.memid));
                }
            }
        }
        for v in &t.vars {
            if let Some(n) = &v.name {
                all.push((n.as_str(), i, v.memid));
            }
        }
    }
    all
}

/// IsName(name, hash, found): the name is written back as the library
/// has it; *found is an int.
fn is_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, buffer, _hash, found] = args[..] else {
        unreachable!()
    };
    let Some(m) = lib_model(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if buffer == 0 {
        return Some(E_INVALIDARG);
    }
    let name = name_at(cpu, buffer);
    let hit = names(&m).into_iter().find(|(n, _, _)| same_name(n, &name));
    if let Some((n, _, _)) = hit {
        let text = ansi(page(cpu), n);
        if text.len() == super::string_bytes(cpu, buffer, 0x1_0000).len() {
            super::poke(cpu, buffer, &text);
        }
    }
    out_16(cpu, found, u16::from(hit.is_some()));
    Some(S_OK)
}

/// FindName(name, hash, types, member ids, found): up to *found types
/// with a type or a member of the name.
fn find_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, buffer, _hash, types, ids, found] = args[..] else {
        unreachable!()
    };
    let Some(m) = lib_model(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if buffer == 0 || found == 0 {
        return Some(E_INVALIDARG);
    }
    let name = name_at(cpu, buffer);
    let max = super::read_16(cpu, found) as usize;
    let mut hits: Vec<(usize, i32)> = Vec::new();
    for (n, i, memid) in names(&m) {
        if hits.len() < max && same_name(n, &name) && !hits.iter().any(|&(h, _)| h == i) {
            hits.push((i, memid));
        }
    }
    for (k, &(i, memid)) in hits.iter().enumerate() {
        add_ref(cpu, this);
        give_info(cpu, this, i, types + 4 * k as u32);
        out_32(cpu, ids + 4 * k as u32, memid as u32);
    }
    super::write_16(cpu, found, hits.len() as u16);
    Some(S_OK)
}

/// ReleaseTLibAttr, ReleaseTypeAttr, ReleaseFuncDesc and ReleaseVarDesc:
/// each structure is one block.
fn release_block(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[1] != 0 {
        ole2::free(cpu, args[1]);
    }
    Some(0)
}

// ITypeInfo.

fn info_query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, iid, out] = args[..] else {
        unreachable!()
    };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    let iid = ole2::read_guid(cpu, iid);
    if iid == IID_IUNKNOWN || iid == IID_ITYPEINFO {
        add_ref(cpu, this);
        super::write_32(cpu, out, this);
        return Some(S_OK);
    }
    super::write_32(cpu, out, 0);
    Some(E_NOINTERFACE)
}

fn info_add_ref(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(add_ref(cpu, args[0]))
}

fn info_release(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let this = args[0];
    let (refs, last) = take_ref(cpu, this);
    if last {
        let at = super::linear(cpu, this);
        let lib = super::read_32(cpu, at + INFO_LIB);
        release_lib(cpu, lib);
        ole2::free(cpu, this);
    }
    Some(refs)
}

/// GetTypeAttr(out): a TYPEATTR, which ReleaseTypeAttr frees.
fn get_type_attr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, out] = args[..] else {
        unreachable!()
    };
    let Some((_, index, m)) = info_parts(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    let page = page(cpu);
    Some(type_attr(&m.infos[index], page).give(cpu, out))
}

/// GetFuncDesc(index, out).
fn get_func_desc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, index, out] = args[..] else {
        unreachable!()
    };
    let Some((_, i, m)) = info_parts(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    let Some(f) = m.infos[i].funcs.get(index as usize) else {
        super::write_32(cpu, out, 0);
        return Some(TYPE_E_ELEMENTNOTFOUND);
    };
    let page = page(cpu);
    Some(func_desc(f, page).give(cpu, out))
}

/// GetVarDesc(index, out).
fn get_var_desc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, index, out] = args[..] else {
        unreachable!()
    };
    let Some((_, i, m)) = info_parts(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    let Some(v) = m.infos[i].vars.get(index as usize) else {
        super::write_32(cpu, out, 0);
        return Some(TYPE_E_ELEMENTNOTFOUND);
    };
    let page = page(cpu);
    Some(var_desc(v, page).give(cpu, out))
}

/// The function's name and its parameters', as GetNames gives them. A
/// property's put function has its value's parameter without a name.
fn func_names(f: &Func) -> Vec<Option<String>> {
    let mut names = vec![f.name.clone()];
    names.extend(f.params.iter().map(|p| p.name.clone()));
    names
}

/// GetNames(memid, names, max, count): BSTRs, *count an unsigned int.
fn get_names(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, memid, out, max, count] = args[..] else {
        unreachable!()
    };
    let Some((_, index, m)) = info_parts(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if count == 0 || (out == 0 && max != 0) {
        return Some(E_INVALIDARG);
    }
    let memid = memid as i32;
    let found = search(cpu, m, index, |_, _, t| {
        if let Some(f) = t.funcs.iter().find(|f| f.memid == memid) {
            return Some(func_names(f));
        }
        let v = t.vars.iter().find(|v| v.memid == memid)?;
        Some(vec![v.name.clone()])
    });
    let Some(found) = found else {
        super::write_16(cpu, count, 0);
        return Some(TYPE_E_ELEMENTNOTFOUND);
    };
    let n = found.len().min(max as usize);
    for (i, name) in found.iter().take(n).enumerate() {
        out_bstr(cpu, out + 4 * i as u32, name.as_deref());
    }
    super::write_16(cpu, count, n as u16);
    Some(S_OK)
}

/// GetRefTypeOfImplType(index, out).
fn get_ref_type_of_impl_type(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, index, out] = args[..] else {
        unreachable!()
    };
    let Some((_, i, m)) = info_parts(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    match m.infos[i].impls.get(index as usize) {
        Some(imp) => {
            super::write_32(cpu, out, imp.href);
            Some(S_OK)
        }
        None => {
            super::write_32(cpu, out, 0);
            Some(TYPE_E_ELEMENTNOTFOUND)
        }
    }
}

/// GetImplTypeFlags(index, out): an int.
fn get_impl_type_flags(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, index, out] = args[..] else {
        unreachable!()
    };
    let Some((_, i, m)) = info_parts(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    match m.infos[i].impls.get(index as usize) {
        Some(imp) => {
            super::write_16(cpu, out, imp.flags as u16);
            Some(S_OK)
        }
        None => {
            super::write_16(cpu, out, 0);
            Some(TYPE_E_ELEMENTNOTFOUND)
        }
    }
}

/// GetIDsOfNames(names, count, ids): the member's id, then its
/// parameters' places; DISP_E_UNKNOWNNAME with -1 for each not found.
fn get_ids_of_names(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, names, count, ids] = args[..] else {
        unreachable!()
    };
    let Some((_, index, m)) = info_parts(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if names == 0 || ids == 0 {
        return Some(E_INVALIDARG);
    }
    let strings: Vec<String> = (0..count)
        .map(|i| {
            let at = super::linear(cpu, super::read_32(cpu, names + 4 * i));
            if at == 0 {
                String::new()
            } else {
                name_at(cpu, at)
            }
        })
        .collect();
    let mut found = vec![MEMBERID_NIL; strings.len()];
    let Some(first) = strings.first().cloned() else {
        return Some(S_OK);
    };
    let member = search(cpu, m, index, |_, _, t| {
        if let Some(f) = t
            .funcs
            .iter()
            .find(|f| f.name.as_ref().is_some_and(|n| same_name(n, &first)))
        {
            let params: Vec<Option<String>> = f.params.iter().map(|p| p.name.clone()).collect();
            return Some((f.memid, params));
        }
        let v = t
            .vars
            .iter()
            .find(|v| v.name.as_ref().is_some_and(|n| same_name(n, &first)))?;
        Some((v.memid, Vec::new()))
    });
    let mut hr = S_OK;
    match member {
        Some((memid, params)) => {
            found[0] = memid;
            for (i, name) in strings.iter().enumerate().skip(1) {
                match params
                    .iter()
                    .position(|p| p.as_ref().is_some_and(|p| same_name(p, name)))
                {
                    Some(at) => found[i] = at as i32,
                    None => hr = DISP_E_UNKNOWNNAME,
                }
            }
        }
        None => hr = DISP_E_UNKNOWNNAME,
    }
    for (i, id) in found.iter().enumerate() {
        super::write_32(cpu, ids + 4 * i as u32, *id as u32);
    }
    Some(hr)
}

pub(super) fn log_not_implemented(cpu: &mut Cpu, name: &str) {
    cpu.bus.log_string(&format!(
        "[W16] TYPELIB: {name} isn't implemented: E_NOTIMPL"
    ));
}

/// GetDocumentation(memid, name, doc, help context, help file): of the
/// type for MEMBERID_NIL.
fn info_get_documentation(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, memid, name, doc, context, file] = args[..] else {
        unreachable!()
    };
    let Some((_, index, m)) = info_parts(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    let memid = memid as i32;
    type Doc = (Option<String>, Option<String>, u32, Option<String>);
    let found: Option<Doc> = search(cpu, m, index, |lib, _, t| {
        let file = lib.help_file.clone();
        if memid == MEMBERID_NIL {
            return Some((t.name.clone(), t.doc.clone(), t.help_context, file));
        }
        if let Some(f) = t.funcs.iter().find(|f| f.memid == memid) {
            return Some((f.name.clone(), f.doc.clone(), f.help_context, file));
        }
        let v = t.vars.iter().find(|v| v.memid == memid)?;
        Some((v.name.clone(), v.doc.clone(), v.help_context, file))
    });
    let Some((n, d, c, f)) = found else {
        return Some(TYPE_E_ELEMENTNOTFOUND);
    };
    out_bstr(cpu, name, n.as_deref());
    out_bstr(cpu, doc, d.as_deref());
    out_32(cpu, context, c);
    out_bstr(cpu, file, f.as_deref());
    Some(S_OK)
}

/// GetDllEntry(memid, invoke kind, DLL, name, ordinal): of a module's
/// function.
fn get_dll_entry(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, memid, invkind, dll, name, ordinal] = args[..] else {
        unreachable!()
    };
    let Some((_, index, m)) = info_parts(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    let t = &m.infos[index];
    if t.kind != TKIND_MODULE {
        return Some(TYPE_E_BADMODULEKIND);
    }
    let Some(f) = t
        .funcs
        .iter()
        .find(|f| f.memid == memid as i32 && f.invkind & invkind != 0)
    else {
        return Some(TYPE_E_ELEMENTNOTFOUND);
    };
    out_bstr(cpu, dll, t.dll.as_deref());
    let (n, o) = match &f.entry {
        Some(Entry::Name(n)) => (Some(n.as_str()), 0),
        Some(Entry::Ordinal(o)) => (None, *o),
        None => (None, 0),
    };
    out_bstr(cpu, name, n);
    out_16(cpu, ordinal, o);
    Some(S_OK)
}

/// GetRefTypeInfo(href, out).
fn get_ref_type_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, href, out] = args[..] else {
        unreachable!()
    };
    let Some((lib, index, m)) = info_parts(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    super::write_32(cpu, out, 0);
    let (other, i) = match find_ref(cpu, &m, index, href) {
        Ok(found) => found,
        Err(e) => return Some(e),
    };
    let lib = match other {
        None => {
            add_ref(cpu, lib);
            lib
        }
        Some((source, _)) => match new_lib(cpu, source) {
            Ok(lib) => lib,
            Err(e) => return Some(e),
        },
    };
    Some(give_info(cpu, lib, i, out))
}

/// AddressOfMember(memid, invoke kind, out): not done.
fn address_of_member(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    out_32(cpu, args[3], 0);
    log_not_implemented(cpu, "ITypeInfo::AddressOfMember");
    Some(E_NOTIMPL)
}

/// CreateInstance(outer, iid, out): not done.
fn create_instance(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    out_32(cpu, args[3], 0);
    log_not_implemented(cpu, "ITypeInfo::CreateInstance");
    Some(E_NOTIMPL)
}

/// GetMops(memid, out): no marshaling opcodes, as Windows gives.
fn get_mops(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    out_32(cpu, args[2], 0);
    Some(S_OK)
}

/// GetContainingTypeLib(lib, index): *index an unsigned int.
fn get_containing_type_lib(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, out, index] = args[..] else {
        unreachable!()
    };
    let Some((lib, i, _)) = info_parts(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if out != 0 {
        add_ref(cpu, lib);
        super::write_32(cpu, out, lib);
    }
    out_16(cpu, index, i as u16);
    Some(S_OK)
}
