//! COM objects of the built-in DLLs: DirectDraw's, DirectSound's and
//! DirectInput's interfaces, and the class factories OLE32 hands out.
//!
//! A pointer to an interface, as the program holds it, is the address of
//! a block in the process heap: its first word points at the interface's
//! vtable in the DLL's code section (`stubs::Vtable`), whose entries are
//! the DLL's stubs; its second is the object's id in the DLL's own table
//! of objects, where the object's state is. A DLL may keep more words of
//! its own after those (`extra`). One object with several interfaces has
//! a block for each, with the same id, as DirectDraw's objects have
//! separate reference counts for their interfaces.
//!
//! Classes the program can create with CoCreateInstance or a DLL's
//! DllGetClassObject are in `CLASSES`, which each DLL adds its own to.

use super::stubs;
use crate::cpu::Cpu;

/// HRESULTs.
pub const S_OK: u32 = 0;
pub const S_FALSE: u32 = 1;
pub const E_NOTIMPL: u32 = 0x8000_4001;
pub const E_NOINTERFACE: u32 = 0x8000_4002;
pub const E_POINTER: u32 = 0x8000_4003;
pub const E_OUTOFMEMORY: u32 = 0x8007_000E;
pub const E_INVALIDARG: u32 = 0x8007_0057;
pub const CLASS_E_NOAGGREGATION: u32 = 0x8004_0110;
pub const CLASS_E_CLASSNOTAVAILABLE: u32 = 0x8004_0111;
pub const REGDB_E_CLASSNOTREG: u32 = 0x8004_0154;
pub const CO_E_NOTINITIALIZED: u32 = 0x8004_01F0;

/// A GUID as it is in memory: Data1 to Data3 little-endian, then Data4.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Guid(pub [u8; 16]);

impl Guid {
    pub const fn new(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Guid {
        let a = d1.to_le_bytes();
        let b = d2.to_le_bytes();
        let c = d3.to_le_bytes();
        Guid([
            a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4],
            d4[5], d4[6], d4[7],
        ])
    }

    /// The GUID at `at`, if it can be read.
    pub fn read(cpu: &mut Cpu, at: u32) -> Option<Guid> {
        if at == 0 || !cpu.bus.guest_probe(at, 16, false) {
            return None;
        }
        let mut bytes = [0; 16];
        cpu.bus.guest_read_bytes(at, &mut bytes);
        Some(Guid(bytes))
    }

    /// Write it at `at`; false if it can't be written there.
    pub fn write(&self, cpu: &mut Cpu, at: u32) -> bool {
        if at == 0 || !cpu.bus.guest_probe(at, 16, true) {
            return false;
        }
        cpu.bus.guest_write_bytes(at, &self.0);
        true
    }

    /// The GUID of the string "{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}",
    /// with its braces.
    pub fn parse(s: &str) -> Option<Guid> {
        let inner = s.strip_prefix('{')?.strip_suffix('}')?;
        let parts: Vec<&str> = inner.split('-').collect();
        let lens = [8, 4, 4, 4, 12];
        if parts.len() != 5 || parts.iter().zip(lens).any(|(p, n)| p.len() != n) {
            return None;
        }
        if !inner.chars().all(|c| c == '-' || c.is_ascii_hexdigit()) {
            return None;
        }
        let d1 = u32::from_str_radix(parts[0], 16).ok()?;
        let d2 = u16::from_str_radix(parts[1], 16).ok()?;
        let d3 = u16::from_str_radix(parts[2], 16).ok()?;
        let tail = format!("{}{}", parts[3], parts[4]);
        let mut d4 = [0; 8];
        for (i, byte) in d4.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&tail[2 * i..2 * i + 2], 16).ok()?;
        }
        Some(Guid::new(d1, d2, d3, d4))
    }
}

impl std::fmt::Display for Guid {
    /// As the registry has it: "{6BA57C31-...}", in capitals.
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let g = &self.0;
        write!(
            f,
            "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
            u32::from_le_bytes([g[0], g[1], g[2], g[3]]),
            u16::from_le_bytes([g[4], g[5]]),
            u16::from_le_bytes([g[6], g[7]]),
            g[8],
            g[9],
            g[10],
            g[11],
            g[12],
            g[13],
            g[14],
            g[15]
        )
    }
}

pub const GUID_NULL: Guid = Guid([0; 16]);
pub const IID_IUNKNOWN: Guid = Guid::new(0, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
pub const IID_ICLASSFACTORY: Guid = Guid::new(1, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);

/// Bytes of an interface's block that are com.rs's: the vtable and the
/// id.
pub const HEADER: u32 = 8;

/// A new interface of the object `id`: a block of `HEADER + extra` bytes,
/// its vtable the DLL `dll`'s vtable `vtable`, the rest zeroed. Its
/// address, or None when the heap is full.
pub fn new_interface(cpu: &mut Cpu, dll: usize, vtable: usize, id: u32, extra: u32) -> Option<u32> {
    let size = HEADER + extra;
    let at = super::with_process(cpu, |p, bus| p.heap.alloc(&mut p.memory, bus, size))?;
    cpu.bus.guest_fill(at, size as usize, 0);
    cpu.bus
        .guest_write_32(at, stubs::vtable_address(dll, vtable));
    cpu.bus.guest_write_32(at + 4, id);
    Some(at)
}

/// Give the block of the interface `this` back to the heap.
pub fn free_interface(cpu: &mut Cpu, this: u32) {
    // Its vtable no longer, so a call through a pointer the program kept
    // fails as one to an object that is gone.
    if cpu.bus.guest_probe(this, 4, true) {
        cpu.bus.guest_write_32(this, 0);
    }
    super::with_process(cpu, |p, _| p.heap.free(this));
}

/// The interface `this` is, if it is one of the DLL `dll`'s: its vtable's
/// place in the DLL's list, and the object's id.
pub fn interface_of(cpu: &mut Cpu, dll: usize, this: u32) -> Option<(usize, u32)> {
    if this == 0 || !cpu.bus.guest_probe(this, HEADER as usize, false) {
        return None;
    }
    let vtable = cpu.bus.guest_read_32(this);
    let index =
        (0..stubs::DLLS[dll].vtables.len()).find(|&i| stubs::vtable_address(dll, i) == vtable)?;
    Some((index, cpu.bus.guest_read_32(this + 4)))
}

/// Store `value` at the pointer `out` the program passed; false if it
/// can't be written there.
pub fn write_out(cpu: &mut Cpu, out: u32, value: u32) -> bool {
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return false;
    }
    cpu.bus.guest_write_32(out, value);
    true
}

/// A class the program can create: by its CLSID, with `create`, which
/// makes an object of it and stores the interface `iid` asks for at
/// `out` (`create(cpu, iid, out)`), returning an HRESULT.
pub struct Class {
    pub clsid: Guid,
    /// Its DLL, as `stubs::DLLS` names it.
    pub dll: &'static str,
    pub name: &'static str,
    pub create: fn(&mut Cpu, &Guid, u32) -> u32,
}

/// The classes of the built-in DLLs.
pub static CLASSES: &[&[Class]] = &[
    super::dinput::CLASSES,
    super::ddraw::CLASSES,
    super::dsound::CLASSES,
    super::dplayx::CLASSES,
    super::dmusic::CLASSES,
    super::shell32::CLASSES,
    super::dinput::CLASSES8,
];

/// The class with CLSID `clsid`, and its place in the list of all.
pub fn find_class(clsid: &Guid) -> Option<(usize, &'static Class)> {
    CLASSES
        .iter()
        .flat_map(|c| c.iter())
        .enumerate()
        .find(|(_, c)| c.clsid == *clsid)
}

/// The class at `index` in the list of all.
pub fn class_at(index: usize) -> Option<&'static Class> {
    CLASSES.iter().flat_map(|c| c.iter()).nth(index)
}
