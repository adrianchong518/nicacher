use std::{fmt, io};

use apalis::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{cache, config, fetch, nix};

pub async fn init<'a>(
    config: &'a config::Config,
    cache: &'a cache::Cache,
) -> (
    impl std::future::Future<Output = io::Result<()>> + 'a,
    apalis::sqlite::SqliteStorage<Jobs>,
) {
    let storage = apalis::sqlite::SqliteStorage::connect("sqlite::memory:")
        .await
        .expect("Unable to connect to in-memory sqlite database");
    storage
        .setup()
        .await
        .expect("Unable to migrate sqlite database");

    let workers = {
        let storage = storage.clone();

        async move {
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

            Monitor::new()
                .register_with_count(4, move |_| {
                    WorkerBuilder::new(storage.clone())
                        .layer(TraceLayer::new().make_span_with(custom_make_span))
                        .layer(Extension(config.clone()))
                        .layer(Extension(cache.clone()))
                        .build_fn(dispatch_jobs)
                })
                // .register(cron_worker)
                .run()
                .await
        }
    };

    (workers, storage)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Jobs {
    CacheNar { hash: nix::Hash, is_force: bool },
}

impl Job for Jobs {
    const NAME: &'static str = "nicacher::jobs::Jobs";
}

async fn dispatch_jobs(job: Jobs, ctx: JobContext) -> Result<JobResult, JobError> {
    let config = ctx.data_opt::<config::Config>().unwrap();
    let cache = ctx.data_opt::<cache::Cache>().unwrap();

    match job {
        Jobs::CacheNar { hash, is_force } => {
            tracing::info!("Caching {}.narinfo and corresponding nar file", hash.string);

            let is_cached = cache.is_nar_info_cached(&hash).await.map_err(|e| {
                tracing::error!("When checking cache status: {e}");
                JobError::Unknown
            })?;

            if !is_force && is_cached {
                tracing::warn!(
                    "{}.narinfo is already cached, skipping insertion",
                    hash.string
                );

                return Ok(JobResult::Success);
            }

            let (nar_info, upstream) =
                fetch::request_nar_info(config, &hash).await.map_err(|e| {
                    tracing::error!("Error when requesting narinfo: {e}");
                    JobError::Unknown
                })?;

            cache
                .insert_nar_info(&hash, &nar_info, is_force)
                .await
                .map_err(|e| {
                    tracing::error!("Error when inserting narinfo into cache database: {e}",);
                    JobError::Unknown
                })?;

            fetch::download_nar_file(config, &upstream, &nar_info)
                .await
                .map_err(|e| {
                    tracing::error!("Error when downloading nar file: {e}");
                    JobError::Unknown
                })?;
        }
    };

    Ok(JobResult::Success)
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct Periodic;

impl Job for Periodic {
    const NAME: &'static str = "nicacher::jobs::Periodic";
}

async fn periodic_job(_: Periodic, _ctx: JobContext) -> Result<JobResult, JobError> {
    tracing::info!("Ran periodic job");
    Ok(JobResult::Success)
}
