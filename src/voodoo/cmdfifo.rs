//! The Voodoo2's command FIFO (CMDFIFO): a ring of packets in the frame
//! buffer's memory that the host writes through the upper half of the
//! register space (address bit 21) once fbiInit7 bit 8 turns it on, and
//! that the card reads and runs: register writes, triangle setup
//! vertices, 2D blitter registers and memory writes. Glide 3 and the
//! Voodoo2's Direct3D driver send everything this way.
//!
//! From MAME's `command_fifo` (voodoo_2.cpp), with the packet formats of
//! the Voodoo2 specification and Glide's cvgdefs.h. The card here runs a
//! packet as soon as its last word is written, so the FIFO is about empty
//! whenever the host looks: the read pointer it polls for room follows
//! its writes.

use super::regs::*;
use super::{Effect, Now, Voodoo};
use super::tsu::Vertex;

/// The command FIFO's registers and where it is in its ring.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CmdFifo {
    /// fbiInit7 bit 8.
    pub enabled: bool,
    /// Whether out-of-order writes are counted as holes (fbiInit7 bit 10
    /// clear); without, every write adds a word.
    pub count_holes: bool,
    /// The ring's first byte and the byte after it, in the frame buffer.
    pub base: u32,
    pub end: u32,
    /// The byte address of the next word the card reads.
    pub read: u32,
    /// The lowest and highest addresses written since the last word the
    /// card may read, the words it may read, and the words not yet
    /// written between them.
    pub amin: u32,
    pub amax: u32,
    pub depth: u32,
    pub holes: u32,
}

crate::state_fields!(CmdFifo { enabled, count_holes, base, end, read, amin, amax, depth, holes });

impl CmdFifo {
    /// A word written at byte address `addr`: in order it is ready to
    /// read; out of order it opens or fills holes, and the words below
    /// the highest become ready when the last hole is filled (MAME's
    /// `command_fifo::write`).
    fn count(&mut self, addr: u32) {
        if !self.count_holes || (self.holes == 0 && addr == self.amin.wrapping_add(4)) {
            self.amin = addr;
            self.amax = addr;
            self.depth += 1;
        } else if addr < self.amin {
            // Back to the start of the ring.
            self.holes += addr.wrapping_sub(self.base) / 4;
            self.amin = self.base;
            self.amax = addr;
            self.depth += 1;
        } else if addr < self.amax {
            self.holes = self.holes.saturating_sub(1);
            if self.holes == 0 {
                self.depth += self.amax.wrapping_sub(self.amin) / 4;
                self.amin = self.amax;
            }
        } else {
            self.holes += (addr - self.amax) / 4 - 1;
            self.amax = addr;
        }
    }
}

/// The words a packet takes with its header `header` (MAME's
/// `words_needed`).
fn words_needed(header: u32) -> u32 {
    let field = |shift: u32, bits: u32| (header >> shift) & ((1 << bits) - 1);
    match header & 7 {
        0 => 1 + (field(3, 3) == 4) as u32,
        1 => 1 + field(16, 16),
        2 => 1 + field(3, 29).count_ones(),
        3 => {
            let mut words = 2;
            if header & (1 << 28) != 0 {
                words += (field(10, 2) != 0) as u32;
            } else {
                words += 3 * field(10, 1) + field(11, 1);
            }
            words += field(12, 1) + field(13, 1) + field(14, 1) + 2 * field(15, 1) + field(16, 1) + 2 * field(17, 1);
            1 + words * field(6, 4) + field(29, 3)
        }
        4 => 1 + field(15, 14).count_ones() + field(29, 3),
        5 => 2 + field(3, 19),
        _ => 1,
    }
}

/// One-time log message.
const LOG_PACKET: u32 = 1 << 8;

impl Voodoo {
    /// Whether the command FIFO takes the writes to the upper half of the
    /// register space.
    pub(crate) fn cmdfifo_on(&self) -> bool {
        self.v2() && self.reg[FBI_INIT7] & (1 << 8) != 0
    }

    /// A write to the command FIFO at dword `index` of the window: the
    /// word goes into the ring (byte-swapped with address bit 18), and the
    /// packets it completes run.
    pub(crate) fn cmdfifo_write(&mut self, index: u32, data: u32, now: Now, effect: &mut Effect) {
        let data = if index & (0x4_0000 / 4) != 0 { data.swap_bytes() } else { data };
        let addr = self.cmdfifo.base.wrapping_add((index & 0xFFFF) << 2);
        if addr < self.cmdfifo.end {
            self.fifo_poke(addr, data);
        }
        self.cmdfifo.count(addr);
        self.run_cmdfifo(now, effect);
    }

    /// The dword of the frame buffer at byte address `addr`.
    fn fifo_peek(&self, addr: u32) -> u32 {
        let ram = &self.fbi.ram;
        let at = ((addr & self.fbi.mask) >> 1) as usize;
        ram.get(at) as u32 | (ram.get((at + 1) % ram.len()) as u32) << 16
    }

    fn fifo_poke(&mut self, addr: u32, data: u32) {
        let ram = &self.fbi.ram;
        let at = ((addr & self.fbi.mask) >> 1) as usize;
        ram.set(at, data as u16);
        ram.set((at + 1) % ram.len(), (data >> 16) as u16);
    }

    /// The next word of the ring, read.
    fn fifo_next(&mut self) -> u32 {
        let word = self.fifo_peek(self.cmdfifo.read);
        self.cmdfifo.read = self.cmdfifo.read.wrapping_add(4);
        self.cmdfifo.depth = self.cmdfifo.depth.saturating_sub(1);
        word
    }

    fn fifo_next_float(&mut self) -> f32 {
        f32::from_bits(self.fifo_next())
    }

    /// Run the packets the ring holds whole (`execute_if_ready`).
    pub(crate) fn run_cmdfifo(&mut self, now: Now, effect: &mut Effect) {
        while self.cmdfifo_on() && self.cmdfifo.depth > 0 {
            let header = self.fifo_peek(self.cmdfifo.read);
            if self.cmdfifo.depth < words_needed(header) {
                return;
            }
            self.fifo_next();
            match header & 7 {
                0 => self.packet0(header),
                1 => {
                    let (count, inc, mut target) = (header >> 16, (header >> 15) & 1, (header >> 3) & 0xFFF);
                    for _ in 0..count {
                        let data = self.fifo_next();
                        self.cmdfifo_register(target, data, now, effect);
                        target = target.wrapping_add(inc);
                    }
                }
                2 => {
                    // The 2D registers from bltSrcBaseAddr, one a mask bit.
                    for bit in 3..32 {
                        if header & (1 << bit) != 0 {
                            let data = self.fifo_next();
                            self.cmdfifo_register((BLT_SRC_BASE_ADDR as u32 + bit - 3) | 0x100, data, now, effect);
                        }
                    }
                }
                3 => self.packet3(header),
                4 => {
                    let first = (header >> 3) & 0xFFF;
                    for (target, bit) in (first..).zip(15..29) {
                        if header & (1 << bit) != 0 {
                            let data = self.fifo_next();
                            self.cmdfifo_register(target, data, now, effect);
                        }
                    }
                    self.skip(header >> 29);
                }
                5 => self.packet5(header),
                _ => self.log_once(LOG_PACKET, "[3DFX] A command FIFO packet of type 6 or 7 is ignored"),
            }
        }
    }

    fn skip(&mut self, words: u32) {
        for _ in 0..words {
            self.fifo_next();
        }
    }

    /// A register written by a packet: `target` is the register with the
    /// chip select bits above it, as in the register space.
    fn cmdfifo_register(&mut self, target: u32, data: u32, now: Now, effect: &mut Effect) {
        let mut chips = (target >> 8) & 0xF;
        if chips == 0 {
            chips = 0xF;
        }
        self.register_to((target & 0xFF) as usize, chips & self.chipmask, data, now, effect);
    }

    /// Packet type 0: a NOP or a jump (JSR, RET and the AGP jump are a
    /// Voodoo Banshee's).
    fn packet0(&mut self, header: u32) {
        let target = ((header >> 6) & 0x7F_FFFF) << 2;
        match (header >> 3) & 7 {
            0 => {}
            3 => self.cmdfifo.read = target,
            4 => {
                self.fifo_next();
            }
            _ => self.log_once(LOG_PACKET, "[3DFX] A command FIFO subroutine call is ignored"),
        }
    }

    /// Packet type 3: vertices for the triangle setup engine, each with
    /// the parameters the header's mask asks for, as independent
    /// triangles or a strip or fan.
    fn packet3(&mut self, header: u32) {
        let count = (header >> 6) & 0xF;
        let code = (header >> 3) & 7;
        let bit = |n: u32| header & (1 << n) != 0;
        // The setup mode the packet carries.
        self.reg[S_SETUP_MODE] = ((header >> 10) & 0xFF) | ((header >> 22) & 0xF) << 16;
        let mut v = Vertex::default();
        for n in 0..count {
            v.x = self.fifo_next_float();
            v.y = self.fifo_next_float();
            if bit(28) {
                if bit(10) || bit(11) {
                    let argb = self.fifo_next();
                    if bit(10) {
                        v.r = ((argb >> 16) & 0xFF) as f32;
                        v.g = ((argb >> 8) & 0xFF) as f32;
                        v.b = (argb & 0xFF) as f32;
                    }
                    if bit(11) {
                        v.a = (argb >> 24) as f32;
                    }
                }
            } else {
                if bit(10) {
                    v.r = self.fifo_next_float();
                    v.g = self.fifo_next_float();
                    v.b = self.fifo_next_float();
                }
                if bit(11) {
                    v.a = self.fifo_next_float();
                }
            }
            if bit(12) {
                v.z = self.fifo_next_float();
            }
            if bit(13) {
                v.wb = self.fifo_next_float();
                v.w0 = v.wb;
                v.w1 = v.wb;
            }
            if bit(14) {
                v.w0 = self.fifo_next_float();
                v.w1 = v.w0;
            }
            if bit(15) {
                v.s0 = self.fifo_next_float();
                v.t0 = self.fifo_next_float();
                v.s1 = v.s0;
                v.t1 = v.t0;
            }
            if bit(16) {
                v.w1 = self.fifo_next_float();
            }
            if bit(17) {
                v.s1 = self.fifo_next_float();
                v.t1 = self.fifo_next_float();
            }
            // A new strip, or the first of each three independent
            // vertices, starts over.
            if (code == 1 && n == 0) || (code == 0 && n % 3 == 0) {
                self.start_vertices(v);
            } else {
                self.add_vertex(v);
            }
        }
        self.skip(header >> 29);
    }

    /// Packet type 5: words into the frame buffer's memory as it is, the
    /// linear frame buffer (through lfbMode), or the texture memory.
    fn packet5(&mut self, header: u32) {
        let count = (header >> 3) & 0x7_FFFF;
        let mut target = (self.fifo_next() & 0x1FF_FFFF) / 4;
        // The bytes each word writes: bits 29:26 disable bytes of the
        // first, bits 25:22 of the others (Glide's edge pixels).
        let bytes = |n: u32| {
            let disabled = if n == 0 { (header >> 26) & 0xF } else { (header >> 22) & 0xF };
            (0..4).filter(|b| disabled & (1 << b) == 0).fold(0u32, |m, b| m | 0xFF << (8 * b))
        };
        match header >> 30 {
            0 => {
                // Behind what is drawn.
                self.flush();
                for n in 0..count {
                    let (data, mask) = (self.fifo_next(), bytes(n));
                    let old = self.fifo_peek(target << 2);
                    self.fifo_poke(target << 2, (old & !mask) | (data & mask));
                    target += 1;
                }
                self.display_dirty = true;
                if let Some(mirror) = &mut self.mirror {
                    mirror.invalidate();
                }
            }
            2 => {
                for n in 0..count {
                    let data = self.fifo_next();
                    self.lfb_write(target, data, bytes(n));
                    target += 1;
                }
            }
            3 => {
                for _ in 0..count {
                    let data = self.fifo_next();
                    self.texture_write(target, data);
                    target += 1;
                }
            }
            _ => {
                self.log_once(LOG_PACKET, "[3DFX] Planar YUV writes through the command FIFO are ignored");
                self.skip(count);
            }
        }
    }

    /// cmdFifoBaseAddr, cmdFifoAMin or cmdFifoAMax written (`reg_cmdfifo_w`).
    pub(crate) fn cmdfifo_registers(&mut self) {
        let base = self.reg[CMDFIFO_BASE_ADDR];
        self.cmdfifo.base = (base & 0x3FF) << 12;
        self.cmdfifo.end = (((base >> 16) & 0x3FF) + 1) << 12;
        self.cmdfifo.amin = self.reg[CMDFIFO_AMIN];
        self.cmdfifo.amax = self.reg[CMDFIFO_AMAX];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_sizes() {
        // A NOP and a jump: their header.
        assert_eq!(words_needed(0), 1);
        assert_eq!(words_needed(3 << 3), 1);
        // Type 1: three registers.
        assert_eq!(words_needed(3 << 16 | 1), 4);
        // Type 2: two 2D registers.
        assert_eq!(words_needed(0b101 << 3 | 2), 3);
        // Type 3: three vertices with X, Y, packed ARGB and Z, one pad.
        assert_eq!(words_needed(1 << 29 | 1 << 28 | 1 << 12 | 1 << 10 | 3 << 6 | 3), 1 + 3 * 4 + 1);
        // Type 4: two registers and two pads.
        assert_eq!(words_needed(2 << 29 | 0b11 << 15 | 4), 5);
        // Type 5: the address and two words.
        assert_eq!(words_needed(2 << 3 | 5), 4);
    }

    /// A Voodoo2 with its command FIFO at 100000h-10FFFFh.
    fn card() -> Voodoo {
        let mut v = Voodoo::with_workers(super::super::Board::Voodoo2, 0);
        v.config_write(0x40, 1);
        let reg = |v: &mut Voodoo, r: usize, data: u32| v.write((r as u32) << 2, data, !0, Now::default());
        reg(&mut v, CMDFIFO_BASE_ADDR, 0x10F << 16 | 0x100);
        reg(&mut v, CMDFIFO_AMIN, 0x100000 - 4);
        reg(&mut v, CMDFIFO_AMAX, 0x100000 - 4);
        reg(&mut v, CMDFIFO_RD_PTR, 0x100000);
        reg(&mut v, FBI_INIT7, 1 << 8);
        assert!(v.cmdfifo_on());
        v
    }

    /// Words into the command FIFO from its start, `at` words on.
    fn send(v: &mut Voodoo, at: u32, words: &[u32]) {
        for (i, &w) in words.iter().enumerate() {
            v.write(0x20_0000 + 4 * (at + i as u32), w, !0, Now::default());
        }
    }

    #[test]
    fn packets_write_registers_and_draw() {
        let mut v = card();
        // Type 1: fbzMode and lfbMode (110h, 114h), incrementing.
        send(&mut v, 0, &[2 << 16 | 1 << 15 | (FBZ_MODE as u32) << 3 | 1, 0x0200, 0x0005]);
        assert_eq!((v.reg[FBZ_MODE], v.reg[LFB_MODE]), (0x0200, 0x0005));
        // Writes outside the FIFO are dropped while it is on.
        v.write((FBZ_MODE as u32) << 2, 0x0300, !0, Now::default());
        assert_eq!(v.reg[FBZ_MODE], 0x0200);
        // Type 3: an independent triangle of three X/Y vertices.
        let xy = |x: f32, y: f32| [x.to_bits(), y.to_bits()];
        let mut tri = vec![3 << 6 | 3];
        for (x, y) in [(0.0, 0.0), (0.0, 8.0), (8.0, 0.0)] {
            tri.extend(xy(x, y));
        }
        send(&mut v, 3, &tri);
        assert_eq!(v.reg[FBI_TRIANGLES_OUT], 1);
        assert_eq!(v.cmdfifo.read, 0x100000 + 4 * 10);
        assert_eq!(v.read((CMDFIFO_DEPTH as u32) << 2, Now::default()), 0);
    }

    #[test]
    fn a_jump_goes_back_to_the_start() {
        let mut v = card();
        // NOP, then a jump to the base, then a register write there.
        send(&mut v, 0, &[0, 3 << 3 | (0x100000 >> 2) << 6]);
        assert_eq!(v.cmdfifo.read, 0x100000);
        send(&mut v, 0, &[1 << 16 | (FOG_COLOR as u32) << 3 | 1, 0x123456]);
        assert_eq!(v.reg[FOG_COLOR], 0x123456);
    }

    #[test]
    fn a_packet_waits_for_its_last_word() {
        let mut v = card();
        send(&mut v, 0, &[1 << 16 | (FOG_COLOR as u32) << 3 | 1]);
        assert_eq!(v.reg[FOG_COLOR], 0);
        send(&mut v, 1, &[0xABCDEF]);
        assert_eq!(v.reg[FOG_COLOR], 0xABCDEF);
    }

    #[test]
    fn a_voodoo_graphics_has_no_command_fifo() {
        let mut v = Voodoo::with_workers(super::super::Board::Max, 0);
        v.config_write(0x40, 1);
        v.write((FBI_INIT7 as u32) << 2, 1 << 8, !0, Now::default());
        assert!(!v.cmdfifo_on());
        assert_eq!(v.reg[FBI_INIT7], 0);
    }

    #[test]
    fn writes_in_order_are_ready_at_once() {
        let mut f = CmdFifo { count_holes: true, base: 0x1000, end: 0x2000, amin: 0xFFC, amax: 0xFFC, ..Default::default() };
        f.count(0x1000);
        f.count(0x1004);
        assert_eq!((f.depth, f.holes, f.amin), (2, 0, 0x1004));
    }

    #[test]
    fn a_hole_holds_back_the_words_above_it() {
        let mut f = CmdFifo { count_holes: true, base: 0x1000, end: 0x2000, amin: 0xFFC, amax: 0xFFC, ..Default::default() };
        f.count(0x1000);
        // 1004h is skipped.
        f.count(0x1008);
        assert_eq!((f.depth, f.holes), (1, 1));
        f.count(0x1004);
        assert_eq!((f.depth, f.holes, f.amin), (3, 0, 0x1008));
        // Back to the start of the ring.
        f.count(0x1000);
        assert_eq!((f.depth, f.holes, f.amin), (4, 0, 0x1000));
    }
}
