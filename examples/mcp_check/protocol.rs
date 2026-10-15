//! Actual reference SDK exchanges, separate from native guest capability tests.
use super::acquisition::{read_bounded, require, source_digest};
use base64::Engine;
use http_body_util::BodyExt;
use rmcp::{
    ServerHandler,
    model::{ClientConfig, ProtocolVersion, ServerCapabilities, ServerConfig},
    service::{ClientLifecycleMode, ClientServiceExt},
    transport::{
        StreamableHttpClientTransport,
        streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
        },
    },
};
use serde_json::{Value, json};
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

#[derive(Clone)]
struct Reference;
impl ServerHandler for Reference {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Provenance {
    schema_version: u32,
    sdk: String,
    version: String,
    protocols: Vec<String>,
    sources: std::collections::BTreeMap<String, String>,
    wire_sha256: String,
    native_producer: Executable,
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Executable {
    path: PathBuf,
    sha256: String,
}

pub(super) fn verify_reference(metadata: &[u8], wire: &[u8]) -> io::Result<()> {
    let provenance: Provenance = serde_json::from_slice(metadata).map_err(io::Error::other)?;
    require(
        provenance.schema_version == 1
            && provenance.sdk == "rmcp"
            && provenance.version == "3.5.1"
            && provenance.protocols == ["2025-11-25", "2026-07-28"]
            && provenance.sources.len() == 3,
        "reference SDK provenance contract mismatch",
    )?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).canonicalize()?;
    for path in ["Cargo.lock", "Cargo.toml", "examples/mcp_check/protocol.rs"] {
        let digest = provenance
            .sources
            .get(path)
            .ok_or_else(|| io::Error::other("reference SDK source binding missing"))?;
        super::acquisition::verify_source(
            &super::acquisition::confined(&root, Path::new(path))?,
            digest,
        )?;
    }
    require(
        provenance.native_producer.path.is_absolute(),
        "reference SDK executable path must be absolute",
    )?;
    super::acquisition::verify_source(
        &provenance.native_producer.path,
        &provenance.native_producer.sha256,
    )?;
    use sha2::Digest;
    let mut hash = sha2::Sha256::new();
    hash.update(wire);
    require(
        super::acquisition::hex_digest(hash) == provenance.wire_sha256,
        "reference SDK wire hash mismatch",
    )?;
    super::acquisition::verify_protocol(wire)
}

struct Wire {
    file: Mutex<(std::fs::File, usize)>,
    next: AtomicU64,
    failed: AtomicBool,
}
impl Wire {
    fn record(&self, value: Value) -> io::Result<()> {
        let bytes = serde_json::to_vec(&value).map_err(io::Error::other)?;
        let mut file = self
            .file
            .lock()
            .map_err(|_| io::Error::other("reference wire lock poisoned"))?;
        file.1 = file
            .1
            .checked_add(bytes.len() + 1)
            .filter(|&total| total <= 1024 * 1024)
            .ok_or_else(|| io::Error::other("reference wire byte bound"))?;
        file.0.write_all(&bytes)?;
        file.0.write_all(b"\n")?;
        file.0.flush()
    }

    fn capture(&self, value: Value) -> bool {
        if self.record(value).is_err() {
            self.failed.store(true, Ordering::Relaxed);
            return false;
        }
        true
    }
}

async fn record_wire(
    axum::extract::State(wire): axum::extract::State<Arc<Wire>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let id = wire.next.fetch_add(1, Ordering::Relaxed);
    let (parts, body) = request.into_parts();
    let bytes = match axum::body::to_bytes(body, 16 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => {
            wire.failed.store(true, Ordering::Relaxed);
            return axum::http::StatusCode::PAYLOAD_TOO_LARGE.into_response();
        }
    };
    if !wire.capture(json!({"id":id,"kind":"request","method":parts.method.as_str(),"uri":parts.uri.to_string(),"headers":format!("{:?}",parts.headers),"base64":base64::engine::general_purpose::STANDARD.encode(&bytes)})) {
        return axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let response = next
        .run(axum::extract::Request::from_parts(
            parts,
            axum::body::Body::from(bytes),
        ))
        .await;
    let (parts, body) = response.into_parts();
    if !wire.capture(json!({"id":id,"kind":"response","status":parts.status.as_u16(),"headers":format!("{:?}",parts.headers)})) {
        return axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let body = body.map_frame(move |frame| {
        if let Some(bytes) = frame.data_ref() {
            if bytes.len() > 16 * 1024 { wire.failed.store(true, Ordering::Relaxed); }
            else { wire.capture(json!({"id":id,"kind":"response_chunk","base64":base64::engine::general_purpose::STANDARD.encode(bytes)})); }
        }
        frame
    });
    axum::response::Response::from_parts(parts, axum::body::Body::new(body))
}

async fn exchanges(uri: String) -> io::Result<()> {
    for version in [ProtocolVersion::V_2025_11_25, ProtocolVersion::V_2026_07_28] {
        let transport = StreamableHttpClientTransport::from_uri(uri.clone());
        let lifecycle = if version.has_initialize() {
            ClientLifecycleMode::Initialize
        } else {
            ClientLifecycleMode::Discover {
                preferred_versions: vec![version.clone()],
            }
        };
        let client = ClientConfig::default()
            .with_protocol_version(version.clone())
            .serve_with_lifecycle(transport, lifecycle)
            .await
            .map_err(io::Error::other)?;
        let result = async {
            require(
                client
                    .list_all_tools()
                    .await
                    .map_err(io::Error::other)?
                    .is_empty(),
                "reference probe must expose no guest tools",
            )?;
            require(
                client
                    .peer_info()
                    .is_some_and(|peer| peer.protocol_version == version),
                "reference negotiation mismatch",
            )
        }
        .await;
        client.cancel().await.map_err(io::Error::other)?;
        result?;
    }
    tokio::task::spawn_blocking(move || {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(2)))
            .max_redirects(0)
            .build();
        let result = ureq::Agent::new_with_config(config)
            .get(&uri)
            .header("Origin", "http://example.invalid")
            .call();
        require(
            matches!(result, Err(ureq::Error::StatusCode(403))),
            "reference Origin rejection missing",
        )
    })
    .await
    .map_err(io::Error::other)??;
    Ok(())
}

pub(super) async fn acquire(root: &Path) -> io::Result<()> {
    super::replay::validate_root(root)?;
    std::fs::create_dir(root)?;
    let wire = Arc::new(Wire {
        file: Mutex::new((std::fs::File::create(root.join("wire.jsonl"))?, 0)),
        next: AtomicU64::new(1),
        failed: AtomicBool::new(false),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(true)
        .with_allowed_hosts([address.to_string()])
        .enforce_origin_validation();
    let cancellation = config.cancellation_token.clone();
    let service = StreamableHttpService::new(
        || Ok(Reference),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    let router = axum::Router::new().nest_service("/mcp", service).layer(
        axum::middleware::from_fn_with_state(wire.clone(), record_wire),
    );
    let shutdown = cancellation.clone();
    let mut server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await
    });
    let result = tokio::time::timeout(
        Duration::from_secs(20),
        exchanges(format!("http://{address}/mcp")),
    )
    .await
    .map_err(|_| io::Error::other("reference protocol journey deadline"));
    cancellation.cancel();
    match tokio::time::timeout(Duration::from_secs(5), &mut server).await {
        Ok(result) => {
            result.map_err(io::Error::other)??;
        }
        Err(_) => {
            server.abort();
            let _ = server.await;
            return Err(io::Error::other("reference server shutdown deadline"));
        }
    }
    result??;
    require(
        !wire.failed.load(Ordering::Relaxed),
        "reference wire recording failed",
    )?;
    super::acquisition::verify_protocol(&read_bounded(&root.join("wire.jsonl"), 1024 * 1024)?)?;
    let mut sources = serde_json::Map::new();
    for path in ["Cargo.lock", "Cargo.toml", "examples/mcp_check/protocol.rs"] {
        sources.insert(path.to_owned(), json!(source_digest(Path::new(path))?));
    }
    let executable = root.join(if cfg!(windows) {
        "reference-sdk-probe.exe"
    } else {
        "reference-sdk-probe"
    });
    let running = std::env::current_exe()?;
    source_digest(&running)?;
    std::fs::copy(running, &executable)?;
    std::fs::write(root.join("producer.json"), serde_json::to_vec_pretty(&json!({"schema_version":1,"sdk":"rmcp","version":"3.5.1","protocols":["2025-11-25","2026-07-28"],"sources":sources,"wire_sha256":source_digest(&root.join("wire.jsonl"))?,"native_producer":{"path":executable,"sha256":source_digest(&executable)?}})).map_err(io::Error::other)?)?;
    verify_reference(
        &read_bounded(&root.join("producer.json"), 64 * 1024)?,
        &read_bounded(&root.join("wire.jsonl"), 1024 * 1024)?,
    )?;
    println!(
        "Verified actual reference SDK negotiations and Origin rejection: {}",
        root.display()
    );
    Ok(())
}
