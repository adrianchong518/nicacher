use std::sync::Arc;

use crate::{cache, config, http, jobs};

#[derive(Debug)]
pub struct App {
    config: config::Config,
    server: http::Server,
    cache: cache::Cache,
    workers: jobs::Workers,

    server_shutdown_tx: tokio::sync::oneshot::Sender<()>,
}

#[derive(Clone, Debug)]
pub struct State {
    pub config: Arc<config::Config>,
    pub cache: cache::Cache,
    pub workers: jobs::Workers,
}

impl App {
    #[tracing::instrument(name = "app_init")]
    pub async fn new() -> anyhow::Result<Self> {
        let config = config::Config::get();

        let (server_shutdown_tx, server_shutdown_rx) = tokio::sync::oneshot::channel();

        let server = http::Server::new(server_shutdown_rx);

        let cache = cache::Cache::new(&config).await?;
        let workers = jobs::Workers::new().await?;

        Ok(Self {
            config,
            server,
            cache,
            workers,
            server_shutdown_tx,
        })
    }

    pub async fn run(self) -> anyhow::Result<()> {
        let state = State {
            config: Arc::new(self.config),
            cache: self.cache.clone(),
            workers: self.workers.clone(),
        };

        tokio::try_join!(
            self.server.run(state.clone()),
            self.workers.run(state.clone(), self.server_shutdown_tx),
        )?;

        tracing::info!("Cleaning up cache database");
        self.cache.cleanup().await;

        Ok(())
    }
}
