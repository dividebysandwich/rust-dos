//! The VFX1 from the host's side, at each of its frames: the head and the
//! puck to the card, and the eyes' pictures from the screen while the
//! headset in the 3D scene is worn.
//!
//! Two variables help trying it without a headset:
//! `RUST_DOS_VFX1_HEAD=yaw,pitch,roll` (degrees) is the head wherever it
//! is, and `RUST_DOS_VFX1_DUMP=<folder>` writes the eyes' pictures there
//! as `left.png` and `right.png` once a second, worn or not.

use super::stereo::{Eyes, Layout, Splitter};
use super::{Tracking, Vfx1Settings, pose};
use crate::bus::Bus;
use crate::video::Frame;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How often the eyes' pictures are written for `RUST_DOS_VFX1_DUMP`.
const DUMP_EVERY: Duration = Duration::from_secs(1);

pub struct Host {
    splitter: Splitter,
    started: Instant,
    head: Option<[i16; 3]>,
    dump: Option<(PathBuf, Option<Instant>)>,
}

impl Default for Host {
    fn default() -> Self {
        let head = std::env::var("RUST_DOS_VFX1_HEAD")
            .ok()
            .and_then(|text| parse_head(&text));
        let dump = std::env::var_os("RUST_DOS_VFX1_DUMP").map(|dir| (PathBuf::from(dir), None));
        Self {
            splitter: Splitter::default(),
            started: Instant::now(),
            head,
            dump,
        }
    }
}

/// `yaw,pitch,roll` in degrees, as the VFX1's units.
fn parse_head(text: &str) -> Option<[i16; 3]> {
    let degrees: Vec<f64> = text
        .split(',')
        .map(|v| v.trim().parse().ok())
        .collect::<Option<_>>()?;
    let [yaw, pitch, roll] = degrees[..] else {
        return None;
    };
    Some([yaw, pitch, roll].map(|d| pose::units(d.to_radians())))
}

impl Host {
    /// The head and the puck to the card, if there is one: as the headset
    /// `worn` has them, or else still. `frozen` (paused, or deterministic)
    /// keeps them still too.
    pub fn track(
        &self,
        bus: &mut Bus,
        worn: Option<Tracking>,
        settings: &Vfx1Settings,
        frozen: bool,
    ) {
        let Some(vfx1) = &mut bus.vfx1 else { return };
        let mut tracking = worn.filter(|_| !frozen).unwrap_or_default();
        if let Some(head) = self.head {
            tracking.head = head;
        }
        vfx1.set_tracking(tracking, settings.steer);
    }

    /// The eyes' pictures of `frame`, the screen now, while the headset is
    /// `worn` (and written out for `RUST_DOS_VFX1_DUMP`).
    pub fn eyes(
        &mut self,
        bus: &Bus,
        frame: &Frame,
        settings: &Vfx1Settings,
        worn: bool,
    ) -> Option<&Eyes> {
        let vfx1 = bus.vfx1.as_ref()?;
        if !worn && self.dump.is_none() {
            return None;
        }
        let now = self.started.elapsed().as_secs_f64();
        let size = Splitter::native_size(frame);
        let layout = self
            .splitter
            .layout(settings.stereo, vfx1.activity(), size, now);
        self.splitter.feed(frame, layout, settings.swap_eyes);
        if let Some((dir, last)) = &mut self.dump
            && last.is_none_or(|at| at.elapsed() >= DUMP_EVERY)
        {
            *last = Some(Instant::now());
            let eyes = &self.splitter.eyes;
            for (name, picture) in [("left.png", &eyes.left), ("right.png", &eyes.right)] {
                if let Ok(png) = crate::capture::png::encode(picture) {
                    let _ = std::fs::write(dir.join(name), png);
                }
            }
            let layout = match eyes.layout {
                Layout::Mono => "mono",
                Layout::Pages => "pages",
                Layout::Lines => "lines",
                Layout::Side => "side",
            };
            let _ = std::fs::write(dir.join("layout.txt"), format!("{}\n", layout));
        }
        worn.then_some(&self.splitter.eyes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heads_are_read_in_degrees() {
        assert_eq!(parse_head("90, -45,0"), Some([16384, -8192, 0]));
        assert_eq!(parse_head("90,45"), None);
        assert_eq!(parse_head("left"), None);
    }
}
