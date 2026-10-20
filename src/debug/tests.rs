use super::dispatch::step_over_len;
use super::journal::{OpLookup, OpState};
use super::{Answer, Cmd, DebugHub, InputEvent, LowInput, Reply, StateRequest, TraceQuery, keys_for_char};
use super::trace::TraceEntry;
use rust_dos::deterministic::Deterministic;
use crate::cpu::{Cpu, CpuState};
use crate::exec::{self, StopReason};
use rust_dos::keylayout::Layout;
use serde_json::{Value, json};
use tokio::sync::oneshot;

fn mouse_machine(mode: u16, max_x: u16, max_y: u16) -> (Cpu, DebugHub) {
    let mut cpu = Cpu::new(".".into());
    cpu.set_ax(mode);
    rust_dos::interrupts::int10::handle(&mut cpu);
    cpu.set_ax(0);
    rust_dos::interrupts::int33::handle(&mut cpu);
    for (axis, max) in [(7, max_x), (8, max_y)] {
        cpu.set_ax(axis);
        cpu.set_cx(0);
        cpu.set_dx(max);
        rust_dos::interrupts::int33::handle(&mut cpu);
    }
    let hub = DebugHub::for_tests(&cpu);
    (cpu, hub)
}

fn mouse_input(cpu: &mut Cpu, hub: &mut DebugHub, event: Value) -> (u16, u16, u16) {
    let event = serde_json::from_value(event).unwrap();
    reply(cpu, hub, Cmd::Input { events: vec![event], wait: false }).unwrap();
    hub.process_input(cpu);
    cpu.set_ax(3);
    rust_dos::interrupts::int33::handle(cpu);
    (cpu.bx(), cpu.cx(), cpu.dx())
}

#[test]
fn screen_mouse_uses_large_absolute_ranges_for_incremental_dragging() {
    // SEAL's polling driver uses eight coordinate units per picture pixel.
    let (mut cpu, mut hub) = mouse_machine(0x12, 5112, 3832);
    assert_eq!(crate::video::frame_size(&cpu.bus), (640, 480));
    let event = |action, x, y| json!({"type":"mouse", "action":action, "x":x, "y":y, "button":"left", "coords":"screen"});
    assert_eq!(mouse_input(&mut cpu, &mut hub, event("down", 260, 74)), (1, 2080, 592));
    assert_eq!(mouse_input(&mut cpu, &mut hub, event("move", 320, 180)), (1, 2560, 1440));
    assert_eq!(mouse_input(&mut cpu, &mut hub, event("up", 320, 180)), (0, 2560, 1440));
}

#[test]
fn screen_mouse_has_no_large_range_cutoff_and_clamps_picture_coordinates() {
    for (max, center, last) in [(2047, 1024, 2044), (2048, 1025, 2048), (32767, 16409, 32767)] {
        let (mut cpu, mut hub) = mouse_machine(0x12, max, 479);
        let event = |x, y| json!({"type":"mouse", "action":"move", "x":x, "y":y, "coords":"screen"});
        assert_eq!(mouse_input(&mut cpu, &mut hub, event(320, 240)), (0, center, 240), "range {max}");
        assert_eq!(mouse_input(&mut cpu, &mut hub, event(-100, -100)), (0, 0, 0));
        assert_eq!(mouse_input(&mut cpu, &mut hub, event(i32::MAX, i32::MAX)), (0, last, 479), "range {max}");
    }
}

#[test]
fn screen_mouse_keeps_default_low_resolution_and_virtual_coordinates() {
    let (mut cpu, mut hub) = mouse_machine(0x13, 639, 199);
    assert_eq!(crate::video::frame_size(&cpu.bus), (640, 400));
    let screen = json!({"type":"mouse", "action":"move", "x":80, "y":80, "coords":"screen"});
    assert_eq!(mouse_input(&mut cpu, &mut hub, screen), (0, 80, 40));
    let virtual_position = json!({"type":"mouse", "action":"move", "x":123, "y":99, "coords":"virtual"});
    assert_eq!(mouse_input(&mut cpu, &mut hub, virtual_position), (0, 123, 99));
    let relative = json!({"type":"mouse", "action":"move", "dx":3, "dy":-4});
    assert_eq!(mouse_input(&mut cpu, &mut hub, relative), (0, 126, 95));
}

fn handle(cpu: &mut Cpu, hub: &mut DebugHub, cmd: Cmd) -> Reply {
    let (request, mut rx) = hub.request(cmd);
    hub.handle(cpu, request);
    rx.try_recv().expect("an answer at once").reply
}

/// The journal's entries, as JSON.
fn entries(hub: &DebugHub) -> Vec<Value> {
    hub.with_journal(|j| j.after(0).map(|it| it.map(|e| e.value()).collect()).unwrap_or_default()).unwrap_or_default()
}

/// The journal's entries of type `kind`.
fn entries_of(hub: &DebugHub, kind: &str) -> Vec<Value> {
    entries(hub).into_iter().filter(|e| e["type"] == kind).collect()
}

/// A trace page from cursor `since`: its instruction counts, the next
/// cursor and the number dropped.
fn page(cpu: &mut Cpu, hub: &mut DebugHub, since: u64, limit: usize) -> (Vec<u64>, u64, u64) {
    let q = TraceQuery { since: Some(since), limit: Some(limit), ..Default::default() };
    match handle(cpu, hub, Cmd::TraceQuery(q)) {
        Reply::Trace { entries, next, dropped, .. } => (entries.iter().map(|e| e.icount).collect(), next, dropped),
        _ => panic!("not a trace"),
    }
}

#[test]
fn a_trace_with_a_count_records_that_many_and_stops() {
    let mut cpu = Cpu::new(".".into());
    cpu.load_shell();
    let mut hub = DebugHub::new(None, None, 1000);
    handle(&mut cpu, &mut hub, Cmd::TraceControl { enabled: None, clear: false, stream_max: None, count: Some(50) });
    // The shell idles at its prompt waiting for a key, running only a
    // few instructions a batch.
    for _ in 0..100 {
        let hot = hub.begin_batch(&cpu);
        cpu.bus.start_batch(cpu.bus.clock.icount + 2000);
        crate::exec::run_batch(&mut cpu, &mut hub, hot);
        hub.end_batch(&cpu);
    }
    assert_eq!(hub.trace.total(), 50);
    assert!(!hub.trace_enabled && hub.trace_until.is_none());
    // Paged through: two pages of 30 and 20, then nothing new.
    let (first, next, dropped) = page(&mut cpu, &mut hub, 0, 30);
    assert_eq!((first.len(), next, dropped), (30, 30, 0));
    let (second, next, _) = page(&mut cpu, &mut hub, next, 30);
    assert_eq!((second.len(), next), (20, 50));
    assert!(first.last() < second.first(), "oldest first, in order");
    assert_eq!(page(&mut cpu, &mut hub, next, 30), (vec![], 50, 0));
}

#[test]
fn last_n_gets_that_many_entries() {
    let mut cpu = Cpu::new(".".into());
    let mut hub = DebugHub::new(None, None, 30_000);
    for icount in 1..=25_000 {
        hub.trace.push(TraceEntry { icount, ..Default::default() });
    }
    let mut last = |last_n, limit| {
        let q = TraceQuery { last_n: Some(last_n), limit, ..Default::default() };
        match handle(&mut cpu, &mut hub, Cmd::TraceQuery(q)) {
            Reply::Trace { entries, truncated, .. } => (entries.len(), entries.first().map(|e| e.icount), truncated),
            _ => panic!("not a trace"),
        }
    };
    assert_eq!(last(20_000, None), (20_000, Some(5_001), false));
    assert_eq!(last(20_000, Some(100)), (100, Some(24_901), true));
    assert_eq!(last(50, None), (50, Some(24_951), false));
    // More than the ring holds: all of it, and nothing left out.
    assert_eq!(last(30_000, None), (25_000, Some(1), false));
    // Without `last_n`, and in a page read with `since`, `limit` is
    // 10000 unless given, and the reply says it left entries out.
    let mut count = |q| match handle(&mut cpu, &mut hub, Cmd::TraceQuery(q)) {
        Reply::Trace { entries, truncated, .. } => (entries.len(), truncated),
        _ => panic!("not a trace"),
    };
    assert_eq!(count(TraceQuery::default()), (10_000, true));
    assert_eq!(count(TraceQuery { since: Some(0), last_n: Some(20_000), ..Default::default() }), (10_000, true));
    assert_eq!(count(TraceQuery { since: Some(20_000), ..Default::default() }), (5_000, false));
}

#[test]
fn screen_text_has_each_rows_attributes_as_runs() {
    let mut cpu = Cpu::new(".".into());
    let mut hub = DebugHub::new(None, None, 1000);
    // Row 2: " Play  Quit", "Quit" highlighted (black on cyan).
    for (c, ch) in " Play  Quit".bytes().enumerate() {
        let at = 0xB8000 + 2 * 80 * 2 + c * 2;
        cpu.bus.write_8(at, ch);
        cpu.bus.write_8(at + 1, if c >= 7 { 0x30 } else { 0x07 });
    }
    for c in 11..80 {
        cpu.bus.write_8(0xB8000 + 2 * 80 * 2 + c * 2 + 1, 0x07);
    }
    let Reply::Json(v) = handle(&mut cpu, &mut hub, Cmd::ScreenText) else { panic!("not JSON") };
    assert_eq!(v["lines"][2], " Play  Quit");
    let runs: Vec<(u64, u64, &str)> = v["attrs"][2]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["col"].as_u64().unwrap(), r["len"].as_u64().unwrap(), r["attr"].as_str().unwrap()))
        .collect();
    assert_eq!(runs, [(0, 7, "07"), (7, 4, "30"), (11, 69, "07")]);
    assert_eq!(v["attrs"].as_array().unwrap().len(), v["rows"].as_u64().unwrap() as usize);
    assert!(v["blink"].is_boolean() && v["mono"] == false, "{v}");
}

#[test]
fn screen_text_gives_the_cursor_of_the_active_page() {
    let mut cpu = Cpu::new(".".into());
    let mut hub = DebugHub::new(None, None, 1000);
    cpu.bus.write_8(0x0450, 1);
    cpu.bus.write_8(0x0451, 2);
    cpu.bus.write_8(0x0452, 5);
    cpu.bus.write_8(0x0453, 3);
    cpu.bus.write_8(0x0462, 1);
    let Reply::Json(v) = handle(&mut cpu, &mut hub, Cmd::ScreenText) else { panic!("not JSON") };
    assert_eq!((v["cursor"]["col"].as_u64(), v["cursor"]["row"].as_u64()), (Some(5), Some(3)));
}

#[test]
fn a_trace_reply_shows_whether_it_records_now() {
    let mut cpu = Cpu::new(".".into());
    let mut hub = DebugHub::new(None, None, 1000);
    let mut control = |enabled, count| {
        let cmd = Cmd::TraceControl { enabled, clear: false, stream_max: None, count };
        match handle(&mut cpu, &mut hub, cmd) {
            Reply::Json(v) => v["active"].as_bool(),
            _ => panic!("not JSON"),
        }
    };
    assert_eq!(control(Some(true), None), Some(true));
    assert_eq!(control(None, Some(0)), Some(false));
    assert_eq!(control(None, Some(10)), Some(true));
    assert_eq!(control(Some(false), None), Some(false));
}

#[test]
fn a_count_shows_what_is_left_and_a_count_of_0_stops_the_trace() {
    let mut cpu = Cpu::new(".".into());
    let mut hub = DebugHub::new(None, None, 1000);
    let control = |count| Cmd::TraceControl { enabled: None, clear: false, stream_max: None, count: Some(count) };
    let Reply::Json(status) = handle(&mut cpu, &mut hub, control(50)) else { panic!("not json") };
    assert_eq!((status["enabled"].as_bool(), status["remaining"].as_u64()), (Some(true), Some(50)));
    let Reply::Json(status) = handle(&mut cpu, &mut hub, control(0)) else { panic!("not json") };
    assert_eq!(status["enabled"].as_bool(), Some(false));
    assert!(status["remaining"].is_null());
}

/// The reply to a command the hub answers at once.
fn reply(cpu: &mut Cpu, hub: &mut DebugHub, cmd: Cmd) -> Result<Value, (u16, String)> {
    match handle(cpu, hub, cmd) {
        Reply::Json(v) => Ok(v),
        Reply::Error(code, e) => Err((code, e)),
        Reply::ErrorJson(code, v) => Err((code, v.to_string())),
        _ => panic!("no JSON reply"),
    }
}

fn write(addr: &str, data: &[u8], expect: Option<&[u8]>) -> Cmd {
    Cmd::WriteMem { addr: addr.into(), data: data.to_vec(), expect: expect.map(<[u8]>::to_vec) }
}

#[test]
fn a_write_reports_the_bytes_it_replaced() {
    let (mut cpu, mut hub) = (Cpu::new(".".into()), DebugHub::disabled());
    cpu.bus.write_8(0x2000, 0xCD);
    cpu.bus.write_8(0x2001, 0xAB);
    let done = reply(&mut cpu, &mut hub, write("0200:0000", &[0x21, 0x43], None)).unwrap();
    assert_eq!((done["old"].as_str(), done["new"].as_str()), (Some("CD AB"), Some("21 43")));
}

#[test]
fn a_write_expecting_other_bytes_writes_nothing() {
    let (mut cpu, mut hub) = (Cpu::new(".".into()), DebugHub::disabled());
    cpu.bus.write_8(0x2000, 0xCD);
    cpu.bus.write_8(0x2001, 0xAB);
    let refused = reply(&mut cpu, &mut hub, write("2000", &[0x21, 0x43], Some(&[0x34, 0x12]))).unwrap_err();
    assert_eq!(refused.0, 409);
    assert!(refused.1.contains(r#""found":"CD AB""#), "{}", refused.1);
    assert!(refused.1.contains(r#""expected":"34 12""#), "{}", refused.1);
    assert_eq!((cpu.bus.peek_8(0x2000), cpu.bus.peek_8(0x2001)), (0xCD, 0xAB));
    let done = reply(&mut cpu, &mut hub, write("2000", &[0x21, 0x43], Some(&[0xCD, 0xAB]))).unwrap();
    assert_eq!(done["new"], "21 43");
    let short = reply(&mut cpu, &mut hub, write("2000", &[0x21, 0x43], Some(&[0x21]))).unwrap_err();
    assert_eq!(short.0, 400, "expect is as long as the data");
}

/// A .COM program: MOV AX,1234h; MOV BX,5678h; MOV AX,[0110h];
/// MOV AX,4C00h; INT 21h; and the word ABCDh at 0110h.
const PROBE: &str = "B83412BB7856A11001B8004CCD210000CDAB";

/// A .COM program that divides by zero: XOR AX,AX; DIV AL.
const CRASH: [u8; 4] = [0x31, 0xC0, 0xF6, 0xF0];

/// A machine at the prompt with PROBE.COM on C:, and a hub with no
/// server, which takes its commands from `send`.
fn machine(name: &str) -> (Cpu, DebugHub) {
    let dir = crate::test_dir::path(&format!("debug-{name}"));
    std::fs::create_dir_all(&dir).unwrap();
    let code: Vec<u8> = (0..PROBE.len()).step_by(2).map(|i| u8::from_str_radix(&PROBE[i..i + 2], 16).unwrap()).collect();
    std::fs::write(dir.join("PROBE.COM"), code).unwrap();
    std::fs::write(dir.join("CRASH.COM"), CRASH).unwrap();
    let mut cpu = Cpu::new(dir);
    cpu.load_shell();
    // Polled as the server's is; this one's commands come from `send`.
    let mut hub = DebugHub::for_tests(&cpu);
    run(&mut cpu, &mut hub, 20);
    assert!(cpu.shell_idle(), "the shell waits at its prompt");
    (cpu, hub)
}

/// Run `frames` frames of a few thousand instructions, as the main loop
/// does: the hub's requests first, nothing while it is paused.
fn run(cpu: &mut Cpu, hub: &mut DebugHub, frames: usize) {
    for _ in 0..frames {
        hub.poll(cpu);
        if hub.paused {
            continue;
        }
        let hot = hub.begin_batch(cpu);
        let end = cpu.bus.clock.icount + 5000;
        cpu.bus.start_batch(end);
        while cpu.bus.clock.icount < end {
            match exec::run_batch(cpu, hub, hot) {
                StopReason::BatchEnd | StopReason::Paused | StopReason::Exit | StopReason::Hook => break,
                StopReason::ShellReloaded => cpu.bus.start_batch(end),
            }
        }
        hub.end_batch(cpu);
    }
}

/// A request sent to the hub: what the server waits for after the
/// hub's answer, and the answer.
struct Sent {
    wait: Wait,
    rx: oneshot::Receiver<Answer>,
}

enum Wait {
    /// Nothing: the answer is the reply.
    No,
    /// The end of the operation the request started.
    Op,
    /// The stop the machine is in, or the next.
    Stop,
}

fn send(cpu: &mut Cpu, hub: &mut DebugHub, cmd: Cmd) -> Sent {
    let wait = match cmd {
        Cmd::Step { .. } | Cmd::StepOver | Cmd::Run { .. } => Wait::Op,
        Cmd::WaitPause => Wait::Stop,
        _ => Wait::No,
    };
    let (request, rx) = hub.request(cmd);
    hub.handle(cpu, request);
    Sent { wait, rx }
}

/// How operation `op` ended, as the server tells it: the stop, a run's
/// program, or its failure. None while it is pending.
fn op_reply(hub: &DebugHub, op: u64) -> Option<Result<Value, (u16, String)>> {
    hub.with_journal(|j| {
        let OpLookup::Found(found) = j.op(op) else { panic!("no operation {}", op) };
        let end = found.end_seq.and_then(|s| j.get(s)).map(|e| e.value()).unwrap_or(Value::Null);
        match found.state {
            OpState::Pending => None,
            OpState::Stopped => Some(Ok(end)),
            OpState::Done => Some(Ok(serde_json::json!({"ok": true, "program": end["program"]}))),
            OpState::Failed => Some(Err(found.error.clone().unwrap())),
            OpState::Cancelled => Some(Err((409, end.to_string()))),
            OpState::Superseded => Some(Err((409, "superseded".into()))),
        }
    })
    .unwrap()
}

/// The first `paused` entry after `after`.
fn stop_after(hub: &DebugHub, after: u64) -> Option<Value> {
    entries(hub).into_iter().find(|e| e["type"] == "paused" && e["seq"].as_u64() > Some(after))
}

/// The reply to a command, after running up to `frames` frames for it
/// and what the server waits for after the hub's answer.
fn answer(cpu: &mut Cpu, hub: &mut DebugHub, mut sent: Sent, frames: usize) -> Result<Value, (u16, String)> {
    let mut frame = 0;
    let answer = loop {
        match sent.rx.try_recv() {
            Ok(answer) => break answer,
            Err(_) if frame < frames => run(cpu, hub, 1),
            Err(_) => panic!("no answer in {} frames", frames),
        }
        frame += 1;
    };
    let value = match answer.reply {
        Reply::Json(v) => v,
        Reply::Error(code, e) => return Err((code, e)),
        Reply::ErrorJson(code, v) => return Err((code, v.to_string())),
        _ => panic!("not a JSON reply"),
    };
    loop {
        let done = match sent.wait {
            Wait::No => Some(Ok(value.clone())),
            Wait::Op => op_reply(hub, answer.meta.op.expect("an operation")),
            Wait::Stop if value["paused"] == true => Some(Ok(value.clone())),
            Wait::Stop => stop_after(hub, answer.meta.last).map(Ok),
        };
        match done {
            Some(done) => return done,
            None if frame < frames => run(cpu, hub, 1),
            None => panic!("no end in {} frames", frames),
        }
        frame += 1;
    }
}

fn ask(cpu: &mut Cpu, hub: &mut DebugHub, cmd: Cmd, frames: usize) -> Result<Value, (u16, String)> {
    let rx = send(cpu, hub, cmd);
    answer(cpu, hub, rx, frames)
}

fn break_on(program_start: Option<bool>, program_exit: Option<bool>) -> Cmd {
    Cmd::BreakOn { exceptions: None, clear_exceptions: None, mode_switch: None, program_start, program_exit }
}

#[test]
fn run_stops_at_the_program_s_entry_point() {
    let (mut cpu, mut hub) = machine("entry");
    let rx = send(&mut cpu, &mut hub, Cmd::Run { command: "PROBE.COM".into(), stop_at_entry: true, queue: false });
    let stop = answer(&mut cpu, &mut hub, rx, 50).unwrap();
    assert_eq!(stop["reason"], "program_start");
    assert_eq!(stop["program"]["name"], "PROBE.COM");
    assert_eq!(stop["registers"]["ip"], "0100");
    assert_eq!(stop["program"]["entry"], format!("{}:0100", stop["registers"]["cs"].as_str().unwrap()));
    assert!(hub.paused);
    // Nothing of the program ran: AX is what DOS starts it with.
    assert_eq!(cpu.ax(), 0);
}

#[test]
fn a_program_s_end_stops_the_machine_with_its_exit_code() {
    let (mut cpu, mut hub) = machine("exit");
    ask(&mut cpu, &mut hub, break_on(None, Some(true)), 0).unwrap();
    ask(&mut cpu, &mut hub, Cmd::Run { command: "PROBE.COM".into(), stop_at_entry: true, queue: false }, 50).unwrap();
    ask(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None }, 0).unwrap();
    let stop = ask(&mut cpu, &mut hub, Cmd::WaitPause, 50).unwrap();
    assert_eq!(stop["reason"], "program_exit");
    assert!(hub.paused);
    assert_eq!(stop["exit"], serde_json::json!({"name": "PROBE.COM", "code": 0, "resident": false, "aborted": false}));
}

#[test]
fn a_program_the_emulator_ends_stops_the_machine_with_no_exit_code() {
    let (mut cpu, mut hub) = machine("abort");
    ask(&mut cpu, &mut hub, break_on(None, Some(true)), 0).unwrap();
    ask(&mut cpu, &mut hub, Cmd::Run { command: "CRASH.COM".into(), stop_at_entry: true, queue: false }, 50).unwrap();
    ask(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None }, 0).unwrap();
    let stop = ask(&mut cpu, &mut hub, Cmd::WaitPause, 50).unwrap();
    assert_eq!(stop["reason"], "program_exit");
    assert_eq!(stop["exit"], serde_json::json!({"name": "CRASH.COM", "code": null, "resident": false, "aborted": true}));
    // The next program's own exit is told apart again.
    ask(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None }, 0).unwrap();
    run(&mut cpu, &mut hub, 20);
    ask(&mut cpu, &mut hub, Cmd::Run { command: "PROBE.COM".into(), stop_at_entry: true, queue: false }, 50).unwrap();
    ask(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None }, 0).unwrap();
    let stop = ask(&mut cpu, &mut hub, Cmd::WaitPause, 50).unwrap();
    assert_eq!(stop["exit"]["aborted"], false);
}

#[test]
fn a_batch_file_s_program_stops_at_its_end_before_the_next_line() {
    let (mut cpu, mut hub) = machine("batch");
    ask(&mut cpu, &mut hub, break_on(None, Some(true)), 0).unwrap();
    // The shell runs the next line right after the first program ends,
    // with no instruction of the machine's in between.
    cpu.queue_batch_lines(["PROBE.COM", "CRASH.COM"]);
    let stop = ask(&mut cpu, &mut hub, Cmd::WaitPause, 50).unwrap();
    assert_eq!(stop["reason"], "program_exit");
    assert_eq!(stop["exit"]["name"], "PROBE.COM");
    assert!(cpu.shell_idle() && cpu.program.is_empty(), "back at the prompt, CRASH.COM not started");
}

#[test]
fn run_without_stopping_answers_once_the_program_started() {
    let (mut cpu, mut hub) = machine("started");
    let started = ask(&mut cpu, &mut hub, Cmd::Run { command: "PROBE.COM".into(), stop_at_entry: false, queue: false }, 50).unwrap();
    assert_eq!(started["program"]["name"], "PROBE.COM");
    assert!(!hub.paused);
    run(&mut cpu, &mut hub, 20);
    assert_eq!(super::status::exit_json(&cpu)["name"], "PROBE.COM", "the status shows how it ended");
}

#[test]
fn every_program_start_stops_while_asked_to() {
    let (mut cpu, mut hub) = machine("every");
    ask(&mut cpu, &mut hub, break_on(Some(true), None), 0).unwrap();
    cpu.queue_batch_lines(["PROBE.COM"]);
    let stop = ask(&mut cpu, &mut hub, Cmd::WaitPause, 50).unwrap();
    assert_eq!((stop["reason"].as_str(), stop["registers"]["ip"].as_str()), (Some("program_start"), Some("0100")));
}

#[test]
fn run_refuses_a_command_line_that_starts_no_program() {
    let (mut cpu, mut hub) = machine("none");
    let rx = send(&mut cpu, &mut hub, Cmd::Run { command: "NOSUCH.EXE".into(), stop_at_entry: true, queue: false });
    assert_eq!(answer(&mut cpu, &mut hub, rx, 50).unwrap_err().0, 422);
    assert!(!hub.paused && !hub.start_once, "no stop is left armed");
}

#[test]
fn run_waits_for_the_prompt() {
    let (mut cpu, mut hub) = machine("busy");
    let rx = send(&mut cpu, &mut hub, Cmd::Run { command: "PROBE.COM".into(), stop_at_entry: true, queue: false });
    answer(&mut cpu, &mut hub, rx, 50).unwrap();
    let rx = send(&mut cpu, &mut hub, Cmd::Run { command: "PROBE.COM".into(), stop_at_entry: true, queue: false });
    assert_eq!(answer(&mut cpu, &mut hub, rx, 0).unwrap_err().0, 409, "a program is running");
}

#[test]
fn a_queued_run_waits_for_the_batch_lines_before_it() {
    let (mut cpu, mut hub) = machine("queued");
    // A batch file runs, as AUTOEXEC.BAT does at startup.
    cpu.batch.append_file("AUTOEXEC", b"@echo off\r\nPROBE.COM\r\n", "");
    let refused = send(&mut cpu, &mut hub, Cmd::Run { command: "CRASH.COM".into(), stop_at_entry: true, queue: false });
    assert_eq!(answer(&mut cpu, &mut hub, refused, 0).unwrap_err().0, 409, "busy without queue");
    let rx = send(&mut cpu, &mut hub, Cmd::Run { command: "CRASH.COM".into(), stop_at_entry: true, queue: true });
    let stop = answer(&mut cpu, &mut hub, rx, 50).unwrap();
    // PROBE.COM's start didn't take the run's stop.
    assert_eq!((stop["reason"].as_str(), stop["program"]["name"].as_str()), (Some("program_start"), Some("CRASH.COM")));
    assert_eq!(cpu.programs.started, 2);
    let started: Vec<Value> = entries_of(&hub, "program_started").iter().map(|e| e["program"]["name"].clone()).collect();
    assert_eq!(started, ["PROBE.COM", "CRASH.COM"]);
    assert_eq!(entries_of(&hub, "resumed").last().unwrap()["queued"], true);
}

#[test]
fn a_queued_run_without_stopping_answers_with_its_own_program() {
    let (mut cpu, mut hub) = machine("queued-started");
    cpu.batch.append_file("AUTOEXEC", b"PROBE.COM\r\n", "");
    let rx = send(&mut cpu, &mut hub, Cmd::Run { command: "CRASH.COM".into(), stop_at_entry: false, queue: true });
    let started = answer(&mut cpu, &mut hub, rx, 50).unwrap();
    assert_eq!(started["program"]["name"], "CRASH.COM");
}

#[test]
fn a_queued_run_leaves_the_batch_file_s_program_without_the_hook() {
    let (mut cpu, mut hub) = machine("queued-hook");
    let dir = crate::test_dir::path("debug-queued-hook");
    // mov cx,FFFFh; loop $; mov ax,4C00h; int 21h: a few frames' work.
    std::fs::write(dir.join("SPIN.COM"), [0xB9, 0xFF, 0xFF, 0xE2, 0xFE, 0xB8, 0x00, 0x4C, 0xCD, 0x21]).unwrap();
    cpu.batch.append_file("AUTOEXEC", b"@echo off\r\nSPIN.COM\r\n", "");
    let mut sent = send(&mut cpu, &mut hub, Cmd::Run { command: "PROBE.COM".into(), stop_at_entry: true, queue: true });
    let op = sent.rx.try_recv().expect("an answer at once").meta.op.expect("an operation");
    let (mut cold, mut stop) = (0, None);
    for _ in 0..100 {
        if let Some(end) = op_reply(&hub, op) {
            stop = Some(end.unwrap());
            break;
        }
        if cpu.programs.running > 0 && !cpu.in_shell_code() && !hub.begin_batch(&cpu) {
            cold += 1;
        }
        run(&mut cpu, &mut hub, 1);
    }
    assert!(cold > 0, "SPIN.COM runs without the per-instruction hook");
    let stop = stop.expect("the run ended");
    assert_eq!((stop["reason"].as_str(), stop["program"]["name"].as_str()), (Some("program_start"), Some("PROBE.COM")));
    let started: Vec<Value> = entries_of(&hub, "program_started").iter().map(|e| e["program"]["name"].clone()).collect();
    assert_eq!(started, ["SPIN.COM", "PROBE.COM"]);
}

#[test]
fn a_queued_run_at_an_idle_prompt_runs_at_once() {
    let (mut cpu, mut hub) = machine("queued-idle");
    let rx = send(&mut cpu, &mut hub, Cmd::Run { command: "PROBE.COM".into(), stop_at_entry: true, queue: true });
    let stop = answer(&mut cpu, &mut hub, rx, 50).unwrap();
    assert_eq!(stop["program"]["name"], "PROBE.COM");
    assert_eq!(entries_of(&hub, "resumed").last().unwrap()["queued"], false);
}

#[test]
fn a_queued_run_dropped_with_its_batch_lines_fails() {
    let (mut cpu, mut hub) = machine("queued-dropped");
    cpu.batch.append_file("AUTOEXEC", b"PAUSE\r\n", "");
    let rx = send(&mut cpu, &mut hub, Cmd::Run { command: "PROBE.COM".into(), stop_at_entry: true, queue: true });
    run(&mut cpu, &mut hub, 20);
    assert!(cpu.shell_wait.is_some(), "PAUSE waits for a key");
    // Ctrl+C at PAUSE ends the batch files, the run's line with them.
    cpu.batch.clear();
    cpu.shell_wait = None;
    let failed = answer(&mut cpu, &mut hub, rx, 50).unwrap_err();
    assert_eq!(failed.0, 422, "{}", failed.1);
    assert!(hub.run_wait.is_none() && !hub.start_once);
}

#[test]
fn reboot_shell_drops_the_batch_file_too() {
    let (mut cpu, mut hub) = (Cpu::new(".".into()), DebugHub::disabled());
    cpu.load_shell();
    cpu.queue_batch_lines(["GAME.EXE", "EXIT"]);
    let reply = handle(&mut cpu, &mut hub, Cmd::RebootShell);
    assert!(matches!(reply, Reply::Json(_)));
    assert_eq!(cpu.state, CpuState::RebootShell);
    assert!(hub.take_closed_program(), "the front end releases its part");
    // Run the shell's reload and what would come after it: without the
    // fix, the batch's EXIT runs and asks to turn the machine off.
    let mut reloaded = false;
    for _ in 0..200 {
        let end = cpu.bus.clock.icount + 1000;
        cpu.bus.start_batch(end);
        match exec::run_batch(&mut cpu, &mut exec::NoHook, false) {
            StopReason::ShellReloaded => reloaded = true,
            StopReason::Exit => panic!("EXIT ran after the reload"),
            _ => {}
        }
    }
    assert!(reloaded, "the shell reloads");
    assert!(!cpu.bus.exit_requested);
    assert!(!cpu.batch.is_active(), "EXIT must not run after the reload");
}

/// Pause the machine, as the main loop does: its stop is told at the
/// end of the frame.
fn pause(cpu: &mut Cpu, hub: &mut DebugHub) {
    let rx = send(cpu, hub, Cmd::Pause);
    hub.end_batch(cpu);
    answer(cpu, hub, rx, 0).unwrap();
    assert!(hub.paused);
}

/// Ask to save a state, with strict_match or not, and the requests the
/// front end gets to carry out, with the reply of one refused.
fn state_request(cpu: &mut Cpu, hub: &mut DebugHub, strict_match: bool) -> (usize, Option<(u16, Value)>) {
    let mut sent = send(cpu, hub, Cmd::SaveState { path: "/tmp/x.state".into(), strict_match, check_files: false });
    let taken = take_state_requests(hub).len();
    let refused = match sent.rx.try_recv().map(|a| a.reply) {
        Ok(Reply::ErrorJson(code, body)) => Some((code, body)),
        Ok(_) => panic!("not a refusal"),
        Err(_) => None,
    };
    (taken, refused)
}

/// The save states the front end gets to save or load now.
fn take_state_requests(hub: &mut DebugHub) -> Vec<StateRequest> {
    std::iter::from_fn(|| {
        let request = hub.next_state_request();
        hub.finish();
        request
    })
    .collect()
}

#[test]
fn a_strict_match_save_or_load_is_done_only_while_the_machine_is_paused() {
    let (mut cpu, mut hub) = machine("strict-match");
    let (taken, refused) = state_request(&mut cpu, &mut hub, true);
    let (code, body) = refused.expect("refused while running");
    assert_eq!((taken, code), (0, 409));
    assert!(body["error"].as_str().unwrap().contains("pause it first"), "{}", body);

    pause(&mut cpu, &mut hub);
    let (taken, refused) = state_request(&mut cpu, &mut hub, true);
    assert_eq!((taken, refused.is_none()), (1, true), "paused with nothing pending");

    // A step sent with it, in the same frame, starts the machine first.
    let strict = Cmd::SaveState { path: "/tmp/x.state".into(), strict_match: true, check_files: false };
    let _save = send(&mut cpu, &mut hub, strict);
    let _step = send(&mut cpu, &mut hub, Cmd::Step { count: 1 });
    assert!(take_state_requests(&mut hub).is_empty(), "refused: the step is under way");

    // Queued input goes on being typed while paused: a save waits for
    // it, and a load drops it.
    let (mut cpu, mut hub) = machine("strict-match-input");
    pause(&mut cpu, &mut hub);
    hub.input.push_back(LowInput::Wait(std::time::Duration::from_secs(60)));
    let (taken, refused) = state_request(&mut cpu, &mut hub, true);
    let (code, body) = refused.expect("refused with input queued");
    assert_eq!((taken, code), (0, 409));
    assert!(body["error"].as_str().unwrap().contains("queued input"), "{}", body);
    let _load = send(&mut cpu, &mut hub, Cmd::LoadState { path: "/tmp/x.state".into(), strict_match: true, check_files: false, folders: Vec::new() });
    assert_eq!(take_state_requests(&mut hub).len(), 1, "a load drops the input");
}

#[test]
fn input_whose_last_wait_is_under_way_is_still_being_typed() {
    let (mut cpu, mut hub) = machine("strict-match-wait");
    pause(&mut cpu, &mut hub);
    let op = started_op(&mut cpu, &mut hub, Cmd::Input { events: vec![InputEvent::Wait { ms: 60_000 }], wait: false });
    // The wait starts: only the end of the request is left in the queue.
    run(&mut cpu, &mut hub, 3);
    assert_eq!(op_state(&hub, op).state, OpState::Pending);
    let (taken, refused) = state_request(&mut cpu, &mut hub, true);
    let (code, body) = refused.expect("refused while the input's operation is pending");
    assert_eq!((taken, code, body["input_queued"].as_u64()), (0, 409, Some(1)), "{}", body);
    let read = journal_state(&hub).0;
    let (refused, _) = ask_guarded(&mut cpu, &mut hub, Cmd::GetRegs, Some(read));
    assert_eq!(refused.unwrap_err().1["precondition"], "state");
}

#[test]
fn a_plain_save_or_load_is_done_running_or_not() {
    let (mut cpu, mut hub) = machine("no-strict-match");
    let (taken, refused) = state_request(&mut cpu, &mut hub, false);
    assert_eq!((taken, refused.is_none()), (1, true), "taken while running");

    // Sent with a strict-match save while running: only that save is refused.
    let mut strict = send(&mut cpu, &mut hub, Cmd::SaveState { path: "/tmp/a.state".into(), strict_match: true, check_files: false });
    let _plain = send(&mut cpu, &mut hub, Cmd::LoadState { path: "/tmp/b.state".into(), strict_match: false, check_files: false, folders: Vec::new() });
    let taken = take_state_requests(&mut hub);
    assert_eq!(taken.iter().map(|r| (r.load, r.strict_match)).collect::<Vec<_>>(), [(true, false)]);
    assert!(matches!(strict.rx.try_recv().map(|a| a.reply), Ok(Reply::ErrorJson(409, _))));
}

#[test]
fn a_loaded_state_leaves_no_breakpoint_watchpoint_or_stop_from_before() {
    let (mut cpu, mut hub) = machine("reset");
    pause(&mut cpu, &mut hub);
    ask(&mut cpu, &mut hub, Cmd::AddBreakpoint { addr: "0100:0000".into(), once: false }, 0).unwrap();
    ask(&mut cpu, &mut hub, Cmd::AddWatchpoint { addr: "0200:0000".into(), len: 2 }, 0).unwrap();
    let on = Cmd::BreakOn { exceptions: Some(1), clear_exceptions: None, mode_switch: Some(true), program_start: None, program_exit: Some(true) };
    ask(&mut cpu, &mut hub, on, 0).unwrap();
    hub.temp_breakpoint = Some(0x1234);
    let typed = Cmd::Input { events: vec![InputEvent::Wait { ms: 60_000 }], wait: true };
    let sent = send(&mut cpu, &mut hub, typed);
    answer(&mut cpu, &mut hub, sent, 0).unwrap();
    hub.key_down(&mut cpu, super::keys::lookup("a").unwrap(), b'a');

    let (cleared, cancelled) = hub.reset_after_load(&cpu);
    assert_eq!(
        cleared,
        serde_json::json!({
            "breakpoints": 1, "watchpoints": 1, "break_on": ["exceptions", "mode_switch", "program_exit"],
            "run_to": true, "step": false, "pause_waits": 0, "run": false, "input": 1, "held_keys": 1, "until_ms": false,
        })
    );
    let queued = entries_of(&hub, "input_queued");
    assert_eq!(cancelled, [queued[0]["cause"].as_u64().unwrap()], "the input's operation is cancelled");
    // The key is forgotten: no key-up reaches the restored machine.
    let before = cpu.bus.kbc.pending();
    hub.release_keys(&mut cpu);
    assert_eq!(cpu.bus.kbc.pending(), before, "no break code after the load");
    assert!(!hub.begin_batch(&cpu), "nothing is left to stop the machine");
    assert!(hub.paused, "the machine stays paused");
    let listed = ask(&mut cpu, &mut hub, Cmd::ListBreakpoints, 0).unwrap();
    assert_eq!(listed["breakpoints"].as_array().map(Vec::len), Some(0), "{}", listed);
}

#[test]
fn the_cache_hit_rate_starts_again_after_a_load() {
    let (_cpu, mut hub) = machine("cache-rate");
    hub.stats.cache_mark = (100, 50);
    hub.stats.window_start = std::time::Instant::now() - std::time::Duration::from_secs(2);
    // A loaded state put in a new decode cache, counting from zero.
    hub.record_batch(0, std::time::Duration::ZERO, 10, 5);
    assert!((hub.stats.cache_hit_rate - 10.0 / 15.0).abs() < 1e-9, "{}", hub.stats.cache_hit_rate);
    assert_eq!(hub.stats.cache_mark, (10, 5));
}

/// The scan codes of the keys going down to type `c`.
fn scans(c: char, layout: &str) -> Vec<u8> {
    let mut out = Vec::new();
    keys_for_char(c, Layout::by_code(layout).unwrap(), &mut out).unwrap();
    out.iter()
        .filter_map(|ev| match ev {
            LowInput::KeyDown { key, .. } => Some(key.scan),
            LowInput::Char(byte) => Some(*byte),
            _ => None,
        })
        .collect()
}

#[test]
fn text_is_typed_on_the_layout_s_keys() {
    assert_eq!(scans('y', "gr"), [0x2C]);
    assert_eq!(scans('y', "us"), [0x15]);
    assert_eq!(scans(':', "gr"), [0x2A, 0x34], "Shift and the period key");
    assert_eq!(scans('\\', "gr"), [0x38, 0x0C], "AltGr and the ß key");
    assert_eq!(scans('ä', "gr"), [0x28]);
    // A character no key types comes as a keystroke of its own.
    assert_eq!(scans('é', "us"), [0x82]);
    assert_eq!(scans('\n', "fr"), [0x1C]);
}

#[test]
fn step_over_runs_calls_interrupts_loops_and_repeats() {
    // CALL rel16, CALL [BX], INT 21h, INT3, LOOP, REP MOVSB, REPNE SCASB.
    assert_eq!(step_over_len(&[0xE8, 0x10, 0x00, 0x90], false), Some(3));
    assert_eq!(step_over_len(&[0xFF, 0x17, 0x90], false), Some(2));
    assert_eq!(step_over_len(&[0xCD, 0x21, 0x90], false), Some(2));
    assert_eq!(step_over_len(&[0xCC, 0x90], false), Some(1));
    assert_eq!(step_over_len(&[0xE2, 0xFE, 0x90], false), Some(2));
    assert_eq!(step_over_len(&[0xF3, 0xA4, 0x90], false), Some(2));
    assert_eq!(step_over_len(&[0xF2, 0xAE, 0x90], false), Some(2));
    // CALL rel32 in 32-bit code.
    assert_eq!(step_over_len(&[0xE8, 0, 0, 0, 0, 0x90], true), Some(5));
}

#[test]
fn other_instructions_just_step() {
    // MOV AX,1234h; JMP short; MOVSB without REP; the emulator's traps.
    assert_eq!(step_over_len(&[0xB8, 0x34, 0x12], false), None);
    assert_eq!(step_over_len(&[0xEB, 0xFE], false), None);
    assert_eq!(step_over_len(&[0xA4], false), None);
    assert_eq!(step_over_len(&[0xFE, 0x38, 0x21], false), None);
    assert_eq!(step_over_len(&[0xFE, 0x39, 0x08], false), None);
}

/// A machine at the prompt in deterministic mode, paused at the start
/// as the front end has it, with PROBE.COM on C:.
fn deterministic_machine(name: &str) -> (Cpu, DebugHub, Deterministic) {
    let mode = Deterministic::new(rust_dos::deterministic::default_start());
    let dir = crate::test_dir::path(&format!("det-{name}"));
    std::fs::create_dir_all(&dir).unwrap();
    let code: Vec<u8> = (0..PROBE.len()).step_by(2).map(|i| u8::from_str_radix(&PROBE[i..i + 2], 16).unwrap()).collect();
    std::fs::write(dir.join("PROBE.COM"), code).unwrap();
    let mut cpu = Cpu::new(dir);
    cpu.bus.set_cycles_per_ms(3000);
    cpu.load_shell();
    let mut hub = DebugHub::for_tests(&cpu);
    hub.deterministic = Some(mode.start);
    hub.pause_at_start(&cpu);
    (cpu, hub, mode)
}

/// Run `frames` host frames of about `frame_ms` emulated milliseconds,
/// as the main loop does in deterministic mode.
fn run_deterministic(cpu: &mut Cpu, hub: &mut DebugHub, mode: &mut Deterministic, frames: usize, frame_ms: u64) {
    for _ in 0..frames {
        hub.poll(cpu);
        if hub.paused {
            continue;
        }
        let hot = hub.begin_batch(cpu);
        let target = cpu.bus.clock.icount_at_ns(cpu.bus.clock.now_ns() + frame_ms * 1_000_000);
        let end = mode.frame_end(cpu, target);
        hub.run_deterministic(cpu, mode, hot, end, |cpu, hub, reached| {
            if reached.tick {
                hub.feed_input(cpu);
            }
        });
        hub.end_batch(cpu);
    }
}

fn typed(text: &str) -> Cmd {
    Cmd::Input { events: vec![InputEvent::Type { text: text.into(), delay_ms: None }], wait: false }
}

/// Type PROBE at the prompt and run to 2 s of emulated time, in host
/// frames of `frame_ms`: the instructions executed, and memory.
fn typed_run(name: &str, frame_ms: u64) -> (u64, Vec<u8>, Vec<Value>) {
    let (mut cpu, mut hub, mut mode) = deterministic_machine(name);
    reply(&mut cpu, &mut hub, typed("PROBE\n")).unwrap();
    let resumed = reply(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: Some(2000) }).unwrap();
    assert_eq!(resumed["paused"], false);
    run_deterministic(&mut cpu, &mut hub, &mut mode, 1000, frame_ms);
    assert!(hub.paused, "stopped at until_ms");
    assert_eq!(cpu.bus.clock.now_ns() / 1_000_000, 2000);
    assert_eq!(cpu.programs.started, 1, "PROBE ran");
    let stop = entries_of(&hub, "paused").pop().unwrap();
    // At the first instruction boundary past 2 s.
    assert_eq!((stop["reason"].as_str(), stop["emu_ns"].as_u64().map(|ns| ns / 1_000_000)), (Some("time"), Some(2000)));
    // The journal, but for the host's time.
    let journal = entries(&hub)
        .into_iter()
        .map(|mut e| {
            e.as_object_mut().unwrap().remove("t_ms");
            e
        })
        .collect();
    (cpu.executed, cpu.bus.ram().to_vec(), journal)
}

#[test]
fn deterministic_input_gives_the_same_run_whatever_the_frames() {
    let a = typed_run("a", 16);
    let b = typed_run("b", 41);
    assert_eq!(a.0, b.0);
    assert!(a.1 == b.1, "the same memory");
    assert_eq!(a.2, b.2, "the same journal");
    assert!(a.2.iter().any(|e| e["type"] == "input_delivered") && a.2.iter().any(|e| e["type"] == "program_started"));
}

/// From the startup pause, with a batch file busy at the prompt as
/// AUTOEXEC.BAT is: a queued `run` of TARGET.COM resumes the machine
/// and stops at its entry. Run in host frames of `frame_ms`: the stop,
/// the instructions executed, and the journal but for the host's time.
fn queued_run(name: &str, frame_ms: u64) -> (Value, u64, Vec<Value>) {
    let (mut cpu, mut hub, mut mode) = deterministic_machine(name);
    let dir = crate::test_dir::path(&format!("det-{name}"));
    std::fs::write(dir.join("TARGET.COM"), CRASH).unwrap();
    cpu.batch.append_file("AUTOEXEC", b"@echo off\r\nPROBE.COM\r\necho ready\r\n", "");
    assert!(hub.paused, "paused at the start");
    let busy = Cmd::Run { command: "TARGET.COM".into(), stop_at_entry: true, queue: false };
    assert_eq!(reply(&mut cpu, &mut hub, busy).unwrap_err().0, 409, "busy with the batch file");
    let mut sent = send(&mut cpu, &mut hub, Cmd::Run { command: "TARGET.COM".into(), stop_at_entry: true, queue: true });
    let answer = sent.rx.try_recv().expect("an answer at once");
    assert!(matches!(answer.reply, Reply::Json(_)), "accepted");
    let op = answer.meta.op.expect("an operation");
    let mut stop = None;
    for _ in 0..1000 {
        if let Some(end) = op_reply(&hub, op) {
            stop = Some(end.unwrap());
            break;
        }
        run_deterministic(&mut cpu, &mut hub, &mut mode, 1, frame_ms);
    }
    let mut stop = stop.expect("the run ended");
    stop.as_object_mut().unwrap().remove("t_ms");
    assert!(hub.paused);
    let journal = entries(&hub)
        .into_iter()
        .map(|mut e| {
            e.as_object_mut().unwrap().remove("t_ms");
            e
        })
        .collect();
    (stop, cpu.executed, journal)
}

#[test]
fn a_paused_deterministic_machine_resumes_into_a_queued_run_the_same_whatever_the_frames() {
    let a = queued_run("queued-a", 16);
    let b = queued_run("queued-b", 41);
    let (stop, ..) = &a;
    assert_eq!((stop["reason"].as_str(), stop["program"]["name"].as_str()), (Some("program_start"), Some("TARGET.COM")));
    assert_eq!(stop["registers"]["ip"], "0100", "nothing of it ran");
    assert!(a.2.iter().any(|e| e["type"] == "program_started" && e["program"]["name"] == "PROBE.COM"), "after the batch file's program");
    assert_eq!(a.1, b.1, "the same instructions");
    assert_eq!(a.0, b.0, "the same stop");
    assert_eq!(a.2, b.2, "the same journal");
}

/// Carry out the save states asked for, in memory, as the
/// deterministic front end does.
fn carry_out_states(cpu: &mut Cpu, hub: &mut DebugHub, mode: &mut Deterministic, saved: &mut Option<Vec<u8>>) {
    while let Some(request) = hub.next_state_request() {
        let result = if request.load {
            rust_dos::savestate::machine::load(cpu, saved.as_ref().unwrap()).unwrap();
            mode.resync(cpu);
            Ok(serde_json::json!({"loaded": true}))
        } else {
            *saved = Some(rust_dos::savestate::machine::save(cpu));
            Ok(serde_json::json!({"saved": true}))
        };
        hub.state_done(cpu, request, result);
    }
}

/// A session in deterministic mode, in host frames whose emulated
/// lengths come from `frames`: typed input with waits, a strict-match
/// save and load, and a rewind. The instructions executed, memory and
/// the journal but for the host's time.
fn session(name: &str, frames: &[u64]) -> (u64, Vec<u8>, Vec<Value>) {
    let (mut cpu, mut hub, mut mode) = deterministic_machine(name);
    let mut saved = None;
    let mut next = frames.iter().copied().cycle();
    let mut go = |cpu: &mut Cpu, hub: &mut DebugHub, mode: &mut Deterministic, until_ms: u64| {
        reply(cpu, hub, Cmd::Resume { until: None, until_ms: Some(until_ms) }).unwrap();
        for _ in 0..100_000 {
            if hub.paused {
                break;
            }
            run_deterministic(cpu, hub, mode, 1, next.next().unwrap());
        }
        assert!(hub.paused && cpu.bus.clock.now_ns() / 1_000_000 == until_ms, "{}: at {} ms", name, until_ms);
    };
    let wait = |ms| InputEvent::Wait { ms };
    let ty = |text: &str| InputEvent::Type { text: text.into(), delay_ms: Some(30) };
    reply(&mut cpu, &mut hub, Cmd::Input { events: vec![wait(120), ty("PROBE"), wait(45), ty("\n")], wait: false }).unwrap();
    go(&mut cpu, &mut hub, &mut mode, 900);
    let sent = send(&mut cpu, &mut hub, Cmd::SaveState { path: "s".into(), strict_match: true, check_files: false });
    carry_out_states(&mut cpu, &mut hub, &mut mode, &mut saved);
    answer(&mut cpu, &mut hub, sent, 0).unwrap();
    reply(&mut cpu, &mut hub, Cmd::Input { events: vec![ty("DIR"), wait(70), ty("\n")], wait: false }).unwrap();
    go(&mut cpu, &mut hub, &mut mode, 1600);
    let sent = send(&mut cpu, &mut hub, Cmd::LoadState { path: "s".into(), strict_match: true, check_files: false, folders: Vec::new() });
    carry_out_states(&mut cpu, &mut hub, &mut mode, &mut saved);
    answer(&mut cpu, &mut hub, sent, 0).unwrap();
    reply(&mut cpu, &mut hub, Cmd::Input { events: vec![ty("VER\n")], wait: false }).unwrap();
    go(&mut cpu, &mut hub, &mut mode, 1500);
    // A rewind: the front end loads a state and tells the hub after.
    rust_dos::savestate::machine::load(&mut cpu, saved.as_ref().unwrap()).unwrap();
    mode.resync(&cpu);
    hub.state_loaded_elsewhere(&cpu, "rewind");
    reply(&mut cpu, &mut hub, Cmd::Input { events: vec![ty("PROBE\n")], wait: false }).unwrap();
    go(&mut cpu, &mut hub, &mut mode, 2400);
    let journal = entries(&hub)
        .into_iter()
        .map(|mut e| {
            e.as_object_mut().unwrap().remove("t_ms");
            e
        })
        .collect();
    (cpu.executed, cpu.bus.ram().to_vec(), journal)
}

#[test]
fn a_deterministic_session_with_loads_and_a_rewind_is_the_same_whatever_the_frames() {
    let a = session("session-a", &[16]);
    let b = session("session-b", &[41]);
    let c = session("session-c", &[3, 29, 1, 70, 12]);
    for other in [&b, &c] {
        assert_eq!(a.0, other.0, "the same instructions");
        assert!(a.1 == other.1, "the same memory");
        assert_eq!(a.2.len(), other.2.len(), "as many entries");
        for (x, y) in a.2.iter().zip(&other.2) {
            assert_eq!(x, y, "the same journal");
        }
    }
    let kinds: Vec<&str> = a.2.iter().map(|e| e["type"].as_str().unwrap()).collect();
    for kind in ["input_delivered", "state_saved", "state_loaded", "debugger_reset", "program_started"] {
        assert!(kinds.contains(&kind), "{} in {:?}", kind, kinds);
    }
    assert_eq!(a.2.iter().filter(|e| e["type"] == "state_loaded").count(), 2, "the load and the rewind");
}

#[test]
fn deterministic_mode_takes_input_only_while_paused() {
    let (mut cpu, mut hub, _mode) = deterministic_machine("refuse");
    assert!(reply(&mut cpu, &mut hub, typed("x")).is_ok(), "paused at the start");
    let waited = Cmd::Input { events: vec![InputEvent::Type { text: "y".into(), delay_ms: None }], wait: true };
    let held = reply(&mut cpu, &mut hub, waited).unwrap();
    assert_eq!(held["held"], true, "answered at once while paused, not when delivered");
    reply(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None }).unwrap();
    assert_eq!(reply(&mut cpu, &mut hub, typed("x")).unwrap_err().0, 409);
    let run = Cmd::Run { command: "PROBE.COM".into(), stop_at_entry: false, queue: false };
    assert_eq!(reply(&mut cpu, &mut hub, run).unwrap_err().0, 409, "run while running");
    assert_eq!(reply(&mut cpu, &mut hub, write("0050:0000", &[1], None)).unwrap_err().0, 409, "a memory write while running");
    assert!(reply(&mut cpu, &mut hub, Cmd::GetRegs).is_ok(), "reads still work");
    let passed = reply(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: Some(0) }).unwrap_err();
    assert_eq!(passed.0, 400, "a time that has passed");
    let (mut cpu, mut hub) = (Cpu::new(".".into()), DebugHub::disabled());
    let refused = reply(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: Some(5) }).unwrap_err();
    assert_eq!(refused.0, 400, "until_ms needs deterministic mode");
}

/// The body of a 409 refusal, from the request's reply.
fn refusal(sent: &mut Sent) -> Value {
    match sent.rx.try_recv().map(|a| a.reply) {
        Ok(Reply::ErrorJson(409, body)) => body,
        _ => panic!("not refused with 409"),
    }
}

#[test]
fn deterministic_mode_loads_a_state_only_while_paused_and_refuses_as_a_strict_match_save_does() {
    let (mut cpu, mut hub, _mode) = deterministic_machine("state-refuse");
    reply(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None }).unwrap();

    // A load while running is refused as it arrives, strict match or
    // not, with the body a refused strict-match save has.
    for strict_match in [false, true] {
        let mut load = send(&mut cpu, &mut hub, Cmd::LoadState { path: "/tmp/x.state".into(), strict_match, check_files: false, folders: Vec::new() });
        let body = refusal(&mut load);
        assert_eq!(body["paused"], false, "{}", body);
        assert_eq!(body["precondition"], "paused", "{}", body);
        assert!(body["error"].as_str().unwrap().contains("deterministic mode"), "{}", body);
    }
    // A strict-match save is refused by its own rule, in the same shape.
    let mut save = send(&mut cpu, &mut hub, Cmd::SaveState { path: "/tmp/x.state".into(), strict_match: true, check_files: false });
    assert!(take_state_requests(&mut hub).is_empty());
    let body = refusal(&mut save);
    assert_eq!(body["paused"], false, "{}", body);
    assert!(body["error"].as_str().unwrap().contains("pause it first"), "{}", body);
    // A plain save changes nothing and is taken while running.
    let _save = send(&mut cpu, &mut hub, Cmd::SaveState { path: "/tmp/x.state".into(), strict_match: false, check_files: false });
    assert_eq!(take_state_requests(&mut hub).len(), 1);

    // A load sent while paused, with a resume in the same frame: the
    // machine runs when the front end would load it, so it is refused.
    pause(&mut cpu, &mut hub);
    let mut load = send(&mut cpu, &mut hub, Cmd::LoadState { path: "/tmp/x.state".into(), strict_match: false, check_files: false, folders: Vec::new() });
    reply(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None }).unwrap();
    assert!(take_state_requests(&mut hub).is_empty());
    assert_eq!(refusal(&mut load)["paused"], false);

    // Paused with nothing pending, both loads are taken.
    pause(&mut cpu, &mut hub);
    let _plain = send(&mut cpu, &mut hub, Cmd::LoadState { path: "/tmp/x.state".into(), strict_match: false, check_files: false, folders: Vec::new() });
    let _strict = send(&mut cpu, &mut hub, Cmd::LoadState { path: "/tmp/x.state".into(), strict_match: true, check_files: false, folders: Vec::new() });
    assert_eq!(take_state_requests(&mut hub).len(), 2);
}

#[test]
fn a_strict_match_load_drops_a_pending_until_ms_and_input_waiting_on_emulated_time() {
    let (mut cpu, mut hub, mut mode) = deterministic_machine("reset-until");
    reply(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: Some(5000) }).unwrap();
    run_deterministic(&mut cpu, &mut hub, &mut mode, 10, 16);
    pause(&mut cpu, &mut hub);
    let status = reply(&mut cpu, &mut hub, Cmd::Status).unwrap();
    assert_eq!(status["deterministic"]["until_ms"], 5000, "still pending while paused");
    // Input typed on emulated time: a wait under way and keys after it.
    hub.input_wait_ns = Some(cpu.bus.clock.now_ns() + 500_000_000);
    reply(&mut cpu, &mut hub, typed("x")).unwrap();

    let (cleared, _) = hub.reset_after_load(&cpu);
    assert_eq!(cleared["until_ms"], true, "{}", cleared);
    assert!(cleared["input"].as_u64().unwrap() > 0, "{}", cleared);
    assert_eq!((hub.stop_at_ns, hub.input_wait_ns), (None, None));
    let status = reply(&mut cpu, &mut hub, Cmd::Status).unwrap();
    assert_eq!(status["deterministic"]["until_ms"], serde_json::Value::Null);
    assert!(!hub.begin_batch(&cpu), "nothing is left to stop the machine");

    // Run on past 5 s: no stop at the old time.
    reply(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None }).unwrap();
    run_deterministic(&mut cpu, &mut hub, &mut mode, 700, 16);
    assert!(cpu.bus.clock.now_ns() > 5_000_000_000);
    assert!(!hub.paused, "the dropped until_ms didn't stop it");
}

// ----- the journal --------------------------------------------------------

/// The answer to `cmd` sent with `If-State-Seq: if_state`: the reply's
/// JSON or the error's status and body, and the journal's state.
fn ask_guarded(cpu: &mut Cpu, hub: &mut DebugHub, cmd: Cmd, if_state: Option<u64>) -> (Result<Value, (u16, Value)>, super::Meta) {
    let (mut request, mut rx) = hub.request(cmd);
    request.if_state = if_state;
    hub.handle(cpu, request);
    let answer = rx.try_recv().expect("an answer at once");
    let result = match answer.reply {
        Reply::Json(v) => Ok(v),
        Reply::Error(code, e) => Err((code, serde_json::json!({"error": e}))),
        Reply::ErrorJson(code, v) => Err((code, v)),
        _ => panic!("not a JSON reply"),
    };
    (result, answer.meta)
}

/// The journal's state now.
fn journal_state(hub: &DebugHub) -> (u64, Option<u64>, u64) {
    hub.with_journal(|j| (j.state_seq, j.stop_seq, j.last())).unwrap()
}

fn op_state(hub: &DebugHub, op: u64) -> super::journal::Op {
    hub.with_journal(|j| match j.op(op) {
        OpLookup::Found(op) => op.clone(),
        _ => panic!("no operation {}", op),
    })
    .unwrap()
}

/// The request number the answer to a request that started an
/// operation names.
fn started_op(cpu: &mut Cpu, hub: &mut DebugHub, cmd: Cmd) -> u64 {
    let (request, mut rx) = hub.request(cmd);
    let req = request.req;
    hub.handle(cpu, request);
    let answer = rx.try_recv().expect("an answer at once");
    assert!(matches!(answer.reply, Reply::Json(_)), "accepted");
    assert_eq!(answer.meta.op, Some(req));
    req
}

/// Stopped at PROBE.COM's first instruction: its code segment.
fn at_probe_entry(cpu: &mut Cpu, hub: &mut DebugHub) -> String {
    let stop = ask(cpu, hub, Cmd::Run { command: "PROBE.COM".into(), stop_at_entry: true, queue: false }, 50).unwrap();
    stop["registers"]["cs"].as_str().unwrap().to_string()
}

#[test]
fn the_journal_numbers_its_entries_and_follows_the_state() {
    let (mut cpu, mut hub) = machine("journal-numbers");
    let cs = at_probe_entry(&mut cpu, &mut hub);
    ask(&mut cpu, &mut hub, write("0200:0000", &[1, 2], None), 0).unwrap();
    ask(&mut cpu, &mut hub, Cmd::AddBreakpoint { addr: format!("{}:0103", cs), once: false }, 0).unwrap();
    ask(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None }, 0).unwrap();
    ask(&mut cpu, &mut hub, Cmd::WaitPause, 50).unwrap();
    let all = entries(&hub);
    assert_eq!(all[0]["type"], "started");
    assert_eq!(all[0]["seq"], 1);
    assert_eq!(all[0]["build"], rust_dos::build_info::json(), "the commit the program was built from");
    for (i, e) in all.iter().enumerate() {
        assert_eq!(e["seq"].as_u64(), Some(i as u64 + 1), "gapless: {}", e);
    }
    let changed = all.iter().rfind(|e| super::Kind::parse(e["type"].as_str().unwrap()).unwrap().changes_state());
    assert_eq!(journal_state(&hub).0, changed.unwrap()["seq"].as_u64().unwrap());
    // A breakpoint is the debugger's own: it changes nothing.
    let set = &entries_of(&hub, "breakpoint_set")[0];
    assert!(set["seq"].as_u64().unwrap() > entries_of(&hub, "memory_written")[0]["seq"].as_u64().unwrap());
}

#[test]
fn a_pause_and_a_resume_in_one_drain_are_told_in_order() {
    let (mut cpu, mut hub) = machine("journal-order");
    let pause = hub.request(Cmd::Pause);
    let resume = hub.request(Cmd::Resume { until: None, until_ms: None });
    let resume_req = resume.0.req;
    hub.handle(&mut cpu, pause.0);
    hub.handle(&mut cpu, resume.0);
    let types: Vec<Value> = entries(&hub).iter().skip(1).map(|e| e["type"].clone()).collect();
    assert_eq!(types, ["paused", "resumed"]);
    assert_eq!(op_state(&hub, resume_req).state, OpState::Pending);
    // Another client pauses: the resume ends stopped, by that pause.
    let (request, _rx) = hub.request(Cmd::Pause);
    let pause_req = request.req;
    hub.handle(&mut cpu, request);
    let op = op_state(&hub, resume_req);
    assert_eq!(op.state, OpState::Stopped);
    let stop = &entries_of(&hub, "paused")[1];
    assert_eq!((stop["by"].as_u64(), stop["cause"].as_u64(), stop["op"].as_u64()), (Some(pause_req), Some(pause_req), Some(resume_req)));
    assert_eq!(op.end_seq, stop["seq"].as_u64());
}

#[test]
fn a_stop_before_the_wait_is_found_after_the_resume() {
    let (mut cpu, mut hub) = machine("journal-early-stop");
    let cs = at_probe_entry(&mut cpu, &mut hub);
    ask(&mut cpu, &mut hub, Cmd::AddBreakpoint { addr: format!("{}:0103", cs), once: false }, 0).unwrap();
    let op = started_op(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None });
    let resumed = entries_of(&hub, "resumed").last().unwrap()["seq"].as_u64().unwrap();
    // The machine stops before anyone asks.
    run(&mut cpu, &mut hub, 5);
    assert!(hub.paused);
    let stop = stop_after(&hub, resumed).expect("the stop is in the journal");
    assert_eq!((stop["reason"].as_str(), stop["op"].as_u64()), (Some("breakpoint"), Some(op)));
    assert_eq!(op_reply(&hub, op).unwrap().unwrap()["seq"], stop["seq"]);
}

#[test]
fn a_breakpoint_once_goes_once_it_stops_the_machine() {
    let (mut cpu, mut hub) = machine("journal-once");
    let cs = at_probe_entry(&mut cpu, &mut hub);
    let at = format!("{}:0103", cs);
    let listed = ask(&mut cpu, &mut hub, Cmd::AddBreakpoint { addr: at.clone(), once: true }, 0).unwrap();
    assert_eq!(listed["once"].as_array().map(Vec::len), Some(1));
    ask(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None }, 0).unwrap();
    run(&mut cpu, &mut hub, 5);
    // A wait that comes late, and the journal, show the same stop.
    let late = ask(&mut cpu, &mut hub, Cmd::WaitPause, 0).unwrap();
    let stop = entries_of(&hub, "paused").pop().unwrap();
    for e in [&late, &stop] {
        assert_eq!((e["reason"].as_str(), e["once"].as_bool()), (Some("breakpoint"), Some(true)), "{}", e);
        assert_eq!(e["breakpoint"], stop["breakpoint"]);
    }
    assert_eq!(late["stop"], stop);
    let listed = ask(&mut cpu, &mut hub, Cmd::ListBreakpoints, 0).unwrap();
    assert_eq!(listed["breakpoints"].as_array().map(Vec::len), Some(0), "{}", listed);

    // A run to the same address names no breakpoint.
    ask(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None }, 0).unwrap();
    run(&mut cpu, &mut hub, 20);
    let cs = at_probe_entry(&mut cpu, &mut hub);
    ask(&mut cpu, &mut hub, Cmd::Resume { until: Some(format!("{}:0103", cs)), until_ms: None }, 0).unwrap();
    run(&mut cpu, &mut hub, 5);
    let stop = entries_of(&hub, "paused").pop().unwrap();
    assert!(stop.get("breakpoint").is_none() && stop["target"].is_string(), "{}", stop);
}

#[test]
fn a_step_another_resume_takes_over_is_superseded() {
    let (mut cpu, mut hub) = machine("journal-supersede");
    let cs = at_probe_entry(&mut cpu, &mut hub);
    ask(&mut cpu, &mut hub, Cmd::AddBreakpoint { addr: format!("{}:0106", cs), once: false }, 0).unwrap();
    let step = started_op(&mut cpu, &mut hub, Cmd::Step { count: 1 });
    let resume = started_op(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None });
    let op = op_state(&hub, step);
    assert_eq!((op.state, op.superseded_by), (OpState::Superseded, Some(resume)));
    assert_eq!(entries_of(&hub, "resumed").last().unwrap()["supersedes"].as_u64(), Some(step));
    run(&mut cpu, &mut hub, 5);
    // Where the step's chain ends: the resume's stop.
    let stop = op_reply(&hub, resume).unwrap().unwrap();
    assert_eq!((stop["reason"].as_str(), stop["op"].as_u64()), (Some("breakpoint"), Some(resume)));
}

#[test]
fn a_run_another_resume_takes_over_does_not_stop_at_entry() {
    let (mut cpu, mut hub) = machine("journal-supersede-run");
    let run_op = started_op(&mut cpu, &mut hub, Cmd::Run { command: "PROBE.COM".into(), stop_at_entry: true, queue: false });
    let resume = started_op(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None });
    assert_eq!(op_state(&hub, run_op).superseded_by, Some(resume));
    assert!(hub.run_wait.is_none() && !hub.start_once, "the run's stop at entry is dropped");
    run(&mut cpu, &mut hub, 50);
    assert!(!hub.paused, "the program ran through its entry");
    assert!(entries_of(&hub, "paused").is_empty(), "{:?}", entries_of(&hub, "paused"));
    assert_eq!(op_state(&hub, run_op).state, OpState::Superseded);
    assert!(entries_of(&hub, "program_started").iter().any(|e| e["program"]["name"] == "PROBE.COM"), "the command line still ran");
}

#[test]
fn if_state_seq_holds_only_in_the_state_read() {
    let (mut cpu, mut hub) = machine("journal-if-state");
    pause(&mut cpu, &mut hub);
    let read = journal_state(&hub).0;
    let (done, meta) = ask_guarded(&mut cpu, &mut hub, write("0200:0000", &[1], None), Some(read));
    done.unwrap();
    assert!(meta.state_seq > read, "the write moves the state");
    // Another client wrote since.
    let mine = meta.state_seq;
    ask(&mut cpu, &mut hub, write("0200:0000", &[2], None), 0).unwrap();
    let (refused, _) = ask_guarded(&mut cpu, &mut hub, write("0200:0000", &[3], None), Some(mine));
    let (code, body) = refused.unwrap_err();
    assert_eq!((code, body["precondition"].as_str()), (409, Some("state")), "{}", body);
    assert_eq!(body["state_seq"].as_u64(), Some(journal_state(&hub).0));
    assert_eq!(cpu.bus.peek_8(0x2000), 2, "nothing was written");

    // A resume and a stop since.
    let read = journal_state(&hub).0;
    ask(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None }, 0).unwrap();
    let (refused, _) = ask_guarded(&mut cpu, &mut hub, write("0200:0000", &[3], None), Some(read));
    assert_eq!(refused.unwrap_err().1["paused"], false, "refused while running");
    pause(&mut cpu, &mut hub);
    let (refused, _) = ask_guarded(&mut cpu, &mut hub, write("0200:0000", &[3], None), Some(read));
    assert_eq!(refused.unwrap_err().0, 409, "refused after the stop");

    // Input being typed while paused, and the settings window open.
    let read = journal_state(&hub).0;
    ask(&mut cpu, &mut hub, Cmd::Input { events: vec![InputEvent::Wait { ms: 60_000 }], wait: false }, 0).unwrap();
    let (refused, _) = ask_guarded(&mut cpu, &mut hub, Cmd::GetRegs, Some(read));
    assert_eq!(refused.unwrap_err().1["input_queued"], 1);
    ask(&mut cpu, &mut hub, Cmd::InputClear, 0).unwrap();
    let read = journal_state(&hub).0;
    hub.divert = true;
    let (refused, _) = ask_guarded(&mut cpu, &mut hub, Cmd::GetRegs, Some(read));
    assert_eq!(refused.unwrap_err().1["settings_window"], true);
    hub.divert = false;
    assert!(ask_guarded(&mut cpu, &mut hub, Cmd::GetRegs, Some(read)).0.is_ok());
    // Rewinding loads states the journal hears about only at its end.
    hub.rewinding = true;
    let (refused, _) = ask_guarded(&mut cpu, &mut hub, Cmd::GetRegs, Some(read));
    assert_eq!(refused.unwrap_err().1["error"], "the machine is being rewound");
    hub.rewinding = false;
    assert!(ask_guarded(&mut cpu, &mut hub, Cmd::GetRegs, Some(read)).0.is_ok());

    // Deterministic mode types nothing while paused: queued input is no
    // obstacle there.
    let (mut cpu, mut hub, _mode) = deterministic_machine("journal-if-state-det");
    reply(&mut cpu, &mut hub, typed("x")).unwrap();
    let read = journal_state(&hub).0;
    assert!(ask_guarded(&mut cpu, &mut hub, write("0200:0000", &[1], None), Some(read)).0.is_ok());
}

#[test]
fn a_strict_match_load_cancels_the_operations_under_way() {
    let (mut cpu, mut hub) = machine("journal-strict");
    pause(&mut cpu, &mut hub);
    hub.input.push_back(LowInput::Wait(std::time::Duration::from_secs(60)));
    let (_, refused) = state_request(&mut cpu, &mut hub, true);
    assert_eq!(refused.unwrap().1["precondition"], "quiescent", "a save waits for the input");
    hub.input.clear();

    let input = started_op(&mut cpu, &mut hub, Cmd::Input { events: vec![InputEvent::Wait { ms: 60_000 }], wait: true });
    let (load, _rx) = hub.request(Cmd::LoadState { path: "/tmp/x.state".into(), strict_match: true, check_files: false, folders: Vec::new() });
    let load_req = load.req;
    hub.handle(&mut cpu, load);
    let request = hub.next_state_request().expect("taken");
    hub.state_done(&cpu, request, Ok(serde_json::json!({"loaded": true})));
    let tail: Vec<Value> = entries(&hub).into_iter().rev().take(3).collect();
    assert_eq!(tail.iter().rev().map(|e| e["type"].as_str().unwrap()).collect::<Vec<_>>(), ["state_loaded", "debugger_reset", "paused"]);
    assert_eq!(tail[1]["cancelled"], serde_json::json!([input]));
    assert_eq!((tail[0]["reason"].as_str(), tail[0]["cause"].as_u64()), (Some("load"), Some(load_req)));
    assert_eq!(op_state(&hub, input).state, OpState::Cancelled);
    assert_eq!(op_state(&hub, input).end_seq, tail[1]["seq"].as_u64());
}

#[test]
fn eviction_tells_what_is_gone() {
    let cpu = Cpu::new(".".into());
    let shared = std::sync::Arc::new(super::Shared::with_capacity(std::time::Instant::now(), 8));
    let mut hub = DebugHub::new(None, Some(shared), 10);
    hub.announce(&cpu);
    for i in 0..20 {
        hub.append(&cpu, super::Kind::BreakpointSet, None, serde_json::json!({"i": i}));
    }
    hub.with_journal(|j| {
        assert_eq!((j.first(), j.last()), (14, 21));
        assert_eq!(j.after(5).err(), Some(super::journal::Gone { first: 14, last: 21 }));
        assert_eq!(j.after(13).map(|it| it.count()).ok(), Some(8));
    });
}

#[test]
fn a_pending_operation_outlives_any_number_of_others() {
    let (mut cpu, mut hub) = machine("journal-pending");
    pause(&mut cpu, &mut hub);
    // Input queued with a client's ID, which nothing delivers while
    // only requests are carried out.
    let (mut request, _rx) = hub.request(typed("x"));
    let input = request.req;
    request.tag = Some(std::sync::Arc::from("typed-1"));
    hub.handle(&mut cpu, request);
    // More operations than the journal keeps, each ended at once.
    for _ in 0..super::journal::OPS_CAPACITY + 100 {
        started_op(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None });
        ask(&mut cpu, &mut hub, Cmd::Pause, 0).unwrap();
    }
    assert_eq!(op_state(&hub, input).state, OpState::Pending, "still there");
    assert!(hub.with_journal(|j| j.ops_len()).unwrap() <= super::journal::OPS_CAPACITY + 1);
    // Delivered now, its entry has the request's ID.
    run(&mut cpu, &mut hub, 20);
    assert_eq!(op_state(&hub, input).state, OpState::Done);
    let delivered = entries_of(&hub, "input_delivered").pop().unwrap();
    assert_eq!((delivered["cause"].as_u64(), delivered["request_id"].as_str()), (Some(input), Some("typed-1")));
}

#[test]
fn input_past_the_pending_limit_is_refused() {
    let (mut cpu, mut hub) = machine("journal-pending-limit");
    pause(&mut cpu, &mut hub);
    for _ in 0..super::journal::PENDING_INPUT_CAPACITY {
        ask(&mut cpu, &mut hub, typed("x"), 0).unwrap();
    }
    let refused = reply(&mut cpu, &mut hub, typed("x")).unwrap_err();
    assert_eq!(refused.0, 429, "{}", refused.1);
    // Once some are delivered, input is taken again.
    ask(&mut cpu, &mut hub, Cmd::InputClear, 0).unwrap();
    assert!(reply(&mut cpu, &mut hub, typed("x")).is_ok());
}

#[test]
fn a_write_after_a_load_in_the_same_frame_comes_after_it() {
    let mut cpu = Cpu::new(".".into());
    let (tx, rx) = std::sync::mpsc::channel();
    let shared = std::sync::Arc::new(super::Shared::new(std::time::Instant::now()));
    let mut hub = DebugHub::new(Some(rx), Some(shared), 10);
    hub.announce(&cpu);
    let (load, _load_rx) = hub.request(Cmd::LoadState { path: "/tmp/x.state".into(), strict_match: false, check_files: false, folders: Vec::new() });
    let (write, mut write_rx) = hub.request(write("0200:0000", &[9], None));
    tx.send(load).unwrap();
    tx.send(write).unwrap();
    hub.poll(&mut cpu);
    assert!(write_rx.try_recv().is_err(), "the write waits for the load");
    let request = hub.next_state_request().unwrap();
    hub.state_done(&cpu, request, Ok(serde_json::json!({"loaded": true})));
    hub.poll(&mut cpu);
    assert!(write_rx.try_recv().is_ok());
    let types: Vec<Value> = entries(&hub).iter().skip(1).map(|e| e["type"].clone()).collect();
    assert_eq!(types, ["state_loaded", "memory_written"]);
}

#[test]
fn programs_are_told_without_a_stop_on_them() {
    let (mut cpu, mut hub) = machine("journal-programs");
    cpu.queue_batch_lines(["PROBE.COM"]);
    run(&mut cpu, &mut hub, 50);
    let started = entries_of(&hub, "program_started");
    let exited = entries_of(&hub, "program_exited");
    assert_eq!(started.last().unwrap()["program"]["name"], "PROBE.COM");
    assert_eq!(exited.last().unwrap()["exit"]["name"], "PROBE.COM");
    assert!(started.last().unwrap()["seq"].as_u64() < exited.last().unwrap()["seq"].as_u64());
    assert!(entries_of(&hub, "paused").is_empty());
}

#[test]
fn a_quit_cancels_what_is_pending_and_ends_the_journal() {
    let mut cpu = Cpu::new(".".into());
    cpu.load_shell();
    let (tx, rx) = std::sync::mpsc::channel();
    let shared = std::sync::Arc::new(super::Shared::new(std::time::Instant::now()));
    let mut hub = DebugHub::new(Some(rx), Some(shared.clone()), 10);
    hub.announce(&cpu);
    let resume = started_op(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None });
    let (quit, mut quit_rx) = hub.request(Cmd::Quit);
    let quit_req = quit.req;
    let (late, mut late_rx) = hub.request(write("0200:0000", &[9], None));
    tx.send(quit).unwrap();
    tx.send(late).unwrap();
    hub.poll(&mut cpu);
    assert!(hub.quit_requested());
    assert!(matches!(quit_rx.try_recv().map(|a| a.reply), Ok(Reply::Json(_))));
    assert!(late_rx.try_recv().is_err(), "nothing is carried out after a quit");
    let (refused, _) = ask_guarded(&mut cpu, &mut hub, Cmd::GetRegs, None);
    assert_eq!(refused.unwrap_err().0, 503);

    hub.shutdown(&cpu, super::ShutdownReason::Quit, 0);
    assert!(matches!(late_rx.try_recv().map(|a| a.reply), Ok(Reply::Error(503, _))));
    assert_ne!(cpu.bus.peek_8(0x2000), 9);
    let last = entries(&hub).pop().unwrap();
    assert_eq!((last["type"].as_str(), last["reason"].as_str()), (Some("shutdown"), Some("quit")));
    assert_eq!((last["by"].as_u64(), last["cause"].as_u64()), (Some(quit_req), Some(quit_req)));
    assert_eq!(last["cancelled"], serde_json::json!([resume]));
    assert_eq!(op_state(&hub, resume).state, OpState::Cancelled);
    assert!(shared.closed.load(std::sync::atomic::Ordering::Acquire));
    hub.append(&cpu, super::Kind::BreakpointSet, None, serde_json::json!({}));
    assert_eq!(entries(&hub).pop().unwrap()["type"], "shutdown", "nothing after it");
    // A request that passed the server's check just before the shutdown
    // isn't left waiting: the channel is closed.
    let (after, mut after_rx) = hub.request(Cmd::GetRegs);
    assert!(tx.send(after).is_err(), "nobody takes requests any more");
    assert!(matches!(after_rx.try_recv(), Err(oneshot::error::TryRecvError::Closed)));
}

#[test]
fn requests_sent_together_take_effect_in_the_order_sent() {
    let mut cpu = Cpu::new(".".into());
    cpu.load_shell();
    let (tx, rx) = std::sync::mpsc::channel();
    let shared = std::sync::Arc::new(super::Shared::new(std::time::Instant::now()));
    let mut hub = DebugHub::new(Some(rx), Some(shared), 10);
    hub.announce(&cpu);
    let (p, _p) = hub.request(Cmd::Pause);
    tx.send(p).unwrap();
    hub.poll(&mut cpu);
    assert!(hub.paused);
    let before = journal_state(&hub).2;
    let state = journal_state(&hub).0;
    // All in the channel at once, as from clients in one frame.
    let mut cmds = vec![
        (write("0200:0000", &[1], None), None),
        (Cmd::SaveState { path: "s".into(), strict_match: true, check_files: false }, None),
        (write("0200:0000", &[2], None), None),
        (Cmd::Speed { cycles: "5000".into() }, None),
        // Guarded by the state before the first write: refused.
        (write("0200:0000", &[3], None), Some(state)),
        (Cmd::LoadState { path: "s".into(), strict_match: true, check_files: false, folders: Vec::new() }, None),
        // Holds only once the load brought back the 1.
        (write("0200:0000", &[4], Some(&[1])), None),
        (Cmd::Resume { until: None, until_ms: None }, None),
        (Cmd::Pause, None),
    ];
    let mut answers = Vec::new();
    for (cmd, if_state) in cmds.drain(..) {
        let (mut request, rx) = hub.request(cmd);
        request.if_state = if_state;
        tx.send(request).unwrap();
        answers.push(rx);
    }
    let mut saved: Option<Vec<u8>> = None;
    for _ in 0..10 {
        hub.poll(&mut cpu);
        while let Some(request) = hub.next_state_request() {
            let result = if request.load {
                rust_dos::savestate::machine::load(&mut cpu, saved.as_ref().unwrap()).unwrap();
                Ok(serde_json::json!({"loaded": true}))
            } else {
                saved = Some(rust_dos::savestate::machine::save(&cpu));
                Ok(serde_json::json!({"saved": true}))
            };
            hub.state_done(&cpu, request, result);
        }
        while let Some(request) = hub.next_speed_request() {
            let cycles = request.cycles.clone();
            hub.speed_done(&cpu, request, Ok(serde_json::json!({"cycles": cycles})));
        }
    }
    let statuses: Vec<u16> = answers
        .iter_mut()
        .map(|rx| match rx.try_recv().expect("answered").reply {
            Reply::Json(_) => 200,
            Reply::Error(code, _) | Reply::ErrorJson(code, _) => code,
            _ => panic!("not a JSON reply"),
        })
        .collect();
    assert_eq!(statuses, [200, 200, 200, 200, 409, 200, 200, 200, 200]);
    let types: Vec<String> = entries(&hub).iter().filter(|e| e["seq"].as_u64() > Some(before)).map(|e| e["type"].as_str().unwrap().to_string()).collect();
    assert_eq!(
        types,
        ["memory_written", "state_saved", "memory_written", "speed_changed", "state_loaded", "debugger_reset", "paused", "memory_written", "resumed", "paused"]
    );
    assert_eq!(cpu.bus.peek_8(0x2000), 4);
}

/// A program that stops the machine every way a program can: it writes
/// memory, calls, switches to protected mode and back, then divides by
/// zero.
const STOPS: [u8; 34] = [
    0xC7, 0x06, 0x00, 0x02, 0x34, 0x12, // 0100 mov word [0200], 1234
    0xB8, 0x01, 0x00, // 0106 mov ax, 1
    0xE8, 0x04, 0x00, // 0109 call 0110
    0x90, // 010C nop
    0xEB, 0x02, // 010D jmp 0111
    0x90, // 010F
    0xC3, // 0110 ret
    0x0F, 0x20, 0xC0, // 0111 mov eax, cr0
    0x0C, 0x01, // 0114 or al, 1
    0x0F, 0x22, 0xC0, // 0116 mov cr0, eax
    0x24, 0xFE, // 0119 and al, FE
    0x0F, 0x22, 0xC0, // 011B mov cr0, eax
    0x31, 0xC0, 0xF6, 0xF0, // 011E xor ax, ax; div al
];

/// Start an operation and run until the machine stops: the stop's
/// entry, checked against the operation it ended.
fn stopped_by(cpu: &mut Cpu, hub: &mut DebugHub, cmd: Cmd, reason: &str) -> Value {
    let op = started_op(cpu, hub, cmd);
    for _ in 0..200 {
        if hub.paused {
            break;
        }
        run(cpu, hub, 1);
    }
    assert!(hub.paused, "stopped: {}", reason);
    let stop = entries_of(hub, "paused").pop().unwrap();
    assert_eq!(stop["reason"].as_str(), Some(reason), "{}", stop);
    assert_eq!((stop["op"].as_u64(), stop["cause"].as_u64()), (Some(op), Some(op)), "{}", stop);
    let ended = op_state(hub, op);
    assert_eq!((ended.state, ended.end_seq), (OpState::Stopped, stop["seq"].as_u64()), "{}", stop);
    let (state_seq, stop_seq, _) = journal_state(hub);
    assert_eq!((Some(state_seq), stop_seq), (stop["seq"].as_u64(), stop["seq"].as_u64()), "the state is the stop's");
    stop
}

#[test]
fn every_stop_names_its_reason_and_ends_its_operation() {
    let (mut cpu, mut hub) = machine("journal-stops");
    let dir = crate::test_dir::path("debug-journal-stops");
    std::fs::write(dir.join("STOPS.COM"), STOPS).unwrap();
    let entry = stopped_by(&mut cpu, &mut hub, Cmd::Run { command: "STOPS.COM".into(), stop_at_entry: true, queue: false }, "program_start");
    let cs = entry["registers"]["cs"].as_str().unwrap().to_string();
    let at = |stop: &Value| stop["registers"]["ip"].as_str().unwrap().to_string();

    ask(&mut cpu, &mut hub, Cmd::AddWatchpoint { addr: format!("{}:0200", cs), len: 2 }, 0).unwrap();
    let resume = || Cmd::Resume { until: None, until_ms: None };
    let stop = stopped_by(&mut cpu, &mut hub, resume(), "watchpoint");
    assert_eq!(at(&stop), "0106");
    let stop = stopped_by(&mut cpu, &mut hub, Cmd::Step { count: 1 }, "step");
    assert_eq!(at(&stop), "0109");
    // A call is stepped over by running to the instruction after it.
    let stop = stopped_by(&mut cpu, &mut hub, Cmd::StepOver, "breakpoint");
    assert_eq!((at(&stop).as_str(), stop["target"].is_string()), ("010C", true), "the call ran through");
    ask(&mut cpu, &mut hub, Cmd::AddBreakpoint { addr: format!("{}:010D", cs), once: true }, 0).unwrap();
    let stop = stopped_by(&mut cpu, &mut hub, resume(), "breakpoint");
    assert_eq!((at(&stop).as_str(), stop["once"].as_bool()), ("010D", Some(true)));
    let stop = stopped_by(&mut cpu, &mut hub, Cmd::Resume { until: Some(format!("{}:0114", cs)), until_ms: None }, "breakpoint");
    assert_eq!(at(&stop), "0114");
    let on = |mode_switch, exceptions, program_exit| Cmd::BreakOn { exceptions, clear_exceptions: None, mode_switch, program_start: None, program_exit };
    ask(&mut cpu, &mut hub, on(Some(true), None, None), 0).unwrap();
    stopped_by(&mut cpu, &mut hub, resume(), "mode_switch");
    ask(&mut cpu, &mut hub, on(Some(false), Some(1), None), 0).unwrap();
    stopped_by(&mut cpu, &mut hub, resume(), "exception");
    ask(&mut cpu, &mut hub, Cmd::BreakOn { exceptions: None, clear_exceptions: Some(1), mode_switch: None, program_start: None, program_exit: Some(true) }, 0).unwrap();
    let stop = stopped_by(&mut cpu, &mut hub, resume(), "program_exit");
    assert_eq!(stop["exit"]["name"], "STOPS.COM");

    // A pause: by the request, ending the resume it stops.
    let resumed = started_op(&mut cpu, &mut hub, resume());
    run(&mut cpu, &mut hub, 2);
    let (pause, _rx) = hub.request(Cmd::Pause);
    let by = pause.req;
    hub.handle(&mut cpu, pause);
    hub.end_batch(&cpu);
    let stop = entries_of(&hub, "paused").pop().unwrap();
    assert_eq!(stop["reason"], "request");
    assert_eq!((stop["by"].as_u64(), stop["cause"].as_u64(), stop["op"].as_u64()), (Some(by), Some(by), Some(resumed)), "{}", stop);
    assert_eq!((op_state(&hub, resumed).state, op_state(&hub, resumed).end_seq), (OpState::Stopped, stop["seq"].as_u64()));

    // Deterministic mode starts stopped, by nothing.
    let (_cpu, hub, _mode) = deterministic_machine("journal-startup");
    let stop = entries_of(&hub, "paused").pop().unwrap();
    assert_eq!((stop["reason"].as_str(), &stop["op"], &stop["cause"]), (Some("startup"), &Value::Null, &Value::Null), "{}", stop);
}

#[test]
fn a_run_whose_operation_a_stop_ended_waits_no_more() {
    let (mut cpu, mut hub) = machine("journal-run-stopped");
    let dir = crate::test_dir::path("debug-journal-run-stopped");
    std::fs::write(dir.join("HOLD.BAT"), "@pause\r\nPROBE.COM\r\n").unwrap();
    let op = started_op(&mut cpu, &mut hub, Cmd::Run { command: "HOLD.BAT".into(), stop_at_entry: true, queue: false });
    run(&mut cpu, &mut hub, 20);
    assert!(hub.run_wait.is_some(), "waiting at PAUSE");
    pause(&mut cpu, &mut hub);
    assert_eq!(op_state(&hub, op).state, OpState::Stopped);
    assert!(hub.run_wait.is_none() && !hub.start_once, "the stop ended the run's wait");
    let (taken, refused) = state_request(&mut cpu, &mut hub, true);
    assert_eq!((taken, refused.map(|r| r.0)), (1, None), "a strict-match save goes ahead");
    // Its program starts with nobody stopping it at its entry.
    ask(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None }, 0).unwrap();
    ask(&mut cpu, &mut hub, Cmd::Input { events: vec![InputEvent::Type { text: " ".into(), delay_ms: None }], wait: false }, 0).unwrap();
    run(&mut cpu, &mut hub, 100);
    assert!(cpu.programs.started > 0 && entries_of(&hub, "paused").iter().all(|e| e["reason"] != "program_start"));
}

#[test]
fn a_superseded_resume_s_time_stops_nothing() {
    let (mut cpu, mut hub, mut mode) = deterministic_machine("superseded-time");
    reply(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: Some(300) }).unwrap();
    run_deterministic(&mut cpu, &mut hub, &mut mode, 2, 16);
    // A step takes over, then a plain resume runs past 300 ms.
    let step = send(&mut cpu, &mut hub, Cmd::Step { count: 1 });
    answer(&mut cpu, &mut hub, step, 5).unwrap();
    reply(&mut cpu, &mut hub, Cmd::Resume { until: None, until_ms: None }).unwrap();
    run_deterministic(&mut cpu, &mut hub, &mut mode, 40, 16);
    assert!(cpu.bus.clock.now_ns() > 400_000_000);
    assert!(!hub.paused, "{:?}", entries_of(&hub, "paused").pop());
}

#[test]
fn log_lines_name_their_request_and_the_journal_s_entry() {
    let (mut cpu, mut hub) = machine("journal-log");
    let shared = hub.shared.clone().unwrap();
    let hook = shared.clone();
    cpu.bus.log_hook = Some(Box::new(move |line: &str| hook.log_line(line)));
    let dir = crate::test_dir::path("debug-log");
    std::fs::create_dir_all(&dir).unwrap();
    let before = journal_state(&hub).2;
    let mount = Cmd::Mount { drive: "D".into(), path: dir.display().to_string(), kind: None, label: None, read_only: true, images: Vec::new() };
    let (request, _rx) = hub.request(mount);
    let req = request.req;
    hub.handle(&mut cpu, request);
    let log = shared.log.lock().unwrap();
    let line = log.iter().find(|l| l.line.contains("mounted")).expect("the mount's line");
    assert_eq!((line.cause, line.at), (Some(req), before));
    assert!(log.iter().zip(log.iter().skip(1)).all(|(a, b)| b.n == a.n + 1), "numbered in order");
    assert_eq!(entries_of(&hub, "drive_mounted")[0]["cause"].as_u64(), Some(req));
}

#[test]
fn the_fpu_registers_read_back_by_model() {
    let (mut cpu, mut hub) = (Cpu::new(".".into()), DebugHub::disabled());
    cpu.fpu_push_f64(2.0);
    cpu.fpu_push_f64(f64::INFINITY);
    let fpu = reply(&mut cpu, &mut hub, Cmd::GetFpu).unwrap();
    assert_eq!(fpu["control"], "037F");
    assert_eq!(fpu["top"], 6);
    assert_eq!(fpu["status"], "3000", "the stack top in bits 11-13");
    assert_eq!(fpu["st"][0]["reg"], 6);
    assert_eq!(fpu["st"][0]["tag"], "special");
    assert_eq!(fpu["st"][0]["value"], Value::Null, "an infinity");
    assert_eq!(fpu["st"][1]["tag"], "valid");
    assert_eq!(fpu["st"][1]["value"], 2.0);
    assert_eq!(fpu["st"][1]["bits"], "40008000000000000000");
    assert_eq!(fpu["st"][2]["tag"], "empty");
    assert!(fpu.get("mm").is_none() && fpu.get("xmm").is_none(), "a 486 has neither");

    cpu.model = crate::cpu::CpuModel::PentiumIII;
    cpu.fpu_set_reg(3, rust_dos::f80::F80 { st: 0xFFFF_0123_4567_89AB_CDEF });
    cpu.sse.xmm[1] = (1.5f32.to_bits() as u128) << 32 | 0x3F80_0000;
    let fpu = reply(&mut cpu, &mut hub, Cmd::GetFpu).unwrap();
    assert_eq!(fpu["mm"][3], "0123456789ABCDEF", "physical register 3's significand");
    assert_eq!(fpu["xmm"][1]["bits"], "00000000000000003FC000003F800000");
    assert_eq!(fpu["xmm"][1]["ps"], json!([1.0, 1.5, 0.0, 0.0]));
    assert_eq!(fpu["mxcsr"], "00001F80");
}

#[test]
fn an_fpu_value_beyond_a_double_reads_back_by_its_tag() {
    let (mut cpu, mut hub) = (Cpu::new(".".into()), DebugHub::disabled());
    for _ in 0..4 {
        cpu.fpu_push_f64(1.0);
    }
    cpu.model = crate::cpu::CpuModel::PentiumIII;
    // Physical registers 4 to 7 are ST(0) to ST(3).
    cpu.fpu_set_reg(4, rust_dos::f80::F80 { st: 0x4530_8000_0000_0000_0000 });
    cpu.fpu_set_reg(5, rust_dos::f80::F80 { st: 0x0000_4000_0000_0000_0000 });
    cpu.fpu_set_reg(6, rust_dos::f80::F80 { st: 0x3FFF_4000_0000_0000_0000 });
    cpu.sse.xmm[0] = 0x7FC0_0000;
    let fpu = reply(&mut cpu, &mut hub, Cmd::GetFpu).unwrap();
    assert_eq!(fpu["st"][0]["tag"], "valid");
    assert_eq!(fpu["st"][0]["value"], Value::Null, "2^1329 is beyond a double");
    assert_eq!(fpu["st"][1]["tag"], "special", "a denormal");
    assert_eq!(fpu["st"][1]["value"], 0.0);
    assert_eq!(fpu["st"][2]["tag"], "special", "an unnormal");
    assert_eq!(fpu["st"][3]["tag"], "valid");
    assert_eq!(fpu["tag"], "28FF", "FSTENV's tag word");
    assert_eq!(fpu["xmm"][0]["ps"][0], Value::Null, "a NaN single");
}

#[test]
fn win32_without_a_program_has_no_processes() {
    let (mut cpu, mut hub) = (Cpu::new(".".into()), DebugHub::disabled());
    cpu.bus.win32.enabled = true;
    let v = reply(&mut cpu, &mut hub, Cmd::Win32).unwrap();
    let expected = json!({"enabled": true, "active": false, "pending": null, "processes": []});
    assert_eq!(v, expected);
}

#[test]
fn win32_snapshot_reads_as_json() {
    use rust_dos::win32::snapshot::*;
    let event = ObjectInfo {
        id: 7,
        kind: "event",
        state: vec![("manual", Field::Bool(false)), ("signaled", Field::Bool(true))],
        name: Some("go".into()),
        refs: 2,
    };
    let thread = ObjectInfo {
        id: 3,
        kind: "thread",
        state: vec![("id", Field::Id(0xFFF0_0048)), ("exit_code", Field::Code(None))],
        name: None,
        refs: 2,
    };
    let snapshot = Snapshot {
        enabled: true,
        active: true,
        pending: None,
        processes: vec![ProcessInfo {
            id: 0xFFF0_0024,
            name: "T.EXE".into(),
            path: "C:\\T.EXE".into(),
            command_line: Some("T.EXE /x".into()),
            image_base: 0x40_0000,
            running: true,
            root: true,
            std: [4, 8, 12],
            modules: vec![ModuleInfo {
                name: "KERNEL32.DLL".into(),
                base: 0xBFF7_0000,
                size: 0x1_0000,
                entry: 0,
                builtin: true,
            }],
            threads: vec![ThreadInfo {
                id: 0xFFF0_0048,
                state: ThreadState::Waiting,
                eip: 0x40_1000,
                esp: 0x6F_FF00,
                last_error: 6,
                suspended: 0,
                wait: Some(WaitInfo {
                    objects: vec![(Some(16), event.clone())],
                    timeout_ms: Some(500),
                    ..WaitInfo::default()
                }),
                ..ThreadInfo::default()
            }],
            handles: vec![
                HandleInfo { handle: 16, object: event },
                HandleInfo { handle: 20, object: thread },
            ],
            ..ProcessInfo::default()
        }],
    };
    let v = super::win32::snapshot_json(&snapshot);
    let p = &v["processes"][0];
    assert_eq!(p["id"], "FFF00024");
    assert_eq!(p["command_line"], "T.EXE /x");
    assert_eq!(p["exit_code"], Value::Null);
    assert_eq!(p["modules"][0], json!({"name": "KERNEL32.DLL", "base": "BFF70000", "size": "00010000", "entry": null, "builtin": true}));
    let t = &p["threads"][0];
    assert_eq!((t["state"].as_str(), t["registers"].as_str()), (Some("waiting"), Some("saved")));
    assert_eq!((t["eip"].as_str(), t["esp"].as_str()), (Some("00401000"), Some("006FFF00")));
    assert_eq!(t["last_error"], 6);
    assert_eq!(t["wait"]["timeout_ms"], 500);
    assert_eq!(t["wait"]["critical_section"], Value::Null);
    assert_eq!(t["wait"]["objects"][0]["handle"], 16);
    assert_eq!(t["wait"]["objects"][0]["kind"], "event");
    let event = json!({"handle": 16, "object": 7, "kind": "event", "state": {"manual": false, "signaled": true}, "name": "go", "refs": 2});
    assert_eq!(p["handles"][0], event);
    assert_eq!(p["handles"][1]["state"], json!({"id": "FFF00048", "exit_code": null}));
}
