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

#[get("/start_download_torrentfile")]
pub async fn start_download_torrentfile(
    state: web::Data<AppState>,
    req_body: web::Query<TorrentFileQuery>,
) -> impl Responder {
    let id = Uuid::new_v4();

    let file_path = format!("{}", &req_body.filepath.trim_matches('"'));
    let mut meta = TorrentFile::new();

    let encoded_data = match meta.read_file(Path::new(&file_path)) {
        Ok(value) => value,

        Err(e) => {
            return HttpResponse::BadRequest().json({
                serde_json::json!({
                    "status": "false",
                    "message":format!("Failed to read file for given path. Error: {}",e)
                })
            });
        }
    };

    meta = meta.parse_metafile(&encoded_data);

    let name = meta.info.name.clone();

    let (length, _files) = match meta.info.key.clone() {
        FileKey::SingleFile { length } => (Some(length), None),
        FileKey::MultiFile { files } => (None, Some(files)),
    };

    {
        let state = state.downloads.lock();
        if let Ok(mut state) = state {
            state.insert(id, DownloadState::new_meta(meta));
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

#[get("/start_download_magnetlink")]
pub async fn start_download_magnetlink(
    state: web::Data<AppState>,
    req_body: web::Query<MagnetLinkQuery>,
) -> impl Responder {
    let id = Uuid::new_v4();

    let mut parser = &mut Parser::new(req_body.url.clone());
    parser = parser.parse();

    let info_hash = &parser.magnet_link.xt.clone();

    let (announce_url, name) = match (parser.magnet_link.tr.clone(), parser.magnet_link.dn.clone())
    {
        (Some(url), Some(name)) => (url, name),
        _ => {
            return HttpResponse::BadRequest().json({
                serde_json::json!({
                    "status": "false",
                    "message":format!("Failed to parse the provided link {:?}. Could not parse announce_url or name of the download.", req_body.url)
                })
            });
        }
    };

    let mut extd_handshake = ExtendedExchange::new(parser);
    let info_hash = if let Ok(info_hash) = hex::decode(info_hash) {
        info_hash
    } else {
        return HttpResponse::InternalServerError().json({
            serde_json::json!({
                "status": "false",
                "message":format!("Failed to decode the info_hash. Please Check your provided url: {}. if incase error persist, feel free to report", req_body.url)
            })
        });
    };

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
    let info = if let Some(inf) = extd
        .handshaking(ips.clone(), info_hash.clone(), &peer_id)
        .await
    {
        inf
    } else {
        return HttpResponse::InternalServerError().json({
            serde_json::json!({
                "status": "false",
                "message":format!("Failed to get metadata from handshaking. Please Check your provided url: {}. if incase error persist, feel free to report", req_body.url)
            })
        });
    };

    let (length, _files) = match info.1.key.clone() {
        FileKey::SingleFile { length } => (Some(length), None),
        FileKey::MultiFile { files } => (None, Some(files)),
    };

    {
        let state = state.downloads.lock();
        if let Ok(mut state) = state {
            state.insert(id, DownloadState::new_magnet(info));
        }
    };

    HttpResponse::Ok().json({
        serde_json::json!({
            "uuid": id.to_string(),
            "name": name,
            "length": length
        })
    })
}
