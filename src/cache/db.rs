use std::{path::PathBuf, str::FromStr};

use anyhow::Context as _;
use futures::StreamExt as _;

use crate::{cache, config, nix};

const CACHE_DB_FILE: &str = "cache.db";

#[derive(Clone, Debug)]
pub(super) struct Database(sqlx::SqlitePool);

#[derive(Debug, sqlx::FromRow)]
pub struct Entry {
    status: Status,
    last_cached: chrono::NaiveDateTime,
    last_accessed: Option<chrono::NaiveDateTime>,
}

#[derive(
    Clone, Copy, Debug, Default, num_enum::IntoPrimitive, num_enum::FromPrimitive, sqlx::Encode,
)]
#[repr(i64)]
pub enum Status {
    #[default]
    NotAvailable,
    Fetching,
    OnlyInfo,
    Available,
    Purging,
}

impl<DB> sqlx::Type<DB> for Status
where
    DB: sqlx::Database,
    i64: sqlx::Type<DB>,
{
    fn type_info() -> <DB as sqlx::Database>::TypeInfo {
        <i64 as sqlx::Type<DB>>::type_info()
    }

    fn compatible(ty: &<DB as sqlx::Database>::TypeInfo) -> bool {
        <i64 as sqlx::Type<DB>>::compatible(ty)
    }
}

impl<'r, DB> sqlx::Decode<'r, DB> for Status
where
    DB: sqlx::Database,
    i64: sqlx::Decode<'r, DB>,
{
    fn decode(
        value: <DB as sqlx::database::HasValueRef<'r>>::ValueRef,
    ) -> Result<Self, sqlx::error::BoxDynError> {
        let value = <i64 as sqlx::Decode<DB>>::decode(value)?;
        Ok(Status::from(value))
    }
}

// TODO: Check for way to see if DELETE/UPDATE is successful

impl Database {
    #[tracing::instrument(name = "cache_db_init", skip(config))]
    pub(super) async fn new(config: &config::Config) -> anyhow::Result<Self> {
        use sqlx::sqlite::{
            SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous,
        };

        tracing::info!("Establishing connection to SQLite cache database");

        let database_url = format!(
            "sqlite://{}",
            config.local_data_path.join(CACHE_DB_FILE).display()
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
        sqlx::query!(r#"PRAGMA temp_store = MEMORY;"#)
            .execute(&db_pool)
            .await?;
        sqlx::migrate!().run(&db_pool).await?;

        Ok(Self(db_pool))
    }

    pub(super) async fn cleanup(self) {
        self.0.close().await;
    }

    pub(super) async fn transaction(
        &self,
    ) -> sqlx::Result<sqlx::Transaction<'static, sqlx::Sqlite>> {
        self.0.begin().await
    }

    pub(super) fn pool(&self) -> &sqlx::SqlitePool {
        &self.0
    }
}

#[macro_export]
macro_rules! transaction {
    (begin: $cache:expr) => {
        $cache
            .db_transaction()
            .await
            .context("Failed to begin transaction")
    };

    (commit: $tx:expr) => {
        $tx.commit().await.context("Failed to commit transaction")
    };

    (rollback: $tx:expr) => {
        $tx.rollback()
            .await
            .context("Failed to rollback transaction")
    };
}

#[tracing::instrument]
pub async fn get_nar_info<'c, E>(
    executor: E,
    hash: &nix::Hash,
) -> anyhow::Result<Option<nix::NarInfo>>
where
    E: sqlx::SqliteExecutor<'c>,
{
    tracing::info!("Getting {}.narinfo from cache database", hash.string);

    let entry = sqlx::query_as!(
        NarInfoEntry,
        r#"
            SELECT
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
                signature
            FROM narinfo
            WHERE hash = ?
        "#,
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
    E: sqlx::SqliteExecutor<'c>,
{
    tracing::info!(
        "Getting {}.narinfo and upstream from cache database",
        hash.string
    );

    let entry: Option<NarInfoWithUpstreamEntry> = sqlx::query_as(
        r#"
            SELECT *
            FROM narinfo
            WHERE hash = ?
        "#,
    )
    .bind(&hash.string)
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
    E: sqlx::SqliteExecutor<'c>,
{
    tracing::info!("Getting file hash of {}.narinfo", hash.string);

    let entry = sqlx::query!(
        r#"
            SELECT
                file_hash_method AS method,
                file_hash AS hash,
                compression
            FROM narinfo
            WHERE hash = ?
        "#,
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

        Ok(Some(cache::nar_file_path_from_parts(
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
    E: sqlx::SqliteExecutor<'c>,
{
    let entry = NarInfoEntry::from_nar_info(hash, nar_info);
    let upstream_url = upstream.url().to_string();

    if force {
        tracing::info!(
            "Forcefully REPLACING {}.narinfo in cache database",
            hash.string
        );

        sqlx::query!(
            r#"
                REPLACE INTO narinfo
                VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)
            "#,
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
            r#"
                INSERT INTO narinfo
                VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)
            "#,
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
pub fn get_store_paths<'c, E>(
    executor: E,
) -> futures::stream::BoxStream<'c, anyhow::Result<nix::StorePath>>
where
    E: sqlx::SqliteExecutor<'c> + 'c,
{
    tracing::info!("Getting all cached store paths");

    Box::pin(
        sqlx::query_scalar!(
            r#"
                SELECT narinfo.store_path
                FROM cache
                INNER JOIN narinfo ON cache.hash = narinfo.hash
                WHERE cache.status = ?
            "#,
            Status::Available
        )
        .fetch(executor)
        .map(|path_opt| -> anyhow::Result<_> {
            match path_opt {
                Ok(path) => Ok(nix::StorePath::from_str(&path)?),
                Err(err) => Err(err.into()),
            }
        }),
    )
}

#[tracing::instrument]
pub async fn purge_nar_info<'c, E>(executor: E, hash: &nix::Hash) -> anyhow::Result<()>
where
    E: sqlx::SqliteExecutor<'c>,
{
    tracing::info!("Deleting entry for {}.narinfo", hash.string);

    sqlx::query!(
        r#"
            DELETE FROM cache
            WHERE hash = ?
        "#,
        hash.string
    )
    .execute(executor)
    .await?;

    Ok(())
}

#[tracing::instrument(level = "debug")]
pub async fn get_entry<'c, E>(executor: E, hash: &nix::Hash) -> anyhow::Result<Option<Entry>>
where
    E: sqlx::SqliteExecutor<'c>,
{
    tracing::debug!("Querying entry details of {}.narinfo", hash.string);

    Ok(sqlx::query_as!(
        Entry,
        r#"
            SELECT
                status as "status: Status",
                last_cached,
                last_accessed
            FROM cache
            WHERE hash = ?
        "#,
        hash.string
    )
    .fetch_optional(executor)
    .await?)
}

#[tracing::instrument(level = "debug")]
pub async fn get_status<'c, E>(executor: E, hash: &nix::Hash) -> anyhow::Result<Option<Status>>
where
    E: sqlx::SqliteExecutor<'c>,
{
    tracing::debug!("Querying status of {}.narinfo", hash.string);

    Ok(sqlx::query_scalar!(
        r#"
            SELECT status as "status: Status"
            FROM cache
            WHERE hash = ?
        "#,
        hash.string
    )
    .fetch_optional(executor)
    .await?)
}

#[tracing::instrument(level = "debug")]
pub async fn insert_status<'c, E>(
    executor: E,
    hash: &nix::Hash,
    status: Status,
) -> anyhow::Result<()>
where
    E: sqlx::SqliteExecutor<'c>,
{
    tracing::debug!(
        "Inserting new cache entry for {}.narinfo with status: {status:?}",
        hash.string
    );

    sqlx::query!(
        r#"
            INSERT INTO cache (hash, status)
            VALUES (?,?)
        "#,
        hash.string,
        status
    )
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
    E: sqlx::SqliteExecutor<'c>,
{
    tracing::debug!("Updating status of {}.narinfo to {status:?}", hash.string);

    sqlx::query!(
        r#"
            UPDATE cache
            SET status = ?
            WHERE hash = ?
        "#,
        status,
        hash.string
    )
    .execute(executor)
    .await?;

    Ok(())
}

#[tracing::instrument(level = "debug")]
pub async fn get_reported_nar_size<'c, E>(executor: E) -> anyhow::Result<u64>
where
    E: sqlx::SqliteExecutor<'c>,
{
    tracing::debug!("Getting reported size of cached nar files");

    Ok(sqlx::query_scalar!(
        r#"
            SELECT SUM(file_size) FROM narinfo
        "#
    )
    .fetch_one(executor)
    .await?
    .unwrap_or_default() as u64)
}

#[tracing::instrument(level = "debug")]
pub async fn is_cached_by_hash<'c, E>(executor: E, hash: &nix::Hash) -> anyhow::Result<bool>
where
    E: sqlx::SqliteExecutor<'c>,
{
    Ok(sqlx::query_scalar!(
        r#"
            SELECT 1
            FROM cache
            WHERE hash = ? AND status = ?
        "#,
        hash.string,
        Status::Available
    )
    .fetch_optional(executor)
    .await?
    .is_some())
}

// HACK: Added `Copy` trait by bypass moved value error, but disallows the use of `&mut _`
pub async fn is_nar_file_cached<'c, E>(executor: E, nar_file: &nix::NarFile) -> anyhow::Result<bool>
where
    E: sqlx::SqliteExecutor<'c> + Copy,
{
    let compression = nar_file.compression.to_string();
    let hash = match sqlx::query_scalar!(
        r#"
            SELECT hash
            FROM narinfo
            WHERE file_hash = ? AND compression = ?
        "#,
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
        let url = format!("nar/{}.nar.{compression}", file_hash.string);

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
    #[sqlx(flatten)]
    nar_info_entry: NarInfoEntry,
    upstream_url: String,
}

impl From<NarInfoWithUpstreamEntry> for NarInfoEntry {
    fn from(NarInfoWithUpstreamEntry { nar_info_entry, .. }: NarInfoWithUpstreamEntry) -> Self {
        nar_info_entry
    }
}

impl TryFrom<NarInfoWithUpstreamEntry> for nix::NarInfo {
    type Error = <nix::NarInfo as FromStr>::Err;

    fn try_from(value: NarInfoWithUpstreamEntry) -> Result<Self, Self::Error> {
        NarInfoEntry::from(value).try_into()
    }
}
