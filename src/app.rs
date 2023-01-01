use std::sync::Arc;

use crate::{cache, config, http, jobs};

#[derive(Debug)]
pub struct App {
    config: config::Config,
    server: http::Server,
    cache: cache::Cache,
    workers: jobs::Workers,
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
        let server = http::Server::new();
        let cache = cache::Cache::new(&config).await?;
        let workers = jobs::Workers::new().await?;

        Ok(Self {
            config,
            server,
            cache,
            workers,
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
            self.workers.run(state.clone())
        )?;

        self.cache.cleanup().await;

        Ok(())
    }
}
