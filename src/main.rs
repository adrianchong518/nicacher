mod cache;
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

    let config = config::get();

    tracing::info!("NiCacher Server starts");

    let (workers, fetch_jobs_storage) = jobs::init(&config).await;

    let datas = (
        web::Data::new(fetch_jobs_storage.clone()),
        web::Data::new(config.clone()),
    );

    let http = actix_web::HttpServer::new(move || {
        actix_web::App::new()
            .wrap(tracing_actix_web::TracingLogger::default())
            .app_data(datas.0.clone())
            .app_data(datas.1.clone())
            .service(index)
            .service(nix_cache_info)
            .service(get_nar_info)
            .service(get_nar_file)
            .service(fetch_nar_info)
            .service(fetch_nar_file)
    })
    .bind(("0.0.0.0", 8080))?
    .run();

    tokio::try_join!(http, workers)?;

    Ok(())
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

    // let nar_info = fetch::request_nar_info_raw(&hash)
    //     .await
    //     .unwrap()
    //     .text()
    //     .await
    //     .unwrap();

    HttpResponse::Ok()
        .content_type(ContentType(nix::NARINFO_MIME.parse().unwrap()))
        .body("LOL")
    // .body(nar_info)
}

#[get("/nar/{hash}.nar.{compression}")]
async fn get_nar_file(nar_file: web::Path<nix::NarFile>) -> impl Responder {
    tracing::debug!("Request for {nar_file:?}");

    format!("{nar_file:?}")
}

#[get("/admin/fetch_nar_info/{hash}")]
async fn fetch_nar_info(
    hash: web::Path<nix::Hash>,
    fetch_jobs_storage: web::Data<SqliteStorage<jobs::FetchJob>>,
) -> impl Responder {
    let mut fetch_jobs_storage = (*fetch_jobs_storage.into_inner()).clone();
    let hash = hash.into_inner();

    match fetch_jobs_storage
        .push(jobs::FetchJob::NarInfo(hash.clone()))
        .await
    {
        Ok(()) => {
            HttpResponse::Ok().body(format!("Pushed job for fetching {hash}.narinfo to queue"))
        }
        Err(e) => HttpResponse::InternalServerError().body(format!(
            "Failed to push job for fetching {hash}.narinfo to queue:\n{e}"
        )),
    }
}

#[get("/admin/fetch_nar_file/{hash}")]
async fn fetch_nar_file(
    hash: web::Path<nix::Hash>,
    fetch_jobs_storage: web::Data<SqliteStorage<jobs::FetchJob>>,
) -> impl Responder {
    let mut fetch_jobs_storage = (*fetch_jobs_storage.into_inner()).clone();
    let hash = hash.into_inner();

    match fetch_jobs_storage
        .push(jobs::FetchJob::NarFile(hash.clone()))
        .await
    {
        Ok(()) => HttpResponse::Ok().body(format!(
            "Pushed job for fetching nar file ({hash}) to queue"
        )),
        Err(e) => HttpResponse::InternalServerError().body(format!(
            "Failed to push job for fetching nar file ({hash}) to queue:\n{e}"
        )),
    }
}
