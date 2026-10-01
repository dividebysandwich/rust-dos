//! The Sound Canvas with real ROMs, when there are some (in
//! `RUST_DOS_SC55_ROMS`): the firmware starts and plays, and, with
//! Nuked-SC55's own build to compare with (`RUST_DOS_SC55_HARNESS`, built
//! from tests/sc55diff/harness.cpp), every frame is the same:
//!
//! ```sh
//! RUST_DOS_SC55_ROMS=<unpacked sets> RUST_DOS_SC55_HARNESS=target/sc55diff/harness \
//!   cargo test --release --test sc55_tests
//! ```
//!
//! Where they part, `DUMP_FILE` (the harness's) and `DUMP_FILE_RUST`
//! with `DUMP_FROM`, `DUMP_TO` and `DUMP_EVERY` write both states.

use rust_dos::sc55::Sc55;
use rust_dos::sc55::rom;
use std::path::PathBuf;

/// Frames into the run the MIDI goes in at, and the bytes.
fn events(rate: usize) -> Vec<(usize, Vec<u8>)> {
    // After the firmware has started (2.2 s for the SC-55), and the mkII
    // has got over the GS reset (1.6 s in Nuked-SC55).
    let s = |sec: f64| ((sec + 2.0) * rate as f64) as usize;
    let mut ev = vec![
        // GS reset, a display message, then a little of everything.
        (s(0.0), vec![0xF0, 0x41, 0x10, 0x42, 0x12, 0x40, 0x00, 0x7F, 0x00, 0x41, 0xF7]),
        (s(0.2), vec![0xF0, 0x41, 0x10, 0x45, 0x12, 0x10, 0x00, 0x00, b'H', b'i', 0x00, 0xF7]),
        (s(2.3), vec![0xC0, 0x00, 0xC1, 0x30, 0xC2, 0x21, 0xB1, 0x5B, 0x7F, 0xB2, 0x5D, 0x60, 0xB0, 0x0A, 0x20]),
        (s(2.35), vec![0x90, 0x3C, 0x64, 0x90, 0x40, 0x64, 0x91, 0x37, 0x50, 0x92, 0x24, 0x70, 0x99, 0x24, 0x7F, 0x99, 0x2A, 0x60]),
        (s(2.6), vec![0x99, 0x26, 0x7F, 0x99, 0x2A, 0x60]),
        (s(2.8), vec![0xE0, 0x00, 0x50, 0x80, 0x3C, 0x40]),
        (s(3.0), vec![0x99, 0x24, 0x7F, 0x91, 0x3E, 0x60, 0xC3, 0x58, 0x93, 0x48, 0x70]),
        (s(3.4), vec![0x80, 0x40, 0x00, 0x81, 0x37, 0x00, 0x82, 0x24, 0x00, 0xE0, 0x00, 0x40]),
        (s(3.6), vec![0xB0, 0x7B, 0x00, 0xB1, 0x7B, 0x00, 0xB3, 0x7B, 0x00]),
    ];
    ev.sort_by_key(|e| e.0);
    ev
}

fn roms(name: &str) -> Option<rom::Loaded> {
    let dir = PathBuf::from(std::env::var_os("RUST_DOS_SC55_ROMS")?);
    let found = rom::scan(&[dir], name).into_iter().next()?;
    Some(rom::load(&found).unwrap())
}

/// The native frames of a run of `frames` with `events`.
fn run(roms: &rom::Loaded, frames: usize, events: &[(usize, Vec<u8>)]) -> Vec<i32> {
    let mut synth = Sc55::switched_on(roms, 44100);
    let mut out = Vec::with_capacity(frames * 2);
    let mut next = 0;
    // A dump of the state, to compare with the harness's.
    let var = |name| std::env::var(name).ok();
    let mut dump = var("DUMP_FILE_RUST").map(|path| {
        let file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
        (file, var("DUMP_FROM").unwrap().parse::<usize>().unwrap(), var("DUMP_TO").unwrap().parse::<usize>().unwrap())
    });
    for n in 0..frames {
        while next < events.len() && events[next].0 <= n {
            synth.midi_bytes(&events[next].1);
            next += 1;
        }
        out.extend(synth.native_frame());
        if let Some(dump) = &mut dump
            && (dump.1..dump.2).contains(&n)
            && n % var("DUMP_EVERY").map_or(1, |v| v.parse::<usize>().unwrap()) == 0
        {
            use std::io::Write;
            let state: Vec<String> = synth.debug_state().iter().map(|v| v.to_string()).collect();
            writeln!(dump.0, "{} {}", n, state.join(" ")).unwrap();
        }
    }
    out
}

fn check(set: &str, harness_family: &str) {
    let Some(roms) = roms(set) else {
        eprintln!("skipped, no {} ROMs in RUST_DOS_SC55_ROMS", set);
        return;
    };
    let rate = Sc55::switched_on(&roms, 44100).native_rate() as usize;
    let frames = 6 * rate;
    let events = events(rate);
    let start = std::time::Instant::now();
    let ours = run(&roms, frames, &events);
    eprintln!("{}: {} s of sound in {:?}", set, frames / rate, start.elapsed());

    compare(set, harness_family, frames, &events, &ours);

    // It plays: the notes are heard.
    let loudest = |from: usize, to: usize| ours[from * 2..to * 2].iter().map(|v| v.unsigned_abs()).max().unwrap();
    assert!(loudest(rate * 44 / 10, rate * 5) > 1 << 25, "{}: silent", set);
}

/// Every frame the same as Nuked-SC55's, if it is there to ask.
fn compare(set: &str, harness_family: &str, frames: usize, events: &[(usize, Vec<u8>)], ours: &[i32]) {
    let Some(harness) = std::env::var_os("RUST_DOS_SC55_HARNESS") else { return };
    let dir = PathBuf::from(std::env::var_os("RUST_DOS_SC55_ROMS").unwrap());
    let found = rom::scan(&[dir], set).into_iter().next().unwrap();
    let event_file = std::env::temp_dir().join(format!("sc55-events-{}-{}", set, std::process::id()));
    let lines: Vec<String> = events
        .iter()
        .map(|(at, bytes)| format!("{} {}", at, bytes.iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(" ")))
        .collect();
    std::fs::write(&event_file, lines.join("\n")).unwrap();
    let mut cmd = std::process::Command::new(harness);
    cmd.arg(harness_family).arg(frames.to_string()).arg(&event_file);
    for (location, source) in &found.sources {
        let rom::Source::File(path) = source else { panic!("unpack the ROMs for the comparison") };
        let name = match location {
            rom::Location::Rom1 => "rom1",
            rom::Location::Rom2 => "rom2",
            rom::Location::SmRom => "smrom",
            rom::Location::Wave1 => "wave1",
            rom::Location::Wave2 => "wave2",
            rom::Location::Wave3 => "wave3",
        };
        cmd.arg(format!("{}={}", name, path.display()));
    }
    let output = cmd.output().unwrap();
    let _ = std::fs::remove_file(&event_file);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let theirs: Vec<i32> = output.stdout.as_chunks::<4>().0.iter().map(|&b| i32::from_le_bytes(b)).collect();
    assert_eq!(theirs.len(), ours.len());
    if let Some(i) = (0..ours.len()).find(|&i| ours[i] != theirs[i]) {
        panic!("{}: frame {} differs: ours {:?}, Nuked-SC55's {:?}", set, i / 2, &ours[i..i + 4], &theirs[i..i + 4]);
    }
}

#[test]
fn sc55_matches_nuked() {
    check("mk1-v1.21", "mk1");
}

#[test]
fn sc55_mk2_matches_nuked() {
    check("mk2-v1.01", "mk2");
}

#[test]
fn a_silent_module_is_silent() {
    // The firmware's steady output level isn't heard, from the start.
    let Some(roms) = roms("mk1-v1.21") else { return };
    let mut synth = Sc55::new(&roms, 44100);
    let loudest = (0..44100).map(|_| synth.render()).fold(0.0f32, |m, (l, r)| m.max(l.abs()).max(r.abs()));
    assert!(loudest < 8.0, "{}", loudest);
    synth.message(0x90, 60, 100);
    let loudest = (0..22050).map(|_| synth.render()).fold(0.0f32, |m, (l, r)| m.max(l.abs()).max(r.abs()));
    assert!(loudest > 300.0, "{}", loudest);
}
