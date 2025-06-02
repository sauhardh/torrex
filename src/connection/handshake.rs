use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;

#[derive(Debug)]
#[repr(C)]
pub struct Handshake {
    /// Length of Protocol string `Bittorrent Protocol`, which is of length `19`, 1 Byte.
    length: u8,
    /// the string `BitTorrent protocol` (19 bytes)
    string: String,
    /// 8 reserved bytes which are all initialized to 0 (8 bytes)
    reserved: Vec<u8>,
    /// sh1 info hash (20 bytes)
    info_hash: Vec<u8>,
    /// peer id, generate random 20 bytes value
    pub peer_id: Vec<u8>,
}

impl Handshake {
    /// Initialize the struct with parameter required for handshaking
    pub fn init(info_hash: Vec<u8>, peer_id: Vec<u8>) -> Self {
        Self {
            length: 19,
            string: "BitTorrent protocol".to_string(),
            reserved: vec![0; 8],
            info_hash,
            peer_id,
        }
    }

    /// Converts all the fields into the bytes
    ///
    /// Returns Vector of `u8` bytes in the order of `length`,`string`,`reserved`, `info_hash`, `peer_id`.
    #[inline]
    pub fn to_bytes(&self) -> Vec<u8> {
        // Handshake with its content has size of 68.
        let mut buffer: Vec<u8> = Vec::with_capacity(68);

        buffer.push(self.length);
        buffer.extend_from_slice(self.string.as_bytes());
        buffer.extend_from_slice(&self.reserved);
        buffer.extend_from_slice(&self.info_hash);
        buffer.extend_from_slice(&self.peer_id);

        buffer
    }

    pub async fn handshake_reply(
        &self,
        stream: &mut TcpStream,
    ) -> Result<Handshake, Box<dyn std::error::Error + Send + Sync>> {
        let response = self.receive_handshake(stream).await?;
        let reply = self.parse_handshake(&response);

        Ok(reply)
    }

    #[inline]
    pub async fn receive_handshake(
        &self,
        stream: &mut TcpStream,
    ) -> Result<[u8; 68], Box<dyn std::error::Error + Send + Sync>> {
        let mut buf = [0u8; 68];
        stream.read_exact(&mut buf).await?;

        Ok(buf)
    }

    #[inline]
    pub fn parse_handshake(&self, buf: &[u8]) -> Handshake {
        if buf.len() != 68 {
            eprintln!(
                "Could not parse handshake buffer. Got incorrect buffer length of {}",
                buf.len()
            );
        }
        let length = buf[0];
        let string = String::from_utf8(buf[1..20].to_vec()).unwrap();
        let reserved = buf[20..28].to_vec();
        let info_hash = buf[28..48].to_vec();
        let peer_id = buf[48..].to_vec();

        let reply = Handshake {
            length,
            string,
            reserved,
            info_hash,
            peer_id,
        };

        reply
    }
}
