use std::collections::BTreeMap;

use rand::rand_core::le;
use serde::Serialize;

/// The metadata extension supports the following message types.
#[derive(Debug, Serialize)]
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
    pub fn set_message_id(&mut self, id: Option<u8>) -> &Self {
        self.message_id = if id.is_some() { id.unwrap() } else { 20 };

        self
    }
    /// To set peer's metadata extension ID, which is received during the extension handshake
    pub fn set_extension_message_id(&mut self, id: u8) -> &Self {
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

        let benc: String =
            serde_bencode::from_bytes(format!("{:?}", self.message.payload).as_bytes()).unwrap();

        msg.extend_from_slice(benc.as_bytes());

        let mut new_msg = Vec::new();

        let length = msg.len() as u32;
        new_msg.extend_from_slice(&length.to_be_bytes());
        new_msg.extend_from_slice(&msg);

        new_msg
    }
}
