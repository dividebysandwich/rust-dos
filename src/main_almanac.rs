//! `almanac_auto` in the rust-dos program: while a program from the prompt
//! waits (almanac.rs's `ask_first`), the almanac is asked about its file
//! on a thread of its own, and its settings are put in place before it
//! starts. Once it has ended, the settings before it come back, as after
//! a game launched from its profile.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, TryRecvError};

use crate::config::Settings;
use crate::config_file::config_warning;
use crate::config_ui::Host;
use crate::main_host::MainHost;
use rust_dos::almanac::{self, Candidate};

type Answer = Result<Vec<Candidate>, String>;

#[derive(Default)]
pub(crate) struct AutoApply {
    /// The question under way: the program's file, its checksum and where
    /// the answer comes.
    asking: Option<(String, String, Receiver<Answer>, std::time::Instant)>,
    /// The answers so far, by the file's MD5, for programs started again.
    known: HashMap<String, Vec<Candidate>>,
    /// The settings before the program that plays with the almanac's,
    /// and `Cpu::programs_loaded` as it was about to start.
    applied: Option<(Settings, u64)>,
}

impl AutoApply {
    /// Every frame: ask about a program that waits, take the answer, and
    /// put the settings back after the program. Returns what to tell the
    /// user over the picture.
    pub(crate) fn step(&mut self, host: &mut MainHost, window_open: bool) -> Option<String> {
        if host.game.is_some() {
            // A game launched from its profile has the settings now, and
            // puts back the configuration's as it ends.
            self.applied = None;
        }
        host.cpu.almanac.auto = host.settings.almanac_auto
            && host.game.is_none()
            && self.applied.is_none()
            && host.deterministic.is_none();

        if let Some((base, before)) = &self.applied
            && !window_open
            && rust_dos::games::ended(host.cpu, *before)
        {
            let base = base.clone();
            self.applied = None;
            if let Err(e) = host.apply(&base) {
                config_warning(host.cpu, &e);
            }
            return Some("The program has ended: your settings are back".to_string());
        }

        let waiting = almanac::waiting(host.cpu).map(str::to_string)?;
        if host.cpu.almanac.answer.is_some() {
            return None;
        }
        match &self.asking {
            Some((program, _, _, _)) if *program == waiting => {}
            _ => {
                let Some(program) = almanac::program(host.cpu, &waiting) else {
                    host.cpu.almanac.answer =
                        Some("The program can't be read: your own settings".to_string());
                    return None;
                };
                let md5 = program
                    .files
                    .first()
                    .and_then(|f| f.md5.clone())
                    .unwrap_or_default();
                if let Some(candidates) = self.known.get(&md5).cloned() {
                    self.asking = None;
                    return self.answered(host, Ok(candidates));
                }
                let (done, answer) = std::sync::mpsc::channel();
                let files = program.files;
                let spawned = std::thread::Builder::new()
                    .name("almanac".to_string())
                    .spawn(move || {
                        let _ = done.send(almanac::candidates(&files, "", false));
                    });
                if let Err(e) = spawned {
                    host.cpu.almanac.answer = Some(format!("Can't ask {}: {}", almanac::SITE, e));
                    return None;
                }
                self.asking = Some((waiting, md5, answer, std::time::Instant::now()));
                return None;
            }
        }
        let (_, md5, answer, since) = self.asking.as_ref()?;
        let result = match answer.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) if since.elapsed() < almanac::WAIT => return None,
            Err(TryRecvError::Empty) => Err(format!("{} took too long", almanac::SITE)),
            Err(TryRecvError::Disconnected) => Err("the question stopped".to_string()),
        };
        if let Ok(candidates) = &result {
            self.known.insert(md5.clone(), candidates.clone());
        }
        self.asking = None;
        self.answered(host, result)
    }

    /// The almanac's answer for the program waiting: its best settings
    /// over the current ones, and the program goes on.
    fn answered(&mut self, host: &mut MainHost, result: Answer) -> Option<String> {
        let candidates = match result {
            Ok(candidates) => candidates,
            Err(e) => {
                host.cpu.bus.log_string(&format!("[ALMANAC] {}", e));
                host.cpu.almanac.answer = Some(format!(
                    "No answer from {}: your own settings",
                    almanac::SITE
                ));
                return None;
            }
        };
        let Some(best) = almanac::best(&candidates) else {
            let text = match candidates.iter().find(|c| c.hash_match) {
                Some(c) => format!("The almanac has {}, but no settings for it yet", c.name),
                None => "The almanac doesn't know this program: your own settings".to_string(),
            };
            host.cpu.almanac.answer = Some(text);
            return None;
        };
        let recommendation = best.recommendation.as_ref()?;
        let base = host.settings.clone();
        let home = rust_dos::hostdirs::home_dir();
        let layered = almanac::layer(&base, &recommendation.usable(), home.as_deref());
        let about = format!(
            "{}'s settings from the almanac ({})",
            best.name,
            recommendation.describe()
        );
        host.cpu.bus.log_string(&format!(
            "[ALMANAC] {} starts with {}",
            waiting_name(host),
            about
        ));
        match host.apply(&layered) {
            Ok(Some(note)) => host.cpu.bus.log_string(&format!("[ALMANAC] {}", note)),
            Ok(None) => {}
            Err(e) => config_warning(host.cpu, &e),
        }
        self.applied = Some((base, host.cpu.programs_loaded));
        host.cpu.almanac.answer = Some(format!("With {}", about));
        Some(format!("With {}", about))
    }
}

/// The file of the program waiting, for the log.
fn waiting_name(host: &MainHost) -> String {
    almanac::waiting(host.cpu).map_or_else(String::new, |path| {
        path.rsplit(['\\', ':']).next().unwrap_or(path).to_string()
    })
}
