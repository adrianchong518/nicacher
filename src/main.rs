mod app;
mod cache;
mod config;
mod fetch;
mod http;
mod jobs;
mod nix;

use anyhow::Context as _;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    {
        use tracing_subscriber::{filter::EnvFilter, fmt::format::FmtSpan, prelude::*};

        tracing_log::LogTracer::init().context("Failed to set logger")?;

        let env_filter = EnvFilter::try_from_env("NICACHER_LOG")
            .unwrap_or_else(|_| EnvFilter::new("info"))
            .add_directive(
                "sqlx::query=warn"
                    .parse()
                    .context("Invalid sqlx tracing_subscriber directive")?,
            );

        let formatting_layer = tracing_subscriber::fmt::layer()
            .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
            .pretty();

        let subscriber = tracing_subscriber::registry()
            .with(formatting_layer)
            .with(env_filter);

        tracing::subscriber::set_global_default(subscriber).context("Failed to set subscriber")?;

        // Set up panic logging
        std::panic::set_hook(Box::new(|panic| {
            if let Some(location) = panic.location() {
                tracing::error!(
                    message = %panic,
                    panic.file = location.file(),
                    panic.line = location.line(),
                    panic.column = location.column(),
                    panic.backtrace = ?backtrace::Backtrace::new()
                );
            } else {
                tracing::error!(message = %panic);
            }
        }));
    }

    let app = app::App::new().await?;

    tracing::info!("Nicacher server starting");

    app.run().await?;

    Ok(())
}
