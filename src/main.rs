mod cache;
mod config;
mod fetch;
mod jobs;
mod nix;

use std::io;

use actix_web::{get, web, HttpResponse, Responder};
use apalis::prelude::*;
use apalis::sqlite::SqliteStorage;
use serde::Deserialize;

const PKG_NAME: &str = env!("CARGO_PKG_NAME");

#[actix_web::main]
async fn main() -> io::Result<()> {
    {
        use tracing::subscriber::set_global_default;
        use tracing_subscriber::filter::EnvFilter;
        use tracing_subscriber::prelude::*;

        tracing_log::LogTracer::init().expect("Failed to set logger");

        let env_filter = EnvFilter::try_from_env("NICACHER_LOG")
            .unwrap_or_else(|_| EnvFilter::new("info"))
            .add_directive("sqlx::query=warn".parse().unwrap());

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

    let cache = cache::Cache::new(&config)
        .await
        .expect("Failed to initialize cache");

    let (workers, jobs_storage) = jobs::init(&config, &cache).await;

    let datas = (
        web::Data::new(config.clone()),
        web::Data::new(jobs_storage.clone()),
        web::Data::new(cache.clone()),
    );

    let http = actix_web::HttpServer::new(move || {
        actix_web::App::new()
            .wrap(tracing_actix_web::TracingLogger::default())
            .app_data(datas.0.clone())
            .app_data(datas.1.clone())
            .app_data(datas.2.clone())
            .service(index)
            .service(nix_cache_info)
            .service(get_nar_info)
            .service(get_nar_file)
            .service(cache_nar)
            .service(list_cache_diff)
    })
    .bind(("0.0.0.0", 8080))?
    .run();

    tokio::try_join!(http, workers)?;

    cache.cleanup().await;

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
async fn get_nar_info(
    hash: web::Path<nix::Hash>,
    jobs_storage: web::Data<SqliteStorage<jobs::Jobs>>,
    cache: web::Data<cache::Cache>,
) -> impl Responder {
    use actix_web::http::header::ContentType;

    let mut jobs_storage = (*jobs_storage.into_inner()).clone();

    tracing::info!("Request for {}.narinfo", hash.string);

    match cache.get_nar_info(&hash).await {
        Ok(Some(nar_info)) => HttpResponse::Ok()
            .content_type(ContentType(nix::NARINFO_MIME.parse().unwrap()))
            .body(nar_info.to_string()),
        Ok(None) => {
            tracing::info!("Cache miss, pushing job to attempt caching");

            let job = jobs::Jobs::CacheNar {
                hash: hash.clone(),
                is_force: false,
            };
            match jobs_storage.push(job.clone()).await {
                Ok(()) => {
                    HttpResponse::NotFound().body(format!("{}.narinfo unavaliable", hash.string))
                }
                Err(err) => {
                    tracing::error!("Failed to push {job:?}: {err}");
                    HttpResponse::InternalServerError().body(format!(
                        "Failed to request caching of {}.narinfo due to internal error: {err}",
                        hash.string
                    ))
                }
            }
        }
        Err(err) => {
            tracing::error!("Failed to get narinfo due to internal error: {err}");
            HttpResponse::InternalServerError().body(format!(
                "Failed to get {}.narinfo due to internal error: {err}",
                hash.string
            ))
        }
    }
}

#[get("/nar/{hash}.nar.{compression}")]
async fn get_nar_file(
    req: actix_web::HttpRequest,
    nar_file: web::Path<nix::NarFile>,
    config: web::Data<config::Config>,
    cache: web::Data<cache::Cache>,
) -> impl Responder {
    tracing::info!("Request for {nar_file}");

    (|| async {
        if cache.is_nar_file_cached(&nar_file).await? {
            Ok(actix_files::NamedFile::open_async(
                config
                    .local_data_path
                    .join(cache::NAR_FILE_DIR)
                    .join(nar_file.to_string()),
            )
            .await?
            .into_response(&req))
        } else {
            Ok::<_, anyhow::Error>(HttpResponse::NotFound().body(format!("{nar_file} unavaliable")))
        }
    })()
    .await
    .unwrap_or_else(|err| {
        HttpResponse::InternalServerError().body(format!(
            "Failed to get {nar_file} due to internal error: {err}"
        ))
    })
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct IsForce {
    #[serde(rename = "force")]
    is_force: bool,
}

#[get("/admin/cache_nar/{hash}")]
async fn cache_nar(
    hash: web::Path<nix::Hash>,
    web::Query(IsForce { is_force }): web::Query<IsForce>,
    jobs_storage: web::Data<SqliteStorage<jobs::Jobs>>,
) -> impl Responder {
    let mut jobs_storage = (*jobs_storage.into_inner()).clone();
    let hash = hash.into_inner();

    match jobs_storage
        .push(jobs::Jobs::CacheNar {
            hash: hash.clone(),
            is_force,
        })
        .await
    {
        Ok(()) => HttpResponse::Ok().body(format!("Pushed job for caching {hash} to queue")),
        Err(e) => HttpResponse::InternalServerError().body(format!(
            "Failed to push job for caching {hash} to queue:\n{e}"
        )),
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct ListLimit {
    limit: usize,
}

impl Default for ListLimit {
    fn default() -> Self {
        Self { limit: 30 }
    }
}

#[get("/admin/list_cache_diff")]
async fn list_cache_diff(
    web::Query(ListLimit { limit }): web::Query<ListLimit>,
    config: web::Data<config::Config>,
    cache: web::Data<cache::Cache>,
) -> impl Responder {
    use std::collections::BTreeSet;

    let cached_store_paths = match cache.get_store_paths::<BTreeSet<_>>().await {
        Ok(v) => v,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .body(format!("Failed to get cached store paths:\n{e}"))
        }
    };

    let upstream_store_paths =
        match fetch::request_store_paths::<BTreeSet<_>>(&config, &config.channels[0]).await {
            Ok(v) => v,
            Err(e) => {
                return HttpResponse::InternalServerError()
                    .body(format!("Failed to request up-to-date store paths:\n{e}"))
            }
        };

    tracing::debug!("Proccessing difference between local cache and upstream");

    let diff = upstream_store_paths.difference(&cached_store_paths);

    let diff_len = diff.clone().count();

    let res = if diff_len == 0 {
        "No missing derivations from cache".to_string()
    } else {
        format!(
            "\
Number of missing derivations from cache: {diff_len}
----------------------------------------------------
Store paths of first {limit} of missing derivations:

{}
",
            diff.take(limit)
                .map(nix::StorePath::to_string)
                .reduce(|acc, path| acc + "\n" + &path)
                .unwrap()
        )
    };

    HttpResponse::Ok().body(res)
}
