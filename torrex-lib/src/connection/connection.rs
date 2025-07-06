use rand::seq::SliceRandom;
use serde::Serialize;
use tokio::fs::File;
use tokio::io::AsyncSeekExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::sync::RwLock;
use tokio::sync::Semaphore;
use tokio::sync::mpsc;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use tokio::time::timeout;

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::error;
use std::ops::DerefMut;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::RwLockWriteGuard;
use std::thread::sleep;
use std::time::Duration;

use crate::connection::download;
use crate::connection::download::DownloadCommand;
use crate::connection::download::DownloadManager;
use crate::connection::download::DownloadState;
use crate::connection::handshake::Handshake;
use crate::connection::message::MessageType;
use crate::connection::message::Messages;
use crate::cryptography::sha1_hash;
use crate::utils::config::config_dir;

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
        let stream_result = timeout(Duration::from_secs(280), async {
            TcpStream::connect(addr.clone()).await
        })
        .await;

        let stream = if let Ok(stream) = stream_result {
            match stream {
                Ok(stream) => stream,
                Err(e) => {
                    eprintln!("Failed to connect to {addr}: {e}");
                    return Err("Failed to establish tcp connection".into());
                }
            }
        } else {
            eprintln!("Timeout while connecting to {addr}");
            return Err("timeout while establishing tcp connection".into());
        };

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

        let reply = handshake.handshake_reply(&mut stream).await;
        reply
    }

    pub async fn receive_have(&mut self, peer_id: String) -> bool {
        let output = {
            let mut message = self.message.lock().await;
            message.wait_have().await
        };

        if let Some(payload) = output {
            self.bitfield_info.peer_id = peer_id;
            self.bitfield_info.present_pieces.insert(payload);

            return true;
        };

        false
    }

    pub async fn receive_bitfield(&mut self, peer_id: String) -> bool {
        let output = {
            let mut message = self.message.lock().await;
            message.wait_bitfield().await
        };

        if let Some(payload) = output {
            self.bitfield_info.peer_id = peer_id;
            self.bitfield_info.present_pieces = payload.into();

            return true;
        };

        false
    }

    #[inline]
    pub async fn send_request(
        &self,
        index: u32,
        length: usize,
        begin: usize,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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

    async fn request_piece_block(
        &mut self,
        index: u32,
        actual_piece_length: usize,
        total_blocks: usize,
    ) -> Result<(), Box<dyn std::error::Error>> {

        for block_idx in 0..total_blocks {
            let begin = block_idx * BLOCK_SIZE as usize;

            let block_length = if begin + BLOCK_SIZE as usize <= actual_piece_length {
                BLOCK_SIZE as usize
            } else {
                actual_piece_length - begin
            };

            if let Err(e) = self
                .send_request(index, block_length, begin)
                .await
            {
                return Err(format!("Failed to send block request. FurtherMore:  {e}").into());
            }
        }

        Ok(())
    }

    // #[inline]
    // async fn receive_piece_block(
    //     &mut self,
    //     total_blocks: usize,
    //     actual_piece_length: usize,
    // ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    //     let mut received_blocks = 0;

    //     let mut consecutive_failure = 0;
    //     const MAX_CONSECUTIVE_FAILURE : i32 = 2;

    //     while received_blocks < total_blocks {
    //         let output = {
    //             let mut message = self.message.lock().await;
    //             message.wait_piece_block().await
    //         };
    //         print!("waiting piece block done!! for {received_blocks} / {total_blocks}...\n");

    //         if let Some(pb) = output {
    //             match pb {
    //                 MessageType::Piece {
    //                     index,
    //                     begin,
    //                     block,
    //                 } => {
    //                     {
    //                         let mut storage = self.block_storage.write().await;
    //                         storage
    //                             .entry(index)
    //                             .or_default()
    //                             .insert(begin, block.clone());
    //                     }

    //                     {
    //                         let mut book = self.block_book.write().await;
    //                         book.entry(index)
    //                             .and_modify(|l| *l += block.len())
    //                             .or_insert(block.len());
    //                         print!("Book {:?}", book.get(&index));
    //                         print!("/ Piece_length {}\n", actual_piece_length);
    //                     }

    //                     received_blocks += 1;

    //                     // Check if piece is complete
    //                     let is_complete = {
    //                         let book = self.block_book.read().await;
    //                         *book.get(&index).unwrap_or(&0) >= actual_piece_length
    //                     };

    //                     if is_complete {
    //                         return Ok(true);
    //                     }
    //                 }
    //                 other => {
    //                     eprintln!(" receive_piece_block...Error: >> Expected only Piece type block. Got {other:?}");
    //                     consecutive_failure +=1;

    //                     if consecutive_failure >= MAX_CONSECUTIVE_FAILURE{
    //                         return Err("To many consecutive failure. Could not receive data by".into());
    //                     }

    //                     println!("Retry to recieve piece block {}", consecutive_failure);
    //                     // tokio::time::sleep(Duration::from_millis(100)).await;
    //                     // continues;
    //                 }
    //             }
    //         }
    //     }

    //     Ok(false)
    // }

    #[inline]
async fn receive_piece_block(
    &mut self,
    total_blocks: usize,
    actual_piece_length: usize,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let mut received_blocks = 0;
    let mut consecutive_failure = 0;
    const MAX_CONSECUTIVE_FAILURE: i32 = 5; // Increased from 2 to 5

    while received_blocks < total_blocks {
        let output = {
            let mut message = self.message.lock().await;
            message.wait_piece_block().await
        };
        print!("waiting piece block done!! for {received_blocks} / {total_blocks}...\n");

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
                        print!("Book {:?}", book.get(&index));
                        print!("/ Piece_length {}\n", actual_piece_length);
                    }

                    received_blocks += 1;
                    consecutive_failure = 0; // Reset failure counter on success

                    // Check if piece is complete
                    let is_complete = {
                        let book = self.block_book.read().await;
                        *book.get(&index).unwrap_or(&0) >= actual_piece_length
                    };

                    if is_complete {
                        return Ok(true);
                    }
                }
                
                // Handle keep-alive messages gracefully
                MessageType::Unknown(0, _) => {
                    // This is a keep-alive message, not an error
                    println!("Received keep-alive message, continuing...");
                    consecutive_failure = 0; // Reset failure counter
                    continue;
                }
                
                // Handle other non-error messages
                MessageType::Choke => {
                    println!("Received choke message during piece download");
                    consecutive_failure += 1;
                }
                
                MessageType::Unchoke => {
                    println!("Received unchoke message during piece download");
                    consecutive_failure = 0; // Reset failure counter
                }
                
                MessageType::Have(_) => {
                    println!("Received have message during piece download");
                    consecutive_failure = 0; // Reset failure counter
                }
                
                MessageType::BitField(_) => {
                    println!("Received bitfield message during piece download");
                    consecutive_failure = 0; // Reset failure counter
                }
                
                MessageType::Interested | MessageType::NotInterested => {
                    println!("Received interest message during piece download");
                    consecutive_failure = 0; // Reset failure counter
                }
                
                MessageType::Extended(_, _) => {
                    println!("Received extended message during piece download");
                    consecutive_failure = 0; // Reset failure counter
                }
                
                // Handle actual errors
                other => {
                    println!("Unexpected message during piece download: {other:?}");
                    consecutive_failure += 1;
                }
            }
        } else {
            // Timeout occurred
            consecutive_failure += 1;
            println!("Timeout waiting for piece block, retry {}/{}", consecutive_failure, MAX_CONSECUTIVE_FAILURE);
        }

        // Check if we've exceeded the failure limit
        if consecutive_failure >= MAX_CONSECUTIVE_FAILURE {
            return Err(format!("Too many consecutive failures ({}) while receiving piece blocks", consecutive_failure).into());
        }

        // Small delay before retrying
        tokio::time::sleep(Duration::from_millis(100)).await;
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

        if !self.bitfield_info.present_pieces.contains(&index) {
            eprintln!("This particular index {index} is not associated with this peer.");
            return Ok(());
        }

        // Send piece requests
        if let Err(e) = self
            .request_piece_block(index, actual_piece_length, total_blocks)
            .await
        {
            eprintln!(
                "Error occurred while requesting a piece block of index {index}. FurtherMore {e}"
            );
            return Err(format!("error: {e}").into());
        }


        // Listen for responses with better error handling
        match self.receive_piece_block(total_blocks, actual_piece_length).await {
            Ok(is_complete) => {
                if is_complete {
                    println!("piece Completed {index}");
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
                            "Error occurred while calling for integrity checking and saving to file. FurtherMore: {e}"
                        );
                        return Err(e);
                    }
                }
                Ok(())
            }
            Err(e) => {
                eprintln!("Error receiving piece blocks: {}", e);
                Err(e)
            }
        }
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
                print!("Integrity passed.");
                // Function to save data to a file
                if let Err(e) = self
                    .save_to_file(destination, flag, index, piece_len, &data)
                    .await
                {
                    eprintln!("Failed to save to file for index {index}. FurtherMore: {e}");
                };
                print!("Saved to file. for {index}\n");

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
            file.seek(std::io::SeekFrom::Start(index as u64 * piece_len as u64))
                .await?;
        }

        file.write_all(&data).await?;
        file.flush().await?;

        // Progress notifer
        // This sends as a notifier to the receiver, so that the receiver can now start to send a progress info to its receiver.
        if let Some(sender) = &self.progress_notifier_tx {
            println!("--Sending notification--");
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
    /// Info hash of the current download
    info_hash: String,
    /// Keeps number of pieces each peer has to download
    peer_pieces: HashMap<String, Vec<u32>>,
    /// This is to keep track of the downloaded pieces.
    completed_pieces: Arc<RwLock<HashSet<u32>>>,
    /// A Sender to send the progress data real time, if incase subscribed.
    progress_tx: Option<Sender<Progress>>,
    /// To receive a call from each peers that a progress can be send.
    notfier_rx: Arc<Mutex<Option<Receiver<()>>>>,
    /// Download manager of the current downloading
    pub download_manager: Arc<Mutex<DownloadManager>>,

    pub unchoked_peers: Arc<RwLock<HashSet<String>>>,
}

impl SwarmManager {
    pub fn init() -> Self {
        Self {
            connections: Arc::new(Mutex::new(Vec::new())),
            self_peer_id: String::new(),
            destination: String::new(),
            peer_pieces: HashMap::new(),
            info_hash: String::new(),
            completed_pieces: Arc::new(RwLock::new(HashSet::new())),
            // For real time progress
            progress_tx: None,
            notfier_rx: Arc::new(Mutex::new(None)),
            // Download state
            download_manager: Arc::new(Mutex::new(DownloadManager::new())),

            unchoked_peers: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    pub fn destination(&mut self, dest: String) -> &mut Self {
        self.destination = dest;

        self
    }

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

    /// A function to subscribe to get support for download manager
    ///
    /// Feature includes download, pause, resume
    ///
    /// However download is by default and it will start even if subscribe is not called
    pub async fn subscribe_downloadmanager(&mut self) -> &mut Self {
        let (sender, receiver) = mpsc::channel::<DownloadCommand>(2);
        {
            let dm = self.download_manager.lock().await;
            let mut control_tx = dm.control_tx.lock().await;
            *control_tx = Some(sender);
        }

        {
            let dm = self.download_manager.lock().await;
            let mut control_rx = dm.control_rx.lock().await;
            *control_rx = Some(receiver);
        }

        self
    }

    async fn get_torrex_dir(&self, filename: String) -> Result<PathBuf, Box<dyn error::Error>> {
        let path = config_dir("torrex").join(format!("{}.json", filename));

        if !path.exists() {
            File::create(path.clone()).await?;
        } else {
            return Err("Could not find the requested file/dir in the given path".into());
        }

        Ok(path)
    }

    pub async fn connect_and_exchange_bitfield(
        &mut self,
        ip_addr: Vec<String>,
        info_hash: Vec<u8>,
        peer_id: Vec<u8>,
        file_size: usize,
        piece_len: usize,
    ) {
        let mut tasks = vec![];
        self.self_peer_id = peer_id.iter().map(|b| format!("{:02x}", b)).collect();
    
        //<< To handle a "ok" call from peer, so that progress can be received and sent
        let (tx, rx) = mpsc::channel::<()>(16);
        self.notfier_rx = Arc::new(Mutex::new(Some(rx)));
        //>>
    
        let semaphore = Arc::new(Semaphore::new(5));
        let unchoked_peers = Arc::clone(&self.unchoked_peers);
    
        for addr in ip_addr {
    
            let info_hash = info_hash.clone();
            let self_peer_id = peer_id.clone();
            let connections = Arc::clone(&self.connections);
            let notifier_tx = tx.clone();
            let semaphore = semaphore.clone();
            let unchoked_peers = unchoked_peers.clone();
            let total_pieces = (file_size + piece_len - 1) / piece_len;

    
            tasks.push(tokio::spawn(async move {
                let _permit = semaphore.acquire().await.expect("Failed to acquire semaphore");
    
                match PeerConnection::init(addr).await {
                    Ok(mut conn) => match conn.init_handshaking(info_hash, self_peer_id).await {
                        Ok(handshake) => {                            
                            conn.bitfield_info.peer_id = handshake.remote_peer_id();
                            // Set up progress subscriber
                            conn.include_progress_subscriber(notifier_tx);
                            
                            let mut has_bitfield = false;
                            let mut has_unchoke = false;
                            let mut has_have = false;
                            let mut sent_interested = false;
                            let mut timeout_counter = 0;
                            const MAX_TIMEOUT: u32 = 3;

                            loop {

                                let  msg_result = {
                                    let mut message = conn.message.lock().await;
                                    message.exchange_msg().await
                                };

                                match msg_result{
                                    Ok(_) => {
                                        // POINTER
                                        let message_type = {
                                            let message = conn.message.lock().await;
                                            message.message_type.clone()
                                        };

                                        match message_type {
                                            MessageType::BitField(payload) => {
                                                let mut piece_idx: HashSet<u32> = HashSet::new();
                                                for (byte_idx, byte) in payload.iter().enumerate() {
                                                    for bit in 0..8 {
                                                        if byte >> (7-bit) & 1 == 1 {
                                                            let piece_index = byte_idx as u32 * 8 + bit;

                                                            if piece_index >= total_pieces as u32{
                                                                break;
                                                            }
                                                            piece_idx.insert(piece_index);
                                                        }  
                                                    }
                                                }
                                                conn.bitfield_info.present_pieces = piece_idx.clone();
                                                has_bitfield = true;
                                                timeout_counter = 0;

                                                // if let Err(e) = message.interested().await { 
                                                let mut msg_guard = {
                                                    conn.message.lock().await
                                                };
                                                if let Err(e) = msg_guard.interested().await { 
                                                    eprintln!("Failed to send interested message: {e}");
                                                    return;
                                                }
                                                sent_interested = true;               
                                            }
    
                                            MessageType::Have(piece_index) => {
                                                conn.bitfield_info.present_pieces.insert(piece_index);
                                                has_have = true;
                                                timeout_counter = 0;

                                                if !sent_interested{
                                                    let mut msg_guard = {
                                                        conn.message.lock().await
                                                    };
                                                if let Err(e) = msg_guard.interested().await { 
                                                        eprintln!("Failed to send interested message: {e}");
                                                        return;
                                                    }
                                                    sent_interested = true;
                                                }
                                            }
    
                                            MessageType::Unchoke => {
                                                has_unchoke = true;
                                                unchoked_peers.write().await.insert(handshake.remote_peer_id());
                                                timeout_counter = 0;

                                                println!("Got unchoke from peer {}", handshake.remote_peer_id());
                                            }
    
                                            MessageType::Choke => {
                                                println!("Got choked by peer {}", handshake.remote_peer_id());
                                            }
    
                                            other => {

                                                match other{
                                                    MessageType::Unknown(id,_payload )=>{
                                                        println!("Received UnKnown Message({id}) from peer {}",handshake.remote_peer_id());
                                                    }
                                                    
                                                    MessageType::Choke=>{
                                                        println!("Received Choked Message from peer {}",  handshake.remote_peer_id());

                                                        let mut msg_guard = {
                                                            conn.message.lock().await
                                                        };
                                                        if let Err(e) = msg_guard.interested().await { 
                                                            eprintln!("Failed to send interested message: {e}");
                                                            return;
                                                        }
                                                        sent_interested = true;               
                                                    }

                                                    o=>{
                                                        println!("Received unhandled message type {o:?} from peer {}",  handshake.remote_peer_id());
                                                    }
                                                }
                                                timeout_counter = 0;
                                            }
                                        }
    
                                        // If we got unchoke, we can work with this peer
                                        if has_unchoke && (has_bitfield || has_have) {
                                            println!("SUCCESS: Got unchoke from {} - peer is ready for download!", handshake.remote_peer_id());
                                            break;
                                        }
                                        
                                    }
                                    Err(e) => {
                                        eprintln!("Error exchanging message with {}: {e}", handshake.remote_peer_id());
                                        break;                        
                                    }
                                }
    
                                // Increment timeout counter
                                timeout_counter += 1;
                                
                                // Check timeout
                                if timeout_counter >= MAX_TIMEOUT {
                                    println!("TIMEOUT ON CONNECTION: No progress from peer {} for {} seconds, giving up", handshake.remote_peer_id(), MAX_TIMEOUT);
                                    break;
                                }

                                // Small delay to prevent busy waiting
                                tokio::time::sleep(Duration::from_millis(100)).await;
                            }    
    
                            if !(timeout_counter >= MAX_TIMEOUT) && (has_unchoke && (has_bitfield ||  has_have)) {
                                let mut connections = connections.lock().await;
                                
                                connections.push(conn);
                            }
                        }
    
                        Err(e) => {
                            eprintln!("Error occurred on initiating handshake: {e:?}. Peer: {}", conn.bitfield_info.peer_id);
                            return;
                        }
                    },
    
                    Err(e) => {
                        eprintln!("Failed to establish connection while connecting to peer: {e:?}");
                        return;
                    }
                }

                drop(_permit);
            }));
        }
    
        for task in tasks {
            let _ = task.await;
        }

        // To receive a call from PeerConnection, to send the progress info through the channel.
        if self.progress_tx.clone().is_some() {
            let this = Arc::new(self.clone());
            this.progress_listener().await;
        }
    
        self.info_hash = info_hash.iter().map(|b| format!("{:02X}", b)).collect()
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
            if peers.len() > 2 {
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
        // which pieces each peer will download
        self.peer_pieces_selection().await;
                
        if self.peer_pieces.is_empty() {
            println!("ERROR: No peers with pieces available. Cannot start download.");
            return;
        }
        
        let total_pieces = (file_size + piece_len - 1) / piece_len;
    
        //<<  Set up shared state for download tasks
        let completed_pieces = Arc::clone(&self.completed_pieces);
        // let unchoked_peers = Arc::clone(&self.unchoked_peers);
        let mut tasks = Vec::new();
    
        let destination = Arc::new(self.destination.clone());
        let pieces = Arc::new(pieces.clone());
        let connections = self.connections.clone();
        //>>
    
        //  Initialize download manager
        let download_manager = Arc::clone(&self.download_manager);
        {
            download_manager
                .lock()
                .await
                .update_state_downloading()
                .await;
        }
    
        let semaphore = Arc::new(Semaphore::new(5)); // Limit concurrent downloads
    
        for (peer, indexes) in &self.peer_pieces.clone() {
            let connections = Arc::clone(&connections);
            let destination = Arc::clone(&destination);
            let completed_pieces = Arc::clone(&completed_pieces);
            //let unchoked_peers = Arc::clone(&unchoked_peers);
            let pieces = Arc::clone(&pieces);
            let download_manager = Arc::clone(&download_manager);
            let semaphore = semaphore.clone();
    
            let indexes = indexes.clone();
            let peer = peer.clone();

            let info_hash = self.info_hash.clone();
            let peer_pieces = self.peer_pieces.clone();

    
            tasks.push(tokio::spawn(async move {
                let _permit = semaphore.acquire().await.expect("Could not acquire semaphore");
    
                for index in indexes {
                    println!("\n___________index {index}_________________\n");

                    // let should_continue = 
                    {
                    let dl_state = { download_manager.lock().await.dl_state.read().await.clone() };
                    match dl_state {
                        DownloadState::Paused=>{
                            // println!("Paused inside final peer msg");
                            // while {
                            //     let state = download_manager.lock().await.dl_state.read().await.clone();
                            //     state == DownloadState::Paused
                            // }{
                            //     // tokio::time::sleep(Duration::from_millis(100)).await;
                            //     println!("Waiting for Resume state");

                            // }
                            // true

                            loop{
                                let state = {
                                    let dm = download_manager.lock().await;
                                    dm.dl_state.read().await.clone()
                                };

                                if state != DownloadState::Paused{
                                    break;
                                }

                                // tokio::time::sleep(Duration::from_millis(100)).await;
                                println!("Waiting for Resume state")
                            }

                            true
                        }
                        // DownloadState::Resumed => {
                        //     // load download state
                        //     // start download
                        //     // update state to downloading
                        //     {
                        //         let dm = download_manager.lock().await;
                        //         let mut state = dm.dl_state.write().await;
                        //         *state = DownloadState::Downloading;
                        //     }
                        // }

                        DownloadState::Stopped => {
                            println!("stopped inside final peere msg");
                            // Save download state
                            let dm = download_manager.lock().await;
                            if let Err(e) = dm
                                .save_download_state(
                                    completed_pieces.read().await.clone(),
                                    destination.to_string(),
                                    info_hash,
                                    peer_pieces,
                                )
                                .await
                            {
                                *dm.dl_state.write().await = DownloadState::Error;
                                eprintln!("Error occured while saving downloads state. More: {e}");
                            };

                            return;
                        }

                        DownloadState::Completed => return,
                        DownloadState::Error => {
                            eprintln!("DownloadState has an error.");
                            false
                        }

                        // This is for `DownloadState::Downloading` and `DownloadState::Initialized`
                        // No need to interfere in those state.
                        _ => true,
                    }
                };

                // if !should_continue{
                //     while {
                //         let state = download_manager.lock().await.dl_state.read().await.clone();
                //         state == DownloadState::Paused
                //     }{
                //         println!("Waiting for Resume state");
                //         tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                //     }
                //     continue;
                // }

                    // Add timeout for each piece download
                    match tokio::time::timeout(Duration::from_secs(300), async {
                        let mut connections = connections.lock().await;
                        if let Some(conn) = connections
                            .iter_mut()
                            .find(|x| x.bitfield_info.peer_id == *peer)
                        {
                            // Verify peer has this piece
                            if !conn.bitfield_info.present_pieces.contains(&index) {
                                println!("This peer doesnot have requested index {index}. Returned");
                                return Ok(());
                            }

                            // Calculate piece length
                            let actual_piece_length = if index as usize == total_pieces - 1 {
                                file_size - (index as usize * piece_len)
                            } else {
                                piece_len
                            };

                            // Download the piece
                            conn.download_piece_blocks(
                                index,
                                &pieces,
                                actual_piece_length,
                                &destination,
                                piece_len as u32,
                                Some("download".to_string()),
                            ).await
                        } else {
                            eprintln!("ERROR: Could not find connection for peer {}", peer);
                            return Ok(());
                        }
                    }).await {
                        Ok(Ok(_)) => {
                            // Success
                            let mut completed = completed_pieces.write().await;
                            completed.insert(index);

                            println!("Piece {} downloaded from peer {}", index, peer);
                        }
                        Ok(Err(e)) => {
                            eprintln!("ERROR: Failed to download piece {} from peer {}: {e}", index, peer);
                        }
                        Err(_) => {
                            eprintln!("TIMEOUT: Piece {} download timed out for peer {}", index, peer);
                        }
                    }
                }
    
                drop(_permit);
            }));
        }
    

            let mut completed_count = 0;
            let mut failed_count = 0;
            
            for task in tasks {
                match task.await {
                    Ok(_) => {
                        completed_count += 1;
                    }
                    Err(e) => {
                        failed_count += 1;
                        eprintln!("Download task failed: {e}");
                    }
                }
            }
            
            // Final status and cleanup
            {
                let completed = completed_pieces.read().await;
                println!("__Download summary: {} pieces completed, {} tasks succeeded, {} tasks failed__ at {}", 
                        completed.len(), completed_count, failed_count, destination);
            }
            
            // Update download manager state
            {
                let  dm = download_manager.lock().await;
                if completed_pieces.read().await.len() >= total_pieces {
                    dm.update_state_completed().await;
                } else {
                    dm.update_state_stopped().await;
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

    use std::env::temp_dir;
    use std::path::Path;

    #[tokio::test]
    async fn connection() {
        let meta: TorrentFile = TorrentFile::new();
        // let encoded_data = meta.read_file(Path::new("./sample.torrent")).unwrap();
        let encoded_data = meta
            .read_file(Path::new(
                "/home/sauhardha-kafle/Desktop/ubuntu-25.04-desktop-amd64.iso.torrent",
            ))
            .unwrap();
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

        let file_size =             length.unwrap().clone();
        let piece_length =             meta.info.piece_length;

        dm.connect_and_exchange_bitfield(
            ip_addr,
            info_hash.to_vec(),
            self_peer_id.as_bytes().to_vec(),
            file_size,
            piece_length,
        )
        .await;


        let path = temp_dir().join("testing.iso").to_string_lossy().to_string();
        dm.destination(path)
            .final_peer_msg(
                file_size,
                &meta.info.pieces_hashes(),
                piece_length,
            )
            .await;
    }
}
