//! The 3D engine's registers in the Banshee's and Voodoo3's map: the
//! Voodoo2's triangle and mode registers, with what the card does its
//! own way. Its buffers are placed by address and stride (colBufferAddr,
//! auxBufferAddr and their strides) instead of the fbiInit registers,
//! linear or in tiles of 128 bytes by 32 rows; the Y origin at the
//! bottom counts from miscInit0's; a swap shows leftOverlayBuf through
//! the video processor's overlay, at once or at a vertical retrace,
//! while the command FIFOs wait for it; swapPending counts the swaps the
//! status register reports; texBaseAddr is a byte address; and the 3D
//! linear frame buffer has rows of 2048 pixels.
//!
//! From 86Box's vid_voodoo_reg.c, vid_voodoo_render.c, vid_voodoo_fb.c
//! and vid_voodoo_banshee.c (Sarah Walker, GPLv2) and sections 5 and 8
//! of 3dfx's Voodoo3 specification.

use super::super::mirror::{BufferKey, COLOR_BUFFERS, LayoutKey};
use super::super::raster::Stride;
use super::super::regs::*;
use super::super::{Effect, NONE, Now, PendingSwap, Voodoo};
use super::{VID_DESKTOP_START_ADDR, VID_PROC_CFG, VIDPROC_HALF_MODE, VIDPROC_OVERLAY_ENABLE};

// The card's registers where a Voodoo2 has its command FIFO's and some
// of its init registers (dword numbers in the 3D space).
pub const COL_BUFFER_ADDR: usize = 0x1EC / 4;
pub const COL_BUFFER_STRIDE: usize = 0x1F0 / 4;
pub const AUX_BUFFER_ADDR: usize = 0x1F4 / 4;
pub const AUX_BUFFER_STRIDE: usize = 0x1F8 / 4;
pub const CLIP_LEFT_RIGHT1: usize = 0x200 / 4;
pub const CLIP_TOP_BOTTOM1: usize = 0x204 / 4;
pub const SWAP_PENDING: usize = 0x24C / 4;
pub const LEFT_OVERLAY_BUF: usize = 0x250 / 4;
pub const RIGHT_OVERLAY_BUF: usize = 0x254 / 4;

/// The registers the card keeps as written and reads back.
const KEPT: [usize; 8] = [
    COL_BUFFER_ADDR,
    COL_BUFFER_STRIDE,
    AUX_BUFFER_ADDR,
    AUX_BUFFER_STRIDE,
    CLIP_LEFT_RIGHT1,
    CLIP_TOP_BOTTOM1,
    LEFT_OVERLAY_BUF,
    RIGHT_OVERLAY_BUF,
];

/// A buffer's stride register: bytes a row when linear (bit 15 clear),
/// tiles a row of tiles when tiled.
fn stride(value: u32) -> Stride {
    if value & (1 << 15) != 0 {
        Stride {
            words: (value & 0x7F) as usize * 2048,
            tiled: true,
        }
    } else {
        Stride::linear((value & 0x3FFF) as usize / 2)
    }
}

impl Voodoo {
    /// A 3D register read through BAR0 (200000h-5FFFFFh).
    pub(crate) fn banshee_3d_read(&self, offset: u32, now: Now) -> u32 {
        match ((offset >> 2) & 0xFF) as usize {
            STATUS => self.banshee_status(now),
            INTR_CTRL => self.banshee.as_ref().map_or(0, |b| b.intr_ctrl),
            regnum if KEPT.contains(&regnum) => self.reg[regnum],
            // The Voodoo2's video and init registers aren't there.
            0x80..=0x96 => 0,
            regnum => self.register_read(regnum as u32, now),
        }
    }

    /// A 3D register written through BAR0. swapPending counts at once;
    /// the rest waits behind a swap that waits for the retrace, as in the
    /// card's FIFO.
    pub(crate) fn banshee_3d_write(
        &mut self,
        offset: u32,
        data: u32,
        now: Now,
        effect: &mut Effect,
    ) {
        match ((offset >> 2) & 0xFF) as usize {
            SWAP_PENDING => self.banshee_3d_register(offset & 0x3FFC, data, now, effect),
            INTR_CTRL => {
                if let Some(b) = &mut self.banshee {
                    b.intr_ctrl = data & 0x0030_003F;
                }
            }
            _ => {
                self.banshee_3d_register(offset & 0x3FFC, data, now, effect);
                self.fifo_write(now, effect);
            }
        }
    }

    /// A 3D register written, from BAR0 or a command FIFO: byte address
    /// `addr`, with the chips it goes to in bits 13:10.
    pub(crate) fn banshee_3d_register(
        &mut self,
        addr: u32,
        data: u32,
        now: Now,
        effect: &mut Effect,
    ) {
        let mut chips = (addr >> 10) & 0xF;
        if chips == 0 {
            chips = 0xF;
        }
        chips &= self.chipmask;
        let regnum = ((addr >> 2) & 0xFF) as usize;
        match regnum {
            COL_BUFFER_ADDR | COL_BUFFER_STRIDE | AUX_BUFFER_ADDR | AUX_BUFFER_STRIDE => {
                self.reg[regnum] = data;
                self.banshee_buffers();
            }
            CLIP_LEFT_RIGHT1 | CLIP_TOP_BOTTOM1 | LEFT_OVERLAY_BUF | RIGHT_OVERLAY_BUF => {
                self.reg[regnum] = data;
            }
            SWAPBUFFER_CMD => self.banshee_swap(data, now, effect),
            // From BAR0 or from a command FIFO packet alike.
            SWAP_PENDING => {
                if let Some(b) = &mut self.banshee {
                    b.swap_count += 1;
                }
            }
            // The Voodoo2's video and init registers aren't there.
            0x80..=0x96 => {}
            _ => {
                if self.chip.access[regnum] & WRITE != 0 {
                    self.register_to(regnum, chips, data, now, effect);
                }
            }
        }
    }

    /// Where the 3D engine draws, from colBufferAddr, auxBufferAddr and
    /// their strides: the colour buffer is the same whichever fbzMode
    /// and lfbMode pick.
    pub(crate) fn banshee_buffers(&mut self) {
        self.blt_queue_stale();
        let col = self.reg[COL_BUFFER_ADDR] & 0xFF_FFF0;
        let aux = self.reg[AUX_BUFFER_ADDR] & 0xFF_FFF0;
        let (col_stride, aux_stride) = (
            stride(self.reg[COL_BUFFER_STRIDE]),
            stride(self.reg[AUX_BUFFER_STRIDE]),
        );
        let fbi = &mut self.fbi;
        fbi.rgboffs = [col, col, NONE];
        fbi.auxoffs = aux;
        fbi.frontbuf = 0;
        fbi.backbuf = 1;
        fbi.stride = col_stride;
        fbi.aux_stride = aux_stride;
        fbi.rowpixels = col_stride.pixels() as u32;
        fbi.ymask = -1;
        fbi.fifo_size = 0;
        // Rows below the buffer are other memory, not other rows.
        fbi.width = fbi.rowpixels;
        fbi.height = 4096;
        self.update_tiles();
    }

    /// The buffers the OpenGL renderer's recording keeps (`mirror_sync`),
    /// `old` the layout recorded last. A program swaps by moving
    /// colBufferAddr between two or three buffers, and may draw into
    /// textures in between (a render target of another size, which
    /// colBufferAddr and colBufferStride point at while it draws there).
    /// Those drawn into lately stay in the layout beside colBufferAddr's,
    /// each with its own stride, so that neither a swap nor a turn at a
    /// texture makes the recording start over from memory, which would
    /// read every buffer the renderer drew back first: up to
    /// `COLOR_BUFFERS`, the oldest dropped first, while they stay apart
    /// from colBufferAddr's buffer and the auxiliary one. The buffers are
    /// as wide as their stride and as high as the desktop, which is as
    /// much as the video processor shows of them unscaled; in half mode
    /// with the overlay off, half as high, as it shows each row of the
    /// desktop's buffer twice. The card has no register for their height.
    pub(crate) fn banshee_mirror_key(&self, old: Option<&LayoutKey>) -> LayoutKey {
        let fbi = &self.fbi;
        let height = self.banshee_mirror_height();
        let buffer = |at: u32, stride: Stride| {
            BufferKey::new(at, stride, (stride.pixels() as u32).clamp(1, 2048))
        };
        let col = buffer(fbi.rgboffs[0], fbi.stride);
        let aux = buffer(fbi.auxoffs, fbi.aux_stride);
        let Some(old) = old.filter(|o| (o.height, o.aux) == (height, aux)) else {
            let mut color = [BufferKey::NONE; COLOR_BUFFERS];
            color[0] = col;
            return LayoutKey::new(height, color, aux);
        };
        if !col.is_some() || old.colors().any(|c| *c == col) {
            return *old;
        }
        let apart = |a: &BufferKey, b: &BufferKey| {
            let ((a0, a1), (b0, b1)) = (a.span(height), b.span(height));
            !b.is_some() || a1 <= b0 || b1 <= a0
        };
        let mut color = [BufferKey::NONE; COLOR_BUFFERS];
        let mut n = 0;
        for c in old.colors() {
            if apart(c, &col) && apart(c, &aux) {
                color[n] = *c;
                n += 1;
            }
        }
        if n == COLOR_BUFFERS {
            color.rotate_left(1);
            n -= 1;
        }
        color[n] = col;
        LayoutKey::new(height, color, aux)
    }

    /// The rows of the recording's buffers (`banshee_mirror_key`): the
    /// desktop's, or half of them in half mode with the overlay off. They
    /// come from the CRTC and vidProcCfg, which are written outside
    /// `register_to` and don't drop `blt_queue`, unlike the rest of the
    /// layout.
    pub(crate) fn banshee_mirror_height(&self) -> u32 {
        let height = self.banshee.as_ref().map_or(1, |b| {
            let cfg = b.reg(VID_PROC_CFG);
            let rows = b.desktop_size().1;
            let half = cfg & (VIDPROC_HALF_MODE | VIDPROC_OVERLAY_ENABLE) == VIDPROC_HALF_MODE;
            if half { rows.div_ceil(2) } else { rows }
        });
        height.clamp(1, 2048)
    }

    /// The colour buffer of the recording (its word offset) the video
    /// processor shows: the overlay's, which a swap points at
    /// leftOverlayBuf, while the overlay is on, or else the desktop's.
    pub(crate) fn banshee_mirror_front(&self, key: &LayoutKey) -> Option<u32> {
        let b = self.banshee.as_ref()?;
        let addr = if b.reg(VID_PROC_CFG) & VIDPROC_OVERLAY_ENABLE != 0 {
            b.overlay_addr
        } else {
            b.reg(VID_DESKTOP_START_ADDR)
        };
        key.color_at(addr).map(|_| addr / 2)
    }

    /// swapbufferCMD: the overlay shows leftOverlayBuf, now or with bit 0
    /// at the retrace `bits 8:1` retraces after the last swap, and the
    /// swap counts off swapPending's count then (86Box's
    /// `voodoo_reg_writel`). The address is taken now, so a program can
    /// set the next one while the swap waits.
    fn banshee_swap(&mut self, data: u32, now: Now, effect: &mut Effect) {
        self.backlog.borrow_mut().swapped();
        let addr = self.reg[LEFT_OVERLAY_BUF] & 0xFF_FFFF;
        let current = self.retraces(now.ns);
        let mark = self.swap_mark();
        if data & 1 == 0 {
            self.last_swap = current;
            self.banshee_show(addr, mark);
            return;
        }
        let interval = ((data >> 1) & 0xFF) as u64;
        let after = self.swaps.back().map_or(self.last_swap, |s| s.retrace);
        let retrace = (current + 1).max(after + interval.max(1));
        let due = self.retrace_ticks(retrace);
        self.swaps.push_back(PendingSwap { retrace, due });
        if let Some(b) = &mut self.banshee {
            b.overlay_queue.push_back(addr);
            self.swap_marks.push_back(mark);
        }
        effect.reschedule = true;
    }

    /// A swap happened: the overlay shows `addr`, which needs the jobs up
    /// to batch `mark` drawn (`swap_mark`).
    fn banshee_show(&mut self, addr: u32, mark: Option<u64>) {
        if let Some(b) = &mut self.banshee {
            b.overlay_addr = addr;
            b.swap_count = b.swap_count.saturating_sub(1);
            b.dirty = true;
        }
        self.swapped += 1;
        self.display_dirty = true;
        self.shown_up_to(mark);
    }

    /// `retired` swaps happened: their buffers shown, and the command
    /// FIFOs that waited for them go on.
    pub(crate) fn banshee_swaps_done(&mut self, retired: usize, now: Now, effect: &mut Effect) {
        for _ in 0..retired {
            let addr = self
                .banshee
                .as_mut()
                .and_then(|b| b.overlay_queue.pop_front());
            // After a load the swaps have no batches: wait for all.
            let mark = self.swap_marks.pop_front().flatten();
            if let Some(addr) = addr {
                self.banshee_show(addr, mark);
            }
        }
        if retired > 0 && self.swaps.is_empty() {
            for n in 0..2 {
                self.run_banshee_fifo(n, now, effect);
            }
        }
    }

    /// Whether the command FIFOs wait for a swap's retrace.
    pub(crate) fn banshee_fifo_waits(&self) -> bool {
        !self.swaps.is_empty()
    }

    /// TMU 0's texture space from 600000h: texture memory from its
    /// texBaseAddr on, linear (86Box's `voodoo_tex_writel`). The
    /// triangles queued before are drawn first if they may read or draw
    /// into the dword and can't be given a copy of what they read
    /// (`spare`).
    pub(crate) fn banshee_texture_write(&mut self, offset: u32, data: u32) {
        let base = self.reg[0x100 + TEX_BASE_ADDR] & 0xFF_FFF0;
        let addr = base.wrapping_add(offset & 0x1F_FFFC) & self.fbi.mask;
        if !self.spare(addr, addr + 4) {
            self.flush();
        }
        self.gpu_sync_bytes(addr, 4);
        super::blit2d::write(&self.fbi.ram, addr, 4, data, self.fbi.mask);
        self.mirror_memory(addr, addr + 4);
    }

    /// `data` downloaded into the card's memory from byte `addr` on, as
    /// writes of it through TMU 0's texture space would put it, all at
    /// once: Glide's texture downloads. The triangles queued before
    /// keep what they read: they are given a copy of it (`spare`), the
    /// bytes take their turn behind them (`banshee_texture_queue`), or
    /// they are drawn first. The recording hears of the bytes once.
    /// Past the end of the memory they wrap round.
    pub(crate) fn banshee_texture_download(&mut self, addr: u32, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        let size = self.fbi.mask as usize + 1;
        let start = addr as usize & (size - 1);
        let (here, wrapped) = data.split_at(data.len().min(size - start));
        let spared = self.spare(start as u32, (start + here.len()) as u32);
        if spared || !self.banshee_texture_queue(start, here) {
            if !spared {
                self.flush();
            }
            self.gpu_sync_bytes(start as u32, here.len());
            self.fbi.ram.write_bytes(start, here);
            self.mirror_memory(start as u32, (start + here.len()) as u32);
        }
        if !wrapped.is_empty() {
            self.banshee_texture_download(0, wrapped);
        }
    }

    /// For the debugger: where the 3D engine draws and what it shows.
    pub(crate) fn banshee_3d_describe(&self) -> serde_json::Value {
        let fbi = &self.fbi;
        let describe = |s: Stride| {
            if s.tiled {
                format!("{} tiles", s.words / 2048)
            } else {
                format!("{} bytes", s.words * 2)
            }
        };
        serde_json::json!({
            "color": format!("{:06X}", fbi.rgboffs[0]),
            "color_stride": describe(fbi.stride),
            "aux": format!("{:06X}", fbi.auxoffs),
            "aux_stride": describe(fbi.aux_stride),
            "y_origin": fbi.yorigin,
            "left_overlay": format!("{:06X}", self.reg[LEFT_OVERLAY_BUF] & 0xFF_FFFF),
            "swap_count": self.banshee.as_ref().map_or(0, |b| b.swap_count),
            "swaps": self.swapped,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::super::Board;
    use super::super::{MISC_INIT0, VID_CURR_OVERLAY_START_ADDR};
    use super::*;

    fn card() -> Voodoo {
        Voodoo::with_workers(Board::Voodoo3, 0)
    }

    pub(crate) fn reg3d(v: &mut Voodoo, regnum: usize, data: u32) {
        v.write(
            0x20_0000 + 4 * regnum as u32,
            data,
            0xFFFF_FFFF,
            Now::default(),
        );
    }

    pub(crate) fn float(v: &mut Voodoo, regnum: usize, value: f32) {
        reg3d(v, regnum, value.to_bits());
    }

    /// A flat-shaded rectangle of two triangles, `x0..x1` by `y0..y1`, in
    /// colour `rgb`, the iterated colour.
    pub(crate) fn rectangle(
        v: &mut Voodoo,
        (x0, y0, x1, y1): (f32, f32, f32, f32),
        rgb: (f32, f32, f32),
    ) {
        for (a, b, c) in [
            ((x0, y0), (x1, y0), (x1, y1)),
            ((x0, y0), (x1, y1), (x0, y1)),
        ] {
            float(v, FVERTEX_AX, a.0);
            float(v, FVERTEX_AY, a.1);
            float(v, FVERTEX_BX, b.0);
            float(v, FVERTEX_BY, b.1);
            float(v, FVERTEX_CX, c.0);
            float(v, FVERTEX_CY, c.1);
            float(v, FSTART_R, rgb.0);
            float(v, FSTART_G, rgb.1);
            float(v, FSTART_B, rgb.2);
            reg3d(v, FTRIANGLE_CMD, 0);
        }
    }

    /// A gate the next frame's job waits at (`Job::Hold`).
    pub(crate) type Gate = std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>;

    fn open(gate: &Gate) {
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
    }

    /// The picture comes with the job at `gate` still held. If it waits
    /// for that job after all, the gate opens after a while and the test
    /// fails, instead of waiting for ever.
    pub(crate) fn picture_comes_before(v: &mut Voodoo, gate: &Gate, label: &str) {
        let (done, wait) = std::sync::mpsc::channel::<()>();
        std::thread::scope(|s| {
            let watchdog = s.spawn(move || {
                let late = wait
                    .recv_timeout(std::time::Duration::from_secs(10))
                    .is_err();
                if late {
                    open(gate);
                }
                late
            });
            v.catch_up_shown();
            let _ = done.send(());
            assert!(!watchdog.join().unwrap(), "{label}: the picture waited");
        });
        assert!(v.pool.busy(), "{label}: the next frame waits at the gate");
    }

    /// The picture waits for every job, the one at `gate` too.
    pub(crate) fn picture_waits_for_all(v: &mut Voodoo, gate: &Gate, label: &str) {
        use std::sync::atomic::{AtomicBool, Ordering};
        let returned = AtomicBool::new(false);
        std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(std::time::Duration::from_millis(50));
                let early = returned.load(Ordering::SeqCst);
                open(gate);
                assert!(!early, "{label}: it waits for all");
            });
            v.catch_up_shown();
            returned.store(true, Ordering::SeqCst);
        });
        assert!(!v.pool.busy(), "{label}");
    }

    /// Double buffering, as Myth draws: the next frame goes into the
    /// buffer shown until its swap's retrace, and is sent before it. The
    /// picture made after the retrace shows the other buffer, and doesn't
    /// wait for that frame.
    #[test]
    fn the_next_frame_drawn_before_the_retrace_doesnt_hold_the_picture() {
        use super::super::super::workers::Job;
        use super::super::{VID_PROC_CFG, VIDPROC_ENABLE};

        let (a, b) = (0x10_0000u32, 0x20_0000u32);
        let mut v = Voodoo::with_workers(Board::Voodoo3, 2);
        let cfg = VIDPROC_ENABLE | VIDPROC_OVERLAY_ENABLE;
        v.write(VID_PROC_CFG, cfg, u32::MAX, Now::default());
        reg3d(&mut v, LEFT_OVERLAY_BUF, a);
        reg3d(&mut v, SWAPBUFFER_CMD, 0);
        // A frame into B, swapped at the next retrace; the next one into A,
        // shown meanwhile, held.
        v.mark_drawn(b as usize / 2);
        reg3d(&mut v, LEFT_OVERLAY_BUF, b);
        reg3d(&mut v, SWAPBUFFER_CMD, 1);
        let gate = Gate::default();
        v.submit_outside(Job::Hold(gate.clone()), None);
        v.mark_drawn(a as usize / 2);
        let due = v.next_event().expect("the swap waits for a retrace");
        v.service(Now {
            ns: 1_000_000_000,
            ticks: due,
        });
        assert_eq!(v.banshee.as_ref().unwrap().overlay_addr, b);
        picture_comes_before(&mut v, &gate, "double buffering");
        open(&gate);
        v.flush();
    }

    /// Frame buffer writes into the buffer shown, queued right after the
    /// swap, go out with the next job, in a batch after the swap's: the
    /// picture waits for them.
    #[test]
    fn frame_buffer_writes_queued_after_a_swap_hold_the_picture() {
        use super::super::super::workers::{Job, Word};
        use super::super::{VID_PROC_CFG, VIDPROC_ENABLE};

        let shown = 0x10_0000u32;
        let mut v = Voodoo::with_workers(Board::Voodoo3, 2);
        let cfg = VIDPROC_ENABLE | VIDPROC_OVERLAY_ENABLE;
        v.write(VID_PROC_CFG, cfg, u32::MAX, Now::default());
        let word = |value| Word {
            row: 0,
            at: shown / 2,
            value,
        };
        let fb = v.fbi.ram.clone();
        let words = vec![word(0x1234)];
        v.submit_outside(Job::Pixels { fb, words }, None);
        reg3d(&mut v, LEFT_OVERLAY_BUF, shown);
        reg3d(&mut v, SWAPBUFFER_CMD, 0);
        // A write into it waits behind the jobs, as `lfb_write` queues
        // it, and the next frame's job, held, takes it out.
        assert!(v.writes_queue());
        v.queue_words(&[word(0x5678)]);
        v.mark_drawn(shown as usize / 2);
        let gate = Gate::default();
        v.submit_outside(Job::Hold(gate.clone()), None);
        picture_waits_for_all(&mut v, &gate, "frame buffer write");
        assert_eq!(v.fbi.ram.get(shown as usize / 2), 0x5678);
    }

    /// A swap shown at once while one waits for its retrace: the jobs
    /// drawn between the two into the waiting swap's buffer stay noted,
    /// and the picture waits for them once that buffer is shown.
    #[test]
    fn a_swap_past_a_waiting_one_keeps_what_that_one_shows() {
        use super::super::super::workers::Job;
        use super::super::{VID_PROC_CFG, VIDPROC_ENABLE};

        let (a, b) = (0x10_0000u32, 0x20_0000u32);
        let mut v = Voodoo::with_workers(Board::Voodoo3, 2);
        let cfg = VIDPROC_ENABLE | VIDPROC_OVERLAY_ENABLE;
        v.write(VID_PROC_CFG, cfg, u32::MAX, Now::default());
        reg3d(&mut v, LEFT_OVERLAY_BUF, a);
        reg3d(&mut v, SWAPBUFFER_CMD, 1);
        let gate = Gate::default();
        v.submit_outside(Job::Hold(gate.clone()), None);
        v.mark_drawn(a as usize / 2);
        // Written straight to the register, the swap doesn't wait.
        reg3d(&mut v, LEFT_OVERLAY_BUF, b);
        reg3d(&mut v, SWAPBUFFER_CMD, 0);
        let due = v.next_event().expect("the first swap waits for a retrace");
        v.service(Now {
            ns: 1_000_000_000,
            ticks: due,
        });
        assert_eq!(v.banshee.as_ref().unwrap().overlay_addr, a);
        picture_waits_for_all(&mut v, &gate, "the waiting swap's buffer");
    }

    /// Triangles held back for the OpenGL renderer go out in a batch not
    /// known as they come. Drawn into the buffer shown after a swap from
    /// before the recording, and given out when it stops, they hold the
    /// picture.
    #[test]
    fn jobs_held_for_the_renderer_hold_the_picture_once_given_out() {
        use super::super::super::workers::Job;
        use super::super::{VID_PROC_CFG, VIDPROC_ENABLE};

        let shown = 0x10_0000u32;
        let mut v = Voodoo::with_workers(Board::Voodoo3, 2);
        let cfg = VIDPROC_ENABLE | VIDPROC_OVERLAY_ENABLE;
        v.write(VID_PROC_CFG, cfg, u32::MAX, Now::default());
        reg3d(&mut v, COL_BUFFER_ADDR, shown);
        reg3d(&mut v, COL_BUFFER_STRIDE, 1280);
        reg3d(&mut v, AUX_BUFFER_ADDR, 0x20_0000);
        reg3d(&mut v, AUX_BUFFER_STRIDE, 1280);
        reg3d(&mut v, FBZ_MODE, 1 << 9);
        reg3d(&mut v, LEFT_OVERLAY_BUF, shown);
        reg3d(&mut v, SWAPBUFFER_CMD, 0);
        v.set_mirror(true);
        rectangle(&mut v, (0.0, 0.0, 16.0, 16.0), (255.0, 0.0, 0.0));
        assert!(v.writes_queue(), "the rectangle is held back");
        // The workers wait at a gate, which goes out ahead of it.
        let gate = Gate::default();
        v.submit_outside(Job::Hold(gate.clone()), None);
        v.set_mirror(false);
        picture_waits_for_all(&mut v, &gate, "held back for the renderer");
        assert_eq!(v.fbi.ram.get(shown as usize / 2), 0xF800);
    }

    /// After a swap the picture waits for the jobs given out before it,
    /// and the next frame's go on drawing beside the program; unless one
    /// of those draws into what is shown when the picture is made, which
    /// waits for all.
    #[test]
    fn the_picture_waits_for_the_frame_a_swap_shows() {
        use super::super::super::workers::{Job, Word};
        use super::super::{VID_DESKTOP_OVERLAY_STRIDE, VID_PROC_CFG, VIDPROC_ENABLE};

        let shown = 0x10_0000;
        let changes = [
            "drawn into",
            "texture written",
            "drawn while hidden",
            "drawn into a swap's to come",
            "desktop moved",
            "stride changed",
        ];
        for change in changes {
            let mut v = Voodoo::with_workers(Board::Voodoo3, 2);
            let cfg = VIDPROC_ENABLE | VIDPROC_OVERLAY_ENABLE;
            v.write(VID_PROC_CFG, cfg, u32::MAX, Now::default());
            // The frame shown: a word of the buffer at 100000h.
            let word = Word {
                row: 0,
                at: shown / 2,
                value: 0x1234,
            };
            let fb = v.fbi.ram.clone();
            let words = vec![word];
            v.submit_outside(Job::Pixels { fb, words }, None);
            reg3d(&mut v, LEFT_OVERLAY_BUF, shown);
            reg3d(&mut v, SWAPBUFFER_CMD, 0);
            // A swap at the next retrace, to the buffer at 300000h.
            let next = 0x30_0000;
            if change == "drawn into a swap's to come" {
                reg3d(&mut v, LEFT_OVERLAY_BUF, next);
                reg3d(&mut v, SWAPBUFFER_CMD, 1);
            }
            // The next frame's jobs, held.
            let gate = Gate::default();
            v.submit_outside(Job::Hold(gate.clone()), None);
            picture_comes_before(&mut v, &gate, change);
            assert_eq!(v.fbi.ram.get(shown as usize / 2), 0x1234);

            let write = |v: &mut Voodoo, reg, value| {
                v.write(reg, value, u32::MAX, Now::default());
            };
            match change {
                "drawn into" => v.mark_drawn(shown as usize / 2),
                "texture written" => {
                    assert!(v.banshee_texture_queue(shown as usize, &[1, 2, 3, 4]));
                }
                // Drawn while the overlay is off, then shown again.
                "drawn while hidden" => {
                    write(&mut v, VID_PROC_CFG, VIDPROC_ENABLE);
                    v.mark_drawn(shown as usize / 2);
                    write(&mut v, VID_PROC_CFG, cfg);
                }
                // Drawn while the swap waits, which then shows it.
                "drawn into a swap's to come" => {
                    v.mark_drawn(next as usize / 2);
                    let due = v.next_event().expect("the swap waits for a retrace");
                    v.service(Now {
                        ns: 1_000_000_000,
                        ticks: due,
                    });
                    assert_eq!(v.banshee.as_ref().unwrap().overlay_addr, next);
                }
                // Drawn into a buffer the desktop then shows.
                "desktop moved" => {
                    v.mark_drawn(0x20_0000 / 2);
                    write(&mut v, VID_DESKTOP_START_ADDR, 0x20_0000);
                }
                // Texture data past the overlay's rows, which a wider stride
                // then shows.
                _ => {
                    assert!(v.banshee_texture_queue(shown as usize + 0x100, &[1, 2, 3, 4]));
                    write(&mut v, VID_DESKTOP_OVERLAY_STRIDE, 1280 << 16 | 1280);
                }
            }
            picture_waits_for_all(&mut v, &gate, change);
        }
    }

    #[test]
    fn gl_display_leaves_jobs_until_a_software_picture_is_requested() {
        use super::super::{VGA_INIT0, VGAINIT0_EXTENDED_SHIFT_OUT, VID_PROC_CFG, VIDPROC_ENABLE};

        for board in [Board::Banshee, Board::Voodoo3] {
            let mut v = Voodoo::with_workers(board, 4);
            v.write(VGA_INIT0, VGAINIT0_EXTENDED_SHIFT_OUT, u32::MAX, Now::default());
            v.write(VID_PROC_CFG, VIDPROC_ENABLE, u32::MAX, Now::default());
            reg3d(&mut v, COL_BUFFER_ADDR, 0);
            reg3d(&mut v, COL_BUFFER_STRIDE, 1280);
            reg3d(&mut v, AUX_BUFFER_ADDR, 0x20_0000);
            reg3d(&mut v, AUX_BUFFER_STRIDE, 1280);
            reg3d(&mut v, FBZ_MODE, 1 << 9);
            v.set_mirror(true);
            v.set_software_picture(false);
            // Consume the initial snapshot before queuing any drawing.
            v.take_mirror();
            assert!(v.output());
            assert!(v.prepare_display(), "switching output still signals a change");
            rectangle(&mut v, (0.0, 0.0, 16.0, 16.0), (255.0, 0.0, 0.0));
            assert!(v.writes_queue());
            assert_eq!(v.fbi.ram.get(0), 0, "the batch has not been sent yet");
            assert!(!v.prepare_display());
            assert_eq!(v.fbi.ram.get(0), 0, "GL scanout must not flush the batch");
            assert!(v.display_dirty);
            // The video processor's change went to the renderer with the
            // scanout `take_mirror` gave it.
            assert!(!v.banshee.as_ref().unwrap().dirty);

            // Screenshots, the debugger and renderer failure request the
            // software picture through the same switch.
            v.set_software_picture(true);
            assert!(v.prepare_display());
            assert!(!v.writes_queue());
            assert_eq!(v.fbi.ram.get(0), 0xF800);
            assert!(!v.prepare_display());

            // A change confined to the video processor is retained too.
            v.set_software_picture(false);
            v.banshee.as_mut().unwrap().dirty = true;
            assert!(!v.prepare_display());
            assert!(v.banshee.as_ref().unwrap().dirty);
            v.set_mirror(false);
            assert!(v.prepare_display());
            assert!(!v.banshee.as_ref().unwrap().dirty);
        }
    }

    #[test]
    fn triangles_draw_into_a_tiled_colour_buffer() {
        let mut v = card();
        // The colour buffer at 1 MB, 16 tiles (1024 pixels) a row of tiles.
        reg3d(&mut v, COL_BUFFER_ADDR, 0x10_0000);
        reg3d(&mut v, COL_BUFFER_STRIDE, 1 << 15 | 16);
        // RGB writes, no clipping or depth.
        reg3d(&mut v, FBZ_MODE, 1 << 9);
        rectangle(&mut v, (60.0, 30.0, 70.0, 40.0), (255.0, 0.0, 0.0));
        v.flush();
        // Pixel (65, 33): tile 1 of the second row of tiles, row 1 in it.
        let band = 0x10_0000 / 2 + 16 * 2048;
        assert_eq!(v.fbi.ram.get(band + 2048 + 64 + 1), 0xF800);
        // Pixel (59, 33) is left of the rectangle.
        assert_eq!(v.fbi.ram.get(band + 64 + 59), 0);
        assert_eq!(v.reg[FBI_TRIANGLES_OUT], 2);
    }

    #[test]
    fn a_fastfill_of_a_linear_buffer_and_tiled_depth() {
        let mut v = card();
        reg3d(&mut v, COL_BUFFER_ADDR, 0x20_0000);
        reg3d(&mut v, COL_BUFFER_STRIDE, 1280);
        reg3d(&mut v, AUX_BUFFER_ADDR, 0x30_0000);
        reg3d(&mut v, AUX_BUFFER_STRIDE, 1 << 15 | 10);
        reg3d(&mut v, CLIP_LEFT_RIGHT, 2 << 16 | 4);
        reg3d(&mut v, CLIP_LOW_Y_HIGH_Y, 40 << 16 | 41);
        reg3d(&mut v, COLOR1, 0x00_00FF);
        reg3d(&mut v, ZA_COLOR, 0x1234);
        reg3d(&mut v, FBZ_MODE, 1 << 9 | 1 << 10);
        reg3d(&mut v, FASTFILL_CMD, 0);
        v.flush();
        let ram = &v.fbi.ram;
        assert_eq!(ram.get(0x20_0000 / 2 + 40 * 640 + 3), 0x001F);
        assert_eq!(ram.get(0x20_0000 / 2 + 40 * 640 + 4), 0);
        // Row 40 of the depth is row 8 of the second row of tiles.
        assert_eq!(ram.get(0x30_0000 / 2 + 10 * 2048 + 8 * 64 + 3), 0x1234);
    }

    #[test]
    fn the_y_origin_comes_from_misc_init0() {
        let mut v = card();
        v.write(MISC_INIT0, 99 << 18, 0xFFFF_FFFF, Now::default());
        assert_eq!(v.fbi.yorigin, 99);
        reg3d(&mut v, COL_BUFFER_ADDR, 0);
        reg3d(&mut v, COL_BUFFER_STRIDE, 2048);
        // The Y origin at the bottom: row 0 is line 99.
        reg3d(&mut v, FBZ_MODE, 1 << 9 | 1 << 17);
        rectangle(&mut v, (0.0, 0.0, 4.0, 1.0), (0.0, 255.0, 0.0));
        v.flush();
        assert_eq!(v.fbi.ram.get(99 * 1024 + 2), 0x07E0);
        assert_eq!(v.fbi.ram.get(2), 0);
    }

    #[test]
    fn lfb_rows_are_2048_pixels() {
        let mut v = card();
        reg3d(&mut v, COL_BUFFER_ADDR, 0x40_0000);
        reg3d(&mut v, COL_BUFFER_STRIDE, 1 << 15 | 8);
        // 16-bit 5-6-5, two pixels a dword: pixels (6, 3) and (7, 3).
        reg3d(&mut v, LFB_MODE, 0);
        let offset = 0x100_0000 + 3 * 4096 + 6 * 2;
        v.write(offset, 0x07E0_F800, 0xFFFF_FFFF, Now::default());
        let row = 0x40_0000 / 2 + 3 * 64;
        assert_eq!(v.fbi.ram.get(row + 6), 0xF800);
        assert_eq!(v.fbi.ram.get(row + 7), 0x07E0);
        assert_eq!(v.read(offset, Now::default()), 0x07E0_F800);
        // 32-bit ARGB, a pixel a dword: pixel (70, 1), in the next tile.
        reg3d(&mut v, LFB_MODE, 5);
        v.write(
            0x100_0000 + 8192 + 70 * 4,
            0xFF00_00FF,
            0xFFFF_FFFF,
            Now::default(),
        );
        assert_eq!(v.fbi.ram.get(0x40_0000 / 2 + 2048 + 64 + 6), 0x001F);
    }

    #[test]
    fn swaps_show_the_left_overlay_buffer() {
        let mut v = card();
        let now = Now::default();
        // A pending swap counted, then a swap at once.
        reg3d(&mut v, LEFT_OVERLAY_BUF, 0x12_3400);
        reg3d(&mut v, SWAP_PENDING, 0);
        assert_eq!(v.banshee_status(now) >> 28, 1);
        reg3d(&mut v, SWAPBUFFER_CMD, 0);
        assert_eq!(v.read(VID_CURR_OVERLAY_START_ADDR, now), 0x12_3400);
        assert_eq!(v.banshee_status(now) >> 28, 0);
        // At a retrace: pending until it comes, with the address of when
        // it was asked for.
        reg3d(&mut v, LEFT_OVERLAY_BUF, 0x20_0000);
        reg3d(&mut v, SWAP_PENDING, 0);
        reg3d(&mut v, SWAPBUFFER_CMD, 1);
        reg3d(&mut v, LEFT_OVERLAY_BUF, 0x30_0000);
        assert_eq!(v.read(VID_CURR_OVERLAY_START_ADDR, now), 0x12_3400);
        assert_eq!(v.banshee_status(now) >> 28, 1);
        let due = v.next_event().expect("the swap waits for a retrace");
        v.service(Now {
            ns: 1_000_000_000,
            ticks: due,
        });
        assert_eq!(v.read(VID_CURR_OVERLAY_START_ADDR, now), 0x20_0000);
        assert_eq!(v.banshee_status(now) >> 28, 0);
    }

    #[test]
    fn the_card_is_busy_while_a_swap_waits_for_its_retrace() {
        // 3dfx's driver on Windows 95 flips Moto Racer's menus with a
        // swap at the retrace, then waits for the card to go idle before
        // it draws the next frame into the buffer still shown (#202).
        let mut v = card();
        let now = Now::default();
        let busy = |v: &Voodoo, now: Now| v.banshee_status(now) & 0x780 != 0;
        assert!(!busy(&v, now));
        reg3d(&mut v, LEFT_OVERLAY_BUF, 0x10_0000);
        reg3d(&mut v, SWAPBUFFER_CMD, 0);
        assert!(!busy(&v, now), "a swap at once is done at once");
        reg3d(&mut v, LEFT_OVERLAY_BUF, 0x20_0000);
        reg3d(&mut v, SWAPBUFFER_CMD, 1);
        assert!(busy(&v, now), "busy until the retrace");
        assert_eq!(v.read(VID_CURR_OVERLAY_START_ADDR, now), 0x10_0000);
        let due = v.next_event().expect("the swap waits for a retrace");
        let then = Now {
            ns: 1_000_000_000,
            ticks: due,
        };
        // Busy until the overlay shows the new buffer, which the timer
        // event at the retrace does.
        assert!(busy(&v, then), "busy until the swap is done");
        v.service(then);
        assert_eq!(v.read(VID_CURR_OVERLAY_START_ADDR, then), 0x20_0000);
        assert!(!busy(&v, then));
    }

    #[test]
    fn a_lower_refresh_rate_doesnt_put_swaps_off() {
        // A swap at once after ten minutes at 70 Hz, then a mode at 60
        // Hz: the next swap at a retrace waits about a frame, not for the
        // 6000 retraces 70 Hz counted past what 60 Hz has.
        use crate::timer::PIT_HZ;
        use crate::video::crt::CrtTiming;
        let mut v = card();
        let now = Now {
            ns: 600_000_000_000,
            ticks: 600 * PIT_HZ,
        };
        let swap = |v: &mut Voodoo, data: u32| {
            let at = 0x20_0000 + 4 * SWAPBUFFER_CMD as u32;
            v.write(at, data, 0xFFFF_FFFF, now);
        };
        v.set_timing(CrtTiming::VGA_400, now);
        swap(&mut v, 0);
        v.set_timing(CrtTiming::VESA_480, now);
        swap(&mut v, 1 << 1 | 1);
        let due = v.next_event().expect("the swap waits for a retrace");
        assert!(due - now.ticks < PIT_HZ / 30);
    }

    /// A card recording for the OpenGL renderer, a desktop of 64 rows on
    /// and the linear frame buffer untiled.
    pub(crate) fn mirrored() -> Voodoo {
        let mut v = card();
        let now = Now::default();
        v.write(VGA_INIT0, VGAINIT0_EXTENDED_SHIFT_OUT, u32::MAX, now);
        v.write(VID_PROC_CFG, VIDPROC_ENABLE | 1 << 18, u32::MAX, now);
        v.write(LFB_MEMORY_CONFIG, 0x1FFF, u32::MAX, now);
        v.banshee.as_mut().unwrap().crtc[0x12] = 63;
        reg3d(&mut v, FBZ_MODE, 1 << 9);
        v.set_mirror(true);
        v
    }

    fn snapshots(frame: &Frame) -> Vec<&Snapshot> {
        let resyncs = frame.commands.iter().filter_map(|c| match c {
            Command::Resync(s) => Some(&**s),
            _ => None,
        });
        resyncs.collect()
    }

    fn pixels(frame: &Frame) -> Vec<&Pixels> {
        let rows = frame.commands.iter().filter_map(|c| match c {
            Command::Pixels(p) => Some(p),
            _ => None,
        });
        rows.collect()
    }

    use super::super::super::mirror::{COLOR_BUFFERS, Command, Frame, Pixels, Snapshot};
    use super::super::{LFB_MEMORY_CONFIG, VGA_INIT0, VGAINIT0_EXTENDED_SHIFT_OUT, VIDPROC_ENABLE};

    #[test]
    fn the_recording_reads_tiled_buffers() {
        let mut v = mirrored();
        // Colour at 1 MB in rows of 10 tiles (640 pixels), depth at 2 MB
        // linear, 1280 bytes a row.
        reg3d(&mut v, COL_BUFFER_ADDR, 0x10_0000);
        reg3d(&mut v, COL_BUFFER_STRIDE, 1 << 15 | 10);
        reg3d(&mut v, AUX_BUFFER_ADDR, 0x20_0000);
        reg3d(&mut v, AUX_BUFFER_STRIDE, 1280);
        // Pixel (65, 33): tile 1 of the second row of tiles, row 1 in it.
        v.fbi
            .ram
            .set(0x10_0000 / 2 + 10 * 2048 + 2048 + 64 + 1, 0xF800);
        v.fbi.ram.set(0x20_0000 / 2 + 33 * 640 + 65, 0x1234);
        let frame = v.take_mirror().unwrap();
        let [snapshot] = snapshots(&frame)[..] else {
            panic!("one snapshot")
        };
        let layout = &snapshot.layout;
        assert_eq!((layout.width, layout.height), (640, 64));
        assert_eq!(layout.color, [0x10_0000 / 2]);
        assert_eq!(snapshot.color[0][33 * 640 + 65], 0xF800);
        assert_eq!(snapshot.color[0][33 * 640 + 64], 0);
        assert_eq!(snapshot.aux.as_ref().unwrap()[33 * 640 + 65], 0x1234);
    }

    #[test]
    fn textures_written_through_bar1_are_decoded_again() {
        use super::super::LINEAR;
        let decoded = |frame: &Frame| -> Vec<(u32, u32)> {
            let textures = frame.commands.iter().filter_map(|c| match c {
                Command::Texture(t) => Some((t.id, t.levels[0].argb[0])),
                _ => None,
            });
            textures.collect()
        };
        let mut v = mirrored();
        reg3d(&mut v, COL_BUFFER_ADDR, 0x10_0000);
        reg3d(&mut v, COL_BUFFER_STRIDE, 1280);
        // Level 5 (8x8) of a 5-6-5 texture at 5 MB, as its colour.
        reg3d(&mut v, TEX_BASE_ADDR, 0x50_0000);
        reg3d(&mut v, T_LOD, 20 | 20 << 6);
        let decal = 10 << 8 | 1 << 12 | 1 << 18 | 1 << 21 | 1 << 27;
        reg3d(&mut v, TEXTURE_MODE, decal);
        reg3d(&mut v, FBZ_COLOR_PATH, 1 | 1 << 27);
        rectangle(&mut v, (0.0, 0.0, 8.0, 8.0), (0.0, 0.0, 0.0));
        // A Voodoo3's two units draw with it.
        let [(first, _), (second, _)] = decoded(&v.take_mirror().unwrap())[..] else {
            panic!("a texture for each unit")
        };
        // Drawn with again, nothing to decode; its first texel written by
        // the CPU, which a Banshee's texture units share memory with, it
        // is decoded again as it is now.
        rectangle(&mut v, (0.0, 0.0, 8.0, 8.0), (0.0, 0.0, 0.0));
        assert!(decoded(&v.take_mirror().unwrap()).is_empty());
        let level5 = 2 * (65536 + 16384 + 4096 + 1024 + 256);
        v.write(LINEAR + 0x50_0000 + level5, 0xF800, 0xFFFF, Now::default());
        rectangle(&mut v, (0.0, 0.0, 8.0, 8.0), (0.0, 0.0, 0.0));
        let red = 0xFFFF_0000;
        assert_eq!(
            decoded(&v.take_mirror().unwrap()),
            [(first, red), (second, red)]
        );
    }

    #[test]
    fn double_buffered_frames_drop_what_is_drawn_over() {
        use super::super::LINEAR;
        // Eight frames as a game draws them, into two tiled buffers in
        // turn: clear the buffer and the depth with a fastfill, draw,
        // swap through the overlay, polling the busy bits on the way. The
        // status register reads leave held jobs alone. The recording
        // takes the second buffer's memory once, when it joins the
        // layout; from then on nothing waits for the drawing.
        let (a, b) = (0x10_0000, 0x20_0000);
        let run = |mirror: bool| {
            let mut v = mirrored();
            v.pool = super::super::super::workers::Pool::new(2);
            v.set_mirror(mirror);
            reg3d(&mut v, COL_BUFFER_STRIDE, 1 << 15 | 10);
            reg3d(&mut v, AUX_BUFFER_ADDR, 0x40_0000);
            reg3d(&mut v, AUX_BUFFER_STRIDE, 1 << 15 | 10);
            reg3d(&mut v, CLIP_LEFT_RIGHT, 640);
            reg3d(&mut v, CLIP_LOW_Y_HIGH_Y, 64);
            reg3d(&mut v, FBZ_MODE, 1 | 1 << 9 | 1 << 10);
            let mut joined = 0;
            for frame in 0..8u32 {
                if frame == 2 {
                    joined = v.catch_ups.get();
                }
                let draw = if frame % 2 == 0 { a } else { b };
                reg3d(&mut v, COL_BUFFER_ADDR, draw);
                reg3d(&mut v, COLOR1, frame * 0x10_1010);
                reg3d(&mut v, FASTFILL_CMD, 0);
                let x = frame as f32 * 8.0;
                rectangle(&mut v, (x, 4.0, x + 30.0, 40.0), (255.0, 128.0, 0.0));
                v.read(0x20_0000 + 4 * STATUS as u32, Now::default());
                reg3d(&mut v, LEFT_OVERLAY_BUF, draw);
                reg3d(&mut v, SWAPBUFFER_CMD, 0);
                if mirror {
                    v.take_mirror();
                }
            }
            let held = (v.backlog.borrow().pruned, v.catch_ups.get() - joined);
            // The CPU reads pixel (65, 5) of the last frame.
            let pixel = v.read(LINEAR + b + 2 * (2048 + 5 * 64 + 1), Now::default());
            (held, pixel, v.frame_buffer().to_bytes())
        };
        let ((pruned, catch_ups), pixel, recorded) = run(true);
        assert!(pruned > 0, "the frames drawn over are dropped");
        assert_eq!(catch_ups, 0, "nothing read the memory while they were held");
        let (_, software_pixel, software) = run(false);
        assert_eq!(pixel, software_pixel);
        assert!(recorded == software, "the memory ends up the same");
    }

    #[test]
    fn frames_drawn_over_after_a_depth_clear_drop_what_was_under_them() {
        // A game that clears only the depth and draws over the whole
        // picture each frame, double buffered. The background is drawn
        // with "less than or equal"; from frame 4 on, a depth-only
        // triangle nearer than it goes first in a corner, where the
        // background then fails its test and the colour of two frames
        // before stays. A blended triangle goes on top.
        let (a, b) = (0x10_0000, 0x20_0000);
        let depth_only = 1 | 1 << 10;
        let background = 1 | 1 << 4 | 3 << 5 | 1 << 9 | 1 << 10;
        let run = |mirror: bool| {
            let mut v = mirrored();
            v.pool = super::super::super::workers::Pool::new(2);
            v.set_mirror(mirror);
            reg3d(&mut v, COL_BUFFER_STRIDE, 1 << 15 | 10);
            reg3d(&mut v, AUX_BUFFER_ADDR, 0x40_0000);
            reg3d(&mut v, AUX_BUFFER_STRIDE, 1 << 15 | 10);
            reg3d(&mut v, CLIP_LEFT_RIGHT, 640);
            reg3d(&mut v, CLIP_LOW_Y_HIGH_Y, 64);
            reg3d(&mut v, ZA_COLOR, 0xFFFF);
            let mut joined = 0;
            for frame in 0..10u32 {
                if frame == 2 {
                    joined = v.catch_ups.get();
                }
                let draw = if frame % 2 == 0 { a } else { b };
                reg3d(&mut v, COL_BUFFER_ADDR, draw);
                reg3d(&mut v, ALPHA_MODE, 0);
                reg3d(&mut v, FBZ_MODE, 1 << 10);
                reg3d(&mut v, FASTFILL_CMD, 0);
                if frame >= 4 {
                    reg3d(&mut v, FBZ_MODE, depth_only);
                    float(&mut v, FSTART_Z, 0.0);
                    rectangle(&mut v, (0.0, 0.0, 20.0, 12.0), (0.0, 0.0, 0.0));
                }
                reg3d(&mut v, FBZ_MODE, background);
                float(&mut v, FSTART_Z, 1000.0);
                let shade = (frame * 25) as f32;
                let colour = (shade, 255.0 - shade, 60.0);
                rectangle(&mut v, (0.0, 0.0, 640.0, 64.0), colour);
                // Blended: source alpha and one minus it.
                reg3d(&mut v, ALPHA_MODE, 1 << 4 | 1 << 8 | 5 << 12);
                float(&mut v, FSTART_A, 128.0);
                rectangle(&mut v, (100.0, 10.0, 200.0, 50.0), (255.0, 255.0, 255.0));
                reg3d(&mut v, LEFT_OVERLAY_BUF, draw);
                reg3d(&mut v, SWAPBUFFER_CMD, 0);
                if mirror {
                    v.take_mirror();
                }
            }
            let held = (v.backlog.borrow().pruned, v.catch_ups.get() - joined);
            (held, v.frame_buffer().to_bytes())
        };
        let ((pruned, catch_ups), recorded) = run(true);
        assert!(pruned > 0, "the frames drawn over are dropped");
        assert_eq!(catch_ups, 0);
        let (_, software) = run(false);
        assert!(recorded == software, "the memory ends up the same");
    }

    #[test]
    fn triangles_textured_from_a_colour_buffer_keep_what_they_read() {
        // A Banshee's texture units read the frame buffer's memory, so a
        // triangle can draw with a colour buffer's pixels as its texture.
        // Each frame clears the depth, covers the picture, then draws a
        // small square over pixel (0, 0), in a tile of its own. Frame 8
        // first draws with buffer A's own pixel (0, 0) as texture, before
        // the background covers it; frame 9 draws into B with A's. A
        // fastfill of A's first tile comes last. What these read has to
        // be drawn, and stay what the software path draws.
        let (a, b) = (0x10_0000u32, 0x20_0000u32);
        let near = 1 | 1 << 4 | 3 << 5 | 1 << 9 | 1 << 10;
        let decal = 10 << 8 | 1 << 12 | 1 << 18 | 1 << 21 | 1 << 27;
        let textured = |v: &mut Voodoo, from: u32, x: f32| {
            reg3d(v, TEX_BASE_ADDR, from);
            reg3d(v, T_LOD, 0);
            reg3d(v, TEXTURE_MODE, decal);
            reg3d(v, FBZ_COLOR_PATH, 1 | 1 << 27);
            reg3d(v, FBZ_MODE, near);
            float(v, FSTART_Z, 0.0);
            rectangle(v, (x, 20.0, x + 40.0, 40.0), (0.0, 0.0, 0.0));
            reg3d(v, FBZ_COLOR_PATH, 0);
        };
        let run = |mirror: bool| {
            // No workers: each draws its own rows, and a texture
            // read from another's rows doesn't wait for it.
            let mut v = mirrored();
            v.set_mirror(mirror);
            reg3d(&mut v, COL_BUFFER_STRIDE, 1 << 15 | 10);
            reg3d(&mut v, AUX_BUFFER_ADDR, 0x40_0000);
            reg3d(&mut v, AUX_BUFFER_STRIDE, 1 << 15 | 10);
            reg3d(&mut v, CLIP_LEFT_RIGHT, 640);
            reg3d(&mut v, CLIP_LOW_Y_HIGH_Y, 64);
            reg3d(&mut v, ZA_COLOR, 0xFFFF);
            for frame in 0..10u32 {
                let draw = if frame % 2 == 0 { a } else { b };
                reg3d(&mut v, COL_BUFFER_ADDR, draw);
                reg3d(&mut v, FBZ_MODE, 1 << 10);
                reg3d(&mut v, FASTFILL_CMD, 0);
                if frame == 8 {
                    textured(&mut v, a, 400.0);
                }
                reg3d(&mut v, FBZ_MODE, near);
                float(&mut v, FSTART_Z, 1000.0);
                let shade = (frame * 25) as f32;
                let colour = (shade, 255.0 - shade, 60.0);
                rectangle(&mut v, (0.0, 0.0, 640.0, 64.0), colour);
                float(&mut v, FSTART_Z, 500.0);
                rectangle(&mut v, (0.0, 0.0, 8.0, 4.0), (60.0, shade, 255.0));
                if frame == 9 {
                    textured(&mut v, a, 500.0);
                }
                reg3d(&mut v, LEFT_OVERLAY_BUF, draw);
                reg3d(&mut v, SWAPBUFFER_CMD, 0);
                if mirror {
                    v.take_mirror();
                }
            }
            reg3d(&mut v, COL_BUFFER_ADDR, a);
            reg3d(&mut v, CLIP_LEFT_RIGHT, 80);
            reg3d(&mut v, CLIP_LOW_Y_HIGH_Y, 8);
            reg3d(&mut v, FBZ_MODE, 1 | 1 << 9);
            reg3d(&mut v, COLOR1, 0xFF_FFFF);
            reg3d(&mut v, FASTFILL_CMD, 0);
            let memory = v.frame_buffer().to_bytes();
            (v.backlog.borrow().pruned, memory)
        };
        let (pruned, recorded) = run(true);
        assert!(pruned > 0, "the frames drawn over are dropped");
        let (_, software) = run(false);
        // Pixel (420, 30) of A and (520, 30) of B, which the textured
        // squares drew: row 30 of the first row of tiles.
        let pixel = |memory: &[u8], base: u32, x: u32| {
            let at = (base / 2 + (x / 64) * 2048 + 30 * 64 + x % 64) as usize * 2;
            u16::from_le_bytes([memory[at], memory[at + 1]])
        };
        for (base, x) in [(a, 420), (b, 520)] {
            assert_eq!(pixel(&recorded, base, x), pixel(&software, base, x));
        }
        assert!(recorded == software, "the memory ends up the same");
    }

    use super::super::super::gpu::TestRenderer as Renderer;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// Two colour buffers of 640 by 64 in tiles, and a depth buffer. The
    /// renderer (`TestRenderer`) drew both in 0x1234, as their word
    /// offsets end in 16 zero bits.
    pub(crate) const A: u32 = 0x10_0000;
    pub(crate) const B: u32 = 0x20_0000;

    /// A card whose 3D buffers are read back from the renderer
    /// (`voodoo_readback=gpu`), with `TestRenderer` as the renderer, or
    /// with None from the software rasterizer.
    fn gpu_card(log: Option<&Rc<RefCell<Vec<String>>>>) -> Voodoo {
        let mut v = mirrored();
        v.set_software_picture(false);
        if let Some(log) = log {
            v.set_gpu_link(Some(Box::new(Renderer(Rc::clone(log)))));
            v.set_gpu_reads(true);
        }
        reg3d(&mut v, COL_BUFFER_STRIDE, 1 << 15 | 10);
        reg3d(&mut v, AUX_BUFFER_ADDR, 0x40_0000);
        reg3d(&mut v, AUX_BUFFER_STRIDE, 1 << 15 | 10);
        reg3d(&mut v, CLIP_LEFT_RIGHT, 640);
        reg3d(&mut v, CLIP_LOW_Y_HIGH_Y, 64);
        reg3d(&mut v, ZA_COLOR, 0xFFFF);
        v
    }

    /// For tests elsewhere: a card whose renderer (`TestRenderer`, which
    /// notes its reads in `log`) alone drew a frame of 640 by 64 into a
    /// linear colour buffer at byte `A`, 1280 bytes a row, in 0x1234.
    pub(crate) fn gpu_frame_in_linear_buffer(log: &Rc<RefCell<Vec<String>>>) -> Voodoo {
        let mut v = gpu_card(Some(log));
        reg3d(&mut v, COL_BUFFER_STRIDE, 1280);
        reg3d(&mut v, AUX_BUFFER_STRIDE, 1280);
        draw_frame(&mut v, A);
        v
    }

    /// A frame drawn into `buffer`: the depth cleared, the picture
    /// covered, the buffer shown.
    fn draw_frame(v: &mut Voodoo, buffer: u32) {
        reg3d(v, COL_BUFFER_ADDR, buffer);
        reg3d(v, FBZ_MODE, 1 << 10);
        reg3d(v, FASTFILL_CMD, 0);
        reg3d(v, FBZ_MODE, 1 | 1 << 4 | 3 << 5 | 1 << 9 | 1 << 10);
        float(v, FSTART_Z, 1000.0);
        rectangle(v, (0.0, 0.0, 640.0, 64.0), (200.0, 30.0, 60.0));
        reg3d(v, LEFT_OVERLAY_BUF, buffer);
        reg3d(v, SWAPBUFFER_CMD, 0);
        v.take_mirror();
    }

    /// A rectangle into B textured from A in 5-6-5: the first texel of the
    /// texture the recording decoded for it, None if it decoded none (it
    /// had the texture already).
    fn textured(v: &mut Voodoo) -> Option<u32> {
        textured_from(v, A)
    }

    /// The same with the texture at byte `base`.
    pub(crate) fn textured_from(v: &mut Voodoo, base: u32) -> Option<u32> {
        reg3d(v, COL_BUFFER_ADDR, B);
        reg3d(v, TEX_BASE_ADDR, base);
        reg3d(v, T_LOD, 0);
        let mode = 10 << 8 | 1 << 12 | 1 << 18 | 1 << 21 | 1 << 27;
        reg3d(v, TEXTURE_MODE, mode);
        reg3d(v, FBZ_COLOR_PATH, 1 | 1 << 27);
        reg3d(v, FBZ_MODE, 1 | 1 << 9);
        rectangle(v, (400.0, 20.0, 440.0, 40.0), (0.0, 0.0, 0.0));
        reg3d(v, FBZ_COLOR_PATH, 0);
        let frame = v.take_mirror().unwrap();
        frame.commands.iter().rev().find_map(|c| match c {
            Command::Texture(t) => t.levels.first()?.argb.first().copied(),
            _ => None,
        })
    }

    pub(crate) fn reads(log: &Rc<RefCell<Vec<String>>>) -> Vec<String> {
        let log = log.borrow();
        log.iter()
            .filter(|l| l.starts_with("read"))
            .cloned()
            .collect()
    }

    pub(crate) fn gpu_only(v: &Voodoo) -> u64 {
        v.describe(Now::default())["gpu_only_jobs"]
            .as_u64()
            .unwrap()
    }

    #[test]
    fn gpu_reads_leave_the_3d_to_the_renderer() {
        // With voodoo_readback=gpu the triangles and fastfills are drawn
        // by the renderer alone. A read of the memory through BAR1 reads
        // back the band of 32 rows it is in, and makes it hot: the next
        // frame's triangles into that buffer are drawn in software, over
        // the renderer's pixels, as without the renderer.
        let log = Rc::default();
        let mut v = gpu_card(Some(&log));
        draw_frame(&mut v, A);
        assert_eq!(gpu_only(&v), 3);
        assert_eq!(v.describe(Now::default())["held_jobs"], 0);
        assert_eq!(v.fbi.ram.get(A as usize / 2), 0, "the memory waits");
        assert!(reads(&log).is_empty());
        let mut pixel = [0u8; 2];
        v.banshee_memory_read(A, &mut pixel);
        assert_eq!(u16::from_le_bytes(pixel), 0x1234);
        assert_eq!(reads(&log), [format!("read {:?} 0..32", Some(A / 2))]);
        assert_eq!(v.describe(Now::default())["gpu_read_backs"], 1);
        // The second band of rows is still the renderer's alone.
        let band = 10 * 2048;
        assert_eq!(v.fbi.ram.get(A as usize / 2 + band), 0);
        // Hot, A is drawn in software with its depth clear, and only
        // the rows of A and the depth buffer come back for it; B goes on
        // on the GPU.
        log.borrow_mut().clear();
        draw_frame(&mut v, A);
        assert_eq!(gpu_only(&v), 3, "A's frame is drawn in software");
        assert_eq!(
            reads(&log),
            [
                "read None 0..64".to_string(),
                format!("read {:?} 32..64", Some(A / 2))
            ]
        );
        let mut sw = gpu_card(None);
        draw_frame(&mut sw, A);
        let memory = v.frame_buffer();
        assert_eq!(
            memory.get(A as usize / 2),
            sw.frame_buffer().get(A as usize / 2)
        );
        draw_frame(&mut v, B);
        assert_eq!(gpu_only(&v), 6);
    }

    /// A card as `gpu_card` gives, with buffers 256 rows high, whose
    /// renderer alone drew a frame into A; then the program read A's first
    /// pixel, which brought back the band of rows it is in and made A hot.
    fn gpu_card_hot_after_a_read(log: &Rc<RefCell<Vec<String>>>) -> Voodoo {
        gpu_card_hot_after_reading(log, A, Some(A / 2))
    }

    /// The same with the first pixel at byte `at` read, of the buffer
    /// whose key in the recording is `key`.
    fn gpu_card_hot_after_reading(
        log: &Rc<RefCell<Vec<String>>>,
        at: u32,
        key: Option<u32>,
    ) -> Voodoo {
        let mut v = gpu_card(Some(log));
        v.banshee.as_mut().unwrap().crtc[0x12] = 255;
        reg3d(&mut v, CLIP_LOW_Y_HIGH_Y, 256);
        reg3d(&mut v, COL_BUFFER_ADDR, A);
        reg3d(&mut v, FBZ_MODE, 1 << 10);
        reg3d(&mut v, FASTFILL_CMD, 0);
        reg3d(&mut v, FBZ_MODE, 1 | 1 << 4 | 3 << 5 | 1 << 9 | 1 << 10);
        float(&mut v, FSTART_Z, 1000.0);
        rectangle(&mut v, (0.0, 0.0, 640.0, 256.0), (200.0, 30.0, 60.0));
        assert_eq!(gpu_only(&v), 3);
        let mut pixel = [0u8; 2];
        v.banshee_memory_read(at, &mut pixel);
        assert_eq!(reads(log), [format!("read {key:?} 0..32")]);
        log.borrow_mut().clear();
        v
    }

    /// The word of the first pixel of row `y` of a buffer at byte `base`,
    /// tiled 10 tiles wide as `gpu_card` makes them.
    fn tiled_row(base: u32, y: usize) -> usize {
        base as usize / 2 + y / 32 * 10 * 2048 + y % 32 * 64
    }

    #[test]
    fn gpu_reads_bring_back_whole_buffers_for_the_first_job_drawn_in_software() {
        // A job into a hot buffer is drawn in software, over the
        // renderer's pixels: the first one that draws over rows still
        // behind brings back the rest of its colour buffer and its depth
        // buffer in one read each, as the jobs after it are drawn in
        // software too. Test Drive 4 reads two pixels of the back buffer
        // in the middle of a frame, and each triangle after that read
        // back a band of its own, waiting for the GPU every time.
        let log = Rc::default();
        let mut v = gpu_card_hot_after_a_read(&log);
        float(&mut v, FSTART_Z, 500.0);
        rectangle(&mut v, (0.0, 100.0, 64.0, 110.0), (0.0, 255.0, 0.0));
        assert_eq!(gpu_only(&v), 3, "drawn in software");
        assert_eq!(
            reads(&log),
            [
                format!("read {:?} 32..256", Some(A / 2)),
                "read None 0..256".to_string()
            ]
        );
        log.borrow_mut().clear();
        rectangle(&mut v, (0.0, 200.0, 64.0, 210.0), (0.0, 255.0, 0.0));
        assert!(reads(&log).is_empty(), "{:?}", reads(&log));
        // The memory has the renderer's frame under what was drawn since.
        let memory = v.frame_buffer();
        assert_eq!(memory.get(tiled_row(A, 150)), 0x1234);
        assert_ne!(memory.get(tiled_row(A, 105)), 0x1234);
    }

    #[test]
    fn gpu_reads_of_depth_rows_bring_back_only_the_rows_of_a_job_over_them() {
        // A read of one depth (3DMark 2000's lens flares) makes its band
        // of depth rows hot, not the colour buffer: a triangle over the
        // band is drawn in software after reading back its own band of
        // colour rows, not the whole buffer, as the triangles elsewhere
        // still go to the GPU.
        let log = Rc::default();
        let mut v = gpu_card_hot_after_reading(&log, 0x40_0000, None);
        float(&mut v, FSTART_Z, 500.0);
        rectangle(&mut v, (0.0, 0.0, 64.0, 10.0), (0.0, 255.0, 0.0));
        assert_eq!(gpu_only(&v), 3, "drawn in software");
        assert_eq!(reads(&log), [format!("read {:?} 0..32", Some(A / 2))]);
        rectangle(&mut v, (0.0, 100.0, 64.0, 110.0), (0.0, 255.0, 0.0));
        assert_eq!(gpu_only(&v), 5, "both triangles on the GPU");
    }

    #[test]
    fn gpu_reads_hand_jobs_into_a_hot_buffer_to_the_workers_at_once() {
        // The program reads a hot buffer back every frame, so its jobs
        // aren't held back for a read that may never come: held, a read
        // in the middle of the frame would wait for all of them to be
        // drawn then, instead of the few the workers haven't done yet.
        let log = Rc::default();
        let mut v = gpu_card_hot_after_a_read(&log);
        assert_eq!(v.describe(Now::default())["held_jobs"], 0);
        rectangle(&mut v, (0.0, 100.0, 64.0, 110.0), (0.0, 255.0, 0.0));
        rectangle(&mut v, (0.0, 200.0, 64.0, 210.0), (0.0, 255.0, 0.0));
        assert_eq!(gpu_only(&v), 3, "drawn in software");
        assert_eq!(v.describe(Now::default())["held_jobs"], 0);
        assert_eq!(v.describe(Now::default())["catch_ups"], 0);
        let memory = v.frame_buffer();
        assert_ne!(memory.get(tiled_row(A, 105)), 0x1234);
        assert_ne!(memory.get(tiled_row(A, 205)), 0x1234);
    }

    #[test]
    fn gpu_reads_texture_from_a_3d_buffer_with_the_renderers_pixels() {
        // A triangle in B textured from A, which the renderer alone drew:
        // A comes back from the renderer before the texture is decoded
        // from it, so the renderer draws with its own pixels, and so does
        // the software rasterizer.
        let log = Rc::default();
        let mut v = gpu_card(Some(&log));
        draw_frame(&mut v, A);
        draw_frame(&mut v, B);
        let texel = textured(&mut v);
        assert_eq!(v.fbi.ram.get(A as usize / 2), 0x1234);
        let read = format!("read {:?}", Some(A / 2));
        assert!(
            reads(&log).iter().any(|l| l.starts_with(&read)),
            "{:?}",
            log.borrow()
        );
        // The texture the frame draws with has A's pixels as the
        // renderer drew them: 0x1234 in 5-6-5, widened.
        assert_eq!(texel, Some(0xFF10_45A5), "{texel:x?}");
    }

    #[test]
    fn gpu_reads_decode_a_texture_again_after_its_buffer_was_read_back() {
        // A texture decoded from A, then A drawn by the renderer alone and
        // read back by the 2D engine: the memory under the texture
        // changed, and the next triangle decodes it again from there.
        let log = Rc::default();
        let mut v = gpu_card(Some(&log));
        assert!(textured(&mut v).is_some());
        draw_frame(&mut v, A);
        blit_2d(&mut v, true, (0, 0), (8, 1));
        assert_eq!(v.fbi.ram.get(A as usize / 2), 0x1234);
        let texel = textured(&mut v);
        assert_eq!(texel, Some(0xFF10_45A5), "{texel:x?}");
    }

    #[test]
    fn gpu_reads_decode_a_texture_again_after_its_buffer_left_the_layout() {
        // A program that draws into a texture and then draws with it from
        // another layout (Giants composes its ground textures so, with a
        // render target the size of the texture): the texture was decoded
        // from A before, A is then drawn by the renderer alone, and the
        // next triangle draws into B with another depth buffer, a layout
        // without A. A comes back from the renderer as the layout changes,
        // and the triangle decodes the texture again from it.
        let log = Rc::default();
        let mut v = gpu_card(Some(&log));
        assert!(textured(&mut v).is_some());
        draw_frame(&mut v, A);
        reg3d(&mut v, AUX_BUFFER_ADDR, 0x50_0000);
        let texel = textured(&mut v);
        assert_eq!(v.fbi.ram.get(A as usize / 2), 0x1234);
        assert_eq!(texel, Some(0xFF10_45A5), "{texel:x?}");
    }

    #[test]
    fn queued_frame_buffer_writes_leave_textures_outside_the_buffers() {
        // Frame buffer writes go only into the buffers drawn now. Queued
        // behind held jobs, they don't make a triangle textured from
        // memory elsewhere draw the jobs held or decode its texture again.
        let mut v = mirrored();
        v.pool = super::super::super::workers::Pool::new(2);
        reg3d(&mut v, COL_BUFFER_STRIDE, 1 << 15 | 10);
        reg3d(&mut v, AUX_BUFFER_ADDR, 0x40_0000);
        reg3d(&mut v, AUX_BUFFER_STRIDE, 1 << 15 | 10);
        reg3d(&mut v, CLIP_LEFT_RIGHT, 640);
        reg3d(&mut v, CLIP_LOW_Y_HIGH_Y, 64);
        reg3d(&mut v, COL_BUFFER_ADDR, A);
        let textured = |v: &mut Voodoo| {
            reg3d(v, TEX_BASE_ADDR, 0x30_0000);
            reg3d(v, T_LOD, 0);
            let mode = 10 << 8 | 1 << 12 | 1 << 18 | 1 << 21 | 1 << 27;
            reg3d(v, TEXTURE_MODE, mode);
            reg3d(v, FBZ_COLOR_PATH, 1 | 1 << 27);
            reg3d(v, FBZ_MODE, 1 | 1 << 9);
            rectangle(v, (400.0, 20.0, 440.0, 40.0), (0.0, 0.0, 0.0));
            reg3d(v, FBZ_COLOR_PATH, 0);
        };
        let decoded = |frame: &Frame| {
            let textures = frame.commands.iter();
            textures
                .filter(|c| matches!(c, Command::Texture(_)))
                .count()
        };
        textured(&mut v);
        assert!(decoded(&v.take_mirror().unwrap()) > 0);
        reg3d(&mut v, FBZ_MODE, 1 << 9);
        rectangle(&mut v, (0.0, 0.0, 64.0, 32.0), (255.0, 0.0, 0.0));
        reg3d(&mut v, LFB_MODE, 0);
        v.write(
            0x100_0000 + 3 * 4096 + 6 * 2,
            0x07E0_F800,
            u32::MAX,
            Now::default(),
        );
        let held = v.describe(Now::default())["held_jobs"].as_u64().unwrap();
        assert!(held > 0);
        let catch_ups = v.catch_ups.get();
        textured(&mut v);
        assert_eq!(v.catch_ups.get(), catch_ups, "the jobs stay held");
        assert_eq!(decoded(&v.take_mirror().unwrap()), 0);
    }

    #[test]
    fn a_texture_from_a_buffer_drawn_in_software_is_decoded_again() {
        // A texture decoded from A, then A drawn by the software
        // rasterizer, and those jobs done before the next triangle that
        // takes its texture from A: that triangle decodes it again, as a
        // card that decodes it for the first time then does.
        let mut v = gpu_card(None);
        assert!(textured(&mut v).is_some());
        draw_frame(&mut v, A);
        v.flush();
        let mut fresh = gpu_card(None);
        draw_frame(&mut fresh, A);
        let texel = textured(&mut fresh);
        assert!(texel.is_some());
        assert_eq!(textured(&mut v), texel, "{texel:x?}");
    }

    #[test]
    fn gpu_reads_leave_alpha_planes_another_buffer_wrote_to_software() {
        // The renderer keeps the alpha planes in the alpha of the colour
        // buffer drawn into: writing them stays with it, and so does
        // blending by them into that buffer. Blending into another buffer
        // by the alpha written with A reads alpha the renderer doesn't
        // have there, and the software rasterizer draws from then on.
        let log = Rc::default();
        let mut v = gpu_card(Some(&log));
        reg3d(&mut v, COL_BUFFER_ADDR, A);
        reg3d(&mut v, FBZ_MODE, 1 << 9 | 1 << 10 | 1 << 18);
        rectangle(&mut v, (0.0, 0.0, 64.0, 32.0), (255.0, 0.0, 0.0));
        reg3d(&mut v, ALPHA_MODE, 1 << 4 | 3 << 8 | 7 << 12);
        reg3d(&mut v, FBZ_MODE, 1 << 9 | 1 << 18);
        rectangle(&mut v, (0.0, 0.0, 64.0, 32.0), (0.0, 255.0, 0.0));
        assert_eq!(gpu_only(&v), 4);
        reg3d(&mut v, COL_BUFFER_ADDR, B);
        rectangle(&mut v, (0.0, 0.0, 64.0, 32.0), (0.0, 0.0, 255.0));
        assert_eq!(gpu_only(&v), 4);
        assert!(v.describe(Now::default())["gpu_alpha"].as_bool().unwrap());
    }

    #[test]
    fn gpu_reads_of_the_depth_buffer_after_a_colour_fastfill_read_nothing_back() {
        // A fastfill writes the depth buffer only with fbzMode's aux
        // write mask (`fastfill_row`), whatever the depth test bit says:
        // after one that fills only the colour buffer, the depth buffer
        // isn't behind the renderer.
        let log = Rc::default();
        let mut v = gpu_card(Some(&log));
        reg3d(&mut v, COL_BUFFER_ADDR, A);
        reg3d(&mut v, FBZ_MODE, 1 << 4 | 1 << 9);
        reg3d(&mut v, FASTFILL_CMD, 0);
        assert_eq!(gpu_only(&v), 1);
        let mut pixel = [0u8; 2];
        v.banshee_memory_read(0x40_0000, &mut pixel);
        assert!(reads(&log).is_empty(), "{:?}", log.borrow());
        v.banshee_memory_read(A, &mut pixel);
        assert_eq!(reads(&log), [format!("read {:?} 0..32", Some(A / 2))]);
    }

    /// A 64x32 5-6-5 texture whose levels start right after the depth
    /// buffer of `gpu_card` made linear, 1280 bytes a row and 64 rows to
    /// 0x41_4000, as Glide points the card at it: grTexSource sets
    /// texBaseAddr where a 256x128 level 0 would start, the sizes of the
    /// 256x128 and 128x64 levels it hasn't below the texture, which is at
    /// the depth buffer's first byte here. tLOD's LOD range is levels 2
    /// (64x32) to 8, aspect 2:1 with s wider.
    const BELOW_DEPTH: u32 = 0x41_4000 - 2 * (256 * 128 + 128 * 64);

    /// A rectangle into A textured with the texture at `BELOW_DEPTH`'s
    /// levels: the frame the recording made of it. A, as `draw_frame`
    /// left the layout: drawing into another buffer would read the
    /// layout's buffers back as it changes.
    fn textured_above_depth(v: &mut Voodoo) -> Frame {
        reg3d(v, COL_BUFFER_ADDR, A);
        reg3d(v, TEX_BASE_ADDR, BELOW_DEPTH);
        reg3d(v, T_LOD, 8 | 32 << 6 | 1 << 20 | 1 << 21);
        let mode = 10 << 8 | 1 << 12 | 1 << 18 | 1 << 21 | 1 << 27;
        reg3d(v, TEXTURE_MODE, mode);
        reg3d(v, FBZ_COLOR_PATH, 1 | 1 << 27);
        reg3d(v, FBZ_MODE, 1 | 1 << 4 | 3 << 5 | 1 << 9 | 1 << 10);
        rectangle(v, (400.0, 20.0, 440.0, 40.0), (0.0, 0.0, 0.0));
        reg3d(v, FBZ_COLOR_PATH, 0);
        v.take_mirror().unwrap()
    }

    #[test]
    fn gpu_reads_a_texture_above_the_depth_buffer_reads_nothing_back() {
        // Giants on the built-in Windows (#487): the HAL puts textures
        // right after the depth buffer, and Glide's texBaseAddr for one
        // smaller than 256x256 points into the depth buffer's last rows.
        // The texture units read only the levels the LOD range picks,
        // which lie above it, so a textured triangle reads no depth rows
        // back from the renderer, though the renderer alone drew them.
        let log = Rc::default();
        let mut v = gpu_card(Some(&log));
        reg3d(&mut v, COL_BUFFER_STRIDE, 1280);
        reg3d(&mut v, AUX_BUFFER_STRIDE, 1280);
        draw_frame(&mut v, A);
        assert!(v.describe(Now::default())["gpu_only_jobs"].as_u64() > Some(0));
        log.borrow_mut().clear();
        textured_above_depth(&mut v);
        assert!(reads(&log).is_empty(), "{:?}", log.borrow());
        assert_eq!(v.describe(Now::default())["gpu_read_backs"], 0);
        // The same texture with level 0 in its LOD range reads from the
        // depth buffer's rows, and they come back first.
        reg3d(&mut v, T_LOD, 32 << 6 | 1 << 20 | 1 << 21);
        reg3d(&mut v, FBZ_COLOR_PATH, 1 | 1 << 27);
        rectangle(&mut v, (400.0, 20.0, 440.0, 40.0), (0.0, 0.0, 0.0));
        assert!(
            reads(&log).iter().any(|l| l.starts_with("read None")),
            "{:?}",
            log.borrow()
        );
    }

    #[test]
    fn a_write_below_a_textures_levels_doesnt_decode_it_again() {
        // The recording decodes a texture again when the memory its
        // levels lie in changes. A write into the depth buffer under
        // `BELOW_DEPTH`'s texture, where its base address points but no
        // level it reads is, leaves it as it is; one into its first
        // level decodes it again.
        use super::super::LINEAR;
        let decoded = |frame: &Frame| {
            let textures = frame.commands.iter();
            textures
                .filter(|c| matches!(c, Command::Texture(_)))
                .count()
        };
        let mut v = gpu_card(None);
        reg3d(&mut v, COL_BUFFER_STRIDE, 1280);
        reg3d(&mut v, AUX_BUFFER_STRIDE, 1280);
        assert!(decoded(&textured_above_depth(&mut v)) > 0);
        assert_eq!(decoded(&textured_above_depth(&mut v)), 0);
        let now = Now::default();
        v.write(LINEAR + 0x41_3000, 0xF800, 0xFFFF, now);
        assert_eq!(decoded(&textured_above_depth(&mut v)), 0);
        v.write(LINEAR + 0x41_4000, 0xF800, 0xFFFF, now);
        assert!(decoded(&textured_above_depth(&mut v)) > 0);
    }

    /// Where `blit_2d` copies to: a linear surface of 16-bit pixels,
    /// 1280 bytes a row.
    const DESKTOP: u32 = 0x60_0000;

    /// A screen to screen copy by the 2D engine of `size` 16-bit pixels
    /// from (`from`) of A, in tiles if `tiled` (as `gpu_card` has it),
    /// else 1280 bytes a row, to (0, 0) of `DESKTOP`.
    fn blit_2d(v: &mut Voodoo, tiled: bool, from: (u32, u32), size: (u32, u32)) {
        let src = if tiled {
            (A | 1 << 31, 3 << 16 | 10)
        } else {
            (A, 3 << 16 | 1280)
        };
        for (reg, value) in [
            (0x08, 0),
            (0x0C, 0x0FFF_0FFF),
            (0x10, DESKTOP),
            (0x14, 3 << 16 | 1280),
            (0x34, src.0),
            (0x54, src.1),
            (0x5C, from.1 << 16 | from.0),
            (0x68, size.1 << 16 | size.0),
            (0x6C, 0),
            (0x70, 0xCC << 24 | 1 << 8 | 1),
        ] {
            v.banshee_2d_write(reg, value);
        }
    }

    /// Pixel (`x`, `y`) of `DESKTOP`.
    fn desktop(v: &Voodoo, x: u32, y: u32) -> u16 {
        v.fbi.ram.get(((DESKTOP + y * 1280) / 2 + x) as usize)
    }

    #[test]
    fn gpu_reads_2d_register_writes_that_run_no_command_read_nothing_back() {
        // The 2D engine reads and draws only when a command runs: the
        // registers set up before then leave the renderer's buffers on
        // the GPU.
        let log = Rc::default();
        let mut v = gpu_card(Some(&log));
        draw_frame(&mut v, A);
        for (reg, value) in [
            (0x08, 0),
            (0x0C, 0x0FFF_0FFF),
            (0x10, A),
            (0x14, 3 << 16 | 1280),
            (0x64, 0xF800),
            (0x68, 4 << 16 | 4),
            (0x6C, 0),
            // A fill without the initiate bit, and a host to screen blit
            // still waiting for its data.
            (0x70, 0xCC << 24 | 5),
            (0x70, 0xCC << 24 | 1 << 8 | 3),
        ] {
            v.banshee_2d_write(reg, value);
        }
        assert!(reads(&log).is_empty(), "{:?}", log.borrow());
        assert_eq!(v.fbi.ram.get(A as usize / 2), 0, "the memory waits");
    }

    #[test]
    fn gpu_reads_bring_back_the_rows_a_blit_reads() {
        // A blit from a buffer only the renderer drew: the rows it reads
        // come back first, the band of 32 rows they are in for a tiled
        // buffer, and the depth buffer stays on the GPU. Read back, the
        // rows are the memory's, and the same blit again reads nothing.
        let log = Rc::default();
        let mut v = gpu_card(Some(&log));
        draw_frame(&mut v, A);
        blit_2d(&mut v, true, (0, 0), (8, 1));
        assert_eq!(reads(&log), [format!("read {:?} 0..32", Some(A / 2))]);
        assert_eq!(desktop(&v, 7, 0), 0x1234);
        assert_eq!(v.fbi.ram.get(0x40_0000 / 2), 0, "the depth waits");
        blit_2d(&mut v, true, (0, 0), (8, 1));
        assert_eq!(reads(&log).len(), 1, "read back once");
    }

    #[test]
    fn gpu_reads_a_blit_reads_back_only_its_rectangle() {
        // In a linear buffer the rows are read back one by one: a copy of
        // 100 by 4 pixels from (20, 10) reads rows 10 to 13 and no other,
        // and copies the renderer's pixels.
        let log = Rc::default();
        let mut v = gpu_frame_in_linear_buffer(&log);
        blit_2d(&mut v, false, (20, 10), (100, 4));
        assert_eq!(reads(&log), [format!("read {:?} 10..14", Some(A / 2))]);
        assert_eq!(v.describe(Now::default())["gpu_read_backs"], 1);
        assert_eq!(desktop(&v, 0, 0), 0x1234);
        assert_eq!(desktop(&v, 99, 3), 0x1234);
        assert_eq!(desktop(&v, 100, 3), 0, "past the rectangle");
        assert_eq!(v.fbi.ram.get(A as usize / 2), 0, "row 0 waits");
    }

    #[test]
    fn gpu_reads_a_buffer_blitted_from_every_frame_is_drawn_in_software() {
        // A windowed game copies its back buffer to the desktop with the
        // 2D engine every frame. The rows the copy reads back are hot, as
        // a program's reads are: the next frame into the buffer is drawn
        // in software, and its copy reads nothing back. The desktop gets
        // what the software path draws.
        let log = Rc::default();
        let mut v = gpu_frame_in_linear_buffer(&log);
        assert_eq!(gpu_only(&v), 3);
        blit_2d(&mut v, false, (0, 0), (640, 64));
        assert_eq!(desktop(&v, 639, 63), 0x1234);
        draw_frame(&mut v, A);
        assert_eq!(gpu_only(&v), 3, "the second frame is drawn in software");
        log.borrow_mut().clear();
        blit_2d(&mut v, false, (0, 0), (640, 64));
        let buffer = format!("read {:?}", Some(A / 2));
        assert!(
            !reads(&log).iter().any(|l| l.starts_with(&buffer)),
            "{:?}",
            log.borrow()
        );
        let mut sw = gpu_card(None);
        reg3d(&mut sw, COL_BUFFER_STRIDE, 1280);
        reg3d(&mut sw, AUX_BUFFER_STRIDE, 1280);
        draw_frame(&mut sw, A);
        blit_2d(&mut sw, false, (0, 0), (640, 64));
        for (x, y) in [(0, 0), (320, 30), (639, 63)] {
            assert_eq!(desktop(&v, x, y), desktop(&sw, x, y), "pixel {x} {y}");
        }
    }

    #[test]
    fn gpu_reads_a_buffer_blitted_from_every_frame_goes_back_to_the_gpu_once_cool() {
        // The rows a copy reads back stay hot for `HOT_FRAMES` frames of
        // the picture. Then one frame into the buffer goes to the GPU,
        // its copy reads it back, and the buffer is hot again: the
        // desktop gets the renderer's pixels in that frame and the
        // software rasterizer's in the others.
        use super::super::super::gpu::HOT_FRAMES;
        let log = Rc::default();
        let mut v = gpu_frame_in_linear_buffer(&log);
        blit_2d(&mut v, false, (0, 0), (640, 64));
        let mut sw = gpu_card(None);
        reg3d(&mut sw, COL_BUFFER_STRIDE, 1280);
        reg3d(&mut sw, AUX_BUFFER_STRIDE, 1280);
        draw_frame(&mut sw, A);
        blit_2d(&mut sw, false, (0, 0), (640, 64));
        let software = desktop(&sw, 320, 30);
        assert_ne!(software, 0x1234);
        let mut on_gpu = Vec::new();
        for frame in 1..=2 * HOT_FRAMES + 2 {
            let before = gpu_only(&v);
            draw_frame(&mut v, A);
            blit_2d(&mut v, false, (0, 0), (640, 64));
            if gpu_only(&v) > before {
                on_gpu.push(frame);
                assert_eq!(desktop(&v, 320, 30), 0x1234, "frame {frame}");
            } else {
                assert_eq!(desktop(&v, 320, 30), software, "frame {frame}");
            }
        }
        // One frame in each period of `HOT_FRAMES` frames goes to the GPU.
        assert_eq!(on_gpu.len(), 2, "{on_gpu:?}");
        assert_eq!(on_gpu[1] - on_gpu[0], HOT_FRAMES, "{on_gpu:?}");
    }

    #[test]
    fn gpu_reads_a_2d_fill_into_a_buffer_on_the_gpu_stays() {
        // A fill by the 2D engine into rows of a buffer only the renderer
        // drew is drawn by the renderer too (`gpu2d`): nothing is read
        // back for it, and the rows stay the renderer's. Reading back the
        // buffer later brings the fill and the renderer's pixels around it.
        let model = Rc::default();
        let mut v = super::super::gpu2d::tests::model_card(&model, false);
        reg3d(&mut v, AUX_BUFFER_STRIDE, 1280);
        super::super::gpu2d::tests::frame(&mut v, &model, A);
        for (reg, value) in [
            (0x08, 0),
            (0x0C, 0x0FFF_0FFF),
            (0x10, A),
            (0x14, 3 << 16 | 1280),
            (0x64, 0xF800),
            (0x68, 2 << 16 | 8),
            (0x6C, 0),
            (0x70, 0xCC << 24 | 1 << 8 | 5),
        ] {
            v.banshee_2d_write(reg, value);
        }
        assert_eq!(v.describe(Now::default())["gpu_read_backs"], 0);
        let picture = |x, y| model.borrow().pixel(Some(A / 2), x, y);
        let (right, below) = (picture(8, 0), picture(0, 2));
        let memory = v.frame_buffer();
        let row = 640;
        assert_eq!(memory.get(A as usize / 2 + 7), 0xF800);
        assert_eq!(memory.get(A as usize / 2 + row + 7), 0xF800);
        assert_eq!(memory.get(A as usize / 2 + 8), right);
        assert_eq!(memory.get(A as usize / 2 + 2 * row), below);
    }

    #[test]
    fn half_mode_buffers_are_half_the_desktop() {
        let mut v = mirrored();
        let cfg = VIDPROC_ENABLE | 1 << 18 | VIDPROC_HALF_MODE;
        v.write(VID_PROC_CFG, cfg, u32::MAX, Now::default());
        reg3d(&mut v, COL_BUFFER_STRIDE, 1280);
        // Two buffers of 32 rows back to back stay apart.
        let (a, b) = (0x10_0000, 0x10_0000 + 32 * 1280);
        for draw in [a, b] {
            reg3d(&mut v, COL_BUFFER_ADDR, draw);
            rectangle(&mut v, (0.0, 0.0, 8.0, 8.0), (255.0, 0.0, 0.0));
        }
        let frame = v.take_mirror().unwrap();
        let [snapshot] = snapshots(&frame)[..] else {
            panic!("one snapshot")
        };
        assert_eq!(snapshot.layout.height, 32);
        assert_eq!(snapshot.layout.color, [a / 2, b / 2]);
    }

    #[test]
    fn swapping_between_buffers_keeps_the_recording() {
        let mut v = mirrored();
        reg3d(&mut v, COL_BUFFER_STRIDE, 1 << 15 | 10);
        reg3d(&mut v, AUX_BUFFER_ADDR, 0x40_0000);
        reg3d(&mut v, AUX_BUFFER_STRIDE, 1 << 15 | 10);
        // Draw into a buffer, show it through the overlay, draw into the
        // other, as a program double buffering does.
        let (a, b) = (0x10_0000, 0x20_0000);
        let mut frames = Vec::new();
        for draw in [a, b, a, b] {
            reg3d(&mut v, COL_BUFFER_ADDR, draw);
            rectangle(&mut v, (0.0, 0.0, 8.0, 8.0), (255.0, 0.0, 0.0));
            reg3d(&mut v, LEFT_OVERLAY_BUF, draw);
            reg3d(&mut v, SWAPBUFFER_CMD, 0);
            frames.push(v.take_mirror().unwrap());
        }
        // The second buffer joins the layout once; then nothing starts
        // over from memory.
        let counts: Vec<usize> = frames.iter().map(|f| snapshots(f).len()).collect();
        assert_eq!(counts, [1, 1, 0, 0]);
        assert_eq!(snapshots(&frames[1])[0].layout.color, [a / 2, b / 2]);
        // Without the overlay on, the desktop's buffer is the one shown.
        assert_eq!(frames[3].front, None);
        let cfg = VIDPROC_ENABLE | 1 << 18 | VIDPROC_OVERLAY_ENABLE;
        v.write(VID_PROC_CFG, cfg, u32::MAX, Now::default());
        assert_eq!(v.take_mirror().unwrap().front, Some(b / 2));
        v.write(VID_DESKTOP_START_ADDR, a, u32::MAX, Now::default());
        v.write(
            VID_PROC_CFG,
            VIDPROC_ENABLE | 1 << 18,
            u32::MAX,
            Now::default(),
        );
        assert_eq!(v.take_mirror().unwrap().front, Some(a / 2));
        // More buffers join up to `COLOR_BUFFERS`, and the one after
        // drops the oldest.
        let mut expect = vec![a, b];
        for draw in [0x30, 0x50, 0x60, 0x70, 0x80, 0x90, 0xA0].map(|mb| mb << 16) {
            reg3d(&mut v, COL_BUFFER_ADDR, draw);
            rectangle(&mut v, (0.0, 0.0, 8.0, 8.0), (255.0, 0.0, 0.0));
            if expect.len() == COLOR_BUFFERS {
                expect.remove(0);
            }
            expect.push(draw);
            let frame = v.take_mirror().unwrap();
            let words: Vec<u32> = expect.iter().map(|c| c / 2).collect();
            assert_eq!(snapshots(&frame)[0].layout.color, words);
        }
        assert_eq!(expect[0], b, "a went first");
        // One that overlaps a buffer replaces it.
        reg3d(&mut v, COL_BUFFER_ADDR, 0x30_0000 + 0x8000);
        rectangle(&mut v, (0.0, 0.0, 8.0, 8.0), (255.0, 0.0, 0.0));
        expect.retain(|&c| c != 0x30_0000);
        expect.push(0x30_8000);
        let frame = v.take_mirror().unwrap();
        let words: Vec<u32> = expect.iter().map(|c| c / 2).collect();
        assert_eq!(snapshots(&frame)[0].layout.color, words);
    }

    /// Colour buffers at byte `at` with rows of `stride` bytes, linear,
    /// `width` pixels wide, and the clip rectangle on all of it, as the
    /// Direct3D HAL points the card at a render target or a surface it
    /// clears (`hal::card_fill`).
    fn render_target(v: &mut Voodoo, at: u32, stride: u32, width: u32) {
        reg3d(v, COL_BUFFER_ADDR, at);
        reg3d(v, COL_BUFFER_STRIDE, stride);
        reg3d(v, CLIP_LEFT_RIGHT, width);
        reg3d(v, CLIP_LOW_Y_HIGH_Y, 64);
    }

    /// A frame as Giants draws one on a Voodoo3 (#541): it clears its back
    /// buffer A (640 pixels a row here) and the depth buffer with
    /// colBufferAddr left at the texture it drew into last, clears two
    /// textures it draws into (64 and 256 pixels a row), and draws into
    /// A and into the wider texture, each test and write depth.
    const SMALL: u32 = 0x60_0000;
    const WIDE: u32 = 0x70_0000;

    fn frame_of_render_targets(v: &mut Voodoo) -> Frame {
        let fill = |v: &mut Voodoo, fbz: u32| {
            reg3d(v, FBZ_MODE, fbz);
            reg3d(v, FASTFILL_CMD, 0);
        };
        let draw = |v: &mut Voodoo, width: f32| {
            reg3d(v, FBZ_MODE, 1 | 1 << 4 | 3 << 5 | 1 << 9 | 1 << 10);
            float(v, FSTART_Z, 1000.0);
            rectangle(v, (0.0, 0.0, width, 64.0), (200.0, 30.0, 60.0));
        };
        render_target(v, A, 1280, 640);
        fill(v, 1 | 1 << 9);
        // The depth buffer's clear: colBufferAddr where the texture
        // left it, the clip rectangle the depth buffer's.
        render_target(v, WIDE, 512, 256);
        reg3d(v, CLIP_LEFT_RIGHT, 640);
        fill(v, 1 | 1 << 10);
        render_target(v, SMALL, 128, 64);
        fill(v, 1 | 1 << 9);
        render_target(v, A, 1280, 640);
        draw(v, 640.0);
        render_target(v, WIDE, 512, 256);
        draw(v, 256.0);
        v.take_mirror().unwrap()
    }

    #[test]
    fn render_targets_of_other_widths_stay_in_the_layout() {
        // Each buffer keeps its own stride and width in the layout: after
        // the frame that brings them in, moving colBufferAddr and
        // colBufferStride between them neither starts the recording over
        // from memory nor reads back what the renderer drew. With one
        // stride for the layout, every move to a buffer of another width
        // did both, about 7 whole buffers read back a frame in Giants.
        let log = Rc::default();
        let mut v = gpu_card(Some(&log));
        reg3d(&mut v, AUX_BUFFER_STRIDE, 1280);
        let first = frame_of_render_targets(&mut v);
        let read_backs = v.describe(Now::default())["gpu_read_backs"].clone();
        let layout = &snapshots(&first).last().unwrap().layout;
        assert_eq!(layout.color, [A / 2, WIDE / 2, SMALL / 2]);
        assert_eq!((layout.width, layout.height), (640, 64));
        let key = v.mirror_layout().unwrap();
        let widths = [A, WIDE, SMALL].map(|at| key.color_at(at).unwrap().width);
        assert_eq!(widths, [640, 256, 64]);
        log.borrow_mut().clear();
        for _ in 0..3 {
            let frame = frame_of_render_targets(&mut v);
            assert!(snapshots(&frame).is_empty());
        }
        assert!(reads(&log).is_empty(), "{:?}", reads(&log));
        assert_eq!(v.describe(Now::default())["gpu_read_backs"], read_backs);
        assert!(gpu_only(&v) > 0);
    }

    #[test]
    fn a_narrow_buffer_reads_back_only_its_own_columns() {
        // The renderer's pictures are as wide as the widest buffer, 640
        // pixels here: a read of the 64 pixels wide texture takes its 64
        // pixels of each row, and leaves the memory after its rows alone.
        let log = Rc::default();
        let mut v = gpu_card(Some(&log));
        reg3d(&mut v, AUX_BUFFER_STRIDE, 1280);
        frame_of_render_targets(&mut v);
        frame_of_render_targets(&mut v);
        log.borrow_mut().clear();
        let mut rows = vec![0u8; 128 * 64];
        v.banshee_memory_read(SMALL, &mut rows);
        assert_eq!(reads(&log), [format!("read {:?} 0..64", Some(SMALL / 2))]);
        let value = (0x1234 ^ (SMALL / 2)) as u16;
        let pixels = rows.as_chunks::<2>().0;
        assert!(pixels.iter().all(|p| u16::from_le_bytes(*p) == value));
        assert_eq!(v.fbi.ram.get((SMALL + 128 * 64) as usize / 2), 0);
    }

    #[test]
    fn a_clear_leaves_the_rows_past_its_clip_in_memory() {
        // The renderer draws a job's rows, those of its clip rectangle:
        // memory past them, such as a texture after a smaller render
        // target, isn't read back from the renderer's picture of the
        // buffer, which is as high as the desktop (64 rows here).
        const TARGET: u32 = 0x50_0000;
        let log = Rc::default();
        let mut v = gpu_card(Some(&log));
        reg3d(&mut v, AUX_BUFFER_STRIDE, 1280);
        frame_of_render_targets(&mut v);
        frame_of_render_targets(&mut v);
        for _ in 0..2 {
            frame_of_render_targets(&mut v);
            render_target(&mut v, TARGET, 128, 64);
            reg3d(&mut v, CLIP_LOW_Y_HIGH_Y, 16);
            reg3d(&mut v, FBZ_MODE, 1 | 1 << 9);
            reg3d(&mut v, FASTFILL_CMD, 0);
            v.take_mirror();
        }
        log.borrow_mut().clear();
        let mut rows = vec![0u8; 128 * 16];
        v.banshee_memory_read(TARGET + 128 * 32, &mut rows);
        assert!(reads(&log).is_empty(), "{:?}", reads(&log));
        v.banshee_memory_read(TARGET, &mut rows);
        assert_eq!(reads(&log), [format!("read {:?} 0..16", Some(TARGET / 2))]);
    }

    #[test]
    fn writes_past_the_3d_engine_reach_the_recording() {
        use super::super::LINEAR;
        let mut v = mirrored();
        reg3d(&mut v, COL_BUFFER_ADDR, 0x10_0000);
        reg3d(&mut v, COL_BUFFER_STRIDE, 1280);
        reg3d(&mut v, AUX_BUFFER_ADDR, 0x20_0000);
        reg3d(&mut v, AUX_BUFFER_STRIDE, 1 << 15 | 10);
        v.take_mirror();
        // The CPU through BAR1: pixels (2, 3) and (3, 3) of the colour
        // buffer, then (65, 33) of the tiled depth.
        let now = Now::default();
        v.write(
            LINEAR + 0x10_0000 + 3 * 1280 + 4,
            0x07E0_F800,
            u32::MAX,
            now,
        );
        let depth = 0x20_0000 + 2 * (10 * 2048 + 2048 + 64);
        v.write(LINEAR + depth, 0x4321_FFFF, 0xFFFF_0000, now);
        let frame = v.take_mirror().unwrap();
        let rows = pixels(&frame);
        assert_eq!(rows.len(), 2);
        assert_eq!(
            (rows[0].dest, rows[0].x, rows[0].y),
            (Some(0x10_0000 / 2), 2, 3)
        );
        assert_eq!(rows[0].values, [0xF800, 0x07E0]);
        assert_eq!((rows[1].dest, rows[1].x, rows[1].y), (None, 65, 33));
        assert_eq!(rows[1].values, [0x4321]);
        // Memory elsewhere isn't any of the buffers' pixels.
        v.write(LINEAR + 0x30_0000, 1, u32::MAX, now);
        assert!(pixels(&v.take_mirror().unwrap()).is_empty());

        // The 2D engine: a rectangle of 12x3 at (1, 2) in the colour
        // buffer. It reaches the recording as the pixels drawn, a run of
        // them per row, not as the rows whole, which would overwrite
        // what the OpenGL renderer drew beside it.
        let reg2d =
            |v: &mut Voodoo, reg: u32, data: u32| v.write(0x10_0000 + reg, data, u32::MAX, now);
        reg2d(&mut v, 0x14, 3 << 16 | 1280);
        reg2d(&mut v, 0x10, 0x10_0000);
        reg2d(&mut v, 0x08, 0);
        reg2d(&mut v, 0x0C, 0x0FFF_0FFF);
        reg2d(&mut v, 0x64, 0x001F);
        reg2d(&mut v, 0x68, 3 << 16 | 12);
        reg2d(&mut v, 0x6C, 2 << 16 | 1);
        reg2d(&mut v, 0x70, 0xCC << 24 | 1 << 8 | 5);
        let frame = v.take_mirror().unwrap();
        let rows = pixels(&frame);
        let covered: Vec<u32> = rows.iter().map(|p| p.y).collect();
        assert_eq!(covered, [2, 3, 4]);
        assert!(rows.iter().all(|p| p.x == 1 && p.values == [0x001F; 12]));
    }

    #[test]
    fn a_run_through_the_pipeline_writes_what_its_dwords_write() {
        // 1-5-5-5 through the pixel pipeline with chroma keying, as Sub
        // Culture writes its overlay: the keyed pixels stay as they were.
        let setup = |v: &mut Voodoo| {
            reg3d(v, COL_BUFFER_ADDR, 0x40_0000);
            reg3d(v, COL_BUFFER_STRIDE, 1 << 15 | 8);
            reg3d(v, LFB_MODE, 2 | 1 << 8);
            reg3d(v, FBZ_MODE, 1 << 1 | 1 << 9);
            reg3d(v, CHROMA_KEY, 0);
            // The first two tiles, 64 pixels by 32 rows each.
            for i in 0..2 * 2048 {
                v.fbi.ram.set(0x40_0000 / 2 + i, 0x1234);
            }
        };
        let data: Vec<u32> = (0..40u32)
            .map(|i| {
                if i % 5 == 0 {
                    0
                } else {
                    i.wrapping_mul(0x9E37_79B9) | 0x8000_8000
                }
            })
            .collect();
        let (mut run, mut each) = (card(), card());
        setup(&mut run);
        setup(&mut each);
        // Pixels 6 to 85 of row 3: into the second tile from pixel 64 on.
        let offset = 0x100_0000 + 3 * 4096 + 6 * 2;
        let index = run
            .lfb_run(offset, 4 * data.len() as u32)
            .expect("in the LFB");
        run.lfb_write_run(index, data.iter().copied());
        for (i, &value) in (0..).zip(&data) {
            each.write(offset + 4 * i, value, u32::MAX, Now::default());
        }
        let base = 0x40_0000 / 2;
        for i in 0..2 * 2048 {
            assert_eq!(
                run.fbi.ram.get(base + i),
                each.fbi.ram.get(base + i),
                "word {i}"
            );
        }
        let (row, next_tile) = (base + 3 * 64, base + 2048 + 3 * 64);
        assert_eq!(run.fbi.ram.get(row + 6), 0x1234, "keyed");
        assert_ne!(run.fbi.ram.get(row + 8), 0x1234);
        assert_ne!(run.fbi.ram.get(next_tile), 0x1234, "pixel 64");
    }

    #[test]
    fn pipeline_writes_are_recorded_from_a_tiled_buffer() {
        let mut v = mirrored();
        reg3d(&mut v, COL_BUFFER_ADDR, 0x10_0000);
        reg3d(&mut v, COL_BUFFER_STRIDE, 1 << 15 | 10);
        v.take_mirror();
        // 5-6-5 through the pixel pipeline: pixel (64, 33).
        reg3d(&mut v, LFB_MODE, 1 << 8);
        let offset = 0x100_0000 + 33 * 4096 + 64 * 2;
        v.write(offset, 0xF800, 0x0000_FFFF, Now::default());
        assert_eq!(v.fbi.ram.get(0x10_0000 / 2 + 10 * 2048 + 2048 + 64), 0xF800);
        let frame = v.take_mirror().unwrap();
        let rows = pixels(&frame);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (rows[0].dest, rows[0].x, rows[0].y),
            (Some(0x10_0000 / 2), 64, 33)
        );
        assert_eq!(rows[0].values, [0xF800]);
    }

    /// The pages of memory a frame's recording carries for the
    /// renderer's copy of it: where each run starts and its bytes.
    fn memory(frame: &Frame) -> Vec<(u32, &[u8])> {
        let runs = frame.commands.iter().filter_map(|c| match c {
            Command::Memory { at, bytes, .. } => Some((*at, &bytes[..])),
            _ => None,
        });
        runs.collect()
    }

    #[test]
    fn the_scanout_sends_the_pages_written_since_the_renderer_had_them() {
        use super::super::super::tracker::PAGE;
        use super::super::{LINEAR, VID_DESKTOP_OVERLAY_STRIDE};
        let mut v = mirrored();
        v.set_software_picture(false);
        let now = Now::default();
        v.write(VID_DESKTOP_OVERLAY_STRIDE, 1280, u32::MAX, now);
        reg3d(&mut v, COL_BUFFER_ADDR, 0x10_0000);
        reg3d(&mut v, COL_BUFFER_STRIDE, 1 << 15 | 10);
        // The desktop at 0 isn't a buffer the renderer draws: its pages
        // go the first time, and not again until something writes them.
        let first = v.take_mirror().unwrap();
        assert_eq!(first.scanout.as_ref().unwrap().desktop.buffer, None);
        assert_eq!(memory(&first).first().map(|m| m.0), Some(0));
        assert!(memory(&v.take_mirror().unwrap()).is_empty());
        v.write(LINEAR + 4, 0x1234_5678, u32::MAX, now);
        let frame = v.take_mirror().unwrap();
        let sent = memory(&frame);
        assert_eq!(sent.len(), 1);
        assert_eq!((sent[0].0, sent[0].1.len()), (0, PAGE as usize));
        assert_eq!(sent[0].1[4..8], [0x78, 0x56, 0x34, 0x12]);
        // Moved onto the colour buffer, it's drawn from there; moved back,
        // the renderer still has its pages, and none go again.
        v.write(VID_DESKTOP_START_ADDR, 0x10_0000, u32::MAX, now);
        v.write(
            VID_PROC_CFG,
            VIDPROC_ENABLE | 1 << 18 | 1 << 24,
            u32::MAX,
            now,
        );
        v.write(VID_DESKTOP_OVERLAY_STRIDE, 10, u32::MAX, now);
        let on_buffer = v.take_mirror().unwrap();
        let scanout = on_buffer.scanout.as_ref().unwrap();
        assert_eq!(scanout.desktop.buffer, Some(0x10_0000 / 2));
        assert!(memory(&on_buffer).is_empty());
        v.write(VID_DESKTOP_START_ADDR, 0, u32::MAX, now);
        v.write(VID_PROC_CFG, VIDPROC_ENABLE | 1 << 18, u32::MAX, now);
        v.write(VID_DESKTOP_OVERLAY_STRIDE, 1280, u32::MAX, now);
        assert!(memory(&v.take_mirror().unwrap()).is_empty());
        // A recording wants the software picture too, which looks at the
        // change first: the renderer still gets the page.
        v.set_software_picture(true);
        v.prepare_display();
        v.write(LINEAR + 8, 0x0BAD_F00D, u32::MAX, now);
        assert!(v.prepare_display());
        let frame = v.take_mirror().unwrap();
        assert_eq!(memory(&frame)[0].1[8..12], [0x0D, 0xF0, 0xAD, 0x0B]);
        // A renderer starting over gets them all again.
        v.set_mirror(false);
        v.set_mirror(true);
        assert_eq!(memory(&v.take_mirror().unwrap()).first().map(|m| m.0), Some(0));
    }

    #[test]
    fn texture_base_addresses_are_bytes() {
        let mut v = card();
        reg3d(&mut v, TEX_BASE_ADDR, 0x0050_0010);
        let reg = v.reg.clone();
        v.tmu[0].prepare(&reg[..]);
        assert_eq!(v.tmu[0].lodoffset[0], 0x50_0010);
        // A download from 600000h lands from there on.
        v.write(0x60_0008, 0x1122_3344, 0xFFFF_FFFF, Now::default());
        assert_eq!(v.fbi.ram.get(0x50_0018 / 2), 0x3344);
    }

    #[test]
    fn a_texture_download_is_what_writes_through_the_texture_space_make() {
        // Rows of an odd length at an odd address: the bytes around them
        // stay as they were.
        let data: Vec<u8> = (0..37u8).map(|i| i.wrapping_mul(7) ^ 0x5A).collect();
        let (at, end) = (0x40_0003u32, 0x40_0003 + data.len() as u32);
        let (mut bulk, mut dwords) = (card(), card());
        for v in [&bulk, &dwords] {
            v.fbi.ram.fill(0x1F_FFF0..0x20_0040, 0xA5C3);
        }
        bulk.banshee_texture_download(at, &data);
        // The same bytes a dword at a time, merged into what is there.
        let port = at & !0xF;
        reg3d(&mut dwords, TEX_BASE_ADDR, port);
        for dword in (at & !3..end).step_by(4) {
            let bytes: [u8; 4] = std::array::from_fn(|i| {
                let a = dword + i as u32;
                if (at..end).contains(&a) {
                    data[(a - at) as usize]
                } else {
                    dwords.fbi.ram.byte(a as usize)
                }
            });
            let value = u32::from_le_bytes(bytes);
            dwords.write(0x60_0000 + dword - port, value, u32::MAX, Now::default());
        }
        let around = |v: &Voodoo| {
            (0x3F_FFE0..0x40_0080)
                .map(|a| v.fbi.ram.byte(a))
                .collect::<Vec<_>>()
        };
        assert_eq!(around(&bulk), around(&dwords));
        assert_eq!(bulk.fbi.ram.byte(at as usize - 1), 0xC3);
        assert_eq!(bulk.fbi.ram.byte(at as usize), data[0]);
    }
}
