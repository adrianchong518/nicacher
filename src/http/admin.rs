use anyhow::Context as _;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use futures::{FutureExt as _, StreamExt as _, TryStreamExt as _};
use serde::Deserialize;

use crate::{app, cache, http, jobs, nix, transaction};

pub(super) fn router() -> axum::Router<app::State> {
    use axum::routing::get;

    let push_job = axum::Router::new()
        .route("/cache_nar/:hash", get(push_cache_nar))
        .route("/purge_nar/:hash", get(push_purge_nar));

    axum::Router::new()
        .route("/cache_size", get(cache_size))
        .route("/list_cached", get(list_cached))
        .route("/list_cache_diff", get(list_cache_diff))
        .route("/nar_status/:hash", get(nar_status))
        .route("/nar_entry/:hash", get(nar_entry))
        .route("/cache_nar/:hash", get(cache_nar))
        .route("/purge_nar/:hash", get(purge_nar))
        .nest("/push", push_job)
}

async fn nar_entry(
    Path(hash): Path<nix::Hash>,
    State(app::State { cache, .. }): State<app::State>,
) -> http::Result<impl IntoResponse> {
    Ok(format!(
        "{:#?}",
        cache::db::get_entry(cache.db.pool(), &hash).await?
    ))
}

async fn nar_status(
    Path(hash): Path<nix::Hash>,
    State(app::State { cache, .. }): State<app::State>,
) -> http::Result<impl IntoResponse> {
    Ok(format!(
        "{:#?}",
        cache::db::get_status(cache.db.pool(), &hash).await?
    ))
}

async fn cache_size(
    State(app::State { config, cache, .. }): State<app::State>,
) -> http::Result<impl IntoResponse> {
    let disk_size = cache::disk_size(&config)
        .await
        .context("Failed to get total cache disk size")?;

    let nar_disk_size = cache::nar_disk_size(&config)
        .await
        .context("Failed to get total cached nar file disk size")?;

    let reported_size = cache::db::get_reported_total_nar_size(cache.db.pool())
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
    State(app::State { config, cache, .. }): State<app::State>,
) -> http::Result<impl IntoResponse> {
    let res = jobs::cache_nar(&config, &cache, hash, is_force).await?;
    Ok(format!("{res:#?}"))
}

async fn push_cache_nar(
    Path(hash): Path<nix::Hash>,
    Query(IsForce { is_force }): Query<IsForce>,
    State(app::State { mut workers, .. }): State<app::State>,
) -> http::Result<impl IntoResponse> {
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
    State(app::State { config, cache, .. }): State<app::State>,
) -> http::Result<impl IntoResponse> {
    let res = jobs::purge_nar(&config, &cache, hash, is_force).await?;
    Ok(format!("{res:#?}"))
}

async fn push_purge_nar(
    Path(hash): Path<nix::Hash>,
    Query(IsForce { is_force }): Query<IsForce>,
    State(app::State { mut workers, .. }): State<app::State>,
) -> http::Result<impl IntoResponse> {
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
) -> http::Result<impl IntoResponse> {
    let (num_cached, cached_store_paths) = {
        let mut tx = transaction!(begin: cache)?;

        let num_cached = cache::db::get_num_store_paths(&mut tx).await?;

        let cached_store_paths = cache::db::get_store_paths(&mut tx)
            .map_ok(|p| nix::StorePath::to_string(&p))
            .take(limit)
            .try_fold(
                String::new(),
                |acc, path| async move { Ok(acc + &path + "\n") },
            )
            .await
            .context("Failed to get cached store paths")?;

        transaction!(commit: tx)?;

        (num_cached, cached_store_paths)
    };

    if num_cached == 0 {
        Ok("No (0) derivations cached".into_response())
    } else {
        Ok(format!(
            "\
Number derivations cached: {num_cached}
Store paths of cached derivations: (limit: {limit})

{}",
            cached_store_paths
        )
        .into_response())
    }
}

async fn list_cache_diff(
    Query(ListLimit { limit }): Query<ListLimit>,
    State(app::State { config, cache, .. }): State<app::State>,
) -> http::Result<impl IntoResponse> {
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
