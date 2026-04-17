//! Sound Blaster 2.0 emulation.
//!
//! Implements the Creative DSP (Digital Signal Processor) at base port 0x220,
//! an 8237-compatible DMA channel 1 for 8-bit PCM transfers, and the pieces
//! of the mixer games poke at for detection. The feature set targets DSP
//! version 2.01 — enough for Direct-DAC playback, single-cycle and auto-init
//! DMA PCM output up to ~44 kHz mono, speaker on/off, block-size setup, and
//! the DSP identification/version handshake that drivers use to confirm the
//! card is present.
//!
//! FM music already lives in `adlib.rs` (ports 0x388/0x389); the SB FM ports
//! at 0x228/0x229 are aliased through the bus, so nothing in this module
//! handles them directly.
//!
//! Per-sample PCM pull happens inside `audio::pump_audio` — each host audio
//! sample advances a phase accumulator, and when it overflows we fetch the
//! next byte from the DMA channel's memory window, signed-center it, and
//! emit it at the host output rate. DMA block completion raises IRQ 5 via
//! `irq_pending`, which `main.rs` delivers through the PIC like IRQ 0/1.

use std::collections::VecDeque;

// SB 2.0 DSP version. Major=2 Minor=1 → games detect "Sound Blaster 2.0".
// Older SB 1.x ignore minor; driver-shipped detection wants at least 2.00.
pub const DSP_VERSION_MAJOR: u8 = 2;
pub const DSP_VERSION_MINOR: u8 = 1;

/// Minimal 8237 DMA channel-1 state. Only what 8-bit PCM transfers touch:
/// the base addr/count pair, the page register, a mask flag, and the
/// flipflop that alternates LSB/MSB writes on ports 0x02 / 0x03.
#[derive(Default)]
pub struct Dma8237Ch1 {
    /// Base (latched) address written by the program. Reloaded into
    /// `cur_addr` on auto-init terminal count.
    pub base_addr: u16,
    pub base_count: u16,
    pub cur_addr: u16,
    pub cur_count: u16,
    /// High 8 bits of the 20-bit physical address; page << 16 | addr.
    pub page: u8,
    /// Mode byte from port 0x0B. Bit 4 = auto-init. We only act on bit 4;
    /// the transfer-type / direction bits don't affect SB playback because
    /// the DSP drives the pull regardless.
    pub mode: u8,
    /// True when the channel is masked (transfers halted).
    pub masked: bool,
    /// Address/count flipflop shared by all channels. False = next write is
    /// LSB, true = next write is MSB. Cleared by port 0x0C.
    pub flipflop: bool,
}

impl Dma8237Ch1 {
    /// Called after LSB+MSB of the address or count have been latched.
    /// Copies base→current so the next transfer starts from the programmed
    /// base. Real hardware does this implicitly on each base-reg write.
    fn arm(&mut self) {
        self.cur_addr = self.base_addr;
        self.cur_count = self.base_count;
    }
}

pub struct SoundBlaster {
    // ---- DSP reset handshake ----
    /// Tracks the "write 1 then 0 to port 0x226" reset sequence. 0 = idle,
    /// 1 = saw the initial 1. Completing it pushes 0xAA into `read_buf`
    /// so the driver's detection routine reads back the ready signature.
    reset_stage: u8,

    // ---- Command-byte state machine ----
    in_command: Option<u8>,
    param_buf: Vec<u8>,
    params_needed: usize,

    /// Read-side FIFO exposed at port 0x22A. DSP responses land here and
    /// drivers drain them one byte at a time.
    read_buf: VecDeque<u8>,

    /// Scratch register for DSP commands 0xE4 (write) / 0xE8 (read-back).
    /// A handful of drivers use this pair as a secondary sanity check.
    test_reg: u8,

    /// D1/D3 speaker enable. DMA output is still generated when off, but
    /// we gate the mixed-in audio so a game that forgets to turn it on
    /// stays silent — matching real hardware behavior.
    pub speaker_on: bool,

    // ---- Sample rate ----
    pub time_constant: u8,
    pub sample_rate_hz: u32,

    // ---- PCM rendering state ----
    /// Phase accumulator for DMA→host resampling. Advanced by
    /// (sample_rate_hz / output_rate) on each output sample. When it
    /// crosses 1.0 we pull the next byte from DMA memory.
    pub phase: f64,
    /// Last sample emitted, held between DMA pulls and used as the output
    /// of Direct-DAC command 0x10 until the next write.
    pub cur_sample: i16,

    // ---- DMA playback state ----
    pub dma_active: bool,
    pub dma_auto_init: bool,
    /// Block length minus 1 as latched by cmd 0x48, or derived from the
    /// length argument of 0x14/0x16/0x17. IRQ fires when this many bytes
    /// have been transferred; in auto-init mode it reloads afterward.
    pub dma_block_length: u32,
    pub dma_remaining: u32,

    /// Set by DMA terminal count, 0xF2/0xF8 force-IRQ, and 0x80 silence.
    /// Cleared when the program reads the buffer-status port 0x22E.
    pub irq_pending: bool,

    // ---- Mixer stubs (SB Pro CT1345) ----
    /// SB 2.0 proper has no mixer, but we accept writes to 0x224/0x225 so
    /// drivers that probe both addresses don't see open-bus garbage.
    mixer_reg: u8,
    mixer_regs: [u8; 256],
}

impl SoundBlaster {
    pub fn new() -> Self {
        Self {
            reset_stage: 0,
            in_command: None,
            param_buf: Vec::with_capacity(4),
            params_needed: 0,
            read_buf: VecDeque::new(),
            test_reg: 0,
            speaker_on: false,
            time_constant: 0,
            sample_rate_hz: 22050,
            phase: 0.0,
            cur_sample: 0,
            dma_active: false,
            dma_auto_init: false,
            dma_block_length: 0,
            dma_remaining: 0,
            irq_pending: false,
            mixer_reg: 0,
            mixer_regs: [0; 256],
        }
    }

    fn full_reset(&mut self) {
        self.in_command = None;
        self.param_buf.clear();
        self.read_buf.clear();
        self.speaker_on = false;
        self.dma_active = false;
        self.dma_auto_init = false;
        self.dma_remaining = 0;
        self.irq_pending = false;
        self.phase = 0.0;
        self.cur_sample = 0;
    }

    // ---- Port 0x226: DSP Reset ----
    /// The reset handshake is "write 1, hold for a bit, write 0". When the
    /// 0 arrives we clear everything and push 0xAA for the driver to read.
    pub fn write_reset(&mut self, value: u8) {
        if value & 0x01 != 0 {
            self.reset_stage = 1;
        } else if self.reset_stage == 1 {
            self.full_reset();
            self.read_buf.push_back(0xAA);
            self.reset_stage = 0;
        } else {
            self.reset_stage = 0;
        }
    }

    // ---- Port 0x22A: DSP Read Data ----
    pub fn read_data(&mut self) -> u8 {
        self.read_buf.pop_front().unwrap_or(0xFF)
    }

    // ---- Port 0x22C: Write-buffer status (bit 7 set = busy) ----
    /// We're always willing to accept a command byte immediately, so bit 7
    /// is always clear. The low 7 bits are open bus on real silicon; 0x7F
    /// is the canonical "ready" value drivers expect.
    pub fn read_write_status(&self) -> u8 {
        0x7F
    }

    // ---- Port 0x22E: Read-buffer status (bit 7 set = data avail) ----
    /// Reading this port ALSO acknowledges IRQ 5 on real hardware, which is
    /// how ISRs clear the interrupt line before issuing their EOI.
    pub fn read_buffer_status(&mut self) -> u8 {
        self.irq_pending = false;
        if self.read_buf.is_empty() {
            0x7F
        } else {
            0xFF
        }
    }

    // ---- Port 0x22C: DSP command / data byte ----
    pub fn write_command(&mut self, value: u8) {
        if let Some(cmd) = self.in_command {
            self.param_buf.push(value);
            if self.param_buf.len() >= self.params_needed {
                self.execute_command(cmd);
                self.in_command = None;
                self.param_buf.clear();
            }
            return;
        }

        // Number of parameter bytes expected after this command byte. The
        // DSP latches them one at a time via successive writes to 0x22C;
        // commands with needs==0 execute immediately.
        let needs = match value {
            0x10 => 1,                 // Direct DAC
            0x14 | 0x16 | 0x17 => 2,   // 8-bit single-cycle DMA output
            0x24 => 2,                 // 8-bit single-cycle DMA input
            0x40 => 1,                 // Set time constant
            0x41 => 2,                 // Set output sample rate (SB16; harmless)
            0x48 => 2,                 // Set DMA block size
            0x80 => 2,                 // Silence DAC (pause n samples, then IRQ)
            0xE0 => 1,                 // DSP identification (returns ~byte)
            0xE4 => 1,                 // Write test register
            _ => 0,
        };
        if needs == 0 {
            self.execute_command(value);
        } else {
            self.in_command = Some(value);
            self.params_needed = needs;
        }
    }

    fn execute_command(&mut self, cmd: u8) {
        match cmd {
            // --- Direct DAC output. One byte, held until overwritten. ---
            0x10 => {
                let s = self.param_buf[0];
                // 8-bit unsigned PCM centered at 128 → signed i16.
                self.cur_sample = ((s as i16) - 128) << 8;
            }

            // --- 8-bit single-cycle DMA output. Length = param+1 bytes. ---
            0x14 | 0x16 | 0x17 => {
                let len = u16::from_le_bytes([self.param_buf[0], self.param_buf[1]]) as u32 + 1;
                self.dma_active = true;
                self.dma_auto_init = false;
                self.dma_remaining = len;
                self.dma_block_length = len;
            }

            // --- 8-bit single-cycle DMA input. We don't synthesize audio
            // input, so just consume the program's block length and IRQ.
            0x24 => {
                let len = u16::from_le_bytes([self.param_buf[0], self.param_buf[1]]) as u32 + 1;
                self.dma_remaining = 0;
                self.dma_block_length = len;
                self.irq_pending = true;
            }

            // --- 8-bit auto-init DMA output. Uses previously set 0x48 len. ---
            0x1C | 0x1F => {
                self.dma_active = true;
                self.dma_auto_init = true;
                self.dma_remaining = self.dma_block_length;
            }
            // --- Auto-init ADC: stub, matches 0x24 behavior. ---
            0x2C | 0x2F => {
                self.dma_active = false;
                self.irq_pending = true;
            }

            // --- Set time constant. Sample rate = 1e6 / (256 - tc). ---
            // Clamp tc upper bound so TC=0xFF (rate=1 MHz) doesn't produce
            // absurd phase increments; real SB 2.0 tops out near 44.1 kHz.
            0x40 => {
                let tc = self.param_buf[0];
                self.time_constant = tc;
                let denom = 256u32.saturating_sub(tc as u32).max(1);
                let rate = (1_000_000u32 / denom).clamp(4_000, 48_000);
                self.sample_rate_hz = rate;
            }

            // --- SB16 "set output sample rate" command. High-byte-first. ---
            0x41 => {
                let rate = u16::from_be_bytes([self.param_buf[0], self.param_buf[1]]) as u32;
                self.sample_rate_hz = rate.clamp(4_000, 48_000);
            }

            // --- Set DMA block size (len-1). ---
            0x48 => {
                let len = u16::from_le_bytes([self.param_buf[0], self.param_buf[1]]) as u32 + 1;
                self.dma_block_length = len;
            }

            // --- Silence DAC for N samples, then IRQ. We fire IRQ
            // immediately so games waiting for the silence completion
            // don't stall; inaudible-filler semantics are preserved.
            0x80 => {
                self.cur_sample = 0;
                self.irq_pending = true;
            }

            // --- Halt/continue 8-bit DMA. ---
            0xD0 => { self.dma_active = false; }
            0xD4 => { self.dma_active = true; }
            // Exit auto-init at end of current block.
            0xDA => { self.dma_auto_init = false; }

            // --- Speaker on/off. ---
            0xD1 => { self.speaker_on = true; }
            0xD3 => { self.speaker_on = false; }
            0xD8 => {
                self.read_buf
                    .push_back(if self.speaker_on { 0xFF } else { 0x00 });
            }

            // --- DSP identification. Echoes ~byte, drivers compare. ---
            0xE0 => {
                self.read_buf.push_back(!self.param_buf[0]);
            }
            // --- DSP version. Two bytes, major then minor. ---
            0xE1 => {
                self.read_buf.push_back(DSP_VERSION_MAJOR);
                self.read_buf.push_back(DSP_VERSION_MINOR);
            }
            // --- Test register R/W. Some detection code writes a known
            // byte and reads it back to confirm the DSP accepts params.
            0xE4 => { self.test_reg = self.param_buf[0]; }
            0xE8 => { self.read_buf.push_back(self.test_reg); }

            // --- Force IRQ. Sound systems use this during init to
            // confirm their IRQ line is wired up correctly.
            0xF2 | 0xF8 => { self.irq_pending = true; }

            // --- ADPCM and other unimplemented opcodes: swallow silently.
            _ => {}
        }
    }

    // ---- Mixer (0x224/0x225). Writes stored, reads echo. SB Pro+ only. ----
    pub fn mixer_index_write(&mut self, value: u8) { self.mixer_reg = value; }
    pub fn mixer_data_write(&mut self, value: u8) {
        self.mixer_regs[self.mixer_reg as usize] = value;
    }
    pub fn mixer_data_read(&self) -> u8 {
        self.mixer_regs[self.mixer_reg as usize]
    }

    /// Advance one host-rate PCM sample. Called from `pump_audio` with the
    /// system RAM slice and the DMA channel state. Returns the i16 sample
    /// to mix into the host output. When DMA is inactive, returns the
    /// last-held sample (so Direct-DAC writes persist until overwritten).
    pub fn advance_one(&mut self, ram: &[u8], dma: &mut Dma8237Ch1, host_rate: u32) -> i16 {
        if !self.dma_active || dma.masked || self.sample_rate_hz == 0 {
            return self.cur_sample;
        }
        self.phase += self.sample_rate_hz as f64 / host_rate as f64;
        // `while` rather than `if` — at very high SB rates (≥44 kHz)
        // against a 44.1 kHz host the increment is ~1.0 and occasional
        // double-pulls keep us synced. Games never exceed this.
        while self.phase >= 1.0 {
            self.phase -= 1.0;
            if self.dma_remaining == 0 { break; }
            let phys = (((dma.page as usize) << 16) | (dma.cur_addr as usize)) & 0xFFFFF;
            let byte = ram[phys];
            self.cur_sample = ((byte as i16) - 128) << 8;
            dma.cur_addr = dma.cur_addr.wrapping_add(1);
            dma.cur_count = dma.cur_count.wrapping_sub(1);
            self.dma_remaining -= 1;
            if self.dma_remaining == 0 {
                self.irq_pending = true;
                if self.dma_auto_init {
                    self.dma_remaining = self.dma_block_length;
                    dma.cur_addr = dma.base_addr;
                    dma.cur_count = dma.base_count;
                } else {
                    self.dma_active = false;
                }
            }
        }
        self.cur_sample
    }
}

impl Default for SoundBlaster {
    fn default() -> Self { Self::new() }
}

// ============================================================
// 8237 DMA port helpers. Exposed so the bus can delegate its
// 0x00-0x0F and 0x8x dispatches without touching internals.
// ============================================================
impl Dma8237Ch1 {
    /// Port 0x02 write — channel 1 base address, LSB then MSB.
    pub fn write_addr(&mut self, value: u8) {
        if !self.flipflop {
            self.base_addr = (self.base_addr & 0xFF00) | value as u16;
            self.flipflop = true;
        } else {
            self.base_addr = (self.base_addr & 0x00FF) | ((value as u16) << 8);
            self.flipflop = false;
            self.cur_addr = self.base_addr;
        }
    }
    /// Port 0x03 write — channel 1 count, LSB then MSB.
    pub fn write_count(&mut self, value: u8) {
        if !self.flipflop {
            self.base_count = (self.base_count & 0xFF00) | value as u16;
            self.flipflop = true;
        } else {
            self.base_count = (self.base_count & 0x00FF) | ((value as u16) << 8);
            self.flipflop = false;
            self.cur_count = self.base_count;
            self.arm();
        }
    }
    pub fn read_addr(&mut self) -> u8 {
        let b = if !self.flipflop {
            self.flipflop = true;
            (self.cur_addr & 0xFF) as u8
        } else {
            self.flipflop = false;
            (self.cur_addr >> 8) as u8
        };
        b
    }
    pub fn read_count(&mut self) -> u8 {
        let b = if !self.flipflop {
            self.flipflop = true;
            (self.cur_count & 0xFF) as u8
        } else {
            self.flipflop = false;
            (self.cur_count >> 8) as u8
        };
        b
    }
    /// Port 0x0A — single-channel mask. Bits 0-1 = channel, bit 2 = mask.
    pub fn write_single_mask(&mut self, value: u8) {
        if (value & 0x03) == 0x01 {
            self.masked = (value & 0x04) != 0;
        }
    }
    /// Port 0x0B — mode register. Bits 0-1 = channel, bit 4 = auto-init.
    pub fn write_mode(&mut self, value: u8) {
        if (value & 0x03) == 0x01 {
            self.mode = value;
        }
    }
    /// Port 0x0C — clear LSB/MSB flipflop. Value is irrelevant.
    pub fn clear_flipflop(&mut self) { self.flipflop = false; }
    /// Port 0x0D — master reset. Clears mask state too.
    pub fn master_reset(&mut self) {
        self.masked = true;
        self.flipflop = false;
    }
    /// Port 0x83 — channel 1 page register.
    pub fn write_page(&mut self, value: u8) { self.page = value; }
    pub fn read_page(&self) -> u8 { self.page }
}
