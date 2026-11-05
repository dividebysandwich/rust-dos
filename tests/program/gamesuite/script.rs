//! What a scenario does to its machine, step by step, in emulated time,
//! and what it expects to see and hear.

use super::machine::{self, Machine, Rig};
use std::path::Path;

#[derive(Clone, Debug)]
pub enum Step {
    /// Run at this many instructions a millisecond from now on.
    Cycles(u32),
    /// Send input to, and look at, this machine of several (0, 1).
    Focus(usize),
    /// Let `ms` milliseconds pass.
    Wait(u64),
    /// Press and release a key (`keyboard::lookup` names).
    Key(&'static str),
    /// Press each key in turn, `ms` milliseconds apart.
    Keys(&'static [&'static str], u64),
    /// Hold a key for `ms` milliseconds.
    Hold(&'static str, u64),
    /// Type text, `\n` for Enter.
    Type(&'static str),
    /// Press a key every `ms` milliseconds until the condition holds, at
    /// most `n` times.
    PressUntil(&'static str, Cond, u64, u32),
    /// Wait up to `ms` milliseconds until the condition holds.
    WaitFor(Cond, u64),
    /// Fail unless the condition holds.
    Check(Cond),
    /// Hash the picture.
    Shot(&'static str),
    /// Hash `ms` milliseconds of the sound and want it audible.
    Listen(&'static str, u64),
    /// Hash `ms` milliseconds of the sound, audible or not.
    Record(&'static str, u64),
}

#[derive(Clone, Debug)]
pub enum Cond {
    /// The program ended and the prompt is back, with no batch file
    /// running.
    ShellIdle,
    /// A program runs.
    Running,
    /// The BIOS video mode is this one.
    Mode(u8),
    /// A text-mode screen shows this.
    Text(&'static str),
    /// The picture has at least this many colours.
    Colours(usize),
    Not(&'static Cond),
}

/// A picture or sound hashed on the way.
#[derive(Clone, Debug)]
pub struct Checkpoint {
    pub label: String,
    pub hash: String,
}

/// What came out of a run: its checkpoints, and notes for the report
/// (sound peaks, the text left on the screen).
#[derive(Default, Debug)]
pub struct Outcome {
    pub checkpoints: Vec<Checkpoint>,
    pub notes: Vec<String>,
}

/// Each `WaitFor` polls this often.
const POLL_MS: u64 = 20;

impl Cond {
    pub fn holds(&self, m: &mut Machine) -> bool {
        match self {
            Cond::ShellIdle => m.shell_idle() && !m.cpu.batch.is_active(),
            Cond::Running => !m.shell_idle(),
            Cond::Mode(mode) => m.bios_mode() == *mode,
            Cond::Text(text) => m.screen_text().is_some_and(|screen| screen.contains(text)),
            Cond::Colours(n) => {
                let frame = m.picture();
                machine::colours(&frame) >= *n
            }
            Cond::Not(cond) => !cond.holds(m),
        }
    }
}

/// Whether the path (`\`- or `/`-separated) exists under `dir`, in any
/// case, as DOS sees it.
pub fn exists_nocase(dir: &Path, name: &str) -> bool {
    let mut at = dir.to_path_buf();
    for part in name.split(['\\', '/']).filter(|p| !p.is_empty()) {
        let Ok(entries) = std::fs::read_dir(&at) else { return false };
        let found = entries
            .flatten()
            .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(part));
        match found {
            Some(e) => at = e.path(),
            None => return false,
        }
    }
    true
}

fn key(name: &str) -> Result<rust_dos::keyboard::PcKey, String> {
    rust_dos::keyboard::lookup(name).ok_or_else(|| format!("no key {:?}", name))
}

/// Run the steps. Pictures and sounds go to `artifacts` as PNG and WAV
/// files named by their labels.
pub fn run(rig: &mut Rig, steps: &[Step], artifacts: &Path, deadline: std::time::Instant) -> Result<Outcome, (String, Outcome)> {
    let mut out = Outcome::default();
    for (i, step) in steps.iter().enumerate() {
        if std::time::Instant::now() > deadline {
            return Err((format!("step {} ({:?}): out of wall-clock time", i, step), out));
        }
        let result: Result<(), String> = (|| {
            match step {
                Step::Wait(ms) => rig.run(*ms),
                Step::Cycles(n) => rig.machines.iter_mut().for_each(|m| m.set_cycles(*n)),
                Step::Focus(i) => {
                    if *i >= rig.machines.len() {
                        return Err(format!("there are {} machines", rig.machines.len()));
                    }
                    rig.focus = *i;
                }
                Step::Key(name) => {
                    let k = key(name)?;
                    rig.press(k, 0, 60, 60);
                }
                Step::Keys(names, gap) => {
                    for name in names.iter() {
                        let k = key(name)?;
                        rig.press(k, 0, 60, gap.saturating_sub(60));
                    }
                }
                Step::Hold(name, ms) => {
                    let k = key(name)?;
                    rig.press(k, 0, *ms, 60);
                }
                Step::Type(text) => rig.type_text(text)?,
                Step::PressUntil(name, cond, gap, n) => {
                    let k = key(name)?;
                    let mut presses = 0;
                    while !cond.holds(rig.m()) {
                        if presses == *n || rig.exited() {
                            return Err(format!("{:?} didn't come about after {} presses", cond, n));
                        }
                        rig.press(k, 0, 60, gap.saturating_sub(60));
                        presses += 1;
                    }
                }
                Step::WaitFor(cond, ms) => {
                    let until = rig.ms() + ms;
                    while !cond.holds(rig.m()) {
                        if rig.ms() >= until || rig.exited() {
                            return Err(format!("{:?} didn't come about in {} ms", cond, ms));
                        }
                        if std::time::Instant::now() > deadline {
                            return Err("out of wall-clock time".into());
                        }
                        rig.run(POLL_MS);
                    }
                }
                Step::Check(cond) => {
                    if !cond.holds(rig.m()) {
                        return Err(format!("{:?} doesn't hold", cond));
                    }
                }
                Step::Shot(label) => {
                    let frame = rig.m().picture();
                    let _ = rust_dos::capture::png::save(&frame, &artifacts.join(format!("{}.png", label)));
                    out.notes.push(format!("{}: {}x{}, {} colours", label, frame.width, frame.height, machine::colours(&frame)));
                    out.checkpoints.push(Checkpoint { label: label.to_string(), hash: machine::hash_frame(&frame) });
                }
                Step::Listen(label, ms) | Step::Record(label, ms) => {
                    let samples = rig.listen(*ms);
                    save_wav(&artifacts.join(format!("{}.wav", label)), &samples);
                    let peak = machine::peak(&samples);
                    out.notes.push(format!("{}: peak {}", label, peak));
                    out.checkpoints.push(Checkpoint { label: label.to_string(), hash: machine::hash_samples(&samples) });
                    if matches!(step, Step::Listen(..)) && peak == 0 {
                        return Err("silence".into());
                    }
                }
            }
            Ok(())
        })();
        if let Err(why) = result {
            return Err((format!("step {} ({:?}) at {} ms: {}", i, step, rig.ms(), why), out));
        }
    }
    Ok(out)
}

fn save_wav(path: &Path, samples: &[i16]) {
    if let Ok(mut w) = rust_dos::capture::wav::WavWriter::create(path) {
        let _ = w.write(samples);
        let _ = w.finish();
    }
}
