//! The debugger and the built-in Windows's page file: reads, writes,
//! breakpoints and watchpoints on pages of a program's own memory that
//! are in the page file, or go out to it and come back.

use super::*;

const WIN32_BASE: u32 = 0x40_0000;

/// T.EXE's data: the address of the block it committed, the stage it
/// waits in, the stage the debugger lets it go on from, and the
/// protection VirtualProtect replaced.
const BLOCK: u32 = 0;
const READY: u32 = 4;
const GO: u32 = 8;
const OLD_PROTECT: u32 = 12;

/// T.EXE, a Win32 program, and the address of its data. It commits
/// `pages` pages with VirtualAlloc, which start in the page file as pages
/// of zeros, and waits in stage 1 (`READY` is 1 until `GO` is).
///
/// Without `sweep` it has touched the second page by then, made the
/// third read-only, untouched, and touched the fourth and made it
/// PAGE_NOACCESS. With `sweep` it has written 1234h to the first page,
/// and after stage 1 it touches every page from the second on, which
/// sends pages touched before out to the page file on a machine with
/// less memory than the block, and waits in stage 2.
///
/// It then writes 5678h to the block's first double word, calls the code
/// at 100h in the block and ends with what that returns in AL.
fn paging_program(pages: u32, sweep: bool) -> (Vec<u8>, u32) {
    use iced_x86::code_asm::*;
    use rust_dos_pe::{self as pe, Builder, Imp};
    const PAGE_NOACCESS: i32 = 1;
    const PAGE_READONLY: i32 = 2;
    const PAGE_READWRITE: i32 = 4;
    const MEM_COMMIT_RESERVE: i32 = 0x3000;
    let mut b = Builder::exe(WIN32_BASE);
    b.subsystem(pe::SUBSYSTEM_CONSOLE);
    let names = [Imp::Name("VirtualAlloc"), Imp::Name("VirtualProtect")];
    let iat = b.imports(&[("KERNEL32.DLL", &names[..])]).concat();
    let flags = pe::SCN_INITIALIZED_DATA | pe::SCN_READ | pe::SCN_WRITE;
    let data = WIN32_BASE + b.section(".data", &[0; 16], flags);
    let at = |offset: u32| dword_ptr((data + offset) as u64);
    let code_rva = b.next_rva();
    let mut a = CodeAssembler::new(32).unwrap();
    let mut code = || -> Result<(), IcedError> {
        for arg in [
            PAGE_READWRITE,
            MEM_COMMIT_RESERVE,
            (pages * 0x1000) as i32,
            0,
        ] {
            a.push(arg)?;
        }
        a.call(dword_ptr(iat[0] as u64))?;
        a.mov(at(BLOCK), eax)?;
        if sweep {
            a.mov(dword_ptr(eax), 0x1234)?;
        } else {
            a.mov(byte_ptr(eax + 0x1000), 0x11)?;
            a.mov(byte_ptr(eax + 0x3000), 0x33)?;
            for (page, protect) in [(0x2000, PAGE_READONLY), (0x3000, PAGE_NOACCESS)] {
                a.push((data + OLD_PROTECT) as i32)?;
                a.push(protect)?;
                a.push(0x1000)?;
                a.mov(eax, at(BLOCK))?;
                a.add(eax, page)?;
                a.push(eax)?;
                a.call(dword_ptr(iat[1] as u64))?;
            }
        }
        let stages = if sweep { 2 } else { 1 };
        for stage in 1..=stages {
            if stage == 2 {
                a.mov(ecx, at(BLOCK))?;
                a.mov(edx, pages as i32 - 1)?;
                let mut next = a.create_label();
                a.set_label(&mut next)?;
                a.add(ecx, 0x1000)?;
                a.mov(byte_ptr(ecx + 0x10), 1)?;
                a.dec(edx)?;
                a.jnz(next)?;
            }
            a.mov(at(READY), stage)?;
            let mut wait = a.create_label();
            a.set_label(&mut wait)?;
            a.cmp(at(GO), stage)?;
            a.jne(wait)?;
        }
        a.mov(eax, at(BLOCK))?;
        a.mov(dword_ptr(eax), 0x5678)?;
        a.add(eax, 0x100)?;
        a.call(eax)?;
        a.ret()
    };
    code().unwrap();
    let code = a.assemble((WIN32_BASE + code_rva) as u64).unwrap();
    b.section(
        ".text",
        &code,
        pe::SCN_CODE | pe::SCN_EXECUTE | pe::SCN_READ,
    );
    b.entry(code_rva);
    (b.build(), data)
}

/// MOV EAX,`value`; RET: the code the debugger puts at 100h in the block.
fn returns(value: u8) -> [u8; 6] {
    [0xB8, value, 0, 0, 0, 0xC3]
}

/// A machine with `memory_mb` MB at the shell's prompt, with the
/// built-in Windows on and `paging_program` as T.EXE in its folder; the
/// address of the program's data.
fn paging_shell(name: &str, memory_mb: usize, pages: u32, sweep: bool) -> (Cpu, u32) {
    let dir = crate::test_dir::path(&format!("debug-{name}"));
    std::fs::create_dir_all(&dir).unwrap();
    let (program, data) = paging_program(pages, sweep);
    std::fs::write(dir.join("T.EXE"), program).unwrap();
    let mut cpu = Cpu::with_memory(dir, memory_mb);
    cpu.bus.win32.enabled = true;
    cpu.host_console = Some(Default::default());
    cpu.load_shell();
    (cpu, data)
}

/// A machine with `memory_mb` MB running `paging_program` under the
/// built-in Windows, with a hub, at the program's stage 1; the address
/// of the program's data and of its block.
fn paging_machine(
    name: &str,
    memory_mb: usize,
    pages: u32,
    sweep: bool,
) -> (Cpu, DebugHub, u32, u32) {
    let (mut cpu, data) = paging_shell(name, memory_mb, pages, sweep);
    assert!(cpu.load_executable("T.EXE", None));
    let mut hub = DebugHub::for_tests(&cpu);
    until_stage(&mut cpu, &mut hub, data, 1);
    let block = u32::from_le_bytes(
        memory(&mut cpu, &mut hub, data + BLOCK, 4)
            .try_into()
            .unwrap(),
    );
    (cpu, hub, data, block)
}

/// Run until the program waits in `stage`, in its own code.
fn until_stage(cpu: &mut Cpu, hub: &mut DebugHub, data: u32, stage: u8) {
    for _ in 0..20_000 {
        let in_program = rust_dos::win32::has_page_file(cpu)
            && (WIN32_BASE..WIN32_BASE + 0x1_0000).contains(&cpu.eip());
        if in_program && crate::debug::pm::peek(cpu, data + READY) == Some(stage) {
            return;
        }
        assert!(
            !hub.paused,
            "stopped before stage {}: {:?}",
            stage,
            entries_of(hub, "paused").pop()
        );
        run(cpu, hub, 1);
    }
    panic!("the program never waited in stage {}", stage);
}

/// Let the program go on from `stage`.
fn go(cpu: &mut Cpu, hub: &mut DebugHub, data: u32, stage: u8) {
    reply(
        cpu,
        hub,
        write(&format!("lin:{:X}", data + GO), &[stage], None),
    )
    .unwrap();
}

/// What `GET /api/memory` reads at the linear address `lin`.
fn memory(cpu: &mut Cpu, hub: &mut DebugHub, lin: u32, len: usize) -> Vec<u8> {
    match handle(
        cpu,
        hub,
        Cmd::ReadMem {
            addr: format!("lin:{:X}", lin),
            len,
        },
    ) {
        Reply::Bytes { data, .. } => data,
        _ => panic!("no bytes"),
    }
}

fn write_lin(
    cpu: &mut Cpu,
    hub: &mut DebugHub,
    lin: u32,
    data: &[u8],
) -> Result<Value, (u16, String)> {
    reply(cpu, hub, write(&format!("lin:{:X}", lin), data, None))
}

fn pagewalk(cpu: &mut Cpu, hub: &mut DebugHub, lin: u32) -> Value {
    reply(cpu, hub, Cmd::PageWalk(format!("lin:{:X}", lin))).unwrap()
}

/// `/api/win32`'s `page_file`: its size, what it holds and the counts of
/// the pages that came in and went out.
fn page_file(cpu: &mut Cpu, hub: &mut DebugHub) -> Value {
    reply(cpu, hub, Cmd::Win32).unwrap()["page_file"].clone()
}

/// Run until the machine stops, and the stop.
fn next_stop(cpu: &mut Cpu, hub: &mut DebugHub) -> Value {
    for _ in 0..2000 {
        if hub.paused {
            return entries_of(hub, "paused").pop().unwrap();
        }
        run(cpu, hub, 1);
    }
    panic!("the machine didn't stop");
}

fn resume(cpu: &mut Cpu, hub: &mut DebugHub) {
    ask(
        cpu,
        hub,
        Cmd::Resume {
            until: None,
            until_ms: None,
        },
        0,
    )
    .unwrap();
}

/// Run until the program has ended; its exit code.
fn exit_code(cpu: &mut Cpu, hub: &mut DebugHub) -> u8 {
    for _ in 0..2000 {
        if !cpu.bus.win32.active() {
            return cpu.errorlevel;
        }
        assert!(
            !hub.paused,
            "stopped: {:?}",
            entries_of(hub, "paused").pop()
        );
        run(cpu, hub, 1);
    }
    panic!("the program didn't end");
}

/// A page a program committed and hasn't touched is a page of zeros in
/// the page file. The debugger's write goes to the page file, where its
/// read, the disassembler and a breakpoint find it, and no page comes in
/// or goes out for any of them: the page file's counts stay as they
/// were. The program then runs the code the debugger wrote, which its
/// first touch of the page brings in, and stops at the breakpoint.
#[test]
fn a_write_to_an_untouched_page_stays_in_the_page_file_until_the_program_touches_it() {
    let (mut cpu, mut hub, data, block) = paging_machine("paged-write", 32, 4, false);
    let at = block + 0x100;
    let before = page_file(&mut cpu, &mut hub);
    let walk = pagewalk(&mut cpu, &mut hub, at);
    assert_eq!(
        (&walk["page_file"], &walk["physical"], &walk["pte"]["slot"]),
        (&json!(true), &Value::Null, &json!(0))
    );
    assert_eq!(memory(&mut cpu, &mut hub, at, 6), [0; 6]);

    let (state_seq, ..) = journal_state(&hub);
    let done = write_lin(&mut cpu, &mut hub, at, &returns(0x55)).unwrap();
    assert_eq!(
        (done["old"].as_str(), done["new"].as_str()),
        (Some("00 00 00 00 00 00"), Some("B8 55 00 00 00 C3"))
    );
    assert_eq!(
        (&done["page_file"], &done["addr"]),
        (&json!(true), &Value::Null),
        "{}",
        done
    );
    assert_eq!(done["linear"], format!("{:08X}", at));
    // Journalled as any write is, and the state moved on.
    let entry = entries_of(&hub, "memory_written").pop().unwrap();
    assert_eq!(
        (&entry["page_file"], &entry["linear"], &entry["len"]),
        (&json!(true), &done["linear"], &json!(6))
    );
    assert!(journal_state(&hub).0 > state_seq);

    // Every path reads what was written, and the page is still there.
    assert_eq!(memory(&mut cpu, &mut hub, at, 6), returns(0x55));
    let rows = reply(
        &mut cpu,
        &mut hub,
        Cmd::Disasm {
            addr: Some(format!("lin:{:X}", at)),
            count: 2,
            bits: None,
        },
    )
    .unwrap();
    assert_eq!(rows["rows"][0]["bytes"], "B8 55 00 00 00");
    assert_eq!(rows["rows"][0]["phys"], Value::Null);
    // The second row's own linear address, which the debugger's page
    // sets a breakpoint by.
    assert_eq!(rows["rows"][1]["lin"], json!(format!("{:08X}", at + 5)));
    assert!(
        rows["rows"][1]["asm"].as_str().unwrap().contains("ret"),
        "{}",
        rows
    );
    let walk = pagewalk(&mut cpu, &mut hub, at);
    assert_eq!(walk["page_file"], true);
    assert_ne!(
        walk["pte"]["slot"], 0,
        "its contents have a place in the page file now"
    );
    let ranges = reply(&mut cpu, &mut hub, Cmd::Win32PageFile { limit: 1000 }).unwrap();
    let has_block = |r: &Value| {
        let start = u32::from_str_radix(r["addr"].as_str().unwrap(), 16).unwrap();
        let pages = r["pages"].as_u64().unwrap() as u32;
        (start..start + pages * 0x1000).contains(&block) && r["zeros"] == false
    };
    assert!(
        ranges["ranges"].as_array().unwrap().iter().any(has_block),
        "{}",
        ranges
    );

    // A breakpoint there is kept by its linear address.
    let addr = format!("lin:{:08X}", at);
    let listed = reply(
        &mut cpu,
        &mut hub,
        Cmd::AddBreakpoint {
            addr: addr.clone(),
            once: false,
        },
    )
    .unwrap();
    assert_eq!(listed["breakpoints"], json!([addr]));
    let rows = reply(
        &mut cpu,
        &mut hub,
        Cmd::Disasm {
            addr: Some(addr.clone()),
            count: 1,
            bits: None,
        },
    )
    .unwrap();
    assert_eq!(rows["rows"][0]["breakpoint"], true);
    assert_eq!(
        page_file(&mut cpu, &mut hub),
        before,
        "no page came in or went out"
    );

    go(&mut cpu, &mut hub, data, 1);
    let stop = next_stop(&mut cpu, &mut hub);
    assert_eq!(
        (stop["reason"].as_str(), stop["breakpoint"].as_str()),
        (Some("breakpoint"), Some(addr.as_str())),
        "{}",
        stop
    );
    assert_eq!(cpu.eip(), at);
    // The program's touch brought the page in, with the debugger's bytes
    // and the program's own.
    let walk = pagewalk(&mut cpu, &mut hub, at);
    assert_eq!(walk["page_file"], false);
    assert!(walk["physical"].is_string(), "{}", walk);
    assert_eq!(
        page_file(&mut cpu, &mut hub)["ins"].as_u64(),
        before["ins"].as_u64().map(|n| n + 1)
    );
    assert_eq!(memory(&mut cpu, &mut hub, at, 6), returns(0x55));
    assert_eq!(memory(&mut cpu, &mut hub, block, 2), [0x78, 0x56]);
    // Found and removed by the same address, with the page in memory.
    let listed = reply(&mut cpu, &mut hub, Cmd::RemoveBreakpoint(Some(addr))).unwrap();
    assert_eq!(listed["breakpoints"], json!([]));
    resume(&mut cpu, &mut hub);
    assert_eq!(exit_code(&mut cpu, &mut hub), 0x55);
}

/// A breakpoint and a watchpoint kept by linear address in another address
/// space (another process's, or one that has ended) are removed by the
/// name the lists give them, which doesn't say which address space it is.
/// One of the running address space is removed first.
#[test]
fn a_spot_of_another_address_space_is_removed_by_its_listed_name() {
    use super::super::{Watch, pm::Spot};
    let (mut cpu, mut hub) = (Cpu::new(".".into()), DebugHub::disabled());
    let (lin, other) = (0x0003_0100, cpu.cr3 ^ 0x1000);
    hub.breakpoints.insert(Spot::Lin { cr3: other, lin });
    hub.breakpoints.insert(Spot::Phys(lin as usize));
    hub.watchpoints.push(Watch {
        spot: Spot::Lin { cr3: other, lin },
        len: 2,
        value: 0,
    });
    let name = format!("lin:{:08X}", lin);
    let listed = reply(&mut cpu, &mut hub, Cmd::ListBreakpoints).unwrap();
    assert_eq!(listed["breakpoints"], json!(["30100", name]));
    // With paging off, the address's physical one is the running one.
    let listed = reply(
        &mut cpu,
        &mut hub,
        Cmd::RemoveBreakpoint(Some(name.clone())),
    )
    .unwrap();
    assert_eq!(listed["breakpoints"], json!([name]));
    let listed = reply(
        &mut cpu,
        &mut hub,
        Cmd::RemoveBreakpoint(Some(name.clone())),
    )
    .unwrap();
    assert_eq!(listed["breakpoints"], json!([]));
    let gone = reply(
        &mut cpu,
        &mut hub,
        Cmd::RemoveBreakpoint(Some(name.clone())),
    );
    assert_eq!(gone.unwrap_err().0, 404);
    let listed = reply(&mut cpu, &mut hub, Cmd::RemoveWatchpoint(Some(name))).unwrap();
    assert_eq!(listed["watchpoints"], json!([]));
}

/// A write is made whole or not at all: across a page in memory and one
/// in the page file both parts are written, and one that runs into a
/// page that isn't committed, or expects other bytes, writes nothing.
/// The debugger writes pages the program may not (read-only in the page
/// file, PAGE_NOACCESS in memory), which keep their protection and their
/// place.
#[test]
fn a_write_across_pages_in_memory_and_in_the_page_file_is_whole_or_not_made() {
    let (mut cpu, mut hub, _, block) = paging_machine("paged-across", 32, 4, false);
    let before = page_file(&mut cpu, &mut hub);
    let walks = |cpu: &mut Cpu, hub: &mut DebugHub| -> Vec<Value> {
        (0..5)
            .map(|n| pagewalk(cpu, hub, block + n * 0x1000))
            .collect()
    };
    let places = walks(&mut cpu, &mut hub);
    let in_file: Vec<bool> = places.iter().map(|w| w["page_file"] == true).collect();
    let present: Vec<bool> = places.iter().map(|w| w["physical"].is_string()).collect();
    assert_eq!(in_file, [true, false, true, false, false]);
    assert_eq!(
        present,
        [false, true, false, false, false],
        "the fourth is PAGE_NOACCESS, the fifth not committed"
    );

    // The second page is in memory, the third read-only in the page file.
    let at = block + 0x1FFE;
    let done = write_lin(&mut cpu, &mut hub, at, &[1, 2, 3, 4]).unwrap();
    assert_eq!(
        (done["old"].as_str(), done["new"].as_str()),
        (Some("00 00 00 00"), Some("01 02 03 04"))
    );
    assert!(
        done["addr"].is_string() && done["page_file"] == true,
        "{}",
        done
    );
    assert_eq!(memory(&mut cpu, &mut hub, at, 4), [1, 2, 3, 4]);
    let phys = usize::from_str_radix(done["addr"].as_str().unwrap(), 16).unwrap();
    assert_eq!(
        (cpu.bus.peek_8(phys), cpu.bus.peek_8(phys + 1)),
        (1, 2),
        "the part in memory"
    );

    // The fourth page is in memory with PAGE_NOACCESS: read and written
    // where it is.
    let at = block + 0x2FFE;
    assert_eq!(memory(&mut cpu, &mut hub, at, 4), [0, 0, 0x33, 0]);
    write_lin(&mut cpu, &mut hub, at, &[5, 6, 7, 8]).unwrap();
    assert_eq!(memory(&mut cpu, &mut hub, at, 4), [5, 6, 7, 8]);

    // The fifth page isn't committed: nothing of a write that reaches it
    // is made.
    let at = block + 0x3FFE;
    let refused = write_lin(&mut cpu, &mut hub, at, &[9, 9, 9, 9]).unwrap_err();
    assert_eq!(refused.0, 400);
    assert!(refused.1.contains("unmapped page"), "{}", refused.1);
    assert_eq!(memory(&mut cpu, &mut hub, at, 4), [0, 0, 0xFF, 0xFF]);
    // Nor of one that expects other bytes than the page file has.
    let guarded = write(
        &format!("lin:{:X}", block + 0x1FFE),
        &[9; 4],
        Some(&[1, 2, 3, 5]),
    );
    let refused = reply(&mut cpu, &mut hub, guarded).unwrap_err();
    assert_eq!(refused.0, 409);
    assert!(
        refused.1.contains(r#""found":"01 02 03 04""#),
        "{}",
        refused.1
    );
    assert_eq!(memory(&mut cpu, &mut hub, block + 0x1FFE, 4), [1, 2, 3, 4]);

    // Every page is where it was, with the protection it had: only the
    // third's entry changed, to its contents' place in the page file.
    let after = walks(&mut cpu, &mut hub);
    for (n, (was, is)) in places.iter().zip(&after).enumerate() {
        let entry =
            |w: &Value| u32::from_str_radix(w["pte"]["value"].as_str().unwrap(), 16).unwrap();
        let mask = if n == 2 { 0xFFF } else { !0 };
        assert_eq!(entry(was) & mask, entry(is) & mask, "page {n}");
        assert_eq!(
            (&was["page_file"], &was["physical"]),
            (&is["page_file"], &is["physical"]),
            "page {n}"
        );
    }
    assert_eq!(
        page_file(&mut cpu, &mut hub),
        before,
        "no page came in or went out"
    );
}

/// A breakpoint and a watchpoint on a page of a program's own memory
/// follow the page: set while it is in memory, they are still on it
/// after it went out to the page file and came back at another physical
/// address. While it is out, the debugger reads and writes it in the
/// page file, the watchpoint keeps its value, and the physical page it
/// had, which another page of the program's has by then, stops nothing.
#[test]
fn a_breakpoint_and_a_watchpoint_follow_a_page_out_to_the_page_file_and_back() {
    // 24 MB committed on a machine with 16 MB.
    let (mut cpu, mut hub, data, block) = paging_machine("paged-follow", 16, 0x1800, true);
    let at = block + 0x100;
    let walk = pagewalk(&mut cpu, &mut hub, at);
    let first = walk["physical"]
        .as_str()
        .expect("the program touched the page")
        .to_string();
    write_lin(&mut cpu, &mut hub, at, &returns(0x55)).unwrap();
    let addr = format!("lin:{:08X}", at);
    let listed = reply(
        &mut cpu,
        &mut hub,
        Cmd::AddBreakpoint {
            addr: addr.clone(),
            once: false,
        },
    )
    .unwrap();
    assert_eq!(
        listed["breakpoints"],
        json!([addr]),
        "kept by its linear address"
    );
    let watch = format!("lin:{:08X}", block);
    let listed = reply(
        &mut cpu,
        &mut hub,
        Cmd::AddWatchpoint {
            addr: watch.clone(),
            len: 4,
        },
    )
    .unwrap();
    assert_eq!(
        listed["watchpoints"],
        json!([{"addr": watch, "len": 4, "value": "00001234"}])
    );

    // The program touches the other pages, which sends this one out.
    // Neither stops the machine on the way.
    go(&mut cpu, &mut hub, data, 1);
    until_stage(&mut cpu, &mut hub, data, 2);
    let before = page_file(&mut cpu, &mut hub);
    assert!(before["outs"].as_u64() > Some(0x800), "{}", before);
    let walk = pagewalk(&mut cpu, &mut hub, at);
    assert_eq!(
        (&walk["page_file"], &walk["physical"]),
        (&json!(true), &Value::Null),
        "the page went out"
    );
    assert_ne!(walk["pte"]["slot"], 0);

    // Read and written in the page file.
    assert_eq!(memory(&mut cpu, &mut hub, at, 6), returns(0x55));
    assert_eq!(memory(&mut cpu, &mut hub, block, 4), [0x34, 0x12, 0, 0]);
    let done = write_lin(&mut cpu, &mut hub, at + 1, &[0x66]).unwrap();
    assert_eq!(
        (
            done["old"].as_str(),
            done["new"].as_str(),
            &done["page_file"]
        ),
        (Some("55"), Some("66"), &json!(true))
    );
    assert_eq!(memory(&mut cpu, &mut hub, at, 6), returns(0x66));
    let listed = reply(&mut cpu, &mut hub, Cmd::ListWatchpoints).unwrap();
    assert_eq!(listed["watchpoints"][0]["value"], "00001234");
    let rows = reply(
        &mut cpu,
        &mut hub,
        Cmd::Disasm {
            addr: Some(addr.clone()),
            count: 1,
            bits: None,
        },
    )
    .unwrap();
    assert_eq!(
        (
            &rows["rows"][0]["breakpoint"],
            rows["rows"][0]["bytes"].as_str()
        ),
        (&json!(true), Some("B8 66 00 00 00"))
    );
    assert_eq!(
        page_file(&mut cpu, &mut hub),
        before,
        "no page came in or went out"
    );

    // The program's write brings the page in and changes the watched
    // bytes.
    go(&mut cpu, &mut hub, data, 2);
    let stop = next_stop(&mut cpu, &mut hub);
    assert_eq!(stop["reason"].as_str(), Some("watchpoint"), "{}", stop);
    assert_eq!(
        stop["watch"],
        json!({"addr": watch, "len": 4, "old": "00001234", "new": "00005678"})
    );
    let walk = pagewalk(&mut cpu, &mut hub, at);
    let second = walk["physical"]
        .as_str()
        .expect("the page came in")
        .to_string();
    assert_ne!(first, second, "at another physical address");
    // Then it runs the code, at the breakpoint.
    resume(&mut cpu, &mut hub);
    let stop = next_stop(&mut cpu, &mut hub);
    assert_eq!(
        (stop["reason"].as_str(), stop["breakpoint"].as_str()),
        (Some("breakpoint"), Some(addr.as_str())),
        "{}",
        stop
    );
    assert_eq!(cpu.eip(), at);
    reply(&mut cpu, &mut hub, Cmd::RemoveWatchpoint(Some(watch))).unwrap();
    reply(&mut cpu, &mut hub, Cmd::RemoveBreakpoint(Some(addr))).unwrap();
    resume(&mut cpu, &mut hub);
    assert_eq!(exit_code(&mut cpu, &mut hub), 0x66);
}

/// The address of the block the program committed.
fn block_of(cpu: &mut Cpu, hub: &mut DebugHub, data: u32) -> u32 {
    u32::from_le_bytes(memory(cpu, hub, data + BLOCK, 4).try_into().unwrap())
}

/// A breakpoint and a watchpoint kept by linear address go when their
/// process ends, each with a journal entry. The same program started
/// again gets the same page tables and its block at the same address,
/// where the old breakpoint would stop it and the old watchpoint would
/// see its write: it runs to its end without a stop.
#[test]
fn linear_breakpoints_and_watchpoints_go_when_their_process_ends() {
    let (mut cpu, mut hub, data, block) = paging_machine("lin-ended", 32, 4, false);
    let cr3 = cpu.cr3;
    let at = block + 0x100;
    write_lin(&mut cpu, &mut hub, at, &returns(0x55)).unwrap();
    let addr = format!("lin:{:08X}", at);
    let watch = format!("lin:{:08X}", block);
    let set = Cmd::AddBreakpoint {
        addr: addr.clone(),
        once: false,
    };
    reply(&mut cpu, &mut hub, set).unwrap();
    let set = Cmd::AddWatchpoint {
        addr: watch.clone(),
        len: 4,
    };
    reply(&mut cpu, &mut hub, set).unwrap();
    // Both stop this process.
    go(&mut cpu, &mut hub, data, 1);
    let stop = next_stop(&mut cpu, &mut hub);
    assert_eq!(stop["reason"].as_str(), Some("watchpoint"), "{}", stop);
    resume(&mut cpu, &mut hub);
    let stop = next_stop(&mut cpu, &mut hub);
    assert_eq!(stop["breakpoint"].as_str(), Some(addr.as_str()), "{}", stop);
    resume(&mut cpu, &mut hub);
    assert_eq!(exit_code(&mut cpu, &mut hub), 0x55);

    // They went with it.
    let listed = reply(&mut cpu, &mut hub, Cmd::ListBreakpoints).unwrap();
    assert_eq!(listed["breakpoints"], json!([]));
    let listed = reply(&mut cpu, &mut hub, Cmd::ListWatchpoints).unwrap();
    assert_eq!(listed["watchpoints"], json!([]));
    let removed = entries_of(&hub, "breakpoint_removed").pop().unwrap();
    assert_eq!(
        (&removed["addr"], &removed["process_ended"]),
        (&json!(addr), &json!(true))
    );
    let removed = entries_of(&hub, "watchpoint_removed").pop().unwrap();
    assert_eq!(
        (&removed["addr"], &removed["process_ended"]),
        (&json!(watch), &json!(true))
    );

    // The program again, in the same page tables at the same addresses.
    assert!(cpu.load_executable("T.EXE", None));
    until_stage(&mut cpu, &mut hub, data, 1);
    let again = block_of(&mut cpu, &mut hub, data);
    assert_eq!((cpu.cr3, again), (cr3, block), "where the old spots were");
    write_lin(&mut cpu, &mut hub, at, &returns(0x66)).unwrap();
    go(&mut cpu, &mut hub, data, 1);
    assert_eq!(exit_code(&mut cpu, &mut hub), 0x66);
}

/// A watchpoint kept by linear address reads its process's memory
/// through that process's page tables while the processor has others,
/// as the system's write into a process that doesn't run (a read that
/// finishes later) changes it; and once the process has ended it is
/// dropped before it can read another's.
#[test]
fn a_linear_watchpoint_reads_its_own_address_space_whichever_runs() {
    use super::super::pm::{self, Spot};
    let (mut cpu, mut hub, data, block) = paging_machine("lin-other", 32, 4, false);
    let at = block + 0x1000;
    let spot = Spot::of_lin(&cpu, at).expect("committed");
    assert!(matches!(spot, Spot::Lin { .. }), "{:?}", spot);
    assert_eq!(spot.value(&cpu, 1), Some(0x11));
    let Some(pm::Place::Phys(phys)) = pm::place(&cpu, at) else {
        panic!("the second page is in memory");
    };
    // Another address space runs: the processor's tables don't map the
    // block (the first megabyte's, which are the identity), and the
    // system writes the process's page.
    let own = cpu.cr3;
    cpu.cr0 &= !crate::cpu::CR0_PG;
    cpu.cr3 = 0;
    cpu.bus.write_8(phys, 0x22);
    assert_eq!(spot.value(&cpu, 1), Some(0x22));
    assert!(!spot.ended(&cpu));
    cpu.cr0 |= crate::cpu::CR0_PG;
    cpu.cr3 = own;
    // A page in the page file reads from there.
    let zeros = Spot::of_lin(&cpu, block + 0x2000).expect("committed");
    assert_eq!(zeros.value(&cpu, 4), Some(0));
    // An address space no process has is an ended one.
    let gone = Spot::Lin {
        cr3: own ^ 0x1000,
        lin: at,
    };
    assert!(gone.ended(&cpu));
    assert_eq!(gone.value(&cpu, 1), None);
    // The program ends: the spot's address space is no process's then.
    write_lin(&mut cpu, &mut hub, block + 0x100, &returns(1)).unwrap();
    go(&mut cpu, &mut hub, data, 1);
    assert_eq!(exit_code(&mut cpu, &mut hub), 1);
    assert!(spot.ended(&cpu));
}

/// The stages P.EXE waits in: before it starts its child, and once the
/// child has ended.
const STARTING: u8 = 8;
const CHILD_ENDED: u8 = 9;

/// P.EXE, a Win32 program that starts T.EXE and lives as long as it
/// does, and the address of its data, which has `BLOCK`, `READY` and `GO`
/// where T.EXE's has them. It commits four pages as T.EXE does, puts
/// MOV EAX,77h; RET at 100h in them, waits in stage `STARTING`, starts
/// T.EXE and then, until T.EXE has ended, counts in the block's first
/// double word, calls the code at 100h and sleeps a millisecond. It
/// waits in stage `CHILD_ENDED` then, calls the code once more and ends
/// with what that returns.
fn launcher_program() -> (Vec<u8>, u32) {
    use iced_x86::code_asm::*;
    use rust_dos_pe::{self as pe, Builder, Imp};
    const LINE: u32 = 0x20;
    const STARTUP_INFO: u32 = 0x40;
    const PROCESS_INFORMATION: u32 = 0x90;
    let mut b = Builder::exe(WIN32_BASE);
    b.subsystem(pe::SUBSYSTEM_CONSOLE);
    let names = [
        "VirtualAlloc",
        "CreateProcessA",
        "Sleep",
        "WaitForSingleObject",
    ];
    let imports = names.map(Imp::Name);
    let iat = b.imports(&[("KERNEL32.DLL", &imports[..])]).concat();
    let f = |name: &str| dword_ptr(iat[names.iter().position(|n| *n == name).unwrap()] as u64);
    let mut bytes = [0; 0xB0];
    bytes[LINE as usize..][..5].copy_from_slice(b"T.EXE");
    bytes[STARTUP_INFO as usize] = 68;
    let flags = pe::SCN_INITIALIZED_DATA | pe::SCN_READ | pe::SCN_WRITE;
    let data = WIN32_BASE + b.section(".data", &bytes, flags);
    let at = |offset: u32| dword_ptr((data + offset) as u64);
    let code_rva = b.next_rva();
    let mut a = CodeAssembler::new(32).unwrap();
    let mut code = || -> Result<(), IcedError> {
        // PAGE_READWRITE, MEM_COMMIT | MEM_RESERVE, four pages, anywhere.
        for arg in [4, 0x3000, 0x4000, 0] {
            a.push(arg)?;
        }
        a.call(f("VirtualAlloc"))?;
        a.mov(at(BLOCK), eax)?;
        a.mov(dword_ptr(eax + 0x100), 0x77B8)?;
        a.mov(word_ptr(eax + 0x104), 0xC300)?;
        a.mov(at(READY), STARTING as i32)?;
        let mut start = a.create_label();
        a.set_label(&mut start)?;
        a.cmp(at(GO), STARTING as i32)?;
        a.jne(start)?;
        a.push((data + PROCESS_INFORMATION) as i32)?;
        a.push((data + STARTUP_INFO) as i32)?;
        for _ in 0..6 {
            a.push(0)?;
        }
        a.push((data + LINE) as i32)?;
        a.push(0)?;
        a.call(f("CreateProcessA"))?;
        let mut again = a.create_label();
        a.set_label(&mut again)?;
        a.mov(eax, at(BLOCK))?;
        a.inc(dword_ptr(eax))?;
        a.add(eax, 0x100)?;
        a.call(eax)?;
        a.push(1)?;
        a.call(f("Sleep"))?;
        a.push(0)?;
        a.push(at(PROCESS_INFORMATION))?;
        a.call(f("WaitForSingleObject"))?;
        a.test(eax, eax)?;
        a.jnz(again)?;
        a.mov(at(READY), CHILD_ENDED as i32)?;
        let mut wait = a.create_label();
        a.set_label(&mut wait)?;
        a.cmp(at(GO), CHILD_ENDED as i32)?;
        a.jne(wait)?;
        a.mov(eax, at(BLOCK))?;
        a.add(eax, 0x100)?;
        a.call(eax)?;
        a.ret()
    };
    code().unwrap();
    let code = a.assemble((WIN32_BASE + code_rva) as u64).unwrap();
    b.section(
        ".text",
        &code,
        pe::SCN_CODE | pe::SCN_EXECUTE | pe::SCN_READ,
    );
    b.entry(code_rva);
    (b.build(), data)
}

/// Two processes alive at once, with the same linear addresses in both:
/// a launcher and the program it started, each with code at 100h in a
/// block at the same address. A breakpoint and a watchpoint set in the
/// started program's address space don't stop the launcher, which runs
/// the code at the breakpoint's address and writes the watched one over
/// and over. They stop the program, and go with a journal entry when it
/// ends, while the launcher lives on.
#[test]
fn linear_spots_stop_only_their_own_process_of_two() {
    use super::super::pm::Spot;
    let (mut cpu, data) = paging_shell("lin-two", 32, 4, false);
    let (launcher, launcher_data) = launcher_program();
    assert_eq!(launcher_data, data, "one layout for both programs' data");
    let dir = crate::test_dir::path("debug-lin-two");
    std::fs::write(dir.join("P.EXE"), launcher).unwrap();
    assert!(cpu.load_executable("P.EXE", None));
    let mut hub = DebugHub::for_tests(&cpu);
    // The launcher, before it starts the program.
    until_stage(&mut cpu, &mut hub, data, STARTING);
    let launcher = cpu.cr3;
    go(&mut cpu, &mut hub, data, STARTING);
    // The launcher's READY is never 1, so this is the program, T.EXE,
    // in its own address space.
    until_stage(&mut cpu, &mut hub, data, 1);
    let child = cpu.cr3;
    let block = block_of(&mut cpu, &mut hub, data);
    let at = block + 0x100;
    write_lin(&mut cpu, &mut hub, at, &returns(0x55)).unwrap();
    let addr = format!("lin:{:08X}", at);
    let watch = format!("lin:{:08X}", block);
    let set = Cmd::AddBreakpoint {
        addr: addr.clone(),
        once: false,
    };
    reply(&mut cpu, &mut hub, set).unwrap();
    let set = Cmd::AddWatchpoint {
        addr: watch.clone(),
        len: 4,
    };
    let listed = reply(&mut cpu, &mut hub, set).unwrap();
    assert_eq!(listed["watchpoints"][0]["value"], "00000000");

    // The launcher goes round while the program waits, in its own
    // address space: the count in its block grows, at the watched linear
    // address, and each round calls the code at the breakpoint's.
    // Neither stops the machine, and the watchpoint reads the program's
    // block all along.
    assert_ne!(launcher, child);
    let own = |lin: u32| Spot::Lin { cr3: launcher, lin };
    assert_eq!(
        own(data + BLOCK).value(&cpu, 4),
        Some(block),
        "the same linear address in both"
    );
    assert_eq!(own(at).value(&cpu, 2), Some(0x77B8));
    let first = own(block).value(&cpu, 4).unwrap();
    for _ in 0..400 {
        run(&mut cpu, &mut hub, 1);
        assert!(!hub.paused, "{:?}", entries_of(&hub, "paused").pop());
    }
    let last = own(block).value(&cpu, 4).unwrap();
    assert!(
        last >= first + 3,
        "the launcher's rounds: {first} to {last}"
    );
    let listed = reply(&mut cpu, &mut hub, Cmd::ListWatchpoints).unwrap();
    assert_eq!(listed["watchpoints"][0]["value"], "00000000");

    // The program goes on: its write and its call stop it.
    until_stage(&mut cpu, &mut hub, data, 1);
    assert_eq!(cpu.cr3, child);
    go(&mut cpu, &mut hub, data, 1);
    let stop = next_stop(&mut cpu, &mut hub);
    assert_eq!(
        stop["watch"],
        json!({"addr": watch, "len": 4, "old": "00000000", "new": "00005678"}),
        "{}",
        stop
    );
    assert_eq!(cpu.cr3, child);
    resume(&mut cpu, &mut hub);
    let stop = next_stop(&mut cpu, &mut hub);
    assert_eq!(stop["breakpoint"].as_str(), Some(addr.as_str()), "{}", stop);
    assert_eq!((cpu.cr3, cpu.eip()), (child, at));
    resume(&mut cpu, &mut hub);

    // It ends, and the launcher, alive, waits in its last stage. The
    // spots went with the program.
    until_stage(&mut cpu, &mut hub, data, CHILD_ENDED);
    assert_ne!(cpu.cr3, child);
    let listed = reply(&mut cpu, &mut hub, Cmd::ListBreakpoints).unwrap();
    assert_eq!(listed["breakpoints"], json!([]));
    let listed = reply(&mut cpu, &mut hub, Cmd::ListWatchpoints).unwrap();
    assert_eq!(listed["watchpoints"], json!([]));
    let removed = entries_of(&hub, "breakpoint_removed");
    assert_eq!(removed.len(), 1, "{:?}", removed);
    assert_eq!(
        (&removed[0]["addr"], &removed[0]["process_ended"]),
        (&json!(addr), &json!(true))
    );
    let removed = entries_of(&hub, "watchpoint_removed");
    assert_eq!(removed.len(), 1, "{:?}", removed);
    assert_eq!(
        (&removed[0]["addr"], &removed[0]["process_ended"]),
        (&json!(watch), &json!(true))
    );
    go(&mut cpu, &mut hub, data, CHILD_ENDED);
    assert_eq!(exit_code(&mut cpu, &mut hub), 0x77);
}

/// Load the save state `state` as `POST /api/state/load` without
/// `strict_match` does: the front end loads it and tells the hub.
fn load_state(cpu: &mut Cpu, hub: &mut DebugHub, state: &[u8]) -> Value {
    let load = Cmd::LoadState {
        path: "s".into(),
        strict_match: false,
        check_files: false,
        folders: Vec::new(),
    };
    let sent = send(cpu, hub, load);
    let request = hub.next_state_request().expect("a load to carry out");
    rust_dos::savestate::machine::load(cpu, state).unwrap();
    hub.state_done(cpu, request, Ok(json!({"loaded": true})));
    answer(cpu, hub, sent, 0).unwrap()
}

/// A save state's load keeps the breakpoints and watchpoints kept by
/// linear address whose process the loaded machine has, as it keeps
/// physical ones, and they stop it there. It drops those whose address
/// space no process of the loaded machine has, each with the journal
/// entry of a process that ended, before anything lists them: a load
/// through the debug server and one from the window alike.
#[test]
fn a_loaded_state_keeps_the_linear_spots_of_its_processes_and_drops_the_others() {
    use rust_dos::savestate::machine;
    let (mut cpu, data) = paging_shell("lin-load", 32, 4, false);
    let at_prompt = machine::save(&cpu);
    assert!(cpu.load_executable("T.EXE", None));
    let mut hub = DebugHub::for_tests(&cpu);
    until_stage(&mut cpu, &mut hub, data, 1);
    let block = block_of(&mut cpu, &mut hub, data);
    let at = block + 0x100;
    write_lin(&mut cpu, &mut hub, at, &returns(0x55)).unwrap();
    let in_program = machine::save(&cpu);
    let addr = format!("lin:{:08X}", at);
    let watch = format!("lin:{:08X}", block);
    let set = |cpu: &mut Cpu, hub: &mut DebugHub| {
        let set = Cmd::AddBreakpoint {
            addr: addr.clone(),
            once: false,
        };
        reply(cpu, hub, set).unwrap();
        let set = Cmd::AddWatchpoint {
            addr: watch.clone(),
            len: 4,
        };
        reply(cpu, hub, set).unwrap();
    };
    let kept = |cpu: &mut Cpu, hub: &mut DebugHub| {
        let breakpoints = reply(cpu, hub, Cmd::ListBreakpoints).unwrap();
        let watchpoints = reply(cpu, hub, Cmd::ListWatchpoints).unwrap();
        let count = |list: &Value| list.as_array().unwrap().len();
        (
            count(&breakpoints["breakpoints"]),
            count(&watchpoints["watchpoints"]),
        )
    };
    let dropped = |hub: &DebugHub| {
        let of = |kind: &str| {
            let entries = entries_of(hub, kind);
            let ended = entries.iter().filter(|e| e["process_ended"] == true);
            ended.count()
        };
        (of("breakpoint_removed"), of("watchpoint_removed"))
    };
    set(&mut cpu, &mut hub);

    // The state with the program in it, both ways: its process has the
    // address space the spots are in.
    load_state(&mut cpu, &mut hub, &in_program);
    assert_eq!(kept(&mut cpu, &mut hub), (1, 1));
    machine::load(&mut cpu, &in_program).unwrap();
    hub.state_loaded_elsewhere(&cpu, "window");
    assert_eq!(kept(&mut cpu, &mut hub), (1, 1));
    assert_eq!(dropped(&hub), (0, 0));

    // The state from the prompt, before the program: no process.
    load_state(&mut cpu, &mut hub, &at_prompt);
    assert_eq!(kept(&mut cpu, &mut hub), (0, 0), "before the machine ran");
    assert_eq!(dropped(&hub), (1, 1));
    let journal = entries(&hub);
    let place = |kind: &str| journal.iter().rposition(|e| e["type"] == kind).unwrap();
    assert!(place("state_loaded") < place("breakpoint_removed"));
    let removed = entries_of(&hub, "watchpoint_removed").pop().unwrap();
    assert_eq!(removed["addr"], json!(watch));

    // Back in the program, the spots set again stop it.
    machine::load(&mut cpu, &in_program).unwrap();
    hub.state_loaded_elsewhere(&cpu, "window");
    set(&mut cpu, &mut hub);
    go(&mut cpu, &mut hub, data, 1);
    let stop = next_stop(&mut cpu, &mut hub);
    assert_eq!(stop["reason"].as_str(), Some("watchpoint"), "{}", stop);
    resume(&mut cpu, &mut hub);
    let stop = next_stop(&mut cpu, &mut hub);
    assert_eq!(stop["breakpoint"].as_str(), Some(addr.as_str()), "{}", stop);

    // Paused there, the window loads the state from the prompt: the
    // stop it makes lists neither.
    machine::load(&mut cpu, &at_prompt).unwrap();
    hub.state_loaded_elsewhere(&cpu, "window");
    assert_eq!(dropped(&hub), (2, 2));
    let stop = entries_of(&hub, "paused").pop().unwrap();
    assert_eq!(stop["reason"].as_str(), Some("load"), "{}", stop);
    assert_eq!(kept(&mut cpu, &mut hub), (0, 0));
}
