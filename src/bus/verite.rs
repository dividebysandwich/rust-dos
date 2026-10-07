//! The Rendition Vérité on the bus: its ports, the DMA that feeds its
//! FIFO from lists in the PC's memory, and its BIOS's own functions.

use super::Bus;
use crate::verite::{regs, swap};

/// DMA lists followed at most, against loops.
const DMA_ENTRIES: usize = 1 << 16;

impl Bus {
    /// Whether the display adapter is the Vérité.
    pub fn verite(&self) -> bool {
        self.vga.adapter.is_verite()
    }

    /// The Vérité's register at `port`, if it is one.
    pub(crate) fn verite_port(&self, port: u16) -> Option<u8> {
        if self.verite() { self.verite.port(port) } else { None }
    }

    pub(crate) fn verite_read(&mut self, reg: u8, len: u8) -> u32 {
        let value = self.verite.read(reg, len);
        self.verite_log();
        value
    }

    pub(crate) fn verite_write(&mut self, reg: u8, value: u32, len: u8) {
        self.verite.write(reg, value, len);
        self.verite_execute();
        // The DMA list pointer's last byte starts the DMA.
        if reg <= regs::DMACMDPTR + 3 && reg + len > regs::DMACMDPTR + 3 {
            self.verite_dma();
        }
        self.verite_log();
    }

    /// Follow the DMA list the pointer register points to, feeding the
    /// FIFO with the blocks it names.
    fn verite_dma(&mut self) {
        let pointer = self.verite.read_quiet(regs::DMACMDPTR) & !3;
        if self.verite.read_quiet(regs::MODE) & 0x08 == 0 || pointer == 0 {
            return;
        }
        let mut at = pointer;
        for _ in 0..DMA_ENTRIES {
            let (buffer, length) = (self.read_32(at as usize), self.read_32(at as usize + 4));
            if buffer == 0 {
                break;
            }
            if length & 0x8000_0000 != 0 {
                self.verite.trace(|| format!("DMA link {:08X}", buffer));
                at = buffer;
                continue;
            }
            let (bytes, mode) = (length & 0x00FF_FFFC, (length & 3) as u8);
            self.verite.trace(|| format!("DMA block {:08X} {} bytes mode {}", buffer, bytes, mode));
            for i in (0..bytes).step_by(4) {
                let word = swap(self.read_32((buffer + i) as usize), mode);
                self.verite.trace(|| format!("FIFO {:08X}", word));
                self.verite.fifo.push(word);
            }
            at += 8;
        }
        self.verite_execute();
        self.verite.flush_trace();
    }

    /// Carry out the commands waiting in the FIFO.
    fn verite_execute(&mut self) {
        let fx = self.verite.execute(&mut self.vbe.vram);
        if let Some(address) = fx.display {
            self.vbe.start = address;
            self.note_display_start();
        }
        if fx.drawn || fx.display.is_some() {
            self.vga.mark_dirty_full();
        }
    }

    fn verite_log(&mut self) {
        for line in std::mem::take(&mut self.verite.log) {
            self.log_string(&line);
        }
    }
}

/// INT 10h AH=15h: the Vérité BIOS's own functions, which Rendition's DOS
/// library calls to find the card and start its processor. As DOSBox's
/// Rendition fork answers them: AX=0015h for done.
pub fn bios(cpu: &mut crate::cpu::Cpu) {
    let al = cpu.get_al();
    cpu.bus.verite.trace(|| format!("INT 10h AX={:04X} BX={:04X} CX={:04X} DX={:04X}", cpu.ax(), cpu.bx(), cpu.cx(), cpu.dx()));
    match al {
        // The board's description, at DX:CX.
        0x8D => {
            let (segment, offset) = crate::verite::BOARD_DATA;
            cpu.set_ax(0x0015);
            cpu.set_dx(segment);
            cpu.set_cx(offset);
        }
        // The BIOS's version, and what it has.
        0x80 => {
            cpu.set_ax(0x0015);
            cpu.set_bx(0x0084);
        }
        // Hold the processor, for its microcode to be loaded.
        0x82 => {
            cpu.bus.verite.write(regs::MODE, 0x09, 1);
            cpu.bus.verite.run(None);
            cpu.set_ax(0x0015);
        }
        // Start it at DX:CX.
        0x83 => {
            cpu.bus.verite.run(Some((cpu.dx() as u32) << 16 | cpu.cx() as u32));
            cpu.set_ax(0x0015);
        }
        _ => cpu.set_ax(0x0015),
    }
    let line = format!("[VERITE] INT 10h AX={:04X}", 0x1500 | al as u16);
    cpu.bus.log_string(&line);
    cpu.bus.verite.flush_trace();
}
