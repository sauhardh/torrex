use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::time::timeout;

use std::collections::HashSet;
use std::fmt::Debug;
use std::pin::Pin;
use std::process::Output;
use std::sync::Arc;
use std::time::Duration;
use std::vec;

/// Messages to send to a peer. It's to exchange multiple peer messages to download the file.
#[derive(Debug, PartialEq, Clone)]
pub enum MessageType {
    /// Not uploading to them right now.
    ///
    /// Has no payload.
    Choke,
    /// Allowing to upload to them.
    ///
    /// Has no payload
    Unchoke,
    /// To receive something another peer has.
    ///
    /// Has no payload
    Interested,
    /// To not receive something another peer has.
    ///
    /// Has no payload
    NotInterested,
    /// Represents pieces that just finished downloading.
    ///
    /// Single payload. The index which that downloader just completed and checked the hash of.
    Have(u32),
    /// Represents bitfield showing all the pieces peer has.
    ///
    /// `bitfield` is only ever sent as first message.
    ///
    /// It's payload is a bitfield which each index that downloader has sent set to one and rest set to zero.
    BitField(Vec<u8>),
    /// Contains `index`, `begin`, `length`. Last two are byte offsets.
    ///
    /// Requesting data of `length` bytes from `index`, starting at byte offset `begin`.
    ///
    /// Length is generally power of two.
    ///
    /// All current implementation use `2^16 (16 KiB)`, and close connections which request an amount greater than that.
    Request { index: u32, begin: u32, length: u32 },
    /// Response to the request.
    ///
    /// Contains, `index`, `begin`, and `block`.
    Piece {
        index: u32,
        begin: u32,
        block: Vec<u8>,
    },
    /// Same payload as `request` messages.
    ///
    /// They are generally sent towards the end of the download (endgame mode).
    /// To make sure the last few pieces come in quickly (incase some slow peer is holding it.),
    /// it will send duplicate request to other peers also. After getting the pieces from the "fastest" peers, other peers should be informed
    /// so, `Cancel` message is sent.
    Cancel { index: u32, begin: u32, length: u32 },
    /// Unknown message
    Unknown(u8, Vec<u8>), // Fallback
}

#[derive(Debug)]
pub struct Messages {
    stream: Arc<Mutex<TcpStream>>,
    message_type: MessageType,
}

impl Messages {
    pub fn init(stream: Arc<Mutex<TcpStream>>) -> Self {
        Self {
            stream,
            message_type: MessageType::Unknown(0, vec![]),
        }
    }

    pub async fn exchange_msg(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let mut msg: [u8; 5] = [0u8; 5];

        let mut stream = self.stream.lock().await;
        stream.read_exact(&mut msg).await?;

        let msg_len = u32::from_be_bytes(msg[0..4].try_into()?);
        let msg_id = msg[4];

        if msg_len == 0 {
            self.message_type = MessageType::Unknown(0, vec![]);
            return Ok(());
        }
        let mut payload = vec![0u8; (msg_len - 1) as usize];
        stream.read_exact(&mut payload).await?;

        let message = match msg_id {
            // 'choke', 'unchoke', 'interested', and 'not interested' has no payload.
            0 => MessageType::Choke,
            1 => MessageType::Unchoke,
            2 => MessageType::Interested,
            3 => MessageType::NotInterested,
            // Have
            4 => {
                let index = u32::from_be_bytes(payload[..4].try_into()?);
                MessageType::Have(index)
            }

            // BitField
            5 => MessageType::BitField(payload.clone()),

            // 6 - Request, 7 - Piece, 8 - Cancel
            6 => {
                let index = u32::from_be_bytes(payload[0..4].try_into()?);
                let begin = u32::from_be_bytes(payload[4..8].try_into()?);
                let length = u32::from_be_bytes(payload[8..12].try_into()?);

                MessageType::Request {
                    index,
                    begin,
                    length,
                }
            }
            7 => {
                let index = u32::from_be_bytes(payload[0..4].try_into()?);
                let begin = u32::from_be_bytes(payload[4..8].try_into()?);
                let block = payload[8..].to_vec();

                MessageType::Piece {
                    index,
                    begin,
                    block,
                }
            }
            8 => {
                let index = u32::from_be_bytes(payload[0..4].try_into()?);
                let begin = u32::from_be_bytes(payload[4..8].try_into()?);
                let length = u32::from_be_bytes(payload[8..12].try_into()?);

                MessageType::Cancel {
                    index,
                    begin,
                    length,
                }
            }

            _ => MessageType::Unknown(msg_id, payload.clone()),
        };

        self.message_type = message;
        Ok(())
    }

    pub async fn wait_bitfield(&mut self) -> Option<HashSet<u32>> {
        if let Err(e) = self.exchange_msg().await {
            eprintln!("Caused an error while waiting bitfield message. {e}");
        };

        if let MessageType::BitField(payload) = &self.message_type {
            let bin = payload
                .into_iter()
                .map(|b| format!("{:08b}", b))
                .collect::<String>();

            let mut pieces_idx = HashSet::new();

            for (idx, val) in bin.split("").into_iter().enumerate() {
                if val == "1" {
                    pieces_idx.insert(idx as u32 - 1);
                }
            }

            return Some(pieces_idx);
        }
        None
    }

    pub async fn interested(&mut self) -> &Self {
        let mut stream = self.stream.lock().await;
        let _ = stream.write_all(&[0, 0, 0, 1, 2]).await;

        self
    }

    pub async fn wait_unchoke<'a>(&'a mut self) -> bool {
        match timeout(Duration::from_secs(7), self.exchange_msg()).await {
            Ok(Ok(_)) if self.message_type == MessageType::Unchoke => true,
            Ok(Ok(_)) => false,

            Err(_) | Ok(Err(_)) => {
                eprintln!(" Error on waiting to receive `Unchoke` message within time");
                false
            }
        }
    }

    pub async fn wait_piece_block(&mut self) -> Option<MessageType> {
        match self.exchange_msg().await {
            Ok(_) => Some(self.message_type.clone()),
            Err(e) => {
                eprintln!("Error on waiting to receive `Piece` message. FurtherMore: {e}");
                None
            }
        }
    }

    /// Message Type: request of ID 6.
    ///  
    /// Payload consist of `index`, `begin`, `length`
    #[inline]
    pub async fn send_request(
        &self,
        index: u32,
        length: usize,
        begin: usize,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut msg: Vec<u8> = Vec::with_capacity(17);
        // Message length: 13 bytes of Payload (index, begin, length) + 1 bytes of message Id
        msg.extend(&13u32.to_be_bytes());
        // Message Id: 6 for `request` message type.
        msg.push(6);
        // Payload: (index, begin, length) each as 4 bytes.
        msg.extend(&(index).to_be_bytes());
        msg.extend(&(begin as u32).to_be_bytes());
        msg.extend(&(length as u32).to_be_bytes());

        let mut stream = self.stream.lock().await;
        stream.write_all(&msg).await?;

        Ok(())
    }
}
