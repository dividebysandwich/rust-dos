//! Save state and speed requests the front end carries out.

use serde_json::{Value, json};

use super::journal::{Kind, OpState};
use super::{DebugHub, LowInput, Need, PauseReason, Reply, PlaybackRequest, StateRequest};
use crate::cpu::Cpu;

impl DebugHub {
    /// The next save state a remote client asked to save or load, for the
    /// front end to carry out and answer with `state_done`. Requests that
    /// can't be carried out now are refused here (409, from `check`): a
    /// strict-match save or load while the machine isn't stopped with
    /// nothing pending (they are only done while the debugger holds the
    /// machine between two instructions), a load in deterministic mode
    /// while the machine runs (a `resume` sent in the same frame starts it
    /// before the load is carried out), and one whose `If-State-Seq` no
    /// longer holds. The others are carried out at the end of the frame,
    /// running or not.
    pub fn next_state_request(&mut self) -> Option<StateRequest> {
        while !self.state_requests.is_empty() {
            let request = self.state_requests.remove(0);
            let mut refusal = Ok(());
            if request.load {
                refusal = self.check(Need::Paused);
            }
            if request.strict_match {
                refusal = refusal.and_then(|_| {
                    self.check(Need::Quiescent { input_ok: request.load }).map_err(|mut body| {
                        let what = if request.load { "load" } else { "save" };
                        body["error"] = format!("can't do a strict-match {}: {}", what, body["error"].as_str().unwrap_or("")).into();
                        body
                    })
                });
            }
            if let Some(n) = request.if_state {
                refusal = refusal.and_then(|_| self.check(Need::State(n)));
            }
            match refusal {
                Err(body) => self.answer(&request.ctx, request.reply, Reply::ErrorJson(409, body)),
                Ok(()) => {
                    self.begin(request.ctx.clone());
                    return Some(request);
                }
            }
        }
        None
    }

    /// The front end carried out `request` from `next_state_request`:
    /// `result` is its reply, or the status and body of its failure. A
    /// save appends `state_saved`; a load `state_loaded`, and when strict,
    /// resets the debugger (`debugger_reset`); a load while paused stops
    /// the machine in the loaded state (`paused`, reason `load`). A failed
    /// load appends nothing.
    pub fn state_done(&mut self, cpu: &Cpu, request: StateRequest, result: Result<Value, (u16, Value)>) {
        let path = request.path.display().to_string();
        let reply = match result {
            Err((status, body)) => Reply::ErrorJson(status, body),
            Ok(mut value) if request.load => {
                #[cfg(feature = "mcp")]
                if !request.strict_match { self.invalidate_owned_after_load(cpu); }
                self.told_starts = cpu.programs.started;
                self.told_ends = cpu.programs.ended;
                let reset = request.strict_match.then(|| self.reset_after_load(cpu));
                let differences = value.get("differences").cloned().unwrap_or_else(|| json!([]));
                let mut fields = json!({"path": path, "strict_match": request.strict_match, "source": "debugger", "differences": differences});
                if let Some(folders) = value.get("folders") {
                    fields["folders"] = folders.clone();
                }
                self.record(cpu, Kind::StateLoaded, fields);
                if let Some((cleared, cancelled)) = reset {
                    value["debugger_reset"] = cleared.clone();
                    let ids = cancelled.clone();
                    let cause = Some(request.ctx.req);
                    self.append_then(cpu, Kind::DebuggerReset, cause, json!({"cleared": cleared, "cancelled": cancelled}), |j, seq| {
                        for req in ids {
                            j.end_op(req, OpState::Cancelled, Some(seq));
                        }
                    });
                }
                if self.paused {
                    self.pause_hit = Some(PauseReason::Load);
                    self.flush_stop(cpu);
                }
                Reply::Json(value)
            }
            Ok(value) => {
                let fields = json!({
                    "path": path,
                    "strict_match": request.strict_match,
                    "format": value.get("format"),
                    "emulator": value.get("emulator"),
                });
                self.record(cpu, Kind::StateSaved, fields);
                Reply::Json(value)
            }
        };
        let ctx = self.finish();
        self.answer(&ctx, request.reply, reply);
    }

    /// Whether deterministic mode refuses a change to the machine now: it
    /// runs, and the settings window doesn't hold it.
    pub(super) fn deterministic_refuses(&self) -> bool {
        self.deterministic.is_some() && !self.paused && !self.divert
    }

    /// Forget what the debugger was told before a save state was loaded,
    /// so none of it stops the machine the state brought: breakpoints,
    /// watchpoints, the stops on exceptions, mode switches and program
    /// starts and ends, a step or the target of a step-over or run-to, and
    /// queued input. Returns what was cleared, and the requests whose
    /// operations it cancelled: the control operation under way, a `run`
    /// waiting for its program, and queued input.
    pub fn reset_after_load(&mut self, cpu: &Cpu) -> (Value, Vec<u64>) {
        let input = self.input_pending();
        let held_keys = self.remote_held.len();
        #[cfg(feature = "mcp")]
        let native_cancelled = self.native_invalidate_restored(cpu);
        let break_on: Vec<&str> = [
            (self.break_exceptions != 0, "exceptions"),
            (self.break_mode_switch, "mode_switch"),
            (self.break_program_start, "program_start"),
            (self.break_program_exit, "program_exit"),
        ]
        .into_iter()
        .filter_map(|(on, name)| on.then_some(name))
        .collect();
        let mut cancelled: Vec<u64> = self.current_op.take().into_iter().collect();
        let cleared = json!({
            "breakpoints": std::mem::take(&mut self.breakpoints).len(),
            "watchpoints": std::mem::take(&mut self.watchpoints).len(),
            "break_on": break_on,
            "run_to": self.temp_breakpoint.take().is_some(),
            "step": self.step_budget.take().is_some(),
            // The control operation that was to end with a stop.
            "pause_waits": cancelled.len(),
            "run": self.run_wait.is_some(),
            "input": input,
            "held_keys": held_keys,
            // Deterministic mode: a resume's stop at an emulated time.
            "until_ms": self.stop_at_ns.take().is_some(),
        });
        #[cfg(feature = "mcp")]
        cancelled.extend(native_cancelled);
        self.once.clear();
        // Keys held before the load are forgotten without a release: the
        // restored keyboard has the keys of the moment it was saved, and
        // a key-up now would reach the restored machine.
        self.remote_held.clear();
        self.remote_mods = 0;
        if let Some(run) = self.run_wait.take() {
            cancelled.push(run.op);
        }
        for event in self.input.drain(..) {
            if let LowInput::End { req } = event {
                cancelled.push(req);
            }
        }
        #[cfg(feature = "mcp")]
        { self.owned_input.spans.clear(); self.owned_input.wait_owner = None; }
        cancelled.sort_unstable();
        cancelled.dedup();
        self.input_wait_until = None;
        self.input_wait_ns = None;
        self.break_exceptions = 0;
        self.break_mode_switch = false;
        self.break_program_start = false;
        self.break_program_exit = false;
        self.start_once = false;
        self.start_breakpoint = None;
        self.skip_bp_once = false;
        self.pause_hit = None;
        self.pause_by = None;
        self.breakpoint_hit = None;
        self.watch_hit = None;
        // The loaded machine's counts are where the debugger starts from.
        self.seen_exceptions = cpu.exceptions;
        self.seen_mode_switches = cpu.mode_switches;
        self.seen_starts = cpu.programs.started;
        self.seen_ends = cpu.programs.ended;
        (cleared, cancelled)
    }

    /// The next execution control a remote client asked for, for the front end
    /// to apply and answer with `playback_done`. One that can't be applied
    /// now (deterministic mode running, or its `If-State-Seq` no longer
    /// holding) is refused here.
    pub fn next_playback_request(&mut self) -> Option<PlaybackRequest> {
        while !self.playback_requests.is_empty() {
            let request = self.playback_requests.remove(0);
            if self.quit.is_some() || self.shared.as_ref().is_some_and(|shared| shared.closed.load(std::sync::atomic::Ordering::Acquire)) {
                self.answer(&request.ctx, request.reply, Reply::Error(503, "the emulator is shutting down".into()));
                continue;
            }
            if request.deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline || request.reply.is_closed()) {
                self.answer(&request.ctx, request.reply, Reply::Error(408, "the request expired before frontend application".into()));
                continue;
            }
            let changes_machine = request.action.changes_machine();
            let refusal = (if changes_machine { self.check(Need::Paused) } else { Ok(()) }).and_then(|_| request.if_state.map_or(Ok(()), |n| self.check(Need::State(n))));
            match refusal {
                Err(body) => self.answer(&request.ctx, request.reply, Reply::ErrorJson(409, body)),
                Ok(()) => {
                    self.begin(request.ctx.clone());
                    return Some(request);
                }
            }
        }
        None
    }

    /// The front end carried out `request`: `result` is the current control
    /// state or a rejection. A CPU-speed mutation appends `speed_changed`.
    pub fn playback_done(&mut self, cpu: &Cpu, request: PlaybackRequest, result: Result<Value, String>) {
        let reply = match result {
            Ok(value) => {
                if matches!(request.action, crate::playback::Action::Speed(Some(_))) {
                    self.record(cpu, Kind::SpeedChanged, json!({"cycles": value.get("cycles"), "source": "debugger"}));
                }
                Reply::Json(value)
            }
            Err(e) => Reply::bad(e),
        };
        let ctx = self.finish();
        self.answer(&ctx, request.reply, reply);
    }
}
