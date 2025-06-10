use serde_json::value::Value;
use std::collections::BTreeMap;
use tokio::{io::AsyncWriteExt, net::TcpStream};

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

#[derive(Debug, Default)]
pub struct MetadataMessage {
    // peer's metadata extension ID, which is received during the extension handshake.
    extd_msg_id: u8,

    payload: MetadataMessageType,
}

#[derive(Debug, Default)]
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
        value: &Vec<u8>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        stream.write_all(&value).await?;

        Ok(())
    }
}

#[cfg(test)]
mod test_extdmetadataexchange {
    use std::collections::HashMap;

    use crate::connection::PeerConnection;
    use crate::extension::magnet_link::ExtendedExchange;
    use crate::extension::magnet_link::ExtendedHandshake;
    use crate::extension::magnet_link::ExtendedMetadataExchange;
    use crate::extension::magnet_link::Parser;
    use crate::random;

    #[tokio::test]
    async fn test() {
        let url: String = "magnet:?xt=urn:btih:3f994a835e090238873498636b98a3e78d1c34ca&dn=magnet2.gif&tr=http%3A%2F%2Fbittorrent-test-tracker.codecrafters.io%2Fannounce".to_string();
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
                                    let mut metadata = ExtendedMetadataExchange::new();
                                    let v = metadata
                                        .set_message_id(Some(20))
                                        .set_extension_message_id(0)
                                        .set_request(Some(0))
                                        .to_bytes();

                                    if let Err(e) = metadata.send_request(stream, &v).await {
                                        eprintln!(
                                            "Failed to send request for metadata. FurtherMore {:?}",
                                            e
                                        );
                                    };
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
}
