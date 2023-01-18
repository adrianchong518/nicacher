mod admin;
mod api;

use anyhow::Context as _;

use crate::app;

#[derive(Debug)]
pub struct Server {
    router: axum::Router<app::State>,
}

impl Server {
    #[tracing::instrument(name = "server_init")]
    pub fn new() -> Self {
        use tower_http::trace::TraceLayer;

        let router = api::router().layer(TraceLayer::new_for_http());

        Self { router }
    }

    pub async fn run(self, state: app::State) -> anyhow::Result<()> {
        let server = axum::Server::bind(&"0.0.0.0:8080".parse().unwrap())
            .serve(self.router.with_state(state).into_make_service())
            .with_graceful_shutdown(shutdown_signal());

        tracing::info!("Starting http server");

        server.await.context("Http server error")?;

        tracing::debug!("Http server stopped");

        Ok(())
    }
}

async fn shutdown_signal() {
    use tokio::signal;

    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    println!("signal received, starting graceful shutdown");
}
