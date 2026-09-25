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
