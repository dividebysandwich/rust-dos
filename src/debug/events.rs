//! Stops, operations and the events the journal records.

use std::sync::atomic::Ordering;
use std::time::Duration;

use serde_json::{Value, json};

use super::journal::{Kind, OpKind, OpState};
use super::status::{cs_ip, exit_json, program_json};
use super::{Answer, DebugHub, LowInput, Meta, Need, PauseReason, Reply, ShutdownReason, pm, regs_json};
use crate::cpu::Cpu;

impl DebugHub {
    /// Append the `paused` entry of the stop the batch or a request made,
    /// if there is one, ending the operation under way. Returns the entry.
    pub(super) fn flush_stop(&mut self, cpu: &Cpu) -> Option<Value> {
        let reason = self.pause_hit.take()?;
        let mut stop = json!({"reason": reason.name()});
        if let Some(hit) = self.breakpoint_hit.take() {
            if hit.user {
                stop["breakpoint"] = format!("{:05X}", hit.phys).into();
                stop["once"] = hit.once.into();
            }
            if hit.target {
                stop["target"] = format!("{:05X}", hit.phys).into();
            }
        }
        match reason {
            PauseReason::ProgramStart => stop["program"] = program_json(cpu),
            PauseReason::ProgramExit => stop["exit"] = exit_json(cpu),
            _ => {}
        }
        if reason == PauseReason::Exception
            && let Some(e) = pm::exceptions_json(cpu)["recent"].as_array().and_then(|a| a.last().cloned())
        {
            stop["exception"] = e;
        }
        if let Some((phys, len, old, new)) = self.watch_hit.take() {
            let digits = len as usize * 2;
            stop["watch"] = json!({
                "addr": format!("{:05X}", phys),
                "len": len,
                "old": format!("{:0width$X}", old, width = digits),
                "new": format!("{:0width$X}", new, width = digits),
            });
        }
        let by = self.pause_by.take();
        if reason == PauseReason::Request {
            stop["by"] = by.into();
        }
        stop["cs_ip"] = cs_ip(cpu).into();
        stop["registers"] = regs_json(cpu);
        if self.trace.total() > 0 {
            stop["trace"] = self.trace.total().into();
        }
        if reason == PauseReason::ProgramStart && self.run_wait.as_ref().is_some_and(|r| r.stop_at_entry && r.before.is_some()) {
            self.run_wait = None;
        }
        self.host_input_told = false;
        let op = self.current_op;
        // A `run` whose operation this stop ends waits no more: it no
        // longer stops at its program's entry or blocks another `run`.
        if op.is_some() && self.run_wait.as_ref().is_some_and(|r| Some(r.op) == op) {
            self.run_wait = None;
            self.start_once = false;
        }
        let cause = match reason {
            PauseReason::Request => by,
            PauseReason::Startup => None,
            PauseReason::Load => self.cur.as_ref().map(|c| c.req),
            _ => op,
        };
        let mut fallback = stop.clone();
        let seq = self.append_then(cpu, Kind::Paused, cause, stop, |j, seq| {
            if let Some(op) = op {
                j.end_op(op, OpState::Stopped, Some(seq));
            }
        });
        self.current_op = None;
        // The whole entry, as the journal gives it.
        if let Some(entry) = seq.and_then(|s| self.with_journal(|j| j.get(s).map(|e| e.value())).flatten()) {
            self.stop_entry = Some(entry.clone());
            return Some(entry);
        }
        fallback["icount"] = cpu.executed.into();
        Some(fallback)
    }

    /// The `paused` entry the machine is stopped in, if it is.
    pub(super) fn current_stop(&self) -> Option<Value> {
        let seq = self.with_journal(|j| j.stop_seq).flatten()?;
        self.stop_entry.as_ref().filter(|e| e["seq"].as_u64() == Some(seq)).cloned()
    }

    /// Start a control request's operation of `kind`: it takes over the
    /// machine from the one under way, which is superseded, and resumes
    /// it, appending `resumed` with `fields`.
    pub(super) fn start_op(&mut self, cpu: &Cpu, kind: OpKind, mut fields: Value) {
        let Some((req, tag)) = self.cur.as_ref().map(|c| (c.req, c.tag.clone())) else { return };
        let supersedes = self.current_op.replace(req);
        fields["kind"] = kind.name().into();
        fields["from_paused"] = self.paused.into();
        fields["supersedes"] = supersedes.into();
        self.resume();
        self.append_then(cpu, Kind::Resumed, Some(req), fields, |j, seq| {
            if let Some(old) = supersedes {
                j.supersede_op(old, req);
            }
            j.begin_op(req, kind, seq, tag);
        });
        if let Some(ctx) = self.cur.as_mut() {
            ctx.op = Some(req);
        }
    }

    /// A control request takes over the machine: the step count, the
    /// run-to target and the time stop (`until_ms`) of the operation it
    /// supersedes stop nothing more.
    /// A `run` whose operation is superseded doesn't stop at its program's
    /// entry and no longer blocks another `run` or a strict-match save; the
    /// command line it queued still runs.
    pub(super) fn take_over(&mut self) {
        self.step_budget = None;
        self.temp_breakpoint = None;
        self.stop_at_ns = None;
        if self.run_wait.as_ref().is_some_and(|r| self.current_op == Some(r.op)) {
            self.run_wait = None;
            self.start_once = false;
        }
    }

    /// Program starts and ends since the journal last told them, as
    /// `program_started` and `program_exited` (with `count` when there were
    /// several in the batch). When both came in one batch, the start goes
    /// first if the program that started no longer runs (its PSP isn't the
    /// current one): it ended in the batch too.
    pub(super) fn tell_programs(&mut self, cpu: &Cpu) {
        let (started, ended) = (cpu.programs.started, cpu.programs.ended);
        let new_start = started > self.told_starts;
        let new_end = ended > self.told_ends;
        let start_first = new_start && new_end && cpu.current_psp != cpu.programs.psp;
        if new_end && !start_first {
            self.tell_exit(cpu, ended);
        }
        if new_start {
            let mut fields = json!({"program": program_json(cpu)});
            if started - self.told_starts > 1 {
                fields["count"] = (started - self.told_starts).into();
            }
            // A `run` that doesn't stop at the entry is done with this entry.
            let run = self.run_wait.as_ref().filter(|r| !r.stop_at_entry && r.before.is_some_and(|b| started > b)).map(|r| r.op);
            let seq = self.append_then(cpu, Kind::ProgramStarted, None, fields, |j, seq| {
                if let Some(op) = run {
                    j.end_op(op, OpState::Done, Some(seq));
                }
            });
            self.last_start_seq = seq.or(self.last_start_seq);
            if let Some(op) = run.filter(|_| seq.is_some()) {
                self.run_wait = None;
                if self.current_op == Some(op) {
                    self.current_op = None;
                }
            }
        }
        if start_first {
            self.tell_exit(cpu, ended);
        }
        // A reboot or a load can take the counts back.
        self.told_starts = started;
        self.told_ends = ended;
    }

    /// The `program_exited` entry of the ends since the journal last told
    /// them, `ended` now.
    fn tell_exit(&mut self, cpu: &Cpu, ended: u64) {
        let mut fields = json!({"exit": exit_json(cpu)});
        if ended - self.told_ends > 1 {
            fields["count"] = (ended - self.told_ends).into();
        }
        self.append(cpu, Kind::ProgramExited, None, fields);
    }

    /// Whether `need` holds now; if not, the 409 body that says why.
    pub fn check(&self, need: Need) -> Result<(), Value> {
        let (stop_seq, state_seq, pending_ops) = self
            .with_journal(|j| (j.stop_seq, j.state_seq, j.pending_ops(|k| k != OpKind::Input)))
            .unwrap_or((None, 0, Vec::new()));
        let input_queued = self.input_pending();
        let why: Option<(&str, String)> = match need {
            Need::Paused => self.deterministic_refuses().then(|| {
                (
                    "paused",
                    "deterministic mode takes input and changes to the machine only while it is paused: \
                     pause it, or resume with until_ms, then send the request"
                        .to_string(),
                )
            }),
            Need::Quiescent { input_ok } => {
                let why = if !self.paused {
                    Some("the machine runs; pause it first (POST /api/control/pause)")
                } else if self.rewinding {
                    Some("the machine is being rewound")
                } else if self.step_budget.is_some() {
                    Some("a step is pending")
                } else if self.run_wait.is_some() {
                    Some("a run command waits for its program")
                } else if !input_ok && input_queued > 0 {
                    Some("queued input is still being typed")
                } else {
                    None
                };
                why.map(|w| ("quiescent", w.to_string()))
            }
            Need::State(n) => {
                // Input goes on being typed while paused, but in
                // deterministic mode, where it waits for emulated time.
                let typing = input_queued > 0 && (self.deterministic.is_none() || self.divert);
                let why = if !self.paused || stop_seq.is_none() {
                    Some("the machine runs; pause it, then read its state again".to_string())
                } else if self.divert {
                    Some("the settings window is open".to_string())
                } else if self.rewinding {
                    Some("the machine is being rewound".to_string())
                } else if !pending_ops.is_empty() {
                    Some("an operation is under way".to_string())
                } else if typing {
                    Some("queued input is being typed".to_string())
                } else if state_seq != n {
                    Some(format!("the state changed since entry {}: it is now at entry {}", n, state_seq))
                } else {
                    None
                };
                why.map(|w| ("state", w))
            }
        };
        match why {
            None => Ok(()),
            Some((precondition, error)) => Err(json!({
                "error": error,
                "precondition": precondition,
                "paused": self.paused,
                "stop_seq": stop_seq,
                "state_seq": state_seq,
                "pending_ops": pending_ops,
                "input_queued": input_queued,
                "settings_window": self.divert,
            })),
        }
    }

    /// The input events still to be delivered, counting a wait under way:
    /// an input request whose last event is a wait isn't delivered until
    /// the wait ends.
    pub(super) fn input_pending(&self) -> usize {
        let waiting = self.input_wait_until.is_some() || self.input_wait_ns.is_some();
        self.input.iter().filter(|e| !matches!(e, LowInput::End { .. })).count() + waiting as usize
    }

    /// Whether a remote client asked to turn the emulator off.
    pub fn quit_requested(&self) -> bool {
        self.quit.is_some()
    }

    /// The emulator turns off: what is pending is cancelled, the journal
    /// ends with `shutdown`, requests still coming get 503, and the server
    /// gets a moment to send what it has.
    pub fn shutdown(&mut self, cpu: &Cpu, reason: ShutdownReason, exit_code: i32) {
        let Some(shared) = self.shared.clone() else { return;
        };
        let by = self.quit.take();
        let cancelled = self.with_journal(|j| j.pending_ops(|_| true)).unwrap_or_default();
        self.current_op = None;
        let fields = json!({
            "reason": match reason {
                ShutdownReason::Quit => "quit",
                ShutdownReason::Exit => "exit",
                ShutdownReason::Window => "window",
            },
            "by": by.as_ref().map(|c| c.req),
            "cancelled": cancelled,
            "exit_code": exit_code,
        });
        if let Some(ctx) = by {
            self.begin(ctx);
        }
        let ids = cancelled.clone();
        self.append_then(cpu, Kind::Shutdown, self.cur.as_ref().map(|c| c.req), fields, move |j, seq| {
            for req in ids {
                j.end_op(req, OpState::Cancelled, Some(seq));
            }
        });
        self.finish();
        shared.closed.store(true, Ordering::Release);
        let gone = || Reply::Error(503, "the emulator is shutting down".into());
        for (ctx, tx) in std::mem::take(&mut self.frame_waiters) {
            self.answer(&ctx, tx, gone());
        }
        for request in std::mem::take(&mut self.playback_requests) {
            self.answer(&request.ctx, request.reply, gone());
        }
        // Requests still queued get 503, and the channel closes: one sent
        // later fails at once, and the server answers it with 503 too.
        if let Some(rx) = self.rx.take() {
            while let Ok(req) = rx.try_recv() {
                let _ = req.reply.send(Answer { reply: gone(), meta: Meta::default() });
            }
        }
        shared.drain(Duration::from_secs(2));
    }

    /// Window input reached the machine while the debugger holds it: one
    /// `host_input` entry a stop.
    pub fn host_input(&mut self, cpu: &Cpu) {
        if self.paused && !self.host_input_told {
            self.host_input_told = true;
            self.append(cpu, Kind::HostInput, None, json!({}));
        }
    }

    /// The settings window opened or closed.
    pub fn settings_window(&mut self, cpu: &Cpu, open: bool) {
        self.append(cpu, Kind::SettingsWindow, None, json!({"open": open}));
    }

    /// The speed changed otherwise than through the debug server.
    pub fn speed_changed(&mut self, cpu: &Cpu, cycles: &str, source: &str) {
        self.append(cpu, Kind::SpeedChanged, None, json!({"cycles": cycles, "source": source}));
    }

    /// A save state was loaded otherwise than through the debug server:
    /// from the window (`source` "window") or by rewinding ("rewind").
    pub fn state_loaded_elsewhere(&mut self, cpu: &Cpu, source: &str) {
        #[cfg(feature = "mcp")]
        self.invalidate_owned_after_load(cpu);
        self.told_starts = cpu.programs.started;
        self.told_ends = cpu.programs.ended;
        self.append(cpu, Kind::StateLoaded, None, json!({"source": source}));
        if self.paused {
            self.pause_hit = Some(PauseReason::Load);
            self.flush_stop(cpu);
        }
    }
}
