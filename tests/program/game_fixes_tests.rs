//! `game_fixes`: the code fixes are applied when a program resets the
//! DSP, the patched Sound Blaster drivers mix around the centre line, and
//! Jukebox Adventure's music player gets past effect Bxx, and Lands of
//! Lore 2's launcher, patched as it loads, starts the game with 32 MB of
//! virtual memory. The fixes that skip a program at EXEC are tested in
//! exec_tests.rs.

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
        Kind::SkipAtExec { .. } | Kind::AtLoad { .. } => panic!("{name} isn't a code fix"),
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
fn prehistorik_2_fills_the_rest_of_an_effects_block_with_128() {
    let layout = &LAYOUTS[0];
    let mut cpu = machine(layout);
    reset_dsp(&mut cpu);
    // The effect filled the block's first 40 bytes, AX of them; the driver
    // filled the rest with 0.
    cpu.set_ax(0x28);
    cpu.set_es(0x5000);
    cpu.set_di(0x28);
    run(&mut cpu, (0x20FB, 0x2108));
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

/// Jukebox Adventure with both its fixes' code at CS:0000, and DS:0355h's
/// table pointing effect B at its handler, 47D7h.
fn jukebox() -> Cpu {
    let mut cpu = Cpu::new(PathBuf::from("."));
    load(&mut cpu, code("jukebox-sb-mix"));
    load(&mut cpu, code("jukebox-mod-hang"));
    cpu.bus.write_16(BASE + 0x355 + 2 * 0x0B, 0x47D7);
    cpu.set_ds(CS);
    cpu
}

/// Run the music player's effect dispatch for effect `effect` from
/// `cmp bx,0Bh`, for at most `steps` instructions: where IP ends up.
fn dispatch(cpu: &mut Cpu, effect: u16, steps: usize) -> u16 {
    cpu.set_cs(CS);
    cpu.set_ip(0x47B7);
    cpu.set_bx(effect);
    for _ in 0..steps {
        if cpu.ip() == 0x47D7 {
            break;
        }
        cpu.step();
    }
    cpu.ip()
}

#[test]
fn jukebox_adventures_music_player_gets_past_effect_b() {
    let mut cpu = jukebox();
    // As the game shipped: cmp / je round and round.
    assert_ne!(dispatch(&mut cpu, 0x0B, 100), 0x47D7);
    reset_dsp(&mut cpu);
    assert_eq!(dispatch(&mut cpu, 0x0B, 100), 0x47D7);
}

#[test]
fn a_fix_left_out_by_name_stays_out() {
    let mut cpu = jukebox();
    cpu.bus.game_fixes_off = vec!["jukebox-mod-hang".to_string()];
    let volume_table = BASE + 0x427A;
    let before = cpu.bus.read_8(volume_table);
    reset_dsp(&mut cpu);
    // The mix is fixed, the hang isn't.
    assert_ne!(cpu.bus.read_8(volume_table), before);
    assert_ne!(dispatch(&mut cpu, 0x0B, 100), 0x47D7);
    // With game_fixes off, neither is.
    let mut cpu = jukebox();
    cpu.bus.game_fixes = false;
    reset_dsp(&mut cpu);
    assert_eq!(cpu.bus.read_8(volume_table), before);
    assert_ne!(dispatch(&mut cpu, 0x0B, 100), 0x47D7);
}

/// A launcher that does what Lands of Lore 2's LOLG.EXE does with its
/// DOS/4GW settings, with the pieces `lol2-virtual-size` checks at their
/// offsets in the file: it builds an environment of the string at DS:0071h
/// with LOLG.EXE's own code at 01DAh, and EXECs LOLG.DAT with it.
fn lol2_launcher() -> Vec<u8> {
    let Kind::AtLoad { code, .. } = by_name("lol2-virtual-size").unwrap().kind else {
        panic!("lol2-virtual-size patches LOLG.EXE as it loads");
    };
    // LOLG.EXE's size, 512 bytes of header and the load module.
    let mut file = vec![0u8; 2847];
    for b in code.checks.iter().chain(code.patches.iter().map(|(old, _)| old)) {
        file[b.at..b.at + b.bytes.len()].copy_from_slice(b.bytes);
    }
    let header: [u16; 14] = [
        0x5A4D,
        (file.len() % 512) as u16,
        file.len().div_ceil(512) as u16,
        0,      // no relocations
        0x20,   // header paragraphs
        0x20,   // MINALLOC
        0xFFFF, // MAXALLOC
        0x92,   // SS
        0x100,  // SP
        0,
        0, // IP
        0, // CS
        0x1C,
        0,
    ];
    for (i, word) in header.iter().enumerate() {
        file[i * 2..i * 2 + 2].copy_from_slice(&word.to_le_bytes());
    }
    #[rustfmt::skip]
    let start = [
        0x8C, 0xC5,             // MOV BP,ES: the PSP
        0x8C, 0xC8,             // MOV AX,CS
        0x05, 0x21, 0x00,       // ADD AX,21h: LOLG.EXE's data segment
        0x8E, 0xD8,             // MOV DS,AX
        0x8E, 0xC5,             // MOV ES,BP
        0xBB, 0x00, 0x01,       // MOV BX,100h
        0xB4, 0x4A,             // MOV AH,4Ah: keep 4 KB
        0xCD, 0x21,             // INT 21h
        0x8C, 0xC8,             // MOV AX,CS
        0x05, 0x60, 0x00,       // ADD AX,60h: the environment's block
        0x8E, 0xC0,             // MOV ES,AX
        0x31, 0xFF,             // XOR DI,DI
        0xE8, 0xBC, 0x01,       // CALL 01DAh: append the settings
        0x8C, 0xC0,             // MOV AX,ES
        0xA3, 0x20, 0x03,       // MOV [320h],AX: EXEC's environment
        0x8C, 0xD8,             // MOV AX,DS
        0xA3, 0x24, 0x03,       // MOV [324h],AX: the command tail's segment
        0xA3, 0x28, 0x03,       // MOV [328h],AX: FCB 1's
        0xA3, 0x2C, 0x03,       // MOV [32Ch],AX: FCB 2's
        0x1E,                   // PUSH DS
        0x07,                   // POP ES
        0xBB, 0x20, 0x03,       // MOV BX,320h
        0xBA, 0x00, 0x03,       // MOV DX,300h: LOLG.DAT
        0xB8, 0x00, 0x4B,       // MOV AX,4B00h
        0xCD, 0x21,             // INT 21h
        0xF4,                   // HLT
    ];
    file[0x200..0x200 + start.len()].copy_from_slice(&start);
    // RET after the code at 01DAh.
    file[0x200 + 0x1E4] = 0xC3;
    // The data segment, at 0210h in the load module.
    let data = 0x200 + 0x210;
    file[data + 0x300..data + 0x309].copy_from_slice(b"LOLG.DAT\0");
    file[data + 0x322..data + 0x324].copy_from_slice(&0x340u16.to_le_bytes());
    file[data + 0x326..data + 0x328].copy_from_slice(&0x350u16.to_le_bytes());
    file[data + 0x32A..data + 0x32C].copy_from_slice(&0x350u16.to_le_bytes());
    file[data + 0x341] = 0x0D;
    file
}

/// Run the launcher in a directory of its own, with LOLG.DAT (a HLT)
/// beside it when `game`, until a program halts: the environment LOLG.DAT
/// got, or the launcher's own string when it couldn't start it.
fn run_lol2_launcher(name: &str, game: bool, setup: impl Fn(&mut Cpu)) -> String {
    let dir = PathBuf::from("target/test_game_fixes").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("LOLG.EXE"), lol2_launcher()).unwrap();
    if game {
        std::fs::write(dir.join("LOLG.DAT"), [0xF4]).unwrap();
    }
    let mut cpu = Cpu::new(dir);
    setup(&mut cpu);
    assert!(cpu.load_executable("LOLG.EXE", None));
    let launcher = cpu.current_psp;
    let mut steps = 0;
    while cpu.bus.read_8(cpu.get_physical_addr(cpu.cs(), cpu.ip())) != 0xF4 {
        cpu.step();
        steps += 1;
        assert!(steps < 10_000, "stuck at {:04X}:{:04X}", cpu.cs(), cpu.ip());
    }
    // LOLG.DAT runs only when it is there.
    assert_eq!(cpu.current_psp != launcher, game);
    let text = if game {
        let psp = cpu.current_psp as usize * 16;
        let env = cpu.bus.read_16(psp + 0x2C) as usize * 16;
        read(&mut cpu, env, 0x100)
    } else {
        // The string in the launcher's data segment.
        read(&mut cpu, (launcher as usize + 0x10) * 16 + 0x281, 0x5A)
    };
    let end = text.iter().position(|&b| b == 0).unwrap();
    String::from_utf8(text[..end].to_vec()).unwrap()
}

const SETTINGS: &str = "DOS4GVM=VirtualMemory:ON,VirtualSize:XXM,SwapFileName:lolgswap.swp,SwapMin:20M,SwapInc:0";

#[test]
fn lands_of_lore_2_starts_with_32_mb_of_virtual_memory() {
    let got = run_lol2_launcher("lol2", true, |_| {});
    assert_eq!(got, SETTINGS.replace("XX", "32"));
}

#[test]
fn lands_of_lore_2_keeps_its_settings_with_the_fix_off() {
    let off = run_lol2_launcher("lol2_fixes_off", true, |cpu| cpu.bus.game_fixes = false);
    assert_eq!(off, SETTINGS.replace("XX", "24"));
    let left_out = run_lol2_launcher("lol2_fix_left_out", true, |cpu| {
        cpu.bus.game_fixes_off = vec!["lol2-virtual-size".to_string()];
    });
    assert_eq!(left_out, SETTINGS.replace("XX", "24"));
}

#[test]
fn a_lolg_exe_without_lolg_dat_beside_it_isnt_patched() {
    let got = run_lol2_launcher("lol2_alone", false, |_| {});
    assert_eq!(got, SETTINGS.replace("XX", "24"));
}
