use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default, Debug)]
pub struct MagnetLink {
    /// urn:btih: followed by the 40-char hex-encoded info hash
    pub xt: String,
    /// The name of the file to be downloaded (example: magnet1.gif)
    pub dn: Option<String>,
    /// The tracker URL (example: http://bittorrent-tracker-torrex.io/announce)
    pub tr: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Default)]
/// To parse the magnet link
/// Example of link, v1: magnet:?xt=urn:btih:<info-hash>&dn=<name>&tr=<tracker-url>&x.pe=<peer-address>
pub struct Parser {
    pub magnet_link: MagnetLink,
    pub link: String,
}

impl Parser {
    pub fn new(link: String) -> Self {
        Self {
            magnet_link: MagnetLink::default(),
            link,
        }
    }

    #[inline]
    fn verify_magnet(&mut self) -> bool {
        if self.link.starts_with("magnet:?") {
            return true;
        }
        false
    }

    pub fn parse(&mut self) -> &mut Self {
        if !self.verify_magnet() {
            eprintln!("Provided is not the magnet link.");
        }

        let query = self.link.trim_start_matches("magnet:?");
        for part in query.split('&') {
            if let Some((key, val)) = part.split_once('=') {
                let val = urlencoding::decode(val).unwrap_or_default().to_string();
                match key {
                    "xt" => self.magnet_link.xt = val.trim_start_matches("urn:btih:").to_string(),
                    "dn" => self.magnet_link.dn = Some(val),
                    "tr" => self.magnet_link.tr = Some(val),
                    _ => {
                        println!("Not Implemented. Incorrect Argument {:?}", key);
                    }
                }
            }
        }

        self
    }
}

#[cfg(test)]
mod test_parser {
    use super::*;

    #[test]
    fn parse() {
        // let link = "magnet:?xt=urn:btih:ad42ce8109f54c99613ce38f9b4d87e70f24a165&dn=magnet1.gif&tr=http%3A%2F%2Fbittorrent-test-tracker.codecrafters.io%2Fannounce";
        let link = "magnet:?xt=urn:btih:4344503b7e797ebf31582327a5baae35b11bda01&dn=ubuntu-16.04-desktop-amd64.iso&tr=http%3A%2F%2Ftorrent.ubuntu.com%3A6969%2Fannounce&tr=http%3A%2F%2Fipv6.torrent.ubuntu.com%3A6969%2Fannounce";
        let mut parser = Parser::new(link.to_string());
        let parser = parser.parse();
        println!("parser: {:#?}", parser);
    }
}
