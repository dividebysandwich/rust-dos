//! Real SDK observations of mounted, licensed DOS graphics applications.
#[path = "native/catalog.rs"]
mod catalog;
use super::acquisition::{png_pixels, require};
use base64::Engine;
pub(super) use catalog::{
    BuildReceipt, build_instrumented_producer, build_producer, challenge_metadata,
    finish_instrumented_producer, verify_build_receipt, verify_catalog,
    verify_frontend_mouse_evidence, verify_mouse_evidence, verify_producer, write_catalog,
};
use rmcp::{
    model::{CallToolRequestParams, ClientConfig, ListToolsResult, ProtocolVersion},
    service::{ClientLifecycleMode, ClientServiceExt},
    transport::StreamableHttpClientTransport,
};
use std::{io, path::Path, time::Duration};

pub(super) async fn connect_client(
    url: &str,
    version: ProtocolVersion,
) -> io::Result<rmcp::service::RunningService<rmcp::RoleClient, ClientConfig>> {
    let lifecycle = if version.has_initialize() {
        ClientLifecycleMode::Initialize
    } else {
        ClientLifecycleMode::Discover {
            preferred_versions: vec![version.clone()],
        }
    };
    tokio::time::timeout(
        Duration::from_secs(10),
        ClientConfig::default()
            .with_protocol_version(version)
            .serve_with_lifecycle(
                StreamableHttpClientTransport::from_uri(format!("{url}/mcp")),
                lifecycle,
            ),
    )
    .await
    .map_err(io::Error::other)?
    .map_err(io::Error::other)
}

async fn mouse_call(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ClientConfig>,
    root: &Path,
    label: &str,
    tool: &str,
    arguments: serde_json::Value,
) -> io::Result<rmcp::model::CallToolResult> {
    let mut request = CallToolRequestParams::new(tool.to_owned());
    request.arguments = Some(
        arguments
            .as_object()
            .ok_or_else(|| io::Error::other("mouse request must be an object"))?
            .clone(),
    );
    std::fs::write(
        root.join(format!("{label}-request.json")),
        serde_json::to_vec(&request).map_err(io::Error::other)?,
    )?;
    let reply = tokio::time::timeout(Duration::from_secs(10), client.call_tool(request))
        .await
        .map_err(io::Error::other)?
        .map_err(io::Error::other)?;
    std::fs::write(
        root.join(format!("{label}-result.json")),
        serde_json::to_vec(&reply).map_err(io::Error::other)?,
    )?;
    require(reply.is_error != Some(true), "native mouse action rejected")?;
    if tool == "mouse" {
        let accepted = reply
            .structured_content
            .as_ref()
            .ok_or_else(|| io::Error::other("mouse acceptance metadata missing"))?;
        let mut operation = CallToolRequestParams::new("operation");
        operation.arguments = Some(
            serde_json::json!({
                "instance": accepted["instance"], "op": accepted["op"]
            })
            .as_object()
            .unwrap()
            .clone(),
        );
        std::fs::write(
            root.join(format!("{label}-operation-request.json")),
            serde_json::to_vec(&operation).map_err(io::Error::other)?,
        )?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let observed = tokio::time::timeout_at(deadline, client.call_tool(operation.clone()))
                .await
                .map_err(io::Error::other)?
                .map_err(io::Error::other)?;
            std::fs::write(
                root.join(format!("{label}-operation-result.json")),
                serde_json::to_vec(&observed).map_err(io::Error::other)?,
            )?;
            require(
                observed.is_error != Some(true),
                "mouse operation query rejected",
            )?;
            let state = observed
                .structured_content
                .as_ref()
                .and_then(|value| value["delivery"].as_str())
                .ok_or_else(|| io::Error::other("mouse operation delivery missing"))?;
            if state == "delivered" {
                break;
            }
            require(state == "pending", "mouse operation failed before delivery")?;
            require(
                tokio::time::Instant::now() < deadline,
                "mouse operation delivery deadline",
            )?;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    Ok(reply)
}

pub(super) async fn mouse(
    url: &str,
    root: &Path,
    program: &str,
    start: (i32, i32),
    end: (i32, i32),
    mut image: impl FnMut(&str) -> io::Result<Vec<u8>>,
) -> io::Result<()> {
    use serde_json::json;
    for (label, version) in [
        ("legacy", ProtocolVersion::V_2025_11_25),
        ("current", ProtocolVersion::V_2026_07_28),
    ] {
        let client = connect_client(url, version.clone()).await?;
        let result: io::Result<()> = tokio::time::timeout(Duration::from_secs(30), async {
            require(
                client
                    .peer_info()
                    .is_some_and(|peer| peer.protocol_version == version),
                "native mouse protocol negotiation mismatch",
            )?;
            let discovery = client.list_tools(None).await.map_err(io::Error::other)?;
            std::fs::write(
                root.join(format!("mouse-{label}-discovery.json")),
                serde_json::to_vec(&discovery).map_err(io::Error::other)?,
            )?;
            require(
                discovery.tools.iter().any(|tool| tool.name == "mouse"),
                "native mouse tool missing for the actual graphics application",
            )?;
            let capture = mouse_call(
                &client,
                root,
                &format!("mouse-{label}-capture"),
                "capture",
                json!({}),
            )
            .await?;
            let value = capture
                .structured_content
                .ok_or_else(|| io::Error::other("native picture metadata missing"))?;
            let picture = value["mouse_picture"].clone();
            require(
                picture["mode_epoch"].as_u64().is_some(),
                "native mouse picture basis missing",
            )?;
            let before = png_pixels(&image(&format!("mouse-{label}-before"))?)?;
            require(
                before.0
                    == if program == "seal" {
                        (640, 480)
                    } else {
                        (640, 400)
                    },
                "native mouse actual picture dimensions mismatch",
            )?;
            let shift = if program == "grafx2" && label == "current" {
                120
            } else {
                0
            };
            let down = mouse_call(
                &client,
                root,
                &format!("mouse-{label}-down"),
                "mouse",
                json!({"action":"down","picture":picture,"button":"left",
                    "x":start.0,"y":start.1 + shift,"lease_ms":30000}),
            )
            .await?;
            let down = down
                .structured_content
                .ok_or_else(|| io::Error::other("native button outcome missing"))?;
            let handle = down["hold"]["handle"]
                .as_str()
                .ok_or_else(|| io::Error::other("native button handle missing"))?
                .to_owned();
            let _held = image(&format!("mouse-{label}-held"))?;
            mouse_call(
                &client,
                root,
                &format!("mouse-{label}-move"),
                "mouse",
                json!({"action":"move","picture":picture,"handle":handle,
                    "x":end.0,"y":end.1 + shift}),
            )
            .await?;
            let _moved = image(&format!("mouse-{label}-moved-held"))?;
            mouse_call(
                &client,
                root,
                &format!("mouse-{label}-up"),
                "mouse",
                json!({"action":"up","handle":handle}),
            )
            .await?;
            mouse_call(
                &client,
                root,
                &format!("mouse-{label}-cursor-away"),
                "mouse",
                json!({"action":"move","picture":picture,"x":630,"y":390}),
            )
            .await?;
            let after = png_pixels(&image(&format!("mouse-{label}-after"))?)?;
            require(
                after.0 == before.0,
                "native mouse effect changed picture mode",
            )?;
            if program == "seal" {
                super::acquisition::graphics_effect(program, &before.1, &after.1)?;
            } else {
                let offset = ((110 + shift as usize) * 640 + 200) * 3;
                require(
                    before.1[offset..offset + 3] == [0, 0, 0]
                        && after.1[offset..offset + 3] == [252, 252, 252],
                    "native incremental drag paints its actual canvas pixel",
                )?;
            }
            if program == "seal" {
                mouse_call(
                    &client,
                    root,
                    &format!("mouse-{label}-finite"),
                    "mouse",
                    json!({"action":"drag","picture":picture,"button":"left","path":[
                        {"x":end.0,"y":end.1,"delay_ms":150},
                        {"x":start.0,"y":start.1,"delay_ms":300}]}),
                )
                .await?;
                mouse_call(
                    &client,
                    root,
                    &format!("mouse-{label}-finite-away"),
                    "mouse",
                    json!({"action":"move","picture":picture,"x":630,"y":390}),
                )
                .await?;
                let restored = png_pixels(&image(&format!("mouse-{label}-finite-after"))?)?;
                super::acquisition::graphics_effect("seal", &restored.1, &after.1)?;
            } else {
                mouse_call(
                    &client,
                    root,
                    &format!("mouse-{label}-finite"),
                    "mouse",
                    json!({"action":"drag","picture":picture,"button":"left","path":[
                        {"x":80,"y":140 + shift,"delay_ms":150},
                        {"x":120,"y":160 + shift,"delay_ms":300}]}),
                )
                .await?;
                let painted = png_pixels(&image(&format!("mouse-{label}-finite-after"))?)?;
                let offset = ((150 + shift as usize) * 640 + 200) * 3;
                require(
                    before.1[offset..offset + 3] == [0, 0, 0]
                        && painted.1[offset..offset + 3] == [252, 252, 252],
                    "native finite path paints the measured canvas pixel",
                )?;
            }
            mouse_call(
                &client,
                root,
                &format!("mouse-{label}-click"),
                "mouse",
                json!({"action":"click","picture":picture,"button":"left",
                    "x":if program == "seal" {450} else {140},
                    "y":if program == "seal" {300} else {180},"hold_ms":150}),
            )
            .await?;
            let previous = mouse_call(
                &client,
                root,
                &format!("mouse-{label}-relative-before"),
                "status",
                json!({}),
            )
            .await?
            .structured_content
            .ok_or_else(|| io::Error::other("native relative mouse starting status missing"))?;
            mouse_call(
                &client,
                root,
                &format!("mouse-{label}-relative"),
                "mouse",
                json!({"action":"move","picture":picture,"dx":5,"dy":-3}),
            )
            .await?;
            let observed = mouse_call(
                &client,
                root,
                &format!("mouse-{label}-relative-after"),
                "status",
                json!({}),
            )
            .await?
            .structured_content
            .ok_or_else(|| io::Error::other("native relative mouse result status missing"))?;
            require(
                observed["mouse"]["x"].as_i64() == previous["mouse"]["x"].as_i64().map(|x| x + 5)
                    && observed["mouse"]["y"].as_i64()
                        == previous["mouse"]["y"].as_i64().map(|y| y - 3)
                    && observed["mouse"]["buttons"] == 0,
                "native click release and actual guest relative motion mismatch",
            )?;
            Ok(())
        })
        .await
        .map_err(io::Error::other)
        .and_then(|result| result);
        let cancelled = client.cancel().await;
        result?;
        cancelled.map_err(io::Error::other)?;
    }
    Ok(())
}

pub(super) fn discovery_contract(discovery: &ListToolsResult) -> io::Result<()> {
    require(
        discovery.next_cursor.is_none(),
        "native observation discovery must not paginate",
    )?;
    let mut names: Vec<_> = discovery
        .tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect();
    names.sort();
    require(
        names
            == [
                "capture",
                "fast_forward",
                "key",
                "mouse",
                "operation",
                "speed",
                "status",
                "type",
            ],
        "native discovery advertises unsupported tools",
    )?;
    for tool in &discovery.tools {
        let expected = match tool.name.as_ref() {
            "status" | "capture" => {
                serde_json::json!({"type":"object","additionalProperties":false})
            }
            "key" => {
                serde_json::json!({"type":"object","additionalProperties":false,"properties":{
                    "action":{"type":"string","enum":["press","down","up","renew"]},
                    "key":{"type":"string"},
                    "mods":{"type":"array","items":{"type":"string"},"maxItems":4,"uniqueItems":true},
                    "hold_ms":{"type":"integer","minimum":1,"maximum":5000},
                    "lease_ms":{"type":"integer","minimum":1,"maximum":30000},
                    "handle":{"type":"string","maxLength":50}
                }})
            }
            "type" => {
                serde_json::json!({"type":"object","additionalProperties":false,"properties":{
                "text":{"type":"string","maxLength":4096},
                "delay_ms":{"type":"integer","minimum":0,"maximum":5000}
            },"required":["text"]})
            }
            "operation" => {
                serde_json::json!({"type":"object","additionalProperties":false,"properties":{
                "instance":{"type":"string","pattern":"^[0-9a-f]{16}$"},
                "op":{"type":"integer","minimum":1}
            },"required":["instance","op"]})
            }
            "mouse" => {
                serde_json::from_str(include_str!("../../tests/fixtures/mcp/mouse.schema.json"))
                    .map_err(io::Error::other)?
            }
            "speed" => {
                serde_json::json!({"type":"object","additionalProperties":false,"properties":{"cycles":{"type":"string","minLength":1,"maxLength":32}}})
            }
            "fast_forward" => serde_json::json!({
                "type":"object","additionalProperties":false,"properties":{"action":{"enum":["get","start","stop"]},"duration_ms":{"type":"integer","minimum":1,"maximum":5000}},"required":["action"],
                "oneOf":[{"properties":{"action":{"enum":["get","stop"]}},"not":{"required":["duration_ms"]}},{"properties":{"action":{"const":"start"}},"required":["duration_ms"]}]
            }),
            _ => {
                return Err(io::Error::other(
                    "native discovery advertises unsupported tools",
                ));
            }
        };
        require(
            serde_json::Value::Object((*tool.input_schema).clone()) == expected,
            "native schema differs from the supported bounded contract",
        )?;
    }
    Ok(())
}

pub(super) fn challenge_discovery(discovery: &ListToolsResult) -> io::Result<usize> {
    discovery_contract(discovery)?;
    let mut killed = 0;
    for index in 0..discovery.tools.len() {
        for corruption in 0..4 {
            let mut damaged = discovery.clone();
            let tool = &mut damaged.tools[index];
            match corruption {
                0 => {
                    std::sync::Arc::make_mut(&mut tool.input_schema).remove("type");
                }
                1 => {
                    std::sync::Arc::make_mut(&mut tool.input_schema)
                        .insert("additionalProperties".into(), serde_json::Value::Bool(true));
                }
                2 => {
                    std::sync::Arc::make_mut(&mut tool.input_schema)
                        .insert("type".into(), serde_json::Value::String("string".into()));
                }
                _ => tool.name.to_mut().push('_'),
            }
            require(
                discovery_contract(&damaged).is_err(),
                "actual discovery corruption escaped validation",
            )?;
            killed += 1;
        }
        if let Some(properties) = discovery.tools[index]
            .input_schema
            .get("properties")
            .and_then(serde_json::Value::as_object)
        {
            for field in properties.keys() {
                let mut damaged = discovery.clone();
                std::sync::Arc::make_mut(&mut damaged.tools[index].input_schema)
                    .get_mut("properties")
                    .and_then(serde_json::Value::as_object_mut)
                    .ok_or_else(|| io::Error::other("actual discovery properties missing"))?
                    .remove(field);
                require(
                    discovery_contract(&damaged).is_err(),
                    "actual tool property removal escaped validation",
                )?;
                killed += 1;
            }
        }
        if discovery.tools[index].input_schema.contains_key("required") {
            let mut damaged = discovery.clone();
            std::sync::Arc::make_mut(&mut damaged.tools[index].input_schema).remove("required");
            require(
                discovery_contract(&damaged).is_err(),
                "actual required argument removal escaped validation",
            )?;
            killed += 1;
        }
    }
    Ok(killed)
}

pub(super) async fn capture(
    url: &str,
    root: &Path,
    http_png: &[u8],
    mut advance: impl FnMut() -> io::Result<Vec<u8>>,
) -> io::Result<()> {
    let mut expected = png_pixels(http_png)?;
    let mut instance = None;
    for (label, version) in [
        ("legacy", ProtocolVersion::V_2025_11_25),
        ("current", ProtocolVersion::V_2026_07_28),
    ] {
        let client = connect_client(url, version.clone()).await?;
        let result = tokio::time::timeout(Duration::from_secs(15), async {
            require(
                client
                    .peer_info()
                    .is_some_and(|peer| peer.protocol_version == version),
                "native protocol negotiation mismatch",
            )?;
            std::fs::write(root.join(format!("mcp-{label}-peer.json")), serde_json::to_vec(client.peer_info().unwrap().as_ref()).map_err(io::Error::other)?)?;
            let discovery = client
                .list_tools(None)
                .await
                .map_err(io::Error::other)?;
            std::fs::write(root.join(format!("mcp-{label}-discovery.json")), serde_json::to_vec(&discovery).map_err(io::Error::other)?)?;
            discovery_contract(&discovery)?;
            let first_expected = expected.clone();
            let mut previous_request = None;
            for round in 0..2 {
            let status = client
                .call_tool(CallToolRequestParams::new("status"))
                .await
                .map_err(io::Error::other)?;
            require(
                status.is_error != Some(true),
                "native application status failed",
            )?;
            let oracle = ureq::Agent::new_with_config(ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(2))).build());
            let mut response = oracle.get(format!("{url}/api/status")).call().map_err(io::Error::other)?;
            let http_status = response.body_mut().with_config().limit(32 * 1024)
                .read_to_vec().map_err(io::Error::other)?;
            let http_status_value: serde_json::Value = serde_json::from_slice(&http_status).map_err(io::Error::other)?;
            status_parity(&status, &http_status_value)?;
            std::fs::write(root.join(format!("mcp-{label}-{round}-http-status.json")), &http_status)?;
            let status_meta = status
                .structured_content
                .as_ref()
                .ok_or_else(|| io::Error::other("native status metadata missing"))?;
            require(
                status_meta["paused"] == true
                    && status_meta["video"]["width"] == expected.0.0
                    && status_meta["video"]["height"] == expected.0.1,
                "native paused application dimensions mismatch",
            )?;
            let capture = client
                .call_tool(CallToolRequestParams::new("capture"))
                .await
                .map_err(io::Error::other)?;
            require(
                capture.is_error != Some(true),
                "native application capture failed",
            )?;
            let image = capture
                .content
                .iter()
                .find_map(|content| content.as_image())
                .ok_or_else(|| io::Error::other("native image content missing"))?;
            require(
                image.mime_type == "image/png" && image.data.len() <= 24 * 1024 * 1024,
                "native image format/size mismatch",
            )?;
            let png = base64::engine::general_purpose::STANDARD
                .decode(&image.data)
                .map_err(io::Error::other)?;
            let actual = png_pixels(&png)?;
            pixel_parity(&actual, &expected)?;
            let (identity, request) = observation_metadata(&status, &capture, actual.0)?;
            require_same_instance(&mut instance, identity)?;
            require(previous_request.is_none_or(|previous| request > previous), "native capture request order mismatch")?;
            previous_request = Some(request);
            std::fs::write(root.join(format!("mcp-{label}-{round}-capture.png")), &png)?;
            std::fs::write(
                root.join(format!("mcp-{label}-{round}-status.json")),
                serde_json::to_vec(&status).map_err(io::Error::other)?,
            )?;
            std::fs::write(
                root.join(format!("mcp-{label}-{round}-capture.json")),
                serde_json::to_vec(&capture).map_err(io::Error::other)?,
            )?;
            if round == 0 {
                let next_http = advance()?;
                let next = png_pixels(&next_http)?;
                require(next.0 == expected.0 && next.1 != expected.1, "guest must produce a different second picture")?;
                require(pixel_parity(&actual, &next).is_err(), "cached-first-image mutation must fail pixel oracle")?;
                expected = next;
            } else {
                require(pixel_parity(&actual, &first_expected).is_err(), "second-image-first mutation must fail the same pixel oracle")?;
                std::fs::write(root.join(format!("mcp-{label}-replay-sensitivity.json")), b"{\"cached_first_image_rejected\":true,\"swapped_images_rejected\":true,\"source\":\"actual_guest_frames\"}")?;
            }
            }
            Ok::<_, io::Error>(())
        })
        .await
        .map_err(io::Error::other)
        .and_then(|result| result);
        let cancelled = client.cancel().await.map_err(io::Error::other);
        result?;
        cancelled?;
    }
    println!(
        "Verified real native legacy/current application PNG parity at {}x{}",
        expected.0.0, expected.0.1
    );
    Ok(())
}

fn pixel_parity(
    actual: &((u32, u32), Vec<u8>),
    expected: &((u32, u32), Vec<u8>),
) -> io::Result<()> {
    require(
        actual == expected,
        "native application pixels differ from paused HTTP picture",
    )
}

fn status_parity(status: &rmcp::model::CallToolResult, http: &serde_json::Value) -> io::Result<()> {
    let status = status
        .structured_content
        .as_ref()
        .ok_or_else(|| io::Error::other("native status metadata missing"))?;
    require(
        status["program"].is_string()
            && status["program"] == http["program"]
            && status["instructions"].is_u64()
            && status["instructions"] == http["icount"]
            && status["mouse"]["installed"].is_boolean()
            && status["mouse"]["installed"] == http["mouse"]["installed"]
            && status["mouse"]["buttons"].is_u64()
            && status["mouse"]["buttons"] == http["mouse"]["buttons"]
            && status["mouse"]["x"].is_i64()
            && status["mouse"]["x"] == http["mouse"]["x"]
            && status["mouse"]["y"].is_i64()
            && status["mouse"]["y"] == http["mouse"]["y"],
        "native program, instruction count or driver mouse state differs from paused HTTP status",
    )
}

fn require_same_instance(instance: &mut Option<String>, observed: &str) -> io::Result<()> {
    if let Some(previous) = instance {
        require(previous == observed, "native guest instance changed")
    } else {
        *instance = Some(observed.to_owned());
        Ok(())
    }
}

fn observation_metadata<'a>(
    status: &rmcp::model::CallToolResult,
    capture: &'a rmcp::model::CallToolResult,
    dimensions: (u32, u32),
) -> io::Result<(&'a str, u64)> {
    require(
        status.is_error != Some(true) && capture.is_error != Some(true),
        "native observation failed",
    )?;
    let status = status
        .structured_content
        .as_ref()
        .ok_or_else(|| io::Error::other("native status metadata missing"))?;
    let capture = capture
        .structured_content
        .as_ref()
        .ok_or_else(|| io::Error::other("native image metadata missing"))?;
    let identity = capture["instance"]
        .as_str()
        .ok_or_else(|| io::Error::other("native instance identity missing"))?;
    require(
        identity.len() == 16
            && identity.bytes().all(|byte| byte.is_ascii_hexdigit())
            && status["instance"].as_str() == Some(identity),
        "native instance identity invalid",
    )?;
    require(
        capture["width"] == dimensions.0
            && capture["height"] == dimensions.1
            && status["video"]["width"] == dimensions.0
            && status["video"]["height"] == dimensions.1
            && status["paused"] == true,
        "native observation dimensions/state mismatch",
    )?;
    for metadata in [status, capture] {
        require(
            metadata["state_seq"].is_u64()
                && metadata["journal_seq"].is_u64()
                && metadata
                    .get("stop_seq")
                    .is_some_and(|value| value.is_null() || value.is_u64()),
            "native observation sequence missing",
        )?;
    }
    let request = capture["req"]
        .as_u64()
        .ok_or_else(|| io::Error::other("native capture request missing"))?;
    let status_request = status["req"]
        .as_u64()
        .ok_or_else(|| io::Error::other("native status request missing"))?;
    require(
        request > status_request && capture["state_seq"].as_u64() >= status["state_seq"].as_u64(),
        "native observation request/state order mismatch",
    )?;
    Ok((identity, request))
}
