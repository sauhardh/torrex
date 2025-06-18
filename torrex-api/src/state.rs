use torrex_lib::extension::magnet_link::ExtendedMetadataExchange;
use torrex_lib::metainfo;
use torrex_lib::metainfo::TorrentFile;
use uuid::Uuid;

use std::collections::HashMap;
use std::sync::Mutex;

pub enum DownloadKind {
    meta(TorrentFile),
    magnet((ExtendedMetadataExchange, metainfo::Info)),
}

pub struct DownloadState {
    kind: DownloadKind,
}

impl DownloadState {
    pub fn new_meta(meta: TorrentFile) -> Self {
        Self {
            kind: DownloadKind::meta(meta),
        }
    }

    pub fn new_magnet(magnet: (ExtendedMetadataExchange, metainfo::Info)) -> Self {
        Self {
            kind: DownloadKind::magnet(magnet),
        }
    }
}

pub struct AppState {
    pub downloads: Mutex<HashMap<Uuid, DownloadState>>,
}
