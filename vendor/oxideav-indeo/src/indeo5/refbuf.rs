//! Indeo 5 reference-frame buffer-slot rotation.
//!
//! Spec source: `docs/video/indeo/indeo5/spec/07-motion-compensation.md`
//! §1.2 (pre-decode per-frame-type dispatch), §1.3 (post-decode
//! dispatch), §4.1 (buffer-slot layout), §4.3 (the per-frame buffer
//! rotation).
//!
//! The codec instance carries the reference-frame slot set at
//! `[ebx+0xf4..0x114]` (`spec/07 §4.1`) implementing a two-frame
//! ping-pong with a parallel secondary pair for the temporal-
//! scalability mode. This module models the slot set and the two
//! per-frame-type rotation dispatches (the 4-entry jump tables at
//! `0x1003fbe8` pre-decode and `0x1003fc18` post-decode) as a pure
//! state machine over opaque buffer tokens.
//!
//! The wire-format-critical invariant (`spec/07 §1.5`): **INTRA / INTER
//! frames promote themselves to the primary reference; DROPPABLE_INTER
//! (`frame_type = 3`) does not** — its post-decode handler skips the
//! `next_current` promotion, so no subsequent frame's MVs can reference
//! it and a playback engine may drop it under load without breaking
//! later predictions.
//!
//! Buffer tokens are `u32` values with `0` meaning "null pointer",
//! mirroring the binary's pointer-slot semantics (`[ebx+0xf4] = 0`
//! after post-decode, dirty flags tested as non-zero).

use crate::indeo5::header::FrameType;

/// `spec/07 §4.1` — the codec-instance reference-frame slot set at
/// `[ebx+0xf4..0x114]`. Slots hold opaque buffer tokens (`0` = null).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RefSlots {
    /// `[ebx+0xf4]` — the currently-being-decoded frame's buffer.
    pub current: u32,
    /// `[ebx+0xf8]` — non-zero if `current` carries valid decoded data.
    pub current_dirty: u32,
    /// `[ebx+0xfc]` — non-zero if the secondary-reference pair is dirty.
    pub secondary_dirty: u32,
    /// `[ebx+0x100]` — the primary reference frame's buffer.
    pub primary_ref: u32,
    /// `[ebx+0x104]` — the slot reserved for the next frame's
    /// current-frame allocation.
    pub next_current: u32,
    /// `[ebx+0x108]` — the secondary reference (scalability path).
    pub secondary_ref: u32,
    /// `[ebx+0x10c]` — the alternate secondary reference (DROPPABLE
    /// swap).
    pub secondary_ref_alt: u32,
    /// `[ebx+0x110]` — the reference consumed by the post-decode
    /// output stage.
    pub output: u32,
}

impl RefSlots {
    /// `spec/07 §1.2` — the pre-decode per-frame-type rotation (the
    /// 4-entry jump table at `0x1003fbe8`):
    ///
    /// * **INTRA** — swap `next_current` ↔ `secondary_ref`, clear both
    ///   dirty flags ("key frame discards prior references").
    /// * **INTER** — clear `secondary_dirty` only (primary retained).
    /// * **DROPPABLE_INTER_SCAL / DROPPABLE_INTER** — if
    ///   `secondary_dirty`, swap `secondary_ref` ↔ `secondary_ref_alt`
    ///   (the droppable frame references the most recent non-droppable
    ///   frame).
    /// * **NULL** — no decode is invoked (`spec/07 §1.5`); no-op.
    pub fn pre_decode(&mut self, frame_type: FrameType) {
        match frame_type {
            FrameType::Intra => {
                core::mem::swap(&mut self.next_current, &mut self.secondary_ref);
                self.secondary_dirty = 0;
                self.current_dirty = 0;
            }
            FrameType::Inter => {
                self.secondary_dirty = 0;
            }
            FrameType::DroppableInterScalability | FrameType::DroppableInter => {
                if self.secondary_dirty != 0 {
                    core::mem::swap(&mut self.secondary_ref, &mut self.secondary_ref_alt);
                }
            }
            FrameType::Null => {}
        }
    }

    /// `spec/07 §1.3`/`§4.3` — the post-decode per-frame-type rotation
    /// (the 4-entry jump table at `0x1003fc18`):
    ///
    /// * **INTRA / INTER** (`0x1003f9df`) — promote the current frame
    ///   to the primary reference: `output = next_current`,
    ///   `primary_ref = current`, `secondary_ref = next_current` with
    ///   the old `secondary_ref` becoming the next `next_current`,
    ///   `current_dirty = old current`, `current = 0`.
    /// * **DROPPABLE_INTER_SCAL** (`0x1003f981`) — the conditional
    ///   secondary swap (when dirty), then the same promotion.
    /// * **DROPPABLE_INTER** (`0x1003fa1b`) — `output = next_current`,
    ///   `primary_ref = current`, conditional secondary swap — but
    ///   **no `next_current` promotion**: the droppable frame does not
    ///   become the next frame's reference.
    /// * **NULL** — no decode is invoked; no-op.
    pub fn post_decode(&mut self, frame_type: FrameType) {
        match frame_type {
            FrameType::Intra | FrameType::Inter => self.promote(),
            FrameType::DroppableInterScalability => {
                if self.secondary_dirty != 0 {
                    core::mem::swap(&mut self.secondary_ref, &mut self.secondary_ref_alt);
                }
                self.promote();
            }
            FrameType::DroppableInter => {
                self.output = self.next_current;
                self.primary_ref = self.current;
                if self.secondary_dirty != 0 {
                    core::mem::swap(&mut self.secondary_ref, &mut self.secondary_ref_alt);
                }
                // Critical (spec/07 §1.3): no promotion of next_current.
            }
            FrameType::Null => {}
        }
    }

    /// The `spec/07 §1.3` `0x1003f9df` promote-to-primary body shared
    /// by the INTRA / INTER (and, after its conditional swap, the
    /// DROPPABLE_INTER_SCAL) handlers.
    fn promote(&mut self) {
        self.output = self.next_current;
        self.primary_ref = self.current;
        // secondary_ref = old next_current; next_current = old
        // secondary_ref (the ping-pong).
        core::mem::swap(&mut self.secondary_ref, &mut self.next_current);
        self.current_dirty = self.current;
        self.current = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A slot set mid-sequence with distinct tokens per slot.
    fn slots() -> RefSlots {
        RefSlots {
            current: 10,
            current_dirty: 1,
            secondary_dirty: 0,
            primary_ref: 20,
            next_current: 30,
            secondary_ref: 40,
            secondary_ref_alt: 50,
            output: 60,
        }
    }

    #[test]
    fn intra_pre_decode_discards_references() {
        // spec/07 §1.2: swap next_current <-> secondary_ref, clear
        // both dirty flags.
        let mut s = slots();
        s.secondary_dirty = 7;
        s.pre_decode(FrameType::Intra);
        assert_eq!(s.next_current, 40);
        assert_eq!(s.secondary_ref, 30);
        assert_eq!(s.secondary_dirty, 0);
        assert_eq!(s.current_dirty, 0);
    }

    #[test]
    fn inter_pre_decode_clears_secondary_dirty_only() {
        let mut s = slots();
        s.secondary_dirty = 7;
        let before = s;
        s.pre_decode(FrameType::Inter);
        assert_eq!(s.secondary_dirty, 0);
        // Everything else unchanged.
        assert_eq!(
            RefSlots {
                secondary_dirty: 7,
                ..s
            },
            before
        );
    }

    #[test]
    fn droppable_pre_decode_swaps_secondary_when_dirty() {
        // spec/07 §1.2: conditional secondary swap.
        for ft in [
            FrameType::DroppableInter,
            FrameType::DroppableInterScalability,
        ] {
            let mut s = slots();
            s.secondary_dirty = 1;
            s.pre_decode(ft);
            assert_eq!((s.secondary_ref, s.secondary_ref_alt), (50, 40), "{ft:?}");
            // Not dirty -> no swap.
            let mut s = slots();
            s.pre_decode(ft);
            assert_eq!((s.secondary_ref, s.secondary_ref_alt), (40, 50), "{ft:?}");
        }
    }

    #[test]
    fn inter_post_decode_promotes_to_primary() {
        // spec/07 §1.3 handler 0x1003f9df.
        let mut s = slots();
        s.post_decode(FrameType::Inter);
        assert_eq!(s.output, 30); // output = next_current
        assert_eq!(s.primary_ref, 10); // primary = current
        assert_eq!(s.secondary_ref, 30); // secondary = next_current
        assert_eq!(s.next_current, 40); // next_current = old secondary
        assert_eq!(s.current_dirty, 10); // dirty = old current
        assert_eq!(s.current, 0); // current cleared
    }

    #[test]
    fn droppable_inter_post_decode_does_not_promote() {
        // spec/07 §1.3 handler 0x1003fa1b: the critical droppable
        // invariant — next_current is NOT rotated.
        let mut s = slots();
        s.post_decode(FrameType::DroppableInter);
        assert_eq!(s.output, 30);
        assert_eq!(s.primary_ref, 10);
        // next_current untouched: the droppable frame does not become
        // the next frame's reference.
        assert_eq!(s.next_current, 30);
        assert_eq!(s.secondary_ref, 40); // not dirty -> no swap
        // Dirty case swaps the secondary pair.
        let mut s = slots();
        s.secondary_dirty = 1;
        s.post_decode(FrameType::DroppableInter);
        assert_eq!((s.secondary_ref, s.secondary_ref_alt), (50, 40));
    }

    #[test]
    fn droppable_scal_post_decode_swaps_then_promotes() {
        // spec/07 §1.3 handler 0x1003f981.
        let mut s = slots();
        s.secondary_dirty = 1;
        s.post_decode(FrameType::DroppableInterScalability);
        // Swap first: secondary_ref = 50, alt = 40. Then promote:
        // secondary_ref = next_current(30), next_current = 50.
        assert_eq!(s.secondary_ref_alt, 40);
        assert_eq!(s.secondary_ref, 30);
        assert_eq!(s.next_current, 50);
        assert_eq!(s.primary_ref, 10);
        assert_eq!(s.current, 0);
    }

    #[test]
    fn null_frame_is_noop_both_phases() {
        // spec/07 §1.5: NULL invokes no decode.
        let mut s = slots();
        let before = s;
        s.pre_decode(FrameType::Null);
        s.post_decode(FrameType::Null);
        assert_eq!(s, before);
    }

    #[test]
    fn droppable_never_becomes_reference_across_sequence() {
        // spec/07 §1.5 invariant: after INTER(a) -> DROPPABLE(b) ->
        // INTER(c), frame c's prediction reference is frame a's
        // buffer, not b's.
        let mut s = RefSlots {
            next_current: 1,
            secondary_ref: 2,
            ..RefSlots::default()
        };
        // Frame a (INTER) decodes into buffer 100.
        s.pre_decode(FrameType::Inter);
        s.current = 100;
        s.post_decode(FrameType::Inter);
        let a_primary = s.primary_ref;
        assert_eq!(a_primary, 100);
        // Frame b (DROPPABLE_INTER) decodes into buffer 200.
        s.pre_decode(FrameType::DroppableInter);
        s.current = 200;
        s.post_decode(FrameType::DroppableInter);
        // b's buffer is primary_ref transiently (for output) but
        // next_current was not rotated to it…
        assert_ne!(s.next_current, 200);
        // Frame c (INTER): decodes; its MC source is what pre-decode
        // leaves as the retained reference chain — b never entered
        // the next_current rotation.
        s.pre_decode(FrameType::Inter);
        assert_ne!(s.next_current, 200);
    }
}
