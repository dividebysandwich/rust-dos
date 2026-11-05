//! munt's MT-32 on the MPU-401. These run only where munt's library is
//! installed and MT-32 ROMs are found (in `RUST_DOS_MT32_ROMS`, or the
//! places `mt32::default_rom_dirs` lists); elsewhere they pass saying so.
#![cfg(not(target_arch = "wasm32"))]

use rust_dos::bus::Bus;
use rust_dos::mt32::{Mt32, Mt32Model};
use std::path::PathBuf;

/// A bus at 1000 instructions per emulated ms with an MT-32 on the MPU-401,
/// or None without munt or ROMs.
fn bus_with_mt32() -> Option<Bus> {
    let roms = std::env::var_os("RUST_DOS_MT32_ROMS").map(PathBuf::from);
    let synth = match Mt32::open(roms.as_deref(), Mt32Model::Auto, None, rust_dos::opl::RATE) {
        Ok(synth) => synth,
        Err(e) => {
            eprintln!("skipped, no MT-32: {}", e);
            return None;
        }
    };
    let mut bus = Bus::new(PathBuf::from("."));
    bus.set_cycles_per_ms(1000);
    let what = bus.mpu.load_mt32(synth);
    eprintln!("{}", what);
    Some(bus)
}

fn midi(bus: &mut Bus, bytes: &[u8]) {
    for &b in bytes {
        bus.io_write(0x330, b);
    }
}

/// The loudest sample of the next `ms` milliseconds, rendered in steps
/// short enough that the mixer doesn't skip ahead.
fn peak(bus: &mut Bus, ms: u64) -> u16 {
    let mut loudest = 0;
    for _ in 0..ms.div_ceil(100) {
        bus.clock.icount += 100 * 1000;
        bus.audio_catch_up();
        loudest = bus.audio_out.drain(..).map(|s| s.unsigned_abs()).max().unwrap_or(0).max(loudest);
    }
    loudest
}

#[test]
fn a_note_plays() {
    let Some(mut bus) = bus_with_mt32() else { return };
    // UART mode, as games put the MPU-401 in.
    bus.io_write(0x331, 0x3F);
    // Let the synthesizer settle after its start.
    peak(&mut bus, 300);
    assert!(peak(&mut bus, 100) < 64, "silent before a note");

    // Part 1 (channel 2) with the acoustic piano, middle C.
    midi(&mut bus, &[0xC1, 0x00, 0x91, 0x3C, 0x7F]);
    let loud = peak(&mut bus, 300);
    assert!(loud > 1000, "the note is heard: {}", loud);

    // A program ending silences it.
    bus.mpu.reset();
    peak(&mut bus, 2000);
    assert!(peak(&mut bus, 100) < 64, "silent after the reset");
}

#[test]
fn the_display_shows_what_games_send() {
    let Some(mut bus) = bus_with_mt32() else { return };
    peak(&mut bus, 100);
    bus.mpu.take_lcd_message();
    // Roland's Data Set 1 to the display (address 20 00 00), with its
    // checksum.
    let text = b"  Insert Buckazoid  ";
    let mut body = vec![0x20, 0x00, 0x00];
    body.extend_from_slice(text);
    let sum: u32 = body.iter().map(|&b| b as u32).sum();
    let checksum = ((128 - sum % 128) % 128) as u8;
    let mut sysex = vec![0xF0, 0x41, 0x10, 0x16, 0x12];
    sysex.extend_from_slice(&body);
    sysex.extend_from_slice(&[checksum, 0xF7]);
    midi(&mut bus, &sysex);
    // munt plays messages as it renders.
    peak(&mut bus, 100);
    assert_eq!(bus.mpu.take_lcd_message().as_deref(), Some("Insert Buckazoid"));
}
