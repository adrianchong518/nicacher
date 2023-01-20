pub type Result<T> = std::result::Result<T, Error>;

pub struct Error(anyhow::Error);

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

impl From<Error> for apalis::prelude::JobError {
    fn from(err: Error) -> Self {
        Self::Failed(err.0.into())
    }
}

trait IntoJobError {
    fn into_job_error(self) -> apalis::prelude::JobError;
}
