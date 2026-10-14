//! axum HTTP/WebSocket front end for the debug interface. Runs on its own
//! thread with a private tokio runtime; everything that needs emulator
//! state is forwarded to the main thread as a `Cmd`, and everything that
//! waits for the machine waits here, on the journal (`journal.rs`).
//!
//! Every request gets a number (`req`, the `X-Debug-Req` header and `"req"`
//! in JSON object bodies), may carry the client's `X-Request-Id` (echoed,
//! and copied into the journal entries it causes) and `If-State-Seq`
//! (carried out only if the machine is still stopped in that state).

use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post, put};
use base64::Engine;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::sync::{broadcast, oneshot};

use super::journal::{self, Filter, Journal, Kind, OpKind, OpLookup, OpState};
use super::trace::{self, TraceEntry};
use super::{Answer, Cmd, InputEvent, Meta, Reply, Request, Shared, TraceQuery, stop_fields};
use crate::video;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
/// The debugger page, a single file with its styles and scripts.
const UI: &str = include_str!("ui.html");
const INPUT_WAIT_TIMEOUT: Duration = Duration::from_secs(120);
/// The longest a long poll (`wait=`) waits.
const MAX_WAIT: Duration = Duration::from_secs(600);

#[derive(Clone)]
struct AppState {
    tx: mpsc::Sender<Request>,
    shared: Arc<Shared>,
}

/// Start the server on `addr`, and return the address it listens on. With
/// port 0 the system picks a free port, and the returned address has it.
pub fn spawn(addr: SocketAddr, tx: mpsc::Sender<Request>, shared: Arc<Shared>) -> Result<SocketAddr, String> {
    // Bind synchronously so a port conflict is reported at startup rather
    // than silently in the background thread.
    let listener = std::net::TcpListener::bind(addr).map_err(|e| format!("debug server: cannot bind {}: {}", addr, e))?;
    let bound = listener.local_addr().map_err(|e| format!("debug server: no local address for {}: {}", addr, e))?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    std::thread::Builder::new()
        .name("debug-server".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("debug server runtime");
            rt.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).expect("debug listener");
                let app = router(AppState { tx, shared });
                if let Err(e) = axum::serve(listener, app).await {
                    eprintln!("[DEBUG] server error: {}", e);
                }
            });
        })
        .map_err(|e| e.to_string())?;
    Ok(bound)
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(root))
        .route("/ui", get(ui))
        .route("/api", get(help))
        .route("/api/version", get(version))
        .route("/api/journal", get(journal_get))
        .route("/api/ops/{req}", get(op_get))
        .route("/api/status", get(status))
        .route("/api/stats", get(stats))
        .route("/api/profile", get(profile_get).post(profile_post))
        .route("/api/screenshot", get(screenshot))
        .route("/api/screen/text", get(screen_text))
        .route("/api/trace", get(trace_get).post(trace_post))
        .route("/api/log", get(log_get))
        .route("/api/ports", get(ports_get).post(ports_post))
        .route("/api/input", axum::routing::delete(input_clear))
        .route("/api/input/{kind}", post(input_post))
        .route("/api/drive", get(drives))
        .route("/api/drive/swap", post(swap_images))
        .route("/api/drive/sync", post(sync_shared))
        .route("/api/drive/{letter}", put(mount).post(mount).delete(unmount))
        .route("/api/state/{action}", post(state_file))
        .route("/api/speed", post(speed))
        .route("/api/control/{action}", post(control))
        .route("/api/control/wait", get(wait_pause))
        .route("/api/registers", get(regs_get).put(regs_put))
        .route("/api/registers/fpu", get(fpu_get))
        .route("/api/memory", get(mem_get).put(mem_put))
        .route("/api/disasm", get(disasm))
        .route("/api/breakpoints", get(bp_list).post(bp_add).delete(bp_remove))
        .route("/api/watchpoints", get(wp_list).post(wp_add).delete(wp_remove))
        .route("/api/ivt", get(ivt))
        .route("/api/gdt", get(gdt))
        .route("/api/ldt", get(ldt))
        .route("/api/idt", get(idt))
        .route("/api/tss", get(tss))
        .route("/api/pagewalk", get(pagewalk))
        .route("/api/xms", get(xms))
        .route("/api/net", get(net))
        .route("/api/serial", get(serial))
        .route("/api/printer", get(printer))
        .route("/api/printer/eject", post(printer_eject))
        .route("/api/gus", get(gus))
        .route("/api/sound", get(sound))
        .route("/api/awe32", get(awe32))
        .route("/api/exceptions", get(exceptions))
        .route("/ws/trace", get(ws_trace))
        .route("/ws/events", get(ws_events))
        .route("/ws/screen", get(ws_screen))
        .route("/ws/heat", get(ws_heat))
        .route("/api/heat", get(heat_get))
        .route("/ws/audio", get(ws_audio))
        .route("/ws/input", get(ws_input))
        .layer(middleware::from_fn_with_state(state.clone(), request_ids))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Request numbers, tags and preconditions
// ---------------------------------------------------------------------------

/// What a request carries to the emulator thread besides its command.
#[derive(Clone, Debug)]
struct Ctx {
    req: u64,
    tag: Option<Arc<str>>,
    if_state: Option<u64>,
}

tokio::task_local! {
    static CTX: Ctx;
}

impl Ctx {
    /// The context of the HTTP request being answered.
    fn current() -> Option<Ctx> {
        CTX.try_with(Ctx::clone).ok()
    }
}

/// Counts a request or stream in flight, which the shutdown waits for.
struct Busy(Arc<Shared>);

impl Busy {
    fn new(shared: &Arc<Shared>) -> Self {
        shared.busy.fetch_add(1, Ordering::AcqRel);
        Busy(shared.clone())
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        self.0.busy.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A client's request ID: 1 to 128 printable ASCII characters.
fn request_id(tag: &str) -> Result<Arc<str>, ApiError> {
    if (1..=128).contains(&tag.len()) && tag.bytes().all(|b| (0x20..0x7F).contains(&b)) {
        Ok(Arc::from(tag))
    } else {
        Err(bad("X-Request-Id: 1 to 128 printable ASCII characters"))
    }
}

/// The context of a request from its headers.
fn context(req: u64, headers: &HeaderMap) -> Result<Ctx, ApiError> {
    for name in ["x-request-id", "if-state-seq"] {
        if headers.get_all(name).iter().nth(1).is_some() {
            return Err(bad(format!("{}: one header, not several", name)));
        }
    }
    let tag = match headers.get("x-request-id") {
        Some(v) => Some(request_id(v.to_str().map_err(|_| bad("X-Request-Id: printable ASCII"))?)?),
        None => None,
    };
    let if_state = match headers.get("if-state-seq") {
        Some(v) => Some(
            v.to_str()
                .ok()
                .and_then(|v| v.trim().parse::<u64>().ok())
                .ok_or_else(|| bad("If-State-Seq: a journal sequence number"))?,
        ),
        None => None,
    };
    Ok(Ctx { req, tag, if_state })
}

/// Number every request, take its `X-Request-Id` and `If-State-Seq`, and
/// put the number (and the ID) on the response: `X-Debug-Req`, and `"req"`
/// in a JSON object body.
async fn request_ids(State(s): State<AppState>, request: axum::extract::Request, next: Next) -> Response {
    let _busy = Busy::new(&s.shared);
    let req = s.shared.next_req.fetch_add(1, Ordering::Relaxed);
    let tag = request.headers().get("x-request-id").cloned();
    let mut response = match context(req, request.headers()) {
        Ok(ctx) => CTX.scope(ctx, next.run(request)).await,
        Err(e) => json_body(e.status, &e.body, Some(req)),
    };
    // The body is the handler's: its JSON has `"req"` already (`json_body`).
    let headers = response.headers_mut();
    headers.insert("x-debug-req", HeaderValue::from(req));
    if let Some(tag) = tag {
        headers.insert("x-request-id", tag);
    }
    response
}

/// The number of the HTTP request being answered.
fn current_req() -> Option<u64> {
    Ctx::current().map(|c| c.req)
}

/// A JSON response, serialized once. An object gets the request's number
/// first, as `"req"`.
fn json_body(status: StatusCode, v: &Value, req: Option<u64>) -> Response {
    (status, [(header::CONTENT_TYPE, "application/json")], json_bytes(v, req, 256)).into_response()
}

/// The bytes of `json_body`, in a buffer reserved for `capacity` of them.
fn json_bytes(v: &Value, req: Option<u64>, capacity: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(capacity);
    match (req, v) {
        (Some(req), Value::Object(map)) => {
            out.extend_from_slice(format!("{{\"req\":{}", req).as_bytes());
            for (k, v) in map.iter().filter(|(k, _)| k.as_str() != "req") {
                out.push(b',');
                // Writing to a Vec doesn't fail.
                let _ = serde_json::to_writer(&mut out, k);
                out.push(b':');
                let _ = serde_json::to_writer(&mut out, v);
            }
            out.push(b'}');
        }
        _ => {
            let _ = serde_json::to_writer(&mut out, v);
        }
    }
    out
}

/// `json_body` for the request being answered.
fn json_ok(v: &Value) -> Response {
    json_body(StatusCode::OK, v, current_req())
}

/// Refuse `If-State-Seq` on an endpoint the server answers itself: there
/// is no machine state to hold it against.
fn server_only() -> Result<(), ApiError> {
    match Ctx::current().and_then(|c| c.if_state) {
        Some(_) => Err(bad("If-State-Seq applies to requests the emulator carries out, not to this one")),
        None => Ok(()),
    }
}

/// Refuse a cursor (`after`) past the journal's last entry: no entry of
/// this instance has that number, so it is another instance's, and waiting
/// on it would skip entries.
fn check_after(shared: &Shared, after: Option<u64>) -> Result<(), ApiError> {
    let last = shared.journal.lock().unwrap_or_else(|e| e.into_inner()).last();
    match after {
        Some(after) if after > last => Err(ApiError {
            status: StatusCode::BAD_REQUEST,
            body: json!({
                "error": format!("after {} is past the journal's last entry {}: a cursor from another instance?", after, last),
                "last": last,
                "instance": shared.instance_hex(),
            }),
            meta: None,
        }),
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Plumbing
// ---------------------------------------------------------------------------

/// An error reply: its status, its JSON body (with `error`) and, when the
/// emulator answered it, the journal's state then.
struct ApiError {
    status: StatusCode,
    body: Value,
    meta: Option<Meta>,
}

impl ApiError {
    fn new(status: StatusCode, msg: impl Into<String>) -> Self {
        ApiError { status, body: json!({"error": msg.into()}), meta: None }
    }

    fn message(&self) -> String {
        self.body["error"].as_str().unwrap_or("refused").to_string()
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self.meta {
            Some(meta) => json_response(self.status, self.body, &meta),
            None => json_body(self.status, &self.body, current_req()),
        }
    }
}

fn bad(msg: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, msg)
}

fn status_of(code: u16) -> StatusCode {
    StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST)
}

type ApiResult = Result<Response, ApiError>;

/// Put the journal's state on a JSON object (`state_seq`, `stop_seq`, and
/// what the request did: `seq`, `op`), unless it has those already.
fn merge_meta(v: &mut Value, meta: &Meta) {
    if let Value::Object(o) = v {
        o.entry("state_seq").or_insert(meta.state_seq.into());
        o.entry("stop_seq").or_insert(meta.stop_seq.into());
        if let Some(seq) = meta.seq {
            o.entry("seq").or_insert(seq.into());
        }
        if let Some(op) = meta.op {
            o.entry("op").or_insert(op.into());
        }
    }
}

/// The `X-Debug-State-Seq` header of an answer from the emulator.
fn with_meta(mut response: Response, meta: &Meta) -> Response {
    response.headers_mut().insert("x-debug-state-seq", HeaderValue::from(meta.state_seq));
    response
}

fn json_response(status: StatusCode, v: Value, meta: &Meta) -> Response {
    json_response_to(status, v, meta, current_req())
}

/// `json_response` for request `req`, outside the request's task.
fn json_response_to(status: StatusCode, mut v: Value, meta: &Meta, req: Option<u64>) -> Response {
    merge_meta(&mut v, meta);
    with_meta(json_body(status, &v, req), meta)
}

fn unexpected() -> ApiError {
    ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "unexpected reply")
}

fn shutting_down() -> ApiError {
    ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "the emulator has shut down")
}

fn gone_op(req: u64) -> ApiError {
    ApiError::new(StatusCode::GONE, format!("operation {} is gone from the journal", req))
}

/// 410: some of the entries after `after` were evicted.
fn gone_entries(after: u64, gone: &journal::Gone) -> ApiError {
    let body = json!({
        "error": format!("entries after {} are gone", after),
        "gone": true,
        "first": gone.first,
        "last": gone.last,
    });
    ApiError { status: StatusCode::GONE, body, meta: None }
}

fn unknown_op(req: u64) -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, format!("no operation {}", req))
}

impl AppState {
    /// A context for a request outside HTTP (a `/ws/input` message).
    fn context(&self, tag: Option<Arc<str>>, if_state: Option<u64>) -> Ctx {
        Ctx { req: self.shared.next_req.fetch_add(1, Ordering::Relaxed), tag, if_state }
    }

    async fn call(&self, cmd: Cmd, timeout: Duration) -> Result<(Reply, Meta), ApiError> {
        let ctx = Ctx::current().unwrap_or_else(|| self.context(None, None));
        self.call_with(&ctx, cmd, timeout).await
    }

    async fn call_with(&self, ctx: &Ctx, cmd: Cmd, timeout: Duration) -> Result<(Reply, Meta), ApiError> {
        if self.shared.closed.load(Ordering::Acquire) {
            return Err(shutting_down());
        }
        let (reply, rx) = oneshot::channel();
        let request = Request { cmd, reply, req: ctx.req, tag: ctx.tag.clone(), if_state: ctx.if_state };
        self.tx.send(request).map_err(|_| shutting_down())?;
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Answer { reply: Reply::Error(code, msg), meta })) => {
                Err(ApiError { status: status_of(code), body: json!({"error": msg}), meta: Some(meta) })
            }
            Ok(Ok(Answer { reply: Reply::ErrorJson(code, body), meta })) => {
                Err(ApiError { status: status_of(code), body, meta: Some(meta) })
            }
            Ok(Ok(Answer { reply, meta })) => Ok((reply, meta)),
            Ok(Err(_)) if self.shared.closed.load(Ordering::Acquire) => Err(shutting_down()),
            Ok(Err(_)) => Err(ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "request dropped")),
            Err(_) => Err(ApiError::new(StatusCode::GATEWAY_TIMEOUT, "timed out waiting for the emulator loop")),
        }
    }

    /// A command whose JSON reply is the response.
    async fn call_json(&self, cmd: Cmd, timeout: Duration) -> ApiResult {
        match self.call(cmd, timeout).await? {
            (Reply::Json(v), meta) => Ok(json_response(StatusCode::OK, v, &meta)),
            _ => Err(unexpected()),
        }
    }

    /// The journal's state now, with what a request did.
    fn meta_now(&self, seq: Option<u64>, op: Option<u64>) -> Meta {
        let j = self.shared.journal.lock().unwrap_or_else(|e| e.into_inner());
        Meta { state_seq: j.state_seq, stop_seq: j.stop_seq, last: j.last(), seq, op }
    }
}

fn parse_body(body: &Bytes) -> Result<Value, ApiError> {
    if body.is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    serde_json::from_slice(body).map_err(|e| bad(format!("invalid JSON body: {}", e)))
}

fn from_value<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, ApiError> {
    serde_json::from_value(v).map_err(|e| bad(with_field_hint(e.to_string())))
}

/// serde's "unknown field `modifiers`, expected one of `key`, ..., `mods`"
/// (or "unknown variant") with "(did you mean `mods`?)" added when one of
/// the names it lists is close to the unknown one: the same first three
/// letters, or two edits apart.
fn with_field_hint(msg: String) -> String {
    let Some(rest) = msg.strip_prefix("unknown field `").or_else(|| msg.strip_prefix("unknown variant `")) else {
        return msg;
    };
    let Some((unknown, expected)) = rest.split_once('`') else {
        return msg;
    };
    let unknown = unknown.to_ascii_lowercase();
    let prefix = |a: &str, b: &str| a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count();
    let best = expected
        .split('`')
        .skip(1)
        .step_by(2)
        .map(|field| (field, prefix(&unknown, field), edit_distance(&unknown, field)))
        .filter(|&(_, p, d)| p >= 3 || (d <= 2 && d < unknown.len()))
        .min_by_key(|&(_, p, d)| (std::cmp::Reverse(p.min(3)), d));
    match best {
        Some((field, ..)) => format!("{} (did you mean `{}`?)", msg, field),
        None => msg,
    }
}

/// Levenshtein distance, for the short field names above.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diag = row[0];
        row[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let next = (diag + usize::from(ca != cb)).min(row[j] + 1).min(row[j + 1] + 1);
            diag = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}

fn text_response(s: String) -> Response {
    ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], s).into_response()
}

fn encode_png(frame: &video::Frame) -> Result<Vec<u8>, String> {
    rust_dos::capture::png::encode(frame)
}

// ---------------------------------------------------------------------------
// Waiting on the journal
// ---------------------------------------------------------------------------

/// Wait until `f` finds what it looks for in the journal, or until
/// `deadline` (None).
async fn wait_journal<T>(shared: &Shared, deadline: tokio::time::Instant, mut f: impl FnMut(&Journal) -> Option<T>) -> Option<T> {
    // Subscribed before looking, so that no entry slips in between.
    let mut seq = shared.journal_seq.subscribe();
    loop {
        seq.borrow_and_update();
        let found = {
            let j = shared.journal.lock().unwrap_or_else(|e| e.into_inner());
            f(&j)
        };
        if found.is_some() {
            return found;
        }
        if !matches!(tokio::time::timeout_at(deadline, seq.changed()).await, Ok(Ok(()))) {
            return None;
        }
    }
}

fn deadline(wait: Duration) -> tokio::time::Instant {
    tokio::time::Instant::now() + wait.min(MAX_WAIT)
}

/// How an operation ended: its number (the one it was superseded by, when
/// followed), its record and the entry that ended it.
struct OpEnd {
    req: u64,
    op: journal::Op,
    end: Option<Value>,
}

/// Operation `req` once it ended, None while it is pending. With `follow`,
/// a superseded operation is followed to the one that took over.
fn op_end(j: &Journal, mut req: u64, follow: bool) -> Option<Result<OpEnd, ApiError>> {
    loop {
        let op = match j.op(req) {
            OpLookup::Found(op) => op.clone(),
            OpLookup::Gone => return Some(Err(gone_op(req))),
            OpLookup::Unknown => return Some(Err(unknown_op(req))),
        };
        match (op.state, op.superseded_by) {
            (OpState::Pending, _) => return None,
            (OpState::Superseded, Some(by)) if follow => req = by,
            _ => {
                // Kept with the operation: the ring may have dropped it.
                let end = op.end.as_deref().and_then(|e| serde_json::from_str(e).ok());
                return Some(Ok(OpEnd { req, op, end }));
            }
        }
    }
}

/// An operation as `/api/ops` shows it.
fn op_json(req: u64, end: &OpEnd) -> Value {
    let op = &end.op;
    let stop = end.end.as_ref().filter(|e| e["type"] == "paused").cloned();
    json!({
        "op": req,
        "kind": op.kind.name(),
        "state": op.state.name(),
        "start_seq": op.start_seq,
        "end_seq": op.end_seq,
        "superseded_by": op.superseded_by,
        "stop": stop,
        "end": end.end,
        "error": op.error.as_ref().map(|(_, e)| e),
    })
}

/// The reply to a request that waited for its operation `op` (started
/// with entry `seq`) to end, as replies have it since before the journal:
/// the stop it ended with, a `run`'s program, delivered input, or the
/// error that ended it.
async fn finish_op(s: &AppState, op: u64, seq: Option<u64>, timeout: Duration) -> ApiResult {
    // The reply's shape is the operation's kind's, whoever waits for it.
    let kind = match s.shared.journal.lock().unwrap_or_else(|e| e.into_inner()).op(op) {
        OpLookup::Found(found) => Some(found.kind),
        _ => None,
    };
    let ended = wait_journal(&s.shared, deadline(timeout), |j| op_end(j, op, true)).await;
    let meta = s.meta_now(seq, Some(op));
    let Some(ended) = ended else {
        let error = match kind {
            Some(OpKind::Input) => "timed out waiting for the input to be delivered",
            Some(OpKind::Run) => "timed out waiting for the program to start",
            _ => "timed out waiting for the stop",
        };
        return Err(ApiError { status: StatusCode::GATEWAY_TIMEOUT, body: json!({"error": error, "op": op}), meta: Some(meta) });
    };
    let end = ended?;
    let entry = end.end.clone().unwrap_or(Value::Null);
    let refused = |status: StatusCode, mut body: Value| {
        body["op"] = op.into();
        Err(ApiError { status, body, meta: Some(meta) })
    };
    match end.op.state {
        OpState::Stopped => {
            let mut reply = stop_fields(&entry);
            if end.req != op {
                reply["superseded_by"] = end.req.into();
            }
            Ok(json_response(StatusCode::OK, reply, &meta))
        }
        OpState::Done if end.op.kind == OpKind::Input => Ok(json_response(StatusCode::OK, json!({"ok": true, "done": true}), &meta)),
        OpState::Done => Ok(json_response(StatusCode::OK, json!({"ok": true, "program": entry["program"]}), &meta)),
        OpState::Failed => {
            let (status, error) = end.op.error.clone().unwrap_or((500, "failed".into()));
            refused(status_of(status), json!({"error": error}))
        }
        OpState::Cancelled => match entry["type"].as_str() {
            Some("input_cleared") => Ok(json_response(StatusCode::OK, json!({"ok": false, "cancelled": true}), &meta)),
            Some("debugger_reset") => refused(StatusCode::CONFLICT, json!({"error": "a save state was loaded"})),
            _ => refused(StatusCode::SERVICE_UNAVAILABLE, json!({"error": "the emulator shut down", "shutdown": entry})),
        },
        // Not followed: the superseding operation was gone.
        OpState::Superseded | OpState::Pending => Err(unexpected()),
    }
}

// ---------------------------------------------------------------------------
// HTTP handlers
// ---------------------------------------------------------------------------

async fn help() -> ApiResult {
    server_only()?;
    Ok(text_response(HELP.to_string()))
}

/// A browser gets the debugger page; everything else (curl, agents) the
/// endpoint reference.
async fn root(headers: HeaderMap) -> ApiResult {
    let html = headers
        .get(header::ACCEPT)
        .and_then(|a| a.to_str().ok())
        .is_some_and(|a| a.contains("text/html"));
    if html {
        server_only()?;
        Ok(Redirect::to("/ui").into_response())
    } else {
        help().await
    }
}

async fn ui() -> ApiResult {
    server_only()?;
    Ok(([(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")], UI).into_response())
}

async fn version(State(s): State<AppState>) -> ApiResult {
    server_only()?;
    Ok(json_ok(&json!({
        "name": "rust-dos",
        "version": env!("CARGO_PKG_VERSION"),
        "build": rust_dos::build_info::json(),
        "api_version": journal::API_VERSION,
        "instance": s.shared.instance_hex(),
        "features": journal::FEATURES,
    })))
}

#[derive(Deserialize)]
struct JournalQuery {
    after: Option<u64>,
    /// Entry types, comma-separated.
    #[serde(rename = "type")]
    kinds: Option<String>,
    op: Option<u64>,
    cause: Option<u64>,
    limit: Option<usize>,
    /// Milliseconds to wait for a matching entry.
    wait: Option<u64>,
    /// Go on from the oldest entry kept when some after `after` are gone.
    #[serde(default)]
    allow_gap: bool,
}

/// The entries after `after` that `filter` takes, up to `limit`, and the
/// `after` of the next page.
fn journal_page(j: &Journal, after: u64, allow_gap: bool, filter: &Filter, limit: usize) -> Result<(Vec<Arc<str>>, u64), journal::Gone> {
    let from = match j.after(after) {
        Ok(_) => after,
        Err(gone) if allow_gap => gone.first - 1,
        Err(gone) => return Err(gone),
    };
    let mut entries = Vec::new();
    let mut next = j.last().max(from);
    for e in j.after(from)? {
        if filter.matches(e) {
            if entries.len() == limit {
                next = e.seq - 1;
                break;
            }
            entries.push(e.json.clone());
        }
    }
    Ok((entries, next))
}

/// The journal's entries after `after` that the filter takes, up to
/// `limit`; with `wait`, once there is one.
async fn journal_get(State(s): State<AppState>, Query(q): Query<JournalQuery>) -> ApiResult {
    server_only()?;
    check_after(&s.shared, q.after)?;
    let kinds = match &q.kinds {
        None => None,
        Some(list) => Some(
            list.split(',')
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(|t| Kind::parse(t).ok_or_else(|| bad(format!("unknown entry type '{}'", t))))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    };
    let filter = Filter { kinds, op: q.op, cause: q.cause };
    let limit = q.limit.unwrap_or(200).clamp(1, journal::CAPACITY);
    let after = q.after.unwrap_or(0);
    let wait = Duration::from_millis(q.wait.unwrap_or(0));
    let found = wait_journal(&s.shared, deadline(wait), |j| match journal_page(j, after, q.allow_gap, &filter, limit) {
        Ok((entries, _)) if entries.is_empty() => None,
        other => Some(other),
    })
    .await;
    let j = s.shared.journal.lock().unwrap_or_else(|e| e.into_inner());
    let (result, timed_out) = match found {
        Some(r) => (r, false),
        None => (journal_page(&j, after, q.allow_gap, &filter, limit), wait > Duration::ZERO),
    };
    let (entries, next) = match result {
        Ok(page) => page,
        Err(gone) => return Err(gone_entries(after, &gone)),
    };
    // The entries' JSON as it is, without parsing it again.
    let mut body = format!(
        r#"{{"req":{},"instance":"{}","first":{},"last":{},"state_seq":{},"stop_seq":{},"next":{},"timed_out":{},"entries":["#,
        current_req().unwrap_or(0),
        s.shared.instance_hex(),
        j.first(),
        j.last(),
        j.state_seq,
        j.stop_seq.map_or("null".to_string(), |s| s.to_string()),
        next,
        timed_out,
    );
    drop(j);
    body.reserve(entries.iter().map(|e| e.len() + 1).sum::<usize>() + 2);
    for (i, e) in entries.iter().enumerate() {
        if i > 0 {
            body.push(',');
        }
        body.push_str(e);
    }
    body.push_str("]}");
    Ok(([(header::CONTENT_TYPE, "application/json")], body).into_response())
}

#[derive(Deserialize)]
struct OpQuery {
    /// Milliseconds to wait for it to end.
    wait: Option<u64>,
}

/// An operation: its state, and the entries that started and ended it.
async fn op_get(State(s): State<AppState>, Path(req): Path<u64>, Query(q): Query<OpQuery>) -> ApiResult {
    server_only()?;
    let wait = Duration::from_millis(q.wait.unwrap_or(0));
    let end = match wait_journal(&s.shared, deadline(wait), |j| op_end(j, req, false)).await {
        Some(end) => end?,
        // Still pending.
        None => {
            let j = s.shared.journal.lock().unwrap_or_else(|e| e.into_inner());
            match j.op(req) {
                OpLookup::Found(op) => OpEnd { req, op: op.clone(), end: None },
                OpLookup::Gone => return Err(gone_op(req)),
                OpLookup::Unknown => return Err(unknown_op(req)),
            }
        }
    };
    Ok(json_ok(&op_json(req, &end)))
}

async fn status(State(s): State<AppState>) -> ApiResult {
    s.call_json(Cmd::Status, DEFAULT_TIMEOUT).await
}

async fn stats(State(s): State<AppState>) -> ApiResult {
    s.call_json(Cmd::Stats, DEFAULT_TIMEOUT).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfilePost {
    enabled: Option<bool>,
    hz: Option<u32>,
    /// `sleep` or `spin`.
    timer: Option<String>,
    duration_ms: Option<u64>,
}

async fn profile_post(State(s): State<AppState>, body: Bytes) -> ApiResult {
    use rust_dos::profile::sampler::Timer;
    use rust_dos::profile::session::{MAX_HZ, MIN_HZ};
    let p: ProfilePost = from_value(parse_body(&body)?)?;
    let timer = match p.timer.as_deref() {
        None => None,
        Some("sleep") => Some(Timer::Sleep),
        Some("spin") => Some(Timer::Spin),
        Some(t) => return Err(bad(format!("timer '{}': sleep or spin", t))),
    };
    if let Some(hz) = p.hz.filter(|hz| !(MIN_HZ..=MAX_HZ).contains(hz)) {
        return Err(bad(format!("hz {}: from {} to {}", hz, MIN_HZ, MAX_HZ)));
    }
    if p.duration_ms == Some(0) {
        return Err(bad("duration_ms 0: leave it out to profile until stopped"));
    }
    let control = super::profile::Control { enabled: p.enabled, hz: p.hz, timer, duration_ms: p.duration_ms };
    s.call_json(Cmd::ProfileControl(control), DEFAULT_TIMEOUT).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileQuery {
    /// `json` (the default), `folded` or `text`.
    format: Option<String>,
    top: Option<usize>,
    zone: Option<String>,
    bucket: Option<u32>,
    disasm: Option<usize>,
    timeline: Option<bool>,
    /// Wait up to this many milliseconds for a profile with a duration to
    /// end.
    wait_ms: Option<u64>,
}

async fn profile_get(State(s): State<AppState>, Query(q): Query<ProfileQuery>) -> ApiResult {
    use super::profile::Format;
    let format = match q.format.as_deref() {
        None | Some("json") => Format::Json,
        Some("folded") => Format::Folded,
        Some("text") => Format::Text,
        Some(f) => return Err(bad(format!("format '{}': json, folded or text", f))),
    };
    if let Some(wait) = q.wait_ms {
        let until = std::time::Instant::now() + Duration::from_millis(wait).min(MAX_WAIT);
        loop {
            let (Reply::Json(status), _) = s.call(Cmd::ProfileStatus, DEFAULT_TIMEOUT).await? else {
                return Err(unexpected());
            };
            if status["running"] != json!(true) || std::time::Instant::now() >= until {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    let defaults = rust_dos::profile::report::Query::default();
    let query = rust_dos::profile::report::Query {
        top: q.top.unwrap_or(defaults.top).min(1000),
        zone: q.zone,
        bucket: q.bucket.unwrap_or(defaults.bucket),
        disasm: q.disasm.unwrap_or(defaults.disasm).min(32),
        timeline: q.timeline.unwrap_or(false),
    };
    let (Reply::Json(v), meta) = s.call(Cmd::ProfileGet { query, format }, DEFAULT_TIMEOUT).await? else {
        return Err(unexpected());
    };
    match format {
        Format::Json => Ok(json_response(StatusCode::OK, v, &meta)),
        Format::Folded | Format::Text => {
            Ok(with_meta(text_response(v["text"].as_str().unwrap_or_default().to_string()), &meta))
        }
    }
}

#[derive(Deserialize)]
struct FormatQuery {
    format: Option<String>,
}

async fn screenshot(State(s): State<AppState>, Query(q): Query<FormatQuery>) -> ApiResult {
    let (Reply::Frame(frame), meta) = s.call(Cmd::Screenshot, DEFAULT_TIMEOUT).await? else {
        return Err(unexpected());
    };
    let response = match q.format.as_deref().unwrap_or("png") {
        "png" => {
            let png = tokio::task::spawn_blocking(move || encode_png(&frame))
                .await
                .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
                .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e))?;
            ([(header::CONTENT_TYPE, "image/png")], png).into_response()
        }
        "raw" => (
            [
                (header::CONTENT_TYPE, "application/octet-stream".to_string()),
                (header::HeaderName::from_static("x-width"), frame.width.to_string()),
                (header::HeaderName::from_static("x-height"), frame.height.to_string()),
                (header::HeaderName::from_static("x-pixel-format"), "RGB24".to_string()),
            ],
            frame.rgb,
        )
            .into_response(),
        f => return Err(bad(format!("unknown format '{}' (png, raw)", f))),
    };
    Ok(with_meta(response, &meta))
}

async fn screen_text(State(s): State<AppState>, Query(q): Query<FormatQuery>) -> ApiResult {
    let (Reply::Json(v), meta) = s.call(Cmd::ScreenText, DEFAULT_TIMEOUT).await? else {
        return Err(unexpected());
    };
    if q.format.as_deref() == Some("text") {
        let lines: Vec<&str> = v["lines"].as_array().into_iter().flatten().filter_map(|l| l.as_str()).collect();
        return Ok(with_meta(text_response(lines.join("\n") + "\n"), &meta));
    }
    Ok(json_response(StatusCode::OK, v, &meta))
}

async fn trace_get(State(s): State<AppState>, Query(q): Query<TraceQuery>) -> ApiResult {
    let json = q.format.as_deref() == Some("json");
    let (Reply::Trace { entries, next, dropped, truncated }, meta) = s.call(Cmd::TraceQuery(q), DEFAULT_TIMEOUT).await? else {
        return Err(unexpected());
    };
    let req = current_req();
    let body = tokio::task::spawn_blocking(move || format_trace(&entries, json, next, dropped, truncated, &meta, req))
        .await
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let cursor = [
        (header::HeaderName::from_static("x-trace-next"), next.to_string()),
        (header::HeaderName::from_static("x-trace-dropped"), dropped.to_string()),
        (header::HeaderName::from_static("x-trace-truncated"), truncated.to_string()),
    ];
    Ok(with_meta((cursor, body).into_response(), &meta))
}

fn format_trace(entries: &[TraceEntry], json: bool, next: u64, dropped: u64, truncated: bool, meta: &Meta, req: Option<u64>) -> Response {
    if json {
        let out = trace_json(entries, next, dropped, truncated, meta, req);
        (StatusCode::OK, [(header::CONTENT_TYPE, "application/json")], out).into_response()
    } else {
        let mut out = String::with_capacity(entries.len() * 160 + 128);
        out.push_str(trace::TEXT_HEADER);
        out.push('\n');
        for e in entries {
            out.push_str(&e.to_text());
            out.push('\n');
        }
        text_response(out)
    }
}

/// A trace reply as JSON, with `entries` last. The entries are written one
/// at a time: a `Value` for each of a full ring's million entries would take
/// gigabytes.
fn trace_json(entries: &[TraceEntry], next: u64, dropped: u64, truncated: bool, meta: &Meta, req: Option<u64>) -> Vec<u8> {
    let mut head = json!({"count": entries.len(), "next": next, "dropped": dropped, "truncated": truncated});
    merge_meta(&mut head, meta);
    let mut out = json_bytes(&head, req, entries.len() * 450 + 256);
    out.pop(); // the object's closing brace
    out.extend_from_slice(b",\"entries\":[");
    for (i, e) in entries.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        // Writing to a Vec doesn't fail.
        let _ = serde_json::to_writer(&mut out, &e.to_json());
    }
    out.extend_from_slice(b"]}");
    out
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PortsPost {
    enabled: Option<bool>,
    #[serde(default)]
    clear: bool,
    capacity: Option<usize>,
    /// The ports to keep, hex: `3C9` or `3C0-3CF` (all of them to begin with).
    ports: Option<String>,
}

/// A port or a range of them, hex: `3C9` or `3C0-3CF`.
fn port_range(range: &str) -> Result<(u16, u16), ApiError> {
    let hex = |t: &str| u16::from_str_radix(t.trim().trim_start_matches("0x").trim_end_matches(['h', 'H']), 16);
    let (a, b) = range.split_once('-').unwrap_or((range, range));
    match (hex(a), hex(b)) {
        (Ok(a), Ok(b)) => Ok((a, b)),
        _ => Err(bad("ports: hex, as 3C9 or 3C0-3CF")),
    }
}

async fn ports_post(State(s): State<AppState>, body: Bytes) -> ApiResult {
    let p: PortsPost = from_value(parse_body(&body)?)?;
    let ports = p.ports.as_deref().map(port_range).transpose()?;
    s.call_json(Cmd::PortsControl { enabled: p.enabled, clear: p.clear, capacity: p.capacity, ports }, DEFAULT_TIMEOUT).await
}

#[derive(Deserialize)]
struct PortsQuery {
    /// A port or a range of them, hex: `3C9` or `3C0-3CF`.
    port: Option<String>,
    /// `in` or `out`.
    dir: Option<String>,
    last_n: Option<usize>,
}

async fn ports_get(State(s): State<AppState>, Query(q): Query<PortsQuery>) -> ApiResult {
    let (from, to) = match q.port.as_deref() {
        None => (0, 0xFFFF),
        Some(range) => port_range(range)?,
    };
    let write = match q.dir.as_deref() {
        None => None,
        Some("out") => Some(true),
        Some("in") => Some(false),
        Some(_) => return Err(bad("dir: in or out")),
    };
    s.call_json(Cmd::PortsQuery { from, to, write, last_n: q.last_n.unwrap_or(200) }, DEFAULT_TIMEOUT).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TracePost {
    enabled: Option<bool>,
    #[serde(default)]
    clear: bool,
    stream_max: Option<usize>,
    /// Record this many more instructions, then stop.
    count: Option<u64>,
}

async fn trace_post(State(s): State<AppState>, body: Bytes) -> ApiResult {
    let p: TracePost = from_value(parse_body(&body)?)?;
    // A nonzero count turns the trace on and 0 turns it off.
    if let Some((e, n)) = p.enabled.zip(p.count).filter(|&(e, n)| e != (n > 0)) {
        return Err(bad(format!("\"enabled\":{e} contradicts \"count\":{n}")));
    }
    let cmd = Cmd::TraceControl { enabled: p.enabled, clear: p.clear, stream_max: p.stream_max, count: p.count };
    s.call_json(cmd, DEFAULT_TIMEOUT).await
}

#[derive(Deserialize)]
struct LogQuery {
    since_ms: Option<u64>,
    /// Only the lines after line number N (an earlier line's `n`), oldest
    /// first.
    after_n: Option<u64>,
    /// With `after_n`: milliseconds to wait for a line.
    wait: Option<u64>,
    limit: Option<usize>,
    grep: Option<String>,
    format: Option<String>,
}

async fn log_get(State(s): State<AppState>, Query(q): Query<LogQuery>) -> ApiResult {
    server_only()?;
    let limit = q.limit.unwrap_or(500);
    let pick = || -> Vec<super::LogLine> {
        let log = s.shared.log.lock().unwrap_or_else(|e| e.into_inner());
        let keep = |l: &&super::LogLine| {
            q.since_ms.is_none_or(|t| l.t_ms >= t) && q.grep.as_deref().is_none_or(|g| l.line.contains(g))
        };
        match q.after_n {
            // Oldest first from the cursor, so the next page goes on there.
            Some(n) => log.iter().filter(|l| l.n > n).filter(keep).take(limit).cloned().collect(),
            None => {
                let mut v: Vec<_> = log.iter().rev().filter(keep).take(limit).cloned().collect();
                v.reverse();
                v
            }
        }
    };
    let mut lines = pick();
    if lines.is_empty()
        && let (Some(_), Some(ms)) = (q.after_n, q.wait)
    {
        let mut n = s.shared.log_n.subscribe();
        // The shutdown ends the wait too, so that it doesn't hold the exit.
        let mut seq = s.shared.journal_seq.subscribe();
        let until = deadline(Duration::from_millis(ms));
        loop {
            n.borrow_and_update();
            seq.borrow_and_update();
            lines = pick();
            let shut = s.shared.journal.lock().unwrap_or_else(|e| e.into_inner()).shutdown.is_some();
            if !lines.is_empty() || shut {
                break;
            }
            let woke = tokio::time::timeout_at(until, async {
                tokio::select! {
                    r = n.changed() => r.is_ok(),
                    r = seq.changed() => r.is_ok(),
                }
            })
            .await;
            if !matches!(woke, Ok(true)) {
                break;
            }
        }
    }
    if q.format.as_deref() == Some("text") {
        let s: String = lines.iter().map(|l| format!("{:>10} {}\n", l.t_ms, l.line)).collect();
        return Ok(text_response(s));
    }
    Ok(json_ok(&json!({"lines": lines})))
}

#[derive(Deserialize)]
struct WaitQuery {
    wait: Option<bool>,
}

/// Parse an input body: a single event object, an array of events, or
/// `{"events": [...]}`. `default_type` is injected into objects lacking a
/// `type` field (so `/api/input/key` accepts `{"key": "enter"}`).
fn parse_events(v: Value, default_type: Option<&str>) -> Result<Vec<InputEvent>, ApiError> {
    let (list, several) = match v {
        Value::Array(a) => (a, true),
        Value::Object(mut o) if o.contains_key("events") => {
            let Some(Value::Array(a)) = o.remove("events") else {
                return Err(bad("'events' must be an array"));
            };
            // A field beside the events is refused as one in an event is.
            if let Some(field) = o.keys().next() {
                return Err(bad(format!("unknown field `{}` beside `events`", field)));
            }
            (a, true)
        }
        other => (vec![other], false),
    };
    list.into_iter()
        .enumerate()
        .map(|(i, mut ev)| {
            if let (Some(t), Value::Object(o)) = (default_type, &mut ev) {
                o.entry("type").or_insert_with(|| Value::String(t.to_string()));
            }
            from_value(ev).map_err(|e| if several { bad(format!("events[{}]: {}", i, e.message())) } else { e })
        })
        .collect()
}

async fn input_post(
    State(s): State<AppState>,
    Path(kind): Path<String>,
    Query(q): Query<WaitQuery>,
    body: Bytes,
) -> ApiResult {
    let default_type = match kind.as_str() {
        "key" | "type" | "mouse" | "wait" => Some(kind.as_str()),
        "batch" => None,
        _ => return Err(ApiError::new(StatusCode::NOT_FOUND, format!("unknown input kind '{}'", kind))),
    };
    let events = parse_events(parse_body(&body)?, default_type)?;
    let wait = q.wait.unwrap_or(true);
    let ctx = Ctx::current().unwrap_or_else(|| s.context(None, None));
    queue_input(&s, &ctx, events, wait).await
}

/// Queue input events; with `wait`, answer once they are delivered (or
/// cancelled), unless deterministic mode holds them while paused.
async fn queue_input(s: &AppState, ctx: &Ctx, events: Vec<InputEvent>, wait: bool) -> ApiResult {
    let (Reply::Json(v), meta) = s.call_with(ctx, Cmd::Input { events, wait }, DEFAULT_TIMEOUT).await? else {
        return Err(unexpected());
    };
    match meta.op {
        Some(op) if wait && v["held"] != true => finish_op(s, op, meta.seq, INPUT_WAIT_TIMEOUT).await,
        _ => Ok(json_response(StatusCode::OK, v, &meta)),
    }
}

async fn input_clear(State(s): State<AppState>) -> ApiResult {
    s.call_json(Cmd::InputClear, DEFAULT_TIMEOUT).await
}

async fn drives(State(s): State<AppState>) -> ApiResult {
    s.call_json(Cmd::Drives, DEFAULT_TIMEOUT).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MountBody {
    path: String,
    /// floppy, hdd or cdrom
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    read_only: bool,
    /// More images after `path`, which /api/drive/swap (Ctrl+F4) steps
    /// through.
    #[serde(default)]
    images: Vec<String>,
}

async fn swap_images(State(s): State<AppState>) -> ApiResult {
    s.call_json(Cmd::SwapImages, DEFAULT_TIMEOUT).await
}

async fn sync_shared(State(s): State<AppState>) -> ApiResult {
    s.call_json(Cmd::SyncShared, DEFAULT_TIMEOUT).await
}

async fn mount(State(s): State<AppState>, Path(letter): Path<String>, body: Bytes) -> ApiResult {
    let b: MountBody = from_value(parse_body(&body)?)?;
    let cmd = Cmd::Mount {
        drive: letter,
        path: b.path,
        kind: b.kind,
        label: b.label,
        read_only: b.read_only,
        images: b.images,
    };
    s.call_json(cmd, DEFAULT_TIMEOUT).await
}

async fn unmount(State(s): State<AppState>, Path(letter): Path<String>) -> ApiResult {
    s.call_json(Cmd::Unmount { drive: letter }, DEFAULT_TIMEOUT).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StateBody {
    /// The save state file, on the host.
    path: String,
    /// A strict match with this machine: only while paused, and a load
    /// must match its hardware and media and resets the debugger.
    #[serde(default)]
    strict_match: bool,
    /// Record the files on the writable drives (save), or compare them
    /// with the record (load). Off unless asked for.
    #[serde(default)]
    check_files: bool,
    /// A load only: folder drives of the state, by letter, to take as
    /// mounted from these host folders instead of the ones it records.
    #[serde(default)]
    folders: Map<String, Value>,
}

/// A load's `folders`: each drive letter's number and its absolute host folder.
fn state_folders(folders: Map<String, Value>) -> Result<Vec<(u8, String)>, ApiError> {
    let mut out: Vec<(u8, String)> = Vec::new();
    for (letter, folder) in folders {
        let Some(drive) = crate::mount::parse_drive_letter(&letter) else {
            return Err(bad(format!("folders: '{}' isn't a drive letter", letter)));
        };
        // "C" and "c" would name one drive twice.
        if out.iter().any(|(d, _)| *d == drive) {
            return Err(bad(format!("folders: drive {} is named twice", letter.to_ascii_uppercase())));
        }
        let folder = folder.as_str().ok_or_else(|| bad(format!("folders.{}: a host folder's path", letter)))?;
        if !std::path::Path::new(folder).is_absolute() {
            return Err(bad(format!("folders.{}: '{}' isn't an absolute path", letter, folder)));
        }
        out.push((drive, folder.to_string()));
    }
    Ok(out)
}

async fn state_file(State(s): State<AppState>, Path(action): Path<String>, body: Bytes) -> ApiResult {
    let b: StateBody = from_value(parse_body(&body)?)?;
    let (path, strict_match, check_files) = (b.path, b.strict_match, b.check_files);
    match action.as_str() {
        "save" if !b.folders.is_empty() => Err(bad("folders is for a load: a save records the folders as they are")),
        "save" => s.call_json(Cmd::SaveState { path, strict_match, check_files }, DEFAULT_TIMEOUT).await,
        "load" => {
            let folders = state_folders(b.folders)?;
            s.call_json(Cmd::LoadState { path, strict_match, check_files, folders }, DEFAULT_TIMEOUT).await
        }
        _ => Err(ApiError::new(StatusCode::NOT_FOUND, format!("unknown action '{}' (save, load)", action))),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpeedBody {
    /// As `cycles` in the configuration file: auto, max or a number.
    cycles: String,
}

async fn speed(State(s): State<AppState>, body: Bytes) -> ApiResult {
    let b: SpeedBody = from_value(parse_body(&body)?)?;
    s.call_json(Cmd::Speed { cycles: b.cycles }, DEFAULT_TIMEOUT).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlBody {
    count: Option<u64>,
    until: Option<String>,
    until_ms: Option<u64>,
    timeout_ms: Option<u64>,
    /// `run`: the command line, and whether to stop at the program's entry.
    command: Option<String>,
    #[serde(default)]
    stop_at_entry: bool,
    /// `run`: take the command line while the prompt is busy, to run after
    /// what keeps it busy.
    #[serde(default)]
    queue: bool,
    /// step, step_over, run: false answers at once (202) with the
    /// operation, to wait on with /api/ops or /api/control/wait?op=.
    wait: Option<bool>,
}

/// How long `run` waits for its program to start, unless `timeout_ms` says.
const RUN_TIMEOUT: Duration = Duration::from_secs(30);

async fn control(State(s): State<AppState>, Path(action): Path<String>, body: Bytes) -> ApiResult {
    let b: ControlBody = from_value(parse_body(&body)?)?;
    let timeout = b.timeout_ms.map_or(DEFAULT_TIMEOUT, Duration::from_millis);
    let (cmd, timeout) = match action.as_str() {
        "pause" => return s.call_json(Cmd::Pause, DEFAULT_TIMEOUT).await,
        "resume" | "continue" => {
            return s.call_json(Cmd::Resume { until: b.until, until_ms: b.until_ms }, DEFAULT_TIMEOUT).await;
        }
        "reboot_shell" => return s.call_json(Cmd::RebootShell, DEFAULT_TIMEOUT).await,
        "reboot" => return s.call_json(Cmd::Reboot, DEFAULT_TIMEOUT).await,
        "quit" => return s.call_json(Cmd::Quit, DEFAULT_TIMEOUT).await,
        "step" => (Cmd::Step { count: b.count.unwrap_or(1) }, timeout),
        "step_over" => (Cmd::StepOver, timeout),
        "run" => {
            let command = b.command.ok_or_else(|| bad("missing 'command'"))?;
            let timeout = b.timeout_ms.map_or(RUN_TIMEOUT, Duration::from_millis);
            (Cmd::Run { command, stop_at_entry: b.stop_at_entry, queue: b.queue }, timeout)
        }
        _ => {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                format!("unknown action '{}' (pause, resume, step, step_over, run, reboot_shell, reboot, quit)", action),
            ));
        }
    };
    let (Reply::Json(_), meta) = s.call(cmd, DEFAULT_TIMEOUT).await? else {
        return Err(unexpected());
    };
    let op = meta.op.ok_or_else(unexpected)?;
    if b.wait == Some(false) {
        return Ok(json_response(StatusCode::ACCEPTED, json!({"ok": true}), &meta));
    }
    finish_op(&s, op, meta.seq, timeout).await
}

#[derive(Deserialize)]
struct PauseWaitQuery {
    timeout_ms: Option<u64>,
    /// The first stop after this entry.
    after: Option<u64>,
    /// The end of this operation.
    op: Option<u64>,
}

/// Wait for a stop: the first after an entry (`after`), the end of an
/// operation (`op`), or with neither, the stop the machine is in or the
/// next one.
async fn wait_pause(State(s): State<AppState>, Query(q): Query<PauseWaitQuery>) -> ApiResult {
    let timeout = Duration::from_millis(q.timeout_ms.unwrap_or(30_000));
    if let Some(op) = q.op {
        server_only()?;
        return finish_op(&s, op, None, timeout).await;
    }
    let after = match q.after {
        Some(after) => {
            server_only()?;
            check_after(&s.shared, Some(after))?;
            // The first stop after `after` may be among entries evicted.
            let gone = s.shared.journal.lock().unwrap_or_else(|e| e.into_inner()).after(after).err();
            if let Some(gone) = gone {
                return Err(gone_entries(after, &gone));
            }
            after
        }
        None => match s.call(Cmd::WaitPause, DEFAULT_TIMEOUT).await? {
            (Reply::Json(v), meta) if v["paused"] == true => return Ok(json_response(StatusCode::OK, v, &meta)),
            (Reply::Json(_), meta) => meta.last,
            _ => return Err(unexpected()),
        },
    };
    let legacy = q.after.is_none();
    let stops = Filter { kinds: Some(vec![Kind::Paused, Kind::DebuggerReset]), ..Default::default() };
    let found = wait_journal(&s.shared, deadline(timeout), |j| {
        let mut from = after;
        loop {
            let e = j.find_after(from, &stops)?;
            // The stop in a loaded state isn't one the machine ran into,
            // and a debugger reset ends only the form without `after`.
            match e.kind {
                Kind::Paused if legacy && e.value()["reason"] == "load" => from = e.seq,
                Kind::DebuggerReset if !legacy => from = e.seq,
                _ => return Some((e.kind, e.value())),
            }
        }
    })
    .await;
    let meta = s.meta_now(None, None);
    let refused = |status, body| Err(ApiError { status, body, meta: Some(meta) });
    match found {
        None => refused(StatusCode::GATEWAY_TIMEOUT, json!({"error": "timed out waiting for a stop"})),
        Some((Kind::Paused, stop)) => Ok(json_response(StatusCode::OK, stop_fields(&stop), &meta)),
        Some((Kind::DebuggerReset, _)) => refused(StatusCode::CONFLICT, json!({"error": "a save state was loaded"})),
        Some((_, entry)) => {
            refused(StatusCode::SERVICE_UNAVAILABLE, json!({"error": "the emulator shut down", "shutdown": entry}))
        }
    }
}

async fn regs_get(State(s): State<AppState>) -> ApiResult {
    s.call_json(Cmd::GetRegs, DEFAULT_TIMEOUT).await
}

async fn fpu_get(State(s): State<AppState>) -> ApiResult {
    s.call_json(Cmd::GetFpu, DEFAULT_TIMEOUT).await
}

async fn regs_put(State(s): State<AppState>, body: Bytes) -> ApiResult {
    let Value::Object(map) = parse_body(&body)? else {
        return Err(bad("expected a JSON object of register: value"));
    };
    s.call_json(Cmd::SetRegs(map), DEFAULT_TIMEOUT).await
}

#[derive(Deserialize)]
struct MemQuery {
    addr: String,
    len: Option<usize>,
    format: Option<String>,
}

async fn mem_get(State(s): State<AppState>, Query(q): Query<MemQuery>) -> ApiResult {
    let len = q.len.unwrap_or(256).min(1 << 20);
    // An unknown format is refused before the emulator reads anything.
    let format = q.format.as_deref().unwrap_or("hex");
    if !matches!(format, "hex" | "json" | "base64" | "raw" | "bin") {
        return Err(bad(format!("unknown format '{}' (hex, json, base64, raw)", format)));
    }
    let (Reply::Bytes { addr, segoff, data }, meta) = s.call(Cmd::ReadMem { addr: q.addr, len }, DEFAULT_TIMEOUT).await? else {
        return Err(unexpected());
    };
    // The JSON formats: the bytes as base64, or as the hex string PUT takes.
    let json_with = |key: &str, bytes: String| {
        json_response(StatusCode::OK, json!({"addr": format!("{:05X}", addr), "len": data.len(), key: bytes}), &meta)
    };
    match format {
        "hex" => Ok(with_meta(text_response(hexdump(addr, segoff, &data)), &meta)),
        "base64" => Ok(json_with("base64", base64::engine::general_purpose::STANDARD.encode(&data))),
        "json" => Ok(json_with("hex", trace::hex_bytes(&data))),
        _ => Ok(with_meta(([(header::CONTENT_TYPE, "application/octet-stream")], data).into_response(), &meta)),
    }
}

/// Classic DEBUG-style dump. Rows are labelled SEG:OFF when the request used
/// a segmented address (with a 32-bit offset past FFFFh), otherwise with the
/// address.
fn hexdump(base: usize, segoff: Option<(u16, u32)>, data: &[u8]) -> String {
    let mut out = String::new();
    let wide = segoff.is_some_and(|(_, off)| off as usize + data.len() > 0x10000);
    for (i, chunk) in data.chunks(16).enumerate() {
        let label = match segoff {
            Some((seg, off)) if wide => format!("{:04X}:{:08X}", seg, off.wrapping_add((i * 16) as u32)),
            Some((seg, off)) => format!("{:04X}:{:04X}", seg, (off as usize + i * 16) as u16),
            None if base + data.len() > 0x10_0000 => format!("{:08X}", base + i * 16),
            None => format!("{:05X}", base + i * 16),
        };
        let hex: Vec<String> = chunk.iter().map(|b| format!("{:02X}", b)).collect();
        let ascii: String = chunk
            .iter()
            .map(|&b| if (0x20..0x7F).contains(&b) { b as char } else { '.' })
            .collect();
        out.push_str(&format!("{}  {:<48} {}\n", label, hex.join(" "), ascii));
    }
    out
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MemPut {
    addr: String,
    hex: Option<String>,
    base64: Option<String>,
    /// Hex bytes that must be at `addr` for the write to happen.
    expect: Option<String>,
}

/// Bytes from hex digits. Anything else is skipped, as is a `0x` that
/// starts a number, so "0xCD 0xAB" is CD AB.
fn hex_data(h: &str) -> Result<Vec<u8>, ApiError> {
    let mut clean = String::with_capacity(h.len());
    let mut prev: Option<char> = None;
    let mut chars = h.chars().peekable();
    while let Some(c) = chars.next() {
        let starts_number = !prev.is_some_and(|p| p.is_ascii_hexdigit());
        if c == '0' && starts_number && matches!(chars.peek(), Some('x' | 'X')) {
            chars.next();
            prev = Some('x');
            continue;
        }
        if c.is_ascii_hexdigit() {
            clean.push(c);
        }
        prev = Some(c);
    }
    if !clean.len().is_multiple_of(2) {
        return Err(bad("hex data must have an even number of digits"));
    }
    Ok((0..clean.len()).step_by(2).map(|i| u8::from_str_radix(&clean[i..i + 2], 16).unwrap()).collect())
}

async fn mem_put(State(s): State<AppState>, body: Bytes) -> ApiResult {
    let b: MemPut = from_value(parse_body(&body)?)?;
    let data = match (b.hex, b.base64) {
        (Some(h), None) => hex_data(&h)?,
        (None, Some(b64)) => base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .map_err(|e| bad(format!("invalid base64: {}", e)))?,
        _ => return Err(bad("provide exactly one of 'hex' or 'base64'")),
    };
    let expect = b.expect.as_deref().map(hex_data).transpose()?;
    s.call_json(Cmd::WriteMem { addr: b.addr, data, expect }, DEFAULT_TIMEOUT).await
}

#[derive(Deserialize)]
struct DisasmQuery {
    addr: Option<String>,
    count: Option<usize>,
    format: Option<String>,
}

async fn disasm(State(s): State<AppState>, Query(q): Query<DisasmQuery>) -> ApiResult {
    let (Reply::Json(v), meta) = s.call(Cmd::Disasm { addr: q.addr, count: q.count.unwrap_or(20) }, DEFAULT_TIMEOUT).await?
    else {
        return Err(unexpected());
    };
    if q.format.as_deref() == Some("json") {
        return Ok(json_response(StatusCode::OK, v, &meta));
    }
    let lines: Vec<&str> = v["lines"].as_array().into_iter().flatten().filter_map(|l| l.as_str()).collect();
    Ok(with_meta(text_response(lines.join("\n") + "\n"), &meta))
}

async fn bp_list(State(s): State<AppState>) -> ApiResult {
    s.call_json(Cmd::ListBreakpoints, DEFAULT_TIMEOUT).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BpBody {
    addr: Option<String>,
    /// With `addr`: the breakpoint goes once it stops the machine.
    #[serde(default)]
    once: bool,
    /// An exception vector (hex, or a number) or "any".
    exception: Option<Value>,
    mode_switch: Option<bool>,
    /// Pause at the entry point of each program DOS starts, or after each
    /// program ends (false stops).
    program_start: Option<bool>,
    program_exit: Option<bool>,
    /// With `exception`: false stops pausing on it.
    enabled: Option<bool>,
}

/// The vectors an `exception` breakpoint names, as a mask.
fn exception_mask(v: &Value) -> Result<u32, ApiError> {
    let vector = match v {
        Value::String(s) if s.eq_ignore_ascii_case("any") => return Ok(u32::MAX),
        Value::String(s) => super::parse_hex(s).map_err(bad)?,
        Value::Number(n) => n.as_u64().ok_or_else(|| bad("exception: not a vector number"))? as u32,
        _ => return Err(bad("exception: a vector number or \"any\"")),
    };
    if vector >= 32 {
        return Err(bad("exception vectors are 0 to 1F"));
    }
    Ok(1 << vector)
}

async fn bp_add(State(s): State<AppState>, body: Bytes) -> ApiResult {
    let b: BpBody = from_value(parse_body(&body)?)?;
    if b.exception.is_some() || b.mode_switch.is_some() || b.program_start.is_some() || b.program_exit.is_some() {
        if b.once {
            return Err(bad("once goes with an address breakpoint, not with exception, mode_switch, program_start or program_exit"));
        }
        let mask = b.exception.as_ref().map(exception_mask).transpose()?;
        let (exceptions, clear_exceptions) = if b.enabled == Some(false) { (None, mask) } else { (mask, None) };
        let cmd = Cmd::BreakOn {
            exceptions,
            clear_exceptions,
            mode_switch: b.mode_switch,
            program_start: b.program_start,
            program_exit: b.program_exit,
        };
        return s.call_json(cmd, DEFAULT_TIMEOUT).await;
    }
    let addr = b.addr.ok_or_else(|| bad("missing 'addr' (or 'exception', 'mode_switch', 'program_start', 'program_exit')"))?;
    s.call_json(Cmd::AddBreakpoint { addr, once: b.once }, DEFAULT_TIMEOUT).await
}

async fn bp_remove(State(s): State<AppState>, Query(q): Query<BpBody>, body: Bytes) -> ApiResult {
    let b: BpBody = from_value(parse_body(&body)?)?;
    s.call_json(Cmd::RemoveBreakpoint(q.addr.or(b.addr)), DEFAULT_TIMEOUT).await
}

async fn wp_list(State(s): State<AppState>) -> ApiResult {
    s.call_json(Cmd::ListWatchpoints, DEFAULT_TIMEOUT).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WpBody {
    addr: Option<String>,
    len: Option<u8>,
}

async fn wp_add(State(s): State<AppState>, body: Bytes) -> ApiResult {
    let b: WpBody = from_value(parse_body(&body)?)?;
    let addr = b.addr.ok_or_else(|| bad("missing 'addr'"))?;
    s.call_json(Cmd::AddWatchpoint { addr, len: b.len.unwrap_or(1) }, DEFAULT_TIMEOUT).await
}

async fn wp_remove(State(s): State<AppState>, Query(q): Query<WpBody>, body: Bytes) -> ApiResult {
    let b: WpBody = from_value(parse_body(&body)?)?;
    s.call_json(Cmd::RemoveWatchpoint(q.addr.or(b.addr)), DEFAULT_TIMEOUT).await
}

async fn ivt(State(s): State<AppState>) -> ApiResult {
    s.call_json(Cmd::Ivt, DEFAULT_TIMEOUT).await
}

async fn gdt(State(s): State<AppState>) -> ApiResult {
    s.call_json(Cmd::Gdt, DEFAULT_TIMEOUT).await
}

async fn ldt(State(s): State<AppState>) -> ApiResult {
    s.call_json(Cmd::Ldt, DEFAULT_TIMEOUT).await
}

async fn idt(State(s): State<AppState>) -> ApiResult {
    s.call_json(Cmd::Idt, DEFAULT_TIMEOUT).await
}

async fn tss(State(s): State<AppState>) -> ApiResult {
    s.call_json(Cmd::Tss, DEFAULT_TIMEOUT).await
}

#[derive(Deserialize)]
struct AddrQuery {
    addr: String,
}

async fn pagewalk(State(s): State<AppState>, Query(q): Query<AddrQuery>) -> ApiResult {
    s.call_json(Cmd::PageWalk(q.addr), DEFAULT_TIMEOUT).await
}

async fn xms(State(s): State<AppState>) -> ApiResult {
    s.call_json(Cmd::Xms, DEFAULT_TIMEOUT).await
}

async fn net(State(s): State<AppState>) -> ApiResult {
    s.call_json(Cmd::Net, DEFAULT_TIMEOUT).await
}

async fn serial(State(s): State<AppState>) -> ApiResult {
    s.call_json(Cmd::Serial, DEFAULT_TIMEOUT).await
}

async fn printer(State(s): State<AppState>) -> ApiResult {
    s.call_json(Cmd::Printer { eject: false }, DEFAULT_TIMEOUT).await
}

async fn printer_eject(State(s): State<AppState>) -> ApiResult {
    s.call_json(Cmd::Printer { eject: true }, DEFAULT_TIMEOUT).await
}

async fn sound(State(s): State<AppState>) -> ApiResult {
    s.call_json(Cmd::Sound, DEFAULT_TIMEOUT).await
}

async fn gus(State(s): State<AppState>) -> ApiResult {
    s.call_json(Cmd::Gus, DEFAULT_TIMEOUT).await
}

async fn awe32(State(s): State<AppState>) -> ApiResult {
    s.call_json(Cmd::Awe32, DEFAULT_TIMEOUT).await
}

async fn exceptions(State(s): State<AppState>) -> ApiResult {
    s.call_json(Cmd::Exceptions, DEFAULT_TIMEOUT).await
}

// ---------------------------------------------------------------------------
// WebSockets
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct EventsQuery {
    /// Replay the journal's entries after this one, then go on live.
    after: Option<u64>,
    /// Log lines and video mode changes, which the journal doesn't keep
    /// (default true).
    volatile: Option<bool>,
    /// Replay the log lines after line number N first.
    log_after: Option<u64>,
}

/// Where a stream of events starts, and what else it carries.
struct EventStream {
    after: Option<u64>,
    volatile: bool,
    log_after: Option<u64>,
    trace: Option<broadcast::Receiver<Arc<str>>>,
}

/// What a broadcast receiver gives next, or never without one.
async fn next_broadcast(rx: Option<&mut broadcast::Receiver<Arc<str>>>) -> Result<Arc<str>, broadcast::error::RecvError> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

/// Send the journal's entries (after `after`, or from now) as they come,
/// with the log lines and video mode changes (`volatile`), and the trace
/// batches of `/ws/trace`, until either side closes or the journal ends
/// with `shutdown`, after which the socket is closed. Entries come from
/// the journal itself, in order, without gaps or repeats; if some after
/// `after` were evicted, `{"type":"gone","first":F}` comes first.
async fn stream_events(mut socket: WebSocket, shared: Arc<Shared>, mut stream: EventStream) {
    use broadcast::error::RecvError;
    let _busy = Busy::new(&shared);
    // Subscribed before reading anything, so that nothing falls between
    // the replay and what comes live.
    let mut seq = shared.journal_seq.subscribe();
    let mut volatile = stream.volatile.then(|| shared.events.subscribe());
    let mut cursor = stream.after.unwrap_or_else(|| *seq.borrow());
    let mut log_replay: std::collections::VecDeque<super::LogLine> = match stream.log_after {
        Some(n) => {
            let log = shared.log.lock().unwrap_or_else(|e| e.into_inner());
            log.iter().filter(|l| l.n > n).cloned().collect()
        }
        None => Default::default(),
    };
    // Live log lines up to this one were replayed: those are dropped
    // until the first live line past it.
    let replayed = log_replay.back().map_or(0, |l| l.n);
    let mut dedup = replayed > 0;
    loop {
        seq.borrow_and_update();
        let mut out: Vec<String> = Vec::new();
        let mut ended = false;
        {
            let j = shared.journal.lock().unwrap_or_else(|e| e.into_inner());
            if let Err(gone) = j.after(cursor) {
                out.push(json!({"type": "gone", "first": gone.first}).to_string());
                cursor = gone.first - 1;
            }
            if let Ok(entries) = j.after(cursor) {
                for e in entries {
                    // Log lines written before this entry go first.
                    while log_replay.front().is_some_and(|l| l.at < e.seq) {
                        out.extend(log_replay.pop_front().map(|l| l.event_json()));
                    }
                    out.push(e.json.to_string());
                    cursor = e.seq;
                    ended |= e.kind == Kind::Shutdown;
                }
            }
        }
        out.extend(log_replay.drain(..).map(|l| l.event_json()));
        for text in out {
            if socket.send(Message::Text(text.into())).await.is_err() {
                return;
            }
        }
        if ended {
            let _ = socket.send(Message::Close(None)).await;
            return;
        }
        let text = tokio::select! {
            changed = seq.changed() => match changed {
                Ok(()) => continue,
                Err(_) => return,
            },
            m = next_broadcast(volatile.as_mut()) => match m {
                Ok(m) => {
                    if dedup {
                        match log_n(&m) {
                            Some(n) if n <= replayed => continue,
                            Some(_) => dedup = false,
                            None => {}
                        }
                    }
                    m.to_string()
                }
                Err(RecvError::Lagged(n)) => json!({"type": "lagged", "missed": n}).to_string(),
                Err(RecvError::Closed) => return,
            },
            m = next_broadcast(stream.trace.as_mut()) => match m {
                Ok(m) => m.to_string(),
                Err(RecvError::Lagged(n)) => json!({"type": "lagged", "missed": n}).to_string(),
                Err(RecvError::Closed) => return,
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                _ => continue,
            },
        };
        if socket.send(Message::Text(text.into())).await.is_err() {
            return;
        }
    }
}

/// A live log line's number, if the event is one.
fn log_n(event: &str) -> Option<u64> {
    if !event.starts_with(r#"{"#) {
        return None;
    }
    let v: Value = serde_json::from_str(event).ok()?;
    if v["type"] == "log" { v["n"].as_u64() } else { None }
}

async fn ws_trace(State(s): State<AppState>, Query(q): Query<EventsQuery>, ws: WebSocketUpgrade) -> ApiResult {
    server_only()?;
    check_after(&s.shared, q.after)?;
    // Subscribing to the trace channel is what turns streaming on; the main
    // loop checks receiver_count() every batch.
    let stream = EventStream {
        after: q.after,
        volatile: q.volatile.unwrap_or(true),
        log_after: q.log_after,
        trace: Some(s.shared.trace.subscribe()),
    };
    let shared = s.shared.clone();
    Ok(ws.on_upgrade(move |socket| stream_events(socket, shared, stream)))
}

async fn ws_events(State(s): State<AppState>, Query(q): Query<EventsQuery>, ws: WebSocketUpgrade) -> ApiResult {
    server_only()?;
    check_after(&s.shared, q.after)?;
    let stream = EventStream { after: q.after, volatile: q.volatile.unwrap_or(true), log_after: q.log_after, trace: None };
    let shared = s.shared.clone();
    Ok(ws.on_upgrade(move |socket| stream_events(socket, shared, stream)))
}

#[derive(Deserialize)]
struct ScreenQuery {
    fps: Option<f64>,
}

struct SubscriberGuard(Arc<Shared>);

impl Drop for SubscriberGuard {
    fn drop(&mut self) {
        self.0.screen_subscribers.fetch_sub(1, Ordering::Relaxed);
    }
}

async fn ws_screen(State(s): State<AppState>, Query(q): Query<ScreenQuery>, ws: WebSocketUpgrade) -> ApiResult {
    server_only()?;
    let fps = q.fps.unwrap_or(5.0).clamp(0.1, 60.0);
    Ok(ws.on_upgrade(move |mut socket| async move {
        s.shared.screen_subscribers.fetch_add(1, Ordering::Relaxed);
        let _guard = SubscriberGuard(s.shared.clone());
        let mut interval = tokio::time::interval(Duration::from_secs_f64(1.0 / fps));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last_seq = u64::MAX;
        loop {
            tokio::select! {
                _ = interval.tick() => {}
                incoming = socket.recv() => match incoming {
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                    _ => continue,
                },
            }
            let seq = s.shared.frame_seq.load(Ordering::Acquire);
            // seq 0 = nothing captured yet since the first subscriber arrived.
            if seq == last_seq || seq == 0 {
                continue;
            }
            last_seq = seq;
            let frame = match s.shared.frame.lock() {
                Ok(f) => f.clone(),
                Err(_) => return,
            };
            let png = match tokio::task::spawn_blocking(move || encode_png(&frame)).await {
                Ok(Ok(p)) => p,
                _ => continue,
            };
            if socket.send(Message::Binary(png.into())).await.is_err() {
                return;
            }
        }
    }))
}

struct HeatGuard(Arc<Shared>);

impl Drop for HeatGuard {
    fn drop(&mut self) {
        self.0.heat_subscribers.fetch_sub(1, Ordering::Relaxed);
    }
}

/// The memory heatmap (`heat.rs`), a binary message a frame at up to `fps`
/// a second; the machine's writes are watched while one is open.
async fn ws_heat(State(s): State<AppState>, Query(q): Query<ScreenQuery>, ws: WebSocketUpgrade) -> ApiResult {
    server_only()?;
    let fps = q.fps.unwrap_or(15.0).clamp(0.1, 60.0);
    Ok(ws.on_upgrade(move |mut socket| async move {
        s.shared.heat_subscribers.fetch_add(1, Ordering::Relaxed);
        let _guard = HeatGuard(s.shared.clone());
        let mut interval = tokio::time::interval(Duration::from_secs_f64(1.0 / fps));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last_seq = 0;
        loop {
            tokio::select! {
                _ = interval.tick() => {}
                incoming = socket.recv() => match incoming {
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                    _ => continue,
                },
            }
            let seq = s.shared.heat_seq.load(Ordering::Acquire);
            if seq == last_seq {
                continue;
            }
            last_seq = seq;
            let message = match s.shared.heat.lock() {
                Ok(m) => m.clone(),
                Err(_) => return,
            };
            if socket.send(Message::Binary(message.to_vec().into())).await.is_err() {
                return;
            }
        }
    }))
}

/// The address ranges the heatmap saw written in the last second or so,
/// while a `/ws/heat` stream is open.
async fn heat_get(State(s): State<AppState>) -> ApiResult {
    server_only()?;
    let active = s.shared.heat_subscribers.load(Ordering::Relaxed) > 0;
    let ranges: Vec<Value> = s
        .shared
        .heat_ranges
        .lock()
        .map(|r| r.iter().map(|&(a, b)| json!([format!("{:#07X}", a), format!("{:#07X}", b)])).collect())
        .unwrap_or_default();
    let ranges = if active { ranges } else { Vec::new() };
    Ok(json_ok(&json!({ "active": active, "block_size": 64, "written": ranges })))
}

async fn ws_audio(State(s): State<AppState>, ws: WebSocketUpgrade) -> ApiResult {
    server_only()?;
    let mut rx = s.shared.audio.subscribe();
    Ok(ws.on_upgrade(move |mut socket| async move {
        use broadcast::error::RecvError;
        let header = json!({"type": "audio_format", "sample_rate": 44100, "channels": 2, "format": "s16le"});
        if socket.send(Message::Text(header.to_string().into())).await.is_err() {
            return;
        }
        loop {
            let chunk = tokio::select! {
                m = rx.recv() => m,
                incoming = socket.recv() => match incoming {
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                    _ => continue,
                },
            };
            let samples = match chunk {
                Ok(c) => c,
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return,
            };
            let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
            if socket.send(Message::Binary(bytes.into())).await.is_err() {
                return;
            }
        }
    }))
}

/// Each message carries its own `if_state_seq`; the header on the upgrade
/// would hold for none of them.
async fn ws_input(State(s): State<AppState>, ws: WebSocketUpgrade) -> ApiResult {
    server_only()?;
    Ok(ws.on_upgrade(move |mut socket| async move {
        while let Some(Ok(msg)) = socket.recv().await {
            let text = match msg {
                Message::Text(t) => t.to_string(),
                Message::Close(_) => return,
                _ => continue,
            };
            let reply = handle_ws_input(&s, &text).await;
            if socket.send(Message::Text(reply.to_string().into())).await.is_err() {
                return;
            }
        }
    }))
}

/// One `/ws/input` message: input events as `/api/input/batch` takes them,
/// with `wait`, `request_id` and `if_state_seq`. The reply has the
/// message's `req` and the journal's `state_seq`.
async fn handle_ws_input(s: &AppState, text: &str) -> Value {
    let mut req = None;
    let result = async {
        let mut v: Value = serde_json::from_str(text).map_err(|e| bad(format!("invalid JSON: {}", e)))?;
        let mut take = |key: &str| match &mut v {
            Value::Object(o) => o.remove(key),
            _ => None,
        };
        let wait = match take("wait") {
            Some(Value::Bool(w)) => w,
            Some(_) => return Err(bad("wait: true or false")),
            None => false,
        };
        let tag = match take("request_id") {
            Some(Value::String(t)) => Some(request_id(&t)?),
            Some(_) => return Err(bad("request_id: a string")),
            None => None,
        };
        let if_state = match take("if_state_seq") {
            Some(n) => Some(n.as_u64().ok_or_else(|| bad("if_state_seq: a journal sequence number"))?),
            None => None,
        };
        let ctx = s.context(tag, if_state);
        req = Some(ctx.req);
        let events = parse_events(v, None)?;
        queue_input(s, &ctx, events, wait).await
    }
    .await;
    let mut reply = match result {
        Ok(response) => {
            // The HTTP reply's body, which is JSON: a body that isn't is an
            // error, told as one.
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap_or_default();
            serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({"ok": false, "error": "unexpected reply"}))
        }
        Err(e) => {
            let mut body = e.body.clone();
            body["ok"] = false.into();
            body["error"] = e.message().into();
            if let Some(meta) = &e.meta {
                merge_meta(&mut body, meta);
            }
            body
        }
    };
    if let (Some(req), Value::Object(o)) = (req, &mut reply) {
        o.insert("req".into(), req.into());
    }
    reply
}

const HELP: &str = r#"rust-dos debug interface
========================
All endpoints are local-only and unauthenticated. JSON in, JSON out unless noted.
A JSON body with a field the endpoint doesn't have gets 400 naming it.
Open this address in a web browser for the debugger page (/ui): the screen,
execution control, registers, disassembly, memory, breakpoints and the log.
Addresses are hex: "SEG:OFF" (registers allowed, e.g. "CS:IP", "DS:SI", "B800:0")
or a linear address ("0x12345", "B8000").

REQUESTS AND THE JOURNAL
  The journal is the numbered record of what the machine and the debugger
  did. Every response has X-Debug-Req (and "req" in a JSON object body):
  the request's number. X-Request-Id: up to 128 printable ASCII characters,
  echoed and copied into the journal entries the request causes as
  "request_id". Answers from the emulator have X-Debug-State-Seq and, in
  JSON, "state_seq" and "stop_seq"; a request that changed something also
  "seq" (its last journal entry), and one that started an operation "op".
  If-State-Seq: N   carry the request out only if the machine is still
                    paused in the state of entry N: nothing pending, the
                    settings window closed, no input being typed, and no
                    entry since N that changed the machine. Otherwise 409
                    {"error","precondition":"state",...}. A 400 on requests
                    the server answers itself (journal, ops, log, version,
                    waits with after= or op=, the page and the streams).
  Either header twice is a 400.
  GET  /api/version                        name, version, build, api_version, instance, features
                   build: {"commit","commit_short","dirty"}, the git commit the
                   program was built from and whether a tracked file differed
                   from it; all null when the build had no git checkout. The
                   journal's "started" entry has it too.
  GET  /api/journal?after=S[&type=paused,resumed&op=N&cause=N&limit=200&wait=MS&allow_gap=true]
                   the entries after S, oldest first: {"instance","first","last",
                   "state_seq","stop_seq","entries","next","timed_out"}. With
                   wait, waits for a matching entry. 410 if some after S are
                   gone (allow_gap goes on from the oldest kept). "shutdown"
                   matches every filter. An after past the last entry is a
                   400 with last and instance (a cursor from another instance).
  GET  /api/ops/N[?wait=MS]                operation N (a resume, step, step_over, run,
                   reboot, reboot_shell or input request): kind, state (pending,
                   stopped, done, failed, superseded, cancelled), start_seq,
                   end_seq, superseded_by, stop, end, error. 404 unknown, 410 gone
                   (the latest 1024 ended ones are kept, pending ones until they
                   end). Input past 1024 requests still being typed is a 429.
  Entry types: started, resumed, paused, program_started, program_exited,
  op_failed, input_queued, input_delivered, input_cleared, memory_written,
  registers_written, breakpoint_set, breakpoint_removed, watchpoint_set,
  watchpoint_removed, break_on, drive_mounted, drive_unmounted,
  images_swapped, shared_synced, state_saved, state_loaded, debugger_reset,
  speed_changed, settings_window, host_input, shutdown (always last).

STATUS / SCREEN
  GET  /api/status                         emulator state, CS:IP, video mode, trace fill, fps, speed,
                                           and the journal: seq, first_seq, state_seq, stop, op,
                                           quiescent, instance
  GET  /api/stats                          execution speed (MIPS) and decode-cache hit rate, per ~1 s;
                                           frame, per 1/4 s: the Stats page's numbers and emulated_speed
                                           (emulated s per wall s, full speed while near 1); fps the
                                           program draws; cpu_use is the percent of one host core the
                                           main thread is busy
  POST /api/profile                        {"enabled":true,"hz":2000,"timer":"sleep"|"spin","duration_ms":N}
                                           starts a profile (again, if one runs): a thread samples where
                                           the emulation thread's host time goes, by zone and guest
                                           CS:EIP, and the bus counts ports, services, IRQs, exceptions,
                                           device memory and translations; {"enabled":false} stops it
  GET  /api/profile                        the profile so far: ?format=json|text|folded (flame graph
                                           stacks), top=25, zone=io.vga or a group (io, hle, ...),
                                           bucket=16 (EIP rounded down), disasm=4 (lines a place),
                                           timeline=true (zones a second), wait_ms=N (wait for a
                                           profile with a duration to end); docs/profiling.md
  GET  /api/screenshot[?format=png|raw]    the screen with its cursors, at the picture's
                                           size: 640x400 in text modes, the mode's size
                                           (small ones doubled) otherwise (raw = RGB24
                                           bytes, size in x-width/x-height)
  GET  /api/screen/text[?format=text]      text-mode screen contents (CP437 -> Unicode);
                                           JSON has attrs: each row's attribute runs {col,len,attr},
                                           blink (bit 7 blinks, else bright background),
                                           mono (MDA/Hercules attributes), and the cursor
                                           of the active display page
                                           (features has "screen_attrs")
  GET  /api/log[?since_ms=&limit=&grep=&format=text]   emulator log lines, each with its
                                           number n, the journal's last entry "at", and the
                                           request that wrote it ("cause")
  GET  /api/log?after_n=N[&wait=MS]        the lines after line N, oldest first
  POST /api/ports {"enabled":true, "clear":true, "capacity":100000, "ports":"3C0-3CF"}  log port accesses
  GET  /api/ports[?port=3C0-3CF&dir=in|out&last_n=200]  the logged accesses, oldest first

TRACE
  POST /api/trace  {"enabled":true, "clear":false, "stream_max":1000}
                   {"count":5000}: record the next 5000 instructions, then stop
                   ({"count":0} stops now; /api/status shows the rest as remaining)
  GET  /api/trace?last_n=200 | ?last_ms=500 | ?from_ms=&to_ms=  [&limit=&cs=&no_bios=true&format=text|json]
       the newest entries: last_n of them (at most limit, if given), else up to limit (default 10000);
       truncated (x-trace-truncated) is true when limit left out entries that match
  GET  /api/trace?since=N[&limit=]   the entries recorded after cursor N, oldest first;
       the reply's next (x-trace-next) is the cursor to go on from, and dropped
       (x-trace-dropped) counts entries after N overwritten before they were read
       Instruction trace (registers shown are BEFORE execution). Timestamps are
       ms since emulator start, sampled per ~16 ms batch; icount orders exactly.

INPUT  (append ?wait=false to return immediately instead of after delivery)
  Every input request is an operation ("op"), done once its last event is
  delivered (input_delivered). Paused in deterministic mode, a wait is
  answered at once ("held":true) and the operation ends at delivery.
  POST /api/input/key    {"key":"enter", "action":"press|down|up", "mods":["ctrl"], "hold_ms":50}
                         or {"scancode":30, "ascii":97}
  POST /api/input/type   {"text":"dir\n", "delay_ms":0}
  POST /api/input/mouse  {"action":"move|down|up|click", "x":320, "y":200, "dx":0, "dy":0,
                          "button":"left|right|middle", "coords":"screen|virtual"}
                         screen coords = pixels of the screenshot (default)
  POST /api/input/wait   {"ms":500}
  POST /api/input/batch  [{"type":"type","text":"cd games\n"},{"type":"wait","ms":300},
                          {"type":"key","key":"f1"}]
                         A field an event doesn't have gets 400 and queues nothing.
  DELETE /api/input      clear pending queued input ("cancelled": the input operations)

EXECUTION CONTROL
  resume, step, step_over, run, reboot and reboot_shell start an operation
  ("op"), which ends when the machine stops, or when another of them takes
  over ("superseded"). The stop names the request that caused it ("cause").
  POST /api/control/pause                            the stop is the reply
  POST /api/control/resume   {"until":"1234:0100"}   (optional temporary breakpoint)
                             {"until_ms":5000}       with --deterministic: pause when
                                                     emulated time reaches 5000 ms
  POST /api/control/step     {"count":1}             returns registers after stepping
  POST /api/control/step_over                        step, but run a CALL, INT, LOOP or
                                                     REP string instruction to the next one
  POST /api/control/run      {"command":"GAME.EXE /x", "stop_at_entry":true}
                             run a command line at the DOS prompt; replies once it
                             started a program ({"program":{name,entry,psp}}), or
                             with stop_at_entry once paused at its first instruction
                             (reason "program_start"); 422 if it started none.
                             409 while the prompt is busy (a program, a batch
                             file such as AUTOEXEC.BAT, PAUSE); with "queue":true
                             the command line waits its turn after the batch
                             lines and runs once the prompt is back (the run's
                             "resumed" entry has "queued":true)
                             step, step_over and run wait for their end; "wait":false
                             answers at once (202) with "op"; timeout_ms (504 with "op")
  POST /api/control/reboot_shell                     kill the running program and its batch file
  POST /api/control/reboot                           reset the machine, as Ctrl+Alt+Del
  POST /api/control/quit                             turn the emulator off; the journal ends
                                                     with "shutdown"
  GET  /api/control/wait?after=S[&timeout_ms=]       the first stop after entry S, at once if
                                                     there is one; 410 if entries after S
                                                     are gone
  GET  /api/control/wait?op=N                        the end of operation N
  GET  /api/control/wait?timeout_ms=30000            the stop the machine is in, or the next
                                                     (a stop before the request is missed:
                                                     use after= or op=)
  GET  /api/registers        PUT /api/registers {"ax":"1234","flags":"0202"}
  GET  /api/registers/fpu    the x87 words and ST(0)-ST(7), MM0-MM7 with MMX,
                             XMM0-XMM7 and MXCSR with SSE
  GET  /api/memory?addr=DS:SI&len=256[&format=hex|json|base64|raw]
                             hex (the default): a text dump; raw: the bytes;
                             json: {"addr","len","hex":"41 1F"}; base64:
                             {"addr","len","base64"}. The JSON formats have
                             "state_seq"; X-Debug-State-Seq is on all four.
                             Their "addr" is linear (physical only for a
                             bare address); write back to the read's addr=.
                             (features has "memory_json" when json is there)
  PUT  /api/memory {"addr":"B800:0000","hex":"41 1F"}  (or "base64"); replies with
                             the bytes replaced ("old") and read back ("new");
                             "expect":"20 07" writes only if those bytes are there (409)
  GET  /api/disasm?addr=CS:IP&count=20[&format=json]   json: "lines" and "rows"
                                           ({label, phys, bytes, asm, len, current, breakpoint})
  GET/POST/DELETE /api/breakpoints   {"addr":"1234:0100"}  (DELETE without addr = all)
                  {"addr":"1234:0100","once":true}: goes once it stops the machine
                  (the stop has "breakpoint" and "once":true; the list "once")
                  also {"exception":"0D"} or {"exception":"any"}: pause in the handler
                  after the CPU raises it ({"exception":"0D","enabled":false} stops); {"mode_switch":true}: pause after CR0.PE changes
                  {"program_start":true}: pause at each program's entry point;
                  {"program_exit":true}: pause after each program ends ("exit":{name,code,resident,aborted})
  GET/POST/DELETE /api/watchpoints   {"addr":"DS:0100","len":2}  pause when the 1, 2 or
                  4 bytes there change (DELETE without addr = all)
  GET  /api/ivt              interrupt vector table (real mode)
  Addresses: SEG:OFF (in protected mode SEL:OFF32 through the GDT/LDT; register
  names like CS:EIP, DS:ESI), lin:ADDR (through the page tables), phys:ADDR or ADDR.

PROTECTED MODE
  GET  /api/gdt  /api/ldt    descriptors, decoded
  GET  /api/idt              interrupt, trap and task gates
  GET  /api/tss              the current TSS (stacks, CR3, registers, I/O map)
  GET  /api/pagewalk?addr=lin:00401000   page directory/table entries and flags
  GET  /api/exceptions       the last 64 exceptions (vector, error code, CS:EIP, CR2)
  GET  /api/xms              XMS handles, A20 and the HMA
  GET  /api/net              The IPX driver's sockets, ECBs and packets, and the LAN
  GET  /api/serial           The serial ports: UART registers, what is plugged in,
                             and the serial link to the other player or a host
  GET  /api/printer          The printer on LPT1: output, bytes and pages printed,
                             whether a job is printing, the last file written
  POST /api/printer/eject    Eject the page and end the print job; returns once
                             its files are written

MEMORY HEATMAP
  GET  /api/heat             the address ranges written in about the last second, while
                             a /ws/heat stream is open ("active")

SOUND
  GET  /api/sound            what the sound visualizer (Ctrl+Shift+S) shows: the FM
                             channels' notes and envelopes, the Ultrasound's voices,
                             the MIDI notes held, and what it muted
  GET  /api/gus              the Gravis Ultrasound: latches, IRQ status, timers,
                             DMA and every voice's registers
  GET  /api/awe32            the AWE32's EMU8000: ROM, RAM, configuration, effects
                             and the sounding voices

DRIVES
  GET    /api/drive                                list drives, types and paths
  PUT    /api/drive/D {"path":"/home/me/dos/cd","type":"cdrom","label":"GAMECD"}
                   mount or replace a drive; type floppy|hdd|cdrom, optional
                   "read_only":true. The path is a directory or a disk or CD
                   image (.img, .ima, .vfd, .flp, .dsk, .86f, .cue, .iso,
                   .bin); an image's type is found from the image unless
                   given.
                   "images":["disk2.img",...] adds images to step through.
                   Replacing closes that drive's open files; with no options
                   a remount keeps the drive's type and label.
  POST   /api/drive/swap                           next image in every drive; a booted
                   system's CD made from a host folder is made again
  POST   /api/drive/sync                           copy what a booted system changed on
                   the disks of shared host folders into them (no deletions)
                   mounted from a list of images (Ctrl+F4)
  DELETE /api/drive/D                              unmount (not C: or Z:)

SAVE STATES
  POST   /api/state/save {"path":"/tmp/keen.state"}  save the machine to a file
  POST   /api/state/load {"path":"/tmp/keen.state"}  load one: its hardware
                   settings, then the machine and its drives (the same
                   memsize only). Breakpoints and watchpoints stay.
                   "differences" lists the hardware settings, drives,
                   folders, images, files and deterministic start time
                   that differed from this machine's, if any. A failed load
                   (400) changes nothing ("loaded":false, "unchanged":true).
                   Replies have "saved"/"loaded", the state's "header"
                   (with its "media"), "format" and "emulator" (this
                   version).
                   "strict_match":true on either: only while paused with
                   no step or run pending, and for a save no queued input
                   left to type (409 {"error","paused"} otherwise). A load
                   is refused (409, "differences", nothing loaded) unless
                   the state's hardware settings, media and deterministic
                   start time match this machine's. It clears breakpoints,
                   watchpoints, break_on stops, run-to targets, queued
                   input, held keys and an until_ms, cancels the
                   operations under way (409 to their waits), stays
                   paused, and lists what it cleared in "debugger_reset".
                   With --deterministic any load is taken only while
                   paused (409 otherwise).
                   A load while paused stops the machine in the loaded
                   state: a "paused" entry with reason "load".
                   "check_files":true on either, for extra strictness: a
                   save records the size and SHA-256 of every file on
                   the writable drives and of writable images; a load
                   names the files changed since in "differences"
                   (refused if strict), or lists in "unchecked" the
                   drives the state has no such record of. Off by
                   default: files rarely affect the machine's state,
                   hashing large folders takes time, and other files on
                   the drive show up as changes.
                   "folders":{"C":"/abs/copy"} on a load: take the
                   state's folder drive C: as mounted from /abs/copy
                   instead of the folder it records, to load it in
                   another run whose C: is a copy of that folder. The
                   media are compared, and with "check_files" the files
                   in /abs/copy too, and the load leaves C: mounted from
                   it. 400 for a drive the state hasn't as a folder,
                   a path that isn't a folder, or a drive named twice.

SPEED
  POST   /api/speed {"cycles":"auto"}   the CPU speed, as `cycles` takes it
                   (auto, auto 5000, max or a number); /api/status has
                   cycles_per_ms, the speed it runs at now

WEBSOCKETS
  /ws/events[?after=S&volatile=false&log_after=N]
                      JSON: the journal's entries (after S, or from now), then
                      live; log lines and video_mode changes unless
                      volatile=false (log_after=N replays the log lines after
                      line N). Evicted entries: {"type":"gone","first":F} first.
                      After "shutdown" the socket closes.
  /ws/trace           as /ws/events, with per-frame batches
                      {"type":"trace","dropped":N,"entries":[...]}
  /ws/screen?fps=5    binary PNG frames, sent only when the screen changes
  /ws/heat?fps=15     the memory heatmap: u32 LE block size (64), u32 LE block count,
                      a heat byte per block (255 = just written, fading), then a bit
                      per block holding code. Every write is watched while it is open
                      (the recompiler's stores take a slower path).
  /ws/audio           text header, then binary s16le 44100 Hz stereo (interleaved) chunks
  /ws/input           send input events (same JSON as /api/input/batch, optional "wait":true,
                      "request_id" and "if_state_seq"); replies have "req" and "state_seq"
"#;

#[cfg(test)]
mod http_tests;

#[cfg(test)]
mod tests {
    use super::{Answer, Cmd, Meta, Reply, Request, Shared, hex_data, json, json_body, with_field_hint};
    use std::io::{Read, Write};
    use std::net::SocketAddr;
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    /// The response to `GET path` from the server at `addr`.
    fn get(addr: SocketAddr, path: &str) -> String {
        let mut stream = std::net::TcpStream::connect(addr).unwrap();
        // A server that never answers fails the test instead of hanging it.
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let request = format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    }

    #[test]
    fn a_json_trace_reply_has_the_fields_and_every_entry() {
        let entries: Vec<_> = (1..=3)
            .map(|icount| super::TraceEntry { icount, eip: 0x1234_5678, gpr: [0xFFFF_FFFF; 8], ..Default::default() })
            .collect();
        let meta = Meta { state_seq: 7, ..Default::default() };
        let out = super::trace_json(&entries, 3, 1, true, &meta, Some(42));
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!((v["req"].as_u64(), v["count"].as_u64(), v["next"].as_u64()), (Some(42), Some(3), Some(3)));
        assert_eq!((v["dropped"].as_u64(), v["truncated"].as_bool(), v["state_seq"].as_u64()), (Some(1), Some(true), Some(7)));
        let icounts: Vec<_> = v["entries"].as_array().unwrap().iter().map(|e| e["icount"].as_u64().unwrap()).collect();
        assert_eq!(icounts, [1, 2, 3]);
        assert_eq!(v["entries"][0], entries[0].to_json());
        // The guide gives the size of a full ring from these.
        let one = super::trace_json(&entries[..1], 3, 0, false, &meta, None).len();
        let two = super::trace_json(&entries[..2], 3, 0, false, &meta, None).len();
        eprintln!("json bytes per entry: {}, text: {}", two - one, entries[0].to_text().len() + 1);
        assert!(two - one < 450, "{} bytes", two - one);
        let none = super::trace_json(&[], 0, 0, false, &meta, None);
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&none).unwrap()["entries"], json!([]));
    }

    #[test]
    fn spawn_on_port_0_returns_the_port_it_listens_on() {
        let (tx, _rx) = mpsc::channel::<Request>();
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let bound = super::spawn(addr, tx, Arc::new(Shared::new(std::time::Instant::now()))).unwrap();
        assert_eq!(bound.ip(), addr.ip());
        assert_ne!(bound.port(), 0);
        // `/api` is the help text, which the server answers without the emulator.
        let response = get(bound, "/api");
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.contains("/api/status"), "{response}");
    }

    #[test]
    fn spawn_on_port_0_passes_requests_to_the_emulator() {
        let (tx, rx) = mpsc::channel::<Request>();
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let addr = super::spawn(addr, tx, Arc::new(Shared::new(std::time::Instant::now()))).unwrap();
        std::thread::spawn(move || {
            for req in rx {
                let status = matches!(req.cmd, Cmd::Status);
                let _ = req.reply.send(Answer { reply: Reply::Json(json!({"status": status})), meta: Meta::default() });
            }
        });
        let response = get(addr, "/api/status");
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.contains(r#""status":true"#), "{response}");
    }

    #[test]
    fn hex_data_skips_0x_prefixes() {
        assert_eq!(hex_data("CD AB").ok(), Some(vec![0xCD, 0xAB]));
        assert_eq!(hex_data("0xCD 0xAB").ok(), Some(vec![0xCD, 0xAB]));
        assert_eq!(hex_data("0XCDAB").ok(), Some(vec![0xCD, 0xAB]));
        assert_eq!(hex_data("00 0x10").ok(), Some(vec![0x00, 0x10]));
        assert!(hex_data("CDA").is_err());
    }

    #[test]
    fn an_unknown_field_close_to_a_known_one_is_named() {
        let hint = |m: &str| with_field_hint(m.to_string());
        let key = "unknown field `modifiers`, expected one of `key`, `scancode`, `ascii`, `action`, `mods`, `hold_ms`";
        assert!(hint(key).ends_with(" (did you mean `mods`?)"), "{}", hint(key));
        let mount = "unknown field `readonly`, expected one of `path`, `type`, `label`, `read_only`, `images`";
        assert!(hint(mount).ends_with(" (did you mean `read_only`?)"), "{}", hint(mount));
        let step = "unknown field `cnt`, expected one of `count`, `until`";
        assert!(hint(step).ends_with(" (did you mean `count`?)"), "{}", hint(step));
        let variant = "unknown variant `keys`, expected one of `key`, `type`, `mouse`, `wait`";
        assert!(hint(variant).ends_with(" (did you mean `key`?)"), "{}", hint(variant));
        // Nothing close: the message as serde gave it.
        let far = "unknown field `z`, expected one of `x`, `y`, `dx`";
        assert_eq!(hint(far), far);
        assert_eq!(hint("missing field `ms`"), "missing field `ms`");
    }

    #[test]
    fn the_request_number_goes_first_in_a_json_object() {
        let body = |v: serde_json::Value, req| {
            let response = json_body(axum::http::StatusCode::OK, &v, req);
            let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
            let bytes = rt.block_on(axum::body::to_bytes(response.into_body(), usize::MAX)).unwrap();
            String::from_utf8(bytes.to_vec()).unwrap()
        };
        assert_eq!(body(json!({"a": 1, "b": "x"}), Some(7)), r#"{"req":7,"a":1,"b":"x"}"#);
        assert_eq!(body(json!({}), Some(7)), r#"{"req":7}"#);
        assert_eq!(body(json!({"req": 3, "a": 1}), Some(7)), r#"{"req":7,"a":1}"#, "one req, the request's");
        assert_eq!(body(json!([1]), Some(7)), "[1]");
        assert_eq!(body(json!({"a": 1}), None), r#"{"a":1}"#);
    }
}
