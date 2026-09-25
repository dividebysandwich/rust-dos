//! Game profiles on a running machine: a profile's settings layered over
//! the configuration's, its commands run at the prompt, and the game over
//! once they have run and the prompt is back.

use rust_dos::config::Settings;
use rust_dos::cpu::Cpu;
use rust_dos::exec::{self, NoHook};
use rust_dos::games::{self, ActiveGame, NewGame};
use rust_dos::timer::CpuSpeed;
use std::fs;
use std::path::{Path, PathBuf};

fn scratch(name: &str, files: &[(&str, &[u8])]) -> PathBuf {
    let base = PathBuf::from("target/test_games").join(name);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    for (file, bytes) in files {
        fs::write(base.join(file), bytes).unwrap();
    }
    base
}

/// Run the machine for up to `ms` ms of emulated time, until `stop`.
fn run_until(cpu: &mut Cpu, ms: u64, stop: impl Fn(&Cpu) -> bool) -> bool {
    for _ in 0..ms {
        if stop(cpu) {
            return true;
        }
        let end = cpu.bus.clock.icount + 1000;
        cpu.bus.start_batch(end);
        exec::run_batch(cpu, &mut NoHook, false);
    }
    stop(cpu)
}

#[test]
fn a_game_runs_its_commands_and_ends_at_the_prompt() {
    // GAME.COM: wait a little (a loop), then exit.
    #[rustfmt::skip]
    let game_com = [
        0xB9, 0x00, 0x40,       // MOV CX, 4000h
        0xE2, 0xFE,             // LOOP $
        0xB8, 0x00, 0x4C,       // MOV AX, 4C00h
        0xCD, 0x21,             // INT 21h
    ];
    let dir = scratch("run", &[("GAME.COM", &game_com)]);
    let mut cpu = Cpu::new(dir.clone());
    cpu.bus.set_cycles_per_ms(1000);
    cpu.load_shell();

    // The profile made in the settings window, with a slower CPU.
    let base = Settings::default();
    let current = Settings { cycles: CpuSpeed::Fixed(3000), ..base.clone() };
    let new = NewGame { name: "The Game".into(), directory: "C:\\".into(), command: "GAME".into() };
    let text = games::profile_text(&new, &base, &current, &[], None).unwrap();

    let prepared = games::prepare("the-game", &base, &text, Path::new("/"), None).unwrap();
    assert_eq!(prepared.settings.cycles, CpuSpeed::Fixed(3000));
    cpu.queue_batch_lines(&prepared.autoexec);
    let game = ActiveGame {
        id: "the-game".into(),
        name: prepared.name,
        base,
        saved: prepared.settings,
        replaced: Vec::new(),
    };
    assert!(!game.done(&cpu), "its commands are waiting");

    // It starts, and is not over while it runs.
    assert!(run_until(&mut cpu, 1000, |cpu| !cpu.shell_idle()), "the game starts");
    assert!(!game.done(&cpu));
    // It ends, and the prompt is back.
    assert!(run_until(&mut cpu, 5000, |cpu| game.done(cpu)), "the game ends");
    assert!(!cpu.batch.is_active());
}
