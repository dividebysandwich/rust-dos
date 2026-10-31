//! `game_fixes`: the code fixes are applied when a program resets the
//! DSP, and the patched Sound Blaster drivers mix around the centre line.
//! The fixes that skip a program at EXEC are tested in exec_tests.rs.

use rust_dos::cpu::Cpu;
use rust_dos::game_fixes::{CodePatch, Kind, by_name};
use std::path::PathBuf;

/// The driver's code segment in these tests.
const CS: u16 = 0x2000;
const BASE: usize = (CS as usize) << 4;

/// Where the tests run each driver's code, by offset in its segment.
struct Layout {
    /// The fix's name.
    name: &'static str,
    /// Code around the patches that the tests run: xor cl,cl before the
    /// volume table's loop, mov al,es:[di] before and stosb / loop after
    /// the effect conversion, and what follows a patch the tests run on.
    around: &'static [(usize, &'static [u8])],
    /// The volume table's loops, from xor cl,cl to the end.
    volume_table: (u16, u16),
    /// The effect conversion's loop, from its first instruction to the end.
    effects: (u16, u16),
    /// The fill of a block without an effect.
    silence: (u16, u16),
}

const LAYOUTS: [Layout; 2] = [
    Layout {
        name: "prehistorik2-sb-mix",
        around: &[
            (0x1E8E, &[0x32, 0xC9]),
            (0x1F3F, &[0x26, 0x8A, 0x05]),
            (0x1F48, &[0xAA, 0xE2, 0xF4]),
        ],
        volume_table: (0x1E8E, 0x1EB1),
        effects: (0x1F3F, 0x1F4B),
        silence: (0x2112, 0x2119),
    },
    Layout {
        name: "jukebox-sb-mix",
        around: &[
            (0x4278, &[0x32, 0xC9]),
            (0x4332, &[0x26, 0x8A, 0x05]),
            (0x433A, &[0xAA, 0xE2, 0xF5]),
            // The ramp after an effect, up to its rep stosb.
            (
                0x4509,
                &[
                    0xFE, 0xC8, 0x74, 0x05, 0xAA, 0xE2, 0xF9, 0xEB, 0x02, 0xF3, 0xAA,
                ],
            ),
        ],
        volume_table: (0x4278, 0x4292),
        effects: (0x4332, 0x433D),
        silence: (0x4552, 0x4559),
    },
];

/// The code of the fix called `name`.
fn code(name: &str) -> &'static CodePatch {
    match by_name(name).unwrap().kind {
        Kind::Code(code) => code,
        Kind::SkipAtExec { .. } => panic!("{name} isn't a code fix"),
    }
}

/// Write the pieces of `code` at CS:0000.
fn load(cpu: &mut Cpu, code: &CodePatch) {
    let pieces = code
        .checks
        .iter()
        .map(|b| (b.at, b.bytes))
        .chain(code.patches.iter().map(|(old, _)| (old.at, old.bytes)));
    for (at, bytes) in pieces {
        for (i, &byte) in bytes.iter().enumerate() {
            cpu.bus.write_8(BASE + at + i, byte);
        }
    }
}

/// A machine with the driver's pieces at CS:0000, and the code around the
/// patches that the tests run.
fn machine(layout: &Layout) -> Cpu {
    let mut cpu = Cpu::new(PathBuf::from("."));
    load(&mut cpu, code(layout.name));
    for &(at, bytes) in layout.around {
        for (i, &byte) in bytes.iter().enumerate() {
            cpu.bus.write_8(BASE + at + i, byte);
        }
    }
    cpu
}

/// Run from CS:`from` until IP is `to`.
fn run(cpu: &mut Cpu, (from, to): (u16, u16)) {
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
    for layout in &LAYOUTS {
        let mut cpu = machine(layout);
        reset_dsp(&mut cpu);
        cpu.set_es(0x3000);
        cpu.set_di(0x12C1);
        run(&mut cpu, layout.volume_table);
        let table = read(&mut cpu, 0x3_12C1, 65 * 64);
        for volume in 0..=64i32 {
            for sample in 0..64i32 {
                let want = ((sample - 32) * volume).div_euclid(64) as i8;
                assert_eq!(
                    table[(volume * 64 + sample) as usize] as i8,
                    want,
                    "{}: volume {} sample {}",
                    layout.name,
                    volume,
                    sample
                );
            }
        }
        assert_eq!(cpu.bus.read_8(0x3_12C1 + 65 * 64), 0, "{}", layout.name);
    }
}

#[test]
fn the_patched_effects_and_empty_blocks_sit_at_128() {
    for layout in &LAYOUTS {
        let mut cpu = machine(layout);
        reset_dsp(&mut cpu);
        let samples = [0x80u8, 0xFF, 0x00, 0x7F];
        for (i, &s) in samples.iter().enumerate() {
            cpu.bus.write_8(0x4_0000 + i, s);
        }
        cpu.set_es(0x4000);
        cpu.set_di(0);
        cpu.set_cx(samples.len() as u16);
        run(&mut cpu, layout.effects);
        let converted = read(&mut cpu, 0x4_0000, 4);
        assert_eq!(converted, [96, 127, 128, 159], "{}", layout.name);

        // A block without an effect: 168 bytes of 80h.
        cpu.set_es(0x5000);
        cpu.set_di(0);
        cpu.set_cx(0);
        run(&mut cpu, layout.silence);
        let block = read(&mut cpu, 0x5_0000, 0xA9);
        assert!(block[..0xA8].iter().all(|&b| b == 0x80), "{}", layout.name);
        assert_eq!(block[0xA8], 0, "{}", layout.name);
    }
}

#[test]
fn jukebox_adventure_fills_the_rest_of_an_effects_block_with_128() {
    let layout = &LAYOUTS[1];
    let mut cpu = machine(layout);
    reset_dsp(&mut cpu);
    // The effect ended 40 bytes into the block, on a byte far from 128;
    // the driver ramped from there to 0.
    cpu.bus.write_8(0x5_0027, 0xF0);
    cpu.set_es(0x5000);
    cpu.set_di(0x28);
    cpu.set_cx(0xA8 - 0x28);
    run(&mut cpu, (0x4501, 0x4514));
    let block = read(&mut cpu, 0x5_0028, 0x81);
    assert!(block[..0x80].iter().all(|&b| b == 0x80));
    assert_eq!(block[0x80], 0);
}

#[test]
fn nothing_changes_with_the_fix_off() {
    for layout in &LAYOUTS {
        let mut cpu = machine(layout);
        let before = read(&mut cpu, BASE, 0x4800);
        cpu.bus.game_fixes = false;
        reset_dsp(&mut cpu);
        assert_eq!(read(&mut cpu, BASE, 0x4800), before, "{}", layout.name);
        cpu.bus.game_fixes = true;
        cpu.bus.game_fixes_off = vec![layout.name.to_string()];
        reset_dsp(&mut cpu);
        assert_eq!(read(&mut cpu, BASE, 0x4800), before, "{}", layout.name);
        cpu.bus.game_fixes_off.clear();
        reset_dsp(&mut cpu);
        assert_ne!(read(&mut cpu, BASE, 0x4800), before, "{}", layout.name);
    }
}
