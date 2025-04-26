use std::borrow::Cow;

use reqwest;
use serde::Serialize;
use sha1::digest::generic_array::GenericArray;
use sha1::digest::generic_array::typenum::U20;
use urlencoding::encode_binary; // 0.8

use crate::metainfo::TorrentFile;
use crate::random;

#[derive(Debug, Serialize)]
enum Event {
    started,
    completed,
    stopped,
    empty, // This is same as not being present.
}

#[derive(Default, Debug, Serialize)]
struct RequestParams {
    /// URL encoded info hash of the torrent, 20 bytes long.
    info_hash: Vec<u8>,
    /// Each downloader generates its own id (string of length 20) at random at the start of a new download.
    /// This value will also almost certainly have to be escaped.
    peer_id: String,
    /// **(optional)** An optional parameter giving the IP (or dns name) which this peer is at.
    /// Generally used for the origin if it's on the same machine as the tracker.
    ip: Option<String>,
    /// The port number this peer is listening on.
    /// Common behavior is for a downloader to try to listen on port 6881 and if that port is taken try 6882, then 6883, etc. and give up after 6889.
    port: usize,
    /// The total amount uploaded so far, encoded in base ten ascii.
    uploaded: usize,
    /// The total amount downloaded so far, encoded in base ten ascii.
    downloaded: usize,
    /// The number of bytes this peer still has to download, encoded in base ten ascii.
    /// Note that this can't be computed from downloaded and the file length since it might be a resume,
    /// and there's a chance that some of the downloaded data failed an integrity check and had to be re-downloaded.
    left: usize,
    /// **(optional)** This is an optional key which maps to started, completed, or stopped (or empty, which is the same as not being present).
    /// If not present, this is one of the announcements done at regular intervals.
    /// No completed is sent if the file was complete when started. Downloaders send an announcement using stopped when they cease downloading.
    event: Option<Event>,
    /// The compact format uses the same peers key in the bencoded tracker response,
    /// but the value is a bencoded string rather than a bencoded list.
    /// compact=0 means client prefers original format, and compact=1 means client prefers compact format.
    compact: u32,
}

impl RequestParams {
    pub fn build(
        &self,
        info_hash: Vec<u8>,
        peer_id: String,
        ip: Option<String>,
        port: usize,
        uploaded: usize,
        downloaded: usize,
        left: usize,
        event: Option<Event>,
        compact: u32,
    ) -> Self {
        // let infohash = encode_binary(&info_hash);
        Self {
            // info_hash: infohash.to_string(),
            info_hash,
            peer_id,
            ip,
            port,
            uploaded,
            downloaded,
            left,
            event,
            compact,
        }
    }
}

#[derive(Debug, Default)]
struct ResponseParams {
    /// An integer, indicating how often your client should make a request to the tracker.
    interval: u32,
    /// A string, which contains list of peers that your client can connect to.
    /// Each peer is represented using 6 bytes.
    /// The first 4 bytes are the peer's IP address and the last 2 bytes are the peer's port number.
    peers: String,
}

#[derive(Debug, Default)]
struct Peers {
    // URL itself,
    announce_url: String,
    request: RequestParams,
    response: ResponseParams,
}

impl Peers {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn request_tracker(&mut self, params: &RequestParams) {
        let client = reqwest::Client::new();
        let mut url = reqwest::Url::parse(&self.announce_url).unwrap();

        url.query_pairs_mut()
            .encoding_override(Some(&|input| {
                if input == "{{info_hash}}" {
                    Cow::Owned(params.info_hash.clone())
                } else {
                    Cow::Borrowed(input.as_bytes())
                }
            }))
            .append_pair("info_hash", "{{info_hash}}")
            .append_pair("peer_id", &params.peer_id)
            .append_pair("port", &params.port.to_string())
            .append_pair("uploaded", &params.uploaded.to_string())
            .append_pair("downloaded", &params.downloaded.to_string())
            .append_pair("left", &params.left.to_string())
            .append_pair("compact", &params.compact.to_string());

        if let Some(event) = &params.event {
            url.query_pairs_mut()
                .append_pair("event", &format!("{:?}", event));
        }

        let response = client.get(url).send().await.unwrap();
        let body = response.bytes().await.unwrap();
        println!("Got: {body:?}");
    }
}

#[cfg(test)]
mod test_peers {
    use super::*;

    use crate::metainfo::FileKey;
    use crate::metainfo::TorrentFile;
    use std::path::Path;

    #[tokio::test]
    async fn peers() {
        let torr_file = TorrentFile::new();
        let encoded_data = torr_file.read_file(Path::new("./sample.torrent")).unwrap();

        let (pieces_data, pieces_idx) = torr_file.parse_pieces(&encoded_data).unwrap();

        let meta_info: TorrentFile =
            torr_file.parse_metafile(&encoded_data, &pieces_data.to_vec(), pieces_idx);

        let info = meta_info.info_parser(&encoded_data).unwrap();
        let info_hash = meta_info.sha1_hash(info);

        let (_length, _files) = match &meta_info.info.key {
            FileKey::SingleFile { length } => (Some(length), None),
            FileKey::MultiFile { files } => (None, Some(files)),
        };

        let mut peers = Peers::new();

        // Discovering peers
        let params = &peers.request.build(
            info_hash.to_vec(),
            random::generate_peerid(), // random string
            None,
            6881,
            0,
            0,
            *_length.unwrap(),
            None,
            1,
        );

        peers.announce_url = meta_info.announce;
        peers.request_tracker(params).await;
    }
}
