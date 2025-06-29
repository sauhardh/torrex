use serde_json::value::Value;
use std::{
    collections::{BTreeMap, HashMap},
    time::Duration,
    vec,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

use crate::{
    bencode::Bencode,
    connection::PeerConnection,
    extension::magnet_link::ExtendedHandshake,
    metainfo::{FileKey, Info},
};
use serde::Serialize;

/// The metadata extension supports the following message types.
#[derive(Debug, Serialize, Clone)]
enum MetadataMessageType {
    /// Requests a piece of metadata from the peer
    #[serde(rename = "request")]
    Request { msg_type: u8, piece: u8 },
    /// Sends a piece of metadata to the peer
    #[serde(rename = "data")]
    Data {
        msg_type: u8,
        piece: u8,
        total_size: usize,
    },
    /// Signals that the peer doesn't have the piece of metadata that was requested
    #[serde(rename = "reject")]
    Reject { msg_type: u8, piece: u8 },
}

impl Default for MetadataMessageType {
    fn default() -> Self {
        Self::Request {
            msg_type: 0,
            piece: 0,
        }
    }
}

impl Into<BTreeMap<&'static str, serde_json::value::Value>> for MetadataMessageType {
    fn into(self) -> BTreeMap<&'static str, serde_json::value::Value> {
        let mut map = BTreeMap::new();

        match self {
            MetadataMessageType::Reject { msg_type, piece } => {
                map.insert("msg_type", Value::Number(msg_type.into()));
                map.insert("piece", Value::Number(piece.into()));
            }
            MetadataMessageType::Data {
                msg_type,
                piece,
                total_size,
            } => {
                map.insert("msg_type", Value::Number(msg_type.into()));
                map.insert("piece", Value::Number(piece.into()));
                map.insert("total_size", Value::Number(total_size.into()));
            }
            MetadataMessageType::Request { msg_type, piece } => {
                map.insert("msg_type", Value::Number(msg_type.into()));
                map.insert("piece", Value::Number(piece.into()));
            }
        };

        map
    }
}

#[derive(Debug, Default, Clone)]
pub struct MetadataMessage {
    // peer's metadata extension ID, which is received during the extension handshake.
    extd_msg_id: u8,

    payload: MetadataMessageType,
}

#[derive(Debug, Default, Clone)]
pub struct ExtendedMetadataExchange {
    message_len: u32,
    message_id: u8,
    message: MetadataMessage,
}

impl ExtendedMetadataExchange {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_request(&mut self, piece: Option<u8>) -> &Self {
        self.message.payload = MetadataMessageType::Request {
            msg_type: 0,
            piece: piece.unwrap_or(0),
        };

        self
    }

    pub fn data_response(
        message_len: u32,
        message_id: u8,
        extd_msg_id: u8,
        msg_type: u8,
        piece: u8,
        total_size: usize,
    ) -> ExtendedMetadataExchange {
        ExtendedMetadataExchange {
            message_len,
            message_id,
            message: MetadataMessage {
                extd_msg_id,
                payload: MetadataMessageType::Data {
                    msg_type,
                    piece,
                    total_size,
                },
            },
        }
    }

    /// Id is 20 for all the message implemented by this magnet link extension
    pub fn set_message_id(&mut self, id: Option<u8>) -> &mut Self {
        self.message_id = if id.is_some() { id.unwrap() } else { 20 };

        self
    }
    /// To set peer's metadata extension ID, which is received during the extension handshake
    pub fn set_extension_message_id(&mut self, id: u8) -> &mut Self {
        self.message.extd_msg_id = id;

        self
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut msg: Vec<u8> = Vec::new();
        // message id (1 byte)
        msg.push(self.message_id);

        /*
        payload (variable size)
        */

        // extension message id (1 byte)
        msg.push(self.message.extd_msg_id);

        let payload: BTreeMap<&'static str, Value> = self.message.payload.clone().into();
        let benc = serde_bencode::to_bytes(&payload).unwrap();
        msg.extend_from_slice(&benc);

        let mut final_msg = Vec::new();

        let length = msg.len() as u32;
        final_msg.extend_from_slice(&length.to_be_bytes());
        final_msg.extend_from_slice(&msg);

        final_msg
    }

    pub async fn send_request(
        &self,
        stream: &mut TcpStream,
        value: &[u8],
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
        stream.write_all(&value).await?;

        Ok(())
    }

    pub async fn receive_data(
        self,
        stream: &mut TcpStream,
    ) -> Result<(ExtendedMetadataExchange, Info), Box<dyn std::error::Error + Send + Sync>> {
        // message length prefix (4 bytes)
        let mut len = [0u8; 4];
        stream.read_exact(&mut len).await?;
        let message_len = u32::from_be_bytes(len);

        let mut payload = vec![0u8; message_len as usize];
        stream.read_exact(&mut payload).await?;

        // message id (1 byte)
        let message_id = payload[0];

        // extension id
        let extd_msg_id = payload[1];

        let ben_dict: BTreeMap<String, Value> =
            serde_json::from_value(Bencode::new().decoder(&payload[2..]).0)?;

        let msg_type = ben_dict
            .get("msg_type")
            .and_then(|x| x.as_u64())
            .expect("msg_type missing") as u8;

        let piece = ben_dict
            .get("piece")
            .and_then(|v| v.as_u64())
            .expect("piece missing") as u8;

        let total_size = ben_dict
            .get("total_size")
            .and_then(|v| v.as_u64())
            .expect("total_size missing");

        let metadata = &payload[(payload.len() - total_size as usize)..];
        let metainfo: Info = serde_json::from_value(Bencode::new().decoder(&metadata).0).unwrap();

        Ok((
            Self::data_response(
                message_len,
                message_id,
                extd_msg_id,
                msg_type,
                piece,
                total_size as usize,
            ),
            metainfo,
        ))
    }

    pub async fn handshaking(
        &self,
        ips: Vec<String>,
        info_hash: Vec<u8>,
        peer_id: &String,
    ) -> Option<(ExtendedMetadataExchange, Info)> {
        for addr in ips.clone() {
            if let Ok(mut conn) = PeerConnection::init(addr).await {
                let handshake_result = timeout(
                    Duration::from_secs(10),
                    conn.init_handshaking(info_hash.clone(), peer_id.as_bytes().to_vec()),
                )
                .await;

                match handshake_result {
                    Ok(h) => match h {
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
                                        let mut metadata = ExtendedMetadataExchange::new();
                                        let v = metadata
                                            .set_message_id(Some(20))
                                            .set_extension_message_id(_response.m.ut_metadata)
                                            .set_request(Some(0))
                                            .to_bytes();

                                        if let Err(e) = metadata.send_request(stream, &v).await {
                                            eprintln!(
                                                "Failed to send request for metadata. FurtherMore {:?}",
                                                e
                                            );
                                        };

                                        match metadata.receive_data(stream).await {
                                            Ok(info) => {
                                                let (length, _files) = match &info.1.key {
                                                    FileKey::SingleFile { length } => {
                                                        (Some(length), None)
                                                    }
                                                    FileKey::MultiFile { files } => {
                                                        (None, Some(files))
                                                    }
                                                };

                                                return Some(info);
                                            }
                                            Err(e) => {
                                                eprintln!(
                                                    "Failed to receive metadata. FurtherMore {:?}",
                                                    e
                                                );
                                                continue;
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        eprintln!(
                                            "Failed to send extented handshake. FurtherMore: {e}"
                                        );
                                        continue;
                                    }
                                }
                            }
                        }

                        Err(e) => {
                            eprintln!("Error occured on initiating handshake {e:?} ",);
                            continue;
                        }
                    },
                    Err(e) => {
                        eprintln!("timeout while inital handshaking {e:?} ",);
                        continue;
                    }
                }
            };
        }

        None
    }
}

#[cfg(test)]
mod test_extdmetadataexchange {

    use std::env::temp_dir;

    use crate::connection::connection::SwarmManager;
    use crate::extension::magnet_link::ExtendedExchange;
    use crate::extension::magnet_link::ExtendedMetadataExchange;
    use crate::extension::magnet_link::Parser;
    use crate::metainfo::FileKey;
    use crate::random;

    #[tokio::test]
    async fn test() {
        // let url: String = "magnet:?xt=urn:btih:ad42ce8109f54c99613ce38f9b4d87e70f24a165&dn=magnet1.gif&tr=http%3A%2F%2Fbittorrent-test-tracker.codecrafters.io%2Fannounce".to_string();
        // let url: String = "magnet:?xt=urn:btih:c5fb9894bdaba464811b088d806bdd611ba490af&dn=magnet3.gif&tr=http%3A%2F%2Fbittorrent-test-tracker.codecrafters.io%2Fannounce".to_string();
        // let url: String = "magnet:?xt=urn:btih:4344503b7e797ebf31582327a5baae35b11bda01&dn=ubuntu-16.04-desktop-amd64.iso&tr=http%3A%2F%2Ftorrent.ubuntu.com%3A6969%2Fannounce&tr=http%3A%2F%2Fipv6.torrent.ubuntu.com%3A6969%2Fannounce".to_string();
        // let url: String = "magnet:?xt=urn:btih:cfb0ce978c1c8ec691d76e207f349337bce6efc4&dn=ubuntu-20.04.6-desktop-amd64.iso&tr=http://tracker.renfei.net:8080/announce".to_string();
        // let url: String = "magnet:?xt=urn:btih:B825E41C45159B8F9E5D71955222CE257E2D90D4&dn=The.Phoenician.Scheme.2025.1080p.WEB-DL.DDP5.1.x265-NeoNoir&tr=http://tracker.openbittorrent.com:80/announce".to_string();
        let url: String = "magnet:?xt=urn:btih:f1c3123613342bf5c480324bf55192e5fa9a67c2&dn=MaunaLinux-24.7-MATE-amd64.iso&tr=http://tracker.moxing.party:6969/announce".to_string();

        let mut parser = &mut Parser::new(url);
        parser = parser.parse();

        println!("parser {parser:?}");

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

        println!("ips {ips:?}");

        let self_peer_id = random::generate_magnet_peerid();
        let extd = ExtendedMetadataExchange::new();
        let info = extd
            .handshaking(ips.clone(), info_hash.clone(), &peer_id)
            .await
            .unwrap();

        let (length, _files) = match &info.1.key {
            FileKey::SingleFile { length } => (Some(length), None),
            FileKey::MultiFile { files } => (None, Some(files)),
        };

        let mut sm = SwarmManager::init();
        // sm.connect_and_exchange_bitfield(ips, info_hash.to_vec(), self_peer_id.as_bytes().to_vec())
        //     .await;

        // sm.destination(format!("{}{}", "/tmp/", info.1.name))
        //     .final_peer_msg(
        //         length.unwrap().clone(),
        //         &info.1.pieces_hashes(),
        //         info.1.piece_length,
        //     )
        //     .await;

        let file_size = length.unwrap().clone();
        let piece_length = info.1.piece_length;

        sm.connect_and_exchange_bitfield(
            ips,
            info_hash.to_vec(),
            self_peer_id.as_bytes().to_vec(),
            file_size,
            piece_length,
        )
        .await;

        let path = temp_dir().join("testing.iso").to_string_lossy().to_string();
        sm.destination(path)
            .final_peer_msg(file_size, &info.1.pieces_hashes(), piece_length)
            .await;
    }
}
