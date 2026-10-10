//! The Sound Blaster AWE32's EMU8000: 32 wavetable voices at 44.1 kHz
//! playing 16-bit samples from a 1 MB General MIDI ROM and the card's
//! sample RAM, each with a resonant filter, two envelopes and two LFOs,
//! and a chorus, reverb and equalizer after them. It sits at the Sound
//! Blaster's base + 400h, 800h and C00h (620h, A20h, E20h for a card at
//! 220h), accessed a word at a time through a register pointer.
//!
//! The emulation follows 86Box's EMU8000 with the corrections the AWE32Emu
//! project measured on a real card (as in turican0's DOSBox-X AWE32 fork):
//! the B-spline interpolation, the Chamberlin filter, the attack shapes,
//! the Creative drivers' envelope rates, the one-pole volume slide, the
//! fitted reverb and equalizer and the output level.
//!
//! The chip renders a frame at a time as the mixer asks, and the bus
//! catches the audio up before every register access, so each write lands
//! on its own sample. Its sample counter follows emulated time.
//!
//! After power-on the chip is muted; the card here starts in the state
//! Creative's `AWEUTIL /S` leaves it in (see `init`), which some programs
//! (Doom) depend on.

pub mod effects;
pub mod rom;
pub mod tables;
pub mod voice;

use std::sync::Arc;

use effects::{Chorus, Eq, Init, Reverb};
use voice::{Sums, Voice};

/// Word addresses of sample memory: the ROM at 0, the RAM from 200000h.
const ADDRESS_MASK: u32 = 0xFF_FFFF;
const RAM_START: u32 = 0x20_0000;
/// The FM passthrough area at the top, which RAM writes skip.
const FM_ADDRESS: u32 = 0xFF_FFE0;
/// The ROM's words.
pub const ROM_WORDS: usize = 0x8_0000;

/// Largest RAM, in KB: 28 MB fill the address space.
pub const MAX_RAM_KB: u32 = 28 * 1024;
/// The RAM sizes cards came with, in KB.
pub const RAM_SIZES: [u32; 9] = [0, 512, 1024, 2048, 4096, 8192, 12288, 16384, 28672];
pub const DEFAULT_RAM_KB: u32 = 512;

// Port offsets from the chip's base (Sound Blaster base + 400h).
const DATA0: u16 = 0x000;
const DATA0_HI: u16 = 0x002;
const DATA1: u16 = 0x400;
const DATA2: u16 = 0x402;
const DATA3: u16 = 0x800;
const POINTER: u16 = 0x802;

/// The output level: the card clips one voice 1.1 dB lower than full.
const OUTPUT_GAIN: i64 = 57737;

/// Sample memory: the ROM, shared between cards and never saved, and RAM.
#[derive(Clone, Debug, Default)]
pub struct Memory {
    rom: Arc<[i16]>,
    /// Little-endian words.
    ram: Vec<u8>,
    /// The first word address past the RAM.
    ram_end: u32,
}

impl Memory {
    fn new(rom: Arc<[i16]>, ram_kb: u32) -> Self {
        let kb = ram_kb.min(MAX_RAM_KB);
        let mut mem = Self { rom, ram: vec![0; kb as usize * 1024], ram_end: 0 };
        mem.ram_end = RAM_START + (mem.ram.len() / 2) as u32;
        mem
    }

    /// The word at `addr`; unfitted addresses read 0.
    #[inline]
    pub fn word(&self, addr: u32) -> i16 {
        let a = addr & ADDRESS_MASK;
        if (a as usize) < ROM_WORDS {
            self.rom.get(a as usize).copied().unwrap_or(0)
        } else if a >= RAM_START && a < self.ram_end {
            let i = 2 * (a - RAM_START) as usize;
            i16::from_le_bytes([self.ram[i], self.ram[i + 1]])
        } else {
            0
        }
    }

    /// Write RAM. Addresses past its end wrap around, which programs
    /// sizing the RAM (Cubic Player) look for; the ROM and the FM area
    /// don't take writes.
    fn write(&mut self, addr: u32, value: u16) {
        let a = addr & ADDRESS_MASK;
        if self.ram.is_empty() || !(RAM_START..FM_ADDRESS).contains(&a) {
            return;
        }
        let i = 2 * ((a - RAM_START) as usize % (self.ram.len() / 2));
        self.ram[i..i + 2].copy_from_slice(&value.to_le_bytes());
    }

    pub fn ram_kb(&self) -> u32 {
        (self.ram.len() / 1024) as u32
    }
}

pub struct Emu8000 {
    /// The chip's first port: the Sound Blaster's base + 400h.
    pub base: u16,
    mem: Memory,
    voices: [Voice; 32],
    hwcf1: u16,
    hwcf2: u16,
    hwcf3: u16,
    hwcf4: u32,
    hwcf5: u32,
    hwcf6: u32,
    hwcf7: u32,
    /// The effects processor's INIT1-4 arrays.
    init: [[u16; 32]; 4],
    smalr: u32,
    smarr: u32,
    smalw: u32,
    smarw: u32,
    /// The words read ahead for SMLD and SMRD.
    smld_buffer: u16,
    smrd_buffer: u16,
    id: u16,
    reg: u8,
    voice: u8,
    /// The pointer register's high byte counts its reads.
    pointer_reads: u8,
    /// The frame (44.1 kHz, since the machine started) of the last reset,
    /// which the sample counter counts from.
    clock_base: u64,
    chorus: Chorus,
    reverb: Reverb,
    eq: Eq,
    /// INIT words changed: the reverb and equalizer settings to find again.
    effects_dirty: bool,
}

impl Emu8000 {
    /// The chip of a card at `sb_base`, as `AWEUTIL /S` leaves it.
    /// `now_frames` is the emulated time in 44.1 kHz frames.
    pub fn new(sb_base: u16, rom: Arc<[i16]>, ram_kb: u32, now_frames: u64) -> Self {
        let mut chip = Self {
            base: sb_base.wrapping_add(0x400),
            mem: Memory::new(rom, ram_kb),
            voices: std::array::from_fn(|_| Voice::new()),
            hwcf1: 0,
            hwcf2: 0,
            hwcf3: 0,
            hwcf4: 0,
            hwcf5: 0,
            hwcf6: 0,
            hwcf7: 0,
            init: [[0; 32]; 4],
            smalr: 0,
            smarr: 0,
            smalw: 0,
            smarw: 0,
            smld_buffer: 0,
            smrd_buffer: 0,
            id: 0,
            reg: 0,
            voice: 0,
            pointer_reads: 0,
            clock_base: now_frames,
            chorus: Chorus::default(),
            reverb: Reverb::default(),
            eq: Eq::default(),
            effects_dirty: false,
        };
        chip.power_on(now_frames);
        chip
    }

    /// Power-on state, then the initialisation `AWEUTIL /S` does. The RAM
    /// keeps what it holds.
    pub fn power_on(&mut self, now_frames: u64) {
        for v in self.voices.iter_mut() {
            *v = Voice::new();
        }
        self.hwcf1 = 0x59;
        self.hwcf2 = 0x20;
        self.hwcf3 = 0;
        (self.hwcf4, self.hwcf5, self.hwcf6, self.hwcf7) = (0, 0, 0, 0);
        self.init = [[0; 32]; 4];
        (self.smalr, self.smarr, self.smalw, self.smarw) = (0, 0, 0, 0);
        (self.smld_buffer, self.smrd_buffer) = (0, 0);
        self.id = 0;
        self.reg = 0;
        self.voice = 0;
        self.clock_base = now_frames;
        self.chorus = Chorus::default();
        self.reverb = Reverb::default();
        self.eq = Eq::default();
        self.effects_dirty = false;
        self.init();
    }

    /// Stop every voice, as when a program ends; everything else stays.
    pub fn silence(&mut self) {
        for v in self.voices.iter_mut() {
            v.silence();
        }
    }

    /// Replace the ROM (a new file configured).
    pub fn set_rom(&mut self, rom: Arc<[i16]>) {
        self.mem.rom = rom;
    }

    pub fn ram_kb(&self) -> u32 {
        self.mem.ram_kb()
    }

    /// Whether `port` is one of the chip's: base+0-3, +400h-403h, +800h-803h.
    #[inline]
    pub fn claims(&self, port: u16) -> bool {
        matches!(port.wrapping_sub(self.base) & !3, 0x000 | 0x400 | 0x800)
    }

    /// Whether an access only selects a register: the pointer, which the
    /// sound doesn't depend on.
    #[inline]
    pub fn latch_only(&self, port: u16) -> bool {
        port.wrapping_sub(self.base) & !1 == POINTER
    }

    /// Whether a read of `port` is the sample counter, which follows
    /// emulated time rather than rendered audio.
    #[inline]
    pub fn reads_clock(&self, port: u16) -> bool {
        port.wrapping_sub(self.base) & !1 == DATA2 && self.reg == 1 && self.voice == 27
    }

    /// A word read. `now_frames` is the emulated time in 44.1 kHz frames.
    #[inline]
    pub fn read_word(&mut self, port: u16, now_frames: u64) -> u16 {
        self.read16(port.wrapping_sub(self.base) & !1, now_frames)
    }

    /// A byte read: the low or the high byte of the word.
    #[inline]
    pub fn read_byte(&mut self, port: u16, now_frames: u64) -> u8 {
        let word = self.read_word(port, now_frames);
        if port & 1 != 0 { (word >> 8) as u8 } else { word as u8 }
    }

    #[inline]
    pub fn write_word(&mut self, port: u16, value: u16) {
        self.write16(port.wrapping_sub(self.base) & !1, value);
    }

    /// A byte write, which the chip takes as a word: the byte in the low
    /// or the high half, the other half 0. Programs write words.
    #[inline]
    pub fn write_byte(&mut self, port: u16, value: u8) {
        let word = if port & 1 != 0 { (value as u16) << 8 } else { value as u16 };
        self.write_word(port, word);
    }

    fn read16(&mut self, offset: u16, now_frames: u64) -> u16 {
        let v = self.voice as usize;
        let voice = &self.voices[v];
        let lo = |r: u32| r as u16;
        let hi = |r: u32| (r >> 16) as u16;
        match (offset, self.reg) {
            (DATA0 | DATA0_HI, reg) => {
                let r = match reg {
                    0 => voice.cpf,
                    1 => voice.ptrx,
                    2 => voice.cvcf,
                    3 => voice.vtft,
                    4 => voice.z2,
                    5 => voice.z1,
                    6 => voice.psst,
                    _ => voice.csl,
                };
                if offset == DATA0 { lo(r) } else { hi(r) }
            }
            (DATA1, 0) => lo(voice.ccca),
            (DATA2, 0) => hi(voice.ccca),
            (DATA1 | DATA2, 1) => self.read_global(offset == DATA2, now_frames),
            (DATA1, 2) => self.init[0][v],
            (DATA2, 2) => self.init[1][v],
            (DATA1, 3) => self.init[2][v],
            (DATA2, 3) => self.init[3][v],
            (DATA1, 4) => voice.envvol,
            (DATA1, 5) => voice.dcysusv,
            (DATA1, 6) => voice.envval,
            (DATA1, 7) => voice.dcysus,
            (DATA2, 4) => voice.atkhldv,
            (DATA2, 5) => voice.lfo1val,
            (DATA2, 6) => voice.atkhld,
            (DATA2, 7) => voice.lfo2val,
            (DATA3, 0) => voice.ip,
            (DATA3, 1) => voice.ifatn,
            (DATA3, 2) => voice.pefe,
            (DATA3, 3) => voice.fmmod,
            (DATA3, 4) => voice.tremfrq,
            (DATA3, 5) => voice.fm2frq2,
            (DATA3, 6) => 0xFFFF,
            // The chip's ID: exactly 000Ch, as Creative's AWEUTIL checks.
            (DATA3, _) => 0x000C | if self.id & 2 != 0 { 0xFF02 } else { 0 },
            _ => {
                // The pointer: the register and voice selected, and a
                // count of its reads in the high byte, whose change
                // detection code (Impulse Tracker, Cubic Player) looks for.
                self.pointer_reads = (self.pointer_reads + 1) & 0x1F;
                (0x80 | self.pointer_reads as u16) << 8 | (self.reg as u16) << 5 | self.voice as u16
            }
        }
    }

    /// Register 1 of DATA1 (`high` false) or DATA2: the chip's global
    /// registers, by the voice field.
    fn read_global(&mut self, high: bool, now_frames: u64) -> u16 {
        let half = |r: u32| if high { (r >> 16) as u16 } else { r as u16 };
        match (self.voice, high) {
            (9, _) => half(self.hwcf4),
            (10, _) => half(self.hwcf5),
            (13, _) => half(self.hwcf6),
            (14, _) => half(self.hwcf7),
            // Never busy: memory access is immediate here.
            (20, _) => half(self.smalr),
            (21, _) => half(self.smarr),
            (22, _) => half(self.smalw),
            (23, _) => half(self.smarw),
            // The data ports return the word read ahead and fetch the
            // next, which is why drivers throw the first read away.
            (26, false) => {
                let value = self.smld_buffer;
                self.smld_buffer = self.mem.word(self.smalr) as u16;
                self.smalr = (self.smalr + 1) & ADDRESS_MASK;
                value
            }
            (26, true) => {
                let value = self.smrd_buffer;
                self.smrd_buffer = self.mem.word(self.smarr) as u16;
                self.smarr = (self.smarr + 1) & ADDRESS_MASK;
                value
            }
            // The sample counter, 44.1 kHz.
            (27, true) => now_frames.wrapping_sub(self.clock_base) as u16,
            // The configuration words read back scrambled ("a VLSI
            // error", says the programmer's guide), which drivers use to
            // find the chip.
            (29, false) => (self.hwcf1 & 0xFE) | (self.hwcf3 & 0x01),
            (30, false) => {
                let h3 = self.hwcf3;
                ((self.hwcf2 >> 4) & 0x0E)
                    | (self.hwcf1 & 0x01)
                    | if h3 & 0x02 != 0 { 0x10 } else { 0 }
                    | if h3 & 0x04 != 0 { 0x40 } else { 0 }
                    | if h3 & 0x08 != 0 { 0x20 } else { 0 }
                    | if h3 & 0x10 != 0 { 0x80 } else { 0 }
            }
            (31, false) => self.hwcf2 & 0x1F,
            _ => 0xFFFF,
        }
    }

    fn write16(&mut self, offset: u16, value: u16) {
        let v = self.voice as usize;
        let set_lo = |r: &mut u32| *r = (*r & 0xFFFF_0000) | value as u32;
        let set_hi = |r: &mut u32| *r = (*r & 0xFFFF) | (value as u32) << 16;
        let set = |r: &mut u32, high: bool| if high { set_hi(r) } else { set_lo(r) };
        // Effect parameters take effect once the first two INIT passes are
        // over, which leave 03FFh in INIT1[0].
        let effects_on = self.init[0][0] != 0x03FF;
        match (offset, self.reg) {
            (DATA0 | DATA0_HI, reg) => {
                let high = offset == DATA0_HI;
                let voice = &mut self.voices[v];
                match reg {
                    0 => set(&mut voice.cpf, high),
                    1 => set(&mut voice.ptrx, high),
                    2 => set(&mut voice.cvcf, high),
                    3 => set(&mut voice.vtft, high),
                    4 => set(&mut voice.z2, high),
                    5 => set(&mut voice.z1, high),
                    6 => {
                        set(&mut voice.psst, high);
                        voice.write_psst(high);
                    }
                    _ => {
                        set(&mut voice.csl, high);
                        voice.write_csl();
                    }
                }
            }
            (DATA1 | DATA2, 0) => {
                let high = offset == DATA2;
                let voice = &mut self.voices[v];
                set(&mut voice.ccca, high);
                voice.write_ccca(high);
            }
            (DATA1 | DATA2, 1) => self.write_global(offset == DATA2, value, effects_on),
            (DATA1 | DATA2, 2 | 3) => {
                let array = (self.reg as usize - 2) * 2 + (offset == DATA2) as usize;
                self.init[array][v] = value;
                self.effects_dirty = true;
                if effects_on {
                    match (array, v) {
                        (2, 9) => self.chorus.feedback = (value & 0xFF) as i32,
                        (2, 12) => self.chorus.delay = (value & 0x1FFF) as i32,
                        // The delay swings by the low byte, in samples.
                        (3, 3) => self.chorus.depth = (value & 0xFF) as f64,
                        _ => {}
                    }
                }
            }
            (DATA1, 4) => self.voices[v].write_envvol(value),
            (DATA1, 5) => {
                // Doom and Cubic Player 1.7 play without initialising the
                // chip: a note unmutes it.
                if self.voices[v].write_dcysusv(value) && self.hwcf3 != 0x04 {
                    self.hwcf3 = 0x04;
                }
            }
            (DATA1, 6) => self.voices[v].write_envval(value),
            (DATA1, 7) => self.voices[v].write_dcysus(value),
            (DATA2, 4) => self.voices[v].write_atkhldv(value),
            (DATA2, 5) => self.voices[v].write_lfo1val(value),
            (DATA2, 6) => self.voices[v].write_atkhld(value),
            (DATA2, 7) => self.voices[v].write_lfo2val(value),
            (DATA3, 0) => self.voices[v].write_ip(value),
            (DATA3, 1) => self.voices[v].write_ifatn(value),
            (DATA3, 2) => self.voices[v].write_pefe(value),
            (DATA3, 3) => self.voices[v].write_fmmod(value),
            (DATA3, 4) => self.voices[v].write_tremfrq(value),
            (DATA3, 5) => self.voices[v].write_fm2frq2(value),
            (DATA3, 7) => self.id = value,
            (DATA3, _) => {}
            _ => {
                self.voice = (value & 0x1F) as u8;
                self.reg = ((value >> 5) & 7) as u8;
            }
        }
    }

    fn write_global(&mut self, high: bool, value: u16, effects_on: bool) {
        let set = |r: &mut u32| {
            *r = if high { (*r & 0xFFFF) | (value as u32) << 16 } else { (*r & 0xFFFF_0000) | value as u32 }
        };
        // The address registers' top byte is status, not address.
        let set_address = |r: &mut u32| {
            *r = if high { (*r & 0xFFFF) | ((value & 0xFF) as u32) << 16 } else { (*r & 0xFFFF_0000) | value as u32 }
        };
        match (self.voice, high) {
            (9, _) => {
                set(&mut self.hwcf4);
                if high && effects_on {
                    // The right chorus tap's offset, in 1/256 samples.
                    self.chorus.right_offset = (self.hwcf4 & 0x1F_FFFF) as f64 / 256.0;
                }
            }
            (10, _) => {
                set(&mut self.hwcf5);
                if high && effects_on {
                    // The chorus LFO: HWCF5 / 2^24 of a period a sample.
                    self.chorus.lfo_inc = (self.hwcf5 as u64) << 24;
                }
            }
            (13, _) => set(&mut self.hwcf6),
            (14, _) => set(&mut self.hwcf7),
            (20, _) => set_address(&mut self.smalr),
            (21, _) => set_address(&mut self.smarr),
            (22, _) => set_address(&mut self.smalw),
            (23, _) => set_address(&mut self.smarw),
            (26, false) => {
                self.mem.write(self.smalw, value);
                self.smalw = (self.smalw + 1) & ADDRESS_MASK;
            }
            (26, true) => {
                self.mem.write(self.smarw, value);
                self.smarw = (self.smarw + 1) & ADDRESS_MASK;
            }
            (29, false) => self.hwcf1 = value,
            (30, false) => self.hwcf2 = value,
            (31, false) => self.hwcf3 = value,
            _ => {}
        }
    }

    /// One frame of the chip's output, at 44.1 kHz.
    #[inline]
    pub fn render(&mut self) -> (f32, f32) {
        if self.effects_dirty {
            self.effects_dirty = false;
            let init = Init { init: [&self.init[0], &self.init[1], &self.init[2], &self.init[3]] };
            self.reverb.decode(&init);
            self.eq.decode(&init);
        }
        let unmuted = self.hwcf3 & 0x04 != 0;
        let mut sums = Sums::default();
        for voice in self.voices.iter_mut() {
            voice.step(&self.mem, unmuted, &mut sums);
        }
        // The effects always run, like on the chip.
        let (rl, rr) = self.reverb.run(sums.reverb);
        let (cl, cr) = self.chorus.run(sums.chorus);
        let [l, r] = self.eq.run([sums.left + rl + cl, sums.right + rr + cr]);
        (((l as i64 * OUTPUT_GAIN) >> 16) as f32, ((r as i64 * OUTPUT_GAIN) >> 16) as f32)
    }

    // The initialisation of the programmer's guide, as AWEUTIL /S and the
    // drivers do it, through the registers.

    fn select(&mut self, reg: u16, voice: u16) {
        self.write16(POINTER, reg << 5 | voice);
    }

    fn poke(&mut self, port: u16, reg: u16, voice: u16, value: u16) {
        self.select(reg, voice);
        self.write16(port, value);
    }

    /// A doubleword register: low word at `port`, high word at `port` + 2.
    fn poke_dw(&mut self, port: u16, reg: u16, voice: u16, value: u32) {
        self.select(reg, voice);
        self.write16(port, value as u16);
        self.write16(port + 2, (value >> 16) as u16);
    }

    fn init(&mut self) {
        // Configuration, with the audio muted meanwhile.
        self.poke(DATA1, 1, 29, 0x0059);
        self.poke(DATA1, 1, 30, 0x0020);
        self.poke(DATA1, 1, 31, 0x0000);

        // Every voice: envelope engine off, then its registers cleared.
        for ch in 0..32 {
            self.poke(DATA1, 5, ch, 0x0080);
        }
        for ch in 0..32 {
            for (port, reg) in [(DATA1, 4), (DATA1, 6), (DATA1, 7), (DATA2, 4), (DATA2, 5), (DATA2, 6), (DATA2, 7)] {
                self.poke(port, reg, ch, 0);
            }
            for reg in 0..6 {
                self.poke(DATA3, reg, ch, 0);
            }
            for reg in [1, 3, 6, 7] {
                self.poke_dw(DATA0, reg, ch, 0);
            }
            self.poke_dw(DATA1, 0, ch, 0);
        }
        for ch in 0..32 {
            self.poke_dw(DATA0, 0, ch, 0);
            self.poke_dw(DATA0, 2, ch, 0);
        }

        // Sample memory addresses.
        for voice in 20..24 {
            self.poke_dw(DATA1, 1, voice, 0);
        }

        // The effects processor's arrays, in four passes.
        self.send_array(&INIT_ARRAYS[0]);
        self.send_array(&INIT_ARRAYS[1]);
        self.send_array(&INIT_ARRAYS[2]);
        self.poke_dw(DATA1, 1, 9, 0);
        self.poke_dw(DATA1, 1, 10, 0x83);
        self.poke_dw(DATA1, 1, 13, 0x8000);
        self.send_array(&INIT_ARRAYS[3]);

        // Voices 30 and 31 refresh the DRAM and carry the FM synthesizer's
        // effects: they loop over the FM area at the top of memory.
        self.poke(DATA1, 5, 30, 0x0080);
        self.poke_dw(DATA0, 6, 30, 0xFFFF_FFE0);
        self.poke_dw(DATA0, 7, 30, 0x00FF_FFE8);
        self.poke_dw(DATA0, 1, 30, 0);
        self.poke_dw(DATA0, 0, 30, 0);
        self.poke_dw(DATA1, 0, 30, 0x00FF_FFE3);
        self.poke(DATA1, 5, 31, 0x0080);
        self.poke_dw(DATA0, 6, 31, 0x00FF_FFF0);
        self.poke_dw(DATA0, 7, 31, 0x00FF_FFF8);
        self.poke_dw(DATA0, 1, 31, 0);
        self.poke_dw(DATA0, 0, 31, 0x8000);
        self.poke_dw(DATA1, 0, 31, 0x00FF_FFF3);
        self.poke(DATA0, 1, 30, 0x4828);
        self.poke(DATA1, 1, 28, 0);
        self.poke_dw(DATA0, 3, 30, 0x8000_FFFF);
        self.poke_dw(DATA0, 3, 31, 0x8000_FFFF);

        // Voices off, and the audio on.
        for ch in 0..30 {
            self.poke(DATA1, 5, ch, 0x807F);
        }
        self.poke(DATA1, 1, 31, 0x0004);

        // The default equalizer, chorus 3 and hall 2.
        let bass = effects::BASS_WORDS[effects::DEFAULT_BASS];
        let treble = effects::TREBLE_WORDS[effects::DEFAULT_TREBLE];
        self.poke(DATA2, 3, 0x01, bass[0]);
        self.poke(DATA2, 3, 0x11, bass[1]);
        self.poke(DATA1, 3, 0x11, treble[0]);
        self.poke(DATA1, 3, 0x13, treble[1]);
        self.poke(DATA1, 3, 0x1B, treble[2]);
        self.poke(DATA2, 3, 0x07, treble[3]);
        self.poke(DATA2, 3, 0x0B, treble[4]);
        self.poke(DATA2, 3, 0x0D, treble[5]);
        self.poke(DATA2, 3, 0x17, treble[6]);
        self.poke(DATA2, 3, 0x19, treble[7]);
        // The sum of the positions' last words: 1 for bass 5, 2 for treble 9.
        let w: u16 = 1 + 2;
        self.poke(DATA2, 3, 0x15, w + 0x0262);
        self.poke(DATA2, 3, 0x1D, w + 0x8362);

        self.poke(DATA1, 3, 0x09, 0xE610);
        self.poke(DATA1, 3, 0x0C, 0x031A);
        self.poke(DATA2, 3, 0x03, 0xBC84);
        self.poke_dw(DATA1, 1, 9, 0);
        self.poke_dw(DATA1, 1, 10, 0x83);
        self.poke_dw(DATA1, 1, 13, 0x8000);
        self.poke_dw(DATA1, 1, 14, 0);

        let words = effects::REVERB_PRESETS[effects::DEFAULT_REVERB];
        for (&(array, slot), &word) in REVERB_WRITES.iter().zip(&words) {
            let (port, reg) = match array {
                1 => (DATA1, 2),
                2 => (DATA2, 2),
                3 => (DATA1, 3),
                _ => (DATA2, 3),
            };
            self.poke(port, reg, slot, word);
        }
        self.select(0, 0);
    }

    /// An INIT pass: 32 words to each of INIT1 to INIT4.
    fn send_array(&mut self, data: &[u16; 128]) {
        for (i, &word) in data.iter().enumerate() {
            let (port, reg) = [(DATA1, 2), (DATA2, 2), (DATA1, 3), (DATA2, 3)][i / 32];
            self.poke(port, reg, (i % 32) as u16, word);
        }
    }

    /// The chip's state for the debugger.
    pub fn snapshot(&self) -> serde_json::Value {
        let voices: Vec<_> = self
            .voices
            .iter()
            .enumerate()
            .filter(|(_, v)| v.engine_on || v.cur_volume() != 0)
            .map(|(i, v)| {
                serde_json::json!({
                    "voice": i,
                    "addr": format!("{:06X}", v.addr >> 32),
                    "loop": format!("{:06X}-{:06X}", v.loop_start, v.loop_end),
                    "ip": format!("{:04X}", v.ip),
                    "ifatn": format!("{:04X}", v.ifatn),
                    "volume": v.cur_volume(),
                    "vol_env": format!("{:?}", v.vol_env.stage),
                    "mod_env": format!("{:?}", v.mod_env.stage),
                    "dcysusv": format!("{:04X}", v.dcysusv),
                })
            })
            .collect();
        serde_json::json!({
            "base": format!("{:03X}", self.base),
            "rom": !self.mem.rom.is_empty(),
            "ram_kb": self.ram_kb(),
            "hwcf": format!("{:04X} {:04X} {:04X}", self.hwcf1, self.hwcf2, self.hwcf3),
            "pointer": format!("reg {} voice {}", self.reg, self.voice),
            "reverb_preset": self.reverb.preset,
            "chorus": { "feedback": self.chorus.feedback, "delay": self.chorus.delay, "depth": self.chorus.depth },
            "eq": { "bass": self.eq.bass, "treble": self.eq.treble },
            "voices": voices,
        })
    }

    /// After a state was loaded: the effects' settings and lines.
    pub fn after_load(&mut self) {
        self.mem.ram_end = RAM_START + (self.mem.ram.len() / 2) as u32;
        self.effects_dirty = true;
        self.reverb.after_load();
        self.eq.after_load();
        self.chorus.clear();
    }
}

/// Where the reverb words go, as `effects::REVERB_SLOTS`.
const REVERB_WRITES: [(u8, u16); 28] = [
    (1, 0x03), (1, 0x05), (4, 0x1F), (1, 0x07), (2, 0x14), (2, 0x16), (1, 0x0F),
    (1, 0x17), (1, 0x1F), (2, 0x07), (2, 0x0F), (2, 0x17), (2, 0x1D), (2, 0x1F),
    (3, 0x01), (3, 0x03), (1, 0x09), (1, 0x0B), (1, 0x11), (1, 0x13), (1, 0x19),
    (1, 0x1B), (2, 0x01), (2, 0x03), (2, 0x09), (2, 0x0B), (2, 0x11), (2, 0x13),
];

/// The four passes of INIT words from E-mu's programmer's guide (ADIP).
const INIT_ARRAYS: [[u16; 128]; 4] = [
    [
        0x03ff, 0x0030, 0x07ff, 0x0130, 0x0bff, 0x0230, 0x0fff, 0x0330, 0x13ff, 0x0430, 0x17ff, 0x0530, 0x1bff, 0x0630,
        0x1fff, 0x0730, 0x23ff, 0x0830, 0x27ff, 0x0930, 0x2bff, 0x0a30, 0x2fff, 0x0b30, 0x33ff, 0x0c30, 0x37ff, 0x0d30,
        0x3bff, 0x0e30, 0x3fff, 0x0f30, 0x43ff, 0x0030, 0x47ff, 0x0130, 0x4bff, 0x0230, 0x4fff, 0x0330, 0x53ff, 0x0430,
        0x57ff, 0x0530, 0x5bff, 0x0630, 0x5fff, 0x0730, 0x63ff, 0x0830, 0x67ff, 0x0930, 0x6bff, 0x0a30, 0x6fff, 0x0b30,
        0x73ff, 0x0c30, 0x77ff, 0x0d30, 0x7bff, 0x0e30, 0x7fff, 0x0f30, 0x83ff, 0x0030, 0x87ff, 0x0130, 0x8bff, 0x0230,
        0x8fff, 0x0330, 0x93ff, 0x0430, 0x97ff, 0x0530, 0x9bff, 0x0630, 0x9fff, 0x0730, 0xa3ff, 0x0830, 0xa7ff, 0x0930,
        0xabff, 0x0a30, 0xafff, 0x0b30, 0xb3ff, 0x0c30, 0xb7ff, 0x0d30, 0xbbff, 0x0e30, 0xbfff, 0x0f30, 0xc3ff, 0x0030,
        0xc7ff, 0x0130, 0xcbff, 0x0230, 0xcfff, 0x0330, 0xd3ff, 0x0430, 0xd7ff, 0x0530, 0xdbff, 0x0630, 0xdfff, 0x0730,
        0xe3ff, 0x0830, 0xe7ff, 0x0930, 0xebff, 0x0a30, 0xefff, 0x0b30, 0xf3ff, 0x0c30, 0xf7ff, 0x0d30, 0xfbff, 0x0e30,
        0xffff, 0x0f30,
    ],
    [
        0x03ff, 0x8030, 0x07ff, 0x8130, 0x0bff, 0x8230, 0x0fff, 0x8330, 0x13ff, 0x8430, 0x17ff, 0x8530, 0x1bff, 0x8630,
        0x1fff, 0x8730, 0x23ff, 0x8830, 0x27ff, 0x8930, 0x2bff, 0x8a30, 0x2fff, 0x8b30, 0x33ff, 0x8c30, 0x37ff, 0x8d30,
        0x3bff, 0x8e30, 0x3fff, 0x8f30, 0x43ff, 0x8030, 0x47ff, 0x8130, 0x4bff, 0x8230, 0x4fff, 0x8330, 0x53ff, 0x8430,
        0x57ff, 0x8530, 0x5bff, 0x8630, 0x5fff, 0x8730, 0x63ff, 0x8830, 0x67ff, 0x8930, 0x6bff, 0x8a30, 0x6fff, 0x8b30,
        0x73ff, 0x8c30, 0x77ff, 0x8d30, 0x7bff, 0x8e30, 0x7fff, 0x8f30, 0x83ff, 0x8030, 0x87ff, 0x8130, 0x8bff, 0x8230,
        0x8fff, 0x8330, 0x93ff, 0x8430, 0x97ff, 0x8530, 0x9bff, 0x8630, 0x9fff, 0x8730, 0xa3ff, 0x8830, 0xa7ff, 0x8930,
        0xabff, 0x8a30, 0xafff, 0x8b30, 0xb3ff, 0x8c30, 0xb7ff, 0x8d30, 0xbbff, 0x8e30, 0xbfff, 0x8f30, 0xc3ff, 0x8030,
        0xc7ff, 0x8130, 0xcbff, 0x8230, 0xcfff, 0x8330, 0xd3ff, 0x8430, 0xd7ff, 0x8530, 0xdbff, 0x8630, 0xdfff, 0x8730,
        0xe3ff, 0x8830, 0xe7ff, 0x8930, 0xebff, 0x8a30, 0xefff, 0x8b30, 0xf3ff, 0x8c30, 0xf7ff, 0x8d30, 0xfbff, 0x8e30,
        0xffff, 0x8f30,
    ],
    [
        0x0C10, 0x8470, 0x14FE, 0xB488, 0x167F, 0xA470, 0x18E7, 0x84B5, 0x1B6E, 0x842A, 0x1F1D, 0x852A, 0x0DA3, 0x8F7C,
        0x167E, 0xF254, 0x0000, 0x842A, 0x0001, 0x852A, 0x18E6, 0x8BAA, 0x1B6D, 0xF234, 0x229F, 0x8429, 0x2746, 0x8529,
        0x1F1C, 0x86E7, 0x229E, 0xF224, 0x0DA4, 0x8429, 0x2C29, 0x8529, 0x2745, 0x87F6, 0x2C28, 0xF254, 0x383B, 0x8428,
        0x320F, 0x8528, 0x320E, 0x8F02, 0x1341, 0xF264, 0x3EB6, 0x8428, 0x3EB9, 0x8528, 0x383A, 0x8FA9, 0x3EB5, 0xF294,
        0x3EB7, 0x8474, 0x3EBA, 0x8575, 0x3EB8, 0xC4C3, 0x3EBB, 0xC5C3, 0x0000, 0xA404, 0x0001, 0xA504, 0x141F, 0x8671,
        0x14FD, 0x8287, 0x3EBC, 0xE610, 0x3EC8, 0x8C7B, 0x031A, 0x87E6, 0x3EC8, 0x86F7, 0x3EC0, 0x821E, 0x3EBE, 0xD208,
        0x3EBD, 0x821F, 0x3ECA, 0x8386, 0x3EC1, 0x8C03, 0x3EC9, 0x831E, 0x3ECA, 0x8C4C, 0x3EBF, 0x8C55, 0x3EC9, 0xC208,
        0x3EC4, 0xBC84, 0x3EC8, 0x8EAD, 0x3EC8, 0xD308, 0x3EC2, 0x8F7E, 0x3ECB, 0x8219, 0x3ECB, 0xD26E, 0x3EC5, 0x831F,
        0x3EC6, 0xC308, 0x3EC3, 0xB2FF, 0x3EC9, 0x8265, 0x3EC9, 0x8319, 0x1342, 0xD36E, 0x3EC7, 0xB3FF, 0x0000, 0x8365,
        0x1420, 0x9570,
    ],
    [
        0x0C10, 0x8470, 0x14FE, 0xB488, 0x167F, 0xA470, 0x18E7, 0x84B5, 0x1B6E, 0x842A, 0x1F1D, 0x852A, 0x0DA3, 0x0F7C,
        0x167E, 0x7254, 0x0000, 0x842A, 0x0001, 0x852A, 0x18E6, 0x0BAA, 0x1B6D, 0x7234, 0x229F, 0x8429, 0x2746, 0x8529,
        0x1F1C, 0x06E7, 0x229E, 0x7224, 0x0DA4, 0x8429, 0x2C29, 0x8529, 0x2745, 0x07F6, 0x2C28, 0x7254, 0x383B, 0x8428,
        0x320F, 0x8528, 0x320E, 0x0F02, 0x1341, 0x7264, 0x3EB6, 0x8428, 0x3EB9, 0x8528, 0x383A, 0x0FA9, 0x3EB5, 0x7294,
        0x3EB7, 0x8474, 0x3EBA, 0x8575, 0x3EB8, 0x44C3, 0x3EBB, 0x45C3, 0x0000, 0xA404, 0x0001, 0xA504, 0x141F, 0x0671,
        0x14FD, 0x0287, 0x3EBC, 0xE610, 0x3EC8, 0x0C7B, 0x031A, 0x07E6, 0x3EC8, 0x86F7, 0x3EC0, 0x821E, 0x3EBE, 0xD208,
        0x3EBD, 0x021F, 0x3ECA, 0x0386, 0x3EC1, 0x0C03, 0x3EC9, 0x031E, 0x3ECA, 0x8C4C, 0x3EBF, 0x0C55, 0x3EC9, 0xC208,
        0x3EC4, 0xBC84, 0x3EC8, 0x0EAD, 0x3EC8, 0xD308, 0x3EC2, 0x8F7E, 0x3ECB, 0x0219, 0x3ECB, 0xD26E, 0x3EC5, 0x031F,
        0x3EC6, 0xC308, 0x3EC3, 0x32FF, 0x3EC9, 0x0265, 0x3EC9, 0x8319, 0x1342, 0xD36E, 0x3EC7, 0x33FF, 0x0000, 0x8365,
        0x1420, 0x9570,
    ],
];

rust_dos_savestate::state_fields!(Memory { ram } skip {
    // The ROM comes from its file, and the end from the RAM's size.
    rom, ram_end,
});
rust_dos_savestate::state_fields!(Emu8000 {
    mem, voices, hwcf1, hwcf2, hwcf3, hwcf4, hwcf5, hwcf6, hwcf7, init, smalr, smarr, smalw, smarw, smld_buffer,
    smrd_buffer, id, reg, voice, pointer_reads, clock_base, chorus, reverb, eq,
} skip {
    // The configuration's, and worked out again after a load.
    base, effects_dirty,
});
