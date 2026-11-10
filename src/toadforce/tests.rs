//! The card as a guest reaches it: registers, the ring, the error latch
//! and the interrupt sources (chapters 3 to 5 of
//! `docs/toadforce-spec.md`).

use super::command::{self, encode};
use super::format::{Format, Rect, Surface};
use super::regs::{self, Error};
use super::*;
use crate::video::crt::CrtTiming;

const MEMORY: u32 = 32 << 20;

/// A card and its memory.
struct Rig {
    card: ToadForce,
    memory: Vec<u8>,
    ns: u64,
}

impl Rig {
    fn new() -> Self {
        Rig {
            card: ToadForce::new(MEMORY, caps::NEWEST_LEVEL),
            memory: vec![0; MEMORY as usize],
            ns: 0,
        }
    }

    fn live(&self) -> Live {
        Live {
            ns: self.ns,
            vga: CrtTiming::VGA_400,
        }
    }

    fn read(&self, offset: u32) -> u32 {
        self.card.read(offset, self.live())
    }

    fn write(&mut self, offset: u32, value: u32) -> Effects {
        let live = self.live();
        self.card.write(offset, value, &mut self.memory, live)
    }

    /// Put `words` in the ring at byte `at`, round its end.
    fn ring(&mut self, at: u32, words: &[u32]) -> u32 {
        let mut at = at;
        for &word in words {
            self.card.ring_write(at, word, 4);
            at = (at + 4) % regs::RING_SIZE;
        }
        at
    }

    /// Run `words` through the ring from `RING_GET` on.
    fn run(&mut self, words: &[u32]) -> Effects {
        let put = self.ring(self.read(regs::RING_GET), words);
        self.write(regs::RING_PUT, put)
    }

    fn error(&self) -> (u32, u32) {
        (self.read(regs::ERROR), self.read(regs::ERROR_AT))
    }

    /// Show a mode of 640 by 480 in `R5G6B5` at 60 Hz.
    fn mode(&mut self) {
        for (reg, value) in [
            (regs::MODE_WIDTH, 640),
            (regs::MODE_HEIGHT, 480),
            (regs::MODE_FORMAT, Format::R5G6B5.code()),
            (regs::MODE_PITCH, 1280),
            (regs::MODE_REFRESH, 60),
        ] {
            self.write(reg, value);
        }
        assert_eq!(self.write(regs::MODE_ENABLE, 1).display, Change::Mode);
    }

    /// The time the mode's first vertical blank begins at.
    fn first_blank(&self) -> u64 {
        let t = self.card.display.shown().unwrap().timing();
        t.retrace_start as u64 * t.line_ns as u64
    }
}

fn surface(offset: u32) -> Surface {
    Surface {
        offset,
        pitch: 64 * 4,
        width: 64,
        height: 32,
        format: Format::X8R8G8B8,
    }
}

const WHOLE: Rect = Rect {
    left: 0,
    top: 0,
    width: 64,
    height: 32,
};

fn pixel(memory: &[u8], s: &Surface, x: u32, y: u32) -> u32 {
    let at = s.at(x, y);
    u32::from_le_bytes(memory[at..at + 4].try_into().unwrap())
}

#[test]
fn the_identity_registers_read_as_the_card() {
    let rig = Rig::new();
    assert_eq!(rig.read(regs::ID), 0x4441_4F54);
    assert_eq!(&rig.read(regs::ID).to_le_bytes(), b"TOAD");
    assert_eq!(rig.read(regs::LEVEL_MAX), caps::NEWEST_LEVEL);
    assert_eq!(rig.read(regs::LEVEL), 0);
    assert_eq!(rig.read(regs::MEMORY), MEMORY);
    assert_eq!(rig.read(regs::STATUS), 0);
    assert_eq!(rig.read(regs::IRQ_STATUS), 0);
    assert_eq!(rig.read(regs::CAPS), 5);
    assert_eq!(rig.read(regs::CAPS + 4), caps::SURFACE_MAX);
    assert_eq!(rig.read(regs::CAPS + 24), 0);
    // Reserved registers read as 0 and ignore writes.
    let mut rig = rig;
    for offset in [0x030, 0x068, 0x0FC, 0x800, 0xFFFC] {
        rig.write(offset, u32::MAX);
        assert_eq!(rig.read(offset), 0, "{offset:X}h");
    }
    // So do the read-only ones.
    rig.write(regs::ID, 0);
    rig.write(regs::MEMORY, 0);
    rig.write(regs::CAPS, 0);
    assert_eq!(rig.read(regs::ID), regs::ID_VALUE);
    assert_eq!(rig.read(regs::MEMORY), MEMORY);
    assert_eq!(rig.read(regs::CAPS), 5);
    assert_eq!(rig.error(), (0, 0));
}

#[test]
fn a_level_the_card_lacks_is_ignored() {
    let mut rig = Rig::new();
    rig.write(regs::LEVEL, caps::NEWEST_LEVEL + 1);
    assert_eq!(rig.read(regs::LEVEL), 0);
    rig.write(regs::LEVEL, caps::NEWEST_LEVEL);
    assert_eq!(rig.read(regs::LEVEL), caps::NEWEST_LEVEL);
    // A card told to have fewer levels has no more than it has.
    assert_eq!(ToadForce::new(MEMORY, 9).level_max(), caps::NEWEST_LEVEL);
    assert_eq!(rig.error(), (0, 0));
}

#[test]
fn the_ring_runs_up_to_put_before_the_write_returns() {
    let mut rig = Rig::new();
    let s = surface(0x1000);
    let mut words = Vec::new();
    encode::nop(&mut words, 3);
    encode::fill(&mut words, &s, &WHOLE, 0x0012_3456);
    let fx = rig.run(&words);
    assert_eq!(rig.read(regs::RING_PUT), 4 * words.len() as u32);
    assert_eq!(rig.read(regs::RING_GET), rig.read(regs::RING_PUT));
    assert_eq!(pixel(&rig.memory, &s, 63, 31), 0x0012_3456);
    assert_eq!(fx.drawn, vec![s.span(&WHOLE)]);
    assert_eq!(rig.error(), (0, 0));
    // What the guest wrote in the ring reads back, by bytes too.
    assert_eq!(rig.card.ring_read(0, 4), command::header(command::NOP, 3));
    assert_eq!(rig.card.ring_read(12, 4), words[3]);
    assert_eq!(rig.card.ring_read(13, 2), words[3] >> 8 & 0xFFFF);
    rig.card.ring_write(13, 0xAB, 1);
    assert_eq!(rig.card.ring_read(12, 4), words[3] & !0xFF00 | 0xAB00);
    // A write of the same value runs nothing more.
    let put = rig.read(regs::RING_PUT);
    rig.memory.fill(0);
    assert_eq!(rig.write(regs::RING_PUT, put), Effects::default());
    assert_eq!(pixel(&rig.memory, &s, 0, 0), 0);
}

#[test]
fn commands_go_round_the_rings_end() {
    let mut rig = Rig::new();
    // Up to three words before the end with no-operations.
    let near = regs::RING_SIZE - 12;
    let mut filler = Vec::new();
    while filler.len() < (near / 4) as usize {
        let left = (near / 4) as usize - filler.len();
        encode::nop(&mut filler, left.min(0xFFFF));
    }
    rig.run(&filler);
    assert_eq!(rig.read(regs::RING_GET), near);
    // A fill that begins there and ends 8 words into the ring.
    let s = surface(0);
    let mut words = Vec::new();
    encode::fill(&mut words, &s, &WHOLE, 0x00AB_CDEF);
    let put = rig.ring(near, &words);
    assert_eq!(put, 4 * 8);
    rig.write(regs::RING_PUT, put);
    // It was cut at the end: the part there is no whole command, which
    // stops the run and loses what follows.
    assert_eq!(
        rig.error(),
        (Error::BadLength as u32, near),
        "a command doesn't go round the end"
    );
    assert_eq!(rig.read(regs::RING_GET), put);
    assert_eq!(pixel(&rig.memory, &s, 0, 0), 0);

    // The driver's way: a no-operation up to the end, the command at 0.
    let mut rig = Rig::new();
    rig.run(&filler);
    let mut words = Vec::new();
    encode::nop(&mut words, 3);
    encode::fill(&mut words, &s, &WHOLE, 0x00AB_CDEF);
    let put = rig.ring(near, &words);
    assert_eq!(put, 4 * 11);
    rig.write(regs::RING_PUT, put);
    assert_eq!(rig.error(), (0, 0));
    assert_eq!(pixel(&rig.memory, &s, 5, 5), 0x00AB_CDEF);
    assert_eq!(rig.read(regs::RING_GET), put);
}

#[test]
fn a_put_that_is_no_offset_of_a_word_is_rejected() {
    let mut rig = Rig::new();
    for put in [2, 0x10_0000, 0xFFFF_FFFC, 7] {
        rig.write(regs::RING_PUT, put);
        assert_eq!(
            rig.error(),
            (Error::BadPut as u32, regs::AT_REGISTER),
            "{put:X}h"
        );
        assert_eq!(rig.read(regs::RING_PUT), 0);
        rig.write(regs::ERROR, 0);
    }
    rig.write(regs::RING_PUT, regs::RING_PUT_MAX);
    assert_eq!(rig.error(), (Error::BadLength as u32, 0), "a ring of zeros");
}

#[test]
fn a_rejected_command_is_skipped_and_the_first_error_stays() {
    let mut rig = Rig::new();
    let s = surface(0x2000);
    let outside = Rect { left: 1, ..WHOLE };
    let mut words = Vec::new();
    encode::fill(&mut words, &s, &outside, 1); // rejected: range
    let second = words.len();
    words.push(command::header(0x7FFF, 2)); // rejected: opcode
    words.push(0);
    encode::fill(&mut words, &s, &WHOLE, 0x55); // runs
    let fx = rig.run(&words);
    assert_eq!(rig.error(), (Error::BadRange as u32, 0));
    assert_eq!(
        rig.read(regs::STATUS) & regs::STATUS_ERROR,
        regs::STATUS_ERROR
    );
    assert_eq!(rig.read(regs::IRQ_STATUS), regs::IRQ_ERROR);
    assert_eq!(pixel(&rig.memory, &s, 0, 0), 0x55);
    assert_eq!(fx.drawn.len(), 1);
    // Cleared, the next rejection latches: with where it was.
    rig.write(regs::ERROR, 0xFFFF_FFFF);
    assert_eq!(rig.error(), (0, 0));
    assert_eq!(rig.read(regs::STATUS) & regs::STATUS_ERROR, 0);
    let base = rig.read(regs::RING_GET);
    rig.run(&words[second..]);
    assert_eq!(rig.error(), (Error::BadOpcode as u32, base));
    // A length that leaves no next command drops the rest.
    rig.write(regs::ERROR, 0);
    rig.memory.fill(0);
    let base = rig.read(regs::RING_GET);
    let mut words = vec![
        command::header(command::NOP, 1),
        command::header(command::NOP, 0),
    ];
    encode::fill(&mut words, &s, &WHOLE, 0x55);
    rig.run(&words);
    assert_eq!(rig.error(), (Error::BadLength as u32, base + 4));
    assert_eq!(rig.read(regs::RING_GET), base + 4 * words.len() as u32);
    assert_eq!(pixel(&rig.memory, &s, 0, 0), 0);
}

#[test]
fn submitted_words_run_as_the_rings_do() {
    let s = surface(0x4000);
    let d = surface(0x8000);
    let half = Rect {
        width: 32,
        height: 16,
        ..WHOLE
    };
    let mut words = Vec::new();
    encode::fill(&mut words, &s, &WHOLE, 0x0011_2233);
    encode::fill(&mut words, &s, &half, 0x0044_5566);
    encode::blt(&mut words, &d, &WHOLE, &s, &half, None);
    encode::blt(&mut words, &d, &half, &s, &WHOLE, Some(0x0011_2233));
    encode::flip(&mut words, 0x4000, false);
    let outside = Rect { top: 1, ..WHOLE };
    encode::fill(&mut words, &d, &outside, 9);

    let mut ring = Rig::new();
    let ring_fx = ring.run(&words);
    let mut direct = Rig::new();
    let live = direct.live();
    let direct_fx = direct.card.submit(&words, &mut direct.memory, live);
    assert_eq!(ring_fx, direct_fx);
    assert!(ring.memory == direct.memory);
    assert_eq!(ring.read(regs::MODE_START), 0x4000);
    assert_eq!(direct.read(regs::MODE_START), 0x4000);
    assert_eq!(ring.read(regs::IRQ_STATUS), direct.read(regs::IRQ_STATUS));
    // The rejected command is told apart by where it was.
    assert_eq!(ring.error().0, Error::BadRange as u32);
    assert_eq!(direct.error(), (Error::BadRange as u32, regs::AT_DIRECT));
    assert_eq!(ring.error().1, 4 * (words.len() as u32 - 11));
    // Submitting leaves the ring's offsets alone.
    assert_eq!(direct.read(regs::RING_PUT), 0);
}

#[test]
fn a_bad_mode_latches_and_shows_nothing() {
    let mut rig = Rig::new();
    rig.write(regs::MODE_WIDTH, 640);
    let fx = rig.write(regs::MODE_ENABLE, 1);
    assert_eq!(fx.display, Change::None);
    assert_eq!(rig.error(), (Error::BadMode as u32, regs::AT_REGISTER));
    assert_eq!(rig.read(regs::MODE_ENABLE), 0);
    rig.write(regs::ERROR, 0);
    rig.mode();
    assert_eq!(rig.read(regs::MODE_ENABLE), 1);
    // A start that leaves the surface outside card memory.
    let fx = rig.write(regs::MODE_START, MEMORY - 16);
    assert_eq!(fx.display, Change::None);
    assert_eq!(rig.error(), (Error::BadRange as u32, regs::AT_REGISTER));
    assert_eq!(rig.read(regs::MODE_START), 0);
}

#[test]
fn the_blank_raises_its_interrupt_once_enabled() {
    let mut rig = Rig::new();
    rig.mode();
    assert_eq!(rig.card.irq(), None);
    // Nothing is waited for: no event.
    assert_eq!(rig.card.next_event(), None);
    rig.write(regs::IRQ_ENABLE, 0xFFFF_FFFF);
    assert_eq!(rig.read(regs::IRQ_ENABLE), regs::IRQ_ALL);
    let blank = rig.first_blank();
    assert_eq!(rig.card.next_event(), Some(blank));
    // A read sees the bit as soon as the blank began, and changes
    // nothing: the line goes up when the card is served.
    rig.ns = blank;
    assert_eq!(rig.read(regs::IRQ_STATUS), regs::IRQ_VBLANK);
    assert_eq!(rig.read(regs::FRAME), 1);
    assert_eq!(rig.card.irq(), None);
    let live = rig.live();
    assert_eq!(rig.card.service(live), Effects::default());
    assert_eq!(rig.card.irq(), Some(11));
    // Up, the blank's interrupt asks for no further event.
    assert_eq!(rig.card.next_event(), None);
    // Writing 1 clears the source, and the next blank is waited for.
    rig.write(regs::IRQ_STATUS, regs::IRQ_VBLANK);
    assert_eq!(rig.read(regs::IRQ_STATUS), 0);
    assert_eq!(rig.card.irq(), None);
    let frame = rig.card.display.shown().unwrap().timing().frame_ns();
    assert_eq!(rig.card.next_event(), Some(blank + frame));
    // A line the system didn't give is none.
    rig.ns = blank + frame;
    let live = rig.live();
    rig.card.service(live);
    rig.card.pci.write(0x3C, 0xFF);
    assert_eq!(rig.card.irq(), None);
    rig.card.pci.write(0x3C, 5);
    assert_eq!(rig.card.irq(), Some(5));
}

#[test]
fn a_waiting_flip_shows_at_the_blank_and_says_so() {
    let mut rig = Rig::new();
    rig.mode();
    let mut words = Vec::new();
    encode::flip(&mut words, 0x10_0000, true);
    let fx = rig.run(&words);
    assert_eq!(fx.display, Change::None);
    assert_eq!(rig.read(regs::MODE_START), 0);
    assert_eq!(rig.read(regs::STATUS), regs::STATUS_FLIP_PENDING);
    assert_eq!(rig.read(regs::IRQ_STATUS), 0);
    let blank = rig.first_blank();
    assert_eq!(rig.card.next_event(), Some(blank));
    rig.ns = blank - 1;
    assert_eq!(rig.read(regs::MODE_START), 0);
    rig.ns = blank;
    // A read at the blank already sees the flip made.
    assert_eq!(rig.read(regs::MODE_START), 0x10_0000);
    assert_eq!(rig.read(regs::STATUS), regs::STATUS_VBLANK);
    assert_eq!(
        rig.read(regs::IRQ_STATUS),
        regs::IRQ_VBLANK | regs::IRQ_FLIP
    );
    let live = rig.live();
    assert_eq!(rig.card.service(live).display, Change::Picture);
    assert_eq!(rig.card.next_event(), None);
    // One that doesn't wait shows at once.
    let mut words = Vec::new();
    encode::flip(&mut words, 0x20_0000, false);
    rig.write(regs::IRQ_STATUS, regs::IRQ_ALL);
    assert_eq!(rig.run(&words).display, Change::Picture);
    assert_eq!(rig.read(regs::MODE_START), 0x20_0000);
    assert_eq!(rig.read(regs::IRQ_STATUS), regs::IRQ_FLIP);
}

#[test]
fn reset_keeps_memory_the_ring_and_configuration_space() {
    let mut rig = Rig::new();
    rig.mode();
    let s = surface(0);
    let mut words = Vec::new();
    encode::fill(&mut words, &s, &WHOLE, 0x77);
    encode::flip(&mut words, 0x1000, true);
    words.push(command::header(0x7FFF, 1));
    rig.run(&words);
    rig.write(regs::IRQ_ENABLE, regs::IRQ_ALL);
    rig.write(regs::PALETTE, 0x12_3456);
    rig.card.pci.write(0x3C, 5);
    assert_ne!(rig.error(), (0, 0));

    let fx = rig.write(regs::RESET, 1);
    assert_eq!(fx.display, Change::Mode);
    for offset in [
        regs::LEVEL,
        regs::STATUS,
        regs::IRQ_STATUS,
        regs::IRQ_ENABLE,
        regs::RING_PUT,
        regs::RING_GET,
        regs::ERROR,
        regs::ERROR_AT,
        regs::MODE_ENABLE,
        regs::MODE_WIDTH,
        regs::MODE_START,
        regs::PALETTE,
    ] {
        assert_eq!(rig.read(offset), 0, "{offset:X}h");
    }
    assert!(rig.card.display.shown().is_none());
    assert_eq!(rig.card.next_event(), None);
    assert_eq!(pixel(&rig.memory, &s, 0, 0), 0x77);
    assert_eq!(rig.card.ring_read(0, 4), words[0]);
    assert_eq!(rig.card.pci.interrupt_line, 5);
    // A write of 0 does nothing.
    rig.mode();
    assert_eq!(rig.write(regs::RESET, 0), Effects::default());
    assert_eq!(rig.read(regs::MODE_ENABLE), 1);
}

#[test]
fn a_saved_card_loads_as_it_was() {
    use crate::savestate::{Reader, State, Writer};
    let mut rig = Rig::new();
    rig.mode();
    let mut words = Vec::new();
    encode::flip(&mut words, 0x1000, true);
    words.push(command::header(0x7FFF, 1));
    rig.run(&words);
    rig.write(regs::IRQ_ENABLE, regs::IRQ_VBLANK);
    rig.write(regs::PALETTE + 4, 0xABCDEF);
    rig.card.pci.write(0x13, 0xC0);
    let mut w = Writer::new();
    rig.card.save(&mut w);
    let bytes = w.buf;
    let mut loaded = ToadForce::new(MEMORY, caps::NEWEST_LEVEL);
    loaded.load(&mut Reader::new(&bytes)).unwrap();
    rig.ns = rig.first_blank() + 5;
    let live = rig.live();
    for offset in (0..0x800).step_by(4) {
        assert_eq!(loaded.read(offset, live), rig.read(offset), "{offset:X}h");
    }
    assert_eq!(loaded.ring_read(0, 4), words[0]);
    assert_eq!(loaded.next_event(), rig.card.next_event());
    assert_eq!(loaded.pci.read(0x13), 0xC0);
}
