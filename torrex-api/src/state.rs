use uuid::Uuid;

use std::collections::HashMap;
use std::sync::Mutex;

use torrex_lib::extension::magnet_link::ExtendedMetadataExchange;
use torrex_lib::metainfo;
use torrex_lib::metainfo::TorrentFile;

#[derive(Clone, Debug)]
pub enum DownloadKind {
    Meta(TorrentFile),
    Magnet((ExtendedMetadataExchange, metainfo::Info)),
}

#[derive(Debug)]
pub struct DownloadState {
    pub kind: DownloadKind,
    pub info_hash: Vec<u8>,
    pub self_peer_id: String,
    pub ips: Vec<String>,
    pub length: usize,
}

impl DownloadState {
    pub fn new_meta(
        meta: TorrentFile,
        info_hash: Vec<u8>,
        self_peer_id: String,
        ips: Vec<String>,
        length: usize,
    ) -> Self {
        Self {
            kind: DownloadKind::Meta(meta),
            info_hash,
            self_peer_id,
            ips,
            length,
        }
    }

    pub fn new_magnet(
        magnet: (ExtendedMetadataExchange, metainfo::Info),
        info_hash: Vec<u8>,
        self_peer_id: String,
        ips: Vec<String>,
        length: usize,
    ) -> Self {
        Self {
            kind: DownloadKind::Magnet(magnet),
            info_hash,
            self_peer_id,
            ips,
            length,
        }
    }
}

#[derive(Debug)]
pub struct AppState {
    pub downloads: Mutex<HashMap<Uuid, DownloadState>>,
}
