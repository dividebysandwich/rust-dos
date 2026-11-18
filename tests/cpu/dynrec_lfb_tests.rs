//! Translated code's own loads and stores into the VESA linear frame
//! buffer (`dynrec::helpers::card_state`, `Bus::lfb_direct`), on a
//! ToadForce whose host renderer has a surface ahead of card memory: the
//! interpreter and the recompiler side by side must leave the same card
//! memory, the same counts for `cycles=auto`, and hand the host the same
//! written pixels with the same stamps of its pages.

use crate::pmrig::*;
use iced_x86::code_asm::*;
use rust_dos::cpu::CoreMode;
use rust_dos::dynrec::AVAILABLE;
use rust_dos::toadforce::format::{Format, Rect, Surface};
use rust_dos::toadforce::gpu::{Batch, Clear, Decline, Host, Pages};
use rust_dos::toadforce::level1::{self, CLEAR_COLOUR, Target};
use rust_dos::toadforce::pci::{INITIAL_MEMORY, INITIAL_REGISTERS};
use rust_dos::toadforce::raster::pipeline::DrawState;
use rust_dos::toadforce::regs;
use rust_dos::video::adapter::{Adapter, VideoSetup};
use rust_dos::video::bios;
use std::cell::RefCell;
use std::rc::Rc;

const MEMORY: u32 = INITIAL_MEMORY;

/// What the host was handed, in order.
type Log = Rc<RefCell<Vec<String>>>;

/// A host that takes every clear (so the surface cleared is ahead after
/// it), and logs the pixels the card hands it with the stamp of their
/// pages: a write the card or the code left out of `Written` or out of
/// `Pages` shows as a difference between two runs.
struct Recorder(Log);

impl Recorder {
    fn log(&self, line: String) {
        self.0.borrow_mut().push(line);
    }
}

fn range(s: &Surface) -> (u32, u32) {
    (s.offset, s.offset + s.pitch * s.height)
}

impl Host for Recorder {
    fn name(&self) -> &str {
        "recorder"
    }

    fn declines(&self, _: &DrawState) -> Option<Decline> {
        None
    }

    fn draw(&mut self, _: &Batch, _: &[u8], _: &Pages) -> Vec<Surface> {
        Vec::new()
    }

    fn clear(&mut self, clear: &Clear, _: &[u8], pages: &Pages) -> Option<Surface> {
        let s = clear.surface;
        self.log(format!(
            "clear {s:?} {:?} {:#x} {:?}",
            clear.rects,
            clear.value,
            pages.stamp(range(&s))
        ));
        Some(s)
    }

    fn fill(&mut self, _: &Surface, _: &Rect, _: u32, _: &[u8], _: &Pages) -> Option<Surface> {
        None
    }

    fn blt(
        &mut self,
        _: &Surface,
        _: &Rect,
        _: &Surface,
        _: &Rect,
        _: Option<u32>,
        _: &[u8],
        _: &Pages,
    ) -> Option<Surface> {
        None
    }

    fn write(&mut self, surface: &Surface, rects: &[Rect], memory: &[u8]) -> bool {
        let (start, end) = range(surface);
        let sum: u64 = memory[start as usize..end as usize]
            .iter()
            .map(|&b| b as u64)
            .sum();
        self.log(format!("write {surface:?} {rects:?} sum {sum}"));
        true
    }

    fn read_back(&mut self, surface: &Surface, memory: &mut [u8], _: Option<&Pages>) -> bool {
        self.log(format!("read back {surface:?}"));
        let (start, end) = range(surface);
        memory[start as usize..end as usize].fill(0x5A);
        true
    }

    fn read_back_rows(&mut self, surface: &Surface, rows: (u32, u32), memory: &mut [u8]) -> bool {
        self.log(format!("read back rows {rows:?} of {surface:?}"));
        let at = |row: u32| (surface.offset + row * surface.pitch) as usize;
        memory[at(rows.0)..at(rows.1)].fill(0x5A);
        true
    }

    fn forget(&mut self) {
        self.log("forget".to_string());
    }
}

/// Two surfaces of 256 by 64 16-bit pixels from 1 MB on, out of the
/// picture, which the host has ahead after a clear. The program writes
/// whole pixels of the first, which the host then gets as rectangles,
/// and parts of pixels of the second, which the card reads back first.
const AHEAD: Surface = Surface {
    offset: 0x10_0000,
    pitch: 512,
    width: 256,
    height: 64,
    format: Format::R5G6B5,
};

const MIXED: Surface = Surface {
    offset: 0x10_8000,
    ..AHEAD
};

/// Where the program writes out of every surface and of the picture
/// shown.
const OUT_OFFSET: u32 = 0x20_0000;

const WHOLE: Rect = Rect {
    left: 0,
    top: 0,
    width: 256,
    height: 64,
};

/// A rig on `core` with a ToadForce showing 640x480 at 16 bits from the
/// start of its memory, its host the recorder, and `AHEAD` and `MIXED`
/// ahead on it.
fn rig(core: CoreMode, code: &[u8]) -> (Rig, Log) {
    let mut rig = Rig::new();
    rig.cpu.core = core;
    // MMX and SSE, as a Pentium III with the OS's SSE support on.
    rig.cpu.model = rust_dos::cpu::CpuModel::PentiumIII;
    rig.cpu.cr4 |= 0x200;
    bios::install(
        &mut rig.cpu.bus,
        VideoSetup {
            adapter: Adapter::ToadForce,
            ..Default::default()
        },
    );
    let bus = &mut rig.cpu.bus;
    for (register, value) in [
        (regs::MODE_WIDTH, 640),
        (regs::MODE_HEIGHT, 480),
        (regs::MODE_FORMAT, Format::R5G6B5.code()),
        (regs::MODE_PITCH, 1280),
        (regs::MODE_REFRESH, 60),
        (regs::MODE_ENABLE, 1),
        // The commands of level 1, CLEAR among them.
        (regs::LEVEL, 1),
    ] {
        bus.write_32(INITIAL_REGISTERS as usize + register as usize, value);
    }
    assert_eq!(rust_dos::autospeed::picture(bus), Some((0, 640 * 480 * 2)));
    // The picture `cycles=auto` counts is the one shown before a flip
    // (the next retrace makes it the one shown): part of the memory the
    // program writes out of every surface, from the middle of a page.
    let counted = (OUT_OFFSET as u64 + 0x800, OUT_OFFSET as u64 + 0x9000);
    bus.activity.set_picture(Some(counted));
    let log = Log::default();
    bus.toadforce_set_host(Some(Box::new(Recorder(log.clone()))));
    let mut words = Vec::new();
    for colour in [AHEAD, MIXED] {
        let target = Target {
            colour,
            depth: None,
        };
        level1::encode::render_target(&mut words, Some(&target));
        level1::encode::clear(&mut words, CLEAR_COLOUR, 0x1234, 0.0, 0, &[WHOLE]);
    }
    bus.toadforce_submit(&words);
    bus.toadforce_flush();
    assert!(bus.toadforce_ahead());
    rig.load(CODE, code);
    rig.enter_pm();
    (rig, log)
}

/// A program's own drawing in the frame buffer: whole pixels into
/// `AHEAD`, bytes, words and dwords at every alignment into `MIXED`
/// (some of them across a word of its bits, and past its end), into
/// memory out of the picture and out of any surface (some across
/// pages), after a pause longer than a burst's gap, and into the
/// picture; then reads of `MIXED` and the rest into EBX.
fn program() -> Vec<u8> {
    const OUT: u32 = MEMORY + OUT_OFFSET;
    let ahead = MEMORY + AHEAD.offset;
    let mixed = MEMORY + MIXED.offset;
    asm32(CODE, |a| {
        let mut bytes = a.create_label();
        let mut pixels = a.create_label();
        let mut words = a.create_label();
        let mut dwords = a.create_label();
        let mut runs = a.create_label();
        let mut pause = a.create_label();
        let mut shown = a.create_label();
        let mut sums = a.create_label();
        // Bytes into `AHEAD`'s rows 2 to 9.
        a.mov(edi, ahead + 2 * 512)?;
        a.mov(ecx, 0x1000u32)?;
        a.set_label(&mut bytes)?;
        a.mov(byte_ptr(edi), cl)?;
        a.inc(edi)?;
        a.dec(ecx)?;
        a.jnz(bytes)?;
        // Its pixels in rows 20 to 27.
        a.mov(edi, ahead + 20 * 512)?;
        a.mov(ecx, 0x800u32)?;
        a.set_label(&mut pixels)?;
        a.mov(word_ptr(edi), cx)?;
        a.add(edi, 2)?;
        a.dec(ecx)?;
        a.jnz(pixels)?;
        // Words three bytes apart into `MIXED`'s rows 20 on, and the
        // bytes after each.
        a.mov(edi, mixed + 20 * 512 + 1)?;
        a.mov(ecx, 0x800u32)?;
        a.set_label(&mut words)?;
        a.mov(word_ptr(edi), cx)?;
        a.xor(byte_ptr(edi + 2), 0x5A)?;
        a.add(edi, 3)?;
        a.dec(ecx)?;
        a.jnz(words)?;
        // Dwords five bytes apart out of every surface, the last ones
        // across a page, and the same into `MIXED`'s last rows and past
        // its end.
        a.mov(edi, OUT + 0x0FF0)?;
        a.mov(esi, mixed + 60 * 512)?;
        a.mov(ecx, 0x400u32)?;
        a.set_label(&mut dwords)?;
        a.mov(dword_ptr(edi), ecx)?;
        a.add(dword_ptr(esi), ecx)?;
        a.add(edi, 5)?;
        a.add(esi, 5)?;
        a.dec(ecx)?;
        a.jnz(dwords)?;
        // MMX and SSE stores of 8 and 16 bytes into `AHEAD`'s rows 40
        // on, one across a word of its bits, and out of every surface,
        // some of them into the picture `cycles=auto` counts and the
        // last across a page.
        a.mov(edi, ahead + 40 * 512)?;
        a.mov(esi, OUT + 0x4004)?;
        a.mov(ecx, 0x100u32)?;
        a.movd(mm0, ecx)?;
        a.set_label(&mut runs)?;
        a.movd(mm1, ecx)?;
        a.paddd(mm0, mm1)?;
        a.movq(qword_ptr(edi), mm0)?;
        a.movq(qword_ptr(edi + 60), mm0)?;
        a.movq(qword_ptr(esi), mm0)?;
        a.movups(xmmword_ptr(esi + 0x8), xmm0)?;
        a.movups(xmm0, xmmword_ptr(edi))?;
        a.add(edi, 8)?;
        a.add(esi, 0x31)?;
        a.dec(ecx)?;
        a.jnz(runs)?;
        a.emms()?;
        // A pause longer than a burst's gap, and writes after it.
        a.mov(ecx, 4000u32)?;
        a.set_label(&mut pause)?;
        a.dec(ecx)?;
        a.jnz(pause)?;
        a.mov(dword_ptr(OUT as u64 + 0x8000), eax)?;
        a.mov(dword_ptr(OUT as u64 + 0x8004), ecx)?;
        // Bytes into the picture's last rows.
        a.mov(edi, MEMORY + 640 * 480 * 2 - 0x1800)?;
        a.mov(ecx, 0x2000u32)?;
        a.set_label(&mut shown)?;
        a.mov(byte_ptr(edi), cl)?;
        a.inc(edi)?;
        a.dec(ecx)?;
        a.jnz(shown)?;
        // `MIXED` and what is out of every surface read back.
        a.xor(ebx, ebx)?;
        a.mov(esi, mixed)?;
        a.mov(edi, OUT + 0x0FF0)?;
        a.mov(ecx, 0x2000u32)?;
        a.set_label(&mut sums)?;
        a.movzx(eax, byte_ptr(esi))?;
        a.add(ebx, eax)?;
        a.movzx(eax, word_ptr(esi + 0x2001))?;
        a.add(ebx, eax)?;
        a.add(ebx, dword_ptr(edi))?;
        a.rol(ebx, 1)?;
        a.add(esi, 3)?;
        a.add(edi, 1)?;
        a.dec(ecx)?;
        a.jnz(sums)?;
        a.hlt()
    })
}

#[test]
fn a_programs_drawing_into_a_toadforces_memory_is_the_interpreters() {
    let code = program();
    let (mut a, log_a) = rig(CoreMode::Normal, &code);
    let (mut b, log_b) = rig(CoreMode::Dynamic, &code);
    a.run_batched_to_halt();
    b.run_batched_to_halt();
    assert_ne!(b.cpu.ebx(), 0);
    assert_eq!(a.cpu.ebx(), b.cpu.ebx(), "the sums read back differ");
    for rig in [&mut a, &mut b] {
        // The card draws again: the host gets what was written first.
        let mut words = Vec::new();
        let first = Rect {
            left: 0,
            top: 0,
            width: 4,
            height: 1,
        };
        level1::encode::clear(&mut words, CLEAR_COLOUR, 7, 0.0, 0, &[first]);
        rig.cpu.bus.toadforce_submit(&words);
        rig.cpu.bus.toadforce_flush();
    }
    let memory = |rig: &Rig| rig.cpu.bus.vbe.vram[..0x30_0000].to_vec();
    assert!(memory(&a) == memory(&b), "card memory differs");
    // All of what `cycles=auto` counts, the burst's bytes in the picture
    // and the last write's time too.
    let activity = |rig: &Rig| format!("{:?}", rig.cpu.bus.activity);
    assert_eq!(activity(&a), activity(&b));
    assert_ne!(a.cpu.bus.activity.video_bytes, 0);
    // The interpreter writes an MMX store's 8 bytes as two dwords, the
    // recompiler as one run, as the bus counts `write_to_lfb`'s: one
    // write more of the page in the generation, the same pages written.
    // The host compares stamps only for a change.
    let log = |log: &Log| {
        log.borrow()
            .iter()
            .map(|l| without_generation(l))
            .collect::<Vec<_>>()
    };
    assert_eq!(log(&log_a), log(&log_b));
    assert!(
        log_b.borrow().iter().any(|l| l.starts_with("write")),
        "{:?}",
        log_b.borrow()
    );
    if AVAILABLE && cfg!(target_arch = "x86_64") {
        // Of some 50,000 accesses the bus makes 25,065: the picture's
        // 2000h bytes, the reads of `MIXED`, whose pixels the host has,
        // and a few hundred stores (the first into each surface, which
        // makes its bits, those across a word of its bits, a page or its
        // end, and the first after the pause). The MMX and SSE stores
        // through the bus would add a thousand.
        let calls = b.cpu.dynrec.calls().map_or(0, |c| c.dev + c.slow);
        assert!(calls < 25_500, "{calls} calls");
    }
}

/// A log line without the generation of its stamp.
fn without_generation(line: &str) -> String {
    match line.find("generation: ") {
        Some(at) => {
            let rest = &line[at + 12..];
            let digits = rest
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len());
            format!("{}{}", &line[..at], &rest[digits..])
        }
        None => line.to_string(),
    }
}
