//! Opt-in context diagnostics. SDK messages can contain complete guest payloads.
use tracing_subscriber::{Layer, layer::SubscriberExt};

pub(super) const TARGET: &str = "rust_dos::mcp::context";

fn subscriber<W>(writer: W) -> impl tracing::Subscriber + Send + Sync
where
    W: for<'a> tracing_subscriber::fmt::MakeWriter<'a> + Send + Sync + 'static,
{
    tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(writer)
            .with_filter(tracing_subscriber::filter::filter_fn(|metadata| {
                metadata.target() == TARGET
            })),
    )
}

pub(super) fn install() -> Result<(), String> {
    tracing::subscriber::set_global_default(subscriber(std::io::stderr))
        .map_err(|error| format!("cannot install payload-free MCP diagnostics: {error}"))
}
