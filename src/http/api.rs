use crate::{app, cache, http, jobs, nix};

use axum::{
    extract::{Path, State},
    http::{header, Request, StatusCode},
    response::IntoResponse,
};
use serde_with::DeserializeFromStr;

use anyhow::Context as _;
use tower::ServiceExt as _;

use std::str::FromStr;

pub(super) fn router() -> axum::Router<app::State> {
    use axum::routing::get;

    axum::Router::new()
        .route("/", get(index))
        .route("/nix-cache-info", get(nix_cache_info))
        .route("/:nar_info", get(get_nar_info))
        .route("/nar/:nar_file", get(get_nar_file))
        .nest("/admin", http::admin::router())
}

async fn index() -> impl IntoResponse {
    "Nicacher is up!"
}

async fn nix_cache_info() -> impl IntoResponse {
    "\
StoreDir: /nix/store
WantMassQuery: 0
Priority: 30"
}

#[derive(Debug, DeserializeFromStr)]
struct NarInfoPath(nix::Hash);

impl FromStr for NarInfoPath {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.rsplit_once('.') {
            Some((hash, "narinfo")) => Ok(Self(hash.parse()?)),

            _ => anyhow::bail!("Invalid narinfo path format: {s}"),
        }
    }
}

async fn get_nar_info(
    Path(NarInfoPath(hash)): Path<NarInfoPath>,
    State(app::State {
        cache, mut workers, ..
    }): State<app::State>,
) -> http::Result<impl IntoResponse> {
    tracing::info!("Request for {}.narinfo", hash.string);

    let nar_info = cache::db::get_nar_info(cache.db_pool(), &hash)
        .await
        .with_context(|| {
            format!(
                "Failed to get {}.narinfo due to internal error",
                hash.string
            )
        })?;

    if let Some(nar_info) = nar_info {
        cache::db::set_last_accessed(cache.db_pool(), &hash)
            .await
            .with_context(|| {
                format!(
                    "Failed to set last_accessed time for {}.narinfo due to internal error",
                    hash.string
                )
            })?;

        Ok((
            [(header::CONTENT_TYPE, nix::NARINFO_MIME)],
            nar_info.to_string(),
        )
            .into_response())
    } else {
        tracing::info!("Cache miss, pushing job to attempt caching");

        let job = jobs::Job::CacheNar {
            hash: hash.clone(),
            is_force: false,
        };

        workers.push_job(job.clone()).await.with_context(|| {
            format!(
                "Failed to request caching of {}.narinfo due to internal error",
                hash.string
            )
        })?;

        Ok((
            StatusCode::NOT_FOUND,
            format!("{}.narinfo unavaliable", hash.string),
        )
            .into_response())
    }
}

async fn get_nar_file(
    Path(nar_file): Path<nix::NarFile>,
    State(app::State { config, cache, .. }): State<app::State>,
) -> http::Result<impl IntoResponse> {
    tracing::info!("Request for {nar_file}");

    let res = (|| async {
        if cache::db::is_nar_file_cached(cache.db_pool(), &nar_file).await? {
            let nar_file_path = cache::nar_file_path_from_nar_file(&config, &nar_file);

            Ok(tower_http::services::ServeFile::new_with_mime(
                nar_file_path,
                &nix::NAR_FILE_MIME.parse().unwrap(),
            )
            .oneshot(Request::new(()))
            .await?
            .into_response())
        } else {
            tracing::debug!("{nar_file} not found");
            Ok::<_, anyhow::Error>(StatusCode::NOT_FOUND.into_response())
        }
    })()
    .await
    .with_context(|| format!("Failed to get {nar_file}"))?;

    Ok(res)
}
