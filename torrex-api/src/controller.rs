use actix_web::HttpResponse;
use actix_web::Responder;
use actix_web::get;
use actix_web::post;
use actix_web::web;
use serde::Deserialize;
use torrex_lib::metainfo;
use uuid::Uuid;

use std::path::Path;

use torrex_lib::extension::magnet_link::ExtendedExchange;
use torrex_lib::extension::magnet_link::ExtendedMetadataExchange;
use torrex_lib::extension::magnet_link::Parser;
use torrex_lib::metainfo::FileKey;
use torrex_lib::metainfo::TorrentFile;
use torrex_lib::random;

use crate::state::AppState;
use crate::state::DownloadState;

#[get("")]
pub async fn init() -> impl Responder {
    HttpResponse::Ok().json(serde_json::json!({
        "status": "ok",
        "message":"Torrex API is running"
    }))
}

#[derive(Debug, Deserialize)]
struct TorrentFileQuery {
    filepath: String,
}

#[post("/start_download_torrentfile")]
pub async fn start_download_torrentfile(
    state: web::Data<AppState>,
    req_body: web::Json<TorrentFileQuery>,
) -> impl Responder {
    let id = Uuid::new_v4();

    println!("req body {:?}", req_body);

    let mut meta = TorrentFile::new();
    let encoded_data = meta.read_file(Path::new(&req_body.filepath)).unwrap();
    meta = meta.parse_metafile(&encoded_data);

    let name = meta.info.name.clone();

    let (length, _files) = match meta.info.key.clone() {
        FileKey::SingleFile { length } => (Some(length), None),
        FileKey::MultiFile { files } => (None, Some(files)),
    };

    {
        let state = state.downloads.lock();
        if let Ok(mut state) = state {
            state.insert(id, DownloadState::new(Some(meta), None));
        }
    };

    HttpResponse::Ok().json({
        serde_json::json!({
            "uuid": id.to_string(),
            "name": name,
            "length": length.unwrap()
        })
    })
}

#[derive(Debug, Deserialize)]
struct MagnetLinkQuery {
    url: String,
}

#[post("/start_download_magnetlink")]
pub async fn start_download_magnetlink(
    state: web::Data<AppState>,
    req_body: web::Json<MagnetLinkQuery>,
) -> impl Responder {
    let id = Uuid::new_v4();

    let mut parser = &mut Parser::new(req_body.url.clone());
    parser = parser.parse();

    let info_hash = &parser.magnet_link.xt.clone();
    let announce_url = &parser
        .magnet_link
        .tr
        .clone()
        .expect("Tracker URL (tr=...) missing in magnet link");

    let mut extd_handshake = ExtendedExchange::new(parser);

    let info_hash = hex::decode(info_hash).expect("Could not decode provided info_hash");
    let peer_id = random::generate_magnet_peerid();

    let ips = extd_handshake
        .set_request(
            info_hash.clone(),
            peer_id.clone(), // random string
            None,
            6881,
            0,
            0,
            999,
            None,
            1,
        )
        .set_url(announce_url.to_string())
        .request_tracker()
        .await
        .peers_ip();

    let extd = ExtendedMetadataExchange::new();
    let info: (ExtendedMetadataExchange, metainfo::Info) = extd
        .handshaking(ips.clone(), info_hash.clone(), &peer_id)
        .await
        .unwrap();

    let (length, _files) = match info.1.key.clone() {
        FileKey::SingleFile { length } => (Some(length), None),
        FileKey::MultiFile { files } => (None, Some(files)),
    };

    {
        let state = state.downloads.lock();
        if let Ok(mut state) = state {
            state.insert(id, DownloadState::new(None, Some(info)));
        }
    };

    HttpResponse::Ok().json({
        serde_json::json!({
            "uuid": id.to_string(),
            "name": parser.magnet_link.dn.as_ref().unwrap(),
            "length": length
        })
    })
}
