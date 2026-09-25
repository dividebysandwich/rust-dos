//! Conformance run of the SingleStepTests/80386 real-mode suite
//! (https://github.com/SingleStepTests/80386, `v1_ex_real_mode`), captured
//! from a real 80386EX: every test sets the registers and memory, runs one
//! instruction and the HLT after it, and checks the final registers and
//! memory against the hardware's.
//!
//! Opt-in and ignored by default; see docs/cpu-tests.md. Fetch the suite
//! with `tests/sst386/fetch.sh`, then:
//!
//! ```sh
//! SST386_DIR=target/sst386/v1_ex_real_mode \
//!   cargo test --release --test sst386 -- --ignored --nocapture
//! ```
//!
//! Environment:
//! - `SST386_DIR`: directory of `*.MOO.gz` files (required; the test is
//!   skipped when unset). `revocation_list.txt` and `80386.csv` are read
//!   from it or its parent.
//! - `SST386_FILTER`: comma-separated substrings of the file names to run,
//!   e.g. `0FB6` or `80.4,C1.`.
//! - `SST386_STRICT=1`: fail unless every test that ran passed. Otherwise
//!   the run only reports, as a progress meter.
//! - `SST386_SAMPLES`: failure details kept per file (default 3).
//! - `SST386_THREADS`: worker threads (default: all cores).
//!
//! The report goes to stdout and `target/sst386-report.txt`.

#[path = "sst386/machine.rs"]
mod machine;
#[path = "sst386/moo.rs"]
mod moo;

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use machine::{Machine, RunEnd};
use moo::{Reader, Regs, Test};

/// Instructions a test may run before it counts as not halting. A REP
/// string instruction has CX < 128, so this leaves room for a CPU that
/// steps one iteration at a time.
const STEP_BUDGET: usize = 256;

/// Opcode files that are never run, and why.
const SKIPPED_FILES: &[(&str, &str)] = &[
    ("FE.7", "FE 38/FE 39 are the emulator's service traps (BOPs)"),
    // The suite reads FFh from every port; the emulator's devices answer.
    ("E4", PORT_INPUT),
    ("E5", PORT_INPUT),
    ("66E5", PORT_INPUT),
    ("EC", PORT_INPUT),
    ("ED", PORT_INPUT),
    ("66ED", PORT_INPUT),
    ("6C", PORT_INPUT),
    ("676C", PORT_INPUT),
    ("6D", PORT_INPUT),
    ("676D", PORT_INPUT),
    ("666D", PORT_INPUT),
    ("67666D", PORT_INPUT),
];

const PORT_INPUT: &str = "port input: the suite reads FFh where the emulator's devices answer";

/// Why a single test was not run to completion.
#[derive(Clone, Copy)]
enum Skip {
    Revoked,
    BeyondRam,
    Tripwire,
    ServiceTrap,
}

const SKIP_KINDS: usize = 4;
const SKIP_NAMES: [&str; SKIP_KINDS] = [
    "revoked",
    "address beyond emulated RAM",
    "tripwire",
    "service trap at CS:IP",
];

struct Config {
    dir: PathBuf,
    filters: Vec<String>,
    /// `SST386_INDEX`: run only the test with this index in each file.
    index: Option<u32>,
    strict: bool,
    samples: usize,
    threads: usize,
    revoked: HashSet<[u8; 20]>,
    /// `f_umask` from 80386.csv by (opcode, ModR/M extension).
    flag_masks: HashMap<(String, Option<u8>), u32>,
}

/// An opcode file name such as `67660FBA.4.MOO.gz`, taken apart.
struct OpcodeName {
    /// `67660FBA.4`
    stem: String,
    /// Size override prefixes as written, `6766`.
    prefixes: String,
    /// `0FBA`
    opcode: String,
    /// ModR/M reg field of group opcodes, `4`.
    ext: Option<u8>,
}

impl OpcodeName {
    fn parse(file_name: &str) -> Self {
        let stem = file_name
            .trim_end_matches(".gz")
            .trim_end_matches(".MOO")
            .to_string();
        let (op, ext) = match stem.split_once('.') {
            Some((op, ext)) => (op, ext.parse().ok()),
            None => (stem.as_str(), None),
        };
        let mut rest = op;
        let mut prefixes = String::new();
        while rest.len() > 2 && (rest.starts_with("66") || rest.starts_with("67")) {
            prefixes.push_str(&rest[..2]);
            rest = &rest[2..];
        }
        Self {
            opcode: rest.to_string(),
            prefixes,
            ext,
            stem,
        }
    }

    /// Opcode, then extension, then prefixes: the forms of one instruction
    /// sort next to each other.
    fn sort_key(&self) -> (String, Option<u8>, String) {
        (self.opcode.clone(), self.ext, self.prefixes.clone())
    }

    /// Opcode family for the summary: size prefixes and opcode map.
    fn family(&self) -> String {
        let prefixes = match self.prefixes.as_str() {
            "" => "no prefix",
            "66" => "66 (operand size)",
            "67" => "67 (address size)",
            _ => "67 66 (both)",
        };
        let map = if self.opcode.starts_with("0F") {
            "0F xx"
        } else {
            "one-byte"
        };
        format!("{map:<9} {prefixes}")
    }
}

#[derive(Default)]
struct FileResult {
    stem: String,
    mnemonic: String,
    tests: u32,
    passed: u32,
    failed: u32,
    panicked: u32,
    skipped: [u32; SKIP_KINDS],
    /// The whole file was skipped, and why.
    file_skip: Option<&'static str>,
    /// The file could not be read.
    error: Option<String>,
    /// Details of the first failures.
    samples: Vec<String>,
}

impl FileResult {
    fn skipped_total(&self) -> u32 {
        self.skipped.iter().sum()
    }

    fn ran(&self) -> u32 {
        self.passed + self.failed + self.panicked
    }

    fn skip_summary(&self) -> String {
        if let Some(reason) = self.file_skip {
            return format!("file skipped: {reason}");
        }
        let mut parts: Vec<String> = SKIP_NAMES
            .iter()
            .zip(self.skipped)
            .filter(|&(_, n)| n > 0)
            .map(|(name, n)| format!("{n} {name}"))
            .collect();
        if let Some(e) = &self.error {
            parts.push(format!("ERROR: {e}"));
        }
        parts.join(", ")
    }
}

fn env_list(name: &str) -> Vec<String> {
    std::env::var(name)
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// `name` in `dir` or its parent.
fn find_beside(dir: &Path, name: &str) -> Option<PathBuf> {
    [Some(dir), dir.parent()]
        .into_iter()
        .flatten()
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

fn load_revocations(dir: &Path) -> HashSet<[u8; 20]> {
    let Some(path) = find_beside(dir, "revocation_list.txt") else {
        eprintln!("[sst386] no revocation_list.txt next to {}", dir.display());
        return HashSet::new();
    };
    std::fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter_map(moo::parse_hash)
        .collect()
}

/// Split a CSV line, honouring double quotes.
fn csv_fields(line: &str) -> Vec<String> {
    let mut fields = vec![String::new()];
    let mut quoted = false;
    for c in line.chars() {
        match c {
            '"' => quoted = !quoted,
            ',' if !quoted => fields.push(String::new()),
            _ => fields.last_mut().unwrap().push(c),
        }
    }
    fields
}

/// Undefined-flag masks (`f_umask`, 1 = defined) from 80386.csv.
fn load_flag_masks(dir: &Path) -> HashMap<(String, Option<u8>), u32> {
    let mut masks = HashMap::new();
    let Some(path) = find_beside(dir, "80386.csv") else {
        eprintln!("[sst386] no 80386.csv next to {}", dir.display());
        return masks;
    };
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut lines = text.lines();
    let header = csv_fields(lines.next().unwrap_or_default());
    let col = |name: &str| header.iter().position(|h| h == name);
    let (Some(op), Some(ex), Some(umask)) = (col("op"), col("ex"), col("f_umask")) else {
        eprintln!("[sst386] 80386.csv lacks op/ex/f_umask columns");
        return masks;
    };
    for line in lines {
        let f = csv_fields(line);
        let (Some(o), Some(e), Some(m)) = (f.get(op), f.get(ex), f.get(umask)) else {
            continue;
        };
        let Ok(m) = u32::from_str_radix(m.trim_start_matches("0x"), 16) else {
            continue;
        };
        masks.insert((o.to_uppercase(), e.parse().ok()), 0xFFFF_0000 | m);
    }
    masks
}

impl Config {
    fn flag_mask(&self, name: &OpcodeName) -> u32 {
        let op = name.opcode.to_uppercase();
        self.flag_masks
            .get(&(op.clone(), name.ext))
            .or_else(|| self.flag_masks.get(&(op, None)))
            .copied()
            .unwrap_or(u32::MAX)
    }
}

const FLAG_NAMES: [(u32, &str); 17] = [
    (0x0001, "CF"),
    (0x0002, "bit1"),
    (0x0004, "PF"),
    (0x0008, "bit3"),
    (0x0010, "AF"),
    (0x0020, "bit5"),
    (0x0040, "ZF"),
    (0x0080, "SF"),
    (0x0100, "TF"),
    (0x0200, "IF"),
    (0x0400, "DF"),
    (0x0800, "OF"),
    (0x3000, "IOPL"),
    (0x4000, "NT"),
    (0x8000, "bit15"),
    (0x1_0000, "RF"),
    (0x2_0000, "VM"),
];

fn flag_names(bits: u32) -> String {
    FLAG_NAMES
        .iter()
        .filter(|(b, _)| bits & b != 0)
        .map(|(_, n)| *n)
        .collect::<Vec<_>>()
        .join(",")
}

/// Compare the machine against the test's final state. With `out`, the
/// differences are described there.
fn compare(
    m: &Machine,
    t: &Test,
    file_mask: &Regs,
    csv_flags: u32,
    mut out: Option<&mut String>,
) -> bool {
    let mut ok = true;
    for slot in machine::COMPARED {
        let (Some(want), Some(got)) = (t.expected(slot), m.reg(slot)) else {
            continue;
        };
        let mask = machine::defined_mask(slot, file_mask, &t.final_mask, csv_flags);
        let diff = (want ^ got) & mask;
        if diff == 0 {
            continue;
        }
        ok = false;
        let Some(out) = out.as_deref_mut() else {
            return false;
        };
        let name = moo::REG_NAMES[slot];
        let _ = if slot == moo::EFLAGS {
            write!(
                out,
                "; {name} got {:05X} want {:05X} ({})",
                got & machine::EFLAGS_386,
                want & machine::EFLAGS_386,
                flag_names(diff)
            )
        } else {
            write!(out, "; {name} got {got:X} want {want:X}")
        };
    }

    // Bytes of the FLAGS image an exception pushed may hold undefined
    // flags.
    let flags_mask = machine::defined_mask(moo::EFLAGS, file_mask, &t.final_mask, csv_flags);
    let byte_mask = |addr: u32| match t.exception {
        Some(e) if addr == e.flag_addr => flags_mask as u8,
        Some(e) if addr == e.flag_addr.wrapping_add(1) => (flags_mask >> 8) as u8,
        _ => 0xFF,
    };
    // The final state lists the bytes that changed; every other byte of
    // the initial state must be as it was.
    let unchanged = t
        .initial_ram
        .iter()
        .filter(|(a, _)| !t.final_ram.iter().any(|(f, _)| f == a));
    for (i, &(addr, want)) in t.final_ram.iter().chain(unchanged).enumerate() {
        let got = m.mem(addr);
        if (got ^ want) & byte_mask(addr) == 0 {
            continue;
        }
        ok = false;
        let Some(out) = out.as_deref_mut() else {
            return false;
        };
        let what = if i < t.final_ram.len() {
            ""
        } else {
            " (unchanged)"
        };
        let _ = write!(out, "; mem[{addr:06X}] got {got:02X} want {want:02X}{what}");
    }
    ok
}

/// One-line description of a failing test.
fn describe(t: &Test, m: &Machine, problem: &str, diffs: &str) -> String {
    let mut s = format!(
        "#{} '{}' [{}] hash {}: {}{}",
        t.index,
        t.name,
        t.bytes_hex(),
        t.hash_hex(),
        problem,
        diffs
    );
    if let Some(e) = t.exception {
        let _ = write!(s, " (test raises int {:02X}h)", e.number);
    }
    if std::env::var_os("SST386_VERBOSE").is_some() {
        let names = [
            ("eax", moo::EAX), ("ebx", moo::EBX), ("ecx", moo::ECX), ("edx", moo::EDX),
            ("esi", moo::ESI), ("edi", moo::EDI), ("ebp", moo::EBP), ("esp", moo::ESP),
            ("cs", moo::CS), ("ds", moo::DS), ("es", moo::ES), ("fs", moo::FS),
            ("gs", moo::GS), ("ss", moo::SS), ("eip", moo::EIP), ("eflags", moo::EFLAGS),
        ];
        let _ = write!(s, " | initial:");
        for (name, slot) in names {
            if let Some(v) = t.initial.get(slot) {
                let _ = write!(s, " {name}={v:X}");
            }
        }
    }
    let log = m.log.borrow();
    if let Some(line) = log.first() {
        let line: String = line.chars().take(160).collect();
        let _ = write!(s, " | log: {line}");
    }
    s
}

fn read_file(path: &Path) -> Result<Vec<u8>, String> {
    let raw = std::fs::read(path).map_err(|e| e.to_string())?;
    if !path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("gz"))
    {
        return Ok(raw);
    }
    let mut buf = Vec::with_capacity(raw.len() * 6);
    flate2::read::GzDecoder::new(raw.as_slice())
        .read_to_end(&mut buf)
        .map_err(|e| format!("gunzip: {e}"))?;
    Ok(buf)
}

fn run_file(m: &mut Machine, path: &Path, name: &OpcodeName, cfg: &Config) -> FileResult {
    let mut r = FileResult {
        stem: name.stem.clone(),
        ..Default::default()
    };
    if let Some(&(_, reason)) = SKIPPED_FILES.iter().find(|(s, _)| *s == name.stem) {
        r.file_skip = Some(reason);
        return r;
    }
    let data = match read_file(path) {
        Ok(d) => d,
        Err(e) => {
            r.error = Some(e);
            return r;
        }
    };
    let mut reader = match Reader::new(&data) {
        Ok(rd) => rd,
        Err(e) => {
            r.error = Some(e);
            return r;
        }
    };
    let header = &reader.header;
    if header.version.0 != 1 || header.cpu_id != "386E" {
        r.error = Some(format!(
            "not an 80386 MOO 1.x file (version {}.{}, CPU '{}')",
            header.version.0, header.version.1, header.cpu_id
        ));
        return r;
    }
    let csv_flags = cfg.flag_mask(name);
    let ram_len = m.ram_len() as u64;
    let mut t = Test::default();
    let mut diffs = String::new();
    loop {
        match reader.next_into(&mut t) {
            Ok(true) => {}
            Ok(false) => break,
            Err(e) => {
                r.error = Some(e);
                break;
            }
        }
        r.tests += 1;
        if cfg.revoked.contains(&t.hash) {
            r.skipped[Skip::Revoked as usize] += 1;
            continue;
        }
        let beyond = |ram: &[(u32, u8)]| ram.iter().any(|&(a, _)| a as u64 >= ram_len);
        if beyond(&t.initial_ram) || beyond(&t.final_ram) {
            r.skipped[Skip::BeyondRam as usize] += 1;
            continue;
        }

        if cfg.index.is_some_and(|i| i != t.index) {
            continue;
        }
        if std::env::var_os("SST386_DUMP").is_some() {
            // One line per test for offline analysis: name, then the
            // initial and expected register values.
            let slots = [
                moo::EAX, moo::EBX, moo::ECX, moo::EDX, moo::ESI, moo::EDI, moo::EBP, moo::ESP,
                moo::EIP, moo::EFLAGS,
            ];
            let fmt = |f: &dyn Fn(usize) -> Option<u32>| {
                slots.iter().map(|&s| format!("{:08X}", f(s).unwrap_or(0))).collect::<Vec<_>>().join(" ")
            };
            let ram: Vec<String> = t.initial_ram.iter().map(|(a, v)| format!("{a:06X}={v:02X}")).collect();
            let fram: Vec<String> = t.final_ram.iter().map(|(a, v)| format!("{a:06X}={v:02X}")).collect();
            eprintln!(
                "DUMP|{}|{}|{}|{}|{}|{}",
                t.name,
                fmt(&|s| t.initial.get(s)),
                fmt(&|s| t.expected(s)),
                ram.join(" "),
                fram.join(" "),
                t.exception.map_or(-1, |e| e.number as i32)
            );
        }
        if cfg.index.is_some() {
            let ram: Vec<String> = t.initial_ram.iter().map(|(a, v)| format!("{a:06X}={v:02X}")).collect();
            eprintln!("[sst386] test #{} initial ram: {}", t.index, ram.join(" "));
        }

        m.load(&t);
        let end = m.run(STEP_BUDGET);
        let problem = match end {
            RunEnd::Halted => None,
            RunEnd::NoHalt => Some(format!("no HLT within {STEP_BUDGET} instructions")),
            RunEnd::Tripwire => {
                r.skipped[Skip::Tripwire as usize] += 1;
                m.clean();
                continue;
            }
            RunEnd::ServiceTrap { steps: 0 } => {
                r.skipped[Skip::ServiceTrap as usize] += 1;
                m.clean();
                continue;
            }
            RunEnd::ServiceTrap { steps } => Some(format!(
                "ran into an emulator service trap after {steps} instructions"
            )),
            RunEnd::Panic(msg) => {
                r.panicked += 1;
                if r.samples.len() < cfg.samples {
                    r.samples
                        .push(describe(&t, m, &format!("PANIC: {msg}"), ""));
                }
                m.clean();
                continue;
            }
        };
        let file_mask = reader.file_mask;
        if problem.is_none() && compare(m, &t, &file_mask, csv_flags, None) {
            r.passed += 1;
        } else {
            r.failed += 1;
            if r.samples.len() < cfg.samples {
                diffs.clear();
                compare(m, &t, &file_mask, csv_flags, Some(&mut diffs));
                let problem = problem.unwrap_or_default();
                let diffs = if problem.is_empty() {
                    diffs.trim_start_matches("; ")
                } else {
                    diffs.as_str()
                };
                r.samples.push(describe(&t, m, &problem, diffs));
            }
        }
        m.clean();
    }
    r.mnemonic = reader.header.mnemonic.clone();
    if r.tests != reader.header.test_count && r.error.is_none() {
        r.error = Some(format!(
            "header says {} tests, file has {}",
            reader.header.test_count, r.tests
        ));
    }
    r
}

fn pct(n: u64, d: u64) -> String {
    if d == 0 {
        "-".to_string()
    } else {
        format!("{:.1}%", n as f64 * 100.0 / d as f64)
    }
}

#[derive(Default)]
struct Totals {
    files: u64,
    tests: u64,
    passed: u64,
    failed: u64,
    panicked: u64,
    skipped: [u64; SKIP_KINDS],
    files_all_pass: u64,
}

impl Totals {
    fn add(&mut self, r: &FileResult) {
        self.files += 1;
        self.tests += r.tests as u64;
        self.passed += r.passed as u64;
        self.failed += r.failed as u64;
        self.panicked += r.panicked as u64;
        for (t, s) in self.skipped.iter_mut().zip(r.skipped) {
            *t += s as u64;
        }
        if r.ran() > 0 && r.passed == r.ran() && r.error.is_none() {
            self.files_all_pass += 1;
        }
    }

    fn ran(&self) -> u64 {
        self.passed + self.failed + self.panicked
    }
}

/// The summary: one row per file (and its failure samples), totals per
/// opcode family, and overall totals.
fn render(results: &[(OpcodeName, FileResult)], samples_per_file: usize, header: &str) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "{header}\n");
    let _ = writeln!(
        s,
        "{:<12} {:<8} {:>6} {:>6} {:>6} {:>6} {:>6} {:>7}  skipped",
        "file", "mnemonic", "tests", "pass", "fail", "panic", "skip", "pass%"
    );
    let mut totals = Totals::default();
    let mut families: Vec<(String, Totals)> = Vec::new();
    for (name, r) in results {
        totals.add(r);
        let family = name.family();
        match families.iter_mut().find(|(f, _)| *f == family) {
            Some((_, t)) => t.add(r),
            None => {
                let mut t = Totals::default();
                t.add(r);
                families.push((family, t));
            }
        }
        let _ = writeln!(
            s,
            "{:<12} {:<8} {:>6} {:>6} {:>6} {:>6} {:>6} {:>7}  {}",
            r.stem,
            r.mnemonic,
            r.tests,
            r.passed,
            r.failed,
            r.panicked,
            r.skipped_total(),
            pct(r.passed as u64, r.ran() as u64),
            r.skip_summary()
        );
        for sample in r.samples.iter().take(samples_per_file) {
            let _ = writeln!(s, "    {sample}");
        }
    }

    families.sort_by(|a, b| a.0.cmp(&b.0));
    let skips: Vec<String> = SKIP_NAMES
        .iter()
        .zip(totals.skipped)
        .filter(|&(_, n)| n > 0)
        .map(|(name, n)| format!("{n} {name}"))
        .collect();
    families.push(("TOTAL".to_string(), totals));
    let _ = writeln!(
        s,
        "\n{:<29} {:>5} {:>8} {:>8} {:>8} {:>7} {:>8} {:>7}",
        "family", "files", "tests", "pass", "fail", "panic", "skip", "pass%"
    );
    for (family, t) in &families {
        let _ = writeln!(
            s,
            "{:<29} {:>5} {:>8} {:>8} {:>8} {:>7} {:>8} {:>7}   ({} files all pass)",
            family,
            t.files,
            t.tests,
            t.passed,
            t.failed,
            t.panicked,
            t.skipped.iter().sum::<u64>(),
            pct(t.passed, t.ran()),
            t.files_all_pass
        );
    }
    if !skips.is_empty() {
        let _ = writeln!(s, "\nskipped tests: {}", skips.join(", "));
    }
    s
}

#[test]
#[ignore = "needs the SingleStepTests/80386 suite; see docs/cpu-tests.md"]
fn sst386_real_mode() {
    let Some(dir) = std::env::var("SST386_DIR").ok().filter(|d| !d.is_empty()) else {
        eprintln!(
            "SST386_DIR is not set; skipping. Fetch the suite with tests/sst386/fetch.sh and \
             set SST386_DIR=target/sst386/v1_ex_real_mode."
        );
        return;
    };
    let dir = PathBuf::from(dir);
    let cfg = Config {
        filters: env_list("SST386_FILTER"),
        index: std::env::var("SST386_INDEX").ok().and_then(|v| v.parse().ok()),
        strict: std::env::var("SST386_STRICT").is_ok_and(|v| v == "1"),
        samples: std::env::var("SST386_SAMPLES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3),
        threads: std::env::var("SST386_THREADS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()))
            .max(1),
        revoked: load_revocations(&dir),
        flag_masks: load_flag_masks(&dir),
        dir,
    };

    let mut files: Vec<(OpcodeName, PathBuf)> = std::fs::read_dir(&cfg.dir)
        .unwrap_or_else(|e| panic!("cannot read SST386_DIR {}: {e}", cfg.dir.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter_map(|p| {
            let file_name = p.file_name()?.to_str()?.to_string();
            let upper = file_name.to_uppercase();
            if !(upper.ends_with(".MOO") || upper.ends_with(".MOO.GZ")) {
                return None;
            }
            if !cfg.filters.is_empty()
                && !cfg
                    .filters
                    .iter()
                    .any(|f| upper.contains(&f.to_uppercase()))
            {
                return None;
            }
            Some((OpcodeName::parse(&file_name), p))
        })
        .collect();
    files.sort_by_key(|(name, _)| name.sort_key());
    assert!(
        !files.is_empty(),
        "no MOO files in {} match SST386_FILTER {:?}",
        cfg.dir.display(),
        cfg.filters
    );

    // Emulator panics are expected while the CPU is incomplete: record
    // where they happened instead of printing a backtrace for each.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|info| {
        let at = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()));
        machine::PANIC_LOCATION.with(|l| *l.borrow_mut() = at);
    }));

    let started = Instant::now();
    let threads = cfg.threads.min(files.len());
    eprintln!(
        "[sst386] {} files from {}, {} threads",
        files.len(),
        cfg.dir.display(),
        threads
    );
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    // Blocks the dynamic recompiler ran (RUST_DOS_CORE=dynamic).
    let translated = std::sync::atomic::AtomicU64::new(0);
    let slots: Vec<Mutex<Option<FileResult>>> = files.iter().map(|_| Mutex::new(None)).collect();
    let worker_panic = std::thread::scope(|s| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some((name, path)) = files.get(i) else {
                            break;
                        };
                        // A fresh machine per file: device state (the PIT a
                        // test can read with IN) must not depend on which
                        // files a worker ran before.
                        let mut machine = Machine::new();
                        let r = run_file(&mut machine, path, name, &cfg);
                        translated.fetch_add(machine.cpu.dynrec.stats().runs, Ordering::Relaxed);
                        *slots[i].lock().unwrap() = Some(r);
                        let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                        if n % 50 == 0 || n == files.len() {
                            eprintln!(
                                "[sst386] {n}/{} files, {:.1}s",
                                files.len(),
                                started.elapsed().as_secs_f64()
                            );
                        }
                    }
                })
            })
            .collect();
        workers
            .into_iter()
            .filter_map(|w| w.join().err())
            .map(machine::panic_message)
            .next()
    });
    std::panic::set_hook(default_hook);
    if let Some(msg) = worker_panic {
        panic!("a harness worker panicked outside a test: {msg}");
    }

    let results: Vec<(OpcodeName, FileResult)> = files
        .into_iter()
        .zip(slots)
        .map(|((name, _), slot)| (name, slot.into_inner().unwrap().unwrap()))
        .collect();
    let translated = translated.into_inner();
    let header = format!(
        "SingleStepTests/80386 real mode: {} files from {} in {:.1}s ({} threads{})",
        results.len(),
        cfg.dir.display(),
        started.elapsed().as_secs_f64(),
        threads,
        if translated > 0 { format!(", dynamic core: {} translated blocks run", translated) } else { String::new() }
    );
    let stdout_report = render(&results, 1, &header);
    let full_report = render(&results, cfg.samples, &header);
    println!("{stdout_report}");

    let report_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/sst386-report.txt");
    match std::fs::create_dir_all(report_path.parent().unwrap())
        .and_then(|_| std::fs::write(&report_path, &full_report))
    {
        Ok(()) => println!("Full report: {}", report_path.display()),
        Err(e) => eprintln!("cannot write {}: {e}", report_path.display()),
    }

    let failed: u64 = results
        .iter()
        .map(|(_, r)| (r.failed + r.panicked) as u64)
        .sum();
    let errors: Vec<&str> = results
        .iter()
        .filter(|(_, r)| r.error.is_some())
        .map(|(_, r)| r.stem.as_str())
        .collect();
    if cfg.strict {
        assert!(
            failed == 0 && errors.is_empty(),
            "{failed} tests failed or panicked; unreadable files: {errors:?}"
        );
    }
}
