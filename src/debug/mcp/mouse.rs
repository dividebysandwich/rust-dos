//! Mouse wire decoding; validation against the guest belongs to the actor.
#![deny(clippy::all)]
use rmcp::{
    ErrorData,
    model::{CallToolRequestParams, Tool},
};
use rust_dos::control::{
    GuestInput, MouseButton, MouseDragStep, MouseInput, MouseMotion, MousePicture, PictureSize,
};
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Picture {
    instance: String,
    mode_epoch: u64,
    width: u32,
    height: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Point {
    x: Option<i32>,
    y: Option<i32>,
    dx: Option<i32>,
    dy: Option<i32>,
    delay_ms: Option<u64>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    action: Option<String>,
    picture: Option<Picture>,
    x: Option<i32>,
    y: Option<i32>,
    dx: Option<i32>,
    dy: Option<i32>,
    button: Option<String>,
    hold_ms: Option<u64>,
    lease_ms: Option<u64>,
    handle: Option<String>,
    path: Option<Vec<Point>>,
}
fn invalid() -> ErrorData {
    ErrorData::invalid_params(
        "invalid mouse parameters; use one coordinate form and the capture's mouse_picture",
        None,
    )
}
fn motion(
    x: Option<i32>,
    y: Option<i32>,
    dx: Option<i32>,
    dy: Option<i32>,
) -> Result<Option<MouseMotion>, ErrorData> {
    match (x, y, dx, dy) {
        (Some(x), Some(y), None, None) => Ok(Some(MouseMotion::Absolute { x, y })),
        (None, None, Some(dx), Some(dy)) => Ok(Some(MouseMotion::Relative { dx, dy })),
        (None, None, None, None) => Ok(None),
        _ => Err(invalid()),
    }
}
pub(super) fn parse(request: &CallToolRequestParams) -> Result<GuestInput, ErrorData> {
    let arguments = request.arguments.clone().unwrap_or_default();
    if arguments.values().any(Value::is_null) {
        return Err(invalid());
    }
    if let Some(path) = arguments.get("path").and_then(Value::as_array)
        && (path.is_empty()
            || path.len() > super::super::owned_mouse::PATH_LIMIT
            || path.iter().any(|point| {
                point
                    .as_object()
                    .is_some_and(|fields| fields.values().any(Value::is_null))
            }))
    {
        return Err(invalid());
    }
    let args: Args = serde_json::from_value(Value::Object(arguments)).map_err(|_| invalid())?;
    let action = args.action.as_deref().unwrap_or("move");
    let movement = motion(args.x, args.y, args.dx, args.dy)?;
    let input = if matches!(action, "up" | "renew") {
        if args.picture.is_some()
            || movement.is_some()
            || args.button.is_some()
            || args.hold_ms.is_some()
            || args.path.is_some()
        {
            return Err(invalid());
        }
        let handle =
            super::super::owned_input::parse_token(args.handle.as_deref().ok_or_else(invalid)?)
                .ok_or_else(invalid)?;
        if action == "up" {
            if args.lease_ms.is_some() {
                return Err(invalid());
            }
            MouseInput::Up { handle }
        } else {
            let lease = args
                .lease_ms
                .filter(|lease| (1..=30000).contains(lease))
                .ok_or_else(invalid)?;
            MouseInput::Renew {
                handle,
                lease: Duration::from_millis(lease),
            }
        }
    } else {
        let picture = args.picture.ok_or_else(invalid)?;
        if picture.instance.len() != 16
            || !picture
                .instance
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || picture.mode_epoch == 0
            || picture.width == 0
            || picture.height == 0
        {
            return Err(invalid());
        }
        let picture = MousePicture {
            instance: picture.instance,
            mode_epoch: picture.mode_epoch,
            size: PictureSize {
                width: picture.width,
                height: picture.height,
            },
        };
        let button = match args.button.as_deref().unwrap_or("left") {
            "left" => MouseButton::Left,
            "right" => MouseButton::Right,
            "middle" => MouseButton::Middle,
            _ => return Err(invalid()),
        };
        match action {
            "move" => {
                if args.button.is_some()
                    || args.hold_ms.is_some()
                    || args.lease_ms.is_some()
                    || args.path.is_some()
                {
                    return Err(invalid());
                }
                let handle = args
                    .handle
                    .as_deref()
                    .map(|value| super::super::owned_input::parse_token(value).ok_or_else(invalid))
                    .transpose()?;
                MouseInput::Move {
                    picture,
                    motion: movement.ok_or_else(invalid)?,
                    handle,
                }
            }
            "down" => {
                if args.handle.is_some() || args.hold_ms.is_some() || args.path.is_some() {
                    return Err(invalid());
                }
                let lease = args
                    .lease_ms
                    .filter(|lease| (1..=30000).contains(lease))
                    .ok_or_else(invalid)?;
                MouseInput::Down {
                    picture,
                    motion: movement,
                    button,
                    lease: Duration::from_millis(lease),
                }
            }
            "click" => {
                if args.handle.is_some() || args.lease_ms.is_some() || args.path.is_some() {
                    return Err(invalid());
                }
                let duration = args.hold_ms.unwrap_or(super::super::DEFAULT_HOLD_MS);
                if !(1..=5000).contains(&duration) {
                    return Err(invalid());
                }
                MouseInput::Click {
                    picture,
                    motion: movement,
                    button,
                    duration: Duration::from_millis(duration),
                }
            }
            "drag" => {
                if args.handle.is_some()
                    || args.lease_ms.is_some()
                    || args.hold_ms.is_some()
                    || movement.is_some()
                {
                    return Err(invalid());
                }
                let path = args.path.ok_or_else(invalid)?;
                if path.is_empty() || path.len() > super::super::owned_mouse::PATH_LIMIT {
                    return Err(invalid());
                }
                let mut total = 0u64;
                let mut steps = Vec::with_capacity(path.len());
                for point in path {
                    let delay = point.delay_ms.unwrap_or(0);
                    total = total
                        .checked_add(delay)
                        .filter(|sum| *sum <= 5000)
                        .ok_or_else(invalid)?;
                    steps.push(MouseDragStep {
                        motion: motion(point.x, point.y, point.dx, point.dy)?
                            .ok_or_else(invalid)?,
                        delay: Duration::from_millis(delay),
                    });
                }
                MouseInput::Drag {
                    picture,
                    button,
                    path: steps,
                }
            }
            _ => return Err(invalid()),
        }
    };
    Ok(GuestInput::Mouse(input))
}
#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::ErrorCode;
    use serde_json::json;

    const HANDLE: &str = "0123456789abcdef.0000000000000001.0000000000000001";

    fn picture() -> Value {
        json!({"instance":"0123456789abcdef","mode_epoch":1,"width":640,"height":400})
    }

    // These fixtures exercise syntax only; guest/picture behavior has SDK tests.
    fn action(name: &str) -> Value {
        match name {
            "up" => json!({"action":name,"handle":HANDLE}),
            "renew" => json!({"action":name,"handle":HANDLE,"lease_ms":10}),
            "move" => json!({"action":name,"picture":picture(),"x":10,"y":20}),
            "down" => json!({"action":name,"picture":picture(),"lease_ms":10}),
            "click" => json!({"action":name,"picture":picture()}),
            "drag" => json!({"action":name,"picture":picture(),"path":[{"x":10,"y":20}]}),
            _ => unreachable!(),
        }
    }

    fn decode(args: Value) -> Result<GuestInput, ErrorData> {
        let mut request = CallToolRequestParams::new("mouse");
        request.arguments = Some(args.as_object().unwrap().clone());
        parse(&request)
    }

    #[test]
    fn each_action_rejects_fields_from_other_actions() {
        for (name, forbidden) in [
            (
                "up",
                &["picture", "button", "hold_ms", "lease_ms", "path"][..],
            ),
            ("renew", &["picture", "button", "hold_ms", "path"][..]),
            ("move", &["button", "hold_ms", "lease_ms", "path"][..]),
            ("down", &["handle", "hold_ms", "path"][..]),
            ("click", &["handle", "lease_ms", "path"][..]),
            ("drag", &["handle", "lease_ms", "hold_ms"][..]),
        ] {
            let valid = action(name);
            assert!(decode(valid.clone()).is_ok(), "valid {name}");
            for field in forbidden {
                let mut invalid = valid.clone();
                invalid[*field] = match *field {
                    "picture" => picture(),
                    "handle" => json!(HANDLE),
                    "button" => json!("left"),
                    "path" => json!([{"x":10,"y":20}]),
                    _ => json!(10),
                };
                assert_eq!(
                    decode(invalid).unwrap_err().code,
                    ErrorCode::INVALID_PARAMS,
                    "{name} rejects {field}"
                );
            }
        }
        for name in ["up", "renew", "drag"] {
            let mut invalid = action(name);
            invalid["x"] = json!(10);
            invalid["y"] = json!(20);
            assert_eq!(decode(invalid).unwrap_err().code, ErrorCode::INVALID_PARAMS);
        }
    }

    #[test]
    fn picture_syntax_and_duration_limits_fail_explicitly() {
        for (field, value) in [
            ("instance", json!("0123456789abcde")),
            ("instance", json!("0123456789abcdeg")),
            ("mode_epoch", json!(0)),
            ("width", json!(0)),
            ("height", json!(0)),
        ] {
            let mut invalid = action("move");
            invalid["picture"][field] = value;
            assert_eq!(decode(invalid).unwrap_err().code, ErrorCode::INVALID_PARAMS);
        }
        for (name, field, limit) in [
            ("click", "hold_ms", 5000),
            ("down", "lease_ms", 30000),
            ("renew", "lease_ms", 30000),
        ] {
            for value in [0, limit + 1] {
                let mut invalid = action(name);
                invalid[field] = json!(value);
                assert_eq!(decode(invalid).unwrap_err().code, ErrorCode::INVALID_PARAMS);
            }
        }
    }

    #[test]
    fn nulls_unpaired_coordinates_and_invalid_paths_are_rejected() {
        let mut null = action("move");
        null["handle"] = Value::Null;
        assert_eq!(decode(null).unwrap_err().code, ErrorCode::INVALID_PARAMS);
        for path in [
            json!([]),
            json!(vec![json!({"x":10,"y":20}); 257]),
            json!([{"x":10,"y":null}]),
            json!([{"x":10}]),
        ] {
            let mut invalid = action("drag");
            invalid["path"] = path;
            assert_eq!(decode(invalid).unwrap_err().code, ErrorCode::INVALID_PARAMS);
        }
        let mut relative = action("move");
        relative.as_object_mut().unwrap().remove("x");
        relative.as_object_mut().unwrap().remove("y");
        relative["dx"] = json!(5);
        relative["dy"] = json!(-3);
        assert!(decode(relative.clone()).is_ok());
        relative.as_object_mut().unwrap().remove("dy");
        assert_eq!(
            decode(relative).unwrap_err().code,
            ErrorCode::INVALID_PARAMS
        );
    }
}

pub(super) fn tool() -> Tool {
    Tool::new(
        "mouse",
        "Move using capture picture pixels or guest-relative deltas; default action is move. Click, drag, or retain a button with a renewable lease (maximum 30 seconds). Use the capture\'s mouse_picture; recapture after mode/replacement changes. Movement during a hold requires its opaque handle. Finite paths use one coordinate form, at most 256 points and 5000 ms total. Input may activate controls or write mounted drives. Acceptance is not delivery. Do not replay uncertain acceptance; inspect operation.",
        super::schemas::mouse_input()
            .as_object()
            .expect("mouse schema object")
            .clone(),
    )
}
