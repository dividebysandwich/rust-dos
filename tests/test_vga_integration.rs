use rust_dos::cpu::Cpu;
use std::path::PathBuf;
use std::time::Instant;

#[test]
fn test_vga_initialization() {
    let root_path = PathBuf::from(".");
    let mut cpu = Cpu::new(root_path);

    let loaded = cpu.load_executable("TEST13.EXE") || cpu.load_executable("test13.exe");

    if !loaded {
        println!("TEST13.EXE not found in current directory. Skipping integration test.");
        return;
    }

    let start = Instant::now();
    let max_duration = std::time::Duration::from_secs(4); // Give it 4 seconds

    let mut instructions = 0;

    // Initial State Check
    assert_eq!(cpu.bus.video_mode, rust_dos::video::VideoMode::Text80x25);

    loop {
        if start.elapsed() > max_duration {
            break;
        }

        cpu.step();
        instructions += 1;

        // Stop if CPU halts
        if cpu.state != rust_dos::cpu::CpuState::Running {
            panic!(
                "CPU Stopped running prematurely after {} instructions. State: {:?}",
                instructions, cpu.state
            );
        }

        // Success Fast-Exit: If we switch to Mode 13h, we are good!
        if cpu.bus.video_mode == rust_dos::video::VideoMode::Graphics320x200 {
            println!(
                "Success! Switch to Mode 13h detected after {} instructions.",
                instructions
            );
            return;
        }
    }

    if cpu.bus.video_mode != rust_dos::video::VideoMode::Graphics320x200 {
        println!("Test Failed to Switch Mode. Dumping Text Screen Content:");
        // Dump 80x25 text buffer
        for row in 0..25 {
            let mut line = String::new();
            for col in 0..80 {
                let offset = (row * 80 + col) * 2;
                // vram_text stores Char, Attribute pairs.
                if offset < cpu.bus.vga.vram_text.len() {
                    let char_code = cpu.bus.vga.vram_text[offset];
                    let c = if char_code >= 32 && char_code <= 126 {
                        char_code as char
                    } else {
                        '.'
                    };
                    line.push(c);
                }
            }
            println!("{}", line);
        }
    }

    assert_eq!(
        cpu.bus.video_mode,
        rust_dos::video::VideoMode::Graphics320x200,
        "Failed to switch to VGA Mode 13h within timeout!"
    );
}
