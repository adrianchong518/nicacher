use std::str::FromStr;

use crate::{config, nix};

pub const NAR_FILE_DIR: &str = "nar";
pub const CACHE_DB_PATH: &str = "cache.db";

#[derive(Clone, Debug)]
pub struct CacheDatabasePool(sqlx::SqlitePool);

impl CacheDatabasePool {
    pub async fn new(config: &config::Config) -> anyhow::Result<CacheDatabasePool> {
        tracing::trace!("Creating directory structure in data path");
        tokio::fs::create_dir_all(config.local_data_path.join(NAR_FILE_DIR)).await?;

        tracing::info!("Establishing connection to SQLite cache database");
        let cache_db_pool = {
            use sqlx::sqlite::{
                SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous,
            };
            use std::str::FromStr as _;

            let database_url = format!(
                "sqlite://{}",
                config.local_data_path.join(CACHE_DB_PATH).display()
            );

            let connection_options = SqliteConnectOptions::from_str(&database_url)?
                .create_if_missing(true)
                .journal_mode(SqliteJournalMode::Wal)
                .synchronous(SqliteSynchronous::Normal);

            let cache_db_pool = SqlitePoolOptions::new()
                .max_connections(config.database_max_connections)
                .connect_with(connection_options)
                .await?;

            tracing::debug!("Migrating cache database");
            sqlx::query("PRAGMA temp_store = MEMORY;")
                .execute(&cache_db_pool)
                .await?;
            sqlx::migrate!().run(&cache_db_pool).await?;

            CacheDatabasePool(cache_db_pool)
        };

        Ok(cache_db_pool)
    }

    #[tracing::instrument]
    pub async fn get_nar_info(&self, hash: &nix::Hash) -> anyhow::Result<Option<nix::NarInfo>> {
        tracing::info!("Getting {}.narinfo from cache database", hash.string);

        let entry: Option<NarInfoEntry> = sqlx::query_as("SELECT * FROM narinfo WHERE hash = ?1")
            .bind(&hash.string)
            .fetch_optional(&self.0)
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
    pub async fn insert_nar_info(
        &self,
        hash: &nix::Hash,
        nar_info: &nix::NarInfo,
        force: bool,
    ) -> anyhow::Result<()> {
        let entry = NarInfoEntry::from_nar_info(hash, nar_info);

        if force {
            tracing::info!(
                "Forcefully REPLACING {}.narinfo in cache database",
                hash.string
            );

            sqlx::query("REPLACE INTO narinfo VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)")
                .bind(entry.hash)
                .bind(entry.store_path)
                .bind(entry.compression)
                .bind(entry.file_hash_method)
                .bind(entry.file_hash)
                .bind(entry.file_size)
                .bind(entry.nar_hash_method)
                .bind(entry.nar_hash)
                .bind(entry.nar_size)
                .bind(entry.deriver)
                .bind(entry.system)
                .bind(entry.refs)
                .bind(entry.signature)
        } else {
            tracing::info!("Inserting {}.narinfo into cache database", hash.string);

            sqlx::query("INSERT INTO narinfo VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)")
                .bind(entry.hash)
                .bind(entry.store_path)
                .bind(entry.compression)
                .bind(entry.file_hash_method)
                .bind(entry.file_hash)
                .bind(entry.file_size)
                .bind(entry.nar_hash_method)
                .bind(entry.nar_hash)
                .bind(entry.nar_size)
                .bind(entry.deriver)
                .bind(entry.system)
                .bind(entry.refs)
                .bind(entry.signature)
        }
        .execute(&self.0)
        .await?;

        Ok(())
    }

    pub async fn is_nar_info_cached(&self, hash: &nix::Hash) -> anyhow::Result<bool> {
        Ok(
            sqlx::query_scalar::<_, i64>("SELECT 1 FROM narinfo WHERE hash = ?")
                .bind(&hash.string)
                .fetch_optional(&self.0)
                .await?
                .is_some(),
        )
    }

    pub async fn is_nar_file_cached(&self, nar_file: &nix::NarFile) -> anyhow::Result<bool> {
        Ok(sqlx::query_scalar::<_, i64>(
            "SELECT 1 FROM narinfo WHERE file_hash = ? AND compression = ?",
        )
        .bind(&nar_file.hash.string)
        .bind(nar_file.compression.to_string())
        .fetch_optional(&self.0)
        .await?
        .is_some())
    }
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
            hash: hash.to_string(),
            store_path: nar_info.store_path.path.to_string_lossy().to_string(),
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

        let file_hash = format!("{}:{}", value.file_hash_method, value.file_hash)
            .parse::<Hash>()
            .map_err(|e| Self::Error::InvalidFieldValue("FileHash".to_owned(), e.to_string()))?;
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
            .nar_hash(
                format!("{}:{}", value.nar_hash_method, value.nar_hash)
                    .parse::<Hash>()
                    .map_err(|e| {
                        Self::Error::InvalidFieldValue("NarHash".to_owned(), e.to_string())
                    })?,
            )
            .nar_size(value.nar_size as usize)
            .deriver(value.deriver)
            .system(value.system)
            .references(
                value
                    .refs
                    .split_whitespace()
                    .map(Derivation::from_str)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(Self::Error::InvalidReference)?,
            )
            .signature(value.signature)
            .build()
            .map_err(Self::Error::MissingField)
    }
}
