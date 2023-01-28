use std::{fmt, time::Duration};

use anyhow::Context as _;
use apalis::prelude::{Job as ApalisJob, *};
use serde::{Deserialize, Serialize};
use tracing::Instrument as _;

use crate::{app, cache, config, fetch, nix, transaction};

// TODO: handle `Job::PurgeNar` requests better, ie force actually tries to delete fetching jobs

macro_rules! extract_state {
    ({ $($var:ident),* $(,)? } <- $ctx:expr) => {
        let $crate::app::State { $($var,)* .. } = $ctx.data_opt::<$crate::app::State>().unwrap();
    };
}

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

    pub async fn run(self, state: app::State) -> anyhow::Result<()> {
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

        macro_rules! new_cron_worker {
            ($cron:literal => $job:expr) => {{
                use anyhow::Context as _;
                use apalis::cron::{CronWorker, Schedule};
                use std::str::FromStr as _;
                use tower::ServiceBuilder;

                CronWorker::new(
                    Schedule::from_str($cron).unwrap(),
                    ServiceBuilder::new()
                        .layer(TraceLayer::new().make_span_with(custom_make_span))
                        .layer(Extension(state.clone()))
                        .service(job_fn(|_: Periodic, ctx: JobContext| async move {
                            extract_state!({ workers } <- ctx);
                            let mut workers = workers.clone();

                            let job = $job;
                            tracing::debug!("Running job: {job:?}");

                            workers
                                .push_job(job)
                                .await
                                .context("Failed to push job")
                                .map_err(|e| {
                                    tracing::error!("Job failed: {e:#}");
                                    JobError::Failed(e.into())
                                })?;

                            Ok::<_, JobError>(JobResult::Success)
                        })),
                )
            }};
        }

        let monitor = Monitor::new().register_with_count(4, |_| {
            WorkerBuilder::new(self.storage())
                .layer(TraceLayer::new().make_span_with(custom_make_span))
                .layer(Extension(state.clone()))
                .build_fn(dispatch_jobs)
        });
        // .register(new_cron_worker!("*/10 * * * * *" => Job::Test));

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
    Test,
}

impl ApalisJob for Job {
    const NAME: &'static str = "nicacher::jobs::Job";
}

async fn dispatch_jobs(job: Job, ctx: JobContext) -> Result<JobResult, JobError> {
    extract_state!({ config, cache } <- ctx);

    match job {
        Job::CacheNar { hash, is_force } => cache_nar(config, cache, hash, is_force).await,
        Job::PurgeNar { hash, is_force } => purge_nar(config, cache, hash, is_force).await,
        Job::Test => {
            tracing::info!("Ran test job");
            Ok(JobResult::Success)
        }
    }
    .map_err(|e| {
        tracing::error!("Job failed: {e:#}");
        JobError::Failed(e.into())
    })
}

#[tracing::instrument(skip(config, cache))]
pub async fn cache_nar(
    config: &config::Config,
    cache: &cache::Cache,
    hash: nix::Hash,
    is_force: bool,
) -> anyhow::Result<JobResult> {
    tracing::info!("Caching {} narinfo and corresponding nar file", hash.string);

    let ret = async {
        use cache::db::Status;

        let mut tx = transaction!(begin: cache).map_err(Err)?;

        match cache::db::get_status(&mut tx, &hash).await.map_err(Err)? {
            Some(Status::Fetching) => {
                tracing::warn!("Already fetching by other worker, killing");
                return Err(Ok(JobResult::Kill));
            }
            Some(Status::Available) if !is_force => {
                tracing::warn!("Already cached, killing");
                return Err(Ok(JobResult::Kill));
            }
            Some(Status::Purging) if is_force => {
                tracing::warn!("Purging by other worker, rescheduling due to `is_force`");
                return Err(Ok(JobResult::Reschedule(Duration::from_secs(10))));
            }
            Some(Status::Purging) if !is_force => {
                tracing::warn!("Purging by other worker, killing");
                return Err(Ok(JobResult::Kill));
            }
            _ => {
                cache::db::set_status(&mut tx, &hash, Status::Fetching)
                    .await
                    .map_err(Err)?;
            }
        };

        cache::db::set_last_cached(&mut tx, &hash)
            .await
            .map_err(Err)?;

        transaction!(commit: tx).map_err(Err)?;

        Ok::<_, anyhow::Result<JobResult>>(())
    }
    .instrument(tracing::debug_span!("cache_nar_init"))
    .await;

    if let Err(ret) = ret {
        return ret;
    }

    if let Some(derivation) = fetch::request_derivation(config, &hash).await {
        async {
            let mut tx = transaction!(begin: cache)?;

            cache::db::insert_nar_info(
                &mut tx,
                &hash,
                &derivation.nar_info,
                &derivation.upstream,
                is_force,
            )
            .await?;

            cache::db::set_status(&mut tx, &hash, cache::db::Status::Available).await?;

            cache::write_nar_file(config, &derivation.nar_file).await?;

            transaction!(commit: tx)?;

            tracing::info!("Commit success");

            Ok::<_, anyhow::Error>(())
        }
        .instrument(tracing::debug_span!("cache_nar_insert"))
        .await?;
    } else {
        cache::db::set_status(cache.db_pool(), &hash, cache::db::Status::NotAvailable).await?;
    }

    Ok(JobResult::Success)
}

#[tracing::instrument(skip(config, cache))]
pub async fn purge_nar(
    config: &config::Config,
    cache: &cache::Cache,
    hash: nix::Hash,
    is_force: bool,
) -> anyhow::Result<JobResult> {
    tracing::info!("Purging {} narinfo and corresponding nar file", hash.string);

    let ret = async {
        use cache::db::Status;

        let mut tx = transaction!(begin: cache).map_err(Err)?;

        let nar_file_path = match cache::db::get_status(&mut tx, &hash)
            .await
            .context("Failed to check cache status")
            .map_err(Err)?
        {
            None => {
                tracing::warn!("Not cached, killing");
                return Err(Ok(JobResult::Kill));
            }
            Some(Status::Purging) => {
                tracing::warn!("Already purging by other worker, killing");
                return Err(Ok(JobResult::Kill));
            }
            Some(Status::Fetching) if is_force => {
                tracing::warn!("Fetching by other worker, rescheduling due to `is_force`");
                return Err(Ok(JobResult::Reschedule(Duration::from_secs(10))));
            }
            Some(Status::Fetching) if !is_force => {
                tracing::warn!("Fetching by other worker, killing");
                return Err(Ok(JobResult::Kill));
            }
            Some(Status::NotAvailable) if !is_force => {
                tracing::warn!("Cached data not avaliable, killing");
                return Err(Ok(JobResult::Kill));
            }
            _ => cache::db::get_nar_file_path(cache.db_pool(), config, &hash)
                .await
                .with_context(|| format!("Failed to get {} narinfo from cache db", hash.string))
                .map_err(Err)?,
        };

        cache::db::set_status(&mut tx, &hash, Status::Purging)
            .await
            .map_err(Err)?;

        transaction!(commit: tx).map_err(Err)?;

        Ok::<_, anyhow::Result<JobResult>>(nar_file_path)
    }
    .instrument(tracing::debug_span!("purge_nar_init"))
    .await;

    match ret {
        Ok(Some(path)) => {
            tracing::debug!("Deleting {}", path.display());

            tokio::fs::remove_file(path)
                .await
                .context("Error when deeleting nar file")?;
        }
        Err(ret) => return ret,
        _ => {}
    };

    cache::db::purge_nar_info(cache.db_pool(), &hash)
        .await
        .context("Error when deleting narinfo entry from cache db")?;

    Ok(JobResult::Success)
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct Periodic;

impl ApalisJob for Periodic {
    const NAME: &'static str = "nicacher::jobs::Periodic";
}
