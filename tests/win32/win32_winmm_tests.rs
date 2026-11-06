//! The built-in Windows' WINMM: a periodic timer counted over emulated
//! time, MIDI out reaching the MPU-401's synthesizer, MCI playing a CD
//! image's audio track, and RIFF files read and written with the mmio
//! functions. Each runs on the interpreter and on the dynamic
//! recompiler. The programs are PE files made with rust-dos-pe's
//! builder, their code assembled with iced.

use crate::cdimage;

use iced_x86::code_asm::*;
use rust_dos::cdrom::audio::PlayState;
use rust_dos::cpu::{CoreMode, Cpu, HostConsole};
use rust_dos::disk::MountOptions;
use rust_dos::exec::{NoHook, StopReason, run_batch};
use rust_dos_pe::{self as pe, Builder, Imp};
use std::fs;
use std::path::PathBuf;

const BASE: u32 = 0x40_0000;
const STD_OUTPUT_HANDLE: i32 = -11;
const CORES: [CoreMode; 2] = [CoreMode::Normal, CoreMode::Dynamic];

/// Where a test program's data and imports are, for its code.
struct Layout {
    data: u32,
    iat: Vec<u32>,
}

impl Layout {
    /// The address of byte `offset` of the data.
    fn at(&self, offset: u32) -> u64 {
        (self.data + offset) as u64
    }

    /// The import address table slot of the function `index`.
    fn import(&self, index: usize) -> AsmMemoryOperand {
        dword_ptr(self.iat[index] as u64)
    }
}

/// A program importing `imports`, with `data` and the code `f` makes.
fn program(
    imports: &[(&str, &[&str])],
    data: &[u8],
    f: impl FnOnce(&mut CodeAssembler, &Layout) -> Result<(), IcedError>,
) -> Vec<u8> {
    let mut b = Builder::exe(BASE);
    let lists: Vec<Vec<Imp>> = imports
        .iter()
        .map(|(_, names)| names.iter().map(|n| Imp::Name(n)).collect())
        .collect();
    let dlls: Vec<(&str, &[Imp])> = imports
        .iter()
        .zip(&lists)
        .map(|((dll, _), list)| (*dll, list.as_slice()))
        .collect();
    let iat = b.imports(&dlls).concat();
    let data_rva = b.section(
        ".data",
        data,
        pe::SCN_INITIALIZED_DATA | pe::SCN_READ | pe::SCN_WRITE,
    );
    let layout = Layout {
        data: BASE + data_rva,
        iat,
    };
    let code_rva = b.next_rva();
    let mut a = CodeAssembler::new(32).unwrap();
    f(&mut a, &layout).unwrap();
    let code = a.assemble((BASE + code_rva) as u64).unwrap();
    let flags = pe::SCN_CODE | pe::SCN_EXECUTE | pe::SCN_READ;
    assert_eq!(b.section(".text", &code, flags), code_rva);
    b.entry(code_rva);
    b.build()
}

/// The folder of a test's drive C:.
fn scratch_path(name: &str) -> PathBuf {
    PathBuf::from("target/test_win32_winmm").join(name)
}

/// A fresh folder for a test's drive C:, with `files` in it.
fn scratch(name: &str, files: &[(&str, Vec<u8>)]) -> PathBuf {
    let base = scratch_path(name);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    for (file, bytes) in files {
        fs::write(base.join(file), bytes).unwrap();
    }
    base
}

/// A machine on `core` with the built-in Windows on, `program` loaded
/// from the shell as T.EXE, and what it prints kept. `prepare` runs
/// before the program is loaded.
fn machine(
    test: &str,
    core: CoreMode,
    files: &[(&str, Vec<u8>)],
    prepare: impl FnOnce(&mut Cpu),
) -> Cpu {
    let mut cpu = Cpu::new(scratch(test, files));
    cpu.core = core;
    cpu.bus.win32.enabled = true;
    cpu.host_console = Some(HostConsole::default());
    prepare(&mut cpu);
    cpu.load_shell();
    assert!(cpu.load_executable("T.EXE", None));
    cpu
}

/// Run one batch; whether the program ended and the shell is back.
fn batch(cpu: &mut Cpu) -> bool {
    cpu.bus.start_batch(cpu.bus.clock.icount + 100_000);
    run_batch(cpu, &mut NoHook, false) == StopReason::ShellReloaded
}

/// Run the machine until the program ends, for at most `batches`
/// batches, calling `watch` after each.
fn run_to_exit(cpu: &mut Cpu, batches: usize, mut watch: impl FnMut(&Cpu)) {
    for _ in 0..batches {
        let ended = batch(cpu);
        watch(cpu);
        if ended {
            return;
        }
    }
    panic!("the program didn't end: {:?}", printed(cpu));
}

/// What the program printed, to standard output and error.
fn printed(cpu: &Cpu) -> String {
    let console = cpu.host_console.as_ref().unwrap();
    let mut text = console.stdout.clone();
    text.extend(&console.stderr);
    String::from_utf8_lossy(&text).into_owned()
}

/// Put `bytes` at `offset` of `data`.
fn put(data: &mut [u8], offset: usize, bytes: &[u8]) {
    data[offset..offset + bytes.len()].copy_from_slice(bytes);
}

/// Call the import `slot` with `args`, pushed last first.
fn call(a: &mut CodeAssembler, slot: AsmMemoryOperand, args: &[i32]) -> Result<(), IcedError> {
    for &arg in args.iter().rev() {
        a.push(arg)?;
    }
    a.call(slot)
}

/// Set bit `bit` of EBX when EAX is `value`.
fn check_eax(a: &mut CodeAssembler, value: i32, bit: u32) -> Result<(), IcedError> {
    let mut skip = a.create_label();
    a.cmp(eax, value)?;
    a.jne(skip)?;
    a.or(ebx, 1 << bit)?;
    a.set_label(&mut skip)?;
    a.nop()
}

const TIMER_COUNT: u32 = 0x00;
const TIMER_START: u32 = 0x04;

/// A periodic timer of 10 ms counted by its callback over Sleep(105),
/// then killed. The exit code is the count, with bit 4 set if the
/// callback ran in the 50 ms after the kill, and bits 5 and 6 if
/// timeGetTime saw less than 105 ms or more than 120 ms pass over the
/// Sleep.
fn timer_program() -> Vec<u8> {
    const TIME_PERIODIC: i32 = 1;
    let imports: &[(&str, &[&str])] = &[
        ("KERNEL32.dll", &["Sleep", "ExitProcess"]),
        (
            "WINMM.dll",
            &["timeSetEvent", "timeKillEvent", "timeGetTime"],
        ),
    ];
    program(imports, &[0; 0x20], |a, l| {
        // The callback's address in EBP, which the CALL over it pushes.
        let mut start = a.create_label();
        a.call(start)?;
        // void CALLBACK TimeProc(UINT id, UINT msg, DWORD_PTR user,
        // DWORD_PTR dw1, DWORD_PTR dw2)
        a.inc(dword_ptr(l.at(TIMER_COUNT)))?;
        a.ret_1(20)?;
        a.set_label(&mut start)?;
        a.pop(ebp)?;
        a.call(l.import(4))?;
        a.mov(dword_ptr(l.at(TIMER_START)), eax)?;
        a.push(TIME_PERIODIC)?;
        a.push(0)?;
        a.push(ebp)?;
        a.push(1)?;
        a.push(10)?;
        a.call(l.import(2))?;
        a.mov(esi, eax)?;
        call(a, l.import(0), &[105])?;
        a.push(esi)?;
        a.call(l.import(3))?;
        a.call(l.import(4))?;
        a.sub(eax, dword_ptr(l.at(TIMER_START)))?;
        a.mov(edi, eax)?;
        a.mov(ebx, dword_ptr(l.at(TIMER_COUNT)))?;
        call(a, l.import(0), &[50])?;
        let mut quiet = a.create_label();
        a.cmp(ebx, dword_ptr(l.at(TIMER_COUNT)))?;
        a.je(quiet)?;
        a.or(ebx, 1 << 4)?;
        a.set_label(&mut quiet)?;
        let mut long_enough = a.create_label();
        a.cmp(edi, 105)?;
        a.jae(long_enough)?;
        a.or(ebx, 1 << 5)?;
        a.set_label(&mut long_enough)?;
        let mut short_enough = a.create_label();
        a.cmp(edi, 120)?;
        a.jbe(short_enough)?;
        a.or(ebx, 1 << 6)?;
        a.set_label(&mut short_enough)?;
        a.push(ebx)?;
        a.call(l.import(1))?;
        a.int3()
    })
}

#[test]
fn a_periodic_timer_fires_once_a_period_of_emulated_time() {
    for core in CORES {
        let mut cpu = machine("timer", core, &[("T.EXE", timer_program())], |_| {});
        run_to_exit(&mut cpu, 20_000, |_| {});
        // Sleep(105) holds 10 periods of 10 ms, and one more may come
        // due as the thread wakes.
        let code = cpu.errorlevel;
        assert!((10..=11).contains(&code), "{core:?}: exit code {code:#x}");
        assert!(printed(&cpu).is_empty(), "{:?}", printed(&cpu));
    }
}

const SPIN_FLAG: u32 = 0x00;
const SPIN_START: u32 = 0x04;
const SPIN_TICKS: u32 = 0x08;

/// A thread started, then a periodic timer of 5 ms, whose thread is of a
/// higher priority and preempts the main one every period, and the main
/// thread spinning until the thread sets a flag, for at most 2 seconds.
/// The exit code is 1 if the flag came, 2 if it didn't, with bit 4 set if
/// the timer never ran.
fn preempted_spin_program() -> Vec<u8> {
    const TIME_PERIODIC: i32 = 1;
    let imports: &[(&str, &[&str])] = &[
        ("KERNEL32.dll", &["CreateThread", "ExitProcess"]),
        ("WINMM.dll", &["timeSetEvent", "timeGetTime"]),
    ];
    program(imports, &[0; 0x20], |a, l| {
        // CreateThread's last three arguments, then its start address,
        // which the call pushes: the code after it, the thread's, which
        // sets the flag and ends.
        let mut start = a.create_label();
        a.push(0)?;
        a.push(0)?;
        a.push(0)?;
        a.call(start)?;
        a.mov(dword_ptr(l.at(SPIN_FLAG)), 1)?;
        a.xor(eax, eax)?;
        a.ret_1(4)?;
        // The timer's callback (id, msg, user, dw1, dw2), whose address
        // the call before it pushes: count.
        let mut main = a.create_label();
        a.set_label(&mut start)?;
        a.push(0)?;
        a.push(0)?;
        a.call(l.import(0))?;
        a.call(main)?;
        a.inc(dword_ptr(l.at(SPIN_TICKS)))?;
        a.ret_1(20)?;
        a.set_label(&mut main)?;
        a.pop(ebp)?;
        a.push(TIME_PERIODIC)?;
        a.push(0)?;
        a.push(ebp)?;
        a.push(1)?;
        a.push(5)?;
        a.call(l.import(2))?;
        a.call(l.import(3))?;
        a.mov(dword_ptr(l.at(SPIN_START)), eax)?;
        let (mut spin, mut done) = (a.create_label(), a.create_label());
        a.mov(ebx, 1)?;
        a.set_label(&mut spin)?;
        a.cmp(dword_ptr(l.at(SPIN_FLAG)), 0)?;
        a.jne(done)?;
        a.call(l.import(3))?;
        a.sub(eax, dword_ptr(l.at(SPIN_START)))?;
        a.cmp(eax, 2000)?;
        a.jb(spin)?;
        a.mov(ebx, 2)?;
        a.set_label(&mut done)?;
        let mut ticked = a.create_label();
        a.cmp(dword_ptr(l.at(SPIN_TICKS)), 0)?;
        a.jne(ticked)?;
        a.or(ebx, 1 << 4)?;
        a.set_label(&mut ticked)?;
        a.push(ebx)?;
        a.call(l.import(1))?;
        a.int3()
    })
}

/// A thread that a multimedia timer's thread preempts goes on with the
/// rest of its turn, so another thread of its priority gets the
/// processor when that turn is over, as on Windows. The timer used to
/// start the spinning thread's turn over every period, and the other
/// thread, started before the timer, never ran.
#[test]
fn a_thread_a_timer_preempts_still_gives_way_at_the_end_of_its_turn() {
    for core in CORES {
        let files = [("T.EXE", preempted_spin_program())];
        let mut cpu = machine("preempted_spin", core, &files, |_| {});
        run_to_exit(&mut cpu, 20_000, |_| {});
        let code = cpu.errorlevel;
        assert_eq!(code, 1, "{core:?}: exit code {code:#x}");
        assert!(printed(&cpu).is_empty(), "{:?}", printed(&cpu));
    }
}

const NEST_DONE: u32 = 0x00;
const NEST_BUSY: u32 = 0x04;
const NEST_TICKS: u32 = 0x08;
const NEST_SEEN: u32 = 0x0C;
const NEST_OVERLAP: u32 = 0x10;
const NEST_MAIN_BUSY: u32 = 0x14;
const NEST_START: u32 = 0x18;

/// A thread that ten times sleeps 1 ms and then is busy for 8 ms, with a
/// flag set, a periodic timer of 5 ms, and the main thread, of the
/// thread's priority, spinning until the thread is done, for at most 2
/// seconds, counting the passes it makes while the flag is set. The
/// thread whose sleep ends preempts the main one, and the timer's thread
/// preempts it while it is busy. The exit code has bit 0 set if the
/// thread never finished, bit 1 if the main thread ran while the other
/// was busy, and bit 2 if the timer never ran while it was.
fn nested_preemption_program() -> Vec<u8> {
    const TIME_PERIODIC: i32 = 1;
    let imports: &[(&str, &[&str])] = &[
        ("KERNEL32.dll", &["CreateThread", "ExitProcess", "Sleep"]),
        ("WINMM.dll", &["timeSetEvent", "timeGetTime"]),
    ];
    program(imports, &[0; 0x20], |a, l| {
        // CreateThread's last three arguments, then its start address,
        // which the call pushes: the code after it, the thread's.
        let mut create = a.create_label();
        a.push(0)?;
        a.push(0)?;
        a.push(0)?;
        a.call(create)?;
        let (mut round, mut spin, mut quiet) =
            (a.create_label(), a.create_label(), a.create_label());
        a.mov(esi, 10)?;
        a.set_label(&mut round)?;
        a.push(1)?;
        a.call(l.import(2))?;
        a.mov(dword_ptr(l.at(NEST_BUSY)), 1)?;
        a.mov(eax, dword_ptr(l.at(NEST_TICKS)))?;
        a.mov(dword_ptr(l.at(NEST_SEEN)), eax)?;
        a.call(l.import(4))?;
        a.mov(edi, eax)?;
        a.set_label(&mut spin)?;
        a.call(l.import(4))?;
        a.sub(eax, edi)?;
        a.cmp(eax, 8)?;
        a.jb(spin)?;
        a.mov(eax, dword_ptr(l.at(NEST_TICKS)))?;
        a.cmp(eax, dword_ptr(l.at(NEST_SEEN)))?;
        a.je(quiet)?;
        a.mov(dword_ptr(l.at(NEST_OVERLAP)), 1)?;
        a.set_label(&mut quiet)?;
        a.mov(dword_ptr(l.at(NEST_BUSY)), 0)?;
        a.dec(esi)?;
        a.jnz(round)?;
        a.mov(dword_ptr(l.at(NEST_DONE)), 1)?;
        a.xor(eax, eax)?;
        a.ret_1(4)?;
        // The timer's callback (id, msg, user, dw1, dw2), whose address
        // the call before it pushes: count.
        let mut main = a.create_label();
        a.set_label(&mut create)?;
        a.push(0)?;
        a.push(0)?;
        a.call(l.import(0))?;
        a.call(main)?;
        a.inc(dword_ptr(l.at(NEST_TICKS)))?;
        a.ret_1(20)?;
        a.set_label(&mut main)?;
        a.pop(ebp)?;
        a.push(TIME_PERIODIC)?;
        a.push(0)?;
        a.push(ebp)?;
        a.push(1)?;
        a.push(5)?;
        a.call(l.import(3))?;
        a.call(l.import(4))?;
        a.mov(dword_ptr(l.at(NEST_START)), eax)?;
        let (mut wait, mut free, mut finish) =
            (a.create_label(), a.create_label(), a.create_label());
        a.set_label(&mut wait)?;
        a.cmp(dword_ptr(l.at(NEST_DONE)), 0)?;
        a.jne(finish)?;
        a.cmp(dword_ptr(l.at(NEST_BUSY)), 0)?;
        a.je(free)?;
        a.inc(dword_ptr(l.at(NEST_MAIN_BUSY)))?;
        a.set_label(&mut free)?;
        a.call(l.import(4))?;
        a.sub(eax, dword_ptr(l.at(NEST_START)))?;
        a.cmp(eax, 2000)?;
        a.jb(wait)?;
        a.set_label(&mut finish)?;
        let (mut finished, mut alone, mut overlapped) =
            (a.create_label(), a.create_label(), a.create_label());
        a.xor(ebx, ebx)?;
        a.cmp(dword_ptr(l.at(NEST_DONE)), 0)?;
        a.jne(finished)?;
        a.or(ebx, 1)?;
        a.set_label(&mut finished)?;
        a.cmp(dword_ptr(l.at(NEST_MAIN_BUSY)), 0)?;
        a.je(alone)?;
        a.or(ebx, 2)?;
        a.set_label(&mut alone)?;
        a.cmp(dword_ptr(l.at(NEST_OVERLAP)), 0)?;
        a.jne(overlapped)?;
        a.or(ebx, 4)?;
        a.set_label(&mut overlapped)?;
        a.push(ebx)?;
        a.call(l.import(1))?;
        a.int3()
    })
}

/// A thread that preempted another and is preempted in turn goes on
/// first when the thread that preempted it stops, as Windows puts each
/// preempted thread back at the head of its priority's queue. The main
/// thread, preempted first, used to go on instead, while the other was in
/// the middle of its work.
#[test]
fn a_thread_preempted_while_it_preempts_another_goes_on_first() {
    for core in CORES {
        let files = [("T.EXE", nested_preemption_program())];
        let mut cpu = machine("nested_preemption", core, &files, |_| {});
        run_to_exit(&mut cpu, 20_000, |_| {});
        let code = cpu.errorlevel;
        assert_eq!(code, 0, "{core:?}: exit code {code:#x}");
        assert!(printed(&cpu).is_empty(), "{:?}", printed(&cpu));
    }
}

/// MIDI out device 0 opened, a note played on channel 1 for 100 ms and
/// stopped, a second one on channel 2 left for midiOutReset, and the
/// device closed. A bit in the exit code for each call that worked.
fn midi_program() -> Vec<u8> {
    let imports: &[(&str, &[&str])] = &[
        ("KERNEL32.dll", &["Sleep", "ExitProcess"]),
        (
            "WINMM.dll",
            &[
                "midiOutOpen",
                "midiOutShortMsg",
                "midiOutReset",
                "midiOutClose",
            ],
        ),
    ];
    program(imports, &[0; 0x10], |a, l| {
        a.xor(ebx, ebx)?;
        call(a, l.import(2), &[l.at(0) as i32, 0, 0, 0, 0])?;
        check_eax(a, 0, 0)?;
        a.mov(esi, dword_ptr(l.at(0)))?;
        // Note on: key 60 at velocity 127, and key 64 at 100.
        a.push(0x7F_3C_90)?;
        a.push(esi)?;
        a.call(l.import(3))?;
        check_eax(a, 0, 1)?;
        a.push(0x64_40_91)?;
        a.push(esi)?;
        a.call(l.import(3))?;
        check_eax(a, 0, 2)?;
        call(a, l.import(0), &[100])?;
        // Note off for the first: a note on at velocity 0, in running
        // status after a status byte of its own.
        a.push(0x00_3C_90)?;
        a.push(esi)?;
        a.call(l.import(3))?;
        check_eax(a, 0, 3)?;
        call(a, l.import(0), &[100])?;
        a.push(esi)?;
        a.call(l.import(4))?;
        check_eax(a, 0, 4)?;
        a.push(esi)?;
        a.call(l.import(5))?;
        check_eax(a, 0, 5)?;
        a.push(ebx)?;
        a.call(l.import(1))?;
        a.int3()
    })
}

#[test]
fn midi_out_messages_reach_the_mpu_401s_synthesizer() {
    for core in CORES {
        let mut cpu = machine("midi", core, &[("T.EXE", midi_program())], |_| {});
        // What the synthesizer holds on channels 1 and 2, as it changes.
        let mut seen = vec![(0, 0)];
        run_to_exit(&mut cpu, 20_000, |cpu| {
            let held = cpu.bus.mpu.held_notes();
            let now = (held[0][60], held[1][64]);
            if seen.last() != Some(&now) {
                seen.push(now);
            }
        });
        assert_eq!(cpu.errorlevel, 0x3F, "{core:?}: {:?}", printed(&cpu));
        assert_eq!(seen, [(0, 0), (127, 100), (0, 100), (0, 0)], "{core:?}");
    }
}

const CD_COMMANDS: [&str; 8] = [
    "open cdaudio alias cd",
    "set cd time format tmsf",
    "play cd from 2",
    "status cd mode",
    "status cd position",
    "stop cd",
    "status cd mode",
    "close cd",
];

/// The MCI strings of CD_COMMANDS sent in turn, with 200 ms between the
/// play and the position. What each status returns is printed on a line
/// of its own; the exit code has a bit for each string that worked.
fn cd_program() -> Vec<u8> {
    const RET: u32 = 0x300;
    const CRLF: u32 = 0x340;
    const WRITTEN: u32 = 0x348;
    let mut data = vec![0u8; 0x350];
    for (i, command) in CD_COMMANDS.iter().enumerate() {
        put(&mut data, i * 0x40, command.as_bytes());
    }
    put(&mut data, CRLF as usize, b"\r\n");
    let imports: &[(&str, &[&str])] = &[
        (
            "KERNEL32.dll",
            &[
                "Sleep",
                "ExitProcess",
                "GetStdHandle",
                "WriteFile",
                "lstrlenA",
            ],
        ),
        ("WINMM.dll", &["mciSendStringA"]),
    ];
    program(imports, &data, |a, l| {
        a.xor(ebx, ebx)?;
        call(a, l.import(2), &[STD_OUTPUT_HANDLE])?;
        a.mov(edi, eax)?;
        for (i, command) in CD_COMMANDS.iter().enumerate() {
            if command.starts_with("status cd position") {
                call(a, l.import(0), &[200])?;
            }
            let args = [l.at(i as u32 * 0x40) as i32, l.at(RET) as i32, 64, 0];
            call(a, l.import(5), &args)?;
            check_eax(a, 0, i as u32)?;
            if command.starts_with("status") {
                call(a, l.import(4), &[l.at(RET) as i32])?;
                a.push(0)?;
                a.push(l.at(WRITTEN) as i32)?;
                a.push(eax)?;
                a.push(l.at(RET) as i32)?;
                a.push(edi)?;
                a.call(l.import(3))?;
                a.push(0)?;
                a.push(l.at(WRITTEN) as i32)?;
                a.push(2)?;
                a.push(l.at(CRLF) as i32)?;
                a.push(edi)?;
                a.call(l.import(3))?;
            }
        }
        a.push(ebx)?;
        a.call(l.import(1))?;
        a.int3()
    })
}

#[test]
fn mci_plays_an_audio_track_of_a_cd_image() {
    for core in CORES {
        let dir = cdimage::scratch("winmm_cd");
        let iso = cdimage::iso("TESTDISC", &[cdimage::file("A.TXT", b"a")]);
        // Five seconds of audio in track 2.
        let cue = cdimage::mixed_disc(&dir, &iso, 375);
        let mut cpu = machine("cd", core, &[("T.EXE", cd_program())], |cpu| {
            let options = MountOptions::default();
            cpu.bus.mount_drive(3, &cue, options, false).unwrap();
        });
        let mut played = false;
        run_to_exit(&mut cpu, 20_000, |cpu| {
            let player = &cpu.bus.cdaudio;
            played |= player.state() == PlayState::Playing && player.track() == Some(2);
        });
        let text = printed(&cpu);
        assert_eq!(cpu.errorlevel, 0xFF, "{core:?}: {text:?}");
        assert!(played, "{core:?}: the drive didn't play track 2");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{core:?}: {text:?}");
        assert_eq!(lines[0], "playing");
        // 200 ms is 15 of the 75 frames a second.
        assert_eq!(lines[1], "02:00:00:15");
        assert_eq!(lines[2], "stopped");
        assert_eq!(cpu.bus.cdaudio.state(), PlayState::Stopped, "{core:?}");
    }
}

// The program's data for the mmio test: two file names, and the
// MMCKINFOs (ckid, cksize, fccType, dwDataOffset, dwFlags) it reads and
// writes the chunks with.
const IN_NAME: u32 = 0x00;
const OUT_NAME: u32 = 0x10;
const READ_RIFF: u32 = 0x40;
const READ_CHUNK: u32 = 0x60;
const BUFFER: u32 = 0x80;
const TEXT: u32 = 0xA0;
const WRITE_RIFF: u32 = 0xC0;
const WRITE_CHUNK: u32 = 0xE0;

/// IN.WAV: a RIFF WAVE file of a 'fmt ' chunk and an 8-byte 'data' one.
fn riff_in() -> Vec<u8> {
    let mut file = b"RIFF".to_vec();
    file.extend(44u32.to_le_bytes());
    file.extend(b"WAVEfmt ");
    file.extend(16u32.to_le_bytes());
    file.extend([1, 0, 1, 0, 0x11, 0x2B, 0, 0, 0x11, 0x2B, 0, 0, 1, 0, 8, 0]);
    file.extend(b"data");
    file.extend(8u32.to_le_bytes());
    file.extend(b"12345678");
    file
}

/// IN.WAV's 'data' chunk found and read with mmioDescend and mmioRead,
/// and a search for its 'fmt ' chunk past it, which fails; then
/// OUT.WAV made with a RIFF WAVE chunk holding a 'data' chunk of three
/// bytes. Bits 0 to 6 of the exit code are the steps of the reading
/// that worked, and bit 7 is set if each of the writing's calls did.
fn mmio_program() -> Vec<u8> {
    const MMIO_WRITE: i32 = 1;
    const MMIO_CREATE: i32 = 0x1000;
    const MMIO_FINDCHUNK: i32 = 0x10;
    const MMIO_FINDRIFF: i32 = 0x20;
    const MMIO_CREATERIFF: i32 = 0x20;
    const MMIOERR_CHUNKNOTFOUND: i32 = 265;
    let fourcc = |s: &[u8; 4]| u32::from_le_bytes(*s) as i32;
    let mut data = vec![0u8; 0x100];
    put(&mut data, IN_NAME as usize, b"IN.WAV");
    put(&mut data, OUT_NAME as usize, b"OUT.WAV");
    put(&mut data, (READ_RIFF + 8) as usize, b"WAVE");
    put(&mut data, READ_CHUNK as usize, b"data");
    put(&mut data, TEXT as usize, b"abc");
    put(&mut data, (WRITE_RIFF + 8) as usize, b"WAVE");
    put(&mut data, WRITE_CHUNK as usize, b"data");
    let imports: &[(&str, &[&str])] = &[
        ("KERNEL32.dll", &["ExitProcess"]),
        (
            "WINMM.dll",
            &[
                "mmioOpenA",
                "mmioDescend",
                "mmioRead",
                "mmioAscend",
                "mmioCreateChunk",
                "mmioWrite",
                "mmioClose",
            ],
        ),
    ];
    program(imports, &data, |a, l| {
        let at = |offset: u32| l.at(offset) as i32;
        a.xor(ebx, ebx)?;
        call(a, l.import(1), &[at(IN_NAME), 0, 0])?;
        a.mov(esi, eax)?;
        a.test(eax, eax)?;
        a.setne(bl)?;
        a.push(MMIO_FINDRIFF)?;
        a.push(0)?;
        a.push(at(READ_RIFF))?;
        a.push(esi)?;
        a.call(l.import(2))?;
        check_eax(a, 0, 1)?;
        let find_chunk = |a: &mut CodeAssembler| -> Result<(), IcedError> {
            a.push(MMIO_FINDCHUNK)?;
            a.push(at(READ_RIFF))?;
            a.push(at(READ_CHUNK))?;
            a.push(esi)?;
            a.call(l.import(2))
        };
        find_chunk(a)?;
        check_eax(a, 0, 2)?;
        a.mov(eax, dword_ptr(l.at(READ_CHUNK + 4)))?;
        check_eax(a, 8, 3)?;
        a.push(8)?;
        a.push(at(BUFFER))?;
        a.push(esi)?;
        a.call(l.import(3))?;
        a.mov(eax, dword_ptr(l.at(BUFFER + 4)))?;
        check_eax(a, fourcc(b"5678"), 4)?;
        a.push(0)?;
        a.push(at(READ_CHUNK))?;
        a.push(esi)?;
        a.call(l.import(4))?;
        check_eax(a, 0, 5)?;
        a.mov(dword_ptr(l.at(READ_CHUNK)), fourcc(b"fmt "))?;
        find_chunk(a)?;
        check_eax(a, MMIOERR_CHUNKNOTFOUND, 6)?;
        a.push(0)?;
        a.push(esi)?;
        a.call(l.import(7))?;

        // Each call of the writing jumps to `failed` if it fails.
        let mut failed = a.create_label();
        call(a, l.import(1), &[at(OUT_NAME), 0, MMIO_CREATE | MMIO_WRITE])?;
        a.mov(esi, eax)?;
        a.test(eax, eax)?;
        a.jz(failed)?;
        let steps: [(usize, [i32; 3], i32); 6] = [
            (5, [at(WRITE_RIFF), MMIO_CREATERIFF, 0], 0),
            (5, [at(WRITE_CHUNK), 0, 0], 0),
            (6, [at(TEXT), 3, 0], 3),
            (4, [at(WRITE_CHUNK), 0, 0], 0),
            (4, [at(WRITE_RIFF), 0, 0], 0),
            (7, [0, 0, 0], 0),
        ];
        for (import, args, ok) in steps {
            // mmioClose takes two arguments, the others three.
            let count = if import == 7 { 1 } else { 2 };
            for &arg in args[..count].iter().rev() {
                a.push(arg)?;
            }
            a.push(esi)?;
            a.call(l.import(import))?;
            a.cmp(eax, ok)?;
            a.jne(failed)?;
        }
        a.or(ebx, 1 << 7)?;
        a.set_label(&mut failed)?;
        a.push(ebx)?;
        a.call(l.import(0))?;
        a.int3()
    })
}

#[test]
fn mmio_reads_and_writes_riff_chunks() {
    for core in CORES {
        let files = [("T.EXE", mmio_program()), ("IN.WAV", riff_in())];
        let mut cpu = machine("mmio", core, &files, |_| {});
        run_to_exit(&mut cpu, 20_000, |_| {});
        assert_eq!(cpu.errorlevel, 0xFF, "{core:?}: {:?}", printed(&cpu));
        let out = fs::read(scratch_path("mmio").join("OUT.WAV")).unwrap();
        // The 'data' chunk's odd size is padded to a word.
        let mut expected = b"RIFF".to_vec();
        expected.extend(16u32.to_le_bytes());
        expected.extend(b"WAVEdata");
        expected.extend(3u32.to_le_bytes());
        expected.extend(b"abc\0");
        assert_eq!(out, expected, "{core:?}");
    }
}

// The sequencer test's data: its two MCI strings, the alias, an
// MCI_SEQ_SET_PARMS (dwCallback, dwTimeFormat, dwAudio, dwTempo, dwPort,
// dwSlave, dwMaster, dwOffset) and an MCI_STATUS_PARMS (dwCallback,
// dwReturn, dwItem, dwTrack).
const SEQ_OPEN: u32 = 0x00;
const SEQ_CLOSE: u32 = 0x40;
const SEQ_ALIAS: u32 = 0x60;
const SEQ_PARMS: u32 = 0x80;
const SEQ_PORT: u32 = SEQ_PARMS + 16;
const SEQ_STATUS: u32 = 0xA0;
const SEQ_RETURN: u32 = SEQ_STATUS + 4;
const SEQ_ITEM: u32 = SEQ_STATUS + 8;

/// A Standard MIDI File of one empty track.
fn empty_midi() -> Vec<u8> {
    let mut file = b"MThd".to_vec();
    file.extend(6u32.to_be_bytes());
    file.extend([0, 0, 0, 1, 0, 96]);
    file.extend(b"MTrk");
    file.extend(4u32.to_be_bytes());
    file.extend([0x00, 0xFF, 0x2F, 0x00]);
    file
}

/// M.MID opened on the sequencer, its port set to Win32's MIDI_MAPPER,
/// (UINT)-1, then to port 0, then to port 2, which doesn't exist, and
/// closed, with MCI_STATUS of the port after each set. Bits of the exit
/// code: 0, the open worked; 1 and 2, the mapper was taken and reported
/// as 65535, the 16-bit driver's word; 3 and 4, port 0 was taken and
/// reported; 5 and 6, port 2 was out of range and port 0 stayed; 7, the
/// close worked.
fn sequencer_program() -> Vec<u8> {
    let mut data = vec![0u8; 0xC0];
    put(
        &mut data,
        SEQ_OPEN as usize,
        b"open M.MID type sequencer alias m",
    );
    put(&mut data, SEQ_CLOSE as usize, b"close m");
    put(&mut data, SEQ_ALIAS as usize, b"m");
    const MCI_SEQ_STATUS_PORT: u32 = 0x4003;
    put(
        &mut data,
        SEQ_ITEM as usize,
        &MCI_SEQ_STATUS_PORT.to_le_bytes(),
    );
    let imports: &[(&str, &[&str])] = &[
        ("KERNEL32.dll", &["ExitProcess"]),
        (
            "WINMM.dll",
            &["mciSendStringA", "mciGetDeviceIDA", "mciSendCommandA"],
        ),
    ];
    const MCI_SET: i32 = 0x80D;
    const MCI_STATUS: i32 = 0x814;
    const MCI_SEQ_SET_PORT: i32 = 0x0002_0000;
    const MCI_STATUS_ITEM: i32 = 0x100;
    const MCIERR_OUTOFRANGE: i32 = 282;
    let sets = [
        (-1, 0, 65535, 1),
        (0, 0, 0, 3),
        (2, MCIERR_OUTOFRANGE, 0, 5),
    ];
    program(imports, &data, |a, l| {
        a.xor(ebx, ebx)?;
        call(a, l.import(1), &[l.at(SEQ_OPEN) as i32, 0, 0, 0])?;
        check_eax(a, 0, 0)?;
        call(a, l.import(2), &[l.at(SEQ_ALIAS) as i32])?;
        a.mov(esi, eax)?;
        for (port, result, reported, bit) in sets {
            a.mov(dword_ptr(l.at(SEQ_PORT)), port)?;
            a.push(l.at(SEQ_PARMS) as i32)?;
            a.push(MCI_SEQ_SET_PORT)?;
            a.push(MCI_SET)?;
            a.push(esi)?;
            a.call(l.import(3))?;
            check_eax(a, result, bit)?;
            a.mov(dword_ptr(l.at(SEQ_RETURN)), -2)?;
            a.push(l.at(SEQ_STATUS) as i32)?;
            a.push(MCI_STATUS_ITEM)?;
            a.push(MCI_STATUS)?;
            a.push(esi)?;
            a.call(l.import(3))?;
            a.test(eax, eax)?;
            let mut failed = a.create_label();
            a.jne(failed)?;
            a.mov(eax, dword_ptr(l.at(SEQ_RETURN)))?;
            check_eax(a, reported, bit + 1)?;
            a.set_label(&mut failed)?;
            a.nop()?;
        }
        call(a, l.import(1), &[l.at(SEQ_CLOSE) as i32, 0, 0, 0])?;
        check_eax(a, 0, 7)?;
        a.push(ebx)?;
        a.call(l.import(0))?;
        a.int3()
    })
}

#[test]
fn the_sequencer_takes_win32s_midi_mapper_as_its_port() {
    for core in CORES {
        let files = [("T.EXE", sequencer_program()), ("M.MID", empty_midi())];
        let mut cpu = machine("sequencer", core, &files, |_| {});
        run_to_exit(&mut cpu, 20_000, |_| {});
        assert_eq!(cpu.errorlevel, 0xFF, "{core:?}: {:?}", printed(&cpu));
    }
}

const AUX_CAPS: u32 = 0x00;
const AUX_VOLUME: u32 = 0x40;

/// The auxiliary devices as Test Drive 4 uses them: auxSetVolume on
/// device 1 with no auxGetNumDevs first, then the CD's device and the
/// microphone's, which is mono. A bit in the exit code for each call
/// that returned what Windows 95's Sound Blaster 16 driver does.
fn aux_program() -> Vec<u8> {
    const MMSYSERR_BADDEVICEID: i32 = 2;
    const AUXCAPS_CDAUDIO: i32 = 1;
    let imports: &[(&str, &[&str])] = &[
        ("KERNEL32.dll", &["Sleep", "ExitProcess"]),
        (
            "WINMM.dll",
            &[
                "auxGetNumDevs",
                "auxGetDevCapsA",
                "auxGetVolume",
                "auxSetVolume",
            ],
        ),
    ];
    program(imports, &[0; 0x50], |a, l| {
        a.xor(ebx, ebx)?;
        call(a, l.import(5), &[1, 0x1FFF_1FFF])?;
        check_eax(a, 0, 0)?;
        a.call(l.import(2))?;
        check_eax(a, 7, 1)?;
        call(a, l.import(5), &[2, 0x4000_2000])?;
        check_eax(a, 0, 2)?;
        call(a, l.import(5), &[7, 0])?;
        check_eax(a, MMSYSERR_BADDEVICEID, 3)?;
        // Device 2 is the CD's: AUXCAPS' wTechnology, at 40, says so.
        call(a, l.import(3), &[2, l.at(AUX_CAPS) as i32, 48])?;
        check_eax(a, 0, 4)?;
        a.movzx(eax, word_ptr(l.at(AUX_CAPS + 40)))?;
        check_eax(a, AUXCAPS_CDAUDIO, 5)?;
        // There is no aux mapper.
        call(a, l.import(3), &[-1, l.at(AUX_CAPS) as i32, 48])?;
        check_eax(a, MMSYSERR_BADDEVICEID, 6)?;
        // The microphone keeps the volume it is given.
        call(a, l.import(5), &[4, 0x4000_2000])?;
        call(a, l.import(4), &[4, l.at(AUX_VOLUME) as i32])?;
        a.mov(eax, dword_ptr(l.at(AUX_VOLUME)))?;
        check_eax(a, 0x4000_2000, 7)?;
        // A while for the test to see the volumes before the exit.
        call(a, l.import(0), &[100])?;
        a.push(ebx)?;
        a.call(l.import(1))?;
        a.int3()
    })
}

/// Device 1 is the MIDI synthesizer's volume, as on a Sound Blaster 16,
/// and device 2 the CD's: what they are set to scales what each plays.
#[test]
fn aux_devices_are_the_sound_blaster_16s_lines() {
    for core in CORES {
        let mut cpu = machine("aux", core, &[("T.EXE", aux_program())], |_| {});
        let mut seen = Vec::new();
        run_to_exit(&mut cpu, 20_000, |cpu| {
            let now = (cpu.bus.win32.midi_gain(), cpu.bus.win32.cd_gain());
            if seen.last() != Some(&now) {
                seen.push(now);
            }
        });
        assert_eq!(cpu.errorlevel, 0xFF, "{core:?}: {:?}", printed(&cpu));
        let level = |v: u32| v as f32 / 65535.0;
        let set = (
            (level(0x1FFF), level(0x1FFF)),
            (level(0x2000), level(0x4000)),
        );
        assert!(seen.contains(&set), "{core:?}: {seen:?}");
    }
}

/// 12 frames of 32 by 32 at 25 a second, 6 red and 6 blue: a video
/// elementary stream, as an .MPG file may be. The DirectShow tests
/// play it too.
pub(super) fn clip() -> Vec<u8> {
    use oxideav_mpeg12video::frame_assembly::FrameBuffer;
    use oxideav_mpeg12video::mpeg1_encoder::encode_mpeg1_display_order_sequence;
    use oxideav_mpeg12video::mpeg1_stream_writer::Mpeg1SequenceParams;
    use oxideav_mpeg12video::sequence_extension::ChromaFormat;
    let frame = |(y, cb, cr): (u8, u8, u8)| {
        let mut f = FrameBuffer::new(32, 32, ChromaFormat::Yuv420);
        for (plane, v) in [(&mut f.y, y), (&mut f.cb, cb), (&mut f.cr, cr)] {
            for row in 0..plane.height() {
                for col in 0..plane.width() {
                    plane.put_sample(col, row, v);
                }
            }
        }
        f
    };
    let frames: Vec<FrameBuffer> = (0..12)
        .map(|n| frame(if n < 6 { (81, 90, 240) } else { (41, 240, 110) }))
        .collect();
    let seq = Mpeg1SequenceParams {
        horizontal_size: 32,
        vertical_size: 32,
        picture_rate_code: 3,
        ..Default::default()
    };
    encode_mpeg1_display_order_sequence(&frames, 2, 2, &seq, 4, 1, 1).unwrap()
}

// The program's data for the MPEGVideo test.
const CLIP_NAME: u32 = 0x00;
const STATIC: u32 = 0x20;
const OPEN_PARMS: u32 = 0x40;
const STATUS_PARMS: u32 = 0x60;
const WHERE_PARMS: u32 = 0x80;
const PLAY_PARMS: u32 = 0xA0;
const CLOSE_PARMS: u32 = 0xC0;

/// The program: a popup window, and the file `name` on C: opened in a
/// child of it with MCI_DGV_OPEN_PARMSA, whose window (MCI_STATUS) is
/// that child and whose source rectangle (MCI_WHERE) is the picture, 32
/// by 32. A play draws a red frame 100 ms in and a blue one at its end,
/// which GetPixel sees in the window, and the device then stops. The
/// exit code has a bit for each.
fn video_program(name: &str) -> Vec<u8> {
    const WS_POPUP_VISIBLE: i32 = 0x9000_0000_u32 as i32;
    const WS_CHILD_VISIBLE: i32 = 0x5000_0000;
    const MCI_OPEN: i32 = 0x803;
    const MCI_CLOSE: i32 = 0x804;
    const MCI_PLAY: i32 = 0x806;
    const MCI_STATUS: i32 = 0x814;
    const MCI_WHERE: i32 = 0x843;
    const OPEN_FLAGS: i32 = 0x0200 | 0x1_0000 | 0x2_0000;
    let mut data = vec![0u8; 0xE0];
    put(&mut data, CLIP_NAME as usize, format!("C:\\{name}\0").as_bytes());
    put(&mut data, STATIC as usize, b"Static\0");
    let open = OPEN_PARMS as usize;
    put(&mut data, open + 20, &(WS_CHILD_VISIBLE as u32).to_le_bytes());
    put(&mut data, STATUS_PARMS as usize + 8, &0x4001u32.to_le_bytes());
    let imports: &[(&str, &[&str])] = &[
        ("KERNEL32.dll", &["Sleep", "ExitProcess"]),
        ("WINMM.dll", &["mciSendCommandA"]),
        (
            "USER32.dll",
            &["CreateWindowExA", "GetParent", "GetDC"],
        ),
        ("GDI32.dll", &["GetPixel"]),
    ];
    program(imports, &data, |a, l| {
        let send = |a: &mut CodeAssembler, msg: i32, flags: i32, parms: u32| {
            a.push(l.at(parms) as i32)?;
            a.push(flags)?;
            a.push(msg)?;
            a.push(edi)?;
            a.call(l.import(2))
        };
        a.xor(ebx, ebx)?;
        a.xor(edi, edi)?;
        a.mov(dword_ptr(l.at(OPEN_PARMS + 12)), l.at(CLIP_NAME) as i32)?;
        let args = [
            0,
            l.at(STATIC) as i32,
            0,
            WS_POPUP_VISIBLE,
            0,
            0,
            64,
            64,
            0,
            0,
            0,
            0,
        ];
        call(a, l.import(3), &args)?;
        a.mov(esi, eax)?;
        a.mov(dword_ptr(l.at(OPEN_PARMS + 24)), eax)?;
        send(a, MCI_OPEN, OPEN_FLAGS, OPEN_PARMS)?;
        check_eax(a, 0, 0)?;
        a.mov(edi, dword_ptr(l.at(OPEN_PARMS + 4)))?;
        send(a, MCI_STATUS, 0x100, STATUS_PARMS)?;
        check_eax(a, 0, 1)?;
        a.push(dword_ptr(l.at(STATUS_PARMS + 4)))?;
        a.call(l.import(4))?;
        a.cmp(eax, esi)?;
        let mut not_child = a.create_label();
        a.jne(not_child)?;
        a.or(ebx, 1 << 2)?;
        a.set_label(&mut not_child)?;
        a.nop()?;
        send(a, MCI_WHERE, 0x2_0000, WHERE_PARMS)?;
        check_eax(a, 0, 3)?;
        a.mov(eax, dword_ptr(l.at(WHERE_PARMS + 12)))?;
        a.add(eax, dword_ptr(l.at(WHERE_PARMS + 16)))?;
        check_eax(a, 64, 4)?;
        send(a, MCI_PLAY, 0, PLAY_PARMS)?;
        check_eax(a, 0, 5)?;
        for (sleep, bit, red) in [(100, 6, true), (500, 7, false)] {
            call(a, l.import(0), &[sleep])?;
            // GetPixel(GetDC(the device's window), 16, 16).
            a.push(16)?;
            a.push(16)?;
            a.push(dword_ptr(l.at(STATUS_PARMS + 4)))?;
            a.call(l.import(5))?;
            a.push(eax)?;
            a.call(l.import(6))?;
            // Red is 0x0000FF-ish, blue 0xFF0000-ish.
            let (hi, lo) = if red { (0, 16) } else { (16, 0) };
            let mut skip = a.create_label();
            a.mov(ecx, eax)?;
            a.shr(ecx, hi)?;
            a.and(ecx, 0xFF)?;
            a.cmp(ecx, 0xC0)?;
            a.jb(skip)?;
            a.mov(ecx, eax)?;
            a.shr(ecx, lo)?;
            a.and(ecx, 0xFF)?;
            a.cmp(ecx, 0x40)?;
            a.jae(skip)?;
            a.or(ebx, 1 << bit)?;
            a.set_label(&mut skip)?;
            a.nop()?;
        }
        send(a, MCI_CLOSE, 0, CLOSE_PARMS)?;
        a.push(ebx)?;
        a.call(l.import(1))?;
        a.int3()
    })
}

/// MPEGVideo plays an MPEG-1 file in a child window of the program's,
/// as Windows 98's MCIQTZ does: the frames come at their times.
#[test]
fn mpegvideo_draws_its_frames_in_a_child_window() {
    for core in CORES {
        let files = [("T.EXE", video_program("CLIP.MPG")), ("CLIP.MPG", clip())];
        let mut cpu = machine("mpeg", core, &files, |_| {});
        run_to_exit(&mut cpu, 20_000, |_| {});
        assert_eq!(cpu.errorlevel, 0xFF, "{core:?}: {:?}", printed(&cpu));
    }
}

/// The frames of `clip` in an AVI file: uncompressed 24-bit pictures,
/// with an index.
fn avi_clip() -> Vec<u8> {
    fn chunk(out: &mut Vec<u8>, id: &[u8], data: &[u8]) {
        out.extend_from_slice(id);
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
    }
    let le = |v: u32| v.to_le_bytes();
    let mut strh = vec![0; 56];
    strh[..4].copy_from_slice(b"vids");
    strh[20..24].copy_from_slice(&le(1));
    strh[24..28].copy_from_slice(&le(25));
    strh[32..36].copy_from_slice(&le(12));
    let mut bih = vec![0; 40];
    bih[..4].copy_from_slice(&le(40));
    bih[4..8].copy_from_slice(&le(32));
    bih[8..12].copy_from_slice(&le(32));
    bih[12..16].copy_from_slice(&[1, 0, 24, 0]);
    let mut strl = b"strl".to_vec();
    chunk(&mut strl, b"strh", &strh);
    chunk(&mut strl, b"strf", &bih);
    let mut hdrl = b"hdrl".to_vec();
    chunk(&mut hdrl, b"avih", &[0; 56]);
    chunk(&mut hdrl, b"LIST", &strl);
    let mut movi = b"movi".to_vec();
    let mut idx1 = Vec::new();
    for n in 0..12 {
        let px: [u8; 3] = if n < 6 { [0, 0, 255] } else { [255, 0, 0] };
        idx1.extend_from_slice(b"00db");
        idx1.extend_from_slice(&le(0x10));
        idx1.extend_from_slice(&le(movi.len() as u32));
        idx1.extend_from_slice(&le(32 * 32 * 3));
        chunk(&mut movi, b"00db", &px.repeat(32 * 32));
    }
    let mut body = b"AVI ".to_vec();
    chunk(&mut body, b"LIST", &hdrl);
    chunk(&mut body, b"LIST", &movi);
    chunk(&mut body, b"idx1", &idx1);
    let mut out = Vec::new();
    chunk(&mut out, b"RIFF", &body);
    out
}

/// A file with the .AVI extension opens on AVIVideo, which plays it as
/// MPEGVideo plays an MPEG-1 file.
#[test]
fn avivideo_draws_its_frames_in_a_child_window() {
    for core in CORES {
        let files = [("T.EXE", video_program("CLIP.AVI")), ("CLIP.AVI", avi_clip())];
        let mut cpu = machine("avi", core, &files, |_| {});
        run_to_exit(&mut cpu, 20_000, |_| {});
        assert_eq!(cpu.errorlevel, 0xFF, "{core:?}: {:?}", printed(&cpu));
    }
}
