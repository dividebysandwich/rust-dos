//! The 3dfx card on the bus: its 16 MB window where BAR0 puts it, with
//! the access sizes the card takes (DOSBox-X's `Voodoo_PageHandler`), the
//! time its writes take when its FIFO is full, its swaps' retraces among
//! the timer events, and putting it in or taking it out.

use super::{Bus, Hold};
use crate::voodoo::lfb::MemoryRun;
use crate::voodoo::{Board, Effect, Now, Voodoo, WINDOW};

/// Where the bytes of a run written into the card come from
/// (`Bus::run_to_voodoo`).
#[derive(Clone, Copy)]
enum RunSource<'a> {
    /// Plain RAM from this address on: a string move's source.
    Ram(usize),
    /// These bytes: a store's.
    Bytes(&'a [u8]),
}

impl Bus {
    /// Put in the 3dfx card `board` (None: take it out). A card already
    /// there of the same kind stays as it is.
    pub fn configure_voodoo(&mut self, board: Option<Board>) {
        if self.voodoo.as_ref().map(|v| v.board) == board {
            return;
        }
        self.lower_banshee_irq();
        self.voodoo = board.map(Voodoo::new);
        if let Some(v) = &mut self.voodoo {
            v.frame_cap = self.frame_cap.clone();
        }
        if board.is_some_and(|b| b.banshee().is_some()) {
            self.banshee_settle();
        } else {
            self.vga.svga_ext = None;
            self.vga.invalidate_timing();
        }
        // The accesses of a session, for `tests/video/voodoo_tests.rs` to
        // replay. A recording names a Voodoo Graphics or Voodoo2, not a
        // Banshee.
        if let (Some(v), Ok(path)) = (&mut self.voodoo, std::env::var("RUST_DOS_VOODOO_RECORD")) {
            if v.board.banshee().is_some() {
                self.log_string("[3DFX] Can't record a Banshee or Voodoo3's accesses");
            } else if let Err(e) = v.record_into(std::path::Path::new(&path)) {
                self.log_string(&format!("[3DFX] Can't record into {}: {}", path, e));
            }
        }
        self.vga.mark_dirty_full();
        self.clock.schedule(self.next_event());
        // Glide's DLLs come and go with the card.
        crate::win32::dll_files::sync(self);
    }

    /// A PCI reset of the 3dfx card, as at power-on.
    pub fn reset_voodoo(&mut self) {
        self.lower_banshee_irq();
        if let Some(v) = &mut self.voodoo {
            v.reset();
            self.vga.mark_dirty_full();
        }
        self.banshee_settle();
    }

    /// The card's BAR0 or video clock changed, or a Banshee's BARs or
    /// interrupt line: a request it has up moves to the new line.
    pub(crate) fn voodoo_moved(&mut self) {
        self.vga.mark_dirty_full();
        self.sync_banshee_irq();
    }

    /// The time, as the card needs it.
    pub(crate) fn voodoo_now(&self) -> Now {
        Now { ns: self.clock.now_ns(), ticks: self.clock.now_ticks() }
    }

    /// The offset of `addr` in the card's window, if it is in it. The
    /// window counts where it is above the RAM and below the BIOS ROM's
    /// mirror at the top.
    #[inline]
    pub fn voodoo_at(&self, addr: usize) -> Option<u32> {
        let v = self.voodoo.as_ref()?;
        if let Some(b) = v.banshee() {
            return b.window_at(addr as u32, self.ram.len());
        }
        let base = v.base()? as usize;
        (addr >= base && addr < base + WINDOW as usize && base >= self.ram.len() && addr < 0xFFFE_0000)
            .then(|| (addr - base) as u32)
    }

    pub(crate) fn voodoo_read_16(&self, offset: u32) -> u16 {
        let Some(v) = &self.voodoo else { return 0xFFFF };
        if v.banshee.is_some() {
            return self.banshee_read_bytes(offset, 2) as u16;
        }
        if offset & 1 != 0 {
            return 0xFFFF;
        }
        let dword = v.read(offset & !3, self.voodoo_now());
        if offset & 2 != 0 { (dword >> 16) as u16 } else { dword as u16 }
    }

    pub(crate) fn voodoo_read_32(&self, offset: u32) -> u32 {
        let Some(v) = &self.voodoo else { return 0xFFFF_FFFF };
        if v.banshee.is_some() {
            return self.banshee_read_bytes(offset, 4);
        }
        let now = self.voodoo_now();
        match offset & 3 {
            0 => v.read(offset, now),
            2 => (v.read(offset & !3, now) >> 16) | v.read((offset & !3).wrapping_add(4), now) << 16,
            _ => 0xFFFF_FFFF,
        }
    }

    pub(crate) fn voodoo_write_16(&mut self, offset: u32, value: u16) {
        if self.voodoo.as_ref().is_some_and(|v| v.banshee.is_some()) {
            self.banshee_write_bytes(offset, value as u32, 2);
            return;
        }
        if offset & 1 != 0 {
            return;
        }
        if offset & 2 != 0 {
            self.voodoo_write(offset & !3, (value as u32) << 16, 0xFFFF_0000);
        } else {
            self.voodoo_write(offset, value as u32, 0x0000_FFFF);
        }
    }

    pub(crate) fn voodoo_write_32(&mut self, offset: u32, value: u32) {
        if self.voodoo.as_ref().is_some_and(|v| v.banshee.is_some()) {
            self.banshee_write_bytes(offset, value, 4);
            return;
        }
        match offset & 3 {
            0 => self.voodoo_write(offset, value, 0xFFFF_FFFF),
            2 => {
                self.voodoo_write(offset & !3, value << 16, 0xFFFF_0000);
                self.voodoo_write((offset & !3).wrapping_add(4), value >> 16, 0x0000_FFFF);
            }
            // Odd: the two dwords it touches, read, merged and written.
            odd => {
                let (first, second) = (offset & !3, (offset & !3).wrapping_add(4));
                let (a, b) = (self.voodoo_read_32(first), self.voodoo_read_32(second));
                let (a, b) = if odd == 1 {
                    ((a & 0xFF_FFFF) | (value & 0xFF) << 24, (b & 0xFF00_0000) | value >> 8)
                } else {
                    ((a & 0xFF) | (value & 0xFF_FFFF) << 8, (b & 0xFFFF_FF00) | value >> 24)
                };
                self.voodoo_write(first, a, 0xFFFF_FFFF);
                self.voodoo_write(second, b, 0xFFFF_FFFF);
            }
        }
    }

    /// A byte written to the window, which a Voodoo Graphics or Voodoo2
    /// ignores and a Banshee takes.
    pub(crate) fn voodoo_write_8(&mut self, offset: u32, value: u8) {
        if self.voodoo.as_ref().is_some_and(|v| v.banshee.is_some()) {
            self.banshee_write_bytes(offset, value as u32, 1);
            return;
        }
        if let Some(v) = &mut self.voodoo {
            v.byte_write();
        }
        self.voodoo_log();
    }

    /// A byte read from the window: a Banshee's, open bus otherwise.
    pub(crate) fn voodoo_read_8(&self, offset: u32) -> u8 {
        if self.voodoo.as_ref().is_some_and(|v| v.banshee.is_some()) {
            return self.banshee_read_bytes(offset, 1) as u8;
        }
        0xFF
    }

    /// Write the `len` bytes of plain RAM at `src` into the 3dfx card's
    /// window at `dst`, a dword at a time in order, as a string move's
    /// writes through `write_32` do (a game's 2D picture copied into the
    /// linear frame buffer row by row), in one call. Returns false,
    /// having written nothing, where the elements aren't dwords going up,
    /// or the range isn't all memory of the card's (`Voodoo::memory_run`).
    pub fn move_to_voodoo(
        &mut self,
        src: usize,
        dst: usize,
        len: usize,
        size: usize,
        backward: bool,
    ) -> bool {
        if size != 4 || backward {
            return false;
        }
        self.run_to_voodoo(dst, len, RunSource::Ram(src))
    }

    /// Write `bytes` into the 3dfx card's window at `dst` as the dwords
    /// of `write_32` from the first to the last would, in one call: an MMX
    /// or SSE store of 8 or 16 bytes, or a single dword, into the linear
    /// frame buffer. False, having written nothing, where `move_to_voodoo`
    /// is false for them.
    pub(crate) fn write_to_voodoo(&mut self, dst: usize, bytes: &[u8]) -> bool {
        self.run_to_voodoo(dst, bytes.len(), RunSource::Bytes(bytes))
    }

    /// `move_to_voodoo` and `write_to_voodoo`: `len` bytes from `src` at
    /// `dst`.
    fn run_to_voodoo(&mut self, dst: usize, len: usize, src: RunSource<'_>) -> bool {
        if self.voodoo.is_none() || dst & 3 != 0 || len == 0 || !len.is_multiple_of(4) {
            return false;
        }
        let Some(first) = self.voodoo_at(dst) else {
            return false;
        };
        let last = dst + len - 4;
        // `write_32` gives the VESA frame buffer and the S3's and Mach64's
        // registers the dwords of theirs before the card: a run that
        // starts or ends in one of them goes a dword at a time.
        let other = |at: usize| {
            self.vbe.lfb_offset(at, 4).is_some()
                || self.s3_mmio(at).is_some()
                || self.mach64_mmio(at).is_some()
                || self.toadforce_at(at).is_some()
        };
        let len32 = len as u32;
        if self.voodoo_at(last) != Some(first + len32 - 4) || other(dst) || other(last) {
            return false;
        }
        let Some(run) = self
            .voodoo
            .as_ref()
            .and_then(|v| v.memory_run(first, len32))
        else {
            return false;
        };
        let _profile = self
            .profile
            .as_deref()
            .map(|p| p.mem_run(dst, true, (len / 4) as u64));
        if self.observe.on {
            self.observe.mapped_write(dst, self.ram.len());
        }
        if run == MemoryRun::Fifo {
            for i in (0..len).step_by(4) {
                let value = match src {
                    RunSource::Ram(at) => &self.ram[at + i..at + i + 4],
                    RunSource::Bytes(bytes) => &bytes[i..i + 4],
                };
                let value = u32::from_le_bytes(value.try_into().unwrap());
                self.voodoo_write_32(first + i as u32, value);
            }
            return true;
        }
        let _zone = crate::profile::zone(crate::profile::Zone::Voodoo);
        self.voodoo_swap_wait();
        let Some(v) = &mut self.voodoo else {
            return true;
        };
        let data = match src {
            RunSource::Ram(at) => &self.ram[at..at + len],
            RunSource::Bytes(bytes) => bytes,
        };
        match run {
            MemoryRun::Lfb(index) => {
                let dwords = data.as_chunks::<4>().0.iter();
                v.lfb_write_run(index, dwords.map(|&d| u32::from_le_bytes(d)));
            }
            MemoryRun::Linear(addr) => {
                v.banshee_memory_write(addr, data);
                self.note_banshee_memory_write(addr, len as u32);
            }
            MemoryRun::Fifo => {}
        }
        true
    }

    /// Where the `len` bytes of the bus from `addr` on are in a Banshee's
    /// or Voodoo3's memory, all of them, through BAR1 below the tile base
    /// and outside the command FIFOs' rings (`Voodoo::linear_run`): the
    /// first byte's address in the card's memory. None where any of them
    /// is somewhere else, such as a register or another card's window,
    /// which reads and writes reach a byte at a time.
    fn banshee_linear_at(&self, addr: usize, len: usize) -> Option<u32> {
        let v = self.voodoo.as_ref().filter(|v| v.banshee.is_some())?;
        let last = addr.checked_add(len.checked_sub(1)?)?;
        let first = self.voodoo_at(addr)?;
        // `read_8` and `write_8` give these their bytes before the card.
        let other = |at: usize| {
            self.vbe.lfb_offset(at, 1).is_some()
                || self.s3_mmio(at).is_some()
                || self.mach64_mmio(at).is_some()
                || self.powervr_at(at).is_some()
                || self.toadforce_at(at).is_some()
        };
        if self.voodoo_at(last) != Some(first + (len - 1) as u32) || other(addr) || other(last) {
            return None;
        }
        v.linear_run(first, len as u32)
    }

    /// Fill `data` from a Banshee's or Voodoo3's memory at `src` in one
    /// copy, as reads of its bytes one at a time would, the drawing
    /// queued before done first (`Voodoo::banshee_memory_read`): a
    /// service reading a surface in the card's memory, such as GDI
    /// loading the rows it blits onto. False, having read nothing, where
    /// the bytes aren't all the card's memory (`banshee_linear_at`).
    pub(crate) fn read_from_voodoo(&self, src: usize, data: &mut [u8]) -> bool {
        let Some(addr) = self.banshee_linear_at(src, data.len()) else {
            return false;
        };
        // Counted as the dwords of a 32-bit program's copy.
        let _profile = self
            .profile
            .as_deref()
            .map(|p| p.mem_run(src, false, data.len().div_ceil(4) as u64));
        if self.observe.on {
            self.observe.mapped(src, self.ram.len());
        }
        if let Some(v) = &self.voodoo {
            v.banshee_memory_read(addr, data);
        }
        true
    }

    /// Write `data` into a Banshee's or Voodoo3's memory at `dst` in one
    /// copy, as writes of its bytes one at a time would, `cycles=auto`
    /// hearing of what lands in the desktop shown. False, having written
    /// nothing, where `read_from_voodoo` is false for them.
    pub(crate) fn bytes_to_voodoo(&mut self, dst: usize, data: &[u8]) -> bool {
        let Some(addr) = self.banshee_linear_at(dst, data.len()) else {
            return false;
        };
        let _profile = self
            .profile
            .as_deref()
            .map(|p| p.mem_run(dst, true, data.len().div_ceil(4) as u64));
        if self.observe.on {
            self.observe.mapped_write(dst, self.ram.len());
        }
        let _zone = crate::profile::zone(crate::profile::Zone::Voodoo);
        self.voodoo_swap_wait();
        if let Some(v) = &mut self.voodoo {
            v.banshee_memory_write(addr, data);
        }
        self.note_banshee_memory_write(addr, data.len() as u32);
        true
    }

    /// A write behind a swap the frame rate cap holds back (the rest of
    /// the code the program ran before it was held) waits for the swap.
    fn voodoo_swap_wait(&mut self) {
        if let Some(due) = self.voodoo.as_ref().and_then(|v| v.deferred_swap()) {
            self.clock.stall_to(due);
            let now = self.voodoo_now();
            if let Some(v) = &mut self.voodoo {
                v.service(now);
            }
        }
    }

    pub(crate) fn voodoo_write(&mut self, offset: u32, value: u32, mask: u32) {
        let _profile = crate::profile::zone(crate::profile::Zone::Voodoo);
        self.voodoo_swap_wait();
        let now = self.voodoo_now();
        let Some(v) = &mut self.voodoo else { return };
        let effect = v.write(offset, value, mask, now);
        self.voodoo_effect(effect);
        // A Banshee's desktop start, its init register in BAR0 (and in
        // the I/O BAR, which comes here), while the video processor shows
        // the desktop: a guest driver's page flip. A mode set writes it
        // with the video processor off, which shows nothing new.
        if offset < 0x8_0000
            && offset & 0xFC == crate::voodoo::banshee::VID_DESKTOP_START_ADDR
            && self.banshee().is_some_and(|b| b.extended())
        {
            self.note_display_start();
        }
    }

    /// Texture data put into a Banshee's or Voodoo3's memory from byte
    /// `addr` on, as a burst of writes through TMU 0's texture space
    /// would put it, in one copy: Glide's texture downloads.
    pub(crate) fn banshee_texture_download(&mut self, addr: u32, data: &[u8]) {
        let _profile = crate::profile::zone(crate::profile::Zone::Voodoo);
        self.voodoo_swap_wait();
        if let Some(v) = &mut self.voodoo
            && v.banshee.is_some()
        {
            v.banshee_texture_download(addr, data);
        }
    }

    /// Carry out what a write asked for: the CPU waiting for a full FIFO,
    /// a swap's retrace among the timer events, the program held until a
    /// swap's turn under the frame rate cap.
    pub(crate) fn voodoo_effect(&mut self, effect: Effect) {
        if let Some(to) = effect.stall_to {
            self.clock.stall_to(to);
            let now = self.voodoo_now();
            if let Some(v) = &mut self.voodoo {
                v.service(now);
            }
        }
        if let Some(to) = effect.hold_to {
            self.hold = Some(Hold { until: to, at: None });
            // Out of the code running, to the loop that holds it.
            self.clock.deadline = self.clock.icount;
        }
        if effect.reschedule || effect.stall_to.is_some() {
            self.clock.schedule(self.next_event());
        }
        if effect.settle {
            self.banshee_settle_ext();
        }
        self.voodoo_log();
    }

    fn voodoo_log(&mut self) {
        let lines = self.voodoo.as_mut().map(|v| std::mem::take(&mut v.log)).unwrap_or_default();
        for line in lines {
            self.log_string(&line);
        }
    }

    pub(crate) fn voodoo_next_event(&self) -> Option<u64> {
        self.voodoo.as_ref().and_then(|v| v.next_event())
    }

    /// Retire the swaps whose retraces came.
    pub(crate) fn voodoo_service(&mut self) {
        let _profile = crate::profile::zone(crate::profile::Zone::Voodoo);
        let now = self.voodoo_now();
        if let Some(v) = &mut self.voodoo {
            v.service(now);
        }
    }

    /// Texture sampling for both the software and mirrored OpenGL draws.
    pub fn set_voodoo_texture_sampling(&mut self, sampling: crate::voodoo::TextureSampling) {
        if let Some(v) = &mut self.voodoo {
            v.set_texture_sampling(sampling);
        }
    }

    /// For the debugger's status: what the 3dfx card is doing.
    pub fn voodoo_status(&self) -> Option<serde_json::Value> {
        self.voodoo.as_ref().map(|v| v.describe(self.voodoo_now()))
    }

    /// Whether the 3dfx card shows its picture instead of the VGA's.
    pub fn voodoo_output(&self) -> bool {
        self.voodoo.as_ref().is_some_and(|v| v.output())
    }

    /// For the debugger: a dword of the card's window without the effects
    /// of a read by the CPU (which has none but the status's timing).
    pub fn voodoo_peek_32(&self, addr: usize) -> Option<u32> {
        let offset = self.voodoo_at(addr)?;
        Some(self.voodoo_read_32(offset & !3) >> (8 * (offset & 3)))
    }
}
