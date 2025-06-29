use actix_web::HttpRequest;
use actix_web::HttpResponse;
use actix_web::Responder;
use actix_web::get;
use actix_web::post;
use actix_web::web;
use actix_ws::CloseReason;
use log::info;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use torrex_lib::connection::connection::Progress;
use uuid::Uuid;

use std::path::Path;

use torrex_lib::connection::connection::SwarmManager;
use torrex_lib::extension::magnet_link::ExtendedExchange;
use torrex_lib::extension::magnet_link::ExtendedMetadataExchange;
use torrex_lib::extension::magnet_link::Parser;
use torrex_lib::metainfo::FileKey;
use torrex_lib::metainfo::TorrentFile;
use torrex_lib::peers::Peers;
use torrex_lib::random;

use crate::state::AppState;
use crate::state::DownloadKind;
use crate::state::DownloadState;

#[get("")]
pub async fn init() -> impl Responder {
    HttpResponse::Ok().json(json!({
        "success": "true",
        "message":"Torrex API is running"
    }))
}

#[derive(Debug, Deserialize)]
struct TorrentFileQuery {
    filepath: String,
}

#[get("/initial_info_metafile")]
pub async fn initial_download_info_metafile(
    state: web::Data<AppState>,
    query: web::Query<TorrentFileQuery>,
) -> impl Responder {
    let id = Uuid::new_v4();

    let file_path = format!("{}", &query.filepath.trim_matches('"'));
    let mut meta = TorrentFile::new();

    let encoded_data = match meta.read_file(Path::new(&file_path)) {
        Ok(value) => value,

        Err(e) => {
            return HttpResponse::BadRequest().json(json!({
                "success": "false",
                "message":format!("Failed to read file for given path. Error: {}",e)
            }));
        }
    };

    meta = meta.parse_metafile(&encoded_data);
    let info_hash = meta.info_hash(&encoded_data).unwrap().to_vec();

    let name = meta.info.name.clone();

    let (length, _files) = match meta.info.key.clone() {
        FileKey::SingleFile { length } => (Some(length), None),
        FileKey::MultiFile { files } => (None, Some(files)),
    };

    // Discovering peers
    let self_peer_id = random::generate_peerid();
    let mut peers = Peers::new();
    let params = &peers.request.new(
        info_hash.to_vec(),
        self_peer_id.clone(), // random string
        None,
        6881,
        0,
        0,
        length.unwrap(),
        None,
        1,
    );

    let peers = peers
        .announce_url(meta.announce.clone())
        .request_tracker(&params)
        .await;

    println!("peers {peers:?}");
    // TODO: handle more gracefully
    let ips = peers.response.as_ref().unwrap().peers_ip();
    println!("___ips  are__: {ips:?}");

    {
        let state = state.downloads.lock();
        if let Ok(mut state) = state {
            state.insert(
                id,
                DownloadState::new_meta(meta, info_hash, self_peer_id, ips, length.unwrap()),
            );
        }
    };

    return HttpResponse::Ok().json(json!({
        "success": "true",
        "uuid": id.to_string(),
        "name": name,
        "length": length.unwrap(),
    }));
}

#[derive(Debug, Deserialize)]
struct MagnetLinkQuery {
    url: String,
}

#[get("/initial_info_magnet")]
pub async fn initial_download_info_magnet(
    state: web::Data<AppState>,
    query: web::Query<MagnetLinkQuery>,
) -> impl Responder {
    let id = Uuid::new_v4();

    let mut parser = &mut Parser::new(query.url.clone());
    parser = parser.parse();

    let info_hash = &parser.magnet_link.xt.clone();

    let announce_url = parser.magnet_link.tr.clone();
    let name = parser
        .magnet_link
        .dn
        .clone()
        .unwrap_or_else(|| "torrex_UnknownDownload".to_string());

    let mut extd_handshake = ExtendedExchange::new(parser);
    let info_hash = if let Ok(info_hash) = hex::decode(info_hash) {
        info_hash
    } else {
        return HttpResponse::InternalServerError().json(
            json!({
                "success": "false",
                "message":format!("Failed to decode the info_hash. Please Check your provided url: {}. if incase error persist, feel free to report", query.url)
            }));
    };

    let peer_id = random::generate_magnet_peerid();
    let ips = if let Some(url) = announce_url {
        extd_handshake
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
            .set_url(url)
            .request_tracker()
            .await
            .peers_ip()
    } else {
        return HttpResponse::BadRequest().json(json!({
            "success": "false",
            "message": "Could not get accounce_url. Currently trackerless features is not implemented"
        }));
    };

    let extd = ExtendedMetadataExchange::new();
    println!("parser: {:?}", parser);
    println!("ips: {:?}", ips);
    let info = if let Some(inf) = extd
        .handshaking(ips.clone(), info_hash.clone(), &peer_id)
        .await
    {
        inf
    } else {
        return HttpResponse::InternalServerError().json(
            json!({
                "success": "false",
                "message":format!("Failed to get metadata from handshaking. Please Check your provided url: {}. if incase error persist, feel free to report", query.url)
            }));
    };

    let (length, _files) = match info.1.key.clone() {
        FileKey::SingleFile { length } => (Some(length), None),
        FileKey::MultiFile { files } => (None, Some(files)),
    };

    {
        let state = state.downloads.lock();
        let self_peer_id = random::generate_magnet_peerid();
        if let Ok(mut state) = state {
            state.insert(
                id,
                DownloadState::new_magnet(info, info_hash, self_peer_id, ips, length.unwrap()),
            );
        }
    };

    return HttpResponse::Ok().json(json!({
        "success":"true",
        "uuid": id.to_string(),
        "name": name,
        "length": length.unwrap()
    }));
}

#[derive(Debug, Deserialize)]
struct StartDownloadQuery {
    uuid: String,
    destination: Option<String>,
}

#[post("/start_download")]
pub async fn start_download(
    req_body: web::Json<StartDownloadQuery>,
    state: web::Data<AppState>,
) -> impl Responder {
    let Ok(uuid) = Uuid::parse_str(&req_body.uuid) else {
        return HttpResponse::BadRequest().json(json!({
            "success":"false",
            "message":"Failed to parse uuid to string"
        }));
    };

    let (info_hash, kind, self_peer_id, ips, length) = {
        let state = match state.downloads.lock() {
            Ok(s) => s,
            Err(e) => {
                return HttpResponse::InternalServerError().json(json!({
                    "success":"false",
                    "message":format!("Initial download states are not present. Failed to lock the resources {e}")
                }));
            }
        };

        if let Some(dl_state) = state.get(&uuid) {
            (
                dl_state.info_hash.clone(),
                dl_state.kind.clone(),
                dl_state.self_peer_id.clone(),
                dl_state.ips.clone(),
                dl_state.length,
            )
        } else {
            return HttpResponse::BadRequest().json(json!({
            "success":"false",
            "message":format!("Failed to match the downloading state with provided uuid: {}",uuid)
        }));
        }
    };

    let temp_dir = std::env::temp_dir();

    let sm = SwarmManager::init();
    let (tx, rx) = mpsc::channel::<Progress>(16);

    {
        let receiver = state.receiver.lock();
        if let Ok(mut r) = receiver {
            if r.get(&uuid).is_none() {
                r.insert(uuid, rx);
            }
        }
    }

    {
        if let Ok(mut manager_guard) = state.managers.lock() {
            if manager_guard.get(&uuid).is_none() {
                manager_guard.insert(uuid, sm);
            };
        }
    }

    actix_web::rt::spawn(async move {
        match kind {
            //------To download using the magnet link------
            DownloadKind::Magnet((_, info)) => {
                let mut manager = state.managers.lock().unwrap();
                let sm = manager.get_mut(&uuid).unwrap();

                // subscribe for updates
                // subscribe for download manager
                let sm = sm.subscribe_updates(tx).subscribe_downloadmanager().await;

                let file_size = length;
                let piece_length = info.piece_length;

                sm.connect_and_exchange_bitfield(
                    ips,
                    info_hash.to_vec(),
                    self_peer_id.as_bytes().to_vec(),
                    file_size,
                    piece_length,
                )
                .await;
                // sm.connect_and_exchange_bitfield(
                //     ips,
                //     info_hash.to_vec(),
                //     self_peer_id.as_bytes().to_vec(),
                // )
                // .await;
                let destination = if let Some(dest) = &req_body.destination {
                    dest.clone()
                } else {
                    temp_dir
                        .join(info.name.clone())
                        .to_string_lossy()
                        .to_string()
                };

                sm.destination(destination)
                    .final_peer_msg(length, &info.pieces_hashes(), info.piece_length)
                    .await;
            }

            //------To download using the torrent file------
            DownloadKind::Meta(meta) => {
                let mut manager = state.managers.lock().unwrap();
                let sm = manager.get_mut(&uuid).unwrap();

                // subscribe for updates
                let sm = sm.subscribe_updates(tx).subscribe_downloadmanager().await;

                let file_size = length;
                let piece_length = meta.info.piece_length;

                sm.connect_and_exchange_bitfield(
                    ips,
                    info_hash.to_vec(),
                    self_peer_id.as_bytes().to_vec(),
                    file_size,
                    piece_length,
                )
                .await;
                // sm.connect_and_exchange_bitfield(
                //     ips,
                //     info_hash.to_vec(),
                //     self_peer_id.as_bytes().to_vec(),
                // )
                // .await;

                let destination = if let Some(dest) = &req_body.destination {
                    dest.clone()
                } else {
                    temp_dir
                        .join(meta.info.name.clone())
                        .to_string_lossy()
                        .to_string()
                };

                sm.destination(destination)
                    .final_peer_msg(file_size, &meta.info.pieces_hashes(), piece_length)
                    .await;
            }
        }
    });

    return HttpResponse::Ok().json(json!({
        "success": "true",
        "uuid":uuid.to_string(),
        "message": "Download has been started"
    }));
}

#[get("/ws/download/{uuid}")]
async fn download_progress(
    req: HttpRequest,
    stream: web::Payload,
    state: web::Data<AppState>,
    uuid: web::Path<String>,
) -> Result<HttpResponse, actix_web::Error> {
    let (res, mut session, _stream) = actix_ws::handle(&req, stream)?;

    let Ok(uuid) = Uuid::parse_str(&uuid) else {
        return Ok(HttpResponse::BadRequest().json(json!({
            "success":"false",
            "message":"Failed to parse uuid to string"
        })));
    };

    let rx_opt = {
        if let Ok(mut receiver) = state.receiver.lock() {
            receiver.remove(&uuid)
        } else {
            None
        }
    };

    let Some(mut rx) = rx_opt else {
        let _ = session.text("Download not in progress").await;
        let _ = session
            .close(Some(CloseReason {
                code: 1000.into(),
                description: Some("No download in progress".into()),
            }))
            .await;

        return Ok(res);
    };

    actix_web::rt::spawn(async move {
        while let Some(progress) = rx.recv().await {
            let msg = serde_json::to_string(&progress).unwrap();
            if session.text(msg).await.is_err() {
                break;
            }
        }

        // let _ = session
        //     .close(Some(CloseReason {
        //         code: 1001.into(),
        //         description: Some("No more progress to send".into()),
        //     }))
        //     .await;
    });

    Ok(res)
}

#[derive(Deserialize)]
struct UuidQuery {
    uuid: String,
}

#[get("/pause")]
async fn pause_download(
    state: web::Data<AppState>,
    query: web::Query<UuidQuery>,
) -> impl Responder {
    let Ok(uuid) = Uuid::parse_str(&query.uuid) else {
        return HttpResponse::BadRequest().json(json!({
            "success":"false",
            "message":"Failed to parse uuid to string"
        }));
    };

    let state = state.managers.lock();
    let s_guard = match state {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to lock the state. {e:?}");
            return HttpResponse::BadRequest().json(json!({
                "success":"false",
                "message":"Failed to parse uuid to string"
            }));
        }
    };

    if let Some(s) = s_guard.get(&uuid) {
        let dm = s.download_manager.lock().await;
        if let Err(e) = dm.pause().await {
            return HttpResponse::BadRequest().json(json!({
                "success":"false",
                "message": format!("Failed to pause the download. {:?}", e)
            }));
        };
    };

    return HttpResponse::Ok().json(json!({
        "success": "true",
        "uuid": uuid.to_string()
    }));
}

#[get("/stop")]
async fn stop_download(state: web::Data<AppState>, query: web::Query<UuidQuery>) -> impl Responder {
    let Ok(uuid) = Uuid::parse_str(&query.uuid) else {
        return HttpResponse::BadRequest().json(json!({
            "success":"false",
            "message":"Failed to parse uuid to string"
        }));
    };

    let state = state.managers.lock();
    let s_guard = match state {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to lock the state. {e:?}");
            return HttpResponse::BadRequest().json(json!({
                "success":"false",
                "message":"Failed to parse uuid to string"
            }));
        }
    };

    if let Some(s) = s_guard.get(&uuid) {
        let dm = s.download_manager.lock().await;
        if let Err(e) = dm.stop().await {
            return HttpResponse::BadRequest().json(json!({
                "success":"false",
                "message": format!("Failed to stop the download. {:?}", e)
            }));
        };
    };

    return HttpResponse::Ok().json(json!({
        "success": "true",
        "uuid": uuid.to_string()
    }));
}

#[get("/resume")]
async fn resume_download(
    state: web::Data<AppState>,
    query: web::Query<UuidQuery>,
) -> impl Responder {
    let Ok(uuid) = Uuid::parse_str(&query.uuid) else {
        return HttpResponse::BadRequest().json(json!({
            "success":"false",
            "message":"Failed to parse uuid to string"
        }));
    };

    let state = state.managers.lock();
    let s_guard = match state {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to lock the state. {e:?}");
            return HttpResponse::BadRequest().json(json!({
                "success":"false",
                "message":"Failed to parse uuid to string"
            }));
        }
    };

    if let Some(s) = s_guard.get(&uuid) {
        let dm = s.download_manager.lock().await;
        if let Err(e) = dm.resume().await {
            return HttpResponse::BadRequest().json(json!({
                "success":"false",
                "message": format!("Failed to resume the download. {:?}", e)
            }));
        };
    };

    return HttpResponse::Ok().json(json!({
        "success": "true",
        "uuid": uuid.to_string()
    }));
}
