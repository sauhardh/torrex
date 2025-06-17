use torrex_lib::extension::magnet_link::ExtendedMetadataExchange;
use torrex_lib::metainfo;
use torrex_lib::metainfo::TorrentFile;
use uuid::Uuid;

use std::collections::HashMap;
use std::sync::Mutex;

pub struct DownloadState {
    pub meta: Option<TorrentFile>,
    pub magnet: Option<(ExtendedMetadataExchange, metainfo::Info)>,
}

impl DownloadState {
    pub fn new(
        meta: Option<TorrentFile>,
        magnet: Option<(ExtendedMetadataExchange, metainfo::Info)>,
    ) -> Self {
        Self { meta, magnet }
    }
}

pub struct AppState {
    pub downloads: Mutex<HashMap<Uuid, DownloadState>>,
}
