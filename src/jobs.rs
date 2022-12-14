use std::fmt;
use std::time::Duration;

use apalis::prelude::Job as ApalisJob;
use apalis::prelude::*;
use serde::{Deserialize, Serialize};

use anyhow::Context as _;
use tracing::Instrument as _;

use crate::{app, cache, config, fetch, nix, transaction};

#[derive(Clone, Debug)]
pub struct Workers {
    storage: apalis::sqlite::SqliteStorage<Job>,
}

impl Workers {
    #[tracing::instrument(name = "workers_init", skip_all)]
    pub async fn new() -> anyhow::Result<Self> {
        let storage = apalis::sqlite::SqliteStorage::connect("sqlite::memory:")
            .await
            .context("Unable to connect to in-memory sqlite database")?;
        storage
            .setup()
            .await
            .context("Unable to migrate sqlite database")?;

        Ok(Self { storage })
    }

    pub async fn run(
        self,
        state: app::State,
        server_shutdown_tx: tokio::sync::oneshot::Sender<()>,
    ) -> anyhow::Result<()> {
        use apalis::layers::{Extension, TraceLayer};

        fn custom_make_span<T>(req: &JobRequest<T>) -> tracing::Span
        where
            T: fmt::Debug,
        {
            tracing::span!(
                parent: tracing::Span::current(),
                tracing::Level::DEBUG,
                "job",
                job = format!("{:?}", req.inner()),
                job_id = req.id().as_str(),
                current_attempt = req.attempts(),
            )
        }

        // let cron_worker = {
        //     use std::str::FromStr as _;
        //
        //     use apalis::cron::{CronWorker, Schedule};
        //     use tower::ServiceBuilder;
        //
        //     CronWorker::new(
        //         Schedule::from_str("*/5 * * * * *").unwrap(),
        //         ServiceBuilder::new()
        //             .layer(TraceLayer::new().make_span_with(custom_make_span))
        //             .service(job_fn(periodic_job)),
        //     )
        // };

        let monitor = Monitor::new().register_with_count(4, move |_| {
            WorkerBuilder::new(self.storage())
                .layer(TraceLayer::new().make_span_with(custom_make_span))
                .layer(Extension(state.clone()))
                .build_fn(dispatch_jobs)
        });
        // .register(cron_worker)

        tracing::info!("Starting workers");

        monitor.run().await?;

        tracing::info!("Workers stopped");

        Ok(())
    }

    pub fn storage(&self) -> apalis::sqlite::SqliteStorage<Job> {
        self.storage.clone()
    }

    pub async fn push_job(&mut self, job: Job) -> apalis_core::storage::StorageResult<()> {
        self.storage.push(job).await
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Job {
    CacheNar { hash: nix::Hash, is_force: bool },
    PurgeNar { hash: nix::Hash, is_force: bool },
}

impl ApalisJob for Job {
    const NAME: &'static str = "nicacher::jobs::Job";
}

macro_rules! extract_state {
    ({ $($var:ident),+ $(,)? } <- $ctx:expr) => {
        let $crate::app::State { $($var),+, .. } = $ctx.data_opt::<$crate::app::State>().unwrap();
    };
}

async fn dispatch_jobs(job: Job, ctx: JobContext) -> Result<JobResult, JobError> {
    extract_state!({ config, cache } <- ctx);

    match job {
        Job::CacheNar { hash, is_force } => cache_nar(config, cache, hash, is_force).await,
        Job::PurgeNar { hash, is_force } => purge_nar(config, cache, hash, is_force).await,
    }
}

#[tracing::instrument(skip(config, cache))]
async fn cache_nar(
    config: &config::Config,
    cache: &cache::Cache,
    hash: nix::Hash,
    is_force: bool,
) -> Result<JobResult, JobError> {
    tracing::info!("Caching {} narinfo and corresponding nar file", hash.string);

    let ret = async {
        let mut tx = transaction!(begin: cache).map_err(Err)?;

        let is_info_available = match cache::status(&mut tx, &hash).await.map_err(|e| {
            tracing::error!("Failed to check cache status: {e:#}");
            Err(JobError::Unknown)
        })? {
            Some(cache::Status::Fetching) => {
                tracing::warn!("Already fetching by other worker, killing");
                return Err(Ok(JobResult::Kill));
            }
            Some(cache::Status::Available) if !is_force => {
                tracing::warn!("Already cached, killing");
                return Err(Ok(JobResult::Kill));
            }
            Some(cache::Status::Purging) if is_force => {
                tracing::warn!("Purging by other worker, rescheduling due to `is_force`");
                return Err(Ok(JobResult::Reschedule(Duration::from_secs(10))));
            }
            Some(cache::Status::Purging) if !is_force => {
                tracing::warn!("Purging by other worker, killing");
                return Err(Ok(JobResult::Kill));
            }
            Some(cache::Status::OnlyInfo) if !is_force => {
                tracing::warn!("Narinfo already cached");
                true
            }
            None => {
                cache::insert_status(&mut tx, &hash, cache::Status::Fetching)
                    .await
                    .map_err(|e| {
                        tracing::error!("Failed to insert new cache status to `Fetching`: {e:#}");
                        Err(JobError::Unknown)
                    })?;
                false
            }
            _ => {
                cache::update_status(&mut tx, &hash, cache::Status::Fetching)
                    .await
                    .map_err(|e| {
                        tracing::error!("Failed to update cache status to `Fetching`: {e:#}");
                        Err(JobError::Unknown)
                    })?;
                false
            }
        };

        transaction!(commit: tx).map_err(Err)?;

        Ok::<_, Result<JobResult, JobError>>(is_info_available)
    }
    .instrument(tracing::debug_span!("cache_nar_init"))
    .await;

    let is_info_available = match ret {
        Ok(v) => v,
        Err(ret) => return ret,
    };

    let (nar_info, upstream) = if is_info_available {
        tracing::debug!("Skipping request from upstream, querying from local db");
        match cache::get_nar_info_with_upstream(cache.db_pool(), &hash)
            .await
            .map_err(|e| {
                tracing::error!("Error when querying narinfo from local db: {e:#}");
                JobError::Unknown
            })? {
            Some(v) => v,
            None => {
                // HACK: There should be a better way of handling this
                tracing::warn!("Race condition, narinfo became unavaliable, retrying");
                return Ok(JobResult::Retry);
            }
        }
    } else {
        let (nar_info, upstream) = fetch::request_nar_info(config, &hash).await.map_err(|e| {
            tracing::error!("Error when requesting narinfo: {e:#}");
            JobError::Unknown
        })?;

        async {
            let mut tx = transaction!(begin: cache)?;

            cache::insert_nar_info(&mut tx, &hash, &nar_info, &upstream, is_force)
                .await
                .map_err(|e| {
                    tracing::error!("Error when inserting narinfo into cache database: {e:#}",);
                    JobError::Unknown
                })?;

            cache::update_status(&mut tx, &hash, cache::Status::OnlyInfo)
                .await
                .map_err(|e| {
                    tracing::error!("Failed to update cache status to `OnlyInfo`: {e:#}");
                    JobError::Unknown
                })?;

            transaction!(commit: tx)?;

            Ok::<_, JobError>(())
        }
        .instrument(tracing::debug_span!("cache_nar_insert_info"))
        .await?;

        (nar_info, upstream)
    };

    fetch::download_nar_file(config, &upstream, &nar_info)
        .await
        .map_err(|e| {
            tracing::error!("Error when downloading nar file: {e:#}");
            JobError::Unknown
        })?;

    cache::update_status(cache.db_pool(), &hash, cache::Status::Available)
        .await
        .map_err(|e| {
            tracing::error!("Failed to update cache status to `Available`: {e:#}");
            JobError::Unknown
        })?;

    Ok(JobResult::Success)
}

#[tracing::instrument(skip(config, cache))]
async fn purge_nar(
    config: &config::Config,
    cache: &cache::Cache,
    hash: nix::Hash,
    is_force: bool,
) -> Result<JobResult, JobError> {
    tracing::info!("Purging {} narinfo and corresponding nar file", hash.string);

    let ret = async {
        let mut tx = transaction!(begin: cache).map_err(Err)?;

        let is_nar_file_cached = match cache::status(&mut tx, &hash).await.map_err(|e| {
            tracing::error!("Failed to check cache status: {e:#}");
            Err(JobError::Unknown)
        })? {
            None => {
                tracing::warn!("Not cached, killing");
                return Err(Ok(JobResult::Kill));
            }
            Some(cache::Status::Purging) => {
                tracing::warn!("Already purging by other worker, killing");
                return Err(Ok(JobResult::Kill));
            }
            Some(cache::Status::Fetching) if is_force => {
                tracing::warn!("Fetching by other worker, rescheduling due to `is_force`");
                return Err(Ok(JobResult::Reschedule(Duration::from_secs(10))));
            }
            Some(cache::Status::Fetching) if !is_force => {
                tracing::warn!("Fetching by other worker, killing");
                return Err(Ok(JobResult::Kill));
            }
            Some(cache::Status::NotAvailable) if !is_force => {
                tracing::warn!("Cached data not avaliable, killing");
                return Err(Ok(JobResult::Kill));
            }
            Some(cache::Status::NotAvailable) if is_force => false,
            Some(cache::Status::OnlyInfo) => false,
            _ => true,
        };

        cache::update_status(&mut tx, &hash, cache::Status::Purging)
            .await
            .map_err(|e| {
                tracing::error!("Failed to update cache status to `Purging`: {e:#}");
                Err(JobError::Unknown)
            })?;

        transaction!(commit: tx).map_err(Err)?;

        Ok::<_, Result<JobResult, JobError>>(is_nar_file_cached)
    }
    .instrument(tracing::debug_span!("purge_nar_init"))
    .await;

    let is_nar_file_cached = match ret {
        Ok(v) => v,
        Err(ret) => return ret,
    };

    if is_nar_file_cached {
        let nar_file_path = match cache::get_nar_file_path(cache.db_pool(), config, &hash)
            .await
            .map_err(|e| {
                tracing::error!("Failed to get {} narinfo from cache db: {e:#}", hash.string);
                JobError::Unknown
            })? {
            Some(path) => path,
            None => {
                // HACK: There should be a better way of handling this
                tracing::warn!("Race condition, narinfo became unavaliable, retrying");
                return Ok(JobResult::Retry);
            }
        };

        tracing::debug!("Deleting {}", nar_file_path.display());

        tokio::fs::remove_file(nar_file_path).await.map_err(|e| {
            tracing::error!("Error when deleting nar file: {e}");
            JobError::Unknown
        })?;
    }

    cache::purge_nar_info(cache.db_pool(), &hash)
        .await
        .map_err(|e| {
            tracing::error!("Error when deleting narinfo entry from cache db: {e:#}");
            JobError::Unknown
        })?;

    Ok(JobResult::Success)
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct Periodic;

impl ApalisJob for Periodic {
    const NAME: &'static str = "nicacher::jobs::Periodic";
}

async fn periodic_job(_: Periodic, _ctx: JobContext) -> Result<JobResult, JobError> {
    tracing::info!("Ran periodic job");
    Ok(JobResult::Success)
}
