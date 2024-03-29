pub mod db;

use std::{collections::HashSet, path::PathBuf};

use anyhow::Context as _;
use futures::TryStreamExt as _;

use crate::{config, fetch, nix};

const NAR_FILE_DIR: &str = "nar";

#[derive(Clone, Debug)]
pub struct Cache {
    pub db: db::Database,
}

impl Cache {
    #[tracing::instrument(name = "cache_init", skip(config))]
    pub async fn new(config: &config::Config) -> anyhow::Result<Self> {
        {
            tracing::trace!("Creating directory structure in data path");
            tokio::fs::create_dir_all(config.local_data_path.join(NAR_FILE_DIR)).await?;
        }

        let db = db::Database::new(config).await?;

        Ok(Self { db })
    }
}

#[tracing::instrument(skip_all)]
pub async fn write_nar_file(
    config: &config::Config,
    nar_file: &nix::NarFile,
) -> anyhow::Result<()> {
    use tokio::io::AsyncWriteExt as _;

    let file_path = nar_file_path_from_nar_file(config, &nar_file.info);

    tracing::debug!("Writing nar file to {}", file_path.display());

    tokio::fs::File::create(&file_path)
        .await
        .with_context(|| {
            format!(
                "Failed to create/open {} for writing nar file",
                file_path.display()
            )
        })?
        .write_all(&nar_file.data)
        .await
        .with_context(|| format!("Failed to write nar file to {}", file_path.display()))
}

#[tracing::instrument(skip_all)]
pub async fn missing_from_channel_upstreams(
    config: &config::Config,
    cache: &Cache,
) -> anyhow::Result<HashSet<nix::StorePath>> {
    let cached_store_paths = db::get_store_paths(cache.db.pool())
        .try_collect::<HashSet<_>>()
        .await
        .context("Failed to get cached store paths")?;

    let upstream_store_paths = fetch::request_all_channel_stores(config)
        .await
        .context("Failed to request up-to-date store paths from channel upstreams")?;

    tracing::debug!("Proccessing difference between local cache and upstream");
    Ok(upstream_store_paths
        .difference(&cached_store_paths)
        .map(Clone::clone)
        .collect())
}

pub fn nar_file_path(config: &config::Config, nar_info: &nix::NarInfo) -> PathBuf {
    nar_file_path_from_parts(config, &nar_info.file_hash, &nar_info.compression)
}

pub fn nar_file_path_from_nar_file(
    config: &config::Config,
    nar_file: &nix::NarFileInfo,
) -> PathBuf {
    nar_file_path_from_parts(config, &nar_file.hash, &nar_file.compression)
}

pub async fn disk_size(config: &config::Config) -> tokio::io::Result<u64> {
    tracing::debug!("Getting total cache disk size");
    folder_size(&config.local_data_path).await
}

pub async fn nar_disk_size(config: &config::Config) -> tokio::io::Result<u64> {
    tracing::debug!("Getting total cached nar file disk size");
    folder_size(&config.local_data_path.join(NAR_FILE_DIR)).await
}

#[async_recursion::async_recursion]
async fn folder_size(path: &std::path::Path) -> tokio::io::Result<u64> {
    use tokio::fs;

    let mut result = 0;

    if path.is_dir() {
        let mut read_dir = fs::read_dir(&path).await?;

        while let Some(entry) = read_dir.next_entry().await? {
            let p = entry.path();
            if p.is_file() {
                result += fs::metadata(p).await?.len();
            } else {
                result += folder_size(&p).await?;
            }
        }
    } else {
        result = fs::metadata(path).await?.len();
    }

    Ok(result)
}

fn nar_file_path_from_parts(
    config: &config::Config,
    file_hash: &nix::Hash,
    compression: &nix::CompressionType,
) -> PathBuf {
    config
        .local_data_path
        .join(NAR_FILE_DIR)
        .join(format!("{}.nar.{compression}", file_hash.string))
}
