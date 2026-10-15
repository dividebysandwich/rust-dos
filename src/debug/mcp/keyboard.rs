//! Native keyboard argument/response mapping; all effects belong to the actor.
#![deny(clippy::all)]
use rmcp::{
    ErrorData,
    model::{CallToolRequestParams, Tool},
};
use rust_dos::control::{GuestInput, HoldState, InputOutcome, InputWait, KeyChord};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyArgs {
    #[serde(default)]
    action: Option<String>,
    key: Option<String>,
    #[serde(default)]
    mods: Vec<String>,
    hold_ms: Option<u64>,
    lease_ms: Option<u64>,
    handle: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TextArgs {
    text: String,
    delay_ms: Option<u64>,
}
fn invalid() -> ErrorData {
    ErrorData::invalid_params("invalid keyboard arguments", None)
}

pub(super) fn parse(request: &CallToolRequestParams) -> Result<GuestInput, ErrorData> {
    let args = request.arguments.clone().unwrap_or_default();
    if args.values().any(Value::is_null) {
        return Err(invalid());
    }
    let args = Value::Object(args);
    if request.name == "dos_type" {
        let args: TextArgs = serde_json::from_value(args).map_err(|_| invalid())?;
        if args.text.len() > 4096
            || args.delay_ms.is_some_and(|delay| delay > 5000)
            || args.text.chars().any(|c| {
                !matches!(c, '\n' | '\r' | '\t' | '\u{8}')
                    && rust_dos::keylayout::cp437(c).is_none()
            })
        {
            return Err(invalid());
        }
        return Ok(GuestInput::Type {
            text: args.text,
            delay: Duration::from_millis(args.delay_ms.unwrap_or(0)),
        });
    }
    let args: KeyArgs = serde_json::from_value(args).map_err(|_| invalid())?;
    match args.action.as_deref().unwrap_or("press") {
        "up" | "renew" => {
            if args.key.is_some() || !args.mods.is_empty() || args.hold_ms.is_some() {
                return Err(invalid());
            }
            let handle =
                super::super::owned_input::parse_token(args.handle.as_deref().ok_or_else(invalid)?)
                    .ok_or_else(invalid)?;
            if args.action.as_deref() == Some("up") {
                if args.lease_ms.is_some() {
                    return Err(invalid());
                }
                Ok(GuestInput::Up { handle })
            } else {
                let lease = args
                    .lease_ms
                    .filter(|lease| (1..=30000).contains(lease))
                    .ok_or_else(invalid)?;
                Ok(GuestInput::Renew {
                    handle,
                    lease: Duration::from_millis(lease),
                })
            }
        }
        "press" | "down" => {
            if args.handle.is_some() || args.mods.len() > 4 {
                return Err(invalid());
            }
            let key = rust_dos::keyboard::lookup(args.key.as_deref().ok_or_else(invalid)?)
                .ok_or_else(invalid)?;
            let mut modifiers = Vec::new();
            for name in args.mods {
                let modifier = rust_dos::keyboard::lookup(&name)
                    .filter(|key| key.modifier != 0)
                    .ok_or_else(invalid)?;
                if modifiers.contains(&modifier) || modifier == key {
                    return Err(invalid());
                }
                modifiers.push(modifier);
            }
            let chord = KeyChord { key, modifiers };
            if args.action.as_deref() == Some("down") {
                if args.hold_ms.is_some() {
                    return Err(invalid());
                }
                let lease = args
                    .lease_ms
                    .filter(|lease| (1..=30000).contains(lease))
                    .ok_or_else(invalid)?;
                Ok(GuestInput::Down {
                    chord,
                    lease: Duration::from_millis(lease),
                })
            } else {
                if args.lease_ms.is_some() {
                    return Err(invalid());
                }
                let duration = args.hold_ms.unwrap_or(50);
                if !(1..=5000).contains(&duration) {
                    return Err(invalid());
                }
                Ok(GuestInput::Press {
                    chord,
                    duration: Duration::from_millis(duration),
                })
            }
        }
        _ => Err(invalid()),
    }
}

pub(super) fn value(outcome: InputOutcome) -> Value {
    match outcome {
        InputOutcome::Unknown { operation } => {
            json!({"instance":operation.instance,"op":operation.request,"acceptance":"unknown"})
        }
        InputOutcome::Accepted {
            operation,
            context,
            wait,
            held,
            hold,
        } => {
            let mut value = super::metadata(&context);
            value["op"] = operation.request.into();
            value["acceptance"] = "accepted".into();
            value["wait"] = match wait {
                InputWait::Pending => "pending",
                InputWait::Delivered => "delivered",
                InputWait::TimedOut => "timed_out",
            }
            .into();
            value["held"] = held.into();
            if let Some(hold) = hold {
                value["hold"] = json!({"handle":super::super::owned_input::token(&hold.handle),"state":match hold.state { HoldState::Reserved=>"reserved",HoldState::Delivered=>"delivered",HoldState::Released=>"released",HoldState::Expired=>"expired",HoldState::Reset=>"reset" },"remaining_ms":hold.remaining.as_millis() as u64});
            }
            value
        }
    }
}

pub(super) fn tools() -> Vec<Tool> {
    vec![
        Tool::new("dos_operation", "Observe input delivery and owned hold lifetime without sending input. Use the returned instance and operation IDs to inspect accepted input before continuing; queued input is not yet delivered.", json!({"type":"object","additionalProperties":false,"properties":{"instance":{"type":"string","pattern":"^[0-9a-f]{16}$"},"op":{"type":"integer","minimum":1}},"required":["instance","op"]}).as_object().unwrap().clone()),
        Tool::new("dos_key", "Send a canonical DOS key; default action is press. Down retains a key and modifiers with a renewable lease of at most 30 seconds. Use the returned opaque handle to renew or release explicitly. Input may run commands or write mounted drives. Acceptance is not delivery; inspect dos_operation. Do not replay input after uncertain acceptance.", json!({"type":"object","additionalProperties":false,"properties":{"action":{"type":"string","enum":["press","down","up","renew"]},"key":{"type":"string"},"mods":{"type":"array","items":{"type":"string"},"maxItems":4,"uniqueItems":true},"hold_ms":{"type":"integer","minimum":1,"maximum":5000},"lease_ms":{"type":"integer","minimum":1,"maximum":30000},"handle":{"type":"string","maxLength":50}}}).as_object().unwrap().clone()),
        Tool::new("dos_type", "Type supported text into the actual DOS guest, at most 4096 bytes. Input may run commands or write mounted drives. Acceptance is not delivery; inspect dos_operation before continuing. Do not replay text after uncertain acceptance.", json!({"type":"object","additionalProperties":false,"properties":{"text":{"type":"string","maxLength":4096},"delay_ms":{"type":"integer","minimum":0,"maximum":5000}},"required":["text"]}).as_object().unwrap().clone()),
    ]
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OperationArgs {
    instance: String,
    op: u64,
}

pub(super) fn operation(
    request: &CallToolRequestParams,
) -> Result<rust_dos::control::OperationId, ErrorData> {
    let args: OperationArgs =
        serde_json::from_value(Value::Object(request.arguments.clone().unwrap_or_default()))
            .map_err(|_| invalid())?;
    if args.instance.len() != 16
        || !args
            .instance
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || args.op == 0
    {
        return Err(invalid());
    }
    Ok(rust_dos::control::OperationId {
        instance: args.instance,
        request: args.op,
    })
}

pub(super) fn operation_value(operation: rust_dos::control::InputOperation) -> Value {
    let mut value = super::metadata(&operation.context);
    value["op"] = operation.operation.request.into();
    value["delivery"] = match operation.delivery {
        rust_dos::control::InputDelivery::Pending => "pending",
        rust_dos::control::InputDelivery::Delivered => "delivered",
        rust_dos::control::InputDelivery::Cancelled => "cancelled",
        rust_dos::control::InputDelivery::Failed => "failed",
    }
    .into();
    if let Some(hold) = operation.hold {
        let accepted = InputOutcome::Accepted {
            operation: operation.operation,
            context: operation.context,
            wait: InputWait::Pending,
            held: false,
            hold: Some(hold),
        };
        value["hold"] = self::value(accepted)["hold"].clone();
    }
    value
}
