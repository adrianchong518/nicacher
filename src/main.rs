mod config;
mod fetch;
mod jobs;
mod nix;

use std::io;

use actix_web::{get, web, HttpResponse, Responder};
use apalis::prelude::*;
use apalis::sqlite::SqliteStorage;

const PKG_NAME: &str = env!("CARGO_PKG_NAME");

#[actix_web::main]
async fn main() -> io::Result<()> {
    {
        use tracing::subscriber::set_global_default;
        use tracing_subscriber::filter::EnvFilter;
        use tracing_subscriber::prelude::*;

        tracing_log::LogTracer::init().expect("Failed to set logger");

        let env_filter = EnvFilter::try_from_env("NICACHER_LOG")
            .unwrap_or_else(|_| EnvFilter::new("info,sqlx::query=warn"));

        let formatting_layer =
            tracing_bunyan_formatter::BunyanFormattingLayer::new(PKG_NAME.into(), std::io::stdout);

        let subscriber = tracing_subscriber::Registry::default()
            .with(formatting_layer)
            .with(tracing_bunyan_formatter::JsonStorageLayer)
            .with(env_filter);

        set_global_default(subscriber).expect("Failed to set subscriber");
    }

    let _config = config::get();

    tracing::info!("NiCacher Server starts");

    let jobs_storage = SqliteStorage::connect("sqlite::memory:")
        .await
        .expect("Unable to connect to in-memory sqlite database");
    jobs_storage
        .setup()
        .await
        .expect("Unable to migrate sqlite database");

    let data = web::Data::new(jobs_storage.clone());

    let http = actix_web::HttpServer::new(move || {
        actix_web::App::new()
            .wrap(tracing_actix_web::TracingLogger::default())
            .app_data(data.clone())
            .service(index)
            .service(nix_cache_info)
            .service(get_nar_info)
            .service(get_nar_file)
            .route("/test/{id}", web::to(test_job_endpoint))
    })
    .bind(("0.0.0.0", 8080))?
    .run();

    let workers = jobs::config_workers(&jobs_storage);

    tokio::try_join!(http, workers)?;

    Ok(())
}

use jobs::TestJob;

async fn test_job_endpoint(
    job: web::Path<TestJob>,
    jobs_storage: web::Data<SqliteStorage<TestJob>>,
) -> HttpResponse {
    let mut storage = (*jobs_storage.into_inner()).clone();
    let job = job.into_inner();

    match storage.push(job.clone()).await {
        Ok(()) => HttpResponse::Ok().body(format!("Test Job added to queue: {job:?}")),
        Err(e) => HttpResponse::InternalServerError().body(format!("{e}")),
    }
}

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
async fn get_nar_info(hash: web::Path<nix::Hash>) -> HttpResponse {
    use actix_web::http::header::ContentType;

    let nar_info = fetch::get_nar_info_raw(&hash)
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    HttpResponse::Ok()
        .content_type(ContentType(nix::NARINFO_MIME.parse().unwrap()))
        .body(nar_info)
}

#[get("/nar/{hash}.nar.{compression}")]
async fn get_nar_file(nar_file: web::Path<nix::NarFile>) -> impl Responder {
    tracing::debug!("Request for {nar_file:?}");

    format!("{nar_file:?}")
}
