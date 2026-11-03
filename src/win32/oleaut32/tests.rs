//! The type library readers and the views of their types, against what
//! Windows' own OLEAUT32 gives for the same files.
//!
//! testdata/gen_tlb.c wrote the two fixtures, one library in both
//! formats, with Windows' writers (CreateTypeLib2 for MSFT,
//! CreateTypeLib for SLTG); testdata/dump_tlb.c printed what 32-bit
//! OLEAUT32 reads from them into the .txt files, the help file's path
//! replaced by F. The test prints the same from the model and the views
//! typelib.rs gives the program, and compares. GetIDsOfNames' lines are
//! left out: the guest's runs of the tool check those.

use super::model::*;
use super::typelib::{View, bases_of, names_func, shape};
use super::{msft, sltg, stdole};
use std::fmt::Write;
use std::sync::Arc;

/// The type an HREFTYPE leads to, as the test finds it: the libraries
/// the fixtures import are STDOLE's.
type Found = (Arc<TypeLib>, usize, bool);

fn resolve_in(m: &Arc<TypeLib>, index: usize, href: u32) -> Option<Found> {
    let interface_bit = m.format != Format::Sltg && href & HREF_INTERFACE != 0;
    match m.find_ref(index, href)? {
        Ref::Local(i) => Some((m.clone(), i, interface_bit && m.infos[i].is_dual())),
        Ref::Import { lib, target } => {
            let import = &m.imports[lib];
            assert_eq!(import.guid, stdole::LIBID);
            let other = Arc::new(stdole::library(import.major));
            let i = match target {
                Target::Index(n) => n as usize,
                Target::Guid(g) => other.find_guid(&g)?,
            };
            let dual = other.infos.get(i)?.is_dual();
            Some((other, i, interface_bit && dual))
        }
    }
}

/// A type with the interfaces its dispinterface side inherits from.
struct Type {
    lib: Arc<TypeLib>,
    index: usize,
    interface: bool,
    bases: Vec<(Arc<TypeLib>, usize)>,
}

impl Type {
    fn new(lib: Arc<TypeLib>, index: usize, interface: bool) -> Type {
        let dual = lib.infos[index].is_dual();
        let interface = interface && dual;
        let bases = if dual && !interface {
            bases_of(lib.clone(), index, |m, i| {
                let first = m.infos[i].impls.first()?;
                resolve_in(m, i, first.href).map(|(l, i, _)| (l, i))
            })
        } else {
            Vec::new()
        };
        Type {
            lib,
            index,
            interface,
            bases,
        }
    }

    fn view(&self) -> View {
        shape(self.lib.clone(), self.index, self.interface, &self.bases)
    }

    /// GetRefTypeInfo's type.
    fn resolve(&self, href: u32) -> Option<Type> {
        let t = &self.lib.infos[self.index];
        let (lib, index, interface) = if (href as i32) < 0 {
            if !t.is_dual() {
                return None;
            }
            match self.bases.last() {
                Some((lib, index))
                    if self.lib.format == Format::Sltg
                        && href == 0xFFFF_FFFE
                        && !self.interface =>
                {
                    (lib.clone(), *index, false)
                }
                _ => (self.lib.clone(), self.index, !self.interface),
            }
        } else {
            let level = (href >> 24) as usize;
            match self.bases.get(level.wrapping_sub(1)) {
                Some((lib, index)) if !self.interface => resolve_in(lib, *index, href & 0xFF_FFFF)?,
                _ => resolve_in(&self.lib, self.index, href)?,
            }
        };
        Some(Type::new(lib, index, interface))
    }

    fn name(&self) -> String {
        self.lib.infos[self.index].name.clone().unwrap_or_default()
    }
}

fn string(what: &str, s: &Option<String>) -> String {
    match s {
        Some(s) => format!(" {what}=\"{s}\""),
        None => format!(" {what}=null"),
    }
}

/// The help file, its path replaced by F as in the expectations.
fn file(m: &TypeLib) -> &'static str {
    if m.help_file.is_some() {
        " file=F"
    } else {
        " file=null"
    }
}

fn td(t: &Type, d: &TypeDesc) -> String {
    match d {
        TypeDesc::Base(vt) => vt.to_string(),
        TypeDesc::Ptr(inner) => format!("ptr({})", td(t, inner)),
        TypeDesc::SafeArray(inner) => format!("safearray({})", td(t, inner)),
        TypeDesc::CArray(inner, bounds) => format!("carray[{}]({})", bounds.len(), td(t, inner)),
        TypeDesc::User(href) => match t.resolve(*href) {
            Some(r) => format!("user({href:x} {})", r.name()),
            None => format!("user({href:x})"),
        },
    }
}

fn value(v: &Value) -> String {
    match v {
        Value::Bits(vt, bits) => format!("vt{vt}:{}", *bits as u32 as i32),
        Value::Str(_) => format!("vt{VT_BSTR}:0"),
    }
}

/// What dump_tlb.c prints of the type `t`.
fn dump(out: &mut String, t: &Type, depth: usize) {
    let v = t.view();
    let i = v.info();
    let pad = " ".repeat(depth);
    let file = file(&v.lib);
    write!(
        out,
        "{pad}type {} kind={} guid={} flags={:x} ver={}.{} funcs={} vars={} impl={} vft={} \
         size={} align={} lcid={:x}{} ctx={}{}",
        i.name.as_deref().unwrap_or_default(),
        v.kind,
        i.guid,
        v.flags,
        i.major,
        i.minor,
        v.funcs.len(),
        i.vars.len(),
        v.impls.len(),
        v.vft,
        i.size,
        i.align,
        i.lcid,
        string("doc", &i.doc),
        i.help_context,
        file,
    )
    .unwrap();
    if let Some(alias) = &i.alias {
        write!(out, " alias={}", td(t, alias)).unwrap();
    }
    out.push('\n');
    for (n, imp) in v.impls.iter().enumerate() {
        write!(
            out,
            "{pad} impl {n} hr=0 href={:x} flags={}",
            imp.href, imp.flags
        )
        .unwrap();
        if let Some(r) = t.resolve(imp.href) {
            write!(out, " -> {}", r.name()).unwrap();
        }
        out.push('\n');
    }
    match v.other {
        Ok(href) => {
            write!(out, "{pad} impl -1 hr=0 href={href:x}").unwrap();
            if depth == 0
                && let Some(r) = t.resolve(href)
            {
                out.push('\n');
                dump(out, &r, 4);
            }
            out.push('\n');
        }
        Err(e) => writeln!(out, "{pad} impl -1 hr={e:x}").unwrap(),
    }
    for (n, f) in v.funcs.iter().enumerate() {
        // GetNames prefers a property's get method to its put methods.
        let first = names_func(&v.funcs, f.memid).unwrap();
        let names: Vec<String> = std::iter::once(first.name.clone())
            .chain(first.params.iter().map(|p| p.name.clone()))
            .map(Option::unwrap_or_default)
            .take(8)
            .collect();
        writeln!(
            out,
            "{pad} func {n} memid={:x} kind={} inv={} cc={} params={} opt={} vft={} flags={:x} \
             ret={} names={}:{}{}",
            f.memid,
            f.kind,
            f.invkind,
            f.callconv,
            f.params.len(),
            f.opt,
            f.vft,
            f.flags,
            td(t, &f.ret),
            names.len(),
            names.iter().map(|n| format!(" {n}")).collect::<String>(),
            string("doc", &first.doc),
        )
        .unwrap();
        for (j, p) in f.params.iter().enumerate() {
            write!(
                out,
                "{pad}  param {j} flags={:x} type={}",
                p.flags,
                td(t, &p.ty)
            )
            .unwrap();
            if p.flags & PARAMFLAG_FHASDEFAULT != 0
                && let Some(d) = &p.default
            {
                write!(out, " default={}", value(d)).unwrap();
            }
            out.push('\n');
        }
    }
    for (n, var) in i.vars.iter().enumerate() {
        write!(
            out,
            "{pad} var {n} memid={:x} kind={} flags={:x} name={} type={}",
            var.memid,
            var.kind,
            var.flags,
            var.name.as_deref().unwrap_or_default(),
            td(t, &var.ty)
        )
        .unwrap();
        match &var.value {
            VarValue::Const(c) => writeln!(out, " value={}", value(c)).unwrap(),
            VarValue::Offset(o) => writeln!(out, " oInst={o}").unwrap(),
        }
    }
}

fn dump_lib(m: TypeLib) -> String {
    let m = Arc::new(m);
    let file = file(&m);
    let mut out = format!(
        "lib {} guid={} lcid={:x} syskind={} ver={}.{} flags={:x}{} ctx={}{}\n",
        m.name.as_deref().unwrap_or_default(),
        m.guid,
        m.lcid,
        m.syskind,
        m.major,
        m.minor,
        m.flags,
        string("doc", &m.doc),
        m.help_context,
        file,
    );
    for index in 0..m.infos.len() {
        dump(&mut out, &Type::new(m.clone(), index, false), 0);
    }
    out
}

/// Windows' dump without GetIDsOfNames' lines.
fn expected(text: &str) -> String {
    text.lines()
        .filter(|l| !l.trim_start().starts_with("ids("))
        .map(|l| format!("{l}\n"))
        .collect()
}

#[test]
fn an_msft_library_reads_as_windows_reads_it() {
    let lib = msft::read(include_bytes!("testdata/rdtest-msft.tlb")).expect("an MSFT library");
    assert_eq!(lib.format, Format::Msft);
    let want = expected(include_str!("testdata/rdtest-msft.txt"));
    assert_eq!(dump_lib(lib), want);
}

#[test]
fn an_sltg_library_reads_as_windows_reads_it() {
    let lib = sltg::read(include_bytes!("testdata/rdtest-sltg.tlb")).expect("an SLTG library");
    assert_eq!(lib.format, Format::Sltg);
    let want = expected(include_str!("testdata/rdtest-sltg.txt"));
    assert_eq!(dump_lib(lib), want);
}

#[test]
fn neither_reader_takes_the_other_format() {
    assert!(msft::read(include_bytes!("testdata/rdtest-sltg.tlb")).is_none());
    assert!(sltg::read(include_bytes!("testdata/rdtest-msft.tlb")).is_none());
    assert!(msft::read(b"MSFT").is_none());
    assert!(sltg::read(b"SLTG").is_none());
}

#[test]
fn a_truncated_library_is_refused_without_a_panic() {
    for data in [
        &include_bytes!("testdata/rdtest-msft.tlb")[..],
        &include_bytes!("testdata/rdtest-sltg.tlb")[..],
    ] {
        for len in (0..data.len()).step_by(7) {
            let _ = msft::read(&data[..len]);
            let _ = sltg::read(&data[..len]);
        }
    }
}

#[test]
fn names_come_from_a_propertys_get_method() {
    let func = |invkind, param: Option<&str>| Func {
        memid: 1,
        name: Some("Rate".into()),
        doc: None,
        help_context: 0,
        kind: FUNC_PUREVIRTUAL,
        invkind,
        callconv: 4,
        opt: 0,
        vft: 0,
        flags: 0,
        ret: TypeDesc::Base(VT_HRESULT),
        params: vec![Param {
            name: param.map(Into::into),
            ty: TypeDesc::Base(5),
            flags: PARAMFLAG_FIN,
            default: None,
        }],
        entry: None,
    };
    let funcs = [
        func(INVOKE_PROPERTYPUT, None),
        func(INVOKE_PROPERTYGET, Some("pdRate")),
    ];
    assert_eq!(names_func(&funcs, 1), Some(&funcs[1]));
    assert_eq!(names_func(&funcs[..1], 1), Some(&funcs[0]));
    assert_eq!(names_func(&funcs, 2), None);
}
