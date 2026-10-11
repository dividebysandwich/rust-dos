//! The debug server's journal: one ordered record of what happened to the
//! machine and what the debugger did to it, written only by the emulator
//! thread (`DebugHub`) and read by the server thread, which waits on it.
//!
//! Every entry has a gapless sequence number (`seq`), so a reader that
//! keeps the last one it saw can tell whether it missed anything, and a
//! JSON form serialized once and handed to every reader. The journal also
//! keeps what readers wait on: the stop the machine is in (`stop_seq`), the
//! last entry that changed the machine (`state_seq`), and the operations
//! (`Op`) started by control and input requests.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use serde_json::{Map, Value};

/// Entries kept; the oldest go first.
pub const CAPACITY: usize = 4096;
/// Operations kept once they ended; asking for one evicted gets 410. An
/// operation still pending is never evicted.
pub const OPS_CAPACITY: usize = 1024;
/// Input operations pending at once; an input request past it gets 429.
/// At most one control operation is pending, so this bounds the pending
/// operations.
pub const PENDING_INPUT_CAPACITY: usize = 1024;
/// Goes up only when a change breaks clients; additions go in `FEATURES`.
pub const API_VERSION: u32 = 1;
/// What this server offers, for clients to check before relying on it.
pub const FEATURES: &[&str] = &[
    "journal",
    "ops",
    "if_state_seq",
    "request_ids",
    "breakpoint_once",
    "quit",
    "strict_match",
    "until_ms",
    "write_expect",
    "trace_cursor",
    "run",
    "program_stops",
];

macro_rules! kinds {
    ($($kind:ident = $name:literal, $changes:literal;)*) => {
        /// What an entry records.
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        pub enum Kind {
            $($kind,)*
        }

        impl Kind {
            pub const ALL: &[Kind] = &[$(Kind::$kind,)*];

            /// The entry's `type` on the wire.
            pub fn name(self) -> &'static str {
                match self {
                    $(Kind::$kind => $name,)*
                }
            }

            /// Whether an entry of this kind changes the machine (or what
            /// a request may rely on about it), and so moves `state_seq`.
            pub fn changes_state(self) -> bool {
                match self {
                    $(Kind::$kind => $changes,)*
                }
            }
        }
    };
}

kinds! {
    Started = "started", false;
    Resumed = "resumed", true;
    Paused = "paused", true;
    ProgramStarted = "program_started", true;
    ProgramExited = "program_exited", true;
    OpFailed = "op_failed", false;
    InputQueued = "input_queued", false;
    InputDelivered = "input_delivered", true;
    InputCleared = "input_cleared", true;
    MemoryWritten = "memory_written", true;
    RegistersWritten = "registers_written", true;
    BreakpointSet = "breakpoint_set", false;
    BreakpointRemoved = "breakpoint_removed", false;
    WatchpointSet = "watchpoint_set", false;
    WatchpointRemoved = "watchpoint_removed", false;
    BreakOn = "break_on", false;
    DriveMounted = "drive_mounted", true;
    DriveUnmounted = "drive_unmounted", true;
    ImagesSwapped = "images_swapped", true;
    SharedSynced = "shared_synced", true;
    StateSaved = "state_saved", false;
    StateLoaded = "state_loaded", true;
    DebuggerReset = "debugger_reset", true;
    SpeedChanged = "speed_changed", true;
    SettingsWindow = "settings_window", true;
    HostInput = "host_input", true;
    Shutdown = "shutdown", true;
}

impl Kind {
    pub fn parse(name: &str) -> Option<Kind> {
        Kind::ALL.iter().copied().find(|k| k.name() == name)
    }
}

/// One journal entry: its sequence number, kind, the request that caused
/// it (None: the machine or the window did), the operation under way when
/// it happened, and its JSON form.
#[derive(Clone, Debug)]
pub struct Entry {
    pub seq: u64,
    pub kind: Kind,
    pub cause: Option<u64>,
    pub op: Option<u64>,
    pub json: Arc<str>,
}

impl Entry {
    /// The entry as a JSON value, for replies that build on it.
    pub fn value(&self) -> Value {
        serde_json::from_str(&self.json).unwrap_or(Value::Null)
    }
}

/// What every entry carries besides its kind's fields.
pub struct Stamp {
    /// Host milliseconds since the emulator started (the only field that
    /// differs between two deterministic runs).
    pub t_ms: u64,
    /// Instructions executed (`cpu.executed`).
    pub icount: u64,
    /// Emulated nanoseconds since power-on.
    pub emu_ns: u64,
    pub cause: Option<u64>,
    /// The client's `X-Request-Id` of the request in `cause`.
    pub request_id: Option<Arc<str>>,
    pub op: Option<u64>,
}

/// The kind of request an operation came from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OpKind {
    Resume,
    Step,
    StepOver,
    Run,
    Reboot,
    RebootShell,
    /// Queued input, done once its last event is delivered.
    Input,
}

impl OpKind {
    pub fn name(self) -> &'static str {
        match self {
            OpKind::Resume => "resume",
            OpKind::Step => "step",
            OpKind::StepOver => "step_over",
            OpKind::Run => "run",
            OpKind::Reboot => "reboot",
            OpKind::RebootShell => "reboot_shell",
            OpKind::Input => "input",
        }
    }
}

/// Where an operation is. Everything but `Pending` is final.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OpState {
    Pending,
    /// A stop ended it (`end_seq` is the `paused` entry).
    Stopped,
    /// It finished without a stop: input delivered, or a `run` whose
    /// program started (`end_seq` is the entry that says so).
    Done,
    /// A `run` that started no program (`end_seq` is the `op_failed` entry).
    Failed,
    /// Another control request took over the machine before it stopped.
    Superseded,
    /// A strict-match load, a cleared input queue or the shutdown dropped it.
    Cancelled,
}

impl OpState {
    pub fn name(self) -> &'static str {
        match self {
            OpState::Pending => "pending",
            OpState::Stopped => "stopped",
            OpState::Done => "done",
            OpState::Failed => "failed",
            OpState::Superseded => "superseded",
            OpState::Cancelled => "cancelled",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Op {
    pub kind: OpKind,
    pub state: OpState,
    /// The entry the request made as it started the operation.
    pub start_seq: u64,
    /// The entry that ended it.
    pub end_seq: Option<u64>,
    pub superseded_by: Option<u64>,
    /// A failed `run`'s status and message.
    pub error: Option<(u16, String)>,
    /// The `X-Request-Id` of the request that started it, for the entries
    /// it causes later.
    pub tag: Option<Arc<str>>,
}

/// An operation looked up by its request number.
pub enum OpLookup<'a> {
    Found(&'a Op),
    /// Evicted (or, among the evicted numbers, never an operation).
    Gone,
    Unknown,
}

/// Entries after a sequence number some of which were evicted.
#[derive(Debug, PartialEq)]
pub struct Gone {
    pub first: u64,
    pub last: u64,
}

/// Which entries a reader wants. `shutdown` matches every filter, so no
/// reader waits past it.
#[derive(Default, Clone, Debug)]
pub struct Filter {
    pub kinds: Option<Vec<Kind>>,
    pub op: Option<u64>,
    pub cause: Option<u64>,
}

impl Filter {
    pub fn matches(&self, e: &Entry) -> bool {
        e.kind == Kind::Shutdown
            || (self.kinds.as_ref().is_none_or(|k| k.contains(&e.kind))
                && self.op.is_none_or(|op| e.op == Some(op))
                && self.cause.is_none_or(|c| e.cause == Some(c)))
    }
}

pub struct Journal {
    entries: VecDeque<Entry>,
    capacity: usize,
    /// The last sequence number given out (0: none yet).
    last: u64,
    /// The `paused` entry the machine is stopped in now, if it is.
    pub stop_seq: Option<u64>,
    /// The last entry that changed the machine.
    pub state_seq: u64,
    ops: BTreeMap<u64, Op>,
    /// The operations that ended, in the order they did. The first goes
    /// once more than `OPS_CAPACITY` ended.
    ended: VecDeque<u64>,
    /// The highest operation number evicted.
    ops_evicted: u64,
    /// The `shutdown` entry, once there is one.
    pub shutdown: Option<u64>,
}

impl Journal {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(capacity.min(CAPACITY)),
            capacity: capacity.max(1),
            last: 0,
            stop_seq: None,
            state_seq: 0,
            ops: BTreeMap::new(),
            ended: VecDeque::new(),
            ops_evicted: 0,
            shutdown: None,
        }
    }

    /// The oldest entry kept (`last + 1` while there is none).
    pub fn first(&self) -> u64 {
        self.entries.front().map_or(self.last + 1, |e| e.seq)
    }

    pub fn last(&self) -> u64 {
        self.last
    }

    /// Append an entry of `kind` with `fields`, stamped with `stamp`.
    /// Returns its sequence number. Nothing is appended after `shutdown`.
    pub fn append(&mut self, kind: Kind, stamp: Stamp, fields: Map<String, Value>) -> Option<u64> {
        if self.shutdown.is_some() {
            return None;
        }
        self.last += 1;
        let seq = self.last;
        let json = serialize(seq, kind, &stamp, fields);
        if self.entries.len() >= self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(Entry { seq, kind, cause: stamp.cause, op: stamp.op, json: Arc::from(json) });
        if kind.changes_state() {
            self.state_seq = seq;
        }
        match kind {
            Kind::Paused => self.stop_seq = Some(seq),
            Kind::Resumed => self.stop_seq = None,
            Kind::Shutdown => self.shutdown = Some(seq),
            _ => {}
        }
        Some(seq)
    }

    pub fn get(&self, seq: u64) -> Option<&Entry> {
        let first = self.entries.front()?.seq;
        self.entries.get(seq.checked_sub(first)? as usize)
    }

    /// The entries after `seq`, oldest first, or `Gone` if some of them
    /// were evicted.
    pub fn after(&self, seq: u64) -> Result<impl Iterator<Item = &Entry>, Gone> {
        let first = self.first();
        if seq.saturating_add(1) < first {
            return Err(Gone { first, last: self.last });
        }
        let skip = (seq + 1).saturating_sub(first) as usize;
        Ok(self.entries.iter().skip(skip))
    }

    /// The first entry after `seq` that `filter` matches, starting at the
    /// oldest kept if those after `seq` were evicted.
    pub fn find_after(&self, seq: u64, filter: &Filter) -> Option<&Entry> {
        let from = seq.max(self.first().saturating_sub(1));
        self.after(from).ok()?.find(|e| filter.matches(e))
    }

    /// Start operation `req`, of `kind`, at entry `start_seq`, for a
    /// request with the client's ID `tag`.
    pub fn begin_op(&mut self, req: u64, kind: OpKind, start_seq: u64, tag: Option<Arc<str>>) {
        let op = Op { kind, state: OpState::Pending, start_seq, end_seq: None, superseded_by: None, error: None, tag };
        self.ops.insert(req, op);
    }

    /// Operation `req` ended. Past `OPS_CAPACITY` ended ones, the one that
    /// ended first goes; pending ones stay, however old.
    fn ended(&mut self, req: u64) {
        self.ended.push_back(req);
        while self.ended.len() > OPS_CAPACITY {
            if let Some(old) = self.ended.pop_front() {
                self.ops.remove(&old);
                self.ops_evicted = self.ops_evicted.max(old);
            }
        }
    }

    /// The pending operation `req`, to end.
    fn pending(&mut self, req: u64) -> Option<&mut Op> {
        self.ops.get_mut(&req).filter(|op| op.state == OpState::Pending)
    }

    /// End operation `req` in `state` at entry `end_seq`, if it is still
    /// pending.
    pub fn end_op(&mut self, req: u64, state: OpState, end_seq: Option<u64>) -> bool {
        let Some(op) = self.pending(req) else { return false };
        op.state = state;
        op.end_seq = end_seq;
        self.ended(req);
        true
    }

    pub fn supersede_op(&mut self, req: u64, by: u64) {
        if let Some(op) = self.pending(req) {
            op.state = OpState::Superseded;
            op.superseded_by = Some(by);
            self.ended(req);
        }
    }

    pub fn fail_op(&mut self, req: u64, end_seq: Option<u64>, status: u16, error: String) {
        if let Some(op) = self.pending(req) {
            op.state = OpState::Failed;
            op.end_seq = end_seq;
            op.error = Some((status, error));
            self.ended(req);
        }
    }

    pub fn op(&self, req: u64) -> OpLookup<'_> {
        match self.ops.get(&req) {
            Some(op) => OpLookup::Found(op),
            None if req != 0 && req <= self.ops_evicted => OpLookup::Gone,
            None => OpLookup::Unknown,
        }
    }

    /// The `X-Request-Id` of the request that started operation `req`.
    pub fn op_tag(&self, req: u64) -> Option<Arc<str>> {
        self.ops.get(&req).and_then(|op| op.tag.clone())
    }

    /// The operations kept, pending and ended.
    pub fn ops_len(&self) -> usize {
        self.ops.len()
    }

    /// The operations still pending, of the kinds `want` takes.
    pub fn pending_ops(&self, want: impl Fn(OpKind) -> bool) -> Vec<u64> {
        self.ops.iter().filter(|(_, op)| op.state == OpState::Pending && want(op.kind)).map(|(req, _)| *req).collect()
    }
}

/// The JSON of an entry: the stamp's fields first, in a fixed order, then
/// the kind's own.
fn serialize(seq: u64, kind: Kind, stamp: &Stamp, fields: Map<String, Value>) -> String {
    let opt = |v: Option<u64>| v.map_or("null".to_string(), |v| v.to_string());
    let mut out = format!(
        r#"{{"seq":{},"type":"{}","t_ms":{},"icount":{},"emu_ns":{},"cause":{}"#,
        seq,
        kind.name(),
        stamp.t_ms,
        stamp.icount,
        stamp.emu_ns,
        opt(stamp.cause)
    );
    if let Some(tag) = &stamp.request_id {
        out.push_str(",\"request_id\":");
        out.push_str(&Value::String(tag.to_string()).to_string());
    }
    out.push_str(",\"op\":");
    out.push_str(&opt(stamp.op));
    for (k, v) in fields {
        out.push(',');
        out.push_str(&Value::String(k).to_string());
        out.push(':');
        out.push_str(&v.to_string());
    }
    out.push('}');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn stamp(cause: Option<u64>) -> Stamp {
        Stamp { t_ms: 7, icount: 100, emu_ns: 2000, cause, request_id: None, op: None }
    }

    fn fields(v: Value) -> Map<String, Value> {
        match v {
            Value::Object(m) => m,
            _ => Map::new(),
        }
    }

    #[test]
    fn entries_are_numbered_without_gaps_and_track_the_stop_and_state() {
        let mut j = Journal::new(16);
        assert_eq!(j.append(Kind::Started, stamp(None), Map::new()), Some(1));
        assert_eq!(j.state_seq, 0, "started changes nothing");
        assert_eq!(j.append(Kind::Paused, stamp(None), fields(json!({"reason": "startup"}))), Some(2));
        assert_eq!((j.stop_seq, j.state_seq), (Some(2), 2));
        assert_eq!(j.append(Kind::BreakpointSet, stamp(Some(5)), Map::new()), Some(3));
        assert_eq!(j.state_seq, 2, "a breakpoint is debugger configuration");
        assert_eq!(j.append(Kind::Resumed, stamp(Some(6)), Map::new()), Some(4));
        assert_eq!((j.stop_seq, j.state_seq), (None, 4));
        let seqs: Vec<u64> = j.after(0).unwrap().map(|e| e.seq).collect();
        assert_eq!(seqs, [1, 2, 3, 4]);
    }

    #[test]
    fn an_entry_s_json_has_the_stamp_first_and_its_fields() {
        let mut j = Journal::new(4);
        let s = Stamp { request_id: Some(Arc::from("my \"id\"")), op: Some(3), ..stamp(Some(4)) };
        j.append(Kind::MemoryWritten, s, fields(json!({"addr": "02000", "len": 2})));
        let e = j.get(1).unwrap();
        assert!(e.json.starts_with(r#"{"seq":1,"type":"memory_written","t_ms":7,"icount":100,"emu_ns":2000,"cause":4,"#), "{}", e.json);
        let v = e.value();
        assert_eq!(v["request_id"], "my \"id\"");
        assert_eq!((v["op"].as_u64(), v["addr"].as_str(), v["len"].as_u64()), (Some(3), Some("02000"), Some(2)));
    }

    #[test]
    fn eviction_keeps_the_newest_and_reports_what_is_gone() {
        let mut j = Journal::new(8);
        for _ in 0..20 {
            j.append(Kind::InputQueued, stamp(None), Map::new());
        }
        assert_eq!((j.first(), j.last()), (13, 20));
        assert_eq!(j.after(5).err(), Some(Gone { first: 13, last: 20 }));
        assert_eq!(j.after(12).unwrap().count(), 8, "nothing after 12 is missing");
        assert!(j.get(12).is_none() && j.get(13).is_some() && j.get(21).is_none());
        // A search from an evicted number starts at the oldest kept.
        assert_eq!(j.find_after(1, &Filter::default()).map(|e| e.seq), Some(13));
    }

    #[test]
    fn filters_pick_by_kind_op_and_cause_and_shutdown_matches_all() {
        let mut j = Journal::new(16);
        j.append(Kind::Resumed, Stamp { op: Some(9), ..stamp(Some(9)) }, Map::new());
        j.append(Kind::Paused, Stamp { op: Some(9), ..stamp(Some(9)) }, Map::new());
        j.append(Kind::MemoryWritten, stamp(Some(11)), Map::new());
        let paused = Filter { kinds: Some(vec![Kind::Paused]), ..Default::default() };
        assert_eq!(j.find_after(0, &paused).map(|e| e.seq), Some(2));
        let by_cause = Filter { cause: Some(11), ..Default::default() };
        assert_eq!(j.find_after(0, &by_cause).map(|e| e.seq), Some(3));
        let by_op = Filter { op: Some(9), ..Default::default() };
        assert_eq!(j.find_after(1, &by_op).map(|e| e.seq), Some(2));
        let none = Filter { cause: Some(99), ..Default::default() };
        assert!(j.find_after(0, &none).is_none());
        j.append(Kind::Shutdown, stamp(None), Map::new());
        assert_eq!(j.find_after(0, &none).map(|e| e.kind), Some(Kind::Shutdown));
        assert_eq!(j.append(Kind::Paused, stamp(None), Map::new()), None, "nothing after the shutdown");
    }

    #[test]
    fn operations_end_once_and_old_ones_are_gone() {
        let mut j = Journal::new(16);
        j.begin_op(5, OpKind::Step, 1, Some(Arc::from("mine")));
        j.supersede_op(5, 6);
        assert!(!j.end_op(5, OpState::Stopped, Some(3)), "a superseded op stays superseded");
        let OpLookup::Found(op) = j.op(5) else { panic!("no op") };
        assert_eq!((op.state, op.superseded_by), (OpState::Superseded, Some(6)));
        assert_eq!(j.op_tag(5).as_deref(), Some("mine"));
        for req in 10..10 + OPS_CAPACITY as u64 {
            j.begin_op(req, OpKind::Input, 1, None);
            j.end_op(req, OpState::Done, Some(2));
        }
        assert!(matches!(j.op(5), OpLookup::Gone), "the first to end goes first");
        assert!(matches!(j.op(10), OpLookup::Found(_)));
        assert!(matches!(j.op(1 << 40), OpLookup::Unknown));
        assert!(matches!(j.op(0), OpLookup::Unknown), "0 is never an operation");
        assert_eq!(j.ops_len(), OPS_CAPACITY);
    }

    #[test]
    fn a_pending_operation_is_never_evicted() {
        let mut j = Journal::new(16);
        j.begin_op(1, OpKind::Input, 1, Some(Arc::from("old")));
        j.begin_op(2, OpKind::Resume, 1, None);
        for req in 10..10 + 3 * OPS_CAPACITY as u64 {
            j.begin_op(req, OpKind::Step, 1, None);
            j.supersede_op(req, req + 1);
        }
        assert!(matches!(j.op(1), OpLookup::Found(op) if op.state == OpState::Pending));
        assert!(matches!(j.op(2), OpLookup::Found(op) if op.state == OpState::Pending));
        assert_eq!(j.op_tag(1).as_deref(), Some("old"), "its request's ID stays with it");
        assert_eq!(j.ops_len(), OPS_CAPACITY + 2, "the ended ones are bounded");
        assert!(j.end_op(1, OpState::Done, Some(5)), "it still ends");
        assert!(matches!(j.op(10), OpLookup::Gone));
    }

    #[test]
    fn every_kind_has_a_name_that_parses_back() {
        for &k in Kind::ALL {
            assert_eq!(Kind::parse(k.name()), Some(k));
        }
        assert_eq!(Kind::parse("video_mode"), None);
    }
}
