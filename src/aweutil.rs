//! AWEUTIL, the part of Creative's utility a DOS setup needs: `/S`
//! initialises the EMU8000, as AUTOEXEC.BAT does on a PC with an AWE32.
//! The card here starts initialised, so this only matters to programs
//! that left it in a state of their own, and to AUTOEXECs written for a
//! real card, which run cleanly with it.

use crate::command::ShellCommand;
use crate::cpu::Cpu;
use crate::video::print_string;

pub struct AweUtilCommand;

const USAGE: &str = "Initialises the Sound Blaster AWE32's EMU8000.\r\n\r\nAWEUTIL /S\r\n\r\n  /S   Initialise the EMU8000 (the sample RAM keeps what it holds).\r\n";

impl ShellCommand for AweUtilCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        let args = args.trim().to_ascii_uppercase();
        let switches: Vec<&str> = args.split_whitespace().collect();
        let text = if switches.contains(&"/?") {
            USAGE.to_string()
        } else if cpu.bus.awe.is_none() {
            "AWEUTIL: no Sound Blaster AWE32 (sbtype=awe32)\r\n".to_string()
        } else if switches.is_empty() || switches.contains(&"/S") {
            cpu.bus.awe_init();
            "AWE32 EMU8000 initialised.\r\n".to_string()
        } else if switches.iter().any(|s| s.starts_with("/EM") || s.starts_with("/U")) {
            // The resident MIDI and OPL emulation needs NMIs the card here
            // doesn't raise; programs get the synthesizer through their
            // own AWE32 drivers.
            "AWEUTIL: the resident MIDI and FM emulation isn't supported; use the game's AWE32 driver.\r\n"
                .to_string()
        } else {
            format!("AWEUTIL: unknown switch {}\r\n\r\n{}", switches.join(" "), USAGE)
        };
        print_string(cpu, &text);
    }
}
