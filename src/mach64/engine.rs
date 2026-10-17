//! The mach64's draw engine: rectangles, blits, lines and the 3D
//! engine's trapezoids, each carried out at once when the register that
//! starts it is written. An operation that takes its source from the
//! host waits for the data written to HOST_DATA, and only then is the
//! engine busy.

use super::Mach64;
use crate::savestate::{Reader, Result, State, Writer};

/// What a register write started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// A rectangle: a fill or a blit (DST_HEIGHT_WIDTH, DST_X_WIDTH,
    /// DST_WIDTH).
    Fill,
    /// A line, or a trapezoid with DRAW_TRAP (DST_BRES_LNTH).
    Line,
    /// A context loaded from the memory (CONTEXT_LOAD_CNTL).
    Context,
    /// The 3D RAGE II's engine defaults (DP_SET_GUI_ENGINE).
    SetGuiEngine,
}

#[derive(Clone, Debug)]
pub struct Engine {
    /// The 3D RAGE II's texture palette, RGB888.
    pub palette: Vec<u32>,
}

impl Default for Engine {
    fn default() -> Self {
        Self {
            palette: vec![0; 256],
        }
    }
}

impl Engine {
    /// Back to idle, the operation in progress dropped.
    pub fn reset(&mut self) {}

    /// Whether an operation waits for host data.
    pub fn busy(&self) -> bool {
        false
    }

    /// Start `op` with the registers of `card`, drawing into `vram`. True
    /// if it drew.
    pub fn start(&mut self, op: Op, card: &mut Mach64, vram: &mut [u8]) -> bool {
        let _ = (op, card, vram);
        false
    }

    /// `bits` of host data for the operation in progress.
    pub fn host_data(&mut self, data: u32, bits: u32, card: &mut Mach64, vram: &mut [u8]) -> bool {
        let _ = (data, bits, card, vram);
        false
    }

    pub fn describe(&self) -> serde_json::Value {
        serde_json::json!({ "busy": self.busy() })
    }
}

impl State for Engine {
    fn save(&self, w: &mut Writer) {
        self.palette.save(w);
    }

    fn load(&mut self, r: &mut Reader) -> Result<()> {
        self.palette.load(r)?;
        self.palette.resize(256, 0);
        Ok(())
    }
}
