use serde::Deserialize;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::vec;

use serde::Serialize;

use crate::bencode::Bencode;
use crate::extension::magnet_link::Parser;
use crate::peers::Event;
use crate::peers::Peers;

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct ExtendedHandshakeM {
    #[serde(default)]
    pub ut_metadata: u8,
    ut_pex: Option<u8>,

    #[serde(flatten)]
    pub extra: HashMap<String, serde_bencode::value::Value>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ExtendedHandshakeResponse {
    #[serde(default)]
    pub m: ExtendedHandshakeM,
    metadata_size: Option<u32>,
    reqq: Option<u32>,
    #[serde(default)]
    v: Option<String>,
    #[serde(default, with = "serde_bytes")]
    yourip: Option<Vec<u8>>,

    /// For anyextra filed
    #[serde(flatten)]
    pub extra: HashMap<String, serde_bencode::value::Value>,
}

#[derive(Debug, Default, Serialize)]
struct PayloadDict {
    m: BTreeMap<String, u8>,
}

#[derive(Debug, Default, Serialize)]
struct ExtendedHandshakePayload {
    pub ext_message_id: u8,
    dict: PayloadDict,
}

#[derive(Debug, Default, Serialize)]
pub struct ExtendedHandshake {
    message_length: u32,
    message_id: u8,
    payload: ExtendedHandshakePayload,
}

impl ExtendedHandshake {
    pub fn new() -> Self {
        Self {
            message_length: 0,
            message_id: 20,
            payload: ExtendedHandshakePayload {
                ext_message_id: 0,
                dict: PayloadDict { m: BTreeMap::new() },
            },
        }
    }

    pub fn insert_payload(&mut self, params: HashMap<String, u8>) -> &mut Self {
        let m: BTreeMap<String, u8> = params.into_iter().collect();

        let mut h = BTreeMap::new();
        h.insert("m", m.clone());

        let handshake = PayloadDict { m };
        self.payload.dict = handshake;

        self
    }

    pub fn insert_message_id(&mut self, id: u8) -> &Self {
        self.message_id = id;

        self
    }

    fn insert_message_len(&mut self, len: u32) -> &Self {
        self.message_length = len;

        self
    }

    pub fn to_bytes(&mut self) -> Option<Vec<u8>> {
        // TODO: extend bencode.rs to support for encoding to bencode
        if let Ok(value) = serde_bencode::to_bytes(&self.payload.dict) {
            // self.insert_message_len(1 + value.len() as u32);

            return Some(value);
        }

        None
    }

    pub fn to_full_message(&mut self) -> Vec<u8> {
        /*
        message_id is already set to 20 in `new()` associated function.
        self.insert_message_id(20);
        */

        // Calling self.to_bytes() will also set message_len() to self;
        let bencoded_dict = self.to_bytes().unwrap();
        self.insert_message_len(1 + bencoded_dict.len() as u32 + 1);

        let mut message = Vec::new();

        // message length(4 bytes)
        message.extend_from_slice(&self.message_length.to_be_bytes());

        // message_id (1 byte)
        message.push(self.message_id);

        /*
        Now For payload (Varaible size)
        */

        // extension message id of size 1 byte
        message.push(self.payload.ext_message_id);
        // bencoded dictionary of varaible size
        message.extend_from_slice(&bencoded_dict);

        message
    }

    pub async fn extd_receive_handshake(
        &mut self,
        stream: &mut TcpStream,
    ) -> Result<ExtendedHandshakeResponse, Box<dyn std::error::Error>> {
        // For magnet link and selected tracker, we need this to uncomment

        // let mut bitfield_buf = [0u8; 6];
        // stream.read_exact(&mut bitfield_buf).await?;

        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).await?;
        let len = u32::from_be_bytes(len_buf);

        let mut payload = vec![0u8; len as usize];
        stream.read_exact(&mut payload).await?;

        if payload[0] != 20 || payload[1] != 0 {
            return Err("Not an extended handshake".into());
        }

        let bencoded_data = &payload[2..];
        match serde_json::from_value::<ExtendedHandshakeResponse>(
            Bencode::new().decoder(bencoded_data).0,
        ) {
            Ok(res) => {
                return Ok(res);
            }
            Err(e) => {
                return Err(format!(
                    "Failed to parse and handle receiving extended handshake data. FurtherMore {e:?}"
                )
                .into());
            }
        }
        // let response: ExtendedHandshakeResponse = serde_bencode::from_bytes(bencoded_data)?;
        // Ok(response)
    }

    pub async fn extd_init_handshaking(
        &mut self,
        stream: &mut TcpStream,
        params: HashMap<String, u8>,
    ) -> Result<ExtendedHandshakeResponse, Box<dyn std::error::Error>> {
        let message = self.insert_payload(params).to_full_message();
        stream.write_all(&message).await?;

        let res = self.extd_receive_handshake(stream).await;
        res
    }
}

#[derive(Debug)]
pub struct ExtendedExchange<'a> {
    pub parser: &'a mut Parser,
    peers: Peers,
}

impl<'a> ExtendedExchange<'a> {
    pub fn new(parser: &'a mut Parser) -> Self {
        Self {
            parser,
            peers: Peers::default(),
        }
    }

    pub fn set_request(
        &mut self,
        info_hash: Vec<u8>,
        peer_id: String,
        ip: Option<String>,
        port: usize,
        uploaded: usize,
        downloaded: usize,
        left: usize,
        event: Option<Event>,
        compact: u32,
    ) -> &mut Self {
        let req = self.peers.request.new(
            info_hash.clone(),
            peer_id.clone(), // random string
            ip,
            port,
            uploaded,
            downloaded,
            left,
            event,
            compact,
        );

        self.peers.request = req;

        self
    }

    pub async fn request_tracker(&mut self) -> &Self {
        let request = self.peers.request.clone();
        self.peers.request_tracker(&request).await;

        self
    }

    pub fn set_url(&mut self, url: String) -> &mut Self {
        self.peers.announce_url(url);

        self
    }

    pub fn peers_ip(&self) -> Vec<String> {
        self.peers
            .response
            .as_ref()
            .map(|r| r.peers_ip())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod test_extdhandshake {

    use super::*;

    use crate::connection::connection::PeerConnection;
    use crate::extension::magnet_link::Parser;
    use crate::random;

    #[tokio::test]
    async fn test() {
        let url: String = "magnet:?xt=urn:btih:3f994a835e090238873498636b98a3e78d1c34ca&dn=magnet2.gif&tr=http%3A%2F%2Fbittorrent-test-tracker.codecrafters.io%2Fannounce".to_string();
        // let url: String = "magnet:?xt=urn:btih:4344503b7e797ebf31582327a5baae35b11bda01&dn=ubuntu-16.04-desktop-amd64.iso&tr=http%3A%2F%2Ftorrent.ubuntu.com%3A6969%2Fannounce&tr=http%3A%2F%2Fipv6.torrent.ubuntu.com%3A6969%2Fannounce".to_string();
        // let url: String = "magnet:?xt=urn:btih:f1c3123613342bf5c480324bf55192e5fa9a67c2&dn=MaunaLinux-24.7-MATE-amd64.iso&tr=http://tracker.moxing.party:6969/announce".to_string();

        let mut parser = &mut Parser::new(url);
        parser = parser.parse();

        let info_hash = &parser.magnet_link.xt.clone();
        let announce_url = &parser
            .magnet_link
            .tr
            .clone()
            .expect("Tracker URL (tr=...) missing in magnet link");

        let mut extd_exchange = ExtendedExchange::new(parser);

        let info_hash = hex::decode(info_hash).expect("Could not decode provided info_hash");
        let peer_id = random::generate_magnet_peerid();

        let ips = extd_exchange
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

        for addr in ips {
            if let Ok(mut conn) = PeerConnection::init(addr).await {
                match conn
                    .init_handshaking(info_hash.clone(), peer_id.as_bytes().to_vec())
                    .await
                {
                    Ok(handshake) => {
                        if handshake.is_magnet_supported() {
                            let mut params: HashMap<String, u8> = HashMap::new();
                            params.insert("ut_metadata".to_string(), 16);

                            let stream = &mut conn.stream.lock().await;

                            match ExtendedHandshake::new()
                                .extd_init_handshaking(stream, params)
                                .await
                            {
                                Ok(_response) => {
                                    println!("Peer ID: {}", handshake.remote_peer_id());
                                    println!(
                                        "Peer Metadata Extension ID: {:?}",
                                        _response.m.ut_metadata
                                    );
                                }
                                Err(e) => {
                                    eprintln!("Failed to send extented handshake. FurtherMore: {e}")
                                }
                            }
                        }
                    }

                    Err(e) => {
                        eprintln!("Error occured on initiating handshake {e:?} ",);
                        return;
                    }
                }
            };
        }
    }

    // #[test]
    fn tester() {
        let mut params: HashMap<String, u8> = HashMap::new();
        params.insert("ut_metadata".to_string(), 16);

        ExtendedHandshake::new().insert_payload(params).to_bytes();
    }
}
