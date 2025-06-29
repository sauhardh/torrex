use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::time::timeout;

use std::collections::HashSet;
use std::fmt::Debug;
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
    /// Extended protocol message
    Extended(u8, Vec<u8>),
    /// Unknown message
    Unknown(u8, Vec<u8>), // Fallback
}

#[derive(Debug, Clone)]
pub struct Messages {
    stream: Arc<Mutex<TcpStream>>,
    pub message_type: MessageType,
}

impl Messages {
    pub fn init(stream: Arc<Mutex<TcpStream>>) -> Self {
        Self {
            stream,
            message_type: MessageType::Unknown(0, vec![]),
        }
    }

    pub async fn exchange_msg(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut msg: [u8; 5] = [0u8; 5];

        let mut stream = self.stream.lock().await;
        stream.read_exact(&mut msg).await?;

        let msg_len = u32::from_be_bytes(msg[0..4].try_into()?);
        let msg_id = msg[4];

        if msg_len == 0 {
            self.message_type = MessageType::Unknown(0, vec![]);
            // return Ok(None);
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

            // Extended protocol (ID 20)
            20 => {
                if payload.len() >= 1 {
                    let ext_id = payload[0];
                    let ext_payload = payload[1..].to_vec();
                    MessageType::Extended(ext_id, ext_payload)
                } else {
                    MessageType::Extended(0, payload)
                }
            }

            _ => MessageType::Unknown(msg_id, payload.clone()),
        };

        // println!("\n_______message {:?}\n", message);
        self.message_type = message;
        // Ok(Some(self))
        Ok(())
    }

    pub async fn wait_have(&mut self) -> Option<u32> {
        if let Err(e) = self.exchange_msg().await {
            eprintln!("Caused an error while waiting for have message. {e}");
        }

        if let MessageType::Have(payload) = &self.message_type {
            return Some(*payload);
        }

        None
    }

    pub async fn wait_bitfield(&mut self) -> Option<HashSet<u32>> {
        if let Err(e) = self.exchange_msg().await {
            eprintln!("Caused an error while waiting bitfield message. {e}");
        };

        if let MessageType::BitField(payload) = &self.message_type {
            let mut pieces_idx = HashSet::new();
            for (byte_idx, byte) in payload.iter().enumerate() {
                for bit in 0..8 {
                    if (byte >> (7 - bit)) & 1 == 1 {
                        pieces_idx.insert((byte_idx * 8 + bit) as u32);
                    }
                }
            }

            return Some(pieces_idx);
        }
        None
    }

    pub async fn interested(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut stream = self.stream.lock().await;
        stream.write_all(&[0, 0, 0, 1, 2]).await?;

        Ok(())
    }

    pub async fn wait_piece_block(&mut self) -> Option<MessageType> {
        if let Ok(result) = timeout(Duration::from_secs(15), async {
            match self.exchange_msg().await {
                Ok(_) => Some(self.message_type.clone()),
                Err(e) => {
                    eprintln!("Error on waiting to receive `Piece` message. FurtherMore: {e}");
                    None
                }
            }
        })
        .await
        {
            result
        } else {
            None
        }
    }

    //
    // --> Commented out as it is not used "right now."
    //
    // / Message Type: request of ID 6.
    // /
    // / Payload consist of `index`, `begin`, `length`
    // #[inline]
    // pub async fn send_request(
    //     &self,
    //     index: u32,
    //     length: usize,
    //     begin: usize,
    // ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    //     let mut msg: Vec<u8> = Vec::with_capacity(17);
    //     // Message length: 13 bytes of Payload (index, begin, length) + 1 bytes of message Id
    //     msg.extend(&13u32.to_be_bytes());
    //     // Message Id: 6 for `request` message type.
    //     msg.push(6);
    //     // Payload: (index, begin, length) each as 4 bytes.
    //     msg.extend(&(index).to_be_bytes());
    //     msg.extend(&(begin as u32).to_be_bytes());
    //     msg.extend(&(length as u32).to_be_bytes());

    //     let mut stream = self.stream.lock().await;
    //     stream.write_all(&msg).await?;

    //     Ok(())
    // }
}
