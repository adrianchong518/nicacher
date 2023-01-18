use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;

use anyhow::Context as _;

use crate::{app, cache, error, fetch, jobs, nix};

pub(super) fn router() -> axum::Router<app::State> {
    use axum::routing::get;

    axum::Router::new()
        .route("/cache_size", get(cache_size))
        .route("/list_cached", get(list_cached))
        .route("/list_cache_diff", get(list_cache_diff))
        .route("/nar_status/:hash", get(nar_status))
        .route("/cache_nar/:hash", get(cache_nar))
        .route("/purge_nar/:hash", get(purge_nar))
}

async fn nar_status(
    Path(hash): Path<nix::Hash>,
    State(app::State { cache, .. }): State<app::State>,
) -> impl IntoResponse {
    format!("{:#?}", cache::status(cache.db_pool(), &hash).await)
}

async fn cache_size(
    State(app::State { config, cache, .. }): State<app::State>,
) -> error::Result<impl IntoResponse> {
    let disk_size = cache::disk_size(&config)
        .await
        .context("Failed to get total cache disk size")?;

    let nar_disk_size = cache::nar_disk_size(&config)
        .await
        .context("Failed to get total cached nar file disk size")?;

    let reported_size = cache::reported_size(cache.db_pool())
        .await
        .context("Failed to get reported cache size")?;

    Ok(format!(
        "\
Cache disk size: {disk_size} (nar: {nar_disk_size})
Cache reported size: {reported_size}"
    ))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct IsForce {
    #[serde(rename = "force")]
    is_force: bool,
}

async fn cache_nar(
    Path(hash): Path<nix::Hash>,
    Query(IsForce { is_force }): Query<IsForce>,
    State(app::State { mut workers, .. }): State<app::State>,
) -> error::Result<impl IntoResponse> {
    workers
        .push_job(jobs::Job::CacheNar {
            hash: hash.clone(),
            is_force,
        })
        .await
        .with_context(|| format!("Failed to push job for caching {} to queue", hash.string))?;

    Ok(format!("Pushed job for caching {} to queue", hash.string))
}

async fn purge_nar(
    Path(hash): Path<nix::Hash>,
    Query(IsForce { is_force }): Query<IsForce>,
    State(app::State {
        cache, mut workers, ..
    }): State<app::State>,
) -> error::Result<impl IntoResponse> {
    let is_cached = cache::is_cached_by_hash(cache.db_pool(), &hash)
        .await
        .with_context(|| format!("Failed to get information on {}.narinfo", hash.string))?;

    if is_cached {
        workers
            .push_job(jobs::Job::PurgeNar {
                hash: hash.clone(),
                is_force,
            })
            .await
            .with_context(|| format!("Failed to push job for purging {} to queue", hash.string))?;

        Ok((
            StatusCode::OK,
            format!("Pushed job for purging {} to queue", hash.string),
        ))
    } else {
        Ok((
            StatusCode::NOT_FOUND,
            format!("{}.narinfo is not cached", hash.string),
        ))
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct ListLimit {
    limit: usize,
}

impl Default for ListLimit {
    fn default() -> Self {
        Self { limit: 30 }
    }
}

async fn list_cached(
    Query(ListLimit { limit }): Query<ListLimit>,
    State(app::State { cache, .. }): State<app::State>,
) -> error::Result<impl IntoResponse> {
    let cached_store_paths = cache::get_store_paths::<_, Vec<_>>(cache.db_pool())
        .await
        .context("Failed to get cached store paths")?;

    Ok(format!(
        "\
Store paths of cached derivations: (limit: {limit})

{}",
        cached_store_paths
            .iter()
            .take(limit)
            .map(nix::StorePath::to_string)
            .reduce(|acc, path| acc + "\n" + &path)
            .unwrap()
    ))
}

async fn list_cache_diff(
    Query(ListLimit { limit }): Query<ListLimit>,
    State(app::State { config, cache, .. }): State<app::State>,
) -> error::Result<impl IntoResponse> {
    use std::collections::BTreeSet;

    let cached_store_paths = cache::get_store_paths::<_, BTreeSet<_>>(cache.db_pool())
        .await
        .context("Failed to get cached store paths")?;

    let upstream_store_paths =
        fetch::request_store_paths::<BTreeSet<_>>(&config, &config.channels[0])
            .await
            .context("Failed to request up-to-date store paths")?;

    tracing::debug!("Proccessing difference between local cache and upstream");

    let diff = upstream_store_paths.difference(&cached_store_paths);
    let diff_len = diff.clone().count();

    if diff_len == 0 {
        Ok("No missing derivations from cache".to_string())
    } else {
        Ok(format!(
            "\
Number of missing derivations from cache: {diff_len}
----------------------------------------------------
Store paths of missing derivations: (limit: {limit})

{}",
            diff.take(limit)
                .map(nix::StorePath::to_string)
                .reduce(|acc, path| acc + "\n" + &path)
                .unwrap()
        ))
    }
}
