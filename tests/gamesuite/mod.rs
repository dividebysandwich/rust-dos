//! The game regression suite: shareware and freeware DOS games, downloaded
//! and installed once, played by scripts in emulated time on machines set
//! up in many ways, their pictures and sounds checked against the hashes
//! recorded before (`golden/`). See docs/game-suite.md.

pub mod catalog;
pub mod fetch;
pub mod machine;
pub mod network;
pub mod script;

use script::{Checkpoint, Step};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// How a game gets from its archives onto C:.
pub enum Install {
    /// Unpacked as they are, leaving out the folder `strip` the files are
    /// in.
    Unpack { strip: &'static str },
    /// Unpacked onto D:, and the game's installer run there onto an empty
    /// C: by `steps`, after which C: must have the files `wants`.
    Installer { ini: &'static str, steps: Vec<Step>, wants: &'static [&'static str] },
}

pub struct Game {
    pub id: &'static str,
    pub archives: Vec<fetch::Archive>,
    pub install: Install,
}

pub struct Scenario {
    /// `game/variant`.
    pub name: String,
    pub game: &'static str,
    /// The golden file the hashes are checked against; variants that must
    /// come out the same (the interpreter and the recompiler) share one.
    /// None for scenarios that depend on the host's timing.
    pub golden: Option<String>,
    /// The configuration: C: is a fresh copy of the installed game.
    pub ini: String,
    /// Files written over the game's, by DOS path: its settings.
    pub files: Vec<(&'static str, Vec<u8>)>,
    pub steps: Vec<Step>,
    /// Wall-clock time it may take.
    pub timeout: Duration,
    /// A known failure, with why.
    pub expect_fail: Option<&'static str>,
    /// A second machine's configuration, for games played over a network
    /// or a serial link: both machines join a LAN room on a relay of
    /// their own.
    pub partner: Option<String>,
}

impl Scenario {
    pub fn new(game: &'static str, variant: &str, ini: String, steps: Vec<Step>) -> Self {
        let name = format!("{}/{}", game, variant);
        Scenario {
            golden: Some(name.clone()),
            name,
            game,
            ini,
            files: Vec::new(),
            steps,
            timeout: Duration::from_secs(300),
            expect_fail: None,
            partner: None,
        }
    }

    /// Played on two linked machines, the second with the configuration
    /// `ini`. Their timing follows the relay's, on the host's clock: no
    /// golden hashes.
    pub fn partner(mut self, ini: String) -> Self {
        self.partner = Some(ini);
        self.golden = None;
        self
    }

    pub fn golden(mut self, key: &str) -> Self {
        self.golden = Some(format!("{}/{}", self.game, key));
        self
    }

    pub fn file(mut self, name: &'static str, bytes: impl Into<Vec<u8>>) -> Self {
        self.files.push((name, bytes.into()));
        self
    }

    pub fn expect_fail(mut self, why: &'static str) -> Self {
        self.expect_fail = Some(why);
        self
    }

}

/// Where a run's pictures, sounds and logs go.
pub fn artifacts_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/game_suite")
}

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/gamesuite/golden")
}

fn file_name(name: &str) -> String {
    name.replace('/', "__")
}

/// `GAME_SUITE_SNAP`: save the picture every so many emulated
/// milliseconds.
fn snap_every() -> Option<u64> {
    std::env::var("GAME_SUITE_SNAP").ok()?.parse().ok().filter(|ms| *ms > 0)
}

/// The game installed in the cache: unpacked, or installed by its
/// installer the first time.
pub fn install(game: &Game) -> Result<PathBuf, String> {
    let mut paths = Vec::new();
    for archive in &game.archives {
        paths.push(fetch::ensure(archive)?);
    }
    let tag = &game.archives[0].sha256[..8];
    let dest = fetch::cache_dir().join("installed").join(format!("{}-{}", game.id, tag));
    if dest.is_dir() {
        return Ok(dest);
    }
    let work = fetch::cache_dir().join("installed").join(format!("{}-{}.tmp", game.id, tag));
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    match &game.install {
        Install::Unpack { strip } => {
            for path in &paths {
                fetch::unzip(path, &work, strip)?;
            }
        }
        Install::Installer { ini, steps, wants } => {
            let source = fetch::cache_dir().join("source").join(game.id);
            let _ = fs::remove_dir_all(&source);
            fs::create_dir_all(&source).map_err(|e| e.to_string())?;
            for path in &paths {
                fetch::unzip(path, &source, "")?;
            }
            let ini = format!("{}\n[drives]\nd = {}\n", ini, source.display());
            let artifacts = artifacts_root().join(format!("install__{}", game.id));
            let _ = fs::remove_dir_all(&artifacts);
            fs::create_dir_all(&artifacts).map_err(|e| e.to_string())?;
            let mut m = machine::Machine::new(&work, &ini, Some(&artifacts.join("rust-dos.log")))?;
            m.snap = snap_every().map(|every| (artifacts.clone(), every));
            let deadline = Instant::now() + Duration::from_secs(600);
            script::run(&mut machine::Rig::new(vec![m]), steps, &artifacts, deadline)
                .map_err(|(why, _)| format!("installing {}: {}", game.id, why))?;
            for want in wants.iter() {
                if !script::exists_nocase(&work, want) {
                    return Err(format!("installing {}: no {} afterwards", game.id, want));
                }
            }
        }
    }
    fs::rename(&work, &dest).map_err(|e| e.to_string())?;
    Ok(dest)
}

pub enum Verdict {
    Pass,
    Fail(String),
    /// Failed as it is known to.
    KnownFail(String),
    /// Known to fail, but passed.
    UnexpectedPass,
}

pub struct Report {
    pub name: String,
    pub verdict: Verdict,
    pub checkpoints: Vec<Checkpoint>,
    pub notes: Vec<String>,
    pub emulated_ms: u64,
    pub wall: Duration,
}

thread_local! {
    /// Where the last panic on this thread was, as the panic hook saw it.
    static PANIC_AT: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// Keep panics' places for the report rather than printing them.
fn catch_panic_places() {
    std::panic::set_hook(Box::new(|info| {
        let at = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        PANIC_AT.with(|p| *p.borrow_mut() = at);
    }));
}

/// Run a scenario, with the emulator panicking as one of the ways for it
/// to fail.
fn guarded(scenario: &Scenario, installed: &Path) -> Report {
    let started = Instant::now();
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_scenario(scenario, installed))) {
        Ok(report) => report,
        Err(panic) => {
            let message = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            let why = format!("{} at {}", message, PANIC_AT.with(|p| p.borrow().clone()));
            Report {
                name: scenario.name.clone(),
                verdict: Verdict::Fail(format!("the emulator panicked: {}", why)),
                checkpoints: Vec::new(),
                notes: Vec::new(),
                emulated_ms: 0,
                wall: started.elapsed(),
            }
        }
    }
}

/// Run a scenario on a fresh copy of its game.
pub fn run_scenario(scenario: &Scenario, installed: &Path) -> Report {
    let started = Instant::now();
    let artifacts = artifacts_root().join(file_name(&scenario.name));
    let mut report = Report {
        name: scenario.name.clone(),
        verdict: Verdict::Pass,
        checkpoints: Vec::new(),
        notes: Vec::new(),
        emulated_ms: 0,
        wall: Duration::ZERO,
    };
    let result = (|| -> Result<(), String> {
        let _ = fs::remove_dir_all(&artifacts);
        let c = artifacts.join("c");
        fetch::copy_tree(installed, &c)?;
        for (name, bytes) in &scenario.files {
            fetch::write_nocase(&c, name, bytes)?;
        }
        let m = machine::Machine::new(&c, &scenario.ini, Some(&artifacts.join("rust-dos.log")))?;
        let mut rig = machine::Rig::new(vec![m]);
        rig.m().snap = snap_every().map(|every| (artifacts.clone(), every));
        // Kept until the machines are done with it.
        let _relay = match &scenario.partner {
            Some(ini) => {
                let c2 = artifacts.join("c2");
                fetch::copy_tree(installed, &c2)?;
                for (name, bytes) in &scenario.files {
                    fetch::write_nocase(&c2, name, bytes)?;
                }
                let log = artifacts.join("rust-dos-2.log");
                let mut m2 = machine::Machine::with_clock(&c2, ini, Some(&log), machine::PARTNER_CLOCK_MS)?;
                m2.snap = snap_every().map(|every| {
                    let dir = artifacts.join("partner");
                    let _ = fs::create_dir_all(&dir);
                    (dir, every)
                });
                rig.machines.push(m2);
                Some(network::link(&mut rig)?)
            }
            None => None,
        };
        let outcome = script::run(&mut rig, &scenario.steps, &artifacts, started + scenario.timeout);
        let m = &mut rig.machines[0];
        report.emulated_ms = m.ms;
        report.notes.extend(m.notable.borrow().iter().map(|l| format!("log: {}", l)));
        if let Some(text) = m.screen_text() {
            let _ = fs::write(artifacts.join("screen.txt"), text);
        }
        let _ = rust_dos::capture::png::save(&m.picture(), &artifacts.join("last.png"));
        match outcome {
            Ok(out) => {
                report.checkpoints = out.checkpoints;
                report.notes.splice(0..0, out.notes);
                Ok(())
            }
            Err((why, out)) => {
                report.checkpoints = out.checkpoints;
                report.notes.splice(0..0, out.notes);
                Err(why)
            }
        }
    })();
    if let Err(why) = result {
        report.verdict = Verdict::Fail(why);
    }
    report.wall = started.elapsed();
    report
}

fn read_golden(key: &str) -> BTreeMap<String, String> {
    let path = golden_dir().join(format!("{}.txt", file_name(key)));
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.split_once(' ').map(|(l, h)| (l.to_string(), h.trim().to_string())))
        .collect()
}

fn write_golden(key: &str, checkpoints: &[Checkpoint]) {
    let dir = golden_dir();
    fs::create_dir_all(&dir).unwrap();
    let text: String = checkpoints.iter().map(|c| format!("{} {}\n", c.label, c.hash)).collect();
    fs::write(dir.join(format!("{}.txt", file_name(key))), text).unwrap();
}

/// Check the reports' hashes against the golden files, or record them
/// (`bless`). Scenarios sharing a golden file must agree with each other.
fn judge(scenarios: &[Scenario], reports: &mut [Report], bless: bool) {
    let mut blessed: BTreeMap<String, (String, Vec<Checkpoint>)> = BTreeMap::new();
    for (scenario, report) in scenarios.iter().zip(reports.iter_mut()) {
        if let Some(key) = &scenario.golden
            && matches!(report.verdict, Verdict::Pass)
        {
            if bless {
                match blessed.get(key) {
                    Some((first, theirs)) => {
                        if let Some(why) = differences(theirs, &report.checkpoints) {
                            report.verdict = Verdict::Fail(format!("differs from {}: {}", first, why));
                        }
                    }
                    None => {
                        blessed.insert(key.clone(), (scenario.name.clone(), report.checkpoints.clone()));
                        keep_blessed(key, &scenario.name);
                    }
                }
            } else {
                let golden = read_golden(key);
                let expected: Vec<Checkpoint> =
                    golden.iter().map(|(label, hash)| Checkpoint { label: label.clone(), hash: hash.clone() }).collect();
                if golden.is_empty() {
                    report.verdict = Verdict::Fail(format!("no golden hashes for {} (run with GAME_SUITE_BLESS=1)", key));
                } else if let Some(why) = differences(&expected, &report.checkpoints) {
                    show_blessed(key, &scenario.name);
                    report.verdict = Verdict::Fail(why);
                }
            }
        }
        report.verdict = match (std::mem::replace(&mut report.verdict, Verdict::Pass), scenario.expect_fail) {
            (Verdict::Fail(why), Some(_)) => Verdict::KnownFail(why),
            (Verdict::Pass, Some(_)) => Verdict::UnexpectedPass,
            (verdict, _) => verdict,
        };
    }
    if bless {
        for (key, (_, checkpoints)) in &blessed {
            write_golden(key, checkpoints);
        }
    }
}

/// Where the pictures and sounds the golden file `key` was recorded from
/// are kept: in the cache, as the games' own pictures don't go in the
/// repository.
fn blessed_dir(key: &str) -> PathBuf {
    fetch::cache_dir().join("blessed").join(file_name(key))
}

/// Keep the scenario's pictures and sounds as the ones its golden file was
/// recorded from.
fn keep_blessed(key: &str, scenario: &str) {
    let dir = blessed_dir(key);
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::create_dir_all(&dir);
    if let Ok(entries) = fs::read_dir(artifacts_root().join(file_name(scenario))) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if (name.ends_with(".png") || name.ends_with(".wav")) && !name.starts_with("snap-") && name != "last.png" {
                let _ = fs::copy(entry.path(), dir.join(&name));
            }
        }
    }
}

/// Put the recorded pictures and sounds next to a failed run's, as
/// `<label>.expected.png` and `.wav`, to compare.
fn show_blessed(key: &str, scenario: &str) {
    let artifacts = artifacts_root().join(file_name(scenario));
    if let Ok(entries) = fs::read_dir(blessed_dir(key)) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let (Some(stem), Some(ext)) = (path.file_stem(), path.extension()) {
                let target = artifacts.join(format!("{}.expected.{}", stem.to_string_lossy(), ext.to_string_lossy()));
                let _ = fs::copy(&path, target);
            }
        }
    }
}

/// How `got` differs from `expected`, by label.
fn differences(expected: &[Checkpoint], got: &[Checkpoint]) -> Option<String> {
    let want: BTreeMap<&str, &str> = expected.iter().map(|c| (c.label.as_str(), c.hash.as_str())).collect();
    let have: BTreeMap<&str, &str> = got.iter().map(|c| (c.label.as_str(), c.hash.as_str())).collect();
    let mut diffs = Vec::new();
    for (label, hash) in &want {
        match have.get(label) {
            Some(h) if h == hash => {}
            Some(_) => diffs.push(format!("{} changed", label)),
            None => diffs.push(format!("{} missing", label)),
        }
    }
    for label in have.keys().filter(|l| !want.contains_key(*l)) {
        diffs.push(format!("{} is new", label));
    }
    (!diffs.is_empty()).then(|| diffs.join(", "))
}

/// The scenarios the filter `GAME_SUITE` picks (a part of their name, or
/// several separated by commas), all without one.
pub fn selected(all: Vec<Scenario>) -> Vec<Scenario> {
    match std::env::var("GAME_SUITE") {
        Ok(filter) if !filter.is_empty() => {
            let parts: Vec<&str> = filter.split(',').collect();
            all.into_iter().filter(|s| parts.iter().any(|p| s.name.contains(p))).collect()
        }
        _ => all,
    }
}

fn jobs() -> usize {
    std::env::var("GAME_SUITE_JOBS")
        .ok()
        .and_then(|j| j.parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4))
        .max(1)
}

/// Install the scenarios' games, run the scenarios a few at a time, judge
/// them and write the report. Panics if any failed.
pub fn run_all(scenarios: Vec<Scenario>) {
    let bless = std::env::var("GAME_SUITE_BLESS").is_ok_and(|v| v == "1");
    let games = catalog::games();
    let mut installed: BTreeMap<&str, Result<PathBuf, String>> = BTreeMap::new();
    for scenario in &scenarios {
        if !installed.contains_key(scenario.game) {
            let game = games.iter().find(|g| g.id == scenario.game).expect("the scenario's game is in the catalog");
            eprintln!("[game-suite] installing {}", game.id);
            installed.insert(scenario.game, install(game));
        }
    }

    catch_panic_places();
    let next = AtomicUsize::new(0);
    let reports: Mutex<Vec<Option<Report>>> = Mutex::new((0..scenarios.len()).map(|_| None).collect());
    std::thread::scope(|scope| {
        for _ in 0..jobs().min(scenarios.len()) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some(scenario) = scenarios.get(i) else { break };
                    let report = match &installed[scenario.game] {
                        Ok(dir) => guarded(scenario, dir),
                        Err(why) => Report {
                            name: scenario.name.clone(),
                            verdict: Verdict::Fail(why.clone()),
                            checkpoints: Vec::new(),
                            notes: Vec::new(),
                            emulated_ms: 0,
                            wall: Duration::ZERO,
                        },
                    };
                    eprintln!("[game-suite] {} {} ({:.1} s)", verdict_word(&report.verdict), report.name, report.wall.as_secs_f64());
                    reports.lock().unwrap()[i] = Some(report);
                }
            });
        }
    });
    let mut reports: Vec<Report> = reports.into_inner().unwrap().into_iter().map(Option::unwrap).collect();
    judge(&scenarios, &mut reports, bless);
    let _ = std::panic::take_hook();
    let failed = write_report(&scenarios, &reports, bless);
    assert!(failed == 0, "{} of {} scenarios failed; see {}", failed, reports.len(), artifacts_root().join("report.md").display());
}

fn verdict_word(verdict: &Verdict) -> &'static str {
    match verdict {
        Verdict::Pass => "PASS",
        Verdict::Fail(_) => "FAIL",
        Verdict::KnownFail(_) => "known-fail",
        Verdict::UnexpectedPass => "UNEXPECTED-PASS",
    }
}

/// Print the summary and write `report.md`. Returns how many failed.
fn write_report(scenarios: &[Scenario], reports: &[Report], bless: bool) -> usize {
    let mut md = String::from("# Game suite\n\n| Scenario | Result | Emulated | Wall | Details |\n|---|---|---|---|---|\n");
    let mut failed = 0;
    eprintln!("\n{:<44} {:<16} {:>9} {:>7}", "scenario", "result", "emulated", "wall");
    for (scenario, report) in scenarios.iter().zip(reports) {
        let detail = match &report.verdict {
            Verdict::Fail(why) => {
                failed += 1;
                why.clone()
            }
            Verdict::KnownFail(why) => format!("{} — {}", scenario.expect_fail.unwrap_or(""), why),
            Verdict::UnexpectedPass => {
                failed += 1;
                format!("known to fail ({}), but passed", scenario.expect_fail.unwrap_or(""))
            }
            Verdict::Pass => String::new(),
        };
        let word = verdict_word(&report.verdict);
        let emulated = format!("{:.1} s", report.emulated_ms as f64 / 1000.0);
        let wall = format!("{:.1} s", report.wall.as_secs_f64());
        eprintln!("{:<44} {:<16} {:>9} {:>7} {}", report.name, word, emulated, wall, detail);
        let notes = report.notes.join("<br>").replace('|', "\\|");
        md.push_str(&format!(
            "| {} | {} | {} | {} | {}{}{} |\n",
            report.name,
            word,
            emulated,
            wall,
            detail.replace('|', "\\|"),
            if detail.is_empty() || notes.is_empty() { "" } else { "<br>" },
            notes
        ));
    }
    if bless {
        md.push_str("\nGolden hashes were recorded (GAME_SUITE_BLESS=1).\n");
    }
    let _ = fs::create_dir_all(artifacts_root());
    let _ = fs::write(artifacts_root().join("report.md"), md);
    failed
}
