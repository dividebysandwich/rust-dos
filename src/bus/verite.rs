//! The Rendition Vérité on the bus: its ports, the DMA that feeds its
//! FIFO from lists in the PC's memory, and its BIOS's own functions.
//!
//! With `verite_risc=lle` the chip's own registers (below 80h) are those
//! of `verite::lle::Core`, whose RISC runs as the emulated clock goes:
//! before each access of the card's ports, and every millisecond while it
//! has work, as a timer event. The board's registers (the CRTC, RAMDAC
//! and clocks) are the same either way.

use super::Bus;
use crate::verite::{lle, regs, swap};
use crate::video::VideoMode;
use crate::video::vbe::VbeMode;

/// DMA lists followed at most, against loops.
const DMA_ENTRIES: usize = 1 << 16;

/// How often the RISC runs while the host leaves it alone: 1 ms.
const RUN_TICKS: u64 = crate::timer::PIT_HZ / 1000;

/// CRTCCTL's bits the CRTC sets ([SPEC] 5.14): the frame FRAMEBASEA
/// names was shown, it was switched to, and the stereo frame is even.
const FRAME_DISPLAYED: u32 = 0x4000;
const EVEN_FRAME: u32 = 0x1_0000;
const FRAME_SWITCHED: u32 = 0x4_0000;

impl Bus {
    /// Whether the display adapter is the Vérité.
    pub fn verite(&self) -> bool {
        self.vga.adapter.is_verite()
    }

    /// The Vérité's register at `port`, if it is one.
    pub(crate) fn verite_port(&self, port: u16) -> Option<u8> {
        if self.verite() { self.verite.port(port) } else { None }
    }

    /// Whether the Vérité's RISC runs at a low level.
    fn verite_lle(&self) -> bool {
        self.verite() && self.verite.lle_on
    }

    /// Set the Vérité up as the settings ask: its RISC, the board ROM it
    /// sees, its speed and interrupt line. Returns what went wrong.
    pub fn configure_verite(&mut self, settings: &crate::verite::VeriteSettings) -> Vec<String> {
        let mut warnings = Vec::new();
        let rom = match &settings.rom {
            Some(path) if settings.risc == crate::verite::Risc::Lle => match std::fs::read(path) {
                Ok(data) if data.len() == lle::ROM_SIZE => Some(data),
                Ok(data) => {
                    warnings.push(format!(
                        "verite_rom: {} has {} bytes, not the {} of a Vérité board's ROM",
                        path.display(),
                        data.len(),
                        lle::ROM_SIZE
                    ));
                    None
                }
                Err(e) => {
                    warnings.push(format!("verite_rom: {}: {}", path.display(), e));
                    None
                }
            },
            _ => None,
        };
        self.verite.configure(settings, rom);
        if self.verite() {
            let (segment, offset) = crate::verite::BOARD_DATA;
            let data = crate::verite::board_data(crate::video::vbe::VRAM_SIZE as u32, settings.irq);
            self.write_rom(((segment as usize) << 4) + offset as usize, &data);
        }
        self.sync_verite_irq(false);
        warnings
    }

    pub(crate) fn verite_read(&mut self, reg: u8, len: u8) -> u32 {
        if self.verite_lle() {
            self.verite_sync();
            if reg < 0x80 {
                let core = self.verite.lle.as_mut().expect("low-level Vérité");
                let value = core.io_read(reg, len, &self.vbe.vram);
                let w = 2 * len as usize;
                self.verite.trace(|| format!("r {:<14} {} = {:0w$X}", regs::name(reg), len, value));
                self.verite_settled();
                return value;
            }
        }
        self.verite_ext_read(reg, len)
    }

    pub(crate) fn verite_write(&mut self, reg: u8, value: u32, len: u8) {
        if self.verite_lle() {
            self.verite_sync();
            if reg < 0x80 {
                let w = 2 * len as usize;
                self.verite.trace(|| format!("w {:<14} {} = {:0w$X}", regs::name(reg), len, value));
                self.verite_core(|core, vram, bus| core.io_write(reg, value, len, vram, bus));
                // BankSelect is the window at A0000h, in 8 KB units.
                if reg < lle::io::BANK_SELECT + 4 && reg + len > lle::io::BANK_SELECT {
                    let select = self.verite.lle.as_ref().map_or(0, |core| core.bank_select());
                    self.vbe.bank = (select & 0x00FF_E000) >> 16;
                }
                // A word for a RISC that waits for one is taken at once.
                if reg < 0x10 {
                    self.verite_sync();
                }
                self.verite_settled();
                return;
            }
            self.verite_ext_write(reg, value, len);
            return;
        }
        if Self::verite_dac_port(reg) {
            self.verite_ext_write(reg, value, len);
            return;
        }
        self.verite.write(reg, value, len);
        if reg < regs::CRTCSTATUS && reg + len > regs::CRTCCTL {
            self.verite_settle();
        }
        if reg <= regs::DEBUGREG && reg + len > regs::DEBUGREG {
            self.verite.debug(&mut self.vbe.vram);
        }
        self.verite_execute();
        // The DMA list pointer's last byte starts the DMA.
        if reg <= regs::DMACMDPTR + 3 && reg + len > regs::DMACMDPTR + 3 {
            self.verite_dma();
        }
        self.verite_log();
    }

    /// A read of the board's registers (80h on): the CRTC's, whose status
    /// the beam's position gives, and the RAMDAC's.
    fn verite_ext_read(&mut self, reg: u8, len: u8) -> u32 {
        if (regs::CRTCSTATUS..regs::CRTCSTATUS + 4).contains(&reg) {
            let status = crate::verite::crtc_status(&self.vga.timing(), self.clock.now_ns());
            return status >> (8 * (reg - regs::CRTCSTATUS));
        }
        if Self::verite_dac_port(reg) {
            let mut value = 0;
            for i in 0..len {
                let byte = self.verite.dac.read((reg + i - regs::PALETTE) & 0xF, &self.vga.palette);
                value |= (byte as u32) << (8 * i);
            }
            self.verite.trace(|| format!("r DAC {:02X} {} = {:X}", reg - regs::PALETTE, len, value));
            return value;
        }
        if self.verite_lle() && (regs::CRTCCTL..regs::CRTCCTL + 4).contains(&reg) {
            let ctl = self.verite.read_quiet(regs::CRTCCTL) | self.verite_frame_flags();
            return ctl >> (8 * (reg - regs::CRTCCTL));
        }
        let value = self.verite.read(reg, len);
        self.verite_log();
        value
    }

    /// A write of the board's registers (80h on).
    fn verite_ext_write(&mut self, reg: u8, value: u32, len: u8) {
        if Self::verite_dac_port(reg) {
            self.verite.trace(|| format!("w DAC {:02X} {} = {:X}", reg - regs::PALETTE, len, value));
            for i in 0..len {
                self.verite.dac.write((reg + i - regs::PALETTE) & 0xF, (value >> (8 * i)) as u8, &mut self.vga.palette);
            }
            self.vga.dac_8bit = self.verite.dac.dac_8bit();
            self.vga.mark_dirty_full();
            return;
        }
        self.verite.write(reg, value, len);
        if reg <= regs::FRAMEBASEA + 3 && reg + len > regs::FRAMEBASEA {
            self.verite.frame_written_ns = self.clock.now_ns();
        }
        if reg < regs::CRTCSTATUS && reg + len > regs::CRTCCTL {
            self.verite_settle();
        }
        self.verite_log();
    }

    /// CRTCCTL's frame bits at the time: FrameSwitched in the vertical
    /// blank or once one came after FRAMEBASEA was written, FrameDisplayed
    /// once the frame after it began, and the even stereo frame.
    fn verite_frame_flags(&mut self) -> u32 {
        let timing = self.vga.timing();
        let (now, written) = (self.clock.now_ns(), self.verite.frame_written_ns);
        let frame = timing.frame_ns().max(1);
        // Vertical blanks begun up to a time: they begin past the display.
        let blank_ns = timing.display as u64 * timing.line_ns as u64;
        let blanks = |t: u64| (t + frame - blank_ns) / frame;
        let blanking = timing.position(now).0 >= timing.display;
        let mut flags = 0;
        if blanking || blanks(now) > blanks(written) {
            flags |= FRAME_SWITCHED;
        }
        if now / frame > written / frame {
            flags |= FRAME_DISPLAYED;
        }
        if (now / frame).is_multiple_of(2) {
            flags |= EVEN_FRAME;
        }
        flags
    }

    /// Run `f` with the low-level chip, the card's memory and the bus as
    /// its board, the chip and the memory taken out of the bus meanwhile.
    fn verite_core<R>(
        &mut self,
        f: impl FnOnce(&mut lle::Core, &mut [u8], &mut Bus) -> R,
    ) -> Option<R> {
        let mut core = self.verite.lle.take()?;
        let mut vram = std::mem::take(&mut self.vbe.vram);
        let result = f(&mut core, &mut vram, self);
        self.vbe.vram = vram;
        self.verite.lle = Some(core);
        Some(result)
    }

    /// Bring the low-level chip up to the present: the vertical retraces
    /// since its last run, and the instructions its RISC is owed.
    pub(crate) fn verite_sync(&mut self) {
        if !self.verite_lle() || self.verite.lle.is_none() {
            return;
        }
        let _profile = crate::profile::zone(crate::profile::Zone::IoVerite);
        let now = self.clock.now_ns();
        let retraces = self.vga.timing().retraces(now);
        if retraces != self.verite.retraces {
            self.verite.retraces = retraces;
            if let Some(core) = &mut self.verite.lle {
                core.vertical_retrace();
            }
        }
        self.verite_core(|core, vram, bus| core.catch_up(now, vram, bus));
        self.verite_settled();
    }

    /// After the chip ran or was accessed: what it drew shown, its log
    /// passed on, its interrupt request up or down.
    fn verite_settled(&mut self) {
        let Some(core) = &mut self.verite.lle else {
            return;
        };
        let drew = std::mem::take(&mut core.drew);
        let log = std::mem::take(&mut core.log);
        let irq = core.irq();
        self.sync_verite_irq(irq);
        self.verite_host_view();
        if drew {
            self.vga.mark_dirty_full();
        }
        for line in log {
            self.log_string(&line);
        }
        self.clock.schedule(self.next_event());
    }

    /// How the host's and the display's accesses to the card's memory are
    /// swapped: by the MemEndian register for the host, by the pixel size
    /// for the display, which shows the RISC's big-endian pixels.
    fn verite_host_view(&mut self) {
        let (host, display) = match &self.verite.lle {
            Some(core) if self.verite() => {
                let host = [0, 3, 1, 2][core.mem_endian() as usize];
                let display = match self.vbe.mode.map(|m| m.bpp) {
                    Some(8) => 3,
                    Some(15 | 16) => 2,
                    _ => 0,
                };
                (host, if self.video_mode == VideoMode::Vesa { display } else { 0 })
            }
            _ => (0, 0),
        };
        if self.vbe.host_xor != host || self.vbe.scan_xor != display {
            self.vbe.host_xor = host;
            self.vbe.scan_xor = display;
            self.vga.mark_dirty_full();
        }
    }

    /// After a state was loaded: the retraces counted from now, the
    /// interrupt request and the memory's byte order as the state's chip
    /// has them.
    pub(crate) fn verite_after_load(&mut self) {
        self.verite.retraces = self.vga.timing().retraces(self.clock.now_ns());
        let irq = self.verite.lle.as_ref().is_some_and(|core| core.irq());
        self.sync_verite_irq(irq);
        self.verite_host_view();
    }

    /// The card's interrupt request up (`level`) or down, on the line the
    /// BIOS routed it to.
    pub(crate) fn sync_verite_irq(&mut self, level: bool) {
        let line = (level && self.verite()).then(|| self.verite.irq_line());
        let old = std::mem::replace(&mut self.verite.line, line);
        if let Some(old) = old
            && line != Some(old)
        {
            self.withdraw_irq(old);
        }
        if let Some(irq) = line
            && old != line
        {
            self.pic.raise(irq);
        }
    }

    /// When the low-level chip next needs to run: in a millisecond while
    /// its RISC has work, else at the next retrace if its interrupt is on.
    pub(crate) fn verite_next_event(&self) -> Option<u64> {
        let core = self.verite.lle.as_ref().filter(|_| self.verite())?;
        let now = self.clock.now_ticks();
        if core.busy() {
            return Some(now + RUN_TICKS);
        }
        if core.wants_retrace() {
            let timing = self.vga.peek_timing();
            let now_ns = self.clock.now_ns();
            let wait = timing.next_retrace(now_ns + 1).saturating_sub(now_ns);
            return Some(now + crate::timer::mul_div(wait, crate::timer::PIT_HZ, 1_000_000_000) + 1);
        }
        None
    }

    /// The board's register writes and reads the low-level chip's RISC
    /// makes, and its bus master reads.
    pub(crate) fn verite_service(&mut self) {
        self.verite_sync();
    }

    /// A VESA mode set on the low-level chip: as the Rendition BIOS does,
    /// its own CRTC shows the mode, with VESA timings, modes up to 400
    /// pixels wide line-doubled, and MemEndian set so that the frame
    /// buffer looks linear to the PC (bytes swapped at 8 bits a pixel,
    /// half-words at 16, nothing at 32).
    pub(crate) fn verite_program_mode(&mut self, mode: &VbeMode) {
        if !self.verite_lle() {
            return;
        }
        let (width, height) = (mode.width as u32, mode.height as u32);
        let double = width <= 400;
        let lines = if double { height * 2 } else { height };
        let eighths = |px: u32| (px / 8).max(1);
        let (front, sync) = (eighths(width / 40), eighths(width * 3 / 20));
        let back = eighths(width * 3 / 40);
        let horz = (width / 8 - 1) | (back - 1) << 9 | (sync - 1) << 16 | (front.min(8) - 1) << 21;
        let total = match lines {
            0..=480 => 525,
            481..=600 => 628,
            601..=768 => 806,
            _ => lines * 1066 / 1024,
        };
        let spare = total - lines;
        let v_back = ((spare - 2) * 3 / 4).clamp(1, 64);
        let v_front = (spare - 2 - v_back).clamp(1, 64);
        let vert = (lines - 1) | (v_back - 1) << 11 | (2 - 1) << 17 | (v_front - 1) << 20;
        let (format, endian) = match mode.bpp {
            8 => (1, 1),
            15 => (6, 3),
            16 => (4, 3),
            _ => (12, 0),
        };
        let ctl = format | 0x10 | 0x0C00 | 0x1000 | if double { 0x2_0000 } else { 0 };
        self.verite_write(regs::CRTCHORZ, horz, 4);
        self.verite_write(regs::CRTCVERT, vert, 4);
        self.verite_crtc_follow();
        self.verite_write(regs::CRTCCTL, ctl, 4);
        self.verite_write(lle::io::MEM_ENDIAN, endian, 1);
        let mode = self.verite_mode() & lle::mode::DMA | lle::mode::VESA;
        self.verite_write(lle::io::MODE, mode as u32, 1);
    }

    /// The low-level chip's Mode register: a mode set keeps its DMA on.
    fn verite_mode(&self) -> u8 {
        self.verite.lle.as_ref().map_or(0, |core| core.mode())
    }

    /// The CRTC's frame base and line offset after the VBE moved the
    /// display start or changed the line length ([SPEC] 5.14: the offset
    /// is what follows the last video FIFO fetch of a line, of 128 bytes).
    pub(crate) fn verite_crtc_follow(&mut self) {
        let Some(mode) = self.vbe.mode.filter(|_| self.verite_lle()) else {
            return;
        };
        let (start, pitch) = (self.vbe.start, self.vbe.pitch);
        let bytes = mode.width as u32 * (mode.bpp as u32).div_ceil(8);
        let fetched = if start & 7 == 0 { (bytes - 1) / 128 * 128 } else { bytes / 128 * 128 };
        self.verite_write(regs::FRAMEBASEA, start, 4);
        self.verite_write(regs::CRTCOFFSET, pitch.saturating_sub(fetched) & 0xFFFF, 4);
    }

    /// The VBE's window moved to 64 KB bank `bank`: the low-level chip's
    /// BankSelect says the same.
    pub(crate) fn verite_set_bank(&mut self, bank: u32) {
        if self.verite_lle() {
            self.verite_write(lle::io::BANK_SELECT, bank << 16, 4);
        }
    }

    /// A standard mode set: the low-level chip's CRTC lets the VGA show it,
    /// with no byte swapping, as the Rendition BIOS leaves it.
    pub(crate) fn verite_mode_set(&mut self) {
        if !self.verite_lle() {
            return;
        }
        let ctl = self.verite.read_quiet(regs::CRTCCTL);
        self.verite_write(regs::CRTCCTL, ctl & !0x1C00, 4);
        self.verite_write(lle::io::MEM_ENDIAN, 0, 1);
        let mode = self.verite_mode() & lle::mode::DMA | lle::mode::VGA;
        self.verite_write(lle::io::MODE, mode as u32, 1);
    }

    /// The low-level chip's state for the debugger.
    pub fn verite_lle_describe(&self) -> Option<serde_json::Value> {
        self.verite.lle.as_ref().map(|core| core.describe())
    }

    /// Whether `reg` is one of the RAMDAC's ports.
    fn verite_dac_port(reg: u8) -> bool {
        (regs::PALETTE..regs::PALETTE + 16).contains(&reg)
    }

    /// Show what the card's own CRTC describes, as a VESA mode, or go back
    /// to the VGA's when its video goes off. Runs after every write to its
    /// registers.
    fn verite_settle(&mut self) {
        match self.verite.screen() {
            Some(screen) => {
                // A VESA mode the BIOS set keeps its number.
                let size = (screen.width, screen.height, screen.bpp);
                let number = self
                    .vbe
                    .mode
                    .filter(|m| (m.width, m.height, m.bpp) == size)
                    .map_or(0, |m| m.number);
                let mode = VbeMode {
                    number,
                    width: screen.width,
                    height: screen.height,
                    bpp: screen.bpp,
                    timing: screen.timing,
                };
                let same = self.verite.native
                    && self.video_mode == VideoMode::Vesa
                    && self.vbe.mode == Some(mode)
                    && self.vbe.pitch == screen.pitch;
                if !same {
                    let size = |m: &VbeMode| (m.width, m.height, m.bpp);
                    if !self.verite.native || self.vbe.mode.as_ref().map(size) != Some(size(&mode)) {
                        let line = format!(
                            "[VERITE] Display {}x{}, {} bits per pixel, {} bytes a line",
                            screen.width, screen.height, screen.bpp, screen.pitch
                        );
                        self.log_string(&line);
                    }
                    self.verite.native = true;
                    self.vbe.mode = Some(mode);
                    self.vbe.pitch = screen.pitch;
                    self.video_mode = VideoMode::Vesa;
                    self.vga.set_fixed_timing(Some(screen.timing));
                    self.vga.mark_dirty_full();
                }
                if self.vbe.start != screen.base {
                    self.vbe.start = screen.base;
                    self.note_display_start();
                }
            }
            None if self.verite.native => {
                self.verite.native = false;
                self.vbe.reset();
                self.vga.set_fixed_timing(None);
                let mode = self.vga.register_mode().or_else(|| self.vga.check_video_mode()).unwrap_or(VideoMode::Text80x25Color);
                self.log_string(&format!("[VERITE] Back to the VGA's {:?}", mode));
                self.video_mode = mode;
                self.vga.mark_dirty_full();
            }
            None => {}
        }
        self.verite_host_view();
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

impl lle::Board for Bus {
    fn ext_read(&mut self, io: u8, len: u8) -> u32 {
        self.verite_ext_read(io, len)
    }

    fn ext_write(&mut self, io: u8, value: u32, len: u8) {
        self.verite_ext_write(io, value, len);
    }

    /// The PC's RAM; the card can't read the frame buffer it is.
    fn dma_read(&mut self, phys: u32) -> u32 {
        if (phys as usize).saturating_add(4) <= self.ram.len() {
            self.read_32(phys as usize)
        } else {
            0
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
        0x82..=0x84 if cpu.bus.verite_lle() => lle_bios(cpu, al),
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

/// The functions that drive the RISC, on the low-level chip, as the
/// Rendition BIOS has them:
/// - 1582h readies the board for a microcode load: VESA mode 101h, the
///   RISC reset and held, the memory controller and the interrupts reset,
///   the display off, VESA and DMA on with a low water mark of 3, no byte
///   swapping, bank 0, and its palette loader at address 0;
/// - 1583h starts the RISC at CX (DX the high word): a jump forced in
///   with its delay slot, the interrupts cleared, the hold released;
///   BX=0064h, DI=0200h;
/// - 1584h releases it.
fn lle_bios(cpu: &mut crate::cpu::Cpu, al: u8) {
    use lle::{debug, io, mode};
    let bus = &mut cpu.bus;
    let write = |bus: &mut Bus, reg: u8, value: u32, len: u8| bus.verite_write(reg, value, len);
    match al {
        0x82 => {
            if let Some(m) = crate::video::vbe::find_mode(0x101, false) {
                crate::interrupts::vbe::enter_mode(bus, &m, true, false);
            }
            write(bus, io::DEBUG, (debug::SOFT_RESET | debug::HOLD) as u32, 1);
            write(bus, io::DEBUG, debug::HOLD as u32, 1);
            write(bus, regs::DRAMCTL, 0x0014_0000, 4);
            write(bus, io::INTR, 0xFFFF, 2);
            let ctl = bus.verite.read_quiet(regs::CRTCCTL);
            write(bus, regs::CRTCCTL, ctl & !0x1C00, 4);
            write(bus, io::MODE, (mode::VESA | mode::DMA) as u32, 1);
            write(bus, io::LOW_WATER_MARK, 3, 1);
            write(bus, io::MEM_ENDIAN, 0, 1);
            write(bus, io::BANK_SELECT, 0, 4);
            for (k, &word) in lle::PALETTE_LOADER.iter().enumerate() {
                lle::write_word(&mut bus.vbe.vram, 4 * k as u32, word);
            }
            bus.vga.mark_dirty_full();
        }
        0x83 => {
            let pc = (cpu.dx() as u32) << 16 | cpu.cx() as u32;
            let bus = &mut cpu.bus;
            write(bus, io::DEBUG, debug::HOLD as u32, 1);
            for ir in [0x6C00_0000 | (pc >> 2 & 0xFF_FFFF), 0] {
                write(bus, io::STATE_INDEX, 0x80, 1);
                write(bus, io::STATE_DATA, ir, 4);
                write(bus, io::DEBUG, (debug::HOLD | debug::STEP) as u32, 1);
            }
            write(bus, io::INTR, 0xFFFF, 2);
            write(bus, io::DEBUG, 0, 1);
            cpu.set_bx(0x0064);
            cpu.set_di(0x0200);
        }
        _ => write(bus, io::DEBUG, 0, 1),
    }
    cpu.set_ax(0x0015);
}
