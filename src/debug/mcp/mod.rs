//! Optional native MCP composition; game observations are supplied by control.
use super::control::{Busy, CapturePolicy, ControlClient};
use axum::{Router, routing::any};
use base64::Engine;
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock as Content,
        Implementation, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig,
        Tool, ToolAnnotations,
    },
    service::RequestContext,
    transport::streamable_http_server::StreamableHttpServerConfig,
};
use rust_dos::control::{ObservationContext, ObservationService};
use serde_json::{Value, json};
use std::{
    net::SocketAddr,
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

mod diagnostics;
mod keyboard;
#[cfg(test)]
pub(crate) mod metrics;
mod mouse;
mod playback;
mod schemas;
mod sessions;
pub(super) fn install_diagnostics() -> Result<(), String> {
    diagnostics::install()
}
mod transport;
use sessions::{DeliveryTicket, Sessions};
use transport::{CaptureHolder, Deadline, Limits, NativeState};

#[derive(Clone)]
struct NativeObservations {
    control: ControlClient,
    limits: Arc<Limits>,
}

impl ServerHandler for NativeObservations {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("rust-dos", env!("CARGO_PKG_VERSION")))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            tools: tools(),
            ..Default::default()
        })
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        tools().into_iter().find(|tool| tool.name == name)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if matches!(request.name.as_ref(), "speed" | "fast_forward") {
            let action = playback::parse(&request)
                .inspect_err(|_| self.control.shared.metrics.rejected())?;
            let _busy = Busy::new(&self.control.shared);
            let deadline = context
                .extensions
                .get::<axum::http::request::Parts>()
                .and_then(|parts| parts.extensions.get::<Deadline>())
                .ok_or_else(|| ErrorData::internal_error("native deadline missing", None))?;
            let result = playback::call(&self.control, action, deadline.0).await;
            if result.is_error == Some(true) {
                self.control.shared.metrics.rejected();
            }
            return Ok(result.into());
        }
        if request.name == "operation" {
            use rust_dos::control::OperationQuery;
            let id = keyboard::operation(&request)
                .inspect_err(|_| self.control.shared.metrics.rejected())?;
            let _busy = Busy::new(&self.control.shared);
            let deadline = context
                .extensions
                .get::<axum::http::request::Parts>()
                .and_then(|parts| parts.extensions.get::<Deadline>())
                .ok_or_else(|| ErrorData::internal_error("native deadline missing", None))?;
            let result = match self.control.inputs(deadline.0).operation(id).await {
                Ok(operation) => {
                    let value = keyboard::operation_value(operation);
                    let mut result =
                        CallToolResult::success(vec![Content::text(value.to_string())]);
                    result.structured_content = Some(value);
                    result
                }
                Err(error) => {
                    self.control.shared.metrics.rejected();
                    operation_error(error)
                }
            };
            return Ok(result.into());
        }
        if matches!(request.name.as_ref(), "key" | "type" | "mouse") {
            use rust_dos::control::GuestInputService;
            let decoded = if request.name == "mouse" {
                mouse::parse(&request)
            } else {
                keyboard::parse(&request)
            };
            let input = decoded.inspect_err(|_| self.control.shared.metrics.rejected())?;
            let _busy = Busy::new(&self.control.shared);
            let deadline = context
                .extensions
                .get::<axum::http::request::Parts>()
                .and_then(|parts| parts.extensions.get::<Deadline>())
                .ok_or_else(|| ErrorData::internal_error("native deadline missing", None))?;
            let result = match self.control.inputs(deadline.0).input(input).await {
                Ok(outcome) => {
                    let value = keyboard::value(outcome);
                    let mut result =
                        CallToolResult::success(vec![Content::text(value.to_string())]);
                    result.structured_content = Some(value);
                    result
                }
                Err(error) => {
                    self.control.shared.metrics.rejected();
                    input_error(error)
                }
            };
            return Ok(result.into());
        }
        let capture_requested = match request.name.as_ref() {
            "status" => false,
            "capture" => true,
            _ => {
                self.control.shared.metrics.rejected();
                return Err(ErrorData::invalid_params("unknown native tool", None));
            }
        };
        if request
            .arguments
            .as_ref()
            .is_some_and(|arguments| !arguments.is_empty())
        {
            self.control.shared.metrics.rejected();
            return Err(ErrorData::invalid_params(
                "observations accept no arguments",
                None,
            ));
        }
        let capture_permit = if capture_requested {
            let Ok(permit) = self.limits.captures.clone().try_acquire_owned() else {
                self.control.shared.metrics.rejected();
                return Ok(failure(
                    "capture_busy",
                    "Capture capacity is occupied.",
                    "observe_again",
                )
                .into());
            };
            self.control
                .shared
                .metrics
                .capture_started(&self.limits.captures);
            let permit = Arc::new(permit);
            let holder = context
                .extensions
                .get::<axum::http::request::Parts>()
                .and_then(|parts| parts.extensions.get::<CaptureHolder>())
                .cloned()
                .ok_or_else(|| ErrorData::internal_error("native capture context missing", None))?;
            if !holder.retain(permit.clone()) {
                self.control.shared.metrics.rejected();
                return Ok(failure(
                    "capture_closed",
                    "The capture request closed before completion.",
                    "observe_again",
                )
                .into());
            }
            if let Some(ticket) = context.extensions.get::<DeliveryTicket>() {
                ticket
                    .capture
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .replace(permit.clone());
            }
            Some(permit)
        } else {
            None
        };
        let _busy = Busy::new(&self.control.shared);
        let mut observations = self.control.observations();
        let deadline = context
            .extensions
            .get::<axum::http::request::Parts>()
            .and_then(|parts| parts.extensions.get::<Deadline>())
            .copied()
            .ok_or_else(|| ErrorData::internal_error("native deadline missing", None))?;
        observations = observations.with_deadline(deadline.0);
        if let Some(permit) = &capture_permit {
            observations = observations.with_capture(CapturePolicy {
                deadline: deadline.0,
                _retention: permit.clone(),
            });
        }
        let outcome = match capture_requested {
            false => observations.status().await.map(|status| {
                let mut value = metadata(&status.context);
                value["paused"] = status.paused.into();
                value["program"] = status.program.into();
                value["instructions"] = status.instructions.into();
                value["video"] = json!({"width":status.picture.width,"height":status.picture.height});
                value["mouse"] = json!({"installed":status.mouse.installed,"buttons":status.mouse.buttons,"x":status.mouse.x,"y":status.mouse.y});
                let mut result = CallToolResult::success(vec![Content::text(value.to_string())]);
                result.structured_content = Some(value);
                result
            }),
            true => {
                match observations.capture().await {
                    Ok(capture) => {
                        let mut value = metadata(&capture.context);
                        value["width"] = capture.picture.width.into();
                        value["height"] = capture.picture.height.into();
                        let frame = crate::video::Frame::from_rgb(capture.picture.width, capture.picture.height, capture.rgb);
                        let metrics = self.control.shared.clone();
                        match tokio::task::spawn_blocking(move || {
                            let _encoding_timing = metrics.metrics.encoding.start();
                            let _capture_permit = capture_permit;
                            encode_capture(&frame, deadline.0, 13 * 1024 * 1024)
                        }).await {
                            Ok(Ok(encoded)) => {
                                let mut result = CallToolResult::success(vec![Content::image(encoded, "image/png"), Content::text(value.to_string())]);
                                result.structured_content = Some(value);
                                Ok(result)
                            }
                            Ok(Err(error)) => Err(error),
                            Err(_) => Err(rust_dos::control::ControlError::Unavailable),
                        }
                    }
                    Err(error) => Err(error),
                }
            }
        };
        if outcome.is_err() {
            self.control.shared.metrics.rejected();
        }
        tracing::info!(target: diagnostics::TARGET,
            tool = request.name.as_ref(),
            instance = self.control.shared.instance_hex(),
            request = observations.request_id(),
            success = outcome.is_ok(),
            "observation completed");
        Ok(outcome.unwrap_or_else(control_error).into())
    }
}

fn metadata(context: &ObservationContext) -> Value {
    let mut value = json!({"instance":context.instance,"req":context.request,"state_seq":context.state_seq,
        "journal_seq":context.journal_seq,"stop_seq":context.stop_seq});
    if let Some(picture) = &context.mouse_picture {
        value["mouse_picture"] = json!({"instance":picture.instance,"mode_epoch":picture.mode_epoch,
            "width":picture.size.width,"height":picture.size.height});
    }
    value
}

struct PngSink {
    bytes: Vec<u8>,
    limit: usize,
}

impl std::io::Write for PngSink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let size = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .ok_or_else(|| std::io::Error::other("native PNG size overflow"))?;
        if size > self.limit {
            return Err(std::io::Error::other("native PNG limit"));
        }
        self.bytes
            .try_reserve_exact(bytes.len())
            .map_err(std::io::Error::other)?;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(super) fn encode_capture(
    frame: &crate::video::Frame,
    deadline: Instant,
    max_png: usize,
) -> Result<String, rust_dos::control::ControlError> {
    use rust_dos::control::ControlError;
    if Instant::now() >= deadline {
        return Err(ControlError::Timeout);
    }
    let mut sink = PngSink {
        bytes: Vec::new(),
        limit: max_png,
    };
    rust_dos::capture::png::encode_to(frame, &mut sink).map_err(|_| ControlError::Rejected(413))?;
    encode_png_payload(sink.bytes, deadline)
}

fn encode_png_payload(
    bytes: Vec<u8>,
    deadline: Instant,
) -> Result<String, rust_dos::control::ControlError> {
    use rust_dos::control::ControlError;
    if Instant::now() >= deadline {
        return Err(ControlError::Timeout);
    }
    let base64_size = bytes
        .len()
        .checked_add(2)
        .and_then(|size| (size / 3).checked_mul(4))
        .ok_or(ControlError::Rejected(413))?;
    if base64_size
        .checked_add(1024 * 1024)
        .is_none_or(|size| size > 24 * 1024 * 1024)
    {
        return Err(ControlError::Rejected(413));
    }
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn native_negative_capture_contexts_report_real_retention_and_backend_failure() {
        use rmcp::{
            ServiceExt,
            model::{ClientConfig, ProtocolVersion},
        };
        let (tx, receive) = std::sync::mpsc::channel();
        drop(receive);
        let shared = Arc::new(crate::debug::Shared::new(Instant::now()));
        let handler = NativeObservations {
            control: ControlClient {
                tx,
                shared: shared.clone(),
            },
            limits: Arc::new(Limits::default()),
        };
        let (client_io, server_io) = tokio::io::duplex(8192);
        let serving = tokio::spawn(handler.clone().serve(server_io));
        let client = ClientConfig::default()
            .with_protocol_version(ProtocolVersion::V_2025_11_25)
            .serve(client_io)
            .await
            .unwrap();
        let server = serving.await.unwrap().unwrap();
        for closed in [true, false] {
            let holder = CaptureHolder::default();
            if closed {
                holder.close_for_test();
            }
            let (mut parts, _) = axum::http::Request::new(()).into_parts();
            parts.extensions.insert(holder);
            parts
                .extensions
                .insert(Deadline(Instant::now() + Duration::from_secs(2)));
            let mut context =
                RequestContext::new(rmcp::model::RequestId::Number(1), server.peer().clone());
            context.extensions.insert(parts);
            let response = handler
                .call_tool(CallToolRequestParams::new("capture"), context)
                .await
                .unwrap();
            let CallToolResponse::Complete(result) = response else {
                panic!("negative capture must finish with a real tool failure");
            };
            assert_eq!(result.is_error, Some(true));
            assert_eq!(
                result.structured_content.unwrap()["error"]["code"],
                if closed {
                    "capture_closed"
                } else {
                    "instance_closed"
                }
            );
            assert_eq!(
                handler.limits.captures.available_permits(),
                1,
                "failed context does not retain capture capacity"
            );
        }
        assert_eq!(shared.metrics.snapshot()["rejected_requests"], 2);
        client.cancel().await.unwrap();
        server.cancel().await.unwrap();
    }

    #[tokio::test]
    async fn native_reaper_exits_when_composition_is_disposed() {
        let (tx, _receive) = std::sync::mpsc::channel();
        let state = Arc::new(NativeState {
            control: ControlClient {
                tx,
                shared: Arc::new(crate::debug::Shared::new(Instant::now())),
            },
            limits: Arc::new(Limits::default()),
            sessions: Arc::new(Sessions::default()),
            config: StreamableHttpServerConfig::default(),
        });
        let weak = Arc::downgrade(&state);
        let reaper = spawn_reaper(&state);
        drop(state);
        assert!(
            weak.upgrade().is_none(),
            "reaper never retains disposed native composition"
        );
        tokio::time::timeout(Duration::from_secs(2), reaper)
            .await
            .unwrap()
            .unwrap();
    }

    #[test]
    fn native_base64_stage_rejects_deadline_expired_after_png() {
        let png = crate::debug::server::http_tests::native_guard_picture();
        assert_eq!(
            encode_png_payload(png, Instant::now() - Duration::from_millis(1)),
            Err(rust_dos::control::ControlError::Timeout)
        );
    }
}

fn failure(code: &str, message: &str, recovery: &str) -> CallToolResult {
    CallToolResult::structured_error(
        json!({"error":{"code":code,"message":message,"recovery":recovery}}),
    )
}

fn control_error(error: rust_dos::control::ControlError) -> CallToolResult {
    use rust_dos::control::ControlError;
    let code = match error {
        ControlError::Closed => "instance_closed",
        ControlError::Timeout => "deadline_expired",
        ControlError::Unavailable => "backend_unavailable",
        ControlError::Rejected(_) => "request_rejected",
    };
    failure(code, &error.to_string(), "observe_again")
}

fn operation_error(error: rust_dos::control::OperationError) -> CallToolResult {
    use rust_dos::control::OperationError;
    match error {
        OperationError::Backend(error) => control_error(error),
        OperationError::StaleInstance => failure(
            "stale_instance",
            "The operation belongs to a replaced DOS instance.",
            "observe_again",
        ),
        OperationError::Unknown => failure(
            "unknown_operation",
            "No input operation has this ID.",
            "check_operation_id",
        ),
        OperationError::Evicted => failure(
            "evicted_operation",
            "The bounded operation history no longer retains this ID.",
            "observe_again",
        ),
    }
}

fn input_error(error: rust_dos::control::InputError) -> CallToolResult {
    use rust_dos::control::InputError;
    let (code, message, recovery) = match error {
        InputError::StalePicture => (
            "stale_picture",
            "The mouse picture no longer matches the running guest.",
            "capture_again",
        ),
        InputError::StaleInstance => (
            "stale_instance",
            "The operation belongs to a replaced DOS instance.",
            "capture_again",
        ),
        InputError::UnknownHold => (
            "unknown_hold",
            "The hold handle is not active in this instance.",
            "inspect_operation",
        ),
        InputError::ReleasedHold => (
            "released_hold",
            "The hold has already been released.",
            "inspect_operation",
        ),
        InputError::ExpiredHold => (
            "expired_hold",
            "The hold lease has expired.",
            "inspect_operation",
        ),
        InputError::ResetHold => (
            "reset_hold",
            "Guest replacement or reset ended the hold.",
            "capture_again",
        ),
        InputError::Capacity => (
            "capacity",
            "The bounded input capacity is occupied.",
            "inspect_operation",
        ),
        InputError::ExpiredBeforeAcceptance => (
            "expired_before_acceptance",
            "The request expired before input acceptance.",
            "capture_again",
        ),
        InputError::Invalid(message) => {
            return failure("invalid_input", &message, "correct_arguments");
        }
        InputError::Conflict(_) => (
            "input_conflict",
            "Input conflicts with an existing reservation.",
            "inspect_operation",
        ),
        InputError::Backend(_) => (
            "backend_unavailable",
            "The input service could not complete the request.",
            "inspect_operation",
        ),
    };
    failure(code, message, recovery)
}

fn tools() -> Vec<Tool> {
    let schema = json!({"type":"object","additionalProperties":false})
        .as_object()
        .unwrap()
        .clone();
    let mut tools = vec![
        Tool::new(
            "status",
            "Observe the running DOS machine and driver state",
            schema.clone(),
        ),
        Tool::new(
            "capture",
            "Capture the actual DOS picture as a PNG image",
            schema,
        ),
    ];
    tools.extend(keyboard::tools());
    tools.push(mouse::tool());
    tools.extend(playback::tools());
    for tool in &mut tools {
        tool.output_schema = Some(Arc::new(
            schemas::output(&tool.name)
                .as_object()
                .expect("output schema object")
                .clone(),
        ));
        let read_only = matches!(tool.name.as_ref(), "status" | "capture" | "operation");
        tool.annotations = Some(ToolAnnotations::from_raw(
            None,
            Some(read_only),
            (!read_only).then_some(true),
            (!read_only).then_some(false),
            Some(false),
        ));
    }
    tools
}

pub(super) fn attach(router: Router, control: ControlClient, address: SocketAddr) -> Router {
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(true)
        .with_json_response(true)
        .with_allowed_hosts([address.to_string()])
        .enforce_origin_validation();
    let state = Arc::new(NativeState {
        control,
        limits: Arc::new(Limits::default()),
        sessions: Arc::new(Sessions::default()),
        config,
    });
    spawn_reaper(&state);
    router.merge(
        Router::new()
            .route("/mcp", any(transport::handle))
            .with_state(state),
    )
}

fn spawn_reaper(state: &Arc<NativeState>) -> tokio::task::JoinHandle<()> {
    let weak = Arc::downgrade(state);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let Some(state) = weak.upgrade() else { break };
            if state.control.shared.closed.load(Ordering::Acquire) {
                state.limits.requests.close();
                state.limits.captures.close();
                state.sessions.shutdown().await;
                state.config.cancellation_token.cancel();
                break;
            }
            let _ = state.sessions.reclaim_expired().await;
        }
    })
}
