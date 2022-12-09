mod fetch;
mod nar;

use std::{io, str::FromStr};

use actix_web::{get, http::header::ContentType, web, App, HttpResponse, HttpServer, Responder};
use apalis::{
    cron::{CronWorker, Schedule},
    prelude::*,
    sqlite::SqliteStorage,
};
use env_logger::Env;
use serde::{Deserialize, Serialize};
use tower::ServiceBuilder;
use tracing::{info, trace};
use tracing_actix_web::TracingLogger;

use crate::nar::{Hash, NarFile, NARINFO_MIME};

#[get("/")]
async fn index() -> impl Responder {
    "NiCacher is up!"
}

#[get("/nix-cache-info")]
async fn nix_cache_info() -> impl Responder {
    "StoreDir: /nix/store\n\
    WantMassQuery: 0\n\
    Priority: 30\n"
}

#[get("/{hash}.narinfo")]
async fn get_nar_info(hash: web::Path<Hash>) -> HttpResponse {
    let nar_info = fetch::get_nar_info_raw(&hash)
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    HttpResponse::Ok()
        .content_type(ContentType(NARINFO_MIME.parse().unwrap()))
        .body(nar_info)
}

#[get("/nar/{hash}.nar.{compression}")]
async fn get_nar_file(nar_file: web::Path<NarFile>) -> impl Responder {
    format!("{nar_file:?}")
}

#[actix_web::main]
async fn main() -> io::Result<()> {
    let env = Env::default().default_filter_or("info,sqlx::query=warn");
    env_logger::Builder::from_env(env).init();

    info!("NiCacher Server starts");

    let storage = SqliteStorage::connect("sqlite::memory:")
        .await
        .expect("Unable to connect to in-memory sqlite database");
    storage
        .setup()
        .await
        .expect("Unable to migrate sqlite database");

    let data = web::Data::new(storage.clone());

    let http = HttpServer::new(move || {
        App::new()
            .wrap(TracingLogger::default())
            .app_data(data.clone())
            .service(index)
            .service(nix_cache_info)
            .service(get_nar_info)
            .service(get_nar_file)
            .route("/test/{id}", web::to(test_job_endpoint))
    })
    .bind(("127.0.0.1", 8080))?
    .run();

    let cron_worker = CronWorker::new(
        Schedule::from_str("*/10 * * * * *").unwrap(),
        ServiceBuilder::new().service(job_fn(run_test_job)),
    );

    let workers = Monitor::new()
        .register_with_count(2, move |_| {
            WorkerBuilder::new(storage.clone()).build_fn(run_test_job)
        })
        .register(cron_worker)
        .run();

    tokio::try_join!(http, workers)?;

    Ok(())
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct TestJob(u8);

impl Job for TestJob {
    const NAME: &'static str = "nicacher::TestJob";
}

async fn run_test_job(job: TestJob, _ctx: JobContext) -> Result<JobResult, JobError> {
    info!("Ran test job {}", job.0);
    Ok(JobResult::Success)
}

async fn test_job_endpoint(
    job: web::Path<TestJob>,
    storage: web::Data<SqliteStorage<TestJob>>,
) -> HttpResponse {
    let storage = &*storage.into_inner();
    let mut storage = storage.clone();
    let res = storage.push(job.into_inner()).await;

    match res {
        Ok(()) => HttpResponse::Ok().body(format!("Test Job added to queue")),
        Err(e) => HttpResponse::InternalServerError().body(format!("{e}")),
    }
}
