//! The order of the built-in function tables, which saved states depend
//! on.
//!
//! A built-in DLL's function traps with its id: the DLL's place in
//! `stubs::DLLS` and the function's in the DLL's table (`stubs::id`), and
//! its stub sits at that place in the DLL's image. A 16-bit system
//! module's functions are the same with `win16::builtin::MODULES`. A
//! saved state keeps the stubs in memory with their ids, and the program
//! keeps their addresses in its import table and wherever it put what
//! GetProcAddress gave it. An entry put before others moves every later
//! entry to the next id and the next stub, so a state saved before it
//! calls the function in front of the one it was bound to. The tables
//! therefore only grow at their ends: `stub_tables.txt` holds the order
//! each table has had, and `tables_only_grow_at_their_ends` fails on an
//! entry inserted, moved or taken out. It holds the DLLs' variables the
//! same way, with their sizes: programs keep their addresses too, and a
//! variable put before others or grown moves every later one.
//!
//! Entries went into the middle of tables before that test was there.
//! Those merged before the W32 section's version 71 (2026-11-15) can't
//! reach a state this rust-dos loads, since it refuses older versions.
//! The ones merged since are in `INSERTED`. A state saved before one of
//! them has an image of the table in an older order, and often a shorter
//! one, since functions were appended after it too. Its program holds
//! addresses in that order, and so does what the older build's Rust left
//! in the state: the system's window classes' procedures, scripts and
//! steps of tasks, return addresses and exception handlers on stacks.
//!
//! So the image keeps its order when the state loads (`mend`): each of
//! its stubs traps with the id its function has now, the functions it
//! lacks get stubs after its code, and the process records the image's
//! order (`OlderImage`). Every address Rust works out for a function of
//! such a DLL follows the image of the running process (`address`), and a
//! process started after the load gets the same image (`shared`): the
//! system's window classes, which all processes share, hold its
//! addresses, as Windows 9x has one image of its system DLLs for all
//! processes. The 16-bit system modules are mended the same way through
//! their entry tables (`win16::builtin::mend_order`).

use super::{Process, TRAP, loader, stubs};
use crate::bus::Bus;
use crate::cpu::Cpu;
use crate::savestate::{Result, StateError};
use std::collections::HashMap;

/// What a table is the table of: a built-in DLL, by its file name, or a
/// 16-bit system module, by its module name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Of {
    Dll(&'static str),
    Module(&'static str),
}

/// Entries one change put into the middle of a table: images made before
/// it don't have them, and have each entry after one of them a place
/// earlier.
struct Inserted {
    of: Of,
    names: &'static [&'static str],
}

/// The entries put into the middle of tables since the W32 section's
/// version 71 (93c3c549, #1143, 2026-11-15 21:45 +0200), in the order
/// main merged them. An image of a table has the table's groups up to
/// some point and none after it.
const INSERTED: &[Inserted] = &[
    // 4606b228 (#1173), 2026-11-16 08:34.
    Inserted {
        of: Of::Dll("KERNEL32.DLL"),
        names: &["LoadModule"],
    },
    // 565d1c81 (#1237), 2026-11-17 00:45.
    Inserted {
        of: Of::Dll("USER32.DLL"),
        names: &[
            "DlgDirListW",
            "DlgDirSelectExW",
            "DlgDirListComboBoxW",
            "DlgDirSelectComboBoxExW",
        ],
    },
    Inserted {
        of: Of::Module("USER"),
        names: &["DlgDirSelectComboBox"],
    },
    // d34ee52d (#1231), 2026-11-17 02:25.
    Inserted {
        of: Of::Dll("USER32.DLL"),
        names: &["DdeQueryConvInfo", "DdemlWndProc"],
    },
    // 236f17e3 (#1497), 2026-11-20 00:09.
    Inserted {
        of: Of::Dll("USER32.DLL"),
        names: &[
            "CloseDesktop",
            "CloseWindowStation",
            "CreateDesktopA",
            "CreateDesktopW",
            "CreateWindowStationA",
            "CreateWindowStationW",
            "EnumDesktopWindows",
            "EnumDesktopsA",
            "EnumDesktopsW",
            "EnumWindowStationsA",
            "EnumWindowStationsW",
            "GetInputDesktop",
            "GetUserObjectInformationW",
            "GetUserObjectSecurity",
            "LockWindowStation",
            "OpenDesktopA",
            "OpenDesktopW",
            "OpenInputDesktop",
            "OpenWindowStationA",
            "OpenWindowStationW",
            "SetProcessWindowStation",
            "SetThreadDesktop",
            "SetUserObjectInformationA",
            "SetUserObjectInformationW",
            "SetUserObjectSecurity",
            "SwitchDesktop",
            "UnlockWindowStation",
        ],
    },
    // 73150cd8 (#1498), 2026-11-20 00:10.
    Inserted {
        of: Of::Dll("USER32.DLL"),
        names: &["ChangeDisplaySettingsExA"],
    },
    // 63e70b5d (#1500), 2026-11-20 00:12.
    Inserted {
        of: Of::Dll("GDI32.DLL"),
        names: &["CreateScalableFontResourceA"],
    },
];

/// Whether `mend` looks at the images of the table `of`: those that
/// had entries put into their middle.
pub(crate) fn mended(of: Of) -> bool {
    INSERTED.iter().any(|e| e.of == of)
}

/// The orders an image of the table `of`, whose entries are `names` now,
/// may have, each as the places in the table now of its entries: the
/// table's own first, then without the last group merged into it, without
/// the last two, and so on.
fn orders(of: Of, names: &[&str]) -> Vec<Vec<usize>> {
    let groups: Vec<&Inserted> = INSERTED.iter().filter(|e| e.of == of).collect();
    (0..=groups.len())
        .rev()
        .map(|kept| {
            let without: Vec<&str> = groups[kept..]
                .iter()
                .flat_map(|g| g.names.iter().copied())
                .collect();
            (0..names.len())
                .filter(|&i| !without.contains(&names[i]))
                .collect()
        })
        .collect()
}

/// What `image_order` reads of an image of a table.
pub(crate) struct Image<'a> {
    pub of: Of,
    /// The table's entries now.
    pub names: &'a [&'static str],
    /// The place of the stub each export points at, by the export's name.
    /// An export that points elsewhere, as one a program hooked by
    /// rewriting it does, isn't in it.
    pub exported: HashMap<Vec<u8>, usize>,
    /// The bytes of the stub at a place.
    pub stub: &'a dyn Fn(usize) -> Option<[u8; 8]>,
    /// The id of a place in the table now, which the stub there traps
    /// with.
    pub id: &'a dyn Fn(usize) -> u16,
    /// Whether the function at a place in the table now is code its stub
    /// jumps to (`stubs::Conv::Code`).
    pub code: &'a dyn Fn(usize) -> bool,
}

/// The image's order: the place in the table now of the function of each
/// of its stubs, as many as it has. None when no order the table had puts
/// its exports where they are.
pub(crate) fn image_order(image: &Image) -> Option<Vec<usize>> {
    let mut order = orders(image.of, image.names)
        .into_iter()
        .find(|order| fits(image, order))?;
    // It has the stubs up to its last export, and those after it that
    // are stubs of the order's functions: the functions appended after
    // the image was made, and those not exported, aren't in it.
    let exported = |(p, &api): (usize, &usize)| {
        let name = image.names[api].as_bytes();
        (!(image.code)(api) && image.exported.get(name) == Some(&p)).then_some(p + 1)
    };
    let mut len = order
        .iter()
        .enumerate()
        .filter_map(exported)
        .max()
        .unwrap_or(0);
    while len < order.len() && is_stub_of(image, len, order[len]) {
        len += 1;
    }
    order.truncate(len);
    Some(order)
}

/// Whether the image's exports are where `order` has their functions, and
/// it exports none `order` lacks. The export of a function given as code
/// points at its code, which follows the image's stubs: in an image with
/// fewer stubs than the table has now, that is where a later stub would
/// be now, so it says nothing of the order.
fn fits(image: &Image, order: &[usize]) -> bool {
    let mut place = vec![None; image.names.len()];
    for (p, &api) in order.iter().enumerate() {
        place[api] = Some(p);
    }
    image.names.iter().enumerate().all(|(api, name)| {
        (image.code)(api)
            || image
                .exported
                .get(name.as_bytes())
                .is_none_or(|&at| place[api] == Some(at))
    })
}

/// Whether the image's stub at place `p` is that of the function `api`
/// of the table now: a trap with the place's id, as the build that made
/// the image wrote it, or with the function's, as `mend` leaves it; or a
/// jump to the function's code.
fn is_stub_of(image: &Image, p: usize, api: usize) -> bool {
    match (image.stub)(p) {
        Some(b) if b[0] == 0xFE && b[1] == TRAP => {
            let id = u16::from_le_bytes([b[2], b[3]]);
            id == (image.id)(p) || id == (image.id)(api)
        }
        Some(b) if b[0] == 0xE9 => (image.code)(api),
        _ => false,
    }
}

/// Whether `mend` rewrites the stub that starts with `bytes`, at a place
/// whose id is `then`, of a function whose id is `now`: a trap with the
/// place's id that isn't the function's, as the build that made the image
/// wrote it. One `mend` rewrote already traps with `now`.
pub(crate) fn rewrites(bytes: &[u8], then: u16, now: u16) -> bool {
    let then = then.to_le_bytes();
    then != now.to_le_bytes() && bytes[..4] == [0xFE, TRAP, then[0], then[1]]
}

/// A process's image of a built-in DLL made from an older table, which
/// keeps that table's order (`mend`).
#[derive(Clone, Debug, Default)]
pub(crate) struct OlderImage {
    dll: usize,
    /// The address of the stub of each function of the table now, by its
    /// place there: None when the image has none for it.
    stubs: Vec<Option<u32>>,
}

/// An older image a state that was loaded had, which processes started
/// since get too: its bytes as the load left them.
#[derive(Clone, Debug, Default)]
pub(crate) struct SharedImage {
    older: OlderImage,
    bytes: Vec<u8>,
}

/// The address of the stub of function `api` of the built-in DLL `dll` in
/// `process`: where `stubs::stub_address` puts it, or where the process's
/// older image has it. None when that image has none for it.
pub(crate) fn stub_address_in(process: &Process, dll: usize, api: usize) -> Option<u32> {
    stub_in(process.older_images.iter(), dll, api)
}

fn stub_in<'a>(
    mut images: impl Iterator<Item = &'a OlderImage>,
    dll: usize,
    api: usize,
) -> Option<u32> {
    match images.find(|o| o.dll == dll) {
        Some(older) => older.stubs.get(api).copied().flatten(),
        None => Some(stubs::stub_address(dll, api)),
    }
}

/// The stub of the function `wanted` of the built-in DLL at `base` in the
/// process's older image of it, which lacks the function and so doesn't
/// export it: `mend` gave it a stub after the image's code, and a program
/// bound or a GetProcAddress made after the load gets that, as it would
/// get the function of the image of the table now. None when the process
/// has no older image of that DLL or no stub for the function in it, and
/// for a function the running version of Windows doesn't export.
pub(crate) fn lacking_export(
    bus: &Bus,
    process: &Process,
    base: u32,
    wanted: &loader::Wanted,
) -> Option<u32> {
    let d = stubs::DLLS.iter().position(|dll| dll.base == base)?;
    let older = process.older_images.iter().find(|o| o.dll == d)?;
    let dll = stubs::DLLS[d];
    let api = dll.apis.iter().position(|a| {
        a.ordinal != 0
            && match wanted {
                loader::Wanted::Name(name) => a.name == name,
                loader::Wanted::Ordinal(ordinal) => a.ordinal == *ordinal,
            }
    })?;
    if !super::version::exports(bus.win32.os, dll.name, dll.apis[api].name) {
        return None;
    }
    older.stubs.get(api).copied().flatten()
}

/// The address of the stub of function `api` of the built-in DLL `dll`,
/// for Rust to call or to give the program: in the running process's
/// image, or, with no process running, in the image a process gets (both
/// are those of the state that was loaded, if it had an older one). 0
/// when that image has no stub for it, which a call faults on.
pub(crate) fn address(cpu: &Cpu, dll: usize, api: usize) -> u32 {
    match super::running(cpu) {
        Some(process) => stub_address_in(process, dll, api).unwrap_or(0),
        None => address_for_new(&cpu.bus, dll, api),
    }
}

/// The address of the stub of function `api` of the built-in DLL `dll` in
/// the image a process started now gets (`shared`). 0 when that image has
/// no stub for it.
pub(crate) fn address_for_new(bus: &Bus, dll: usize, api: usize) -> u32 {
    let shared = bus.win32.older_images.iter().map(|s| &s.older);
    stub_in(shared, dll, api).unwrap_or(0)
}

/// The address of the stub of function `api` of the built-in DLL `dll`,
/// one of the system's own stubs, which Rust works out where no process
/// is at hand. The place is before every place an entry went into the
/// table, so every image of it, older or not, has the stub there.
pub(crate) fn fixed_address(dll: usize, api: usize) -> u32 {
    debug_assert!(
        api < first_inserted(dll),
        "{} has an entry inserted before {api}",
        stubs::DLLS[dll].name
    );
    stubs::stub_address(dll, api)
}

/// The first place in the table of the built-in DLL `dll` that an entry of
/// `INSERTED` went into: where older images start to differ.
fn first_inserted(dll: usize) -> usize {
    let d = stubs::DLLS[dll];
    INSERTED
        .iter()
        .filter(|e| e.of == Of::Dll(d.name))
        .flat_map(|e| e.names)
        .filter_map(|name| d.apis.iter().position(|a| a.name == *name))
        .min()
        .unwrap_or(d.apis.len())
}

/// The older image of the built-in DLL `dll` a process that maps it gets
/// in place of the one `stubs::image` builds, if the state that was loaded
/// had one: its bytes.
pub(crate) fn shared(bus: &Bus, dll: usize) -> Option<&[u8]> {
    let shared = bus.win32.older_images.iter().find(|s| s.older.dll == dll)?;
    Some(&shared.bytes)
}

/// The process's image of the built-in DLL `dll` was written again
/// (`loader::attach_builtin`), from the shared older image if there is one
/// (`shared`), else from the table as it is now.
pub(crate) fn attached(bus: &Bus, process: &mut Process, dll: usize) {
    process.older_images.retain(|o| o.dll != dll);
    if let Some(shared) = bus.win32.older_images.iter().find(|s| s.older.dll == dll) {
        process.older_images.push(shared.older.clone());
    }
}

/// Mend the images of the built-in DLLs and 16-bit system modules that a
/// state just loaded has from before an entry of `INSERTED`, or from
/// before functions were appended to their tables: each stub of such an
/// image traps with the id its function has now, and the image gets stubs
/// for the functions it lacks after its code. An image whose stubs are
/// where no table had them fails the load, where it would call other
/// functions than its program was bound to.
pub(crate) fn mend(bus: &mut Bus) -> Result<()> {
    bus.win32.older_images.clear();
    for i in 0..bus.win32.processes.len() {
        let mut process = std::mem::take(&mut bus.win32.processes[i]);
        let mended = mend_process(bus, &mut process);
        bus.win32.processes[i] = process;
        mended?;
    }
    crate::win16::builtin::mend_order(bus)
}

fn mend_process(bus: &mut Bus, process: &mut Process) -> Result<()> {
    process.older_images.clear();
    for (d, dll) in stubs::DLLS.iter().enumerate() {
        let module = process
            .modules
            .iter()
            .find(|m| m.base == dll.base && m.flags & loader::BUILTIN != 0);
        let (Some(module), true) = (module, mended(Of::Dll(dll.name))) else {
            continue;
        };
        let size = module.size;
        let names: Vec<&'static str> = dll.apis.iter().map(|a| a.name).collect();
        let order = {
            let (bus, memory) = (&*bus, &process.memory);
            let stub = |p: usize| {
                let bytes = loader::peek(bus, memory, stubs::stub_address(d, p), 8)?;
                bytes.try_into().ok()
            };
            image_order(&Image {
                of: Of::Dll(dll.name),
                names: &names,
                exported: exported(bus, memory, d),
                stub: &stub,
                id: &|api| stubs::id(d, api),
                code: &|api| !dll.apis[api].conv.is_trap(),
            })
        };
        let Some(order) = order else {
            return Err(StateError::Invalid(format!(
                "{}'s image in {} has its functions in an order this rust-dos doesn't \
                 know, and its stubs would call other functions than the program's",
                dll.name, process.name
            )));
        };
        if order.len() == dll.apis.len() {
            continue;
        }
        let older = OlderImage {
            dll: d,
            stubs: mend_image(bus, process, d, &order),
        };
        if !bus.win32.older_images.iter().any(|s| s.older.dll == d)
            && let Some(mut bytes) = loader::peek(bus, &process.memory, dll.base, size as usize)
        {
            unpatched(&mut bytes, d, &older);
            bus.win32.older_images.push(SharedImage {
                older: older.clone(),
                bytes,
            });
        }
        process.older_images.push(older);
    }
    Ok(())
}

/// Give each function of the copy `bytes` of an older image of the
/// built-in DLL `d` that traps the stub the image builder writes for it,
/// with its id now. The copy is of a process's image, whose program may
/// have hooked functions by writing jumps over their stubs: a process
/// started later would jump into memory it doesn't have, where on main it
/// gets an image of its own, as the builder made it.
fn unpatched(bytes: &mut [u8], d: usize, older: &OlderImage) {
    let dll = stubs::DLLS[d];
    for (api, function) in dll.apis.iter().enumerate() {
        let Some(at) = older.stubs.get(api).copied().flatten() else {
            continue;
        };
        let at = (at - dll.base) as usize;
        if function.conv.is_trap()
            && let Some(stub) = bytes.get_mut(at..at + stubs::STUB as usize)
        {
            let id = stubs::id(d, api);
            stub.copy_from_slice(&stubs::stub(id, function.args, function.conv));
        }
    }
}

/// The place of the stub each export of the image of the built-in DLL `d`
/// points at, by the export's name.
fn exported(bus: &Bus, memory: &super::memory::Memory, d: usize) -> HashMap<Vec<u8>, usize> {
    let dll = stubs::DLLS[d];
    let first = stubs::stub_address(d, 0);
    let stubs = first..stubs::stub_address(d, dll.apis.len());
    loader::exports(bus, memory, dll.base)
        .into_iter()
        .filter(|(_, at)| stubs.contains(at) && (at - first).is_multiple_of(stubs::STUB))
        .map(|(name, at)| (name, ((at - first) / stubs::STUB) as usize))
        .collect()
}

/// Mend the process's image of the DLL `d`, whose stubs are those of the
/// functions `order` (places in the table now): the address of each
/// function's stub in it.
fn mend_image(bus: &mut Bus, process: &Process, d: usize, order: &[usize]) -> Vec<Option<u32>> {
    let dll = stubs::DLLS[d];
    let mut found = vec![None; dll.apis.len()];
    for (p, &api) in order.iter().enumerate() {
        let at = stubs::stub_address(d, p);
        if let Some(bytes) = loader::peek(bus, &process.memory, at, 4)
            && rewrites(&bytes, stubs::id(d, p), stubs::id(d, api))
        {
            process
                .memory
                .write(bus, at + 2, &stubs::id(d, api).to_le_bytes());
        }
        found[api] = Some(at);
    }
    // The functions it lacks: stubs after its code, in the rest of the
    // code section's last page, where nothing else is. A function given
    // as code is never lacking (`no_image_lacks_a_function_given_as_code`).
    let Some((mut at, end)) = free_space(bus, &process.memory, dll.base) else {
        return found;
    };
    for (api, function) in dll.apis.iter().enumerate() {
        if found[api].is_some() || !function.conv.is_trap() {
            continue;
        }
        if at + stubs::STUB > end {
            break;
        }
        let stub = stubs::stub(stubs::id(d, api), function.args, function.conv);
        match loader::peek(bus, &process.memory, at, stubs::STUB as usize) {
            Some(bytes) if bytes == stub => found[api] = Some(at),
            Some(bytes) if bytes.iter().all(|&b| b == 0) => {
                process.memory.write(bus, at, &stub);
                found[api] = Some(at);
            }
            _ => {}
        }
        at += stubs::STUB;
    }
    found
}

/// Where the code in the first section of the image at `base`, the one
/// with the stubs, ends, and where the section's last page does.
fn free_space(bus: &Bus, memory: &super::memory::Memory, base: u32) -> Option<(u32, u32)> {
    let header = base.wrapping_add(loader::peek_32(bus, memory, base + 0x3C)?);
    let optional = loader::peek(bus, memory, header + 20, 2)?;
    let section = header + 24 + u16::from_le_bytes([optional[0], optional[1]]) as u32;
    let size = loader::peek_32(bus, memory, section + 8)?;
    let start = base + loader::peek_32(bus, memory, section + 12)?;
    Some((
        start + size.next_multiple_of(stubs::STUB),
        start + size.next_multiple_of(0x1000),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::win16::builtin::MODULES;
    use std::path::PathBuf;

    /// A table: what it is the table of ("DLL GDI32.DLL" or "MODULE
    /// GDI"), and its entries' names in order.
    type Table = (String, Vec<String>);

    /// The tables as they are: the built-in DLLs' in `DLLS` order, then
    /// the 16-bit system modules' in `MODULES` order.
    fn tables_now() -> Vec<Table> {
        let dlls = stubs::DLLS.iter().map(|dll| {
            let names = dll.apis.iter().map(|a| a.name.to_string()).collect();
            (format!("DLL {}", dll.name), names)
        });
        let modules = MODULES.iter().map(|m| {
            let names = m.apis.iter().map(|a| a.name.to_string()).collect();
            (format!("MODULE {}", m.name), names)
        });
        let vars = stubs::DLLS
            .iter()
            .filter(|dll| !dll.vars.is_empty())
            .map(|dll| {
                let vars = dll.vars.iter().map(|v| format!("{} {}", v.name, v.size));
                (format!("VARS {}", dll.name), vars.collect())
            });
        dlls.chain(modules).chain(vars).collect()
    }

    fn file() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/win32/stub_tables.txt")
    }

    /// The tables `text`, as `write` has them.
    fn read(text: &str) -> Vec<Table> {
        let mut tables: Vec<Table> = Vec::new();
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if KINDS.iter().any(|kind| line.starts_with(kind)) {
                tables.push((line.to_string(), Vec::new()));
            } else {
                let table = tables.last_mut().expect("a name before its table's line");
                table.1.push(line.to_string());
            }
        }
        tables
    }

    fn write(tables: &[Table]) -> String {
        let mut text = String::from(
            "# The order of the built-in DLLs' function tables (src/win32/stubs.rs)\n\
             # and the 16-bit system modules' (src/win16/builtin.rs), which saved\n\
             # states keep the stubs of, and of the DLLs' variables with their\n\
             # sizes: src/win32/stub_tables.rs. A new function, variable or table\n\
             # goes at the end of its list, never in the middle.\n",
        );
        for (table, names) in tables {
            text.push('\n');
            text.push_str(table);
            text.push('\n');
            for name in names {
                text.push_str(name);
                text.push('\n');
            }
        }
        text
    }

    /// The kinds of tables: the DLLs' functions in `DLLS` order, the
    /// modules' in `MODULES` order, and the variables of the DLLs that
    /// have them, by their DLL's name.
    const KINDS: [&str; 3] = ["DLL ", "MODULE ", "VARS "];

    /// Why the tables `now` aren't `recorded` with entries added at the
    /// ends of tables and new tables at the ends of their lists: the
    /// first entry or table that moved. None when they are.
    fn moved(recorded: &[Table], now: &[Table]) -> Option<String> {
        fn of(kind: &str, tables: &[Table]) -> Vec<Table> {
            tables
                .iter()
                .filter(|t| t.0.starts_with(kind))
                .cloned()
                .collect()
        }
        // A DLL's or module's place in its list is the high bits of its
        // functions' ids.
        for kind in &KINDS[..2] {
            let (recorded, now) = (of(kind, recorded), of(kind, now));
            for (i, table) in recorded.iter().enumerate() {
                let Some(table_now) = now.get(i) else {
                    return Some(format!(
                        "{} is gone, and the tables after it moved",
                        table.0
                    ));
                };
                if table_now.0 != table.0 {
                    return Some(format!(
                        "{} is where {} was: a DLL or module goes at the end of its list, \
                         and every one after one put in moves to another index",
                        table_now.0, table.0
                    ));
                }
                if let Some(why) = entry_moved(table, table_now) {
                    return Some(why);
                }
            }
        }
        // A DLL's variables follow its functions: their tables have no
        // place of their own.
        for table in of("VARS ", recorded) {
            let Some(table_now) = now.iter().find(|t| t.0 == table.0) else {
                return Some(format!("{} is gone", table.0));
            };
            if let Some(why) = entry_moved(&table, table_now) {
                return Some(why);
            }
        }
        None
    }

    /// Why the entries of `now` aren't those of `recorded` with others
    /// after them: the first one that moved.
    fn entry_moved((table, names): &Table, (_, names_now): &Table) -> Option<String> {
        let variables = table.starts_with("VARS ");
        for (j, name) in names.iter().enumerate() {
            match names_now.get(j) {
                Some(n) if n == name => {}
                Some(n) if variables => {
                    return Some(format!(
                        "{table}'s entry {j} was {name} and is now {n}: a variable put before \
                         others, or one grown, moves every one after it, and a saved state's \
                         program keeps their old addresses. Add variables at the end"
                    ));
                }
                Some(n) => {
                    return Some(format!(
                        "{table}'s entry {j} was {name} and is now {n}: a function put \
                         before others gives each of them the next id and stub, and a \
                         saved state's stubs then call the function before the one the \
                         program was bound to. Add functions at the end of the table"
                    ));
                }
                None => {
                    return Some(format!(
                        "{table}'s entry {j}, {name}, is gone: a saved state's program may \
                         still use it. Keep the entry"
                    ));
                }
            }
        }
        None
    }

    /// Run with RUST_DOS_BLESS_STUB_TABLES=1 to add new functions and
    /// tables to `stub_tables.txt`; it adds only what went at the ends.
    #[test]
    fn tables_only_grow_at_their_ends() {
        let recorded = read(&std::fs::read_to_string(file()).unwrap());
        let now = tables_now();
        if let Some(why) = moved(&recorded, &now) {
            panic!("{why}");
        }
        if recorded != now {
            if std::env::var_os("RUST_DOS_BLESS_STUB_TABLES").is_some() {
                std::fs::write(file(), write(&now)).unwrap();
                return;
            }
            panic!(
                "functions or tables were added at the ends of the tables: run this test \
                 with RUST_DOS_BLESS_STUB_TABLES=1 to add them to stub_tables.txt"
            );
        }
    }

    /// The tables before CreateScalableFontResourceA went into GDI32's,
    /// after AddFontResourceA: what the test says of the change that put
    /// it there.
    #[test]
    fn an_entry_inserted_into_a_table_is_found() {
        let now = tables_now();
        let mut before = now.clone();
        let gdi32 = before.iter_mut().find(|t| t.0 == "DLL GDI32.DLL").unwrap();
        gdi32.1.retain(|n| n != "CreateScalableFontResourceA");
        let why = moved(&before, &now).expect("the insertion is found");
        assert!(why.contains("GDI32.DLL's entry"), "{why}");
        assert!(why.contains("is now CreateScalableFontResourceA"), "{why}");
        // An entry added at the end of a table, a DLL at the end of
        // `DLLS` (before the modules in the file), a module at the end of
        // `MODULES`, and the variables of a DLL that had none aren't moves.
        let mut grown = now.clone();
        grown[4].1.push("Later".into());
        let modules = grown
            .iter()
            .position(|t| t.0.starts_with("MODULE "))
            .unwrap();
        grown.insert(modules, ("DLL LATER.DLL".into(), vec!["First".into()]));
        let vars = grown.iter().position(|t| t.0.starts_with("VARS ")).unwrap();
        grown.insert(vars, ("MODULE LATER".into(), vec!["First".into()]));
        grown.insert(vars + 1, ("VARS GDI32.DLL".into(), vec!["first 4".into()]));
        assert_eq!(moved(&now, &grown), None);
        // One taken out is.
        let mut shrunk = now.clone();
        shrunk[4].1.pop();
        assert!(moved(&now, &shrunk).unwrap().contains("is gone"));
        // So is a variable put in before others, or grown.
        let msvcrt = now.iter().position(|t| t.0 == "VARS MSVCRT.DLL").unwrap();
        let mut put_in = now.clone();
        put_in[msvcrt].1.insert(1, "early 4".into());
        assert!(
            moved(&now, &put_in)
                .unwrap()
                .contains("a variable put before")
        );
        let mut resized = now.clone();
        resized[msvcrt].1[0].push('0');
        assert!(
            moved(&now, &resized)
                .unwrap()
                .contains("a variable put before")
        );
    }

    #[test]
    fn the_file_reads_back_as_written() {
        let now = tables_now();
        assert_eq!(read(&write(&now)), now);
    }

    /// The tables main had since the W32 section's version 71 (93c3c549),
    /// from its history: what each was the table of, how many of the
    /// groups `INSERTED` has for it it had, how many entries it had, the
    /// FNV-1a hash of their names (each followed by a newline), and the
    /// first and last commits on main's first-parent line that had it.
    const HISTORY: &[(Of, usize, usize, u64, &str, &str)] = &[
        (
            Of::Dll("KERNEL32.DLL"),
            0,
            483,
            0xeefa2f0c0da3774d,
            "93c3c549",
            "33c3db42",
        ),
        (
            Of::Dll("KERNEL32.DLL"),
            1,
            484,
            0xd8ca52e6dfc330d3,
            "4606b228",
            "6a81ba40",
        ),
        (
            Of::Dll("KERNEL32.DLL"),
            1,
            486,
            0xe335494e32e8e12d,
            "93dba421",
            "eb0f7999",
        ),
        (
            Of::Dll("KERNEL32.DLL"),
            1,
            490,
            0xd0b02e527f5515ef,
            "321caa13",
            "5176d01a",
        ),
        (
            Of::Dll("USER32.DLL"),
            0,
            524,
            0xd598dce3a184900c,
            "93c3c549",
            "db0edee7",
        ),
        (
            Of::Dll("USER32.DLL"),
            1,
            528,
            0xc4b6667404c13bc4,
            "565d1c81",
            "86177249",
        ),
        (
            Of::Dll("USER32.DLL"),
            2,
            530,
            0x3950fa2b0f1a8ee8,
            "d34ee52d",
            "0a6f352a",
        ),
        (
            Of::Dll("USER32.DLL"),
            3,
            557,
            0xed4230076d72a73f,
            "236f17e3",
            "236f17e3",
        ),
        (
            Of::Module("USER"),
            0,
            306,
            0x93614185ceb5095f,
            "93c3c549",
            "db0edee7",
        ),
        (
            Of::Module("USER"),
            1,
            307,
            0xfa9970555897de52,
            "565d1c81",
            "4a0a7992",
        ),
        (
            Of::Dll("GDI32.DLL"),
            0,
            242,
            0x43f5fa73a538ea2e,
            "93c3c549",
            "a85dd9d3",
        ),
    ];

    /// The entries of the table `of` now.
    fn names_of(of: Of) -> Vec<&'static str> {
        match of {
            Of::Dll(name) => {
                let dll = stubs::DLLS[stubs::find_dll(name).unwrap()];
                dll.apis.iter().map(|a| a.name).collect()
            }
            Of::Module(name) => {
                let m = MODULES.iter().find(|m| m.name == name).unwrap();
                m.apis.iter().map(|a| a.name).collect()
            }
        }
    }

    /// The table main had with the first `kept` groups of `INSERTED` for
    /// `of` and `len` entries, as `orders` models it.
    fn modelled(of: Of, kept: usize, len: usize) -> Vec<&'static str> {
        let names = names_of(of);
        let groups = INSERTED.iter().filter(|e| e.of == of).count();
        let order = &orders(of, &names)[groups - kept];
        order[..len].iter().map(|&api| names[api]).collect()
    }

    fn fnv(names: &[&str]) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for name in names {
            for byte in name.bytes().chain([b'\n']) {
                hash = (hash ^ byte as u64).wrapping_mul(0x100_0000_01b3);
            }
        }
        hash
    }

    /// Every table main had since version 71 is one `orders` gives: the
    /// table now without the groups of `INSERTED` merged after it, cut to
    /// its length.
    #[test]
    fn older_tables_are_those_main_had() {
        for &(of, kept, len, hash, first, last) in HISTORY {
            let table = modelled(of, kept, len);
            assert_eq!(fnv(&table), hash, "{of:?} of {first} to {last}");
        }
        // The tables now are the last ones.
        for of in [
            Of::Dll("KERNEL32.DLL"),
            Of::Dll("USER32.DLL"),
            Of::Dll("GDI32.DLL"),
            Of::Module("USER"),
        ] {
            let names = names_of(of);
            assert_eq!(orders(of, &names)[0], (0..names.len()).collect::<Vec<_>>());
        }
    }

    /// Every entry of `INSERTED` is in its table, and a DLL with entries
    /// in it has no vtables or variables, which an older image would have
    /// elsewhere: `mend` mends the stubs only.
    #[test]
    fn inserted_entries_are_in_tables_mend_can_mend() {
        for e in INSERTED {
            let names = names_of(e.of);
            for name in e.names {
                assert!(names.contains(name), "{:?} {name}", e.of);
            }
            if let Of::Dll(name) = e.of {
                let dll = stubs::DLLS[stubs::find_dll(name).unwrap()];
                assert!(dll.vtables.is_empty() && dll.vars.is_empty(), "{name}");
            }
        }
    }

    /// No older table lacks a function given as code, which `mend` can't
    /// give a stub (it would need its code too): those functions are all
    /// in every table main had.
    #[test]
    fn no_image_lacks_a_function_given_as_code() {
        for &(of, kept, len, _, first, _) in HISTORY {
            let Of::Dll(name) = of else {
                continue;
            };
            let dll = stubs::DLLS[stubs::find_dll(name).unwrap()];
            let table = modelled(of, kept, len);
            for api in dll.apis.iter().filter(|a| !a.conv.is_trap()) {
                assert!(
                    table.contains(&api.name),
                    "{name} of {first} lacks {}",
                    api.name
                );
            }
        }
    }
}
