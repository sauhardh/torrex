use std::collections::HashMap;
use std::sync::Mutex;

use actix_web::App;
use actix_web::HttpServer;
use actix_web::web;
use log::info;

mod controller;
mod state;

use controller::start_download_magnetlink;
use controller::start_download_torrentfile;
use state::AppState;

use crate::controller::init;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));
    info!("Starting server");

    let app_state = web::Data::new(AppState {
        downloads: Mutex::new(HashMap::new()),
    });

    HttpServer::new(move || {
        App::new().app_data(app_state.clone()).service(
            web::scope("/torrex/api/v1")
                .service(start_download_torrentfile)
                .service(start_download_magnetlink)
                .service(init),
        )
    })
    .bind(("127.0.0.1", 7878))?
    .run()
    .await
}
