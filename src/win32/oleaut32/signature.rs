//! The methods of an interface as OLE Automation's marshaler sees them:
//! for each place in the vtable, the parameters and what each holds, so
//! that a call can be copied from one process to another (ole32's
//! rpc.rs). They come from the type library the registry names for the
//! interface (HKEY_CLASSES_ROOT\Interface\{iid}\TypeLib), as Windows'
//! marshaler of interfaces and duals ({00020424-...}) reads them, or for
//! IUnknown, IDispatch and IClassFactory from what COM knows of them.

use super::model::*;
use super::typelib;
use crate::cpu::Cpu;
use crate::win32::com::{Guid, IID_ICLASSFACTORY, IID_IUNKNOWN};
use crate::win32::registry::{self, HKEY_CLASSES_ROOT};

pub const IID_IDISPATCH: Guid = Guid::new(0x0002_0400, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);

/// What a parameter holds, as far as copying it goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// 4 bytes copied as they are: integers, enums, BOOL, float, HRESULT.
    Word,
    /// 8 bytes: double, CY, DATE, 64-bit integers.
    Quad,
    Bstr,
    /// A zero-ended string of bytes or of Unicode characters.
    Str,
    WStr,
    /// A VARIANT, 16 bytes on the stack.
    Variant,
    /// A pointer to an interface whose IID is known.
    Iface(Guid),
    /// A pointer to an interface whose IID the parameter at this place
    /// holds (QueryInterface's, IClassFactory::CreateInstance's).
    IidIs(usize),
    /// A SAFEARRAY pointer.
    Array,
    /// A structure of this many bytes with no pointers in it, or a GUID.
    Record(u32),
    Ptr(Box<Kind>),
    /// IDispatch::GetIDsOfNames' names, as many as the parameter at this
    /// place says.
    Names(usize),
    /// IDispatch::GetIDsOfNames' DISPIDs: as many as the parameter at
    /// this place says, written by the method.
    Ids(usize),
    /// IDispatch::Invoke's DISPPARAMS: its arguments, the ones passed by
    /// reference copied back.
    DispParams,
    /// IDispatch::Invoke's EXCEPINFO, written by the method.
    ExcepInfo,
    /// A type the marshaler doesn't copy.
    Unsupported,
}

impl Kind {
    /// The bytes it takes on the stack, and in a slot of memory.
    pub fn size(&self) -> u32 {
        match self {
            Kind::Quad => 8,
            Kind::Variant => 16,
            Kind::Record(n) => n.next_multiple_of(4),
            _ => 4,
        }
    }
}

pub const PARAM_IN: u16 = PARAMFLAG_FIN;
pub const PARAM_OUT: u16 = PARAMFLAG_FOUT;

/// A parameter: what it holds and whether it goes in, out or both.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Param {
    pub kind: Kind,
    pub flags: u16,
}

impl Param {
    fn new(kind: Kind, flags: u16) -> Param {
        // Neither [in] nor [out] is [in].
        let flags = if flags & (PARAM_IN | PARAM_OUT) == 0 {
            PARAM_IN
        } else {
            flags & (PARAM_IN | PARAM_OUT)
        };
        Param { kind, flags }
    }

    pub fn is_in(&self) -> bool {
        self.flags & PARAM_IN != 0
    }

    pub fn is_out(&self) -> bool {
        self.flags & PARAM_OUT != 0
    }
}

/// A method: its parameters after `this`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Method {
    pub params: Vec<Param>,
}

impl Method {
    /// The bytes of its arguments on the stack, `this` included: what
    /// it takes off the stack when it returns.
    pub fn stack(&self) -> u32 {
        4 + self.params.iter().map(|p| p.kind.size()).sum::<u32>()
    }
}

fn method(params: Vec<(Kind, u16)>) -> Option<Method> {
    Some(Method {
        params: params.into_iter().map(|(k, f)| Param::new(k, f)).collect(),
    })
}

fn guid_in() -> (Kind, u16) {
    (Kind::Ptr(Box::new(Kind::Record(16))), PARAM_IN)
}

/// IUnknown's methods.
pub fn unknown() -> Vec<Option<Method>> {
    vec![
        method(vec![
            guid_in(),
            (Kind::Ptr(Box::new(Kind::IidIs(0))), PARAM_OUT),
        ]),
        method(vec![]),
        method(vec![]),
    ]
}

/// IDispatch's methods, after IUnknown's.
fn dispatch() -> Vec<Option<Method>> {
    let mut m = unknown();
    let out = |k: Kind| (Kind::Ptr(Box::new(k)), PARAM_OUT);
    // GetTypeInfoCount(count).
    m.push(method(vec![out(Kind::Word)]));
    // GetTypeInfo(index, lcid, info): the type information goes as an
    // interface that isn't marshalled; rpc.rs answers it in the caller.
    m.push(method(vec![
        (Kind::Word, PARAM_IN),
        (Kind::Word, PARAM_IN),
        out(Kind::Unsupported),
    ]));
    // GetIDsOfNames(iid, names, count, lcid, ids).
    m.push(method(vec![
        guid_in(),
        (Kind::Names(2), PARAM_IN),
        (Kind::Word, PARAM_IN),
        (Kind::Word, PARAM_IN),
        (Kind::Ids(2), PARAM_OUT),
    ]));
    // Invoke(id, iid, lcid, flags, params, result, exception, argument).
    m.push(method(vec![
        (Kind::Word, PARAM_IN),
        guid_in(),
        (Kind::Word, PARAM_IN),
        (Kind::Word, PARAM_IN),
        (Kind::DispParams, PARAM_IN | PARAM_OUT),
        out(Kind::Variant),
        (Kind::ExcepInfo, PARAM_OUT),
        out(Kind::Word),
    ]));
    m
}

/// IClassFactory's methods.
fn class_factory() -> Vec<Option<Method>> {
    let mut m = unknown();
    // CreateInstance(outer, iid, out).
    m.push(method(vec![
        (Kind::Iface(IID_IUNKNOWN), PARAM_IN),
        guid_in(),
        (Kind::Ptr(Box::new(Kind::IidIs(1))), PARAM_OUT),
    ]));
    // LockServer(lock).
    m.push(method(vec![(Kind::Word, PARAM_IN)]));
    m
}

/// The methods of the interface `iid`, by their places in its vtable
/// (None for one that can't be copied), or None when the marshaler
/// can't copy calls of it: it isn't one COM knows and has no type
/// library registered, or its library says it isn't for OLE
/// Automation.
pub fn methods(cpu: &mut Cpu, iid: &Guid) -> Option<Vec<Option<Method>>> {
    match *iid {
        IID_IUNKNOWN => return Some(unknown()),
        IID_IDISPATCH => return Some(dispatch()),
        IID_ICLASSFACTORY => return Some(class_factory()),
        _ => {}
    }
    let base = format!("Interface\\{}", iid);
    let libid = registry::string_value(cpu, HKEY_CLASSES_ROOT, &format!("{base}\\TypeLib"), "")?;
    let libid = Guid::parse(&libid)?;
    let version = registry::string_value(
        cpu,
        HKEY_CLASSES_ROOT,
        &format!("{base}\\TypeLib"),
        "Version",
    );
    let (major, minor) =
        version
            .as_deref()
            .and_then(|v| v.split_once('.'))
            .map_or((1, 0), |(a, b)| {
                (
                    u16::from_str_radix(a, 16).unwrap_or(1),
                    u16::from_str_radix(b, 16).unwrap_or(0),
                )
            });
    let path = typelib::query_path(cpu, &libid, major, minor, 0).ok()?;
    let (lib, _) = typelib::load(cpu, &path).ok()?;
    let mut held = vec![lib];
    let m = typelib::model(cpu, lib);
    let found = m.find_guid(iid).and_then(|index| {
        let mut slots = Vec::new();
        collect(cpu, lib, index as u32, &mut slots, &mut held, 0).then_some(slots)
    });
    for lib in held {
        typelib::release_lib(cpu, lib);
    }
    found
}

/// Put the methods of the type at `index` of `lib` into `slots`, those
/// of the interfaces it inherits from first. False when calls of it
/// can't be copied. The libraries it takes a reference to go into
/// `held`.
fn collect(
    cpu: &mut Cpu,
    lib: u32,
    index: u32,
    slots: &mut Vec<Option<Method>>,
    held: &mut Vec<u32>,
    depth: usize,
) -> bool {
    let m = typelib::model(cpu, lib);
    let t = &m.infos[index as usize];
    if t.guid == IID_IUNKNOWN {
        *slots = unknown();
        return true;
    }
    if t.guid == IID_IDISPATCH || (t.kind == TKIND_DISPATCH && !t.is_dual()) {
        *slots = dispatch();
        return true;
    }
    if depth > 16 || !matches!(t.kind, TKIND_INTERFACE | TKIND_DISPATCH) {
        return false;
    }
    let Some(base) = t.impls.first() else {
        return false;
    };
    let href = if m.format == Format::Sltg {
        base.href
    } else {
        base.href | HREF_INTERFACE
    };
    let Ok((base_lib, base_index, _)) = typelib::resolve_in(cpu, lib, index, href) else {
        return false;
    };
    held.push(base_lib);
    if !collect(cpu, base_lib, base_index, slots, held, depth + 1) {
        return false;
    }
    for f in &m.infos[index as usize].funcs {
        let slot = (f.vft as u16 as usize & !3) / 4;
        let params = f
            .params
            .iter()
            .map(|p| Param::new(kind_of(cpu, lib, index, &p.ty, held), p.flags))
            .collect();
        if slots.len() <= slot {
            slots.resize(slot + 1, None);
        }
        slots[slot] = Some(Method { params });
    }
    true
}

/// What a parameter of the type `ty` of the type at `index` of `lib`
/// holds.
fn kind_of(cpu: &mut Cpu, lib: u32, index: u32, ty: &TypeDesc, held: &mut Vec<u32>) -> Kind {
    kind_and_type(cpu, lib, index, ty, held).0
}

/// `kind_of`'s answer, and whether `ty` is an interface's own type
/// (IFoo, or an alias of it) rather than a pointer to one.
fn kind_and_type(
    cpu: &mut Cpu,
    lib: u32,
    index: u32,
    ty: &TypeDesc,
    held: &mut Vec<u32>,
) -> (Kind, bool) {
    let kind = match ty {
        TypeDesc::Base(vt) => base_kind(*vt),
        TypeDesc::SafeArray(_) => Kind::Array,
        TypeDesc::CArray(..) => Kind::Unsupported,
        TypeDesc::Ptr(inner) => match kind_and_type(cpu, lib, index, inner, held) {
            // A type library names an interface pointer as a pointer to
            // the interface's type (IFoo*): that pointer is the value.
            // A pointer to an interface pointer (IFoo**, VT_UNKNOWN*)
            // points to a value.
            (Kind::Iface(iid), true) => Kind::Iface(iid),
            (Kind::Unsupported, _) => Kind::Unsupported,
            (k, _) => Kind::Ptr(Box::new(k)),
        },
        TypeDesc::User(href) => {
            let Ok((l, i, _)) = typelib::resolve_in(cpu, lib, index, *href) else {
                return (Kind::Unsupported, false);
            };
            held.push(l);
            let m2 = typelib::model(cpu, l);
            let t = &m2.infos[i as usize];
            return match t.kind {
                TKIND_ENUM => (Kind::Word, false),
                TKIND_RECORD | TKIND_UNION if plain(t) => (Kind::Record(t.size), false),
                TKIND_INTERFACE | TKIND_DISPATCH => (Kind::Iface(t.guid), true),
                TKIND_COCLASS => (Kind::Iface(IID_IUNKNOWN), true),
                TKIND_ALIAS => match t.alias.clone() {
                    Some(alias) => kind_and_type(cpu, l, i, &alias, held),
                    None => (Kind::Unsupported, false),
                },
                _ => (Kind::Unsupported, false),
            };
        }
    };
    (kind, false)
}

/// Whether the structure `t` holds no pointers, so its bytes can be
/// copied as they are.
fn plain(t: &TypeInfo) -> bool {
    let scalar = |ty: &TypeDesc| matches!(ty, TypeDesc::Base(vt) if matches!(base_kind(*vt), Kind::Word | Kind::Quad));
    t.vars.iter().all(|v| match &v.ty {
        TypeDesc::CArray(inner, _) => scalar(inner),
        ty => scalar(ty),
    })
}

/// What a parameter of the base type `vt` holds.
fn base_kind(vt: u16) -> Kind {
    match vt {
        VT_I2 | VT_I4 | 4 | VT_ERROR | VT_BOOL | VT_I1 | VT_UI1 | VT_UI2 | VT_UI4 | VT_INT
        | VT_UINT | VT_HRESULT => Kind::Word,
        // VT_R8, VT_CY, VT_DATE, VT_I8, VT_UI8.
        5 | 6 | 7 | 20 | 21 => Kind::Quad,
        VT_BSTR => Kind::Bstr,
        VT_LPSTR => Kind::Str,
        VT_LPWSTR => Kind::WStr,
        VT_VARIANT => Kind::Variant,
        VT_UNKNOWN => Kind::Iface(IID_IUNKNOWN),
        VT_DISPATCH => Kind::Iface(IID_IDISPATCH),
        _ => Kind::Unsupported,
    }
}
