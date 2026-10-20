//! Window classes: the program's (RegisterClassA) and the system's
//! (Button, Edit, the dialog class and the rest), and what
//! GetClassLongA reads.
//!
//! A class is found by its name or atom, and for a local class by the
//! module that registered it; global classes (CS_GLOBALCLASS) and the
//! system's are found from any module. Class atoms come from USER's atom
//! table, which RegisterWindowMessageA's message ids come from too, as on
//! Windows.

use super::{
    ERROR_CLASS_ALREADY_EXISTS, ERROR_CLASS_DOES_NOT_EXIST, ERROR_CLASS_HAS_WINDOWS,
    ERROR_INVALID_INDEX, ERROR_INVALID_PARAMETER, ERROR_INVALID_WINDOW_HANDLE, FALSE, Name, State,
    TRUE, address_of, current_pid, read_words, set_error, string_at, user, write_words,
};
use crate::cpu::Cpu;

pub const CS_VREDRAW: u32 = 0x0001;
pub const CS_HREDRAW: u32 = 0x0002;
pub const CS_DBLCLKS: u32 = 0x0008;
pub const CS_OWNDC: u32 = 0x0020;
pub const CS_CLASSDC: u32 = 0x0040;
pub const CS_PARENTDC: u32 = 0x0080;
pub const CS_NOCLOSE: u32 = 0x0200;
pub const CS_SAVEBITS: u32 = 0x0800;
pub const CS_GLOBALCLASS: u32 = 0x4000;

/// GetClassLongA's indices.
const GCL_MENUNAME: i32 = -8;
const GCL_HBRBACKGROUND: i32 = -10;
const GCL_HCURSOR: i32 = -12;
const GCL_HICON: i32 = -14;
const GCL_HMODULE: i32 = -16;
const GCL_CBWNDEXTRA: i32 = -18;
const GCL_CBCLSEXTRA: i32 = -20;
const GCL_WNDPROC: i32 = -24;
const GCL_STYLE: i32 = -26;
const GCW_ATOM: i32 = -32;
const GCL_HICONSM: i32 = -34;

/// The most extra bytes a class or its windows may have on Windows 95.
const MAX_EXTRA: u32 = 40;

#[derive(Clone, Debug, Default)]
pub struct Class {
    pub atom: u16,
    pub name: String,
    pub style: u32,
    pub proc: u32,
    pub extra: Vec<u8>,
    pub wnd_extra: u32,
    pub instance: u32,
    pub icon: u32,
    pub icon_small: u32,
    pub cursor: u32,
    pub background: u32,
    /// The menu's name as the program gave it: a number, or the address
    /// of its string.
    pub menu_name: u32,
    pub global: bool,
    /// The process that registered it, or 0 for the system's.
    pub pid: u32,
    pub system: bool,
    /// How many windows it has.
    pub windows: u32,
    /// The DC its windows share (CS_CLASSDC).
    pub dc: u32,
}

/// The atom of `name` in USER's atom table, added if it isn't there.
pub fn atom(state: &mut State, name: &str) -> u16 {
    if let Some(i) = state
        .registered
        .iter()
        .position(|n| n.eq_ignore_ascii_case(name))
    {
        return 0xC000 + i as u16;
    }
    state.registered.push(name.to_string());
    0xC000 + state.registered.len() as u16 - 1
}

/// The atom of `name`, if it is in the table.
pub fn find_atom(state: &State, name: &str) -> Option<u16> {
    state
        .registered
        .iter()
        .position(|n| n.eq_ignore_ascii_case(name))
        .map(|i| 0xC000 + i as u16)
}

/// Whether the class at `index` is the one `name` names.
pub fn matches(state: &State, index: u32, name: &Name) -> bool {
    let Some(class) = state.classes.get(index as usize).and_then(|c| c.as_ref()) else {
        return false;
    };
    match name {
        Name::Id(atom) => class.atom == *atom,
        Name::Text(text) => class.name.eq_ignore_ascii_case(text),
    }
}

/// The class of the window `hwnd`, if both are there.
pub fn of(state: &State, hwnd: u32) -> Option<&Class> {
    let w = state.window(hwnd)?;
    state.classes.get(w.class as usize)?.as_ref()
}

/// The style of the window's class (CS_*), or 0.
pub fn style_of(state: &State, hwnd: u32) -> u32 {
    of(state, hwnd).map_or(0, |c| c.style)
}

/// Whether the window `hwnd` is of the class called `name`.
pub fn is_class(state: &State, hwnd: u32, name: &str) -> bool {
    state
        .window(hwnd)
        .is_some_and(|w| matches(state, w.class, &Name::Text(name.to_string())))
}

/// The class `name` that a window of the module `instance` in the process
/// `pid` gets: a local class of that module, a global class, a system
/// class; Windows 95 then takes a local class of another module of the
/// process.
pub fn find(state: &State, name: &Name, instance: u32, pid: u32) -> Option<u32> {
    let found = |f: &dyn Fn(&Class) -> bool| {
        state
            .classes
            .iter()
            .position(|c| c.as_ref().is_some_and(f))
            .map(|i| i as u32)
    };
    let named = |c: &Class| match name {
        Name::Id(atom) => c.atom == *atom,
        Name::Text(text) => c.name.eq_ignore_ascii_case(text),
    };
    let instance = if instance == 0 {
        main_module(pid)
    } else {
        instance
    };
    found(&|c| named(c) && !c.global && !c.system && c.pid == pid && c.instance == instance)
        .or_else(|| found(&|c| named(c) && c.global && c.pid == pid))
        .or_else(|| found(&|c| named(c) && c.system))
        .or_else(|| found(&|c| named(c) && c.global))
        .or_else(|| found(&|c| named(c) && c.pid == pid))
}

/// The program's module, which instance 0 means: there is one process
/// at a time that registers classes in practice, so this is its base.
fn main_module(_pid: u32) -> u32 {
    0
}

/// The system class `name`'s place.
pub fn find_system(state: &State, name: &str) -> Option<u32> {
    state
        .classes
        .iter()
        .position(|c| {
            c.as_ref()
                .is_some_and(|c| c.system && c.name.eq_ignore_ascii_case(name))
        })
        .map(|i| i as u32)
}

/// The system's classes: name, style, window procedure, extra bytes,
/// cursor (an IDC_ id), background (a COLOR_ index + 1, or 0).
const SYSTEM: &[(&str, u32, &str, u32, u32, u32)] = &[
    (
        "Button",
        CS_DBLCLKS | CS_VREDRAW | CS_HREDRAW | CS_PARENTDC,
        "ButtonWndProc",
        8,
        32512,
        0,
    ),
    ("Edit", CS_DBLCLKS | CS_PARENTDC, "EditWndProc", 6, 32513, 0),
    (
        "Static",
        CS_DBLCLKS | CS_PARENTDC,
        "StaticWndProc",
        8,
        32512,
        0,
    ),
    (
        "ListBox",
        CS_DBLCLKS | CS_PARENTDC,
        "ListBoxWndProc",
        4,
        32512,
        0,
    ),
    (
        "ComboBox",
        CS_DBLCLKS | CS_PARENTDC | CS_VREDRAW | CS_HREDRAW,
        "ComboBoxWndProc",
        4,
        32512,
        0,
    ),
    (
        "ComboLBox",
        CS_DBLCLKS | CS_SAVEBITS,
        "ComboLBoxWndProc",
        4,
        32512,
        0,
    ),
    (
        "ScrollBar",
        CS_DBLCLKS | CS_VREDRAW | CS_HREDRAW | CS_PARENTDC,
        "ScrollBarWndProc",
        4,
        32512,
        0,
    ),
    ("MDIClient", 0, "MDIClientWndProc", 4, 32512, 13),
    (
        "#32770",
        CS_SAVEBITS | CS_DBLCLKS,
        "DefDlgProcA",
        30,
        32512,
        0,
    ),
    (
        "#32768",
        CS_SAVEBITS | CS_DBLCLKS,
        "MenuWndProc",
        4,
        32512,
        0,
    ),
    ("#32769", CS_DBLCLKS, "DesktopWndProc", 0, 32512, 2),
    ("#32772", 0, "IconTitleWndProc", 0, 32512, 0),
];

/// Register the system's classes.
pub fn register_system(cpu: &mut Cpu) {
    for &(name, style, proc, extra, cursor, background) in SYSTEM {
        let atom = match name.strip_prefix('#') {
            Some(n) => n.parse().unwrap_or(0),
            None => atom(user(cpu), name),
        };
        let class = Class {
            atom,
            name: name.to_string(),
            style: style | CS_GLOBALCLASS,
            proc: address_of(proc),
            wnd_extra: extra,
            instance: super::DLL.base,
            cursor: super::resource::system_cursor(cpu, cursor),
            background,
            global: true,
            system: true,
            ..Class::default()
        };
        user(cpu).classes.push(Some(class));
    }
}

/// RegisterClassA(class): its atom.
pub fn register_class(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let w = read_words(cpu, args[0], 10);
    if cpu.bus.guest_faulted() {
        return None;
    }
    register(cpu, &w, 0)
}

/// RegisterClassExA(class): its atom.
pub fn register_class_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let w = read_words(cpu, args[0], 12);
    if cpu.bus.guest_faulted() {
        return None;
    }
    if w[0] != 48 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    register(cpu, &w[1..11], w[11])
}

/// Register the class WNDCLASSA `w` describes, with the small icon
/// `icon_small`.
fn register(cpu: &mut Cpu, w: &[u32], icon_small: u32) -> Option<u32> {
    super::init(cpu);
    let &[
        style,
        proc,
        cls_extra,
        wnd_extra,
        instance,
        icon,
        cursor,
        background,
        menu,
        name,
    ] = w
    else {
        unreachable!()
    };
    let name = match Name::read(cpu, name)? {
        Name::Text(text) => text,
        Name::Id(id) => {
            // A class may be registered under an atom it has already.
            match user(cpu).registered.get(id.wrapping_sub(0xC000) as usize) {
                Some(n) => n.clone(),
                None => {
                    set_error(cpu, ERROR_INVALID_PARAMETER);
                    return Some(0);
                }
            }
        }
    };
    if cls_extra > MAX_EXTRA || wnd_extra > MAX_EXTRA {
        // Windows 95 takes no more, and says nothing; NT fails.
        if (cls_extra as i32) < 0 || (wnd_extra as i32) < 0 {
            set_error(cpu, ERROR_INVALID_PARAMETER);
            return Some(0);
        }
    }
    if instance == super::DLL.base {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    let pid = current_pid(cpu);
    let instance = if instance == 0 {
        crate::win32::running(cpu).map_or(0, |p| p.image_base)
    } else {
        instance
    };
    let global = style & CS_GLOBALCLASS != 0;
    let state = user(cpu);
    let exists = state.classes.iter().flatten().any(|c| {
        c.name.eq_ignore_ascii_case(&name)
            && if global {
                (c.global && (c.pid == pid || c.system))
                    || (c.pid == pid && !c.system && c.instance == instance)
            } else {
                c.pid == pid && !c.system && (c.global || c.instance == instance)
            }
    });
    if exists {
        set_error(cpu, ERROR_CLASS_ALREADY_EXISTS);
        return Some(0);
    }
    let atom = atom(state, &name);
    let class = Class {
        atom,
        name,
        style,
        proc,
        extra: vec![0; cls_extra.min(0x1000) as usize],
        wnd_extra: wnd_extra.min(0x1000),
        instance,
        icon,
        icon_small,
        cursor,
        background,
        menu_name: menu,
        global,
        pid,
        ..Class::default()
    };
    match state.classes.iter().position(|c| c.is_none()) {
        Some(i) => state.classes[i] = Some(class),
        None => state.classes.push(Some(class)),
    }
    Some(atom as u32)
}

/// The class an Unregister or GetClassInfo call names: the module's
/// local one, or a global or system one.
fn named(cpu: &mut Cpu, name: u32, instance: u32) -> Option<Option<u32>> {
    let name = Name::read(cpu, name)?;
    let pid = current_pid(cpu);
    let instance = if instance == 0 {
        crate::win32::running(cpu).map_or(0, |p| p.image_base)
    } else {
        instance
    };
    let state = user(cpu);
    let named = |c: &Class| match &name {
        Name::Id(atom) => c.atom == *atom,
        Name::Text(text) => c.name.eq_ignore_ascii_case(text),
    };
    let found = |f: &dyn Fn(&Class) -> bool| {
        state
            .classes
            .iter()
            .position(|c| c.as_ref().is_some_and(|c| named(c) && f(c)))
            .map(|i| i as u32)
    };
    Some(
        found(&|c| !c.global && !c.system && c.pid == pid && c.instance == instance)
            .or_else(|| found(&|c| c.global && !c.system && c.pid == pid))
            .or_else(|| found(&|c| c.system)),
    )
}

/// UnregisterClassA(name, instance).
pub fn unregister_class(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    super::init(cpu);
    let Some(index) = named(cpu, args[0], args[1])? else {
        set_error(cpu, ERROR_CLASS_DOES_NOT_EXIST);
        return Some(FALSE);
    };
    let state = user(cpu);
    if state.classes[index as usize]
        .as_ref()
        .is_some_and(|c| c.windows > 0)
    {
        set_error(cpu, ERROR_CLASS_HAS_WINDOWS);
        return Some(FALSE);
    }
    state.classes[index as usize] = None;
    Some(TRUE)
}

/// GetClassInfoA(instance, name, class): TRUE, with the WNDCLASSA.
pub fn get_class_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    super::init(cpu);
    let Some(index) = named(cpu, args[1], args[0])? else {
        set_error(cpu, ERROR_CLASS_DOES_NOT_EXIST);
        return Some(FALSE);
    };
    let c = user(cpu).classes[index as usize].clone().unwrap();
    let words = [
        c.style & !CS_GLOBALCLASS,
        c.proc,
        c.extra.len() as u32,
        c.wnd_extra,
        if c.system { args[0] } else { c.instance },
        c.icon,
        c.cursor,
        c.background,
        c.menu_name,
        args[1],
    ];
    write_words(cpu, args[2], &words);
    Some(TRUE)
}

/// GetClassInfoExA(instance, name, class): the WNDCLASSEXA.
pub fn get_class_info_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    super::init(cpu);
    let Some(index) = named(cpu, args[1], args[0])? else {
        set_error(cpu, ERROR_CLASS_DOES_NOT_EXIST);
        return Some(FALSE);
    };
    let c = user(cpu).classes[index as usize].clone().unwrap();
    let words = [
        48,
        c.style & !CS_GLOBALCLASS,
        c.proc,
        c.extra.len() as u32,
        c.wnd_extra,
        if c.system { args[0] } else { c.instance },
        c.icon,
        c.cursor,
        c.background,
        c.menu_name,
        args[1],
        c.icon_small,
    ];
    write_words(cpu, args[2], &words);
    Some(c.atom as u32)
}

/// The class of the window `hwnd`'s place, or the last error set.
fn class_of(cpu: &mut Cpu, hwnd: u32) -> Option<u32> {
    match user(cpu).window(hwnd) {
        Some(w) => Some(w.class),
        None => {
            set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
            None
        }
    }
}

/// GetClassLongA(hwnd, index).
pub fn get_class_long(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(index) = class_of(cpu, args[0]) else {
        return Some(0);
    };
    let Some(c) = user(cpu).classes.get(index as usize).cloned().flatten() else {
        return Some(0);
    };
    let value = match args[1] as i32 {
        GCL_MENUNAME => c.menu_name,
        GCL_HBRBACKGROUND => c.background,
        GCL_HCURSOR => c.cursor,
        GCL_HICON => c.icon,
        GCL_HMODULE => c.instance,
        GCL_CBWNDEXTRA => c.wnd_extra,
        GCL_CBCLSEXTRA => c.extra.len() as u32,
        GCL_WNDPROC => c.proc,
        GCL_STYLE => {
            c.style & !CS_GLOBALCLASS
                | if c.global && !c.system {
                    CS_GLOBALCLASS
                } else {
                    0
                }
        }
        GCW_ATOM => c.atom as u32,
        GCL_HICONSM => c.icon_small,
        i if i >= 0 && (i as usize + 4) <= c.extra.len() => {
            let i = i as usize;
            u32::from_le_bytes(c.extra[i..i + 4].try_into().unwrap())
        }
        _ => {
            set_error(cpu, ERROR_INVALID_INDEX);
            0
        }
    };
    Some(value)
}

/// SetClassLongA(hwnd, index, value): the old value.
pub fn set_class_long(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(index) = class_of(cpu, args[0]) else {
        return Some(0);
    };
    let value = args[2];
    let state = user(cpu);
    let Some(c) = state
        .classes
        .get_mut(index as usize)
        .and_then(|c| c.as_mut())
    else {
        return Some(0);
    };
    let old = match args[1] as i32 {
        GCL_MENUNAME => std::mem::replace(&mut c.menu_name, value),
        GCL_HBRBACKGROUND => std::mem::replace(&mut c.background, value),
        GCL_HCURSOR => std::mem::replace(&mut c.cursor, value),
        GCL_HICON => std::mem::replace(&mut c.icon, value),
        GCL_HMODULE => std::mem::replace(&mut c.instance, value),
        GCL_CBWNDEXTRA => std::mem::replace(&mut c.wnd_extra, value.min(0x1000)),
        GCL_WNDPROC => std::mem::replace(&mut c.proc, value),
        GCL_STYLE => {
            let old = c.style;
            c.style = value;
            old
        }
        GCL_HICONSM => std::mem::replace(&mut c.icon_small, value),
        GCL_CBCLSEXTRA => {
            set_error(cpu, ERROR_INVALID_PARAMETER);
            return Some(0);
        }
        i if i >= 0 && (i as usize + 4) <= c.extra.len() => {
            let i = i as usize;
            let old = u32::from_le_bytes(c.extra[i..i + 4].try_into().unwrap());
            c.extra[i..i + 4].copy_from_slice(&value.to_le_bytes());
            old
        }
        _ => {
            set_error(cpu, ERROR_INVALID_INDEX);
            return Some(0);
        }
    };
    Some(old)
}

/// GetClassWord(hwnd, index): the extra bytes, or GCW_ATOM.
pub fn get_class_word(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(index) = class_of(cpu, args[0]) else {
        return Some(0);
    };
    let Some(c) = user(cpu).classes.get(index as usize).cloned().flatten() else {
        return Some(0);
    };
    Some(match args[1] as i32 {
        GCW_ATOM => c.atom as u32,
        i if i >= 0 && (i as usize + 2) <= c.extra.len() => {
            let i = i as usize;
            u16::from_le_bytes(c.extra[i..i + 2].try_into().unwrap()) as u32
        }
        i if i < 0 => get_class_long(cpu, args)? & 0xFFFF,
        _ => {
            set_error(cpu, ERROR_INVALID_INDEX);
            0
        }
    })
}

/// SetClassWord(hwnd, index, value).
pub fn set_class_word(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(index) = class_of(cpu, args[0]) else {
        return Some(0);
    };
    let state = user(cpu);
    let Some(c) = state
        .classes
        .get_mut(index as usize)
        .and_then(|c| c.as_mut())
    else {
        return Some(0);
    };
    let i = args[1] as i32;
    if i >= 0 && (i as usize + 2) <= c.extra.len() {
        let i = i as usize;
        let old = u16::from_le_bytes(c.extra[i..i + 2].try_into().unwrap()) as u32;
        c.extra[i..i + 2].copy_from_slice(&(args[2] as u16).to_le_bytes());
        return Some(old);
    }
    set_error(cpu, ERROR_INVALID_INDEX);
    Some(0)
}

/// GetClassNameA(hwnd, buffer, size): the length copied.
pub fn get_class_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(index) = class_of(cpu, args[0]) else {
        return Some(0);
    };
    let name = user(cpu)
        .classes
        .get(index as usize)
        .and_then(|c| c.as_ref())
        .map(|c| c.name.clone())
        .unwrap_or_default();
    let bytes = crate::dosstr::to_bytes(&name);
    Some(super::copy_out(cpu, args[1], args[2], &bytes))
}

#[allow(dead_code)]
fn unused(cpu: &mut Cpu, at: u32) -> Option<String> {
    string_at(cpu, at)
}

crate::state_fields!(Class {
    atom,
    name,
    style,
    proc,
    extra,
    wnd_extra,
    instance,
    icon,
    icon_small,
    cursor,
    background,
    menu_name,
    global,
    pid,
    system,
    windows,
    dc
});
