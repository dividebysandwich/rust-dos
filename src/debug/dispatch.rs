//! Request handling: dispatching commands from the server.

use std::path::PathBuf;

use serde_json::{Map, Value, json};

use super::input::expand_input;
use super::journal::{Kind, OpKind, OpState};
use super::status::{at_idle_prompt, bytes_record, drives_json, ivt_json, net_json, printer_json, screen_text, serial_json, set_regs, sound_json, stop_reply};
use super::{Cmd, Ctx, DebugHub, LowInput, Need, PauseReason, Reply, Request, RunWait, SpeedRequest, StateRequest, Watch, journal, parse_addr, pm, regs_json, trace};
use crate::cpu::Cpu;
use crate::disk::{DriveKind, MountOptions};
use crate::mount::{parse_drive_name, parse_kind};
use crate::video;

impl DebugHub {
    // ----- request handling --------------------------------------------------

    pub(super) fn handle(&mut self, cpu: &mut Cpu, req: Request) {
        // A stop comes before anything this request does.
        self.flush_stop(cpu);
        let Request { cmd, reply: tx, req, tag, if_state } = req;
        let ctx = Ctx { req, tag, seq: None, op: None };
        if self.quit.is_some() {
            self.answer(&ctx, tx, Reply::Error(503, "the emulator is shutting down".into()));
            return;
        }
        // In deterministic mode, a change to the machine that came while it
        // runs would land at whatever emulated time the host got to. The
        // settings window, which holds the machine, takes its input at once.
        // Save states and speeds are checked again as they are carried out.
        let need = changes_machine(&cmd).then_some(Need::Paused);
        let refusal = need.map_or(Ok(()), |n| self.check(n)).and_then(|_| if_state.map_or(Ok(()), |n| self.check(Need::State(n))));
        if let Err(body) = refusal {
            self.answer(&ctx, tx, Reply::ErrorJson(409, body));
            return;
        }
        let cmd = match cmd {
            Cmd::SaveState { path, .. } | Cmd::LoadState { path, .. } if path.is_empty() => {
                self.answer(&ctx, tx, Reply::bad("a path is needed"));
                return;
            }
            Cmd::SaveState { path, strict_match, check_files } => {
                let path = PathBuf::from(path);
                let folders = Vec::new();
                self.state_requests.push(StateRequest { load: false, path, strict_match, check_files, folders, ctx, if_state, reply: tx });
                return;
            }
            Cmd::LoadState { path, strict_match, check_files, folders } => {
                let path = PathBuf::from(path);
                let folders = folders.into_iter().map(|(drive, folder)| (drive, PathBuf::from(folder))).collect();
                self.state_requests.push(StateRequest { load: true, path, strict_match, check_files, folders, ctx, if_state, reply: tx });
                return;
            }
            Cmd::Speed { cycles } => {
                self.speed_requests.push(SpeedRequest { cycles, ctx, if_state, reply: tx });
                return;
            }
            Cmd::Screenshot => {
                // Answered by the next capture_frame call.
                self.frame_waiters.push((ctx, tx));
                return;
            }
            cmd => cmd,
        };
        self.begin(ctx);
        let reply = self.dispatch(cpu, cmd);
        let ctx = self.finish();
        self.answer(&ctx, tx, reply);
    }

    /// Carry out a command the hub answers at once.
    fn dispatch(&mut self, cpu: &mut Cpu, cmd: Cmd) -> Reply {
        match cmd {
            Cmd::Status => {
                let status = self.status(cpu);
                cpu.bus.audio_peak = 0;
                Reply::Json(status)
            }
            Cmd::Stats => Reply::Json(self.stats_json(cpu)),
            Cmd::ProfileControl(c) => self.profile_control(cpu, c),
            Cmd::ProfileStatus => Reply::Json(self.profile_status(cpu)),
            Cmd::ProfileGet { query, format } => self.profile_report(cpu, &query, format),
            Cmd::Screenshot | Cmd::SaveState { .. } | Cmd::LoadState { .. } | Cmd::Speed { .. } => {
                unreachable!("answered later, by `handle`'s arrangement")
            }
            Cmd::Quit => {
                self.quit = self.cur.clone();
                Reply::Json(json!({"ok": true, "shutting_down": true}))
            }
            Cmd::ScreenText => screen_text(cpu),
            Cmd::TraceQuery(q) => self.trace_query(cpu, q),
            Cmd::TraceControl { enabled, clear, stream_max, count } => {
                if let Some(e) = enabled {
                    self.trace_enabled = e;
                    self.trace_until = None;
                }
                // A count of 0 records nothing more: the trace stops now.
                if let Some(n) = count {
                    self.trace_enabled = n > 0;
                    self.trace_until = (n > 0).then(|| self.trace.total().saturating_add(n));
                }
                if clear {
                    self.trace.clear();
                }
                if let Some(m) = stream_max {
                    self.trace_stream_max = m.max(1);
                }
                // As the next batch will, so the reply's `active` is current.
                let streaming = self.shared.as_ref().is_some_and(|s| s.trace.receiver_count() > 0);
                self.tracing_now = self.trace_enabled || streaming;
                Reply::Json(self.trace_status())
            }
            Cmd::PortsControl { enabled, clear, capacity, ports } => {
                let log = &mut cpu.bus.port_log;
                if let Some(capacity) = capacity {
                    *log = Some(rust_dos::bus::port_log::PortLog::new(capacity.clamp(1, 10_000_000)));
                }
                match enabled {
                    Some(true) if log.is_none() => *log = Some(rust_dos::bus::port_log::PortLog::new(100_000)),
                    Some(false) => *log = None,
                    _ => {}
                }
                if let Some(log) = log {
                    if clear {
                        log.clear();
                    }
                    if let Some(ports) = ports {
                        log.ports = ports;
                    }
                }
                Reply::Json(json!({
                    "enabled": log.is_some(),
                    "entries": log.as_ref().map_or(0, |l| l.len()),
                    "capacity": log.as_ref().map_or(0, |l| l.capacity()),
                    "ports": log.as_ref().map(|l| format!("{:04X}-{:04X}", l.ports.0, l.ports.1)),
                }))
            }
            Cmd::PortsQuery { from, to, write, last_n } => match &cpu.bus.port_log {
                None => Reply::bad("the port log is off: POST /api/ports {\"enabled\":true} first"),
                Some(log) => {
                    let mut picked: Vec<Value> = log
                        .iter()
                        .rev()
                        .filter(|a| (from..=to).contains(&a.port) && write.is_none_or(|w| w == a.write))
                        .take(last_n)
                        .map(|a| {
                            json!({
                                "icount": a.icount,
                                "port": format!("{:04X}", a.port),
                                "dir": if a.write { "out" } else { "in" },
                                "value": format!("{:0width$X}", a.value, width = a.len as usize * 2),
                            })
                        })
                        .collect();
                    picked.reverse();
                    Reply::Json(json!({"count": picked.len(), "accesses": picked}))
                }
            },
            Cmd::Input { events, wait } => {
                // Pending operations are never evicted: this bounds them.
                let pending = self.with_journal(|j| j.pending_ops(|k| k == OpKind::Input).len()).unwrap_or(0);
                if pending >= journal::PENDING_INPUT_CAPACITY {
                    return Reply::Error(
                        429,
                        format!("{} input requests are still being typed: wait for some (or DELETE /api/input)", pending),
                    );
                }
                let mut low = Vec::new();
                for ev in &events {
                    if let Err(e) = expand_input(ev, cpu.bus.typing_layout(), &mut low) {
                        return Reply::bad(e);
                    }
                }
                self.input.extend(low);
                // Every input request is an operation, done once its last
                // event is delivered (`input_delivered`).
                let req = self.cur.as_ref().map_or(0, |c| c.req);
                self.input.push_back(LowInput::End { req });
                let tag = self.cur.as_ref().and_then(|c| c.tag.clone());
                let fields = json!({"events": events.len(), "wait": wait});
                self.append_then(cpu, Kind::InputQueued, Some(req), fields, |j, seq| j.begin_op(req, OpKind::Input, seq, tag));
                if let Some(ctx) = self.cur.as_mut() {
                    ctx.op = Some(req);
                }
                // Paused in deterministic mode, the input is delivered only
                // once the machine resumes: a wait for it is answered at once
                // (`held`), and its operation shows when it was delivered.
                let held = self.deterministic.is_some() && self.paused && !self.divert;
                Reply::Json(json!({"ok": true, "queued": self.input_pending(), "held": held}))
            }
            Cmd::InputClear => {
                let mut cancelled = Vec::new();
                for ev in self.input.drain(..) {
                    if let LowInput::End { req } = ev {
                        cancelled.push(req);
                    }
                }
                self.input_wait_until = None;
                self.input_wait_ns = None;
                let ids = cancelled.clone();
                let cause = self.cur.as_ref().map(|c| c.req);
                self.append_then(cpu, Kind::InputCleared, cause, json!({"cancelled": cancelled}), |j, seq| {
                    for req in ids {
                        j.end_op(req, OpState::Cancelled, Some(seq));
                    }
                });
                Reply::Json(json!({"ok": true, "cancelled": cancelled}))
            }
            Cmd::Pause => {
                if !self.paused {
                    self.enter_pause(PauseReason::Request);
                    self.pause_by = self.cur.as_ref().map(|c| c.req);
                }
                // A stop the batch made comes first; the stop is the reply.
                let stop = self.flush_stop(cpu).or_else(|| self.current_stop());
                Reply::Json(stop_reply(stop, cpu))
            }
            Cmd::Resume { until_ms: Some(_), .. } if self.deterministic.is_none() => {
                Reply::bad("until_ms needs deterministic mode (--deterministic)")
            }
            Cmd::Resume { until_ms: Some(ms), .. } if ms.saturating_mul(1_000_000) <= cpu.bus.clock.now_ns() => {
                Reply::bad(format!("until_ms {} has passed: emulated time is {} ms", ms, cpu.bus.clock.now_ns() / 1_000_000))
            }
            Cmd::Resume { until, until_ms } => {
                let target = match until.as_deref().map(|a| parse_addr(cpu, a).and_then(breakpoint_phys)) {
                    Some(Err(e)) => return Reply::bad(e),
                    Some(Ok(phys)) => Some(phys),
                    None => None,
                };
                self.take_over();
                self.temp_breakpoint = target;
                self.stop_at_ns = until_ms.map(|ms| ms.saturating_mul(1_000_000));
                self.start_op(cpu, OpKind::Resume, json!({"until": until, "until_ms": until_ms}));
                Reply::Json(json!({"ok": true, "paused": false}))
            }
            Cmd::Step { count } => {
                self.take_over();
                self.step_budget = Some(count.max(1));
                self.start_op(cpu, OpKind::Step, json!({"count": count.max(1)}));
                Reply::Json(json!({"ok": true, "paused": false}))
            }
            Cmd::StepOver => {
                let target = step_over_target(cpu);
                self.take_over();
                match target {
                    Some(next) => self.temp_breakpoint = Some(next),
                    None => self.step_budget = Some(1),
                }
                self.start_op(cpu, OpKind::StepOver, json!({"until": target.map(|p| format!("{:05X}", p))}));
                Reply::Json(json!({"ok": true, "paused": false}))
            }
            Cmd::WaitPause => match self.paused.then(|| self.current_stop()).flatten() {
                // Stopped: the stop, with the registers as they are now.
                Some(stop) => Reply::Json(stop_reply(Some(stop), cpu)),
                // The server waits for the next stop in the journal.
                None => Reply::Json(json!({"paused": false})),
            },
            Cmd::RebootShell => {
                // As the settings window's close does: the batch file and
                // commands queued go too, or their next line (often EXIT)
                // would run. The front end does the rest (take_closed_program).
                self.release_keys(cpu);
                cpu.close_program();
                self.closed_program = true;
                self.take_over();
                self.start_op(cpu, OpKind::RebootShell, json!({}));
                Reply::Json(json!({"ok": true}))
            }
            Cmd::Reboot => {
                cpu.bus.reset_requested = true;
                self.take_over();
                self.start_op(cpu, OpKind::Reboot, json!({}));
                Reply::Json(json!({"ok": true}))
            }
            Cmd::GetRegs => Reply::Json(regs_json(cpu)),
            Cmd::GetFpu => Reply::Json(super::status::fpu_json(cpu)),
            Cmd::SetRegs(map) => match set_regs(cpu, &map) {
                Ok(()) => {
                    let changed: Map<String, Value> = map.iter().map(|(k, v)| (k.to_ascii_lowercase(), v.clone())).collect();
                    self.record(cpu, Kind::RegistersWritten, json!({"changed": changed}));
                    Reply::Json(regs_json(cpu))
                }
                Err(e) => Reply::bad(e),
            },
            Cmd::ReadMem { addr, len } => match parse_addr(cpu, &addr) {
                Ok(a) => {
                    let len = match (a.lin, a.phys) {
                        (Some(_), _) => len,
                        // The linear frame buffer, up to its end.
                        (None, Some(p)) if cpu.bus.vbe.lfb_offset(p, 1).is_some() => {
                            len.min(cpu.bus.vbe.lfb_base.unwrap_or(0) as usize + video::vbe::VRAM_SIZE - p)
                        }
                        // The 3dfx card's window, up to its end.
                        (None, Some(p)) if cpu.bus.voodoo_at(p).is_some() => {
                            len.min(rust_dos::voodoo::WINDOW as usize - cpu.bus.voodoo_at(p).unwrap_or(0) as usize)
                        }
                        // A PowerVR card's window, up to its end.
                        (None, Some(p)) if let Some((window, offset)) = cpu.bus.powervr_at(p) => {
                            let size = match window {
                                rust_dos::powervr::Window::Registers => rust_dos::powervr::REGISTER_WINDOW,
                                rust_dos::powervr::Window::Textures => rust_dos::powervr::TEXTURE_MEMORY,
                            };
                            len.min((size - offset) as usize)
                        }
                        (None, p) => len.min(cpu.bus.ram().len().saturating_sub(p.unwrap_or(0))),
                    };
                    let data = a.read(cpu, len);
                    Reply::Bytes { addr: a.lin.map_or(a.phys.unwrap_or(0), |l| l as usize), segoff: a.segoff, data }
                }
                Err(e) => Reply::bad(e),
            },
            Cmd::WriteMem { expect: Some(expect), data, .. } if expect.len() != data.len() => {
                Reply::bad(format!("expect has {} bytes, the data {}", expect.len(), data.len()))
            }
            Cmd::WriteMem { addr, data, expect } => match parse_addr(cpu, &addr) {
                Ok(a) => {
                    let targets: Option<Vec<usize>> = (0..data.len()).map(|i| a.byte(cpu, i)).collect();
                    let peek = |cpu: &Cpu, t: &[usize]| t.iter().map(|&p| cpu.bus.peek_8(p)).collect::<Vec<u8>>();
                    match targets {
                        Some(t) if t.iter().all(|&p| p < cpu.bus.ram().len() || cpu.bus.vbe.lfb_offset(p, 1).is_some()) => {
                            let old = peek(cpu, &t);
                            let guarded = expect.is_some();
                            if let Some(expect) = expect.filter(|e| *e != old) {
                                let (found, expected) = (trace::hex_bytes(&old), trace::hex_bytes(&expect));
                                Reply::ErrorJson(
                                    409,
                                    json!({
                                        "error": format!(
                                            "the memory holds {found}, not the expected {expected}; nothing was written"
                                        ),
                                        "found": found,
                                        "expected": expected,
                                    }),
                                )
                            } else {
                                for (p, b) in t.iter().zip(&data) {
                                    // The debugger patches the ROMs too.
                                    if rust_dos::bus::Bus::is_rom(*p) {
                                        cpu.bus.write_rom(*p, &[*b]);
                                    } else {
                                        cpu.bus.write_8(*p, *b);
                                    }
                                }
                                // The debugger's own changes don't stop the
                                // machine.
                                for w in &mut self.watchpoints {
                                    w.value = Watch::read(cpu, w.phys, w.len);
                                }
                                // The bytes replaced and what reads back,
                                // which can differ from the data: VGA memory
                                // reads through the read mode.
                                let new = peek(cpu, &t);
                                let addr = format!("{:05X}", a.phys.unwrap_or(0));
                                self.record(cpu, Kind::MemoryWritten, json!({
                                    "addr": addr,
                                    "len": data.len(),
                                    "old": bytes_record(&old),
                                    "new": bytes_record(&new),
                                    "expect": guarded,
                                }));
                                Reply::Json(json!({
                                    "ok": true,
                                    "addr": addr,
                                    "written": data.len(),
                                    "old": trace::hex_bytes(&old),
                                    "new": trace::hex_bytes(&new),
                                }))
                            }
                        }
                        Some(_) => Reply::bad("write extends past end of memory"),
                        None => Reply::bad("write reaches an unmapped page"),
                    }
                }
                Err(e) => Reply::bad(e),
            },
            Cmd::Disasm { addr, count } => self.disasm(cpu, addr, count),
            Cmd::ListBreakpoints => Reply::Json(self.breakpoints_json()),
            Cmd::AddBreakpoint { addr, once } => match parse_addr(cpu, &addr).and_then(breakpoint_phys) {
                Ok(phys) => {
                    self.breakpoints.insert(phys);
                    if once {
                        self.once.insert(phys);
                    } else {
                        self.once.remove(&phys);
                    }
                    self.record(cpu, Kind::BreakpointSet, json!({"addr": format!("{:05X}", phys), "once": once}));
                    Reply::Json(self.breakpoints_json())
                }
                Err(e) => Reply::bad(e),
            },
            Cmd::RemoveBreakpoint(a) => match a {
                None => {
                    self.breakpoints.clear();
                    self.once.clear();
                    self.break_exceptions = 0;
                    self.break_mode_switch = false;
                    self.break_program_start = false;
                    self.break_program_exit = false;
                    self.record(cpu, Kind::BreakpointRemoved, json!({"all": true}));
                    Reply::Json(self.breakpoints_json())
                }
                Some(a) => match parse_addr(cpu, &a).and_then(breakpoint_phys) {
                    Ok(phys) => {
                        if self.breakpoints.remove(&phys) {
                            self.once.remove(&phys);
                            self.record(cpu, Kind::BreakpointRemoved, json!({"addr": format!("{:05X}", phys)}));
                            Reply::Json(self.breakpoints_json())
                        } else {
                            Reply::Error(404, format!("no breakpoint at {:05X}", phys))
                        }
                    }
                    Err(e) => Reply::bad(e),
                },
            },
            Cmd::ListWatchpoints => Reply::Json(self.watchpoints_json(cpu)),
            Cmd::AddWatchpoint { addr, len } => {
                if !matches!(len, 1 | 2 | 4) {
                    return Reply::bad("len is 1, 2 or 4");
                }
                match parse_addr(cpu, &addr).and_then(breakpoint_phys) {
                    Ok(phys) => {
                        self.watchpoints.retain(|w| w.phys != phys);
                        self.watchpoints.push(Watch { phys, len, value: Watch::read(cpu, phys, len) });
                        self.record(cpu, Kind::WatchpointSet, json!({"addr": format!("{:05X}", phys), "len": len}));
                        Reply::Json(self.watchpoints_json(cpu))
                    }
                    Err(e) => Reply::bad(e),
                }
            }
            Cmd::RemoveWatchpoint(addr) => match addr {
                None => {
                    self.watchpoints.clear();
                    self.record(cpu, Kind::WatchpointRemoved, json!({"all": true}));
                    Reply::Json(self.watchpoints_json(cpu))
                }
                Some(a) => match parse_addr(cpu, &a).and_then(breakpoint_phys) {
                    Ok(phys) => {
                        let before = self.watchpoints.len();
                        self.watchpoints.retain(|w| w.phys != phys);
                        if self.watchpoints.len() < before {
                            self.record(cpu, Kind::WatchpointRemoved, json!({"addr": format!("{:05X}", phys)}));
                            Reply::Json(self.watchpoints_json(cpu))
                        } else {
                            Reply::Error(404, format!("no watchpoint at {:05X}", phys))
                        }
                    }
                    Err(e) => Reply::bad(e),
                },
            },
            Cmd::BreakOn { exceptions, clear_exceptions, mode_switch, program_start, program_exit } => {
                // The counts go stale while nothing watches them. While
                // something does, a program that started or ended in the
                // last instruction of the batch is still to be seen.
                let run_line_waits = self.run_wait.as_ref().is_some_and(|r| r.before.is_none());
                if !self.break_program_start && !self.start_once && !run_line_waits {
                    self.seen_starts = cpu.programs.started;
                }
                if !self.break_program_exit {
                    self.seen_ends = cpu.programs.ended;
                }
                if let Some(mask) = exceptions {
                    self.break_exceptions |= mask;
                }
                if let Some(mask) = clear_exceptions {
                    self.break_exceptions &= !mask;
                }
                if let Some(m) = mode_switch {
                    self.break_mode_switch = m;
                }
                if let Some(b) = program_start {
                    self.break_program_start = b;
                }
                if let Some(b) = program_exit {
                    self.break_program_exit = b;
                }
                self.seen_exceptions = cpu.exceptions;
                self.seen_mode_switches = cpu.mode_switches;
                let list = self.breakpoints_json();
                let fields = json!({
                    "exceptions": list["exceptions"],
                    "mode_switch": self.break_mode_switch,
                    "program_start": self.break_program_start,
                    "program_exit": self.break_program_exit,
                });
                self.record(cpu, Kind::BreakOn, fields);
                Reply::Json(list)
            }
            Cmd::Run { command, stop_at_entry, queue } => {
                let idle = at_idle_prompt(cpu);
                if self.run_wait.is_some() {
                    Reply::Error(409, "a run command is already waiting for its program".into())
                } else if !idle && !queue {
                    Reply::Error(409, "the DOS prompt is busy with a program, a batch file, PAUSE, CHOICE or EDIT".into())
                } else if command.trim().is_empty() || command.contains(['\r', '\n']) {
                    Reply::bad("command: one command line")
                } else {
                    // A batch line after the ones queued: the shell takes
                    // it once they have run and the prompt is back, at once
                    // when it is idle.
                    let op = self.cur.as_ref().map_or(0, |c| c.req);
                    cpu.batch.append_marked(command.trim(), op);
                    self.take_over();
                    self.seen_starts = cpu.programs.started;
                    self.run_wait = Some(RunWait { before: None, stop_at_entry, op });
                    let fields = json!({"command": command.trim(), "stop_at_entry": stop_at_entry, "queued": !idle});
                    self.start_op(cpu, OpKind::Run, fields);
                    Reply::Json(json!({"ok": true}))
                }
            }
            Cmd::Ivt => Reply::Json(ivt_json(cpu)),
            Cmd::Gdt => Reply::Json(pm::table_json(cpu, false, 1024)),
            Cmd::Ldt => Reply::Json(pm::table_json(cpu, true, 1024)),
            Cmd::Idt => Reply::Json(pm::idt_json(cpu)),
            Cmd::Tss => Reply::Json(pm::tss_json(cpu)),
            Cmd::PageWalk(addr) => match parse_addr(cpu, &addr) {
                Ok(a) => Reply::Json(pm::pagewalk_json(cpu, a.lin.unwrap_or(a.phys.unwrap_or(0) as u32))),
                Err(e) => Reply::bad(e),
            },
            Cmd::Xms => Reply::Json(pm::xms_json(cpu)),
            Cmd::Net => Reply::Json(net_json(cpu)),
            Cmd::Serial => Reply::Json(serial_json(cpu)),
            Cmd::Printer { eject } => Reply::Json(printer_json(cpu, eject)),
            Cmd::Gus => Reply::Json(match &cpu.bus.gus {
                Some(gus) => gus.snapshot(),
                None => serde_json::json!({ "installed": false }),
            }),
            Cmd::Sound => Reply::Json(sound_json(cpu)),
            Cmd::Awe32 => Reply::Json(match &cpu.bus.awe {
                Some(awe) => awe.snapshot(),
                None => serde_json::json!({ "installed": false }),
            }),
            Cmd::Exceptions => Reply::Json(pm::exceptions_json(cpu)),
            Cmd::Win32 => Reply::Json(super::win32::win32_json(cpu)),
            Cmd::Drives => Reply::Json(drives_json(cpu)),
            Cmd::SyncShared => {
                let messages = cpu.bus.sync_shared(None);
                self.record(cpu, Kind::SharedSynced, json!({"messages": messages}));
                Reply::Json(json!({"messages": messages}))
            }
            Cmd::SwapImages => {
                let messages = cpu.bus.swap_images();
                for message in &messages {
                    cpu.bus.log_string(&format!("[DEBUG] {}", message));
                }
                self.record(cpu, Kind::ImagesSwapped, json!({"messages": messages}));
                Reply::Json(json!({"messages": messages, "drives": drives_json(cpu)}))
            }
            Cmd::Mount { drive, path, kind, label, read_only, images } => {
                match mount_drive(cpu, &drive, &path, kind.as_deref(), label, read_only, images) {
                    Ok(root) => {
                        cpu.bus.log_string(&format!(
                            "[DEBUG] Drive {} mounted to {}",
                            drive,
                            root.display()
                        ));
                        let drives = drives_json(cpu);
                        let key = parse_drive_name(&drive).map(crate::disk::drive_key);
                        let mounted = key.as_ref().and_then(|k| drives["drives"].get(k).cloned()).unwrap_or(Value::Null);
                        self.record(cpu, Kind::DriveMounted, json!({"drive": key, "mounted": mounted}));
                        Reply::Json(drives)
                    }
                    Err(e) => Reply::bad(e),
                }
            }
            Cmd::Unmount { drive } => {
                let parsed = parse_drive_name(&drive);
                let result = parsed
                    .ok_or_else(|| format!("invalid drive letter '{}'", drive))
                    .and_then(|d| cpu.bus.unmount_drive(d));
                match result {
                    Ok(()) => {
                        self.record(cpu, Kind::DriveUnmounted, json!({"drive": parsed.map(crate::disk::drive_key)}));
                        Reply::Json(drives_json(cpu))
                    }
                    Err(e) => Reply::bad(e),
                }
            }
        }
    }
}

/// Commands that change the machine (its input, memory, registers, drives,
/// speed or state), which deterministic mode takes only while it is paused.
fn changes_machine(cmd: &Cmd) -> bool {
    matches!(
        cmd,
        Cmd::Input { .. }
            | Cmd::Run { .. }
            | Cmd::WriteMem { .. }
            | Cmd::SetRegs(_)
            | Cmd::Mount { .. }
            | Cmd::Unmount { .. }
            | Cmd::SwapImages
            | Cmd::LoadState { .. }
            | Cmd::Speed { .. }
            | Cmd::Reboot
            | Cmd::RebootShell
    )
}

/// Parse a hex number with optional `0x` prefix / `h` suffix.
/// Where stepping over the instruction at CS:EIP stops: the physical
/// address of the next instruction, when the instruction is one that runs
/// code or repeats before getting there (a call, an interrupt, a loop, a
/// repeated string instruction). None for the others, which a single step
/// does.
fn step_over_target(cpu: &Cpu) -> Option<usize> {
    let cs = cpu.seg_cache(crate::cpu::Seg::CS);
    let code32 = cs.attr & crate::cpu::ATTR_DB != 0;
    let eip = cpu.eip();
    let bytes: Vec<u8> = (0..15u32)
        .map(|i| cpu.peek_translate(cs.base.wrapping_add(eip.wrapping_add(i))).map_or(0xFF, |p| cpu.bus.peek_8(p as usize)))
        .collect();
    let len = step_over_len(&bytes, code32)?;
    let next = eip.wrapping_add(len as u32);
    let next = if code32 { next } else { next & 0xFFFF };
    cpu.peek_translate(cs.base.wrapping_add(next)).map(|p| p as usize)
}

/// The length of the instruction in `bytes` if stepping over it runs to
/// the instruction after it, else None.
pub(super) fn step_over_len(bytes: &[u8], code32: bool) -> Option<usize> {
    use iced_x86::{Decoder, DecoderOptions, FlowControl, Mnemonic};
    // The emulator's own interrupt handlers run in one step.
    if bytes.len() >= 2 && bytes[0] == 0xFE && matches!(bytes[1], 0x38 | 0x39) {
        return None;
    }
    let mut decoder = Decoder::new(if code32 { 32 } else { 16 }, bytes, DecoderOptions::NONE);
    let instr = decoder.decode();
    if instr.is_invalid() {
        return None;
    }
    let over = matches!(instr.flow_control(), FlowControl::Call | FlowControl::IndirectCall | FlowControl::Interrupt)
        || matches!(instr.mnemonic(), Mnemonic::Loop | Mnemonic::Loope | Mnemonic::Loopne)
        || (instr.is_string_instruction() && (instr.has_rep_prefix() || instr.has_repe_prefix() || instr.has_repne_prefix()));
    over.then_some(instr.len())
}

/// Where a breakpoint at an address goes: its physical address.
fn breakpoint_phys(a: pm::DebugAddr) -> Result<usize, String> {
    a.phys.ok_or_else(|| "address is on an unmapped page".to_string())
}

/// Mount (or replace) a drive from the debug API. Without explicit options
/// a remount keeps the drive's current type, label and read-only flag.
fn mount_drive(
    cpu: &mut Cpu,
    drive: &str,
    path: &str,
    kind: Option<&str>,
    label: Option<String>,
    read_only: bool,
    images: Vec<String>,
) -> Result<PathBuf, String> {
    let drive =
        parse_drive_name(drive).ok_or_else(|| format!("invalid drive letter '{}'", drive))?;
    let kind = match kind {
        Some(k) => Some(parse_kind(k).ok_or_else(|| format!("unknown drive type '{}'", k))?),
        None => None,
    };
    let explicit = kind.is_some() || label.is_some() || read_only;
    let path = PathBuf::from(path);
    let mut opts = match cpu.bus.disk.drive_info(drive) {
        // An image brings its own type and label.
        Some(info) if !explicit && !path.is_file() => MountOptions {
            kind: info.kind,
            label: info.image.is_none().then_some(info.label),
            read_only: info.read_only,
            ..Default::default()
        },
        _ => MountOptions {
            kind: kind.unwrap_or(DriveKind::HardDisk),
            label,
            read_only,
            ..Default::default()
        },
    };
    opts.more_images = images.into_iter().map(PathBuf::from).collect();
    cpu.bus.mount_drive(drive, &path, opts, true)
}
