//! The built-in Windows' sound: DirectSound buffers and WinMM's wave
//! output, played by Win32 programs made here with rust-dos-pe's builder,
//! heard in the host's sound (`Bus::audio_out`).

use iced_x86::code_asm::*;
use rust_dos::cpu::{CoreMode, Cpu, HostConsole};
use rust_dos::exec::{NoHook, StopReason, run_batch};
use rust_dos_pe::{self as pe, Builder, Imp};
use std::fs;
use std::path::PathBuf;

const BASE: u32 = 0x40_0000;

/// The host's sample rate.
const HOST_RATE: f64 = 44_100.0;

/// Where a test program's data and imports are, for its code.
struct Layout {
    data: u32,
    iat: Vec<u32>,
}

impl Layout {
    fn at(&self, offset: u32) -> u64 {
        (self.data + offset) as u64
    }

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
    assert_eq!(
        b.section(
            ".text",
            &code,
            pe::SCN_CODE | pe::SCN_EXECUTE | pe::SCN_READ
        ),
        code_rva
    );
    b.entry(code_rva);
    b.build()
}

/// A machine with the built-in Windows on, on `core`, and `program`
/// loaded from the shell as T.EXE, with `files` beside it (or in the
/// folders their names give).
fn machine(test: &str, program: Vec<u8>, core: CoreMode, files: &[(&str, Vec<u8>)]) -> Box<Cpu> {
    // Each core its own directory, as the tests run at once.
    let base = PathBuf::from("target/test_win32_audio").join(format!("{test}_{core:?}"));
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    fs::write(base.join("T.EXE"), program).unwrap();
    for (name, bytes) in files {
        let path = base.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    let mut cpu = Cpu::new(base);
    cpu.core = core;
    cpu.bus.win32.enabled = true;
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    assert!(cpu.load_executable("T.EXE", None));
    cpu
}

/// Run the program to its end, at most `batches` batches; its exit code
/// and the host's sound meanwhile, left channel only.
fn run(cpu: &mut Cpu, batches: usize) -> (u8, Vec<i16>) {
    let mut left = Vec::new();
    for _ in 0..batches {
        cpu.bus.start_batch(cpu.bus.clock.icount + 100_000);
        let stop = run_batch(cpu, &mut NoHook, false);
        cpu.bus.audio_catch_up();
        left.extend(cpu.bus.audio_out.drain(..).step_by(2));
        if stop == StopReason::ShellReloaded {
            return (cpu.errorlevel, left);
        }
    }
    panic!("the program didn't end");
}

/// A square wave of 16-bit mono samples, `period` frames to a cycle.
fn square(frames: usize, period: usize) -> Vec<u8> {
    (0..frames)
        .flat_map(|i| {
            let s: i16 = if i % period < period / 2 { 8000 } else { -8000 };
            s.to_le_bytes()
        })
        .collect()
}

/// The pitch of `samples` at the host's rate, in Hz, from the sign
/// changes where they aren't silent; and how loud the loudest is.
fn pitch(samples: &[i16]) -> (f64, i16) {
    let start = samples.iter().position(|&s| s != 0).unwrap_or(0);
    let end = samples.iter().rposition(|&s| s != 0).map_or(0, |i| i + 1);
    let heard = &samples[start..end.max(start)];
    let rising = heard.windows(2).filter(|w| w[0] < 0 && w[1] >= 0).count();
    let peak = heard.iter().map(|s| s.saturating_abs()).max().unwrap_or(0);
    let seconds = heard.len() as f64 / HOST_RATE;
    (rising as f64 / seconds.max(1e-9), peak)
}

/// Call method `offset` of the COM object whose pointer is at `object`,
/// its arguments pushed already.
fn method(a: &mut CodeAssembler, object: u64, offset: i32) -> Result<(), IcedError> {
    a.mov(eax, dword_ptr(object))?;
    a.push(eax)?;
    a.mov(eax, dword_ptr(eax))?;
    a.call(dword_ptr(eax + offset))
}

/// End the program with `step` as its exit code if the call before
/// returned other than 0.
fn check(a: &mut CodeAssembler, fail: CodeLabel, step: i32) -> Result<(), IcedError> {
    a.mov(ebx, step)?;
    a.test(eax, eax)?;
    a.jnz(fail)
}

const DSOUND: &[&str] = &["DirectSoundCreate"];
const KERNEL32: &[&str] = &[
    "ExitProcess",
    "Sleep",
    "GetTickCount",
    "CreateEventA",
    "WaitForSingleObject",
];
// KERNEL32 is the first DLL a program imports from, the other after it.
const EXIT: usize = 0;
const SLEEP: usize = 1;
const TICKS: usize = 2;
const CREATE_EVENT: usize = 3;
const WAIT: usize = 4;
/// The first import of the other DLL.
const OTHER: usize = 5;

// The data of the DirectSound programs.
const DS: u32 = 0x00;
const BUF: u32 = 0x04;
const P1: u32 = 0x08;
const N1: u32 = 0x0C;
const P2: u32 = 0x10;
const N2: u32 = 0x14;
const NOTIFY: u32 = 0x18;
const EVENT: u32 = 0x1C;
const DESC: u32 = 0x20;
const FORMAT: u32 = 0x40;
const POSITIONS: u32 = 0x60;
const IID_NOTIFY: u32 = 0x80;
const T0: u32 = 0x90;
const SAMPLES: u32 = 0x1000;

// IDirectSound's and IDirectSoundBuffer's methods, as vtable offsets.
const DS_CREATE_SOUND_BUFFER: i32 = 12;
const DS_SET_COOPERATIVE_LEVEL: i32 = 24;
const DSB_QUERY_INTERFACE: i32 = 0;
const DSB_GET_CURRENT_POSITION: i32 = 16;
const DSB_LOCK: i32 = 44;
const DSB_PLAY: i32 = 48;
const DSB_UNLOCK: i32 = 76;
const DSN_SET_NOTIFICATION_POSITIONS: i32 = 12;

const DSBCAPS_CTRLPOSITIONNOTIFY: u32 = 0x100;
const DSBCAPS_GLOBALFOCUS: u32 = 0x8000;

/// IID_IDirectSoundNotify.
const IID_IDIRECTSOUNDNOTIFY: [u8; 16] = [
    0x83, 0x07, 0x21, 0xB0, 0xCD, 0x89, 0xD0, 0x11, 0xAF, 0x08, 0x00, 0xA0, 0xC9, 0x25, 0xCD, 0x16,
];

/// The data of a program with a secondary buffer of `samples`, 16-bit
/// mono at `rate`, with `flags`.
fn ds_data(samples: &[u8], rate: u32, flags: u32) -> Vec<u8> {
    let mut data = vec![0u8; SAMPLES as usize];
    data.extend_from_slice(samples);
    let put = |data: &mut Vec<u8>, at: u32, value: u32| {
        data[at as usize..at as usize + 4].copy_from_slice(&value.to_le_bytes());
    };
    let format = BASE + 0x2000 + FORMAT;
    put(&mut data, DESC, 20);
    put(&mut data, DESC + 4, flags | DSBCAPS_GLOBALFOCUS);
    put(&mut data, DESC + 8, samples.len() as u32);
    put(&mut data, DESC + 16, format);
    // WAVEFORMATEX: PCM, mono, 16 bits.
    put(&mut data, FORMAT, 1 | (1 << 16));
    put(&mut data, FORMAT + 4, rate);
    put(&mut data, FORMAT + 8, rate * 2);
    put(&mut data, FORMAT + 12, 2 | (16 << 16));
    data[IID_NOTIFY as usize..IID_NOTIFY as usize + 16].copy_from_slice(&IID_IDIRECTSOUNDNOTIFY);
    data
}

/// Make the buffer and fill it with the samples, steps 1 to 4.
fn ds_setup(a: &mut CodeAssembler, l: &Layout, fail: CodeLabel, len: u32) -> Result<(), IcedError> {
    // DirectSoundCreate(NULL, &ds, NULL).
    a.push(0)?;
    a.push(l.at(DS) as i32)?;
    a.push(0)?;
    a.call(l.import(OTHER))?;
    check(a, fail, 1)?;
    // SetCooperativeLevel(ds, NULL, DSSCL_NORMAL).
    a.push(1)?;
    a.push(0)?;
    method(a, l.at(DS), DS_SET_COOPERATIVE_LEVEL)?;
    check(a, fail, 2)?;
    // CreateSoundBuffer(ds, &desc, &buf, NULL).
    a.push(0)?;
    a.push(l.at(BUF) as i32)?;
    a.push(l.at(DESC) as i32)?;
    method(a, l.at(DS), DS_CREATE_SOUND_BUFFER)?;
    check(a, fail, 3)?;
    // Lock(buf, 0, len, &p1, &n1, &p2, &n2, 0).
    a.push(0)?;
    a.push(l.at(N2) as i32)?;
    a.push(l.at(P2) as i32)?;
    a.push(l.at(N1) as i32)?;
    a.push(l.at(P1) as i32)?;
    a.push(len as i32)?;
    a.push(0)?;
    method(a, l.at(BUF), DSB_LOCK)?;
    check(a, fail, 4)?;
    a.mov(esi, l.at(SAMPLES) as u32)?;
    a.mov(edi, dword_ptr(l.at(P1)))?;
    a.mov(ecx, dword_ptr(l.at(N1)))?;
    a.cld()?;
    a.rep().movsb()?;
    // Unlock(buf, p1, n1, p2, n2).
    a.push(dword_ptr(l.at(N2)))?;
    a.push(dword_ptr(l.at(P2)))?;
    a.push(dword_ptr(l.at(N1)))?;
    a.push(dword_ptr(l.at(P1)))?;
    method(a, l.at(BUF), DSB_UNLOCK)?;
    check(a, fail, 5)
}

/// Play(buf, 0, 0, DSBPLAY_LOOPING), step 6.
fn ds_play(a: &mut CodeAssembler, l: &Layout, fail: CodeLabel) -> Result<(), IcedError> {
    a.push(1)?;
    a.push(0)?;
    a.push(0)?;
    method(a, l.at(BUF), DSB_PLAY)?;
    check(a, fail, 6)
}

fn exit(a: &mut CodeAssembler, l: &Layout, fail: &mut CodeLabel) -> Result<(), IcedError> {
    a.push(0)?;
    a.call(l.import(EXIT))?;
    a.set_label(fail)?;
    a.push(ebx)?;
    a.call(l.import(EXIT))?;
    a.int3()
}

/// A square wave of 441 Hz at 22050 Hz, a fifth of a second of it, plays
/// in a loop for 600 ms.
fn looping_buffer(core: CoreMode) {
    let samples = square(4410, 50);
    let len = samples.len() as u32;
    let imports: &[(&str, &[&str])] = &[("KERNEL32.dll", KERNEL32), ("DSOUND.dll", DSOUND)];
    let exe = program(imports, &ds_data(&samples, 22050, 0), |a, l| {
        let mut fail = a.create_label();
        ds_setup(a, l, fail, len)?;
        ds_play(a, l, fail)?;
        a.push(600)?;
        a.call(l.import(SLEEP))?;
        exit(a, l, &mut fail)
    });
    let mut cpu = machine("looping", exe, core, &[]);
    let (code, left) = run(&mut cpu, 2000);
    assert_eq!(code, 0, "the program failed at step {code}");
    let (hz, peak) = pitch(&left);
    let heard = left.iter().filter(|&&s| s != 0).count() as f64 / HOST_RATE;
    // The buffer is a fifth of a second: more than that heard means it
    // looped.
    assert!(heard > 0.4, "heard {heard} s");
    assert!(peak > 4000, "the loudest sample is {peak}");
    assert!((hz - 441.0).abs() < 441.0 * 0.02, "the pitch is {hz} Hz");
}

#[test]
fn a_looping_directsound_buffer_is_heard_at_its_pitch() {
    looping_buffer(CoreMode::Normal);
}

#[test]
fn a_looping_directsound_buffer_is_heard_on_the_dynamic_recompiler() {
    looping_buffer(CoreMode::Dynamic);
}

// The streaming program's play cursor, write cursor and where it wrote
// up to.
const PLAY: u32 = 0xA0;
const WRITE: u32 = 0xA4;
const LAST: u32 = 0xA8;

/// A looping buffer of a second of one level, at 22080 Hz as Omikron's
/// speech has it, played on the default 22050 Hz device. Every 15 ms, as
/// a streaming program refills what was played, the program asks for the
/// play cursor and writes the opposite level from where it got to up to
/// the cursor. That is what the buffer plays when it comes round again,
/// a second later: until then, only the first level is heard.
fn refill_behind_the_play_cursor(core: CoreMode) {
    let frames = 22080;
    let samples: Vec<u8> = (0..frames).flat_map(|_| 8000i16.to_le_bytes()).collect();
    let len = samples.len() as u32;
    let imports: &[(&str, &[&str])] = &[("KERNEL32.dll", KERNEL32), ("DSOUND.dll", DSOUND)];
    let exe = program(imports, &ds_data(&samples, frames, 0), |a, l| {
        let mut fail = a.create_label();
        ds_setup(a, l, fail, len)?;
        ds_play(a, l, fail)?;
        // 40 refills, 600 ms, inside the first time round.
        a.mov(dword_ptr(l.at(T0)), 40)?;
        let mut again = a.create_label();
        let mut skip = a.create_label();
        a.set_label(&mut again)?;
        a.push(15)?;
        a.call(l.import(SLEEP))?;
        // GetCurrentPosition(buf, &play, &write).
        a.push(l.at(WRITE) as i32)?;
        a.push(l.at(PLAY) as i32)?;
        method(a, l.at(BUF), DSB_GET_CURRENT_POSITION)?;
        check(a, fail, 7)?;
        a.mov(eax, dword_ptr(l.at(PLAY)))?;
        a.sub(eax, dword_ptr(l.at(LAST)))?;
        a.jbe(skip)?;
        // Lock(buf, last, play - last, &p1, &n1, &p2, &n2, 0).
        a.push(0)?;
        a.push(l.at(N2) as i32)?;
        a.push(l.at(P2) as i32)?;
        a.push(l.at(N1) as i32)?;
        a.push(l.at(P1) as i32)?;
        a.push(eax)?;
        a.push(dword_ptr(l.at(LAST)))?;
        method(a, l.at(BUF), DSB_LOCK)?;
        check(a, fail, 8)?;
        a.mov(edi, dword_ptr(l.at(P1)))?;
        a.mov(ecx, dword_ptr(l.at(N1)))?;
        a.shr(ecx, 1)?;
        a.mov(eax, -8000i32)?;
        a.cld()?;
        a.rep().stosw()?;
        a.push(dword_ptr(l.at(N2)))?;
        a.push(dword_ptr(l.at(P2)))?;
        a.push(dword_ptr(l.at(N1)))?;
        a.push(dword_ptr(l.at(P1)))?;
        method(a, l.at(BUF), DSB_UNLOCK)?;
        check(a, fail, 9)?;
        a.mov(eax, dword_ptr(l.at(PLAY)))?;
        a.mov(dword_ptr(l.at(LAST)), eax)?;
        a.set_label(&mut skip)?;
        a.dec(dword_ptr(l.at(T0)))?;
        a.jnz(again)?;
        exit(a, l, &mut fail)
    });
    let mut cpu = machine("refill", exe, core, &[]);
    let (code, left) = run(&mut cpu, 4000);
    assert_eq!(code, 0, "the program failed at step {code}");
    let start = left
        .iter()
        .position(|&s| s != 0)
        .expect("the buffer was heard");
    let end = left.iter().rposition(|&s| s != 0).unwrap() + 1;
    let heard = &left[start..end];
    assert!(
        heard.len() as f64 / HOST_RATE > 0.5,
        "heard {} frames",
        heard.len()
    );
    // The device's 8 bits take the level down to 7936; the refills, heard
    // early, would take it below that.
    let low: Vec<usize> = (0..heard.len()).filter(|&i| heard[i] < 7900).collect();
    assert!(
        low.is_empty(),
        "{} of {} frames heard the refills a second early, the first at {:?}: {:?}",
        low.len(),
        heard.len(),
        low.first(),
        low.first()
            .map(|&i| &heard[i.saturating_sub(2)..(i + 3).min(heard.len())])
    );
}

#[test]
fn a_refill_behind_the_play_cursor_isnt_heard_before_the_loop() {
    refill_behind_the_play_cursor(CoreMode::Normal);
}

/// A second of 22050 Hz samples, looping, with a notification half way:
/// the program waits for it twice. Its exit code is how many hundredths
/// of a second the first wait took, or 250 if the second didn't take a
/// second.
fn notifications(core: CoreMode) {
    let samples = square(22050, 50);
    let len = samples.len() as u32;
    let mut data = ds_data(&samples, 22050, DSBCAPS_CTRLPOSITIONNOTIFY);
    data[POSITIONS as usize..POSITIONS as usize + 4].copy_from_slice(&(len / 2).to_le_bytes());
    let imports: &[(&str, &[&str])] = &[("KERNEL32.dll", KERNEL32), ("DSOUND.dll", DSOUND)];
    let exe = program(imports, &data, |a, l| {
        let mut fail = a.create_label();
        ds_setup(a, l, fail, len)?;
        // QueryInterface(buf, &IID_IDirectSoundNotify, &notify).
        a.push(l.at(NOTIFY) as i32)?;
        a.push(l.at(IID_NOTIFY) as i32)?;
        method(a, l.at(BUF), DSB_QUERY_INTERFACE)?;
        check(a, fail, 7)?;
        // CreateEventA(NULL, FALSE, FALSE, NULL).
        a.push(0)?;
        a.push(0)?;
        a.push(0)?;
        a.push(0)?;
        a.call(l.import(CREATE_EVENT))?;
        a.mov(dword_ptr(l.at(EVENT)), eax)?;
        a.mov(dword_ptr(l.at(POSITIONS + 4)), eax)?;
        // SetNotificationPositions(notify, 1, positions).
        a.push(l.at(POSITIONS) as i32)?;
        a.push(1)?;
        method(a, l.at(NOTIFY), DSN_SET_NOTIFICATION_POSITIONS)?;
        check(a, fail, 8)?;
        a.call(l.import(TICKS))?;
        a.mov(dword_ptr(l.at(T0)), eax)?;
        ds_play(a, l, fail)?;
        for step in [9, 10] {
            a.push(3000)?;
            a.push(dword_ptr(l.at(EVENT)))?;
            a.call(l.import(WAIT))?;
            check(a, fail, step)?;
            a.call(l.import(TICKS))?;
            a.mov(ecx, eax)?;
            a.sub(eax, dword_ptr(l.at(T0)))?;
            a.mov(dword_ptr(l.at(T0)), ecx)?;
            a.mov(dword_ptr(l.at(T0 + 4 * (step as u32 - 8))), eax)?;
        }
        // The second wait: from 900 to 1100 ms.
        a.mov(ebx, 250)?;
        a.mov(eax, dword_ptr(l.at(T0 + 8)))?;
        a.sub(eax, 900)?;
        a.cmp(eax, 200)?;
        a.ja(fail)?;
        a.mov(eax, dword_ptr(l.at(T0 + 4)))?;
        a.xor(edx, edx)?;
        a.mov(ecx, 10)?;
        a.div(ecx)?;
        a.mov(ebx, eax)?;
        a.jmp(fail)?;
        exit(a, l, &mut fail)
    });
    let mut cpu = machine("notify", exe, core, &[]);
    let (code, _) = run(&mut cpu, 4000);
    // Half a second, to the PIT's 55 ms at worst.
    assert!((45..=56).contains(&code), "exit code {code}");
}

#[test]
fn a_notification_fires_at_its_position() {
    notifications(CoreMode::Normal);
}

#[test]
fn a_notification_fires_on_the_dynamic_recompiler() {
    notifications(CoreMode::Dynamic);
}

const WINMM: &[&str] = &[
    "waveOutOpen",
    "waveOutPrepareHeader",
    "waveOutWrite",
    "waveOutUnprepareHeader",
    "waveOutClose",
];

// The data of the waveOut program.
const HWO: u32 = 0x00;
const DONE: u32 = 0x04;
const DONE_HEADER: u32 = 0x08;
const START: u32 = 0x0C;
const WAVE_FORMAT: u32 = 0x20;
const HEADER: u32 = 0x40;

const WOM_DONE: i32 = 0x3BD;
const CALLBACK_FUNCTION: i32 = 0x3_0000;
const WHDR_DONE: u32 = 1;

/// A quarter of a second of 441 Hz at 22050 Hz, written to waveOut with
/// a callback function, which sees WOM_DONE for the header. The exit
/// code is how many hundredths of a second it took, or a step that
/// failed (from 200).
fn wave_out(core: CoreMode) {
    let samples = square(5512, 50);
    let mut data = vec![0u8; SAMPLES as usize];
    data.extend_from_slice(&samples);
    let put = |data: &mut Vec<u8>, at: u32, value: u32| {
        data[at as usize..at as usize + 4].copy_from_slice(&value.to_le_bytes());
    };
    put(&mut data, WAVE_FORMAT, 1 | (1 << 16));
    put(&mut data, WAVE_FORMAT + 4, 22050);
    put(&mut data, WAVE_FORMAT + 8, 44100);
    put(&mut data, WAVE_FORMAT + 12, 2 | (16 << 16));
    // WAVEHDR: lpData, dwBufferLength, the rest 0.
    put(&mut data, HEADER, BASE + 0x2000 + SAMPLES);
    put(&mut data, HEADER + 4, samples.len() as u32);
    let imports: &[(&str, &[&str])] = &[("KERNEL32.dll", KERNEL32), ("WINMM.dll", WINMM)];
    let exe = program(imports, &data, |a, l| {
        let mut fail = a.create_label();
        let mut callback = a.create_label();
        let mut wait = a.create_label();
        let mut ignore = a.create_label();
        // waveOutOpen(&hwo, WAVE_MAPPER, &format, callback, 0, FUNCTION).
        a.push(CALLBACK_FUNCTION)?;
        a.push(0)?;
        // The callback's address: what the call after it pushes.
        a.call(callback)?;
        // callback(hwo, msg, instance, param1, param2), stdcall.
        a.cmp(dword_ptr(esp + 8), WOM_DONE)?;
        a.jne(ignore)?;
        a.mov(eax, dword_ptr(esp + 16))?;
        a.mov(dword_ptr(l.at(DONE_HEADER)), eax)?;
        a.mov(dword_ptr(l.at(DONE)), 1)?;
        a.set_label(&mut ignore)?;
        a.ret_1(20)?;
        a.set_label(&mut callback)?;
        a.push(l.at(WAVE_FORMAT) as i32)?;
        a.push(-1)?;
        a.push(l.at(HWO) as i32)?;
        a.call(l.import(OTHER))?;
        check(a, fail, 201)?;
        for (import, step) in [(1, 202), (2, 203)] {
            if import == 2 {
                a.call(l.import(TICKS))?;
                a.mov(dword_ptr(l.at(START)), eax)?;
            }
            a.push(32)?;
            a.push(l.at(HEADER) as i32)?;
            a.push(dword_ptr(l.at(HWO)))?;
            a.call(l.import(OTHER + import))?;
            check(a, fail, step)?;
        }
        // Wait for the callback, ten seconds at most.
        a.mov(esi, 1000)?;
        a.set_label(&mut wait)?;
        a.push(10)?;
        a.call(l.import(SLEEP))?;
        a.mov(ebx, 204)?;
        a.dec(esi)?;
        a.jz(fail)?;
        a.cmp(dword_ptr(l.at(DONE)), 0)?;
        a.je(wait)?;
        a.call(l.import(TICKS))?;
        a.sub(eax, dword_ptr(l.at(START)))?;
        a.mov(dword_ptr(l.at(START)), eax)?;
        // The callback's header, done.
        a.mov(ebx, 205)?;
        a.cmp(dword_ptr(l.at(DONE_HEADER)), l.at(HEADER) as i32)?;
        a.jne(fail)?;
        a.mov(ebx, 206)?;
        a.test(dword_ptr(l.at(HEADER + 16)), WHDR_DONE as i32)?;
        a.jz(fail)?;
        a.push(32)?;
        a.push(l.at(HEADER) as i32)?;
        a.push(dword_ptr(l.at(HWO)))?;
        a.call(l.import(OTHER + 3))?;
        check(a, fail, 207)?;
        a.push(dword_ptr(l.at(HWO)))?;
        a.call(l.import(OTHER + 4))?;
        check(a, fail, 208)?;
        a.mov(eax, dword_ptr(l.at(START)))?;
        a.xor(edx, edx)?;
        a.mov(ecx, 10)?;
        a.div(ecx)?;
        a.mov(ebx, eax)?;
        a.jmp(fail)?;
        exit(a, l, &mut fail)
    });
    let mut cpu = machine("wave_out", exe, core, &[]);
    let (code, left) = run(&mut cpu, 4000);
    assert!((24..=32).contains(&code), "exit code {code}");
    let (hz, peak) = pitch(&left);
    assert!(peak > 4000, "the loudest sample is {peak}");
    assert!((hz - 441.0).abs() < 441.0 * 0.03, "the pitch is {hz} Hz");
}

#[test]
fn a_wave_out_program_gets_wom_done() {
    wave_out(CoreMode::Normal);
}

#[test]
fn a_wave_out_program_gets_wom_done_on_the_dynamic_recompiler() {
    wave_out(CoreMode::Dynamic);
}

/// A WAV file of `samples`, 16-bit mono at `rate`.
fn wav_file(samples: &[u8], rate: u32) -> Vec<u8> {
    let mut f = b"RIFF".to_vec();
    f.extend_from_slice(&(36 + samples.len() as u32).to_le_bytes());
    f.extend_from_slice(b"WAVEfmt ");
    f.extend_from_slice(&16u32.to_le_bytes());
    f.extend_from_slice(&1u16.to_le_bytes());
    f.extend_from_slice(&1u16.to_le_bytes());
    f.extend_from_slice(&rate.to_le_bytes());
    f.extend_from_slice(&(rate * 2).to_le_bytes());
    f.extend_from_slice(&2u16.to_le_bytes());
    f.extend_from_slice(&16u16.to_le_bytes());
    f.extend_from_slice(b"data");
    f.extend_from_slice(&(samples.len() as u32).to_le_bytes());
    f.extend_from_slice(samples);
    f
}

/// MCI's waveaudio device plays a WAV file of 441 Hz, half a second of
/// it, with "play ... wait": the exit code is how many hundredths of a
/// second the command took, or 250 if it failed.
fn mci_play(core: CoreMode) {
    const COMMAND: &[u8] = b"play tone.wav wait\0";
    let mut data = vec![0u8; 0x40];
    data.extend_from_slice(COMMAND);
    let imports: &[(&str, &[&str])] = &[
        ("KERNEL32.dll", KERNEL32),
        ("WINMM.dll", &["mciSendStringA"]),
    ];
    let exe = program(imports, &data, |a, l| {
        let mut fail = a.create_label();
        a.call(l.import(TICKS))?;
        a.mov(dword_ptr(l.at(0)), eax)?;
        a.push(0)?;
        a.push(0)?;
        a.push(0)?;
        a.push(l.at(0x40) as i32)?;
        a.call(l.import(OTHER))?;
        check(a, fail, 250)?;
        a.call(l.import(TICKS))?;
        a.sub(eax, dword_ptr(l.at(0)))?;
        a.xor(edx, edx)?;
        a.mov(ecx, 10)?;
        a.div(ecx)?;
        a.mov(ebx, eax)?;
        a.jmp(fail)?;
        exit(a, l, &mut fail)
    });
    let tone = wav_file(&square(11025, 50), 22050);
    let mut cpu = machine("mci_play", exe, core, &[("TONE.WAV", tone)]);
    let (code, left) = run(&mut cpu, 4000);
    assert!((49..=56).contains(&code), "exit code {code}");
    let (hz, peak) = pitch(&left);
    assert!(peak > 4000, "the loudest sample is {peak}");
    assert!((hz - 441.0).abs() < 441.0 * 0.03, "the pitch is {hz} Hz");
}

#[test]
fn mci_plays_a_wav_file() {
    mci_play(CoreMode::Normal);
}

#[test]
fn mci_plays_a_wav_file_on_the_dynamic_recompiler() {
    mci_play(CoreMode::Dynamic);
}

/// The data of `acm_decodes`: a stereo MS ADPCM sound and what it
/// decodes to, both made by ffmpeg 7 (its `adpcm_ms` encoder, 64-byte
/// blocks, and its decoder) from a 440 Hz sine on the left and noise on
/// the right at 22050 Hz. The sound is a whole block and 30 bytes of the
/// next: a partial block at the end, which decodes as far as its bytes
/// go, 18 frames.
const MS_ADPCM_FORMAT: [u8; 50] = [
    0x02, 0x00, 0x02, 0x00, 0x22, 0x56, 0x00, 0x00, 0x02, 0x6A, 0x00, 0x00, 0x40, 0x00, 0x04, 0x00,
    0x20, 0x00, 0x34, 0x00, 0x07, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x02, 0x00, 0xFF, 0x00, 0x00,
    0x00, 0x00, 0xC0, 0x00, 0x40, 0x00, 0xF0, 0x00, 0x00, 0x00, 0xCC, 0x01, 0x30, 0xFF, 0x88, 0x01,
    0x18, 0xFF,
];
const MS_ADPCM_SOUND: [u8; 94] = [
    0x00, 0x00, 0x10, 0x00, 0x10, 0x00, 0xFF, 0x01, 0xE8, 0x19, 0x00, 0x00, 0xBF, 0x07, 0x78, 0x77,
    0x78, 0x48, 0x25, 0x1C, 0x1E, 0x25, 0x1F, 0x10, 0x0C, 0x0F, 0x02, 0xE3, 0xE3, 0xD8, 0xC2, 0xC0,
    0xCE, 0xC2, 0xDE, 0xC1, 0xCE, 0xDF, 0xD4, 0xD0, 0xCF, 0xDE, 0xD6, 0xCD, 0xE0, 0xD3, 0xE1, 0xFC,
    0xEE, 0xF1, 0x01, 0x12, 0x2E, 0x40, 0x4F, 0x46, 0x40, 0x3F, 0x52, 0x30, 0x4B, 0x32, 0x3C, 0x44,
    0x00, 0x00, 0x9A, 0x00, 0x4C, 0x0F, 0xAA, 0x05, 0x8B, 0x0F, 0xBF, 0x03, 0x90, 0xE7, 0x3D, 0x34,
    0x3C, 0x41, 0x22, 0x2D, 0x22, 0x2E, 0x15, 0x00, 0xFD, 0xE4, 0xC0, 0xD0, 0xAF, 0xD1,
];
const MS_ADPCM_DECODED: [i16; 140] = [
    0, 1983, 511, 6632, 623, 6504, 889, 6840, 1526, 5920, 2398, 3160, 2920, 8335, 3154, 1723, 3364,
    -2241, 3740, 6659, 3908, 3816, 4058, 3816, 4058, -5360, 4058, -8111, 4058, -3169, 3866, 3491,
    3694, 9473, 3463, -4855, 3187, 5891, 2859, 5891, 2467, -2781, 1999, 5009, 1579, -1989, 1079,
    1154, 483, -4492, -51, -7028, -528, 2084, -954, 2084, -1462, -369, -1918, -4775, -2326, 7099,
    -2814, -4775, -3106, -4775, -3499, 4807, -3733, 7676, -3838, -2632, -4026, -8812, -4110, -6036,
    -4110, -3542, -4043, 938, -3923, -3086, -3711, -3086, -3459, -4709, -3159, 4039, -2803, 4039,
    -2485, 1420, -2010, 6126, -1557, 6126, -1017, -3369, -534, 2697, -102, -8199, 414, 4865, 959,
    -6256, 1450, 3979, 1912, -7769, 2326, 6303, 2695, -10569, 3135, -5511, 3397, 3577, 3631, -8669,
    3841, -1335, 4029, -7923, 4113, 6872, 4113, 6872, 4046, -5866, 3926, 9390, 3714, 9390, 3525,
    9390, 3189, 5700, 2853, 9015,
];

const MSACM32: &[&str] = &[
    "acmFormatSuggest",
    "acmStreamOpen",
    "acmStreamSize",
    "acmStreamPrepareHeader",
    "acmStreamConvert",
    "acmStreamUnprepareHeader",
    "acmStreamClose",
];

// The data of `acm_decodes`.
const ACM_STREAM: u32 = 0x00;
const ACM_SIZE: u32 = 0x04;
const ACM_PCM: u32 = 0x08;
const ACM_ADPCM: u32 = 0x20;
const ACM_HEADER: u32 = 0x60;
const ACM_SOUND: u32 = 0x100;
const ACM_OUT: u32 = 0x200;
const ACM_DECODED: u32 = 0x400;
/// The destination's room: the PCM of two whole blocks, which
/// acmStreamSize gives for the sound's block and a half and the codec
/// asks for, though the half block decodes to less.
const ACM_ROOM: u32 = 0x1A0;
/// Room for one block's PCM, too little for the sound.
const ACM_SMALL_ROOM: u32 = 0x180;

/// A program decodes `MS_ADPCM_SOUND` through MSACM32 as Oni decodes
/// its sounds: it asks acmFormatSuggest for a PCM format, opens a stream
/// to it, sizes the PCM and converts the sound with a prepared header.
fn acm_decodes(core: CoreMode) {
    let decoded: Vec<u8> = MS_ADPCM_DECODED
        .iter()
        .flat_map(|s| s.to_le_bytes())
        .collect();
    let mut data = vec![0xFFu8; ACM_DECODED as usize + decoded.len()];
    let mut put = |at: u32, bytes: &[u8]| {
        data[at as usize..at as usize + bytes.len()].copy_from_slice(bytes);
    };
    put(ACM_ADPCM, &MS_ADPCM_FORMAT);
    put(ACM_HEADER, &[0; 84]);
    put(ACM_HEADER, &84u32.to_le_bytes());
    put(ACM_SOUND, &MS_ADPCM_SOUND);
    put(ACM_DECODED, &decoded);
    let imports: &[(&str, &[&str])] = &[("KERNEL32.dll", KERNEL32), ("MSACM32.dll", MSACM32)];
    let acm = |n: usize| OTHER + n;
    let exe = program(imports, &data, |a, l| {
        let mut fail = a.create_label();
        let at = |offset: u32| l.at(offset) as i32;
        let field = |offset: u32| dword_ptr(l.at(ACM_HEADER + offset));
        let stream = dword_ptr(l.at(ACM_STREAM));
        // acmFormatSuggest(0, adpcm, pcm, 18, ACM_FORMATSUGGESTF_WFORMATTAG)
        // with the tag PCM.
        a.mov(word_ptr(l.at(ACM_PCM)), 1)?;
        for arg in [0x1_0000, 18, at(ACM_PCM), at(ACM_ADPCM), 0] {
            a.push(arg)?;
        }
        a.call(l.import(acm(0)))?;
        check(a, fail, 1)?;
        // 16-bit stereo PCM at 22050 Hz, with no extra bytes.
        let pcm: [(u32, u32); 4] = [(0, 0x0002_0001), (4, 22050), (8, 88200), (12, 0x0010_0004)];
        for (n, (offset, value)) in pcm.into_iter().enumerate() {
            a.mov(ebx, 20 + n as i32)?;
            a.cmp(dword_ptr(l.at(ACM_PCM + offset)), value as i32)?;
            a.jne(fail)?;
        }
        a.cmp(word_ptr(l.at(ACM_PCM + 16)), 0)?;
        a.jne(fail)?;
        // acmStreamOpen(&stream, 0, adpcm, pcm, 0, 0, 0,
        // ACM_STREAMOPENF_NONREALTIME).
        for arg in [4, 0, 0, 0, at(ACM_PCM), at(ACM_ADPCM), 0, at(ACM_STREAM)] {
            a.push(arg)?;
        }
        a.call(l.import(acm(1)))?;
        check(a, fail, 2)?;
        // The PCM of the sound, its half block counted whole as
        // msadp32.acm counts it, and the ADPCM whose PCM fits the room:
        // the whole blocks only.
        for (step, size, flags, wanted) in [(3, 94, 0, 416), (4, ACM_ROOM as i32, 1, 128)] {
            a.push(flags)?;
            a.push(at(ACM_SIZE))?;
            a.push(size)?;
            a.push(stream)?;
            a.call(l.import(acm(2)))?;
            check(a, fail, step)?;
            a.mov(ebx, 30 + step)?;
            a.cmp(dword_ptr(l.at(ACM_SIZE)), wanted)?;
            a.jne(fail)?;
        }
        a.mov(field(0x0C), at(ACM_SOUND))?;
        a.mov(field(0x10), MS_ADPCM_SOUND.len() as i32)?;
        a.mov(field(0x1C), at(ACM_OUT))?;
        // Prepare; convert into one block's room, which the codec
        // refuses with ACMERR_NOTPOSSIBLE; convert into the room the
        // size asks for, and unprepare.
        for (step, function, room, wanted) in [
            (5, 3, ACM_ROOM, 0),
            (6, 4, ACM_SMALL_ROOM, 512),
            (7, 4, ACM_ROOM, 0),
            (8, 5, ACM_ROOM, 0),
        ] {
            a.mov(field(0x20), room as i32)?;
            a.push(0)?;
            a.push(at(ACM_HEADER))?;
            a.push(stream)?;
            a.call(l.import(acm(function)))?;
            a.mov(ebx, step)?;
            a.cmp(eax, wanted)?;
            a.jne(fail)?;
        }
        // All of the sound used, and the PCM written.
        a.mov(ebx, 40)?;
        a.cmp(field(0x14), MS_ADPCM_SOUND.len() as i32)?;
        a.jne(fail)?;
        a.mov(ebx, 41)?;
        a.cmp(field(0x24), decoded.len() as i32)?;
        a.jne(fail)?;
        // ACMSTREAMHEADER_STATUSF_DONE, and no longer prepared.
        a.mov(ebx, 42)?;
        a.cmp(field(4), 0x1_0000)?;
        a.jne(fail)?;
        a.mov(ebx, 43)?;
        a.mov(esi, at(ACM_OUT))?;
        a.mov(edi, at(ACM_DECODED))?;
        a.mov(ecx, decoded.len() as i32)?;
        a.repe().cmpsb()?;
        a.jne(fail)?;
        // Closed, the stream's handle is no more.
        for (step, wanted) in [(9, 0), (10, 5)] {
            a.push(0)?;
            a.push(stream)?;
            a.call(l.import(acm(6)))?;
            a.mov(ebx, step)?;
            a.cmp(eax, wanted)?;
            a.jne(fail)?;
        }
        exit(a, l, &mut fail)
    });
    let mut cpu = machine("acm_decodes", exe, core, &[]);
    let (code, _) = run(&mut cpu, 200);
    assert_eq!(code, 0, "the program failed at step {code}");
}

#[test]
fn msacm32_decodes_ms_adpcm_as_ffmpeg_does() {
    acm_decodes(CoreMode::Normal);
}

// The data of `acm_converts_pcm`.
const PCM_STREAM: u32 = 0x00;
const PCM_SIZE: u32 = 0x04;
const PCM_TO: u32 = 0x08;
const PCM_FROM: u32 = 0x20;
const PCM_24_BITS: u32 = 0x40;
const PCM_HEADER: u32 = 0x60;
const PCM_SOUND: u32 = 0x100;
const PCM_OUT: u32 = 0x200;
const PCM_CONVERTED: u32 = 0x300;

/// 16-bit stereo PCM at 22050 Hz: 8 frames.
fn pcm_sound() -> Vec<[i16; 2]> {
    (0..8i16)
        .map(|i| [1000 * i, 500 * i - 2000 + 30000 * (i / 7)])
        .collect()
}

/// A program converts 16-bit stereo PCM at 22050 Hz to 8-bit mono at
/// 11025 Hz through MSACM32's PCM converter: acmFormatSuggest suggests
/// the PCM the flags ask for, and refuses 3 channels, and acmStreamOpen
/// refuses 24 bits as acmFormatSuggest does. The stream sizes and
/// converts as Wine's converter does: every other frame is dropped, and
/// the two channels' sum, clipped, becomes the sample's high byte.
fn acm_converts_pcm(core: CoreMode) {
    let sound = pcm_sound();
    let source: Vec<u8> = sound.iter().flatten().flat_map(|v| v.to_le_bytes()).collect();
    let converted: Vec<u8> = sound
        .iter()
        .skip(1)
        .step_by(2)
        .map(|&[l, r]| {
            let sum = (i32::from(l) + i32::from(r)).clamp(-32768, 32767);
            ((sum >> 8) as u8) ^ 0x80
        })
        .collect();
    let mut data = vec![0xFFu8; PCM_CONVERTED as usize + converted.len()];
    let mut put = |at: u32, bytes: &[u8]| {
        data[at as usize..at as usize + bytes.len()].copy_from_slice(bytes);
    };
    let wave_format = |channels: u16, rate: u32, bits: u16| {
        let align = channels * bits / 8;
        let mut f = Vec::new();
        f.extend_from_slice(&1u16.to_le_bytes());
        f.extend_from_slice(&channels.to_le_bytes());
        f.extend_from_slice(&rate.to_le_bytes());
        f.extend_from_slice(&(rate * u32::from(align)).to_le_bytes());
        f.extend_from_slice(&align.to_le_bytes());
        f.extend_from_slice(&bits.to_le_bytes());
        f.extend_from_slice(&0u16.to_le_bytes());
        f
    };
    put(PCM_FROM, &wave_format(2, 22050, 16));
    put(PCM_TO, &wave_format(3, 11025, 8));
    put(PCM_24_BITS, &wave_format(1, 11025, 24));
    put(PCM_HEADER, &[0; 84]);
    put(PCM_HEADER, &84u32.to_le_bytes());
    put(PCM_SOUND, &source);
    put(PCM_CONVERTED, &converted);
    let imports: &[(&str, &[&str])] = &[("KERNEL32.dll", KERNEL32), ("MSACM32.dll", MSACM32)];
    let acm = |n: usize| OTHER + n;
    let exe = program(imports, &data, |a, l| {
        let mut fail = a.create_label();
        let at = |offset: u32| l.at(offset) as i32;
        let field = |offset: u32| dword_ptr(l.at(PCM_HEADER + offset));
        let stream = dword_ptr(l.at(PCM_STREAM));
        // acmFormatSuggest(0, from, to, 18, ACM_FORMATSUGGESTF_NCHANNELS |
        // NSAMPLESPERSEC | WBITSPERSAMPLE): 3 channels are refused with
        // ACMERR_NOTPOSSIBLE, 1 is suggested.
        for (step, channels, wanted) in [(1, 3, 512), (2, 1, 0)] {
            a.mov(word_ptr(l.at(PCM_TO + 2)), channels)?;
            for arg in [0xE_0000, 18, at(PCM_TO), at(PCM_FROM), 0] {
                a.push(arg)?;
            }
            a.call(l.import(acm(0)))?;
            a.mov(ebx, step)?;
            a.cmp(eax, wanted)?;
            a.jne(fail)?;
        }
        // 8-bit mono PCM at 11025 Hz.
        let pcm: [(u32, u32); 4] = [(0, 0x0001_0001), (4, 11025), (8, 11025), (12, 0x0008_0001)];
        for (n, (offset, value)) in pcm.into_iter().enumerate() {
            a.mov(ebx, 20 + n as i32)?;
            a.cmp(dword_ptr(l.at(PCM_TO + offset)), value as i32)?;
            a.jne(fail)?;
        }
        // acmStreamOpen(0, 0, from, 24 bits, 0, 0, 0, ACM_STREAMOPENF_QUERY)
        // fails as the suggestion would, and the stream to 8 bits opens.
        for (step, out, to, flags, wanted) in [
            (3, 0, at(PCM_24_BITS), 1, 512),
            (4, at(PCM_STREAM), at(PCM_TO), 0, 0),
        ] {
            for arg in [flags, 0, 0, 0, to, at(PCM_FROM), 0, out] {
                a.push(arg)?;
            }
            a.call(l.import(acm(1)))?;
            a.mov(ebx, step)?;
            a.cmp(eax, wanted)?;
            a.jne(fail)?;
        }
        // The bytes the sound converts to, and back.
        let bytes = source.len() as i32;
        let out = converted.len() as i32;
        for (step, size, flags, wanted) in [(5, bytes, 0, out), (6, out, 1, bytes)] {
            a.push(flags)?;
            a.push(at(PCM_SIZE))?;
            a.push(size)?;
            a.push(stream)?;
            a.call(l.import(acm(2)))?;
            check(a, fail, step)?;
            a.mov(ebx, 30 + step)?;
            a.cmp(dword_ptr(l.at(PCM_SIZE)), wanted)?;
            a.jne(fail)?;
        }
        a.mov(field(0x0C), at(PCM_SOUND))?;
        a.mov(field(0x10), bytes)?;
        a.mov(field(0x1C), at(PCM_OUT))?;
        a.mov(field(0x20), out)?;
        // Prepare, convert and unprepare.
        for (step, function) in [(7, 3), (8, 4), (9, 5)] {
            a.push(0)?;
            a.push(at(PCM_HEADER))?;
            a.push(stream)?;
            a.call(l.import(acm(function)))?;
            check(a, fail, step)?;
        }
        // All of the sound used, and the PCM written.
        a.mov(ebx, 40)?;
        a.cmp(field(0x14), bytes)?;
        a.jne(fail)?;
        a.mov(ebx, 41)?;
        a.cmp(field(0x24), out)?;
        a.jne(fail)?;
        a.mov(ebx, 42)?;
        a.mov(esi, at(PCM_OUT))?;
        a.mov(edi, at(PCM_CONVERTED))?;
        a.mov(ecx, out)?;
        a.repe().cmpsb()?;
        a.jne(fail)?;
        a.push(0)?;
        a.push(stream)?;
        a.call(l.import(acm(6)))?;
        check(a, fail, 10)?;
        exit(a, l, &mut fail)
    });
    let mut cpu = machine("acm_converts_pcm", exe, core, &[]);
    let (code, _) = run(&mut cpu, 200);
    assert_eq!(code, 0, "the program failed at step {code}");
}

#[test]
fn msacm32_converts_pcm_as_wine_does() {
    acm_converts_pcm(CoreMode::Normal);
}

/// The format tag of the test's codec: Windows Media Audio's.
const TEST_TAG: u16 = 0x161;
/// The id the test's codec gives its stream in ACMDRVSTREAMINSTANCE's
/// dwDriver, which it checks when it converts.
const TEST_STREAM_ID: i32 = 0x5678;

/// The code of `acm_driver_dll`: DriverProc at its start, DllMain at
/// `main`.
fn acm_driver_code(a: &mut CodeAssembler, main: &mut CodeLabel) -> Result<(), IcedError> {
    let mut one = a.create_label();
    let mut zero = a.create_label();
    let mut not_possible = a.create_label();
    let mut not_supported = a.create_label();
    let mut open = a.create_label();
    let mut details = a.create_label();
    let mut suggest = a.create_label();
    let mut stream_open = a.create_label();
    let mut size = a.create_label();
    let mut convert = a.create_label();
    // DriverProc(id, driver, message, p1, p2).
    let p1 = dword_ptr(esp + 16);
    let p2 = dword_ptr(esp + 20);
    a.mov(eax, dword_ptr(esp + 12))?;
    for (message, to) in [
        (3, open),
        (0x600A, details),
        (0x601B, suggest),
        (0x604C, stream_open),
        (0x604D, zero),
        (0x604E, size),
        (0x604F, convert),
    ] {
        a.cmp(eax, message)?;
        a.je(to)?;
    }
    // DRV_LOAD, DRV_ENABLE, DRV_CLOSE, DRV_DISABLE and DRV_FREE succeed;
    // the rest, preparing headers among them, it leaves to the ACM.
    a.cmp(eax, 6)?;
    a.jbe(one)?;
    a.jmp(not_supported)?;
    a.set_label(&mut one)?;
    a.mov(eax, 1)?;
    a.ret_1(20)?;
    a.set_label(&mut zero)?;
    a.xor(eax, eax)?;
    a.ret_1(20)?;
    a.set_label(&mut not_possible)?;
    a.mov(eax, 512)?;
    a.ret_1(20)?;
    a.set_label(&mut not_supported)?;
    a.mov(eax, 8)?;
    a.ret_1(20)?;
    // DRV_OPEN: an audio codec's open, or one with no description.
    a.set_label(&mut open)?;
    a.mov(ecx, p2)?;
    a.test(ecx, ecx)?;
    a.jz(one)?;
    a.cmp(dword_ptr(ecx + 4), u32::from_le_bytes(*b"audc") as i32)?;
    a.jne(zero)?;
    a.mov(eax, 0x1234)?;
    a.ret_1(20)?;
    // ACMDM_DRIVER_DETAILS: a codec with two format tags, "Test".
    a.set_label(&mut details)?;
    a.mov(ecx, p1)?;
    a.mov(dword_ptr(ecx + 0x18), 1)?;
    a.mov(dword_ptr(ecx + 0x1C), 2)?;
    a.mov(dword_ptr(ecx + 40), 0x0065_0054)?;
    a.mov(dword_ptr(ecx + 44), 0x0074_0073)?;
    a.mov(word_ptr(ecx + 48), 0)?;
    a.jmp(zero)?;
    // ACMDM_FORMAT_SUGGEST: 16-bit mono PCM at the source's rate.
    a.set_label(&mut suggest)?;
    a.mov(ecx, p1)?;
    a.mov(edx, dword_ptr(ecx + 8))?;
    a.cmp(word_ptr(edx), TEST_TAG as i32)?;
    a.jne(not_possible)?;
    a.mov(ecx, dword_ptr(ecx + 16))?;
    a.mov(dword_ptr(ecx), 0x0001_0001)?;
    a.mov(eax, dword_ptr(edx + 4))?;
    a.mov(dword_ptr(ecx + 4), eax)?;
    a.add(eax, eax)?;
    a.mov(dword_ptr(ecx + 8), eax)?;
    a.mov(dword_ptr(ecx + 12), 0x0010_0002)?;
    a.jmp(zero)?;
    // ACMDM_STREAM_OPEN: from TEST_TAG to PCM.
    a.set_label(&mut stream_open)?;
    a.mov(ecx, p1)?;
    a.mov(edx, dword_ptr(ecx + 4))?;
    a.cmp(word_ptr(edx), TEST_TAG as i32)?;
    a.jne(not_possible)?;
    a.mov(edx, dword_ptr(ecx + 8))?;
    a.cmp(word_ptr(edx), 1)?;
    a.jne(not_possible)?;
    a.mov(dword_ptr(ecx + 32), TEST_STREAM_ID)?;
    a.jmp(zero)?;
    // ACMDM_STREAM_SIZE: the other side of ACMDRVSTREAMSIZE.
    a.set_label(&mut size)?;
    a.mov(ecx, p2)?;
    let mut destination = a.create_label();
    a.test(dword_ptr(ecx + 4), 1)?;
    a.jnz(destination)?;
    a.mov(eax, dword_ptr(ecx + 8))?;
    a.add(eax, eax)?;
    a.mov(dword_ptr(ecx + 12), eax)?;
    a.jmp(zero)?;
    a.set_label(&mut destination)?;
    a.mov(eax, dword_ptr(ecx + 12))?;
    a.shr(eax, 1)?;
    a.mov(dword_ptr(ecx + 8), eax)?;
    a.jmp(zero)?;
    // ACMDM_STREAM_CONVERT of the stream it opened: each byte of the
    // source a sample of that byte twice.
    a.set_label(&mut convert)?;
    a.mov(eax, p1)?;
    a.cmp(dword_ptr(eax + 32), TEST_STREAM_ID)?;
    a.jne(not_possible)?;
    a.push(esi)?;
    a.push(edi)?;
    a.push(ebx)?;
    a.mov(ecx, dword_ptr(esp + 32))?;
    a.mov(esi, dword_ptr(ecx + 0x0C))?;
    a.mov(ebx, dword_ptr(ecx + 0x10))?;
    a.mov(edi, dword_ptr(ecx + 0x1C))?;
    a.mov(dword_ptr(ecx + 0x14), ebx)?;
    a.lea(eax, ptr(ebx + ebx))?;
    a.mov(dword_ptr(ecx + 0x24), eax)?;
    let mut byte = a.create_label();
    let mut converted = a.create_label();
    a.test(ebx, ebx)?;
    a.jz(converted)?;
    a.set_label(&mut byte)?;
    a.mov(al, byte_ptr(esi))?;
    a.mov(ah, al)?;
    a.mov(word_ptr(edi), ax)?;
    a.inc(esi)?;
    a.add(edi, 2)?;
    a.dec(ebx)?;
    a.jnz(byte)?;
    a.set_label(&mut converted)?;
    a.pop(ebx)?;
    a.pop(edi)?;
    a.pop(esi)?;
    a.jmp(zero)?;
    // DllMain.
    a.set_label(main)?;
    a.mov(eax, 1)?;
    a.ret_1(12)
}

/// An installable ACM driver, TESTACM.ACM, as msaud32.acm is one: a DLL
/// whose DriverProc opens for an audio codec's ACMDRVOPENDESC, says it
/// is a codec named "Test", suggests 16-bit PCM of the rate of a source
/// of TEST_TAG, opens a stream from it to PCM, sizes it at two bytes of
/// PCM for each of the source, and converts each byte to a sample of
/// that byte twice. It leaves preparing headers to the ACM.
fn acm_driver_dll() -> Vec<u8> {
    const DLL_BASE: u32 = 0x1000_0000;
    let mut b = Builder::dll(DLL_BASE, "TESTACM.ACM");
    let code_rva = b.next_rva();
    let ip = (DLL_BASE + code_rva) as u64;
    let mut a = CodeAssembler::new(32).unwrap();
    let mut main = a.create_label();
    acm_driver_code(&mut a, &mut main).unwrap();
    let assembled = a
        .assemble_options(ip, iced_x86::BlockEncoderOptions::RETURN_NEW_INSTRUCTION_OFFSETS)
        .unwrap();
    let main = (assembled.label_ip(&main).unwrap() - ip) as u32;
    let code = assembled.inner.code_buffer;
    let flags = pe::SCN_CODE | pe::SCN_EXECUTE | pe::SCN_READ;
    assert_eq!(b.section(".text", &code, flags), code_rva);
    b.export(Some("DriverProc"), 1, code_rva);
    b.entry(code_rva + main);
    b.build()
}

const MSACM32_DRIVERS: &[&str] = &[
    "acmFormatSuggest",
    "acmStreamOpen",
    "acmStreamSize",
    "acmStreamPrepareHeader",
    "acmStreamConvert",
    "acmStreamUnprepareHeader",
    "acmStreamClose",
    "acmDriverEnum",
    "acmDriverDetailsA",
];

// The data of `acm_driver`.
const DRV_STREAM: u32 = 0x00;
const DRV_SIZE: u32 = 0x04;
const DRV_COUNT: u32 = 0x08;
const DRV_ID: u32 = 0x0C;
const DRV_SUPPORT: u32 = 0x10;
const DRV_SOURCE: u32 = 0x20;
const DRV_PCM: u32 = 0x40;
const DRV_HEADER: u32 = 0x60;
const DRV_SOUND: u32 = 0x100;
const DRV_OUT: u32 = 0x140;
const DRV_DECODED: u32 = 0x160;
const DRV_DETAILS: u32 = 0x200;
const DRV_END: u32 = DRV_DETAILS + 920;

/// A program decodes a sound of TEST_TAG through MSACM32 with an
/// installed driver, as Demolition Racer's wmaudsdk.dll decodes its
/// music through msaud32.acm: acmFormatSuggest with no driver finds the
/// PCM the driver suggests, acmStreamOpen with no driver opens a stream
/// of the driver, which sizes and converts the sound. acmDriverEnum then
/// lists the driver as a codec, and acmDriverDetailsA gives its name.
/// The driver is in SYSTEM.INI's [drivers32], or with `registry` under
/// the registry's MediaResources\ACM key.
fn acm_driver(core: CoreMode, registry: bool) {
    let sound: [u8; 8] = [0x00, 0x11, 0x22, 0x80, 0x7F, 0xFF, 0x01, 0x10];
    let decoded: Vec<u8> = sound.iter().flat_map(|&b| [b, b]).collect();
    let mut data = vec![0u8; DRV_END as usize];
    let mut put = |at: u32, bytes: &[u8]| {
        data[at as usize..at as usize + bytes.len()].copy_from_slice(bytes);
    };
    // Mono at 22050 Hz, 64-byte blocks, no extra bytes.
    let mut source = Vec::new();
    for word in [TEST_TAG as u32 | 1 << 16, 22050, 2000, 64] {
        source.extend_from_slice(&word.to_le_bytes());
    }
    source.extend_from_slice(&[0, 0]);
    put(DRV_SOURCE, &source);
    put(DRV_HEADER, &84u32.to_le_bytes());
    put(DRV_SOUND, &sound);
    put(DRV_DECODED, &decoded);
    put(DRV_DETAILS, &920u32.to_le_bytes());
    let imports: &[(&str, &[&str])] = &[
        ("KERNEL32.dll", KERNEL32),
        ("MSACM32.dll", MSACM32_DRIVERS),
    ];
    let acm = |n: usize| OTHER + n;
    let exe = program(imports, &data, |a, l| {
        let mut fail = a.create_label();
        let at = |offset: u32| l.at(offset) as i32;
        let field = |offset: u32| dword_ptr(l.at(DRV_HEADER + offset));
        let stream = dword_ptr(l.at(DRV_STREAM));
        // acmFormatSuggest(0, source, pcm, 18, ACM_FORMATSUGGESTF_WFORMATTAG)
        // with the tag PCM.
        a.mov(word_ptr(l.at(DRV_PCM)), 1)?;
        for arg in [0x1_0000, 18, at(DRV_PCM), at(DRV_SOURCE), 0] {
            a.push(arg)?;
        }
        a.call(l.import(acm(0)))?;
        check(a, fail, 1)?;
        // The driver's 16-bit mono PCM at the source's rate.
        let pcm: [(u32, u32); 4] = [(0, 0x0001_0001), (4, 22050), (8, 44100), (12, 0x0010_0002)];
        for (n, (offset, value)) in pcm.into_iter().enumerate() {
            a.mov(ebx, 20 + n as i32)?;
            a.cmp(dword_ptr(l.at(DRV_PCM + offset)), value as i32)?;
            a.jne(fail)?;
        }
        // acmStreamOpen(&stream, 0, source, pcm, 0, 0, 0, 0).
        for arg in [0, 0, 0, 0, at(DRV_PCM), at(DRV_SOURCE), 0, at(DRV_STREAM)] {
            a.push(arg)?;
        }
        a.call(l.import(acm(1)))?;
        check(a, fail, 2)?;
        // The driver sizes the PCM of the sound.
        a.push(0)?;
        a.push(at(DRV_SIZE))?;
        a.push(sound.len() as i32)?;
        a.push(stream)?;
        a.call(l.import(acm(2)))?;
        check(a, fail, 3)?;
        a.mov(ebx, 30)?;
        a.cmp(dword_ptr(l.at(DRV_SIZE)), decoded.len() as i32)?;
        a.jne(fail)?;
        a.mov(field(0x0C), at(DRV_SOUND))?;
        a.mov(field(0x10), sound.len() as i32)?;
        a.mov(field(0x1C), at(DRV_OUT))?;
        a.mov(field(0x20), decoded.len() as i32)?;
        // Prepare, convert and unprepare.
        for (step, function) in [(4, 3), (5, 4), (6, 5)] {
            a.push(0)?;
            a.push(at(DRV_HEADER))?;
            a.push(stream)?;
            a.call(l.import(acm(function)))?;
            check(a, fail, step)?;
        }
        // All of the sound used and its PCM written; done, and no
        // longer prepared.
        a.mov(ebx, 40)?;
        a.cmp(field(0x14), sound.len() as i32)?;
        a.jne(fail)?;
        a.mov(ebx, 41)?;
        a.cmp(field(0x24), decoded.len() as i32)?;
        a.jne(fail)?;
        a.mov(ebx, 42)?;
        a.cmp(field(4), 0x1_0000)?;
        a.jne(fail)?;
        a.mov(ebx, 43)?;
        a.mov(esi, at(DRV_OUT))?;
        a.mov(edi, at(DRV_DECODED))?;
        a.mov(ecx, decoded.len() as i32)?;
        a.repe().cmpsb()?;
        a.jne(fail)?;
        a.push(0)?;
        a.push(stream)?;
        a.call(l.import(acm(6)))?;
        check(a, fail, 7)?;
        // acmDriverEnum(callback, 0, 0), the callback's address pushed
        // by the call over it. It counts the drivers and keeps the last
        // one's handle and what it supports.
        let mut after = a.create_label();
        a.call(after)?;
        a.inc(dword_ptr(l.at(DRV_COUNT)))?;
        a.mov(eax, dword_ptr(esp + 4))?;
        a.mov(dword_ptr(l.at(DRV_ID)), eax)?;
        a.mov(eax, dword_ptr(esp + 12))?;
        a.mov(dword_ptr(l.at(DRV_SUPPORT)), eax)?;
        a.mov(eax, 1)?;
        a.ret_1(12)?;
        a.set_label(&mut after)?;
        a.pop(eax)?;
        a.push(0)?;
        a.push(0)?;
        a.push(eax)?;
        a.call(l.import(acm(7)))?;
        check(a, fail, 8)?;
        // One driver, a codec (ACMDRIVERDETAILS_SUPPORTF_CODEC).
        a.mov(ebx, 50)?;
        a.cmp(dword_ptr(l.at(DRV_COUNT)), 1)?;
        a.jne(fail)?;
        a.mov(ebx, 51)?;
        a.cmp(dword_ptr(l.at(DRV_SUPPORT)), 1)?;
        a.jne(fail)?;
        // acmDriverDetailsA(driver, details, 0): its short name in ANSI.
        a.push(0)?;
        a.push(at(DRV_DETAILS))?;
        a.push(dword_ptr(l.at(DRV_ID)))?;
        a.call(l.import(acm(8)))?;
        check(a, fail, 9)?;
        a.mov(ebx, 60)?;
        let name = u32::from_le_bytes(*b"Test") as i32;
        a.cmp(dword_ptr(l.at(DRV_DETAILS + 40)), name)?;
        a.jne(fail)?;
        a.cmp(byte_ptr(l.at(DRV_DETAILS + 44)), 0)?;
        a.jne(fail)?;
        a.mov(ebx, 61)?;
        a.cmp(dword_ptr(l.at(DRV_DETAILS + 0x18)), 1)?;
        a.jne(fail)?;
        exit(a, l, &mut fail)
    });
    let mut files = vec![("TESTACM.ACM", acm_driver_dll())];
    if registry {
        let key = r"HKEY_LOCAL_MACHINE\System\CurrentControlSet\Control\MediaResources\ACM";
        let reg = format!(
            "REGEDIT4\r\n\r\n[{key}\\msacm.test]\r\n\"Driver\"=\"testacm.acm\"\r\n"
        );
        files.push(("REGISTRY.REG", reg.into_bytes()));
    } else {
        let ini = "[drivers32]\r\nmsacm.test=testacm.acm\r\n";
        files.push(("WINDOWS/SYSTEM.INI", ini.as_bytes().to_vec()));
    }
    let name = if registry { "acm_driver_reg" } else { "acm_driver_ini" };
    let mut cpu = machine(name, exe, core, &files);
    let (code, _) = run(&mut cpu, 400);
    assert_eq!(code, 0, "the program failed at step {code}");
}

#[test]
fn msacm32_decodes_through_a_driver_of_system_ini() {
    acm_driver(CoreMode::Normal, false);
}

#[test]
fn msacm32_decodes_through_a_driver_of_the_registry() {
    acm_driver(CoreMode::Normal, true);
}
