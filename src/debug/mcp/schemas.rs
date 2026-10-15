//! Structured result contracts advertised through native tool discovery.
#![deny(clippy::all)]
use serde_json::{Value, json};

pub(super) fn picture() -> Value {
    json!({"type":"object","additionalProperties":false,"properties":{
        "instance":{"type":"string","pattern":"^[0-9a-f]{16}$"},
        "mode_epoch":{"type":"integer","minimum":1},
        "width":{"type":"integer","minimum":1},"height":{"type":"integer","minimum":1}
    },"required":["instance","mode_epoch","width","height"]})
}

fn hold() -> Value {
    json!({"type":"object","additionalProperties":false,"properties":{
        "handle":{"type":"string","pattern":"^[0-9a-f]{16}\\.[0-9a-f]{16}\\.[0-9a-f]{16}$"},
        "state":{"enum":["reserved","delivered","released","expired","reset"]},
        "remaining_ms":{"type":"integer","minimum":0}
    },"required":["handle","state","remaining_ms"]})
}

fn absent(fields: &[&str]) -> Value {
    let present = fields
        .iter()
        .map(|field| json!({"required":[field]}))
        .collect::<Vec<_>>();
    json!({"not":{"anyOf":present}})
}

fn coordinate_form(absolute: bool) -> Value {
    let (required, forbidden) = if absolute {
        (["x", "y"], ["dx", "dy"])
    } else {
        (["dx", "dy"], ["x", "y"])
    };
    let mut form = absent(&forbidden);
    form["required"] = json!(required);
    form
}

fn mouse_action(name: &str, required: &[&str], forbidden: &[&str]) -> Value {
    let mut branch = absent(forbidden);
    branch["properties"] = json!({"action":{"const":name}});
    let mut fields = required.to_vec();
    if name == "move" {
        branch["anyOf"] = json!([{"required":["x","y"]},{"required":["dx","dy"]}]);
    } else {
        fields.push("action");
    }
    branch["required"] = json!(fields);
    branch
}

pub(super) fn mouse_input() -> Value {
    let coordinates = json!({
        "x":{"type":"integer","minimum":0},"y":{"type":"integer","minimum":0},
        "dx":{"type":"integer","minimum":-32767,"maximum":32767},
        "dy":{"type":"integer","minimum":-32767,"maximum":32767}
    });
    let mut point_fields = coordinates
        .as_object()
        .expect("coordinate schema object")
        .clone();
    point_fields.insert(
        "delay_ms".into(),
        json!({"type":"integer","minimum":0,"maximum":5000}),
    );
    let point = |absolute| {
        json!({"type":"object","additionalProperties":false,
        "properties":point_fields,"allOf":[coordinate_form(absolute)]})
    };
    let mut fields = coordinates
        .as_object()
        .expect("coordinate schema object")
        .clone();
    fields.extend(json!({
        "action":{"type":"string","enum":["move","click","down","up","renew","drag"],"default":"move"},
        "picture":picture(),"button":{"type":"string","enum":["left","right","middle"]},
        "hold_ms":{"type":"integer","minimum":1,"maximum":5000},
        "lease_ms":{"type":"integer","minimum":1,"maximum":30000},
        "handle":{"type":"string","pattern":"^[0-9a-f]{16}\\.[0-9a-f]{16}\\.[0-9a-f]{16}$"},
        "path":{"type":"array","minItems":1,"maxItems":256,
            "description":"Use one coordinate form throughout. The actor also checks that all delays total at most 5000 ms.",
            "oneOf":[{"items":point(true)},{"items":point(false)}]}
    }).as_object().expect("mouse schema object").clone());
    json!({"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object",
    "additionalProperties":false,"properties":fields,
    "allOf":[{"oneOf":[coordinate_form(true),coordinate_form(false),absent(&["x","y","dx","dy"])]}],
    "oneOf":[
        mouse_action("move", &["picture"], &["button","hold_ms","lease_ms","path"]),
        mouse_action("click", &["picture"], &["handle","lease_ms","path"]),
        mouse_action("down", &["picture","lease_ms"], &["handle","hold_ms","path"]),
        mouse_action("up", &["handle"], &["picture","x","y","dx","dy","button","hold_ms","lease_ms","path"]),
        mouse_action("renew", &["handle","lease_ms"], &["picture","x","y","dx","dy","button","hold_ms","path"]),
        mouse_action("drag", &["picture","path"], &["handle","hold_ms","lease_ms","x","y","dx","dy"])
    ]})
}

fn observed(properties: Value, required: &[&str]) -> Value {
    let mut fields = json!({
        "instance":{"type":"string","pattern":"^[0-9a-f]{16}$"},
        "req":{"type":"integer","minimum":0},
        "state_seq":{"type":"integer","minimum":0},
        "journal_seq":{"type":"integer","minimum":0},
        "stop_seq":{"type":["integer","null"],"minimum":0},
        "mouse_picture":picture()
    })
    .as_object()
    .expect("context schema object")
    .clone();
    fields.extend(
        properties
            .as_object()
            .expect("result schema object")
            .clone(),
    );
    let mut required_fields = vec!["instance", "req", "state_seq", "journal_seq", "stop_seq"];
    required_fields.extend_from_slice(required);
    json!({"type":"object","additionalProperties":false,"properties":fields,"required":required_fields})
}

pub(super) fn output(name: &str) -> Value {
    if matches!(name, "speed" | "fast_forward") {
        return super::playback::output(name);
    }
    let success = match name {
        "status" => observed(
            json!({
                "paused":{"type":"boolean"},"program":{"type":"string"},
                "instructions":{"type":"integer","minimum":0},
                "video":{"type":"object","additionalProperties":false,"properties":{
                    "width":{"type":"integer","minimum":1},"height":{"type":"integer","minimum":1}
                },"required":["width","height"]},
                "mouse":{"type":"object","additionalProperties":false,"properties":{
                    "installed":{"type":"boolean"},"buttons":{"type":"integer","minimum":0},
                    "x":{"type":"integer","minimum":-2147483648_i64,"maximum":2147483647},
                    "y":{"type":"integer","minimum":-2147483648_i64,"maximum":2147483647}
                },"required":["installed","buttons","x","y"]}
            }),
            &["paused", "program", "instructions", "video", "mouse"],
        ),
        "capture" => observed(
            json!({
                "width":{"type":"integer","minimum":1},"height":{"type":"integer","minimum":1}
            }),
            &["width", "height"],
        ),
        "operation" => observed(
            json!({
                "op":{"type":"integer","minimum":1},
                "delivery":{"enum":["pending","delivered","cancelled","failed"]},"hold":hold()
            }),
            &["op", "delivery"],
        ),
        "key" | "type" | "mouse" => json!({"oneOf":[
            observed(json!({
                "op":{"type":"integer","minimum":1},"acceptance":{"const":"accepted"},
                "wait":{"enum":["pending","delivered","timed_out"]},
                "held":{"type":"boolean"},"hold":hold()
            }), &["op", "acceptance", "wait", "held"]),
            {"type":"object","additionalProperties":false,"properties":{
                "instance":{"type":"string","pattern":"^[0-9a-f]{16}$"},
                "op":{"type":"integer","minimum":1},"acceptance":{"const":"unknown"}
            },"required":["instance","op","acceptance"]}
        ]}),
        _ => unreachable!("schemas are only requested for registered native tools"),
    };
    let error = json!({"type":"object","additionalProperties":false,"properties":{
        "error":{"type":"object","additionalProperties":false,"properties":{
            "code":{"type":"string"},"message":{"type":"string"},"recovery":{"type":"string"}
        },"required":["code","message","recovery"]}
    },"required":["error"]});
    json!({"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","oneOf":[success,error]})
}
