//! Native SDK keyboard ownership through the actual isolated SDL frontend.
use super::{Journey, Provider, connect, run_guest, validate_root};
use crate::acquisition::{bios_clock_record, read_bounded, require, source_digest};
use rmcp::{
    model::{CallToolRequestParams, ClientConfig, ProtocolVersion},
    service::{ClientLifecycleMode, ClientServiceExt},
    transport::StreamableHttpClientTransport,
};
use serde_json::{Value, json};
use std::{
    io,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use tokio::io::AsyncReadExt;

pub(crate) async fn acquire(
    root: &Path,
    load: bool,
    coverage: bool,
    without_mcp: bool,
) -> io::Result<()> {
    require(
        !without_mcp || load,
        "default frontend verification requires slot loading",
    )?;
    validate_root(root)?;
    std::fs::create_dir(root)?;
    let build = if coverage {
        crate::native::build_instrumented_producer(root).await?
    } else {
        crate::native::build_producer(root).await?
    };
    let profile = if coverage {
        Some(
            serde_json::from_slice::<String>(&read_bounded(
                &root.join("coverage-profile.json"),
                8192,
            )?)
            .map_err(io::Error::other)?,
        )
    } else {
        None
    };
    std::fs::write(
        root.join("frontend-producer.json"),
        serde_json::to_vec_pretty(&build).map_err(io::Error::other)?,
    )?;
    let native = root.join(if cfg!(windows) {
        "native-rust-dos.exe"
    } else {
        "native-rust-dos"
    });
    require(
        native.is_file(),
        "retained native frontend producer missing",
    )?;
    let guest = root.join("guest");
    std::fs::create_dir(&guest)?;
    let config = root.join("frontend.conf");
    std::fs::write(&config, "[emulator]\nrewind=true\nmonitor_effects=off\n")?;
    let mut command = Command::new("Xvfb");
    command
        .args([
            "-displayfd",
            "1",
            "-screen",
            "0",
            "1024x768x24",
            "-nolisten",
            "tcp",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(std::fs::File::create(root.join("xvfb.log"))?);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut command = tokio::process::Command::from(command);
    command.kill_on_drop(true);
    let mut child = command.spawn()?;
    let tree = match crate::ProcessTree::attach(&child) {
        Ok(tree) => tree,
        Err(error) => {
            child.kill().await?;
            child.wait().await?;
            return Err(error);
        }
    };
    let result = async {
        let mut bytes = [0u8; 32];
        let len = tokio::time::timeout(
            Duration::from_secs(5),
            child.stdout.as_mut().unwrap().read(&mut bytes),
        )
        .await
        .map_err(io::Error::other)??;
        let number = std::str::from_utf8(&bytes[..len])
            .map_err(io::Error::other)?
            .trim()
            .parse::<u16>()
            .map_err(io::Error::other)?;
        let display = format!(":{number}");
        run_guest(
            root,
            &native,
            &guest,
            Journey::Frontend {
                display,
                config,
                load,
                profile,
                mcp: !without_mcp,
            },
        )
        .await
    }
    .await;
    tree.terminate();
    child.start_kill()?;
    tokio::time::timeout(Duration::from_secs(2), child.wait())
        .await
        .map_err(io::Error::other)??;
    result?;
    if coverage {
        crate::native::finish_instrumented_producer(root, build)
    } else {
        crate::native::verify_producer(root, &build)
    }
}

async fn xdo(display: &str, args: &[&str]) -> io::Result<String> {
    let mut command = Command::new("xdotool");
    command.env("DISPLAY", display).args(args);
    let output = crate::run(command, Duration::from_secs(5), 8192).await?;
    Ok(std::str::from_utf8(&output.stdout)
        .map_err(io::Error::other)?
        .trim()
        .to_owned())
}

fn shift(provider: &Provider, label: &str) -> io::Result<bool> {
    let value = provider.get(label, "/api/memory?addr=0040:0017&len=1&format=json")?;
    let bytes = value["hex"]
        .as_str()
        .ok_or_else(|| io::Error::other("BIOS flags missing"))?;
    Ok(u8::from_str_radix(bytes.trim(), 16).map_err(io::Error::other)? & 2 != 0)
}

fn bios_ticks(provider: &Provider, label: &str) -> io::Result<u32> {
    let value = provider.get(label, "/api/memory?addr=0040:006C&len=4&format=json")?;
    bios_clock_record(&value)
}

fn mouse_buttons(provider: &Provider, label: &str) -> io::Result<u64> {
    provider.get(label, "/api/status")?["mouse"]["buttons"]
        .as_u64()
        .ok_or_else(|| io::Error::other("actual frontend mouse state missing"))
}

async fn mouse_call(
    provider: &Provider,
    client: &rmcp::service::RunningService<rmcp::RoleClient, ClientConfig>,
    label: &str,
    args: Value,
) -> io::Result<Value> {
    let mut request = CallToolRequestParams::new("mouse");
    request.arguments = Some(
        args.as_object()
            .ok_or_else(|| io::Error::other("mouse arguments missing"))?
            .clone(),
    );
    std::fs::write(
        provider.root.join(format!("{label}-request.json")),
        serde_json::to_vec(&request).map_err(io::Error::other)?,
    )?;
    let response = client.call_tool(request).await.map_err(io::Error::other)?;
    std::fs::write(
        provider.root.join(format!("{label}-response.json")),
        serde_json::to_vec(&response).map_err(io::Error::other)?,
    )?;
    require(
        response.is_error != Some(true),
        "actual frontend mouse action rejected",
    )?;
    response
        .structured_content
        .ok_or_else(|| io::Error::other("mouse action result missing"))
}

async fn mouse_hold(
    provider: &Provider,
    client: &rmcp::service::RunningService<rmcp::RoleClient, ClientConfig>,
    label: &str,
) -> io::Result<Value> {
    let capture = client
        .call_tool(CallToolRequestParams::new("capture"))
        .await
        .map_err(io::Error::other)?;
    require(
        capture.is_error != Some(true),
        "frontend mouse picture capture rejected",
    )?;
    let picture = capture
        .structured_content
        .ok_or_else(|| io::Error::other("capture metadata missing"))?["mouse_picture"]
        .clone();
    let held = mouse_call(
        provider,
        client,
        label,
        json!({"action":"down","picture":picture,"lease_ms":30000}),
    )
    .await?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while mouse_buttons(provider, &format!("{label}-delivered"))? != 1 {
        require(
            Instant::now() < deadline,
            "actual frontend button delivery deadline",
        )?;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Ok(held)
}

async fn mouse_restored(
    provider: &Provider,
    client: &rmcp::service::RunningService<rmcp::RoleClient, ClientConfig>,
    held: &Value,
    label: &str,
) -> io::Result<()> {
    let mut query = CallToolRequestParams::new("operation");
    query.arguments = Some(
        json!({"instance":held["instance"],"op":held["op"]})
            .as_object()
            .unwrap()
            .clone(),
    );
    let response = client.call_tool(query).await.map_err(io::Error::other)?;
    require(
        response.is_error != Some(true),
        "restored mouse operation missing",
    )?;
    let observed = response
        .structured_content
        .ok_or_else(|| io::Error::other("restored mouse metadata missing"))?;
    std::fs::write(
        provider.root.join(format!("{label}-operation.json")),
        serde_json::to_vec(&observed).map_err(io::Error::other)?,
    )?;
    require(
        matches!(
            observed["hold"]["state"].as_str(),
            Some("reset" | "expired")
        ),
        "frontend restore retained old mouse authority",
    )?;
    for action in ["up", "renew"] {
        let mut args = json!({"action":action,"handle":held["hold"]["handle"]});
        if action == "renew" {
            args["lease_ms"] = 30000.into();
        }
        let mut request = CallToolRequestParams::new("mouse");
        request.arguments = Some(args.as_object().unwrap().clone());
        let refused = client.call_tool(request).await.map_err(io::Error::other)?;
        require(
            refused.is_error == Some(true),
            "old mouse action accepted after frontend restore",
        )?;
        std::fs::write(
            provider
                .root
                .join(format!("{label}-{action}-rejected.json")),
            serde_json::to_vec(&refused).map_err(io::Error::other)?,
        )?;
        require(
            mouse_buttons(provider, &format!("{label}-preserved-{action}"))? == 1,
            "old mouse action changed restored button state",
        )?;
    }
    Ok(())
}

pub(super) async fn journey(
    provider: &mut Provider,
    child: &mut tokio::process::Child,
    display: &str,
    load: bool,
    mcp: bool,
) -> io::Result<()> {
    connect(provider, child).await?;
    let pid = child
        .id()
        .ok_or_else(|| io::Error::other("native frontend PID missing"))?
        .to_string();
    let window = xdo(display, &["search", "--pid", &pid, "--name", "Rust-DOS"]).await?;
    require(
        !window.is_empty() && !window.contains('\n'),
        "unique SDL window required",
    )?;
    xdo(display, &["windowfocus", "--sync", &window]).await?;
    if !mcp {
        return default_slot_load(provider, display, &window).await;
    }
    let client = tokio::time::timeout(
        Duration::from_secs(10),
        ClientConfig::default()
            .with_protocol_version(ProtocolVersion::V_2026_07_28)
            .serve_with_lifecycle(
                StreamableHttpClientTransport::from_uri(format!("{}/mcp", provider.url)),
                ClientLifecycleMode::Discover {
                    preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                },
            ),
    )
    .await
    .map_err(io::Error::other)?
    .map_err(io::Error::other)?;
    let result = tokio::time::timeout(Duration::from_secs(45), async {
        settings_transition(provider, &client, display, &window).await?;
        let mouse = mouse_hold(provider, &client, "frontend-mouse-down").await?;
        let mut request = CallToolRequestParams::new("key");
        request.arguments = Some(
            json!({"action":"down","key":"shift","lease_ms":30000})
                .as_object()
                .unwrap()
                .clone(),
        );
        let down = client.call_tool(request).await.map_err(io::Error::other)?;
        require(
            down.is_error != Some(true),
            "frontend native Shift down rejected",
        )?;
        let down = down
            .structured_content
            .ok_or_else(|| io::Error::other("native hold response missing"))?;
        std::fs::write(
            provider.root.join("frontend-down.json"),
            serde_json::to_vec_pretty(&down).map_err(io::Error::other)?,
        )?;
        let deadline = Instant::now() + Duration::from_secs(5);
        while !shift(provider, "frontend-shift")? {
            require(Instant::now() < deadline, "actual Shift delivery deadline")?;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        for key in ["f12", "s"] {
            let mut request = CallToolRequestParams::new("key");
            request.arguments = Some(json!({"key":key,"mods":["ctrl","shift"],"hold_ms":1}).as_object().unwrap().clone());
            let accepted = client.call_tool(request).await.map_err(io::Error::other)?;
            require(accepted.is_error != Some(true), "native guest shortcut rejected")?;
            let accepted = accepted.structured_content.ok_or_else(|| io::Error::other("shortcut operation missing"))?;
            let deadline = Instant::now() + Duration::from_secs(5);
            while provider.get(&format!("frontend-shortcut-{key}-op"), &format!("/api/ops/{}", accepted["op"].as_u64().ok_or_else(|| io::Error::other("shortcut op missing"))?))?["state"] != "done" {
                require(Instant::now() < deadline, "native shortcut delivery deadline")?;
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            require(provider.get(&format!("frontend-shortcut-{key}-status"), "/api/status")?["settings_window"] == false, "native guest chord opened frontend settings")?;
            require(shift(provider, &format!("frontend-shortcut-{key}-shift"))?, "guest shortcut released its borrowed Shift hold")?;
        }
        if load {
            return slot_load(provider, &client, display, &window, &down, &mouse).await;
        }
        // Leave physical Alt+F11 down so the assertion runs during rewind,
        // before the frontend's existing key-up notification.
        tokio::time::sleep(Duration::from_millis(1200)).await;
        let before = bios_ticks(provider, "frontend-before-rewind")?;
        xdo(display, &["keydown", "--window", &window, "alt+F11"]).await?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if bios_ticks(provider, "frontend-during-rewind")? < before {
                break;
            }
            require(
                Instant::now() < deadline,
                "actual frontend rewind did not replace guest state",
            )?;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let mut query = CallToolRequestParams::new("operation");
        query.arguments = Some(
            json!({"instance":down["instance"],"op":down["op"]})
                .as_object()
                .unwrap()
                .clone(),
        );
        let observed = client.call_tool(query).await.map_err(io::Error::other)?;
        let observed = observed
            .structured_content
            .ok_or_else(|| io::Error::other("native operation result missing"))?;
        std::fs::write(
            provider.root.join("frontend-rewind-operation.json"),
            serde_json::to_vec_pretty(&observed).map_err(io::Error::other)?,
        )?;
        require(
            observed["hold"]["state"] == "reset",
            "frontend rewind must invalidate old native hold before subsequent actor polling",
        )?;
        require(
            shift(provider, "frontend-restored-shift")?,
            "old native cleanup released Shift into the rewound guest",
        )?;
        mouse_restored(provider, &client, &mouse, "frontend-mouse-rewind").await?;
        let mut up = CallToolRequestParams::new("key");
        up.arguments = Some(
            json!({"action":"up","handle":down["hold"]["handle"]})
                .as_object()
                .unwrap()
                .clone(),
        );
        require(
            client
                .call_tool(up)
                .await
                .map_err(io::Error::other)?
                .is_error
                == Some(true),
            "old up must be rejected during frontend rewind",
        )?;
        Ok(())
    })
    .await
    .map_err(io::Error::other)
    .and_then(|result| result);
    let release = if load {
        Ok(String::new())
    } else {
        xdo(display, &["keyup", "--window", &window, "alt+F11"]).await
    };
    let drag = if result.is_ok() && release.is_ok() {
        queued_drag_restore(provider, &client, display, &window, load).await
    } else {
        Ok(())
    };
    let cancelled = client.cancel().await.map_err(io::Error::other);
    result?;
    release?;
    drag?;
    cancelled?;
    let source = Path::new(env!("CARGO_MANIFEST_DIR"));
    let record = json!({"journey":if load {"load"} else {"rewind"}, "main_sha256":source_digest(&source.join("src/main.rs"))?, "actor_sha256":source_digest(&source.join("src/debug/mod.rs"))?, "producer_sha256":source_digest(&provider.root.join("native-rust-dos"))?, "operation":serde_json::from_slice::<Value>(&read_bounded(&provider.root.join(if load {"frontend-load-operation.json"} else {"frontend-rewind-operation.json"}),8192)?).map_err(io::Error::other)?});
    std::fs::write(
        provider.root.join("frontend-result.json"),
        serde_json::to_vec_pretty(&record).map_err(io::Error::other)?,
    )?;
    Ok(())
}

async fn queued_drag_restore(
    provider: &Provider,
    client: &rmcp::service::RunningService<rmcp::RoleClient, ClientConfig>,
    display: &str,
    window: &str,
    load: bool,
) -> io::Result<()> {
    let mode = if load { "load" } else { "rewind" };
    let label = format!("frontend-drag-{mode}");
    provider.input(
        &format!("{label}-saved-button"),
        "mouse",
        json!({"action":"down","button":"left"}),
    )?;
    require(
        mouse_buttons(provider, &format!("{label}-saved-status"))? == 1,
        "queued drag setup requires a held snapshot button",
    )?;
    if load {
        xdo(display, &["key", "--window", window, "ctrl+F3"]).await?;
        xdo(display, &["key", "--window", window, "ctrl+F1"]).await?;
        let slot = provider.root.join("states/dos/slot2.state");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !slot.is_file() {
            require(Instant::now() < deadline, "queued drag slot save deadline")?;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        require(
            rust_dos::savestate::slots::read_file_header(&slot).is_some(),
            "queued drag slot header invalid",
        )?;
    } else {
        tokio::time::sleep(Duration::from_millis(1200)).await;
    }
    let before = bios_ticks(provider, &format!("{label}-before-release"))?;
    provider.input(
        &format!("{label}-live-release"),
        "mouse",
        json!({"action":"up","button":"left"}),
    )?;
    require(
        mouse_buttons(provider, &format!("{label}-released-status"))? == 0,
        "queued drag setup did not release the live button",
    )?;
    let capture = client
        .call_tool(CallToolRequestParams::new("capture"))
        .await
        .map_err(io::Error::other)?;
    require(
        capture.is_error != Some(true),
        "queued drag picture capture rejected",
    )?;
    let picture = capture
        .structured_content
        .ok_or_else(|| io::Error::other("queued drag picture missing"))?["mouse_picture"]
        .clone();
    provider.post(
        &format!("{label}-barrier"),
        "/api/input/wait?wait=false",
        json!({"ms":1500}),
    )?;
    let accepted = mouse_call(
        provider,
        client,
        &format!("{label}-accepted"),
        json!({
            "action":"drag","picture":picture,"button":"left",
            "path":[{"x":3,"y":4,"delay_ms":1},{"x":20,"y":20,"delay_ms":1}]
        }),
    )
    .await?;
    let operation = accepted["op"]
        .as_u64()
        .ok_or_else(|| io::Error::other("queued drag operation missing"))?;
    let path = format!("/api/ops/{operation}");
    require(
        provider.get(&format!("{label}-queued-operation"), &path)?["state"] == "pending",
        "finite drag must still be queued before replacement",
    )?;
    require(
        mouse_buttons(provider, &format!("{label}-queued-status"))? == 0,
        "queued finite drag delivered its button early",
    )?;
    let restore_key = if load {
        ["key", "--window", window, "ctrl+F2"]
    } else {
        ["keydown", "--window", window, "alt+F11"]
    };
    xdo(display, &restore_key).await?;
    let restored = async {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if bios_ticks(provider, &format!("{label}-during-restore"))? < before
                && mouse_buttons(provider, &format!("{label}-restored-status"))? == 1
            {
                break;
            }
            require(
                Instant::now() < deadline,
                "queued drag frontend replacement deadline",
            )?;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok::<_, io::Error>(())
    }
    .await;
    let release = if load {
        Ok(String::new())
    } else {
        xdo(display, &["keyup", "--window", window, "alt+F11"]).await
    };
    restored?;
    release?;
    require(
        provider.get(&format!("{label}-restored-operation"), &path)?["state"] == "cancelled",
        "frontend restore retained queued finite drag work",
    )?;
    let mouse = provider.get(&format!("{label}-restored-status"), "/api/status")?["mouse"].clone();
    require(
        mouse["buttons"] == 1,
        "obsolete drag cleanup released the restored button",
    )?;
    tokio::time::sleep(Duration::from_millis(1700)).await;
    require(
        provider.get(&format!("{label}-after-status"), "/api/status")?["mouse"] == mouse,
        "obsolete drag points or releases changed restored mouse state",
    )?;
    Ok(())
}

async fn default_slot_load(provider: &Provider, display: &str, window: &str) -> io::Result<()> {
    provider.input(
        "default-shift-down",
        "key",
        json!({"key":"shift","action":"down"}),
    )?;
    require(
        shift(provider, "default-shift-delivered")?,
        "default frontend Shift setup failed",
    )?;
    xdo(
        display,
        &["key", "--delay", "0", "--window", window, "ctrl+F1"],
    )
    .await?;
    let slot = provider.root.join("states/dos/slot1.state");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !slot.is_file() {
        require(
            Instant::now() < deadline,
            "default frontend slot save deadline",
        )?;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    require(
        rust_dos::savestate::slots::read_file_header(&slot).is_some(),
        "default frontend slot header invalid",
    )?;
    require(
        shift(provider, "default-saved-shift")?,
        "default slot setup lost Shift",
    )?;
    tokio::time::sleep(Duration::from_millis(150)).await;
    let before = bios_ticks(provider, "default-before-load")?;
    xdo(
        display,
        &["key", "--delay", "0", "--window", window, "ctrl+F2"],
    )
    .await?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if bios_ticks(provider, "default-during-load")? < before {
            break;
        }
        require(
            Instant::now() < deadline,
            "default frontend slot did not load",
        )?;
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    require(
        !shift(provider, "default-loaded-shift")?,
        "MCP opt-out must preserve upstream frontend key release",
    )?;
    let source = Path::new(env!("CARGO_MANIFEST_DIR"));
    let record = json!({"journey":"default-load", "main_sha256":source_digest(&source.join("src/main.rs"))?, "actor_sha256":source_digest(&source.join("src/debug/mod.rs"))?, "producer_sha256":source_digest(&provider.root.join("native-rust-dos"))?, "saved_header":rust_dos::savestate::slots::read_file_header(&slot).is_some(), "before_ticks":before, "after_ticks":bios_ticks(provider, "default-after-load")?});
    std::fs::write(
        provider.root.join("frontend-result.json"),
        serde_json::to_vec_pretty(&record).map_err(io::Error::other)?,
    )?;
    Ok(())
}

async fn settings_transition(
    provider: &Provider,
    client: &rmcp::service::RunningService<rmcp::RoleClient, ClientConfig>,
    display: &str,
    window: &str,
) -> io::Result<()> {
    let held_mouse = mouse_hold(provider, client, "frontend-settings-mouse-held").await?;
    let capture = client
        .call_tool(CallToolRequestParams::new("capture"))
        .await
        .map_err(io::Error::other)?;
    require(
        capture.is_error != Some(true),
        "settings mouse capture rejected",
    )?;
    let picture = capture
        .structured_content
        .ok_or_else(|| io::Error::other("settings picture missing"))?["mouse_picture"]
        .clone();
    provider.post(
        "frontend-settings-wait",
        "/api/input/wait?wait=false",
        json!({"ms":2000}),
    )?;
    let mut operations = Vec::new();
    for (tool, args) in [
        ("key", json!({"key":"a"})),
        ("type", json!({"text":"dir"})),
        (
            "mouse",
            json!({"action":"down","picture":picture,"button":"right","lease_ms":30000}),
        ),
    ] {
        let mut request = CallToolRequestParams::new(tool);
        request.arguments = Some(args.as_object().unwrap().clone());
        let accepted = client.call_tool(request).await.map_err(io::Error::other)?;
        require(
            accepted.is_error != Some(true),
            "pending frontend input rejected",
        )?;
        operations.push(
            accepted
                .structured_content
                .ok_or_else(|| io::Error::other("pending frontend operation missing"))?,
        );
    }
    xdo(display, &["key", "--window", window, "ctrl+F12"]).await?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while provider.get("frontend-settings-open", "/api/status")?["settings_window"] != true {
        require(
            Instant::now() < deadline,
            "physical settings opening deadline",
        )?;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    for (index, operation) in operations.iter().enumerate() {
        let mut request = CallToolRequestParams::new("operation");
        request.arguments = Some(
            json!({"instance":operation["instance"],"op":operation["op"]})
                .as_object()
                .unwrap()
                .clone(),
        );
        let observed = client.call_tool(request).await.map_err(io::Error::other)?;
        require(
            observed.is_error != Some(true),
            "settings transition operation unavailable",
        )?;
        let observed = observed
            .structured_content
            .ok_or_else(|| io::Error::other("settings operation missing"))?;
        require(
            observed["delivery"] == "cancelled",
            "physical settings opening must cancel pending native input",
        )?;
        std::fs::write(
            provider
                .root
                .join(format!("frontend-settings-operation-{index}.json")),
            serde_json::to_vec_pretty(&observed).map_err(io::Error::other)?,
        )?;
    }
    require(
        mouse_buttons(provider, "frontend-settings-mouse-released")? == 0,
        "settings diversion must release only the delivered native mouse claim",
    )?;
    let mut query = CallToolRequestParams::new("operation");
    query.arguments = Some(
        json!({"instance":held_mouse["instance"],"op":held_mouse["op"]})
            .as_object()
            .unwrap()
            .clone(),
    );
    let observed = client.call_tool(query).await.map_err(io::Error::other)?;
    require(
        observed.is_error != Some(true),
        "settings held mouse query rejected",
    )?;
    let observed = observed
        .structured_content
        .ok_or_else(|| io::Error::other("settings hold metadata missing"))?;
    require(
        observed["hold"]["state"] == "reset",
        "settings opening retained native mouse authority",
    )?;
    std::fs::write(
        provider.root.join("frontend-settings-mouse-retired.json"),
        serde_json::to_vec(&observed).map_err(io::Error::other)?,
    )?;
    xdo(display, &["key", "--window", window, "ctrl+F12"]).await?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while provider.get("frontend-settings-closed", "/api/status")?["settings_window"] != false {
        require(
            Instant::now() < deadline,
            "physical settings closing deadline",
        )?;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // The unrelated HTTP wait is deliberately allowed to finish normally.
    tokio::time::sleep(Duration::from_millis(2100)).await;
    Ok(())
}

async fn slot_load(
    provider: &Provider,
    client: &rmcp::service::RunningService<rmcp::RoleClient, ClientConfig>,
    display: &str,
    window: &str,
    down: &Value,
    mouse: &Value,
) -> io::Result<()> {
    xdo(
        display,
        &["key", "--delay", "0", "--window", window, "ctrl+F1"],
    )
    .await?;
    let slot = provider.root.join("states/dos/slot1.state");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !slot.is_file() {
        require(
            Instant::now() < deadline,
            "actual frontend slot save deadline",
        )?;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    require(
        rust_dos::savestate::slots::read_file_header(&slot).is_some(),
        "actual frontend slot header is invalid",
    )?;
    require(
        shift(provider, "frontend-saved-shift")?,
        "slot-save setup lost the delivered Shift state",
    )?;
    tokio::time::sleep(Duration::from_millis(150)).await;
    let before = bios_ticks(provider, "frontend-before-load")?;
    mouse_call(
        provider,
        client,
        "frontend-mouse-renew-before-load",
        json!({"action":"renew","handle":mouse["hold"]["handle"],"lease_ms":10}),
    )
    .await?;
    let mut renew = CallToolRequestParams::new("key");
    renew.arguments = Some(
        json!({"action":"renew","handle":down["hold"]["handle"],"lease_ms":10})
            .as_object()
            .unwrap()
            .clone(),
    );
    let renewed = client.call_tool(renew).await.map_err(io::Error::other)?;
    require(
        renewed.is_error != Some(true),
        "hold renewal before frontend load rejected",
    )?;
    xdo(
        display,
        &["key", "--delay", "0", "--window", window, "ctrl+F2"],
    )
    .await?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if bios_ticks(provider, "frontend-during-load")? < before {
            break;
        }
        require(
            Instant::now() < deadline,
            "actual frontend slot load did not replace guest state",
        )?;
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    require(
        shift(provider, "frontend-loaded-shift")?,
        "old hold expiry released Shift into the frontend-loaded guest",
    )?;
    mouse_restored(provider, client, mouse, "frontend-mouse-load").await?;
    let mut query = CallToolRequestParams::new("operation");
    query.arguments = Some(
        json!({"instance":down["instance"],"op":down["op"]})
            .as_object()
            .unwrap()
            .clone(),
    );
    let observed = client
        .call_tool(query)
        .await
        .map_err(io::Error::other)?
        .structured_content
        .ok_or_else(|| io::Error::other("native operation result missing"))?;
    std::fs::write(
        provider.root.join("frontend-load-operation.json"),
        serde_json::to_vec_pretty(&observed).map_err(io::Error::other)?,
    )?;
    require(
        observed["hold"]["state"] == "reset" || observed["hold"]["state"] == "expired",
        "frontend load retained old native hold authority",
    )?;
    for args in [
        json!({"action":"up","handle":down["hold"]["handle"]}),
        json!({"action":"renew","handle":down["hold"]["handle"],"lease_ms":30000}),
    ] {
        let mut request = CallToolRequestParams::new("key");
        request.arguments = Some(args.as_object().unwrap().clone());
        require(
            client
                .call_tool(request)
                .await
                .map_err(io::Error::other)?
                .is_error
                == Some(true),
            "old up/renew must be rejected after frontend load",
        )?;
        require(
            shift(provider, "frontend-loaded-shift-after-old-action")?,
            "old action changed frontend-loaded Shift state",
        )?;
    }
    Ok(())
}
