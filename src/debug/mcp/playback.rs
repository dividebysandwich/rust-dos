//! Strict MCP mapping onto the existing frontend request queue.
#![deny(clippy::all)]
use super::super::{
    Cmd, Reply,
    control::{ControlClient, SubmissionError},
};
use crate::playback::{Action, FastForwardAction, MAX_FAST_FORWARD_MS};
use rmcp::{
    ErrorData,
    model::{CallToolRequestParams, CallToolResult, ContentBlock, Tool},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpeedArgs {
    cycles: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FastForwardArgs {
    action: String,
    duration_ms: Option<u64>,
}

pub(super) fn parse(request: &CallToolRequestParams) -> Result<Action, ErrorData> {
    let arguments = request.arguments.clone().unwrap_or_default();
    let invalid = || ErrorData::invalid_params("invalid execution control arguments", None);
    if arguments.values().any(Value::is_null) {
        return Err(invalid());
    }
    match request.name.as_ref() {
        "speed" => {
            let args: SpeedArgs =
                serde_json::from_value(Value::Object(arguments)).map_err(|_| invalid())?;
            if let Some(cycles) = &args.cycles {
                if cycles.len() > 32 {
                    return Err(invalid());
                }
                crate::timer::CpuSpeed::parse(cycles).map_err(|_| invalid())?;
            }
            Ok(Action::Speed(args.cycles))
        }
        "fast_forward" => {
            let args: FastForwardArgs =
                serde_json::from_value(Value::Object(arguments)).map_err(|_| invalid())?;
            let action = match (args.action.as_str(), args.duration_ms) {
                ("get", None) => FastForwardAction::Get,
                ("stop", None) => FastForwardAction::Stop,
                ("start", Some(ms @ 1..=MAX_FAST_FORWARD_MS)) => {
                    FastForwardAction::Start(Duration::from_millis(ms))
                }
                _ => return Err(invalid()),
            };
            Ok(Action::FastForward(action))
        }
        _ => Err(invalid()),
    }
}

pub(super) async fn call(
    control: &ControlClient,
    action: Action,
    deadline: Instant,
) -> CallToolResult {
    let mut context = control.context(None, None);
    context.deadline = Some(deadline);
    match control
        .submit(&context, Cmd::Playback { action }, Duration::from_secs(1))
        .await
    {
        Ok(answer) => match answer.reply {
            Reply::Json(value) => {
                let mut result =
                    CallToolResult::success(vec![ContentBlock::text(value.to_string())]);
                result.structured_content = Some(value);
                result
            }
            Reply::Error(408, _) => super::failure(
                "deadline_expired",
                "The frontend request expired; inspect the current setting before retrying.",
                "observe_again",
            ),
            Reply::Error(503, _) => super::control_error(rust_dos::control::ControlError::Closed),
            Reply::Error(_, message) => {
                super::failure("request_rejected", &message, "observe_again")
            }
            Reply::ErrorJson(_, _) => super::failure(
                "request_rejected",
                "The current machine state does not permit this change.",
                "observe_again",
            ),
            _ => super::failure(
                "backend_unavailable",
                "The frontend returned an unexpected reply.",
                "observe_again",
            ),
        },
        Err(error) => {
            let error = match error {
                SubmissionError::Closed => rust_dos::control::ControlError::Closed,
                SubmissionError::Dropped => rust_dos::control::ControlError::Unavailable,
                SubmissionError::Timeout => rust_dos::control::ControlError::Timeout,
                SubmissionError::Busy => rust_dos::control::ControlError::Rejected(503),
            };
            super::control_error(error)
        }
    }
}

pub(super) fn tools() -> [Tool; 2] {
    [
        Tool::new("speed", "Read or set CPU throughput. Omit cycles to read; set auto, auto N, max, or fixed cycles per millisecond. Deterministic mode permits fixed changes only while paused. Inspect current settings after an uncertain reply.", json!({"type":"object","additionalProperties":false,"properties":{"cycles":{"type":"string","minLength":1,"maxLength":32}}}).as_object().unwrap().clone()),
        Tool::new("fast_forward", "Read, start, or stop temporary emulated-time acceleration at the emulator's existing multiplier. Start requires a duration of 1..5000 ms and a running real-time guest. Expiry, pause, restore, focus loss or shutdown cancels agent acceleration. Stop does not cancel manual acceleration; inspect state after uncertain acceptance.", json!({
            "type":"object","additionalProperties":false,"properties":{"action":{"enum":["get","start","stop"]},"duration_ms":{"type":"integer","minimum":1,"maximum":MAX_FAST_FORWARD_MS}},"required":["action"],
            "oneOf":[{"properties":{"action":{"enum":["get","stop"]}},"not":{"required":["duration_ms"]}},{"properties":{"action":{"const":"start"}},"required":["duration_ms"]}]
        }).as_object().unwrap().clone()),
    ]
}

pub(super) fn output(name: &str) -> Value {
    let state = if name == "speed" {
        json!({"type":"object","additionalProperties":false,"properties":{"cycles":{"type":"string"},"previous_cycles":{"type":"string"},"cycles_per_ms":{"type":"integer","minimum":1}},"required":["cycles","previous_cycles","cycles_per_ms"]})
    } else {
        json!({"type":"object","additionalProperties":false,"properties":{"enabled":{"type":"boolean"},"agent_active":{"type":"boolean"},"remaining_ms":{"type":"integer","minimum":0,"maximum":MAX_FAST_FORWARD_MS},"multiplier":{"type":"integer","minimum":1}},"required":["enabled","agent_active","remaining_ms","multiplier"]})
    };
    json!({"oneOf":[state, {"type":"object","additionalProperties":false,"properties":{"error":{"type":"object","additionalProperties":false,"properties":{"code":{"type":"string"},"message":{"type":"string"},"recovery":{"type":"string"}},"required":["code","message","recovery"]}},"required":["error"]}]})
}
