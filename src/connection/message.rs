use std::sync::Arc;
// use std::sync::Mutex;

use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::sync::mpsc::Sender;

use crate::cryptography::sha1_hash;

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

    /// This request peers for `pieces` data.
    #[inline]
    async fn request(
        &self,
        stream: &mut TcpStream,
        piece_len: usize,
        file_size: usize,
        tx: Sender<Vec<u8>>,
    ) {
        // Piece user is requesting
        let total_pieces = (file_size + piece_len - 1) / piece_len;
        let block_size = 16_384;

        for index in 0..total_pieces {
            let current_piece_len = if index == total_pieces - 1 {
                file_size - (index * piece_len)
            } else {
                piece_len
            };

            let mut begin = 0;

            while begin < current_piece_len {
                // Note: Length will be `16 * 1024` except for the last blocks
                let length = if begin + block_size <= current_piece_len {
                    block_size
                } else {
                    current_piece_len - begin
                };

                let mut msg: Vec<u8> = Vec::with_capacity(17);
                // Message length: 13 bytes of Payload (index, begin, length) + 1 bytes of message Id
                msg.extend(&13u32.to_be_bytes());
                // Message Id: 6 for `request` message type.
                msg.push(6);
                // Payload: (index, begin, length) each as 4 bytes.
                msg.extend(&(index as u32).to_be_bytes());
                msg.extend(&(begin as u32).to_be_bytes());
                msg.extend(&(length as u32).to_be_bytes());

                if let Err(e) = stream.write_all(&msg).await {
                    eprintln!("Failed to send request: {:?}", e);
                    break;
                }

                let piece = self.wait_pieces(stream).await;

                // Sending This piece data to the receiver.
                let _tx: Sender<Vec<u8>> = tx.clone();
                let _ = tokio::spawn(async move {
                    match piece {
                        Self::Piece {
                            index,
                            begin,
                            block,
                        } => {
                            let _ = _tx.send(block).await;
                        }

                        _ => {
                            eprintln!("Didn't match the type 'Message'.")
                        }
                    };
                })
                .await;

                println!(
                    "Sent request for piece {} begin {} length {}",
                    index, begin, length
                );

                begin += length;
            }
        }
    }

    pub async fn request_msg(&self, piece_len: usize, file_size: usize, stream: &mut TcpStream) {
        // Channel to receive blocks from the request logic
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(16_384);
        let total_value = Arc::new(Mutex::new(Vec::new()));

        // Task to collect all received blocks into `total_value`
        let total_value_clone = Arc::clone(&total_value);
        let collector_handle = tokio::spawn(async move {
            while let Some(block) = rx.recv().await {
                let mut tot = total_value_clone.lock().await;
                tot.extend_from_slice(&block);
            }
        });

        // Start the request logic
        {
            self.request(stream, piece_len, file_size, tx).await;
        }

        let val = sha1_hash(&total_value.lock().await).to_vec();
        println!("val is  {:?}", val);

        // Wait for the collector task to complete
        // This will finish only if the `request` method finishes and `tx` is dropped
        let _ = collector_handle.await;
    }

    /// This waits for pieces message from the peers right after sending `request` message
    pub async fn wait_pieces(&self, stream: &mut TcpStream) -> Messages {
        Self::start_exchange(stream).await.unwrap()
    }
}
