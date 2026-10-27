//! The global atom table: GlobalAddAtomA, GlobalFindAtomA,
//! GlobalDeleteAtom and GlobalGetAtomNameA. A string atom is a number
//! from C000h up that stands for a name, compared without case, and
//! lasts until it is deleted as many times as it was added. An integer
//! atom, below C000h, is its own value: MAKEINTATOM(n), or the name
//! "#n". Delphi's VCL adds atoms for its window properties when a
//! program starts. USER32's atoms of window messages, clipboard formats
//! and classes are a table of their own, as on Windows 9x.

use super::{ERROR_INVALID_PARAMETER, set_error, string_at};
use crate::cpu::Cpu;

const ERROR_FILE_NOT_FOUND: u32 = 2;
/// The first string atom.
const FIRST: u16 = 0xC000;
/// The longest name an atom may have.
const LONGEST: usize = 255;

/// A string atom: its name and how many times it was added.
#[derive(Clone, Debug, Default)]
pub struct Atom {
    name: String,
    count: u32,
}

/// The string atoms, by their atoms less C000h; None once deleted.
#[derive(Clone, Debug, Default)]
pub struct Table {
    slots: Vec<Option<Atom>>,
}

crate::state_fields!(Atom { name, count });
crate::state_fields!(Table { slots });

impl Table {
    fn find(&self, name: &str) -> Option<usize> {
        self.slots.iter().position(|s| {
            s.as_ref()
                .is_some_and(|a| a.name.eq_ignore_ascii_case(name))
        })
    }
}

fn table(cpu: &mut Cpu) -> &mut Table {
    &mut cpu.bus.win32.atoms
}

/// What an atom function's name argument is: an integer atom, or a name.
enum Name {
    Integer(u16),
    Text(String),
}

/// The name argument at `at`: MAKEINTATOM when its high word is 0, an
/// integer atom when the name is "#" and decimal digits. None, with the
/// error set, for 0, a number from C000h up, an empty or too long name,
/// or a name that isn't there.
fn name(cpu: &mut Cpu, at: u32) -> Option<Name> {
    let integer = |n: u32| (n != 0 && n < FIRST as u32).then_some(Name::Integer(n as u16));
    let found = if at >> 16 == 0 {
        integer(at)
    } else {
        let text = string_at(cpu, at).unwrap_or_default();
        match text.strip_prefix('#') {
            Some(digits) if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) => {
                digits.parse().ok().and_then(integer)
            }
            _ if text.is_empty() || crate::dosstr::to_bytes(&text).len() > LONGEST => None,
            _ => Some(Name::Text(text)),
        }
    };
    if found.is_none() {
        set_error(cpu, ERROR_INVALID_PARAMETER);
    }
    found
}

/// The atom of the string atom `name`, added if it isn't there and
/// counted once more if it is (Program Manager's DDE replies): 0 when
/// there is no room.
pub fn add(cpu: &mut Cpu, name: &str) -> u16 {
    let t = table(cpu);
    let slot = match t.find(name) {
        Some(i) => i,
        None => match t.slots.iter().position(Option::is_none) {
            Some(free) => free,
            None if t.slots.len() < 0x1_0000 - FIRST as usize => {
                t.slots.push(None);
                t.slots.len() - 1
            }
            None => return 0,
        },
    };
    let entry = t.slots[slot].get_or_insert(Atom {
        name: name.to_string(),
        count: 0,
    });
    entry.count += 1;
    FIRST + slot as u16
}

/// The name of the atom `atom`, "#" and the number for an integer atom;
/// None when there is no such atom.
pub fn name_of(cpu: &mut Cpu, atom: u16) -> Option<String> {
    if atom == 0 {
        None
    } else if atom < FIRST {
        Some(format!("#{}", atom))
    } else {
        let slots = &table(cpu).slots;
        slots
            .get((atom - FIRST) as usize)
            .and_then(|s| s.as_ref().map(|a| a.name.clone()))
    }
}

/// GlobalAddAtomA(name): the atom of `name`, added if it isn't there and
/// counted once more if it is; 0 when the name isn't one.
pub(super) fn global_add_atom(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let atom = match name(cpu, args[0]) {
        None => 0,
        Some(Name::Integer(n)) => n,
        Some(Name::Text(text)) => {
            let atom = add(cpu, &text);
            if atom == 0 {
                set_error(cpu, super::ERROR_NOT_ENOUGH_MEMORY);
            }
            atom
        }
    };
    Some(atom as u32)
}

/// GlobalFindAtomA(name): the atom of `name`, or 0 with
/// ERROR_FILE_NOT_FOUND when there is none.
pub(super) fn global_find_atom(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let atom = match name(cpu, args[0]) {
        None => 0,
        Some(Name::Integer(n)) => n,
        Some(Name::Text(text)) => match table(cpu).find(&text) {
            Some(i) => FIRST + i as u16,
            None => {
                set_error(cpu, ERROR_FILE_NOT_FOUND);
                0
            }
        },
    };
    Some(atom as u32)
}

/// GlobalDeleteAtom(atom): counts a string atom once less, and takes it
/// out at 0. 0 when it did, or for an integer atom; the atom when there
/// is no such atom.
pub(super) fn global_delete_atom(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let atom = args[0] & 0xFFFF;
    if atom < FIRST as u32 {
        return Some(0);
    }
    let slot = table(cpu).slots.get_mut((atom - FIRST as u32) as usize);
    match slot {
        Some(entry @ Some(_)) => {
            let atom = entry.as_mut().unwrap();
            atom.count -= 1;
            if atom.count == 0 {
                *entry = None;
            }
            Some(0)
        }
        _ => {
            set_error(cpu, ERROR_INVALID_PARAMETER);
            Some(atom)
        }
    }
}

/// GlobalGetAtomNameA(atom, buffer, size): the atom's name, "#" and the
/// number for an integer atom, cut to `size` - 1 characters and ended
/// with a NUL; its length, or 0 when there is no such atom or no room.
pub(super) fn global_get_atom_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (atom, buffer, size) = (args[0] as u16, args[1], args[2]);
    let Some(text) = name_of(cpu, atom).filter(|_| size > 0) else {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    };
    let mut bytes = crate::dosstr::to_bytes(&text);
    bytes.truncate(size as usize - 1);
    let len = bytes.len() as u32;
    bytes.push(0);
    cpu.bus.guest_write_bytes(buffer, &bytes);
    (!cpu.bus.guest_faulted()).then_some(len)
}
