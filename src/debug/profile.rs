//! `/api/profile`: the profiler (`rust_dos::profile`) through the debug
//! server. The hub starts and stops a profile and builds its report, with
//! the disassembly of the hottest guest code.

use serde_json::{Map, Value, json};

use rust_dos::profile::report::{self, Query, pct};
use rust_dos::profile::sampler::Timer;
use rust_dos::profile::session::{Options, Session, Totals};
use rust_dos::profile::{Engine, Loc, Mode, Zone, counters::MemKind};

use super::{DebugHub, Reply};
use crate::cpu::Cpu;

/// `POST /api/profile`.
#[derive(Clone, Debug, Default)]
pub struct Control {
    /// Start (true) or stop (false) a profile; starting one while one runs
    /// starts it over.
    pub enabled: Option<bool>,
    pub hz: Option<u32>,
    pub timer: Option<Timer>,
    /// Stop after this many milliseconds.
    pub duration_ms: Option<u64>,
}

/// How `GET /api/profile` answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Json,
    /// Folded stacks for flame graphs.
    Folded,
    /// A short table.
    Text,
}

impl DebugHub {
    pub(super) fn profile_control(&mut self, cpu: &mut Cpu, c: Control) -> Reply {
        match c.enabled {
            Some(true) => {
                if let Some(old) = &mut self.profile {
                    old.stop(cpu);
                }
                let options = Options {
                    hz: c.hz.unwrap_or(rust_dos::profile::session::DEFAULT_HZ),
                    timer: c.timer.unwrap_or(Timer::Sleep),
                    duration: c.duration_ms.map(std::time::Duration::from_millis),
                };
                match Session::start(cpu, options) {
                    Ok(session) => {
                        self.profile = Some(session);
                        cpu.bus.log_string(&format!(
                            "[PROFILE] Started at {} Hz ({}){}",
                            options.hz.clamp(rust_dos::profile::session::MIN_HZ, rust_dos::profile::session::MAX_HZ),
                            options.timer.name(),
                            c.duration_ms.map(|ms| format!(" for {} ms", ms)).unwrap_or_default()
                        ));
                    }
                    Err(e) => return Reply::Error(500, format!("the profiler's thread can't start: {}", e)),
                }
            }
            Some(false) => match &mut self.profile {
                Some(session) => {
                    session.stop(cpu);
                    cpu.bus.log_string("[PROFILE] Stopped");
                }
                None => return Reply::Error(404, NO_PROFILE.into()),
            },
            None if c.hz.is_some() || c.timer.is_some() || c.duration_ms.is_some() => {
                return Reply::bad("hz, timer and duration_ms go with \"enabled\":true");
            }
            None => {}
        }
        Reply::Json(self.profile_status(cpu))
    }

    /// `--profile`: start a profile with the machine.
    pub fn profile_at_start(&mut self, cpu: &mut Cpu, hz: u32) -> std::io::Result<()> {
        let options = Options { hz, ..Options::default() };
        self.profile = Some(Session::start(cpu, options)?);
        cpu.bus.log_string(&format!("[PROFILE] Started at {} Hz until the emulator exits", options.hz));
        Ok(())
    }

    /// `--profile FILE`, as the emulator exits: stop the profile and write
    /// its report, as JSON to `path`, and as a table and as folded stacks
    /// beside it (`.txt`, `.folded`). Returns the files written.
    pub fn profile_write(&mut self, cpu: &mut Cpu, path: &std::path::Path) -> std::io::Result<Vec<std::path::PathBuf>> {
        let Some(session) = &mut self.profile else { return Ok(Vec::new()) };
        session.stop(cpu);
        let session = self.profile.as_ref().unwrap();
        let query = Query { top: 50, ..Query::default() };
        let report = self.profile_json(cpu, session, &query);
        let folded = report::folded(&session.samples().counts, &query);
        let files = [
            (path.to_path_buf(), serde_json::to_string_pretty(&report).unwrap_or_default()),
            (path.with_extension("txt"), text(&report)),
            (path.with_extension("folded"), folded),
        ];
        for (file, contents) in &files {
            std::fs::write(file, contents)?;
        }
        Ok(files.into_iter().map(|(file, _)| file).collect())
    }

    /// Stop a profile at the end of its duration. Once a frame.
    pub(super) fn profile_poll(&mut self, cpu: &mut Cpu) {
        if let Some(session) = &mut self.profile
            && session.running()
        {
            session.poll(cpu);
            if !session.running() {
                cpu.bus.log_string("[PROFILE] Stopped at the end of its duration");
            }
        }
    }

    /// Whether a profile runs, since when and how it samples.
    pub(super) fn profile_status(&self, cpu: &Cpu) -> Value {
        match &self.profile {
            None => json!({"profile": null, "running": false}),
            Some(s) => {
                let samples = s.samples();
                let (a, b) = s.window(cpu);
                json!({
                    "running": s.running(),
                    "hz": s.options.hz,
                    "timer": s.options.timer.name(),
                    "duration_ms": s.options.duration.map(|d| d.as_millis() as u64),
                    "wall_ms": ms(&a, &b),
                    "samples": samples.total,
                })
            }
        }
    }

    pub(super) fn profile_report(&self, cpu: &Cpu, query: &Query, format: Format) -> Reply {
        let Some(session) = &self.profile else { return Reply::Error(404, NO_PROFILE.into()) };
        if let Err(e) = query.check() {
            return Reply::bad(e);
        }
        let samples = session.samples();
        match format {
            Format::Folded => Reply::Json(json!({"text": report::folded(&samples.counts, query)})),
            Format::Json => Reply::Json(self.profile_json(cpu, session, query)),
            Format::Text => Reply::Json(json!({"text": text(&self.profile_json(cpu, session, query))})),
        }
    }

    fn profile_json(&self, cpu: &Cpu, session: &Session, query: &Query) -> Value {
        let samples = session.samples();
        let total = samples.total;
        let (a, b) = session.window(cpu);
        let wall_s = b.at.duration_since(a.at).as_secs_f64();
        let per_s = |n: u64| if wall_s > 0.0 { (n as f64 / wall_s * 10.0).round() / 10.0 } else { 0.0 };
        let zones = report::by_zone(&samples.counts);
        let busy = report::busy(&zones);
        let waits = total - busy;

        let mut engines = [0u64; 3];
        for (&word, &n) in &samples.counts {
            engines[Loc(word).engine() as usize] += n;
        }
        let taken: u64 = samples.counts.iter().filter(|&(&w, _)| query.takes(Loc(w).zone())).map(|(_, &n)| n).sum();
        let taken_busy: u64 = samples
            .counts
            .iter()
            .filter(|&(&w, _)| query.takes(Loc(w).zone()) && !Loc(w).zone().is_wait())
            .map(|(_, &n)| n)
            .sum();
        let places: Vec<Value> = report::places(&samples.counts, query)
            .into_iter()
            .take(query.top)
            .map(|p| {
                let mut v = json!({
                    "addr": p.loc.addr_text(),
                    "mode": p.loc.mode().name(),
                    "engine": p.loc.engine().name(),
                    "samples": p.samples,
                    "pct": pct(p.samples, total),
                    "busy_pct": pct(p.samples, busy),
                    "zones": p.zones.iter().map(|&(z, n)| json!({"zone": z.name(), "samples": n, "pct": pct(n, total), "busy_pct": pct(n, busy)})).collect::<Vec<_>>(),
                });
                if let Some(lin) = linear(p.loc) {
                    v["lin"] = json!(format!("{:08X}", lin));
                }
                if query.bucket > 1 {
                    v["hottest"] = json!(p.hottest.addr_text());
                }
                if query.disasm > 0 {
                    v["disasm"] = self.place_disasm(cpu, p.hottest, query.disasm);
                }
                v
            })
            .collect();
        let segments: Vec<Value> = report::segments(&samples.counts, query)
            .into_iter()
            .take(query.top.min(10))
            .map(|((cs, mode), n)| json!({"cs": format!("{:04X}", cs), "mode": mode.name(), "samples": n, "pct": pct(n, total), "busy_pct": pct(n, busy)}))
            .collect();

        let mut out = json!({
            "running": session.running(),
            "build": rust_dos::build_info::json(),
            "sampler": {
                "hz": session.options.hz,
                "timer": session.options.timer.name(),
                "duration_ms": session.options.duration.map(|d| d.as_millis() as u64),
                "samples": total,
                "achieved_hz": (samples.rate() * 10.0).round() / 10.0,
                "interval_us": {
                    "mean": if samples.waits > 0 { samples.wait_sum_us / samples.waits } else { 0 },
                    "min": samples.wait_min_us,
                    "p50": samples.wait_percentile_us(50.0),
                    "p90": samples.wait_percentile_us(90.0),
                    "p99": samples.wait_percentile_us(99.0),
                    "max": samples.wait_max_us,
                },
            },
            "window": window_json(&a, &b),
            "busy": {"samples": busy, "pct": pct(busy, total)},
            "engines": {
                "frontend": pct(engines[Engine::None as usize], total),
                "interp": pct(engines[Engine::Interp as usize], total),
                "jit": pct(engines[Engine::Jit as usize], total),
                "busy": {
                    "frontend": pct(engines[Engine::None as usize] - waits, busy),
                    "interp": pct(engines[Engine::Interp as usize], busy),
                    "jit": pct(engines[Engine::Jit as usize], busy),
                },
            },
            "groups": report::groups_json(&zones, total),
            "zones": report::zones_json(&zones, total),
            "query": {
                "zone": query.zone,
                "bucket": query.bucket,
                "samples": taken,
                "pct": pct(taken, total),
                "busy_pct": pct(taken_busy, busy),
            },
            "top": places,
            "segments": segments,
        });
        if let Some(c) = session.counters(cpu) {
            out["ports"] = ports_json(c, query.top, &per_s);
            out["hle"] = hle_json(c, query.top, &per_s);
            out["irqs"] = Value::Array(
                c.irqs.iter().enumerate().filter(|&(_, &n)| n > 0).map(|(irq, &n)| json!({"irq": irq, "count": n, "per_s": per_s(n)})).collect(),
            );
            out["exceptions"] = Value::Array(
                c.exceptions
                    .iter()
                    .enumerate()
                    .filter(|&(_, &n)| n > 0)
                    .map(|(v, &n)| json!({"vector": format!("{:02X}", v), "count": n, "per_s": per_s(n)}))
                    .collect(),
            );
            let mut mem = Map::new();
            for kind in MemKind::ALL {
                let (r, w) = (c.mem_reads[kind as usize].get(), c.mem_writes[kind as usize].get());
                mem.insert(kind.name().into(), json!({"reads": r, "writes": w, "per_s": per_s(r + w)}));
            }
            out["mapped_memory"] = Value::Object(mem);
            out["dynrec"] = dynrec_json(&a, &b, c, query.top);
        }
        if query.timeline {
            out["timeline"] = Value::Array(
                samples
                    .timeline
                    .iter()
                    .enumerate()
                    .map(|(s, zones)| {
                        let n: u64 = zones.iter().map(|&z| z as u64).sum();
                        let mut by: Vec<(Zone, u32)> =
                            zones.iter().enumerate().filter(|&(_, &z)| z > 0).map(|(z, &k)| (Zone::ALL[z], k)).collect();
                        by.sort_by_key(|&(z, k)| (std::cmp::Reverse(k), z));
                        let shares: Vec<Value> = by.iter().map(|&(z, k)| json!({"zone": z.name(), "pct": pct(k as u64, n)})).collect();
                        json!({"t_s": s, "samples": n, "zones": shares})
                    })
                    .collect(),
            );
        }
        out
    }

    /// A few lines of disassembly from a place, if its address means the
    /// same in the CPU's mode now.
    fn place_disasm(&self, cpu: &Cpu, loc: Loc, lines: usize) -> Value {
        let Some((cs, eip)) = loc.cs_eip() else { return Value::Null };
        let addr = match (loc.mode().segmented(), cpu.pm()) {
            (true, false) => format!("{:04X}:{:04X}", cs, eip),
            (true, true) => format!("lin:{:X}", ((cs as u32) << 4).wrapping_add(eip)),
            (false, true) => format!("{:04X}:{:08X}", cs, eip),
            (false, false) => return Value::Null,
        };
        match self.disasm(cpu, Some(addr), lines) {
            Reply::Json(v) => v["rows"]
                .as_array()
                .map(|rows| {
                    Value::Array(
                        rows.iter()
                            .map(|r| json!(format!("{} {}", r["bytes"].as_str().unwrap_or(""), r["asm"].as_str().unwrap_or("")).trim().to_string()))
                            .collect(),
                    )
                })
                .unwrap_or(Value::Null),
            _ => Value::Null,
        }
    }
}

const NO_PROFILE: &str = "no profile; POST /api/profile {\"enabled\":true} starts one";

fn ms(a: &Totals, b: &Totals) -> u64 {
    b.at.duration_since(a.at).as_millis() as u64
}

/// The linear address of a place in real or virtual-8086 mode.
fn linear(loc: Loc) -> Option<u32> {
    let (cs, eip) = loc.cs_eip()?;
    matches!(loc.mode(), Mode::Real | Mode::V86).then(|| ((cs as u32) << 4).wrapping_add(eip))
}

/// What the machine did in the window.
fn window_json(a: &Totals, b: &Totals) -> Value {
    let wall_s = b.at.duration_since(a.at).as_secs_f64();
    let emulated_ns = b.emulated_ns.saturating_sub(a.emulated_ns);
    let executed = b.executed.saturating_sub(a.executed);
    let jit = b.dynrec.executed.saturating_sub(a.dynrec.executed);
    let ticks = b.icount.saturating_sub(a.icount);
    let idle = b.idle.saturating_sub(a.idle);
    let frames = b.frames_drawn.saturating_sub(a.frames_drawn);
    let hits = b.decode_hits.saturating_sub(a.decode_hits);
    let misses = b.decode_misses.saturating_sub(a.decode_misses);
    let rate = |n: u64| if wall_s > 0.0 { (n as f64 / wall_s * 10.0).round() / 10.0 } else { 0.0 };
    let names = ["bursts", "flips", "polls", "status_reads", "video_bytes", "io_bytes"];
    let activity: Map<String, Value> =
        names.iter().zip(a.activity.iter().zip(&b.activity)).map(|(n, (x, y))| (n.to_string(), json!(y.saturating_sub(*x)))).collect();
    json!({
        "wall_ms": (wall_s * 1000.0).round() as u64,
        "emulated_ms": emulated_ns / 1_000_000,
        "emulated_speed": if wall_s > 0.0 { (emulated_ns as f64 / 1e9 / wall_s * 1000.0).round() / 1000.0 } else { 0.0 },
        "instructions": executed,
        "mips": if wall_s > 0.0 { (executed as f64 / wall_s / 1e4).round() / 100.0 } else { 0.0 },
        "recompiled_pct": pct(jit.min(executed), executed),
        "halted_pct": pct(idle, ticks),
        "frames_drawn": frames,
        "fps": rate(frames),
        "decode_cache_hit_pct": pct(hits, hits + misses),
        "activity": activity,
    })
}

fn ports_json(c: &rust_dos::profile::Counters, top: usize, per_s: &dyn Fn(u64) -> f64) -> Value {
    let mut devices: Vec<(Zone, u64, u64)> = c
        .ports_by_zone()
        .iter()
        .enumerate()
        .filter(|&(_, &(r, w))| r + w > 0)
        .map(|(z, &(r, w))| (Zone::ALL[z], r, w))
        .collect();
    devices.sort_by_key(|&(z, r, w)| (std::cmp::Reverse(r + w), z));
    let mut ports: Vec<(usize, u64, u64)> =
        (0..0x10000).filter(|&p| c.port_reads[p] | c.port_writes[p] != 0).map(|p| (p, c.port_reads[p], c.port_writes[p])).collect();
    ports.sort_by_key(|&(p, r, w)| (std::cmp::Reverse(r + w), p));
    json!({
        "total": ports.iter().map(|&(_, r, w)| r + w).sum::<u64>(),
        "by_device": devices
            .iter()
            .map(|&(z, r, w)| json!({"device": z.name(), "reads": r, "writes": w, "per_s": per_s(r + w)}))
            .collect::<Vec<_>>(),
        "top": ports
            .iter()
            .take(top)
            .map(|&(p, r, w)| json!({
                "port": format!("{:04X}", p),
                "device": c.port_zone(p as u16).name(),
                "reads": r,
                "writes": w,
                "per_s": per_s(r + w),
            }))
            .collect::<Vec<_>>(),
    })
}

fn hle_json(c: &rust_dos::profile::Counters, top: usize, per_s: &dyn Fn(u64) -> f64) -> Value {
    let mut calls: Vec<(usize, u64)> = c.hle.iter().copied().enumerate().filter(|&(_, n)| n > 0).collect();
    calls.sort_by_key(|&(k, n)| (std::cmp::Reverse(n), k));
    let mut by_int = [0u64; 256];
    for &(k, n) in &calls {
        by_int[k >> 8] += n;
    }
    let mut ints: Vec<(usize, u64)> = by_int.iter().copied().enumerate().filter(|&(_, n)| n > 0).collect();
    ints.sort_by_key(|&(v, n)| (std::cmp::Reverse(n), v));
    const KINDS: [&str; 3] = ["inline", "far", "dpmi"];
    let mut services: Vec<(&str, usize, u64)> = KINDS
        .iter()
        .enumerate()
        .flat_map(|(k, &kind)| c.services[k].iter().enumerate().filter(|&(_, &n)| n > 0).map(move |(v, &n)| (kind, v, n)))
        .collect();
    services.sort_by_key(|&(kind, v, n)| (std::cmp::Reverse(n), kind, v));
    let services: Vec<Value> = services
        .iter()
        .take(top)
        .map(|&(kind, v, n)| json!({"kind": kind, "service": format!("{:02X}", v), "calls": n, "per_s": per_s(n)}))
        .collect();
    json!({
        "total": calls.iter().map(|&(_, n)| n).sum::<u64>(),
        "by_int": ints.iter().map(|&(v, n)| json!({"int": format!("{:02X}", v), "calls": n, "per_s": per_s(n)})).collect::<Vec<_>>(),
        "top": calls
            .iter()
            .take(top)
            .map(|&(k, n)| json!({
                "int": format!("{:02X}", k >> 8),
                "function": format!("{:02X}", k & 0xFF),
                "register": rust_dos::profile::counters::function_register((k >> 8) as u8),
                "name": report::service_name((k >> 8) as u8, k as u8),
                "calls": n,
                "per_s": per_s(n),
            }))
            .collect::<Vec<_>>(),
        "services": services,
    })
}

/// The recompiler's counts in the window, and its pages.
fn dynrec_json(a: &Totals, b: &Totals, c: &rust_dos::profile::Counters, top: usize) -> Value {
    const EXITS: [&str; 16] = [
        "", "fault", "smc", "panic", "next", "deadline", "stale", "gp0", "limit", "unlinked", "de", "watched", "after",
        "next_page", "env", "far_env",
    ];
    let (x, y) = (&a.dynrec, &b.dynrec);
    let d = |p: u64, q: u64| q.saturating_sub(p);
    let exits: Map<String, Value> = (1..16)
        .filter(|&k| d(x.exits[k], y.exits[k]) > 0)
        .map(|k| (EXITS[k].to_string(), json!(d(x.exits[k], y.exits[k]))))
        .collect();
    let calls = match (&a.calls, &b.calls) {
        (Some(p), Some(q)) => {
            let names: Vec<_> = iced_x86::Mnemonic::values().collect();
            let mut fallback: Vec<(usize, u64)> = q
                .fallback
                .iter()
                .enumerate()
                .map(|(m, &n)| (m, n.saturating_sub(p.fallback.get(m).copied().unwrap_or(0))))
                .filter(|&(_, n)| n > 0)
                .collect();
            fallback.sort_by_key(|&(m, n)| (std::cmp::Reverse(n), m));
            json!({
                "fallback": fallback.iter().map(|&(_, n)| n).sum::<u64>(),
                "fallback_top": fallback.iter().take(top).map(|&(m, n)| json!([format!("{:?}", names[m]), n])).collect::<Vec<_>>(),
                "revalidate": d(p.revalidate, q.revalidate),
                "memref": d(p.memref, q.memref),
                "slow": d(p.slow, q.slow),
                "dev": d(p.dev, q.dev),
                "port": d(p.port, q.port),
                "load_seg": d(p.load_seg, q.load_seg),
                "fetch": d(p.fetch, q.fetch),
                "fpu": d(p.fpu, q.fpu),
            })
        }
        // (Started before the recompiler made its first block.)
        (None, Some(q)) => json!({"fallback": q.fallback.iter().sum::<u64>(), "slow": q.slow, "dev": q.dev, "port": q.port}),
        _ => Value::Null,
    };
    let mut pages: Vec<(usize, rust_dos::profile::counters::CodePage)> =
        c.code_pages.iter().copied().enumerate().filter(|(_, p)| p.translated + p.stale + p.smc > 0).collect();
    pages.sort_by_key(|&(i, p)| (std::cmp::Reverse((p.stale + p.smc, p.translated)), i));
    json!({
        "blocks_translated": d(x.blocks, y.blocks),
        "instructions_translated": d(x.instructions, y.instructions),
        "native": d(x.native, y.native),
        "live_blocks": y.live_blocks,
        "code_bytes": y.code_bytes,
        "flushes": d(x.flushes, y.flushes),
        "runs": d(x.runs, y.runs),
        "executed": d(x.executed, y.executed),
        "deadline": d(x.deadline, y.deadline),
        "stale": d(x.stale, y.stale),
        "smc": d(x.smc, y.smc),
        "watched": d(x.watched, y.watched),
        "return_misses": d(x.return_misses, y.return_misses),
        "exits": exits,
        "calls": calls,
        "pages": pages
            .iter()
            .take(top)
            .map(|&(i, p)| json!({"page": format!("{:05X}", i << 12), "translated": p.translated, "stale": p.stale, "smc": p.smc}))
            .collect::<Vec<_>>(),
    })
}

/// The report as a short table.
fn text(r: &Value) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let s = &r["sampler"];
    let w = &r["window"];
    let _ = writeln!(
        out,
        "Profile {}: {:.1} s, {} samples at {} Hz (asked {}, {}; waits p50 {} us, p99 {} us)",
        if r["running"].as_bool() == Some(true) { "running" } else { "stopped" },
        w["wall_ms"].as_u64().unwrap_or(0) as f64 / 1000.0,
        s["samples"],
        s["achieved_hz"],
        s["hz"],
        s["timer"].as_str().unwrap_or(""),
        s["interval_us"]["p50"],
        s["interval_us"]["p99"],
    );
    let _ = writeln!(
        out,
        "Machine: {} ms emulated ({}x), {} instructions ({} MIPS), {}% recompiled, {}% halted, {} fps",
        w["emulated_ms"], w["emulated_speed"], w["instructions"], w["mips"], w["recompiled_pct"], w["halted_pct"], w["fps"],
    );
    let f = |v: &Value| v.as_f64().unwrap_or(0.0);
    let _ = writeln!(
        out,
        "Busy: {:.2}% of the samples, the rest waiting for the next frame; \"busy\" shares are of the busy samples",
        f(&r["busy"]["pct"]),
    );
    let e = &r["engines"]["busy"];
    let _ = writeln!(
        out,
        "Engines (busy): interpreter {:.2}%, translated code {:.2}%, front end {:.2}%",
        f(&e["interp"]),
        f(&e["jit"]),
        f(&e["frontend"]),
    );
    let _ = writeln!(out, "\nZones:       busy      all");
    for z in r["zones"].as_array().into_iter().flatten().take(20) {
        let busy = if z["busy_pct"].is_null() { "-".to_string() } else { format!("{:.2}%", f(&z["busy_pct"])) };
        let _ = writeln!(out, "  {:>12} {:>8.2}%  {}", busy, f(&z["pct"]), z["zone"].as_str().unwrap_or(""));
    }
    let q = &r["query"];
    let filter = q["zone"].as_str().map(|z| format!(" in {}", z)).unwrap_or_default();
    let share = if q["zone"].is_null() { f(&e["interp"]) + f(&e["jit"]) } else { f(&q["busy_pct"]) };
    let _ = writeln!(out, "\nGuest code{} ({:.2}% of busy):", filter, share);
    for p in r["top"].as_array().into_iter().flatten() {
        let zones: Vec<String> = p["zones"]
            .as_array()
            .into_iter()
            .flatten()
            .skip(1)
            .take(3)
            .map(|z| format!("{} {:.2}%", z["zone"].as_str().unwrap_or(""), f(&z["busy_pct"])))
            .collect();
        let first = p["disasm"].as_array().and_then(|d| d.first()).and_then(|l| l.as_str()).unwrap_or("");
        let _ = writeln!(
            out,
            "  {:>11.2}% {:>8.2}%  {} {:<14} {:<10} {}{}",
            f(&p["busy_pct"]),
            f(&p["pct"]),
            p["mode"].as_str().unwrap_or(""),
            p["addr"].as_str().unwrap_or(""),
            p["engine"].as_str().unwrap_or(""),
            first,
            if zones.is_empty() { String::new() } else { format!("  [{}]", zones.join(", ")) },
        );
    }
    if let Some(ports) = r["ports"]["top"].as_array().filter(|p| !p.is_empty()) {
        let _ = writeln!(out, "\nPorts ({} accesses):", r["ports"]["total"]);
        for p in ports.iter().take(10) {
            let _ = writeln!(
                out,
                "  {} {:<14} {:>10} reads {:>10} writes {:>10}/s",
                p["port"].as_str().unwrap_or(""),
                p["device"].as_str().unwrap_or(""),
                p["reads"],
                p["writes"],
                p["per_s"],
            );
        }
    }
    // The interrupt handlers' functions and the emulator's other services
    // (the DPMI host, inline and far-call traps) in one list, most first.
    let mut lines: Vec<(u64, String)> = Vec::new();
    for c in r["hle"]["top"].as_array().into_iter().flatten() {
        let what = format!(
            "INT {} {}={} {}",
            c["int"].as_str().unwrap_or(""),
            c["register"].as_str().unwrap_or(""),
            c["function"].as_str().unwrap_or(""),
            c["name"].as_str().unwrap_or(""),
        );
        lines.push((c["calls"].as_u64().unwrap_or(0), format!("  {:<42} {:>10} {:>10}/s", what, c["calls"], c["per_s"])));
    }
    for c in r["hle"]["services"].as_array().into_iter().flatten() {
        let what = format!("{} service {}", c["kind"].as_str().unwrap_or(""), c["service"].as_str().unwrap_or(""));
        lines.push((c["calls"].as_u64().unwrap_or(0), format!("  {:<42} {:>10} {:>10}/s", what, c["calls"], c["per_s"])));
    }
    if !lines.is_empty() {
        lines.sort_by_key(|(n, _)| std::cmp::Reverse(*n));
        let _ = writeln!(out, "\nServices:");
        for (_, line) in lines.iter().take(12) {
            let _ = writeln!(out, "{}", line);
        }
    }
    let d = &r["dynrec"];
    if d.is_object() {
        let _ = write!(
            out,
            "\nRecompiler: {} blocks translated, {} stale, {} smc, {} flushes",
            d["blocks_translated"], d["stale"], d["smc"], d["flushes"],
        );
        match d["calls"]["fallback"].as_u64() {
            Some(n) => {
                let _ = writeln!(out, ", {} fallbacks", n);
            }
            None => out.push('\n'),
        }
        let top: Vec<String> = d["calls"]["fallback_top"]
            .as_array()
            .into_iter()
            .flatten()
            .take(8)
            .map(|f| format!("{} {}", f[0].as_str().unwrap_or(""), f[1]))
            .collect();
        if !top.is_empty() {
            let _ = writeln!(out, "  fallbacks: {}", top.join(", "));
        }
    }
    out
}
