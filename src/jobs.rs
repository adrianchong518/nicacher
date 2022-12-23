use std::{fmt, io};

use apalis::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{config, fetch, nix};

pub async fn init(
    config: &config::Config,
) -> (
    impl std::future::Future<Output = io::Result<()>> + '_,
    apalis::sqlite::SqliteStorage<FetchJob>,
) {
    let storage = apalis::sqlite::SqliteStorage::connect("sqlite::memory:")
        .await
        .expect("Unable to connect to in-memory sqlite database");
    storage
        .setup()
        .await
        .expect("Unable to migrate sqlite database");

    (config_workers(config, storage.clone()), storage)
}

async fn config_workers(
    config: &config::Config,
    storage: apalis::sqlite::SqliteStorage<FetchJob>,
) -> io::Result<()> {
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
                .build_fn(dispatch_fetch)
        })
        // .register(cron_worker)
        .run()
        .await
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum FetchJob {
    NarInfo(nix::Hash),
    NarFile(nix::Hash),
}

impl Job for FetchJob {
    const NAME: &'static str = "nicacher::jobs::FetchJob";
}

async fn dispatch_fetch(job: FetchJob, ctx: JobContext) -> Result<JobResult, JobError> {
    let config = ctx.data_opt::<config::Config>().unwrap();

    match job {
        FetchJob::NarInfo(hash) => {
            let file_path = config
                .local_data_path
                .join("narinfo")
                .join(format!("{hash}.narinfo"));

            fetch::download_nar_info(&config, &hash, file_path)
                .await
                .unwrap();
        }

        FetchJob::NarFile(hash) => {
            let nar_info = fetch::request_nar_info(&config, &hash).await.unwrap();
            let file_path = config.local_data_path.join(&nar_info.url);

            fetch::download_nar_file(&config, &nar_info.url, file_path)
                .await
                .unwrap();
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
