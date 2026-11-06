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
/// loaded from the shell as T.EXE, with `files` beside it.
fn machine(test: &str, program: Vec<u8>, core: CoreMode, files: &[(&str, Vec<u8>)]) -> Cpu {
    // Each core its own directory, as the tests run at once.
    let base = PathBuf::from("target/test_win32_audio").join(format!("{test}_{core:?}"));
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    fs::write(base.join("T.EXE"), program).unwrap();
    for (name, bytes) in files {
        fs::write(base.join(name), bytes).unwrap();
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
