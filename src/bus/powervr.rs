//! The PowerVR card on the bus: its two windows where BAR0 and BAR1 put
//! them, its interrupt line, and putting it in or taking it out.

use super::Bus;
use crate::powervr::{Chip, PowerVr, Window};

/// The 4x4 ordered dither the card adds before dropping bits, 0-7.
const DITHER: [[u32; 4]; 4] = [[0, 4, 1, 5], [6, 2, 7, 3], [1, 5, 0, 4], [7, 3, 6, 2]];

impl Bus {
    /// Put in the PowerVR card `chip` (None: take it out). A card already
    /// there of the same kind stays as it is.
    pub fn configure_powervr(&mut self, chip: Option<Chip>) {
        if self.powervr.as_ref().map(|p| p.chip) == chip {
            return;
        }
        self.powervr = chip.map(PowerVr::new);
        self.powervr_log();
        self.sync_powervr_irq();
    }

    /// A PCI reset of the PowerVR card, as at power-on.
    pub fn reset_powervr(&mut self) {
        if let Some(p) = &mut self.powervr {
            p.reset();
        }
        self.sync_powervr_irq();
    }

    /// The window of the card `addr` is in, and the offset there. The
    /// windows count where they are above the RAM and below the BIOS
    /// ROM's mirror at the top.
    #[inline]
    pub fn powervr_at(&self, addr: usize) -> Option<(Window, u32)> {
        let p = self.powervr.as_ref()?;
        if addr < self.ram.len() || addr >= 0xFFFE_0000 {
            return None;
        }
        p.window(addr as u32)
    }

    /// Read `len` bytes (1, 2 or 4) of a window, not crossing a dword.
    pub(crate) fn powervr_read(&self, (window, offset): (Window, u32), len: u32) -> u32 {
        let Some(p) = &self.powervr else { return u32::MAX };
        match window {
            Window::Textures => p.read_texture(offset, len),
            Window::Registers => {
                let dword = p.read_register(offset & !3);
                let value = dword >> (8 * (offset & 3));
                if len == 4 { value } else { value & ((1 << (8 * len)) - 1) }
            }
        }
    }

    /// Write `len` bytes (1, 2 or 4) of a window, not crossing a dword.
    pub(crate) fn powervr_write(&mut self, (window, offset): (Window, u32), value: u32, len: u32) {
        let Some(p) = &mut self.powervr else { return };
        match window {
            Window::Textures => p.write_texture(offset, value, len),
            Window::Registers => {
                let shift = 8 * (offset & 3);
                let mask = if len == 4 { u32::MAX } else { ((1 << (8 * len)) - 1) << shift };
                if p.write_register(offset & !3, value << shift, mask) {
                    if p.take_start() {
                        self.powervr_render();
                    }
                    self.sync_powervr_irq();
                }
            }
        }
        self.powervr_log();
    }

    /// Carry out the render the card started, writing its pixels to the
    /// frame buffer.
    fn powervr_render(&mut self) {
        let Some(p) = &self.powervr else { return };
        if let Some((dir, n)) = p.snapshot_wanted() {
            self.powervr_snapshot(&dir, n);
        }
        let Some(p) = &self.powervr else { return };
        let rendered = p.render(&self.ram);
        let out = p.output();
        let bytes = out.format.bytes();
        let mut row = Vec::new();
        for tile in &rendered.tiles {
            for y in 0..tile.height {
                let line = tile.y + y;
                let first = tile.x.max(out.columns.start);
                let end = (tile.x + tile.width).min(out.columns.end);
                if first >= end {
                    continue;
                }
                row.clear();
                for x in first..end {
                    let rgb = tile.pixels[(y * tile.width + x - tile.x) as usize];
                    let bias = if out.dither { DITHER[(line & 3) as usize][(x & 3) as usize] } else { 0 };
                    let packed = out.format.pack(rgb, bias);
                    row.extend_from_slice(&packed.to_le_bytes()[..bytes as usize]);
                }
                let at = out.address.wrapping_add(line * out.stride).wrapping_add(first * bytes);
                self.powervr_store(at as usize, &row);
            }
        }
        if let Some(p) = &mut self.powervr {
            p.finish_render();
        }
    }

    /// Bytes the card writes to the machine's memory, by bus mastering:
    /// most often the VGA's linear frame buffer.
    fn powervr_store(&mut self, addr: usize, bytes: &[u8]) {
        if let Some(offset) = self.vbe.lfb_offset(addr, bytes.len()) {
            self.write_vram(offset, bytes);
        } else {
            for (i, &b) in bytes.iter().enumerate() {
                self.write_8(addr + i, b);
            }
        }
    }

    /// For the trace: the card's registers and texture memory and the
    /// machine's RAM as a render starts, as `render-N.regs`, `.tex` and
    /// `.ram` in the trace folder.
    #[cold]
    fn powervr_snapshot(&mut self, dir: &std::path::Path, n: u32) {
        let Some(p) = &self.powervr else { return };
        let regs: Vec<u8> = p.registers().iter().flat_map(|r| r.to_le_bytes()).collect();
        let files: [(&str, &[u8]); 3] = [("regs", &regs), ("tex", p.textures()), ("ram", &self.ram)];
        let failed = files.iter().find_map(|(ext, bytes)| {
            let path = dir.join(format!("render-{}.{}", n, ext));
            std::fs::write(&path, bytes).err().map(|e| format!("[PVR] Can't write {}: {}", path.display(), e))
        });
        p.trace_note(&format!("-- snapshot render-{}", n));
        if let Some(line) = failed {
            self.log_string(&line);
        }
    }

    /// Drive the card's interrupt line: raise the request when it goes
    /// up, withdraw it when it drops.
    fn sync_powervr_irq(&mut self) {
        let line = self.powervr.as_ref().filter(|p| p.irq_asserted()).and_then(|p| p.irq());
        if let Some(old) = self.powervr_line
            && line != Some(old)
        {
            self.pic.lower(old);
        }
        if let Some(irq) = line
            && self.powervr_line != line
        {
            self.pic.raise(irq);
        }
        self.powervr_line = line;
        self.refresh_irq();
    }

    fn powervr_log(&mut self) {
        let lines = self.powervr.as_mut().map(|p| std::mem::take(&mut p.log)).unwrap_or_default();
        for line in lines {
            self.log_string(&line);
        }
    }

    /// For the debugger's status: what the PowerVR card is doing.
    pub fn powervr_status(&self) -> Option<serde_json::Value> {
        self.powervr.as_ref().map(|p| p.describe())
    }
}
