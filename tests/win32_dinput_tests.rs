//! The built-in DirectInput: Win32 programs made here with rust-dos-pe's
//! builder acquire the keyboard, the mouse and a joystick, and read what
//! the emulated 8042 keyboard, PS/2 mouse and game port did as device
//! state and buffered data.
//!
//! Each program sets its devices up and says so in its data (`READY` 1),
//! then sleeps until the test, having pressed keys or moved the mouse,
//! says go (`GO` 1). It reads the device's state and buffered data,
//! says it has (`READY` 2), and ends when the test lets it (`GO` 2).
//!
//! With `DINPUT_LOG` set, each test's machine logs to `trace.log` in its
//! directory under `target/test_win32_dinput`.

use iced_x86::code_asm::*;
use rust_dos::cpu::{Cpu, HostConsole};
use rust_dos::exec::{NoHook, StopReason, run_batch};
use rust_dos::joystick::{JoystickSettings, JoystickType, PAD_A, PadState};
use rust_dos_pe::{self as pe, Builder, Imp};
use std::fs;
use std::path::PathBuf;

const BASE: u32 = 0x40_0000;

const DISCL_NONEXCLUSIVE: u32 = 2;
const DISCL_BACKGROUND: u32 = 8;
const DIDFT_AXIS: u32 = 0x03;
const DIDFT_BUTTON: u32 = 0x0C;
const DIDFT_ANYINSTANCE: u32 = 0x00FF_FF00;
const DIDFT_OPTIONAL: u32 = 0x8000_0000;
const DIDF_RELAXIS: u32 = 2;
const DIDF_ABSAXIS: u32 = 1;
const DIPROP_BUFFERSIZE: u32 = 1;
const DIPROP_RANGE: u32 = 4;

/// IDirectInputA's and IDirectInputDeviceA's methods, by their place in
/// the vtables.
const CREATE_DEVICE: u32 = 3;
const SET_PROPERTY: u32 = 6;
const ACQUIRE: u32 = 7;
const GET_DEVICE_STATE: u32 = 9;
const GET_DEVICE_DATA: u32 = 10;
const SET_DATA_FORMAT: u32 = 11;
const SET_COOPERATIVE_LEVEL: u32 = 13;

/// The data's layout: the flags, the interfaces, the results of the
/// calls, the state and the buffered data read.
const READY: u32 = 0;
const GO: u32 = 4;
const DI: u32 = 8;
const DEVICE: u32 = 12;
const RESULTS: u32 = 16;
const COUNT: u32 = 64;
const STATE: u32 = 0x100;
const EVENTS: u32 = 0x200;
/// Room for this many DIDEVICEOBJECTDATA.
const EVENT_ROOM: u32 = 16;
/// Where the rest (GUIDs, the data format, properties) goes.
const REST: u32 = 0x400;

/// The data a program starts with, and the places in it that hold the
/// address of another place, which the program's code fills in, as its
/// base is only known once the image is built.
struct Data {
    bytes: Vec<u8>,
    pointers: Vec<(u32, u32)>,
}

impl Data {
    fn new() -> Data {
        Data {
            bytes: vec![0; REST as usize],
            pointers: Vec::new(),
        }
    }

    /// Add `bytes` on a four-byte boundary: their offset.
    fn add(&mut self, bytes: &[u8]) -> u32 {
        self.bytes.resize(self.bytes.len().next_multiple_of(4), 0);
        let at = self.bytes.len() as u32;
        self.bytes.extend_from_slice(bytes);
        at
    }

    /// Add one of DirectInput's GUIDs, which all end the same way.
    fn guid(&mut self, d1: u32, d2: u16, d3: u16) -> u32 {
        let mut g = d1.to_le_bytes().to_vec();
        g.extend(d2.to_le_bytes());
        g.extend(d3.to_le_bytes());
        g.extend([0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0, 0]);
        self.add(&g)
    }

    /// A DIDATAFORMAT with `objects` (GUID's offset or None, data offset,
    /// type), `flags` and `size` bytes of data.
    fn format(&mut self, flags: u32, size: u32, objects: &[(Option<u32>, u32, u32)]) -> u32 {
        let mut list = Vec::new();
        for &(_, ofs, ty) in objects {
            list.extend([0u32, ofs, ty, 0].iter().flat_map(|v| v.to_le_bytes()));
        }
        let list_at = self.add(&list);
        for (i, &(guid, _, _)) in objects.iter().enumerate() {
            if let Some(guid) = guid {
                self.pointers.push((list_at + 16 * i as u32, guid));
            }
        }
        let header = [24, 16, flags, size, objects.len() as u32, 0];
        let at = self.add(&words(&header));
        self.pointers.push((at + 20, list_at));
        at
    }
}

fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// What a device method is called with.
enum Arg {
    /// A value.
    I(u32),
    /// The address of a place in the data.
    At(u32),
}

/// What a program does with its device: which one, how it reads it, and
/// the properties it sets before acquiring it.
struct Setup {
    guid: u32,
    format: u32,
    state_size: u32,
    properties: Vec<(u32, u32)>,
}

/// The program: DirectInputCreateA, CreateDevice, SetDataFormat,
/// SetCooperativeLevel (background, nonexclusive, without a window),
/// the properties, Acquire, then the waits and the reads described at
/// the top. Each call's result goes to `RESULTS` in turn. The image,
/// and its data's address.
fn program(data: &Data, setup: &Setup) -> (Vec<u8>, u32) {
    let kernel32: &[&str] = &["GetModuleHandleA", "Sleep", "ExitProcess"];
    let dinput: &[&str] = &["DirectInputCreateA"];
    let mut b = Builder::exe(BASE);
    let lists: Vec<Imp> = kernel32.iter().map(|n| Imp::Name(n)).collect();
    let dlists: Vec<Imp> = dinput.iter().map(|n| Imp::Name(n)).collect();
    let iat = b
        .imports(&[("KERNEL32.dll", &lists), ("DINPUT.dll", &dlists)])
        .concat();
    let data_rva = b.section(
        ".data",
        &data.bytes,
        pe::SCN_INITIALIZED_DATA | pe::SCN_READ | pe::SCN_WRITE,
    );
    let base = BASE + data_rva;
    let at = |offset: u32| (base + offset) as u64;
    let code_rva = b.next_rva();
    let mut a = CodeAssembler::new(32).unwrap();
    let mut results = 0;
    (|| -> Result<(), IcedError> {
        for &(field, target) in &data.pointers {
            a.mov(dword_ptr(at(field)), (base + target) as i32)?;
        }
        let mut result = |a: &mut CodeAssembler| {
            let slot = RESULTS + 4 * results;
            results += 1;
            a.mov(dword_ptr(at(slot)), eax)
        };
        a.push(0)?;
        a.call(dword_ptr(iat[0] as u64))?;
        a.push(0)?;
        a.push(at(DI) as i32)?;
        a.push(0x500)?;
        a.push(eax)?;
        a.call(dword_ptr(iat[3] as u64))?;
        result(&mut a)?;
        let method = |a: &mut CodeAssembler, object: u32, index: u32, args: &[Arg]| {
            for arg in args.iter().rev() {
                match *arg {
                    Arg::I(v) => a.push(v as i32)?,
                    Arg::At(offset) => a.push(at(offset) as i32)?,
                }
            }
            a.mov(eax, dword_ptr(at(object)))?;
            a.push(eax)?;
            a.mov(ecx, dword_ptr(eax))?;
            a.call(dword_ptr(ecx + 4 * index))
        };
        let args = [Arg::At(setup.guid), Arg::At(DEVICE), Arg::I(0)];
        method(&mut a, DI, CREATE_DEVICE, &args)?;
        result(&mut a)?;
        method(&mut a, DEVICE, SET_DATA_FORMAT, &[Arg::At(setup.format)])?;
        result(&mut a)?;
        let coop = DISCL_NONEXCLUSIVE | DISCL_BACKGROUND;
        method(
            &mut a,
            DEVICE,
            SET_COOPERATIVE_LEVEL,
            &[Arg::I(0), Arg::I(coop)],
        )?;
        result(&mut a)?;
        for &(property, value) in &setup.properties {
            method(
                &mut a,
                DEVICE,
                SET_PROPERTY,
                &[Arg::I(property), Arg::At(value)],
            )?;
            result(&mut a)?;
        }
        method(&mut a, DEVICE, ACQUIRE, &[])?;
        result(&mut a)?;
        let wait = |a: &mut CodeAssembler, ready: u32, go: u32| {
            a.mov(dword_ptr(at(READY)), ready)?;
            let mut again = a.create_label();
            a.set_label(&mut again)?;
            a.push(10)?;
            a.call(dword_ptr(iat[1] as u64))?;
            a.cmp(dword_ptr(at(GO)), go)?;
            a.jne(again)
        };
        wait(&mut a, 1, 1)?;
        let args = [Arg::I(setup.state_size), Arg::At(STATE)];
        method(&mut a, DEVICE, GET_DEVICE_STATE, &args)?;
        result(&mut a)?;
        a.mov(dword_ptr(at(COUNT)), EVENT_ROOM)?;
        let args = [Arg::I(16), Arg::At(EVENTS), Arg::At(COUNT), Arg::I(0)];
        method(&mut a, DEVICE, GET_DEVICE_DATA, &args)?;
        result(&mut a)?;
        wait(&mut a, 2, 2)?;
        a.push(0)?;
        a.call(dword_ptr(iat[2] as u64))?;
        a.int3()
    })()
    .unwrap();
    let code = a.assemble((BASE + code_rva) as u64).unwrap();
    let flags = pe::SCN_CODE | pe::SCN_EXECUTE | pe::SCN_READ;
    assert_eq!(b.section(".text", &code, flags), code_rva);
    b.entry(code_rva);
    (b.build(), base)
}

/// A machine with the built-in Windows on and `program` loaded from the
/// shell as T.EXE.
fn machine(test: &str, program: Vec<u8>) -> Cpu {
    let dir = PathBuf::from("target/test_win32_dinput").join(test);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("T.EXE"), program).unwrap();
    let mut cpu = Cpu::new(dir.clone());
    cpu.bus.win32.enabled = true;
    cpu.host_console = Some(HostConsole::default());
    cpu.load_shell();
    assert!(cpu.load_executable("T.EXE", None));
    if std::env::var_os("DINPUT_LOG").is_some() {
        let log = rust_dos::log::LogFile::create_with_limit(&dir.join("trace.log"), 1 << 28);
        cpu.bus.log_file = Some(log.unwrap());
        // Every call, not the first few from each place.
        rust_dos::win32::trace::set_level(&mut cpu, rust_dos::config::Win32Trace::All);
    }
    cpu
}

/// What the program printed, to standard output and error.
fn printed(cpu: &Cpu) -> String {
    let console = cpu.host_console.as_ref().unwrap();
    let mut text = console.stdout.clone();
    text.extend(&console.stderr);
    String::from_utf8_lossy(&text).into_owned()
}

/// Run a batch of `n` instructions: whether the program ended.
fn batch(cpu: &mut Cpu, n: u64) -> bool {
    cpu.bus.start_batch(cpu.bus.clock.icount + n);
    run_batch(cpu, &mut NoHook, false) == StopReason::ShellReloaded
}

/// Run the machine until the program runs with its page tables on:
/// where its data at `data` is in memory. The data's first page, which
/// holds what the test reads and writes, stays there.
fn started(cpu: &mut Cpu, data: u32) -> usize {
    for _ in 0..20_000 {
        assert!(!batch(cpu, 1000), "the program ended early");
        if cpu.bus.win32.active()
            && cpu.cr0 & 0x8000_0000 != 0
            && let Some(at) = cpu.peek_translate(data)
        {
            return at as usize;
        }
    }
    panic!("the program didn't start");
}

/// The data's word at `offset`.
fn word(cpu: &Cpu, data: usize, offset: u32) -> u32 {
    cpu.bus.read_32(data + offset as usize)
}

/// Run the program until its data's `READY` is `value`.
fn until_ready(cpu: &mut Cpu, data: usize, value: u32) {
    for _ in 0..2000 {
        if batch(cpu, 20_000) {
            panic!("the program ended early: {}", printed(cpu));
        }
        if word(cpu, data, READY) == value {
            return;
        }
    }
    let results: Vec<u32> = (0..10).map(|i| word(cpu, data, RESULTS + 4 * i)).collect();
    panic!(
        "the program didn't get to step {}: {:x?} {}",
        value,
        results,
        printed(cpu)
    );
}

/// Let time pass, so that the devices are read.
fn settle(cpu: &mut Cpu) {
    for _ in 0..20 {
        assert!(!batch(cpu, 20_000));
    }
}

/// What the program read, after it has: the results of its calls, the
/// first `state` bytes of the device state, and the buffered data
/// (offset, data, time, sequence).
struct Read {
    results: Vec<u32>,
    state: Vec<u8>,
    events: Vec<(u32, u32, u32, u32)>,
}

/// Say go, wait for the program's reads, let it end and give them.
fn read(cpu: &mut Cpu, data: usize, calls: u32, state: usize) -> Read {
    cpu.bus.write_32(data + GO as usize, 1);
    until_ready(cpu, data, 2);
    let results = (0..calls)
        .map(|i| word(cpu, data, RESULTS + 4 * i))
        .collect();
    let bytes = (0..state)
        .map(|i| cpu.bus.read_8(data + STATE as usize + i))
        .collect();
    let count = word(cpu, data, COUNT).min(EVENT_ROOM);
    let events = (0..count)
        .map(|i| {
            let field = |k: u32| word(cpu, data, EVENTS + 16 * i + 4 * k);
            (field(0), field(1), field(2), field(3))
        })
        .collect();
    let read = Read {
        results,
        state: bytes,
        events,
    };
    cpu.bus.write_32(data + GO as usize, 2);
    let mut ended = false;
    for _ in 0..2000 {
        if batch(cpu, 20_000) {
            ended = true;
            break;
        }
    }
    assert!(ended, "the program didn't end");
    assert_eq!(cpu.errorlevel, 0);
    read
}

/// The sequence numbers go up and the times don't go back.
fn in_order(events: &[(u32, u32, u32, u32)]) {
    for pair in events.windows(2) {
        assert!(pair[1].3 >= pair[0].3, "{:x?}", events);
        assert!(pair[1].2 >= pair[0].2, "{:x?}", events);
    }
}

fn keyboard(cpu: &mut Cpu, scan: u8, extended: bool, down: bool) {
    rust_dos::keyboard::key_event(&mut cpu.bus, scan, extended, down, None);
    settle(cpu);
}

#[test]
fn the_keyboard_gives_its_keys_as_state_and_buffered_data() {
    let mut d = Data::new();
    let guid = d.guid(0x6F1D_2B61, 0xD5A0, 0x11CF);
    // A key per byte, as c_dfDIKeyboard has them.
    let keys: Vec<(Option<u32>, u32, u32)> = (0..256)
        .map(|i| (None, i, DIDFT_OPTIONAL | DIDFT_BUTTON | i << 8))
        .collect();
    let format = d.format(0, 256, &keys);
    let buffer = d.add(&words(&[20, 16, 0, 0, EVENT_ROOM]));
    let setup = Setup {
        guid,
        format,
        state_size: 256,
        properties: vec![(DIPROP_BUFFERSIZE, buffer)],
    };
    let (image, data) = program(&d, &setup);
    let mut cpu = machine("keyboard", image);
    let data = started(&mut cpu, data);
    until_ready(&mut cpu, data, 1);
    // A goes down and up, then the left arrow (E0 4B) down.
    keyboard(&mut cpu, 0x1E, false, true);
    keyboard(&mut cpu, 0x1E, false, false);
    keyboard(&mut cpu, 0x4B, true, true);
    let r = read(&mut cpu, data, 8, 256);
    assert_eq!(r.results, [0; 8], "{:x?}", r.results);
    assert_eq!(r.state[0x1E], 0);
    assert_eq!(r.state[0xCB], 0x80);
    assert_eq!(r.state.iter().filter(|&&b| b != 0).count(), 1);
    let got: Vec<(u32, u32)> = r.events.iter().map(|e| (e.0, e.1)).collect();
    assert_eq!(got, [(0x1E, 0x80), (0x1E, 0), (0xCB, 0x80)]);
    in_order(&r.events);
    assert!(r.events[0].3 < r.events[1].3 && r.events[1].3 < r.events[2].3);
}

#[test]
fn a_key_that_goes_down_and_up_between_two_reads_is_in_the_buffer() {
    let mut d = Data::new();
    let guid = d.guid(0x6F1D_2B61, 0xD5A0, 0x11CF);
    let keys: Vec<(Option<u32>, u32, u32)> = (0..256)
        .map(|i| (None, i, DIDFT_OPTIONAL | DIDFT_BUTTON | i << 8))
        .collect();
    let format = d.format(0, 256, &keys);
    let buffer = d.add(&words(&[20, 16, 0, 0, EVENT_ROOM]));
    let setup = Setup {
        guid,
        format,
        state_size: 256,
        properties: vec![(DIPROP_BUFFERSIZE, buffer)],
    };
    let (image, data) = program(&d, &setup);
    let mut cpu = machine("keyboard-tap", image);
    let data = started(&mut cpu, data);
    until_ready(&mut cpu, data, 1);
    // A goes down and up with no time for a read between.
    rust_dos::keyboard::key_event(&mut cpu.bus, 0x1E, false, true, None);
    keyboard(&mut cpu, 0x1E, false, false);
    let r = read(&mut cpu, data, 8, 256);
    assert_eq!(r.results, [0; 8], "{:x?}", r.results);
    assert_eq!(r.state.iter().filter(|&&b| b != 0).count(), 0);
    let got: Vec<(u32, u32)> = r.events.iter().map(|e| (e.0, e.1)).collect();
    assert_eq!(got, [(0x1E, 0x80), (0x1E, 0)]);
    in_order(&r.events);
}

#[test]
fn the_mouse_gives_its_motion_as_relative_axes() {
    let mut d = Data::new();
    let guid = d.guid(0x6F1D_2B60, 0xD5A0, 0x11CF);
    let x = d.guid(0xA36D_02E0, 0xC9F3, 0x11CF);
    let y = d.guid(0xA36D_02E1, 0xC9F3, 0x11CF);
    let z = d.guid(0xA36D_02E2, 0xC9F3, 0x11CF);
    let button = DIDFT_BUTTON | DIDFT_ANYINSTANCE | DIDFT_OPTIONAL;
    // As c_dfDIMouse is.
    let objects = [
        (Some(x), 0, DIDFT_AXIS | DIDFT_ANYINSTANCE),
        (Some(y), 4, DIDFT_AXIS | DIDFT_ANYINSTANCE),
        (Some(z), 8, DIDFT_AXIS | DIDFT_ANYINSTANCE | DIDFT_OPTIONAL),
        (None, 12, button),
        (None, 13, button),
        (None, 14, button),
        (None, 15, button),
    ];
    let format = d.format(DIDF_RELAXIS, 16, &objects);
    let buffer = d.add(&words(&[20, 16, 0, 0, EVENT_ROOM]));
    let setup = Setup {
        guid,
        format,
        state_size: 16,
        properties: vec![(DIPROP_BUFFERSIZE, buffer)],
    };
    let (image, data) = program(&d, &setup);
    let mut cpu = machine("mouse", image);
    let data = started(&mut cpu, data);
    until_ready(&mut cpu, data, 1);
    cpu.bus.mouse.move_by(5.0, -3.0);
    settle(&mut cpu);
    cpu.bus.mouse.button_down(0);
    settle(&mut cpu);
    cpu.bus.mouse.move_by(2.0, 0.0);
    settle(&mut cpu);
    let r = read(&mut cpu, data, 8, 16);
    assert_eq!(r.results, [0; 8], "{:x?}", r.results);
    let value = |k: usize| i32::from_le_bytes(r.state[k..k + 4].try_into().unwrap());
    assert_eq!((value(0), value(4), value(8)), (7, -3, 0));
    assert_eq!(r.state[12..16], [0x80, 0, 0, 0]);
    let got: Vec<(u32, i32)> = r.events.iter().map(|e| (e.0, e.1 as i32)).collect();
    assert_eq!(got, [(0, 5), (4, -3), (12, 0x80), (0, 2)]);
    in_order(&r.events);
    // The two axes of one move share a sequence number.
    assert_eq!(r.events[0].3, r.events[1].3);
    assert!(r.events[1].3 < r.events[2].3 && r.events[2].3 < r.events[3].3);
}

#[test]
fn a_joystick_on_the_game_port_gives_its_axes_in_their_range() {
    let mut d = Data::new();
    let guid = d.guid(0x6F1D_2B70, 0xD5A0, 0x11CF);
    let x = d.guid(0xA36D_02E0, 0xC9F3, 0x11CF);
    let y = d.guid(0xA36D_02E1, 0xC9F3, 0x11CF);
    let button = DIDFT_BUTTON | DIDFT_ANYINSTANCE | DIDFT_OPTIONAL;
    let objects = [
        (Some(x), 0, DIDFT_AXIS | DIDFT_ANYINSTANCE),
        (Some(y), 4, DIDFT_AXIS | DIDFT_ANYINSTANCE),
        (None, 8, button),
        (None, 9, button),
    ];
    let format = d.format(DIDF_ABSAXIS, 12, &objects);
    let buffer = d.add(&words(&[20, 16, 0, 0, EVENT_ROOM]));
    let range = d.add(&words(&[24, 16, 0, 0, -1000i32 as u32, 1000]));
    let setup = Setup {
        guid,
        format,
        state_size: 12,
        properties: vec![(DIPROP_BUFFERSIZE, buffer), (DIPROP_RANGE, range)],
    };
    let (image, data) = program(&d, &setup);
    let mut cpu = machine("joystick", image);
    cpu.bus.joystick.set_settings(JoystickSettings {
        kind: JoystickType::FourAxis,
        ..Default::default()
    });
    let centre = PadState {
        axes: [0.0; 4],
        buttons: 0,
    };
    cpu.bus.joystick.set_pad(0, Some(centre));
    let data = started(&mut cpu, data);
    until_ready(&mut cpu, data, 1);
    // Right and up, with button A held.
    let pushed = PadState {
        axes: [1.0, -1.0, 0.0, 0.0],
        buttons: PAD_A,
    };
    cpu.bus.joystick.set_pad(0, Some(pushed));
    settle(&mut cpu);
    let r = read(&mut cpu, data, 9, 12);
    assert_eq!(r.results, [0; 9], "{:x?}", r.results);
    let value = |k: usize| i32::from_le_bytes(r.state[k..k + 4].try_into().unwrap());
    assert!(value(0) > 950 && value(0) <= 1000, "x {}", value(0));
    assert!(value(4) < -950 && value(4) >= -1000, "y {}", value(4));
    assert_eq!(r.state[8..10], [0x80, 0]);
    let offsets: Vec<u32> = r.events.iter().map(|e| e.0).collect();
    assert_eq!(offsets, [0, 4, 8], "{:x?}", r.events);
    assert_eq!(r.events[0].1 as i32, value(0));
    assert_eq!(r.events[1].1 as i32, value(4));
    assert_eq!(r.events[2].1, 0x80);
    in_order(&r.events);
}
