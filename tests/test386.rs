//! test386.asm (github.com/barotto/test386.asm), a CPU test ROM that runs
//! from reset through real mode, protected mode, paging, ring 3, virtual-8086
//! mode and task switches, reporting its progress as POST codes.
//!
//! Opt-in: build the ROM with tests/test386/build.sh, then run
//! `TEST386_DIR=target/test386 cargo test --release --test test386 -- --ignored --nocapture`.
//! The ROM halts with the POST code of a failed test, or FFh when all
//! passed; the arithmetic results of test EEh are compared with the
//! reference that comes with it.

use rust_dos::cpu::{Cpu, CpuModel, CpuState};
use std::path::PathBuf;

#[test]
#[ignore]
fn test386_rom() {
    let dir = PathBuf::from(std::env::var("TEST386_DIR").expect("TEST386_DIR not set"));
    let rom = std::fs::read(dir.join("test386.bin")).expect("test386.bin");
    assert!(rom.len() == 0x10000 || rom.len() == 0x20000);

    let mut cpu = Cpu::with_memory(PathBuf::from("."), 2);
    cpu.model = CpuModel::I386;
    cpu.bus.load_bytes(0x10_0000 - rom.len(), &rom);
    cpu.bus.io_write(0x21, 0xFF);
    cpu.bus.io_write(0xA1, 0xFF);
    cpu.reset();

    // TEST386_TRACE=n prints the last n instruction addresses on failure.
    let trace_len: usize = std::env::var("TEST386_TRACE").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut trace = std::collections::VecDeque::with_capacity(trace_len + 1);
    let mut last_post = cpu.bus.post_code;
    let mut steps = 0u64;
    while cpu.state == CpuState::Running && steps < 500_000_000 {
        if trace_len > 0 && trace.back().map(|t: &(u16, u32, u32, u32)| t.1) != Some(cpu.eip()) {
            if trace.len() == trace_len {
                trace.pop_front();
            }
            trace.push_back((cpu.cs(), cpu.eip(), cpu.esp(), cpu.eax()));
        }
        cpu.step();
        steps += 1;
        if cpu.bus.post_code != last_post {
            last_post = cpu.bus.post_code;
            println!("POST {:02X} after {} instructions", last_post, steps);
        }
    }
    let post = cpu.bus.post_code;
    if post != 0xFF {
        for (cs, eip, esp, eax) in &trace {
            println!("  {:04X}:{:08X}  ESP={:08X} EAX={:08X}", cs, eip, esp, eax);
        }
    }
    println!(
        "Stopped with POST {:02X} at {:04X}:{:08X} ({:?}) after {} instructions",
        post,
        cpu.cs(),
        cpu.eip(),
        cpu.state,
        steps
    );
    assert_eq!(post, 0xFF, "test {:02X} failed; look up EIP in test386.lst", post);

    // Test EEh prints its results; compare them with the reference.
    let output = String::from_utf8_lossy(&cpu.bus.debug_console).to_string();
    let reference = std::fs::read_to_string(dir.join("test386-EE-reference.txt")).unwrap();
    std::fs::write(dir.join("rust-dos-EE-output.txt"), &output).unwrap();
    let ours: Vec<&str> = output.lines().collect();
    let theirs: Vec<&str> = reference.lines().collect();
    let differing: Vec<_> = theirs
        .iter()
        .zip(ours.iter())
        .filter(|(a, b)| a != b)
        .take(20)
        .collect();
    for (want, got) in &differing {
        println!("want: {}\n got: {}", want, got);
    }
    assert_eq!(ours.len(), theirs.len(), "EE output lines");
    assert!(differing.is_empty(), "EE results differ (see rust-dos-EE-output.txt)");
}
