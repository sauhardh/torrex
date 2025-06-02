use rand::seq::SliceRandom;
use tokio::fs::File;
use tokio::io::AsyncSeekExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::Mutex;

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::net::SocketAddrV4;
use std::ops::DerefMut;
use std::sync::Arc;

use crate::connection::handshake::Handshake;
use crate::connection::message::MessageType;
use crate::connection::message::Messages;
use crate::cryptography::sha1_hash;

const BLOCK_SIZE: u32 = 16_384;

#[derive(Debug)]
pub struct BitFieldInfo {
    pub peer_id: String,
    pub present_pieces: HashSet<u32>,
}

#[derive(Debug)]
pub struct Connection {
    stream: Arc<Mutex<TcpStream>>,
    pub bitfield_info: BitFieldInfo,
    pub message: Arc<Mutex<Messages>>,
    allocated_pieces: HashSet<u32>,
    block_storage: Arc<Mutex<HashMap<u32, BTreeMap<u32, Vec<u8>>>>>,
    block_book: Arc<Mutex<HashMap<u32, usize>>>,
}

impl Connection {
    pub async fn init(addr: String) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let addr = addr.parse::<SocketAddrV4>()?;
        let stream = TcpStream::connect(addr).await?;
        let arc_stream = Arc::new(Mutex::new(stream));

        let message = Arc::new(Mutex::new(Messages::init(arc_stream.clone())));

        Ok(Self {
            stream: arc_stream,
            bitfield_info: BitFieldInfo {
                peer_id: String::new(),
                present_pieces: HashSet::new(),
            },
            message,
            allocated_pieces: HashSet::new(),
            block_storage: Arc::new(Mutex::new(HashMap::new())),
            block_book: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub async fn init_handshaking(
        &mut self,
        info_hash: Vec<u8>,
        peer_id: Vec<u8>,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let handshake = Handshake::init(info_hash, peer_id);
        let buf: Vec<u8> = handshake.to_bytes();

        let mut stream = self.stream.lock().await;
        stream.write_all(&buf).await?;

        let reply = handshake.handshake_reply(&mut stream).await?;

        let remote_peer_id = reply.peer_id.iter().map(|b| format!("{:02x}", b)).collect();
        Ok(remote_peer_id)
    }

    // if let Some(bitfield_payload) = message.wait_bitfield().await {
    //     self.bitfield_info.peer_id = peer_id;
    //     self.bitfield_info.present_pieces = bitfield_payload.into();
    // }
    pub async fn receive_bitfield(&mut self, peer_id: String) {
        let output = {
            let mut message = self.message.lock().await;
            message.wait_bitfield().await
        };

        if let Some(bitfield_payload) = output {
            self.bitfield_info.peer_id = peer_id;
            self.bitfield_info.present_pieces = bitfield_payload.into();
        };
    }

    pub async fn is_piece_complete(&self, index: u32, total_length: usize) -> bool {
        let output = {
            let book = self.block_book.lock().await;
            book.get(&index).copied()
        };

        if let Some(current_length) = output {
            if current_length >= total_length {
                return true;
            }
        }
        false
    }

    pub async fn request_piece_block(&mut self, index: u32, piece_length: usize) {
        let current_offset = {
            let book = self.block_book.lock().await;
            book.get(&index).copied().unwrap_or(0)
        };

        let block_length = if current_offset + BLOCK_SIZE as usize <= piece_length {
            BLOCK_SIZE as usize
        } else {
            piece_length - current_offset
        };

        println!(
            "sending request to index: {index}, block_length {block_length} and current_offset {current_offset}"
        );

        let output = {
            let message = self.message.lock().await;
            message
                .send_request(index, block_length, current_offset)
                .await
        };

        if let Err(e) = output {
            eprintln!(
                "Error occured while sending request message to the peer for pieces of index: {index}. FurtherMore: {e}"
            );
        }
    }

    async fn receive_piece_block(&mut self) -> Option<()> {
        let output = {
            let mut message = self.message.lock().await;
            message.wait_piece_block().await
        };

        if let Some(pb) = output {
            match pb {
                MessageType::Piece {
                    index,
                    begin,
                    block,
                } => {
                    {
                        let mut block_storage = self.block_storage.lock().await;
                        block_storage
                            .entry(index)
                            .or_default()
                            .insert(begin, block.clone());
                    }

                    {
                        let mut block_book = self.block_book.lock().await;
                        block_book
                            .entry(index)
                            .and_modify(|l| *l += block.len())
                            .or_insert(block.len());
                    }

                    println!("blook_book {:?}\n\n", self.block_book);
                    return Some(());
                }

                _ => {
                    eprintln!("Expected only Piece type block. ");
                }
            }
        }
        None
    }

    pub async fn download_piece_blocks(
        &mut self,
        index: u32,
        pieces: &[String],
        piece_length: usize,
        destination: &String,
        piece_len: u32,
        flag: Option<String>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let total_blocks = (piece_length + BLOCK_SIZE as usize - 1) / BLOCK_SIZE as usize;
        let message = Arc::clone(&self.message);
        let storage = Arc::clone(&self.block_storage);
        let book = Arc::clone(&self.block_book);

        println!("total_blocks {total_blocks}");

        // Send all block requests immediately
        for block_idx in 0..total_blocks {
            let begin = block_idx * BLOCK_SIZE as usize;

            let block_length = if begin + BLOCK_SIZE as usize <= piece_length {
                BLOCK_SIZE as usize
            } else {
                piece_length - begin
            };

            println!(
                "for sending request block_idx {block_idx} and begin {begin} and block_length {block_length}"
            );

            if let Err(e) = message
                .lock()
                .await
                .send_request(index, block_length, begin)
                .await
            {
                eprintln!("Failed to send block request: {e}");
                return Err("Failed to send block request".into());
            }
        }

        // Listen for responses
        let mut received_blocks = 0;
        let start_time = std::time::Instant::now();
        let timeout = std::time::Duration::from_secs(30);

        while received_blocks < total_blocks {
            if start_time.elapsed() > timeout {
                eprintln!("Timeout waiting for piece {}", index);
                return Err("Timeout waiting for piece".into());
            }

            if let Some(pb) = message.lock().await.wait_piece_block().await {
                match pb {
                    MessageType::Piece {
                        index,
                        begin,
                        block,
                    } => {
                        {
                            let mut block_storage = storage.lock().await;
                            block_storage
                                .entry(index)
                                .or_default()
                                .insert(begin, block.clone());
                        }

                        {
                            let mut block_book = book.lock().await;
                            block_book
                                .entry(index)
                                .and_modify(|l| *l += block.len())
                                .or_insert(block.len());
                        }

                        received_blocks += 1;

                        // Check if piece is complete
                        let is_complete = {
                            let block_book = book.lock().await;
                            *block_book.get(&index).unwrap_or(&0) >= piece_length
                        };

                        if is_complete {
                            // Write the piece to file
                            if let Some(blocks) = storage.lock().await.get(&index) {
                                let mut data = Vec::with_capacity(piece_length);
                                let mut sorted_blocks: Vec<_> = blocks.iter().collect();
                                sorted_blocks.sort_by_key(|(begin, _)| *begin);

                                for (_, block) in sorted_blocks {
                                    data.extend(block);
                                }

                                // Verify piece length
                                if data.len() != piece_length {
                                    eprintln!(
                                        "Piece {} has incorrect length: expected {}, got {}",
                                        index,
                                        piece_length,
                                        data.len()
                                    );
                                    return Err("Piece length mismatch".into());
                                }

                                let val = sha1_hash(&data).to_vec();
                                let hex_val: String =
                                    val.iter().map(|b| format!("{:02x}", b)).collect();

                                if pieces[index as usize] == hex_val {
                                    if let Some(parent) =
                                        std::path::Path::new(&destination).parent()
                                    {
                                        tokio::fs::create_dir_all(parent).await?;
                                    }

                                    let mut file = File::options()
                                        .create(true)
                                        .write(true)
                                        .open(&destination)
                                        .await?;

                                    if !flag.is_some_and(|x| x == "download_piece") {
                                        file.seek(std::io::SeekFrom::Start(
                                            (index * piece_len) as u64,
                                        ))
                                        .await?;
                                    }

                                    file.write_all(&data).await?;
                                    file.flush().await?;

                                    println!(
                                        "Successfully wrote piece {} to file (length: {})",
                                        index,
                                        data.len()
                                    );
                                    return Ok(());
                                } else {
                                    eprintln!(
                                        "SHA-1 mismatch for piece {}: expected {}, got {}",
                                        index, pieces[index as usize], hex_val
                                    );
                                    return Err("SHA-1 mismatch".into());
                                }
                            }
                        }
                    }
                    _ => {
                        eprintln!("Error: >> Expected only Piece type block. ");
                    }
                }
            }
        }
        Err("Failed to receive all blocks".into())
    }

    // pub async fn check_integrity(
    //     &self,
    //     index: u32,
    //     pieces: &[String],
    //     destination: &String,
    //     piece_len: u32,
    // ) -> Result<(), Box<dyn std::error::Error>> {
    //     if let Some(blocks) = self.block_storage.lock().await.get(&index) {
    //         let mut data = Vec::new();

    //         // Sort blocks by begin offset to ensure correct order
    //         let mut sorted_blocks: Vec<_> = blocks.iter().collect();
    //         sorted_blocks.sort_by_key(|(begin, _)| *begin);

    //         for (_, block) in sorted_blocks {
    //             data.extend(block);
    //         }

    //         let val = sha1_hash(&data).to_vec();
    //         let hex_val: String = val.iter().map(|b| format!("{:02x}", b)).collect();

    //         if pieces[index as usize] == hex_val {
    //             // Create directory if it doesn't exist
    //             if let Some(parent) = std::path::Path::new(destination).parent() {
    //                 tokio::fs::create_dir_all(parent).await?;
    //             }

    //             // Open file with append mode
    //             let mut file = File::options()
    //                 .create(true)
    //                 .append(true) // Use append instead of write
    //                 .open(destination)
    //                 .await?;

    //             // Seek to the correct position
    //             file.seek(std::io::SeekFrom::Start((index * piece_len) as u64))
    //                 .await?;

    //             // Write the data
    //             file.write_all(&data).await?;
    //             file.flush().await?; // Ensure data is written to disk

    //             println!("Successfully wrote piece {} to file", index);
    //             return Ok(());
    //         }
    //     }

    //     Err("Integrity check failed".into())
    // }

    // pub async fn save_to_file(
    //     &self,
    //     dest: &String,
    //     index: u32,
    //     piece_len: u32,
    // ) -> Result<(), Box<dyn std::error::Error>> {
    //     let mut file = File::options().create(true).append(true).open(dest).await?;
    //     if let Some(b) = self.block_storage.lock().await.get(&index) {
    //         file.seek(std::io::SeekFrom::Start((index * piece_len) as u64))
    //             .await?;

    //         for block in b.values() {
    //             file.write_all(&block).await?;
    //         }
    //     }

    //     Ok(())
    // }

    pub async fn flush(&mut self) {
        let mut book = self.block_book.lock().await;
        let mut block = self.block_storage.lock().await;

        book.clear();
        block.clear();
        self.allocated_pieces.clear();

        let stream = self.stream.clone();
        tokio::spawn(async move {
            let mut stream = stream.lock().await;
            if let Err(e) = stream.shutdown().await {
                eprintln!("Failed to shutdown TCP stream. FurtherMore: {e}")
            };
        });
    }
}

/////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////
pub struct DownloadManager {
    pub connections: Arc<Mutex<Vec<Connection>>>,
    self_peer_id: String,
    destination: String,
    peer_pieces: HashMap<String, Vec<u32>>,
}

impl DownloadManager {
    pub fn init() -> Self {
        Self {
            connections: Arc::new(Mutex::new(Vec::new())),
            self_peer_id: String::new(),
            destination: String::new(),
            peer_pieces: HashMap::new(),
        }
    }

    pub fn destination(&mut self, dest: String) -> &mut Self {
        self.destination = dest;

        self
    }

    pub async fn connect_and_exchange_bitfield(
        &mut self,
        ip_addr: Vec<String>,
        info_hash: Vec<u8>,
        peer_id: Vec<u8>,
    ) {
        let mut tasks = vec![];
        self.self_peer_id = peer_id.iter().map(|b| format!("{:02x}", b)).collect();

        for addr in ip_addr {
            let info_hash = info_hash.clone();
            let peer_id = peer_id.clone();
            let connections = Arc::clone(&self.connections);

            tasks.push(tokio::spawn(async move {
                if let Ok(mut conn) = Connection::init(addr).await {
                    match conn.init_handshaking(info_hash, peer_id).await {
                        Ok(remote_peer_id) => {
                            conn.receive_bitfield(remote_peer_id).await;

                            let mut connections = connections.lock().await;
                            connections.push(conn);
                        }

                        Err(e) => {
                            eprintln!("Error occured on initiating handshake {e:?} ",);
                            return;
                        }
                    }
                };
            }));
        }

        for task in tasks {
            let _ = task.await;
        }
    }

    async fn peer_pieces_selection(&mut self) {
        let mut connections = self.connections.lock().await;

        // /*
        // Step 1: Number of peers available for particular piece
        // */
        let mut pieces_peer: HashMap<u32, Vec<String>> = HashMap::new();
        for conn in connections.deref_mut() {
            for piece_idx in &conn.bitfield_info.present_pieces {
                pieces_peer
                    .entry(*piece_idx)
                    .or_default()
                    .push(conn.bitfield_info.peer_id.clone());
            }
        }

        // /*
        // Step 2: Find the rarest pieces.
        // */
        let mut rarest_pieces: Vec<(u32, Vec<String>)> = pieces_peer.into_iter().collect();
        rarest_pieces.sort_by_key(|(_, peers)| peers.len());

        /*
        Step 3: Number of Pieces per peer.
        Based on random number.
        */
        let mut assigned_pieces: HashSet<u32> = HashSet::new();
        let mut peer_pieces: HashMap<String, Vec<u32>> = HashMap::new();
        let mut peer_load: HashMap<String, usize> = HashMap::new();
        for (piece_idx, mut peers) in rarest_pieces {
            if peers.len() > 1 {
                peers.shuffle(&mut rand::rng());
            }

            peers.sort_by_key(|peer| peer_load.get(peer).unwrap_or(&0));

            for peer in peers {
                if assigned_pieces.insert(piece_idx) {
                    peer_pieces.entry(peer.clone()).or_default().push(piece_idx);
                    *peer_load.entry(peer).or_insert(0) += 1;
                    break;
                }
            }
        }

        for conn in connections.deref_mut() {
            if let Some(idxs) = peer_pieces.get(&conn.bitfield_info.peer_id) {
                conn.allocated_pieces.extend(idxs);
            }
        }

        self.peer_pieces = peer_pieces;
    }

    pub async fn final_peer_msg(
        &mut self,
        file_size: usize,
        pieces: &Vec<String>,
        piece_len: usize,
    ) {
        self.peer_pieces_selection().await;
        let total_pieces = (file_size + piece_len - 1) / piece_len;

        //<< For Passing through asynchronous task.
        let completed_pieces: Arc<Mutex<HashSet<u32>>> = Arc::new(Mutex::new(HashSet::new()));
        let unchoked_peers: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
        let destination = Arc::new(self.destination.clone());
        let pieces = Arc::new(pieces.clone());
        let connections = self.connections.clone();
        let downloading_pieces: Arc<Mutex<HashSet<u32>>> = Arc::new(Mutex::new(HashSet::new()));

        let mut tasks = Vec::new();
        //>>

        for (peer, indexes) in &self.peer_pieces {
            let connections = Arc::clone(&connections);
            let destination = Arc::clone(&destination);
            let completed_pieces = Arc::clone(&completed_pieces);
            let unchoked_peers = Arc::clone(&unchoked_peers);
            let downloading_pieces = Arc::clone(&downloading_pieces);

            let pieces = Arc::clone(&pieces);

            let indexes = indexes.clone();
            let peer = peer.clone();

            tasks.push(tokio::spawn(async move {
                for index in indexes {
                    let pieces_completed_or_downloading = {
                        let completed_pieces = completed_pieces.lock().await;
                        let mut working_pieces = downloading_pieces.lock().await;

                        if completed_pieces.contains(&index) || working_pieces.contains(&index) {
                            true
                        } else {
                            working_pieces.insert(index);
                            false
                        }
                    };

                    if pieces_completed_or_downloading {
                        continue;
                    };

                    let mut connections = connections.lock().await;
                    if let Some(conn) = connections
                        .iter_mut()
                        .find(|x| x.bitfield_info.peer_id == *peer)
                    {
                        let should_unchoke = {
                            let unchoke = unchoked_peers.lock().await;
                            !unchoke.contains(&peer)
                        };
                        if should_unchoke {
                            let mut message = conn.message.lock().await;
                            message.interested().await;

                            if !message.wait_unchoke().await {
                                eprintln!("Peer {} did not unchoke, skipping", peer);

                                return;
                            };
                            unchoked_peers.lock().await.insert(peer.clone());
                        }

                        let piece_length = if index as usize == total_pieces - 1 {
                            file_size - (index as usize * piece_len)
                        } else {
                            piece_len
                        };

                        println!("Download piece blocks with {piece_length}, {file_size} {index}");
                        if let Err(e) = conn
                            .download_piece_blocks(
                                index,
                                &pieces,
                                piece_length,
                                &destination,
                                piece_len as u32,
                                Some("download".to_string()),
                            )
                            .await
                        {
                            eprintln!("Error on downloading piece {e}");
                        };

                        {
                            // To Mark pieces of particular index as completed
                            let mut completed = completed_pieces.lock().await;
                            completed.insert(index);
                        }

                        {
                            // It's no longer downloading as it should have already downloaded by now.
                            let mut downloading = downloading_pieces.lock().await;
                            downloading.remove(&index);
                        }
                    }
                }
            }));
        }

        for task in tasks {
            if let Err(e) = task.await {
                eprintln!("Task failed: {e}");
            }
        }
    }
}

#[cfg(test)]
mod test_connection {
    use super::*;

    use crate::metainfo::FileKey;
    use crate::metainfo::TorrentFile;
    use crate::peers::Peers;
    use crate::utils::random;

    use std::path::Path;

    #[tokio::test]
    async fn connection() {
        let meta: TorrentFile = TorrentFile::new();
        let encoded_data = meta.read_file(Path::new("./sample.torrent")).unwrap();
        let meta: TorrentFile = meta.parse_metafile(&encoded_data);

        let (_length, _files) = match &meta.info.key {
            FileKey::SingleFile { length } => (Some(length), None),
            FileKey::MultiFile { files } => (None, Some(files)),
        };
        let info_hash = meta.info_hash(&encoded_data).unwrap();

        // Discovering peers
        let self_peer_id = random::generate_peerid();

        let mut peers = Peers::new();
        let params = &peers.request.new(
            info_hash.to_vec(),
            self_peer_id.clone(), // random string
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
            .peers_ip();

        let mut dm = DownloadManager::init();

        dm.connect_and_exchange_bitfield(
            ip_addr,
            info_hash.to_vec(),
            self_peer_id.as_bytes().to_vec(),
        )
        .await;

        dm.destination("/tmp/test.txt".to_string())
            .final_peer_msg(
                _length.unwrap().clone(),
                &meta.info.pieces_hashes(),
                meta.info.piece_length,
            )
            .await;
    }
}
