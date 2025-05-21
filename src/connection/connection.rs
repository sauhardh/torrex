use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

use std::net::SocketAddrV4;

use crate::handshake::Handshake;
use crate::handshake::HandshakeReply;
use crate::message::Messages;

#[derive(Debug)]
pub struct Connection<'a> {
    stream: TcpStream,
    handshake: Option<Handshake<'a>>,
    handshake_reply: Option<HandshakeReply>,
    messages: Option<Messages>,
}

impl<'a> Connection<'a> {
    /// Start `TCP` connection
    pub async fn start_connection(addr: String) -> Result<Self, Box<dyn std::error::Error>> {
        // Starts the connection
        let addr = addr.parse::<SocketAddrV4>()?;

        let stream = TcpStream::connect(addr).await?;
        Ok(Self {
            stream,
            handshake: None,
            handshake_reply: None,
            messages: None,
        })
    }

    pub async fn start_handshaking(
        &mut self,
        info_hash: &'a Vec<u8>,
        peer_id: &'a [u8],
    ) -> Result<&mut Self, Box<dyn std::error::Error>> {
        // Initialize the handshaking struct.
        self.handshake = Some(Handshake::init(info_hash, peer_id));

        // For handshaking, convert handshake data into bytes and write it into the stream.
        let buf: Vec<u8> = self.handshake.as_mut().unwrap().to_bytes();
        self.stream.write_all(&buf).await?;

        // For handshaking, after sending handshaking data, should get data in same format
        // as a reply and completion of handshaking process.
        self.handshake_reply = Some(
            self.handshake
                .as_mut()
                .unwrap()
                .handshake_reply(&mut self.stream)
                .await?,
        );

        Ok(self)
    }

    pub async fn peer_messages(
        &mut self,
        piece_len: usize,
        file_size: usize,
        pieces: &Vec<String>,
        file_path: &String,
        requested_piece_idx: Option<u32>,
    ) {
        // This waits for bitfield message from peer indicating which pieces it has.

        let msg = self::Messages::start_exchange(&mut self.stream)
            .await
            .unwrap();

        // This sends the Message Type Interested
        msg.interested(&mut self.stream).await;
        let mut unchoke_msg: Option<Messages> = None;

        // This waits for unchoke message from peer, until then it cannot proceed.
        while unchoke_msg.is_none() || unchoke_msg != Some(Messages::Unchoke) {
            unchoke_msg = Some(msg.wait_unchoke(&mut self.stream).await);
        }

        // This is for downloading all pieces
        msg.request_and_receive_pieces(
            &mut self.stream,
            piece_len,
            file_size,
            &pieces,
            file_path,
            requested_piece_idx,
        )
        .await;

        // receive pieces right after request
        // let piece = msg.wait_pieces(&mut self.stream).await;
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
        let peer_id = random::generate_peerid();

        let mut peers = Peers::new();
        let params = &peers.request.new(
            info_hash.to_vec(),
            peer_id.clone(), // random string
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

        for addr in ip_addr {
            match Connection::start_connection(addr)
                .await
                .unwrap()
                .start_handshaking(&info_hash.to_vec(), peer_id.as_bytes())
                .await
            {
                Ok(conn) => {
                    let peer_id: String = conn
                        .handshake_reply
                        .as_mut()
                        .unwrap()
                        .peer_id
                        .iter()
                        .map(|b| format!("{:02x}", b))
                        .collect();

                    // Hardcode value for temporary use
                    let file_path = "/tmp/tasty.txt".to_string();

                    // To explicitly request particular piece to download
                    let requested_piece_idx = None;

                    conn.peer_messages(
                        meta.info.piece_length,
                        _length.unwrap().clone(),
                        &meta.info.pieces_hashes(),
                        &file_path,
                        requested_piece_idx,
                    )
                    .await;

                    break;
                }
                Err(err) => {
                    eprintln!("Error occured while securing connection: {err:?}");
                }
            }
        }
    }
}
