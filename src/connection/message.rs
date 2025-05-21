use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

use std::collections::HashMap;
use std::vec;

use crate::cryptography::sha1_hash;

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
    message_type: MessageType,
    save_to: String,
    requested_pieces: Option<u32>,
}

impl Messages {
    pub fn init(message_type: MessageType, save_to: String, requested_pieces: Option<u32>) -> Self {
        Self {
            message_type,
            save_to,
            requested_pieces,
        }
    }
    async fn enter_message_type(&mut self, messages: MessageType) {
        self.message_type = messages;
    }

    /// This reads message from the stream. At first it reads message of 5 bytes Having length of 4 bytes and Message ID 1 bytes.
    ///
    /// Then, Payload of dynamic size is read having size of message length received in earlier read.
    ///
    /// Returns Message type with payload if it has.
    pub async fn start_exchange(
        &mut self,
        stream: &mut TcpStream,
    ) -> Result<&mut Messages, Box<dyn std::error::Error>> {
        // Read the length (4 bytes) + message ID (1 bytes)
        let mut msg: [u8; 5] = [0u8; 5];

        stream.read_exact(&mut msg).await?;

        let msg_len = u32::from_be_bytes(msg[0..4].try_into()?);
        let msg_id = msg[4];

        if msg_len == 0 {
            self.message_type = MessageType::Unknown(0, vec![]);
            return Ok(self);
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
        Ok(self)
    }

    /// Sends a interested message of `id 2`.
    ///
    /// Payload for this message is empty
    pub async fn interested(&self, stream: &mut TcpStream) -> &Self {
        let _ = stream.write_all(&[0, 0, 0, 1, 2]).await;

        self
    }

    /// This waits for the `Unchoke` message from the stream.
    /// Only after that we can initiate the `request`.
    ///
    /// Internally It calls `start_exchange` function. This returns of type `Message`.
    // pub async fn wait_unchoke(&self, stream: &mut TcpStream) -> Messages {
    pub async fn wait_unchoke(&mut self, stream: &mut TcpStream) -> MessageType {
        match self.start_exchange(stream).await {
            Ok(msg) => msg.message_type.clone(),
            Err(e) => {
                eprintln!("Failed to receive message: {:?}", e);
                return MessageType::Unknown(0, vec![]);
            }
        }
    }

    /// Sends `Request` Message to the peer. It has Message `Id` of 6. And payload having `index`, `begin` and `length`.
    #[inline]
    async fn send_request_msg(
        &self,
        stream: &mut TcpStream,
        index: u32,
        begin: u32,
        length: u32,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Message Id for `request` is 6
        // Payload consist of `index`, `begin`, `length`
        let mut msg: Vec<u8> = Vec::with_capacity(17);
        // Message length: 13 bytes of Payload (index, begin, length) + 1 bytes of message Id
        msg.extend(&13u32.to_be_bytes());
        // Message Id: 6 for `request` message type.
        msg.push(6);
        // Payload: (index, begin, length) each as 4 bytes.
        msg.extend(&(index as u32).to_be_bytes());
        msg.extend(&(begin as u32).to_be_bytes());
        msg.extend(&(length as u32).to_be_bytes());

        // Sending the request message
        stream.write_all(&msg).await?;

        Ok(())
    }

    /// For requesting message and waiting for pieces. Internally, it calls function of particular tasks like sending request and waiting for pieces.
    ///
    /// This compiles the functionality as such: request message -> wait for pieces blocks -> combine blocks to form pieces -> check integrity -> make/append data to file.
    pub async fn request_and_receive_pieces(
        &mut self,
        stream: &mut TcpStream,
        piece_len: usize,
        file_size: usize,
        pieces: &Vec<String>,
        file_path: &String,
        requested_piece_idx: Option<u32>,
    ) {
        // Piece user is requesting
        let total_pieces = (file_size + piece_len - 1) / piece_len;
        let block_size = 16_384;

        let mut mapped_block: HashMap<u32, Vec<Vec<u8>>> = HashMap::new();
        let mut received_block_size: HashMap<u32, usize> = HashMap::new();

        for index in 0..total_pieces {
            // Just if incase there is explicitly request for certain piece_idx
            if requested_piece_idx.is_some_and(|x| x as usize != index) {
                continue;
            }

            let actual_piece_len = if index == total_pieces - 1 {
                file_size - (index * piece_len)
            } else {
                piece_len
            };

            let mut piece_begin_at = 0;
            while piece_begin_at < actual_piece_len {
                // Note: Length will be `16 * 1024` except for the last blocks
                let length = if piece_begin_at + block_size <= actual_piece_len {
                    block_size
                } else {
                    actual_piece_len - piece_begin_at
                };

                // To send the `request` block to the peer.
                let _ = self
                    .send_request_msg(stream, index as u32, piece_begin_at as u32, length as u32)
                    .await;
                let piece = self.wait_pieces(stream).await;

                match piece {
                    MessageType::Piece {
                        index,
                        begin: _,
                        block,
                    } => {
                        // It check for integrity of the blocks received
                        mapped_block.entry(index).or_default().push(block);
                        *received_block_size.entry(index).or_insert(0) += length;

                        if *received_block_size.get(&index).unwrap() >= actual_piece_len {
                            let piece_blocks = mapped_block.get(&index).unwrap().concat();

                            mapped_block.remove(&index);

                            if self.check_integrity(&piece_blocks, pieces, index as usize) {
                                let mut file = File::options()
                                    .create(true)
                                    .write(true)
                                    .open(&file_path)
                                    .await
                                    .unwrap();

                                if let Err(e) = file.write_all(&piece_blocks).await {
                                    eprintln!("Failed to write piece to file: {:?}", e);
                                }
                            }
                        }
                    }
                    _ => {
                        eprintln!("Didn't match the type 'Message'.")
                    }
                };

                piece_begin_at += length;
            }
        }
    }

    /// This checks integrity of the pieces received from peers.
    ///
    /// It compares `sha1` hash got from torrent file with `sha1` hash of each pieces on particular index.
    #[inline]
    fn check_integrity(&self, block: &Vec<u8>, pieces: &Vec<String>, index: usize) -> bool {
        let val = sha1_hash(&block).to_vec();
        let hex_val: String = val.iter().map(|b| format!("{:02x}", b)).collect();

        if pieces[index as usize] != hex_val {
            eprintln!(
                "Integrity error: Could not find the piece_hashes that matches provided piece."
            );
            return false;
        }
        true
    }

    /// This waits for pieces message from the peers right after sending `request` message
    #[inline]
    async fn wait_pieces(&mut self, stream: &mut TcpStream) -> MessageType {
        match self.start_exchange(stream).await {
            Ok(msg) => msg.message_type.clone(),
            Err(e) => {
                eprintln!("Failed to receive message: {:?}", e);
                return MessageType::Unknown(0, vec![]);
            }
        }
    }
}
