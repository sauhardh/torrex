use std::net::SocketAddrV4;

use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

#[derive(Debug)]
#[repr(C)]
pub struct Handshake<'a> {
    /// Length of Protocol string `Bittorrent Protocol`, which is of length `19`, 1 Byte.
    length: u8,
    /// the string `BitTorrent protocol` (19 bytes)
    string: &'a str,
    /// 8 reserved bytes which are all initialized to 0 (8 bytes)
    reserved: Vec<u8>,
    /// sh1 info hash (20 bytes)
    info_hash: &'a Vec<u8>,
    /// peer id, generate random 20 bytes value
    peer_id: &'a [u8],
}

impl<'a> Handshake<'a> {
    /// Initialize the struct with parameter required for handshaking
    pub fn init(info_hash: &'a Vec<u8>, peer_id: &'a [u8]) -> Self {
        Self {
            length: 19,
            string: "BitTorrent protocol",
            reserved: vec![0; 8],
            info_hash,
            peer_id,
        }
    }

    /// Converts all the fields into the bytes
    ///
    /// Returns Vector of `u8` bytes in the order of `length`,`string`,`reserved`, `info_hash`, `peer_id`.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buffer: Vec<u8> = Vec::with_capacity(68);

        buffer.push(self.length);
        buffer.extend_from_slice(self.string.as_bytes());
        buffer.extend_from_slice(&self.reserved);
        buffer.extend_from_slice(&self.info_hash);
        buffer.extend_from_slice(self.peer_id);

        buffer
    }

    /// Start `TCP` connection for handshaking with peers
    pub async fn perform(
        &self,
        addr: String,
    ) -> Result<HandshakeReply, Box<dyn std::error::Error>> {
        let addr = addr.parse::<SocketAddrV4>()?;
        let mut stream = TcpStream::connect(addr).await?;

        let buf = self.to_bytes();
        stream.write_all(&buf).await?;

        let response = self.receive_handshake(&mut stream).await?;
        let reply = self.parse_handshake(&response);

        Ok(reply)
    }

    #[inline]
    pub async fn receive_handshake(
        &self,
        stream: &mut TcpStream,
    ) -> Result<[u8; 68], Box<dyn std::error::Error>> {
        let mut buf = [0u8; 68];
        stream.read_exact(&mut buf).await?;

        Ok(buf)
    }

    #[inline]
    pub fn parse_handshake(&self, buf: &[u8]) -> HandshakeReply {
        let length = buf[0];
        let string = String::from_utf8(buf[1..20].to_vec()).unwrap();
        let reserved = buf[20..28].to_vec();
        let info_hash = buf[28..48].to_vec();
        let peer_id = buf[48..].to_vec();

        let reply = HandshakeReply {
            length,
            string,
            reserved,
            info_hash,
            peer_id,
        };

        reply
    }
}

#[derive(Debug)]
pub struct HandshakeReply {
    length: u8,
    string: String,
    reserved: Vec<u8>,
    info_hash: Vec<u8>,
    peer_id: Vec<u8>,
}

#[cfg(test)]

mod test_handshake {
    use super::*;

    use crate::metainfo::FileKey;
    use crate::metainfo::TorrentFile;
    use crate::peers::Peers;
    use crate::utils::random;

    use std::path::Path;

    #[tokio::test]
    async fn handshake() {
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
            match Handshake::init(&params.info_hash, peer_id.as_bytes())
                .perform(addr)
                .await
            {
                Ok(received) => {
                    let hex_peer_id: String = received
                        .peer_id
                        .iter()
                        .map(|b| format!("{:02x}", b))
                        .collect();

                    println!("Peer ID: {}", hex_peer_id);
                }
                Err(err) => {
                    println!("received:{}", err)
                }
            };
        }
    }
}
