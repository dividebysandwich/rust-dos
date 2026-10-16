//! The two eyes' pictures, taken from what the screen shows as the VFX1
//! would show them.
//!
//! Most VFX1 games use its multiple-buffer mode: the 640x480 frame holds
//! four 320x240 pages, the left eye's in its left half and the right's in
//! its right, each on the even lines and again on the odd ones. The game
//! draws the parity not shown, then has the headset show it. Not every game
//! tells the card which (Descent never writes the page select register),
//! so the page shown is found the way forte-vfx1-vr does it
//! (src/vfxhost/vfxhost.c, FINDINGS Q3): the lines that changed are the
//! page being drawn, and it is done once drawing moves to the other lines.

use super::{Activity, Vfx1Stereo};
use crate::video::Frame;

/// Every how many pixels of a line are compared between frames.
const STEP: usize = 8;

/// How long after the game last wrote the page select register or read
/// the head tracker it counts as showing stereo, in seconds.
const ACTIVE_FOR: f64 = 0.6;

/// How the screen is split into the eyes' pictures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Mono,
    Pages,
    Lines,
    Side,
}

/// The pictures for the eyes, and how they were split.
pub struct Eyes {
    pub left: Frame,
    pub right: Frame,
    pub layout: Layout,
}

impl Default for Eyes {
    fn default() -> Self {
        Self {
            left: Frame::new(0, 0),
            right: Frame::new(0, 0),
            layout: Layout::Mono,
        }
    }
}

/// Splits the screen's pictures into the eyes'.
#[derive(Default)]
pub struct Splitter {
    flip: PageFlip,
    /// When the game last told the headset something, by the host's clock
    /// in seconds, and the counts it was seen at.
    active_at: Option<f64>,
    seen: (u32, u32),
    /// The screen at its own size (the renderer doubles small modes).
    native: Vec<u8>,
    pub eyes: Eyes,
}

impl Splitter {
    /// The layout `stereo` asks for: for `auto`, the driver's video mode,
    /// or lines while the 3D BIOS has its glasses on, or pages on a
    /// 640x480 screen while the game uses the headset, or else none. `now`
    /// is the host's clock in seconds.
    pub fn layout(
        &mut self,
        stereo: Vfx1Stereo,
        activity: Activity,
        size: (u32, u32),
        now: f64,
    ) -> Layout {
        let counts = (activity.page_writes, activity.polls);
        if counts != self.seen {
            self.seen = counts;
            self.active_at = Some(now);
        }
        match stereo {
            Vfx1Stereo::Pages => Layout::Pages,
            Vfx1Stereo::Lines => Layout::Lines,
            Vfx1Stereo::Side => Layout::Side,
            Vfx1Stereo::Mono => Layout::Mono,
            Vfx1Stereo::Auto => match activity.video_mode {
                1 => Layout::Pages,
                2 => Layout::Lines,
                _ if activity.glasses => Layout::Lines,
                _ if size == (640, 480)
                    && self.active_at.is_some_and(|at| now - at < ACTIVE_FOR) =>
                {
                    Layout::Pages
                }
                _ => Layout::Mono,
            },
        }
    }

    /// The screen's size at its own resolution.
    pub fn native_size(frame: &Frame) -> (u32, u32) {
        if frame.doubled {
            (frame.width / 2, frame.height / 2)
        } else {
            (frame.width, frame.height)
        }
    }

    /// Take the eyes' pictures from `frame` as `layout` has them, the
    /// left one the right's with `swap`.
    pub fn feed(&mut self, frame: &Frame, layout: Layout, swap: bool) {
        let (w, h) = Self::native_size(frame);
        let (w, h) = (w as usize, h as usize);
        let rgb: &[u8] = if frame.doubled {
            self.native.resize(w * h * 3, 0);
            let row = frame.width as usize * 3;
            for y in 0..h {
                for x in 0..w {
                    let from = 2 * y * row + 2 * x * 3;
                    self.native[(y * w + x) * 3..][..3].copy_from_slice(&frame.rgb[from..from + 3]);
                }
            }
            &self.native
        } else {
            &frame.rgb
        };
        if layout != Layout::Pages {
            self.flip = PageFlip::default();
        }
        let eyes = &mut self.eyes;
        eyes.layout = layout;
        match layout {
            Layout::Mono => {
                copy_area(rgb, w, (0, 0), (w, h), 1, &mut eyes.left);
                copy_area(rgb, w, (0, 0), (w, h), 1, &mut eyes.right);
            }
            Layout::Side => {
                copy_area(rgb, w, (0, 0), (w / 2, h), 1, &mut eyes.left);
                copy_area(rgb, w, (w / 2, 0), (w / 2, h), 1, &mut eyes.right);
            }
            Layout::Lines => {
                copy_area(rgb, w, (0, 0), (w, h / 2), 2, &mut eyes.left);
                copy_area(rgb, w, (0, 1), (w, h / 2), 2, &mut eyes.right);
            }
            Layout::Pages => {
                // The page shown is copied when it is done, as the frame
                // that finishes it doesn't touch it.
                if let Some(parity) = self.flip.feed(rgb, w, h) {
                    copy_area(rgb, w, (0, parity), (w / 2, h / 2), 2, &mut eyes.left);
                    copy_area(rgb, w, (w / 2, parity), (w / 2, h / 2), 2, &mut eyes.right);
                }
            }
        }
        if swap {
            std::mem::swap(&mut eyes.left, &mut eyes.right);
        }
    }
}

/// `size` pixels of the `w`-wide picture `rgb` from `at`, every
/// `line_step`th line, into `out`.
fn copy_area(
    rgb: &[u8],
    w: usize,
    at: (usize, usize),
    size: (usize, usize),
    line_step: usize,
    out: &mut Frame,
) {
    out.resize(size.0 as u32, size.1 as u32);
    out.doubled = false;
    let row = size.0 * 3;
    for y in 0..size.1 {
        let from = ((at.1 + y * line_step) * w + at.0) * 3;
        if from + row <= rgb.len() {
            out.rgb[y * row..][..row].copy_from_slice(&rgb[from..from + row]);
        }
    }
}

/// Finds the page the game has finished drawing from the lines that
/// change between frames.
#[derive(Default)]
struct PageFlip {
    prev: Vec<u8>,
    /// The parity being drawn, and which of its halves were (bit 0 the
    /// left, bit 1 the right).
    pending: Option<usize>,
    pending_halves: u8,
}

impl PageFlip {
    /// A new frame `w` x `h`: the parity of the lines of a page done in
    /// it, if one is (on the first frame, the even lines).
    fn feed(&mut self, rgb: &[u8], w: usize, h: usize) -> Option<usize> {
        if self.prev.len() != rgb.len() {
            self.prev = rgb.to_vec();
            self.pending = None;
            return Some(0);
        }
        let mut changed = changed_per_parity(rgb, &self.prev, w, h);
        // A mouse cursor isn't a page being drawn: System Shock draws its
        // cursor into the lines it doesn't show, about 0.2% of them.
        let small = (h / 2) * w.div_ceil(STEP) / 200;
        for c in &mut changed {
            if *c <= small {
                *c = 0;
            }
        }
        let mut drawing = match changed {
            [0, 0] => None,
            [even, odd] => Some(usize::from(odd > even)),
        };
        let mut halves = 0;
        // A new palette changes both parities alike and isn't drawing:
        // Descent 2 sets one every frame, between drawing its pages.
        if changed[0] > 0 && changed[1] > 0 && palette_change(rgb, &self.prev, w, h) {
            drawing = self.pending;
        } else if let Some(parity) = drawing {
            halves = changed_halves(rgb, &self.prev, w, h, parity);
        }
        self.prev.copy_from_slice(rgb);

        // A pause finishes the page only once both eyes are in it: Descent
        // draws the left eye, waits a frame or two, then the right. Drawing
        // moving to the other parity always finishes it.
        let done = self
            .pending
            .filter(|_| drawing != self.pending && (drawing.is_some() || self.pending_halves == 3));
        match drawing {
            Some(parity) => {
                if drawing != self.pending {
                    self.pending_halves = 0;
                }
                self.pending = Some(parity);
                self.pending_halves |= halves;
            }
            None if done.is_some() => self.pending = None,
            None => {}
        }
        done
    }
}

fn differs(a: &[u8], b: &[u8], i: usize) -> bool {
    a[i..i + 3] != b[i..i + 3]
}

/// How many of the pixels compared changed on the even lines and on the
/// odd ones.
fn changed_per_parity(cur: &[u8], prev: &[u8], w: usize, h: usize) -> [usize; 2] {
    let mut changed = [0; 2];
    for y in 0..h {
        for x in (0..w).step_by(STEP) {
            if differs(cur, prev, (y * w + x) * 3) {
                changed[y & 1] += 1;
            }
        }
    }
    changed
}

/// Which halves of a parity's lines changed: bit 0 the left, bit 1 the
/// right.
fn changed_halves(cur: &[u8], prev: &[u8], w: usize, h: usize, parity: usize) -> u8 {
    let mut halves = 0;
    for y in (parity..h).step_by(2) {
        for x in (0..w).step_by(STEP) {
            if differs(cur, prev, (y * w + x) * 3) {
                halves |= if x < w / 2 { 1 } else { 2 };
            }
        }
        if halves == 3 {
            break;
        }
    }
    halves
}

/// Whether the changes are a new palette: the pixels of each old colour
/// all turned one new colour (at least 95% of them), as no drawing does.
fn palette_change(cur: &[u8], prev: &[u8], w: usize, h: usize) -> bool {
    let colour = |rgb: &[u8], i: usize| u32::from_be_bytes([0, rgb[i], rgb[i + 1], rgb[i + 2]]);
    let mut map = std::collections::HashMap::new();
    let (mut changed, mut conflicts) = (0u32, 0u32);
    for y in 0..h {
        for x in (0..w).step_by(STEP) {
            let i = (y * w + x) * 3;
            let (from, to) = (colour(prev, i), colour(cur, i));
            if from == to {
                continue;
            }
            changed += 1;
            if *map.entry(from).or_insert(to) != to {
                conflicts += 1;
            }
            // Not a 256-colour picture.
            if map.len() > 2048 {
                return false;
            }
        }
    }
    changed > 0 && conflicts * 20 < changed
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 640;
    const H: usize = 480;

    /// A 640x480 frame with each page (half, parity) filled with a shade
    /// from `shades`: left even, right even, left odd, right odd.
    fn frame(shades: [u8; 4]) -> Frame {
        let mut rgb = vec![0; W * H * 3];
        for y in 0..H {
            for x in 0..W {
                let shade = shades[(y & 1) * 2 + usize::from(x >= W / 2)];
                // Some texture, so a page drawn over is a change.
                let v = shade.wrapping_add((x / 7) as u8);
                rgb[(y * W + x) * 3..][..3].copy_from_slice(&[v, v / 2, shade]);
            }
        }
        Frame::from_rgb(W as u32, H as u32, rgb)
    }

    fn shade(f: &Frame) -> u8 {
        f.rgb[2]
    }

    #[test]
    fn pages_follow_the_drawing() {
        let mut s = Splitter::default();
        // The first frame shows the even lines.
        s.feed(&frame([10, 20, 30, 40]), Layout::Pages, false);
        assert_eq!((shade(&s.eyes.left), shade(&s.eyes.right)), (10, 20));
        assert_eq!((s.eyes.left.width, s.eyes.left.height), (320, 240));
        // The odd lines drawn, left eye then a pause then the right: not
        // done until drawing moves to the even lines again.
        s.feed(&frame([10, 20, 31, 40]), Layout::Pages, false);
        s.feed(&frame([10, 20, 31, 40]), Layout::Pages, false);
        s.feed(&frame([10, 20, 31, 41]), Layout::Pages, false);
        assert_eq!((shade(&s.eyes.left), shade(&s.eyes.right)), (10, 20));
        // Both eyes drawn and a pause: done.
        s.feed(&frame([10, 20, 31, 41]), Layout::Pages, false);
        assert_eq!((shade(&s.eyes.left), shade(&s.eyes.right)), (31, 41));
        // Drawing the even lines again, then moving to the odd: done.
        s.feed(&frame([12, 20, 31, 41]), Layout::Pages, false);
        s.feed(&frame([12, 22, 31, 41]), Layout::Pages, false);
        s.feed(&frame([12, 22, 33, 41]), Layout::Pages, true);
        assert_eq!((shade(&s.eyes.left), shade(&s.eyes.right)), (22, 12));
    }

    #[test]
    fn a_new_palette_is_not_a_page() {
        let mut s = Splitter::default();
        s.feed(&frame([10, 20, 30, 40]), Layout::Pages, false);
        s.feed(&frame([10, 20, 31, 41]), Layout::Pages, false);
        // Every colour changes to another, both parities alike.
        s.feed(&frame([110, 120, 131, 141]), Layout::Pages, false);
        assert_eq!(shade(&s.eyes.left), 10);
        assert_eq!(s.flip.pending, Some(1));
    }

    #[test]
    fn lines_and_halves() {
        let mut s = Splitter::default();
        s.feed(&frame([10, 20, 30, 40]), Layout::Lines, false);
        assert_eq!((shade(&s.eyes.left), shade(&s.eyes.right)), (10, 30));
        assert_eq!((s.eyes.left.width, s.eyes.left.height), (640, 240));
        s.feed(&frame([10, 20, 30, 40]), Layout::Side, false);
        assert_eq!((shade(&s.eyes.left), shade(&s.eyes.right)), (10, 20));
        assert_eq!((s.eyes.right.width, s.eyes.right.height), (320, 480));
    }

    #[test]
    fn doubled_frames_are_split_at_their_own_size() {
        let mut doubled = Frame::new(640, 400);
        doubled.doubled = true;
        for y in 0..400 {
            let v = if (y / 2) % 2 == 0 { 50 } else { 60 };
            for x in 0..640 {
                doubled.rgb[(y * 640 + x) * 3 + 2] = v;
            }
        }
        let mut s = Splitter::default();
        s.feed(&doubled, Layout::Lines, false);
        assert_eq!((s.eyes.left.width, s.eyes.left.height), (320, 100));
        assert_eq!((shade(&s.eyes.left), shade(&s.eyes.right)), (50, 60));
    }

    #[test]
    fn auto_layout() {
        let mut s = Splitter::default();
        let quiet = Activity::default();
        assert_eq!(
            s.layout(Vfx1Stereo::Auto, quiet, (640, 480), 0.0),
            Layout::Mono
        );
        let polled = Activity { polls: 1, ..quiet };
        assert_eq!(
            s.layout(Vfx1Stereo::Auto, polled, (640, 480), 1.0),
            Layout::Pages
        );
        assert_eq!(
            s.layout(Vfx1Stereo::Auto, polled, (320, 200), 1.1),
            Layout::Mono
        );
        assert_eq!(
            s.layout(Vfx1Stereo::Auto, polled, (640, 480), 2.0),
            Layout::Mono
        );
        let glasses = Activity {
            glasses: true,
            ..quiet
        };
        assert_eq!(
            s.layout(Vfx1Stereo::Auto, glasses, (800, 600), 2.5),
            Layout::Lines
        );
        let lines = Activity {
            video_mode: 2,
            ..polled
        };
        assert_eq!(
            s.layout(Vfx1Stereo::Auto, lines, (640, 400), 3.0),
            Layout::Lines
        );
        assert_eq!(
            s.layout(Vfx1Stereo::Side, quiet, (640, 400), 3.0),
            Layout::Side
        );
    }
}
