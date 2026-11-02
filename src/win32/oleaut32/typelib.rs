//! Type libraries: LoadTypeLib and the functions around it, which read a
//! library (msft.rs, sltg.rs, or the built-in stdole.rs) into the model
//! (model.rs), and the ITypeLib and ITypeInfo objects that show it to
//! the program. RegisterTypeLib writes the keys a library's controls
//! need under HKEY_CLASSES_ROOT\TypeLib and \Interface.
//!
//! What the program sees follows Windows 98's OLEAUT32, from what its
//! ITypeInfo gives for libraries made for the purpose and for those of
//! ActiveMovie and OLEPRO32. A dual interface has two sides. Its type in
//! the library is the dispinterface side (TKIND_DISPATCH): the methods
//! of IUnknown and IDispatch come first, then its own as
//! IDispatch::Invoke calls them, with no HRESULT and the [retval]
//! parameter as the return type; GetRefTypeOfImplType(-1) leads to the
//! interface side (TKIND_INTERFACE), with the methods as the vtable has
//! them. Where the two formats differ in the HREFTYPEs of a dual
//! interface's sides, each does as Windows does with it.
//!
//! The order of the steps follows Wine's oleaut32/typelib.c (LGPL 2.1
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
use super::{desc, dll_index, msft, sltg, stdole, vtable};
use crate::cpu::Cpu;
use crate::win32::com::{self, E_INVALIDARG, E_NOINTERFACE, E_NOTIMPL, E_OUTOFMEMORY, Guid, S_OK};
use crate::win32::registry::{self, HKEY_CLASSES_ROOT, Open, REG_SZ};
use std::sync::Arc;

pub const TYPE_E_ELEMENTNOTFOUND: u32 = 0x8002_802B;
pub const TYPE_E_LIBNOTREGISTERED: u32 = 0x8002_801D;
pub const TYPE_E_REGISTRYACCESS: u32 = 0x8002_801C;
pub const TYPE_E_CANTLOADLIBRARY: u32 = 0x8002_9C4A;
const TYPE_E_BADMODULEKIND: u32 = 0x8002_88BD;
pub const DISP_E_UNKNOWNNAME: u32 = 0x8002_0006;
pub const E_FAIL: u32 = 0x8000_4005;

const fn ole_guid(d1: u32) -> Guid {
    Guid::new(d1, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46])
}
const IID_ITYPEINFO: Guid = ole_guid(0x0002_0401);
const IID_ITYPELIB: Guid = ole_guid(0x0002_0402);
const IID_ITYPELIB2: Guid = ole_guid(0x0002_0411);
const IID_ITYPEINFO2: Guid = ole_guid(0x0002_0412);

/// The proxy classes RegisterTypeLib names for an interface: OLE
/// Automation's marshaler of interfaces and duals, and of
/// dispinterfaces.
const PSOAINTERFACE: &str = "{00020424-0000-0000-C000-000000000046}";
const PSDISPATCH: &str = "{00020420-0000-0000-C000-000000000046}";

/// LoadTypeLibEx's REGKIND.
const REGKIND_DEFAULT: u32 = 0;
const REGKIND_REGISTER: u32 = 1;

/// IDispatch's vtable: the size of a dispinterface's.
const DISPATCH_VFT: u16 = 28;

/// The HREFTYPEs of a dual interface's dispinterface side for the methods
/// it inherits: the HREFTYPE in the library of the interface they come
/// from, plus this times its place among the interfaces it inherits
/// from, counted from IUnknown's as 1.
const LEVEL_SHIFT: u32 = 24;

/// A library the program has loaded.
#[derive(Clone, Debug, Default)]
pub struct Lib {
    pub pid: u32,
    pub refs: u32,
    /// The file's path, in capitals, and the number of its TYPELIB
    /// resource: what LoadTypeLib finds it again by.
    pub path: String,
    pub resource: u32,
    /// The built-in STDOLE's version; 0 for a library from a file.
    pub builtin: u16,
    /// The library as the file has it.
    pub data: Vec<u8>,
    /// Its ITypeLib.
    pub iface: u32,
}

crate::state_fields!(Lib {
    pid,
    refs,
    path,
    resource,
    builtin,
    data,
    iface
});

/// A type of a library, as an ITypeInfo shows it: which type, which side
/// of a dual interface, and for its dispinterface side the interfaces it
/// inherits from, IUnknown first. It holds a reference to the library of
/// each.
#[derive(Clone, Debug, Default)]
pub struct Ctx {
    pub lib: u32,
    pub index: u32,
    pub interface: bool,
    pub bases: Vec<(u32, u32)>,
}

crate::state_fields!(Ctx {
    lib,
    index,
    interface,
    bases
});

#[derive(Clone, Debug, Default)]
pub struct Info {
    pub pid: u32,
    pub refs: u32,
    pub ctx: Ctx,
    pub iface: u32,
}

crate::state_fields!(Info {
    pid,
    refs,
    ctx,
    iface
});

/// The libraries read into the model, by the ids of their objects: not
/// saved, as they are read again from `Lib::data`.
#[derive(Clone, Debug, Default)]
pub struct Parsed(Vec<Option<Arc<TypeLib>>>);

impl crate::savestate::State for Parsed {
    fn save(&self, _: &mut crate::savestate::Writer) {}

    fn load(&mut self, _: &mut crate::savestate::Reader) -> crate::savestate::Result<()> {
        self.0.clear();
        Ok(())
    }
}

fn state(cpu: &mut Cpu) -> &mut super::State {
    &mut cpu.bus.win32.oleaut32
}

fn pid(cpu: &Cpu) -> u32 {
    crate::win32::user32::current_pid(cpu)
}

/// The library read from `data`, or the built-in STDOLE `builtin`.
fn parse(builtin: u16, data: &[u8]) -> Option<TypeLib> {
    if builtin != 0 {
        return Some(stdole::library(builtin));
    }
    match data.get(..4)? {
        b"MSFT" => msft::read(data),
        b"SLTG" => sltg::read(data),
        _ => None,
    }
}

/// The model of the library `lib`.
fn model(cpu: &mut Cpu, lib: u32) -> Arc<TypeLib> {
    let s = state(cpu);
    if let Some(Some(m)) = s.parsed.0.get(lib as usize) {
        return m.clone();
    }
    let l = s.libs[lib as usize]
        .as_ref()
        .expect("a library of the table");
    let m = Arc::new(parse(l.builtin, &l.data).expect("a library that was read before"));
    let cache = &mut s.parsed.0;
    if cache.len() <= lib as usize {
        cache.resize(lib as usize + 1, None);
    }
    cache[lib as usize] = Some(m.clone());
    m
}

fn lib_mut(cpu: &mut Cpu, lib: u32) -> &mut Lib {
    state(cpu).libs[lib as usize]
        .as_mut()
        .expect("a library of the table")
}

fn add_ref_lib(cpu: &mut Cpu, lib: u32) {
    let l = lib_mut(cpu, lib);
    l.refs = l.refs.saturating_add(1);
}

fn release_lib(cpu: &mut Cpu, lib: u32) -> u32 {
    let l = lib_mut(cpu, lib);
    l.refs = l.refs.saturating_sub(1);
    let refs = l.refs;
    if refs == 0 {
        let iface = l.iface;
        state(cpu).libs[lib as usize] = None;
        if let Some(slot) = state(cpu).parsed.0.get_mut(lib as usize) {
            *slot = None;
        }
        com::free_interface(cpu, iface);
    }
    refs
}

/// The id of a free slot of `table`, filled with `item`.
fn insert<T>(table: &mut Vec<Option<T>>, item: T) -> usize {
    match table.iter().position(Option::is_none) {
        Some(i) => {
            table[i] = Some(item);
            i
        }
        None => {
            table.push(Some(item));
            table.len() - 1
        }
    }
}

/// A new library object of the process, with one reference; None when
/// memory is out.
fn new_lib(cpu: &mut Cpu, path: String, resource: u32, builtin: u16, data: Vec<u8>) -> Option<u32> {
    let parsed = Arc::new(parse(builtin, &data)?);
    let lib = Lib {
        pid: pid(cpu),
        refs: 1,
        path,
        resource,
        builtin,
        data,
        iface: 0,
    };
    let id = insert(&mut state(cpu).libs, lib);
    let cache = &mut state(cpu).parsed.0;
    if cache.len() <= id {
        cache.resize(id + 1, None);
    }
    cache[id] = Some(parsed);
    let Some(iface) = com::new_interface(cpu, dll_index(), vtable("ITypeLib"), id as u32, 0) else {
        state(cpu).libs[id] = None;
        state(cpu).parsed.0[id] = None;
        return None;
    };
    lib_mut(cpu, id as u32).iface = iface;
    Some(id as u32)
}

/// The process's library object of the file `path` (in capitals) and
/// resource, or of the built-in STDOLE `builtin`, if it has one: with a
/// reference added.
fn find_lib(cpu: &mut Cpu, path: &str, resource: u32, builtin: u16) -> Option<u32> {
    let pid = pid(cpu);
    let id = state(cpu).libs.iter().position(|l| {
        l.as_ref().is_some_and(|l| {
            l.pid == pid && l.builtin == builtin && l.path == path && l.resource == resource
        })
    })? as u32;
    add_ref_lib(cpu, id);
    Some(id)
}

fn builtin_lib(cpu: &mut Cpu, major: u16) -> Result<u32, u32> {
    let major = if major >= 2 { 2 } else { 1 };
    if let Some(id) = find_lib(cpu, "", 0, major) {
        return Ok(id);
    }
    new_lib(cpu, String::new(), 0, major, Vec::new()).ok_or(E_OUTOFMEMORY)
}

/// The type library in the file `file`: its TYPELIB resource `resource`
/// in a PE file, or the whole file.
fn library_bytes(file: Vec<u8>, resource: u32) -> Option<Vec<u8>> {
    if !file.starts_with(b"MZ") {
        return Some(file);
    }
    use rust_dos_pe::{Image, ResId};
    let image = Image::load(&file).ok()?;
    let r = image.resources().ok()?.into_iter().find(|r| {
        matches!(&r.kind, ResId::Name(n) if n.eq_ignore_ascii_case("TYPELIB"))
            && r.name == ResId::Id(resource as u16)
    })?;
    let start = r.rva as usize;
    image
        .data
        .get(start..start + r.size as usize)
        .map(<[u8]>::to_vec)
}

/// Load the library `name` as LoadTypeLib does: the file SearchPath finds
/// by the name, or with a last part of digits the resource of that
/// number in the file before it. STDOLE2.TLB and STDOLE32.TLB are the
/// built-in ones when there are no such files. The library and the
/// file's full path.
fn load(cpu: &mut Cpu, name: &str) -> Result<(u32, String), u32> {
    let path = super::path_of(name);
    let mut found = crate::win32::kernel32::search_file(cpu, &path).map(|f| (f, 1));
    if found.is_none()
        && let Some((file, number)) = path.rsplit_once('\\')
        && !number.is_empty()
        && number.bytes().all(|b| b.is_ascii_digit())
    {
        let number = number.parse().unwrap_or(1);
        found = crate::win32::kernel32::search_file(cpu, file).map(|f| (f, number));
    }
    let Some((full, resource)) = found else {
        let file = path.rsplit('\\').next().unwrap_or(&path);
        return match () {
            _ if file.eq_ignore_ascii_case(stdole::STDOLE2) => {
                builtin_lib(cpu, 2).map(|l| (l, system_file(stdole::STDOLE2)))
            }
            _ if file.eq_ignore_ascii_case(stdole::STDOLE32) => {
                builtin_lib(cpu, 1).map(|l| (l, system_file(stdole::STDOLE32)))
            }
            _ => Err(TYPE_E_CANTLOADLIBRARY),
        };
    };
    let key = full.to_uppercase();
    if let Some(id) = find_lib(cpu, &key, resource, 0) {
        return Ok((id, full));
    }
    let (file, _, _) =
        crate::win32::kernel32::read_path(cpu, &full).map_err(|_| TYPE_E_CANTLOADLIBRARY)?;
    let data = library_bytes(file, resource).ok_or(TYPE_E_CANTLOADLIBRARY)?;
    if parse(0, &data).is_none() {
        return Err(TYPE_E_CANTLOADLIBRARY);
    }
    let id = new_lib(cpu, key, resource, 0, data).ok_or(E_OUTOFMEMORY)?;
    Ok((id, full))
}

fn system_file(name: &str) -> String {
    format!("C:\\WINDOWS\\SYSTEM\\{name}")
}

/// The library `import` names, as the library that imports it finds it:
/// by the registry, else by the file it names.
fn load_import(cpu: &mut Cpu, import: &ImportLib) -> Result<u32, u32> {
    if let Ok(path) = query_path(cpu, &import.guid, import.major, import.minor, import.lcid)
        && let Ok((lib, _)) = load(cpu, &path)
    {
        return Ok(lib);
    }
    if import.guid == stdole::LIBID {
        return builtin_lib(cpu, import.major);
    }
    load(cpu, &import.file).map(|(lib, _)| lib)
}

/// The type the HREFTYPE `href` of the type at `index` of `lib` refers
/// to: its library, with a reference added, its place, and whether it is
/// the interface side of a dual interface.
fn resolve_in(cpu: &mut Cpu, lib: u32, index: u32, href: u32) -> Result<(u32, u32, bool), u32> {
    let m = model(cpu, lib);
    let interface_bit = m.format != Format::Sltg && href & HREF_INTERFACE != 0;
    match m
        .find_ref(index as usize, href)
        .ok_or(TYPE_E_ELEMENTNOTFOUND)?
    {
        Ref::Local(i) => {
            let t = m.infos.get(i).ok_or(TYPE_E_ELEMENTNOTFOUND)?;
            add_ref_lib(cpu, lib);
            Ok((lib, i as u32, interface_bit && t.is_dual()))
        }
        Ref::Import { lib: k, target } => {
            let import = m.imports.get(k).ok_or(TYPE_E_ELEMENTNOTFOUND)?;
            let other = load_import(cpu, import)?;
            let m2 = model(cpu, other);
            let i = match target {
                Target::Index(n) => n as usize,
                Target::Guid(g) => m2.find_guid(&g).unwrap_or(usize::MAX),
            };
            let Some(t) = m2.infos.get(i) else {
                release_lib(cpu, other);
                return Err(TYPE_E_ELEMENTNOTFOUND);
            };
            Ok((other, i as u32, interface_bit && t.is_dual()))
        }
    }
}

/// A context of the type at `index` of `lib`, which takes the reference
/// to `lib` the caller holds.
fn make_ctx(cpu: &mut Cpu, lib: u32, index: u32, interface: bool) -> Ctx {
    let m = model(cpu, lib);
    let dual = m.infos[index as usize].is_dual();
    let mut ctx = Ctx {
        lib,
        index,
        interface: interface && dual,
        bases: Vec::new(),
    };
    if dual && !interface {
        ctx.bases = bases_of(lib, index, |&l, i| {
            let m = model(cpu, l);
            let first = m.infos[i as usize].impls.first()?;
            resolve_in(cpu, l, i, first.href)
                .ok()
                .map(|(l, i, _)| (l, i))
        });
    }
    ctx
}

/// The interfaces the type at `index` of `lib` inherits from, IUnknown
/// first: `base` finds the one a type inherits from directly.
pub(super) fn bases_of<L: Clone, I: Copy>(
    lib: L,
    index: I,
    mut base: impl FnMut(&L, I) -> Option<(L, I)>,
) -> Vec<(L, I)> {
    let mut chain: Vec<(L, I)> = Vec::new();
    let mut at = (lib, index);
    while chain.len() < 16 {
        let Some(next) = base(&at.0, at.1) else {
            break;
        };
        chain.push(next.clone());
        at = next;
    }
    chain.reverse();
    chain
}

fn release_ctx(cpu: &mut Cpu, ctx: &Ctx) {
    release_lib(cpu, ctx.lib);
    for &(lib, _) in &ctx.bases {
        release_lib(cpu, lib);
    }
}

/// The type the HREFTYPE `href` of the type of `ctx` refers to, as
/// GetRefTypeInfo finds it.
fn resolve(cpu: &mut Cpu, ctx: &Ctx, href: u32) -> Result<Ctx, u32> {
    let m = model(cpu, ctx.lib);
    let t = &m.infos[ctx.index as usize];
    let (lib, index, interface) = if (href as i32) < 0 {
        // The other side of a dual interface; in SLTG's numbering -2 of
        // the dispinterface side is IDispatch.
        if !t.is_dual() {
            return Err(TYPE_E_ELEMENTNOTFOUND);
        }
        match ctx.bases.last() {
            Some(&(lib, index))
                if m.format == Format::Sltg && href == 0xFFFF_FFFE && !ctx.interface =>
            {
                add_ref_lib(cpu, lib);
                (lib, index, false)
            }
            _ => {
                add_ref_lib(cpu, ctx.lib);
                (ctx.lib, ctx.index, !ctx.interface)
            }
        }
    } else {
        let level = (href >> LEVEL_SHIFT) as usize;
        match ctx.bases.get(level.wrapping_sub(1)) {
            Some(&(lib, index)) if !ctx.interface => {
                resolve_in(cpu, lib, index, href & ((1 << LEVEL_SHIFT) - 1))?
            }
            _ => resolve_in(cpu, ctx.lib, ctx.index, href)?,
        }
    };
    Ok(make_ctx(cpu, lib, index, interface))
}

/// A type as the program sees it through an ITypeInfo.
pub(super) struct View {
    pub(super) lib: Arc<TypeLib>,
    pub(super) index: usize,
    pub(super) kind: u32,
    pub(super) flags: u16,
    pub(super) vft: u16,
    pub(super) impls: Vec<Impl>,
    /// What GetRefTypeOfImplType(-1) gives.
    pub(super) other: Result<u32, u32>,
    pub(super) funcs: Vec<Func>,
}

impl View {
    pub(super) fn info(&self) -> &TypeInfo {
        &self.lib.infos[self.index]
    }
}

fn view(cpu: &mut Cpu, ctx: &Ctx) -> View {
    let m = model(cpu, ctx.lib);
    let bases: Vec<(Arc<TypeLib>, usize)> = ctx
        .bases
        .iter()
        .map(|&(lib, i)| (model(cpu, lib), i as usize))
        .collect();
    shape(m, ctx.index as usize, ctx.interface, &bases)
}

/// The view of the type at `index` of `m`: of the interface side of a
/// dual interface if `interface`, else of its dispinterface side, which
/// inherits the methods of `bases`, IUnknown first.
pub(super) fn shape(
    m: Arc<TypeLib>,
    index: usize,
    interface: bool,
    bases: &[(Arc<TypeLib>, usize)],
) -> View {
    let t = &m.infos[index];
    let sltg = m.format == Format::Sltg;
    let minus_one = if sltg { 0xFFFF_FFFC } else { 0xFFFF_FFFE };
    let base = t.impls.first().copied();
    let (kind, flags, vft, impls, other, funcs) = if !t.is_dual() {
        let mut impls = t.impls.clone();
        // An MSFT dispinterface has IDispatch from the library's header.
        if t.kind == TKIND_DISPATCH
            && impls.is_empty()
            && let Some(href) = m.dispatch_href
        {
            impls.push(Impl { href, flags: 0 });
        }
        // MSFT's HREFTYPE of an interface's base says it is one.
        if t.kind == TKIND_INTERFACE && !sltg {
            for i in &mut impls {
                i.href |= HREF_INTERFACE;
            }
        }
        // A dispinterface's vtable is IDispatch's, whatever the file says.
        let vft = match t.kind {
            TKIND_DISPATCH if !impls.is_empty() => DISPATCH_VFT,
            _ => t.vft,
        };
        let none = match t.kind {
            TKIND_ENUM | TKIND_RECORD | TKIND_UNION | TKIND_ALIAS | TKIND_MODULE if sltg => {
                TYPE_E_BADMODULEKIND
            }
            _ => TYPE_E_ELEMENTNOTFOUND,
        };
        (t.kind, t.flags, vft, impls, Err(none), t.funcs.clone())
    } else if interface {
        let impls = base.map(|b| Impl {
            href: if sltg {
                b.href
            } else {
                b.href | HREF_INTERFACE
            },
            flags: b.flags,
        });
        (
            TKIND_INTERFACE,
            t.flags | TYPEFLAG_FOLEAUTOMATION,
            t.vft,
            impls.into_iter().collect(),
            Ok(minus_one),
            t.funcs.clone(),
        )
    } else {
        let impls = base.map(|b| Impl {
            href: if sltg { 0xFFFF_FFFE } else { b.href },
            flags: 0,
        });
        let mut funcs = Vec::new();
        for (level, (bm, i)) in bases.iter().enumerate() {
            for f in &bm.infos[*i].funcs {
                funcs.push(dispatch_func(f, level as u32 + 1));
            }
        }
        funcs.extend(t.funcs.iter().map(|f| dispatch_func(f, 0)));
        (
            TKIND_DISPATCH,
            t.flags & !TYPEFLAG_FOLEAUTOMATION,
            DISPATCH_VFT,
            impls.into_iter().collect(),
            Ok(minus_one),
            funcs,
        )
    };
    View {
        lib: m.clone(),
        index,
        kind,
        flags,
        vft,
        impls,
        other,
        funcs,
    }
}

/// The method `f` of an interface as a dispinterface shows it: the
/// [retval] parameter its return type, else no HRESULT, and no [lcid]
/// parameter. `level` is the place of the interface it comes from
/// among those the dispinterface inherits from (0 for its own).
fn dispatch_func(f: &Func, level: u32) -> Func {
    let mut f = f.clone();
    if f.kind != FUNC_DISPATCH {
        let retval = f
            .params
            .last()
            .is_some_and(|p| p.flags & PARAMFLAG_FRETVAL != 0 && matches!(p.ty, TypeDesc::Ptr(_)));
        if retval {
            if let Some(Param {
                ty: TypeDesc::Ptr(inner),
                ..
            }) = f.params.pop()
            {
                f.ret = *inner;
            }
        } else if f.ret == TypeDesc::Base(VT_HRESULT) {
            f.ret = TypeDesc::Base(VT_VOID);
        }
        if f.params
            .last()
            .is_some_and(|p| p.flags & PARAMFLAG_FLCID != 0)
        {
            f.params.pop();
        }
    }
    f.kind = FUNC_DISPATCH;
    f.vft &= !3;
    if level > 0 {
        shift(&mut f.ret, level);
        for p in &mut f.params {
            shift(&mut p.ty, level);
        }
    }
    f
}

fn shift(t: &mut TypeDesc, level: u32) {
    match t {
        TypeDesc::User(href) => *href += level << LEVEL_SHIFT,
        TypeDesc::Ptr(inner) | TypeDesc::SafeArray(inner) | TypeDesc::CArray(inner, _) => {
            shift(inner, level)
        }
        TypeDesc::Base(_) => {}
    }
}

/// The method of `funcs` whose names GetNames gives for `memid`. When a
/// property has a get and a put method, Windows takes the get, as the
/// dumps of QUARTZ.DLL's library show: the value of IMediaPosition's
/// put_CurrentPosition has no name in the file, and Windows gives the
/// name of get_CurrentPosition's parameter for either method, and one
/// name only on the dispinterface side, where get has no parameters.
pub(super) fn names_func(funcs: &[Func], memid: i32) -> Option<&Func> {
    let put = INVOKE_PROPERTYPUT | INVOKE_PROPERTYPUTREF;
    funcs
        .iter()
        .find(|f| f.memid == memid && f.invkind & put == 0)
        .or_else(|| funcs.iter().find(|f| f.memid == memid))
}

/// The names GetNames gives for a method: its own and its parameters'.
fn func_names(f: &Func) -> Vec<Option<String>> {
    std::iter::once(f.name.clone())
        .chain(f.params.iter().map(|p| p.name.clone()))
        .collect()
}

// The objects.

/// The library object of the ITypeLib `this`.
pub fn lib_of(cpu: &mut Cpu, this: u32) -> Option<u32> {
    let (v, id) = com::interface_of(cpu, dll_index(), this)?;
    if v != vtable("ITypeLib") {
        return None;
    }
    state(cpu).libs.get(id as usize)?.as_ref()?;
    Some(id)
}

/// The type object of the ITypeInfo `this`.
pub fn info_of(cpu: &mut Cpu, this: u32) -> Option<u32> {
    let (v, id) = com::interface_of(cpu, dll_index(), this)?;
    if v != vtable("ITypeInfo") {
        return None;
    }
    state(cpu).infos.get(id as usize)?.as_ref()?;
    Some(id)
}

fn info_ctx(cpu: &mut Cpu, info: u32) -> Ctx {
    state(cpu).infos[info as usize]
        .as_ref()
        .expect("a type of the table")
        .ctx
        .clone()
}

/// A new ITypeInfo of `ctx`, which takes its references; None when memory
/// is out.
fn new_info(cpu: &mut Cpu, ctx: Ctx) -> Option<u32> {
    let info = Info {
        pid: pid(cpu),
        refs: 1,
        ctx,
        iface: 0,
    };
    let id = insert(&mut state(cpu).infos, info);
    let Some(iface) = com::new_interface(cpu, dll_index(), vtable("ITypeInfo"), id as u32, 0)
    else {
        let ctx = state(cpu).infos[id].take().expect("the new type").ctx;
        release_ctx(cpu, &ctx);
        return None;
    };
    state(cpu).infos[id].as_mut().expect("the new type").iface = iface;
    Some(iface)
}

/// Store a new ITypeInfo of `ctx` at `out`.
fn give_info(cpu: &mut Cpu, ctx: Ctx, out: u32) -> u32 {
    match new_info(cpu, ctx) {
        Some(iface) => {
            com::write_out(cpu, out, iface);
            S_OK
        }
        None => {
            com::write_out(cpu, out, 0);
            E_OUTOFMEMORY
        }
    }
}

/// A process ended: its objects went with its heap.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    let s = state(cpu);
    for (id, slot) in s.libs.iter_mut().enumerate() {
        if slot.as_ref().is_some_and(|l| l.pid == pid) {
            *slot = None;
            if let Some(p) = s.parsed.0.get_mut(id) {
                *p = None;
            }
        }
    }
    for slot in s.infos.iter_mut() {
        if slot.as_ref().is_some_and(|i| i.pid == pid) {
            *slot = None;
        }
    }
}

/// The string `s` as a new BSTR at `out`, if `out` isn't 0.
fn out_string(cpu: &mut Cpu, out: u32, s: Option<&str>) {
    if out != 0 {
        let b = s.map_or(0, |s| super::bstr(cpu, s));
        com::write_out(cpu, out, b);
    }
}

fn out_32(cpu: &mut Cpu, out: u32, value: u32) {
    if out != 0 {
        com::write_out(cpu, out, value);
    }
}

// The functions.

/// LoadTypeLib(file, out).
pub fn load_type_lib(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    load_type_lib_ex(cpu, &[args[0], REGKIND_DEFAULT, args[1]])
}

/// LoadTypeLibEx(file, regkind, out): REGKIND_DEFAULT registers a
/// library found by a name without its drive or share.
pub fn load_type_lib_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[file, regkind, out] = args else {
        unreachable!()
    };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    com::write_out(cpu, out, 0);
    let Some(name) = super::wide_string(cpu, file) else {
        return Some(E_INVALIDARG);
    };
    let (lib, full) = match load(cpu, &name) {
        Ok(found) => found,
        Err(e) => {
            log(cpu, &format!("LoadTypeLib can't load {name}: {e:08X}"));
            return Some(e);
        }
    };
    let qualified = name.as_bytes().get(1) == Some(&b':') || name.starts_with("\\\\");
    let register = match regkind {
        REGKIND_REGISTER => true,
        REGKIND_DEFAULT => !qualified,
        _ => false,
    };
    if register {
        let hr = register_lib(cpu, lib, &full, None);
        if hr != S_OK {
            release_lib(cpu, lib);
            return Some(hr);
        }
    }
    let iface = lib_mut(cpu, lib).iface;
    com::write_out(cpu, out, iface);
    Some(S_OK)
}

fn log(cpu: &mut Cpu, message: &str) {
    cpu.bus.log_string(&format!("[W32] OLEAUT32: {message}"));
}

/// The registry's path of the library `guid` of the version `major`.`minor`
/// or a later minor version, for `lcid` or its language or no language.
fn query_path(
    cpu: &mut Cpu,
    guid: &Guid,
    major: u16,
    minor: u16,
    lcid: u32,
) -> Result<String, u32> {
    let reg = registry::get(cpu);
    let Open::Key(root) = reg.open(HKEY_CLASSES_ROOT, 0) else {
        return Err(TYPE_E_LIBNOTREGISTERED);
    };
    let lib = reg
        .find(root, &format!("TypeLib\\{guid}"))
        .ok_or(TYPE_E_LIBNOTREGISTERED)?;
    // The version asked for, else the latest later minor version.
    let mut versions = Vec::new();
    for &child in &reg.key(lib).children {
        let Some((maj, min)) = reg.key(child).name.split_once('.') else {
            continue;
        };
        if let (Ok(maj), Ok(min)) = (u16::from_str_radix(maj, 16), u16::from_str_radix(min, 16))
            && maj == major
            && min >= minor
        {
            versions.push((min != minor, min, child));
        }
    }
    let (_, _, version) = versions
        .into_iter()
        .min_by_key(|&(later, min, _)| (later, std::cmp::Reverse(min)))
        .ok_or(TYPE_E_LIBNOTREGISTERED)?;
    for l in [lcid, lcid & 0x3FF, 0] {
        if let Some(key) = reg.find(version, &format!("{l:x}\\win32"))
            && let Some(v) = reg.value(key, "")
        {
            let end = v.data.iter().position(|&b| b == 0).unwrap_or(v.data.len());
            return Ok(ansi(&v.data[..end]));
        }
    }
    Err(TYPE_E_LIBNOTREGISTERED)
}

/// QueryPathOfRegTypeLib(guid, major, minor, lcid, out).
pub fn query_path_of_reg_type_lib(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[guid, major, minor, lcid, out] = args else {
        unreachable!()
    };
    let Some(guid) = Guid::read(cpu, guid) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    match query_path(cpu, &guid, major as u16, minor as u16, lcid) {
        Ok(path) => {
            out_string(cpu, out, Some(&path));
            Some(S_OK)
        }
        Err(e) => Some(e),
    }
}

/// LoadRegTypeLib(guid, major, minor, lcid, out): the library the
/// registry names.
pub fn load_reg_type_lib(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[guid, major, minor, lcid, out] = args else {
        unreachable!()
    };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    com::write_out(cpu, out, 0);
    let Some(guid) = Guid::read(cpu, guid) else {
        return Some(E_INVALIDARG);
    };
    let path = match query_path(cpu, &guid, major as u16, minor as u16, lcid) {
        Ok(path) => path,
        Err(e) => return Some(e),
    };
    match load(cpu, &path) {
        Ok((lib, _)) => {
            let iface = lib_mut(cpu, lib).iface;
            com::write_out(cpu, out, iface);
            Some(S_OK)
        }
        Err(e) => Some(e),
    }
}

/// RegisterTypeLib(lib, path, help directory).
pub fn register_type_lib(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, path, help] = args else {
        unreachable!()
    };
    let (Some(lib), Some(path)) = (lib_of(cpu, this), super::wide_string(cpu, path)) else {
        // A library of the program's own isn't taken yet.
        return Some(E_INVALIDARG);
    };
    let help = super::wide_string(cpu, help);
    Some(register_lib(cpu, lib, &path, help.as_deref()))
}

fn set_string(reg: &mut registry::Registry, key: u32, name: &str, value: &str) {
    let mut data = super::ansi(value);
    data.push(0);
    reg.set_value(key, name, REG_SZ, data);
}

/// Write the library's keys and those of its interfaces OLE Automation
/// marshals: its dispinterfaces, duals and [oleautomation] interfaces.
fn register_lib(cpu: &mut Cpu, lib: u32, path: &str, help: Option<&str>) -> u32 {
    let m = model(cpu, lib);
    let platform = match m.syskind {
        0 => "win16",
        1 => "win32",
        3 => "win64",
        _ => return E_FAIL,
    };
    let reg = registry::get(cpu);
    let Open::Key(root) = reg.open(HKEY_CLASSES_ROOT, 0) else {
        return TYPE_E_REGISTRYACCESS;
    };
    let version = format!("{:x}.{:x}", m.major, m.minor);
    let (key, _) = reg.create(root, &format!("TypeLib\\{}\\{version}", m.guid));
    if let Some(title) = m.doc.as_ref().or(m.name.as_ref()) {
        set_string(reg, key, "", title);
    }
    let (file, _) = reg.create(key, &format!("{:x}\\{platform}", m.lcid));
    set_string(reg, file, "", path);
    // The flags without FHASDISKIMAGE, which says where it was loaded
    // from rather than what it is.
    let (flags, _) = reg.create(key, "FLAGS");
    set_string(
        reg,
        flags,
        "",
        &(m.flags & !LIBFLAG_FHASDISKIMAGE).to_string(),
    );
    let (help_key, created) = reg.create(key, "HELPDIR");
    let dir = match help {
        Some(dir) => Some(dir.to_string()),
        None if created => Some(directory_of(path)),
        None => None,
    };
    if let Some(dir) = dir {
        set_string(reg, help_key, "", &dir);
    }
    for t in &m.infos {
        let automation = match t.kind {
            TKIND_DISPATCH => true,
            TKIND_INTERFACE => t.flags & TYPEFLAG_FOLEAUTOMATION != 0,
            _ => false,
        };
        if !automation {
            continue;
        }
        let (iface, _) = reg.create(root, &format!("Interface\\{}", t.guid));
        if let Some(name) = &t.name {
            set_string(reg, iface, "", name);
        }
        let proxy = if t.kind == TKIND_INTERFACE || t.flags & TYPEFLAG_FDUAL != 0 {
            PSOAINTERFACE
        } else {
            PSDISPATCH
        };
        for name in ["ProxyStubClsid", "ProxyStubClsid32"] {
            let (k, _) = reg.create(iface, name);
            set_string(reg, k, "", proxy);
        }
        let (k, _) = reg.create(iface, "TypeLib");
        set_string(reg, k, "", &m.guid.to_string());
        set_string(reg, k, "Version", &version);
    }
    S_OK
}

/// The directory of the library's file `path`: without its file name and
/// a resource's number after it.
fn directory_of(path: &str) -> String {
    let mut dir = path;
    if let Some((before, last)) = dir.rsplit_once('\\')
        && !last.is_empty()
        && last.bytes().all(|b| b.is_ascii_digit())
    {
        dir = before;
    }
    dir.rsplit_once('\\').map_or(dir, |(d, _)| d).to_string()
}

/// UnRegisterTypeLib(guid, major, minor, lcid, syskind): the library's
/// version key and its interfaces' keys that name it go.
pub fn unregister_type_lib(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[guid, major, minor, lcid, _syskind] = args else {
        unreachable!()
    };
    let Some(guid) = Guid::read(cpu, guid) else {
        return Some(E_INVALIDARG);
    };
    let path = match query_path(cpu, &guid, major as u16, minor as u16, lcid) {
        Ok(path) => path,
        Err(e) => return Some(e),
    };
    // The interfaces are those of the library as it is now.
    let interfaces: Vec<Guid> = match load(cpu, &path) {
        Ok((lib, _)) => {
            let m = model(cpu, lib);
            release_lib(cpu, lib);
            m.infos
                .iter()
                .filter(|t| matches!(t.kind, TKIND_DISPATCH | TKIND_INTERFACE))
                .map(|t| t.guid)
                .collect()
        }
        Err(_) => Vec::new(),
    };
    let reg = registry::get(cpu);
    let Open::Key(root) = reg.open(HKEY_CLASSES_ROOT, 0) else {
        return Some(TYPE_E_REGISTRYACCESS);
    };
    let libid = guid.to_string();
    for iid in interfaces {
        let Some(key) = reg.find(root, &format!("Interface\\{iid}")) else {
            continue;
        };
        let names_lib = reg
            .find(key, "TypeLib")
            .and_then(|k| reg.value(k, ""))
            .is_some_and(|v| v.text().eq_ignore_ascii_case(&libid));
        if names_lib {
            reg.delete(key);
        }
    }
    if let Some(key) = reg.find(root, &format!("TypeLib\\{libid}\\{major:x}.{minor:x}")) {
        reg.delete(key);
    }
    if let Some(key) = reg.find(root, &format!("TypeLib\\{libid}"))
        && reg.key(key).children.is_empty()
    {
        reg.delete(key);
    }
    Some(S_OK)
}

// ITypeLib.

fn this_lib(cpu: &mut Cpu, this: u32) -> Option<(u32, Arc<TypeLib>)> {
    let lib = lib_of(cpu, this)?;
    Some((lib, model(cpu, lib)))
}

pub fn lib_query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, iid, out] = args else {
        unreachable!()
    };
    let Some(lib) = lib_of(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    let Some(iid) = Guid::read(cpu, iid) else {
        return Some(E_INVALIDARG);
    };
    if [com::IID_IUNKNOWN, IID_ITYPELIB, IID_ITYPELIB2].contains(&iid) {
        add_ref_lib(cpu, lib);
        com::write_out(cpu, out, this);
        return Some(S_OK);
    }
    com::write_out(cpu, out, 0);
    Some(E_NOINTERFACE)
}

pub fn lib_add_ref(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(lib) = lib_of(cpu, args[0]) else {
        return Some(0);
    };
    add_ref_lib(cpu, lib);
    Some(lib_mut(cpu, lib).refs)
}

pub fn lib_release(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    match lib_of(cpu, args[0]) {
        Some(lib) => Some(release_lib(cpu, lib)),
        None => Some(0),
    }
}

pub fn get_type_info_count(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(this_lib(cpu, args[0]).map_or(0, |(_, m)| m.infos.len() as u32))
}

/// GetTypeInfo(index, out).
pub fn get_type_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, index, out] = args else {
        unreachable!()
    };
    let Some((lib, m)) = this_lib(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    if index as usize >= m.infos.len() {
        com::write_out(cpu, out, 0);
        return Some(TYPE_E_ELEMENTNOTFOUND);
    }
    add_ref_lib(cpu, lib);
    let ctx = make_ctx(cpu, lib, index, false);
    Some(give_info(cpu, ctx, out))
}

/// GetTypeInfoType(index, out).
pub fn get_type_info_type(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, index, out] = args else {
        unreachable!()
    };
    let Some((_, m)) = this_lib(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    let Some(t) = m.infos.get(index as usize) else {
        return Some(TYPE_E_ELEMENTNOTFOUND);
    };
    if !com::write_out(cpu, out, t.kind) {
        return Some(E_INVALIDARG);
    }
    Some(S_OK)
}

/// GetTypeInfoOfGuid(guid, out).
pub fn get_type_info_of_guid(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, guid, out] = args else {
        unreachable!()
    };
    let (Some((lib, m)), Some(guid)) = (this_lib(cpu, this), Guid::read(cpu, guid)) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    let Some(index) = m.find_guid(&guid) else {
        com::write_out(cpu, out, 0);
        return Some(TYPE_E_ELEMENTNOTFOUND);
    };
    add_ref_lib(cpu, lib);
    let ctx = make_ctx(cpu, lib, index as u32, false);
    Some(give_info(cpu, ctx, out))
}

/// GetLibAttr(out): a TLIBATTR ReleaseTLibAttr frees.
pub fn get_lib_attr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, out] = args else { unreachable!() };
    let Some((_, m)) = this_lib(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    let attr = desc::lib_attr(cpu, &m);
    com::write_out(cpu, out, attr);
    Some(if attr == 0 { E_OUTOFMEMORY } else { S_OK })
}

/// GetTypeComp, ITypeInfo's too: binding names to members isn't done
/// yet.
pub fn get_type_comp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    com::write_out(cpu, args[1], 0);
    Some(E_NOTIMPL)
}

/// GetDocumentation(index, name, doc, help context, help file): of the
/// library for -1.
pub fn lib_get_documentation(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, index, name, doc, context, file] = args else {
        unreachable!()
    };
    let Some((_, m)) = this_lib(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    let (n, d, c) = if index as i32 == -1 {
        (&m.name, &m.doc, m.help_context)
    } else {
        let Some(t) = m.infos.get(index as usize) else {
            return Some(TYPE_E_ELEMENTNOTFOUND);
        };
        (&t.name, &t.doc, t.help_context)
    };
    out_string(cpu, name, n.as_deref());
    out_string(cpu, doc, d.as_deref());
    out_32(cpu, context, c);
    out_string(cpu, file, m.help_file.as_deref());
    Some(S_OK)
}

/// The names of the library's types, their members and their
/// parameters, as IsName and FindName look through them: the type's
/// place and the member's id for each.
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
/// has it.
pub fn is_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, buffer, _hash, found] = args else {
        unreachable!()
    };
    let (Some((_, m)), Some(name)) = (this_lib(cpu, this), super::wide_string(cpu, buffer)) else {
        return Some(E_INVALIDARG);
    };
    let hit = names(&m).into_iter().find(|(n, _, _)| same_name(n, &name));
    if let Some((n, _, _)) = hit {
        let units: Vec<u16> = n.encode_utf16().collect();
        if units.len() == name.encode_utf16().count() {
            for (i, u) in units.iter().enumerate() {
                cpu.bus.guest_write_16(buffer + 2 * i as u32, *u);
            }
        }
    }
    out_32(cpu, found, u32::from(hit.is_some()));
    Some(S_OK)
}

/// FindName(name, hash, types, member ids, found): up to *found types
/// with a type or a member of the name.
pub fn find_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, buffer, _hash, types, ids, found] = args else {
        unreachable!()
    };
    let (Some((lib, m)), Some(name)) = (this_lib(cpu, this), super::wide_string(cpu, buffer))
    else {
        return Some(E_INVALIDARG);
    };
    if found == 0 || !cpu.bus.guest_probe(found, 2, true) {
        return Some(E_INVALIDARG);
    }
    let max = cpu.bus.guest_read_16(found) as usize;
    let mut hits: Vec<(usize, i32)> = Vec::new();
    for (n, i, memid) in names(&m) {
        if hits.len() < max && same_name(n, &name) && !hits.iter().any(|&(h, _)| h == i) {
            hits.push((i, memid));
        }
    }
    for (k, &(i, memid)) in hits.iter().enumerate() {
        add_ref_lib(cpu, lib);
        let ctx = make_ctx(cpu, lib, i as u32, false);
        give_info(cpu, ctx, types + 4 * k as u32);
        com::write_out(cpu, ids + 4 * k as u32, memid as u32);
    }
    cpu.bus.guest_write_16(found, hits.len() as u16);
    Some(S_OK)
}

/// ReleaseTLibAttr(attr), ReleaseTypeAttr(attr).
pub fn release_attr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    crate::win32::ole32::task_free(cpu, args[1]);
    Some(0)
}

/// GetCustData(guid, out) of ITypeLib2 and ITypeInfo2: neither format
/// read keeps custom data yet, so none is there, which is VT_EMPTY.
pub fn get_cust_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let out = *args.last().expect("an argument");
    if out == 0 || !cpu.bus.guest_probe(out, 16, true) {
        return Some(E_INVALIDARG);
    }
    cpu.bus.guest_fill(out, 16, 0);
    Some(S_OK)
}

/// GetAllCustData(out) and the like: a CUSTDATA of nothing.
pub fn get_all_cust_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let out = *args.last().expect("an argument");
    if out == 0 || !cpu.bus.guest_probe(out, 8, true) {
        return Some(E_INVALIDARG);
    }
    cpu.bus.guest_fill(out, 8, 0);
    Some(S_OK)
}

/// GetLibStatistics(unique names, their characters).
pub fn get_lib_statistics(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, count, chars] = args else {
        unreachable!()
    };
    let Some((_, m)) = this_lib(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    let mut unique: Vec<String> = Vec::new();
    for (n, _, _) in names(&m) {
        if !unique.iter().any(|u| same_name(u, n)) {
            unique.push(n.to_string());
        }
    }
    let total: usize = unique.iter().map(|u| u.encode_utf16().count()).sum();
    if count == 0 {
        return Some(E_INVALIDARG);
    }
    out_32(cpu, count, unique.len() as u32);
    out_32(cpu, chars, total as u32);
    Some(S_OK)
}

/// GetDocumentation2(index, lcid, help string, its context, help DLL):
/// no localized help strings.
pub fn lib_get_documentation2(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, index, _lcid, string, context, dll] = args else {
        unreachable!()
    };
    let Some((_, m)) = this_lib(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if index as i32 != -1 && index as usize >= m.infos.len() {
        return Some(TYPE_E_ELEMENTNOTFOUND);
    }
    out_string(cpu, string, None);
    out_32(cpu, context, 0);
    out_string(cpu, dll, None);
    Some(S_OK)
}

// ITypeInfo.

fn this_info(cpu: &mut Cpu, this: u32) -> Option<(u32, Ctx)> {
    let info = info_of(cpu, this)?;
    Some((info, info_ctx(cpu, info)))
}

pub fn info_query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, iid, out] = args else {
        unreachable!()
    };
    let Some(info) = info_of(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    let Some(iid) = Guid::read(cpu, iid) else {
        return Some(E_INVALIDARG);
    };
    if [com::IID_IUNKNOWN, IID_ITYPEINFO, IID_ITYPEINFO2].contains(&iid) {
        let i = state(cpu).infos[info as usize].as_mut().expect("a type");
        i.refs = i.refs.saturating_add(1);
        com::write_out(cpu, out, this);
        return Some(S_OK);
    }
    com::write_out(cpu, out, 0);
    Some(E_NOINTERFACE)
}

pub fn info_add_ref(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(info) = info_of(cpu, args[0]) else {
        return Some(0);
    };
    let i = state(cpu).infos[info as usize].as_mut().expect("a type");
    i.refs = i.refs.saturating_add(1);
    Some(i.refs)
}

pub fn info_release(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(info) = info_of(cpu, args[0]) else {
        return Some(0);
    };
    let i = state(cpu).infos[info as usize].as_mut().expect("a type");
    i.refs = i.refs.saturating_sub(1);
    if i.refs > 0 {
        return Some(i.refs);
    }
    let i = state(cpu).infos[info as usize].take().expect("a type");
    release_ctx(cpu, &i.ctx);
    com::free_interface(cpu, i.iface);
    Some(0)
}

/// GetTypeAttr(out): a TYPEATTR ReleaseTypeAttr frees.
pub fn get_type_attr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, out] = args else { unreachable!() };
    let Some((_, ctx)) = this_info(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    let v = view(cpu, &ctx);
    let attr = desc::type_attr(
        cpu,
        &desc::Attr {
            info: v.info(),
            kind: v.kind,
            flags: v.flags,
            funcs: v.funcs.len(),
            impls: v.impls.len(),
            vft: v.vft,
        },
    );
    com::write_out(cpu, out, attr);
    Some(if attr == 0 { E_OUTOFMEMORY } else { S_OK })
}

/// GetFuncDesc(index, out).
pub fn get_func_desc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, index, out] = args else {
        unreachable!()
    };
    let Some((_, ctx)) = this_info(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    let v = view(cpu, &ctx);
    let Some(f) = v.funcs.get(index as usize) else {
        return Some(TYPE_E_ELEMENTNOTFOUND);
    };
    let at = desc::func_desc(cpu, f);
    com::write_out(cpu, out, at);
    Some(if at == 0 { E_OUTOFMEMORY } else { S_OK })
}

/// GetVarDesc(index, out).
pub fn get_var_desc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, index, out] = args else {
        unreachable!()
    };
    let Some((_, ctx)) = this_info(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    let v = view(cpu, &ctx);
    let Some(var) = v.info().vars.get(index as usize) else {
        return Some(TYPE_E_ELEMENTNOTFOUND);
    };
    let at = desc::var_desc(cpu, var);
    com::write_out(cpu, out, at);
    Some(if at == 0 { E_OUTOFMEMORY } else { S_OK })
}

/// Look `find` up in the type of `ctx`, and if it isn't there in the
/// first interface it implements or inherits from, and so on.
fn search<T>(cpu: &mut Cpu, ctx: &Ctx, find: &impl Fn(&View) -> Option<T>) -> Option<T> {
    let v = view(cpu, ctx);
    if let Some(found) = find(&v) {
        return Some(found);
    }
    let first = v.impls.first()?.href;
    let base = resolve(cpu, ctx, first).ok()?;
    let found = search(cpu, &base, find);
    release_ctx(cpu, &base);
    found
}

/// GetNames(memid, names, max, count).
pub fn get_names(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, memid, out, max, count] = args else {
        unreachable!()
    };
    let Some((_, ctx)) = this_info(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if count == 0 {
        return Some(E_INVALIDARG);
    }
    let memid = memid as i32;
    let found = search(cpu, &ctx, &|v: &View| {
        if let Some(f) = names_func(&v.funcs, memid) {
            return Some(func_names(f));
        }
        let var = v.info().vars.iter().find(|var| var.memid == memid)?;
        Some(vec![var.name.clone()])
    });
    let Some(found) = found else {
        com::write_out(cpu, count, 0);
        return Some(TYPE_E_ELEMENTNOTFOUND);
    };
    let n = found.len().min(max as usize);
    for (i, name) in found.iter().take(n).enumerate() {
        out_string(cpu, out + 4 * i as u32, name.as_deref());
    }
    com::write_out(cpu, count, n as u32);
    Some(S_OK)
}

/// GetRefTypeOfImplType(index, out): -1 for the other side of a dual
/// interface.
pub fn get_ref_type_of_impl_type(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, index, out] = args else {
        unreachable!()
    };
    let Some((_, ctx)) = this_info(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    let v = view(cpu, &ctx);
    let href = if index as i32 == -1 {
        v.other
    } else {
        v.impls
            .get(index as usize)
            .map(|i| i.href)
            .ok_or(TYPE_E_ELEMENTNOTFOUND)
    };
    match href {
        Ok(href) => {
            if !com::write_out(cpu, out, href) {
                return Some(E_INVALIDARG);
            }
            Some(S_OK)
        }
        Err(e) => {
            out_32(cpu, out, 0);
            Some(e)
        }
    }
}

/// GetImplTypeFlags(index, out).
pub fn get_impl_type_flags(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, index, out] = args else {
        unreachable!()
    };
    let Some((_, ctx)) = this_info(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    let v = view(cpu, &ctx);
    let Some(i) = v.impls.get(index as usize) else {
        out_32(cpu, out, 0);
        return Some(TYPE_E_ELEMENTNOTFOUND);
    };
    if !com::write_out(cpu, out, i.flags as u32) {
        return Some(E_INVALIDARG);
    }
    Some(S_OK)
}

/// The ids of the names `names`, a member's and its parameters', in the
/// type of `ctx`: DISP_E_UNKNOWNNAME with -1 for each not found.
pub fn ids_of_names(cpu: &mut Cpu, ctx: &Ctx, names: &[String]) -> (u32, Vec<i32>) {
    let mut ids = vec![MEMBERID_NIL; names.len()];
    let Some(first) = names.first() else {
        return (S_OK, ids);
    };
    let found = search(cpu, ctx, &|v: &View| {
        if let Some(f) = v
            .funcs
            .iter()
            .find(|f| f.name.as_ref().is_some_and(|n| same_name(n, first)))
        {
            let params: Vec<Option<String>> = f.params.iter().map(|p| p.name.clone()).collect();
            return Some((f.memid, params));
        }
        let var = v
            .info()
            .vars
            .iter()
            .find(|var| var.name.as_ref().is_some_and(|n| same_name(n, first)))?;
        Some((var.memid, Vec::new()))
    });
    let Some((memid, params)) = found else {
        return (DISP_E_UNKNOWNNAME, ids);
    };
    ids[0] = memid;
    let mut hr = S_OK;
    for (i, name) in names.iter().enumerate().skip(1) {
        match params
            .iter()
            .position(|p| p.as_ref().is_some_and(|p| same_name(p, name)))
        {
            Some(at) => ids[i] = at as i32,
            None => hr = DISP_E_UNKNOWNNAME,
        }
    }
    (hr, ids)
}

/// GetIDsOfNames(names, count, ids).
pub fn get_ids_of_names(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, names, count, ids] = args else {
        unreachable!()
    };
    let Some((_, ctx)) = this_info(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    Some(ids_at(cpu, &ctx, names, count, ids))
}

/// GetIDsOfNames of `ctx` with the program's arguments.
pub fn ids_at(cpu: &mut Cpu, ctx: &Ctx, names: u32, count: u32, ids: u32) -> u32 {
    if count > 0x1000 || !cpu.bus.guest_probe(ids, 4 * count as usize, true) {
        return E_INVALIDARG;
    }
    let mut strings = Vec::new();
    for i in 0..count {
        let at = names.wrapping_add(4 * i);
        if !cpu.bus.guest_probe(at, 4, false) {
            return E_INVALIDARG;
        }
        let s = cpu.bus.guest_read_32(at);
        let Some(s) = super::wide_string(cpu, s) else {
            return E_INVALIDARG;
        };
        strings.push(s);
    }
    let (hr, found) = ids_of_names(cpu, ctx, &strings);
    for (i, id) in found.iter().enumerate() {
        cpu.bus.guest_write_32(ids + 4 * i as u32, *id as u32);
    }
    hr
}

/// Invoke: calling a method of the program's object by the description
/// of its vtable isn't done yet.
pub fn invoke(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(E_NOTIMPL)
}

/// GetDocumentation(memid, name, doc, help context, help file): of the
/// type for MEMBERID_NIL.
pub fn info_get_documentation(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, memid, name, doc, context, file] = args else {
        unreachable!()
    };
    let Some((_, ctx)) = this_info(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    let memid = memid as i32;
    type Doc = (Option<String>, Option<String>, u32, Option<String>);
    let found: Option<Doc> = search(cpu, &ctx, &|v: &View| {
        let file = v.lib.help_file.clone();
        let t = v.info();
        if memid == MEMBERID_NIL {
            return Some((t.name.clone(), t.doc.clone(), t.help_context, file));
        }
        if let Some(f) = v.funcs.iter().find(|f| f.memid == memid) {
            return Some((f.name.clone(), f.doc.clone(), f.help_context, file));
        }
        let var = t.vars.iter().find(|var| var.memid == memid)?;
        Some((var.name.clone(), var.doc.clone(), var.help_context, file))
    });
    let Some((n, d, c, f)) = found else {
        return Some(TYPE_E_ELEMENTNOTFOUND);
    };
    out_string(cpu, name, n.as_deref());
    out_string(cpu, doc, d.as_deref());
    out_32(cpu, context, c);
    out_string(cpu, file, f.as_deref());
    Some(S_OK)
}

/// GetDllEntry(memid, invoke kind, DLL, name, ordinal): of a module's
/// function.
pub fn get_dll_entry(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, memid, invkind, dll, name, ordinal] = args else {
        unreachable!()
    };
    let Some((_, ctx)) = this_info(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    let v = view(cpu, &ctx);
    let t = v.info();
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
    out_string(cpu, dll, t.dll.as_deref());
    let (n, o) = match &f.entry {
        Some(Entry::Name(n)) => (Some(n.as_str()), 0),
        Some(Entry::Ordinal(o)) => (None, *o),
        None => (None, 0),
    };
    out_string(cpu, name, n);
    if ordinal != 0 && cpu.bus.guest_probe(ordinal, 2, true) {
        cpu.bus.guest_write_16(ordinal, o);
    }
    Some(S_OK)
}

/// GetRefTypeInfo(href, out).
pub fn get_ref_type_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, href, out] = args else {
        unreachable!()
    };
    let Some((_, ctx)) = this_info(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    match resolve(cpu, &ctx, href) {
        Ok(found) => Some(give_info(cpu, found, out)),
        Err(e) => {
            com::write_out(cpu, out, 0);
            Some(e)
        }
    }
}

/// AddressOfMember and CreateInstance: neither is done yet.
pub fn not_yet(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    out_32(cpu, args[3], 0);
    Some(E_NOTIMPL)
}

/// GetMops(memid, out): no marshaling opcodes, as Windows gives.
pub fn get_mops(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    out_string(cpu, args[2], None);
    Some(S_OK)
}

/// GetContainingTypeLib(lib, index).
pub fn get_containing_type_lib(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, out, index] = args else {
        unreachable!()
    };
    let Some((_, ctx)) = this_info(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if out != 0 {
        add_ref_lib(cpu, ctx.lib);
        let iface = lib_mut(cpu, ctx.lib).iface;
        com::write_out(cpu, out, iface);
    }
    out_32(cpu, index, ctx.index);
    Some(S_OK)
}

pub fn release_func_desc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    desc::free_func_desc(cpu, args[1]);
    Some(0)
}

pub fn release_var_desc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    desc::free_var_desc(cpu, args[1]);
    Some(0)
}

// ITypeInfo2.

pub fn get_type_kind(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((_, ctx)) = this_info(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let kind = view(cpu, &ctx).kind;
    Some(if com::write_out(cpu, args[1], kind) {
        S_OK
    } else {
        E_INVALIDARG
    })
}

pub fn get_type_flags(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((_, ctx)) = this_info(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let flags = view(cpu, &ctx).flags as u32;
    Some(if com::write_out(cpu, args[1], flags) {
        S_OK
    } else {
        E_INVALIDARG
    })
}

/// GetFuncIndexOfMemId(memid, invoke kind, out).
pub fn get_func_index_of_mem_id(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, memid, invkind, out] = args else {
        unreachable!()
    };
    let Some((_, ctx)) = this_info(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    let v = view(cpu, &ctx);
    match v
        .funcs
        .iter()
        .position(|f| f.memid == memid as i32 && f.invkind & invkind != 0)
    {
        Some(i) => Some(if com::write_out(cpu, out, i as u32) {
            S_OK
        } else {
            E_INVALIDARG
        }),
        None => Some(TYPE_E_ELEMENTNOTFOUND),
    }
}

/// GetVarIndexOfMemId(memid, out).
pub fn get_var_index_of_mem_id(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, memid, out] = args else {
        unreachable!()
    };
    let Some((_, ctx)) = this_info(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    let v = view(cpu, &ctx);
    match v
        .info()
        .vars
        .iter()
        .position(|var| var.memid == memid as i32)
    {
        Some(i) => Some(if com::write_out(cpu, out, i as u32) {
            S_OK
        } else {
            E_INVALIDARG
        }),
        None => Some(TYPE_E_ELEMENTNOTFOUND),
    }
}

/// GetDocumentation2(memid, lcid, help string, its context, help DLL):
/// no localized help strings.
pub fn info_get_documentation2(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, _memid, _lcid, string, context, dll] = args else {
        unreachable!()
    };
    if info_of(cpu, this).is_none() {
        return Some(E_INVALIDARG);
    }
    out_string(cpu, string, None);
    out_32(cpu, context, 0);
    out_string(cpu, dll, None);
    Some(S_OK)
}
