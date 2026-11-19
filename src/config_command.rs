//! The CONFIG command of DOSBox, which batch files of packages made for
//! DOSBox (eXoDOS's launchers) run to set up the machine for the choice
//! made in their menu:
//!
//! ```text
//! CONFIG -set "composite=on"
//! CONFIG -set "cpu cycles=175000"
//! CONFIG -set "midi mididevice=mt32"
//! ```
//!
//! DOSBox's settings become rust-dos's as the DOSBox import makes them
//! (`import::dosbox::config_set`), and the frontend applies them as the
//! settings window's changes are applied (`Bus::settings_requests`). The
//! batch file waits for that, so the program it starts next finds them in
//! place. As with the window's, a game's settings come back when it ends.
//!
//! DOSBox has CONFIG as a program on Z:, so a game's own CONFIG.EXE (often
//! its setup program) runs instead, from the current directory or the
//! PATH: rust-dos runs it only where no program of the name is found
//! (`command::run_fallback`).

use crate::command::ShellCommand;
use crate::cpu::Cpu;
use crate::video::print_string;

/// How long CONFIG -SET waits for the frontend to take its settings, in
/// PIT ticks of emulated time: a frontend that takes them does so at the
/// end of the frame, and one that doesn't (`rust-dos run`) doesn't hold
/// the batch file up for longer.
const WAIT_TICKS: u64 = crate::timer::PIT_HZ;

pub struct ConfigCommand;

impl ShellCommand for ConfigCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        let args = args.trim();
        if args.is_empty() || args == "/?" || args.eq_ignore_ascii_case("-?") {
            print_string(cpu, "Sets a setting of DOSBox's as rust-dos has it:\r\n");
            print_string(cpu, "CONFIG -SET \"[section] setting=value\"\r\n");
            return;
        }
        let (settings, warnings) = match crate::import::dosbox::config_set(args) {
            Ok(set) => set,
            Err(e) => {
                cpu.bus
                    .log_string(&format!("[CONFIG] CONFIG {}: {}", args, e));
                print_string(cpu, &format!("CONFIG: {}\r\n", e));
                return;
            }
        };
        for warning in &warnings {
            cpu.bus
                .log_string(&format!("[CONFIG] CONFIG {}: {}", args, warning));
            print_string(cpu, &format!("CONFIG: {}\r\n", warning));
        }
        if settings.is_empty() {
            return;
        }
        for (section, key, value) in &settings {
            cpu.bus.log_string(&format!(
                "[CONFIG] CONFIG -SET: [{}] {}={}",
                section, key, value
            ));
        }
        cpu.bus.settings_requests.extend(settings);
        let until = cpu.bus.clock.now_ticks() + WAIT_TICKS;
        crate::shell::enter_wait(cpu, crate::shell::ShellWait::Settings { until });
    }
}
