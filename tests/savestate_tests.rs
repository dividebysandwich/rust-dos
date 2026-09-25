//! Save states of a running machine: a machine a state is loaded into
//! goes on exactly as the one that saved it, whatever it was doing, and a
//! state that can't be loaded leaves the machine as it was.

mod dyndiff;
mod pmrig;

use chrono::NaiveDate;
use iced_x86::code_asm::*;
use pmrig::*;
use rust_dos::cpu::Cpu;
use rust_dos::exec::{self, NoHook};
use rust_dos::interrupts::int10;
use rust_dos::savestate::{StateError, machine};
use rust_dos::video::adapter::{Adapter, VideoSetup};
use rust_dos::video::{self, Frame, bios};
use std::fs;
use std::path::{Path, PathBuf};

fn fix_time() {
    let at = NaiveDate::from_ymd_opt(1995, 4, 11).unwrap().and_hms_opt(12, 34, 56).unwrap();
    rust_dos::hosttime::fix(Some(at));
}

fn scratch(name: &str, files: &[(&str, &[u8])]) -> PathBuf {
    let base = PathBuf::from("target/test_savestate").join(name);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    for (file, bytes) in files {
        fs::write(base.join(file), bytes).unwrap();
    }
    base
}

fn machine_in(dir: &Path) -> Cpu {
    let mut cpu = Cpu::new(dir.to_path_buf());
    cpu.bus.set_cycles_per_ms(1000);
    cpu.load_shell();
    cpu
}

/// Run a millisecond of emulated time.
fn run_ms(cpu: &mut Cpu) {
    let end = cpu.bus.clock.icount + 1000;
    cpu.bus.start_batch(end);
    exec::run_batch(cpu, &mut NoHook, false);
}

/// A program that prints a dot on each of 40 timer ticks, waiting for
/// them with HLT, then waits for a key and exits with 3.
fn dots_program() -> Vec<u8> {
    let mut a = CodeAssembler::new(16).unwrap();
    let mut next = a.create_label();
    a.mov(cx, 40u32).unwrap();
    a.set_label(&mut next).unwrap();
    a.mov(ah, 2u32).unwrap();
    a.mov(dl, b'.' as u32).unwrap();
    a.int(0x21).unwrap();
    a.hlt().unwrap();
    a.loop_(next).unwrap();
    a.mov(ah, 0u32).unwrap();
    a.int(0x16).unwrap();
    a.mov(ax, 0x4C03u32).unwrap();
    a.int(0x21).unwrap();
    a.assemble(0x100).unwrap()
}

/// The text screen, from the BIOS's text memory.
fn screen(cpu: &Cpu) -> String {
    cpu.bus.vga.vram_text.chunks(2).take(80 * 25).map(|cell| if cell[0] == 0 { ' ' } else { cell[0] as char }).collect()
}

#[test]
fn a_loaded_dos_machine_goes_on_as_the_one_that_saved_it() {
    fix_time();
    let dir = scratch(
        "dos",
        &[
            ("DOTS.COM", &dots_program()),
            ("RUN.BAT", b"@echo off\r\ndots\r\nif errorlevel 3 echo three\r\necho done\r\n"),
        ],
    );
    let mut a = machine_in(&dir);
    a.pending_command = Some("RUN".to_string());
    for _ in 0..800 {
        run_ms(&mut a);
    }
    assert!(a.batch.is_active() && a.current_psp != 0, "the batch file's program runs");

    // The state goes into a fresh machine, and back into the one that
    // saved it, so both start with empty caches.
    let state = machine::save(&a);
    let mut b = machine_in(&dir);
    machine::load(&mut b, &state).unwrap();
    machine::load(&mut a, &state).unwrap();
    assert!(machine::save(&b) == state, "a loaded state saves as the same bytes");

    let enter = rust_dos::keyboard::lookup("enter").unwrap();
    for n in 0..4000 {
        for cpu in [&mut a, &mut b] {
            if n == 2500 {
                rust_dos::keyboard::apply_key(&mut cpu.bus, enter, enter.ascii, true);
            }
            if n == 2550 {
                rust_dos::keyboard::apply_key(&mut cpu.bus, enter, enter.ascii, false);
            }
            run_ms(cpu);
        }
        let (sa, sb) = (dyndiff::State::of(&a), dyndiff::State::of(&b));
        assert!(sa == sb, "diverged after {} ms:\n{}", n, sa.diff(&sb));
        if let Some(difference) = dyndiff::memory_difference(&a, &b) {
            panic!("diverged after {} ms: {}", n, difference);
        }
        if n % 500 == 0 {
            assert!(machine::save(&a) == machine::save(&b), "the states diverged after {} ms", n);
        }
    }
    assert!(!b.batch.is_active(), "the batch file ended");
    let text = screen(&b);
    assert!(text.contains("three") && text.contains("done"), "{}", text.trim_end());
}

#[test]
fn a_loaded_protected_mode_machine_goes_on_as_the_one_that_saved_it() {
    fix_time();
    let mut rig = Rig::new();
    rig.handler(0x08, 0, |a| {
        a.push(eax)?;
        a.inc(edi)?;
        a.mov(al, 0x20)?;
        a.out(0x20, al)?;
        a.pop(eax)?;
        a.iretd()
    });
    let code = asm32(CODE, |a| {
        a.mov(al, 0xFE)?;
        a.out(0x21, al)?;
        a.mov(al, 0x34)?;
        a.out(0x43, al)?;
        a.mov(al, 0x00)?;
        a.out(0x40, al)?;
        a.mov(al, 0x01)?;
        a.out(0x40, al)?;
        a.xor(edi, edi)?;
        a.mov(ecx, 100_000u32)?;
        a.sti()?;
        let mut top = a.create_label();
        a.set_label(&mut top)?;
        a.mov(eax, ecx)?;
        a.imul_3(eax, eax, 7)?;
        a.xor(dword_ptr(DATA), eax)?;
        a.fild(dword_ptr(DATA))?;
        a.fstp(dword_ptr(DATA + 8))?;
        a.dec(ecx)?;
        a.jnz(top)?;
        a.cli()?;
        a.hlt()
    });
    rig.load(CODE, &code);
    rig.enter_pm();
    let mut a = rig.cpu;
    for _ in 0..100 {
        dyndiff::batch(&mut a, 5_000, true);
    }
    assert!(a.edi() > 0 && a.ecx() != 0, "the timer interrupts the loop");

    // A machine that never ran the program (with every interrupt masked,
    // as the rig starts) gets all it needs from the state: the descriptor
    // tables in RAM, the segment caches, the PIC's and the PIT's
    // programming.
    let state = machine::save(&a);
    let mut b = Rig::new().cpu;
    machine::load(&mut b, &state).unwrap();
    machine::load(&mut a, &state).unwrap();
    dyndiff::lockstep_with(&mut a, &mut b, 400, 5_000, true, |_, _| {}).unwrap();
    assert_eq!(b.ecx(), 0, "the loop ran to the end");
}

#[test]
fn a_state_that_cant_be_loaded_leaves_the_machine_as_it_was() {
    fix_time();
    let dir = scratch("refused", &[]);
    let mut small = Cpu::with_memory(dir.clone(), 4);
    small.load_shell();
    let before = machine::save(&small);

    let state = machine::save(&machine_in(&dir));
    let refused = machine::load(&mut small, &state);
    assert!(matches!(refused, Err(StateError::Mismatch(_))), "{:?}", refused);
    assert!(machine::save(&small) == before, "a 16 MB state leaves a 4 MB machine alone");

    let own = machine::save(&small);
    assert_eq!(machine::load(&mut small, &own[..own.len() - 10]), Err(StateError::Truncated));
    assert!(machine::save(&small) == before, "a state cut short leaves the machine alone");
}

/// A machine with `adapter`, at the prompt.
fn video_machine(adapter: Adapter) -> Cpu {
    let mut cpu = Cpu::new(PathBuf::from("."));
    cpu.bus.set_cycles_per_ms(1000);
    bios::install(&mut cpu.bus, VideoSetup { adapter, ..Default::default() });
    cpu.load_shell();
    cpu
}

fn picture(cpu: &mut Cpu) -> Vec<u8> {
    let (width, height) = video::frame_size(&cpu.bus);
    let mut frame = Frame::new(width, height);
    cpu.bus.vga.mark_dirty_full();
    video::render_screen(&mut frame, &cpu.bus);
    frame.rgb
}

/// INT 10h with AX and BX.
fn int10(cpu: &mut Cpu, ax_value: u16, bx_value: u16) {
    cpu.set_ax(ax_value);
    cpu.set_bx(bx_value);
    int10::handle(cpu);
}

/// Save `a`, load the state into a fresh machine with the same adapter,
/// and check that it shows the same picture and saves the same state.
fn shows_the_same_after_a_load(what: &str, adapter: Adapter, a: &mut Cpu) {
    let before = picture(a);
    let colours: std::collections::HashSet<&[u8]> = before.chunks(3).collect();
    assert!(colours.len() > 1, "{}: the picture has something in it", what);
    let state = machine::save(a);
    let mut b = video_machine(adapter);
    assert!(picture(&mut b) != before, "{}: the fresh machine shows something else", what);
    machine::load(&mut b, &state).unwrap();
    assert!(machine::save(&b) == state, "{}: the loaded state saves as the same bytes", what);
    assert!(picture(&mut b) == before, "{}: the loaded machine shows the same picture", what);
}

#[test]
fn a_loaded_machine_shows_the_same_picture() {
    // VGA 256 colours, with a palette of the program's own.
    let mut a = video_machine(Adapter::Vga);
    int10::set_mode(&mut a, 0x13);
    a.bus.io_write(0x3C8, 1);
    for value in [63, 20, 0] {
        a.bus.io_write(0x3C9, value);
    }
    for i in 0..64_000 {
        a.bus.write_8(0xA0000 + i, (i % 7) as u8);
    }
    shows_the_same_after_a_load("mode 13h", Adapter::Vga, &mut a);

    // VGA planes, written through the map mask, with a scrolled start.
    let mut a = video_machine(Adapter::Vga);
    int10::set_mode(&mut a, 0x12);
    for (plane, offset) in [(1u8, 0usize), (2, 4000), (4, 8000), (8, 12000)] {
        a.bus.io_write(0x3C4, 2);
        a.bus.io_write(0x3C5, plane);
        for i in 0..6000 {
            a.bus.write_8(0xA0000 + offset + i, 0xF0);
        }
    }
    a.bus.io_write(0x3D4, 0x0D);
    a.bus.io_write(0x3D5, 80);
    a.bus.vga.latch_start_address();
    shows_the_same_after_a_load("mode 12h", Adapter::Vga, &mut a);

    // VESA direct colour in the linear frame buffer, shown from a line on.
    let mut a = video_machine(Adapter::Svga);
    int10(&mut a, 0x4F02, 0x4111);
    for i in 0..640 * 400 {
        let pixel = ((i * 37) & 0xFFFF) as u16;
        a.bus.write_8(video::vbe::LFB_BASE + i * 2, pixel as u8);
        a.bus.write_8(video::vbe::LFB_BASE + i * 2 + 1, (pixel >> 8) as u8);
    }
    a.set_cx(0);
    a.set_dx(3);
    int10(&mut a, 0x4F07, 0);
    a.bus.vga.latch_start_address();
    shows_the_same_after_a_load("VESA 111h", Adapter::Svga, &mut a);

    // Text with colours, on the VGA and on a Hercules card.
    for adapter in [Adapter::Vga, Adapter::Hercules] {
        let mut a = video_machine(adapter);
        int10::set_mode(&mut a, if adapter == Adapter::Hercules { 0x07 } else { 0x03 });
        for (i, c) in b"Saved and loaded".iter().enumerate() {
            a.bus.write_8(0xB8000 + i * 2, *c);
            a.bus.write_8(0xB8000 + i * 2 + 1, 0x1E);
            a.bus.write_8(0xB0000 + i * 2, *c);
            a.bus.write_8(0xB0000 + i * 2 + 1, 0x0F);
        }
        shows_the_same_after_a_load(&format!("text on {:?}", adapter), adapter, &mut a);
    }

    // CGA 4 colours in the other palette, and the Tandy's 16 colours in
    // system memory.
    let mut a = video_machine(Adapter::Cga);
    int10::set_mode(&mut a, 0x04);
    a.bus.io_write(0x3D9, 0x21);
    for i in 0..8000 {
        a.bus.write_8(0xB8000 + i, 0x1B);
        a.bus.write_8(0xBA000 + i, 0xE4);
    }
    shows_the_same_after_a_load("CGA mode 4", Adapter::Cga, &mut a);

    let mut a = video_machine(Adapter::Tandy);
    int10::set_mode(&mut a, 0x09);
    for i in 0..0x8000 {
        a.bus.write_8(0xB8000 + i, (i * 13) as u8);
    }
    shows_the_same_after_a_load("Tandy mode 9", Adapter::Tandy, &mut a);
}

/// A machine at the prompt with the default Sound Blaster 16 and
/// Ultrasound, and the Tandy's sound chip.
fn sound_machine() -> Cpu {
    let mut cpu = Cpu::new(PathBuf::from("."));
    cpu.bus.set_cycles_per_ms(1000);
    cpu.bus.configure_sound(Some(rust_dos::sb::SbConfig::default()), true);
    cpu.bus.configure_tandy_sound(rust_dos::sn76489::TandySound::On);
    cpu.load_shell();
    cpu
}

/// The sound of the millisecond since the last call.
fn sound(cpu: &mut Cpu) -> Vec<i16> {
    cpu.bus.audio_catch_up();
    cpu.bus.audio_out.drain(..).collect()
}

#[test]
fn a_loaded_machine_sounds_the_same() {
    fix_time();
    let mut a = sound_machine();
    let bus = &mut a.bus;
    // The Sound Blaster plays a sawtooth from memory over and over.
    for i in 0..4096 {
        bus.write_8(0x20000 + i, (i * 7) as u8);
    }
    bus.io_write(0x226, 1);
    bus.io_write(0x226, 0);
    let dsp = |bus: &mut rust_dos::bus::Bus, value: u8| bus.io_write(0x22C, value);
    dsp(bus, 0xD1);
    dsp(bus, 0x40);
    dsp(bus, 211);
    bus.io_write(0x0A, 0x05);
    bus.io_write(0x0C, 0x00);
    bus.io_write(0x0B, 0x59);
    for value in [0x00, 0x00, 0x02] {
        bus.io_write(if value == 0x02 { 0x83 } else { 0x02 }, value);
    }
    bus.io_write(0x03, 0xFF);
    bus.io_write(0x03, 0x0F);
    bus.io_write(0x0A, 0x01);
    dsp(bus, 0x48);
    dsp(bus, 0xFF);
    dsp(bus, 0x07);
    dsp(bus, 0x1C);
    // The FM chip holds a note.
    for (reg, value) in [(0x20, 0x01), (0x40, 0x10), (0x60, 0xF0), (0x80, 0x77), (0x23, 0x01), (0x43, 0x00), (0x63, 0xF0), (0x83, 0x77), (0xA0, 0x98), (0xB0, 0x31)] {
        bus.io_write(0x388, reg);
        bus.io_write(0x389, value);
    }
    // The Ultrasound loops a sample at a quarter of its volume.
    let gus = |bus: &mut rust_dos::bus::Bus, reg: u8, value: u16, wide: bool| {
        bus.io_write(0x343, reg);
        if wide {
            bus.io_write(0x344, value as u8);
        }
        bus.io_write(0x345, (value >> 8) as u8);
    };
    for addr in 0..256u32 {
        gus(bus, 0x43, addr as u16, true);
        gus(bus, 0x44, 0, false);
        bus.io_write(0x347, (addr * 3) as u8);
    }
    bus.io_write(0x342, 0);
    for (reg, sample) in [(0x02, 0u32), (0x04, 255), (0x0A, 0)] {
        let pos = sample << 9;
        gus(bus, reg, (pos >> 16) as u16 & 0x1FFF, true);
        gus(bus, reg + 1, pos as u16, true);
    }
    gus(bus, 0x01, 1 << 10, true);
    gus(bus, 0x09, 0xC000, true);
    gus(bus, 0x0D, 0x0300, false);
    gus(bus, 0x00, 0x0800, false);
    // The Tandy's chip holds a tone.
    for value in [0x8E, 0x0F, 0x92] {
        bus.io_write(0xC0, value);
    }
    for _ in 0..300 {
        run_ms(&mut a);
    }

    let state = machine::save(&a);
    let mut b = sound_machine();
    machine::load(&mut b, &state).unwrap();
    machine::load(&mut a, &state).unwrap();
    assert!(machine::save(&b) == state, "a loaded state saves as the same bytes");
    let mut loudest = 0;
    for n in 0..300 {
        run_ms(&mut a);
        run_ms(&mut b);
        let (sa, sb) = (sound(&mut a), sound(&mut b));
        assert!(sa == sb, "the sound differs after {} ms", n);
        loudest = sa.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0).max(loudest);
        if n % 100 == 0 {
            assert!(machine::save(&a) == machine::save(&b), "the states diverged after {} ms", n);
        }
    }
    assert!(loudest > 1000, "there is something to hear: {}", loudest);
}

/// INT 21h with AX, BX, CX and DS:DX at 2000:0000, with `text` there as
/// an ASCIIZ string. Returns AX, or the error with CF set.
fn dos(cpu: &mut Cpu, function: u16, bx_value: u16, cx_value: u16, text: &str) -> Result<u16, u16> {
    for (i, b) in text.bytes().chain(std::iter::once(0)).enumerate() {
        cpu.bus.write_8(0x20000 + i, b);
    }
    cpu.set_ds(0x2000);
    cpu.set_dx(0);
    cpu.set_ax(function);
    cpu.set_bx(bx_value);
    cpu.set_cx(cx_value);
    rust_dos::interrupts::int21::handle(cpu);
    if cpu.get_cpu_flag(rust_dos::cpu::CpuFlags::CF) { Err(cpu.ax()) } else { Ok(cpu.ax()) }
}

/// Read `count` bytes from `handle`.
fn read(cpu: &mut Cpu, handle: u16, count: u16) -> Vec<u8> {
    let read = dos(cpu, 0x3F00, handle, count, "").unwrap() as usize;
    (0..read).map(|i| cpu.bus.read_8(0x20000 + i)).collect()
}

#[test]
fn a_loaded_machine_has_its_drives_files_and_memory() {
    fix_time();
    let data: Vec<u8> = (0..1024u32).map(|i| i as u8).collect();
    let dir = scratch("files", &[("DATA.BIN", &data), ("TEMP.BIN", b"gone soon")]);
    fs::create_dir_all(dir.join("SUB")).unwrap();
    let d_dir = scratch("files_d", &[("ON_D.TXT", b"d")]);
    let e_dir = scratch("files_e", &[]);
    let machine = || {
        let mut cpu = machine_in(&dir);
        rust_dos::ems::set_enabled(&mut cpu.bus, true);
        cpu
    };
    let mut a = machine();
    a.bus.disk.mount(3, &d_dir, Default::default(), false).unwrap();
    dos(&mut a, 0x3B00, 0, 0, "SUB").unwrap();
    let data_handle = dos(&mut a, 0x3D00, 0, 0, "..\\DATA.BIN").unwrap();
    assert_eq!(read(&mut a, data_handle, 10), &data[..10]);
    let duplicate = dos(&mut a, 0x4500, data_handle, 0, "").unwrap();
    let temp_handle = dos(&mut a, 0x3D02, 0, 0, "C:\\TEMP.BIN").unwrap();
    let on_d = dos(&mut a, 0x3D00, 0, 0, "D:ON_D.TXT").unwrap();
    // Expanded memory with a page mapped and written.
    a.set_bx(4);
    a.set_ax(0x4300);
    rust_dos::ems::handle(&mut a);
    let ems_handle = a.dx();
    a.set_bx(2);
    a.set_ax(0x4401);
    rust_dos::ems::handle(&mut a);
    assert_eq!(a.get_ah(), 0);
    a.bus.write_8(0xE4000, 0x5A);

    let state = machine::save(&a);
    fs::remove_file(dir.join("TEMP.BIN")).unwrap();
    let mut b = machine();
    b.bus.disk.mount(4, &e_dir, Default::default(), false).unwrap();
    machine::load(&mut b, &state).unwrap();

    // The drives as they were: D: mounted, E: gone, SUB current on C:.
    assert!(b.bus.disk.is_mounted(3) && !b.bus.disk.is_mounted(4));
    assert_eq!(b.bus.disk.get_current_directory_of(2).as_deref(), Some("SUB"));
    // The file and its duplicate share their position again; the file
    // that went is closed.
    assert_eq!(read(&mut b, duplicate, 5), &data[10..15]);
    assert_eq!(read(&mut b, data_handle, 5), &data[15..20]);
    assert_eq!(read(&mut b, on_d, 5), b"d");
    assert!(!b.bus.disk.is_open(temp_handle));
    // The expanded memory's handle and its page in the frame.
    let handles = b.bus.ems.as_ref().unwrap().handles();
    assert!(handles.iter().any(|&(h, pages, _)| h == ems_handle && pages == 4), "{:?}", handles);
    assert_eq!(b.bus.read_8(0xE4000), 0x5A);
    b.set_bx(0);
    b.set_ax(0x4401);
    b.set_dx(ems_handle);
    rust_dos::ems::handle(&mut b);
    assert_eq!(b.get_ah(), 0, "the handle can be mapped");
}
