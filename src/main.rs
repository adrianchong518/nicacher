mod fetch;
mod nar;

use actix_web::{get, http::header::ContentType, web, App, HttpResponse, HttpServer, Responder};
use anyhow::Result;
use env_logger::Env;
use tracing::info;
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

#[get("/{store_path_hash}.narinfo")]
async fn get_nar_info(path: web::Path<Hash>) -> impl Responder {
    let store_path_hash = path.into_inner();

    let nar_info = fetch::get_nar_info_raw(&store_path_hash).await.unwrap();

    HttpResponse::Ok()
        .content_type(ContentType(NARINFO_MIME.parse().unwrap()))
        .body(nar_info)
}

#[get("/nar/{hash}.nar.{compression}")]
async fn get_nar_file(path: web::Path<NarFile>) -> impl Responder {
    let nar_file = path.into_inner();

    format!("{nar_file:?}")
}

#[actix_web::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();

    info!("NiCacher Server starts");

    HttpServer::new(move || {
        App::new()
            .wrap(TracingLogger::default())
            .service(index)
            .service(nix_cache_info)
            .service(get_nar_info)
            .service(get_nar_file)
    })
    .bind(("127.0.0.1", 8080))?
    .run()
    .await
    .map_err(anyhow::Error::from)
    //
    // let store_paths = fetch::get_store_paths("nixpkgs-unstable").await?;
    //
    // let derivation = &store_paths[0].derivation;
    //
    // let nar_info = fetch::get_nar_info(derivation).await?;
    //
    // debug!("{nar_info:#?}");
    // debug!("{nar_info}");
    //
    // Ok(())
}
