use reqwest;
use serde::{Deserialize, Serialize};
use serde_bytes::ByteBuf;

use std::borrow::Cow;

use crate::utils::random;

#[derive(Debug, Serialize, Clone)]
pub enum Event {
    Started,
    Completed,
    Stopped,
    /// This is same as not being present.
    Empty,
}

#[derive(Default, Debug, Serialize, Clone)]
pub struct RequestParams {
    /// **(required)**. URL encoded info hash of the torrent, 20 bytes long.
    pub info_hash: Vec<u8>,
    /// **(required)**. Each downloader generates its own id (string of length 20) at random at the start of a new download.
    /// This value will also almost certainly have to be escaped.
    pub peer_id: String,
    /// **(optional)**. An optional parameter giving the IP (or dns name) which this peer is at.
    /// Generally used for the origin if it's on the same machine as the tracker.
    ip: Option<String>,
    /// **(required)**.The port number this peer is listening on.
    /// Common behavior is for a downloader to try to listen on port 6881 and if that port is taken try 6882, then 6883, etc. and give up after 6889.
    port: usize,
    /// **(required)**. The total amount uploaded so far, encoded in base ten ascii.
    uploaded: usize,
    /// **(required)**. The total amount downloaded so far, encoded in base ten ascii.
    downloaded: usize,
    /// **(required)**. The number of bytes this peer still has to download, encoded in base ten ascii.
    /// Note that this can't be computed from downloaded and the file length since it might be a resume,
    /// and there's a chance that some of the downloaded data failed an integrity check and had to be re-downloaded.
    left: usize,
    /// **(optional)** This is an optional key which maps to started, completed, or stopped (or empty, which is the same as not being present).
    /// If not present, this is one of the announcements done at regular intervals.
    /// No completed is sent if the file was complete when started. Downloaders send an announcement using stopped when they cease downloading.
    event: Option<Event>,
    /// **(required)**. The compact format uses the same peers key in the bencoded tracker response,
    /// but the value is a bencoded string rather than a bencoded list.
    /// compact=0 means client prefers original format, and compact=1 means client prefers compact format.
    compact: u32,
}

impl RequestParams {
    pub fn new(
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
        Self {
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

#[derive(Debug, Deserialize, Clone)]
pub struct PeerDict {
    pub ip: String,
    #[serde(rename = "peer id", default)]
    pub peer_id: Option<ByteBuf>, // Changed to ByteBuf to handle binary data
    pub port: u16,
}

#[derive(Debug, Clone)]
pub enum PeersField {
    Compact(ByteBuf),
    Dict(Vec<PeerDict>),
}

impl Default for PeersField {
    fn default() -> Self {
        PeersField::Compact(ByteBuf::new())
    }
}

use serde::de::{self, Visitor};
use std::fmt;

impl<'de> Deserialize<'de> for PeersField {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        struct PeersVisitor;

        impl<'de> Visitor<'de> for PeersVisitor {
            type Value = PeersField;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("Compact binary peer list or list of dictionaries")
            }

            fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(PeersField::Compact(ByteBuf::from(v)))
            }

            fn visit_byte_buf<E>(self, v: Vec<u8>) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(PeersField::Compact(ByteBuf::from(v)))
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(PeersField::Compact(ByteBuf::from(v.as_bytes())))
            }

            fn visit_seq<A>(self, seq: A) -> Result<Self::Value, A::Error>
            where
                A: de::SeqAccess<'de>,
            {
                let peers = Deserialize::deserialize(de::value::SeqAccessDeserializer::new(seq))?;
                Ok(PeersField::Dict(peers))
            }
        }

        deserializer.deserialize_any(PeersVisitor)
    }
}

#[derive(Debug, Default, Deserialize, Clone)]
pub struct ResponseParams {
    /// Number of peers who have finished downloading
    #[serde(default)]
    pub complete: u32,
    /// Number of peers who are still downloading (leechers)
    #[serde(default)]
    pub incomplete: u32,
    /// Minimum seconds to wait before recontacting the tracker.
    #[serde(rename = "min interval", default)]
    pub min_interval: u32,
    /// An integer, indicating how often your client should make a request to the tracker.
    #[serde(default)]
    pub interval: u32,
    /// Dictionary which contains list of peers that your client can connect to.
    /// Each peer is represented using 6 bytes.
    /// The first 4 bytes are the peer's IP address and the last 2 bytes are the peer's port number.
    #[serde(default)]
    pub peers: PeersField,
}

impl ResponseParams {
    pub fn peers_ip(&self) -> Vec<String> {
        match &self.peers {
            PeersField::Compact(buf) => buf
                .as_ref()
                .chunks_exact(6)
                .map(|chunk| {
                    let (ip_bytes, port_bytes) = chunk.split_at(4);
                    let port: u16 = ((port_bytes[0] as u16) << 8) | port_bytes[1] as u16;

                    let ip_addr = ip_bytes
                        .iter()
                        .map(|byte| byte.to_string())
                        .collect::<Vec<_>>()
                        .join(".");

                    format!("{ip_addr}:{port}")
                })
                .collect(),

            PeersField::Dict(list) => list
                .iter()
                .map(|p| format!("{}:{}", p.ip, p.port))
                .collect(),
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct Peers {
    /// URL to request tracker for information.
    announce_url: String,
    pub request: RequestParams,
    pub response: Option<ResponseParams>,
}

impl Peers {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn request_tracker(&mut self, params: &RequestParams) -> &Self {
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
            let event_str = match event {
                Event::Started => "started",
                Event::Completed => "completed",
                Event::Stopped => "stopped",
                Event::Empty => "",
            };
            if !event_str.is_empty() {
                url.query_pairs_mut().append_pair("event", event_str);
            }
        }

        let res_body = match client.get(url).send().await {
            Ok(res) => match res.bytes().await {
                Ok(bytes) => Some(bytes),
                Err(e) => {
                    eprintln!("Failed to read response body: {e}");
                    None
                }
            },
            Err(e) => {
                eprintln!(
                    "Failed to get url requested. Currently it only supports for HTTP connection. Please check your url again: {e}"
                );
                None
            }
        };

        if let Some(body) = res_body {
            match serde_bencode::from_bytes::<ResponseParams>(&body) {
                Ok(parsed) => {
                    self.response = Some(parsed);
                }
                Err(e) => {
                    eprintln!(
                        "Failed to parse response. Error: {e}. Got: Raw response: {}",
                        String::from_utf8_lossy(&body)
                    );

                    if let Ok(debug_value) =
                        serde_bencode::from_bytes::<serde_bencode::value::Value>(&body)
                    {
                        eprintln!("Debug parsed structure: {:#?}", debug_value);
                    }
                }
            }
        } else {
            eprintln!("Tracker request failed, response body is empty");
        }

        self
    }

    pub fn announce_url(&mut self, url: String) -> &mut Self {
        self.announce_url = url;

        self
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
        let meta: TorrentFile = TorrentFile::new();
        let encoded_data = meta.read_file(Path::new("./sample.torrent")).unwrap();
        let meta: TorrentFile = meta.parse_metafile(&encoded_data);

        let (_length, _files) = match &meta.info.key {
            FileKey::SingleFile { length } => (Some(length), None),
            FileKey::MultiFile { files } => (None, Some(files)),
        };
        let info_hash = meta.info_hash(&encoded_data).unwrap();

        // Discovering peers
        let mut peers = Peers::new();
        let params = &peers.request.new(
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

        let ip_addr = peers
            .announce_url(meta.announce)
            .request_tracker(params)
            .await
            .response
            .as_ref()
            .unwrap()
            .peers_ip();

        println!("ip_adrr:{:?}", ip_addr);
    }
}
