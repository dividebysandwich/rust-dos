//! The Banshee's two command FIFOs: rings of packets in the card's
//! memory that the host writes through the linear frame buffer (BAR1)
//! and the card reads and runs: 2D and 3D register writes, triangle
//! setup vertices, memory, frame buffer and texture writes, jumps and
//! subroutine calls. Glide 3 and 3dfx's Direct3D driver send their work
//! this way.
//!
//! From 86Box's banshee_cmd_write, banshee_write_linear_l (the ring's
//! write pointers and holes) and the CMDFIFO part of vid_voodoo_fifo.c
//! (Sarah Walker, PCem; GPLv2), and chapters 12 and 14 of 3dfx's Voodoo3
//! specification. As with the Voodoo2's (`super::super::cmdfifo`), a
//! packet runs as soon as its last word is written, so the read pointer
//! the host polls for room follows its writes.

use super::super::{Effect, Now, Voodoo};
use super::Agp;

/// A command FIFO's registers and where it is in its ring.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fifo {
    /// The ring's first byte and the byte after it.
    pub base: u32,
    pub end: u32,
    /// cmdBaseSize as written: the size in 4 KB pages less 1, and the
    /// enable (bit 8) and AGP (bit 9) bits.
    pub size: u32,
    pub enabled: bool,
    pub agp: bool,
    /// The byte address of the next word the card reads.
    pub read: u32,
    /// The lowest and highest addresses written since the last word the
    /// card may read, the words it may read, and the words not yet
    /// written between them.
    pub amin: u32,
    pub amax: u32,
    pub depth: u32,
    pub holes: u32,
    pub status: u32,
    /// In a subroutine (packet 0's JSR) and where it returns to.
    pub in_sub: bool,
    pub ret: u32,
}

crate::state_fields!(Fifo {
    base,
    end,
    size,
    enabled,
    agp,
    read,
    amin,
    amax,
    depth,
    holes,
    status,
    in_sub,
    ret
});

impl Fifo {
    fn set_base(&mut self, value: u32) {
        self.base = (value & 0xFFF) << 12;
        self.end = self.base + (((self.size & 0xFF) + 1) << 12);
    }

    fn set_size(&mut self, value: u32) {
        self.size = value;
        self.end = self.base + (((value & 0xFF) + 1) << 12);
        self.enabled = value & 0x100 != 0;
        self.agp = value & 0x200 != 0;
        if !self.enabled {
            self.in_sub = false;
        }
    }

    /// Whether memory address `addr` is in the ring, while it is on.
    pub fn holds(&self, addr: u32) -> bool {
        self.enabled && (self.base..self.end).contains(&addr)
    }

    /// A dword written at memory address `addr`: in its ring, in order or
    /// opening or filling holes (86Box's `banshee_write_linear_l`). True
    /// if it was in the ring.
    pub fn written(&mut self, addr: u32) -> bool {
        if !self.holds(addr) {
            return false;
        }
        if addr == self.base && self.holes == 0 {
            // Back at the start.
            self.amin = self.base;
            self.amax = self.base;
            self.depth = self.depth.wrapping_add(1);
        } else if self.holes > 0 {
            self.holes -= 1;
            if self.holes == 0 {
                self.depth = self
                    .depth
                    .wrapping_add(self.amax.wrapping_sub(self.amin) >> 2);
                self.amin = self.amax;
            }
        } else if addr == self.amax.wrapping_add(4) {
            self.amin = addr;
            self.amax = addr;
            self.depth = self.depth.wrapping_add(1);
        } else {
            if addr < self.amin {
                // Out of order from the start: the write itself is too.
                self.amin = self.base.wrapping_sub(4);
            }
            self.amax = addr;
            self.holes = (self.amax.wrapping_sub(self.amin) >> 2).saturating_sub(1);
        }
        true
    }

    pub fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "enabled": self.enabled,
            "base": format!("{:06X}", self.base),
            "end": format!("{:06X}", self.end),
            "read": format!("{:06X}", self.read),
            "depth": self.depth,
            "holes": self.holes,
        })
    }
}

/// The words a packet takes with its header `header`, the header too.
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
            words += field(12, 1)
                + field(13, 1)
                + field(14, 1)
                + 2 * field(15, 1)
                + field(16, 1)
                + 2 * field(17, 1);
            1 + words * field(6, 4) + field(29, 3)
        }
        4 => 1 + field(15, 14).count_ones() + field(29, 3),
        5 => 2 + field(3, 19).max(1),
        6 => 5,
        _ => 1,
    }
}

/// The words a subroutine may run, against loops.
const SUBROUTINE_WORDS: u32 = 1 << 22;

/// One-time log messages.
const LOG_AGP: u32 = 1 << 12;
const LOG_PACKET: u32 = 1 << 13;

impl Voodoo {
    /// A register of the command FIFOs and the AGP mover read
    /// (`banshee_cmd_read`).
    pub(crate) fn banshee_cmd_read(&self, offset: u32) -> u32 {
        let Some(b) = &self.banshee else {
            return 0xFFFF_FFFF;
        };
        let agp = &b.agp;
        let reg = offset & 0x1FC;
        match reg {
            0x00 => agp.req_size,
            0x04 => agp.host_low,
            0x08 => agp.host_high,
            0x0C => agp.graphics_address,
            0x10 => agp.graphics_stride,
            0x14 => agp.move_cmd,
            0x20..=0x7C => {
                let n = (reg >= 0x50) as usize;
                let f = &b.fifo[n];
                match reg - 0x30 * n as u32 {
                    0x20 => f.base >> 12,
                    0x24 => f.size & 0xFF | (f.enabled as u32) << 8 | (f.agp as u32) << 9,
                    0x2C => f.read,
                    0x30 => 0,
                    0x34 => f.amin,
                    0x3C => f.amax,
                    0x40 => {
                        f.status
                            | ((f.depth > 0 || f.in_sub) as u32) << 11
                            | (f.in_sub as u32) << 27
                    }
                    0x44 => f.depth,
                    0x48 => f.holes,
                    _ => 0,
                }
            }
            _ => 0xFFFF_FFFF,
        }
    }

    /// A register of the command FIFOs and the AGP mover written
    /// (`banshee_cmd_write`).
    pub(crate) fn banshee_cmd_write(
        &mut self,
        offset: u32,
        data: u32,
        now: Now,
        effect: &mut Effect,
    ) {
        let Some(b) = &mut self.banshee else { return };
        let reg = offset & 0x1FC;
        match reg {
            0x00 => b.agp.req_size = data,
            0x04 => b.agp.host_low = data,
            0x08 => b.agp.host_high = data,
            0x0C => b.agp.graphics_address = data,
            0x10 => b.agp.graphics_stride = data,
            0x14 => {
                b.agp.move_cmd = data;
                let Agp { req_size, .. } = b.agp;
                if req_size != 0 {
                    self.log_once(
                        LOG_AGP,
                        "[3DFX] AGP transfers are ignored: the card is on the PCI bus",
                    );
                }
            }
            0x20..=0x7C => {
                let n = (reg >= 0x50) as usize;
                let f = &mut b.fifo[n];
                match reg - 0x30 * n as u32 {
                    0x20 => f.set_base(data),
                    0x24 => f.set_size(data),
                    // cmdBump: the host counts the words it wrote itself.
                    0x28 => f.depth = f.depth.wrapping_add(data & 0xFFFF),
                    0x2C => f.read = data,
                    0x34 => f.amin = data,
                    0x3C => f.amax = data,
                    0x44 => f.depth = data & 0xFFFF,
                    0x48 => f.holes = data & 0xFFFF,
                    _ => {}
                }
                self.run_banshee_fifo(n, now, effect);
            }
            _ => {}
        }
    }

    /// A dword written through BAR1 at memory address `addr`: the words
    /// it completes in a ring run.
    pub(crate) fn banshee_fifo_written(&mut self, addr: u32, now: Now, effect: &mut Effect) {
        for n in 0..2 {
            let written = self
                .banshee
                .as_mut()
                .is_some_and(|b| b.fifo[n].written(addr));
            if written {
                self.run_banshee_fifo(n, now, effect);
            }
        }
    }

    fn fifo_word(&self, addr: u32) -> u32 {
        let ram = &self.fbi.ram;
        let at = ((addr & self.fbi.mask) >> 1) as usize;
        ram.get(at) as u32 | (ram.get((at + 1) % ram.len()) as u32) << 16
    }

    /// The next word of FIFO `n`: in a subroutine from where it reads,
    /// otherwise from the ring, counted off its depth.
    fn fifo_take(&mut self, n: usize) -> u32 {
        let Some(b) = &self.banshee else { return 0 };
        let f = b.fifo[n];
        let word = self.fifo_word(f.read);
        if let Some(b) = &mut self.banshee {
            let f = &mut b.fifo[n];
            f.read = f.read.wrapping_add(4);
            if !f.in_sub {
                f.depth = f.depth.saturating_sub(1);
            }
        }
        word
    }

    /// Run the packets FIFO `n` holds whole, and the subroutines they
    /// call.
    pub(crate) fn run_banshee_fifo(&mut self, n: usize, now: Now, effect: &mut Effect) {
        let mut sub_words = 0u32;
        loop {
            let Some(b) = &self.banshee else { return };
            let f = b.fifo[n];
            // A swap waiting for its retrace holds the FIFO up.
            if !f.enabled || self.banshee_fifo_waits() {
                return;
            }
            let header = self.fifo_word(f.read);
            let needed = words_needed(header);
            if f.in_sub {
                sub_words += needed;
                if sub_words > SUBROUTINE_WORDS {
                    self.log_once(
                        LOG_PACKET,
                        "[3DFX] A command FIFO subroutine that doesn't return was left",
                    );
                    if let Some(b) = &mut self.banshee {
                        b.fifo[n].in_sub = false;
                        b.fifo[n].read = b.fifo[n].ret;
                    }
                    continue;
                }
            } else if f.depth < needed {
                return;
            }
            if f.agp {
                self.log_once(
                    LOG_AGP,
                    "[3DFX] A command FIFO in AGP memory is ignored: the card is on the PCI bus",
                );
                return;
            }
            self.fifo_take(n);
            self.banshee_packet(n, header, now, effect);
        }
    }

    fn banshee_packet(&mut self, n: usize, header: u32, now: Now, effect: &mut Effect) {
        match header & 7 {
            0 => {
                let target = (header >> 4) & 0xFF_FFFC;
                let Some(b) = &mut self.banshee else { return };
                let f = &mut b.fifo[n];
                match (header >> 3) & 7 {
                    1 => {
                        f.ret = f.read;
                        f.read = target;
                        f.in_sub = true;
                    }
                    2 => {
                        f.read = f.ret;
                        f.in_sub = false;
                    }
                    3 => f.read = target,
                    4 => {
                        self.fifo_take(n);
                        self.log_once(
                            LOG_AGP,
                            "[3DFX] A command FIFO jump to AGP memory is ignored",
                        );
                    }
                    _ => {}
                }
            }
            1 => {
                let count = header >> 16;
                let mut addr = (header & 0x7FF8) >> 1;
                for _ in 0..count {
                    let data = self.fifo_take(n);
                    self.banshee_fifo_register(addr, data, now, effect);
                    if header & (1 << 15) != 0 {
                        addr += 4;
                    }
                }
            }
            2 => {
                // The 2D registers from clip0Min, one a mask bit.
                let mut addr = 8;
                let mut mask = header >> 3;
                while mask != 0 {
                    if mask & 1 != 0 {
                        let data = self.fifo_take(n);
                        self.banshee_2d_write(addr, data);
                    }
                    addr += 4;
                    mask >>= 1;
                }
            }
            3 => self.banshee_packet3(n, header, now, effect),
            4 => {
                let mut addr = (header & 0x7FF8) >> 1;
                let mut mask = (header >> 15) & 0x3FFF;
                while mask != 0 {
                    if mask & 1 != 0 {
                        let data = self.fifo_take(n);
                        self.banshee_fifo_register(addr, data, now, effect);
                    }
                    addr += 4;
                    mask >>= 1;
                }
                for _ in 0..header >> 29 {
                    self.fifo_take(n);
                }
            }
            5 => {
                let count = ((header >> 3) & 0x7_FFFF).max(1);
                let mut addr = self.fifo_take(n) & 0xFF_FFFF;
                match header >> 30 {
                    0 | 1 => {
                        if self.writes_queue() {
                            self.catch_up();
                        }
                        let start = addr & self.fbi.mask;
                        for _ in 0..count {
                            let data = self.fifo_take(n);
                            super::blit2d::write(&self.fbi.ram, addr, 4, data, self.fbi.mask);
                            addr += 4;
                        }
                        self.display_dirty = true;
                        self.mirror_memory(start, start + 4 * count);
                    }
                    2 => {
                        // The display changes if the buffer written is
                        // shown (`mark_drawn`).
                        for _ in 0..count {
                            let data = self.fifo_take(n);
                            self.lfb_write(addr >> 2, data, 0xFFFF_FFFF);
                            addr += 4;
                        }
                    }
                    _ => {
                        for _ in 0..count {
                            let data = self.fifo_take(n);
                            self.banshee_texture_write(addr, data);
                            addr += 4;
                        }
                    }
                }
            }
            6 => {
                for _ in 0..4 {
                    self.fifo_take(n);
                }
                self.log_once(
                    LOG_AGP,
                    "[3DFX] A command FIFO AGP transfer is ignored: the card is on the PCI bus",
                );
            }
            _ => self.log_once(
                LOG_PACKET,
                "[3DFX] A command FIFO packet of type 7 is ignored",
            ),
        }
    }

    /// A register a packet writes: with address bit 13 a 2D register,
    /// otherwise a 3D one.
    fn banshee_fifo_register(&mut self, addr: u32, data: u32, now: Now, effect: &mut Effect) {
        if addr & (1 << 13) != 0 {
            self.banshee_2d_write(addr & 0x1FC, data);
        } else {
            self.banshee_3d_register(addr & 0x3FFC, data, now, effect);
        }
    }

    /// Packet type 3: vertices for the triangle setup engine, as the
    /// Voodoo2's.
    fn banshee_packet3(&mut self, n: usize, header: u32, _now: Now, _effect: &mut Effect) {
        use super::super::regs::S_SETUP_MODE;
        use super::super::tsu::Vertex;
        let count = (header >> 6) & 0xF;
        let code = (header >> 3) & 7;
        let bit = |b: u32| header & (1 << b) != 0;
        self.reg[S_SETUP_MODE] = ((header >> 10) & 0xFF) | ((header >> 22) & 0xF) << 16;
        let mut v = Vertex::default();
        let f = |s: &mut Self| f32::from_bits(s.fifo_take(n));
        for i in 0..count {
            v.x = f(self);
            v.y = f(self);
            if bit(28) {
                if bit(10) || bit(11) {
                    let argb = self.fifo_take(n);
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
                    v.r = f(self);
                    v.g = f(self);
                    v.b = f(self);
                }
                if bit(11) {
                    v.a = f(self);
                }
            }
            if bit(12) {
                v.z = f(self);
            }
            if bit(13) {
                v.wb = f(self);
                v.w0 = v.wb;
                v.w1 = v.wb;
            }
            if bit(14) {
                v.w0 = f(self);
                v.w1 = v.w0;
            }
            if bit(15) {
                v.s0 = f(self);
                v.t0 = f(self);
                v.s1 = v.s0;
                v.t1 = v.t0;
            }
            if bit(16) {
                v.w1 = f(self);
            }
            if bit(17) {
                v.s1 = f(self);
                v.t1 = f(self);
            }
            if (code == 1 && i == 0) || (code == 0 && i % 3 == 0) {
                self.start_vertices(v);
            } else {
                self.add_vertex(v);
            }
        }
        for _ in 0..header >> 29 {
            self.fifo_take(n);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_order_writes_count_and_holes_wait() {
        let mut f = Fifo::default();
        f.set_size(0x100 | 1);
        f.set_base(0x10);
        assert_eq!((f.base, f.end), (0x1_0000, 0x1_2000));
        assert!(f.written(0x1_0000));
        assert!(f.written(0x1_0004));
        assert_eq!(f.depth, 2);
        // Skip one word: a hole, nothing ready until it is filled.
        assert!(f.written(0x1_000C));
        assert_eq!((f.depth, f.holes), (2, 1));
        assert!(f.written(0x1_0008));
        assert_eq!((f.depth, f.holes), (4, 0));
        assert!(!f.written(0x2_0000));
    }

    #[test]
    fn packet_sizes() {
        // Type 1: three registers.
        assert_eq!(words_needed(3 << 16 | 1), 4);
        // Type 2: clip0Min and dstBaseAddr.
        assert_eq!(words_needed(0b101 << 3 | 2), 3);
        // Type 5 of no words still carries one.
        assert_eq!(words_needed(5), 3);
        assert_eq!(words_needed(0x20 | 4 << 3), 2);
    }
}
