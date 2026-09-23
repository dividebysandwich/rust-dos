//! axum HTTP/WebSocket front end for the debug interface. Runs on its own
//! thread with a private tokio runtime; everything that needs emulator
//! state is forwarded to the main thread as a `Cmd`.

use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use base64::Engine;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::sync::{broadcast, oneshot};

use super::trace::{self, TraceEntry};
use super::{Cmd, InputEvent, Reply, Request, Shared, TraceQuery};
use crate::video;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
const INPUT_WAIT_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone)]
struct AppState {
    tx: mpsc::Sender<Request>,
    shared: Arc<Shared>,
}

pub fn spawn(addr: SocketAddr, tx: mpsc::Sender<Request>, shared: Arc<Shared>) -> Result<(), String> {
    // Bind synchronously so a port conflict is reported at startup rather
    // than silently in the background thread.
    let listener = std::net::TcpListener::bind(addr).map_err(|e| format!("debug server: cannot bind {}: {}", addr, e))?;
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
    Ok(())
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(help))
        .route("/api", get(help))
        .route("/api/status", get(status))
        .route("/api/screenshot", get(screenshot))
        .route("/api/screen/text", get(screen_text))
        .route("/api/trace", get(trace_get).post(trace_post))
        .route("/api/log", get(log_get))
        .route("/api/input", axum::routing::delete(input_clear))
        .route("/api/input/{kind}", post(input_post))
        .route("/api/drive", get(drives))
        .route("/api/drive/{letter}", put(mount).post(mount).delete(unmount))
        .route("/api/control/{action}", post(control))
        .route("/api/control/wait", get(wait_pause))
        .route("/api/registers", get(regs_get).put(regs_put))
        .route("/api/memory", get(mem_get).put(mem_put))
        .route("/api/disasm", get(disasm))
        .route("/api/breakpoints", get(bp_list).post(bp_add).delete(bp_remove))
        .route("/api/ivt", get(ivt))
        .route("/ws/trace", get(ws_trace))
        .route("/ws/events", get(ws_events))
        .route("/ws/screen", get(ws_screen))
        .route("/ws/audio", get(ws_audio))
        .route("/ws/input", get(ws_input))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Plumbing
// ---------------------------------------------------------------------------

struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, axum::Json(json!({"error": self.1}))).into_response()
    }
}

fn bad(msg: impl Into<String>) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, msg.into())
}

type ApiResult = Result<Response, ApiError>;

impl AppState {
    async fn call(&self, cmd: Cmd, timeout: Duration) -> Result<Reply, ApiError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(Request { cmd, reply })
            .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "emulator has shut down".into()))?;
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Reply::Error(code, msg))) => {
                Err(ApiError(StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST), msg))
            }
            Ok(Ok(r)) => Ok(r),
            Ok(Err(_)) => Err(ApiError(StatusCode::INTERNAL_SERVER_ERROR, "request dropped".into())),
            Err(_) => Err(ApiError(
                StatusCode::GATEWAY_TIMEOUT,
                "timed out waiting for the emulator loop".into(),
            )),
        }
    }

    async fn call_json(&self, cmd: Cmd, timeout: Duration) -> ApiResult {
        match self.call(cmd, timeout).await? {
            Reply::Json(v) => Ok(axum::Json(v).into_response()),
            _ => Err(ApiError(StatusCode::INTERNAL_SERVER_ERROR, "unexpected reply".into())),
        }
    }
}

fn parse_body(body: &Bytes) -> Result<Value, ApiError> {
    if body.is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    serde_json::from_slice(body).map_err(|e| bad(format!("invalid JSON body: {}", e)))
}

fn from_value<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, ApiError> {
    serde_json::from_value(v).map_err(|e| bad(e.to_string()))
}

fn text_response(s: String) -> Response {
    ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], s).into_response()
}

fn encode_png(rgb: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, video::SCREEN_WIDTH, video::SCREEN_HEIGHT);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let mut w = enc.write_header().map_err(|e| e.to_string())?;
        w.write_image_data(rgb).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// HTTP handlers
// ---------------------------------------------------------------------------

async fn help() -> Response {
    text_response(HELP.to_string())
}

async fn status(State(s): State<AppState>) -> ApiResult {
    s.call_json(Cmd::Status, DEFAULT_TIMEOUT).await
}

#[derive(Deserialize)]
struct FormatQuery {
    format: Option<String>,
}

async fn screenshot(State(s): State<AppState>, Query(q): Query<FormatQuery>) -> ApiResult {
    let Reply::Frame(rgb) = s.call(Cmd::Screenshot, DEFAULT_TIMEOUT).await? else {
        return Err(ApiError(StatusCode::INTERNAL_SERVER_ERROR, "unexpected reply".into()));
    };
    match q.format.as_deref().unwrap_or("png") {
        "png" => {
            let png = tokio::task::spawn_blocking(move || encode_png(&rgb))
                .await
                .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
                .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e))?;
            Ok(([(header::CONTENT_TYPE, "image/png")], png).into_response())
        }
        "raw" => Ok((
            [
                (header::CONTENT_TYPE, "application/octet-stream".to_string()),
                (header::HeaderName::from_static("x-width"), video::SCREEN_WIDTH.to_string()),
                (header::HeaderName::from_static("x-height"), video::SCREEN_HEIGHT.to_string()),
                (header::HeaderName::from_static("x-pixel-format"), "RGB24".to_string()),
            ],
            rgb,
        )
            .into_response()),
        f => Err(bad(format!("unknown format '{}' (png, raw)", f))),
    }
}

async fn screen_text(State(s): State<AppState>, Query(q): Query<FormatQuery>) -> ApiResult {
    let Reply::Json(v) = s.call(Cmd::ScreenText, DEFAULT_TIMEOUT).await? else {
        return Err(ApiError(StatusCode::INTERNAL_SERVER_ERROR, "unexpected reply".into()));
    };
    if q.format.as_deref() == Some("text") {
        let lines: Vec<&str> = v["lines"].as_array().into_iter().flatten().filter_map(|l| l.as_str()).collect();
        return Ok(text_response(lines.join("\n") + "\n"));
    }
    Ok(axum::Json(v).into_response())
}

async fn trace_get(State(s): State<AppState>, Query(q): Query<TraceQuery>) -> ApiResult {
    let json = q.format.as_deref() == Some("json");
    let Reply::Trace(entries) = s.call(Cmd::TraceQuery(q), DEFAULT_TIMEOUT).await? else {
        return Err(ApiError(StatusCode::INTERNAL_SERVER_ERROR, "unexpected reply".into()));
    };
    let body = tokio::task::spawn_blocking(move || format_trace(&entries, json))
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(body)
}

fn format_trace(entries: &[TraceEntry], json: bool) -> Response {
    if json {
        let v: Vec<Value> = entries.iter().map(|e| e.to_json()).collect();
        axum::Json(json!({"count": v.len(), "entries": v})).into_response()
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

#[derive(Deserialize)]
struct TracePost {
    enabled: Option<bool>,
    #[serde(default)]
    clear: bool,
    stream_max: Option<usize>,
}

async fn trace_post(State(s): State<AppState>, body: Bytes) -> ApiResult {
    let p: TracePost = from_value(parse_body(&body)?)?;
    s.call_json(Cmd::TraceControl { enabled: p.enabled, clear: p.clear, stream_max: p.stream_max }, DEFAULT_TIMEOUT)
        .await
}

#[derive(Deserialize)]
struct LogQuery {
    since_ms: Option<u64>,
    limit: Option<usize>,
    grep: Option<String>,
    format: Option<String>,
}

async fn log_get(State(s): State<AppState>, Query(q): Query<LogQuery>) -> ApiResult {
    let lines: Vec<super::LogLine> = {
        let log = s.shared.log.lock().map_err(|_| bad("log poisoned"))?;
        let mut v: Vec<_> = log
            .iter()
            .rev()
            .filter(|l| q.since_ms.is_none_or(|t| l.t_ms >= t))
            .filter(|l| q.grep.as_deref().is_none_or(|g| l.line.contains(g)))
            .take(q.limit.unwrap_or(500))
            .cloned()
            .collect();
        v.reverse();
        v
    };
    if q.format.as_deref() == Some("text") {
        let s: String = lines.iter().map(|l| format!("{:>10} {}\n", l.t_ms, l.line)).collect();
        return Ok(text_response(s));
    }
    Ok(axum::Json(json!({"lines": lines})).into_response())
}

#[derive(Deserialize)]
struct WaitQuery {
    wait: Option<bool>,
}

/// Parse an input body: a single event object, an array of events, or
/// `{"events": [...]}`. `default_type` is injected into objects lacking a
/// `type` field (so `/api/input/key` accepts `{"key": "enter"}`).
fn parse_events(v: Value, default_type: Option<&str>) -> Result<Vec<InputEvent>, ApiError> {
    let list = match v {
        Value::Array(a) => a,
        Value::Object(mut o) if o.contains_key("events") => match o.remove("events") {
            Some(Value::Array(a)) => a,
            _ => return Err(bad("'events' must be an array")),
        },
        other => vec![other],
    };
    list.into_iter()
        .map(|mut ev| {
            if let (Some(t), Value::Object(o)) = (default_type, &mut ev) {
                o.entry("type").or_insert_with(|| Value::String(t.to_string()));
            }
            from_value(ev)
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
        _ => return Err(ApiError(StatusCode::NOT_FOUND, format!("unknown input kind '{}'", kind))),
    };
    let events = parse_events(parse_body(&body)?, default_type)?;
    let wait = q.wait.unwrap_or(true);
    s.call_json(Cmd::Input { events, wait }, if wait { INPUT_WAIT_TIMEOUT } else { DEFAULT_TIMEOUT }).await
}

async fn input_clear(State(s): State<AppState>) -> ApiResult {
    s.call_json(Cmd::InputClear, DEFAULT_TIMEOUT).await
}

async fn drives(State(s): State<AppState>) -> ApiResult {
    s.call_json(Cmd::Drives, DEFAULT_TIMEOUT).await
}

#[derive(Deserialize)]
struct MountBody {
    path: String,
    /// floppy, hdd or cdrom
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    read_only: bool,
}

async fn mount(State(s): State<AppState>, Path(letter): Path<String>, body: Bytes) -> ApiResult {
    let b: MountBody = from_value(parse_body(&body)?)?;
    let cmd = Cmd::Mount {
        drive: letter,
        path: b.path,
        kind: b.kind,
        label: b.label,
        read_only: b.read_only,
    };
    s.call_json(cmd, DEFAULT_TIMEOUT).await
}

async fn unmount(State(s): State<AppState>, Path(letter): Path<String>) -> ApiResult {
    s.call_json(Cmd::Unmount { drive: letter }, DEFAULT_TIMEOUT).await
}

#[derive(Deserialize)]
struct ControlBody {
    count: Option<u64>,
    until: Option<String>,
    timeout_ms: Option<u64>,
}

async fn control(State(s): State<AppState>, Path(action): Path<String>, body: Bytes) -> ApiResult {
    let b: ControlBody = from_value(parse_body(&body)?)?;
    let timeout = b.timeout_ms.map_or(DEFAULT_TIMEOUT, Duration::from_millis);
    match action.as_str() {
        "pause" => s.call_json(Cmd::Pause, DEFAULT_TIMEOUT).await,
        "resume" | "continue" => s.call_json(Cmd::Resume { until: b.until }, DEFAULT_TIMEOUT).await,
        "step" => s.call_json(Cmd::Step { count: b.count.unwrap_or(1) }, timeout).await,
        "reboot_shell" => s.call_json(Cmd::RebootShell, DEFAULT_TIMEOUT).await,
        _ => Err(ApiError(
            StatusCode::NOT_FOUND,
            format!("unknown action '{}' (pause, resume, step, reboot_shell)", action),
        )),
    }
}

#[derive(Deserialize)]
struct TimeoutQuery {
    timeout_ms: Option<u64>,
}

async fn wait_pause(State(s): State<AppState>, Query(q): Query<TimeoutQuery>) -> ApiResult {
    let t = Duration::from_millis(q.timeout_ms.unwrap_or(30_000));
    s.call_json(Cmd::WaitPause, t).await
}

async fn regs_get(State(s): State<AppState>) -> ApiResult {
    s.call_json(Cmd::GetRegs, DEFAULT_TIMEOUT).await
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
    let Reply::Bytes { addr, segoff, data } = s.call(Cmd::ReadMem { addr: q.addr, len }, DEFAULT_TIMEOUT).await? else {
        return Err(ApiError(StatusCode::INTERNAL_SERVER_ERROR, "unexpected reply".into()));
    };
    match q.format.as_deref().unwrap_or("hex") {
        "hex" => Ok(text_response(hexdump(addr, segoff, &data))),
        "base64" => Ok(axum::Json(json!({
            "addr": format!("{:05X}", addr),
            "len": data.len(),
            "base64": base64::engine::general_purpose::STANDARD.encode(&data),
        }))
        .into_response()),
        "raw" | "bin" => Ok(([(header::CONTENT_TYPE, "application/octet-stream")], data).into_response()),
        f => Err(bad(format!("unknown format '{}' (hex, base64, raw)", f))),
    }
}

/// Classic DEBUG-style dump. Rows are labelled SEG:OFF when the request used
/// a segmented address, otherwise with the 5-digit linear address.
fn hexdump(base: usize, segoff: Option<(u16, u16)>, data: &[u8]) -> String {
    let mut out = String::new();
    for (i, chunk) in data.chunks(16).enumerate() {
        let label = match segoff {
            Some((seg, off)) => format!("{:04X}:{:04X}", seg, off.wrapping_add((i * 16) as u16)),
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
struct MemPut {
    addr: String,
    hex: Option<String>,
    base64: Option<String>,
}

async fn mem_put(State(s): State<AppState>, body: Bytes) -> ApiResult {
    let b: MemPut = from_value(parse_body(&body)?)?;
    let data = match (b.hex, b.base64) {
        (Some(h), None) => {
            let clean: String = h.chars().filter(|c| c.is_ascii_hexdigit()).collect();
            if clean.len() % 2 != 0 {
                return Err(bad("hex data must have an even number of digits"));
            }
            (0..clean.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&clean[i..i + 2], 16).unwrap())
                .collect()
        }
        (None, Some(b64)) => base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .map_err(|e| bad(format!("invalid base64: {}", e)))?,
        _ => return Err(bad("provide exactly one of 'hex' or 'base64'")),
    };
    s.call_json(Cmd::WriteMem { addr: b.addr, data }, DEFAULT_TIMEOUT).await
}

#[derive(Deserialize)]
struct DisasmQuery {
    addr: Option<String>,
    count: Option<usize>,
    format: Option<String>,
}

async fn disasm(State(s): State<AppState>, Query(q): Query<DisasmQuery>) -> ApiResult {
    let Reply::Json(v) = s.call(Cmd::Disasm { addr: q.addr, count: q.count.unwrap_or(20) }, DEFAULT_TIMEOUT).await?
    else {
        return Err(ApiError(StatusCode::INTERNAL_SERVER_ERROR, "unexpected reply".into()));
    };
    if q.format.as_deref() == Some("json") {
        return Ok(axum::Json(v).into_response());
    }
    let lines: Vec<&str> = v["lines"].as_array().into_iter().flatten().filter_map(|l| l.as_str()).collect();
    Ok(text_response(lines.join("\n") + "\n"))
}

async fn bp_list(State(s): State<AppState>) -> ApiResult {
    s.call_json(Cmd::ListBreakpoints, DEFAULT_TIMEOUT).await
}

#[derive(Deserialize)]
struct BpBody {
    addr: Option<String>,
}

async fn bp_add(State(s): State<AppState>, body: Bytes) -> ApiResult {
    let b: BpBody = from_value(parse_body(&body)?)?;
    let addr = b.addr.ok_or_else(|| bad("missing 'addr'"))?;
    s.call_json(Cmd::AddBreakpoint(addr), DEFAULT_TIMEOUT).await
}

async fn bp_remove(State(s): State<AppState>, Query(q): Query<BpBody>, body: Bytes) -> ApiResult {
    let b: BpBody = from_value(parse_body(&body)?)?;
    s.call_json(Cmd::RemoveBreakpoint(q.addr.or(b.addr)), DEFAULT_TIMEOUT).await
}

async fn ivt(State(s): State<AppState>) -> ApiResult {
    s.call_json(Cmd::Ivt, DEFAULT_TIMEOUT).await
}

// ---------------------------------------------------------------------------
// WebSockets
// ---------------------------------------------------------------------------

/// Forward a broadcast channel to a socket until either side closes.
/// Lagging receivers skip ahead and are told how many messages they missed.
async fn forward_text(mut socket: WebSocket, mut rxs: Vec<broadcast::Receiver<Arc<str>>>) {
    use broadcast::error::RecvError;
    loop {
        // Poll all receivers fairly; there are at most two.
        let msg = {
            let (a, rest) = rxs.split_at_mut(1);
            tokio::select! {
                m = a[0].recv() => m,
                m = async { match rest.first_mut() { Some(r) => r.recv().await, None => std::future::pending().await } } => m,
                incoming = socket.recv() => match incoming {
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                    _ => continue,
                },
            }
        };
        let text = match msg {
            Ok(m) => m.to_string(),
            Err(RecvError::Lagged(n)) => json!({"type": "lagged", "missed": n}).to_string(),
            Err(RecvError::Closed) => return,
        };
        if socket.send(Message::Text(text.into())).await.is_err() {
            return;
        }
    }
}

async fn ws_trace(State(s): State<AppState>, ws: WebSocketUpgrade) -> Response {
    // Subscribing to the trace channel is what turns streaming on; the main
    // loop checks receiver_count() every batch.
    let rxs = vec![s.shared.trace.subscribe(), s.shared.events.subscribe()];
    ws.on_upgrade(move |socket| forward_text(socket, rxs))
}

async fn ws_events(State(s): State<AppState>, ws: WebSocketUpgrade) -> Response {
    let rxs = vec![s.shared.events.subscribe()];
    ws.on_upgrade(move |socket| forward_text(socket, rxs))
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

async fn ws_screen(State(s): State<AppState>, Query(q): Query<ScreenQuery>, ws: WebSocketUpgrade) -> Response {
    let fps = q.fps.unwrap_or(5.0).clamp(0.1, 60.0);
    ws.on_upgrade(move |mut socket| async move {
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
            let rgb = match s.shared.frame.lock() {
                Ok(f) => f.clone(),
                Err(_) => return,
            };
            let png = match tokio::task::spawn_blocking(move || encode_png(&rgb)).await {
                Ok(Ok(p)) => p,
                _ => continue,
            };
            if socket.send(Message::Binary(png.into())).await.is_err() {
                return;
            }
        }
    })
}

async fn ws_audio(State(s): State<AppState>, ws: WebSocketUpgrade) -> Response {
    let mut rx = s.shared.audio.subscribe();
    ws.on_upgrade(move |mut socket| async move {
        use broadcast::error::RecvError;
        let header = json!({"type": "audio_format", "sample_rate": 44100, "channels": 1, "format": "s16le"});
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
    })
}

async fn ws_input(State(s): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |mut socket| async move {
        while let Some(Ok(msg)) = socket.recv().await {
            let text = match msg {
                Message::Text(t) => t.to_string(),
                Message::Close(_) => return,
                _ => continue,
            };
            let reply = match handle_ws_input(&s, &text).await {
                Ok(v) => v,
                Err(ApiError(_, e)) => json!({"ok": false, "error": e}),
            };
            if socket.send(Message::Text(reply.to_string().into())).await.is_err() {
                return;
            }
        }
    })
}

async fn handle_ws_input(s: &AppState, text: &str) -> Result<Value, ApiError> {
    let mut v: Value = serde_json::from_str(text).map_err(|e| bad(format!("invalid JSON: {}", e)))?;
    let wait = v.get("wait").and_then(|w| w.as_bool()).unwrap_or(false);
    if let Value::Object(o) = &mut v {
        o.remove("wait");
    }
    let events = parse_events(v, None)?;
    let timeout = if wait { INPUT_WAIT_TIMEOUT } else { DEFAULT_TIMEOUT };
    match s.call(Cmd::Input { events, wait }, timeout).await? {
        Reply::Json(v) => Ok(v),
        _ => Err(ApiError(StatusCode::INTERNAL_SERVER_ERROR, "unexpected reply".into())),
    }
}

const HELP: &str = r#"rust-dos debug interface
========================
All endpoints are local-only and unauthenticated. JSON in, JSON out unless noted.
Addresses are hex: "SEG:OFF" (registers allowed, e.g. "CS:IP", "DS:SI", "B800:0")
or a linear address ("0x12345", "B8000").

STATUS / SCREEN
  GET  /api/status                         emulator state, CS:IP, video mode, trace fill, fps
  GET  /api/screenshot[?format=png|raw]    composited 640x400 frame (raw = RGB24 bytes)
  GET  /api/screen/text[?format=text]      text-mode screen contents (CP437 -> Unicode)
  GET  /api/log[?since_ms=&limit=&grep=&format=text]   emulator log lines

TRACE
  POST /api/trace  {"enabled":true, "clear":false, "stream_max":1000}
  GET  /api/trace?last_n=200 | ?last_ms=500 | ?from_ms=&to_ms=  [&limit=&cs=&no_bios=true&format=text|json]
       Instruction trace (registers shown are BEFORE execution). Timestamps are
       ms since emulator start, sampled per ~16 ms batch; icount orders exactly.

INPUT  (append ?wait=false to return immediately instead of after delivery)
  POST /api/input/key    {"key":"enter", "action":"press|down|up", "mods":["ctrl"], "hold_ms":50}
                         or {"scancode":30, "ascii":97}
  POST /api/input/type   {"text":"dir\n", "delay_ms":0}
  POST /api/input/mouse  {"action":"move|down|up|click", "x":320, "y":200, "dx":0, "dy":0,
                          "button":"left|right|middle", "coords":"screen|virtual"}
                         screen coords = pixels of the 640x400 screenshot (default)
  POST /api/input/wait   {"ms":500}
  POST /api/input/batch  [{"type":"type","text":"cd games\n"},{"type":"wait","ms":300},
                          {"type":"key","key":"f1"}]
  DELETE /api/input      clear pending queued input

EXECUTION CONTROL
  POST /api/control/pause
  POST /api/control/resume   {"until":"1234:0100"}   (optional temporary breakpoint)
  POST /api/control/step     {"count":1}             returns registers after stepping
  POST /api/control/reboot_shell                     kill the running program
  GET  /api/control/wait?timeout_ms=30000            block until the emulator pauses
  GET  /api/registers        PUT /api/registers {"ax":"1234","flags":"0202"}
  GET  /api/memory?addr=DS:SI&len=256[&format=hex|base64|raw]
  PUT  /api/memory {"addr":"B800:0000","hex":"41 1F"}  (or "base64")
  GET  /api/disasm?addr=CS:IP&count=20[&format=json]
  GET/POST/DELETE /api/breakpoints   {"addr":"1234:0100"}  (DELETE without addr = all)
  GET  /api/ivt              interrupt vector table

DRIVES
  GET    /api/drive                                list drives, types and paths
  PUT    /api/drive/D {"path":"/home/me/dos/cd","type":"cdrom","label":"GAMECD"}
                   mount or replace a drive; type floppy|hdd|cdrom, optional
                   "read_only":true. Replacing closes that drive's open files;
                   with no options a remount keeps the drive's type and label.
  DELETE /api/drive/D                              unmount (not C: or Z:)

WEBSOCKETS
  /ws/events          JSON: log lines, paused/resumed, video_mode changes
  /ws/trace           JSON: per-frame batches {"type":"trace","dropped":N,"entries":[...]} + events
  /ws/screen?fps=5    binary PNG frames, sent only when the screen changes
  /ws/audio           text header, then binary s16le 44100 Hz mono chunks
  /ws/input           send input events (same JSON as /api/input/batch, optional "wait":true)
"#;
