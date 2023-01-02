use std::path::PathBuf;
use std::str::FromStr;

use anyhow::Context as _;
use futures::{StreamExt as _, TryStreamExt as _};

use crate::{config, nix};

pub const NAR_FILE_DIR: &str = "nar";
pub const CACHE_DB_PATH: &str = "cache.db";

#[derive(Clone, Debug)]
pub struct Cache {
    db_pool: sqlx::SqlitePool,
}

#[derive(Clone, Copy, Debug, num_enum::IntoPrimitive, num_enum::FromPrimitive)]
#[repr(i64)]
pub enum Status {
    #[num_enum(default)]
    NotAvailable,
    Fetching,
    OnlyInfo,
    Available,
    Purging,
}

// TODO: Check for way to see if DELETE/UPDATE is successful

impl Cache {
    #[tracing::instrument(name = "cache_init", skip(config))]
    pub async fn new(config: &config::Config) -> anyhow::Result<Cache> {
        {
            tracing::trace!("Creating directory structure in data path");
            tokio::fs::create_dir_all(config.local_data_path.join(NAR_FILE_DIR)).await?;
        }

        tracing::info!("Establishing connection to SQLite cache database");
        let cache = {
            use sqlx::sqlite::{
                SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous,
            };

            let database_url = format!(
                "sqlite://{}",
                config.local_data_path.join(CACHE_DB_PATH).display()
            );

            let connection_options = SqliteConnectOptions::from_str(&database_url)?
                .create_if_missing(true)
                .journal_mode(SqliteJournalMode::Wal)
                .synchronous(SqliteSynchronous::Normal);

            let db_pool = SqlitePoolOptions::new()
                .max_connections(config.database_max_connections)
                .connect_with(connection_options)
                .await?;

            tracing::info!("Migrating cache database");
            sqlx::query!("PRAGMA temp_store = MEMORY;")
                .execute(&db_pool)
                .await?;
            sqlx::migrate!().run(&db_pool).await?;

            Cache { db_pool }
        };

        Ok(cache)
    }

    pub async fn cleanup(self) {
        self.db_pool.close().await;
    }

    pub async fn begin_transaction(
        &self,
    ) -> Result<sqlx::Transaction<'static, sqlx::Sqlite>, sqlx::Error> {
        self.db_pool.begin().await
    }

    pub fn db_pool(&self) -> &sqlx::SqlitePool {
        &self.db_pool
    }
}

#[macro_export]
macro_rules! transaction {
    (begin: $cache:expr) => {
        $cache.begin_transaction().await.map_err(|e| {
            tracing::error!("Failed to begin transaction: {e}");
            apalis::prelude::JobError::Unknown
        })
    };

    (commit: $tx:expr) => {
        $tx.commit().await.map_err(|e| {
            tracing::error!("Failed to commit transaction: {e}");
            apalis::prelude::JobError::Unknown
        })
    };

    (rollback: $tx:expr) => {
        $tx.rollback().await.map_err(|e| {
            tracing::error!("Failed to rollback transaction: {e}");
            apalis::prelude::JobError::Unknown
        })
    };
}

#[tracing::instrument]
pub async fn get_nar_info<'c, E>(
    executor: E,
    hash: &nix::Hash,
) -> anyhow::Result<Option<nix::NarInfo>>
where
    E: sqlx::Executor<'c, Database = sqlx::Sqlite>,
{
    tracing::info!("Getting {}.narinfo from cache database", hash.string);

    let entry = sqlx::query_as!(
        NarInfoEntry,
        "SELECT * FROM narinfo_fields_view WHERE hash = ?1;",
        hash.string
    )
    .fetch_optional(executor)
    .await?;

    if let Some(entry) = entry {
        tracing::debug!("Found narinfo entry in database");
        Ok(Some(nix::NarInfo::try_from(entry)?))
    } else {
        tracing::debug!(
            "Unable to find entry for {}.narinfo in database",
            hash.string
        );

        Ok(None)
    }
}

#[tracing::instrument]
pub async fn get_nar_info_with_upstream<'c, E>(
    executor: E,
    hash: &nix::Hash,
) -> anyhow::Result<Option<(nix::NarInfo, nix::Upstream)>>
where
    E: sqlx::Executor<'c, Database = sqlx::Sqlite>,
{
    tracing::info!(
        "Getting {}.narinfo and upstream from cache database",
        hash.string
    );

    let entry = sqlx::query_as!(
        NarInfoWithUpstreamEntry,
        "SELECT * FROM narinfo WHERE hash = ?1;",
        hash.string
    )
    .fetch_optional(executor)
    .await?;

    if let Some(entry) = entry {
        tracing::debug!("Found narinfo entry in database");
        let upstream = nix::Upstream::new(entry.upstream_url.parse()?);
        let nar_info = nix::NarInfo::try_from(entry)?;
        Ok(Some((nar_info, upstream)))
    } else {
        tracing::debug!(
            "Unable to find entry for {}.narinfo in database",
            hash.string
        );

        Ok(None)
    }
}

#[tracing::instrument(skip(config))]
pub async fn get_nar_file_path<'c, E>(
    executor: E,
    config: &config::Config,
    hash: &nix::Hash,
) -> anyhow::Result<Option<PathBuf>>
where
    E: sqlx::Executor<'c, Database = sqlx::Sqlite>,
{
    tracing::info!("Getting file hash of {}.narinfo", hash.string);

    let entry = sqlx::query!(
        "
        SELECT
            file_hash_method AS method,
            file_hash AS hash,
            compression
        FROM narinfo
        WHERE hash = ?1",
        hash.string
    )
    .fetch_optional(executor)
    .await?;

    if let Some(entry) = entry {
        tracing::debug!("Found file hash in database");

        let file_hash = nix::Hash::from_method_hash(entry.method, entry.hash);
        let compression = entry
            .compression
            .parse()
            .context("Failed to parse compression type from cache db")?;

        Ok(Some(nar_file_path_from_parts(
            config,
            &file_hash,
            &compression,
        )))
    } else {
        tracing::debug!(
            "Unable to find file hash for {}.narinfo in database",
            hash.string
        );

        Ok(None)
    }
}

#[tracing::instrument]
pub async fn insert_nar_info<'c, E>(
    executor: E,
    hash: &nix::Hash,
    nar_info: &nix::NarInfo,
    upstream: &nix::Upstream,
    force: bool,
) -> anyhow::Result<()>
where
    E: sqlx::Executor<'c, Database = sqlx::Sqlite>,
{
    let entry = NarInfoEntry::from_nar_info(hash, nar_info);
    let upstream_url = upstream.url().to_string();

    if force {
        tracing::info!(
            "Forcefully REPLACING {}.narinfo in cache database",
            hash.string
        );

        sqlx::query!(
            "REPLACE INTO narinfo VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            entry.hash,
            entry.store_path,
            entry.compression,
            entry.file_hash_method,
            entry.file_hash,
            entry.file_size,
            entry.nar_hash_method,
            entry.nar_hash,
            entry.nar_size,
            entry.deriver,
            entry.system,
            entry.refs,
            entry.signature,
            upstream_url,
        )
    } else {
        tracing::info!("Inserting {}.narinfo into cache database", hash.string);

        sqlx::query!(
            "INSERT INTO narinfo VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            entry.hash,
            entry.store_path,
            entry.compression,
            entry.file_hash_method,
            entry.file_hash,
            entry.file_size,
            entry.nar_hash_method,
            entry.nar_hash,
            entry.nar_size,
            entry.deriver,
            entry.system,
            entry.refs,
            entry.signature,
            upstream_url,
        )
    }
    .execute(executor)
    .await?;

    Ok(())
}

#[tracing::instrument]
pub async fn get_store_paths<'c, E, T>(executor: E) -> anyhow::Result<T>
where
    E: sqlx::Executor<'c, Database = sqlx::Sqlite>,
    T: Extend<nix::StorePath> + Default,
{
    tracing::info!("Getting all cached store paths");

    let status: i64 = Status::Available.into();
    Ok(sqlx::query_scalar!(
        "SELECT narinfo.store_path
         FROM cache
         INNER JOIN narinfo ON cache.hash = narinfo.hash
         WHERE cache.status = ?",
        status
    )
    .fetch(executor)
    .map(|path_opt| -> anyhow::Result<_> {
        match path_opt {
            Ok(path) => Ok(nix::StorePath::from_str(&path)?),
            Err(err) => Err(err.into()),
        }
    })
    .try_collect::<T>()
    .await?)
}

#[tracing::instrument]
pub async fn purge_nar_info<'c, E>(executor: E, hash: &nix::Hash) -> anyhow::Result<()>
where
    E: sqlx::Executor<'c, Database = sqlx::Sqlite>,
{
    tracing::info!("Deleting entry for {}.narinfo", hash.string);

    sqlx::query!("DELETE FROM cache WHERE hash = ?1", hash.string)
        .execute(executor)
        .await?;

    Ok(())
}

#[tracing::instrument(level = "debug")]
pub async fn status<'c, E>(executor: E, hash: &nix::Hash) -> anyhow::Result<Option<Status>>
where
    E: sqlx::Executor<'c, Database = sqlx::Sqlite>,
{
    tracing::debug!("Querying status of {}.narinfo", hash.string);

    Ok(
        sqlx::query_scalar!("SELECT status FROM cache WHERE hash = ?", hash.string)
            .fetch_optional(executor)
            .await?
            .map(Status::from),
    )
}

#[tracing::instrument(level = "debug")]
pub async fn insert_status<'c, E>(
    executor: E,
    hash: &nix::Hash,
    status: Status,
) -> anyhow::Result<()>
where
    E: sqlx::Executor<'c, Database = sqlx::Sqlite>,
{
    tracing::debug!(
        "Inserting new cache entry for {}.narinfo with status: {status:?}",
        hash.string
    );

    let status: i64 = status.into();
    sqlx::query!("INSERT INTO cache VALUES (?,?)", hash.string, status)
        .execute(executor)
        .await?;

    Ok(())
}

#[tracing::instrument(level = "debug")]
pub async fn update_status<'c, E>(
    executor: E,
    hash: &nix::Hash,
    status: Status,
) -> anyhow::Result<()>
where
    E: sqlx::Executor<'c, Database = sqlx::Sqlite>,
{
    tracing::debug!("Updating status of {}.narinfo to {status:?}", hash.string);

    let status: i64 = status.into();
    sqlx::query!(
        "UPDATE cache SET status = ? WHERE hash = ?",
        status,
        hash.string
    )
    .execute(executor)
    .await?;

    Ok(())
}

pub async fn reported_size<'c, E>(executor: E) -> anyhow::Result<u64>
where
    E: sqlx::Executor<'c, Database = sqlx::Sqlite>,
{
    Ok(sqlx::query_scalar!("SELECT SUM(file_size) FROM narinfo")
        .fetch_one(executor)
        .await?
        .unwrap_or_default() as u64)
}

pub async fn is_cached_by_hash<'c, E>(executor: E, hash: &nix::Hash) -> anyhow::Result<bool>
where
    E: sqlx::Executor<'c, Database = sqlx::Sqlite>,
{
    Ok(
        sqlx::query_scalar!("SELECT status FROM cache WHERE hash = ?", hash.string)
            .fetch_optional(executor)
            .await?
            .map(|status| matches!(Status::from(status), Status::Available))
            .unwrap_or_default(),
    )
}

// HACK: Added `Copy` trait by bypass moved value error, but disallows the use of `&mut _`
pub async fn is_nar_file_cached<'c, E>(executor: E, nar_file: &nix::NarFile) -> anyhow::Result<bool>
where
    E: sqlx::Executor<'c, Database = sqlx::Sqlite> + Copy,
{
    let compression = nar_file.compression.to_string();
    let hash = match sqlx::query_scalar!(
        r#"SELECT hash FROM narinfo WHERE file_hash = ? AND compression = ?"#,
        nar_file.hash.string,
        compression
    )
    .fetch_optional(executor)
    .await?
    {
        Some(hash) => hash,
        None => return Ok(false),
    };

    is_cached_by_hash(executor, &nix::Hash::from_hash(hash)).await
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

pub fn nar_file_path(config: &config::Config, nar_info: &nix::NarInfo) -> PathBuf {
    nar_file_path_from_parts(config, &nar_info.file_hash, &nar_info.compression)
}

pub fn nar_file_path_from_nar_file(config: &config::Config, nar_file: &nix::NarFile) -> PathBuf {
    nar_file_path_from_parts(config, &nar_file.hash, &nar_file.compression)
}

fn nar_file_path_from_parts(
    config: &config::Config,
    file_hash: &nix::Hash,
    compression: &nix::CompressionType,
) -> PathBuf {
    config
        .local_data_path
        .join(NAR_FILE_DIR)
        .join(format!("{}.nar.{}", file_hash.string, compression))
}

#[allow(dead_code)]
#[derive(Debug, sqlx::FromRow)]
struct NarInfoEntry {
    hash: String,
    store_path: String,
    compression: String,
    file_hash_method: String,
    file_hash: String,
    file_size: i64,
    nar_hash_method: String,
    nar_hash: String,
    nar_size: i64,
    deriver: Option<String>,
    system: Option<String>,
    refs: String,
    signature: Option<String>,
}

impl NarInfoEntry {
    fn from_nar_info(hash: &nix::Hash, nar_info: &nix::NarInfo) -> Self {
        Self {
            hash: hash.string.clone(),
            store_path: nar_info.store_path.path().to_string_lossy().to_string(),
            compression: nar_info.compression.to_string(),
            file_hash_method: nar_info
                .file_hash
                .method
                .clone()
                .unwrap_or_default()
                .to_string(),
            file_hash: nar_info.file_hash.string.clone(),
            file_size: nar_info.file_size as i64,
            nar_hash_method: nar_info
                .file_hash
                .method
                .clone()
                .unwrap_or_default()
                .to_string(),
            nar_hash: nar_info.nar_hash.string.clone(),
            nar_size: nar_info.nar_size as i64,
            deriver: nar_info.deriver.clone(),
            system: nar_info.system.clone(),
            refs: nar_info
                .references
                .iter()
                .map(nix::Derivation::to_string)
                .fold(String::new(), |a, v| a + " " + &v),
            signature: nar_info.signature.clone(),
        }
    }
}

impl TryFrom<NarInfoEntry> for nix::NarInfo {
    type Error = <nix::NarInfo as FromStr>::Err;

    fn try_from(value: NarInfoEntry) -> Result<Self, Self::Error> {
        use nix::{CompressionType, Derivation, Hash, StorePath};

        let file_hash = Hash::from_method_hash(value.file_hash_method, value.file_hash);
        let compression = value
            .compression
            .parse::<CompressionType>()
            .map_err(|e| Self::Error::InvalidFieldValue("Compression".to_owned(), e.to_string()))?;
        let url = format!("nar/{}.nar.{}", file_hash.string, compression);

        nix::NarInfoBuilder::default()
            .store_path(value.store_path.parse::<StorePath>().map_err(|e| {
                Self::Error::InvalidFieldValue("StorePath".to_owned(), e.to_string())
            })?)
            .url(url)
            .compression(compression)
            .file_hash(file_hash)
            .file_size(value.file_size as usize)
            .nar_hash(Hash::from_method_hash(
                value.nar_hash_method,
                value.nar_hash,
            ))
            .nar_size(value.nar_size as usize)
            .deriver(value.deriver.clone())
            .system(value.system.clone())
            .references(
                value
                    .refs
                    .split_whitespace()
                    .map(Derivation::from_str)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(Self::Error::InvalidReference)?,
            )
            .signature(value.signature.clone())
            .build()
            .map_err(Self::Error::MissingField)
    }
}

#[allow(dead_code)]
#[derive(Debug, sqlx::FromRow)]
struct NarInfoWithUpstreamEntry {
    hash: String,
    store_path: String,
    compression: String,
    file_hash_method: String,
    file_hash: String,
    file_size: i64,
    nar_hash_method: String,
    nar_hash: String,
    nar_size: i64,
    deriver: Option<String>,
    system: Option<String>,
    refs: String,
    signature: Option<String>,
    upstream_url: String,
}

impl From<NarInfoWithUpstreamEntry> for NarInfoEntry {
    fn from(
        NarInfoWithUpstreamEntry {
            hash,
            store_path,
            compression,
            file_hash_method,
            file_hash,
            file_size,
            nar_hash_method,
            nar_hash,
            nar_size,
            deriver,
            system,
            refs,
            signature,
            ..
        }: NarInfoWithUpstreamEntry,
    ) -> Self {
        NarInfoEntry {
            hash,
            store_path,
            compression,
            file_hash_method,
            file_hash,
            file_size,
            nar_hash_method,
            nar_hash,
            nar_size,
            deriver,
            system,
            refs,
            signature,
        }
    }
}

impl TryFrom<NarInfoWithUpstreamEntry> for nix::NarInfo {
    type Error = <nix::NarInfo as FromStr>::Err;

    fn try_from(value: NarInfoWithUpstreamEntry) -> Result<Self, Self::Error> {
        NarInfoEntry::from(value).try_into()
    }
}
