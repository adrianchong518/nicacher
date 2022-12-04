mod nar;

use crate::nar::NarFile;

use actix_web::{get, web, App, HttpServer, Responder};
use env_logger::Env;
use tracing::info;
use tracing_actix_web::TracingLogger;

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
async fn get_nar_info(path: web::Path<(String,)>) -> impl Responder {
    let (store_path_hash,) = path.into_inner();

    format!("Requested {store_path_hash}.narinfo\n")
}

#[get("/nar/{hash}.nar.{compression}")]
async fn get_nar_file(path: web::Path<NarFile>) -> impl Responder {
    let nar_file = path.into_inner();

    format!("{nar_file:?}")
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
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
}
