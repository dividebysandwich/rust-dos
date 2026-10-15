//! Per-frame hooks: execution batches, breakpoints, pauses and stats.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::journal::{Kind, OpState};
use super::status::at_idle_prompt;
use super::trace::TraceEntry;
use super::{Answer, BreakpointHit, DebugHub, PauseReason, Reply, Watch, heat, journal};
use crate::cpu::Cpu;
use crate::video;

impl DebugHub {
    // ----- per-frame hooks ------------------------------------------------

    /// Drain pending requests and feed queued input. Call once per frame,
    /// before the execution batch.
    pub fn poll(&mut self, cpu: &mut Cpu) {
        // (A profile from `--profile` runs without the server.)
        self.profile_poll(cpu);
        #[cfg(feature = "mcp")]
        {
            self.native_sync_picture(cpu);
            self.native_guard_target(cpu);
            self.native_mouse_guard(cpu);
            self.native_expire(cpu);
        }
        if self.frame_waiters.iter().any(|(ctx, reply)| {
            ctx.capture
                .as_ref()
                .is_some_and(|policy| reply.is_closed() || Instant::now() >= policy.deadline)
        }) {
            let waiters = std::mem::take(&mut self.frame_waiters);
            for (ctx, reply) in waiters {
                if let Some(policy) = &ctx.capture {
                    if reply.is_closed() {
                        continue;
                    }
                    if Instant::now() >= policy.deadline {
                        self.answer(
                            &ctx,
                            reply,
                            Reply::Error(408, "the queued capture expired".into()),
                        );
                        continue;
                    }
                }
                self.frame_waiters.push((ctx, reply));
            }
        }
        let Some(rx) = self.rx.take() else { return };
        // A save state or speed request is carried out by the front end
        // after this, before any request behind it, so that requests take
        // effect in the order they came: those behind it wait a frame. After
        // a quit nothing more is carried out.
        let bounded = self
            .shared
            .as_ref()
            .is_some_and(|shared| shared.native_enabled.load(Ordering::Acquire));
        let started = Instant::now();
        let mut handled = 0;
        while self.state_requests.is_empty() && self.playback_requests.is_empty() && self.quit.is_none() {
            if bounded
                && (handled >= 16 || (handled > 0 && started.elapsed() >= Duration::from_millis(1)))
            {
                break;
            }
            let Ok(req) = rx.try_recv() else { break };
            self.handle(cpu, req);
            handled += 1;
        }
        self.rx = Some(rx);
        self.update_heat(cpu);

        // Deterministic mode delivers the machine's input at its ticks
        // (`feed_input`); the settings window's goes as it comes.
        if self.deterministic.is_none() || self.divert {
            self.process_input(cpu);
        }
        self.check_run(cpu);

        let mode = (cpu.bus.video_mode, video::frame_size(&cpu.bus));
        if self.last_mode != Some(mode) {
            let (w, h) = cpu.bus.video_mode.dimensions();
            self.emit(json!({
                "type": "video_mode",
                "mode": cpu.bus.video_mode as u8,
                "name": format!("{:?}", cpu.bus.video_mode),
                "width": w, "height": h,
                "frame": {"width": mode.1.0, "height": mode.1.1},
            }));
            self.last_mode = Some(mode);
        }
    }

    /// Prepare for an execution batch. Returns true when the per-instruction
    /// hook (`before_exec`) must run.
    #[inline]
    pub fn begin_batch(&mut self, cpu: &Cpu) -> bool {
        let streaming = self.shared.as_ref().is_some_and(|s| s.trace.receiver_count() > 0);
        self.tracing_now = self.trace_enabled || streaming;
        if self.tracing_now {
            self.batch_t_us = cpu.bus.start_time.elapsed().as_micros() as u64;
        }
        // A `run`'s line waits behind the batch lines. The programs they
        // start may have run without the hook, which counts the starts
        // before the line from here. (Unless the shell took the line at
        // the end of the last batch.)
        self.note_run_line(cpu);
        if self.run_wait.as_ref().is_some_and(|r| r.before.is_none()) && !self.break_program_start && !self.start_once {
            self.seen_starts = cpu.programs.started;
        }
        self.tracing_now
            || self.step_budget.is_some()
            || self.temp_breakpoint.is_some()
            || !self.breakpoints.is_empty()
            || self.break_exceptions != 0
            || self.break_mode_switch
            || !self.watchpoints.is_empty()
            || self.break_program_start
            || self.break_program_exit
            || self.start_once
            || self.start_breakpoint.is_some()
            // A `run`'s line waits: the hook runs while the shell's code
            // does, which is where the shell takes batch lines, and a batch
            // run without it ends before the shell takes any while the line
            // waits (`StopReason::Hook`). The programs the lines before it
            // start run without the hook.
            || (cpu.batch.marked_line_waits() && cpu.in_shell_code())
    }

    /// Per-instruction hook, called just before an instruction at `phys_ip`
    /// executes. Returns true if execution must stop (the hub is now paused).
    #[inline]
    fn check_before_exec(&mut self, cpu: &Cpu, phys_ip: usize, ram: &[u8]) -> bool {
        // Memory the last instruction (or an interrupt handler of the
        // emulator's) changed.
        for w in &mut self.watchpoints {
            let now = Watch::read(cpu, w.phys, w.len);
            if now != w.value {
                self.watch_hit = Some((w.phys, w.len, w.value, now));
                w.value = now;
                self.enter_pause(PauseReason::Watchpoint);
                return true;
            }
        }
        if cpu.exceptions != self.seen_exceptions {
            self.seen_exceptions = cpu.exceptions;
            let hit = cpu.exception_log.back().is_some_and(|e| e.vector < 32 && self.break_exceptions & (1 << e.vector) != 0);
            if hit {
                self.enter_pause(PauseReason::Exception);
                return true;
            }
        }
        if cpu.mode_switches != self.seen_mode_switches {
            self.seen_mode_switches = cpu.mode_switches;
            if self.break_mode_switch {
                self.enter_pause(PauseReason::ModeSwitch);
                return true;
            }
        }
        if cpu.programs.ended != self.seen_ends {
            self.seen_ends = cpu.programs.ended;
            if self.break_program_exit {
                self.enter_pause(PauseReason::ProgramExit);
                return true;
            }
        }
        // Before the start the run's line made, which no instruction
        // separates from it.
        self.note_run_line(cpu);
        // A program just started: run to its entry point, through any
        // interrupt that comes first.
        if cpu.programs.started != self.seen_starts {
            self.seen_starts = cpu.programs.started;
            // This start uses up `run`'s one stop, even when every start
            // stops anyway.
            let once = std::mem::take(&mut self.start_once);
            if self.break_program_start || once {
                self.start_breakpoint = Some(cpu.programs.entry);
            }
        }
        if self.start_breakpoint == Some(phys_ip) {
            self.start_breakpoint = None;
            self.enter_pause(PauseReason::ProgramStart);
            return true;
        }
        if !self.skip_bp_once
            && (self.temp_breakpoint == Some(phys_ip) || self.breakpoints.contains(&phys_ip))
        {
            let target = self.temp_breakpoint.take() == Some(phys_ip);
            let user = self.breakpoints.contains(&phys_ip);
            let once = user && self.once.remove(&phys_ip);
            if once {
                self.breakpoints.remove(&phys_ip);
            }
            self.breakpoint_hit = Some(BreakpointHit { phys: phys_ip, user, once, target });
            self.enter_pause(PauseReason::Breakpoint);
            return true;
        }
        self.skip_bp_once = false;

        if let Some(n) = self.step_budget {
            if n == 0 {
                self.step_budget = None;
                self.enter_pause(PauseReason::Step);
                return true;
            }
            self.step_budget = Some(n - 1);
        }

        if self.tracing_now {
            let mut bytes = [0u8; 15];
            let end = (phys_ip + 15).min(ram.len());
            let len = end.saturating_sub(phys_ip);
            bytes[..len].copy_from_slice(&ram[phys_ip..end]);
            self.trace.push(TraceEntry {
                t_us: self.batch_t_us,
                icount: cpu.executed,
                cs: cpu.cs(),
                eip: cpu.eip(),
                gpr: [
                    cpu.eax(),
                    cpu.ecx(),
                    cpu.edx(),
                    cpu.ebx(),
                    cpu.esp(),
                    cpu.ebp(),
                    cpu.esi(),
                    cpu.edi(),
                ],
                ds: cpu.ds(),
                es: cpu.es(),
                ss: cpu.ss(),
                fs: cpu.fs(),
                gs: cpu.gs(),
                eflags: cpu.get_cpu_flags().bits(),
                code32: cpu.seg_cache(crate::cpu::Seg::CS).attr & crate::cpu::ATTR_DB != 0,
                pm: cpu.pm(),
                bytes,
                len: len as u8,
            });
            if self.trace_until.is_some_and(|n| self.trace.total() >= n) {
                self.trace_until = None;
                self.trace_enabled = false;
                self.tracing_now = self.shared.as_ref().is_some_and(|s| s.trace.receiver_count() > 0);
            }
        }
        false
    }

    /// Call after the execution batch: delivers pause notifications and
    /// streams new trace entries.
    pub fn end_batch(&mut self, cpu: &Cpu) {
        // Programs that started or ended in the batch, then its stop.
        if self.shared.is_some() {
            self.tell_programs(cpu);
        }
        self.flush_stop(cpu);

        if let Some(shared) = &self.shared {
            if shared.trace.receiver_count() > 0 {
                let (entries, dropped) = self.trace.since(self.trace_stream_cursor, self.trace_stream_max);
                self.trace_stream_cursor = self.trace.total();
                if !entries.is_empty() || dropped > 0 {
                    let msg = json!({
                        "type": "trace",
                        "dropped": dropped,
                        "entries": entries.iter().map(|e| e.to_json()).collect::<Vec<_>>(),
                    });
                    let _ = shared.trace.send(Arc::from(msg.to_string()));
                }
            } else {
                self.trace_stream_cursor = self.trace.total();
            }
        }
    }

    /// Account for one execution batch: `executed` instructions (not counting
    /// time skipped while halted) took `exec_time` of host time. `cache_hits`
    /// and `cache_misses` are the decode cache's running totals.
    pub fn record_batch(&mut self, executed: u64, exec_time: Duration, cache_hits: u64, cache_misses: u64) {
        let st = &mut self.stats;
        st.executed += executed;
        st.exec_time += exec_time;
        st.total_executed += executed;
        let wall = st.window_start.elapsed();
        if wall >= Duration::from_secs(1) {
            let exec_secs = st.exec_time.as_secs_f64();
            st.mips = if exec_secs > 0.0 { st.executed as f64 / exec_secs / 1e6 } else { 0.0 };
            st.emulated_mips = st.executed as f64 / wall.as_secs_f64() / 1e6;
            // A loaded save state starts the decode cache's counts again.
            let hits = cache_hits.checked_sub(st.cache_mark.0).unwrap_or(cache_hits);
            let misses = cache_misses.checked_sub(st.cache_mark.1).unwrap_or(cache_misses);
            st.cache_hit_rate = if hits + misses > 0 { hits as f64 / (hits + misses) as f64 } else { 0.0 };
            st.cache_mark = (cache_hits, cache_misses);
            st.executed = 0;
            st.exec_time = Duration::ZERO;
            st.window_start = Instant::now();
        }
    }

    pub(super) fn stats_json(&self, cpu: &Cpu) -> Value {
        fn dynrec_exits(d: &rust_dos::dynrec::DynStats) -> Value {
            const NAMES: [&str; 16] = [
                "", "fault", "smc", "panic", "next", "deadline", "stale", "gp0", "limit", "unlinked", "de", "watched",
                "after", "next_page", "env", "far_env",
            ];
            let mut map = serde_json::Map::new();
            for (k, name) in NAMES.iter().enumerate().skip(1) {
                if d.exits[k] != 0 {
                    map.insert(name.to_string(), json!(d.exits[k]));
                }
            }
            Value::Object(map)
        }
        fn dynrec_calls(calls: Option<&rust_dos::dynrec::Calls>) -> Value {
            let Some(c) = calls else { return Value::Null };
            let mut top: Vec<(usize, u64)> = c.fallback.iter().copied().enumerate().filter(|&(_, n)| n != 0).collect();
            top.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
            let names: Vec<_> = iced_x86::Mnemonic::values().collect();
            let fallback: Vec<Value> =
                top.iter().take(30).map(|&(m, n)| json!([format!("{:?}", names[m]), n])).collect();
            json!({
                "fallback": c.fallback.iter().sum::<u64>(),
                "fallback_top": fallback,
                "revalidate": c.revalidate,
                "memref": c.memref,
                "slow": c.slow,
                "dev": c.dev,
                "port": c.port,
                "load_seg": c.load_seg,
                "fpu": c.fpu,
            })
        }
        let st = &self.stats;
        let d = cpu.dynrec.stats();
        json!({
            // The `core` setting, and whether the dynamic recompiler runs
            // the instructions now.
            "core": cpu.core.name(),
            "recompiler": cpu.dynamic_active(),
            // The recompiler's counts since start: blocks translated and
            // their instructions (native: into host code; the others call
            // their interpreter handlers), blocks translated now, their
            // host code and the links between them, flushes of all of it,
            // blocks entered from the execution loop and the instructions
            // the translated code ran, and blocks that stopped at once for
            // the timer deadline, or because their code had changed
            // (stale) or changed under them (smc).
            "dynrec": {
                "blocks": d.blocks,
                "instructions": d.instructions,
                "native": d.native,
                "live_blocks": d.live_blocks,
                "code_bytes": d.code_bytes,
                "links": d.links,
                "flushes": d.flushes,
                "runs": d.runs,
                "executed": d.executed,
                "deadline": d.deadline,
                "stale": d.stale,
                "smc": d.smc,
                "watched": d.watched,
                // Exits of translated code by their kind, and the calls it
                // made into Rust, with the instructions that went through
                // their handlers most (for profiling).
                "exits": dynrec_exits(&d),
                "return_misses": d.return_misses,
                "calls": dynrec_calls(cpu.dynrec.calls()),
            },
            // Busy-wait loops proven to change nothing, proofs that failed,
            // and the instructions skipped (`idle`).
            "idle_skip": {
                "proofs": cpu.bus.observe.stats.proofs,
                "failures": cpu.bus.observe.stats.failures,
                "skipped": cpu.bus.observe.stats.skipped,
                "failure": cpu.bus.observe.stats.failure,
            },
            // Host speed while executing guest code, excluding idle skips,
            // rendering and frame pacing.
            "mips": (st.mips * 100.0).round() / 100.0,
            // Guest instructions per wall-clock second, as the program sees it.
            "emulated_mips": (st.emulated_mips * 100.0).round() / 100.0,
            "decode_cache_hit_rate": (st.cache_hit_rate * 10000.0).round() / 10000.0,
            "instructions": st.total_executed,
            "fps": (self.fps * 10.0).round() / 10.0,
            // The Stats page's numbers over the last quarter second, and
            // the emulated speed, which the page doesn't show: the frames
            // the program drew a second, the display's refresh rate,
            // emulated seconds a wall second, and in percent the
            // wall time the emulator's main thread worked, the emulated
            // time the CPU sat halted and the instructions the
            // recompiler's code ran; and ms a frame drawing the picture.
            // Null before the first frame.
            "frame": self.frame_stats.as_ref().map(|v| {
                let round = |x: f32, places: f64| (x as f64 * places).round() / places;
                json!({
                    "fps": round(v.fps, 10.0),
                    "refresh_hz": round(v.refresh_hz, 10.0),
                    "emulated_speed": round(v.emulated_speed, 100.0),
                    "cpu_use": round(v.cpu_use, 10.0),
                    "halted": round(v.halted, 10.0),
                    "recompiled": round(v.recompiled, 10.0),
                    "render_ms": round(v.render_ms, 100.0),
                })
            }),
        })
    }

    /// Keep the Stats page's numbers for `/api/stats`.
    pub fn set_frame_stats(&mut self, view: &rust_dos::stats::StatsView) {
        if self.shared.is_some() {
            self.frame_stats = Some(view.clone());
        }
    }

    /// Count a frame of the window's for `fps`, shown anew or not.
    pub fn count_frame(&mut self) {
        self.frames += 1;
        let (mark_t, mark_n) = self.fps_mark;
        let dt = mark_t.elapsed().as_secs_f64();
        if dt >= 1.0 {
            self.fps = (self.frames - mark_n) as f64 / dt;
            self.fps_mark = (Instant::now(), self.frames);
        }
    }

    /// Whether `capture_frame` wants to see the composited frame this frame.
    pub fn wants_frame(&self) -> bool {
        !self.frame_waiters.is_empty()
            || self.shared.as_ref().is_some_and(|s| s.screen_subscribers.load(Ordering::Relaxed) > 0)
    }

    /// Hand the composited frame (cached VGA render + cursor overlays) to
    /// screenshot requests and the screen stream.
    /// Watch the machine's writes while a heatmap stream is open, and
    /// publish what they were.
    fn update_heat(&mut self, cpu: &mut Cpu) {
        let Some(shared) = &self.shared else { return };
        let wanted = shared.heat_subscribers.load(Ordering::Relaxed) > 0;
        match (&mut self.heat, wanted) {
            (None, true) => self.heat = Some(heat::Heat::start(&mut cpu.bus)),
            (Some(_), false) => {
                heat::Heat::stop(&mut cpu.bus);
                self.heat = None;
            }
            (Some(heat), true) => {
                heat.sample(&cpu.bus);
                if let Ok(mut message) = shared.heat.lock() {
                    *message = Arc::from(heat.message(&cpu.bus));
                    shared.heat_seq.fetch_add(1, Ordering::Release);
                }
                if let Ok(mut ranges) = shared.heat_ranges.lock() {
                    *ranges = heat.hot_ranges();
                }
            }
            (None, false) => {}
        }
    }

    pub fn capture_frame(&mut self, screen: &video::Frame) {
        self.count_frame();
        if !self.wants_frame() {
            return;
        }
        for (ctx, w) in std::mem::take(&mut self.frame_waiters) {
            if let Some(policy) = &ctx.capture {
                if w.is_closed() {
                    continue;
                }
                if Instant::now() >= policy.deadline {
                    self.answer(
                        &ctx,
                        w,
                        Reply::Error(408, "the capture deadline expired before copying".into()),
                    );
                    continue;
                }
                let rgb_size = (screen.width as usize)
                    .checked_mul(screen.height as usize)
                    .and_then(|pixels| pixels.checked_mul(3));
                if screen.width == 0
                    || screen.height == 0
                    || screen.width > 2048
                    || screen.height > 2048
                    || rgb_size
                        .is_none_or(|size| size > 12 * 1024 * 1024 || size != screen.rgb.len())
                {
                    self.answer(
                        &ctx,
                        w,
                        Reply::Error(413, "the native capture frame exceeds its bounds".into()),
                    );
                    continue;
                }
            }
            let meta = self.meta(&ctx);
            let _ = w.send(Answer { reply: Reply::Frame(screen.clone()), meta });
        }
        if let Some(shared) = &self.shared
            && shared.screen_subscribers.load(Ordering::Relaxed) > 0
            && let Ok(mut frame) = shared.frame.lock()
            && *frame != *screen
        {
            frame.clone_from(screen);
            shared.frame_seq.fetch_add(1, Ordering::Release);
        }
    }

    /// Once the shell took a `run`'s command line: the programs started
    /// from then on are its own, and the first of them stops with
    /// `stop_at_entry`. `seen_starts` counts the starts before it: the
    /// per-instruction hook runs while the shell takes batch lines, and
    /// `begin_batch` catches up with the starts it didn't see.
    fn note_run_line(&mut self, cpu: &Cpu) {
        if let Some(run) = self.run_wait.as_mut().filter(|r| r.before.is_none())
            && cpu.batch.reached == Some(run.op)
        {
            run.before = Some(self.seen_starts);
            self.start_once = run.stop_at_entry;
        }
    }

    /// End a `run` operation whose program started, or whose command line
    /// ended at the prompt without starting one. A `run` goes on when its
    /// client stops waiting: how it ends is in the journal.
    fn check_run(&mut self, cpu: &Cpu) {
        // The shell may have taken the line after the batch's last
        // instruction.
        self.note_run_line(cpu);
        let idle = at_idle_prompt(cpu);
        if idle {
            // No program runs, so drop an entry point it never reached
            // (it was closed first).
            self.start_breakpoint = None;
        }
        let Some(run) = &self.run_wait else { return };
        let (op, before, stop_at_entry) = (run.op, run.before, run.stop_at_entry);
        let Some(before) = before else {
            // The prompt is back and the shell never took the line: the
            // batch lines were ended with it (Ctrl+C, the settings window
            // closing the program; `reboot_shell` supersedes the run first).
            if idle {
                self.start_once = false;
                self.run_wait = None;
                self.fail_op(cpu, op, 422, "the command line was dropped with the batch lines before it ran");
            }
            return;
        };
        if cpu.programs.started > before {
            if !stop_at_entry {
                // A start the journal didn't tell (a load brought it): the
                // run is done with the last start told.
                self.run_wait = None;
                let end = self.last_start_seq;
                self.with_journal(|j| j.end_op(op, OpState::Done, end));
                self.wake();
                if self.current_op == Some(op) {
                    self.current_op = None;
                }
            } else if idle {
                self.run_wait = None;
                self.fail_op(cpu, op, 409, "the program ended before it reached its entry point");
            }
        } else if idle {
            self.start_once = false;
            self.run_wait = None;
            self.fail_op(cpu, op, 422, "the command line started no program");
        }
    }

    /// End operation `op` as failed with `status`, appending `op_failed`.
    /// An operation that already ended (a `run` another control request
    /// superseded, or a pause stopped) is left as it is.
    fn fail_op(&mut self, cpu: &Cpu, op: u64, status: u16, error: &str) {
        let pending = self.with_journal(|j| matches!(j.op(op), journal::OpLookup::Found(o) if o.state == OpState::Pending));
        if pending == Some(true) {
            let message = error.to_string();
            let fields = json!({"status": status, "error": error});
            self.append_then(cpu, Kind::OpFailed, Some(op), fields, move |j, seq| j.fail_op(op, Some(seq), status, message));
        }
        if self.current_op == Some(op) {
            self.current_op = None;
        }
    }

    pub(super) fn enter_pause(&mut self, reason: PauseReason) {
        self.paused = true;
        self.step_budget = None;
        self.pause_hit = Some(reason);
    }

    pub(super) fn resume(&mut self) {
        if self.paused {
            self.paused = false;
            self.skip_bp_once = true;
        }
    }

    /// Deterministic mode with a client: the machine waits for it before
    /// running anything.
    pub fn pause_at_start(&mut self, cpu: &Cpu) {
        self.enter_pause(PauseReason::Startup);
        self.flush_stop(cpu);
    }

    /// Pause if emulated time reached a resume's `until_ms`. The
    /// deterministic front end asks at every emulated millisecond.
    pub fn time_stop(&mut self, cpu: &Cpu) -> bool {
        if self.stop_at_ns.is_some_and(|at| cpu.bus.clock.now_ns() >= at) {
            self.stop_at_ns = None;
            self.enter_pause(PauseReason::Time);
            return true;
        }
        false
    }
}

impl crate::exec::ExecHook for DebugHub {
    #[inline]
    fn before_exec(&mut self, cpu: &Cpu, phys_ip: usize, ram: &[u8]) -> bool {
        self.check_before_exec(cpu, phys_ip, ram)
    }

    /// A program that ended stops the machine before the shell goes on to
    /// the next line of its batch file (another program, or EXIT).
    fn before_shell(&mut self, cpu: &Cpu) -> bool {
        if cpu.programs.ended != self.seen_ends {
            self.seen_ends = cpu.programs.ended;
            if self.break_program_exit {
                self.enter_pause(PauseReason::ProgramExit);
                return true;
            }
        }
        false
    }
}
