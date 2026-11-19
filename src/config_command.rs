//! The CONFIG command of DOSBox, which batch files of packages made for
//! DOSBox (eXoDOS's launchers) run to set up the machine for the choice
//! made in their menu:
//!
//! ```text
//! CONFIG -set "composite=on"
//! CONFIG -set "cpu cycles=175000"
//! CONFIG -set midi mididevice mt32
//! ```
//!
//! DOSBox's settings become rust-dos's as the DOSBox import makes them
//! (`import::dosbox::config_set`), and the frontend applies them as the
//! settings window's changes are applied (`shell::take_settings`). The
//! batch file waits for that, so the program it starts next finds them in
//! place. As with the window's, a game's settings come back when it ends.
//!
//! DOSBox has CONFIG as a program on Z:, which is first on its PATH, so a
//! game's own CONFIG.EXE (often its setup program) in the current
//! directory runs instead, and one in a directory of the PATH doesn't
//! (`command::z_program`).

use crate::command::ShellCommand;
use crate::cpu::Cpu;
use crate::video::print_string;

/// How long CONFIG -SET waits for the frontend to take its settings, in
/// PIT ticks of emulated time: a frontend that takes them does so at the
/// end of the frame, and one that doesn't (`rust-dos run`, tests) doesn't
/// hold the batch file up for longer.
const WAIT_TICKS: u64 = crate::timer::PIT_HZ;

/// rust-dos's settings that a machine takes only as it starts (the
/// settings window's "next start" ones): DOSBox can't change them at run
/// time either, and says so.
const AT_START: [(&str, &str); 1] = [("emulator", "memsize")];

pub struct ConfigCommand;

impl ShellCommand for ConfigCommand {
    fn execute(&self, cpu: &mut Cpu, args: &str) {
        let args = args.trim();
        if args.is_empty() || args == "/?" || args.eq_ignore_ascii_case("-?") {
            print_string(cpu, "Sets a setting of DOSBox's as rust-dos has it:\r\n");
            print_string(cpu, "CONFIG -SET \"section setting=value\"\r\n");
            print_string(cpu, "CONFIG -SET \"setting=value\"\r\n");
            return;
        }
        let tell = |cpu: &mut Cpu, message: &str| {
            cpu.bus
                .log_string(&format!("[CONFIG] CONFIG {}: {}", args, message));
            print_string(cpu, &format!("CONFIG: {}\r\n", message));
        };
        let (settings, warnings) = match crate::import::dosbox::config_set(args) {
            Ok(set) => set,
            Err(e) => return tell(cpu, &e),
        };
        for warning in &warnings {
            tell(cpu, warning);
        }
        let (at_start, settings): (Vec<_>, Vec<_>) = settings
            .into_iter()
            .partition(|(section, key, _)| AT_START.contains(&(section.as_str(), key.as_str())));
        for (_, key, _) in &at_start {
            tell(
                cpu,
                &format!("{} can't be changed while the machine runs", key),
            );
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
        let until = cpu.bus.clock.now_ticks() + WAIT_TICKS;
        crate::shell::enter_wait(
            cpu,
            crate::shell::ShellWait::Settings {
                requests: settings,
                until,
            },
        );
    }
}
