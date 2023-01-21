use anyhow::Context as _;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use futures::TryStreamExt as _;
use serde::Deserialize;

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
    format!("{:#?}", cache::db::get_status(cache.db_pool(), &hash).await)
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

    let reported_size = cache::db::get_reported_nar_size(cache.db_pool())
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
    let is_cached = cache::db::is_cached_by_hash(cache.db_pool(), &hash)
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
    let cached_store_paths = cache::db::get_store_paths(cache.db_pool())
        .map_ok(|p| nix::StorePath::to_string(&p))
        .try_fold(
            String::new(),
            |acc, path| async move { Ok(acc + "\n" + &path) },
        )
        .await
        .context("Failed to get cached store paths")?;

    Ok(format!(
        "\
Store paths of cached derivations: (limit: {limit})

{}",
        cached_store_paths
    ))
}

async fn list_cache_diff(
    Query(ListLimit { limit }): Query<ListLimit>,
    State(app::State { config, cache, .. }): State<app::State>,
) -> error::Result<impl IntoResponse> {
    let diff = cache::missing_from_channel_upstreams(&config, &cache).await?;
    let diff_len = diff.len();

    if diff_len == 0 {
        Ok("No missing derivations from cache".to_string())
    } else {
        Ok(format!(
            "\
Number of missing derivations from cache: {diff_len}
----------------------------------------------------
Store paths of missing derivations: (limit: {limit})

{}",
            diff.iter()
                .take(limit)
                .map(nix::StorePath::to_string)
                .reduce(|acc, path| acc + "\n" + &path)
                .unwrap()
        ))
    }
}
