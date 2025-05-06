use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

/// Messages to send to a peer. It's to exchange multiple peer messages to download the file.
#[derive(Debug, PartialEq)]
pub enum Messages {
    /// Not uploading to them right now.
    /// Have no payload.
    Choke,
    /// Allowing to upload to them
    /// Have no payload
    Unchoke,
    /// To receive something another peer has.
    /// Have no payload
    Interested,
    /// To not receive something another peer has.
    /// Have no payload
    NotInterested,
    /// Represents pieces that just finished downloading.
    /// Single payload. The index which that downloader just completed and checked the hash of.
    Have(u32),
    /// Represents bitfield showing all the pieces peer has.
    /// `bitfield` is only ever sent as first message.
    /// It's payload is a bitfield which each index that downloader has sent set to one and rest set to zero.
    BitField(Vec<u8>),
    /// Contains `index`, `begin`, `length`. Last two are byte offsets.
    /// Requesting data of `length` bytes from `index`, starting at byte offset `begin`.
    /// Length is generally power of two.
    /// All current implementation use `2^16 (16 KiB)`, and close connections which request an amount greater than that.
    Request { index: u32, begin: u32, length: u32 },
    /// Response to the request.
    /// Contains, `index`, `begin`, and `block`.
    Piece {
        index: u32,
        begin: u32,
        block: Vec<u8>,
    },
    /// Same payload as `request` messages.
    /// They are generally sent towards the end of the download (endgame mode).
    ///
    /// To make sure the last few pieces come in quickly (incase some slow peer is holding it.),
    /// it will send duplicate request to other peers also. After getting the pieces from the "fastest" peers, other peers should be informed
    /// so, `Cancel` message is sent.
    Cancel { index: u32, begin: u32, length: u32 },

    /// Unknown message
    Unknown(u8, Vec<u8>), // Fallback
}

impl Messages {
    pub async fn start_exchange(
        stream: &mut TcpStream,
    ) -> Result<Messages, Box<dyn std::error::Error>> {
        // Read the length (4 bytes) + message ID (1 bytes)
        let mut msg: [u8; 5] = [0u8; 5];

        stream.read_exact(&mut msg).await?;

        let msg_len = u32::from_be_bytes(msg[0..4].try_into()?);
        let msg_id = msg[4];

        if msg_len < 1 {
            return Err("Not Enough Message Length".into());
        }

        let mut payload = vec![0u8; (msg_len - 1) as usize];
        stream.read_exact(&mut payload).await?;

        let message = match msg_id {
            // 'choke', 'unchoke', 'interested', and 'not interested' have no payload.
            0 => Messages::Choke,
            1 => Messages::Unchoke,
            2 => Messages::Interested,
            3 => Messages::NotInterested,
            // Have
            4 => {
                let index = u32::from_be_bytes(payload[..4].try_into()?);
                Messages::Have(index)
            }

            // BitField
            5 => Messages::BitField(payload.clone()),

            // 6 - Request, 7 - Piece, 8 - Cancel
            6 | 7 | 8 => {
                let index = u32::from_be_bytes(payload[0..4].try_into()?);
                let begin = u32::from_be_bytes(payload[4..8].try_into()?);
                let length = u32::from_be_bytes(payload[8..12].try_into()?);

                if msg_id == 6 {
                    Messages::Request {
                        index,
                        begin,
                        length,
                    }
                } else if msg_id == 8 {
                    Messages::Cancel {
                        index,
                        begin,
                        length,
                    }
                } else {
                    let block = payload[8..].to_vec();
                    Messages::Piece {
                        index,
                        begin,
                        block,
                    }
                }
            }
            _ => Messages::Unknown(msg_id, payload.clone()),
        };

        Ok(message)
    }

    /// Sends a interested message of id 2.
    /// Payload for this message is empty
    pub async fn interested(&self, stream: &mut TcpStream) -> &Self {
        let _ = stream.write_all(&[0, 0, 0, 1, 2]).await;

        self
    }

    /// This waits for the `Unchoke` message from the stream.
    /// Only after that we can initiate the `request`.
    pub async fn wait_unchoke(&self, stream: &mut TcpStream) -> Messages {
        Self::start_exchange(stream).await.unwrap()
    }
}
