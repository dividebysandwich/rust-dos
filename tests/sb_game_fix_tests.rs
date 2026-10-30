//! `sb_game_fix`: a known broken Sound Blaster driver is patched when it
//! resets the DSP, and the patched code mixes around the centre line.

use rust_dos::cpu::Cpu;
use rust_dos::sb_game_fix::DRIVERS;
use std::path::PathBuf;

/// The driver's code segment in these tests.
const CS: u16 = 0x2000;
const BASE: usize = (CS as usize) << 4;

/// A machine with Prehistorik 2's driver pieces at CS:0000, and the code
/// around the patches that the tests run.
fn machine() -> Cpu {
    let mut cpu = Cpu::new(PathBuf::from("."));
    let driver = &DRIVERS[0];
    for b in driver
        .checks
        .iter()
        .chain(driver.patches.iter().map(|(old, _)| old))
    {
        for (i, &byte) in b.bytes.iter().enumerate() {
            cpu.bus.write_8(BASE + b.at + i, byte);
        }
    }
    // xor cl,cl / xor ch,ch before the volume table's loop, and the effect
    // conversion's mov al,es:[di] before and stosb / loop after its patch.
    let around: [(usize, &[u8]); 3] = [
        (0x1E8E, &[0x32, 0xC9, 0x32, 0xED]),
        (0x1F3F, &[0x26, 0x8A, 0x05]),
        (0x1F48, &[0xAA, 0xE2, 0xF4]),
    ];
    for (at, bytes) in around {
        for (i, &byte) in bytes.iter().enumerate() {
            cpu.bus.write_8(BASE + at + i, byte);
        }
    }
    cpu
}

/// Run from CS:`from` until IP is `to`.
fn run(cpu: &mut Cpu, from: u16, to: u16) {
    cpu.set_cs(CS);
    cpu.set_ip(from);
    let mut steps = 0;
    while cpu.ip() != to {
        cpu.step();
        steps += 1;
        assert!(steps < 200_000, "stuck at {:04X}", cpu.ip());
    }
}

fn reset_dsp(cpu: &mut Cpu) {
    cpu.bus.io_write(0x226, 1);
    cpu.bus.io_write(0x226, 0);
}

fn read(cpu: &mut Cpu, addr: usize, len: usize) -> Vec<u8> {
    (0..len).map(|i| cpu.bus.read_8(addr + i)).collect()
}

#[test]
fn the_patched_volume_table_scales_around_the_centre() {
    let mut cpu = machine();
    reset_dsp(&mut cpu);
    cpu.set_es(0x3000);
    cpu.set_di(0x12C1);
    run(&mut cpu, 0x1E8E, 0x1EB1);
    let table = read(&mut cpu, 0x3_12C1, 65 * 64);
    for volume in 0..=64i32 {
        for sample in 0..64i32 {
            let want = ((sample - 32) * volume).div_euclid(64) as i8;
            assert_eq!(
                table[(volume * 64 + sample) as usize] as i8,
                want,
                "volume {} sample {}",
                volume,
                sample
            );
        }
    }
}

#[test]
fn the_patched_effects_and_empty_blocks_sit_at_128() {
    let mut cpu = machine();
    reset_dsp(&mut cpu);
    let samples = [0x80u8, 0xFF, 0x00, 0x7F];
    for (i, &s) in samples.iter().enumerate() {
        cpu.bus.write_8(0x4_0000 + i, s);
    }
    cpu.set_es(0x4000);
    cpu.set_di(0);
    cpu.set_cx(samples.len() as u16);
    run(&mut cpu, 0x1F3F, 0x1F4B);
    assert_eq!(read(&mut cpu, 0x4_0000, 4), [96, 127, 128, 159]);

    // A block without an effect: 168 bytes of 80h.
    cpu.set_es(0x5000);
    cpu.set_di(0);
    cpu.set_cx(0);
    run(&mut cpu, 0x2112, 0x2119);
    assert!(read(&mut cpu, 0x5_0000, 0xA8).iter().all(|&b| b == 0x80));
    assert_eq!(cpu.bus.read_8(0x5_00A8), 0);
}

#[test]
fn nothing_changes_with_the_fix_off() {
    let mut cpu = machine();
    let before = read(&mut cpu, BASE, 0x2240);
    cpu.bus.sb_game_fix = false;
    reset_dsp(&mut cpu);
    assert_eq!(read(&mut cpu, BASE, 0x2240), before);
    cpu.bus.sb_game_fix = true;
    reset_dsp(&mut cpu);
    assert_ne!(read(&mut cpu, BASE, 0x2240), before);
}
