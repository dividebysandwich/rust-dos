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
//! entry inserted, moved or taken out.
//!
//! One entry went into the middle of a table before that test was there:
//! GDI32's CreateScalableFontResourceA, after AddFontResourceA (#1583).
//! A process whose GDI32 image was made without it is mended when its
//! state loads (`mend`): its image keeps its order, each stub then traps
//! with the id its function has now, CreateScalableFontResourceA gets a
//! stub after the last one, and the addresses Rust works out for the
//! process's GDI32 functions follow the image (`stub_address_in`).

use super::{Process, TRAP, loader, stubs};
use crate::bus::Bus;
use crate::savestate::{Result, StateError};

/// An entry put into the middle of a built-in DLL's table: images made
/// before it don't have it, and their stubs from it on are one place
/// earlier.
struct Inserted {
    dll: &'static str,
    name: &'static str,
}

/// Every such entry. Nothing is added here: `stub_tables.txt` stops a
/// new one.
const INSERTED: &[Inserted] = &[Inserted {
    dll: "GDI32.DLL",
    name: "CreateScalableFontResourceA",
}];

/// A process's image of a built-in DLL made from an older table, which
/// keeps that table's order (`mend`).
#[derive(Clone, Debug, Default)]
pub(crate) struct OlderImage {
    dll: usize,
    /// The stub of each function of the table as it is now, by its place
    /// there: its place in the image, or None when the image has no stub
    /// for it.
    slots: Vec<Option<usize>>,
}

/// The address of the stub of function `api` of the built-in DLL `dll` in
/// `process`: where `stubs::stub_address` puts it, or where the process's
/// older image has it. None when that image has none for it.
pub(crate) fn stub_address_in(process: &Process, dll: usize, api: usize) -> Option<u32> {
    match process.older_images.iter().find(|o| o.dll == dll) {
        Some(older) => older
            .slots
            .get(api)
            .copied()
            .flatten()
            .map(|slot| stubs::stub_address(dll, slot)),
        None => Some(stubs::stub_address(dll, api)),
    }
}

/// Mend the images of the built-in DLLs that the processes of a state just
/// loaded have from before an entry of `INSERTED`: each stub of such an
/// image traps with the id its function has now, and the image gets stubs
/// for the functions it lacks after its last one. An image whose stubs
/// are neither where the table puts them nor where an older table did
/// fails the load, where it would call other functions than its program
/// was bound to.
pub(crate) fn mend(bus: &mut Bus) -> Result<()> {
    for i in 0..bus.win32.processes.len() {
        let mut process = std::mem::take(&mut bus.win32.processes[i]);
        let mended = mend_process(bus, &mut process);
        bus.win32.processes[i] = process;
        mended?;
    }
    Ok(())
}

fn mend_process(bus: &mut Bus, process: &mut Process) -> Result<()> {
    process.older_images.clear();
    for (d, dll) in stubs::DLLS.iter().enumerate() {
        let inserted: Vec<usize> = INSERTED
            .iter()
            .filter(|e| e.dll == dll.name)
            .map(|e| {
                dll.apis
                    .iter()
                    .position(|a| a.name == e.name)
                    .expect("an inserted entry is in its table")
            })
            .collect();
        let mapped = process
            .modules
            .iter()
            .any(|m| m.base == dll.base && m.flags & loader::BUILTIN != 0);
        if inserted.is_empty() || !mapped {
            continue;
        }
        let Some(lacking) = lacking(bus, process, d, &inserted) else {
            return Err(StateError::Invalid(format!(
                "{}'s image in {} has its functions in an order this rust-dos doesn't \
                 know, and its stubs would call other functions than the program's",
                dll.name, process.name
            )));
        };
        if lacking.is_empty() {
            continue;
        }
        let older = OlderImage {
            dll: d,
            slots: mend_image(bus, process, d, &lacking),
        };
        process.older_images.push(older);
    }
    Ok(())
}

/// Which of the entries `inserted` (places in the table of the DLL `d`)
/// the process's image of the DLL was made without: the set whose taking
/// out puts the stubs the image exports where it has them, the empty set
/// first. None when no set does.
fn lacking(bus: &Bus, process: &Process, d: usize, inserted: &[usize]) -> Option<Vec<usize>> {
    let dll = stubs::DLLS[d];
    (0..1u32 << inserted.len())
        .map(|mask| {
            let mut without: Vec<usize> = (0..inserted.len())
                .filter(|&b| mask >> b & 1 != 0)
                .map(|b| inserted[b])
                .collect();
            without.sort_unstable();
            without
        })
        .find(|without| {
            // After each inserted entry, the first function the image
            // exports that is a stub: where the image has it tells
            // whether the entry was there.
            inserted.iter().all(|&entry| {
                let anchor = (entry + 1..dll.apis.len())
                    .filter(|i| !without.contains(i))
                    .filter(|&i| dll.apis[i].ordinal != 0 && dll.apis[i].conv.is_trap())
                    .find_map(|i| {
                        let at = loader::export_address(
                            bus,
                            &process.memory,
                            dll.base,
                            dll.apis[i].name,
                        )?;
                        Some((i, at))
                    });
                anchor.is_none_or(|(i, at)| {
                    let slot = i - without.iter().filter(|&&w| w < i).count();
                    at == stubs::stub_address(d, slot)
                })
            })
        })
}

/// Mend the process's image of the DLL `d`, made without the entries
/// `lacking`: the place in it of each function of the table as it is now.
fn mend_image(bus: &mut Bus, process: &Process, d: usize, lacking: &[usize]) -> Vec<Option<usize>> {
    let dll = stubs::DLLS[d];
    // The image's order: the older table's, then what it lacks.
    let order: Vec<usize> = (0..dll.apis.len())
        .filter(|i| !lacking.contains(i))
        .chain(lacking.iter().copied())
        .collect();
    let older_len = dll.apis.len() - lacking.len();
    let code_end = code_end(bus, &process.memory, dll.base);
    let mut slots = vec![None; dll.apis.len()];
    for (slot, &api) in order.iter().enumerate() {
        let at = stubs::stub_address(d, slot);
        let Some(bytes) = loader::peek(bus, &process.memory, at, stubs::STUB as usize) else {
            continue;
        };
        let now = stubs::id(d, api).to_le_bytes();
        if slot < older_len {
            // The older build's stub traps with its place, its id then.
            // One mended before traps with `now`, and a function given as
            // code jumps to its code.
            let then = stubs::id(d, slot).to_le_bytes();
            if bytes[..2] == [0xFE, TRAP] && bytes[2..4] == then && then != now {
                process.memory.write(bus, at + 2, &now);
            }
            slots[api] = Some(slot);
            continue;
        }
        // Past the older stubs: the space before the image's next
        // section, if nothing else is there.
        let function = &dll.apis[api];
        if !function.conv.is_trap() {
            continue;
        }
        let stub = stubs::stub(stubs::id(d, api), function.args, function.conv);
        let fits = code_end.is_some_and(|end| at + stubs::STUB <= end);
        if bytes == stub {
            slots[api] = Some(slot);
        } else if fits && bytes.iter().all(|&b| b == 0) {
            process.memory.write(bus, at, &stub);
            slots[api] = Some(slot);
        }
    }
    slots
}

/// Where the first section of the image at `base`, which holds the stubs,
/// ends in memory: its size rounded up to whole pages.
fn code_end(bus: &Bus, memory: &super::memory::Memory, base: u32) -> Option<u32> {
    let header = base.wrapping_add(loader::peek_32(bus, memory, base + 0x3C)?);
    let optional = loader::peek(bus, memory, header + 20, 2)?;
    let section = header + 24 + u16::from_le_bytes([optional[0], optional[1]]) as u32;
    let size = loader::peek_32(bus, memory, section + 8)?;
    let rva = loader::peek_32(bus, memory, section + 12)?;
    Some(base + rva + size.next_multiple_of(0x1000))
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
        dlls.chain(modules).collect()
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
            if line.starts_with("DLL ") || line.starts_with("MODULE ") {
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
             # states keep the stubs of: src/win32/stub_tables.rs. A new function\n\
             # or table goes at the end of its list, never in the middle.\n",
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

    /// Why the tables `now` aren't `recorded` with entries added at the
    /// ends of tables and new tables at the end: the first entry or
    /// table that moved. None when they are.
    fn moved(recorded: &[Table], now: &[Table]) -> Option<String> {
        for (i, (table, names)) in recorded.iter().enumerate() {
            let Some((table_now, names_now)) = now.get(i) else {
                return Some(format!("{table} is gone, and the tables after it moved"));
            };
            if table_now != table {
                return Some(format!(
                    "{table_now} is where {table} was: a table goes at the end of the list, \
                     and every table after one put in moves to another index"
                ));
            }
            for (j, name) in names.iter().enumerate() {
                match names_now.get(j) {
                    Some(n) if n == name => {}
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
                            "{table}'s entry {j}, {name}, is gone: a saved state's stub of it \
                             would call nothing. Keep the entry"
                        ));
                    }
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
        // An entry added at the end of a table, and a table at the end of
        // the list, aren't moves.
        let mut grown = now.clone();
        grown[4].1.push("Later".into());
        grown.push(("DLL LATER.DLL".into(), vec!["First".into()]));
        assert_eq!(moved(&now, &grown), None);
        // One taken out is.
        let mut shrunk = now.clone();
        shrunk[4].1.pop();
        assert!(moved(&now, &shrunk).unwrap().contains("is gone"));
    }

    #[test]
    fn the_file_reads_back_as_written() {
        let now = tables_now();
        assert_eq!(read(&write(&now)), now);
    }

    /// Every entry of `INSERTED` is in its DLL's table, and the DLL has no
    /// vtables, variables or functions given as code after its stubs,
    /// which an older image would have elsewhere: `mend` mends the stubs
    /// only.
    #[test]
    fn inserted_entries_are_in_dlls_mend_can_mend() {
        for e in INSERTED {
            let dll = stubs::DLLS[stubs::find_dll(e.dll).unwrap()];
            assert!(
                dll.apis.iter().any(|a| a.name == e.name),
                "{} {}",
                e.dll,
                e.name
            );
            assert!(dll.vtables.is_empty() && dll.vars.is_empty(), "{}", e.dll);
            assert!(dll.apis.iter().all(|a| a.conv.is_trap()), "{}", e.dll);
        }
    }
}
