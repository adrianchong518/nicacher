use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;

pub struct Error(anyhow::Error);

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl axum::response::IntoResponse for Error {
    fn into_response(self) -> axum::response::Response {
        tracing::error!("{:?}", self);

        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!(
                "Failed to handle request due to internal server error:\n{:?}",
                self
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
