//! Audio synthesis on the emulation thread: what one video frame's worth of
//! sound (a 60th of a second) costs the chips that render it a sample at a
//! time.
//!
//! The AWE32 scenarios play all 32 voices from a looped sample in the
//! card's RAM, as a General MIDI piece does through a SoundFont driver,
//! with the voices' resonant filter set the ways SoundFont presets set it.

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use rust_dos::awe32::Emu8000;
use rust_dos::bus::Bus;
use rust_dos::sb::{SbConfig, SbModel};
use std::hint::black_box;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Frames of one 60 Hz video frame at 44.1 kHz.
const FRAMES: usize = 735;

// The chip's ports, for a card at 220h.
const DATA0: u16 = 0x620;
const DATA1: u16 = 0xA20;
const DATA2: u16 = 0xA22;
const DATA3: u16 = 0xE20;
const POINTER: u16 = 0xE22;
/// Where the sample goes: the start of RAM, plus the guard words drivers
/// leave before a sample.
const SAMPLE: u32 = 0x20_0010;
const SAMPLE_WORDS: u32 = 4096;

/// Word writes to the chip's ports: on the chip itself, or through the
/// machine's bus as a driver makes them.
trait Ports {
    fn out(&mut self, port: u16, value: u16);
}

impl Ports for Emu8000 {
    fn out(&mut self, port: u16, value: u16) {
        self.write_word(port, value);
    }
}

impl Ports for Bus {
    fn out(&mut self, port: u16, value: u16) {
        self.io_write_wide(port, value as u32, 2);
    }
}

fn select(chip: &mut impl Ports, reg: u16, voice: u16) {
    chip.out(POINTER, reg << 5 | voice);
}

fn write16(chip: &mut impl Ports, port: u16, reg: u16, voice: u16, value: u16) {
    select(chip, reg, voice);
    chip.out(port, value);
}

fn write32(chip: &mut impl Ports, port: u16, reg: u16, voice: u16, value: u32) {
    select(chip, reg, voice);
    chip.out(port, value as u16);
    chip.out(port + 2, (value >> 16) as u16);
}

/// How a preset sets a voice's filter and its modulation.
#[derive(Clone, Copy)]
struct Preset {
    name: &'static str,
    /// IFATN's high byte: the initial cutoff (FFh open).
    cutoff: u8,
    /// The filter's Q, 0 to 15.
    q: u8,
    /// PEFE's low byte: the modulation envelope's depth on the cutoff.
    env_to_filter: u8,
    /// FMMOD's low byte: LFO1's depth on the cutoff.
    lfo_to_filter: u8,
}

const PRESETS: [Preset; 4] = [
    // Most of the ROM's General MIDI presets: the filter open, no Q.
    Preset { name: "open", cutoff: 0xFF, q: 0, env_to_filter: 0, lfo_to_filter: 0 },
    // A fixed low-pass: piano, strings.
    Preset { name: "static", cutoff: 0x90, q: 4, env_to_filter: 0, lfo_to_filter: 0 },
    // A filter envelope sweeping the cutoff: brass, synth bass, plucks.
    Preset { name: "filter_env", cutoff: 0x60, q: 6, env_to_filter: 0x50, lfo_to_filter: 0 },
    // LFO1 wobbling the cutoff: pads and synth leads.
    Preset { name: "filter_lfo", cutoff: 0x80, q: 6, env_to_filter: 0, lfo_to_filter: 0x30 },
];

/// A sawtooth in RAM and every voice set up from `preset`, its engine off.
fn set_up(chip: &mut impl Ports, preset: Preset) {
    // The sample, through SMALW and SMLD.
    write32(chip, DATA1, 1, 22, SAMPLE);
    select(chip, 1, 26);
    for i in 0..SAMPLE_WORDS {
        let saw = ((i % 128) as i32 * 512 - 32768) as i16;
        chip.out(DATA1, saw as u16);
    }
    for v in 0..32 {
        write16(chip, DATA1, 5, v, 0x0080);
        // Volume envelope: fast attack, sustain near full.
        write16(chip, DATA1, 4, v, 0x8000);
        write16(chip, DATA2, 4, v, 0x7F7F);
        // Modulation envelope: a 90 ms attack, then a 2 s decay, so the
        // cutoff moves all through a measurement.
        write16(chip, DATA1, 6, v, 0x8000);
        write16(chip, DATA2, 6, v, 0x7F40);
        write16(chip, DATA1, 7, v, 0x2010);
        // LFO1 at about 5 Hz, no delay.
        write16(chip, DATA2, 5, v, 0x8000);
        write16(chip, DATA3, 4, v, 0x0077);
        // Pitches spread over two octaves.
        write16(chip, DATA3, 0, v, 0xD000 + (v * 0x100));
        write16(chip, DATA3, 1, v, (preset.cutoff as u16) << 8 | 0x10);
        write16(chip, DATA3, 2, v, preset.env_to_filter as u16);
        write16(chip, DATA3, 3, v, preset.lfo_to_filter as u16);
        let pan = (v as u32 * 8) << 24;
        write32(chip, DATA0, 6, v, pan | (SAMPLE - 1));
        write32(chip, DATA0, 7, v, SAMPLE + SAMPLE_WORDS - 1);
        write32(chip, DATA1, 0, v, (preset.q as u32) << 28 | (SAMPLE - 1));
    }
}

/// Note-on for all 32 voices: the engine off, then on, which starts both
/// envelopes.
fn notes_on(chip: &mut impl Ports) {
    for v in 0..32 {
        write16(chip, DATA1, 5, v, 0x0080);
        write16(chip, DATA1, 0, v, (SAMPLE - 1) as u16);
        write16(chip, DATA1, 5, v, 0x7F7F);
    }
}

/// `iters` video frames from `frame`, timed. The notes start again before
/// each Criterion sample and run past the volume attack first, so every
/// sample measures notes 70 ms to a few seconds old.
fn time_frames<C: Ports>(chip: &mut C, iters: u64, frame: impl Fn(&mut C) -> f32) -> Duration {
    notes_on(chip);
    for _ in 0..4 {
        frame(chip);
    }
    let start = Instant::now();
    for _ in 0..iters {
        black_box(frame(chip));
    }
    start.elapsed()
}

/// One video frame of 32 voices with each filter preset, rendered by the
/// chip alone.
fn awe32_voices(c: &mut Criterion) {
    let render = |chip: &mut Emu8000| {
        let mut sum = 0.0;
        for _ in 0..FRAMES {
            let (l, r) = chip.render();
            sum += l + r;
        }
        sum
    };
    let mut group = c.benchmark_group("audio/awe32_32_voices");
    for preset in PRESETS {
        let mut chip = Emu8000::new(0x220, Arc::from([]), 512, 0);
        set_up(&mut chip, preset);
        // A silent chip would time nothing worth measuring.
        notes_on(&mut chip);
        let sounds = (0..FRAMES).map(|_| chip.render()).any(|(l, r)| l != 0.0 || r != 0.0);
        assert!(sounds, "{}: the voices are silent", preset.name);
        group.bench_with_input(BenchmarkId::from_parameter(preset.name), &preset, |b, _| {
            b.iter_custom(|iters| time_frames(&mut chip, iters, render))
        });
    }
    group.finish();
}

/// The same through the whole machine: a bus with an AWE32 at 220h, the
/// driver's port writes, and each iteration a 60th of a second of emulated
/// time mixed into the output queue, as the emulation thread does between
/// two video frames.
fn awe32_machine(c: &mut Criterion) {
    let frame = |bus: &mut Bus| {
        bus.clock.icount += 1000 * 1000 / 60;
        if bus.clock.icount >= bus.clock.deadline {
            bus.service_timers();
        }
        bus.audio_catch_up();
        let n = bus.audio_out.len();
        bus.audio_out.clear();
        n as f32
    };
    let mut group = c.benchmark_group("audio/awe32_machine");
    for preset in PRESETS {
        let mut bus = Bus::new(PathBuf::from("."));
        bus.set_cycles_per_ms(1000);
        bus.set_awe_setup(Arc::from([]), 512);
        bus.configure_sound(Some(SbConfig { model: SbModel::Awe32, ..SbConfig::default() }), true);
        set_up(&mut bus, preset);
        notes_on(&mut bus);
        bus.clock.icount += 1000 * 1000 / 60;
        if bus.clock.icount >= bus.clock.deadline {
            bus.service_timers();
        }
        bus.audio_catch_up();
        assert!(bus.audio_out.iter().any(|&s| s != 0), "{}: the voices are silent", preset.name);
        bus.audio_out.clear();
        group.bench_with_input(BenchmarkId::from_parameter(preset.name), &preset, |b, _| {
            b.iter_custom(|iters| time_frames(&mut bus, iters, frame))
        });
    }
    group.finish();
}

criterion_group!(benches, awe32_voices, awe32_machine);
criterion_main!(benches);
