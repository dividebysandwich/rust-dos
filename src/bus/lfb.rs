//! Where translated code may read and write the VESA linear frame buffer
//! by itself (`dynrec`'s `card_state`): video memory through the BAR of
//! a ToadForce, an S3, a mach64 or the VBE's own, where a program draws
//! into a locked surface a pixel at a time.
//!
//! A write there through the bus (`Bus::write_lfb`) is the bytes and
//! what the bus does around them: the ToadForce's queued commands run
//! first, a surface its host renderer drew into notes the bytes as card
//! memory's (`toadforce_writing`), the write counts for `cycles=auto`
//! (`Activity::video_write`) and for the host's copies of the page
//! (`toadforce_wrote`), and the rows of the picture it lands in are
//! drawn again. `Bus::lfb_direct` says where the code can do the same
//! by itself: no commands queued, no observer, and the bytes of the
//! picture shown left to the bus, which marks their rows.

use super::Bus;
use crate::toadforce::gpu::Note;
use crate::video::VideoMode;

/// The most surfaces ahead on a ToadForce's host renderer whose
/// written bytes translated code notes itself. With more, the code
/// leaves card memory to the bus.
pub const NOTES: usize = 4;

/// Where translated code may read and write the VESA linear frame
/// buffer by itself (`Bus::lfb_direct`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LfbDirect {
    /// Video memory's first byte on the host.
    pub base: *mut u8,
    /// The frame buffer's physical address and length: all of video
    /// memory.
    pub lo: u32,
    pub len: u32,
    /// The bytes of video memory (start, end) of the picture shown,
    /// whose rows a write marks for drawing: the bus's to write.
    pub shown: (u32, u32),
    /// Those of the picture `cycles=auto` counts the bytes of
    /// (`Activity::picture`), the picture as it was at the last
    /// retrace: after a flip, the buffer a game draws its next frame
    /// into.
    pub counted: (u32, u32),
    /// A ToadForce's counts of the writes to each 4 KB page, for its
    /// host renderer's copies: the generation and the first page's
    /// count (`gpu::Pages::counters`). Null without a host.
    pub generation: *mut u64,
    pub counts: *mut u64,
    /// The surfaces ahead on the host (`gpu::Gpu::notes`), the first
    /// `ahead` of them. Their bytes are read through the bus, which
    /// answers with the host's pixels.
    pub notes: [Note; NOTES],
    pub ahead: usize,
}

const NO_NOTE: Note = Note {
    start: 0,
    end: 0,
    bits: std::ptr::null_mut(),
    any: std::ptr::null_mut(),
};

/// Clip a range of video memory's offsets to `len` bytes.
fn clip((start, end): (u64, u64), len: u32) -> (u32, u32) {
    let end = end.min(len as u64) as u32;
    (start.min(end as u64) as u32, end)
}

impl Bus {
    /// Where translated code may read and write the VESA linear frame
    /// buffer by itself, and what it must do for a write (`LfbDirect`).
    /// None where an access there does more, or reaches something else:
    /// commands queued on a ToadForce, a proof of an idle loop that
    /// follows every access (`idle`), the Vérité's byte swapping, a
    /// picture `cycles=auto` doesn't know the bytes of, or registers of
    /// an S3 or a mach64 inside the frame buffer, which the bus finds
    /// before it.
    #[cfg_attr(not(dynrec), allow(dead_code))]
    pub(crate) fn lfb_direct(&mut self) -> Option<LfbDirect> {
        let lo = self.vbe.lfb_base?;
        let len = u32::try_from(self.vbe.vram.len()).ok()?;
        let end = lo as u64 + len as u64;
        if (lo as usize) < self.ram.len()
            || end > 0xFFFE_0000
            || self.observe.on
            || self.vbe.host_xor != 0
            || self.toadforce_queued()
        {
            return None;
        }
        if self.s3()
            && let Some(base) = self.s3_lfb_base()
        {
            // The registers in the 64 KB from 16 MB above the frame
            // buffer's base.
            let mmio = base as u64 + 0x100_0000;
            if mmio < end && (lo as u64) < mmio + 0x1_0000 {
                return None;
            }
        }
        if self.mach64()
            && let Some(aperture) = self.mach64.aperture()
        {
            // The registers at the end of each half of the aperture.
            let registers = aperture as u64 + crate::mach64::REGISTERS as u64;
            if registers < end && (lo as u64) < aperture as u64 + crate::mach64::APERTURE as u64 {
                return None;
            }
        }
        // Each write counts for `cycles=auto`: where the picture isn't
        // known, all of it is the picture's.
        let counted = self.activity.picture()?;
        let shown = match self.video_mode {
            VideoMode::Vesa => self.vbe.picture().map_or((0, 0), |p| clip(p, len)),
            _ => (0, 0),
        };
        let mut direct = LfbDirect {
            base: self.vbe.vram.as_mut_ptr(),
            lo,
            len,
            shown,
            counted: clip(counted, len),
            generation: std::ptr::null_mut(),
            counts: std::ptr::null_mut(),
            notes: [NO_NOTE; NOTES],
            ahead: 0,
        };
        if let Some(card) = self.toadforce.as_deref_mut() {
            let (counters, notes) = card.direct();
            if let Some((generation, counts)) = counters {
                (direct.generation, direct.counts) = (generation, counts);
            }
            for note in notes {
                if direct.ahead == NOTES {
                    return None;
                }
                direct.notes[direct.ahead] = note;
                direct.ahead += 1;
            }
        }
        Some(direct)
    }
}
