use std::io;

use apalis::prelude::*;
use serde::{Deserialize, Serialize};

pub async fn config_workers(storage: &apalis::sqlite::SqliteStorage<TestJob>) -> io::Result<()> {
    let cron_worker = {
        use std::str::FromStr as _;

        use apalis::cron::{CronWorker, Schedule};
        use tower::ServiceBuilder;

        CronWorker::new(
            Schedule::from_str("*/5 * * * * *").unwrap(),
            ServiceBuilder::new().service(job_fn(run_test_job)),
        )
    };

    Monitor::new()
        .register_with_count(2, move |_| {
            WorkerBuilder::new(storage.clone()).build_fn(run_test_job)
        })
        .register(cron_worker)
        .run()
        .await
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct TestJob(u8);

impl Job for TestJob {
    const NAME: &'static str = "nicacher::TestJob";
}

async fn run_test_job(job: TestJob, _ctx: JobContext) -> Result<JobResult, JobError> {
    tracing::info!("Ran test job {}", job.0);
    Ok(JobResult::Success)
}
