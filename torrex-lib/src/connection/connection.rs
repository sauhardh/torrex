use rand::seq::SliceRandom;
use serde::Serialize;
use tokio::fs::File;
use tokio::io::AsyncSeekExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::sync::RwLock;
use tokio::sync::mpsc;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;

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

#[derive(Debug, Clone)]
pub struct BitFieldInfo {
    pub peer_id: String,
    /// pieces available on this specific peer (not all peers).
    pub present_pieces: HashSet<u32>,
}

/// ---------------------- PeerConnection ----------------------
///
/// This handles the connection and logic for download for each peers.

#[derive(Debug, Clone)]
pub struct PeerConnection {
    pub stream: Arc<Mutex<TcpStream>>,
    /// It has the info of the peer id and pieces present
    pub bitfield_info: BitFieldInfo,
    /// for the instance of [`Messages`](crate::connection::message::Messages) struct
    pub message: Arc<Mutex<Messages>>,
    /// Track what pieces has been allocated to particular peer
    allocated_pieces: HashSet<u32>,
    /// It stores block data together for particular index
    ///
    /// After a size of piece_length has been received, it is saved to the file and cleared from memory.
    block_storage: Arc<RwLock<HashMap<u32, BTreeMap<u32, Vec<u8>>>>>,
    /// To keep track of size of the block that has been downloaded.
    ///
    /// It stores "block size" out of  "piece size" that has been downloaded for a particular index(piece).
    block_book: Arc<RwLock<HashMap<u32, usize>>>,

    progress_notifier_tx: Option<Sender<()>>,
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
            block_storage: Arc::new(RwLock::new(HashMap::new())),
            block_book: Arc::new(RwLock::new(HashMap::new())),
            progress_notifier_tx: None,
        })
    }

    pub fn include_progress_subscriber(&mut self, progress_tx: Sender<()>) -> &Self {
        self.progress_notifier_tx = Some(progress_tx);

        self
    }

    pub async fn init_handshaking(
        &mut self,
        info_hash: Vec<u8>,
        peer_id: Vec<u8>,
    ) -> Result<Handshake, Box<dyn std::error::Error + Send + Sync>> {
        let mut handshake = Handshake::init(info_hash, peer_id);
        // Support for magnet link extension
        let buf: Vec<u8> = handshake.reserve_magnetlink().to_bytes();

        let mut stream = self.stream.lock().await;
        stream.write_all(&buf).await?;

        handshake.handshake_reply(&mut stream).await
    }

    pub async fn receive_bitfield(&mut self, peer_id: String) {
        let output = {
            let mut message = self.message.lock().await;
            message.wait_bitfield().await
        };

        if let Some(payload) = output {
            self.bitfield_info.peer_id = peer_id;
            self.bitfield_info.present_pieces = payload.into();
        };
    }

    async fn request_piece_block(
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

    #[inline]
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
                            let mut storage = self.block_storage.write().await;
                            storage
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

    #[inline]
    async fn check_integrity_and_save(
        &self,
        index: u32,
        pieces: &[String],
        destination: &String,
        piece_len: u32,
        actual_piece_length: usize,
        flag: &Option<String>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let storage = self.block_storage.read().await;
        if let Some(blocks) = storage.get(&index) {
            let mut data = Vec::with_capacity(actual_piece_length);

            let mut sorted_blocks: Vec<_> = blocks.iter().collect();
            sorted_blocks.sort_by_key(|(begin, _)| *begin);

            for (_, block) in sorted_blocks {
                data.extend(block);
            }

            // for (_, block) in blocks {
            //     data.extend(block);
            // }

            // Verify piece length
            if data.len() != actual_piece_length {
                return Err(format!(
                    "Piece {} has incorrect length: expected {}, got {}",
                    index,
                    actual_piece_length,
                    data.len()
                )
                .into());
            }

            let val = sha1_hash(&data).to_vec();
            if pieces[index as usize]
                == val.iter().map(|b| format!("{:02x}", b)).collect::<String>()
            {
                // Function to save data to a file
                if let Err(e) = self
                    .save_to_file(destination, flag, index, piece_len, &data)
                    .await
                {
                    eprintln!("Failed to save to file for index {index}. FurtherMore: {e}");
                };

                println!("Downlaod Successfull, Path: {destination} for index {index}");
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

        if flag.as_deref() != Some("download_piece") {
            file.seek(std::io::SeekFrom::Start((index * piece_len) as u64))
                .await?;
        }

        file.write_all(&data).await?;
        file.flush().await?;

        // Progress notifer
        //
        if let Some(sender) = &self.progress_notifier_tx {
            sender.send(()).await?;
        };

        Ok(())
    }
}

/// Struct to handle the progress infomation that can be passed to the frontend
#[derive(Debug, Serialize, Default)]
pub struct Progress {
    downloaded: u64, //
    download_speed: u64,
    upload_speed: Option<u64>,
    peers: u32, //
    status: String,
    eta: Option<u64>,
}

impl Progress {
    pub fn new(
        downloaded: u64,
        download_speed: u64,
        upload_speed: Option<u64>,
        peers: u32,
        status: String,
        eta: Option<u64>,
    ) -> Self {
        Self {
            downloaded,
            download_speed,
            upload_speed,
            peers,
            status,
            eta,
        }
    }
}

/// ---------------------- SwarmManager ----------------------
///
/// This handle the connection between different peers

#[derive(Debug, Clone)]
pub struct SwarmManager {
    /// Pool of connections for each peer.
    pub connections: Arc<Mutex<Vec<PeerConnection>>>,
    /// Id of the user as peer.
    self_peer_id: String,
    /// Path to save the particular file
    destination: String,
    /// Keeps number of pieces each peer has to download
    peer_pieces: HashMap<String, Vec<u32>>,
    /// A Sender to send the progress data real time, if incase subscribed.
    progress_tx: Option<Sender<Progress>>,
    /// To receive a call from each peers that a progress can be send.
    notfier_rx: Arc<Mutex<Option<Receiver<()>>>>,
}

impl SwarmManager {
    pub fn init() -> Self {
        Self {
            connections: Arc::new(Mutex::new(Vec::new())),
            self_peer_id: String::new(),
            destination: String::new(),
            peer_pieces: HashMap::new(),
            progress_tx: None,
            notfier_rx: Arc::new(Mutex::new(None)),
        }
    }

    pub fn destination(&mut self, dest: String) -> &mut Self {
        self.destination = dest;

        self
    }

    //<<----- Functionality related to real time progress updates.
    ///
    /// To subscribe for the progress updates. Must be called right after initialization of  [`SwarmManager`] struct.
    ///
    /// This will send a real time information through channel. A `Sender` must be passed as parameter.
    pub fn subscribe_updates(&mut self, tx: Sender<Progress>) -> &mut Self {
        self.progress_tx = Some(tx);

        self
    }

    /// Returns the number of peers active.
    async fn get_peers(&self) -> u64 {
        let l = self.connections.lock().await.len() as u64;

        l
    }

    /// Returns the size of data that has been downloaded till now.
    async fn get_downloaded(&self) -> u64 {
        let mut piece_progress: HashMap<u32, usize> = HashMap::new();

        let connections = {
            let lock = self.connections.lock().await;
            lock.clone()
        };

        for conn in connections.iter() {
            for (&piece_idx, &bytes) in conn.block_book.read().await.iter() {
                piece_progress
                    .entry(piece_idx)
                    .and_modify(|b| *b = (*b).max(bytes))
                    .or_insert(bytes);
            }
        }

        piece_progress.values().sum::<usize>() as u64
    }

    /// Sends [`Progress`] info thorugh the subscribed channel.
    ///
    /// Requires a call to [`subscribe_updates()`] beforehand
    async fn report_progress(&self) -> Result<(), Box<dyn std::error::Error>> {
        let peers = self.get_peers().await as u32;
        let downloaded = self.get_downloaded().await;

        let progress = Progress::new(downloaded, 0, Some(0), peers, "status".to_string(), Some(0));

        // Spwan a task only if progress_tx is available.
        // User has to first subcribe for updates and pass the tx(Sender)
        //
        // Security: progress_tx is expected to be there. As this function is called if and only if tx is present.
        // else, panic may occur
        let tx = self.progress_tx.clone().unwrap();
        tokio::spawn(async move {
            if let Err(e) = tx.send(progress).await {
                eprintln!("Failed to pass the progress through channel. {e}");
            };
        });

        Ok(())
    }

    pub async fn progress_listener(self: Arc<Self>) {
        let rx = Arc::clone(&self.notfier_rx);
        let this = Arc::clone(&self);

        tokio::spawn(async move {
            let mut rx = rx.lock().await;
            let rx = rx.as_mut().expect("Notifier rx is not set Yet!");

            while let Some(_) = rx.recv().await {
                if let Err(e) = this.report_progress().await {
                    eprintln!("Error occured on reporting progress. {e}");
                };
            }
        });
    }
    //----->>

    pub async fn connect_and_exchange_bitfield(
        &mut self,
        ip_addr: Vec<String>,
        info_hash: Vec<u8>,
        peer_id: Vec<u8>,
    ) {
        let mut tasks = vec![];
        self.self_peer_id = peer_id.iter().map(|b| format!("{:02x}", b)).collect();

        //<< To handle a "ok" call from peer, so that progress can be received and sent
        let (tx, rx) = mpsc::channel::<()>(16);
        self.notfier_rx = Arc::new(Mutex::new(Some(rx)));

        //>>

        for addr in ip_addr {
            let info_hash = info_hash.clone();
            let self_peer_id = peer_id.clone();
            let connections = Arc::clone(&self.connections);

            let notifier_tx = tx.clone();

            tasks.push(tokio::spawn(async move {
                match PeerConnection::init(addr).await {
                    Ok(mut conn) => match conn.init_handshaking(info_hash, self_peer_id).await {
                        Ok(handshake) => {
                            conn.receive_bitfield(handshake.remote_peer_id()).await;

                            let mut connections = connections.lock().await;

                            // This is so that each peer can notify--after any progress on download--those progress info are ready to be received.
                            conn.include_progress_subscriber(notifier_tx);

                            connections.push(conn);
                        }

                        Err(e) => {
                            eprintln!("Error occured on initiating handshake. FurtherMore {e:?} ",);
                            return;
                        }
                    },

                    Err(e) => {
                        eprintln!("Failed to establish connection. FurtherMore {:?}", e);
                    }
                }
            }));
        }

        for task in tasks {
            let _ = task.await;
        }

        // To receive a call from PeerConnection, to send the progress info through the channel.
        // User has to first subcribe for updates and pass the tx(Sender)
        if self.progress_tx.clone().is_some() {
            let this = Arc::new(self.clone());
            this.progress_listener().await;
        }
    }

    #[inline]
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
        let completed_pieces: Arc<RwLock<HashSet<u32>>> = Arc::new(RwLock::new(HashSet::new()));
        let unchoked_peers: Arc<RwLock<HashSet<String>>> = Arc::new(RwLock::new(HashSet::new()));
        let working_pieces: Arc<RwLock<HashSet<u32>>> = Arc::new(RwLock::new(HashSet::new()));

        let mut tasks = Vec::new();

        let destination = Arc::new(self.destination.clone());
        let pieces = Arc::new(pieces.clone());
        let connections = self.connections.clone();
        //>>

        for (peer, indexes) in &self.peer_pieces {
            let connections = Arc::clone(&connections);
            let destination = Arc::clone(&destination);
            let completed_pieces = Arc::clone(&completed_pieces);
            let unchoked_peers = Arc::clone(&unchoked_peers);
            let working_pieces = Arc::clone(&working_pieces);
            let pieces = Arc::clone(&pieces);

            let indexes = indexes.clone();
            let peer = peer.clone();

            tasks.push(tokio::spawn(async move {
                for index in indexes {
                    let pieces_completed_or_downloading = {
                        let completed = completed_pieces.read().await;
                        let mut working = working_pieces.write().await;

                        if completed.contains(&index) || working.contains(&index) {
                            true
                        } else {
                            working.insert(index);
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
                            let unchoke = unchoked_peers.read().await;
                            !unchoke.contains(&peer)
                        };
                        if should_unchoke {
                            let mut message = conn.message.lock().await;
                            message.interested().await;

                            if !message.wait_unchoke().await {
                                if !message.wait_unchoke().await {
                                    eprintln!("Peer {} did not unchoke, skipping", peer);
                                    return;
                                }
                            };
                            unchoked_peers.write().await.insert(peer.clone());
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

                        // To Mark pieces of particular index as completed
                        // let mut completed = completed_pieces.lock().await;
                        let mut completed = completed_pieces.write().await;
                        completed.insert(index);
                        drop(completed);

                        // It's no longer downloading as it should have already downloaded by now.
                        let mut working = working_pieces.write().await;
                        working.remove(&index);
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

        let (length, _files) = match &meta.info.key {
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
            *length.unwrap(),
            None,
            1,
        );

        let ip_addr = peers
            .announce_url(meta.announce)
            .request_tracker(params)
            .await
            .response
            .as_ref()
            .unwrap()
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
                length.unwrap().clone(),
                &meta.info.pieces_hashes(),
                meta.info.piece_length,
            )
            .await;
    }
}
