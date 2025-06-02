use rand::seq::SliceRandom;
use tokio::fs::File;
use tokio::io::AsyncSeekExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::sync::RwLock;

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
pub struct PeerConnection {
    stream: Arc<Mutex<TcpStream>>,
    pub bitfield_info: BitFieldInfo,
    pub message: Arc<Mutex<Messages>>,
    allocated_pieces: HashSet<u32>,
    block_storage: Arc<Mutex<HashMap<u32, BTreeMap<u32, Vec<u8>>>>>,
    block_book: Arc<RwLock<HashMap<u32, usize>>>,
}

impl PeerConnection {
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
            block_book: Arc::new(RwLock::new(HashMap::new())),
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
            let book = self.block_book.read().await;
            book.get(&index).copied()
        };

        if let Some(current_length) = output {
            if current_length >= total_length {
                return true;
            }
        }
        false
    }

    pub async fn request_piece_block(
        &mut self,
        index: u32,
        actual_piece_length: usize,
        total_blocks: usize,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let message: Arc<Mutex<Messages>> = Arc::clone(&self.message);

        for block_idx in 0..total_blocks {
            let begin = block_idx * BLOCK_SIZE as usize;

            let block_length = if begin + BLOCK_SIZE as usize <= actual_piece_length {
                BLOCK_SIZE as usize
            } else {
                actual_piece_length - begin
            };

            if let Err(e) = message
                .lock()
                .await
                .send_request(index, block_length, begin)
                .await
            {
                return Err(format!("Failed to send block request. FurtherMore:  {e}").into());
            }
        }

        Ok(())
    }

    async fn receive_piece_block(
        &mut self,
        total_blocks: usize,
        actual_piece_length: usize,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let mut received_blocks = 0;
        let start_time = std::time::Instant::now();
        let timeout = std::time::Duration::from_secs(30);

        while received_blocks < total_blocks {
            if start_time.elapsed() > timeout {
                return Err("Timeout waiting for piece".into());
            }

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
                            let mut book = self.block_book.write().await;
                            book.entry(index)
                                .and_modify(|l| *l += block.len())
                                .or_insert(block.len());
                        }

                        received_blocks += 1;

                        // Check if piece is complete
                        let is_complete = {
                            let book = self.block_book.read().await;
                            *book.get(&index).unwrap_or(&0) >= actual_piece_length
                        };

                        if is_complete {
                            return Ok(true);
                        }
                    }
                    _ => {
                        eprintln!("Error: >> Expected only Piece type block. ");
                    }
                }
            }
        }

        Ok(false)
    }

    pub async fn download_piece_blocks(
        &mut self,
        index: u32,
        pieces: &[String],
        actual_piece_length: usize,
        destination: &String,
        piece_len: u32,
        flag: Option<String>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let total_blocks = (actual_piece_length + BLOCK_SIZE as usize - 1) / BLOCK_SIZE as usize;

        if let Err(e) = self
            .request_piece_block(index, actual_piece_length, total_blocks)
            .await
        {
            eprintln!(
                "Error occured while requesting a piece block of index {index}. FurtherMore {e}"
            );
        };

        // Listen for responses
        if let Ok(is_complete) = self
            .receive_piece_block(total_blocks, actual_piece_length)
            .await
        {
            if is_complete {
                if let Err(e) = self
                    .check_integrity_and_save(
                        index,
                        pieces,
                        destination,
                        piece_len,
                        actual_piece_length,
                        &flag,
                    )
                    .await
                {
                    eprintln!(
                        "Error occured while calling for integrity checking and saving to file. FurtherMore: {e}"
                    )
                };
            }
        }

        Ok(())
    }

    pub async fn check_integrity_and_save(
        &self,
        index: u32,
        pieces: &[String],
        destination: &String,
        piece_len: u32,
        actual_piece_length: usize,
        flag: &Option<String>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let storage = self.block_storage.lock().await;
        if let Some(blocks) = storage.get(&index) {
            let mut data = Vec::with_capacity(actual_piece_length);

            let mut sorted_blocks: Vec<_> = blocks.iter().collect();
            sorted_blocks.sort_by_key(|(begin, _)| *begin);

            for (_, block) in sorted_blocks {
                data.extend(block);
            }

            // Verify piece length
            if data.len() != actual_piece_length {
                eprintln!(
                    "Piece {} has incorrect length: expected {}, got {}",
                    index,
                    actual_piece_length,
                    data.len()
                );
                return Err("Piece length mismatch".into());
            }

            let val = sha1_hash(&data).to_vec();
            let hex_val: String = val.iter().map(|b| format!("{:02x}", b)).collect();

            if pieces[index as usize] == hex_val {
                // Function to save data to a file
                if let Err(e) = self
                    .save_to_file(destination, flag, index, piece_len, &data)
                    .await
                {
                    eprintln!("Failed to save to file for index {index}. FurtherMore: {e}");
                };

                return Ok(());
            } else {
                return Err("SHA-1 mismatch".into());
            }
        } else {
            return Err("Failed to find any buffer for particular index".into());
        }
    }

    pub async fn save_to_file(
        &self,
        destination: &String,
        flag: &Option<String>,
        index: u32,
        piece_len: u32,
        data: &Vec<u8>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(parent) = std::path::Path::new(&destination).parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let mut file = File::options()
            .create(true)
            .write(true)
            .open(&destination)
            .await?;

        if !flag.clone().is_some_and(|x| x == "download_piece") {
            file.seek(std::io::SeekFrom::Start((index * piece_len) as u64))
                .await?;
        }

        file.write_all(&data).await?;
        file.flush().await?;

        Ok(())
    }
}

pub struct SwarmManager {
    pub connections: Arc<Mutex<Vec<PeerConnection>>>,
    self_peer_id: String,
    destination: String,
    peer_pieces: HashMap<String, Vec<u32>>,
}

impl SwarmManager {
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
                if let Ok(mut conn) = PeerConnection::init(addr).await {
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

        /*
        Number of peers available for particular piece
        */
        let mut pieces_peer: HashMap<u32, Vec<String>> = HashMap::new();
        for conn in connections.deref_mut() {
            for piece_idx in &conn.bitfield_info.present_pieces {
                pieces_peer
                    .entry(*piece_idx)
                    .or_default()
                    .push(conn.bitfield_info.peer_id.clone());
            }
        }

        /*
        Find the rarest pieces.
        */
        let mut rarest_pieces: Vec<(u32, Vec<String>)> = pieces_peer.into_iter().collect();
        rarest_pieces.sort_by_key(|(_, peers)| peers.len());

        /*
        Number of Pieces per peer.
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

                        let actual_piece_length = if index as usize == total_pieces - 1 {
                            file_size - (index as usize * piece_len)
                        } else {
                            piece_len
                        };

                        if let Err(e) = conn
                            .download_piece_blocks(
                                index,
                                &pieces,
                                actual_piece_length,
                                &destination,
                                piece_len as u32,
                                Some("download".to_string()),
                            )
                            .await
                        {
                            eprintln!("Error on downloading piece. FurtherMore:  {e}");
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

        let mut dm = SwarmManager::init();

        dm.connect_and_exchange_bitfield(
            ip_addr,
            info_hash.to_vec(),
            self_peer_id.as_bytes().to_vec(),
        )
        .await;

        dm.destination("/tmp/testing.txt".to_string())
            .final_peer_msg(
                _length.unwrap().clone(),
                &meta.info.pieces_hashes(),
                meta.info.piece_length,
            )
            .await;
    }
}
