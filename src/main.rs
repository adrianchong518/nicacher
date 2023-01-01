mod app;
mod cache;
mod config;
mod fetch;
mod http;
mod jobs;
mod nix;

use anyhow::Context as _;

const PKG_NAME: &str = env!("CARGO_PKG_NAME");

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    {
        use tracing::subscriber::set_global_default;
        use tracing_subscriber::filter::EnvFilter;
        use tracing_subscriber::prelude::*;

        tracing_log::LogTracer::init().context("Failed to set logger")?;

        let env_filter = EnvFilter::try_from_env("NICACHER_LOG")
            .unwrap_or_else(|_| EnvFilter::new("info"))
            .add_directive(
                "sqlx::query=warn"
                    .parse()
                    .context("Invalid sqlx tracing_subscriber directive")?,
            );

        let formatting_layer =
            tracing_bunyan_formatter::BunyanFormattingLayer::new(PKG_NAME.into(), std::io::stdout);

        let subscriber = tracing_subscriber::Registry::default()
            .with(formatting_layer)
            .with(tracing_bunyan_formatter::JsonStorageLayer)
            .with(env_filter);

        set_global_default(subscriber).context("Failed to set subscriber")?;
    }

    let app = app::App::new().await?;

    tracing::info!("Nicacher server starting");

    app.run().await?;

    Ok(())
}
