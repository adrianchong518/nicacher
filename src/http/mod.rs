mod admin;
mod api;

use anyhow::Context as _;

use crate::app;

#[derive(Debug)]
pub struct Server {
    router: axum::Router<app::State>,
    shutdown_rx: tokio::sync::oneshot::Receiver<()>,
}

impl Server {
    #[tracing::instrument(name = "server_init")]
    pub fn new(shutdown_rx: tokio::sync::oneshot::Receiver<()>) -> Self {
        use tower_http::trace::TraceLayer;

        let router = api::router().layer(TraceLayer::new_for_http());

        Self {
            router,
            shutdown_rx,
        }
    }

    pub async fn run(self, state: app::State) -> anyhow::Result<()> {
        let server = axum::Server::bind(&"0.0.0.0:8080".parse().unwrap())
            .serve(self.router.with_state(state).into_make_service())
            .with_graceful_shutdown(async {
                self.shutdown_rx.await.ok();
            });

        tracing::info!("Starting http server");

        server.await.context("Http server error")?;

        Ok(())
    }
}

type Result<T> = std::result::Result<T, Error>;

struct Error(anyhow::Error);

impl axum::response::IntoResponse for Error {
    fn into_response(self) -> axum::response::Response {
        tracing::error!("{:?}", self.0);

        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!(
                "Failed to handle request due to internal server error:\n{:?}",
                self.0
            ),
        )
            .into_response()
    }
}

impl<E> From<E> for Error
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self(err.into())
    }
}