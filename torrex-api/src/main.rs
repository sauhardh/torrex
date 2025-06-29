use actix_cors::Cors;
use actix_web::App;
use actix_web::HttpServer;
use actix_web::middleware;
use actix_web::web;
use log::info;

use std::collections::HashMap;
use std::sync::Mutex;

mod controller;
mod state;

use controller::initial_download_info_magnet;
use controller::initial_download_info_metafile;
use state::AppState;

use crate::controller::download_progress;
use crate::controller::init;
use crate::controller::pause_download;
use crate::controller::resume_download;
use crate::controller::start_download;
use crate::controller::stop_download;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));
    const PORT: u16 = 7878;
    info!("Starting server localhost at port {PORT}");

    let app_state = web::Data::new(AppState {
        downloads: Mutex::new(HashMap::new()),
        managers: Mutex::new(HashMap::new()),
        receiver: Mutex::new(HashMap::new()),
    });

    HttpServer::new(move || {
        let cors = Cors::permissive();

        App::new()
            .wrap(cors)
            .app_data(app_state.clone())
            .service(
                web::scope("/torrex/api/v1")
                    .service(init)
                    .service(initial_download_info_magnet)
                    .service(initial_download_info_metafile)
                    .service(start_download)
                    .service(download_progress)
                    .service(pause_download)
                    .service(resume_download)
                    .service(stop_download),
            )
            .wrap(middleware::NormalizePath::trim())
            .wrap(middleware::Logger::default())
    })
    .workers(3)
    .bind(("127.0.0.1", PORT))?
    .run()
    .await
}
